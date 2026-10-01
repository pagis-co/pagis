-- The model step takes a key for each provider and checks each one, so
-- one Workspace holds one key check for each provider (ADR-0025).
--
-- The SQLite migration of the same number holds the same columns.

ALTER TABLE onboarding_model_verifications DROP CONSTRAINT onboarding_model_verifications_pkey;
ALTER TABLE onboarding_model_verifications ADD PRIMARY KEY (workspace_id, provider);
