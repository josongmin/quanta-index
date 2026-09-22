use std::fs;
use std::path::Path;

use quanta_index_contract::{
    ChunkId, RuntimeCatalogIngestBatch, RuntimeChangedRecord, RuntimeDocFacetRecord,
    RuntimeEdgeAuthorityRecord, RuntimeSnapshotRecord, SearchPlaneTrackKind,
};
use quanta_index_ipc::encode_cbor_payload;
use tempfile::tempdir;

use crate::auxiliary_authority;
use crate::readiness::auxiliary_store::{AuxiliaryAuthorityStore, restore_auxiliary_rows_into};
use crate::readiness::history_state::{
    HistoryAuthoritySnapshot, HistoryAuthorityState, HistoryStateMeta,
};
use crate::readiness::ledger::Ledger;
use crate::readiness::runtime_state::{RuntimeAuthoritySnapshot, RuntimeMetadataState};
use crate::readiness::structural_state::StructuralAuthoritySnapshot;
use crate::readiness::tests::support::{
    TestResult, generation, install_chunk, install_chunk_with_id, persist_whole_ledger, repo_id,
    revision_id, search_corpus_retention,
};

/// Every auxiliary family and the structural track round-trip through
/// the catalog rows: what the whole-state encoding writes, the boot
/// restore reads back equal (QI-BB-020).
#[test]
fn auxiliary_authorities_roundtrip_through_catalog_rows() -> TestResult {
    let store = auxiliary_authority::testing::MemoryAuxiliaryCatalog::default();
    let mut ledger = Ledger::default();

    ledger
        .aux_restore_mut::<HistoryAuthorityState>(&repo_id(), &revision_id(), generation())
        .restore_meta(HistoryStateMeta {
            commits_materialized: true,
            ..HistoryStateMeta::default()
        });
    ledger
        .aux_restore_mut::<RuntimeMetadataState>(&repo_id(), &revision_id(), generation())
        .restore_dirty_doc(
            ChunkId::new("dirty-1"),
            crate::readiness::runtime_state::DirtyDocState {
                applied_at_ms: 42,
                payload_hash: [7_u8; 32],
            },
        );
    install_chunk(&mut ledger, "src/lib.rs", "fn main() {}")?;
    ledger.request_structural_seal(
        &repo_id(),
        &revision_id(),
        generation(),
        std::time::Instant::now(),
    )?;
    ledger.record_track_materialized(
        &repo_id(),
        &revision_id(),
        SearchPlaneTrackKind::Structural,
        generation(),
        Some("digest-17"),
    );
    ledger.record_track_seal_with_digest(
        &repo_id(),
        &revision_id(),
        SearchPlaneTrackKind::Structural,
        generation(),
        "digest-17",
    );

    persist_whole_ledger(&store, &ledger)?;

    let mut restored = Ledger::default();
    let _rows = restore_auxiliary_rows_into(&mut restored, &store)?;

    if !restored
        .history_state(&repo_id(), &revision_id(), generation())
        .is_some_and(crate::readiness::history_state::HistoryAuthorityState::commits_materialized)
    {
        return Err("history authority did not restore commit materialization".into());
    }
    // The epoch row restores the epoch each domain was at (QI-BB-020 W2):
    // the structural authority advanced through the chunk install and the
    // seal request, the history one was filled in place.
    let now = std::time::Instant::now();
    let structural_epoch = |ledger: &Ledger| -> Result<
        quanta_index_contract::AuxEpochV1,
        Box<dyn std::error::Error>,
    > {
        Ok(ledger
            .structural_read_at(&repo_id(), &revision_id(), generation(), None, now)?
            .ok_or("structural authority exists")?
            .epoch)
    };
    if structural_epoch(&restored)? != structural_epoch(&ledger)?
        || structural_epoch(&ledger)? == quanta_index_contract::AuxEpochV1::GENESIS
    {
        return Err(format!(
            "the structural epoch must restore as written: {:?} vs {:?}",
            structural_epoch(&restored)?,
            structural_epoch(&ledger)?
        )
        .into());
    }
    if restored
        .runtime_state(&repo_id(), &revision_id(), generation())
        .and_then(|state| state.dirty_docs().get(&ChunkId::new("dirty-1")))
        .map(crate::readiness::runtime_state::DirtyDocState::applied_at_ms)
        != Some(42)
    {
        return Err("runtime authority did not restore dirty-doc payload".into());
    }
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
    ledger.apply_runtime_catalog_batch(
        &RuntimeCatalogIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            overlay_epoch_ms: 99,
            batch_digest: "catalog-roundtrip".to_string(),
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
    persist_whole_ledger(&store, &ledger)?;
    let mut restored_catalog = Ledger::default();
    let _rows = restore_auxiliary_rows_into(&mut restored_catalog, &store)?;
    let runtime = restored_catalog
        .runtime_state(&repo_id(), &revision_id(), generation())
        .ok_or("runtime catalog state missing after restore")?;
    if !runtime.catalog_materialized() {
        return Err("runtime catalog materialization flag did not restore".into());
    }
    if runtime.catalog_overlay_epoch_ms() != Some(99) {
        return Err("runtime catalog overlay epoch did not restore".into());
    }
    if runtime.catalog_batch_digest() != Some("catalog-roundtrip") {
        return Err("runtime catalog batch digest did not restore".into());
    }
    if runtime.generation_materialized_at_ms() != Some(20) {
        return Err("runtime catalog generation timestamp did not restore".into());
    }
    if runtime
        .changed_docs()
        .get(&ChunkId::new("changed-1"))
        .map(crate::readiness::runtime_state::ChangedDocState::applied_at_ms)
        != Some(25)
    {
        return Err("runtime catalog changed-doc payload did not restore".into());
    }
    if runtime
        .doc_facets()
        .get(&ChunkId::new("facet-1"))
        .and_then(crate::readiness::runtime_state::DocFacetState::owner)
        != Some("team-a")
    {
        return Err("runtime catalog facet payload did not restore".into());
    }
    if !runtime
        .snapshots()
        .get("active")
        .is_some_and(|docs| docs.contains(&ChunkId::new("snap-1")))
    {
        return Err("runtime catalog snapshot membership did not restore".into());
    }
    if !runtime
        .affected_docs()
        .get("rebuild=lexical")
        .is_some_and(|docs| docs.contains(&ChunkId::new("affected-1")))
    {
        return Err("runtime catalog affected edge payload did not restore".into());
    }
    if !runtime
        .invalidated_by_docs()
        .get("rebuild=lexical")
        .is_some_and(|docs| docs.contains(&ChunkId::new("invalidated-1")))
    {
        return Err("runtime catalog invalidated_by edge payload did not restore".into());
    }
    if restored
        .structural_state(&repo_id(), &revision_id(), generation())
        .map(|state| state.chunks().len())
        != Some(1)
    {
        return Err("structural authority did not restore chunk inventory".into());
    }
    if restored.track_sealed(&repo_id(), &revision_id(), SearchPlaneTrackKind::Structural)
        != Some(generation())
    {
        return Err("structural track seal did not restore".into());
    }
    Ok(())
}

/// The pre-catalog snapshot files move into the catalog once: every
/// record is restored from rows afterwards, the files are gone, and a
/// second open finds nothing to migrate.
#[test]
fn legacy_auxiliary_snapshots_migrate_into_the_catalog_once() -> TestResult {
    let dir = tempdir()?;
    let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    let mut ledger = Ledger::default();
    ledger
        .aux_restore_mut::<HistoryAuthorityState>(&repo_id(), &revision_id(), generation())
        .restore_meta(HistoryStateMeta {
            commits_materialized: true,
            ..HistoryStateMeta::default()
        });
    ledger
        .aux_restore_mut::<RuntimeMetadataState>(&repo_id(), &revision_id(), generation())
        .restore_dirty_doc(
            ChunkId::new("dirty-legacy"),
            crate::readiness::runtime_state::DirtyDocState {
                applied_at_ms: 7,
                payload_hash: [3_u8; 32],
            },
        );
    install_chunk(&mut ledger, "src/legacy.rs", "fn legacy() {}")?;
    ledger.request_structural_seal(
        &repo_id(),
        &revision_id(),
        generation(),
        std::time::Instant::now(),
    )?;
    ledger.record_track_seal_with_digest(
        &repo_id(),
        &revision_id(),
        SearchPlaneTrackKind::Structural,
        generation(),
        "digest-legacy",
    );
    // Write the three snapshot files exactly as the pre-catalog store did.
    let legacy = |path: &Path, bytes: Vec<u8>| -> TestResult {
        fs::write(path, bytes)?;
        Ok(())
    };
    legacy(
        &store.history,
        encode_cbor_payload(&HistoryAuthoritySnapshot {
            entries: ledger
                .history
                .iter()
                .map(|(key, registry)| (key.clone(), registry.current().clone()))
                .collect(),
        })?,
    )?;
    legacy(
        &store.runtime,
        encode_cbor_payload(&RuntimeAuthoritySnapshot {
            entries: ledger
                .runtime_metadata
                .iter()
                .map(|(key, registry)| (key.clone(), registry.current().clone()))
                .collect(),
        })?,
    )?;
    legacy(
        &store.structural,
        encode_cbor_payload(&StructuralAuthoritySnapshot {
            entries: ledger
                .structural
                .iter()
                .map(|(key, registry)| (key.clone(), registry.current().clone()))
                .collect(),
            tracks: ledger.search_tracks.clone(),
        })?,
    )?;

    let catalog = auxiliary_authority::testing::MemoryAuxiliaryCatalog::default();
    let receipt = store
        .migrate_legacy_auxiliary_snapshots(&catalog)?
        .ok_or("legacy files present, migration must run")?;
    if receipt.generations != 1 || receipt.rows_written == 0 {
        return Err(format!("migration receipt drifted: {receipt:?}").into());
    }
    for path in [&store.history, &store.runtime, &store.structural] {
        if path.exists() {
            return Err(format!("migrated snapshot {} must be removed", path.display()).into());
        }
    }
    if store
        .migrate_legacy_auxiliary_snapshots(&catalog)?
        .is_some()
    {
        return Err("a second open must find nothing to migrate".into());
    }
    let mut restored = Ledger::default();
    let _rows = restore_auxiliary_rows_into(&mut restored, &catalog)?;
    if !restored
        .history_state(&repo_id(), &revision_id(), generation())
        .is_some_and(crate::readiness::history_state::HistoryAuthorityState::commits_materialized)
    {
        return Err("migrated history flags did not restore".into());
    }
    if restored
        .runtime_state(&repo_id(), &revision_id(), generation())
        .and_then(|state| state.dirty_docs().get(&ChunkId::new("dirty-legacy")))
        .map(crate::readiness::runtime_state::DirtyDocState::applied_at_ms)
        != Some(7)
    {
        return Err("migrated dirty doc did not restore".into());
    }
    if restored
        .structural_state(&repo_id(), &revision_id(), generation())
        .map(|state| state.chunks().len())
        != Some(1)
    {
        return Err("migrated chunk universe did not restore".into());
    }
    if restored.track_sealed(&repo_id(), &revision_id(), SearchPlaneTrackKind::Structural)
        != Some(generation())
    {
        return Err("migrated structural track seal did not restore".into());
    }
    Ok(())
}
