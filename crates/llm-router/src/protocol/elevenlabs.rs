//! The `elevenlabs` codec: ElevenLabs text to speech and speech to text.
//!
//! - Speech: the voice id goes in the URL path and the output format in a
//!   query param; the body carries the text and model.
//! - Transcription: Scribe takes a multipart upload of the file and the
//!   model, and answers the words with their times. A live session is
//!   Scribe's realtime socket ([`crate::realtime`]).
//! - The model list is `/v1/models` of the key, where the speech models say
//!   so. The voices belong to the account, not to a model: `/v2/voices`
//!   lists them a page at a time, and the model list gives them to each
//!   speech model.
//!
//! Chat and the other modalities report as unsupported.

use serde_json::{Value, json};

use crate::config::ProviderConfig;
use crate::error::{Error, ErrorKind};
use crate::protocol::{ModelPage, Protocol, VoicePage};
use crate::registry::{ListedModel, ListedVoice};
use crate::types::{
    AudioFormat, SpeechRequest, TranscriptWord, TranscriptionRequest, TranscriptionResponse,
};

pub struct ElevenLabs;

/// Add the provider's headers and the key, which ElevenLabs takes in
/// `xi-api-key`.
fn authorized(
    provider_key: &str,
    provider: &ProviderConfig,
    mut request: reqwest::RequestBuilder,
) -> Result<reqwest::RequestBuilder, Error> {
    for (name, value) in &provider.headers {
        request = request.header(name, value);
    }
    if !provider.api_key.is_empty() {
        let key = crate::protocol::sensitive_header(provider_key, &provider.api_key)?;
        request = request.header("xi-api-key", key);
    }
    Ok(request)
}

fn decode(provider_key: &str, body: &[u8], what: &str) -> Result<Value, Error> {
    serde_json::from_slice(body).map_err(|e| Error::InvalidResponse {
        provider: provider_key.to_owned(),
        message: format!("failed to decode the {what}: {e}"),
    })
}

impl Protocol for ElevenLabs {
    fn build_list_models_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        _after: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, Error> {
        authorized(
            provider_key,
            provider,
            http.get(format!("{}/models", provider.base_url)),
        )
    }

    /// The speech models of the key, in the provider's order. The list
    /// names no transcription model.
    fn parse_list_models(&self, provider_key: &str, body: &[u8]) -> Result<ModelPage, Error> {
        let wire = decode(provider_key, body, "model list")?;
        let models = wire
            .as_array()
            .into_iter()
            .flatten()
            .filter(|model| model["can_do_text_to_speech"] == true)
            .filter_map(|model| model["model_id"].as_str())
            .map(|id| ListedModel {
                output_modalities: Some(vec!["speech".to_owned()]),
                ..ListedModel::new(id)
            })
            .collect();
        Ok(ModelPage { models, next: None })
    }

    fn lists_voices_apart(&self) -> bool {
        true
    }

    /// `/v2/voices` sits beside the `/v1` base URL.
    fn build_list_voices_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        after: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let root = provider
            .base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1");
        let mut query = vec![("page_size", "100".to_owned())];
        if let Some(token) = after {
            query.push(("next_page_token", token.to_owned()));
        }
        authorized(
            provider_key,
            provider,
            http.get(format!("{root}/v2/voices")).query(&query),
        )
    }

    fn parse_list_voices(&self, provider_key: &str, body: &[u8]) -> Result<VoicePage, Error> {
        let wire = decode(provider_key, body, "voice list")?;
        let voices = wire["voices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|voice| {
                Some(ListedVoice {
                    id: voice["voice_id"].as_str()?.to_owned(),
                    name: voice["name"].as_str().map(str::to_owned),
                })
            })
            .collect();
        let next = (wire["has_more"] == true)
            .then(|| wire["next_page_token"].as_str().map(str::to_owned))
            .flatten();
        Ok(VoicePage { voices, next })
    }

    fn build_transcription_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        model: &str,
        req: &TranscriptionRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let file = reqwest::multipart::Part::bytes(req.audio.to_vec())
            .file_name("audio")
            .mime_str(&req.media_type)
            .map_err(|_| {
                Error::InvalidConfig(format!(
                    "provider `{provider_key}`: `{}` is not a valid media type",
                    req.media_type
                ))
            })?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", file)
            .text("model_id", model.to_owned())
            .text("timestamps_granularity", "word");
        if let Some(language) = &req.language {
            form = form.text("language_code", language.clone());
        }
        authorized(
            provider_key,
            provider,
            http.post(format!("{}/speech-to-text", provider.base_url))
                .multipart(form),
        )
    }

    fn parse_transcription_response(
        &self,
        provider_key: &str,
        model: &str,
        body: &[u8],
    ) -> Result<TranscriptionResponse, Error> {
        let wire = decode(provider_key, body, "transcription")?;
        let words = wire["words"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|word| word["type"] == "word")
            .map(|word| TranscriptWord {
                start_s: word["start"].as_f64().unwrap_or(0.0),
                end_s: word["end"].as_f64().unwrap_or(0.0),
                word: word["text"].as_str().unwrap_or_default().to_owned(),
            })
            .collect();
        Ok(TranscriptionResponse {
            provider: provider_key.to_owned(),
            model: model.to_owned(),
            text: wire["text"].as_str().unwrap_or_default().to_owned(),
            segments: Vec::new(),
            words,
            language: wire["language_code"].as_str().map(str::to_owned),
            duration_s: wire["audio_duration_secs"].as_f64(),
        })
    }

    fn parse_error(&self, provider_key: &str, status: u16, body: &[u8]) -> Error {
        let raw: Option<Value> = serde_json::from_slice(body).ok();
        // Errors come as `{"detail": {"status": ..., "message": ...}}`.
        let message = raw
            .as_ref()
            .and_then(|v| v.pointer("/detail/message"))
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

    fn build_speech_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        model: &str,
        req: &SpeechRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        // The voice id becomes one URL path segment; it may come from end
        // users, so restrict it to the id alphabet instead of letting it
        // rewrite the path or query.
        let valid_voice = !req.voice.is_empty()
            && req
                .voice
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        if !valid_voice {
            return Err(Error::InvalidConfig(format!(
                "provider `{provider_key}`: `{}` is not a valid voice id",
                req.voice
            )));
        }
        let mut url = format!("{}/text-to-speech/{}", provider.base_url, req.voice);
        if let Some(format) = req.format {
            let name = output_format(provider_key, format)?;
            url.push_str(&format!("?output_format={name}"));
        }
        let mut body = json!({"text": req.input, "model_id": model});
        let obj = body.as_object_mut().expect("body is an object");
        for (key, value) in &req.extra {
            obj.insert(key.clone(), value.clone());
        }
        authorized(provider_key, provider, http.post(url).json(&body))
    }
}

/// The ElevenLabs `output_format` name. The API has no wav, flac, or aac.
fn output_format(provider_key: &str, format: AudioFormat) -> Result<&'static str, Error> {
    match format {
        AudioFormat::Mp3 => Ok("mp3_44100_128"),
        AudioFormat::Opus => Ok("opus_48000_64"),
        AudioFormat::Pcm16 => Ok("pcm_24000"),
        AudioFormat::Wav | AudioFormat::Flac | AudioFormat::Aac => {
            Err(Error::InvalidConfig(format!(
                "provider `{provider_key}` does not offer `{}` output; use mp3, opus, or pcm16",
                format.wire_name()
            )))
        }
    }
}
