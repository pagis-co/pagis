//! Validation of the values a `form`, a `choice` or a `widget`
//! Request submits.
//!
//! The schema comes from the Request row, never from the block. A
//! client can post back a mutated block, so the block's denormalized
//! copy of the fields is not evidence. A failure returns a message the
//! API turns into a 422; the Run stays parked.

use serde_json::{Map, Value};

use crate::block::{ChoiceOption, FormField, FormFieldKind};

/// The largest JSON value one Widget answer carries.
pub const MAX_WIDGET_VALUE_BYTES: usize = 64 * 1024;
/// The longest text one Widget answer carries.
pub const MAX_WIDGET_TEXT: usize = 4096;

/// The values a decision carries, validated against the schema on the
/// Request payload. `form` validates every field; `choice` validates
/// the single `value` against the option list; `widget` takes the
/// text the view wrote and an optional JSON value beside it.
pub fn validate_values(
    kind: &str,
    payload: &Value,
    values: Option<&Value>,
) -> Result<Value, String> {
    let values = values.ok_or_else(|| format!("a {kind} decision needs values"))?;
    let values = values
        .as_object()
        .ok_or_else(|| "values must be an object".to_string())?;
    match kind {
        crate::Request::FORM_KIND => validate_form(&form_fields(payload)?, values),
        crate::Request::CHOICE_KIND => validate_choice(&choice_options(payload)?, values),
        crate::Request::WIDGET_KIND => validate_widget(values),
        other => Err(format!("kind {other} takes no values")),
    }
}

/// One Widget answer: the text the view wrote, and an optional JSON
/// value beside it (ADR-0016). The daemon holds no schema for the
/// value, so it checks the size alone and labels it untrusted where
/// it reaches a model.
fn validate_widget(values: &Map<String, Value>) -> Result<Value, String> {
    for key in values.keys() {
        if key != "text" && key != "value" {
            return Err(format!("unknown field: {key}"));
        }
    }
    let text = values
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| "field text is required".to_string())?;
    if text.trim().is_empty() {
        return Err("field text must not be empty".to_string());
    }
    if text.chars().count() > MAX_WIDGET_TEXT {
        return Err(format!(
            "field text is longer than {MAX_WIDGET_TEXT} characters"
        ));
    }
    let value = values.get("value").cloned().unwrap_or(Value::Null);
    let bytes = serde_json::to_vec(&value).map_or(usize::MAX, |json| json.len());
    if bytes > MAX_WIDGET_VALUE_BYTES {
        return Err(format!(
            "field value is {bytes} bytes; the limit is {MAX_WIDGET_VALUE_BYTES}"
        ));
    }
    Ok(serde_json::json!({ "text": text, "value": value }))
}

fn form_fields(payload: &Value) -> Result<Vec<FormField>, String> {
    serde_json::from_value(payload["fields"].clone())
        .map_err(|e| format!("the request has no readable field schema: {e}"))
}

fn choice_options(payload: &Value) -> Result<Vec<ChoiceOption>, String> {
    serde_json::from_value(payload["options"].clone())
        .map_err(|e| format!("the request has no readable option list: {e}"))
}

/// Every submitted key names a field, every required field has a
/// value, and every value fits its field kind. Unknown keys are a
/// failure, not a silent drop: a client that sends one did not render
/// this schema.
fn validate_form(fields: &[FormField], values: &Map<String, Value>) -> Result<Value, String> {
    for key in values.keys() {
        if !fields.iter().any(|field| &field.key == key) {
            return Err(format!("unknown field: {key}"));
        }
    }
    let mut out = Map::new();
    for field in fields {
        match values.get(&field.key) {
            None | Some(Value::Null) => {
                if field.required {
                    return Err(format!("field {} is required", field.key));
                }
            }
            Some(value) => {
                check_field(field, value)?;
                out.insert(field.key.clone(), value.clone());
            }
        }
    }
    Ok(Value::Object(out))
}

fn check_field(field: &FormField, value: &Value) -> Result<(), String> {
    let key = &field.key;
    let fits = match field.kind {
        FormFieldKind::Text | FormFieldKind::Date => value.is_string(),
        FormFieldKind::Number => value.is_number(),
        FormFieldKind::Checkbox => value.is_boolean(),
        FormFieldKind::Select => value
            .as_str()
            .is_some_and(|text| field.options.iter().any(|option| option.value == text)),
    };
    if fits {
        return Ok(());
    }
    Err(match field.kind {
        FormFieldKind::Select => format!("field {key} is not one of its options"),
        FormFieldKind::Text => format!("field {key} must be a string"),
        FormFieldKind::Date => format!("field {key} must be a date string"),
        FormFieldKind::Number => format!("field {key} must be a number"),
        FormFieldKind::Checkbox => format!("field {key} must be a boolean"),
    })
}

/// One tap: `{"value": "<an option value>"}` and nothing else.
fn validate_choice(options: &[ChoiceOption], values: &Map<String, Value>) -> Result<Value, String> {
    for key in values.keys() {
        if key != "value" {
            return Err(format!("unknown field: {key}"));
        }
    }
    let chosen = values
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| "a choice decision needs a string value".to_string())?;
    if !options.iter().any(|option| option.value == chosen) {
        return Err(format!("{chosen} is not one of the options"));
    }
    Ok(serde_json::json!({ "value": chosen }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Request;

    fn form_payload() -> Value {
        serde_json::json!({
            "title": "Book the room",
            "fields": [
                {"key": "who", "label": "Who", "kind": "text", "required": true},
                {"key": "seats", "label": "Seats", "kind": "number"},
                {"key": "room", "label": "Room", "kind": "select", "options": [
                    {"value": "a", "label": "Room A"},
                    {"value": "b", "label": "Room B"}
                ]},
                {"key": "catering", "label": "Catering", "kind": "checkbox"}
            ]
        })
    }

    fn choice_payload() -> Value {
        serde_json::json!({
            "title": "Which one?",
            "options": [{"value": "yes", "label": "Yes"}, {"value": "no", "label": "No"}]
        })
    }

    fn validate(payload: &Value, kind: &str, values: Value) -> Result<Value, String> {
        validate_values(kind, payload, Some(&values))
    }

    #[test]
    fn a_full_form_submission_round_trips() {
        let values = serde_json::json!({
            "who": "Ada", "seats": 4, "room": "b", "catering": true
        });
        let out = validate(&form_payload(), Request::FORM_KIND, values.clone()).expect("valid");
        assert_eq!(out, values);
    }

    #[test]
    fn optional_fields_may_be_absent() {
        let out = validate(
            &form_payload(),
            Request::FORM_KIND,
            serde_json::json!({"who": "Ada"}),
        )
        .expect("valid");
        assert_eq!(out, serde_json::json!({"who": "Ada"}));
    }

    #[test]
    fn a_missing_required_field_fails() {
        let err = validate(
            &form_payload(),
            Request::FORM_KIND,
            serde_json::json!({"seats": 2}),
        )
        .expect_err("required");
        assert!(err.contains("who"), "{err}");
    }

    #[test]
    fn a_wrong_type_fails() {
        let err = validate(
            &form_payload(),
            Request::FORM_KIND,
            serde_json::json!({"who": "Ada", "seats": "four"}),
        )
        .expect_err("kind");
        assert!(err.contains("seats"), "{err}");
    }

    #[test]
    fn a_select_outside_its_options_fails() {
        let err = validate(
            &form_payload(),
            Request::FORM_KIND,
            serde_json::json!({"who": "Ada", "room": "z"}),
        )
        .expect_err("option");
        assert!(err.contains("room"), "{err}");
    }

    #[test]
    fn an_unknown_field_fails() {
        let err = validate(
            &form_payload(),
            Request::FORM_KIND,
            serde_json::json!({"who": "Ada", "admin": true}),
        )
        .expect_err("unknown");
        assert!(err.contains("admin"), "{err}");
    }

    #[test]
    fn a_choice_takes_one_option_value() {
        let out = validate(
            &choice_payload(),
            Request::CHOICE_KIND,
            serde_json::json!({"value": "no"}),
        )
        .expect("valid");
        assert_eq!(out, serde_json::json!({"value": "no"}));
    }

    #[test]
    fn a_choice_outside_its_options_fails() {
        let err = validate(
            &choice_payload(),
            Request::CHOICE_KIND,
            serde_json::json!({"value": "maybe"}),
        )
        .expect_err("option");
        assert!(err.contains("maybe"), "{err}");
    }

    #[test]
    fn values_are_required_and_must_be_an_object() {
        assert!(validate_values(Request::FORM_KIND, &form_payload(), None).is_err());
        assert!(
            validate(
                &form_payload(),
                Request::FORM_KIND,
                serde_json::json!("Ada")
            )
            .is_err()
        );
    }

    // A Widget answers once with text and an optional JSON value
    // (ADR-0016).

    #[test]
    fn a_widget_answers_with_text_and_a_value() {
        let out = validate(
            &serde_json::json!({}),
            Request::WIDGET_KIND,
            serde_json::json!({"text": "the second row", "value": {"row": 2}}),
        )
        .expect("valid");
        assert_eq!(
            out,
            serde_json::json!({"text": "the second row", "value": {"row": 2}})
        );
    }

    #[test]
    fn a_widget_answer_without_a_value_reads_as_null() {
        let out = validate(
            &serde_json::json!({}),
            Request::WIDGET_KIND,
            serde_json::json!({"text": "done"}),
        )
        .expect("valid");
        assert_eq!(out, serde_json::json!({"text": "done", "value": null}));
    }

    #[test]
    fn a_widget_answer_needs_text() {
        for bad in [serde_json::json!({}), serde_json::json!({"text": "  "})] {
            assert!(
                validate(&serde_json::json!({}), Request::WIDGET_KIND, bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_widget_answer_takes_no_other_field() {
        let err = validate(
            &serde_json::json!({}),
            Request::WIDGET_KIND,
            serde_json::json!({"text": "hi", "role": "user"}),
        )
        .expect_err("unknown field");
        assert!(err.contains("role"), "{err}");
    }

    #[test]
    fn a_widget_answer_over_the_caps_fails() {
        let long = "x".repeat(super::MAX_WIDGET_TEXT + 1);
        assert!(
            validate(
                &serde_json::json!({}),
                Request::WIDGET_KIND,
                serde_json::json!({"text": long}),
            )
            .is_err()
        );
        let big = "x".repeat(super::MAX_WIDGET_VALUE_BYTES);
        assert!(
            validate(
                &serde_json::json!({}),
                Request::WIDGET_KIND,
                serde_json::json!({"text": "hi", "value": {"note": big}}),
            )
            .is_err()
        );
    }

    #[test]
    fn a_tool_action_takes_no_values() {
        assert!(
            validate(
                &serde_json::json!({}),
                Request::TOOL_ACTION_KIND,
                serde_json::json!({})
            )
            .is_err()
        );
    }
}
