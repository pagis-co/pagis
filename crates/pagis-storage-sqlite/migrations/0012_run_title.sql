-- A title is fixed at creation. Rebuild the table to require it without
-- a default, and keep every reference to the original Run ids.
PRAGMA defer_foreign_keys = ON;

CREATE TABLE runs_with_title (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    agent_id TEXT NOT NULL REFERENCES agents (id),
    channel_id TEXT REFERENCES channels (id),
    root_message_id TEXT REFERENCES messages (id),
    trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('message', 'schedule', 'event', 'arrival', 'review')),
    trigger_ref TEXT,
    state TEXT NOT NULL CHECK (state IN (
        'queued', 'running', 'reflecting', 'waiting_for_user', 'waiting_for_approval',
        'completed', 'failed', 'canceled'
    )),
    error TEXT,
    started_at INTEGER,
    ended_at INTEGER,
    created_at INTEGER NOT NULL,
    -- Delegation hop guard: the count of agent-to-agent hops in
    -- the trigger chain behind a run. A user-triggered run has hop 0.
    hop_count INTEGER NOT NULL DEFAULT 0,
    -- The waiting conversation behind a delegation chain: the
    -- agent that owes an answer, and the channel and thread it owes it
    -- in. A run the user triggered starts the chain and has no origin.
    origin_agent_id TEXT,
    origin_channel_id TEXT,
    origin_root_message_id TEXT,
    failure_kind TEXT CHECK (failure_kind IN ('agent_missing', 'model_missing', 'context_failed', 'tool_failed', 'call_failed', 'access_changed', 'publication_rejected', 'lease_failed', 'daemon_restarted', 'model_failed', 'turn_limit', 'spend_cap_reached', 'unknown'))
    ,dismissed_at INTEGER
);

WITH RECURSIVE whitespace(chars) AS (
    VALUES (char(9, 11, 12, 13, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288))
), message_lines AS (
    SELECT r.id, trim(substr(COALESCE(m.text_content, ''), 1, CASE WHEN instr(COALESCE(m.text_content, ''), char(10)) = 0 THEN length(COALESCE(m.text_content, '')) ELSE instr(m.text_content, char(10)) - 1 END), (SELECT chars FROM whitespace)) AS line
    FROM runs r LEFT JOIN messages m ON m.id = r.trigger_ref AND m.workspace_id = r.workspace_id
), positions(n) AS (
    SELECT 1 UNION ALL SELECT n + 1 FROM positions WHERE n < 79
), message_titles AS (
    SELECT id, CASE
        WHEN line = '' THEN 'A message with an attachment'
        WHEN length(line) <= 80 THEN line
        ELSE rtrim(substr(line, 1, COALESCE((
            SELECT max(n) - 1 FROM positions WHERE instr((SELECT chars FROM whitespace), substr(line, n, 1)) > 0
        ), 79)), (SELECT chars FROM whitespace)) || '…'
    END AS title FROM message_lines
)
INSERT INTO runs_with_title (id, workspace_id, agent_id, channel_id, root_message_id, trigger_kind, trigger_ref, state, error, started_at, ended_at, created_at, hop_count, origin_agent_id, origin_channel_id, origin_root_message_id, failure_kind, dismissed_at, title)
SELECT r.id, r.workspace_id, r.agent_id, r.channel_id, r.root_message_id, r.trigger_kind, r.trigger_ref, r.state, r.error, r.started_at, r.ended_at, r.created_at, r.hop_count, r.origin_agent_id, r.origin_channel_id, r.origin_root_message_id, r.failure_kind, r.dismissed_at,
    CASE r.trigger_kind
    WHEN 'message' THEN mt.title
    WHEN 'arrival' THEN 'Bring a source into memory'
    WHEN 'review' THEN 'Review what was learned'
    ELSE COALESCE((SELECT rule_name FROM wakeups w WHERE w.id = r.trigger_ref AND w.workspace_id = r.workspace_id),
        (SELECT 'Call from ' || remote_e164 FROM calls c WHERE c.run_id = r.id AND c.direction = 'inbound'),
        'An event') END
FROM runs r JOIN message_titles mt ON mt.id = r.id;

DROP TABLE runs;
ALTER TABLE runs_with_title RENAME TO runs;
CREATE INDEX idx_runs_agent ON runs (agent_id, created_at);
CREATE INDEX idx_runs_channel ON runs (channel_id);


-- The parent rows are present again. Clear the deferred drop checks;
-- the migration test also checks the final foreign-key graph.
PRAGMA defer_foreign_keys = OFF;
