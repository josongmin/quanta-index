//! Test producer state: one immutable source event per sealed generation.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, anyhow, ensure};
use quanta_index_contract::lex::{LanguageCode, SymbolRecord};
use quanta_index_contract::{
    BatchIngestMode, ChunkRecord, ManifestGeneration, RepoId, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusPublishOutcome, SearchCorpusReplaceScope, SearchCorpusTombstoneScope,
    SourceFileCoverage, SourceFileKey, SourceFileRevision, SourcePublicationBinding,
    SourcePublicationEvent, SymbolCoverage, source_event_payload_sha256,
    source_file_unit_set_sha256,
};
use sha2::{Digest as _, Sha256};

#[derive(Clone, Default)]
pub(super) struct FixtureFile {
    pub(super) chunks: Vec<ChunkRecord>,
    pub(super) symbols: Vec<SymbolRecord>,
    pub(super) symbols_requested: bool,
}

#[derive(Default)]
pub(super) struct CorpusPublicationState {
    files: BTreeMap<SourceFileKey, FixtureFile>,
    committed_files: BTreeMap<SourceFileKey, FixtureFile>,
    dirty: BTreeSet<SourceFileKey>,
    tombstones: BTreeSet<SourceFileKey>,
    metadata: Option<Vec<u8>>,
    semantic_fixture: Option<Vec<quanta_index_contract::SemanticSourceReplaceScopeV1>>,
    frozen: Option<SearchCorpusIngestBatch>,
    sealed_events:
        BTreeMap<(RepoId, RevisionId, ManifestGeneration), (String, SourcePublicationEvent)>,
    active_events: BTreeMap<RepoId, String>,
}

impl CorpusPublicationState {
    pub(super) fn active_event(&self, repo: &RepoId) -> Option<String> {
        self.active_events.get(repo).cloned()
    }

    pub(super) fn empty_successor(&self) -> Self {
        Self {
            active_events: self.active_events.clone(),
            committed_files: self.committed_files.clone(),
            ..Self::default()
        }
    }

    pub(super) fn ensure_mutable(&self) -> Result<()> {
        ensure!(
            self.frozen.is_none(),
            "harness source event is frozen after seal attempt; retry the original seal before editing"
        );
        Ok(())
    }

    pub(super) fn replace_chunks(
        &mut self,
        repo: &RepoId,
        records: Vec<ChunkRecord>,
    ) -> Result<()> {
        self.ensure_mutable()?;
        ensure!(
            self.semantic_fixture.is_none(),
            "explicit fixture already owns pending corpus"
        );
        let mut grouped = BTreeMap::<SourceFileKey, Vec<ChunkRecord>>::new();
        for record in records {
            let key = SourceFileKey {
                source_repo_id: record
                    .source_repo_id
                    .clone()
                    .unwrap_or_else(|| repo.clone()),
                repo_relative_path: record.repo_relative_path.clone(),
            };
            grouped.entry(key).or_default().push(record);
        }
        for (key, chunks) in grouped {
            match self.files.entry(key.clone()) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().chunks = chunks;
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let _inserted = entry.insert(FixtureFile {
                        chunks,
                        ..FixtureFile::default()
                    });
                }
            }
            let _was_tombstoned = self.tombstones.remove(&key);
            let _newly_dirty = self.dirty.insert(key);
        }
        Ok(())
    }

    pub(super) fn add_symbol(&mut self, repo: RepoId, symbol: SymbolRecord) -> Result<()> {
        self.ensure_mutable()?;
        ensure!(
            self.semantic_fixture.is_none(),
            "explicit fixture already owns pending corpus"
        );
        let key = SourceFileKey {
            source_repo_id: repo,
            repo_relative_path: symbol.repo_relative_path.clone(),
        };
        let file = self.files.entry(key.clone()).or_default();
        ensure!(
            !file
                .symbols
                .iter()
                .any(|existing| existing.symbol_id == symbol.symbol_id),
            "duplicate staged symbol identity"
        );
        file.symbols.push(symbol);
        file.symbols_requested = true;
        let _was_tombstoned = self.tombstones.remove(&key);
        let _newly_dirty = self.dirty.insert(key);
        Ok(())
    }

    pub(super) fn declare_all_staged_symbols_complete(&mut self) -> Result<()> {
        self.ensure_mutable()?;
        ensure!(
            self.semantic_fixture.is_none() && !self.files.is_empty(),
            "symbol extraction completion requires staged source files"
        );
        for (key, file) in &mut self.files {
            file.symbols_requested = true;
            let _newly_dirty = self.dirty.insert(key.clone());
        }
        Ok(())
    }

    pub(super) fn delete_path(&mut self, repo: RepoId, path: &str) -> Result<()> {
        self.ensure_mutable()?;
        ensure!(
            self.semantic_fixture.is_none(),
            "explicit fixture already owns pending corpus"
        );
        let mut keys = self
            .files
            .keys()
            .filter(|key| key.repo_relative_path.as_str() == path)
            .cloned()
            .collect::<Vec<_>>();
        if keys.is_empty() {
            keys.push(SourceFileKey {
                source_repo_id: repo,
                repo_relative_path: quanta_index_contract::RepoRelativePath::new(path),
            });
        }
        for key in keys {
            let _removed = self.files.remove(&key);
            let _was_dirty = self.dirty.remove(&key);
            let _new_tombstone = self.tombstones.insert(key);
        }
        Ok(())
    }

    pub(super) fn stage_fixture(
        &mut self,
        repo: &RepoId,
        chunks: Vec<ChunkRecord>,
        symbols: Vec<SymbolRecord>,
        semantic_scopes: Vec<quanta_index_contract::SemanticSourceReplaceScopeV1>,
    ) -> Result<()> {
        self.ensure_mutable()?;
        ensure!(
            self.dirty.is_empty() && self.tombstones.is_empty(),
            "explicit fixture must own the generation's complete pending corpus"
        );
        ensure!(self.semantic_fixture.is_none(), "fixture already staged");
        ensure!(
            self.committed_files.is_empty(),
            "explicit semantic fixture requires an initial corpus"
        );
        // Build off-state: a duplicate symbol or invalid fixture never partially stages.
        let mut staged = Self::default();
        staged.replace_chunks(repo, chunks)?;
        for symbol in symbols {
            staged.add_symbol(repo.clone(), symbol)?;
        }
        self.files.extend(staged.files);
        self.dirty.extend(staged.dirty);
        self.semantic_fixture = Some(semantic_scopes);
        Ok(())
    }

    pub(super) fn set_metadata(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.ensure_mutable()?;
        self.metadata = Some(bytes);
        Ok(())
    }

    pub(super) fn build_batch(
        &self,
        repo: RepoId,
        revision: RevisionId,
        generation: ManifestGeneration,
        mode: BatchIngestMode,
        base_generation: Option<ManifestGeneration>,
        event_id: String,
    ) -> Result<SearchCorpusIngestBatch> {
        let mut replace_scopes = Vec::new();
        let mut semantic_replace_scopes = Vec::new();
        let mut semantic_tombstone_scopes = Vec::new();
        for key in self.dirty.iter().chain(self.tombstones.iter()) {
            let file = self.files.get(key);
            let new_ids = file
                .into_iter()
                .flat_map(|file| &file.chunks)
                .map(|chunk| chunk.chunk_id.as_str())
                .collect::<BTreeSet<_>>();
            if let Some(previous) = self.committed_files.get(key) {
                for scope in super::semantic_source_scopes_for_chunk_records(&previous.chunks) {
                    if !new_ids.contains(scope.scope.owner_id.as_str()) {
                        semantic_tombstone_scopes.push(scope.scope);
                    }
                }
            }
            if let Some(file) = file {
                let scope = source_scope(key.clone(), revision.clone(), file)?;
                semantic_replace_scopes.extend(super::semantic_source_scopes_for_chunk_records(
                    &scope.chunks,
                ));
                replace_scopes.push(scope);
            }
        }
        let mut batch = SearchCorpusIngestBatch {
            source_event: SourcePublicationEvent {
                stream_id: "e2e-harness-source-v1".to_string(),
                event_id,
                expected_base_event_id: self.active_events.get(&repo).cloned(),
                payload_sha256: [0; 32],
            },
            repo_id: repo,
            revision_id: revision,
            generation,
            base_generation,
            manifest_digest: format!("lex-seal:{}", generation.get()),
            batch_digest: String::new(),
            mode,
            bundle_payload: self.metadata.clone(),
            clear_surfaces: Vec::new(),
            replace_scopes,
            tombstone_scopes: self
                .tombstones
                .iter()
                .cloned()
                .map(|file| SearchCorpusTombstoneScope { file })
                .collect(),
            semantic_replace_scopes: self
                .semantic_fixture
                .clone()
                .unwrap_or(semantic_replace_scopes),
            semantic_tombstone_scopes,
            seal: true,
        };
        batch.source_event.payload_sha256 = source_event_payload_sha256(&batch)?;
        quanta_index_ipc::stamp_batch_digest_v1(&mut batch)?;
        Ok(batch)
    }

    pub(super) fn frozen(&self) -> Option<&SearchCorpusIngestBatch> {
        self.frozen.as_ref()
    }
    pub(super) fn freeze(&mut self, batch: SearchCorpusIngestBatch) {
        self.frozen = Some(batch);
    }

    pub(super) fn ensure_publishable(&self, batch: &SearchCorpusIngestBatch) -> Result<()> {
        if let Some(frozen) = &self.frozen {
            ensure!(
                frozen == batch,
                "only the original frozen publication may be retried"
            );
        }
        ensure!(
            self.frozen.is_some() || (self.dirty.is_empty() && self.tombstones.is_empty()),
            "raw publication cannot discard a pending staged fixture"
        );
        Ok(())
    }

    pub(super) fn accept(
        &mut self,
        batch: &SearchCorpusIngestBatch,
        outcome: &SearchCorpusPublishOutcome,
    ) -> Result<()> {
        outcome
            .publication
            .validate_receipt(
                &SourcePublicationBinding::for_batch(batch),
                true,
                &outcome.receipt,
            )
            .map_err(anyhow::Error::msg)?;
        let receipt = &outcome.receipt;
        ensure!(
            usize::try_from(receipt.accepted_replace_scopes)? == batch.replace_scopes.len()
                && usize::try_from(receipt.accepted_tombstone_scopes)?
                    == batch.tombstone_scopes.len()
                && usize::try_from(receipt.accepted_clear_surfaces)? == batch.clear_surfaces.len()
                && usize::try_from(receipt.accepted_semantic_replace_scopes)?
                    == batch.semantic_replace_scopes.len()
                && usize::try_from(receipt.accepted_semantic_tombstone_scopes)?
                    == batch.semantic_tombstone_scopes.len(),
            "sealed corpus receipt mutation counts differ from the frozen source event"
        );
        // A validated raw publication can be the predecessor of a later staged delta.
        // Old replay observations never replace the current source-file baseline.
        if self.frozen.is_none()
            && self.dirty.is_empty()
            && self.tombstones.is_empty()
            && (outcome.publication.event.expected_base_event_id.as_ref()
                == self.active_events.get(&batch.repo_id)
                || Some(&outcome.publication.event.event_id)
                    == self.active_events.get(&batch.repo_id))
        {
            if batch.mode == BatchIngestMode::ReplaceGeneration {
                self.files.clear();
            }
            for scope in &batch.tombstone_scopes {
                let _removed = self.files.remove(&scope.file);
            }
            for scope in &batch.replace_scopes {
                let _previous = self.files.insert(
                    scope.coverage.source.file.clone(),
                    FixtureFile {
                        chunks: scope.chunks.clone(),
                        symbols: scope.symbols.clone(),
                        symbols_requested: matches!(
                            scope.coverage.symbols,
                            SymbolCoverage::Complete { .. }
                        ),
                    },
                );
            }
            self.committed_files = self.files.clone();
        }
        let _previous = self.sealed_events.insert(
            (
                outcome.publication.target.repo_id.clone(),
                outcome.publication.target.revision_id.clone(),
                outcome.publication.target.manifest_generation,
            ),
            (
                outcome.publication.target.manifest_digest.clone(),
                outcome.publication.event.clone(),
            ),
        );
        Ok(())
    }

    pub(super) fn finish_frozen(&mut self) {
        self.committed_files = self.files.clone();
        self.dirty.clear();
        self.tombstones.clear();
        self.metadata = None;
        self.semantic_fixture = None;
        self.frozen = None;
    }

    pub(super) fn observe_accepted_activation(
        &mut self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        manifest: &str,
    ) {
        if let Some((accepted_manifest, event)) =
            self.sealed_events
                .get(&(repo.clone(), revision.clone(), generation))
            && accepted_manifest == manifest
            && event.stream_id == "e2e-harness-source-v1"
            && event.expected_base_event_id.as_ref() == self.active_events.get(repo)
        {
            let _previous = self
                .active_events
                .insert(repo.clone(), event.event_id.clone());
        }
    }

    pub(super) fn activate(
        &mut self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        manifest: &str,
    ) -> Result<()> {
        let (accepted_manifest, event) = self
            .sealed_events
            .get(&(repo.clone(), revision.clone(), generation))
            .ok_or_else(|| anyhow!("activation lacks a producer-validated source event receipt"))?;
        ensure!(
            accepted_manifest == manifest,
            "activation manifest differs from accepted source event"
        );
        if event.stream_id == "e2e-harness-source-v1"
            && event.expected_base_event_id.as_ref() == self.active_events.get(repo)
        {
            let _previous = self
                .active_events
                .insert(repo.clone(), event.event_id.clone());
        }
        Ok(())
    }
}

pub(super) fn source_scope(
    key: SourceFileKey,
    revision: RevisionId,
    file: &FixtureFile,
) -> Result<SearchCorpusReplaceScope> {
    let mut source_bytes = Vec::new();
    let mut chunks = file.chunks.clone();
    let mut symbols = file.symbols.clone();
    for chunk in &mut chunks {
        chunk.start_byte = u32::try_from(source_bytes.len())?;
        source_bytes.extend_from_slice(chunk.text.as_bytes());
        chunk.end_byte = u32::try_from(source_bytes.len())?;
        source_bytes.push(b'\n');
    }
    for symbol in &mut symbols {
        symbol.definition_span.byte_start = u32::try_from(source_bytes.len())?;
        source_bytes.extend_from_slice(symbol.local_name.as_bytes());
        symbol.definition_span.byte_end = u32::try_from(source_bytes.len())?;
        source_bytes.push(b'\n');
    }
    let language: LanguageCode = chunks
        .first()
        .map(|chunk| chunk.language.clone())
        .or_else(|| symbols.first().map(|symbol| symbol.language.clone()))
        .ok_or_else(|| anyhow!("fixture source scope has no declared language"))?;
    let coverage = SourceFileCoverage {
        source: SourceFileRevision {
            file: key,
            revision_id: revision,
            source_sha256: Sha256::digest(&source_bytes).into(),
        },
        language,
        producer_policy_sha256: Sha256::digest(b"e2e-harness-explicit-synthetic-source-v1").into(),
        symbol_name_source_policy:
            quanta_index_contract::SymbolNameSourcePolicyV1::RawAsciiLocalName,
        unit_set_sha256: source_file_unit_set_sha256(&chunks, &symbols)?,
        text_admitted: !chunks.is_empty(),
        symbols: if file.symbols_requested {
            SymbolCoverage::Complete {
                symbol_count: u64::try_from(symbols.len())?,
            }
        } else {
            SymbolCoverage::NotRequested
        },
    };
    Ok(SearchCorpusReplaceScope {
        coverage,
        source_bytes,
        chunks,
        symbols,
    })
}

/// Build one SDK fixture scope from explicitly declared synthetic source rows.
///
/// This calls the same issuer as staged harness publication; SDK tests still
/// choose their own source event identity and predecessor explicitly.
pub fn fixture_source_scope_v1(
    file: SourceFileKey,
    revision: RevisionId,
    chunks: Vec<ChunkRecord>,
    symbols: Vec<SymbolRecord>,
) -> Result<SearchCorpusReplaceScope> {
    let symbols_requested = !symbols.is_empty();
    source_scope(
        file,
        revision,
        &FixtureFile {
            chunks,
            symbols,
            symbols_requested,
        },
    )
}

/// Single initial publication for socket-level fixtures with their own declared repo/revision.
/// Uses the same producer state and receipt validation as `E2eRuntime`.
pub struct SourceCorpusFixture {
    state: CorpusPublicationState,
    repo: RepoId,
    revision: RevisionId,
    generation: ManifestGeneration,
    event_id: String,
}

impl SourceCorpusFixture {
    #[must_use]
    pub fn new(
        repo: RepoId,
        revision: RevisionId,
        generation: ManifestGeneration,
        event_id: String,
    ) -> Self {
        Self {
            state: CorpusPublicationState::default(),
            repo,
            revision,
            generation,
            event_id,
        }
    }

    pub fn stage_fixture(
        &mut self,
        chunks: Vec<ChunkRecord>,
        symbols: Vec<SymbolRecord>,
        semantic_scopes: Vec<quanta_index_contract::SemanticSourceReplaceScopeV1>,
    ) -> Result<()> {
        self.state
            .stage_fixture(&self.repo, chunks, symbols, semantic_scopes)
    }

    pub fn replace_chunks(&mut self, chunks: Vec<ChunkRecord>) -> Result<()> {
        self.state.replace_chunks(&self.repo, chunks)
    }

    pub fn set_metadata(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.state.set_metadata(bytes)
    }

    pub fn delete_path(&mut self, path: &str) -> Result<()> {
        self.state.delete_path(self.repo.clone(), path)
    }

    pub fn frozen_batch(&mut self) -> Result<SearchCorpusIngestBatch> {
        if self.state.frozen().is_none() {
            let batch = self.state.build_batch(
                self.repo.clone(),
                self.revision.clone(),
                self.generation,
                BatchIngestMode::ReplaceGeneration,
                None,
                self.event_id.clone(),
            )?;
            self.state.freeze(batch);
        }
        self.state
            .frozen()
            .cloned()
            .ok_or_else(|| anyhow!("fixture publication was not frozen"))
    }

    pub fn publish(&mut self, socket: &std::path::Path, request_id: u64) -> Result<()> {
        use quanta_index_contract::{
            SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope,
            SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope,
        };
        let batch = self.frozen_batch()?;
        let response: SearchPlaneIngestIpcResponseEnvelope = quanta_index_ipc::send_request(
            socket,
            &SearchPlaneIngestIpcRequestEnvelope {
                request_id,
                payload: SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
            },
            quanta_index_ipc::ClientIoPolicy::default(),
        )?;
        ensure!(
            response.request_id == request_id,
            "fixture publication response id mismatch"
        );
        let SearchPlaneIngestIpcResponse::SearchCorpusReceipt(outcome) = response.payload else {
            return Err(anyhow!(
                "fixture publication rejected: {:?}",
                response.payload
            ));
        };
        ensure!(
            outcome.receipt.semantic_content.is_some(),
            "sealed fixture receipt lacks semantic roots"
        );
        if let Some(observation) = &outcome.observation {
            observation
                .validate_for(request_id, &batch, &outcome.publication, &outcome.receipt)
                .map_err(anyhow::Error::msg)?;
        }
        self.state.accept(&batch, &outcome)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "source_publication_tests.rs"]
mod tests;
