use std::ops::Deref;
use std::sync::Arc;
use std::time::SystemTime;

use color_eyre::eyre::{bail, OptionExt};
use color_eyre::{eyre::eyre, Result};
use lazy_static::lazy_static;

use starknet::core::types::Felt;
use starknet::core::types::{
    BlockId, BlockWithTxs, ComputationResources, DataAvailabilityResources, DataResources,
    ExecutionResources, ExecutionResult, MaybePendingBlockWithTxs, StarknetError,
};
use starknet::providers::ProviderError;
use starknet::providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider};
use tokio::task::JoinSet;

use std::time::Duration;
use sysinfo::System;

lazy_static! {
    pub static ref SYSINFO: SysInfo = SysInfo::new();
}

#[derive(Debug, Clone)]
pub struct SysInfo {
    pub os_name: String,
    pub kernel_version: String,
    pub arch: String,
    pub cpu_count: usize,
    pub cpu_frequency: u64,
    pub cpu_brand: String,
    pub memory: u64,
}

impl SysInfo {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let sys = System::new_all();
        let cpu = sys.global_cpu_info();

        Self {
            os_name: System::long_os_version().unwrap().trim().to_string(),
            kernel_version: System::kernel_version().unwrap(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_count: sys.cpus().len(),
            cpu_frequency: cpu.frequency(),
            cpu_brand: cpu.brand().to_string(),
            memory: sys.total_memory(),
        }
    }
}

pub fn sysinfo_string() -> String {
    let SysInfo {
        os_name,
        kernel_version,
        arch,
        cpu_count,
        cpu_frequency,
        cpu_brand,
        memory,
    } = SYSINFO.deref();

    let gigabyte_memory = memory / (1024 * 1024 * 1024);

    format!(
        "CPU Count: {cpu_count}\n\
        CPU Model: {cpu_brand}\n\
        CPU Speed (MHz): {cpu_frequency}\n\
        Total Memory: {gigabyte_memory} GB\n\
        Platform: {os_name}\n\
        Release: {kernel_version}\n\
        Architecture: {arch}",
    )
}

const WAIT_FOR_TX_TIMEOUT: Duration = Duration::from_secs(60);

pub async fn wait_for_tx(
    provider: &JsonRpcClient<HttpTransport>,
    tx_hash: Felt,
    check_interval: Duration,
) -> Result<()> {
    let start = SystemTime::now();

    loop {
        if start.elapsed().unwrap() >= WAIT_FOR_TX_TIMEOUT {
            return Err(eyre!(
                "Timeout while waiting for transaction {tx_hash:#064x}"
            ));
        }

        match provider.get_transaction_receipt(tx_hash).await {
            Ok(tx) => match tx.receipt.execution_result() {
                ExecutionResult::Succeeded => {
                    return Ok(());
                }
                ExecutionResult::Reverted { reason } => {
                    return Err(eyre!(format!(
                        "Transaction {tx_hash:#064x} has been rejected/reverted: {reason}"
                    )));
                }
            },
            Err(ProviderError::StarknetError(StarknetError::TransactionHashNotFound)) => {
                tracing::debug!("Waiting for transaction {tx_hash:#064x} to show up");
                tokio::time::sleep(check_interval).await;
            }
            Err(err) => {
                return Err(eyre!(err).wrap_err(format!(
                    "Error while waiting for transaction {tx_hash:#064x}"
                )));
            }
        }
    }
}

/// Get a list of blocks with transaction information from
/// `start_block` to `end_block` (including both)
/// This is meant to be used to calculate multiple metrics such as TPS and UOPS
/// without hitting the StarkNet RPC multiple times
pub async fn get_blocks_with_txs(
    starknet_rpc: &Arc<JsonRpcClient<HttpTransport>>,
    block_range: impl Iterator<Item = u64>,
) -> Result<Vec<(BlockWithTxs, Vec<ExecutionResources>)>> {
    const MAX_CONCURRENT: usize = 50;

    // A collection of spawned tokio tasks
    let mut join_set = JoinSet::new();

    let mut results = Vec::with_capacity(block_range.size_hint().0);

    for block_number in block_range {
        // Make sure we don't hit dev server with too many requests
        while join_set.len() >= MAX_CONCURRENT {
            let next = join_set
                .join_next()
                .await
                .ok_or_eyre("JoinSet should have items")???;

            results.push(next);
        }

        let starknet_rpc = starknet_rpc.clone();

        join_set.spawn(get_block_info(starknet_rpc, block_number));
    }

    async fn get_block_info(
        starknet_rpc: Arc<JsonRpcClient<HttpTransport>>,
        block_number: u64,
    ) -> Result<(BlockWithTxs, Vec<ExecutionResources>)> {
        let block_with_txs = match starknet_rpc
            .get_block_with_txs(BlockId::Number(block_number))
            .await?
        {
            MaybePendingBlockWithTxs::Block(b) => b,
            MaybePendingBlockWithTxs::PendingBlock(pending) => {
                bail!("Block should not be pending. Pending: {pending:?}")
            }
        };

        let mut resources = Vec::with_capacity(block_with_txs.transactions.len());

        #[cfg(feature = "with_sps")]
        for tx in block_with_txs.transactions.iter() {
            let maybe_receipt = starknet_rpc
                .get_transaction_receipt(tx.transaction_hash())
                .await?;

            use starknet::core::types::TransactionReceipt as TR;

            let resource = match maybe_receipt {
                Receipt(receipt) => match receipt {
                    TR::Invoke(receipt) => receipt.execution_resources,
                    TR::L1Handler(receipt) => receipt.execution_resources,
                    TR::Declare(receipt) => receipt.execution_resources,
                    TR::Deploy(receipt) => receipt.execution_resources,
                    TR::DeployAccount(receipt) => receipt.execution_resources,
                },
                PendingReceipt(pending) => {
                    bail!("Transaction should not be pending. Pending: {pending:?}");
                }
            };

            resources.push(resource);
        }
        #[cfg(not(feature = "with_sps"))]
        for _ in block_with_txs.transactions.iter() {
            resources.push(ExecutionResources {
                computation_resources: ComputationResources {
                    steps: 0,
                    memory_holes: None,
                    range_check_builtin_applications: None,
                    pedersen_builtin_applications: None,
                    poseidon_builtin_applications: None,
                    ec_op_builtin_applications: None,
                    ecdsa_builtin_applications: None,
                    bitwise_builtin_applications: None,
                    keccak_builtin_applications: None,
                    segment_arena_builtin: None,
                },
                data_resources: DataResources {
                    data_availability: DataAvailabilityResources {
                        l1_data_gas: 0,
                        l1_gas: 0,
                    },
                },
            });
        }

        Ok((block_with_txs, resources))
    }

    // Process the rest
    while let Some(next) = join_set.join_next().await {
        results.push(next??)
    }

    // Make sure blocks are in order
    results.sort_unstable_by_key(|(block, _)| block.block_number);

    Ok(results)
}

/// Sanitize a string to be used as a filename by removing/replacing illegal chars
pub fn sanitize_filename(input: &str) -> String {
    // Define a set of characters to replace or remove
    let invalid_chars: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|', ' '];

    // Replace invalid characters with underscores and remove control characters
    let sanitized = input
        .to_lowercase()
        .chars()
        .map(|c| {
            if invalid_chars.contains(&c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect::<String>();

    // Truncate the string to a reasonable length for file names
    let max_length = 255; // Maximum file name length for many file systems
    let truncated = if sanitized.len() > max_length {
        &sanitized[..max_length]
    } else {
        &sanitized
    };

    truncated.to_string()
}
