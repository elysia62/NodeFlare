use super::super::{Database, now};
use anyhow::Result;
use sqlx::{AssertSqlSafe, Row};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

const DAY: i64 = 86_400;
const CANDIDATE_LIMIT: i64 = 128;
// Agents can resend latency samples from the previous two hours. Wait until
// that window closes before replacing their deduplication keys with summaries.
const LATE_SAMPLE_WINDOW: i64 = 2 * 3600 + 60;

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
    let retained_since = current - retention_days.clamp(1, 3650) * DAY;
    // Oldest first: old uncompressed data can go directly to hourly buckets.
    for (age, interval, younger_than) in [
        (30 * DAY, 3600, i64::MAX),
        (7 * DAY, 300, 30 * DAY),
        (LATE_SAMPLE_WINDOW, 60, 7 * DAY),
    ] {
        let end = (current - age).div_euclid(interval) * interval;
        let start =
            retained_since.max(current.saturating_sub(younger_than).div_euclid(3600) * 3600);
        if start >= end {
            continue;
        }
        for latency in [false, true] {
            if started.elapsed() >= budget {
                return Ok(());
            }
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
                .bind(end)
                .bind(interval)
                .bind(CANDIDATE_LIMIT)
                .fetch_all(db.pool())
                .await?;
            let mut buckets = BTreeSet::new();
            for row in rows {
                let timestamp: i64 = row.try_get("timestamp")?;
                buckets.insert((
                    timestamp.div_euclid(interval) * interval,
                    row.try_get::<String, _>("server_id")?,
                    row.try_get::<String, _>("task_id")?,
                ));
            }
            for (bucket, server_id, task_id) in buckets {
                if started.elapsed() >= budget {
                    return Ok(());
                }
                compact_bucket(db, &server_id, &task_id, bucket, interval, latency).await?;
            }
        }
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
