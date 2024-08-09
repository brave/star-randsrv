//! STAR Randomness web service route implementation

use axum::body::Bytes;
use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use base64::prelude::{Engine as _, BASE64_STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLockReadGuard;
use tracing::{debug, instrument};

use crate::instance::OPRFInstance;
use crate::result::{Error, Result};
use crate::state::OPRFState;
use ppoprf::ppoprf;

/// Request structure for the randomness endpoint
#[derive(Deserialize, Debug)]
pub struct RandomnessRequest {
    /// Array of points to evaluate
    /// Should be base64-encoded, compressed Ristretto curve points.
    points: Vec<String>,
    /// Optional request for evaluation within a specific epoch
    epoch: Option<u8>,
}

/// Response structure for the randomness endpoint
#[derive(Serialize, Debug)]
pub struct RandomnessResponse {
    /// Resulting points from the OPRF valuation
    /// Should be base64-encoded, compressed points in one-to-one
    /// correspondence with the request points array.
    points: Vec<String>,
    /// Randomness epoch used in the evaluation
    epoch: u8,
}

/// Response structure for the info endpoint
/// Rename fields to match the earlier golang implementation.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InfoResponse {
    /// ServerPublicKey used to verify zero-knowledge proof
    public_key: String,
    /// Currently active randomness epoch
    current_epoch: u8,
    /// Timestamp of the next epoch rotation
    /// This should be a string in RFC 3339 format,
    /// e.g. 2023-03-14T16:33:05Z.
    next_epoch_time: String,
    /// Maximum number of points accepted in a single request
    max_points: usize,
}

/// Response structure for the "list instances" endpoint.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ListInstancesResponse {
    /// A list of available instances on the server.
    instances: Vec<String>,
    /// The default instance on this server.
    /// A requests made to /info and /randomness will utilize this instance.
    default_instance: String,
}

/// Response returned to report error conditions
#[derive(Serialize, Debug)]
struct ErrorResponse {
    /// Human-readable description of the error
    message: String,
}

impl axum::response::IntoResponse for Error {
    /// Construct an http response from our error type
    fn into_response(self) -> axum::response::Response {
        let code = match self {
            Error::InstanceNotFound(_) => StatusCode::NOT_FOUND,
            Error::PPOPRFNotReady => StatusCode::SERVICE_UNAVAILABLE,
            // Other cases are the client's fault.
            _ => StatusCode::BAD_REQUEST,
        };
        let body = Json(ErrorResponse {
            message: self.to_string(),
        });
        (code, body).into_response()
    }
}

async fn get_server_from_state<'a>(
    state: &'a OPRFState,
    instance_name: &'a str,
) -> Result<RwLockReadGuard<'a, Option<OPRFInstance>>> {
    Ok(state
        .instances
        .get(instance_name)
        .ok_or_else(|| Error::InstanceNotFound(instance_name.to_string()))?
        .read()
        .await)
}

/// Process PPOPRF evaluation requests
#[instrument(skip(state, request))]
async fn randomness(
    state: OPRFState,
    instance_name: String,
    request: RandomnessRequest,
) -> Result<Json<RandomnessResponse>> {
    debug!("recv: {request:?}");
    let state_guard = get_server_from_state(&state, &instance_name).await?;
    match state_guard.as_ref() {
        None => Err(Error::PPOPRFNotReady),
        Some(state) => {
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
    }
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
    let state_guard = get_server_from_state(&state, &instance_name).await?;
    match state_guard.as_ref() {
        None => Err(Error::PPOPRFNotReady),
        Some(state) => {
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
    }
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

// Lists all available instances, as well as the default instance
pub async fn list_instances(State(state): State<OPRFState>) -> Result<Json<ListInstancesResponse>> {
    Ok(Json(ListInstancesResponse {
        instances: state.instances.keys().cloned().collect(),
        default_instance: state.default_instance.clone(),
    }))
}

/// Stores keys sent by nitriding, and sourced from the leader enclave.
pub async fn set_ppoprf_private_key(State(state): State<OPRFState>, body: Bytes) -> Result<()> {
    state.set_private_keys(body).await
}

/// Generates & exports keys so that nitriding and forward the keys to worker enclaves.
pub async fn get_ppoprf_private_key(State(state): State<OPRFState>) -> Result<Vec<u8>> {
    state.create_missing_instances().await;
    state.get_private_keys().await
}
