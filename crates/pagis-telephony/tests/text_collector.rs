//! Text Records belong to the Workspace Number and follow its Chief of Staff.
use async_trait::async_trait;
use pagis_core::*;
use pagis_telephony::fake::FakeTextTransport;
use pagis_telephony::text::{InboundText, TextCollector, TextEvents, TextTransports};
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct Inbox(Mutex<Vec<(AgentId, TextRecord)>>);
#[async_trait]
impl TextEvents for Inbox {
    async fn received(
        &self,
        _: &PhoneNumber,
        agent: &AgentId,
        record: &TextRecord,
    ) -> Result<(), String> {
        self.0.lock().unwrap().push((agent.clone(), record.clone()));
        Ok(())
    }
}
#[tokio::test(start_paused = true)]
async fn inbound_texts_follow_the_designation_and_keep_the_tier() {
    let (stores, _, call, next) = super::handoff::setup().await;
    let number = stores
        .phone_numbers
        .get(&call.workspace_id, &call.phone_number_id)
        .await
        .unwrap()
        .unwrap();
    let connection = Connection {
        id: number.connection_id.clone(),
        workspace_id: call.workspace_id.clone(),
        provider: "telnyx".into(),
        alias: "carrier".into(),
        display_name: "Telnyx".into(),
        status: "connected".into(),
        authorized_capabilities: vec![],
        config: serde_json::json!({}),
        created_at: now_ms(),
    };
    stores.connections.create(&connection).await.unwrap();
    stores
        .trust_list
        .upsert(&TrustEntry {
            id: TrustEntryId::generate(),
            workspace_id: call.workspace_id.clone(),
            agent_id: None,
            subject: TrustSubject::Number,
            value: call.remote_e164.clone(),
            tier: TrustTier::Owner,
            label: "Person".into(),
            created_at: now_ms(),
        })
        .await
        .unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    secrets
        .set(
            &pagis_telephony::carrier_key_secret_name("telnyx", "carrier"),
            "key",
        )
        .unwrap();
    let transport = Arc::new(FakeTextTransport::default());
    let inbox = Arc::new(Inbox::default());
    let collector = Arc::new(TextCollector {
        numbers: stores.phone_numbers.clone(),
        workspaces: stores.workspaces.clone(),
        agents: stores.agents.clone(),
        connections: stores.connections.clone(),
        org_workspace_id: call.workspace_id.clone(),
        secrets,
        transports: Arc::new(TextTransports::single("telnyx", transport.clone())),
        records: stores.text_records.clone(),
        trust: stores.trust_list.clone(),
        events: inbox.clone(),
    });
    let text = |id: &str| InboundText {
        carrier_id: id.into(),
        from_e164: call.remote_e164.clone(),
        to_e164: number.e164.clone(),
        body: "Please book a table.".into(),
        received_at: now_ms(),
        media_urls: vec![],
    };
    transport.push_inbound(text("first"));
    let cancel = tokio_util::sync::CancellationToken::new();
    let task = collector.spawn(cancel.clone());
    wait_for(&inbox, 1).await;
    {
        let inbox = inbox.0.lock().unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].0, call.agent_id);
        assert_eq!(inbox[0].1.workspace_id, call.workspace_id);
        assert_eq!(inbox[0].1.tier, TrustTier::Owner);
        assert_eq!(inbox[0].1.body, "Please book a table.");
    }
    stores
        .workspaces
        .set_chief_of_staff(&call.workspace_id, Some(&next.id))
        .await
        .unwrap();
    transport.push_inbound(text("second"));
    tokio::time::advance(pagis_telephony::text::TEXT_POLL_INTERVAL).await;
    wait_for(&inbox, 2).await;
    assert_eq!(inbox.0.lock().unwrap()[1].0, next.id);
    stores
        .phone_numbers
        .release(&call.workspace_id, &number.id)
        .await
        .unwrap();
    transport.push_inbound(text("third"));
    tokio::time::advance(pagis_telephony::text::TEXT_POLL_INTERVAL).await;
    cancel.cancel();
    task.await.unwrap();
    collector.collect(&call.workspace_id).await.unwrap();
    assert_eq!(inbox.0.lock().unwrap().len(), 2);
}

async fn wait_for(inbox: &Inbox, count: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while inbox.0.lock().unwrap().len() < count {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the text collector delivers each message");
}
