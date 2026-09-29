//! Skills (ADR-0017): the instruction documents a Plugin ships
//! and the first-party set the Computer image carries.
//!
//! A Skill is listed in the system prompt, loaded on demand with
//! `skill_load`, and its directory is mounted into the Computer. The
//! three consumers — the run loop, the core tool runtime and the
//! Computer manager — reach one catalogue through [`Skills`]. Nothing
//! of a Skill enters a memory repository.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::id::{AgentId, WorkspaceId};

/// The separator between the Plugin and the Skill in the name the
/// model sees: `<plugin>:<skill>`.
pub const SKILL_SEPARATOR: char = ':';

/// The longest description a listed Skill carries (ADR-0017). The
/// system prompt holds one line per Skill, so a long description
/// would crowd out the rest of the prompt.
pub const MAX_SKILL_DESCRIPTION: usize = 200;

/// One Skill an Agent may load, as the listing shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// The Plugin that ships it, by name. First-party Skills carry
    /// [`FIRST_PARTY_SKILLS`].
    pub plugin: String,
    /// The directory the Skill lives in, which is also its name.
    pub name: String,
    /// The frontmatter description, capped at
    /// [`MAX_SKILL_DESCRIPTION`].
    pub description: String,
}

/// The Plugin name the first-party Skills of the Computer image carry.
/// It is reserved: no installed Plugin may take it.
pub const FIRST_PARTY_SKILLS: &str = "pagis";

impl Skill {
    /// The name the model calls the Skill by: `<plugin>:<skill>`.
    pub fn qualified(&self) -> String {
        format!("{}{SKILL_SEPARATOR}{}", self.plugin, self.name)
    }

    /// Split `<plugin>:<skill>` back into its two halves. A name with
    /// no separator, or with an empty half, is not a Skill name.
    pub fn split_qualified(name: &str) -> Option<(&str, &str)> {
        let (plugin, skill) = name.split_once(SKILL_SEPARATOR)?;
        if plugin.is_empty() || skill.is_empty() {
            return None;
        }
        Some((plugin, skill))
    }
}

/// One Plugin's `skills/` directory on this computer, for the Computer
/// to mount read-only (ADR-0017).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMount {
    /// The Plugin's name, which names the mount inside the container.
    pub plugin: String,
    /// The host directory that holds the Plugin's Skills.
    pub skills_dir: PathBuf,
}

/// The Skills one Agent of one tenant may reach now (ADR-0017). Every
/// method takes the Grant into account, so a caller never has to check
/// one, and every method names the Workspace, so one catalogue serves
/// every tenant of the daemon.
#[async_trait]
pub trait Skills: Send + Sync {
    /// Every Skill the Agent may load, first-party first and then the
    /// Plugins in install order.
    async fn list(&self, workspace_id: &WorkspaceId, agent_id: &AgentId) -> Vec<Skill>;

    /// The SKILL.md body of one Skill, or `None` when the Agent may
    /// not reach it or it does not exist.
    async fn body(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        plugin: &str,
        skill: &str,
    ) -> Option<String>;

    /// The `skills/` directory of every Plugin the Agent holds a Grant
    /// on, in the same order as the listing.
    async fn mounts(&self, workspace_id: &WorkspaceId, agent_id: &AgentId) -> Vec<SkillMount>;
}

/// The catalogue of a daemon that serves no Skills. Tests that do not
/// exercise Skills take it instead of building a Plugin store.
pub struct NoSkills;

#[async_trait]
impl Skills for NoSkills {
    async fn list(&self, _workspace_id: &WorkspaceId, _agent_id: &AgentId) -> Vec<Skill> {
        Vec::new()
    }

    async fn body(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _plugin: &str,
        _skill: &str,
    ) -> Option<String> {
        None
    }

    async fn mounts(&self, _workspace_id: &WorkspaceId, _agent_id: &AgentId) -> Vec<SkillMount> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_qualified_name_joins_the_plugin_and_the_skill() {
        let skill = Skill {
            plugin: "acme".to_string(),
            name: "invoices".to_string(),
            description: String::new(),
        };
        assert_eq!(skill.qualified(), "acme:invoices");
        assert_eq!(
            Skill::split_qualified("acme:invoices"),
            Some(("acme", "invoices"))
        );
    }

    #[test]
    fn a_name_with_no_separator_or_an_empty_half_is_not_a_skill_name() {
        assert_eq!(Skill::split_qualified("invoices"), None);
        assert_eq!(Skill::split_qualified(":invoices"), None);
        assert_eq!(Skill::split_qualified("acme:"), None);
    }
}
