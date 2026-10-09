-- A Run has a title, derived from its Trigger and fixed at creation
-- (ADR-0002). The column has no default, so each insert writes it.
--
-- SQLite cannot add a `NOT NULL` column without a default, so this
-- migration makes the table again and keeps its rows. The migrator runs
-- the file in a transaction with foreign keys on, and `PRAGMA
-- foreign_keys` does not change inside a transaction. Thus the foreign
-- keys are deferred to the commit. The table keeps its name: the rows and
-- their titles are copied aside, the table is dropped and made again with
-- the same name, and the rows come back. Many tables point at `runs` by
-- name, so their references stay correct, and the commit checks every
-- one.
--
-- The title rules are those of `pagis_core::run_title`. The Postgres
-- migration of the same number gives the same titles.

PRAGMA defer_foreign_keys = ON;

CREATE TABLE runs_rows AS
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
SELECT r.*,
    CASE r.trigger_kind
    WHEN 'message' THEN mt.title
    WHEN 'arrival' THEN 'Bring a source into memory'
    WHEN 'review' THEN 'Review what was learned'
    ELSE COALESCE((SELECT rule_name FROM wakeups w WHERE w.id = r.trigger_ref AND w.workspace_id = r.workspace_id),
        (SELECT 'Call from ' || remote_e164 FROM calls c WHERE c.run_id = r.id AND c.direction = 'inbound'),
        'An event') END AS title
FROM runs r JOIN message_titles mt ON mt.id = r.id;

DROP TABLE runs;

CREATE TABLE runs (
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
CREATE INDEX idx_runs_agent ON runs (agent_id, created_at);
CREATE INDEX idx_runs_channel ON runs (channel_id);

INSERT INTO runs (id, title, workspace_id, agent_id, channel_id, root_message_id, trigger_kind,
    trigger_ref, state, error, started_at, ended_at, created_at, hop_count, origin_agent_id,
    origin_channel_id, origin_root_message_id, failure_kind, dismissed_at)
SELECT id, title, workspace_id, agent_id, channel_id, root_message_id, trigger_kind, trigger_ref,
    state, error, started_at, ended_at, created_at, hop_count, origin_agent_id, origin_channel_id,
    origin_root_message_id, failure_kind, dismissed_at
FROM runs_rows;

DROP TABLE runs_rows;
