//! STAR Randomness web service

use axum::extract::{Json, State};
use axum::{routing::get, routing::post, Router};
use base64::prelude::{Engine as _, BASE64_STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use ppoprf::ppoprf;
use std::sync::{Arc, RwLock};

struct OPRFServer {
    server: ppoprf::Server,
    epoch: u8,
}

#[derive(Deserialize, Debug)]
struct RandomnessRequest {
    points: Vec<String>,
    epoch: Option<u8>,
}

#[derive(Serialize, Debug)]
struct RandomnessResponse {
    points: Vec<String>,
    epoch: u8,
}

/// Process PPOPRF evaluation requests
async fn randomness(
    State(state): State<Arc<RwLock<OPRFServer>>>,
    Json(request): Json<RandomnessRequest>,
) -> Json<RandomnessResponse> {
    debug!("recv: {request:?}");
    let state = state.read().unwrap();
    let epoch = request.epoch.unwrap_or(state.epoch);
    let prove = false;
    let points = request
        .points
        .into_iter()
        .map(|base64_input| BASE64.decode(base64_input).unwrap())
        .map(|input| ppoprf::Point::from(input.as_slice()))
        .map(|point| state.server.eval(&point, epoch, prove).unwrap())
        .map(|evaluation| BASE64.encode(evaluation.output.as_bytes()))
        .collect();
    Json(RandomnessResponse { points, epoch })
}

#[tokio::main]
async fn main() {
    // Start logging
    // The default subscriber respects filter directives like `RUST_LOG=info`
    //let filter = tracing_subscriber::EnvFilter::from_default_env();
    //let logger = tracing_subscriber::FmtSubscriber::new();
    //tracing::subscriber::set_global_default(logger).unwrap();
    tracing_subscriber::fmt::init();
    info!("Staring up!");

    // Obvlivious function state
    info!("initializing OPRF state...");
    let epochs: Vec<u8> = (0..255).collect();
    let epoch = epochs[0];
    let server = ppoprf::Server::new(epochs)
        .expect("Could not initialize PPOPRF state");
    let oprf_state = Arc::new(RwLock::new(OPRFServer { server, epoch }));

    // Set up routes and middleware
    info!("initializing routes...");
    let app = Router::new()
        // Friendly default route to identify the site
        .route("/", get(|| async { "STAR randomness server\n" }))
        // Main endpoint
        .route("/randomness", post(randomness))
        .with_state(oprf_state)
        // Logging must come after active routes
        .layer(tower_http::trace::TraceLayer::new_for_http());

    // Start the server
    let addr = "127.0.0.1:8080".parse().unwrap();
    info!("Listening on {}", &addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
