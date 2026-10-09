-- The Session Approval Mode of a Coding Session is `person` or `agent`
-- (ADR-0033). A session in the `auto` mode reads `person`, the narrowest
-- mode.
--
-- SQLite cannot change a check, so this migration makes `coding_sessions`
-- again and keeps its rows, its `model_token_hash` column and its indexes.
-- The migrator runs the file in a transaction with foreign keys on, and
-- `PRAGMA foreign_keys` does not change inside a transaction. Thus the
-- foreign keys are deferred to the commit. The table keeps its name: the
-- rows are copied aside, the table is dropped and made again with the same
-- name, and the rows come back. `coding_session_events`,
-- `event_subscriptions`, `source_batches` and `incoming_events` point at
-- the table by name, so their references stay correct, and the commit
-- checks every one.
--
-- The Postgres migration of the same number holds the same change.

PRAGMA defer_foreign_keys = ON;

UPDATE coding_sessions SET approval_mode = 'person' WHERE approval_mode = 'auto';

CREATE TABLE coding_sessions_rows AS SELECT * FROM coding_sessions;

DROP TABLE coding_sessions;

CREATE TABLE coding_sessions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    agent_id TEXT NOT NULL REFERENCES agents (id),
    harness_id TEXT NOT NULL,
    harness_version TEXT NOT NULL,
    place TEXT NOT NULL CHECK (place IN ('host', 'computer')),
    host_id TEXT REFERENCES hosts (id),
    directory TEXT NOT NULL,
    working_directory TEXT,
    worktree_branch TEXT,
    approval_mode TEXT NOT NULL CHECK (approval_mode IN ('person', 'agent')),
    title TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN (
            'starting', 'working', 'needs_decision', 'idle', 'interrupted', 'closed', 'failed'
        )
    ),
    end_reason TEXT,
    end_detail TEXT,
    acp_session_id TEXT,
    channel_id TEXT NOT NULL REFERENCES channels (id),
    root_message_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs (id),
    context_used INTEGER,
    context_size INTEGER,
    cost_amount REAL,
    cost_currency TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    ended_at INTEGER,
    model_token_hash TEXT,
    -- A Host session names its Host, and a Computer session names none.
    CHECK ((place = 'host') = (host_id IS NOT NULL)),
    -- Every session that ended has a reason and a time, and no other
    -- session has one.
    CHECK ((state IN ('closed', 'failed')) = (end_reason IS NOT NULL)),
    CHECK ((state IN ('closed', 'failed')) = (ended_at IS NOT NULL))
);
CREATE INDEX idx_coding_sessions_workspace ON coding_sessions (workspace_id, id);
CREATE INDEX idx_coding_sessions_agent_state ON coding_sessions (workspace_id, agent_id, state);
CREATE UNIQUE INDEX idx_coding_sessions_model_token ON coding_sessions (model_token_hash);

INSERT INTO coding_sessions (id, workspace_id, agent_id, harness_id, harness_version, place,
    host_id, directory, working_directory, worktree_branch, approval_mode, title, state,
    end_reason, end_detail, acp_session_id, channel_id, root_message_id, message_id, run_id,
    context_used, context_size, cost_amount, cost_currency, created_at, updated_at, ended_at,
    model_token_hash)
SELECT id, workspace_id, agent_id, harness_id, harness_version, place, host_id, directory,
    working_directory, worktree_branch, approval_mode, title, state, end_reason, end_detail,
    acp_session_id, channel_id, root_message_id, message_id, run_id, context_used, context_size,
    cost_amount, cost_currency, created_at, updated_at, ended_at, model_token_hash
FROM coding_sessions_rows;

DROP TABLE coding_sessions_rows;
