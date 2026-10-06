-- The Model Request Capture (ADR-0031): one model request of a Run and
-- the provider's answer, kept only while an Administrator has the System
-- Setting on. `request` and `answer` are JSON text. The retention sweep
-- deletes by `created_at` across every Workspace, Forget deletes by Run,
-- and a Backup leaves the rows of the table out of its dump.
--
-- The SQLite migration of the same number holds the same columns.

CREATE TABLE model_request_captures (
    id TEXT COLLATE "C" PRIMARY KEY,
    workspace_id TEXT COLLATE "C" NOT NULL REFERENCES workspaces (id),
    run_id TEXT COLLATE "C" NOT NULL REFERENCES runs (id),
    phase TEXT COLLATE "C" NOT NULL,
    phase_request BIGINT NOT NULL,
    request TEXT COLLATE "C" NOT NULL,
    answer TEXT COLLATE "C" NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE INDEX idx_model_request_captures_run ON model_request_captures (workspace_id, run_id);
CREATE INDEX idx_model_request_captures_created ON model_request_captures (created_at);
