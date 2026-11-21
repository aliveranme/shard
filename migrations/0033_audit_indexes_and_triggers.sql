-- Add missing composite, partial, and foreign-key indexes for hot queries,
-- and fix git_ref_observe projection/filtering behavior.

CREATE INDEX IF NOT EXISTS repos_updated
  ON repos(updated_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS repos_owner_updated
  ON repos(owner_id, updated_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS repos_artifacts_repo
  ON repos(artifacts_repo);

CREATE INDEX IF NOT EXISTS issues_repo_kind_state_updated
  ON issues(repo_id, kind, state, updated_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS issues_repo_kind_updated
  ON issues(repo_id, kind, updated_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS issues_kind_state_updated
  ON issues(kind, state, updated_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS issues_kind_updated
  ON issues(kind, updated_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS repo_copies_owner_id
  ON repo_copies(owner_id, id DESC);

CREATE INDEX IF NOT EXISTS merge_queue_queued_branch
  ON merge_queue(repo_id, base_branch)
  WHERE state = 'queued';

CREATE INDEX IF NOT EXISTS pulls_head_commit
  ON pulls(head_commit);

CREATE INDEX IF NOT EXISTS organization_repo_grants_link_role
  ON organization_repo_grants(link_id, organization_id, role, team_id);

CREATE INDEX IF NOT EXISTS issues_author ON issues(author_id);
CREATE INDEX IF NOT EXISTS comments_author ON comments(author_id);
CREATE INDEX IF NOT EXISTS issue_events_actor ON issue_events(actor_id);
CREATE INDEX IF NOT EXISTS pull_reviews_actor ON pull_reviews(actor_id);
CREATE INDEX IF NOT EXISTS merge_queue_actor ON merge_queue(actor_id);
CREATE INDEX IF NOT EXISTS repo_transfers_sender ON repo_transfers(sender_id);

DROP TRIGGER IF EXISTS git_ref_observe;

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
  ON CONFLICT (repo_id, name) DO UPDATE SET oid = excluded.oid
    WHERE NEW.recovered = 0 OR repository_git_refs.oid IS NEW.before_oid;

  INSERT OR REPLACE INTO pull_comparison_updates (issue_id, due_at)
  SELECT p.issue_id, NEW.observed_at FROM issues i JOIN pulls p ON p.issue_id = i.id
  WHERE NEW.name GLOB 'refs/heads/*' AND i.repo_id = NEW.repo_id AND i.kind = 'pull'
    AND i.state = 'open' AND p.merged = 0
    AND (p.base_branch = substr(NEW.name, 12)
         OR (p.source_repo_id IS NULL AND p.head_branch = substr(NEW.name, 12)))
  UNION
  SELECT p.issue_id, NEW.observed_at FROM pulls p JOIN issues i ON i.id = p.issue_id
  WHERE NEW.name GLOB 'refs/heads/*' AND p.source_repo_id = NEW.repo_id AND i.kind = 'pull'
    AND p.head_branch = substr(NEW.name, 12) AND i.state = 'open' AND p.merged = 0;
END;
