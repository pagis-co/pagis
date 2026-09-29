//! The origin marker of a Fork (ADR-0016).
//!
//! `software_fork` writes `.pagis-origin.toml` at the root of the new
//! working copy. The fork's first publish reads it, writes the origin
//! on the package record, and drops the file: a published Version
//! never holds it, because the record is where the origin lives.

use serde::{Deserialize, Serialize};

/// The marker file `software_fork` writes and a publish consumes.
pub const ORIGIN_FILE: &str = ".pagis-origin.toml";
/// The largest origin marker a publish reads. The marker that
/// `software_fork` writes holds a comment and two short lines.
pub const MAX_ORIGIN_BYTES: u64 = 4 * 1024;

/// Where one Fork came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    /// The name of the package the Fork was copied from.
    pub package: String,
    /// The Version of that package the copy was taken at.
    pub version: String,
}

impl Origin {
    /// The text of the marker file.
    pub fn to_toml(&self) -> String {
        format!(
            "# Written by software_fork. The first publish reads it and drops it.\npackage \
             = \"{}\"\nversion = \"{}\"\n",
            self.package, self.version
        )
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| format!("{ORIGIN_FILE} does not parse: {error}"))
    }

    /// The origin that one marker file declares. The read stops after
    /// `MAX_ORIGIN_BYTES`, so a marker never takes more memory than that.
    pub fn read(marker: impl std::io::Read) -> Result<Self, String> {
        use std::io::Read;

        let mut bytes = Vec::new();
        marker
            .take(MAX_ORIGIN_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read {ORIGIN_FILE}: {error}"))?;
        if bytes.len() as u64 > MAX_ORIGIN_BYTES {
            return Err(format!(
                "{ORIGIN_FILE} is larger than the limit of {MAX_ORIGIN_BYTES} bytes"
            ));
        }
        let text = String::from_utf8(bytes)
            .map_err(|error| format!("cannot read {ORIGIN_FILE}: {error}"))?;
        Self::parse(&text)
    }
}

/// Rewrite the `name` of the `[package]` table. The rest of the
/// manifest, the comments included, stays as the author wrote it.
pub fn rename_package(manifest: &str, new_name: &str) -> Result<String, String> {
    let mut inside = false;
    let mut renamed = false;
    let mut lines = Vec::new();
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            inside = trimmed == "[package]";
        }
        if inside && !renamed && is_name_assignment(trimmed) {
            lines.push(format!("name = \"{new_name}\""));
            renamed = true;
            continue;
        }
        lines.push(line.to_string());
    }
    if !renamed {
        return Err(format!(
            "the {} of the version has no [package] name",
            crate::manifest::MANIFEST_FILE
        ));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    Ok(text)
}

/// Whether one line assigns the key `name`.
fn is_name_assignment(line: &str) -> bool {
    line.strip_prefix("name")
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}
