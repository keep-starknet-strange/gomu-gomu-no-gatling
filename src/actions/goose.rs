use std::{mem, sync::Arc};

use color_eyre::eyre::ensure;
use crossbeam_queue::ArrayQueue;
use goose::{config::GooseConfiguration, prelude::*};
use serde::{de::DeserializeOwned, Serialize};
use starknet::{
    accounts::{
        Account, Call, ConnectedAccount, ExecutionEncoder, RawExecution, SingleOwnerAccount,
    },
    core::types::{
        ExecutionResult, FieldElement, InvokeTransactionResult, MaybePendingTransactionReceipt,
    },
    macros::{felt, selector},
    providers::{
        jsonrpc::{
            HttpTransport, HttpTransportError, JsonRpcClientError, JsonRpcMethod, JsonRpcResponse,
        },
        JsonRpcClient, ProviderError,
    },
    signers::LocalWallet,
};

use crate::{
    actions::shoot::{GatlingShooterSetup, CHECK_INTERVAL, MAX_FEE},
    generators::get_rng,
    utils::wait_for_tx,
};

use super::shoot::StarknetAccount;

pub async fn erc20(shooter: &GatlingShooterSetup) -> color_eyre::Result<()> {
    let environment = shooter.environment()?;
    let erc20_address = environment.erc20_address;
    let config = shooter.config();

    ensure!(
        config.run.num_erc20_transfers >= config.run.concurrency,
        "Too few erc20 transfers for the amount of concurrency"
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

    let goose_config = {
        let mut default = GooseConfiguration::default();
        default.host = config.rpc.url.clone();
        default.iterations = user_iterations as usize;
        default.users = Some(config.run.concurrency as usize);
        default
    };

    let transfer_setup: TransactionFunction =
        setup(environment.accounts.clone(), user_iterations as usize).await?;

    let transfer: TransactionFunction =
        Arc::new(move |user| Box::pin(transfer(user, erc20_address)));

    let transfer_wait: TransactionFunction = goose_user_wait_last_tx(shooter.rpc_client().clone());

    GooseAttack::initialize_with_config(goose_config.clone())?
        .register_scenario(
            scenario!("Transfer")
                .register_transaction(
                    Transaction::new(transfer_setup)
                        .set_name("Transfer Setup")
                        .set_on_start(),
                )
                .register_transaction(
                    Transaction::new(transfer)
                        .set_name("Transfer")
                        .set_sequence(1),
                )
                .register_transaction(
                    Transaction::new(transfer_wait)
                        .set_name("Transfer Finalizing")
                        .set_sequence(2)
                        .set_on_stop(),
                )
                .register_transaction(
                    transaction!(verify_transactions)
                        .set_name("Transfer Verification")
                        .set_sequence(3)
                        .set_on_stop(),
                ),
        )
        .execute()
        .await?;

    Ok(())
}

pub async fn erc721(shooter: &GatlingShooterSetup) -> color_eyre::Result<()> {
    let config = shooter.config();
    let environment = shooter.environment()?;

    ensure!(
        config.run.num_erc20_transfers >= config.run.concurrency,
        "Too few erc721 mints for the amount of concurrency"
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

    let goose_mint_config = {
        let mut default = GooseConfiguration::default();
        default.host = config.rpc.url.clone();
        default.iterations = user_iterations as usize;
        default.users = Some(config.run.concurrency as usize);
        default
    };

    let nonces = Arc::new(ArrayQueue::new(total_transactions as usize));
    let erc721_address = environment.erc721_address;
    let mut nonce = shooter.deployer_account().get_nonce().await?;

    for _ in 0..total_transactions {
        nonces
            .push(nonce)
            .expect("ArrayQueue has capacity for all mints");
        nonce += FieldElement::ONE;
    }

    let from_account = shooter.deployer_account().clone();

    let mint_setup: TransactionFunction =
        setup(environment.accounts.clone(), user_iterations as usize).await?;

    let mint: TransactionFunction = Arc::new(move |user| {
        let nonce = nonces
            .pop()
            .expect("Nonce ArrayQueue should have enough nonces for all mints");
        let from_account = from_account.clone();
        Box::pin(async move { mint(user, erc721_address, nonce, &from_account).await })
    });

    let mint_wait: TransactionFunction = goose_user_wait_last_tx(shooter.rpc_client().clone());

    GooseAttack::initialize_with_config(goose_mint_config.clone())?
        .register_scenario(
            scenario!("Minting")
                .register_transaction(
                    Transaction::new(mint_setup)
                        .set_name("Mint Setup")
                        .set_on_start(),
                )
                .register_transaction(Transaction::new(mint).set_name("Minting").set_sequence(1))
                .register_transaction(
                    Transaction::new(mint_wait)
                        .set_name("Mint Finalizing")
                        .set_sequence(2)
                        .set_on_stop(),
                )
                .register_transaction(
                    transaction!(verify_transactions)
                        .set_name("Mint Verification")
                        .set_sequence(3)
                        .set_on_stop(),
                ),
        )
        .execute()
        .await?;

    Ok(())
}

#[derive(Debug, Clone)]
struct GooseUserState {
    account: StarknetAccount,
    nonce: FieldElement,
    prev_tx: Vec<FieldElement>,
}

pub type RpcError = ProviderError<JsonRpcClientError<HttpTransportError>>;

impl GooseUserState {
    pub async fn new(
        account: StarknetAccount,
        transactions_amount: usize,
    ) -> Result<Self, RpcError> {
        Ok(Self {
            nonce: account.get_nonce().await?,
            account,
            prev_tx: Vec::with_capacity(transactions_amount),
        })
    }
}

async fn setup(
    accounts: Vec<StarknetAccount>,
    transactions_amount: usize,
) -> Result<TransactionFunction, RpcError> {
    let queue = ArrayQueue::new(accounts.len());
    for account in accounts {
        queue
            .push(GooseUserState::new(account, transactions_amount).await?)
            .expect("Queue should have enough space for all accounts as it's length is from the accounts vec");
    }
    let queue = Arc::new(queue);

    Ok(Arc::new(move |user| {
        let queue = queue.clone();
        user.set_session_data(
            queue
                .pop()
                .expect("Not enough accounts were created for the amount of users"),
        );

        Box::pin(async { Ok(()) })
    }))
}

fn goose_user_wait_last_tx(provider: Arc<JsonRpcClient<HttpTransport>>) -> TransactionFunction {
    Arc::new(move |user| {
        let thing = user
            .get_session_data::<GooseUserState>()
            .expect("Should be in a goose user with GooseUserState session data")
            .prev_tx
            .last()
            .expect("At least one transaction should have been done");

        let provider = provider.clone();

        Box::pin(async move {
            wait_for_tx(&provider, *thing, CHECK_INTERVAL)
                .await
                .expect("Transaction should have been successful");

            Ok(())
        })
    })
}

// Hex: 0xdead
// from_hex_be isn't const whereas from_mont is
const VOID_ADDRESS: FieldElement = FieldElement::from_mont([
    18446744073707727457,
    18446744073709551615,
    18446744073709551615,
    576460752272412784,
]);

async fn transfer(user: &mut GooseUser, erc20_address: FieldElement) -> TransactionResult {
    let GooseUserState { account, nonce, .. } = user
        .get_session_data::<GooseUserState>()
        .expect("Should be in a goose user with GooseUserState session data");

    let (amount_low, amount_high) = (felt!("1"), felt!("0"));

    let call = Call {
        to: erc20_address,
        selector: selector!("transfer"),
        calldata: vec![VOID_ADDRESS, amount_low, amount_high],
    };

    let response: InvokeTransactionResult = send_execution(
        user,
        vec![call],
        *nonce,
        &account.clone(),
        JsonRpcMethod::AddInvokeTransaction,
    )
    .await?;

    let GooseUserState { nonce, prev_tx, .. } =
        user.get_session_data_mut::<GooseUserState>().expect(
            "Should be successful as we already asserted that the session data is a GooseUserState",
        );

    *nonce += FieldElement::ONE;

    prev_tx.push(response.transaction_hash);

    Ok(())
}

async fn mint(
    user: &mut GooseUser,
    erc721_address: FieldElement,
    nonce: FieldElement,
    from_account: &SingleOwnerAccount<Arc<JsonRpcClient<HttpTransport>>, LocalWallet>,
) -> TransactionResult {
    let recipient = user
        .get_session_data::<GooseUserState>()
        .expect("Should be in a goose user with GooseUserState session data")
        .account
        .clone()
        .address();

    let (token_id_low, token_id_high) = (get_rng(), felt!("0x0000"));

    let call = Call {
        to: erc721_address,
        selector: selector!("mint"),
        calldata: vec![recipient, token_id_low, token_id_high],
    };

    let response: InvokeTransactionResult = send_execution(
        user,
        vec![call],
        nonce,
        from_account,
        JsonRpcMethod::AddInvokeTransaction,
    )
    .await?;

    user.get_session_data_mut::<GooseUserState>()
        .expect(
            "Should be successful as we already asserted that the session data is a GooseUserState",
        )
        .prev_tx
        .push(response.transaction_hash);

    Ok(())
}

async fn verify_transactions(user: &mut GooseUser) -> TransactionResult {
    let transactions = mem::take(
        &mut user
            .get_session_data_mut::<GooseUserState>()
            .expect("Should be in a goose user with GooseUserState session data")
            .prev_tx,
    );

    for tx in transactions {
        let receipt: MaybePendingTransactionReceipt =
            send_request(user, JsonRpcMethod::GetTransactionReceipt, tx).await?;

        match receipt {
            MaybePendingTransactionReceipt::Receipt(receipt) => match receipt.execution_result() {
                ExecutionResult::Succeeded => {}
                ExecutionResult::Reverted { reason } => {
                    panic!("Transaction {tx:#064x} has been rejected/reverted: {reason}");
                }
            },
            MaybePendingTransactionReceipt::PendingReceipt(_) => {
                panic!("Transaction {tx:#064x} is pending when no transactions should be")
            }
        }
    }

    Ok(())
}

pub async fn send_execution<T: DeserializeOwned>(
    user: &mut GooseUser,
    calls: Vec<Call>,
    nonce: FieldElement,
    from_account: &SingleOwnerAccount<Arc<JsonRpcClient<HttpTransport>>, LocalWallet>,
    method: JsonRpcMethod,
) -> Result<T, Box<TransactionError>> {
    let calldata = from_account.encode_calls(&calls);

    #[allow(dead_code)] // Removes warning for unused fields, we need them to properly transmute
    struct FakeRawExecution {
        calls: Vec<Call>,
        nonce: FieldElement,
        max_fee: FieldElement,
    }

    let raw_exec = FakeRawExecution {
        calls,
        nonce,
        max_fee: MAX_FEE,
    };

    // TODO: We cannot right now construct RawExecution directly and need to use this hack
    // see https://github.com/xJonathanLEI/starknet-rs/issues/538
    let raw_exec = unsafe { mem::transmute::<FakeRawExecution, RawExecution>(raw_exec) };

    let param = starknet::core::types::BroadcastedInvokeTransaction {
        sender_address: from_account.address(),
        calldata,
        max_fee: MAX_FEE,
        signature: from_account
            .sign_execution(&raw_exec)
            .await
            .expect("Raw Execution should be correctly constructed for signature"),
        nonce,
        is_query: false,
    };

    send_request(user, method, param).await
}

pub async fn send_request<T: DeserializeOwned>(
    user: &mut GooseUser,
    method: JsonRpcMethod,
    param: impl Serialize,
) -> Result<T, Box<TransactionError>> {
    // Copied from https://docs.rs/starknet-providers/0.9.0/src/starknet_providers/jsonrpc/transports/http.rs.html#21-27
    #[derive(Debug, Serialize)]
    struct JsonRpcRequest<T> {
        id: u64,
        jsonrpc: &'static str,
        method: JsonRpcMethod,
        params: T,
    }

    let request = JsonRpcRequest {
        id: 1,
        jsonrpc: "2.0",
        method,
        params: [param],
    };

    let body = user
        .post_json("/", &request)
        .await?
        .response
        .map_err(TransactionError::Reqwest)?
        .json::<JsonRpcResponse<T>>()
        .await
        .map_err(TransactionError::Reqwest)?;

    match body {
        JsonRpcResponse::Success { result, .. } => Ok(result),
        // Returning this error would probably be a good idea,
        // but the goose error type doesn't allow it and we are
        // required to return it as a constraint of TransactionFunction
        JsonRpcResponse::Error { error, .. } => panic!("{error}"),
    }
}
