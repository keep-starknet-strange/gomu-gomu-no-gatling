use crate::config::GatlingConfig;
use color_eyre::eyre::Result;
use log::{debug, info};

/// Shoot the load test simulation.
pub async fn shoot(config: &GatlingConfig) -> Result<()> {
    info!("starting simulation with config: {:?}", config);
    // Trigger the setup phase.
    setup().await?;
    // Run the simulation.
    run().await?;
    // Trigger the teardown phase.
    teardown().await?;
    Ok(())
}

/// Setup the simulation.
pub async fn setup() -> Result<()> {
    debug!("setting up!");
    Ok(())
}

/// Teardown the simulation.
pub async fn teardown() -> Result<()> {
    debug!("tearing down!");
    Ok(())
}

/// Run the simulation.
pub async fn run() -> Result<()> {
    debug!("firing!");
    Ok(())
}
