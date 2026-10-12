use serde::Deserialize;

/// One user input event from a viewer's data channel: `move`,
/// `button`, `scroll`, `key`, or `text` with at most
/// [`USER_TEXT_LIMIT`] characters.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum UserInput {
    Move {
        x: f64,
        y: f64,
    },
    Button {
        button: String,
        down: bool,
    },
    Scroll {
        dx: f64,
        dy: f64,
    },
    /// One raw key transition, as a browser `KeyboardEvent.code`.
    Key {
        code: String,
        down: bool,
    },
    /// Text from an on-screen keyboard or an IME, which gives
    /// characters and no reliable key code: `{"op": "text", "text":
    /// "hé"}`. screend types it as `InputOp::Text` does.
    Text {
        text: UserText,
    },
}

/// The most characters that one `text` user input holds. The limit
/// keeps one data channel message small and bounds the work of one
/// `type_text` call.
pub const USER_TEXT_LIMIT: usize = 1024;

/// The text of one `text` user input: at most [`USER_TEXT_LIMIT`]
/// characters. A longer text does not parse, and the refusal does not
/// hold the text, because the Person can type a secret.
#[derive(Debug, Deserialize)]
#[serde(try_from = "String")]
pub struct UserText(String);

impl UserText {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for UserText {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        let count = text.chars().count();
        if count > USER_TEXT_LIMIT {
            return Err(format!(
                "a text of {count} characters is more than the limit of {USER_TEXT_LIMIT}"
            ));
        }
        Ok(UserText(text))
    }
}
