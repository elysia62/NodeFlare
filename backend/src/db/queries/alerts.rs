use super::super::{Database, now};
use super::ASSIGNMENT_INSERT_BATCH_ROWS;
use super::ingest::placeholder_group;
use crate::models::{AlertRuleInput, AlertRuleView};
use anyhow::Result;
use sqlx::any::AnyArguments;
use sqlx::{Any, Arguments, AssertSqlSafe, Row, Transaction};
use std::collections::HashMap;

pub async fn list_alert_rules(db: &Database) -> Result<Vec<AlertRuleView>> {
    let rows = sqlx::query(
        "SELECT id, name, metric, threshold, duration_minutes, aggregation, all_servers, enabled \
         FROM alert_rules ORDER BY created_at",
    )
    .fetch_all(db.pool())
    .await?;
    let assignments = sqlx::query("SELECT rule_id, server_id FROM alert_rule_servers")
        .fetch_all(db.pool())
        .await?;
    let mut by_rule = HashMap::<String, Vec<String>>::new();
    for row in assignments {
        by_rule
            .entry(row.try_get("rule_id")?)
            .or_default()
            .push(row.try_get("server_id")?);
    }
    rows.into_iter()
        .map(|row| {
            let id: String = row.try_get("id")?;
            Ok(AlertRuleView {
                server_ids: by_rule.remove(&id).unwrap_or_default(),
                id,
                name: row.try_get("name")?,
                metric: row.try_get("metric")?,
                threshold: row.try_get("threshold")?,
                duration_minutes: row.try_get("duration_minutes")?,
                aggregation: row.try_get("aggregation")?,
                all_servers: row.try_get::<i64, _>("all_servers")? != 0,
                enabled: row.try_get::<i64, _>("enabled")? != 0,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

pub async fn create_alert_rule(db: &Database, input: &AlertRuleInput) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let timestamp = now();
    let mut transaction = db.pool().begin().await?;
    sqlx::query(db.sql(
        "INSERT INTO alert_rules(id, name, metric, threshold, duration_minutes, aggregation, \
         all_servers, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(&id)
    .bind(input.name.trim())
    .bind(&input.metric)
    .bind(input.threshold)
    .bind(input.duration_minutes)
    .bind(&input.aggregation)
    .bind(i64::from(input.all_servers))
    .bind(i64::from(input.enabled))
    .bind(timestamp)
    .bind(timestamp)
    .execute(&mut *transaction)
    .await?;
    replace_alert_servers(db, &mut transaction, &id, &input.server_ids).await?;
    transaction.commit().await?;
    Ok(id)
}

pub async fn update_alert_rule(db: &Database, id: &str, input: &AlertRuleInput) -> Result<bool> {
    let mut transaction = db.pool().begin().await?;
    let result = sqlx::query(db.sql(
        "UPDATE alert_rules SET name=?, metric=?, threshold=?, duration_minutes=?, aggregation=?, \
         all_servers=?, enabled=?, updated_at=? WHERE id=?",
    ))
    .bind(input.name.trim())
    .bind(&input.metric)
    .bind(input.threshold)
    .bind(input.duration_minutes)
    .bind(&input.aggregation)
    .bind(i64::from(input.all_servers))
    .bind(i64::from(input.enabled))
    .bind(now())
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    if result.rows_affected() == 0 {
        transaction.rollback().await?;
        return Ok(false);
    }
    replace_alert_servers(db, &mut transaction, id, &input.server_ids).await?;
    transaction.commit().await?;
    Ok(true)
}
pub(crate) async fn replace_alert_servers(
    db: &Database,
    transaction: &mut Transaction<'_, Any>,
    rule_id: &str,
    server_ids: &[String],
) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM alert_rule_servers WHERE rule_id=?"))
        .bind(rule_id)
        .execute(&mut **transaction)
        .await?;
    for batch in server_ids.chunks(ASSIGNMENT_INSERT_BATCH_ROWS) {
        let mut arguments = AnyArguments::default();
        let mut parameter_index = 1_usize;
        let mut groups = Vec::with_capacity(batch.len());
        for server_id in batch {
            groups.push(placeholder_group(db, &mut parameter_index, 2));
            arguments
                .add(rule_id.to_string())
                .map_err(|error| anyhow::anyhow!("无法编码告警规则节点：{error}"))?;
            arguments
                .add(server_id.clone())
                .map_err(|error| anyhow::anyhow!("无法编码告警规则节点：{error}"))?;
        }
        let sql = format!(
            "INSERT INTO alert_rule_servers(rule_id, server_id) VALUES {}",
            groups.join(",")
        );
        sqlx::query_with(AssertSqlSafe(sql), arguments)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

pub async fn delete_alert_rule(db: &Database, id: &str) -> Result<bool> {
    let result = sqlx::query(db.sql("DELETE FROM alert_rules WHERE id=?"))
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

#[derive(Debug, Clone)]
pub struct ResourceAlertEvaluation {
    pub rule: AlertRuleView,
    pub value: f64,
    pub triggered: bool,
}

pub async fn evaluate_resource_rules(
    db: &Database,
    server_id: &str,
) -> Result<Vec<ResourceAlertEvaluation>> {
    let report_interval =
        sqlx::query_scalar::<_, i64>(db.sql("SELECT report_interval FROM servers WHERE id=?"))
            .bind(server_id)
            .fetch_optional(db.pool())
            .await?
            .unwrap_or(60);
    // Load only the rules that apply to this server instead of the full rule
    // table plus every assignment row on every evaluation pass; this runs per
    // persisted batch, so the previous full scan scaled badly with fleet size.
    let rows = sqlx::query(db.sql(
        "SELECT id, name, metric, threshold, duration_minutes, aggregation, all_servers \
         FROM alert_rules WHERE enabled=1 \
         AND (all_servers=1 OR id IN (SELECT rule_id FROM alert_rule_servers WHERE server_id=?)) \
         ORDER BY created_at",
    ))
    .bind(server_id)
    .fetch_all(db.pool())
    .await?;
    let mut evaluations = Vec::new();
    for row in rows {
        let rule = AlertRuleView {
            server_ids: Vec::new(),
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            metric: row.try_get("metric")?,
            threshold: row.try_get("threshold")?,
            duration_minutes: row.try_get("duration_minutes")?,
            aggregation: row.try_get("aggregation")?,
            all_servers: row.try_get::<i64, _>("all_servers")? != 0,
            enabled: true,
        };
        let (average_expression, minimum_expression) = match rule.metric.as_str() {
            "cpu" => ("cpu", "cpu_min"),
            "memory" => ("memory_avg", "memory_min"),
            "disk" => ("disk_avg", "disk_min"),
            "net_in" => ("net_in_avg/1048576.0", "net_in_min/1048576.0"),
            "net_out" => ("net_out_avg/1048576.0", "net_out_min/1048576.0"),
            _ => continue,
        };
        let filter = db.sql("server_id=? AND timestamp>=? AND last_timestamp>=?");
        let query = format!(
            "SELECT MIN(first_timestamp) AS first_timestamp, \
             MAX(last_timestamp) AS last_timestamp, \
             SUM(({average_expression}) * sample_count) / NULLIF(SUM(sample_count), 0) AS average_value, \
             MIN({minimum_expression}) AS minimum_value, COUNT(*) AS sample_count \
             FROM metric_history WHERE {filter}"
        );
        let current = now();
        let since = current - rule.duration_minutes * 60;
        let row = sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
            .bind(server_id)
            .bind(since - 3600)
            .bind(since)
            .fetch_one(db.pool())
            .await?;
        let first: Option<i64> = row.try_get("first_timestamp")?;
        let last: Option<i64> = row.try_get("last_timestamp")?;
        let count: i64 = row.try_get("sample_count")?;
        let average: Option<f64> = row.try_get("average_value")?;
        let minimum: Option<f64> = row.try_get("minimum_value")?;
        let value = if rule.aggregation == "continuous" {
            minimum.unwrap_or_default()
        } else {
            average.unwrap_or_default()
        };
        let covered = count > 0
            && first.is_some_and(|first| first <= since + 5)
            && last.is_some_and(|last| last >= current - report_interval - 5);
        evaluations.push(ResourceAlertEvaluation {
            triggered: covered && value >= rule.threshold,
            rule,
            value,
        });
    }
    Ok(evaluations)
}

pub struct AlertObservation<'a> {
    pub key: &'a str,
    pub server_id: &'a str,
    pub rule_id: Option<&'a str>,
    pub active: bool,
    pub details: serde_json::Value,
    pub notification: Option<AlertNotification<'a>>,
}

pub struct AlertNotification<'a> {
    pub title: &'a str,
    pub server_name: &'a str,
    pub message: &'a str,
}

pub async fn record_alert(db: &Database, observation: AlertObservation<'_>) -> Result<()> {
    let mut transaction = if db.is_postgres() {
        db.pool().begin().await?
    } else {
        db.pool().begin_with("BEGIN IMMEDIATE").await?
    };
    let timestamp = now();
    sqlx::query(db.sql(
        "INSERT INTO alert_states(state_key, server_id, rule_id, active, updated_at) \
         VALUES (?, ?, ?, 0, ?) ON CONFLICT(state_key) DO NOTHING",
    ))
    .bind(observation.key)
    .bind(observation.server_id)
    .bind(observation.rule_id)
    .bind(timestamp)
    .execute(&mut *transaction)
    .await?;
    // Serialize observations so the state change and its delivery event commit together.
    let select = if db.is_postgres() {
        "SELECT active FROM alert_states WHERE state_key=? FOR UPDATE"
    } else {
        "SELECT active FROM alert_states WHERE state_key=?"
    };
    let previous = sqlx::query_scalar::<_, i64>(db.sql(select))
        .bind(observation.key)
        .fetch_one(&mut *transaction)
        .await?
        != 0;
    if previous != observation.active
        && let Some(notification) = observation.notification
    {
        let row = sqlx::query(db.sql(
            "SELECT COUNT(*) AS pending, COALESCE(MAX(sequence), 0) + 1 AS sequence \
             FROM notification_outbox WHERE state_key=?",
        ))
        .bind(observation.key)
        .fetch_one(&mut *transaction)
        .await?;
        // A prolonged outage must not create an unbounded queue for a flapping alert.
        anyhow::ensure!(
            row.try_get::<i64, _>("pending")? < 64,
            "alert notification queue is full"
        );
        sqlx::query(db.sql(
            "INSERT INTO notification_outbox(id, state_key, sequence, title, server_name, message, \
             created_at, next_attempt_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        ))
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(observation.key)
        .bind(row.try_get::<i64, _>("sequence")?)
        .bind(notification.title)
        .bind(notification.server_name)
        .bind(notification.message)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        db.sql("UPDATE alert_states SET active=?, updated_at=?, details_json=? WHERE state_key=?"),
    )
    .bind(i64::from(observation.active))
    .bind(timestamp)
    .bind(observation.details.to_string())
    .bind(observation.key)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

pub struct PendingAlertNotification {
    pub id: String,
    pub title: String,
    pub server_name: String,
    pub message: String,
    pub attempts: i64,
}

pub async fn next_notification(db: &Database) -> Result<Option<PendingAlertNotification>> {
    let row = sqlx::query(db.sql(
        "SELECT id, title, server_name, message, attempts FROM notification_outbox AS pending \
         WHERE next_attempt_at<=? AND NOT EXISTS ( \
           SELECT 1 FROM notification_outbox AS earlier \
           WHERE earlier.state_key=pending.state_key AND earlier.sequence<pending.sequence) \
         ORDER BY created_at, id LIMIT 1",
    ))
    .bind(now())
    .fetch_optional(db.pool())
    .await?;
    row.map(|row| {
        Ok(PendingAlertNotification {
            id: row.try_get("id")?,
            title: row.try_get("title")?,
            server_name: row.try_get("server_name")?,
            message: row.try_get("message")?,
            attempts: row.try_get("attempts")?,
        })
    })
    .transpose()
}

pub async fn complete_notification(db: &Database, id: &str) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM notification_outbox WHERE id=?"))
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn retry_notification(db: &Database, id: &str, attempts: i64) -> Result<()> {
    let delay = (5_i64 << attempts.clamp(0, 10)).min(3600);
    sqlx::query(db.sql("UPDATE notification_outbox SET attempts=?, next_attempt_at=? WHERE id=?"))
        .bind(attempts.saturating_add(1))
        .bind(now().saturating_add(delay))
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok(())
}
