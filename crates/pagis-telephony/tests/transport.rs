//! The declared capability set (ADR-0020, ADR-0005): the fake declares
//! what a test gives it, and Telnyx over SIP declares what the pure-SIP
//! path has and has not. The SIP transport reads the dialed number of
//! an inbound `INVITE`, and a line refuses a call with a SIP status.

use std::sync::Arc;

use pagis_telephony::fake::{FakeCallTransport, TokioClock};
use pagis_telephony::{
    CallTransport, Refusal, SipCallTransport, TransportCapabilities, dialed_e164, register_contact,
};

#[test]
fn telnyx_over_sip_declares_amd_absent_and_needs_no_ingress() {
    let capabilities = SipCallTransport::new().unwrap().capabilities();

    assert_eq!(
        capabilities,
        TransportCapabilities {
            send_dtmf: true,
            answering_machine_detection: false,
            wideband_audio: false,
            public_ingress_required: false,
        }
    );
}

#[test]
fn the_fake_declares_the_capabilities_it_is_given() {
    let transport = FakeCallTransport::new(Arc::new(TokioClock));
    assert!(!transport.capabilities().answering_machine_detection);

    let with_amd = TransportCapabilities {
        answering_machine_detection: true,
        ..transport.capabilities()
    };
    transport.set_capabilities(with_amd);

    assert_eq!(transport.capabilities(), with_amd);
}

/// Telnyx routes an inbound INVITE to the registered Contact by the
/// transport it names. A Contact without `transport=tls` is reached
/// over UDP, which a line behind NAT never sees.
#[test]
fn the_register_contact_names_tls_so_inbound_calls_come_back_over_the_socket() {
    let local =
        rsipstack::sip::Uri::try_from("sips:pagis9f@192.168.86.55:56636;transport=tls").unwrap();
    let rendered = register_contact(local).to_string();
    assert!(
        rendered.starts_with("<sip:pagis9f@192.168.86.55:56636"),
        "{rendered}"
    );
    assert!(
        rendered.to_ascii_lowercase().contains("transport=tls"),
        "{rendered}"
    );

    let bare = rsipstack::sip::Uri::try_from("sip:pagis9f@192.168.86.55:56636").unwrap();
    let rendered = register_contact(bare).to_string();
    assert!(
        rendered.to_ascii_lowercase().contains("transport=tls"),
        "{rendered}"
    );
}

const USERNAME: &str = "pagisline";

/// An inbound `INVITE` as the carrier sends it to the registered
/// Contact.
fn invite(request_uri: &str, to: &str) -> rsipstack::sip::Request {
    rsipstack::sip::Request::try_from(format!(
        "INVITE {request_uri} SIP/2.0\r\n\
         Via: SIP/2.0/TLS 192.0.2.1:5061;branch=z9hG4bK776asdhds\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:+16505550100@sip.telnyx.com>;tag=1928301774\r\n\
         To: {to}\r\n\
         Call-ID: a84b4c76e66710@sip.telnyx.com\r\n\
         CSeq: 314159 INVITE\r\n\
         Content-Length: 0\r\n\r\n"
    ))
    .expect("a SIP request")
}

/// A carrier routes by the dialed number and puts it in the
/// Request-URI. `To` can name something else, and the Request-URI wins.
#[test]
fn the_dialed_number_is_the_user_part_of_the_request_uri() {
    let request = invite(
        "sip:+14155550123@192.0.2.10:5061;transport=tls",
        "<sip:+14155550124@sip.telnyx.com>",
    );

    assert_eq!(
        dialed_e164(&request, USERNAME).as_deref(),
        Some("+14155550123")
    );
}

/// A credential connection can send the `INVITE` to the registered
/// Contact, whose user part is the SIP username. Then `To` names the
/// dialed number.
#[test]
fn a_request_uri_that_names_the_sip_username_reads_the_number_from_to() {
    let request = invite(
        &format!("sip:{USERNAME}@192.0.2.10:5061;transport=tls"),
        "<sip:+14155550124@sip.telnyx.com>",
    );

    assert_eq!(
        dialed_e164(&request, USERNAME).as_deref(),
        Some("+14155550124")
    );
}

/// A number that is not E.164 is not a number the line routes by, and
/// no `+` is added: a national number with a `+` in front is another
/// country's number.
#[test]
fn a_dialed_number_that_is_not_e164_or_is_missing_is_none() {
    let national = invite(
        "sip:4155550123@192.0.2.10:5061",
        "<sip:4155550123@sip.telnyx.com>",
    );
    assert_eq!(dialed_e164(&national, USERNAME), None);

    let no_user = invite("sip:192.0.2.10:5061", "<sip:+14155550124@sip.telnyx.com>");
    assert_eq!(dialed_e164(&no_user, USERNAME), None);

    let to_is_the_username = invite(
        &format!("sip:{USERNAME}@192.0.2.10:5061"),
        &format!("<sip:{USERNAME}@sip.telnyx.com>"),
    );
    assert_eq!(dialed_e164(&to_is_the_username, USERNAME), None);
}

#[test]
fn a_refused_call_gets_the_sip_status_of_its_reason() {
    assert_eq!(Refusal::Busy.status(), 486);
    assert_eq!(Refusal::NotFound.status(), 404);
    assert_eq!(Refusal::Unavailable.status(), 480);
}
