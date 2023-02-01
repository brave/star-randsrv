//! STAR Randomness web service

use axum::{Router, routing::get, routing::post};
use axum::extract::{Json, State};
use base64::prelude::{Engine as _, BASE64_STANDARD as BASE64};
use serde::{Serialize, Deserialize};
use tracing::{info, debug};

use std::sync::{Arc, Mutex};
use ppoprf::ppoprf;

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
    State(state): State<Arc<Mutex<OPRFServer>>>,
    Json(request): Json<RandomnessRequest>,
) -> Json<RandomnessResponse> {
    debug!("recv: {request:?}");
    let state = state.lock().unwrap();
    let epoch = request.epoch.unwrap_or(state.epoch);
    let point = BASE64.decode(&request.points[0]).unwrap();
    let point = ppoprf::Point::from(point.as_slice());
    let point = state.server.eval(&point, epoch, false).unwrap();
    let point = BASE64.encode(point.output.as_bytes());
    Json(RandomnessResponse{
        points: vec![point],
        epoch,
    })

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
    let oprf_state = Arc::new(Mutex::new(OPRFServer {
        server,
        epoch,
    }));

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
