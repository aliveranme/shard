-- Existing repositories keep lfs/<repo id>/<oid>. A fork reserves
-- lfs/fork-<repo_copies.id>/<oid> before it has a repository row.
ALTER TABLE repos ADD COLUMN lfs_copy_id INTEGER CHECK (lfs_copy_id > 0);
CREATE UNIQUE INDEX repos_lfs_copy ON repos(lfs_copy_id) WHERE lfs_copy_id IS NOT NULL;
CREATE INDEX repos_storage_identity ON repos(git_backend, artifacts_repo);

ALTER TABLE repo_copies ADD COLUMN lfs_cursor TEXT;
ALTER TABLE repo_copies ADD COLUMN lfs_complete INTEGER NOT NULL DEFAULT 0 CHECK (lfs_complete IN (0, 1));
ALTER TABLE repo_copies ADD COLUMN lfs_lease_until INTEGER NOT NULL DEFAULT 0;

-- Upload leases survive repository deletion, so a late PUT cannot race
-- reclamation of its prefix. Reconciliation prunes expired leases.
CREATE TABLE repo_lfs_uploads (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX repo_lfs_uploads_repo ON repo_lfs_uploads(repo_id, expires_at);
CREATE INDEX repo_lfs_uploads_expiry ON repo_lfs_uploads(expires_at);

-- No foreign keys: the storage can outlive its repository, copy, and owner.
CREATE TABLE repo_storage_cleanup (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id INTEGER UNIQUE,
  copy_id INTEGER UNIQUE,
  lfs_copy_id INTEGER,
  artifacts_repo TEXT NOT NULL,
  git_backend TEXT NOT NULL CHECK (git_backend IN ('artifacts', 'native')),
  git_deleted INTEGER NOT NULL DEFAULT 0 CHECK (git_deleted IN (0, 1)),
  due_at INTEGER NOT NULL DEFAULT (unixepoch()),
  CHECK ((repo_id IS NULL) <> (copy_id IS NULL))
);
CREATE INDEX repo_storage_cleanup_due ON repo_storage_cleanup(due_at, id);
CREATE INDEX repo_storage_cleanup_identity ON repo_storage_cleanup(git_backend, artifacts_repo);

CREATE TRIGGER repo_storage_cleanup_deleted AFTER DELETE ON repos
BEGIN
  INSERT INTO repo_storage_cleanup (repo_id, lfs_copy_id, artifacts_repo, git_backend)
  VALUES (OLD.id, OLD.lfs_copy_id, OLD.artifacts_repo, OLD.git_backend);
END;

CREATE TRIGGER repo_storage_cleanup_discard AFTER UPDATE OF state ON repo_copies
WHEN NEW.state = 'discard' AND OLD.state <> 'discard'
BEGIN
  INSERT INTO repo_storage_cleanup (copy_id, lfs_copy_id, artifacts_repo, git_backend)
  VALUES (NEW.id, CASE WHEN NEW.kind = 'fork' THEN NEW.id END, NEW.artifacts_repo, NEW.git_backend);
END;

CREATE TRIGGER repo_storage_cleanup_copy_deleted AFTER DELETE ON repo_copies
WHEN OLD.state IN ('starting', 'pending', 'discard')
BEGIN
  INSERT INTO repo_storage_cleanup (copy_id, lfs_copy_id, artifacts_repo, git_backend, due_at)
  VALUES (OLD.id, CASE WHEN OLD.kind = 'fork' THEN OLD.id END, OLD.artifacts_repo, OLD.git_backend,
          MAX(unixepoch(), OLD.lfs_lease_until,
              CASE WHEN OLD.state = 'starting' THEN OLD.created_at + 300 ELSE 0 END))
  ON CONFLICT (copy_id) DO UPDATE SET due_at = MAX(due_at, excluded.due_at);
END;

INSERT INTO repo_storage_cleanup (copy_id, lfs_copy_id, artifacts_repo, git_backend)
SELECT id, CASE WHEN kind = 'fork' THEN id END, artifacts_repo, git_backend
FROM repo_copies WHERE state = 'discard';
