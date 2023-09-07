//! General configuration

use std::path::PathBuf;

use color_eyre::eyre::Result;
use config::{builder::DefaultState, Config, ConfigBuilder, File};
use serde_derive::Deserialize;
use starknet::core::types::{contract::CompiledClass, FieldElement};

/// Configuration for the application.
#[derive(Debug, Deserialize, Clone)]
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

#[derive(Debug, Deserialize, Clone)]
pub struct ContractSourceConfigV1 {
    pub path: PathBuf,
    pub casm_path: PathBuf,
}

impl ContractSourceConfigV1 {
    pub fn get_casm_hash(&self) -> Result<FieldElement> {
        let mut casm_file = std::fs::File::open(&self.casm_path)?;
        let casm_class = serde_json::from_reader::<_, CompiledClass>(&mut casm_file)?;
        let casm_hash = casm_class.class_hash()?;
        Ok(casm_hash)
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum ContractSourceConfig {
    V0(PathBuf),
    V1(ContractSourceConfigV1),
}

impl ContractSourceConfig {
    pub fn get_contract_path(&self) -> &PathBuf {
        match self {
            ContractSourceConfig::V0(path) => path,
            ContractSourceConfig::V1(config) => &config.path,
        }
    }

    pub fn get_casm_hash(&self) -> Result<Option<FieldElement>> {
        if let ContractSourceConfig::V1(config) = self {
            let mut casm_file = std::fs::File::open(&config.casm_path)?;
            let casm_class = serde_json::from_reader::<_, CompiledClass>(&mut casm_file)?;

            Ok(Some(casm_class.class_hash()?))
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct SetupConfig {
    pub erc20_contract: ContractSourceConfig,
    pub erc721_contract: ContractSourceConfig,
    pub account_contract: ContractSourceConfig,
    pub fee_token_address: FieldElement,
    pub num_accounts: usize,
}

#[derive(Debug, Deserialize, Clone)]
// #[allow(unused)]
pub struct DeployerConfig {
    pub salt: FieldElement,
    pub address: FieldElement,
    pub signing_key: FieldElement,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RunConfig {
    pub num_erc20_transfers: u64,
    pub num_erc721_mints: u64,
}

#[derive(Debug, Deserialize, Clone)]
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
