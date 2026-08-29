use std::collections::HashMap;

use serde::{Deserialize, Serialize, Serializer};
use worker::{wasm_bindgen::JsValue, D1Database, D1PreparedStatement, Date, Result};

use crate::models::{
    AgentConfigView, AgentIdentityRow, AgentReport, AlertRuleInput, AlertRuleView, HistoryPoint,
    ServerInput, ServerView, SettingsInput, ThemeInput, ThemeView,
};

/// 分钟粒度历史的落库间隔。`HistoryMetricAggregate::point` 按分钟取桶，取 60
/// 让每一分钟都落到自己的桶里，1 小时图表（`history` 里 bucket=60）不再隔一个
/// 桶空一个。代价是每台每天 1440 行而不是 720 行。
pub(crate) const HISTORY_WRITE_INTERVAL_SECONDS: i64 = 60;
pub(crate) const MAX_HISTORY_RETENTION_DAYS: i64 = 30;

/// 分钟粒度历史按天轮换，`metric_history` 的上一代留在 `metric_history_old`。
/// 轮换而不是 DELETE 的理由：DELETE 每行计一行 D1 写入，DROP TABLE 不计。
/// 代价是当前表在刚轮换后是空的，所以任何近期读取都必须走 [`RECENT_HISTORY`]。
pub(crate) const HISTORY_ROTATION_INTERVAL_SECONDS: i64 = 86_400;

/// 分钟粒度历史的读取源。两代合起来覆盖 24-48 小时。
///
/// 轮换边界上同一分钟桶可能在两张表里各有一行（当前表刚建好、旧表还留着同
/// 一分钟的前半段）。所有消费方都按 `sample_count` 加权聚合，两个半行合并后
/// 与单行等价，所以重复是良性的，不需要去重。
pub(crate) const RECENT_HISTORY: &str =
    "SELECT * FROM metric_history UNION ALL SELECT * FROM metric_history_old";

/// `metric_history` 及其轮换副本的表结构，两处必须完全一致。
///
/// 轮换要在运行时重建表，所以 DDL 不能只存在于 migration 里。
/// `metric_history_ddl_matches_migration` 测试比对本函数与 migration 的文本，
/// 防止两边漂移。
pub(crate) fn metric_history_ddl(table: &str) -> String {
    format!(
        r#"CREATE TABLE {table} (
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  timestamp INTEGER NOT NULL,
  cpu REAL NOT NULL DEFAULT 0,
  load1 REAL NOT NULL DEFAULT 0,
  load5 REAL NOT NULL DEFAULT 0,
  load15 REAL NOT NULL DEFAULT 0,
  mem_used INTEGER NOT NULL DEFAULT 0,
  mem_total INTEGER NOT NULL DEFAULT 0,
  swap_used INTEGER NOT NULL DEFAULT 0,
  swap_total INTEGER NOT NULL DEFAULT 0,
  disk_used INTEGER NOT NULL DEFAULT 0,
  disk_total INTEGER NOT NULL DEFAULT 0,
  net_in REAL NOT NULL DEFAULT 0,
  net_out REAL NOT NULL DEFAULT 0,
  net_rx_total INTEGER NOT NULL DEFAULT 0,
  net_tx_total INTEGER NOT NULL DEFAULT 0,
  processes INTEGER NOT NULL DEFAULT 0,
  tcp_connections INTEGER NOT NULL DEFAULT 0,
  udp_connections INTEGER NOT NULL DEFAULT 0,
  gpu_usage REAL NOT NULL DEFAULT 0,
  disk_read_bps REAL NOT NULL DEFAULT 0,
  disk_write_bps REAL NOT NULL DEFAULT 0,
  disk_read_iops REAL NOT NULL DEFAULT 0,
  disk_write_iops REAL NOT NULL DEFAULT 0,
  disk_await_ms REAL NOT NULL DEFAULT 0,
  disk_utilization REAL NOT NULL DEFAULT 0,
  sample_count INTEGER NOT NULL DEFAULT 1 CHECK(sample_count > 0),
  latest_timestamp INTEGER NOT NULL,
  latency_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(latency_json)),
  PRIMARY KEY(server_id, timestamp)
) WITHOUT ROWID"#
    )
}

const SERVER_SELECT: &str = r#"
WITH latest_state AS (
  SELECT s0.id AS server_id, l.latest_json AS state
  FROM servers s0
  LEFT JOIN server_latest_state l ON l.server_id = s0.id
)
SELECT
  s.id, s.name, s.region, s.group_name, s.tags, s.hidden,
  s.expires_at, s.traffic_limit, s.traffic_limit_type,
  s.price, s.billing_cycle, s.currency, s.auto_renewal,
  s.last_ip,
  s.network_interface, s.reset_day, s.report_interval, s.collect_interval,
  s.rx_correction, s.tx_correction, s.agent_mirror, s.offline_notify_disabled, s.auto_update,
  s.created_at,
  CAST(json_extract(m.state, '$.report.timestamp') AS INTEGER) AS timestamp,
  CAST(json_extract(m.state, '$.report.cpu') AS REAL) AS cpu,
  CAST(json_extract(m.state, '$.report.load1') AS REAL) AS load1,
  CAST(json_extract(m.state, '$.report.load5') AS REAL) AS load5,
  CAST(json_extract(m.state, '$.report.load15') AS REAL) AS load15,
  CAST(json_extract(m.state, '$.report.mem_used') AS INTEGER) AS mem_used,
  CAST(json_extract(m.state, '$.report.mem_total') AS INTEGER) AS mem_total,
  CAST(json_extract(m.state, '$.report.swap_used') AS INTEGER) AS swap_used,
  CAST(json_extract(m.state, '$.report.swap_total') AS INTEGER) AS swap_total,
  CAST(json_extract(m.state, '$.report.disk_used') AS INTEGER) AS disk_used,
  CAST(json_extract(m.state, '$.report.disk_total') AS INTEGER) AS disk_total,
  CAST(json_extract(m.state, '$.report.net_in') AS REAL) AS net_in,
  CAST(json_extract(m.state, '$.report.net_out') AS REAL) AS net_out,
  CASE WHEN m.state IS NULL THEN NULL ELSE MAX(0,
    CAST(json_extract(m.state, '$.report.net_rx_total') AS INTEGER) + s.rx_correction)
  END AS net_rx_total,
  CASE WHEN m.state IS NULL THEN NULL ELSE MAX(0,
    CAST(json_extract(m.state, '$.report.net_tx_total') AS INTEGER) + s.tx_correction)
  END AS net_tx_total,
  CAST(json_extract(m.state, '$.report.uptime') AS INTEGER) AS uptime,
  CAST(json_extract(m.state, '$.report.processes') AS INTEGER) AS processes,
  CAST(json_extract(m.state, '$.report.tcp_connections') AS INTEGER) AS tcp_connections,
  CAST(json_extract(m.state, '$.report.udp_connections') AS INTEGER) AS udp_connections,
  CAST(json_extract(m.state, '$.report.cpu_cores') AS INTEGER) AS cpu_cores,
  json_extract(m.state, '$.report.cpu_model') AS cpu_model,
  json_extract(m.state, '$.report.os') AS os,
  json_extract(m.state, '$.report.kernel') AS kernel,
  json_extract(m.state, '$.report.arch') AS arch,
  json_extract(m.state, '$.report.virtualization') AS virtualization,
  CAST(json_extract(m.state, '$.report.gpu_usage') AS REAL) AS gpu_usage,
  json_extract(m.state, '$.report.gpu_model') AS gpu_model,
  json_extract(m.state, '$.report.agent_version') AS agent_version,
  CAST(json_extract(m.state, '$.report.disk_read_bps') AS REAL) AS disk_read_bps,
  CAST(json_extract(m.state, '$.report.disk_write_bps') AS REAL) AS disk_write_bps,
  CAST(json_extract(m.state, '$.report.disk_read_iops') AS REAL) AS disk_read_iops,
  CAST(json_extract(m.state, '$.report.disk_write_iops') AS REAL) AS disk_write_iops,
  CAST(json_extract(m.state, '$.report.disk_await_ms') AS REAL) AS disk_await_ms,
  CAST(json_extract(m.state, '$.report.disk_utilization') AS REAL) AS disk_utilization,
  json_extract(m.state, '$.report.disks') AS disk_info,
  json_extract(m.state, '$.report.gpus') AS gpu_info
FROM servers s
LEFT JOIN latest_state m ON m.server_id = s.id
"#;

pub const SECRET_MASK: &str = "********";

fn secret_for_api(value: &str) -> &str {
    if value.is_empty() {
        ""
    } else {
        SECRET_MASK
    }
}

fn serialize_secret<S>(value: &str, serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(secret_for_api(value))
}

#[derive(Debug, Deserialize)]
struct ThemeRow {
    id: String,
    name: String,
    description: String,
    url: String,
    version: String,
}

#[derive(Debug, Deserialize)]
struct AgentConfigRow {
    report_interval: i64,
    collect_interval: i64,
    network_interface: String,
    agent_mirror: String,
    auto_update: i64,
}

#[derive(Debug, Deserialize)]
struct ServerIdRow {
    id: String,
}

#[derive(Debug, Deserialize)]
pub struct AgentLiveContext {
    pub report_interval: i64,
    pub collect_interval: i64,
    pub reset_day: i64,
    pub cycle_key: i64,
    pub traffic_reset_day: i64,
    pub traffic_timestamp: i64,
    pub raw_rx: i64,
    pub raw_tx: i64,
    pub used_rx: i64,
    pub used_tx: i64,
    pub rx_correction: i64,
    pub tx_correction: i64,
    pub last_persisted_at: i64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TrafficCounterState {
    pub cycle_key: i64,
    pub reset_day: i64,
    pub timestamp: i64,
    pub raw_rx: i64,
    pub raw_tx: i64,
    pub used_rx: i64,
    pub used_tx: i64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HistoryMetricAggregate {
    #[serde(rename = "n")]
    count: u64,
    #[serde(rename = "t")]
    timestamp: i64,
    #[serde(rename = "c")]
    cpu_sum: f64,
    #[serde(rename = "a")]
    load1_sum: f64,
    #[serde(rename = "b")]
    load5_sum: f64,
    #[serde(rename = "d")]
    load15_sum: f64,
    #[serde(rename = "m")]
    mem_used_sum: f64,
    #[serde(rename = "w")]
    swap_used_sum: f64,
    #[serde(rename = "mt")]
    mem_total: i64,
    #[serde(rename = "wt")]
    swap_total: i64,
    #[serde(rename = "du")]
    disk_used: i64,
    #[serde(rename = "dt")]
    disk_total: i64,
    #[serde(rename = "nr")]
    net_rx_total: i64,
    #[serde(rename = "nt")]
    net_tx_total: i64,
    #[serde(rename = "g")]
    gpu_usage: f64,
    #[serde(rename = "ni")]
    net_in: f64,
    #[serde(rename = "no")]
    net_out: f64,
    #[serde(rename = "p")]
    processes: i64,
    #[serde(rename = "tc")]
    tcp_connections: i64,
    #[serde(rename = "uc")]
    udp_connections: i64,
    #[serde(rename = "dr")]
    disk_read_bps: f64,
    #[serde(rename = "dw")]
    disk_write_bps: f64,
    #[serde(rename = "ri")]
    disk_read_iops: f64,
    #[serde(rename = "wi")]
    disk_write_iops: f64,
    #[serde(rename = "da")]
    disk_await_ms: f64,
    #[serde(rename = "di")]
    disk_utilization: f64,
}

impl HistoryMetricAggregate {
    pub fn extend(&mut self, reports: &[AgentReport]) {
        for report in reports {
            self.count = self.count.saturating_add(1);
            self.cpu_sum += report.cpu;
            self.load1_sum += report.load1;
            self.load5_sum += report.load5;
            self.load15_sum += report.load15;
            self.mem_used_sum += report.mem_used as f64;
            self.swap_used_sum += report.swap_used as f64;

            self.net_in = self.net_in.max(report.net_in);
            self.net_out = self.net_out.max(report.net_out);
            self.processes = self.processes.max(report.processes);
            self.tcp_connections = self.tcp_connections.max(report.tcp_connections);
            self.udp_connections = self.udp_connections.max(report.udp_connections);
            self.disk_read_bps = self.disk_read_bps.max(report.disk_read_bps);
            self.disk_write_bps = self.disk_write_bps.max(report.disk_write_bps);
            self.disk_read_iops = self.disk_read_iops.max(report.disk_read_iops);
            self.disk_write_iops = self.disk_write_iops.max(report.disk_write_iops);
            self.disk_await_ms = self.disk_await_ms.max(report.disk_await_ms);
            self.disk_utilization = self.disk_utilization.max(report.disk_utilization);

            if report.timestamp >= self.timestamp {
                self.timestamp = report.timestamp;
                self.mem_total = report.mem_total;
                self.swap_total = report.swap_total;
                self.disk_used = report.disk_used;
                self.disk_total = report.disk_total;
                self.net_rx_total = report.net_rx_total;
                self.net_tx_total = report.net_tx_total;
                self.gpu_usage = report.gpu_usage;
            }
        }
    }

    pub fn merge(&mut self, other: Self) {
        if other.count == 0 {
            return;
        }
        if self.count == 0 {
            *self = other;
            return;
        }

        self.count = self.count.saturating_add(other.count);
        self.cpu_sum += other.cpu_sum;
        self.load1_sum += other.load1_sum;
        self.load5_sum += other.load5_sum;
        self.load15_sum += other.load15_sum;
        self.mem_used_sum += other.mem_used_sum;
        self.swap_used_sum += other.swap_used_sum;
        self.net_in = self.net_in.max(other.net_in);
        self.net_out = self.net_out.max(other.net_out);
        self.processes = self.processes.max(other.processes);
        self.tcp_connections = self.tcp_connections.max(other.tcp_connections);
        self.udp_connections = self.udp_connections.max(other.udp_connections);
        self.disk_read_bps = self.disk_read_bps.max(other.disk_read_bps);
        self.disk_write_bps = self.disk_write_bps.max(other.disk_write_bps);
        self.disk_read_iops = self.disk_read_iops.max(other.disk_read_iops);
        self.disk_write_iops = self.disk_write_iops.max(other.disk_write_iops);
        self.disk_await_ms = self.disk_await_ms.max(other.disk_await_ms);
        self.disk_utilization = self.disk_utilization.max(other.disk_utilization);

        if other.timestamp > self.timestamp {
            self.timestamp = other.timestamp;
            self.mem_total = other.mem_total;
            self.swap_total = other.swap_total;
            self.disk_used = other.disk_used;
            self.disk_total = other.disk_total;
            self.net_rx_total = other.net_rx_total;
            self.net_tx_total = other.net_tx_total;
            self.gpu_usage = other.gpu_usage;
        }
    }

    pub fn sample_count(&self) -> i64 {
        self.count.min(i64::MAX as u64) as i64
    }

    pub fn point(&self) -> Option<HistoryPoint> {
        if self.count == 0 {
            return None;
        }
        let count = self.count as f64;
        let rounded_mean = |sum: f64| (sum / count).round() as i64;
        Some(HistoryPoint {
            timestamp: self.timestamp / 60 * 60,
            cpu: self.cpu_sum / count,
            load1: self.load1_sum / count,
            load5: self.load5_sum / count,
            load15: self.load15_sum / count,
            mem_used: rounded_mean(self.mem_used_sum),
            mem_total: self.mem_total,
            swap_used: rounded_mean(self.swap_used_sum),
            swap_total: self.swap_total,
            disk_used: self.disk_used,
            disk_total: self.disk_total,
            net_in: self.net_in,
            net_out: self.net_out,
            net_rx_total: self.net_rx_total,
            net_tx_total: self.net_tx_total,
            processes: self.processes,
            tcp_connections: self.tcp_connections,
            udp_connections: self.udp_connections,
            gpu_usage: self.gpu_usage,
            disk_read_bps: self.disk_read_bps,
            disk_write_bps: self.disk_write_bps,
            disk_read_iops: self.disk_read_iops,
            disk_write_iops: self.disk_write_iops,
            disk_await_ms: self.disk_await_ms,
            disk_utilization: self.disk_utilization,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    pub site_name: String,
    pub site_description: String,
    pub site_announcement: String,
    pub logo_url: String,
    pub locale: String,
    pub public_dashboard: bool,
    pub offline_threshold_seconds: i64,
    pub history_retention_days: i64,
    #[serde(skip_serializing)]
    pub history_cache_version: i64,
    pub default_theme: String,
    pub active_theme_id: String,
    pub background_url: String,
    pub theme_options: serde_json::Value,
    pub show_search: bool,
    pub show_groups: bool,
    pub show_stats: bool,
    pub show_assets: bool,
    pub show_traffic: bool,
    pub show_speed: bool,
    pub show_price: bool,
    pub show_expiry: bool,
    pub show_latency: bool,
    pub show_uptime: bool,
    pub admin_username: String,
    pub admin_password_configured: bool,
    #[serde(skip_serializing)]
    pub admin_password_hash: String,
    #[serde(skip_serializing)]
    pub password_client_salt: String,
    pub turnstile_enabled: bool,
    pub turnstile_login_enabled: bool,
    #[serde(serialize_with = "serialize_secret")]
    pub turnstile_site_key: String,
    #[serde(serialize_with = "serialize_secret")]
    pub turnstile_secret_key: String,
    pub notification_enabled: bool,
    pub offline_alert_minutes: i64,
    pub expiry_alert_days: i64,
    pub traffic_alert_percentage: i64,
    #[serde(serialize_with = "serialize_secret")]
    pub cloudflare_account_id: String,
    #[serde(serialize_with = "serialize_secret")]
    pub cloudflare_api_token: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DatabaseStats {
    pub server_count: i64,
    pub online_count: i64,
    pub history_rows: i64,
    pub oldest_history: Option<i64>,
    pub newest_history: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExchangeRateSnapshot {
    pub base_currency: String,
    pub rates_json: String,
    pub source: String,
    pub rate_date: String,
    pub fetched_at: i64,
    pub attempted_at: i64,
}

#[derive(Debug, Deserialize)]
struct AlertRuleRow {
    id: String,
    name: String,
    metric: String,
    threshold: f64,
    duration_minutes: i64,
    aggregation: String,
    enabled: i64,
}

#[derive(Debug, Deserialize)]
struct AlertServerRow {
    rule_id: String,
    server_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AlertMetricRow {
    pub server_id: String,
    pub name: String,
    pub value: f64,
    pub sample_count: i64,
    pub first_timestamp: i64,
    pub last_timestamp: i64,
    pub report_interval: i64,
}

fn now() -> i64 {
    Date::now().as_millis() as i64 / 1000
}

fn text(value: &str) -> JsValue {
    JsValue::from_str(value)
}

fn number(value: impl ToString) -> JsValue {
    JsValue::from_f64(value.to_string().parse::<f64>().unwrap_or(0.0))
}

fn civil_date_from_unix_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

pub(crate) fn traffic_cycle_key(timestamp: i64, reset_day: i64) -> i64 {
    let (year, month, day) = civil_date_from_unix_days(timestamp.div_euclid(86_400));
    let boundary = reset_day.clamp(1, 31).min(days_in_month(year, month));
    let current_month = year * 12 + month - 1;
    if day >= boundary {
        current_month
    } else {
        current_month - 1
    }
}

pub async fn list_servers(db: &D1Database, include_hidden: bool) -> Result<Vec<ServerView>> {
    let filter = if include_hidden {
        ""
    } else {
        "WHERE s.hidden = 0"
    };
    let query = format!("{SERVER_SELECT} {filter} ORDER BY s.sort_order ASC, s.created_at ASC");
    db.prepare(query).all().await?.results()
}

/// Returns only requested server IDs that exist and are visible, preserving request order.
pub async fn visible_server_ids(db: &D1Database, ids: &[String]) -> Result<Vec<String>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let requested = serde_json::to_string(ids)?;
    let rows = db
        .prepare(
            r#"SELECT s.id
               FROM json_each(?1) requested
               JOIN servers s ON s.id = CAST(requested.value AS TEXT)
               WHERE s.hidden = 0
               ORDER BY CAST(requested.key AS INTEGER)"#,
        )
        .bind(&[text(&requested)])?
        .all()
        .await?
        .results::<ServerIdRow>()?;
    Ok(rows.into_iter().map(|row| row.id).collect())
}

pub async fn get_server(
    db: &D1Database,
    id: &str,
    include_hidden: bool,
) -> Result<Option<ServerView>> {
    let hidden = if include_hidden {
        ""
    } else {
        "AND s.hidden = 0"
    };
    let query = format!("{SERVER_SELECT} WHERE s.id = ?1 {hidden} LIMIT 1");
    db.prepare(query).bind(&[text(id)])?.first(None).await
}

pub async fn get_agent_identity(db: &D1Database, token: &str) -> Result<Option<AgentIdentityRow>> {
    db.prepare("SELECT id, hidden FROM servers WHERE token = ?1 LIMIT 1")
        .bind(&[text(token)])?
        .first(None)
        .await
}

pub async fn agent_live_context(db: &D1Database, id: &str) -> Result<Option<AgentLiveContext>> {
    let mut context: Option<AgentLiveContext> = db
        .prepare(
            r#"WITH latest_state AS (
             SELECT latest_json AS state FROM server_latest_state WHERE server_id = ?1
           ) SELECT
             s.report_interval,
             s.collect_interval,
             s.reset_day,
             COALESCE(CAST(json_extract(m.state, '$.traffic.cycle_key') AS INTEGER), 0) AS cycle_key,
             COALESCE(CAST(json_extract(m.state, '$.traffic.reset_day') AS INTEGER), s.reset_day) AS traffic_reset_day,
             COALESCE(CAST(json_extract(m.state, '$.traffic.timestamp') AS INTEGER), 0) AS traffic_timestamp,
             COALESCE(CAST(json_extract(m.state, '$.traffic.raw_rx') AS INTEGER), 0) AS raw_rx,
             COALESCE(CAST(json_extract(m.state, '$.traffic.raw_tx') AS INTEGER), 0) AS raw_tx,
             COALESCE(CAST(json_extract(m.state, '$.traffic.used_rx') AS INTEGER), 0) AS used_rx,
             COALESCE(CAST(json_extract(m.state, '$.traffic.used_tx') AS INTEGER), 0) AS used_tx,
             s.rx_correction,
             s.tx_correction,
             COALESCE(CAST(json_extract(m.state, '$.report.timestamp') AS INTEGER), 0) AS last_persisted_at
           FROM servers s
           LEFT JOIN latest_state m ON true
           WHERE s.id = ?1
           LIMIT 1"#,
        )
        .bind(&[text(id)])?
        .first(None)
        .await?;
    if let Some(context) = context.as_mut() {
        if context.cycle_key == 0 {
            context.cycle_key = traffic_cycle_key(crate::now(), context.reset_day);
        }
    }
    Ok(context)
}

pub async fn get_agent_token(db: &D1Database, id: &str) -> Result<Option<String>> {
    db.prepare("SELECT token FROM servers WHERE id = ?1 LIMIT 1")
        .bind(&[text(id)])?
        .first(Some("token"))
        .await
}

pub async fn update_last_ip(db: &D1Database, id: &str, ip: &str) -> Result<()> {
    db.prepare("UPDATE servers SET last_ip = ?2 WHERE id = ?1 AND last_ip != ?2")
        .bind(&[text(id), text(ip)])?
        .run()
        .await?;
    Ok(())
}

pub async fn agent_config(db: &D1Database, id: &str) -> Result<Option<AgentConfigView>> {
    let row: Option<AgentConfigRow> = db
        .prepare(
            "SELECT report_interval, collect_interval, network_interface, agent_mirror, auto_update \
             FROM servers WHERE id = ?1",
        )
        .bind(&[text(id)])?
        .first(None)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(AgentConfigView {
        report_interval: row.report_interval,
        collect_interval: row.collect_interval,
        network_interface: row.network_interface.trim().to_string(),
        agent_mirror: row.agent_mirror.trim().trim_end_matches('/').to_string(),
        auto_update: row.auto_update != 0,
        latency_tasks: crate::latency::tasks_for_server(db, id).await?,
    }))
}

pub async fn create_server(
    db: &D1Database,
    id: &str,
    token: &str,
    input: &ServerInput,
) -> Result<()> {
    let timestamp = now();
    db.prepare(
        r#"INSERT INTO servers (
          id, name, region, group_name, tags, hidden, sort_order,
          expires_at, traffic_limit, traffic_limit_type, price, billing_cycle,
          currency, auto_renewal, network_interface, reset_day,
          report_interval, collect_interval, rx_correction, tx_correction, agent_mirror,
          offline_notify_disabled, auto_update,
          token, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6,
          COALESCE((SELECT MAX(sort_order) + 1 FROM servers), 0),
          ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
          ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?24)"#,
    )
    .bind(&[
        text(id),
        text(input.name.trim()),
        text(input.region.trim()),
        text(input.group_name.trim()),
        text(input.tags.trim()),
        JsValue::from_bool(input.hidden),
        input.expires_at.map(number).unwrap_or(JsValue::NULL),
        number(input.traffic_limit),
        text(input.traffic_limit_type.trim()),
        number(input.price),
        number(input.billing_cycle),
        text(input.currency.trim()),
        JsValue::from_bool(input.auto_renewal),
        text(input.network_interface.trim()),
        number(input.reset_day),
        number(input.report_interval),
        number(input.collect_interval),
        number(input.rx_correction),
        number(input.tx_correction),
        text(input.agent_mirror.trim().trim_end_matches('/')),
        JsValue::from_bool(input.offline_notify_disabled),
        JsValue::from_bool(input.auto_update),
        text(token),
        number(timestamp),
    ])?
    .run()
    .await?;
    Ok(())
}

pub async fn update_server(db: &D1Database, id: &str, input: &ServerInput) -> Result<bool> {
    let result = db
        .prepare(
            r#"UPDATE servers SET
              name = ?2, region = ?3, group_name = ?4, tags = ?5,
              hidden = ?6, expires_at = ?7, traffic_limit = ?8,
              traffic_limit_type = ?9, price = ?10, billing_cycle = ?11,
              currency = ?12, auto_renewal = ?13,
              network_interface = ?14, reset_day = ?15, report_interval = ?16,
              collect_interval = ?17, rx_correction = ?18, tx_correction = ?19,
              agent_mirror = ?20, offline_notify_disabled = ?21, auto_update = ?22, updated_at = ?23
            WHERE id = ?1"#,
        )
        .bind(&[
            text(id),
            text(input.name.trim()),
            text(input.region.trim()),
            text(input.group_name.trim()),
            text(input.tags.trim()),
            JsValue::from_bool(input.hidden),
            input.expires_at.map(number).unwrap_or(JsValue::NULL),
            number(input.traffic_limit),
            text(input.traffic_limit_type.trim()),
            number(input.price),
            number(input.billing_cycle),
            text(input.currency.trim()),
            JsValue::from_bool(input.auto_renewal),
            text(input.network_interface.trim()),
            number(input.reset_day),
            number(input.report_interval),
            number(input.collect_interval),
            number(input.rx_correction),
            number(input.tx_correction),
            text(input.agent_mirror.trim().trim_end_matches('/')),
            JsValue::from_bool(input.offline_notify_disabled),
            JsValue::from_bool(input.auto_update),
            number(now()),
        ])?
        .run()
        .await?;
    Ok(result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0)
}

pub async fn delete_server(db: &D1Database, id: &str) -> Result<bool> {
    let result = db
        .prepare("DELETE FROM servers WHERE id = ?1")
        .bind(&[text(id)])?
        .run()
        .await?;
    Ok(result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0)
}

pub async fn delete_servers(db: &D1Database, ids: &[String]) -> Result<()> {
    let base = db.prepare("DELETE FROM servers WHERE id = ?1");
    let mut statements = Vec::with_capacity(ids.len());
    for id in ids {
        statements.push(base.clone().bind(&[text(id)])?);
    }
    if !statements.is_empty() {
        db.batch(statements).await?;
    }
    Ok(())
}

pub async fn reorder_servers(db: &D1Database, ids: &[String]) -> Result<()> {
    let base = db.prepare("UPDATE servers SET sort_order = ?2, updated_at = ?3 WHERE id = ?1");
    let timestamp = now();
    let mut statements = Vec::with_capacity(ids.len());
    for (index, id) in ids.iter().enumerate() {
        statements.push(
            base.clone()
                .bind(&[text(id), number(index), number(timestamp)])?,
        );
    }
    if !statements.is_empty() {
        db.batch(statements).await?;
    }
    Ok(())
}

pub async fn list_alert_rules(db: &D1Database) -> Result<Vec<AlertRuleView>> {
    let rows: Vec<AlertRuleRow> = db
        .prepare(
            "SELECT id, name, metric, threshold, duration_minutes, aggregation, enabled \
             FROM alert_rules ORDER BY created_at ASC",
        )
        .all()
        .await?
        .results()?;
    let assignments: Vec<AlertServerRow> = db
        .prepare(
            "SELECT rule_id, server_id FROM alert_rule_servers \
             ORDER BY rule_id ASC, server_id ASC",
        )
        .all()
        .await?
        .results()?;
    let mut servers_by_rule = HashMap::<String, Vec<String>>::new();
    for assignment in assignments {
        servers_by_rule
            .entry(assignment.rule_id)
            .or_default()
            .push(assignment.server_id);
    }
    let mut rules = Vec::with_capacity(rows.len());
    for row in rows {
        let server_ids = servers_by_rule.remove(&row.id).unwrap_or_default();
        rules.push(AlertRuleView {
            id: row.id,
            name: row.name,
            metric: row.metric,
            threshold: row.threshold,
            duration_minutes: row.duration_minutes,
            aggregation: row.aggregation,
            enabled: row.enabled != 0,
            server_ids,
        });
    }
    Ok(rules)
}

pub async fn has_enabled_alert_rules(db: &D1Database) -> Result<bool> {
    Ok(db
        .prepare("SELECT 1 AS present FROM alert_rules WHERE enabled=1 LIMIT 1")
        .first::<serde_json::Value>(None)
        .await?
        .is_some())
}

fn alert_rule_server_statements(
    db: &D1Database,
    rule_id: &str,
    server_ids: &[String],
) -> Result<Vec<D1PreparedStatement>> {
    let mut statements = vec![db
        .prepare("DELETE FROM alert_rule_servers WHERE rule_id = ?1")
        .bind(&[text(rule_id)])?];
    let insert = db.prepare("INSERT INTO alert_rule_servers(rule_id, server_id) VALUES (?1, ?2)");
    for server_id in server_ids {
        statements.push(insert.clone().bind(&[text(rule_id), text(server_id)])?);
    }
    Ok(statements)
}

pub async fn create_alert_rule(db: &D1Database, id: &str, input: &AlertRuleInput) -> Result<()> {
    let timestamp = now();
    let mut statements = vec![db
        .prepare(
            "INSERT INTO alert_rules(id, name, metric, threshold, duration_minutes, \
         aggregation, enabled, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        )
        .bind(&[
            text(id),
            text(input.name.trim()),
            text(input.metric.trim()),
            number(input.threshold),
            number(input.duration_minutes),
            text(input.aggregation.trim()),
            JsValue::from_bool(input.enabled),
            number(timestamp),
        ])?];
    statements.extend(alert_rule_server_statements(db, id, &input.server_ids)?);
    db.batch(statements).await?;
    Ok(())
}

pub async fn update_alert_rule(db: &D1Database, id: &str, input: &AlertRuleInput) -> Result<bool> {
    let exists = db
        .prepare("SELECT 1 AS present FROM alert_rules WHERE id=?1 LIMIT 1")
        .bind(&[text(id)])?
        .first::<serde_json::Value>(None)
        .await?
        .is_some();
    if !exists {
        return Ok(false);
    }
    let mut statements = vec![db
        .prepare(
            "UPDATE alert_rules SET name=?2, metric=?3, threshold=?4, duration_minutes=?5, \
         aggregation=?6, enabled=?7, updated_at=?8 WHERE id=?1",
        )
        .bind(&[
            text(id),
            text(input.name.trim()),
            text(input.metric.trim()),
            number(input.threshold),
            number(input.duration_minutes),
            text(input.aggregation.trim()),
            JsValue::from_bool(input.enabled),
            number(now()),
        ])?];
    statements.extend(alert_rule_server_statements(db, id, &input.server_ids)?);
    db.batch(statements).await?;
    Ok(true)
}

pub async fn delete_alert_rule(db: &D1Database, id: &str) -> Result<bool> {
    let result = db
        .prepare("DELETE FROM alert_rules WHERE id=?1")
        .bind(&[text(id)])?
        .run()
        .await?;
    Ok(result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0)
}

pub(crate) fn alert_window_covered(
    row: &AlertMetricRow,
    duration_minutes: i64,
    current_time: i64,
) -> bool {
    let window_seconds = duration_minutes.clamp(1, 1440) * 60;
    let sample_interval = row
        .report_interval
        .clamp(15, 3600)
        .max(HISTORY_WRITE_INTERVAL_SECONDS);
    let expected_samples = ((window_seconds + sample_interval - 1) / sample_interval).max(1);
    let required_samples = if expected_samples == 1 {
        1
    } else {
        ((expected_samples * 3 + 4) / 5).max(2)
    };
    let fresh_within = sample_interval * 2;
    row.sample_count >= required_samples
        && row.first_timestamp <= row.last_timestamp
        && current_time.saturating_sub(row.last_timestamp) <= fresh_within
}

pub async fn history(db: &D1Database, id: &str, hours: i64) -> Result<Vec<HistoryPoint>> {
    let hours = hours.clamp(1, 24 * 30);
    let since = now() - hours * 3600;
    let bucket = match hours {
        1 => 60,
        2..=4 => 120,
        5..=24 => 600,
        25..=168 => 3600,
        _ => 14_400,
    };
    let source = if hours <= 24 {
        RECENT_HISTORY.to_string()
    } else {
        format!("{RECENT_HISTORY} UNION ALL SELECT * FROM metric_history_hourly")
    };
    let query = format!(
        r#"SELECT
          (timestamp / {bucket}) * {bucket} AS timestamp,
          SUM(cpu * sample_count) / SUM(sample_count) AS cpu,
          SUM(load1 * sample_count) / SUM(sample_count) AS load1,
          SUM(load5 * sample_count) / SUM(sample_count) AS load5,
          SUM(load15 * sample_count) / SUM(sample_count) AS load15,
          CAST(SUM(mem_used * sample_count) / SUM(sample_count) AS INTEGER) AS mem_used,
          CAST(MAX(mem_total) AS INTEGER) AS mem_total,
          CAST(SUM(swap_used * sample_count) / SUM(sample_count) AS INTEGER) AS swap_used,
          CAST(MAX(swap_total) AS INTEGER) AS swap_total,
          CAST(SUM(disk_used * sample_count) / SUM(sample_count) AS INTEGER) AS disk_used,
          CAST(MAX(disk_total) AS INTEGER) AS disk_total,
          MAX(net_in) AS net_in, MAX(net_out) AS net_out,
          CAST(MAX(net_rx_total) AS INTEGER) AS net_rx_total,
          CAST(MAX(net_tx_total) AS INTEGER) AS net_tx_total,
          CAST(MAX(processes) AS INTEGER) AS processes,
          CAST(MAX(tcp_connections) AS INTEGER) AS tcp_connections,
          CAST(MAX(udp_connections) AS INTEGER) AS udp_connections,
          SUM(gpu_usage * sample_count) / SUM(sample_count) AS gpu_usage,
          MAX(disk_read_bps) AS disk_read_bps,
          MAX(disk_write_bps) AS disk_write_bps,
          MAX(disk_read_iops) AS disk_read_iops,
          MAX(disk_write_iops) AS disk_write_iops,
          MAX(disk_await_ms) AS disk_await_ms,
          MAX(disk_utilization) AS disk_utilization
        FROM ({source})
        WHERE server_id = ?1 AND timestamp >= ?2
        GROUP BY timestamp / {bucket}
        ORDER BY timestamp ASC
        LIMIT 1000"#,
    );
    db.prepare(query)
        .bind(&[text(id), number(since)])?
        .all()
        .await?
        .results()
}

pub(crate) fn reports_after(reports: &[AgentReport], timestamp: i64) -> Vec<AgentReport> {
    reports
        .iter()
        .filter(|report| report.timestamp > timestamp)
        .cloned()
        .collect()
}

pub(crate) fn valid_report_batch_id(batch_id: &str) -> bool {
    !batch_id.is_empty()
        && batch_id.len() <= 128
        && batch_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

pub(crate) struct ReportBatchRef<'a> {
    pub reports: &'a [AgentReport],
    pub id: &'a str,
}

pub async fn save_reports_with_history(
    db: &D1Database,
    server_id: &str,
    batch: ReportBatchRef<'_>,
    history_point: &HistoryPoint,
    sample_count: i64,
    traffic: &TrafficCounterState,
    latency: &crate::latency::LatencyMetricAggregates,
) -> Result<bool> {
    let Some(latest_report) = batch.reports.iter().max_by_key(|report| report.timestamp) else {
        return Ok(false);
    };
    let latest_timestamp = latest_report.timestamp;
    let mut latest_report = latest_report.clone();
    latest_report.net_rx_total = traffic.used_rx;
    latest_report.net_tx_total = traffic.used_tx;
    latest_report.latency_results.clear();
    let latest_json = serde_json::json!({
        "report": latest_report,
        "traffic": traffic,
    })
    .to_string();
    let latency_json = latency.stored_json()?;
    let batch_id = batch.id;
    let metric = db.prepare(
        r#"INSERT INTO metric_history (
          server_id, timestamp, cpu, load1, load5, load15,
          mem_used, mem_total, swap_used, swap_total, disk_used, disk_total,
          net_in, net_out, net_rx_total, net_tx_total, processes,
          tcp_connections, udp_connections, gpu_usage,
          disk_read_bps, disk_write_bps, disk_read_iops, disk_write_iops,
          disk_await_ms, disk_utilization, sample_count,
          latest_timestamp, latency_json
        ) SELECT
          ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
          ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26,
          ?27, ?28, ?29
        WHERE NOT EXISTS (
          SELECT 1 FROM server_latest_state
          WHERE server_id = ?1 AND last_batch_id = ?30
        ) ON CONFLICT(server_id, timestamp) DO UPDATE SET
          cpu=(metric_history.cpu * metric_history.sample_count +
            excluded.cpu * excluded.sample_count) /
            (metric_history.sample_count + excluded.sample_count),
          load1=(metric_history.load1 * metric_history.sample_count +
            excluded.load1 * excluded.sample_count) /
            (metric_history.sample_count + excluded.sample_count),
          load5=(metric_history.load5 * metric_history.sample_count +
            excluded.load5 * excluded.sample_count) /
            (metric_history.sample_count + excluded.sample_count),
          load15=(metric_history.load15 * metric_history.sample_count +
            excluded.load15 * excluded.sample_count) /
            (metric_history.sample_count + excluded.sample_count),
          mem_used=CAST((metric_history.mem_used * metric_history.sample_count +
            excluded.mem_used * excluded.sample_count) /
            (metric_history.sample_count + excluded.sample_count) AS INTEGER),
          mem_total=excluded.mem_total,
          swap_used=CAST((metric_history.swap_used * metric_history.sample_count +
            excluded.swap_used * excluded.sample_count) /
            (metric_history.sample_count + excluded.sample_count) AS INTEGER),
          swap_total=excluded.swap_total, disk_used=excluded.disk_used,
          disk_total=excluded.disk_total,
          net_in=MAX(metric_history.net_in, excluded.net_in),
          net_out=MAX(metric_history.net_out, excluded.net_out),
          net_rx_total=excluded.net_rx_total,
          net_tx_total=excluded.net_tx_total,
          processes=MAX(metric_history.processes, excluded.processes),
          tcp_connections=MAX(metric_history.tcp_connections, excluded.tcp_connections),
          udp_connections=MAX(metric_history.udp_connections, excluded.udp_connections),
          gpu_usage=excluded.gpu_usage,
          disk_read_bps=MAX(metric_history.disk_read_bps, excluded.disk_read_bps),
          disk_write_bps=MAX(metric_history.disk_write_bps, excluded.disk_write_bps),
          disk_read_iops=MAX(metric_history.disk_read_iops, excluded.disk_read_iops),
          disk_write_iops=MAX(metric_history.disk_write_iops, excluded.disk_write_iops),
          disk_await_ms=MAX(metric_history.disk_await_ms, excluded.disk_await_ms),
          disk_utilization=MAX(metric_history.disk_utilization, excluded.disk_utilization),
          sample_count=metric_history.sample_count + excluded.sample_count,
          latest_timestamp=MAX(metric_history.latest_timestamp, excluded.latest_timestamp),
          latency_json=CASE
            WHEN json_array_length(excluded.latency_json) = 0 THEN metric_history.latency_json
            ELSE (
              WITH latency_rows AS (
                SELECT
                  json_extract(value, '$.task_id') AS task_id,
                  CAST(json_extract(value, '$.timestamp') AS INTEGER) AS timestamp,
                  CAST(json_extract(value, '$.latency_ms') AS REAL) AS latency_ms,
                  CAST(json_extract(value, '$.packet_loss') AS REAL) AS packet_loss,
                  CAST(json_extract(value, '$.sample_count') AS INTEGER) AS sample_count,
                  CAST(json_extract(value, '$.success_count') AS INTEGER) AS success_count,
                  CAST(json_extract(value, '$.latest_timestamp') AS INTEGER) AS latest_timestamp,
                  CAST(json_extract(value, '$.latest_latency_ms') AS REAL) AS latest_latency_ms,
                  CAST(json_extract(value, '$.latest_packet_loss') AS REAL) AS latest_packet_loss
                FROM json_each(metric_history.latency_json)
                UNION ALL
                SELECT
                  json_extract(value, '$.task_id'),
                  CAST(json_extract(value, '$.timestamp') AS INTEGER),
                  CAST(json_extract(value, '$.latency_ms') AS REAL),
                  CAST(json_extract(value, '$.packet_loss') AS REAL),
                  CAST(json_extract(value, '$.sample_count') AS INTEGER),
                  CAST(json_extract(value, '$.success_count') AS INTEGER),
                  CAST(json_extract(value, '$.latest_timestamp') AS INTEGER),
                  CAST(json_extract(value, '$.latest_latency_ms') AS REAL),
                  CAST(json_extract(value, '$.latest_packet_loss') AS REAL)
                FROM json_each(excluded.latency_json)
              ), latency_ranked AS (
                SELECT *, ROW_NUMBER() OVER (
                  PARTITION BY task_id ORDER BY latest_timestamp DESC
                ) AS latest_position
                FROM latency_rows
              ), latency_tasks AS (
                SELECT task_id, MAX(timestamp) AS timestamp,
                  CASE WHEN SUM(success_count) > 0
                    THEN SUM(latency_ms * success_count) / SUM(success_count)
                    ELSE -1 END AS latency_ms,
                  SUM(packet_loss * sample_count) / SUM(sample_count) AS packet_loss,
                  SUM(sample_count) AS sample_count,
                  SUM(success_count) AS success_count,
                  MAX(CASE WHEN latest_position = 1 THEN latest_timestamp END) AS latest_timestamp,
                  MAX(CASE WHEN latest_position = 1 THEN latest_latency_ms END) AS latest_latency_ms,
                  MAX(CASE WHEN latest_position = 1 THEN latest_packet_loss END) AS latest_packet_loss
                FROM latency_ranked GROUP BY task_id
              ) SELECT json_group_array(json_object(
                'task_id', task_id, 'timestamp', timestamp,
                'latency_ms', latency_ms, 'packet_loss', packet_loss,
                'sample_count', sample_count, 'success_count', success_count,
                'latest_timestamp', latest_timestamp,
                'latest_latency_ms', latest_latency_ms,
                'latest_packet_loss', latest_packet_loss
              )) FROM (SELECT * FROM latency_tasks ORDER BY task_id)
            ) END
          WHERE excluded.latest_timestamp > metric_history.latest_timestamp"#,
    );
    let metric = metric.bind(&[
        text(server_id),
        number(history_point.timestamp),
        number(history_point.cpu),
        number(history_point.load1),
        number(history_point.load5),
        number(history_point.load15),
        number(history_point.mem_used),
        number(history_point.mem_total),
        number(history_point.swap_used),
        number(history_point.swap_total),
        number(history_point.disk_used),
        number(history_point.disk_total),
        number(history_point.net_in),
        number(history_point.net_out),
        number(traffic.used_rx),
        number(traffic.used_tx),
        number(history_point.processes),
        number(history_point.tcp_connections),
        number(history_point.udp_connections),
        number(history_point.gpu_usage),
        number(history_point.disk_read_bps),
        number(history_point.disk_write_bps),
        number(history_point.disk_read_iops),
        number(history_point.disk_write_iops),
        number(history_point.disk_await_ms),
        number(history_point.disk_utilization),
        number(sample_count.max(1)),
        number(latest_timestamp),
        text(&latency_json),
        text(batch_id),
    ])?;
    let snapshot = db
        .prepare(
            r#"INSERT INTO server_latest_state(
              server_id, latest_timestamp, latest_json, last_batch_id
            ) SELECT ?1, ?2, ?3, ?4 WHERE NOT EXISTS (
              SELECT 1 FROM server_latest_state
              WHERE server_id=?1 AND last_batch_id=?4
            ) ON CONFLICT(server_id) DO UPDATE SET
              latest_timestamp=excluded.latest_timestamp,
              latest_json=excluded.latest_json,
              last_batch_id=excluded.last_batch_id
            WHERE excluded.latest_timestamp >= server_latest_state.latest_timestamp"#,
        )
        .bind(&[
            text(server_id),
            number(latest_timestamp),
            text(&latest_json),
            text(batch_id),
        ])?;
    let results = db.batch(vec![metric, snapshot]).await?;
    Ok(results
        .first()
        .and_then(|result| result.meta().ok().flatten())
        .and_then(|meta| meta.changes)
        .unwrap_or(0)
        > 0)
}

fn bool_setting(values: &HashMap<String, String>, key: &str, fallback: bool) -> bool {
    values
        .get(key)
        .map(|value| value == "true")
        .unwrap_or(fallback)
}

fn string_setting(values: &HashMap<String, String>, key: &str, fallback: &str) -> String {
    values
        .get(key)
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

fn non_empty_string_setting(values: &HashMap<String, String>, key: &str, fallback: &str) -> String {
    values
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.trim())
        .to_string()
}

fn integer_setting(values: &HashMap<String, String>, key: &str, fallback: i64) -> i64 {
    values
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

pub async fn settings(
    db: &D1Database,
    default_name: &str,
    default_threshold: i64,
    default_retention: i64,
    default_username: &str,
) -> Result<SettingsView> {
    let raw = db
        .prepare("SELECT value FROM settings WHERE id = 1")
        .first::<String>(Some("value"))
        .await?
        .ok_or_else(|| worker::Error::RustError("站点设置尚未初始化".to_string()))?;
    let values: HashMap<String, String> = serde_json::from_str(&raw)
        .map_err(|_| worker::Error::RustError("站点设置格式无效".to_string()))?;
    Ok(SettingsView {
        site_name: values
            .get("site_name")
            .cloned()
            .unwrap_or_else(|| default_name.to_string()),
        site_description: string_setting(&values, "site_description", "轻量、实时的服务器运行状态"),
        site_announcement: string_setting(&values, "site_announcement", ""),
        logo_url: string_setting(&values, "logo_url", ""),
        locale: string_setting(&values, "locale", "zh-CN"),
        public_dashboard: bool_setting(&values, "public_dashboard", true),
        offline_threshold_seconds: values
            .get("offline_threshold_seconds")
            .and_then(|value| value.parse().ok())
            .unwrap_or(default_threshold),
        history_retention_days: integer_setting(
            &values,
            "history_retention_days",
            default_retention.clamp(1, MAX_HISTORY_RETENTION_DAYS),
        )
        .clamp(1, MAX_HISTORY_RETENTION_DAYS),
        history_cache_version: integer_setting(&values, "history_cache_version", 0),
        default_theme: string_setting(&values, "default_theme", "system"),
        active_theme_id: string_setting(&values, "active_theme_id", crate::theme::BUILTIN_THEME_ID),
        background_url: string_setting(&values, "background_url", ""),
        theme_options: values
            .get("theme_options")
            .and_then(|value| serde_json::from_str(value).ok())
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| serde_json::json!({})),
        show_search: bool_setting(&values, "show_search", true),
        show_groups: bool_setting(&values, "show_groups", true),
        show_stats: bool_setting(&values, "show_stats", true),
        show_assets: bool_setting(&values, "show_assets", true),
        show_traffic: bool_setting(&values, "show_traffic", true),
        show_speed: bool_setting(&values, "show_speed", true),
        show_price: bool_setting(&values, "show_price", true),
        show_expiry: bool_setting(&values, "show_expiry", true),
        show_latency: bool_setting(&values, "show_latency", true),
        show_uptime: bool_setting(&values, "show_uptime", true),
        admin_username: non_empty_string_setting(&values, "admin_username", default_username),
        admin_password_configured: values
            .get("admin_password_hash")
            .is_some_and(|value| !value.is_empty()),
        admin_password_hash: string_setting(&values, "admin_password_hash", ""),
        password_client_salt: string_setting(&values, "password_client_salt", ""),
        turnstile_enabled: bool_setting(&values, "turnstile_enabled", false),
        turnstile_login_enabled: bool_setting(&values, "turnstile_login_enabled", true),
        turnstile_site_key: string_setting(&values, "turnstile_site_key", ""),
        turnstile_secret_key: string_setting(&values, "turnstile_secret_key", ""),
        notification_enabled: bool_setting(&values, "notification_enabled", false),
        offline_alert_minutes: integer_setting(&values, "offline_alert_minutes", 5),
        expiry_alert_days: integer_setting(&values, "expiry_alert_days", 7),
        traffic_alert_percentage: integer_setting(&values, "traffic_alert_percentage", 80)
            .clamp(50, 100),
        cloudflare_account_id: string_setting(&values, "cloudflare_account_id", ""),
        cloudflare_api_token: string_setting(&values, "cloudflare_api_token", ""),
    })
}

pub async fn update_settings(
    db: &D1Database,
    input: &SettingsInput,
    password_hash: Option<&str>,
) -> Result<()> {
    let mut updates = Vec::<(&'static str, String)>::new();
    macro_rules! push_setting {
        ($key:expr, $value:expr) => {
            updates.push(($key, $value.to_string()));
        };
    }

    if let Some(value) = input.site_name.as_deref() {
        push_setting!("site_name", value.trim());
    }
    if let Some(value) = input.site_description.as_deref() {
        push_setting!("site_description", value.trim());
    }
    if let Some(value) = input.site_announcement.as_deref() {
        push_setting!("site_announcement", value.trim());
    }
    if let Some(value) = input.logo_url.as_deref() {
        push_setting!("logo_url", value.trim());
    }
    if let Some(value) = input.locale.as_deref() {
        push_setting!("locale", value.trim());
    }
    if let Some(value) = input.public_dashboard {
        push_setting!("public_dashboard", if value { "true" } else { "false" });
    }
    if let Some(value) = input.offline_threshold_seconds {
        let value = value.clamp(30, 3600).to_string();
        push_setting!("offline_threshold_seconds", &value);
    }
    if let Some(value) = input.history_retention_days {
        let value = value.clamp(1, MAX_HISTORY_RETENTION_DAYS).to_string();
        push_setting!("history_retention_days", &value);
    }
    if let Some(value) = input.default_theme.as_deref() {
        push_setting!("default_theme", value);
    }
    if let Some(value) = input.active_theme_id.as_deref() {
        push_setting!("active_theme_id", value.trim());
    }
    if let Some(value) = input.background_url.as_deref() {
        push_setting!("background_url", value.trim());
    }
    if let Some(value) = input.theme_options.as_ref() {
        let serialized = value.to_string();
        push_setting!("theme_options", &serialized);
    }
    if let Some(value) = input.show_search {
        push_setting!("show_search", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_groups {
        push_setting!("show_groups", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_stats {
        push_setting!("show_stats", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_assets {
        push_setting!("show_assets", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_traffic {
        push_setting!("show_traffic", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_speed {
        push_setting!("show_speed", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_price {
        push_setting!("show_price", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_expiry {
        push_setting!("show_expiry", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_latency {
        push_setting!("show_latency", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_uptime {
        push_setting!("show_uptime", if value { "true" } else { "false" });
    }
    if let Some(value) = input.admin_username.as_deref() {
        push_setting!("admin_username", value.trim());
    }
    if let Some(value) = password_hash {
        push_setting!("admin_password_hash", value);
    }
    if let Some(value) = input.turnstile_enabled {
        push_setting!("turnstile_enabled", if value { "true" } else { "false" });
    }
    if let Some(value) = input.turnstile_login_enabled {
        push_setting!(
            "turnstile_login_enabled",
            if value { "true" } else { "false" }
        );
    }
    if let Some(value) = input
        .turnstile_site_key
        .as_deref()
        .filter(|value| value.trim() != SECRET_MASK)
    {
        push_setting!("turnstile_site_key", value.trim());
    }
    if let Some(value) = input
        .turnstile_secret_key
        .as_deref()
        .filter(|value| value.trim() != SECRET_MASK)
    {
        push_setting!("turnstile_secret_key", value.trim());
    }
    if let Some(value) = input.notification_enabled {
        push_setting!("notification_enabled", if value { "true" } else { "false" });
    }
    if let Some(value) = input.offline_alert_minutes {
        let value = value.clamp(2, 1440).to_string();
        push_setting!("offline_alert_minutes", &value);
    }
    if let Some(value) = input.expiry_alert_days {
        let value = value.clamp(0, 365).to_string();
        push_setting!("expiry_alert_days", &value);
    }
    if let Some(value) = input.traffic_alert_percentage {
        let value = value.clamp(50, 100).to_string();
        push_setting!("traffic_alert_percentage", &value);
    }
    if let Some(value) = input
        .cloudflare_account_id
        .as_deref()
        .filter(|value| value.trim() != SECRET_MASK)
    {
        push_setting!("cloudflare_account_id", value.trim());
    }
    if let Some(value) = input
        .cloudflare_api_token
        .as_deref()
        .filter(|value| value.trim() != SECRET_MASK)
    {
        push_setting!("cloudflare_api_token", value.trim());
    }
    if !updates.is_empty() {
        let patch = updates
            .into_iter()
            .map(|(key, value)| (key.to_string(), serde_json::Value::String(value)))
            .collect::<serde_json::Map<_, _>>();
        db.prepare("UPDATE settings SET value=json_patch(value, ?1), updated_at=?2 WHERE id=1")
            .bind(&[
                text(&serde_json::Value::Object(patch).to_string()),
                number(now()),
            ])?
            .run()
            .await?;
    }
    Ok(())
}

pub async fn list_themes(db: &D1Database, active_id: &str) -> Result<Vec<ThemeView>> {
    let rows: Vec<ThemeRow> = db
        .prepare("SELECT id, name, description, url, version FROM themes ORDER BY created_at DESC")
        .all()
        .await?
        .results()?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let active = row.id == active_id;
            ThemeView {
                id: row.id,
                name: row.name,
                description: row.description,
                url: row.url,
                version: row.version,
                builtin: false,
                active,
            }
        })
        .collect())
}

pub async fn create_theme(
    db: &D1Database,
    id: &str,
    input: &ThemeInput,
    source_url: &str,
    version: &str,
    timestamp: i64,
) -> Result<()> {
    db.prepare(
        "INSERT INTO themes(id, name, description, url, version, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(&[
        text(id),
        text(input.name.trim()),
        text(input.description.trim()),
        text(source_url),
        text(version),
        number(timestamp),
    ])?
    .run()
    .await?;
    Ok(())
}

pub async fn theme_exists(db: &D1Database, id: &str) -> Result<bool> {
    let row: Option<ThemeRow> = db
        .prepare("SELECT id, name, description, url, version FROM themes WHERE id = ?1")
        .bind(&[text(id)])?
        .first(None)
        .await?;
    Ok(row.is_some())
}

pub async fn theme_url(db: &D1Database, id: &str) -> Result<Option<String>> {
    let row: Option<ThemeRow> = db
        .prepare("SELECT id, name, description, url, version FROM themes WHERE id = ?1")
        .bind(&[text(id)])?
        .first(None)
        .await?;
    row.map(|theme| crate::theme::resolve_url(&theme.url).map(|resolved| resolved.resolved_url))
        .transpose()
}

pub async fn update_theme_version(db: &D1Database, id: &str, version: &str) -> Result<()> {
    db.prepare("UPDATE themes SET version = ?2 WHERE id = ?1")
        .bind(&[text(id), text(version)])?
        .run()
        .await?;
    Ok(())
}

pub async fn set_active_theme(db: &D1Database, id: &str) -> Result<bool> {
    if id != crate::theme::BUILTIN_THEME_ID && !theme_exists(db, id).await? {
        return Ok(false);
    }
    save_setting(db, "active_theme_id", id).await?;
    Ok(true)
}

pub async fn delete_theme(db: &D1Database, id: &str) -> Result<bool> {
    let result = db
        .prepare("DELETE FROM themes WHERE id = ?1")
        .bind(&[text(id)])?
        .run()
        .await?;
    let deleted = result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0;
    if deleted && get_setting(db, "active_theme_id").await?.as_deref() == Some(id) {
        save_setting(db, "active_theme_id", crate::theme::BUILTIN_THEME_ID).await?;
    }
    Ok(deleted)
}

/// 把分钟历史压成小时粒度，然后轮换分钟表、按保留期删除过期小时行。
///
/// 压缩读两代分钟表，区间是 `[保留期下界, 当前整小时)`，小时表按覆盖语义写入
/// （不累加，见那条 SQL 里的注释）。三者合起来让压缩天然幂等：
///
/// - 读两代保证轮换即将丢弃的行一定已经压过，不需要「哪一代压过了」的状态标记。
/// - 上界卡在整小时，避免把还在写的小时压进归档。
/// - 覆盖语义让重复压缩变成重算同一个值，跑几次结果都一样。
///
/// 所以这个函数崩在任何一步、下一次 cron 重跑都是对的。
///
/// 顺序是「压缩 + 删除过期小时行（同一个 batch）→ 轮换分钟表」。
///
/// 调用方（`src/lib.rs` 的 cron）已经把频率限制在一天一次，与
/// [`HISTORY_ROTATION_INTERVAL_SECONDS`] 一致。分钟数据因此保留 24-48 小时而不
/// 是精确 24 小时；小时数据仍然精确按 `retention_days` 删除。
pub async fn cleanup_history(db: &D1Database, retention_days: i64) -> Result<()> {
    let current = now();
    let cutoff = history_cutoff(current, retention_days);
    // 只压已经走完的小时；当前这个未完成的小时留给下一次 cron。
    let complete_hours_before = current.div_euclid(3600) * 3600;
    let compact = db
        .prepare(
            r#"WITH recent AS (
          -- 两代分钟表，等价于 db::RECENT_HISTORY。下面 ranked 和 latency_ranked
          -- 都从这里取，保证指标和延迟压的是同一批源行。
          SELECT * FROM metric_history UNION ALL SELECT * FROM metric_history_old
        ), ranked AS (
          SELECT h.*, (h.timestamp / 3600) * 3600 AS bucket,
            ROW_NUMBER() OVER (
              PARTITION BY h.server_id, h.timestamp / 3600
              ORDER BY latest_timestamp DESC
            ) AS latest_position
          FROM recent h
          WHERE h.timestamp >= ?1 AND h.timestamp < ?2
        ), metrics AS (
          SELECT
            server_id, bucket,
            SUM(cpu * sample_count) / SUM(sample_count) AS cpu,
            SUM(load1 * sample_count) / SUM(sample_count) AS load1,
            SUM(load5 * sample_count) / SUM(sample_count) AS load5,
            SUM(load15 * sample_count) / SUM(sample_count) AS load15,
            CAST(SUM(mem_used * sample_count) / SUM(sample_count) AS INTEGER) AS mem_used,
            MAX(mem_total) AS mem_total,
            CAST(SUM(swap_used * sample_count) / SUM(sample_count) AS INTEGER) AS swap_used,
            MAX(swap_total) AS swap_total,
            CAST(SUM(disk_used * sample_count) / SUM(sample_count) AS INTEGER) AS disk_used,
            MAX(disk_total) AS disk_total, MAX(net_in) AS net_in, MAX(net_out) AS net_out,
            MAX(net_rx_total) AS net_rx_total, MAX(net_tx_total) AS net_tx_total,
            CAST(MAX(processes) AS INTEGER) AS processes,
            CAST(MAX(tcp_connections) AS INTEGER) AS tcp_connections,
            CAST(MAX(udp_connections) AS INTEGER) AS udp_connections,
            SUM(gpu_usage * sample_count) / SUM(sample_count) AS gpu_usage,
            MAX(disk_read_bps) AS disk_read_bps,
            MAX(disk_write_bps) AS disk_write_bps,
            MAX(disk_read_iops) AS disk_read_iops,
            MAX(disk_write_iops) AS disk_write_iops,
            MAX(disk_await_ms) AS disk_await_ms,
            MAX(disk_utilization) AS disk_utilization,
            SUM(sample_count) AS sample_count,
            MAX(CASE WHEN latest_position = 1 THEN latest_timestamp END) AS latest_timestamp
          FROM ranked GROUP BY server_id, bucket
        ), latency_ranked AS (
          SELECT h.server_id, (h.timestamp / 3600) * 3600 AS bucket,
            json_extract(j.value, '$.task_id') AS task_id,
            CAST(json_extract(j.value, '$.latency_ms') AS REAL) AS latency_ms,
            CAST(json_extract(j.value, '$.packet_loss') AS REAL) AS packet_loss,
            CAST(json_extract(j.value, '$.sample_count') AS INTEGER) AS sample_count,
            CAST(json_extract(j.value, '$.success_count') AS INTEGER) AS success_count,
            CAST(json_extract(j.value, '$.latest_timestamp') AS INTEGER) AS latest_timestamp,
            CAST(json_extract(j.value, '$.latest_latency_ms') AS REAL) AS latest_latency_ms,
            CAST(json_extract(j.value, '$.latest_packet_loss') AS REAL) AS latest_packet_loss,
            ROW_NUMBER() OVER (
              PARTITION BY h.server_id, h.timestamp / 3600, json_extract(j.value, '$.task_id')
              ORDER BY CAST(json_extract(j.value, '$.latest_timestamp') AS INTEGER) DESC
            ) AS latest_position
          FROM recent h, json_each(h.latency_json) j
          WHERE h.timestamp >= ?1 AND h.timestamp < ?2
        ), latency_tasks AS (
          SELECT server_id, bucket, task_id,
            CASE WHEN SUM(success_count) > 0
              THEN SUM(latency_ms * success_count) / SUM(success_count)
              ELSE -1 END AS latency_ms,
            SUM(packet_loss * sample_count) / SUM(sample_count) AS packet_loss,
            SUM(sample_count) AS sample_count,
            SUM(success_count) AS success_count,
            MAX(CASE WHEN latest_position = 1 THEN latest_timestamp END) AS latest_timestamp,
            MAX(CASE WHEN latest_position = 1 THEN latest_latency_ms END) AS latest_latency_ms,
            MAX(CASE WHEN latest_position = 1 THEN latest_packet_loss END) AS latest_packet_loss
          FROM latency_ranked GROUP BY server_id, bucket, task_id
        ), latency_packed AS (
          SELECT server_id, bucket, json_group_array(json_object(
            'task_id', task_id, 'timestamp', bucket,
            'latency_ms', latency_ms, 'packet_loss', packet_loss,
            'sample_count', sample_count, 'success_count', success_count,
            'latest_timestamp', latest_timestamp,
            'latest_latency_ms', latest_latency_ms,
            'latest_packet_loss', latest_packet_loss
          )) AS latency_json
          FROM latency_tasks GROUP BY server_id, bucket
        ) INSERT INTO metric_history_hourly (
          server_id, timestamp, cpu, load1, load5, load15, mem_used, mem_total,
          swap_used, swap_total, disk_used, disk_total, net_in, net_out,
          net_rx_total, net_tx_total, processes, tcp_connections, udp_connections,
          gpu_usage, disk_read_bps, disk_write_bps, disk_read_iops, disk_write_iops,
          disk_await_ms, disk_utilization, sample_count,
          latest_timestamp, latency_json
        ) SELECT
          m.server_id, m.bucket, m.cpu, m.load1, m.load5, m.load15, m.mem_used,
          m.mem_total, m.swap_used, m.swap_total, m.disk_used, m.disk_total,
          m.net_in, m.net_out, m.net_rx_total, m.net_tx_total, m.processes,
          m.tcp_connections, m.udp_connections, m.gpu_usage, m.disk_read_bps,
          m.disk_write_bps, m.disk_read_iops, m.disk_write_iops, m.disk_await_ms,
          m.disk_utilization, m.sample_count, m.latest_timestamp,
          COALESCE(l.latency_json, '[]')
        FROM metrics m LEFT JOIN latency_packed l
          ON l.server_id = m.server_id AND l.bucket = m.bucket
        WHERE true
        -- 直接覆盖，不做累加。上面的 CTE 是从两代分钟表里把这个小时桶的全部
        -- 源行重新算了一遍，所以 excluded 就是该桶的完整真值。
        -- 一个桶的源行在分钟表里可见 24-48 小时，而它在写完后的第一次 cron 就会
        -- 被压缩，所以每次压缩看到的都是完整的桶；等源行被轮换丢弃时，这个桶也
        -- 不再出现在 union 里，不会再触发 upsert。
        -- 覆盖语义顺带让压缩对乱序到达免疫：迟到的行只是让下一次重算包含它。
        ON CONFLICT(server_id, timestamp) DO UPDATE SET
          cpu=excluded.cpu, load1=excluded.load1, load5=excluded.load5,
          load15=excluded.load15, mem_used=excluded.mem_used,
          mem_total=excluded.mem_total, swap_used=excluded.swap_used,
          swap_total=excluded.swap_total, disk_used=excluded.disk_used,
          disk_total=excluded.disk_total, net_in=excluded.net_in,
          net_out=excluded.net_out, net_rx_total=excluded.net_rx_total,
          net_tx_total=excluded.net_tx_total, processes=excluded.processes,
          tcp_connections=excluded.tcp_connections,
          udp_connections=excluded.udp_connections,
          gpu_usage=excluded.gpu_usage, disk_read_bps=excluded.disk_read_bps,
          disk_write_bps=excluded.disk_write_bps,
          disk_read_iops=excluded.disk_read_iops,
          disk_write_iops=excluded.disk_write_iops,
          disk_await_ms=excluded.disk_await_ms,
          disk_utilization=excluded.disk_utilization,
          sample_count=excluded.sample_count,
          latest_timestamp=excluded.latest_timestamp,
          latency_json=excluded.latency_json"#,
        )
        .bind(&[number(cutoff), number(complete_hours_before)])?;
    let delete_archive = db
        .prepare("DELETE FROM metric_history_hourly WHERE timestamp < ?1")
        .bind(&[number(cutoff)])?;
    // 压缩和保留期删除先原子提交；轮换随后在另一个原子 batch 中完成。
    // 两个阶段都可独立重试，且轮换失败时不会留下缺少当前表的半完成状态。
    db.batch(vec![compact, delete_archive]).await?;
    rotate_recent_history(db).await
}

/// 丢弃上一代分钟历史，把当前代降级为上一代，再建一张空的当前代。
async fn rotate_recent_history(db: &D1Database) -> Result<()> {
    // D1 batch guarantees transactional execution, including SQLite DDL. If any
    // statement fails, the prior pair of recent-history tables remains intact.
    db.batch(vec![
        db.prepare("DROP TABLE IF EXISTS metric_history_old"),
        db.prepare("ALTER TABLE metric_history RENAME TO metric_history_old"),
        db.prepare(metric_history_ddl("metric_history")),
    ])
    .await?;
    Ok(())
}

/// 保留期下界，对齐到整小时。
fn history_cutoff(current: i64, retention_days: i64) -> i64 {
    let cutoff = current - retention_days.clamp(1, MAX_HISTORY_RETENTION_DAYS) * 86_400;
    cutoff.div_euclid(3600) * 3600
}

pub async fn clear_history(db: &D1Database) -> Result<()> {
    db.batch(vec![
        db.prepare("DELETE FROM metric_history"),
        db.prepare("DELETE FROM metric_history_old"),
        db.prepare("DELETE FROM metric_history_hourly"),
    ])
    .await?;
    Ok(())
}

pub async fn exchange_rate_snapshot(
    db: &D1Database,
    base_currency: &str,
) -> Result<Option<ExchangeRateSnapshot>> {
    db.prepare(
        "SELECT base_currency, rates_json, source, rate_date, fetched_at, attempted_at \
         FROM exchange_rate_snapshots WHERE base_currency = ?1",
    )
    .bind(&[text(base_currency)])?
    .first(None)
    .await
}

pub async fn mark_exchange_rate_attempt(
    db: &D1Database,
    base_currency: &str,
    attempted_at: i64,
) -> Result<()> {
    db.prepare(
        "INSERT INTO exchange_rate_snapshots( \
           base_currency, rates_json, source, rate_date, fetched_at, attempted_at \
         ) VALUES (?1, '{}', 'default', '', 0, ?2) \
         ON CONFLICT(base_currency) DO UPDATE SET attempted_at=excluded.attempted_at",
    )
    .bind(&[text(base_currency), number(attempted_at)])?
    .run()
    .await?;
    Ok(())
}

pub async fn upsert_exchange_rate_snapshot(
    db: &D1Database,
    base_currency: &str,
    rates_json: &str,
    source: &str,
    rate_date: &str,
    fetched_at: i64,
) -> Result<()> {
    db.prepare(
        "INSERT INTO exchange_rate_snapshots( \
           base_currency, rates_json, source, rate_date, fetched_at, attempted_at \
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?5) \
         ON CONFLICT(base_currency) DO UPDATE SET \
           rates_json=excluded.rates_json, source=excluded.source, \
           rate_date=excluded.rate_date, fetched_at=excluded.fetched_at, \
           attempted_at=excluded.attempted_at",
    )
    .bind(&[
        text(base_currency),
        text(rates_json),
        text(source),
        text(rate_date),
        number(fetched_at),
    ])?
    .run()
    .await?;
    Ok(())
}

pub async fn database_stats(db: &D1Database, offline_threshold: i64) -> Result<DatabaseStats> {
    let cutoff = now() - offline_threshold.clamp(30, 3600);
    db.prepare(
        r#"SELECT
          (SELECT COUNT(*) FROM servers) AS server_count,
          (SELECT COUNT(*) FROM servers s WHERE COALESCE(
             (SELECT MAX(latest_timestamp) FROM (
                SELECT server_id, latest_timestamp FROM metric_history UNION ALL
                SELECT server_id, latest_timestamp FROM metric_history_old
              ) h WHERE h.server_id = s.id),
             (SELECT latest_timestamp FROM metric_history_hourly h
              WHERE h.server_id = s.id ORDER BY timestamp DESC LIMIT 1),
             0
           ) >= ?1) AS online_count,
          ((SELECT COUNT(*) FROM metric_history) +
           (SELECT COUNT(*) FROM metric_history_old) +
           (SELECT COUNT(*) FROM metric_history_hourly)) AS history_rows,
          (SELECT MIN(timestamp) FROM (
             SELECT timestamp FROM metric_history UNION ALL
             SELECT timestamp FROM metric_history_old UNION ALL
             SELECT timestamp FROM metric_history_hourly
           )) AS oldest_history,
          (SELECT MAX(timestamp) FROM (
             SELECT timestamp FROM metric_history UNION ALL
             SELECT timestamp FROM metric_history_old UNION ALL
             SELECT timestamp FROM metric_history_hourly
           )) AS newest_history"#,
    )
    .bind(&[number(cutoff)])?
    .first(None)
    .await?
    .ok_or_else(|| worker::Error::RustError("无法读取数据库统计".to_string()))
}

pub async fn get_setting(db: &D1Database, key: &str) -> Result<Option<String>> {
    db.prepare("SELECT json_extract(value, ?1) AS value FROM settings WHERE id = 1")
        .bind(&[text(&format!("$.{key}"))])?
        .first::<String>(Some("value"))
        .await
}

pub async fn save_setting(db: &D1Database, key: &str, value: &str) -> Result<()> {
    db.prepare("UPDATE settings SET value=json_set(value, ?1, ?2), updated_at=?3 WHERE id=1")
        .bind(&[text(&format!("$.{key}")), text(value), number(now())])?
        .run()
        .await?;
    Ok(())
}

pub async fn increment_setting(db: &D1Database, key: &str) -> Result<()> {
    db.prepare(
        "UPDATE settings SET value=json_set( \
           value, ?1, CAST(CAST(COALESCE(json_extract(value, ?1), '0') AS INTEGER) + 1 AS TEXT) \
         ), updated_at=?2 WHERE id=1",
    )
    .bind(&[text(&format!("$.{key}")), number(now())])?
    .run()
    .await?;
    Ok(())
}

pub async fn update_expiry(db: &D1Database, id: &str, expires_at: i64) -> Result<()> {
    db.prepare("UPDATE servers SET expires_at = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(&[text(id), number(expires_at), number(now())])?
        .run()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        alert_window_covered, history_cutoff, metric_history_ddl, secret_for_api,
        traffic_cycle_key, AlertMetricRow, HistoryMetricAggregate, MAX_HISTORY_RETENTION_DAYS,
        SECRET_MASK,
    };
    use crate::models::AgentReport;

    fn row(samples: i64, last_timestamp: i64, report_interval: i64) -> AlertMetricRow {
        AlertMetricRow {
            server_id: "server".to_string(),
            name: "Server".to_string(),
            value: 90.0,
            sample_count: samples,
            first_timestamp: last_timestamp - samples.saturating_sub(1) * 60,
            last_timestamp,
            report_interval,
        }
    }

    #[test]
    fn alert_coverage_tracks_report_interval_and_freshness() {
        let current = 10_000;
        // 落库间隔 60s 时分钟样本每分钟一行，10 分钟窗口期望 10 个样本，
        // 覆盖线是 (10*3+4)/5 = 6 个；report_interval 300 高于下限，不受影响。
        assert!(alert_window_covered(&row(6, current - 30, 60), 10, current));
        assert!(!alert_window_covered(
            &row(5, current - 30, 60),
            10,
            current
        ));
        assert!(alert_window_covered(
            &row(2, current - 60, 300),
            10,
            current
        ));
        assert!(!alert_window_covered(
            &row(2, current - 700, 300),
            10,
            current
        ));
    }

    #[test]
    fn aligns_history_cutoff_and_caps_retention() {
        let current = 40 * 86_400 + 12 * 3600 + 34 * 60 + 56;
        let cutoff = history_cutoff(current, 365);
        assert_eq!(cutoff % 3600, 0);
        assert_eq!(
            cutoff,
            (40 - MAX_HISTORY_RETENTION_DAYS) * 86_400 + 12 * 3600
        );
    }

    /// 轮换要在运行时重建 `metric_history`，DDL 因此在 Rust 里存了一份。
    /// 这个测试比对它和 migration 的文本，防止两边漂移。三张 metric_* 表必须同构：
    /// 轮换靠 RENAME 把当前代变成上一代，压缩靠 UNION ALL 同时读它们。
    #[test]
    fn metric_history_ddl_matches_migration() {
        let schema = include_str!("../migrations/initial_01.sql");

        let table_body = |sql: &str, table: &str| -> String {
            let head = format!("CREATE TABLE {table} (");
            let start = sql.find(&head).unwrap_or_else(|| {
                panic!("migration 里找不到 {table}");
            }) + head.len();
            let rest = &sql[start..];
            let end = rest.find(") WITHOUT ROWID").expect("缺少 WITHOUT ROWID");
            rest[..end].split_whitespace().collect::<Vec<_>>().join(" ")
        };

        let generated = table_body(&metric_history_ddl("metric_history"), "metric_history");
        for table in [
            "metric_history",
            "metric_history_old",
            "metric_history_hourly",
        ] {
            assert_eq!(
                generated,
                table_body(schema, table),
                "migration 的 {table} 与 db::metric_history_ddl 不一致"
            );
        }
    }

    #[test]
    fn masks_stored_secrets_in_api_responses() {
        assert_eq!(secret_for_api(""), "");
        assert_eq!(secret_for_api("stored-secret"), SECRET_MASK);
    }

    #[test]
    fn assigns_monthly_traffic_periods_with_short_months() {
        let timestamp = |year: i64, month: i64, day: i64| {
            let year = year - i64::from(month <= 2);
            let era = year.div_euclid(400);
            let year_of_era = year - era * 400;
            let month_prime = month + if month > 2 { -3 } else { 9 };
            let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
            let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
            (era * 146_097 + day_of_era - 719_468) * 86_400
        };
        assert_eq!(traffic_cycle_key(timestamp(2026, 8, 9), 10), 2026 * 12 + 6);
        assert_eq!(traffic_cycle_key(timestamp(2026, 8, 10), 10), 2026 * 12 + 7);
        assert_eq!(traffic_cycle_key(timestamp(2028, 2, 28), 31), 2028 * 12);
        assert_eq!(traffic_cycle_key(timestamp(2028, 2, 29), 31), 2028 * 12 + 1);
    }

    #[test]
    fn aggregates_history_with_averages_and_peaks() {
        let reports = [
            AgentReport {
                timestamp: 121,
                cpu: 10.0,
                mem_used: 100,
                swap_used: 20,
                net_in: 50.0,
                net_out: 80.0,
                processes: 4,
                tcp_connections: 7,
                udp_connections: 2,
                disk_read_bps: 30.0,
                disk_write_bps: 90.0,
                disk_read_iops: 3.0,
                disk_write_iops: 9.0,
                disk_await_ms: 2.0,
                disk_utilization: 20.0,
                ..AgentReport::default()
            },
            AgentReport {
                timestamp: 124,
                cpu: 30.0,
                mem_used: 300,
                mem_total: 1_000,
                swap_used: 60,
                swap_total: 500,
                net_in: 200.0,
                net_out: 40.0,
                net_rx_total: 2_000,
                net_tx_total: 3_000,
                processes: 2,
                tcp_connections: 5,
                udp_connections: 8,
                disk_read_bps: 100.0,
                disk_write_bps: 20.0,
                disk_read_iops: 10.0,
                disk_write_iops: 2.0,
                disk_await_ms: 5.0,
                disk_utilization: 10.0,
                ..AgentReport::default()
            },
        ];
        let mut aggregate = HistoryMetricAggregate::default();
        aggregate.extend(&reports[..1]);
        aggregate.extend(&reports[1..]);
        let point = aggregate.point().unwrap();

        assert_eq!(aggregate.sample_count(), 2);
        assert_eq!(point.timestamp, 120);
        assert_eq!(point.cpu, 20.0);
        assert_eq!(point.mem_used, 200);
        assert_eq!(point.swap_used, 40);
        assert_eq!(point.net_in, 200.0);
        assert_eq!(point.net_out, 80.0);
        assert_eq!(point.processes, 4);
        assert_eq!(point.tcp_connections, 7);
        assert_eq!(point.udp_connections, 8);
        assert_eq!(point.disk_read_bps, 100.0);
        assert_eq!(point.disk_write_bps, 90.0);
        assert_eq!(point.disk_read_iops, 10.0);
        assert_eq!(point.disk_write_iops, 9.0);
        assert_eq!(point.disk_await_ms, 5.0);
        assert_eq!(point.disk_utilization, 20.0);
        assert_eq!(point.net_rx_total, 2_000);
        assert_eq!(point.net_tx_total, 3_000);
    }
}
