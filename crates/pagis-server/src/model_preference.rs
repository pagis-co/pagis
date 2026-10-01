//! The Model Preference of each well-known Model Alias (ADR-0025): the
//! models the product selects for the alias, best first.
//!
//! The seed gives each alias the preferred models of the first provider
//! in its preference. A key change gives an alias that no keyed provider
//! serves the preferred models of the first keyed provider that serves
//! its use ([`crate::model_lists::route_unrouted_aliases`]). A route that
//! a keyed provider still serves stays, because a person may have chosen
//! it. The `default` alias also reads the Provider Model Lists, so its
//! route is one listed model ([`crate::model_lists::default_route`]).
//!
//! A route names the models of one provider. The seed adds no candidate
//! of another provider: a silent fallback changes the provider, the price
//! and the tools under the person.

use pagis_core::{DEFAULT_MODEL_ALIAS, Provider, ProviderUse};

/// The Model Preference of the `default` alias. Each provider names one
/// model, so its route is one model.
pub const DEFAULT_PREFERENCE: &[&str] = &[
    "openai/gpt-6-luna",
    "openrouter/openai/gpt-6-luna",
    "anthropic/claude-sonnet-5-5",
];

/// Each well-known alias and its Model Preference.
pub const PREFERENCES: [(&str, &[&str]); 6] = [
    (DEFAULT_MODEL_ALIAS, DEFAULT_PREFERENCE),
    (
        pagis_voice::TRANSCRIBE_ALIAS,
        &[
            "deepgram/nova-3",
            "elevenlabs/scribe_v2",
            "openai/gpt-4o-transcribe",
            "openrouter/openai/gpt-4o-transcribe",
        ],
    ),
    (
        pagis_voice::SPEAK_ALIAS,
        &[
            "elevenlabs/eleven_flash_v2_5",
            "deepgram/aura-2",
            "openai/gpt-4o-mini-tts",
            "openrouter/google/gemini-3.8-flash-tts",
        ],
    ),
    (pagis_telephony::PHONE_ALIAS, &pagis_telephony::PHONE_MODELS),
    (
        pagis_telephony::GPT_LIVE_REASONING_ALIAS,
        &pagis_telephony::GPT_LIVE_REASONING_MODELS,
    ),
    (
        pagis_telephony::PHONE_CLASSIFIER_ALIAS,
        &pagis_telephony::PHONE_CLASSIFIER_MODELS,
    ),
];

/// What a provider must serve to answer an alias. The voice and call
/// aliases are plumbing; every other alias carries the Runs of Agents.
pub fn alias_use(alias: &str) -> ProviderUse {
    match alias {
        pagis_voice::SPEAK_ALIAS => ProviderUse::SpokenReplies,
        pagis_voice::TRANSCRIBE_ALIAS => ProviderUse::Dictation,
        pagis_telephony::PHONE_ALIAS
        | pagis_telephony::GPT_LIVE_REASONING_ALIAS
        | pagis_telephony::PHONE_CLASSIFIER_ALIAS => ProviderUse::Calls,
        _ => ProviderUse::Thinking,
    }
}

/// The provider a candidate names, if it names a known one.
pub fn provider_of(candidate: &str) -> Option<Provider> {
    candidate
        .split_once('/')
        .and_then(|(provider, _)| Provider::from_id(provider))
}

/// The preferred candidates of `provider` for `alias`, in order.
fn candidates_of(alias: &str, provider: Provider) -> Vec<String> {
    preference(alias)
        .iter()
        .filter(|candidate| provider_of(candidate) == Some(provider))
        .map(|candidate| candidate.to_string())
        .collect()
}

/// The Model Preference of a well-known alias. Another alias has none.
pub fn preference(alias: &str) -> &'static [&'static str] {
    PREFERENCES
        .iter()
        .find(|(name, _)| *name == alias)
        .map(|(_, preference)| *preference)
        .unwrap_or_default()
}

/// The providers of an alias's preference that serve its use, best
/// first, each once.
pub fn preferred_providers(alias: &str) -> Vec<Provider> {
    let mut providers = Vec::new();
    for candidate in preference(alias) {
        if let Some(provider) = provider_of(candidate)
            && provider.serves(alias_use(alias))
            && !providers.contains(&provider)
        {
            providers.push(provider);
        }
    }
    providers
}

/// The route the seed gives an alias: the preferred candidates of the
/// first provider in its preference.
pub fn seed_route(alias: &str) -> Vec<String> {
    preferred_providers(alias)
        .first()
        .map(|provider| candidates_of(alias, *provider))
        .unwrap_or_default()
}

/// The route of an alias for the providers that hold a key: the preferred
/// candidates of the first of them that serves the alias's use, or `None`
/// when none does.
pub fn route_for(alias: &str, keyed: &[Provider]) -> Option<Vec<String>> {
    preferred_providers(alias)
        .into_iter()
        .find(|provider| keyed.contains(provider))
        .map(|provider| candidates_of(alias, provider))
}

#[cfg(test)]
mod tests {
    use pagis_core::Provider;

    use super::*;

    /// A preference names only providers that serve its alias's use, so
    /// the seed and a key change never pick a provider that cannot
    /// answer.
    #[test]
    fn each_preferred_candidate_names_a_provider_that_serves_the_alias() {
        for (alias, preference) in PREFERENCES {
            assert!(!preference.is_empty(), "{alias} prefers nothing");
            for candidate in preference {
                let provider = provider_of(candidate)
                    .unwrap_or_else(|| panic!("{candidate} of {alias} names no known provider"));
                assert!(
                    provider.serves(alias_use(alias)),
                    "{candidate} cannot serve {alias}"
                );
            }
        }
    }

    /// Each provider that thinks names one default model, so its route is
    /// one model and a key of it always has a model to preselect.
    #[test]
    fn the_default_preference_names_one_model_for_each_provider_that_thinks() {
        for provider in pagis_core::PROVIDERS
            .into_iter()
            .filter(|provider| provider.serves(pagis_core::ProviderUse::Thinking))
        {
            assert_eq!(
                candidates_of(DEFAULT_MODEL_ALIAS, provider).len(),
                1,
                "{}",
                provider.id()
            );
        }
    }

    /// A route takes every preferred candidate of one provider: the phone
    /// alias falls back from GPT-Live to Realtime on OpenAI.
    #[test]
    fn a_route_takes_the_preferred_candidates_of_the_first_keyed_provider() {
        assert_eq!(
            route_for(pagis_voice::TRANSCRIBE_ALIAS, &[Provider::OpenRouter]),
            Some(vec!["openrouter/openai/gpt-4o-transcribe".to_string()])
        );
        assert_eq!(
            route_for(
                pagis_voice::TRANSCRIBE_ALIAS,
                &[Provider::OpenRouter, Provider::OpenAi]
            ),
            Some(vec!["openai/gpt-4o-transcribe".to_string()])
        );
        assert_eq!(
            route_for(pagis_telephony::PHONE_ALIAS, &[Provider::OpenAi]),
            Some(pagis_telephony::PHONE_MODELS.map(str::to_string).to_vec())
        );
    }

    #[test]
    fn no_route_when_no_keyed_provider_serves_the_alias() {
        assert_eq!(
            route_for(
                pagis_telephony::PHONE_ALIAS,
                &[Provider::Anthropic, Provider::OpenRouter]
            ),
            None
        );
    }
}
