//! One plugin package on disk, written by the test that needs it.
//!
//! Every test binary compiles the whole module, so a builder only one
//! of them calls is not dead code.
#![allow(dead_code)]

use std::path::Path;

use tempfile::TempDir;

/// The `$schema` values every valid package carries.
pub const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
pub const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

/// A package directory a test fills in.
pub struct Package {
    pub directory: TempDir,
}

impl Package {
    pub fn new() -> Self {
        Self {
            directory: tempfile::tempdir().expect("a scratch directory"),
        }
    }

    pub fn root(&self) -> &Path {
        self.directory.path()
    }

    /// Write `plugin.json` from the fields beside `$schema`.
    pub fn manifest(self, body: serde_json::Value) -> Self {
        let mut document = serde_json::json!({ "$schema": PLUGIN_SCHEMA });
        merge(&mut document, body);
        self.file("plugin.json", &document.to_string())
    }

    /// Write `mcp.json` from the servers it declares.
    pub fn mcp(self, servers: serde_json::Value) -> Self {
        let document = serde_json::json!({
            "$schema": MCP_SCHEMA,
            "mcpServers": servers,
        });
        self.file("mcp.json", &document.to_string())
    }

    pub fn file(self, path: &str, text: &str) -> Self {
        let target = self.root().join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("a directory");
        }
        std::fs::write(target, text).expect("a file");
        self
    }

    pub fn directory(self, path: &str) -> Self {
        std::fs::create_dir_all(self.root().join(path)).expect("a directory");
        self
    }

    /// The package as one uncompressed tar, as an upload carries it.
    pub fn tar(&self) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        builder
            .append_dir_all(".", self.root())
            .expect("the upload tar");
        builder.into_inner().expect("the upload tar")
    }
}

fn merge(into: &mut serde_json::Value, from: serde_json::Value) {
    let (Some(into), Some(from)) = (into.as_object_mut(), from.as_object()) else {
        return;
    };
    for (key, value) in from {
        into.insert(key.clone(), value.clone());
    }
}

/// The smallest package that installs: a name and nothing else.
pub fn minimal() -> Package {
    Package::new().manifest(serde_json::json!({ "name": "weather" }))
}
