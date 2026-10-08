-- The source of an Event Subscription, a Source Batch and an Incoming
-- Event is a Connection or a Coding Session (ADR-0006, ADR-0033). Each
-- row names exactly one of the two. An Incoming Event is unique by its
-- source, its kind and its provider event id. The rows that the tables
-- hold keep their Connection.
--
-- The SQLite migration of the same number holds the same columns.

ALTER TABLE event_subscriptions
    ALTER COLUMN connection_id DROP NOT NULL,
    ADD COLUMN coding_session_id TEXT COLLATE "C" REFERENCES coding_sessions (id),
    ADD CHECK ((connection_id IS NULL) <> (coding_session_id IS NULL));
CREATE INDEX idx_subscriptions_coding_session
    ON event_subscriptions (coding_session_id, event_kind, state);

ALTER TABLE source_batches
    ALTER COLUMN connection_id DROP NOT NULL,
    ADD COLUMN coding_session_id TEXT COLLATE "C" REFERENCES coding_sessions (id),
    ADD CHECK ((connection_id IS NULL) <> (coding_session_id IS NULL));
CREATE INDEX idx_batches_coding_session
    ON source_batches (coding_session_id, event_kind, id DESC);

ALTER TABLE incoming_events
    ALTER COLUMN connection_id DROP NOT NULL,
    ADD COLUMN coding_session_id TEXT COLLATE "C" REFERENCES coding_sessions (id),
    ADD CHECK ((connection_id IS NULL) <> (coding_session_id IS NULL)),
    ADD UNIQUE (coding_session_id, event_kind, provider_event_id);
CREATE INDEX idx_incoming_events_coding_session
    ON incoming_events (coding_session_id, event_kind, id DESC);
