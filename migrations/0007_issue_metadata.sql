-- Composite foreign keys keep classifications inside the issue's repository.
CREATE UNIQUE INDEX issue_metadata_repo_key ON issues(repo_id, id);

CREATE TABLE repo_labels (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  name    TEXT NOT NULL COLLATE NOCASE,
  UNIQUE(repo_id, name),
  UNIQUE(repo_id, id)
);

CREATE TABLE repo_milestones (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  name    TEXT NOT NULL COLLATE NOCASE,
  UNIQUE(repo_id, name),
  UNIQUE(repo_id, id)
);

CREATE TABLE issue_labels (
  issue_id INTEGER NOT NULL,
  repo_id  INTEGER NOT NULL,
  label_id INTEGER NOT NULL,
  PRIMARY KEY(issue_id, label_id),
  FOREIGN KEY(repo_id, issue_id) REFERENCES issues(repo_id, id) ON DELETE CASCADE,
  FOREIGN KEY(repo_id, label_id) REFERENCES repo_labels(repo_id, id) ON DELETE CASCADE
);
CREATE INDEX issue_labels_catalog ON issue_labels(repo_id, label_id);

CREATE TABLE issue_milestones (
  issue_id     INTEGER PRIMARY KEY,
  repo_id      INTEGER NOT NULL,
  milestone_id INTEGER NOT NULL,
  FOREIGN KEY(repo_id, issue_id) REFERENCES issues(repo_id, id) ON DELETE CASCADE,
  FOREIGN KEY(repo_id, milestone_id) REFERENCES repo_milestones(repo_id, id) ON DELETE CASCADE
);
CREATE INDEX issue_milestones_catalog ON issue_milestones(repo_id, milestone_id);

ALTER TABLE issues ADD COLUMN assignee_id INTEGER REFERENCES users(id) ON DELETE SET NULL;
CREATE INDEX issues_assignee ON issues(assignee_id, repo_id) WHERE assignee_id IS NOT NULL;
CREATE INDEX issue_assignee_site_admins ON users(id) WHERE is_admin = 1;

-- Clear a lost assignment without touching issue content or other repositories.
CREATE TRIGGER issue_assignee_collaborator_removed AFTER DELETE ON repo_collaborators
WHEN EXISTS (SELECT 1 FROM repos WHERE id = OLD.repo_id AND owner_id <> OLD.user_id)
  AND NOT EXISTS (SELECT 1 FROM users WHERE id = OLD.user_id AND is_admin = 1)
BEGIN
  UPDATE issues SET assignee_id = NULL WHERE repo_id = OLD.repo_id AND assignee_id = OLD.user_id;
END;

CREATE TRIGGER issue_assignee_owner_changed AFTER UPDATE OF owner_id ON repos
WHEN NEW.owner_id <> OLD.owner_id
  AND NOT EXISTS (SELECT 1 FROM users WHERE id = OLD.owner_id AND is_admin = 1)
  AND NOT EXISTS (SELECT 1 FROM repo_collaborators WHERE repo_id = NEW.id AND user_id = OLD.owner_id)
BEGIN
  UPDATE issues SET assignee_id = NULL WHERE repo_id = NEW.id AND assignee_id = OLD.owner_id;
END;

CREATE TRIGGER issue_assignee_admin_removed AFTER UPDATE OF is_admin ON users
WHEN OLD.is_admin = 1 AND NEW.is_admin <> 1
BEGIN
  UPDATE issues SET assignee_id = NULL WHERE assignee_id = NEW.id
    AND NOT EXISTS (SELECT 1 FROM repos r WHERE r.id = issues.repo_id AND r.owner_id = NEW.id)
    AND NOT EXISTS (SELECT 1 FROM repo_collaborators c WHERE c.repo_id = issues.repo_id AND c.user_id = NEW.id);
END;
