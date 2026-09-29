//! What a package declares against what one process or one request
//! carries (ADR-0017).

use std::collections::BTreeMap;

use pagis_plugin::{McpServer, PluginPaths};
use pagis_plugins::{Bound, SubstitutionError, endpoint_of, spawn_of};

fn paths() -> PluginPaths {
    PluginPaths {
        repository: "/data/plugins/w/p.git".into(),
        root: "/data/plugins/w/p".into(),
        data: "/data/plugins/w/p.data".into(),
    }
}

fn bound(field: &str, value: &str) -> Bound {
    Bound::new(BTreeMap::from([(field.to_string(), value.to_string())]))
}

fn stdio(env: BTreeMap<String, String>, cwd: Option<&str>) -> McpServer {
    McpServer::Stdio {
        command: "./server".to_string(),
        args: vec!["--root".to_string(), "${PLUGIN_ROOT}".to_string()],
        env,
        cwd: cwd.map(str::to_string),
    }
}

#[test]
fn a_stdio_server_starts_with_the_declared_environment_and_nothing_inherited() {
    let server = stdio(
        BTreeMap::from([
            ("API_KEY".to_string(), "${config.api_key}".to_string()),
            ("STATE".to_string(), "${PLUGIN_DATA}/state".to_string()),
        ]),
        None,
    );

    let spawn = spawn_of(&server, &paths(), &bound("api_key", "the-key")).expect("the command");

    assert_eq!(spawn.program, "/data/plugins/w/p/server");
    assert_eq!(spawn.args, ["--root", "/data/plugins/w/p"]);
    assert_eq!(spawn.cwd, paths().root);
    assert_eq!(spawn.env["API_KEY"], "the-key");
    assert_eq!(spawn.env["STATE"], "/data/plugins/w/p.data/state");
    assert_eq!(spawn.env["PLUGIN_ROOT"], "/data/plugins/w/p");
    assert_eq!(spawn.env["PLUGIN_DATA"], "/data/plugins/w/p.data");
    // The base environment is the container's, not the daemon's.
    assert_eq!(spawn.env["HOME"], "/data/agent");
    let names: Vec<&str> = spawn.env.keys().map(String::as_str).collect();
    for name in &names {
        assert!(
            pagis_plugins::CONTAINER_ENV
                .iter()
                .any(|(inherited, _)| inherited == name)
                || matches!(*name, "API_KEY" | "STATE" | "PLUGIN_ROOT" | "PLUGIN_DATA"),
            "the environment holds {name:?}"
        );
    }
}

#[test]
fn a_field_with_no_bound_value_stops_the_start() {
    let server = stdio(
        BTreeMap::from([("API_KEY".to_string(), "${config.api_key}".to_string())]),
        None,
    );

    let refused = spawn_of(&server, &paths(), &Bound::default());

    assert_eq!(
        refused.err(),
        Some(SubstitutionError::Unbound("api_key".to_string()))
    );
}

#[test]
fn the_working_directory_is_the_root_or_the_data_directory() {
    let inside = stdio(BTreeMap::new(), Some("./bin"));
    let data = stdio(BTreeMap::new(), Some("${PLUGIN_DATA}"));
    let under_data = stdio(BTreeMap::new(), Some("${PLUGIN_DATA}/work"));

    assert_eq!(
        spawn_of(&inside, &paths(), &Bound::default())
            .expect("the command")
            .cwd,
        paths().root.join("bin")
    );
    assert_eq!(
        spawn_of(&data, &paths(), &Bound::default())
            .expect("the command")
            .cwd,
        paths().data
    );
    assert_eq!(
        spawn_of(&under_data, &paths(), &Bound::default())
            .expect("the command")
            .cwd,
        paths().data.join("work")
    );
}

#[test]
fn a_header_carries_the_bound_secret_and_the_address_carries_a_value() {
    let server = McpServer::StreamableHttp {
        url: "https://api.example.com/${config.tenant}/mcp".to_string(),
        headers: BTreeMap::from([(
            "Authorization".to_string(),
            "Bearer ${config.tenant}".to_string(),
        )]),
    };

    let endpoint = endpoint_of(&server, &paths(), &bound("tenant", "acme")).expect("the endpoint");

    assert_eq!(endpoint.url, "https://api.example.com/acme/mcp");
    assert_eq!(endpoint.headers["Authorization"], "Bearer acme");
}

#[test]
fn plain_http_to_a_remote_host_is_refused_at_the_start() {
    let remote = McpServer::StreamableHttp {
        url: "http://${config.host}/mcp".to_string(),
        headers: BTreeMap::new(),
    };

    let refused = endpoint_of(&remote, &paths(), &bound("host", "api.example.com"));

    assert_eq!(
        refused.err(),
        Some(SubstitutionError::Address(
            "http://api.example.com/mcp".to_string()
        ))
    );
    // The same address on the loopback interface is allowed.
    let local = McpServer::StreamableHttp {
        url: "http://${config.host}/mcp".to_string(),
        headers: BTreeMap::new(),
    };
    assert!(endpoint_of(&local, &paths(), &bound("host", "127.0.0.1:8931")).is_ok());
}

/// The documents state that an HTTP or SSE server reaches every HTTPS
/// host that the daemon reaches (ADR-0017). The egress policy of a
/// Computer refuses these addresses; the address rule of the daemon does
/// not. A change to this rule changes the documents too.
#[test]
fn https_to_a_private_or_link_local_address_is_allowed_at_the_start() {
    let hosts = [
        "10.0.5.7",
        "172.16.0.10:8443",
        "192.168.1.40",
        "100.64.0.1",
        "169.254.169.254",
        "[fd00::1]",
    ];
    let servers = [
        McpServer::StreamableHttp {
            url: "https://${config.host}/mcp".to_string(),
            headers: BTreeMap::new(),
        },
        McpServer::Sse {
            url: "https://${config.host}/mcp".to_string(),
            headers: BTreeMap::new(),
        },
    ];

    for server in &servers {
        for host in hosts {
            match endpoint_of(server, &paths(), &bound("host", host)) {
                Ok(endpoint) => assert_eq!(endpoint.url, format!("https://{host}/mcp")),
                Err(error) => panic!("https to {host} was refused: {error}"),
            }
        }
    }
}
