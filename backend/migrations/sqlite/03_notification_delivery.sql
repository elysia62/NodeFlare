CREATE TABLE notification_deliveries (
  notification_id TEXT NOT NULL REFERENCES notification_outbox(id) ON DELETE CASCADE,
  channel TEXT NOT NULL CHECK(channel IN ('telegram', 'custom', 'bark', 'discord', 'slack', 'wecom', 'dingtalk', 'feishu', 'ntfy', 'gotify')),
  delivered_at INTEGER NOT NULL,
  PRIMARY KEY(notification_id, channel)
) WITHOUT ROWID;

INSERT INTO settings(key,value)
SELECT 'notification_webhook/' || COALESCE(json_extract(value, '$.preset'), 'custom'), value
FROM settings WHERE key='notification_webhook'
ON CONFLICT(key) DO NOTHING;
DELETE FROM settings WHERE key='notification_webhook';

ALTER TABLE servers ADD COLUMN reset_timezone TEXT NOT NULL DEFAULT 'UTC';
ALTER TABLE servers ADD COLUMN agent_remote_control INTEGER NOT NULL DEFAULT 0 CHECK(agent_remote_control IN (0, 1));
ALTER TABLE server_traffic_state ADD COLUMN reset_timezone TEXT NOT NULL DEFAULT 'UTC';
