use std::{collections::HashMap, time::SystemTime};

use color_eyre::{eyre::eyre, Result};
use log::{info, debug};
use starknet::core::types::{
    MaybePendingTransactionReceipt::{PendingReceipt, Receipt},
    TransactionReceipt::{Declare, Deploy, DeployAccount, Invoke, L1Handler},
};
use starknet::core::types::{StarknetError, TransactionStatus};
use starknet::core::{crypto::compute_hash_on_elements, types::FieldElement};
use starknet::providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider};
use starknet::providers::{MaybeUnknownErrorCode, ProviderError};

use std::time::Duration;
use sysinfo::{CpuExt, System, SystemExt};

/// Cairo string for "STARKNET_CONTRACT_ADDRESS"
/// Used to calculate contract addresses
const PREFIX_CONTRACT_ADDRESS: FieldElement = FieldElement::from_mont([
    3829237882463328880,
    17289941567720117366,
    8635008616843941496,
    533439743893157637,
]);

// 2 ** 251 - 256
/// Used to calculate contract addresses
const ADDR_BOUND: FieldElement = FieldElement::from_mont([
    18446743986131443745,
    160989183,
    18446744073709255680,
    576459263475590224,
]);

/// Copied from starknet-rs since it's not public
pub fn calculate_contract_address(
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

pub fn get_sysinfo() -> HashMap<String, String> {
    let sys = System::new_all();
    let cpu = sys.global_cpu_info();

    let mut sysinfo = HashMap::new();

    let system = format!(
        "{} Kernel Version {}",
        sys.long_os_version().unwrap().trim(),
        sys.kernel_version().unwrap()
    );
    sysinfo.insert("System".to_string(), system);

    let cpu = format!(
        "{} {:.2}GHz {} cores",
        cpu.brand(),
        cpu.frequency() as f32 / 1000.0,
        sys.cpus().len()
    );
    sysinfo.insert("CPU".to_string(), cpu);

    let memory = format!("{} GB", sys.total_memory() / 1024 / 1024 / 1024);
    sysinfo.insert("Memory".to_string(), memory);

    sysinfo.insert("Arch".to_string(), std::env::consts::ARCH.to_string());

    sysinfo
}

pub fn pretty_print_hashmap(sysinfo: &HashMap<String, String>) {
    let key_max_length = sysinfo.keys().map(|key| key.len()).max().unwrap();

    for (name, value) in sysinfo {
        info!("{:key_max_length$} : {}", name, value);
    }
}

const WAIT_FOR_TX_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn wait_for_tx(
    provider: &JsonRpcClient<HttpTransport>,
    tx_hash: FieldElement,
) -> Result<&str> {
    let start = SystemTime::now();

    loop {
        if start.elapsed().unwrap() >= WAIT_FOR_TX_TIMEOUT {
            return Err(eyre!("Timeout while waiting for transaction {tx_hash:#064x}"))
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
                        debug!("Waiting for transaction {tx_hash:#064x} to be mined");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    TransactionStatus::AcceptedOnL2 | TransactionStatus::AcceptedOnL1 => {
                        return Ok("Transaction accepted")
                    }
                    TransactionStatus::Rejected => {
                        return Err(eyre!("Transaction has been rejected"));
                    }
                }
            }
            Ok(PendingReceipt(_)) => {
                debug!("Waiting for transaction {tx_hash:#064x} to be mined");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(ProviderError::StarknetError(e)) => {
                if let MaybeUnknownErrorCode::Known(e) = e.code {
                    if e == StarknetError::TransactionHashNotFound {
                        debug!("Waiting for transaction {tx_hash:#064x} to show up");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
            // TODO: use wrap_err
            Err(err) => return Err(eyre!(err)),
        }
    }
}
