use log::info;
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Initialize the logger.
    env_logger::init();

    info!("Starting Gomu Gomu no Gatling...");

    Ok(())
}
