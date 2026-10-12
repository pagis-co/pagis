CREATE TABLE run_journal (
    run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    turn BIGINT,
    state TEXT,
    pending_turn TEXT,
    resumed BIGINT NOT NULL DEFAULT 0
);
CREATE TABLE run_journal_calls (
    run_id TEXT NOT NULL REFERENCES run_journal(run_id) ON DELETE CASCADE,
    call_id TEXT NOT NULL,
    effect_class TEXT NOT NULL,
    result TEXT,
    PRIMARY KEY (run_id, call_id)
);
