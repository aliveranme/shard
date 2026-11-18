-- NULL marks comparisons created before source repositories were recorded.
-- Keep an explicit source ID when its repository is deleted.
ALTER TABLE pulls ADD COLUMN source_repo_id INTEGER CHECK (source_repo_id > 0);
CREATE INDEX pulls_source_branch ON pulls(source_repo_id, head_branch) WHERE merged = 0;

CREATE TRIGGER pull_source_at_creation BEFORE INSERT ON pulls
WHEN NEW.source_repo_id IS NOT NULL AND NOT EXISTS (
  SELECT 1 FROM issues i
  JOIN users u ON u.id = i.author_id
  JOIN repos t ON t.id = i.repo_id
  JOIN repos s ON s.id = NEW.source_repo_id
  WHERE i.id = NEW.issue_id
    AND (t.visibility = 'public' OR t.owner_id = u.id OR u.is_admin = 1 OR EXISTS (
      SELECT 1 FROM repo_member_roles m WHERE m.repo_id = t.id AND m.user_id = u.id
    ))
    AND (s.visibility = 'public' OR s.owner_id = u.id OR u.is_admin = 1 OR EXISTS (
      SELECT 1 FROM repo_member_roles m WHERE m.repo_id = s.id AND m.user_id = u.id
    ))
    AND (s.id = t.id OR s.visibility = 'public' OR t.visibility <> 'public')
)
BEGIN
  SELECT RAISE(ABORT, 'pull repository unavailable');
END;

CREATE TABLE pull_comparison_history (
  issue_id            INTEGER NOT NULL REFERENCES pulls(issue_id) ON DELETE CASCADE,
  comparison_version  INTEGER NOT NULL,
  base_commit         TEXT,
  head_commit         TEXT,
  merge_base          TEXT,
  PRIMARY KEY (issue_id, comparison_version)
) WITHOUT ROWID;

CREATE TRIGGER pull_comparison_archived AFTER UPDATE OF comparison_version ON pulls
WHEN OLD.comparison_version <> NEW.comparison_version
BEGIN
  INSERT INTO pull_comparison_history (issue_id, comparison_version, base_commit, head_commit, merge_base)
  VALUES (OLD.issue_id, OLD.comparison_version, OLD.base_commit, OLD.head_commit, OLD.merge_base);
END;

CREATE TABLE pull_comparison_updates (
  generation  INTEGER PRIMARY KEY AUTOINCREMENT,
  issue_id    INTEGER NOT NULL UNIQUE REFERENCES pulls(issue_id) ON DELETE CASCADE,
  due_at      INTEGER NOT NULL
);
CREATE INDEX pull_comparison_updates_due ON pull_comparison_updates(due_at, issue_id);

-- Target integrations may be readable by users who cannot read the source.
-- The comparison review remains available in the pull discussion to authorized readers.
DROP TRIGGER notification_pull_reviewed;
CREATE TRIGGER notification_pull_reviewed AFTER INSERT ON pull_reviews
WHEN NEW.decision = 'merge_verified' OR EXISTS (
  SELECT 1 FROM pulls p JOIN issues i ON i.id = p.issue_id
  WHERE p.issue_id = NEW.issue_id AND COALESCE(p.source_repo_id, i.repo_id) = i.repo_id
)
BEGIN
  INSERT INTO notification_events (issue_id, actor_id, action, created_at)
  VALUES (NEW.issue_id, NEW.actor_id, NEW.decision, NEW.created_at);
END;
