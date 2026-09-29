//! The `elevenlabs` codec: ElevenLabs text-to-speech.
//!
//! Speech only. The voice id goes in the URL path and the output format in a
//! query param; the body carries the text and model. Chat and the other
//! modalities report as unsupported.

use serde_json::{Value, json};

use crate::config::ProviderConfig;
use crate::error::{Error, ErrorKind};
use crate::protocol::Protocol;
use crate::types::{AudioFormat, SpeechRequest};

pub struct ElevenLabs;

impl Protocol for ElevenLabs {
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
            raw,
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
        let mut request = http.post(url).json(&body);
        for (name, value) in &provider.headers {
            request = request.header(name, value);
        }
        if !provider.api_key.is_empty() {
            let key = crate::protocol::sensitive_header(provider_key, &provider.api_key)?;
            request = request.header("xi-api-key", key);
        }
        Ok(request)
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
