-- The Model Request Capture (ADR-0030): one model request of a Run and
-- the provider's answer, kept only while an Administrator has the System
-- Setting on. `request` and `answer` are JSON text. The retention sweep
-- deletes by `created_at` across every Workspace, Forget deletes by Run,
-- and a Backup deletes every row from its copy of the database.
--
-- The Postgres migration of the same number holds the same columns.

CREATE TABLE model_request_captures (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id),
    run_id TEXT NOT NULL REFERENCES runs (id),
    phase TEXT NOT NULL,
    phase_request INTEGER NOT NULL,
    request TEXT NOT NULL,
    answer TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_model_request_captures_run ON model_request_captures (workspace_id, run_id);
CREATE INDEX idx_model_request_captures_created ON model_request_captures (created_at);
