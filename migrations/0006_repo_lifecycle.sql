-- Each canonical or historical path belongs to one repository. In-progress
-- copies use the same namespace so they cannot race with creates or moves.
ALTER TABLE repos ADD COLUMN private_fork INTEGER NOT NULL DEFAULT 0
  CHECK (private_fork IN (0, 1) AND (private_fork = 0 OR visibility = 'private'));

CREATE TABLE repo_transfers (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id      INTEGER NOT NULL UNIQUE REFERENCES repos(id) ON DELETE CASCADE,
  sender_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  recipient_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at   INTEGER NOT NULL,
  CHECK (sender_id <> recipient_id)
);
CREATE INDEX repo_transfers_recipient ON repo_transfers(recipient_id, id DESC);

CREATE TABLE repo_copies (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  owner_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name          TEXT NOT NULL COLLATE NOCASE,
  description   TEXT NOT NULL,
  visibility    TEXT NOT NULL CHECK (visibility IN ('public', 'private')),
  private_fork  INTEGER NOT NULL CHECK (private_fork IN (0, 1)),
  artifacts_repo TEXT NOT NULL UNIQUE,
  kind          TEXT NOT NULL CHECK (kind IN ('fork', 'import')),
  source_id     INTEGER REFERENCES repos(id) ON DELETE SET NULL,
  repo_id       INTEGER REFERENCES repos(id) ON DELETE SET NULL,
  state         TEXT NOT NULL DEFAULT 'starting'
    CHECK (state IN ('starting', 'pending', 'discard', 'failed', 'ready')),
  created_at    INTEGER NOT NULL
);
CREATE INDEX repo_copies_source ON repo_copies(source_id);
CREATE INDEX repo_copies_result ON repo_copies(repo_id);

CREATE TABLE repo_paths (
  owner_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name     TEXT NOT NULL COLLATE NOCASE,
  repo_id  INTEGER REFERENCES repos(id) ON DELETE CASCADE,
  copy_id  INTEGER REFERENCES repo_copies(id) ON DELETE CASCADE,
  PRIMARY KEY (owner_id, name),
  CHECK ((repo_id IS NULL) <> (copy_id IS NULL))
) WITHOUT ROWID;
CREATE INDEX repo_paths_repo ON repo_paths(repo_id);
CREATE UNIQUE INDEX repo_paths_copy ON repo_paths(copy_id) WHERE copy_id IS NOT NULL;

-- Stop on ambiguous legacy paths instead of silently reassigning them.
INSERT INTO repo_paths (owner_id, name, repo_id) SELECT owner_id, name, id FROM repos;

-- Explicit guards keep outer INSERT/UPDATE OR IGNORE/REPLACE clauses from
-- skipping a reservation or replacing a path owned by a different repo.
CREATE TRIGGER repo_copy_reservation_guard BEFORE INSERT ON repo_copies
WHEN EXISTS (SELECT 1 FROM repo_paths WHERE owner_id = NEW.owner_id AND name = NEW.name)
BEGIN
  SELECT RAISE(ABORT, 'repository path is reserved');
END;

CREATE TRIGGER repo_path_insert_guard BEFORE INSERT ON repos
WHEN EXISTS (
  SELECT 1 FROM repo_paths p WHERE p.owner_id = NEW.owner_id AND p.name = NEW.name
    AND NOT EXISTS (SELECT 1 FROM repo_copies c WHERE c.id = p.copy_id
      AND c.artifacts_repo = NEW.artifacts_repo AND c.state = 'pending')
)
BEGIN
  SELECT RAISE(ABORT, 'repository path is reserved');
END;

CREATE TRIGGER repo_path_move_guard BEFORE UPDATE OF name, owner_id ON repos
WHEN EXISTS (
  SELECT 1 FROM repo_paths WHERE owner_id = NEW.owner_id AND name = NEW.name AND repo_id IS NOT OLD.id
)
BEGIN
  SELECT RAISE(ABORT, 'repository path is reserved');
END;

CREATE TRIGGER repo_copy_reserve AFTER INSERT ON repo_copies
BEGIN
  INSERT INTO repo_paths (owner_id, name, copy_id) VALUES (NEW.owner_id, NEW.name, NEW.id);
END;

CREATE TRIGGER repo_copy_release AFTER UPDATE OF state ON repo_copies
WHEN NEW.state = 'failed'
BEGIN
  DELETE FROM repo_paths WHERE copy_id = NEW.id;
END;

CREATE TRIGGER repo_path_insert AFTER INSERT ON repos
BEGIN
  -- A likely match by name is insufficient. Only the matching copy job's
  -- opaque Artifacts identity can promote a reservation to a repository.
  UPDATE repo_paths SET copy_id = NULL, repo_id = NEW.id
  WHERE owner_id = NEW.owner_id AND name = NEW.name AND copy_id = (
    SELECT id FROM repo_copies WHERE artifacts_repo = NEW.artifacts_repo AND state = 'pending'
  );
  INSERT INTO repo_paths (owner_id, name, repo_id)
  SELECT NEW.owner_id, NEW.name, NEW.id
  WHERE NOT EXISTS (
    SELECT 1 FROM repo_paths WHERE owner_id = NEW.owner_id AND name = NEW.name AND repo_id = NEW.id
  );
  UPDATE repo_copies SET state = 'ready', repo_id = NEW.id WHERE artifacts_repo = NEW.artifacts_repo
    AND owner_id = NEW.owner_id AND name = NEW.name AND state = 'pending';
END;

CREATE TRIGGER repo_path_move AFTER UPDATE OF name, owner_id ON repos
WHEN NEW.name <> OLD.name OR NEW.owner_id <> OLD.owner_id
BEGIN
  INSERT INTO repo_paths (owner_id, name, repo_id)
  SELECT NEW.owner_id, NEW.name, NEW.id
  WHERE NOT EXISTS (
    SELECT 1 FROM repo_paths WHERE owner_id = NEW.owner_id AND name = NEW.name AND repo_id = NEW.id
  );
END;

CREATE TRIGGER repo_transfer_complete AFTER UPDATE OF owner_id ON repos
WHEN NEW.owner_id <> OLD.owner_id
BEGIN
  DELETE FROM repo_transfers WHERE repo_id = NEW.id;
  DELETE FROM repo_collaborators WHERE repo_id = NEW.id AND user_id = NEW.owner_id;
END;

-- A source that becomes private while a copy is running pins that copy's
-- visibility even if the source subsequently goes public or is deleted.
CREATE TRIGGER repo_copy_pin_privacy AFTER UPDATE OF visibility ON repos
WHEN NEW.visibility = 'private' AND OLD.visibility <> 'private'
BEGIN
  UPDATE repo_copies SET private_fork = 1
  WHERE source_id = NEW.id AND state IN ('starting', 'pending');
END;

CREATE VIEW repo_copy_access AS
SELECT c.id, c.kind = 'import' OR (s.id IS NOT NULL AND (
  s.visibility = 'public' OR s.owner_id = c.owner_id OR u.is_admin = 1 OR EXISTS (
    SELECT 1 FROM repo_collaborators rc WHERE rc.repo_id = s.id AND rc.user_id = c.owner_id
  )
)) AS allowed
FROM repo_copies c LEFT JOIN repos s ON s.id = c.source_id JOIN users u ON u.id = c.owner_id;
