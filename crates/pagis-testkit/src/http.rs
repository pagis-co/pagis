//! HTTP requests through the daemon's tower services, with no listener.

use std::net::SocketAddr;

use axum::body::{Body, to_bytes};
use axum::http::{HeaderName, HeaderValue, Request};
use pagis_server::Peer;
use tower::ServiceExt;

#[derive(Clone)]
pub struct Client {
    pub(crate) product: axum::Router,
    pub(crate) administration: axum::Router,
    pub(crate) administration_origin: String,
}

impl Client {
    pub fn get(&self, url: impl reqwest::IntoUrl) -> RequestBuilder {
        self.request(reqwest::Method::GET, url)
    }
    pub fn post(&self, url: impl reqwest::IntoUrl) -> RequestBuilder {
        self.request(reqwest::Method::POST, url)
    }
    pub fn put(&self, url: impl reqwest::IntoUrl) -> RequestBuilder {
        self.request(reqwest::Method::PUT, url)
    }
    pub fn delete(&self, url: impl reqwest::IntoUrl) -> RequestBuilder {
        self.request(reqwest::Method::DELETE, url)
    }
    pub fn request(&self, method: reqwest::Method, url: impl reqwest::IntoUrl) -> RequestBuilder {
        RequestBuilder {
            client: self.clone(),
            request: reqwest::Client::new().request(method, url),
        }
    }
}

pub struct RequestBuilder {
    client: Client,
    request: reqwest::RequestBuilder,
}

impl RequestBuilder {
    pub fn header<K, V>(mut self, key: K, value: V) -> Self
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<axum::http::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<axum::http::Error>,
    {
        self.request = self.request.header(key, value);
        self
    }

    pub fn json(mut self, value: &impl serde::Serialize) -> Self {
        self.request = self.request.json(value);
        self
    }

    pub fn multipart(mut self, form: reqwest::multipart::Form) -> Self {
        self.request = self.request.multipart(form);
        self
    }

    pub async fn send(self) -> Result<reqwest::Response, Box<dyn std::error::Error + Send + Sync>> {
        let mut request = self.request.build()?;
        let router =
            if request.url().origin().ascii_serialization() == self.client.administration_origin {
                self.client.administration
            } else {
                self.client.product
            };
        let body = request.body_mut().take().map(Body::new).unwrap_or_default();
        let mut http = Request::builder()
            .method(request.method())
            .uri(request.url().as_str())
            .body(body)?;
        *http.headers_mut() = request.headers().clone();
        if !http.headers().contains_key(axum::http::header::HOST) {
            let authority = http
                .uri()
                .authority()
                .expect("the request URL has a host")
                .as_str()
                .to_string();
            http.headers_mut()
                .insert(axum::http::header::HOST, authority.parse()?);
        }
        http.extensions_mut()
            .insert(Peer(SocketAddr::from(([127, 0, 0, 1], 12345))));
        let (parts, body) = router.oneshot(http).await?.into_parts();
        let bytes = to_bytes(body, usize::MAX).await?;
        Ok(reqwest::Response::from(axum::http::Response::from_parts(
            parts, bytes,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::ConnectInfo;
    use axum::http::StatusCode;
    use pagis_server::SESSION_COOKIE;

    use crate::{TestDaemon, TestDaemonOptions};

    #[tokio::test]
    async fn peer_alone_signs_in_and_passes_the_session_middleware() {
        let daemon = TestDaemon::start_in_memory().await;
        let stores = daemon.stores();
        stores
            .users
            .set_email_and_password(
                &daemon.user_id,
                "ada@example.com",
                &pagis_server::hash_password("a good password").unwrap(),
                pagis_core::now_ms(),
            )
            .await
            .unwrap();

        for router in [daemon.product.clone(), daemon.administration.clone()] {
            let request = Request::builder()
                .method("POST")
                .uri("/api/v1/sessions")
                .header("host", "localhost")
                .header("content-type", "application/json")
                .extension(Peer("127.0.0.1:54321".parse().unwrap()))
                .body(Body::from(
                    r#"{"email":"ada@example.com","password":"a good password"}"#,
                ))
                .unwrap();
            assert!(
                request
                    .extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .is_none()
            );
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let cookie = response.headers()["set-cookie"]
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_string();
            assert!(cookie.starts_with(SESSION_COOKIE));

            let request = Request::builder()
                .uri("/api/v1/user")
                .header("host", "localhost")
                .header("cookie", cookie)
                .extension(Peer("127.0.0.1:54321".parse().unwrap()))
                .body(Body::empty())
                .unwrap();
            let response = router.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["id"], daemon.user_id.to_string());
        }
    }

    #[tokio::test]
    async fn missing_peer_refuses_sign_in_and_the_session_middleware() {
        let daemon = TestDaemon::start_with(TestDaemonOptions {
            remote_access: true,
            ..Default::default()
        })
        .await;
        for router in [daemon.product.clone(), daemon.administration.clone()] {
            for (method, path, body) in [
                (
                    "POST",
                    "/api/v1/sessions",
                    r#"{"email":"ada@example.com","password":"a good password"}"#,
                ),
                ("GET", "/api/v1/user", ""),
            ] {
                for connect_info in [false, true] {
                    let mut request = Request::builder()
                        .method(method)
                        .uri(path)
                        .header("host", "localhost")
                        .header("content-type", "application/json")
                        .header("cookie", &daemon.session_cookie)
                        .body(Body::from(body))
                        .unwrap();
                    if connect_info {
                        request.extensions_mut().insert(ConnectInfo(
                            "127.0.0.1:54321".parse::<SocketAddr>().unwrap(),
                        ));
                    }
                    let response = router.clone().oneshot(request).await.unwrap();
                    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
                    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
                    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(body["error"]["code"], "internal");
                }
            }
        }
    }
}
