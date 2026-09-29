//! Daemon wiring: configuration, first-boot bootstrap, startup.

mod analytics;
mod app;
mod arrival_commit;
pub mod backup;
mod boot;
pub mod call_events;
pub mod collectors;
mod config;
pub mod connections;
pub mod keypad;
mod knowledge;
pub mod logging;
pub mod mail_events;
mod package;
mod page_signals;
pub mod plugin_tools;
pub mod retention;
pub mod schedule_tools;
mod secrets;
pub mod software_tools;
mod spa;
pub mod subscription_tools;
pub mod system;
mod tools;

pub use analytics::AnalyticsOptions;
pub use app::{AppOptions, Interfaces, app};
pub use boot::{
    Booted, CLIENT_CREDENTIAL_FILE, Installation, boot, create_state_directory,
    plugin_logs_directory, sign_in_link,
};
pub use config::{Administration, Config, Screen, Turn};
pub use package::validate_server_package;
pub use secrets::{EncryptedFileSecretStore, platform_secret_store};
pub use spa::product_app_is_built;
pub use system::{FileSystemConfig, taken_administration_port_message, taken_port_message};
