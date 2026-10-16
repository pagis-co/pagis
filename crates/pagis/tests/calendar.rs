//! Calendar CRUD and recurrence through the full daemon.
use pagis_testkit::TestDaemon;
use serde_json::{Value, json};

#[tokio::test]
async fn calendar_crud_and_occurrences() {
    let daemon = TestDaemon::start().await;
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/calendar/events", daemon.base_url);
    let mut body = json!({
        "title": "Review", "description": "Plan", "location": "Office",
        "start": "2026-03-07T09:00:00", "end": "2026-03-07T10:00:00",
        "all_day": false, "timezone": "America/New_York",
        "rrule": "FREQ=DAILY;COUNT=3", "exdates": [],
        "attendees": [],
        "reminders": [15], "meeting_url": "https://meet.example.com/review"
    });
    let response = client
        .post(&url)
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let created: Value = response.json().await.unwrap();
    assert_eq!(created["created_by"]["kind"], "person");
    let item = format!("{url}/{}", created["id"].as_str().unwrap());
    let read: Value = client
        .get(&item)
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read, created);
    body["title"] = json!("Changed");
    assert_eq!(
        client
            .put(&item)
            .header("cookie", daemon.cookie())
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let found: Vec<Value> = client
        .get(&url)
        .header("cookie", daemon.cookie())
        .query(&[("from", "1772841600000"), ("to", "1773187200000")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(found.len(), 3);
    assert_eq!(found[1]["start"], "2026-03-08T09:00:00-04:00");
    assert_eq!(found[1]["event"]["data"]["reminders"], json!([15]));
    body["meeting_url"] = json!("javascript:alert(1)");
    assert_eq!(
        client
            .post(&url)
            .header("cookie", daemon.cookie())
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    assert_eq!(
        client
            .delete(&item)
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        client
            .get(&item)
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
}
