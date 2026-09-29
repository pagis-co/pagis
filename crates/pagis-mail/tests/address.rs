//! The local part rules of an Agent Mailbox address (ADR-0019):
//! what the form suggests from an Agent name, what it accepts from the
//! user, and the names the mail host keeps for itself.

use pagis_mail::{
    LocalPartError, MAX_LOCAL_PART, RESERVED_LOCAL_PARTS, mailbox_address, suggest_local_part,
    validate_local_part, with_suffix,
};

#[test]
fn a_local_part_is_suggested_from_the_agent_name() {
    // The derivation table of ADR-0019: lowercase ASCII letters,
    // digits, a dot and a hyphen; other characters become one dot; a
    // name that is not ASCII is transliterated.
    let table = [
        ("Ada", "ada"),
        ("Ada Lovelace", "ada.lovelace"),
        ("ADA LOVELACE", "ada.lovelace"),
        ("Ada-Lovelace", "ada-lovelace"),
        ("Ada  Lovelace", "ada.lovelace"),
        ("Ada   ...   Lovelace", "ada.lovelace"),
        ("  Ada Lovelace  ", "ada.lovelace"),
        ("Ada Lovelace!!!", "ada.lovelace"),
        ("Agent 47", "agent.47"),
        ("José Núñez", "jose.nunez"),
        ("Ångström", "angstrom"),
        ("Ада", "ada"),
        ("Ada (billing)", "ada.billing"),
        ("....", ""),
        ("", ""),
    ];

    for (agent_name, expected) in table {
        assert_eq!(
            suggest_local_part(agent_name),
            expected,
            "the suggestion for {agent_name}"
        );
    }
}

#[test]
fn a_suggestion_never_passes_the_length_limit() {
    let suggestion = suggest_local_part(&"a".repeat(200));

    assert_eq!(suggestion.chars().count(), MAX_LOCAL_PART);
}

#[test]
fn a_suggestion_is_a_name_the_form_accepts() {
    for agent_name in ["Ada Lovelace", "José Núñez", "Agent 47", "Ada-Lovelace"] {
        let suggestion = suggest_local_part(agent_name);

        assert_eq!(
            validate_local_part(&suggestion),
            Ok(suggestion.clone()),
            "the form refused its own suggestion for {agent_name}"
        );
    }
}

#[test]
fn the_form_trims_and_lowercases_what_the_user_types() {
    assert_eq!(
        validate_local_part("  Ada.Lovelace "),
        Ok("ada.lovelace".to_string())
    );
}

#[test]
fn the_form_refuses_a_name_that_is_not_an_address() {
    let table = [
        ("", LocalPartError::Empty),
        ("   ", LocalPartError::Empty),
        ("ada lovelace", LocalPartError::Shape),
        ("ada@example.com", LocalPartError::Shape),
        ("ada_lovelace", LocalPartError::Shape),
        (".ada", LocalPartError::Shape),
        ("ada.", LocalPartError::Shape),
        ("-ada", LocalPartError::Shape),
        ("ada-", LocalPartError::Shape),
        ("ada..lovelace", LocalPartError::Shape),
        ("josé", LocalPartError::Shape),
        (&"a".repeat(MAX_LOCAL_PART + 1), LocalPartError::TooLong),
    ];

    for (typed, expected) in table {
        assert_eq!(validate_local_part(typed), Err(expected), "typing {typed}");
    }
}

#[test]
fn the_mail_host_keeps_its_own_names() {
    for reserved in RESERVED_LOCAL_PARTS {
        assert_eq!(
            validate_local_part(reserved),
            Err(LocalPartError::Reserved),
            "{reserved} is the mail host's own name"
        );
        // The refusal holds whatever the user types (ADR-0019).
        assert_eq!(
            validate_local_part(&reserved.to_uppercase()),
            Err(LocalPartError::Reserved)
        );
    }
}

#[test]
fn a_ledger_collision_appends_a_number() {
    assert_eq!(with_suffix("ada", 2), "ada2");
    assert_eq!(with_suffix("ada", 3), "ada3");
    assert_eq!(with_suffix("ada.lovelace", 10), "ada.lovelace10");
}

#[test]
fn a_suffix_shortens_the_name_rather_than_itself() {
    let long = "a".repeat(MAX_LOCAL_PART);

    let suffixed = with_suffix(&long, 2);

    assert_eq!(suffixed.chars().count(), MAX_LOCAL_PART);
    assert!(suffixed.ends_with('2'));
}

#[test]
fn a_suffixed_name_never_ends_in_a_dot() {
    // The truncation must not leave a dot beside the number.
    let name = format!("{}.", "a".repeat(MAX_LOCAL_PART - 2));

    assert_eq!(
        with_suffix(&name, 2),
        format!("{}2", "a".repeat(MAX_LOCAL_PART - 2))
    );
}

#[test]
fn an_address_is_one_shape_everywhere() {
    assert_eq!(mailbox_address("Ada", "Example.COM"), "ada@example.com");
}
