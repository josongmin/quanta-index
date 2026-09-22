use quanta_index_contract::{
    ChunkId, RuntimeCatalogIngestBatch, RuntimeChangedRecord, RuntimeDocFacetRecord,
    RuntimeEdgeAuthorityRecord, RuntimeSnapshotRecord,
};
use quanta_index_core::CoreError;

use crate::readiness::ledger::Ledger;
use crate::readiness::tests::support::{
    TestResult, generation, install_chunk, install_chunk_with_id, repo_id, revision_id,
};

#[test]
fn runtime_catalog_batch_replaces_previous_snapshot_state() -> TestResult {
    let mut ledger = Ledger::default();
    install_chunk_with_id(
        &mut ledger,
        "changed-1",
        "src/changed.rs",
        "fn changed() {}",
    )?;
    install_chunk_with_id(&mut ledger, "facet-1", "src/facet.rs", "fn facet() {}")?;
    install_chunk_with_id(&mut ledger, "snap-1", "src/snap.rs", "fn snap() {}")?;
    install_chunk_with_id(
        &mut ledger,
        "affected-1",
        "src/affected.rs",
        "fn affected() {}",
    )?;
    install_chunk_with_id(
        &mut ledger,
        "invalidated-1",
        "src/invalidated.rs",
        "fn invalidated() {}",
    )?;
    install_chunk_with_id(
        &mut ledger,
        "changed-2",
        "src/changed2.rs",
        "fn changed2() {}",
    )?;

    ledger.apply_runtime_catalog_batch(
        &RuntimeCatalogIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            overlay_epoch_ms: 10,
            batch_digest: "catalog-v1".to_string(),
            producer_head_applied_at_ms: 100,
            generation_materialized_at_ms: 20,
            changed_entries: vec![RuntimeChangedRecord {
                doc_id: ChunkId::new("changed-1"),
                applied_at_ms: 25,
                payload_hash: [0xbb; 32],
            }],
            facet_entries: vec![RuntimeDocFacetRecord {
                doc_id: ChunkId::new("facet-1"),
                owner: Some("team-a".to_string()),
                service: None,
                layer: None,
                surface: None,
            }],
            snapshot_entries: vec![RuntimeSnapshotRecord {
                name: "active".to_string(),
                doc_ids: vec![ChunkId::new("snap-1")],
            }],
            affected_entries: vec![RuntimeEdgeAuthorityRecord {
                key: "rebuild=lexical".to_string(),
                doc_ids: vec![ChunkId::new("affected-1")],
            }],
            invalidated_by_entries: vec![RuntimeEdgeAuthorityRecord {
                key: "rebuild=lexical".to_string(),
                doc_ids: vec![ChunkId::new("invalidated-1")],
            }],
        },
        std::time::Instant::now(),
    )?;

    ledger.apply_runtime_catalog_batch(
        &RuntimeCatalogIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            overlay_epoch_ms: 11,
            batch_digest: "catalog-v2".to_string(),
            producer_head_applied_at_ms: 101,
            generation_materialized_at_ms: 21,
            changed_entries: vec![RuntimeChangedRecord {
                doc_id: ChunkId::new("changed-2"),
                applied_at_ms: 30,
                payload_hash: [0xcc; 32],
            }],
            facet_entries: Vec::new(),
            snapshot_entries: Vec::new(),
            affected_entries: Vec::new(),
            invalidated_by_entries: Vec::new(),
        },
        std::time::Instant::now(),
    )?;

    let runtime = ledger
        .runtime_state(&repo_id(), &revision_id(), generation())
        .ok_or("runtime catalog state missing")?;
    if runtime
        .changed_docs()
        .contains_key(&ChunkId::new("changed-1"))
    {
        return Err("runtime catalog retained old changed-doc entry after replacement".into());
    }
    if !runtime
        .changed_docs()
        .contains_key(&ChunkId::new("changed-2"))
    {
        return Err("runtime catalog did not materialize new changed-doc entry".into());
    }
    if !runtime.doc_facets().is_empty()
        || !runtime.snapshots().is_empty()
        || !runtime.affected_docs().is_empty()
        || !runtime.invalidated_by_docs().is_empty()
    {
        return Err("runtime catalog replacement failed to drop removed keyspaces".into());
    }
    Ok(())
}

#[test]
fn runtime_catalog_batch_rejects_older_epoch_replay() -> TestResult {
    let mut ledger = Ledger::default();
    install_chunk_with_id(
        &mut ledger,
        "changed-1",
        "src/changed.rs",
        "fn changed() {}",
    )?;
    ledger.apply_runtime_catalog_batch(
        &RuntimeCatalogIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            overlay_epoch_ms: 10,
            batch_digest: "catalog-v1".to_string(),
            producer_head_applied_at_ms: 100,
            generation_materialized_at_ms: 20,
            changed_entries: vec![RuntimeChangedRecord {
                doc_id: ChunkId::new("changed-1"),
                applied_at_ms: 25,
                payload_hash: [0xbb; 32],
            }],
            facet_entries: Vec::new(),
            snapshot_entries: Vec::new(),
            affected_entries: Vec::new(),
            invalidated_by_entries: Vec::new(),
        },
        std::time::Instant::now(),
    )?;

    let err = ledger
        .apply_runtime_catalog_batch(
            &RuntimeCatalogIngestBatch {
                repo_id: repo_id(),
                revision_id: revision_id(),
                generation: generation(),
                overlay_epoch_ms: 9,
                batch_digest: "catalog-stale".to_string(),
                producer_head_applied_at_ms: 101,
                generation_materialized_at_ms: 21,
                changed_entries: Vec::new(),
                facet_entries: Vec::new(),
                snapshot_entries: Vec::new(),
                affected_entries: Vec::new(),
                invalidated_by_entries: Vec::new(),
            },
            std::time::Instant::now(),
        )
        .err()
        .ok_or("expected stale runtime catalog replay to fail")?;
    match err {
        CoreError::Typed { code, .. }
            if code == crate::readiness::errors::ERR_RUNTIME_CATALOG_STALE_BATCH => {}
        other @ (CoreError::Typed { .. }
        | CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => {
            return Err(format!("expected stale batch typed error, got {other:?}").into());
        }
    }
    Ok(())
}

#[test]
fn runtime_catalog_batch_rejects_conflicting_same_epoch_replay() -> TestResult {
    let mut ledger = Ledger::default();
    install_chunk_with_id(
        &mut ledger,
        "changed-1",
        "src/changed.rs",
        "fn changed() {}",
    )?;
    ledger.apply_runtime_catalog_batch(
        &RuntimeCatalogIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            overlay_epoch_ms: 10,
            batch_digest: "catalog-v1".to_string(),
            producer_head_applied_at_ms: 100,
            generation_materialized_at_ms: 20,
            changed_entries: vec![RuntimeChangedRecord {
                doc_id: ChunkId::new("changed-1"),
                applied_at_ms: 25,
                payload_hash: [0xbb; 32],
            }],
            facet_entries: Vec::new(),
            snapshot_entries: Vec::new(),
            affected_entries: Vec::new(),
            invalidated_by_entries: Vec::new(),
        },
        std::time::Instant::now(),
    )?;

    let err = ledger
        .apply_runtime_catalog_batch(
            &RuntimeCatalogIngestBatch {
                repo_id: repo_id(),
                revision_id: revision_id(),
                generation: generation(),
                overlay_epoch_ms: 10,
                batch_digest: "catalog-v2".to_string(),
                producer_head_applied_at_ms: 100,
                generation_materialized_at_ms: 20,
                changed_entries: Vec::new(),
                facet_entries: Vec::new(),
                snapshot_entries: Vec::new(),
                affected_entries: Vec::new(),
                invalidated_by_entries: Vec::new(),
            },
            std::time::Instant::now(),
        )
        .err()
        .ok_or("expected conflicting runtime catalog replay to fail")?;
    match err {
        CoreError::Typed { code, .. }
            if code == crate::readiness::errors::ERR_RUNTIME_CATALOG_CONFLICTING_BATCH => {}
        other @ (CoreError::Typed { .. }
        | CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => {
            return Err(format!("expected conflicting batch typed error, got {other:?}").into());
        }
    }
    Ok(())
}

#[test]
fn runtime_catalog_batch_rejects_unknown_doc_id() -> TestResult {
    let mut ledger = Ledger::default();
    install_chunk(&mut ledger, "src/lib.rs", "fn main() {}")?;
    let err = ledger
        .apply_runtime_catalog_batch(
            &RuntimeCatalogIngestBatch {
                repo_id: repo_id(),
                revision_id: revision_id(),
                generation: generation(),
                overlay_epoch_ms: 10,
                batch_digest: "catalog-v1".to_string(),
                producer_head_applied_at_ms: 100,
                generation_materialized_at_ms: 20,
                changed_entries: vec![RuntimeChangedRecord {
                    doc_id: ChunkId::new("missing-doc"),
                    applied_at_ms: 25,
                    payload_hash: [0xbb; 32],
                }],
                facet_entries: Vec::new(),
                snapshot_entries: Vec::new(),
                affected_entries: Vec::new(),
                invalidated_by_entries: Vec::new(),
            },
            std::time::Instant::now(),
        )
        .err()
        .ok_or("expected unknown runtime catalog doc id to fail")?;
    match err {
        CoreError::Typed { code, .. }
            if code == crate::readiness::errors::ERR_RUNTIME_CATALOG_UNKNOWN_DOC_ID => {}
        other @ (CoreError::Typed { .. }
        | CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => {
            return Err(format!("expected unknown doc typed error, got {other:?}").into());
        }
    }
    Ok(())
}
