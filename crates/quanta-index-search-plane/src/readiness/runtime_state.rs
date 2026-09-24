//! Runtime-metadata authority state: dirty overlay and the runtime catalog
//! (changed docs, facets, snapshots, edge authorities) per generation.
//!
//! The record maps are persistent (structurally shared) so the ledger can
//! retain superseded epoch snapshots at the cost of the deltas alone
//! (QI-BB-020 W2); see `history_state`.

use std::collections::BTreeSet;
use std::fmt;

use imbl::OrdMap;
use quanta_index_contract::AuxEpochV1;
use quanta_index_contract::{ChunkId, RuntimeCatalogIngestBatch};
use quanta_index_core::{AuxiliaryGenerationKeyV1, CoreError};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::readiness::errors::{
    ERR_RUNTIME_CATALOG_CONFLICTING_BATCH, ERR_RUNTIME_CATALOG_STALE_BATCH,
    ERR_RUNTIME_CATALOG_UNKNOWN_DOC_ID,
};
use crate::readiness::serde_support::impl_struct_serde;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyDocState {
    pub(super) applied_at_ms: u64,
    pub(super) payload_hash: [u8; 32],
}

impl DirtyDocState {
    #[must_use]
    pub(crate) const fn new(applied_at_ms: u64, payload_hash: [u8; 32]) -> Self {
        Self {
            applied_at_ms,
            payload_hash,
        }
    }

    #[must_use]
    pub const fn applied_at_ms(&self) -> u64 {
        self.applied_at_ms
    }

    #[must_use]
    pub const fn payload_hash(&self) -> &[u8; 32] {
        &self.payload_hash
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChangedDocState {
    applied_at_ms: u64,
    payload_hash: [u8; 32],
}

impl ChangedDocState {
    #[must_use]
    pub(crate) const fn new(applied_at_ms: u64, payload_hash: [u8; 32]) -> Self {
        Self {
            applied_at_ms,
            payload_hash,
        }
    }

    #[must_use]
    pub const fn applied_at_ms(&self) -> u64 {
        self.applied_at_ms
    }

    #[must_use]
    pub const fn payload_hash(&self) -> &[u8; 32] {
        &self.payload_hash
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocFacetState {
    owner: Option<Box<str>>,
    service: Option<Box<str>>,
    layer: Option<Box<str>>,
    surface: Option<Box<str>>,
}

impl DocFacetState {
    #[must_use]
    pub(crate) const fn new(
        owner: Option<Box<str>>,
        service: Option<Box<str>>,
        layer: Option<Box<str>>,
        surface: Option<Box<str>>,
    ) -> Self {
        Self {
            owner,
            service,
            layer,
            surface,
        }
    }

    #[must_use]
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    #[must_use]
    pub fn service(&self) -> Option<&str> {
        self.service.as_deref()
    }

    #[must_use]
    pub fn layer(&self) -> Option<&str> {
        self.layer.as_deref()
    }

    #[must_use]
    pub fn surface(&self) -> Option<&str> {
        self.surface.as_deref()
    }
}

/// One generation's dirty overlay and runtime catalog.
///
/// The record maps are private: the dirty overlay changes one doc at a
/// time through the methods here and the catalog is replaced whole by a
/// validated delta; nothing else writes them.
#[derive(Clone, Debug, Default)]
pub struct RuntimeMetadataState {
    dirty_docs: OrdMap<ChunkId, DirtyDocState>,
    changed_docs: OrdMap<ChunkId, ChangedDocState>,
    doc_facets: OrdMap<ChunkId, DocFacetState>,
    snapshots: OrdMap<Box<str>, BTreeSet<ChunkId>>,
    affected_docs: OrdMap<Box<str>, BTreeSet<ChunkId>>,
    invalidated_by_docs: OrdMap<Box<str>, BTreeSet<ChunkId>>,
    catalog_overlay_epoch_ms: Option<u64>,
    catalog_batch_digest: Option<Box<str>>,
    producer_head_applied_at_ms: Option<u64>,
    generation_materialized_at_ms: Option<u64>,
    catalog_materialized: bool,
}

/// The part of a runtime generation's state that is not a record: the
/// catalog's epoch, digest, timestamps and materialization.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RuntimeStateMeta {
    pub(crate) catalog_overlay_epoch_ms: Option<u64>,
    pub(crate) catalog_batch_digest: Option<Box<str>>,
    pub(crate) producer_head_applied_at_ms: Option<u64>,
    pub(crate) generation_materialized_at_ms: Option<u64>,
    pub(crate) catalog_materialized: bool,
}

impl RuntimeMetadataState {
    /// The catalog meta.
    #[must_use]
    pub(crate) fn meta(&self) -> RuntimeStateMeta {
        RuntimeStateMeta {
            catalog_overlay_epoch_ms: self.catalog_overlay_epoch_ms,
            catalog_batch_digest: self.catalog_batch_digest.clone(),
            producer_head_applied_at_ms: self.producer_head_applied_at_ms,
            generation_materialized_at_ms: self.generation_materialized_at_ms,
            catalog_materialized: self.catalog_materialized,
        }
    }

    pub(crate) fn restore_meta(&mut self, meta: RuntimeStateMeta) {
        self.catalog_overlay_epoch_ms = meta.catalog_overlay_epoch_ms;
        self.catalog_batch_digest = meta.catalog_batch_digest;
        self.producer_head_applied_at_ms = meta.producer_head_applied_at_ms;
        self.generation_materialized_at_ms = meta.generation_materialized_at_ms;
        self.catalog_materialized = meta.catalog_materialized;
    }

    /// Apply a dirty-overlay delta: upserts then deletes, as the
    /// transition ordered them.
    pub(crate) fn apply_dirty_delta(&mut self, delta: &RuntimeDirtyDelta) {
        for (chunk_id, doc) in &delta.upserts {
            self.restore_dirty_doc(chunk_id.clone(), doc.clone());
        }
        for chunk_id in &delta.deletes {
            self.evict_dirty_doc(chunk_id);
        }
    }

    /// Apply a validated catalog delta: the generation's catalog is
    /// replaced whole, meta included.
    pub(crate) fn apply_catalog_delta(&mut self, delta: &RuntimeCatalogDelta) {
        self.restore_meta(delta.meta.clone());
        self.changed_docs = delta.changed_docs.clone();
        self.doc_facets = delta.doc_facets.clone();
        self.snapshots = delta.snapshots.clone();
        self.affected_docs = delta.affected_docs.clone();
        self.invalidated_by_docs = delta.invalidated_by_docs.clone();
    }

    pub(crate) fn restore_dirty_doc(&mut self, chunk_id: ChunkId, doc: DirtyDocState) {
        let _previous = self.dirty_docs.insert(chunk_id, doc);
    }

    /// Drop one doc from the dirty overlay (absent is fine).
    pub(crate) fn evict_dirty_doc(&mut self, chunk_id: &ChunkId) {
        let _removed = self.dirty_docs.remove(chunk_id);
    }

    pub(crate) fn restore_changed_doc(&mut self, chunk_id: ChunkId, doc: ChangedDocState) {
        let _previous = self.changed_docs.insert(chunk_id, doc);
    }

    pub(crate) fn restore_doc_facet(&mut self, chunk_id: ChunkId, facet: DocFacetState) {
        let _previous = self.doc_facets.insert(chunk_id, facet);
    }

    pub(crate) fn restore_snapshot(&mut self, name: &str, chunk_ids: BTreeSet<ChunkId>) {
        let _previous = self.snapshots.insert(name.into(), chunk_ids);
    }

    pub(crate) fn restore_affected_docs(&mut self, name: &str, chunk_ids: BTreeSet<ChunkId>) {
        let _previous = self.affected_docs.insert(name.into(), chunk_ids);
    }

    pub(crate) fn restore_invalidated_by_docs(&mut self, name: &str, chunk_ids: BTreeSet<ChunkId>) {
        let _previous = self.invalidated_by_docs.insert(name.into(), chunk_ids);
    }

    #[must_use]
    pub fn dirty_docs(&self) -> &OrdMap<ChunkId, DirtyDocState> {
        &self.dirty_docs
    }

    #[must_use]
    pub fn changed_docs(&self) -> &OrdMap<ChunkId, ChangedDocState> {
        &self.changed_docs
    }

    #[must_use]
    pub fn doc_facets(&self) -> &OrdMap<ChunkId, DocFacetState> {
        &self.doc_facets
    }

    #[must_use]
    pub fn snapshots(&self) -> &OrdMap<Box<str>, BTreeSet<ChunkId>> {
        &self.snapshots
    }

    #[must_use]
    pub fn affected_docs(&self) -> &OrdMap<Box<str>, BTreeSet<ChunkId>> {
        &self.affected_docs
    }

    #[must_use]
    pub fn invalidated_by_docs(&self) -> &OrdMap<Box<str>, BTreeSet<ChunkId>> {
        &self.invalidated_by_docs
    }

    #[must_use]
    pub const fn catalog_overlay_epoch_ms(&self) -> Option<u64> {
        self.catalog_overlay_epoch_ms
    }

    #[must_use]
    pub fn catalog_batch_digest(&self) -> Option<&str> {
        self.catalog_batch_digest.as_deref()
    }

    #[must_use]
    pub const fn producer_head_applied_at_ms(&self) -> Option<u64> {
        self.producer_head_applied_at_ms
    }

    #[must_use]
    pub const fn generation_materialized_at_ms(&self) -> Option<u64> {
        self.generation_materialized_at_ms
    }

    #[must_use]
    pub const fn catalog_materialized(&self) -> bool {
        self.catalog_materialized
    }
}

pub(super) fn runtime_catalog_typed(
    code: quanta_index_contract::SearchPlaneErrorCodeV2,
    message: impl Into<String>,
) -> CoreError {
    CoreError::Typed {
        code,
        message: message.into(),
    }
}

pub(crate) fn enforce_runtime_catalog_batch_order(
    state: &RuntimeMetadataState,
    batch: &RuntimeCatalogIngestBatch,
) -> Result<(), CoreError> {
    let Some(current_epoch) = state.catalog_overlay_epoch_ms() else {
        return Ok(());
    };
    if batch.overlay_epoch_ms < current_epoch {
        return Err(runtime_catalog_typed(
            ERR_RUNTIME_CATALOG_STALE_BATCH,
            format!(
                "runtime catalog ingest: stale overlay epoch {} is older than materialized epoch {}",
                batch.overlay_epoch_ms, current_epoch
            ),
        ));
    }
    if batch.overlay_epoch_ms == current_epoch
        && state.catalog_batch_digest() != Some(batch.batch_digest.as_str())
    {
        return Err(runtime_catalog_typed(
            ERR_RUNTIME_CATALOG_CONFLICTING_BATCH,
            format!(
                "runtime catalog ingest: conflicting batch digest `{}` for overlay epoch {}",
                batch.batch_digest, batch.overlay_epoch_ms
            ),
        ));
    }
    Ok(())
}

pub(crate) fn validate_runtime_catalog_doc_ids(
    batch: &RuntimeCatalogIngestBatch,
    chunk_universe: &BTreeSet<ChunkId>,
) -> Result<(), CoreError> {
    for doc_id in batch
        .changed_entries
        .iter()
        .map(|record| &record.doc_id)
        .chain(batch.facet_entries.iter().map(|record| &record.doc_id))
        .chain(
            batch
                .snapshot_entries
                .iter()
                .flat_map(|record| record.doc_ids.iter()),
        )
        .chain(
            batch
                .affected_entries
                .iter()
                .flat_map(|record| record.doc_ids.iter()),
        )
        .chain(
            batch
                .invalidated_by_entries
                .iter()
                .flat_map(|record| record.doc_ids.iter()),
        )
    {
        if !chunk_universe.contains(doc_id) {
            return Err(runtime_catalog_typed(
                ERR_RUNTIME_CATALOG_UNKNOWN_DOC_ID,
                format!(
                    "runtime catalog ingest: doc_id `{}` is not present in the pinned lexical chunk universe",
                    doc_id.as_str()
                ),
            ));
        }
    }
    Ok(())
}

impl_struct_serde!(RuntimeStateMeta {
    catalog_overlay_epoch_ms: Option<u64>,
    catalog_batch_digest: Option<Box<str>>,
    producer_head_applied_at_ms: Option<u64>,
    generation_materialized_at_ms: Option<u64>,
    catalog_materialized: bool,
});

impl_struct_serde!(DirtyDocState {
    applied_at_ms: u64,
    payload_hash: [u8; 32],
});

impl_struct_serde!(ChangedDocState {
    applied_at_ms: u64,
    payload_hash: [u8; 32],
});

impl_struct_serde!(DocFacetState {
    owner: Option<Box<str>>,
    service: Option<Box<str>>,
    layer: Option<Box<str>>,
    surface: Option<Box<str>>,
});

impl_struct_serde!(RuntimeMetadataState {
    dirty_docs: OrdMap<ChunkId, DirtyDocState>,
    changed_docs: OrdMap<ChunkId, ChangedDocState>,
    doc_facets: OrdMap<ChunkId, DocFacetState>,
    snapshots: OrdMap<Box<str>, BTreeSet<ChunkId>>,
    affected_docs: OrdMap<Box<str>, BTreeSet<ChunkId>>,
    invalidated_by_docs: OrdMap<Box<str>, BTreeSet<ChunkId>>,
    catalog_overlay_epoch_ms: Option<u64>,
    catalog_batch_digest: Option<Box<str>>,
    producer_head_applied_at_ms: Option<u64>,
    generation_materialized_at_ms: Option<u64>,
    catalog_materialized: bool,
});

/// What one dirty-overlay batch changes.
///
/// The generation's meta is carried unchanged so the generation exists in
/// the catalog even when the batch nets to zero dirty docs: a published
/// empty overlay is materialized, an absent one is not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeDirtyDelta {
    pub(crate) generation: AuxiliaryGenerationKeyV1,
    /// The epoch the snapshot after this delta has.
    pub(crate) epoch: AuxEpochV1,
    pub(crate) upserts: Vec<(ChunkId, DirtyDocState)>,
    pub(crate) deletes: Vec<ChunkId>,
    pub(crate) meta: RuntimeStateMeta,
}

/// What one runtime catalog batch changes: the whole catalog of the
/// generation, replaced, plus its meta.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeCatalogDelta {
    pub(crate) generation: AuxiliaryGenerationKeyV1,
    /// The epoch the snapshot after this delta has.
    pub(crate) epoch: AuxEpochV1,
    pub(crate) meta: RuntimeStateMeta,
    pub(crate) changed_docs: OrdMap<ChunkId, ChangedDocState>,
    pub(crate) doc_facets: OrdMap<ChunkId, DocFacetState>,
    pub(crate) snapshots: OrdMap<Box<str>, BTreeSet<ChunkId>>,
    pub(crate) affected_docs: OrdMap<Box<str>, BTreeSet<ChunkId>>,
    pub(crate) invalidated_by_docs: OrdMap<Box<str>, BTreeSet<ChunkId>>,
}
