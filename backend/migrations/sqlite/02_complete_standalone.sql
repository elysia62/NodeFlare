PRAGMA foreign_keys = ON;

ALTER TABLE servers ADD COLUMN ip_v4 TEXT NOT NULL DEFAULT '';
ALTER TABLE servers ADD COLUMN ip_v6 TEXT NOT NULL DEFAULT '';

ALTER TABLE metric_history ADD COLUMN gpu_usage REAL NOT NULL DEFAULT 0;
ALTER TABLE metric_history ADD COLUMN disk_read_bps REAL NOT NULL DEFAULT 0;
ALTER TABLE metric_history ADD COLUMN disk_write_bps REAL NOT NULL DEFAULT 0;
ALTER TABLE metric_history ADD COLUMN disk_read_iops REAL NOT NULL DEFAULT 0;
ALTER TABLE metric_history ADD COLUMN disk_write_iops REAL NOT NULL DEFAULT 0;
ALTER TABLE metric_history ADD COLUMN disk_await_ms REAL NOT NULL DEFAULT 0;
ALTER TABLE metric_history ADD COLUMN disk_utilization REAL NOT NULL DEFAULT 0;

ALTER TABLE server_latest_state ADD COLUMN latest_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE server_latest_state ADD COLUMN last_batch_id TEXT NOT NULL DEFAULT '';

CREATE TABLE sessions (
  token_hash TEXT PRIMARY KEY,
  username TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX sessions_expires_at ON sessions(expires_at);

CREATE TABLE dashboard_proofs (
  token_hash TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX dashboard_proofs_expires_at ON dashboard_proofs(expires_at);

CREATE TABLE server_traffic_state (
  server_id TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
  cycle_key INTEGER NOT NULL DEFAULT 0,
  reset_day INTEGER NOT NULL DEFAULT 1,
  timestamp INTEGER NOT NULL DEFAULT 0,
  raw_rx INTEGER NOT NULL DEFAULT 0,
  raw_tx INTEGER NOT NULL DEFAULT 0,
  used_rx INTEGER NOT NULL DEFAULT 0,
  used_tx INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;

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

CREATE INDEX latency_task_servers_server ON latency_task_servers(server_id, task_id);

CREATE TABLE latency_results (
  task_id TEXT NOT NULL REFERENCES latency_tasks(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  timestamp INTEGER NOT NULL,
  latency_ms REAL NOT NULL,
  packet_loss REAL NOT NULL,
  PRIMARY KEY(task_id, server_id, timestamp)
) WITHOUT ROWID;

CREATE INDEX latency_results_server_time ON latency_results(server_id, timestamp DESC);

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

CREATE TABLE alert_states (
  state_key TEXT PRIMARY KEY,
  active INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  details_json TEXT NOT NULL DEFAULT '{}'
) WITHOUT ROWID;

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
);

CREATE INDEX themes_created_at ON themes(created_at DESC);

CREATE TABLE theme_previews (
  token_hash TEXT PRIMARY KEY,
  theme_id TEXT NOT NULL REFERENCES themes(id) ON DELETE CASCADE,
  expires_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX theme_previews_expires_at ON theme_previews(expires_at);

PRAGMA optimize;
