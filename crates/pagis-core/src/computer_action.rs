//! Plain values for Computer input, actions and captures.

use serde::Serialize;

/// One input operation, the wire shape screend executes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum InputOp {
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
    Text {
        text: String,
    },
    Key {
        keys: Vec<String>,
        hold_ms: Option<u64>,
    },
}

/// A PNG capture fitted to the model's display space, with the factors
/// that map model coordinates back into capture space.
pub struct DisplayFrame {
    pub png: Vec<u8>,
    pub scale_x: f64,
    pub scale_y: f64,
}

/// One normalized computer action.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Screenshot,
    Click {
        button: String,
        x: f64,
        y: f64,
        /// Modifier keys held around the click.
        modifiers: Vec<String>,
        count: u32,
    },
    MouseDown {
        button: String,
    },
    MouseUp {
        button: String,
    },
    Move {
        x: f64,
        y: f64,
    },
    Drag {
        path: Vec<(f64, f64)>,
    },
    Scroll {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    Type {
        text: String,
    },
    Key {
        keys: Vec<String>,
        hold_ms: Option<u64>,
        repeat: u32,
    },
    Wait {
        ms: u64,
    },
}
