use std::collections::HashSet;

use ppoprf::ppoprf::ServerKeyState;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

/// Parse a timestamp given as a config option
pub fn parse_timestamp(stamp: &str) -> Result<OffsetDateTime, &'static str> {
    OffsetDateTime::parse(stamp, &Rfc3339).map_err(|_| "Try something like '2023-05-15T04:30:00Z'.")
}

/// Decode a postcard-serialized OPRF private key
pub fn decode_static_key(key: &[u8]) -> Result<ServerKeyState, postcard::Error> {
    postcard::from_bytes(key)
}

/// Asserts that all instance names are unique
pub fn assert_unique_names(instance_names: &[String]) {
    let mut name_set = HashSet::new();
    assert!(
        instance_names.iter().all(|n| name_set.insert(n)),
        "all instance names must be unique"
    );
}

/// Asserts that, if any static keys are defined, a valid key
/// is defined for every instance
pub fn assert_valid_static_keys(instance_names: &[String], static_keys: &[Vec<u8>]) {
    if static_keys.is_empty() {
        return;
    }
    assert!(
        instance_names.len() == static_keys.len(),
        "instance-name switch count must match static-key switch count"
    );
    assert!(
        static_keys.iter().all(|key| decode_static_key(key).is_ok()),
        "all static keys must be valid"
    );
}
