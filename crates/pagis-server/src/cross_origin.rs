//! The refusal of a browser request from an origin that a listener does
//! not serve.
//!
//! The Session cookie is `SameSite=Strict`, which stops a cross-site
//! request only. A page on a sibling subdomain of the Public Origin, with
//! the same scheme, is same-site: the browser sends the cookie with the
//! WebSocket handshake and with the form post of that page. The CORS
//! answer stops only the read of a response. So each listener refuses
//! such a request before the Session check and before any upgrade
//! (ADR-0024).
//!
//! The check is the `CrossOriginProtection` of Go `net/http`:
//!
//! 1. A request with `Sec-Fetch-Site` passes when the value is
//!    `same-origin` or `none`. Every other value is refused.
//! 2. A request without `Sec-Fetch-Site` that has `Origin` passes when
//!    `Origin` is exactly an origin that the listener serves.
//! 3. A request with neither header passes to the Session check. A
//!    browser sends one of them on every POST and on every WebSocket
//!    handshake, so this request comes from a program, such as the Host
//!    socket of the Client App.
//!
//! Go checks every method except GET, HEAD and OPTIONS. This check also
//! applies to every request that asks for an upgrade, because a WebSocket
//! handshake is a GET. A browser sends no `Sec-Fetch-Site` on a WebSocket
//! handshake, so the `Origin` comparison decides every upgrade.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};

use crate::cors::is_public_origin;
use crate::error::ApiError;

/// The header in which a browser tells how the origin of the page relates
/// to the origin of the request.
const SEC_FETCH_SITE: &str = "sec-fetch-site";

/// Wrap every route of the router of one listener in the check. `served`
/// holds the origins that the listener serves: the Public Origin and, on
/// a local installation, the loopback origin on the product port, and its
/// own origin on the Administration Port.
pub fn refuse_other_origins(router: Router, served: Vec<String>) -> Router {
    router.layer(middleware::from_fn_with_state(
        Arc::<[String]>::from(served),
        check,
    ))
}

/// Refuse a browser request from an origin that is not in `served`.
async fn check(State(served): State<Arc<[String]>>, request: Request, next: Next) -> Response {
    if !admits(&served, request.method(), request.headers()) {
        return ApiError::forbidden("this port refuses a request from a page at another origin")
            .into_response();
    }
    next.run(request).await
}

/// Whether the check lets a request through to the routes. The
/// comparison with each origin of `served` is the one of the CORS answer:
/// an origin is scheme, host and port, compared as bytes.
fn admits(served: &[impl AsRef<str>], method: &Method, headers: &HeaderMap) -> bool {
    let reads = matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    if reads && !headers.contains_key(header::UPGRADE) {
        return true;
    }
    match headers.get(SEC_FETCH_SITE) {
        Some(site) => matches!(site.as_bytes(), b"same-origin" | b"none"),
        None => headers.get(header::ORIGIN).is_none_or(|origin| {
            served
                .iter()
                .any(|served| is_public_origin(served.as_ref(), origin))
        }),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    const PUBLIC: &str = "https://pagis.example.com";
    const SERVED: &[&str] = &[PUBLIC];

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_static(value));
        }
        headers
    }

    /// The same headers on a WebSocket handshake.
    fn handshake(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = headers(pairs);
        headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        headers
    }

    const WRITES: [Method; 4] = [Method::POST, Method::PUT, Method::DELETE, Method::PATCH];

    /// A read that asks for no upgrade changes nothing, so a link or an
    /// image on any page reaches it.
    #[test]
    fn a_read_passes_from_any_origin() {
        for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(admits(
                SERVED,
                &method,
                &headers(&[
                    ("origin", "https://files.example.com"),
                    ("sec-fetch-site", "cross-site"),
                ])
            ));
        }
    }

    /// `Sec-Fetch-Site` decides when the browser sends it, whatever
    /// `Origin` says.
    #[test]
    fn fetch_metadata_decides_when_the_browser_sends_it() {
        for method in WRITES {
            for site in ["same-origin", "none"] {
                assert!(admits(
                    SERVED,
                    &method,
                    &headers(&[("sec-fetch-site", site)])
                ));
            }
            for site in ["same-site", "cross-site", ""] {
                assert!(
                    !admits(
                        SERVED,
                        &method,
                        &headers(&[("sec-fetch-site", site), ("origin", PUBLIC)])
                    ),
                    "{method} {site:?}"
                );
            }
        }
    }

    /// Without `Sec-Fetch-Site`, `Origin` must be exactly the served
    /// origin. Another scheme, host or port is another origin, and so is
    /// the opaque `null` of a sandboxed frame.
    #[test]
    fn without_fetch_metadata_the_origin_must_be_the_served_one() {
        for method in WRITES {
            assert!(admits(SERVED, &method, &headers(&[("origin", PUBLIC)])));
            for other in [
                "https://files.example.com",
                "http://pagis.example.com",
                "https://pagis.example.com:8443",
                "https://pagis.example.com.evil.test",
                "null",
            ] {
                assert!(
                    !admits(SERVED, &method, &headers(&[("origin", other)])),
                    "{method} {other}"
                );
            }
        }
    }

    /// A WebSocket handshake is a GET, and the check applies to it as it
    /// does to a write.
    #[test]
    fn an_upgrade_is_checked_as_a_write_is() {
        assert!(admits(
            SERVED,
            &Method::GET,
            &handshake(&[("origin", PUBLIC)])
        ));
        assert!(admits(
            SERVED,
            &Method::GET,
            &handshake(&[("sec-fetch-site", "same-origin")])
        ));
        assert!(!admits(
            SERVED,
            &Method::GET,
            &handshake(&[("origin", "https://files.example.com")])
        ));
        assert!(!admits(
            SERVED,
            &Method::GET,
            &handshake(&[("origin", PUBLIC), ("sec-fetch-site", "same-site")])
        ));
    }

    /// A program that sends neither header, such as the Host socket of
    /// the Client App, passes to the Session check.
    #[test]
    fn a_request_with_neither_header_passes_to_the_session_check() {
        for method in WRITES {
            assert!(admits(SERVED, &method, &headers(&[("cookie", "s=1")])));
        }
        assert!(admits(
            SERVED,
            &Method::GET,
            &handshake(&[("sec-fetch-mode", "websocket")])
        ));
    }

    /// A listener that serves two origins admits each of them exactly,
    /// and no other.
    #[test]
    fn each_served_origin_is_admitted() {
        let served = [PUBLIC, "http://127.0.0.1:4400"];
        for origin in served {
            assert!(admits(
                &served,
                &Method::POST,
                &headers(&[("origin", origin)])
            ));
        }
        for other in ["http://127.0.0.1:5173", "http://localhost:4400"] {
            assert!(
                !admits(&served, &Method::POST, &headers(&[("origin", other)])),
                "{other}"
            );
        }
    }

    /// An empty served origin admits no `Origin` at all, rather than
    /// every request that sends one.
    #[test]
    fn an_empty_served_origin_admits_no_origin() {
        assert!(!admits(
            &[""],
            &Method::POST,
            &headers(&[("origin", "null")])
        ));
    }
}
