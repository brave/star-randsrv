//! STAR Randomness web service route implementation

use std::sync::RwLockReadGuard;

use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use base64::prelude::{Engine as _, BASE64_STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use crate::state::{OPRFInstance, OPRFState};
use ppoprf::ppoprf;

/// Request format for the randomness endpoint
#[derive(Deserialize, Debug)]
pub struct RandomnessRequest {
    /// Array of points to evaluate
    /// Should be base64-encoded, compressed Ristretto curve points.
    points: Vec<String>,
    /// Optional request for evaluation within a specific epoch
    epoch: Option<u8>,
}

/// Response format for the randomness endpoint
#[derive(Serialize, Debug)]
pub struct RandomnessResponse {
    /// Resulting points from the OPRF valuation
    /// Should be base64-encoded, compressed points in one-to-one
    /// correspondence with the request points array.
    points: Vec<String>,
    /// Randomness epoch used in the evaluation
    epoch: u8,
}

/// Response format for the info endpoint
/// Rename fields to match the earlier golang implementation.
#[derive(Serialize, Debug)]
pub struct InfoResponse {
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
pub enum Error {
    #[error("instance '{0}' not found")]
    InstanceNotFound(String),
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
        let code = match self {
            Error::InstanceNotFound(_) => StatusCode::NOT_FOUND,
            // This indicates internal failure.
            Error::LockFailure => StatusCode::INTERNAL_SERVER_ERROR,
            // Other cases are the client's fault.
            _ => StatusCode::BAD_REQUEST,
        };
        let body = Json(ErrorResponse {
            message: self.to_string(),
        });
        (code, body).into_response()
    }
}

type Result<T> = std::result::Result<T, Error>;

fn get_server_from_state(
    state: &OPRFState,
    instance_name: String,
) -> Result<RwLockReadGuard<'_, OPRFInstance>> {
    Ok(state
        .instances
        .get(&instance_name)
        .ok_or(Error::InstanceNotFound(instance_name))?
        .read()?)
}

/// Process PPOPRF evaluation requests
#[instrument(skip(state, request))]
async fn randomness(
    state: OPRFState,
    instance_name: String,
    request: RandomnessRequest,
) -> Result<Json<RandomnessResponse>> {
    debug!("recv: {request:?}");
    let state = get_server_from_state(&state, instance_name)?;
    let epoch = request.epoch.unwrap_or(state.epoch);
    if epoch != state.epoch {
        return Err(Error::BadEpoch(epoch));
    }
    if request.points.len() > crate::MAX_POINTS {
        return Err(Error::TooManyPoints);
    }
    // Don't support returning proofs until we have a more
    // space-efficient batch proof implemented in ppoprf.
    let mut points = Vec::with_capacity(request.points.len());
    for base64_point in request.points {
        let input = BASE64.decode(base64_point)?;
        // FIXME: Point::from is fallible and needs to return a result.
        // partial work-around: check correct length
        if input.len() != ppoprf::COMPRESSED_POINT_LEN {
            return Err(Error::BadPoint);
        }
        let point = ppoprf::Point::from(input.as_slice());
        let evaluation = state.server.eval(&point, epoch, false)?;
        points.push(BASE64.encode(evaluation.output.as_bytes()));
    }
    let response = RandomnessResponse { points, epoch };
    debug!("send: {response:?}");
    Ok(Json(response))
}

/// Process PPOPRF evaluation requests using default instance
pub async fn default_instance_randomness(
    State(state): State<OPRFState>,
    Json(request): Json<RandomnessRequest>,
) -> Result<Json<RandomnessResponse>> {
    let instance_name = state.default_instance.clone();
    randomness(state, instance_name, request).await
}

/// Process PPOPRF evaluation requests using specific instance
pub async fn specific_instance_randomness(
    State(state): State<OPRFState>,
    Path(instance_name): Path<String>,
    Json(request): Json<RandomnessRequest>,
) -> Result<Json<RandomnessResponse>> {
    randomness(state, instance_name, request).await
}

/// Provide PPOPRF epoch and key metadata
#[instrument(skip(state))]
async fn info(state: OPRFState, instance_name: String) -> Result<Json<InfoResponse>> {
    debug!("recv: info request");
    let state = get_server_from_state(&state, instance_name)?;
    let public_key = state.server.get_public_key().serialize_to_bincode()?;
    let public_key = BASE64.encode(public_key);
    let response = InfoResponse {
        current_epoch: state.epoch,
        next_epoch_time: state.next_epoch_time.clone(),
        max_points: crate::MAX_POINTS,
        public_key,
    };
    debug!("send: {response:?}");
    Ok(Json(response))
}

/// Provide PPOPRF epoch and key metadata using default instance
pub async fn default_instance_info(State(state): State<OPRFState>) -> Result<Json<InfoResponse>> {
    let instance_name = state.default_instance.clone();
    info(state, instance_name).await
}

/// Provide PPOPRF epoch and key metadata using specific instance
pub async fn specific_instance_info(
    State(state): State<OPRFState>,
    Path(instance_name): Path<String>,
) -> Result<Json<InfoResponse>> {
    info(state, instance_name).await
}
