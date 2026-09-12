use crate::db::{self, Database, Settings, queries};
use crate::models::TelegramSettingsView;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

pub async fn test_telegram(db: &Database, client: &reqwest::Client) -> Result<()> {
    send(
        db,
        client,
        "NodeFlare 测试通知",
        "NodeFlare",
        "Telegram 通知配置有效。",
    )
    .await
}

pub async fn evaluate_resource_alerts(
    db: &Database,
    settings: &Settings,
    server_id: &str,
    server_name: &str,
) -> Result<()> {
    if !settings.notification_enabled {
        return Ok(());
    }
    for evaluation in queries::evaluate_resource_rules(db, server_id).await? {
        let key = format!("resource:{}:{server_id}", evaluation.rule.id);
        let details = serde_json::json!({
            "value": evaluation.value,
            "threshold": evaluation.rule.threshold,
            "metric": evaluation.rule.metric,
        });
        let unit = if evaluation.rule.metric.starts_with("net_") {
            "MiB/s"
        } else {
            "%"
        };
        let (title, message) = if evaluation.triggered {
            (
                "资源告警",
                format!(
                    "{}：{:.2} {}，阈值 {:.2} {}（{} 分钟{}）",
                    evaluation.rule.name,
                    evaluation.value,
                    unit,
                    evaluation.rule.threshold,
                    unit,
                    evaluation.rule.duration_minutes,
                    if evaluation.rule.aggregation == "continuous" {
                        "持续超限"
                    } else {
                        "平均值"
                    }
                ),
            )
        } else {
            (
                "资源恢复",
                format!(
                    "{} 已恢复，当前值 {:.2} {}",
                    evaluation.rule.name, evaluation.value, unit
                ),
            )
        };
        record_observation(
            db,
            queries::AlertObservation {
                key: &key,
                server_id,
                rule_id: Some(&evaluation.rule.id),
                active: evaluation.triggered,
                details,
                notification: Some(queries::AlertNotification {
                    title,
                    server_name,
                    message: &message,
                }),
            },
        )
        .await;
    }
    Ok(())
}

pub async fn run_periodic(
    db: &Database,
    settings: &Settings,
    last_reports: &HashMap<String, i64>,
    started_at: i64,
) -> Result<()> {
    if !settings.notification_enabled {
        return Ok(());
    }
    let current = db::now();
    for server in queries::list_servers(db, true).await? {
        if !server.offline_notify_disabled
            && let Some((offline, last_seen)) = offline_status(
                server.timestamp,
                last_reports.get(&server.id).copied(),
                started_at,
                current,
                settings.offline_alert_minutes * 60,
            )
        {
            let key = format!("offline:{}", server.id);
            let (title, message) = if offline {
                (
                    "服务器离线",
                    format!(
                        "超过 {} 分钟未收到 Agent 上报",
                        settings.offline_alert_minutes
                    ),
                )
            } else {
                ("服务器恢复在线", "已重新收到 Agent 上报".to_string())
            };
            record_observation(
                db,
                queries::AlertObservation {
                    key: &key,
                    server_id: &server.id,
                    rule_id: None,
                    active: offline,
                    details: serde_json::json!({"timestamp": last_seen}),
                    notification: Some(queries::AlertNotification {
                        title,
                        server_name: &server.name,
                        message: &message,
                    }),
                },
            )
            .await;
        }

        if let Some(expires_at) = server.expires_at {
            let expiring = settings.expiry_alert_days > 0
                && expires_at >= current
                && expires_at - current <= settings.expiry_alert_days * 86_400;
            let key = format!("expiry:{}", server.id);
            let days = (expires_at - current + 86_399) / 86_400;
            record_observation(
                db,
                queries::AlertObservation {
                    key: &key,
                    server_id: &server.id,
                    rule_id: None,
                    active: expiring,
                    details: serde_json::json!({"expires_at": expires_at}),
                    notification: expiring.then_some(queries::AlertNotification {
                        title: "服务器即将到期",
                        server_name: &server.name,
                        message: &format!("预计还有 {days} 天到期"),
                    }),
                },
            )
            .await;
        }

        if server.traffic_limit > 0 {
            let rx = server.net_rx_total.unwrap_or_default().max(0);
            let tx = server.net_tx_total.unwrap_or_default().max(0);
            let used = match server.traffic_limit_type.as_str() {
                "max" => rx.max(tx),
                "min" => rx.min(tx),
                "up" => tx,
                "down" => rx,
                _ => rx.saturating_add(tx),
            };
            let percentage = used as f64 / server.traffic_limit as f64 * 100.0;
            for milestone in traffic_milestones(settings.traffic_alert_percentage) {
                let exceeded = percentage >= milestone as f64;
                let key = format!("traffic:{}:{milestone}", server.id);
                record_observation(db, queries::AlertObservation {
                    key: &key,
                    server_id: &server.id,
                    rule_id: None,
                    active: exceeded,
                    details: serde_json::json!({"percentage": percentage, "milestone": milestone}),
                    notification: exceeded.then_some(queries::AlertNotification {
                        title: "流量告警",
                        server_name: &server.name,
                        message: &format!(
                            "当前周期已使用 {:.1}% 的流量额度（达到 {milestone}% 提醒线）",
                            percentage
                        ),
                    }),
                }).await;
            }
        }
    }
    Ok(())
}

async fn record_observation(db: &Database, observation: queries::AlertObservation<'_>) {
    let key = observation.key;
    if let Err(error) = queries::record_alert(db, observation).await {
        tracing::error!(%error, alert_key = key, "failed to record alert observation");
    }
}

fn offline_status(
    persisted_at: Option<i64>,
    received_at: Option<i64>,
    started_at: i64,
    current: i64,
    delay: i64,
) -> Option<(bool, i64)> {
    // No status transition until the first report or the end of the restart grace period.
    let last_seen = received_at.or(persisted_at)?;
    if received_at.is_none() && current.saturating_sub(started_at) < delay {
        return None;
    }
    let elapsed = current.saturating_sub(last_seen);
    Some((elapsed >= delay, last_seen))
}

fn traffic_milestones(start: i64) -> Vec<i64> {
    let mut milestones = vec![start.clamp(50, 100)];
    while *milestones.last().unwrap_or(&100) < 100 {
        let next = (*milestones.last().unwrap_or(&100) + 5).min(100);
        milestones.push(next);
    }
    milestones
}

pub async fn deliver_next(db: &Database, client: &reqwest::Client) -> Result<bool> {
    deliver_next_with(db, |notification| async move {
        send(
            db,
            client,
            &notification.title,
            &notification.server_name,
            &notification.message,
        )
        .await
    })
    .await
}

async fn deliver_next_with<F, Fut>(db: &Database, deliver: F) -> Result<bool>
where
    F: FnOnce(queries::PendingAlertNotification) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let Some(notification) = queries::next_notification(db).await? else {
        return Ok(false);
    };
    let id = notification.id.clone();
    let attempts = notification.attempts;
    if let Err(error) = deliver(notification).await {
        queries::retry_notification(db, &id, attempts).await?;
        tracing::warn!(%error, notification_id = id, "Telegram notification scheduled for retry");
        return Ok(false);
    }
    queries::complete_notification(db, &id).await?;
    Ok(true)
}

async fn send(
    db: &Database,
    client: &reqwest::Client,
    title: &str,
    server: &str,
    message: &str,
) -> Result<()> {
    let settings = queries::raw_telegram_settings(db)
        .await?
        .context("尚未配置 Telegram")?;
    validate_settings(&settings)?;
    let text = render(&settings.template, title, server, message);
    let mut payload = serde_json::json!({
        "chat_id": settings.chat_id,
        "text": text,
        "disable_web_page_preview": true,
    });
    if let Some(thread_id) = settings.message_thread_id {
        payload["message_thread_id"] = Value::from(thread_id);
    }
    let response = client
        .post(format!(
            "https://api.telegram.org/bot{}/sendMessage",
            settings.bot_token
        ))
        .timeout(Duration::from_secs(10))
        .json(&payload)
        .send()
        .await
        .map_err(reqwest::Error::without_url)?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(reqwest::Error::without_url)?;
    if body.len() > 64 * 1024 {
        anyhow::bail!("Telegram response is too large");
    }
    let value = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    if !status.is_success() || value.get("ok").and_then(Value::as_bool) != Some(true) {
        // Surface Telegram's error_code/description (e.g. "chat not found",
        // "bot was blocked by the user") so admins can act on the failure;
        // neither field contains the bot token.
        let description = value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("no description");
        let status_text = if status.is_success() {
            String::new()
        } else {
            format!(" HTTP {}", status.as_u16())
        };
        match value.get("error_code").and_then(Value::as_i64) {
            Some(code) => {
                anyhow::bail!(
                    "Telegram rejected the message{status_text} (error {code}): {description}"
                )
            }
            None => anyhow::bail!("Telegram rejected the message{status_text}: {description}"),
        }
    }
    Ok(())
}

fn validate_settings(settings: &TelegramSettingsView) -> Result<()> {
    if settings.bot_token.trim().is_empty()
        || settings.bot_token.len() > 512
        || settings.bot_token.chars().any(char::is_whitespace)
    {
        anyhow::bail!("Telegram Bot Token 无效");
    }
    if settings.chat_id.trim().is_empty() || settings.chat_id.len() > 128 {
        anyhow::bail!("Telegram Chat ID 无效");
    }
    if settings.template.trim().is_empty() || settings.template.chars().count() > 4000 {
        anyhow::bail!("Telegram 消息模板无效");
    }
    Ok(())
}

fn render(template: &str, title: &str, server: &str, message: &str) -> String {
    let timestamp = time::OffsetDateTime::now_utc();
    let time = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        timestamp.year(),
        timestamp.month() as u8,
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second()
    );
    template
        .replace("{{title}}", title)
        .replace("{{server}}", server)
        .replace("{{message}}", message)
        .replace("{{time}}", &time)
}

#[cfg(test)]
mod tests {
    use super::{offline_status, traffic_milestones};

    #[tokio::test]
    async fn offline_alerts_start_only_after_the_first_valid_report() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        sqlx::query("INSERT INTO servers(id,name,token_hash,created_at,updated_at) VALUES ('node','Node','token',1,1)")
            .execute(db.pool()).await.unwrap();
        let mut settings = crate::db::load_settings(&db).await.unwrap();
        settings.notification_enabled = true;
        settings.offline_alert_minutes = 1;
        let now = crate::db::now();
        let mut reports = std::collections::HashMap::new();
        super::run_periodic(&db, &settings, &reports, now - 3_600)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM alert_states")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        for (received_at, expected_active) in [(now, 0), (now - 120, 1), (now - 120, 1), (now, 0)] {
            reports.insert("node".to_string(), received_at);
            super::run_periodic(&db, &settings, &reports, now - 3_600)
                .await
                .unwrap();
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT active FROM alert_states WHERE state_key='offline:node'"
                )
                .fetch_one(db.pool())
                .await
                .unwrap(),
                expected_active
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT title FROM notification_outbox ORDER BY sequence"
            )
            .fetch_all(db.pool())
            .await
            .unwrap(),
            ["服务器离线", "服务器恢复在线"]
        );
        assert!(
            !super::deliver_next_with(&db, |_| async { anyhow::bail!("temporary failure") })
                .await
                .unwrap()
        );
        assert!(
            crate::db::queries::next_notification(&db)
                .await
                .unwrap()
                .is_none(),
            "recovery must wait for the failed offline notification"
        );
        let (attempts, next_attempt): (i64, i64) = sqlx::query_as(
            "SELECT attempts, next_attempt_at FROM notification_outbox WHERE sequence=1",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(attempts, 1);
        assert!(next_attempt >= now + 5);
        sqlx::query("UPDATE alert_states SET updated_at=1")
            .execute(db.pool())
            .await
            .unwrap();
        crate::db::queries::cleanup_database(&db, 1).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notification_outbox")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            2,
            "cleanup must keep undelivered notifications"
        );
        sqlx::query("UPDATE notification_outbox SET next_attempt_at=0")
            .execute(db.pool())
            .await
            .unwrap();
        for expected in ["服务器离线", "服务器恢复在线"] {
            assert!(
                super::deliver_next_with(&db, |notification| async move {
                    assert_eq!(notification.title, expected);
                    Ok(())
                })
                .await
                .unwrap()
            );
        }
        assert!(
            crate::db::queries::next_notification(&db)
                .await
                .unwrap()
                .is_none()
        );
        crate::db::queries::cleanup_database(&db, 1).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM alert_states")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn a_full_notification_queue_does_not_block_other_servers() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        sqlx::query("INSERT INTO servers(id,name,token_hash,sort_order,created_at,updated_at) VALUES ('full','Full','full',0,1,1),('next','Next','next',1,1,1)")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO alert_states(state_key,server_id,active,updated_at) VALUES ('offline:full','full',0,1)")
            .execute(db.pool()).await.unwrap();
        sqlx::query("WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<64) \
            INSERT INTO notification_outbox(id,state_key,sequence,title,server_name,message,created_at,next_attempt_at) \
            SELECT CAST(n AS TEXT),'offline:full',n,'Offline','Full','Event',1,1 FROM seq")
            .execute(db.pool()).await.unwrap();
        let mut settings = crate::db::load_settings(&db).await.unwrap();
        settings.notification_enabled = true;
        settings.offline_alert_minutes = 1;
        let now = crate::db::now();
        let reports = std::collections::HashMap::from([
            ("full".to_string(), now - 120),
            ("next".to_string(), now - 120),
        ]);
        super::run_periodic(&db, &settings, &reports, now - 3600)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM notification_outbox WHERE state_key='offline:full'"
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            64
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM notification_outbox WHERE state_key='offline:next'"
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );
    }

    #[test]
    fn live_reports_override_old_persisted_history() {
        assert_eq!(
            offline_status(Some(100), Some(999), 100, 1_000, 300),
            Some((false, 999))
        );
        assert_eq!(
            offline_status(None, Some(999), 100, 1_000, 300),
            Some((false, 999))
        );
        assert_eq!(
            offline_status(Some(100), Some(600), 100, 1_000, 300),
            Some((true, 600))
        );
    }

    #[test]
    fn backend_restart_allows_agents_time_to_reconnect() {
        assert_eq!(offline_status(Some(100), None, 900, 1_000, 300), None);
        assert_eq!(
            offline_status(Some(100), Some(999), 900, 1_000, 300),
            Some((false, 999))
        );
        assert_eq!(
            offline_status(Some(100), None, 900, 1_200, 300),
            Some((true, 100))
        );
    }

    #[test]
    fn traffic_alerts_advance_in_five_percent_steps() {
        assert_eq!(traffic_milestones(80), vec![80, 85, 90, 95, 100]);
        assert_eq!(traffic_milestones(98), vec![98, 100]);
        assert_eq!(traffic_milestones(100), vec![100]);
    }
}
