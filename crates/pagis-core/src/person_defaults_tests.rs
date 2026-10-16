use crate::AddressRule;

#[test]
fn address_rule_uses_name_parts_and_lower_case() {
    assert_eq!(
        AddressRule::FirstName.local_part(" Ada Lovelace ").unwrap(),
        "ada"
    );
    assert_eq!(
        AddressRule::FirstLast
            .local_part("Ada Byron Lovelace")
            .unwrap(),
        "ada.lovelace"
    );
    assert_eq!(
        AddressRule::FirstInitialLast
            .local_part("Ada Lovelace")
            .unwrap(),
        "alovelace"
    );
}

#[test]
fn address_rule_rejects_missing_parts_and_unsupported_names() {
    assert!(AddressRule::FirstLast.local_part("Ada").is_err());
    assert!(AddressRule::FirstName.local_part("  ").is_err());
    assert!(AddressRule::FirstName.local_part("李").is_err());
    assert_eq!(
        AddressRule::FirstLast
            .local_part("Anne-Marie O'Neil")
            .unwrap(),
        "anne-marie.oneil"
    );
}

#[test]
fn address_rule_has_a_small_closed_contract() {
    assert_eq!(
        serde_json::to_string(&AddressRule::FirstLast).unwrap(),
        "\"first_last\""
    );
    assert!(serde_json::from_str::<AddressRule>("\"{name}\"").is_err());
}
