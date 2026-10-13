//! The Skills of the installed Plugins (ADR-0017).
//!
//! A Skill is a directory under `skills/` that carries a `SKILL.md`.
//! The frontmatter of that file gives the name and the description the
//! listing shows; the body is what `skill_load` returns. Nothing is
//! copied: the listing and the load read the installed checkout, and
//! the Computer mounts the same directory read-only.
//!
//! The first-party Skills the Computer image ships at
//! `/opt/pagis/skills/` are the same files, embedded in the daemon so
//! it can list and load them without reaching into a container.

use pagis_core::{MAX_SKILL_DESCRIPTION, Skill};

use crate::manifest::SKILLS_DIR;
use crate::package::PackageFiles;

#[cfg(feature = "native")]
mod catalog;
#[cfg(feature = "native")]
pub use catalog::{SkillCatalog, SkillCatalogDeps, first_party_skills};

/// The one file that makes a directory a Skill.
pub const SKILL_FILE: &str = "SKILL.md";

/// What one read of a `skills/` directory found: the Skills, and the
/// problems an install must refuse.
pub(crate) struct SkillsRead {
    pub skills: Vec<Skill>,
    pub problems: Vec<String>,
}

/// Every Skill under `skills/` in the Plugin Package. Each immediate child that carries
/// a `SKILL.md` is one Skill; nothing deeper is searched (ADR-0017).
/// A frontmatter `name` that is not the directory name is a problem,
/// because the directory alone names the mount and the load.
pub(crate) fn read_skills(plugin: &str, root: &PackageFiles) -> SkillsRead {
    let mut skills = Vec::new();
    let mut problems = Vec::new();
    for (path, _) in root.iter() {
        let Ok(relative) = path.strip_prefix(SKILLS_DIR) else {
            continue;
        };
        let parts: Vec<_> = relative.components().collect();
        if parts.len() != 2 || parts[1].as_os_str() != SKILL_FILE {
            continue;
        }
        let Some(name) = parts[0].as_os_str().to_str() else {
            continue;
        };
        let Some(path) = path.to_str() else {
            continue;
        };
        let Ok(text) = root.read_text(path) else {
            problems.push(format!("cannot read {SKILLS_DIR}/{name}/{SKILL_FILE}"));
            continue;
        };
        let (front, body) = split_frontmatter(text);
        if let Some(declared) = front.and_then(|front| field(front, "name"))
            && declared != name
        {
            problems.push(format!(
                "the skill in {SKILLS_DIR}/{name} declares the name {declared:?}; \
                 a skill is named by its directory"
            ));
        }
        skills.push(Skill {
            plugin: plugin.to_string(),
            name: name.to_string(),
            description: description(front, body),
        });
    }
    SkillsRead { skills, problems }
}

/// The description the listing shows: the frontmatter `description`,
/// or the first line of the body, capped (ADR-0017).
fn description(front: Option<&str>, body: &str) -> String {
    let declared = front.and_then(|front| field(front, "description"));
    let text = declared.unwrap_or_else(|| first_line(body));
    cap(&text, MAX_SKILL_DESCRIPTION)
}

/// The YAML frontmatter block and the body after it. A file that does
/// not open with a `---` line, or that never closes one, carries no
/// frontmatter and is all body.
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(opened) = open_marker(text) else {
        return (None, text);
    };
    let mut offset = opened;
    while offset < text.len() {
        let end = text[offset..]
            .find('\n')
            .map(|index| offset + index + 1)
            .unwrap_or(text.len());
        if text[offset..end].trim_end() == "---" {
            return (Some(&text[opened..offset]), &text[end..]);
        }
        offset = end;
    }
    (None, text)
}

/// Where the frontmatter starts, when the first line is the marker.
fn open_marker(text: &str) -> Option<usize> {
    let end = text.find('\n')?;
    (text[..end].trim_end() == "---").then_some(end + 1)
}

/// One top-level scalar of the frontmatter. A value that spans lines
/// is not read: a listed field is one line by definition.
fn field(front: &str, key: &str) -> Option<String> {
    for line in front.lines() {
        let Some(value) = line.strip_prefix(key) else {
            continue;
        };
        let Some(value) = value.strip_prefix(':') else {
            continue;
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        if value.is_empty() {
            return None;
        }
        return Some(value.to_string());
    }
    None
}

/// The first line of the body that says something, without its
/// Markdown heading marks.
fn first_line(body: &str) -> String {
    body.lines()
        .map(|line| line.trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// One line of at most `limit` characters. The text is folded to
/// single spaces first, because the listing gives each Skill one line.
fn cap(text: &str, limit: usize) -> String {
    let folded = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if folded.chars().count() <= limit {
        return folded;
    }
    let kept: String = folded.chars().take(limit - 1).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frontmatter_gives_the_description() {
        let (front, body) = split_frontmatter(
            "---\nname: invoices\ndescription: Raise an invoice.\n---\n\n# Invoices\n\nSteps.\n",
        );
        assert_eq!(field(front.unwrap(), "name").as_deref(), Some("invoices"));
        assert_eq!(description(front, body), "Raise an invoice.");
        assert!(body.starts_with("\n# Invoices"));
    }

    #[test]
    fn a_quoted_value_loses_its_quotes() {
        let (front, _) = split_frontmatter("---\ndescription: \"Raise an invoice.\"\n---\nBody\n");
        assert_eq!(
            field(front.unwrap(), "description").as_deref(),
            Some("Raise an invoice.")
        );
    }

    #[test]
    fn a_file_with_no_frontmatter_describes_itself_by_its_first_line() {
        let (front, body) = split_frontmatter("# Invoices\n\nRaise an invoice.\n");
        assert!(front.is_none());
        assert_eq!(description(front, body), "Invoices");
    }

    #[test]
    fn an_unclosed_frontmatter_is_body() {
        let (front, body) = split_frontmatter("---\nname: invoices\n\n# Invoices\n");
        assert!(front.is_none());
        assert!(body.starts_with("---"));
    }

    #[test]
    fn a_long_description_is_capped_to_one_line() {
        let long = "word ".repeat(100);
        let capped = cap(&long, MAX_SKILL_DESCRIPTION);
        assert_eq!(capped.chars().count(), MAX_SKILL_DESCRIPTION);
        assert!(capped.ends_with('…'));
        assert!(!capped.contains('\n'));
    }
}
