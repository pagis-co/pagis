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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use pagis_core::{
    AgentId, FIRST_PARTY_SKILLS, Grant, GrantStore, MAX_SKILL_DESCRIPTION, Plugin, PluginState,
    PluginStore, Skill, SkillMount, Skills, WorkspaceId,
};
use rust_embed::RustEmbed;

use crate::git::PluginGitStore;
use crate::manifest::SKILLS_DIR;

/// The one file that makes a directory a Skill.
pub const SKILL_FILE: &str = "SKILL.md";

/// The first-party Skills, from the same directory the Computer image
/// copies to `/opt/pagis/skills/`.
#[derive(RustEmbed)]
#[folder = "../../computer/skills"]
struct FirstPartySkills;

/// What one read of a `skills/` directory found: the Skills, and the
/// problems an install must refuse.
pub(crate) struct SkillsRead {
    pub skills: Vec<Skill>,
    pub problems: Vec<String>,
}

/// Every Skill under `root/skills/`. Each immediate child that carries
/// a `SKILL.md` is one Skill; nothing deeper is searched (ADR-0017).
/// A frontmatter `name` that is not the directory name is a problem,
/// because the directory alone names the mount and the load.
pub(crate) fn read_skills(plugin: &str, root: &Path) -> SkillsRead {
    let mut skills = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(root.join(SKILLS_DIR)) else {
        return SkillsRead { skills, problems };
    };
    let mut directories: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join(SKILL_FILE).is_file())
        .collect();
    directories.sort();
    for directory in directories {
        let Some(name) = directory.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(directory.join(SKILL_FILE)) else {
            problems.push(format!("cannot read {SKILLS_DIR}/{name}/{SKILL_FILE}"));
            continue;
        };
        let (front, body) = split_frontmatter(&text);
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

/// A Skill name from the model, refused unless it is one path segment.
/// The name reaches the filesystem, so it may not walk out of the
/// Plugin.
fn is_one_segment(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

pub struct SkillCatalogDeps {
    pub plugins: Arc<dyn PluginStore>,
    pub grants: Arc<dyn GrantStore>,
    pub git: Arc<PluginGitStore>,
}

/// The Skills of every Workspace, by tenant and Agent
/// (ADR-0017). It answers from the installed checkouts, so an update changes
/// what it says as soon as the checkout changes.
pub struct SkillCatalog {
    deps: SkillCatalogDeps,
}

impl SkillCatalog {
    pub fn new(deps: SkillCatalogDeps) -> Self {
        Self { deps }
    }

    /// The enabled Plugins the Agent holds a live Grant on, in install
    /// order, with the checkout each one's Skills live in.
    async fn granted(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Vec<(Plugin, PathBuf)> {
        let grants = self
            .deps
            .grants
            .list_live_for_agent(workspace_id, agent_id)
            .await
            .unwrap_or_default();
        let plugins = self
            .deps
            .plugins
            .list(workspace_id)
            .await
            .unwrap_or_default();
        let mut granted: Vec<Plugin> = plugins
            .into_iter()
            .filter(|plugin| plugin.state == PluginState::Enabled && holds(&grants, plugin))
            .collect();
        // The listing is in install order (ADR-0017), so the cap drops
        // the newest Plugins and never reshuffles the older ones.
        granted.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.name.cmp(&right.name))
        });
        granted
            .into_iter()
            .map(|plugin| {
                let root = self.deps.git.paths(workspace_id, plugin.id.as_str()).root;
                (plugin, root)
            })
            .collect()
    }
}

#[async_trait]
impl Skills for SkillCatalog {
    async fn list(&self, workspace_id: &WorkspaceId, agent_id: &AgentId) -> Vec<Skill> {
        let mut skills = first_party_skills();
        for (plugin, root) in self.granted(workspace_id, agent_id).await {
            skills.extend(read_skills(&plugin.name, &root).skills);
        }
        skills
    }

    async fn body(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        plugin: &str,
        skill: &str,
    ) -> Option<String> {
        if !is_one_segment(skill) {
            return None;
        }
        if plugin == FIRST_PARTY_SKILLS {
            return first_party_body(skill);
        }
        let (_, root) = self
            .granted(workspace_id, agent_id)
            .await
            .into_iter()
            .find(|(held, _)| held.name == plugin)?;
        std::fs::read_to_string(root.join(SKILLS_DIR).join(skill).join(SKILL_FILE)).ok()
    }

    async fn mounts(&self, workspace_id: &WorkspaceId, agent_id: &AgentId) -> Vec<SkillMount> {
        self.granted(workspace_id, agent_id)
            .await
            .into_iter()
            .filter_map(|(plugin, root)| {
                let skills_dir = root.join(SKILLS_DIR);
                skills_dir.is_dir().then_some(SkillMount {
                    plugin: plugin.name,
                    skills_dir,
                })
            })
            .collect()
    }
}

fn holds(grants: &[Grant], plugin: &Plugin) -> bool {
    grants.iter().any(|grant| {
        grant.resource_kind == Grant::PLUGIN_KIND
            && grant.resource_id.as_deref() == Some(plugin.id.as_str())
    })
}

/// The first-party Skills, listed for every Agent (ADR-0017).
pub fn first_party_skills() -> Vec<Skill> {
    let mut names: Vec<String> = FirstPartySkills::iter()
        .filter_map(|path| Some(path.strip_suffix(&format!("/{SKILL_FILE}"))?.to_string()))
        .filter(|name| is_one_segment(name))
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let text = first_party_body(&name)?;
            let (front, body) = split_frontmatter(&text);
            Some(Skill {
                plugin: FIRST_PARTY_SKILLS.to_string(),
                description: description(front, body),
                name,
            })
        })
        .collect()
}

fn first_party_body(skill: &str) -> Option<String> {
    let file = FirstPartySkills::get(&format!("{skill}/{SKILL_FILE}"))?;
    String::from_utf8(file.data.into_owned()).ok()
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

    #[test]
    fn a_skill_name_that_walks_out_of_the_plugin_is_refused() {
        assert!(is_one_segment("invoices"));
        assert!(!is_one_segment(".."));
        assert!(!is_one_segment("../../etc/passwd"));
        assert!(!is_one_segment(""));
    }
}
