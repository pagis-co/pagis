//! Decoders for the provider model lists.
//!
//! Every chat protocol has a list endpoint (`GET {base_url}/models`). The
//! endpoints agree on the id and disagree on the metadata, so each decoder
//! reads the fields its providers document and leaves the rest `None`:
//!
//! - Anthropic: `max_input_tokens`, `max_tokens`; newest first; cursor
//!   pages (`has_more`, `last_id`, `after_id`).
//! - OpenAI and compatible servers: an OpenAI `data` array of `id` and
//!   `created`. Compatible servers add their own limits: `context_length`
//!   (OpenRouter, Together), `context_window` (Groq), `max_context_length`
//!   (Mistral), `max_model_len` (vLLM), and `top_provider` (OpenRouter).
//! - Prices: OpenRouter's `pricing` holds USD per token as strings.
//!
//! A zero or negative number means "not reported", because providers use
//! both for an unknown value (OpenRouter prices a variable router at -1).

use serde::Deserialize;
use serde_json::Value;

use crate::error::Error;
use crate::protocol::ModelPage;
use crate::registry::{ListedModel, ModelPrices};

/// Decode an OpenAI-shaped list: one page, newest first when the server
/// reports `created`, otherwise in the server's order.
pub(super) fn parse_openai(provider_key: &str, body: &[u8]) -> Result<ModelPage, Error> {
    let wire: OpenAiList = decode(provider_key, body)?;
    let mut models: Vec<(Option<i64>, ListedModel)> = wire
        .data
        .into_iter()
        .map(|model| {
            let context_window = positive(model.context_length)
                .or(positive(model.context_window))
                .or(positive(model.max_context_length))
                .or(positive(model.max_model_len))
                .or(model
                    .top_provider
                    .as_ref()
                    .and_then(|top| positive(top.context_length)));
            let max_output_tokens = model
                .top_provider
                .as_ref()
                .and_then(|top| positive(top.max_completion_tokens))
                .or(positive(model.max_completion_tokens));
            let prices = model.pricing.as_ref().and_then(per_token_prices);
            (
                model.created,
                ListedModel {
                    id: model.id,
                    context_window,
                    max_output_tokens,
                    prices,
                },
            )
        })
        .collect();
    if models.iter().all(|(created, _)| created.is_some()) {
        models.sort_by_key(|(created, _)| std::cmp::Reverse(*created));
    }
    Ok(ModelPage {
        models: models.into_iter().map(|(_, model)| model).collect(),
        next: None,
    })
}

/// Decode one page of the Anthropic list, which is newest first.
pub(super) fn parse_anthropic(provider_key: &str, body: &[u8]) -> Result<ModelPage, Error> {
    let wire: AnthropicList = decode(provider_key, body)?;
    let next = match (wire.has_more, wire.last_id) {
        (true, Some(last)) => Some(last),
        (true, None) => {
            return Err(Error::InvalidResponse {
                provider: provider_key.to_owned(),
                message: "the model list has more pages and no `last_id`".to_owned(),
            });
        }
        (false, _) => None,
    };
    Ok(ModelPage {
        models: wire
            .data
            .into_iter()
            .map(|model| ListedModel {
                id: model.id,
                context_window: positive(model.max_input_tokens),
                max_output_tokens: positive(model.max_tokens),
                prices: None,
            })
            .collect(),
        next,
    })
}

fn decode<'a, T: Deserialize<'a>>(provider_key: &str, body: &'a [u8]) -> Result<T, Error> {
    serde_json::from_slice(body).map_err(|e| Error::InvalidResponse {
        provider: provider_key.to_owned(),
        message: format!("failed to decode the model list: {e}"),
    })
}

fn positive(value: Option<f64>) -> Option<u32> {
    value
        .filter(|value| value.is_finite() && *value >= 1.0)
        .map(|value| value.min(f64::from(u32::MAX)) as u32)
}

/// OpenRouter's `pricing`: USD per token, as decimal strings. The result
/// is USD per million tokens. A model without a prompt and a completion
/// price has no known price.
fn per_token_prices(pricing: &Value) -> Option<ModelPrices> {
    let rate = |field: &str| -> Option<f64> {
        let value = pricing.get(field)?;
        let per_token = match value {
            Value::String(text) => text.parse::<f64>().ok()?,
            Value::Number(number) => number.as_f64()?,
            _ => return None,
        };
        (per_token.is_finite() && per_token >= 0.0).then_some(per_token * 1e6)
    };
    Some(ModelPrices {
        input_cost: rate("prompt")?,
        output_cost: rate("completion")?,
        cache_read_cost: rate("input_cache_read").unwrap_or_default(),
        cache_write_cost: rate("input_cache_write").unwrap_or_default(),
    })
}

#[derive(Deserialize)]
struct OpenAiList {
    data: Vec<OpenAiModel>,
}

#[derive(Deserialize)]
struct OpenAiModel {
    id: String,
    #[serde(default)]
    created: Option<i64>,
    #[serde(default)]
    context_length: Option<f64>,
    #[serde(default)]
    context_window: Option<f64>,
    #[serde(default)]
    max_context_length: Option<f64>,
    #[serde(default)]
    max_model_len: Option<f64>,
    #[serde(default)]
    max_completion_tokens: Option<f64>,
    #[serde(default)]
    top_provider: Option<TopProvider>,
    #[serde(default)]
    pricing: Option<Value>,
}

#[derive(Deserialize)]
struct TopProvider {
    #[serde(default)]
    context_length: Option<f64>,
    #[serde(default)]
    max_completion_tokens: Option<f64>,
}

#[derive(Deserialize)]
struct AnthropicList {
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    last_id: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicModel {
    id: String,
    #[serde(default)]
    max_input_tokens: Option<f64>,
    #[serde(default)]
    max_tokens: Option<f64>,
}
