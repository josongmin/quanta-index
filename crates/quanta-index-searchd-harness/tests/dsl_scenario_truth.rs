#![forbid(unsafe_code)]

use anyhow::Result as AnyResult;
use quanta_index_searchd_harness::bench_support::{
    ScenarioTruthMode, validate_pinned_scenario_truth,
};
use quanta_index_searchd_harness::scenarios::SCENARIOS;

#[test]
fn warm_matrix_scenarios_match_golden_truth() -> AnyResult<()> {
    validate_pinned_scenario_truth(ScenarioTruthMode::SharedWarmFixture, SCENARIOS.iter())
}

const COLD_SHARD_COUNT: usize = 4;

fn cold_matrix_shard_matches_golden_truth(shard: usize) -> AnyResult<()> {
    validate_pinned_scenario_truth(
        ScenarioTruthMode::IsolatedFixture,
        SCENARIOS
            .iter()
            .enumerate()
            .filter_map(|(index, scenario)| {
                (index % COLD_SHARD_COUNT == shard).then_some(scenario)
            }),
    )
}

#[test]
fn cold_matrix_shard_0_matches_golden_truth() -> AnyResult<()> {
    cold_matrix_shard_matches_golden_truth(0)
}

#[test]
fn cold_matrix_shard_1_matches_golden_truth() -> AnyResult<()> {
    cold_matrix_shard_matches_golden_truth(1)
}

#[test]
fn cold_matrix_shard_2_matches_golden_truth() -> AnyResult<()> {
    cold_matrix_shard_matches_golden_truth(2)
}

#[test]
fn cold_matrix_shard_3_matches_golden_truth() -> AnyResult<()> {
    cold_matrix_shard_matches_golden_truth(3)
}
