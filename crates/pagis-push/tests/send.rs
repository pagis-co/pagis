//! A Web Push to a fake push service: the body that the client decrypts,
//! the VAPID token, the headers, and the outcome of each answer.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::SecretKey;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use pagis_push::{Options, Outcome, Policy, Subscription, Topic, Urgency, WebPush};
use reqwest::StatusCode;
use web_push_native::Auth;
use web_push_native::jwt_simple::algorithms::{ECDSAP256PublicKeyLike, ES256PublicKey};
use web_push_native::jwt_simple::claims::NoCustomClaims;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const PUBLIC_ORIGIN: &str = "https://pagis.example.com";
const PROJECT: &str = "https://github.com/pagis-co/pagis";
const TWELVE_HOURS: u64 = 12 * 60 * 60;

fn random_key() -> SecretKey {
    SecretKey::from_slice(&rand::random::<[u8; 32]>()).expect("a P-256 secret key")
}

/// A client of the Product App: the key pair and the auth secret of its
/// Push Subscription. Only it can decrypt a Web Push.
struct Client {
    secret: SecretKey,
    auth: [u8; 16],
}

impl Client {
    fn new() -> Self {
        Self {
            secret: random_key(),
            auth: rand::random(),
        }
    }

    fn subscription(&self, endpoint: &str) -> Subscription {
        Subscription {
            endpoint: endpoint.to_string(),
            p256dh: URL_SAFE_NO_PAD
                .encode(self.secret.public_key().to_encoded_point(false).as_bytes()),
            auth: URL_SAFE_NO_PAD.encode(self.auth),
        }
    }

    fn decrypt(&self, body: &[u8]) -> Vec<u8> {
        web_push_native::decrypt(
            body.to_vec(),
            &self.secret,
            &Auth::clone_from_slice(&self.auth),
        )
        .expect("the client decrypts the body")
    }
}

/// A push service that answers each Web Push with `answer`, and the
/// client whose endpoint it holds.
struct PushService {
    server: MockServer,
    client: Client,
    vapid_key: SecretKey,
}

impl PushService {
    async fn answering(answer: ResponseTemplate) -> Self {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/push/one"))
            .respond_with(answer)
            .mount(&server)
            .await;
        Self {
            server,
            client: Client::new(),
            vapid_key: random_key(),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/push/one", self.server.uri())
    }

    fn subscription(&self) -> Subscription {
        self.client.subscription(&self.endpoint())
    }

    fn web_push(&self, public_origin: &str) -> WebPush {
        WebPush::new(&self.vapid_key, public_origin, Policy::AllowLoopback)
            .expect("a Web Push sender")
    }

    async fn send(&self, plaintext: &[u8], options: Options) -> Outcome {
        self.web_push(PUBLIC_ORIGIN)
            .send(&self.subscription(), plaintext, options)
            .await
    }

    async fn received(&self) -> Vec<Request> {
        self.server
            .received_requests()
            .await
            .expect("the push service records requests")
    }

    /// The plaintext of each Web Push that the push service received, as
    /// the client decrypts it.
    async fn plaintexts(&self) -> Vec<Vec<u8>> {
        self.received()
            .await
            .iter()
            .map(|request| self.client.decrypt(&request.body))
            .collect()
    }
}

fn options() -> Options {
    Options {
        ttl: Duration::from_secs(86_400),
        urgency: Urgency::High,
        topic: None,
    }
}

fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request
        .headers
        .get(name)
        .map(|value| value.to_str().expect("an ASCII header"))
}

/// The plaintext goes as one `aes128gcm` record that only the client
/// decrypts: the body is the plaintext and 103 bytes.
#[tokio::test]
async fn a_web_push_reaches_the_endpoint_and_the_client_decrypts_it() {
    let service = PushService::answering(ResponseTemplate::new(201)).await;
    let plaintext = br#"{"web_push":8030,"notification":{"title":"Approve a payment"}}"#;

    let outcome = service.send(plaintext, options()).await;

    assert_eq!(outcome, Outcome::Delivered);
    let received = service.received().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].body.len(), plaintext.len() + 103);
    assert_ne!(&received[0].body[..], &plaintext[..]);
    assert_eq!(service.plaintexts().await, vec![plaintext.to_vec()]);
}

/// The VAPID token is signed by the key in `k`, names the origin of the
/// endpoint, ends 12 hours from now and names the Public Origin as the
/// contact, or the project when the Public Origin is not `https`.
#[tokio::test]
async fn the_vapid_token_is_signed_by_k_for_the_origin_of_the_endpoint() {
    let service = PushService::answering(ResponseTemplate::new(201)).await;
    for public_origin in [PUBLIC_ORIGIN, "http://127.0.0.1:7880"] {
        service
            .web_push(public_origin)
            .send(&service.subscription(), b"hello", options())
            .await;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a time after the epoch")
        .as_secs();

    let received = service.received().await;
    assert_eq!(received.len(), 2);
    let vapid_public = service
        .vapid_key
        .public_key()
        .to_encoded_point(false)
        .as_bytes()
        .to_vec();
    for (request, contact) in received.iter().zip([PUBLIC_ORIGIN, PROJECT]) {
        let authorization = header(request, "authorization").expect("an Authorization header");
        let (token, k) = authorization
            .strip_prefix("vapid t=")
            .and_then(|rest| rest.split_once(", k="))
            .expect("the vapid scheme with t and k");
        let k = URL_SAFE_NO_PAD.decode(k).expect("k is base64url");
        assert_eq!(k, vapid_public, "k is the public half of the VAPID Key");
        let claims = ES256PublicKey::from_bytes(&k)
            .expect("k is a P-256 point")
            .verify_token::<NoCustomClaims>(token, None)
            .expect("the token verifies against k");
        let audience = claims
            .audiences
            .expect("an aud claim")
            .into_string()
            .expect("one audience");
        assert_eq!(audience, service.server.uri(), "aud is the origin");
        assert_eq!(claims.subject.as_deref(), Some(contact));
        let expires = claims.expires_at.expect("an exp claim").as_secs();
        assert!(
            expires.abs_diff(now + TWELVE_HOURS) <= 60,
            "exp is {expires}, now is {now}"
        );
    }
}

/// Each Web Push carries `TTL`, `Urgency`, `Topic` and the encoding of
/// its body.
#[tokio::test]
async fn the_headers_carry_ttl_urgency_topic_and_the_encoding() {
    let service = PushService::answering(ResponseTemplate::new(201)).await;
    let urgencies = [
        (Urgency::VeryLow, "very-low"),
        (Urgency::Low, "low"),
        (Urgency::Normal, "normal"),
        (Urgency::High, "high"),
    ];
    for (urgency, _) in urgencies {
        let options = Options {
            ttl: Duration::from_secs(86_400),
            urgency,
            topic: Some(Topic::new("01HZY3N4X0A1B2C3D4E5F6G7H8").expect("a topic")),
        };
        assert_eq!(service.send(b"hello", options).await, Outcome::Delivered);
    }
    assert_eq!(service.send(b"hello", options()).await, Outcome::Delivered);

    let received = service.received().await;
    assert_eq!(received.len(), 5);
    for (request, (_, urgency)) in received.iter().zip(urgencies) {
        assert_eq!(header(request, "ttl"), Some("86400"));
        assert_eq!(header(request, "urgency"), Some(urgency));
        assert_eq!(header(request, "topic"), Some("01HZY3N4X0A1B2C3D4E5F6G7H8"));
        assert_eq!(header(request, "content-encoding"), Some("aes128gcm"));
        assert_eq!(
            header(request, "content-type"),
            Some("application/octet-stream")
        );
    }
    assert_eq!(header(&received[4], "topic"), None, "no topic, no header");
    assert_eq!(service.plaintexts().await, vec![b"hello".to_vec(); 5]);
}

/// A topic is at most 32 characters of the URL-safe base64 alphabet.
#[test]
fn a_topic_is_at_most_32_url_safe_base64_characters() {
    let longest = "a".repeat(32);
    assert_eq!(
        Topic::new(&longest).expect("32 characters").as_str(),
        longest
    );
    assert!(Topic::new("Az09-_").is_ok());
    for refused in [
        "",
        &"a".repeat(33),
        "item 1",
        "item+1",
        "item/1",
        "item=",
        "ítem",
    ] {
        assert!(Topic::new(refused).is_err(), "{refused:?} is a topic");
    }
}

/// Each answer of the push service maps to its outcome.
#[tokio::test]
async fn each_answer_of_the_push_service_maps_to_an_outcome() {
    let in_two_minutes = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(120));
    let cases = [
        (ResponseTemplate::new(201), Outcome::Delivered),
        (ResponseTemplate::new(404), Outcome::Gone),
        (ResponseTemplate::new(410), Outcome::Gone),
        (ResponseTemplate::new(413), Outcome::TooLarge),
        (
            ResponseTemplate::new(429).insert_header("Retry-After", "120"),
            Outcome::RateLimited {
                retry_after: Some(Duration::from_secs(120)),
            },
        ),
        (
            ResponseTemplate::new(429),
            Outcome::RateLimited { retry_after: None },
        ),
    ];
    for (answer, expected) in cases {
        let service = PushService::answering(answer).await;

        let outcome = service.send(b"hello", options()).await;

        assert_eq!(outcome, expected);
        assert_eq!(service.plaintexts().await, vec![b"hello".to_vec()]);
    }

    let service = PushService::answering(
        ResponseTemplate::new(429).insert_header("Retry-After", in_two_minutes.as_str()),
    )
    .await;
    let Outcome::RateLimited {
        retry_after: Some(retry_after),
    } = service.send(b"hello", options()).await
    else {
        panic!("a 429 with an HTTP-date is not rate limited with a delay");
    };
    assert!(
        (Duration::from_secs(115)..=Duration::from_secs(120)).contains(&retry_after),
        "{retry_after:?}"
    );

    let service =
        PushService::answering(ResponseTemplate::new(400).set_body_string("bad VAPID token")).await;
    let Outcome::Failed { status, error } = service.send(b"hello", options()).await else {
        panic!("a 400 is not a failure");
    };
    assert_eq!(status, Some(StatusCode::BAD_REQUEST));
    assert!(error.contains("bad VAPID token"), "{error}");
}

/// A redirect is an answer, and the sender does not follow it.
#[tokio::test]
async fn the_sender_follows_no_redirect() {
    let service =
        PushService::answering(ResponseTemplate::new(307).insert_header("Location", "/push/two"))
            .await;

    let outcome = service.send(b"hello", options()).await;

    let Outcome::Failed { status, .. } = outcome else {
        panic!("a redirect is not a failure: {outcome:?}");
    };
    assert_eq!(status, Some(StatusCode::TEMPORARY_REDIRECT));
    assert_eq!(service.received().await.len(), 1);
}

/// The public policy refuses a name of a loopback address, an IP address
/// and an endpoint that is not `https`. The push service gets nothing.
#[tokio::test]
async fn the_public_policy_refuses_an_endpoint_that_is_not_public() {
    let service = PushService::answering(ResponseTemplate::new(201)).await;
    let port = service.server.address().port();
    let web_push =
        WebPush::new(&service.vapid_key, PUBLIC_ORIGIN, Policy::Public).expect("a Web Push sender");
    for (endpoint, reason) in [
        (
            format!("https://localhost:{port}/push/one"),
            "localhost resolves to no public address",
        ),
        (format!("https://127.0.0.1:{port}/push/one"), "IP address"),
        ("https://10.0.0.1/push/one".to_string(), "IP address"),
        ("https://[::1]/push/one".to_string(), "IP address"),
        (format!("http://push.example.com:{port}/push/one"), "https"),
        (service.endpoint(), "https"),
    ] {
        let outcome = web_push
            .send(&service.client.subscription(&endpoint), b"hello", options())
            .await;

        let Outcome::Failed {
            status: None,
            error,
        } = outcome
        else {
            panic!("{endpoint} is not refused: {outcome:?}");
        };
        assert!(error.contains(reason), "{endpoint}: {error}");
    }
    assert!(service.received().await.is_empty());
}

/// The loopback policy of a test reaches a name of a loopback address
/// through the address guard.
#[tokio::test]
async fn the_loopback_policy_reaches_a_name_of_a_loopback_address() {
    let service = PushService::answering(ResponseTemplate::new(201)).await;
    let endpoint = format!(
        "http://localhost:{}/push/one",
        service.server.address().port()
    );

    let outcome = service
        .web_push(PUBLIC_ORIGIN)
        .send(&service.client.subscription(&endpoint), b"hello", options())
        .await;

    assert_eq!(outcome, Outcome::Delivered);
    assert_eq!(service.plaintexts().await, vec![b"hello".to_vec()]);
}

/// A plaintext over 2048 bytes is refused before the encryption, and the
/// push service gets nothing. One of 2048 bytes goes.
#[tokio::test]
async fn a_plaintext_over_2048_bytes_is_refused_before_the_send() {
    let service = PushService::answering(ResponseTemplate::new(201)).await;

    let refused = service.send(&[b'a'; 2049], options()).await;

    assert_eq!(refused, Outcome::TooLarge);
    assert!(service.received().await.is_empty());

    let largest = service.send(&[b'a'; 2048], options()).await;

    assert_eq!(largest, Outcome::Delivered);
    let received = service.received().await;
    assert_eq!(received[0].body.len(), 2048 + 103);
    assert_eq!(service.plaintexts().await, vec![vec![b'a'; 2048]]);
}

/// A Push Subscription whose keys are not keys fails with no request.
#[tokio::test]
async fn a_subscription_with_a_malformed_key_fails_before_the_send() {
    let service = PushService::answering(ResponseTemplate::new(201)).await;
    let good = service.subscription();
    let cases = [
        Subscription {
            p256dh: "not base64url!".to_string(),
            ..good.clone()
        },
        Subscription {
            p256dh: URL_SAFE_NO_PAD.encode([4u8; 65]),
            ..good.clone()
        },
        Subscription {
            auth: URL_SAFE_NO_PAD.encode([1u8; 15]),
            ..good.clone()
        },
        Subscription {
            endpoint: "not a URL".to_string(),
            ..good
        },
    ];
    let web_push = service.web_push(PUBLIC_ORIGIN);
    for subscription in &cases {
        let outcome = web_push.send(subscription, b"hello", options()).await;

        assert!(
            matches!(outcome, Outcome::Failed { status: None, .. }),
            "{subscription:?}: {outcome:?}"
        );
    }
    assert!(service.received().await.is_empty());
}
