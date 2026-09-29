//! Daemon and agent text injection through the virtual keyboard.
//!
//! The image ships no `wtype`: a Wayland client the agent's shell can
//! also run is exactly the bypass ADR-0013 closes. screend types the
//! way `wtype` does — it compiles one throwaway xkb
//! keymap that maps a keycode to each keysym the operation needs,
//! uploads it to a virtual keyboard, and presses those keycodes.
//!
//! Keycodes run from evdev 1 upward; xkb numbers the same key eight
//! higher. An operation that needs more distinct keysyms than one
//! keymap holds is typed in several passes.

/// How many keysyms one generated keymap carries. Keycodes stop at the
/// xkb maximum of 255, which leaves evdev 1..=247.
pub const KEYS_PER_KEYMAP: usize = 240;

/// The xkb keysym name for one character. Control characters that a
/// keyboard reaches through a named key are mapped to that key; every
/// other character rides its Unicode keysym, the form `xkbcommon`
/// parses as `U` plus a hex codepoint.
pub fn char_keysym(c: char) -> String {
    match c {
        '\n' | '\r' => "Return".to_string(),
        '\t' => "Tab".to_string(),
        '\x08' => "BackSpace".to_string(),
        '\x1b' => "Escape".to_string(),
        _ => format!("U{:04X}", c as u32),
    }
}

/// The xkb keysym name for one key of a chord. The action vocabulary
/// uses short modifier aliases beside plain keysym names.
pub fn key_keysym(key: &str) -> String {
    match key {
        "ctrl" => "Control_L",
        "shift" => "Shift_L",
        "alt" => "Alt_L",
        "super" | "meta" => "Super_L",
        other => other,
    }
    .to_string()
}

/// The depressed-modifier mask a chord key contributes, or `None` when
/// the key is not a modifier.
pub fn modifier_mask(key: &str) -> Option<u32> {
    match key_keysym(key).as_str() {
        "Control_L" | "Control_R" => Some(crate::keymap::MOD_CTRL),
        "Shift_L" | "Shift_R" => Some(crate::keymap::MOD_SHIFT),
        "Alt_L" | "Alt_R" => Some(crate::keymap::MOD_ALT),
        "Super_L" | "Super_R" => Some(crate::keymap::MOD_SUPER),
        _ => None,
    }
}

/// One xkb keymap that carries these keysyms, in order. The nth keysym
/// answers to evdev keycode n+1.
pub fn keymap_for(keysyms: &[String]) -> String {
    let mut codes = String::new();
    let mut symbols = String::new();
    for (index, keysym) in keysyms.iter().enumerate() {
        let code = index + 9;
        codes.push_str(&format!("        <K{index}> = {code};\n"));
        symbols.push_str(&format!("        key <K{index}> {{[ {keysym} ]}};\n"));
    }
    format!(
        "xkb_keymap {{\n\
         \x20   xkb_keycodes \"pagis\" {{\n\
         \x20       minimum = 8;\n\
         \x20       maximum = 255;\n\
         {codes}\
         \x20   }};\n\
         \x20   xkb_types \"pagis\" {{ include \"complete\" }};\n\
         \x20   xkb_compatibility \"pagis\" {{ include \"complete\" }};\n\
         \x20   xkb_symbols \"pagis\" {{\n\
         {symbols}\
         \x20   }};\n\
         }};\n"
    )
}

/// Split a keysym sequence into passes that each fit one keymap, and
/// resolve every keysym to its evdev keycode inside its own pass.
/// Order is preserved: a pass ends only when it is full.
pub fn passes(keysyms: &[String]) -> Vec<(Vec<String>, Vec<u32>)> {
    let mut out: Vec<(Vec<String>, Vec<u32>)> = Vec::new();
    let mut table: Vec<String> = Vec::new();
    let mut codes: Vec<u32> = Vec::new();
    for keysym in keysyms {
        let existing = table.iter().position(|known| known == keysym);
        let index = match existing {
            Some(index) => index,
            None if table.len() < KEYS_PER_KEYMAP => {
                table.push(keysym.clone());
                table.len() - 1
            }
            None => {
                out.push((std::mem::take(&mut table), std::mem::take(&mut codes)));
                table.push(keysym.clone());
                0
            }
        };
        codes.push(index as u32 + 1);
    }
    if !table.is_empty() {
        out.push((table, codes));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_maps_to_its_unicode_keysym() {
        assert_eq!(char_keysym('a'), "U0061");
        assert_eq!(char_keysym('#'), "U0023");
        assert_eq!(char_keysym('é'), "U00E9");
        assert_eq!(char_keysym('\n'), "Return");
    }

    #[test]
    fn one_pass_reuses_a_keycode_for_a_repeated_keysym() {
        let keysyms: Vec<String> = "aba".chars().map(char_keysym).collect();
        let passes = passes(&keysyms);
        assert_eq!(passes.len(), 1);
        assert_eq!(passes[0].0, vec!["U0061", "U0062"]);
        assert_eq!(passes[0].1, vec![1, 2, 1]);
    }

    #[test]
    fn a_long_alphabet_splits_into_passes_that_keep_the_order() {
        let text: String = (0..KEYS_PER_KEYMAP as u32 + 5)
            .map(|index| char::from_u32(0x100 + index).expect("codepoint"))
            .collect();
        let keysyms: Vec<String> = text.chars().map(char_keysym).collect();
        let passes = passes(&keysyms);
        assert_eq!(passes.len(), 2);
        assert_eq!(passes[0].0.len(), KEYS_PER_KEYMAP);
        assert_eq!(passes[1].0.len(), 5);
        assert_eq!(passes[0].1.len() + passes[1].1.len(), keysyms.len());
    }

    #[test]
    fn the_generated_keymap_carries_one_key_per_keysym() {
        let keymap = keymap_for(&["U0061".to_string(), "Return".to_string()]);
        assert!(keymap.contains("<K0> = 9;"), "{keymap}");
        assert!(keymap.contains("<K1> = 10;"), "{keymap}");
        assert!(keymap.contains("key <K0> {[ U0061 ]};"), "{keymap}");
        assert!(keymap.contains("key <K1> {[ Return ]};"), "{keymap}");
    }

    #[test]
    fn a_modifier_alias_resolves_to_its_keysym_and_mask() {
        assert_eq!(key_keysym("ctrl"), "Control_L");
        assert_eq!(key_keysym("Return"), "Return");
        assert_eq!(modifier_mask("ctrl"), Some(crate::keymap::MOD_CTRL));
        assert_eq!(modifier_mask("Return"), None);
    }
}
