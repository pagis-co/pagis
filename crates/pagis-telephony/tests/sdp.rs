//! The offer and the answer (ADR-0020): PCMU and PCMA only, `ptime=20`,
//! RTCP mux, SDES-SRTP and no cleartext path. The media session binds a
//! local UDP port to write them; nothing is sent.

use pagis_telephony::audio::Codec;
use pagis_telephony::sip_media::{DTMF_PAYLOAD_TYPE, Negotiated, SipMedia, negotiated_from_sdp};
use rustrtc::{SdpType, SessionDescription};

fn audio_line(sdp: &str) -> &str {
    sdp.lines()
        .find(|line| line.starts_with("m=audio"))
        .expect("an audio line")
}

fn attributes<'a>(sdp: &'a str, key: &str) -> Vec<&'a str> {
    let prefix = format!("a={key}:");
    sdp.lines()
        .filter_map(|line| line.strip_prefix(prefix.as_str()))
        .collect()
}

#[tokio::test]
async fn the_offer_carries_g711_only_over_srtp_with_rtcp_mux_and_ptime_20() {
    let media = SipMedia::new();

    let offer = media.offer().await.unwrap();

    let audio = audio_line(&offer);
    let parts: Vec<&str> = audio.split_whitespace().collect();
    assert_eq!(parts[2], "RTP/SAVP", "no cleartext profile: {audio}");
    let formats: Vec<&str> = parts[3..].to_vec();
    assert_eq!(formats, vec!["0", "8", "101"], "{audio}");
    let rtpmaps = attributes(&offer, "rtpmap");
    assert!(rtpmaps.contains(&"0 PCMU/8000"), "{rtpmaps:?}");
    assert!(rtpmaps.contains(&"8 PCMA/8000"), "{rtpmaps:?}");
    assert!(rtpmaps.contains(&"101 telephone-event/8000"), "{rtpmaps:?}");
    assert_eq!(attributes(&offer, "ptime"), vec!["20"]);
    assert!(offer.lines().any(|line| line == "a=rtcp-mux"), "{offer}");
    let crypto = attributes(&offer, "crypto");
    assert_eq!(crypto.len(), 1, "{offer}");
    assert!(
        crypto[0].starts_with("1 AES_CM_128_HMAC_SHA1_80 inline:"),
        "{crypto:?}"
    );
    // Key only: no lifetime and no MKI, because the packets Pagis sends
    // carry no MKI, and a carrier that reads the MKI rejects the call.
    assert!(!crypto[0].contains('|'), "{crypto:?}");
    assert_eq!(crypto[0].split_whitespace().count(), 3, "{crypto:?}");
    assert!(!offer.contains("a=ice-ufrag"), "no ICE: {offer}");
    assert!(!offer.contains("a=fingerprint"), "no DTLS: {offer}");
    assert!(!offer.contains("m=video"), "{offer}");
    assert_eq!(media.local_sdp().as_deref(), Some(offer.as_str()));
}

const OFFER_PCMA_FIRST: &str = "v=0\r\n\
o=telnyx 1 1 IN IP4 192.0.2.10\r\n\
s=-\r\n\
c=IN IP4 192.0.2.10\r\n\
t=0 0\r\n\
m=audio 20000 RTP/SAVP 8 0 96\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:96 telephone-event/8000\r\n\
a=fmtp:96 0-16\r\n\
a=ptime:20\r\n\
a=rtcp-mux\r\n\
a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:WVNfX19zZW1jdGwgKCkgewkyMjA7fQp9CnVubGVz|2^31|1:1\r\n\
a=sendrecv\r\n";

#[tokio::test]
async fn the_answer_follows_the_offer_and_keeps_srtp() {
    let media = SipMedia::new();

    let (answer, negotiated) = media.answer(OFFER_PCMA_FIRST).await.unwrap().unwrap();

    assert_eq!(
        negotiated,
        Negotiated {
            codec: Codec::Pcma,
            dtmf_payload_type: Some(96),
        }
    );
    let audio = audio_line(&answer);
    assert!(audio.contains("RTP/SAVP"), "{audio}");
    assert!(
        !audio.contains(" 9 ") && !audio.ends_with(" 9"),
        "no G.722: {audio}"
    );
    assert_eq!(attributes(&answer, "ptime"), vec!["20"]);
    assert!(answer.lines().any(|line| line == "a=rtcp-mux"), "{answer}");
    let crypto = attributes(&answer, "crypto");
    assert_eq!(crypto.len(), 1, "{answer}");
    assert!(!crypto[0].contains('|'), "{crypto:?}");
}

#[tokio::test]
async fn an_offer_without_g711_gets_no_answer() {
    let media = SipMedia::new();
    let offer = OFFER_PCMA_FIRST
        .replace(
            "m=audio 20000 RTP/SAVP 8 0 96",
            "m=audio 20000 RTP/SAVP 9 96",
        )
        .replace(
            "a=rtpmap:8 PCMA/8000\r\na=rtpmap:0 PCMU/8000\r\n",
            "a=rtpmap:9 G722/8000\r\n",
        );

    let answer = media.answer(&offer).await.unwrap();

    assert!(answer.is_none());
}

#[test]
fn what_the_far_side_settled_is_read_from_its_sdp() {
    let sdp = SessionDescription::parse(SdpType::Answer, OFFER_PCMA_FIRST).unwrap();
    assert_eq!(
        negotiated_from_sdp(&sdp),
        Some(Negotiated {
            codec: Codec::Pcma,
            dtmf_payload_type: Some(96),
        })
    );

    let no_events = OFFER_PCMA_FIRST
        .replace("m=audio 20000 RTP/SAVP 8 0 96", "m=audio 20000 RTP/SAVP 0")
        .replace("a=rtpmap:96 telephone-event/8000\r\na=fmtp:96 0-16\r\n", "");
    let sdp = SessionDescription::parse(SdpType::Answer, &no_events).unwrap();
    assert_eq!(
        negotiated_from_sdp(&sdp),
        Some(Negotiated {
            codec: Codec::Pcmu,
            dtmf_payload_type: None,
        })
    );
    assert_eq!(DTMF_PAYLOAD_TYPE, 101);
}

/// The far side's SDP with the `a=crypto` line taken out and the
/// cleartext profile in its place: what a carrier answers when SRTP
/// is off on its side.
fn cleartext(sdp: &str) -> String {
    sdp.lines()
        .filter(|line| !line.starts_with("a=crypto:"))
        .map(|line| line.replace("RTP/SAVP", "RTP/AVP"))
        .collect::<Vec<_>>()
        .join("\r\n")
        + "\r\n"
}

#[tokio::test]
async fn an_answer_without_crypto_is_refused() {
    let media = SipMedia::new();
    media.offer().await.unwrap();

    let refused = media.apply_answer(&cleartext(OFFER_PCMA_FIRST)).await;

    assert!(refused.is_err(), "a cleartext answer is not applied");
}

#[tokio::test]
async fn an_offer_without_crypto_gets_no_answer() {
    let media = SipMedia::new();

    let answer = media.answer(&cleartext(OFFER_PCMA_FIRST)).await.unwrap();

    assert!(answer.is_none());
}

/// The answered side keys SRTP from the offer it applied and the answer
/// it wrote. The media session must come up on that pair, or every
/// packet in both directions is lost and the call dies at the media
/// timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_answered_media_session_comes_up() {
    for _ in 0..50 {
        let media = SipMedia::new();
        let (_answer, _negotiated) = media
            .answer(OFFER_PCMA_FIRST)
            .await
            .unwrap()
            .expect("the offer is answerable");
        assert_eq!(media.connected().await, Ok(()));
    }
}

/// Telnyx offers eleven suites and puts the AEAD ones first. The answer
/// takes the one suite Pagis keys with and echoes its tag (RFC 4568),
/// and the media session comes up on it: rustrtc keys from the first
/// remote crypto line, so the offer it sees carries only that one.
const OFFER_MANY_SUITES: &str = "v=0\r\n\
o=Telnyx 1 1 IN IP4 192.0.2.10\r\n\
s=Telnyx\r\n\
c=IN IP4 192.0.2.10\r\n\
t=0 0\r\n\
m=audio 20000 RTP/SAVP 0 8 101\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:101 telephone-event/8000\r\n\
a=fmtp:101 0-15\r\n\
a=sendrecv\r\n\
a=crypto:1 AEAD_AES_256_GCM_8 inline:r131L5Gxybsf7IzQ67DC0KqtM9ove8qA6BWmX/en5mYAN6Frqf2vyEFtY/Y=\r\n\
a=crypto:2 AEAD_AES_256_GCM inline:KkG0Yld2w/Y2sHubo5BJ1xeQqtTmcyOdepfbCU4Z0uextb2jGHK78SOTTX8=\r\n\
a=crypto:5 AES_256_CM_HMAC_SHA1_80 inline:1nIremqPbeQtdu3umaLQlns9L2lMJo+/GpW1vAgbhc6IDNBQkVwioVRKhMm5NA==\r\n\
a=crypto:7 AES_CM_128_HMAC_SHA1_80 inline:0Ea1RslcJrxISOq+6YuEXpj+rSHUgUIfe1zxe4VI\r\n\
a=crypto:10 AES_CM_128_HMAC_SHA1_32 inline:k614PsW1eIttIWViOOcbeCpW5vgmN/2SEZa3GGzc\r\n\
a=ptime:20\r\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_answer_takes_one_suite_of_many_and_echoes_its_tag() {
    let media = SipMedia::new();
    let (answer, _negotiated) = media
        .answer(OFFER_MANY_SUITES)
        .await
        .unwrap()
        .expect("the offer is answerable");
    let crypto = attributes(&answer, "crypto");
    assert_eq!(crypto.len(), 1, "{answer}");
    assert!(
        crypto[0].starts_with("7 AES_CM_128_HMAC_SHA1_80 inline:"),
        "{crypto:?}"
    );
    assert_eq!(media.connected().await, Ok(()));
}
