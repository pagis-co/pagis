-- The Session Approval Mode of a Coding Session is `person` or `agent`
-- (ADR-0033). A session in the `auto` mode reads `person`, the narrowest
-- mode.
--
-- The SQLite migration of the same number holds the same change.

UPDATE coding_sessions SET approval_mode = 'person' WHERE approval_mode = 'auto';

ALTER TABLE coding_sessions DROP CONSTRAINT coding_sessions_approval_mode_check;
ALTER TABLE coding_sessions
    ADD CONSTRAINT coding_sessions_approval_mode_check
    CHECK (approval_mode IN ('person', 'agent'));
