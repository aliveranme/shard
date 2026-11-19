-- HEAD updates on Git services reserve the repository while the Worker waits
-- for storage. An expired reservation can be replaced after a lost request.
ALTER TABLE repos ADD COLUMN git_head_lease_until INTEGER NOT NULL DEFAULT 0;
ALTER TABLE repos ADD COLUMN git_head_lease_id INTEGER NOT NULL DEFAULT 0;

CREATE TRIGGER repo_transfer_head_guard BEFORE UPDATE OF owner_id ON repos
WHEN NEW.owner_id <> OLD.owner_id AND OLD.git_head_lease_until > unixepoch()
BEGIN
  SELECT RAISE(ABORT, 'repository Git HEAD update in progress');
END;

CREATE TRIGGER repo_transfer_disable_webhooks AFTER UPDATE OF owner_id ON repos
WHEN NEW.owner_id <> OLD.owner_id
BEGIN
  UPDATE webhook_subscriptions SET disabled_at = unixepoch(), encrypted_secret = NULL
  WHERE repo_id = NEW.id AND disabled_at IS NULL;

  UPDATE webhook_subscription_deliveries SET state = 'canceled'
  WHERE repo_id = NEW.id AND state IN ('pending', 'sending');
END;
