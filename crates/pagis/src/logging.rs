use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

const KEEP_LOG_FILES: usize = 7;

/// The crates whose events follow the `log_level` System Setting. Every
/// other crate logs warnings and errors alone, so a start shows the
/// daemon's own lines and not the internals of a library. A target is a
/// prefix, so `pagis` holds every `pagis_*` crate.
const OWN_TARGETS: [&str; 2] = ["pagis", "llm_router"];

/// The filter of one daemon: `PAGIS_LOG` as it is, else the `log_level`
/// System Setting for the daemon's own crates and `warn` for the rest.
pub fn filter(log_level: &str) -> EnvFilter {
    EnvFilter::try_from_env("PAGIS_LOG").unwrap_or_else(|_| EnvFilter::new(directives(log_level)))
}

fn directives(log_level: &str) -> String {
    std::iter::once("warn".to_string())
        .chain(OWN_TARGETS.map(|target| format!("{target}={log_level}")))
        .collect::<Vec<_>>()
        .join(",")
}

/// Install the global tracing subscriber: stderr plus a daily-rotated
/// file in `<home>/logs` (7 files kept), under [`filter`] (ADR-0024).
/// Returns the guard that flushes the file writer; hold it for the
/// life of the process.
pub fn init(home: &Path, log_level: &str) -> anyhow::Result<WorkerGuard> {
    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("pagis")
        .filename_suffix("log")
        .max_log_files(KEEP_LOG_FILES)
        .build(home.join("logs"))?;
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(filter(log_level))
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(fmt::layer().with_ansi(false).with_writer(file_writer))
        .try_init()?;
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// What one scoped subscriber under the default filter writes.
    fn written(log_level: &str, emit: impl FnOnce()) -> String {
        let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer = Arc::clone(&buffer);
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new(directives(log_level)))
            .with(
                fmt::layer()
                    .with_ansi(false)
                    .with_writer(move || Writer(Arc::clone(&writer))),
            );
        tracing::subscriber::with_default(subscriber, emit);
        String::from_utf8(buffer.lock().unwrap().clone()).unwrap()
    }

    struct Writer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_library_logs_warnings_alone_at_the_default_level() {
        let out = written("info", || {
            tracing::info!(target: "tantivy::indexer::segment_updater", "save metas");
            tracing::warn!(target: "tantivy::directory", "a library warning");
            tracing::info!(target: "pagis_server::system", "a daemon line");
            tracing::info!(target: "llm_router::router", "a router line");
        });

        assert!(!out.contains("save metas"), "{out}");
        assert!(out.contains("a library warning"), "{out}");
        assert!(out.contains("a daemon line"), "{out}");
        assert!(out.contains("a router line"), "{out}");
    }

    #[test]
    fn the_setting_moves_the_daemon_level_alone() {
        let out = written("debug", || {
            tracing::debug!(target: "pagis_agent::run", "a daemon detail");
            tracing::debug!(target: "hyper::proto", "a library detail");
        });

        assert!(out.contains("a daemon detail"), "{out}");
        assert!(!out.contains("a library detail"), "{out}");
    }
}
