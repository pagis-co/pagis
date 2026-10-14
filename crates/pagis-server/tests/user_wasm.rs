//! The user answer over fake stores, with no listener.

#![cfg(not(feature = "native"))]

mod support {
    #[allow(dead_code)]
    pub mod agents;
}

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_core::{Org, StoreError, User};
use pagis_server::{OrgDto, OrgMenu, OrgMenuEntry};
use serde_json::{Value, json};
use support::agents::Embedder;
use tower::ServiceExt;

async fn user(embedder: &Embedder, cookie: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .uri("/api/v1/user")
        .extension(pagis_server::Peer("127.0.0.1:1234".parse().unwrap()));
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let response = embedder
        .routers
        .product
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[derive(Default)]
struct PersonOrgMenu {
    people: Mutex<Vec<User>>,
    revision: AtomicUsize,
}

#[async_trait]
impl OrgMenu for PersonOrgMenu {
    async fn org(&self, person: &User, org: Org) -> Result<OrgDto, StoreError> {
        self.people.lock().unwrap().push(person.clone());
        let revision = self.revision.load(Ordering::SeqCst);
        Ok(OrgDto {
            name: format!("{} revision {revision}", org.name),
            entries: vec![OrgMenuEntry {
                label: format!("{} revision {revision}", person.id),
                url: format!("https://org.example.com/{}", person.id),
                detail: Some(person.role.as_str().into()),
                current: true,
            }],
        })
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn the_embedder_resolves_the_org_name_and_entries_for_each_person_on_each_request() {
    let menu = Arc::new(PersonOrgMenu::default());
    let embedder = Embedder::with_org_menu(menu.clone()).await;
    let mut people = Vec::new();
    for (cookie, workspace_id) in [
        (&embedder.cookie, &embedder.workspace),
        (&embedder.other_cookie, &embedder.other_workspace),
    ] {
        let workspace = embedder
            .stores
            .workspaces
            .get(workspace_id)
            .await
            .unwrap()
            .unwrap();
        let person = embedder
            .stores
            .users
            .get(&workspace.user_id)
            .await
            .unwrap()
            .unwrap();
        let (status, body) = user(&embedder, Some(cookie)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["org"],
            json!({"name": "Org revision 0", "entries": [{
                "label": format!("{} revision 0", person.id),
                "url": format!("https://org.example.com/{}", person.id),
                "detail": person.role.as_str(),
                "current": true,
            }]})
        );
        people.push(person);
    }
    assert_ne!(people[0].id, people[1].id);
    assert_eq!(*menu.people.lock().unwrap(), people);

    menu.revision.store(1, Ordering::SeqCst);
    let (status, body) = user(&embedder, Some(&embedder.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["org"]["name"], "Org revision 1");
    assert_eq!(
        body["org"]["entries"][0]["label"],
        format!("{} revision 1", people[0].id)
    );
    people.push(people[0].clone());
    assert_eq!(*menu.people.lock().unwrap(), people);

    assert_eq!(user(&embedder, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(*menu.people.lock().unwrap(), people);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn the_daemon_shows_the_store_name_and_no_entries() {
    let embedder = Embedder::new().await;
    let (status, body) = user(&embedder, Some(&embedder.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["org"], json!({"name": "Org", "entries": []}));
    assert_eq!(body["notice"], Value::Null);
}

#[derive(Default)]
struct NoticeProvider {
    people: Mutex<Vec<User>>,
    revision: AtomicUsize,
}

#[async_trait]
impl pagis_server::PersonNotice for NoticeProvider {
    async fn notice(
        &self,
        person: &User,
    ) -> Result<Option<pagis_server::PersonNoticeDto>, StoreError> {
        self.people.lock().unwrap().push(person.clone());
        Ok(Some(pagis_server::PersonNoticeDto {
            tone: Some(pagis_server::NoticeTone::Waiting),
            label: "Balance".into(),
            text: pagis_server::NoticeText::try_from(format!(
                "{} revision {}",
                person.id,
                self.revision.load(Ordering::SeqCst)
            ))
            .unwrap(),
            link: Some(pagis_server::ConfiguredLink {
                label: "Add funds".into(),
                url: "https://org.example.com/balance".into(),
            }),
        }))
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn the_embedder_resolves_the_notice_for_each_person_on_each_request() {
    let provider = Arc::new(NoticeProvider::default());
    let embedder = Embedder::with_person_notice(provider.clone()).await;
    for (revision, cookie) in [&embedder.cookie, &embedder.other_cookie, &embedder.cookie]
        .into_iter()
        .enumerate()
    {
        provider.revision.store(revision, Ordering::SeqCst);
        let (status, body) = user(&embedder, Some(cookie)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["notice"],
            json!({
                "tone": "waiting", "label": "Balance",
                "text": format!("{} revision {revision}", body["id"].as_str().unwrap()),
                "link": { "label": "Add funds", "url": "https://org.example.com/balance" }
            })
        );
    }
    let people = provider.people.lock().unwrap().clone();
    assert_ne!(people[0].id, people[1].id);
    assert_eq!(people[0], people[2]);
    assert_eq!(user(&embedder, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(*provider.people.lock().unwrap(), people);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn the_user_answer_gives_only_an_administrator_the_offered_sections_and_links() {
    let sections = pagis_server::AdministrationSections::without(&[
        pagis_server::AdministrationSection::Providers,
    ])
    .with_links(vec![pagis_server::ConfiguredLink {
        label: "Help".into(),
        url: "https://team.example.com/help".into(),
    }]);
    let embedder = Embedder::with_administration_sections(sections).await;
    let (status, body) = user(&embedder, Some(&embedder.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["administration"]["sections"],
        json!([
            "spend",
            "people",
            "sessions",
            "resources",
            "plugins",
            "health"
        ])
    );
    assert_eq!(
        body["administration"]["links"],
        json!([{ "label": "Help", "url": "https://team.example.com/help" }])
    );
    let request = Request::builder()
        .uri("/api/v1/administration/sections")
        .header("cookie", &embedder.cookie)
        .extension(pagis_server::Peer("127.0.0.1:1234".parse().unwrap()))
        .body(Body::empty())
        .unwrap();
    let response = embedder
        .routers
        .administration
        .clone()
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let offered: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(body["administration"]["sections"], offered["sections"]);
    assert_eq!(body["administration"]["links"], offered["links"]);

    let workspace = embedder
        .stores
        .workspaces
        .get(&embedder.workspace)
        .await
        .unwrap()
        .unwrap();
    embedder
        .stores
        .users
        .set_role(
            &workspace.user_id,
            pagis_core::UserRole::Member,
            pagis_core::now_ms(),
        )
        .await
        .unwrap();
    let (status, body) = user(&embedder, Some(&embedder.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["administration"], Value::Null);
}
