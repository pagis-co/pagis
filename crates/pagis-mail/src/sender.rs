//! The sender check of inbound mail (ADR-0019).
//!
//! A `From` header is text that the sender writes. It selects a
//! candidate tier and proves nothing. The proof is the mail host's own
//! authentication result: the one Authentication-Results header whose
//! authserv-id is the Mailbox Provider's (RFC 8601), with a DMARC pass
//! that aligns with the `From` domain (RFC 7489). A copy with another
//! authserv-id can come from any host on the way, or from the sender,
//! so the check does not read it.
//!
//! More than one copy with the provider's authserv-id makes the sender
//! Unknown. A host can write its own copy below the headers of the
//! message and keep a copy that the sender wrote above them, so the
//! position of a copy does not prove which copy the host wrote. A host
//! that removes the copies the sender wrote (RFC 8601 section 5) leaves
//! one copy, and the check reads it.
//!
//! A message whose `From` headers name more than one mailbox is Unknown
//! too. The DMARC check reads one author and the tier would read
//! another, so the message has no one sender (RFC 7489 section 6.6.1).
//!
//! The reader takes only what the check needs from RFC 8601: the
//! authserv-id, and the method, result and properties of each result.
//! Comments and quoted strings are skipped as the grammar says, so a
//! word inside them is never read as a result.

use crate::events::sender_domain;
use crate::transport::MessageSummary;

/// The name of the header a receiving host writes its checks in.
pub(crate) const AUTHENTICATION_RESULTS: &str = "Authentication-Results";

/// What the check found for one message. Only
/// [`SenderVerification::AlignedDmarcPass`] lets a candidate tier hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenderVerification {
    /// The one result of the Mailbox Provider shows a DMARC pass that
    /// aligns with the `From` domain.
    AlignedDmarcPass,
    /// The mailbox is on a manual host. Pagis knows no authserv-id for
    /// it, so it cannot tell the host's result from a forged one.
    ManualHost,
    /// The `From` headers name more than one mailbox.
    SeveralAuthors,
    /// No Authentication-Results header carries the Mailbox Provider's
    /// authserv-id, or the evidence holds no result at all.
    NoResult,
    /// More than one Authentication-Results header carries the Mailbox
    /// Provider's authserv-id, so no copy proves the host's result.
    SeveralResults,
    /// The Mailbox Provider's result shows no DMARC pass.
    NoDmarcPass,
    /// The DMARC pass is for a domain that does not align with the
    /// `From` domain, as on a forwarded message.
    NotAligned,
}

impl SenderVerification {
    /// Whether the evidence confirms the candidate tier.
    pub fn is_verified(self) -> bool {
        self == SenderVerification::AlignedDmarcPass
    }

    /// The reason, as the event metadata records it.
    pub fn as_str(self) -> &'static str {
        match self {
            SenderVerification::AlignedDmarcPass => "aligned_dmarc_pass",
            SenderVerification::ManualHost => "manual_host",
            SenderVerification::SeveralAuthors => "several_authors",
            SenderVerification::NoResult => "no_result",
            SenderVerification::SeveralResults => "several_results",
            SenderVerification::NoDmarcPass => "no_dmarc_pass",
            SenderVerification::NotAligned => "not_aligned",
        }
    }
}

/// What one message tells about its sender (ADR-0019): the `From`
/// value, which selects a candidate tier, and the check that confirms
/// it or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderEvidence {
    pub from: String,
    pub verification: SenderVerification,
}

impl SenderEvidence {
    /// The evidence one message carries, read against the authserv-ids
    /// of the Mailbox Provider that received it.
    pub fn of_message(summary: &MessageSummary, authserv_ids: &[&str]) -> Self {
        Self {
            from: summary.from.clone(),
            verification: verify_sender(authserv_ids, summary),
        }
    }
}

/// Check the sender of one message. `authserv_ids` holds the ids of the
/// Mailbox Provider that received it, and an empty list is a manual
/// host.
///
/// Every DMARC result in the provider's header must be a pass that
/// aligns, and at least one must be present. Alignment is relaxed
/// (RFC 7489 3.1): the two domains share one organizational domain.
pub fn verify_sender(authserv_ids: &[&str], summary: &MessageSummary) -> SenderVerification {
    if authserv_ids.is_empty() {
        return SenderVerification::ManualHost;
    }
    if summary.from_mailboxes > 1 {
        return SenderVerification::SeveralAuthors;
    }
    let mut provider_results = summary
        .authentication_results
        .iter()
        .filter_map(|value| AuthenticationResult::parse(value))
        .filter(|result| {
            authserv_ids
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&result.authserv_id))
        });
    let Some(result) = provider_results.next() else {
        return SenderVerification::NoResult;
    };
    if provider_results.next().is_some() {
        return SenderVerification::SeveralResults;
    }
    let dmarc: Vec<&MethodResult> = result
        .results
        .iter()
        .filter(|method| method.method == "dmarc")
        .collect();
    if dmarc.is_empty() || dmarc.iter().any(|method| method.result != "pass") {
        return SenderVerification::NoDmarcPass;
    }
    let Some(from_domain) = sender_domain(&summary.from) else {
        return SenderVerification::NotAligned;
    };
    let aligned = dmarc.iter().all(|method| {
        method
            .property("header.from")
            .is_some_and(|domain| aligned(&from_domain, domain))
    });
    match aligned {
        true => SenderVerification::AlignedDmarcPass,
        false => SenderVerification::NotAligned,
    }
}

/// Whether the DMARC domain aligns with the `From` domain in relaxed
/// mode. A domain with no public suffix aligns with itself alone.
fn aligned(from_domain: &str, dmarc_domain: &str) -> bool {
    let from_domain = normal_domain(from_domain);
    let dmarc_domain = normal_domain(
        dmarc_domain
            .rsplit_once('@')
            .map_or(dmarc_domain, |(_, domain)| domain),
    );
    if from_domain == dmarc_domain {
        return true;
    }
    match (
        psl::domain_str(&from_domain),
        psl::domain_str(&dmarc_domain),
    ) {
        (Some(from), Some(dmarc)) => from == dmarc,
        _ => false,
    }
}

fn normal_domain(domain: &str) -> String {
    domain.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// One Authentication-Results header, as far as the check reads it.
struct AuthenticationResult {
    /// Lowercased.
    authserv_id: String,
    results: Vec<MethodResult>,
}

/// One `method=result` with its properties, for example
/// `dmarc=pass header.from=example.com`.
struct MethodResult {
    /// Lowercased, without a method version.
    method: String,
    /// Lowercased.
    result: String,
    /// The `ptype.property` name, lowercased, and its value.
    properties: Vec<(String, String)>,
}

impl AuthenticationResult {
    /// Read one header value. A value with no authserv-id is not a
    /// result at all.
    fn parse(value: &str) -> Option<Self> {
        let text = without_comments(value);
        let mut parts = split_outside_quotes(&text, ';').into_iter();
        let authserv_id = parts
            .next()?
            .split_whitespace()
            .next()
            .map(|id| unquote(id).to_ascii_lowercase())
            .filter(|id| !id.is_empty())?;
        let results = parts
            .filter_map(|part| MethodResult::parse(&part))
            .collect();
        Some(Self {
            authserv_id,
            results,
        })
    }
}

impl MethodResult {
    /// Read one `resinfo`. `none`, the answer of a host that checked
    /// nothing, is no result.
    fn parse(part: &str) -> Option<Self> {
        let words = split_whitespace_outside_quotes(&without_space_at_signs(part));
        let mut words = words.into_iter();
        let (method, result) = words.next()?.split_once('=').map(|(method, result)| {
            let method = method.split('/').next().unwrap_or(method);
            (
                method.to_ascii_lowercase(),
                unquote(result).to_ascii_lowercase(),
            )
        })?;
        let properties = words
            .filter_map(|word| {
                let (name, value) = word.split_once('=')?;
                Some((name.to_ascii_lowercase(), unquote(value)))
            })
            .collect();
        Some(Self {
            method,
            result,
            properties,
        })
    }

    fn property(&self, name: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(held, _)| held == name)
            .map(|(_, value)| value.as_str())
    }
}

/// The value with every comment replaced by a space, and every line
/// break and tab read as a space. A comment nests and takes a
/// backslash escape (RFC 5322 3.2.2); a quoted string keeps its
/// parentheses.
fn without_comments(value: &str) -> String {
    let mut text = String::with_capacity(value.len());
    let mut depth = 0usize;
    let mut quoted = false;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        let c = if c.is_whitespace() { ' ' } else { c };
        if depth > 0 {
            match c {
                '\\' => {
                    chars.next();
                }
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                text.push(' ');
            }
            continue;
        }
        if quoted {
            text.push(c);
            match c {
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        text.push(escaped);
                    }
                }
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '(' => depth = 1,
            '"' => {
                quoted = true;
                text.push(c);
            }
            _ => text.push(c),
        }
    }
    text
}

/// The text split at every `separator` outside a quoted string.
fn split_outside_quotes(text: &str, separator: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut quoted = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        let part = parts.last_mut().expect("one part at least");
        match c {
            '\\' if quoted => {
                part.push(c);
                if let Some(escaped) = chars.next() {
                    part.push(escaped);
                }
            }
            '"' => {
                quoted = !quoted;
                part.push(c);
            }
            c if c == separator && !quoted => parts.push(String::new()),
            c => part.push(c),
        }
    }
    parts
}

/// The text with the space around `=`, `.` and `/` removed outside a
/// quoted string, so `dmarc = pass header . from = x` reads as
/// `dmarc=pass header.from=x`. The grammar allows space at each of
/// those signs, and a value never holds a space outside quotes.
fn without_space_at_signs(text: &str) -> String {
    const SIGNS: [char; 3] = ['=', '.', '/'];
    let mut joined = String::with_capacity(text.len());
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            joined.push(c);
            match c {
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        joined.push(escaped);
                    }
                }
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        if c == ' ' {
            while chars.peek() == Some(&' ') {
                chars.next();
            }
            let before = joined.chars().last();
            let after = chars.peek().copied();
            let at_sign = before.is_some_and(|before| SIGNS.contains(&before))
                || after.is_some_and(|after| SIGNS.contains(&after));
            if !at_sign {
                joined.push(' ');
            }
            continue;
        }
        if c == '"' {
            quoted = true;
        }
        joined.push(c);
    }
    joined
}

/// The words of the text, split at spaces outside a quoted string.
fn split_whitespace_outside_quotes(text: &str) -> Vec<String> {
    split_outside_quotes(text, ' ')
        .into_iter()
        .filter(|word| !word.is_empty())
        .collect()
}

/// A quoted string's content, with its escapes read. Any other value
/// is returned as it is.
fn unquote(value: &str) -> String {
    let Some(inner) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return value.to_string();
    };
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => text.extend(chars.next()),
            c => text.push(c),
        }
    }
    text
}
