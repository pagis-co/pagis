//! `cargo xtask pins --check`: the pin rules of the third-party code that
//! the release workflows and the image builds run.
//!
//! An upstream owner can move a tag or a branch to other code, and the
//! next build then runs that code. A commit ID or a digest cannot move.
//! So the check fails on each of these references:
//!
//! - a `uses:` in `.github/workflows/` that does not name a full commit ID
//!   with its release tag in a trailing comment
//!   (`uses: owner/repo@<40 hex digits> # v1.2.3`). A path in this
//!   repository (`uses: ./.github/workflows/ci.yml`) runs the code of the
//!   same commit and needs no pin;
//! - an `actions/checkout` step that does not set
//!   `persist-credentials: false`. Without it, the job token stays in the
//!   git configuration for each later step;
//! - a `FROM` in a Dockerfile of [`DOCKERFILES`], or an `image:` in a
//!   Compose file of [`COMPOSE`], that does not name its image by tag and
//!   digest (`image:tag@sha256:<digest>`). The tag shows the version to a
//!   reader, and Dependabot uses it to find the next digest. The check
//!   skips the Headless Server image and the Push Relay image in the
//!   Compose files, because their tags are the versions that a deployment
//!   sets in `PAGIS_VERSION` and `PUSH_RELAY_VERSION`.
//!
//! The check reads the files line by line, in the block style that they
//! use, and does not parse YAML. It fails on a form that it does not read,
//! such as a flow mapping for `with:`, which is the safe direction.
//! `.github/dependabot.yml` opens the pull requests that move the pins.

use std::fmt;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::relay_image::RELAY_IMAGE_REPOSITORY;
use crate::server_image::SERVER_IMAGE_REPOSITORY;

/// The directory of the workflows. The check reads each `.yml` and
/// `.yaml` file in it.
pub const WORKFLOWS: &str = ".github/workflows";

/// The Dockerfiles of the Headless Server image, the Computer Image and
/// the Push Relay image.
pub const DOCKERFILES: [&str; 3] = [
    "Dockerfile",
    "computer/Dockerfile",
    "crates/pagis-push-relay/Dockerfile",
];

/// The Compose files of a Headless Server deployment and of a Push Relay
/// deployment.
pub const COMPOSE: [&str; 2] = ["deploy/compose.yaml", "deploy/push-relay/compose.yaml"];

/// The images whose tag is a version that a deployment sets, and not a
/// pin.
const DEPLOYED_IMAGES: [&str; 2] = [SERVER_IMAGE_REPOSITORY, RELAY_IMAGE_REPOSITORY];

/// One reference that breaks a pin rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The path of the file, relative to the repository root.
    pub file: String,
    /// The line of the reference, from 1.
    pub line: usize,
    pub reason: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.reason)
    }
}

/// Check the files of the repository at `root`, print each violation, and
/// fail when there is one.
pub fn run(root: &Path) -> Result<()> {
    let found = violations(root)?;
    if !found.is_empty() {
        for violation in &found {
            eprintln!("{violation}");
        }
        bail!(
            "{} reference(s) to third-party code can move; pin each one as stated above",
            found.len()
        );
    }
    println!(
        "each action is pinned to a commit ID, each checkout keeps no credentials, \
         and each image is pinned by digest"
    );
    Ok(())
}

/// The violations of each workflow, of each Dockerfile and of each
/// Compose file of the repository at `root`. A file that is missing is an
/// error, so a file that moves does not leave the check.
pub fn violations(root: &Path) -> Result<Vec<Violation>> {
    let mut found = Vec::new();
    for file in workflow_files(root)? {
        found.extend(workflow_violations(&file, &read(root, &file)?));
    }
    for file in DOCKERFILES {
        found.extend(dockerfile_violations(file, &read(root, file)?));
    }
    for file in COMPOSE {
        found.extend(compose_violations(file, &read(root, file)?));
    }
    Ok(found)
}

fn workflow_files(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join(WORKFLOWS))
        .with_context(|| format!("cannot read {WORKFLOWS}"))?
    {
        let name = entry
            .with_context(|| format!("cannot read {WORKFLOWS}"))?
            .file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".yml") || name.ends_with(".yaml") {
            files.push(format!("{WORKFLOWS}/{name}"));
        }
    }
    files.sort();
    Ok(files)
}

fn read(root: &Path, file: &str) -> Result<String> {
    std::fs::read_to_string(root.join(file)).with_context(|| format!("cannot read {file}"))
}

/// The violations of one workflow, `text`, at `file`.
pub fn workflow_violations(file: &str, text: &str) -> Vec<Violation> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        let Some((column, value)) = key_value(line, "uses") else {
            continue;
        };
        let mut violation = |reason: String| {
            found.push(Violation {
                file: file.to_string(),
                line: at + 1,
                reason,
            });
        };
        let (reference, tag) = split_comment(value);
        let reference = unquote(reference);
        // A workflow or an action of this repository runs from the same
        // commit as the workflow that names it.
        if reference.starts_with("./") {
            continue;
        }
        let (action, commit) = reference.rsplit_once('@').unwrap_or((reference, ""));
        if !is_lower_hex(commit, 40) {
            violation(format!(
                "`{reference}` does not name a 40-character commit ID; \
                 write `{action}@<commit> # <tag>`"
            ));
        } else if tag.is_empty() {
            violation(format!(
                "`{reference}` names no release tag; add it in a trailing comment (`# <tag>`)"
            ));
        }
        if action.eq_ignore_ascii_case("actions/checkout")
            && step_input(&lines, at, column, "persist-credentials") != Some("false")
        {
            violation(
                "the `actions/checkout` step does not set `persist-credentials: false`".into(),
            );
        }
    }
    found
}

/// The violations of one Dockerfile, `text`, at `file`.
pub fn dockerfile_violations(file: &str, text: &str) -> Vec<Violation> {
    let mut found = Vec::new();
    let mut stages: Vec<String> = Vec::new();
    let mut continued = false;
    for (at, line) in text.lines().enumerate() {
        let line = line.trim();
        // A comment line or an empty line inside an instruction does not
        // end the instruction.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let starts_instruction = !continued;
        continued = line.ends_with('\\');
        let mut words = line.split_whitespace();
        if !starts_instruction || !words.next().is_some_and(|w| w.eq_ignore_ascii_case("FROM")) {
            continue;
        }
        let words: Vec<&str> = words.filter(|word| !word.starts_with("--")).collect();
        let image = words.first().copied().unwrap_or_default();
        // A stage of the same file and `scratch` name no image. Stage
        // names are case-insensitive.
        let stage = image.to_ascii_lowercase();
        if stage != "scratch"
            && !stages.contains(&stage)
            && let Some(reason) = image_problem(image)
        {
            found.push(Violation {
                file: file.to_string(),
                line: at + 1,
                reason,
            });
        }
        if let [_, keyword, name] = words.as_slice()
            && keyword.eq_ignore_ascii_case("AS")
        {
            stages.push(name.to_ascii_lowercase());
        }
    }
    found
}

/// The violations of one Compose file, `text`, at `file`.
pub fn compose_violations(file: &str, text: &str) -> Vec<Violation> {
    let mut found = Vec::new();
    for (at, line) in text.lines().enumerate() {
        let Some((_, value)) = key_value(line, "image") else {
            continue;
        };
        let image = unquote(split_comment(value).0);
        let deployed = DEPLOYED_IMAGES.iter().any(|repository| {
            image
                .strip_prefix(repository)
                .is_some_and(|rest| rest.starts_with(':'))
        });
        if deployed {
            continue;
        }
        if let Some(reason) = image_problem(image) {
            found.push(Violation {
                file: file.to_string(),
                line: at + 1,
                reason,
            });
        }
    }
    found
}

/// Why `image` is not pinned as `name:tag@sha256:<64 hex digits>`, or
/// `None` when it is.
fn image_problem(image: &str) -> Option<String> {
    let Some((name, digest)) = image.split_once('@') else {
        return Some(format!(
            "`{image}` has no `@sha256:` digest; write `{image}@sha256:<digest>`"
        ));
    };
    if !digest
        .strip_prefix("sha256:")
        .is_some_and(|hex| is_lower_hex(hex, 64))
    {
        return Some(format!(
            "`{image}` has no `@sha256:` digest of 64 hex digits"
        ));
    }
    // A registry port also has a colon, so the tag is after the last `/`.
    let tagged = name
        .rsplit('/')
        .next()
        .and_then(|last| last.split_once(':'))
        .is_some_and(|(_, tag)| !tag.is_empty());
    if !tagged {
        return Some(format!(
            "`{image}` has a digest but no tag; keep the tag beside the digest \
             (`{name}:<tag>@{digest}`)"
        ));
    }
    None
}

fn is_lower_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The column and the value of `key` when `line` holds that key of a
/// block mapping, also as the first key of a sequence item (`- key:`).
fn key_value<'a>(line: &'a str, key: &str) -> Option<(usize, &'a str)> {
    let mut column = indent(line);
    let mut rest = &line[column..];
    if let Some(item) = rest.strip_prefix("- ") {
        let key_start = item.trim_start();
        column += rest.len() - key_start.len();
        rest = key_start;
    }
    let value = rest.strip_prefix(key)?.strip_prefix(':')?;
    if !value.is_empty() && !value.starts_with(' ') {
        return None;
    }
    Some((column, value.trim()))
}

/// The value of the input `name` in the `with:` mapping of the step that
/// has a key at `column` on line `at`, or `None` when the step does not
/// set it.
fn step_input<'a>(lines: &[&'a str], at: usize, column: usize, name: &str) -> Option<&'a str> {
    let content = |line: &str| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with('#')
    };
    let item = |line: &str| line.trim_start().starts_with("- ");
    // The keys of the step are at `column`, and the first key can be on
    // the line of the sequence item (`- key:`).
    let mut start = at;
    if !item(lines[at]) {
        for i in (0..at).rev() {
            if !content(lines[i]) {
                continue;
            }
            if indent(lines[i]) >= column {
                start = i;
                continue;
            }
            if item(lines[i]) && item_column(lines[i]) == column {
                start = i;
            }
            break;
        }
    }
    let end = (at + 1..lines.len())
        .find(|&i| content(lines[i]) && indent(lines[i]) < column)
        .unwrap_or(lines.len());
    let with = (start..end).find(|&i| {
        key_value(lines[i], "with")
            .is_some_and(|(key, value)| key == column && split_comment(value).0.is_empty())
    })?;
    (with + 1..end)
        .take_while(|&i| !content(lines[i]) || indent(lines[i]) > column)
        .find_map(|i| {
            let (_, value) = key_value(lines[i], name)?;
            Some(unquote(split_comment(value).0))
        })
}

/// The column of the first key of a sequence item line (`- key:`).
fn item_column(line: &str) -> usize {
    let dash = indent(line);
    let item = &line[dash + 1..];
    dash + 1 + (item.len() - item.trim_start().len())
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The value and the text of its trailing comment. A YAML comment starts
/// at a `#` after a space.
fn split_comment(value: &str) -> (&str, &str) {
    if let Some(comment) = value.strip_prefix('#') {
        return ("", comment.trim());
    }
    match value.find(" #") {
        Some(at) => (value[..at].trim(), value[at + 2..].trim()),
        None => (value.trim(), ""),
    }
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}
