//! The sender check of inbound mail (ADR-0019). Only the one
//! Authentication-Results header with the Mailbox Provider's
//! authserv-id counts (RFC 8601), only a DMARC pass that aligns with
//! the `From` domain verifies the sender (RFC 7489), and a message with
//! more than one such header or more than one author is Unknown.
//!
//! Every message reads from its bytes through the real MIME reader.

use pagis_mail::fake::raw_message;
use pagis_mail::{MessageId, SenderVerification, read_mime, verify_sender};

/// The authserv-ids of one Mailbox Provider, as a test names them.
const PROVIDER: &[&str] = &["aspmx1.migadu.com", "aspmx2.migadu.com"];

/// The shape a Migadu exchanger writes, folded over four lines, with a
/// DMARC verdict for one `From` domain.
fn migadu(authserv_id: &str, dmarc: &str, domain: &str) -> String {
    format!(
        "{authserv_id};\r\n\
         \tdkim=pass header.d={domain} header.s=mail2 header.b=oeLExxHN;\r\n\
         \tdmarc={dmarc} (policy=quarantine) header.from={domain};\r\n\
         \tspf=pass ({authserv_id}: domain of alert@{domain} designates 192.0.2.150 \
         as permitted sender) smtp.mailfrom=alert@{domain}"
    )
}

/// The check of one message on a host with these authserv-ids.
fn check_on(authserv_ids: &[&str], from: &str, results: &[String]) -> SenderVerification {
    let results: Vec<&str> = results.iter().map(String::as_str).collect();
    check_raw(authserv_ids, &raw_message(from, "Hello", &results))
}

/// The check of one message as its bytes read.
fn check_raw(authserv_ids: &[&str], raw: &[u8]) -> SenderVerification {
    let message = read_mime(raw, MessageId::new("INBOX", 1)).expect("the message reads");
    verify_sender(authserv_ids, &message.summary)
}

fn check(from: &str, results: &[String]) -> SenderVerification {
    check_on(PROVIDER, from, results)
}

#[test]
fn an_aligned_dmarc_pass_from_the_provider_verifies_the_sender() {
    let verification = check(
        "alert@example.org",
        &[migadu("aspmx1.migadu.com", "pass", "example.org")],
    );
    assert_eq!(verification, SenderVerification::AlignedDmarcPass);
    assert!(verification.is_verified());
    assert_eq!(verification.as_str(), "aligned_dmarc_pass");
}

#[test]
fn each_authserv_id_of_the_provider_counts() {
    assert_eq!(
        check(
            "alert@example.org",
            &[migadu("aspmx2.migadu.com", "pass", "example.org")],
        ),
        SenderVerification::AlignedDmarcPass
    );
    // An authserv-id is a host name, so its case does not matter.
    assert_eq!(
        check(
            "alert@example.org",
            &[migadu("ASPMX1.Migadu.COM", "pass", "example.org")],
        ),
        SenderVerification::AlignedDmarcPass
    );
}

#[test]
fn a_host_with_no_known_authserv_id_verifies_nobody() {
    let verification = check_on(
        &[],
        "alert@example.org",
        &[migadu("aspmx1.migadu.com", "pass", "example.org")],
    );
    assert_eq!(verification, SenderVerification::ManualHost);
    assert!(!verification.is_verified());
    assert_eq!(verification.as_str(), "manual_host");
}

#[test]
fn a_message_with_no_result_from_the_provider_has_no_result() {
    assert_eq!(
        check("alert@example.org", &[]),
        SenderVerification::NoResult
    );
    // A result under another authserv-id can come from any host on the
    // way, or from the sender.
    let foreign = check(
        "alert@example.org",
        &[
            migadu("mx.example.org", "pass", "example.org"),
            migadu("aspmx1.migadu.com.example.org", "pass", "example.org"),
        ],
    );
    assert_eq!(foreign, SenderVerification::NoResult);
    assert_eq!(foreign.as_str(), "no_result");
}

#[test]
fn more_than_one_result_of_the_provider_is_unknown() {
    // A host can write its own copy below the headers and keep the copy
    // the sender wrote above them, so the position of a copy proves
    // nothing, whichever copy passes.
    for results in [
        [
            migadu("aspmx1.migadu.com", "fail", "example.org"),
            migadu("aspmx1.migadu.com", "pass", "example.org"),
        ],
        [
            migadu("aspmx1.migadu.com", "pass", "example.org"),
            migadu("aspmx1.migadu.com", "fail", "example.org"),
        ],
        [
            migadu("aspmx1.migadu.com", "pass", "example.org"),
            migadu("aspmx1.migadu.com", "pass", "example.org"),
        ],
        // The two exchangers are one provider.
        [
            migadu("aspmx1.migadu.com", "pass", "example.org"),
            migadu("aspmx2.migadu.com", "pass", "example.org"),
        ],
    ] {
        let verification = check("alert@example.org", &results);
        assert_eq!(verification, SenderVerification::SeveralResults);
        assert!(!verification.is_verified());
        assert_eq!(verification.as_str(), "several_results");
    }
    // A result under another authserv-id is not a second result.
    assert_eq!(
        check(
            "alert@example.org",
            &[
                migadu("mx.example.org", "fail", "example.org"),
                migadu("aspmx1.migadu.com", "pass", "example.org"),
                migadu("mx.example.org", "pass", "example.org"),
            ],
        ),
        SenderVerification::AlignedDmarcPass
    );
}

#[test]
fn a_message_with_more_than_one_author_is_unknown() {
    let pass = [migadu("aspmx1.migadu.com", "pass", "example.org")];
    // One `From` header with two mailboxes, and one with a group.
    for from in [
        "alert@example.org, boss@example.org",
        "Team: alert@example.org, boss@example.org;",
    ] {
        let verification = check(from, &pass);
        assert_eq!(verification, SenderVerification::SeveralAuthors, "{from}");
        assert!(!verification.is_verified());
        assert_eq!(verification.as_str(), "several_authors");
    }
    // Two `From` headers.
    let raw = format!(
        "Authentication-Results: {}\r\n\
         From: alert@example.org\r\n\
         From: boss@example.org\r\n\
         Subject: Hello\r\n\
         \r\n\
         Hello.\r\n",
        pass[0]
    );
    assert_eq!(
        check_raw(PROVIDER, raw.as_bytes()),
        SenderVerification::SeveralAuthors
    );
}

#[test]
fn a_dmarc_result_other_than_pass_does_not_verify() {
    for dmarc in [
        "fail",
        "none",
        "temperror",
        "permerror",
        "policy",
        "bestguesspass",
    ] {
        let verification = check(
            "alert@example.org",
            &[migadu("aspmx1.migadu.com", dmarc, "example.org")],
        );
        assert_eq!(verification, SenderVerification::NoDmarcPass, "{dmarc}");
        assert_eq!(verification.as_str(), "no_dmarc_pass");
    }
    // A result with no DMARC method: SPF and DKIM alone do not bind the
    // `From` domain.
    assert_eq!(
        check(
            "alert@example.org",
            &["aspmx1.migadu.com; dkim=pass header.d=example.org; \
               spf=pass smtp.mailfrom=alert@example.org"
                .to_string()],
        ),
        SenderVerification::NoDmarcPass
    );
    // The host checked nothing.
    assert_eq!(
        check(
            "alert@example.org",
            &["aspmx1.migadu.com; none".to_string()]
        ),
        SenderVerification::NoDmarcPass
    );
}

#[test]
fn every_dmarc_result_in_the_header_must_pass() {
    assert_eq!(
        check(
            "alert@example.org",
            &["aspmx1.migadu.com; dmarc=pass header.from=example.org; \
               dmarc=fail header.from=example.org"
                .to_string()],
        ),
        SenderVerification::NoDmarcPass
    );
}

#[test]
fn words_in_a_comment_or_a_quoted_string_are_not_a_result() {
    for value in [
        "aspmx1.migadu.com; dmarc=fail (dmarc=pass header.from=example.org) \
         header.from=example.org",
        "aspmx1.migadu.com; dmarc=fail reason=\"x; dmarc=pass header.from=example.org\" \
         header.from=example.org",
        "aspmx1.migadu.com; dmarc=fail (a (nested; dmarc=pass) comment) \
         header.from=example.org",
    ] {
        assert_eq!(
            check("alert@example.org", &[value.to_string()]),
            SenderVerification::NoDmarcPass,
            "{value}"
        );
    }
    // A comment, nested or with an escaped parenthesis, hides nothing
    // that is outside it.
    assert_eq!(
        check(
            "alert@example.org",
            &["aspmx1.migadu.com; dmarc=pass (a (b) \\) c) header.from=example.org".to_string()],
        ),
        SenderVerification::AlignedDmarcPass
    );
}

#[test]
fn a_comment_can_hold_the_authserv_id_without_being_it() {
    assert_eq!(
        check(
            "alert@example.org",
            &[
                "(aspmx1.migadu.com) mx.example.org; dmarc=pass header.from=example.org"
                    .to_string()
            ],
        ),
        SenderVerification::NoResult
    );
}

#[test]
fn the_reader_takes_a_version_case_and_space_around_the_signs() {
    assert_eq!(
        check(
            "Alert <alert@example.org>",
            &["ASPMX1.migadu.com 1 ;\r\n DMARC = Pass Header . From = Example.ORG".to_string()],
        ),
        SenderVerification::AlignedDmarcPass
    );
    // A quoted property value reads as the value.
    assert_eq!(
        check(
            "alert@example.org",
            &["aspmx1.migadu.com; dmarc=pass header.from=\"example.org\"".to_string()],
        ),
        SenderVerification::AlignedDmarcPass
    );
}

#[test]
fn alignment_is_relaxed_to_the_organizational_domain() {
    let aligned =
        |from: &str, domain: &str| check(from, &[migadu("aspmx1.migadu.com", "pass", domain)]);
    // A subdomain on either side shares the organizational domain.
    assert_eq!(
        aligned("alert@mail.example.co.uk", "example.co.uk"),
        SenderVerification::AlignedDmarcPass
    );
    assert_eq!(
        aligned("alert@example.co.uk", "news.example.co.uk"),
        SenderVerification::AlignedDmarcPass
    );
    // A shared public suffix is not a shared organization.
    let other = aligned("alert@example.co.uk", "other.co.uk");
    assert_eq!(other, SenderVerification::NotAligned);
    assert_eq!(other.as_str(), "not_aligned");
    // A forwarder passes DMARC for its own domain, not the sender's.
    assert_eq!(
        aligned("boss@home.test", "forwarder.test"),
        SenderVerification::NotAligned
    );
}

#[test]
fn a_pass_that_names_no_aligned_from_domain_does_not_verify() {
    assert_eq!(
        check(
            "alert@example.org",
            &["aspmx1.migadu.com; dmarc=pass".to_string()],
        ),
        SenderVerification::NotAligned
    );
    // A `From` with no address has no domain to align with.
    assert_eq!(
        check(
            "undisclosed-recipients",
            &[migadu("aspmx1.migadu.com", "pass", "example.org")],
        ),
        SenderVerification::NotAligned
    );
}
