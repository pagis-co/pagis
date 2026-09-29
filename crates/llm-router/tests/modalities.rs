//! Tests for embeddings and image generation.

use crate::common;

use common::single_provider_router;
use llm_router::{
    EmbeddingsRequest, Error, GeneratedImage, ImageData, ImageInput, ImageRequest, ProtocolKind,
    SizeSpec,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn embeddings_round_trip_on_openai_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "concrete-model",
            "data": [
                {"index": 1, "embedding": [0.3, 0.4]},
                {"index": 0, "embedding": [0.1, 0.2]}
            ],
            "usage": {"prompt_tokens": 7, "total_tokens": 7}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let req = EmbeddingsRequest::new("m", ["hello".to_owned(), "world".to_owned()]);
    let response = router.embed(&req).await.unwrap();

    // Vectors come back in input order even when the provider reorders them.
    assert_eq!(response.embeddings, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    assert_eq!(response.usage.input_tokens, 7);

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["model"], "concrete-model");
    assert_eq!(sent["input"], json!(["hello", "world"]));
}

#[tokio::test]
async fn embeddings_are_unsupported_on_anthropic_protocol() {
    let server = MockServer::start().await;
    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let req = EmbeddingsRequest::new("m", ["hello".to_owned()]);
    let error = router.embed(&req).await.unwrap_err();
    let Error::Exhausted { last, .. } = error else {
        panic!("expected Exhausted, got: {error:?}");
    };
    assert!(matches!(*last, Error::Unsupported { feature, .. } if feature == "embeddings"));
    // Nothing was sent upstream.
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn image_generation_round_trips_on_openai_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "created": 1,
            "output_format": "png",
            "data": [
                {"b64_json": "QUJD", "revised_prompt": "A cat wearing a hat."},
                {"url": "https://example.com/img.png"}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 4000, "total_tokens": 4010}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let mut req = ImageRequest::new("m", "a cat in a hat");
    req.count = Some(2);
    req.size = Some(SizeSpec::pixels(1024, 1024));
    req.quality = Some("high".into());
    req.background = Some("transparent".into());
    let response = router.generate_image(&req).await.unwrap();

    assert_eq!(
        response.images,
        vec![
            GeneratedImage {
                image: ImageData::B64 {
                    data: "QUJD".into()
                },
                revised_prompt: Some("A cat wearing a hat.".into()),
            },
            GeneratedImage {
                image: ImageData::Url {
                    url: "https://example.com/img.png".into()
                },
                revised_prompt: None,
            },
        ]
    );
    assert_eq!(response.mime_type.as_deref(), Some("image/png"));
    assert_eq!(response.usage.unwrap().output_tokens, 4000);

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["model"], "concrete-model");
    assert_eq!(sent["prompt"], "a cat in a hat");
    assert_eq!(sent["n"], 2);
    assert_eq!(sent["size"], "1024x1024");
    assert_eq!(sent["quality"], "high");
    assert_eq!(sent["background"], "transparent");
}

#[tokio::test]
async fn input_images_route_to_the_multipart_edits_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/images/edits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"b64_json": "REVG"}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let mut req = ImageRequest::new("m", "make the cat orange");
    // "ABC" as base64, twice: several images use the array field name.
    req.input_images = vec![
        ImageInput::B64 {
            data: "QUJD".into(),
            media_type: "image/png".into(),
        },
        ImageInput::B64 {
            data: "QUJD".into(),
            media_type: "image/png".into(),
        },
    ];
    req.mask = Some(ImageInput::B64 {
        data: "QUJD".into(),
        media_type: "image/png".into(),
    });
    let response = router.generate_image(&req).await.unwrap();
    assert_eq!(
        response.images[0].image,
        ImageData::B64 {
            data: "REVG".into()
        }
    );

    let request = &server.received_requests().await.unwrap()[0];
    let content_type = request
        .headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.starts_with("multipart/form-data"));
    let body = String::from_utf8_lossy(&request.body);
    assert!(body.contains("name=\"image[]\""));
    assert!(body.contains("filename=\"image.png\""));
    assert!(body.contains("name=\"mask\""));
    assert!(body.contains("name=\"prompt\""));
    // The base64 input decoded to raw bytes on the wire.
    assert!(body.contains("ABC"));
}

#[tokio::test]
async fn url_input_images_are_rejected_for_edits() {
    let server = MockServer::start().await;
    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let mut req = ImageRequest::new("m", "edit this");
    req.input_images = vec![ImageInput::Url {
        url: "https://example.com/cat.png".into(),
    }];
    let error = router.generate_image(&req).await.unwrap_err();
    assert!(matches!(error, Error::InvalidConfig(_)));
    assert!(server.received_requests().await.unwrap().is_empty());
}
