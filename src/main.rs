//! STAR Randomness web service

use axum::{routing::get, routing::post, Router};
use axum_prometheus::PrometheusMetricLayer;
use clap::Parser;
use metrics_exporter_prometheus::PrometheusHandle;
use rlimit::Resource;
use std::sync::{Arc, RwLock};
use tikv_jemallocator::Jemalloc;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tracing::{debug, info, metadata::LevelFilter};
use tracing_subscriber::EnvFilter;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

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
    #[arg(long, value_name = "RFC 3339 timestamp", value_parser = parse_timestamp)]
    epoch_base_time: Option<OffsetDateTime>,
    /// Increases OS nofile limit to 65535, so the server can handle
    /// more concurrent connections.
    #[arg(long, default_value_t = false)]
    increase_nofile_limit: bool,
    /// Enable prometheus metric reporting and listen on specified address.
    #[arg(long)]
    prometheus_listen: Option<String>,
}

/// Parse a timestamp given as a config option
fn parse_timestamp(stamp: &str) -> Result<OffsetDateTime, &'static str> {
    OffsetDateTime::parse(stamp, &Rfc3339).map_err(|_| "Try something like '2023-05-15T04:30:00Z'.")
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

fn start_prometheus_server(metrics_handle: PrometheusHandle, listen: String) {
    tokio::spawn(async move {
        let addr = listen.parse().unwrap();
        let metrics_app =
            Router::new().route("/metrics", get(|| async move { metrics_handle.render() }));
        info!("Metrics server listening on {}", &listen);
        axum::Server::bind(&addr)
            .serve(metrics_app.into_make_service())
            .await
            .unwrap();
    });
}

fn increase_nofile_limit() {
    let curr_limits =
        rlimit::getrlimit(Resource::NOFILE).expect("should be able to get current nofile limit");
    info!("Current nofile limits = {:?}", curr_limits);

    rlimit::setrlimit(Resource::NOFILE, 65535, 65535).expect("should be able to set nofile limit");
    let curr_limits = rlimit::getrlimit(Resource::NOFILE)
        .expect("should be able to get current nofile limit after updating it");
    info!(
        "Attempted nofile limit change! Current nofile limits = {:?}",
        curr_limits
    );
}

#[tokio::main]
async fn main() {
    // Start logging
    // The default subscriber respects filter directives like `RUST_LOG=info`
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env()
                .unwrap(),
        )
        .init();
    info!("STARing up!");

    // Command line switches
    let config = Config::parse();
    debug!(?config, "config parsed");
    let addr = config.listen.parse().unwrap();

    if config.increase_nofile_limit {
        increase_nofile_limit();
    }

    // Oblivious function state
    info!("initializing OPRF state...");
    let server = state::OPRFServer::new(&config).expect("Could not initialize PPOPRF state");
    info!("epoch now {}", server.epoch);
    let oprf_state = Arc::new(RwLock::new(server));

    let metric_layer = config.prometheus_listen.as_ref().map(|listen| {
        let (layer, handle) = PrometheusMetricLayer::pair();
        start_prometheus_server(handle, listen.clone());
        layer
    });

    // Spawn a background process to advance the epoch
    info!("Spawning background epoch rotation task...");
    let background_state = oprf_state.clone();
    tokio::spawn(async move { state::epoch_loop(background_state, &config).await });

    // Set up routes and middleware
    info!("initializing routes...");
    let mut app = app(oprf_state);
    if let Some(metric_layer) = metric_layer {
        app = app.layer(metric_layer);
    }

    // Start the server
    info!("Listening on {}", &addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
