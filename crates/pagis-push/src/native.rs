//! The native HTTP client and the I/O of one Web Push.

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use http::Request;
use reqwest::{Client, Response};

use crate::guard::AddressGuard;
use crate::web_push::MAX_ERROR_BODY;
use crate::{Outcome, Policy};

pub(crate) fn client(policy: Policy) -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .https_only(policy == Policy::Public)
        .dns_resolver(Arc::new(AddressGuard::new(policy)))
        .build()
}

pub(crate) async fn send(client: &Client, request: Request<Vec<u8>>) -> Outcome {
    let request = match reqwest::Request::try_from(request) {
        Ok(request) => request,
        Err(error) => return Outcome::failed(error_chain(&error)),
    };
    let mut response = match client.execute(request).await {
        Ok(response) => response,
        Err(error) => return Outcome::failed(error_chain(&error)),
    };
    let now = pagis_core::now_ms();
    let status = response.status();
    let body_start = if status.is_success() || matches!(status.as_u16(), 404 | 410 | 413 | 429) {
        Vec::new()
    } else {
        error_body(&mut response).await
    };
    Outcome::of(status, response.headers(), &body_start, now)
}

/// Keep only the start of a failed answer. Do not read the rest.
async fn error_body(response: &mut Response) -> Vec<u8> {
    let mut body = Vec::new();
    while body.len() < MAX_ERROR_BODY {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = MAX_ERROR_BODY - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            _ => break,
        }
    }
    body
}

/// Keep each source, so the address guard's reason reaches the caller.
fn error_chain(error: &dyn Error) -> String {
    let mut chain = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        chain.push_str(": ");
        chain.push_str(&cause.to_string());
        source = cause.source();
    }
    chain
}
