//! STAR Randomness web service

use axum::extract::{Json, State};
use axum ::http::StatusCode;
use axum::{routing::get, routing::post, Router};
use base64::prelude::{Engine as _, BASE64_STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use ppoprf::ppoprf;
use std::sync::{Arc, RwLock};

/// Internal state of the OPRF service
struct OPRFServer {
    /// oprf implementation
    server: ppoprf::Server,
    /// currently-valid randomness epoch
    epoch: u8,
}

impl OPRFServer {
    /// Initialize a new OPRFServer state supporting the given list of epochs
    fn new(epochs: &[u8]) -> Result<Self, ppoprf::PPRFError> {
        let epoch = epochs[0];
        let server = ppoprf::Server::new(epochs.to_owned())?;
        Ok(OPRFServer{ server, epoch })
    }
}

/// Shareable wrapper around the server state
type OPRFState = Arc<RwLock<OPRFServer>>;

/// Request format for the randomness endpoint
#[derive(Deserialize, Debug)]
struct RandomnessRequest {
    /// Array of points to evaluate
    /// Should be base64-encoded compressed Ristretto curve points
    points: Vec<String>,
    /// Optional request for evaluation within a specific epoch
    epoch: Option<u8>,
}

/// Response format for the randomness endpoint
#[derive(Serialize, Debug)]
struct RandomnessResponse {
    /// Resulting points from the OPRF valuation
    /// Should be base64-encode compressed points in one-to-one
    /// correspondence with the request points.
    points: Vec<String>,
    /// Randomness epoch used in the evaluation.
    epoch: u8,
}

/// Response returned to report error conditions
#[derive(Serialize, Debug)]
struct ErrorResponse {
    /// Human-readable description of the error
    message: String,
}

/// Server error conditions
#[derive(Debug)]
enum Error {
    LockFailure,
    BadPoint,
    Base64(base64::DecodeError),
    Oprf(ppoprf::PPRFError),
}

impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        let message = match self {
            Error::LockFailure =>
                "Couldn't lock state: RwLock poisoned".into(),
            Error::BadPoint =>
                "Invalid point".into(),
            Error::Base64(e) =>
                format!("invalid base64 encoding: {e}"),
            Error::Oprf(e) =>
                format!("PPOPRF error: {e}"),
        };
        let body = Json(ErrorResponse { message });
        (StatusCode::BAD_REQUEST, body).into_response()
    }
}

/// Process PPOPRF evaluation requests
async fn randomness(
    State(state): State<OPRFState>,
    Json(request): Json<RandomnessRequest>,
) -> Result<Json<RandomnessResponse>, Error> {
    debug!("recv: {request:?}");
    let state = state.read().map_err(|_| Error::LockFailure)?;
    let epoch = request.epoch.unwrap_or(state.epoch);
    let prove = false;
    let mut points = Vec::with_capacity(request.points.len());
    for base64_point in request.points {
        let input = BASE64.decode(base64_point)
            .map_err(Error::Base64)?;
        // FIXME: Point::from is fallible and needs to return a result.
        // partial work-around: check correct length
        if input.len() != ppoprf::COMPRESSED_POINT_LEN {
            return Err(Error::BadPoint);
        }
        let point = ppoprf::Point::from(input.as_slice());
        let evaluation = state.server.eval(&point, epoch, prove)
            .map_err(Error::Oprf)?;
        points.push(BASE64.encode(evaluation.output.as_bytes()));
    }
    let response = RandomnessResponse { points, epoch };
    debug!("send: {response:?}");
    Ok(Json(response))
}

/// Advance to the next epoch on a timer
async fn epoch_update_loop(state: OPRFState, epochs: Vec<u8>) {
    let mut future_epochs = Vec::with_capacity(epochs.len() - 1);

    let interval = std::time::Duration::from_secs(5);
    info!("rotating epoch every {} seconds", interval.as_secs());

    loop {
        if future_epochs.is_empty() {
            // Flip the epoch list so we can pop() them off in order.
            future_epochs = epochs[1..].to_owned();
            future_epochs.reverse();
        }

        // Wait until the current epoch ends.
        tokio::time::sleep(interval).await;

        // Acquire exclusive access to the oprf state.
        let mut s = state.write().expect("Failed to lock OPRFState");

        // Puncture the current epoch so it can no longer be used.
        let old_epoch = s.epoch;
        s.server.puncture(old_epoch).expect("Failed to puncture current epoch");
        // Mark the next epoch as current.
        if let Some(new_epoch) = future_epochs.pop() {
            s.epoch = new_epoch;
        } else {
            info!("Epochs exhausted! Rotating OPRF key");
            *s = OPRFServer::new(&epochs)
                .expect("Could not initialize new PPOPRF state");
        }
        info!("epoch now {}", s.epoch);
    }
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
    let server = OPRFServer::new(&epochs)
        .expect("Could not initialize PPOPRF state");
    let oprf_state = Arc::new(RwLock::new(server));

    // Spawn a background process to advance the epoch
    info!("Spawning background task...");
    let background_state = oprf_state.clone();
    let background_epochs = epochs[1..].to_owned();
    tokio::spawn(async move {
        epoch_update_loop(background_state, background_epochs).await
    });

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
