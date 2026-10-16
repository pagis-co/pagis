//! Disclosure storage has one owner and no copy in an asker's Workspace.
use super::Backend;
use pagis_core::{DisclosureCategory, DisclosureRule};

pub async fn disclosure_profile_and_rules_have_workspace_boundaries(backend: &Backend) {
    let a = backend.seeded_workspace().await;
    let b = backend.seeded_workspace().await;
    let store = &backend.stores().disclosure;
    assert_eq!(store.profile(&a.id).await.unwrap(), "");
    store.set_profile(&a.id, "I work on design.").await.unwrap();
    assert_eq!(store.profile(&a.id).await.unwrap(), "I work on design.");
    assert_eq!(store.profile(&b.id).await.unwrap(), "");
    let rule = DisclosureRule {
        asker_workspace_id: Some(b.id.clone()),
        category: DisclosureCategory::Mail,
    };
    store.add(&a.id, &rule).await.unwrap();
    store.add(&a.id, &rule).await.unwrap();
    assert_eq!(store.list(&a.id).await.unwrap(), vec![rule.clone()]);
    assert!(store.list(&b.id).await.unwrap().is_empty());
    store.remove(&b.id, &rule).await.unwrap();
    assert_eq!(store.list(&a.id).await.unwrap(), vec![rule.clone()]);
    store.remove(&a.id, &rule).await.unwrap();
    assert!(store.list(&a.id).await.unwrap().is_empty());
    let org = DisclosureRule {
        asker_workspace_id: None,
        category: DisclosureCategory::Memory,
    };
    store.add(&a.id, &org).await.unwrap();
    assert_eq!(store.list(&a.id).await.unwrap(), vec![org.clone()]);
    store.remove(&a.id, &org).await.unwrap();
    assert!(store.list(&a.id).await.unwrap().is_empty());
}
#[macro_export]
macro_rules! store_suite_disclosure {
    ($emit:path) => {
        $emit!(
            disclosure,
            disclosure_profile_and_rules_have_workspace_boundaries,
        );
    };
}
