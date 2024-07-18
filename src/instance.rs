use calendar_duration::CalendarDuration;
use tokio::task::JoinHandle;
use tracing::info;

use crate::result::Result;
use crate::{util::format_rfc3339, Config};
use ppoprf::ppoprf;

/// Internal state of an OPRF instance
pub struct OPRFInstance {
    /// oprf implementation
    pub server: ppoprf::Server,
    /// currently-valid randomness epoch
    pub epoch: u8,
    /// Duration of each epoch
    pub epoch_duration: CalendarDuration,
    /// RFC 3339 timestamp of the next epoch rotation
    pub next_epoch_time: String,
    /// Handle for the background task associated with the instance
    pub background_task_handle: Option<JoinHandle<()>>,
}

impl OPRFInstance {
    /// Initialize a new OPRFServer state with the given configuration
    pub fn new(
        config: &Config,
        instance_name: &str,
        puncture_previous_epochs: bool,
    ) -> Result<Self> {
        let epochs_range = config.first_epoch..=config.last_epoch;
        let mut server = ppoprf::Server::new(epochs_range.clone().collect())?;

        // Get epoch duration matching the instance name.
        let instance_index = config
            .instance_names
            .iter()
            .position(|name| name == instance_name)
            .unwrap();
        let epoch_duration = config.epoch_durations[instance_index];

        // Get base time for calculating curren epochs
        let now = time::OffsetDateTime::now_utc()
            .replace_millisecond(0)
            .expect("failed to remove millisecond component from OffsetDateTime");
        let base_time = config.epoch_base_time.unwrap_or(now);

        assert!(now >= base_time, "epoch-base-time should be in the past");

        // Calculate the total amount of epochs elapsed since the base time
        // and time of next rotation by using the epoch_duration to iterate
        // from the base time until now.
        let mut elapsed_epoch_count = 0;
        let mut next_epoch_time = base_time + epoch_duration;
        while next_epoch_time <= now {
            next_epoch_time = next_epoch_time + epoch_duration;
            elapsed_epoch_count += 1;
        }

        // Calculate the current epoch using modulo arithmetic.
        let offset = elapsed_epoch_count % epochs_range.len();
        let current_epoch = config.first_epoch + offset as u8;

        // puncture_previous_epochs should be false if the keys will be
        // explictly set after construction, since the synced key will include
        // punctured information.
        if current_epoch != config.first_epoch && puncture_previous_epochs {
            // Advance to the current epoch if base time indicates we started
            // in the middle of a sequence.
            info!(
                "Puncturing obsolete epochs {}..{} to match base time",
                config.first_epoch, current_epoch
            );
            for epoch in config.first_epoch..current_epoch {
                server
                    .puncture(epoch)
                    .expect("Failed to puncture obsolete epoch");
            }
        }

        Ok(OPRFInstance {
            server,
            epoch: current_epoch,
            epoch_duration,
            next_epoch_time: format_rfc3339(&next_epoch_time),
            background_task_handle: None,
        })
    }
}
