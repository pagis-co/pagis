-- One row for each installation of the Mobile App that registers with
-- the Push Relay (ADR-0030).
--
-- `id` is the random id in the endpoint. The endpoint never holds the
-- device token, so a leaked endpoint names no phone. `secret_hash` is
-- the SHA-256 of the secret that the installation sends to change or
-- remove its registration; the relay keeps no secret it can give back.
-- `vapid_key` is the uncompressed P-256 public key that a push to the
-- endpoint must be signed with. `pushes_today` counts the pushes of the
-- UTC day `day` (`YYYY-MM-DD`). Times are Unix milliseconds.

CREATE TABLE registrations (
    id TEXT PRIMARY KEY NOT NULL,
    secret_hash BLOB NOT NULL CHECK (length(secret_hash) = 32),
    platform TEXT NOT NULL CHECK (platform IN ('ios', 'android')),
    environment TEXT CHECK (
        (platform = 'ios' AND environment IN ('production', 'sandbox'))
        OR (platform = 'android' AND environment IS NULL)
    ),
    token TEXT NOT NULL,
    vapid_key BLOB NOT NULL CHECK (length(vapid_key) = 65),
    created_at INTEGER NOT NULL,
    last_push_at INTEGER,
    pushes_today INTEGER NOT NULL DEFAULT 0,
    day TEXT
) STRICT;
