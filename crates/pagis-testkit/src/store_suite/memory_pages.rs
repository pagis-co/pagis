//! The Page Index trait tests of the suite (ADR-0008).
//!
//! Each body is one test. It takes a [`Backend`], reads the store set
//! from it, and never names a pool type, so it runs on both backends
//! from one text. Name every new body in `store_suite_memory_pages!`
//! below: the guard test of the parent module fails while one is
//! missing.
//!
//! The two backends rank with different functions: SQLite with `bm25`
//! over an FTS5 table, Postgres with `ts_rank_cd` over a `tsvector`.
//! ADR-0008 therefore defines relevance by properties and not by one
//! engine's scoring, and no body here asserts an exact order of hits.
//! A body asserts the set of hits, or it compares the `rank` of two
//! hits, where a smaller rank is the better hit on both backends.
//!
//! The two tokenizers differ as well. SQLite uses the FTS5 porter
//! tokenizer and Postgres the `english` configuration, which also drops
//! stopwords, so a body searches for a word that no configuration
//! drops.

use pagis_core::memory_page::PageBrief;
use pagis_core::{
    GrantId, IndexedPage, MemoryAuthor, MemoryExposure, PageIndexHead, PageIndexUpdate, Workspace,
    WorkspaceId,
};

use super::Backend;

/// The Org, the administrator and one Workspace row, and the id of the
/// Workspace, which is all the Page Index takes.
async fn seed(backend: &Backend) -> WorkspaceId {
    backend.seeded_workspace().await.id
}

/// Two Workspaces of one person. Each holds its own Page Index rows.
async fn seed_pair(backend: &Backend) -> (WorkspaceId, WorkspaceId) {
    let first = backend.seeded_workspace().await;
    let second = Workspace {
        id: WorkspaceId::generate(),
        ..first.clone()
    };
    backend
        .stores()
        .workspaces
        .create(&second)
        .await
        .expect("write the second Workspace");
    (first.id, second.id)
}

fn change(path: &str, position: u64, by: &str) -> IndexedPage {
    IndexedPage {
        path: path.to_string(),
        position,
        changed_at: 1_000 * position as i64,
        changed_by: MemoryAuthor {
            name: by.to_string(),
            email: format!("{}@agents.pagis.local", by.to_lowercase()),
        },
        title: path.to_string(),
        body: String::new(),
        kind: None,
        source_connection_id: None,
        exposures: Some(Vec::new()),
        brief: None,
        links: Vec::new(),
    }
}

fn update(revision: &str, next_position: u64, changed: Vec<IndexedPage>) -> PageIndexUpdate {
    PageIndexUpdate {
        replace: false,
        head: PageIndexHead {
            revision: revision.to_string(),
            next_position,
        },
        changed,
        restamped: Vec::new(),
        removed: Vec::new(),
    }
}

pub async fn search_finds_a_word_in_the_body_and_returns_a_snippet(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    let page = IndexedPage {
        title: "Vignesh".to_string(),
        body: "Vignesh can be reached at the heliotrope office.".to_string(),
        ..change("shared/vignesh-contact.md", 0, "Sage")
    };
    index
        .apply(&ws, &update("r1", 1, vec![page]))
        .await
        .unwrap();

    let hits = index
        .search(&ws, "shared/", "heliotrope", 10)
        .await
        .unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].page.path, "shared/vignesh-contact.md");
    assert!(hits[0].snippet.contains("heliotrope"));
}

pub async fn search_finds_a_page_that_holds_only_one_of_the_words(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    let contact = IndexedPage {
        body: "J has a contact named Vignesh at +1 555 0100.".to_string(),
        ..change("shared/vignesh-contact.md", 0, "Sage")
    };
    let both = IndexedPage {
        body: "The phone of Vignesh was busy.".to_string(),
        ..change("shared/call.md", 1, "Sage")
    };
    index
        .apply(&ws, &update("r1", 2, vec![contact, both]))
        .await
        .unwrap();

    let hits = index
        .search(&ws, "shared/", "Vignesh phone number", 10)
        .await
        .unwrap();

    // The words are joined with OR, so the page that holds one of them
    // is found beside the page that holds two. Which of the two ranks
    // higher is the scoring function of the backend, so this body holds
    // the set of hits and not their order.
    let mut paths: Vec<_> = hits.iter().map(|hit| hit.page.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["shared/call.md", "shared/vignesh-contact.md"]);
}

pub async fn search_finds_another_form_of_a_word(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    let page = IndexedPage {
        body: "The car insurance was renewed in December.".to_string(),
        ..change("shared/car-insurance.md", 0, "Sage")
    };
    index
        .apply(&ws, &update("r1", 1, vec![page]))
        .await
        .unwrap();

    let hits = index.search(&ws, "shared/", "renewal", 10).await.unwrap();

    assert_eq!(hits.len(), 1);
}

/// A page whose title holds the word outranks a page whose body holds
/// it, and nothing else differs: each page holds the word once and the
/// same filler in the other field. A smaller `rank` is the better hit
/// on both backends, so the body compares the two ranks and asserts no
/// rank value.
pub async fn search_orders_a_title_match_before_a_body_match(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    let in_title = IndexedPage {
        title: "Heliotrope".to_string(),
        body: "A memory page.".to_string(),
        ..change("shared/a.md", 0, "Sage")
    };
    let in_body = IndexedPage {
        title: "A memory page.".to_string(),
        body: "Heliotrope".to_string(),
        ..change("shared/b.md", 0, "Sage")
    };
    index
        .apply(&ws, &update("r1", 1, vec![in_body, in_title]))
        .await
        .unwrap();

    let hits = index
        .search(&ws, "shared/", "heliotrope", 10)
        .await
        .unwrap();

    let ranked: Vec<_> = hits
        .iter()
        .map(|hit| (hit.page.path.as_str(), hit.rank))
        .collect();
    assert_eq!(hits.len(), 2, "{ranked:?}");
    assert!(
        hits[0].rank < hits[1].rank,
        "the title match is the better hit, and a smaller rank is a better hit: {ranked:?}"
    );
    assert_eq!(hits[0].page.path, "shared/a.md", "{ranked:?}");
}

pub async fn changed_removed_and_replace_updates_change_search_results(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    let old = IndexedPage {
        body: "oldword".to_string(),
        ..change("shared/a.md", 0, "Sage")
    };
    let removed = IndexedPage {
        body: "removeword".to_string(),
        ..change("shared/b.md", 0, "Sage")
    };
    index
        .apply(&ws, &update("r1", 1, vec![old, removed]))
        .await
        .unwrap();
    let changed = IndexedPage {
        body: "newword".to_string(),
        ..change("shared/a.md", 1, "Sage")
    };
    index
        .apply(
            &ws,
            &PageIndexUpdate {
                removed: vec!["shared/b.md".to_string()],
                ..update("r2", 2, vec![changed])
            },
        )
        .await
        .unwrap();

    assert!(
        index
            .search(&ws, "shared/", "oldword", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        index
            .search(&ws, "shared/", "removeword", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        index
            .search(&ws, "shared/", "newword", 10)
            .await
            .unwrap()
            .len(),
        1
    );

    let kept = IndexedPage {
        body: "keptword".to_string(),
        ..change("shared/kept.md", 0, "Sage")
    };
    index
        .apply(
            &ws,
            &PageIndexUpdate {
                replace: true,
                ..update("r3", 1, vec![kept])
            },
        )
        .await
        .unwrap();
    assert!(
        index
            .search(&ws, "shared/", "newword", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        index
            .search(&ws, "shared/", "keptword", 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

pub async fn search_handles_punctuation_and_isolates_workspace_and_root(backend: &Backend) {
    let ws = seed(backend).await;
    let other = seed(backend).await;
    let index = &backend.stores().memory_pages;
    for (workspace, path) in [
        (&ws, "shared/found.md"),
        (&ws, "agents/a/private.md"),
        (&other, "shared/other.md"),
    ] {
        let page = IndexedPage {
            body: "isolationword".to_string(),
            ..change(path, 0, "Sage")
        };
        index
            .apply(workspace, &update("r1", 1, vec![page]))
            .await
            .unwrap();
    }

    assert!(
        index
            .search(&ws, "shared/", "... !!!", 10)
            .await
            .unwrap()
            .is_empty()
    );
    let paths: Vec<_> = index
        .search(&ws, "shared/", "isolationword", 10)
        .await
        .unwrap()
        .into_iter()
        .map(|hit| hit.page.path)
        .collect();
    assert_eq!(paths, vec!["shared/found.md"]);
}

pub async fn an_index_with_no_update_has_no_head(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;

    assert_eq!(index.head(&ws).await.unwrap(), None);
    assert!(index.pages(&ws, "shared/").await.unwrap().is_empty());
}

pub async fn an_update_moves_the_head_and_a_newer_change_replaces_the_row(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    index
        .apply(
            &ws,
            &update(
                "r1",
                2,
                vec![
                    change("shared/a.md", 0, "Sage"),
                    change("shared/b.md", 1, "Sage"),
                    change("agents/ag1/craft.md", 1, "Sage"),
                ],
            ),
        )
        .await
        .unwrap();
    index
        .apply(
            &ws,
            &update("r2", 3, vec![change("shared/a.md", 2, "Clown")]),
        )
        .await
        .unwrap();

    assert_eq!(
        index.head(&ws).await.unwrap(),
        Some(PageIndexHead {
            revision: "r2".to_string(),
            next_position: 3,
        })
    );
    assert_eq!(
        index.pages(&ws, "shared/").await.unwrap(),
        vec![
            change("shared/a.md", 2, "Clown"),
            change("shared/b.md", 1, "Sage")
        ],
        "the newest change is first, and another root stays out"
    );
}

pub async fn changes_of_one_commit_come_in_path_order(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    index
        .apply(
            &ws,
            &update(
                "r1",
                1,
                vec![
                    change("shared/b.md", 0, "Sage"),
                    change("shared/a.md", 0, "Sage"),
                ],
            ),
        )
        .await
        .unwrap();

    let paths: Vec<_> = index
        .pages(&ws, "shared/")
        .await
        .unwrap()
        .into_iter()
        .map(|change| change.path)
        .collect();
    assert_eq!(paths, vec!["shared/a.md", "shared/b.md"]);
}

pub async fn a_root_with_a_like_wildcard_matches_only_itself(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    index
        .apply(
            &ws,
            &update(
                "r1",
                1,
                vec![
                    change("agents/a_1/craft.md", 0, "Sage"),
                    change("agents/ab1/craft.md", 0, "Sage"),
                ],
            ),
        )
        .await
        .unwrap();

    let paths: Vec<_> = index
        .pages(&ws, "agents/a_1/")
        .await
        .unwrap()
        .into_iter()
        .map(|change| change.path)
        .collect();
    assert_eq!(paths, vec!["agents/a_1/craft.md"]);
}

pub async fn a_removed_path_leaves_the_index(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    index
        .apply(
            &ws,
            &update(
                "r1",
                1,
                vec![
                    change("shared/a.md", 0, "Sage"),
                    change("shared/b.md", 0, "Sage"),
                ],
            ),
        )
        .await
        .unwrap();
    index
        .apply(
            &ws,
            &PageIndexUpdate {
                removed: vec!["shared/a.md".to_string()],
                ..update("r2", 2, Vec::new())
            },
        )
        .await
        .unwrap();

    assert_eq!(
        index.pages(&ws, "shared/").await.unwrap(),
        vec![change("shared/b.md", 0, "Sage")]
    );
}

pub async fn a_replace_update_drops_the_earlier_rows_of_its_workspace_only(backend: &Backend) {
    let ws = seed(backend).await;
    let other = seed(backend).await;
    let index = &backend.stores().memory_pages;
    for workspace_id in [&ws, &other] {
        index
            .apply(
                workspace_id,
                &update("r1", 1, vec![change("shared/forgotten.md", 0, "Sage")]),
            )
            .await
            .unwrap();
    }

    index
        .apply(
            &ws,
            &PageIndexUpdate {
                replace: true,
                ..update("rebuilt", 1, vec![change("shared/kept.md", 0, "Sage")])
            },
        )
        .await
        .unwrap();

    assert_eq!(
        index.pages(&ws, "shared/").await.unwrap(),
        vec![change("shared/kept.md", 0, "Sage")]
    );
    assert_eq!(index.pages(&other, "shared/").await.unwrap().len(), 1);
}

pub async fn a_page_keeps_its_summary_and_its_stamp(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    let exposure = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 3,
    };
    let subject = IndexedPage {
        title: "Priya Raman".to_string(),
        kind: Some("Person".to_string()),
        source_connection_id: Some("conn-1".to_string()),
        exposures: Some(vec![exposure]),
        brief: Some(PageBrief {
            words: vec!["pilot".to_string(), "priya".to_string()],
        }),
        ..change("agents/ag1/subjects/gmail/priya.md", 1, "Pagis")
    };
    let unstamped = IndexedPage {
        exposures: None,
        ..change("agents/ag1/legacy.md", 0, "Sage")
    };
    index
        .apply(
            &ws,
            &update("r1", 2, vec![subject.clone(), unstamped.clone()]),
        )
        .await
        .unwrap();

    assert_eq!(
        index.pages(&ws, "agents/ag1/").await.unwrap(),
        vec![subject, unstamped]
    );
}

pub async fn a_restamp_changes_the_exposures_and_keeps_the_last_change(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    let legacy = IndexedPage {
        exposures: None,
        ..change("shared/legacy.md", 0, "Sage")
    };
    index
        .apply(&ws, &update("r1", 1, vec![legacy.clone()]))
        .await
        .unwrap();

    index
        .apply(
            &ws,
            &PageIndexUpdate {
                restamped: vec![("shared/legacy.md".to_string(), Some(Vec::new()))],
                ..update("r2", 2, Vec::new())
            },
        )
        .await
        .unwrap();

    assert_eq!(
        index.pages(&ws, "shared/").await.unwrap(),
        vec![IndexedPage {
            exposures: Some(Vec::new()),
            ..legacy
        }]
    );
}

pub async fn a_subject_page_with_no_words_keeps_its_brief(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    let empty = IndexedPage {
        brief: Some(PageBrief { words: Vec::new() }),
        ..change("agents/ag1/subjects/gmail/empty.md", 0, "Pagis")
    };
    index
        .apply(&ws, &update("r1", 1, vec![empty.clone()]))
        .await
        .unwrap();

    assert_eq!(index.pages(&ws, "agents/ag1/").await.unwrap(), vec![empty]);
}

/// The index keeps one edge for each link, and it keeps an edge whose
/// target no page holds: that edge marks a page worth writing later.
pub async fn the_index_keeps_an_edge_to_a_page_that_does_not_exist(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    let priya = IndexedPage {
        links: vec![
            "agents/ag1/subjects/gmail/northwind.md".to_string(),
            "shared/unwritten.md".to_string(),
        ],
        ..change("agents/ag1/subjects/gmail/priya.md", 0, "Pagis")
    };
    let northwind = change("agents/ag1/subjects/gmail/northwind.md", 1, "Pagis");
    index
        .apply(&ws, &update("r1", 2, vec![priya.clone(), northwind]))
        .await
        .unwrap();

    let pages = index.pages(&ws, "agents/ag1/").await.unwrap();
    let read = pages
        .iter()
        .find(|page| page.path == priya.path)
        .expect("the page");
    assert_eq!(read.links, priya.links);

    // Whether a target exists is not a column: a reader computes it
    // from the rows it has.
    let held: Vec<&str> = pages.iter().map(|page| page.path.as_str()).collect();
    assert!(held.contains(&"agents/ag1/subjects/gmail/northwind.md"));
    assert!(!held.contains(&"shared/unwritten.md"));
}

/// The links of a page are derived data: a later write replaces them,
/// and a removal takes them with the row.
pub async fn the_links_of_a_page_follow_its_row(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    let first = IndexedPage {
        links: vec!["shared/one.md".to_string()],
        ..change("agents/ag1/a.md", 0, "Pagis")
    };
    index
        .apply(&ws, &update("r1", 1, vec![first.clone()]))
        .await
        .unwrap();

    let second = IndexedPage {
        links: vec!["shared/two.md".to_string()],
        ..change("agents/ag1/a.md", 1, "Pagis")
    };
    index
        .apply(&ws, &update("r2", 2, vec![second]))
        .await
        .unwrap();
    assert_eq!(
        index.pages(&ws, "agents/ag1/").await.unwrap()[0].links,
        vec!["shared/two.md".to_string()],
        "the newer write replaces the links of the page"
    );

    index
        .apply(
            &ws,
            &PageIndexUpdate {
                removed: vec!["agents/ag1/a.md".to_string()],
                ..update("r3", 3, Vec::new())
            },
        )
        .await
        .unwrap();
    index
        .apply(
            &ws,
            &update("r4", 4, vec![change("agents/ag1/a.md", 3, "Pagis")]),
        )
        .await
        .unwrap();

    assert!(
        index.pages(&ws, "agents/ag1/").await.unwrap()[0]
            .links
            .is_empty(),
        "a removed page leaves no link behind"
    );
}

pub async fn the_links_of_a_page_come_back_in_the_order_the_page_names_them(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    // The targets are in the reverse of their alphabetical order, so a
    // read that sorts them, or that takes the order the database
    // happens to give, does not pass.
    let named = vec![
        "shared/zebra.md".to_string(),
        "shared/apple.md".to_string(),
        "shared/mango.md".to_string(),
    ];
    index
        .apply(
            &ws,
            &update(
                "r1",
                1,
                vec![IndexedPage {
                    links: named.clone(),
                    ..change("agents/ag1/a.md", 0, "Pagis")
                }],
            ),
        )
        .await
        .unwrap();

    assert_eq!(
        index.pages(&ws, "agents/ag1/").await.unwrap()[0].links,
        named,
        "a page links in the order it names its pages, so the one hop \
         of a Brief takes the same target on every read"
    );
}

/// Memory Search holds the tenant in its own row, so a search of
/// one workspace never reads the pages of another workspace. The two
/// Workspaces hold the same words, so only the tenant column can keep
/// them apart.
pub async fn search_of_one_workspace_does_not_reach_another_workspace(backend: &Backend) {
    let (first, second) = seed_pair(backend).await;
    let index = &backend.stores().memory_pages;
    index
        .apply(
            &first,
            &update(
                "r1",
                1,
                vec![IndexedPage {
                    body: "The heliotrope plan of the first workspace.".to_string(),
                    ..change("shared/first.md", 0, "Sage")
                }],
            ),
        )
        .await
        .unwrap();
    index
        .apply(
            &second,
            &update(
                "r1",
                1,
                vec![IndexedPage {
                    body: "The heliotrope plan of the second workspace.".to_string(),
                    ..change("shared/second.md", 0, "Sage")
                }],
            ),
        )
        .await
        .unwrap();

    let first_paths: Vec<String> = index
        .search(&first, "shared/", "heliotrope", 10)
        .await
        .unwrap()
        .into_iter()
        .map(|hit| hit.page.path)
        .collect();
    let second_paths: Vec<String> = index
        .search(&second, "shared/", "heliotrope", 10)
        .await
        .unwrap()
        .into_iter()
        .map(|hit| hit.page.path)
        .collect();

    assert_eq!(first_paths, vec!["shared/first.md"]);
    assert_eq!(second_paths, vec!["shared/second.md"]);
}

/// A word of the entity words of a page — the front matter and the
/// first paragraph a Brief reads — is found by a search for that word.
///
/// The entity words have no column of their own in a search row. The
/// complete file is the text of the index (ADR-0008), so an alias in
/// the front matter is searchable through the body.
pub async fn an_entity_word_of_a_page_is_found_by_a_search(backend: &Backend) {
    let ws = seed(backend).await;
    let index = &backend.stores().memory_pages;
    let priya = IndexedPage {
        title: "Priya Raman".to_string(),
        kind: Some("Person".to_string()),
        body: "---\naliases: Northwind Trading\nkind: Person\n---\n\n\
               Priya Raman is the pilot of the Northwind account.\n"
            .to_string(),
        brief: Some(PageBrief {
            words: vec!["northwind".to_string(), "pilot".to_string()],
        }),
        ..change("agents/ag1/subjects/gmail/priya.md", 0, "Pagis")
    };
    index
        .apply(&ws, &update("r1", 1, vec![priya.clone()]))
        .await
        .unwrap();

    for word in &priya.brief.as_ref().expect("the Brief of the page").words {
        let paths: Vec<String> = index
            .search(&ws, "agents/ag1/", word, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|hit| hit.page.path)
            .collect();
        assert_eq!(
            paths,
            vec![priya.path.clone()],
            "a search for the entity word {word:?} finds the page"
        );
    }
}

/// One person's relevance and another person's corpus (ADR-0008).
///
/// The property is that one person's ranks do not move when another
/// person writes. It holds on one backend and not on the
/// other, so this body says which:
///
/// - **Postgres** ranks with `ts_rank_cd`, which reads the row it scores
///   and takes no corpus statistics. Another Workspace's rows cannot move
///   a rank, so the body asserts person A's hit order is the same before
///   and after person B adds a large corpus of the same words.
/// - **SQLite** ranks with `bm25()`, whose term statistics are those of
///   the whole FTS5 table, which holds every Workspace's rows. So another
///   person's corpus does move the numbers, and ADR-0008 does not claim
///   the property there. What this body asserts on SQLite is the
///   property that matters on both backends — B's pages never appear in
///   A's results, whatever B writes.
pub async fn one_persons_corpus_does_not_move_another_persons_results(backend: &Backend) {
    let (a, b) = seed_pair(backend).await;
    let index = &backend.stores().memory_pages;

    // Person A holds three pages that all answer "harbour pilot".
    let a_pages = vec![
        IndexedPage {
            title: "Harbour pilot".to_string(),
            body: "The harbour pilot boards outside the mole.".to_string(),
            ..change("shared/pilot.md", 0, "Sage")
        },
        IndexedPage {
            body: "The harbour pilot answers on channel twelve.".to_string(),
            ..change("shared/channels.md", 1, "Sage")
        },
        IndexedPage {
            body: "A pilot is aboard for the harbour approach.".to_string(),
            ..change("shared/approach.md", 2, "Sage")
        },
    ];
    index.apply(&a, &update("r1", 3, a_pages)).await.unwrap();
    let before: Vec<String> = index
        .search(&a, "shared/", "harbour pilot", 10)
        .await
        .unwrap()
        .into_iter()
        .map(|hit| hit.page.path)
        .collect();
    assert_eq!(before.len(), 3, "{before:?}");

    // Person B writes a large corpus of the same words.
    let b_pages: Vec<IndexedPage> = (0..40)
        .map(|index| IndexedPage {
            title: "Harbour pilot".to_string(),
            body: "harbour pilot harbour pilot harbour pilot".to_string(),
            ..change(&format!("shared/b-{index}.md"), index, "Bo")
        })
        .collect();
    index.apply(&b, &update("r1", 40, b_pages)).await.unwrap();

    let after: Vec<String> = index
        .search(&a, "shared/", "harbour pilot", 10)
        .await
        .unwrap()
        .into_iter()
        .map(|hit| hit.page.path)
        .collect();

    // On both backends: B's pages are not in A's results.
    assert!(
        after.iter().all(|path| !path.starts_with("shared/b-")),
        "person B's pages reached person A's results: {after:?}"
    );
    assert_eq!(
        after.len(),
        before.len(),
        "person A lost or gained a hit: {after:?}"
    );

    if backend.is_postgres() {
        // `ts_rank_cd` reads one row, so the order is the same order.
        assert_eq!(
            after, before,
            "person B's corpus moved person A's hit order on Postgres"
        );
    } else {
        // `bm25` reads the whole table's term statistics, so the order may
        // move. The same set of pages is what SQLite promises (ADR-0008).
        let mut sorted_before = before.clone();
        let mut sorted_after = after.clone();
        sorted_before.sort();
        sorted_after.sort();
        assert_eq!(
            sorted_after, sorted_before,
            "person A's own pages changed on SQLite: {after:?}"
        );
    }
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_memory_pages {
    ($emit:path) => {
        $emit!(
            memory_pages,
            search_finds_a_word_in_the_body_and_returns_a_snippet,
            search_finds_a_page_that_holds_only_one_of_the_words,
            search_finds_another_form_of_a_word,
            search_orders_a_title_match_before_a_body_match,
            changed_removed_and_replace_updates_change_search_results,
            search_handles_punctuation_and_isolates_workspace_and_root,
            an_index_with_no_update_has_no_head,
            an_update_moves_the_head_and_a_newer_change_replaces_the_row,
            changes_of_one_commit_come_in_path_order,
            a_root_with_a_like_wildcard_matches_only_itself,
            a_removed_path_leaves_the_index,
            a_replace_update_drops_the_earlier_rows_of_its_workspace_only,
            a_page_keeps_its_summary_and_its_stamp,
            a_restamp_changes_the_exposures_and_keeps_the_last_change,
            a_subject_page_with_no_words_keeps_its_brief,
            the_index_keeps_an_edge_to_a_page_that_does_not_exist,
            the_links_of_a_page_follow_its_row,
            the_links_of_a_page_come_back_in_the_order_the_page_names_them,
            search_of_one_workspace_does_not_reach_another_workspace,
            an_entity_word_of_a_page_is_found_by_a_search,
            one_persons_corpus_does_not_move_another_persons_results,
        );
    };
}
