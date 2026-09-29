//! RFC 4733 telephone events on the RTP leg. Both the reader and the
//! writer are hand-written (ADR-0020): the payload is four bytes, and
//! Pagis needs the sixteen keypad events and nothing more.
//!
//! One key press is a burst of packets with one timestamp: start packets
//! with a growing duration, then three end packets that repeat the final
//! duration, so one lost packet does not lose the press.

/// The number of end packets a press sends, as RFC 4733 recommends.
pub const END_PACKETS: usize = 3;
/// How long one press lasts on the wire. Carriers want at least 80 ms;
/// five frames is comfortable and still short.
pub const PRESS_FRAMES: u16 = 5;
/// The volume field of every event Pagis sends, in -dBm0.
const VOLUME: u8 = 10;

/// One telephone-event payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    /// The event code: `0`-`9`, `*` is 10, `#` is 11, `A`-`D` are 12-15.
    pub code: u8,
    /// Set on the end packets of a press.
    pub end: bool,
    /// How long the press has lasted, in timestamp units.
    pub duration: u16,
}

pub fn encode(event: &Event) -> [u8; 4] {
    let flags = if event.end { 0x80 } else { 0 } | (VOLUME & 0x3F);
    let [high, low] = event.duration.to_be_bytes();
    [event.code, flags, high, low]
}

/// `None` when the payload is not a telephone event.
pub fn decode(payload: &[u8]) -> Option<Event> {
    let [code, flags, high, low, ..] = payload else {
        return None;
    };
    Some(Event {
        code: *code,
        end: flags & 0x80 != 0,
        duration: u16::from_be_bytes([*high, *low]),
    })
}

/// The event code of one keypad character, or `None` for a character
/// that has no key.
pub fn digit_to_code(digit: char) -> Option<u8> {
    match digit {
        '0'..='9' => Some(digit as u8 - b'0'),
        '*' => Some(10),
        '#' => Some(11),
        'A'..='D' => Some(digit as u8 - b'A' + 12),
        _ => None,
    }
}

pub fn digit_from_code(code: u8) -> Option<char> {
    match code {
        0..=9 => Some((b'0' + code) as char),
        10 => Some('*'),
        11 => Some('#'),
        12..=15 => Some((b'A' + code - 12) as char),
        _ => None,
    }
}
