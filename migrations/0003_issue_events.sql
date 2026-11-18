-- State transitions share the issue's lifetime, while deleted actors stay anonymous.
CREATE TABLE issue_events (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  issue_id   INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
  actor_id   INTEGER REFERENCES users(id) ON DELETE SET NULL,
  event      TEXT NOT NULL CHECK (event IN ('closed', 'reopened')),
  created_at INTEGER NOT NULL
);
CREATE INDEX issue_events_issue ON issue_events(issue_id, created_at, id);

-- Old closures have a known time but no recorded actor. Keep that attribution neutral.
INSERT INTO issue_events (issue_id, actor_id, event, created_at)
SELECT id, NULL, 'closed', closed_at FROM issues
WHERE state = 'closed' AND closed_at IS NOT NULL;
