use std::{
    mem,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

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
};

use super::shoot::StarknetAccount;

pub async fn erc20(shooter: &GatlingShooterSetup) -> color_eyre::Result<()> {
    let _erc20_address = shooter.environment().unwrap().erc20_address;
    let config = shooter.config();

    let goose_config = {
        let mut default = GooseConfiguration::default();
        default.host = config.rpc.url.clone();
        default.iterations = (config.run.num_erc20_transfers / config.run.concurrency) as usize;
        default.users = Some(config.run.concurrency as usize);
        default
    };

    let queue = Arc::new(ArrayQueue::new(config.run.num_erc20_transfers as usize));
    let queue_trans = queue.clone();
    let queue_trans_verify = queue_trans.clone();

    let last_trans = Arc::new([
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ]);
    let last_transaction_clone = last_trans.clone();

    let transfer: TransactionFunction = Arc::new(move |user| {
        let queue = queue_trans.clone();
        let last_mint = last_transaction_clone.clone();
        Box::pin(async move { transfer(user, &queue, _erc20_address, &last_mint).await })
    });

    let transfer_verify: TransactionFunction = Arc::new(move |user| {
        let queue = queue_trans_verify.clone();
        Box::pin(async move { verify_transacs(user, &queue).await })
    });

    let transfer_setup = setup(shooter.environment()?.accounts.clone()).await?;

    GooseAttack::initialize_with_config(goose_config.clone())?
        .register_scenario(
            scenario!("Transactions")
                .register_transaction(
                    transaction!(transfer_setup)
                        .set_name("Setup")
                        .set_on_start(),
                )
                .register_transaction(transaction!(transfer).set_name("Transfer")),
        )
        .execute()
        .await?;

    // Wait for the last transaction to be incorporated in a block
    shooter
        .wait_for_tx(
            FieldElement::from_mont(Arc::try_unwrap(last_trans).unwrap().map(|x| x.into_inner())),
            CHECK_INTERVAL,
        )
        .await?;

    GooseAttack::initialize_with_config(goose_config)?
        .register_scenario(
            scenario!("Transactions Verify").register_transaction(transaction!(transfer_verify)),
        )
        .execute()
        .await?;

    // todo!()

    Ok(())
}

pub async fn erc721(shooter: &GatlingShooterSetup) -> color_eyre::Result<()> {
    let config = shooter.config();
    let nonces = Arc::new(ArrayQueue::new(config.run.num_erc721_mints as usize));
    let erc721_address = shooter.environment().unwrap().erc721_address;
    let mut nonce = shooter.deployer_account().get_nonce().await?;

    for _ in 0..config.run.num_erc721_mints {
        nonces
            .push(nonce)
            .expect("ArrayQueue has capacity for all mints");
        nonce += FieldElement::ONE;
    }

    let goose_mint_config = {
        let mut default = GooseConfiguration::default();
        default.host = config.rpc.url.clone();
        default.iterations = (config.run.num_erc721_mints / config.run.concurrency) as usize;
        default.users = Some(config.run.concurrency as usize);
        default
    };

    let queue = Arc::new(ArrayQueue::new(config.run.num_erc721_mints as usize));
    let queue_mint = queue.clone();
    let queue_mint_verify = queue_mint.clone();

    let last_trans = Arc::new([
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ]);
    let last_mint_clone = last_trans.clone();

    let from_account = shooter.deployer_account().clone();

    let mint: TransactionFunction = Arc::new(move |user| {
        let queue = queue_mint.clone();
        let nonces = nonces.clone();
        let nonce = nonces.pop().unwrap();
        let last_mint = last_mint_clone.clone();
        let from_account = from_account.clone();
        Box::pin(async move {
            mint(
                user,
                &queue,
                erc721_address,
                nonce,
                &from_account,
                &last_mint,
            )
            .await
        })
    });

    let mint_verify: TransactionFunction = Arc::new(move |user| {
        let queue = queue_mint_verify.clone();
        Box::pin(async move { verify_transacs(user, &queue).await })
    });

    let mint_setup = setup(shooter.environment()?.accounts.clone()).await?;

    GooseAttack::initialize_with_config(goose_mint_config.clone())?
        .register_scenario(
            scenario!("Minting")
                .register_transaction(transaction!(mint_setup).set_name("Setup").set_on_start())
                .register_transaction(transaction!(mint).set_name("Minting")),
        )
        .execute()
        .await?;

    // Wait for the last transaction to be incorporated in a block
    shooter
        .wait_for_tx(
            FieldElement::from_mont(Arc::try_unwrap(last_trans).unwrap().map(|x| x.into_inner())),
            CHECK_INTERVAL,
        )
        .await?;

    GooseAttack::initialize_with_config(goose_mint_config)?
        .register_scenario(scenario!("Mint Verify").register_transaction(transaction!(mint_verify)))
        .execute()
        .await?;

    // todo!()

    Ok(())
}

#[derive(Debug, Clone)]
struct GooseUserState {
    account: StarknetAccount,
    nonce: FieldElement,
}

pub type RpcError = ProviderError<JsonRpcClientError<HttpTransportError>>;

impl GooseUserState {
    pub async fn new(account: StarknetAccount) -> Result<Self, RpcError> {
        Ok(Self {
            nonce: account.get_nonce().await?,
            account,
        })
    }
}

async fn setup(accounts: Vec<StarknetAccount>) -> Result<TransactionFunction, RpcError> {
    let queue = ArrayQueue::new(accounts.len());
    for account in accounts {
        queue.push(GooseUserState::new(account).await?).unwrap();
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

async fn transfer(
    user: &mut GooseUser,
    queue: &ArrayQueue<FieldElement>,
    erc20_address: FieldElement,
    prev_hash: &[AtomicU64; 4],
) -> TransactionResult {
    let GooseUserState {
        account,
        nonce: state_nonce,
    } = user.get_session_data_mut::<GooseUserState>().unwrap();

    let nonce = *state_nonce;
    *state_nonce += FieldElement::ONE;
    let account = account.clone();

    let (amount_low, amount_high) = (felt!("1"), felt!("0"));

    let call = Call {
        to: erc20_address,
        selector: selector!("transfer"),
        calldata: vec![
            FieldElement::from_hex_be("0xdead").unwrap(), // recipient
            amount_low,
            amount_high,
        ],
    };

    let response: InvokeTransactionResult = send_execution(
        user,
        vec![call],
        nonce,
        &account,
        JsonRpcMethod::AddInvokeTransaction,
    )
    .await?;

    queue.push(response.transaction_hash).unwrap();

    for (atomic, store) in prev_hash.iter().zip(response.transaction_hash.into_mont()) {
        atomic.store(store, Ordering::Relaxed)
    }

    Ok(())
}

async fn mint(
    user: &mut GooseUser,
    queue: &ArrayQueue<FieldElement>,
    erc721_address: FieldElement,
    nonce: FieldElement,
    from_account: &SingleOwnerAccount<Arc<JsonRpcClient<HttpTransport>>, LocalWallet>,
    prev_hash: &[AtomicU64; 4],
) -> TransactionResult {
    let recipient = user
        .get_session_data::<GooseUserState>()
        .unwrap()
        .account
        .clone()
        .address();

    let (token_id_low, token_id_high) = (get_rng(), felt!("0x0000"));

    let call = Call {
        to: erc721_address,
        selector: selector!("mint"),
        calldata: vec![
            recipient, // recipient
            token_id_low,
            token_id_high,
        ],
    };

    let response: InvokeTransactionResult = send_execution(
        user,
        vec![call],
        nonce,
        from_account,
        JsonRpcMethod::AddInvokeTransaction,
    )
    .await?;

    queue.push(response.transaction_hash).unwrap();

    for (atomic, store) in prev_hash.iter().zip(response.transaction_hash.into_mont()) {
        atomic.store(store, Ordering::Relaxed)
    }

    Ok(())
}

async fn verify_transacs(
    user: &mut GooseUser,
    queue: &ArrayQueue<FieldElement>,
) -> TransactionResult {
    let transaction = queue.pop().unwrap();

    let receipt: MaybePendingTransactionReceipt =
        send_request(user, JsonRpcMethod::GetTransactionReceipt, transaction).await?;

    match receipt {
        MaybePendingTransactionReceipt::Receipt(receipt) => match receipt.execution_result() {
            ExecutionResult::Succeeded => Ok(()),
            ExecutionResult::Reverted { reason } => {
                panic!("Transaction {transaction:#064x} has been rejected/reverted: {reason}");
            }
        },
        MaybePendingTransactionReceipt::PendingReceipt(_) => {
            panic!("Transaction {transaction:#064x} is pending when no transactions should be")
        }
    }
}

pub async fn send_execution<T: DeserializeOwned>(
    user: &mut GooseUser,
    calls: Vec<Call>,
    nonce: FieldElement,
    from_account: &SingleOwnerAccount<Arc<JsonRpcClient<HttpTransport>>, LocalWallet>,
    method: JsonRpcMethod,
) -> Result<T, Box<TransactionError>> {
    let calldata = from_account.encode_calls(&calls);

    #[allow(dead_code)]
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
        signature: from_account.sign_execution(&raw_exec).await.unwrap(),
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
        // but the goose error type doesn't allow it
        JsonRpcResponse::Error { error, .. } => panic!("{error}"),
    }
}
