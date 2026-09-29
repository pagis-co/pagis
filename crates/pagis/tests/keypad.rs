//! The Keypad Code check the bridge proves a tier against
//! (ADR-0021): the vault's code of the call's Workspace, read through
//! telephony's `CodeCheck`, and the failed-attempt count the Person
//! clears in Settings.

use std::sync::Arc;

use pagis::keypad::VaultCodeCheck;
use pagis_audit::AuditEventBus;
use pagis_core::{
    AgentId, CallDirection, CallId, MemorySecretStore, PhoneNumberId, RunId, SecretStore,
    SystemClock, TrustTier, WorkspaceId, now_ms,
};
use pagis_telephony::keypad::CodeCheck;
use pagis_telephony::{
    CLASSIFY_PROMPT, CallBrief, CallLog, DEFAULT_DURATION_CAP, EMERGENCY_RULE, IVR_MODE_PROMPT,
    Keypad, PlacedCall, TAKE_A_MESSAGE, TierGate, VoicemailPolicy,
};
use pagis_testkit::{TestDaemon, TestDaemonOptions};
use pagis_vault::KeypadCode;

#[tokio::test]
async fn a_workspace_with_no_code_proves_nothing() {
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    let check = VaultCodeCheck::new(Arc::clone(&secrets));
    let workspace_id = WorkspaceId::generate();

    assert!(!check.is_set(&workspace_id).await);
    assert!(!check.verify(&workspace_id, "246813").await);
}

#[tokio::test]
async fn the_code_the_user_set_is_the_one_that_verifies() {
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    let workspace_id = WorkspaceId::generate();
    KeypadCode::new(secrets.as_ref(), &workspace_id)
        .set("246813")
        .expect("the code is stored");
    let check = VaultCodeCheck::new(Arc::clone(&secrets));

    assert!(check.is_set(&workspace_id).await);
    assert!(check.verify(&workspace_id, "246813").await);
    assert!(!check.verify(&workspace_id, "000000").await);
}

/// Two people set different codes. Each code confirms only calls to
/// the Workspace of the person who set it, so it confirms only calls
/// to that person's Agents.
#[tokio::test]
async fn each_code_confirms_only_its_own_workspace() {
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    let ada = WorkspaceId::generate();
    let grace = WorkspaceId::generate();
    KeypadCode::new(secrets.as_ref(), &ada)
        .set("246813")
        .expect("Ada's code is stored");
    KeypadCode::new(secrets.as_ref(), &grace)
        .set("135792")
        .expect("Grace's code is stored");
    let check = VaultCodeCheck::new(Arc::clone(&secrets));

    assert!(check.verify(&ada, "246813").await);
    assert!(!check.verify(&ada, "135792").await);
    assert!(check.verify(&grace, "135792").await);
    assert!(!check.verify(&grace, "246813").await);
}

/// The audit trail of one inbound Call to a Workspace of the daemon, on
/// the daemon's own event log.
fn call_log(daemon: &TestDaemon) -> CallLog {
    let call = PlacedCall {
        id: CallId::generate(),
        workspace_id: daemon.workspace_id.clone(),
        run_id: RunId::generate(),
        brief: CallBrief {
            direction: CallDirection::Inbound,
            agent_id: AgentId::from(daemon.agent_id.clone()),
            agent_name: "Robin".to_string(),
            voice: None,
            phone_number_id: PhoneNumberId::generate(),
            own_e164: "+14155550123".to_string(),
            remote_e164: "+14155550100".to_string(),
            tier: TrustTier::Unknown,
            purpose: TAKE_A_MESSAGE.to_string(),
            success_criteria: None,
            voicemail: VoicemailPolicy::HangUp,
            duration_cap: DEFAULT_DURATION_CAP,
            tools: Vec::new(),
            ivr_mode_prompt: IVR_MODE_PROMPT,
            classify_prompt: CLASSIFY_PROMPT,
            emergency_rule: EMERGENCY_RULE,
        },
        tools: Vec::new(),
    };
    CallLog::new(
        Arc::new(AuditEventBus::new(daemon.stores().events.clone())),
        &call,
    )
}

/// Type the code after the session started, on a new Call from an
/// Owner-listed number. `Some` is the tier it raised the Call to.
async fn code_on_a_new_call(
    daemon: &TestDaemon,
    keypad: &Keypad,
    digits: &str,
) -> Option<TrustTier> {
    let gate = TierGate::inbound(
        TrustTier::Owner,
        daemon.workspace_id.clone(),
        keypad.clone(),
        call_log(daemon),
    );
    let mut raised = None;
    for digit in digits.chars().chain(['#']) {
        if let Some(change) = gate.late_digit(digit).await {
            raised = Some(change.to);
        }
    }
    raised
}

/// The Keypad Code card of the Person's Workspace, as Settings reads it.
async fn keypad_card(daemon: &TestDaemon) -> serde_json::Value {
    let page: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/api/v1/settings/trust-list", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("GET the trust list")
        .error_for_status()
        .expect("the trust list")
        .json()
        .await
        .expect("a trust list page");
    page["keypad_code"].clone()
}

/// A delay starts at the sixth wrong code. Settings shows the count and
/// the end of the delay, and the Person clears it there. The next
/// correct code then raises the tier at once, although the delay had
/// not ended (ADR-0021).
#[tokio::test]
async fn the_person_clears_the_count_in_settings_and_the_code_works_at_once() {
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        secrets: Arc::clone(&secrets),
        ..TestDaemonOptions::default()
    })
    .await;
    let client = reqwest::Client::new();
    let set = client
        .put(format!("{}/api/v1/settings/keypad-code", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "code": "246813" }))
        .send()
        .await
        .expect("PUT the code");
    assert_eq!(set.status(), 200);
    let keypad = Keypad {
        code: Arc::new(VaultCodeCheck::new(Arc::clone(&secrets))),
        failures: daemon.stores().keypad_failures.clone(),
        clock: Arc::new(SystemClock),
    };
    for _ in 0..7 {
        assert_eq!(code_on_a_new_call(&daemon, &keypad, "975319").await, None);
    }

    let card = keypad_card(&daemon).await;
    assert_eq!(
        card["failed_attempts"], 6,
        "the seventh code came during the delay, so it was not checked"
    );
    let until = card["suspended_until"].as_i64().expect("a delay runs");
    assert!(until > now_ms(), "the delay has not ended");
    assert_eq!(
        code_on_a_new_call(&daemon, &keypad, "246813").await,
        None,
        "the code works during the delay"
    );

    let cleared = client
        .delete(format!(
            "{}/api/v1/settings/keypad-code/failures",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("DELETE the failed attempts");
    assert_eq!(cleared.status(), 204);

    let card = keypad_card(&daemon).await;
    assert_eq!(card["failed_attempts"], 0);
    assert_eq!(card["suspended_until"], serde_json::Value::Null);
    assert_eq!(
        code_on_a_new_call(&daemon, &keypad, "246813").await,
        Some(TrustTier::Owner)
    );
}
