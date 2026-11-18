-- One effective check per context and exact commit in each repository.
CREATE TABLE commit_checks (
  repo_id     INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  commit_hash TEXT NOT NULL CHECK (length(commit_hash) = 40 AND commit_hash NOT GLOB '*[^0-9a-f]*'),
  context     TEXT NOT NULL CHECK (length(CAST(context AS BLOB)) BETWEEN 1 AND 80),
  state       TEXT NOT NULL CHECK (state IN ('pending', 'success', 'failure', 'error')),
  summary     TEXT CHECK (length(CAST(summary AS BLOB)) <= 1024),
  PRIMARY KEY (repo_id, commit_hash, context)
) WITHOUT ROWID;
