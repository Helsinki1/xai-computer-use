use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::Json;
use axum::http::StatusCode;
use axum::response::sse::Sse;
use axum::routing::post;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::stream;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use xai_grok_sampler::{
    ApiBackend, ProtectedOverlayAck, RequestId, RetryPolicy, SamplerActor, SamplerConfig,
};
use xai_grok_sampling_types::{ContentPart, ConversationItem, ConversationRequest, UserItem};
use xai_grok_test_support::sse;

const SNAPSHOT_ID: &str = "private-snapshot-correlation";
const OBSERVATION: &str = "Window=Fixture\n[7] AXButton title=Save frame=(10,20,40,20)";
const ONE_PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAX+XDSwAAAABJRU5ErkJggg==";

struct MockServer {
    addr: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
}

impl MockServer {
    async fn spawn(app: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Self { addr, shutdown_tx }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }
}

fn request() -> ConversationRequest {
    ConversationRequest {
        items: vec![ConversationItem::User(UserItem {
            content: vec![ContentPart::Text {
                text: Arc::<str>::from("inspect the app"),
            }],
            ..Default::default()
        })],
        ..Default::default()
    }
}

fn png_and_hash() -> (Vec<u8>, String) {
    let png = STANDARD.decode(ONE_PIXEL_PNG).unwrap();
    let hash = format!("{:x}", Sha256::digest(&png));
    (png, hash)
}

fn config(base_url: String, backend: ApiBackend) -> SamplerConfig {
    SamplerConfig {
        api_key: Some("test-key".into()),
        base_url,
        model: "test-model".into(),
        api_backend: backend,
        max_retries: Some(4),
        idle_timeout_secs: Some(30),
        ..SamplerConfig::default()
    }
}

fn route_for(backend: &ApiBackend) -> &'static str {
    match backend {
        ApiBackend::ChatCompletions => "/v1/chat/completions",
        ApiBackend::Responses => "/v1/responses",
        ApiBackend::Messages => "/v1/messages",
    }
}

fn strings<'a>(value: &'a Value, output: &mut Vec<&'a str>) {
    match value {
        Value::String(value) => output.push(value),
        Value::Array(values) => {
            for value in values {
                strings(value, output);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                strings(value, output);
            }
        }
        _ => {}
    }
}

async fn exercise_backend(backend: ApiBackend) {
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured_for_handler = Arc::clone(&captured);
    let backend_for_handler = backend.clone();
    let app = Router::new().route(
        route_for(&backend),
        post(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured_for_handler);
            let backend = backend_for_handler.clone();
            async move {
                captured.lock().unwrap().push(body);
                let events = match backend {
                    ApiBackend::ChatCompletions => {
                        sse::chat_completion_events("done", "test-model")
                    }
                    ApiBackend::Responses => sse::responses_api_events("done", "test-model"),
                    ApiBackend::Messages => {
                        sse::messages_api_events("done", "test-model", "end_turn")
                    }
                };
                Sse::new(stream::iter(events.into_iter().map(Ok::<_, Infallible>)))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        config(server.base_url(), backend.clone()),
        RetryPolicy::default(),
        event_tx,
    );
    let (png, hash) = png_and_hash();
    let encoded = STANDARD.encode(&png);
    let overlay = handle
        .attest_protected_overlay(SNAPSHOT_ID, OBSERVATION, png, &hash, 1, 1)
        .expect("overlay attests");

    let (ack, result) = handle
        .submit_and_collect_protected(RequestId::from("protected"), request(), overlay)
        .await;
    result.expect("inference succeeds");
    let receipt = match ack {
        ProtectedOverlayAck::Attached(receipt) => receipt,
        ProtectedOverlayAck::NotAttached => panic!("protected body was not acknowledged"),
    };
    assert!(receipt.matches_attestation(SNAPSHOT_ID, &hash));
    assert_eq!(receipt.pixel_dimensions(), (1, 1));

    let bodies = captured.lock().unwrap();
    assert_eq!(bodies.len(), 1, "protected submission sends one request");
    let mut body_strings = Vec::new();
    strings(&bodies[0], &mut body_strings);
    let carrier = match backend {
        ApiBackend::Messages => encoded,
        ApiBackend::ChatCompletions | ApiBackend::Responses => {
            format!("data:image/png;base64,{encoded}")
        }
    };
    assert_eq!(
        body_strings
            .iter()
            .filter(|value| **value == carrier)
            .count(),
        1,
        "attested PNG carrier must occur exactly once"
    );
    let protected_text = body_strings
        .iter()
        .find(|value| value.contains("<computer_use_screenshot>"))
        .expect("protected model-only text present");
    assert!(protected_text.contains(SNAPSHOT_ID));
    assert!(protected_text.contains(OBSERVATION));
    assert!(protected_text.contains("continuous PNG edge-space"));
    assert!(protected_text.contains("x in [0,1)"));

    let _ = server.shutdown_tx.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_png_is_attested_once_in_chat_body() {
    exercise_backend(ApiBackend::ChatCompletions).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_png_is_attested_once_in_responses_body() {
    exercise_backend(ApiBackend::Responses).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_png_is_attested_once_in_messages_body() {
    exercise_backend(ApiBackend::Messages).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_overlay_disables_retry_after_dispatch() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_handler = Arc::clone(&attempts);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(_body): Json<Value>| {
            let attempts = Arc::clone(&attempts_for_handler);
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": {"message": "retryable"}})),
                )
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        config(server.base_url(), ApiBackend::ChatCompletions),
        RetryPolicy::default(),
        event_tx,
    );
    let (png, hash) = png_and_hash();
    let overlay = handle
        .attest_protected_overlay(SNAPSHOT_ID, OBSERVATION, png, &hash, 1, 1)
        .expect("overlay attests");

    let (ack, result) = handle
        .submit_and_collect_protected(RequestId::from("no-retry"), request(), overlay)
        .await;
    assert!(result.is_err());
    match ack {
        ProtectedOverlayAck::Attached(receipt) => {
            assert!(receipt.matches_attestation(SNAPSHOT_ID, &hash));
        }
        ProtectedOverlayAck::NotAttached => panic!("body build should be acknowledged"),
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    let _ = server.shutdown_tx.send(());
}

#[tokio::test]
async fn overlay_from_another_handle_is_rejected_before_dispatch() {
    let source = xai_grok_sampler::SamplerHandle::noop();
    let target = xai_grok_sampler::SamplerHandle::noop();
    let (png, hash) = png_and_hash();
    let overlay = source
        .attest_protected_overlay(SNAPSHOT_ID, OBSERVATION, png, &hash, 1, 1)
        .expect("overlay attests");

    let (ack, result) = target
        .submit_and_collect_protected(RequestId::from("wrong-capability"), request(), overlay)
        .await;
    assert!(matches!(ack, ProtectedOverlayAck::NotAttached));
    assert!(result.is_err());
}
