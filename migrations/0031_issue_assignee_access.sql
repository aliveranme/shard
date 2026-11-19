-- Owners, site admins and effective repository members can be assigned issues.
CREATE VIEW repo_assignee_access (repo_id, user_id) AS
SELECT id, owner_id FROM repos
UNION ALL
SELECT r.id, u.id FROM repos r CROSS JOIN users u WHERE u.is_admin = 1
UNION ALL
SELECT repo_id, user_id FROM repo_member_roles;

DROP TRIGGER issue_assignee_collaborator_removed;
DROP TRIGGER issue_assignee_owner_changed;
DROP TRIGGER issue_assignee_admin_removed;

CREATE TRIGGER issue_assignee_collaborator_removed AFTER DELETE ON repo_collaborators
WHEN EXISTS (SELECT 1 FROM repos WHERE id = OLD.repo_id)
  AND NOT EXISTS (SELECT 1 FROM repo_assignee_access WHERE repo_id = OLD.repo_id AND user_id = OLD.user_id)
BEGIN
  UPDATE issues SET assignee_id = NULL WHERE repo_id = OLD.repo_id AND assignee_id = OLD.user_id;
END;

CREATE TRIGGER issue_assignee_owner_changed AFTER UPDATE OF owner_id ON repos
WHEN NEW.owner_id <> OLD.owner_id
  AND NOT EXISTS (SELECT 1 FROM repo_assignee_access WHERE repo_id = NEW.id AND user_id = OLD.owner_id)
BEGIN
  UPDATE issues SET assignee_id = NULL WHERE repo_id = NEW.id AND assignee_id = OLD.owner_id;
END;

CREATE TRIGGER issue_assignee_admin_removed AFTER UPDATE OF is_admin ON users
WHEN OLD.is_admin = 1 AND NEW.is_admin <> 1
BEGIN
  UPDATE issues SET assignee_id = NULL WHERE assignee_id = NEW.id
    AND NOT EXISTS (SELECT 1 FROM repo_assignee_access a WHERE a.repo_id = issues.repo_id AND a.user_id = NEW.id);
END;

-- Team and organization deletions cascade through memberships and grants. The
-- first removal of a membership or grant can still use the other to find issues.
CREATE TRIGGER issue_assignee_team_member_removed AFTER DELETE ON organization_team_members
BEGIN
  UPDATE issues SET assignee_id = NULL WHERE assignee_id = OLD.user_id
    AND repo_id IN (
      SELECT l.repo_id FROM organization_repo_grants g
      JOIN organization_repositories l ON l.organization_id = g.organization_id AND l.id = g.link_id
      WHERE g.organization_id = OLD.organization_id AND g.team_id = OLD.team_id AND l.accepted = 1
    )
    AND NOT EXISTS (SELECT 1 FROM repo_assignee_access a WHERE a.repo_id = issues.repo_id AND a.user_id = OLD.user_id);
END;

CREATE TRIGGER issue_assignee_team_grant_removed AFTER DELETE ON organization_repo_grants
BEGIN
  UPDATE issues SET assignee_id = NULL
    WHERE repo_id = (
      SELECT repo_id FROM organization_repositories WHERE id = OLD.link_id AND accepted = 1
    )
    AND assignee_id IN (
      SELECT user_id FROM organization_team_members WHERE organization_id = OLD.organization_id AND team_id = OLD.team_id
    )
    AND NOT EXISTS (SELECT 1 FROM repo_assignee_access a WHERE a.repo_id = issues.repo_id AND a.user_id = issues.assignee_id);
END;

-- Grant cascades cannot look up a link after it has been deleted. Use the old
-- link's repository to clear any assignments supported only by that link.
CREATE TRIGGER issue_assignee_org_link_removed AFTER DELETE ON organization_repositories
WHEN OLD.accepted = 1 AND EXISTS (SELECT 1 FROM repos WHERE id = OLD.repo_id)
BEGIN
  UPDATE issues SET assignee_id = NULL WHERE repo_id = OLD.repo_id AND assignee_id IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM repo_assignee_access a WHERE a.repo_id = OLD.repo_id AND a.user_id = issues.assignee_id);
END;

CREATE TRIGGER issue_assignee_org_link_unaccepted AFTER UPDATE OF accepted ON organization_repositories
WHEN OLD.accepted = 1 AND NEW.accepted = 0
BEGIN
  UPDATE issues SET assignee_id = NULL WHERE repo_id = OLD.repo_id AND assignee_id IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM repo_assignee_access a WHERE a.repo_id = OLD.repo_id AND a.user_id = issues.assignee_id);
END;

-- Reconcile existing assignments when upgrading an installation.
UPDATE issues SET assignee_id = NULL WHERE assignee_id IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM repo_assignee_access a WHERE a.repo_id = issues.repo_id AND a.user_id = issues.assignee_id);
