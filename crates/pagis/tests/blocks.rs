//! The block union end to end (ADR-0004): every one of the nine
//! variants survives the store and the REST timeline unchanged, a
//! block type from a newer daemon survives beside them instead of
//! failing the message, and `text_content` is the block projection.

use pagis_core::{
    AgentId, AgentStore, AuthorKind, Block, ChannelId, ChoiceOption, FormField, FormFieldKind,
    KnownBlock, Message, MessageId, MessageStatus, MessageStore, TableAlign, TableCell,
    TableColumn, WorkspaceId, blocks_text, now_ms,
};
use pagis_storage_sqlite::SqliteMessageStore;
use pagis_testkit::TestDaemon;

/// One block of every type in the vocabulary, plus a block only a
/// newer daemon knows.
fn every_block() -> Vec<Block> {
    vec![
        Block::markdown("Here is the week."),
        KnownBlock::Table {
            columns: vec![
                TableColumn {
                    key: "day".to_string(),
                    label: "Day".to_string(),
                    align: None,
                },
                TableColumn {
                    key: "hours".to_string(),
                    label: "Hours".to_string(),
                    align: Some(TableAlign::Right),
                },
            ],
            rows: vec![vec![
                TableCell::Text {
                    text: "Monday".to_string(),
                },
                TableCell::Number {
                    number: serde_json::Number::from(6),
                },
            ]],
        }
        .into(),
        KnownBlock::Form {
            request_id: "req_form".to_string(),
            title: "Book the room".to_string(),
            fields: vec![FormField {
                key: "when".to_string(),
                label: "When".to_string(),
                kind: FormFieldKind::Date,
                required: true,
                options: Vec::new(),
                placeholder: None,
            }],
            submit_label: Some("Book".to_string()),
        }
        .into(),
        KnownBlock::ChoiceCard {
            request_id: "req_choice".to_string(),
            title: "Which date?".to_string(),
            body: None,
            options: vec![ChoiceOption {
                value: "tue".to_string(),
                label: "Tuesday".to_string(),
                description: None,
            }],
        }
        .into(),
        Block::approval_card("apr_1", "Run a command", "echo hi"),
        KnownBlock::Progress {
            run_id: "run_1".to_string(),
            text: "Reading the calendar".to_string(),
        }
        .into(),
        Block::image("art_image", Some("a red square".to_string())),
        Block::file(
            "art_file",
            "notes.txt",
            Some("text/plain".to_string()),
            Some(12),
        ),
        KnownBlock::Screen {
            agent_id: "agt_1".to_string(),
        }
        .into(),
        // A type this build does not know: it is stored and served
        // unchanged, and the UI renders it with the fallback.
        Block::Unknown(serde_json::json!({"type": "hologram", "text": "Beam it up"})),
    ]
}

async fn workspace_id(daemon: &TestDaemon) -> WorkspaceId {
    pagis_storage_sqlite::SqliteAgentStore::new(daemon.pool().clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap()
        .workspace_id
}

#[tokio::test]
async fn every_block_variant_survives_the_store_and_the_timeline() {
    let daemon = TestDaemon::start().await;
    let blocks = every_block();
    let message = Message {
        id: MessageId::generate(),
        workspace_id: workspace_id(&daemon).await,
        channel_id: ChannelId::from(daemon.dm_channel_id.clone()),
        parent_message_id: None,
        author_kind: AuthorKind::System,
        author_agent_id: None,
        run_id: None,
        status: MessageStatus::Complete,
        blocks: blocks.clone(),
        text_content: blocks_text(&blocks),
        pending_id: None,
        created_at: now_ms(),
        completed_at: Some(now_ms()),
    };
    let messages = SqliteMessageStore::new(daemon.pool().clone());
    messages.insert(&message).await.unwrap();
    messages
        .set_exposures(&message.workspace_id, &message.id, &[])
        .await
        .unwrap();

    // The store returns what it was given, unknown block included.
    let stored = messages
        .get(&message.workspace_id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.blocks, blocks);

    // So does the API, byte for byte against the serialized union.
    let page: serde_json::Value = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let served = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == message.id.as_str())
        .expect("the seeded message is in the timeline");
    assert_eq!(served["blocks"], serde_json::to_value(&blocks).unwrap());
    assert_eq!(
        served["blocks"][9]["type"], "hologram",
        "an unknown type reaches the UI intact"
    );

    // The projection is what search and the model read.
    assert_eq!(
        served["text_content"].as_str().unwrap(),
        "Here is the week.\n\n\
         | Day | Hours |\n| --- | ---: |\n| Monday | 6 |\n\n\
         Book the room\n- When\n\n\
         Which date?\n- Tuesday\n\n\
         Run a command\necho hi\n\n\
         Reading the calendar\n\n\
         a red square\n\n\
         notes.txt\n\n\
         [live screen]\n\n\
         Beam it up"
    );
}

#[tokio::test]
async fn a_user_send_projects_its_blocks_into_text_content() {
    let daemon = TestDaemon::start().await;
    let upload = reqwest::Client::new()
        .post(format!("{}/api/v1/artifacts", daemon.base_url))
        .header("cookie", daemon.cookie())
        .multipart(
            reqwest::multipart::Form::new().part(
                "file",
                reqwest::multipart::Part::bytes(b"data".to_vec())
                    .file_name("notes.txt")
                    .mime_str("text/plain")
                    .unwrap(),
            ),
        )
        .send()
        .await
        .unwrap();
    let artifact: serde_json::Value = upload.json().await.unwrap();

    let sent: serde_json::Value = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "pending_id": "p1",
            "text": "Keep this.",
            "artifact_ids": [artifact["id"]],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(sent["blocks"][0]["type"], "markdown");
    assert_eq!(sent["blocks"][1]["type"], "file");
    assert_eq!(sent["text_content"], "Keep this.\n\nnotes.txt");
}
