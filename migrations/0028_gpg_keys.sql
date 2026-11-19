CREATE TABLE gpg_keys (
  user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  fingerprint TEXT NOT NULL CHECK (length(fingerprint) IN (40, 64)),
  email       TEXT NOT NULL COLLATE NOCASE,
  name        TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
  public_key  TEXT NOT NULL CHECK (length(public_key) BETWEEN 1 AND 32768),
  key_ids     TEXT NOT NULL CHECK (json_valid(key_ids) AND json_type(key_ids) = 'array'),
  created_at  INTEGER NOT NULL,
  PRIMARY KEY (user_id, fingerprint)
);
