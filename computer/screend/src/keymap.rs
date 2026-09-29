//! User keyboard input: the fixed US xkb keymap the pipeline
//! uploads to its virtual keyboard, and the static
//! `KeyboardEvent.code` → evdev keycode table the viewer's key events
//! map through.

/// The compiled US keymap (xkb_v1 text format), generated with
/// `xkbcli compile-keymap --layout us` and committed as an asset.
pub const US_KEYMAP: &str = include_str!("../assets/us.xkb");

/// xkb real-modifier masks in the US keymap.
pub const MOD_SHIFT: u32 = 1 << 0;
pub const MOD_LOCK: u32 = 1 << 1;
pub const MOD_CTRL: u32 = 1 << 2;
pub const MOD_ALT: u32 = 1 << 3;
pub const MOD_SUPER: u32 = 1 << 6;

/// The depressed-modifier mask a key contributes while held.
pub fn modifier_mask(code: &str) -> Option<u32> {
    match code {
        "ShiftLeft" | "ShiftRight" => Some(MOD_SHIFT),
        "ControlLeft" | "ControlRight" => Some(MOD_CTRL),
        "AltLeft" | "AltRight" => Some(MOD_ALT),
        "MetaLeft" | "MetaRight" => Some(MOD_SUPER),
        _ => None,
    }
}

/// Map one browser `KeyboardEvent.code` to its evdev keycode
/// (`input-event-codes.h`). The compositor interprets the code through
/// the uploaded US keymap.
pub fn code_to_evdev(code: &str) -> Option<u32> {
    Some(match code {
        "Escape" => 1,
        "Digit1" => 2,
        "Digit2" => 3,
        "Digit3" => 4,
        "Digit4" => 5,
        "Digit5" => 6,
        "Digit6" => 7,
        "Digit7" => 8,
        "Digit8" => 9,
        "Digit9" => 10,
        "Digit0" => 11,
        "Minus" => 12,
        "Equal" => 13,
        "Backspace" => 14,
        "Tab" => 15,
        "KeyQ" => 16,
        "KeyW" => 17,
        "KeyE" => 18,
        "KeyR" => 19,
        "KeyT" => 20,
        "KeyY" => 21,
        "KeyU" => 22,
        "KeyI" => 23,
        "KeyO" => 24,
        "KeyP" => 25,
        "BracketLeft" => 26,
        "BracketRight" => 27,
        "Enter" => 28,
        "ControlLeft" => 29,
        "KeyA" => 30,
        "KeyS" => 31,
        "KeyD" => 32,
        "KeyF" => 33,
        "KeyG" => 34,
        "KeyH" => 35,
        "KeyJ" => 36,
        "KeyK" => 37,
        "KeyL" => 38,
        "Semicolon" => 39,
        "Quote" => 40,
        "Backquote" => 41,
        "ShiftLeft" => 42,
        "Backslash" => 43,
        "KeyZ" => 44,
        "KeyX" => 45,
        "KeyC" => 46,
        "KeyV" => 47,
        "KeyB" => 48,
        "KeyN" => 49,
        "KeyM" => 50,
        "Comma" => 51,
        "Period" => 52,
        "Slash" => 53,
        "ShiftRight" => 54,
        "NumpadMultiply" => 55,
        "AltLeft" => 56,
        "Space" => 57,
        "CapsLock" => 58,
        "F1" => 59,
        "F2" => 60,
        "F3" => 61,
        "F4" => 62,
        "F5" => 63,
        "F6" => 64,
        "F7" => 65,
        "F8" => 66,
        "F9" => 67,
        "F10" => 68,
        "NumLock" => 69,
        "ScrollLock" => 70,
        "Numpad7" => 71,
        "Numpad8" => 72,
        "Numpad9" => 73,
        "NumpadSubtract" => 74,
        "Numpad4" => 75,
        "Numpad5" => 76,
        "Numpad6" => 77,
        "NumpadAdd" => 78,
        "Numpad1" => 79,
        "Numpad2" => 80,
        "Numpad3" => 81,
        "Numpad0" => 82,
        "NumpadDecimal" => 83,
        "F11" => 87,
        "F12" => 88,
        "NumpadEnter" => 96,
        "ControlRight" => 97,
        "NumpadDivide" => 98,
        "AltRight" => 100,
        "Home" => 102,
        "ArrowUp" => 103,
        "PageUp" => 104,
        "ArrowLeft" => 105,
        "ArrowRight" => 106,
        "End" => 107,
        "ArrowDown" => 108,
        "PageDown" => 109,
        "Insert" => 110,
        "Delete" => 111,
        "MetaLeft" => 125,
        "MetaRight" => 126,
        "ContextMenu" => 127,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_navigation_map_to_evdev() {
        assert_eq!(code_to_evdev("KeyA"), Some(30));
        assert_eq!(code_to_evdev("Digit0"), Some(11));
        assert_eq!(code_to_evdev("Enter"), Some(28));
        assert_eq!(code_to_evdev("ArrowLeft"), Some(105));
        assert_eq!(code_to_evdev("Unidentified"), None);
    }

    #[test]
    fn modifier_keys_carry_their_masks() {
        assert_eq!(modifier_mask("ShiftRight"), Some(MOD_SHIFT));
        assert_eq!(modifier_mask("ControlLeft"), Some(MOD_CTRL));
        assert_eq!(modifier_mask("MetaLeft"), Some(MOD_SUPER));
        assert_eq!(modifier_mask("KeyA"), None);
    }

    #[test]
    fn the_vendored_keymap_is_a_compiled_us_keymap() {
        assert!(US_KEYMAP.starts_with("xkb_keymap {"));
        assert!(US_KEYMAP.contains("modifier_map Shift { <LFSH>, <RTSH> };"));
    }
}
