//! What the reverse proxy in front of the daemon says about a request.
//!
//! The daemon terminates no TLS. A deployment that serves a network puts
//! a reverse proxy in front, and the proxy tells the daemon two things
//! the socket does not carry: which address the browser came from
//! (`X-Forwarded-For`) and whether the browser spoke TLS
//! (`X-Forwarded-Proto`). Both headers are client-writable, so the
//! daemon reads them from one configured address and from nowhere else.
//! Without that address it reads neither, which is the local
//! installation.
//!
//! A proxy appends the peer it saw to `X-Forwarded-For`, so the entry
//! the daemon may believe is the **last** one: everything before it
//! reached the proxy inside the request and may be invented.
//! https://docs.pagis.co/server/proxy holds the proxy configuration that
//! matches.

use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;

/// `X-Forwarded-For`: the addresses a chain of proxies saw.
const FORWARDED_FOR: &str = "x-forwarded-for";
/// `X-Forwarded-Proto`: the scheme the browser spoke to the proxy.
const FORWARDED_PROTO: &str = "x-forwarded-proto";

/// The one address whose forwarded headers the daemon believes.
///
/// `None` trusts nothing: a local installation speaks to a browser on
/// the same machine over plain HTTP, and every request is exactly what
/// the socket says it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrustedProxy(Option<IpAddr>);

impl TrustedProxy {
    /// Believe the forwarded headers of this address alone.
    pub fn at(address: IpAddr) -> Self {
        Self(Some(address))
    }

    /// Believe nobody's forwarded headers.
    pub fn none() -> Self {
        Self(None)
    }

    /// The configured address, for a log line that says what the daemon
    /// trusts.
    pub fn address(&self) -> Option<IpAddr> {
        self.0
    }

    fn trusts(&self, peer: SocketAddr) -> bool {
        self.0 == Some(peer.ip())
    }

    /// The address the request really came from: the browser's own when
    /// the trusted proxy forwarded it, and otherwise the peer on the
    /// socket. The sign-in rate limit counts against this, so one
    /// person behind the proxy cannot lock out everybody else.
    pub fn client_address(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
        if !self.trusts(peer) {
            return peer.ip();
        }
        last_value(headers, FORWARDED_FOR)
            .and_then(|value| value.parse::<IpAddr>().ok())
            .unwrap_or_else(|| peer.ip())
    }

    /// Whether the browser reached this installation over TLS. The
    /// Session cookie carries `Secure` when it did, so a network
    /// deployment never hands a cookie to a plain-HTTP page.
    pub fn is_secure(&self, peer: SocketAddr, headers: &HeaderMap) -> bool {
        if !self.trusts(peer) {
            return false;
        }
        last_value(headers, FORWARDED_PROTO)
            .is_some_and(|value| value.eq_ignore_ascii_case("https"))
    }
}

/// The headers a proxy writes to say that it forwarded a request. A
/// client that talks to the daemon directly has no reason to send any
/// of them.
const PROXY_HEADERS: &[&str] = &[
    "forwarded",
    "via",
    FORWARDED_FOR,
    "x-forwarded-host",
    FORWARDED_PROTO,
    "x-real-ip",
    // The Funnel of Remote Access marks each request from the internet.
    "tailscale-funnel-request",
];

/// Whether a request came from a program on this machine and not
/// through a proxy (ADR-0025).
///
/// The Client Credential trade, the Sign-In Link and the runtime
/// identity handshake answer only such a request. Three things must
/// hold:
///
/// 1. The socket peer is a loopback address. The daemon reads the
///    socket and never `X-Forwarded-For` for this, because a client
///    writes that header.
/// 2. The request carries no header that a proxy writes, such as
///    `X-Forwarded-For`, `Forwarded` or `Via`.
/// 3. The `Host` header names a loopback host. A proxy passes on the
///    name that people open, and that name is not loopback.
///
/// The Trusted Proxy address does not decide it. A proxy on the same
/// machine connects from `127.0.0.1`, which is the address the Client
/// App connects from too, so that address cannot tell the two apart. A
/// proxy on another machine fails the first rule. A proxy on this
/// machine, named as the Trusted Proxy or not, fails the second rule or
/// the third, because it writes `X-Forwarded-For` or passes on the
/// public name.
pub fn is_from_this_machine(peer: SocketAddr, headers: &HeaderMap) -> bool {
    peer.ip().is_loopback()
        && PROXY_HEADERS
            .iter()
            .all(|name| !headers.contains_key(*name))
        && headers
            .get(axum::http::header::HOST)
            .and_then(|value| value.to_str().ok())
            .is_some_and(is_loopback_host)
}

/// Refuse a request that did not come from a program on this machine.
///
/// A local installation with Remote Access off serves the People of
/// this machine alone. The daemon then binds loopback, but a Funnel, a
/// reverse proxy or a tunnel on this machine can still forward requests
/// from other machines to it, and it may still run after Remote Access
/// goes off. So every request must pass [`is_from_this_machine`], and a
/// Session that a Member opened from another machine reaches nothing
/// until Remote Access is on again, whether the Funnel still runs or
/// not.
pub async fn refuse_other_machines(
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if !is_from_this_machine(peer, request.headers()) {
        return refusal(request.uri().path(), request.headers());
    }
    next.run(request).await
}

/// Why [`refuse_other_machines`] refuses a request.
const ONLY_THIS_MACHINE: &str = "this installation serves only the people of its own computer; \
     its owner turns on Remote Access to serve other machines";

/// The refusal as a page, for a browser that opens the Public Origin.
/// The refusal blocks the fonts and the stylesheets of the UI too, so
/// the page holds the few design tokens it uses and loads nothing.
const ONLY_THIS_MACHINE_PAGE: &str = include_str!("only_this_machine.html");

/// The page holds its style inline and loads nothing, so it needs no
/// other source.
const ONLY_THIS_MACHINE_PAGE_POLICY: &str =
    "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'";

/// The answer to a request from another machine. A browser that opens a
/// page gets [`ONLY_THIS_MACHINE_PAGE`], and every other request, an API
/// request above all, gets the JSON error of the API.
fn refusal(path: &str, headers: &HeaderMap) -> axum::response::Response {
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;
    let api = path == "/api" || path.starts_with("/api/");
    if api || !prefers_html(headers) {
        return crate::error::ApiError::forbidden(ONLY_THIS_MACHINE).into_response();
    }
    (
        StatusCode::FORBIDDEN,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            // The owner can turn Remote Access on, and the page must not
            // outlive that.
            (header::CACHE_CONTROL, "no-store"),
            (
                header::CONTENT_SECURITY_POLICY,
                ONLY_THIS_MACHINE_PAGE_POLICY,
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        ONLY_THIS_MACHINE_PAGE,
    )
        .into_response()
}

/// Whether the `Accept` header gives `text/html` a higher quality than
/// `application/json` (RFC 9110, section 12.5.1). The most specific
/// media range that matches a type gives its quality. A tie, `*/*`
/// alone or no header at all selects JSON, the answer of the API.
fn prefers_html(headers: &HeaderMap) -> bool {
    let ranges: Vec<(String, f32)> = headers
        .get_all(axum::http::header::ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(media_range)
        .collect();
    let quality = |kind: &str, subtype: &str| {
        let exact = format!("{kind}/{subtype}");
        let any_subtype = format!("{kind}/*");
        [exact.as_str(), any_subtype.as_str(), "*/*"]
            .iter()
            .find_map(|wanted| {
                ranges
                    .iter()
                    .find(|(range, _)| range == wanted)
                    .map(|(_, quality)| *quality)
            })
            .unwrap_or(0.0)
    };
    quality("text", "html") > quality("application", "json")
}

/// One entry of an `Accept` header: the media range in lower case and
/// its quality, 1 when the entry names none.
fn media_range(entry: &str) -> Option<(String, f32)> {
    let mut parts = entry.split(';').map(str::trim);
    let range = parts.next()?.to_ascii_lowercase();
    if !range.contains('/') {
        return None;
    }
    let quality = parts
        .filter_map(|parameter| parameter.split_once('='))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("q"))
        .and_then(|(_, value)| value.trim().parse::<f32>().ok())
        .unwrap_or(1.0);
    Some((range, quality))
}

/// Whether a `Host` value names this machine: `localhost` or a loopback
/// address, with or without a port.
fn is_loopback_host(value: &str) -> bool {
    let Ok(authority) = value.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    let host = authority.host();
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// The last entry of a forwarded header, across repeated headers and
/// comma-separated lists. The trusted proxy appends its own entry last,
/// so that is the one entry the request itself could not write.
fn last_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|value| value.trim())
        .rfind(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> SocketAddr {
        "10.0.0.2:52000".parse().unwrap()
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        headers
    }

    fn proxy() -> TrustedProxy {
        TrustedProxy::at("10.0.0.2".parse().unwrap())
    }

    #[test]
    fn the_trusted_proxy_hands_over_the_browser_address_and_the_scheme() {
        let headers = headers(&[(FORWARDED_FOR, "203.0.113.7"), (FORWARDED_PROTO, "https")]);

        assert_eq!(
            proxy().client_address(peer(), &headers),
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
        assert!(proxy().is_secure(peer(), &headers));
    }

    /// An address the daemon does not trust is only what the socket
    /// says, however it labels itself.
    #[test]
    fn a_request_from_another_address_cannot_claim_a_browser_or_tls() {
        let headers = headers(&[(FORWARDED_FOR, "203.0.113.7"), (FORWARDED_PROTO, "https")]);
        let stranger: SocketAddr = "10.0.0.9:52000".parse().unwrap();

        assert_eq!(
            proxy().client_address(stranger, &headers),
            "10.0.0.9".parse::<IpAddr>().unwrap()
        );
        assert!(!proxy().is_secure(stranger, &headers));
    }

    /// A local installation trusts nothing, so the headers change
    /// nothing at all.
    #[test]
    fn with_no_trusted_proxy_the_headers_are_ignored() {
        let headers = headers(&[(FORWARDED_FOR, "203.0.113.7"), (FORWARDED_PROTO, "https")]);

        assert_eq!(
            TrustedProxy::none().client_address(peer(), &headers),
            peer().ip()
        );
        assert!(!TrustedProxy::none().is_secure(peer(), &headers));
    }

    /// A browser that sends its own `X-Forwarded-For` gets its entry
    /// pushed in front of the proxy's, and the daemon reads the proxy's.
    #[test]
    fn an_invented_entry_in_front_of_the_proxys_is_not_the_client() {
        let appended = headers(&[(FORWARDED_FOR, "198.51.100.1, 203.0.113.7")]);
        assert_eq!(
            proxy().client_address(peer(), &appended),
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );

        // Repeated headers read the same way as one list.
        let repeated = headers(&[
            (FORWARDED_FOR, "198.51.100.1"),
            (FORWARDED_FOR, "203.0.113.7"),
        ]);
        assert_eq!(
            proxy().client_address(peer(), &repeated),
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
    }

    /// A header the proxy did not set, or one that holds no address,
    /// leaves the peer as the answer rather than nothing.
    #[test]
    fn a_missing_or_unreadable_header_falls_back_to_the_peer() {
        assert_eq!(
            proxy().client_address(peer(), &HeaderMap::new()),
            peer().ip()
        );
        let nonsense = headers(&[(FORWARDED_FOR, "unknown")]);
        assert_eq!(proxy().client_address(peer(), &nonsense), peer().ip());
        assert!(!proxy().is_secure(peer(), &headers(&[(FORWARDED_PROTO, "http")])));
        assert!(proxy().is_secure(peer(), &headers(&[(FORWARDED_PROTO, "HTTPS")])));
    }

    fn loopback() -> SocketAddr {
        "127.0.0.1:52000".parse().unwrap()
    }

    /// The Client App on this machine connects over loopback, names a
    /// loopback host and sends no forwarding header.
    #[test]
    fn a_loopback_client_with_a_loopback_host_is_from_this_machine() {
        for host in [
            "127.0.0.1:4400",
            "localhost:4400",
            "[::1]:4400",
            "localhost",
        ] {
            assert!(
                is_from_this_machine(loopback(), &headers(&[("host", host)])),
                "{host}"
            );
        }
        let six: SocketAddr = "[::1]:52000".parse().unwrap();
        assert!(is_from_this_machine(
            six,
            &headers(&[("host", "[::1]:4400")])
        ));
    }

    /// The socket peer decides where a request came from. A header
    /// cannot move a request from another machine onto this one.
    #[test]
    fn a_peer_on_another_machine_is_not_from_this_machine() {
        let stranger: SocketAddr = "10.0.0.9:52000".parse().unwrap();

        assert!(!is_from_this_machine(
            stranger,
            &headers(&[("host", "127.0.0.1:4400")])
        ));
        assert!(!is_from_this_machine(
            stranger,
            &headers(&[("host", "127.0.0.1:4400"), (FORWARDED_FOR, "127.0.0.1")])
        ));
    }

    /// A proxy on this machine connects from loopback, as the Client App
    /// does. The header it writes, or the public name it passes on, says
    /// that the request came through it.
    #[test]
    fn a_request_through_a_proxy_on_this_machine_is_not_from_this_machine() {
        for proxied in [
            headers(&[("host", "127.0.0.1:4400"), (FORWARDED_FOR, "203.0.113.7")]),
            headers(&[("host", "127.0.0.1:4400"), (FORWARDED_PROTO, "https")]),
            headers(&[
                ("host", "127.0.0.1:4400"),
                ("x-forwarded-host", "pagis.example"),
            ]),
            headers(&[("host", "127.0.0.1:4400"), ("x-real-ip", "203.0.113.7")]),
            headers(&[("host", "127.0.0.1:4400"), ("forwarded", "for=203.0.113.7")]),
            headers(&[("host", "127.0.0.1:4400"), ("via", "1.1 caddy")]),
            headers(&[
                ("host", "127.0.0.1:4400"),
                ("tailscale-funnel-request", "?1"),
            ]),
            headers(&[("host", "pagis.example")]),
            headers(&[("host", "pagis.example:443")]),
            headers(&[("host", "10.0.0.5:4400")]),
            headers(&[("host", "localhost.pagis.example")]),
            HeaderMap::new(),
        ] {
            assert!(!is_from_this_machine(loopback(), &proxied), "{proxied:?}");
        }
    }

    /// A browser that navigates to a page sends `text/html` first in its
    /// `Accept` header.
    const BROWSER_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

    /// The API message as the page writes it: a capital first letter and
    /// a full stop.
    fn as_sentence(message: &str) -> String {
        let mut letters = message.chars();
        let first = letters.next().unwrap().to_uppercase();
        format!("{first}{}.", letters.as_str())
    }

    async fn body_of(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn content_type(response: &axum::response::Response) -> &str {
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    }

    /// A browser that opens a page through the proxy gets a page with
    /// the refusal, not the JSON error of the API.
    #[tokio::test]
    async fn a_browser_that_opens_a_page_gets_the_refusal_as_a_page() {
        for path in ["/", "/chat/abc", "/administration.html"] {
            let response = refusal(path, &headers(&[("accept", BROWSER_ACCEPT)]));

            assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
            assert!(
                content_type(&response).starts_with("text/html"),
                "{path}: {}",
                content_type(&response)
            );
            assert_eq!(
                response
                    .headers()
                    .get(axum::http::header::CACHE_CONTROL)
                    .unwrap(),
                "no-store"
            );
            let page = body_of(response).await;
            assert!(page.starts_with("<!doctype html>"), "{page}");
            assert!(page.contains(&as_sentence(ONLY_THIS_MACHINE)), "{page}");
            // The refusal blocks every asset of the UI too, so the page
            // loads nothing from the daemon.
            assert!(!page.contains(" src="), "{page}");
            assert!(!page.contains("<link"), "{page}");
        }
    }

    /// An API client, and a page request that does not ask for HTML
    /// first, keep the JSON error.
    #[tokio::test]
    async fn an_api_request_and_a_request_for_no_html_get_the_json_error() {
        for (path, accept) in [
            ("/api/v1/me", Some(BROWSER_ACCEPT)),
            ("/api", Some(BROWSER_ACCEPT)),
            ("/api/v1/me", None),
            ("/", None),
            ("/", Some("application/json")),
            ("/", Some("*/*")),
            ("/", Some("application/json, text/html;q=0.5")),
            ("/", Some("text/html;q=0")),
        ] {
            let pairs = accept.map(|value| ("accept", value));
            let response = refusal(path, &headers(pairs.as_slice()));

            assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
            assert_eq!(
                content_type(&response),
                "application/json",
                "{path} {accept:?}"
            );
            let body: serde_json::Value = serde_json::from_str(&body_of(response).await).unwrap();
            assert_eq!(body["error"]["code"], "forbidden");
            assert_eq!(body["error"]["message"], ONLY_THIS_MACHINE);
        }
    }

    /// The negotiation reads the quality of each media range and the
    /// most specific range that matches, as RFC 9110 says.
    #[test]
    fn a_request_prefers_html_when_html_has_the_higher_quality() {
        for accept in [
            BROWSER_ACCEPT,
            "text/html",
            "text/*",
            "TEXT/HTML; charset=utf-8",
            "application/json;q=0.4, text/html;q=0.9",
            "*/*;q=0.1, text/html",
        ] {
            assert!(prefers_html(&headers(&[("accept", accept)])), "{accept}");
        }
        for accept in [
            "",
            "*/*",
            "application/json",
            "text/html, application/json",
            "text/html;q=0.2, */*;q=0.9",
            "text/html;q=0, */*",
            "text/plain",
            "nonsense",
        ] {
            assert!(!prefers_html(&headers(&[("accept", accept)])), "{accept}");
        }
        assert!(!prefers_html(&HeaderMap::new()));
    }
}
