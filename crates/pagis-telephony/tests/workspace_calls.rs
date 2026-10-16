//! A live Workspace Number Grant controls outbound Calls.
use async_trait::async_trait;
use pagis_broker::{AuthorizedCall, CoreTool, ToolExecutor, ToolRoute};
use pagis_core::*;
use pagis_telephony::fake::{FakeCallTransport, TokioClock};
use pagis_telephony::{FakeCallBridge, LiveCalls, PhoneToolDeps, PhoneToolRuntime, RunTools};
use std::sync::Arc;

struct Tools;
#[async_trait]
impl RunTools for Tools {
    async fn free_tools(
        &self,
        _: &WorkspaceId,
        _: &RunId,
    ) -> Result<Vec<pagis_broker::ToolDef>, String> {
        Ok(vec![])
    }
}

async fn attempt(number: bool, grant: bool, revoked: bool, to: &str) {
    let (stores, _, call, _) = super::handoff::setup().await;
    if !number {
        stores
            .phone_numbers
            .release(&call.workspace_id, &call.phone_number_id)
            .await
            .unwrap();
    }
    if grant {
        stores
            .grants
            .create(&Grant {
                id: GrantId::generate(),
                workspace_id: call.workspace_id.clone(),
                agent_id: call.agent_id.clone(),
                resource_kind: Grant::WORKSPACE_NUMBER_KIND.into(),
                resource_id: Some(call.phone_number_id.to_string()),
                scope: serde_json::json!({}),
                revision: 1,
                created_by: Author::System,
                created_at: now_ms(),
                revoked_at: revoked.then(now_ms),
            })
            .await
            .unwrap();
    }
    let bridge = Arc::new(FakeCallBridge::reporting(FakeCallBridge::answered_report(
        "Done",
    )));
    let runtime = PhoneToolRuntime::new(PhoneToolDeps {
        numbers: stores.phone_numbers.clone(),
        grants: stores.grants.clone(),
        agents: stores.agents.clone(),
        workspaces: stores.workspaces.clone(),
        users: stores.users.clone(),
        runs: Arc::new(Tools),
        run_records: stores.runs.clone(),
        messages: stores.messages.clone(),
        calls: stores.calls.clone(),
        tiers: stores.trust_list.clone(),
        transport: Arc::new(FakeCallTransport::new(Arc::new(TokioClock))),
        bridge: bridge.clone(),
        live: Arc::new(LiveCalls::default()),
        files: stores.files.clone(),
        blobs: Arc::new(object_store::memory::InMemory::new()),
        bus: Arc::new(pagis_audit::AuditEventBus::new(stores.events.clone())),
    });
    let result = runtime
        .execute(AuthorizedCall {
            workspace_id: call.workspace_id.clone(),
            agent_id: call.agent_id.clone(),
            run_id: call.run_id.clone(),
            tool_name: "phone_call".into(),
            source_version: "test".into(),
            route: ToolRoute::Core {
                tool: CoreTool::PhoneCall,
            },
            arguments: serde_json::json!({"to":to,"purpose":"Book a table"}),
            selected_connection: None,
            grant_id: None,
            grant_revision: None,
            call_timeout: None,
            tool_call_id: None,
            host: None,
            approved_by_rule: false,
        })
        .await;
    let allowed = number && grant && !revoked && to != "+1911";
    assert_eq!(result.is_error, !allowed, "{result:?}");
    if !allowed {
        let expected = if !number {
            "workspace_number_missing"
        } else if !grant || revoked {
            "permission_revoked"
        } else {
            "emergency_number_refused"
        };
        assert_eq!(result.code.as_deref(), Some(expected));
    }
    assert_eq!(bridge.placed().len(), usize::from(allowed));
    if allowed {
        let placed = bridge.placed();
        assert_eq!(placed[0].brief.own_e164, call.own_e164);
        assert_eq!(placed[0].brief.phone_number_id, call.phone_number_id);
        assert_eq!(placed[0].brief.agent_id, call.agent_id);
        assert_eq!(placed[0].run_id, call.run_id);
        assert_eq!(placed[0].brief.purpose, "Book a table");
    }
}
#[tokio::test]
async fn no_grant_refuses_without_dial() {
    attempt(true, false, false, "+16505550100").await;
}
#[tokio::test]
async fn revoked_grant_refuses_without_dial() {
    attempt(true, true, true, "+16505550100").await;
}
#[tokio::test]
async fn live_grant_calls_from_workspace_number() {
    attempt(true, true, false, "+16505550100").await;
}
#[tokio::test]
async fn no_number_refuses_without_dial() {
    attempt(false, true, false, "+16505550100").await;
}
#[tokio::test]
async fn emergency_number_refuses_without_dial() {
    attempt(true, true, false, "+1911").await;
}
