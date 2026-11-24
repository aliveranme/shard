-- Repository descriptions are rendered on public listing pages. Bound new writes
-- in the application and truncate any oversized legacy values already stored.
UPDATE repos SET description = substr(description, 1, 350)
WHERE length(description) > 350;

UPDATE repo_copies SET description = substr(description, 1, 350)
WHERE length(description) > 350;
