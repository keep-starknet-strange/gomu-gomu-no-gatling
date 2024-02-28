use crate::utils::get_num_tx_per_block;

use color_eyre::{
    eyre::{bail, eyre},
    Result,
};

use goose::metrics::GooseMetrics;
use serde_derive::Serialize;
use starknet::providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider};
use std::fmt;

pub const BLOCK_TIME: u64 = 6;

#[derive(Clone, Debug, Serialize)]
pub struct WholeReport {
    pub users: u64,
    pub all_bench_report: BenchmarkReport,
    pub benches: Vec<BenchmarkReport>,
    pub extra: String,
}

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
pub struct NodeMetrics {
    pub name: &'static str,
    pub unit: &'static str,
    pub compute: fn(&[u64]) -> f64,
}

/// A struct that contains the result of a metric computation alognside the name and unit
/// This struct is used for displaying the metric result
/// Example:
/// MetricResult { name: "Average TPS", unit: "transactions/second", value: 1000 }
/// "Average TPS: 1000 transactions/second"
#[derive(Debug, Clone, Serialize)]
pub struct MetricResult {
    pub name: &'static str,
    pub unit: &'static str,
    pub value: serde_json::Value,
}

/// A benchmark report contains a name and a vector of metric results
/// of all the metrics that were computed for the benchmark
/// A benchmark report can be created from a block range or from the last x blocks
/// It implements the Serialize trait so it can be serialized to json
#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkReport {
    #[serde(skip_serializing_if = "str::is_empty")]
    pub name: &'static str,
    pub amount: usize,
    pub metrics: Vec<MetricResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_x_blocks_metrics: Option<LastXBlocksMetric>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LastXBlocksMetric {
    pub num_blocks: u64,
    pub metrics: Vec<MetricResult>,
}

impl BenchmarkReport {
    pub fn new(name: &'static str, amount: usize) -> BenchmarkReport {
        BenchmarkReport {
            name,
            amount,
            metrics: Vec::new(),
            last_x_blocks_metrics: None,
        }
    }

    pub async fn with_block_range(
        &mut self,
        starknet_rpc: &JsonRpcClient<HttpTransport>,
        mut start_block: u64,
        mut end_block: u64,
    ) -> Result<()> {
        // Whenever possible, skip the first and last blocks from the metrics
        // to make sure all the blocks used for calculating metrics are full
        if end_block - start_block > 2 {
            start_block += 1;
            end_block -= 1;
        }

        let num_tx_per_block = get_num_tx_per_block(starknet_rpc, start_block, end_block).await?;
        let metrics = compute_node_metrics(num_tx_per_block);

        self.metrics.extend_from_slice(&metrics);

        Ok(())
    }

    pub async fn with_last_x_blocks(
        &mut self,
        starknet_rpc: &JsonRpcClient<HttpTransport>,
        num_blocks: u64,
    ) -> Result<()> {
        // The last block won't be full of transactions, so we skip it
        let end_block = starknet_rpc.block_number().await? - 1;
        let start_block = end_block - num_blocks;

        let num_tx_per_block = get_num_tx_per_block(starknet_rpc, start_block, end_block).await?;
        let metrics = compute_node_metrics(num_tx_per_block).to_vec();

        self.last_x_blocks_metrics = Some(LastXBlocksMetric {
            num_blocks,
            metrics,
        });

        Ok(())
    }

    pub fn with_goose_metrics(&mut self, metrics: &GooseMetrics) -> Result<()> {
        let scenario = metrics
            .scenarios
            .first()
            .ok_or(eyre!("There is no scenario"))?;

        let transactions = metrics
            .transactions
            .first()
            .ok_or(eyre!("Could no find scenario's transactions"))?;

        let [_setup, requests, _finalizing, verification] = transactions.as_slice() else {
            bail!("Failed at getting all transaction aggragates")
        };

        let verification_requests = metrics
            .requests
            .get("POST Verification")
            .ok_or(eyre!("Found no verification request metrics"))?;

        self.metrics.extend_from_slice(&[
            MetricResult {
                name: "Total Submission Time",
                unit: "milliseconds",
                value: requests.total_time.into(),
            },
            MetricResult {
                name: "Total Verification Time",
                unit: "milliseconds",
                value: verification.total_time.into(),
            },
            MetricResult {
                name: "Failed Transactions Verifications",
                unit: "",
                value: verification_requests.fail_count.into(),
            },
            MetricResult {
                name: "Failed Transaction Submissions",
                unit: "",
                value: requests.fail_count.into(),
            },
            MetricResult {
                name: "Max Submission Time",
                unit: "milliseconds",
                value: requests.max_time.into(),
            },
            MetricResult {
                name: "Min Submission Time",
                unit: "milliseconds",
                value: requests.min_time.into(),
            },
            MetricResult {
                name: "Average Submission Time",
                unit: "milliseconds",
                value: (requests.total_time as f64 / scenario.counter as f64).into(),
            },
            MetricResult {
                name: "Max Verification Time",
                unit: "milliseconds",
                value: verification_requests.raw_data.maximum_time.into(),
            },
            MetricResult {
                name: "Min Verification Time",
                unit: "milliseconds",
                value: verification_requests.raw_data.minimum_time.into(),
            },
            MetricResult {
                name: "Average Verification Time",
                unit: "milliseconds",
                value: (verification_requests.raw_data.total_time as f64 / scenario.counter as f64)
                    .into(),
            },
        ]);

        Ok(())
    }
}

impl fmt::Display for MetricResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { name, value, unit } = self;

        write!(f, "{name}: {value} {unit}")
    }
}

impl fmt::Display for BenchmarkReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            name,
            amount,
            metrics,
            last_x_blocks_metrics: last_x_blocks,
        } = self;

        writeln!(f, "Benchmark Report: {name} ({amount})")?;

        for metric in metrics {
            writeln!(f, "{metric}")?;
        }

        if let Some(last_x_blocks) = last_x_blocks {
            writeln!(f, "Last {} block metrics:", last_x_blocks.num_blocks)?;

            for metric in &last_x_blocks.metrics {
                writeln!(f, "{metric}")?;
            }
        }

        Ok(())
    }
}

pub fn compute_node_metrics(num_tx_per_block: Vec<u64>) -> [MetricResult; 2] {
    [
        MetricResult {
            name: "Average TPS",
            unit: "transactions/second",
            value: average_tps(&num_tx_per_block).into(),
        },
        MetricResult {
            name: "Average Extrinsics per block",
            unit: "extrinsics/block",
            value: average_tpb(&num_tx_per_block).into(),
        },
    ]
}

fn average_tps(num_tx_per_block: &[u64]) -> f64 {
    average_tpb(num_tx_per_block) / BLOCK_TIME as f64
}

fn average_tpb(num_tx_per_block: &[u64]) -> f64 {
    num_tx_per_block.iter().sum::<u64>() as f64 / num_tx_per_block.len() as f64
}
