//! Logging wiring: init writes a rolling file under `<home>/logs`, with
//! the daemon's own lines and not a library's.
//! One test only — the tracing subscriber is a process-wide global.

#[test]
fn init_writes_a_rolling_log_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("logs")).unwrap();

    let guard = pagis::logging::init(dir.path(), "info").unwrap();
    tracing::info!(target: "pagis", "boot log line");
    tracing::info!(target: "tantivy::indexer", "library log line");
    drop(guard);

    let entries: Vec<_> = std::fs::read_dir(dir.path().join("logs"))
        .unwrap()
        .map(|e| e.unwrap())
        .collect();
    assert_eq!(entries.len(), 1);
    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    assert!(content.contains("boot log line"));
    assert!(!content.contains("library log line"));
}
