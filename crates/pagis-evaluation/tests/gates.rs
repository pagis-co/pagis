use pagis_evaluation::{fraction_gate, wilson_lower_bound};

#[test]
fn the_wilson_lower_bound_matches_the_worked_held_out_example() {
    let lower = wilson_lower_bound(15, 24, 1.645).expect("a nonempty sample");

    assert!((lower - 0.457_709_513_591_548_16).abs() < 1e-12);
}

#[test]
fn a_fraction_threshold_uses_the_wilson_lower_bound() {
    let gate = fraction_gate(22, 24, 0.8).expect("a nonempty sample");

    assert!((gate.point_estimate - 22.0 / 24.0).abs() < 1e-12);
    assert!(gate.lower_bound < 0.8);
    assert!(!gate.passed);
}

#[test]
fn a_one_threshold_keeps_exact_zero_tolerance() {
    assert!(fraction_gate(24, 24, 1.0).unwrap().passed);
    assert!(!fraction_gate(23, 24, 1.0).unwrap().passed);
}

#[test]
fn an_empty_fraction_has_no_gate_result() {
    assert!(fraction_gate(0, 0, 0.8).is_none());
}
