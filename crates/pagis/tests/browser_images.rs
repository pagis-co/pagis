//! The images of the Product App in a real browser, under the policy of
//! its entry page. Text that an Agent writes can name an image at any
//! host. The browser of the Person who reads it must send no request
//! to that host, and each image that the Product App makes must still
//! show:
//!
//! - the image Block, the Run screenshot and the Computer preview fetch
//!   their bytes through the daemon and show them from a `blob:` URL;
//! - an avatar shows a sprite portrait of the bundle, at the daemon's
//!   own origin, or a `data:` URL that a canvas draws.
//!
//! The test opens the entry page, and a script in that page loads one
//! image of each kind of address. The browser applies the policy to the
//! document, so the result is the same with or without a UI build.

use pagis_testkit::TestDaemon;
use pagis_testkit::browser::{Browser, Outcome};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::artifacts::upload;

/// A PNG that a browser decodes.
fn png() -> Vec<u8> {
    let image = image::DynamicImage::new_rgb8(4, 4);
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("a PNG");
    out.into_inner()
}

#[tokio::test]
async fn the_product_app_shows_its_own_images_and_loads_none_from_another_host() {
    let daemon = TestDaemon::start().await;
    let artifact: serde_json::Value = upload(&daemon, "shot.png", "image/png", png())
        .await
        .json()
        .await
        .expect("the Artifact");
    let artifact = format!(
        "/api/v1/artifacts/{}",
        artifact["id"].as_str().expect("an Artifact id")
    );
    // The host that the author of the text chooses. It answers with an
    // image, so an image that the browser asks it for shows.
    let host = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(png(), "image/png"))
        .mount(&host)
        .await;
    let other_host = format!("{}/p.png?d=secret", host.uri());

    let browser = Browser::launch().await;
    let tab = browser.signed_in(&daemon).await;
    let entry = format!("{}/", daemon.base_url);
    assert_eq!(tab.follow(&entry).await.outcome, Outcome::Shown);

    let shown: serde_json::Value = tab
        .evaluate(&format!(
            "async () => {{
                const load = (src) => new Promise((resolve) => {{
                    const image = new Image();
                    image.onload = () => resolve(image.naturalWidth > 0 ? 'shown' : 'empty');
                    image.onerror = () => resolve('not shown');
                    image.src = src;
                }});
                const violation = new Promise((resolve) => {{
                    document.addEventListener('securitypolicyviolation', (event) => {{
                        if (event.blockedURI.startsWith({host:?})) resolve(event.effectiveDirective);
                    }});
                    setTimeout(() => resolve('no violation within 5 s'), 5000);
                }});
                const bytes = await (await fetch({artifact:?})).blob();
                const canvas = document.createElement('canvas');
                canvas.width = 4;
                canvas.height = 4;
                canvas.getContext('2d').fillRect(0, 0, 4, 4);
                const otherHost = await load({other_host:?});
                return {{
                    objectUrl: await load(URL.createObjectURL(bytes)),
                    ownOrigin: await load({artifact:?}),
                    dataUrl: await load(canvas.toDataURL('image/png')),
                    otherHost,
                    violation: otherHost === 'shown' ? null : await violation,
                }};
            }}",
            host = host.uri(),
        ))
        .await;

    assert_eq!(
        shown["objectUrl"], "shown",
        "an Artifact from an object URL, as the image Block, the Run screenshot \
         and the Computer preview show it"
    );
    assert_eq!(
        shown["ownOrigin"], "shown",
        "an image at the daemon's own origin, as a sprite portrait is"
    );
    assert_eq!(
        shown["dataUrl"], "shown",
        "a data: URL that a canvas draws, as an avatar portrait is"
    );
    assert_eq!(shown["otherHost"], "not shown", "an image at another host");
    assert_eq!(shown["violation"], "img-src", "the policy blocks it");
    let sent = host.received_requests().await.expect("the host records");
    assert!(
        sent.is_empty(),
        "the browser sends a request to another host: {:?}",
        sent.iter()
            .map(|request| request.url.as_str())
            .collect::<Vec<_>>()
    );
}
