//! Runtime-metadata authority state: dirty overlay and the runtime catalog
//! (changed docs, facets, snapshots, edge authorities) per generation.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use quanta_index_contract::{ChunkId, RuntimeCatalogIngestBatch};
use quanta_index_core::CoreError;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::readiness::errors::{
    ERR_RUNTIME_CATALOG_CONFLICTING_BATCH, ERR_RUNTIME_CATALOG_STALE_BATCH,
    ERR_RUNTIME_CATALOG_UNKNOWN_DOC_ID,
};
use crate::readiness::keys::AuthorityKey;
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

#[derive(Clone, Debug, Default)]
pub struct RuntimeMetadataState {
    pub(super) dirty_docs: BTreeMap<ChunkId, DirtyDocState>,
    pub(super) changed_docs: BTreeMap<ChunkId, ChangedDocState>,
    pub(super) doc_facets: BTreeMap<ChunkId, DocFacetState>,
    pub(super) snapshots: BTreeMap<Box<str>, BTreeSet<ChunkId>>,
    pub(super) affected_docs: BTreeMap<Box<str>, BTreeSet<ChunkId>>,
    pub(super) invalidated_by_docs: BTreeMap<Box<str>, BTreeSet<ChunkId>>,
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

    pub(crate) fn restore_dirty_doc(&mut self, chunk_id: ChunkId, doc: DirtyDocState) {
        let _previous = self.dirty_docs.insert(chunk_id, doc);
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
    pub fn dirty_docs(&self) -> &BTreeMap<ChunkId, DirtyDocState> {
        &self.dirty_docs
    }

    #[must_use]
    pub fn changed_docs(&self) -> &BTreeMap<ChunkId, ChangedDocState> {
        &self.changed_docs
    }

    #[must_use]
    pub fn doc_facets(&self) -> &BTreeMap<ChunkId, DocFacetState> {
        &self.doc_facets
    }

    #[must_use]
    pub fn snapshots(&self) -> &BTreeMap<Box<str>, BTreeSet<ChunkId>> {
        &self.snapshots
    }

    #[must_use]
    pub fn affected_docs(&self) -> &BTreeMap<Box<str>, BTreeSet<ChunkId>> {
        &self.affected_docs
    }

    #[must_use]
    pub fn invalidated_by_docs(&self) -> &BTreeMap<Box<str>, BTreeSet<ChunkId>> {
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

pub(super) fn runtime_catalog_typed(code: &str, message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: code.to_string(),
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

#[derive(Clone, Debug, Default)]
pub(super) struct RuntimeAuthoritySnapshot {
    pub(super) entries: BTreeMap<AuthorityKey, RuntimeMetadataState>,
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
    dirty_docs: BTreeMap<ChunkId, DirtyDocState>,
    changed_docs: BTreeMap<ChunkId, ChangedDocState>,
    doc_facets: BTreeMap<ChunkId, DocFacetState>,
    snapshots: BTreeMap<Box<str>, BTreeSet<ChunkId>>,
    affected_docs: BTreeMap<Box<str>, BTreeSet<ChunkId>>,
    invalidated_by_docs: BTreeMap<Box<str>, BTreeSet<ChunkId>>,
    catalog_overlay_epoch_ms: Option<u64>,
    catalog_batch_digest: Option<Box<str>>,
    producer_head_applied_at_ms: Option<u64>,
    generation_materialized_at_ms: Option<u64>,
    catalog_materialized: bool,
});

impl_struct_serde!(RuntimeAuthoritySnapshot {
    entries: BTreeMap<AuthorityKey, RuntimeMetadataState>,
});
