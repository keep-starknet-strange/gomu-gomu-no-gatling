use std::{fs::File, sync::Arc};

use log::info;

use crate::{
    config::GatlingConfig,
    metrics::{BenchmarkReport, GlobalReport},
};

use self::{
    setup::GatlingSetup,
    shooters::{mint::MintShooter, transfer::TransferShooter, Shooter, ShooterAttack},
};

mod goose;
mod setup;
mod shooters;

pub async fn shoot(config: GatlingConfig) -> color_eyre::Result<()> {
    let total_txs = config.run.num_erc20_transfers + config.run.num_erc721_mints;

    let mut shooter_setup = GatlingSetup::from_config(config).await?;
    let transfer_shooter = TransferShooter::setup(&mut shooter_setup).await?;
    shooter_setup
        .setup_accounts(transfer_shooter.erc20_address)
        .await?;

    let mut global_report = GlobalReport {
        users: shooter_setup.config().run.concurrency,
        all_bench_report: None,
        benches: Vec::new(),
        extra: crate::utils::sysinfo_string(),
    };

    let mut blocks = Option::<(u64, u64)>::None;

    if shooter_setup.config().run.num_erc20_transfers != 0 {
        let report = make_report_over_shooter(transfer_shooter, &shooter_setup).await?;

        global_report.benches.push(report.0);
        blocks.get_or_insert((report.1, report.2)).1 = report.2;
    } else {
        log::info!("Skipping erc20 transfers")
    }

    if shooter_setup.config().run.num_erc721_mints != 0 {
        let shooter = MintShooter::setup(&mut shooter_setup).await?;

        let report = make_report_over_shooter(shooter, &shooter_setup).await?;

        global_report.benches.push(report.0);
        blocks.get_or_insert((report.1, report.2)).1 = report.2;

    } else {
        log::info!("Skipping erc721 mints")
    }

    let mut all_bench_report = BenchmarkReport::new("".into(), total_txs as usize);

    if let Some((start_block, end_block)) = blocks {
        info!("Start and End Blocks: {start_block}, {end_block}");

        let rpc_result = all_bench_report
        .with_block_range(shooter_setup.rpc_client(), start_block, end_block)
        .await;

        global_report.all_bench_report = Some(all_bench_report);

        if let Err(error) = rpc_result {
            log::error!("Failed to get block range: {error}")
        }
    }

    let report_path = shooter_setup
        .config()
        .report
        .output_location
        .with_extension("json");

    serde_json::to_writer_pretty(File::create(report_path)?, &global_report)?;

    Ok(())
}

async fn make_report_over_shooter<S: Shooter + Send + Sync + 'static>(
    shooter: S,
    setup: &GatlingSetup,
) -> color_eyre::Result<(BenchmarkReport, u64, u64)> {
    let goose_config = S::get_goose_config(setup.config())?;

    let ShooterAttack {
        goose_metrics,
        first_block,
        last_block,
    } = Arc::new(shooter)
        .goose_attack(goose_config, setup.accounts().to_vec())
        .await?;

    let mut report = BenchmarkReport::new(S::NAME.to_string(), goose_metrics.scenarios[0].counter);

    let rpc_result = report
        .with_block_range(setup.rpc_client(), first_block + 1, last_block)
        .await;

    let num_blocks = setup.config().report.num_blocks;

    if let Err(error) = rpc_result {
        log::error!("Failed to get block range: {error}")
    } else if num_blocks != 0 {
        report
            .with_last_x_blocks(setup.rpc_client(), num_blocks)
            .await?;
    }

    report.with_goose_write_metrics(&goose_metrics)?;
    Ok((report, first_block, last_block))
}

pub async fn read(config: GatlingConfig) -> color_eyre::Result<()> {
    let shooter_setup = GatlingSetup::from_config(config).await?;

    let mut global_report = GlobalReport {
        users: shooter_setup.config().run.concurrency,
        all_bench_report: None,
        benches: Vec::new(),
        extra: crate::utils::sysinfo_string(),
    };

    for read_bench in &shooter_setup.config().run.read_benches {
        let metrics = goose::read_method(
            &shooter_setup,
            read_bench.num_requests,
            read_bench.method,
            read_bench.parameters_location.clone(),
        )
        .await?;

        let mut report =
            BenchmarkReport::new(read_bench.name.clone(), metrics.scenarios[0].counter);

        report.with_goose_read_metrics(&metrics)?;

        global_report.benches.push(report);
    }

    let report_path = shooter_setup
        .config()
        .report
        .output_location
        .with_extension("json");

    serde_json::to_writer_pretty(File::create(report_path)?, &global_report)?;

    Ok(())
}
