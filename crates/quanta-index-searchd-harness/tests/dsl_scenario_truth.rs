#![forbid(unsafe_code)]

use anyhow::Result as AnyResult;
use quanta_index_searchd_harness::bench_support::{
    ScenarioTruthMode, prepare_cold_runtime, prepare_warm_runtime, run_scenario_query,
    validate_scenario_outcome,
};
use quanta_index_searchd_harness::scenarios::SCENARIOS;

#[test]
fn warm_matrix_scenarios_match_golden_truth() -> AnyResult<()> {
    let mut runtime = prepare_warm_runtime()?;
    for scenario in SCENARIOS {
        let outcome = run_scenario_query(&mut runtime, scenario);
        validate_scenario_outcome(scenario, ScenarioTruthMode::SharedWarmFixture, &outcome)
            .map_err(|err| {
                anyhow::anyhow!("warm scenario {} drifted from golden truth: {err:#}", scenario.id)
            })?;
    }
    Ok(())
}

const COLD_SHARD_COUNT: usize = 4;

fn cold_matrix_shard_matches_golden_truth(shard: usize) -> AnyResult<()> {
    for (index, scenario) in SCENARIOS.iter().enumerate() {
        if index % COLD_SHARD_COUNT != shard {
            continue;
        }
        let mut runtime = prepare_cold_runtime(scenario)?;
        let outcome = run_scenario_query(&mut runtime, scenario);
        validate_scenario_outcome(scenario, ScenarioTruthMode::IsolatedFixture, &outcome).map_err(
            |err| {
                anyhow::anyhow!("cold scenario {} drifted from golden truth: {err:#}", scenario.id)
            },
        )?;
    }
    Ok(())
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
