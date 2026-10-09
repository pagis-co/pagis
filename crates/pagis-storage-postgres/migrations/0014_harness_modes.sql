-- A Coding Session records its Harness Mode: the id of the current mode,
-- and the modes that the harness offered as JSON. A session of a harness
-- that answered no modes has no mode and an empty list. The transcript
-- takes the kind `mode` for each change of the mode (ADR-0033).
--
-- The SQLite migration of the same number holds the same change.

ALTER TABLE coding_sessions ADD COLUMN harness_mode TEXT COLLATE "C";
ALTER TABLE coding_sessions ADD COLUMN harness_modes TEXT COLLATE "C" NOT NULL DEFAULT '[]';

ALTER TABLE coding_session_events DROP CONSTRAINT coding_session_events_kind_check;
ALTER TABLE coding_session_events
    ADD CONSTRAINT coding_session_events_kind_check
    CHECK (
        kind IN (
            'prompt', 'agent_message', 'thought', 'tool_call', 'tool_call_update', 'plan',
            'usage', 'permission', 'decision', 'question', 'answer', 'turn_end', 'mode'
        )
    );
