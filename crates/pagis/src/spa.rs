//! Serve the built pages of `ui/dist`: the product SPA at
//! `/` on the product port, and the administration SPA at `/` on the
//! administration port. Both are entry points of the one `ui` package
//! and share its assets, so one build serves both. Release builds embed
//! the bundle in the binary (rust-embed); debug builds read `ui/dist`
//! from disk, so `npm run build` output shows without a recompile.

use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../ui/dist"]
struct Assets;

/// The Content-Security-Policy of the Product App. Text that an Agent
/// writes can name an image at any host, and the browser of each Person
/// who reads it must send no request to that host. So an image loads
/// only from the daemon, from a `data:` URL or from a `blob:` URL that
/// the page makes. The policy has no other directive, so it limits
/// images only.
const PRODUCT_APP_POLICY: &str = "img-src 'self' data: blob:";

/// Shown when the checkout has no UI build. Tells the developer how to
/// produce one.
const UNBUILT: &str = "<!doctype html><html><head><title>pagis</title></head>\
<body><h1>pagis</h1><p>The UI bundle is not built. \
Run <code>npm ci &amp;&amp; npm run build</code> in <code>ui/</code>, \
then reload this page.</p></body></html>";

/// True when this binary can serve the Product App entry point. Release
/// packaging calls this without booting a Workspace or touching secrets.
pub fn product_app_is_built() -> bool {
    Assets::get(PRODUCT_PAGE).is_some()
}

/// The entry point of the product SPA.
const PRODUCT_PAGE: &str = "index.html";
/// The entry point of the administration SPA. It is a second
/// Vite entry of the same package, so it reuses every component the
/// product uses and cannot drift from it.
const ADMINISTRATION_PAGE: &str = "administration.html";

/// The product port's fallback: static assets by path, `index.html` for
/// every other non-API path. Each response carries the Product App
/// policy. A browser applies a policy only to a document, so the policy
/// applies to the entry page at each path, `/index.html` included, and
/// the other responses ignore it.
pub async fn serve(uri: Uri) -> Response {
    let mut response = page(uri, PRODUCT_PAGE);
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(PRODUCT_APP_POLICY),
    );
    response
}

/// The administration port's fallback. It serves the
/// administration page and never the product one: a browser on this port
/// reaches the administration SPA at every address.
///
/// It holds no data of anybody, which is why it sits outside the
/// administrator guard: the page it returns is the same for a person
/// with no Session at all, and the sign-in it shows is the guarded
/// API's answer.
pub async fn serve_administration(uri: Uri) -> Response {
    page(uri, ADMINISTRATION_PAGE)
}

/// Static assets by path, the named page for every other non-API path.
/// Unknown `/api` paths stay 404 so API typos do not come back as HTML.
fn page(uri: Uri, entry: &str) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": { "code": "not_found", "message": "no such route" }
            })),
        )
            .into_response();
    }

    // Hashed build assets are immutable; an entry page must revalidate
    // so a new build reaches the browser.
    if let Some(asset) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return (
            [
                (header::CONTENT_TYPE, mime.as_ref().to_string()),
                (
                    header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable".to_string(),
                ),
            ],
            asset.data.into_owned(),
        )
            .into_response();
    }
    match Assets::get(entry) {
        Some(index) => (
            [(header::CACHE_CONTROL, "no-cache".to_string())],
            Html(index.data.into_owned()),
        )
            .into_response(),
        None => Html(UNBUILT).into_response(),
    }
}
