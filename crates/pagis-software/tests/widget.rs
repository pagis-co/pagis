//! The Widget half of a Software Package (ADR-0016): what a
//! manifest may declare, the Content-Security-Policy the daemon
//! serves the page under, and the split of one tool result.

use std::path::{Path, PathBuf};

use pagis_software::widget::{
    WidgetCsp, is_csp_origin, parse_widget_uri, split_result, widget_uri,
};
use pagis_software::{Manifest, validate};

const TOOL_SCHEMA: &str = r#"{"type": "object", "properties": {"city": {"type": "string"}}}"#;
const WIDGET_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["high"],
  "properties": {"high": {"type": "number"}}
}"#;

/// A working copy with one tool that renders one Widget.
fn working_copy(manifest: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("working copy");
    write(root.path(), "pagis-software.toml", manifest);
    write(root.path(), "schemas/forecast.json", TOOL_SCHEMA);
    write(root.path(), "schemas/chart.json", WIDGET_SCHEMA);
    write(root.path(), "widgets/chart.html", "<!doctype html><title>c");
    write(root.path(), "bin/forecast.py", "#!/usr/bin/env python3\n");
    make_executable(&root.path().join("bin/forecast.py"));
    root
}

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("create dirs");
    std::fs::write(path, content).expect("write");
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

/// A manifest whose tool and Widget both take extra lines.
fn manifest_with(tool: &str, widget: &str) -> String {
    format!(
        r#"
[package]
name = "weather"
description = "Weather for a city"

[[tool]]
name = "forecast"
description = "Tomorrow's forecast"
entry = "bin/forecast.py"
schema = "schemas/forecast.json"
widget = "chart"
{tool}

[[widget]]
name = "chart"
html = "widgets/chart.html"
schema = "schemas/chart.json"
{widget}
"#
    )
}

fn problems(root: &Path) -> Vec<String> {
    validate(root).expect_err("validation failed").0
}

// ---------------------------------------------------------------- manifest

#[test]
fn a_widget_package_passes_and_carries_its_widget_schema() {
    let root = working_copy(&manifest_with("", ""));

    let version = validate(root.path()).expect("the package is sound");

    assert_eq!(version.manifest.widgets.len(), 1);
    assert_eq!(version.widget_schemas["chart"]["required"][0], "high");
    let tool = version.manifest.tool("forecast").expect("the tool");
    assert_eq!(tool.widget.as_deref(), Some("chart"));
    // The default is the model and the widget, as MCP Apps has it.
    assert!(tool.visible_to_model());
    assert!(tool.visible_to_app());
    assert!(!tool.awaits_input);
}

#[test]
fn a_tool_hides_from_the_model_with_app_visibility() {
    let root = working_copy(&manifest_with(r#"visibility = ["app"]"#, ""));

    let version = validate(root.path()).expect("the package is sound");

    let tool = version.manifest.tool("forecast").expect("the tool");
    assert!(!tool.visible_to_model());
    assert!(tool.visible_to_app());
}

#[test]
fn a_tool_that_renders_an_undeclared_widget_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    write(
        root.path(),
        "pagis-software.toml",
        &manifest_with("", "").replace(r#"widget = "chart""#, r#"widget = "table""#),
    );

    let problems = problems(root.path());

    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("which the package does not declare")),
        "{problems:?}"
    );
}

#[test]
fn a_widget_whose_page_is_missing_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    std::fs::remove_file(root.path().join("widgets/chart.html")).expect("remove");

    let problems = problems(root.path());

    assert!(
        problems
            .iter()
            .any(|problem| problem.starts_with("widget \"chart\" html")),
        "{problems:?}"
    );
}

#[test]
fn a_widget_page_over_the_cap_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    let big = "x".repeat(pagis_software::MAX_WIDGET_HTML_BYTES as usize + 1);
    write(root.path(), "widgets/chart.html", &big);

    let problems = problems(root.path());

    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("the limit is 1048576")),
        "{problems:?}"
    );
}

#[test]
fn a_widget_page_outside_the_root_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    write(
        root.path(),
        "pagis-software.toml",
        &manifest_with("", "").replace(
            r#"html = "widgets/chart.html""#,
            r#"html = "../escape.html""#,
        ),
    );

    let problems = problems(root.path());

    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("leaves the package root")),
        "{problems:?}"
    );
}

#[test]
fn a_declared_domain_that_is_not_an_https_origin_is_refused() {
    let root = working_copy(&manifest_with("", r#"csp = { connect = ["example.com"] }"#));

    let problems = problems(root.path());

    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("which is not an https origin")),
        "{problems:?}"
    );
}

#[test]
fn awaits_input_without_a_widget_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    write(
        root.path(),
        "pagis-software.toml",
        &manifest_with("awaits_input = true", "").replace("widget = \"chart\"\n", ""),
    );

    let problems = problems(root.path());

    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("only a widget can ask")),
        "{problems:?}"
    );
}

#[test]
fn an_empty_visibility_is_refused() {
    let root = working_copy(&manifest_with("visibility = []", ""));

    let problems = problems(root.path());

    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("visibility is empty")),
        "{problems:?}"
    );
}

#[test]
fn a_manifest_without_a_widget_parses() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "weather"
description = "Weather"

[[tool]]
name = "forecast"
description = "A forecast"
entry = "bin/forecast.py"
schema = "schemas/forecast.json"
"#,
    )
    .expect("the manifest parses");

    assert!(manifest.widgets.is_empty());
    assert!(
        manifest
            .tool("forecast")
            .expect("the tool")
            .visible_to_model()
    );
}

// --------------------------------------------------------------------- uri

#[test]
fn a_widget_uri_is_derived_from_the_package_and_the_name() {
    assert_eq!(widget_uri("weather", "chart"), "ui://weather/chart");
    assert_eq!(
        parse_widget_uri("ui://weather/chart"),
        Some(("weather", "chart"))
    );
    assert_eq!(parse_widget_uri("https://weather/chart"), None);
    assert_eq!(parse_widget_uri("ui://weather"), None);
    assert_eq!(parse_widget_uri("ui://weather/chart/deep"), None);
    assert_eq!(parse_widget_uri("ui:///chart"), None);
}

#[test]
fn only_an_https_origin_can_join_a_policy() {
    for good in [
        "https://example.com",
        "https://api.example.com:8443",
        "https://*.example.com",
    ] {
        assert!(is_csp_origin(good), "{good} is an origin");
    }
    for bad in [
        "example.com",
        "http://example.com",
        "https://example.com/path",
        "https://localhost",
        "https://example.com:port",
        "https://user@example.com",
        "https://*",
        "'unsafe-inline'",
        "*",
    ] {
        assert!(!is_csp_origin(bad), "{bad} is not an origin");
    }
}

// --------------------------------------------------------------------- csp

#[test]
fn a_widget_that_declares_nothing_has_no_network() {
    let policy = WidgetCsp::default().header();

    assert_eq!(
        policy,
        "default-src 'none'; \
         script-src 'self' 'unsafe-inline'; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; \
         font-src 'self' data:; \
         media-src 'self' data:; \
         connect-src 'none'; \
         frame-src 'none'; \
         base-uri 'none'; \
         form-action 'none'"
    );
}

#[test]
fn a_declared_connect_domain_opens_connect_src_alone() {
    let policy = WidgetCsp {
        connect: vec!["https://api.example.com".to_string()],
        resource: Vec::new(),
    }
    .header();

    assert!(policy.contains("connect-src https://api.example.com;"));
    assert!(policy.contains("img-src 'self' data:;"));
    assert!(policy.starts_with("default-src 'none';"));
}

#[test]
fn a_declared_resource_domain_opens_every_resource_source() {
    let policy = WidgetCsp {
        connect: Vec::new(),
        resource: vec!["https://cdn.example.com".to_string()],
    }
    .header();

    for source in [
        "script-src",
        "style-src",
        "img-src",
        "font-src",
        "media-src",
    ] {
        assert!(
            policy.contains(&format!("{source} ")) && policy.contains("https://cdn.example.com"),
            "{source} takes the declared domain"
        );
    }
    // The wall stays shut: a resource domain is not a connect domain.
    assert!(policy.contains("connect-src 'none';"));
}

// ------------------------------------------------------------------ result

fn schema() -> serde_json::Value {
    serde_json::from_str(WIDGET_SCHEMA).expect("the schema parses")
}

#[test]
fn a_result_splits_into_the_widget_half_and_the_model_half() {
    let result = serde_json::json!({
        "content": "Tomorrow reaches 21 degrees.",
        "structuredContent": {"high": 21},
    });

    let split = split_result(&result, &schema()).expect("the result splits");

    assert_eq!(split.content, "Tomorrow reaches 21 degrees.");
    assert_eq!(split.structured_content, serde_json::json!({"high": 21}));
}

#[test]
fn a_data_half_that_fails_the_schema_is_refused() {
    let result = serde_json::json!({
        "content": "Tomorrow is warm.",
        "structuredContent": {"high": "warm"},
    });

    let problem = split_result(&result, &schema()).expect_err("the schema refuses it");

    assert!(
        problem.contains("does not match the widget schema"),
        "{problem}"
    );
}

#[test]
fn a_result_without_a_projection_is_refused() {
    let result = serde_json::json!({"structuredContent": {"high": 21}});

    let problem = split_result(&result, &schema()).expect_err("there is no projection");

    assert!(problem.contains("\"content\""), "{problem}");
}

#[test]
fn an_empty_projection_is_refused() {
    let result = serde_json::json!({"content": "  ", "structuredContent": {"high": 21}});

    let problem = split_result(&result, &schema()).expect_err("the projection is empty");

    assert!(problem.contains("must not be empty"), "{problem}");
}

#[test]
fn a_result_without_data_is_refused() {
    let result = serde_json::json!({"content": "Tomorrow is warm."});

    let problem = split_result(&result, &schema()).expect_err("there is no data");

    assert!(problem.contains("\"structuredContent\""), "{problem}");
}

#[test]
fn data_that_is_not_an_object_is_refused() {
    let result = serde_json::json!({"content": "a", "structuredContent": [1, 2]});

    let problem = split_result(&result, &schema()).expect_err("the data is a list");

    assert!(problem.contains("must be a JSON object"), "{problem}");
}

#[test]
fn data_over_the_cap_is_refused() {
    let big = "x".repeat(pagis_software::MAX_STRUCTURED_CONTENT_BYTES);
    let result = serde_json::json!({
        "content": "a",
        "structuredContent": {"high": 1, "note": big},
    });

    let problem = split_result(&result, &schema()).expect_err("the data is too big");

    assert!(problem.contains("the limit is 262144"), "{problem}");
}

// ----------------------------------------------------------- the scaffold

/// The scaffold package the `pagis:widgets` Skill ships. An
/// Agent forks it, so it must pass the same publish validation every
/// other package passes.
fn scaffold() -> std::path::PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
        .join("../../computer/skills/widgets/scaffold")
}

/// Run one tool of the scaffold the way the runner does: the entry as
/// the process, the package root as the working directory, and the
/// argument object on stdin.
fn run_tool(entry: &str, arguments: serde_json::Value) -> serde_json::Value {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let root = scaffold();
    let mut child = Command::new(root.join(entry))
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the entry runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(arguments.to_string().as_bytes())
        .expect("the arguments");
    let output = child.wait_with_output().expect("the tool ends");
    assert!(
        output.status.success(),
        "{entry}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the tool prints one JSON value")
}

#[test]
fn the_chart_scaffold_passes_the_publish_validation() {
    let version = validate(&scaffold()).expect("the scaffold is sound");

    assert_eq!(version.manifest.package.name, "chart");
    assert_eq!(version.manifest.widgets.len(), 1);
    assert_eq!(version.widget_schemas["chart"]["required"][0], "title");

    let show = version.manifest.tool("show").expect("the widget tool");
    assert_eq!(show.widget.as_deref(), Some("chart"));
    assert!(show.visible_to_model() && !show.awaits_input);

    let series = version.manifest.tool("series").expect("the app-only tool");
    assert!(
        !series.visible_to_model() && series.visible_to_app(),
        "the widget alone calls it"
    );
    assert!(series.widget.is_none(), "an app-only tool renders nothing");

    let ask = version.manifest.tool("ask").expect("the asking tool");
    assert!(ask.awaits_input && ask.widget.as_deref() == Some("chart"));
}

#[test]
fn the_scaffold_widget_tools_split_into_the_two_halves() {
    let version = validate(&scaffold()).expect("the scaffold is sound");
    let widget_schema = version.widget_schemas["chart"].clone();

    let shown = run_tool("bin/show.py", serde_json::json!({ "days": 3 }));
    let split = split_result(&shown, &widget_schema).expect("the result splits");
    assert!(split.content.contains("3 readings"), "{}", split.content);
    assert_eq!(
        split.structured_content["bars"].as_array().unwrap().len(),
        3
    );
    assert!(
        split.structured_content.get("question").is_none(),
        "a tool that does not ask sends no question"
    );

    let asked = run_tool(
        "bin/ask.py",
        serde_json::json!({ "question": "Which day?" }),
    );
    let split = split_result(&asked, &widget_schema).expect("the result splits");
    assert_eq!(split.structured_content["question"], "Which day?");
}

#[test]
fn the_scaffold_app_only_tool_prints_the_widgets_own_data() {
    let version = validate(&scaffold()).expect("the scaffold is sound");
    let widget_schema = version.widget_schemas["chart"].clone();

    let redrawn = run_tool("bin/series.py", serde_json::json!({ "days": 14 }));

    // The page reads this value back from the text of the tool result
    // and draws it, so it must be what the widget schema takes.
    let as_data = serde_json::json!({ "content": "x", "structuredContent": redrawn.clone() });
    split_result(&as_data, &widget_schema).expect("the redraw matches the widget schema");
    assert_eq!(redrawn["bars"].as_array().unwrap().len(), 14);
}
