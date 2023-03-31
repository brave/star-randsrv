//! STAR Randomness web service
//! Epoch and key rotation

use time::format_description::well_known::Rfc3339;
use tracing::{info, instrument};

use crate::handler::OPRFServer;
use crate::handler::OPRFState;
use crate::Config;

/// Advance to the next epoch on a timer
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
