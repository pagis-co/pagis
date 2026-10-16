//! Persons share one conversation through their Private Workspaces.
use futures::{SinkExt, StreamExt};
use pagis_core::{ChannelId, User, UserRole, Workspace, WorkspaceId};
use pagis_testkit::TestDaemon;
use serde_json::{Value, json};

async fn person(daemon: &TestDaemon, name: &str) -> (WorkspaceId, String) {
    let stores = daemon.stores();
    let owner = stores.users.get(&daemon.user_id).await.unwrap().unwrap();
    let user = User {
        name: Some(name.into()),
        ..User::new(owner.org_id, UserRole::Member, pagis_core::now_ms())
    };
    stores.users.create(&user).await.unwrap();
    let workspace = Workspace {
        delegate_everything: false,
        kind: pagis_core::WorkspaceKind::Private,
        id: WorkspaceId::generate(),
        user_id: Some(user.id.clone()),
        name: name.into(),
        timezone: "UTC".into(),
        legal_hold: None,
        monthly_spend_cap_usd: None,
        created_at: pagis_core::now_ms(),
        onboarded_at: None,
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
        home_exit_host_id: None,
    };
    stores.workspaces.create(&workspace).await.unwrap();
    for mut alias in stores
        .model_aliases
        .list(&daemon.workspace_id)
        .await
        .unwrap()
    {
        alias.id = pagis_core::ModelAliasId::generate();
        alias.workspace_id = workspace.id.clone();
        stores.model_aliases.create(&alias).await.unwrap();
    }
    (workspace.id, daemon.cookie_for(&user.id).await)
}

async fn request(
    daemon: &TestDaemon,
    cookie: &str,
    method: reqwest::Method,
    path: &str,
    body: Value,
) -> reqwest::Response {
    daemon
        .client()
        .request(method, format!("{}{path}", daemon.base_url))
        .header("cookie", cookie)
        .json(&body)
        .send()
        .await
        .unwrap()
}
async fn get(daemon: &TestDaemon, cookie: &str, path: &str) -> Value {
    request(daemon, cookie, reqwest::Method::GET, path, Value::Null)
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}
async fn post(daemon: &TestDaemon, cookie: &str, path: &str, body: Value) -> Value {
    request(daemon, cookie, reqwest::Method::POST, path, body)
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn hire(daemon: &TestDaemon, cookie: &str, name: &str) -> Value {
    let agent = post(
        daemon,
        cookie,
        "/api/v1/agents",
        json!({"name":name,"job":"assistant"}),
    )
    .await;
    request(
        daemon,
        cookie,
        reqwest::Method::PUT,
        "/api/v1/workspace/chief-of-staff",
        json!({"agent_id":agent["id"]}),
    )
    .await
    .error_for_status()
    .unwrap();
    agent
}

#[tokio::test]
async fn conversations_share_messages_authorship_presence_and_live_delivery() {
    let daemon = TestDaemon::start_in_memory().await;
    daemon
        .stores()
        .users
        .set_name(&daemon.user_id, "Alice", pagis_core::now_ms())
        .await
        .unwrap();
    let (b, cookie_b) = person(&daemon, "Bob").await;
    let (_, cookie_c) = person(&daemon, "Carol").await;
    let colleagues = get(&daemon, daemon.cookie(), "/api/v1/colleagues").await;
    let bob = colleagues["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Bob")
        .unwrap();
    assert_eq!(bob["workspace_id"], json!(b));
    assert_eq!(bob["presence"], "away");
    let mut live = daemon.event_socket(&cookie_b).await;
    live.send(tokio_tungstenite::tungstenite::Message::text(
        json!({"type":"activity"}).to_string(),
    ))
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let colleagues = get(&daemon, daemon.cookie(), "/api/v1/colleagues").await;
            if colleagues["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["name"] == "Bob" && p["presence"] == "active")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let body = json!({"workspace_ids": [b]});
    let conversation = post(
        &daemon,
        daemon.cookie(),
        "/api/v1/conversations",
        body.clone(),
    )
    .await;
    let again = post(
        &daemon,
        &cookie_b,
        "/api/v1/conversations",
        json!({"workspace_ids": [daemon.workspace_id]}),
    )
    .await;
    assert_eq!(conversation["id"], again["id"]);
    let id = conversation["id"].as_str().unwrap();
    let path = format!("/api/v1/conversations/{id}/messages");
    let sent = post(
        &daemon,
        daemon.cookie(),
        &path,
        json!({"text":"Hello Bob", "pending_id":"alice-1"}),
    )
    .await;
    assert_eq!(sent["author"]["name"], "Alice");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let frame: Value =
                serde_json::from_str(&live.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            if frame["type"] == "conversation.message" {
                assert_eq!(frame["payload"]["payload"]["message_id"], sent["id"]);
                assert!(frame["payload"]["payload"]["message"].is_null());
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        get(&daemon, &cookie_b, &path).await["items"][0]["author"]["name"],
        "Alice"
    );
    post(
        &daemon,
        &cookie_b,
        &path,
        json!({"text":"Hello Alice", "pending_id":"bob-1"}),
    )
    .await;
    let page = get(&daemon, daemon.cookie(), &path).await;
    assert_eq!(page["items"][0]["author"]["name"], "Bob");
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    for method in [reqwest::Method::GET, reqwest::Method::POST] {
        assert_eq!(
            request(
                &daemon,
                &cookie_c,
                method,
                &path,
                json!({"text":"No", "pending_id":"c"})
            )
            .await
            .status(),
            404
        );
    }
    assert!(
        get(&daemon, &cookie_c, "/api/v1/conversations").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let owner = daemon
        .stores()
        .orgs
        .list()
        .await
        .unwrap()
        .pop()
        .unwrap()
        .workspace_id;
    let rows = daemon
        .stores()
        .messages
        .list_top_level(&owner, &ChannelId::from(id.to_string()), None, 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    for workspace in [&daemon.workspace_id, &b] {
        assert!(
            daemon
                .stores()
                .messages
                .list_top_level(workspace, &ChannelId::from(id.to_string()), None, 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn away_chief_answers_and_person_only_keeps_context_private() {
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use std::sync::Arc;
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_in_memory_with(TestDaemonOptions {
        brain: brain.clone(),
        ..Default::default()
    })
    .await;
    let (recipient, cookie) = person(&daemon, "Recipient").await;
    let agent = hire(&daemon, &cookie, "Helper").await;
    let agent_id = agent["id"].as_str().unwrap();
    brain.push_for("Helper", Script::reply(&["I can help."]));
    brain.push_for("Helper", Script::reply(&["A private note."]));
    let conversation = post(
        &daemon,
        daemon.cookie(),
        "/api/v1/conversations",
        json!({"workspace_ids":[recipient]}),
    )
    .await;
    let path = format!(
        "/api/v1/conversations/{}/messages",
        conversation["id"].as_str().unwrap()
    );
    post(
        &daemon,
        daemon.cookie(),
        &path,
        json!({"text":"Can you help?", "pending_id":"request", "for_person_only":false}),
    )
    .await;
    let wait = async {
        loop {
            let page = get(&daemon, daemon.cookie(), &path).await;
            if page["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["text_content"] == "I can help." && m["status"] == "complete")
            {
                break page;
            }
            tokio::task::yield_now().await;
        }
    };
    let page = match tokio::time::timeout(std::time::Duration::from_secs(10), wait).await {
        Ok(page) => page,
        Err(_) => panic!(
            "No shared reply. Runs: {}. Model requests: {:?}",
            get(&daemon, &cookie, "/api/v1/runs").await,
            brain.requests()
        ),
    };
    assert_eq!(page["items"][0]["author"]["name"], "Helper, for Recipient");
    let private = post(
        &daemon,
        daemon.cookie(),
        &path,
        json!({"text":"Human review", "pending_id":"private", "for_person_only":true}),
    )
    .await;
    assert_eq!(private["for_person_only"], true);
    let channels = get(&daemon, &cookie, "/api/v1/channels").await;
    let channel = channels["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| {
            c["agent_ids"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == agent_id)
        })
        .unwrap()["id"]
        .as_str()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let page = get(
                &daemon,
                &cookie,
                &format!("/api/v1/channels/{channel}/messages"),
            )
            .await;
            if page.to_string().contains("A private note.") {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !get(&daemon, daemon.cookie(), &path)
            .await
            .to_string()
            .contains("A private note.")
    );
    let requests = brain.requests();
    assert!(requests.iter().any(|r| {
        r.messages
            .iter()
            .any(|m| format!("{:?}", m).contains("trust=colleague"))
    }));
}

#[tokio::test]
async fn active_person_receives_directly_and_delegate_everything_overrides_activity() {
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use std::sync::Arc;
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_in_memory_with(TestDaemonOptions {
        brain: brain.clone(),
        ..Default::default()
    })
    .await;
    let (recipient, cookie) = person(&daemon, "Recipient").await;
    hire(&daemon, &cookie, "Helper").await;
    let mut live = daemon.event_socket(&cookie).await;
    live.send(tokio_tungstenite::tungstenite::Message::text(
        json!({"type":"activity"}).to_string(),
    ))
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let colleagues = get(&daemon, daemon.cookie(), "/api/v1/colleagues").await;
            if colleagues["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["workspace_id"] == json!(recipient) && p["presence"] == "active")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let conversation = post(
        &daemon,
        daemon.cookie(),
        "/api/v1/conversations",
        json!({"workspace_ids":[recipient]}),
    )
    .await;
    let path = format!(
        "/api/v1/conversations/{}/messages",
        conversation["id"].as_str().unwrap()
    );
    post(
        &daemon,
        daemon.cookie(),
        &path,
        json!({"text":"Direct question", "pending_id":"direct"}),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(brain.requests().is_empty());
    assert_eq!(
        get(&daemon, &cookie, &path).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let saved = request(
        &daemon,
        &cookie,
        reqwest::Method::PUT,
        "/api/v1/workspace/delegate-everything",
        json!({"enabled":true}),
    )
    .await;
    assert_eq!(saved.status(), 200);
    assert_eq!(
        get(&daemon, &cookie, "/api/v1/workspace").await["delegate_everything"],
        true
    );
    brain.push_for("Helper", Script::reply(&["Delegated answer"]));
    post(
        &daemon,
        daemon.cookie(),
        &path,
        json!({"text":"Delegated question", "pending_id":"delegated"}),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if get(&daemon, daemon.cookie(), &path)
                .await
                .to_string()
                .contains("Delegated answer")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn agents_between_workspaces_use_the_delegation_hop_cap() {
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use std::sync::Arc;
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_in_memory_with(TestDaemonOptions {
        brain: brain.clone(),
        agents: pagis_agent::AgentLoopConfig {
            max_hops: 1,
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let sender = post(
        &daemon,
        daemon.cookie(),
        "/api/v1/agents",
        json!({"name":"First helper", "job":"assistant"}),
    )
    .await;
    request(
        &daemon,
        daemon.cookie(),
        reqwest::Method::PUT,
        "/api/v1/workspace/chief-of-staff",
        json!({"agent_id":sender["id"]}),
    )
    .await
    .error_for_status()
    .unwrap();
    let (recipient, cookie) = person(&daemon, "Recipient").await;
    hire(&daemon, &cookie, "Second helper").await;
    brain.push_for("Second helper", Script::reply(&["First reply"]));
    brain.push_for("First helper", Script::reply(&["Second reply"]));
    brain.push_for("Second helper", Script::reply(&["Over the cap"]));
    let conversation = post(
        &daemon,
        daemon.cookie(),
        "/api/v1/conversations",
        json!({"workspace_ids":[recipient]}),
    )
    .await;
    let path = format!(
        "/api/v1/conversations/{}/messages",
        conversation["id"].as_str().unwrap()
    );
    post(
        &daemon,
        daemon.cookie(),
        &path,
        json!({"text":"Start", "pending_id":"chain"}),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if get(&daemon, daemon.cookie(), &path)
                .await
                .to_string()
                .contains("Second reply")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let page = get(&daemon, daemon.cookie(), &path).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 3);
    assert!(!page.to_string().contains("Over the cap"));
    for cookie in [daemon.cookie(), &cookie] {
        let runs = get(&daemon, cookie, "/api/v1/runs").await;
        assert!(
            runs["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["hop_count"].as_u64().unwrap() <= 1)
        );
    }
    assert!(brain.requests().iter().any(|r| {
        r.messages
            .iter()
            .any(|m| m.text.contains("trust=colleague") && m.text.contains("First reply"))
    }));
}

#[tokio::test]
async fn a_private_message_does_not_join_a_reply_to_a_colleague() {
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use std::sync::Arc;
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_in_memory_with(TestDaemonOptions {
        brain: brain.clone(),
        ..Default::default()
    })
    .await;
    let (recipient, cookie) = person(&daemon, "Recipient").await;
    let agent = hire(&daemon, &cookie, "Helper").await;
    let channels = get(&daemon, &cookie, "/api/v1/channels").await;
    let dm = channels["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["agent_ids"].as_array().unwrap().contains(&agent["id"]))
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    brain.push_for(
        "Helper",
        Script::GateReply(
            vec!["Shared answer".into()],
            started.clone(),
            release.clone(),
        ),
    );
    brain.push_for("Helper", Script::reply(&["Private answer"]));
    let conversation = post(
        &daemon,
        daemon.cookie(),
        "/api/v1/conversations",
        json!({"workspace_ids":[recipient]}),
    )
    .await;
    let path = format!(
        "/api/v1/conversations/{}/messages",
        conversation["id"].as_str().unwrap()
    );
    post(
        &daemon,
        daemon.cookie(),
        &path,
        json!({"text":"Colleague question", "pending_id":"shared"}),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(10), started.notified())
        .await
        .unwrap();
    post(
        &daemon,
        &cookie,
        &format!("/api/v1/channels/{dm}/messages"),
        json!({"text":"Private question", "pending_id":"private-question"}),
    )
    .await;
    let private_answer = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let page = get(&daemon, &cookie, &format!("/api/v1/channels/{dm}/messages")).await;
            if page.to_string().contains("Private answer") {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    release.notify_one();
    assert!(
        private_answer.is_ok(),
        "The private message needs its own Run."
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let page = get(&daemon, daemon.cookie(), &path).await;
            if page.to_string().contains("Shared answer") {
                assert!(!page.to_string().contains("Private question"));
                assert!(!page.to_string().contains("Private answer"));
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn disclosure_private_memory_waits_for_approval_and_always_stores_a_rule() {
    use pagis_core::{AgentId, MemoryAccess, MemoryAuthor, MemoryChangeset, ScopedPath};
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use std::sync::Arc;
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_in_memory_with(TestDaemonOptions {
        brain: brain.clone(),
        ..Default::default()
    })
    .await;
    let (recipient, cookie) = person(&daemon, "Recipient").await;
    let agent = hire(&daemon, &cookie, "Helper").await;
    let agent_id = AgentId::from(agent["id"].as_str().unwrap().to_owned());
    let mut changes = MemoryChangeset::default();
    changes.writes.insert(
        ScopedPath::parse("shared/MEMORY.md").unwrap(),
        "Private index: secret-river-481\n".into(),
    );
    changes.writes.insert(
        ScopedPath::parse("shared/private.md").unwrap(),
        "Private fact: secret-river-481\n".into(),
    );
    daemon
        .state
        .memory
        .commit(
            &recipient,
            &agent_id,
            &MemoryAccess::Owner {
                agent_id: agent_id.clone(),
            },
            &MemoryAuthor {
                name: "Recipient".into(),
                email: "recipient@example.com".into(),
            },
            &changes,
            None,
            "Remember the private fact",
            None,
        )
        .await
        .unwrap();
    daemon
        .stores()
        .disclosure
        .set_profile(&recipient, "I design tools.")
        .await
        .unwrap();
    let conversation = post(
        &daemon,
        daemon.cookie(),
        "/api/v1/conversations",
        json!({"workspace_ids":[recipient]}),
    )
    .await;
    let path = format!(
        "/api/v1/conversations/{}/messages",
        conversation["id"].as_str().unwrap()
    );
    for (n, scope) in [(0, "deny"), (1, "always"), (2, "rule")] {
        brain.push_for(
            "Helper",
            Script::tool_call(&[], "memory_read", json!({"path":"shared/private.md"})),
        );
        brain.push_for(
            "Helper",
            Script::reply(&[if n == 0 {
                "I cannot share that."
            } else {
                "The approved answer."
            }]),
        );
        post(&daemon, daemon.cookie(), &path, json!({"text":"What private fact do you remember?", "pending_id":format!("disclosure-{n}")})).await;
        if scope != "rule" {
            let pending = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let requests = get(&daemon, &cookie, "/api/v1/requests").await;
                    if let Some(request) = requests["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|r| r["payload"]["disclosure_rules"].is_array())
                    {
                        break request.clone();
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "the private read waits for an Approval: {error:?}; model requests: {:?}",
                    brain.requests()
                )
            });
            if n == 0 {
                for request in brain.requests() {
                    assert!(
                        !format!("{request:?}").contains("secret-river-481"),
                        "private memory enters no model request without permission"
                    );
                }
                assert!(
                    brain
                        .requests()
                        .iter()
                        .any(|r| r.system.contains("I design tools."))
                );
            }
            assert!(
                !get(&daemon, daemon.cookie(), &path)
                    .await
                    .to_string()
                    .contains("secret-river-481")
            );
            // The asker cannot decide the recipient's Approval.
            assert_eq!(
                request(
                    &daemon,
                    daemon.cookie(),
                    reqwest::Method::POST,
                    &format!(
                        "/api/v1/requests/{}/decision",
                        pending["id"].as_str().unwrap()
                    ),
                    json!({"decision":"approved"})
                )
                .await
                .status(),
                404
            );
            post(
                &daemon,
                &cookie,
                &format!(
                    "/api/v1/requests/{}/decision",
                    pending["id"].as_str().unwrap()
                ),
                if n == 0 {
                    json!({"decision":"denied"})
                } else {
                    json!({"decision":"approved","scope":"always"})
                },
            )
            .await;
        }
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let runs = get(&daemon, &cookie, "/api/v1/runs").await;
                if runs["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|r| r["state"] == "completed")
                    .count()
                    > n
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the conversation Run completes");
        let rules = daemon.stores().disclosure.list(&recipient).await.unwrap();
        assert_eq!(rules.len(), usize::from(n > 0));
        if n > 0 {
            assert_eq!(
                rules[0].asker_workspace_id.as_ref(),
                Some(&daemon.workspace_id)
            );
            assert_eq!(rules[0].category, pagis_core::DisclosureCategory::Memory);
            assert!(brain.requests().iter().any(|r| format!("{:?}", r.messages).contains("Private fact: secret-river-481")));
        }
    }
    assert!(
        get(&daemon, &cookie, "/api/v1/requests").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn disclosure_free_busy_passes_without_calendar_details() {
    use pagis_core::{Author, CalendarEvent, CalendarEventData};
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use std::sync::Arc;
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_in_memory_with(TestDaemonOptions {
        brain: brain.clone(),
        ..Default::default()
    })
    .await;
    let (recipient, cookie) = person(&daemon, "Recipient").await;
    hire(&daemon, &cookie, "Helper").await;
    daemon
        .stores()
        .calendar
        .create(&CalendarEvent::new(
            recipient.clone(),
            Author::System,
            CalendarEventData {
                title: "Secret appointment".into(),
                description: "secret-clinic-529".into(),
                location: "Private office".into(),
                start: "2026-10-16T09:00:00".into(),
                end: "2026-10-16T10:00:00".into(),
                all_day: false,
                timezone: "UTC".into(),
                rrule: None,
                exdates: vec![],
                attendees: vec![],
                reminders: vec![],
                meeting_url: None,
            },
        ))
        .await
        .unwrap();
    brain.push_for(
        "Helper",
        Script::tool_call(
            &[],
            "calendar_free_busy",
            json!({"from":"2026-10-16T00:00:00Z", "to":"2026-10-17T00:00:00Z"}),
        ),
    );
    brain.push_for("Helper", Script::reply(&["Busy from 09:00 to 10:00 UTC."]));
    let conversation = post(
        &daemon,
        daemon.cookie(),
        "/api/v1/conversations",
        json!({"workspace_ids":[recipient]}),
    )
    .await;
    let path = format!(
        "/api/v1/conversations/{}/messages",
        conversation["id"].as_str().unwrap()
    );
    post(
        &daemon,
        daemon.cookie(),
        &path,
        json!({"text":"When are you busy?", "pending_id":"free-busy"}),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if get(&daemon, daemon.cookie(), &path)
                .await
                .to_string()
                .contains("Busy from 09:00")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for request in brain.requests() {
        assert!(!format!("{request:?}").contains("secret-clinic-529"));
        assert!(!format!("{request:?}").contains("Secret appointment"));
    }
    assert!(
        brain
            .requests()
            .iter()
            .any(|r| format!("{:?}", r.messages).contains("2026-10-16T09:00:00Z"))
    );
    assert!(
        get(&daemon, &cookie, "/api/v1/requests").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn disclosure_calendar_details_need_approval() {
    use pagis_core::{Author, CalendarEvent, CalendarEventData};
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use std::sync::Arc;
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_in_memory_with(TestDaemonOptions {
        brain: brain.clone(),
        ..Default::default()
    })
    .await;
    let (recipient, cookie) = person(&daemon, "Recipient").await;
    hire(&daemon, &cookie, "Helper").await;
    let event = CalendarEvent::new(
        recipient.clone(),
        Author::System,
        CalendarEventData {
            title: "Private appointment".into(),
            description: "secret-clinic-529".into(),
            location: "Private office".into(),
            start: "2026-10-16T09:00:00".into(),
            end: "2026-10-16T10:00:00".into(),
            all_day: false,
            timezone: "UTC".into(),
            rrule: None,
            exdates: vec![],
            attendees: vec![],
            reminders: vec![],
            meeting_url: None,
        },
    );
    daemon.stores().calendar.create(&event).await.unwrap();
    brain.push_for(
        "Helper",
        Script::tool_call(&[], "calendar_read", json!({"event_id":event.id})),
    );
    brain.push_for(
        "Helper",
        Script::reply(&["I cannot share the appointment."]),
    );
    let conversation = post(
        &daemon,
        daemon.cookie(),
        "/api/v1/conversations",
        json!({"workspace_ids":[recipient]}),
    )
    .await;
    let path = format!(
        "/api/v1/conversations/{}/messages",
        conversation["id"].as_str().unwrap()
    );
    post(
        &daemon,
        daemon.cookie(),
        &path,
        json!({"text":"Why are you busy?", "pending_id":"calendar-details"}),
    )
    .await;
    let pending = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let requests = get(&daemon, &cookie, "/api/v1/requests").await;
            if let Some(request) = requests["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["payload"]["disclosure_rules"][0]["category"] == "calendar_details")
            {
                break request.clone();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Event details wait for an Approval");
    for request in brain.requests() {
        assert!(!format!("{request:?}").contains("secret-clinic-529"));
    }
    post(
        &daemon,
        &cookie,
        &format!(
            "/api/v1/requests/{}/decision",
            pending["id"].as_str().unwrap()
        ),
        json!({"decision":"denied"}),
    )
    .await;
    assert!(
        !get(&daemon, daemon.cookie(), &path)
            .await
            .to_string()
            .contains("secret-clinic-529")
    );
}
