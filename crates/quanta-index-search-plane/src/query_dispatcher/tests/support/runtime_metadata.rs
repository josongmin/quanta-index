//! Runtime-metadata authority ledgers and request builders.

use std::sync::{Arc, RwLock};

use quanta_index_contract::lex::DirtyRecord;
use quanta_index_contract::{
    ChunkId, DirtyIngestBatch, DirtyMutation, ManifestGeneration, RepoId, RevisionId,
    RuntimeCatalogIngestBatch, RuntimeChangedRecord, RuntimeDocFacetRecord,
    RuntimeEdgeAuthorityRecord, RuntimeMetadataQueryRequest, RuntimeSnapshotRecord,
    SearchPlaneQueryIpcRequest, TextQueryRequest, TextQuerySyntax,
};

use crate::Ledger;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::tests::support::common::{
    ready_ledger, ready_pin, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::RejectLexicalOpener;
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapQueryPort;
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;
use crate::query_dispatcher::tests::support::structural::{
    FailClosedStructuralProducer, install_structural_test_chunk,
};

pub(crate) fn runtime_metadata_dispatcher_with_ledger(
    ledger: Arc<RwLock<Ledger>>,
) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>> {
    Ok(SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ledger,
        test_activation_catalog()?,
    ))
}

pub(crate) fn runtime_query_request(
    syntax: TextQuerySyntax,
    query_text: &str,
) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
        text_query: TextQueryRequest {
            syntax,
            query_text: query_text.to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 5,
        },
    })
}

pub(crate) fn ready_runtime_metadata_ledger(
    producer_head_applied_at_ms: u64,
    generation_materialized_at_ms: u64,
) -> Arc<RwLock<Ledger>> {
    let ledger = ready_ledger();
    {
        let mut guard = ledger
            .write()
            .expect("runtime metadata test ledger poisoned");
        for (chunk_id, path, text) in [
            ("chunk-dirty", "src/dirty.rs", "todo dirty"),
            ("chunk-clean", "src/clean.rs", "todo clean"),
            ("chunk-changed", "src/changed.rs", "catalog changed"),
            ("chunk-stale", "src/stale.rs", "catalog stale"),
            ("chunk-snapshot", "src/snapshot.rs", "catalog snapshot"),
            ("chunk-owner", "src/owner.rs", "catalog owner"),
        ] {
            install_structural_test_chunk(&mut guard, chunk_id, path, text)
                .expect("runtime metadata test chunk install");
        }
        guard.apply_runtime_batch(&DirtyIngestBatch {
            repo_id: RepoId::new("repo-map-ipc"),
            revision_id: RevisionId::new("rev-map-ipc"),
            generation: ManifestGeneration::new(9),
            overlay_epoch_ms: 100,
            batch_digest: "dirty:test".to_string(),
            entries: vec![DirtyMutation::Upsert(DirtyRecord {
                wire_version: 1,
                doc_id: ChunkId::new("chunk-dirty"),
                applied_at_ms: 100,
                payload_hash: [0x5a; 32],
            })],
        });
        guard
            .apply_runtime_catalog_batch(&RuntimeCatalogIngestBatch {
                repo_id: RepoId::new("repo-map-ipc"),
                revision_id: RevisionId::new("rev-map-ipc"),
                generation: ManifestGeneration::new(9),
                overlay_epoch_ms: 20,
                batch_digest: "catalog:test".to_string(),
                producer_head_applied_at_ms,
                generation_materialized_at_ms,
                changed_entries: vec![RuntimeChangedRecord {
                    doc_id: ChunkId::new("chunk-changed"),
                    applied_at_ms: 25,
                    payload_hash: [0xaa; 32],
                }],
                facet_entries: vec![RuntimeDocFacetRecord {
                    doc_id: ChunkId::new("chunk-owner"),
                    owner: Some("team-a".to_string()),
                    service: Some("search".to_string()),
                    layer: Some("index".to_string()),
                    surface: Some("lexical".to_string()),
                }],
                snapshot_entries: vec![RuntimeSnapshotRecord {
                    name: "active".to_string(),
                    doc_ids: vec![ChunkId::new("chunk-snapshot")],
                }],
                affected_entries: vec![RuntimeEdgeAuthorityRecord {
                    key: "rebuild=lexical".to_string(),
                    doc_ids: vec![ChunkId::new("chunk-changed")],
                }],
                invalidated_by_entries: vec![RuntimeEdgeAuthorityRecord {
                    key: "rebuild=lexical".to_string(),
                    doc_ids: vec![ChunkId::new("chunk-changed")],
                }],
            })
            .expect("runtime metadata test catalog install");
    }
    ledger
}
