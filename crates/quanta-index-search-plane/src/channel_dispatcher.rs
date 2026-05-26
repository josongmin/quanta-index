//! Channel event dispatcher.
//!
//! Drains lexical / semantic subscribers, applies each op to the respective
//! adapter, and on `Seal` marks the generation sealed in the [`Ledger`].
//!
//! Apply granularity is per-event (`build(&[op])`) — the reference adapters
//! treat that as cheap; production adapters can batch internally.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Result;
use quanta_index_channel::{
    BundleChannelSubscriber, LexicalChannelEvent, LexicalWalSubscriber, SemanticChannelEvent,
    SemanticWalSubscriber,
};
use quanta_index_contract::{ChannelSeq, LexicalChannelOp, SemanticChannelOp};
use quanta_index_core::{ChannelDispatchPolicy, LexicalIndexBuildPort, SemanticIndexBuildPort};

use crate::Ledger;

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Identifies which indexing track a ledger read/write targets.
#[derive(Debug, Clone, Copy)]
enum ChannelTrack {
    Lexical,
    Semantic,
}

pub struct ChannelDispatcher {
    lex_sub: LexicalWalSubscriber,
    sem_sub: SemanticWalSubscriber,
    lex_builder: Arc<dyn LexicalIndexBuildPort + Send + Sync>,
    sem_builder: Arc<dyn SemanticIndexBuildPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
    poll_interval: Duration,
}

impl ChannelDispatcher {
    #[must_use]
    pub fn new(
        lex_sub: LexicalWalSubscriber,
        sem_sub: SemanticWalSubscriber,
        lex_builder: Arc<dyn LexicalIndexBuildPort + Send + Sync>,
        sem_builder: Arc<dyn SemanticIndexBuildPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self {
            lex_sub,
            sem_sub,
            lex_builder,
            sem_builder,
            ledger,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    /// Drain both subscribers once (non-blocking). Returns `true` if any
    /// event was processed. Useful for tests.
    pub fn poll_once(&mut self) -> Result<bool> {
        let lex = self.drain_lex()?;
        let sem = self.drain_sem()?;
        Ok(lex || sem)
    }

    /// Run until `should_stop` returns `true`. Sleeps `poll_interval` between
    /// empty drains; otherwise drains greedily.
    pub fn run_until<F>(&mut self, mut should_stop: F) -> Result<()>
    where
        F: FnMut() -> bool,
    {
        while !should_stop() {
            let progressed = self.poll_once()?;
            if !progressed {
                std::thread::sleep(self.poll_interval);
            }
        }
        Ok(())
    }

    fn drain_lex(&mut self) -> Result<bool> {
        let mut progressed = false;
        let mut last_emitted = read_last_seen(ChannelTrack::Lexical, &self.ledger)?;
        while let Some(event) = self.lex_sub.next_event()? {
            ChannelDispatchPolicy::validate_monotonic_seq(event.seq, last_emitted)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            last_emitted = event.seq;
            apply_lex(&event, self.lex_builder.as_ref(), &self.ledger)?;
            self.lex_sub.ack(event.seq)?;
            record_observed(ChannelTrack::Lexical, event.seq, &self.ledger)?;
            progressed = true;
        }
        Ok(progressed)
    }

    fn drain_sem(&mut self) -> Result<bool> {
        let mut progressed = false;
        let mut last_emitted = read_last_seen(ChannelTrack::Semantic, &self.ledger)?;
        while let Some(event) = self.sem_sub.next_event()? {
            ChannelDispatchPolicy::validate_monotonic_seq(event.seq, last_emitted)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            last_emitted = event.seq;
            apply_sem(&event, self.sem_builder.as_ref(), &self.ledger)?;
            self.sem_sub.ack(event.seq)?;
            record_observed(ChannelTrack::Semantic, event.seq, &self.ledger)?;
            progressed = true;
        }
        Ok(progressed)
    }
}

/// Read the `last_seen` cursor for the given track. Centralises the lock-poison
/// translation so both drain loops share one implementation.
fn read_last_seen(track: ChannelTrack, ledger: &Arc<RwLock<Ledger>>) -> Result<ChannelSeq> {
    let guard = ledger
        .read()
        .map_err(|err| anyhow::anyhow!("ledger poisoned: {err}"))?;
    Ok(match track {
        ChannelTrack::Lexical => guard.lexical_last_seen(),
        ChannelTrack::Semantic => guard.semantic_last_seen(),
    })
}

/// Persist the observed seq into the ledger's per-track cursor. Used after a
/// successful `apply_*` + `ack` so the next drain validates monotonicity
/// against a fresh baseline.
fn record_observed(
    track: ChannelTrack,
    seq: ChannelSeq,
    ledger: &Arc<RwLock<Ledger>>,
) -> Result<()> {
    let mut guard = ledger
        .write()
        .map_err(|err| anyhow::anyhow!("ledger poisoned: {err}"))?;
    match track {
        ChannelTrack::Lexical => guard.set_lexical_last_seen(seq),
        ChannelTrack::Semantic => guard.set_semantic_last_seen(seq),
    }
    drop(guard);
    Ok(())
}

fn apply_lex(
    event: &LexicalChannelEvent,
    builder: &(dyn LexicalIndexBuildPort + Send + Sync),
    ledger: &Arc<RwLock<Ledger>>,
) -> Result<()> {
    let repo = event.op.repo_id().clone();
    let revision = event.op.revision_id().clone();
    let generation = event.op.generation();
    let ops = [event.op.clone()];
    builder
        .build(&repo, &revision, generation, &ops)
        .map_err(|e| anyhow::anyhow!("lexical build: {e}"))?;
    let mut guard = ledger
        .write()
        .map_err(|err| anyhow::anyhow!("ledger poisoned: {err}"))?;
    guard
        .apply_lexical_authority_op(&event.op)
        .map_err(|err| anyhow::anyhow!("lexical authority materialize: {err}"))?;
    if matches!(event.op, LexicalChannelOp::Seal(_)) {
        guard.lexical_seal(generation);
        guard.record_track_seal(
            &repo,
            &revision,
            quanta_index_contract::SearchPlaneTrackKind::Lexical,
            generation,
        );
    }
    drop(guard);
    Ok(())
}

fn apply_sem(
    event: &SemanticChannelEvent,
    builder: &(dyn SemanticIndexBuildPort + Send + Sync),
    ledger: &Arc<RwLock<Ledger>>,
) -> Result<()> {
    let repo = event.op.repo_id().clone();
    let revision = event.op.revision_id().clone();
    let generation = event.op.generation();
    let ops = [event.op.clone()];
    builder
        .build(&repo, &revision, generation, &ops)
        .map_err(|e| anyhow::anyhow!("semantic build: {e}"))?;
    if matches!(event.op, SemanticChannelOp::Seal(_)) {
        let mut guard = ledger
            .write()
            .map_err(|err| anyhow::anyhow!("ledger poisoned: {err}"))?;
        guard.semantic_seal(generation);
        guard.record_track_seal(
            &repo,
            &revision,
            quanta_index_contract::SearchPlaneTrackKind::Semantic,
            generation,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, RwLock};

    use quanta_index_channel::{
        BundleChannelPublisher, open_lexical_publisher, open_lexical_subscriber,
        open_semantic_publisher, open_semantic_subscriber,
    };
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        BatchIngestMode, ChunkId, ChunkRecord, EmbeddingDistanceMetric, EmbeddingId,
        EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord, LexicalChannelOp,
        LexicalReplaceScope, ManifestGeneration, OwnerDocKind, ReplaceLexicalScope,
        ReplaceSemanticScope, RepoId, RepoRelativePath, RevisionId, SearchScopeKey,
        SearchScopeSurface, SemanticChannelOp, SemanticReplaceScope,
    };
    use quanta_index_core::CoreError;

    use super::ChannelDispatcher;
    use crate::Ledger;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[derive(Default)]
    struct RecordingLexicalBuilder {
        calls: Mutex<Vec<String>>,
    }

    impl quanta_index_core::LexicalIndexBuildPort for RecordingLexicalBuilder {
        #[expect(
            clippy::significant_drop_tightening,
            reason = "test recorder; guard held for whole loop is intentional"
        )]
        fn build(
            &self,
            _repo: &RepoId,
            _revision: &RevisionId,
            _generation: ManifestGeneration,
            ops: &[LexicalChannelOp],
        ) -> Result<(), CoreError> {
            let mut guard = self
                .calls
                .lock()
                .map_err(|err| CoreError::Storage(format!("lex builder poisoned: {err}")))?;
            for op in ops {
                let label = match op {
                    LexicalChannelOp::FullBundle(_) => "full",
                    LexicalChannelOp::UpsertChunk(_) => "upsert_chunk",
                    LexicalChannelOp::DeleteChunk(_) => "delete_chunk",
                    LexicalChannelOp::UpsertSymbol(_) => "upsert_symbol",
                    LexicalChannelOp::DeleteSymbol(_) => "delete_symbol",
                    LexicalChannelOp::ReplaceLexicalScope(_) => "replace_lexical_scope",
                    LexicalChannelOp::TombstoneLexicalScope(_) => "tombstone_lexical_scope",
                    LexicalChannelOp::Seal(_) => "seal",
                    LexicalChannelOp::UpsertCommit(_) => "upsert_commit",
                    LexicalChannelOp::UpsertRef(_) => "upsert_ref",
                    LexicalChannelOp::UpsertTag(_) => "upsert_tag",
                    LexicalChannelOp::DeleteRef(_) => "delete_ref",
                    LexicalChannelOp::DeleteTag(_) => "delete_tag",
                    LexicalChannelOp::UpsertDirty(_) => "upsert_dirty",
                    LexicalChannelOp::EvictDirty(_) => "evict_dirty",
                    LexicalChannelOp::UpsertParseTree(_) => "upsert_parse_tree",
                    LexicalChannelOp::DeleteParseTree(_) => "delete_parse_tree",
                    LexicalChannelOp::ReplaceStructuralScope(_) => "replace_structural_scope",
                    LexicalChannelOp::TombstoneStructuralScope(_) => "tombstone_structural_scope",
                    LexicalChannelOp::UpsertDiffHunk(_) => "upsert_diff_hunk",
                };
                guard.push(label.to_string());
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingSemanticBuilder {
        calls: Mutex<Vec<String>>,
    }

    impl quanta_index_core::SemanticIndexBuildPort for RecordingSemanticBuilder {
        #[expect(
            clippy::significant_drop_tightening,
            reason = "test recorder; guard held for whole loop is intentional"
        )]
        fn build(
            &self,
            _repo: &RepoId,
            _revision: &RevisionId,
            _generation: ManifestGeneration,
            ops: &[SemanticChannelOp],
        ) -> Result<(), CoreError> {
            let mut guard = self
                .calls
                .lock()
                .map_err(|err| CoreError::Storage(format!("sem builder poisoned: {err}")))?;
            for op in ops {
                let label = match op {
                    SemanticChannelOp::FullBundle(_) => "full",
                    SemanticChannelOp::UpsertEmbedding(_) => "upsert_embedding",
                    SemanticChannelOp::DeleteEmbedding(_) => "delete_embedding",
                    SemanticChannelOp::ReplaceSemanticScope(_) => "replace_semantic_scope",
                    SemanticChannelOp::TombstoneSemanticScope(_) => "tombstone_semantic_scope",
                    SemanticChannelOp::Seal(_) => "seal",
                };
                guard.push(label.to_string());
            }
            Ok(())
        }
    }

    fn repo_id() -> RepoId {
        RepoId::new("repo-channel")
    }

    fn revision_id() -> RevisionId {
        RevisionId::new("rev-channel")
    }

    fn generation() -> ManifestGeneration {
        ManifestGeneration::new(5)
    }

    fn scope_key() -> SearchScopeKey {
        SearchScopeKey {
            doc_surface: SearchScopeSurface::Chunk,
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        }
    }

    fn rust_language() -> Result<LanguageCode, Box<dyn std::error::Error>> {
        LanguageCode::new("rust")
            .map_err(|err| format!("invalid hard-coded test language code: {err}").into())
    }

    fn chunk_record() -> Result<ChunkRecord, Box<dyn std::error::Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new("chunk-1"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            language: rust_language()?,
            start_byte: 0,
            end_byte: 3,
            start_line: 1,
            end_line: 1,
            snippet: "lex".to_string().into_boxed_str(),
            indexed_text: "lex".to_string().into_boxed_str(),
            text_digest: "text:1".to_string().into_boxed_str(),
            shape_digest: "shape:1".to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
        })
    }

    fn embedding_record() -> Result<EmbeddingRecord, Box<dyn std::error::Error>> {
        Ok(EmbeddingRecord {
            embedding_id: EmbeddingId::new("emb-1"),
            owner_kind: OwnerDocKind::Chunk,
            owner_id: "chunk-1".to_string().into_boxed_str(),
            source_doc_id: "chunk-1".to_string().into_boxed_str(),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            language: rust_language()?,
            symbol_kind: None,
            start_byte: 0,
            end_byte: 3,
            start_line: 1,
            end_line: 1,
            snippet: "lex".to_string().into_boxed_str(),
            embedding_input_digest: "input:1".to_string().into_boxed_str(),
            vector_digest: "vector:1".to_string().into_boxed_str(),
            view_kind: "raw_chunk".to_string().into_boxed_str(),
            vector: vec![0.1, 0.2, 0.3],
        })
    }

    fn model_contract() -> EmbeddingModelContract {
        EmbeddingModelContract {
            model_id: "test-model".to_string().into_boxed_str(),
            model_version: None,
            dimension: 3,
            normalization: EmbeddingNormalization::None,
            distance_metric: EmbeddingDistanceMetric::Cosine,
            policy_digest: "policy:test".to_string().into_boxed_str(),
            view_policy_digest: None,
        }
    }

    fn encode_cbor<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut buf = Vec::new();
        ciborium::into_writer(value, &mut buf)?;
        Ok(buf)
    }

    #[test]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "test wires up multiple mock channel handles + builders intentionally held for the whole scenario"
    )]
    fn poll_once_updates_ledger_and_replays_both_tracks() -> TestResult {
        let dir = tempfile::tempdir()?;
        let chunk_payload = encode_cbor(&(
            BatchIngestMode::ReplaceGeneration,
            None::<ManifestGeneration>,
            LexicalReplaceScope {
                scope: scope_key(),
                scope_digest: "scope:lexical".to_string(),
                chunks: vec![chunk_record()?],
                symbols: Vec::new(),
            },
        ))?;
        let lex_pub = open_lexical_publisher(dir.path())?;
        let _lex_upsert_seq =
            lex_pub.publish(LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
                repo_id: repo_id(),
                revision_id: revision_id(),
                generation: generation(),
                payload: chunk_payload,
            }))?;
        let _lex_seal_seq = lex_pub.seal(repo_id(), revision_id(), generation())?;

        let embedding_payload = encode_cbor(&(
            BatchIngestMode::ReplaceGeneration,
            None::<ManifestGeneration>,
            model_contract(),
            SemanticReplaceScope {
                scope: scope_key(),
                scope_digest: "scope:semantic".to_string(),
                embeddings: vec![embedding_record()?],
            },
        ))?;
        let sem_pub = open_semantic_publisher(dir.path())?;
        let _sem_upsert_seq = sem_pub.publish(SemanticChannelOp::ReplaceSemanticScope(
            ReplaceSemanticScope {
                repo_id: repo_id(),
                revision_id: revision_id(),
                generation: generation(),
                payload: embedding_payload,
            },
        ))?;
        let _sem_seal_seq = sem_pub.seal(repo_id(), revision_id(), generation())?;

        let lex_builder = Arc::new(RecordingLexicalBuilder::default());
        let sem_builder = Arc::new(RecordingSemanticBuilder::default());
        let lex_port: Arc<dyn quanta_index_core::LexicalIndexBuildPort + Send + Sync> =
            lex_builder.clone();
        let sem_port: Arc<dyn quanta_index_core::SemanticIndexBuildPort + Send + Sync> =
            sem_builder.clone();
        let ledger = Arc::new(RwLock::new(Ledger::default()));
        let mut dispatcher = ChannelDispatcher::new(
            open_lexical_subscriber(dir.path())?,
            open_semantic_subscriber(dir.path())?,
            lex_port,
            sem_port,
            Arc::clone(&ledger),
        );

        if !dispatcher.poll_once()? {
            return Err("expected first poll_once() to replay channel events".into());
        }
        if dispatcher.poll_once()? {
            return Err("expected second poll_once() to observe no new events".into());
        }

        let guard = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        if guard.lexical_sealed() != Some(generation()) {
            return Err(format!("unexpected lexical seal: {:?}", guard.lexical_sealed()).into());
        }
        if guard.semantic_sealed() != Some(generation()) {
            return Err(format!("unexpected semantic seal: {:?}", guard.semantic_sealed()).into());
        }
        if guard.lexical_last_seen().get() != 2 {
            return Err(format!(
                "unexpected lexical last_seen: {}",
                guard.lexical_last_seen().get()
            )
            .into());
        }
        if guard.semantic_last_seen().get() != 2 {
            return Err(format!(
                "unexpected semantic last_seen: {}",
                guard.semantic_last_seen().get()
            )
            .into());
        }
        let structural = guard
            .structural_state(&repo_id(), &revision_id(), generation())
            .ok_or_else(|| "expected structural authority state to materialize".to_string())?;
        if !structural.chunks().contains_key(&ChunkId::new("chunk-1")) {
            return Err(
                "expected replayed lexical scope chunk in structural authority state".into(),
            );
        }
        drop(guard);

        let lex_calls = lex_builder
            .calls
            .lock()
            .map_err(|err| format!("lex calls poisoned: {err}"))?;
        if lex_calls.as_slice() != ["replace_lexical_scope", "seal"] {
            return Err(format!("unexpected lexical calls: {:?}", lex_calls.as_slice()).into());
        }
        drop(lex_calls);

        let sem_calls = sem_builder
            .calls
            .lock()
            .map_err(|err| format!("sem calls poisoned: {err}"))?;
        if sem_calls.as_slice() != ["replace_semantic_scope", "seal"] {
            return Err(format!("unexpected semantic calls: {:?}", sem_calls.as_slice()).into());
        }
        Ok(())
    }
}
