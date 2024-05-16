use std::{fs::File, mem, sync::Arc};

use color_eyre::eyre::bail;
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

pub async fn shoot(mut config: GatlingConfig) -> color_eyre::Result<()> {
    let shooters = mem::take(&mut config.run.shooters);
    let total_txs: u64 = shooters.iter().map(|s| s.shoot).sum();

    let mut shooter_setup = GatlingSetup::from_config(config).await?;
    shooter_setup.setup_accounts().await?;

    let mut global_report = GlobalReport {
        users: shooter_setup.config().run.concurrency,
        all_bench_report: None,
        benches: Vec::new(),
        extra: crate::utils::sysinfo_string(),
    };

    let mut blocks = Option::<(u64, u64)>::None;

    for shooter in shooters {
        if shooter.shoot == 0 {
            log::info!("Skipping {} transfers", shooter.name);
            continue;
        }

        let (report, first_block, last_block) = match shooter.name.as_str() {
            "transfer" => {
                make_report_over_shooter::<TransferShooter>(&mut shooter_setup, shooter.shoot)
                    .await?
            }
            "mint" => {
                make_report_over_shooter::<MintShooter>(&mut shooter_setup, shooter.shoot).await?
            }
            name => bail!("Shooter `{name}` not found!"),
        };

        global_report.benches.push(report);
        blocks.get_or_insert((first_block, last_block)).1 = last_block;
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
    setup: &mut GatlingSetup,
    amount: u64,
) -> color_eyre::Result<(BenchmarkReport, u64, u64)> {
    let shooter = S::setup(setup).await?;
    let goose_config = S::get_goose_config(setup.config(), amount)?;

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
