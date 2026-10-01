-- The model step takes a key for each provider and checks each one, so
-- one Workspace holds one key check for each provider (ADR-0025).
-- SQLite cannot change a primary key in place: the rows move to a new
-- table with the wider key.
--
-- The Postgres migration of the same number holds the same columns.

CREATE TABLE onboarding_model_verifications_by_provider (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    available INTEGER NOT NULL,
    proof TEXT NOT NULL,
    PRIMARY KEY (workspace_id, provider)
);
INSERT INTO onboarding_model_verifications_by_provider (workspace_id, provider, available, proof)
    SELECT workspace_id, provider, available, proof FROM onboarding_model_verifications;
DROP TABLE onboarding_model_verifications;
ALTER TABLE onboarding_model_verifications_by_provider RENAME TO onboarding_model_verifications;
