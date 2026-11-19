-- Multipart receipts let a new Git LFS process resume an interrupted upload.
-- Object bytes remain in a private R2 staging prefix until SHA-256 is verified.
CREATE TABLE repo_lfs_multipart (
  repo_id    INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  oid        TEXT NOT NULL,
  size       INTEGER NOT NULL CHECK (size > 0),
  attempt    TEXT NOT NULL UNIQUE,
  upload_id  TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  PRIMARY KEY (repo_id, oid)
) WITHOUT ROWID;
CREATE INDEX repo_lfs_multipart_expiry ON repo_lfs_multipart(expires_at);

CREATE TABLE repo_lfs_parts (
  attempt     TEXT NOT NULL REFERENCES repo_lfs_multipart(attempt) ON DELETE CASCADE,
  part_number INTEGER NOT NULL CHECK (part_number BETWEEN 1 AND 10000),
  etag        TEXT NOT NULL,
  PRIMARY KEY (attempt, part_number)
) WITHOUT ROWID;
