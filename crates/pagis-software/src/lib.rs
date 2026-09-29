//! Software Packages: the manifest an agent writes, the
//! rules a publish applies, the tar of one version, the materialized
//! copy inside a Computer, and the contract of one tool call.
//!
//! It also holds the version store: the bare repository of each
//! package, the `software_publish` flow, and the search index the
//! Software List is browsed and searched through. The broker stays
//! about grants and snapshots: it only learns that a tool belongs to a
//! package, through `ToolRoute::Software`.

pub mod contribute;
pub mod git;
pub mod index;
pub mod list;
pub mod manifest;
pub mod materialize;
pub mod note;
pub mod origin;
pub mod pack;
pub mod run;
pub mod search;
pub mod validate;
pub mod widget;

pub use contribute::{AgentMessenger, Contributions, ContributionsDeps, MAX_INLINE_PATCH_BYTES};
pub use git::{GitStoreError, NewVersion, Patch, PublishAuthor, SoftwareGitStore, VersionRef};
pub use index::{IndexedPackage, IndexedTool, PackageHit, ToolIndex};
pub use list::{ManifestSink, SoftwareList, SoftwareListDeps, next_version};
pub use manifest::{
    DEFAULT_TIMEOUT_S, MANIFEST_FILE, MAX_TIMEOUT_S, Manifest, Package, PackageVersion, ToolSpec,
};
pub use materialize::{Materializer, SOFTWARE_ROOT, VersionSource, package_root};
pub use note::{
    MemoryNotes, SOFTWARE_NOTE_FILE, SoftwareNotes, close_note, contribute_note, fork_note,
    publish_note,
};
pub use origin::{MAX_ORIGIN_BYTES, ORIGIN_FILE, Origin};
pub use pack::{MAX_GITIGNORE_BYTES, MAX_PACKAGE_BYTES, MAX_PACKAGE_ENTRIES, Packed, pack};
pub use run::SoftwareRunner;
pub use search::{EMPTY_LIST, NO_MATCH, SearchOutcome, tool_search};
pub use validate::{MAX_MANIFEST_BYTES, ValidationErrors, validate};
pub use widget::{
    MAX_STRUCTURED_CONTENT_BYTES, MAX_WIDGET_HTML_BYTES, Visibility, WIDGET_MIME, WidgetCsp,
    WidgetResult, WidgetSpec, is_csp_origin, parse_widget_uri, split_result, widget_uri,
};
