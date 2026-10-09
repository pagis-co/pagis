//! The model route of each Coding Harness in the Agent's Computer
//! (ADR-0033).
//!
//! A harness in a Computer reaches a model only through the Harness Model
//! Endpoint of the daemon. Each harness has the routes of the endpoint
//! that it can use, in a fixed order, and a session takes the first route
//! whose provider holds an Org key. Anthropic comes first where the
//! harness speaks its API, because the Messages pass-through keeps the
//! most of the harness's own features.
//!
//! Each harness reads its provider from its own configuration: Claude
//! Code from its environment, Codex from `config.toml` in `CODEX_HOME`,
//! OpenCode from `OPENCODE_CONFIG_CONTENT`, and pi from `models.json` and
//! `settings.json` in `PI_CODING_AGENT_DIR`. A configuration directory
//! belongs to one session, under `/data/agent/.pagis/coding`, so a session
//! never changes the Agent's own `~/.codex` or `~/.pi`, and a resume finds
//! the harness's own session files again.
//!
//! The exec holds the token of the session in `PAGIS_MODEL_TOKEN`. No
//! configuration file holds the token: each one names the variable.

use std::collections::BTreeMap;
use std::sync::Arc;

use pagis_core::{
    AgentId, AgentStore, CodingSessionId, ModelAliasStore, Provider, ProviderKeys, WorkspaceId,
    harness,
};
use serde_json::json;

/// The variable that holds the token of the session in the exec of its
/// harness.
pub const MODEL_TOKEN_VARIABLE: &str = "PAGIS_MODEL_TOKEN";

/// The directory under which each session in a Computer has its own
/// configuration directory.
const SESSION_CONFIG_ROOT: &str = "/data/agent/.pagis/coding";

/// The name of the provider that the configuration of Codex and pi
/// defines. A built-in provider id of Codex cannot be overridden.
const PROVIDER_ID: &str = "pagis";

/// The API that a harness speaks on one route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelApi {
    /// The Anthropic Messages API.
    AnthropicMessages,
    /// The OpenAI Responses API.
    OpenAiResponses,
    /// The OpenAI Chat Completions API.
    OpenAiChatCompletions,
}

/// One route of the Harness Model Endpoint: the provider whose Org key
/// the requests spend, and the API of the requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointRoute {
    pub provider: Provider,
    pub api: ModelApi,
}

impl EndpointRoute {
    const fn new(provider: Provider, api: ModelApi) -> Self {
        Self { provider, api }
    }

    /// The base path of the route on the endpoint, such as
    /// `/openai/v1`.
    pub fn base_path(self) -> String {
        format!("/{}/v1", self.provider.id())
    }
}

const CLAUDE_ROUTES: &[EndpointRoute] = &[EndpointRoute::new(
    Provider::Anthropic,
    ModelApi::AnthropicMessages,
)];

/// Codex speaks the Responses API over HTTP, because the endpoint serves
/// no WebSocket.
const CODEX_ROUTES: &[EndpointRoute] = &[
    EndpointRoute::new(Provider::OpenAi, ModelApi::OpenAiResponses),
    EndpointRoute::new(Provider::OpenRouter, ModelApi::OpenAiResponses),
];

/// OpenCode's own `anthropic`, `openai` and `openrouter` providers, which
/// its binary bundles. Its `openai` provider speaks the Responses API, and
/// its `openrouter` provider the Chat Completions API.
const OPENCODE_ROUTES: &[EndpointRoute] = &[
    EndpointRoute::new(Provider::Anthropic, ModelApi::AnthropicMessages),
    EndpointRoute::new(Provider::OpenAi, ModelApi::OpenAiResponses),
    EndpointRoute::new(Provider::OpenRouter, ModelApi::OpenAiChatCompletions),
];

/// pi's `anthropic-messages` and `openai-completions` APIs.
const PI_ROUTES: &[EndpointRoute] = &[
    EndpointRoute::new(Provider::Anthropic, ModelApi::AnthropicMessages),
    EndpointRoute::new(Provider::OpenRouter, ModelApi::OpenAiChatCompletions),
    EndpointRoute::new(Provider::OpenAi, ModelApi::OpenAiChatCompletions),
];

/// The routes that the harness can use, in the order that a session
/// tries them. Empty for a harness that does not run in a Computer.
pub fn routes(harness_id: &str) -> &'static [EndpointRoute] {
    match harness_id {
        "claude" => CLAUDE_ROUTES,
        "codex" => CODEX_ROUTES,
        "opencode" => OPENCODE_ROUTES,
        "pi" => PI_ROUTES,
        _ => &[],
    }
}

/// Whether the harness needs a model of the Agent on `route`. pi needs
/// one in its custom provider. OpenRouter names a model of OpenAI
/// `openai/<model>`, so Codex needs one on the OpenRouter route. Elsewhere
/// a harness keeps its own default model of the provider.
fn needs_model(harness_id: &str, route: EndpointRoute) -> bool {
    match harness_id {
        "pi" => true,
        "codex" => route.provider == Provider::OpenRouter,
        _ => false,
    }
}

/// The route that one session of a harness takes, and the model of the
/// Agent on a route that needs one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRoute {
    harness_id: &'static str,
    route: EndpointRoute,
    model: Option<String>,
}

/// Why a harness has no route.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NoModelRoute {
    #[error("{0:?} does not run in a Computer")]
    NotInComputer(String),
    /// No provider of the harness's routes holds an Org key.
    #[error(
        "{label} in your computer spends this installation's {} key, and the installation has \
         none. Ask the user to have an administrator add one in the Administration Interface.",
        provider_names(providers)
    )]
    NoKey {
        label: &'static str,
        providers: Vec<Provider>,
    },
    /// The Agent's model alias has no candidate of a provider that holds
    /// an Org key on the harness's routes.
    #[error(
        "{label} in your computer uses a model of your model alias {alias:?} from the \
         {} provider, and the alias has no candidate of it. Ask the user to add a candidate \
         of that provider to the alias.",
        provider_names(providers)
    )]
    NoCandidate {
        label: &'static str,
        alias: String,
        providers: Vec<Provider>,
    },
}

impl NoModelRoute {
    /// The code of the tool error.
    pub fn code(&self) -> &'static str {
        match self {
            NoModelRoute::NotInComputer(_) => "unknown_harness",
            NoModelRoute::NoKey { .. } => "no_provider_key",
            NoModelRoute::NoCandidate { .. } => "no_model_candidate",
        }
    }
}

/// The names of `providers` for a person: "OpenAI or OpenRouter".
fn provider_names(providers: &[Provider]) -> String {
    providers
        .iter()
        .map(|provider| provider.name())
        .collect::<Vec<_>>()
        .join(" or ")
}

/// The route of a session of `harness_id`: the first route whose
/// provider is in `keyed`. A route where the harness needs a model counts
/// only when its provider has a candidate in `candidates`, the
/// `provider/model` candidates of the Agent's model alias `alias`, and
/// the session takes the first such candidate.
pub fn choose_route(
    harness_id: &str,
    keyed: &[Provider],
    alias: &str,
    candidates: &[String],
) -> Result<ModelRoute, NoModelRoute> {
    let entry = harness::entry(harness_id)
        .filter(|entry| entry.computer.is_some() && !routes(entry.id).is_empty())
        .ok_or_else(|| NoModelRoute::NotInComputer(harness_id.to_string()))?;
    let keyed_routes: Vec<EndpointRoute> = routes(entry.id)
        .iter()
        .copied()
        .filter(|route| keyed.contains(&route.provider))
        .collect();
    if keyed_routes.is_empty() {
        return Err(NoModelRoute::NoKey {
            label: entry.label,
            providers: providers_of(routes(entry.id)),
        });
    }
    keyed_routes
        .iter()
        .find_map(|route| {
            let model = if needs_model(entry.id, *route) {
                Some(candidate_of(candidates, route.provider)?.to_string())
            } else {
                None
            };
            Some(ModelRoute {
                harness_id: entry.id,
                route: *route,
                model,
            })
        })
        .ok_or_else(|| NoModelRoute::NoCandidate {
            label: entry.label,
            alias: alias.to_string(),
            providers: providers_of(&keyed_routes),
        })
}

/// The providers of `routes`, each once, in order.
fn providers_of(routes: &[EndpointRoute]) -> Vec<Provider> {
    let mut providers = Vec::new();
    for route in routes {
        if !providers.contains(&route.provider) {
            providers.push(route.provider);
        }
    }
    providers
}

/// The model of the first candidate of `provider`, in the `provider/model`
/// form that the Agent's brain reads.
fn candidate_of(candidates: &[String], provider: Provider) -> Option<&str> {
    candidates.iter().find_map(|candidate| {
        let (id, model) = candidate.split_once('/')?;
        (id == provider.id() && !model.is_empty()).then_some(model)
    })
}

/// What points one exec of a harness at the Harness Model Endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSetup {
    /// The variables that the exec gets besides those of a shell command.
    pub env: BTreeMap<String, String>,
    /// The configuration directory of the session, for a harness that
    /// reads its provider from files.
    pub config: Option<ConfigDirectory>,
}

/// The configuration directory of one session, and the files that the
/// daemon writes into it before each start of the harness. The directory
/// stays after the session, so a resume finds the harness's own session
/// files in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDirectory {
    /// The absolute path in the Computer.
    pub path: String,
    pub files: Vec<ConfigFile>,
}

/// One file of a configuration directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFile {
    pub name: &'static str,
    pub contents: String,
}

impl ConfigDirectory {
    /// The files as one tar, the stdin of [`ConfigDirectory::write_command`].
    pub fn tar(&self) -> std::io::Result<Vec<u8>> {
        let mut builder = tar::Builder::new(Vec::new());
        for file in &self.files {
            let mut header = tar::Header::new_gnu();
            header.set_size(file.contents.len() as u64);
            header.set_mode(0o600);
            header.set_cksum();
            builder.append_data(&mut header, file.name, file.contents.as_bytes())?;
        }
        builder.into_inner()
    }

    /// The shell command that makes the directory and writes the files of
    /// the tar on its stdin into it. The uid that runs the command owns
    /// the directory and the files, so the harness can change its own
    /// configuration. An upload of the Engine API gives the files to the
    /// user of the container, which is root.
    pub fn write_command(&self) -> String {
        let path = format!("'{}'", self.path.replace('\'', r"'\''"));
        format!("mkdir -p {path} && tar -x -C {path}")
    }
}

impl ModelRoute {
    /// The provider whose Org key the session spends.
    pub fn provider(&self) -> Provider {
        self.route.provider
    }

    /// The route of the endpoint that the session takes.
    pub fn route(&self) -> EndpointRoute {
        self.route
    }

    /// The model in the harness's configuration, on a route that needs
    /// one.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The configuration of one session: `endpoint` is the base URL of
    /// the Harness Model Endpoint as the Computer reaches it, and `token`
    /// the token of the session.
    pub fn setup(&self, endpoint: &str, session_id: &CodingSessionId, token: &str) -> ModelSetup {
        let base_url = format!("{endpoint}{}", self.route.base_path());
        let mut env = BTreeMap::from([(MODEL_TOKEN_VARIABLE.to_string(), token.to_string())]);
        let directory = |name: &str| format!("{SESSION_CONFIG_ROOT}/{session_id}/{name}");
        let config = match self.harness_id {
            // Claude Code takes the base URL of the Messages API with no
            // `/v1`, and the token as a bearer token. It sends no traffic
            // that the work does not need.
            "claude" => {
                env.insert(
                    "ANTHROPIC_BASE_URL".to_string(),
                    format!("{endpoint}/{}", self.route.provider.id()),
                );
                env.insert("ANTHROPIC_AUTH_TOKEN".to_string(), token.to_string());
                env.insert(
                    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_string(),
                    "1".to_string(),
                );
                None
            }
            "codex" => {
                let path = directory("codex");
                env.insert("CODEX_HOME".to_string(), path.clone());
                Some(ConfigDirectory {
                    path,
                    files: vec![ConfigFile {
                        name: "config.toml",
                        contents: codex_config(&base_url, self.model.as_deref()),
                    }],
                })
            }
            "opencode" => {
                env.insert(
                    "OPENCODE_CONFIG_CONTENT".to_string(),
                    opencode_config(self.route.provider, &base_url),
                );
                None
            }
            "pi" => {
                let path = directory("pi");
                env.insert("PI_CODING_AGENT_DIR".to_string(), path.clone());
                // The Anthropic API of pi takes the base URL with no `/v1`.
                let base_url = match self.route.api {
                    ModelApi::AnthropicMessages => {
                        format!("{endpoint}/{}", self.route.provider.id())
                    }
                    ModelApi::OpenAiResponses | ModelApi::OpenAiChatCompletions => base_url,
                };
                let model = self.model.clone().unwrap_or_default();
                Some(ConfigDirectory {
                    path,
                    files: pi_files(self.route.api, &base_url, &model),
                })
            }
            other => unreachable!("a route of {other}, which runs in no Computer"),
        };
        ModelSetup { env, config }
    }
}

/// The `config.toml` of Codex: the provider `pagis` at `base_url`, which
/// reads its bearer token from `PAGIS_MODEL_TOKEN` and speaks the
/// Responses API over HTTP, and `model` when the route needs one.
fn codex_config(base_url: &str, model: Option<&str>) -> String {
    let mut provider = toml::Table::new();
    provider.insert("name".into(), "Pagis".into());
    provider.insert("base_url".into(), base_url.into());
    provider.insert("env_key".into(), MODEL_TOKEN_VARIABLE.into());
    provider.insert("wire_api".into(), "responses".into());
    provider.insert("supports_websockets".into(), false.into());
    let mut providers = toml::Table::new();
    providers.insert(PROVIDER_ID.into(), provider.into());
    let mut config = toml::Table::new();
    if let Some(model) = model {
        config.insert("model".into(), model.into());
    }
    config.insert("model_provider".into(), PROVIDER_ID.into());
    config.insert("model_providers".into(), providers.into());
    config.to_string()
}

/// The inline configuration of OpenCode: its own provider of `provider`
/// at `base_url`, with the key from `PAGIS_MODEL_TOKEN`, and no other
/// provider.
fn opencode_config(provider: Provider, base_url: &str) -> String {
    json!({
        "$schema": "https://opencode.ai/config.json",
        "enabled_providers": [provider.id()],
        "provider": {
            provider.id(): {
                "options": {
                    "baseURL": base_url,
                    "apiKey": format!("{{env:{MODEL_TOKEN_VARIABLE}}}"),
                },
            },
        },
    })
    .to_string()
}

/// The `models.json` and `settings.json` of pi: the one provider `pagis`
/// with one model, and that model as the default.
fn pi_files(api: ModelApi, base_url: &str, model: &str) -> Vec<ConfigFile> {
    let api = match api {
        ModelApi::AnthropicMessages => "anthropic-messages",
        ModelApi::OpenAiResponses => "openai-responses",
        ModelApi::OpenAiChatCompletions => "openai-completions",
    };
    let models = json!({
        "providers": {
            PROVIDER_ID: {
                "baseUrl": base_url,
                "api": api,
                "apiKey": format!("${MODEL_TOKEN_VARIABLE}"),
                "models": [{"id": model}],
            },
        },
    });
    let settings = json!({
        "defaultProvider": PROVIDER_ID,
        "defaultModel": model,
    });
    vec![
        ConfigFile {
            name: "models.json",
            contents: format!("{models:#}\n"),
        },
        ConfigFile {
            name: "settings.json",
            contents: format!("{settings:#}\n"),
        },
    ]
}

/// Chooses the route of a session in a Computer from the Org's keys and
/// the Agent's model alias, both read at each call.
pub struct ModelRoutes {
    keys: Arc<ProviderKeys>,
    agents: Arc<dyn AgentStore>,
    aliases: Arc<dyn ModelAliasStore>,
}

/// Why no route was chosen.
#[derive(Debug, thiserror::Error)]
pub enum RouteFailure {
    #[error(transparent)]
    NoRoute(#[from] NoModelRoute),
    /// A key or a store did not answer.
    #[error("{0}")]
    Unavailable(String),
}

impl ModelRoutes {
    pub fn new(
        keys: Arc<ProviderKeys>,
        agents: Arc<dyn AgentStore>,
        aliases: Arc<dyn ModelAliasStore>,
    ) -> Self {
        Self {
            keys,
            agents,
            aliases,
        }
    }

    /// The route of a session of `harness_id` that `agent_id` starts or
    /// resumes.
    pub async fn route(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        harness_id: &str,
    ) -> Result<ModelRoute, RouteFailure> {
        let mut keyed = Vec::new();
        for provider in providers_of(routes(harness_id)) {
            let key = self
                .keys
                .resolve(provider)
                .map_err(|error| RouteFailure::Unavailable(error.to_string()))?;
            if key.is_some() {
                keyed.push(provider);
            }
        }
        let needs_candidates = routes(harness_id)
            .iter()
            .any(|route| keyed.contains(&route.provider) && needs_model(harness_id, *route));
        let (alias, candidates) = if needs_candidates {
            self.candidates(workspace_id, agent_id).await?
        } else {
            (String::new(), Vec::new())
        };
        Ok(choose_route(harness_id, &keyed, &alias, &candidates)?)
    }

    /// The model alias of the Agent and its candidates. An alias that the
    /// Workspace does not hold has none.
    async fn candidates(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<(String, Vec<String>), RouteFailure> {
        let unavailable =
            |error: pagis_core::StoreError| RouteFailure::Unavailable(error.to_string());
        let agent = self
            .agents
            .get(workspace_id, agent_id)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| RouteFailure::Unavailable(format!("the Agent {agent_id} is gone")))?;
        let candidates = self
            .aliases
            .get_by_alias(workspace_id, &agent.model_alias)
            .await
            .map_err(unavailable)?
            .map(|alias| alias.candidates)
            .unwrap_or_default();
        Ok((agent.model_alias, candidates))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENDPOINT: &str = "http://host.docker.internal:4404";
    const TOKEN: &str = "session-token-1234";

    fn candidates() -> Vec<String> {
        [
            "openai/gpt-5.5",
            "openrouter/openai/gpt-5.5",
            "anthropic/claude-sonnet-4-5",
            "openrouter/anthropic/claude-sonnet-4.5",
            "anthropic/claude-opus-4-1",
        ]
        .map(String::from)
        .to_vec()
    }

    fn route(harness_id: &str, keyed: &[Provider]) -> Result<ModelRoute, NoModelRoute> {
        choose_route(harness_id, keyed, "default", &candidates())
    }

    fn setup(harness_id: &str, keyed: &[Provider]) -> (ModelRoute, ModelSetup) {
        let route = route(harness_id, keyed).expect("a route");
        let session_id = CodingSessionId::from("cs_1".to_string());
        let setup = route.setup(ENDPOINT, &session_id, TOKEN);
        (route, setup)
    }

    fn file<'a>(setup: &'a ModelSetup, name: &str) -> &'a str {
        let config = setup.config.as_ref().expect("a configuration directory");
        &config
            .files
            .iter()
            .find(|file| file.name == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .contents
    }

    /// Each exec holds the token in `PAGIS_MODEL_TOKEN`, and no file
    /// holds it.
    fn assert_token_only_in_the_environment(setup: &ModelSetup) {
        assert_eq!(
            setup.env.get(MODEL_TOKEN_VARIABLE).map(String::as_str),
            Some(TOKEN)
        );
        for file in setup.config.iter().flat_map(|config| &config.files) {
            assert!(
                !file.contents.contains(TOKEN),
                "{}: {}",
                file.name,
                file.contents
            );
        }
    }

    const ALL: &[Provider] = &[Provider::Anthropic, Provider::OpenAi, Provider::OpenRouter];

    #[test]
    fn each_harness_takes_the_first_route_whose_provider_holds_a_key() {
        let cases: &[(&str, &[Provider], Provider)] = &[
            ("claude", ALL, Provider::Anthropic),
            ("codex", ALL, Provider::OpenAi),
            ("codex", &[Provider::OpenRouter], Provider::OpenRouter),
            (
                "codex",
                &[Provider::Anthropic, Provider::OpenRouter],
                Provider::OpenRouter,
            ),
            ("opencode", ALL, Provider::Anthropic),
            (
                "opencode",
                &[Provider::OpenAi, Provider::OpenRouter],
                Provider::OpenAi,
            ),
            ("opencode", &[Provider::OpenRouter], Provider::OpenRouter),
            ("pi", ALL, Provider::Anthropic),
            (
                "pi",
                &[Provider::OpenAi, Provider::OpenRouter],
                Provider::OpenRouter,
            ),
            ("pi", &[Provider::OpenAi], Provider::OpenAi),
        ];
        for (harness_id, keyed, provider) in cases {
            let route = route(harness_id, keyed).expect("a route");
            assert_eq!(route.provider(), *provider, "{harness_id} {keyed:?}");
        }
    }

    #[test]
    fn with_no_key_on_its_routes_a_harness_has_no_route_and_the_cause_names_the_providers() {
        let refusal = route("codex", &[Provider::Anthropic]).unwrap_err();
        assert_eq!(refusal.code(), "no_provider_key");
        assert!(
            refusal.to_string().contains("OpenAI or OpenRouter key"),
            "{refusal}"
        );

        let refusal = route("claude", &[Provider::OpenAi]).unwrap_err();
        assert!(refusal.to_string().contains("Anthropic key"), "{refusal}");

        for (harness_id, providers) in [
            ("opencode", "Anthropic or OpenAI or OpenRouter key"),
            ("pi", "Anthropic or OpenRouter or OpenAI key"),
        ] {
            let refusal = route(harness_id, &[]).unwrap_err();
            assert_eq!(refusal.code(), "no_provider_key", "{harness_id}");
            assert!(refusal.to_string().contains(providers), "{refusal}");
        }
    }

    #[test]
    fn a_harness_that_runs_in_no_computer_has_no_route() {
        assert_eq!(route("gemini", ALL).unwrap_err().code(), "unknown_harness");
    }

    #[test]
    fn pi_takes_the_first_candidate_of_the_agents_alias_for_its_provider() {
        assert_eq!(route("pi", ALL).unwrap().model(), Some("claude-sonnet-4-5"));
        assert_eq!(
            route("pi", &[Provider::OpenRouter]).unwrap().model(),
            Some("openai/gpt-5.5")
        );
        assert_eq!(
            route("pi", &[Provider::OpenAi]).unwrap().model(),
            Some("gpt-5.5")
        );
        assert_eq!(route("codex", ALL).unwrap().model(), None);
    }

    /// OpenRouter names a model of OpenAI `openai/<model>`, which is not
    /// the default model of Codex. So on the OpenRouter route Codex takes
    /// the Agent's OpenRouter candidate, and on the OpenAI route it keeps
    /// its own default model.
    #[test]
    fn codex_takes_the_agents_openrouter_candidate_only_on_the_openrouter_route() {
        assert_eq!(route("codex", ALL).unwrap().model(), None);
        assert_eq!(route("codex", &[Provider::OpenAi]).unwrap().model(), None);
        let route = route("codex", &[Provider::OpenRouter]).unwrap();
        assert_eq!(route.provider(), Provider::OpenRouter);
        assert_eq!(route.model(), Some("openai/gpt-5.5"));
    }

    #[test]
    fn codex_skips_the_openrouter_route_when_the_alias_has_no_openrouter_candidate() {
        let only_openai = ["openai/gpt-5.5".to_string()];

        let route = choose_route(
            "codex",
            &[Provider::OpenAi, Provider::OpenRouter],
            "default",
            &only_openai,
        )
        .unwrap();
        assert_eq!(route.provider(), Provider::OpenAi);
        assert_eq!(route.model(), None);

        let refusal =
            choose_route("codex", &[Provider::OpenRouter], "default", &only_openai).unwrap_err();
        assert_eq!(refusal.code(), "no_model_candidate");
        assert!(refusal.to_string().contains("\"default\""), "{refusal}");
        assert!(
            refusal.to_string().contains("OpenRouter provider"),
            "{refusal}"
        );
    }

    #[test]
    fn pi_skips_a_keyed_provider_that_the_alias_has_no_candidate_of() {
        let only_openai = ["openai/gpt-5.5".to_string()];
        let route = choose_route(
            "pi",
            &[Provider::Anthropic, Provider::OpenAi],
            "default",
            &only_openai,
        )
        .unwrap();
        assert_eq!(route.provider(), Provider::OpenAi);
    }

    #[test]
    fn pi_with_no_candidate_of_a_keyed_provider_has_no_route() {
        let only_openai = ["openai/gpt-5.5".to_string()];
        let refusal =
            choose_route("pi", &[Provider::Anthropic], "default", &only_openai).unwrap_err();
        assert_eq!(refusal.code(), "no_model_candidate");
        assert!(refusal.to_string().contains("\"default\""), "{refusal}");
        assert!(
            refusal.to_string().contains("Anthropic provider"),
            "{refusal}"
        );
    }

    #[test]
    fn claude_code_reads_the_endpoint_and_the_token_from_its_environment() {
        let (_, setup) = setup("claude", ALL);

        assert_eq!(setup.config, None);
        assert_eq!(
            setup.env,
            BTreeMap::from(
                [
                    ("ANTHROPIC_AUTH_TOKEN", TOKEN),
                    (
                        "ANTHROPIC_BASE_URL",
                        "http://host.docker.internal:4404/anthropic"
                    ),
                    ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
                    (MODEL_TOKEN_VARIABLE, TOKEN),
                ]
                .map(|(name, value)| (name.to_string(), value.to_string()))
            )
        );
    }

    #[test]
    fn codex_reads_the_pagis_provider_from_config_toml_in_its_own_codex_home() {
        for (keyed, base_url, model) in [
            (ALL, "http://host.docker.internal:4404/openai/v1", None),
            (
                &[Provider::OpenRouter][..],
                "http://host.docker.internal:4404/openrouter/v1",
                Some("openai/gpt-5.5"),
            ),
        ] {
            let (_, setup) = setup("codex", keyed);

            assert_token_only_in_the_environment(&setup);
            let directory = "/data/agent/.pagis/coding/cs_1/codex";
            assert_eq!(setup.config.as_ref().unwrap().path, directory);
            assert_eq!(setup.env["CODEX_HOME"], directory);
            assert_eq!(setup.env.len(), 2, "{:?}", setup.env);
            let config: toml::Table = toml::from_str(file(&setup, "config.toml")).unwrap();
            assert_eq!(config["model_provider"].as_str(), Some("pagis"));
            assert_eq!(
                config.get("model").and_then(|model| model.as_str()),
                model,
                "{keyed:?}"
            );
            let provider = config["model_providers"]["pagis"].as_table().unwrap();
            assert_eq!(provider["name"].as_str(), Some("Pagis"));
            assert_eq!(provider["base_url"].as_str(), Some(base_url));
            assert_eq!(provider["env_key"].as_str(), Some(MODEL_TOKEN_VARIABLE));
            assert_eq!(provider["wire_api"].as_str(), Some("responses"));
            assert_eq!(provider["supports_websockets"].as_bool(), Some(false));
        }
    }

    #[test]
    fn opencode_reads_its_own_provider_alone_from_its_inline_configuration() {
        for (keyed, provider, base_url) in [
            (
                ALL,
                "anthropic",
                "http://host.docker.internal:4404/anthropic/v1",
            ),
            (
                &[Provider::OpenAi, Provider::OpenRouter][..],
                "openai",
                "http://host.docker.internal:4404/openai/v1",
            ),
            (
                &[Provider::OpenRouter][..],
                "openrouter",
                "http://host.docker.internal:4404/openrouter/v1",
            ),
        ] {
            let (_, setup) = setup("opencode", keyed);

            assert_token_only_in_the_environment(&setup);
            assert_eq!(setup.config, None);
            assert_eq!(setup.env.len(), 2, "{:?}", setup.env);
            let content = &setup.env["OPENCODE_CONFIG_CONTENT"];
            assert!(!content.contains(TOKEN));
            let config: serde_json::Value = serde_json::from_str(content).unwrap();
            assert_eq!(config["enabled_providers"], json!([provider]));
            assert_eq!(
                config["provider"],
                json!({provider: {"options": {
                    "baseURL": base_url,
                    "apiKey": "{env:PAGIS_MODEL_TOKEN}",
                }}})
            );
        }
    }

    #[test]
    fn pi_reads_one_pagis_provider_and_its_default_model_from_its_own_agent_directory() {
        for (keyed, base_url, api, model) in [
            (
                ALL,
                "http://host.docker.internal:4404/anthropic",
                "anthropic-messages",
                "claude-sonnet-4-5",
            ),
            (
                &[Provider::OpenRouter][..],
                "http://host.docker.internal:4404/openrouter/v1",
                "openai-completions",
                "openai/gpt-5.5",
            ),
            (
                &[Provider::OpenAi][..],
                "http://host.docker.internal:4404/openai/v1",
                "openai-completions",
                "gpt-5.5",
            ),
        ] {
            let (_, setup) = setup("pi", keyed);

            assert_token_only_in_the_environment(&setup);
            let directory = "/data/agent/.pagis/coding/cs_1/pi";
            assert_eq!(setup.config.as_ref().unwrap().path, directory);
            assert_eq!(setup.env["PI_CODING_AGENT_DIR"], directory);
            assert_eq!(setup.env.len(), 2, "{:?}", setup.env);
            let models: serde_json::Value =
                serde_json::from_str(file(&setup, "models.json")).unwrap();
            assert_eq!(
                models,
                json!({"providers": {"pagis": {
                    "baseUrl": base_url,
                    "api": api,
                    "apiKey": "$PAGIS_MODEL_TOKEN",
                    "models": [{"id": model}],
                }}})
            );
            let settings: serde_json::Value =
                serde_json::from_str(file(&setup, "settings.json")).unwrap();
            assert_eq!(
                settings,
                json!({"defaultProvider": "pagis", "defaultModel": model})
            );
        }
    }

    #[test]
    fn the_configuration_directory_is_one_tar_of_its_files() {
        let (_, setup) = setup("pi", ALL);
        let config = setup.config.unwrap();

        let tar = config.tar().unwrap();

        let mut archive = tar::Archive::new(tar.as_slice());
        let mut names = Vec::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().to_string_lossy().into_owned();
            let mut contents = String::new();
            std::io::Read::read_to_string(&mut entry, &mut contents).unwrap();
            let file = config.files.iter().find(|file| file.name == name).unwrap();
            assert_eq!(contents, file.contents);
            names.push(name);
        }
        assert_eq!(names, ["models.json", "settings.json"]);
        assert_eq!(
            config.write_command(),
            "mkdir -p '/data/agent/.pagis/coding/cs_1/pi' && tar -x -C '/data/agent/.pagis/coding/cs_1/pi'"
        );
    }
}
