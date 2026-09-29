//! The output cap: a head, a tail, and a count of the
//! bytes between them.

use pagis_computer::{CappedOutput, OutputCap};

const CAP: OutputCap = OutputCap { head: 8, tail: 4 };

#[test]
fn output_under_the_cap_passes_through_whole() {
    let mut output = CappedOutput::new(CAP);
    output.push(b"hello");

    assert!(!output.truncated());
    assert_eq!(output.into_text(), "hello");
}

#[test]
fn output_over_the_cap_keeps_the_head_the_tail_and_a_marker() {
    let mut output = CappedOutput::new(CAP);
    output.push(b"0123456789");
    output.push(b"abcdefghij");

    assert!(output.truncated());
    assert_eq!(
        output.into_text(),
        "01234567\n[... 8 bytes truncated ...]\nghij"
    );
}

#[test]
fn many_small_writes_cap_the_same_as_one_large_write() {
    let mut split = CappedOutput::new(CAP);
    for byte in b"0123456789abcdefghij" {
        split.push(&[*byte]);
    }
    let mut whole = CappedOutput::new(CAP);
    whole.push(b"0123456789abcdefghij");

    assert_eq!(split.into_text(), whole.into_text());
}
