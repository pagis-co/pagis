//! The one origin this installation is served from.
//!
//! The web UI is served by the daemon itself, so every call the product
//! makes is same-origin and needs no CORS answer at all. The layer is
//! here for the calls the product does not make: a page on another
//! origin that tries to read this API with the browser's Session cookie.
//! Naming exactly one origin, and refusing every other, is what keeps
//! that page from reading a person's data.
//!
//! The origin is the Public Origin: the address a browser reaches this
//! installation at, which is the reverse proxy's on a server and
//! loopback on a local installation.

use axum::http::{HeaderName, HeaderValue, Method};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// Whether an `Origin` header is the Public Origin. An origin is scheme,
/// host and port, compared as bytes: a browser sends it in exactly the
/// form it derived from the URL.
pub fn is_public_origin(public_origin: &str, origin: &HeaderValue) -> bool {
    !public_origin.is_empty() && origin.as_bytes() == public_origin.as_bytes()
}

/// The CORS answer of this installation: credentialed requests from the
/// Public Origin, and no answer for any other origin.
pub fn cors_layer(public_origin: &str) -> CorsLayer {
    let public_origin = public_origin.to_string();
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin, _parts| {
            is_public_origin(&public_origin, origin)
        }))
        // The cookie travels with the request, so the browser needs this
        // and refuses a wildcard beside it.
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            HeaderName::from_static("content-type"),
            HeaderName::from_static("accept"),
        ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_public_origin_is_the_only_one_named() {
        let origin = "https://pagis.example.net";
        assert!(is_public_origin(
            origin,
            &HeaderValue::from_static("https://pagis.example.net")
        ));
        // Another scheme, another host and another port are each another
        // origin.
        assert!(!is_public_origin(
            origin,
            &HeaderValue::from_static("http://pagis.example.net")
        ));
        assert!(!is_public_origin(
            origin,
            &HeaderValue::from_static("https://pagis.example.net.evil.test")
        ));
        assert!(!is_public_origin(
            origin,
            &HeaderValue::from_static("https://pagis.example.net:8443")
        ));
        assert!(!is_public_origin(origin, &HeaderValue::from_static("null")));
    }

    /// An installation with no Public Origin names no origin, rather
    /// than matching every request that sends none.
    #[test]
    fn an_empty_public_origin_names_nothing() {
        assert!(!is_public_origin("", &HeaderValue::from_static("null")));
        assert!(!is_public_origin(
            "",
            &HeaderValue::from_static("http://127.0.0.1:4400")
        ));
    }
}
