//! General configuration

use std::path::PathBuf;

use color_eyre::eyre::Result;
use config::{builder::DefaultState, Config, ConfigBuilder, File};
use serde_derive::Deserialize;
use starknet::core::types::FieldElement;

/// Configuration for the application.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct GatlingConfig {
    /// The RPC configuration.
    pub rpc: Rpc,
    /// The simulation configuration.
    pub simulation: Simulation,
    /// The fee paying account
    pub deployer: Deployer,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(unused)]
pub struct Rpc {
    pub url: String,
}

impl Default for Rpc {
    fn default() -> Self {
        Self {
            url: "http://localhost:9944".to_string(),
        }
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
#[allow(unused)]
pub struct Simulation {
    pub fail_fast: bool,
    pub setup: Setup,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Setup {
    pub erc20_contract_path: PathBuf,
    pub erc721_contract_path: PathBuf,
    pub account_contract_path: PathBuf,
    pub fee_token_address: FieldElement,
    pub num_accounts: usize,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[allow(unused)]
pub struct Deployer {
    pub address: String,
    pub signing_key: String,
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
        // Add in settings from the environment (with a prefix of GATLING)
        // Eg.. `GATLING_FAIL_FAST=1 ./target/app` would set the `fail_fast` key
        .add_source(config::Environment::with_prefix("gatling"))
}
