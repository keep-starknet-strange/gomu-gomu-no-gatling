use starknet::providers::Provider;

use crate::{
    config::GatlingConfig,
    metrics::{BenchmarkReport, WholeReport},
};

use self::shoot::GatlingShooterSetup;

mod goose;
mod shoot;

pub async fn shoot(config: GatlingConfig) -> color_eyre::Result<()> {
    let run_erc20 = config.run.num_erc20_transfers != 0;
    let run_erc721 = config.run.num_erc721_mints != 0;

    let mut shooter = GatlingShooterSetup::from_config(config.clone()).await?;
    shooter.setup().await?;

    let mut whole_report = WholeReport {
        users: shooter.config().run.concurrency,
        all_bench_report: BenchmarkReport::new(
            "",
            (config.run.num_erc20_transfers + config.run.num_erc721_mints) as usize,
        ),
        benches: Vec::new(),
        extra: crate::utils::sysinfo_string(),
    };

    let start_block = shooter.rpc_client().block_number().await?;

    if run_erc20 {
        let start_block = shooter.rpc_client().block_number().await?;
        let goose_metrics = goose::erc20(&shooter).await?;
        let end_block = shooter.rpc_client().block_number().await?;

        let mut report =
            BenchmarkReport::new("Erc20 Transfers", goose_metrics.scenarios[0].counter);

        report
            .with_block_range(shooter.rpc_client(), start_block + 1, end_block)
            .await?;

        if config.report.num_blocks != 0 {
            report
                .with_last_x_blocks(shooter.rpc_client(), config.report.num_blocks)
                .await?;
        }

        report.with_goose_metrics(&goose_metrics)?;

        whole_report.benches.push(report);
    } else {
        log::info!("Skipping erc20 transfers")
    }

    if run_erc721 {
        let start_block = shooter.rpc_client().block_number().await?;
        let goose_metrics = goose::erc721(&shooter).await?;
        let end_block = shooter.rpc_client().block_number().await?;

        let mut report = BenchmarkReport::new("Erc721 Mints", goose_metrics.scenarios[0].counter);

        report
            .with_block_range(shooter.rpc_client(), start_block + 1, end_block)
            .await?;

        if config.report.num_blocks != 0 {
            report
                .with_last_x_blocks(shooter.rpc_client(), config.report.num_blocks)
                .await?;
        }

        report.with_goose_metrics(&goose_metrics)?;

        whole_report.benches.push(report);
    } else {
        log::info!("Skipping erc721 mints")
    }

    let end_block = shooter.rpc_client().block_number().await?;

    whole_report
        .all_bench_report
        .with_block_range(shooter.rpc_client(), start_block, end_block)
        .await?;

    let report_path = shooter.config().report.location.with_extension("json");

    let writer = std::fs::File::create(report_path)?;
    serde_json::to_writer_pretty(writer, &whole_report)?;

    Ok(())
}
