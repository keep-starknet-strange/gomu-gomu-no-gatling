use crate::config::GatlingConfig;

use self::shoot::GatlingShooterSetup;

mod goose;
mod shoot;

pub async fn shoot(config: GatlingConfig) -> color_eyre::Result<()> {
    let run_erc20 = config.run.num_erc20_transfers != 0;
    let run_erc721 = config.run.num_erc721_mints != 0;

    let mut shooter = GatlingShooterSetup::from_config(config).await?;
    shooter.setup().await?;

    if run_erc20 {
        goose::erc20(&shooter).await?;
    } else {
        log::info!("Skipping erc20 transfers")
    }

    if run_erc721 {
        goose::erc721(&shooter).await?;
    } else {
        log::info!("Skipping erc721 mints")
    }

    Ok(())
}
