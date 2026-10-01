-- The Person dismisses a failed Run or a missed Call from the
-- Needs-You Queue (ADR-0022). The time stays on the record, so the
-- item stays out of the queue after a reload and on every client.
--
-- The SQLite migration of the same number holds the same columns.

ALTER TABLE runs ADD COLUMN dismissed_at BIGINT;
ALTER TABLE calls ADD COLUMN dismissed_at BIGINT;
