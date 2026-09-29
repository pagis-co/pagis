//! The Reflection Filter (ADR-0011).
//!
//! One sync resource has one ordered rule list. A rule has a verdict and
//! one or more conditions. All conditions of a rule must hold. The first
//! rule that holds gives the verdict, and a default verdict closes the
//! list. The same filter applies to live and historical arrivals.
//!
//! The filter reads Page Signals only: cheap facts that the resource
//! computes from stored metadata. No signal comes from a model.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a signal or a condition value holds.
///
/// `Duration` and `List` are value kinds of a condition, not signal
/// kinds: a duration applies to a DateTime signal, and a list applies to
/// a text signal. A `List` signal is a text signal that also tells the
/// editor which form its values have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValueKind {
    Text,
    DateTime,
    Duration,
    Number,
    Boolean,
    Choice { options: Vec<String> },
    TagSet { options: Vec<String> },
    List { format: ListFormat },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListFormat {
    Email,
    Domain,
    Plain,
}

impl ListFormat {
    /// Whether one list entry has the form this hint names.
    pub fn accepts(self, value: &str) -> bool {
        let value = value.trim();
        if value.is_empty() {
            return false;
        }
        match self {
            ListFormat::Email => crate::mail_address(value).is_some(),
            ListFormat::Domain => crate::mail_domain(value).is_some(),
            ListFormat::Plain => true,
        }
    }
}

impl ValueKind {
    /// The operators this kind allows, in editor order.
    pub fn operators(&self) -> &'static [Operator] {
        use Operator::*;
        match self {
            ValueKind::Text | ValueKind::List { .. } => {
                &[Contains, NotContains, Is, StartsWith, In, NotIn]
            }
            ValueKind::DateTime => &[After, Before, WithinLast, OlderThan],
            ValueKind::Duration => &[WithinLast, OlderThan],
            ValueKind::Number => &[AtLeast, AtMost, Is],
            ValueKind::Boolean => &[Is],
            ValueKind::Choice { .. } => &[Is, IsNot, In],
            ValueKind::TagSet { .. } => &[Has, Lacks],
        }
    }

    /// The options a choice or a tag set offers, else nothing.
    pub fn options(&self) -> Option<&[String]> {
        match self {
            ValueKind::Choice { options } | ValueKind::TagSet { options } => Some(options),
            _ => None,
        }
    }

    fn list_format(&self) -> ListFormat {
        match self {
            ValueKind::List { format } => *format,
            _ => ListFormat::Plain,
        }
    }
}

/// One fact the filter editor offers for one resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Signal {
    pub id: String,
    pub label: String,
    pub kind: ValueKind,
}

/// What one connection's resource offers the filter editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Catalogue {
    pub signals: Vec<Signal>,
}

impl Catalogue {
    pub fn signal(&self, id: &str) -> Option<&Signal> {
        self.signals.iter().find(|signal| signal.id == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Contains,
    NotContains,
    Is,
    IsNot,
    StartsWith,
    After,
    Before,
    WithinLast,
    OlderThan,
    AtLeast,
    AtMost,
    In,
    NotIn,
    Has,
    Lacks,
}

impl Operator {
    pub fn as_str(self) -> &'static str {
        match self {
            Operator::Contains => "contains",
            Operator::NotContains => "does not contain",
            Operator::Is => "is",
            Operator::IsNot => "is not",
            Operator::StartsWith => "starts with",
            Operator::After => "is after",
            Operator::Before => "is before",
            Operator::WithinLast => "is within the last",
            Operator::OlderThan => "is older than",
            Operator::AtLeast => "is at least",
            Operator::AtMost => "is at most",
            Operator::In => "is in",
            Operator::NotIn => "is not in",
            Operator::Has => "has",
            Operator::Lacks => "lacks",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DurationUnit {
    Days,
    Hours,
}

impl DurationUnit {
    fn millis(self) -> i64 {
        match self {
            DurationUnit::Days => 86_400_000,
            DurationUnit::Hours => 3_600_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConditionValue {
    Text {
        value: String,
    },
    /// Unix milliseconds.
    DateTime {
        at: i64,
    },
    Duration {
        amount: u32,
        unit: DurationUnit,
    },
    Number {
        value: i64,
    },
    Boolean {
        value: bool,
    },
    Choice {
        value: String,
    },
    Choices {
        values: Vec<String>,
    },
    List {
        values: Vec<String>,
    },
}

impl ConditionValue {
    fn describe(&self) -> String {
        match self {
            ConditionValue::Text { value } | ConditionValue::Choice { value } => value.clone(),
            ConditionValue::DateTime { at } => at.to_string(),
            ConditionValue::Duration { amount, unit } => match unit {
                DurationUnit::Days => format!("{amount} days"),
                DurationUnit::Hours => format!("{amount} hours"),
            },
            ConditionValue::Number { value } => value.to_string(),
            ConditionValue::Boolean { value } => value.to_string(),
            ConditionValue::Choices { values } | ConditionValue::List { values } => {
                values.join(", ")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub signal: String,
    pub operator: Operator,
    pub value: ConditionValue,
}

impl Condition {
    fn describe(&self) -> String {
        format!(
            "{} {} {}",
            self.signal,
            self.operator.as_str(),
            self.value.describe()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Reflect,
    Skip,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Reflect => "reflect",
            Verdict::Skip => "skip",
        }
    }

    pub fn reflects(self) -> bool {
        self == Verdict::Reflect
    }
}

/// One rule: a verdict and the conditions that must all hold for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub verdict: Verdict,
    pub conditions: Vec<Condition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReflectionFilter {
    pub rules: Vec<Rule>,
    pub default: Verdict,
}

/// Why one filter is not valid for one catalogue. The rule and the
/// condition are zero-based indexes into the filter the user sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FilterError {
    pub rule: usize,
    pub condition: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.condition {
            Some(condition) => write!(
                f,
                "Rule {}, condition {}: {}",
                self.rule + 1,
                condition + 1,
                self.message
            ),
            None => write!(f, "Rule {}: {}", self.rule + 1, self.message),
        }
    }
}

impl std::error::Error for FilterError {}

/// One Subject Page's signal values.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(transparent)]
pub struct PageSignals(pub BTreeMap<String, SignalValue>);

impl PageSignals {
    pub fn get(&self, id: &str) -> Option<&SignalValue> {
        self.0.get(id)
    }

    pub fn insert(&mut self, id: impl Into<String>, value: SignalValue) {
        self.0.insert(id.into(), value);
    }
}

impl FromIterator<(String, SignalValue)> for PageSignals {
    fn from_iter<T: IntoIterator<Item = (String, SignalValue)>>(items: T) -> Self {
        Self(items.into_iter().collect())
    }
}

/// One signal's value. A text signal holds every message's value for the
/// page, so a text condition holds when any one of them matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignalValue {
    Text { values: Vec<String> },
    DateTime { at: i64 },
    Number { value: i64 },
    Boolean { value: bool },
    Choice { value: String },
    Tags { values: Vec<String> },
}

/// The verdict for one page, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub verdict: Verdict,
    /// The rule that gave the verdict, else the default closed the list.
    pub rule: Option<usize>,
    pub reason: String,
}

/// What one filter decides about the stored pages of a resource, for
/// the filter editor. The counts come from Page Signals alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FilterPreview {
    pub total: u64,
    pub reflect: u64,
    pub skip: u64,
    /// Per rule, in filter order: the pages that rule decides first.
    pub rules: Vec<u64>,
}

impl ReflectionFilter {
    /// The counts of one filter over the given pages, in one pass.
    pub fn preview<'a>(
        &self,
        pages: impl IntoIterator<Item = &'a PageSignals>,
        now: i64,
    ) -> FilterPreview {
        let mut preview = FilterPreview {
            total: 0,
            reflect: 0,
            skip: 0,
            rules: vec![0; self.rules.len()],
        };
        for signals in pages {
            let selection = self.select(signals, now);
            preview.total += 1;
            if selection.verdict.reflects() {
                preview.reflect += 1;
            } else {
                preview.skip += 1;
            }
            if let Some(rule) = selection.rule {
                preview.rules[rule] += 1;
            }
        }
        preview
    }

    /// Rejects an unknown signal, an operator the kind does not allow, a
    /// value of the wrong shape, an empty rule, and an option that the
    /// catalogue does not offer.
    pub fn validate(&self, catalogue: &Catalogue) -> Result<(), FilterError> {
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.conditions.is_empty() {
                return Err(FilterError {
                    rule: index,
                    condition: None,
                    message: "a rule must have at least one condition".into(),
                });
            }
            for (position, condition) in rule.conditions.iter().enumerate() {
                condition
                    .validate(catalogue)
                    .map_err(|message| FilterError {
                        rule: index,
                        condition: Some(position),
                        message,
                    })?;
            }
        }
        Ok(())
    }

    /// The verdict for one page. A missing signal makes its condition
    /// false, so a rule that names a signal the page does not have
    /// cannot give the verdict.
    pub fn select(&self, signals: &PageSignals, now: i64) -> Selection {
        for (index, rule) in self.rules.iter().enumerate() {
            if rule
                .conditions
                .iter()
                .all(|condition| condition.holds(signals, now))
            {
                return Selection {
                    verdict: rule.verdict,
                    rule: Some(index),
                    reason: format!(
                        "Rule {} ({}): {}",
                        index + 1,
                        rule.verdict.as_str(),
                        rule.conditions
                            .iter()
                            .map(Condition::describe)
                            .collect::<Vec<_>>()
                            .join(" and ")
                    ),
                };
            }
        }
        Selection {
            verdict: self.default,
            rule: None,
            reason: format!(
                "No rule applies. The default verdict is {}.",
                self.default.as_str()
            ),
        }
    }
}

impl Condition {
    fn validate(&self, catalogue: &Catalogue) -> Result<(), String> {
        let signal = catalogue
            .signal(&self.signal)
            .ok_or_else(|| format!("{} is not a signal of this resource", self.signal))?;
        if !signal.kind.operators().contains(&self.operator) {
            return Err(format!(
                "{} does not accept \"{}\"",
                signal.id,
                self.operator.as_str()
            ));
        }
        let shape = match (&signal.kind, self.operator) {
            (_, Operator::WithinLast | Operator::OlderThan) => "duration",
            (_, Operator::In | Operator::NotIn) if signal.kind.options().is_none() => "list",
            (ValueKind::Choice { .. }, Operator::In) => "choices",
            (ValueKind::Choice { .. } | ValueKind::TagSet { .. }, _) => "choice",
            (ValueKind::DateTime, _) => "date",
            (ValueKind::Number, _) => "number",
            (ValueKind::Boolean, _) => "boolean",
            _ => "text",
        };
        let options = signal.kind.options().unwrap_or_default();
        let known = |value: &String| -> Result<(), String> {
            match options.contains(value) {
                true => Ok(()),
                false => Err(format!("{} is not an option of {}", value, signal.id)),
            }
        };
        match (shape, &self.value) {
            ("duration", ConditionValue::Duration { amount, .. }) => match amount {
                0 => Err("a duration must be more than zero".into()),
                _ => Ok(()),
            },
            ("list", ConditionValue::List { values }) => {
                if values.is_empty() {
                    return Err("a list must have at least one entry".into());
                }
                let format = signal.kind.list_format();
                match values.iter().find(|value| !format.accepts(value)) {
                    Some(value) => Err(format!("{value} is not a {}", format.name())),
                    None => Ok(()),
                }
            }
            ("choices", ConditionValue::Choices { values }) => {
                if values.is_empty() {
                    return Err("a list must have at least one entry".into());
                }
                values.iter().try_for_each(known)
            }
            ("choice", ConditionValue::Choice { value }) => known(value),
            ("date", ConditionValue::DateTime { .. }) => Ok(()),
            ("number", ConditionValue::Number { .. }) => Ok(()),
            ("boolean", ConditionValue::Boolean { .. }) => Ok(()),
            ("text", ConditionValue::Text { value }) => match value.trim().is_empty() {
                true => Err("a text condition must have a value".into()),
                false => Ok(()),
            },
            (shape, _) => Err(format!(
                "\"{}\" needs a {shape} value",
                self.operator.as_str()
            )),
        }
    }

    fn holds(&self, signals: &PageSignals, now: i64) -> bool {
        let Some(value) = signals.get(&self.signal) else {
            return false;
        };
        match (value, &self.value) {
            (SignalValue::Text { values }, ConditionValue::Text { value }) => {
                let text = || {
                    values
                        .iter()
                        .any(|held| matches(held, value, self.operator))
                };
                match self.operator {
                    Operator::NotContains => !values
                        .iter()
                        .any(|held| matches(held, value, Operator::Contains)),
                    _ => text(),
                }
            }
            (SignalValue::Text { values }, ConditionValue::List { values: wanted }) => {
                let inside = values
                    .iter()
                    .any(|held| wanted.iter().any(|want| equal(held, want)));
                match self.operator {
                    Operator::In => inside,
                    Operator::NotIn => !inside,
                    _ => false,
                }
            }
            (SignalValue::DateTime { at }, ConditionValue::DateTime { at: bound }) => {
                match self.operator {
                    Operator::After => at > bound,
                    Operator::Before => at < bound,
                    _ => false,
                }
            }
            (SignalValue::DateTime { at }, ConditionValue::Duration { amount, unit }) => {
                let span = i64::from(*amount).saturating_mul(unit.millis());
                let age = now.saturating_sub(*at);
                match self.operator {
                    Operator::WithinLast => age <= span,
                    Operator::OlderThan => age > span,
                    _ => false,
                }
            }
            (SignalValue::Number { value }, ConditionValue::Number { value: bound }) => {
                match self.operator {
                    Operator::AtLeast => value >= bound,
                    Operator::AtMost => value <= bound,
                    Operator::Is => value == bound,
                    _ => false,
                }
            }
            (SignalValue::Boolean { value }, ConditionValue::Boolean { value: wanted }) => {
                self.operator == Operator::Is && value == wanted
            }
            (SignalValue::Choice { value }, ConditionValue::Choice { value: wanted }) => {
                match self.operator {
                    Operator::Is => value == wanted,
                    Operator::IsNot => value != wanted,
                    _ => false,
                }
            }
            (SignalValue::Choice { value }, ConditionValue::Choices { values }) => {
                self.operator == Operator::In && values.contains(value)
            }
            (SignalValue::Tags { values }, ConditionValue::Choice { value }) => {
                match self.operator {
                    Operator::Has => values.contains(value),
                    Operator::Lacks => !values.contains(value),
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

impl ListFormat {
    fn name(self) -> &'static str {
        match self {
            ListFormat::Email => "mail address",
            ListFormat::Domain => "mail domain",
            ListFormat::Plain => "value",
        }
    }
}

fn equal(held: &str, wanted: &str) -> bool {
    held.trim().eq_ignore_ascii_case(wanted.trim())
}

fn matches(held: &str, wanted: &str, operator: Operator) -> bool {
    let held = held.to_lowercase();
    let wanted = wanted.trim().to_lowercase();
    match operator {
        Operator::Contains => held.contains(&wanted),
        Operator::Is => held.trim() == wanted,
        Operator::StartsWith => held.trim_start().starts_with(&wanted),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;
    const NOW: i64 = 1_789_041_600_000;

    fn catalogue() -> Catalogue {
        Catalogue {
            signals: vec![
                Signal {
                    id: "subject".into(),
                    label: "Subject".into(),
                    kind: ValueKind::Text,
                },
                Signal {
                    id: "sender_address".into(),
                    label: "Sender address".into(),
                    kind: ValueKind::List {
                        format: ListFormat::Email,
                    },
                },
                Signal {
                    id: "newest_at".into(),
                    label: "Newest message".into(),
                    kind: ValueKind::DateTime,
                },
                Signal {
                    id: "message_count".into(),
                    label: "Messages".into(),
                    kind: ValueKind::Number,
                },
                Signal {
                    id: "owner_replied".into(),
                    label: "Owner replied".into(),
                    kind: ValueKind::Boolean,
                },
                Signal {
                    id: "category".into(),
                    label: "Category".into(),
                    kind: ValueKind::Choice {
                        options: vec!["primary".into(), "promotions".into()],
                    },
                },
                Signal {
                    id: "labels".into(),
                    label: "Labels".into(),
                    kind: ValueKind::TagSet {
                        options: vec!["IMPORTANT".into(), "STARRED".into()],
                    },
                },
            ],
        }
    }

    fn signals() -> PageSignals {
        PageSignals(BTreeMap::from([
            (
                "subject".into(),
                SignalValue::Text {
                    values: vec!["Clinic visit".into(), "Re: Clinic VISIT".into()],
                },
            ),
            (
                "sender_address".into(),
                SignalValue::Text {
                    values: vec!["clinic@example.com".into()],
                },
            ),
            ("newest_at".into(), SignalValue::DateTime { at: NOW - DAY }),
            ("message_count".into(), SignalValue::Number { value: 3 }),
            ("owner_replied".into(), SignalValue::Boolean { value: true }),
            (
                "category".into(),
                SignalValue::Choice {
                    value: "primary".into(),
                },
            ),
            (
                "labels".into(),
                SignalValue::Tags {
                    values: vec!["IMPORTANT".into()],
                },
            ),
        ]))
    }

    fn condition(signal: &str, operator: Operator, value: ConditionValue) -> Condition {
        Condition {
            signal: signal.into(),
            operator,
            value,
        }
    }

    fn holds(signal: &str, operator: Operator, value: ConditionValue) -> bool {
        condition(signal, operator, value).holds(&signals(), NOW)
    }

    fn text(value: &str) -> ConditionValue {
        ConditionValue::Text {
            value: value.into(),
        }
    }

    #[test]
    fn a_text_operator_reads_every_message_value_and_ignores_case() {
        assert!(holds("subject", Operator::Contains, text("clinic")));
        assert!(!holds("subject", Operator::Contains, text("invoice")));
        assert!(holds("subject", Operator::NotContains, text("invoice")));
        assert!(!holds("subject", Operator::NotContains, text("clinic")));
        assert!(holds("subject", Operator::Is, text("clinic visit")));
        assert!(!holds("subject", Operator::Is, text("clinic")));
        assert!(holds("subject", Operator::StartsWith, text("Re:")));
        assert!(!holds("subject", Operator::StartsWith, text("Fwd:")));
    }

    #[test]
    fn a_list_operator_tests_membership_of_a_text_signal() {
        let list = |values: &[&str]| ConditionValue::List {
            values: values.iter().map(|v| (*v).into()).collect(),
        };
        assert!(holds(
            "sender_address",
            Operator::In,
            list(&["CLINIC@example.com", "other@example.com"])
        ));
        assert!(!holds("sender_address", Operator::In, list(&["a@b.com"])));
        assert!(holds("sender_address", Operator::NotIn, list(&["a@b.com"])));
    }

    #[test]
    fn a_date_operator_compares_the_instant_and_a_duration_compares_the_age() {
        let at = |at: i64| ConditionValue::DateTime { at };
        assert!(holds("newest_at", Operator::After, at(NOW - 2 * DAY)));
        assert!(!holds("newest_at", Operator::After, at(NOW)));
        assert!(holds("newest_at", Operator::Before, at(NOW)));
        assert!(!holds("newest_at", Operator::Before, at(NOW - 2 * DAY)));
        let days = |amount: u32| ConditionValue::Duration {
            amount,
            unit: DurationUnit::Days,
        };
        assert!(holds("newest_at", Operator::WithinLast, days(2)));
        // The age is exactly one day, so the window includes it and the
        // "older than" test does not.
        assert!(holds("newest_at", Operator::WithinLast, days(1)));
        assert!(!holds("newest_at", Operator::OlderThan, days(1)));
        assert!(!holds("newest_at", Operator::OlderThan, days(2)));
        let hours = |amount: u32| ConditionValue::Duration {
            amount,
            unit: DurationUnit::Hours,
        };
        assert!(holds("newest_at", Operator::WithinLast, hours(25)));
        assert!(!holds("newest_at", Operator::WithinLast, hours(23)));
        assert!(holds("newest_at", Operator::OlderThan, hours(23)));
    }

    #[test]
    fn number_boolean_choice_and_tag_operators_compare_their_values() {
        let number = |value: i64| ConditionValue::Number { value };
        assert!(holds("message_count", Operator::AtLeast, number(3)));
        assert!(!holds("message_count", Operator::AtLeast, number(4)));
        assert!(holds("message_count", Operator::AtMost, number(3)));
        assert!(!holds("message_count", Operator::AtMost, number(2)));
        assert!(holds("message_count", Operator::Is, number(3)));
        assert!(!holds("message_count", Operator::Is, number(2)));
        let boolean = |value: bool| ConditionValue::Boolean { value };
        assert!(holds("owner_replied", Operator::Is, boolean(true)));
        assert!(!holds("owner_replied", Operator::Is, boolean(false)));
        let choice = |value: &str| ConditionValue::Choice {
            value: value.into(),
        };
        assert!(holds("category", Operator::Is, choice("primary")));
        assert!(!holds("category", Operator::Is, choice("promotions")));
        assert!(holds("category", Operator::IsNot, choice("promotions")));
        assert!(!holds("category", Operator::IsNot, choice("primary")));
        assert!(holds(
            "category",
            Operator::In,
            ConditionValue::Choices {
                values: vec!["primary".into(), "promotions".into()],
            }
        ));
        assert!(!holds(
            "category",
            Operator::In,
            ConditionValue::Choices {
                values: vec!["promotions".into()],
            }
        ));
        assert!(holds("labels", Operator::Has, choice("IMPORTANT")));
        assert!(!holds("labels", Operator::Has, choice("STARRED")));
        assert!(holds("labels", Operator::Lacks, choice("STARRED")));
        assert!(!holds("labels", Operator::Lacks, choice("IMPORTANT")));
    }

    #[test]
    fn a_missing_signal_makes_its_condition_false() {
        let empty = PageSignals::default();
        assert!(!condition("subject", Operator::Contains, text("clinic")).holds(&empty, NOW));
        assert!(!condition("subject", Operator::NotContains, text("clinic")).holds(&empty, NOW));
    }

    #[test]
    fn the_first_rule_whose_conditions_all_hold_gives_the_verdict() {
        let filter = ReflectionFilter {
            rules: vec![
                Rule {
                    verdict: Verdict::Skip,
                    conditions: vec![
                        condition("subject", Operator::Contains, text("clinic")),
                        condition(
                            "message_count",
                            Operator::AtLeast,
                            ConditionValue::Number { value: 9 },
                        ),
                    ],
                },
                Rule {
                    verdict: Verdict::Reflect,
                    conditions: vec![condition(
                        "labels",
                        Operator::Has,
                        ConditionValue::Choice {
                            value: "IMPORTANT".into(),
                        },
                    )],
                },
            ],
            default: Verdict::Skip,
        };
        let selection = filter.select(&signals(), NOW);
        assert_eq!(selection.verdict, Verdict::Reflect);
        assert_eq!(selection.rule, Some(1));
        assert!(selection.reason.contains("labels has IMPORTANT"));
    }

    #[test]
    fn a_preview_counts_the_pages_each_rule_decides_first() {
        let filter = ReflectionFilter {
            rules: vec![
                Rule {
                    verdict: Verdict::Skip,
                    conditions: vec![condition("subject", Operator::Contains, text("clinic"))],
                },
                Rule {
                    verdict: Verdict::Reflect,
                    conditions: vec![condition(
                        "labels",
                        Operator::Has,
                        ConditionValue::Choice {
                            value: "IMPORTANT".into(),
                        },
                    )],
                },
            ],
            default: Verdict::Reflect,
        };
        let clinic = signals();
        let mut shop = signals();
        shop.insert(
            "subject",
            SignalValue::Text {
                values: vec!["Half price".into()],
            },
        );
        let preview = filter.preview([&clinic, &clinic, &shop, &PageSignals::default()], NOW);
        assert_eq!(
            preview,
            FilterPreview {
                total: 4,
                reflect: 2,
                skip: 2,
                rules: vec![2, 1],
            }
        );
        assert_eq!(
            filter.preview([], NOW),
            FilterPreview {
                total: 0,
                reflect: 0,
                skip: 0,
                rules: vec![0, 0],
            }
        );
    }

    #[test]
    fn the_default_verdict_closes_the_list() {
        let filter = ReflectionFilter {
            rules: vec![Rule {
                verdict: Verdict::Reflect,
                conditions: vec![condition("subject", Operator::Contains, text("invoice"))],
            }],
            default: Verdict::Skip,
        };
        let selection = filter.select(&signals(), NOW);
        assert_eq!(selection.verdict, Verdict::Skip);
        assert_eq!(selection.rule, None);
        assert!(selection.reason.contains("default verdict is skip"));
    }

    fn rejects(condition: Condition) -> String {
        ReflectionFilter {
            rules: vec![Rule {
                verdict: Verdict::Reflect,
                conditions: vec![condition],
            }],
            default: Verdict::Reflect,
        }
        .validate(&catalogue())
        .expect_err("the filter is not valid")
        .to_string()
    }

    #[test]
    fn validation_rejects_an_unknown_signal_a_bad_operator_and_a_bad_shape() {
        assert!(
            rejects(condition("sender_mood", Operator::Is, text("warm")))
                .contains("sender_mood is not a signal")
        );
        assert!(
            rejects(condition("message_count", Operator::Contains, text("3")))
                .contains("message_count does not accept \"contains\"")
        );
        assert!(
            rejects(condition("message_count", Operator::AtLeast, text("three")))
                .contains("needs a number value")
        );
        assert!(
            rejects(condition(
                "newest_at",
                Operator::WithinLast,
                ConditionValue::Duration {
                    amount: 0,
                    unit: DurationUnit::Days,
                }
            ))
            .contains("more than zero")
        );
        assert!(
            rejects(condition("subject", Operator::Is, text("  "))).contains("must have a value")
        );
    }

    #[test]
    fn validation_rejects_an_option_the_catalogue_does_not_offer() {
        assert!(
            rejects(condition(
                "labels",
                Operator::Has,
                ConditionValue::Choice {
                    value: "SNOOZED".into(),
                }
            ))
            .contains("SNOOZED is not an option of labels")
        );
        assert!(
            rejects(condition(
                "category",
                Operator::In,
                ConditionValue::Choices {
                    values: vec!["primary".into(), "forums".into()],
                }
            ))
            .contains("forums is not an option of category")
        );
    }

    #[test]
    fn validation_rejects_a_list_entry_that_is_not_the_format_the_signal_names() {
        assert!(
            rejects(condition(
                "sender_address",
                Operator::In,
                ConditionValue::List {
                    values: vec!["example.com".into()],
                }
            ))
            .contains("example.com is not a mail address")
        );
        assert!(
            rejects(condition(
                "sender_address",
                Operator::In,
                ConditionValue::List { values: vec![] }
            ))
            .contains("at least one entry")
        );
    }

    #[test]
    fn validation_rejects_an_empty_rule_and_names_it() {
        let error = ReflectionFilter {
            rules: vec![
                Rule {
                    verdict: Verdict::Reflect,
                    conditions: vec![condition("subject", Operator::Contains, text("clinic"))],
                },
                Rule {
                    verdict: Verdict::Skip,
                    conditions: vec![],
                },
            ],
            default: Verdict::Reflect,
        }
        .validate(&catalogue())
        .expect_err("the filter is not valid");
        assert_eq!(error.rule, 1);
        assert_eq!(error.condition, None);
        assert_eq!(
            error.to_string(),
            "Rule 2: a rule must have at least one condition"
        );
    }

    #[test]
    fn a_valid_filter_passes_validation() {
        let filter = ReflectionFilter {
            rules: vec![Rule {
                verdict: Verdict::Skip,
                conditions: vec![
                    condition(
                        "category",
                        Operator::In,
                        ConditionValue::Choices {
                            values: vec!["promotions".into()],
                        },
                    ),
                    condition(
                        "sender_address",
                        Operator::NotIn,
                        ConditionValue::List {
                            values: vec!["clinic@example.com".into()],
                        },
                    ),
                ],
            }],
            default: Verdict::Reflect,
        };
        assert_eq!(filter.validate(&catalogue()), Ok(()));
    }

    #[test]
    fn a_filter_makes_a_round_trip_through_json_and_refuses_an_unknown_field() {
        let filter = ReflectionFilter {
            rules: vec![Rule {
                verdict: Verdict::Reflect,
                conditions: vec![condition("subject", Operator::Contains, text("clinic"))],
            }],
            default: Verdict::Skip,
        };
        let text = serde_json::to_string(&filter).unwrap();
        assert_eq!(
            serde_json::from_str::<ReflectionFilter>(&text).unwrap(),
            filter
        );
        assert!(
            serde_json::from_str::<ReflectionFilter>(
                r#"{"rules":[],"default":"skip","mode":"strict"}"#
            )
            .is_err()
        );
    }
}
