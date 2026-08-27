use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use worker::{wasm_bindgen::JsValue, D1Database, Date, Result};

use crate::models::{AgentLatencyResult, AgentLatencyTask, LatencyTaskInput};

pub const MAX_LATENCY_TASKS: usize = 128;
const MAX_HISTORY_RESPONSE_ROWS: i64 = 4000;

#[derive(Debug, Deserialize)]
struct TaskRow {
    id: String,
    name: String,
    task_type: String,
    target: String,
    port: Option<i64>,
    interval_seconds: i64,
    default_enabled: i64,
}

#[derive(Debug, Deserialize)]
struct AssignmentRow {
    task_id: String,
    server_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct StoredLatencyResult {
    task_id: String,
    timestamp: i64,
    latency_ms: f64,
    packet_loss: f64,
    sample_count: u64,
    success_count: u64,
    latest_timestamp: i64,
    latest_latency_ms: f64,
    latest_packet_loss: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
// Tuple layout keeps Durable Object WebSocket attachments below Cloudflare's
// 16 KiB serialized-attachment limit even with all 128 latency tasks enabled.
struct LatencyMetricAggregate(u64, u64, f64, f64, i64, f64, f64);

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LatencyMetricAggregates {
    #[serde(rename = "v")]
    values: BTreeMap<String, LatencyMetricAggregate>,
}

impl LatencyMetricAggregates {
    pub fn extend(&mut self, results: &[AgentLatencyResult], received_at: i64) {
        for result in results {
            if !self.values.contains_key(&result.task_id) && self.values.len() >= MAX_LATENCY_TASKS
            {
                continue;
            }
            let timestamp = if (result.timestamp - received_at).abs() <= 86_400 {
                result.timestamp
            } else {
                received_at
            };
            let aggregate = self.values.entry(result.task_id.clone()).or_default();
            aggregate.0 = aggregate.0.saturating_add(1);
            aggregate.3 += result.packet_loss;
            if result.latency_ms >= 0.0 {
                aggregate.1 = aggregate.1.saturating_add(1);
                aggregate.2 += result.latency_ms;
            }
            if timestamp >= aggregate.4 {
                aggregate.4 = timestamp;
                aggregate.5 = result.latency_ms;
                aggregate.6 = result.packet_loss;
            }
        }
    }

    pub fn merge(&mut self, other: Self) {
        for (task_id, other) in other.values {
            if !self.values.contains_key(&task_id) && self.values.len() >= MAX_LATENCY_TASKS {
                continue;
            }
            let aggregate = self.values.entry(task_id).or_default();
            aggregate.0 = aggregate.0.saturating_add(other.0);
            aggregate.1 = aggregate.1.saturating_add(other.1);
            aggregate.2 += other.2;
            aggregate.3 += other.3;
            if other.4 > aggregate.4 {
                aggregate.4 = other.4;
                aggregate.5 = other.5;
                aggregate.6 = other.6;
            }
        }
    }

    fn rows_matching(&self, mut include: impl FnMut(&String) -> bool) -> Vec<StoredLatencyResult> {
        let mut history = Vec::new();
        for (task_id, aggregate) in self.values.iter().filter(|(task_id, _)| include(task_id)) {
            history.push(StoredLatencyResult {
                task_id: task_id.clone(),
                timestamp: aggregate.4 / 60 * 60,
                latency_ms: if aggregate.1 == 0 {
                    -1.0
                } else {
                    aggregate.2 / aggregate.1 as f64
                },
                packet_loss: aggregate.3 / aggregate.0.max(1) as f64,
                sample_count: aggregate.0,
                success_count: aggregate.1,
                latest_timestamp: aggregate.4,
                latest_latency_ms: aggregate.5,
                latest_packet_loss: aggregate.6,
            });
        }
        history
    }

    pub fn stored_json(&self) -> Result<String> {
        Ok(serde_json::to_string(&self.rows_matching(|_| true))?)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LatencyTaskView {
    pub id: String,
    pub name: String,
    pub task_type: String,
    pub target: String,
    pub port: Option<i64>,
    pub interval_seconds: i64,
    pub default_enabled: bool,
    pub server_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LatencySample {
    pub task_id: String,
    pub server_id: String,
    pub name: String,
    pub task_type: String,
    pub target: String,
    pub port: Option<i64>,
    pub timestamp: i64,
    pub latency_ms: f64,
    pub packet_loss: f64,
}

fn text(value: &str) -> JsValue {
    JsValue::from_str(value)
}

fn number(value: impl ToString) -> JsValue {
    JsValue::from_f64(value.to_string().parse::<f64>().unwrap_or(0.0))
}

pub async fn list_tasks(db: &D1Database) -> Result<Vec<LatencyTaskView>> {
    let rows: Vec<TaskRow> = db
        .prepare(
            "SELECT id, name, task_type, target, port, interval_seconds, default_enabled \
             FROM latency_tasks \
             ORDER BY sort_order ASC, created_at ASC",
        )
        .all()
        .await?
        .results()?;
    let assignments: Vec<AssignmentRow> = db
        .prepare(
            "SELECT task_id, server_id FROM latency_task_servers \
             ORDER BY task_id ASC, server_id ASC",
        )
        .all()
        .await?
        .results()?;
    let mut by_task: HashMap<String, Vec<String>> = HashMap::new();
    for assignment in assignments {
        by_task
            .entry(assignment.task_id)
            .or_default()
            .push(assignment.server_id);
    }
    Ok(rows
        .into_iter()
        .map(|row| LatencyTaskView {
            server_ids: by_task.remove(&row.id).unwrap_or_default(),
            id: row.id,
            name: row.name,
            task_type: row.task_type,
            target: row.target,
            port: row.port,
            interval_seconds: row.interval_seconds,
            default_enabled: row.default_enabled != 0,
        })
        .collect())
}

pub async fn task_count(db: &D1Database) -> Result<i64> {
    db.prepare("SELECT COUNT(*) AS count FROM latency_tasks")
        .first(Some("count"))
        .await?
        .ok_or_else(|| worker::Error::RustError("无法读取延迟任务数量".to_string()))
}

pub async fn server_ids_for_task(db: &D1Database, task_id: &str) -> Result<Vec<String>> {
    let assignments: Vec<AssignmentRow> = db
        .prepare(
            "SELECT task_id, server_id FROM latency_task_servers WHERE task_id = ?1 ORDER BY server_id ASC",
        )
        .bind(&[text(task_id)])?
        .all()
        .await?
        .results()?;
    Ok(assignments
        .into_iter()
        .map(|assignment| assignment.server_id)
        .collect())
}

pub async fn tasks_for_server(db: &D1Database, server_id: &str) -> Result<Vec<AgentLatencyTask>> {
    db.prepare(
        "SELECT t.id, t.name, t.task_type, t.target, t.port, t.interval_seconds \
         FROM latency_tasks t \
         INNER JOIN latency_task_servers a ON a.task_id = t.id \
         WHERE a.server_id = ?1 ORDER BY t.sort_order ASC, t.created_at ASC",
    )
    .bind(&[text(server_id)])?
    .all()
    .await?
    .results()
}

pub async fn create_task(
    db: &D1Database,
    id: &str,
    input: &LatencyTaskInput,
    timestamp: i64,
) -> Result<()> {
    let server_ids = serde_json::to_string(&input.server_ids)?;
    let statements = vec![
        db.prepare(
            "INSERT INTO latency_tasks( \
               id, name, task_type, target, port, interval_seconds, default_enabled, \
               sort_order, created_at, updated_at \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, \
               COALESCE((SELECT MAX(sort_order) + 1 FROM latency_tasks), 0), ?8, ?8)",
        )
        .bind(&[
            text(id),
            text(input.name.trim()),
            text(input.task_type.trim()),
            text(input.target.trim()),
            input.port.map(number).unwrap_or(JsValue::NULL),
            number(input.interval_seconds),
            JsValue::from_bool(input.default_enabled),
            number(timestamp),
        ])?,
        db.prepare(
            "INSERT INTO latency_task_servers(task_id, server_id, assigned_at) \
             SELECT ?1, CAST(value AS TEXT), ?3 FROM json_each(?2)",
        )
        .bind(&[text(id), text(&server_ids), number(timestamp)])?,
    ];
    db.batch(statements).await?;
    Ok(())
}

pub async fn update_task(
    db: &D1Database,
    id: &str,
    input: &LatencyTaskInput,
    timestamp: i64,
) -> Result<bool> {
    let exists: Option<TaskRow> = db
        .prepare(
            "SELECT id, name, task_type, target, port, interval_seconds, default_enabled \
             FROM latency_tasks WHERE id = ?1",
        )
        .bind(&[text(id)])?
        .first(None)
        .await?;
    let Some(existing) = exists else {
        return Ok(false);
    };
    let reset_history = existing.task_type != input.task_type.trim()
        || existing.target != input.target.trim()
        || existing.port != input.port;
    let server_ids = serde_json::to_string(&input.server_ids)?;
    let mut statements = vec![
        db.prepare(
            "UPDATE latency_tasks SET name = ?2, task_type = ?3, target = ?4, port = ?5, \
             interval_seconds = ?6, default_enabled = ?7, updated_at = ?8 WHERE id = ?1",
        )
        .bind(&[
            text(id),
            text(input.name.trim()),
            text(input.task_type.trim()),
            text(input.target.trim()),
            input.port.map(number).unwrap_or(JsValue::NULL),
            number(input.interval_seconds),
            JsValue::from_bool(input.default_enabled),
            number(timestamp),
        ])?,
        db.prepare(
            "DELETE FROM latency_task_servers WHERE task_id = ?1 AND server_id NOT IN ( \
               SELECT CAST(value AS TEXT) FROM json_each(?2) \
             )",
        )
        .bind(&[text(id), text(&server_ids)])?,
        db.prepare(
            "INSERT OR IGNORE INTO latency_task_servers(task_id, server_id, assigned_at) \
             SELECT ?1, CAST(value AS TEXT), ?3 FROM json_each(?2)",
        )
        .bind(&[text(id), text(&server_ids), number(timestamp)])?,
    ];
    if reset_history {
        statements.push(
            db.prepare("UPDATE latency_task_servers SET assigned_at = ?2 WHERE task_id = ?1")
                .bind(&[text(id), number(timestamp)])?,
        );
    }
    db.batch(statements).await?;
    Ok(true)
}

pub async fn delete_task(db: &D1Database, id: &str) -> Result<bool> {
    let result = db
        .prepare("DELETE FROM latency_tasks WHERE id = ?1")
        .bind(&[text(id)])?
        .run()
        .await?;
    Ok(result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0)
}

pub async fn assign_defaults(db: &D1Database, server_id: &str) -> Result<()> {
    db.prepare(
        "INSERT OR IGNORE INTO latency_task_servers(task_id, server_id, assigned_at) \
         SELECT id, ?1, ?2 FROM latency_tasks WHERE default_enabled = 1",
    )
    .bind(&[text(server_id), number(crate::now())])?
    .run()
    .await?;
    Ok(())
}

#[cfg(test)]
fn compact_results(
    results: &[AgentLatencyResult],
    assigned: &std::collections::HashSet<String>,
    received_at: i64,
) -> Vec<StoredLatencyResult> {
    let mut aggregates = LatencyMetricAggregates::default();
    aggregates.extend(results, received_at);
    aggregates.rows_matching(|task_id| assigned.contains(task_id))
}

pub async fn latest_all(db: &D1Database) -> Result<Vec<LatencySample>> {
    db.prepare(latest_query(None)).all().await?.results()
}

fn latest_query(filter: Option<&str>) -> String {
    let filter = filter.map_or(String::new(), |value| format!("WHERE {value}"));
    format!(
        "WITH assignments AS ( \
           SELECT a.task_id, a.server_id, a.assigned_at, t.name, t.task_type, \
             t.target, t.port, t.sort_order, t.created_at \
           FROM latency_task_servers a \
           INNER JOIN latency_tasks t ON t.id = a.task_id {filter} \
         ), latest AS ( \
           SELECT assignments.*, COALESCE( \
             (SELECT j.value FROM (SELECT * FROM metric_history \
                                   UNION ALL SELECT * FROM metric_history_old) h, \
                                  json_each(h.latency_json) j \
              WHERE h.server_id = assignments.server_id \
                AND json_extract(j.value, '$.task_id') = assignments.task_id \
                AND CAST(json_extract(j.value, '$.latest_timestamp') AS INTEGER) >= assignments.assigned_at \
              ORDER BY h.timestamp DESC LIMIT 1), \
             (SELECT j.value FROM metric_history_hourly h, json_each(h.latency_json) j \
              WHERE h.server_id = assignments.server_id \
                AND json_extract(j.value, '$.task_id') = assignments.task_id \
                AND CAST(json_extract(j.value, '$.latest_timestamp') AS INTEGER) >= assignments.assigned_at \
              ORDER BY h.timestamp DESC LIMIT 1) \
           ) AS result FROM assignments \
         ) SELECT task_id, server_id, name, task_type, target, port, \
           COALESCE(CAST(json_extract(result, '$.latest_timestamp') AS INTEGER), 0) AS timestamp, \
           COALESCE(CAST(json_extract(result, '$.latest_latency_ms') AS REAL), -1) AS latency_ms, \
           COALESCE(CAST(json_extract(result, '$.latest_packet_loss') AS REAL), -1) AS packet_loss \
         FROM latest ORDER BY sort_order ASC, created_at ASC"
    )
}

/// 把一张历史表的 `latency_json` 摊平成延迟样本行。
fn latency_samples_from(table: &str) -> String {
    format!(
        "SELECT h.server_id, h.timestamp, json_extract(j.value, '$.task_id') AS task_id, \
           CAST(json_extract(j.value, '$.latency_ms') AS REAL) AS latency_ms, \
           CAST(json_extract(j.value, '$.packet_loss') AS REAL) AS packet_loss, \
           CAST(json_extract(j.value, '$.sample_count') AS INTEGER) AS sample_count, \
           CAST(json_extract(j.value, '$.success_count') AS INTEGER) AS success_count, \
           CAST(json_extract(j.value, '$.latest_timestamp') AS INTEGER) AS latest_timestamp \
         FROM {table} h, json_each(h.latency_json) j"
    )
}

pub async fn history(db: &D1Database, server_id: &str, hours: i64) -> Result<Vec<LatencySample>> {
    let hours = hours.clamp(1, 24 * crate::db::MAX_HISTORY_RETENTION_DAYS);
    let cutoff = (Date::now().as_millis() as i64 / 1000) - hours * 3600;
    let task_count = db
        .prepare("SELECT COUNT(*) AS count FROM latency_task_servers WHERE server_id = ?1")
        .bind(&[text(server_id)])?
        .first::<i64>(Some("count"))
        .await?
        .unwrap_or(0);
    if task_count == 0 {
        return Ok(Vec::new());
    }
    let bucket = history_bucket_seconds(hours, task_count);
    // 分钟历史分两代（见 db::RECENT_HISTORY），近期区间必须都读。
    let mut tables = vec!["metric_history", "metric_history_old"];
    if hours > 24 {
        tables.push("metric_history_hourly");
    }
    let source = tables
        .into_iter()
        .map(latency_samples_from)
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let query = format!(
        "SELECT h.task_id, h.server_id, t.name, t.task_type, t.target, t.port, \
         (h.timestamp / {bucket}) * {bucket} AS timestamp, \
         CASE WHEN SUM(h.success_count) > 0 \
           THEN SUM(h.latency_ms * h.success_count) / SUM(h.success_count) \
           ELSE -1 END AS latency_ms, \
         SUM(h.packet_loss * h.sample_count) / SUM(h.sample_count) AS packet_loss \
         FROM ({source}) h INNER JOIN latency_tasks t ON t.id = h.task_id \
         INNER JOIN latency_task_servers a \
           ON a.task_id = h.task_id AND a.server_id = h.server_id \
         WHERE h.server_id = ?1 AND h.timestamp >= ?2 AND h.latest_timestamp >= a.assigned_at \
         GROUP BY h.task_id, h.server_id, t.name, t.task_type, t.target, t.port, h.timestamp / {bucket} \
         ORDER BY timestamp ASC, t.sort_order ASC LIMIT {MAX_HISTORY_RESPONSE_ROWS}"
    );
    db.prepare(query)
        .bind(&[text(server_id), number(cutoff)])?
        .all()
        .await?
        .results()
}

fn history_bucket_seconds(hours: i64, task_count: i64) -> i64 {
    let hours = hours.clamp(1, 24 * crate::db::MAX_HISTORY_RETENTION_DAYS);
    let task_count = task_count.clamp(1, MAX_LATENCY_TASKS as i64);
    let base = match hours {
        1 => 60,
        2..=4 => 120,
        5..=24 => 600,
        25..=168 => 3600,
        169..=720 => 14_400,
        _ => 86_400,
    };
    let points_per_task = (MAX_HISTORY_RESPONSE_ROWS / task_count).max(1);
    let bounded = (hours * 3600 + points_per_task - 1) / points_per_task;
    base.max(bounded)
}

#[cfg(test)]
mod tests {
    use super::{compact_results, history_bucket_seconds, MAX_LATENCY_TASKS};
    use crate::models::AgentLatencyResult;
    use std::collections::HashSet;

    #[test]
    fn compacts_latency_results_by_task_and_minute() {
        let assigned = HashSet::from(["task-a".to_string()]);
        let results = vec![
            AgentLatencyResult {
                task_id: "task-a".to_string(),
                timestamp: 121,
                latency_ms: 10.0,
                packet_loss: 0.0,
            },
            AgentLatencyResult {
                task_id: "task-a".to_string(),
                timestamp: 149,
                latency_ms: 20.0,
                packet_loss: 25.0,
            },
            AgentLatencyResult {
                task_id: "unassigned".to_string(),
                timestamp: 149,
                latency_ms: 1.0,
                packet_loss: 0.0,
            },
        ];
        let history = compact_results(&results, &assigned, 150);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].timestamp, 120);
        assert_eq!(history[0].latency_ms, 15.0);
        assert_eq!(history[0].packet_loss, 12.5);
        assert_eq!(history[0].sample_count, 2);
        assert_eq!(history[0].success_count, 2);
        assert_eq!(history[0].latest_timestamp, 149);
        assert_eq!(history[0].latest_latency_ms, 20.0);
        assert_eq!(history[0].latest_packet_loss, 25.0);
    }

    #[test]
    fn bounds_compacted_latency_history_by_task() {
        let assigned = (0..128)
            .map(|index| format!("task-{index}"))
            .collect::<HashSet<_>>();
        let results = (0..40)
            .flat_map(|minute| {
                (0..128).map(move |task| AgentLatencyResult {
                    task_id: format!("task-{task}"),
                    timestamp: 1_000_000 + minute * 60,
                    latency_ms: task as f64,
                    packet_loss: 0.0,
                })
            })
            .collect::<Vec<_>>();
        let history = compact_results(&results, &assigned, results.last().unwrap().timestamp);
        assert_eq!(history.len(), MAX_LATENCY_TASKS);
        assert_eq!(
            history.last().unwrap().timestamp,
            results.last().unwrap().timestamp / 60 * 60
        );
    }

    #[test]
    fn bounds_latency_history_response_across_tasks() {
        assert_eq!(history_bucket_seconds(1, 1), 60);
        assert_eq!(history_bucket_seconds(24, 1), 600);
        assert!(history_bucket_seconds(1, 128) >= 117);
        assert!(24 * 3600 / history_bucket_seconds(24, 128) * 128 <= 4000);
        assert_eq!(history_bucket_seconds(720, 1), 14_400);
    }
}
