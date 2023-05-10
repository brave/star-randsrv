//! STAR Randomness web service

use axum::{routing::get, routing::post, Router};
use clap::Parser;
use std::sync::{Arc, RwLock};
use tracing::{debug, info};

mod handler;
mod state;

pub use state::OPRFState;

#[cfg(test)]
mod tests;

/// Maximum number of points acceptable in a single request
const MAX_POINTS: usize = 1024;

/// Command line switches
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Config {
    /// Host and port to listen for http connections
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: String,
    /// Duration of each randomness epoch
    #[arg(long, default_value_t = 5)]
    epoch_seconds: u32,
    /// First epoch tag to make available
    #[arg(long, default_value_t = 0)]
    first_epoch: u8,
    /// Last epoch tag to make available
    #[arg(long, default_value_t = 255)]
    last_epoch: u8,
    /// Optional absolute time at which to anchor the first epoch
    /// This can be used to align the epoch sequence across different
    /// invocations.
    #[arg(long, value_name = "RFC 3339 timestamp")]
    epoch_base_time: Option<String>,
}

/// Initialize an axum::Router for our web service
/// Having this as a separate function makes testing easier.
fn app(oprf_state: OPRFState) -> Router {
    Router::new()
        // Friendly default route to identify the site
        .route("/", get(|| async { "STAR randomness server\n" }))
        // Main endpoints
        .route("/randomness", post(handler::randomness))
        .route("/info", get(handler::info))
        // Attach shared state
        .with_state(oprf_state)
        // Logging must come after active routes
        .layer(tower_http::trace::TraceLayer::new_for_http())
}

#[tokio::main]
async fn main() {
    // Start logging
    // The default subscriber respects filter directives like `RUST_LOG=info`
    tracing_subscriber::fmt::init();
    info!("STARing up!");

    // Command line switches
    let config = Config::parse();
    debug!(?config, "config parsed");
    let addr = config.listen.parse().unwrap();

    // Oblivious function state
    info!("initializing OPRF state...");
    let server = state::OPRFServer::new(&config)
        .expect("Could not initialize PPOPRF state");
    info!("epoch now {}", server.epoch);
    let oprf_state = Arc::new(RwLock::new(server));

    // Spawn a background process to advance the epoch
    info!("Spawning background epoch rotation task...");
    let background_state = oprf_state.clone();
    tokio::spawn(async move {
        state::epoch_loop(background_state, &config).await
    });

    // Set up routes and middleware
    info!("initializing routes...");
    let app = app(oprf_state);

    // Start the server
    info!("Listening on {}", &addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
