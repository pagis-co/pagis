//! The `deepgram` codec: Deepgram speech-to-text.
//!
//! Transcription only. Audio bytes go raw in the body, options as query
//! params; the nested channels/alternatives response flattens into the
//! neutral shape. Chat and the other modalities report as unsupported.

use serde_json::Value;

use crate::config::ProviderConfig;
use crate::error::{Error, ErrorKind};
use crate::protocol::Protocol;
use crate::types::{TranscriptWord, TranscriptionRequest, TranscriptionResponse};

pub struct Deepgram;

impl Protocol for Deepgram {
    fn parse_error(&self, provider_key: &str, status: u16, body: &[u8]) -> Error {
        let raw: Option<Value> = serde_json::from_slice(body).ok();
        // Errors come as `{"err_code": ..., "err_msg": ...}`.
        let message = raw
            .as_ref()
            .and_then(|v| v.get("err_msg").or_else(|| v.get("error")))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                String::from_utf8_lossy(&body[..body.len().min(2048)])
                    .trim()
                    .to_owned()
            });
        let kind = match status {
            401 | 403 => ErrorKind::Authentication,
            429 => ErrorKind::RateLimit,
            400..=499 => ErrorKind::InvalidRequest,
            503 => ErrorKind::Overloaded,
            500..=599 => ErrorKind::Server,
            _ => ErrorKind::Unknown,
        };
        Error::Provider {
            provider: provider_key.to_owned(),
            status,
            kind,
            message: crate::protocol::cap_error_text(message),
            raw,
        }
    }

    fn build_transcription_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        model: &str,
        req: &TranscriptionRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        // A query string cannot override a repeated param the way a JSON
        // map does, so a built-in param yields to `extra` under the same
        // name.
        let mut query: Vec<(String, String)> = Vec::new();
        if !req.extra.contains_key("model") {
            query.push(("model".into(), model.to_owned()));
        }
        if !req.extra.contains_key("smart_format") {
            query.push(("smart_format".into(), "true".into()));
        }
        if let Some(language) = &req.language
            && !req.extra.contains_key("language")
        {
            query.push(("language".into(), language.clone()));
        }
        for (key, value) in &req.extra {
            let text = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            query.push((key.clone(), text));
        }
        let mut request = http
            .post(format!("{}/listen", provider.base_url))
            .query(&query)
            .header(reqwest::header::CONTENT_TYPE, &req.media_type)
            .body(req.audio.clone());
        for (name, value) in &provider.headers {
            request = request.header(name, value);
        }
        if !provider.api_key.is_empty() {
            let key = crate::protocol::sensitive_header(
                provider_key,
                &format!("Token {}", provider.api_key),
            )?;
            request = request.header(reqwest::header::AUTHORIZATION, key);
        }
        Ok(request)
    }

    fn parse_transcription_response(
        &self,
        provider_key: &str,
        model: &str,
        body: &[u8],
    ) -> Result<TranscriptionResponse, Error> {
        let wire: Value = serde_json::from_slice(body).map_err(|e| Error::InvalidResponse {
            provider: provider_key.to_owned(),
            message: format!("failed to decode transcription: {e}"),
        })?;
        let alternative = wire
            .pointer("/results/channels/0/alternatives/0")
            .ok_or_else(|| Error::InvalidResponse {
                provider: provider_key.to_owned(),
                message: "response has no transcription alternatives".to_owned(),
            })?;
        let text = alternative
            .get("transcript")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let words = alternative
            .get("words")
            .and_then(Value::as_array)
            .map(|words| {
                words
                    .iter()
                    .map(|w| TranscriptWord {
                        start_s: w.get("start").and_then(Value::as_f64).unwrap_or(0.0),
                        end_s: w.get("end").and_then(Value::as_f64).unwrap_or(0.0),
                        word: w
                            .get("punctuated_word")
                            .or_else(|| w.get("word"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(TranscriptionResponse {
            provider: provider_key.to_owned(),
            model: model.to_owned(),
            text,
            segments: Vec::new(),
            words,
            language: wire
                .pointer("/results/channels/0/detected_language")
                .and_then(Value::as_str)
                .map(str::to_owned),
            duration_s: wire.pointer("/metadata/duration").and_then(Value::as_f64),
        })
    }
}
