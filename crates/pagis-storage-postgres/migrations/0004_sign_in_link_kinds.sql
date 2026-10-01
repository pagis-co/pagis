-- A Sign-In Link records where it is spent (ADR-0028). The start link,
-- which the `pagis` binary prints for a browser on its own machine,
-- opens through `GET /api/v1/sessions/link/{code}`. A link of the Public
-- Origin opens at `<public origin>/sign-in` on any machine. Each route
-- spends a link of its own kind alone.
--
-- Until now the table held start links alone, which live one minute, so
-- the table is made again with the kind and no default for it.
--
-- The SQLite migration of the same number holds the same columns.

DROP TABLE sign_in_links;
CREATE TABLE sign_in_links (
    id TEXT COLLATE "C" PRIMARY KEY,
    user_id TEXT COLLATE "C" NOT NULL REFERENCES users (id),
    token_hash TEXT COLLATE "C" NOT NULL UNIQUE,
    kind TEXT COLLATE "C" NOT NULL CHECK (kind IN ('start', 'public_origin')),
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    used_at BIGINT
);
