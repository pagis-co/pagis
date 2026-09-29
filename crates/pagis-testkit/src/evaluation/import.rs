//! The source acquisition wait of the release evaluation driver.

use std::time::{Duration, Instant};

use pagis_core::knowledge::{KnowledgeStore, SourceKey};

pub const IMPORT_TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(25);

/// Waits until one source has acquired and delivered all visible items.
pub struct ImportWait<'a> {
    store: &'a dyn KnowledgeStore,
    key: &'a SourceKey,
    expected: usize,
    budget: Duration,
}

impl<'a> ImportWait<'a> {
    pub fn new(
        store: &'a dyn KnowledgeStore,
        key: &'a SourceKey,
        expected: usize,
        budget: Duration,
    ) -> Self {
        Self {
            store,
            key,
            expected,
            budget,
        }
    }

    pub async fn settled(&self) -> Result<(), String> {
        let deadline = Instant::now() + self.budget;
        loop {
            let acquired = self
                .store
                .versions(self.key, 0, 64)
                .await
                .map_err(|error| error.to_string())?
                .len();
            let status = self
                .store
                .status(self.key, pagis_core::now_ms())
                .await
                .map_err(|error| error.to_string())?
                .ok_or("the acquisition source is not configured")?;
            let delivered = usize::try_from(status.arrival_processed)
                .is_ok_and(|processed| processed >= self.expected);
            if acquired >= self.expected
                && status.caught_up
                && status.arrival_pending == 0
                && delivered
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                let arriving = if status.caught_up {
                    ""
                } else {
                    ", and the source is still arriving"
                };
                return Err(format!(
                    "source acquisition did not settle within {} seconds: {acquired} of {} items acquired, {} arrivals processed{arriving}",
                    self.budget.as_secs(),
                    self.expected,
                    status.arrival_processed,
                ));
            }
            tokio::time::sleep(POLL).await;
        }
    }
}
