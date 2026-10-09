-- A Coding Session records its Harness Model and its thought level: each
-- one is the ACP Session Config Option of its category that the harness
-- offers, with the id of the option, the current choice and the choices,
-- as JSON. A session whose harness offers no such option has NULL
-- (ADR-0033).
--
-- The SQLite migration of the same number holds the same change.

ALTER TABLE coding_sessions ADD COLUMN model TEXT COLLATE "C";
ALTER TABLE coding_sessions ADD COLUMN thought_level TEXT COLLATE "C";
