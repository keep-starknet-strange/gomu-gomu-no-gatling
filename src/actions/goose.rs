use std::{
    mem::{self, size_of},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use color_eyre::owo_colors::OwoColorize;
use crossbeam_queue::ArrayQueue;
use goose::{config::GooseConfiguration, prelude::*};
use log::trace;
use rand::seq::SliceRandom;
use serde_derive::Serialize;
use serde_json::json;
use starknet::{
    accounts::{
        Account, Call, ConnectedAccount, ExecutionEncoder, ExecutionEncoding, RawExecution,
        SingleOwnerAccount,
    },
    core::types::FieldElement,
    macros::{felt, selector},
    providers::{
        jsonrpc::{HttpTransport, JsonRpcMethod},
        JsonRpcClient,
    },
    signers::{LocalWallet, SigningKey},
};
use url::Url;

use crate::{
    actions::shoot::{self, GatlingShooter, CHECK_INTERVAL, MAX_FEE},
    config::GatlingConfig,
    generators::get_rng,
    utils::wait_for_tx,
};

use super::shoot::GatlingReport;

pub async fn goose(config: GatlingConfig) -> color_eyre::Result<()> {
    let mut shooter = GatlingShooter::from_config(config.clone()).await?;
    shooter.setup().await?;
    let accounts: Arc<[_]> = shooter
        .environment()?
        .accounts
        .iter()
        .map(|x| x.address())
        .collect::<Vec<_>>()
        .into();
    let nonces = Arc::new(ArrayQueue::new(config.run.num_erc721_mints as usize));
    let erc721_address = shooter.environment().unwrap().erc721_address;

    // shooter.run_erc20(config.run.num_erc20_transfers).await;

    let mut nonce = shooter.account.get_nonce().await?;

    for _ in 0..config.run.num_erc721_mints {
        nonces
            .push(nonce)
            .expect("ArrayQueue has capacity for all mints");
        nonce += FieldElement::ONE;
    }

    let goose_mint_config = {
        let mut default = GooseConfiguration::default();
        default.host = config.rpc.url;
        default.iterations = (config.run.num_erc721_mints / config.run.concurrency) as usize;
        default.users = Some(config.run.concurrency as usize);
        default.report_file = String::from("./report.html");
        default
    };

    let queue = Arc::new(ArrayQueue::new(config.run.num_erc721_mints as usize));
    let queue_mint = queue.clone();
    let queue_mint_verify = queue_mint.clone();

    let last_mint = Arc::new([
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ]);
    let last_mint_clone = last_mint.clone();

    let mint: TransactionFunction = Arc::new(move |user| {
        let queue = queue_mint.clone();
        let nonces = nonces.clone();
        let nonce = nonces.pop().unwrap();
        let mut rng = rand::thread_rng();
        let account = *accounts.choose(&mut rng).unwrap();
        let last_mint = last_mint_clone.clone();
        let from_account = shooter.account.clone();
        Box::pin(async move {
            mint(
                user,
                &queue,
                erc721_address,
                nonce,
                &from_account,
                account,
                &last_mint,
            )
            .await
        })
    });

    #[allow(unused_variables)]
    let mint_verify: TransactionFunction = Arc::new(move |user| {
        let queue = queue_mint_verify.clone();
        Box::pin(async move { mint_verify(user, &queue).await })
    });

    GooseAttack::initialize_with_config(goose_mint_config.clone())?
        .register_scenario(scenario!("Mint").register_transaction(transaction!(mint)))
        // .register_scenario(scenario!("Mint Verify").register_transaction(transaction!(mint_verify)))
        .execute()
        .await?;

    // Wait for the last transaction to be incorporated in a block
    // wait_for_tx(
    //     &shooter.starknet_rpc,
    //     queue.pop().unwrap(),
    //     // FieldElement::from_mont(Arc::try_unwrap(last_mint).unwrap().map(|x| x.into_inner())),
    //     CHECK_INTERVAL,
    // )
    // .await?;

    // GooseAttack::initialize_with_config(goose_mint_config)?
    //     .register_scenario(scenario!("Mint Verify").register_transaction(transaction!(mint_verify)))
    //     .execute()
    //     .await?;

    // todo!()

    Ok(())
}

async fn mint(
    user: &mut GooseUser,
    queue: &ArrayQueue<FieldElement>,
    erc721_address: FieldElement,
    nonce: FieldElement,
    from_account: &SingleOwnerAccount<Arc<JsonRpcClient<HttpTransport>>, LocalWallet>,
    account: FieldElement,
    prev_hash: &[AtomicU64; 4],
) -> TransactionResult {
    let (token_id_low, token_id_high) = (get_rng(), felt!("0x0000"));

    let call = Call {
        to: erc721_address,
        selector: selector!("mint"),
        calldata: vec![
            account, // recipient
            token_id_low,
            token_id_high,
        ],
    };

    #[allow(dead_code)]
    pub struct FakeRawExecution {
        calls: Vec<Call>,
        nonce: FieldElement,
        max_fee: FieldElement,
    }

    let raw_exec = FakeRawExecution {
        calls: vec![call.clone()],
        nonce,
        max_fee: MAX_FEE,
    };

    // this needs to be removed later, however we can't construct RawExecution ourselves
    let raw_exec = unsafe { mem::transmute::<FakeRawExecution, RawExecution>(raw_exec) };

    let params = starknet::core::types::BroadcastedInvokeTransaction {
        sender_address: from_account.address(),
        calldata: from_account.encode_calls(&[call.clone()]),
        max_fee: MAX_FEE,
        signature: from_account.sign_execution(&raw_exec).await.unwrap(),
        nonce,
        is_query: false,
    };

    let request = JsonRpcRequest {
        id: 1,
        jsonrpc: "2.0",
        method: JsonRpcMethod::AddInvokeTransaction,
        params: [params],
    };

    let goose_response = user
        .post_json("/", &request)
        .await?
        .response
        .map_err(TransactionError::Reqwest)?;
    
    let res: starknet::providers::jsonrpc::JsonRpcResponse<
        starknet::core::types::InvokeTransactionResult,
    > = goose_response.json().await.unwrap();

    let hash = match res {
        starknet::providers::jsonrpc::JsonRpcResponse::Success { result, .. } => result,
        // Actually returning this error would probably be a good idea, but we can't for now
        starknet::providers::jsonrpc::JsonRpcResponse::Error { error, .. } => panic!("{error}"),
    }
    .transaction_hash;

    queue.push(hash).unwrap();

    for (atomic, store) in prev_hash.iter().zip(hash.into_mont()) {
        atomic.store(store, Ordering::Relaxed)
    }

    // Should ideally be replaced with:
    // let result = from_account
    //     .execute(vec![call])
    //     .max_fee(MAX_FEE)
    //     .nonce(nonce)
    //     .to_json()

    Ok(())
}

// Copied from https://docs.rs/starknet-providers/0.9.0/src/starknet_providers/jsonrpc/transports/http.rs.html#21-27
#[derive(Debug, Serialize)]
struct JsonRpcRequest<T> {
    id: u64,
    jsonrpc: &'static str,
    method: JsonRpcMethod,
    params: T,
}

async fn mint_verify(user: &mut GooseUser, queue: &ArrayQueue<FieldElement>) -> TransactionResult {
    println!("{:?}", queue.pop());

    Ok(())
}
