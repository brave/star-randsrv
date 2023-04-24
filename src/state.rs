//! STAR Randomness web service
//! Epoch and key state and its management

use std::sync::{Arc, RwLock};
use time::format_description::well_known::Rfc3339;
use tracing::{info, instrument};

use crate::Config;
use ppoprf::ppoprf;

/// Internal state of the OPRF service
pub struct OPRFServer {
    /// oprf implementation
    pub server: ppoprf::Server,
    /// currently-valid randomness epoch
    pub epoch: u8,
    /// RFC 3339 timestamp of the next epoch rotation
    pub next_epoch_time: Option<String>,
}

/// Shareable wrapper around the server state
pub type OPRFState = Arc<RwLock<OPRFServer>>;

impl OPRFServer {
    /// Initialize a new OPRFServer state with the given configuration
    pub fn new(config: &Config) -> Result<Self, ppoprf::PPRFError> {
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

/// Advance to the next epoch on a timer
/// This can be invoked as a background task to handle
/// epoch advance and key rotation according to the
/// given Config.
#[instrument(skip_all)]
pub async fn epoch_loop(state: OPRFState, config: &Config) {
    let interval =
        std::time::Duration::from_secs(config.epoch_seconds.into());
    info!("rotating epoch every {} seconds", interval.as_secs());

    let epochs = config.first_epoch..=config.last_epoch;
    loop {
        // Pre-calculate the next_epoch_time for the InfoResponse hander.
        let now = time::OffsetDateTime::now_utc();
        let next_rotation = now + interval;
        // Truncate to the nearest second.
        let next_rotation = next_rotation
            .replace_millisecond(0)
            .expect("should be able to round to a fixed ms.");
        let timestamp = next_rotation
            .format(&Rfc3339)
            .expect("well-known timestamp format should always succeed");
        {
            // Acquire a temporary write lock which should be dropped
            // before sleeping. The locking should not fail, but if it
            // does we can't set the field back to None, so panic rather
            // than report stale information.
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

        // Advance to the next epoch, checking for overflow
        // and out-of-range.
        let new_epoch = old_epoch.checked_add(1);
        if new_epoch.filter(|e| epochs.contains(e)).is_some() {
            // Server is already initialized for this one.
            s.epoch = new_epoch.unwrap();
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
