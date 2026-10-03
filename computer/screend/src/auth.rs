//! The control endpoint's bearer token. A container is a
//! boundary between tenants, so a caller that reaches the port is not
//! yet a caller that may drive the screen: every control request
//! carries `Authorization: Bearer <token>`, and the token is the file
//! the daemon mounts into this container alone.
//!
//! The token is read once, at start, from the file named by
//! `PAGIS_SCREEND_TOKEN_FILE`, or from `/run/pagis/screend-token` when
//! that variable is absent. It never arrives as a command-line
//! argument or in the environment: every `docker exec`, the agent's own
//! shell among them, inherits the container environment and reads the
//! process table.

use std::path::{Path, PathBuf};

/// Where the token file is when the environment names no other path.
const DEFAULT_TOKEN_FILE: &str = "/run/pagis/screend-token";

/// The one request path that needs no token: it carries nothing of the
/// person, and both the image's readiness probe and the daemon's boot
/// probe call it before the daemon has a token to send.
pub const OPEN_PATH: &str = "/healthz";

/// The token the control endpoint demands. A guard with no token
/// refuses every request but `OPEN_PATH`: screend fails closed when
/// the token file is absent or empty.
pub struct Guard {
    token: Option<String>,
}

/// The token file this container reads: the path the environment
/// names, or the default one.
pub fn token_path() -> PathBuf {
    std::env::var_os("PAGIS_SCREEND_TOKEN_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_TOKEN_FILE))
}

impl Guard {
    /// Read the token at start from the file the environment names.
    pub fn from_env() -> Self {
        Self::from_file(&token_path())
    }

    /// Read the token from one file. A file that is absent, that this
    /// uid cannot read, or that holds only whitespace gives a closed
    /// guard.
    pub fn from_file(path: &Path) -> Self {
        let token = std::fs::read_to_string(path)
            .ok()
            .map(|text| text.trim_end().to_string())
            .filter(|token| !token.is_empty());
        Self { token }
    }

    /// Does this guard hold a token? A closed guard refuses every
    /// control request, and the caller says so once on stderr.
    pub fn is_closed(&self) -> bool {
        self.token.is_none()
    }

    /// The token itself. The Exit Proxy sends it to the daemon in `Home`
    /// mode, so the daemon knows which Computer asks (see `exit`).
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// May a request for `path` with this `Authorization` header value
    /// proceed?
    pub fn allows(&self, path: &str, authorization: Option<&str>) -> bool {
        if path == OPEN_PATH {
            return true;
        }
        let Some(token) = self.token.as_deref() else {
            return false;
        };
        bearer(authorization).is_some_and(|given| equal_in_constant_time(given, token))
    }
}

/// The token an `Authorization` header value carries. RFC 9110 makes
/// the scheme name case-insensitive.
fn bearer(authorization: Option<&str>) -> Option<&str> {
    let value = authorization?.trim();
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    Some(token.trim())
}

/// Compare two tokens in a time that does not depend on how many bytes
/// agree, so a caller cannot find the token one byte at a time. The
/// lengths are compared first: a token's length is fixed by the daemon
/// that writes it and is not the secret.
fn equal_in_constant_time(given: &str, expected: &str) -> bool {
    let (given, expected) = (given.as_bytes(), expected.as_bytes());
    if given.len() != expected.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in given.iter().zip(expected) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A guard over a token file of this content. Each test names its
    /// own file, because the tests run in parallel threads.
    fn guard_with(name: &str, contents: &str) -> Guard {
        let path = std::env::temp_dir().join(format!("screend-token-{name}"));
        std::fs::write(&path, contents).expect("write the test token file");
        let guard = Guard::from_file(&path);
        std::fs::remove_file(&path).expect("remove the test token file");
        guard
    }

    #[test]
    fn the_right_token_passes() {
        let guard = guard_with("right", "s3cret\n");
        assert!(guard.allows("/frame.png", Some("Bearer s3cret")));
    }

    #[test]
    fn the_scheme_name_is_case_insensitive() {
        let guard = guard_with("scheme", "s3cret");
        assert!(guard.allows("/frame.png", Some("bearer s3cret")));
    }

    #[test]
    fn a_wrong_or_missing_token_fails() {
        let guard = guard_with("wrong", "s3cret");
        assert!(!guard.allows("/frame.png", Some("Bearer s3cres")));
        assert!(!guard.allows("/frame.png", Some("Bearer s3cret_")));
        assert!(!guard.allows("/frame.png", Some("Basic s3cret")));
        assert!(!guard.allows("/frame.png", Some("s3cret")));
        assert!(!guard.allows("/frame.png", None));
    }

    #[test]
    fn healthz_needs_no_token() {
        let guard = guard_with("healthz", "s3cret");
        assert!(guard.allows(OPEN_PATH, None));
    }

    #[test]
    fn an_empty_file_closes_every_path_but_healthz() {
        let guard = guard_with("empty", "  \n");
        assert!(guard.is_closed());
        assert!(guard.allows(OPEN_PATH, None));
        assert!(!guard.allows("/frame.png", Some("Bearer ")));
        assert!(!guard.allows("/input", Some("Bearer anything")));
    }

    #[test]
    fn an_absent_file_closes_the_guard() {
        let guard = Guard::from_file(Path::new("/nonexistent/screend-token"));
        assert!(guard.is_closed());
        assert!(!guard.allows("/input", Some("Bearer anything")));
    }
}
