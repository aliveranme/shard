ALTER TABLE repos ADD COLUMN git_backend TEXT NOT NULL DEFAULT 'artifacts'
  CHECK (git_backend IN ('artifacts', 'native'));
ALTER TABLE repo_copies ADD COLUMN git_backend TEXT NOT NULL DEFAULT 'artifacts'
  CHECK (git_backend IN ('artifacts', 'native'));

-- Keep the existing storage IDs and include their provider in copy ownership checks.
DROP TRIGGER repo_path_insert_guard;
CREATE TRIGGER repo_path_insert_guard BEFORE INSERT ON repos
WHEN EXISTS (
  SELECT 1 FROM repo_paths p WHERE p.owner_id = NEW.owner_id AND p.name = NEW.name
    AND NOT EXISTS (SELECT 1 FROM repo_copies c WHERE c.id = p.copy_id
      AND c.artifacts_repo = NEW.artifacts_repo AND c.git_backend = NEW.git_backend AND c.state = 'pending')
)
BEGIN
  SELECT RAISE(ABORT, 'repository path is reserved');
END;

DROP TRIGGER repo_path_insert;
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
