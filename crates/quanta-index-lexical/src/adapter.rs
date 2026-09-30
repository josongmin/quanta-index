//! The adapter's own operations: writers, generations, overlays and sealing.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::channel_payloads::{
    decode_chunk_payload, decode_replace_scope_payload, decode_symbol_payload,
    decode_tombstone_scope_payload, encode_cbor,
};
use crate::documents::{
    add_content_fields, add_metadata_fields, add_snippet_field, add_symbol_fields,
};
use crate::generation_dir::ensure_unsealed;
use crate::generation_dir::{
    clone_generation_directory_preserving_existing, ensure_base_generation_is_servable,
    lexical_index_content_exists, persist_lexical_delta_base, read_lexical_delta_base,
};
use crate::index_store::{read_lexical_sealed_identity, validate_lexical_sealed_identity};
use crate::overlay_codec::OverlayFamily;
use crate::overlay_codec::{decode_repo_metadata_payload, encode_repo_metadata_payload};
use crate::regex::RegexPolicy;
use crate::regex_match_cache::RegexMatchCache;
use crate::sealed_generation::{persist_overlay, remove_overlay};
use crate::text_authority::TextAuthorityWriteReceipt;
use crate::{
    GenKey, GenerationWriter, LexicalAdapter, LexicalSealCommitmentStats, SYMBOL_DOC_KIND,
    SchemaFields, TEXT_DOC_KIND, TextDocAllocator, WriterCache, WriterRelease,
};
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::{
    BatchIngestMode, ClearLexicalSurface, GenerationSnapshot, LexicalFullBundle, LexicalSeal,
    ManifestGeneration, ReplaceLexicalScope, SearchCorpusIngestBatch, SearchPlaneTrackKind,
    SearchScopeSurface, SourceFileKey, TombstoneLexicalScope,
};
use quanta_index_core::domains::generation::GenerationStorageKeyV1;
use quanta_index_core::{
    CoreError, LexicalExecutionBudgetV1, LexicalWriterCacheStats, LexicalWriterPolicy,
    RegexMatchCachePolicy, RegexMatchCacheStats, TextAuthorityUpdateStats, WriterAdmissionPort,
};
use sha2::{Digest as _, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;
use tantivy::schema::TantivyDocument;
use tantivy::{IndexWriter, Term};

impl LexicalAdapter {
    /// Construct an adapter rooted at the given directory with the default
    /// [`RegexPolicy`] and [`LexicalExecutionBudgetV1`]. The directory will be
    /// created lazily as generations are materialized.
    #[must_use]
    pub fn with_state_root(state_root: PathBuf) -> Self {
        Self::with_state_root_and_policies(
            state_root,
            RegexPolicy::defaults(),
            LexicalExecutionBudgetV1::DEFAULT,
            RegexMatchCachePolicy::DEFAULT,
            LexicalWriterPolicy::DEFAULT,
        )
    }

    /// Construct an adapter rooted at the given directory with explicit
    /// policies. Use this constructor when the deployment needs to tighten or
    /// relax the regex dialect / candidate cap / trigram-missing threshold
    /// defaults, the examined-candidate budget, or the writer envelope.
    #[must_use]
    pub fn with_state_root_and_policies(
        state_root: PathBuf,
        regex_policy: RegexPolicy,
        execution_budget: LexicalExecutionBudgetV1,
        regex_match_cache_policy: RegexMatchCachePolicy,
        writer_policy: LexicalWriterPolicy,
    ) -> Self {
        Self {
            state_root,
            fields: SchemaFields::build(),
            writers: Arc::new(Mutex::new(WriterCache::new(writer_policy))),
            regex_match_cache: Arc::new(Mutex::new(RegexMatchCache::new(regex_match_cache_policy))),
            text_authority_updates: Arc::new(Mutex::new(TextAuthorityUpdateStats::default())),
            seal_commitments: Arc::new(Mutex::new(LexicalSealCommitmentStats::default())),
            coverage_reads: Arc::new(Mutex::new(crate::LexicalCoverageReadByPhaseStats::default())),
            coverage_decode_cache: Mutex::new(
                crate::sealed_generation::coverage::CoverageDecodeCache::default(),
            ),
            regex_policy,
            execution_budget,
            generation_mutations: std::array::from_fn(|_| Mutex::new(())),
            directory_lifecycle: std::sync::RwLock::new(()),
            scrub_progress: Mutex::new(
                quanta_index_core::domains::integrity::IntegrityScrubProgressV1::default(),
            ),
        }
    }

    fn generation_mutation_stripe(&self, key: &GenKey) -> Result<usize, CoreError> {
        let mut hasher = Sha256::new();
        hasher.update(
            GenerationStorageKeyV1::for_repo_revision(&key.repo_id, &key.revision_id)
                .as_str()
                .as_bytes(),
        );
        hasher.update(key.generation.get().to_le_bytes());
        let digest = hasher.finalize();
        let stripe = usize::from(
            digest
                .first()
                .copied()
                .ok_or_else(|| CoreError::Storage("lexical mutation digest is empty".into()))?,
        );
        if self.generation_mutations.get(stripe).is_none() {
            return Err(CoreError::Storage(
                "lexical mutation stripe outside table".into(),
            ));
        }
        Ok(stripe)
    }

    /// Hold one generation's mutation boundary through its last write.
    pub(crate) fn generation_mutation_guard(
        &self,
        key: &GenKey,
    ) -> Result<MutexGuard<'_, ()>, CoreError> {
        let stripe = self.generation_mutation_stripe(key)?;
        self.generation_mutations
            .get(stripe)
            .ok_or_else(|| CoreError::Storage("lexical mutation stripe outside table".into()))?
            .lock()
            .map_err(|error| CoreError::Storage(format!("lexical mutation lock poisoned: {error}")))
    }

    /// Pin a delta's base against scrub quarantine while the target is
    /// planned, cloned and sealed. Sorted, deduplicated stripes avoid a
    /// cross-generation lock cycle even when two keys hash to one stripe.
    pub(crate) fn generation_build_guards(
        &self,
        target: &GenKey,
        base: Option<ManifestGeneration>,
    ) -> Result<Vec<MutexGuard<'_, ()>>, CoreError> {
        let mut stripes = vec![self.generation_mutation_stripe(target)?];
        if let Some(base) = base {
            let base_key = GenKey {
                repo_id: target.repo_id.clone(),
                revision_id: target.revision_id.clone(),
                generation: base,
            };
            stripes.push(self.generation_mutation_stripe(&base_key)?);
        }
        stripes.sort_unstable();
        stripes.dedup();
        stripes
            .into_iter()
            .map(|stripe| {
                self.generation_mutations
                    .get(stripe)
                    .ok_or_else(|| {
                        CoreError::InvalidContract(format!(
                            "lexical mutation stripe {stripe} is out of range"
                        ))
                    })?
                    .lock()
                    .map_err(|error| {
                        CoreError::Storage(format!("lexical mutation lock poisoned: {error}"))
                    })
            })
            .collect()
    }

    /// Hold the sealed-generation directory lifecycle (see the field).
    pub(crate) fn directory_lifecycle_guard(&self) -> Result<RwLockWriteGuard<'_, ()>, CoreError> {
        self.directory_lifecycle.write().map_err(|err| {
            CoreError::Storage(format!(
                "lexical: generation directory lifecycle lock poisoned: {err}"
            ))
        })
    }

    /// Prevent directory removal while a builder or scrub step is active.
    pub(crate) fn directory_lifecycle_read_guard(
        &self,
    ) -> Result<RwLockReadGuard<'_, ()>, CoreError> {
        self.directory_lifecycle.read().map_err(|err| {
            CoreError::Storage(format!(
                "lexical: generation directory lifecycle lock poisoned: {err}"
            ))
        })
    }

    /// How much text-authority derivation the adapter has done so far
    /// (QI-BB-006).
    pub fn text_authority_update_stats(&self) -> Result<TextAuthorityUpdateStats, CoreError> {
        self.text_authority_updates
            .lock()
            .map(|stats| *stats)
            .map_err(|err| {
                CoreError::Storage(format!("lexical text authority stats poisoned: {err}"))
            })
    }

    /// Fold one sidecar write into the adapter's running totals.
    pub(crate) fn record_text_authority_write(
        &self,
        rebuilt: bool,
        receipt: TextAuthorityWriteReceipt,
    ) -> Result<(), CoreError> {
        let mut stats = self.text_authority_updates.lock().map_err(|err| {
            CoreError::Storage(format!("lexical text authority stats poisoned: {err}"))
        })?;
        if rebuilt {
            stats.rebuilds = stats.rebuilds.saturating_add(1);
        } else {
            stats.incremental_updates = stats.incremental_updates.saturating_add(1);
        }
        stats.docs_derived = stats.docs_derived.saturating_add(receipt.docs_derived);
        stats.docs_retired = stats.docs_retired.saturating_add(receipt.docs_retired);
        stats.shards_written = stats.shards_written.saturating_add(receipt.shards_written);
        stats.shards_inherited = stats
            .shards_inherited
            .saturating_add(receipt.shards_inherited);
        drop(stats);
        Ok(())
    }

    /// What the adapter's seals read and inherited so far
    /// (QI-BB-006 보완 #4).
    pub fn seal_commitment_stats(&self) -> Result<LexicalSealCommitmentStats, CoreError> {
        self.seal_commitments
            .lock()
            .map(|stats| *stats)
            .map_err(|err| CoreError::Storage(format!("lexical seal stats poisoned: {err}")))
    }

    /// Fold one seal's measurement into the adapter's running totals.
    pub(crate) fn record_seal_measurement(
        &self,
        seal: LexicalSealCommitmentStats,
    ) -> Result<(), CoreError> {
        self.seal_commitments
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical seal stats poisoned: {err}")))?
            .absorb(seal);
        Ok(())
    }

    /// Successful coverage decoder work across preflight, build and open.
    pub fn coverage_read_stats(&self) -> Result<crate::LexicalCoverageReadStats, CoreError> {
        self.coverage_reads
            .lock()
            .map(|stats| stats.total)
            .map_err(|err| CoreError::Storage(format!("lexical coverage stats poisoned: {err}")))
    }

    /// The same authenticated reads split by their actual call boundary.
    pub fn coverage_read_by_phase_stats(
        &self,
    ) -> Result<crate::LexicalCoverageReadByPhaseStats, CoreError> {
        self.coverage_reads
            .lock()
            .map(|stats| *stats)
            .map_err(|err| CoreError::Storage(format!("lexical coverage stats poisoned: {err}")))
    }

    pub(crate) fn record_coverage_read(
        &self,
        phase: crate::sealed_generation::coverage::CoverageReadPhase,
        read: crate::LexicalCoverageReadStats,
    ) -> Result<(), CoreError> {
        self.coverage_reads
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical coverage stats poisoned: {err}")))?
            .absorb(phase, read);
        Ok(())
    }

    /// What the regex match cache has done so far (QI-BB-024).
    pub fn regex_match_cache_stats(&self) -> Result<RegexMatchCacheStats, CoreError> {
        self.regex_match_cache
            .lock()
            .map(|cache| cache.stats())
            .map_err(|err| CoreError::Storage(format!("lexical regex cache poisoned: {err}")))
    }

    pub(crate) fn index_path(&self, key: &GenKey) -> PathBuf {
        GenerationStorageKeyV1::for_repo_revision(&key.repo_id, &key.revision_id)
            .generation_dir(&self.state_root, key.generation)
    }

    /// Make the sealed index final.
    ///
    /// Opens the writer (creating an empty index for a generation that
    /// indexed nothing, which must still be openable), commits once, waits
    /// for its merges so the sealed `meta.json` is the last one any writer
    /// produces, and drops the writer from the cache so nothing can commit
    /// to this generation again. A sealed generation is never built again,
    /// so its heap goes back to the envelope now.
    pub(crate) fn finalize_index_for_seal(&self, key: &GenKey) -> Result<(), CoreError> {
        let handle = self.writer_handle(key)?;
        drop(handle);
        self.writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?
            .release(key, WriterRelease::Seal)
    }

    pub(crate) fn writer_handle(
        &self,
        key: &GenKey,
    ) -> Result<Arc<Mutex<GenerationWriter>>, CoreError> {
        let path = self.index_path(key);
        let mut guard = self
            .writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
        guard.get_or_open(key, &self.fields, &path, Instant::now())
    }

    /// Commit and release every writer nothing has touched for the policy's
    /// idle interval (QI-BB-016); returns how many were released. The
    /// adapter sweeps after every batch, and the composition root's
    /// maintenance timer sweeps on its own schedule through
    /// [`quanta_index_core::WriterIdleSweepPort`].
    pub fn release_idle_writers(&self) -> Result<u64, CoreError> {
        self.writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?
            .release_idle(Instant::now())
    }

    /// Install the gate every new writer open is checked against
    /// (QI-BB-016); the adapter starts with the unbounded one.
    pub fn with_writer_admission(
        self,
        admission: Arc<dyn WriterAdmissionPort>,
    ) -> Result<Self, CoreError> {
        self.writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?
            .admission = admission;
        Ok(self)
    }

    /// What the writer cache holds and has done (QI-BB-016).
    pub fn writer_cache_stats(&self) -> Result<LexicalWriterCacheStats, CoreError> {
        Ok(self
            .writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?
            .stats())
    }

    pub(crate) fn invalidate_regex_match_cache_generation(
        &self,
        key: &GenKey,
    ) -> Result<(), CoreError> {
        self.regex_match_cache
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical regex cache poisoned: {err}")))?
            .invalidate_generation(key);
        Ok(())
    }

    /// Materializes the base generation this batch declares, exactly once.
    ///
    /// The recorded marker is the authority for "already carried forward", not
    /// directory existence: every per-generation authority sidecar creates the
    /// generation directory as a side effect, so a sidecar published before the
    /// lexical delta would otherwise skip the base clone and silently produce a
    /// generation holding only the delta.
    pub(crate) fn prepare_generation_for_ops(
        &self,
        key: &GenKey,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError> {
        self.prepare_generation_from_base(key, declared_delta_base_generation(ops)?)
    }

    pub(crate) fn prepare_generation_from_base(
        &self,
        key: &GenKey,
        base: Option<ManifestGeneration>,
    ) -> Result<(), CoreError> {
        let Some(requested_base) = base else {
            return Ok(());
        };
        let target_path = self.index_path(key);
        if let Some(recorded_base) = read_lexical_delta_base(&target_path)? {
            if recorded_base == requested_base {
                return Ok(());
            }
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::DeltaBaseConflict,
                message: format!(
                    "lexical: generation {} already carried forward base {}; refusing a batch that declares base {}",
                    key.generation.get(),
                    recorded_base.get(),
                    requested_base.get()
                ),
            });
        }
        if lexical_index_content_exists(&target_path) {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::DeltaBaseUnresolved,
                message: format!(
                    "lexical: generation {} already holds index content with no recorded base; cannot prove base {} was carried forward",
                    key.generation.get(),
                    requested_base.get()
                ),
            });
        }
        let base_key = GenKey {
            repo_id: key.repo_id.clone(),
            revision_id: key.revision_id.clone(),
            generation: requested_base,
        };
        let base_path = self.index_path(&base_key);
        ensure_base_generation_is_servable(&base_path)?;
        clone_generation_directory_preserving_existing(&base_path, &target_path)?;
        persist_lexical_delta_base(&target_path, requested_base)
    }

    pub(crate) fn delete_scope_docs(
        &self,
        writer: &IndexWriter,
        file: &SourceFileKey,
    ) -> Result<(), CoreError> {
        let query = crate::text_docs::source_file_query(&self.fields, file);
        let _opstamp = writer
            .delete_query(Box::new(query))
            .map_err(|error| CoreError::Storage(format!("lexical: source file delete: {error}")))?;
        Ok(())
    }

    pub(crate) fn clear_surface_docs(
        &self,
        writer: &IndexWriter,
        surface: SearchScopeSurface,
    ) -> bool {
        let doc_kind = match surface {
            SearchScopeSurface::Chunk => TEXT_DOC_KIND,
            SearchScopeSurface::Symbol => SYMBOL_DOC_KIND,
            // File and Module semantic rows have no lexical representation.
            SearchScopeSurface::File | SearchScopeSurface::Module => return false,
        };
        let term = Term::from_field_text(self.fields.doc_kind, doc_kind);
        let _opstamp = writer.delete_term(term);
        true
    }

    /// Apply an op that writes a generation-local overlay rather than the
    /// index. Returns `false` (nothing to commit) for every op it handles and
    /// for ops this adapter does not act on.
    ///
    /// The `FullBundle` payload is the repo-metadata overlay: a typed
    /// payload replaces the snapshot, an empty one removes it. Both land
    /// durably, and only before the seal.
    pub(crate) fn apply_snapshot_op(
        &self,
        key: &GenKey,
        op: &LexicalChannelOp,
    ) -> Result<bool, CoreError> {
        if let LexicalChannelOp::FullBundle(bundle) = op {
            let generation_dir = self.index_path(key);
            match decode_repo_metadata_payload(&bundle.payload)? {
                Some(metadata) => persist_overlay(
                    &generation_dir,
                    OverlayFamily::RepoMetadata,
                    &encode_repo_metadata_payload(&metadata)?,
                )?,
                None => remove_overlay(&generation_dir, OverlayFamily::RepoMetadata)?,
            }
        }
        Ok(false)
    }

    /// Publish one overlay family's snapshot into an unsealed generation.
    ///
    /// Every aux ingest route lands here: the batch is encoded by its
    /// family, written durably, and receipted with one accepted scope per
    /// entry. A sealed generation refuses the publish typed before any byte
    /// is written; its manifest committed to the overlays it has.
    pub(crate) fn publish_overlay(
        &self,
        key: &GenKey,
        family: OverlayFamily,
        bytes: &[u8],
        batch_digest: String,
        entries: usize,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let _mutation = self.generation_mutation_guard(key)?;
        let _lifecycle = self.directory_lifecycle_read_guard()?;
        let generation_dir = self.index_path(key);
        ensure_unsealed(&generation_dir, key.generation, family.file_name())?;
        persist_overlay(&generation_dir, family, bytes)?;
        let mut receipt = quanta_index_contract::BatchPublishReceipt::empty_for(
            key.generation,
            None,
            batch_digest,
        );
        for _entry in 0..entries {
            receipt.accept_replace_scope();
        }
        Ok(receipt)
    }

    /// The generation directory a sealed candidate names, and the identity
    /// found there: the preamble every door and every scrub shares.
    pub(crate) fn sealed_generation_dir_for(
        &self,
        candidate: &GenerationSnapshot,
        what: &str,
    ) -> Result<(PathBuf, GenerationSnapshot), CoreError> {
        if candidate.track != SearchPlaneTrackKind::Lexical {
            return Err(CoreError::InvalidContract(format!(
                "lexical {what} received {:?} track",
                candidate.track
            )));
        }
        let key = GenKey {
            repo_id: candidate.repo_id.clone(),
            revision_id: candidate.revision_id.clone(),
            generation: candidate.manifest_generation,
        };
        let generation_dir = self.index_path(&key);
        if !generation_dir.is_dir() {
            return Err(CoreError::NotFound(format!(
                "lexical: generation directory is absent: {}",
                generation_dir.display()
            )));
        }
        let observed = read_lexical_sealed_identity(&generation_dir)?;
        validate_lexical_sealed_identity(&observed, candidate)?;
        Ok((generation_dir, observed))
    }

    /// Apply one op to the writer; `true` when something must be committed.
    ///
    /// Every text document written here takes its text-authority doc id
    /// from `allocator`, which also keeps the document for the sidecar
    /// update, so the index and the text authority agree on the id by
    /// construction.
    pub(crate) fn apply_op(
        &self,
        writer: &IndexWriter,
        key: &GenKey,
        op: &LexicalChannelOp,
        allocator: &mut TextDocAllocator,
    ) -> Result<bool, CoreError> {
        match op {
            LexicalChannelOp::UpsertChunk(upsert) => {
                let candidate_id = upsert.chunk_id.as_str();
                let term = Term::from_field_text(self.fields.candidate_id, candidate_id);
                let _opstamp = writer.delete_term(term);
                let chunk = decode_chunk_payload(&upsert.payload)?;
                let doc_id = allocator.allocate(candidate_id, chunk.text.as_ref())?;
                let mut doc = TantivyDocument::new();
                doc.add_u64(self.fields.text_authority_doc_id, doc_id);
                doc.add_text(self.fields.candidate_id, candidate_id);
                doc.add_text(
                    self.fields.repo_id,
                    chunk.searchable_repo_id(&key.repo_id).as_str(),
                );
                doc.add_text(self.fields.revision_id, key.revision_id.as_str());
                doc.add_text(self.fields.doc_kind, TEXT_DOC_KIND);
                add_metadata_fields(
                    &self.fields,
                    &mut doc,
                    chunk.repo_relative_path.as_str(),
                    Some(chunk.language.as_str()),
                );
                doc.add_u64(self.fields.start_line, u64::from(chunk.start_line));
                doc.add_u64(self.fields.end_line, u64::from(chunk.end_line));
                add_snippet_field(&self.fields, &mut doc, chunk.derived_snippet());
                add_content_fields(&self.fields, &mut doc, chunk.text.as_ref());
                let _opstamp = writer
                    .add_document(doc)
                    .map_err(|err| CoreError::Storage(format!("lexical: add_document: {err}")))?;
                Ok(true)
            }
            LexicalChannelOp::UpsertSymbol(upsert) => {
                let candidate_id = upsert.symbol_id.as_str();
                let term = Term::from_field_text(self.fields.candidate_id, candidate_id);
                let _opstamp = writer.delete_term(term);
                let symbol = decode_symbol_payload(&upsert.payload)?;
                let mut doc = TantivyDocument::new();
                doc.add_text(self.fields.candidate_id, candidate_id);
                doc.add_text(self.fields.repo_id, key.repo_id.as_str());
                doc.add_text(self.fields.revision_id, key.revision_id.as_str());
                doc.add_text(self.fields.doc_kind, SYMBOL_DOC_KIND);
                add_metadata_fields(
                    &self.fields,
                    &mut doc,
                    symbol.repo_relative_path.as_str(),
                    Some(symbol.language.as_str()),
                );
                doc.add_u64(
                    self.fields.start_line,
                    u64::from(symbol.definition_span.line_start),
                );
                doc.add_u64(
                    self.fields.end_line,
                    u64::from(symbol.definition_span.line_end),
                );
                let snippet = match symbol.container_qualified_name.as_deref() {
                    Some(container) if !container.is_empty() => {
                        format!("{} {}", symbol.local_name.as_ref(), container)
                    }
                    _ => symbol.local_name.as_ref().to_string(),
                };
                add_snippet_field(&self.fields, &mut doc, &snippet);
                add_content_fields(&self.fields, &mut doc, &snippet);
                add_symbol_fields(&self.fields, &mut doc, &symbol);
                let _opstamp = writer
                    .add_document(doc)
                    .map_err(|err| CoreError::Storage(format!("lexical: add_document: {err}")))?;
                Ok(true)
            }
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_replace_scope_payload(&payload.payload)?;
                self.delete_scope_docs(writer, &scope.coverage.source.file)?;
                for chunk in &scope.chunks {
                    let doc_id =
                        allocator.allocate(chunk.chunk_id.as_str(), chunk.text.as_ref())?;
                    let mut doc = TantivyDocument::new();
                    doc.add_u64(self.fields.text_authority_doc_id, doc_id);
                    doc.add_text(self.fields.candidate_id, chunk.chunk_id.as_str());
                    doc.add_text(
                        self.fields.repo_id,
                        scope.coverage.source.file.source_repo_id.as_str(),
                    );
                    doc.add_text(self.fields.revision_id, key.revision_id.as_str());
                    crate::documents::add_source_fields(
                        &self.fields,
                        &mut doc,
                        &scope.coverage.source,
                    );
                    doc.add_text(self.fields.doc_kind, TEXT_DOC_KIND);
                    add_metadata_fields(
                        &self.fields,
                        &mut doc,
                        chunk.repo_relative_path.as_str(),
                        Some(chunk.language.as_str()),
                    );
                    crate::documents::add_chunk_provenance_fields(&self.fields, &mut doc, chunk);
                    doc.add_u64(self.fields.start_line, u64::from(chunk.start_line));
                    doc.add_u64(self.fields.end_line, u64::from(chunk.end_line));
                    add_snippet_field(&self.fields, &mut doc, chunk.derived_snippet());
                    add_content_fields(&self.fields, &mut doc, chunk.text.as_ref());
                    let _opstamp = writer.add_document(doc).map_err(|err| {
                        CoreError::Storage(format!("lexical: add_document: {err}"))
                    })?;
                }
                for symbol in &scope.symbols {
                    let mut doc = TantivyDocument::new();
                    doc.add_text(self.fields.candidate_id, symbol.symbol_id.as_str());
                    doc.add_text(
                        self.fields.repo_id,
                        scope.coverage.source.file.source_repo_id.as_str(),
                    );
                    doc.add_text(self.fields.revision_id, key.revision_id.as_str());
                    crate::documents::add_source_fields(
                        &self.fields,
                        &mut doc,
                        &scope.coverage.source,
                    );
                    doc.add_text(self.fields.doc_kind, SYMBOL_DOC_KIND);
                    add_metadata_fields(
                        &self.fields,
                        &mut doc,
                        symbol.repo_relative_path.as_str(),
                        Some(symbol.language.as_str()),
                    );
                    doc.add_u64(
                        self.fields.start_line,
                        u64::from(symbol.definition_span.line_start),
                    );
                    doc.add_u64(
                        self.fields.end_line,
                        u64::from(symbol.definition_span.line_end),
                    );
                    let snippet = match symbol.container_qualified_name.as_deref() {
                        Some(container) if !container.is_empty() => {
                            format!("{} {}", symbol.local_name.as_ref(), container)
                        }
                        _ => symbol.local_name.as_ref().to_string(),
                    };
                    add_snippet_field(&self.fields, &mut doc, &snippet);
                    add_content_fields(&self.fields, &mut doc, &snippet);
                    add_symbol_fields(&self.fields, &mut doc, symbol);
                    let _opstamp = writer.add_document(doc).map_err(|err| {
                        CoreError::Storage(format!("lexical: add_document: {err}"))
                    })?;
                }
                Ok(true)
            }
            LexicalChannelOp::TombstoneLexicalScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_tombstone_scope_payload(&payload.payload)?;
                self.delete_scope_docs(writer, &scope.file)?;
                Ok(true)
            }
            LexicalChannelOp::ClearLexicalSurface(payload) => {
                Ok(self.clear_surface_docs(writer, payload.surface))
            }
            // FullBundle/Seal carry no document-level effect (dispatcher's
            // ledger update observes Seal).
            LexicalChannelOp::FullBundle(_) => self.apply_snapshot_op(key, op),
            LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::ReplaceStructuralScope(_) => Ok(false),
        }
    }
}

/// The base generation declared by the first scope-bearing op in the batch.
///
/// `Ok(None)` means the batch replaces the generation outright and has no base
/// to inherit. Only the first scope-bearing op is consulted: a single batch
/// addresses one `(repo, revision, generation)` target and the dispatcher emits
/// one base per batch, so a later disagreement is a contract violation rather
/// than a second base to merge.
pub(crate) fn declared_delta_base_generation(
    ops: &[LexicalChannelOp],
) -> Result<Option<ManifestGeneration>, CoreError> {
    for op in ops {
        let base_generation = match op {
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                let (_mode, base_generation, _scope) =
                    decode_replace_scope_payload(&payload.payload)?;
                base_generation
            }
            LexicalChannelOp::TombstoneLexicalScope(payload) => {
                let (_mode, base_generation, _scope) =
                    decode_tombstone_scope_payload(&payload.payload)?;
                base_generation
            }
            LexicalChannelOp::ClearLexicalSurface(payload) => payload.base_generation,
            LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::UpsertChunk(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::ReplaceStructuralScope(_) => continue,
        };
        return Ok(base_generation);
    }
    Ok(None)
}

pub(crate) fn legacy_ops_for_batch(
    batch: &SearchCorpusIngestBatch,
    include_seal: bool,
) -> Result<Vec<LexicalChannelOp>, CoreError> {
    batch
        .validate_surface_mutations_v1()
        .map_err(|err| CoreError::InvalidContract(format!("lexical: {err}")))?;
    if batch.mode == BatchIngestMode::Delta && batch.base_generation.is_none() {
        return Err(CoreError::InvalidContract(
            "lexical: Delta search-corpus batch requires base_generation".to_string(),
        ));
    }
    let op_capacity = batch
        .bundle_payload
        .as_ref()
        .map_or(0usize, |_payload| 1usize)
        .saturating_add(
            batch
                .replace_scopes
                .len()
                .saturating_add(batch.clear_surfaces.len())
                .saturating_add(batch.tombstone_scopes.len())
                .saturating_add(usize::from(include_seal)),
        );
    let mut ops = Vec::with_capacity(op_capacity);
    if let Some(payload) = batch.bundle_payload.as_ref() {
        ops.push(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            payload: payload.clone(),
        }));
    }
    for surface in &batch.clear_surfaces {
        ops.push(LexicalChannelOp::ClearLexicalSurface(ClearLexicalSurface {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            base_generation: batch.base_generation,
            surface: *surface,
        }));
    }
    for scope in &batch.replace_scopes {
        ops.push(LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            payload: encode_cbor(
                &(batch.mode, batch.base_generation, scope.clone()),
                "replace lexical scope payload",
            )?,
        }));
    }
    for scope in &batch.tombstone_scopes {
        ops.push(LexicalChannelOp::TombstoneLexicalScope(
            TombstoneLexicalScope {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                payload: encode_cbor(
                    &(batch.mode, batch.base_generation, scope.clone()),
                    "tombstone lexical scope payload",
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
