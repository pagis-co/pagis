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

    if let Some(asset) = Assets::get(path) {
        return (
            [
                (header::CONTENT_TYPE, content_type(path)),
                (header::CACHE_CONTROL, cache_control(path).to_string()),
            ],
            asset.data.into_owned(),
        )
            .into_response();
    }
    match Assets::get(entry) {
        Some(index) => (
            [(header::CACHE_CONTROL, cache_control(entry))],
            Html(index.data.into_owned()),
        )
            .into_response(),
        None => Html(UNBUILT).into_response(),
    }
}

/// The media type of a file of `ui/dist`, from its extension.
fn content_type(path: &str) -> String {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .as_ref()
        .to_string()
}

/// The cache rule of a file of `ui/dist`. Only Vite's `assets/`
/// directory holds names with a content hash, so only those files are
/// immutable. Every other file, such as `sw.js`, the web app manifest,
/// an icon or an entry page, keeps its name from build to build, so the
/// browser revalidates it and a new build reaches the browser.
fn cache_control(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hashed_build_asset_is_immutable() {
        assert_eq!(
            cache_control("assets/index-B2x9kQ1z.js"),
            "public, max-age=31536000, immutable"
        );
    }

    /// A file with no hash in its name keeps its name from build to
    /// build, so the browser asks the daemon again before it uses a
    /// stored copy.
    #[test]
    fn a_file_with_a_fixed_name_revalidates() {
        for path in [
            "sw.js",
            "manifest.webmanifest",
            "icon-192.png",
            "apple-touch-icon.png",
            "index.html",
        ] {
            assert_eq!(cache_control(path), "no-cache", "{path}");
        }
    }

    #[test]
    fn the_web_app_manifest_has_its_media_type() {
        assert_eq!(
            content_type("manifest.webmanifest"),
            "application/manifest+json"
        );
    }
}
