ALTER TABLE repos ADD COLUMN git_head_override INTEGER NOT NULL DEFAULT 0 CHECK (git_head_override IN (0, 1));

CREATE TABLE repo_branch_rules (
  repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  branch TEXT NOT NULL,
  approvals INTEGER NOT NULL CHECK (approvals BETWEEN 0 AND 20),
  checks TEXT NOT NULL CHECK (json_valid(checks) AND json_type(checks) = 'array'),
  PRIMARY KEY (repo_id, branch)
) WITHOUT ROWID;

CREATE TABLE merge_queue (
  issue_id INTEGER PRIMARY KEY REFERENCES pulls(issue_id) ON DELETE CASCADE,
  repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  base_branch TEXT NOT NULL,
  comparison_version INTEGER NOT NULL,
  actor_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
  state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'merged', 'failed')),
  reason TEXT,
  queued_at INTEGER NOT NULL,
  changed_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX merge_queue_running ON merge_queue(repo_id, base_branch) WHERE state = 'running';
CREATE INDEX merge_queue_pending ON merge_queue(changed_at, issue_id) WHERE state IN ('queued', 'running');
CREATE INDEX merge_queue_branch ON merge_queue(repo_id, base_branch, queued_at, issue_id) WHERE state IN ('queued', 'running');
