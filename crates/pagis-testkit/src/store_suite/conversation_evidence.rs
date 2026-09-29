//! The Conversation Evidence trait tests of the suite.
//!
//! Each body is one test. It takes a [`Backend`], reads the store set
//! from it, and never names a pool type, so it runs on both backends
//! from one text. Name every new body in
//! `store_suite_conversation_evidence!` below: the guard test of the
//! parent module fails while one is missing.
//!
//! `search` runs a full-text index, and the two backends build it with
//! different engines: SQLite has an FTS5 virtual table, Postgres has a
//! `tsvector` column. The trait answers the hits of one conversation in
//! message-id order on both (ADR-0008: relevance is a property, and no
//! test asserts the order of a ranking function), so a body holds the
//! order of the ids and the content of a snippet, never a rank.

use pagis_core::{
    Block, ConversationScope, Grant, GrantId, MemoryExposure, MessageId, RetainedToolEvidence,
    Workspace, WorkspaceId,
};

use crate::fixture::{
    agent, agent_participant, channel, queued_run, user_message, user_participant,
};

use super::{Backend, Bind};

pub async fn search_and_range_read_use_message_id_order_inside_one_conversation(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let agent = agent(&workspace.id);
    let channel = channel(&workspace.id);
    backend.stores().agents.create(&agent).await.unwrap();
    backend.stores().channels.create(&channel).await.unwrap();
    let participants = &backend.stores().participants;
    participants
        .create(&user_participant(&workspace.id, &channel.id))
        .await
        .unwrap();
    participants
        .create(&agent_participant(&workspace.id, &channel.id, &agent.id))
        .await
        .unwrap();

    let messages = &backend.stores().messages;
    let mut later_inserted = user_message(&workspace.id, &channel.id, "ultraviolet code is 8241");
    later_inserted.id = MessageId::from("01J00000000000000000000002".to_string());
    later_inserted.created_at = 1_000;
    let mut earlier_id = user_message(
        &workspace.id,
        &channel.id,
        "ultraviolet project starts Friday",
    );
    earlier_id.id = MessageId::from("01J00000000000000000000001".to_string());
    earlier_id.created_at = 1_000;
    messages.insert(&later_inserted).await.unwrap();
    messages.insert(&earlier_id).await.unwrap();

    let scope = ConversationScope {
        workspace_id: workspace.id,
        agent_id: agent.id,
        channel_id: channel.id,
        root_message_id: None,
    };
    let evidence = &backend.stores().conversation_evidence;
    let found = evidence
        .search(&scope, "ultraviolet", None, 10)
        .await
        .unwrap();
    assert_eq!(
        found
            .iter()
            .map(|hit| hit.message_id.as_str())
            .collect::<Vec<_>>(),
        ["01J00000000000000000000001", "01J00000000000000000000002"]
    );
    assert_eq!(found[1].reference, "message:01J00000000000000000000002");
    assert!(found[1].snippet.contains("8241"), "{}", found[1].snippet);

    let page = evidence
        .read_range(
            &scope,
            Some(&MessageId::from("01J00000000000000000000001".to_string())),
            &MessageId::from("01J00000000000000000000002".to_string()),
            10,
        )
        .await
        .unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].text, "ultraviolet code is 8241");
    assert_eq!(page[0].speaker, "user");
    assert_eq!(page[0].created_at, 1_000);
}

pub async fn invalidated_hits_do_not_starve_pages_and_stay_absent_after_restart(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let agent = agent(&workspace.id);
    let channel = channel(&workspace.id);
    backend.stores().agents.create(&agent).await.unwrap();
    backend.stores().channels.create(&channel).await.unwrap();
    let messages = &backend.stores().messages;
    let mut first = user_message(&workspace.id, &channel.id, "marigold first");
    first.id = MessageId::from("01J00000000000000000000011".to_string());
    messages.insert(&first).await.unwrap();

    let grant = Grant {
        id: GrantId::from("grant-evidence".to_string()),
        workspace_id: workspace.id.clone(),
        agent_id: agent.id.clone(),
        resource_kind: Grant::CONNECTION_KIND.to_string(),
        resource_id: None,
        scope: serde_json::json!({}),
        revision: 1,
        created_at: 1,
        revoked_at: None,
    };
    let grants = &backend.stores().grants;
    grants.create(&grant).await.unwrap();
    let mut revoked =
        crate::fixture::agent_message(&workspace.id, &channel.id, &agent.id, "marigold revoked");
    revoked.id = MessageId::from("01J00000000000000000000012".to_string());
    messages
        .insert_stamped(
            &revoked,
            &[MemoryExposure {
                grant_id: grant.id.clone(),
                revision: 1,
            }],
        )
        .await
        .unwrap();
    let mut forgotten = user_message(&workspace.id, &channel.id, "marigold forgotten");
    forgotten.id = MessageId::from("01J00000000000000000000013".to_string());
    messages.insert(&forgotten).await.unwrap();
    let mut last = user_message(&workspace.id, &channel.id, "marigold last");
    last.id = MessageId::from("01J00000000000000000000014".to_string());
    messages.insert(&last).await.unwrap();

    let scope = ConversationScope {
        workspace_id: workspace.id.clone(),
        agent_id: agent.id,
        channel_id: channel.id,
        root_message_id: None,
    };
    let evidence = &backend.stores().conversation_evidence;
    let first_page = evidence.search(&scope, "marigold", None, 1).await.unwrap();
    assert_eq!(first_page[0].message_id, first.id);

    grants
        .revoke(&grant.workspace_id, &grant.id, 2)
        .await
        .unwrap();
    backend
        .execute(
            "INSERT INTO forgotten_messages(message_id) VALUES (?)",
            &[Bind::from(forgotten.id.as_str())],
        )
        .await
        .expect("mark the message forgotten");

    // The store holds a pool and no state of its own, so a read after
    // the revoke and the forget is the read a restarted daemon makes.
    // Invalid rows are filtered before LIMIT, so they cannot consume the
    // next page.
    let next = evidence
        .search(
            &scope,
            "marigold",
            Some(&format!(
                "{}|message:{}",
                first.id.as_str(),
                first.id.as_str()
            )),
            1,
        )
        .await
        .unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].message_id, last.id);
    let direct = evidence
        .read_range(&scope, Some(&first.id), &last.id, 10)
        .await
        .unwrap();
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].message_id, last.id);
    // The row is gone from the index, not merely filtered at the read.
    // `message_id` is a column of both indexes, so the count is the one
    // portable question to ask of them.
    for gone in [&revoked.id, &forgotten.id] {
        assert_eq!(
            backend
                .count(
                    "SELECT count(*) FROM conversation_message_search WHERE message_id = ?",
                    &[Bind::from(gone.as_str())],
                )
                .await
                .expect("count the rows of the message index"),
            0,
            "{gone} still holds a row in the message index"
        );
    }
}

pub async fn range_read_keeps_conversation_scope_and_marks_a_missing_artifact(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let agent = agent(&workspace.id);
    let channel = channel(&workspace.id);
    let other = crate::fixture::channel(&workspace.id);
    backend.stores().agents.create(&agent).await.unwrap();
    let channels = &backend.stores().channels;
    channels.create(&channel).await.unwrap();
    channels.create(&other).await.unwrap();
    let messages = &backend.stores().messages;
    let mut kept = user_message(&workspace.id, &channel.id, "onyx scoped");
    kept.id = MessageId::from("01J00000000000000000000021".to_string());
    kept.blocks = vec![Block::file("missing-artifact", "proof.txt", None, None)];
    kept.text_content = "onyx scoped".to_string();
    messages.insert(&kept).await.unwrap();
    let mut foreign = user_message(&workspace.id, &other.id, "onyx other conversation");
    foreign.id = MessageId::from("01J00000000000000000000022".to_string());
    messages.insert(&foreign).await.unwrap();

    let scope = ConversationScope {
        workspace_id: workspace.id,
        agent_id: agent.id,
        channel_id: channel.id,
        root_message_id: None,
    };
    let evidence = &backend.stores().conversation_evidence;
    let hits = evidence.search(&scope, "onyx", None, 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message_id, kept.id);
    let read = evidence
        .read_range(&scope, None, &foreign.id, 10)
        .await
        .unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].artifacts.len(), 1);
    assert_eq!(read[0].artifacts[0].reference, "artifact:missing-artifact");
    assert!(!read[0].artifacts[0].available);
}

pub async fn thread_scope_returns_its_root_and_replies_only(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let agent = agent(&workspace.id);
    let channel = channel(&workspace.id);
    backend.stores().agents.create(&agent).await.unwrap();
    backend.stores().channels.create(&channel).await.unwrap();
    let messages = &backend.stores().messages;
    let mut root = user_message(&workspace.id, &channel.id, "saffron root");
    root.id = MessageId::from("01J00000000000000000000031".to_string());
    messages.insert(&root).await.unwrap();
    let mut reply = user_message(&workspace.id, &channel.id, "saffron reply");
    reply.id = MessageId::from("01J00000000000000000000032".to_string());
    reply.parent_message_id = Some(root.id.clone());
    messages.insert(&reply).await.unwrap();
    let mut unrelated = user_message(&workspace.id, &channel.id, "saffron unrelated");
    unrelated.id = MessageId::from("01J00000000000000000000033".to_string());
    messages.insert(&unrelated).await.unwrap();

    let scope = ConversationScope {
        workspace_id: workspace.id,
        agent_id: agent.id,
        channel_id: channel.id,
        root_message_id: Some(root.id.clone()),
    };
    let evidence = &backend.stores().conversation_evidence;
    let hits = evidence.search(&scope, "saffron", None, 10).await.unwrap();
    assert_eq!(
        hits.iter().map(|hit| &hit.message_id).collect::<Vec<_>>(),
        [&root.id, &reply.id]
    );
}

/// Both full-text indexes hold the tenant in their own rows, so
/// a search of one workspace never reads the rows of another workspace.
pub async fn message_search_of_one_workspace_does_not_reach_another_workspace(backend: &Backend) {
    let (first, second) = tenant_pair(backend).await;
    let messages = &backend.stores().messages;
    let mut here = user_message(&first.workspace_id, &first.channel_id, "cobalt here");
    here.id = MessageId::from("01J00000000000000000000041".to_string());
    messages.insert(&here).await.unwrap();
    let mut there = user_message(&second.workspace_id, &second.channel_id, "cobalt there");
    there.id = MessageId::from("01J00000000000000000000042".to_string());
    messages.insert(&there).await.unwrap();

    let evidence = &backend.stores().conversation_evidence;
    let found = evidence.search(&first, "cobalt", None, 10).await.unwrap();
    assert_eq!(
        found
            .iter()
            .map(|hit| hit.message_id.as_str())
            .collect::<Vec<_>>(),
        [here.id.as_str()]
    );
    let other = evidence.search(&second, "cobalt", None, 10).await.unwrap();
    assert_eq!(
        other
            .iter()
            .map(|hit| hit.message_id.as_str())
            .collect::<Vec<_>>(),
        [there.id.as_str()]
    );
}

pub async fn tool_search_of_one_workspace_does_not_reach_another_workspace(backend: &Backend) {
    let (first, second) = tenant_pair(backend).await;
    let here = retain_tool(
        backend,
        &first,
        "01J00000000000000000000051",
        "grant-here",
        "the amber report of the first workspace",
    )
    .await;
    let there = retain_tool(
        backend,
        &second,
        "01J00000000000000000000052",
        "grant-there",
        "the amber report of the second workspace",
    )
    .await;

    let evidence = &backend.stores().conversation_evidence;
    let found = evidence.search(&first, "amber", None, 10).await.unwrap();
    assert_eq!(
        found.iter().map(|hit| &hit.reference).collect::<Vec<_>>(),
        [&here]
    );
    let other = evidence.search(&second, "amber", None, 10).await.unwrap();
    assert_eq!(
        other.iter().map(|hit| &hit.reference).collect::<Vec<_>>(),
        [&there]
    );
}

/// One search reads one tenant in both indexes at once. The two
/// Workspaces hold the same word in a message and in a tool result, so a
/// leak in either index shows as a hit that belongs to the other
/// Workspace.
pub async fn search_of_one_workspace_reads_neither_index_of_another_workspace(backend: &Backend) {
    let (first, second) = tenant_pair(backend).await;
    let messages = &backend.stores().messages;
    let mut here = user_message(&first.workspace_id, &first.channel_id, "zinc in a message");
    here.id = MessageId::from("01J00000000000000000000061".to_string());
    messages.insert(&here).await.unwrap();
    let mut there = user_message(
        &second.workspace_id,
        &second.channel_id,
        "zinc in a message",
    );
    there.id = MessageId::from("01J00000000000000000000062".to_string());
    messages.insert(&there).await.unwrap();
    let here_tool = retain_tool(
        backend,
        &first,
        "01J00000000000000000000063",
        "grant-zinc-here",
        "zinc in a tool result",
    )
    .await;
    let there_tool = retain_tool(
        backend,
        &second,
        "01J00000000000000000000064",
        "grant-zinc-there",
        "zinc in a tool result",
    )
    .await;

    let evidence = &backend.stores().conversation_evidence;
    let mine: Vec<String> = evidence
        .search(&first, "zinc", None, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|hit| hit.reference)
        .collect();
    assert_eq!(
        mine,
        vec![format!("message:{}", here.id.as_str()), here_tool]
    );
    let theirs: Vec<String> = evidence
        .search(&second, "zinc", None, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|hit| hit.reference)
        .collect();
    assert_eq!(
        theirs,
        vec![format!("message:{}", there.id.as_str()), there_tool]
    );
}

/// Two workspaces of one person, each with an agent and a channel.
async fn tenant_pair(backend: &Backend) -> (ConversationScope, ConversationScope) {
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
    (tenant(backend, first).await, tenant(backend, second).await)
}

async fn tenant(backend: &Backend, workspace: Workspace) -> ConversationScope {
    let agent = agent(&workspace.id);
    let channel = channel(&workspace.id);
    backend.stores().agents.create(&agent).await.unwrap();
    backend.stores().channels.create(&channel).await.unwrap();
    ConversationScope {
        workspace_id: workspace.id,
        agent_id: agent.id,
        channel_id: channel.id,
        root_message_id: None,
    }
}

/// One retained tool result of the scope, with the source message, the
/// run and the Grant it needs. It answers the reference of the row.
async fn retain_tool(
    backend: &Backend,
    scope: &ConversationScope,
    message_id: &str,
    grant_id: &str,
    content: &str,
) -> String {
    let mut source = user_message(&scope.workspace_id, &scope.channel_id, "source");
    source.id = MessageId::from(message_id.to_string());
    backend.stores().messages.insert(&source).await.unwrap();
    let run = queued_run(&scope.workspace_id, &scope.agent_id, &scope.channel_id);
    backend.stores().runs.create(&run).await.unwrap();
    let grant = Grant {
        id: GrantId::from(grant_id.to_string()),
        workspace_id: scope.workspace_id.clone(),
        agent_id: scope.agent_id.clone(),
        resource_kind: Grant::CONNECTION_KIND.to_string(),
        resource_id: None,
        scope: serde_json::json!({}),
        revision: 1,
        created_at: 1,
        revoked_at: None,
    };
    backend.stores().grants.create(&grant).await.unwrap();
    backend
        .stores()
        .conversation_evidence
        .retain_tool(&RetainedToolEvidence {
            scope: scope.clone(),
            source_message_id: source.id,
            run_id: run.id,
            tool_call_id: "call-1".to_string(),
            tool_name: "search".to_string(),
            content: content.to_string(),
            complete: true,
            exposure: MemoryExposure {
                grant_id: grant.id,
                revision: 1,
            },
            created_at: 1,
        })
        .await
        .unwrap()
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_conversation_evidence {
    ($emit:path) => {
        $emit!(
            conversation_evidence,
            search_and_range_read_use_message_id_order_inside_one_conversation,
            invalidated_hits_do_not_starve_pages_and_stay_absent_after_restart,
            range_read_keeps_conversation_scope_and_marks_a_missing_artifact,
            thread_scope_returns_its_root_and_replies_only,
            message_search_of_one_workspace_does_not_reach_another_workspace,
            tool_search_of_one_workspace_does_not_reach_another_workspace,
            search_of_one_workspace_reads_neither_index_of_another_workspace,
        );
    };
}
