//! General configuration

use color_eyre::eyre::Result;
use config::{builder::DefaultState, Config, ConfigBuilder, File};
use serde_derive::Deserialize;

/// Configuration for the application.
#[derive(Debug, Deserialize)]
pub struct GatlingConfig {
    /// The RPC configuration.
    pub rpc: Rpc,
    /// The simulation configuration.
    pub simulation: Simulation,
}

#[derive(Debug, Deserialize)]
#[allow(unused)]
pub struct Rpc {
    pub url: String,
}

#[derive(Debug, Deserialize)]
#[allow(unused)]
pub struct Simulation {
    pub fail_fast: Option<bool>,
}

impl GatlingConfig {
    /// Create a new configuration from environment variables.
    pub fn new() -> Result<Self> {
        base_config_builder()
            .build()
            .unwrap()
            .try_deserialize()
            .map_err(|e| e.into())
    }

    /// Create a new configuration from a file.
    pub fn from_file(path: &str) -> Result<Self> {
        base_config_builder()
            .add_source(File::with_name(path))
            .build()
            .unwrap()
            .try_deserialize()
            .map_err(|e| e.into())
    }
}

fn base_config_builder() -> ConfigBuilder<DefaultState> {
    Config::builder()
        // Start off by merging in the "default" configuration file
        .add_source(File::with_name("config/default"))
        // Add in settings from the environment (with a prefix of GATLING)
        // Eg.. `GATLING_FAIL_FAST=1 ./target/app` would set the `fail_fast` key
        .add_source(config::Environment::with_prefix("gatling"))
}
