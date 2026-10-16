use pagis_broker::{CallOrigin, ToolCall};
use serde_json::json;

#[test]
fn tool_call_round_trips_json() {
    for origin in [
        CallOrigin::Model,
        CallOrigin::Widget {
            package: "calendar".into(),
        },
    ] {
        let mut call = ToolCall::new("calendar__read", r#"{"limit":3}"#);
        call.loaded.load("calendar");
        call.origin = origin.clone();
        call.tool_call_id = Some("call_1".into());
        let value = serde_json::to_value(&call).unwrap();
        assert_eq!(
            value,
            json!({
                "name": "calendar__read",
                "arguments": r#"{"limit":3}"#,
                "loaded": {"packages": ["calendar"]},
                "origin": serde_json::to_value(origin).unwrap(),
                "tool_call_id": "call_1",
            })
        );
        assert_eq!(serde_json::from_value::<ToolCall>(value).unwrap(), call);
    }
}
