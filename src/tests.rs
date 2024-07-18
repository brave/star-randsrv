//! STAR Randomness web service tests

use crate::state::{KeyInfoRef, OPRFKeys, OPRFKeysRef, OPRFServer};
use axum::body::{to_bytes, Body, Bytes};
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::{Method, Request};
use axum::routing::put;
use axum::Router;
use base64::prelude::{Engine as _, BASE64_STANDARD as BASE64};
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use rand::rngs::OsRng;
use serde_json::{json, Value};
use std::time::Duration;
use time::OffsetDateTime;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tower::Service;
use tower::ServiceExt;

/// Test values
const EPOCH: u8 = 12;
const NEXT_EPOCH_TIME: &str = "2023-03-22T21:46:35Z";

/// Maximum size of a response body to consider
/// This is an approximate bound to allow for crate::MAX_POINTS.
/// The exact size is 32 bytes per point, plus base64 and json overhead.
const RESPONSE_MAX: usize = 48 * 1024;

struct InstanceConfig {
    instance_name: String,
    epoch_duration: String,
}

/// Create an app instance for testing
async fn test_app(instance_configs: Option<Vec<InstanceConfig>>) -> crate::Router {
    let instance_configs = instance_configs.unwrap_or(vec![InstanceConfig {
        instance_name: "main".to_string(),
        epoch_duration: "1s".to_string(),
    }]);
    // arbitrary config
    let config = crate::Config {
        listen: "127.0.0.1:8081".to_string(),
        epoch_durations: instance_configs
            .iter()
            .map(|c| c.epoch_duration.as_str().into())
            .collect(),
        first_epoch: EPOCH,
        last_epoch: EPOCH * 2,
        epoch_base_time: None,
        increase_nofile_limit: false,
        prometheus_listen: None,
        instance_names: instance_configs
            .into_iter()
            .map(|c| c.instance_name)
            .collect(),
        enclave_key_sync: false,
        nitriding_internal_port: None,
    };
    // server state
    let oprf_state = OPRFServer::new(config.clone()).await;

    for instance in oprf_state.instances.values() {
        instance.write().await.as_mut().unwrap().next_epoch_time = NEXT_EPOCH_TIME.to_string();
    }
    // attach axum routes and middleware
    crate::app(&config, oprf_state)
}

/// Create a request for testing
fn test_request(uri: &str, payload: Option<Body>, method: Option<Method>) -> Request<Body> {
    let builder = Request::builder().uri(uri);
    let request = match payload {
        Some(payload) => {
            // POST payload body as json
            builder
                .method(method.unwrap_or(Method::POST))
                .header("Content-Type", "application/json")
                .body(payload)
        }
        None => {
            // regular GET request
            builder.body(Body::empty())
        }
    };
    request.unwrap()
}

#[tokio::test]
async fn welcome() {
    let app = test_app(None).await;

    let request = test_request("/", None, None);
    let response = app.oneshot(request).await.unwrap();

    // Root should return some identifying text for friendliness.
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), RESPONSE_MAX).await.unwrap();
    let message = std::str::from_utf8(&body).unwrap();
    assert!(!message.is_empty());
}

fn validate_info_response_and_return_public_key_b64(status: StatusCode, body: Bytes) -> String {
    assert_eq!(status, StatusCode::OK);
    assert!(!body.is_empty());
    let json: Value =
        serde_json::from_slice(body.as_ref()).expect("Could not parse response body as json");
    assert!(json.is_object());
    assert_eq!(json["currentEpoch"], json!(EPOCH));
    assert!(json["nextEpochTime"].is_string());
    let next_epoch_time = json["nextEpochTime"].as_str().unwrap();
    assert_eq!(next_epoch_time, NEXT_EPOCH_TIME);
    assert!(json["maxPoints"].is_number());
    let max_points = json["maxPoints"].as_u64().unwrap();
    assert_eq!(max_points, crate::MAX_POINTS as u64);
    assert!(json["publicKey"].is_string());
    let b64key = json["publicKey"].as_str().unwrap();
    let binkey = BASE64.decode(b64key).unwrap();
    let _ = ppoprf::ppoprf::ServerPublicKey::load_from_bincode(&binkey)
        .expect("Could not parse server public key");
    b64key.to_string()
}

#[tokio::test]
async fn info() {
    let mut app = test_app(Some(vec![
        InstanceConfig {
            instance_name: "main".to_string(),
            epoch_duration: "1s".to_string(),
        },
        InstanceConfig {
            instance_name: "alternate".to_string(),
            epoch_duration: "1s".to_string(),
        },
    ]))
    .await;

    let response = app.call(test_request("/info", None, None)).await.unwrap();

    // Info should return the correct epoch, etc.
    let default_public_key = validate_info_response_and_return_public_key_b64(
        response.status(),
        to_bytes(response.into_body(), RESPONSE_MAX).await.unwrap(),
    );

    let response = app
        .call(test_request("/instances/main/info", None, None))
        .await
        .unwrap();
    let specific_default_public_key = validate_info_response_and_return_public_key_b64(
        response.status(),
        to_bytes(response.into_body(), RESPONSE_MAX).await.unwrap(),
    );
    assert_eq!(default_public_key, specific_default_public_key);

    let response = app
        .call(test_request("/instances/alternate/info", None, None))
        .await
        .unwrap();
    let alternate_public_key = validate_info_response_and_return_public_key_b64(
        response.status(),
        to_bytes(response.into_body(), RESPONSE_MAX).await.unwrap(),
    );
    assert_ne!(default_public_key, alternate_public_key);

    let response = app
        .call(test_request("/instances/notexisting/info", None, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn randomness() {
    let mut app = test_app(Some(vec![
        InstanceConfig {
            instance_name: "main".to_string(),
            epoch_duration: "1s".to_string(),
        },
        InstanceConfig {
            instance_name: "alternate".to_string(),
            epoch_duration: "1s".to_string(),
        },
    ]))
    .await;

    // Create a single-point randomness request.
    let point = RistrettoPoint::random(&mut OsRng);
    let payload = json!({ "points": [
        BASE64.encode(point.compress().as_bytes())
    ]})
    .to_string();

    // Submit to the hander.
    let request = test_request("/randomness", Some(payload.clone().into()), None);
    let response = app.call(request).await.unwrap();
    // Verify we receive a successful, well-formed response.
    assert_eq!(response.status(), StatusCode::OK);
    let default_body = to_bytes(response.into_body(), RESPONSE_MAX).await.unwrap();
    verify_randomness_body(&default_body, 1);

    let response = app
        .call(test_request(
            "/instances/main/randomness",
            Some(payload.clone().into()),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let specific_default_body = to_bytes(response.into_body(), RESPONSE_MAX).await.unwrap();
    assert_eq!(default_body, specific_default_body);

    let response = app
        .call(test_request(
            "/instances/alternate/randomness",
            Some(payload.clone().into()),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let alternate_body = to_bytes(response.into_body(), RESPONSE_MAX).await.unwrap();
    verify_randomness_body(&alternate_body, 1);
    assert_ne!(default_body, alternate_body);

    let response = app
        .call(test_request(
            "/instances/notexisting/randomness",
            Some(payload.into()),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[allow(clippy::assertions_on_constants)]
async fn epoch() {
    let points = make_points(3);

    // Verify setting the epoch is accepted.
    let payload = json!({
        "points": points,
        "epoch": EPOCH
    })
    .to_string();
    let request = test_request("/randomness", Some(payload.into()), None);
    let response = test_app(None).await.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), RESPONSE_MAX).await.unwrap();
    verify_randomness_body(&body, points.len());

    // Verify earlier epochs are rejected.
    assert!(EPOCH > 0);
    let payload = json!({
        "points": points,
        "epoch": 0
    })
    .to_string();
    let request = test_request("/randomness", Some(payload.into()), None);
    let response = test_app(None).await.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Verify later epochs are rejected.
    let payload = json!({
        "points": points,
        "epoch": EPOCH + 1
    })
    .to_string();
    let request = test_request("/randomness", Some(payload.into()), None);
    let response = test_app(None).await.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// If --epoch-base-time is set, confirm the server starts
/// with the correct epoch.
#[tokio::test]
async fn epoch_base_time() {
    let now = OffsetDateTime::now_utc()
        .replace_millisecond(0)
        .expect("should be able to truncate to a fixed ms");
    let delay = Duration::from_secs(11);

    // Config with explicit base time
    let config = crate::Config {
        listen: "127.0.0.1:8081".to_string(),
        epoch_durations: vec!["10s".into()],
        first_epoch: EPOCH,
        last_epoch: EPOCH * 2,
        epoch_base_time: Some(now - delay),
        increase_nofile_limit: false,
        prometheus_listen: None,
        instance_names: vec!["main".to_string()],
        enclave_key_sync: false,
        nitriding_internal_port: None,
    };
    let expected_epoch = EPOCH + 1;
    let advance = Duration::from_secs(9);
    let expected_time = (now + advance)
        .format(&time::format_description::well_known::Rfc3339)
        .expect("well-known timestamp format should always succeed");

    // server state
    let oprf_state = OPRFServer::new(config.clone()).await;

    // attach axum routes and middleware
    let app = crate::app(&config, oprf_state);

    let request = test_request("/info", None, None);
    let response = app.oneshot(request).await.unwrap();

    // Info should return the correct epoch, etc.
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), RESPONSE_MAX).await.unwrap();
    assert!(!body.is_empty());
    let json: Value =
        serde_json::from_slice(body.as_ref()).expect("Could not parse response body as json");
    assert!(json.is_object());
    assert_eq!(json["currentEpoch"], json!(expected_epoch));
    assert!(json["nextEpochTime"].is_string());
    let next_epoch_time = json["nextEpochTime"].as_str().unwrap();
    assert_eq!(next_epoch_time, expected_time);
}

/// Check a randomness response body for validity
fn verify_randomness_body(body: &Bytes, expected_points: usize) {
    // Randomness should return a list of points and an epoch.
    assert!(!body.is_empty());
    let json: Value =
        serde_json::from_slice(body.as_ref()).expect("Response body should parse as json");
    // Top-level value should be an object.
    assert!(json.is_object());
    // Epoch should match test_app.None
    let epoch = json["epoch"].as_u64().unwrap();
    assert_eq!(epoch, EPOCH as u64);
    // Points array should have the expected number of elements.
    let points = json["points"].as_array().unwrap();
    assert_eq!(points.len(), expected_points);
    // Individual elements should be parseable base64-encoded
    // compressed Ristretto elliptic curve points.
    for value in points {
        let b64point = value.as_str().unwrap();
        let rawpoint = BASE64.decode(b64point).unwrap();
        let _ = CompressedRistretto::from_slice(&rawpoint);
    }
}

/// Generate a number of random base64-encoded points.
fn make_points(count: usize) -> Vec<String> {
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let point = RistrettoPoint::random(&mut OsRng);
        let b64point = BASE64.encode(point.compress().as_bytes());
        points.push(b64point);
    }
    points
}

/// Verify randomness response to a batch of points
async fn verify_batch(points: &[String]) {
    let app = test_app(None).await;
    let payload = json!({ "points": points }).to_string();
    let request = test_request("/randomness", Some(payload.into()), None);
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), RESPONSE_MAX).await.unwrap();
    verify_randomness_body(&body, points.len());
}

#[tokio::test]
async fn point_batches() {
    // Check that we can submit multiple points.
    let points = make_points(5);
    verify_batch(&points).await;

    // Check that we can submit a reasonable number of points.
    let points = make_points(128);
    assert!(points.len() < crate::MAX_POINTS);
    verify_batch(&points).await;
}

#[tokio::test]
async fn max_points() {
    // Check that we can submit the maximum number of points.
    let points = make_points(crate::MAX_POINTS);
    verify_batch(&points).await;

    // Requests with more than the maximum number of points
    // should be rejected.
    let points = make_points(crate::MAX_POINTS + 1);
    let payload = json!({ "points": points }).to_string();
    let request = test_request("/randomness", Some(payload.into()), None);
    let response = test_app(None).await.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_enclave_leader() {
    let config = crate::Config {
        listen: "127.0.0.1:8082".to_string(),
        epoch_durations: vec!["1s".into(), "2s".into()],
        first_epoch: EPOCH,
        last_epoch: EPOCH * 2,
        epoch_base_time: None,
        increase_nofile_limit: false,
        prometheus_listen: None,
        instance_names: vec!["main".to_string(), "secondary".to_string()],
        enclave_key_sync: true,
        nitriding_internal_port: Some(8083),
    };

    let oprf_state = OPRFServer::new(config.clone()).await;

    assert!(oprf_state
        .instances
        .get("main")
        .unwrap()
        .read()
        .await
        .is_none());
    assert!(oprf_state
        .instances
        .get("secondary")
        .unwrap()
        .read()
        .await
        .is_none());
    assert!(!oprf_state.is_leader.initialized());

    let app = crate::app(&config, oprf_state.clone());

    let request = test_request("/enclave/state", None, None);
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), RESPONSE_MAX).await.unwrap();
    assert!(!body.is_empty());

    let private_keys: OPRFKeys =
        bincode::deserialize(&body).expect("Failed to deserialize private keys");

    assert_eq!(private_keys.len(), 2);

    for (instance_name, key_info) in private_keys.iter() {
        let instance = oprf_state.instances.get(instance_name).unwrap();
        let instance_guard = instance.read().await;
        let instance = instance_guard.as_ref().unwrap();

        assert_eq!(instance.epoch, key_info.epoch);
        assert_eq!(
            instance.server.get_private_key(),
            key_info.key_state.as_ref()
        );
    }

    assert_eq!(private_keys.len(), config.instance_names.len());
    for instance_name in config.instance_names.iter() {
        assert!(private_keys.contains_key(instance_name));
    }

    assert_eq!(oprf_state.is_leader.get(), Some(&true));
}

#[tokio::test]
async fn test_enclave_worker() {
    let config = crate::Config {
        listen: "127.0.0.1:8084".to_string(),
        epoch_durations: vec!["1s".into(), "2s".into()],
        first_epoch: EPOCH,
        last_epoch: EPOCH * 2,
        epoch_base_time: None,
        increase_nofile_limit: false,
        prometheus_listen: None,
        instance_names: vec!["main".to_string(), "secondary".to_string()],
        enclave_key_sync: true,
        nitriding_internal_port: Some(8085),
    };

    let oprf_state = OPRFServer::new(config.clone()).await;

    assert!(oprf_state
        .instances
        .get("main")
        .unwrap()
        .read()
        .await
        .is_none());
    assert!(oprf_state
        .instances
        .get("secondary")
        .unwrap()
        .read()
        .await
        .is_none());
    assert!(!oprf_state.is_leader.initialized());

    let mock_ppoprfs = config
        .instance_names
        .iter()
        .map(|instance_name| {
            (
                instance_name,
                ppoprf::ppoprf::Server::new((EPOCH..EPOCH * 2).collect()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let mock_keys = mock_ppoprfs
        .iter()
        .map(|(instance_name, server)| {
            (
                instance_name.to_string(),
                KeyInfoRef {
                    key_state: server.get_private_key(),
                    epoch: EPOCH,
                },
            )
        })
        .collect::<OPRFKeysRef>();

    let mock_keys_bytes = bincode::serialize(&mock_keys).expect("Failed to serialize mock keys");

    let app = crate::app(&config, oprf_state.clone());

    let request = test_request(
        "/enclave/state",
        Some(mock_keys_bytes.into()),
        Some(Method::PUT),
    );
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    assert_eq!(oprf_state.is_leader.get(), Some(&false));

    for (instance_name, key_info) in mock_keys.iter() {
        let instance = oprf_state.instances.get(instance_name).unwrap();
        let instance_guard = instance.read().await;
        let instance = instance_guard.as_ref().unwrap();

        assert_eq!(instance.epoch, key_info.epoch);
        assert_eq!(instance.server.get_private_key(), key_info.key_state);
    }
}

#[tokio::test]
async fn test_leader_updates_keys_with_nitriding() {
    let config = crate::Config {
        listen: "127.0.0.1:8085".to_string(),
        epoch_durations: vec!["1s".into()],
        first_epoch: EPOCH,
        last_epoch: EPOCH + 2,
        epoch_base_time: None,
        increase_nofile_limit: false,
        prometheus_listen: None,
        instance_names: vec!["main".to_string()],
        enclave_key_sync: true,
        nitriding_internal_port: Some(8087),
    };

    let (mock_server_handle, mut body_rx) = start_mock_nitriding_server(8087).await;

    let oprf_state = OPRFServer::new(config.clone()).await;

    let app = crate::app(&config, oprf_state.clone());

    let request = test_request("/enclave/state", None, None);
    app.oneshot(request).await.unwrap();

    assert!(body_rx.is_empty());

    sleep(Duration::from_secs(1)).await;

    let updated_body = body_rx.recv().await.unwrap();
    let updated_keys: OPRFKeys = bincode::deserialize(&updated_body).unwrap();

    assert_eq!(updated_keys.len(), 1);

    for (instance_name, key_info) in updated_keys {
        let instance = oprf_state.instances.get(&instance_name).unwrap();
        let instance_guard = instance.read().await;
        let instance = instance_guard.as_ref().unwrap();

        assert_eq!(instance.epoch, key_info.epoch);
        assert_eq!(
            instance.server.get_private_key(),
            key_info.key_state.as_ref()
        );
    }

    mock_server_handle.abort();
    mock_server_handle.await.ok();
}

async fn start_mock_nitriding_server(
    port: u16,
) -> (JoinHandle<()>, mpsc::UnboundedReceiver<Bytes>) {
    let (body_tx, body_rx) = mpsc::unbounded_channel();

    let app = Router::new()
        .route("/enclave/state", put(nitriding_put_state_handler))
        .with_state(body_tx);

    let handle = tokio::spawn(async move {
        let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        axum::serve(listener, app).await.unwrap();
    });

    (handle, body_rx)
}

async fn nitriding_put_state_handler(
    State(body_tx): State<mpsc::UnboundedSender<Bytes>>,
    body: Bytes,
) {
    body_tx.send(body).unwrap();
}
