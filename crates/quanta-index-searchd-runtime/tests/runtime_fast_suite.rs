#![forbid(unsafe_code)]

use quanta_index_searchd_harness as e2e_harness;

#[path = "common/e2e_corpus.rs"]
mod e2e_corpus;
#[path = "common/fail_closed_wait.rs"]
mod fail_closed_wait;
#[path = "common/frontdoor_scenarios.rs"]
mod frontdoor_scenarios;
#[path = "common/searchd_binary_process.rs"]
mod searchd_binary_process;

#[path = "e2e_explain_score_trace.rs"]
mod e2e_explain_score_trace;
#[path = "e2e_hybrid_filters.rs"]
mod e2e_hybrid_filters;
#[path = "e2e_ingest_idempotency.rs"]
mod e2e_ingest_idempotency;
#[path = "e2e_matrix_inventory.rs"]
mod e2e_matrix_inventory;
#[path = "e2e_read_view.rs"]
mod e2e_read_view;
#[path = "e2e_structural_hellgate.rs"]
mod e2e_structural_hellgate;
#[path = "e2e_text_route_hellgate.rs"]
mod e2e_text_route_hellgate;
#[path = "end_to_end.rs"]
mod end_to_end;
#[path = "sdk_frontdoor.rs"]
mod sdk_frontdoor;
