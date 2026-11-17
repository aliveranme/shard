-- Shard core schema. All timestamps are unix seconds.
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS users (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
  email         TEXT NOT NULL UNIQUE COLLATE NOCASE,
  password_hash TEXT NOT NULL,
  is_admin      INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
  token_hash TEXT PRIMARY KEY,
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_user ON sessions(user_id);

-- Git access tokens (HTTP basic password or bearer). Only the SHA-256 is stored.
CREATE TABLE IF NOT EXISTS access_tokens (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name         TEXT NOT NULL,
  token_hash   TEXT NOT NULL UNIQUE,
  scope        TEXT NOT NULL DEFAULT 'write',
  created_at   INTEGER NOT NULL,
  last_used_at INTEGER
);
CREATE INDEX IF NOT EXISTS access_tokens_user ON access_tokens(user_id);

CREATE TABLE IF NOT EXISTS repos (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  owner_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name              TEXT NOT NULL,
  description       TEXT NOT NULL DEFAULT '',
  default_branch    TEXT NOT NULL DEFAULT 'main',
  artifacts_repo    TEXT NOT NULL,
  artifacts_remote  TEXT NOT NULL DEFAULT '',
  visibility        TEXT NOT NULL DEFAULT 'public',
  next_issue_number INTEGER NOT NULL DEFAULT 1,
  forked_from       TEXT,
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL,
  UNIQUE(owner_id, name)
);
CREATE INDEX IF NOT EXISTS repos_owner ON repos(owner_id, name);

CREATE TABLE IF NOT EXISTS issues (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id    INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  number     INTEGER NOT NULL,
  kind       TEXT NOT NULL DEFAULT 'issue', -- 'issue' | 'pull'
  title      TEXT NOT NULL,
  body       TEXT NOT NULL DEFAULT '',
  state      TEXT NOT NULL DEFAULT 'open',  -- 'open' | 'closed'
  author_id  INTEGER REFERENCES users(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  closed_at  INTEGER,
  UNIQUE(repo_id, number)
);
CREATE INDEX IF NOT EXISTS issues_repo_state ON issues(repo_id, kind, state, updated_at DESC);

CREATE TABLE IF NOT EXISTS pulls (
  issue_id     INTEGER PRIMARY KEY REFERENCES issues(id) ON DELETE CASCADE,
  head_branch  TEXT NOT NULL,
  base_branch  TEXT NOT NULL,
  head_commit  TEXT,
  base_commit  TEXT,
  merged       INTEGER NOT NULL DEFAULT 0,
  merged_at    INTEGER,
  merge_commit TEXT
);

CREATE TABLE IF NOT EXISTS comments (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  issue_id   INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
  author_id  INTEGER REFERENCES users(id) ON DELETE SET NULL,
  body       TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS comments_issue ON comments(issue_id, created_at);
