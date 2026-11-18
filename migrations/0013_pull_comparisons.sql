-- Keep the destination tip and the diff base separate. A NULL merge_base
-- preserves the old two-tip comparison for existing requests and reviews.
ALTER TABLE pulls ADD COLUMN merge_base TEXT CHECK (length(merge_base) = 40 AND merge_base NOT GLOB '*[^0-9a-f]*');
ALTER TABLE pulls ADD COLUMN comparison_version INTEGER NOT NULL DEFAULT 1 CHECK (comparison_version BETWEEN 1 AND 9007199254740991);
ALTER TABLE pull_reviews ADD COLUMN merge_base TEXT CHECK (length(merge_base) = 40 AND merge_base NOT GLOB '*[^0-9a-f]*');
ALTER TABLE pull_reviews ADD COLUMN comparison_version INTEGER NOT NULL DEFAULT 1 CHECK (comparison_version BETWEEN 1 AND 9007199254740991);
