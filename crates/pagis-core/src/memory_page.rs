//! What the Memory page shows about one file: its title and
//! kind from the front matter, and the sources that wrote its
//! Timeline.

use std::collections::BTreeMap;

use crate::continuation::ContinuationState;
use crate::memory::{MEMORY_INDEX_FILE, MemoryScope, ScopedPath};
use crate::subject_page::{
    Fact, FactStatus, FrontMatter, SubjectPage, TimelineEntry, brief_words, fact_file_words,
    page_links,
};

/// The title and kind of one memory file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageHeading {
    pub title: String,
    /// One of [`crate::subject_page::PAGE_KINDS`], or another word the
    /// page declares. A word outside the vocabulary is kept as it is.
    /// `None` when the page declares no kind.
    pub kind: Option<String>,
}

impl PageHeading {
    /// Read the heading of one file. The front matter block gives
    /// `title` and `kind`. A page whose block has no title is named
    /// after its file stem.
    pub fn parse(path: &str, content: &str) -> Self {
        let (front_matter, _) = FrontMatter::split(content);
        Self {
            title: front_matter.title.unwrap_or_else(|| file_stem(path)),
            kind: front_matter.kind,
        }
    }
}

/// What the Page Index holds about the content of one file: the
/// heading for a page list, the connection that wrote most of the
/// Timeline of a Subject Page, and what the selection of a Brief needs.
/// A reader of the index does not read the file (ADR-0008).
#[derive(Debug, Clone, PartialEq)]
pub struct PageSummary {
    pub title: String,
    pub kind: Option<String>,
    /// `None` for a page written from conversations.
    pub source_connection_id: Option<String>,
    /// `None` for a memory index, which a Brief never shows. Every
    /// other memory file is a Brief candidate.
    pub brief: Option<PageBrief>,
    /// The pages this page links to, as repository paths, in the order
    /// they stand and without a repetition.
    pub links: Vec<String>,
}

/// What the selection of a Brief needs about one memory file.
#[derive(Debug, Clone, PartialEq)]
pub struct PageBrief {
    /// See [`crate::subject_page::brief_words`] and
    /// [`crate::subject_page::fact_file_words`]. The words are in
    /// order, so two reads of one page are equal.
    pub words: Vec<String>,
}

impl PageSummary {
    /// Read the summary of the file at `repo_path`, a path in the
    /// memory repository.
    pub fn read(repo_path: &str, content: &str) -> Self {
        let heading = PageHeading::parse(repo_path, content);
        let subject = subject_scope(repo_path);
        // Only a private Subject Page has a Timeline from sources.
        let source_connection_id = if subject == Some(SubjectScope::Private) {
            page_sources(&SubjectPage::timeline_of(content))
                .into_iter()
                .find_map(|source| source.connection_id)
        } else {
            None
        };
        Self {
            brief: page_brief(repo_path, content),
            title: heading.title,
            kind: heading.kind,
            source_connection_id,
            links: page_links(content)
                .iter()
                .map(|target| link_repo_path(repo_path, target))
                .collect(),
        }
    }
}

/// What the selection of a Brief needs about one memory file, or
/// `None` when the file is never a candidate.
///
/// A Subject Page with a valid layout gives the words of its file name
/// and of its compiled truth. Every other memory file is a fact file:
/// it states one thing and has no section to parse, so its words come
/// from the layout-free path. The aliases of the file go into the
/// words of both paths, because an alias exists to be matched.
fn page_brief(repo_path: &str, content: &str) -> Option<PageBrief> {
    if is_memory_index(repo_path) {
        return None;
    }
    let parsed = SubjectPage::parse(content);
    let words = if parsed.layout_valid {
        brief_words(repo_path, &parsed.page)
    } else {
        fact_file_words(repo_path, content)
    };
    let mut words: Vec<_> = words.into_iter().collect();
    words.sort();
    Some(PageBrief { words })
}

/// The Subject Page of one conversation: the page an Agent
/// writes about the channel it works in. The path is a `subjects/`
/// path, so the page is a Brief candidate like every other Subject
/// Page and recall needs no rule of its own for it.
///
/// A Channel id is a ULID, so the file name holds no byte a path
/// refuses. The caller parses the result, and
/// [`crate::memory::ScopedPath`] refuses anything else.
pub fn conversation_page_path(channel_id: &str) -> String {
    format!("private/subjects/conversation/{channel_id}.md")
}

/// Standing work with no end date, the kind of a page that open work
/// starts. The Agent can say the work has an end, and a page it
/// named `Project` keeps that word.
const OPEN_WORK_KIND: &str = "Workstream";

/// The Kind column of a decision the conversation already made. The
/// claim rests on the Timeline entry of the session that settled it.
pub const DECISION_FACT_KIND: &str = "Decision";

/// True when a Continuation Record leaves open work: a next step or
/// an open question. A conversation that ended settled has
/// neither, and it writes no page.
pub fn has_open_work(state: &ContinuationState) -> bool {
    !state.next_steps.is_empty() || !state.open_questions.is_empty()
}

/// The Workstream page of the open work a Continuation Record holds
/// (ADR-0009). The record is working context of one conversation
/// and grants no authority. This step reads it and writes ordinary
/// memory: the goal and where it stands as the compiled truth, the
/// decisions as Facts, the open questions as their own section, and
/// one Timeline entry for this session of work.
///
/// `current` is the page the conversation already has, and the result
/// updates it, so a task worked on over a week is one page with a
/// Timeline and not seven pages. `reference` names the session; a
/// second call with the same reference replaces its entry.
///
/// Open work is a next step or an open question. A record with
/// neither is a conversation that ended settled, and it writes
/// nothing.
pub fn open_work_page(
    current: Option<&SubjectPage>,
    state: &ContinuationState,
    reference: &str,
    source_time: i64,
) -> Option<SubjectPage> {
    if !has_open_work(state) {
        return None;
    }
    let mut page = current.cloned().unwrap_or_default();
    if page.front_matter.title.is_none() {
        page.front_matter.title = state.active_goal.first().cloned();
    }
    if page.front_matter.kind.is_none() {
        page.front_matter.kind = Some(OPEN_WORK_KIND.to_string());
    }
    page.truth = open_work_truth(state, &page.truth);
    page.open_questions = state.open_questions.clone();
    for decision in &state.accepted_decisions {
        if page.facts.iter().any(|fact| &fact.claim == decision) {
            continue;
        }
        page.facts.push(Fact {
            claim: decision.clone(),
            kind: DECISION_FACT_KIND.to_string(),
            source_reference: reference.to_string(),
            status: FactStatus::Active,
        });
    }
    let words = session_words(state);
    match page
        .timeline
        .iter_mut()
        .find(|entry| entry.source_reference == reference)
    {
        Some(entry) => entry.words = words,
        None => {
            page.append(TimelineEntry {
                source_reference: reference.to_string(),
                source_time,
                words,
            });
        }
    }
    Some(page)
}

/// The compiled truth of a Workstream page: the goal, where the work
/// stands and the next step. The goal of a record with none is the
/// first paragraph of the truth the page already has, so a second
/// session keeps the goal and replaces the rest.
fn open_work_truth(state: &ContinuationState, current: &str) -> String {
    let goal = if state.active_goal.is_empty() {
        current.split("\n\n").next().unwrap_or_default().trim()
    } else {
        return joined_truth(&state.active_goal.join(" "), state);
    };
    joined_truth(goal, state)
}

fn joined_truth(goal: &str, state: &ContinuationState) -> String {
    let mut paragraphs = Vec::new();
    if !goal.trim().is_empty() {
        paragraphs.push(goal.trim().to_string());
    }
    if !state.completed_work.is_empty() {
        paragraphs.push(format!(
            "Where it stands: {}",
            state.completed_work.join(" ")
        ));
    }
    if !state.next_steps.is_empty() {
        paragraphs.push(format!("Next step: {}", state.next_steps.join(" ")));
    }
    paragraphs.join("\n\n")
}

/// What one session of work did, as the words of its Timeline entry:
/// the work it completed, the next step it leaves and the questions it
/// leaves open. The caller writes an entry only for a session with
/// open work, so these words are never empty.
fn session_words(state: &ContinuationState) -> String {
    let mut lines = state.completed_work.clone();
    lines.extend(
        state
            .next_steps
            .iter()
            .map(|step| format!("Next step: {step}")),
    );
    lines.extend(
        state
            .open_questions
            .iter()
            .map(|question| format!("Open question: {question}")),
    );
    lines.join("\n")
}

/// The software notes of an Agent, in its private memory. The file
/// name is the one `pagis-software` writes to.
const SOFTWARE_NOTE_NAME: &str = "software.md";

/// True when the file at `repo_path` is an index of a scope: the
/// `MEMORY.md` of a scope root, or the software notes of an Agent.
///
/// An index names the fact files of its scope and holds no fact of its
/// own. A run already reads both `MEMORY.md` indexes in full, so a
/// Brief that volunteered one would spend its budget twice on a table
/// of contents.
fn is_memory_index(repo_path: &str) -> bool {
    let rel = match repo_path.strip_prefix("shared/") {
        Some(rel) => rel,
        None => match repo_path
            .strip_prefix("agents/")
            .and_then(|rest| rest.split_once('/'))
        {
            Some((_, rel)) => rel,
            None => return false,
        },
    };
    rel == MEMORY_INDEX_FILE || rel == SOFTWARE_NOTE_NAME
}

/// The repository path that one link of the page at `repo_path` names.
/// A `private/` link resolves against the scope of the page that holds
/// it, so one agent never names the private memory of another.
///
/// A shared page has no private scope, so a `private/` link of one
/// stays as the page wrote it and names no file of the repository.
fn link_repo_path(repo_path: &str, target: &ScopedPath) -> String {
    match target.scope {
        MemoryScope::Shared => format!("shared/{}", target.rel),
        MemoryScope::Private => match private_root(repo_path) {
            Some(root) => format!("{root}{}", target.rel),
            None => target.display(),
        },
    }
}

/// The `agents/<agent id>/` root of a private repository path.
fn private_root(repo_path: &str) -> Option<String> {
    let agent_id = repo_path.strip_prefix("agents/")?.split_once('/')?.0;
    Some(format!("agents/{agent_id}/"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubjectScope {
    Shared,
    Private,
}

/// The scope of `shared/subjects/**.md` or
/// `agents/<agent id>/subjects/**.md`. Another path is not a Subject
/// Page.
fn subject_scope(repo_path: &str) -> Option<SubjectScope> {
    let (scope, rel) = match repo_path.strip_prefix("shared/") {
        Some(rel) => (SubjectScope::Shared, rel),
        None => (
            SubjectScope::Private,
            repo_path.strip_prefix("agents/")?.split_once('/')?.1,
        ),
    };
    (rel.starts_with("subjects/") && rel.ends_with(".md")).then_some(scope)
}

/// One source that wrote Timeline entries of a page, with how many.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSource {
    /// The connection the entries came through. `None` when the
    /// reference names no connection.
    pub connection_id: Option<String>,
    /// The resource of the reference (`gmail`, `calendar`, ...).
    pub resource: Option<String>,
    pub count: usize,
}

/// The sources of one page's Timeline, most entries first. A source
/// reference is `resource:connection_id:item:version`.
pub fn page_sources(timeline: &[TimelineEntry]) -> Vec<PageSource> {
    let mut counts: BTreeMap<(Option<String>, Option<String>), usize> = BTreeMap::new();
    for entry in timeline {
        let mut parts = entry.source_reference.splitn(4, ':');
        let resource = parts.next().map(str::to_string).filter(|s| !s.is_empty());
        let connection_id = parts.next().map(str::to_string).filter(|s| !s.is_empty());
        *counts.entry((connection_id, resource)).or_default() += 1;
    }
    let mut sources: Vec<PageSource> = counts
        .into_iter()
        .map(|((connection_id, resource), count)| PageSource {
            connection_id,
            resource,
            count,
        })
        .collect();
    sources.sort_by_key(|source| std::cmp::Reverse(source.count));
    sources
}

/// The name a person reads for one memory file, from its model-facing
/// path (`private/...`) and the content it holds, if any: the title its
/// front matter gives, the name of a memory index, "This conversation"
/// for the page of a conversation, whose file is named after a Channel
/// id, or else the file stem.
pub fn page_title(path: &str, content: Option<&str>) -> String {
    if let Some(title) = content.and_then(|content| FrontMatter::split(content).0.title) {
        return title;
    }
    match path.strip_suffix(MEMORY_INDEX_FILE) {
        Some("shared/") => return "Shared memory".to_string(),
        Some("private/") => return "Private memory".to_string(),
        _ => {}
    }
    if path
        .rsplit_once('/')
        .is_some_and(|(directory, _)| directory.ends_with("subjects/conversation"))
    {
        return "This conversation".to_string();
    }
    file_stem(path)
}

fn file_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".md").unwrap_or(name).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_title_is_the_front_matter_title_then_a_readable_name() {
        assert_eq!(
            page_title(
                "private/subjects/conversation/01M3BJ68SX8A040PQBAR51CCE9.md",
                Some("---\ntitle: Trip to Lisbon\nkind: Project\n---\n\nThe truth.\n"),
            ),
            "Trip to Lisbon"
        );
        assert_eq!(
            page_title(
                "private/subjects/conversation/01M3BJ68SX8A040PQBAR51CCE9.md",
                None
            ),
            "This conversation"
        );
        assert_eq!(page_title("shared/user.md", Some("Ada\n")), "user");
        assert_eq!(page_title("shared/MEMORY.md", None), "Shared memory");
        assert_eq!(page_title("private/MEMORY.md", None), "Private memory");
    }

    #[test]
    fn front_matter_gives_title_and_kind() {
        let heading = PageHeading::parse(
            "subjects/gmail/x.md",
            "---\ntitle: Priya Sharma\nkind: Person\n---\n\nPriya leads the pilot.\n",
        );
        assert_eq!(
            heading,
            PageHeading {
                title: "Priya Sharma".into(),
                kind: Some("Person".into()),
            }
        );
    }

    #[test]
    fn a_block_with_a_title_alone_has_no_kind() {
        let heading = PageHeading::parse("x.md", "---\ntitle: Northwind renewal\n---\n\nbody\n");
        assert_eq!(heading.title, "Northwind renewal");
        assert_eq!(heading.kind, None);
    }

    #[test]
    fn a_heading_line_is_not_a_title() {
        let heading = PageHeading::parse("subjects/gmail/19cfcbf4.md", "# Mail: School updates\n");
        assert_eq!(heading.title, "19cfcbf4");
        assert_eq!(heading.kind, None);
    }

    #[test]
    fn a_page_without_front_matter_is_named_after_its_file() {
        let heading = PageHeading::parse("subjects/gmail/19cfcbf4.md", "no front matter here\n");
        assert_eq!(heading.title, "19cfcbf4");
        assert_eq!(heading.kind, None);
    }

    #[test]
    fn sources_count_entries_per_connection() {
        let entry = |reference: &str| TimelineEntry {
            source_reference: reference.into(),
            source_time: 0,
            words: String::new(),
        };
        let sources = page_sources(&[
            entry("gmail:con_1:m1:1"),
            entry("gmail:con_1:m2:1"),
            entry("calendar:con_2:e1:1"),
        ]);
        assert_eq!(
            sources,
            vec![
                PageSource {
                    connection_id: Some("con_1".into()),
                    resource: Some("gmail".into()),
                    count: 2,
                },
                PageSource {
                    connection_id: Some("con_2".into()),
                    resource: Some("calendar".into()),
                    count: 1,
                },
            ]
        );
    }

    #[test]
    fn a_summary_names_the_main_source_of_a_private_subject_page_only() {
        let entry = |reference: &str| TimelineEntry {
            source_reference: reference.into(),
            source_time: 0,
            words: "A message.".into(),
        };
        let page = crate::subject_page::SubjectPage {
            truth: "Priya leads the pilot.".into(),
            timeline: vec![entry("gmail:conn-1:m1:1"), entry("gmail:conn-1:m2:1")],
            ..Default::default()
        }
        .render();

        let subject = PageSummary::read("agents/ag1/subjects/gmail/priya.md", &page);
        assert_eq!(subject.title, "priya");
        assert_eq!(subject.source_connection_id.as_deref(), Some("conn-1"));

        assert_eq!(
            subject.brief,
            Some(PageBrief {
                words: ["leads", "pilot", "priya", "the"]
                    .map(String::from)
                    .to_vec(),
            }),
            "a page with a valid layout has its entity words"
        );

        let shared = PageSummary::read("shared/subjects/gmail/priya.md", &page);
        assert_eq!(shared.source_connection_id, None);
        assert!(
            shared.brief.is_some(),
            "a shared Subject Page is in a Brief"
        );
        let craft = PageSummary::read(
            "agents/ag1/craft.md",
            "---\ntitle: File a claim\nkind: Procedure\n---\n\nHow to file a claim.\n",
        );
        assert_eq!(craft.title, "File a claim");
        assert_eq!(craft.kind.as_deref(), Some("Procedure"));
        assert_eq!(craft.source_connection_id, None);
    }

    /// A fact file has no Subject Page layout, so its words come from
    /// the layout-free path: the file stem, the title, the aliases and
    /// the first paragraph.
    #[test]
    fn a_fact_file_is_a_brief_candidate_through_its_words() {
        let summary = PageSummary::read(
            "shared/couch.md",
            "---\ntitle: Sofa\naliases: [[couch]] [[davenport]]\nkind: Concept\n---\n\n\
             The owner bought it in Lisbon.\n\nIt needs a new cover.\n",
        );

        assert_eq!(
            summary.brief,
            Some(PageBrief {
                words: [
                    "bought",
                    "couch",
                    "davenport",
                    "lisbon",
                    "owner",
                    "sofa",
                    "the"
                ]
                .map(String::from)
                .to_vec(),
            })
        );
    }

    /// A Subject Page with no valid layout states one thing and holds
    /// no section, so the fact file path reads it.
    #[test]
    fn a_page_with_no_layout_is_read_as_a_fact_file() {
        let summary = PageSummary::read(
            "agents/ag1/subjects/gmail/broken.md",
            "The tile order arrived.\n\nThe grout is LATER_PARAGRAPH.\n",
        );

        let words = summary.brief.expect("a fact file is a candidate").words;
        assert!(words.contains(&"tile".to_string()), "{words:?}");
        assert!(!words.contains(&"later_paragraph".to_string()), "{words:?}");
    }

    /// An index names the fact files of its scope and holds no fact of
    /// its own, so a Brief never volunteers one.
    #[test]
    fn a_memory_index_is_never_a_brief_candidate() {
        for path in [
            "shared/MEMORY.md",
            "agents/ag1/MEMORY.md",
            "agents/ag1/software.md",
        ] {
            let summary = PageSummary::read(path, "- [Sofa](couch.md) — bought in Lisbon\n");
            assert_eq!(summary.brief, None, "{path} is an index");
        }
        let page = PageSummary::read("shared/subjects/MEMORY.md", "A page, not an index.\n");
        assert!(
            page.brief.is_some(),
            "the rule names the file at a scope root"
        );
    }

    /// The aliases of a Subject Page are entity words of it, so a turn
    /// that uses one of them volunteers the page.
    #[test]
    fn the_aliases_of_a_subject_page_are_entity_words() {
        let page = crate::subject_page::SubjectPage {
            front_matter: FrontMatter {
                title: Some("Priya Sharma".into()),
                aliases: vec!["pri".into(), "priya@northwind.example".into()],
                ..FrontMatter::default()
            },
            truth: "Priya leads the pilot.".into(),
            ..Default::default()
        }
        .render();

        let words = PageSummary::read("agents/ag1/subjects/gmail/19cfcbf4.md", &page)
            .brief
            .expect("a Subject Page is a candidate")
            .words;

        assert!(words.contains(&"pri".to_string()), "{words:?}");
        assert!(words.contains(&"northwind".to_string()), "{words:?}");
    }

    /// A kind outside the vocabulary is kept, not refused (ADR-0007).
    /// The Page Index reports the word the page carries.
    #[test]
    fn the_page_index_keeps_a_kind_outside_the_vocabulary() {
        let summary = PageSummary::read(
            "agents/ag1/subjects/gmail/19cfcbf4.md",
            "---\ntitle: File a claim\nkind: Procedure\n---\n\nHow to file a claim.\n",
        );

        assert!(!crate::subject_page::is_page_kind("Procedure"));
        assert_eq!(summary.kind.as_deref(), Some("Procedure"));
    }

    /// The block names the page. Its words are not words of the
    /// subject, so the Brief does not select on them.
    #[test]
    fn a_summary_reads_the_front_matter_and_keeps_its_words_out_of_the_brief() {
        let page = crate::subject_page::SubjectPage {
            front_matter: FrontMatter {
                title: Some("Priya Sharma".into()),
                kind: Some("Person".into()),
                aliases: Vec::new(),
                links: Vec::new(),
            },
            truth: "Priya leads the pilot.".into(),
            ..Default::default()
        }
        .render();

        let summary = PageSummary::read("agents/ag1/subjects/gmail/19cfcbf4.md", &page);

        assert_eq!(summary.title, "Priya Sharma");
        assert_eq!(summary.kind.as_deref(), Some("Person"));
        assert_eq!(
            summary.brief,
            Some(PageBrief {
                words: ["19cfcbf4", "leads", "pilot", "priya", "the"]
                    .map(String::from)
                    .to_vec(),
            })
        );
    }

    /// A link resolves against the scope of the page that holds it,
    /// so the index holds repository paths on both ends.
    #[test]
    fn a_summary_resolves_each_link_to_a_repository_path() {
        let content = "---\ntitle: Priya Sharma\nkind: Person\n\
                       links: [[shared/user.md]]\n---\n\n\
                       She leads [[private/subjects/gmail/19ce.md]].\n";

        let private = PageSummary::read("agents/ag1/subjects/gmail/priya.md", content);
        assert_eq!(
            private.links,
            vec![
                "shared/user.md".to_string(),
                "agents/ag1/subjects/gmail/19ce.md".to_string(),
            ]
        );

        let shared = PageSummary::read("shared/subjects/gmail/priya.md", content);
        assert_eq!(
            shared.links,
            vec![
                "shared/user.md".to_string(),
                "private/subjects/gmail/19ce.md".to_string(),
            ],
            "a shared page has no private scope, so the link names no file"
        );
    }

    /// A target that is not a scope-relative path is prose.
    #[test]
    fn a_summary_keeps_no_link_that_is_not_a_path() {
        let summary = PageSummary::read(
            "agents/ag1/subjects/gmail/priya.md",
            "She works with [[Priya Sharma]] on [[../escape.md]].\n",
        );

        assert!(summary.links.is_empty(), "{:?}", summary.links);
    }

    /// The page of a conversation stands under `subjects/`, so the
    /// rules of a Subject Page hold for it and recall reaches it with
    /// no change of its own.
    #[test]
    fn a_conversation_page_is_a_brief_candidate_like_any_subject_page() {
        let path = conversation_page_path("01J8ZC");
        assert_eq!(path, "private/subjects/conversation/01J8ZC.md");

        let page = crate::subject_page::SubjectPage {
            front_matter: FrontMatter {
                title: Some("Tile order".into()),
                kind: Some("Project".into()),
                ..FrontMatter::default()
            },
            truth: "The owner waits for the grout colour.".into(),
            ..Default::default()
        }
        .render();
        let repo_path = format!("agents/ag1/{}", path.strip_prefix("private/").unwrap());

        let summary = PageSummary::read(&repo_path, &page);

        assert_eq!(summary.kind.as_deref(), Some("Project"));
        assert_eq!(
            summary.source_connection_id, None,
            "a page written from a conversation names no connection"
        );
        let words = summary.brief.expect("a Subject Page is a candidate").words;
        assert!(words.contains(&"grout".to_string()), "{words:?}");
    }

    /// A conversation Timeline entry names no connection, so the Page
    /// Index reports the page as one a conversation wrote.
    #[test]
    fn a_conversation_timeline_entry_has_no_connection() {
        let sources = page_sources(&[TimelineEntry {
            source_reference: "conversation::01J8RUN:1".into(),
            source_time: 0,
            words: "The owner chose the grey grout.".into(),
        }]);

        assert_eq!(
            sources,
            vec![PageSource {
                connection_id: None,
                resource: Some("conversation".into()),
                count: 1,
            }]
        );
    }

    fn record(next_steps: &[&str], open_questions: &[&str]) -> ContinuationState {
        ContinuationState {
            active_goal: vec!["Retile the bathroom.".into()],
            accepted_decisions: vec!["Use the grey tiles.".into()],
            completed_work: vec!["Measured the wall.".into()],
            next_steps: next_steps.iter().map(|step| (*step).to_string()).collect(),
            open_questions: open_questions
                .iter()
                .map(|question| (*question).to_string())
                .collect(),
            ..ContinuationState::default()
        }
    }

    /// Open work becomes a durable Workstream page: the goal and where
    /// it stands as the truth, the decisions as Facts, the open
    /// questions as their own section, and one Timeline entry for the
    /// session.
    #[test]
    fn open_work_becomes_a_workstream_page() {
        let state = record(&["Order the grout."], &["Which grout colour?"]);

        let page = open_work_page(None, &state, "conversation::run-1:1", 7)
            .expect("open work writes a page");

        assert_eq!(page.front_matter.kind.as_deref(), Some("Workstream"));
        assert_eq!(
            page.front_matter.title.as_deref(),
            Some("Retile the bathroom.")
        );
        assert_eq!(
            page.truth,
            "Retile the bathroom.\n\nWhere it stands: Measured the wall.\n\nNext step: Order the grout."
        );
        assert_eq!(page.open_questions, ["Which grout colour?"]);
        assert_eq!(
            page.facts,
            vec![Fact {
                claim: "Use the grey tiles.".into(),
                kind: DECISION_FACT_KIND.into(),
                source_reference: "conversation::run-1:1".into(),
                status: FactStatus::Active,
            }],
            "a decision already made is a fact that rests on this session"
        );
        assert_eq!(page.timeline.len(), 1);
        assert_eq!(
            page.timeline[0].words,
            "Measured the wall.\nNext step: Order the grout.\nOpen question: Which grout colour?"
        );
    }

    /// A conversation that ended settled leaves no next step and no
    /// open question, so it writes nothing.
    #[test]
    fn a_settled_record_writes_no_page() {
        assert!(open_work_page(None, &record(&[], &[]), "conversation::run-1:1", 7).is_none());
    }

    /// A second session on the same work updates the one page: it adds
    /// a Timeline entry, replaces the open questions and keeps the
    /// decision it already holds.
    #[test]
    fn a_second_session_adds_a_timeline_entry_to_the_same_page() {
        let first = open_work_page(
            None,
            &record(&["Order the grout."], &["Which grout colour?"]),
            "conversation::run-1:1",
            7,
        )
        .expect("the first session writes a page");
        let mut later = record(&["Book the tiler."], &["Which Saturday?"]);
        later.completed_work = vec!["Ordered the grey grout.".into()];
        later.accepted_decisions = vec!["Use the grey tiles.".into(), "Grout in slate.".into()];

        let second = open_work_page(Some(&first), &later, "conversation::run-2:1", 9)
            .expect("the second session writes a page");

        assert_eq!(second.timeline.len(), 2, "one entry for each session");
        assert_eq!(second.timeline[0], first.timeline[0]);
        assert_eq!(
            second.open_questions,
            ["Which Saturday?"],
            "the section holds the questions that are still open"
        );
        assert_eq!(
            second.facts.len(),
            2,
            "a decision the page already holds is not added again: {:?}",
            second.facts
        );
        assert_eq!(
            second.truth,
            "Retile the bathroom.\n\nWhere it stands: Ordered the grey grout.\n\nNext step: Book the tiler."
        );
    }

    /// A record with no goal keeps the goal the page already states,
    /// and it replaces nothing else of the truth.
    #[test]
    fn a_record_without_a_goal_keeps_the_goal_of_the_page() {
        let first = open_work_page(
            None,
            &record(&["Order the grout."], &[]),
            "conversation::run-1:1",
            7,
        )
        .expect("the first session writes a page");
        let mut later = record(&["Book the tiler."], &[]);
        later.active_goal = Vec::new();
        later.completed_work = Vec::new();

        let second = open_work_page(Some(&first), &later, "conversation::run-2:1", 9)
            .expect("the second session writes a page");

        assert_eq!(
            second.truth,
            "Retile the bathroom.\n\nNext step: Book the tiler."
        );
    }

    /// The page keeps the kind the Agent chose. Work with an end is a
    /// Project, and a promotion does not make it standing work.
    #[test]
    fn a_promotion_keeps_the_page_kind_the_agent_chose() {
        let current = SubjectPage {
            front_matter: FrontMatter {
                title: Some("Bathroom tiles".into()),
                kind: Some("Project".into()),
                ..FrontMatter::default()
            },
            ..Default::default()
        };

        let page = open_work_page(
            Some(&current),
            &record(&["Order the grout."], &[]),
            "conversation::run-1:1",
            7,
        )
        .expect("open work writes a page");

        assert_eq!(page.front_matter.kind.as_deref(), Some("Project"));
        assert_eq!(page.front_matter.title.as_deref(), Some("Bathroom tiles"));
    }

    /// Two promotions of one session write one entry, so a repeated
    /// compaction in the same Run does not grow the Timeline.
    #[test]
    fn a_repeated_session_replaces_its_own_timeline_entry() {
        let first = open_work_page(
            None,
            &record(&["Order the grout."], &[]),
            "conversation::run-1:1",
            7,
        )
        .expect("the first promotion writes a page");

        let again = open_work_page(
            Some(&first),
            &record(&["Order the grout and the trim."], &[]),
            "conversation::run-1:1",
            9,
        )
        .expect("the second promotion writes a page");

        assert_eq!(again.timeline.len(), 1);
        assert_eq!(
            again.timeline[0].words,
            "Measured the wall.\nNext step: Order the grout and the trim."
        );
    }

    #[test]
    fn an_empty_timeline_has_no_sources() {
        assert!(page_sources(&[]).is_empty());
    }
}
