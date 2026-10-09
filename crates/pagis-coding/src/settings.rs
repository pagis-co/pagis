//! The Harness Model and the thought level of a Coding Session
//! (ADR-0033).
//!
//! A harness offers each one as an ACP Session Config Option of the type
//! `select`. The Agent names a choice by its id at the start and with
//! `coding_session_set_model`. The daemon sets each choice with
//! `session/set_config_option`, the model first, because the choices of
//! the thought level can depend on the model. A setting that the Agent
//! does not name keeps the choice of the harness.

use pagis_core::{CodingSession, HarnessSetting};

use crate::sessions::harness_message;
use crate::{AcpSession, SessionSettings};

/// A setting of a session that the Agent chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// The Harness Model: the config option of the category `model`.
    Model,
    /// The config option of the category `thought_level`.
    ThoughtLevel,
}

impl Setting {
    /// The order in which the daemon sets them.
    const ORDER: [Setting; 2] = [Setting::Model, Setting::ThoughtLevel];

    /// The name of the argument of the tools.
    pub fn as_str(self) -> &'static str {
        match self {
            Setting::Model => "model",
            Setting::ThoughtLevel => "thought_level",
        }
    }

    /// The end reason of a start, and the tool error, when the harness
    /// does not offer the choice.
    pub fn not_offered(self) -> &'static str {
        match self {
            Setting::Model => "model_not_offered",
            Setting::ThoughtLevel => "thought_level_not_offered",
        }
    }

    /// The words for a person.
    pub fn words(self) -> &'static str {
        match self {
            Setting::Model => "model",
            Setting::ThoughtLevel => "thought level",
        }
    }

    fn of(self, record: &CodingSession) -> Option<&HarnessSetting> {
        match self {
            Setting::Model => record.model.as_ref(),
            Setting::ThoughtLevel => record.thought_level.as_ref(),
        }
    }

    fn of_mut(self, record: &mut CodingSession) -> Option<&mut HarnessSetting> {
        match self {
            Setting::Model => record.model.as_mut(),
            Setting::ThoughtLevel => record.thought_level.as_mut(),
        }
    }
}

/// The choices of the Agent: the id of a choice for each setting that it
/// names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingChoices {
    pub model: Option<String>,
    pub thought_level: Option<String>,
}

impl SettingChoices {
    fn of(&self, setting: Setting) -> Option<&str> {
        match setting {
            Setting::Model => self.model.as_deref(),
            Setting::ThoughtLevel => self.thought_level.as_deref(),
        }
    }

    /// The current choices of `record`, which a resume sets again.
    pub(crate) fn recorded(record: &CodingSession) -> Self {
        Self {
            model: record.model.as_ref().map(|model| model.current.clone()),
            thought_level: record
                .thought_level
                .as_ref()
                .map(|level| level.current.clone()),
        }
    }
}

/// Why the harness did not take a choice.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChoiceFailure {
    /// The harness does not offer the choice. `offered` lists the choices
    /// that it offers for the setting, as harness text, and is empty when
    /// it offers no such setting.
    #[error("the Coding Harness does not offer the {} {choice:?}", .setting.words())]
    NotOffered {
        setting: Setting,
        choice: String,
        offered: String,
    },
    /// The harness refused the request. The message is harness text.
    #[error("the Coding Harness failed: {0}")]
    Harness(String),
}

/// Puts the settings that the harness answered into `record`. A place
/// that fixes the model keeps no Harness Model, because the harness lists
/// models that the place does not serve.
pub(crate) fn record_settings(
    record: &mut CodingSession,
    settings: &SessionSettings,
    fixed_model: bool,
) {
    record.model = if fixed_model {
        None
    } else {
        settings.model.clone()
    };
    record.thought_level = settings.thought_level.clone();
}

/// Sets each choice of `choices` that differs from the current choice of
/// the session, the model first, and writes the settings that the harness
/// answered into `record`. It answers whether a setting changed.
///
/// A choice that the setting does not offer is `ChoiceFailure::NotOffered`
/// when `strict`, and else the setting keeps the choice of the harness: a
/// resume sets the recorded choices again where the harness still offers
/// them.
pub(crate) async fn apply_choices(
    acp: &AcpSession,
    record: &mut CodingSession,
    choices: &SettingChoices,
    strict: bool,
    fixed_model: bool,
) -> Result<bool, ChoiceFailure> {
    let mut changed = false;
    for setting in Setting::ORDER {
        let Some(choice) = choices.of(setting) else {
            continue;
        };
        let offered = setting.of(record).filter(|current| current.offers(choice));
        let Some(current) = offered else {
            if !strict {
                continue;
            }
            return Err(ChoiceFailure::NotOffered {
                setting,
                choice: choice.to_string(),
                offered: setting
                    .of(record)
                    .map(HarnessSetting::choices_text)
                    .unwrap_or_default(),
            });
        };
        if current.current == choice {
            continue;
        }
        let option_id = current.option_id.clone();
        let answered = acp
            .set_setting(&option_id, choice)
            .await
            .map_err(|error| ChoiceFailure::Harness(harness_message(error)))?;
        if answered == SessionSettings::default() {
            // The harness answered no config options, so only this choice
            // is known to change.
            if let Some(current) = setting.of_mut(record) {
                current.current = choice.to_string();
            }
        } else {
            record_settings(record, &answered, fixed_model);
        }
        changed = true;
    }
    Ok(changed)
}
