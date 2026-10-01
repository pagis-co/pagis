//! The name of a browser Session, from the `User-Agent` of the browser
//! that signed in: "Safari on macOS", "Chrome on Android".
//!
//! The Person reads the name in their Sessions list, so they know which
//! Session to remove. It is a label and nothing more: a client can send
//! any `User-Agent`, so no rule reads it. The rules below name the
//! browsers and the systems that most people use, and a browser that
//! none of them names keeps the part that one of them does name.

use axum::http::HeaderMap;
use axum::http::header::USER_AGENT;

/// The browsers, in the order they are checked. A browser built on
/// another one names that one too (Edge says `Chrome/` and `Safari/`),
/// so the more specific token comes first.
const BROWSERS: &[(&str, &str)] = &[
    ("Edg/", "Edge"),
    ("EdgA/", "Edge"),
    ("EdgiOS/", "Edge"),
    ("OPR/", "Opera"),
    ("SamsungBrowser/", "Samsung Internet"),
    ("Firefox/", "Firefox"),
    ("FxiOS/", "Firefox"),
    ("CriOS/", "Chrome"),
    ("Chrome/", "Chrome"),
    ("Safari/", "Safari"),
];

/// The systems, in the order they are checked. An iPhone says `like Mac
/// OS X` and Android says `Linux`, so each comes before the system it
/// names.
const SYSTEMS: &[(&str, &str)] = &[
    ("iPhone", "iPhone"),
    ("iPad", "iPad"),
    ("Android", "Android"),
    ("CrOS", "ChromeOS"),
    ("Windows", "Windows"),
    ("Macintosh", "macOS"),
    ("Linux", "Linux"),
];

/// The name of the browser that sent these headers, or `None` where
/// its `User-Agent` names no browser and no system of the lists.
pub fn browser_session_name(headers: &HeaderMap) -> Option<String> {
    let agent = headers.get(USER_AGENT)?.to_str().ok()?;
    name_of(agent)
}

fn name_of(agent: &str) -> Option<String> {
    let first = |table: &[(&str, &'static str)]| {
        table
            .iter()
            .find(|(token, _)| agent.contains(token))
            .map(|(_, name)| *name)
    };
    match (first(BROWSERS), first(SYSTEMS)) {
        (Some(browser), Some(system)) => Some(format!("{browser} on {system}")),
        (Some(browser), None) => Some(browser.to_string()),
        (None, Some(system)) => Some(format!("Browser on {system}")),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_common_browsers_are_named_with_their_system() {
        for (agent, name) in [
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) Version/18.5 Safari/605.1.15",
                "Safari on macOS",
            ),
            (
                "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/139.0.0.0 Mobile Safari/537.36",
                "Chrome on Android",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1",
                "Safari on iPhone",
            ),
            (
                "Mozilla/5.0 (iPad; CPU OS 18_5 like Mac OS X) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) CriOS/139.0.7258.76 Mobile/15E148 Safari/604.1",
                "Chrome on iPad",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:142.0) Gecko/20100101 \
                 Firefox/142.0",
                "Firefox on Windows",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) FxiOS/142.0 Mobile/15E148 Safari/605.1.15",
                "Firefox on iPhone",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like \
                 Gecko) Chrome/139.0.0.0 Safari/537.36 Edg/139.0.3405.86",
                "Edge on Windows",
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/139.0.0.0 Safari/537.36 OPR/121.0.0.0",
                "Opera on Linux",
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; SM-S918B) AppleWebKit/537.36 (KHTML, like \
                 Gecko) SamsungBrowser/28.0 Chrome/130.0.0.0 Mobile Safari/537.36",
                "Samsung Internet on Android",
            ),
            (
                "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like \
                 Gecko) Chrome/139.0.0.0 Safari/537.36",
                "Chrome on ChromeOS",
            ),
        ] {
            assert_eq!(name_of(agent).as_deref(), Some(name), "{agent}");
        }
    }

    #[test]
    fn a_browser_or_a_system_alone_keeps_the_part_that_is_known() {
        assert_eq!(
            name_of("Mozilla/5.0 (X11; Linux x86_64) Lynx").as_deref(),
            Some("Browser on Linux")
        );
        assert_eq!(
            name_of("Mozilla/5.0 Firefox/142.0").as_deref(),
            Some("Firefox")
        );
    }

    #[test]
    fn a_client_that_names_nothing_known_has_no_name() {
        assert_eq!(name_of("curl/8.7.1"), None);
        assert_eq!(name_of(""), None);
        assert_eq!(browser_session_name(&HeaderMap::new()), None);
    }

    #[test]
    fn the_name_comes_from_the_user_agent_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Version/18.5 Safari/605.1.15"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            browser_session_name(&headers).as_deref(),
            Some("Safari on macOS")
        );
    }
}
