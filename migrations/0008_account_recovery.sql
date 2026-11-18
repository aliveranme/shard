ALTER TABLE users ADD COLUMN email_verified_at INTEGER;

-- Keep one link and one cooldown per account and purpose. Clearing the hash
-- invalidates a link without resetting the send throttle.
CREATE TABLE account_email_links (
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  purpose TEXT NOT NULL CHECK (purpose IN ('verify', 'reset')),
  email TEXT NOT NULL,
  token_hash TEXT UNIQUE,
  expires_at INTEGER NOT NULL,
  next_send_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, purpose)
);

CREATE TRIGGER users_email_changed
AFTER UPDATE OF email ON users
WHEN OLD.email COLLATE BINARY <> NEW.email
BEGIN
  UPDATE users SET email_verified_at = NULL WHERE id = NEW.id;
  UPDATE account_email_links SET token_hash = NULL WHERE user_id = NEW.id;
END;

CREATE TRIGGER users_email_verified
AFTER UPDATE OF email_verified_at ON users
WHEN NEW.email_verified_at IS NOT NULL
BEGIN
  UPDATE account_email_links SET token_hash = NULL WHERE user_id = NEW.id AND purpose = 'verify';
END;

CREATE TRIGGER users_password_changed
AFTER UPDATE OF password_hash ON users
WHEN OLD.password_hash <> NEW.password_hash
BEGIN
  DELETE FROM sessions WHERE user_id = NEW.id;
  UPDATE account_email_links SET token_hash = NULL WHERE user_id = NEW.id AND purpose = 'reset';
END;
