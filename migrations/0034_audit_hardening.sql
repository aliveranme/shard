-- Revoke linked organization repositories when their owner leaves or is removed,
-- index foreign keys for repository deletion and legacy webhook retention cleanup,
-- and allow notifications/webhooks for pull request reviews from public forks.

DELETE FROM organization_repositories
WHERE NOT EXISTS (
  SELECT 1 FROM repos r
  JOIN organization_members m
    ON m.organization_id = organization_repositories.organization_id
   AND m.user_id = r.owner_id
  WHERE r.id = organization_repositories.repo_id
);

DROP TRIGGER IF EXISTS organization_member_removed_repos;
CREATE TRIGGER organization_member_removed_repos AFTER DELETE ON organization_members
BEGIN
  DELETE FROM organization_repositories
  WHERE organization_id = OLD.organization_id
    AND repo_id IN (SELECT id FROM repos WHERE owner_id = OLD.user_id);
END;

CREATE INDEX IF NOT EXISTS webhook_deliveries_repo
  ON webhook_deliveries(repo_id);

CREATE INDEX IF NOT EXISTS webhook_deliveries_created
  ON webhook_deliveries(created_at, event_id);

CREATE INDEX IF NOT EXISTS merge_queue_repo
  ON merge_queue(repo_id);

DROP TRIGGER IF EXISTS notification_pull_reviewed;
CREATE TRIGGER notification_pull_reviewed AFTER INSERT ON pull_reviews
WHEN NEW.decision = 'merge_verified' OR EXISTS (
  SELECT 1 FROM pulls p JOIN issues i ON i.id = p.issue_id
  LEFT JOIN repos s ON s.id = p.source_repo_id
  WHERE p.issue_id = NEW.issue_id
    AND (COALESCE(p.source_repo_id, i.repo_id) = i.repo_id OR s.visibility = 'public')
)
BEGIN
  INSERT INTO notification_events (issue_id, actor_id, action, created_at)
  VALUES (NEW.issue_id, NEW.actor_id, NEW.decision, NEW.created_at);
END;
