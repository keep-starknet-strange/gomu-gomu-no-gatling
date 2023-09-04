use crate::utils::{get_num_tx_per_block, SYSINFO};

use color_eyre::Result;

use log::warn;
use serde::{ser::SerializeStruct, Serialize};
use starknet::providers::{jsonrpc::HttpTransport, JsonRpcClient};
use statrs::statistics::Statistics;
use std::{fmt, sync::Arc};

use lazy_static::lazy_static;

pub static BLOCK_TIME: u64 = 6;

/// Metric struct that contains the name, unit and compute function for a metric
/// A Metric is a measure of a specific performance aspect of a benchmark through
/// the compute function which receives a vector of number of transactions per block
/// and returns the metric value as a f64
/// The name and unit are used for displaying the metric
///
/// ### Example
/// { name: "Average TPS", unit: "transactions/second", compute: average_tps }
/// "Average TPS: 1000 transactions/second"
#[derive(PartialEq, Eq, Hash, Clone)]
pub struct Metric {
    pub name: String,
    pub unit: String,
    pub compute: fn(&Vec<u64>) -> f64,
}

/// A struct that contains the result of a metric computation alognside the name and unit
/// This struct is used for displaying the metric result
/// Example:
/// MetricResult { name: "Average TPS", unit: "transactions/second", value: 1000 }
/// "Average TPS: 1000 transactions/second"
#[derive(Debug, Clone)]
pub struct MetricResult {
    pub name: String,
    pub unit: String,
    pub value: f64,
}

/// A benchmark report contains a name (used for displaying) and a vector of metric results
/// of all the metrics that were computed for the benchmark
/// A benchmark report can be created from a block range or from the last x blocks
/// It implements the Serialize trait so it can be serialized to json
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct BenchmarkReport {
    #[serde(skip)]
    pub name: String,
    pub metrics: Vec<MetricResult>,
}

impl BenchmarkReport {
    pub async fn from_block_range(
        starknet_rpc: Arc<JsonRpcClient<HttpTransport>>,
        name: String,
        start_block: u64,
        end_block: u64,
    ) -> Result<Self> {
        let mut start_block = start_block;
        let mut end_block = end_block;

        // Whenever possible, skip the first and last blocks from the metrics
        // to make sure all the blocks used for calculating metrics are full
        if end_block - start_block > 2 {
            start_block += 1;
            end_block -= 1;
        }

        let num_tx_per_block = get_num_tx_per_block(starknet_rpc, start_block, end_block).await?;
        let metrics = compute_all_metrics(num_tx_per_block);

        Ok(Self { name, metrics })
    }

    pub async fn from_last_x_blocks(
        starknet_rpc: Arc<JsonRpcClient<HttpTransport>>,
        name: String,
        first_block: u64,
        last_block: u64,
        num_last_blocks: u64,
    ) -> Result<Self> {
        let mut start_block = last_block - num_last_blocks + 1;
        let mut end_block = last_block;

        let actual_num_blocks = last_block - first_block + 1;

        if num_last_blocks > actual_num_blocks {
            warn!("Creating benchmark report `{name}` using the last {num_last_blocks} blocks while only {actual_num_blocks} blocks have transactions, you should either use a lower number of blocks for the metrics or more transactions")
        } else {
            // Whenever possible, skip the first and last blocks from the metrics
            // to make sure all the blocks used for calculating metrics are full
            if actual_num_blocks - num_last_blocks > 2 {
                start_block += 1;
                end_block -= 1;
            }
        }

        let num_tx_per_block = get_num_tx_per_block(starknet_rpc, start_block, end_block).await?;
        let metrics = compute_all_metrics(num_tx_per_block);

        Ok(Self { name, metrics })
    }
}

impl fmt::Display for MetricResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} {}", self.name, self.value, self.unit)
    }
}

impl fmt::Display for BenchmarkReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Benchmark Report: {}", self.name)?;

        for metric in &self.metrics {
            writeln!(f, "{}", metric)?;
        }

        Ok(())
    }
}

impl Serialize for MetricResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut s = serializer.serialize_struct("metric", 4)?;

        let sysinfo_string = format!(
            "CPU Count: {}\n\
            CPU Model: {}\n\
            CPU Speed (MHz): {}\n\
            Total Memory: {} GB\n\
            Platform: {}\n\
            Release: {}\n\
            Architecture: {}",
            SYSINFO.cpu_count,
            SYSINFO.cpu_frequency,
            SYSINFO.cpu_brand,
            SYSINFO.memory / (1024 * 1024 * 1024),
            SYSINFO.os_name,
            SYSINFO.kernel_version,
            SYSINFO.arch
        );

        s.serialize_field("name", &self.name)?;
        s.serialize_field("unit", &self.unit)?;
        s.serialize_field("value", &self.value)?;
        s.serialize_field("extra", &sysinfo_string)?;

        s.end()
    }
}

fn average_tps(num_tx_per_block: &Vec<u64>) -> f64 {
    average_tpb(num_tx_per_block) / BLOCK_TIME as f64
}

fn average_tpb(num_tx_per_block: &Vec<u64>) -> f64 {
    num_tx_per_block.iter().map(|x| *x as f64).mean()
}

pub fn compute_all_metrics(num_tx_per_block: Vec<u64>) -> Vec<MetricResult> {
    METRICS
        .iter()
        .map(|metric| {
            let value = (metric.compute)(&num_tx_per_block);
            MetricResult {
                name: metric.name.clone(),
                unit: metric.unit.clone(),
                value,
            }
        })
        .collect()
}

lazy_static! {
    pub static ref METRICS: Vec<Metric> = vec![
        Metric {
            name: "Average TPS".to_string(),
            unit: "transactions/second".to_string(),
            compute: average_tps,
        },
        Metric {
            name: "Average Extrinsics per block".to_string(),
            unit: "extrinsics/block".to_string(),
            compute: average_tpb,
        },
    ];
}
