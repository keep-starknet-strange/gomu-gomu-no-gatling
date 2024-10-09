use std::{collections::HashMap, sync::Arc};

use color_eyre::eyre::bail;
use starknet::{
    accounts::{Account, ConnectedAccount},
    contract::ContractFactory,
    core::{
        types::{BlockId, BlockTag, Call, Felt},
        utils::{get_udc_deployed_address, UdcUniqueness},
    },
    macros::{felt, selector},
    providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider},
};
use tokio::task::JoinSet;

use crate::{
    actions::setup::{GatlingSetup, StarknetAccount, CHECK_INTERVAL, MAX_FEE},
    generators::get_rng,
    utils::wait_for_tx,
};

use super::Shooter;

pub struct MintShooter {
    pub account_to_erc721_addresses: HashMap<Felt, Felt>,
    #[allow(unused)]
    pub recipient: StarknetAccount,
}

impl Shooter for MintShooter {
    const NAME: &'static str = "Erc721 Mints";

    async fn setup(setup: &mut GatlingSetup) -> color_eyre::Result<Self> {
        let erc721_class_hash = setup
            .declare_contract(&setup.config().setup.erc721_contract.clone())
            .await?;

        let deployer_salt = setup.config().deployer.salt;
        let mut join_set = JoinSet::new();

        for account in setup.accounts().iter().cloned() {
            let address = account.address();
            let rpc_client = setup.rpc_client().clone();
            join_set.spawn(async move {
                let contract =
                    Self::deploy_erc721(rpc_client, deployer_salt, erc721_class_hash, account)
                        .await;

                (address, contract)
            });
        }

        let mut map = HashMap::with_capacity(setup.accounts().len());
        while let Some((account_address, contract_result)) =
            join_set.join_next().await.transpose()?
        {
            map.insert(account_address, contract_result?);
        }

        Ok(Self {
            account_to_erc721_addresses: map,
            recipient: setup.deployer_account().clone(),
        })
    }

    fn get_execution_data(&self, account: &StarknetAccount) -> Call {
        let recipient = account.address();

        let (token_id_low, token_id_high) = (get_rng(), felt!("0x0000"));

        Call {
            to: self.account_to_erc721_addresses[&account.address()],
            selector: selector!("mint"),
            calldata: vec![recipient, token_id_low, token_id_high],
        }
    }
}

impl MintShooter {
    async fn deploy_erc721(
        starknet_rpc: Arc<JsonRpcClient<HttpTransport>>,
        deployer_salt: Felt,
        class_hash: Felt,
        recipient: StarknetAccount,
    ) -> color_eyre::Result<Felt> {
        let contract_factory = ContractFactory::new(class_hash, &recipient);

        let name = selector!("TestNFT");
        let symbol = selector!("TNFT");

        let constructor_args = vec![name, symbol, recipient.address()];

        let udc_uniqueness = UdcUniqueness::NotUnique;
        let unique = matches!(udc_uniqueness, UdcUniqueness::Unique(_));
        let address = get_udc_deployed_address(
            deployer_salt,
            class_hash,
            &UdcUniqueness::NotUnique,
            &constructor_args,
        );

        if let Ok(contract_class_hash) = starknet_rpc
            .get_class_hash_at(BlockId::Tag(BlockTag::Pending), address)
            .await
        {
            if contract_class_hash == class_hash {
                tracing::warn!("ERC721 contract already deployed at address {address:#064x}");
                return Ok(address);
            } else {
                bail!("ERC721 contract {address:#064x} already deployed with a different class hash {contract_class_hash:#064x}, expected {class_hash:#064x}");
            }
        }

        let deploy = contract_factory.deploy_v1(constructor_args, deployer_salt, unique);

        let nonce = recipient.get_nonce().await?;

        tracing::info!("Deploying ERC721 with nonce={}, address={address}", nonce);

        let result = deploy.nonce(nonce).max_fee(MAX_FEE).send().await?;
        wait_for_tx(&starknet_rpc, result.transaction_hash, CHECK_INTERVAL).await?;

        tracing::info!(
            "Deploy ERC721 transaction accepted {:#064x}",
            result.transaction_hash
        );

        tracing::info!("ERC721 contract deployed at address {:#064x}", address);
        Ok(address)
    }
}
