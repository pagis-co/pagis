//! The check of the values that answer a question of a harness (ACP
//! `elicitation/create`, form mode) against the `requestedSchema` of the
//! question.
//!
//! ACP lets the harness check the values again, so Pagis checks what the
//! schema names: each `required` property is there, no property is
//! unknown, each value has the type of its property, a value of an enum
//! is one of its options, a string keeps its length limits, a number keeps
//! its range, and a multi-select keeps its item limits and its options.

use agent_client_protocol::schema::v1 as acp;
use serde_json::{Map, Value};

/// The form of one question, which Pagis can check a value against.
#[derive(Debug, Clone)]
pub(crate) struct Form {
    schema: acp::ElicitationSchema,
}

/// Why a value does not match the form. The property name is harness
/// text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("\"{property}\" {problem}")]
pub(crate) struct ValueError {
    pub(crate) property: String,
    pub(crate) problem: String,
}

impl Form {
    /// The form of `schema`, the `requestedSchema` as JSON. It is `None`
    /// when Pagis cannot check a value of some property: a property of a
    /// type that ACP does not name, or a multi-select of such items.
    pub(crate) fn parse(schema: &Value) -> Option<Self> {
        let schema: acp::ElicitationSchema = serde_json::from_value(schema.clone()).ok()?;
        let checkable = schema.properties.values().all(|property| match property {
            acp::ElicitationPropertySchema::String(_)
            | acp::ElicitationPropertySchema::Number(_)
            | acp::ElicitationPropertySchema::Integer(_)
            | acp::ElicitationPropertySchema::Boolean(_) => true,
            acp::ElicitationPropertySchema::Array(multi) => multi_options(multi).is_some(),
            _ => false,
        });
        checkable.then_some(Self { schema })
    }

    /// Checks `values` against the form: first that it knows each
    /// property, then that each required property is there, then each
    /// value.
    pub(crate) fn check(&self, values: &Map<String, Value>) -> Result<(), ValueError> {
        let properties = &self.schema.properties;
        if let Some(unknown) = values.keys().find(|name| !properties.contains_key(*name)) {
            return Err(refused(unknown, "is not in the form"));
        }
        let required = self.schema.required.iter().flatten();
        if let Some(missing) = required
            .into_iter()
            .find(|name| !values.contains_key(*name))
        {
            return Err(refused(missing, "is required"));
        }
        for (name, value) in values {
            check_value(&properties[name], value).map_err(|problem| refused(name, problem))?;
        }
        Ok(())
    }
}

fn refused(property: &str, problem: impl Into<String>) -> ValueError {
    ValueError {
        property: property.to_string(),
        problem: problem.into(),
    }
}

/// Checks one value against its property, and answers the problem.
fn check_value(property: &acp::ElicitationPropertySchema, value: &Value) -> Result<(), String> {
    match property {
        acp::ElicitationPropertySchema::String(schema) => check_string(schema, value),
        acp::ElicitationPropertySchema::Number(schema) => {
            let number = value.as_f64().ok_or("must be a number")?;
            check_range(number, schema.minimum, schema.maximum)
        }
        acp::ElicitationPropertySchema::Integer(schema) => {
            let integer = value.as_i64().ok_or("must be a whole number")?;
            check_range(integer, schema.minimum, schema.maximum)
        }
        acp::ElicitationPropertySchema::Boolean(_) => match value {
            Value::Bool(_) => Ok(()),
            _ => Err("must be true or false".to_string()),
        },
        acp::ElicitationPropertySchema::Array(schema) => check_multi_select(schema, value),
        // `Form::parse` keeps no form with another property.
        _ => Err("has a type that Pagis cannot check".to_string()),
    }
}

fn check_string(schema: &acp::StringPropertySchema, value: &Value) -> Result<(), String> {
    let text = value.as_str().ok_or("must be text")?;
    let options: Option<Vec<&str>> = match (&schema.enum_values, &schema.one_of) {
        (Some(values), _) => Some(values.iter().map(String::as_str).collect()),
        (None, Some(options)) => Some(options.iter().map(|option| option.value.as_str()).collect()),
        (None, None) => None,
    };
    if let Some(options) = options
        && !options.contains(&text)
    {
        return Err(format!("must be one of: {}", options.join(", ")));
    }
    // JSON Schema counts a length in characters.
    let length = text.chars().count();
    if let Some(min) = schema.min_length
        && length < min as usize
    {
        return Err(format!("must have at least {min} characters"));
    }
    if let Some(max) = schema.max_length
        && length > max as usize
    {
        return Err(format!("must have at most {max} characters"));
    }
    Ok(())
}

fn check_range<T: PartialOrd + std::fmt::Display>(
    value: T,
    minimum: Option<T>,
    maximum: Option<T>,
) -> Result<(), String> {
    if let Some(minimum) = minimum
        && value < minimum
    {
        return Err(format!("must be at least {minimum}"));
    }
    if let Some(maximum) = maximum
        && value > maximum
    {
        return Err(format!("must be at most {maximum}"));
    }
    Ok(())
}

fn check_multi_select(
    schema: &acp::MultiSelectPropertySchema,
    value: &Value,
) -> Result<(), String> {
    let chosen: Vec<&str> = value
        .as_array()
        .and_then(|items| items.iter().map(Value::as_str).collect())
        .ok_or("must be a list of options")?;
    let options = multi_options(schema).unwrap_or_default();
    if chosen.iter().any(|item| !options.contains(item)) {
        return Err(format!(
            "must hold only these options: {}",
            options.join(", ")
        ));
    }
    let count = chosen.len() as u64;
    if let Some(min) = schema.min_items
        && count < min
    {
        return Err(format!("must hold at least {min} of the options"));
    }
    if let Some(max) = schema.max_items
        && count > max
    {
        return Err(format!("must hold at most {max} of the options"));
    }
    Ok(())
}

/// The options of a multi-select, or `None` for items of a type that ACP
/// does not name.
fn multi_options(schema: &acp::MultiSelectPropertySchema) -> Option<Vec<&str>> {
    match &schema.items {
        acp::MultiSelectItems::String(items) => {
            Some(items.values.iter().map(String::as_str).collect())
        }
        acp::MultiSelectItems::Titled(items) => Some(
            items
                .options
                .iter()
                .map(|option| option.value.as_str())
                .collect(),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn form(schema: acp::ElicitationSchema) -> Form {
        Form::parse(&serde_json::to_value(schema).unwrap()).expect("Pagis checks the form")
    }

    fn values(values: Value) -> Map<String, Value> {
        let Value::Object(values) = values else {
            panic!("the values are a JSON object");
        };
        values
    }

    /// The property and the problem of a check that fails.
    fn refused(form: &Form, given: Value) -> (String, String) {
        let error = form
            .check(&values(given))
            .expect_err("the values do not match the form");
        (error.property, error.problem)
    }

    #[test]
    fn values_that_match_each_property_type_pass() {
        let form = form(
            acp::ElicitationSchema::new()
                .string("branch", true)
                .number("ratio", 0.0, 1.0, true)
                .integer("retries", 0, 5, true)
                .boolean("push", true)
                .property(
                    "targets",
                    acp::MultiSelectPropertySchema::new(vec![
                        "linux".to_string(),
                        "macos".to_string(),
                    ]),
                    true,
                ),
        );

        assert_eq!(
            form.check(&values(json!({
                "branch": "main",
                "ratio": 0.5,
                "retries": 3,
                "push": true,
                "targets": ["macos"],
            }))),
            Ok(())
        );
    }

    #[test]
    fn a_missing_required_property_names_the_property() {
        let form = form(
            acp::ElicitationSchema::new()
                .string("branch", true)
                .boolean("push", false),
        );

        assert_eq!(form.check(&values(json!({"branch": "main"}))), Ok(()));
        let (property, problem) = refused(&form, json!({"push": true}));
        assert_eq!(property, "branch");
        assert_eq!(problem, "is required");
    }

    #[test]
    fn an_unknown_property_is_refused() {
        let form = form(acp::ElicitationSchema::new().string("branch", true));

        let (property, problem) = refused(&form, json!({"branch": "main", "force": true}));
        assert_eq!(property, "force");
        assert_eq!(problem, "is not in the form");
    }

    #[test]
    fn a_value_of_another_type_is_refused_for_each_property_type() {
        let form = form(
            acp::ElicitationSchema::new()
                .string("branch", false)
                .number("ratio", 0.0, 1.0, false)
                .integer("retries", 0, 5, false)
                .boolean("push", false)
                .property(
                    "targets",
                    acp::MultiSelectPropertySchema::new(vec!["linux".to_string()]),
                    false,
                ),
        );

        for (given, problem) in [
            (json!({"branch": 7}), "must be text"),
            (json!({"ratio": "half"}), "must be a number"),
            (json!({"retries": 1.5}), "must be a whole number"),
            (json!({"retries": "2"}), "must be a whole number"),
            (json!({"push": "yes"}), "must be true or false"),
            (json!({"targets": "linux"}), "must be a list of options"),
            (json!({"targets": [1]}), "must be a list of options"),
            (json!({"branch": null}), "must be text"),
        ] {
            assert_eq!(refused(&form, given.clone()).1, problem, "{given}");
        }
    }

    #[test]
    fn a_value_of_an_enum_is_one_of_its_options() {
        let form = form(
            acp::ElicitationSchema::new()
                .property(
                    "branch",
                    acp::StringPropertySchema::new()
                        .enum_values(vec!["main".to_string(), "dev".to_string()]),
                    false,
                )
                .property(
                    "level",
                    acp::StringPropertySchema::new().one_of(vec![
                        acp::EnumOption::new("low", "Low"),
                        acp::EnumOption::new("high", "High"),
                    ]),
                    false,
                ),
        );

        assert_eq!(
            form.check(&values(json!({"branch": "dev", "level": "high"}))),
            Ok(())
        );
        assert_eq!(
            refused(&form, json!({"branch": "release"})),
            (
                "branch".to_string(),
                "must be one of: main, dev".to_string()
            )
        );
        assert_eq!(
            refused(&form, json!({"level": "High"})),
            ("level".to_string(), "must be one of: low, high".to_string())
        );
    }

    #[test]
    fn a_string_keeps_its_length_limits_in_characters() {
        let form = form(acp::ElicitationSchema::new().property(
            "code",
            acp::StringPropertySchema::new().min_length(2).max_length(3),
            true,
        ));

        assert_eq!(form.check(&values(json!({"code": "ÄÖÜ"}))), Ok(()));
        assert_eq!(
            refused(&form, json!({"code": "a"})).1,
            "must have at least 2 characters"
        );
        assert_eq!(
            refused(&form, json!({"code": "abcd"})).1,
            "must have at most 3 characters"
        );
    }

    #[test]
    fn a_number_and_an_integer_keep_their_range() {
        let form = form(
            acp::ElicitationSchema::new()
                .number("ratio", 0.5, 1.5, false)
                .integer("retries", 1, 3, false),
        );

        assert_eq!(
            form.check(&values(json!({"ratio": 1.5, "retries": 1}))),
            Ok(())
        );
        assert_eq!(
            refused(&form, json!({"ratio": 0.25})).1,
            "must be at least 0.5"
        );
        assert_eq!(refused(&form, json!({"ratio": 2})).1, "must be at most 1.5");
        assert_eq!(
            refused(&form, json!({"retries": 0})).1,
            "must be at least 1"
        );
        assert_eq!(refused(&form, json!({"retries": 4})).1, "must be at most 3");
    }

    #[test]
    fn a_multi_select_keeps_its_item_limits_and_its_options() {
        let form = form(
            acp::ElicitationSchema::new()
                .property(
                    "targets",
                    acp::MultiSelectPropertySchema::new(vec![
                        "linux".to_string(),
                        "macos".to_string(),
                        "windows".to_string(),
                    ])
                    .min_items(1)
                    .max_items(2),
                    false,
                )
                .property(
                    "checks",
                    acp::MultiSelectPropertySchema::titled(vec![
                        acp::EnumOption::new("lint", "Lint"),
                        acp::EnumOption::new("test", "Test"),
                    ]),
                    false,
                ),
        );

        assert_eq!(
            form.check(&values(
                json!({"targets": ["linux", "macos"], "checks": ["test"]})
            )),
            Ok(())
        );
        assert_eq!(
            refused(&form, json!({"targets": []})).1,
            "must hold at least 1 of the options"
        );
        assert_eq!(
            refused(&form, json!({"targets": ["linux", "macos", "windows"]})).1,
            "must hold at most 2 of the options"
        );
        assert_eq!(
            refused(&form, json!({"targets": ["bsd"]})).1,
            "must hold only these options: linux, macos, windows"
        );
        assert_eq!(
            refused(&form, json!({"checks": ["Lint"]})).1,
            "must hold only these options: lint, test"
        );
    }

    #[test]
    fn a_property_of_a_type_that_acp_does_not_name_cannot_be_checked() {
        let schema = json!({
            "type": "object",
            "properties": {
                "branch": {"type": "string"},
                "config": {"type": "object"},
            },
        });
        assert!(Form::parse(&schema).is_none());

        let items = json!({
            "type": "object",
            "properties": {
                "sizes": {"type": "array", "items": {"type": "number"}},
            },
        });
        assert!(Form::parse(&items).is_none());
    }
}
