//! The voices of a provider that lists none.
//!
//! OpenRouter names the voices of each speech model in its model list
//! ([`crate::ListedModel::voices`]). OpenAI lists no voices: its speech
//! and realtime models share one fixed set, so the set is a constant.

/// Every voice OpenAI's speech and realtime models share, in the order a
/// picker shows them. The first is the default.
pub const OPENAI_VOICES: &[&str] = &[
    "alloy", "ash", "ballad", "cedar", "coral", "echo", "fable", "marin", "nova", "onyx", "sage",
    "shimmer", "verse",
];
