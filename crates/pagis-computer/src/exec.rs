//! The computer-use action executor: both provider action
//! vocabularies normalize into one internal action list, coordinates
//! rescale from the model's screenshot space into capture space, and
//! a batch executes sequentially, stopping at the first failure. The
//! router deliberately keeps the vocabularies provider-raw; this
//! module is the one translation point.

use serde::Serialize;
use serde_json::Value;

/// The screen size the model is told about: the tool definition
/// and every screenshot use this space. The capture rescales to it.
pub const DISPLAY_WIDTH: u32 = 1280;
pub const DISPLAY_HEIGHT: u32 = 720;

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

/// A capture frame fitted to the model's display space: PNG at
/// `DISPLAY_WIDTH`x`DISPLAY_HEIGHT`, plus the factors that map model
/// coordinates back into capture space.
pub struct DisplayFrame {
    pub png: Vec<u8>,
    pub scale_x: f64,
    pub scale_y: f64,
}

/// Fit one captured PNG to the display space. A capture already at
/// display size passes through untouched (the compositor's headless
/// output normally is); anything else resizes, and coordinates from
/// the model scale back by capture/display.
pub fn fit_display(png: &[u8]) -> Result<DisplayFrame, String> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|err| format!("bad capture png: {err}"))?;
    let (width, height) = (image.width(), image.height());
    if (width, height) == (DISPLAY_WIDTH, DISPLAY_HEIGHT) {
        return Ok(DisplayFrame {
            png: png.to_vec(),
            scale_x: 1.0,
            scale_y: 1.0,
        });
    }
    let resized = image.resize_exact(
        DISPLAY_WIDTH,
        DISPLAY_HEIGHT,
        image::imageops::FilterType::Triangle,
    );
    let mut out = std::io::Cursor::new(Vec::new());
    resized
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|err| format!("png encode failed: {err}"))?;
    Ok(DisplayFrame {
        png: out.into_inner(),
        scale_x: width as f64 / DISPLAY_WIDTH as f64,
        scale_y: height as f64 / DISPLAY_HEIGHT as f64,
    })
}

/// The share of pixels two frames may differ by and still show one
/// settled screen: 0.5%, about a 68px square. A blinking caret or a
/// small animation stays below it, and a menu, a dialog or a new page
/// goes above it.
const SETTLED_CHANGE: f64 = 0.005;

/// Whether two PNG frames show the same settled screen. Frames that do
/// not decode, or differ in size, match only when their bytes are equal.
pub fn frames_match(a: &[u8], b: &[u8]) -> bool {
    if a == b {
        return true;
    }
    let decode = |png: &[u8]| {
        image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .ok()
            .map(|image| image.to_rgb8())
    };
    let (Some(a), Some(b)) = (decode(a), decode(b)) else {
        return false;
    };
    if a.dimensions() != b.dimensions() {
        return false;
    }
    let changed = a
        .pixels()
        .zip(b.pixels())
        .filter(|(left, right)| left != right)
        .count();
    (changed as f64) <= SETTLED_CHANGE * f64::from(a.width() * a.height())
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

/// Normalize one modifier or key name to the xkb keysym wtype expects.
fn key_name(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => "ctrl".to_string(),
        "shift" => "shift".to_string(),
        "alt" | "option" => "alt".to_string(),
        "super" | "meta" | "cmd" | "win" => "super".to_string(),
        "enter" | "return" => "Return".to_string(),
        "esc" | "escape" => "Escape".to_string(),
        "backspace" => "BackSpace".to_string(),
        "delete" | "del" => "Delete".to_string(),
        "tab" => "Tab".to_string(),
        "space" => "space".to_string(),
        "up" | "arrowup" => "Up".to_string(),
        "down" | "arrowdown" => "Down".to_string(),
        "left" | "arrowleft" => "Left".to_string(),
        "right" | "arrowright" => "Right".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" | "page_up" => "Page_Up".to_string(),
        "pagedown" | "page_down" => "Page_Down".to_string(),
        other if other.len() == 1 => other.to_string(),
        _ => raw.to_string(),
    }
}

/// Split an Anthropic key combo string (`ctrl+shift+t`, `Return`).
fn split_combo(combo: &str) -> Vec<String> {
    combo
        .split(['+', '-'])
        .filter(|part| !part.is_empty())
        .map(key_name)
        .collect()
}

fn coordinate(value: &Value) -> Result<(f64, f64), String> {
    let pair = value
        .as_array()
        .filter(|pair| pair.len() == 2)
        .ok_or_else(|| format!("bad coordinate: {value}"))?;
    match (pair[0].as_f64(), pair[1].as_f64()) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(format!("bad coordinate: {value}")),
    }
}

fn xy(action: &Value) -> Result<(f64, f64), String> {
    match (action["x"].as_f64(), action["y"].as_f64()) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(format!("action needs x and y: {action}")),
    }
}

fn keys_array(action: &Value) -> Vec<String> {
    action["keys"]
        .as_array()
        .map(|keys| {
            keys.iter()
                .filter_map(Value::as_str)
                .map(key_name)
                .collect()
        })
        .unwrap_or_default()
}

/// A key press names at least one key: an empty one does nothing on
/// the screen, and the model reads the error and corrects the call.
fn required_keys(keys: Vec<String>) -> Result<Vec<String>, String> {
    if keys.is_empty() {
        return Err("a key press needs at least one key, for example [\"ENTER\"]".to_string());
    }
    Ok(keys)
}

/// Parse one Anthropic `computer_20251124` action object.
pub fn parse_anthropic(action: &Value) -> Result<Action, String> {
    let kind = action["action"]
        .as_str()
        .ok_or_else(|| format!("action object without `action`: {action}"))?;
    let click = |button: &str, count: u32| -> Result<Action, String> {
        let (x, y) = coordinate(&action["coordinate"])?;
        let modifiers = action["text"].as_str().map(split_combo).unwrap_or_default();
        Ok(Action::Click {
            button: button.to_string(),
            x,
            y,
            modifiers,
            count,
        })
    };
    match kind {
        // `zoom` asks for a region crop; the answer is the full
        // screenshot, and the model can still read the area.
        "screenshot" | "zoom" => Ok(Action::Screenshot),
        "left_click" => click("left", 1),
        "right_click" => click("right", 1),
        "middle_click" => click("middle", 1),
        "double_click" => click("left", 2),
        "triple_click" => click("left", 3),
        "mouse_move" => {
            let (x, y) = coordinate(&action["coordinate"])?;
            Ok(Action::Move { x, y })
        }
        "left_mouse_down" => Ok(Action::MouseDown {
            button: "left".to_string(),
        }),
        "left_mouse_up" => Ok(Action::MouseUp {
            button: "left".to_string(),
        }),
        "left_click_drag" => {
            let start = coordinate(&action["start_coordinate"])?;
            let end = coordinate(&action["coordinate"])?;
            Ok(Action::Drag {
                path: vec![start, end],
            })
        }
        "scroll" => {
            let (x, y) = coordinate(&action["coordinate"])?;
            let amount = action["scroll_amount"].as_f64().unwrap_or(3.0);
            let (dx, dy) = match action["scroll_direction"].as_str() {
                Some("up") => (0.0, -amount),
                Some("down") | None => (0.0, amount),
                Some("left") => (-amount, 0.0),
                Some("right") => (amount, 0.0),
                Some(other) => return Err(format!("unknown scroll direction {other:?}")),
            };
            Ok(Action::Scroll { x, y, dx, dy })
        }
        "type" => Ok(Action::Type {
            text: action["text"].as_str().unwrap_or_default().to_string(),
        }),
        "key" => Ok(Action::Key {
            keys: required_keys(split_combo(action["text"].as_str().unwrap_or_default()))?,
            hold_ms: None,
            repeat: action["repeat"].as_u64().unwrap_or(1).max(1) as u32,
        }),
        "hold_key" => Ok(Action::Key {
            keys: required_keys(split_combo(action["text"].as_str().unwrap_or_default()))?,
            hold_ms: Some(
                (action["duration"].as_f64().unwrap_or(1.0) * 1000.0).clamp(0.0, 10_000.0) as u64,
            ),
            repeat: 1,
        }),
        "wait" => Ok(Action::Wait {
            ms: (action["duration"].as_f64().unwrap_or(1.0) * 1000.0).clamp(0.0, 30_000.0) as u64,
        }),
        "cursor_position" => Err("cursor_position is not supported; take a screenshot".to_string()),
        other => Err(format!("unknown action {other:?}")),
    }
}

/// Parse one OpenAI GA `computer` action.
pub fn parse_openai(action: &Value) -> Result<Action, String> {
    let kind = action["type"]
        .as_str()
        .ok_or_else(|| format!("action object without `type`: {action}"))?;
    match kind {
        "screenshot" => Ok(Action::Screenshot),
        "click" | "double_click" => {
            let (x, y) = xy(action)?;
            let button = match action["button"].as_str() {
                Some("wheel") => "middle",
                Some(button) => button,
                None => "left",
            };
            Ok(Action::Click {
                button: button.to_string(),
                x,
                y,
                modifiers: keys_array(action),
                count: if kind == "double_click" { 2 } else { 1 },
            })
        }
        "move" => {
            let (x, y) = xy(action)?;
            Ok(Action::Move { x, y })
        }
        "drag" => {
            let path = action["path"]
                .as_array()
                .ok_or_else(|| format!("drag needs a path: {action}"))?
                .iter()
                .map(xy)
                .collect::<Result<Vec<_>, _>>()?;
            if path.len() < 2 {
                return Err("drag path needs at least two points".to_string());
            }
            Ok(Action::Drag { path })
        }
        "scroll" => {
            let (x, y) = xy(action)?;
            // OpenAI scrolls in pixels; a wheel click is ~100 px.
            Ok(Action::Scroll {
                x,
                y,
                dx: action["scroll_x"].as_f64().unwrap_or(0.0) / 100.0,
                dy: action["scroll_y"].as_f64().unwrap_or(0.0) / 100.0,
            })
        }
        "type" => Ok(Action::Type {
            text: action["text"].as_str().unwrap_or_default().to_string(),
        }),
        "keypress" => Ok(Action::Key {
            keys: required_keys(keys_array(action))?,
            hold_ms: None,
            repeat: 1,
        }),
        "wait" => Ok(Action::Wait { ms: 1000 }),
        other => Err(format!("unknown action {other:?}")),
    }
}

/// The input operations one action injects, with model-space
/// coordinates rescaled into capture space by `(scale_x, scale_y)`.
/// `Screenshot` and `Wait` inject nothing; the executor handles them.
pub fn input_ops(action: &Action, scale_x: f64, scale_y: f64) -> Vec<InputOp> {
    let rescale = |x: f64, y: f64| (x * scale_x, y * scale_y);
    match action {
        Action::Screenshot | Action::Wait { .. } => Vec::new(),
        Action::Move { x, y } => {
            let (x, y) = rescale(*x, *y);
            vec![InputOp::Move { x, y }]
        }
        Action::Click {
            button,
            x,
            y,
            modifiers,
            count,
        } => {
            let (x, y) = rescale(*x, *y);
            let mut ops = Vec::new();
            if !modifiers.is_empty() {
                // Hold the modifiers around the clicks via key press
                // ops screend maps onto wtype -M/-m.
                ops.push(InputOp::Key {
                    keys: modifiers.clone(),
                    hold_ms: Some(0),
                });
            }
            ops.push(InputOp::Move { x, y });
            for _ in 0..*count {
                ops.push(InputOp::Button {
                    button: button.clone(),
                    down: true,
                });
                ops.push(InputOp::Button {
                    button: button.clone(),
                    down: false,
                });
            }
            ops
        }
        Action::MouseDown { button } => vec![InputOp::Button {
            button: button.clone(),
            down: true,
        }],
        Action::MouseUp { button } => vec![InputOp::Button {
            button: button.clone(),
            down: false,
        }],
        Action::Drag { path } => {
            let mut ops = Vec::new();
            let (x, y) = rescale(path[0].0, path[0].1);
            ops.push(InputOp::Move { x, y });
            ops.push(InputOp::Button {
                button: "left".to_string(),
                down: true,
            });
            for (x, y) in &path[1..] {
                let (x, y) = rescale(*x, *y);
                ops.push(InputOp::Move { x, y });
            }
            ops.push(InputOp::Button {
                button: "left".to_string(),
                down: false,
            });
            ops
        }
        Action::Scroll { x, y, dx, dy } => {
            let (x, y) = rescale(*x, *y);
            vec![InputOp::Move { x, y }, InputOp::Scroll { dx: *dx, dy: *dy }]
        }
        Action::Type { text } => vec![InputOp::Text { text: text.clone() }],
        Action::Key {
            keys,
            hold_ms,
            repeat,
        } => (0..*repeat)
            .map(|_| InputOp::Key {
                keys: keys.clone(),
                hold_ms: *hold_ms,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png_with_box(width: u32, height: u32) -> Vec<u8> {
        let mut image = image::RgbImage::new(DISPLAY_WIDTH, DISPLAY_HEIGHT);
        for x in 0..width {
            for y in 0..height {
                image.put_pixel(x, y, image::Rgb([255, 255, 255]));
            }
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    /// A blinking caret is not a screen that still changes.
    #[test]
    fn frames_that_differ_by_a_caret_match() {
        assert!(frames_match(&png_with_box(0, 0), &png_with_box(0, 0)));
        assert!(frames_match(&png_with_box(0, 0), &png_with_box(2, 20)));
    }

    /// A menu that opens changes a large area.
    #[test]
    fn frames_that_differ_by_a_menu_do_not_match() {
        assert!(!frames_match(&png_with_box(0, 0), &png_with_box(300, 200)));
    }

    #[test]
    fn frames_that_are_not_images_match_only_when_equal() {
        assert!(frames_match(b"frame", b"frame"));
        assert!(!frames_match(b"frame", b"other"));
    }

    /// A key press without a key does nothing on the screen. The model
    /// reads an error and can correct the call.
    #[test]
    fn a_key_press_without_a_key_is_an_error() {
        assert!(parse_openai(&json!({"type": "keypress", "keys": []})).is_err());
        assert!(parse_openai(&json!({"type": "keypress"})).is_err());
        assert!(parse_anthropic(&json!({"action": "key", "text": ""})).is_err());
    }

    fn png_of(width: u32, height: u32) -> Vec<u8> {
        let image = image::DynamicImage::new_rgb8(width, height);
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn display_size_captures_pass_through() {
        let png = png_of(DISPLAY_WIDTH, DISPLAY_HEIGHT);
        let frame = fit_display(&png).unwrap();
        assert_eq!(frame.png, png);
        assert_eq!((frame.scale_x, frame.scale_y), (1.0, 1.0));
    }

    #[test]
    fn oversize_captures_downscale_and_coordinates_round_trip() {
        let frame = fit_display(&png_of(2560, 1440)).unwrap();
        let resized =
            image::load_from_memory_with_format(&frame.png, image::ImageFormat::Png).unwrap();
        assert_eq!(
            (resized.width(), resized.height()),
            (DISPLAY_WIDTH, DISPLAY_HEIGHT)
        );
        assert_eq!((frame.scale_x, frame.scale_y), (2.0, 2.0));
        // A model click at display center lands at capture center.
        let ops = input_ops(
            &Action::Move { x: 640.0, y: 360.0 },
            frame.scale_x,
            frame.scale_y,
        );
        assert_eq!(
            ops[0],
            InputOp::Move {
                x: 1280.0,
                y: 720.0
            }
        );
    }

    #[test]
    fn anthropic_actions_normalize() {
        let click = parse_anthropic(&json!({
            "action": "double_click", "coordinate": [10, 20], "text": "ctrl"
        }))
        .unwrap();
        assert_eq!(
            click,
            Action::Click {
                button: "left".to_string(),
                x: 10.0,
                y: 20.0,
                modifiers: vec!["ctrl".to_string()],
                count: 2
            }
        );

        let scroll = parse_anthropic(&json!({
            "action": "scroll", "coordinate": [5, 6],
            "scroll_direction": "up", "scroll_amount": 2
        }))
        .unwrap();
        assert_eq!(
            scroll,
            Action::Scroll {
                x: 5.0,
                y: 6.0,
                dx: 0.0,
                dy: -2.0
            }
        );

        let key = parse_anthropic(&json!({
            "action": "key", "text": "ctrl+shift+Return", "repeat": 3
        }))
        .unwrap();
        assert_eq!(
            key,
            Action::Key {
                keys: vec!["ctrl".into(), "shift".into(), "Return".into()],
                hold_ms: None,
                repeat: 3
            }
        );

        assert_eq!(
            parse_anthropic(&json!({"action": "zoom", "region": [0, 0, 10, 10]})).unwrap(),
            Action::Screenshot
        );
        assert!(parse_anthropic(&json!({"action": "cursor_position"})).is_err());
        assert!(parse_anthropic(&json!({"action": "levitate"})).is_err());
    }

    #[test]
    fn openai_actions_normalize() {
        let click = parse_openai(&json!({
            "type": "click", "button": "wheel", "x": 3, "y": 4
        }))
        .unwrap();
        assert_eq!(
            click,
            Action::Click {
                button: "middle".to_string(),
                x: 3.0,
                y: 4.0,
                modifiers: vec![],
                count: 1
            }
        );

        let keypress = parse_openai(&json!({
            "type": "keypress", "keys": ["CTRL", "L"]
        }))
        .unwrap();
        assert_eq!(
            keypress,
            Action::Key {
                keys: vec!["ctrl".into(), "l".into()],
                hold_ms: None,
                repeat: 1
            }
        );

        let drag = parse_openai(&json!({
            "type": "drag", "path": [{"x": 0, "y": 0}, {"x": 10, "y": 10}]
        }))
        .unwrap();
        assert_eq!(
            drag,
            Action::Drag {
                path: vec![(0.0, 0.0), (10.0, 10.0)]
            }
        );

        assert!(parse_openai(&json!({"type": "drag", "path": [{"x": 0, "y": 0}]})).is_err());
    }

    #[test]
    fn coordinates_rescale_into_capture_space() {
        let ops = input_ops(
            &Action::Click {
                button: "left".to_string(),
                x: 100.0,
                y: 50.0,
                modifiers: vec![],
                count: 1,
            },
            2.0,
            1.5,
        );
        assert_eq!(ops[0], InputOp::Move { x: 200.0, y: 75.0 });
        assert_eq!(
            ops[1],
            InputOp::Button {
                button: "left".to_string(),
                down: true
            }
        );

        // A double click presses twice after one move.
        let ops = input_ops(
            &Action::Click {
                button: "left".to_string(),
                x: 1.0,
                y: 1.0,
                modifiers: vec![],
                count: 2,
            },
            1.0,
            1.0,
        );
        assert_eq!(ops.len(), 5);
    }

    #[test]
    fn drags_move_press_trace_release() {
        let ops = input_ops(
            &Action::Drag {
                path: vec![(0.0, 0.0), (5.0, 5.0), (9.0, 9.0)],
            },
            1.0,
            1.0,
        );
        assert_eq!(ops.len(), 5);
        assert!(matches!(ops[1], InputOp::Button { ref button, down: true } if button == "left"));
        assert!(matches!(ops[4], InputOp::Button { ref button, down: false } if button == "left"));
    }
}
