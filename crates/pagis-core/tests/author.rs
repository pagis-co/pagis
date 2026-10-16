use pagis_core::{AgentId, Author, UserId};
use serde_json::json;

#[test]
fn authors_use_one_tagged_shape_with_the_id_of_their_kind() {
    for (author, value) in [
        (
            Author::Person(UserId::from("person".to_string())),
            json!({"kind": "person", "person_id": "person"}),
        ),
        (
            Author::Agent(AgentId::from("agent".to_string())),
            json!({"kind": "agent", "agent_id": "agent"}),
        ),
        (Author::System, json!({"kind": "system"})),
    ] {
        assert_eq!(serde_json::to_value(&author).unwrap(), value);
        assert_eq!(serde_json::from_value::<Author>(value).unwrap(), author);
    }
}

#[test]
fn an_author_cannot_have_another_kinds_id_or_no_identity() {
    for value in [
        json!({"kind": "person"}),
        json!({"kind": "agent"}),
        json!({"kind": "person", "person_id": "person", "agent_id": "agent"}),
        json!({"kind": "system", "person_id": "person"}),
        json!({"kind": "user"}),
    ] {
        assert!(
            serde_json::from_value::<Author>(value.clone()).is_err(),
            "accepted an invalid author: {value}"
        );
    }
}
