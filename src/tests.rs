//! STAR Randomness web service tests

use crate::state::OPRFServer;
use axum::body::Body;
use axum::body::Bytes;
use axum::http::Request;
use axum::http::StatusCode;
use base64::prelude::{Engine as _, BASE64_STANDARD as BASE64};
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use rand::rngs::OsRng;
use serde_json::{json, Value};
use std::time::Duration;
use time::OffsetDateTime;
use tower::Service;
use tower::ServiceExt;

const EPOCH: u8 = 12;
const NEXT_EPOCH_TIME: &str = "2023-03-22T21:46:35Z";

struct InstanceConfig {
    instance_name: String,
    epoch_duration: String,
}

/// Create an app instance for testing
fn test_app(instance_configs: Option<Vec<InstanceConfig>>) -> crate::Router {
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
    };
    // server state
    let oprf_state = OPRFServer::new(&config);
    for instance in oprf_state.instances.values() {
        instance.write().unwrap().next_epoch_time = Some(NEXT_EPOCH_TIME.to_owned());
    }

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
    let app = test_app(None);

    let request = test_request("/", None);
    let response = app.oneshot(request).await.unwrap();

    // Root should return some identifying text for friendliness.
    assert_eq!(response.status(), StatusCode::OK);
    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    let message = std::str::from_utf8(body.as_ref()).unwrap();
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
    ]));

    let response = app.call(test_request("/info", None)).await.unwrap();

    // Info should return the correct epoch, etc.
    let default_public_key = validate_info_response_and_return_public_key_b64(
        response.status(),
        hyper::body::to_bytes(response.into_body()).await.unwrap(),
    );

    let response = app
        .call(test_request("/instances/main/info", None))
        .await
        .unwrap();
    let specific_default_public_key = validate_info_response_and_return_public_key_b64(
        response.status(),
        hyper::body::to_bytes(response.into_body()).await.unwrap(),
    );
    assert_eq!(default_public_key, specific_default_public_key);

    let response = app
        .call(test_request("/instances/alternate/info", None))
        .await
        .unwrap();
    let alternate_public_key = validate_info_response_and_return_public_key_b64(
        response.status(),
        hyper::body::to_bytes(response.into_body()).await.unwrap(),
    );
    assert_ne!(default_public_key, alternate_public_key);

    let response = app
        .call(test_request("/instances/notexisting/info", None))
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
    ]));

    // Create a single-point randomness request.
    let point = RistrettoPoint::random(&mut OsRng);
    let payload = json!({ "points": [
        BASE64.encode(point.compress().as_bytes())
    ]})
    .to_string();

    // Submit to the hander.
    let request = test_request("/randomness", Some(payload.clone()));
    let response = app.call(request).await.unwrap();
    // Verify we receive a successful, well-formed response.
    assert_eq!(response.status(), StatusCode::OK);
    let default_body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    verify_randomness_body(&default_body, 1);

    let response = app
        .call(test_request(
            "/instances/main/randomness",
            Some(payload.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let specific_default_body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    assert_eq!(default_body, specific_default_body);

    let response = app
        .call(test_request(
            "/instances/alternate/randomness",
            Some(payload.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let alternate_body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    verify_randomness_body(&alternate_body, 1);
    assert_ne!(default_body, alternate_body);

    let response = app
        .call(test_request(
            "/instances/notexisting/randomness",
            Some(payload),
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
    let request = test_request("/randomness", Some(payload));
    let response = test_app(None).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    verify_randomness_body(&body, points.len());

    // Verify earlier epochs are rejected.
    assert!(EPOCH > 0);
    let payload = json!({
        "points": points,
        "epoch": 0
    })
    .to_string();
    let request = test_request("/randomness", Some(payload));
    let response = test_app(None).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Verify later epochs are rejected.
    let payload = json!({
        "points": points,
        "epoch": EPOCH + 1
    })
    .to_string();
    let request = test_request("/randomness", Some(payload));
    let response = test_app(None).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// If --epoch-base-time is set, confirm the server starts
/// with the correct epoch.
#[tokio::test]
async fn epoch_base_time() {
    let now = OffsetDateTime::now_utc();
    let delay = Duration::from_secs(5);

    // Config with explicit base time
    let config = crate::Config {
        listen: "127.0.0.1:8081".to_string(),
        epoch_durations: vec!["1s".into()],
        first_epoch: EPOCH,
        last_epoch: EPOCH * 2,
        epoch_base_time: Some(now - delay),
        increase_nofile_limit: false,
        prometheus_listen: None,
        instance_names: vec!["main".to_string()],
    };
    // Verify test parameters are compatible with the
    // expected_epoch calculation.
    assert!(EPOCH as u64 + delay.as_secs() < EPOCH as u64 * 2);
    let expected_epoch = EPOCH + delay.as_secs() as u8;
    let advance = Duration::from_secs(1);
    let expected_time = (now + advance)
        // Published timestamp is truncated to the second.
        .replace_millisecond(0)
        .expect("should be able to truncate to a fixed ms")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("well-known timestamp format should always succeed");

    // server state
    let oprf_state = OPRFServer::new(&config);
    // background task to manage epoch rotation
    oprf_state.start_background_tasks(&config);

    // Wait for `epoch_loop` to update `next_epoch_time` as a proxy
    // for completing epoch schedule initialization. Use a timeout
    // to avoid hanging test runs.
    let pause = Duration::from_millis(10);
    let mut tries = 0;
    let oprf_instance = oprf_state.instances.get("main").unwrap();
    while oprf_instance.read().unwrap().next_epoch_time.is_none() {
        println!("waiting for {pause:?} for initialization {tries}");
        assert!(tries < 10, "timeout waiting for epoch_loop initialization");
        tokio::time::sleep(pause).await;
        tries += 1;
    }

    // attach axum routes and middleware
    let app = crate::app(oprf_state);

    let request = test_request("/info", None);
    let response = app.oneshot(request).await.unwrap();

    // Info should return the correct epoch, etc.
    assert_eq!(response.status(), StatusCode::OK);
    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
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
    let app = test_app(None);
    let payload = json!({ "points": points }).to_string();
    let request = test_request("/randomness", Some(payload));
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
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
    let request = test_request("/randomness", Some(payload));
    let response = test_app(None).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
