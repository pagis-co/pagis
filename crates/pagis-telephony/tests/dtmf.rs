//! RFC 4733 telephone events: the daemon writes and reads them by hand
//! (ADR-0020), so both halves are tested against the wire format.

use pagis_telephony::dtmf::{Event, decode, digit_from_code, digit_to_code, encode};

#[test]
fn every_keypad_digit_has_one_event_code() {
    for (digit, code) in [
        ('0', 0),
        ('9', 9),
        ('*', 10),
        ('#', 11),
        ('A', 12),
        ('D', 15),
    ] {
        assert_eq!(digit_to_code(digit), Some(code));
        assert_eq!(digit_from_code(code), Some(digit));
    }
    assert_eq!(digit_to_code('x'), None);
    assert_eq!(digit_from_code(16), None);
}

#[test]
fn an_event_round_trips_through_the_four_wire_bytes() {
    let event = Event {
        code: 5,
        end: true,
        duration: 800,
    };
    let bytes = encode(&event);
    assert_eq!(bytes, [5, 0x8A, 0x03, 0x20]);
    assert_eq!(decode(&bytes), Some(event));
}

#[test]
fn a_start_packet_has_the_end_bit_clear() {
    let bytes = encode(&Event {
        code: 11,
        end: false,
        duration: 160,
    });
    assert_eq!(bytes[1] & 0x80, 0);
    assert!(!decode(&bytes).unwrap().end);
}

#[test]
fn a_short_payload_is_not_an_event() {
    assert_eq!(decode(&[1, 2, 3]), None);
}
