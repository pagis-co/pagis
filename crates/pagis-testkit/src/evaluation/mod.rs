//! The release evaluation driver replays one
//! continuous learning chronology through a running daemon and reports
//! what the daemon did.
//!
//! `DaemonDriver` runs the shipped Gmail-to-Subject-Page path.
//!
//! A different model route proposes each probe grade. The owner grades
//! the fixed sample. Real-model runs belong to the release evaluation.

mod driver;
mod import;
mod meter;
mod scripted;
mod source;

pub use driver::{DaemonDriver, EvaluationSpend};
pub use import::{IMPORT_TIMEOUT, ImportWait};
pub use meter::{MeterCursor, MeteredBrain, ModelMeter, ModelSpend};
pub use scripted::ScriptedModel;
pub use source::{FixtureClock, FixtureSource, MailItem, mail_items, millis};
