use std::{
    mem,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

use color_eyre::eyre::ensure;
use crossbeam_queue::ArrayQueue;
use goose::{config::GooseConfiguration, metrics::GooseRequestMetric, prelude::*};
use rand::prelude::SliceRandom;
use serde::{de::DeserializeOwned, Serialize};
use starknet::{
    accounts::RawExecutionV1,
    core::types::{
        Call, SequencerTransactionStatus, TransactionReceiptWithBlockInfo, TransactionStatus,
    },
};
use starknet::{
    accounts::{Account, ConnectedAccount, ExecutionEncoder, SingleOwnerAccount},
    core::types::{
        BroadcastedInvokeTransaction, BroadcastedInvokeTransactionV1, ExecutionResult, Felt,
    },
    providers::{
        jsonrpc::{HttpTransport, JsonRpcError, JsonRpcMethod, JsonRpcResponse},
        JsonRpcClient, ProviderError,
    },
    signers::LocalWallet,
};

use crate::{
    actions::setup::{GatlingSetup, CHECK_INTERVAL, MAX_FEE},
    config::{GatlingConfig, ParametersFile},
};

use super::setup::StarknetAccount;

pub fn make_goose_config(
    config: &GatlingConfig,
    amount: u64,
    name: &'static str,
) -> color_eyre::Result<GooseConfiguration> {
    ensure!(
        amount >= config.run.concurrency,
        "Too few {name} for the amount of concurrent users"
    );

    // div_euclid will truncate integers when not evenly divisable
    let user_iterations = amount.div_euclid(config.run.concurrency);
    // this will always be a multiple of concurrency, unlike the provided amount
    let total_transactions = user_iterations * config.run.concurrency;

    // If these are not equal that means user_iterations was truncated
    if total_transactions != amount {
        tracing::warn!("Number of {name} is not evenly divisble by concurrency, doing {total_transactions} calls instead");
    }

    Ok({
        let mut default = GooseConfiguration::default();
        default.host.clone_from(&config.rpc.url);
        default.iterations = user_iterations as usize;
        default.users = Some(config.run.concurrency as usize);
        default
    })
}

#[derive(Debug, Clone)]
pub struct GooseWriteUserState {
    pub account: StarknetAccount,
    pub nonce: Felt,
    pub prev_tx: Vec<Felt>,
}

impl GooseWriteUserState {
    pub async fn new(
        account: StarknetAccount,
        transactions_amount: usize,
    ) -> Result<Self, ProviderError> {
        Ok(Self {
            nonce: account.get_nonce().await?,
            account,
            prev_tx: Vec::with_capacity(transactions_amount),
        })
    }
}

pub async fn setup(
    accounts: Vec<StarknetAccount>,
    transactions_amount: usize,
) -> Result<TransactionFunction, ProviderError> {
    let queue = ArrayQueue::new(accounts.len());
    for account in accounts {
        queue
            .push(GooseWriteUserState::new(account, transactions_amount).await?)
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

pub fn goose_write_user_wait_last_tx() -> TransactionFunction {
    Arc::new(move |user| {
        let tx = user
            .get_session_data::<GooseWriteUserState>()
            .expect("Should be in a goose user with GooseUserState session data")
            .prev_tx
            .last()
            .copied();

        Box::pin(async move {
            // If all transactions failed, we can skip this step
            if let Some(tx) = tx {
                wait_for_tx_with_goose(user, tx).await?;
            }

            Ok(())
        })
    })
}

pub async fn read_method(
    shooter: &GatlingSetup,
    amount: u64,
    method: JsonRpcMethod,
    parameters_list: ParametersFile,
) -> color_eyre::Result<GooseMetrics> {
    let goose_read_config = make_goose_config(shooter.config(), amount, "read calls")?;

    let reads: TransactionFunction = Arc::new(move |user| {
        let mut rng = rand::thread_rng();

        let mut params_list = parameters_list.clone();
        params_list.shuffle(&mut rng); // Make sure each goose user has their own order
        let mut paramaters_cycle = params_list.into_iter().cycle();

        Box::pin(async move {
            let params = paramaters_cycle
                .next()
                .expect("Cyclic iterator should never end");

            let _: (serde_json::Value, _) =
                send_request(user, method, serde_json::Value::Object(params)).await?;

            Ok(())
        })
    });

    let metrics = GooseAttack::initialize_with_config(goose_read_config)?
        .register_scenario(
            scenario!("Read Metric")
                .register_transaction(Transaction::new(reads).set_name("Request")),
        )
        .execute()
        .await?;

    Ok(metrics)
}

#[derive(Default, Debug)]
pub struct TransactionBlocks {
    pub first: AtomicU64,
    pub last: AtomicU64,
}

pub async fn verify_transactions(
    user: &mut GooseUser,
    blocks: Arc<TransactionBlocks>,
) -> TransactionResult {
    let transactions = mem::take(
        &mut user
            .get_session_data_mut::<GooseWriteUserState>()
            .expect("Should be in a goose user with GooseUserState session data")
            .prev_tx,
    );

    for (index, tx) in transactions.iter().enumerate() {
        let (status, mut metrics) =
            send_request::<TransactionStatus>(user, JsonRpcMethod::GetTransactionStatus, tx)
                .await?;

        match status.finality_status() {
            SequencerTransactionStatus::Rejected => {
                let tag = format!("Transaction {tx:#064x} has been rejected/reverted");

                return user.set_failure(&tag, &mut metrics, None, None);
            }
            SequencerTransactionStatus::Received => {
                let tag =
                    format!("Transaction {tx:#064x} is pending when no transactions should be");

                return user.set_failure(&tag, &mut metrics, None, None);
            }
            SequencerTransactionStatus::AcceptedOnL1 | SequencerTransactionStatus::AcceptedOnL2 => {
                if index == 0 || index == transactions.len() - 1 {
                    let (tx, mut metrics) = send_request::<TransactionReceiptWithBlockInfo>(
                        user,
                        JsonRpcMethod::GetTransactionReceipt,
                        tx,
                    )
                    .await?;
                    let block_number = match tx.block.block_number() {
                        Some(block_number) => block_number,
                        None => {
                            return user.set_failure(
                                "Receipt is not of type InvokeTransactionReceipt or is Pending",
                                &mut metrics,
                                None,
                                None,
                            );
                        }
                    };

                    if index == 0 {
                        blocks.first.store(block_number, Ordering::Relaxed);
                    }

                    if index == transactions.len() - 1 {
                        blocks.last.store(block_number, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    Ok(())
}

const WAIT_FOR_TX_TIMEOUT: Duration = Duration::from_secs(600);

/// This function is different then `crate::utils::wait_for_tx` due to it using the goose requester
pub async fn wait_for_tx_with_goose(
    user: &mut GooseUser,
    tx_hash: Felt,
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

        const TRANSACTION_HASH_NOT_FOUND: i64 = 29;

        match receipt {
            JsonRpcResponse::Success {
                result: TransactionReceiptWithBlockInfo { receipt, block: _ },
                ..
            } => match receipt.execution_result() {
                ExecutionResult::Succeeded => {
                    return Ok(());
                }
                ExecutionResult::Reverted { reason } => {
                    return user.set_failure(
                        &(reverted_tag() + reason),
                        &mut metric,
                        None,
                        Some(reason),
                    );
                }
            },
            JsonRpcResponse::Error {
                error:
                    JsonRpcError {
                        code: TRANSACTION_HASH_NOT_FOUND,
                        ..
                    },
                ..
            } => {
                tracing::debug!("Waiting for transaction {tx_hash:#064x} to show up");
                tokio::time::sleep(CHECK_INTERVAL).await;
            }
            JsonRpcResponse::Error {
                error: JsonRpcError { code, message, .. },
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
    nonce: Felt,
    from_account: &SingleOwnerAccount<Arc<JsonRpcClient<HttpTransport>>, LocalWallet>,
    method: JsonRpcMethod,
) -> Result<(T, GooseRequestMetric), Box<TransactionError>> {
    let calldata = from_account.encode_calls(&calls);

    #[allow(dead_code)] // Removes warning for unused fields, we need them to properly transmute
    struct FakeRawExecution {
        calls: Vec<Call>,
        nonce: Felt,
        max_fee: Felt,
    }

    let raw_exec = FakeRawExecution {
        calls,
        nonce,
        max_fee: MAX_FEE,
    };

    // TODO: We cannot right now construct RawExecution directly and need to use this hack
    // see https://github.com/xJonathanLEI/starknet-rs/issues/538
    let raw_exec = unsafe { mem::transmute::<FakeRawExecution, RawExecutionV1>(raw_exec) };

    let param = BroadcastedInvokeTransaction::V1(BroadcastedInvokeTransactionV1 {
        sender_address: from_account.address(),
        calldata,
        max_fee: MAX_FEE,
        signature: from_account
            .sign_execution_v1(&raw_exec, false)
            .await
            .expect("Raw Execution should be correctly constructed for signature"),
        nonce,
        is_query: false,
    });

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
