-- Index issue_labels by (repo_id, issue_id) so deleting an issue does not scan all labels in the repository.
CREATE INDEX IF NOT EXISTS issue_labels_repo_issue ON issue_labels(repo_id, issue_id);

-- Enforce the 350-character repository description limit at the database layer.
CREATE TRIGGER repos_description_length_insert BEFORE INSERT ON repos
WHEN length(NEW.description) > 350
BEGIN
  SELECT RAISE(ABORT, 'repository description too long');
END;

CREATE TRIGGER repos_description_length_update BEFORE UPDATE OF description ON repos
WHEN length(NEW.description) > 350
BEGIN
  SELECT RAISE(ABORT, 'repository description too long');
END;

CREATE TRIGGER repo_copies_description_length_insert BEFORE INSERT ON repo_copies
WHEN length(NEW.description) > 350
BEGIN
  SELECT RAISE(ABORT, 'repository description too long');
END;

CREATE TRIGGER repo_copies_description_length_update BEFORE UPDATE OF description ON repo_copies
WHEN length(NEW.description) > 350
BEGIN
  SELECT RAISE(ABORT, 'repository description too long');
END;
