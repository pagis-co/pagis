-- The SQLite schema of a Pagis installation (ADR-0024).
--
-- The Postgres baseline holds the same tables, columns, constraints and
-- triggers. A change to one baseline is a change to the other, and the
-- store-trait suite in pagis-testkit runs against both.
--
-- SQLite accepts a forward reference in a foreign key, so `workspaces`
-- names `agents`, `schedules` and `users` before they exist. The three
-- full-text indexes are FTS5 virtual tables. Triggers keep the two
-- conversation indexes in step with their source rows; the daemon
-- writes Memory Search with the Page Index.

CREATE TABLE workspaces (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    -- Onboarding completion: set once when the wizard finishes.
    onboarded_at INTEGER,
    timezone TEXT NOT NULL DEFAULT 'UTC',
    -- One Agent is the Chief of Staff of the Workspace (ADR-0022). The
    -- column is nullable: a Workspace with no active Agent has none.
    chief_of_staff_agent_id TEXT REFERENCES agents (id),
    -- The Schedule that makes the Chief of Staff write the Report for
    -- Home (ADR-0022). NULL until the user asks for one.
    report_schedule_id TEXT REFERENCES schedules (id),
    -- The person who owns the Workspace. NULL on the Org's Workspace
    -- (orgs.workspace_id), which no person owns.
    user_id TEXT REFERENCES users (id)
);

CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    name TEXT NOT NULL,
    job TEXT NOT NULL,
    personality TEXT NOT NULL,
    model_alias TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'archived')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- The Agent Voice (ADR-0020): one name from the catalogue the
    -- daemon knows, validated when it is written. NULL declares no
    -- voice, and the speech provider then uses its own default.
    voice TEXT,
    -- The standing brief (ADR-0020): what an inbound call to the
    -- Agent's desk line is for, read at answer time. NULL declares none.
    standing_brief TEXT,
    -- Sprite appearance, as JSON.
    avatar TEXT NOT NULL
        DEFAULT '{"sprite":"pixie","preset":"mint","colors":{},"accessories":{}}'
        CHECK (json_valid(avatar)),
    -- One line that says what to ask this Agent for. The other Agents
    -- read it in their staff line; the job stays the Agent's own label.
    description TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_agents_workspace ON agents (workspace_id);

CREATE TABLE channels (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    title TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- Channel kinds: a DM triggers its agent on every user
    -- message; a group channel triggers only on @-mentions.
    kind TEXT NOT NULL DEFAULT 'dm' CHECK (kind IN ('dm', 'group'))
);
CREATE INDEX idx_channels_workspace ON channels (workspace_id);

CREATE TABLE channel_participants (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    channel_id TEXT NOT NULL REFERENCES channels (id),
    participant_kind TEXT NOT NULL CHECK (participant_kind IN ('user', 'agent')),
    agent_id TEXT REFERENCES agents (id),
    joined_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_channel_participants_unique
    ON channel_participants (channel_id, participant_kind, agent_id);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    channel_id TEXT NOT NULL REFERENCES channels (id),
    parent_message_id TEXT REFERENCES messages (id),
    author_kind TEXT NOT NULL CHECK (author_kind IN ('user', 'agent', 'system')),
    author_agent_id TEXT REFERENCES agents (id),
    run_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('streaming', 'complete', 'failed')),
    blocks TEXT NOT NULL,
    text_content TEXT NOT NULL,
    pending_id TEXT,
    created_at INTEGER NOT NULL,
    completed_at INTEGER
);
CREATE INDEX idx_messages_channel ON messages (channel_id, created_at);
CREATE INDEX idx_messages_parent ON messages (parent_message_id);
CREATE UNIQUE INDEX idx_messages_pending
    ON messages (channel_id, pending_id)
    WHERE pending_id IS NOT NULL;

CREATE TABLE runs (
    id TEXT PRIMARY KEY,
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
);
CREATE INDEX idx_runs_agent ON runs (agent_id, created_at);
CREATE INDEX idx_runs_channel ON runs (channel_id);

CREATE TABLE events (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    event_type TEXT NOT NULL,
    agent_id TEXT,
    run_id TEXT,
    channel_id TEXT,
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_events_run ON events (run_id);
CREATE INDEX idx_events_workspace_created ON events (workspace_id, created_at);
-- The event bus reads one Workspace's events in seq order, so a socket
-- receives the events of its own tenant and no other. SQLite reads the
-- implicit rowid as `seq`, and the rowid ends every index key.
CREATE INDEX idx_events_workspace_seq ON events (workspace_id);

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    creator_agent_id TEXT REFERENCES agents (id),
    run_id TEXT REFERENCES runs (id),
    filename TEXT,
    mime TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    storage_key TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    -- One retention workflow for every artifact class. The class
    -- is a stored column, because the sweep must not guess a class from
    -- a filename.
    kind TEXT NOT NULL DEFAULT 'file'
);
CREATE UNIQUE INDEX idx_artifacts_dedup ON artifacts (workspace_id, sha256);

CREATE TABLE model_aliases (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    alias TEXT NOT NULL,
    candidates TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_model_aliases_unique ON model_aliases (workspace_id, alias);

CREATE TABLE requests (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    agent_id TEXT NOT NULL REFERENCES agents (id),
    run_id TEXT REFERENCES runs (id),
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN ('pending', 'approved', 'denied', 'expired', 'superseded')
    ),
    decided_at INTEGER,
    created_at INTEGER NOT NULL,
    -- The values a form or a choice submits with its decision.
    submitted_values TEXT
);

CREATE TABLE capability_snapshots (
    id TEXT PRIMARY KEY,
    hash TEXT NOT NULL UNIQUE,
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE run_capability_snapshots (
    run_id TEXT PRIMARY KEY REFERENCES runs (id),
    snapshot_id TEXT NOT NULL REFERENCES capability_snapshots (id)
);

CREATE TABLE credentials (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    -- The registrable domain, from the public suffix list. The
    -- registrable domain of login_url must equal it.
    domain TEXT NOT NULL,
    username TEXT NOT NULL,
    login_url TEXT NOT NULL,
    -- Sealed with ChaCha20-Poly1305 under the vault data key, which
    -- lives in the OS keychain. The database never holds plaintext.
    secret BLOB NOT NULL,
    totp_seed BLOB,
    -- The applied password recipe, in Apple's Password Rules grammar.
    recipe TEXT NOT NULL,
    -- The owning Agent, or NULL for the user. Archiving an Agent
    -- clears it and leaves the record usable.
    owner_agent_id TEXT REFERENCES agents (id),
    provenance TEXT NOT NULL CHECK (provenance IN ('user_supplied', 'agent_minted')),
    created_run_id TEXT REFERENCES runs (id),
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_credentials_workspace_domain ON credentials (workspace_id, domain);
CREATE INDEX idx_requests_state ON requests (workspace_id, state, kind);

CREATE TABLE connections (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    provider TEXT NOT NULL,
    alias TEXT NOT NULL,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('disconnected', 'connecting', 'connected', 'reauth_required', 'unavailable')
    ),
    auth_mode TEXT NOT NULL CHECK (auth_mode IN ('byo', 'brokered')),
    config TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    -- What the user authorized the account for, as a JSON array.
    authorized_capabilities TEXT NOT NULL DEFAULT '[]',
    -- The refresh token of a brokered Connection (ADR-0012). It arrives
    -- from Google at the callback and is sealed with the Tenant Data Key
    -- of the Workspace before it lands here, so no other tenant's key
    -- opens it.
    refresh_token BLOB
);
CREATE UNIQUE INDEX idx_connections_alias ON connections (workspace_id, alias);

CREATE TABLE schedules (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    agent_id TEXT NOT NULL REFERENCES agents (id),
    name TEXT NOT NULL,
    instruction TEXT NOT NULL,
    channel_id TEXT NOT NULL REFERENCES channels (id),
    root_message_id TEXT REFERENCES messages (id),
    kind TEXT NOT NULL CHECK (kind IN ('one_shot', 'cron', 'interval')),
    cron_expression TEXT,
    interval_ms INTEGER,
    anchor_at INTEGER,
    timezone TEXT NOT NULL,
    scheduled_at INTEGER NOT NULL,
    next_due_at INTEGER,
    state TEXT NOT NULL CHECK (state IN ('active', 'paused', 'completed', 'blocked', 'archived')),
    revision INTEGER NOT NULL,
    approved_revision INTEGER,
    creator TEXT NOT NULL CHECK (creator IN ('user', 'agent')),
    creating_run_id TEXT REFERENCES runs (id),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    archived_at INTEGER,
    -- A reminder schedule anchored on forgotten knowledge stays
    -- archived and keyed as forgotten. The mark outlives the followup
    -- row that the purge deletes, so a queued run cannot start from the
    -- anchor later.
    forgotten INTEGER NOT NULL DEFAULT 0 CHECK(forgotten IN (0,1))
);
CREATE INDEX idx_schedules_due ON schedules (next_due_at) WHERE state = 'active';
CREATE INDEX idx_schedules_workspace ON schedules (workspace_id, id DESC);

CREATE TABLE schedule_revisions (
    schedule_id TEXT NOT NULL REFERENCES schedules (id),
    revision INTEGER NOT NULL,
    agent_id TEXT NOT NULL REFERENCES agents (id),
    name TEXT NOT NULL,
    instruction TEXT NOT NULL,
    channel_id TEXT NOT NULL REFERENCES channels (id),
    root_message_id TEXT REFERENCES messages (id),
    kind TEXT NOT NULL CHECK (kind IN ('one_shot', 'cron', 'interval')),
    cron_expression TEXT,
    interval_ms INTEGER,
    anchor_at INTEGER,
    timezone TEXT NOT NULL,
    scheduled_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    creating_run_id TEXT REFERENCES runs (id),
    PRIMARY KEY (schedule_id, revision)
);

CREATE TABLE schedule_occurrences (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    schedule_id TEXT NOT NULL REFERENCES schedules (id),
    schedule_revision INTEGER NOT NULL,
    scheduled_at INTEGER NOT NULL,
    processed_at INTEGER NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('wakeup_created', 'combined', 'skipped')),
    wakeup_id TEXT,
    UNIQUE (schedule_id, scheduled_at)
);
CREATE INDEX idx_occurrences_schedule ON schedule_occurrences (schedule_id, id DESC);

CREATE TABLE event_subscriptions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    agent_id TEXT NOT NULL REFERENCES agents (id),
    connection_id TEXT NOT NULL REFERENCES connections (id),
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
    archived_at INTEGER
);
CREATE INDEX idx_subscriptions_workspace ON event_subscriptions (workspace_id, id DESC);
CREATE INDEX idx_subscriptions_source ON event_subscriptions (connection_id, event_kind, state);

CREATE TABLE provider_cursors (
    connection_id TEXT NOT NULL REFERENCES connections (id),
    event_kind TEXT NOT NULL,
    cursor TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (connection_id, event_kind)
);

CREATE TABLE source_batches (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    connection_id TEXT NOT NULL REFERENCES connections (id),
    event_kind TEXT NOT NULL,
    collected_at INTEGER NOT NULL,
    collected_count INTEGER NOT NULL,
    stored_count INTEGER NOT NULL,
    wakeup_count INTEGER NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('baseline', 'collected', 'failed')),
    detail TEXT
);
CREATE INDEX idx_batches_source ON source_batches (connection_id, event_kind, id DESC);

CREATE TABLE incoming_events (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    connection_id TEXT NOT NULL REFERENCES connections (id),
    event_kind TEXT NOT NULL,
    provider_event_id TEXT NOT NULL,
    metadata TEXT NOT NULL,
    occurred_at INTEGER NOT NULL,
    received_at INTEGER NOT NULL,
    batch_id TEXT NOT NULL REFERENCES source_batches (id),
    UNIQUE (connection_id, event_kind, provider_event_id)
);
CREATE INDEX idx_incoming_events_source ON incoming_events (connection_id, event_kind, id DESC);

CREATE TABLE wakeups (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    source_kind TEXT NOT NULL CHECK (source_kind IN ('schedule', 'event_subscription', 'arrival')),
    schedule_id TEXT REFERENCES schedules (id),
    subscription_id TEXT REFERENCES event_subscriptions (id),
    rule_revision INTEGER NOT NULL,
    rule_name TEXT NOT NULL,
    agent_id TEXT NOT NULL REFERENCES agents (id),
    channel_id TEXT REFERENCES channels (id),
    root_message_id TEXT REFERENCES messages (id),
    instruction TEXT NOT NULL,
    scheduled_at INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'started', 'withdrawn')),
    run_id TEXT UNIQUE REFERENCES runs (id),
    source_count INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    -- A synced arrival starts one reflection-only Run for its Subject
    -- Pages (ADR-0011).
    subject_paths TEXT,
    -- An arrival Wake-up says whether its Subject Pages are historical,
    -- so the briefing can tell the Run (ADR-0011).
    historical INTEGER NOT NULL DEFAULT 0,
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
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    wakeup_id TEXT NOT NULL REFERENCES wakeups (id),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN ('schedule_occurrence', 'incoming_event')
    ),
    source_id TEXT NOT NULL,
    PRIMARY KEY (wakeup_id, source_kind, source_id)
);
CREATE UNIQUE INDEX idx_wakeup_sources_occurrence
    ON wakeup_sources (source_id) WHERE source_kind = 'schedule_occurrence';
CREATE INDEX idx_wakeup_sources_event
    ON wakeup_sources (source_id) WHERE source_kind = 'incoming_event';

CREATE TABLE phone_numbers (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    -- The carrier that sold it. There is no foreign key: a released
    -- record is a tombstone for the Calls that point at it, and it
    -- outlives the Connection the user later deletes.
    connection_id TEXT NOT NULL,
    e164 TEXT NOT NULL,
    provider_number_id TEXT NOT NULL,
    agent_id TEXT REFERENCES agents (id),
    status TEXT NOT NULL CHECK (status IN ('assigned', 'unassigned', 'released')),
    created_at INTEGER NOT NULL,
    assigned_at INTEGER,
    -- The Outgoing Cap of a number and the rules that skip its send card
    -- (ADR-0020). They mirror the fields the Agent Mailbox record has.
    outgoing_cap INTEGER NOT NULL DEFAULT 50,
    allow_rules TEXT NOT NULL DEFAULT '[]',
    -- The day the send tally counts, and the tally. The cap is a day's
    -- allowance, so the tally starts again on a new day.
    sends_day INTEGER,
    sends_today INTEGER NOT NULL DEFAULT 0,
    -- Whether the carrier will deliver an outbound text (ADR-0020), when
    -- the daemon last read it, and the error of a read that failed.
    messaging_readiness TEXT NOT NULL DEFAULT 'unknown',
    messaging_readiness_reason TEXT,
    messaging_readiness_at INTEGER,
    messaging_readiness_error TEXT,
    -- Where the inbound collector reached, and the carrier's messaging
    -- object for this number (ADR-0020).
    text_cursor TEXT,
    messaging_object_id TEXT,
    -- The Telnyx relay function: where it stands, why a ship failed, the
    -- last lines the CLI wrote, and the user's consent to install the
    -- CLI (ADR-0020).
    relay_state TEXT NOT NULL DEFAULT 'absent',
    relay_reason TEXT,
    relay_cli_lines TEXT NOT NULL DEFAULT '[]',
    relay_consent INTEGER NOT NULL DEFAULT 0,
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
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    connection_id TEXT NOT NULL,
    e164 TEXT NOT NULL,
    agent_id TEXT REFERENCES agents (id),
    state TEXT NOT NULL CHECK (state IN ('pending', 'bought', 'abandoned')),
    created_at INTEGER NOT NULL,
    settled_at INTEGER
);
CREATE UNIQUE INDEX idx_purchase_intents_pending
    ON phone_number_purchase_intents (workspace_id, e164) WHERE state = 'pending';

CREATE TABLE calls (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    agent_id TEXT NOT NULL REFERENCES agents (id),
    -- The Run stays alive for the length of the call.
    run_id TEXT NOT NULL REFERENCES runs (id),
    -- No foreign key: a released number is a tombstone the Call still
    -- points at.
    phone_number_id TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('outbound', 'inbound')),
    remote_e164 TEXT NOT NULL,
    tier TEXT NOT NULL CHECK (tier IN ('owner', 'trusted', 'unknown')),
    state TEXT NOT NULL CHECK (state IN ('dialing', 'live', 'ended')),
    outcome TEXT CHECK (
        outcome IN ('answered', 'no_answer', 'busy', 'voicemail', 'failed')
    ),
    ended_reason TEXT,
    classification TEXT CHECK (
        classification IN (
            'human', 'machine-ivr', 'machine-vm', 'machine-unavailable', 'uncertain'
        )
    ),
    message_left INTEGER NOT NULL DEFAULT 0,
    transcript TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    ringing_at INTEGER,
    answered_at INTEGER,
    ended_at INTEGER,
    recording_artifact_id TEXT REFERENCES artifacts (id),
    -- The call brief: who called (the Agent and its own line), what the
    -- call was for, and which tools the call could use. The number
    -- record is a tombstone after a release, so own_e164 must live here.
    agent_name TEXT NOT NULL DEFAULT '',
    own_e164 TEXT NOT NULL DEFAULT '',
    purpose TEXT NOT NULL DEFAULT '',
    -- The tool names, as a JSON array.
    tools TEXT NOT NULL DEFAULT '[]',
    -- Every Call that ended has a reason (ADR-0020).
    CHECK ((state = 'ended') = (ended_reason IS NOT NULL))
);
CREATE INDEX idx_calls_workspace ON calls (workspace_id, created_at);
CREATE INDEX idx_calls_number ON calls (phone_number_id, state);
CREATE INDEX idx_calls_run ON calls (run_id);
CREATE INDEX idx_artifacts_retention ON artifacts (workspace_id, kind, created_at);

CREATE TABLE artifact_retention_policies (
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    kind TEXT NOT NULL CHECK (
        kind IN ('screenshot', 'call_recording', 'call_transcript', 'file')
    ),
    retain_days INTEGER NOT NULL CHECK (retain_days > 0),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, kind)
);

CREATE TABLE software_packages (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    -- The namespace the tools take. The first publish claims it.
    name TEXT NOT NULL,
    -- Only this Agent publishes later Versions. Another Agent forks.
    author_agent_id TEXT NOT NULL REFERENCES agents (id),
    description TEXT NOT NULL,
    -- The manifest keywords, as a JSON array of strings.
    keywords TEXT NOT NULL,
    -- The tag of the newest Version: `v1`, `v2` and so on.
    latest_version TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- A Fork is a Software Package that started as a copy of one
    -- Version of another one. The fork's first publish writes the
    -- origin; the two columns never change after that.
    origin_package_id TEXT REFERENCES software_packages (id),
    origin_version TEXT
);
CREATE UNIQUE INDEX idx_software_packages_name
    ON software_packages (workspace_id, name);

CREATE TABLE software_versions (
    package_id TEXT NOT NULL REFERENCES software_packages (id),
    version TEXT NOT NULL,
    notes TEXT NOT NULL,
    commit_id TEXT NOT NULL,
    -- The manifest and the argument schemas, as JSON.
    manifest TEXT NOT NULL,
    published_at INTEGER NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs (id),
    PRIMARY KEY (package_id, version)
);
CREATE INDEX idx_software_versions_package
    ON software_versions (package_id, published_at);

CREATE TABLE contributions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    -- The origin package the change is offered to.
    package_id TEXT NOT NULL REFERENCES software_packages (id),
    -- The Version the patch is against.
    base_version TEXT NOT NULL,
    -- The latest Version of the origin package when the record opened.
    latest_at_open TEXT NOT NULL,
    fork_package_id TEXT NOT NULL REFERENCES software_packages (id),
    fork_version TEXT NOT NULL,
    -- The unified diff between the two trees.
    patch TEXT NOT NULL,
    summary TEXT NOT NULL,
    -- open, merged or declined.
    status TEXT NOT NULL,
    outcome_reason TEXT,
    created_at INTEGER NOT NULL,
    closed_at INTEGER,
    run_id TEXT NOT NULL REFERENCES runs (id)
);
CREATE INDEX idx_contributions_fork
    ON contributions (fork_package_id, created_at);
CREATE INDEX idx_contributions_package
    ON contributions (package_id, created_at);

CREATE TABLE plugins (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    -- The name of `plugin.json`. It is the tool namespace.
    name TEXT NOT NULL,
    -- The address the files came from, as JSON: a git URL with an
    -- optional ref, or an upload.
    source TEXT NOT NULL,
    installed_commit TEXT NOT NULL,
    -- The Capability Manifest version this state produced: `v1`, `v2`
    -- and so on.
    manifest_version TEXT NOT NULL,
    -- `enabled`, `disabled` or `failed`.
    state TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_plugins_name ON plugins (workspace_id, name);

CREATE TABLE plugin_bindings (
    plugin_id TEXT NOT NULL REFERENCES plugins (id),
    field TEXT NOT NULL,
    -- The bound Connection, secret name or plain value, as JSON.
    value TEXT NOT NULL,
    PRIMARY KEY (plugin_id, field)
);

CREATE TABLE grants (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    agent_id TEXT NOT NULL REFERENCES agents (id),
    resource_kind TEXT NOT NULL
        CHECK (resource_kind IN ('connection', 'credential', 'host', 'plugin')),
    resource_id TEXT,
    scope TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    revoked_at INTEGER
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
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    -- The holder stays on the tombstone, so the ledger says who held
    -- the address.
    agent_id TEXT NOT NULL REFERENCES agents (id),
    -- The Mailbox Provider that carries it. There is no foreign key: a
    -- tombstone outlives the Connection the user later removes.
    connection_id TEXT NOT NULL,
    address TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN ('provisioning', 'active', 'unavailable', 'dormant', 'deleted')
    ),
    -- Why the mailbox is unavailable, in the words the desk shows.
    reason TEXT,
    outgoing_cap INTEGER NOT NULL,
    -- Where the collector reached. The three columns are one cursor,
    -- so they are present together or absent together.
    cursor_folder TEXT,
    cursor_uid_validity INTEGER,
    cursor_last_uid INTEGER,
    -- The day the send tally counts, and the tally. The Outgoing Cap is
    -- a day's allowance, so the tally starts again on a new day.
    sends_day INTEGER,
    sends_today INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    deleted_at INTEGER,
    -- The allow rules the user wrote from a send card, as a JSON array
    -- of registrable domains. They sit on the record and not in a Grant,
    -- because an Agent holds its own mailbox with no Grant.
    allow_rules TEXT NOT NULL DEFAULT '[]',
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
    plugin_id TEXT NOT NULL REFERENCES plugins (id),
    -- `v1`, `v2` and so on.
    version TEXT NOT NULL,
    installed_commit TEXT NOT NULL,
    -- The frozen tools, as a JSON array.
    tools TEXT NOT NULL,
    -- Whether a server has offered a different list since the freeze.
    tools_changed INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (plugin_id, version)
);

CREATE TABLE sent_mail (
    message_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    agent_id TEXT NOT NULL REFERENCES agents (id),
    -- No foreign key: the row outlives the mailbox the user deletes.
    mailbox_id TEXT NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs (id),
    channel_id TEXT,
    -- The message the Run's conversation is rooted at, or NULL when the
    -- Run was not in a Thread.
    thread_id TEXT,
    sent_at INTEGER NOT NULL
);
CREATE INDEX idx_sent_mail_mailbox ON sent_mail (mailbox_id, sent_at);
CREATE UNIQUE INDEX idx_wakeups_pending_subscription
    ON wakeups (subscription_id, channel_id, COALESCE(root_message_id, ''))
    WHERE state = 'pending' AND subscription_id IS NOT NULL;

CREATE TABLE trust_entries (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    -- NULL for the Workspace-wide list.
    agent_id TEXT REFERENCES agents (id),
    -- What the value names. A domain covers every address at it.
    subject TEXT NOT NULL CHECK (subject IN ('number', 'address', 'domain')),
    -- The E.164 number, the bare address or the bare domain, in the one
    -- form the daemon stores.
    value TEXT NOT NULL,
    -- A list proposes a tier above Unknown; Unknown is what a subject on
    -- no list gets, so it is not a row.
    tier TEXT NOT NULL CHECK (tier IN ('owner', 'trusted')),
    label TEXT NOT NULL,
    created_at INTEGER NOT NULL
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
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces (id),
    -- The wrong codes on every Call and every line of the Workspace.
    failed_attempts INTEGER NOT NULL CHECK (failed_attempts >= 0),
    -- The end of the latest delay, or NULL before the first delay.
    suspended_until INTEGER,
    updated_at INTEGER NOT NULL
);

CREATE TABLE sync_resources (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    connection_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    config TEXT NOT NULL CHECK(json_valid(config)),
    revision INTEGER NOT NULL DEFAULT 1,
    checkpoint TEXT NOT NULL DEFAULT 'null'
        CHECK(json_valid(checkpoint)),
    caught_up INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL,
    acquisition_error TEXT,
    arrival_error TEXT,
    cursor_revision INTEGER NOT NULL DEFAULT 1,
    -- The filter is part of the resource config. A change to it bumps
    -- filter_revision, and the reflection records of each revision stay
    -- beside the records of the next one.
    filter_revision INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY(workspace_id, connection_id, resource)
);

CREATE TABLE text_records (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    -- The record stays with the Agent whatever happens to the number,
    -- so a new holder of the number sees none of it.
    agent_id TEXT NOT NULL REFERENCES agents (id),
    -- No foreign key: a released number is a tombstone the record
    -- still points at, as a Call does.
    phone_number_id TEXT NOT NULL,
    counterpart_e164 TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('outbound', 'inbound')),
    tier TEXT NOT NULL CHECK (tier IN ('owner', 'trusted', 'unknown')),
    body TEXT NOT NULL,
    segments INTEGER NOT NULL,
    -- The MMS media, as a JSON array of Artifact ids.
    media_artifact_ids TEXT NOT NULL DEFAULT '[]',
    carrier_message_id TEXT NOT NULL,
    -- The Run, the Channel and the Thread of an outbound text, as the
    -- sent_mail row holds them for a mail.
    run_id TEXT REFERENCES runs (id),
    channel_id TEXT,
    thread_id TEXT,
    delivery_state TEXT CHECK (
        delivery_state IN ('queued', 'sent', 'delivered', 'failed')
    ),
    delivery_code TEXT,
    delivery_reason TEXT,
    -- When the text was received or sent.
    occurred_at INTEGER NOT NULL,
    -- Only an outbound text has a Run that sent it and a delivery
    -- state; an inbound one has neither.
    CHECK ((direction = 'outbound') = (run_id IS NOT NULL)),
    CHECK ((direction = 'outbound') = (delivery_state IS NOT NULL))
);
CREATE INDEX idx_text_records_conversation
    ON text_records (agent_id, phone_number_id, counterpart_e164, occurred_at, id);
CREATE INDEX idx_text_records_workspace ON text_records (workspace_id, occurred_at);

CREATE TABLE message_exposure_sets (
    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE
);

CREATE TABLE message_exposures (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    grant_id TEXT NOT NULL,
    grant_revision INTEGER NOT NULL,
    PRIMARY KEY(message_id, grant_id)
);

CREATE TABLE source_versions (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    id TEXT NOT NULL,
    version TEXT NOT NULL,
    operation TEXT NOT NULL,
    record TEXT NOT NULL CHECK(json_valid(record)),
    observed_at INTEGER NOT NULL,
    historical INTEGER NOT NULL,
    arrival TEXT CHECK(arrival IS NULL OR json_valid(arrival)),
    arrival_ack_at INTEGER,
    -- One arrival fails alone. Its attempts, its next retry time, its
    -- outcome and its last failure stay on the acquisition row (ADR-0011).
    arrival_attempts INTEGER NOT NULL DEFAULT 0,
    arrival_retry_at INTEGER NOT NULL DEFAULT 0,
    arrival_outcome TEXT CHECK (arrival_outcome IN ('appended', 'skipped')),
    arrival_failure TEXT,
    UNIQUE(workspace_id, connection_id, resource, id, version, operation),
    FOREIGN KEY(workspace_id, connection_id, resource) REFERENCES sync_resources(workspace_id, connection_id, resource)
);
CREATE INDEX source_arrivals ON source_versions(workspace_id, connection_id, resource, arrival_ack_at, sequence);

CREATE TABLE knowledge_generations (
    workspace_id TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    generation INTEGER NOT NULL DEFAULT 0,
    rebuilding INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(workspace_id, connection_id, resource)
);

CREATE TABLE knowledge_invalidations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    reason TEXT NOT NULL,
    published INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE forget_operations (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    target TEXT NOT NULL CHECK(json_valid(target)),
    phase TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    error TEXT,
    reopted_at INTEGER,
    purge_plan TEXT CHECK(purge_plan IS NULL OR json_valid(purge_plan)),
    structured_cleaned INTEGER NOT NULL DEFAULT 0,
    connection_id TEXT NOT NULL DEFAULT ''
);

CREATE TABLE forget_suppressions (
    operation_id TEXT NOT NULL REFERENCES forget_operations(id),
    workspace_id TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    identity TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'source' CHECK(scope IN ('source','account')),
    PRIMARY KEY(operation_id,identity)
);
CREATE INDEX forget_suppression_lookup ON forget_suppressions(workspace_id,connection_id,resource,identity);

CREATE TABLE forgotten_messages (
    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE
);

CREATE TABLE memory_brief_cursors (
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    agent_id TEXT NOT NULL REFERENCES agents (id),
    channel_id TEXT NOT NULL REFERENCES channels (id),
    root_message_id TEXT NOT NULL DEFAULT '',
    memory_revision TEXT,
    shown_paths TEXT NOT NULL,
    PRIMARY KEY (workspace_id, agent_id, channel_id, root_message_id)
);

CREATE TABLE schedule_subject_pages (
    schedule_id TEXT PRIMARY KEY REFERENCES schedules (id) ON DELETE CASCADE,
    path TEXT NOT NULL
);
CREATE INDEX idx_schedule_subject_pages_path ON schedule_subject_pages (path, schedule_id);

CREATE TABLE source_items (
    workspace_id TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    id TEXT NOT NULL,
    version TEXT NOT NULL,
    record TEXT NOT NULL CHECK(json_valid(record)),
    observed_at INTEGER NOT NULL,
    PRIMARY KEY(workspace_id, connection_id, resource, id),
    FOREIGN KEY(workspace_id, connection_id, resource)
        REFERENCES sync_resources(workspace_id, connection_id, resource)
);

CREATE INDEX source_versions_parent ON source_versions(
    workspace_id, connection_id, resource, json_extract(record, '$.parent'));

CREATE TABLE page_reflections (
    workspace_id TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    page_path TEXT NOT NULL,
    -- The source subject of the page, such as a mail thread id. The
    -- backfill walks subjects, so it reads the decision by subject.
    page_subject TEXT NOT NULL,
    filter_revision INTEGER NOT NULL,
    decided_by TEXT NOT NULL CHECK(decided_by IN ('live', 'backfill')),
    verdict TEXT NOT NULL CHECK(verdict IN ('reflect', 'skip')),
    rule_index INTEGER,
    reason TEXT NOT NULL,
    -- The arrival Wake-up that reflects the page, and the moment the
    -- backfill started it. A live decision leaves both empty: the live
    -- pass starts its own Run.
    wakeup_id TEXT REFERENCES wakeups (id),
    reflected_at INTEGER,
    decided_at INTEGER NOT NULL,
    -- A page counts the Runs that failed to reflect it, so a poisoned
    -- batch stops after the attempt limit (ADR-0011).
    attempts INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(workspace_id, connection_id, resource, page_path, filter_revision),
    FOREIGN KEY(workspace_id, connection_id, resource)
        REFERENCES sync_resources(workspace_id, connection_id, resource)
);
CREATE INDEX page_reflections_backfill ON page_reflections(
    workspace_id, connection_id, resource, filter_revision, decided_by);

CREATE INDEX source_versions_arrival_subject ON source_versions(
    workspace_id, connection_id, resource,
    COALESCE(json_extract(arrival, '$.metadata.thread_id'),
             json_extract(arrival, '$.provider_event_id')))
    WHERE arrival IS NOT NULL AND historical = 1;

CREATE TABLE memory_page_index_heads (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces (id),
    revision TEXT NOT NULL,
    next_position INTEGER NOT NULL
);

CREATE TABLE memory_page_index (
    -- The rowid, which the FTS5 rows of Memory Search share.
    id INTEGER PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    path TEXT NOT NULL,
    position INTEGER NOT NULL,
    changed_at INTEGER NOT NULL,
    changed_by_name TEXT NOT NULL,
    changed_by_email TEXT NOT NULL,
    title TEXT NOT NULL,
    kind TEXT,
    source_connection_id TEXT,
    -- The exposure stamp as JSON. NULL is a file with no stamp, which
    -- no reader sees.
    exposures TEXT,
    -- NULL is a file that is not a Subject Page with a valid layout. The
    -- words are in order, with one space between two.
    brief_words TEXT,
    UNIQUE (workspace_id, path)
);

-- Memory Search is derived data beside the Page Index (ADR-0008): an
-- FTS5 index over the path, the title and the body of each page, with
-- the tenant of the row. The daemon writes it with the Page Index.
CREATE VIRTUAL TABLE memory_page_search USING fts5(
    workspace_id UNINDEXED,
    path,
    title,
    body,
    tokenize = 'porter unicode61'
);
-- A delete takes the words of its row out of the index at once. Without
-- this option, FTS5 keeps them in its segments until a later merge, and
-- the words of a page that a Forget purges stay in the database file.
INSERT INTO memory_page_search(memory_page_search, rank) VALUES ('secure-delete', 1);

CREATE TABLE onboarding_model_verifications (
    workspace_id TEXT PRIMARY KEY NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    available INTEGER NOT NULL,
    proof TEXT NOT NULL
);

CREATE TABLE pending_evidence (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    agent_id TEXT NOT NULL REFERENCES agents(id),
    channel_id TEXT NOT NULL REFERENCES channels(id),
    root_message_id TEXT REFERENCES messages(id),
    subject TEXT NOT NULL,
    after_exclusive TEXT,
    through_inclusive TEXT NOT NULL REFERENCES messages(id),
    reason TEXT NOT NULL,
    urgency TEXT NOT NULL CHECK (urgency IN ('normal', 'urgent')),
    eligible_at INTEGER NOT NULL,
    maximum_due_at INTEGER NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL CHECK (state IN ('pending', 'leased', 'completed', 'failed', 'invalidated')),
    revision INTEGER NOT NULL DEFAULT 1,
    lease_run_id TEXT REFERENCES runs(id),
    leased_at INTEGER,
    lease_expires_at INTEGER,
    error TEXT,
    memory_revision TEXT,
    completed_at INTEGER,
    failed_overlap_id TEXT REFERENCES pending_evidence(id),
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_pending_evidence_due
    ON pending_evidence(state, urgency, eligible_at, maximum_due_at, created_at, id);
CREATE UNIQUE INDEX idx_pending_evidence_open_subject
    ON pending_evidence(workspace_id, agent_id, subject)
    WHERE state = 'pending';

CREATE TABLE pending_evidence_messages (
    pending_id TEXT NOT NULL REFERENCES pending_evidence(id) ON DELETE CASCADE,
    message_id TEXT NOT NULL REFERENCES messages(id),
    channel_id TEXT NOT NULL REFERENCES channels(id),
    root_message_id TEXT REFERENCES messages(id),
    root_key TEXT NOT NULL,
    PRIMARY KEY (pending_id, message_id)
);

CREATE TABLE pending_evidence_exposures (
    pending_id TEXT NOT NULL REFERENCES pending_evidence(id) ON DELETE CASCADE,
    grant_id TEXT NOT NULL REFERENCES grants(id),
    grant_revision INTEGER NOT NULL,
    PRIMARY KEY (pending_id, grant_id, grant_revision)
);

CREATE TABLE pending_review_cursors (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    agent_id TEXT NOT NULL REFERENCES agents(id),
    channel_id TEXT NOT NULL REFERENCES channels(id),
    root_message_id TEXT,
    root_key TEXT NOT NULL,
    subject TEXT NOT NULL,
    through_inclusive TEXT NOT NULL REFERENCES messages(id),
    memory_revision TEXT,
    reviewed_at INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, agent_id, channel_id, root_key, subject)
);

-- An FTS5 index over the text of a complete message, with the tenant
-- of the row. The triggers at the end keep it in step with `messages`.
CREATE VIRTUAL TABLE conversation_message_search USING fts5(
    workspace_id UNINDEXED,
    message_id UNINDEXED,
    text_content,
    tokenize = 'porter unicode61'
);
-- A delete takes the words of its row out of the index at once, so a
-- forgotten message leaves no word in the FTS5 segments (ADR-0008).
INSERT INTO conversation_message_search(conversation_message_search, rank)
VALUES ('secure-delete', 1);

CREATE TABLE conversation_tool_evidence (
    reference TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    agent_id TEXT NOT NULL REFERENCES agents(id),
    channel_id TEXT NOT NULL REFERENCES channels(id),
    root_message_id TEXT REFERENCES messages(id),
    source_message_id TEXT NOT NULL REFERENCES messages(id),
    run_id TEXT NOT NULL REFERENCES runs(id),
    tool_call_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    content TEXT NOT NULL,
    complete INTEGER NOT NULL CHECK(complete IN (0, 1)),
    grant_id TEXT NOT NULL REFERENCES grants(id),
    grant_revision INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(run_id, tool_call_id)
);
CREATE INDEX conversation_tool_evidence_scope
ON conversation_tool_evidence(workspace_id, agent_id, channel_id, root_message_id, source_message_id);

-- An FTS5 index over the content of a tool result, with the tenant of
-- the row. The triggers at the end keep it in step with
-- `conversation_tool_evidence`.
CREATE VIRTUAL TABLE conversation_tool_search USING fts5(
    workspace_id UNINDEXED,
    reference UNINDEXED,
    content,
    tokenize = 'porter unicode61'
);
-- A delete takes the words of its row out of the index at once, so a
-- deleted tool result leaves no word in the FTS5 segments (ADR-0008).
INSERT INTO conversation_tool_search(conversation_tool_search, rank)
VALUES ('secure-delete', 1);

-- The synced source content that a Run read (ADR-0008): one Source
-- Item, or every item of one parent, such as the messages of one mail
-- thread. A Forget of an item forgets each message of a Run that read
-- it, and its purge deletes the rows that name the item or its parent.
CREATE TABLE run_source_reads (
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    run_id TEXT NOT NULL REFERENCES runs (id),
    connection_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('item', 'parent')),
    source_id TEXT NOT NULL,
    PRIMARY KEY (run_id, connection_id, resource, kind, source_id)
);
CREATE INDEX run_source_reads_source
    ON run_source_reads (workspace_id, connection_id, resource, source_id);

CREATE TABLE continuation_checkpoints (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    agent_id TEXT NOT NULL REFERENCES agents(id),
    channel_id TEXT NOT NULL REFERENCES channels(id),
    root_message_id TEXT REFERENCES messages(id),
    root_key TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision > 0),
    after_exclusive TEXT,
    through_inclusive TEXT NOT NULL REFERENCES messages(id),
    state TEXT NOT NULL CHECK(json_valid(state)),
    created_at INTEGER NOT NULL,
    PRIMARY KEY(workspace_id, agent_id, channel_id, root_key),
    CHECK(root_key = coalesce(root_message_id, ''))
);

CREATE TABLE continuation_checkpoint_messages (
    workspace_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    root_key TEXT NOT NULL,
    message_id TEXT NOT NULL REFERENCES messages(id),
    PRIMARY KEY(workspace_id, agent_id, channel_id, root_key, message_id),
    FOREIGN KEY(workspace_id, agent_id, channel_id, root_key)
        REFERENCES continuation_checkpoints(workspace_id, agent_id, channel_id, root_key)
        ON DELETE CASCADE
);

CREATE TABLE continuation_checkpoint_exposures (
    workspace_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    root_key TEXT NOT NULL,
    grant_id TEXT NOT NULL REFERENCES grants(id),
    grant_revision INTEGER NOT NULL,
    PRIMARY KEY(workspace_id, agent_id, channel_id, root_key, grant_id, grant_revision),
    FOREIGN KEY(workspace_id, agent_id, channel_id, root_key)
        REFERENCES continuation_checkpoints(workspace_id, agent_id, channel_id, root_key)
        ON DELETE CASCADE
);

CREATE TABLE memory_page_link (
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    -- The repository path of the page that holds the link.
    source_path TEXT NOT NULL,
    -- The repository path the link names.
    target_path TEXT NOT NULL,
    -- Where the link stands in the page, counted from zero. A reader
    -- orders on it: a page links in the order it names its pages, and
    -- the one hop of a Brief takes the first of them. Without it the
    -- order of a read is arbitrary and the Brief is not repeatable.
    position INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, source_path, target_path)
) WITHOUT ROWID;
CREATE INDEX memory_page_link_target
    ON memory_page_link (workspace_id, target_path);

CREATE TABLE orgs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    -- The Google Web OAuth client of the installation (ADR-0012). Every
    -- person consents against it. Its secret is an installation secret
    -- in the secret file, not a column.
    google_client_id TEXT,
    -- The Workspace that holds the Org's own records: the installed
    -- Plugins and the Installation Connections. No person owns it, so
    -- its user_id is NULL and no Session reaches it.
    workspace_id TEXT NOT NULL UNIQUE REFERENCES workspaces (id)
);

CREATE TABLE users (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES orgs (id),
    -- NULL for the seeded person of a local installation, who signs in
    -- with the Client Credential.
    email TEXT,
    name TEXT,
    -- The argon2id hash. NULL means this person has no password.
    password_hash TEXT,
    role TEXT NOT NULL CHECK (role IN ('administrator', 'member')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- An account the Administrator manages: it can be disabled and
    -- given back, and a disabled account keeps its Workspace.
    disabled_at INTEGER,
    -- When a client last signed in as the Person.
    last_signed_in_at INTEGER,
    -- The monthly Spend Cap the Administrator sets. NULL sets no cap.
    monthly_spend_cap_usd REAL
);
CREATE UNIQUE INDEX idx_users_email ON users (email);
CREATE INDEX idx_users_org ON users (org_id);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users (id),
    token_hash TEXT NOT NULL UNIQUE,
    client_kind TEXT NOT NULL CHECK (client_kind IN ('browser', 'desktop')),
    client_name TEXT,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions (user_id);
CREATE INDEX idx_sessions_expires ON sessions (expires_at);

CREATE TABLE sign_in_links (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users (id),
    token_hash TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at INTEGER
);
CREATE INDEX idx_workspaces_user ON workspaces (user_id);

-- The Usage Record: one row for each model call, with the token counts
-- of the provider and the cost the price table of the serving model
-- makes of them. It names the Workspace that spent it and the Run that
-- asked. A NULL cost is an unknown cost: no layer of the model metadata
-- prices the model.
CREATE TABLE usage (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    run_id TEXT NOT NULL REFERENCES runs (id),
    provider TEXT,
    model TEXT,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cache_read_tokens INTEGER NOT NULL,
    cache_write_tokens INTEGER NOT NULL,
    cost_usd REAL,
    created_at INTEGER NOT NULL
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
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    name TEXT NOT NULL,
    platform TEXT NOT NULL,
    -- The declared capabilities, as a JSON array of strings.
    capabilities TEXT NOT NULL,
    last_seen_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_hosts_workspace_name ON hosts (workspace_id, name);

-- Triggers. Each one has a twin of the same name in the Postgres
-- baseline, where a plpgsql function holds its body.

-- A change of Connection access makes every sync resource of the
-- Connection catch up again.
CREATE TRIGGER knowledge_connection_access AFTER UPDATE OF status, authorized_capabilities ON connections
WHEN old.status IS NOT new.status OR old.authorized_capabilities IS NOT new.authorized_capabilities
BEGIN
    UPDATE sync_resources SET caught_up = 0, revision = revision + 1
    WHERE connection_id = new.id AND workspace_id = new.workspace_id;
END;

-- A new or changed Connection grant makes the sync resources of the
-- Agent that holds it catch up again.
CREATE TRIGGER knowledge_grant_insert AFTER INSERT ON grants
WHEN new.resource_kind = 'connection'
BEGIN
    UPDATE sync_resources SET caught_up = 0, revision = revision + 1
    WHERE connection_id = new.resource_id AND workspace_id = new.workspace_id
    AND json_extract(config, '$.agent_id') = new.agent_id;
END;

CREATE TRIGGER knowledge_grant_update AFTER UPDATE OF scope, revoked_at ON grants
WHEN new.resource_kind = 'connection' AND (old.scope IS NOT new.scope OR old.revoked_at IS NOT new.revoked_at)
BEGIN
    UPDATE sync_resources SET caught_up = 0, revision = revision + 1
    WHERE connection_id = new.resource_id AND workspace_id = new.workspace_id
    AND json_extract(config, '$.agent_id') = new.agent_id;
END;

-- The words of the user carry no source scope, so a user message gets
-- an empty exposure set when it lands.
CREATE TRIGGER message_exposure_set_insert AFTER INSERT ON messages
WHEN new.author_kind = 'user' BEGIN
    INSERT INTO message_exposure_sets(message_id) VALUES (new.id);
END;

-- A message whose exposure reaches a forgotten source is forgotten too.
CREATE TRIGGER forget_message_exposure AFTER INSERT ON message_exposures BEGIN
    INSERT OR IGNORE INTO forgotten_messages
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
    );
END;

-- A new version of a source item moves the knowledge generation of its
-- resource forward, and records the change. A retrieval that a Forget
-- blocks writes no version (ADR-0008).
CREATE TRIGGER knowledge_source_version_insert AFTER INSERT ON source_versions
BEGIN
    INSERT INTO knowledge_generations(workspace_id,connection_id,resource,generation)
    VALUES(new.workspace_id,new.connection_id,new.resource,1)
    ON CONFLICT(workspace_id,connection_id,resource) DO UPDATE SET generation=generation+1;
    INSERT INTO knowledge_invalidations(workspace_id,connection_id,resource,reason)
    VALUES(new.workspace_id,new.connection_id,new.resource,'source_changed');
END;

-- The message search index holds the complete messages that are not
-- progress lines and not forgotten.
CREATE TRIGGER conversation_message_search_insert AFTER INSERT ON messages
WHEN new.status = 'complete'
 AND json_extract(new.blocks, '$[0].type') IS NOT 'progress'
BEGIN
    INSERT INTO conversation_message_search(workspace_id, message_id, text_content)
    VALUES (new.workspace_id, new.id, new.text_content);
END;

CREATE TRIGGER conversation_message_search_update
AFTER UPDATE OF status, blocks, text_content ON messages
BEGIN
    DELETE FROM conversation_message_search WHERE message_id = old.id;
    INSERT INTO conversation_message_search(workspace_id, message_id, text_content)
    SELECT new.workspace_id, new.id, new.text_content
    WHERE new.status = 'complete'
      AND json_extract(new.blocks, '$[0].type') IS NOT 'progress'
      AND NOT EXISTS (
          SELECT 1 FROM forgotten_messages f WHERE f.message_id = new.id
      );
END;

CREATE TRIGGER conversation_message_search_delete AFTER DELETE ON messages
BEGIN
    DELETE FROM conversation_message_search WHERE message_id = old.id;
END;

CREATE TRIGGER conversation_message_search_forget
AFTER INSERT ON forgotten_messages
BEGIN
    DELETE FROM conversation_message_search WHERE message_id = new.message_id;
END;

-- A changed or revoked grant takes the messages it exposed out of the
-- index.
CREATE TRIGGER conversation_message_search_grant_change
AFTER UPDATE OF revision, revoked_at ON grants
BEGIN
    DELETE FROM conversation_message_search
    WHERE message_id IN (
        SELECT message_id FROM message_exposures WHERE grant_id = new.id
    );
END;

-- The tool search index follows the tool evidence rows. Forgetting a
-- message and changing a grant delete the evidence, and the delete
-- trigger then clears the index.
CREATE TRIGGER conversation_tool_search_insert
AFTER INSERT ON conversation_tool_evidence
BEGIN
    INSERT INTO conversation_tool_search(workspace_id, reference, content)
    VALUES (new.workspace_id, new.reference, new.content);
END;

CREATE TRIGGER conversation_tool_search_delete
AFTER DELETE ON conversation_tool_evidence
BEGIN
    DELETE FROM conversation_tool_search WHERE reference = old.reference;
END;

CREATE TRIGGER conversation_tool_search_forget
AFTER INSERT ON forgotten_messages
BEGIN
    DELETE FROM conversation_tool_evidence
    WHERE source_message_id = new.message_id;
END;

CREATE TRIGGER conversation_tool_search_grant_change
AFTER UPDATE OF revision, revoked_at ON grants
BEGIN
    DELETE FROM conversation_tool_evidence WHERE grant_id = new.id;
END;

-- A Continuation Record that summarized a forgotten message, or
-- read under a grant that changed, is deleted (ADR-0009).
CREATE TRIGGER continuation_checkpoint_forget
AFTER INSERT ON forgotten_messages
BEGIN
    DELETE FROM continuation_checkpoints
    WHERE (workspace_id, agent_id, channel_id, root_key) IN (
        SELECT workspace_id, agent_id, channel_id, root_key
        FROM continuation_checkpoint_messages
        WHERE message_id = new.message_id
    );
END;

CREATE TRIGGER continuation_checkpoint_grant_change
AFTER UPDATE OF revision, revoked_at ON grants
BEGIN
    DELETE FROM continuation_checkpoints
    WHERE (workspace_id, agent_id, channel_id, root_key) IN (
        SELECT workspace_id, agent_id, channel_id, root_key
        FROM continuation_checkpoint_exposures
        WHERE grant_id = new.id
    );
END;
