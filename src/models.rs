use serde::{Deserialize, Serialize, Serializer};

fn serialize_sqlite_bool<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_bool(*value != 0)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub password_derived: String,
    pub turnstile_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnstileVerifyRequest {
    pub token: String,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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
    pub new_password: Option<String>,
    pub new_password_derived: Option<String>,
    pub turnstile_enabled: Option<bool>,
    pub turnstile_login_enabled: Option<bool>,
    pub turnstile_site_key: Option<String>,
    pub turnstile_secret_key: Option<String>,
    pub notification_enabled: Option<bool>,
    pub offline_alert_minutes: Option<i64>,
    pub expiry_alert_days: Option<i64>,
    pub traffic_alert_percentage: Option<i64>,
    pub cloudflare_account_id: Option<String>,
    pub cloudflare_api_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramNotificationInput {
    pub bot_token: String,
    pub chat_id: String,
    #[serde(default)]
    pub message_thread_id: Option<i64>,
    pub template: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeInput {
    pub name: String,
    pub description: String,
    pub url: String,
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
pub struct LatencyTaskInput {
    pub name: String,
    pub task_type: String,
    pub target: String,
    pub port: Option<i64>,
    pub interval_seconds: i64,
    pub default_enabled: bool,
    pub server_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentLatencyTask {
    pub id: String,
    pub name: String,
    pub task_type: String,
    pub target: String,
    pub port: Option<i64>,
    pub interval_seconds: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLatencyResult {
    pub task_id: String,
    pub timestamp: i64,
    pub latency_ms: f64,
    pub packet_loss: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentGpuMetric {
    pub model: String,
    pub usage: Option<f64>,
    pub memory_used: i64,
    pub memory_total: i64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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
    /// 双栈公网地址，agent 双栈探测的结果；空串表示该协议族不可用或未上报。
    #[serde(default)]
    pub ip_v4: String,
    #[serde(default)]
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

#[derive(Debug, Deserialize, Serialize)]
pub struct ServerView {
    pub id: String,
    pub name: String,
    pub region: String,
    pub group_name: String,
    pub tags: String,
    #[serde(serialize_with = "serialize_sqlite_bool")]
    pub hidden: i64,
    pub expires_at: Option<i64>,
    pub traffic_limit: i64,
    pub traffic_limit_type: String,
    pub price: f64,
    pub billing_cycle: i64,
    pub currency: String,
    #[serde(serialize_with = "serialize_sqlite_bool")]
    pub auto_renewal: i64,
    pub last_ip: String,
    pub ip_v4: Option<String>,
    pub ip_v6: Option<String>,
    pub network_interface: String,
    pub reset_day: i64,
    pub report_interval: i64,
    pub collect_interval: i64,
    pub rx_correction: i64,
    pub tx_correction: i64,
    pub agent_mirror: String,
    #[serde(serialize_with = "serialize_sqlite_bool")]
    pub offline_notify_disabled: i64,
    #[serde(serialize_with = "serialize_sqlite_bool")]
    pub auto_update: i64,
    #[serde(skip_serializing)]
    pub created_at: i64,
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
    #[serde(skip_serializing)]
    pub disk_info: Option<String>,
    #[serde(skip_serializing)]
    pub gpu_info: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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

#[derive(Debug, Deserialize)]
pub struct AgentIdentityRow {
    pub id: String,
    pub hidden: i64,
}

#[derive(Debug, Serialize)]
pub struct AgentConfigView {
    pub report_interval: i64,
    pub collect_interval: i64,
    pub network_interface: String,
    pub agent_mirror: String,
    pub auto_update: bool,
    pub latency_tasks: Vec<AgentLatencyTask>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Serialize)]
pub struct ApiError<'a> {
    pub error: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<&'a str>,
}
