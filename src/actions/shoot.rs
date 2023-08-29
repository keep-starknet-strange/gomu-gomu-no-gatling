use crate::config::{CreateAccounts, GatlingConfig};
use crate::generators::get_rng;
use crate::utils::{calculate_contract_address, get_sysinfo, pretty_print_hashmap, wait_for_tx};
use color_eyre::Result;

use log::{debug, info, warn};

use std::collections::HashMap;

use crate::metrics::compute_all_metrics;

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

// TODO: Read the contract addresses from the deployed contracts
// TODO: move the public key and max_fee to the config file
pub static NFT_TOKEN_ADDRESS: FieldElement =
    felt!("0x040e59c2c182a58fb0a74349bfa4769cbbcba32547591dd3fb1def8623997d02");
pub static FEE_TOKEN_ADDRESS: FieldElement =
    felt!("0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7");
pub static OZ_CLASS_HASH: FieldElement =
    felt!("0x045e3ac97b1c575540dbf6b6f089f390f00fa98928415bb91a27a43790b52f30");
pub static PUBLIC_KEY: FieldElement =
    felt!("0x3603a2692a2ae60abb343e832ee53b55d6b25f02a3ef1565ec691edc7a209b2");
pub static MAX_FEE: FieldElement = felt!("0xfffffffffff");

pub static NFT_CLASS_HASH: FieldElement = felt!("0x80000");

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
    erc721_address: Option<FieldElement>,
}

impl GatlingShooter {
    pub async fn new(config: GatlingConfig) -> Result<Self> {
        let starknet_rpc = Arc::new(starknet_rpc_provider(Url::parse(
            &config.clone().rpc.unwrap_or_default().url,
        )?));
        let deployer = config.clone().deployer.unwrap_or_default();

        let signer = LocalWallet::from(SigningKey::from_secret_scalar(
            FieldElement::from_hex_be(deployer.signing_key.as_str()).unwrap_or_default(),
        ));

        // implement let account = Arc::new(account); instead of signer
        let address = FieldElement::from_hex_be(deployer.address.as_str()).unwrap_or_default();

        let account = SingleOwnerAccount::new(
            starknet_rpc.clone(),
            signer.clone(),
            address,
            chain_id::TESTNET,
        );

        let nonce = account.get_nonce().await?;

        Ok(Self {
            config,
            starknet_rpc,
            signer,
            account,
            nonce,
            erc721_address: None,
        })
    }

    /// Setup the simulation.
    async fn setup<'a>(&mut self, simulation_report: &'a mut SimulationReport) -> Result<()> {
        let chain_id = self.starknet_rpc.chain_id().await?.to_bytes_be();
        let block_number = self.starknet_rpc.block_number().await?;
        info!(
            "Shoot - {} @ block number - {}",
            str::from_utf8(&chain_id)?.trim_start_matches('\0'),
            block_number
        );

        self.declare_contract_legacy("contracts/v0/ERC20.json")
            .await;

        self.declare_contract_legacy("contracts/v0/ERC721.json")
            .await;

        if let Some(setup) = self.config.clone().simulation.unwrap_or_default().setup {
            if let Some(create_accounts) = setup.create_accounts {
                self.declare_contract_legacy("contracts/v0/OpenzeppelinAccount.json")
                    .await;

                self.create_accounts(simulation_report, create_accounts)
                    .await?;
            }
        }

        let erc721_address = self.deploy_erc721().await?;

        self.erc721_address = Some(erc721_address);

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
        pretty_print_hashmap(&simulation_report.reports["erc20"]);

        Ok(())
    }

    async fn check_transactions(&self, transactions: Vec<FieldElement>) {
        info!("Checking transactions ...");
        let now = SystemTime::now();
        for transaction in transactions {
            let result = wait_for_tx(&self.starknet_rpc, transaction)
                .await
                .expect(format!("Transaction failed {transaction:#064x}").as_str());

            debug!("{:#?} {:#064x}", result, transaction)
        }
        info!(
            "Took {} seconds to check transactions",
            now.elapsed().unwrap().as_secs()
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
        let transactions = self.run_erc20(simulation_report).await?;
        self.check_transactions(transactions).await;
        // TODO: make it configurable
        let num_blocks = 5;

        let num_tx_per_block = self.get_num_tx_per_block(num_blocks).await?;
        let erc20_metrics = compute_all_metrics(num_tx_per_block);

        simulation_report.reports.insert(
            "erc20".to_string(),
            erc20_metrics
                .iter()
                .map(|(metric, value)| (metric.name.clone(), value.to_string()))
                .collect(),
        );

        // let nonce = self.account.get_nonce().await?;// + felt!("1");
        let transactions = self.run_erc721(simulation_report).await?;
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

    async fn run_erc20<'a>(
        &mut self,
        _simulation_report: &'a mut SimulationReport,
    ) -> Result<Vec<FieldElement>> {
        let num_erc20_transfers = 5;

        info!("Sending {num_erc20_transfers} ERC20 transfers ...");
        let _fail_fast = self.config.simulation.clone().unwrap_or_default().fail_fast;

        let address = felt!("0x0389a894d597c962b3873b703d9682e49b95ab1f0e5242a7dd512ecf7186294e");

        let start = SystemTime::now();

        let mut transactions = Vec::new();

        for _ in 0..num_erc20_transfers {
            let transaction_hash = self.transfer(address).await?;
            transactions.push(transaction_hash);
        }

        let mut took = start.elapsed().unwrap().as_secs();
        if took == 0 {
            took = 1;
        }

        info!(
            "Took {} seconds to send {} transfer transaction, on average {} sent per second",
            took,
            num_erc20_transfers,
            num_erc20_transfers / took
        );

        Ok(transactions)
    }

    async fn run_erc721<'a>(
        &mut self,
        _simulation_report: &'a mut SimulationReport,
    ) -> Result<Vec<FieldElement>> {
        let num_erc721_mints = 5;

        info!("Sending {num_erc721_mints} ERC721 mints ...");
        let _fail_fast = self.config.simulation.clone().unwrap_or_default().fail_fast;

        let start = SystemTime::now();

        let mut transactions = Vec::new();

        // TODO: don't use unwrap
        let address = self.erc721_address.unwrap();

        for _ in 0..num_erc721_mints {
            let token_id = get_rng();
            let transaction_hash = self.mint(token_id, address).await?;
            transactions.push(transaction_hash);
        }

        let mut took = start.elapsed().unwrap().as_secs();
        if took == 0 {
            took = 1;
        }

        info!(
            "Took {} seconds to send {} mint transaction, on average {} sent per second",
            took,
            num_erc721_mints,
            num_erc721_mints / took
        );
        Ok(transactions)
    }

    async fn transfer(&mut self, address: FieldElement) -> Result<FieldElement> {
        debug!(
            "Transferring to address={:#064x} with nonce={}",
            address, self.nonce
        );

        let call = Call {
            to: FEE_TOKEN_ADDRESS,
            selector: selector!("transfer"),
            calldata: vec![address, felt!("0x1000000"), felt!("0x0")],
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

    async fn mint(
        &mut self,
        token_id: FieldElement,
        nft_contract_address: FieldElement,
    ) -> Result<FieldElement> {
        debug!(
            "Minting token_id={} for address={:#064x} with nonce={}",
            token_id, nft_contract_address, self.nonce
        );

        let call = Call {
            to: nft_contract_address,
            selector: selector!("mint"),
            calldata: vec![felt!("0x2"), token_id, felt!("0x0")],
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

    async fn deploy_erc721(&mut self) -> Result<FieldElement> {
        let contract_factory = ContractFactory::new(NFT_CLASS_HASH, self.account.clone());

        let salt = get_rng();

        let constructor_args = &[felt!("0xa1"), felt!("0xa2"), self.account.address()];
        let unique = false;

        let deploy = contract_factory.deploy(constructor_args, salt, unique);

        let max_fee = MAX_FEE + felt!("1");

        info!(
            "Deploying erc721 with nonce={:#064x} and max_fee={max_fee:#064x}",
            self.nonce
        );

        let result = deploy.nonce(self.nonce).max_fee(max_fee).send().await?;
        self.nonce = self.nonce + felt!("1");
        info!("{result:#?}");

        let result_str = wait_for_tx(&self.starknet_rpc, result.transaction_hash).await?;
        info!(
            "result_str={result_str:#?}, transaction_hash={:#064x}",
            result.transaction_hash
        );

        let address = calculate_contract_address(salt, NFT_CLASS_HASH, constructor_args);

        info!("Calculated address={:#064x}", address);
        Ok(address)
    }

    /// Create accounts.
    async fn create_accounts<'a>(
        &mut self,
        _simulation_report: &'a mut SimulationReport,
        account_details: CreateAccounts,
    ) -> Result<()> {
        info!("Creating {} accounts", account_details.num_accounts);

        // let salt = &self.config.deployer.as_ref().unwrap().salt;

        for i in 0..account_details.num_accounts {
            self.account.set_block_id(BlockId::Tag(BlockTag::Pending));

            let account_factory = OpenZeppelinAccountFactory::new(
                OZ_CLASS_HASH,
                chain_id::TESTNET,
                &self.signer,
                &self.starknet_rpc,
            )
            .await?;

            let salt = get_rng();

            let deploy = account_factory.deploy(salt);
            info!(
                "Deploying account {i} with salt={} address={:#064x}",
                salt,
                deploy.address()
            );

            let result = deploy.send().await?;

            debug!("Waiting for deploy account tx");
            wait_for_tx(&self.starknet_rpc, result.transaction_hash).await?;

            self.transfer(deploy.address()).await?;
        }

        Ok(())
    }

    async fn declare_contract_legacy<'a>(&mut self, contract_path: &str) {
        let contract_artifact: LegacyContractClass =
            serde_json::from_reader(std::fs::File::open(contract_path).unwrap()).unwrap();

        self.account.set_block_id(BlockId::Tag(BlockTag::Pending));

        let class_hash = contract_artifact.class_hash();

        match self
            .account
            .declare_legacy(Arc::new(contract_artifact))
            .send()
            .await
        {
            Ok(tx_resp) => {
                info!("Declared Contract TX: {:?}", tx_resp.transaction_hash);
            }
            Err(AccountError::Provider(ProviderError::StarknetError(
                StarknetErrorWithMessage {
                    code: MaybeUnknownErrorCode::Known(StarknetError::ClassAlreadyDeclared),
                    ..
                },
            ))) => {
                warn!("Contract already declared class_hash={:?}", class_hash);
            }
            Err(e) => {
                panic!("Could not declare contract: {e}");
            }
        };
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
