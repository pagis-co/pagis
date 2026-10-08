-- The token of the Harness Model Endpoint of a Coding Session (ADR-0033).
-- `model_token_hash` holds the SHA-256 of the token, in hexadecimal, and
-- never the token. A new token of the session writes its hash in place of
-- the old one, so the old token stops. NULL while the session has no
-- token.
--
-- The SQLite migration of the same number holds the same change.

ALTER TABLE coding_sessions ADD COLUMN model_token_hash TEXT COLLATE "C";
CREATE UNIQUE INDEX idx_coding_sessions_model_token ON coding_sessions (model_token_hash);
