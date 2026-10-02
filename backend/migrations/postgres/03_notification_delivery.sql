CREATE TABLE notification_deliveries (
  notification_id TEXT NOT NULL REFERENCES notification_outbox(id) ON DELETE CASCADE,
  channel TEXT NOT NULL CHECK(channel IN ('telegram', 'webhook')),
  delivered_at BIGINT NOT NULL,
  PRIMARY KEY(notification_id, channel)
);

ALTER TABLE servers ADD COLUMN reset_timezone TEXT NOT NULL DEFAULT 'UTC';
ALTER TABLE server_traffic_state ADD COLUMN reset_timezone TEXT NOT NULL DEFAULT 'UTC';
