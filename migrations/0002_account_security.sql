-- Nullable expiry preserves existing access tokens.
ALTER TABLE access_tokens ADD COLUMN expires_at INTEGER;
CREATE INDEX access_tokens_expiry ON access_tokens(expires_at);
CREATE INDEX sessions_expiry ON sessions(expires_at);

-- Keys contain only a hash of the source and normalized login.
CREATE TABLE login_throttle (
  key_hash TEXT PRIMARY KEY,
  failures INTEGER NOT NULL,
  window_started INTEGER NOT NULL
);
CREATE INDEX login_throttle_window ON login_throttle(window_started);
