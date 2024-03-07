use std::{
    mem,
    sync::Arc,
    time::{Duration, SystemTime},
};

use color_eyre::eyre::ensure;
use crossbeam_queue::ArrayQueue;
use goose::{config::GooseConfiguration, metrics::GooseRequestMetric, prelude::*};
use serde::{de::DeserializeOwned, Serialize};
use starknet::{
    accounts::{
        Account, Call, ConnectedAccount, ExecutionEncoder, RawExecution, SingleOwnerAccount,
    },
    core::types::{
        ExecutionResult, FieldElement, InvokeTransactionResult, MaybePendingTransactionReceipt,
        StarknetError,
    },
    macros::{felt, selector},
    providers::{
        jsonrpc::{
            HttpTransport, HttpTransportError, JsonRpcClientError, JsonRpcError, JsonRpcMethod,
            JsonRpcResponse,
        },
        JsonRpcClient, ProviderError,
    },
    signers::LocalWallet,
};

use crate::{
    actions::shoot::{GatlingShooterSetup, CHECK_INTERVAL, MAX_FEE},
    generators::get_rng,
};

use super::shoot::StarknetAccount;

pub async fn erc20(shooter: &GatlingShooterSetup) -> color_eyre::Result<GooseMetrics> {
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

    let transfer_wait: TransactionFunction = goose_user_wait_last_tx();

    let metrics = GooseAttack::initialize_with_config(goose_config.clone())?
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
                        .set_name("Verification")
                        .set_sequence(3)
                        .set_on_stop(),
                ),
        )
        .execute()
        .await?;

    Ok(metrics)
}

pub async fn erc721(shooter: &GatlingShooterSetup) -> color_eyre::Result<GooseMetrics> {
    let config = shooter.config();
    let environment = shooter.environment()?;

    ensure!(
        config.run.num_erc721_mints >= config.run.concurrency,
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

    let mint_wait: TransactionFunction = goose_user_wait_last_tx();

    let metrics = GooseAttack::initialize_with_config(goose_mint_config.clone())?
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
                        .set_name("Verification")
                        .set_sequence(3)
                        .set_on_stop(),
                ),
        )
        .execute()
        .await?;

    Ok(metrics)
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

fn goose_user_wait_last_tx() -> TransactionFunction {
    Arc::new(move |user| {
        let tx = user
            .get_session_data::<GooseUserState>()
            .expect("Should be in a goose user with GooseUserState session data")
            .prev_tx
            .last()
            .copied();

        Box::pin(async move {
            // If all transactions failed, we can skip this step
            if let Some(tx) = tx {
                wait_for_tx(user, tx).await?;
            }

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
    .await?
    .0;

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
    .await?
    .0;

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
        let (receipt, mut metrics) =
            send_request(user, JsonRpcMethod::GetTransactionReceipt, tx).await?;

        match receipt {
            MaybePendingTransactionReceipt::Receipt(receipt) => match receipt.execution_result() {
                ExecutionResult::Succeeded => {}
                ExecutionResult::Reverted { reason } => {
                    let tag = format!("Transaction {tx:#064x} has been rejected/reverted");

                    return user.set_failure(&tag, &mut metrics, None, Some(reason));
                }
            },
            MaybePendingTransactionReceipt::PendingReceipt(pending) => {
                let tag =
                    format!("Transaction {tx:#064x} is pending when no transactions should be");
                let body = format!("{pending:?}");

                return user.set_failure(&tag, &mut metrics, None, Some(&body));
            }
        }
    }

    Ok(())
}

const WAIT_FOR_TX_TIMEOUT: Duration = Duration::from_secs(60);

pub async fn wait_for_tx(
    user: &mut GooseUser,
    tx_hash: FieldElement,
) -> Result<(), Box<TransactionError>> {
    let start = SystemTime::now();

    loop {
        let (receipt, mut metric) =
            raw_send_request(user, JsonRpcMethod::GetTransactionReceipt, tx_hash).await?;

        if start.elapsed().unwrap() >= WAIT_FOR_TX_TIMEOUT {
            let tag = format!("Timeout while waiting for transaction {tx_hash:#064x}");
            return user.set_failure(&tag, &mut metric, None, None);
        }

        let reverted_tag = || format!("Transaction {tx_hash:#064x} has been rejected/reverted");

        match receipt {
            JsonRpcResponse::Success {
                result: MaybePendingTransactionReceipt::Receipt(receipt),
                ..
            } => {
                // Logic copied from starkli and the following comment too
                // tWith JSON-RPC, once we get a receipt, the transaction must have been confirmed.
                // Rejected transactions simply aren't available. This needs to be changed once we
                // implement the sequencer fallback.

                match receipt.execution_result() {
                    ExecutionResult::Succeeded => {
                        return Ok(());
                    }
                    ExecutionResult::Reverted { reason } => {
                        return user.set_failure(&reverted_tag(), &mut metric, None, Some(reason));
                    }
                }
            }
            JsonRpcResponse::Success {
                result: MaybePendingTransactionReceipt::PendingReceipt(pending),
                ..
            } => {
                if let ExecutionResult::Reverted { reason } = pending.execution_result() {
                    return user.set_failure(&reverted_tag(), &mut metric, None, Some(reason));
                }
                log::debug!("Waiting for transaction {tx_hash:#064x} to be accepted");
                tokio::time::sleep(CHECK_INTERVAL).await;
            }
            JsonRpcResponse::Error {
                error: JsonRpcError { code, .. },
                ..
            } if code == StarknetError::TransactionHashNotFound as i64 => {
                log::debug!("Waiting for transaction {tx_hash:#064x} to show up");
                tokio::time::sleep(CHECK_INTERVAL).await;
            }
            JsonRpcResponse::Error {
                error: JsonRpcError { code, message },
                ..
            } => {
                let tag = format!("Error Code {code} while waiting for tx {tx_hash:#064x}");

                return user.set_failure(&tag, &mut metric, None, Some(&message));
            }
        }
    }
}

/// Sends a execution request via goose, returning the successful json rpc response
pub async fn send_execution<T: DeserializeOwned>(
    user: &mut GooseUser,
    calls: Vec<Call>,
    nonce: FieldElement,
    from_account: &SingleOwnerAccount<Arc<JsonRpcClient<HttpTransport>>, LocalWallet>,
    method: JsonRpcMethod,
) -> Result<(T, GooseRequestMetric), Box<TransactionError>> {
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

/// Sends request via goose, returning the successful json rpc response
pub async fn send_request<T: DeserializeOwned>(
    user: &mut GooseUser,
    method: JsonRpcMethod,
    param: impl Serialize,
) -> Result<(T, GooseRequestMetric), Box<TransactionError>> {
    let (body, mut metrics) = raw_send_request(user, method, param).await?;

    match body {
        JsonRpcResponse::Success { result, .. } => Ok((result, metrics)),
        JsonRpcResponse::Error { error, .. } => {
            // While this is not the actual body we cannot serialize `JsonRpcResponse`
            // due to it not implementing `Serialize`, and if we were to get the text before
            // serializing we couldn't map the serde_json error to `TransactionError`.
            // It also provides the exact same info and this is just debug information
            let error = error.to_string();

            Err(user
                .set_failure("RPC Response was Error", &mut metrics, None, Some(&error))
                .unwrap_err()) // SAFETY: This always returns a error
        }
    }
}

/// Sends request via goose, returning the deserialized response
pub async fn raw_send_request<T: DeserializeOwned>(
    user: &mut GooseUser,
    method: JsonRpcMethod,
    param: impl Serialize,
) -> Result<(JsonRpcResponse<T>, GooseRequestMetric), Box<TransactionError>> {
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

    let goose_response = user.post_json("/", &request).await?;

    let body = goose_response
        .response
        .map_err(TransactionError::Reqwest)?
        .json::<JsonRpcResponse<T>>()
        .await
        .map_err(TransactionError::Reqwest)?;

    Ok((body, goose_response.request))
}
