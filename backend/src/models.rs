use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub username: String,
    pub password_derived: String,
    #[serde(default)]
    pub turnstile_token: String,
    #[serde(default)]
    pub totp_code: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnstileVerifyRequest {
    pub token: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerInput {
    pub name: String,
    pub region: String,
    pub group_name: String,
    pub tags: String,
    pub hidden: bool,
    pub expires_at: Option<i64>,
    pub traffic_limit: i64,
    pub traffic_limit_type: String,
    pub price: f64,
    pub billing_cycle: i64,
    pub currency: String,
    pub auto_renewal: bool,
    pub network_interface: String,
    pub reset_day: i64,
    pub report_interval: i64,
    pub collect_interval: i64,
    pub rx_correction: i64,
    pub tx_correction: i64,
    pub agent_mirror: String,
    pub offline_notify_disabled: bool,
    pub auto_update: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsInput {
    pub site_name: Option<String>,
    pub site_description: Option<String>,
    pub site_announcement: Option<String>,
    pub logo_url: Option<String>,
    pub locale: Option<String>,
    pub public_dashboard: Option<bool>,
    pub offline_threshold_seconds: Option<i64>,
    pub history_retention_days: Option<i64>,
    pub default_theme: Option<String>,
    pub active_theme_id: Option<String>,
    pub background_url: Option<String>,
    pub theme_options: Option<serde_json::Value>,
    pub show_search: Option<bool>,
    pub show_groups: Option<bool>,
    pub show_stats: Option<bool>,
    pub show_assets: Option<bool>,
    pub show_traffic: Option<bool>,
    pub show_speed: Option<bool>,
    pub show_price: Option<bool>,
    pub show_expiry: Option<bool>,
    pub show_latency: Option<bool>,
    pub show_uptime: Option<bool>,
    pub admin_username: Option<String>,
    pub new_password_derived: Option<String>,
    pub turnstile_enabled: Option<bool>,
    pub turnstile_login_enabled: Option<bool>,
    pub turnstile_site_key: Option<String>,
    pub turnstile_secret_key: Option<String>,
    pub notification_enabled: Option<bool>,
    pub offline_alert_minutes: Option<i64>,
    pub expiry_alert_days: Option<i64>,
    pub traffic_alert_percentage: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicConfig {
    pub site_name: String,
    pub site_description: String,
    pub site_announcement: String,
    pub logo_url: String,
    pub locale: String,
    pub public_dashboard: bool,
    pub offline_threshold_seconds: i64,
    pub history_retention_days: i64,
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
    pub turnstile_enabled: bool,
    pub turnstile_login_enabled: bool,
    pub totp_login_enabled: bool,
    pub turnstile_site_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub password_client_salt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    #[serde(flatten)]
    pub public: PublicConfig,
    pub admin_username: String,
    pub admin_password_configured: bool,
    pub turnstile_secret_key: String,
    pub notification_enabled: bool,
    pub offline_alert_minutes: i64,
    pub expiry_alert_days: i64,
    pub traffic_alert_percentage: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginSessionView {
    pub id: String,
    pub ip_address: String,
    pub user_agent: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub expires_at: i64,
    pub current: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentReport {
    pub timestamp: i64,
    pub cpu: f64,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub mem_used: i64,
    pub mem_total: i64,
    pub swap_used: i64,
    pub swap_total: i64,
    pub disk_used: i64,
    pub disk_total: i64,
    pub net_in: f64,
    pub net_out: f64,
    pub net_rx_total: i64,
    pub net_tx_total: i64,
    pub uptime: i64,
    pub processes: i64,
    pub tcp_connections: i64,
    pub udp_connections: i64,
    pub cpu_cores: i64,
    pub cpu_model: String,
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub virtualization: String,
    pub gpu_usage: f64,
    pub gpu_model: String,
    pub agent_version: String,
    pub ip_v4: String,
    pub ip_v6: String,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub disk_read_iops: f64,
    pub disk_write_iops: f64,
    pub disk_await_ms: f64,
    pub disk_utilization: f64,
    pub disks: Vec<AgentDiskMetric>,
    pub gpus: Vec<AgentGpuMetric>,
    pub latency_results: Vec<AgentLatencyResult>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentDiskMetric {
    pub name: String,
    pub mount_point: String,
    pub used: i64,
    pub total: i64,
    pub read_bps: f64,
    pub write_bps: f64,
    pub read_iops: f64,
    pub write_iops: f64,
    pub await_ms: f64,
    pub utilization: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentGpuMetric {
    pub model: String,
    pub usage: Option<f64>,
    pub memory_used: i64,
    pub memory_total: i64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentLatencyResult {
    pub task_id: String,
    pub timestamp: i64,
    pub latency_ms: f64,
    pub packet_loss: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerView {
    pub id: String,
    pub name: String,
    pub region: String,
    pub group_name: String,
    pub tags: String,
    pub hidden: bool,
    pub expires_at: Option<i64>,
    pub traffic_limit: i64,
    pub traffic_limit_type: String,
    pub price: f64,
    pub billing_cycle: i64,
    pub currency: String,
    pub auto_renewal: bool,
    pub last_ip: String,
    pub ip_v4: String,
    pub ip_v6: String,
    pub network_interface: String,
    pub reset_day: i64,
    pub report_interval: i64,
    pub collect_interval: i64,
    pub rx_correction: i64,
    pub tx_correction: i64,
    pub agent_mirror: String,
    pub offline_notify_disabled: bool,
    pub auto_update: bool,
    pub timestamp: Option<i64>,
    pub cpu: Option<f64>,
    pub load1: Option<f64>,
    pub load5: Option<f64>,
    pub load15: Option<f64>,
    pub mem_used: Option<i64>,
    pub mem_total: Option<i64>,
    pub swap_used: Option<i64>,
    pub swap_total: Option<i64>,
    pub disk_used: Option<i64>,
    pub disk_total: Option<i64>,
    pub net_in: Option<f64>,
    pub net_out: Option<f64>,
    pub net_rx_total: Option<i64>,
    pub net_tx_total: Option<i64>,
    pub uptime: Option<i64>,
    pub processes: Option<i64>,
    pub tcp_connections: Option<i64>,
    pub udp_connections: Option<i64>,
    pub cpu_cores: Option<i64>,
    pub cpu_model: Option<String>,
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub arch: Option<String>,
    pub virtualization: Option<String>,
    pub gpu_usage: Option<f64>,
    pub gpu_model: Option<String>,
    pub agent_version: Option<String>,
    pub disk_read_bps: Option<f64>,
    pub disk_write_bps: Option<f64>,
    pub disk_read_iops: Option<f64>,
    pub disk_write_iops: Option<f64>,
    pub disk_await_ms: Option<f64>,
    pub disk_utilization: Option<f64>,
    pub disks: Vec<AgentDiskMetric>,
    pub gpus: Vec<AgentGpuMetric>,
    pub latency: Vec<LatencySample>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct HistoryPoint {
    pub timestamp: i64,
    pub cpu: f64,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub mem_used: i64,
    pub mem_total: i64,
    pub swap_used: i64,
    pub swap_total: i64,
    pub disk_used: i64,
    pub disk_total: i64,
    pub net_in: f64,
    pub net_out: f64,
    pub net_rx_total: i64,
    pub net_tx_total: i64,
    pub processes: i64,
    pub tcp_connections: i64,
    pub udp_connections: i64,
    pub gpu_usage: f64,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub disk_read_iops: f64,
    pub disk_write_iops: f64,
    pub disk_await_ms: f64,
    pub disk_utilization: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LatencyTaskInput {
    pub name: String,
    pub task_type: String,
    pub target: String,
    pub port: Option<i64>,
    pub interval_seconds: i64,
    pub default_enabled: bool,
    pub server_ids: Vec<String>,
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

#[derive(Debug, Clone, Serialize)]
pub struct AgentLatencyTask {
    pub id: String,
    pub name: String,
    pub task_type: String,
    pub target: String,
    pub port: Option<i64>,
    pub interval_seconds: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AlertRuleInput {
    pub name: String,
    pub metric: String,
    pub threshold: f64,
    pub duration_minutes: i64,
    pub aggregation: String,
    pub enabled: bool,
    pub server_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AlertRuleView {
    pub id: String,
    pub name: String,
    pub metric: String,
    pub threshold: f64,
    pub duration_minutes: i64,
    pub aggregation: String,
    pub enabled: bool,
    pub server_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramSettingsInput {
    pub bot_token: String,
    pub chat_id: String,
    #[serde(default)]
    pub message_thread_id: Option<i64>,
    pub template: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TelegramSettingsView {
    pub bot_token: String,
    pub chat_id: String,
    pub message_thread_id: Option<i64>,
    pub template: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeInput {
    pub name: String,
    pub description: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeUploadInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub filename: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThemeView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub url: String,
    pub version: String,
    pub builtin: bool,
    pub active: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerOrderInput {
    pub ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerBatchInput {
    pub ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WakeServersInput {
    pub server_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enable2FaRequest {
    pub totp_code: String,
}

#[derive(Debug, Serialize)]
pub struct TotpSetupResponse {
    pub secret: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct TotpStatusResponse {
    pub enabled: bool,
    pub has_secret: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRemoteTaskRequest {
    pub server_ids: Vec<String>,
    pub command: String,
    pub totp_code: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteTaskInfo {
    pub id: String,
    pub server_id: String,
    pub command: String,
    pub status: String,
    pub requested_by: String,
    pub requested_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub result: String,
    pub exit_code: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatabaseStats {
    pub server_count: i64,
    pub online_count: i64,
    pub history_rows: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExchangeRatesView {
    pub base: String,
    pub rates: BTreeMap<String, f64>,
    pub source: String,
    pub date: String,
    pub fetched_at: i64,
    pub stale: bool,
}

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
}

impl ApiError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
        }
    }
}
