//! STAR Randomness web service
//! Epoch and key state and its management

use std::sync::{Arc, RwLock};
use time::format_description::well_known::Rfc3339;
use tracing::{info, instrument, warn};

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
/// This can be invoked as a background task to handle epoch
/// advance and key rotation according to the given Config.
#[instrument(skip_all)]
pub async fn epoch_loop(state: OPRFState, config: &Config) {
    let interval =
        std::time::Duration::from_secs(config.epoch_seconds.into());
    info!("rotating epoch every {} seconds", interval.as_secs());

    let starttime = time::OffsetDateTime::now_utc();
    // Parse the epoch basetime if given.
    let basetime: Option<time::OffsetDateTime> = config.into();
    // If no epoch basetime was specified, use the startup time.
    let basetime = basetime.unwrap_or(starttime);
    assert!(
        starttime >= basetime,
        "epoch-basetime should be in the past"
    );

    // Calculate where we are in the epoch schedule relative to the
    // basetime. We may need to start in the middle of the range.
    let elapsed_epochs = (starttime - basetime) / interval;
    let elapsed_count = elapsed_epochs.floor() as i64;
    let epoch_count = config.last_epoch - config.first_epoch;
    let offset = elapsed_count.rem_euclid(epoch_count.into());
    let start = config.first_epoch + offset as u8;

    // Advance to the current epoch if basetime indicates we started
    // in the middle of a sequence.
    if start != config.first_epoch {
        info!(
            "Puncturing obsolete epochs {}..{} to match basetime",
            config.first_epoch, start
        );
        let mut s = state.write().expect("Failed to lock OPRFState");
        for epoch in config.first_epoch..start {
            s.server
                .puncture(epoch)
                .expect("Failed to puncture obsolete epoch");
        }
        s.epoch = start;
        info!("epoch now {}", s.epoch);
    }

    // First rotation uses whatever time remains for the current epoch.
    // This will be `interval` unless an epoch_basetime is specified.
    let partial_seconds =
        (1.0 - elapsed_epochs.fract()) * interval.as_secs_f64();
    let partial = std::time::Duration::from_secs_f64(partial_seconds);
    let mut next_rotation = starttime + partial;

    let epochs = config.first_epoch..=config.last_epoch;

    loop {
        // Pre-calculate the next_epoch_time for the InfoResponse hander.
        // Truncate to the nearest second.
        let timestamp = next_rotation;
        let timestamp = timestamp
            .replace_millisecond(0)
            .expect("should be able to round to a fixed ms")
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
        let sleep_duration = next_rotation - time::OffsetDateTime::now_utc();
        // Negative durations mean we're behind.
        if sleep_duration.is_positive() {
            tokio::time::sleep(sleep_duration.unsigned_abs()).await;
        }
        next_rotation += interval;

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

/// Parse a timestamp out of the Config
impl From<&Config> for Option<time::OffsetDateTime> {
    fn from(config: &Config) -> Self {
        let mut basetime = None;
        if let Some(stamp) = &config.epoch_basetime {
            basetime = match time::OffsetDateTime::parse(stamp, &Rfc3339) {
                Ok(timestamp) => Some(timestamp),
                Err(e) => {
                    warn!("Couldn't parse epoch-basetime argument: {e}");
                    None
                }
            }
        }
        basetime
    }
}
