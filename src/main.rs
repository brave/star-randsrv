//! STAR Randomness web service

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::{routing::get, routing::post, Router};
use base64::prelude::{Engine as _, BASE64_STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use tracing::{debug, info};

use ppoprf::ppoprf;
use std::sync::{Arc, RwLock};

use clap::Parser;

/// Internal state of the OPRF service
struct OPRFServer {
    /// oprf implementation
    server: ppoprf::Server,
    /// currently-valid randomness epoch
    epoch: u8,
    /// RFC 3339 timestamp of the next epoch rotation
    next_epoch_time: Option<String>,
}

/// Shareable wrapper around the server state
type OPRFState = Arc<RwLock<OPRFServer>>;

impl OPRFServer {
    /// Initialize a new OPRFServer state with the given configuration
    fn new(config: &Config) -> Result<Self, ppoprf::PPRFError> {
        // ppoprf wants a vector, so generate one from our range.
        let epochs: Vec<u8> =
            (config.first_epoch..=config.last_epoch).collect();
        let epoch = epochs[0];
        let server = ppoprf::Server::new(epochs)?;
        Ok(OPRFServer {
            server,
            epoch,
            next_epoch_time: None,
        })
    }
}

/// Request format for the randomness endpoint
#[derive(Deserialize, Debug)]
struct RandomnessRequest {
    /// Array of points to evaluate
    /// Should be base64-encoded, compressed Ristretto curve points.
    points: Vec<String>,
    /// Optional request for evaluation within a specific epoch
    epoch: Option<u8>,
}

/// Response format for the randomness endpoint
#[derive(Serialize, Debug)]
struct RandomnessResponse {
    /// Resulting points from the OPRF valuation
    /// Should be base64-encoded, compressed points in one-to-one
    /// correspondence with the request points array.
    points: Vec<String>,
    /// Randomness epoch used in the evaluation
    epoch: u8,
}

/// Maximum number of points acceptable in a single request
const MAX_POINTS: usize = 1024;

/// Response format for the info endpoint
/// Rename fields to match the earlier golang implementation.
#[derive(Serialize, Debug)]
struct InfoResponse {
    /// ServerPublicKey used to verify zero-knowledge proof
    #[serde(rename = "publicKey")]
    public_key: String,
    /// Currently active randomness epoch
    #[serde(rename = "currentEpoch")]
    current_epoch: u8,
    /// Timestamp of the next epoch rotation
    /// This should be a string in RFC 3339 format,
    /// e.g. 2023-03-14T16:33:05Z.
    #[serde(rename = "nextEpochTime")]
    next_epoch_time: Option<String>,
    /// Maximum number of points accepted in a single request
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
///
/// Used to generate an `ErrorResponse` from the `?` operator
/// handling requests.
#[derive(thiserror::Error, Debug)]
enum Error {
    #[error("Couldn't lock state: RwLock poisoned")]
    LockFailure,
    #[error("Invalid point")]
    BadPoint,
    #[error("Too many points for a single request")]
    TooManyPoints,
    #[error("Invalid epoch {0}`")]
    BadEpoch(u8),
    #[error("Invalid base64 encoding: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("PPOPRF error: {0}")]
    Oprf(#[from] ppoprf::PPRFError),
}

/// thiserror doesn't generate a `From` impl without
/// an inner value to wrap. Write one explicitly for
/// `std::sync::PoisonError<T>` to avoid making the
/// whole `Error` struct generic. This allows us to
/// use `?` with `RwLock` methods instead of an
/// explicit `.map_err()`.
impl<T> From<std::sync::PoisonError<T>> for Error {
    fn from(_: std::sync::PoisonError<T>) -> Self {
        Error::LockFailure
    }
}

impl axum::response::IntoResponse for Error {
    /// Construct an http response from our error type
    fn into_response(self) -> axum::response::Response {
        let body = Json(ErrorResponse {
            message: self.to_string(),
        });
        (StatusCode::BAD_REQUEST, body).into_response()
    }
}

/// Process PPOPRF evaluation requests
async fn randomness(
    State(state): State<OPRFState>,
    Json(request): Json<RandomnessRequest>,
) -> Result<Json<RandomnessResponse>, Error> {
    debug!("recv: {request:?}");
    let state = state.read()?;
    let epoch = request.epoch.unwrap_or(state.epoch);
    if epoch != state.epoch {
        return Err(Error::BadEpoch(epoch));
    }
    if request.points.len() > MAX_POINTS {
        return Err(Error::TooManyPoints);
    }
    // Don't support returning proofs until we have a more
    // space-efficient batch proof implemented in ppoprf.
    let prove = false;
    let mut points = Vec::with_capacity(request.points.len());
    for base64_point in request.points {
        let input = BASE64.decode(base64_point)?;
        // FIXME: Point::from is fallible and needs to return a result.
        // partial work-around: check correct length
        if input.len() != ppoprf::COMPRESSED_POINT_LEN {
            return Err(Error::BadPoint);
        }
        let point = ppoprf::Point::from(input.as_slice());
        let evaluation = state.server.eval(&point, epoch, prove)?;
        points.push(BASE64.encode(evaluation.output.as_bytes()));
    }
    let response = RandomnessResponse { points, epoch };
    debug!("send: {response:?}");
    Ok(Json(response))
}

/// Process PPOPRF epoch and key requests
async fn info(
    State(state): State<OPRFState>,
) -> Result<Json<InfoResponse>, Error> {
    debug!("recv: info request");
    let state = state.read()?;
    let current_epoch = state.epoch;
    let next_epoch_time = state.next_epoch_time.clone();
    let max_points = MAX_POINTS;
    let public_key = state.server.get_public_key().serialize_to_bincode()?;
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
async fn epoch_update_loop(state: OPRFState, config: &Config) {
    let interval =
        std::time::Duration::from_secs(config.epoch_seconds.into());
    info!("rotating epoch every {} seconds", interval.as_secs());

    let epochs = config.first_epoch..=config.last_epoch;
    loop {
        // Pre-calculate the next_epoch_time for the InfoResponse hander.
        // This acquires a temporary write lock which should be dropped
        // before sleeping.
        {
            let now = time::OffsetDateTime::now_utc();
            let next_rotation = now + interval;
            // Truncate to the nearest second.
            let next_rotation = next_rotation
                .replace_millisecond(0)
                .expect("should be able to round to a fixed ms.");
            let timestamp = next_rotation
                .format(&Rfc3339)
                .expect("well_known timestamp format should always succeed");
            // Locking should not fail, but if it does we can't set the field
            // back to None, so panic rather than report stale information.
            let mut s = state
                .write()
                .expect("should be able to update next_epoch_time");
            s.next_epoch_time = Some(timestamp);
        }

        // Wait until the current epoch ends.
        tokio::time::sleep(interval).await;

        // Acquire exclusive access to the oprf state.
        // Panics if this fails, since processing requests with an
        // expired epoch weakens user privacy.
        let mut s = state.write().expect("Failed to lock OPRFState");

        // Puncture the current epoch so it can no longer be used.
        let old_epoch = s.epoch;
        s.server
            .puncture(old_epoch)
            .expect("Failed to puncture current epoch");

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
            *s = OPRFServer::new(config)
                .expect("Could not initialize new PPOPRF state");
        }
        info!("epoch now {}", s.epoch);
    }
}

/// Initialize an axum::Router for our web service
/// Having this as a separate function makes testing easier.
fn app(oprf_state: OPRFState) -> Router {
    Router::new()
        // Friendly default route to identify the site
        .route("/", get(|| async { "STAR randomness server\n" }))
        // Main endpoints
        .route("/randomness", post(randomness))
        .route("/info", get(info))
        // Attach shared state
        .with_state(oprf_state)
        // Logging must come after active routes
        .layer(tower_http::trace::TraceLayer::new_for_http())
}

/// Command line switches
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Config {
    /// Duration of each randomness epoch
    #[arg(long, default_value_t = 5)]
    epoch_seconds: u32,
    /// First epoch tag to make available
    #[arg(long, default_value_t = 0)]
    first_epoch: u8,
    /// Last epoch tag to make available
    #[arg(long, default_value_t = 255)]
    last_epoch: u8,
}

#[tokio::main]
async fn main() {
    // Start logging
    // The default subscriber respects filter directives like `RUST_LOG=info`
    tracing_subscriber::fmt::init();
    info!("STARing up!");

    // Command line switches
    let config = Config::parse();
    debug!("{config:?}");

    // Oblivious function state
    info!("initializing OPRF state...");
    let server =
        OPRFServer::new(&config).expect("Could not initialize PPOPRF state");
    info!("epoch now {}", server.epoch);
    let oprf_state = Arc::new(RwLock::new(server));

    // Spawn a background process to advance the epoch
    info!("Spawning background epoch rotation task...");
    let background_state = oprf_state.clone();
    tokio::spawn(async move {
        epoch_update_loop(background_state, &config).await
    });

    // Set up routes and middleware
    info!("initializing routes...");
    let app = app(oprf_state);

    // Start the server
    let addr = "127.0.0.1:8080".parse().unwrap();
    info!("Listening on {}", &addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::StatusCode;
    use base64::prelude::Engine as _;
    use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
    use serde_json::{json, Value};
    use std::sync::{Arc, RwLock};
    use tower::ServiceExt;

    const EPOCH: u8 = 12;

    /// Create an app instance for testing
    fn test_app() -> crate::Router {
        // arbitrary config
        let config = crate::Config {
            epoch_seconds: 1,
            first_epoch: EPOCH,
            last_epoch: EPOCH * 2,
        };
        // server state
        let mut server = crate::OPRFServer::new(&config)
            .expect("Could not initialize PPOPRF state");
        server.next_epoch_time = Some("2023-03-22T21:46:35Z".to_owned());
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
        assert!(message.len() > 0);
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
        assert!(json["maxPoints"].is_number());
        assert!(json["publicKey"].is_string());
        let b64key = json["publicKey"].as_str().unwrap();
        let binkey = crate::BASE64.decode(b64key).unwrap();
        let _ = ppoprf::ppoprf::ServerPublicKey::load_from_bincode(&binkey)
            .expect("Could not parse server public key");
    }

    #[tokio::test]
    async fn randomness() {
        let app = test_app();

        let point = RistrettoPoint::random(&mut rand_core::OsRng);
        let payload = json!({ "points": [
            crate::BASE64.encode(point.compress().as_bytes())
        ]})
        .to_string();
        println!("request body {payload:?}");

        let request = test_request("/randomness", Some(payload));
        println!("request {request:?}");
        let response = app.oneshot(request).await.unwrap();
        println!("response {response:?}");

        // Randomness should return a list of points and an epoch.
        assert_eq!(response.status(), StatusCode::OK);
        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        assert!(!body.is_empty());
        let json: Value = serde_json::from_slice(body.as_ref())
            .expect("Could not parse response body as json");
        assert!(json.is_object());
        println!("response body {json:?}");
        let epoch = json["epoch"].as_u64().unwrap();
        assert_eq!(epoch, EPOCH as u64);
        let points = json["points"].as_array().unwrap();
        assert_eq!(points.len(), 1);
        let b64point = points[0].as_str().unwrap();
        let point = crate::BASE64.decode(b64point).unwrap();
        let _ = CompressedRistretto::from_slice(&point);
    }
}
