ALTER TABLE repo_copies ADD COLUMN last_checked_at INTEGER;
CREATE INDEX repo_copies_reconcile ON repo_copies(COALESCE(last_checked_at, created_at), id)
  WHERE state IN ('starting', 'pending', 'discard');

CREATE TABLE repo_metadata_sources (
  repo_id     INTEGER PRIMARY KEY REFERENCES repos(id) ON DELETE CASCADE,
  source_url  TEXT NOT NULL,
  imported_at INTEGER NOT NULL
);

ALTER TABLE issues ADD COLUMN metadata_imported INTEGER NOT NULL DEFAULT 0
  CHECK (metadata_imported IN (0, 1));
ALTER TABLE repo_releases ADD COLUMN metadata_imported INTEGER NOT NULL DEFAULT 0
  CHECK (metadata_imported IN (0, 1));

-- Import historical rows without sending current issue-open notifications or webhooks.
DROP TRIGGER notification_issue_opened;
CREATE TRIGGER notification_issue_opened AFTER INSERT ON issues
WHEN NEW.metadata_imported = 0
BEGIN
  INSERT INTO notification_events (issue_id, actor_id, action, created_at)
  VALUES (NEW.id, NEW.author_id, 'opened', NEW.created_at);
END;
