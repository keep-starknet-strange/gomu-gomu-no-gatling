use color_eyre::eyre::bail;
use log::{debug, info, warn};
use starknet::{
    accounts::{Account, Call, ConnectedAccount},
    contract::ContractFactory,
    core::types::{BlockId, BlockTag, FieldElement},
    macros::{felt, selector},
    providers::Provider,
};
use tokio::task::JoinSet;

use crate::{
    actions::setup::{self, GatlingSetup, StarknetAccount, CHECK_INTERVAL, MAX_FEE},
    config::GatlingConfig,
    utils::{compute_contract_address, wait_for_tx},
};

use super::Shooter;

pub struct TransferShooter {
    pub erc20_address: FieldElement,
    pub account: StarknetAccount,
}

impl Shooter for TransferShooter {
    const NAME: &'static str = "Erc20 Transfers";

    async fn setup(setup: &mut GatlingSetup) -> color_eyre::Result<Self>
    where
        Self: Sized,
    {
        let class_hash = setup
            .declare_contract(&setup.config().setup.erc20_contract.clone())
            .await?;

        let contract_factory = ContractFactory::new(class_hash, setup.deployer_account().clone());
        let mut nonce = setup.deployer_account().get_nonce().await?;

        let name = selector!("TestToken");
        let symbol = selector!("TT");
        let decimals = felt!("128");
        let (initial_supply_low, initial_supply_high) =
            (felt!("0xFFFFFFFFF"), felt!("0xFFFFFFFFF"));
        let recipient = setup.deployer_account().address();

        let constructor_args = vec![
            name,
            symbol,
            decimals,
            initial_supply_low,
            initial_supply_high,
            recipient,
        ];
        let unique = false;

        let address =
            compute_contract_address(setup.config().deployer.salt, class_hash, &constructor_args);

        if let Ok(contract_class_hash) = setup
            .rpc_client()
            .get_class_hash_at(BlockId::Tag(BlockTag::Pending), address)
            .await
        {
            if contract_class_hash == class_hash {
                warn!("ERC20 contract already deployed at address {address:#064x}");
                return Ok(TransferShooter {
                    erc20_address: address,
                    account: setup.deployer_account().clone(),
                });
            } else {
                bail!("ERC20 contract {address:#064x} already deployed with a different class hash {contract_class_hash:#064x}, expected {class_hash:#064x}");
            }
        }

        let deploy =
            contract_factory.deploy(constructor_args, setup.config().deployer.salt, unique);

        info!(
            "Deploying ERC20 contract with nonce={}, address={:#064x}",
            nonce, address
        );

        let result = deploy.nonce(nonce).max_fee(MAX_FEE).send().await?;
        nonce += FieldElement::ONE;
        wait_for_tx(setup.rpc_client(), result.transaction_hash, CHECK_INTERVAL).await?;

        debug!(
            "Deploy ERC20 transaction accepted {:#064x}",
            result.transaction_hash
        );

        info!("ERC20 contract deployed at address {:#064x}", address);

        let mut joinset = JoinSet::new();

        for account in setup.accounts() {
            info!("Funding account at address {address:#064x}");

            let tx_hash = setup::transfer(
                setup.deployer_account().clone(),
                nonce,
                felt!("0xFFFFF"),
                address,
                account.address(),
            )
            .await?;

            nonce += FieldElement::ONE;
            let rpc_client = setup.rpc_client().clone();
            joinset.spawn(async move { wait_for_tx(&rpc_client, tx_hash, CHECK_INTERVAL).await });
        }

        while let Some(result) = joinset.join_next().await {
            result??;
        }

        Ok(TransferShooter {
            erc20_address: address,
            account: setup.deployer_account().clone(),
        })
    }

    fn get_amount(config: &GatlingConfig) -> u64 {
        config.run.num_erc20_transfers
    }

    fn get_execution_data(&self, _account: &StarknetAccount) -> Call {
        let (amount_low, amount_high) = (felt!("1"), felt!("0"));

        // Hex: 0xdead
        // from_hex_be isn't const whereas from_mont is
        const VOID_ADDRESS: FieldElement = FieldElement::from_mont([
            18446744073707727457,
            18446744073709551615,
            18446744073709551615,
            576460752272412784,
        ]);

        Call {
            to: self.erc20_address,
            selector: selector!("transfer"),
            calldata: vec![VOID_ADDRESS, amount_low, amount_high],
        }
    }
}
