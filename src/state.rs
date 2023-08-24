//! STAR Randomness web service
//! Epoch and key state and its management

use calendar_duration::CalendarDuration;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tracing::{info, instrument};

use crate::Config;
use ppoprf::ppoprf;

/// Internal state of an OPRF instance
pub struct OPRFInstance {
    /// oprf implementation
    pub server: ppoprf::Server,
    /// currently-valid randomness epoch
    pub epoch: u8,
    /// RFC 3339 timestamp of the next epoch rotation
    pub next_epoch_time: Option<String>,
}

impl OPRFInstance {
    /// Initialize a new OPRFServer state with the given configuration
    pub fn new(config: &Config) -> Result<Self, ppoprf::PPRFError> {
        // ppoprf wants a vector, so generate one from our range.
        let epochs: Vec<u8> = (config.first_epoch..=config.last_epoch).collect();
        let epoch = epochs[0];
        let server = ppoprf::Server::new(epochs)?;
        Ok(OPRFInstance {
            server,
            epoch,
            next_epoch_time: None,
        })
    }
}

/// Container for OPRF instances
pub struct OPRFServer {
    /// All OPRF instances, keyed by instance name
    pub instances: HashMap<String, RwLock<OPRFInstance>>,
    /// The name of the default instance
    pub default_instance: String,
}

/// Arc wrapper for OPRFServer
pub type OPRFState = Arc<OPRFServer>;

struct StartingEpochInfo {
    elapsed_epoch_count: usize,
    next_rotation: OffsetDateTime,
}

impl StartingEpochInfo {
    fn calculate(base_time: OffsetDateTime, instance_epoch_duration: CalendarDuration) -> Self {
        let now = time::OffsetDateTime::now_utc();
        let mut elapsed_epoch_count = 0;
        let mut next_rotation = base_time + instance_epoch_duration;
        while next_rotation < now {
            next_rotation = next_rotation + instance_epoch_duration;
            elapsed_epoch_count += 1;
        }
        Self {
            elapsed_epoch_count,
            next_rotation,
        }
    }
}

impl OPRFServer {
    /// Initialize all OPRF instances with given configuration
    pub fn new(config: &Config) -> Arc<Self> {
        let instances = config
            .instance_names
            .iter()
            .map(|instance_name| {
                // Oblivious function state
                info!(instance_name, "initializing OPRF state...");
                let server = OPRFInstance::new(config).expect("Could not initialize PPOPRF state");
                info!(instance_name, "epoch now {}", server.epoch);

                (instance_name.to_string(), RwLock::new(server))
            })
            .collect();
        Arc::new(OPRFServer {
            instances,
            default_instance: config.instance_names.first().cloned().unwrap(),
        })
    }

    /// Start background tasks to keep OPRF instances up to date
    pub fn start_background_tasks(self: &Arc<Self>, config: &Config) {
        for (instance_name, instance_epoch_duration) in config
            .instance_names
            .iter()
            .cloned()
            .zip(config.epoch_durations.iter().cloned())
        {
            // Spawn a background process to advance the epoch
            info!(instance_name, "Spawning background epoch rotation task...");
            let background_state = self.clone();
            let background_config = config.clone();
            tokio::spawn(async move {
                background_state
                    .epoch_loop(background_config, instance_name, instance_epoch_duration)
                    .await
            });
        }
    }

    /// Advance to the next epoch on a timer
    /// This can be invoked as a background task to handle epoch
    /// advance and key rotation according to the given instance.
    #[instrument(skip(self, config, instance_epoch_duration))]
    async fn epoch_loop(
        self: Arc<Self>,
        config: Config,
        instance_name: String,
        instance_epoch_duration: CalendarDuration,
    ) {
        let server = self
            .instances
            .get(&instance_name)
            .expect("OPRFServer should exist for instance name");
        let epochs = config.first_epoch..=config.last_epoch;

        info!("rotating epoch every {instance_epoch_duration}");

        let start_time = OffsetDateTime::now_utc();
        // Epoch base_time comes from a config argument if given,
        // otherwise use start_time.
        let base_time = config.epoch_base_time.unwrap_or(start_time);
        info!(
            "epoch base time = {}",
            base_time
                .format(&Rfc3339)
                .expect("well-known timestamp format should always succeed")
        );

        // Calculate where we are in the epoch schedule relative to the
        // base time. We may need to start in the middle of the range.
        assert!(
            start_time >= base_time,
            "epoch-base-time should be in the past"
        );
        let StartingEpochInfo {
            elapsed_epoch_count,
            mut next_rotation,
        } = StartingEpochInfo::calculate(base_time, instance_epoch_duration);

        // The `epochs` range is `u8`, so the length can be no more
        // than `u8::MAX + 1`, making it safe to truncate the modulo.
        let offset = elapsed_epoch_count % epochs.len();
        let current_epoch = epochs.start() + offset as u8;

        // Advance to the current epoch if base time indicates we started
        // in the middle of a sequence.
        if current_epoch != config.first_epoch {
            info!(
                "Puncturing obsolete epochs {}..{} to match base time",
                config.first_epoch, current_epoch
            );
            let mut s = server.write().expect("Failed to lock OPRFServer");
            for epoch in config.first_epoch..current_epoch {
                s.server
                    .puncture(epoch)
                    .expect("Failed to puncture obsolete epoch");
            }
            s.epoch = current_epoch;
            info!("epoch now {}, next rotation = {next_rotation}", s.epoch);
        }

        loop {
            // Pre-calculate the next_epoch_time for the InfoResponse hander.
            // Truncate to the nearest second.
            let timestamp = next_rotation
                .replace_millisecond(0)
                .expect("should be able to truncate to a fixed ms")
                .format(&Rfc3339)
                .expect("well-known timestamp format should always succeed");
            {
                // Acquire a temporary write lock which should be dropped
                // before sleeping. The locking should not fail, but if it
                // does we can't set the field back to None, so panic rather
                // than report stale information.
                let mut s = server
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
            next_rotation = next_rotation + instance_epoch_duration;

            // Acquire exclusive access to the oprf state.
            // Panics if this fails, since processing requests with an
            // expired epoch weakens user privacy.
            let mut s = server.write().expect("Failed to lock OPRFServer");

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
                *s = OPRFInstance::new(&config).expect("Could not initialize new PPOPRF server");
            }
            info!("epoch now {}, next rotation = {next_rotation}", s.epoch);
        }
    }
}
