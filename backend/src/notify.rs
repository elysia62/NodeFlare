use crate::db::{self, Database, Settings, queries};
use crate::models::TelegramSettingsView;
use anyhow::{Context, Result};
use serde_json::Value;
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
) -> Result<()> {
    if !settings.notification_enabled {
        return Ok(());
    }
    let current = db::now();
    for server in queries::list_servers(db, true).await? {
        if !server.offline_notify_disabled {
            let offline = server.timestamp.is_none_or(|timestamp| {
                current.saturating_sub(timestamp) >= settings.offline_alert_minutes * 60
            });
            let key = format!("offline:{}", server.id);
            if queries::update_alert_state(
                db,
                &key,
                offline,
                &serde_json::json!({"timestamp": server.timestamp}),
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
    use super::traffic_milestones;

    #[test]
    fn traffic_alerts_advance_in_five_percent_steps() {
        assert_eq!(traffic_milestones(80), vec![80, 85, 90, 95, 100]);
        assert_eq!(traffic_milestones(98), vec![98, 100]);
        assert_eq!(traffic_milestones(100), vec![100]);
    }
}
