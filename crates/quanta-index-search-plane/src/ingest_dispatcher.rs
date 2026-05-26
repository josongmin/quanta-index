//! Search-plane ingest orchestration (QI-RT-01).
//!
//! Producer sends a typed [`SearchPlaneIngestIpcRequest`] over UDS
//! `ingest.sock`. This dispatcher routes the typed batch to owner materializer
//! ports. The concrete runtime may choose to mirror accepted batches into
//! legacy channel persistence, but channel row-op fanout is no longer the
//! public ingest truth.
//!
//! Composition root in `quanta-index-searchd` is the only place that names
//! concrete adapter types (materializers, channel mirrors, repo-map ingest);
//! this module holds only [`Arc<dyn ...Port>`] (CLAUDE.md DIP rule).

use std::sync::{Arc, RwLock};

use quanta_index_channel::BundleChannelPublisher;
use quanta_index_contract::{
    BatchPublishReceipt, DeleteRef, DeleteTag, DirtyIngestBatch, EvictDirty, HistoryIngestBatch,
    LexicalChannelOp, LexicalIngestBatch, LexicalSeal, ReplaceLexicalScope, ReplaceSemanticScope,
    ReplaceStructuralScope, RepoMapMutationAck, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneIpcError, SearchPlaneTrackKind, SemanticChannelOp,
    SemanticIngestBatch, SemanticSeal, StructuralIngestBatch, TombstoneLexicalScope,
    TombstoneSemanticScope, TombstoneStructuralScope, UpsertCommit, UpsertDiffHunk, UpsertDirty,
    UpsertRef, UpsertTag,
};
use quanta_index_core::{
    CoreError, LexicalIndexBuildPort, LexicalIngestPort, RepoMapBundleIngestPort,
    SemanticIndexBuildPort, SemanticIngestPort,
};

use crate::Ledger;

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";

pub trait HistoryIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError>;
}

pub trait RuntimeMetadataIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError>;
}

pub trait StructuralIngestPort: Send + Sync {
    fn publish_batch(
        &self,
        batch: &StructuralIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

// =============================================================================
// Legacy channel mirror adapters
// =============================================================================

/// Adapter that implements [`LexicalIngestPort`] on top of a channel publisher.
///
/// Lifted out of the SDK so the SDK can drop its direct
/// `quanta-index-channel` dependency (QI-SDK-01); searchd's composition root
/// owns the publisher. In the may-26 integration packet this adapter is a
/// persistence mirror, not the readiness/materialization authority.
pub struct ChannelLexicalIngestAdapter {
    publisher: Arc<dyn BundleChannelPublisher<Op = LexicalChannelOp> + Send + Sync>,
}

impl ChannelLexicalIngestAdapter {
    #[must_use]
    pub fn new(
        publisher: Arc<dyn BundleChannelPublisher<Op = LexicalChannelOp> + Send + Sync>,
    ) -> Self {
        Self { publisher }
    }
}

impl LexicalIngestPort for ChannelLexicalIngestAdapter {
    fn publish_batch(&self, batch: &LexicalIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.manifest_digest.clone());
        for scope in &batch.replace_scopes {
            let payload = encode_cbor(&(batch.mode, batch.base_generation, scope.clone()))
                .map_err(|err| {
                    CoreError::InvalidContract(format!(
                        "lexical ingest: encode replace scope payload: {err}"
                    ))
                })?;
            let _published_seq = self
                .publisher
                .publish(LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                    payload,
                }))
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.accept_replace_scope();
        }
        for scope in &batch.tombstone_scopes {
            let payload = encode_cbor(&(batch.mode, batch.base_generation, scope.clone()))
                .map_err(|err| {
                    CoreError::InvalidContract(format!(
                        "lexical ingest: encode tombstone scope payload: {err}"
                    ))
                })?;
            let _published_seq = self
                .publisher
                .publish(LexicalChannelOp::TombstoneLexicalScope(
                    TombstoneLexicalScope {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        payload,
                    },
                ))
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.accept_tombstone_scope();
        }
        if batch.seal {
            let _published_seq = self
                .publisher
                .seal(
                    batch.repo_id.clone(),
                    batch.revision_id.clone(),
                    batch.generation,
                )
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.mark_sealed();
        }
        self.publisher
            .flush()
            .map_err(|err| channel_error_to_core(&err))?;
        Ok(receipt)
    }
}

/// Adapter that implements [`SemanticIngestPort`] on top of a channel
/// publisher as a legacy persistence mirror.
pub struct ChannelSemanticIngestAdapter {
    publisher: Arc<dyn BundleChannelPublisher<Op = SemanticChannelOp> + Send + Sync>,
}

impl ChannelSemanticIngestAdapter {
    #[must_use]
    pub fn new(
        publisher: Arc<dyn BundleChannelPublisher<Op = SemanticChannelOp> + Send + Sync>,
    ) -> Self {
        Self { publisher }
    }
}

impl SemanticIngestPort for ChannelSemanticIngestAdapter {
    fn publish_batch(&self, batch: &SemanticIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.manifest_digest.clone());
        for scope in &batch.replace_scopes {
            let payload = encode_cbor(&(
                batch.mode,
                batch.base_generation,
                batch.model_contract.clone(),
                scope.clone(),
            ))
            .map_err(|err| {
                CoreError::InvalidContract(format!(
                    "semantic ingest: encode replace scope payload: {err}"
                ))
            })?;
            let _published_seq = self
                .publisher
                .publish(SemanticChannelOp::ReplaceSemanticScope(
                    ReplaceSemanticScope {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        payload,
                    },
                ))
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.accept_replace_scope();
        }
        for scope in &batch.tombstone_scopes {
            let payload = encode_cbor(&(
                batch.mode,
                batch.base_generation,
                batch.model_contract.clone(),
                scope.clone(),
            ))
            .map_err(|err| {
                CoreError::InvalidContract(format!(
                    "semantic ingest: encode tombstone scope payload: {err}"
                ))
            })?;
            let _published_seq = self
                .publisher
                .publish(SemanticChannelOp::TombstoneSemanticScope(
                    TombstoneSemanticScope {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        payload,
                    },
                ))
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.accept_tombstone_scope();
        }
        if batch.seal {
            let _published_seq = self
                .publisher
                .seal(
                    batch.repo_id.clone(),
                    batch.revision_id.clone(),
                    batch.generation,
                )
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.mark_sealed();
        }
        self.publisher
            .flush()
            .map_err(|err| channel_error_to_core(&err))?;
        Ok(receipt)
    }
}

pub struct ChannelHistoryIngestAdapter {
    publisher: Arc<dyn BundleChannelPublisher<Op = LexicalChannelOp> + Send + Sync>,
}

impl ChannelHistoryIngestAdapter {
    #[must_use]
    pub fn new(
        publisher: Arc<dyn BundleChannelPublisher<Op = LexicalChannelOp> + Send + Sync>,
    ) -> Self {
        Self { publisher }
    }
}

impl HistoryIngestPort for ChannelHistoryIngestAdapter {
    fn publish_batch(&self, batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let mut receipt = BatchPublishReceipt::empty_for(batch.generation, String::new());
        for record in &batch.commits {
            let _published_seq = self
                .publisher
                .publish(LexicalChannelOp::UpsertCommit(UpsertCommit {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                    payload: encode_cbor(record).map_err(|err| {
                        CoreError::InvalidContract(format!(
                            "history ingest: encode commit record: {err}"
                        ))
                    })?,
                }))
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.accept_replace_scope();
        }
        for mutation in &batch.refs {
            let (op, is_replace_scope) = match mutation {
                quanta_index_contract::HistoryRefMutation::Upsert(payload) => (
                    LexicalChannelOp::UpsertRef(UpsertRef {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        name: payload.name.clone(),
                        sha: *payload.sha.as_bytes(),
                    }),
                    true,
                ),
                quanta_index_contract::HistoryRefMutation::Delete(payload) => (
                    LexicalChannelOp::DeleteRef(DeleteRef {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        name: payload.name.clone(),
                    }),
                    false,
                ),
            };
            let _published_seq = self
                .publisher
                .publish(op)
                .map_err(|err| channel_error_to_core(&err))?;
            if is_replace_scope {
                receipt.accept_replace_scope();
            } else {
                receipt.accept_tombstone_scope();
            }
        }
        for mutation in &batch.tags {
            let (op, is_replace_scope) = match mutation {
                quanta_index_contract::HistoryRefMutation::Upsert(payload) => (
                    LexicalChannelOp::UpsertTag(UpsertTag {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        name: payload.name.clone(),
                        sha: *payload.sha.as_bytes(),
                    }),
                    true,
                ),
                quanta_index_contract::HistoryRefMutation::Delete(payload) => (
                    LexicalChannelOp::DeleteTag(DeleteTag {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        name: payload.name.clone(),
                    }),
                    false,
                ),
            };
            let _published_seq = self
                .publisher
                .publish(op)
                .map_err(|err| channel_error_to_core(&err))?;
            if is_replace_scope {
                receipt.accept_replace_scope();
            } else {
                receipt.accept_tombstone_scope();
            }
        }
        for hunk in &batch.diff_hunks {
            let _published_seq = self
                .publisher
                .publish(LexicalChannelOp::UpsertDiffHunk(UpsertDiffHunk {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                    commit_sha: *hunk.commit_sha.as_bytes(),
                    file_path: hunk.file_path.clone(),
                    payload: encode_cbor(&hunk.record).map_err(|err| {
                        CoreError::InvalidContract(format!(
                            "history ingest: encode diff hunk record: {err}"
                        ))
                    })?,
                }))
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.accept_replace_scope();
        }
        self.publisher
            .flush()
            .map_err(|err| channel_error_to_core(&err))?;
        Ok(receipt)
    }
}

/// Adapter that implements [`RuntimeMetadataIngestPort`] on top of a channel
/// publisher as a legacy persistence mirror.
pub struct ChannelRuntimeMetadataIngestAdapter {
    publisher: Arc<dyn BundleChannelPublisher<Op = LexicalChannelOp> + Send + Sync>,
}

impl ChannelRuntimeMetadataIngestAdapter {
    #[must_use]
    pub fn new(
        publisher: Arc<dyn BundleChannelPublisher<Op = LexicalChannelOp> + Send + Sync>,
    ) -> Self {
        Self { publisher }
    }
}

impl RuntimeMetadataIngestPort for ChannelRuntimeMetadataIngestAdapter {
    fn publish_batch(&self, batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let mut receipt = BatchPublishReceipt::empty_for(batch.generation, String::new());
        for entry in &batch.entries {
            let (op, is_replace_scope) = match entry {
                quanta_index_contract::DirtyMutation::Upsert(record) => (
                    LexicalChannelOp::UpsertDirty(UpsertDirty {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        doc_id: record.doc_id.clone(),
                        applied_at_ms: record.applied_at_ms,
                        payload_hash: record.payload_hash,
                    }),
                    true,
                ),
                quanta_index_contract::DirtyMutation::Delete(payload) => (
                    LexicalChannelOp::EvictDirty(EvictDirty {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        doc_id: payload.doc_id.clone(),
                    }),
                    false,
                ),
            };
            let _published_seq = self
                .publisher
                .publish(op)
                .map_err(|err| channel_error_to_core(&err))?;
            if is_replace_scope {
                receipt.accept_replace_scope();
            } else {
                receipt.accept_tombstone_scope();
            }
        }
        self.publisher
            .flush()
            .map_err(|err| channel_error_to_core(&err))?;
        Ok(receipt)
    }
}

pub struct ChannelStructuralIngestAdapter {
    publisher: Arc<dyn BundleChannelPublisher<Op = LexicalChannelOp> + Send + Sync>,
}

impl ChannelStructuralIngestAdapter {
    #[must_use]
    pub fn new(
        publisher: Arc<dyn BundleChannelPublisher<Op = LexicalChannelOp> + Send + Sync>,
    ) -> Self {
        Self { publisher }
    }
}

impl StructuralIngestPort for ChannelStructuralIngestAdapter {
    fn publish_batch(
        &self,
        batch: &StructuralIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.manifest_digest.clone());
        for scope in &batch.replace_scopes {
            let payload = encode_cbor(&(batch.mode, batch.base_generation, scope.clone()))
                .map_err(|err| {
                    CoreError::InvalidContract(format!(
                        "structural ingest: encode replace scope payload: {err}"
                    ))
                })?;
            let _published_seq = self
                .publisher
                .publish(LexicalChannelOp::ReplaceStructuralScope(
                    ReplaceStructuralScope {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        payload,
                    },
                ))
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.accept_replace_scope();
        }
        for scope in &batch.tombstone_scopes {
            let payload = encode_cbor(&(batch.mode, batch.base_generation, scope.clone()))
                .map_err(|err| {
                    CoreError::InvalidContract(format!(
                        "structural ingest: encode tombstone scope payload: {err}"
                    ))
                })?;
            let _published_seq = self
                .publisher
                .publish(LexicalChannelOp::TombstoneStructuralScope(
                    TombstoneStructuralScope {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        payload,
                    },
                ))
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.accept_tombstone_scope();
        }
        if batch.seal {
            let _published_seq = self
                .publisher
                .seal(
                    batch.repo_id.clone(),
                    batch.revision_id.clone(),
                    batch.generation,
                )
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.mark_sealed();
        }
        self.publisher
            .flush()
            .map_err(|err| channel_error_to_core(&err))?;
        Ok(receipt)
    }
}

fn lexical_ops_for_batch(
    batch: &LexicalIngestBatch,
    include_seal: bool,
) -> Result<Vec<LexicalChannelOp>, CoreError> {
    let op_capacity = batch
        .replace_scopes
        .len()
        .saturating_add(batch.tombstone_scopes.len())
        .saturating_add(usize::from(include_seal));
    let mut ops = Vec::with_capacity(op_capacity);
    for scope in &batch.replace_scopes {
        ops.push(LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            payload: encode_cbor(&(batch.mode, batch.base_generation, scope.clone())).map_err(
                |err| {
                    CoreError::InvalidContract(format!(
                        "direct lexical materialize: encode replace scope payload: {err}"
                    ))
                },
            )?,
        }));
    }
    for scope in &batch.tombstone_scopes {
        ops.push(LexicalChannelOp::TombstoneLexicalScope(
            TombstoneLexicalScope {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                payload: encode_cbor(&(batch.mode, batch.base_generation, scope.clone())).map_err(
                    |err| {
                        CoreError::InvalidContract(format!(
                            "direct lexical materialize: encode tombstone scope payload: {err}"
                        ))
                    },
                )?,
            },
        ));
    }
    if include_seal {
        ops.push(LexicalChannelOp::Seal(LexicalSeal {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        }));
    }
    Ok(ops)
}

fn semantic_ops_for_batch(
    batch: &SemanticIngestBatch,
    include_seal: bool,
) -> Result<Vec<SemanticChannelOp>, CoreError> {
    let op_capacity = batch
        .replace_scopes
        .len()
        .saturating_add(batch.tombstone_scopes.len())
        .saturating_add(usize::from(include_seal));
    let mut ops = Vec::with_capacity(op_capacity);
    for scope in &batch.replace_scopes {
        ops.push(SemanticChannelOp::ReplaceSemanticScope(
            ReplaceSemanticScope {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                payload: encode_cbor(&(
                    batch.mode,
                    batch.base_generation,
                    batch.model_contract.clone(),
                    scope.clone(),
                ))
                .map_err(|err| {
                    CoreError::InvalidContract(format!(
                        "direct semantic materialize: encode replace scope payload: {err}"
                    ))
                })?,
            },
        ));
    }
    for scope in &batch.tombstone_scopes {
        ops.push(SemanticChannelOp::TombstoneSemanticScope(
            TombstoneSemanticScope {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                payload: encode_cbor(&(
                    batch.mode,
                    batch.base_generation,
                    batch.model_contract.clone(),
                    scope.clone(),
                ))
                .map_err(|err| {
                    CoreError::InvalidContract(format!(
                        "direct semantic materialize: encode tombstone scope payload: {err}"
                    ))
                })?,
            },
        ));
    }
    if include_seal {
        ops.push(SemanticChannelOp::Seal(SemanticSeal {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        }));
    }
    Ok(ops)
}

fn history_ops_for_batch(batch: &HistoryIngestBatch) -> Result<Vec<LexicalChannelOp>, CoreError> {
    let op_capacity = batch
        .commits
        .len()
        .saturating_add(batch.refs.len())
        .saturating_add(batch.tags.len())
        .saturating_add(batch.diff_hunks.len());
    let mut ops = Vec::with_capacity(op_capacity);
    for record in &batch.commits {
        ops.push(LexicalChannelOp::UpsertCommit(UpsertCommit {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            payload: encode_cbor(record).map_err(|err| {
                CoreError::InvalidContract(format!(
                    "direct history materialize: encode commit record: {err}"
                ))
            })?,
        }));
    }
    for mutation in &batch.refs {
        ops.push(match mutation {
            quanta_index_contract::HistoryRefMutation::Upsert(payload) => {
                LexicalChannelOp::UpsertRef(UpsertRef {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                    name: payload.name.clone(),
                    sha: *payload.sha.as_bytes(),
                })
            }
            quanta_index_contract::HistoryRefMutation::Delete(payload) => {
                LexicalChannelOp::DeleteRef(DeleteRef {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                    name: payload.name.clone(),
                })
            }
        });
    }
    for mutation in &batch.tags {
        ops.push(match mutation {
            quanta_index_contract::HistoryRefMutation::Upsert(payload) => {
                LexicalChannelOp::UpsertTag(UpsertTag {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                    name: payload.name.clone(),
                    sha: *payload.sha.as_bytes(),
                })
            }
            quanta_index_contract::HistoryRefMutation::Delete(payload) => {
                LexicalChannelOp::DeleteTag(DeleteTag {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                    name: payload.name.clone(),
                })
            }
        });
    }
    for hunk in &batch.diff_hunks {
        ops.push(LexicalChannelOp::UpsertDiffHunk(UpsertDiffHunk {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            commit_sha: *hunk.commit_sha.as_bytes(),
            file_path: hunk.file_path.clone(),
            payload: encode_cbor(&hunk.record).map_err(|err| {
                CoreError::InvalidContract(format!(
                    "direct history materialize: encode diff hunk record: {err}"
                ))
            })?,
        }));
    }
    Ok(ops)
}

fn dirty_ops_for_batch(batch: &DirtyIngestBatch) -> Vec<LexicalChannelOp> {
    batch
        .entries
        .iter()
        .map(|entry| match entry {
            quanta_index_contract::DirtyMutation::Upsert(record) => {
                LexicalChannelOp::UpsertDirty(UpsertDirty {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                    doc_id: record.doc_id.clone(),
                    applied_at_ms: record.applied_at_ms,
                    payload_hash: record.payload_hash,
                })
            }
            quanta_index_contract::DirtyMutation::Delete(payload) => {
                LexicalChannelOp::EvictDirty(EvictDirty {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                    doc_id: payload.doc_id.clone(),
                })
            }
        })
        .collect()
}

fn structural_ops_for_batch(
    batch: &StructuralIngestBatch,
) -> Result<Vec<LexicalChannelOp>, CoreError> {
    let op_capacity = batch
        .replace_scopes
        .len()
        .saturating_add(batch.tombstone_scopes.len());
    let mut ops = Vec::with_capacity(op_capacity);
    for scope in &batch.replace_scopes {
        ops.push(LexicalChannelOp::ReplaceStructuralScope(
            ReplaceStructuralScope {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                payload: encode_cbor(&(batch.mode, batch.base_generation, scope.clone())).map_err(
                    |err| {
                        CoreError::InvalidContract(format!(
                            "direct structural materialize: encode replace scope payload: {err}"
                        ))
                    },
                )?,
            },
        ));
    }
    for scope in &batch.tombstone_scopes {
        ops.push(LexicalChannelOp::TombstoneStructuralScope(
            TombstoneStructuralScope {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                payload: encode_cbor(&(batch.mode, batch.base_generation, scope.clone())).map_err(
                    |err| {
                        CoreError::InvalidContract(format!(
                            "direct structural materialize: encode tombstone scope payload: {err}"
                        ))
                    },
                )?,
            },
        ));
    }
    Ok(ops)
}

/// Direct lexical batch materializer that updates the builder + readiness
/// ledger immediately and keeps the supplied ingest port only as a durability
/// mirror.
pub struct DirectLexicalMaterializer {
    mirror: Arc<dyn LexicalIngestPort + Send + Sync>,
    builder: Arc<dyn LexicalIndexBuildPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectLexicalMaterializer {
    #[must_use]
    pub fn new(
        mirror: Arc<dyn LexicalIngestPort + Send + Sync>,
        builder: Arc<dyn LexicalIndexBuildPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self {
            mirror,
            builder,
            ledger,
        }
    }
}

impl LexicalIngestPort for DirectLexicalMaterializer {
    fn publish_batch(&self, batch: &LexicalIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let _mirror_receipt = self.mirror.publish_batch(batch)?;
        let ops = lexical_ops_for_batch(batch, batch.seal)?;
        self.builder
            .build(&batch.repo_id, &batch.revision_id, batch.generation, &ops)?;
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct lexical materialize: ledger poisoned: {err}"
            ))
        })?;
        for op in &ops {
            guard.apply_lexical_authority_op(op)?;
        }
        guard.lexical_materialize(batch.generation, Some(batch.manifest_digest.as_str()));
        guard.record_track_materialized(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Lexical,
            batch.generation,
            Some(batch.manifest_digest.as_str()),
        );
        if batch.seal {
            guard.lexical_seal_with_digest(batch.generation, batch.manifest_digest.as_str());
            guard.record_track_seal_with_digest(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Lexical,
                batch.generation,
                batch.manifest_digest.as_str(),
            );
        }
        drop(guard);
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.manifest_digest.clone());
        for _scope in &batch.replace_scopes {
            receipt.accept_replace_scope();
        }
        for _scope in &batch.tombstone_scopes {
            receipt.accept_tombstone_scope();
        }
        if batch.seal {
            receipt.mark_sealed();
        }
        Ok(receipt)
    }
}

/// Direct semantic batch materializer that updates the semantic builder + the
/// readiness ledger immediately and keeps the supplied ingest port only as a
/// durability mirror.
pub struct DirectSemanticMaterializer {
    mirror: Arc<dyn SemanticIngestPort + Send + Sync>,
    builder: Arc<dyn SemanticIndexBuildPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectSemanticMaterializer {
    #[must_use]
    pub fn new(
        mirror: Arc<dyn SemanticIngestPort + Send + Sync>,
        builder: Arc<dyn SemanticIndexBuildPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self {
            mirror,
            builder,
            ledger,
        }
    }
}

impl SemanticIngestPort for DirectSemanticMaterializer {
    fn publish_batch(&self, batch: &SemanticIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let _mirror_receipt = self.mirror.publish_batch(batch)?;
        let ops = semantic_ops_for_batch(batch, batch.seal)?;
        self.builder
            .build(&batch.repo_id, &batch.revision_id, batch.generation, &ops)?;
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct semantic materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.semantic_materialize(batch.generation, Some(batch.manifest_digest.as_str()));
        guard.record_track_materialized(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Semantic,
            batch.generation,
            Some(batch.manifest_digest.as_str()),
        );
        if batch.seal {
            guard.semantic_seal_with_digest(batch.generation, batch.manifest_digest.as_str());
            guard.record_track_seal_with_digest(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Semantic,
                batch.generation,
                batch.manifest_digest.as_str(),
            );
        }
        drop(guard);
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.manifest_digest.clone());
        for _scope in &batch.replace_scopes {
            receipt.accept_replace_scope();
        }
        for _scope in &batch.tombstone_scopes {
            receipt.accept_tombstone_scope();
        }
        if batch.seal {
            receipt.mark_sealed();
        }
        Ok(receipt)
    }
}

/// Direct history materializer. History is auxiliary and non-activation
/// blocking, but direct ledger updates keep query truth aligned with accepted
/// ingest batches.
pub struct DirectHistoryMaterializer {
    mirror: Arc<dyn HistoryIngestPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectHistoryMaterializer {
    #[must_use]
    pub fn new(
        mirror: Arc<dyn HistoryIngestPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self { mirror, ledger }
    }
}

impl HistoryIngestPort for DirectHistoryMaterializer {
    fn publish_batch(&self, batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let _mirror_receipt = self.mirror.publish_batch(batch)?;
        let ops = history_ops_for_batch(batch)?;
        let mut receipt = BatchPublishReceipt::empty_for(batch.generation, String::new());
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct history materialize: ledger poisoned: {err}"
            ))
        })?;
        for op in &ops {
            if matches!(
                op,
                LexicalChannelOp::DeleteRef(_) | LexicalChannelOp::DeleteTag(_)
            ) {
                receipt.accept_tombstone_scope();
            } else {
                receipt.accept_replace_scope();
            }
            guard.apply_lexical_authority_op(op)?;
        }
        drop(guard);
        Ok(receipt)
    }
}

/// Direct dirty-overlay materializer. Dirty state remains auxiliary and
/// non-activation-blocking.
pub struct DirectRuntimeMetadataMaterializer {
    mirror: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectRuntimeMetadataMaterializer {
    #[must_use]
    pub fn new(
        mirror: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self { mirror, ledger }
    }
}

impl RuntimeMetadataIngestPort for DirectRuntimeMetadataMaterializer {
    fn publish_batch(&self, batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let _mirror_receipt = self.mirror.publish_batch(batch)?;
        let ops = dirty_ops_for_batch(batch);
        let mut receipt = BatchPublishReceipt::empty_for(batch.generation, String::new());
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!("direct dirty materialize: ledger poisoned: {err}"))
        })?;
        for op in &ops {
            if matches!(op, LexicalChannelOp::EvictDirty(_)) {
                receipt.accept_tombstone_scope();
            } else {
                receipt.accept_replace_scope();
            }
            guard.apply_lexical_authority_op(op)?;
        }
        drop(guard);
        Ok(receipt)
    }
}

/// Direct structural materializer. Structural readiness is first-class and no
/// longer inferred from lexical seal replay; the mirrored lexical channel path
/// is kept only for restart-time authority rebuild.
pub struct DirectStructuralMaterializer {
    mirror: Arc<dyn StructuralIngestPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectStructuralMaterializer {
    #[must_use]
    pub fn new(
        mirror: Arc<dyn StructuralIngestPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self { mirror, ledger }
    }
}

impl StructuralIngestPort for DirectStructuralMaterializer {
    fn publish_batch(
        &self,
        batch: &StructuralIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        let mut mirrored_batch = batch.clone();
        mirrored_batch.seal = false;
        let _mirror_receipt = self.mirror.publish_batch(&mirrored_batch)?;
        let ops = structural_ops_for_batch(batch)?;
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct structural materialize: ledger poisoned: {err}"
            ))
        })?;
        for op in &ops {
            guard.apply_lexical_authority_op(op)?;
        }
        guard.record_track_materialized(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Structural,
            batch.generation,
            Some(batch.manifest_digest.as_str()),
        );
        let has_parse_trees = guard
            .structural_state(&batch.repo_id, &batch.revision_id, batch.generation)
            .is_some_and(|state| !state.parse_trees().is_empty());
        if batch.seal && has_parse_trees {
            guard.request_structural_seal(&batch.repo_id, &batch.revision_id, batch.generation);
            guard.record_track_seal_with_digest(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Structural,
                batch.generation,
                batch.manifest_digest.as_str(),
            );
        }
        drop(guard);
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.manifest_digest.clone());
        for _scope in &batch.replace_scopes {
            receipt.accept_replace_scope();
        }
        for _scope in &batch.tombstone_scopes {
            receipt.accept_tombstone_scope();
        }
        if batch.seal {
            receipt.mark_sealed();
        }
        Ok(receipt)
    }
}

// =============================================================================
// Top-level dispatcher
// =============================================================================

/// Routes typed ingest requests to the appropriate domain port. Mirrors the
/// shape of [`crate::SearchPlaneControlDispatcher`] / [`crate::SearchPlaneDispatcher`]
/// for the new ingest surface (QI-RT-01).
pub struct SearchPlaneIngestDispatcher {
    lexical: Arc<dyn LexicalIngestPort + Send + Sync>,
    semantic: Arc<dyn SemanticIngestPort + Send + Sync>,
    history: Arc<dyn HistoryIngestPort + Send + Sync>,
    runtime: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
    structural: Arc<dyn StructuralIngestPort + Send + Sync>,
    repomap: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
}

impl SearchPlaneIngestDispatcher {
    #[must_use]
    pub fn new(
        lexical: Arc<dyn LexicalIngestPort + Send + Sync>,
        semantic: Arc<dyn SemanticIngestPort + Send + Sync>,
        history: Arc<dyn HistoryIngestPort + Send + Sync>,
        runtime: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
        structural: Arc<dyn StructuralIngestPort + Send + Sync>,
        repomap: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    ) -> Self {
        Self {
            lexical,
            semantic,
            history,
            runtime,
            structural,
            repomap,
        }
    }

    #[must_use]
    pub fn dispatch(&self, request: SearchPlaneIngestIpcRequest) -> SearchPlaneIngestIpcResponse {
        match request {
            SearchPlaneIngestIpcRequest::PublishLexicalBatch(batch) => {
                match self.lexical.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::LexicalReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishSemanticBatch(batch) => {
                match self.semantic.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::SemanticReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) => {
                match self.history.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::HistoryReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch) => {
                match self.runtime.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::DirtyReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(batch) => {
                match self.structural.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::StructuralReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoMapBundle(bundle) => {
                match self.repomap.ingest_bundle(&bundle) {
                    Ok(()) => SearchPlaneIngestIpcResponse::RepoMapReceipt(RepoMapMutationAck {
                        repo_id: bundle.repo_id,
                        revision_id: bundle.revision_id,
                        manifest_generation: bundle.manifest_generation,
                    }),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            } // QI-LXB-01 / QI-HIST-01 / QI-RT-02 / QI-STR-02: history /
              // dirty / structural batches now have first-class arms above.
              // No fallback arm needed.
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn encode_cbor<T: serde::Serialize>(
    value: &T,
) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>> {
    let mut buf: Vec<u8> = Vec::new();
    ciborium::into_writer(value, &mut buf)?;
    Ok(buf)
}

fn channel_error_to_core(err: &quanta_index_channel::ChannelError) -> CoreError {
    // Channel errors carry vendor-shaped context; map them to a typed
    // domain error category so the IPC error message does not leak
    // backend-specific terms (CLAUDE.md "No vendor / transport tokens
    // outside their owning adapter").
    CoreError::Storage(format!("ingest channel: {err}"))
}

fn core_error_to_ipc(err: CoreError) -> SearchPlaneIpcError {
    let (code, message) = match err {
        CoreError::InvalidContract(msg) => (ERR_INVALID.to_string(), msg),
        CoreError::Typed { code, message } => (code, message),
        CoreError::NotReady(msg) => (ERR_NOT_READY.to_string(), msg),
        CoreError::NotImplemented(msg) => (ERR_NOT_IMPLEMENTED.to_string(), msg),
        CoreError::NotFound(msg) => (ERR_NOT_FOUND.to_string(), msg),
        CoreError::Storage(msg) => (ERR_INTERNAL.to_string(), msg),
    };
    SearchPlaneIpcError { code, message }
}

// =============================================================================
// Tests — in-memory fake publisher exercises the fanout shape
// =============================================================================

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use quanta_index_channel::ChannelError;
    use quanta_index_contract::lex::{
        CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, LanguageCode, ParseNode,
        ParseRoleTag, ParseTreeRecord, compute_parse_tree_source_hash,
    };
    use quanta_index_contract::{
        BatchIngestMode, ChannelSeq, ChunkId, ChunkRecord, DiffHunkSide, EmbeddingDistanceMetric,
        EmbeddingId, EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord,
        HistoryDiffHunkUpsert, HistoryIngestBatch, HistoryRefMutation, HistoryRefUpsert,
        LexicalIngestBatch, LexicalReplaceScope, LexicalTombstoneScope, ManifestGeneration,
        OwnerDocKind, RepoId, RepoRelativePath, RevisionId, SearchScopeKey, SearchScopeSurface,
        SemanticIngestBatch, SemanticReplaceScope, StructuralIngestBatch, StructuralReplaceScope,
        StructuralTreeRecord,
    };

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    fn test_failure(message: impl Into<String>) -> Box<dyn std::error::Error> {
        Box::new(std::io::Error::other(message.into()))
    }

    fn ensure(condition: bool, message: impl Into<String>) -> TestRes {
        if condition {
            Ok(())
        } else {
            Err(test_failure(message))
        }
    }

    fn next_channel_seq(next_seq: &Mutex<u64>) -> Result<ChannelSeq, ChannelError> {
        let current = {
            let mut next = next_seq
                .lock()
                .map_err(|err| ChannelError::State(format!("seq mutex poisoned: {err}")))?;
            let current = *next;
            *next = current.saturating_add(1);
            current
        };
        Ok(ChannelSeq::new(current))
    }

    fn take_ops<T>(ops: &Mutex<Vec<T>>) -> Result<Vec<T>, ChannelError> {
        let taken = {
            let mut guard = ops
                .lock()
                .map_err(|err| ChannelError::State(format!("ops mutex poisoned: {err}")))?;
            std::mem::take(&mut *guard)
        };
        Ok(taken)
    }

    fn push_op<T>(ops: &Mutex<Vec<T>>, op: T) -> Result<(), ChannelError> {
        {
            let mut guard = ops
                .lock()
                .map_err(|err| ChannelError::State(format!("ops mutex poisoned: {err}")))?;
            guard.push(op);
        }
        Ok(())
    }

    struct FakeLexicalPublisher {
        next_seq: Mutex<u64>,
        ops: Mutex<Vec<LexicalChannelOp>>,
    }

    impl FakeLexicalPublisher {
        fn new() -> Self {
            Self {
                next_seq: Mutex::new(0),
                ops: Mutex::new(Vec::new()),
            }
        }

        fn take(&self) -> Result<Vec<LexicalChannelOp>, ChannelError> {
            take_ops(&self.ops)
        }
    }

    impl BundleChannelPublisher for FakeLexicalPublisher {
        type Op = LexicalChannelOp;

        fn publish(&self, op: Self::Op) -> Result<ChannelSeq, ChannelError> {
            let seq = next_channel_seq(&self.next_seq)?;
            push_op(&self.ops, op)?;
            Ok(seq)
        }

        fn seal(
            &self,
            repo: RepoId,
            revision: RevisionId,
            generation: ManifestGeneration,
        ) -> Result<ChannelSeq, ChannelError> {
            let seq = next_channel_seq(&self.next_seq)?;
            push_op(
                &self.ops,
                LexicalChannelOp::Seal(quanta_index_contract::LexicalSeal {
                    repo_id: repo,
                    revision_id: revision,
                    generation,
                }),
            )?;
            Ok(seq)
        }

        fn flush(&self) -> Result<(), ChannelError> {
            Ok(())
        }
    }

    struct FakeSemanticPublisher {
        next_seq: Mutex<u64>,
        ops: Mutex<Vec<SemanticChannelOp>>,
    }

    impl FakeSemanticPublisher {
        fn new() -> Self {
            Self {
                next_seq: Mutex::new(0),
                ops: Mutex::new(Vec::new()),
            }
        }

        fn take(&self) -> Result<Vec<SemanticChannelOp>, ChannelError> {
            take_ops(&self.ops)
        }
    }

    impl BundleChannelPublisher for FakeSemanticPublisher {
        type Op = SemanticChannelOp;

        fn publish(&self, op: Self::Op) -> Result<ChannelSeq, ChannelError> {
            let seq = next_channel_seq(&self.next_seq)?;
            push_op(&self.ops, op)?;
            Ok(seq)
        }

        fn seal(
            &self,
            repo: RepoId,
            revision: RevisionId,
            generation: ManifestGeneration,
        ) -> Result<ChannelSeq, ChannelError> {
            let seq = next_channel_seq(&self.next_seq)?;
            push_op(
                &self.ops,
                SemanticChannelOp::Seal(quanta_index_contract::SemanticSeal {
                    repo_id: repo,
                    revision_id: revision,
                    generation,
                }),
            )?;
            Ok(seq)
        }

        fn flush(&self) -> Result<(), ChannelError> {
            Ok(())
        }
    }

    fn fixture_chunk_record() -> Result<ChunkRecord, Box<dyn std::error::Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new("chunk-1"),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: rust_language()?,
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 1,
            snippet: "fn main() {}".to_string().into_boxed_str(),
            indexed_text: "fn main() {}".to_string().into_boxed_str(),
            text_digest: "text:abc".to_string().into_boxed_str(),
            shape_digest: "shape:def".to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
        })
    }

    fn fixture_embedding_record() -> Result<EmbeddingRecord, Box<dyn std::error::Error>> {
        Ok(EmbeddingRecord {
            embedding_id: EmbeddingId::new("emb-1"),
            owner_kind: OwnerDocKind::Chunk,
            owner_id: "main".to_string().into_boxed_str(),
            source_doc_id: "chunk-1".to_string().into_boxed_str(),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: rust_language()?,
            symbol_kind: None,
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 1,
            snippet: "fn main() {}".to_string().into_boxed_str(),
            embedding_input_digest: "input:abc".to_string().into_boxed_str(),
            vector_digest: "vec:def".to_string().into_boxed_str(),
            view_kind: "raw_chunk".to_string().into_boxed_str(),
            vector: vec![0.1, 0.2, 0.3],
        })
    }

    fn fixture_commit_sha() -> CommitSha {
        CommitSha::from_bytes([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef, 0x01, 0x23, 0x45, 0x67,
        ])
    }

    fn fixture_commit_record() -> CommitRecord {
        CommitRecord {
            wire_version: 1,
            sha: fixture_commit_sha(),
            parents: Vec::new(),
            author_time_ms: 11,
            committer_time_ms: 12,
            applied_at_ms: 13,
            author: "alice".to_string().into_boxed_str(),
            committer: "alice".to_string().into_boxed_str(),
            message: "fix: sample".to_string().into_boxed_str(),
            is_merge: false,
            tags: vec!["v1.0.0".to_string().into_boxed_str()],
        }
    }

    fn fixture_diff_record() -> DiffHunkRecord {
        DiffHunkRecord {
            wire_version: 1,
            hunk_header: "@@ -1,1 +1,2 @@".to_string().into_boxed_str(),
            side: DiffHunkSide::After,
            added_text: "todo!".to_string().into_boxed_str(),
            removed_text: String::new().into_boxed_str(),
            touched_text: "todo!".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: 5,
        }
    }

    fn fixture_dirty_record() -> DirtyRecord {
        DirtyRecord {
            wire_version: 1,
            doc_id: ChunkId::new("chunk-dirty"),
            applied_at_ms: 55,
            payload_hash: [7; 32],
        }
    }

    fn fixture_parse_tree_record() -> Result<ParseTreeRecord, Box<dyn std::error::Error>> {
        Ok(ParseTreeRecord {
            wire_version: 1,
            lang: rust_language()?,
            root: ParseNode {
                kind: "function_item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 10,
                children: Vec::new(),
            },
            source_hash: compute_parse_tree_source_hash("fn main() {}"),
            role_tag_schema_version: 1,
            role_tags: vec![ParseRoleTag {
                role: "expr".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 4,
            }],
        })
    }

    fn rust_language() -> Result<LanguageCode, Box<dyn std::error::Error>> {
        LanguageCode::new("rust")
            .map_err(|err| test_failure(format!("invalid hard-coded test language code: {err}")))
    }

    fn fixture_scope() -> SearchScopeKey {
        SearchScopeKey {
            doc_surface: SearchScopeSurface::Chunk,
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
        }
    }

    fn fixture_model_contract() -> EmbeddingModelContract {
        EmbeddingModelContract {
            model_id: "test-model".to_string().into_boxed_str(),
            model_version: None,
            dimension: 3,
            normalization: EmbeddingNormalization::None,
            distance_metric: EmbeddingDistanceMetric::Cosine,
            policy_digest: "policy:abc".to_string().into_boxed_str(),
            view_policy_digest: None,
        }
    }

    #[test]
    fn lexical_adapter_fans_out_replace_generation_with_chunks_and_seal() -> TestRes {
        let publisher = Arc::new(FakeLexicalPublisher::new());
        let adapter = ChannelLexicalIngestAdapter::new(publisher.clone());

        let batch = LexicalIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
            base_generation: None,
            manifest_digest: "manifest:lex".to_string(),
            batch_digest: "batch:lex".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            replace_scopes: vec![LexicalReplaceScope {
                scope: fixture_scope(),
                scope_digest: "scope:lex".to_string(),
                chunks: vec![fixture_chunk_record()?],
                symbols: vec![],
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        };
        let receipt = adapter.publish_batch(&batch)?;
        let ops = publisher.take()?;
        ensure(
            matches!(
                ops.as_slice(),
                [
                    LexicalChannelOp::ReplaceLexicalScope(_),
                    LexicalChannelOp::Seal(_),
                ]
            ),
            "unexpected lexical op sequence for replace-generation batch",
        )?;
        ensure(
            receipt.generation == ManifestGeneration::new(1),
            "unexpected lexical receipt generation",
        )?;
        ensure(
            receipt.manifest_digest == "manifest:lex",
            "unexpected lexical receipt manifest_digest",
        )?;
        ensure(
            receipt.accepted_replace_scopes == 1 && receipt.accepted_tombstone_scopes == 0,
            "unexpected lexical receipt scope counts",
        )?;
        ensure(receipt.sealed, "expected lexical receipt to be sealed")?;
        Ok(())
    }

    #[test]
    fn lexical_adapter_skips_full_bundle_in_delta_mode_and_no_seal_when_disabled() -> TestRes {
        let publisher = Arc::new(FakeLexicalPublisher::new());
        let adapter = ChannelLexicalIngestAdapter::new(publisher.clone());

        let batch = LexicalIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
            base_generation: Some(ManifestGeneration::new(0)),
            manifest_digest: "manifest:lex-delta".to_string(),
            batch_digest: "batch:lex-delta".to_string(),
            mode: BatchIngestMode::Delta,
            replace_scopes: vec![LexicalReplaceScope {
                scope: fixture_scope(),
                scope_digest: "scope:lex-delta".to_string(),
                chunks: vec![fixture_chunk_record()?],
                symbols: vec![],
            }],
            tombstone_scopes: Vec::new(),
            seal: false,
        };
        let receipt = adapter.publish_batch(&batch)?;
        let ops = publisher.take()?;
        ensure(
            matches!(ops.as_slice(), [LexicalChannelOp::ReplaceLexicalScope(_)]),
            "unexpected lexical op sequence for delta batch",
        )?;
        ensure(
            !receipt.sealed,
            "expected lexical receipt to remain unsealed",
        )?;
        ensure(
            receipt.accepted_replace_scopes == 1 && receipt.accepted_tombstone_scopes == 0,
            "unexpected lexical delta receipt scope counts",
        )?;
        Ok(())
    }

    #[test]
    fn lexical_adapter_rejects_tombstone_scopes_before_publish() -> TestRes {
        let publisher = Arc::new(FakeLexicalPublisher::new());
        let adapter = ChannelLexicalIngestAdapter::new(publisher.clone());

        let batch = LexicalIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
            base_generation: Some(ManifestGeneration::new(0)),
            manifest_digest: "manifest:lex-tombstone".to_string(),
            batch_digest: "batch:lex-tombstone".to_string(),
            mode: BatchIngestMode::Delta,
            replace_scopes: Vec::new(),
            tombstone_scopes: vec![LexicalTombstoneScope {
                scope: fixture_scope(),
            }],
            seal: false,
        };

        let receipt = adapter.publish_batch(&batch)?;
        ensure(
            receipt.accepted_replace_scopes == 0 && receipt.accepted_tombstone_scopes == 1,
            "unexpected lexical tombstone receipt counts",
        )?;
        ensure(
            matches!(
                publisher.take()?.as_slice(),
                [LexicalChannelOp::TombstoneLexicalScope(_)]
            ),
            "unexpected lexical tombstone op sequence",
        )?;
        Ok(())
    }

    #[test]
    fn semantic_adapter_fans_out_embeddings_with_seal() -> TestRes {
        let publisher = Arc::new(FakeSemanticPublisher::new());
        let adapter = ChannelSemanticIngestAdapter::new(publisher.clone());

        let batch = SemanticIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
            base_generation: None,
            manifest_digest: "manifest:sem".to_string(),
            batch_digest: "batch:sem".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: fixture_model_contract(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: fixture_scope(),
                scope_digest: "scope:sem".to_string(),
                embeddings: vec![fixture_embedding_record()?],
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        };
        let receipt = adapter.publish_batch(&batch)?;
        let ops = publisher.take()?;
        ensure(
            matches!(
                ops.as_slice(),
                [
                    SemanticChannelOp::ReplaceSemanticScope(_),
                    SemanticChannelOp::Seal(_),
                ]
            ),
            "unexpected semantic op sequence for replace-generation batch",
        )?;
        ensure(
            receipt.generation == ManifestGeneration::new(1)
                && receipt.accepted_replace_scopes == 1
                && receipt.accepted_tombstone_scopes == 0,
            "unexpected semantic receipt scope counts",
        )?;
        ensure(receipt.sealed, "expected semantic receipt to be sealed")?;
        Ok(())
    }

    #[test]
    fn history_adapter_fans_out_commit_ref_tag_and_diff_hunk_ops() -> TestRes {
        let publisher = Arc::new(FakeLexicalPublisher::new());
        let adapter = ChannelHistoryIngestAdapter::new(publisher.clone());

        let batch = HistoryIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
            manifest_digest: Some("manifest:hist".to_string()),
            batch_digest: "batch:hist".to_string(),
            commits: vec![fixture_commit_record()],
            refs: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: "refs/heads/main".to_string().into_boxed_str(),
                sha: fixture_commit_sha(),
            })],
            tags: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: "v1.0.0".to_string().into_boxed_str(),
                sha: fixture_commit_sha(),
            })],
            diff_hunks: vec![HistoryDiffHunkUpsert {
                commit_sha: fixture_commit_sha(),
                file_path: "src/lib.rs".to_string().into_boxed_str(),
                record: fixture_diff_record(),
            }],
        };
        let receipt = adapter.publish_batch(&batch)?;
        let ops = publisher.take()?;
        ensure(
            matches!(
                ops.as_slice(),
                [
                    LexicalChannelOp::UpsertCommit(_),
                    LexicalChannelOp::UpsertRef(_),
                    LexicalChannelOp::UpsertTag(_),
                    LexicalChannelOp::UpsertDiffHunk(_),
                ]
            ),
            "unexpected history op sequence",
        )?;
        let Some(LexicalChannelOp::UpsertCommit(commit)) = ops.first() else {
            return Err(test_failure("expected UpsertCommit"));
        };
        let decoded_commit: CommitRecord = ciborium::from_reader(commit.payload.as_slice())?;
        ensure(
            decoded_commit.author_time_ms == 11,
            "history commit payload lost author_time_ms",
        )?;
        let Some(LexicalChannelOp::UpsertDiffHunk(diff)) = ops.get(3) else {
            return Err(test_failure("expected UpsertDiffHunk"));
        };
        let decoded_diff: DiffHunkRecord = ciborium::from_reader(diff.payload.as_slice())?;
        ensure(
            decoded_diff.hunk_header.as_ref() == "@@ -1,1 +1,2 @@",
            "history diff payload lost hunk header",
        )?;
        ensure(
            receipt.generation == ManifestGeneration::new(1)
                && receipt.accepted_replace_scopes == 4
                && receipt.accepted_tombstone_scopes == 0,
            "unexpected history receipt scope counts",
        )?;
        Ok(())
    }

    #[test]
    fn runtime_metadata_adapter_fans_out_dirty_upsert_and_evict_ops() -> TestRes {
        let publisher = Arc::new(FakeLexicalPublisher::new());
        let adapter = ChannelRuntimeMetadataIngestAdapter::new(publisher.clone());

        let batch = DirtyIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
            overlay_epoch_ms: 1_717_171_717_000,
            batch_digest: "batch:dirty".to_string(),
            entries: vec![
                quanta_index_contract::DirtyMutation::Upsert(fixture_dirty_record()),
                quanta_index_contract::DirtyMutation::Delete(quanta_index_contract::DirtyDelete {
                    doc_id: ChunkId::new("chunk-evict"),
                }),
            ],
        };
        let receipt = adapter.publish_batch(&batch)?;
        let ops = publisher.take()?;
        ensure(
            matches!(
                ops.as_slice(),
                [
                    LexicalChannelOp::UpsertDirty(_),
                    LexicalChannelOp::EvictDirty(_),
                ]
            ),
            "unexpected dirty op sequence",
        )?;
        let Some(LexicalChannelOp::UpsertDirty(entry)) = ops.first() else {
            return Err(test_failure("expected UpsertDirty"));
        };
        ensure(
            entry.doc_id == ChunkId::new("chunk-dirty")
                && entry.applied_at_ms == 55
                && entry.payload_hash == [7; 32],
            "dirty upsert op lost inline authority fields",
        )?;
        ensure(
            receipt.generation == ManifestGeneration::new(1)
                && receipt.accepted_replace_scopes == 1
                && receipt.accepted_tombstone_scopes == 1,
            "unexpected dirty receipt scope counts",
        )?;
        Ok(())
    }

    #[test]
    fn structural_adapter_fans_out_parse_tree_upsert_and_seal_ops() -> TestRes {
        let publisher = Arc::new(FakeLexicalPublisher::new());
        let adapter = ChannelStructuralIngestAdapter::new(publisher.clone());

        let batch = StructuralIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
            base_generation: None,
            manifest_digest: "manifest:str".to_string(),
            batch_digest: "batch:str".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            replace_scopes: vec![StructuralReplaceScope {
                scope: fixture_scope(),
                scope_digest: "scope:str".to_string(),
                trees: vec![StructuralTreeRecord {
                    chunk_id: ChunkId::new("chunk-tree"),
                    record: fixture_parse_tree_record()?,
                }],
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        };
        let receipt = adapter.publish_batch(&batch)?;
        let ops = publisher.take()?;
        ensure(
            matches!(
                ops.as_slice(),
                [
                    LexicalChannelOp::ReplaceStructuralScope(_),
                    LexicalChannelOp::Seal(_),
                ]
            ),
            "unexpected structural op sequence",
        )?;
        let Some(LexicalChannelOp::ReplaceStructuralScope(scope_op)) = ops.first() else {
            return Err(test_failure("expected ReplaceStructuralScope"));
        };
        let (_mode, _base_generation, decoded_scope): (
            BatchIngestMode,
            Option<ManifestGeneration>,
            StructuralReplaceScope,
        ) = ciborium::from_reader(scope_op.payload.as_slice())?;
        let Some(decoded_tree) = decoded_scope.trees.first().map(|tree| &tree.record) else {
            return Err(test_failure(
                "expected one structural tree in replace scope",
            ));
        };
        let first_role = decoded_tree
            .role_tags
            .first()
            .map(|role| role.role.as_ref());
        ensure(
            decoded_tree.role_tags.len() == 1 && first_role == Some("expr"),
            "structural payload lost role tags",
        )?;
        ensure(
            receipt.generation == ManifestGeneration::new(1)
                && receipt.accepted_replace_scopes == 1
                && receipt.accepted_tombstone_scopes == 0,
            "unexpected structural receipt scope counts",
        )?;
        Ok(())
    }

    #[test]
    fn structural_adapter_rejects_tombstone_scopes_before_publish() -> TestRes {
        let publisher = Arc::new(FakeLexicalPublisher::new());
        let adapter = ChannelStructuralIngestAdapter::new(publisher.clone());

        let batch = StructuralIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
            base_generation: Some(ManifestGeneration::new(0)),
            manifest_digest: "manifest:str-tombstone".to_string(),
            batch_digest: "batch:str-tombstone".to_string(),
            mode: BatchIngestMode::Delta,
            replace_scopes: Vec::new(),
            tombstone_scopes: vec![quanta_index_contract::StructuralTombstoneScope {
                scope: fixture_scope(),
            }],
            seal: false,
        };

        let receipt = adapter.publish_batch(&batch)?;
        ensure(
            receipt.accepted_replace_scopes == 0 && receipt.accepted_tombstone_scopes == 1,
            "unexpected structural tombstone receipt counts",
        )?;
        ensure(
            matches!(
                publisher.take()?.as_slice(),
                [LexicalChannelOp::TombstoneStructuralScope(_)]
            ),
            "unexpected structural tombstone op sequence",
        )?;
        Ok(())
    }
}
