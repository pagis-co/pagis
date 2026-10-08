//! The `veo` codec: Google Veo video generation on the Gemini API.
//!
//! Video only. Submitting starts a long-running operation
//! (`:predictLongRunning`); polling reads the operation; the artifact is a
//! signed URI fetched with the API key. Chat and the other modalities report
//! as unsupported.

use serde_json::{Value, json};

use crate::config::ProviderConfig;
use crate::error::{Error, ErrorKind};
use crate::protocol::Protocol;
use crate::types::{ImageInput, SizeSpec, VideoJob, VideoRequest, VideoStatus};

pub struct Veo;

impl Protocol for Veo {
    fn parse_error(&self, provider_key: &str, status: u16, body: &[u8]) -> Error {
        let raw: Option<Value> = serde_json::from_slice(body).ok();
        // Google errors come as `{"error": {"code", "message", "status"}}`.
        let message = raw
            .as_ref()
            .and_then(|v| v.pointer("/error/message"))
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

    fn build_video_create_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        model: &str,
        req: &VideoRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let mut instance = json!({"prompt": req.prompt});
        if let Some(input) = &req.input_image {
            let image = match input {
                ImageInput::B64 { data, media_type } => {
                    json!({"bytesBase64Encoded": data, "mimeType": media_type})
                }
                ImageInput::Url { .. } => {
                    return Err(Error::InvalidConfig(format!(
                        "provider `{provider_key}` takes input images as base64 bytes, not URLs"
                    )));
                }
            };
            instance["image"] = image;
        }
        let mut parameters = serde_json::Map::new();
        if let Some(seconds) = req.seconds {
            parameters.insert("durationSeconds".into(), json!(seconds));
        }
        match &req.size {
            None => {}
            Some(SizeSpec::Aspect { ratio, tier }) => {
                parameters.insert("aspectRatio".into(), json!(ratio));
                if let Some(tier) = tier {
                    parameters.insert("resolution".into(), json!(tier));
                }
            }
            Some(SizeSpec::Pixels { .. }) => {
                return Err(Error::InvalidConfig(format!(
                    "provider `{provider_key}` takes aspect-ratio sizes; use SizeSpec::Aspect"
                )));
            }
        }
        for (key, value) in &req.extra {
            parameters.insert(key.clone(), value.clone());
        }
        let body = json!({"instances": [instance], "parameters": parameters});
        let url = format!("{}/models/{model}:predictLongRunning", provider.base_url);
        auth(provider_key, provider, http.post(url)).map(|request| request.json(&body))
    }

    fn parse_video_job(
        &self,
        provider_key: &str,
        model: &str,
        body: &[u8],
    ) -> Result<VideoJob, Error> {
        let wire: Value = serde_json::from_slice(body).map_err(|e| Error::InvalidResponse {
            provider: provider_key.to_owned(),
            message: format!("failed to decode operation: {e}"),
        })?;
        let name = wire
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::InvalidResponse {
                provider: provider_key.to_owned(),
                message: "operation has no name".to_owned(),
            })?
            .to_owned();
        let done = wire.get("done").and_then(Value::as_bool).unwrap_or(false);
        let error = wire
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::to_owned);
        // The signed artifact URI, wherever the API version puts it.
        let video_url = wire
            .pointer("/response/generateVideoResponse/generatedSamples/0/video/uri")
            .or_else(|| wire.pointer("/response/generatedVideos/0/video/uri"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let status = match (done, &error) {
            (false, _) => VideoStatus::InProgress,
            (true, Some(_)) => VideoStatus::Failed,
            (true, None) => VideoStatus::Completed,
        };
        Ok(VideoJob {
            id: name,
            provider: provider_key.to_owned(),
            model: model.to_owned(),
            status,
            progress: None,
            created_at: None,
            expires_at: None,
            video_url,
            error,
        })
    }

    fn build_video_status_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        native_id: &str,
    ) -> Result<reqwest::RequestBuilder, Error> {
        // The native id is the operation name, e.g.
        // `models/veo-3.0-generate-001/operations/abc123`. Job ids are
        // caller input: reject anything that would rewrite the request
        // path or query.
        let traversal = native_id
            .split('/')
            .any(|segment| segment.is_empty() || segment == "..");
        if native_id.is_empty() || traversal || native_id.contains(['?', '#']) {
            return Err(Error::InvalidConfig(format!(
                "`{native_id}` is not a valid operation name"
            )));
        }
        let url = format!("{}/{native_id}", provider.base_url);
        auth(provider_key, provider, http.get(url))
    }

    fn build_video_content_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        _native_id: &str,
        job: &VideoJob,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let url = job
            .video_url
            .as_ref()
            .ok_or_else(|| Error::InvalidResponse {
                provider: provider_key.to_owned(),
                message: "completed operation has no video URI".to_owned(),
            })?;
        // The URI comes from the provider's response and the request
        // carries the API key: require the provider's own host over a
        // secure scheme, so a hostile response cannot pull the key to
        // another host.
        validate_artifact_url(provider_key, provider, url)?;
        auth(provider_key, provider, http.get(url))
    }
}

/// Require the artifact URI to sit on the provider's host, over `https` (or
/// `http` to a loopback host, matching the base-URL rule).
fn validate_artifact_url(
    provider_key: &str,
    provider: &ProviderConfig,
    url: &str,
) -> Result<(), Error> {
    let outside = || Error::InvalidResponse {
        provider: provider_key.to_owned(),
        message: format!("operation returned a video URI outside the provider host: `{url}`"),
    };
    let parsed = reqwest::Url::parse(url).map_err(|_| outside())?;
    let base = reqwest::Url::parse(&provider.base_url).map_err(|_| outside())?;
    let secure = match parsed.scheme() {
        "https" => true,
        "http" => crate::router::is_local_host(&parsed),
        _ => false,
    };
    if !secure || parsed.host_str() != base.host_str() {
        return Err(outside());
    }
    Ok(())
}

fn auth(
    provider_key: &str,
    provider: &ProviderConfig,
    mut request: reqwest::RequestBuilder,
) -> Result<reqwest::RequestBuilder, Error> {
    for (name, value) in &provider.headers {
        request = request.header(name, value);
    }
    if !provider.api_key.is_empty() {
        let key = crate::protocol::sensitive_header(provider_key, &provider.api_key)?;
        request = request.header("x-goog-api-key", key);
    }
    Ok(request)
}
