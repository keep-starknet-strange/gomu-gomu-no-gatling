use statrs::statistics::Statistics;
use std::collections::HashMap;

use lazy_static::lazy_static;

pub static BLOCK_TIME: u64 = 6;

#[derive(PartialEq, Eq, Hash, Clone)]
pub struct Metric {
    pub id: String,
    pub name: String,
    pub unit: String,
    pub compute: fn(&HashMap<u64, u64>) -> f64,
}

fn average_tps(num_tx_per_block: &HashMap<u64, u64>) -> f64 {
    num_tx_per_block
        .values()
        .map(|x| *x as f64 / BLOCK_TIME as f64)
        .mean()
}

fn average_tbs(num_tx_per_block: &HashMap<u64, u64>) -> f64 {
    num_tx_per_block.values().map(|x| *x as f64).mean()
}

pub fn compute_all_metrics(num_tx_per_block: HashMap<u64, u64>) -> HashMap<Metric, f64> {
    let mut result = HashMap::new();

    for metric in METRICS.values() {
        result.insert(metric.clone(), (metric.compute)(&num_tx_per_block));
    }

    result
}

lazy_static! {
    pub static ref METRICS: HashMap<String, Metric> = {
        let metrics = vec![
            Metric {
                id: "average_tps".to_string(),
                name: "Average TPS".to_string(),
                unit: "transactions/second".to_string(),
                compute: average_tps,
            },
            Metric {
                id: "average_tbs".to_string(),
                name: "Average Extrinsics per block".to_string(),
                unit: "extrinsics/block".to_string(),
                compute: average_tbs,
            }
        ];

        metrics
            .into_iter()
            .map(|metric| (metric.id.clone(), metric))
            .collect()
    };
}
