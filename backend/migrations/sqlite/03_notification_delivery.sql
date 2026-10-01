CREATE TABLE notification_deliveries (
  notification_id TEXT NOT NULL REFERENCES notification_outbox(id) ON DELETE CASCADE,
  channel TEXT NOT NULL CHECK(channel IN ('telegram', 'webhook')),
  delivered_at INTEGER NOT NULL,
  PRIMARY KEY(notification_id, channel)
) WITHOUT ROWID;
