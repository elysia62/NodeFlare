-- SQLite migration for NodeFlare backend
PRAGMA foreign_keys = ON;

-- Settings table (key-value store)
CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
) WITHOUT ROWID;

-- Servers table
CREATE TABLE IF NOT EXISTS servers (
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
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_servers_sort ON servers(sort_order, created_at);

-- Minute-level metrics history
CREATE TABLE IF NOT EXISTS metric_history (
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
  PRIMARY KEY(server_id, timestamp)
) WITHOUT ROWID;

-- Latest state per server
CREATE TABLE IF NOT EXISTS server_latest_state (
  server_id TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
  latest_timestamp INTEGER NOT NULL,
  cpu REAL NOT NULL DEFAULT 0,
  mem_used INTEGER NOT NULL DEFAULT 0,
  mem_total INTEGER NOT NULL DEFAULT 0,
  disk_used INTEGER NOT NULL DEFAULT 0,
  disk_total INTEGER NOT NULL DEFAULT 0,
  net_in REAL NOT NULL DEFAULT 0,
  net_out REAL NOT NULL DEFAULT 0,
  uptime INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;

-- 2FA configuration
CREATE TABLE IF NOT EXISTS admin_2fa (
  username TEXT PRIMARY KEY,
  totp_secret TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
) WITHOUT ROWID;

-- Remote execution tasks
CREATE TABLE IF NOT EXISTS remote_tasks (
  id TEXT PRIMARY KEY,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  command TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'sent', 'running', 'success', 'failed')),
  requested_by TEXT NOT NULL,
  requested_at INTEGER NOT NULL,
  started_at INTEGER,
  completed_at INTEGER,
  result TEXT NOT NULL DEFAULT '',
  exit_code INTEGER
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_remote_tasks_server ON remote_tasks(server_id, requested_at DESC);
CREATE INDEX IF NOT EXISTS idx_remote_tasks_status ON remote_tasks(status, requested_at DESC);

-- Exchange rates
CREATE TABLE IF NOT EXISTS exchange_rates (
  base_currency TEXT PRIMARY KEY,
  rates_json TEXT NOT NULL CHECK(json_valid(rates_json)),
  source TEXT NOT NULL,
  rate_date TEXT NOT NULL,
  fetched_at INTEGER NOT NULL,
  attempted_at INTEGER NOT NULL
) WITHOUT ROWID;
