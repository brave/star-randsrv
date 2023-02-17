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

/// Shareable wrapper around the server state
type OPRFState = Arc<RwLock<OPRFServer>>;
/// Represents an contiguous range of configured randomness epochs
type EpochRange = std::ops::Range<u8>;

impl OPRFServer {
    /// Initialize a new OPRFServer state supporting the given list of epochs
    fn new(epochs: &EpochRange) -> Result<Self, ppoprf::PPRFError> {
        // ppoprf wants a vector, so generate one from our range.
        let epochs: Vec<u8> = epochs.to_owned().collect();
        let epoch = epochs[0];
        let server = ppoprf::Server::new(epochs)?;
        Ok(OPRFServer{ server, epoch })
    }
}


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

/// Maximum number of points acceptable in a single request
const MAX_POINTS: usize = 1024;

/// Response format for the info endpoint
/// Rename fields to match the earlier go implementation.
#[derive(Serialize, Debug)]
struct InfoResponse {
    #[serde(rename = "publicKey")]
    public_key: String,
    #[serde(rename = "currentEpoch")]
    current_epoch: u8,
    #[serde(rename = "nextEpochTime")]
    next_epoch_time: String,
    #[serde(rename = "maxPoints")]
    max_points: usize,
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

/// Process PPOPRF epoch and key requests
async fn info(
    State(state): State<OPRFState>
) -> Result<Json<InfoResponse>, Error> {
    debug!("recv: info reqeust");
    let state = state.read().map_err(|_| Error::LockFailure)?;
    let current_epoch = state.epoch;
    // FIXME: return the end of the current epoch
    let next_epoch_time = "unknown".to_owned();
    let max_points = MAX_POINTS;
    let public_key = state.server.get_public_key()
        .serialize_to_bincode()
        .map_err(Error::Oprf)?;
    let public_key = BASE64.encode(public_key);
    let response = InfoResponse {
        current_epoch,
        next_epoch_time,
        max_points,
        public_key,
    };
    debug!("send: {response:?}");
    Ok(Json(response))
}

/// Advance to the next epoch on a timer
async fn epoch_update_loop(state: OPRFState, epochs: EpochRange) {
    let interval = std::time::Duration::from_secs(5);
    info!("rotating epoch every {} seconds", interval.as_secs());

    loop {
        // Wait until the current epoch ends.
        tokio::time::sleep(interval).await;

        // Acquire exclusive access to the oprf state.
        // Panics if this fails, since processing requests with an
        // expired epoch weakens user privacy.
        let mut s = state.write().expect("Failed to lock OPRFState");

        // Puncture the current epoch so it can no longer be used.
        let old_epoch = s.epoch;
        s.server.puncture(old_epoch).expect("Failed to puncture current epoch");

        // Advance to the next epoch.
        let new_epoch = old_epoch + 1;
        if epochs.contains(&new_epoch) {
            // Server is already initialized for this one.
            s.epoch = new_epoch;
        } else {
            info!("Epochs exhausted! Rotating OPRF key");
            // Panics if this fails. Puncture should mean we can't
            // violate privacy through further evaluations, but we
            // still want to drop the inner state with its private key.
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
    let epochs = 0..255;
    let server = OPRFServer::new(&epochs)
        .expect("Could not initialize PPOPRF state");
    let oprf_state = Arc::new(RwLock::new(server));
    info!("epoch now {}", epochs.start);

    // Spawn a background process to advance the epoch
    info!("Spawning background task...");
    let background_state = oprf_state.clone();
    tokio::spawn(async move {
        epoch_update_loop(background_state, epochs).await
    });

    // Set up routes and middleware
    info!("initializing routes...");
    let app = Router::new()
        // Friendly default route to identify the site
        .route("/", get(|| async { "STAR randomness server\n" }))
        // Main endpoint
        .route("/randomness", post(randomness))
        .route("/info", get(info))
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
