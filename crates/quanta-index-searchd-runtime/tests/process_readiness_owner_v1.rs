#![forbid(unsafe_code)]

// Current SDK compatibility with an explicit pinned Legacy reference.
// Native Linux process-readiness acceptance is a separate product gate.
#[path = "composite_generation_authority_restart.rs"]
mod composite_generation_authority_restart;
#[path = "e2e_ann_incremental_seal.rs"]
mod e2e_ann_incremental_seal;
#[path = "e2e_aux_epoch.rs"]
mod e2e_aux_epoch;
#[path = "e2e_auxiliary_catalog.rs"]
mod e2e_auxiliary_catalog;
#[path = "e2e_auxiliary_catalog_history.rs"]
mod e2e_auxiliary_catalog_history;
#[path = "e2e_boot_quarantine.rs"]
mod e2e_boot_quarantine;
#[path = "e2e_coverage_pipeline_cost.rs"]
mod e2e_coverage_pipeline_cost;
#[path = "e2e_exact_count_window.rs"]
mod e2e_exact_count_window;
#[path = "e2e_generation_activation_concurrency.rs"]
mod e2e_generation_activation_concurrency;
#[path = "e2e_history_order.rs"]
mod e2e_history_order;
#[path = "e2e_history_relevance.rs"]
mod e2e_history_relevance;
#[path = "e2e_history_text_predicate.rs"]
mod e2e_history_text_predicate;
#[path = "e2e_ingest_idempotency.rs"]
mod e2e_ingest_idempotency;
#[path = "e2e_ingest_preflight.rs"]
mod e2e_ingest_preflight;
#[path = "e2e_ingest_resource_envelope.rs"]
mod e2e_ingest_resource_envelope;
#[path = "e2e_integrity_scrub.rs"]
mod e2e_integrity_scrub;
#[path = "e2e_keyset_cursors.rs"]
mod e2e_keyset_cursors;
#[path = "e2e_legacy_reference_metrics.rs"]
mod e2e_legacy_reference_metrics;
#[path = "e2e_legacy_reference_scale_restart.rs"]
mod e2e_legacy_reference_scale_restart;
#[path = "e2e_lexical_full_fidelity.rs"]
mod e2e_lexical_full_fidelity;
#[path = "e2e_lexical_sealed_overlays.rs"]
mod e2e_lexical_sealed_overlays;
#[path = "e2e_lifecycle_history.rs"]
mod e2e_lifecycle_history;
#[path = "e2e_matrix_inventory.rs"]
mod e2e_matrix_inventory;
#[path = "legacy_perf_chaos_routes.rs"]
mod e2e_perf_chaos;
#[path = "e2e_physical_gc.rs"]
mod e2e_physical_gc;
#[path = "e2e_predicate_authority_boolean.rs"]
mod e2e_predicate_authority_boolean;
#[path = "e2e_predicate_authority_lifecycle.rs"]
mod e2e_predicate_authority_lifecycle;
#[path = "e2e_ranked_pages.rs"]
mod e2e_ranked_pages;
#[path = "e2e_restart_replay_determinism.rs"]
mod e2e_restart_replay_determinism;
#[path = "e2e_runtime_corpus_inventory.rs"]
mod e2e_runtime_corpus_inventory;
#[path = "e2e_scale_process_restart.rs"]
mod e2e_scale_process_restart;
#[path = "e2e_semantic_scope_cap.rs"]
mod e2e_semantic_scope_cap;
#[path = "e2e_socket_access.rs"]
mod e2e_socket_access;
#[path = "e2e_top_k_truth_table.rs"]
mod e2e_top_k_truth_table;
#[path = "e2e_umask_hardening.rs"]
mod e2e_umask_hardening;
#[path = "e2e_unicode_text_semantics.rs"]
mod e2e_unicode_text_semantics;
#[path = "explain.rs"]
mod explain;
#[path = "common/fail_closed_wait.rs"]
mod fail_closed_wait;
#[path = "l4_preview_sdk.rs"]
mod l4_preview_sdk;
#[path = "legacy_boot_refusal.rs"]
mod legacy_boot_refusal;
#[path = "legacy_dirty_only_route.rs"]
mod legacy_dirty_only_route;
#[path = "legacy_explain_score_trace.rs"]
mod legacy_explain_score_trace;
#[path = "legacy_hybrid_filters.rs"]
mod legacy_hybrid_filters;
#[path = "legacy_l1_query_contract_sdk.rs"]
mod legacy_l1_query_contract_sdk;
#[path = "legacy_lexical_semantic_replay_sdk.rs"]
mod legacy_lexical_semantic_replay_sdk;
#[path = "legacy_offline_refusal.rs"]
mod legacy_offline_refusal;
#[path = "legacy_physical_retention_sdk.rs"]
mod legacy_physical_retention_sdk;
#[path = "legacy_provider_metrics.rs"]
mod legacy_provider_metrics;
#[path = "legacy_read_view_domains.rs"]
mod legacy_read_view_domains;
#[path = "legacy_read_view_selection.rs"]
mod legacy_read_view_selection;
#[path = "legacy_reference_process/mod.rs"]
mod legacy_reference_process;
#[path = "legacy_rev_at_time_route.rs"]
mod legacy_rev_at_time_route;
#[path = "legacy_sdk_code_search_frontdoor.rs"]
mod legacy_sdk_code_search_frontdoor;
#[path = "common/legacy_searchd_binary.rs"]
mod legacy_searchd_binary;
#[path = "legacy_semantic_activation_authority_sdk.rs"]
mod legacy_semantic_activation_authority_sdk;
#[path = "legacy_snapshot_registry_sdk.rs"]
mod legacy_snapshot_registry_sdk;
#[path = "legacy_socket_access_refusal.rs"]
mod legacy_socket_access_refusal;
#[path = "legacy_source_reservation_sdk.rs"]
mod legacy_source_reservation_sdk;
#[path = "common/legacy_start_probe.rs"]
mod legacy_start_probe;
#[path = "legacy_structural_hellgate.rs"]
mod legacy_structural_hellgate;
#[path = "legacy_text_route_hellgate.rs"]
mod legacy_text_route_hellgate;
#[path = "common/offline_legacy_root.rs"]
mod offline_legacy_root;
#[path = "common/offline_lexical_pool.rs"]
mod offline_lexical_pool;
#[path = "common/offline_live_root.rs"]
mod offline_live_root;
#[path = "offline_state_custody.rs"]
mod offline_state_custody;
#[path = "common/offline_tree_inventory.rs"]
mod offline_tree_inventory;
#[path = "common/process_envelope_scrape.rs"]
mod process_envelope_scrape;
#[path = "repo_map_end_to_end.rs"]
mod repo_map_end_to_end;
#[path = "legacy_reference_process/scale_process_fixture.rs"]
mod scale_process_fixture;
#[path = "common/sdk_frontdoor_lexical_fixture.rs"]
mod sdk_frontdoor_lexical_fixture;
#[path = "semantic_boot_report.rs"]
mod semantic_boot_report;
