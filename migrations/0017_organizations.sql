-- Repository ownership stays with users. An accepted link authorizes its organization to grant team access.
CREATE TABLE organizations (
  id       INTEGER PRIMARY KEY AUTOINCREMENT,
  slug     TEXT NOT NULL UNIQUE COLLATE NOCASE,
  owner_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE
);
CREATE INDEX organizations_owner ON organizations(owner_id);

CREATE TABLE organization_members (
  organization_id INTEGER NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  PRIMARY KEY (organization_id, user_id)
) WITHOUT ROWID;
CREATE INDEX organization_members_user ON organization_members(user_id);

CREATE TRIGGER organization_owner_membership AFTER INSERT ON organizations
BEGIN
  INSERT INTO organization_members (organization_id, user_id) VALUES (NEW.id, NEW.owner_id);
END;

CREATE TABLE organization_teams (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  organization_id INTEGER NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  slug            TEXT NOT NULL COLLATE NOCASE,
  UNIQUE (organization_id, slug),
  UNIQUE (organization_id, id)
);

CREATE TABLE organization_team_members (
  organization_id INTEGER NOT NULL,
  team_id         INTEGER NOT NULL,
  user_id         INTEGER NOT NULL,
  PRIMARY KEY (organization_id, team_id, user_id),
  FOREIGN KEY (organization_id, team_id) REFERENCES organization_teams(organization_id, id) ON DELETE CASCADE,
  FOREIGN KEY (organization_id, user_id) REFERENCES organization_members(organization_id, user_id) ON DELETE CASCADE
) WITHOUT ROWID;
CREATE INDEX organization_team_members_user ON organization_team_members(user_id, organization_id);

CREATE TABLE organization_repositories (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  organization_id INTEGER NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  repo_id         INTEGER NOT NULL UNIQUE REFERENCES repos(id) ON DELETE CASCADE,
  accepted        INTEGER NOT NULL DEFAULT 0 CHECK (accepted IN (0, 1)),
  UNIQUE (organization_id, id)
);

CREATE TABLE organization_repo_grants (
  organization_id INTEGER NOT NULL,
  team_id         INTEGER NOT NULL,
  link_id         INTEGER NOT NULL,
  role            TEXT NOT NULL CHECK (role IN ('read', 'write')),
  PRIMARY KEY (organization_id, team_id, link_id),
  FOREIGN KEY (organization_id, team_id) REFERENCES organization_teams(organization_id, id) ON DELETE CASCADE,
  FOREIGN KEY (organization_id, link_id) REFERENCES organization_repositories(organization_id, id) ON DELETE CASCADE
) WITHOUT ROWID;
CREATE INDEX organization_repo_grants_link ON organization_repo_grants(link_id, organization_id);

-- Moving to a different repository owner requires a fresh offer from that owner.
CREATE TRIGGER organization_repo_owner_changed AFTER UPDATE OF owner_id ON repos
WHEN NEW.owner_id <> OLD.owner_id
BEGIN
  DELETE FROM organization_repositories WHERE repo_id = NEW.id;
END;

-- Consumers can test for any row, or test for a write row when write is required.
CREATE VIEW repo_member_roles AS
SELECT repo_id, user_id, role FROM repo_collaborators
UNION ALL
SELECT l.repo_id, m.user_id, g.role
  FROM organization_repositories l
  JOIN organization_repo_grants g ON g.organization_id = l.organization_id AND g.link_id = l.id
  JOIN organization_team_members m ON m.organization_id = g.organization_id AND m.team_id = g.team_id
 WHERE l.accepted = 1;

DROP VIEW repo_copy_access;
CREATE VIEW repo_copy_access AS
SELECT c.id, c.kind = 'import' OR (s.id IS NOT NULL AND (
  s.visibility = 'public' OR s.owner_id = c.owner_id OR u.is_admin = 1 OR EXISTS (
    SELECT 1 FROM repo_member_roles rc WHERE rc.repo_id = s.id AND rc.user_id = c.owner_id
  )
)) AS allowed
FROM repo_copies c LEFT JOIN repos s ON s.id = c.source_id JOIN users u ON u.id = c.owner_id;

DROP TRIGGER notification_recipients;
CREATE TRIGGER notification_recipients AFTER INSERT ON notification_events
BEGIN
  INSERT INTO notifications (recipient_id, event_id)
  SELECT u.id, NEW.id
    FROM issues i JOIN repos r ON r.id = i.repo_id
    JOIN users u ON (u.id IN (
      SELECT r.owner_id
      UNION ALL SELECT i.author_id
      UNION ALL SELECT i.assignee_id
      UNION ALL SELECT c.author_id FROM comments c WHERE c.issue_id = NEW.issue_id
      UNION ALL SELECT p.actor_id FROM pull_reviews p WHERE p.issue_id = NEW.issue_id
    ) OR u.id IN (SELECT w.user_id FROM repo_watches w WHERE w.repo_id = r.id))
   WHERE i.id = NEW.issue_id AND u.id IS NOT NEW.actor_id
     AND (r.visibility = 'public' OR r.owner_id = u.id OR u.is_admin = 1
          OR EXISTS (SELECT 1 FROM repo_member_roles c WHERE c.repo_id = r.id AND c.user_id = u.id));
END;
