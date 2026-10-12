-- The retry state of each Workspace's collector targets.
ALTER TABLE event_subscriptions
    ADD COLUMN failures INTEGER NOT NULL DEFAULT 0 CHECK (failures >= 0);
ALTER TABLE event_subscriptions
    ADD COLUMN next_attempt_at INTEGER NOT NULL DEFAULT 0;
