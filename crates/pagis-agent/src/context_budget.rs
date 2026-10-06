//! Provider-aware bounds for complete model requests.
//!
//! Context and output limits for current OpenAI routes come from
//! https://developers.openai.com/api/docs/models/gpt-6-astra and the
//! matching model pages linked from https://developers.openai.com/api/docs/models.
//! The `gpt-5.6-*` routes use the published `gpt-5.6` request limits.
//! Claude limits come from
//! https://platform.claude.com/docs/en/about-claude/models/overview.
//! OpenRouter routes use the limits of the provider model they name.
//!
//! The limits come in layers ([`ModelCatalog::metadata`]): the Provider
//! Model List, then `models.json`, then a conservative default. So the
//! budget never refuses a model the provider lists or the person typed.

use crate::brain::TurnRequest;
use crate::model_catalog::ModelCatalog;

const OUTPUT_RESERVE_MAX: u64 = 16_384;
const STRUCTURE_TOKENS: u64 = 64;
/// The largest documented token cost of one accepted image, which also
/// bounds an image on a model with no documented cost.
const UNKNOWN_IMAGE_TOKENS: u64 = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestBudget {
    pub output_reserve: u64,
    pub input_allowance: u64,
    pub compact_above: u64,
    pub compact_target: u64,
    pub recent_verbatim: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestEstimate {
    pub input_tokens: u64,
    pub output_reserve: u64,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BudgetError {
    #[error("model candidate `{0}` must use provider/model format")]
    InvalidCandidate(String),
    #[error("the model route names no candidate")]
    EmptyRoute,
}

impl RequestBudget {
    pub fn for_candidates(
        candidates: &[String],
        models: &ModelCatalog,
    ) -> Result<Self, BudgetError> {
        let mut context_window = u64::MAX;
        let mut max_output = u64::MAX;
        for candidate in candidates {
            let (provider, model) = split_candidate(candidate)?;
            let metadata = models.metadata(provider, model);
            context_window = context_window.min(u64::from(metadata.context_window));
            max_output = max_output.min(u64::from(metadata.max_output_tokens));
        }
        if context_window == u64::MAX {
            return Err(BudgetError::EmptyRoute);
        }
        let output_reserve = OUTPUT_RESERVE_MAX.min(max_output).min(context_window / 4);
        let input_allowance = context_window.saturating_sub(output_reserve);
        Ok(Self {
            output_reserve,
            input_allowance,
            compact_above: input_allowance.saturating_mul(80) / 100,
            compact_target: input_allowance.saturating_mul(60) / 100,
            recent_verbatim: OUTPUT_RESERVE_MAX.min(input_allowance / 4),
        })
    }
}

impl RequestEstimate {
    /// Bound the provider-visible request. UTF-8 bytes are a conservative
    /// text-token bound because one token always consumes at least one byte.
    /// An image costs its documented formula at its pixel size on the
    /// most expensive candidate. An image without a readable size costs
    /// the documented maximum for one accepted image at `auto` detail.
    pub fn of(request: &TurnRequest, models: &ModelCatalog) -> Result<Self, BudgetError> {
        let budget = RequestBudget::for_candidates(&request.model_candidates, models)?;
        let mut input = STRUCTURE_TOKENS.saturating_add(text_tokens(&request.system));
        for message in &request.messages {
            input = input
                .saturating_add(STRUCTURE_TOKENS)
                .saturating_add(text_tokens(&message.text));
            for image in &message.images {
                input = input.saturating_add(route_image_tokens(&request.model_candidates, image)?);
            }
            for call in &message.tool_calls {
                input = input
                    .saturating_add(STRUCTURE_TOKENS)
                    .saturating_add(text_tokens(&call.id))
                    .saturating_add(text_tokens(&call.name))
                    .saturating_add(text_tokens(&call.arguments));
            }
            if let Some(call_id) = &message.tool_call_id {
                input = input.saturating_add(text_tokens(call_id));
            }
        }
        for tool in &request.tools {
            input = input
                .saturating_add(STRUCTURE_TOKENS)
                .saturating_add(text_tokens(&tool.name))
                .saturating_add(text_tokens(&tool.description))
                .saturating_add(text_tokens(&tool.parameters.to_string()));
        }
        if request.computer {
            input = input.saturating_add(512);
        }
        if let Some(schema) = &request.output_schema {
            input = input
                .saturating_add(STRUCTURE_TOKENS)
                .saturating_add(text_tokens(&schema.name))
                .saturating_add(text_tokens(&schema.schema.to_string()));
        }
        Ok(Self {
            input_tokens: input,
            output_reserve: budget.output_reserve,
        })
    }
}

/// What one model request holds, in counts and token bounds only. The
/// Run records it before the call, so a request that fails still shows
/// its size. It never holds message text, a tool result or an image.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct RequestSummary {
    pub model_alias: String,
    pub model_candidates: Vec<String>,
    /// `None` when the route has no budget, which also refuses the call.
    pub estimated_input_tokens: Option<u64>,
    pub input_allowance: Option<u64>,
    /// The output limit the request sends; `None` sends no limit.
    pub max_output_tokens: Option<u32>,
    pub messages: usize,
    pub tools: usize,
    pub images: usize,
}

impl RequestSummary {
    pub fn of(request: &TurnRequest, models: &ModelCatalog) -> Self {
        Self {
            model_alias: request.model_alias.clone(),
            model_candidates: request.model_candidates.clone(),
            estimated_input_tokens: RequestEstimate::of(request, models)
                .ok()
                .map(|estimate| estimate.input_tokens),
            input_allowance: RequestBudget::for_candidates(&request.model_candidates, models)
                .ok()
                .map(|budget| budget.input_allowance),
            max_output_tokens: request.max_output_tokens,
            messages: request.messages.len(),
            tools: request.tools.len(),
            images: request
                .messages
                .iter()
                .map(|message| message.images.len())
                .sum(),
        }
    }
}

pub(crate) fn estimate_message_tokens(message: &crate::brain::TurnMessage) -> u64 {
    let calls = message.tool_calls.iter().fold(0_u64, |total, call| {
        total
            .saturating_add(text_tokens(&call.id))
            .saturating_add(text_tokens(&call.name))
            .saturating_add(text_tokens(&call.arguments))
            .saturating_add(STRUCTURE_TOKENS)
    });
    STRUCTURE_TOKENS
        .saturating_add(text_tokens(&message.text))
        .saturating_add(calls)
}

fn text_tokens(text: &str) -> u64 {
    u64::try_from(text.len()).unwrap_or(u64::MAX)
}

/// Split `provider/model`; the model part of an aggregator keeps its
/// own slash (`openrouter/anthropic/claude-sonnet-4.6`).
fn split_candidate(candidate: &str) -> Result<(&str, &str), BudgetError> {
    candidate
        .split_once('/')
        .ok_or_else(|| BudgetError::InvalidCandidate(candidate.to_string()))
}

/// The vendor's own model id: the last path segment.
fn candidate_model(candidate: &str) -> Result<&str, BudgetError> {
    let (_, routed) = split_candidate(candidate)?;
    Ok(routed.rsplit('/').next().unwrap_or(routed))
}

fn route_image_tokens(candidates: &[String], image: &str) -> Result<u64, BudgetError> {
    let mut tokens = 0;
    for candidate in candidates {
        let model = candidate_model(candidate)?;
        let cost = match crate::image_tokens::image_tokens(model, image) {
            Some(cost) => cost,
            None => image_tokens_per_accepted_image(candidate)?,
        };
        tokens = tokens.max(cost);
    }
    Ok(tokens)
}

fn image_tokens_per_accepted_image(candidate: &str) -> Result<u64, BudgetError> {
    let model = candidate_model(candidate)?;
    if model.starts_with("claude-") {
        // https://platform.claude.com/docs/en/build-with-claude/vision
        // Auto-resized accepted images use at most 4,784 visual tokens on
        // the high-resolution tier. This also bounds the 1,568-token tier.
        return Ok(4_784);
    }
    if model.starts_with("gpt-6-astra") || model.starts_with("gpt-5.6") {
        // https://developers.openai.com/api/docs/guides/images-vision
        // Pagis omits detail, so `auto` uses `original`: at most 30,000
        // accepted 32px patches times the documented 1.2 multiplier.
        return Ok(36_000);
    }
    // The older OpenAI tile and patch tables stay below this bound for
    // one accepted `auto` image, including gpt-4o-mini's large
    // multiplier. A model with no documented cost gets the same bound,
    // so an image never refuses a model the provider lists.
    Ok(UNKNOWN_IMAGE_TOKENS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TurnMessage, TurnRequest};

    fn request(candidate: &str, images: usize) -> TurnRequest {
        TurnRequest {
            model_alias: "default".into(),
            model_candidates: vec![candidate.into()],
            system: "Follow the user's request.".into(),
            messages: vec![TurnMessage {
                role: crate::TurnRole::User,
                text: "Inspect this image.".into(),
                images: vec!["data:image/png;base64,AAAA".into(); images],
                tool_calls: vec![],
                tool_call_id: None,
                conversation_evidence: None,
            }],
            tools: vec![],
            computer: false,
            allow_tool_calls: true,
            max_output_tokens: None,
            output_schema: None,
        }
    }

    fn models() -> ModelCatalog {
        ModelCatalog::new(std::sync::Arc::new(pagis_core::ProviderKeys::with_env(
            |_| None,
            std::collections::HashMap::new(),
            std::sync::Arc::new(pagis_core::MemorySecretStore::default()),
        )))
    }

    #[test]
    fn ordinary_supported_image_stays_inside_the_route_budget() {
        let request = request("openai/gpt-5.6", 1);
        let estimate = RequestEstimate::of(&request, &models()).unwrap();
        let budget = RequestBudget::for_candidates(&request.model_candidates, &models()).unwrap();
        assert!(estimate.input_tokens < budget.compact_above);
    }

    #[test]
    fn image_heavy_request_crosses_the_compaction_threshold() {
        let request = request("anthropic/claude-sonnet-4-5", 31);
        let estimate = RequestEstimate::of(&request, &models()).unwrap();
        let budget = RequestBudget::for_candidates(&request.model_candidates, &models()).unwrap();
        assert!(estimate.input_tokens > budget.compact_above);
    }

    #[test]
    fn smallest_candidate_defines_the_route_budget() {
        let budget = RequestBudget::for_candidates(
            &[
                "openai/gpt-5.6".into(),
                "anthropic/claude-sonnet-4-5".into(),
            ],
            &models(),
        )
        .unwrap();
        assert_eq!(budget.input_allowance, 183_616);
        assert_eq!(budget.compact_above, 146_892);
        assert_eq!(budget.compact_target, 110_169);
        assert_eq!(budget.recent_verbatim, 16_384);
    }

    #[test]
    fn openrouter_default_candidate_has_context_metadata() {
        let budget = RequestBudget::for_candidates(
            &["openrouter/anthropic/claude-sonnet-4.6".into()],
            &models(),
        )
        .unwrap();
        assert_eq!(budget.input_allowance, 983_616);
    }

    #[test]
    fn a_model_no_table_knows_gets_the_default_budget() {
        let budget =
            RequestBudget::for_candidates(&["openai/gpt-unlisted".into()], &models()).unwrap();

        let reserve = u64::from(llm_router::DEFAULT_MAX_OUTPUT_TOKENS);
        assert_eq!(budget.output_reserve, reserve);
        assert_eq!(
            budget.input_allowance,
            u64::from(llm_router::DEFAULT_CONTEXT_WINDOW) - reserve
        );
    }

    #[test]
    fn an_image_on_a_route_without_a_documented_bound_takes_the_largest_bound() {
        let request = request("google/gemini-2.5-pro", 1);
        let text_only =
            RequestEstimate::of(&self::request("google/gemini-2.5-pro", 0), &models()).unwrap();

        let estimate = RequestEstimate::of(&request, &models()).unwrap();

        assert_eq!(
            estimate.input_tokens - text_only.input_tokens,
            UNKNOWN_IMAGE_TOKENS
        );
    }

    fn screenshot_uri(width: u32, height: u32) -> String {
        use base64::Engine as _;
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(width, height)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png.into_inner())
        )
    }

    /// The estimated cost of one 1280x720 screenshot on `candidate`.
    fn screenshot_cost(candidate: &str) -> u64 {
        let mut with_image = request(candidate, 1);
        with_image.messages[0].images = vec![screenshot_uri(1280, 720)];
        let text_only = RequestEstimate::of(&request(candidate, 0), &models()).unwrap();
        RequestEstimate::of(&with_image, &models())
            .unwrap()
            .input_tokens
            - text_only.input_tokens
    }

    #[test]
    fn a_screenshot_on_a_model_without_a_documented_formula_costs_its_size() {
        // The largest documented cost of 1280x720: 920 patches of 32px
        // times the 2.46 multiplier of the nano models.
        assert_eq!(screenshot_cost("openai/gpt-unlisted"), 2_264);
    }

    #[test]
    fn a_screenshot_costs_the_documented_formula_of_its_model() {
        // Claude: width times height over 750.
        assert_eq!(screenshot_cost("anthropic/claude-haiku-4-5"), 1_229);
        // gpt-6-astra at `original` detail: 920 patches times 1.2.
        assert_eq!(screenshot_cost("openai/gpt-6-astra"), 1_104);
        // gpt-4o-mini: 6 tiles of 512px at 5,667 plus 2,833.
        assert_eq!(screenshot_cost("openai/gpt-4o-mini"), 36_835);
    }

    #[test]
    fn a_candidate_without_a_provider_is_refused() {
        assert!(matches!(
            RequestBudget::for_candidates(&["gpt-5".into()], &models()),
            Err(BudgetError::InvalidCandidate(candidate)) if candidate == "gpt-5"
        ));
    }
}
