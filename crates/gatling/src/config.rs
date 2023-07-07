//! General configuration

use color_eyre::eyre::Result;
use config::{Config, File};
use serde_derive::Deserialize;

/// Configuration for the application.
#[derive(Debug, Deserialize)]
pub struct GatlingConfig {
    /// The RPC configuration.
    rpc: Rpc,
    /// The simulation configuration.
    simulation: Simulation,
}

#[derive(Debug, Deserialize)]
#[allow(unused)]
struct Rpc {
    pub url: String,
}

#[derive(Debug, Deserialize)]
#[allow(unused)]
struct Simulation {
    pub fail_fast: Option<bool>,
}

impl GatlingConfig {
    /// Create a new configuration from environment variables.
    pub fn new() -> Result<Self> {
        Config::builder()
            // Start off by merging in the "default" configuration file
            .add_source(File::with_name("config/default"))
            // Add in settings from the environment (with a prefix of GATLING)
            // Eg.. `GATLING_FAIL_FAST=1 ./target/app` would set the `fail_fast` key
            .add_source(config::Environment::with_prefix("gatling"))
            .build()
            .unwrap()
            .clone()
            .try_deserialize()
            .map_err(|e| e.into())
    }
}
