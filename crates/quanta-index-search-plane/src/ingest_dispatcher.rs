//! Search-plane ingest orchestration (QI-RT-01).
//!
//! Producer sends a typed [`SearchPlaneIngestIpcRequest`] over UDS
//! `ingest.sock`. This dispatcher fans the typed batch out to the channel
//! publishers (for lexical / semantic) or to the repo-map bundle ingest port
//! (for repo-map). The producer never opens a channel publisher directly.
//!
//! Composition root in `quanta-index-searchd` is the only place that names
//! concrete adapter types (channel publishers, repo-map ingest); this module
//! holds only [`Arc<dyn ...Port>`] (CLAUDE.md DIP rule).

use std::sync::Arc;

use quanta_index_channel::BundleChannelPublisher;
use quanta_index_contract::{
    BatchIngestMode, BatchPublishReceipt, DeleteChunk, DeleteEmbedding, DeleteParseTree, DeleteRef,
    DeleteSymbol, DeleteTag, DirtyIngestBatch, EvictDirty, HistoryIngestBatch, LexicalChannelOp,
    LexicalChunkMutation, LexicalFullBundle, LexicalIngestBatch, LexicalSymbolMutation,
    RepoMapMutationAck, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse,
    SearchPlaneIpcError, SemanticChannelOp, SemanticEmbeddingMutation, SemanticFullBundle,
    SemanticIngestBatch, StructuralIngestBatch, UpsertChunk, UpsertCommit, UpsertDiffHunk,
    UpsertDirty, UpsertEmbedding, UpsertParseTree, UpsertRef, UpsertSymbol, UpsertTag,
};
use quanta_index_core::{
    CoreError, LexicalIngestPort, RepoMapBundleIngestPort, SemanticIngestPort,
};

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
// Channel-backed adapters
// =============================================================================

/// Adapter that implements [`LexicalIngestPort`] on top of a channel publisher.
///
/// Lifted out of the SDK so the SDK can drop its direct
/// `quanta-index-channel` dependency (QI-SDK-01); searchd's composition root
/// owns the publisher.
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
        let mut receipt = BatchPublishReceipt::default();
        if matches!(batch.mode, BatchIngestMode::ReplaceGeneration) {
            let seq = self
                .publisher
                .publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                    payload: batch.manifest_payload.clone(),
                }))
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.record(seq);
        }
        for chunk in &batch.chunks {
            let op = match chunk {
                LexicalChunkMutation::Upsert(payload) => {
                    let encoded = encode_cbor(&payload.record).map_err(|err| {
                        CoreError::InvalidContract(format!(
                            "lexical ingest: encode chunk record: {err}"
                        ))
                    })?;
                    LexicalChannelOp::UpsertChunk(UpsertChunk {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        chunk_id: payload.chunk_id.clone(),
                        payload: encoded,
                    })
                }
                LexicalChunkMutation::Delete(payload) => {
                    LexicalChannelOp::DeleteChunk(DeleteChunk {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        chunk_id: payload.chunk_id.clone(),
                    })
                }
            };
            let seq = self
                .publisher
                .publish(op)
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.record(seq);
        }
        for symbol in &batch.symbols {
            let op = match symbol {
                LexicalSymbolMutation::Upsert(payload) => {
                    let encoded = encode_cbor(&payload.record).map_err(|err| {
                        CoreError::InvalidContract(format!(
                            "lexical ingest: encode symbol record: {err}"
                        ))
                    })?;
                    LexicalChannelOp::UpsertSymbol(UpsertSymbol {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        symbol_id: payload.symbol_id.clone(),
                        payload: encoded,
                    })
                }
                LexicalSymbolMutation::Delete(payload) => {
                    LexicalChannelOp::DeleteSymbol(DeleteSymbol {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        symbol_id: payload.symbol_id.clone(),
                    })
                }
            };
            let seq = self
                .publisher
                .publish(op)
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.record(seq);
        }
        if batch.seal {
            let seq = self
                .publisher
                .seal(
                    batch.repo_id.clone(),
                    batch.revision_id.clone(),
                    batch.generation,
                )
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.record(seq);
            receipt.mark_sealed();
        }
        self.publisher
            .flush()
            .map_err(|err| channel_error_to_core(&err))?;
        Ok(receipt)
    }
}

/// Adapter that implements [`SemanticIngestPort`] on top of a channel
/// publisher.
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
        let mut receipt = BatchPublishReceipt::default();
        if matches!(batch.mode, BatchIngestMode::ReplaceGeneration) {
            let seq = self
                .publisher
                .publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                    payload: batch.manifest_payload.clone(),
                }))
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.record(seq);
        }
        for embedding in &batch.embeddings {
            let op = match embedding {
                SemanticEmbeddingMutation::Upsert(payload) => {
                    let encoded = encode_cbor(&payload.record).map_err(|err| {
                        CoreError::InvalidContract(format!(
                            "semantic ingest: encode embedding record: {err}"
                        ))
                    })?;
                    SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        embedding_id: payload.embedding_id.clone(),
                        payload: encoded,
                    })
                }
                SemanticEmbeddingMutation::Delete(payload) => {
                    SemanticChannelOp::DeleteEmbedding(DeleteEmbedding {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        embedding_id: payload.embedding_id.clone(),
                    })
                }
            };
            let seq = self
                .publisher
                .publish(op)
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.record(seq);
        }
        if batch.seal {
            let seq = self
                .publisher
                .seal(
                    batch.repo_id.clone(),
                    batch.revision_id.clone(),
                    batch.generation,
                )
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.record(seq);
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
        let mut receipt = BatchPublishReceipt::default();
        for record in &batch.commits {
            let seq = self
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
            receipt.record(seq);
        }
        for mutation in &batch.refs {
            let op = match mutation {
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
            };
            let seq = self
                .publisher
                .publish(op)
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.record(seq);
        }
        for mutation in &batch.tags {
            let op = match mutation {
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
            };
            let seq = self
                .publisher
                .publish(op)
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.record(seq);
        }
        for hunk in &batch.diff_hunks {
            let seq = self
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
            receipt.record(seq);
        }
        self.publisher
            .flush()
            .map_err(|err| channel_error_to_core(&err))?;
        Ok(receipt)
    }
}

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
        let mut receipt = BatchPublishReceipt::default();
        for entry in &batch.entries {
            let op = match entry {
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
            };
            let seq = self
                .publisher
                .publish(op)
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.record(seq);
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
        let mut receipt = BatchPublishReceipt::default();
        for tree in &batch.trees {
            let op = match tree {
                quanta_index_contract::ParseTreeMutation::Upsert(payload) => {
                    LexicalChannelOp::UpsertParseTree(UpsertParseTree {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        chunk_id: payload.chunk_id.clone(),
                        payload: encode_cbor(&payload.record).map_err(|err| {
                            CoreError::InvalidContract(format!(
                                "structural ingest: encode parse tree record: {err}"
                            ))
                        })?,
                    })
                }
                quanta_index_contract::ParseTreeMutation::Delete(payload) => {
                    LexicalChannelOp::DeleteParseTree(DeleteParseTree {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        chunk_id: payload.chunk_id.clone(),
                    })
                }
            };
            let seq = self
                .publisher
                .publish(op)
                .map_err(|err| channel_error_to_core(&err))?;
            receipt.record(seq);
        }
        self.publisher
            .flush()
            .map_err(|err| channel_error_to_core(&err))?;
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
        CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, LangId, ParseNode, ParseRoleTag,
        ParseTreeRecord,
    };
    use quanta_index_contract::{
        ChannelSeq, ChunkId, ChunkRecord, DiffHunkSide, EmbeddingId, EmbeddingRecord,
        HistoryDiffHunkUpsert, HistoryIngestBatch, HistoryRefMutation, HistoryRefUpsert,
        LexicalChunkDelete, LexicalChunkUpsert, ManifestGeneration, ParseTreeDelete,
        ParseTreeMutation, ParseTreeUpsert, RepoId, RepoRelativePath, RevisionId,
        SemanticEmbeddingDelete, SemanticEmbeddingUpsert, StructuralIngestBatch,
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

    fn fixture_chunk_record() -> ChunkRecord {
        ChunkRecord {
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: "rust".to_string().into_boxed_str(),
            start_line: 1,
            end_line: 10,
            snippet: "fn main() {}".to_string().into_boxed_str(),
        }
    }

    fn fixture_embedding_record() -> EmbeddingRecord {
        EmbeddingRecord {
            owner_kind: "Function".to_string().into_boxed_str(),
            owner_id: "main".to_string().into_boxed_str(),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: LangId::Rust,
            symbol_kind: None,
            start_line: 1,
            end_line: 10,
            snippet: "fn main() {}".to_string().into_boxed_str(),
            vector: vec![0.1, 0.2, 0.3],
        }
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

    fn fixture_parse_tree_record() -> ParseTreeRecord {
        ParseTreeRecord {
            wire_version: 1,
            lang: LangId::Rust,
            root: ParseNode {
                kind: "function_item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 10,
                children: Vec::new(),
            },
            source_hash: [9; 32],
            role_tag_schema_version: 1,
            role_tags: vec![ParseRoleTag {
                role: "expr".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 4,
            }],
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
            mode: BatchIngestMode::ReplaceGeneration,
            manifest_payload: vec![0xAA],
            chunks: vec![
                LexicalChunkMutation::Upsert(LexicalChunkUpsert {
                    chunk_id: ChunkId::new("c1"),
                    record: fixture_chunk_record(),
                }),
                LexicalChunkMutation::Delete(LexicalChunkDelete {
                    chunk_id: ChunkId::new("c2"),
                }),
            ],
            symbols: vec![],
            seal: true,
        };
        let receipt = adapter.publish_batch(&batch)?;
        // FullBundle + UpsertChunk + DeleteChunk + Seal = 4 ops
        let ops = publisher.take()?;
        ensure(
            matches!(
                ops.as_slice(),
                [
                    LexicalChannelOp::FullBundle(_),
                    LexicalChannelOp::UpsertChunk(_),
                    LexicalChannelOp::DeleteChunk(_),
                    LexicalChannelOp::Seal(_),
                ]
            ),
            "unexpected lexical op sequence for replace-generation batch",
        )?;
        ensure(
            receipt.first_seq == Some(ChannelSeq::new(0)),
            "unexpected lexical receipt first_seq",
        )?;
        ensure(
            receipt.last_seq == Some(ChannelSeq::new(3)),
            "unexpected lexical receipt last_seq",
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
            mode: BatchIngestMode::Delta,
            manifest_payload: vec![],
            chunks: vec![LexicalChunkMutation::Delete(LexicalChunkDelete {
                chunk_id: ChunkId::new("c1"),
            })],
            symbols: vec![],
            seal: false,
        };
        let receipt = adapter.publish_batch(&batch)?;
        let ops = publisher.take()?;
        // Only the DeleteChunk op.
        ensure(
            matches!(ops.as_slice(), [LexicalChannelOp::DeleteChunk(_)]),
            "unexpected lexical op sequence for delta batch",
        )?;
        ensure(
            !receipt.sealed,
            "expected lexical receipt to remain unsealed",
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
            mode: BatchIngestMode::ReplaceGeneration,
            manifest_payload: vec![],
            embeddings: vec![
                SemanticEmbeddingMutation::Upsert(SemanticEmbeddingUpsert {
                    embedding_id: EmbeddingId::new("e1"),
                    record: fixture_embedding_record(),
                }),
                SemanticEmbeddingMutation::Delete(SemanticEmbeddingDelete {
                    embedding_id: EmbeddingId::new("e2"),
                }),
            ],
            seal: true,
        };
        let receipt = adapter.publish_batch(&batch)?;
        let ops = publisher.take()?;
        // FullBundle + UpsertEmbedding + DeleteEmbedding + Seal = 4
        ensure(
            matches!(
                ops.as_slice(),
                [
                    SemanticChannelOp::FullBundle(_),
                    SemanticChannelOp::UpsertEmbedding(_),
                    SemanticChannelOp::DeleteEmbedding(_),
                    SemanticChannelOp::Seal(_),
                ]
            ),
            "unexpected semantic op sequence for replace-generation batch",
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
            receipt.first_seq == Some(ChannelSeq::new(0))
                && receipt.last_seq == Some(ChannelSeq::new(3)),
            "unexpected history receipt sequence range",
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
            receipt.first_seq == Some(ChannelSeq::new(0))
                && receipt.last_seq == Some(ChannelSeq::new(1)),
            "unexpected dirty receipt sequence range",
        )?;
        Ok(())
    }

    #[test]
    fn structural_adapter_fans_out_parse_tree_upsert_and_delete_ops() -> TestRes {
        let publisher = Arc::new(FakeLexicalPublisher::new());
        let adapter = ChannelStructuralIngestAdapter::new(publisher.clone());

        let batch = StructuralIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
            trees: vec![
                ParseTreeMutation::Upsert(ParseTreeUpsert {
                    chunk_id: ChunkId::new("chunk-tree"),
                    record: fixture_parse_tree_record(),
                }),
                ParseTreeMutation::Delete(ParseTreeDelete {
                    chunk_id: ChunkId::new("chunk-drop"),
                }),
            ],
        };
        let receipt = adapter.publish_batch(&batch)?;
        let ops = publisher.take()?;
        ensure(
            matches!(
                ops.as_slice(),
                [
                    LexicalChannelOp::UpsertParseTree(_),
                    LexicalChannelOp::DeleteParseTree(_),
                ]
            ),
            "unexpected structural op sequence",
        )?;
        let Some(LexicalChannelOp::UpsertParseTree(tree)) = ops.first() else {
            return Err(test_failure("expected UpsertParseTree"));
        };
        let decoded_tree: ParseTreeRecord = ciborium::from_reader(tree.payload.as_slice())?;
        let first_role = decoded_tree
            .role_tags
            .first()
            .map(|role| role.role.as_ref());
        ensure(
            decoded_tree.role_tags.len() == 1 && first_role == Some("expr"),
            "structural payload lost role tags",
        )?;
        ensure(
            receipt.first_seq == Some(ChannelSeq::new(0))
                && receipt.last_seq == Some(ChannelSeq::new(1)),
            "unexpected structural receipt sequence range",
        )?;
        Ok(())
    }
}
