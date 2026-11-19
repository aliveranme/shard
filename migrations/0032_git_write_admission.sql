-- A branch rule cannot change while a previously authorized write can move it.
ALTER TABLE git_push_intents ADD COLUMN admitted_until INTEGER NOT NULL DEFAULT 0;
CREATE INDEX git_push_intents_admission ON git_push_intents(repo_id, admitted_until);

CREATE VIEW pending_branch_writes (repo_id, branch) AS
SELECT p.repo_id, substr(json_extract(u.value, '$.name'), 12)
FROM git_push_intents p, json_each(p.updates) u
WHERE p.admitted_until > unixepoch() AND json_extract(u.value, '$.name') GLOB 'refs/heads/*';
