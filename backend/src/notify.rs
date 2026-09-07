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
    client: &reqwest::Client,
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
        if !queries::update_alert_state(db, &key, evaluation.triggered, &details).await? {
            continue;
        }
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
        if let Err(error) = send(db, client, title, server_name, &message).await {
            tracing::error!(%error, server_id, rule_id = %evaluation.rule.id, "resource alert notification failed");
        }
    }
    Ok(())
}

pub async fn run_periodic(
    db: &Database,
    client: &reqwest::Client,
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
            if queries::update_alert_state(
                db,
                &key,
                offline,
                &serde_json::json!({"timestamp": last_seen}),
            )
            .await?
            {
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
                notify_best_effort(db, client, title, &server.name, &message).await;
            }
        }

        if let Some(expires_at) = server.expires_at {
            let expiring = settings.expiry_alert_days > 0
                && expires_at >= current
                && expires_at - current <= settings.expiry_alert_days * 86_400;
            let key = format!("expiry:{}", server.id);
            if queries::update_alert_state(
                db,
                &key,
                expiring,
                &serde_json::json!({"expires_at": expires_at}),
            )
            .await?
                && expiring
            {
                let days = (expires_at - current + 86_399) / 86_400;
                notify_best_effort(
                    db,
                    client,
                    "服务器即将到期",
                    &server.name,
                    &format!("预计还有 {days} 天到期"),
                )
                .await;
            }
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
                if queries::update_alert_state(
                    db,
                    &key,
                    exceeded,
                    &serde_json::json!({"percentage": percentage, "milestone": milestone}),
                )
                .await?
                    && exceeded
                {
                    notify_best_effort(
                        db,
                        client,
                        "流量告警",
                        &server.name,
                        &format!(
                            "当前周期已使用 {:.1}% 的流量额度（达到 {milestone}% 提醒线）",
                            percentage
                        ),
                    )
                    .await;
                }
            }
        }
    }
    Ok(())
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

async fn notify_best_effort(
    db: &Database,
    client: &reqwest::Client,
    title: &str,
    server: &str,
    message: &str,
) {
    if let Err(error) = send(db, client, title, server, message).await {
        tracing::error!(%error, title, server, "Telegram notification failed");
    }
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
        .await?
        .error_for_status()?;
    let body = response.bytes().await?;
    if body.len() > 64 * 1024 {
        anyhow::bail!("Telegram response is too large");
    }
    let value: Value = serde_json::from_slice(&body)?;
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        anyhow::bail!("Telegram rejected the message");
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
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::new();
        let now = crate::db::now();
        let mut reports = std::collections::HashMap::new();
        super::run_periodic(&db, &client, &settings, &reports, now - 3_600)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM alert_states")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        for (received_at, expected_active) in [(now, 0), (now - 120, 1), (now, 0)] {
            reports.insert("node".to_string(), received_at);
            super::run_periodic(&db, &client, &settings, &reports, now - 3_600)
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
    }

    #[test]
    fn unreported_nodes_never_trigger_offline_alerts() {
        assert_eq!(offline_status(None, None, 100, 10_000, 300), None);
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
