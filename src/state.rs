//! STAR Randomness web service
//! Epoch and key state and its management

use axum::body::Bytes;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tokio::sync::{OnceCell, RwLock};
use tracing::{error, info, instrument};

use crate::{
    instance::OPRFInstance,
    result::{Error, Result},
};
use crate::{
    util::{format_rfc3339, parse_timestamp, send_private_keys_to_nitriding},
    Config,
};
use ppoprf::ppoprf;

/// Container for OPRF instances
pub struct OPRFServer {
    /// All OPRF instances, keyed by instance name
    /// If the instance is None, then key sync is enabled
    /// and the server is waiting for nitriding to prompt
    /// key generation or restoration.
    pub instances: HashMap<String, RwLock<Option<OPRFInstance>>>,
    /// The name of the default instance
    pub default_instance: String,
    /// The config for the server
    pub config: Config,
    /// Will only be initialized if key sync is enabled.
    /// If set, the state will reflect the leader/worker status
    /// of the server.
    pub is_leader: OnceCell<bool>,
}

/// Arc wrapper for OPRFServer
pub type OPRFState = Arc<OPRFServer>;

/// Structure containing PPOPRF key information.
/// Used when deserializing and setting keys.
#[derive(Deserialize)]
pub struct KeyInfo {
    pub key_state: ppoprf::ServerKeyState,
    pub epoch: u8,
}

/// Structure containing PPOPRF key information.
/// Used when getting keys for serialization.
#[derive(Serialize)]
pub struct KeyInfoRef<'a> {
    pub key_state: ppoprf::ServerKeyStateRef<'a>,
    pub epoch: u8,
}

/// Map of instance names to KeyInfo.
/// Used for deserializing and setting keys.
pub type OPRFKeys = BTreeMap<String, KeyInfo>;

/// Map of instance names to KeyInfoRef.
/// Used when getting keys for serialization.
pub type OPRFKeysRef<'a> = BTreeMap<String, KeyInfoRef<'a>>;

impl OPRFServer {
    /// Initialize all OPRF instances with given configuration
    pub async fn new(config: Config) -> Arc<Self> {
        let mut instances = HashMap::new();
        for instance_name in &config.instance_names {
            // If key sync is enabled, we should hold off on creating any instances.
            // We should wait until GET or PUT /enclave/state is called to either
            // generate new PPOPRF keys or sync existing keys.
            let instance = match config.enclave_key_sync {
                true => None,
                false => Some(
                    OPRFInstance::new(&config, &instance_name, true)
                        .expect("Could not initialize new PPOPRF server"),
                ),
            };
            instances.insert(instance_name.to_string(), RwLock::new(instance));
        }
        let enclave_key_sync_enabled = config.enclave_key_sync;
        let server = Arc::new(OPRFServer {
            instances,
            default_instance: config.instance_names.first().cloned().unwrap(),
            config,
            is_leader: Default::default(),
        });
        if !enclave_key_sync_enabled {
            for instance_name in &server.config.instance_names {
                server
                    .start_background_task(instance_name.to_string())
                    .await;
            }
        }
        server
    }

    /// Start background tasks to keep OPRF instances up to date
    async fn start_background_task(self: &Arc<Self>, instance_name: String) {
        // Spawn a background process to advance the epoch
        info!(instance_name, "Spawning background epoch rotation task...");
        let background_state = self.clone();
        let mut instance_guard = self.instances.get(&instance_name).unwrap().write().await;
        instance_guard.as_mut().unwrap().background_task_handle = Some(tokio::spawn(async move {
            background_state.epoch_loop(instance_name).await
        }));
    }

    /// Advance to the next epoch on a timer
    /// This can be invoked as a background task to handle epoch
    /// advance and key rotation according to the given instance.
    #[instrument(skip(self, instance_name))]
    async fn epoch_loop(self: Arc<Self>, instance_name: String) {
        let server = self
            .instances
            .get(&instance_name)
            .expect("OPRFServer should exist for instance name");

        let (mut next_epoch_time, epoch_duration) = {
            let server = server.read().await;
            let s = server.as_ref().unwrap();
            info!(
                "epoch now {}, next rotation = {}",
                s.epoch, s.next_epoch_time
            );
            (
                parse_timestamp(&s.next_epoch_time).unwrap(),
                s.epoch_duration,
            )
        };

        let epochs = self.config.first_epoch..=self.config.last_epoch;

        loop {
            // Wait until the current epoch ends.
            let sleep_duration = next_epoch_time - time::OffsetDateTime::now_utc();
            // Negative durations mean we're behind.
            if sleep_duration.is_positive() {
                tokio::time::sleep(sleep_duration.unsigned_abs()).await;
            }
            next_epoch_time = next_epoch_time + epoch_duration;

            {
                // Acquire exclusive access to the oprf state.
                // Panics if this fails, since processing requests with an
                // expired epoch weakens user privacy.
                let mut s_guard = server.write().await;
                let s = s_guard.as_mut().unwrap();

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
                    if let Some(false) = self.is_leader.get() {
                        info!("Epochs exhausted, exiting background task. New task will start after leader shares new key.");
                        *s_guard = None;
                        return;
                    } else {
                        info!("Epochs exhausted! Rotating OPRF key");
                        // Panics if this fails. Puncture should mean we can't
                        // violate privacy through further evaluations, but we
                        // still want to drop the inner state with its private key.
                        *s = OPRFInstance::new(&self.config, &instance_name, true)
                            .expect("Could not initialize new PPOPRF server");
                    }
                }
                s.next_epoch_time = format_rfc3339(&next_epoch_time);
                info!("epoch now {}, next rotation = {next_epoch_time}", s.epoch);
            }

            if self.config.enclave_key_sync {
                if let Some(true) = self.is_leader.get() {
                    // Since a new OPRFInstance was created, we should sync the new key
                    // to other enclaves if key sync is enabled.
                    send_private_keys_to_nitriding(
                        self.config.nitriding_internal_port.unwrap(),
                        self.get_private_keys()
                            .await
                            .expect("failed to get private keys to send to nitriding"),
                    )
                    .await
                    .expect("failed to send updated private keys to nitriding");
                }
            }
        }
    }

    /// Stores keys sent by nitriding, and sourced from the leader enclave.
    /// If this method is called, this server will assume that it is a worker.
    /// OPRFInstances will be created, if not created already.
    pub async fn set_private_keys(self: &Arc<Self>, private_keys_bytes: Bytes) -> Result<()> {
        assert!(self.config.enclave_key_sync);
        if let Some(true) = self.is_leader.get() {
            error!("invalid set_private_keys call on leader");
            return Err(Error::InvalidPrivateKeyCall);
        }
        if !self.is_leader.initialized() {
            self.is_leader
                .set(false)
                .expect("failed to set leader status");
        }
        let private_keys: OPRFKeys =
            bincode::deserialize(&private_keys_bytes).map_err(|e| Error::KeySerialization(e))?;
        for (instance_name, key_info) in private_keys {
            if let Some(instance) = self.instances.get(&instance_name) {
                {
                    let mut instance_guard = instance.write().await;

                    match instance_guard.as_mut() {
                        Some(existing_instance) => {
                            // If the key already matches with the stored key, or if the
                            // epoch from the update does not match the current epoch,
                            // do not update the instance at this time as there is no need
                            // to update.
                            if existing_instance.server.get_private_key()
                                == key_info.key_state.as_ref()
                                || key_info.epoch != existing_instance.epoch
                            {
                                continue;
                            }
                            // Kill existing background task, since we'll create a new one
                            // after setting the key.
                            if let Some(handle) = existing_instance.background_task_handle.take() {
                                handle.abort();
                            }
                        }
                        None => {
                            let new_instance =
                                OPRFInstance::new(&self.config, &instance_name, false)
                                    .expect("Could not initialize PPOPRF state");
                            if key_info.epoch != new_instance.epoch {
                                continue;
                            }
                            *instance_guard = Some(new_instance);
                        }
                    };

                    instance_guard
                        .as_mut()
                        .unwrap()
                        .server
                        .set_private_key(key_info.key_state);
                }

                self.start_background_task(instance_name).await;
            }
        }
        Ok(())
    }

    /// Should be called in GET /enclave/state. Will create OPRFInstances
    /// and start the background tasks so that the leader keys can be exported
    /// to nitriding.
    pub async fn create_missing_instances(self: &Arc<Self>) {
        assert!(self.config.enclave_key_sync);
        for (instance_name, instance) in &self.instances {
            let mut instance = instance.write().await;
            if instance.is_none() {
                *instance = Some(
                    OPRFInstance::new(&self.config, instance_name, true)
                        .expect("Could not initialize PPOPRF state"),
                );
                drop(instance);
                self.start_background_task(instance_name.to_string()).await;
            }
        }
    }

    /// Exports keys so that nitriding and forward the keys to worker enclaves.
    /// If this method is called, the server will assume that it is the leader.
    pub async fn get_private_keys(self: &Arc<Self>) -> Result<Vec<u8>> {
        assert!(self.config.enclave_key_sync);
        if let Some(false) = self.is_leader.get() {
            error!("invalid get_private_keys call on worker");
            return Err(Error::InvalidPrivateKeyCall);
        }
        if !self.is_leader.initialized() {
            self.is_leader
                .set(true)
                .expect("failed to set leader status");
        }
        let mut server_guards = Vec::with_capacity(self.instances.len());
        for (instance_name, instance) in &self.instances {
            server_guards.push((instance_name, instance.write().await))
        }
        let mut private_keys = OPRFKeysRef::default();
        for (instance_name, instance) in &mut server_guards {
            let instance = instance.as_ref().unwrap();

            private_keys.insert(
                instance_name.to_string(),
                KeyInfoRef {
                    epoch: instance.epoch,
                    key_state: instance.server.get_private_key(),
                },
            );
        }
        bincode::serialize(&private_keys).map_err(|e| Error::KeySerialization(e))
    }
}
