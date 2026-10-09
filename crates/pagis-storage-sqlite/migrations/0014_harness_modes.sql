-- A Coding Session records its Harness Mode: the id of the current mode,
-- and the modes that the harness offered as JSON. A session of a harness
-- that answered no modes has no mode and an empty list. The transcript
-- takes the kind `mode` for each change of the mode (ADR-0033).
--
-- SQLite cannot change a check, so this migration makes
-- `coding_session_events` again and keeps its rows. The migrator runs the
-- file in a transaction with foreign keys on, and `PRAGMA foreign_keys`
-- does not change inside a transaction. Thus the foreign keys are deferred
-- to the commit. The table keeps its name: the rows are copied aside, the
-- table is dropped and made again with the same name, and the rows come
-- back.
--
-- The Postgres migration of the same number holds the same change.

PRAGMA defer_foreign_keys = ON;

ALTER TABLE coding_sessions ADD COLUMN harness_mode TEXT;
ALTER TABLE coding_sessions ADD COLUMN harness_modes TEXT NOT NULL DEFAULT '[]';

CREATE TABLE coding_session_events_rows AS SELECT * FROM coding_session_events;

DROP TABLE coding_session_events;

CREATE TABLE coding_session_events (
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    coding_session_id TEXT NOT NULL REFERENCES coding_sessions (id),
    seq INTEGER NOT NULL,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (
        kind IN (
            'prompt', 'agent_message', 'thought', 'tool_call', 'tool_call_update', 'plan',
            'usage', 'permission', 'decision', 'question', 'answer', 'turn_end', 'mode'
        )
    ),
    payload TEXT NOT NULL,
    PRIMARY KEY (coding_session_id, seq)
);

INSERT INTO coding_session_events (workspace_id, coding_session_id, seq, at, kind, payload)
SELECT workspace_id, coding_session_id, seq, at, kind, payload
FROM coding_session_events_rows;

DROP TABLE coding_session_events_rows;
