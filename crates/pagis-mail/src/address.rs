//! The local part of an Agent Mailbox address (ADR-0019).
//!
//! One function suggests a local part from the Agent's name, and one
//! reads back what the user typed. The two are separate on purpose: a
//! suggestion makes the best name it can out of anything, and what the
//! user types is refused when it is not an address.
//!
//! The address is the Agent's public identity, so the rules are the
//! plain ones a person expects: lowercase ASCII letters, digits, a dot
//! and a hyphen, and no more than 64 characters.

use deunicode::deunicode;

/// The local parts the mail host itself answers to. Nobody may hold
/// one, whatever the user types (ADR-0019).
pub const RESERVED_LOCAL_PARTS: [&str; 7] = [
    "abuse",
    "admin",
    "hostmaster",
    "noreply",
    "postmaster",
    "security",
    "webmaster",
];

/// The most characters a local part takes.
pub const MAX_LOCAL_PART: usize = 64;

/// Why what the user typed is not the name of a mailbox. Each message
/// is the one the creation form shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LocalPartError {
    #[error("an address needs a name before the @")]
    Empty,
    #[error("the name before the @ takes at most {MAX_LOCAL_PART} characters")]
    TooLong,
    #[error(
        "a name before the @ takes letters, digits, dots and hyphens, and starts and ends with a letter or a digit"
    )]
    Shape,
    #[error("the mail host keeps this name for itself. Choose another.")]
    Reserved,
}

/// The local part suggested from an Agent name. Letters that are not
/// ASCII are transliterated, every other character becomes a dot, and
/// runs of dots collapse to one. The answer is empty when the name
/// holds nothing an address can carry, and the caller then asks the
/// user for a name.
pub fn suggest_local_part(agent_name: &str) -> String {
    let folded = deunicode(agent_name).to_lowercase();
    let mut suggestion = String::with_capacity(folded.len());
    for character in folded.chars() {
        match character {
            'a'..='z' | '0'..='9' | '-' => suggestion.push(character),
            _ if suggestion.ends_with('.') => {}
            _ => suggestion.push('.'),
        }
    }
    trim_edges(&suggestion, MAX_LOCAL_PART)
}

/// The local part the user typed, in the one shape an address takes.
/// It trims and lowercases, and refuses everything else.
pub fn validate_local_part(value: &str) -> Result<String, LocalPartError> {
    let local_part = value.trim().to_lowercase();
    if local_part.is_empty() {
        return Err(LocalPartError::Empty);
    }
    if local_part.chars().count() > MAX_LOCAL_PART {
        return Err(LocalPartError::TooLong);
    }
    let shaped = local_part
        .chars()
        .all(|character| matches!(character, 'a'..='z' | '0'..='9' | '.' | '-'))
        && local_part.starts_with(|character: char| character.is_ascii_alphanumeric())
        && local_part.ends_with(|character: char| character.is_ascii_alphanumeric())
        && !local_part.contains("..");
    if !shaped {
        return Err(LocalPartError::Shape);
    }
    if RESERVED_LOCAL_PARTS.contains(&local_part.as_str()) {
        return Err(LocalPartError::Reserved);
    }
    Ok(local_part)
}

/// The same local part with the ledger collision suffix on it: `2` for
/// the second holder of the name, `3` for the third. The suffix is
/// part of the 64 characters, so a long name loses its tail rather
/// than its suffix.
pub fn with_suffix(local_part: &str, suffix: u32) -> String {
    let suffix = suffix.to_string();
    let room = MAX_LOCAL_PART.saturating_sub(suffix.len());
    format!("{}{suffix}", trim_edges(local_part, room))
}

/// One mailbox address, lowercased. The address is stored, compared
/// and shown in this one shape.
pub fn mailbox_address(local_part: &str, domain: &str) -> String {
    format!("{}@{}", local_part.to_lowercase(), domain.to_lowercase())
}

/// The domain half of an address, when it has one.
pub fn address_domain(address: &str) -> Option<&str> {
    address.rsplit_once('@').map(|(_, domain)| domain)
}

/// The first `room` characters, with no dot or hyphen left at either
/// end. A truncated name must still read as a name.
fn trim_edges(local_part: &str, room: usize) -> String {
    local_part
        .chars()
        .take(room)
        .collect::<String>()
        .trim_matches(['.', '-'])
        .to_string()
}
