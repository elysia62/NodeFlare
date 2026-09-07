CREATE TABLE server_install_tokens (
  token_hash TEXT PRIMARY KEY,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  created_at BIGINT NOT NULL
);

CREATE INDEX server_install_tokens_server
ON server_install_tokens(server_id, created_at DESC);
