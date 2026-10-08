//! The `deepgram` codec: Deepgram speech to text and text to speech.
//!
//! - Transcription: audio bytes go raw in the body, options as query
//!   params; the nested channels/alternatives response flattens into the
//!   neutral shape. A live session is `/listen` over a WebSocket
//!   ([`crate::realtime`]).
//! - Speech: `POST /speak` with the text in the body. A Deepgram voice is
//!   a model, such as `aura-2-thalia-en`, so the request's voice goes as
//!   the model.
//! - The model list: the public `/models` answers without a key, so it
//!   cannot prove one. The list reads the projects of the key first, and
//!   then the models of its first project. A speech model is one Aura
//!   generation whose voices are its models; a transcription model is one
//!   Nova generation.
//!
//! Chat and the other modalities report as unsupported.

use serde_json::{Value, json};

use crate::config::ProviderConfig;
use crate::error::{Error, ErrorKind};
use crate::protocol::{ModelPage, Protocol};
use crate::registry::{ListedModel, ListedVoice};
use crate::types::{
    AudioFormat, SpeechRequest, TranscriptWord, TranscriptionRequest, TranscriptionResponse,
};

pub struct Deepgram;

/// Add the provider's headers and the key, which Deepgram takes as a
/// `Token`.
fn authorized(
    provider_key: &str,
    provider: &ProviderConfig,
    mut request: reqwest::RequestBuilder,
) -> Result<reqwest::RequestBuilder, Error> {
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

/// One generation of models, in the order of the list, with the voices of
/// a speech generation.
fn generations(rows: &[Value], output: &str, voices: bool) -> Vec<ListedModel> {
    let mut models: Vec<ListedModel> = Vec::new();
    for row in rows {
        let Some(architecture) = row.get("architecture").and_then(Value::as_str) else {
            continue;
        };
        let index = match models.iter().position(|model| model.id == architecture) {
            Some(index) => index,
            None => {
                models.push(ListedModel {
                    output_modalities: Some(vec![output.to_owned()]),
                    voices: voices.then(Vec::new),
                    ..ListedModel::new(architecture)
                });
                models.len() - 1
            }
        };
        if let (Some(voices), Some(voice)) = (
            models[index].voices.as_mut(),
            row.get("canonical_name").and_then(Value::as_str),
        ) && !voices.iter().any(|known| known.id == voice)
        {
            voices.push(ListedVoice {
                id: voice.to_owned(),
                name: row
                    .pointer("/metadata/display_name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
    }
    models
}

impl Protocol for Deepgram {
    fn build_list_models_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        after: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let path = match after {
            None => "/projects".to_owned(),
            Some(project) => format!("/projects/{project}/models"),
        };
        authorized(
            provider_key,
            provider,
            http.get(format!("{}{path}", provider.base_url)),
        )
    }

    /// The first page is the projects of the key: it names the next page,
    /// the models of the first project. The second page is the models.
    fn parse_list_models(&self, provider_key: &str, body: &[u8]) -> Result<ModelPage, Error> {
        let wire: Value = serde_json::from_slice(body).map_err(|e| Error::InvalidResponse {
            provider: provider_key.to_owned(),
            message: format!("failed to decode the model list: {e}"),
        })?;
        if let Some(projects) = wire.get("projects").and_then(Value::as_array) {
            let project = projects
                .iter()
                .find_map(|project| project.get("project_id").and_then(Value::as_str))
                .ok_or_else(|| Error::InvalidResponse {
                    provider: provider_key.to_owned(),
                    message: "the key belongs to no project".to_owned(),
                })?;
            return Ok(ModelPage {
                models: Vec::new(),
                next: Some(project.to_owned()),
            });
        }
        let rows = |name: &str| {
            wire.get(name)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        let mut models = generations(&rows("stt"), "transcription", false);
        models.extend(generations(&rows("tts"), "speech", true));
        Ok(ModelPage { models, next: None })
    }

    fn build_speech_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        model: &str,
        req: &SpeechRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let voice = if req.voice.is_empty() {
            model
        } else {
            &req.voice
        };
        let mut query = vec![("model", voice.to_owned())];
        match req.format.unwrap_or(AudioFormat::Mp3) {
            AudioFormat::Mp3 => query.push(("encoding", "mp3".to_owned())),
            AudioFormat::Wav => query.push(("encoding", "linear16".to_owned())),
            AudioFormat::Pcm16 => {
                query.push(("encoding", "linear16".to_owned()));
                query.push(("container", "none".to_owned()));
            }
            AudioFormat::Flac => query.push(("encoding", "flac".to_owned())),
            AudioFormat::Aac => query.push(("encoding", "aac".to_owned())),
            AudioFormat::Opus => query.push(("encoding", "opus".to_owned())),
        }
        let mut body = json!({ "text": req.input });
        let object = body.as_object_mut().expect("body is an object");
        for (key, value) in &req.extra {
            object.insert(key.clone(), value.clone());
        }
        authorized(
            provider_key,
            provider,
            http.post(format!("{}/speak", provider.base_url))
                .query(&query)
                .json(&body),
        )
    }

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
            raw: raw.map(Box::new),
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
        authorized(
            provider_key,
            provider,
            http.post(format!("{}/listen", provider.base_url))
                .query(&query)
                .header(reqwest::header::CONTENT_TYPE, &req.media_type)
                .body(req.audio.clone()),
        )
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
