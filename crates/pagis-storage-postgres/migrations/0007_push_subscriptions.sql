-- The Push Subscriptions of a Person (ADR-0030): the push endpoint of
-- one client, with its P-256 public key and its auth secret, as
-- base64url. A row belongs to one Session and ends with it: a sign-out,
-- a removal from the Sessions list and the expiry sweep delete the
-- Session row, and the cascade deletes its Push Subscriptions.
-- `last_sent_at` is NULL until the first Web Push.
--
-- The SQLite migration of the same number holds the same table.

CREATE TABLE push_subscriptions (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    session_id TEXT COLLATE "C" NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    endpoint TEXT COLLATE "C" NOT NULL UNIQUE,
    p256dh TEXT COLLATE "C" NOT NULL,
    auth TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL,
    last_sent_at BIGINT
);
CREATE INDEX idx_push_subscriptions_workspace ON push_subscriptions (workspace_id);
CREATE INDEX idx_push_subscriptions_session ON push_subscriptions (session_id);
