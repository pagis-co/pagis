//! What a package declares, turned into what one process or one
//! request carries (ADR-0017).
//!
//! Three kinds of text carry a placeholder. `${config.<field>}` is a
//! bound value: a secret from the vault, the token of a bound
//! Connection, or a plain value the user typed. `${PLUGIN_ROOT}` and
//! `${PLUGIN_DATA}` are the plugin's own two directories. Install
//! validation already refused a credential outside an `env` value or a
//! header (ADR-0017), so expansion here needs no second rule about
//! where a value may land.
//!
//! The environment of a stdio server is built, never inherited: it
//! holds the `PATH`, the `HOME` and the `TMPDIR` of the container it
//! runs in, the plugin's two directories, and the declared `env`
//! entries. Nothing of the daemon's environment reaches a plugin,
//! because the daemon's environment is not the one the process lives
//! in: the server runs inside the tenant's Plugin Computer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pagis_plugin::{McpServer, PluginPaths};

/// The base environment of one server process, which is the
/// environment of the container it runs in (ADR-0017). The
/// declared `env` entries are added to it, and nothing else is.
pub const CONTAINER_ENV: [(&str, &str); 3] = [
    (
        "PATH",
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    ),
    ("HOME", "/data/agent"),
    ("TMPDIR", "/tmp"),
];

/// The plugin's own root, as the specification names it.
pub const PLUGIN_ROOT: &str = "PLUGIN_ROOT";
/// The plugin's writable directory, which survives an update.
pub const PLUGIN_DATA: &str = "PLUGIN_DATA";

/// Why one server could not be prepared.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubstitutionError {
    /// A declared field has no Binding, or its Binding has no value
    /// now: a Connection whose token the daemon cannot read.
    #[error("the field {0:?} has no bound value")]
    Unbound(String),
    /// The address a bound value built is not one a request may go
    /// to: plain HTTP to a remote host, or not a URL at all.
    #[error("the address {0:?} is not https and not the loopback interface")]
    Address(String),
    #[error("the header name {0:?} is not a header name")]
    BadHeaderName(String),
    #[error("the header {0:?} carries a value a request cannot hold")]
    BadHeaderValue(String),
}

/// The bound value of every config field, by field name. A secret and
/// a Connection token are in here as plain text, so the type carries
/// no `Debug`: it must not reach a log line.
#[derive(Clone, Default)]
pub struct Bound(BTreeMap<String, String>);

impl Bound {
    pub fn new(values: BTreeMap<String, String>) -> Self {
        Self(values)
    }

    pub fn get(&self, field: &str) -> Option<&str> {
        self.0.get(field).map(String::as_str)
    }

    /// Expand every placeholder of one string. An unbound field is a
    /// refusal, not an empty value: a server that starts with half a
    /// credential fails in a way nobody can read.
    pub fn expand(&self, text: &str, paths: &PluginPaths) -> Result<String, SubstitutionError> {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("${") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find('}') else {
                out.push_str(&rest[start..]);
                return Ok(out);
            };
            let name = &after[..end];
            match name {
                PLUGIN_ROOT => out.push_str(&paths.root.display().to_string()),
                PLUGIN_DATA => out.push_str(&paths.data.display().to_string()),
                _ => match name.strip_prefix("config.") {
                    Some(field) => match self.get(field) {
                        Some(value) => out.push_str(value),
                        None => return Err(SubstitutionError::Unbound(field.to_string())),
                    },
                    // A placeholder Pagis does not define stays as it
                    // is: it belongs to the plugin, not to the daemon.
                    None => {
                        out.push_str("${");
                        out.push_str(name);
                        out.push('}');
                    }
                },
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }
}

/// One stdio server, ready to spawn.
pub struct Spawn {
    pub program: String,
    pub args: Vec<String>,
    /// The whole environment of the child: nothing is inherited that
    /// is not in here.
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
}

/// One HTTP server, ready to open.
pub struct Endpoint {
    pub url: String,
    pub headers: BTreeMap<String, String>,
}

/// The command of one stdio server, with its environment built from
/// nothing (ADR-0017).
pub fn spawn_of(
    server: &McpServer,
    paths: &PluginPaths,
    bound: &Bound,
) -> Result<Spawn, SubstitutionError> {
    let McpServer::Stdio {
        command,
        args,
        env,
        cwd,
    } = server
    else {
        return Ok(Spawn {
            program: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: paths.root.clone(),
        });
    };
    let program = match command.strip_prefix("./") {
        Some(relative) => paths.root.join(relative).display().to_string(),
        None => command.clone(),
    };
    let mut expanded_args = Vec::with_capacity(args.len());
    for argument in args {
        expanded_args.push(bound.expand(argument, paths)?);
    }
    let mut environment: BTreeMap<String, String> = CONTAINER_ENV
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    environment.insert(PLUGIN_ROOT.to_string(), paths.root.display().to_string());
    environment.insert(PLUGIN_DATA.to_string(), paths.data.display().to_string());
    for (name, value) in env {
        environment.insert(name.clone(), bound.expand(value, paths)?);
    }
    Ok(Spawn {
        program,
        args: expanded_args,
        env: environment,
        cwd: working_directory(cwd.as_deref(), paths),
    })
}

/// The address and headers of one HTTP server. A `sse` server takes
/// the same path: the specification deprecates the transport, and
/// `rmcp` speaks to it through the same client.
pub fn endpoint_of(
    server: &McpServer,
    paths: &PluginPaths,
    bound: &Bound,
) -> Result<Endpoint, SubstitutionError> {
    let (url, headers) = match server {
        McpServer::StreamableHttp { url, headers } | McpServer::Sse { url, headers } => {
            (url, headers)
        }
        McpServer::Stdio { .. } => {
            return Ok(Endpoint {
                url: String::new(),
                headers: BTreeMap::new(),
            });
        }
    };
    let mut expanded = BTreeMap::new();
    for (name, value) in headers {
        expanded.insert(name.clone(), bound.expand(value, paths)?);
    }
    let url = bound.expand(url, paths)?;
    check_address(&url)?;
    Ok(Endpoint {
        url,
        headers: expanded,
    })
}

/// An HTTP server is `https`, except on the loopback interface
/// (ADR-0017). Install validation judged the address the package
/// wrote; this judges the one a bound value built, which is the one a
/// request goes to.
fn check_address(url: &str) -> Result<(), SubstitutionError> {
    let parsed = url::Url::parse(url).map_err(|_| SubstitutionError::Address(url.to_string()))?;
    let loopback = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match parsed.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => Err(SubstitutionError::Address(url.to_string())),
    }
}

/// Where a stdio server starts. A `cwd` the package declares is the
/// plugin root, the plugin data directory, or a path under one of
/// them; install validation refused everything else, so an
/// unrecognised value falls back to the root.
fn working_directory(cwd: Option<&str>, paths: &PluginPaths) -> PathBuf {
    let Some(cwd) = cwd else {
        return paths.root.clone();
    };
    let (base, relative) = if let Some(rest) = cwd.strip_prefix("${PLUGIN_DATA}") {
        (&paths.data, rest)
    } else if let Some(rest) = cwd.strip_prefix("${PLUGIN_ROOT}") {
        (&paths.root, rest)
    } else if let Some(rest) = cwd.strip_prefix("./") {
        (&paths.root, rest)
    } else {
        (&paths.root, "")
    };
    let relative = relative.trim_start_matches('/');
    if relative.is_empty() {
        base.clone()
    } else {
        base.join(Path::new(relative))
    }
}
