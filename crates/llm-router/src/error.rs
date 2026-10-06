//! Error taxonomy.
//!
//! Provider errors normalize to a closed [`ErrorKind`] set; the raw provider
//! payload stays available in [`Error::Provider::raw`]. The classification
//! drives the router's fallback decision: see [`Error::is_retryable`].

use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("unknown model alias `{0}`")]
    UnknownModel(String),

    #[error("unknown provider `{0}`")]
    UnknownProvider(String),

    #[error("transport error calling provider `{provider}`: {source}")]
    Transport {
        provider: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("provider `{provider}` returned status {status} ({kind:?}): {message}")]
    Provider {
        provider: String,
        status: u16,
        kind: ErrorKind,
        message: String,
        /// The raw provider error payload, when it was parseable JSON.
        raw: Option<Value>,
    },

    #[error("invalid response from provider `{provider}`: {message}")]
    InvalidResponse { provider: String, message: String },

    #[error("stream error from provider `{provider}`: {message}")]
    Stream { provider: String, message: String },

    #[error("timed out waiting for the first stream event from provider `{provider}`")]
    FirstEventTimeout { provider: String },

    #[error("provider `{provider}` does not support {feature}")]
    Unsupported {
        provider: String,
        feature: &'static str,
    },

    #[error("realtime connection to provider `{provider}` failed: {message}")]
    Realtime { provider: String, message: String },

    #[error("video job `{id}` has no artifact yet: status {status:?}")]
    JobNotReady {
        id: String,
        status: crate::types::VideoStatus,
    },

    #[error("all candidates for model `{model}` failed: {last}")]
    Exhausted {
        model: String,
        /// Provider calls made across all candidates.
        attempts: u32,
        #[source]
        last: Box<Error>,
    },
}

/// Normalized provider error classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Authentication,
    RateLimit,
    ContextLength,
    InvalidRequest,
    ContentFilter,
    Overloaded,
    Server,
    Unknown,
}

impl Error {
    /// True when retrying the same candidate may succeed, so the run loop
    /// sleeps and tries again before it falls through.
    pub fn retries_same_candidate(&self) -> bool {
        match self {
            Error::Provider { status, .. } => *status == 429 || (500..=504).contains(status),
            Error::Transport { .. } | Error::FirstEventTimeout { .. } | Error::Realtime { .. } => {
                true
            }
            _ => false,
        }
    }

    /// True when the provider refused the key: status 401, or an error
    /// the provider classifies as authentication. An exhausted route
    /// answers for its last candidate.
    pub fn refuses_key(&self) -> bool {
        match self {
            Error::Provider { status, kind, .. } => {
                *status == 401 || *kind == ErrorKind::Authentication
            }
            Error::Exhausted { last, .. } => last.refuses_key(),
            _ => false,
        }
    }

    /// The HTTP status and the parsed error body of the provider answer
    /// that ended the call, through an exhausted route to its last error.
    /// `None` when no provider answered with an error status.
    pub fn provider_response(&self) -> Option<(u16, Option<&Value>)> {
        match self {
            Error::Provider { status, raw, .. } => Some((*status, raw.as_ref())),
            Error::Exhausted { last, .. } => last.provider_response(),
            _ => None,
        }
    }

    /// Provider calls made before this error reached the caller.
    pub fn attempts(&self) -> u32 {
        match self {
            Error::Exhausted { attempts, .. } => *attempts,
            _ => 1,
        }
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Provider { status, kind, .. } => {
                *status == 429
                    || (500..=504).contains(status)
                    || (*status != 400
                        && !matches!(kind, ErrorKind::InvalidRequest | ErrorKind::ContentFilter))
            }
            // Unsupported falls through so an alias can mix providers with
            // different capabilities.
            // Realtime errors happen at connect time, before any frame flows,
            // so the next candidate can still serve the session.
            Error::Transport { .. }
            | Error::InvalidResponse { .. }
            | Error::FirstEventTimeout { .. }
            | Error::Realtime { .. }
            | Error::Unsupported { .. } => true,
            Error::Stream { .. } => false,
            Error::InvalidConfig(_)
            | Error::UnknownModel(_)
            | Error::UnknownProvider(_)
            | Error::JobNotReady { .. }
            | Error::Exhausted { .. } => false,
        }
    }
}

#[cfg(test)]
mod refused_key_tests {
    use super::*;

    fn provider(status: u16, kind: ErrorKind) -> Error {
        Error::Provider {
            provider: "anthropic".into(),
            status,
            kind,
            message: "invalid x-api-key".into(),
            raw: None,
        }
    }

    #[test]
    fn a_refused_key_is_a_401_or_an_authentication_error_through_the_route() {
        assert!(provider(401, ErrorKind::Unknown).refuses_key());
        assert!(provider(403, ErrorKind::Authentication).refuses_key());
        assert!(!provider(429, ErrorKind::RateLimit).refuses_key());
        assert!(
            Error::Exhausted {
                model: "default".into(),
                attempts: 1,
                last: Box::new(provider(401, ErrorKind::Authentication)),
            }
            .refuses_key()
        );
    }
}
