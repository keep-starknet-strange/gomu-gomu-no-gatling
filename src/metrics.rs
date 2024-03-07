use crate::utils::get_blocks_with_txs;

use color_eyre::{
    eyre::{bail, OptionExt},
    Result,
};

use goose::metrics::{GooseMetrics, GooseRequestMetricTimingData};
use goose::metrics::{GooseMetrics, TransactionMetricAggregate, GooseRequestMetricTimingData};
use serde_derive::Serialize;
use starknet::{
    core::types::{
        BlockWithTxs, ExecutionResources, InvokeTransaction, InvokeTransactionV0,
        InvokeTransactionV1, L1HandlerTransaction, Transaction,
    },
    providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider},
};
use std::{fmt, sync::Arc};

pub const BLOCK_TIME: u64 = 6;

#[derive(Clone, Debug, Serialize)]
pub struct GlobalReport {
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
/// ### Example
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

/// A benchmark report contains the metrics for a single benchmark
/// it also includes the name, amount of times it was ran and
/// optionally metrics over the last x blocks
/// It implements the Serialize trait so it can be serialized to json
#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkReport {
    #[serde(skip_serializing_if = "str::is_empty")]
    pub name: String,
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
    pub fn new(name: String, amount: usize) -> BenchmarkReport {
        BenchmarkReport {
            name,
            amount,
            metrics: Vec::new(),
            last_x_blocks_metrics: None,
        }
    }

    pub async fn with_block_range(
        &mut self,
        starknet_rpc: &Arc<JsonRpcClient<HttpTransport>>,
        mut start_block: u64,
        mut end_block: u64,
    ) -> Result<()> {
        // Whenever possible, skip the first and last blocks from the metrics
        // to make sure all the blocks used for calculating metrics are full
        if end_block - start_block > 2 {
            start_block += 1;
            end_block -= 1;
        }

        let blocks_with_txs = get_blocks_with_txs(starknet_rpc, start_block..=end_block).await?;
        let metrics = compute_node_metrics(blocks_with_txs)?;

        self.metrics.extend_from_slice(&metrics);

        Ok(())
    }

    pub async fn with_last_x_blocks(
        &mut self,
        starknet_rpc: &Arc<JsonRpcClient<HttpTransport>>,
        num_blocks: u64,
    ) -> Result<()> {
        // The last block won't be full of transactions, so we skip it
        let end_block = starknet_rpc.block_number().await? - 1;
        let start_block = end_block - num_blocks;

        let blocks_with_txs = get_blocks_with_txs(starknet_rpc, start_block..=end_block).await?;
        let metrics = compute_node_metrics(blocks_with_txs)?;

        self.last_x_blocks_metrics = Some(LastXBlocksMetric {
            num_blocks,
            metrics,
        });

        Ok(())
    }

    pub fn with_goose_metrics(&mut self, metrics: &GooseMetrics) -> Result<()> {
        let transactions = metrics
            .transactions
            .first()
            .ok_or_eyre("Could no find scenario's transactions")?;

        let [_setup, submission, _finalizing, verification] = transactions.as_slice() else {
            bail!("Failed at getting all transaction aggragates")
        };

        let submission_requests = metrics
            .requests
            .get("POST Transaction Submission")
            .ok_or_eyre("Found no submission request metrics")?;

        let verification_requests = metrics
            .requests
            .get("POST Verification")
            .ok_or_eyre("Found no verification request metrics")?;

        self.metrics.extend_from_slice(&[
            MetricResult {
                name: "Total Submission Time",
                unit: "milliseconds",
                value: submission_requests.raw_data.total_time.into(),
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
                value: submission.fail_count.into(),
            },
            MetricResult {
                name: "Max Submission Time",
                unit: "milliseconds",
                value: submission_requests.raw_data.maximum_time.into(),
            },
            MetricResult {
                name: "Min Submission Time",
                unit: "milliseconds",
                value: submission_requests.raw_data.minimum_time.into(),
            },
            MetricResult {
                name: "Average Submission Time",
                unit: "milliseconds",
                value: transaction_average(requests).into(),
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
                value: transaction_average(requests).into(),
            },
        ]);

        if let Some((sub_p50, sub_90)) = calculate_p50_and_p90(&submission_requests.raw_data) {
            self.metrics.extend_from_slice(&[
                MetricResult {
                    name: "P90 Submission Time",
                    unit: "milliseconds",
                    value: sub_90.into(),
                },
                MetricResult {
                    name: "P50 Submission Time",
                    unit: "milliseconds",
                    value: sub_p50.into(),
                },
            ])
        }

        if let Some((ver_p50, ver_p90)) = calculate_p50_and_p90(&verification_requests.raw_data) {
            self.metrics.extend_from_slice(&[
                MetricResult {
                    name: "P90 Verification Time",
                    unit: "milliseconds",
                    value: ver_p90.into(),
                },
                MetricResult {
                    name: "P50 Verification Time",
                    unit: "milliseconds",
                    value: ver_p50.into(),
                },
            ])
        }

        Ok(())
    }
}

fn transaction_average(requests: &TransactionMetricAggregate) -> f64 {
    requests.total_time as f64 / requests.counter as f64
}

fn calculate_p50_and_p90(timing_data: &GooseRequestMetricTimingData) -> Option<(usize, usize)> {
    let p50_idx = (timing_data.counter * 50) / 100;

    let p90_idx = (timing_data.counter * 90) / 100;

    let mut ordered_times = timing_data
        .times
        .iter()
        .flat_map(|(time, &amount)| std::iter::repeat(time).take(amount));

    // These should only return None when there is only 1 or 0 times in the data
    let &p50 = ordered_times.nth(p50_idx)?;

    // p50 already iterated some out, so we subtract it's idx from here
    let &p90 = ordered_times.nth(p90_idx - p50_idx)?;

    Some((p50, p90))
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

pub fn compute_node_metrics(
    blocks_with_txs: Vec<(BlockWithTxs, Vec<ExecutionResources>)>,
) -> Result<Vec<MetricResult>> {
    let total_transactions: usize = blocks_with_txs
        .iter()
        .map(|(b, _)| b.transactions.len())
        .sum();
    let avg_tpb = total_transactions as f64 / blocks_with_txs.len() as f64;

    let mut metrics = vec![
        MetricResult {
            name: "Average TPS",
            unit: "transactions/second",
            value: (avg_tpb / BLOCK_TIME as f64).into(),
        },
        MetricResult {
            name: "Average Extrinsics per block",
            unit: "extrinsics/block",
            value: avg_tpb.into(),
        },
    ];

    let (first_block, _) = blocks_with_txs.first().ok_or_eyre("No first block")?;
    let (last_block, _) = blocks_with_txs.last().ok_or_eyre("No last block")?;

    if first_block.timestamp != last_block.timestamp {
        let total_uops: u64 = blocks_with_txs
            .iter()
            .flat_map(|(b, _)| &b.transactions)
            .map(tx_get_user_operations)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .sum();

        let total_steps: u64 = blocks_with_txs
            .iter()
            .flat_map(|(_, r)| r)
            .map(|resource| resource.steps)
            .sum();

        metrics.push(MetricResult {
            name: "Average UOPS",
            unit: "operations/second",
            value: (total_uops as f64 / blocks_with_txs.len() as f64 / BLOCK_TIME as f64).into(),
        });

        metrics.push(MetricResult {
            name: "Average Steps Per Second",
            unit: "operations/second",
            value: (total_steps as f64 / blocks_with_txs.len() as f64 / BLOCK_TIME as f64).into(),
        });
    }

    Ok(metrics)
}

fn tx_get_user_operations(tx: &Transaction) -> Result<u64> {
    Ok(match tx {
        Transaction::Invoke(
            InvokeTransaction::V0(InvokeTransactionV0 { calldata, .. })
            | InvokeTransaction::V1(InvokeTransactionV1 { calldata, .. }),
        )
        | Transaction::L1Handler(L1HandlerTransaction { calldata, .. }) => {
            let &user_operations = calldata
                .first()
                .ok_or_eyre("Expected calldata to have at least one field element")?;

            user_operations.try_into()?
        }
        _ => 1, // Other txs can be considered as 1 uop
    })
}
