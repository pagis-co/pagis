//! Tests for video generation jobs.

use crate::common;

use common::single_provider_router;
use llm_router::{Error, ImageInput, ProtocolKind, SizeSpec, VideoRequest, VideoStatus};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn video_job_lifecycle_on_openai_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/videos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "video_123",
            "object": "video",
            "model": "concrete-model",
            "status": "queued",
            "progress": 0,
            "created_at": 1700000000u64
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/video_123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "video_123",
            "status": "completed",
            "progress": 100,
            "created_at": 1700000000u64,
            "expires_at": 1700086400u64
        })))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/video_123/content"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "video/mp4")
                .set_body_bytes(b"mp4-bytes".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let mut req = VideoRequest::new("m", "a cat surfing");
    req.seconds = Some(8);
    req.size = Some(SizeSpec::pixels(1280, 720));
    let job = router.create_video(&req).await.unwrap();

    // The job id is router-scoped: provider + native id.
    assert_eq!(job.id, "p:video_123");
    assert_eq!(job.status, VideoStatus::Queued);
    assert_eq!(job.provider, "p");
    assert_eq!(job.model, "concrete-model");

    let polled = router.video_status(&job.id).await.unwrap();
    assert_eq!(polled.id, "p:video_123");
    assert_eq!(polled.status, VideoStatus::Completed);
    assert_eq!(polled.progress, Some(100));

    let bytes = router.video_content(&job.id).await.unwrap();
    assert_eq!(bytes.as_ref(), b"mp4-bytes");

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["model"], "concrete-model");
    assert_eq!(sent["prompt"], "a cat surfing");
    // Seconds go as a string enum on this API.
    assert_eq!(sent["seconds"], "8");
    assert_eq!(sent["size"], "1280x720");
}

#[tokio::test]
async fn content_before_completion_reports_job_not_ready() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/videos/video_123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "video_123",
            "status": "in_progress",
            "progress": 40
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let error = router.video_content("p:video_123").await.unwrap_err();
    assert!(matches!(
        error,
        Error::JobNotReady {
            status: VideoStatus::InProgress,
            ..
        }
    ));
}

#[tokio::test]
async fn video_job_lifecycle_on_veo_protocol() {
    let server = MockServer::start().await;
    let operation = "models/concrete-model/operations/op123";
    Mock::given(method("POST"))
        .and(path("/models/concrete-model:predictLongRunning"))
        .and(header("x-goog-api-key", "test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": operation
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/{operation}")))
        .and(header("x-goog-api-key", "test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": operation,
            "done": true,
            "response": {
                "generateVideoResponse": {
                    "generatedSamples": [
                        {"video": {"uri": format!("{}/files/video.mp4", server.uri())}}
                    ]
                }
            }
        })))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/files/video.mp4"))
        .and(header("x-goog-api-key", "test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "video/mp4")
                .set_body_bytes(b"veo-bytes".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::Veo, &server.uri());
    let mut req = VideoRequest::new("m", "a dog skating");
    req.seconds = Some(8);
    req.size = Some(SizeSpec::Aspect {
        ratio: "16:9".into(),
        tier: Some("720p".into()),
    });
    req.input_image = Some(ImageInput::B64 {
        data: "QUJD".into(),
        media_type: "image/png".into(),
    });
    let job = router.create_video(&req).await.unwrap();
    assert_eq!(job.id, format!("p:{operation}"));
    assert_eq!(job.status, VideoStatus::InProgress);

    let polled = router.video_status(&job.id).await.unwrap();
    assert_eq!(polled.status, VideoStatus::Completed);
    assert!(polled.video_url.is_some());

    let bytes = router.video_content(&job.id).await.unwrap();
    assert_eq!(bytes.as_ref(), b"veo-bytes");

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["instances"][0]["prompt"], "a dog skating");
    assert_eq!(
        sent["instances"][0]["image"],
        json!({"bytesBase64Encoded": "QUJD", "mimeType": "image/png"})
    );
    assert_eq!(sent["parameters"]["durationSeconds"], 8);
    assert_eq!(sent["parameters"]["aspectRatio"], "16:9");
    assert_eq!(sent["parameters"]["resolution"], "720p");
}

#[tokio::test]
async fn failed_veo_operation_reports_the_error() {
    let server = MockServer::start().await;
    let operation = "models/concrete-model/operations/op666";
    Mock::given(method("GET"))
        .and(path(format!("/{operation}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": operation,
            "done": true,
            "error": {"code": 3, "message": "prompt was blocked"}
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::Veo, &server.uri());
    let job = router
        .video_status(&format!("p:{operation}"))
        .await
        .unwrap();
    assert_eq!(job.status, VideoStatus::Failed);
    assert_eq!(job.error.as_deref(), Some("prompt was blocked"));
}

#[tokio::test]
async fn video_is_unsupported_on_anthropic_protocol() {
    let server = MockServer::start().await;
    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let error = router
        .create_video(&VideoRequest::new("m", "a cat"))
        .await
        .unwrap_err();
    let Error::Exhausted { last, .. } = error else {
        panic!("expected Exhausted, got: {error:?}");
    };
    assert!(matches!(*last, Error::Unsupported { feature, .. } if feature == "video generation"));
}

#[tokio::test]
async fn a_foreign_job_id_is_rejected() {
    let server = MockServer::start().await;
    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let error = router.video_status("video_123").await.unwrap_err();
    assert!(matches!(error, Error::InvalidConfig(_)));
}

#[tokio::test]
async fn a_video_uri_outside_the_provider_host_is_rejected() {
    let server = MockServer::start().await;
    let operation = "models/concrete-model/operations/op9";
    // The completed operation points its artifact at a foreign host; the
    // fetch would carry the API key, so the router must refuse.
    Mock::given(method("GET"))
        .and(path(format!("/{operation}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": operation,
            "done": true,
            "response": {"generateVideoResponse": {"generatedSamples": [
                {"video": {"uri": "https://attacker.example/video.mp4"}}
            ]}}
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::Veo, &server.uri());
    let error = router
        .video_content(&format!("p:{operation}"))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidResponse { .. }));
}

#[tokio::test]
async fn job_ids_that_rewrite_the_request_path_are_rejected() {
    let server = MockServer::start().await;
    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let error = router.video_status("p:../models").await.unwrap_err();
    assert!(matches!(error, Error::InvalidConfig(_)));

    let veo = single_provider_router(ProtocolKind::Veo, &server.uri());
    let error = veo.video_status("p:models/../../../etc").await.unwrap_err();
    assert!(matches!(error, Error::InvalidConfig(_)));
    assert!(server.received_requests().await.unwrap().is_empty());
}
