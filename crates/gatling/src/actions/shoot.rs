use crate::config::GatlingConfig;
use color_eyre::eyre::Result;
use log::info;
use starknet::{core::types::FieldElement, providers::Provider};

use starknet::providers::{jsonrpc::HttpTransport, JsonRpcClient};
use url::Url;

/// Shoot the load test simulation.
pub async fn shoot(config: GatlingConfig) -> Result<SimulationReport> {
    info!("starting simulation with config: {:?}", config);
    let mut shooter = GatlingShooter::new(config)?;
    let mut simulation_report = Default::default();
    // Trigger the setup phase.
    shooter.setup(&mut simulation_report).await?;
    // Run the simulation.
    shooter.run(&mut simulation_report).await?;
    // Trigger the teardown phase.
    shooter.teardown(&mut simulation_report).await?;
    Ok(simulation_report.clone())
}

pub struct GatlingShooter {
    config: GatlingConfig,
    starknet_rpc: JsonRpcClient<HttpTransport>,
}

impl GatlingShooter {
    pub fn new(config: GatlingConfig) -> Result<Self> {
        let starknet_rpc = starknet_rpc_provider(Url::parse(&config.rpc.url)?);
        Ok(Self {
            config,
            starknet_rpc,
        })
    }

    /// Setup the simulation.
    async fn setup<'a>(&mut self, _simulation_report: &'a mut SimulationReport) -> Result<()> {
        info!("setting up!");
        let chain_id = self.starknet_rpc.chain_id().await?;
        info!("chain id: {}", chain_id);
        let block_number = self.starknet_rpc.block_number().await?;
        info!("block number: {}", block_number);
        Ok(())
    }

    /// Teardown the simulation.
    async fn teardown<'a>(&mut self, _simulation_report: &'a mut SimulationReport) -> Result<()> {
        info!("tearing down!");
        Ok(())
    }

    /// Run the simulation.
    async fn run<'a>(&mut self, _simulation_report: &'a mut SimulationReport) -> Result<()> {
        info!("firing!");
        let _fail_fast = self.config.simulation.fail_fast.unwrap_or(true);
        Ok(())
    }
}

/// The simulation report.
#[derive(Debug, Default, Clone)]
pub struct SimulationReport {
    pub chain_id: Option<FieldElement>,
    pub block_number: Option<u64>,
}

/// Create a StarkNet RPC provider from a URL.
/// # Arguments
/// * `rpc` - The URL of the StarkNet RPC provider.
/// # Returns
/// A StarkNet RPC provider.
fn starknet_rpc_provider(rpc: Url) -> JsonRpcClient<HttpTransport> {
    JsonRpcClient::new(HttpTransport::new(rpc))
}
