-- The source of an Event Subscription, a Source Batch and an Incoming
-- Event is a Connection or a Coding Session (ADR-0006, ADR-0033). Each
-- row names exactly one of the two. An Incoming Event is unique by its
-- source, its kind and its provider event id.
--
-- SQLite cannot drop `NOT NULL` from a column, so this migration makes
-- the three tables again and keeps their rows. The migrator runs the file
-- in a transaction with foreign keys on, and `PRAGMA foreign_keys` does
-- not change inside a transaction. Thus the foreign keys are deferred to
-- the commit. Each table keeps its name: the rows are copied aside, the
-- table is dropped and made again with the same name, and the rows come
-- back. `wakeups` and `incoming_events` point at these tables by name,
-- so their references stay correct, and the commit checks every one.
--
-- The Postgres migration of the same number holds the same columns.

PRAGMA defer_foreign_keys = ON;

CREATE TABLE incoming_events_rows AS SELECT * FROM incoming_events;
CREATE TABLE source_batches_rows AS SELECT * FROM source_batches;
CREATE TABLE event_subscriptions_rows AS SELECT * FROM event_subscriptions;

DROP TABLE incoming_events;
DROP TABLE source_batches;
DROP TABLE event_subscriptions;

CREATE TABLE event_subscriptions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    agent_id TEXT NOT NULL REFERENCES agents (id),
    connection_id TEXT REFERENCES connections (id),
    coding_session_id TEXT REFERENCES coding_sessions (id),
    event_kind TEXT NOT NULL,
    source_version TEXT NOT NULL,
    name TEXT NOT NULL,
    instruction TEXT NOT NULL,
    channel_id TEXT NOT NULL REFERENCES channels (id),
    root_message_id TEXT REFERENCES messages (id),
    filter TEXT NOT NULL,
    creator TEXT NOT NULL CHECK (creator IN ('user', 'agent')),
    state TEXT NOT NULL CHECK (
        state IN ('active', 'paused', 'blocked', 'archived')
    ),
    revision INTEGER NOT NULL,
    approved_revision INTEGER,
    watermark_at INTEGER,
    -- Why a blocked rule is blocked. A grant gap never catches up; a
    -- Connection reauthorization does, once, from the kept cursor.
    blocked_reason TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    archived_at INTEGER,
    CHECK ((connection_id IS NULL) <> (coding_session_id IS NULL))
);
CREATE INDEX idx_subscriptions_workspace ON event_subscriptions (workspace_id, id DESC);
CREATE INDEX idx_subscriptions_source ON event_subscriptions (connection_id, event_kind, state);
CREATE INDEX idx_subscriptions_coding_session
    ON event_subscriptions (coding_session_id, event_kind, state);

CREATE TABLE source_batches (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    connection_id TEXT REFERENCES connections (id),
    coding_session_id TEXT REFERENCES coding_sessions (id),
    event_kind TEXT NOT NULL,
    collected_at INTEGER NOT NULL,
    collected_count INTEGER NOT NULL,
    stored_count INTEGER NOT NULL,
    wakeup_count INTEGER NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('baseline', 'collected', 'failed')),
    detail TEXT,
    CHECK ((connection_id IS NULL) <> (coding_session_id IS NULL))
);
CREATE INDEX idx_batches_source ON source_batches (connection_id, event_kind, id DESC);
CREATE INDEX idx_batches_coding_session
    ON source_batches (coding_session_id, event_kind, id DESC);

CREATE TABLE incoming_events (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    connection_id TEXT REFERENCES connections (id),
    coding_session_id TEXT REFERENCES coding_sessions (id),
    event_kind TEXT NOT NULL,
    provider_event_id TEXT NOT NULL,
    metadata TEXT NOT NULL,
    occurred_at INTEGER NOT NULL,
    received_at INTEGER NOT NULL,
    batch_id TEXT NOT NULL REFERENCES source_batches (id),
    UNIQUE (connection_id, event_kind, provider_event_id),
    UNIQUE (coding_session_id, event_kind, provider_event_id),
    CHECK ((connection_id IS NULL) <> (coding_session_id IS NULL))
);
CREATE INDEX idx_incoming_events_source ON incoming_events (connection_id, event_kind, id DESC);
CREATE INDEX idx_incoming_events_coding_session
    ON incoming_events (coding_session_id, event_kind, id DESC);

INSERT INTO event_subscriptions (id, workspace_id, agent_id, connection_id, event_kind,
    source_version, name, instruction, channel_id, root_message_id, filter, creator, state,
    revision, approved_revision, watermark_at, blocked_reason, created_at, updated_at,
    archived_at)
SELECT id, workspace_id, agent_id, connection_id, event_kind, source_version, name,
    instruction, channel_id, root_message_id, filter, creator, state, revision,
    approved_revision, watermark_at, blocked_reason, created_at, updated_at, archived_at
FROM event_subscriptions_rows;

INSERT INTO source_batches (id, workspace_id, connection_id, event_kind, collected_at,
    collected_count, stored_count, wakeup_count, outcome, detail)
SELECT id, workspace_id, connection_id, event_kind, collected_at, collected_count,
    stored_count, wakeup_count, outcome, detail
FROM source_batches_rows;

INSERT INTO incoming_events (id, workspace_id, connection_id, event_kind, provider_event_id,
    metadata, occurred_at, received_at, batch_id)
SELECT id, workspace_id, connection_id, event_kind, provider_event_id, metadata, occurred_at,
    received_at, batch_id
FROM incoming_events_rows;

DROP TABLE incoming_events_rows;
DROP TABLE source_batches_rows;
DROP TABLE event_subscriptions_rows;
