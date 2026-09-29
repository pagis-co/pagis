//! The daemon's browser channel (ADR-0013).
//!
//! screend drives the Computer's Chromium over the DevTools protocol on
//! a pipe that only screend and the browser hold. The Agent's uid cannot
//! open it. The daemon uses the channel for a Vault fill alone, and only
//! while it holds the input switch: it opens a page in its own tab of
//! the browser, reads the top-level address the tab shows, and writes
//! text into fields that screend verifies in the same step as the
//! write. No text goes through the compositor, so the focused window
//! never receives a secret.

use serde::Serialize;

/// The kind of field one value goes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    /// A text, email or one-time-code field: a username or a code.
    Text,
    /// A password field: a secret.
    Password,
}

/// One value of a fill and the kind of field it goes into.
///
/// The first value of a fill goes into the field that has the focus in
/// the top frame. Each next value goes into the first field of its kind
/// after the previous field in the same form. screend moves the focus
/// there and verifies that the field keeps it before it writes.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct FillField {
    pub kind: FieldKind,
    pub text: String,
}

impl FillField {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: FieldKind::Text,
            text: text.into(),
        }
    }

    pub fn password(text: impl Into<String>) -> Self {
        Self {
            kind: FieldKind::Password,
            text: text.into(),
        }
    }
}

/// The text is a secret or a one-time code, so a debug print shows its
/// kind and never its value.
impl std::fmt::Debug for FillField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FillField")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_debug_print_never_shows_the_text() {
        let printed = format!("{:?}", [FillField::password("hunter2")]);

        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(printed.contains("Password"), "{printed}");
    }

    #[test]
    fn the_wire_shape_names_the_kind_and_carries_the_text() {
        let wire = serde_json::to_value(FillField::text("alice")).unwrap();

        assert_eq!(wire, serde_json::json!({"kind": "text", "text": "alice"}));
    }
}
