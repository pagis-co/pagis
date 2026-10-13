//! Plugins (ADR-0017): the record, the sources it comes from,
//! the validation an install applies, the Bindings that give it
//! Connections and secrets, and the two Grants a call needs.
//!
//! The MCP host that runs a Plugin's servers and the Skills an Agent
//! loads are separate modules; this crate carries what both stand on.

pub mod access;
#[cfg(feature = "native")]
pub mod git;
#[cfg(feature = "native")]
pub mod install;
pub mod manifest;
pub mod package;
pub mod skills;
#[cfg(feature = "native")]
pub mod source;
pub mod validate;

pub use access::{AccessDenied, check_access};
#[cfg(feature = "native")]
pub use git::{ChangeStatus, ChangedFile, DiffSummary, GitStoreError, PluginGitStore, PluginPaths};
#[cfg(feature = "native")]
pub use install::{
    BindingInput, BindingValueInput, Installed, Namespaces, PluginError, PluginServers, Plugins,
    PluginsDeps, next_manifest_version,
};
pub use manifest::{
    ConfigField, DEFAULT_CALL_TIMEOUT, FieldKind, MAX_CALL_TIMEOUT, MCP_FILE, MCP_SCHEMA_ID,
    McpConfig, McpServer, PAGIS_EXTENSION, PLUGIN_MANIFEST_FILE, PLUGIN_SCHEMA_ID, PagisExtension,
    PluginManifest, PluginPackage, SKILLS_DIR, ToolPolicy, config_references,
};
pub use package::{PackageError, PackageFile, PackageFiles};
pub use skills::SKILL_FILE;
#[cfg(feature = "native")]
pub use skills::{SkillCatalog, SkillCatalogDeps, first_party_skills};
#[cfg(feature = "native")]
pub use source::{SourceError, SourceInput, fetch};
pub use validate::{MAX_PACKAGE_BYTES, ValidationErrors, validate};
