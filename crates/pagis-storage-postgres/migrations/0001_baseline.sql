-- The Postgres schema of a Pagis installation (ADR-0024).
--
-- The SQLite baseline holds the same tables, columns, constraints and
-- triggers. A change to one baseline is a change to the other, and the
-- store-trait suite in pagis-testkit runs against both.
--
-- Differences from the SQLite schema, and why:
--   * A millisecond timestamp, a counter, a position, a revision and a
--     sequence are BIGINT. A 0/1 column is BOOLEAN. A BLOB is BYTEA.
--   * The SQLite dump has forward references. The foreign keys of
--     `workspaces` come last, after `agents`, `schedules` and `users`.
--   * The FTS5 virtual tables become ordinary tables with a tsvector
--     column, a GIN index and the `workspace_id` of the tenant.
--   * A SQLite trigger becomes a plpgsql function with the same name and
--     a trigger that calls it.
--   * Every TEXT column has the `C` collation. SQLite compares TEXT with
--     BINARY, which is byte order. A Postgres database under a locale
--     such as en_US.utf8 orders text by locale rules, and it then makes
--     case and punctuation secondary. One store-trait test suite runs
--     against both backends, and the keyset cursors and the list orders
--     the traits promise must be the same on each. The locale of the
--     database must not decide that order, so the column carries `C` and
--     every index over the column inherits it.

CREATE TABLE workspaces (
    id TEXT COLLATE "C" PRIMARY KEY,
    name TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    -- Onboarding completion: set once when the wizard finishes.
    onboarded_at BIGINT,
    timezone TEXT COLLATE "C" NOT NULL DEFAULT 'UTC',
    -- One Agent is the Chief of Staff of the Workspace (ADR-0022). The
    -- column is nullable: a Workspace with no active Agent has none.
    -- The foreign key comes after the agents table.
    chief_of_staff_agent_id TEXT COLLATE "C",
    -- The Schedule that makes the Chief of Staff write the Report for
    -- Home (ADR-0022). NULL until the user asks for one. The foreign key
    -- comes after the schedules table.
    report_schedule_id TEXT COLLATE "C",
    -- The person who owns the Workspace. NULL on the Org's Workspace
    -- (orgs.workspace_id), which no person owns. The foreign key comes
    -- after the users table.
    user_id TEXT COLLATE "C"
);

CREATE TABLE agents (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    name TEXT COLLATE "C" NOT NULL,
    job TEXT COLLATE "C" NOT NULL,
    personality TEXT COLLATE "C" NOT NULL,
    model_alias TEXT COLLATE "C" NOT NULL,
    status TEXT COLLATE "C" NOT NULL CHECK (status IN ('active', 'archived')),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    -- The Agent Voice (ADR-0020): one name from the catalogue the
    -- daemon knows, validated when it is written. NULL declares no
    -- voice, and the speech provider then uses its own default.
    voice TEXT COLLATE "C",
    -- The standing brief (ADR-0020): what an inbound call to the
    -- Agent's desk line is for, read at answer time. NULL declares none.
    standing_brief TEXT COLLATE "C",
    -- Sprite appearance. SQLite: CHECK (json_valid(avatar)).
    avatar TEXT COLLATE "C" NOT NULL
        DEFAULT '{"sprite":"pixie","preset":"mint","colors":{},"accessories":{}}'
        CHECK (jsonb_typeof(avatar::jsonb) IS NOT NULL),
    -- One line that says what to ask this Agent for. The other Agents
    -- read it in their staff line; the job stays the Agent's own label.
    description TEXT COLLATE "C" NOT NULL DEFAULT ''
);
CREATE INDEX idx_agents_workspace ON agents (workspace_id);

CREATE TABLE channels (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    title TEXT COLLATE "C",
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    -- Channel kinds: a DM triggers its agent on every user
    -- message; a group channel triggers only on @-mentions.
    kind TEXT COLLATE "C" NOT NULL DEFAULT 'dm' CHECK (kind IN ('dm', 'group'))
);
CREATE INDEX idx_channels_workspace ON channels (workspace_id);

CREATE TABLE channel_participants (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    channel_id TEXT COLLATE "C" NOT NULL REFERENCES channels (id),
    participant_kind TEXT COLLATE "C" NOT NULL CHECK (participant_kind IN ('user', 'agent')),
    agent_id TEXT COLLATE "C" REFERENCES agents (id),
    joined_at BIGINT NOT NULL
);
CREATE UNIQUE INDEX idx_channel_participants_unique
    ON channel_participants (channel_id, participant_kind, agent_id);

CREATE TABLE messages (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    channel_id TEXT COLLATE "C" NOT NULL REFERENCES channels (id),
    parent_message_id TEXT COLLATE "C" REFERENCES messages (id),
    author_kind TEXT COLLATE "C" NOT NULL CHECK (author_kind IN ('user', 'agent', 'system')),
    author_agent_id TEXT COLLATE "C" REFERENCES agents (id),
    run_id TEXT COLLATE "C",
    status TEXT COLLATE "C" NOT NULL CHECK (status IN ('streaming', 'complete', 'failed')),
    blocks TEXT COLLATE "C" NOT NULL,
    text_content TEXT COLLATE "C" NOT NULL,
    pending_id TEXT COLLATE "C",
    created_at BIGINT NOT NULL,
    completed_at BIGINT
);
CREATE INDEX idx_messages_channel ON messages (channel_id, created_at);
CREATE INDEX idx_messages_parent ON messages (parent_message_id);
CREATE UNIQUE INDEX idx_messages_pending
    ON messages (channel_id, pending_id)
    WHERE pending_id IS NOT NULL;

CREATE TABLE runs (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    channel_id TEXT COLLATE "C" REFERENCES channels (id),
    root_message_id TEXT COLLATE "C" REFERENCES messages (id),
    trigger_kind TEXT COLLATE "C" NOT NULL CHECK (trigger_kind IN ('message', 'schedule', 'event', 'arrival', 'review')),
    trigger_ref TEXT COLLATE "C",
    state TEXT COLLATE "C" NOT NULL CHECK (state IN (
        'queued', 'running', 'reflecting', 'waiting_for_user', 'waiting_for_approval',
        'completed', 'failed', 'canceled'
    )),
    error TEXT COLLATE "C",
    started_at BIGINT,
    ended_at BIGINT,
    created_at BIGINT NOT NULL,
    -- Delegation hop guard: the count of agent-to-agent hops in
    -- the trigger chain behind a run. A user-triggered run has hop 0.
    hop_count BIGINT NOT NULL DEFAULT 0,
    -- The waiting conversation behind a delegation chain: the
    -- agent that owes an answer, and the channel and thread it owes it
    -- in. A run the user triggered starts the chain and has no origin.
    origin_agent_id TEXT COLLATE "C",
    origin_channel_id TEXT COLLATE "C",
    origin_root_message_id TEXT COLLATE "C",
    failure_kind TEXT COLLATE "C" CHECK (failure_kind IN ('agent_missing', 'model_missing', 'context_failed', 'tool_failed', 'call_failed', 'access_changed', 'publication_rejected', 'lease_failed', 'daemon_restarted', 'model_failed', 'turn_limit', 'spend_cap_reached', 'unknown'))
);
CREATE INDEX idx_runs_agent ON runs (agent_id, created_at);
CREATE INDEX idx_runs_channel ON runs (channel_id);

CREATE TABLE events (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    event_type TEXT COLLATE "C" NOT NULL,
    agent_id TEXT COLLATE "C",
    run_id TEXT COLLATE "C",
    channel_id TEXT COLLATE "C",
    payload TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    -- SQLite reads the implicit rowid as `seq`. Postgres has no rowid, so
    -- the column is explicit. It is strictly increasing; a rolled-back
    -- append burns a number, and every reader treats `seq` as an opaque
    -- cursor, so the gap costs nothing.
    seq BIGINT GENERATED BY DEFAULT AS IDENTITY UNIQUE
);
CREATE INDEX idx_events_run ON events (run_id);
CREATE INDEX idx_events_workspace_created ON events (workspace_id, created_at);
-- The event bus reads one Workspace's events in seq order, so a socket
-- receives the events of its own tenant and no other.
CREATE INDEX idx_events_workspace_seq ON events (workspace_id, seq);

CREATE TABLE artifacts (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    creator_agent_id TEXT COLLATE "C" REFERENCES agents (id),
    run_id TEXT COLLATE "C" REFERENCES runs (id),
    filename TEXT COLLATE "C",
    mime TEXT COLLATE "C" NOT NULL,
    size_bytes BIGINT NOT NULL,
    sha256 TEXT COLLATE "C" NOT NULL,
    storage_key TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    -- One retention workflow for every artifact class. The class
    -- is a stored column, because the sweep must not guess a class from
    -- a filename.
    kind TEXT COLLATE "C" NOT NULL DEFAULT 'file'
);
CREATE UNIQUE INDEX idx_artifacts_dedup ON artifacts (workspace_id, sha256);

CREATE TABLE model_aliases (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    alias TEXT COLLATE "C" NOT NULL,
    candidates TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
CREATE UNIQUE INDEX idx_model_aliases_unique ON model_aliases (workspace_id, alias);

CREATE TABLE requests (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    run_id TEXT COLLATE "C" REFERENCES runs (id),
    kind TEXT COLLATE "C" NOT NULL,
    payload TEXT COLLATE "C" NOT NULL,
    state TEXT COLLATE "C" NOT NULL CHECK (
        state IN ('pending', 'approved', 'denied', 'expired', 'superseded')
    ),
    decided_at BIGINT,
    created_at BIGINT NOT NULL,
    -- The values a form or a choice submits with its decision.
    submitted_values TEXT COLLATE "C"
);

CREATE TABLE capability_snapshots (
    id TEXT COLLATE "C" PRIMARY KEY,
    hash TEXT COLLATE "C" NOT NULL UNIQUE,
    content TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL
);

CREATE TABLE run_capability_snapshots (
    run_id TEXT COLLATE "C" PRIMARY KEY REFERENCES runs (id),
    snapshot_id TEXT COLLATE "C" NOT NULL REFERENCES capability_snapshots (id)
);

CREATE TABLE credentials (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    -- The registrable domain, from the public suffix list. The
    -- registrable domain of login_url must equal it.
    domain TEXT COLLATE "C" NOT NULL,
    username TEXT COLLATE "C" NOT NULL,
    login_url TEXT COLLATE "C" NOT NULL,
    -- Sealed with ChaCha20-Poly1305 under the vault data key, which
    -- lives in the OS keychain. The database never holds plaintext.
    secret BYTEA NOT NULL,
    totp_seed BYTEA,
    -- The applied password recipe, in Apple's Password Rules grammar.
    recipe TEXT COLLATE "C" NOT NULL,
    -- The owning Agent, or NULL for the user. Archiving an Agent
    -- clears it and leaves the record usable.
    owner_agent_id TEXT COLLATE "C" REFERENCES agents (id),
    provenance TEXT COLLATE "C" NOT NULL CHECK (provenance IN ('user_supplied', 'agent_minted')),
    created_run_id TEXT COLLATE "C" REFERENCES runs (id),
    created_at BIGINT NOT NULL
);
CREATE INDEX idx_credentials_workspace_domain ON credentials (workspace_id, domain);
CREATE INDEX idx_requests_state ON requests (workspace_id, state, kind);

CREATE TABLE connections (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    provider TEXT COLLATE "C" NOT NULL,
    alias TEXT COLLATE "C" NOT NULL,
    display_name TEXT COLLATE "C" NOT NULL,
    status TEXT COLLATE "C" NOT NULL CHECK (
        status IN ('disconnected', 'connecting', 'connected', 'reauth_required', 'unavailable')
    ),
    auth_mode TEXT COLLATE "C" NOT NULL CHECK (auth_mode IN ('byo', 'brokered')),
    config TEXT COLLATE "C" NOT NULL DEFAULT '{}',
    created_at BIGINT NOT NULL,
    -- What the user authorized the account for, as a JSON array.
    authorized_capabilities TEXT COLLATE "C" NOT NULL DEFAULT '[]',
    -- The refresh token of a brokered Connection (ADR-0012). It arrives
    -- from Google at the callback and is sealed with the Tenant Data Key
    -- of the Workspace before it lands here, so no other tenant's key
    -- opens it.
    refresh_token BYTEA
);
CREATE UNIQUE INDEX idx_connections_alias ON connections (workspace_id, alias);

CREATE TABLE schedules (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    name TEXT COLLATE "C" NOT NULL,
    instruction TEXT COLLATE "C" NOT NULL,
    channel_id TEXT COLLATE "C" NOT NULL REFERENCES channels (id),
    root_message_id TEXT COLLATE "C" REFERENCES messages (id),
    kind TEXT COLLATE "C" NOT NULL CHECK (kind IN ('one_shot', 'cron', 'interval')),
    cron_expression TEXT COLLATE "C",
    interval_ms BIGINT,
    anchor_at BIGINT,
    timezone TEXT COLLATE "C" NOT NULL,
    scheduled_at BIGINT NOT NULL,
    next_due_at BIGINT,
    state TEXT COLLATE "C" NOT NULL CHECK (state IN ('active', 'paused', 'completed', 'blocked', 'archived')),
    revision BIGINT NOT NULL,
    approved_revision BIGINT,
    creator TEXT COLLATE "C" NOT NULL CHECK (creator IN ('user', 'agent')),
    creating_run_id TEXT COLLATE "C" REFERENCES runs (id),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    archived_at BIGINT,
    -- A reminder schedule anchored on forgotten knowledge stays
    -- archived and keyed as forgotten. The mark outlives the followup
    -- row that the purge deletes, so a queued run cannot start from the
    -- anchor later. SQLite: INTEGER with CHECK(forgotten IN (0,1)).
    forgotten BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX idx_schedules_due ON schedules (next_due_at) WHERE state = 'active';
CREATE INDEX idx_schedules_workspace ON schedules (workspace_id, id DESC);

CREATE TABLE schedule_revisions (
    schedule_id TEXT COLLATE "C" NOT NULL REFERENCES schedules (id),
    revision BIGINT NOT NULL,
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    name TEXT COLLATE "C" NOT NULL,
    instruction TEXT COLLATE "C" NOT NULL,
    channel_id TEXT COLLATE "C" NOT NULL REFERENCES channels (id),
    root_message_id TEXT COLLATE "C" REFERENCES messages (id),
    kind TEXT COLLATE "C" NOT NULL CHECK (kind IN ('one_shot', 'cron', 'interval')),
    cron_expression TEXT COLLATE "C",
    interval_ms BIGINT,
    anchor_at BIGINT,
    timezone TEXT COLLATE "C" NOT NULL,
    scheduled_at BIGINT NOT NULL,
    created_at BIGINT NOT NULL,
    creating_run_id TEXT COLLATE "C" REFERENCES runs (id),
    PRIMARY KEY (schedule_id, revision)
);

CREATE TABLE schedule_occurrences (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    schedule_id TEXT COLLATE "C" NOT NULL REFERENCES schedules (id),
    schedule_revision BIGINT NOT NULL,
    scheduled_at BIGINT NOT NULL,
    processed_at BIGINT NOT NULL,
    outcome TEXT COLLATE "C" NOT NULL CHECK (outcome IN ('wakeup_created', 'combined', 'skipped')),
    wakeup_id TEXT COLLATE "C",
    UNIQUE (schedule_id, scheduled_at)
);
CREATE INDEX idx_occurrences_schedule ON schedule_occurrences (schedule_id, id DESC);

CREATE TABLE event_subscriptions (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    connection_id TEXT COLLATE "C" NOT NULL REFERENCES connections (id),
    event_kind TEXT COLLATE "C" NOT NULL,
    source_version TEXT COLLATE "C" NOT NULL,
    name TEXT COLLATE "C" NOT NULL,
    instruction TEXT COLLATE "C" NOT NULL,
    channel_id TEXT COLLATE "C" NOT NULL REFERENCES channels (id),
    root_message_id TEXT COLLATE "C" REFERENCES messages (id),
    filter TEXT COLLATE "C" NOT NULL,
    creator TEXT COLLATE "C" NOT NULL CHECK (creator IN ('user', 'agent')),
    state TEXT COLLATE "C" NOT NULL CHECK (
        state IN ('active', 'paused', 'blocked', 'archived')
    ),
    revision BIGINT NOT NULL,
    approved_revision BIGINT,
    watermark_at BIGINT,
    -- Why a blocked rule is blocked. A grant gap never catches up; a
    -- Connection reauthorization does, once, from the kept cursor.
    blocked_reason TEXT COLLATE "C",
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    archived_at BIGINT
);
CREATE INDEX idx_subscriptions_workspace ON event_subscriptions (workspace_id, id DESC);
CREATE INDEX idx_subscriptions_source ON event_subscriptions (connection_id, event_kind, state);

CREATE TABLE provider_cursors (
    connection_id TEXT COLLATE "C" NOT NULL REFERENCES connections (id),
    event_kind TEXT COLLATE "C" NOT NULL,
    cursor TEXT COLLATE "C" NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (connection_id, event_kind)
);

CREATE TABLE source_batches (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    connection_id TEXT COLLATE "C" NOT NULL REFERENCES connections (id),
    event_kind TEXT COLLATE "C" NOT NULL,
    collected_at BIGINT NOT NULL,
    collected_count BIGINT NOT NULL,
    stored_count BIGINT NOT NULL,
    wakeup_count BIGINT NOT NULL,
    outcome TEXT COLLATE "C" NOT NULL CHECK (outcome IN ('baseline', 'collected', 'failed')),
    detail TEXT COLLATE "C"
);
CREATE INDEX idx_batches_source ON source_batches (connection_id, event_kind, id DESC);

CREATE TABLE incoming_events (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    connection_id TEXT COLLATE "C" NOT NULL REFERENCES connections (id),
    event_kind TEXT COLLATE "C" NOT NULL,
    provider_event_id TEXT COLLATE "C" NOT NULL,
    metadata TEXT COLLATE "C" NOT NULL,
    occurred_at BIGINT NOT NULL,
    received_at BIGINT NOT NULL,
    batch_id TEXT COLLATE "C" NOT NULL REFERENCES source_batches (id),
    UNIQUE (connection_id, event_kind, provider_event_id)
);
CREATE INDEX idx_incoming_events_source ON incoming_events (connection_id, event_kind, id DESC);

CREATE TABLE wakeups (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    source_kind TEXT COLLATE "C" NOT NULL CHECK (source_kind IN ('schedule', 'event_subscription', 'arrival')),
    schedule_id TEXT COLLATE "C" REFERENCES schedules (id),
    subscription_id TEXT COLLATE "C" REFERENCES event_subscriptions (id),
    rule_revision BIGINT NOT NULL,
    rule_name TEXT COLLATE "C" NOT NULL,
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    channel_id TEXT COLLATE "C" REFERENCES channels (id),
    root_message_id TEXT COLLATE "C" REFERENCES messages (id),
    instruction TEXT COLLATE "C" NOT NULL,
    scheduled_at BIGINT NOT NULL,
    state TEXT COLLATE "C" NOT NULL CHECK (state IN ('pending', 'started', 'withdrawn')),
    run_id TEXT COLLATE "C" UNIQUE REFERENCES runs (id),
    source_count BIGINT NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    started_at BIGINT,
    -- A synced arrival starts one reflection-only Run for its Subject
    -- Pages (ADR-0011).
    subject_paths TEXT COLLATE "C",
    -- An arrival Wake-up says whether its Subject Pages are historical,
    -- so the briefing can tell the Run (ADR-0011). SQLite: INTEGER.
    historical BOOLEAN NOT NULL DEFAULT false,
    CHECK (
        (source_kind = 'schedule' AND schedule_id IS NOT NULL AND subscription_id IS NULL AND subject_paths IS NULL AND channel_id IS NOT NULL)
        OR (source_kind = 'event_subscription' AND subscription_id IS NOT NULL AND schedule_id IS NULL AND subject_paths IS NULL AND channel_id IS NOT NULL)
        OR (source_kind = 'arrival' AND schedule_id IS NULL AND subscription_id IS NULL AND subject_paths IS NOT NULL AND channel_id IS NULL AND root_message_id IS NULL)
    )
);
CREATE INDEX idx_wakeups_pending ON wakeups (agent_id, scheduled_at, id) WHERE state = 'pending';
CREATE INDEX idx_wakeups_schedule ON wakeups (schedule_id, id DESC);
CREATE INDEX idx_wakeups_subscription ON wakeups (subscription_id, id DESC);
CREATE UNIQUE INDEX idx_wakeups_pending_schedule
    ON wakeups (schedule_id) WHERE state = 'pending' AND schedule_id IS NOT NULL;

CREATE TABLE wakeup_sources (
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    wakeup_id TEXT COLLATE "C" NOT NULL REFERENCES wakeups (id),
    source_kind TEXT COLLATE "C" NOT NULL CHECK (
        source_kind IN ('schedule_occurrence', 'incoming_event')
    ),
    source_id TEXT COLLATE "C" NOT NULL,
    PRIMARY KEY (wakeup_id, source_kind, source_id)
);
CREATE UNIQUE INDEX idx_wakeup_sources_occurrence
    ON wakeup_sources (source_id) WHERE source_kind = 'schedule_occurrence';
CREATE INDEX idx_wakeup_sources_event
    ON wakeup_sources (source_id) WHERE source_kind = 'incoming_event';

CREATE TABLE phone_numbers (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    -- The carrier that sold it. There is no foreign key: a released
    -- record is a tombstone for the Calls that point at it, and it
    -- outlives the Connection the user later deletes.
    connection_id TEXT COLLATE "C" NOT NULL,
    e164 TEXT COLLATE "C" NOT NULL,
    provider_number_id TEXT COLLATE "C" NOT NULL,
    agent_id TEXT COLLATE "C" REFERENCES agents (id),
    status TEXT COLLATE "C" NOT NULL CHECK (status IN ('assigned', 'unassigned', 'released')),
    created_at BIGINT NOT NULL,
    assigned_at BIGINT,
    -- The Outgoing Cap of a number and the rules that skip its send card
    -- (ADR-0020). They mirror the fields the Agent Mailbox record has.
    outgoing_cap BIGINT NOT NULL DEFAULT 50,
    allow_rules TEXT COLLATE "C" NOT NULL DEFAULT '[]',
    -- The day the send tally counts, and the tally. The cap is a day's
    -- allowance, so the tally starts again on a new day.
    sends_day BIGINT,
    sends_today BIGINT NOT NULL DEFAULT 0,
    -- Whether the carrier will deliver an outbound text (ADR-0020), when
    -- the daemon last read it, and the error of a read that failed.
    messaging_readiness TEXT COLLATE "C" NOT NULL DEFAULT 'unknown',
    messaging_readiness_reason TEXT COLLATE "C",
    messaging_readiness_at BIGINT,
    messaging_readiness_error TEXT COLLATE "C",
    -- Where the inbound collector reached, and the carrier's messaging
    -- object for this number (ADR-0020).
    text_cursor TEXT COLLATE "C",
    messaging_object_id TEXT COLLATE "C",
    -- The Telnyx relay function: where it stands, why a ship failed, the
    -- last lines the CLI wrote, and the user's consent to install the
    -- CLI (ADR-0020). SQLite: relay_consent INTEGER.
    relay_state TEXT COLLATE "C" NOT NULL DEFAULT 'absent',
    relay_reason TEXT COLLATE "C",
    relay_cli_lines TEXT COLLATE "C" NOT NULL DEFAULT '[]',
    relay_consent BOOLEAN NOT NULL DEFAULT false,
    -- A number is assigned exactly when an Agent holds it.
    CHECK ((status = 'assigned') = (agent_id IS NOT NULL))
);
CREATE UNIQUE INDEX idx_phone_numbers_agent
    ON phone_numbers (agent_id) WHERE agent_id IS NOT NULL;
-- One carrier serves the whole Org, so one number is held once in the
-- installation, whichever Workspace holds it.
CREATE UNIQUE INDEX idx_phone_numbers_e164
    ON phone_numbers (e164) WHERE status <> 'released';
CREATE INDEX idx_phone_numbers_workspace ON phone_numbers (workspace_id, status);
CREATE INDEX idx_phone_numbers_connection ON phone_numbers (connection_id, status);

CREATE TABLE phone_number_purchase_intents (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    connection_id TEXT COLLATE "C" NOT NULL,
    e164 TEXT COLLATE "C" NOT NULL,
    agent_id TEXT COLLATE "C" REFERENCES agents (id),
    state TEXT COLLATE "C" NOT NULL CHECK (state IN ('pending', 'bought', 'abandoned')),
    created_at BIGINT NOT NULL,
    settled_at BIGINT
);
CREATE UNIQUE INDEX idx_purchase_intents_pending
    ON phone_number_purchase_intents (workspace_id, e164) WHERE state = 'pending';

CREATE TABLE calls (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    -- The Run stays alive for the length of the call.
    run_id TEXT COLLATE "C" NOT NULL REFERENCES runs (id),
    -- No foreign key: a released number is a tombstone the Call still
    -- points at.
    phone_number_id TEXT COLLATE "C" NOT NULL,
    direction TEXT COLLATE "C" NOT NULL CHECK (direction IN ('outbound', 'inbound')),
    remote_e164 TEXT COLLATE "C" NOT NULL,
    tier TEXT COLLATE "C" NOT NULL CHECK (tier IN ('owner', 'trusted', 'unknown')),
    state TEXT COLLATE "C" NOT NULL CHECK (state IN ('dialing', 'live', 'ended')),
    outcome TEXT COLLATE "C" CHECK (
        outcome IN ('answered', 'no_answer', 'busy', 'voicemail', 'failed')
    ),
    ended_reason TEXT COLLATE "C",
    classification TEXT COLLATE "C" CHECK (
        classification IN (
            'human', 'machine-ivr', 'machine-vm', 'machine-unavailable', 'uncertain'
        )
    ),
    -- SQLite: INTEGER NOT NULL DEFAULT 0.
    message_left BOOLEAN NOT NULL DEFAULT false,
    transcript TEXT COLLATE "C" NOT NULL DEFAULT '',
    created_at BIGINT NOT NULL,
    ringing_at BIGINT,
    answered_at BIGINT,
    ended_at BIGINT,
    recording_artifact_id TEXT COLLATE "C" REFERENCES artifacts (id),
    -- The call brief: who called (the Agent and its own line), what the
    -- call was for, and which tools the call could use. The number
    -- record is a tombstone after a release, so own_e164 must live here.
    agent_name TEXT COLLATE "C" NOT NULL DEFAULT '',
    own_e164 TEXT COLLATE "C" NOT NULL DEFAULT '',
    purpose TEXT COLLATE "C" NOT NULL DEFAULT '',
    -- The tool names, as a JSON array.
    tools TEXT COLLATE "C" NOT NULL DEFAULT '[]',
    -- Every Call that ended has a reason (ADR-0020).
    CHECK ((state = 'ended') = (ended_reason IS NOT NULL))
);
CREATE INDEX idx_calls_workspace ON calls (workspace_id, created_at);
CREATE INDEX idx_calls_number ON calls (phone_number_id, state);
CREATE INDEX idx_calls_run ON calls (run_id);
CREATE INDEX idx_artifacts_retention ON artifacts (workspace_id, kind, created_at);

CREATE TABLE artifact_retention_policies (
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    kind TEXT COLLATE "C" NOT NULL CHECK (
        kind IN ('screenshot', 'call_recording', 'call_transcript', 'file')
    ),
    retain_days BIGINT NOT NULL CHECK (retain_days > 0),
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (workspace_id, kind)
);

CREATE TABLE software_packages (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    -- The namespace the tools take. The first publish claims it.
    name TEXT COLLATE "C" NOT NULL,
    -- Only this Agent publishes later Versions. Another Agent forks.
    author_agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    description TEXT COLLATE "C" NOT NULL,
    -- The manifest keywords, as a JSON array of strings.
    keywords TEXT COLLATE "C" NOT NULL,
    -- The tag of the newest Version: `v1`, `v2` and so on.
    latest_version TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    -- A Fork is a Software Package that started as a copy of one
    -- Version of another one. The fork's first publish writes the
    -- origin; the two columns never change after that.
    origin_package_id TEXT COLLATE "C" REFERENCES software_packages (id),
    origin_version TEXT COLLATE "C"
);
CREATE UNIQUE INDEX idx_software_packages_name
    ON software_packages (workspace_id, name);

CREATE TABLE software_versions (
    package_id TEXT COLLATE "C" NOT NULL REFERENCES software_packages (id),
    version TEXT COLLATE "C" NOT NULL,
    notes TEXT COLLATE "C" NOT NULL,
    commit_id TEXT COLLATE "C" NOT NULL,
    -- The manifest and the argument schemas, as JSON.
    manifest TEXT COLLATE "C" NOT NULL,
    published_at BIGINT NOT NULL,
    run_id TEXT COLLATE "C" NOT NULL REFERENCES runs (id),
    PRIMARY KEY (package_id, version)
);
CREATE INDEX idx_software_versions_package
    ON software_versions (package_id, published_at);

CREATE TABLE contributions (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    -- The origin package the change is offered to.
    package_id TEXT COLLATE "C" NOT NULL REFERENCES software_packages (id),
    -- The Version the patch is against.
    base_version TEXT COLLATE "C" NOT NULL,
    -- The latest Version of the origin package when the record opened.
    latest_at_open TEXT COLLATE "C" NOT NULL,
    fork_package_id TEXT COLLATE "C" NOT NULL REFERENCES software_packages (id),
    fork_version TEXT COLLATE "C" NOT NULL,
    -- The unified diff between the two trees.
    patch TEXT COLLATE "C" NOT NULL,
    summary TEXT COLLATE "C" NOT NULL,
    -- open, merged or declined.
    status TEXT COLLATE "C" NOT NULL,
    outcome_reason TEXT COLLATE "C",
    created_at BIGINT NOT NULL,
    closed_at BIGINT,
    run_id TEXT COLLATE "C" NOT NULL REFERENCES runs (id)
);
CREATE INDEX idx_contributions_fork
    ON contributions (fork_package_id, created_at);
CREATE INDEX idx_contributions_package
    ON contributions (package_id, created_at);

CREATE TABLE plugins (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    -- The name of `plugin.json`. It is the tool namespace.
    name TEXT COLLATE "C" NOT NULL,
    -- The address the files came from, as JSON: a git URL with an
    -- optional ref, or an upload.
    source TEXT COLLATE "C" NOT NULL,
    installed_commit TEXT COLLATE "C" NOT NULL,
    -- The Capability Manifest version this state produced: `v1`, `v2`
    -- and so on.
    manifest_version TEXT COLLATE "C" NOT NULL,
    -- `enabled`, `disabled` or `failed`.
    state TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
CREATE UNIQUE INDEX idx_plugins_name ON plugins (workspace_id, name);

CREATE TABLE plugin_bindings (
    plugin_id TEXT COLLATE "C" NOT NULL REFERENCES plugins (id),
    field TEXT COLLATE "C" NOT NULL,
    -- The bound Connection, secret name or plain value, as JSON.
    value TEXT COLLATE "C" NOT NULL,
    PRIMARY KEY (plugin_id, field)
);

CREATE TABLE grants (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    resource_kind TEXT COLLATE "C" NOT NULL
        CHECK (resource_kind IN ('connection', 'credential', 'host', 'plugin')),
    resource_id TEXT COLLATE "C",
    scope TEXT COLLATE "C" NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    revoked_at BIGINT
);
CREATE UNIQUE INDEX idx_grants_live_unique
    ON grants (workspace_id, agent_id, resource_kind, resource_id)
    WHERE revoked_at IS NULL;
-- A host grant names the machine it reaches: one live grant for each
-- of the person's machines, and no two for one machine (ADR-0015).
CREATE UNIQUE INDEX idx_grants_live_host_unique
    ON grants (agent_id, resource_id)
    WHERE revoked_at IS NULL AND resource_kind = 'host';

CREATE TABLE agent_mailboxes (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    -- The holder stays on the tombstone, so the ledger says who held
    -- the address.
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    -- The Mailbox Provider that carries it. There is no foreign key: a
    -- tombstone outlives the Connection the user later removes.
    connection_id TEXT COLLATE "C" NOT NULL,
    address TEXT COLLATE "C" NOT NULL,
    state TEXT COLLATE "C" NOT NULL CHECK (
        state IN ('provisioning', 'active', 'unavailable', 'dormant', 'deleted')
    ),
    -- Why the mailbox is unavailable, in the words the desk shows.
    reason TEXT COLLATE "C",
    outgoing_cap BIGINT NOT NULL,
    -- Where the collector reached. The three columns are one cursor,
    -- so they are present together or absent together.
    cursor_folder TEXT COLLATE "C",
    cursor_uid_validity BIGINT,
    cursor_last_uid BIGINT,
    -- The day the send tally counts, and the tally. The Outgoing Cap is
    -- a day's allowance, so the tally starts again on a new day.
    sends_day BIGINT,
    sends_today BIGINT NOT NULL DEFAULT 0,
    created_at BIGINT NOT NULL,
    deleted_at BIGINT,
    -- The allow rules the user wrote from a send card, as a JSON array
    -- of registrable domains. They sit on the record and not in a Grant,
    -- because an Agent holds its own mailbox with no Grant.
    allow_rules TEXT COLLATE "C" NOT NULL DEFAULT '[]',
    -- A mailbox is deleted exactly when it has a deleted time.
    CHECK ((state = 'deleted') = (deleted_at IS NOT NULL)),
    CHECK (
        (cursor_folder IS NULL) = (cursor_uid_validity IS NULL)
        AND (cursor_folder IS NULL) = (cursor_last_uid IS NULL)
    )
);
CREATE UNIQUE INDEX idx_agent_mailboxes_address ON agent_mailboxes (address);
CREATE UNIQUE INDEX idx_agent_mailboxes_agent
    ON agent_mailboxes (agent_id) WHERE state <> 'deleted';
CREATE INDEX idx_agent_mailboxes_connection ON agent_mailboxes (connection_id, state);
CREATE INDEX idx_agent_mailboxes_workspace ON agent_mailboxes (workspace_id, state);

CREATE TABLE plugin_tools (
    plugin_id TEXT COLLATE "C" NOT NULL REFERENCES plugins (id),
    -- `v1`, `v2` and so on.
    version TEXT COLLATE "C" NOT NULL,
    installed_commit TEXT COLLATE "C" NOT NULL,
    -- The frozen tools, as a JSON array.
    tools TEXT COLLATE "C" NOT NULL,
    -- Whether a server has offered a different list since the freeze.
    -- SQLite: INTEGER NOT NULL DEFAULT 0.
    tools_changed BOOLEAN NOT NULL DEFAULT false,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (plugin_id, version)
);

CREATE TABLE sent_mail (
    message_id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    -- No foreign key: the row outlives the mailbox the user deletes.
    mailbox_id TEXT COLLATE "C" NOT NULL,
    run_id TEXT COLLATE "C" NOT NULL REFERENCES runs (id),
    channel_id TEXT COLLATE "C",
    -- The message the Run's conversation is rooted at, or NULL when the
    -- Run was not in a Thread.
    thread_id TEXT COLLATE "C",
    sent_at BIGINT NOT NULL
);
CREATE INDEX idx_sent_mail_mailbox ON sent_mail (mailbox_id, sent_at);
CREATE UNIQUE INDEX idx_wakeups_pending_subscription
    ON wakeups (subscription_id, channel_id, COALESCE(root_message_id, ''))
    WHERE state = 'pending' AND subscription_id IS NOT NULL;

CREATE TABLE trust_entries (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    -- NULL for the Workspace-wide list.
    agent_id TEXT COLLATE "C" REFERENCES agents (id),
    -- What the value names. A domain covers every address at it.
    subject TEXT COLLATE "C" NOT NULL CHECK (subject IN ('number', 'address', 'domain')),
    -- The E.164 number, the bare address or the bare domain, in the one
    -- form the daemon stores.
    value TEXT COLLATE "C" NOT NULL,
    -- A list proposes a tier above Unknown; Unknown is what a subject on
    -- no list gets, so it is not a row.
    tier TEXT COLLATE "C" NOT NULL CHECK (tier IN ('owner', 'trusted')),
    label TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE UNIQUE INDEX idx_trust_entries_workspace_row
    ON trust_entries (workspace_id, subject, value) WHERE agent_id IS NULL;
CREATE UNIQUE INDEX idx_trust_entries_agent_row
    ON trust_entries (agent_id, subject, value) WHERE agent_id IS NOT NULL;
CREATE INDEX idx_trust_entries_lookup
    ON trust_entries (workspace_id, subject, value);

-- The failed-attempt count of each Workspace's Keypad Code (ADR-0021). A
-- Workspace with no row has no failure and no delay, so a clear deletes
-- the row.
CREATE TABLE keypad_failures (
    workspace_id TEXT COLLATE "C" PRIMARY KEY REFERENCES workspaces (id),
    -- The wrong codes on every Call and every line of the Workspace.
    failed_attempts BIGINT NOT NULL CHECK (failed_attempts >= 0),
    -- The end of the latest delay, or NULL before the first delay.
    suspended_until BIGINT,
    updated_at BIGINT NOT NULL
);

CREATE TABLE sync_resources (
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces(id),
    connection_id TEXT COLLATE "C" NOT NULL,
    resource TEXT COLLATE "C" NOT NULL,
    -- SQLite: CHECK(json_valid(config)).
    config TEXT COLLATE "C" NOT NULL CHECK (jsonb_typeof(config::jsonb) IS NOT NULL),
    revision BIGINT NOT NULL DEFAULT 1,
    -- SQLite: CHECK(json_valid(checkpoint)).
    checkpoint TEXT COLLATE "C" NOT NULL DEFAULT 'null'
        CHECK (jsonb_typeof(checkpoint::jsonb) IS NOT NULL),
    -- SQLite: INTEGER NOT NULL DEFAULT 0.
    caught_up BOOLEAN NOT NULL DEFAULT false,
    updated_at BIGINT NOT NULL,
    acquisition_error TEXT COLLATE "C",
    arrival_error TEXT COLLATE "C",
    cursor_revision BIGINT NOT NULL DEFAULT 1,
    -- The filter is part of the resource config. A change to it bumps
    -- filter_revision, and the reflection records of each revision stay
    -- beside the records of the next one.
    filter_revision BIGINT NOT NULL DEFAULT 1,
    PRIMARY KEY(workspace_id, connection_id, resource)
);

CREATE TABLE text_records (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    -- The record stays with the Agent whatever happens to the number,
    -- so a new holder of the number sees none of it.
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    -- No foreign key: a released number is a tombstone the record
    -- still points at, as a Call does.
    phone_number_id TEXT COLLATE "C" NOT NULL,
    counterpart_e164 TEXT COLLATE "C" NOT NULL,
    direction TEXT COLLATE "C" NOT NULL CHECK (direction IN ('outbound', 'inbound')),
    tier TEXT COLLATE "C" NOT NULL CHECK (tier IN ('owner', 'trusted', 'unknown')),
    body TEXT COLLATE "C" NOT NULL,
    segments BIGINT NOT NULL,
    -- The MMS media, as a JSON array of Artifact ids.
    media_artifact_ids TEXT COLLATE "C" NOT NULL DEFAULT '[]',
    carrier_message_id TEXT COLLATE "C" NOT NULL,
    -- The Run, the Channel and the Thread of an outbound text, as the
    -- sent_mail row holds them for a mail.
    run_id TEXT COLLATE "C" REFERENCES runs (id),
    channel_id TEXT COLLATE "C",
    thread_id TEXT COLLATE "C",
    delivery_state TEXT COLLATE "C" CHECK (
        delivery_state IN ('queued', 'sent', 'delivered', 'failed')
    ),
    delivery_code TEXT COLLATE "C",
    delivery_reason TEXT COLLATE "C",
    -- When the text was received or sent.
    occurred_at BIGINT NOT NULL,
    -- Only an outbound text has a Run that sent it and a delivery
    -- state; an inbound one has neither.
    CHECK ((direction = 'outbound') = (run_id IS NOT NULL)),
    CHECK ((direction = 'outbound') = (delivery_state IS NOT NULL))
);
CREATE INDEX idx_text_records_conversation
    ON text_records (agent_id, phone_number_id, counterpart_e164, occurred_at, id);
CREATE INDEX idx_text_records_workspace ON text_records (workspace_id, occurred_at);

CREATE TABLE message_exposure_sets (
    message_id TEXT COLLATE "C" PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE
);

CREATE TABLE message_exposures (
    message_id TEXT COLLATE "C" NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    grant_id TEXT COLLATE "C" NOT NULL,
    grant_revision BIGINT NOT NULL,
    PRIMARY KEY(message_id, grant_id)
);

CREATE TABLE source_versions (
    -- SQLite: INTEGER PRIMARY KEY AUTOINCREMENT.
    sequence BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL,
    connection_id TEXT COLLATE "C" NOT NULL,
    resource TEXT COLLATE "C" NOT NULL,
    id TEXT COLLATE "C" NOT NULL,
    version TEXT COLLATE "C" NOT NULL,
    operation TEXT COLLATE "C" NOT NULL,
    -- SQLite: CHECK(json_valid(record)).
    record TEXT COLLATE "C" NOT NULL CHECK (jsonb_typeof(record::jsonb) IS NOT NULL),
    observed_at BIGINT NOT NULL,
    -- SQLite: INTEGER NOT NULL.
    historical BOOLEAN NOT NULL,
    -- SQLite: CHECK(arrival IS NULL OR json_valid(arrival)).
    arrival TEXT COLLATE "C" CHECK (arrival IS NULL OR jsonb_typeof(arrival::jsonb) IS NOT NULL),
    arrival_ack_at BIGINT,
    -- One arrival fails alone. Its attempts, its next retry time, its
    -- outcome and its last failure stay on the acquisition row (ADR-0011).
    arrival_attempts BIGINT NOT NULL DEFAULT 0,
    arrival_retry_at BIGINT NOT NULL DEFAULT 0,
    arrival_outcome TEXT COLLATE "C" CHECK (arrival_outcome IN ('appended', 'skipped')),
    arrival_failure TEXT COLLATE "C",
    UNIQUE(workspace_id, connection_id, resource, id, version, operation),
    FOREIGN KEY(workspace_id, connection_id, resource) REFERENCES sync_resources(workspace_id, connection_id, resource)
);
CREATE INDEX source_arrivals ON source_versions(workspace_id, connection_id, resource, arrival_ack_at, sequence);

CREATE TABLE knowledge_generations (
    workspace_id TEXT COLLATE "C" NOT NULL,
    connection_id TEXT COLLATE "C" NOT NULL,
    resource TEXT COLLATE "C" NOT NULL,
    generation BIGINT NOT NULL DEFAULT 0,
    -- SQLite: INTEGER NOT NULL DEFAULT 0.
    rebuilding BOOLEAN NOT NULL DEFAULT false,
    PRIMARY KEY(workspace_id, connection_id, resource)
);

CREATE TABLE knowledge_invalidations (
    -- SQLite: INTEGER PRIMARY KEY AUTOINCREMENT.
    id BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL,
    connection_id TEXT COLLATE "C" NOT NULL,
    resource TEXT COLLATE "C" NOT NULL,
    reason TEXT COLLATE "C" NOT NULL,
    -- SQLite: INTEGER NOT NULL DEFAULT 0.
    published BOOLEAN NOT NULL DEFAULT false
);

CREATE TABLE forget_operations (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces(id),
    -- SQLite: CHECK(json_valid(target)).
    target TEXT COLLATE "C" NOT NULL CHECK (jsonb_typeof(target::jsonb) IS NOT NULL),
    phase TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    error TEXT COLLATE "C",
    reopted_at BIGINT,
    -- SQLite: CHECK(purge_plan IS NULL OR json_valid(purge_plan)).
    purge_plan TEXT COLLATE "C" CHECK (purge_plan IS NULL OR jsonb_typeof(purge_plan::jsonb) IS NOT NULL),
    -- SQLite: INTEGER NOT NULL DEFAULT 0.
    structured_cleaned BOOLEAN NOT NULL DEFAULT false,
    connection_id TEXT COLLATE "C" NOT NULL DEFAULT ''
);

CREATE TABLE forget_suppressions (
    operation_id TEXT COLLATE "C" NOT NULL REFERENCES forget_operations(id),
    workspace_id TEXT COLLATE "C" NOT NULL,
    connection_id TEXT COLLATE "C" NOT NULL,
    resource TEXT COLLATE "C" NOT NULL,
    identity TEXT COLLATE "C" NOT NULL,
    scope TEXT COLLATE "C" NOT NULL DEFAULT 'source' CHECK(scope IN ('source','account')),
    PRIMARY KEY(operation_id,identity)
);
CREATE INDEX forget_suppression_lookup ON forget_suppressions(workspace_id,connection_id,resource,identity);

CREATE TABLE forgotten_messages (
    message_id TEXT COLLATE "C" PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE
);

CREATE TABLE memory_brief_cursors (
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents (id),
    channel_id TEXT COLLATE "C" NOT NULL REFERENCES channels (id),
    root_message_id TEXT COLLATE "C" NOT NULL DEFAULT '',
    memory_revision TEXT COLLATE "C",
    shown_paths TEXT COLLATE "C" NOT NULL,
    PRIMARY KEY (workspace_id, agent_id, channel_id, root_message_id)
);

CREATE TABLE schedule_subject_pages (
    schedule_id TEXT COLLATE "C" PRIMARY KEY REFERENCES schedules (id) ON DELETE CASCADE,
    path TEXT COLLATE "C" NOT NULL
);
CREATE INDEX idx_schedule_subject_pages_path ON schedule_subject_pages (path, schedule_id);

CREATE TABLE source_items (
    workspace_id TEXT COLLATE "C" NOT NULL,
    connection_id TEXT COLLATE "C" NOT NULL,
    resource TEXT COLLATE "C" NOT NULL,
    id TEXT COLLATE "C" NOT NULL,
    version TEXT COLLATE "C" NOT NULL,
    -- SQLite: CHECK(json_valid(record)).
    record TEXT COLLATE "C" NOT NULL CHECK (jsonb_typeof(record::jsonb) IS NOT NULL),
    observed_at BIGINT NOT NULL,
    PRIMARY KEY(workspace_id, connection_id, resource, id),
    FOREIGN KEY(workspace_id, connection_id, resource)
        REFERENCES sync_resources(workspace_id, connection_id, resource)
);

-- SQLite: json_extract(record, '$.parent'). An expression index element
-- needs its own parentheses in Postgres.
CREATE INDEX source_versions_parent ON source_versions(
    workspace_id, connection_id, resource, ((record::jsonb) ->> 'parent'));

CREATE TABLE page_reflections (
    workspace_id TEXT COLLATE "C" NOT NULL,
    connection_id TEXT COLLATE "C" NOT NULL,
    resource TEXT COLLATE "C" NOT NULL,
    page_path TEXT COLLATE "C" NOT NULL,
    -- The source subject of the page, such as a mail thread id. The
    -- backfill walks subjects, so it reads the decision by subject.
    page_subject TEXT COLLATE "C" NOT NULL,
    filter_revision BIGINT NOT NULL,
    decided_by TEXT COLLATE "C" NOT NULL CHECK(decided_by IN ('live', 'backfill')),
    verdict TEXT COLLATE "C" NOT NULL CHECK(verdict IN ('reflect', 'skip')),
    rule_index BIGINT,
    reason TEXT COLLATE "C" NOT NULL,
    -- The arrival Wake-up that reflects the page, and the moment the
    -- backfill started it. A live decision leaves both empty: the live
    -- pass starts its own Run.
    wakeup_id TEXT COLLATE "C" REFERENCES wakeups (id),
    reflected_at BIGINT,
    decided_at BIGINT NOT NULL,
    -- A page counts the Runs that failed to reflect it, so a poisoned
    -- batch stops after the attempt limit (ADR-0011).
    attempts BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY(workspace_id, connection_id, resource, page_path, filter_revision),
    FOREIGN KEY(workspace_id, connection_id, resource)
        REFERENCES sync_resources(workspace_id, connection_id, resource)
);
CREATE INDEX page_reflections_backfill ON page_reflections(
    workspace_id, connection_id, resource, filter_revision, decided_by);

-- SQLite: COALESCE(json_extract(arrival, '$.metadata.thread_id'),
-- json_extract(arrival, '$.provider_event_id')) and historical = 1.
CREATE INDEX source_versions_arrival_subject ON source_versions(
    workspace_id, connection_id, resource,
    (COALESCE((arrival::jsonb) #>> '{metadata,thread_id}',
              (arrival::jsonb) ->> 'provider_event_id')))
    WHERE arrival IS NOT NULL AND historical;

CREATE TABLE memory_page_index_heads (
    workspace_id TEXT COLLATE "C" PRIMARY KEY REFERENCES workspaces (id),
    revision TEXT COLLATE "C" NOT NULL,
    next_position BIGINT NOT NULL
);

CREATE TABLE memory_page_index (
    -- SQLite: id INTEGER PRIMARY KEY, the implicit rowid key.
    id BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    path TEXT COLLATE "C" NOT NULL,
    position BIGINT NOT NULL,
    changed_at BIGINT NOT NULL,
    changed_by_name TEXT COLLATE "C" NOT NULL,
    changed_by_email TEXT COLLATE "C" NOT NULL,
    title TEXT COLLATE "C" NOT NULL,
    kind TEXT COLLATE "C",
    source_connection_id TEXT COLLATE "C",
    -- The exposure stamp as JSON. NULL is a file with no stamp, which
    -- no reader sees.
    exposures TEXT COLLATE "C",
    -- NULL is a file that is not a Subject Page with a valid layout. The
    -- words are in order, with one space between two.
    brief_words TEXT COLLATE "C",
    UNIQUE (workspace_id, path)
);

-- Memory Search is derived data beside the Page Index (ADR-0008).
-- SQLite uses an FTS5 virtual table over the path, the title and the
-- body. Here the three columns stay beside a tsvector that the Rust
-- insert writes, with the tenant of the row. The weights stand for the
-- bm25 column weights: title A, path B, body D.
CREATE TABLE memory_page_search (
    id BIGINT PRIMARY KEY REFERENCES memory_page_index (id) ON DELETE CASCADE,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    path TEXT COLLATE "C" NOT NULL,
    title TEXT COLLATE "C" NOT NULL,
    body TEXT COLLATE "C" NOT NULL,
    document tsvector NOT NULL
);
CREATE INDEX memory_page_search_document ON memory_page_search USING gin (document);
CREATE INDEX memory_page_search_workspace ON memory_page_search (workspace_id);

CREATE TABLE onboarding_model_verifications (
    workspace_id TEXT COLLATE "C" PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    provider TEXT COLLATE "C" NOT NULL,
    available BIGINT NOT NULL,
    proof TEXT COLLATE "C" NOT NULL
);

CREATE TABLE pending_evidence (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces(id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents(id),
    channel_id TEXT COLLATE "C" NOT NULL REFERENCES channels(id),
    root_message_id TEXT COLLATE "C" REFERENCES messages(id),
    subject TEXT COLLATE "C" NOT NULL,
    after_exclusive TEXT COLLATE "C",
    through_inclusive TEXT COLLATE "C" NOT NULL REFERENCES messages(id),
    reason TEXT COLLATE "C" NOT NULL,
    urgency TEXT COLLATE "C" NOT NULL CHECK (urgency IN ('normal', 'urgent')),
    eligible_at BIGINT NOT NULL,
    maximum_due_at BIGINT NOT NULL,
    attempt_count BIGINT NOT NULL DEFAULT 0,
    state TEXT COLLATE "C" NOT NULL CHECK (state IN ('pending', 'leased', 'completed', 'failed', 'invalidated')),
    revision BIGINT NOT NULL DEFAULT 1,
    lease_run_id TEXT COLLATE "C" REFERENCES runs(id),
    leased_at BIGINT,
    lease_expires_at BIGINT,
    error TEXT COLLATE "C",
    memory_revision TEXT COLLATE "C",
    completed_at BIGINT,
    failed_overlap_id TEXT COLLATE "C" REFERENCES pending_evidence(id),
    created_at BIGINT NOT NULL
);
CREATE INDEX idx_pending_evidence_due
    ON pending_evidence(state, urgency, eligible_at, maximum_due_at, created_at, id);
CREATE UNIQUE INDEX idx_pending_evidence_open_subject
    ON pending_evidence(workspace_id, agent_id, subject)
    WHERE state = 'pending';

CREATE TABLE pending_evidence_messages (
    pending_id TEXT COLLATE "C" NOT NULL REFERENCES pending_evidence(id) ON DELETE CASCADE,
    message_id TEXT COLLATE "C" NOT NULL REFERENCES messages(id),
    channel_id TEXT COLLATE "C" NOT NULL REFERENCES channels(id),
    root_message_id TEXT COLLATE "C" REFERENCES messages(id),
    root_key TEXT COLLATE "C" NOT NULL,
    PRIMARY KEY (pending_id, message_id)
);

CREATE TABLE pending_evidence_exposures (
    pending_id TEXT COLLATE "C" NOT NULL REFERENCES pending_evidence(id) ON DELETE CASCADE,
    grant_id TEXT COLLATE "C" NOT NULL REFERENCES grants(id),
    grant_revision BIGINT NOT NULL,
    PRIMARY KEY (pending_id, grant_id, grant_revision)
);

CREATE TABLE pending_review_cursors (
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces(id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents(id),
    channel_id TEXT COLLATE "C" NOT NULL REFERENCES channels(id),
    root_message_id TEXT COLLATE "C",
    root_key TEXT COLLATE "C" NOT NULL,
    subject TEXT COLLATE "C" NOT NULL,
    through_inclusive TEXT COLLATE "C" NOT NULL REFERENCES messages(id),
    memory_revision TEXT COLLATE "C",
    reviewed_at BIGINT NOT NULL,
    PRIMARY KEY (workspace_id, agent_id, channel_id, root_key, subject)
);

-- SQLite uses an FTS5 virtual table over the text of a message. The
-- triggers below keep this table in step, and they write the tenant and
-- the tsvector.
CREATE TABLE conversation_message_search (
    message_id TEXT COLLATE "C" PRIMARY KEY REFERENCES messages (id) ON DELETE CASCADE,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    text_content TEXT COLLATE "C" NOT NULL,
    document tsvector NOT NULL
);
CREATE INDEX conversation_message_search_document
    ON conversation_message_search USING gin (document);
CREATE INDEX conversation_message_search_workspace
    ON conversation_message_search (workspace_id);

CREATE TABLE conversation_tool_evidence (
    reference TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces(id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents(id),
    channel_id TEXT COLLATE "C" NOT NULL REFERENCES channels(id),
    root_message_id TEXT COLLATE "C" REFERENCES messages(id),
    source_message_id TEXT COLLATE "C" NOT NULL REFERENCES messages(id),
    run_id TEXT COLLATE "C" NOT NULL REFERENCES runs(id),
    tool_call_id TEXT COLLATE "C" NOT NULL,
    tool_name TEXT COLLATE "C" NOT NULL,
    content TEXT COLLATE "C" NOT NULL,
    -- SQLite: INTEGER NOT NULL CHECK(complete IN (0, 1)).
    complete BOOLEAN NOT NULL,
    grant_id TEXT COLLATE "C" NOT NULL REFERENCES grants(id),
    grant_revision BIGINT NOT NULL,
    created_at BIGINT NOT NULL,
    UNIQUE(run_id, tool_call_id)
);
CREATE INDEX conversation_tool_evidence_scope
ON conversation_tool_evidence(workspace_id, agent_id, channel_id, root_message_id, source_message_id);

-- SQLite uses an FTS5 virtual table over the content of a tool result.
CREATE TABLE conversation_tool_search (
    reference TEXT COLLATE "C" PRIMARY KEY REFERENCES conversation_tool_evidence (reference) ON DELETE CASCADE,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    content TEXT COLLATE "C" NOT NULL,
    document tsvector NOT NULL
);
CREATE INDEX conversation_tool_search_document
    ON conversation_tool_search USING gin (document);
CREATE INDEX conversation_tool_search_workspace
    ON conversation_tool_search (workspace_id);

-- The synced source content that a Run read (ADR-0008): one Source
-- Item, or every item of one parent, such as the messages of one mail
-- thread. A Forget of an item forgets each message of a Run that read
-- it, and its purge deletes the rows that name the item or its parent.
CREATE TABLE run_source_reads (
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    run_id TEXT COLLATE "C" NOT NULL REFERENCES runs (id),
    connection_id TEXT COLLATE "C" NOT NULL,
    resource TEXT COLLATE "C" NOT NULL,
    kind TEXT COLLATE "C" NOT NULL CHECK (kind IN ('item', 'parent')),
    source_id TEXT COLLATE "C" NOT NULL,
    PRIMARY KEY (run_id, connection_id, resource, kind, source_id)
);
CREATE INDEX run_source_reads_source
    ON run_source_reads (workspace_id, connection_id, resource, source_id);

CREATE TABLE continuation_checkpoints (
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces(id),
    agent_id TEXT COLLATE "C" NOT NULL REFERENCES agents(id),
    channel_id TEXT COLLATE "C" NOT NULL REFERENCES channels(id),
    root_message_id TEXT COLLATE "C" REFERENCES messages(id),
    root_key TEXT COLLATE "C" NOT NULL,
    revision BIGINT NOT NULL CHECK(revision > 0),
    after_exclusive TEXT COLLATE "C",
    through_inclusive TEXT COLLATE "C" NOT NULL REFERENCES messages(id),
    -- SQLite: CHECK(json_valid(state)).
    state TEXT COLLATE "C" NOT NULL CHECK (jsonb_typeof(state::jsonb) IS NOT NULL),
    created_at BIGINT NOT NULL,
    PRIMARY KEY(workspace_id, agent_id, channel_id, root_key),
    CHECK(root_key = coalesce(root_message_id, ''))
);

CREATE TABLE continuation_checkpoint_messages (
    workspace_id TEXT COLLATE "C" NOT NULL,
    agent_id TEXT COLLATE "C" NOT NULL,
    channel_id TEXT COLLATE "C" NOT NULL,
    root_key TEXT COLLATE "C" NOT NULL,
    message_id TEXT COLLATE "C" NOT NULL REFERENCES messages(id),
    PRIMARY KEY(workspace_id, agent_id, channel_id, root_key, message_id),
    FOREIGN KEY(workspace_id, agent_id, channel_id, root_key)
        REFERENCES continuation_checkpoints(workspace_id, agent_id, channel_id, root_key)
        ON DELETE CASCADE
);

CREATE TABLE continuation_checkpoint_exposures (
    workspace_id TEXT COLLATE "C" NOT NULL,
    agent_id TEXT COLLATE "C" NOT NULL,
    channel_id TEXT COLLATE "C" NOT NULL,
    root_key TEXT COLLATE "C" NOT NULL,
    grant_id TEXT COLLATE "C" NOT NULL REFERENCES grants(id),
    grant_revision BIGINT NOT NULL,
    PRIMARY KEY(workspace_id, agent_id, channel_id, root_key, grant_id, grant_revision),
    FOREIGN KEY(workspace_id, agent_id, channel_id, root_key)
        REFERENCES continuation_checkpoints(workspace_id, agent_id, channel_id, root_key)
        ON DELETE CASCADE
);

CREATE TABLE memory_page_link (
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    -- The repository path of the page that holds the link.
    source_path TEXT COLLATE "C" NOT NULL,
    -- The repository path the link names.
    target_path TEXT COLLATE "C" NOT NULL,
    -- Where the link stands in the page, counted from zero. A reader
    -- orders on it: a page links in the order it names its pages, and
    -- the one hop of a Brief takes the first of them. Without it the
    -- order of a read is arbitrary and the Brief is not repeatable.
    position BIGINT NOT NULL,
    PRIMARY KEY (workspace_id, source_path, target_path)
);
CREATE INDEX memory_page_link_target
    ON memory_page_link (workspace_id, target_path);

CREATE TABLE orgs (
    id TEXT COLLATE "C" PRIMARY KEY,
    name TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    -- The Google Web OAuth client of the installation (ADR-0012). Every
    -- person consents against it. Its secret is an installation secret
    -- in the secret file, not a column.
    google_client_id TEXT COLLATE "C",
    -- The Workspace that holds the Org's own records: the installed
    -- Plugins and the Installation Connections. No person owns it, so
    -- its user_id is NULL and no Session reaches it.
    workspace_id TEXT COLLATE "C" NOT NULL UNIQUE REFERENCES workspaces (id)
);

CREATE TABLE users (
    id TEXT COLLATE "C" PRIMARY KEY,
    org_id TEXT COLLATE "C" NOT NULL REFERENCES orgs (id),
    -- NULL for the seeded person of a local installation, who signs in
    -- with the Client Credential.
    email TEXT COLLATE "C",
    name TEXT COLLATE "C",
    -- The argon2id hash. NULL means this person has no password.
    password_hash TEXT COLLATE "C",
    role TEXT COLLATE "C" NOT NULL CHECK (role IN ('administrator', 'member')),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    -- An account the Administrator manages: it can be disabled and
    -- given back, and a disabled account keeps its Workspace.
    disabled_at BIGINT,
    -- When a client last signed in as the Person.
    last_signed_in_at BIGINT,
    -- The monthly Spend Cap the Administrator sets. NULL sets no cap.
    -- SQLite: REAL.
    monthly_spend_cap_usd DOUBLE PRECISION
);
CREATE UNIQUE INDEX idx_users_email ON users (email);
CREATE INDEX idx_users_org ON users (org_id);

CREATE TABLE sessions (
    id TEXT COLLATE "C" PRIMARY KEY,
    user_id TEXT COLLATE "C" NOT NULL REFERENCES users (id),
    token_hash TEXT COLLATE "C" NOT NULL UNIQUE,
    client_kind TEXT COLLATE "C" NOT NULL CHECK (client_kind IN ('browser', 'desktop')),
    client_name TEXT COLLATE "C",
    created_at BIGINT NOT NULL,
    last_used_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions (user_id);
CREATE INDEX idx_sessions_expires ON sessions (expires_at);

CREATE TABLE sign_in_links (
    id TEXT COLLATE "C" PRIMARY KEY,
    user_id TEXT COLLATE "C" NOT NULL REFERENCES users (id),
    token_hash TEXT COLLATE "C" NOT NULL UNIQUE,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    used_at BIGINT
);
CREATE INDEX idx_workspaces_user ON workspaces (user_id);

-- The Usage Record: one row for each model call, with the token counts
-- of the provider and the cost the price table of the serving model
-- makes of them. It names the Workspace that spent it and the Run that
-- asked. A NULL cost is an unknown cost: no layer of the model metadata
-- prices the model. SQLite: cost_usd REAL.
CREATE TABLE usage (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    run_id TEXT COLLATE "C" NOT NULL REFERENCES runs (id),
    provider TEXT COLLATE "C",
    model TEXT COLLATE "C",
    input_tokens BIGINT NOT NULL,
    output_tokens BIGINT NOT NULL,
    cache_read_tokens BIGINT NOT NULL,
    cache_write_tokens BIGINT NOT NULL,
    cost_usd DOUBLE PRECISION,
    created_at BIGINT NOT NULL
);
-- The two reads: one Workspace over a period, and every Workspace over
-- a period. Both walk the period, so the timestamp leads the second
-- index.
CREATE INDEX idx_usage_workspace_created ON usage (workspace_id, created_at);
CREATE INDEX idx_usage_created ON usage (created_at);
CREATE INDEX idx_usage_run ON usage (run_id);

-- The machines a person's sprites act on (ADR-0015). A Host is a
-- client of one Person, and the Workspace column is how it belongs to
-- that Person. The name is the identity of the machine in the
-- Workspace, so a Grant that names it survives a restart of the client.
-- Presence is memory of the running daemon; this table holds only the
-- last time the machine was seen. The daemon is never a Host.
CREATE TABLE hosts (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    name TEXT COLLATE "C" NOT NULL,
    platform TEXT COLLATE "C" NOT NULL,
    -- The declared capabilities, as a JSON array of strings.
    capabilities TEXT COLLATE "C" NOT NULL,
    last_seen_at BIGINT NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE UNIQUE INDEX idx_hosts_workspace_name ON hosts (workspace_id, name);

-- The three foreign keys of the workspaces table. SQLite declares them
-- in the CREATE TABLE and accepts the forward reference; Postgres does
-- not, so they are added here.
ALTER TABLE workspaces ADD CONSTRAINT workspaces_chief_of_staff_agent_id_fkey
    FOREIGN KEY (chief_of_staff_agent_id) REFERENCES agents (id);
ALTER TABLE workspaces ADD CONSTRAINT workspaces_report_schedule_id_fkey
    FOREIGN KEY (report_schedule_id) REFERENCES schedules (id);
ALTER TABLE workspaces ADD CONSTRAINT workspaces_user_id_fkey
    FOREIGN KEY (user_id) REFERENCES users (id);

-- Triggers. A SQLite `CREATE TRIGGER ... BEGIN ... END` becomes a
-- plpgsql function with the name of the trigger, and a trigger that
-- calls it. `RETURN NULL` is correct for an AFTER row trigger.

-- A change of Connection access makes every sync resource of the
-- Connection catch up again.
CREATE FUNCTION knowledge_connection_access() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE sync_resources SET caught_up = false, revision = revision + 1
    WHERE connection_id = new.id AND workspace_id = new.workspace_id;
    RETURN NULL;
END $$;
CREATE TRIGGER knowledge_connection_access
AFTER UPDATE OF status, authorized_capabilities ON connections
FOR EACH ROW
WHEN (old.status IS DISTINCT FROM new.status
      OR old.authorized_capabilities IS DISTINCT FROM new.authorized_capabilities)
EXECUTE FUNCTION knowledge_connection_access();

-- A new or changed Connection grant makes the sync resources of the
-- Agent that holds it catch up again.
CREATE FUNCTION knowledge_grant_insert() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE sync_resources SET caught_up = false, revision = revision + 1
    WHERE connection_id = new.resource_id AND workspace_id = new.workspace_id
    AND (config::jsonb ->> 'agent_id') = new.agent_id;
    RETURN NULL;
END $$;
CREATE TRIGGER knowledge_grant_insert
AFTER INSERT ON grants
FOR EACH ROW
WHEN (new.resource_kind = 'connection')
EXECUTE FUNCTION knowledge_grant_insert();

CREATE FUNCTION knowledge_grant_update() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE sync_resources SET caught_up = false, revision = revision + 1
    WHERE connection_id = new.resource_id AND workspace_id = new.workspace_id
    AND (config::jsonb ->> 'agent_id') = new.agent_id;
    RETURN NULL;
END $$;
CREATE TRIGGER knowledge_grant_update
AFTER UPDATE OF scope, revoked_at ON grants
FOR EACH ROW
WHEN (new.resource_kind = 'connection'
      AND (old.scope IS DISTINCT FROM new.scope
           OR old.revoked_at IS DISTINCT FROM new.revoked_at))
EXECUTE FUNCTION knowledge_grant_update();

-- The words of the user carry no source scope, so a user message gets
-- an empty exposure set when it lands.
CREATE FUNCTION message_exposure_set_insert() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO message_exposure_sets(message_id) VALUES (new.id);
    RETURN NULL;
END $$;
CREATE TRIGGER message_exposure_set_insert
AFTER INSERT ON messages
FOR EACH ROW
WHEN (new.author_kind = 'user')
EXECUTE FUNCTION message_exposure_set_insert();

-- A message whose exposure reaches a forgotten source is forgotten too.
-- SQLite: INSERT OR IGNORE.
CREATE FUNCTION forget_message_exposure() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO forgotten_messages
    SELECT m.id FROM messages m
    WHERE m.id=new.message_id AND EXISTS(
        SELECT 1 FROM forget_suppressions f
        WHERE f.workspace_id=m.workspace_id AND (
            NOT EXISTS(SELECT 1 FROM grants g
                WHERE g.workspace_id=m.workspace_id AND g.id=new.grant_id)
            OR EXISTS(SELECT 1 FROM grants g
                WHERE g.workspace_id=m.workspace_id AND g.id=new.grant_id
                AND g.resource_kind='connection' AND g.resource_id=f.connection_id)
        )
    )
    ON CONFLICT DO NOTHING;
    RETURN NULL;
END $$;
CREATE TRIGGER forget_message_exposure
AFTER INSERT ON message_exposures
FOR EACH ROW
EXECUTE FUNCTION forget_message_exposure();

-- A new version of a source item moves the knowledge generation of its
-- resource forward, and records the change. A retrieval that a Forget
-- blocks writes no version (ADR-0008).
CREATE FUNCTION knowledge_source_version_insert() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO knowledge_generations(workspace_id,connection_id,resource,generation)
    VALUES(new.workspace_id,new.connection_id,new.resource,1)
    ON CONFLICT(workspace_id,connection_id,resource) DO UPDATE SET generation=knowledge_generations.generation+1;
    INSERT INTO knowledge_invalidations(workspace_id,connection_id,resource,reason)
    VALUES(new.workspace_id,new.connection_id,new.resource,'source_changed');
    RETURN NULL;
END $$;
CREATE TRIGGER knowledge_source_version_insert
AFTER INSERT ON source_versions
FOR EACH ROW
EXECUTE FUNCTION knowledge_source_version_insert();

-- The message search index holds the complete messages that are not
-- progress lines and not forgotten.
-- SQLite: json_extract(new.blocks, '$[0].type') IS NOT 'progress'.
CREATE FUNCTION conversation_message_search_insert() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO conversation_message_search(message_id, workspace_id, text_content, document)
    VALUES (new.id, new.workspace_id, new.text_content,
            to_tsvector('english', new.text_content));
    RETURN NULL;
END $$;
CREATE TRIGGER conversation_message_search_insert
AFTER INSERT ON messages
FOR EACH ROW
WHEN (new.status = 'complete'
      AND (new.blocks::jsonb -> 0 ->> 'type') IS DISTINCT FROM 'progress')
EXECUTE FUNCTION conversation_message_search_insert();

CREATE FUNCTION conversation_message_search_update() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM conversation_message_search WHERE message_id = old.id;
    INSERT INTO conversation_message_search(message_id, workspace_id, text_content, document)
    SELECT new.id, new.workspace_id, new.text_content,
           to_tsvector('english', new.text_content)
    WHERE new.status = 'complete'
      AND (new.blocks::jsonb -> 0 ->> 'type') IS DISTINCT FROM 'progress'
      AND NOT EXISTS (
          SELECT 1 FROM forgotten_messages f WHERE f.message_id = new.id
      );
    RETURN NULL;
END $$;
CREATE TRIGGER conversation_message_search_update
AFTER UPDATE OF status, blocks, text_content ON messages
FOR EACH ROW
EXECUTE FUNCTION conversation_message_search_update();

CREATE FUNCTION conversation_message_search_delete() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM conversation_message_search WHERE message_id = old.id;
    RETURN NULL;
END $$;
CREATE TRIGGER conversation_message_search_delete
AFTER DELETE ON messages
FOR EACH ROW
EXECUTE FUNCTION conversation_message_search_delete();

CREATE FUNCTION conversation_message_search_forget() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM conversation_message_search WHERE message_id = new.message_id;
    RETURN NULL;
END $$;
CREATE TRIGGER conversation_message_search_forget
AFTER INSERT ON forgotten_messages
FOR EACH ROW
EXECUTE FUNCTION conversation_message_search_forget();

-- A changed or revoked grant takes the messages it exposed out of the
-- index.
CREATE FUNCTION conversation_message_search_grant_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM conversation_message_search
    WHERE message_id IN (
        SELECT message_id FROM message_exposures WHERE grant_id = new.id
    );
    RETURN NULL;
END $$;
CREATE TRIGGER conversation_message_search_grant_change
AFTER UPDATE OF revision, revoked_at ON grants
FOR EACH ROW
EXECUTE FUNCTION conversation_message_search_grant_change();

-- The tool search index follows the tool evidence rows. Forgetting a
-- message and changing a grant delete the evidence, and the delete
-- trigger then clears the index.
CREATE FUNCTION conversation_tool_search_insert() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO conversation_tool_search(reference, workspace_id, content, document)
    VALUES (new.reference, new.workspace_id, new.content,
            to_tsvector('english', new.content));
    RETURN NULL;
END $$;
CREATE TRIGGER conversation_tool_search_insert
AFTER INSERT ON conversation_tool_evidence
FOR EACH ROW
EXECUTE FUNCTION conversation_tool_search_insert();

CREATE FUNCTION conversation_tool_search_delete() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM conversation_tool_search WHERE reference = old.reference;
    RETURN NULL;
END $$;
CREATE TRIGGER conversation_tool_search_delete
AFTER DELETE ON conversation_tool_evidence
FOR EACH ROW
EXECUTE FUNCTION conversation_tool_search_delete();

CREATE FUNCTION conversation_tool_search_forget() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM conversation_tool_evidence
    WHERE source_message_id = new.message_id;
    RETURN NULL;
END $$;
CREATE TRIGGER conversation_tool_search_forget
AFTER INSERT ON forgotten_messages
FOR EACH ROW
EXECUTE FUNCTION conversation_tool_search_forget();

CREATE FUNCTION conversation_tool_search_grant_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM conversation_tool_evidence WHERE grant_id = new.id;
    RETURN NULL;
END $$;
CREATE TRIGGER conversation_tool_search_grant_change
AFTER UPDATE OF revision, revoked_at ON grants
FOR EACH ROW
EXECUTE FUNCTION conversation_tool_search_grant_change();

-- A Continuation Record that summarized a forgotten message, or
-- read under a grant that changed, is deleted (ADR-0009).
CREATE FUNCTION continuation_checkpoint_forget() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM continuation_checkpoints
    WHERE (workspace_id, agent_id, channel_id, root_key) IN (
        SELECT workspace_id, agent_id, channel_id, root_key
        FROM continuation_checkpoint_messages
        WHERE message_id = new.message_id
    );
    RETURN NULL;
END $$;
CREATE TRIGGER continuation_checkpoint_forget
AFTER INSERT ON forgotten_messages
FOR EACH ROW
EXECUTE FUNCTION continuation_checkpoint_forget();

CREATE FUNCTION continuation_checkpoint_grant_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM continuation_checkpoints
    WHERE (workspace_id, agent_id, channel_id, root_key) IN (
        SELECT workspace_id, agent_id, channel_id, root_key
        FROM continuation_checkpoint_exposures
        WHERE grant_id = new.id
    );
    RETURN NULL;
END $$;
CREATE TRIGGER continuation_checkpoint_grant_change
AFTER UPDATE OF revision, revoked_at ON grants
FOR EACH ROW
EXECUTE FUNCTION continuation_checkpoint_grant_change();
