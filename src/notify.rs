use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use worker::{
    console_warn, wasm_bindgen::JsValue, D1Database, Headers, Method, Request, RequestInit, Result,
};

use crate::auth::sha256_hex;
use crate::db::{self, SettingsView, SECRET_MASK};
use crate::live;
use crate::models::TelegramNotificationInput;
use crate::outbound::{fetch_with_timeout, read_response_limited};

const NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(8);
const NOTIFICATION_MAX_BYTES: usize = 64 * 1024;
const MAX_DELIVERY_ATTEMPTS: i64 = 6;
const DELIVERY_BATCH_SIZE: i64 = 5;
const TEMPLATE_KEYS: [&str; 7] = [
    "event",
    "title",
    "server",
    "message",
    "time",
    "value",
    "threshold",
];

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
struct AlertState {
    offline: HashSet<String>,
    expiry: HashSet<String>,
    resources: HashSet<String>,
    traffic: HashMap<String, TrafficAlertState>,
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
struct TrafficAlertState {
    cycle_key: i64,
    step: i64,
}

#[derive(Clone, Deserialize)]
struct TelegramSettingsRow {
    bot_token: String,
    chat_id: String,
    message_thread_id: Option<i64>,
    template: String,
}

#[derive(Serialize)]
pub struct TelegramSettingsView {
    bot_token: String,
    chat_id: String,
    message_thread_id: Option<i64>,
    template: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TelegramConfig {
    bot_token: String,
    chat_id: String,
    #[serde(default)]
    message_thread_id: Option<i64>,
}

#[derive(Serialize)]
struct NewEvent {
    id: String,
    dedupe_key: String,
    kind: String,
    server_id: Option<String>,
    server_name: String,
    title: String,
    message: String,
    details: Value,
    occurred_at: i64,
}

struct EventSource<'a> {
    server_id: Option<&'a str>,
    server_name: &'a str,
}

#[derive(Deserialize)]
struct PendingDeliveryRow {
    event_id: String,
    attempts: i64,
    kind: String,
    server_name: String,
    title: String,
    message: String,
    details_json: String,
    occurred_at: i64,
    bot_token: String,
    chat_id: String,
    message_thread_id: Option<i64>,
    template: String,
}

fn text(value: &str) -> JsValue {
    JsValue::from_str(value)
}

fn number(value: impl ToString) -> JsValue {
    JsValue::from_f64(value.to_string().parse::<f64>().unwrap_or(0.0))
}

fn event(
    dedupe_key: String,
    kind: &str,
    source: EventSource<'_>,
    title: String,
    message: String,
    details: Value,
    occurred_at: i64,
) -> NewEvent {
    NewEvent {
        id: sha256_hex(&dedupe_key).chars().take(32).collect(),
        dedupe_key,
        kind: kind.to_string(),
        server_id: source.server_id.map(str::to_string),
        server_name: source.server_name.to_string(),
        title,
        message,
        details,
        occurred_at,
    }
}

fn valid_telegram_token(value: &str) -> bool {
    let value = value.trim();
    value.split_once(':').is_some_and(|(bot_id, secret)| {
        !bot_id.is_empty()
            && bot_id.chars().all(|ch| ch.is_ascii_digit())
            && secret.len() >= 20
            && secret
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    })
}

fn validate_template(template: &str) -> Option<String> {
    if template.trim().is_empty() || template.chars().count() > 4000 {
        return Some("通知模板长度应为 1 至 4000 个字符".to_string());
    }
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        if rest[..start].contains("}}") {
            return Some("通知模板占位符格式无效".to_string());
        }
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Some("通知模板占位符未闭合".to_string());
        };
        let placeholder = &after[..end];
        let key = placeholder.trim();
        if placeholder != key || !TEMPLATE_KEYS.contains(&key) {
            return Some(format!("通知模板包含不支持的占位符：{{{{{key}}}}}"));
        }
        rest = &after[end + 2..];
    }
    if rest.contains("}}") {
        return Some("通知模板占位符格式无效".to_string());
    }
    None
}

fn normalized_telegram(
    input: &TelegramNotificationInput,
    previous: Option<&TelegramSettingsRow>,
) -> std::result::Result<TelegramConfig, String> {
    if let Some(message) = validate_template(&input.template) {
        return Err(message);
    }
    let bot_token = if input.bot_token.trim() == SECRET_MASK {
        let Some(previous) = previous else {
            return Err("Telegram Bot Token 不能为空".to_string());
        };
        previous.bot_token.clone()
    } else {
        input.bot_token.trim().to_string()
    };
    let chat_id = input.chat_id.trim().to_string();
    if !valid_telegram_token(&bot_token)
        || chat_id.is_empty()
        || chat_id.chars().count() > 128
        || input.message_thread_id.is_some_and(|value| value <= 0)
    {
        return Err("Telegram Bot Token、Chat ID 或话题 ID 格式无效".to_string());
    }
    Ok(TelegramConfig {
        bot_token,
        chat_id,
        message_thread_id: input.message_thread_id,
    })
}

async fn telegram_row(db_conn: &D1Database) -> Result<Option<TelegramSettingsRow>> {
    db_conn
        .prepare(
            "SELECT bot_token, chat_id, message_thread_id, template \
             FROM notification_telegram WHERE id=1",
        )
        .first(None)
        .await
}

pub async fn telegram_settings(db_conn: &D1Database) -> Result<Option<TelegramSettingsView>> {
    Ok(telegram_row(db_conn)
        .await?
        .map(|row| TelegramSettingsView {
            bot_token: SECRET_MASK.to_string(),
            chat_id: row.chat_id,
            message_thread_id: row.message_thread_id,
            template: row.template,
        }))
}

pub async fn save_telegram(
    db_conn: &D1Database,
    input: &TelegramNotificationInput,
    timestamp: i64,
) -> Result<std::result::Result<(), String>> {
    let previous = telegram_row(db_conn).await?;
    let config = match normalized_telegram(input, previous.as_ref()) {
        Ok(value) => value,
        Err(message) => return Ok(Err(message)),
    };
    db_conn
        .prepare(
            "INSERT INTO notification_telegram( \
               id, bot_token, chat_id, message_thread_id, template, updated_at \
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(id) DO UPDATE SET bot_token=excluded.bot_token, chat_id=excluded.chat_id, \
               message_thread_id=excluded.message_thread_id, template=excluded.template, \
               updated_at=excluded.updated_at",
        )
        .bind(&[
            text(&config.bot_token),
            text(&config.chat_id),
            config
                .message_thread_id
                .map(number)
                .unwrap_or(JsValue::NULL),
            text(input.template.trim()),
            number(timestamp),
        ])?
        .run()
        .await?;
    Ok(Ok(()))
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn template_values(
    kind: &str,
    title: &str,
    server: &str,
    message: &str,
    time: &str,
    details: &Value,
) -> Vec<(&'static str, String)> {
    vec![
        ("event", kind.to_string()),
        ("title", title.to_string()),
        ("server", server.to_string()),
        ("message", message.to_string()),
        ("time", time.to_string()),
        (
            "value",
            details.get("value").map(value_text).unwrap_or_default(),
        ),
        (
            "threshold",
            details.get("threshold").map(value_text).unwrap_or_default(),
        ),
    ]
}

fn render_template(template: &str, values: &[(&str, String)]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            output.push_str(&rest[start..]);
            return output;
        };
        let key = &after[..end];
        if let Some((_, value)) = values.iter().find(|(candidate, _)| *candidate == key) {
            output.push_str(value);
        } else {
            output.push_str(&rest[start..start + end + 4]);
        }
        rest = &after[end + 2..];
    }
    output.push_str(rest);
    output
}

fn iso_time(timestamp: i64) -> String {
    let value = worker::js_sys::Date::new(&JsValue::from_f64(timestamp as f64 * 1000.0));
    value
        .to_iso_string()
        .as_string()
        .unwrap_or_else(|| timestamp.to_string())
}

async fn send_request(url: &str, headers: Headers, body: &str) -> Result<()> {
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(body)));
    let request = Request::new_with_init(url, &init)
        .map_err(|_| worker::Error::RustError("通知服务请求无效".to_string()))?;
    let Some(mut response) = fetch_with_timeout(request, NOTIFICATION_TIMEOUT)
        .await
        .map_err(|_| worker::Error::RustError("通知服务请求失败".to_string()))?
    else {
        return Err(worker::Error::RustError("通知服务请求超时".to_string()));
    };
    let status = response.status_code();
    read_response_limited(&mut response, NOTIFICATION_MAX_BYTES)
        .await?
        .ok_or_else(|| worker::Error::RustError("通知服务响应体大小无效".to_string()))?;
    if !(200..300).contains(&status) {
        return Err(worker::Error::RustError(format!(
            "通知服务返回 HTTP {status}"
        )));
    }
    Ok(())
}

async fn send_delivery(row: &PendingDeliveryRow) -> Result<()> {
    let details = serde_json::from_str::<Value>(&row.details_json).unwrap_or_else(|_| json!({}));
    let occurred_at = iso_time(row.occurred_at);
    let values = template_values(
        &row.kind,
        &row.title,
        &row.server_name,
        &row.message,
        &occurred_at,
        &details,
    );
    let rendered_message = render_template(&row.template, &values);
    let headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    let mut body = json!({ "chat_id": row.chat_id, "text": rendered_message });
    if let Some(message_thread_id) = row.message_thread_id {
        body["message_thread_id"] = json!(message_thread_id);
    }
    send_request(
        &format!("https://api.telegram.org/bot{}/sendMessage", row.bot_token),
        headers,
        &body.to_string(),
    )
    .await
}

fn retry_delay(attempts: i64) -> i64 {
    match attempts {
        0 | 1 => 60,
        2 => 300,
        3 => 900,
        _ => 3600,
    }
}

async fn attempt_delivery(
    db_conn: &D1Database,
    row: &PendingDeliveryRow,
    timestamp: i64,
) -> Result<Option<String>> {
    let attempts = row.attempts + 1;
    match send_delivery(row).await {
        Ok(()) => {
            db_conn
                .prepare(
                    "UPDATE notification_deliveries SET status='sent', attempts=?2, sent_at=?3, \
                     last_error=NULL, updated_at=?3 WHERE event_id=?1",
                )
                .bind(&[text(&row.event_id), number(attempts), number(timestamp)])?
                .run()
                .await?;
            Ok(None)
        }
        Err(error) => {
            let message = error.to_string().chars().take(300).collect::<String>();
            let status = if attempts >= MAX_DELIVERY_ATTEMPTS {
                "dead"
            } else {
                "failed"
            };
            db_conn
                .prepare(
                    "UPDATE notification_deliveries SET status=?2, attempts=?3, next_attempt_at=?4, \
                     last_error=?5, updated_at=?6 WHERE event_id=?1",
                )
                .bind(&[
                    text(&row.event_id),
                    text(status),
                    number(attempts),
                    number(timestamp.saturating_add(retry_delay(attempts))),
                    text(&message),
                    number(timestamp),
                ])?
                .run()
                .await?;
            Ok(Some(message))
        }
    }
}

const DELIVERY_SELECT: &str =
    "SELECT d.event_id, d.attempts, e.kind, e.server_name, e.title, e.message, \
            e.details_json, e.occurred_at, t.bot_token, t.chat_id, t.message_thread_id, t.template \
     FROM notification_deliveries d \
     JOIN notification_events e ON e.id=d.event_id \
     JOIN notification_telegram t ON t.id=1";

pub async fn process_pending(db_conn: &D1Database, timestamp: i64) -> Result<usize> {
    let rows: Vec<PendingDeliveryRow> = db_conn
        .prepare(format!(
            "{DELIVERY_SELECT} WHERE d.status IN ('pending', 'failed') AND d.next_attempt_at<=?1 \
             ORDER BY d.created_at ASC LIMIT ?2"
        ))
        .bind(&[number(timestamp), number(DELIVERY_BATCH_SIZE)])?
        .all()
        .await?
        .results()?;
    let count = rows.len();
    for row in rows {
        if let Some(message) = attempt_delivery(db_conn, &row, timestamp).await? {
            console_warn!("telegram notification delivery failed: {message}");
        }
    }
    Ok(count)
}

pub async fn test_telegram(
    db_conn: &D1Database,
    test_id: &str,
    timestamp: i64,
) -> Result<std::result::Result<(), String>> {
    if telegram_row(db_conn).await?.is_none() {
        return Ok(Err("尚未保存 Telegram 配置".to_string()));
    };
    let test_event = event(
        format!("test:telegram:{test_id}"),
        "test",
        EventSource {
            server_id: None,
            server_name: "NodeFlare",
        },
        "NodeFlare 测试通知".to_string(),
        "Telegram 配置成功，事件投递链路工作正常。".to_string(),
        json!({}),
        timestamp,
    );
    let event_insert = db_conn
        .prepare(
            "INSERT INTO notification_events( \
               id, dedupe_key, kind, server_id, server_name, title, message, details_json, occurred_at, created_at \
             ) VALUES (?1, ?2, 'test', NULL, ?3, ?4, ?5, '{}', ?6, ?6)",
        )
        .bind(&[
            text(&test_event.id),
            text(&test_event.dedupe_key),
            text(&test_event.server_name),
            text(&test_event.title),
            text(&test_event.message),
            number(timestamp),
        ])?;
    let delivery_insert = db_conn
        .prepare(
            "INSERT INTO notification_deliveries( \
               event_id, status, attempts, next_attempt_at, created_at, updated_at \
             ) VALUES (?1, 'pending', 0, ?2, ?2, ?2)",
        )
        .bind(&[text(&test_event.id), number(timestamp)])?;
    db_conn.batch(vec![event_insert, delivery_insert]).await?;
    let row = db_conn
        .prepare(format!("{DELIVERY_SELECT} WHERE d.event_id=?1"))
        .bind(&[text(&test_event.id)])?
        .first::<PendingDeliveryRow>(None)
        .await?
        .ok_or_else(|| worker::Error::RustError("测试投递记录创建失败".to_string()))?;
    match attempt_delivery(db_conn, &row, timestamp).await? {
        Some(message) => Ok(Err(message)),
        None => Ok(Ok(())),
    }
}

async fn has_telegram(db_conn: &D1Database) -> Result<bool> {
    Ok(db_conn
        .prepare("SELECT 1 AS found FROM notification_telegram WHERE id=1")
        .first::<i64>(Some("found"))
        .await?
        .is_some())
}

async fn notification_state(db_conn: &D1Database) -> Result<AlertState> {
    let raw = db_conn
        .prepare("SELECT value FROM notification_state WHERE id=1")
        .first::<String>(Some("value"))
        .await?
        .unwrap_or_else(|| "{}".to_string());
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

async fn persist_events_and_state(
    db_conn: &D1Database,
    events: &[NewEvent],
    state: &AlertState,
    timestamp: i64,
) -> Result<()> {
    let state_json = serde_json::to_string(state)?;
    let state_update = db_conn
        .prepare("UPDATE notification_state SET value=?1, updated_at=?2 WHERE id=1")
        .bind(&[text(&state_json), number(timestamp)])?;
    if events.is_empty() {
        state_update.run().await?;
        return Ok(());
    }
    let events_json = serde_json::to_string(events)?;
    let event_insert = db_conn
        .prepare(
            "INSERT OR IGNORE INTO notification_events( \
               id, dedupe_key, kind, server_id, server_name, title, message, details_json, occurred_at, created_at \
             ) SELECT json_extract(value, '$.id'), json_extract(value, '$.dedupe_key'), \
                    json_extract(value, '$.kind'), json_extract(value, '$.server_id'), \
                    json_extract(value, '$.server_name'), json_extract(value, '$.title'), \
                    json_extract(value, '$.message'), json_extract(value, '$.details'), \
                    json_extract(value, '$.occurred_at'), ?2 FROM json_each(?1)",
        )
        .bind(&[text(&events_json), number(timestamp)])?;
    let delivery_insert = db_conn
        .prepare(
            "INSERT OR IGNORE INTO notification_deliveries( \
               event_id, status, attempts, next_attempt_at, created_at, updated_at \
             ) SELECT e.id, 'pending', 0, ?2, ?2, ?2 \
             FROM json_each(?1) j JOIN notification_events e ON e.id=json_extract(j.value, '$.id') \
             WHERE EXISTS (SELECT 1 FROM notification_telegram WHERE id=1)",
        )
        .bind(&[text(&events_json), number(timestamp)])?;
    db_conn
        .batch(vec![event_insert, delivery_insert, state_update])
        .await?;
    Ok(())
}

fn traffic_used(server: &crate::models::ServerView) -> Option<i64> {
    let rx = server.net_rx_total?;
    let tx = server.net_tx_total?;
    Some(match server.traffic_limit_type.as_str() {
        "max" => rx.max(tx),
        "min" => rx.min(tx),
        "up" => tx,
        "down" => rx,
        _ => rx.saturating_add(tx),
    })
}

fn traffic_step(used: i64, limit: i64, threshold: i64) -> i64 {
    if limit <= 0 {
        return 0;
    }
    let percent = ((i128::from(used.max(0)) * 100) / i128::from(limit)).min(100) as i64;
    if percent < threshold {
        0
    } else {
        threshold + ((percent - threshold) / 5) * 5
    }
}

fn gibibytes(value: i64) -> String {
    format!("{:.2} GiB", value.max(0) as f64 / 1024_f64.powi(3))
}

pub async fn renew_servers(db_conn: &D1Database) -> Result<()> {
    let current_time = crate::now();
    for server in db::list_servers(db_conn, true).await? {
        if server.auto_renewal == 0 {
            continue;
        }
        let Some(expires_at) = server.expires_at else {
            continue;
        };
        if let Some(next_expiry) = renewed_expiry(expires_at, server.billing_cycle, current_time) {
            db::update_expiry(db_conn, &server.id, next_expiry).await?;
        }
    }
    Ok(())
}

fn renewed_expiry(expires_at: i64, billing_cycle_days: i64, current_time: i64) -> Option<i64> {
    // billing_cycle = 0 是「一次性」，没有周期可续。不拦的话下面的 clamp(1, ..)
    // 会把它当成 1 天，每跑一次 cron 就把到期日推一天，节点永远停在「剩 1 天」。
    if billing_cycle_days <= 0 {
        return None;
    }
    let renew_before = i128::from(current_time) + 86_400;
    let expires_at = i128::from(expires_at);
    if expires_at > renew_before {
        return None;
    }
    let cycle = i128::from(billing_cycle_days.clamp(1, 3650)) * 86_400;
    let elapsed = renew_before - expires_at;
    let cycles = elapsed / cycle + 1;
    Some((expires_at + cycles * cycle).min(i128::from(i64::MAX)) as i64)
}

pub async fn check_alerts(
    db_conn: &D1Database,
    env: &worker::Env,
    settings: &SettingsView,
) -> Result<()> {
    if !settings.notification_enabled || !has_telegram(db_conn).await? {
        return Ok(());
    }
    let servers = db::list_servers(db_conn, true).await?;
    let previous = notification_state(db_conn).await?;
    let mut current = AlertState::default();
    let current_time = crate::now();
    let offline_after = settings.offline_alert_minutes.clamp(2, 1440) * 60;
    let mut events = Vec::new();

    for server in &servers {
        let source_timestamp = server.timestamp.unwrap_or(server.created_at);
        let is_offline = current_time.saturating_sub(source_timestamp) > offline_after;
        if server.offline_notify_disabled == 0 && is_offline {
            current.offline.insert(server.id.clone());
            if !previous.offline.contains(&server.id) {
                events.push(event(
                    format!("offline:{}:{source_timestamp}", server.id),
                    "offline",
                    EventSource {
                        server_id: Some(&server.id),
                        server_name: &server.name,
                    },
                    "服务器离线".to_string(),
                    format!(
                        "已超过 {} 分钟未收到状态上报。",
                        settings.offline_alert_minutes
                    ),
                    json!({ "threshold": settings.offline_alert_minutes }),
                    current_time,
                ));
            }
        } else if previous.offline.contains(&server.id) {
            events.push(event(
                format!("online:{}:{source_timestamp}", server.id),
                "online",
                EventSource {
                    server_id: Some(&server.id),
                    server_name: &server.name,
                },
                "服务器恢复".to_string(),
                "状态上报已恢复。".to_string(),
                json!({}),
                current_time,
            ));
        }
        if settings.expiry_alert_days > 0 {
            if let Some(expires_at) = server.expires_at {
                let days = (expires_at - current_time + 86_399) / 86_400;
                if days >= 0 && days <= settings.expiry_alert_days {
                    let key = format!("{}:{expires_at}", server.id);
                    current.expiry.insert(key.clone());
                    if !previous.expiry.contains(&key) {
                        events.push(event(
                            format!("expiry:{key}"),
                            "expiry",
                            EventSource {
                                server_id: Some(&server.id),
                                server_name: &server.name,
                            },
                            "服务器即将到期".to_string(),
                            format!("剩余 {days} 天到期。"),
                            json!({ "value": days, "threshold": settings.expiry_alert_days }),
                            current_time,
                        ));
                    }
                }
            }
        }
        if server.traffic_limit > 0 {
            if let Some(used) = traffic_used(server) {
                let cycle_key = db::traffic_cycle_key(current_time, server.reset_day);
                let step = traffic_step(
                    used,
                    server.traffic_limit,
                    settings.traffic_alert_percentage,
                );
                current
                    .traffic
                    .insert(server.id.clone(), TrafficAlertState { cycle_key, step });
                let previous_traffic = previous.traffic.get(&server.id);
                if step >= settings.traffic_alert_percentage
                    && previous_traffic
                        .is_none_or(|state| state.cycle_key != cycle_key || step > state.step)
                {
                    events.push(event(
                        format!("traffic:{}:{cycle_key}:{step}", server.id),
                        "traffic",
                        EventSource {
                            server_id: Some(&server.id),
                            server_name: &server.name,
                        },
                        "流量限额提醒".to_string(),
                        format!("本周期已使用 {} / {}（{}%）。", gibibytes(used), gibibytes(server.traffic_limit), step),
                        json!({ "value": step, "threshold": settings.traffic_alert_percentage, "used_bytes": used, "limit_bytes": server.traffic_limit }),
                        current_time,
                    ));
                }
            }
        }
    }

    let mut evaluated_resources = HashSet::new();
    let mut eligible_resources = HashSet::new();
    let rules = db::list_alert_rules(db_conn).await?;
    let mut values_by_rule = HashMap::new();
    if rules.iter().any(|rule| rule.enabled) {
        match live::evaluate_resource_alerts(env, &rules, &servers, current_time).await {
            Ok(values) => {
                for value in values {
                    values_by_rule
                        .entry(value.rule_id)
                        .or_insert_with(Vec::new)
                        .push(value.row);
                }
            }
            Err(error) => console_warn!("resource alert evaluation failed: {error:?}"),
        }
    }
    for rule in rules {
        if !rule.enabled {
            continue;
        }
        if rule.server_ids.is_empty() {
            eligible_resources.extend(
                servers
                    .iter()
                    .filter(|server| server.hidden == 0)
                    .map(|server| format!("{}:{}", rule.id, server.id)),
            );
        } else {
            eligible_resources.extend(
                rule.server_ids
                    .iter()
                    .map(|server_id| format!("{}:{server_id}", rule.id)),
            );
        }
        let (metric_label, unit) = match rule.metric.as_str() {
            "cpu" => ("CPU", "%"),
            "memory" => ("内存", "%"),
            "disk" => ("磁盘", "%"),
            "net_in" => ("下行", "MiB/s"),
            "net_out" => ("上行", "MiB/s"),
            _ => continue,
        };
        for value in values_by_rule.remove(&rule.id).unwrap_or_default() {
            if !db::alert_window_covered(&value, rule.duration_minutes, current_time) {
                continue;
            }
            let key = format!("{}:{}", rule.id, value.server_id);
            evaluated_resources.insert(key.clone());
            if value.value >= rule.threshold {
                current.resources.insert(key.clone());
                if !previous.resources.contains(&key) {
                    events.push(event(
                        format!("resource:{}:{}:{}:alert", rule.id, value.server_id, value.last_timestamp),
                        "resource_alert",
                        EventSource {
                            server_id: Some(&value.server_id),
                            server_name: &value.name,
                        },
                        format!("资源告警：{}", rule.name),
                        format!("{} {:.1}{unit}，阈值 {:.1}{unit}，{} 分钟{}。", metric_label, value.value, rule.threshold, rule.duration_minutes, if rule.aggregation == "continuous" { "持续超限" } else { "窗口平均" }),
                        json!({ "value": value.value, "threshold": rule.threshold, "rule": rule.name, "metric": rule.metric }),
                        current_time,
                    ));
                }
            } else if previous.resources.contains(&key) {
                events.push(event(
                    format!("resource:{}:{}:{}:recovery", rule.id, value.server_id, value.last_timestamp),
                    "resource_recovery",
                    EventSource {
                        server_id: Some(&value.server_id),
                        server_name: &value.name,
                    },
                    format!("资源恢复：{}", rule.name),
                    format!("{metric_label} 已恢复到阈值以内。"),
                    json!({ "value": value.value, "threshold": rule.threshold, "rule": rule.name, "metric": rule.metric }),
                    current_time,
                ));
            }
        }
    }
    for key in &previous.resources {
        if eligible_resources.contains(key) && !evaluated_resources.contains(key) {
            current.resources.insert(key.clone());
        }
    }
    if previous != current || !events.is_empty() {
        persist_events_and_state(db_conn, &events, &current, current_time).await?;
    }
    Ok(())
}

pub async fn cleanup(db_conn: &D1Database, timestamp: i64) -> Result<()> {
    let cutoff = timestamp.saturating_sub(30 * 86_400);
    db_conn
        .prepare("DELETE FROM notification_events WHERE created_at<?1")
        .bind(&[number(cutoff)])?
        .run()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{render_template, renewed_expiry, retry_delay, traffic_step, validate_template};

    #[test]
    fn renders_whitelisted_templates() {
        let values = [
            ("title", "离线".to_string()),
            ("message", "超时".to_string()),
        ];
        assert_eq!(
            render_template("{{title}}: {{message}}", &values),
            "离线: 超时"
        );
        assert!(validate_template("{{unknown}}").is_some());
        assert!(validate_template("{{ title }}").is_some());
        assert_eq!(
            render_template(
                "{{server}}",
                &[
                    ("server", "{{title}}".to_string()),
                    ("title", "告警".to_string())
                ]
            ),
            "{{title}}"
        );
    }

    #[test]
    fn traffic_steps_and_retry_backoff_are_bounded() {
        assert_eq!(traffic_step(79, 100, 80), 0);
        assert_eq!(traffic_step(80, 100, 80), 80);
        assert_eq!(traffic_step(89, 100, 80), 85);
        assert_eq!(traffic_step(120, 100, 80), 100);
        assert_eq!(
            [
                retry_delay(1),
                retry_delay(2),
                retry_delay(3),
                retry_delay(4)
            ],
            [60, 300, 900, 3600]
        );
    }

    #[test]
    fn expiry_renewal_skips_future_dates() {
        let day = 86_400;
        assert_eq!(renewed_expiry(80 * day, 30, 100 * day), Some(110 * day));
        assert_eq!(renewed_expiry(102 * day, 30, 100 * day), None);
    }

    #[test]
    fn one_time_billing_cycle_never_renews() {
        // 一次性（billing_cycle = 0）就算把自动续费开着也不该续：否则 clamp(1, ..)
        // 把周期当成 1 天，每跑一次 cron 推一天，节点永远停在「剩 1 天」。
        let day = 86_400;
        assert_eq!(renewed_expiry(80 * day, 0, 100 * day), None);
        assert_eq!(renewed_expiry(102 * day, 0, 101 * day), None);
        assert_eq!(renewed_expiry(80 * day, -5, 100 * day), None);
        // 正常周期不受影响。
        assert_eq!(renewed_expiry(80 * day, 1, 100 * day), Some(102 * day));
    }
}
