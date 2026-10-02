use super::super::{Database, now};
use anyhow::Result;
use sqlx::{AssertSqlSafe, Row};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

const DAY: i64 = 86_400;
const CANDIDATE_LIMIT: i64 = 128;
// Agents can resend latency samples from the previous two hours. Wait until
// that window closes before replacing their deduplication keys with summaries.
const LATE_SAMPLE_WINDOW: i64 = 2 * 3600 + 60;
const TIERS: [(i64, i64, i64); 3] = [
    (30 * DAY, 3600, i64::MAX),
    (7 * DAY, 300, 30 * DAY),
    (LATE_SAMPLE_WINDOW, 60, 7 * DAY),
];
const JOB_COUNT: usize = TIERS.len() * 2;
const SCAN_WINDOW: i64 = DAY;

/// Ephemeral cursors avoid rescanning already compacted history. They are not
/// stored in backups: a new connection or a restore starts from retained data.
pub(crate) struct HistoryCompaction {
    pub(crate) progress: tokio::sync::Mutex<CompactionProgress>,
    dirty_since: AtomicI64,
}

impl Default for HistoryCompaction {
    fn default() -> Self {
        Self {
            progress: tokio::sync::Mutex::new(CompactionProgress::default()),
            dirty_since: AtomicI64::new(i64::MAX),
        }
    }
}

impl HistoryCompaction {
    /// Called after history writes, including conservative invalidation when
    /// a commit is cancelled. Ordinary recent reports do not rewind old tiers.
    pub(crate) fn mark_dirty_since(&self, timestamp: i64) {
        self.dirty_since.fetch_min(timestamp, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(crate) struct CompactionProgress {
    cursors: [Option<i64>; JOB_COUNT],
    next_job: usize,
    last_run: Option<(i64, i64)>,
}

impl CompactionProgress {
    fn prepare(&mut self, current: i64, retention_days: i64, dirty_since: i64) {
        if self
            .last_run
            .is_some_and(|(previous, retention)| current < previous || retention_days != retention)
        {
            *self = Self::default();
        }
        self.last_run = Some((current, retention_days));
        if dirty_since != i64::MAX {
            for (index, cursor) in self.cursors.iter_mut().enumerate() {
                if let Some(cursor) = cursor {
                    let interval = TIERS[index / 2].1;
                    *cursor = (*cursor).min(dirty_since.div_euclid(interval) * interval);
                }
            }
        }
    }
}

pub async fn compact_history(db: &Database, retention_days: i64) -> Result<()> {
    compact_history_at(db, retention_days, now(), Duration::from_secs(2)).await
}

pub(super) async fn compact_history_at(
    db: &Database,
    retention_days: i64,
    current: i64,
    budget: Duration,
) -> Result<()> {
    let started = Instant::now();
    if budget.is_zero() {
        return Ok(());
    }
    // Clones of Database share cursors. Do not queue duplicate maintenance
    // passes, or race a restore that is replacing the history underneath us.
    let Ok(mut progress) = db.history_compaction.progress.try_lock() else {
        return Ok(());
    };
    let retention_days = retention_days.clamp(1, 3650);
    progress.prepare(
        current,
        retention_days,
        db.history_compaction
            .dirty_since
            .swap(i64::MAX, Ordering::Relaxed),
    );
    let retained_since = current - retention_days * DAY;
    let mut idle_jobs = 0;
    // Start with the oldest tier, then rotate metric/latency work. Remember
    // the next job across budget exhaustion so a backlog cannot starve one.
    while started.elapsed() < budget && idle_jobs < JOB_COUNT {
        let job = progress.next_job;
        progress.next_job = (job + 1) % JOB_COUNT;
        let (age, interval, younger_than) = TIERS[job / 2];
        let latency = job % 2 != 0;
        let end = (current - age).div_euclid(interval) * interval;
        let lower_bound =
            retained_since.max(current.saturating_sub(younger_than).div_euclid(3600) * 3600);
        let start = progress.cursors[job]
            .unwrap_or(lower_bound)
            .max(lower_bound);
        if start >= end {
            idle_jobs += 1;
            continue;
        }
        idle_jobs = 0;
        // Bound even an empty candidate search: LIMIT bounds returned rows,
        // not the aligned rows SQLite/PostgreSQL may have to skip.
        let window_end = end.min((start + SCAN_WINDOW).div_euclid(interval) * interval);
        let statement = if latency {
            db.sql(
                "SELECT server_id, task_id, timestamp FROM latency_results \
                WHERE timestamp>=? AND timestamp<? AND timestamp % ? <> 0 \
                ORDER BY timestamp LIMIT ?",
            )
        } else {
            db.sql(
                "SELECT server_id, '' AS task_id, timestamp FROM metric_history \
                WHERE timestamp>=? AND timestamp<? AND timestamp % ? <> 0 \
                ORDER BY timestamp LIMIT ?",
            )
        };
        let rows = sqlx::query(statement)
            .bind(start)
            .bind(window_end)
            .bind(interval)
            .bind(CANDIDATE_LIMIT)
            .fetch_all(db.pool())
            .await?;
        let full_page = rows.len() == CANDIDATE_LIMIT as usize;
        let mut buckets = BTreeSet::new();
        for row in rows {
            let timestamp: i64 = row.try_get("timestamp")?;
            buckets.insert((
                timestamp.div_euclid(interval) * interval,
                row.try_get::<String, _>("server_id")?,
                row.try_get::<String, _>("task_id")?,
            ));
        }
        // Stay at the final bucket on a full page: other nodes/tasks in that
        // same bucket may not fit in this page and must still be discovered.
        let resume_at = if full_page {
            buckets.last().expect("full candidate page").0
        } else {
            window_end
        };
        for (bucket, server_id, task_id) in buckets {
            if started.elapsed() >= budget {
                return Ok(());
            }
            compact_bucket(db, &server_id, &task_id, bucket, interval, latency).await?;
        }
        // Advance only after every selected bucket committed. Errors,
        // cancellation and budget exhaustion leave this page retryable.
        progress.cursors[job] = Some(resume_at);
        tokio::task::yield_now().await;
    }
    Ok(())
}

async fn compact_bucket(
    db: &Database,
    server_id: &str,
    task_id: &str,
    bucket: i64,
    interval: i64,
    latency: bool,
) -> Result<()> {
    // Use the same lock order as ingestion. Insert the summary and remove the
    // source rows in one transaction, so interruption/retries cannot lose or
    // double-count samples. A competing maintenance pass simply replaces it.
    let mut transaction = if db.is_postgres() {
        db.pool().begin().await?
    } else {
        db.pool().begin_with("BEGIN IMMEDIATE").await?
    };
    if db.is_postgres() {
        let exists = sqlx::query("SELECT id FROM servers WHERE id=$1 FOR UPDATE")
            .bind(server_id)
            .fetch_optional(&mut *transaction)
            .await?;
        if exists.is_none() {
            return Ok(());
        }
    }
    let end = bucket + interval;
    if latency {
        let sql = latency_rollup_sql(db);
        sqlx::query(AssertSqlSafe(sql))
            .bind(bucket)
            .bind(server_id)
            .bind(task_id)
            .bind(bucket)
            .bind(end)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(db.sql("DELETE FROM latency_results WHERE server_id=? AND task_id=? AND timestamp>? AND timestamp<?"))
            .bind(server_id).bind(task_id).bind(bucket).bind(end)
            .execute(&mut *transaction).await?;
    } else {
        let sql = metric_rollup_sql(db);
        sqlx::query(AssertSqlSafe(sql))
            .bind(bucket)
            .bind(server_id)
            .bind(bucket)
            .bind(end)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            db.sql("DELETE FROM metric_history WHERE server_id=? AND timestamp>? AND timestamp<?"),
        )
        .bind(server_id)
        .bind(bucket)
        .bind(end)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

fn metric_rollup_sql(db: &Database) -> String {
    let source = db.sql(
        "WITH samples AS ( \
        SELECT *, CAST(? AS BIGINT) AS bucket_timestamp FROM metric_history \
        WHERE server_id=? AND timestamp>=? AND timestamp<?), \
        latest AS (SELECT * FROM samples ORDER BY last_timestamp DESC, timestamp DESC LIMIT 1)",
    );
    let mut fields: Vec<(String, String)> = vec![
        ("server_id".into(), "server_id".into()),
        ("timestamp".into(), "bucket_timestamp".into()),
    ];
    for column in [
        "cpu",
        "load1",
        "load5",
        "load15",
        "gpu_usage",
        "mem_used",
        "swap_used",
    ] {
        let mean =
            format!("SUM(CAST({column} AS DOUBLE PRECISION)*sample_count)/SUM(sample_count)");
        let value = if matches!(column, "mem_used" | "swap_used") {
            format!("CAST(ROUND({mean}) AS BIGINT)")
        } else {
            mean
        };
        fields.push((column.into(), value));
    }
    for column in [
        "mem_total",
        "swap_total",
        "disk_used",
        "disk_total",
        "net_in",
        "net_out",
        "processes",
        "tcp_connections",
        "udp_connections",
        "disk_read_bps",
        "disk_write_bps",
        "disk_read_iops",
        "disk_write_iops",
        "disk_await_ms",
        "disk_utilization",
    ] {
        fields.push((column.into(), format!("MAX({column})")));
    }
    for column in ["net_rx_total", "net_tx_total", "uptime", "last_timestamp"] {
        fields.push((column.into(), format!("(SELECT {column} FROM latest)")));
    }
    for (column, expression) in [
        ("sample_count", "CAST(SUM(sample_count) AS BIGINT)"),
        ("first_timestamp", "MIN(first_timestamp)"),
        ("cpu_min", "MIN(COALESCE(cpu_min,cpu))"),
        ("cpu_max", "MAX(COALESCE(cpu_max,cpu))"),
        ("mem_used_max", "MAX(COALESCE(mem_used_max,mem_used))"),
    ] {
        fields.push((column.into(), expression.into()));
    }
    for (prefix, fallback) in [
        (
            "memory",
            "CASE WHEN mem_total>0 THEN CAST(mem_used AS DOUBLE PRECISION)*100/mem_total ELSE 0 END",
        ),
        (
            "disk",
            "CASE WHEN disk_total>0 THEN CAST(disk_used AS DOUBLE PRECISION)*100/disk_total ELSE 0 END",
        ),
        ("net_in", "net_in"),
        ("net_out", "net_out"),
    ] {
        fields.push((
            format!("{prefix}_avg"),
            format!("SUM(COALESCE({prefix}_avg,{fallback})*sample_count)/SUM(sample_count)"),
        ));
        fields.push((
            format!("{prefix}_min"),
            format!("MIN(COALESCE({prefix}_min,{fallback}))"),
        ));
    }
    let columns = fields
        .iter()
        .map(|(column, _)| column.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let values = fields
        .iter()
        .map(|(_, value)| value.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let updates = fields
        .iter()
        .skip(2)
        .map(|(column, _)| format!("{column}=excluded.{column}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{source} INSERT INTO metric_history({columns}) SELECT {values} FROM samples \
        GROUP BY server_id,bucket_timestamp ON CONFLICT(server_id,timestamp) DO UPDATE SET {updates}"
    )
}

fn latency_rollup_sql(db: &Database) -> &'static str {
    db.sql("WITH samples AS ( \
        SELECT r.*, CAST(? AS BIGINT) AS bucket_timestamp, \
            COALESCE(r.latency_sample_count,CASE WHEN r.latency_ms>=0 THEN r.sample_count ELSE 0 END) AS valid_count \
        FROM latency_results r \
        WHERE r.server_id=? AND r.task_id=? AND r.timestamp>=? AND r.timestamp<? \
          AND EXISTS (SELECT 1 FROM latency_task_servers a WHERE a.server_id=r.server_id \
            AND a.task_id=r.task_id AND COALESCE(r.last_timestamp,r.timestamp)>=a.assigned_at)) \
        INSERT INTO latency_results(task_id,server_id,timestamp,latency_ms,packet_loss,sample_count,latency_sample_count,last_timestamp) \
        SELECT task_id,server_id,bucket_timestamp, \
            CASE WHEN SUM(valid_count)>0 THEN SUM(CASE WHEN valid_count>0 THEN latency_ms*valid_count ELSE 0 END)/SUM(valid_count) ELSE -1.0 END, \
            SUM(packet_loss*sample_count)/SUM(sample_count),CAST(SUM(sample_count) AS BIGINT), \
            CAST(SUM(valid_count) AS BIGINT),MAX(COALESCE(last_timestamp,timestamp)) \
        FROM samples GROUP BY task_id,server_id,bucket_timestamp \
        ON CONFLICT(task_id,server_id,timestamp) DO UPDATE SET latency_ms=excluded.latency_ms, \
            packet_loss=excluded.packet_loss,sample_count=excluded.sample_count, \
            latency_sample_count=excluded.latency_sample_count,last_timestamp=excluded.last_timestamp")
}
