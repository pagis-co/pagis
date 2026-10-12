use super::{
    FileStamp, HistoryFile, HistoryRevision, changeset_stamp, require_readable, require_revision,
    select_forget, withheld_paths,
};
use crate::{AgentId, GrantId, MemoryAccess, MemoryContentKind, MemoryExposure};
use std::collections::HashSet;

fn exposure(revision: i64) -> MemoryExposure {
    MemoryExposure {
        grant_id: GrantId::from("grant".to_string()),
        revision,
    }
}
fn stamp(revision: i64) -> FileStamp {
    FileStamp {
        exposures: vec![exposure(revision)],
        kind: MemoryContentKind::DependentView,
    }
}
#[cfg_attr(
    all(target_arch = "wasm32", target_os = "unknown"),
    wasm_bindgen_test::wasm_bindgen_test
)]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), test)]
fn reads_require_a_stamp_and_each_exact_grant_revision() {
    let agent = AgentId::from("sage".to_string());
    let allowed = MemoryAccess::agent(agent.clone(), [exposure(2)]);
    assert!(require_readable(&allowed, Some(&stamp(2))).is_ok());
    assert!(require_readable(&allowed, Some(&stamp(1))).is_err());
    assert!(require_readable(&allowed, None).is_err());
    assert!(require_readable(&MemoryAccess::Owner { agent_id: agent }, Some(&stamp(1))).is_ok());
    assert!(require_revision(&allowed, &[exposure(1)]).is_err());
}
#[cfg_attr(
    all(target_arch = "wasm32", target_os = "unknown"),
    wasm_bindgen_test::wasm_bindgen_test
)]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), test)]
fn a_changeset_keeps_all_old_stamps_and_the_current_access() {
    let agent = AgentId::from("sage".to_string());
    let old = stamp(1);
    let allowed = MemoryAccess::agent(agent.clone(), [exposure(1), exposure(2)]);
    let result = changeset_stamp(&allowed, [Some(&old), Some(&old)]).unwrap();
    assert_eq!(result.exposures, vec![exposure(1), exposure(2)]);
    assert_eq!(result.kind, MemoryContentKind::DependentView);
    let owner = MemoryAccess::Owner { agent_id: agent };
    assert_eq!(
        changeset_stamp(&owner, [Some(&old)]).unwrap().exposures,
        old.exposures
    );
    assert!(
        changeset_stamp(&owner, [None]).is_err(),
        "an existing file needs its stamp"
    );
    assert_eq!(
        changeset_stamp(&owner, []).unwrap().kind,
        MemoryContentKind::Procedure
    );
    assert!(
        changeset_stamp(
            &MemoryAccess::agent(AgentId::from("sage".to_string()), []),
            [Some(&old)]
        )
        .is_err()
    );
}
#[cfg_attr(
    all(target_arch = "wasm32", target_os = "unknown"),
    wasm_bindgen_test::wasm_bindgen_test
)]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), test)]
fn a_revoked_grant_withholds_each_of_its_stamped_paths() {
    let first = stamp(1);
    let second = stamp(2);
    let free = FileStamp {
        exposures: vec![],
        kind: MemoryContentKind::Procedure,
    };
    assert_eq!(
        withheld_paths(
            &exposure(1).grant_id,
            [
                ("shared/a.md", Some(&first)),
                ("shared/b.md", Some(&second)),
                ("shared/free.md", Some(&free)),
                ("shared/unknown.md", None)
            ]
        ),
        ["shared/a.md".to_string(), "shared/b.md".to_string()].into()
    );
}
#[cfg_attr(
    all(target_arch = "wasm32", target_os = "unknown"),
    wasm_bindgen_test::wasm_bindgen_test
)]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), test)]
fn forget_selects_files_and_sentences_across_history_without_removing_unstamped_files() {
    let history = [
        HistoryRevision {
            revision: "r1",
            scope: Some(0),
            files: vec![
                HistoryFile {
                    path: "shared/a.md",
                    scope: Some(1),
                },
                HistoryFile {
                    path: "unknown",
                    scope: None,
                },
            ],
        },
        HistoryRevision {
            revision: "r2",
            scope: Some(1),
            files: vec![
                HistoryFile {
                    path: "shared/a.md",
                    scope: Some(0),
                },
                HistoryFile {
                    path: "shared/b.md",
                    scope: Some(1),
                },
            ],
        },
        HistoryRevision {
            revision: "r3",
            scope: None,
            files: vec![],
        },
    ];
    let result = select_forget(&history, &HashSet::from([1]));
    assert_eq!(
        result.paths,
        ["shared/a.md".to_string(), "shared/b.md".to_string()].into()
    );
    assert_eq!(result.revisions.len(), 2);
    assert!(!result.revisions["r1"].withhold_message);
    assert!(result.revisions["r2"].withhold_message);
    assert_eq!(
        result.revisions["r1"].files,
        ["shared/a.md".to_string()].into()
    );
    assert_eq!(
        result.revisions["r2"].files,
        ["shared/b.md".to_string()].into()
    );
    assert!(
        select_forget(&history, &HashSet::new())
            .revisions
            .is_empty()
    );
}
