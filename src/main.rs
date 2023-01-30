//! STAR Randomness web service

use axum::{Router, routing::get, routing::post};
use axum::extract::{Json, State};
use serde::{Serialize, Deserialize};
use tracing::{info, debug};

use std::sync::Arc;
use ppoprf::ppoprf;

#[derive(Deserialize, Debug)]
struct RandomnessRequest {
    points: Vec<String>,
}

#[derive(Serialize, Debug)]
struct RandomnessResponse {
    points: Vec<String>,
    epoch: u8,
}

/// Process PPOPRF evaluation requests
async fn randomness(
    Json(request): Json<RandomnessRequest>,
//    State(state): State<Arc<ppoprf::Server>>
) -> Json<RandomnessResponse> {
    debug!("recv: {request:?}");
    Json(RandomnessResponse{
        points: vec![],
        epoch: 0u8,
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
    let oprf_state = Arc::new(ppoprf::Server::new(epochs)
        .expect("Could not initialize PPOPRF state"));

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
