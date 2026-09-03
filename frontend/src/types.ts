export interface Config {
  site_name: string;
  site_description: string;
  site_announcement: string;
  /** 站点 Logo，同时用作浏览器标签图标。空则回落到 /logo.svg。 */
  logo_url: string;
  locale: "zh-CN" | "en";
  public_dashboard: boolean;
  offline_threshold_seconds: number;
  history_retention_days: number;
  default_theme: "system" | "light" | "dark";
  active_theme_id: string;
  background_url: string;
  theme_options: Record<string, ThemeSettingValue>;
  show_search: boolean;
  show_groups: boolean;
  show_stats: boolean;
  show_assets: boolean;
  show_traffic: boolean;
  show_speed: boolean;
  show_price: boolean;
  show_expiry: boolean;
  show_latency: boolean;
  show_uptime: boolean;
  turnstile_enabled: boolean;
  turnstile_login_enabled: boolean;
  totp_login_enabled: boolean;
  turnstile_site_key: string;
  password_client_salt: string;
}

export type ThemeSettingValue = string | number | boolean;

export const ASSET_CURRENCIES = [
  "CNY", "USD", "HKD", "EUR", "GBP", "JPY", "RUB", "CHF", "INR", "VND", "THB", "CAD",
] as const;
export type AssetCurrency = typeof ASSET_CURRENCIES[number];

interface ThemeSettingChoice {
  label: string;
  value: string;
}

export interface ThemeSettingField {
  key: string;
  label: string;
  type: "text" | "textarea" | "url" | "color" | "select" | "toggle" | "number";
  default?: ThemeSettingValue;
  placeholder?: string;
  options?: ThemeSettingChoice[];
  min?: number;
  max?: number;
  step?: number;
}

export interface ThemeSettingsSchema {
  schema: number;
  source: "builtin" | "remote";
  settings: ThemeSettingField[];
}

export interface Theme {
  id: string;
  name: string;
  description: string;
  url: string;
  version: string;
  builtin: boolean;
  active: boolean;
}

export interface ExchangeRates {
  base: "CNY";
  rates: Record<string, number>;
  source: string;
  date: string;
  fetched_at: number;
  stale: boolean;
}

export interface Bootstrap {
  config: Config;
  access: "ok" | "login" | "turnstile";
  servers: Server[];
  exchange_rates: ExchangeRates | null;
}

interface DiskMetric {
  name: string;
  mount_point: string;
  used: number;
  total: number;
  read_bps: number;
  write_bps: number;
  read_iops: number;
  write_iops: number;
  await_ms: number;
  utilization: number;
}

interface GpuMetric {
  model: string;
  usage: number | null;
  memory_used: number;
  memory_total: number;
}

export type TrafficLimitType = "sum" | "max" | "min" | "up" | "down";

export interface LatencySample {
  task_id: string;
  server_id: string;
  name: string;
  task_type: "tcp" | "icmp";
  target: string;
  port: number | null;
  timestamp: number;
  latency_ms: number;
  packet_loss: number;
}

export interface LiveLatencyResult {
  task_id: string;
  timestamp: number;
  latency_ms: number;
  packet_loss: number;
}

export interface LatencyTestPoint {
  id: string;
  name: string;
  task_type: "tcp" | "icmp";
  target: string;
  port: number | null;
  interval_seconds: number;
}

export interface LatencyTask {
  id: string;
  name: string;
  task_type: "tcp" | "icmp";
  target: string;
  port: number | null;
  interval_seconds: number;
  default_enabled: boolean;
  server_ids: string[];
}

export type LatencyTaskInput = Pick<LatencyTask, "name" | "task_type" | "target" | "port" | "interval_seconds" | "default_enabled" | "server_ids">;

interface ServerSummary {
  id: string;
  name: string;
  region: string;
  group_name: string;
  tags: string;
  expires_at: number | null;
  traffic_limit: number;
  traffic_limit_type: TrafficLimitType;
  price: number;
  billing_cycle: number;
  currency: string;
  auto_renewal: boolean;
  reset_day: number;
  timestamp: number | null;
  cpu: number | null;
  load1: number | null;
  load5: number | null;
  load15: number | null;
  mem_used: number | null;
  mem_total: number | null;
  swap_used: number | null;
  swap_total: number | null;
  disk_used: number | null;
  disk_total: number | null;
  net_in: number | null;
  net_out: number | null;
  net_rx_total: number | null;
  net_tx_total: number | null;
  uptime: number | null;
  processes: number | null;
  tcp_connections: number | null;
  udp_connections: number | null;
  cpu_cores: number | null;
  cpu_model: string | null;
  os: string | null;
  kernel: string | null;
  arch: string | null;
  virtualization: string | null;
  gpu_usage: number | null;
  gpu_model: string | null;
  agent_version: string | null;
  disk_read_bps: number | null;
  disk_write_bps: number | null;
  disk_read_iops: number | null;
  disk_write_iops: number | null;
  disk_await_ms: number | null;
  disk_utilization: number | null;
}

export interface Server extends ServerSummary {
  disks: DiskMetric[];
  gpus: GpuMetric[];
  latency: LatencySample[];
}

export interface AdminServer extends ServerSummary {
  hidden: boolean;
  last_ip: string;
  ip_v4: string;
  ip_v6: string;
  network_interface: string;
  report_interval: number;
  collect_interval: number;
  rx_correction: number;
  tx_correction: number;
  agent_mirror: string;
  offline_notify_disabled: boolean;
  auto_update: boolean;
}

export interface HistoryPoint {
  timestamp: number;
  cpu: number;
  load1: number;
  load5: number;
  load15: number;
  mem_used: number;
  mem_total: number;
  swap_used: number;
  swap_total: number;
  disk_used: number;
  disk_total: number;
  net_in: number;
  net_out: number;
  net_rx_total: number;
  net_tx_total: number;
  processes: number;
  tcp_connections: number;
  udp_connections: number;
  gpu_usage: number;
  disk_read_bps: number;
  disk_write_bps: number;
  disk_read_iops: number;
  disk_write_iops: number;
  disk_await_ms: number;
  disk_utilization: number;
}

export interface ServerInput {
  name: string;
  region: string;
  group_name: string;
  tags: string;
  hidden: boolean;
  expires_at: number | null;
  traffic_limit: number;
  traffic_limit_type: TrafficLimitType;
  price: number;
  billing_cycle: number;
  currency: string;
  auto_renewal: boolean;
  network_interface: string;
  reset_day: number;
  report_interval: number;
  collect_interval: number;
  rx_correction: number;
  tx_correction: number;
  agent_mirror: string;
  offline_notify_disabled: boolean;
  auto_update: boolean;
}

export interface Settings extends Omit<Config, "password_client_salt"> {
  admin_username: string;
  admin_password_configured: boolean;
  new_password?: string;
  new_password_derived?: string;
  turnstile_secret_key: string;
  notification_enabled: boolean;
  offline_alert_minutes: number;
  expiry_alert_days: number;
  traffic_alert_percentage: number;
}

export interface TelegramSettings {
  bot_token: string;
  chat_id: string;
  message_thread_id: number | null;
  template: string;
}

export type TelegramSettingsInput = Pick<TelegramSettings, "bot_token" | "chat_id" | "message_thread_id" | "template">;

type AlertMetric = "cpu" | "memory" | "disk" | "net_in" | "net_out";
type AlertAggregation = "average" | "continuous";

export interface AlertRule {
  id: string;
  name: string;
  metric: AlertMetric;
  threshold: number;
  duration_minutes: number;
  aggregation: AlertAggregation;
  enabled: boolean;
  server_ids: string[];
}

export interface AlertRuleInput extends Omit<AlertRule, "id" | "enabled"> {
  enabled: boolean;
}

export interface DatabaseStats {
  server_count: number;
  online_count: number;
  history_rows: number;
}

export interface TotpStatus {
  enabled: boolean;
  has_secret: boolean;
}

export interface TotpSetup {
  secret: string;
  uri: string;
  enabled: boolean;
}

export interface RemoteTask {
  id: string;
  server_id: string;
  command: string;
  script: string;
  status: "pending" | "sent" | "success" | "failed";
  requested_by: string;
  requested_at: number;
  started_at: number | null;
  completed_at: number | null;
  result: string;
  exit_code: number | null;
}

export interface RemoteTaskInput {
  server_ids: string[];
  command: string;
  totp_code: string;
}

export interface RemoteTaskCreated {
  server_id: string;
  task_id: string;
}
