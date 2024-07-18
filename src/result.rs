/// Server error conditions
///
/// Used to generate an `ErrorResponse` from the `?` operator
/// handling requests.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("instance '{0}' not found")]
    InstanceNotFound(String),
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
    #[error("Key serialization error: {0}")]
    KeySerialization(bincode::Error),
    #[error("Invalid private key call")]
    InvalidPrivateKeyCall,
    #[error("PPOPRF not ready")]
    PPOPRFNotReady,
}

pub type Result<T> = std::result::Result<T, Error>;
