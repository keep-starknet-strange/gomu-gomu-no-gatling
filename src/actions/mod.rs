use std::sync::Arc;

use starknet::{core::types::BlockId, providers::Provider};

use crate::{
    config::GatlingConfig,
    metrics::{BenchmarkReport, GlobalReport},
};

use self::{
    setup::GatlingSetup,
    shooter::{MintShooter, Shooter, TransferShooter},
};

mod goose;
mod setup;
mod shooter;

pub async fn shoot(config: GatlingConfig) -> color_eyre::Result<()> {
    let total_txs = config.run.num_erc20_transfers + config.run.num_erc721_mints;

    let mut shooter_setup = GatlingSetup::from_config(config).await?;
    let transfer_shooter = TransferShooter::setup(&mut shooter_setup).await?;
    shooter_setup
        .setup_accounts(transfer_shooter.erc20_address)
        .await?;

    let mut global_report = GlobalReport {
        users: shooter_setup.config().run.concurrency,
        all_bench_report: BenchmarkReport::new("".into(), total_txs as usize),
        benches: Vec::new(),
        extra: crate::utils::sysinfo_string(),
    };

    let start_block = shooter_setup.rpc_client().block_number().await?;

    if shooter_setup.config().run.num_erc20_transfers != 0 {
        let report = make_report_over_shooter(transfer_shooter, &shooter_setup).await?;

        global_report.benches.push(report);
    } else {
        log::info!("Skipping erc20 transfers")
    }

    if shooter_setup.config().run.num_erc721_mints != 0 {
        let shooter = MintShooter::setup(&mut shooter_setup).await?;

        let report = make_report_over_shooter(shooter, &shooter_setup).await?;

        global_report.benches.push(report);
    } else {
        log::info!("Skipping erc721 mints")
    }

    let end_block = shooter_setup.rpc_client().block_number().await?;

    for read_bench in &shooter_setup.config().run.read_benches {
        let mut params = read_bench.parameters_location.clone();

        // Look into templating json for these if it becomes more complex to handle
        // liquid_json sees like a relatively popular option for this
        for parameter in &mut params {
            if let Some(from) = parameter.get_mut("from_block") {
                if from.is_null() {
                    *from = serde_json::to_value(BlockId::Number(start_block))?;
                }
            }

            if let Some(to) = parameter.get_mut("to_block") {
                if to.is_null() {
                    *to = serde_json::to_value(BlockId::Number(end_block))?;
                }
            }
        }

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

    let rpc_result = global_report
        .all_bench_report
        .with_block_range(shooter_setup.rpc_client(), start_block, end_block)
        .await;

    if let Err(error) = rpc_result {
        log::error!("Failed to get block range: {error}")
    }

    let report_path = shooter_setup
        .config()
        .report
        .output_location
        .with_extension("json");

    let writer = std::fs::File::create(report_path)?;
    serde_json::to_writer_pretty(writer, &global_report)?;

    Ok(())
}

async fn make_report_over_shooter<S: Shooter + Send + Sync + 'static>(
    shooter: S,
    setup: &GatlingSetup,
) -> color_eyre::Result<BenchmarkReport> {
    let goose_config = S::get_goose_config(setup.config())?;

    let attack = Arc::new(shooter)
        .create_goose_attack(goose_config, setup.accounts().to_vec())
        .await?;

    let start_block = setup.rpc_client().block_number().await?;
    let goose_metrics = attack.execute().await?;
    let end_block = setup.rpc_client().block_number().await?;

    let mut report = BenchmarkReport::new(S::NAME.to_string(), goose_metrics.scenarios[0].counter);

    let rpc_result = report
        .with_block_range(setup.rpc_client(), start_block + 1, end_block)
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
    Ok(report)
}
