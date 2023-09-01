use crate::config::GatlingConfig;
use crate::generators::get_rng;
use crate::utils::{compute_contract_address, get_sysinfo, pretty_print_hashmap, wait_for_tx};
use color_eyre::{eyre::eyre, Result};

use log::{debug, info, warn};

use std::collections::HashMap;
use std::path::Path;

use crate::metrics::compute_all_metrics;

use rand::seq::SliceRandom;

use starknet::accounts::{
    Account, AccountError, AccountFactory, Call, ConnectedAccount, OpenZeppelinAccountFactory,
    SingleOwnerAccount,
};
use starknet::contract::ContractFactory;
use starknet::core::chain_id;
use starknet::core::types::{
    contract::legacy::LegacyContractClass, BlockId, BlockTag, FieldElement, StarknetError,
};
use starknet::macros::{felt, selector};
use starknet::providers::ProviderError;
use starknet::providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider};
use starknet::providers::{MaybeUnknownErrorCode, StarknetErrorWithMessage};
use starknet::signers::{LocalWallet, SigningKey};
use std::str;
use std::sync::Arc;
use std::time::SystemTime;

use url::Url;

// Used to bypass validation
pub static MAX_FEE: FieldElement = felt!("0xfffffffffff");

/// Shoot the load test simulation.
pub async fn shoot(config: GatlingConfig) -> Result<SimulationReport> {
    info!("starting simulation with config: {:?}", config);
    let mut shooter = GatlingShooter::new(config).await?;
    let mut simulation_report = Default::default();
    // Trigger the setup phase.
    shooter.setup(&mut simulation_report).await?;

    // Run the simulation.
    shooter.run(&mut simulation_report).await?;

    // Trigger the teardown phase.
    shooter.teardown(&mut simulation_report).await?;

    Ok(simulation_report.clone())
}

pub struct GatlingShooter {
    config: GatlingConfig,
    starknet_rpc: Arc<JsonRpcClient<HttpTransport>>,
    signer: LocalWallet,
    account: SingleOwnerAccount<Arc<JsonRpcClient<HttpTransport>>, LocalWallet>,
    nonce: FieldElement,
    environment: Option<GatlingEnvironment>, // Will be populated in setup phase
}

#[derive(Clone)]
pub struct GatlingEnvironment {
    erc20_address: FieldElement,
    erc721_address: FieldElement,
    accounts: Vec<FieldElement>,
}

impl GatlingShooter {
    pub async fn new(config: GatlingConfig) -> Result<Self> {
        let starknet_rpc = Arc::new(starknet_rpc_provider(Url::parse(&config.clone().rpc.url)?));

        let signer = LocalWallet::from(SigningKey::from_secret_scalar(config.deployer.signing_key));

        let account = SingleOwnerAccount::new(
            starknet_rpc.clone(),
            signer.clone(),
            config.deployer.address,
            chain_id::TESTNET,
        );

        let nonce = account.get_nonce().await?;

        // TODO: Do we need signer and starknet_rpc, they are already part of account?
        Ok(Self {
            config,
            starknet_rpc,
            signer,
            account,
            nonce,
            environment: None,
        })
    }

    pub fn environment(&self) -> Result<GatlingEnvironment> {
        self.environment.clone().ok_or(eyre!(
            "Environment is not yet populated, you should run the setup function first"
        ))
    }

    /// Return a random account address from the ones deployed during the setup phase
    /// Or the deployer account address if no accounts were deployed or
    /// if the environment is not yet populated
    pub fn get_random_account_address(&self) -> FieldElement {
        match self.environment() {
            Ok(environment) => {
                let mut rng = rand::thread_rng();
                *environment
                    .accounts
                    .choose(&mut rng)
                    .unwrap_or(&self.account.address())
            }
            Err(_) => self.account.address(),
        }
    }

    /// Setup the simulation.
    async fn setup<'a>(&mut self, _simulation_report: &'a mut SimulationReport) -> Result<()> {
        let chain_id = self.starknet_rpc.chain_id().await?.to_bytes_be();
        let block_number = self.starknet_rpc.block_number().await?;
        info!(
            "Shoot - {} @ block number - {}",
            str::from_utf8(&chain_id)?.trim_start_matches('\0'),
            block_number
        );

        let setup_config = self.config.clone().setup;

        let erc20_class_hash = self
            .declare_contract_legacy(&setup_config.erc20_contract_path)
            .await?;

        let erc721_class_hash = self
            .declare_contract_legacy(&setup_config.erc721_contract_path)
            .await?;

        let account_class_hash = self
            .declare_contract_legacy(&setup_config.account_contract_path)
            .await?;

        let accounts = if setup_config.num_accounts > 0 {
            self.create_accounts(account_class_hash, setup_config.num_accounts)
                .await?
        } else {
            Vec::new()
        };

        let erc20_address = self.deploy_erc20(erc20_class_hash).await?;
        let erc721_address = self.deploy_erc721(erc721_class_hash).await?;

        let environment = GatlingEnvironment {
            erc20_address,
            erc721_address,
            accounts,
        };

        self.environment = Some(environment);

        Ok(())
    }

    /// Teardown the simulation.
    async fn teardown<'a>(&mut self, simulation_report: &'a mut SimulationReport) -> Result<()> {
        info!("Tearing down!");
        info!("=> System <=");
        pretty_print_hashmap(&get_sysinfo());

        info!("=> Metrics for ERC20 transfer <=");
        pretty_print_hashmap(&simulation_report.reports["erc20"]);

        info!("=> Metrics for ERC721 mint <=");
        pretty_print_hashmap(&simulation_report.reports["erc721"]);

        Ok(())
    }

    async fn check_transactions(&self, transactions: Vec<FieldElement>) {
        info!("Checking transactions ...");
        let now = SystemTime::now();
        for transaction in transactions {
            wait_for_tx(&self.starknet_rpc, transaction).await.unwrap();
            debug!("Transaction {:#064x} accepted", transaction)
        }
        info!(
            "Took {} seconds to check transactions",
            now.elapsed().unwrap().as_secs_f32()
        );
    }

    /// Get a Map of the number of transactions per block for the last `num_blocks` blocks
    /// This is meant to be used to calculate multiple metrics such as TPS and TPB
    /// without hitting the StarkNet RPC too many times
    // TODO: add a cache to avoid hitting the RPC too many times
    async fn get_num_tx_per_block(&self, num_blocks: u64) -> Result<HashMap<u64, u64>> {
        let mut map = HashMap::new();

        let latest = self.starknet_rpc.block_number().await?;

        for block_number in latest - num_blocks..latest {
            let n = self
                .starknet_rpc
                .get_block_transaction_count(BlockId::Number(block_number))
                .await?;

            map.insert(block_number, n);
        }

        Ok(map)
    }

    /// Run the simulation.
    async fn run<'a>(&mut self, simulation_report: &'a mut SimulationReport) -> Result<()> {
        info!("Firing !");
        let transactions = self.run_erc20().await?;
        self.check_transactions(transactions).await;
        // TODO: make it configurable
        let num_blocks = 4;

        let num_tx_per_block = self.get_num_tx_per_block(num_blocks).await?;
        let erc20_metrics = compute_all_metrics(num_tx_per_block);

        simulation_report.reports.insert(
            "erc20".to_string(),
            erc20_metrics
                .iter()
                .map(|(metric, value)| (metric.name.clone(), value.to_string()))
                .collect(),
        );

        let transactions = self.run_erc721().await?;
        self.check_transactions(transactions).await;

        let num_tx_per_block = self.get_num_tx_per_block(num_blocks).await?;
        let erc721_metrics = compute_all_metrics(num_tx_per_block);
        simulation_report.reports.insert(
            "erc721".to_string(),
            erc721_metrics
                .iter()
                .map(|(metric, value)| (metric.name.clone(), value.to_string()))
                .collect(),
        );

        Ok(())
    }

    async fn run_erc20(&mut self) -> Result<Vec<FieldElement>> {
        let environment = self.environment()?;

        let num_erc20_transfers = 1000;

        info!("Sending {num_erc20_transfers} ERC20 transfer transactions ...");

        let start = SystemTime::now();

        let mut transactions = Vec::new();

        for _ in 0..num_erc20_transfers {
            let transaction_hash = self
                .transfer(environment.erc20_address, self.get_random_account_address())
                .await?;
            transactions.push(transaction_hash);
        }

        let took = start.elapsed().unwrap().as_secs_f32();
        info!(
            "Took {} seconds to send {} transfer transactions, on average {} sent per second",
            took,
            num_erc20_transfers,
            num_erc20_transfers as f32 / took
        );

        Ok(transactions)
    }

    async fn run_erc721<'a>(&mut self) -> Result<Vec<FieldElement>> {
        let environment = self.environment()?;

        let num_erc721_mints = 1000;

        info!("Sending {num_erc721_mints} ERC721 mint transactions ...");

        let start = SystemTime::now();

        let mut transactions = Vec::new();

        for _ in 0..num_erc721_mints {
            let transaction_hash = self.mint(environment.erc721_address).await?;
            transactions.push(transaction_hash);
        }

        let took = start.elapsed().unwrap().as_secs_f32();
        info!(
            "Took {} seconds to send {} mint transactions, on average {} sent per second",
            took,
            num_erc721_mints,
            num_erc721_mints as f32 / took
        );

        Ok(transactions)
    }

    async fn transfer(
        &mut self,
        contract_address: FieldElement,
        account_address: FieldElement,
    ) -> Result<FieldElement> {
        debug!(
            "Transferring to address={:#064x} with nonce={}",
            account_address, self.nonce
        );

        let (amount_low, amount_high) = (felt!("0x1000000"), felt!("0x0"));

        let call = Call {
            to: contract_address,
            selector: selector!("transfer"),
            calldata: vec![account_address, amount_low, amount_high],
        };

        let result = self
            .account
            .execute(vec![call])
            .max_fee(MAX_FEE)
            .nonce(self.nonce)
            .send()
            .await?;

        self.nonce = self.nonce + felt!("1");

        Ok(result.transaction_hash)
    }

    async fn mint(&mut self, contract_address: FieldElement) -> Result<FieldElement> {
        debug!(
            "Minting for address={:#064x} with nonce={}",
            contract_address, self.nonce
        );

        let (token_id_low, token_id_high) = (get_rng(), felt!("0x0000"));

        let call = Call {
            to: contract_address,
            selector: selector!("mint"),
            calldata: vec![
                self.get_random_account_address(),
                token_id_low,
                token_id_high,
            ],
        };

        let result = self
            .account
            .execute(vec![call])
            .max_fee(MAX_FEE)
            .nonce(self.nonce)
            .send()
            .await?;

        self.nonce = self.nonce + felt!("1");

        Ok(result.transaction_hash)
    }

    async fn deploy_erc721(&mut self, class_hash: FieldElement) -> Result<FieldElement> {
        let contract_factory = ContractFactory::new(class_hash, self.account.clone());

        let salt = get_rng();

        let constructor_args = &[felt!("0xa1"), felt!("0xa2"), self.account.address()];
        let unique = false;

        let deploy = contract_factory.deploy(constructor_args, salt, unique);

        info!("Deploying ERC721 with nonce={}", self.nonce);

        let result = deploy.nonce(self.nonce).max_fee(MAX_FEE).send().await?;
        wait_for_tx(&self.starknet_rpc, result.transaction_hash).await?;

        self.nonce = self.nonce + felt!("1");

        debug!(
            "Deploy ERC721 transaction accepted {:#064x}",
            result.transaction_hash
        );

        let address = compute_contract_address(salt, class_hash, constructor_args);

        info!("ERC721 contract deployed at address {:#064x}", address);
        Ok(address)
    }

    async fn deploy_erc20(&mut self, class_hash: FieldElement) -> Result<FieldElement> {
        let contract_factory = ContractFactory::new(class_hash, self.account.clone());

        let salt = get_rng();

        let name = get_rng();
        let symbol = get_rng();
        let decimals = felt!("128");
        let (initial_supply_low, initial_supply_high) = (get_rng(), get_rng());
        let recipient = self.account.address();

        let constructor_args = &[
            name,
            symbol,
            decimals,
            initial_supply_low,
            initial_supply_high,
            recipient,
        ];
        let unique = false;

        let deploy = contract_factory.deploy(constructor_args, salt, unique);

        info!("Deploying ERC20 contract with nonce={}", self.nonce);

        let result = deploy.nonce(self.nonce).max_fee(MAX_FEE).send().await?;
        wait_for_tx(&self.starknet_rpc, result.transaction_hash).await?;

        self.nonce = self.nonce + felt!("1");

        debug!(
            "Deploy ERC20 transaction accepted {:#064x}",
            result.transaction_hash
        );

        let address = compute_contract_address(salt, class_hash, constructor_args);

        info!("ERC20 contract deployed at address {:#064x}", address);
        Ok(address)
    }

    /// Create accounts.
    async fn create_accounts<'a>(
        &mut self,
        class_hash: FieldElement,
        num_accounts: usize,
    ) -> Result<Vec<FieldElement>> {
        info!("Creating {} accounts", num_accounts);

        let mut accounts = Vec::with_capacity(num_accounts);

        for i in 0..num_accounts {
            self.account.set_block_id(BlockId::Tag(BlockTag::Pending));

            // TODO: Check if OpenZepplinAccountFactory could be used with other type of accounts ? Or should we require users to use OpenZepplinAccountFactory ?
            let account_factory = OpenZeppelinAccountFactory::new(
                class_hash,
                chain_id::TESTNET,
                &self.signer,
                &self.starknet_rpc,
            )
            .await?;

            let salt = get_rng();

            let deploy = account_factory.deploy(salt);
            info!("Deploying account {i} with salt={}", salt,);

            let result = deploy.send().await?;

            accounts.push(result.contract_address);

            wait_for_tx(&self.starknet_rpc, result.transaction_hash).await?;

            info!("Account {i} deployed at address {:#064x}", deploy.address());

            self.transfer(self.config.setup.fee_token_address, deploy.address())
                .await?;
        }

        Ok(accounts)
    }

    async fn declare_contract_legacy<'a>(
        &mut self,
        contract_path: impl AsRef<Path>,
    ) -> Result<FieldElement> {
        info!(
            "Declaring contract from path {}",
            contract_path.as_ref().display()
        );
        let file = std::fs::File::open(contract_path)?;

        let contract_artifact: LegacyContractClass = serde_json::from_reader(file)?;

        // TODO: get the class_hash from the already declared error
        let class_hash = contract_artifact.class_hash()?;

        self.account.set_block_id(BlockId::Tag(BlockTag::Pending));

        match self
            .account
            .declare_legacy(Arc::new(contract_artifact))
            .send()
            .await
        {
            Ok(tx_resp) => {
                info!(
                    "Contract declared successfully class_hash={:#064x}",
                    tx_resp.class_hash
                );
                Ok(tx_resp.class_hash)
            }
            Err(AccountError::Provider(ProviderError::StarknetError(
                StarknetErrorWithMessage {
                    code: MaybeUnknownErrorCode::Known(StarknetError::ClassAlreadyDeclared),
                    ..
                },
            ))) => {
                warn!("Contract already declared class_hash={:#064x}", class_hash);
                Ok(class_hash)
            }
            Err(e) => {
                panic!("Could not declare contract: {e}");
            }
        }
    }
}

/// The simulation report.
#[derive(Debug, Default, Clone)]
pub struct SimulationReport {
    pub chain_id: Option<FieldElement>,
    pub block_number: Option<u64>,
    pub reports: HashMap<String, HashMap<String, String>>,
}

/// Create a StarkNet RPC provider from a URL.
/// # Arguments
/// * `rpc` - The URL of the StarkNet RPC provider.
/// # Returns
/// A StarkNet RPC provider.
fn starknet_rpc_provider(rpc: Url) -> JsonRpcClient<HttpTransport> {
    JsonRpcClient::new(HttpTransport::new(rpc))
}
