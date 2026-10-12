CREATE TABLE run_journal (
    run_id TEXT COLLATE "C" PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    turn BIGINT,
    state TEXT COLLATE "C",
    pending_turn TEXT COLLATE "C",
    resumed BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE TABLE run_journal_calls (
    run_id TEXT COLLATE "C" NOT NULL REFERENCES run_journal(run_id) ON DELETE CASCADE,
    call_id TEXT COLLATE "C" NOT NULL,
    effect_class TEXT COLLATE "C" NOT NULL,
    result TEXT COLLATE "C",
    PRIMARY KEY (run_id, call_id)
);
