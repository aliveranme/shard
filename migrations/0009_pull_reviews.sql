-- Review records refer to the exact comparison presented to the reviewer.
CREATE TABLE pull_reviews (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  issue_id     INTEGER NOT NULL REFERENCES pulls(issue_id) ON DELETE CASCADE,
  actor_id     INTEGER REFERENCES users(id) ON DELETE SET NULL,
  decision     TEXT NOT NULL CHECK (decision IN ('approved', 'changes_requested', 'line_comment', 'merge_verified')),
  base_commit  TEXT NOT NULL,
  head_commit  TEXT NOT NULL,
  path         TEXT,
  side         TEXT,
  line         INTEGER,
  body         TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  CHECK ((decision = 'line_comment' AND path IS NOT NULL AND side IS NOT NULL AND side IN ('old', 'new') AND line IS NOT NULL AND line > 0)
         OR (decision <> 'line_comment' AND path IS NULL AND side IS NULL AND line IS NULL))
);
CREATE INDEX pull_reviews_issue ON pull_reviews(issue_id, created_at, id);
