use clap::Parser;
use color_eyre::eyre::Result;
use dotenvy::dotenv;
use gatling::{
    actions,
    cli::{Cli, Command},
    config::GatlingConfig,
};

pub fn setup_tracing() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .compact()
        .with_file(false)
        .with_line_number(true)
        .with_thread_ids(false)
        .with_target(false)
        .init();
}

#[tokio::main(flavor = "multi_thread", worker_threads = 10)]
async fn main() -> Result<()> {
    // Initialize the logger.
    setup_tracing();

    // Initialize the error handler.
    color_eyre::install()?;

    // Load the environment variables from the .env file.
    dotenv().ok();

    tracing::info!("🔫 Starting Gatling...");

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
            actions::shoot(cfg).await?;
        }
        Command::Read { .. } => {
            actions::read(cfg).await?;
        }
    }

    Ok(())
}
