use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

use color_eyre::{eyre::eyre, Result};
use lazy_static::lazy_static;
use log::debug;

use starknet::core::types::{BlockId, ExecutionResult, StarknetError};
use starknet::core::{crypto::compute_hash_on_elements, types::FieldElement};
use starknet::providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider};
use starknet::providers::{MaybeUnknownErrorCode, ProviderError};
use starknet::{
    core::types::MaybePendingTransactionReceipt::{PendingReceipt, Receipt},
    providers::StarknetErrorWithMessage,
};

use std::time::Duration;
use sysinfo::{CpuExt, System, SystemExt};

use crate::metrics::{BenchmarkReport, GatlingReport};

lazy_static! {
    pub static ref SYSINFO: SysInfo = SysInfo::new();
}

/// Cairo string for "STARKNET_CONTRACT_ADDRESS"
const PREFIX_CONTRACT_ADDRESS: FieldElement = FieldElement::from_mont([
    3829237882463328880,
    17289941567720117366,
    8635008616843941496,
    533439743893157637,
]);

/// 2 ** 251 - 256
const ADDR_BOUND: FieldElement = FieldElement::from_mont([
    18446743986131443745,
    160989183,
    18446744073709255680,
    576459263475590224,
]);

// Copied from starknet-rs since it's not public
pub fn compute_contract_address(
    salt: FieldElement,
    class_hash: FieldElement,
    constructor_calldata: &[FieldElement],
) -> FieldElement {
    compute_hash_on_elements(&[
        PREFIX_CONTRACT_ADDRESS,
        FieldElement::ZERO,
        salt,
        class_hash,
        compute_hash_on_elements(constructor_calldata),
    ]) % ADDR_BOUND
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
    pub fn new() -> Self {
        let sys = System::new_all();
        let cpu = sys.global_cpu_info();

        Self {
            os_name: sys.long_os_version().unwrap().trim().to_string(),
            kernel_version: sys.kernel_version().unwrap(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_count: sys.cpus().len(),
            cpu_frequency: cpu.frequency(),
            cpu_brand: cpu.brand().to_string(),
            memory: sys.total_memory(),
        }
    }
}

impl Default for SysInfo {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SysInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            os_name,
            kernel_version,
            arch,
            cpu_count,
            cpu_frequency,
            cpu_brand,
            memory,
        } = self;

        let cpu_ghz_freq = *cpu_frequency as f64 / 1000.0;
        let gigabyte_memory = memory / (1024 * 1024 * 1024);

        writeln!(
            f,
            "System Information:\n\
            System : {os_name} Kernel Version {kernel_version}\n\
            Arch   : {arch}\n\
            CPU    : {cpu_brand} {cpu_ghz_freq:.2} GHz {cpu_count} cores\n\
            Memory : {gigabyte_memory} GB"
        )
    }
}

const WAIT_FOR_TX_TIMEOUT: Duration = Duration::from_secs(60);

pub async fn wait_for_tx(
    provider: &JsonRpcClient<HttpTransport>,
    tx_hash: FieldElement,
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
            Ok(Receipt(receipt)) => {
                // Logic copied from starkli and the following comment too
                // tWith JSON-RPC, once we get a receipt, the transaction must have been confirmed.
                // Rejected transactions simply aren't available. This needs to be changed once we
                // implement the sequencer fallback.

                match receipt.execution_result() {
                    ExecutionResult::Succeeded => {
                        return Ok(());
                    }
                    ExecutionResult::Reverted { reason } => {
                        return Err(eyre!(format!(
                            "Transaction {tx_hash:#064x} has been rejected/reverted: {reason}"
                        )));
                    }
                }
            }
            Ok(PendingReceipt(_)) => {
                debug!("Waiting for transaction {tx_hash:#064x} to be accepted");
                tokio::time::sleep(check_interval).await;
            }
            Err(ProviderError::StarknetError(StarknetErrorWithMessage {
                code: MaybeUnknownErrorCode::Known(StarknetError::TransactionHashNotFound),
                ..
            })) => {
                debug!("Waiting for transaction {tx_hash:#064x} to show up");
                tokio::time::sleep(check_interval).await;
            }
            Err(err) => {
                return Err(eyre!(err).wrap_err(format!(
                    "Error while waiting for transaction {tx_hash:#064x}"
                )))
            }
        }
    }
}

/// Get a Map of the number of transactions per block from `start_block` to
/// `end_block` (including both)
/// This is meant to be used to calculate multiple metrics such as TPS and TPB
/// without hitting the StarkNet RPC multiple times
// TODO: add a cache to avoid hitting the RPC for the same block
pub async fn get_num_tx_per_block(
    starknet_rpc: Arc<JsonRpcClient<HttpTransport>>,
    start_block: u64,
    end_block: u64,
) -> Result<Vec<u64>> {
    let mut num_tx_per_block = Vec::new();

    for block_number in start_block..=end_block {
        let n = starknet_rpc
            .get_block_transaction_count(BlockId::Number(block_number))
            .await?;

        num_tx_per_block.push(n);
    }

    Ok(num_tx_per_block)
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

#[derive(Debug)]
pub enum BenchmarkType {
    BlockRange(u64, u64),
    LatestBlocks(u64),
}

/// Builds a benchmark report for the given benchmark name and block range
pub async fn build_benchmark_report(
    starknet_rpc: Arc<JsonRpcClient<HttpTransport>>,
    benchmark_name: String,
    benchmark_type: BenchmarkType,
    gatling_report: &mut GatlingReport,
) -> Result<BenchmarkReport> {
    let benchmark_report = match benchmark_type {
        BenchmarkType::BlockRange(start, end) => {
            BenchmarkReport::from_block_range(
                starknet_rpc.clone(),
                benchmark_name.clone(),
                start,
                end,
            )
            .await?
        }
        BenchmarkType::LatestBlocks(num_blocks) => {
            BenchmarkReport::from_last_x_blocks(
                starknet_rpc.clone(),
                benchmark_name.clone(),
                num_blocks,
            )
            .await?
        }
    };

    gatling_report
        .benchmark_reports
        .push(benchmark_report.clone());

    Ok(benchmark_report)
}
