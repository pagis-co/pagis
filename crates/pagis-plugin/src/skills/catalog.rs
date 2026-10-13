//! The native Skill catalog and the first-party Skill files.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pagis_core::{
    AgentId, FIRST_PARTY_SKILLS, Grant, GrantStore, Plugin, PluginState, PluginStore, Skill,
    SkillMount, Skills, WorkspaceId,
};
use rust_embed::RustEmbed;

use super::{SKILL_FILE, description, read_skills, split_frontmatter};
use crate::PluginStates;
use crate::git::PluginGitStore;
use crate::manifest::SKILLS_DIR;

/// The first-party Skills, from the same directory the Computer image
/// copies to `/opt/pagis/skills/`.
#[derive(RustEmbed)]
#[folder = "../../computer/skills"]
struct FirstPartySkills;

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
    pub states: Arc<dyn PluginStates>,
}

/// The Skills of every Workspace, by tenant and Agent
/// (ADR-0017). It reads the installed state id of each Plugin.
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
        for (plugin, _) in self.granted(workspace_id, agent_id).await {
            if let Ok(files) = self
                .deps
                .states
                .read(workspace_id, plugin.id.as_str(), &plugin.installed_commit)
                .await
            {
                skills.extend(read_skills(&plugin.name, &files).skills);
            }
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
        let (held, _) = self
            .granted(workspace_id, agent_id)
            .await
            .into_iter()
            .find(|(held, _)| held.name == plugin)?;
        let files = self
            .deps
            .states
            .read(workspace_id, held.id.as_str(), &held.installed_commit)
            .await
            .ok()?;
        String::from_utf8(
            files
                .get(PathBuf::from(SKILLS_DIR).join(skill).join(SKILL_FILE))?
                .bytes
                .clone(),
        )
        .ok()
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
    use super::is_one_segment;

    #[test]
    fn a_skill_name_that_walks_out_of_the_plugin_is_refused() {
        assert!(is_one_segment("invoices"));
        assert!(!is_one_segment(".."));
        assert!(!is_one_segment("../../etc/passwd"));
        assert!(!is_one_segment(""));
    }
}
