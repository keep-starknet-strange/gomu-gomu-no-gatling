use ::goose::metrics::GooseMetrics;
use futures::Future;
use starknet::providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider};

use crate::{
    config::GatlingConfig,
    metrics::{BenchmarkReport, GlobalReport},
};

use self::shoot::GatlingShooterSetup;

mod goose;
mod shoot;

pub async fn shoot(config: GatlingConfig) -> color_eyre::Result<()> {
    let run_erc20 = config.run.num_erc20_transfers != 0;
    let run_erc721 = config.run.num_erc721_mints != 0;
    let total_txs = config.run.num_erc20_transfers + config.run.num_erc721_mints;
    let num_blocks = config.report.num_blocks;

    let mut shooter = GatlingShooterSetup::from_config(config).await?;
    shooter.setup().await?;

    let mut global_report = GlobalReport {
        users: shooter.config().run.concurrency,
        all_bench_report: BenchmarkReport::new("".into(), total_txs as usize),
        benches: Vec::new(),
        extra: crate::utils::sysinfo_string(),
    };

    let start_block = shooter.rpc_client().block_number().await?;

    if run_erc20 {
        let report = make_report_over_bench(
            goose::erc20(&shooter),
            "Erc20 Transfers".into(),
            shooter.rpc_client(),
            num_blocks,
        )
        .await?;

        global_report.benches.push(report);
    } else {
        log::info!("Skipping erc20 transfers")
    }

    if run_erc721 {
        let report = make_report_over_bench(
            goose::erc721(&shooter),
            "Erc721 Mints".into(),
            shooter.rpc_client(),
            num_blocks,
        )
        .await?;

        global_report.benches.push(report);
    } else {
        log::info!("Skipping erc721 mints")
    }

    let end_block = shooter.rpc_client().block_number().await?;

    global_report
        .all_bench_report
        .with_block_range(shooter.rpc_client(), start_block, end_block)
        .await?;

    let report_path = shooter.config().report.location.with_extension("json");

    let writer = std::fs::File::create(report_path)?;
    serde_json::to_writer_pretty(writer, &global_report)?;

    Ok(())
}

async fn make_report_over_bench(
    bench: impl Future<Output = color_eyre::Result<GooseMetrics>>,
    name: String,
    rpc_client: &JsonRpcClient<HttpTransport>,
    num_blocks: u64,
) -> color_eyre::Result<BenchmarkReport> {
    let start_block = rpc_client.block_number().await?;
    let goose_metrics = bench.await?;
    let end_block = rpc_client.block_number().await?;

    let mut report = BenchmarkReport::new(name, goose_metrics.scenarios[0].counter);
    report
        .with_block_range(rpc_client, start_block + 1, end_block)
        .await?;

    if num_blocks != 0 {
        report.with_last_x_blocks(rpc_client, num_blocks).await?;
    }

    report.with_goose_metrics(&goose_metrics)?;
    Ok(report)
}
