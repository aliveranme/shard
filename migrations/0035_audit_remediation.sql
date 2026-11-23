-- Audit remediation (v0.5.2): email-link expiry index, drop redundant indexes,
-- tighten repo_copies_reconcile partial index, and preserve active R2 multipart
-- upload IDs for abort when a repository is deleted.

CREATE INDEX IF NOT EXISTS account_email_links_expiry
  ON account_email_links(expires_at, next_send_at);

DROP INDEX IF EXISTS repos_owner;
DROP INDEX IF EXISTS issues_repo_state;
DROP INDEX IF EXISTS organization_repo_grants_link;

DROP INDEX IF EXISTS repo_copies_reconcile;
CREATE INDEX IF NOT EXISTS repo_copies_reconcile
  ON repo_copies(COALESCE(last_checked_at, created_at), id)
  WHERE state IN ('starting', 'pending');

CREATE TABLE IF NOT EXISTS repo_lfs_multipart_aborts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id INTEGER NOT NULL,
  oid TEXT NOT NULL,
  attempt TEXT NOT NULL,
  upload_id TEXT NOT NULL
);

CREATE TRIGGER IF NOT EXISTS repo_delete_queue_lfs_aborts
BEFORE DELETE ON repos
BEGIN
  INSERT INTO repo_lfs_multipart_aborts (repo_id, oid, attempt, upload_id)
  SELECT repo_id, oid, attempt, upload_id
    FROM repo_lfs_multipart
   WHERE repo_id = OLD.id;
END;
