//! MCP over streamable HTTP. Each message uses one POST; no GET stream opens.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use futures::lock::Mutex;
use futures::stream::BoxStream;
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use rmcp::model::*;
use tokio_util::sync::CancellationToken;

use crate::session::{Overrun, Session, SessionError};
use crate::substitute::Endpoint;
use crate::supervisor::{MAX_FRAME_BYTES, STARTUP_TIMEOUT, ToolListChanged};

/// The bytes of an HTTP answer, read as they arrive.
pub type HttpBody = BoxStream<'static, Result<Bytes, String>>;

/// An Embedder supplies how one HTTP request reaches its server.
#[async_trait]
pub trait McpHttp: Send + Sync {
    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<HttpBody>, String>;
}

/// The daemon's HTTP transport, with the workspace's reqwest client.
#[derive(Default)]
pub struct ReqwestMcpHttp(reqwest::Client);

#[async_trait]
impl McpHttp for ReqwestMcpHttp {
    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<HttpBody>, String> {
        let work = async {
            let (parts, body) = request.into_parts();
            let response = self
                .0
                .request(parts.method, parts.uri.to_string())
                .headers(parts.headers)
                .body(body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let mut answer = Response::builder().status(response.status());
            *answer.headers_mut().expect("a response has headers") = response.headers().clone();
            let body = response.bytes_stream();
            #[cfg(target_arch = "wasm32")]
            let body = send_wrapper::SendWrapper::new(body);
            answer
                .body(body.map(|chunk| chunk.map_err(|e| e.to_string())).boxed())
                .map_err(|e| e.to_string())
        };
        // Browser requests and bodies stay on the isolate's one thread.
        #[cfg(target_arch = "wasm32")]
        let work = send_wrapper::SendWrapper::new(work);
        work.await
    }
}

#[derive(Clone)]
struct HttpSession {
    id: Option<HeaderValue>,
    protocol: HeaderValue,
    generation: u32,
}

pub struct HttpClient {
    http: Arc<dyn McpHttp>,
    url: String,
    headers: HeaderMap,
    session: std::sync::Mutex<Option<HttpSession>>,
    initializing: Mutex<()>,
    next_id: AtomicU32,
    closed: CancellationToken,
    pub(crate) overrun: Arc<Overrun>,
    changed: Arc<dyn ToolListChanged>,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient")
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

impl HttpClient {
    pub(crate) async fn start(
        http: Arc<dyn McpHttp>,
        endpoint: &Endpoint,
        overrun: Arc<Overrun>,
        changed: Arc<dyn ToolListChanged>,
    ) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        for (name, value) in &endpoint.headers {
            headers.insert(
                http::HeaderName::try_from(name.as_str()).map_err(|e| e.to_string())?,
                HeaderValue::try_from(value.as_str()).map_err(|e| e.to_string())?,
            );
        }
        let client = Self {
            http,
            url: endpoint.url.clone(),
            headers,
            session: std::sync::Mutex::new(None),
            initializing: Mutex::new(()),
            next_id: AtomicU32::new(1),
            closed: CancellationToken::new(),
            overrun,
            changed,
        };
        client.session(None).await?;
        Ok(client)
    }

    /// Read the session data without waiting for a handshake.
    fn current_session(&self) -> Option<HttpSession> {
        self.session
            .lock()
            .expect("the session lock is not poisoned")
            .clone()
    }

    fn id(&self) -> RequestId {
        RequestId::Number(self.next_id.fetch_add(1, Ordering::SeqCst).into())
    }

    /// Serialize a new handshake when the current session is absent or expired.
    async fn session(&self, expired: Option<u32>) -> Result<HttpSession, String> {
        if let Some(session) = self.current_session()
            && expired != Some(session.generation)
        {
            return Ok(session);
        }
        let _initializing = self.initializing.lock().await;
        if let Some(session) = self.current_session()
            && expired != Some(session.generation)
        {
            return Ok(session);
        }
        let id = self.id();
        let mut session = HttpSession {
            id: None,
            protocol: HeaderValue::from_static("2025-11-25"),
            generation: self.next_id.load(Ordering::SeqCst),
        };
        let request = ClientJsonRpcMessage::request(
            ClientRequest::InitializeRequest(InitializeRequest::new(InitializeRequestParams::new(
                ClientCapabilities::default(),
                Implementation::new("pagis", env!("CARGO_PKG_VERSION")),
            ))),
            id.clone(),
        );
        let response = self.post(&request, &session).await?;
        session.id = response.headers().get("mcp-session-id").cloned();
        let ServerResult::InitializeResult(info) = self.answer(response, &session, &id).await?
        else {
            return Err("initialize did not answer with server information".to_string());
        };
        if ![
            ProtocolVersion::V_2025_03_26,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_11_25,
        ]
        .contains(&info.protocol_version)
        {
            return Err("the server's protocol version is not supported".to_string());
        }
        session.protocol =
            HeaderValue::from_str(info.protocol_version.as_str()).map_err(|e| e.to_string())?;
        self.accept(
            &ClientJsonRpcMessage::notification(ClientNotification::InitializedNotification(
                InitializedNotification::default(),
            )),
            &session,
        )
        .await?;
        *self
            .session
            .lock()
            .expect("the session lock is not poisoned") = Some(session.clone());
        Ok(session)
    }

    async fn send(
        &self,
        method: Method,
        body: Vec<u8>,
        session: &HttpSession,
    ) -> Result<Response<HttpBody>, String> {
        let mut request = Request::builder()
            .method(method)
            .uri(&self.url)
            .body(body)
            .map_err(|e| e.to_string())?;
        *request.headers_mut() = self.headers.clone();
        let headers = request.headers_mut();
        headers.insert(
            "accept",
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        // Protocol headers are owned by the client, not by Plugin Bindings.
        headers.remove("mcp-session-id");
        if let Some(id) = &session.id {
            headers.insert("mcp-session-id", id.clone());
        }
        headers.insert("mcp-protocol-version", session.protocol.clone());
        self.http.send(request).await
    }

    async fn post(
        &self,
        message: &ClientJsonRpcMessage,
        session: &HttpSession,
    ) -> Result<Response<HttpBody>, String> {
        self.send(
            Method::POST,
            serde_json::to_vec(message).map_err(|e| e.to_string())?,
            session,
        )
        .await
    }

    async fn accept(
        &self,
        message: &ClientJsonRpcMessage,
        session: &HttpSession,
    ) -> Result<(), String> {
        let response = self.post(message, session).await?;
        if response.status() != StatusCode::ACCEPTED {
            return Err(format!(
                "the server refused the message with {}",
                response.status()
            ));
        }
        Ok(())
    }

    async fn request(&self, request: ClientRequest, id: RequestId) -> Result<ServerResult, String> {
        let answer = self.request_open(request, id);
        let closed = self.closed.cancelled();
        futures::pin_mut!(answer, closed);
        match futures::future::select(closed, answer).await {
            futures::future::Either::Left(_) => Err("the HTTP session is closed".to_string()),
            futures::future::Either::Right((answer, _)) => answer,
        }
    }

    async fn request_open(
        &self,
        request: ClientRequest,
        id: RequestId,
    ) -> Result<ServerResult, String> {
        if self.is_closed() {
            return Err("the HTTP session is closed".to_string());
        }
        let message = ClientJsonRpcMessage::request(request, id.clone());
        let mut session = self.session(None).await?;
        let mut response = self.post(&message, &session).await?;
        if response.status() == StatusCode::NOT_FOUND && session.id.is_some() {
            session = self.session(Some(session.generation)).await?;
            response = self.post(&message, &session).await?;
        }
        self.answer(response, &session, &id).await
    }

    /// Bound the bytes before the SSE parser or JSON decoder holds them.
    fn bounded(&self, body: HttpBody) -> HttpBody {
        let mut size = 0usize;
        let overrun = self.overrun.clone();
        let closed = self.closed.clone();
        body.map(move |chunk| {
            let chunk = chunk?;
            size = size.saturating_add(chunk.len());
            if size > MAX_FRAME_BYTES {
                overrun.happened.store(true, Ordering::SeqCst);
                closed.cancel();
                Err("the HTTP answer passed the frame cap".to_string())
            } else {
                Ok(chunk)
            }
        })
        .boxed()
    }

    async fn answer(
        &self,
        response: Response<HttpBody>,
        session: &HttpSession,
        id: &RequestId,
    ) -> Result<ServerResult, String> {
        if !response.status().is_success() {
            return Err(format!("the server answered with {}", response.status()));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        match content_type {
            "application/json" => {
                let mut body = self.bounded(response.into_body());
                let mut bytes = Vec::new();
                while let Some(chunk) = body.next().await {
                    bytes.extend_from_slice(&chunk?);
                }
                self.message(&bytes, session, id)
                    .await?
                    .ok_or_else(|| "the JSON answer has no result".to_string())
            }
            "text/event-stream" => {
                let mut events = self.bounded(response.into_body()).eventsource();
                while let Some(event) = events.next().await {
                    let event = event.map_err(|e| e.to_string())?;
                    if event.data.len() > MAX_FRAME_BYTES {
                        self.overrun.happened.store(true, Ordering::SeqCst);
                        self.closed.cancel();
                        return Err("the SSE event passed the frame cap".to_string());
                    }
                    if event.event != "message" || event.data.is_empty() {
                        continue;
                    }
                    if let Some(result) = self.message(event.data.as_bytes(), session, id).await? {
                        return Ok(result);
                    }
                }
                Err("the SSE answer ended without a result".to_string())
            }
            _ => Err("the HTTP answer has an unsupported content type".to_string()),
        }
    }

    async fn message(
        &self,
        bytes: &[u8],
        session: &HttpSession,
        id: &RequestId,
    ) -> Result<Option<ServerResult>, String> {
        let message: ServerJsonRpcMessage =
            serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        match message {
            JsonRpcMessage::Response(response) if response.id == *id => Ok(Some(response.result)),
            JsonRpcMessage::Error(error) if error.id.as_ref() == Some(id) => {
                Err(error.error.to_string())
            }
            JsonRpcMessage::Notification(notification) => {
                if matches!(
                    notification.notification,
                    ServerNotification::ToolListChangedNotification(_)
                ) {
                    self.changed.tool_list_changed().await;
                }
                Ok(None)
            }
            JsonRpcMessage::Request(request) => {
                let reply = if matches!(request.request, ServerRequest::PingRequest(_)) {
                    ClientJsonRpcMessage::response(ClientResult::empty(()), request.id)
                } else {
                    ClientJsonRpcMessage::error(
                        ErrorData::new(
                            ErrorCode::METHOD_NOT_FOUND,
                            "the client refuses this request",
                            None,
                        ),
                        Some(request.id),
                    )
                };
                self.accept(&reply, session).await?;
                Ok(None)
            }
            _ => Err("the server answered a different request".to_string()),
        }
    }

    pub(crate) async fn list_tools(&self) -> Result<Vec<Tool>, String> {
        let mut tools = Vec::new();
        let mut cursor = None;
        loop {
            let params =
                cursor.map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor)));
            let request = ListToolsRequest {
                params,
                ..Default::default()
            };
            let result = self
                .request(ClientRequest::ListToolsRequest(request), self.id())
                .await?;
            let ServerResult::ListToolsResult(page) = result else {
                return Err("tools/list has no tool list".to_string());
            };
            tools.extend(page.tools);
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(tools);
            }
        }
    }

    pub(crate) async fn call(
        &self,
        params: CallToolRequestParams,
        deadline: Duration,
    ) -> Result<CallToolResult, String> {
        let id = self.id();
        let result = pagis_rt::timeout(
            deadline,
            self.request(
                ClientRequest::CallToolRequest(CallToolRequest::new(params)),
                id.clone(),
            ),
        )
        .await;
        let result = match result {
            Ok(result) => result?,
            Err(_) => {
                if let Some(session) = self.current_session() {
                    let cancel = ClientJsonRpcMessage::notification(
                        ClientNotification::CancelledNotification(CancelledNotification::new(
                            CancelledNotificationParam::new(
                                Some(id),
                                Some("the call deadline passed".to_string()),
                            ),
                        )),
                    );
                    let _ =
                        pagis_rt::timeout(STARTUP_TIMEOUT, self.accept(&cancel, &session)).await;
                }
                return Err(format!(
                    "the call did not answer in {} seconds",
                    deadline.as_secs()
                ));
            }
        };
        match result {
            ServerResult::CallToolResult(result) => Ok(result),
            _ => Err("tools/call has no tool result".to_string()),
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.is_cancelled() || self.overrun.happened()
    }

    pub(crate) async fn stop(&self) {
        self.closed.cancel();
        let session = self
            .session
            .lock()
            .expect("the session lock is not poisoned")
            .take();
        if let Some(session) = session
            && session.id.is_some()
        {
            let _ = pagis_rt::timeout(
                STARTUP_TIMEOUT,
                self.send(Method::DELETE, Vec::new(), &session),
            )
            .await;
        }
    }
}

#[async_trait]
impl Session for HttpClient {
    async fn list_tools(&self) -> Result<Vec<Tool>, String> {
        self.list_tools().await
    }
    async fn call(
        &self,
        params: CallToolRequestParams,
        deadline: Duration,
    ) -> Result<CallToolResult, SessionError> {
        self.call(params, deadline)
            .await
            .map_err(|error| self.overrun.error(error))
    }
    fn is_closed(&self) -> bool {
        self.is_closed()
    }
    async fn close(&self) {
        self.stop().await;
    }
}
