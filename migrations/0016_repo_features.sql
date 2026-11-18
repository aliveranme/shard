CREATE TABLE repo_releases (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id      INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  tag_name     TEXT NOT NULL CHECK (length(tag_name) > 0),
  commit_hash  TEXT NOT NULL CHECK (length(commit_hash) = 40 AND commit_hash NOT GLOB '*[^0-9a-f]*'),
  title        TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 200),
  notes        TEXT NOT NULL CHECK (length(CAST(notes AS BLOB)) <= 65536),
  created_at   INTEGER NOT NULL,
  published_at INTEGER,
  version      INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
  UNIQUE (repo_id, tag_name)
);
CREATE INDEX repo_releases_recent ON repo_releases(repo_id, COALESCE(published_at, created_at) DESC, id DESC);

CREATE TABLE issue_templates (
  id       INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id  INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  name     TEXT NOT NULL COLLATE NOCASE CHECK (length(name) BETWEEN 1 AND 100),
  title    TEXT NOT NULL CHECK (length(title) <= 200),
  body     TEXT NOT NULL CHECK (length(CAST(body AS BLOB)) <= 65536),
  version  INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
  UNIQUE (repo_id, name)
);

CREATE TABLE issue_reactions (
  issue_id  INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
  reaction  TEXT NOT NULL CHECK (reaction IN ('up', 'down')),
  user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  PRIMARY KEY (issue_id, reaction, user_id)
) WITHOUT ROWID;
CREATE INDEX issue_reactions_user ON issue_reactions(user_id);

CREATE TABLE repo_watches (
  repo_id  INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  user_id  INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  PRIMARY KEY (repo_id, user_id)
) WITHOUT ROWID;
CREATE INDEX repo_watches_user ON repo_watches(user_id);

CREATE TABLE repo_stars (
  user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  repo_id     INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  created_at  INTEGER NOT NULL,
  PRIMARY KEY (user_id, repo_id)
) WITHOUT ROWID;
CREATE INDEX repo_stars_recent ON repo_stars(user_id, created_at DESC, repo_id DESC);
CREATE INDEX repo_stars_repo ON repo_stars(repo_id);

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
          OR EXISTS (SELECT 1 FROM repo_collaborators c WHERE c.repo_id = r.id AND c.user_id = u.id));
END;
