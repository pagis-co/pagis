//! The Widget surface (ADR-0016): the page a Version ships, and
//! the one JSON-RPC address a rendered view speaks to.
//!
//! A Widget is an HTML5 document inside a Software Version, so the
//! page is immutable and cached forever. Only the sandbox proxy runs
//! it, under the Content-Security-Policy the manifest declared: a page
//! reaches only the origins its author named. At the Product App origin
//! the page is inert data, which the Product App reads as text and
//! sends to the proxy.
//!
//! The view has no other way into the daemon. It holds a
//! `tool_call_id`, and that id names the one view record the broker
//! kept: the Run, the snapshot, the package, and the `widget` Request
//! it may answer. Nothing the view sends can widen that: a `tools/call`
//! is qualified with the view's own package before the broker sees it,
//! a `resources/read` is refused outside the view's package, and a
//! `ui/message` answers one Request exactly once.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use pagis_broker::{InvokeOutcome, ToolCall, WidgetView};
use pagis_core::{DecideOutcome, NewEvent, Request, RequestState, now_ms, validate_values};
use pagis_software::{WIDGET_MIME, WidgetSpec, parse_widget_uri};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;
use crate::served_file;

/// The only JSON-RPC version the view speaks.
const JSONRPC_VERSION: &str = "2.0";
/// A malformed envelope, or an answer the view already gave.
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;

/// A Version never changes (ADR-0016), so its page is cached forever.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

// -- the page ------------------------------------------------------

#[utoipa::path(
    get,
    path = "/api/v1/widgets/{package}/{version}/{widget}",
    params(
        ("package" = String, Path, description = "The Software Package name"),
        ("version" = String, Path, description = "The Version tag"),
        ("widget" = String, Path, description = "The Widget name"),
    ),
    responses(
        (status = 200, description = "The Widget page as inert bytes to save. Only the sandbox proxy runs it.", content_type = "application/octet-stream"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn widget_page(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((package, version, widget)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    let html = widget_source(&state, &tenant.workspace_id, &package, &version, &widget).await?;
    // A browser that opens this route directly saves the page and runs
    // nothing at the Product App origin.
    Ok((
        [(header::CACHE_CONTROL, IMMUTABLE)],
        served_file::attachment(None),
        html,
    )
        .into_response())
}

/// The Widget's own declaration inside the resolved Version.
async fn widget_spec(
    state: &AppState,
    workspace_id: &pagis_core::WorkspaceId,
    package: &str,
    version: &str,
    widget: &str,
) -> Result<WidgetSpec, ApiError> {
    let resolved = state
        .versions
        .version(workspace_id, package, version)
        .await
        .map_err(|error| {
            tracing::debug!(%package, %version, %error, "no such software version");
            ApiError::not_found("widget")
        })?;
    resolved
        .manifest
        .widget(widget)
        .cloned()
        .ok_or_else(|| ApiError::not_found("widget"))
}

/// The bytes of the Widget's page. A package, a Version, a
/// `[[widget]]` entry or a file that is missing is one 404: the view
/// asked for a page that does not exist, and which of the four is
/// missing is the daemon's business, not the caller's.
async fn widget_source(
    state: &AppState,
    workspace_id: &pagis_core::WorkspaceId,
    package: &str,
    version: &str,
    widget: &str,
) -> Result<Vec<u8>, ApiError> {
    let spec = widget_spec(state, workspace_id, package, version, widget).await?;
    state
        .versions
        .file(workspace_id, package, version, &spec.html)
        .await
        .map_err(|error| {
            tracing::warn!(%package, %version, %widget, %error, "the widget page is missing");
            ApiError::not_found("widget")
        })
}

// -- the sandbox proxy ---------------------------------------------

/// The proxy document itself. It holds no data: it loads the page the
/// host sends it into an inner frame at an opaque origin, and forwards
/// messages.
const SANDBOX_HTML: &str = include_str!("widget_sandbox.html");

#[utoipa::path(
    get,
    path = "/api/v1/widgets/{workspace_id}/{package}/{version}/{widget}/sandbox",
    params(
        ("workspace_id" = String, Path, description = "The Workspace the Widget belongs to"),
        ("package" = String, Path, description = "The Software Package name"),
        ("version" = String, Path, description = "The Version tag"),
        ("widget" = String, Path, description = "The Widget name"),
    ),
    responses((status = 200, description = "The sandbox proxy", content_type = "text/html")),
)]
pub async fn widget_sandbox(
    State(state): State<Arc<AppState>>,
    Path((workspace_id, package, version, widget)): Path<(String, String, String, String)>,
) -> Response {
    // No session needed: the proxy document holds nothing to protect. A
    // sandboxed frame sends no cookie, so the tenant is in the path
    // instead: a Widget of one tenant gets that tenant's declared
    // policy, and a caller who names another tenant reads nothing of it.
    // An unknown Workspace, package, Version or Widget answers the same
    // way, under the closed default policy, so this route tells an
    // unauthenticated caller nothing about what is installed anywhere.
    let workspace_id = pagis_core::WorkspaceId::from(workspace_id);
    let csp = state
        .versions
        .sandbox_csp(&workspace_id, &package, &version, &widget)
        .await
        .unwrap_or_default();
    // The `srcdoc` page inherits this header, so the Widget's policy is
    // enforced by the browser on content the page cannot rewrite.
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (header::CONTENT_SECURITY_POLICY, csp.header()),
            (header::CACHE_CONTROL, IMMUTABLE.to_string()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        SANDBOX_HTML,
    )
        .into_response()
}

// -- the view record -----------------------------------------------

/// The half of a Widget view that is data: what the tool was called
/// with, and what it answered. The host pushes them into the view as
/// `ui/notifications/tool-input` and `ui/notifications/tool-result`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WidgetViewDto {
    pub tool_call_id: String,
    pub package: String,
    pub version: String,
    pub widget: String,
    pub tool_input: Value,
    pub structured_content: Value,
}

#[utoipa::path(
    get,
    path = "/api/v1/widgets/{tool_call_id}/view",
    params(("tool_call_id" = String, Path, description = "The tool call the view renders")),
    responses(
        (status = 200, description = "The live view", body = WidgetViewDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, description = "No live view with that tool call id", body = crate::error::ErrorBody),
    )
)]
pub async fn widget_view(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(tool_call_id): Path<String>,
) -> Result<Json<WidgetViewDto>, ApiError> {
    let view = state
        .broker
        .widget_view(&tenant.workspace_id, &tool_call_id)
        .ok_or_else(|| ApiError::not_found("widget view"))?;
    Ok(Json(WidgetViewDto {
        tool_call_id: view.tool_call_id,
        package: view.package,
        version: view.version,
        widget: view.widget,
        tool_input: view.tool_input,
        structured_content: view.structured_content,
    }))
}

// -- the envelope --------------------------------------------------

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcError {
    fn new(code: i32, message: impl Into<String>) -> Self {
        JsonRpcError {
            code,
            message: message.into(),
        }
    }

    fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(INVALID_REQUEST, message)
    }

    fn method_not_found(message: impl Into<String>) -> Self {
        Self::new(METHOD_NOT_FOUND, message)
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, message)
    }
}

impl JsonRpcResponse {
    fn ok(id: Value, result: Value) -> Self {
        JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn failed(id: Value, error: JsonRpcError) -> Self {
        JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

// -- the view's JSON-RPC -------------------------------------------

#[utoipa::path(
    post,
    path = "/api/v1/widgets/{tool_call_id}/rpc",
    params(("tool_call_id" = String, Path, description = "The tool call the view renders")),
    request_body = JsonRpcRequest,
    responses(
        (status = 200, description = "The JSON-RPC response, result or error", body = JsonRpcResponse),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, description = "No live view with that tool call id", body = crate::error::ErrorBody),
    )
)]
pub async fn widget_rpc(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(tool_call_id): Path<String>,
    Json(request): Json<JsonRpcRequest>,
) -> Result<Json<JsonRpcResponse>, ApiError> {
    // The view record is the whole authority: no live view, no call.
    let view = state
        .broker
        .widget_view(&tenant.workspace_id, &tool_call_id)
        .ok_or_else(|| ApiError::not_found("widget view"))?;
    let id = request.id.clone();
    if request.jsonrpc != JSONRPC_VERSION {
        return Ok(Json(JsonRpcResponse::failed(
            id,
            JsonRpcError::invalid_request(format!(
                "jsonrpc must be \"{JSONRPC_VERSION}\", not \"{}\"",
                request.jsonrpc
            )),
        )));
    }
    Ok(Json(match dispatch(&state, &view, &request).await {
        Ok(result) => JsonRpcResponse::ok(id, result),
        Err(error) => JsonRpcResponse::failed(id, error),
    }))
}

/// The methods a view may call, and nothing else. A failure here is a
/// JSON-RPC error inside a 200: the transport carried the call.
async fn dispatch(
    state: &AppState,
    view: &WidgetView,
    request: &JsonRpcRequest,
) -> Result<Value, JsonRpcError> {
    match request.method.as_str() {
        "ping" => Ok(json!({})),
        "tools/call" => call_tool(state, view, &request.params).await,
        "resources/read" => read_resource(state, view, &request.params).await,
        "ui/message" => answer(state, view, &request.params).await,
        other => Err(JsonRpcError::method_not_found(format!(
            "a widget cannot call {other}"
        ))),
    }
}

/// One tool call from the view. The name the view sends is the
/// package's own, unqualified, and it is qualified with the view's
/// package here: a view cannot name another package's tool at all,
/// before the broker's own check on the call origin ever runs.
async fn call_tool(
    state: &AppState,
    view: &WidgetView,
    params: &Value,
) -> Result<Value, JsonRpcError> {
    let name = params.get("name").and_then(Value::as_str).ok_or_else(|| {
        JsonRpcError::invalid_params("tools/call needs \"name\", the tool to call")
    })?;
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(value) if value.is_object() => value.clone(),
        Some(_) => {
            return Err(JsonRpcError::invalid_params(
                "\"arguments\" must be a JSON object",
            ));
        }
    };
    let call = ToolCall::new(qualified(&view.package, name), arguments.to_string())
        .from_widget(view.package.clone());
    match state
        .broker
        .invoke(&view.workspace_id, &view.run_id, &view.snapshot_id, call)
        .await
    {
        Ok(InvokeOutcome::Completed(result)) => Ok(tool_result(&result.content, false)),
        Ok(InvokeOutcome::Rejected(result)) => Ok(tool_result(&result.content, true)),
        // The user answers a card in the daemon's own surfaces. A
        // widget must not park a run behind a frame nobody watches.
        Ok(InvokeOutcome::Waiting(_)) => Err(JsonRpcError::internal(
            "this tool needs an approval the widget cannot give",
        )),
        Err(error) => Err(JsonRpcError::internal(error.to_string())),
    }
}

/// The tool name as the snapshot holds it.
fn qualified(package: &str, tool: &str) -> String {
    format!("{package}__{tool}")
}

/// One tool result in the MCP shape the view reads.
fn tool_result(content: &str, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": content }],
        "isError": is_error,
    })
}

/// The page behind a `ui://` URI. The view reads its own package's
/// Widgets and nothing else.
async fn read_resource(
    state: &AppState,
    view: &WidgetView,
    params: &Value,
) -> Result<Value, JsonRpcError> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| JsonRpcError::invalid_params("resources/read needs \"uri\""))?;
    let widget = scoped_widget(&view.package, uri)?;
    let html = widget_source(
        state,
        &view.workspace_id,
        &view.package,
        &view.version,
        widget,
    )
    .await
    .map_err(|error| JsonRpcError::invalid_params(error.message))?;
    let text = String::from_utf8(html)
        .map_err(|_| JsonRpcError::internal("the widget page is not UTF-8"))?;
    Ok(json!({
        "contents": [{ "uri": uri, "mimeType": WIDGET_MIME, "text": text }],
    }))
}

/// The Widget a `ui://` URI names, when it belongs to `package`.
fn scoped_widget<'a>(package: &str, uri: &'a str) -> Result<&'a str, JsonRpcError> {
    let Some((named, widget)) = parse_widget_uri(uri) else {
        return Err(JsonRpcError::invalid_params(format!(
            "{uri} is not a ui:// widget URI"
        )));
    };
    if named != package {
        return Err(JsonRpcError::invalid_params(format!(
            "a widget of {package} cannot read {uri}"
        )));
    }
    Ok(widget)
}

/// The view's one answer to the `widget` Request the tool parked on.
/// The decision wakes the run, and the store's own pending check makes
/// it one-shot: a second call finds the row decided and is refused.
async fn answer(
    state: &AppState,
    view: &WidgetView,
    params: &Value,
) -> Result<Value, JsonRpcError> {
    let Some(request_id) = view.request_id.clone() else {
        return Err(JsonRpcError::method_not_found(
            "this widget does not await input",
        ));
    };
    let request = state
        .requests
        .get(&view.workspace_id, &request_id)
        .await
        .map_err(|error| JsonRpcError::internal(error.to_string()))?
        .ok_or_else(|| JsonRpcError::internal("the request this widget answers is gone"))?;
    // The schema comes from the row, never from anything the view sent.
    let values = validate_values(Request::WIDGET_KIND, &request.payload, Some(params))
        .map_err(JsonRpcError::invalid_params)?;

    match state
        .requests
        .decide(
            &view.workspace_id,
            &request_id,
            RequestState::Approved,
            Some(values),
            now_ms(),
        )
        .await
        .map_err(|error| JsonRpcError::internal(error.to_string()))?
    {
        DecideOutcome::Decided(decided) => {
            // The same event the decision route publishes: it wakes the
            // parked run and invalidates every card view.
            state
                .bus
                .publish(NewEvent {
                    workspace_id: decided.workspace_id.clone(),
                    event_type: "request.decided".to_string(),
                    agent_id: Some(decided.agent_id.clone()),
                    run_id: decided.run_id.clone(),
                    channel_id: None,
                    payload: json!({
                        "request_id": decided.id.as_str(),
                        "decision": "approved",
                    }),
                })
                .await
                .map_err(|error| JsonRpcError::internal(error.to_string()))?;
            Ok(json!({}))
        }
        DecideOutcome::NotPending(_) => Err(JsonRpcError::invalid_request(
            "this widget already answered",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_name_is_qualified_with_the_views_package() {
        assert_eq!(qualified("weather", "forecast"), "weather__forecast");
    }

    #[test]
    fn a_uri_of_the_views_own_package_names_its_widget() {
        assert_eq!(
            scoped_widget("weather", "ui://weather/forecast")
                .expect("the view reads its own widget"),
            "forecast"
        );
    }

    #[test]
    fn a_uri_of_another_package_is_refused() {
        let error = scoped_widget("weather", "ui://banking/balance")
            .expect_err("a widget reads only its own package");
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("ui://banking/balance"), "{error:?}");
    }

    #[test]
    fn a_uri_that_is_not_a_widget_uri_is_refused() {
        for uri in [
            "https://example.com/page",
            "ui://weather",
            "ui:///forecast",
            "ui://weather/a/b",
        ] {
            let error = scoped_widget("weather", uri).expect_err(uri);
            assert_eq!(error.code, INVALID_PARAMS, "{uri}");
        }
    }

    #[test]
    fn a_request_without_params_parses_with_an_empty_value() {
        let request: JsonRpcRequest =
            serde_json::from_value(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
                .expect("the envelope parses");
        assert_eq!(request.params, Value::Null);
    }

    #[test]
    fn a_response_carries_result_or_error_and_never_both() {
        let ok = serde_json::to_value(JsonRpcResponse::ok(json!(1), json!({}))).expect("JSON");
        assert_eq!(ok, json!({ "jsonrpc": "2.0", "id": 1, "result": {} }));

        let failed = serde_json::to_value(JsonRpcResponse::failed(
            json!("a"),
            JsonRpcError::method_not_found("a widget cannot call tools/list"),
        ))
        .expect("JSON");
        assert_eq!(
            failed,
            json!({
                "jsonrpc": "2.0",
                "id": "a",
                "error": { "code": -32601, "message": "a widget cannot call tools/list" },
            })
        );
    }

    #[test]
    fn a_tool_result_carries_the_content_and_the_error_flag() {
        assert_eq!(
            tool_result("refused", true),
            json!({ "content": [{ "type": "text", "text": "refused" }], "isError": true })
        );
    }
}
