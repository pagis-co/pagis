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

use llm_router::ListedVoice;
use pagis_core::{Provider, ProviderUse, WorkspaceId};

use crate::AppState;
use crate::error::ApiError;

/// The model that speaks for a Workspace, and its voices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceList {
    pub provider: Provider,
    /// The model id as the provider names it.
    pub model: String,
    /// The voices in the provider's order. The first is the default. A
    /// voice's id is what an Agent Voice holds; its name, where the id
    /// is not one, is what a person reads.
    pub voices: Vec<ListedVoice>,
}

impl VoiceList {
    /// The id of the voice a reply speaks in: `wanted` when the model has
    /// it, else the model's first voice. `None` when the model names no
    /// voice.
    pub fn voice_for(&self, wanted: Option<&str>) -> Option<&str> {
        wanted
            .and_then(|wanted| self.voices.iter().find(|voice| voice.id == wanted))
            .or_else(|| self.voices.first())
            .map(|voice| voice.id.as_str())
    }

    pub fn has(&self, voice: &str) -> bool {
        self.voices.iter().any(|known| known.id == voice)
    }

    /// The voices as a person reads them: "Rachel (21m00…)". A voice whose
    /// id is its name reads as the id.
    pub fn names(&self) -> String {
        self.voices
            .iter()
            .map(|voice| match &voice.name {
                Some(name) => format!("{name} ({})", voice.id),
                None => voice.id.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
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
            .map(|voice| ListedVoice::named_by_id(*voice))
            .collect(),
        Provider::Anthropic | Provider::OpenRouter | Provider::Deepgram | Provider::ElevenLabs => {
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
/// key a person lacks: "ElevenLabs, Deepgram, OpenAI or OpenRouter".
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
    use llm_router::ListedVoice;
    use pagis_core::Provider;

    use super::{VoiceList, speaking_providers};

    fn list() -> VoiceList {
        VoiceList {
            provider: Provider::OpenRouter,
            model: "google/gemini-3.8-flash-tts".to_string(),
            voices: vec![
                ListedVoice::named_by_id("Zephyr"),
                ListedVoice::named_by_id("Kore"),
            ],
        }
    }

    /// A voice id that is no name reads with the voice's name.
    #[test]
    fn the_names_read_as_a_person_reads_them() {
        let elevenlabs = VoiceList {
            provider: Provider::ElevenLabs,
            model: "eleven_flash_v2_5".to_string(),
            voices: vec![ListedVoice {
                id: "21m00Tcm4TlvDq8ikWAM".to_string(),
                name: Some("Rachel".to_string()),
            }],
        };

        assert_eq!(list().names(), "Zephyr, Kore");
        assert_eq!(elevenlabs.names(), "Rachel (21m00Tcm4TlvDq8ikWAM)");
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
        assert_eq!(
            speaking_providers(),
            "ElevenLabs, Deepgram, OpenAI or OpenRouter"
        );
    }
}
