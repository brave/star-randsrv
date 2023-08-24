use std::collections::HashSet;

use time::{format_description::well_known::Rfc3339, OffsetDateTime};

/// Parse a timestamp given as a config option
pub fn parse_timestamp(stamp: &str) -> Result<OffsetDateTime, &'static str> {
    OffsetDateTime::parse(stamp, &Rfc3339).map_err(|_| "Try something like '2023-05-15T04:30:00Z'.")
}

/// Asserts that all instance names are unique
pub fn assert_unique_names(instance_names: &[String]) {
    let mut name_set = HashSet::new();
    assert!(
        instance_names.iter().all(|n| name_set.insert(n)),
        "all instance names must be unique"
    );
}
