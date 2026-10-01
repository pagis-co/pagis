//! The lines that the code under test logs.

use std::sync::{Arc, Mutex};

/// What `run` gives, and every line that the daemon logs at `level` or
/// louder while it runs. The capture holds for this thread, and a test
/// runtime has one thread, so the tasks that `run` spawns log here too.
pub async fn logged<T>(
    level: tracing::Level,
    run: impl std::future::Future<Output = T>,
) -> (T, String) {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let writer = Arc::clone(&buffer);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(level)
        .with_writer(move || Log(Arc::clone(&writer)))
        .finish();
    let value = {
        let _capture = tracing::subscriber::set_default(subscriber);
        run.await
    };
    let log = String::from_utf8(buffer.lock().expect("the log").clone()).expect("the log is text");
    (value, log)
}

struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("the log").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
