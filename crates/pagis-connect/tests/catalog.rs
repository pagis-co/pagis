//! The Provider Catalog (ADR-0012): the list the picker draws,
//! and the parser that turns a declared form into `NewCredentials`.

use std::collections::BTreeMap;

use pagis_connect::{
    ConnectError, FieldKind, NewCredentials, ProviderEntry, ProviderKind, SetupKind, TELEPHONY,
    TEXTING, absent_capabilities, catalog, entry, installation_setup, installation_setups,
    is_installation_provider, person_catalog,
};

/// The default SIP server of a carrier, which its SIP part declares.
fn sip_default(entry: &ProviderEntry) -> Option<&'static str> {
    entry
        .installation
        .iter()
        .find(|part| part.kind == SetupKind::SipCredential)
        .expect("a carrier declares its SIP sign-in")
        .fields
        .iter()
        .find(|field| field.key == "domain")
        .expect("the SIP sign-in names its server")
        .default
}

fn fields(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[test]
fn the_catalog_lists_every_provider_the_connector_accepts() {
    let ids: Vec<&str> = catalog().iter().map(|entry| entry.id).collect();
    assert_eq!(
        ids,
        vec!["google", "telnyx", "twilio", "plivo", "migadu", "manual"]
    );
}

#[test]
fn an_oauth_entry_and_a_fields_entry_differ_in_kind_alone() {
    assert_eq!(entry("google").unwrap().kind, ProviderKind::Oauth);
    for id in ["telnyx", "twilio", "plivo", "migadu", "manual"] {
        assert_eq!(entry(id).unwrap().kind, ProviderKind::Fields, "{id}");
    }
}

#[test]
fn a_carrier_is_limited_to_one_instance_and_the_rest_are_not() {
    for id in ["telnyx", "twilio", "plivo"] {
        assert_eq!(entry(id).unwrap().max_instances, Some(1), "{id}");
    }
    for id in ["google", "migadu", "manual"] {
        assert_eq!(entry(id).unwrap().max_instances, None, "{id}");
    }
}

#[test]
fn every_entry_declares_a_capability_and_copy() {
    for entry in catalog() {
        assert!(!entry.capabilities.is_empty(), "{}", entry.id);
        assert!(!entry.label.is_empty(), "{}", entry.id);
        assert!(!entry.blurb.is_empty(), "{}", entry.id);
        assert!(!entry.default_display_name.is_empty(), "{}", entry.id);
    }
}

#[test]
fn the_carrier_entry_carries_the_portal_and_the_registrar() {
    let carrier = entry("telnyx").unwrap();
    assert_eq!(carrier.capabilities, &[TELEPHONY, TEXTING]);
    assert_eq!(sip_default(carrier), Some("sip.telnyx.com"));
    assert!(carrier.portal.is_some());

    // A Twilio SIP Domain is the user's own, so the entry names no
    // registrar and the user types it with the SIP credential.
    let twilio = entry("twilio").unwrap();
    assert_eq!(twilio.capabilities, &[TELEPHONY, TEXTING]);
    assert_eq!(sip_default(twilio), None);
    assert_eq!(twilio.portal, Some("the Twilio console"));
    // The blurb warns about the media path before a number is bought.
    assert!(twilio.blurb.contains("NAT"), "{}", twilio.blurb);
}

#[test]
fn a_secret_field_is_marked_and_a_port_is_a_number_with_a_default() {
    let manual = entry("manual").unwrap();
    let imap_port = manual
        .fields
        .iter()
        .find(|field| field.key == "imap_port")
        .unwrap();
    assert_eq!(imap_port.kind, FieldKind::Number);
    assert_eq!(imap_port.default, Some("993"));
    let migadu = entry("migadu").unwrap();
    let api_key = migadu
        .fields
        .iter()
        .find(|field| field.key == "api_key")
        .unwrap();
    assert!(api_key.secret);
    assert!(
        !migadu
            .fields
            .iter()
            .any(|field| field.key == "domain" && field.secret)
    );
}

#[test]
fn the_declared_fields_are_what_the_parser_reads() {
    for entry in catalog() {
        let values: BTreeMap<String, String> = entry
            .fields
            .iter()
            .map(|field| {
                let value = match field.kind {
                    FieldKind::Number => "993",
                    FieldKind::Text => "value.example.com",
                };
                (field.key.to_string(), value.to_string())
            })
            .collect();
        let parsed = NewCredentials::from_fields(entry.id, &values);
        assert!(parsed.is_ok(), "{}: {:?}", entry.id, parsed.err());
        assert_eq!(parsed.unwrap().provider(), entry.id);
    }
}

#[test]
fn a_missing_field_is_refused_by_name() {
    let refused = NewCredentials::from_fields("migadu", &fields(&[("domain", "example.com")]));
    match refused {
        Err(ConnectError::Validation(message)) => assert!(message.contains("account"), "{message}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn a_port_that_is_not_a_number_is_refused_by_name() {
    let refused = NewCredentials::from_fields(
        "manual",
        &fields(&[
            ("domain", "example.com"),
            ("imap_host", "imap.example.com"),
            ("imap_port", "many"),
            ("smtp_host", "smtp.example.com"),
            ("smtp_port", "465"),
        ]),
    );
    match refused {
        Err(ConnectError::Validation(message)) => {
            assert!(message.contains("imap_port"), "{message}")
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn an_unknown_provider_is_refused() {
    assert!(matches!(
        NewCredentials::from_fields("fax", &fields(&[])),
        Err(ConnectError::Validation(_))
    ));
    assert!(entry("fax").is_none());
}

/// Plivo needs an answer URL it can reach for every call, and the
/// default install has no public URL. The entry says so before a
/// number is bought (ADR-0020).
#[test]
fn the_plivo_entry_names_the_public_endpoint_its_calls_need() {
    let plivo = entry("plivo").unwrap();
    assert_eq!(plivo.capabilities, &[TELEPHONY]);
    assert_eq!(sip_default(plivo), Some("phone.plivo.com"));
    assert_eq!(plivo.portal, Some("the Plivo console"));
    assert!(
        plivo.blurb.contains("public HTTPS endpoint"),
        "{}",
        plivo.blurb
    );
    let token = plivo
        .fields
        .iter()
        .find(|field| field.key == "auth_token")
        .unwrap();
    assert!(token.secret);
    assert!(
        !plivo
            .fields
            .iter()
            .any(|field| field.key == "auth_id" && field.secret)
    );
}

/// Plivo never returns the body of an inbound text, so Pagis does not
/// text on Plivo at all (ADR-0020). The entry states it, so the desk
/// reads it before a number is bought.
#[test]
fn the_plivo_entry_declares_texting_absent_and_the_others_carry_it() {
    let plivo = entry("plivo").unwrap();
    assert!(!plivo.capabilities.contains(&TEXTING));
    assert_eq!(plivo.absent_capabilities, &[TEXTING]);
    assert_eq!(absent_capabilities("plivo"), vec![TEXTING.to_string()]);

    for id in ["telnyx", "twilio"] {
        let carrier = entry(id).unwrap();
        assert!(carrier.capabilities.contains(&TEXTING), "{id}");
        assert!(carrier.absent_capabilities.is_empty(), "{id}");
    }

    // Only a carrier states an absent capability.
    for id in ["google", "migadu", "manual"] {
        assert!(entry(id).unwrap().absent_capabilities.is_empty(), "{id}");
    }
}

/// A person types nothing to connect Google: no client id, no client
/// secret and no account. Google's own account chooser and consent
/// screen are the whole form (ADR-0012).
#[test]
fn the_google_entry_asks_for_nothing() {
    let google = entry("google").unwrap();
    assert!(google.fields.is_empty(), "{:?}", google.fields);
    assert_eq!(google.portal, None);
    assert!(
        google.blurb.contains("Sign in at Google"),
        "{}",
        google.blurb
    );
}

/// The Google entry works only where an Administrator set up the
/// Installation OAuth Client, and it says who does that.
#[test]
fn the_google_entry_says_whether_the_installation_set_it_up() {
    let google = |client: bool| {
        person_catalog(client)
            .into_iter()
            .find(|provider| provider.entry.id == "google")
            .expect("the google entry")
    };

    let ready = google(true);
    assert!(ready.set_up);
    assert_eq!(ready.entry, *entry("google").unwrap());

    let waiting = google(false);
    assert!(!waiting.set_up);
    assert!(waiting.entry.fields.is_empty());
    assert!(
        waiting.entry.blurb.contains("Administration Interface"),
        "{}",
        waiting.entry.blurb
    );
}

/// A person picks only what a person connects on their own. The
/// carrier account and the mail domain are the installation's, and an
/// administrator sets them up in the Administration Interface.
#[test]
fn the_person_catalog_holds_no_installation_connection() {
    for client in [false, true] {
        let ids: Vec<&str> = person_catalog(client)
            .iter()
            .map(|provider| provider.entry.id)
            .collect();
        assert_eq!(ids, vec!["google"], "client: {client}");
    }
    for id in ["telnyx", "twilio", "plivo", "migadu", "manual"] {
        assert!(is_installation_provider(id), "{id}");
    }
    assert!(!is_installation_provider("google"));
}

/// Google expires a refresh token after seven days while the consent
/// screen of its client is in Testing. The Administrator reads that
/// before they register the client.
#[test]
fn the_oauth_client_part_tells_the_administrator_to_publish_the_consent_screen() {
    let setup = installation_setup("google").unwrap();
    let blurb = setup.parts[0].blurb;
    assert!(blurb.contains("Testing"), "{blurb}");
    assert!(blurb.contains("Internal"), "{blurb}");
    assert!(!blurb.contains("Desktop"), "{blurb}");
}

/// Every provider declares its installation parts in one place, and
/// the Administration Interface sets up each of them: the model keys,
/// the Installation OAuth Client, the carrier accounts with their SIP
/// sign-in, and the mail domains.
#[test]
fn every_provider_declares_what_the_installation_sets_up() {
    let parts = |provider: &str| -> Vec<(&'static str, SetupKind)> {
        installation_setup(provider)
            .unwrap_or_else(|| panic!("{provider} has no installation setup"))
            .parts
            .iter()
            .map(|part| (part.id, part.kind))
            .collect()
    };
    for provider in [
        "anthropic",
        "openai",
        "openrouter",
        "deepgram",
        "elevenlabs",
    ] {
        assert_eq!(parts(provider), vec![("key", SetupKind::ModelKey)]);
    }
    assert_eq!(
        parts("google"),
        vec![("oauth-client", SetupKind::OauthClient)]
    );
    for provider in ["telnyx", "twilio", "plivo"] {
        assert_eq!(
            parts(provider),
            vec![
                ("connection", SetupKind::Connection),
                ("sip", SetupKind::SipCredential)
            ]
        );
    }
    for provider in ["migadu", "manual"] {
        assert_eq!(parts(provider), vec![("connection", SetupKind::Connection)]);
    }
    assert!(installation_setup("fax").is_none());

    let groups: Vec<(&str, &str)> = installation_setups()
        .iter()
        .map(|setup| (setup.provider, setup.group))
        .collect();
    assert_eq!(
        groups,
        vec![
            ("anthropic", "models"),
            ("openai", "models"),
            ("openrouter", "models"),
            ("deepgram", "models"),
            ("elevenlabs", "models"),
            ("google", "accounts"),
            ("telnyx", "telephony"),
            ("twilio", "telephony"),
            ("plivo", "telephony"),
            ("migadu", "mailboxes"),
            ("manual", "mailboxes"),
        ]
    );
}

/// Only a Connection proves its credential again on a test. A part of
/// every kind carries copy and at least one field.
#[test]
fn a_setup_part_says_what_it_is_and_whether_it_can_be_tested() {
    for setup in installation_setups() {
        for part in setup.parts {
            assert!(!part.label.is_empty(), "{}/{}", setup.provider, part.id);
            assert!(!part.blurb.is_empty(), "{}/{}", setup.provider, part.id);
            assert!(!part.fields.is_empty(), "{}/{}", setup.provider, part.id);
            assert_eq!(
                part.kind.testable(),
                part.kind == SetupKind::Connection,
                "{}/{}",
                setup.provider,
                part.id
            );
        }
    }
}
