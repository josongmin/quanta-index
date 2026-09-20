#![forbid(unsafe_code)]

use quanta_index_searchd_harness as e2e_harness;

#[path = "common/frontdoor_scenarios.rs"]
mod frontdoor_scenarios;

#[path = "dsl_scenarios.rs"]
mod dsl_scenarios;
#[path = "e2e_aux_epoch.rs"]
mod e2e_aux_epoch;
#[path = "e2e_boot_quarantine.rs"]
mod e2e_boot_quarantine;
#[path = "e2e_dual_syntax_lowering_parity.rs"]
mod e2e_dual_syntax_lowering_parity;
#[path = "e2e_exact_count_window.rs"]
mod e2e_exact_count_window;
#[path = "e2e_filter_execution.rs"]
mod e2e_filter_execution;
#[path = "e2e_full_corpus.rs"]
mod e2e_full_corpus;
#[path = "e2e_generation_activation_concurrency.rs"]
mod e2e_generation_activation_concurrency;
#[path = "e2e_history_relevance.rs"]
mod e2e_history_relevance;
#[path = "e2e_keyset_cursors.rs"]
mod e2e_keyset_cursors;
#[path = "e2e_lexical_full_fidelity.rs"]
mod e2e_lexical_full_fidelity;
#[path = "e2e_perf_chaos.rs"]
mod e2e_perf_chaos;
#[path = "e2e_physical_gc.rs"]
mod e2e_physical_gc;
#[path = "e2e_predicate_authority_boolean.rs"]
mod e2e_predicate_authority_boolean;
#[path = "e2e_predicate_authority_lifecycle.rs"]
mod e2e_predicate_authority_lifecycle;
#[path = "e2e_restart_replay_determinism.rs"]
mod e2e_restart_replay_determinism;
#[path = "e2e_semantic_scope_cap.rs"]
mod e2e_semantic_scope_cap;
#[path = "e2e_snapshot_registry.rs"]
mod e2e_snapshot_registry;
#[path = "e2e_top_k_truth_table.rs"]
mod e2e_top_k_truth_table;
#[path = "explain.rs"]
mod explain;
#[path = "repo_map_end_to_end.rs"]
mod repo_map_end_to_end;
