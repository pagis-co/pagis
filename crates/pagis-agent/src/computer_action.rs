//! Computer-use actions: normalize provider input, scale coordinates
//! and fit PNG captures to the model's display space.

use pagis_core::computer_action::{Action, DisplayFrame, InputOp};
use serde_json::Value;

/// The screen size the model is told about: the tool definition
/// and every screenshot use this space. The capture rescales to it.
pub const DISPLAY_WIDTH: u32 = 1280;
pub const DISPLAY_HEIGHT: u32 = 720;

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

/// Parse one call to a member of the Anthropic computer toolset
/// (`computer_toolset_20260801`): its `tool_use` block, whose `name` is
/// the member and whose `input` holds the member's parameters.
pub fn parse_anthropic(call: &Value) -> Result<Action, String> {
    let kind = call["name"]
        .as_str()
        .ok_or_else(|| format!("computer call without a member name: {call}"))?;
    let action = &call["input"];
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
        "screenshot" => Ok(Action::Screenshot),
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

    /// A key press without a key does nothing on the screen. The model
    /// reads an error and can correct the call.
    #[test]
    fn a_key_press_without_a_key_is_an_error() {
        assert!(parse_openai(&json!({"type": "keypress", "keys": []})).is_err());
        assert!(parse_openai(&json!({"type": "keypress"})).is_err());
        assert!(parse_anthropic(&member("key", json!({"text": ""}))).is_err());
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

    fn member(name: &str, input: Value) -> Value {
        json!({
            "type": "tool_use", "id": "toolu_1", "name": name,
            "toolset_name": "computer", "input": input
        })
    }

    #[test]
    fn anthropic_actions_normalize() {
        let click = parse_anthropic(&member(
            "double_click",
            json!({"coordinate": [10, 20], "text": "ctrl"}),
        ))
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

        let scroll = parse_anthropic(&member(
            "scroll",
            json!({"coordinate": [5, 6], "scroll_direction": "up", "scroll_amount": 2}),
        ))
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

        let key = parse_anthropic(&member(
            "key",
            json!({"text": "ctrl+shift+Return", "repeat": 3}),
        ))
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
            parse_anthropic(&member("screenshot", json!({}))).unwrap(),
            Action::Screenshot
        );
        assert!(parse_anthropic(&member("cursor_position", json!({}))).is_err());
        assert!(parse_anthropic(&member("levitate", json!({}))).is_err());
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
