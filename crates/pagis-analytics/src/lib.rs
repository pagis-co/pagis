//! Anonymous product analytics (ADR-0026).
//!
//! The daemon is the one part of Pagis that sends analytics, and it
//! sends them to PostHog US Cloud. It sends a few events for each
//! installation, keyed by a random Installation ID: the first start, an
//! upgrade, and one Installation Report each day. Every property is an
//! enum, a number or a flag, never text that a person typed.
//!
//! Only a release build can send. The release build compiles in the
//! PostHog project from its environment, so a build from source or a
//! development build holds no project and sends nothing.
//! `DO_NOT_TRACK` stops a release build, and so does the Analytics
//! System Setting. A background task does all of the work, and a
//! failure costs a log line and nothing else.

mod catalog;
mod ledger;
mod posthog;
mod project;
mod worker;

pub use catalog::{Bucket, Event, Features, InstallationKind, Report, StorageBackend};
pub use ledger::{Due, LEDGER_FILE, Ledger, Lifecycle};
pub use posthog::PostHog;
pub use project::{Blocked, HOST, Project, destination};
pub use worker::{Analytics, CHECK_EVERY, FIRST_AFTER, Source};
