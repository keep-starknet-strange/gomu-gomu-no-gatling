use std::fmt;
use std::sync::Arc;
use std::{collections::HashMap, time::SystemTime};

use color_eyre::{eyre::eyre, Result};
use lazy_static::lazy_static;
use log::debug;
use serde::Serialize;
use starknet::core::types::{BlockId, StarknetError, TransactionStatus};
use starknet::core::{crypto::compute_hash_on_elements, types::FieldElement};
use starknet::providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider};
use starknet::providers::{MaybeUnknownErrorCode, ProviderError};
use starknet::{
    core::types::{
        MaybePendingTransactionReceipt::{PendingReceipt, Receipt},
        TransactionReceipt::{Declare, Deploy, DeployAccount, Invoke, L1Handler},
    },
    providers::StarknetErrorWithMessage,
};

use std::time::Duration;
use sysinfo::{CpuExt, System, SystemExt};

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

impl fmt::Display for SysInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "System Information:\nSystem : {} Kernel Version {}\nArch   : {}\nCPU    : {} {:.2}GHz {} cores\nMemory : {} GB",
            self.os_name,
            self.kernel_version,
            self.arch,
            self.cpu_brand,
            format!("{:.2} GHz", self.cpu_frequency as f64 / 1000.0),
            self.cpu_count,
            self.memory / (1024 * 1024 * 1024)
        )
    }
}

impl Serialize for SysInfo {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let sysinfo_string = format!("CPU Count: {}\nCPU Model: {}\nCPU Speed (MHz): {}\nTotal Memory: {} GB\nPlatform: {}\nRelease: {}\nArchitecture: {}", self.cpu_count, self.cpu_frequency, self.cpu_brand, self.memory, self.os_name, self.kernel_version, self.arch);
        serializer.serialize_str(&sysinfo_string)
    }
}

const WAIT_FOR_TX_TIMEOUT: Duration = Duration::from_secs(30);
const WAIT_FOR_TX_SLEEP: Duration = Duration::from_secs(2);

pub async fn wait_for_tx(
    provider: &JsonRpcClient<HttpTransport>,
    tx_hash: FieldElement,
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
                let status = match receipt {
                    Invoke(receipt) => receipt.status,
                    Declare(receipt) => receipt.status,
                    Deploy(receipt) => receipt.status,
                    DeployAccount(receipt) => receipt.status,
                    L1Handler(receipt) => receipt.status,
                };

                match status {
                    TransactionStatus::Pending => {
                        debug!("Waiting for transaction {tx_hash:#064x} to be accepted");
                        tokio::time::sleep(WAIT_FOR_TX_SLEEP).await;
                    }
                    TransactionStatus::AcceptedOnL2 | TransactionStatus::AcceptedOnL1 => {
                        return Ok(())
                    }
                    TransactionStatus::Rejected => {
                        return Err(eyre!(format!(
                            "Transaction {tx_hash:#064x} has been rejected"
                        )));
                    }
                }
            }
            Ok(PendingReceipt(_)) => {
                debug!("Waiting for transaction {tx_hash:#064x} to be accepted");
                tokio::time::sleep(WAIT_FOR_TX_SLEEP).await;
            }
            Err(ProviderError::StarknetError(StarknetErrorWithMessage {
                code: MaybeUnknownErrorCode::Known(StarknetError::TransactionHashNotFound),
                ..
            })) => {
                debug!("Waiting for transaction {tx_hash:#064x} to show up");
                tokio::time::sleep(WAIT_FOR_TX_SLEEP).await;
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
) -> Result<HashMap<u64, u64>> {
    let mut map = HashMap::new();

    for block_number in start_block..=end_block {
        let n = starknet_rpc
            .get_block_transaction_count(BlockId::Number(block_number))
            .await?;

        map.insert(block_number, n);
    }

    Ok(map)
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
