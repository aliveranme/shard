-- Keep environment-configured deliveries in the original outbox.
CREATE TABLE webhook_subscriptions (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id          INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  endpoint         TEXT NOT NULL,
  endpoint_origin  TEXT NOT NULL,
  label            TEXT NOT NULL,
  encrypted_secret TEXT,
  disabled_at      INTEGER,
  UNIQUE (repo_id, id),
  CHECK ((disabled_at IS NULL) = (encrypted_secret IS NOT NULL))
);
CREATE UNIQUE INDEX webhook_subscriptions_active_endpoint
  ON webhook_subscriptions(repo_id, endpoint) WHERE disabled_at IS NULL;

-- An observed-ref projection. Keep deletions as tombstones to fence late writes.
CREATE TABLE repository_git_refs (
  repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  name    TEXT NOT NULL,
  oid     TEXT,
  PRIMARY KEY (repo_id, name)
) WITHOUT ROWID;

CREATE TABLE webhook_events (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id      INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  repo         TEXT NOT NULL,
  issue_number INTEGER,
  kind         TEXT NOT NULL CHECK (kind IN ('issue', 'pull', 'push', 'tag')),
  action       TEXT NOT NULL,
  actor_id     INTEGER,
  created_at   INTEGER NOT NULL,
  ref          TEXT,
  before_oid   TEXT,
  after_oid    TEXT,
  UNIQUE (repo_id, id),
  CHECK ((issue_number IS NOT NULL AND ref IS NULL AND kind IN ('issue', 'pull'))
      OR (issue_number IS NULL AND ref IS NOT NULL AND kind IN ('push', 'tag')))
);
CREATE INDEX webhook_events_retention ON webhook_events(created_at, id);

CREATE TABLE webhook_subscription_deliveries (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id          INTEGER NOT NULL,
  subscription_id  INTEGER NOT NULL,
  event_id         INTEGER NOT NULL,
  state            TEXT NOT NULL DEFAULT 'pending'
                   CHECK (state IN ('pending', 'sending', 'sent', 'failed', 'canceled')),
  attempts         INTEGER NOT NULL DEFAULT 0,
  next_attempt_at  INTEGER NOT NULL,
  last_status      INTEGER,
  FOREIGN KEY (repo_id, subscription_id) REFERENCES webhook_subscriptions(repo_id, id) ON DELETE CASCADE,
  FOREIGN KEY (repo_id, event_id) REFERENCES webhook_events(repo_id, id) ON DELETE CASCADE
);
CREATE INDEX webhook_subscription_deliveries_due
  ON webhook_subscription_deliveries(next_attempt_at, id) WHERE state IN ('pending', 'sending');
CREATE INDEX webhook_subscription_deliveries_recent
  ON webhook_subscription_deliveries(repo_id, subscription_id, id DESC);
CREATE UNIQUE INDEX webhook_subscription_deliveries_event
  ON webhook_subscription_deliveries(repo_id, event_id, subscription_id);

CREATE TRIGGER webhook_enqueue_subscriptions AFTER INSERT ON webhook_events
BEGIN
  INSERT INTO webhook_subscription_deliveries (repo_id, subscription_id, event_id, next_attempt_at)
  SELECT NEW.repo_id, s.id, NEW.id, NEW.created_at
    FROM webhook_subscriptions s WHERE s.repo_id = NEW.repo_id AND s.disabled_at IS NULL;
END;

CREATE TRIGGER webhook_capture_subscriptions AFTER INSERT ON notification_events
BEGIN
  INSERT INTO webhook_events (repo_id, repo, issue_number, kind, action, actor_id, created_at)
  SELECT i.repo_id, r.artifacts_repo, i.number, i.kind, NEW.action, NEW.actor_id, NEW.created_at
    FROM issues i JOIN repos r ON r.id = i.repo_id
   WHERE i.id = NEW.issue_id AND EXISTS (
      SELECT 1 FROM webhook_subscriptions s WHERE s.repo_id = i.repo_id AND s.disabled_at IS NULL
   );
END;
