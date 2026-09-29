//! The registrable domain (ADR-0013).
//!
//! A fill is bound to its target by the Credential record: `login_url`
//! is one fixed `https` address, and its registrable domain must equal
//! the record's `domain`. The vault checks that when it writes a record
//! and again before every fill, so a record that was written correctly
//! cannot later point somewhere else. A secret sent over `http` is
//! readable on the network path, so an `http` address is refused.
//!
//! The page a fill writes into is checked with the same rule after
//! every redirect: its top-level address must be `https` and on the
//! record's registrable domain.
//!
//! The registrable domain comes from the public suffix list, the way
//! Apple Passwords matches. The list is compiled in through `psl`
//! rather than fetched: a daemon that is offline still has to resolve
//! `co.uk` the same way it did yesterday.

use crate::VaultError;

/// The registrable domain of one login address — `example.co.uk` for
/// `https://www.example.co.uk/login`. Only an `https` address resolves:
/// a secret sent over `http` is readable on the network path, and a
/// `javascript:` or `file:` address has no site to bind to.
pub fn registrable_domain_of_url(login_url: &str) -> Result<String, VaultError> {
    let parsed = url::Url::parse(login_url)
        .map_err(|error| VaultError::BadLoginUrl(format!("{login_url:?} is not a URL: {error}")))?;
    if parsed.scheme() != "https" {
        return Err(VaultError::BadLoginUrl(format!(
            "{login_url:?} must be an https address"
        )));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| VaultError::BadLoginUrl(format!("{login_url:?} carries no host")))?;
    registrable_domain(host)
        .ok_or_else(|| VaultError::BadLoginUrl(format!("{host:?} has no registrable domain")))
}

/// The registrable domain of one host name, lowercased.
pub fn registrable_domain(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    psl::domain_str(&host).map(str::to_string)
}

/// The record's two domain fields agree. This is the check that binds a
/// fill to its target.
pub fn check_login_url(domain: &str, login_url: &str) -> Result<(), VaultError> {
    let resolved = registrable_domain_of_url(login_url)?;
    let declared = registrable_domain(domain)
        .ok_or_else(|| VaultError::BadDomain(format!("{domain:?} is not a registrable domain")))?;
    if resolved != declared {
        return Err(VaultError::DomainMismatch {
            domain: declared,
            login_url_domain: resolved,
        });
    }
    Ok(())
}

/// The origin of a page a fill may write into: the top-level address
/// the daemon's tab shows after every redirect, when it is `https` and
/// its registrable domain is the record's domain. screend writes only
/// while the page is on this origin. A single sign-on host on another
/// registrable domain needs a Credential of its own.
pub fn verified_origin(domain: &str, page_url: &str) -> Result<String, VaultError> {
    let refuse = |reason: String| VaultError::FillFailed(reason);
    let declared = registrable_domain(domain)
        .ok_or_else(|| VaultError::BadDomain(format!("{domain:?} is not a registrable domain")))?;
    let parsed = url::Url::parse(page_url)
        .map_err(|_| refuse(format!("the page address {page_url:?} is not a URL")))?;
    if parsed.scheme() != "https" {
        return Err(refuse(format!(
            "the page is on {page_url:?}, and a fill writes only into an https page"
        )));
    }
    let host = parsed.host_str().unwrap_or_default();
    match registrable_domain(host) {
        Some(found) if found == declared => Ok(parsed.origin().ascii_serialization()),
        _ => Err(refuse(format!(
            "the page is on {host}, and this login is for {declared}: a fill writes only \
             into a page of {declared}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registrable_domain_drops_the_subdomains() {
        assert_eq!(
            registrable_domain_of_url("https://www.example.com/login").unwrap(),
            "example.com"
        );
        assert_eq!(
            registrable_domain_of_url("https://accounts.EXAMPLE.co.uk/in?x=1").unwrap(),
            "example.co.uk"
        );
    }

    #[test]
    fn only_web_addresses_resolve() {
        assert!(registrable_domain_of_url("javascript:alert(1)").is_err());
        assert!(registrable_domain_of_url("file:///etc/passwd").is_err());
        assert!(registrable_domain_of_url("not a url").is_err());
        assert!(registrable_domain_of_url("https://localhost/login").is_err());
    }

    #[test]
    fn an_http_login_address_is_refused() {
        let refused = check_login_url("example.com", "http://accounts.example.com/signin")
            .expect_err("an http login address is refused");

        assert!(matches!(refused, VaultError::BadLoginUrl(_)), "{refused:?}");
        assert!(refused.to_string().contains("https"), "{refused}");
    }

    #[test]
    fn a_page_on_the_records_https_site_gives_its_origin() {
        assert_eq!(
            verified_origin("example.com", "https://login.example.com/in?next=/").unwrap(),
            "https://login.example.com"
        );
        assert_eq!(
            verified_origin("EXAMPLE.com", "https://example.com:8443/in").unwrap(),
            "https://example.com:8443"
        );
    }

    #[test]
    fn a_page_off_the_records_https_site_gets_nothing() {
        for page in [
            "http://login.example.com/in",
            "https://login.example.org/in",
            "https://evil-example.com/in",
            "https://example.com.evil.test/in",
            "https://93.184.215.14/in",
            "about:blank",
            "chrome-error://chromewebdata/",
            "data:text/html,<input>",
            "not a url",
        ] {
            let refused = verified_origin("example.com", page).unwrap_err();
            assert!(
                matches!(refused, VaultError::FillFailed(_)),
                "{page}: {refused:?}"
            );
        }
    }

    #[test]
    fn the_record_binds_its_address_to_its_domain() {
        assert!(check_login_url("example.com", "https://www.example.com/in").is_ok());
        assert!(check_login_url("EXAMPLE.com", "https://example.com/in").is_ok());

        let mismatch = check_login_url("example.com", "https://evil.test/in").unwrap_err();
        assert!(
            matches!(mismatch, VaultError::DomainMismatch { .. }),
            "{mismatch:?}"
        );
        // A lookalike is a different registrable domain.
        assert!(check_login_url("example.com", "https://evil-example.com/in").is_err());
        // A subdomain of the record's domain is the same site.
        assert!(check_login_url("example.com", "https://login.example.com/").is_ok());
    }
}
