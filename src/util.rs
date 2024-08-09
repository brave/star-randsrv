use std::collections::HashSet;

use reqwest::{Client, Method};
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

pub fn format_rfc3339(date: &OffsetDateTime) -> String {
    date.format(&Rfc3339)
        .expect("well-known timestamp format should always succeed")
}

pub async fn send_private_keys_to_nitriding(
    nitriding_internal_port: u16,
    private_key_bincode: Vec<u8>,
) -> Result<(), reqwest::Error> {
    let client = Client::new();
    let request = client
        .request(
            Method::PUT,
            format!("http://127.0.0.1:{nitriding_internal_port}/enclave/state"),
        )
        .body(private_key_bincode)
        .build()?;
    client.execute(request).await.map(|_| ())
}
