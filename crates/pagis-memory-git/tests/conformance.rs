use pagis_testkit::memory_suite::{Harness, NoSuppression};
use std::sync::Arc;
fn memory(forget: Arc<NoSuppression>) -> Harness {
    let root = tempfile::tempdir().unwrap();
    let pages = Arc::new(pagis_core::VolatilePageIndex::default());
    let store = Arc::new(pagis_memory_git::GitMemoryStore::new(
        root.path(),
        forget.clone(),
        pages.clone(),
    ));
    let reopened = Arc::new(pagis_memory_git::GitMemoryStore::new(
        root.path(),
        forget,
        pages,
    ));
    Harness::new(store, reopened).with_resource(root)
}
pagis_testkit::memory_suite!(memory);
