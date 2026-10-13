//! A Plugin Package held in memory.

use pagis_plugin::PackageFiles;

pub const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
pub const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

pub struct Package {
    pub files: PackageFiles,
}

impl Package {
    pub fn new() -> Self {
        Self {
            files: PackageFiles::default(),
        }
    }

    pub fn manifest(self, body: serde_json::Value) -> Self {
        let mut document = serde_json::json!({ "$schema": PLUGIN_SCHEMA });
        document
            .as_object_mut()
            .unwrap()
            .extend(body.as_object().unwrap().clone());
        self.file("plugin.json", &document.to_string())
    }

    pub fn mcp(self, servers: serde_json::Value) -> Self {
        self.file(
            "mcp.json",
            &serde_json::json!({
                "$schema": MCP_SCHEMA, "mcpServers": servers,
            })
            .to_string(),
        )
    }

    pub fn file(mut self, path: &str, text: &str) -> Self {
        self.files
            .insert(path, text.as_bytes().to_vec(), false)
            .unwrap();
        self
    }
}

pub fn minimal() -> Package {
    Package::new().manifest(serde_json::json!({ "name": "weather" }))
}
