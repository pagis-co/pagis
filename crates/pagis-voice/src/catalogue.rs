//! The voice catalogue (ADR-0020): the names an Agent Voice can take.
//!
//! There is one speech provider, OpenAI, and one realtime provider, also
//! OpenAI, with an overlapping catalogue, so the catalogue is a
//! constant of the program. A second provider needs a Workspace `Voice`
//! record with ordered provider candidates. That record is not built.

use crate::VoiceError;

/// Every voice OpenAI's speech and realtime models share, in the
/// order the picker shows them.
pub const VOICES: &[&str] = &[
    "alloy", "ash", "ballad", "cedar", "coral", "echo", "fable", "marin", "nova", "onyx", "sage",
    "shimmer", "verse",
];

/// Refuse a voice that is not in the catalogue. The check runs where
/// the Agent is written, so a stored voice is always one the provider
/// knows.
pub fn validate_voice(voice: &str) -> Result<(), VoiceError> {
    if VOICES.contains(&voice) {
        Ok(())
    } else {
        Err(VoiceError::Provider(format!(
            "`{voice}` is not a voice in the catalogue; choose one of {}",
            VOICES.join(", ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalogue_voice_passes_and_any_other_name_is_refused() {
        assert!(validate_voice("nova").is_ok());
        assert!(validate_voice("marin").is_ok());
        let error = validate_voice("robot").unwrap_err().to_string();
        assert!(error.contains("`robot` is not a voice"), "{error}");
        assert!(error.contains("alloy"), "{error}");
        // The catalogue is case-sensitive: the wire name is the name.
        assert!(validate_voice("Nova").is_err());
    }
}
