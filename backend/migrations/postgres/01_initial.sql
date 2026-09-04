CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE servers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  region TEXT NOT NULL DEFAULT '',
  group_name TEXT NOT NULL DEFAULT '默认',
  tags TEXT NOT NULL DEFAULT '',
  hidden BIGINT NOT NULL DEFAULT 0,
  sort_order BIGINT NOT NULL DEFAULT 0,
  expires_at BIGINT,
  traffic_limit BIGINT NOT NULL DEFAULT 0,
  traffic_limit_type TEXT NOT NULL DEFAULT 'sum',
  price DOUBLE PRECISION NOT NULL DEFAULT 0,
  billing_cycle BIGINT NOT NULL DEFAULT 30,
  currency TEXT NOT NULL DEFAULT 'CNY',
  auto_renewal BIGINT NOT NULL DEFAULT 0,
  last_ip TEXT NOT NULL DEFAULT '',
  ip_v4 TEXT NOT NULL DEFAULT '',
  ip_v6 TEXT NOT NULL DEFAULT '',
  network_interface TEXT NOT NULL DEFAULT '',
  reset_day BIGINT NOT NULL DEFAULT 1,
  report_interval BIGINT NOT NULL DEFAULT 60,
  collect_interval BIGINT NOT NULL DEFAULT 1,
  rx_correction BIGINT NOT NULL DEFAULT 0,
  tx_correction BIGINT NOT NULL DEFAULT 0,
  agent_mirror TEXT NOT NULL DEFAULT '',
  offline_notify_disabled BIGINT NOT NULL DEFAULT 0,
  auto_update BIGINT NOT NULL DEFAULT 1,
  token_hash TEXT NOT NULL UNIQUE,
  created_at BIGINT NOT NULL,
  updated_at BIGINT NOT NULL
);

CREATE INDEX servers_sort ON servers(sort_order, created_at);

CREATE TABLE metric_history (
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  timestamp BIGINT NOT NULL,
  cpu DOUBLE PRECISION NOT NULL DEFAULT 0,
  load1 DOUBLE PRECISION NOT NULL DEFAULT 0,
  load5 DOUBLE PRECISION NOT NULL DEFAULT 0,
  load15 DOUBLE PRECISION NOT NULL DEFAULT 0,
  mem_used BIGINT NOT NULL DEFAULT 0,
  mem_total BIGINT NOT NULL DEFAULT 0,
  swap_used BIGINT NOT NULL DEFAULT 0,
  swap_total BIGINT NOT NULL DEFAULT 0,
  disk_used BIGINT NOT NULL DEFAULT 0,
  disk_total BIGINT NOT NULL DEFAULT 0,
  net_in DOUBLE PRECISION NOT NULL DEFAULT 0,
  net_out DOUBLE PRECISION NOT NULL DEFAULT 0,
  net_rx_total BIGINT NOT NULL DEFAULT 0,
  net_tx_total BIGINT NOT NULL DEFAULT 0,
  uptime BIGINT NOT NULL DEFAULT 0,
  processes BIGINT NOT NULL DEFAULT 0,
  tcp_connections BIGINT NOT NULL DEFAULT 0,
  udp_connections BIGINT NOT NULL DEFAULT 0,
  gpu_usage DOUBLE PRECISION NOT NULL DEFAULT 0,
  disk_read_bps DOUBLE PRECISION NOT NULL DEFAULT 0,
  disk_write_bps DOUBLE PRECISION NOT NULL DEFAULT 0,
  disk_read_iops DOUBLE PRECISION NOT NULL DEFAULT 0,
  disk_write_iops DOUBLE PRECISION NOT NULL DEFAULT 0,
  disk_await_ms DOUBLE PRECISION NOT NULL DEFAULT 0,
  disk_utilization DOUBLE PRECISION NOT NULL DEFAULT 0,
  PRIMARY KEY(server_id, timestamp)
);

CREATE INDEX metric_history_time ON metric_history(timestamp);

CREATE TABLE server_latest_state (
  server_id TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
  latest_timestamp BIGINT NOT NULL,
  cpu DOUBLE PRECISION NOT NULL DEFAULT 0,
  mem_used BIGINT NOT NULL DEFAULT 0,
  mem_total BIGINT NOT NULL DEFAULT 0,
  disk_used BIGINT NOT NULL DEFAULT 0,
  disk_total BIGINT NOT NULL DEFAULT 0,
  net_in DOUBLE PRECISION NOT NULL DEFAULT 0,
  net_out DOUBLE PRECISION NOT NULL DEFAULT 0,
  uptime BIGINT NOT NULL DEFAULT 0,
  latest_json TEXT NOT NULL DEFAULT '{}',
  last_batch_id TEXT NOT NULL DEFAULT ''
);

CREATE INDEX server_latest_state_time ON server_latest_state(latest_timestamp);

CREATE TABLE admin_2fa (
  username TEXT PRIMARY KEY,
  totp_secret TEXT NOT NULL,
  enabled BIGINT NOT NULL DEFAULT 1,
  created_at BIGINT NOT NULL
);

CREATE TABLE remote_tasks (
  id TEXT PRIMARY KEY,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  command TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'sent', 'running', 'success', 'failed')),
  requested_by TEXT NOT NULL,
  requested_at BIGINT NOT NULL,
  started_at BIGINT,
  completed_at BIGINT,
  result TEXT NOT NULL DEFAULT '',
  exit_code BIGINT
);

CREATE INDEX remote_tasks_server_status_time
ON remote_tasks(server_id, status, requested_at);

CREATE INDEX remote_tasks_status_completed
ON remote_tasks(status, completed_at);

CREATE INDEX remote_tasks_status_requested
ON remote_tasks(status, requested_at);

CREATE TABLE exchange_rates (
  base_currency TEXT PRIMARY KEY,
  rates_json TEXT NOT NULL,
  source TEXT NOT NULL,
  rate_date TEXT NOT NULL,
  fetched_at BIGINT NOT NULL,
  attempted_at BIGINT NOT NULL
);

CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  token_hash TEXT NOT NULL UNIQUE,
  username TEXT NOT NULL,
  ip_address TEXT NOT NULL DEFAULT '',
  user_agent TEXT NOT NULL DEFAULT '',
  created_at BIGINT NOT NULL,
  last_seen_at BIGINT NOT NULL,
  expires_at BIGINT NOT NULL
);

CREATE INDEX sessions_expires_at ON sessions(expires_at);
CREATE INDEX sessions_user_activity ON sessions(username, last_seen_at DESC);

CREATE TABLE dashboard_proofs (
  token_hash TEXT PRIMARY KEY,
  created_at BIGINT NOT NULL,
  expires_at BIGINT NOT NULL
);

CREATE INDEX dashboard_proofs_expires_at ON dashboard_proofs(expires_at);

CREATE TABLE server_traffic_state (
  server_id TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
  cycle_key BIGINT NOT NULL DEFAULT 0,
  reset_day BIGINT NOT NULL DEFAULT 1,
  timestamp BIGINT NOT NULL DEFAULT 0,
  raw_rx BIGINT NOT NULL DEFAULT 0,
  raw_tx BIGINT NOT NULL DEFAULT 0,
  used_rx BIGINT NOT NULL DEFAULT 0,
  used_tx BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE latency_tasks (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  task_type TEXT NOT NULL CHECK(task_type IN ('tcp', 'icmp')),
  target TEXT NOT NULL,
  port BIGINT CHECK(port IS NULL OR port BETWEEN 1 AND 65535),
  interval_seconds BIGINT NOT NULL CHECK(interval_seconds BETWEEN 30 AND 3600),
  default_enabled BIGINT NOT NULL DEFAULT 0,
  sort_order BIGINT NOT NULL DEFAULT 0,
  created_at BIGINT NOT NULL,
  updated_at BIGINT NOT NULL,
  CHECK((task_type = 'tcp' AND port IS NOT NULL) OR (task_type = 'icmp' AND port IS NULL))
);

CREATE INDEX latency_tasks_sort ON latency_tasks(sort_order, created_at);

CREATE TABLE latency_task_servers (
  task_id TEXT NOT NULL REFERENCES latency_tasks(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  assigned_at BIGINT NOT NULL,
  PRIMARY KEY(task_id, server_id)
);

CREATE INDEX latency_task_servers_server ON latency_task_servers(server_id, task_id);

CREATE TABLE latency_results (
  task_id TEXT NOT NULL REFERENCES latency_tasks(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  timestamp BIGINT NOT NULL,
  latency_ms DOUBLE PRECISION NOT NULL,
  packet_loss DOUBLE PRECISION NOT NULL,
  PRIMARY KEY(task_id, server_id, timestamp)
);

CREATE INDEX latency_results_server_time ON latency_results(server_id, timestamp DESC);
CREATE INDEX latency_results_time ON latency_results(timestamp);

CREATE TABLE alert_rules (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  metric TEXT NOT NULL CHECK(metric IN ('cpu', 'memory', 'disk', 'net_in', 'net_out')),
  threshold DOUBLE PRECISION NOT NULL CHECK(threshold > 0),
  duration_minutes BIGINT NOT NULL CHECK(duration_minutes BETWEEN 1 AND 1440),
  aggregation TEXT NOT NULL CHECK(aggregation IN ('average', 'continuous')),
  enabled BIGINT NOT NULL DEFAULT 1,
  created_at BIGINT NOT NULL,
  updated_at BIGINT NOT NULL
);

CREATE INDEX alert_rules_created_at ON alert_rules(created_at);

CREATE TABLE alert_rule_servers (
  rule_id TEXT NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  PRIMARY KEY(rule_id, server_id)
);

CREATE INDEX alert_rule_servers_server ON alert_rule_servers(server_id, rule_id);

CREATE TABLE alert_states (
  state_key TEXT PRIMARY KEY,
  active BIGINT NOT NULL DEFAULT 0,
  updated_at BIGINT NOT NULL,
  details_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX alert_states_active_time ON alert_states(active, updated_at);

CREATE TABLE notification_telegram (
  id BIGINT PRIMARY KEY CHECK(id = 1),
  bot_token TEXT NOT NULL,
  chat_id TEXT NOT NULL,
  message_thread_id BIGINT,
  template TEXT NOT NULL,
  updated_at BIGINT NOT NULL
);

CREATE TABLE themes (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  url TEXT NOT NULL UNIQUE,
  resolved_url TEXT NOT NULL,
  version TEXT NOT NULL DEFAULT '',
  created_at BIGINT NOT NULL
);

CREATE INDEX themes_created_at ON themes(created_at DESC);

CREATE TABLE theme_previews (
  token_hash TEXT PRIMARY KEY,
  theme_id TEXT NOT NULL REFERENCES themes(id) ON DELETE CASCADE,
  expires_at BIGINT NOT NULL
);

CREATE INDEX theme_previews_expires_at ON theme_previews(expires_at);
