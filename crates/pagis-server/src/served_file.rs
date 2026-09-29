//! How the daemon serves a file that a Person, an Agent or a Software
//! Package supplies (OWASP File Upload Cheat Sheet).
//!
//! A browser must never run such a file as a document at the Product App
//! origin: its script would send requests with the Person's Session. So
//! the daemon shows only a passive type inline and serves every other
//! type as an `application/octet-stream` attachment. Each response also
//! carries `nosniff` and a `sandbox` policy. These two headers stop the
//! browser from running the bytes, even when the type or the disposition
//! is wrong.

use axum::http::{HeaderName, header};

/// The policy of every served file: the browser runs no script in it
/// and loads nothing for it, even when it shows the file as a page.
const SANDBOX_POLICY: &str = "sandbox; default-src 'none'";

/// The media types that the daemon shows inline. A browser shows each
/// one as an image, in a media player or as plain text, and runs no
/// script in it. The list names each type in full, because a family
/// such as `video/*` also holds `video/x+xml`, which a browser shows as
/// an XML document.
const PASSIVE_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "audio/aac",
    "audio/flac",
    "audio/mp4",
    "audio/mpeg",
    "audio/ogg",
    "audio/wav",
    "audio/webm",
    "video/mp4",
    "video/ogg",
    "video/webm",
    "text/plain",
];

/// The headers of one stored file: its passive type inline, or else
/// inert bytes to save. The served type comes from the list and never
/// from `mime`, which the client that stored the file chose.
pub fn headers(mime: &str, filename: Option<&str>) -> [(HeaderName, String); 4] {
    match passive_type(mime) {
        Some(passive) => file_headers(passive, "inline", filename),
        None => attachment(filename),
    }
}

/// The headers of a file that the browser saves and never shows.
pub fn attachment(filename: Option<&str>) -> [(HeaderName, String); 4] {
    file_headers("application/octet-stream", "attachment", filename)
}

fn file_headers(
    content_type: &str,
    disposition: &str,
    filename: Option<&str>,
) -> [(HeaderName, String); 4] {
    let disposition = match filename {
        Some(name) => format!(
            "{disposition}; filename=\"{}\"",
            name.replace(['"', '\\'], "_")
        ),
        None => disposition.to_string(),
    };
    [
        (header::CONTENT_TYPE, content_type.to_string()),
        (header::CONTENT_DISPOSITION, disposition),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        (header::CONTENT_SECURITY_POLICY, SANDBOX_POLICY.to_string()),
    ]
}

/// The listed type that `mime` names, in any letter case and with any
/// parameters.
fn passive_type(mime: &str) -> Option<&'static str> {
    let essence = mime.split(';').next().unwrap_or_default().trim();
    PASSIVE_TYPES
        .iter()
        .copied()
        .find(|passive| passive.eq_ignore_ascii_case(essence))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(served: &[(HeaderName, String); 4], name: HeaderName) -> &str {
        served
            .iter()
            .find(|(header, _)| *header == name)
            .map(|(_, value)| value.as_str())
            .expect("the header is present")
    }

    #[test]
    fn a_passive_type_shows_inline_as_itself() {
        for mime in PASSIVE_TYPES {
            let served = headers(mime, None);
            assert_eq!(value(&served, header::CONTENT_TYPE), *mime);
            assert_eq!(value(&served, header::CONTENT_DISPOSITION), "inline");
        }
    }

    #[test]
    fn the_served_type_drops_the_parameters_and_the_letter_case_of_the_stored_one() {
        let served = headers("IMAGE/PNG ; name=shot", None);
        assert_eq!(value(&served, header::CONTENT_TYPE), "image/png");

        let served = headers("text/plain; charset=utf-8", None);
        assert_eq!(value(&served, header::CONTENT_TYPE), "text/plain");
    }

    #[test]
    fn every_other_type_is_an_attachment_of_bytes() {
        for mime in [
            "text/html",
            "text/html;profile=mcp-app",
            "image/svg+xml",
            "application/pdf",
            "application/xhtml+xml",
            "text/xml",
            "video/mp4+xml",
            "application/octet-stream",
            "",
        ] {
            let served = headers(mime, None);
            assert_eq!(
                value(&served, header::CONTENT_TYPE),
                "application/octet-stream",
                "{mime}"
            );
            assert_eq!(
                value(&served, header::CONTENT_DISPOSITION),
                "attachment",
                "{mime}"
            );
        }
    }

    #[test]
    fn every_file_carries_nosniff_and_the_sandbox_policy() {
        for served in [
            headers("image/png", None),
            headers("text/html", None),
            attachment(None),
        ] {
            assert_eq!(value(&served, header::X_CONTENT_TYPE_OPTIONS), "nosniff");
            assert_eq!(
                value(&served, header::CONTENT_SECURITY_POLICY),
                "sandbox; default-src 'none'"
            );
        }
    }

    #[test]
    fn the_filename_cannot_close_its_quotes() {
        let served = headers("text/html", Some("a\"b\\c.html"));
        assert_eq!(
            value(&served, header::CONTENT_DISPOSITION),
            "attachment; filename=\"a_b_c.html\""
        );
    }
}
