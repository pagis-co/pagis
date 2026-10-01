//! The Provider Voice List (ADR-0020): the voices of the model that
//! speaks for a Workspace.
//!
//! The model that speaks is the first candidate of the `speak` alias
//! whose provider holds a key and serves spoken replies. Its voices come
//! from the provider, as the Provider Model List does: OpenRouter names
//! the voices of each speech model in its model list, which the daemon
//! refreshes every hour and on a key change. OpenAI lists no voices, so
//! its set is fixed ([`llm_router::OPENAI_VOICES`]).
//!
//! The Agent Voice is one name of this list. A voice the model that
//! speaks does not have is absent: a reply takes the model's first voice
//! and says which voice spoke.

use pagis_core::{Provider, ProviderUse, WorkspaceId};

use crate::AppState;
use crate::error::ApiError;

/// The model that speaks for a Workspace, and its voices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceList {
    pub provider: Provider,
    /// The model id as the provider names it.
    pub model: String,
    /// The voices in the provider's order. The first is the default.
    pub voices: Vec<String>,
}

impl VoiceList {
    /// The voice a reply speaks in: `wanted` when the model has it, else
    /// the model's first voice. `None` when the model names no voice.
    pub fn voice_for(&self, wanted: Option<&str>) -> Option<&str> {
        wanted
            .and_then(|wanted| self.voices.iter().find(|voice| *voice == wanted))
            .or_else(|| self.voices.first())
            .map(String::as_str)
    }

    pub fn has(&self, voice: &str) -> bool {
        self.voices.iter().any(|known| known == voice)
    }
}

/// The voice list of the model that speaks for the Workspace, or `None`
/// when no candidate of its `speak` alias has a keyed provider that
/// serves spoken replies.
pub async fn speaking_voices(
    state: &AppState,
    workspace_id: &WorkspaceId,
) -> Result<Option<VoiceList>, ApiError> {
    let Some(alias) = state
        .model_aliases
        .get_by_alias(workspace_id, pagis_voice::SPEAK_ALIAS)
        .await?
    else {
        return Ok(None);
    };
    let Some(candidate) = crate::model_lists::reachable_candidates(
        &state.keys,
        ProviderUse::SpokenReplies,
        alias.candidates,
    )?
    .into_iter()
    .next() else {
        return Ok(None);
    };
    let Some((provider, model)) = candidate
        .split_once('/')
        .and_then(|(provider, model)| Some((Provider::from_id(provider)?, model.to_string())))
    else {
        return Ok(None);
    };
    let voices = match provider {
        Provider::OpenAi => llm_router::OPENAI_VOICES
            .iter()
            .map(|voice| voice.to_string())
            .collect(),
        Provider::Anthropic | Provider::OpenRouter | Provider::Deepgram => {
            match state.models.models(provider).await {
                Ok(listed) => listed
                    .iter()
                    .find(|listed| listed.id == model)
                    .and_then(|listed| listed.voices.clone())
                    .unwrap_or_default(),
                Err(error) => {
                    tracing::warn!(provider = provider.id(), %error, "the provider did not list its voices");
                    Vec::new()
                }
            }
        }
    };
    Ok(Some(VoiceList {
        provider,
        model,
        voices,
    }))
}

/// The providers that would speak a reply, for a message that names the
/// key a person lacks: "Deepgram, OpenAI or OpenRouter".
pub fn speaking_providers() -> String {
    let names: Vec<&str> = crate::model_preference::preferred_providers(pagis_voice::SPEAK_ALIAS)
        .into_iter()
        .map(Provider::name)
        .collect();
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => names.join(""),
    }
}

#[cfg(test)]
mod tests {
    use pagis_core::Provider;

    use super::{VoiceList, speaking_providers};

    fn list() -> VoiceList {
        VoiceList {
            provider: Provider::OpenRouter,
            model: "google/gemini-3.8-flash-tts".to_string(),
            voices: vec!["Zephyr".to_string(), "Kore".to_string()],
        }
    }

    #[test]
    fn a_listed_voice_speaks_and_any_other_takes_the_first() {
        assert_eq!(list().voice_for(Some("Kore")), Some("Kore"));
        assert_eq!(list().voice_for(Some("nova")), Some("Zephyr"));
        assert_eq!(list().voice_for(None), Some("Zephyr"));
        let silent = VoiceList {
            voices: Vec::new(),
            ..list()
        };
        assert_eq!(silent.voice_for(Some("Kore")), None);
    }

    #[test]
    fn the_message_names_each_provider_that_would_speak() {
        assert_eq!(speaking_providers(), "Deepgram, OpenAI or OpenRouter");
    }
}
