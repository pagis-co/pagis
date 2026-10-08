-- The Coding Sessions (ADR-0033): one ACP session of one Coding Harness
-- that one Agent owns, and its transcript.
--
-- `coding_sessions` holds one row for each session, which the daemon
-- writes again whole as the session moves. `root_message_id` and
-- `message_id` have no foreign key, because the daemon writes the
-- record before the message of its block, as it writes a Call before
-- its strip.
--
-- `coding_session_events` holds the transcript: one row for each update
-- of the harness, numbered from 1 in each session. A chunk of a message
-- rewrites the payload of the last row while that row holds the same
-- message. `payload` is JSON text.
--
-- The Postgres migration of the same number holds the same tables.

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
    approval_mode TEXT NOT NULL CHECK (approval_mode IN ('person', 'agent', 'auto')),
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
    -- A Host session names its Host, and a Computer session names none.
    CHECK ((place = 'host') = (host_id IS NOT NULL)),
    -- Every session that ended has a reason and a time, and no other
    -- session has one.
    CHECK ((state IN ('closed', 'failed')) = (end_reason IS NOT NULL)),
    CHECK ((state IN ('closed', 'failed')) = (ended_at IS NOT NULL))
);
CREATE INDEX idx_coding_sessions_workspace ON coding_sessions (workspace_id, id);
CREATE INDEX idx_coding_sessions_agent_state ON coding_sessions (workspace_id, agent_id, state);

CREATE TABLE coding_session_events (
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    coding_session_id TEXT NOT NULL REFERENCES coding_sessions (id),
    seq INTEGER NOT NULL,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (
        kind IN (
            'prompt', 'agent_message', 'thought', 'tool_call', 'tool_call_update', 'plan',
            'usage', 'permission', 'decision', 'question', 'answer', 'turn_end'
        )
    ),
    payload TEXT NOT NULL,
    PRIMARY KEY (coding_session_id, seq)
);
