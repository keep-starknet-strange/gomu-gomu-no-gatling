use crate::config::GatlingConfig;

use self::shoot::GatlingShooterSetup;

mod goose;
mod shoot;

pub async fn shoot(config: GatlingConfig) -> color_eyre::Result<()> {
    let mut shooter = GatlingShooterSetup::from_config(config).await?;
    shooter.setup().await?;

    goose::erc20(&shooter).await?;
    goose::erc721(&shooter).await?;

    Ok(())
}
