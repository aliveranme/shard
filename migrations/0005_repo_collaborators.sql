CREATE TABLE repo_collaborators (
  repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  role    TEXT NOT NULL CHECK (role IN ('read', 'write')),
  PRIMARY KEY (repo_id, user_id)
) WITHOUT ROWID;

CREATE INDEX repo_collaborators_user ON repo_collaborators(user_id);
