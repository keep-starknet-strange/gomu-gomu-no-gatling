#[macro_use]
extern crate log;
use clap::Parser;
use color_eyre::eyre::Result;
use dotenv::dotenv;
use gatling::{
    actions::shoot::shoot,
    cli::{Cli, Commands},
    config::GatlingConfig,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the logger.
    env_logger::init();

    // Initialize the error handler.
    color_eyre::install()?;

    // Load the environment variables from the .env file.
    dotenv().ok();

    // Retrieve the application configuration.
    let cfg = GatlingConfig::new()?;

    info!("Starting Gomu Gomu no Gatling...");

    // Parse the command line arguments.
    let cli = Cli::parse();

    // Execute the command.
    match cli.command {
        Some(command) => match command {
            Commands::Shoot {} => shoot(&cfg).await?,
        },
        None => {
            info!("nothing to do there, bye 👋");
        }
    }

    Ok(())
}
