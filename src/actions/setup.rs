use crate::config::{ContractSourceConfig, GatlingConfig};
use crate::utils::wait_for_tx;
use color_eyre::{
    eyre::{
        Context, {bail, eyre},
    },
    Result,
};

use log::{debug, info, warn};
use starknet::core::types::contract::SierraClass;

use std::path::Path;

use starknet::accounts::{
    Account, AccountFactory, Call, ConnectedAccount, ExecutionEncoding, OpenZeppelinAccountFactory,
    SingleOwnerAccount,
};
use starknet::core::types::{
    contract::legacy::LegacyContractClass, BlockId, BlockTag, FieldElement, StarknetError,
};
use starknet::macros::{felt, selector};
use starknet::providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider};
use starknet::providers::{MaybeUnknownErrorCode, ProviderError, StarknetErrorWithMessage};
use starknet::signers::{LocalWallet, SigningKey};
use std::sync::Arc;
use std::time::Duration;

use url::Url;

// Used to bypass validation
pub static MAX_FEE: FieldElement = felt!("0x6efb28c75a0000");
pub static CHECK_INTERVAL: Duration = Duration::from_millis(500);

pub type StarknetAccount = SingleOwnerAccount<Arc<JsonRpcClient<HttpTransport>>, LocalWallet>;

pub struct GatlingSetup {
    config: GatlingConfig,
    starknet_rpc: Arc<JsonRpcClient<HttpTransport>>,
    signer: LocalWallet,
    account: StarknetAccount,
    accounts: Vec<StarknetAccount>,
}

impl GatlingSetup {
    pub async fn from_config(config: GatlingConfig) -> Result<Self> {
        let starknet_rpc: Arc<JsonRpcClient<HttpTransport>> =
            Arc::new(starknet_rpc_provider(Url::parse(&config.clone().rpc.url)?));

        let signer = LocalWallet::from(SigningKey::from_secret_scalar(config.deployer.signing_key));

        let account = SingleOwnerAccount::new(
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

        Ok(Self {
            config,
            starknet_rpc,
            signer,
            account,
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
        &self.account
    }

    pub fn accounts(&self) -> &[StarknetAccount] {
        &self.accounts
    }

    /// Setup the simulation.
    pub async fn setup_accounts(&mut self, erc20_address: FieldElement) -> Result<()> {
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
                erc20_address,
            )
            .await?;

        self.accounts = accounts;

        Ok(())
    }

    async fn transfer(
        &mut self,
        contract_address: FieldElement,
        account: StarknetAccount,
        recipient: FieldElement,
        amount: FieldElement,
        nonce: FieldElement,
    ) -> Result<FieldElement> {
        let from_address = account.address();

        debug!(
            "Transferring {amount} of {contract_address:#064x} from address {from_address:#064x} to address {recipient:#064x} with nonce={}",
            nonce,
        );

        let (amount_low, amount_high) = (amount, felt!("0"));

        let call = Call {
            to: contract_address,
            selector: selector!("transfer"),
            calldata: vec![recipient, amount_low, amount_high],
        };

        let result = account
            .execute(vec![call])
            .max_fee(MAX_FEE)
            .nonce(nonce)
            .send()
            .await?;

        Ok(result.transaction_hash)
    }

    /// Create accounts.
    ///
    /// # Arguments
    ///
    /// * `class_hash` - The class hash of the account contract.
    /// * `num_accounts` - The number of accounts to create.
    /// * `execution_encoding` - Execution encoding to use, `Legacy` for Cairo Zero and `New` for Cairo
    /// * `erc20_address` - The address of the ERC20 contract to use for funding the accounts.
    ///
    /// # Returns
    ///
    /// A vector of the created accounts.
    async fn create_accounts<'a>(
        &mut self,
        class_hash: FieldElement,
        num_accounts: usize,
        execution_encoding: ExecutionEncoding,
        erc20_address: FieldElement,
    ) -> Result<Vec<StarknetAccount>> {
        info!("Creating {} accounts", num_accounts);

        let mut deployed_accounts: Vec<StarknetAccount> = Vec::with_capacity(num_accounts);

        let mut nonce = self.account.get_nonce().await?;

        for i in 0..num_accounts {
            self.account.set_block_id(BlockId::Tag(BlockTag::Pending));

            let fee_token_address = self.config.setup.fee_token_address;

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

            let salt = self.config.deployer.salt + FieldElement::from(i);

            let deploy = account_factory.deploy(salt).max_fee(MAX_FEE);
            let address = deploy.address();
            info!("Deploying account {i} with salt {salt} at address {address:#064x}");

            if let Ok(account_class_hash) = self
                .starknet_rpc
                .get_class_hash_at(BlockId::Tag(BlockTag::Pending), address)
                .await
            {
                if account_class_hash == class_hash {
                    warn!("Account {i} already deployed at address {address:#064x}");
                    let account = SingleOwnerAccount::new(
                        self.starknet_rpc.clone(),
                        signer.clone(),
                        address,
                        self.config.setup.chain_id,
                        execution_encoding,
                    );
                    deployed_accounts.push(account);
                    continue;
                } else {
                    bail!("Account {i} already deployed at address {address:#064x} with a different class hash {account_class_hash:#064x}, expected {class_hash:#064x}");
                }
            }

            info!("Funding account {i} at address {address:#064x}");
            let tx_hash = self
                .transfer(
                    erc20_address,
                    self.account.clone(),
                    address,
                    felt!("0xFFF"),
                    nonce,
                )
                .await?;
            nonce += FieldElement::ONE;
            wait_for_tx(&self.starknet_rpc, tx_hash, CHECK_INTERVAL).await?;
            let tx_hash = self
                .transfer(
                    fee_token_address,
                    self.account.clone(),
                    address,
                    felt!("0xFFFFFFFFFFFFFFFFFFFF"),
                    nonce,
                )
                .await?;
            nonce += FieldElement::ONE;
            wait_for_tx(&self.starknet_rpc, tx_hash, CHECK_INTERVAL).await?;

            let result = deploy.send().await?;

            let account = SingleOwnerAccount::new(
                self.starknet_rpc.clone(),
                signer.clone(),
                result.contract_address,
                self.config.setup.chain_id,
                execution_encoding,
            );

            deployed_accounts.push(account);

            wait_for_tx(&self.starknet_rpc, result.transaction_hash, CHECK_INTERVAL).await?;

            info!("Account {i} deployed at address {address:#064x}");
        }

        Ok(deployed_accounts)
    }

    async fn check_already_declared(&self, class_hash: FieldElement) -> Result<bool> {
        match self
            .starknet_rpc
            .get_class(BlockId::Tag(BlockTag::Pending), class_hash)
            .await
        {
            Ok(_) => {
                warn!("Contract already declared at {class_hash:#064x}");
                Ok(true)
            }
            Err(ProviderError::StarknetError(StarknetErrorWithMessage {
                code: MaybeUnknownErrorCode::Known(StarknetError::ClassHashNotFound),
                ..
            })) => Ok(false),
            Err(err) => Err(eyre!(err)),
        }
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
        let class_hash = contract_artifact.class_hash()?;

        if self.check_already_declared(class_hash).await? {
            return Ok(class_hash);
        }

        self.account.set_block_id(BlockId::Tag(BlockTag::Pending));
        let nonce = self.account.get_nonce().await?;

        let tx_resp = self
            .account
            .declare_legacy(Arc::new(contract_artifact))
            .max_fee(MAX_FEE)
            .nonce(nonce)
            .send()
            .await
            .wrap_err("Could not declare contract")?;

        wait_for_tx(&self.starknet_rpc, tx_resp.transaction_hash, CHECK_INTERVAL).await?;

        info!(
            "Contract declared successfully at {:#064x}",
            tx_resp.class_hash
        );

        Ok(tx_resp.class_hash)
    }

    async fn declare_contract_v1<'a>(
        &mut self,
        contract_path: impl AsRef<Path>,
        casm_class_hash: FieldElement,
    ) -> Result<FieldElement> {
        let file = std::fs::File::open(contract_path.as_ref())?;
        let contract_artifact: SierraClass = serde_json::from_reader(file)?;
        let class_hash = contract_artifact.class_hash()?;

        // Sierra class artifact. Output of the `starknet-compile` command
        info!(
            "Declaring contract v1 from path {} with class hash {:#064x}",
            contract_path.as_ref().display(),
            class_hash
        );
        let nonce = self.account.get_nonce().await?;

        if self.check_already_declared(class_hash).await? {
            return Ok(class_hash);
        }

        // `SingleOwnerAccount` defaults to checking nonce and estimating fees against the latest
        // block. Optionally change the target block to pending with the following line:
        self.account.set_block_id(BlockId::Tag(BlockTag::Pending));

        // We need to flatten the ABI into a string first
        let flattened_class = contract_artifact.flatten()?;

        let tx_resp = self
            .account
            .declare(Arc::new(flattened_class), casm_class_hash)
            .max_fee(MAX_FEE)
            .nonce(nonce)
            .send()
            .await
            .wrap_err("Could not declare contract")?;

        wait_for_tx(&self.starknet_rpc, tx_resp.transaction_hash, CHECK_INTERVAL).await?;

        info!(
            "Contract declared successfully at {:#064x}",
            tx_resp.class_hash
        );

        Ok(tx_resp.class_hash)
    }

    pub async fn declare_contract(
        &mut self,
        contract_source: &crate::config::ContractSourceConfig,
    ) -> Result<FieldElement> {
        match contract_source {
            ContractSourceConfig::V0(path) => self.declare_contract_legacy(&path).await,
            ContractSourceConfig::V1(config) => {
                self.declare_contract_v1(&config.path, config.get_casm_hash()?)
                    .await
            }
        }
    }
}

/// Create a StarkNet RPC provider from a URL.
/// # Arguments
/// * `rpc` - The URL of the StarkNet RPC provider.
/// # Returns
/// A StarkNet RPC provider.
fn starknet_rpc_provider(rpc: Url) -> JsonRpcClient<HttpTransport> {
    JsonRpcClient::new(HttpTransport::new(rpc))
}
