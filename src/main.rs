#[macro_use]
extern crate log;
extern crate sys_info;
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

    info!("Starting Gatling...");

    // OS info
    info!(
        "💻 OS type: {}",
        sys_info::os_type().unwrap_or_else(|_| "Unknown".to_string())
    );
    info!(
        "💻 OS release: {}",
        sys_info::os_release().unwrap_or_else(|_| "Unknown".to_string())
    );

    // CPU info
    info!("💻 CPU num: {}", sys_info::cpu_num().unwrap_or(0));
    info!("💻 CPU speed (MHz): {}", sys_info::cpu_speed().unwrap_or(0));

    // Platform info
    info!(
        "💻 Platform: {}",
        sys_info::os_type().unwrap_or_else(|_| "Unknown".to_string())
    );
    info!("💻 Architecture: {}", std::env::consts::ARCH);

    // Memory info
    info!(
        "💿 Total memory (KB): {}",
        sys_info::mem_info().unwrap().total
    );
    info!(
        "💿 Free memory (KB): {}",
        sys_info::mem_info().unwrap().free
    );

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
