#![forbid(unsafe_code)]

#[path = "common/searchd_binary_process.rs"]
mod searchd_binary_process;
#[path = "common/searchd_lease_probe.rs"]
mod searchd_lease_probe;

#[path = "composite_generation_authority_restart.rs"]
mod composite_generation_authority_restart;
#[path = "e2e_crash_matrix.rs"]
mod e2e_crash_matrix;
#[path = "e2e_lexical_sealed_overlays.rs"]
mod e2e_lexical_sealed_overlays;
#[path = "e2e_integrity_scrub.rs"]
mod e2e_integrity_scrub;
#[path = "e2e_ingest_resource_envelope.rs"]
mod e2e_ingest_resource_envelope;
#[path = "e2e_ingest_preflight.rs"]
mod e2e_ingest_preflight;
#[path = "e2e_auxiliary_catalog.rs"]
mod e2e_auxiliary_catalog;
#[path = "e2e_metrics_scrape.rs"]
mod e2e_metrics_scrape;
#[path = "e2e_socket_access.rs"]
mod e2e_socket_access;
#[path = "e2e_umask_hardening.rs"]
mod e2e_umask_hardening;
#[path = "e2e_process_envelope.rs"]
mod e2e_process_envelope;
#[path = "e2e_history_order.rs"]
mod e2e_history_order;
#[path = "e2e_ranked_pages.rs"]
mod e2e_ranked_pages;
#[path = "e2e_history_text_predicate.rs"]
mod e2e_history_text_predicate;
#[path = "e2e_unicode_text_semantics.rs"]
mod e2e_unicode_text_semantics;
#[path = "e2e_ann_incremental_seal.rs"]
mod e2e_ann_incremental_seal;
#[path = "e2e_semantic_budget_interruption.rs"]
mod e2e_semantic_budget_interruption;
#[path = "e2e_semantic_stream_window.rs"]
mod e2e_semantic_stream_window;
#[path = "semantic_boot_report.rs"]
mod semantic_boot_report;
