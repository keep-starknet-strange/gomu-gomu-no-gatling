use crate::config::{ContractSourceConfig, GatlingConfig};
use crate::utils::wait_for_tx;
use color_eyre::{
    eyre::{
        Context, {bail, eyre},
    },
    Result,
};

use starknet::core::types::contract::SierraClass;
use starknet::core::types::Call;
use tokio::task::JoinSet;

use std::path::Path;

use starknet::accounts::{
    Account, AccountFactory, ConnectedAccount, ExecutionEncoding, OpenZeppelinAccountFactory,
    SingleOwnerAccount,
};
use starknet::core::types::{
    contract::legacy::LegacyContractClass, BlockId, BlockTag, Felt, StarknetError,
};
use starknet::macros::{felt, selector};
use starknet::providers::ProviderError;
use starknet::providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider};
use starknet::signers::{LocalWallet, SigningKey};
use std::sync::Arc;
use std::time::Duration;

use url::Url;

// Used to bypass validation
pub static MAX_FEE: Felt = felt!("0x6efb28c75a0000");
pub static CHECK_INTERVAL: Duration = Duration::from_millis(500);

pub type StarknetAccount = SingleOwnerAccount<Arc<JsonRpcClient<HttpTransport>>, LocalWallet>;

pub struct GatlingSetup {
    config: GatlingConfig,
    starknet_rpc: Arc<JsonRpcClient<HttpTransport>>,
    signer: LocalWallet,
    deployer: StarknetAccount,
    accounts: Vec<StarknetAccount>,
}

impl GatlingSetup {
    pub async fn from_config(config: GatlingConfig) -> Result<Self> {
        let starknet_rpc: Arc<JsonRpcClient<HttpTransport>> =
            Arc::new(starknet_rpc_provider(Url::parse(&config.clone().rpc.url)?));

        let signer = LocalWallet::from(SigningKey::from_secret_scalar(config.deployer.signing_key));
        let mut deployer = SingleOwnerAccount::new(
            starknet_rpc.clone(),
            signer.clone(),
            config.deployer.address,
            config.setup.chain_id,
            if config.deployer.legacy_account {
                ExecutionEncoding::Legacy
            } else {
                ExecutionEncoding::New
            },
        );
        deployer.set_block_id(BlockId::Tag(BlockTag::Pending));

        Ok(Self {
            config,
            starknet_rpc,
            signer,
            deployer,
            accounts: vec![],
        })
    }

    pub fn config(&self) -> &GatlingConfig {
        &self.config
    }

    pub fn rpc_client(&self) -> &Arc<JsonRpcClient<HttpTransport>> {
        &self.starknet_rpc
    }

    pub fn deployer_account(&self) -> &StarknetAccount {
        &self.deployer
    }

    pub fn accounts(&self) -> &[StarknetAccount] {
        &self.accounts
    }

    /// Setup the simulation.
    pub async fn setup_accounts(&mut self) -> Result<()> {
        let account_contract = self.config.setup.account_contract.clone();

        let account_class_hash = self.declare_contract(&account_contract).await?;

        let execution_encoding = match account_contract {
            ContractSourceConfig::V0(_) => ExecutionEncoding::Legacy,
            ContractSourceConfig::V1(_) => ExecutionEncoding::New,
        };

        let accounts = self
            .create_accounts(
                account_class_hash,
                self.config.run.concurrency as usize,
                execution_encoding,
            )
            .await?;

        self.accounts = accounts;

        Ok(())
    }

    pub async fn transfer(
        &self,
        contract_address: Felt,
        deployer: StarknetAccount,
        recipient: Felt,
        amount: Felt,
        nonce: Felt,
    ) -> Result<Felt> {
        transfer(deployer, nonce, amount, contract_address, recipient).await
    }

    /// Create accounts.
    ///
    /// # Arguments
    ///
    /// * `class_hash` - The class hash of the account contract.
    /// * `num_accounts` - The number of accounts to create.
    /// * `execution_encoding` - Execution encoding to use, `Legacy` for Cairo Zero and `New` for Cairo
    ///
    /// # Returns
    ///
    /// A vector of the created accounts.
    async fn create_accounts<'a>(
        &mut self,
        class_hash: Felt,
        num_accounts: usize,
        execution_encoding: ExecutionEncoding,
    ) -> Result<Vec<StarknetAccount>> {
        tracing::info!("Creating {} accounts", num_accounts);

        let mut nonce = self.deployer.get_nonce().await?;
        let mut deployed_accounts: Vec<StarknetAccount> = Vec::with_capacity(num_accounts);

        let mut deployment_joinset = JoinSet::new();
        for i in 0..num_accounts {
            // TODO: Check if OpenZepplinAccountFactory could be used with other type of accounts ? or should we require users to use OpenZepplinAccountFactory ?
            let signer = self.signer.clone();
            let provider = self.starknet_rpc.clone();
            let account_factory = OpenZeppelinAccountFactory::new(
                class_hash,
                self.config.setup.chain_id,
                &signer,
                &provider,
            )
            .await?;

            let salt = self.config.deployer.salt + Felt::from(i);

            let deploy = account_factory.deploy_v1(salt).max_fee(MAX_FEE);
            let address = deploy.address();
            tracing::info!("Deploying account {i} with salt {salt} at address {address:#064x}");

            if let Ok(account_class_hash) = self
                .starknet_rpc
                .get_class_hash_at(BlockId::Tag(BlockTag::Pending), address)
                .await
            {
                if account_class_hash == class_hash {
                    tracing::warn!("Account {i} already deployed at address {address:#064x}");
                    let mut already_deployed_account = SingleOwnerAccount::new(
                        self.starknet_rpc.clone(),
                        signer.clone(),
                        address,
                        self.config.setup.chain_id,
                        execution_encoding,
                    );
                    already_deployed_account.set_block_id(BlockId::Tag(BlockTag::Pending));
                    deployed_accounts.push(already_deployed_account);
                    continue;
                } else {
                    bail!("Account {i} already deployed at address {address:#064x} with a different class hash {account_class_hash:#064x}, expected {class_hash:#064x}");
                }
            }

            let fee_token_address = self.config.setup.fee_token_address;
            let tx_hash = self
                .transfer(
                    fee_token_address,
                    self.deployer.clone(),
                    address,
                    felt!("0xFFFFFFFFFFFFFFF"),
                    nonce,
                )
                .await?;
            nonce += Felt::ONE;
            wait_for_tx(&self.starknet_rpc, tx_hash, CHECK_INTERVAL).await?;

            let result = deploy.send().await?;

            let mut new_account = SingleOwnerAccount::new(
                self.starknet_rpc.clone(),
                signer.clone(),
                result.contract_address,
                self.config.setup.chain_id,
                execution_encoding,
            );
            new_account.set_block_id(BlockId::Tag(BlockTag::Pending));
            deployed_accounts.push(new_account);

            let starknet_rpc = self.starknet_rpc.clone();
            deployment_joinset.spawn(async move {
                wait_for_tx(&starknet_rpc, result.transaction_hash, CHECK_INTERVAL).await
            });

            tracing::info!("Account {i} deployed at address {address:#064x}");
        }

        while let Some(result) = deployment_joinset.join_next().await {
            result??;
        }

        Ok(deployed_accounts)
    }

    async fn check_already_declared(&self, class_hash: Felt) -> Result<bool> {
        match self
            .starknet_rpc
            .get_class(BlockId::Tag(BlockTag::Pending), class_hash)
            .await
        {
            Ok(_) => {
                tracing::warn!("Contract already declared at {class_hash:#064x}");
                Ok(true)
            }
            Err(ProviderError::StarknetError(StarknetError::ClassHashNotFound)) => Ok(false),
            Err(err) => Err(eyre!(err)),
        }
    }

    async fn declare_contract_legacy<'a>(
        &mut self,
        contract_path: impl AsRef<Path>,
    ) -> Result<Felt> {
        tracing::info!(
            "Declaring contract from path {}",
            contract_path.as_ref().display()
        );
        let file = std::fs::File::open(contract_path)?;
        let contract_artifact: LegacyContractClass = serde_json::from_reader(file)?;
        let class_hash = contract_artifact.class_hash()?;

        if self.check_already_declared(class_hash).await? {
            return Ok(class_hash);
        }

        let nonce = self.deployer.get_nonce().await?;
        let tx_resp = self
            .deployer
            .declare_legacy(Arc::new(contract_artifact))
            .max_fee(MAX_FEE)
            .nonce(nonce)
            .send()
            .await
            .wrap_err("Could not declare contract")?;

        wait_for_tx(&self.starknet_rpc, tx_resp.transaction_hash, CHECK_INTERVAL).await?;

        tracing::info!(
            "Contract declared successfully at {:#064x}",
            tx_resp.class_hash
        );

        Ok(tx_resp.class_hash)
    }

    async fn declare_contract_v1<'a>(
        &mut self,
        contract_path: impl AsRef<Path>,
        casm_class_hash: Felt,
    ) -> Result<Felt> {
        let file = std::fs::File::open(contract_path.as_ref())?;
        let contract_artifact: SierraClass = serde_json::from_reader(file)?;
        let class_hash = contract_artifact.class_hash()?;

        // Sierra class artifact. Output of the `starknet-compile` command
        tracing::info!(
            "Declaring contract v1 from path {} with class hash {:#064x}",
            contract_path.as_ref().display(),
            class_hash
        );
        let nonce = self.deployer.get_nonce().await?;

        if self.check_already_declared(class_hash).await? {
            return Ok(class_hash);
        }

        self.deployer.set_block_id(BlockId::Tag(BlockTag::Pending));

        // We need to flatten the ABI into a string first
        let flattened_class = contract_artifact.flatten()?;

        let tx_resp = self
            .deployer
            .declare_v2(Arc::new(flattened_class), casm_class_hash)
            .max_fee(MAX_FEE)
            .nonce(nonce)
            .send()
            .await
            .wrap_err("Could not declare contract")?;

        wait_for_tx(&self.starknet_rpc, tx_resp.transaction_hash, CHECK_INTERVAL).await?;

        tracing::info!(
            "Contract declared successfully at {:#064x}",
            tx_resp.class_hash
        );

        Ok(tx_resp.class_hash)
    }

    pub async fn declare_contract(
        &mut self,
        contract_source: &crate::config::ContractSourceConfig,
    ) -> Result<Felt> {
        match contract_source {
            ContractSourceConfig::V0(path) => self.declare_contract_legacy(&path).await,
            ContractSourceConfig::V1(config) => {
                self.declare_contract_v1(&config.path, config.get_casm_hash()?)
                    .await
            }
        }
    }
}

pub async fn transfer(
    account: StarknetAccount,
    nonce: Felt,
    amount: Felt,
    contract_address: Felt,
    recipient: Felt,
) -> color_eyre::Result<Felt> {
    let from_address = account.address();

    tracing::info!(
        "Transfering {amount} of {contract_address:#064x} from address {from_address:#064x} to address {recipient:#064x} with nonce={}",
        nonce,
    );

    let (amount_low, amount_high) = (amount, felt!("0"));

    let call = Call {
        to: contract_address,
        selector: selector!("transfer"),
        calldata: vec![recipient, amount_low, amount_high],
    };

    let result = account
        .execute_v1(vec![call])
        .max_fee(MAX_FEE)
        .nonce(nonce)
        .send()
        .await?;

    Ok(result.transaction_hash)
}

/// Create a StarkNet RPC provider from a URL.
/// # Arguments
/// * `rpc` - The URL of the StarkNet RPC provider.
/// # Returns
/// A StarkNet RPC provider.
fn starknet_rpc_provider(rpc: Url) -> JsonRpcClient<HttpTransport> {
    JsonRpcClient::new(HttpTransport::new(rpc))
}
