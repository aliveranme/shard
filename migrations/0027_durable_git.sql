-- Replace the checked columns in place so foreign keys keep their parent tables.
DROP TRIGGER repo_path_insert_guard;
DROP TRIGGER repo_path_insert;
DROP TRIGGER repo_storage_cleanup_deleted;
DROP TRIGGER repo_storage_cleanup_discard;
DROP TRIGGER repo_storage_cleanup_copy_deleted;
DROP INDEX repos_storage_identity;

ALTER TABLE repos ADD COLUMN git_backend_new TEXT NOT NULL DEFAULT 'artifacts'
  CHECK (git_backend_new IN ('artifacts', 'native', 'durable'));
UPDATE repos SET git_backend_new = git_backend WHERE git_backend <> 'artifacts';
ALTER TABLE repos DROP COLUMN git_backend;
ALTER TABLE repos RENAME COLUMN git_backend_new TO git_backend;

ALTER TABLE repo_copies ADD COLUMN git_backend_new TEXT NOT NULL DEFAULT 'artifacts'
  CHECK (git_backend_new IN ('artifacts', 'native', 'durable'));
UPDATE repo_copies SET git_backend_new = git_backend WHERE git_backend <> 'artifacts';
ALTER TABLE repo_copies DROP COLUMN git_backend;
ALTER TABLE repo_copies RENAME COLUMN git_backend_new TO git_backend;

CREATE INDEX repos_storage_identity ON repos(git_backend, artifacts_repo);

-- The cleanup queue has no foreign keys and requires an explicit backend.
CREATE TABLE repo_storage_cleanup_new (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id INTEGER UNIQUE,
  copy_id INTEGER UNIQUE,
  lfs_copy_id INTEGER,
  artifacts_repo TEXT NOT NULL,
  git_backend TEXT NOT NULL CHECK (git_backend IN ('artifacts', 'native', 'durable')),
  git_deleted INTEGER NOT NULL DEFAULT 0 CHECK (git_deleted IN (0, 1)),
  due_at INTEGER NOT NULL DEFAULT (unixepoch()),
  CHECK ((repo_id IS NULL) <> (copy_id IS NULL))
);

-- Seed AUTOINCREMENT before copying rows, including when the queue is empty.
INSERT INTO repo_storage_cleanup_new (id, repo_id, artifacts_repo, git_backend)
SELECT seq, 0, '', 'artifacts' FROM sqlite_sequence
WHERE name = 'repo_storage_cleanup' AND seq > 0;
DELETE FROM repo_storage_cleanup_new;

INSERT INTO repo_storage_cleanup_new
  (id, repo_id, copy_id, lfs_copy_id, artifacts_repo, git_backend, git_deleted, due_at)
SELECT id, repo_id, copy_id, lfs_copy_id, artifacts_repo, git_backend, git_deleted, due_at
FROM repo_storage_cleanup;
DROP TABLE repo_storage_cleanup;
ALTER TABLE repo_storage_cleanup_new RENAME TO repo_storage_cleanup;
CREATE INDEX repo_storage_cleanup_due ON repo_storage_cleanup(due_at, id);
CREATE INDEX repo_storage_cleanup_identity ON repo_storage_cleanup(git_backend, artifacts_repo);

CREATE TRIGGER repo_path_insert_guard BEFORE INSERT ON repos
WHEN EXISTS (
  SELECT 1 FROM repo_paths p WHERE p.owner_id = NEW.owner_id AND p.name = NEW.name
    AND NOT EXISTS (SELECT 1 FROM repo_copies c WHERE c.id = p.copy_id
      AND c.artifacts_repo = NEW.artifacts_repo AND c.git_backend = NEW.git_backend AND c.state = 'pending')
)
BEGIN
  SELECT RAISE(ABORT, 'repository path is reserved');
END;

CREATE TRIGGER repo_path_insert AFTER INSERT ON repos
BEGIN
  UPDATE repo_paths SET copy_id = NULL, repo_id = NEW.id
  WHERE owner_id = NEW.owner_id AND name = NEW.name AND copy_id = (
    SELECT id FROM repo_copies WHERE artifacts_repo = NEW.artifacts_repo
      AND git_backend = NEW.git_backend AND state = 'pending'
  );
  INSERT INTO repo_paths (owner_id, name, repo_id)
  SELECT NEW.owner_id, NEW.name, NEW.id
  WHERE NOT EXISTS (
    SELECT 1 FROM repo_paths WHERE owner_id = NEW.owner_id AND name = NEW.name AND repo_id = NEW.id
  );
  UPDATE repo_copies SET state = 'ready', repo_id = NEW.id WHERE artifacts_repo = NEW.artifacts_repo
    AND git_backend = NEW.git_backend AND owner_id = NEW.owner_id AND name = NEW.name AND state = 'pending';
END;

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
