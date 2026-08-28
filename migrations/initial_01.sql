PRAGMA foreign_keys = ON;

CREATE TABLE settings (
  id INTEGER PRIMARY KEY CHECK(id = 1),
  value TEXT NOT NULL CHECK(json_valid(value)),
  updated_at INTEGER NOT NULL
);

CREATE TABLE servers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  region TEXT NOT NULL DEFAULT '',
  group_name TEXT NOT NULL DEFAULT '默认',
  tags TEXT NOT NULL DEFAULT '',
  hidden INTEGER NOT NULL DEFAULT 0,
  sort_order INTEGER NOT NULL DEFAULT 0,
  expires_at INTEGER,
  traffic_limit INTEGER NOT NULL DEFAULT 0,
  traffic_limit_type TEXT NOT NULL DEFAULT 'sum',
  price REAL NOT NULL DEFAULT 0,
  billing_cycle INTEGER NOT NULL DEFAULT 30,
  currency TEXT NOT NULL DEFAULT 'CNY',
  auto_renewal INTEGER NOT NULL DEFAULT 0,
  last_ip TEXT NOT NULL DEFAULT '',
  network_interface TEXT NOT NULL DEFAULT '',
  reset_day INTEGER NOT NULL DEFAULT 1,
  report_interval INTEGER NOT NULL DEFAULT 60,
  collect_interval INTEGER NOT NULL DEFAULT 1,
  rx_correction INTEGER NOT NULL DEFAULT 0,
  tx_correction INTEGER NOT NULL DEFAULT 0,
  agent_mirror TEXT NOT NULL DEFAULT '',
  offline_notify_disabled INTEGER NOT NULL DEFAULT 0,
  auto_update INTEGER NOT NULL DEFAULT 1,
  token TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX idx_servers_sort
  ON servers(sort_order, created_at);

-- 分钟粒度历史。下面三张 metric_* 表结构完全一致，统一由
-- db::metric_history_ddl 生成，src/db.rs 的 metric_history_ddl_matches_migration
-- 负责防止漂移——轮换要在运行时重建 metric_history，DDL 因此在 Rust 里也存了一份。
CREATE TABLE metric_history (
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
) WITHOUT ROWID;

-- 分钟粒度历史的上一代。保留期靠表轮换而不是 DELETE：DELETE 每删一行计一行
-- D1 写入，DROP TABLE 是页级回收、不计行写入。轮换后当前表是空的，所以近期
-- 读取（<=24h）必须同时读这两张表，两代合起来覆盖 24-48 小时。这里预建空表，
-- 保证 union 读取在首次轮换之前就不会撞上缺表。
CREATE TABLE metric_history_old (
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
) WITHOUT ROWID;

-- 小时粒度历史。分钟表轮换掉之后的长期留存，按 history_retention_days 删除。
CREATE TABLE metric_history_hourly (
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
) WITHOUT ROWID;

-- 每台机器最新一次上报的落点。和 metric_history 分开，避免轮换影响实时读取。
CREATE TABLE server_latest_state (
  server_id TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
  latest_timestamp INTEGER NOT NULL,
  latest_json TEXT NOT NULL CHECK(json_valid(latest_json)),
  last_batch_id TEXT NOT NULL DEFAULT ''
) WITHOUT ROWID;

CREATE TABLE exchange_rate_snapshots (
  base_currency TEXT PRIMARY KEY,
  rates_json TEXT NOT NULL,
  source TEXT NOT NULL,
  rate_date TEXT NOT NULL,
  fetched_at INTEGER NOT NULL,
  attempted_at INTEGER NOT NULL
);

CREATE TABLE themes (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  url TEXT NOT NULL UNIQUE,
  version TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);

CREATE INDEX idx_themes_created
  ON themes(created_at DESC);

CREATE TABLE latency_tasks (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  task_type TEXT NOT NULL CHECK(task_type IN ('tcp', 'icmp')),
  target TEXT NOT NULL,
  port INTEGER CHECK(port IS NULL OR port BETWEEN 1 AND 65535),
  interval_seconds INTEGER NOT NULL CHECK(interval_seconds BETWEEN 30 AND 3600),
  default_enabled INTEGER NOT NULL DEFAULT 0,
  sort_order INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK((task_type = 'tcp' AND port IS NOT NULL) OR (task_type = 'icmp' AND port IS NULL))
);

CREATE TABLE latency_task_servers (
  task_id TEXT NOT NULL REFERENCES latency_tasks(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  assigned_at INTEGER NOT NULL,
  PRIMARY KEY(task_id, server_id)
) WITHOUT ROWID;

CREATE INDEX idx_latency_task_servers_server
  ON latency_task_servers(server_id, task_id);

CREATE TABLE alert_rules (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  metric TEXT NOT NULL CHECK(metric IN ('cpu', 'memory', 'disk', 'net_in', 'net_out')),
  threshold REAL NOT NULL CHECK(threshold > 0),
  duration_minutes INTEGER NOT NULL CHECK(duration_minutes BETWEEN 1 AND 1440),
  aggregation TEXT NOT NULL CHECK(aggregation IN ('average', 'continuous')),
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE alert_rule_servers (
  rule_id TEXT NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  PRIMARY KEY(rule_id, server_id)
) WITHOUT ROWID;

-- 通知只走 Telegram，单行配置表。
CREATE TABLE notification_telegram (
  id INTEGER PRIMARY KEY CHECK(id = 1),
  bot_token TEXT NOT NULL,
  chat_id TEXT NOT NULL,
  message_thread_id INTEGER CHECK(message_thread_id IS NULL OR message_thread_id > 0),
  template TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE notification_events (
  id TEXT PRIMARY KEY,
  dedupe_key TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK(kind IN (
    'offline', 'online', 'expiry', 'resource_alert', 'resource_recovery', 'traffic', 'test'
  )),
  server_id TEXT REFERENCES servers(id) ON DELETE SET NULL,
  server_name TEXT NOT NULL DEFAULT '',
  title TEXT NOT NULL,
  message TEXT NOT NULL,
  details_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(details_json)),
  occurred_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE notification_deliveries (
  event_id TEXT PRIMARY KEY REFERENCES notification_events(id) ON DELETE CASCADE,
  status TEXT NOT NULL CHECK(status IN ('pending', 'sent', 'failed', 'dead')),
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER NOT NULL,
  last_error TEXT,
  sent_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX idx_notification_deliveries_pending
  ON notification_deliveries(status, next_attempt_at, created_at);

CREATE TABLE notification_state (
  id INTEGER PRIMARY KEY CHECK(id = 1),
  value TEXT NOT NULL CHECK(json_valid(value)),
  updated_at INTEGER NOT NULL
);

INSERT INTO notification_state(id, value, updated_at)
VALUES (1, '{"offline":[],"expiry":[],"resources":[],"traffic":{}}', unixepoch());

INSERT INTO settings(id, value, updated_at) VALUES (
  1,
  json_patch(
    '{
      "site_description": "轻量、实时的服务器运行状态",
      "site_announcement": "",
      "logo_url": "",
      "locale": "zh-CN",
      "public_dashboard": "true",
      "history_cache_version": "0",
      "default_theme": "system",
      "active_theme_id": "builtin-nodeflare-glass",
      "background_url": "",
      "theme_options": "{}",
      "show_search": "true",
      "show_groups": "true",
      "show_stats": "true",
      "show_assets": "true",
      "show_traffic": "true",
      "show_speed": "true",
      "show_price": "true",
      "show_expiry": "true",
      "show_latency": "true",
      "show_uptime": "true",
      "admin_username": "",
      "admin_password_hash": "",
      "turnstile_enabled": "false",
      "turnstile_login_enabled": "true",
      "turnstile_site_key": "",
      "turnstile_secret_key": "",
      "notification_enabled": "false",
      "offline_alert_minutes": "5",
      "expiry_alert_days": "7",
      "traffic_alert_percentage": "80",
      "cloudflare_account_id": "",
      "cloudflare_api_token": ""
    }',
    json_object('password_client_salt', lower(hex(randomblob(16))))
  ),
  unixepoch()
);

INSERT INTO exchange_rate_snapshots (
  base_currency, rates_json, source, rate_date, fetched_at, attempted_at
) VALUES (
  'CNY',
  '{"CNY":1,"USD":0.14799,"CAD":0.2086,"HKD":1.1594,"EUR":0.1275,"GBP":0.11027,"JPY":23.707,"RUB":11.560694,"CHF":0.120661,"INR":14.248668,"VND":3875.968992,"THB":4.97107}',
  'default', '', 0, 0
);

PRAGMA optimize;
