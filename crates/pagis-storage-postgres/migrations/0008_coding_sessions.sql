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
-- The SQLite migration of the same number holds the same tables.

CREATE TABLE coding_sessions (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    harness_id TEXT COLLATE "C" NOT NULL,
    harness_version TEXT COLLATE "C" NOT NULL,
    place TEXT COLLATE "C" NOT NULL CHECK (place IN ('host', 'computer')),
    host_id TEXT COLLATE "C" REFERENCES hosts (id),
    directory TEXT COLLATE "C" NOT NULL,
    working_directory TEXT COLLATE "C",
    worktree_branch TEXT COLLATE "C",
    approval_mode TEXT COLLATE "C" NOT NULL CHECK (approval_mode IN ('person', 'agent', 'auto')),
    title TEXT COLLATE "C" NOT NULL,
    state TEXT COLLATE "C" NOT NULL CHECK (
        state IN (
            'starting', 'working', 'needs_decision', 'idle', 'interrupted', 'closed', 'failed'
        )
    ),
    end_reason TEXT COLLATE "C",
    end_detail TEXT COLLATE "C",
    acp_session_id TEXT COLLATE "C",
    channel_id TEXT COLLATE "C" NOT NULL REFERENCES channels (id),
    root_message_id TEXT COLLATE "C" NOT NULL,
    message_id TEXT COLLATE "C" NOT NULL,
    run_id TEXT COLLATE "C" NOT NULL REFERENCES runs (id),
    context_used BIGINT,
    context_size BIGINT,
    cost_amount DOUBLE PRECISION,
    cost_currency TEXT COLLATE "C",
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    ended_at BIGINT,
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
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    coding_session_id TEXT COLLATE "C" NOT NULL REFERENCES coding_sessions (id),
    seq BIGINT NOT NULL,
    at BIGINT NOT NULL,
    kind TEXT COLLATE "C" NOT NULL CHECK (
        kind IN (
            'prompt', 'agent_message', 'thought', 'tool_call', 'tool_call_update', 'plan',
            'usage', 'permission', 'decision', 'question', 'answer', 'turn_end'
        )
    ),
    payload TEXT COLLATE "C" NOT NULL,
    PRIMARY KEY (coding_session_id, seq)
);
