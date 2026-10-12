//! HTTP requests through the daemon's tower services, with no listener.

use std::net::SocketAddr;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderName, HeaderValue, Request};
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
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        let (parts, body) = router.oneshot(http).await?.into_parts();
        let bytes = to_bytes(body, usize::MAX).await?;
        Ok(reqwest::Response::from(axum::http::Response::from_parts(
            parts, bytes,
        )))
    }
}
