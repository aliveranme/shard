-- Keep the ref commands before forwarding a push so a lost response does not
-- lose the only evidence of which refs may have moved. No pack data is stored.
CREATE TABLE git_push_intents (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id    INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  updates    TEXT NOT NULL CHECK (json_valid(updates) AND json_type(updates) = 'array'),
  check_at   INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX git_push_intents_due ON git_push_intents(check_at, id);

-- One transactional entry point for acknowledged and reconciled transitions.
-- The underlying ref store remains authoritative.
CREATE VIEW git_ref_observations (repo_id, name, before_oid, after_oid, actor_id, observed_at, recovered) AS
SELECT NULL, NULL, NULL, NULL, NULL, NULL, NULL WHERE 0;

CREATE TRIGGER git_ref_observe INSTEAD OF INSERT ON git_ref_observations
WHEN NEW.recovered = 0 OR (
  (NEW.after_oid IS NOT NULL OR EXISTS (
    SELECT 1 FROM repository_git_refs WHERE repo_id = NEW.repo_id AND name = NEW.name AND oid IS NEW.before_oid
  )) AND NOT EXISTS (
    SELECT 1 FROM repository_git_refs WHERE repo_id = NEW.repo_id AND name = NEW.name AND oid IS NOT NEW.before_oid
  )
)
BEGIN
  INSERT INTO webhook_events (repo_id, repo, kind, action, actor_id, created_at, ref, before_oid, after_oid)
  SELECT r.id, r.artifacts_repo,
         CASE WHEN NEW.name GLOB 'refs/tags/*' THEN 'tag' ELSE 'push' END,
         CASE WHEN NEW.before_oid IS NULL THEN 'created'
              WHEN NEW.after_oid IS NULL THEN 'deleted' ELSE 'updated' END,
         NEW.actor_id, NEW.observed_at, NEW.name, NEW.before_oid, NEW.after_oid
  FROM repos r WHERE r.id = NEW.repo_id AND EXISTS (
    SELECT 1 FROM webhook_subscriptions s WHERE s.repo_id = r.id AND s.disabled_at IS NULL
  );

  INSERT INTO repository_git_refs (repo_id, name, oid)
  SELECT id, NEW.name, NEW.after_oid FROM repos WHERE id = NEW.repo_id
  ON CONFLICT (repo_id, name) DO UPDATE SET oid = excluded.oid WHERE repository_git_refs.oid IS NEW.before_oid;

  INSERT OR REPLACE INTO pull_comparison_updates (issue_id, due_at)
  SELECT p.issue_id, NEW.observed_at FROM issues i JOIN pulls p ON p.issue_id = i.id
  WHERE NEW.name GLOB 'refs/heads/*' AND i.repo_id = NEW.repo_id AND i.state = 'open' AND p.merged = 0
    AND (p.base_branch = substr(NEW.name, 12)
         OR (p.source_repo_id IS NULL AND p.head_branch = substr(NEW.name, 12)))
  UNION
  SELECT p.issue_id, NEW.observed_at FROM pulls p JOIN issues i ON i.id = p.issue_id
  WHERE NEW.name GLOB 'refs/heads/*' AND p.source_repo_id = NEW.repo_id
    AND p.head_branch = substr(NEW.name, 12) AND i.state = 'open' AND p.merged = 0;
END;
