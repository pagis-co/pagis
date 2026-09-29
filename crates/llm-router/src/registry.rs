//! Model metadata: context limits and pricing.
//!
//! The built-in table ships as `models.json` (curated, dated). Config
//! `model_info` entries override and extend it. Lookup is by exact model id,
//! then by the longest key that is a dash-separated prefix of the id, so
//! `claude-sonnet-4-5` matches `claude-sonnet-4-5-20250929`. An aggregator
//! id such as `anthropic/claude-sonnet-4.6` that has no entry of its own
//! reads the entry of its last path segment.
//!
//! The table is not the list of models a key can use: the provider's own
//! model list is ([`ListedModel`]). [`ModelMetadata::layered`] combines the
//! two, field by field: what the provider reports, then the table, then a
//! conservative default for the limits. A price has no default, because a
//! made-up price is worse than an unknown one.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::types::Usage;

/// The context window of a model that neither its provider nor the table
/// describes. The chat models that the supported providers list have a
/// context window of 128,000 tokens or more, so this bound does not
/// refuse a request that the model can take.
pub const DEFAULT_CONTEXT_WINDOW: u32 = 128_000;

/// The output limit of a model that neither its provider nor the table
/// describes. It is the output limit that the chat models of the
/// supported providers all accept.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 4_096;

/// Costs are USD per million tokens.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ModelPrices {
    pub input_cost: f64,
    pub output_cost: f64,
    #[serde(default)]
    pub cache_read_cost: f64,
    #[serde(default)]
    pub cache_write_cost: f64,
}

impl ModelPrices {
    /// The USD cost of `usage` at these rates.
    pub fn cost(&self, usage: &Usage) -> f64 {
        let non_cached = usage
            .input_tokens
            .saturating_sub(usage.cache_read_input_tokens)
            .saturating_sub(usage.cache_write_input_tokens);
        (non_cached as f64 * self.input_cost
            + usage.cache_read_input_tokens as f64 * self.cache_read_cost
            + usage.cache_write_input_tokens as f64 * self.cache_write_cost
            + usage.output_tokens as f64 * self.output_cost)
            / 1e6
    }
}

/// One entry of the metadata table.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    pub context_window: u32,
    pub max_output_tokens: u32,
    #[serde(flatten)]
    pub prices: ModelPrices,
}

impl ModelInfo {
    /// The USD cost of `usage` at this model's rates.
    pub fn cost(&self, usage: &Usage) -> f64 {
        self.prices.cost(usage)
    }
}

/// One model that a provider lists for a key, with the metadata the
/// provider reports for it. Each field the provider does not report is
/// `None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListedModel {
    /// The id to send in a request, e.g. `claude-sonnet-4-6` or, on an
    /// aggregator, `anthropic/claude-sonnet-4.6`.
    pub id: String,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub prices: Option<ModelPrices>,
}

/// Id fragments of the model families that do not take a chat turn:
/// embeddings, speech, transcription, images, video, moderation and
/// realtime audio. A provider's list mixes them with chat models and says
/// nothing about the kind, so a default pick skips them by name.
const NOT_CHAT: [&str; 13] = [
    "embed",
    "tts",
    "whisper",
    "transcribe",
    "dall-e",
    "image",
    "sora",
    "moderation",
    "realtime",
    "audio",
    "search",
    "rerank",
    "davinci",
];

impl ListedModel {
    /// Whether the id names a chat model, as far as its name tells. An id
    /// of a family that takes no chat turn answers `false`.
    pub fn looks_like_chat(&self) -> bool {
        let id = self.id.to_ascii_lowercase();
        !NOT_CHAT.iter().any(|fragment| id.contains(fragment))
    }

    /// A listed model with no metadata.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            context_window: None,
            max_output_tokens: None,
            prices: None,
        }
    }
}

/// The metadata to use for one model, after the layers: the limits are
/// always known, and the prices are `None` when no layer names them.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ModelMetadata {
    pub context_window: u32,
    pub max_output_tokens: u32,
    /// `None` means the cost is unknown, never zero.
    pub prices: Option<ModelPrices>,
}

impl ModelMetadata {
    /// Combine the layers field by field: the provider's report, then the
    /// table entry, then [`DEFAULT_CONTEXT_WINDOW`] and
    /// [`DEFAULT_MAX_OUTPUT_TOKENS`].
    pub fn layered(listed: Option<&ListedModel>, table: Option<&ModelInfo>) -> Self {
        let context_window = listed
            .and_then(|model| model.context_window)
            .or(table.map(|info| info.context_window))
            .unwrap_or(DEFAULT_CONTEXT_WINDOW);
        let max_output_tokens = listed
            .and_then(|model| model.max_output_tokens)
            .or(table.map(|info| info.max_output_tokens))
            .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
            // An output limit never exceeds the window it shares.
            .min(context_window);
        let prices = listed
            .and_then(|model| model.prices)
            .or(table.map(|info| info.prices));
        Self {
            context_window,
            max_output_tokens,
            prices,
        }
    }

    /// The USD cost of `usage`, or `None` when the price is unknown.
    pub fn cost(&self, usage: &Usage) -> Option<f64> {
        self.prices.map(|prices| prices.cost(usage))
    }
}

fn builtins() -> &'static HashMap<String, ModelInfo> {
    static TABLE: OnceLock<HashMap<String, ModelInfo>> = OnceLock::new();
    TABLE.get_or_init(|| {
        serde_json::from_str(include_str!("models.json")).expect("models.json is valid")
    })
}

/// The built-in metadata for `model`, by exact id and then longest
/// dash-separated prefix. A caller that builds no `model_info` override
/// reads the same price the router charges.
pub fn model_info(model: &str) -> Option<&'static ModelInfo> {
    find(builtins(), model)
}

/// The layered metadata for `model` over the built-in table. `listed` is
/// the provider's entry for the model, when its list names it.
pub fn model_metadata(model: &str, listed: Option<&ListedModel>) -> ModelMetadata {
    ModelMetadata::layered(listed, model_info(model))
}

/// Look `model` up in `overrides`, then in the built-in table.
pub(crate) fn lookup<'a>(
    overrides: &'a HashMap<String, ModelInfo>,
    model: &str,
) -> Option<&'a ModelInfo>
where
    'static: 'a,
{
    find(overrides, model).or_else(|| find(builtins(), model))
}

fn find<'a>(table: &'a HashMap<String, ModelInfo>, model: &str) -> Option<&'a ModelInfo> {
    find_id(table, model).or_else(|| {
        let (_, last) = model.rsplit_once('/')?;
        find_id(table, last)
    })
}

fn find_id<'a>(table: &'a HashMap<String, ModelInfo>, model: &str) -> Option<&'a ModelInfo> {
    if let Some(info) = table.get(model) {
        return Some(info);
    }
    table
        .iter()
        .filter(|(key, _)| {
            model.starts_with(key.as_str())
                && matches!(model.as_bytes().get(key.len()), Some(b'-') | Some(b'@'))
        })
        .max_by_key(|(key, _)| key.len())
        .map(|(_, info)| info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_uses_non_overlapping_buckets() {
        let info = ModelInfo {
            context_window: 200_000,
            max_output_tokens: 64_000,
            prices: ModelPrices {
                input_cost: 3.0,
                output_cost: 15.0,
                cache_read_cost: 0.3,
                cache_write_cost: 3.75,
            },
        };
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_read_input_tokens: 400_000,
            cache_write_input_tokens: 100_000,
            reasoning_tokens: 0,
        };
        // 500k fresh * 3 + 400k read * 0.3 + 100k write * 3.75 + 100k out * 15
        let expected =
            (500_000.0 * 3.0 + 400_000.0 * 0.3 + 100_000.0 * 3.75 + 100_000.0 * 15.0) / 1e6;
        assert!((info.cost(&usage) - expected).abs() < 1e-9);
    }

    #[test]
    fn lookup_prefers_exact_then_longest_prefix() {
        let overrides = HashMap::new();
        let info = lookup(&overrides, "claude-sonnet-4-5-20250929").unwrap();
        assert_eq!(info.context_window, 200_000);
        // A prefix match must land on a dash boundary.
        assert!(lookup(&overrides, "gpt-5x").is_none());
        assert!(lookup(&overrides, "gpt-5-mini-2025-08-07").is_some());
    }

    #[test]
    fn overrides_win_over_builtins() {
        let mut overrides = HashMap::new();
        let custom = ModelInfo {
            context_window: 1,
            max_output_tokens: 1,
            prices: ModelPrices {
                input_cost: 0.0,
                output_cost: 0.0,
                cache_read_cost: 0.0,
                cache_write_cost: 0.0,
            },
        };
        overrides.insert("gpt-5".to_owned(), custom);
        assert_eq!(lookup(&overrides, "gpt-5").unwrap().context_window, 1);
    }

    #[test]
    fn current_pagis_routes_have_context_limits() {
        for model in [
            "gpt-6-astra",
            "gpt-5.6",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "claude-sonnet-4-6",
            "claude-sonnet-4.6",
        ] {
            let info = model_info(model).unwrap_or_else(|| panic!("missing metadata for {model}"));
            assert!(info.context_window > info.max_output_tokens);
        }
    }

    #[test]
    fn an_aggregator_id_reads_the_entry_of_its_last_segment() {
        let overrides = HashMap::new();
        let routed = lookup(&overrides, "anthropic/claude-sonnet-4-5").unwrap();
        assert_eq!(routed, lookup(&overrides, "claude-sonnet-4-5").unwrap());
        assert!(lookup(&overrides, "vendor/not-a-model").is_none());
    }

    fn prices(input_cost: f64) -> ModelPrices {
        ModelPrices {
            input_cost,
            output_cost: input_cost * 5.0,
            cache_read_cost: 0.0,
            cache_write_cost: 0.0,
        }
    }

    #[test]
    fn the_provider_report_wins_over_the_table_field_by_field() {
        let table = ModelInfo {
            context_window: 200_000,
            max_output_tokens: 64_000,
            prices: prices(3.0),
        };
        let listed = ListedModel {
            context_window: Some(1_000_000),
            ..ListedModel::new("m")
        };

        let metadata = ModelMetadata::layered(Some(&listed), Some(&table));

        assert_eq!(metadata.context_window, 1_000_000);
        assert_eq!(metadata.max_output_tokens, 64_000);
        assert_eq!(metadata.prices, Some(prices(3.0)));

        let priced = ListedModel {
            prices: Some(prices(1.0)),
            ..ListedModel::new("m")
        };
        let metadata = ModelMetadata::layered(Some(&priced), Some(&table));
        assert_eq!(metadata.prices, Some(prices(1.0)));
    }

    /// GPT-6 Sol and Luna: the limits and prices of
    /// https://developers.openai.com/api/docs/models/gpt-6-sol and
    /// https://developers.openai.com/api/docs/models/gpt-6-luna.
    #[test]
    fn the_table_knows_gpt_6_sol_and_luna() {
        for model in ["gpt-6-sol", "gpt-6-luna"] {
            let metadata = model_metadata(model, None);
            assert_eq!(metadata.context_window, 1_050_000, "{model}");
            assert_eq!(metadata.max_output_tokens, 128_000, "{model}");
        }
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Usage::default()
        };
        assert_eq!(model_metadata("gpt-6-sol", None).cost(&usage), Some(12.0));
        assert_eq!(model_metadata("gpt-6-luna", None).cost(&usage), Some(0.6));
    }

    #[test]
    fn a_model_no_layer_knows_gets_the_default_limits_and_no_price() {
        let metadata = model_metadata("gpt-unlisted", Some(&ListedModel::new("gpt-unlisted")));

        assert_eq!(metadata.context_window, DEFAULT_CONTEXT_WINDOW);
        assert_eq!(metadata.max_output_tokens, DEFAULT_MAX_OUTPUT_TOKENS);
        assert_eq!(metadata.prices, None);
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 10,
            ..Usage::default()
        };
        assert_eq!(metadata.cost(&usage), None);
        assert_eq!(model_metadata("gpt-unlisted", None), metadata);
    }

    #[test]
    fn an_output_limit_never_exceeds_the_context_window() {
        let listed = ListedModel {
            context_window: Some(2_048),
            ..ListedModel::new("small")
        };

        let metadata = ModelMetadata::layered(Some(&listed), None);

        assert_eq!(metadata.max_output_tokens, 2_048);
    }
}

#[cfg(test)]
mod chat_name_tests {
    use super::ListedModel;

    #[test]
    fn a_chat_model_looks_like_chat_and_the_other_families_do_not() {
        for chat in [
            "gpt-6-luna",
            "gpt-5.6-luna",
            "claude-sonnet-4-6",
            "anthropic/claude-sonnet-4.6",
        ] {
            assert!(ListedModel::new(chat).looks_like_chat(), "{chat}");
        }
        for other in [
            "text-embedding-3-large",
            "gpt-4o-mini-tts",
            "whisper-1",
            "gpt-4o-transcribe",
            "dall-e-3",
            "gpt-image-2",
            "sora-2",
            "omni-moderation-latest",
            "gpt-realtime-2.1",
            "gpt-audio",
            "gpt-4o-search-preview",
            "davinci-002",
        ] {
            assert!(!ListedModel::new(other).looks_like_chat(), "{other}");
        }
    }
}
