use std::{collections::HashMap, sync::Arc};

use color_eyre::eyre::{bail, ensure};
use goose::{
    config::GooseConfiguration,
    goose::{Scenario, Transaction, TransactionFunction},
    transaction, GooseAttack,
};
use log::{debug, info, warn};
use starknet::{
    accounts::{Account, Call, ConnectedAccount},
    contract::ContractFactory,
    core::types::{BlockId, BlockTag, FieldElement, InvokeTransactionResult},
    macros::{felt, selector},
    providers::{
        jsonrpc::{HttpTransport, JsonRpcMethod},
        JsonRpcClient, Provider,
    },
};
use tokio::task::JoinSet;

use crate::{
    actions::{
        goose::{send_execution, GooseWriteUserState},
        setup::{CHECK_INTERVAL, MAX_FEE},
    },
    config::GatlingConfig,
    generators::get_rng,
    utils::{compute_contract_address, wait_for_tx},
};

use super::{
    goose::{goose_write_user_wait_last_tx, setup, verify_transactions},
    setup::{GatlingSetup, StarknetAccount},
};

pub trait Shooter {
    const NAME: &'static str;

    async fn setup(setup: &mut GatlingSetup) -> color_eyre::Result<Self>
    where
        Self: Sized;

    fn get_goose_config(config: &GatlingConfig) -> color_eyre::Result<GooseConfiguration>;

    async fn create_goose_attack(
        self: Arc<Self>,
        config: GooseConfiguration,
        accounts: Vec<StarknetAccount>,
    ) -> color_eyre::Result<GooseAttack>
    where
        Self: Send + Sync + 'static,
    {
        let setup: TransactionFunction = setup(accounts, config.iterations).await?;

        let submission: TransactionFunction = Self::execute(self.clone());

        let finalizing: TransactionFunction = goose_write_user_wait_last_tx();

        let attack = GooseAttack::initialize_with_config(config)?.register_scenario(
            Scenario::new(Self::NAME)
                .register_transaction(Transaction::new(setup).set_name("Setup").set_on_start())
                .register_transaction(
                    Transaction::new(submission)
                        .set_name("Transaction Submission")
                        .set_sequence(1),
                )
                .register_transaction(
                    Transaction::new(finalizing)
                        .set_name("Finalizing")
                        .set_sequence(2)
                        .set_on_stop(),
                )
                .register_transaction(
                    transaction!(verify_transactions)
                        .set_name("Verification")
                        .set_sequence(3)
                        .set_on_stop(),
                ),
        );

        Ok(attack)
    }

    fn execute(self: Arc<Self>) -> TransactionFunction
    where
        Self: Send + Sync + 'static,
    {
        Arc::new(move |user| {
            let shooter = self.clone();

            Box::pin(async move {
                let GooseWriteUserState { account, nonce, .. } = user
                    .get_session_data::<GooseWriteUserState>()
                    .expect("Should be in a goose user with GooseUserState session data");

                let call = shooter.get_execution_data(account);

                let response: InvokeTransactionResult = send_execution(
                    user,
                    vec![call],
                    *nonce,
                    &account.clone(),
                    JsonRpcMethod::AddInvokeTransaction,
                )
                .await?
                .0;

                let GooseWriteUserState { nonce, prev_tx, .. } =
                    user.get_session_data_mut::<GooseWriteUserState>().expect(
                        "Should be successful as we already asserted that the session data is a GooseUserState",
                    );

                *nonce += FieldElement::ONE;

                prev_tx.push(response.transaction_hash);

                Ok(())
            })
        })
    }

    fn get_execution_data(&self, account: &StarknetAccount) -> Call;
}

pub struct TransferShooter {
    pub erc20_address: FieldElement,
    pub account: StarknetAccount,
}

pub struct MintShooter {
    pub account_to_erc721_addresses: HashMap<FieldElement, FieldElement>,
    pub recipient: StarknetAccount,
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
        let nonce = setup.deployer_account().get_nonce().await?;

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
        wait_for_tx(setup.rpc_client(), result.transaction_hash, CHECK_INTERVAL).await?;

        debug!(
            "Deploy ERC20 transaction accepted {:#064x}",
            result.transaction_hash
        );

        info!("ERC20 contract deployed at address {:#064x}", address);

        Ok(TransferShooter {
            erc20_address: address,
            account: setup.deployer_account().clone(),
        })
    }

    fn get_goose_config(config: &GatlingConfig) -> color_eyre::Result<GooseConfiguration> {
        ensure!(
            config.run.num_erc20_transfers >= config.run.concurrency,
            "Too few erc20 transfers for the amount of concurrent users"
        );

        // div_euclid will truncate integers when not evenly divisable
        let user_iterations = config
            .run
            .num_erc20_transfers
            .div_euclid(config.run.concurrency);
        // this will always be a multiple of concurrency, unlike num_erc20_transfers
        let total_transactions = user_iterations * config.run.concurrency;

        // If these are not equal that means user_iterations was truncated
        if total_transactions != config.run.num_erc20_transfers {
            log::warn!("Number of erc20 transfers is not evenly divisble by concurrency, doing {total_transactions} transfers instead");
        }

        {
            let mut default = GooseConfiguration::default();
            default.host = config.rpc.url.clone();
            default.iterations = user_iterations as usize;
            default.users = Some(config.run.concurrency as usize);
            Ok(default)
        }
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

    fn get_goose_config(config: &GatlingConfig) -> color_eyre::Result<GooseConfiguration> {
        ensure!(
            config.run.num_erc721_mints >= config.run.concurrency,
            "Too few erc721 mints for the amount of concurrent users"
        );

        // div_euclid will truncate integers when not evenly divisable
        let user_iterations = config
            .run
            .num_erc721_mints
            .div_euclid(config.run.concurrency);
        // this will always be a multiple of concurrency, unlike num_erc721_mints
        let total_transactions = user_iterations * config.run.concurrency;

        // If these are not equal that means user_iterations was truncated
        if total_transactions != config.run.num_erc721_mints {
            log::warn!("Number of erc721 mints is not evenly divisble by concurrency, doing {total_transactions} mints instead");
        }

        {
            let mut default = GooseConfiguration::default();
            default.host = config.rpc.url.clone();
            default.iterations = user_iterations as usize;
            default.users = Some(config.run.concurrency as usize);
            Ok(default)
        }
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
        deployer_salt: FieldElement,
        class_hash: FieldElement,
        recipient: StarknetAccount,
    ) -> color_eyre::Result<FieldElement> {
        let contract_factory = ContractFactory::new(class_hash, &recipient);

        let name = selector!("TestNFT");
        let symbol = selector!("TNFT");

        let constructor_args = vec![name, symbol, recipient.address()];
        let unique = false;

        let address = compute_contract_address(deployer_salt, class_hash, &constructor_args);

        if let Ok(contract_class_hash) = starknet_rpc
            .get_class_hash_at(BlockId::Tag(BlockTag::Pending), address)
            .await
        {
            if contract_class_hash == class_hash {
                warn!("ERC721 contract already deployed at address {address:#064x}");
                return Ok(address);
            } else {
                bail!("ERC721 contract {address:#064x} already deployed with a different class hash {contract_class_hash:#064x}, expected {class_hash:#064x}");
            }
        }

        let deploy = contract_factory.deploy(constructor_args, deployer_salt, unique);

        let nonce = recipient.get_nonce().await?;

        info!("Deploying ERC721 with nonce={}, address={address}", nonce);

        let result = deploy.nonce(nonce).max_fee(MAX_FEE).send().await?;
        wait_for_tx(&starknet_rpc, result.transaction_hash, CHECK_INTERVAL).await?;

        debug!(
            "Deploy ERC721 transaction accepted {:#064x}",
            result.transaction_hash
        );

        info!("ERC721 contract deployed at address {:#064x}", address);
        Ok(address)
    }
}
