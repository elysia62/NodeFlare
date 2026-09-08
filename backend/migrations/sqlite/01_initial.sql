PRAGMA foreign_keys = ON;

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE servers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL CHECK(length(trim(name)) BETWEEN 1 AND 80),
  region TEXT NOT NULL DEFAULT '',
  group_name TEXT NOT NULL DEFAULT '默认',
  tags TEXT NOT NULL DEFAULT '',
  hidden INTEGER NOT NULL DEFAULT 0 CHECK(hidden IN (0, 1)),
  sort_order INTEGER NOT NULL DEFAULT 0,
  expires_at INTEGER,
  traffic_limit INTEGER NOT NULL DEFAULT 0 CHECK(traffic_limit >= 0),
  traffic_limit_type TEXT NOT NULL DEFAULT 'sum'
    CHECK(traffic_limit_type IN ('sum', 'max', 'min', 'up', 'down')),
  price REAL NOT NULL DEFAULT 0 CHECK(price BETWEEN 0 AND 1000000000),
  billing_cycle INTEGER NOT NULL DEFAULT 30 CHECK(billing_cycle BETWEEN 0 AND 3650),
  currency TEXT NOT NULL DEFAULT 'CNY'
    CHECK(length(currency) = 3 AND currency NOT GLOB '*[^A-Z]*'),
  auto_renewal INTEGER NOT NULL DEFAULT 0 CHECK(auto_renewal IN (0, 1)),
  last_ip TEXT NOT NULL DEFAULT '',
  ip_v4 TEXT NOT NULL DEFAULT '',
  ip_v6 TEXT NOT NULL DEFAULT '',
  network_interface TEXT NOT NULL DEFAULT '',
  reset_day INTEGER NOT NULL DEFAULT 1 CHECK(reset_day BETWEEN 1 AND 31),
  report_interval INTEGER NOT NULL DEFAULT 60 CHECK(report_interval BETWEEN 15 AND 3600),
  collect_interval INTEGER NOT NULL DEFAULT 1 CHECK(collect_interval BETWEEN 1 AND 60),
  rx_correction INTEGER NOT NULL DEFAULT 0,
  tx_correction INTEGER NOT NULL DEFAULT 0,
  agent_mirror TEXT NOT NULL DEFAULT '',
  offline_notify_disabled INTEGER NOT NULL DEFAULT 0 CHECK(offline_notify_disabled IN (0, 1)),
  auto_update INTEGER NOT NULL DEFAULT 1 CHECK(auto_update IN (0, 1)),
  token_hash TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK(collect_interval <= report_interval),
  CHECK((report_interval + collect_interval - 1) / collect_interval <= 720)
) WITHOUT ROWID;

CREATE INDEX servers_sort ON servers(sort_order, created_at);
CREATE INDEX servers_public_sort ON servers(hidden, sort_order, created_at);

CREATE TABLE server_install_tokens (
  token_hash TEXT PRIMARY KEY,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX server_install_tokens_server ON server_install_tokens(server_id, created_at DESC);

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
  uptime INTEGER NOT NULL DEFAULT 0,
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
  first_timestamp INTEGER NOT NULL DEFAULT 0,
  last_timestamp INTEGER NOT NULL DEFAULT 0,
  cpu_min REAL,
  cpu_max REAL,
  mem_used_max INTEGER,
  memory_avg REAL,
  memory_min REAL,
  disk_avg REAL,
  disk_min REAL,
  net_in_avg REAL,
  net_in_min REAL,
  net_out_avg REAL,
  net_out_min REAL,
  PRIMARY KEY(server_id, timestamp)
) WITHOUT ROWID;

CREATE INDEX metric_history_time ON metric_history(timestamp);

CREATE TABLE server_latest_state (
  server_id TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
  latest_timestamp INTEGER NOT NULL,
  latest_json TEXT NOT NULL DEFAULT '{}',
  last_batch_id TEXT NOT NULL DEFAULT ''
) WITHOUT ROWID;

CREATE TABLE admin_2fa (
  username TEXT PRIMARY KEY,
  totp_secret TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
  created_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE remote_tasks (
  id TEXT PRIMARY KEY,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  command TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'sent', 'success', 'failed')),
  requested_by TEXT NOT NULL,
  requested_at INTEGER NOT NULL,
  started_at INTEGER,
  completed_at INTEGER,
  result TEXT NOT NULL DEFAULT '',
  exit_code INTEGER
) WITHOUT ROWID;

CREATE INDEX remote_tasks_server ON remote_tasks(server_id);

CREATE INDEX remote_tasks_status_completed
ON remote_tasks(status, completed_at);

CREATE INDEX remote_tasks_status_requested
ON remote_tasks(status, requested_at);

CREATE TABLE exchange_rates (
  base_currency TEXT PRIMARY KEY,
  rates_json TEXT NOT NULL CHECK(json_valid(rates_json)),
  source TEXT NOT NULL,
  rate_date TEXT NOT NULL,
  fetched_at INTEGER NOT NULL,
  attempted_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  token_hash TEXT NOT NULL UNIQUE,
  username TEXT NOT NULL,
  ip_address TEXT NOT NULL DEFAULT '',
  user_agent TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  CHECK(last_seen_at >= created_at),
  CHECK(expires_at > created_at)
) WITHOUT ROWID;

CREATE INDEX sessions_expires_at ON sessions(expires_at);
CREATE INDEX sessions_user_activity ON sessions(username, last_seen_at DESC);

CREATE TABLE dashboard_proofs (
  token_hash TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL CHECK(expires_at > created_at)
) WITHOUT ROWID;

CREATE INDEX dashboard_proofs_expires_at ON dashboard_proofs(expires_at);

CREATE TABLE server_traffic_state (
  server_id TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
  cycle_key INTEGER NOT NULL DEFAULT 0,
  reset_day INTEGER NOT NULL DEFAULT 1 CHECK(reset_day BETWEEN 1 AND 31),
  timestamp INTEGER NOT NULL DEFAULT 0,
  raw_rx INTEGER NOT NULL DEFAULT 0 CHECK(raw_rx >= 0),
  raw_tx INTEGER NOT NULL DEFAULT 0 CHECK(raw_tx >= 0),
  used_rx INTEGER NOT NULL DEFAULT 0 CHECK(used_rx >= 0),
  used_tx INTEGER NOT NULL DEFAULT 0 CHECK(used_tx >= 0)
) WITHOUT ROWID;

CREATE TABLE latency_tasks (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  task_type TEXT NOT NULL CHECK(task_type IN ('tcp', 'icmp')),
  target TEXT NOT NULL,
  port INTEGER CHECK(port IS NULL OR port BETWEEN 1 AND 65535),
  interval_seconds INTEGER NOT NULL CHECK(interval_seconds BETWEEN 30 AND 3600),
  default_enabled INTEGER NOT NULL DEFAULT 0 CHECK(default_enabled IN (0, 1)),
  sort_order INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK((task_type = 'tcp' AND port IS NOT NULL) OR (task_type = 'icmp' AND port IS NULL))
) WITHOUT ROWID;

CREATE INDEX latency_tasks_sort ON latency_tasks(sort_order, created_at);

CREATE TABLE latency_task_servers (
  task_id TEXT NOT NULL REFERENCES latency_tasks(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  assigned_at INTEGER NOT NULL,
  PRIMARY KEY(task_id, server_id)
) WITHOUT ROWID;

CREATE INDEX latency_task_servers_server ON latency_task_servers(server_id, task_id);

CREATE TABLE latency_results (
  task_id TEXT NOT NULL REFERENCES latency_tasks(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  timestamp INTEGER NOT NULL,
  latency_ms REAL NOT NULL CHECK(latency_ms >= -1),
  packet_loss REAL NOT NULL CHECK(packet_loss BETWEEN 0 AND 100),
  PRIMARY KEY(task_id, server_id, timestamp)
) WITHOUT ROWID;

CREATE INDEX latency_results_server_time
ON latency_results(server_id, timestamp DESC);

CREATE INDEX latency_results_time ON latency_results(timestamp);

CREATE TABLE alert_rules (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  metric TEXT NOT NULL CHECK(metric IN ('cpu', 'memory', 'disk', 'net_in', 'net_out')),
  threshold REAL NOT NULL CHECK(threshold > 0),
  duration_minutes INTEGER NOT NULL CHECK(duration_minutes BETWEEN 1 AND 1440),
  aggregation TEXT NOT NULL CHECK(aggregation IN ('average', 'continuous')),
  all_servers INTEGER NOT NULL DEFAULT 0 CHECK(all_servers IN (0, 1)),
  enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX alert_rules_created_at ON alert_rules(created_at);

CREATE TABLE alert_rule_servers (
  rule_id TEXT NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  PRIMARY KEY(rule_id, server_id)
) WITHOUT ROWID;

CREATE INDEX alert_rule_servers_server ON alert_rule_servers(server_id, rule_id);

CREATE TABLE alert_states (
  state_key TEXT PRIMARY KEY,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  rule_id TEXT REFERENCES alert_rules(id) ON DELETE CASCADE,
  active INTEGER NOT NULL DEFAULT 0 CHECK(active IN (0, 1)),
  updated_at INTEGER NOT NULL,
  details_json TEXT NOT NULL DEFAULT '{}'
) WITHOUT ROWID;

CREATE INDEX alert_states_active_time ON alert_states(active, updated_at);
CREATE INDEX alert_states_server ON alert_states(server_id);
CREATE INDEX alert_states_rule ON alert_states(rule_id);

CREATE TABLE notification_outbox (
  id TEXT PRIMARY KEY,
  state_key TEXT NOT NULL REFERENCES alert_states(state_key) ON DELETE CASCADE,
  sequence INTEGER NOT NULL CHECK(sequence > 0),
  title TEXT NOT NULL,
  server_name TEXT NOT NULL,
  message TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
  next_attempt_at INTEGER NOT NULL,
  UNIQUE(state_key, sequence)
) WITHOUT ROWID;

CREATE INDEX notification_outbox_due ON notification_outbox(next_attempt_at, created_at);

CREATE TABLE notification_telegram (
  id INTEGER PRIMARY KEY CHECK(id = 1),
  bot_token TEXT NOT NULL,
  chat_id TEXT NOT NULL,
  message_thread_id INTEGER,
  template TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE themes (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  url TEXT NOT NULL UNIQUE,
  resolved_url TEXT NOT NULL,
  version TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX themes_created_at ON themes(created_at DESC);

CREATE TABLE theme_previews (
  token_hash TEXT PRIMARY KEY,
  theme_id TEXT NOT NULL REFERENCES themes(id) ON DELETE CASCADE,
  expires_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX theme_previews_expires_at ON theme_previews(expires_at);

PRAGMA optimize;
