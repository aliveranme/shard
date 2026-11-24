-- Ensure pull metadata can only attach to pull request issues.
DROP TRIGGER pull_source_at_creation;
CREATE TRIGGER pull_source_at_creation BEFORE INSERT ON pulls
WHEN NOT EXISTS (
  SELECT 1 FROM issues i
  JOIN repos t ON t.id = i.repo_id
  WHERE i.id = NEW.issue_id AND i.kind = 'pull'
    AND (NEW.source_repo_id IS NULL OR EXISTS (
      SELECT 1 FROM users u
      JOIN repos s ON s.id = NEW.source_repo_id
      WHERE u.id = i.author_id
        AND (t.visibility = 'public' OR t.owner_id = u.id OR u.is_admin = 1 OR EXISTS (
          SELECT 1 FROM repo_member_roles m WHERE m.repo_id = t.id AND m.user_id = u.id
        ))
        AND (s.visibility = 'public' OR s.owner_id = u.id OR u.is_admin = 1 OR EXISTS (
          SELECT 1 FROM repo_member_roles m WHERE m.repo_id = s.id AND m.user_id = u.id
        ))
        AND (s.id = t.id OR s.visibility = 'public' OR t.visibility <> 'public')
    ))
)
BEGIN
  SELECT RAISE(ABORT, 'pull repository unavailable');
END;
