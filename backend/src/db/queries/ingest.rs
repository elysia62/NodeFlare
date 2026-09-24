use super::super::{Database, now};
use super::history::{HISTORY_AGGREGATE_COLUMNS, HistoryRow, aggregate_history};
use super::{AgentIdentity, PersistResult, TrafficState};
use crate::models::{AgentLatencyResult, AgentReport};
use anyhow::{Context, Result};
use sqlx::any::AnyArguments;
use sqlx::{Any, Arguments, AssertSqlSafe, Row, Transaction};
use std::collections::{HashMap, HashSet};

const AGENT_REPORT_MAX_AGE_SECONDS: i64 = 2 * 60 * 60;
const AGENT_REPORT_MAX_BYTES: usize = 256 * 1024;
const AGENT_REPORT_MAX_DISKS: usize = 128;
const AGENT_REPORT_MAX_GPUS: usize = 32;
pub(crate) const AGENT_REPORT_MAX_LATENCY_RESULTS: usize = 2048;
const HISTORY_INSERT_BATCH_ROWS: usize = 500;
const LATENCY_INSERT_BATCH_ROWS: usize = 500;

pub async fn save_agent_batch(
    db: &Database,
    identity: &AgentIdentity,
    batch_id: &str,
    reports: &[AgentReport],
    remote_ip: &str,
) -> Result<PersistResult> {
    let mut reports = reports.to_vec();
    reports.sort_by_key(|report| report.timestamp);
    reports.dedup_by_key(|report| report.timestamp);
    let current = now();
    reports.retain(|report| {
        report.timestamp > 0
            && report.timestamp >= current - AGENT_REPORT_MAX_AGE_SECONDS
            && report.timestamp <= current + 300
            && valid_agent_report(report, current)
    });
    if reports.is_empty() {
        anyhow::bail!("report batch contains no valid samples");
    }
    if reports.len() > 720 {
        anyhow::bail!("report batch exceeds 720 samples");
    }

    let mut transaction = if db.is_postgres() {
        db.pool().begin().await?
    } else {
        db.pool().begin_with("BEGIN IMMEDIATE").await?
    };
    if db.is_postgres() {
        sqlx::query("SELECT id FROM servers WHERE id=$1 FOR UPDATE")
            .bind(&identity.server_id)
            .fetch_one(&mut *transaction)
            .await?;
    }
    let latest_row = sqlx::query(
        db.sql("SELECT latest_timestamp, last_batch_id FROM server_latest_state WHERE server_id=?"),
    )
    .bind(&identity.server_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let persisted_before = latest_row
        .as_ref()
        .and_then(|row| row.try_get::<i64, _>("latest_timestamp").ok())
        .unwrap_or(0);
    if latest_row.as_ref().is_some_and(|row| {
        row.try_get::<String, _>("last_batch_id").ok().as_deref() == Some(batch_id)
    }) {
        transaction.rollback().await?;
        return Ok(PersistResult {
            reports: Vec::new(),
            persisted: true,
            persisted_through: persisted_before,
            next_persist_after_ms: identity.report_interval.clamp(15, 3600) as u64 * 1000,
        });
    }
    reports.retain(|report| report.timestamp > persisted_before);
    if reports.is_empty() {
        transaction.rollback().await?;
        return Ok(PersistResult {
            reports: Vec::new(),
            persisted: true,
            persisted_through: persisted_before,
            next_persist_after_ms: identity.report_interval.clamp(15, 3600) as u64 * 1000,
        });
    }

    let mut traffic = load_traffic_state(db, &mut transaction, &identity.server_id).await?;
    for report in &mut reports {
        apply_traffic(report, &mut traffic, identity);
    }
    let report_interval = identity.report_interval.clamp(15, 3600);

    let history_rows = aggregate_history(&reports, report_interval);
    save_history_rows(db, &mut transaction, &identity.server_id, &history_rows).await?;
    save_latency_rows(db, &mut transaction, &identity.server_id, &reports).await?;
    let latest = reports.last().context("report batch became empty")?;
    let persisted_through = latest.timestamp;
    let latest_json = serde_json::to_string(latest)?;
    sqlx::query(db.sql(
        "INSERT INTO server_latest_state(server_id, latest_timestamp, latest_json, last_batch_id) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(server_id) DO UPDATE SET latest_timestamp=excluded.latest_timestamp, \
         latest_json=excluded.latest_json, last_batch_id=excluded.last_batch_id",
    ))
    .bind(&identity.server_id)
    .bind(latest.timestamp)
    .bind(latest_json)
    .bind(batch_id)
    .execute(&mut *transaction)
    .await?;
    save_traffic_state(db, &mut transaction, &identity.server_id, &traffic).await?;
    sqlx::query(db.sql(
        "UPDATE servers SET last_ip=?, ip_v4=CASE WHEN ?='' THEN ip_v4 ELSE ? END, \
         ip_v6=CASE WHEN ?='' THEN ip_v6 ELSE ? END, updated_at=? WHERE id=?",
    ))
    .bind(remote_ip)
    .bind(&latest.ip_v4)
    .bind(&latest.ip_v4)
    .bind(&latest.ip_v6)
    .bind(&latest.ip_v6)
    .bind(current)
    .bind(&identity.server_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(PersistResult {
        reports,
        persisted: true,
        persisted_through,
        next_persist_after_ms: report_interval as u64 * 1000,
    })
}

pub(crate) fn valid_agent_report(report: &AgentReport, current: i64) -> bool {
    fn finite_between(value: f64, minimum: f64, maximum: f64) -> bool {
        value.is_finite() && (minimum..=maximum).contains(&value)
    }

    fn valid_capacity(used: i64, total: i64) -> bool {
        used >= 0 && total >= 0 && (total == 0 || used <= total)
    }

    if !finite_between(report.cpu, 0.0, 100.0)
        || !finite_between(report.load1, 0.0, 1_000_000.0)
        || !finite_between(report.load5, 0.0, 1_000_000.0)
        || !finite_between(report.load15, 0.0, 1_000_000.0)
        || !valid_capacity(report.mem_used, report.mem_total)
        || !valid_capacity(report.swap_used, report.swap_total)
        || !valid_capacity(report.disk_used, report.disk_total)
        || !finite_between(report.net_in, 0.0, 1.0e18)
        || !finite_between(report.net_out, 0.0, 1.0e18)
        || report.net_rx_total < 0
        || report.net_tx_total < 0
        || report.uptime < 0
        || report.processes < 0
        || report.tcp_connections < 0
        || report.udp_connections < 0
        || !(0..=1_000_000).contains(&report.cpu_cores)
        || !finite_between(report.gpu_usage, 0.0, 100.0)
        || !finite_between(report.disk_read_bps, 0.0, 1.0e18)
        || !finite_between(report.disk_write_bps, 0.0, 1.0e18)
        || !finite_between(report.disk_read_iops, 0.0, 1.0e12)
        || !finite_between(report.disk_write_iops, 0.0, 1.0e12)
        || !finite_between(report.disk_await_ms, 0.0, 86_400_000.0)
        || !finite_between(report.disk_utilization, 0.0, 100.0)
        || report.cpu_model.len() > 512
        || report.os.len() > 128
        || report.kernel.len() > 256
        || report.arch.len() > 64
        || report.virtualization.len() > 128
        || report.gpu_model.len() > 512
        || report.agent_version.len() > 64
        || report
            .ip_v4
            .parse::<std::net::Ipv4Addr>()
            .is_err_and(|_| !report.ip_v4.is_empty())
        || report
            .ip_v6
            .parse::<std::net::Ipv6Addr>()
            .is_err_and(|_| !report.ip_v6.is_empty())
        || report.disks.len() > AGENT_REPORT_MAX_DISKS
        || report.gpus.len() > AGENT_REPORT_MAX_GPUS
        || report.latency_results.len() > AGENT_REPORT_MAX_LATENCY_RESULTS
    {
        return false;
    }

    if report.disks.iter().any(|disk| {
        disk.name.len() > 128
            || disk.mount_point.len() > 512
            || !valid_capacity(disk.used, disk.total)
            || !finite_between(disk.read_bps, 0.0, 1.0e18)
            || !finite_between(disk.write_bps, 0.0, 1.0e18)
            || !finite_between(disk.read_iops, 0.0, 1.0e12)
            || !finite_between(disk.write_iops, 0.0, 1.0e12)
            || !finite_between(disk.await_ms, 0.0, 86_400_000.0)
            || !finite_between(disk.utilization, 0.0, 100.0)
    }) || report.gpus.iter().any(|gpu| {
        gpu.model.len() > 512
            || gpu
                .usage
                .is_some_and(|usage| !finite_between(usage, 0.0, 100.0))
            || !valid_capacity(gpu.memory_used, gpu.memory_total)
    }) {
        return false;
    }

    let mut latency_ids = HashSet::with_capacity(report.latency_results.len());
    if report.latency_results.iter().any(|latency| {
        latency.task_id.is_empty()
            || latency.task_id.len() > 80
            || !latency_ids.insert(&latency.task_id)
            || latency.timestamp < current - AGENT_REPORT_MAX_AGE_SECONDS
            || latency.timestamp > current + 300
            || !finite_between(latency.latency_ms, -1.0, 86_400_000.0)
            || !finite_between(latency.packet_loss, 0.0, 100.0)
    }) {
        return false;
    }

    serde_json::to_vec(report).is_ok_and(|encoded| encoded.len() <= AGENT_REPORT_MAX_BYTES)
}
pub(crate) async fn load_traffic_state(
    db: &Database,
    transaction: &mut Transaction<'_, Any>,
    server_id: &str,
) -> Result<TrafficState> {
    let row = sqlx::query(db.sql(
        "SELECT cycle_key, reset_day, timestamp, raw_rx, raw_tx, used_rx, used_tx \
         FROM server_traffic_state WHERE server_id=?",
    ))
    .bind(server_id)
    .fetch_optional(&mut **transaction)
    .await?;
    row.map(|row| {
        Ok::<TrafficState, sqlx::Error>(TrafficState {
            cycle_key: row.try_get("cycle_key")?,
            reset_day: row.try_get("reset_day")?,
            timestamp: row.try_get("timestamp")?,
            raw_rx: row.try_get("raw_rx")?,
            raw_tx: row.try_get("raw_tx")?,
            used_rx: row.try_get("used_rx")?,
            used_tx: row.try_get("used_tx")?,
        })
    })
    .transpose()
    .map(Option::unwrap_or_default)
    .map_err(Into::into)
}

pub(crate) async fn agent_traffic_state(db: &Database, server_id: &str) -> Result<TrafficState> {
    let mut transaction = db.pool().begin().await?;
    let state = load_traffic_state(db, &mut transaction, server_id).await?;
    transaction.rollback().await?;
    Ok(state)
}

pub(crate) fn apply_traffic(
    report: &mut AgentReport,
    state: &mut TrafficState,
    identity: &AgentIdentity,
) {
    if report.timestamp <= state.timestamp {
        report.net_rx_total = state.used_rx.saturating_add(identity.rx_correction).max(0);
        report.net_tx_total = state.used_tx.saturating_add(identity.tx_correction).max(0);
        return;
    }
    let cycle_key = traffic_cycle_key(report.timestamp, identity.reset_day);
    if state.timestamp == 0 {
        state.used_rx = report.net_rx_total.max(0);
        state.used_tx = report.net_tx_total.max(0);
    } else if state.cycle_key != cycle_key || state.reset_day != identity.reset_day {
        state.used_rx = 0;
        state.used_tx = 0;
    } else {
        let rx_delta = if report.net_rx_total >= state.raw_rx {
            report.net_rx_total - state.raw_rx
        } else {
            report.net_rx_total
        };
        let tx_delta = if report.net_tx_total >= state.raw_tx {
            report.net_tx_total - state.raw_tx
        } else {
            report.net_tx_total
        };
        state.used_rx = state.used_rx.saturating_add(rx_delta.max(0));
        state.used_tx = state.used_tx.saturating_add(tx_delta.max(0));
    }
    state.cycle_key = cycle_key;
    state.reset_day = identity.reset_day;
    state.timestamp = report.timestamp;
    state.raw_rx = report.net_rx_total.max(0);
    state.raw_tx = report.net_tx_total.max(0);
    report.net_rx_total = state.used_rx.saturating_add(identity.rx_correction).max(0);
    report.net_tx_total = state.used_tx.saturating_add(identity.tx_correction).max(0);
}
pub(crate) async fn save_traffic_state(
    db: &Database,
    transaction: &mut Transaction<'_, Any>,
    server_id: &str,
    state: &TrafficState,
) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO server_traffic_state(server_id, cycle_key, reset_day, timestamp, raw_rx, \
         raw_tx, used_rx, used_tx) VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(server_id) DO UPDATE SET cycle_key=excluded.cycle_key, \
         reset_day=excluded.reset_day, timestamp=excluded.timestamp, raw_rx=excluded.raw_rx, \
         raw_tx=excluded.raw_tx, used_rx=excluded.used_rx, used_tx=excluded.used_tx",
    ))
    .bind(server_id)
    .bind(state.cycle_key)
    .bind(state.reset_day)
    .bind(state.timestamp)
    .bind(state.raw_rx)
    .bind(state.raw_tx)
    .bind(state.used_rx)
    .bind(state.used_tx)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Builds one row's `($1,$2,…)` value group, advancing the shared counter.
///
/// PostgreSQL needs numbered placeholders while SQLite (and the `Any` driver
/// on it) uses `?`, so batch inserts cannot use a single static statement.
pub(crate) fn placeholder_group(
    db: &Database,
    parameter_index: &mut usize,
    columns: usize,
) -> String {
    let mut placeholders = Vec::with_capacity(columns);
    for _ in 0..columns {
        if db.is_postgres() {
            placeholders.push(format!("${}", *parameter_index));
            *parameter_index += 1;
        } else {
            placeholders.push("?".to_string());
        }
    }
    format!("({})", placeholders.join(","))
}
pub(crate) async fn save_history_rows(
    db: &Database,
    transaction: &mut Transaction<'_, Any>,
    server_id: &str,
    rows: &[HistoryRow],
) -> Result<()> {
    let merge_sql = history_merge_sql();
    for batch in rows.chunks(HISTORY_INSERT_BATCH_ROWS) {
        let mut arguments = AnyArguments::default();
        let mut parameter_index = 1_usize;
        let mut value_groups = Vec::with_capacity(batch.len());
        for row in batch {
            let report = &row.report;
            let mut placeholders = Vec::with_capacity(27 + HISTORY_AGGREGATE_COLUMNS.len());
            for _ in 0..27 + HISTORY_AGGREGATE_COLUMNS.len() {
                placeholders.push(if db.is_postgres() {
                    let placeholder = format!("${parameter_index}");
                    parameter_index += 1;
                    placeholder
                } else {
                    "?".to_string()
                });
            }
            value_groups.push(format!("({})", placeholders.join(",")));
            macro_rules! add {
                ($value:expr) => {
                    arguments.add($value).map_err(|error| {
                        anyhow::anyhow!("无法编码历史指标数据库字段：{error}")
                    })?
                };
            }
            add!(server_id.to_string());
            add!(report.timestamp);
            add!(report.cpu);
            add!(report.load1);
            add!(report.load5);
            add!(report.load15);
            add!(report.mem_used);
            add!(report.mem_total);
            add!(report.swap_used);
            add!(report.swap_total);
            add!(report.disk_used);
            add!(report.disk_total);
            add!(report.net_in);
            add!(report.net_out);
            add!(report.net_rx_total);
            add!(report.net_tx_total);
            add!(report.uptime);
            add!(report.processes);
            add!(report.tcp_connections);
            add!(report.udp_connections);
            add!(report.gpu_usage);
            add!(report.disk_read_bps);
            add!(report.disk_write_bps);
            add!(report.disk_read_iops);
            add!(report.disk_write_iops);
            add!(report.disk_await_ms);
            add!(report.disk_utilization);
            add!(row.sample_count);
            add!(row.first_timestamp);
            add!(row.last_timestamp);
            add!(row.cpu_min);
            add!(row.cpu_max);
            add!(row.mem_used_max);
            add!(row.memory_avg);
            add!(row.memory_min);
            add!(row.disk_avg);
            add!(row.disk_min);
            add!(row.net_in_avg);
            add!(row.net_in_min);
            add!(row.net_out_avg);
            add!(row.net_out_min);
        }
        let sql = format!(
            "INSERT INTO metric_history(server_id, timestamp, cpu, load1, load5, load15, \
             mem_used, mem_total, swap_used, swap_total, disk_used, disk_total, net_in, net_out, \
             net_rx_total, net_tx_total, uptime, processes, tcp_connections, udp_connections, \
             gpu_usage, disk_read_bps, disk_write_bps, disk_read_iops, disk_write_iops, \
             disk_await_ms, disk_utilization, {}) VALUES {} \
             ON CONFLICT(server_id, timestamp) DO UPDATE SET {}",
            HISTORY_AGGREGATE_COLUMNS.join(","),
            value_groups.join(","),
            merge_sql,
        );
        sqlx::query_with(AssertSqlSafe(sql), arguments)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}
pub(crate) fn history_merge_sql() -> String {
    let mut updates = Vec::new();
    for column in [
        "cpu",
        "load1",
        "load5",
        "load15",
        "gpu_usage",
        "mem_used",
        "swap_used",
    ] {
        let mean = format!(
            "(CAST(metric_history.{column} AS DOUBLE PRECISION)*metric_history.sample_count \
             +excluded.{column}*excluded.sample_count)/(metric_history.sample_count+excluded.sample_count)"
        );
        let value = if matches!(column, "mem_used" | "swap_used") {
            format!("CAST(ROUND({mean}) AS BIGINT)")
        } else {
            mean
        };
        updates.push(format!("{column}={value}"));
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
        updates.push(format!(
            "{column}=CASE WHEN metric_history.{column}>excluded.{column} \
            THEN metric_history.{column} ELSE excluded.{column} END"
        ));
    }
    for (column, minimum) in [
        ("cpu_min", true),
        ("cpu_max", false),
        ("mem_used_max", false),
    ] {
        let old = format!("metric_history.{column}");
        let comparison = if minimum { "<" } else { ">" };
        updates.push(format!(
            "{column}=CASE WHEN {old}{comparison}excluded.{column} \
            THEN {old} ELSE excluded.{column} END"
        ));
    }
    for prefix in ["memory", "disk", "net_in", "net_out"] {
        let mean = format!("metric_history.{prefix}_avg");
        let min = format!("metric_history.{prefix}_min");
        updates.push(format!("{prefix}_avg=({mean}*metric_history.sample_count \
            +excluded.{prefix}_avg*excluded.sample_count)/(metric_history.sample_count+excluded.sample_count)"));
        updates.push(format!(
            "{prefix}_min=CASE WHEN {min}<excluded.{prefix}_min \
            THEN {min} ELSE excluded.{prefix}_min END"
        ));
    }
    for column in ["net_rx_total", "net_tx_total", "uptime", "last_timestamp"] {
        updates.push(format!("{column}=excluded.{column}"));
    }
    updates.push(
        "first_timestamp=CASE WHEN metric_history.first_timestamp<excluded.first_timestamp THEN \
        metric_history.first_timestamp \
        ELSE excluded.first_timestamp END"
            .to_string(),
    );
    updates.push("sample_count=metric_history.sample_count+excluded.sample_count".to_string());
    updates.join(",")
}
pub(crate) async fn save_latency_rows(
    db: &Database,
    transaction: &mut Transaction<'_, Any>,
    server_id: &str,
    reports: &[AgentReport],
) -> Result<()> {
    let mut unique = HashMap::<(String, i64), AgentLatencyResult>::new();
    for result in reports.iter().flat_map(|report| &report.latency_results) {
        if !result.task_id.is_empty()
            && result.timestamp > 0
            && result.latency_ms.is_finite()
            && result.packet_loss.is_finite()
        {
            unique.insert((result.task_id.clone(), result.timestamp), result.clone());
        }
    }
    let results = unique.into_values().collect::<Vec<_>>();
    for batch in results.chunks(LATENCY_INSERT_BATCH_ROWS) {
        let mut arguments = AnyArguments::default();
        let mut parameter_index = 1_usize;
        let mut value_groups = Vec::with_capacity(batch.len());
        for result in batch {
            let mut placeholders = Vec::with_capacity(5);
            for _ in 0..5 {
                placeholders.push(if db.is_postgres() {
                    let placeholder = format!("${parameter_index}");
                    parameter_index += 1;
                    placeholder
                } else {
                    "?".to_string()
                });
            }
            value_groups.push(format!("({})", placeholders.join(",")));
            for value in [result.task_id.clone(), server_id.to_string()] {
                arguments
                    .add(value)
                    .map_err(|error| anyhow::anyhow!("无法编码延迟结果数据库字段：{error}"))?;
            }
            arguments
                .add(result.timestamp)
                .map_err(|error| anyhow::anyhow!("无法编码延迟结果数据库字段：{error}"))?;
            arguments
                .add(result.latency_ms)
                .map_err(|error| anyhow::anyhow!("无法编码延迟结果数据库字段：{error}"))?;
            arguments
                .add(result.packet_loss)
                .map_err(|error| anyhow::anyhow!("无法编码延迟结果数据库字段：{error}"))?;
        }
        let sql = format!(
            "WITH incoming(task_id, server_id, timestamp, latency_ms, packet_loss) AS (VALUES {}) \
             INSERT INTO latency_results(task_id, server_id, timestamp, latency_ms, packet_loss) \
             SELECT task_id, server_id, timestamp, latency_ms, packet_loss FROM incoming \
             WHERE EXISTS (SELECT 1 FROM latency_task_servers assigned \
               WHERE assigned.task_id=incoming.task_id AND assigned.server_id=incoming.server_id) \
             ON CONFLICT(task_id, server_id, timestamp) DO UPDATE SET \
             latency_ms=excluded.latency_ms, packet_loss=excluded.packet_loss",
            value_groups.join(",")
        );
        sqlx::query_with(AssertSqlSafe(sql), arguments)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}
pub(crate) fn traffic_cycle_key(timestamp: i64, reset_day: i64) -> i64 {
    let Ok(datetime) = time::OffsetDateTime::from_unix_timestamp(timestamp) else {
        return 0;
    };
    let date = datetime.date();
    let year = i64::from(date.year());
    let month = date.month() as i64;
    let day = i64::from(date.day());
    let boundary = reset_day.clamp(1, 31).min(days_in_month(year, month));
    let current_month = year * 12 + month - 1;
    if day >= boundary {
        current_month
    } else {
        current_month - 1
    }
}
pub(crate) fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
