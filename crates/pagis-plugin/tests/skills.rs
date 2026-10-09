//! The Skills catalogue (ADR-0017): what an Agent is listed, what
//! it can load, and which directories its Computer mounts.

use std::sync::Arc;

use pagis_core::{
    AgentId, FIRST_PARTY_SKILLS, Grant, GrantId, GrantStore, Plugin, PluginId, PluginSource,
    PluginState, PluginStore, Skills, WorkspaceId, now_ms,
};
use pagis_plugin::{PluginGitStore, SkillCatalog, SkillCatalogDeps};
use pagis_testkit::{MemoryGrantStore, MemoryPluginStore};

struct Fixture {
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    plugins: Arc<MemoryPluginStore>,
    grants: Arc<MemoryGrantStore>,
    git: Arc<PluginGitStore>,
    catalog: SkillCatalog,
    /// The plugin data root; it must outlive the catalogue.
    _root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let workspace_id = WorkspaceId::generate();
        let root = tempfile::tempdir().expect("plugin root");
        let grants = Arc::new(MemoryGrantStore::default());
        let plugins = Arc::new(MemoryPluginStore::new(Arc::clone(&grants) as _));
        let git = Arc::new(PluginGitStore::new(root.path()));
        let catalog = SkillCatalog::new(SkillCatalogDeps {
            plugins: Arc::clone(&plugins) as _,
            grants: Arc::clone(&grants) as _,
            git: Arc::clone(&git),
        });
        Self {
            agent_id: AgentId::generate(),
            workspace_id,
            plugins,
            grants,
            git,
            catalog,
            _root: root,
        }
    }

    /// Install one Plugin with the Skills named, and write each one's
    /// `SKILL.md` into the checkout the catalogue reads.
    async fn install(&self, name: &str, created_at: i64, skills: &[(&str, &str)]) -> Plugin {
        let plugin = Plugin {
            id: PluginId::generate(),
            workspace_id: self.workspace_id.clone(),
            name: name.to_string(),
            source: PluginSource::Upload,
            installed_commit: "0".repeat(40),
            manifest_version: "v1".to_string(),
            state: PluginState::Enabled,
            created_at,
            updated_at: created_at,
        };
        self.plugins.create(&plugin, &[]).await.expect("installed");
        let root = self.git.paths(&self.workspace_id, plugin.id.as_str()).root;
        for (skill, text) in skills {
            let directory = root.join("skills").join(skill);
            std::fs::create_dir_all(&directory).expect("skill directory");
            std::fs::write(directory.join("SKILL.md"), text).expect("skill file");
        }
        plugin
    }

    async fn grant(&self, plugin: &Plugin) {
        self.grants
            .create(&Grant {
                id: GrantId::generate(),
                workspace_id: self.workspace_id.clone(),
                agent_id: self.agent_id.clone(),
                resource_kind: Grant::PLUGIN_KIND.to_string(),
                resource_id: Some(plugin.id.to_string()),
                scope: serde_json::json!({}),
                revision: 1,
                created_at: now_ms(),
                revoked_at: None,
            })
            .await
            .expect("granted");
    }

    /// The listing without the first-party Skills, which every Agent
    /// gets and which no test installs.
    async fn plugin_skills(&self) -> Vec<String> {
        self.catalog
            .list(&self.workspace_id, &self.agent_id)
            .await
            .into_iter()
            .filter(|skill| skill.plugin != FIRST_PARTY_SKILLS)
            .map(|skill| format!("{}: {}", skill.qualified(), skill.description))
            .collect()
    }

    async fn coding_sessions_body(&self) -> String {
        self.catalog
            .body(
                &self.workspace_id,
                &self.agent_id,
                FIRST_PARTY_SKILLS,
                "coding-sessions",
            )
            .await
            .expect("the body of the coding-sessions Skill")
    }
}

const FORECAST: &str = "---\nname: forecast\ndescription: Read the forecast.\n---\n\n# Forecast\n\nCall the service.\n";

#[tokio::test]
async fn a_granted_plugin_lists_its_skills_and_a_plugin_with_no_grant_lists_none() {
    let fixture = Fixture::new();
    let granted = fixture
        .install("weather", 1, &[("forecast", FORECAST)])
        .await;
    fixture
        .install("payroll", 2, &[("payslips", "# Payslips\n")])
        .await;
    fixture.grant(&granted).await;

    assert_eq!(
        fixture.plugin_skills().await,
        vec!["weather:forecast: Read the forecast.".to_string()]
    );
}

#[tokio::test]
async fn a_disabled_plugin_lists_nothing_even_with_a_grant() {
    let fixture = Fixture::new();
    let plugin = fixture
        .install("weather", 1, &[("forecast", FORECAST)])
        .await;
    fixture.grant(&plugin).await;
    fixture
        .plugins
        .set_state(
            &fixture.workspace_id,
            &plugin.id,
            PluginState::Disabled,
            now_ms(),
        )
        .await
        .expect("disabled");

    assert!(fixture.plugin_skills().await.is_empty());
}

#[tokio::test]
async fn the_listing_follows_install_order() {
    let fixture = Fixture::new();
    let second = fixture.install("alpha", 20, &[("one", "# One\n")]).await;
    let first = fixture.install("zulu", 10, &[("two", "# Two\n")]).await;
    fixture.grant(&first).await;
    fixture.grant(&second).await;

    let names: Vec<String> = fixture
        .catalog
        .list(&fixture.workspace_id, &fixture.agent_id)
        .await
        .into_iter()
        .filter(|skill| skill.plugin != FIRST_PARTY_SKILLS)
        .map(|skill| skill.qualified())
        .collect();
    assert_eq!(names, vec!["zulu:two".to_string(), "alpha:one".to_string()]);
}

#[tokio::test]
async fn the_first_party_skills_reach_every_agent() {
    let fixture = Fixture::new();
    let listed = fixture
        .catalog
        .list(&fixture.workspace_id, &fixture.agent_id)
        .await;
    let first_party: Vec<&pagis_core::Skill> = listed
        .iter()
        .filter(|skill| skill.plugin == FIRST_PARTY_SKILLS)
        .collect();
    assert!(
        !first_party.is_empty(),
        "the image ships at least one first-party skill"
    );
    for skill in &first_party {
        assert!(!skill.description.is_empty(), "{}", skill.qualified());
        assert!(
            fixture
                .catalog
                .body(
                    &fixture.workspace_id,
                    &fixture.agent_id,
                    FIRST_PARTY_SKILLS,
                    &skill.name
                )
                .await
                .is_some_and(|body| !body.trim().is_empty())
        );
    }
}

/// The widget Skill (ADR-0016) teaches the contract the daemon
/// applies, so every Agent must see it without a Grant.
#[tokio::test]
async fn the_widgets_skill_lists_and_loads_for_every_agent() {
    let fixture = Fixture::new();

    let listed = fixture
        .catalog
        .list(&fixture.workspace_id, &fixture.agent_id)
        .await;
    let widgets = listed
        .iter()
        .find(|skill| skill.qualified() == "pagis:widgets")
        .expect("the image ships the widgets Skill");
    assert!(
        widgets.description.contains("widget"),
        "{}",
        widgets.description
    );

    let body = fixture
        .catalog
        .body(
            &fixture.workspace_id,
            &fixture.agent_id,
            FIRST_PARTY_SKILLS,
            "widgets",
        )
        .await
        .expect("the body of the widgets Skill");
    for named in [
        "structuredContent",
        "awaits_input",
        "ui/message",
        "connect-src 'none'",
        "/opt/pagis/skills/widgets/scaffold",
    ] {
        assert!(body.contains(named), "the Skill names {named}");
    }
}

/// Each core tool of a Coding Session (ADR-0033). The coding-sessions
/// Skill names each one, and no other `coding_session_` word.
const CODING_SESSION_TOOLS: [&str; 12] = [
    pagis_broker::CODING_SESSION_START,
    pagis_broker::COMPUTER_CODING_SESSION_START,
    pagis_broker::CODING_SESSION_SEND,
    pagis_broker::CODING_SESSION_READ,
    pagis_broker::CODING_SESSION_CANCEL,
    pagis_broker::CODING_SESSION_CLOSE,
    pagis_broker::CODING_SESSION_LIST,
    pagis_broker::CODING_SESSION_RESUME,
    pagis_broker::CODING_SESSION_DECIDE,
    pagis_broker::CODING_SESSION_ESCALATE,
    pagis_broker::CODING_SESSION_ANSWER,
    pagis_broker::CODING_SESSION_SET_MODE,
];

/// The coding-sessions Skill teaches an Agent to brief, supervise,
/// verify and report a Coding Session, so every Agent must see it
/// without a Grant.
#[tokio::test]
async fn the_coding_sessions_skill_lists_and_loads_for_every_agent() {
    let fixture = Fixture::new();

    let listed = fixture
        .catalog
        .list(&fixture.workspace_id, &fixture.agent_id)
        .await;
    let skill = listed
        .iter()
        .find(|skill| skill.qualified() == "pagis:coding-sessions")
        .expect("the image ships the coding-sessions Skill");
    assert!(
        skill.description.contains("Coding Harness"),
        "{}",
        skill.description
    );
    assert!(
        !skill.description.ends_with('…'),
        "the listing cuts the description: {}",
        skill.description
    );

    let body = fixture.coding_sessions_body().await;
    for named in CODING_SESSION_TOOLS.into_iter().chain([
        "`person`",
        "`agent`",
        "harness_mode",
        "pagis/<slug>",
        "private/skills/pagis/coding-sessions.md",
    ]) {
        assert!(body.contains(named), "the Skill names {named}");
    }
}

/// A renamed tool fails here: each `coding_session_` word of the
/// Skill is the name of a tool that exists.
#[tokio::test]
async fn each_coding_session_tool_the_skill_names_exists() {
    let fixture = Fixture::new();
    let body = fixture.coding_sessions_body().await;

    let words: Vec<&str> = body
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| word.contains("coding_session_"))
        .collect();
    assert!(!words.is_empty(), "the Skill names the tools");
    for word in words {
        assert!(
            CODING_SESSION_TOOLS.contains(&word),
            "the Skill names {word}, which is no tool"
        );
    }
}

#[tokio::test]
async fn a_body_comes_back_only_for_a_granted_plugin() {
    let fixture = Fixture::new();
    let plugin = fixture
        .install("weather", 1, &[("forecast", FORECAST)])
        .await;

    assert_eq!(
        fixture
            .catalog
            .body(
                &fixture.workspace_id,
                &fixture.agent_id,
                "weather",
                "forecast"
            )
            .await,
        None
    );
    fixture.grant(&plugin).await;
    let body = fixture
        .catalog
        .body(
            &fixture.workspace_id,
            &fixture.agent_id,
            "weather",
            "forecast",
        )
        .await
        .expect("the body of a granted skill");
    assert!(body.contains("Call the service."));
}

#[tokio::test]
async fn a_skill_name_that_walks_out_of_the_plugin_reads_nothing() {
    let fixture = Fixture::new();
    let plugin = fixture
        .install("weather", 1, &[("forecast", FORECAST)])
        .await;
    fixture.grant(&plugin).await;

    assert_eq!(
        fixture
            .catalog
            .body(
                &fixture.workspace_id,
                &fixture.agent_id,
                "weather",
                "../../plugin.json"
            )
            .await,
        None
    );
}

#[tokio::test]
async fn the_mounts_name_the_skills_directory_of_every_granted_plugin() {
    let fixture = Fixture::new();
    let plugin = fixture
        .install("weather", 1, &[("forecast", FORECAST)])
        .await;
    fixture
        .install("payroll", 2, &[("payslips", "# Payslips\n")])
        .await;
    fixture.grant(&plugin).await;

    let mounts = fixture
        .catalog
        .mounts(&fixture.workspace_id, &fixture.agent_id)
        .await;
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].plugin, "weather");
    assert_eq!(
        mounts[0].skills_dir,
        fixture
            .git
            .paths(&fixture.workspace_id, plugin.id.as_str())
            .root
            .join("skills")
    );
}

#[tokio::test]
async fn a_plugin_with_no_skills_directory_mounts_nothing() {
    let fixture = Fixture::new();
    let plugin = fixture.install("weather", 1, &[]).await;
    fixture.grant(&plugin).await;

    assert!(
        fixture
            .catalog
            .mounts(&fixture.workspace_id, &fixture.agent_id)
            .await
            .is_empty()
    );
}
