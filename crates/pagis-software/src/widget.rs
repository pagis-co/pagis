//! Widgets a Software Package ships (ADR-0016).
//!
//! A `[[widget]]` entry names an HTML5 document in the version tree, a
//! JSON Schema for the data it receives, and the domains its page may
//! reach. A `[[tool]]` renders into one with `widget = "<name>"`.
//!
//! The module holds the rules that have no other home: the URI a
//! Widget is addressed by, the Content-Security-Policy the daemon
//! serves the page under, and the split of one tool result into the
//! half the Widget reads and the half the model reads.

use serde::{Deserialize, Serialize};

/// The largest HTML document one Widget carries (ADR-0016).
pub const MAX_WIDGET_HTML_BYTES: u64 = 1024 * 1024;
/// The largest `structuredContent` one tool result carries (ADR-0016).
pub const MAX_STRUCTURED_CONTENT_BYTES: usize = 256 * 1024;
/// The MIME type of a Widget page, as the MCP Apps extension sets it.
pub const WIDGET_MIME: &str = "text/html;profile=mcp-app";

/// One `[[widget]]` entry of a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetSpec {
    pub name: String,
    /// The HTML5 document, relative to the package root.
    pub html: String,
    /// The JSON Schema file of the `structuredContent` the Widget
    /// receives, relative to the package root.
    pub schema: String,
    /// The domains the page may reach. Both lists are empty by
    /// default, which is a page with no network.
    #[serde(default)]
    pub csp: WidgetCsp,
}

/// The declared domains of one Widget.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetCsp {
    /// Origins the page may open a request to.
    #[serde(default)]
    pub connect: Vec<String>,
    /// Origins the page may load a script, a style, an image, a font
    /// or a media file from.
    #[serde(default)]
    pub resource: Vec<String>,
}

/// Who may call one tool. The default is both, as the MCP Apps
/// extension has it; `["app"]` hides the tool from the model and
/// leaves it callable from the Widget alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Model,
    App,
}

/// The default `visibility` of a tool: the model and the Widget.
pub fn default_visibility() -> Vec<Visibility> {
    vec![Visibility::Model, Visibility::App]
}

impl WidgetSpec {
    /// The URI this Widget is addressed by, derived from the package
    /// name and the Widget name.
    pub fn uri(&self, package: &str) -> String {
        widget_uri(package, &self.name)
    }
}

/// The URI of one Widget of one package.
pub fn widget_uri(package: &str, widget: &str) -> String {
    format!("ui://{package}/{widget}")
}

/// The package and the Widget a `ui://` URI names.
pub fn parse_widget_uri(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix("ui://")?;
    let (package, widget) = rest.split_once('/')?;
    if package.is_empty() || widget.is_empty() || widget.contains('/') {
        return None;
    }
    Some((package, widget))
}

impl WidgetCsp {
    /// The `Content-Security-Policy` the daemon serves this Widget
    /// under. `default-src 'none'` closes everything the table below
    /// does not open, and `connect-src 'none'` is the wall: inline
    /// script stays allowed, as the MCP Apps extension has it,
    /// because a page that cannot reach the network cannot send
    /// anything out with it.
    pub fn header(&self) -> String {
        let resource = join_sources(&self.resource);
        let connect = if self.connect.is_empty() {
            "'none'".to_string()
        } else {
            self.connect.join(" ")
        };
        format!(
            "default-src 'none'; \
             script-src 'self' 'unsafe-inline'{resource}; \
             style-src 'self' 'unsafe-inline'{resource}; \
             img-src 'self' data:{resource}; \
             font-src 'self' data:{resource}; \
             media-src 'self' data:{resource}; \
             connect-src {connect}; \
             frame-src 'none'; \
             base-uri 'none'; \
             form-action 'none'"
        )
    }
}

/// The declared origins, each with a leading space, or nothing.
fn join_sources(origins: &[String]) -> String {
    origins
        .iter()
        .map(|origin| format!(" {origin}"))
        .collect::<String>()
}

/// Whether a declared CSP origin is one the daemon can serve: an
/// `https` origin with a host and nothing else. A wildcard first
/// label, such as `https://*.example.com`, is allowed, as CSP has it.
pub fn is_csp_origin(origin: &str) -> bool {
    let Some(authority) = origin.strip_prefix("https://") else {
        return false;
    };
    if authority.is_empty()
        || authority.contains('/')
        || authority.contains('?')
        || authority.contains('#')
        || authority.contains('@')
    {
        return false;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if let Some(port) = port
        && (port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    labels.iter().enumerate().all(|(index, label)| {
        (index == 0 && *label == "*")
            || (!label.is_empty()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
    })
}

/// The two halves of a Widget tool's result: the data the Widget
/// alone reads, and the author's plain-text projection the model
/// alone reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetResult {
    pub content: String,
    pub structured_content: serde_json::Value,
}

/// Split one Widget tool's JSON result, and check the data half
/// against the Widget's schema. The error is what the model reads,
/// so it names the field that is wrong.
pub fn split_result(
    result: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<WidgetResult, String> {
    let Some(object) = result.as_object() else {
        return Err("a widget tool must print a JSON object".to_string());
    };
    let Some(content) = object.get("content").and_then(serde_json::Value::as_str) else {
        return Err(
            "a widget tool must print \"content\", the plain-text projection the model reads"
                .to_string(),
        );
    };
    if content.trim().is_empty() {
        return Err(
            "\"content\" must not be empty: the model reads it in place of the widget".to_string(),
        );
    }
    let Some(structured) = object.get("structuredContent") else {
        return Err(
            "a widget tool must print \"structuredContent\", the data the widget reads".to_string(),
        );
    };
    if !structured.is_object() {
        return Err("\"structuredContent\" must be a JSON object".to_string());
    }
    let bytes = serde_json::to_vec(structured).map_or(usize::MAX, |json| json.len());
    if bytes > MAX_STRUCTURED_CONTENT_BYTES {
        return Err(format!(
            "\"structuredContent\" is {bytes} bytes; the limit is {MAX_STRUCTURED_CONTENT_BYTES}"
        ));
    }
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| format!("the widget schema does not compile: {error}"))?;
    if let Err(error) = validator.validate(structured) {
        return Err(format!(
            "\"structuredContent\" does not match the widget schema: {error}"
        ));
    }
    Ok(WidgetResult {
        content: content.to_string(),
        structured_content: structured.clone(),
    })
}
