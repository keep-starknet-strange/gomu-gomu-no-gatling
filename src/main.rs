#[macro_use]
extern crate log;
use clap::Parser;
use color_eyre::eyre::Result;
use dotenv::dotenv;
use gatling::{
    actions::shoot::shoot,
    cli::{Cli, Command},
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

    // TODO: print OS stats CPU info, platform, arch, mem info
    info!("Starting Gatling...");

    // Parse the command line arguments.
    let cli = Cli::parse();

    // Retrieve the application configuration.
    let cfg = match cli.global_opts.config_path {
        Some(path) => GatlingConfig::from_file(&path)?,
        None => GatlingConfig::new()?,
    };

    // Execute the command.
    match cli.command {
        Command::Shoot { .. } => {
            let simulation_report = shoot(cfg).await?;
            info!("simulation completed: {:?}", simulation_report);
        }
    }
    Ok(())
}
