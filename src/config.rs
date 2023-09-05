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
    pub rpc: RpcConfig,
    /// The setup phase configuration.
    pub setup: SetupConfig,
    /// The run phase configuration.
    pub run: RunConfig,
    /// Reporting configuration.
    pub report: ReportConfig,
    /// The fee paying account
    pub deployer: DeployerConfig,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(unused)]
pub struct RpcConfig {
    pub url: String,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:9944".to_string(),
        }
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct SetupConfig {
    pub erc20_contract_path: PathBuf,
    pub erc721_contract_path: PathBuf,
    pub account_contract_path: PathBuf,
    pub fee_token_address: FieldElement,
    pub num_accounts: usize,
}

#[derive(Debug, Deserialize, Default, Clone)]
// #[allow(unused)]
pub struct DeployerConfig {
    pub salt: FieldElement,
    pub address: FieldElement,
    pub signing_key: FieldElement,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct RunConfig {
    pub num_erc20_transfers: u64,
    pub num_erc721_mints: u64,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct ReportConfig {
    pub num_blocks: u64,
    pub reports_dir: PathBuf,
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
