use super::{Database, SECRET_MASK, now};
use crate::auth;
use crate::models::{
    AgentLatencyResult, AgentLatencyTask, AgentReport, AlertRuleInput, AlertRuleView,
    DatabaseStats, HistoryPoint, LatencySample, LatencyTaskInput, LatencyTaskView, RemoteTaskInfo,
    ServerInput, ServerView, TelegramSettingsInput, TelegramSettingsView, ThemeInput, ThemeView,
};
use anyhow::{Context, Result};
use sqlx::any::AnyArguments;
use sqlx::{Any, Arguments, AssertSqlSafe, Row, Transaction};
use std::collections::{HashMap, HashSet};

const REMOTE_TASK_ACTIVE_TTL_SECONDS: i64 = 24 * 60 * 60;
const AGENT_REPORT_MAX_AGE_SECONDS: i64 = 2 * 60 * 60;
const AGENT_REPORT_MAX_BYTES: usize = 256 * 1024;
const AGENT_REPORT_MAX_DISKS: usize = 128;
const AGENT_REPORT_MAX_GPUS: usize = 32;
const AGENT_REPORT_MAX_LATENCY_RESULTS: usize = 2048;
const HISTORY_INSERT_BATCH_ROWS: usize = 500;
const LATENCY_INSERT_BATCH_ROWS: usize = 500;

#[derive(Debug, Clone)]
pub struct AgentIdentity {
    pub server_id: String,
    pub hidden: bool,
    pub report_interval: i64,
    pub collect_interval: i64,
    pub reset_day: i64,
    pub rx_correction: i64,
    pub tx_correction: i64,
}

#[derive(Debug, Clone)]
pub struct PersistResult {
    pub reports: Vec<AgentReport>,
    pub persisted_through: i64,
}

#[derive(Debug, Clone, Default)]
struct TrafficState {
    cycle_key: i64,
    reset_day: i64,
    timestamp: i64,
    raw_rx: i64,
    raw_tx: i64,
    used_rx: i64,
    used_tx: i64,
}

pub async fn list_servers(db: &Database, include_hidden: bool) -> Result<Vec<ServerView>> {
    let statement = if include_hidden {
        "SELECT s.id, s.name, s.region, s.group_name, s.tags, s.hidden, s.expires_at, \
         s.traffic_limit, s.traffic_limit_type, s.price, s.billing_cycle, s.currency, \
         s.auto_renewal, s.last_ip, s.ip_v4, s.ip_v6, s.network_interface, s.reset_day, \
         s.report_interval, s.collect_interval, s.rx_correction, s.tx_correction, \
         s.agent_mirror, s.offline_notify_disabled, s.auto_update, \
         l.latest_json \
         FROM servers s LEFT JOIN server_latest_state l ON l.server_id=s.id \
         ORDER BY s.sort_order, s.created_at"
    } else {
        "SELECT s.id, s.name, s.region, s.group_name, s.tags, s.hidden, s.expires_at, \
         s.traffic_limit, s.traffic_limit_type, s.price, s.billing_cycle, s.currency, \
         s.auto_renewal, s.last_ip, s.ip_v4, s.ip_v6, s.network_interface, s.reset_day, \
         s.report_interval, s.collect_interval, s.rx_correction, s.tx_correction, \
         s.agent_mirror, s.offline_notify_disabled, s.auto_update, \
         l.latest_json \
         FROM servers s LEFT JOIN server_latest_state l ON l.server_id=s.id \
         WHERE s.hidden=0 ORDER BY s.sort_order, s.created_at"
    };
    let rows = sqlx::query(statement).fetch_all(db.pool()).await?;
    let latency = latest_latency_map(db).await?;
    rows.into_iter()
        .map(|row| {
            let id: String = row.try_get("id")?;
            let latest_json: Option<String> = row.try_get("latest_json")?;
            let report = latest_json
                .as_deref()
                .filter(|value| !value.is_empty() && *value != "{}")
                .and_then(|value| serde_json::from_str::<AgentReport>(value).ok());
            let value = |read: fn(&AgentReport) -> f64| report.as_ref().map(read);
            let integer = |read: fn(&AgentReport) -> i64| report.as_ref().map(read);
            let text = |read: fn(&AgentReport) -> &String| report.as_ref().map(|r| read(r).clone());
            Ok(ServerView {
                id: id.clone(),
                name: row.try_get("name")?,
                region: row.try_get("region")?,
                group_name: row.try_get("group_name")?,
                tags: row.try_get("tags")?,
                hidden: row.try_get::<i64, _>("hidden")? != 0,
                expires_at: row.try_get("expires_at")?,
                traffic_limit: row.try_get("traffic_limit")?,
                traffic_limit_type: row.try_get("traffic_limit_type")?,
                price: row.try_get("price")?,
                billing_cycle: row.try_get("billing_cycle")?,
                currency: row.try_get("currency")?,
                auto_renewal: row.try_get::<i64, _>("auto_renewal")? != 0,
                last_ip: row.try_get("last_ip")?,
                ip_v4: row.try_get("ip_v4")?,
                ip_v6: row.try_get("ip_v6")?,
                network_interface: row.try_get("network_interface")?,
                reset_day: row.try_get("reset_day")?,
                report_interval: row.try_get("report_interval")?,
                collect_interval: row.try_get("collect_interval")?,
                rx_correction: row.try_get("rx_correction")?,
                tx_correction: row.try_get("tx_correction")?,
                agent_mirror: row.try_get("agent_mirror")?,
                offline_notify_disabled: row.try_get::<i64, _>("offline_notify_disabled")? != 0,
                auto_update: row.try_get::<i64, _>("auto_update")? != 0,
                timestamp: report.as_ref().map(|report| report.timestamp),
                cpu: value(|r| r.cpu),
                load1: value(|r| r.load1),
                load5: value(|r| r.load5),
                load15: value(|r| r.load15),
                mem_used: integer(|r| r.mem_used),
                mem_total: integer(|r| r.mem_total),
                swap_used: integer(|r| r.swap_used),
                swap_total: integer(|r| r.swap_total),
                disk_used: integer(|r| r.disk_used),
                disk_total: integer(|r| r.disk_total),
                net_in: value(|r| r.net_in),
                net_out: value(|r| r.net_out),
                net_rx_total: integer(|r| r.net_rx_total),
                net_tx_total: integer(|r| r.net_tx_total),
                uptime: integer(|r| r.uptime),
                processes: integer(|r| r.processes),
                tcp_connections: integer(|r| r.tcp_connections),
                udp_connections: integer(|r| r.udp_connections),
                cpu_cores: integer(|r| r.cpu_cores),
                cpu_model: text(|r| &r.cpu_model),
                os: text(|r| &r.os),
                kernel: text(|r| &r.kernel),
                arch: text(|r| &r.arch),
                virtualization: text(|r| &r.virtualization),
                gpu_usage: value(|r| r.gpu_usage),
                gpu_model: text(|r| &r.gpu_model),
                agent_version: text(|r| &r.agent_version),
                disk_read_bps: value(|r| r.disk_read_bps),
                disk_write_bps: value(|r| r.disk_write_bps),
                disk_read_iops: value(|r| r.disk_read_iops),
                disk_write_iops: value(|r| r.disk_write_iops),
                disk_await_ms: value(|r| r.disk_await_ms),
                disk_utilization: value(|r| r.disk_utilization),
                disks: report.as_ref().map(|r| r.disks.clone()).unwrap_or_default(),
                gpus: report.as_ref().map(|r| r.gpus.clone()).unwrap_or_default(),
                latency: latency.get(&id).cloned().unwrap_or_default(),
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

pub async fn all_server_ids(db: &Database) -> Result<HashSet<String>> {
    let rows = sqlx::query("SELECT id FROM servers")
        .fetch_all(db.pool())
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("id").ok())
        .collect())
}

pub async fn create_server(db: &Database, input: &ServerInput) -> Result<(String, String)> {
    let id = uuid::Uuid::new_v4().to_string();
    let token = auth::random_token(32);
    let token_hash = auth::token_hash(&token);
    let timestamp = now();
    let mut transaction = db.pool().begin().await?;
    sqlx::query(db.sql(
        "INSERT INTO servers( \
         id, token_hash, name, region, group_name, tags, hidden, expires_at, traffic_limit, \
         traffic_limit_type, price, billing_cycle, currency, auto_renewal, last_ip, ip_v4, ip_v6, \
         network_interface, reset_day, report_interval, collect_interval, rx_correction, \
         tx_correction, agent_mirror, offline_notify_disabled, auto_update, created_at, updated_at, \
         sort_order) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '', '', '', ?, ?, ?, ?, ?, \
         ?, ?, ?, ?, ?, ?, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM servers))",
    ))
    .bind(&id)
    .bind(&token_hash)
    .bind(input.name.trim())
    .bind(input.region.trim())
    .bind(input.group_name.trim())
    .bind(input.tags.trim())
    .bind(i64::from(input.hidden))
    .bind(input.expires_at)
    .bind(input.traffic_limit)
    .bind(&input.traffic_limit_type)
    .bind(input.price)
    .bind(input.billing_cycle)
    .bind(input.currency.to_ascii_uppercase())
    .bind(i64::from(input.auto_renewal))
    .bind(input.network_interface.trim())
    .bind(input.reset_day)
    .bind(input.report_interval)
    .bind(input.collect_interval)
    .bind(input.rx_correction)
    .bind(input.tx_correction)
    .bind(input.agent_mirror.trim().trim_end_matches('/'))
    .bind(i64::from(input.offline_notify_disabled))
    .bind(i64::from(input.auto_update))
    .bind(timestamp)
    .bind(timestamp)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(db.sql(
        "INSERT INTO latency_task_servers(task_id, server_id, assigned_at) \
         SELECT id, ?, ? FROM latency_tasks WHERE default_enabled=1 \
         ON CONFLICT(task_id, server_id) DO NOTHING",
    ))
    .bind(&id)
    .bind(timestamp)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((id, token))
}

pub async fn update_server(db: &Database, id: &str, input: &ServerInput) -> Result<bool> {
    let result = sqlx::query(db.sql(
        "UPDATE servers SET name=?, region=?, group_name=?, tags=?, hidden=?, expires_at=?, \
         traffic_limit=?, traffic_limit_type=?, price=?, billing_cycle=?, currency=?, auto_renewal=?, \
         network_interface=?, reset_day=?, report_interval=?, collect_interval=?, rx_correction=?, \
         tx_correction=?, agent_mirror=?, offline_notify_disabled=?, auto_update=?, updated_at=? \
         WHERE id=?",
    ))
    .bind(input.name.trim())
    .bind(input.region.trim())
    .bind(input.group_name.trim())
    .bind(input.tags.trim())
    .bind(i64::from(input.hidden))
    .bind(input.expires_at)
    .bind(input.traffic_limit)
    .bind(&input.traffic_limit_type)
    .bind(input.price)
    .bind(input.billing_cycle)
    .bind(input.currency.to_ascii_uppercase())
    .bind(i64::from(input.auto_renewal))
    .bind(input.network_interface.trim())
    .bind(input.reset_day)
    .bind(input.report_interval)
    .bind(input.collect_interval)
    .bind(input.rx_correction)
    .bind(input.tx_correction)
    .bind(input.agent_mirror.trim().trim_end_matches('/'))
    .bind(i64::from(input.offline_notify_disabled))
    .bind(i64::from(input.auto_update))
    .bind(now())
    .bind(id)
    .execute(db.pool())
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete_server(db: &Database, id: &str) -> Result<bool> {
    let result = sqlx::query(db.sql("DELETE FROM servers WHERE id=?"))
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete_servers(db: &Database, ids: &[String]) -> Result<()> {
    let mut transaction = db.pool().begin().await?;
    for id in ids {
        sqlx::query(db.sql("DELETE FROM servers WHERE id=?"))
            .bind(id)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub async fn reorder_servers(db: &Database, ids: &[String]) -> Result<()> {
    let mut transaction = db.pool().begin().await?;
    for (index, id) in ids.iter().enumerate() {
        sqlx::query(db.sql("UPDATE servers SET sort_order=?, updated_at=? WHERE id=?"))
            .bind(index as i64)
            .bind(now())
            .bind(id)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub async fn rotate_server_token(db: &Database, id: &str) -> Result<Option<String>> {
    let token = auth::random_token(32);
    let result = sqlx::query(db.sql("UPDATE servers SET token_hash=?, updated_at=? WHERE id=?"))
        .bind(auth::token_hash(&token))
        .bind(now())
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok((result.rows_affected() > 0).then_some(token))
}

pub async fn server_name(db: &Database, id: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar::<_, String>(db.sql("SELECT name FROM servers WHERE id=?"))
            .bind(id)
            .fetch_optional(db.pool())
            .await?,
    )
}

pub async fn agent_identity(db: &Database, token: &str) -> Result<Option<AgentIdentity>> {
    let row = sqlx::query(db.sql(
        "SELECT id, hidden, report_interval, collect_interval, reset_day, rx_correction, \
         tx_correction FROM servers WHERE token_hash=?",
    ))
    .bind(auth::token_hash(token))
    .fetch_optional(db.pool())
    .await?;
    row.map(|row| {
        Ok::<AgentIdentity, sqlx::Error>(AgentIdentity {
            server_id: row.try_get("id")?,
            hidden: row.try_get::<i64, _>("hidden")? != 0,
            report_interval: row.try_get("report_interval")?,
            collect_interval: row.try_get("collect_interval")?,
            reset_day: row.try_get("reset_day")?,
            rx_correction: row.try_get("rx_correction")?,
            tx_correction: row.try_get("tx_correction")?,
        })
    })
    .transpose()
    .map_err(Into::into)
}

pub async fn agent_config(db: &Database, id: &str) -> Result<Option<serde_json::Value>> {
    let row = sqlx::query(db.sql(
        "SELECT report_interval, collect_interval, network_interface, agent_mirror, auto_update \
         FROM servers WHERE id=?",
    ))
    .bind(id)
    .fetch_optional(db.pool())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let tasks = tasks_for_server(db, id).await?;
    Ok(Some(serde_json::json!({
        "report_interval": row.try_get::<i64, _>("report_interval")?,
        "collect_interval": row.try_get::<i64, _>("collect_interval")?,
        "network_interface": row.try_get::<String, _>("network_interface")?,
        "agent_mirror": row.try_get::<String, _>("agent_mirror")?,
        "auto_update": row.try_get::<i64, _>("auto_update")? != 0,
        "latency_tasks": tasks,
    })))
}

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

    let mut transaction = db.pool().begin().await?;
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
            persisted_through: persisted_before,
        });
    }
    reports.retain(|report| report.timestamp > persisted_before);
    if reports.is_empty() {
        transaction.rollback().await?;
        return Ok(PersistResult {
            reports: Vec::new(),
            persisted_through: persisted_before,
        });
    }

    let mut traffic = load_traffic_state(db, &mut transaction, &identity.server_id).await?;
    for report in &mut reports {
        apply_traffic(report, &mut traffic, identity);
    }
    save_history_rows(db, &mut transaction, &identity.server_id, &reports).await?;
    save_latency_rows(db, &mut transaction, &identity.server_id, &reports).await?;
    let latest = reports.last().context("report batch became empty")?;
    let persisted_through = latest.timestamp;
    let latest_json = serde_json::to_string(latest)?;
    sqlx::query(db.sql(
        "INSERT INTO server_latest_state(server_id, latest_timestamp, cpu, mem_used, mem_total, \
         disk_used, disk_total, net_in, net_out, uptime, latest_json, last_batch_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(server_id) DO UPDATE SET latest_timestamp=excluded.latest_timestamp, \
         cpu=excluded.cpu, mem_used=excluded.mem_used, mem_total=excluded.mem_total, \
         disk_used=excluded.disk_used, disk_total=excluded.disk_total, net_in=excluded.net_in, \
         net_out=excluded.net_out, uptime=excluded.uptime, latest_json=excluded.latest_json, \
         last_batch_id=excluded.last_batch_id",
    ))
    .bind(&identity.server_id)
    .bind(latest.timestamp)
    .bind(latest.cpu)
    .bind(latest.mem_used)
    .bind(latest.mem_total)
    .bind(latest.disk_used)
    .bind(latest.disk_total)
    .bind(latest.net_in)
    .bind(latest.net_out)
    .bind(latest.uptime)
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
        persisted_through,
    })
}

fn valid_agent_report(report: &AgentReport, current: i64) -> bool {
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
            || !finite_between(latency.packet_loss, -1.0, 100.0)
    }) {
        return false;
    }

    serde_json::to_vec(report).is_ok_and(|encoded| encoded.len() <= AGENT_REPORT_MAX_BYTES)
}

async fn load_traffic_state(
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

fn apply_traffic(report: &mut AgentReport, state: &mut TrafficState, identity: &AgentIdentity) {
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

async fn save_traffic_state(
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

async fn save_history_rows(
    db: &Database,
    transaction: &mut Transaction<'_, Any>,
    server_id: &str,
    reports: &[AgentReport],
) -> Result<()> {
    for batch in reports.chunks(HISTORY_INSERT_BATCH_ROWS) {
        let mut arguments = AnyArguments::default();
        let mut parameter_index = 1_usize;
        let mut value_groups = Vec::with_capacity(batch.len());
        for report in batch {
            let mut placeholders = Vec::with_capacity(27);
            for _ in 0..27 {
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
        }
        let sql = format!(
            "INSERT INTO metric_history(server_id, timestamp, cpu, load1, load5, load15, \
             mem_used, mem_total, swap_used, swap_total, disk_used, disk_total, net_in, net_out, \
             net_rx_total, net_tx_total, uptime, processes, tcp_connections, udp_connections, \
             gpu_usage, disk_read_bps, disk_write_bps, disk_read_iops, disk_write_iops, \
             disk_await_ms, disk_utilization) VALUES {} \
             ON CONFLICT(server_id, timestamp) DO UPDATE SET cpu=excluded.cpu, \
             load1=excluded.load1, load5=excluded.load5, load15=excluded.load15, \
             mem_used=excluded.mem_used, mem_total=excluded.mem_total, \
             swap_used=excluded.swap_used, swap_total=excluded.swap_total, \
             disk_used=excluded.disk_used, disk_total=excluded.disk_total, net_in=excluded.net_in, \
             net_out=excluded.net_out, net_rx_total=excluded.net_rx_total, \
             net_tx_total=excluded.net_tx_total, uptime=excluded.uptime, \
             processes=excluded.processes, tcp_connections=excluded.tcp_connections, \
             udp_connections=excluded.udp_connections, gpu_usage=excluded.gpu_usage, \
             disk_read_bps=excluded.disk_read_bps, disk_write_bps=excluded.disk_write_bps, \
             disk_read_iops=excluded.disk_read_iops, disk_write_iops=excluded.disk_write_iops, \
             disk_await_ms=excluded.disk_await_ms, disk_utilization=excluded.disk_utilization",
            value_groups.join(",")
        );
        // SQL identifiers are fixed above; only generated placeholders are interpolated.
        sqlx::query_with(AssertSqlSafe(sql), arguments)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

async fn save_latency_rows(
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
        // SQL identifiers are fixed above; only generated placeholders are interpolated.
        sqlx::query_with(AssertSqlSafe(sql), arguments)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

pub async fn history(db: &Database, server_id: &str, hours: i64) -> Result<Vec<HistoryPoint>> {
    let hours = hours.clamp(1, 24 * 365);
    let bucket = (hours * 3600 / 720).max(1);
    let since = now() - hours * 3600;
    let rows = sqlx::query(db.sql(
        "SELECT bucket_timestamp, AVG(cpu) AS cpu, AVG(load1) AS load1, \
         AVG(load5) AS load5, AVG(load15) AS load15, \
         AVG(CAST(mem_used AS DOUBLE PRECISION)) AS mem_used, \
         MAX(mem_total) AS mem_total, \
         AVG(CAST(swap_used AS DOUBLE PRECISION)) AS swap_used, \
         MAX(swap_total) AS swap_total, \
         MAX(disk_used) AS disk_used, MAX(disk_total) AS disk_total, MAX(net_in) AS net_in, \
         MAX(net_out) AS net_out, MAX(net_rx_total) AS net_rx_total, \
         MAX(net_tx_total) AS net_tx_total, MAX(processes) AS processes, \
         MAX(tcp_connections) AS tcp_connections, MAX(udp_connections) AS udp_connections, \
         AVG(gpu_usage) AS gpu_usage, MAX(disk_read_bps) AS disk_read_bps, \
         MAX(disk_write_bps) AS disk_write_bps, MAX(disk_read_iops) AS disk_read_iops, \
         MAX(disk_write_iops) AS disk_write_iops, MAX(disk_await_ms) AS disk_await_ms, \
         MAX(disk_utilization) AS disk_utilization FROM ( \
           SELECT (timestamp / ?) * ? AS bucket_timestamp, metric_history.* \
           FROM metric_history WHERE server_id=? AND timestamp>=? \
         ) samples GROUP BY bucket_timestamp ORDER BY bucket_timestamp",
    ))
    .bind(bucket)
    .bind(bucket)
    .bind(server_id)
    .bind(since)
    .fetch_all(db.pool())
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(HistoryPoint {
                timestamp: row.try_get("bucket_timestamp")?,
                cpu: row.try_get::<f64, _>("cpu")?,
                load1: row.try_get::<f64, _>("load1")?,
                load5: row.try_get::<f64, _>("load5")?,
                load15: row.try_get::<f64, _>("load15")?,
                mem_used: row.try_get::<f64, _>("mem_used")?.round() as i64,
                mem_total: row.try_get("mem_total")?,
                swap_used: row.try_get::<f64, _>("swap_used")?.round() as i64,
                swap_total: row.try_get("swap_total")?,
                disk_used: row.try_get("disk_used")?,
                disk_total: row.try_get("disk_total")?,
                net_in: row.try_get("net_in")?,
                net_out: row.try_get("net_out")?,
                net_rx_total: row.try_get("net_rx_total")?,
                net_tx_total: row.try_get("net_tx_total")?,
                processes: row.try_get("processes")?,
                tcp_connections: row.try_get("tcp_connections")?,
                udp_connections: row.try_get("udp_connections")?,
                gpu_usage: row.try_get("gpu_usage")?,
                disk_read_bps: row.try_get("disk_read_bps")?,
                disk_write_bps: row.try_get("disk_write_bps")?,
                disk_read_iops: row.try_get("disk_read_iops")?,
                disk_write_iops: row.try_get("disk_write_iops")?,
                disk_await_ms: row.try_get("disk_await_ms")?,
                disk_utilization: row.try_get("disk_utilization")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

pub async fn clear_history(db: &Database) -> Result<()> {
    let mut transaction = db.pool().begin().await?;
    sqlx::query("DELETE FROM metric_history")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM latency_results")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn cleanup_database(db: &Database, retention_days: i64) -> Result<()> {
    let current = now();
    let cutoff = current - retention_days.clamp(1, 3650) * 86_400;
    let active_task_cutoff = current - REMOTE_TASK_ACTIVE_TTL_SECONDS;
    let mut transaction = db.pool().begin().await?;
    sqlx::query(db.sql(
        "UPDATE remote_tasks SET status='failed', completed_at=?, \
         result=CASE WHEN result='' THEN '任务等待超过 24 小时，已自动取消' ELSE result END \
         WHERE status IN ('pending','sent') AND requested_at<?",
    ))
    .bind(current)
    .bind(active_task_cutoff)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(db.sql("DELETE FROM metric_history WHERE timestamp<?"))
        .bind(cutoff)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(db.sql("DELETE FROM latency_results WHERE timestamp<?"))
        .bind(cutoff)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(db.sql(
        "DELETE FROM remote_tasks WHERE status IN ('success','failed') \
         AND completed_at IS NOT NULL AND completed_at<?",
    ))
    .bind(cutoff)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(db.sql("DELETE FROM alert_states WHERE active=0 AND updated_at<?"))
        .bind(cutoff)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn database_stats(db: &Database, offline_threshold: i64) -> Result<DatabaseStats> {
    let cutoff = now() - offline_threshold.clamp(30, 3600);
    let row = sqlx::query(db.sql(
        "SELECT (SELECT COUNT(*) FROM servers) AS server_count, \
         (SELECT COUNT(*) FROM server_latest_state WHERE latest_timestamp>=?) AS online_count, \
         ((SELECT COUNT(*) FROM metric_history) + (SELECT COUNT(*) FROM latency_results)) \
         AS history_rows",
    ))
    .bind(cutoff)
    .fetch_one(db.pool())
    .await?;
    Ok(DatabaseStats {
        server_count: row.try_get("server_count")?,
        online_count: row.try_get("online_count")?,
        history_rows: row.try_get("history_rows")?,
    })
}

pub async fn public_server_exists(db: &Database, server_id: &str) -> Result<bool> {
    let count = sqlx::query_scalar::<_, i64>(
        db.sql("SELECT COUNT(*) FROM servers WHERE id=? AND hidden=0"),
    )
    .bind(server_id)
    .fetch_one(db.pool())
    .await?;
    Ok(count > 0)
}

fn traffic_cycle_key(timestamp: i64, reset_day: i64) -> i64 {
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

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

async fn latest_latency_map(db: &Database) -> Result<HashMap<String, Vec<LatencySample>>> {
    let rows = sqlx::query(
        "SELECT a.server_id, t.id AS task_id, t.name, t.task_type, t.target, t.port, \
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
        result
            .entry(server_id.clone())
            .or_default()
            .push(LatencySample {
                task_id: row.try_get("task_id")?,
                server_id,
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
            });
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

async fn replace_task_servers(
    db: &Database,
    transaction: &mut Transaction<'_, Any>,
    task_id: &str,
    server_ids: &[String],
    timestamp: i64,
) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM latency_task_servers WHERE task_id=?"))
        .bind(task_id)
        .execute(&mut **transaction)
        .await?;
    for server_id in server_ids {
        sqlx::query(db.sql(
            "INSERT INTO latency_task_servers(task_id, server_id, assigned_at) VALUES (?, ?, ?)",
        ))
        .bind(task_id)
        .bind(server_id)
        .bind(timestamp)
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
                         ELSE CAST(0 AS DOUBLE PRECISION) END) ELSE -1 END AS latency_ms, \
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

fn latency_history_bucket_seconds(hours: i64, task_count: i64) -> i64 {
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

pub async fn list_alert_rules(db: &Database) -> Result<Vec<AlertRuleView>> {
    let rows = sqlx::query(
        "SELECT id, name, metric, threshold, duration_minutes, aggregation, enabled \
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
         enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(&id)
    .bind(input.name.trim())
    .bind(&input.metric)
    .bind(input.threshold)
    .bind(input.duration_minutes)
    .bind(&input.aggregation)
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
         enabled=?, updated_at=? WHERE id=?",
    ))
    .bind(input.name.trim())
    .bind(&input.metric)
    .bind(input.threshold)
    .bind(input.duration_minutes)
    .bind(&input.aggregation)
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

async fn replace_alert_servers(
    db: &Database,
    transaction: &mut Transaction<'_, Any>,
    rule_id: &str,
    server_ids: &[String],
) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM alert_rule_servers WHERE rule_id=?"))
        .bind(rule_id)
        .execute(&mut **transaction)
        .await?;
    for server_id in server_ids {
        sqlx::query(db.sql("INSERT INTO alert_rule_servers(rule_id, server_id) VALUES (?, ?)"))
            .bind(rule_id)
            .bind(server_id)
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
    sqlx::query(db.sql("DELETE FROM alert_states WHERE state_key LIKE ?"))
        .bind(format!("resource:{id}:%"))
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
    let rules = list_alert_rules(db).await?;
    let mut evaluations = Vec::new();
    for rule in rules.into_iter().filter(|rule| {
        rule.enabled
            && (rule.server_ids.is_empty() || rule.server_ids.iter().any(|id| id == server_id))
    }) {
        let expression = match rule.metric.as_str() {
            "cpu" => "cpu",
            "memory" => {
                "CASE WHEN mem_total>0 THEN CAST(mem_used AS DOUBLE PRECISION) * \
                 CAST(100 AS DOUBLE PRECISION) / CAST(mem_total AS DOUBLE PRECISION) \
                 ELSE CAST(0 AS DOUBLE PRECISION) END"
            }
            "disk" => {
                "CASE WHEN disk_total>0 THEN CAST(disk_used AS DOUBLE PRECISION) * \
                 CAST(100 AS DOUBLE PRECISION) / CAST(disk_total AS DOUBLE PRECISION) \
                 ELSE CAST(0 AS DOUBLE PRECISION) END"
            }
            "net_in" => "net_in / CAST(1048576 AS DOUBLE PRECISION)",
            "net_out" => "net_out / CAST(1048576 AS DOUBLE PRECISION)",
            _ => continue,
        };
        let query = if db.is_postgres() {
            format!(
                "SELECT MIN(timestamp) AS first_timestamp, AVG({expression}) AS average_value, \
                 MIN({expression}) AS minimum_value, COUNT(*) AS sample_count \
                 FROM metric_history WHERE server_id=$1 AND timestamp>=$2"
            )
        } else {
            format!(
                "SELECT MIN(timestamp) AS first_timestamp, AVG({expression}) AS average_value, \
                 MIN({expression}) AS minimum_value, COUNT(*) AS sample_count \
                 FROM metric_history WHERE server_id=? AND timestamp>=?"
            )
        };
        let since = now() - rule.duration_minutes * 60;
        // `expression` and the placeholder syntax above are selected only from
        // fixed literals; no request data is interpolated into this SQL.
        let row = sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
            .bind(server_id)
            .bind(since)
            .fetch_one(db.pool())
            .await?;
        let first: Option<i64> = row.try_get("first_timestamp")?;
        let count: i64 = row.try_get("sample_count")?;
        let average: Option<f64> = row.try_get("average_value")?;
        let minimum: Option<f64> = row.try_get("minimum_value")?;
        let value = if rule.aggregation == "continuous" {
            minimum.unwrap_or_default()
        } else {
            average.unwrap_or_default()
        };
        let covered = count > 0 && first.is_some_and(|first| first <= since + 5);
        evaluations.push(ResourceAlertEvaluation {
            triggered: covered && value >= rule.threshold,
            rule,
            value,
        });
    }
    Ok(evaluations)
}

pub async fn update_alert_state(
    db: &Database,
    key: &str,
    active: bool,
    details: &serde_json::Value,
) -> Result<bool> {
    let previous =
        sqlx::query_scalar::<_, i64>(db.sql("SELECT active FROM alert_states WHERE state_key=?"))
            .bind(key)
            .fetch_optional(db.pool())
            .await?
            .unwrap_or(0)
            != 0;
    sqlx::query(db.sql(
        "INSERT INTO alert_states(state_key, active, updated_at, details_json) VALUES (?, ?, ?, ?) \
         ON CONFLICT(state_key) DO UPDATE SET active=excluded.active, updated_at=excluded.updated_at, \
         details_json=excluded.details_json",
    ))
    .bind(key)
    .bind(i64::from(active))
    .bind(now())
    .bind(details.to_string())
    .execute(db.pool())
    .await?;
    Ok(previous != active)
}

pub async fn get_totp_secret(db: &Database, username: &str) -> Result<Option<(String, bool)>> {
    let row = sqlx::query(db.sql("SELECT totp_secret, enabled FROM admin_2fa WHERE username=?"))
        .bind(username)
        .fetch_optional(db.pool())
        .await?;
    row.map(|row| {
        Ok::<(String, bool), sqlx::Error>((
            row.try_get("totp_secret")?,
            row.try_get::<i64, _>("enabled")? != 0,
        ))
    })
    .transpose()
    .map_err(Into::into)
}

pub async fn save_totp_secret(
    db: &Database,
    username: &str,
    secret: &str,
    enabled: bool,
) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO admin_2fa(username, totp_secret, enabled, created_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT(username) DO UPDATE SET totp_secret=excluded.totp_secret, \
         enabled=excluded.enabled",
    ))
    .bind(username)
    .bind(secret)
    .bind(i64::from(enabled))
    .bind(now())
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn set_totp_enabled(db: &Database, username: &str, enabled: bool) -> Result<bool> {
    let result = sqlx::query(db.sql("UPDATE admin_2fa SET enabled=? WHERE username=?"))
        .bind(i64::from(enabled))
        .bind(username)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn rename_totp_user(db: &Database, old: &str, new: &str) -> Result<()> {
    if old == new {
        return Ok(());
    }
    sqlx::query(db.sql("UPDATE admin_2fa SET username=? WHERE username=?"))
        .bind(new)
        .bind(old)
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn create_remote_task(
    db: &Database,
    server_id: &str,
    command: &str,
    username: &str,
) -> Result<RemoteTaskInfo> {
    let task = RemoteTaskInfo {
        id: uuid::Uuid::new_v4().to_string(),
        server_id: server_id.to_string(),
        command: command.to_string(),
        status: "pending".to_string(),
        requested_by: username.to_string(),
        requested_at: now(),
        started_at: None,
        completed_at: None,
        result: String::new(),
        exit_code: None,
    };
    sqlx::query(db.sql(
        "INSERT INTO remote_tasks(id, server_id, command, status, requested_by, requested_at) \
         VALUES (?, ?, ?, 'pending', ?, ?)",
    ))
    .bind(&task.id)
    .bind(&task.server_id)
    .bind(&task.command)
    .bind(&task.requested_by)
    .bind(task.requested_at)
    .execute(db.pool())
    .await?;
    Ok(task)
}

pub async fn remote_task(db: &Database, id: &str) -> Result<Option<RemoteTaskInfo>> {
    let row = sqlx::query(db.sql(
        "SELECT id, server_id, command, status, requested_by, requested_at, started_at, \
         completed_at, result, exit_code FROM remote_tasks WHERE id=?",
    ))
    .bind(id)
    .fetch_optional(db.pool())
    .await?;
    row.as_ref()
        .map(remote_task_from_row)
        .transpose()
        .map_err(Into::into)
}

pub async fn pending_remote_tasks(db: &Database, server_id: &str) -> Result<Vec<RemoteTaskInfo>> {
    let current = now();
    let cutoff = current - REMOTE_TASK_ACTIVE_TTL_SECONDS;
    let mut transaction = db.pool().begin().await?;
    sqlx::query(db.sql(
        "UPDATE remote_tasks SET status='failed', completed_at=?, \
         result=CASE WHEN result='' THEN '任务等待超过 24 小时，已自动取消' ELSE result END \
         WHERE server_id=? AND status IN ('pending','sent') AND requested_at<?",
    ))
    .bind(current)
    .bind(server_id)
    .bind(cutoff)
    .execute(&mut *transaction)
    .await?;
    let rows = sqlx::query(db.sql(
        "SELECT id, server_id, command, status, requested_by, requested_at, started_at, \
         completed_at, result, exit_code FROM remote_tasks \
         WHERE server_id=? AND status IN ('pending','sent') \
         ORDER BY requested_at LIMIT 50",
    ))
    .bind(server_id)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    rows.iter()
        .map(remote_task_from_row)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn remote_task_from_row(
    row: &sqlx::any::AnyRow,
) -> std::result::Result<RemoteTaskInfo, sqlx::Error> {
    Ok(RemoteTaskInfo {
        id: row.try_get("id")?,
        server_id: row.try_get("server_id")?,
        command: row.try_get("command")?,
        status: row.try_get("status")?,
        requested_by: row.try_get("requested_by")?,
        requested_at: row.try_get("requested_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        result: row.try_get("result")?,
        exit_code: row.try_get("exit_code")?,
    })
}

pub async fn mark_remote_task_sent(db: &Database, id: &str, server_id: &str) -> Result<()> {
    sqlx::query(db.sql(
        "UPDATE remote_tasks SET status='sent', started_at=? \
         WHERE id=? AND server_id=? AND status='pending'",
    ))
    .bind(now())
    .bind(id)
    .bind(server_id)
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn update_remote_task_result(
    db: &Database,
    server_id: &str,
    id: &str,
    status: &str,
    result: &str,
    exit_code: Option<i64>,
) -> Result<bool> {
    let result = sqlx::query(db.sql(
        "UPDATE remote_tasks SET status=?, result=?, exit_code=?, completed_at=? \
         WHERE id=? AND server_id=? AND status IN ('pending','sent')",
    ))
    .bind(status)
    .bind(result)
    .bind(exit_code)
    .bind(now())
    .bind(id)
    .bind(server_id)
    .execute(db.pool())
    .await?;
    if result.rows_affected() > 0 {
        return Ok(true);
    }
    let completed = sqlx::query_scalar::<_, i64>(db.sql(
        "SELECT COUNT(*) FROM remote_tasks \
         WHERE id=? AND server_id=? AND status IN ('success','failed')",
    ))
    .bind(id)
    .bind(server_id)
    .fetch_one(db.pool())
    .await?;
    Ok(completed > 0)
}

pub async fn telegram_settings(db: &Database) -> Result<Option<TelegramSettingsView>> {
    Ok(raw_telegram_settings(db).await?.map(|mut settings| {
        if !settings.bot_token.is_empty() {
            settings.bot_token = SECRET_MASK.to_string();
        }
        settings
    }))
}

pub async fn raw_telegram_settings(db: &Database) -> Result<Option<TelegramSettingsView>> {
    let row = sqlx::query(
        "SELECT bot_token, chat_id, message_thread_id, template FROM notification_telegram \
         WHERE id=1",
    )
    .fetch_optional(db.pool())
    .await?;
    row.map(|row| {
        Ok::<TelegramSettingsView, sqlx::Error>(TelegramSettingsView {
            bot_token: row.try_get("bot_token")?,
            chat_id: row.try_get("chat_id")?,
            message_thread_id: row.try_get("message_thread_id")?,
            template: row.try_get("template")?,
        })
    })
    .transpose()
    .map_err(Into::into)
}

pub async fn save_telegram_settings(db: &Database, input: &TelegramSettingsInput) -> Result<()> {
    let current = raw_telegram_settings(db).await?;
    let token = if input.bot_token.trim() == SECRET_MASK {
        current.map(|value| value.bot_token).unwrap_or_default()
    } else {
        input.bot_token.trim().to_string()
    };
    sqlx::query(db.sql(
        "INSERT INTO notification_telegram(id, bot_token, chat_id, message_thread_id, template, \
         updated_at) VALUES (1, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET \
         bot_token=excluded.bot_token, chat_id=excluded.chat_id, \
         message_thread_id=excluded.message_thread_id, template=excluded.template, \
         updated_at=excluded.updated_at",
    ))
    .bind(token)
    .bind(input.chat_id.trim())
    .bind(input.message_thread_id)
    .bind(input.template.trim())
    .bind(now())
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn list_themes(db: &Database, active_id: &str) -> Result<Vec<ThemeView>> {
    let rows = sqlx::query(
        "SELECT id, name, description, url, version FROM themes ORDER BY created_at DESC",
    )
    .fetch_all(db.pool())
    .await?;
    let mut themes = vec![ThemeView {
        id: "builtin-nodeflare-glass".to_string(),
        name: "NodeFlare Glass".to_string(),
        description: "默认主题".to_string(),
        url: String::new(),
        version: crate::config::VERSION.to_string(),
        builtin: true,
        active: active_id == "builtin-nodeflare-glass",
    }];
    for row in rows {
        let id: String = row.try_get("id")?;
        themes.push(ThemeView {
            active: id == active_id,
            id,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            url: row.try_get("url")?,
            version: row.try_get("version")?,
            builtin: false,
        });
    }
    Ok(themes)
}

pub async fn create_theme(
    db: &Database,
    id: &str,
    input: &ThemeInput,
    resolved_url: &str,
    version: &str,
) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO themes(id, name, description, url, resolved_url, version, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(id)
    .bind(input.name.trim())
    .bind(input.description.trim())
    .bind(input.url.trim())
    .bind(resolved_url)
    .bind(version)
    .bind(now())
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn theme_exists(db: &Database, id: &str) -> Result<bool> {
    let count = sqlx::query_scalar::<_, i64>(db.sql("SELECT COUNT(*) FROM themes WHERE id=?"))
        .bind(id)
        .fetch_one(db.pool())
        .await?;
    Ok(count > 0)
}

pub async fn theme_resolved_url(db: &Database, id: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar::<_, String>(db.sql("SELECT resolved_url FROM themes WHERE id=?"))
            .bind(id)
            .fetch_optional(db.pool())
            .await?,
    )
}

pub async fn set_active_theme(db: &Database, id: &str) -> Result<bool> {
    if id != "builtin-nodeflare-glass" && !theme_exists(db, id).await? {
        return Ok(false);
    }
    super::set_setting(db, "active_theme_id", id).await?;
    Ok(true)
}

pub async fn delete_theme(db: &Database, id: &str) -> Result<bool> {
    let result = sqlx::query(db.sql("DELETE FROM themes WHERE id=?"))
        .bind(id)
        .execute(db.pool())
        .await?;
    if result.rows_affected() > 0
        && super::get_setting(db, "active_theme_id").await?.as_deref() == Some(id)
    {
        super::set_setting(db, "active_theme_id", "builtin-nodeflare-glass").await?;
    }
    Ok(result.rows_affected() > 0)
}

pub async fn create_theme_preview(db: &Database, theme_id: &str) -> Result<String> {
    let token = auth::random_token(24);
    sqlx::query(
        db.sql("INSERT INTO theme_previews(token_hash, theme_id, expires_at) VALUES (?, ?, ?)"),
    )
    .bind(auth::token_hash(&token))
    .bind(theme_id)
    .bind(now() + 600)
    .execute(db.pool())
    .await?;
    Ok(token)
}

pub async fn theme_preview_url(db: &Database, token: &str) -> Result<Option<String>> {
    Ok(sqlx::query_scalar::<_, String>(db.sql(
        "SELECT t.resolved_url FROM theme_previews p JOIN themes t ON t.id=p.theme_id \
         WHERE p.token_hash=? AND p.expires_at>?",
    ))
    .bind(auth::token_hash(token))
    .bind(now())
    .fetch_optional(db.pool())
    .await?)
}

pub async fn cleanup_theme_previews(db: &Database) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM theme_previews WHERE expires_at<=?"))
        .bind(now())
        .execute(db.pool())
        .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ExchangeSnapshot {
    pub base_currency: String,
    pub rates_json: String,
    pub source: String,
    pub rate_date: String,
    pub fetched_at: i64,
    pub attempted_at: i64,
}

pub async fn exchange_snapshot(db: &Database, base: &str) -> Result<Option<ExchangeSnapshot>> {
    let row = sqlx::query(db.sql(
        "SELECT base_currency, rates_json, source, rate_date, fetched_at, attempted_at \
         FROM exchange_rates WHERE base_currency=?",
    ))
    .bind(base)
    .fetch_optional(db.pool())
    .await?;
    row.map(|row| {
        Ok::<ExchangeSnapshot, sqlx::Error>(ExchangeSnapshot {
            base_currency: row.try_get("base_currency")?,
            rates_json: row.try_get("rates_json")?,
            source: row.try_get("source")?,
            rate_date: row.try_get("rate_date")?,
            fetched_at: row.try_get("fetched_at")?,
            attempted_at: row.try_get("attempted_at")?,
        })
    })
    .transpose()
    .map_err(Into::into)
}

pub async fn mark_exchange_attempt(db: &Database, base: &str, timestamp: i64) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO exchange_rates(base_currency, rates_json, source, rate_date, fetched_at, \
         attempted_at) VALUES (?, '{}', 'default', '', 0, ?) ON CONFLICT(base_currency) \
         DO UPDATE SET attempted_at=excluded.attempted_at",
    ))
    .bind(base)
    .bind(timestamp)
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn upsert_exchange_snapshot(
    db: &Database,
    base: &str,
    rates_json: &str,
    source: &str,
    date: &str,
    timestamp: i64,
) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO exchange_rates(base_currency, rates_json, source, rate_date, fetched_at, \
         attempted_at) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(base_currency) DO UPDATE SET \
         rates_json=excluded.rates_json, source=excluded.source, rate_date=excluded.rate_date, \
         fetched_at=excluded.fetched_at, attempted_at=excluded.attempted_at",
    ))
    .bind(base)
    .bind(rates_json)
    .bind(source)
    .bind(date)
    .bind(timestamp)
    .bind(timestamp)
    .execute(db.pool())
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_input(traffic_limit: i64) -> ServerInput {
        ServerInput {
            name: "Large traffic node".to_string(),
            region: "CN".to_string(),
            group_name: "Test".to_string(),
            tags: String::new(),
            hidden: false,
            expires_at: None,
            traffic_limit,
            traffic_limit_type: "sum".to_string(),
            price: 1.0,
            billing_cycle: 30,
            currency: "CNY".to_string(),
            auto_renewal: false,
            network_interface: String::new(),
            reset_day: 1,
            report_interval: 60,
            collect_interval: 5,
            rx_correction: 0,
            tx_correction: 0,
            agent_mirror: String::new(),
            offline_notify_disabled: false,
            auto_update: true,
        }
    }

    #[test]
    fn validates_agent_report_bounds() {
        let current = now();
        let mut report = AgentReport {
            timestamp: current,
            ..AgentReport::default()
        };
        assert!(valid_agent_report(&report, current));

        report.cpu = f64::NAN;
        assert!(!valid_agent_report(&report, current));
        report.cpu = 10.0;
        report.cpu_model = "x".repeat(513);
        assert!(!valid_agent_report(&report, current));
        report.cpu_model.clear();
        report.latency_results = (0..=AGENT_REPORT_MAX_LATENCY_RESULTS)
            .map(|index| AgentLatencyResult {
                task_id: format!("task-{index}"),
                timestamp: current,
                latency_ms: 10.0,
                packet_loss: 0.0,
            })
            .collect();
        assert!(!valid_agent_report(&report, current));
    }

    #[tokio::test]
    async fn sqlite_preserves_large_traffic_limits() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let expected = 100_i64 * 1024 * 1024 * 1024;
        let (id, _) = create_server(&db, &server_input(expected)).await.unwrap();
        assert_eq!(
            server_name(&db, &id).await.unwrap().as_deref(),
            Some("Large traffic node")
        );
        let raw =
            sqlx::query_scalar::<_, i64>(db.sql("SELECT traffic_limit FROM servers WHERE id=?"))
                .bind(&id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(raw, expected);
        let server = list_servers(&db, true)
            .await
            .unwrap()
            .into_iter()
            .find(|server| server.id == id)
            .unwrap();
        assert_eq!(server.traffic_limit, expected);
    }

    #[tokio::test]
    async fn sqlite_persists_large_agent_batches() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let (id, token) = create_server(&db, &server_input(0)).await.unwrap();
        let latency_task_id = create_latency_task(
            &db,
            &LatencyTaskInput {
                name: "Batch latency".to_string(),
                task_type: "tcp".to_string(),
                target: "example.com".to_string(),
                port: Some(443),
                interval_seconds: 60,
                default_enabled: false,
                server_ids: vec![id.clone()],
            },
        )
        .await
        .unwrap();
        let identity = agent_identity(&db, &token).await.unwrap().unwrap();
        let start = now() - 600;
        let reports = (0..600)
            .map(|offset| AgentReport {
                timestamp: start + offset,
                cpu: offset as f64 / 10.0,
                latency_results: vec![AgentLatencyResult {
                    task_id: latency_task_id.clone(),
                    timestamp: start + offset,
                    latency_ms: 10.0 + offset as f64 / 100.0,
                    packet_loss: 0.0,
                }],
                ..AgentReport::default()
            })
            .collect::<Vec<_>>();

        let result = save_agent_batch(&db, &identity, "large-batch", &reports, "127.0.0.1")
            .await
            .unwrap();
        let rows = sqlx::query_scalar::<_, i64>(
            db.sql("SELECT COUNT(*) FROM metric_history WHERE server_id=?"),
        )
        .bind(&id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        let latency_rows = sqlx::query_scalar::<_, i64>(
            db.sql("SELECT COUNT(*) FROM latency_results WHERE server_id=?"),
        )
        .bind(&id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(result.reports.len(), 600);
        assert_eq!(rows, 600);
        assert_eq!(latency_rows, 600);
    }

    #[tokio::test]
    async fn database_cleanup_expires_tasks_and_removes_retained_data() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let (server_id, _) = create_server(&db, &server_input(0)).await.unwrap();
        let old = now() - 3 * 86_400;
        sqlx::query(
            "INSERT INTO remote_tasks(\
             id, server_id, command, status, requested_by, requested_at, completed_at, result\
             ) VALUES \
             ('pending-old', ?, 'uptime', 'pending', 'admin', ?, NULL, ''), \
             ('success-old', ?, 'uptime', 'success', 'admin', ?, ?, 'done')",
        )
        .bind(&server_id)
        .bind(old)
        .bind(&server_id)
        .bind(old)
        .bind(old)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO alert_states(state_key, active, updated_at, details_json) VALUES \
             ('inactive-old', 0, ?, '{}'), ('active-old', 1, ?, '{}')",
        )
        .bind(old)
        .bind(old)
        .execute(db.pool())
        .await
        .unwrap();

        cleanup_database(&db, 1).await.unwrap();

        let pending_status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM remote_tasks WHERE id='pending-old'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let completed_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM remote_tasks WHERE id='success-old'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let inactive_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM alert_states WHERE state_key='inactive-old'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let active_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM alert_states WHERE state_key='active-old'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(pending_status, "failed");
        assert_eq!(completed_count, 0);
        assert_eq!(inactive_count, 0);
        assert_eq!(active_count, 1);
    }

    #[tokio::test]
    async fn remote_task_delivery_and_results_are_idempotent() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let (server_id, _) = create_server(&db, &server_input(0)).await.unwrap();
        let task = create_remote_task(&db, &server_id, "uptime", "admin")
            .await
            .unwrap();

        mark_remote_task_sent(&db, &task.id, &server_id)
            .await
            .unwrap();
        mark_remote_task_sent(&db, &task.id, &server_id)
            .await
            .unwrap();
        assert_eq!(
            remote_task(&db, &task.id).await.unwrap().unwrap().status,
            "sent"
        );

        assert!(
            update_remote_task_result(&db, &server_id, &task.id, "success", "done", Some(0))
                .await
                .unwrap()
        );
        assert!(
            update_remote_task_result(&db, &server_id, &task.id, "success", "done", Some(0))
                .await
                .unwrap()
        );
        let completed = remote_task(&db, &task.id).await.unwrap().unwrap();
        assert_eq!(completed.status, "success");
        assert_eq!(completed.result, "done");
        assert_eq!(completed.exit_code, Some(0));
    }

    #[tokio::test]
    async fn sqlite_migration_contains_optimized_indexes() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let indexes = sqlx::query_scalar::<_, String>(
            "SELECT name FROM sqlite_master WHERE type='index' AND name IN (\
             'metric_history_time',\
             'latency_results_time',\
             'remote_tasks_server_status_time',\
             'servers_public_sort')",
        )
        .fetch_all(db.pool())
        .await
        .unwrap()
        .into_iter()
        .collect::<HashSet<_>>();
        assert_eq!(indexes.len(), 4);
    }

    #[tokio::test]
    async fn sqlite_migration_rejects_invalid_server_configuration() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let result = sqlx::query(
            "INSERT INTO servers(\
             id, name, hidden, traffic_limit_type, currency, reset_day, report_interval, \
             collect_interval, token_hash, created_at, updated_at\
             ) VALUES ('invalid', 'Invalid', 2, 'sum', 'CNY', 1, 60, 5, 'token', 1, 1)",
        )
        .execute(db.pool())
        .await;
        assert!(result.is_err());
    }

    #[test]
    fn latency_history_bucket_scales_beyond_128_tasks() {
        assert!(latency_history_bucket_seconds(1, 256) > latency_history_bucket_seconds(1, 128));
    }
}
