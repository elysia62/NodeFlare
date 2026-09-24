use super::super::{Database, now};
use super::ASSIGNMENT_INSERT_BATCH_ROWS;
use super::ingest::placeholder_group;
use crate::models::{
    AgentLatencyResult, AgentLatencyTask, AgentReport, LatencySample, LatencyTaskInput,
    LatencyTaskView,
};
use anyhow::Result;
use sqlx::any::AnyArguments;
use sqlx::{Any, Arguments, AssertSqlSafe, Row, Transaction};
use std::collections::HashMap;
pub(crate) async fn latest_latency_map(
    db: &Database,
    live: &HashMap<String, AgentReport>,
) -> Result<HashMap<String, Vec<LatencySample>>> {
    let mut live_latency = HashMap::<(&str, &str), &AgentLatencyResult>::new();
    for (server_id, report) in live {
        for value in &report.latency_results {
            let current = live_latency
                .entry((server_id, &value.task_id))
                .or_insert(value);
            if value.timestamp > current.timestamp {
                *current = value;
            }
        }
    }
    let rows = sqlx::query(
        "SELECT a.server_id, a.assigned_at, t.id AS task_id, t.name, t.task_type, t.target, t.port, \
         lr.timestamp, lr.latency_ms, lr.packet_loss FROM latency_task_servers a \
         JOIN latency_tasks t ON t.id=a.task_id LEFT JOIN latency_results lr \
         ON lr.task_id=a.task_id AND lr.server_id=a.server_id AND lr.timestamp=( \
           SELECT MAX(newer.timestamp) FROM latency_results newer \
           WHERE newer.task_id=a.task_id AND newer.server_id=a.server_id \
             AND newer.timestamp>=a.assigned_at) \
         ORDER BY t.sort_order, t.created_at",
    )
    .fetch_all(db.pool())
    .await?;
    let mut result = HashMap::<String, Vec<LatencySample>>::new();
    for row in rows {
        let server_id: String = row.try_get("server_id")?;
        let mut point = LatencySample {
            task_id: row.try_get("task_id")?,
            server_id: server_id.clone(),
            name: row.try_get("name")?,
            task_type: row.try_get("task_type")?,
            target: row.try_get("target")?,
            port: row.try_get("port")?,
            timestamp: row
                .try_get::<Option<i64>, _>("timestamp")?
                .unwrap_or_default(),
            latency_ms: row.try_get::<Option<f64>, _>("latency_ms")?.unwrap_or(-1.0),
            packet_loss: row
                .try_get::<Option<f64>, _>("packet_loss")?
                .unwrap_or(-1.0),
        };
        let assigned_at = row.try_get::<i64, _>("assigned_at")?;
        if let Some(current) = live_latency
            .get(&(server_id.as_str(), point.task_id.as_str()))
            .filter(|value| value.timestamp >= point.timestamp && value.timestamp >= assigned_at)
        {
            point.timestamp = current.timestamp;
            point.latency_ms = current.latency_ms;
            point.packet_loss = current.packet_loss;
        }
        result.entry(server_id).or_default().push(point);
    }
    Ok(result)
}

pub async fn list_latency_tasks(db: &Database) -> Result<Vec<LatencyTaskView>> {
    let rows = sqlx::query(
        "SELECT id, name, task_type, target, port, interval_seconds, default_enabled \
         FROM latency_tasks ORDER BY sort_order, created_at",
    )
    .fetch_all(db.pool())
    .await?;
    let assignments = sqlx::query(
        "SELECT task_id, server_id FROM latency_task_servers ORDER BY task_id, server_id",
    )
    .fetch_all(db.pool())
    .await?;
    let mut by_task = HashMap::<String, Vec<String>>::new();
    for row in assignments {
        by_task
            .entry(row.try_get("task_id")?)
            .or_default()
            .push(row.try_get("server_id")?);
    }
    rows.into_iter()
        .map(|row| {
            let id: String = row.try_get("id")?;
            Ok(LatencyTaskView {
                server_ids: by_task.remove(&id).unwrap_or_default(),
                id,
                name: row.try_get("name")?,
                task_type: row.try_get("task_type")?,
                target: row.try_get("target")?,
                port: row.try_get("port")?,
                interval_seconds: row.try_get("interval_seconds")?,
                default_enabled: row.try_get::<i64, _>("default_enabled")? != 0,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

pub async fn tasks_for_server(db: &Database, server_id: &str) -> Result<Vec<AgentLatencyTask>> {
    let rows = sqlx::query(db.sql(
        "SELECT t.id, t.name, t.task_type, t.target, t.port, t.interval_seconds \
         FROM latency_tasks t JOIN latency_task_servers a ON a.task_id=t.id \
         WHERE a.server_id=? ORDER BY t.sort_order, t.created_at",
    ))
    .bind(server_id)
    .fetch_all(db.pool())
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(AgentLatencyTask {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                task_type: row.try_get("task_type")?,
                target: row.try_get("target")?,
                port: row.try_get("port")?,
                interval_seconds: row.try_get("interval_seconds")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

pub async fn create_latency_task(db: &Database, input: &LatencyTaskInput) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let timestamp = now();
    let mut transaction = db.pool().begin().await?;
    sqlx::query(db.sql(
        "INSERT INTO latency_tasks(id, name, task_type, target, port, interval_seconds, \
         default_enabled, sort_order, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, \
         (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM latency_tasks), ?, ?)",
    ))
    .bind(&id)
    .bind(input.name.trim())
    .bind(&input.task_type)
    .bind(input.target.trim())
    .bind(input.port)
    .bind(input.interval_seconds)
    .bind(i64::from(input.default_enabled))
    .bind(timestamp)
    .bind(timestamp)
    .execute(&mut *transaction)
    .await?;
    replace_task_servers(db, &mut transaction, &id, &input.server_ids, timestamp).await?;
    transaction.commit().await?;
    Ok(id)
}

pub async fn update_latency_task(
    db: &Database,
    id: &str,
    input: &LatencyTaskInput,
) -> Result<bool> {
    let timestamp = now();
    let mut transaction = db.pool().begin().await?;
    // Only a different probe destination starts a new history segment.
    sqlx::query(db.sql(
        "UPDATE latency_task_servers SET assigned_at=? WHERE task_id=? AND EXISTS (\
         SELECT 1 FROM latency_tasks WHERE id=latency_task_servers.task_id AND \
         (task_type<>? OR target<>? OR COALESCE(port, 0)<>?))",
    ))
    .bind(timestamp)
    .bind(id)
    .bind(&input.task_type)
    .bind(input.target.trim())
    .bind(input.port.unwrap_or(0))
    .execute(&mut *transaction)
    .await?;
    let result = sqlx::query(db.sql(
        "UPDATE latency_tasks SET name=?, task_type=?, target=?, port=?, interval_seconds=?, \
         default_enabled=?, updated_at=? WHERE id=?",
    ))
    .bind(input.name.trim())
    .bind(&input.task_type)
    .bind(input.target.trim())
    .bind(input.port)
    .bind(input.interval_seconds)
    .bind(i64::from(input.default_enabled))
    .bind(timestamp)
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    if result.rows_affected() == 0 {
        transaction.rollback().await?;
        return Ok(false);
    }
    replace_task_servers(db, &mut transaction, id, &input.server_ids, timestamp).await?;
    transaction.commit().await?;
    Ok(true)
}
pub(crate) async fn replace_task_servers(
    db: &Database,
    transaction: &mut Transaction<'_, Any>,
    task_id: &str,
    server_ids: &[String],
    timestamp: i64,
) -> Result<()> {
    let existing = sqlx::query(
        db.sql("SELECT server_id, assigned_at FROM latency_task_servers WHERE task_id=?"),
    )
    .bind(task_id)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(|row| {
        Ok::<_, sqlx::Error>((
            row.try_get::<String, _>("server_id")?,
            row.try_get::<i64, _>("assigned_at")?,
        ))
    })
    .collect::<std::result::Result<HashMap<_, _>, _>>()?;
    sqlx::query(db.sql("DELETE FROM latency_task_servers WHERE task_id=?"))
        .bind(task_id)
        .execute(&mut **transaction)
        .await?;
    for batch in server_ids.chunks(ASSIGNMENT_INSERT_BATCH_ROWS) {
        let mut arguments = AnyArguments::default();
        let mut parameter_index = 1_usize;
        let mut groups = Vec::with_capacity(batch.len());
        for server_id in batch {
            groups.push(placeholder_group(db, &mut parameter_index, 3));
            let assigned_at = existing.get(server_id).copied().unwrap_or(timestamp);
            arguments
                .add(task_id.to_string())
                .map_err(|error| anyhow::anyhow!("无法编码拨测任务节点：{error}"))?;
            arguments
                .add(server_id.clone())
                .map_err(|error| anyhow::anyhow!("无法编码拨测任务节点：{error}"))?;
            arguments
                .add(assigned_at)
                .map_err(|error| anyhow::anyhow!("无法编码拨测任务节点：{error}"))?;
        }
        let sql = format!(
            "INSERT INTO latency_task_servers(task_id, server_id, assigned_at) VALUES {}",
            groups.join(",")
        );
        sqlx::query_with(AssertSqlSafe(sql), arguments)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

pub async fn delete_latency_task(db: &Database, id: &str) -> Result<bool> {
    let result = sqlx::query(db.sql("DELETE FROM latency_tasks WHERE id=?"))
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn latency_task_server_ids(db: &Database, id: &str) -> Result<Vec<String>> {
    let rows = sqlx::query(db.sql("SELECT server_id FROM latency_task_servers WHERE task_id=?"))
        .bind(id)
        .fetch_all(db.pool())
        .await?;
    rows.into_iter()
        .map(|row| row.try_get("server_id"))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub(crate) fn latency_history_bucket_seconds(hours: i64, task_count: i64) -> i64 {
    let hours = hours.clamp(1, 24 * 365);
    let task_count = task_count.max(1);
    let base = match hours {
        1 => 60,
        2..=4 => 120,
        5..=24 => 600,
        25..=168 => 3600,
        169..=720 => 14_400,
        _ => 86_400,
    };
    let points_per_task = (4000 / task_count).max(1);
    let bounded = (hours * 3600 + points_per_task - 1) / points_per_task;
    base.max(bounded)
}

pub async fn latency_history(
    db: &Database,
    server_id: &str,
    hours: i64,
) -> Result<(Vec<AgentLatencyTask>, Vec<LatencySample>)> {
    let tasks = tasks_for_server(db, server_id).await?;
    if tasks.is_empty() {
        return Ok((tasks, Vec::new()));
    }
    let bucket = latency_history_bucket_seconds(hours, tasks.len() as i64);
    let since = now() - hours.clamp(1, 24 * 365) * 3600;
    let rows = sqlx::query(db.sql(
        "SELECT task_id, server_id, name, task_type, target, port, \
         bucket_timestamp AS timestamp, \
         CASE WHEN SUM(CASE WHEN latency_ms>=0 THEN 1 ELSE 0 END)>0 \
           THEN SUM(CASE WHEN latency_ms>=0 THEN latency_ms ELSE 0 END) / \
                SUM(CASE WHEN latency_ms>=0 THEN CAST(1 AS DOUBLE PRECISION) \
                         ELSE CAST(0 AS DOUBLE PRECISION) END) \
           ELSE CAST(-1 AS DOUBLE PRECISION) END AS latency_ms, \
         AVG(packet_loss) AS packet_loss FROM ( \
           SELECT r.task_id, r.server_id, t.name, t.task_type, t.target, t.port, \
                  (r.timestamp / ?) * ? AS bucket_timestamp, r.latency_ms, r.packet_loss \
           FROM latency_results r JOIN latency_tasks t ON t.id=r.task_id \
           JOIN latency_task_servers a ON a.task_id=r.task_id AND a.server_id=r.server_id \
           WHERE r.server_id=? AND r.timestamp>=? AND r.timestamp>=a.assigned_at \
         ) samples GROUP BY task_id, server_id, name, task_type, target, port, bucket_timestamp \
         ORDER BY bucket_timestamp, name LIMIT 4000",
    ))
    .bind(bucket)
    .bind(bucket)
    .bind(server_id)
    .bind(since)
    .fetch_all(db.pool())
    .await?;
    let points = rows
        .into_iter()
        .map(|row| {
            Ok(LatencySample {
                task_id: row.try_get("task_id")?,
                server_id: row.try_get("server_id")?,
                name: row.try_get("name")?,
                task_type: row.try_get("task_type")?,
                target: row.try_get("target")?,
                port: row.try_get("port")?,
                timestamp: row.try_get("timestamp")?,
                latency_ms: row.try_get("latency_ms")?,
                packet_loss: row.try_get("packet_loss")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
    Ok((tasks, points))
}
