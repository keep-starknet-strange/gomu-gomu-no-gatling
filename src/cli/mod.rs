//! Defines the CLI commands.

// Imports
use clap::{Args, Parser, Subcommand};

const VERSION_STRING: &str = env!("CARGO_PKG_VERSION");

/// Main CLI struct
#[derive(Parser, Debug)]
#[command(
    author,
    version = VERSION_STRING,
    about,
    long_about = "Gomu Gomu no Gatling is a load testing tool for Starknet RPC endpoints."
)]
pub struct Cli {
    #[clap(flatten)]
    pub global_opts: GlobalOpts,

    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Subcommands
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Trigger a write load test.
    Shoot {},
    // Trigger a read load test
    Read {},
}

#[derive(Debug, Args)]
pub struct GlobalOpts {
    /// Configuration file path, optional.
    #[clap(short, long, global = true)]
    pub config_path: Option<String>,
}
