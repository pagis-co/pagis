//! What an Administrator configures, on both backends.
//!
//! Two traits meet here: the account fields an Administrator manages on
//! a Person, and the Usage Record the agent loop writes and the
//! Administrator reads. Name every new body in
//! `store_suite_installation!` below; the guard test of the parent
//! module fails while one is missing.

use pagis_core::{
    ClientKind, Run, RunId, RunState, Session, SessionId, TriggerKind, UsageId, UsagePeriod,
    UsageRecord, UsageTotal, User, UserRole, Workspace, WorkspaceId, now_ms,
    seed_org_and_administrator,
};

use super::Backend;

/// A whole month's worth of period, so a body never has to think about
/// the clock.
const EVERYTHING: UsagePeriod = UsagePeriod {
    from: 0,
    to: i64::MAX,
};

/// A second Workspace of the same Org, for the reads that cross one.
async fn second_workspace(backend: &Backend, first: &Workspace) -> Workspace {
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
    second
}

/// The Run a Usage Record points at. The foreign key is real on both
/// backends, so a body writes the Agent and the Run first.
async fn seeded_run(backend: &Backend, workspace_id: &WorkspaceId) -> RunId {
    let agent = crate::fixture::agent(workspace_id);
    backend
        .stores()
        .agents
        .create(&agent)
        .await
        .expect("write the Agent");
    let run = Run {
        id: RunId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent.id,
        channel_id: None,
        root_message_id: None,
        trigger_kind: TriggerKind::Message,
        trigger_ref: None,
        hop_count: 0,
        origin: None,
        state: RunState::Queued,
        error: None,
        failure_kind: None,
        created_at: now_ms(),
        started_at: None,
        ended_at: None,
    };
    backend
        .stores()
        .runs
        .create(&run)
        .await
        .expect("write the Run");
    run.id
}

fn record(
    workspace_id: &WorkspaceId,
    run_id: &RunId,
    cost: impl Into<Option<f64>>,
    at: i64,
) -> UsageRecord {
    UsageRecord {
        id: UsageId::generate(),
        workspace_id: workspace_id.clone(),
        run_id: run_id.clone(),
        provider: Some("anthropic".to_string()),
        model: Some("claude-sonnet-4-6".to_string()),
        input_tokens: 100,
        output_tokens: 20,
        cache_read_tokens: 10,
        cache_write_tokens: 5,
        cost_usd: cost.into(),
        created_at: at,
    }
}

pub async fn a_usage_record_sums_per_workspace(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let run = seeded_run(backend, &workspace.id).await;
    let usage = &backend.stores().usage;

    usage
        .record(&record(&workspace.id, &run, 0.25, 1_000))
        .await
        .unwrap();
    usage
        .record(&record(&workspace.id, &run, 0.75, 2_000))
        .await
        .unwrap();

    let total = usage
        .total_for_workspace(&workspace.id, EVERYTHING)
        .await
        .unwrap();
    assert_eq!(
        total,
        UsageTotal {
            input_tokens: 200,
            output_tokens: 40,
            cache_read_tokens: 20,
            cache_write_tokens: 10,
            cost_usd: 1.0,
            calls: 2,
            unpriced_calls: 0,
        }
    );
}

/// A call with an unknown cost counts as a call and adds no money: the
/// total says how many of its calls have no price, never a zero cost.
pub async fn an_unpriced_call_counts_and_adds_no_cost(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let run = seeded_run(backend, &workspace.id).await;
    let usage = &backend.stores().usage;
    usage
        .record(&record(&workspace.id, &run, 0.5, 1_000))
        .await
        .unwrap();
    usage
        .record(&record(&workspace.id, &run, None, 2_000))
        .await
        .unwrap();

    let total = usage
        .total_for_workspace(&workspace.id, EVERYTHING)
        .await
        .unwrap();

    assert_eq!(total.calls, 2);
    assert_eq!(total.unpriced_calls, 1);
    assert_eq!(total.cost_usd, 0.5);
}

/// An empty period is zero on both backends, not a missing row: the
/// Spend Cap reads this before a person has spent anything.
pub async fn an_empty_period_totals_to_zero(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;

    let total = backend
        .stores()
        .usage
        .total_for_workspace(&workspace.id, EVERYTHING)
        .await
        .unwrap();

    assert_eq!(total, UsageTotal::default());
}

/// The period is half-open: `from` is in it and `to` is not.
pub async fn the_period_takes_its_start_and_leaves_its_end(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let run = seeded_run(backend, &workspace.id).await;
    let usage = &backend.stores().usage;
    usage
        .record(&record(&workspace.id, &run, 1.0, 1_000))
        .await
        .unwrap();
    usage
        .record(&record(&workspace.id, &run, 2.0, 2_000))
        .await
        .unwrap();

    let total = usage
        .total_for_workspace(
            &workspace.id,
            UsagePeriod {
                from: 1_000,
                to: 2_000,
            },
        )
        .await
        .unwrap();

    assert_eq!(total.calls, 1);
    assert_eq!(total.cost_usd, 1.0);
}

/// The Administrator's roster read: one row per Workspace, the biggest
/// spender first, and no other Workspace's rows folded into a total.
pub async fn totals_by_workspace_names_every_workspace_highest_first(backend: &Backend) {
    let first = backend.seeded_workspace().await;
    let first_run = seeded_run(backend, &first.id).await;
    let second = second_workspace(backend, &first).await;
    let second_run = seeded_run(backend, &second.id).await;
    let usage = &backend.stores().usage;
    usage
        .record(&record(&first.id, &first_run, 1.0, 1_000))
        .await
        .unwrap();
    usage
        .record(&record(&second.id, &second_run, 4.0, 1_000))
        .await
        .unwrap();

    let totals = usage.totals_by_workspace(EVERYTHING).await.unwrap();

    let ids: Vec<String> = totals
        .iter()
        .map(|row| row.workspace_id.to_string())
        .collect();
    assert_eq!(ids, vec![second.id.to_string(), first.id.to_string()]);
    assert_eq!(totals[0].total.cost_usd, 4.0);
    assert_eq!(totals[1].total.cost_usd, 1.0);
}

/// The person's own read: one row per Run, newest first.
pub async fn runs_for_a_workspace_group_by_run_newest_first(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let older = seeded_run(backend, &workspace.id).await;
    let newer = seeded_run(backend, &workspace.id).await;
    let usage = &backend.stores().usage;
    usage
        .record(&record(&workspace.id, &older, 1.0, 1_000))
        .await
        .unwrap();
    usage
        .record(&record(&workspace.id, &older, 1.0, 1_500))
        .await
        .unwrap();
    usage
        .record(&record(&workspace.id, &newer, 3.0, 2_000))
        .await
        .unwrap();

    let runs = usage
        .runs_for_workspace(&workspace.id, EVERYTHING, 10)
        .await
        .unwrap();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].run_id, newer);
    assert_eq!(runs[0].last_at, 2_000);
    assert_eq!(runs[1].run_id, older);
    assert_eq!(runs[1].total.calls, 2);
    assert_eq!(runs[1].total.cost_usd, 2.0);
    assert_eq!(runs[1].last_at, 1_500);
}

/// A Usage Record of another Workspace never reaches this one's read.
pub async fn one_workspace_never_reads_another_workspace_usage(backend: &Backend) {
    let mine = backend.seeded_workspace().await;
    let theirs = second_workspace(backend, &mine).await;
    let their_run = seeded_run(backend, &theirs.id).await;
    let usage = &backend.stores().usage;
    usage
        .record(&record(&theirs.id, &their_run, 9.0, 1_000))
        .await
        .unwrap();

    assert_eq!(
        usage
            .total_for_workspace(&mine.id, EVERYTHING)
            .await
            .unwrap(),
        UsageTotal::default()
    );
    assert!(
        usage
            .runs_for_workspace(&mine.id, EVERYTHING, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn an_account_is_disabled_and_given_back(backend: &Backend) {
    let person = seed_org_and_administrator(
        backend.stores().orgs.as_ref(),
        backend.stores().users.as_ref(),
        "Org",
        now_ms(),
    )
    .await
    .unwrap();
    let users = &backend.stores().users;
    assert!(!users.get(&person.id).await.unwrap().unwrap().is_disabled());

    assert!(
        users
            .set_disabled(&person.id, Some(5_000), 5_000)
            .await
            .unwrap()
    );
    let disabled = users.get(&person.id).await.unwrap().unwrap();
    assert_eq!(disabled.disabled_at, Some(5_000));
    assert!(disabled.is_disabled());
    assert_eq!(disabled.updated_at, 5_000);

    assert!(users.set_disabled(&person.id, None, 6_000).await.unwrap());
    assert!(!users.get(&person.id).await.unwrap().unwrap().is_disabled());
}

pub async fn a_password_is_reset_and_taken_away(backend: &Backend) {
    let person = seed_org_and_administrator(
        backend.stores().orgs.as_ref(),
        backend.stores().users.as_ref(),
        "Org",
        now_ms(),
    )
    .await
    .unwrap();
    let users = &backend.stores().users;
    assert_eq!(person.password_hash, None);

    assert!(
        users
            .set_password_hash(&person.id, Some("$argon2id$fake"), 1_000)
            .await
            .unwrap()
    );
    assert_eq!(
        users.get(&person.id).await.unwrap().unwrap().password_hash,
        Some("$argon2id$fake".to_string())
    );

    assert!(
        users
            .set_password_hash(&person.id, None, 2_000)
            .await
            .unwrap()
    );
    assert_eq!(
        users.get(&person.id).await.unwrap().unwrap().password_hash,
        None
    );
}

pub async fn a_spend_cap_is_set_and_cleared(backend: &Backend) {
    let person = seed_org_and_administrator(
        backend.stores().orgs.as_ref(),
        backend.stores().users.as_ref(),
        "Org",
        now_ms(),
    )
    .await
    .unwrap();
    let users = &backend.stores().users;
    assert_eq!(person.monthly_spend_cap_usd, None);

    assert!(
        users
            .set_monthly_spend_cap(&person.id, Some(25.5), 1_000)
            .await
            .unwrap()
    );
    assert_eq!(
        users
            .get(&person.id)
            .await
            .unwrap()
            .unwrap()
            .monthly_spend_cap_usd,
        Some(25.5)
    );

    assert!(
        users
            .set_monthly_spend_cap(&person.id, None, 2_000)
            .await
            .unwrap()
    );
    assert_eq!(
        users
            .get(&person.id)
            .await
            .unwrap()
            .unwrap()
            .monthly_spend_cap_usd,
        None
    );
}

/// A sign-in is recorded for the roster, and it does not touch
/// `updated_at`: a sign-in is not a change an Administrator made.
pub async fn a_sign_in_is_recorded_on_the_person(backend: &Backend) {
    let person = seed_org_and_administrator(
        backend.stores().orgs.as_ref(),
        backend.stores().users.as_ref(),
        "Org",
        now_ms(),
    )
    .await
    .unwrap();
    let users = &backend.stores().users;

    users.record_sign_in(&person.id, 9_000).await.unwrap();

    let read = users.get(&person.id).await.unwrap().unwrap();
    assert_eq!(read.last_signed_in_at, Some(9_000));
    assert_eq!(read.updated_at, person.updated_at);
}

/// The first Administrator is claimed one time. A second claim, which is
/// a second Server Setup that found nobody who can sign in before the
/// first one wrote, answers `false` and changes neither the address nor
/// the password hash.
pub async fn the_first_administrator_is_claimed_once(backend: &Backend) {
    let person = seed_org_and_administrator(
        backend.stores().orgs.as_ref(),
        backend.stores().users.as_ref(),
        "Org",
        now_ms(),
    )
    .await
    .unwrap();
    let users = &backend.stores().users;

    assert!(
        users
            .claim_first_administrator(&person.id, "Ada@Example.COM", "$argon2id$ada", 1_000)
            .await
            .unwrap()
    );
    assert!(
        !users
            .claim_first_administrator(
                &person.id,
                "mallory@example.com",
                "$argon2id$mallory",
                2_000
            )
            .await
            .unwrap()
    );

    let claimed = users.get(&person.id).await.unwrap().unwrap();
    assert_eq!(claimed.email.as_deref(), Some("ada@example.com"));
    assert_eq!(claimed.password_hash.as_deref(), Some("$argon2id$ada"));
    assert_eq!(claimed.updated_at, 1_000);
}

/// No claim wins while another Administrator of the Org holds a
/// password, because somebody can sign in already. A Member is never
/// the first Administrator.
pub async fn no_claim_wins_once_somebody_can_sign_in(backend: &Backend) {
    let seeded = seed_org_and_administrator(
        backend.stores().orgs.as_ref(),
        backend.stores().users.as_ref(),
        "Org",
        now_ms(),
    )
    .await
    .unwrap();
    let users = &backend.stores().users;
    let member = User::new(seeded.org_id.clone(), UserRole::Member, now_ms());
    users.create(&member).await.unwrap();

    assert!(
        !users
            .claim_first_administrator(&member.id, "lin@example.com", "$argon2id$lin", 1_000)
            .await
            .unwrap()
    );

    let signed_in = User {
        email: Some("grace@example.com".to_string()),
        password_hash: Some("$argon2id$grace".to_string()),
        ..User::new(seeded.org_id.clone(), UserRole::Administrator, now_ms())
    };
    users.create(&signed_in).await.unwrap();

    assert!(
        !users
            .claim_first_administrator(
                &seeded.id,
                "mallory@example.com",
                "$argon2id$mallory",
                2_000
            )
            .await
            .unwrap()
    );

    for id in [&seeded.id, &member.id] {
        let unchanged = users.get(id).await.unwrap().unwrap();
        assert_eq!(unchanged.email, None);
        assert_eq!(unchanged.password_hash, None);
    }
}

/// A write against a Person who is not there answers `false` rather
/// than reporting success on nothing.
pub async fn a_missing_person_refuses_every_administrator_write(backend: &Backend) {
    let users = &backend.stores().users;
    let missing = pagis_core::UserId::generate();

    assert!(
        !users
            .claim_first_administrator(&missing, "ada@example.com", "$argon2id$ada", 1)
            .await
            .unwrap()
    );
    assert!(!users.set_disabled(&missing, Some(1), 1).await.unwrap());
    assert!(
        !users
            .set_password_hash(&missing, Some("x"), 1)
            .await
            .unwrap()
    );
    assert!(
        !users
            .set_monthly_spend_cap(&missing, Some(1.0), 1)
            .await
            .unwrap()
    );
}

/// A second Person of the same Org keeps their own role and cap: the
/// role is one column with two values and nothing else keys on it.
pub async fn two_people_of_one_org_keep_their_own_role(backend: &Backend) {
    let administrator = seed_org_and_administrator(
        backend.stores().orgs.as_ref(),
        backend.stores().users.as_ref(),
        "Org",
        now_ms(),
    )
    .await
    .unwrap();
    let users = &backend.stores().users;
    let member = User {
        email: Some("Grace@Example.COM".to_string()),
        name: Some("Grace".to_string()),
        ..User::new(administrator.org_id.clone(), UserRole::Member, now_ms())
    };
    users.create(&member).await.unwrap();

    let roster = users.list_by_org(&administrator.org_id).await.unwrap();
    assert_eq!(roster.len(), 2);
    // The address is stored folded, so one address is one Person
    // whatever a client typed.
    assert_eq!(
        users
            .find_by_email("grace@example.com")
            .await
            .unwrap()
            .map(|person| person.role),
        Some(UserRole::Member)
    );
    assert_eq!(
        users.get(&administrator.id).await.unwrap().unwrap().role,
        UserRole::Administrator
    );
}

/// Every live Session of the installation, newest first, and none that
/// expired. The Administration Interface says who is signed in,
/// so the read crosses every Person of the Org.
pub async fn every_live_session_is_listed_newest_first(backend: &Backend) {
    let administrator = seed_org_and_administrator(
        backend.stores().orgs.as_ref(),
        backend.stores().users.as_ref(),
        "Org",
        now_ms(),
    )
    .await
    .unwrap();
    let member = User {
        email: Some("grace@example.com".to_string()),
        ..User::new(administrator.org_id.clone(), UserRole::Member, now_ms())
    };
    backend.stores().users.create(&member).await.unwrap();
    let sessions = &backend.stores().sessions;
    let desktop = Session {
        id: SessionId::generate(),
        user_id: administrator.id.clone(),
        token_hash: "hash-desktop".to_string(),
        client_kind: ClientKind::Desktop,
        client_name: Some("ada-mac".to_string()),
        created_at: 1_000,
        last_used_at: 1_500,
        expires_at: 9_000,
    };
    let browser = Session {
        id: SessionId::generate(),
        user_id: member.id.clone(),
        token_hash: "hash-browser".to_string(),
        client_kind: ClientKind::Browser,
        client_name: None,
        created_at: 2_000,
        last_used_at: 2_000,
        expires_at: 9_000,
    };
    let expired = Session {
        id: SessionId::generate(),
        user_id: member.id.clone(),
        token_hash: "hash-expired".to_string(),
        client_kind: ClientKind::Browser,
        client_name: None,
        created_at: 500,
        last_used_at: 500,
        expires_at: 3_000,
    };
    for session in [&desktop, &browser, &expired] {
        sessions.create(session).await.unwrap();
    }

    let live = sessions.list_live(4_000).await.unwrap();

    assert_eq!(
        live.iter()
            .map(|session| (session.id.clone(), session.client_kind))
            .collect::<Vec<_>>(),
        vec![
            (browser.id.clone(), ClientKind::Browser),
            (desktop.id.clone(), ClientKind::Desktop),
        ]
    );
    assert_eq!(live[0].user_id, member.id);
    assert_eq!(live[1].client_name.as_deref(), Some("ada-mac"));
    assert_eq!(live[1].created_at, 1_000);
}

/// A Session is found by its id while it is live. A pending Google
/// authorization names the Session that started it and reads it again
/// when Google sends the browser back.
pub async fn a_live_session_is_found_by_its_id(backend: &Backend) {
    let administrator = seed_org_and_administrator(
        backend.stores().orgs.as_ref(),
        backend.stores().users.as_ref(),
        "Org",
        now_ms(),
    )
    .await
    .unwrap();
    let sessions = &backend.stores().sessions;
    let session = Session {
        id: SessionId::generate(),
        user_id: administrator.id.clone(),
        token_hash: "hash-live".to_string(),
        client_kind: ClientKind::Desktop,
        client_name: Some("ada-mac".to_string()),
        created_at: 1_000,
        last_used_at: 1_000,
        expires_at: 9_000,
    };
    sessions.create(&session).await.unwrap();

    let found = sessions
        .find_live_by_id(&session.id, 4_000)
        .await
        .unwrap()
        .expect("the live Session is found");

    assert_eq!(found.user_id, administrator.id);
    assert_eq!(found.client_kind, ClientKind::Desktop);
    assert!(
        sessions
            .find_live_by_id(&session.id, 9_000)
            .await
            .unwrap()
            .is_none(),
        "an expired Session is found"
    );
    assert!(
        sessions
            .find_live_by_id(&SessionId::generate(), 4_000)
            .await
            .unwrap()
            .is_none()
    );
    sessions.delete(&session.id).await.unwrap();
    assert!(
        sessions
            .find_live_by_id(&session.id, 4_000)
            .await
            .unwrap()
            .is_none(),
        "a Session that signed out is found"
    );
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_installation {
    ($emit:path) => {
        $emit!(
            installation,
            a_usage_record_sums_per_workspace,
            an_unpriced_call_counts_and_adds_no_cost,
            an_empty_period_totals_to_zero,
            the_period_takes_its_start_and_leaves_its_end,
            totals_by_workspace_names_every_workspace_highest_first,
            runs_for_a_workspace_group_by_run_newest_first,
            one_workspace_never_reads_another_workspace_usage,
            an_account_is_disabled_and_given_back,
            a_password_is_reset_and_taken_away,
            a_spend_cap_is_set_and_cleared,
            a_sign_in_is_recorded_on_the_person,
            the_first_administrator_is_claimed_once,
            no_claim_wins_once_somebody_can_sign_in,
            a_missing_person_refuses_every_administrator_write,
            two_people_of_one_org_keep_their_own_role,
            every_live_session_is_listed_newest_first,
            a_live_session_is_found_by_its_id,
        );
    };
}
