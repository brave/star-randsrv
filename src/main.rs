//! STAR Randomness web service

use axum::{routing::get, routing::post, Router};
use axum_prometheus::PrometheusMetricLayer;
use calendar_duration::CalendarDuration;
use clap::Parser;
use metrics_exporter_prometheus::PrometheusHandle;
use rlimit::Resource;
use state::{OPRFServer, OPRFState};
use tikv_jemallocator::Jemalloc;
use time::OffsetDateTime;
use tracing::{debug, info, metadata::LevelFilter};
use tracing_subscriber::EnvFilter;
use util::{assert_unique_names, parse_timestamp};

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

mod handler;
mod state;
mod util;

#[cfg(test)]
mod tests;

/// Maximum number of points acceptable in a single request
const MAX_POINTS: usize = 1024;

/// Command line switches
#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct Config {
    /// Host and port to listen for http connections
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: String,
    /// Name of OPRF instance contained in server. Multiple instances may be defined
    /// by defining this switch multiple times. The first defined instance will
    /// become the default instance.
    #[arg(long = "instance-name", default_value = "main")]
    instance_names: Vec<String>,
    /// Duration of each randomness epoch. This switch may be defined multiple times
    /// to set the epoch length for each respective instance, if multiple instances
    /// are defined.
    #[arg(long = "epoch-duration", value_name = "Duration string i.e. 1mon5h2s", default_values = ["5s"])]
    epoch_durations: Vec<CalendarDuration>,
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

/// Initialize an axum::Router for our web service
/// Having this as a separate function makes testing easier.
fn app(oprf_state: OPRFState) -> Router {
    Router::new()
        // Friendly default route to identify the site
        .route("/", get(|| async { "STAR randomness server\n" }))
        // Endpoints for all instances
        .route(
            "/instances/:instance/randomness",
            post(handler::specific_instance_randomness),
        )
        .route(
            "/instances/:instance/info",
            get(handler::specific_instance_info),
        )
        .route("/instances", get(handler::list_instances))
        // Endpoints for default instance
        .route("/randomness", post(handler::default_instance_randomness))
        .route("/info", get(handler::default_instance_info))
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

    assert_unique_names(&config.instance_names);
    assert!(
        !config.epoch_durations.iter().any(|d| d.is_zero()),
        "all epoch lengths must be non-zero"
    );
    assert!(
        !config.instance_names.is_empty(),
        "at least one instance name must be defined"
    );
    assert!(
        config.instance_names.len() == config.epoch_durations.len(),
        "instance-name switch count must match epoch-seconds switch count"
    );

    let metric_layer = config.prometheus_listen.as_ref().map(|listen| {
        let (layer, handle) = PrometheusMetricLayer::pair();
        start_prometheus_server(handle, listen.clone());
        layer
    });

    let oprf_state = OPRFServer::new(&config);
    oprf_state.start_background_tasks(&config);

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
