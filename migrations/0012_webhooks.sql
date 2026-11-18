-- Capture immutable event facts in the source write transaction. Deleting a
-- thread removes its pending deliveries through the notification event FK.
CREATE TABLE webhook_deliveries (
  event_id        INTEGER PRIMARY KEY REFERENCES notification_events(id) ON DELETE CASCADE,
  repo_id         INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  issue_number    INTEGER NOT NULL,
  kind            TEXT NOT NULL,
  action          TEXT NOT NULL,
  actor_id        INTEGER,
  created_at      INTEGER NOT NULL,
  state           TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'sending', 'sent', 'failed')),
  attempts        INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER NOT NULL,
  finished_at     INTEGER,
  last_status     INTEGER
);
CREATE INDEX webhook_deliveries_due ON webhook_deliveries(repo_id, next_attempt_at, event_id)
  WHERE state IN ('pending', 'sending');

CREATE TRIGGER webhook_capture_event AFTER INSERT ON notification_events
BEGIN
  INSERT INTO webhook_deliveries (event_id, repo_id, issue_number, kind, action, actor_id, created_at, next_attempt_at)
  SELECT NEW.id, i.repo_id, i.number, i.kind, NEW.action, NEW.actor_id, NEW.created_at, NEW.created_at
    FROM issues i WHERE i.id = NEW.issue_id;
END;
