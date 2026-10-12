use pagis_testkit::memory_suite::{Harness, NoSuppression};
use std::sync::Arc;
fn memory(forget: Arc<NoSuppression>) -> Harness {
    let store = Arc::new(pagis_testkit::FakeMemoryStore::new(forget));
    Harness::new(store.clone(), store)
}
pagis_testkit::memory_suite!(memory);
