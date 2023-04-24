//! STAR Randomness web service tests

use crate::state::OPRFServer;
use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use base64::prelude::{Engine as _, BASE64_STANDARD as BASE64};
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};
use tower::ServiceExt;

const EPOCH: u8 = 12;
const NEXT_EPOCH_TIME: &str = "2023-03-22T21:46:35Z";

/// Create an app instance for testing
fn test_app() -> crate::Router {
    // arbitrary config
    let config = crate::Config {
        epoch_seconds: 1,
        first_epoch: EPOCH,
        last_epoch: EPOCH * 2,
        listen: "127.0.0.1:8081".to_string(),
    };
    // server state
    let mut server =
        OPRFServer::new(&config).expect("Could not initialize PPOPRF state");
    server.next_epoch_time = Some(NEXT_EPOCH_TIME.to_owned());
    let oprf_state = Arc::new(RwLock::new(server));

    // attach axum routes and middleware
    crate::app(oprf_state)
}

/// Create a request for testing
fn test_request(uri: &str, payload: Option<String>) -> Request<Body> {
    let builder = Request::builder().uri(uri);
    let request = match payload {
        Some(json) => {
            // POST payload body as json
            builder
                .method("POST")
                .header("Content-Type", "application/json")
                .body(json.into())
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
    let app = test_app();

    let request = test_request("/", None);
    let response = app.oneshot(request).await.unwrap();

    // Root should return some identifying text for friendliness.
    assert_eq!(response.status(), StatusCode::OK);
    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    let message = std::str::from_utf8(body.as_ref()).unwrap();
    assert!(!message.is_empty());
}

#[tokio::test]
async fn info() {
    let app = test_app();

    let request = test_request("/info", None);
    let response = app.oneshot(request).await.unwrap();

    // Info should return the correct epoch, etc.
    assert_eq!(response.status(), StatusCode::OK);
    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    assert!(!body.is_empty());
    let json: Value = serde_json::from_slice(body.as_ref())
        .expect("Could not parse response body as json");
    assert!(json.is_object());
    println!("{:?}", json);
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
}

#[tokio::test]
async fn randomness() {
    let app = test_app();

    // Create a single-point randomness request.
    let point = RistrettoPoint::random(&mut rand_core::OsRng);
    let payload = json!({ "points": [
        BASE64.encode(point.compress().as_bytes())
    ]})
    .to_string();
    println!("request body {payload:?}");

    // Submit to the hander.
    let request = test_request("/randomness", Some(payload));
    println!("request {request:?}");
    let response = app.oneshot(request).await.unwrap();
    println!("response {response:?}");
    // Verify we receive a successful, well-formed response.
    assert_eq!(response.status(), StatusCode::OK);
    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    verify_randomness_body(body, 1);
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
    let request = test_request("/randomness", Some(payload));
    let response = test_app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    verify_randomness_body(body, points.len());

    // Verify earlier epochs are rejected.
    assert!(EPOCH > 0);
    let payload = json!({
        "points": points,
        "epoch": 0
    })
    .to_string();
    let request = test_request("/randomness", Some(payload));
    let response = test_app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Verify later epochs are rejected.
    let payload = json!({
        "points": points,
        "epoch": EPOCH + 1
    })
    .to_string();
    let request = test_request("/randomness", Some(payload));
    let response = test_app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// Check a randomness response body for validity
fn verify_randomness_body(body: axum::body::Bytes, expected_points: usize) {
    // Randomness should return a list of points and an epoch.
    assert!(!body.is_empty());
    let json: Value = serde_json::from_slice(body.as_ref())
        .expect("Response body should parse as json");
    // Top-level value should be an object.
    assert!(json.is_object());
    // Epoch should match test_app.
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
        let point = RistrettoPoint::random(&mut rand_core::OsRng);
        let b64point = BASE64.encode(point.compress().as_bytes());
        points.push(b64point);
    }
    points
}

/// Verify randomness response to a batch of points
async fn verify_batch(points: &[String]) {
    let app = test_app();
    let payload = json!({ "points": points }).to_string();
    let request = test_request("/randomness", Some(payload));
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    verify_randomness_body(body, points.len());
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
    let request = test_request("/randomness", Some(payload));
    let response = test_app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
