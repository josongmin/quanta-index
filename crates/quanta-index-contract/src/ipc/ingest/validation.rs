//! Storage-independent search-corpus batch admission.

use core::fmt;
use std::collections::BTreeSet;

use sha2::Digest as _;

use super::super::batch_body::{BATCH_DIGEST_TOKEN_LEN_V1, is_canonical_batch_digest_token_v1};
use super::{
    BatchIngestMode, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
    SearchCorpusSurfaceMutationConflictV1, SearchCorpusTombstoneScope, SearchScopeSurface,
};
use crate::ManifestGeneration;

/// A batch whose shape the contract refuses before any adapter observes it
/// (QI-BB-029).
///
/// These are the defects that used to be discovered one track at a time,
/// after the other track had already mutated: a mode/base pairing that one
/// backend accepts and the other refuses, a digest the activation identity
/// can never carry, a base that cannot precede its target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchCorpusBatchShapeErrorV1 {
    /// `ReplaceGeneration` names a base, or `Delta` names none.
    ModeBaseMismatch {
        mode: BatchIngestMode,
        base_generation: Option<ManifestGeneration>,
    },
    /// A delta base must be strictly older than its target.
    BaseNotOlderThanTarget {
        base_generation: ManifestGeneration,
        generation: ManifestGeneration,
    },
    /// `manifest_digest` is empty or not a bare printable ASCII token; the
    /// activation identity and the retention record both key on it and
    /// neither can carry such a value.
    DigestNotCanonical {
        field: &'static str,
    },
    /// `batch_digest` does not have the shape of a canonical batch digest
    /// (64 lowercase hex characters, see
    /// [`is_canonical_batch_digest_token_v1`]); the idempotency record keys
    /// on it and the search plane recomputes it from the body.
    BatchDigestNotCanonical,
    SourceEventInvalid,
    SourceEventPayloadMismatch,
    UnsealedSourceEvent,
}

impl fmt::Display for SearchCorpusBatchShapeErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceEventInvalid => formatter.write_str("invalid source publication event"),
            Self::SourceEventPayloadMismatch => formatter
                .write_str("source event payload hash does not bind the supplied mutations"),
            Self::UnsealedSourceEvent => {
                formatter.write_str("source-event publication requires a sealed batch")
            }
            Self::ModeBaseMismatch {
                mode,
                base_generation,
            } => write!(
                formatter,
                "batch mode {mode:?} does not admit base_generation={base_generation:?}: ReplaceGeneration takes no base, Delta requires one"
            ),
            Self::BaseNotOlderThanTarget {
                base_generation,
                generation,
            } => write!(
                formatter,
                "delta base generation {} must be older than target generation {}",
                base_generation.get(),
                generation.get()
            ),
            Self::DigestNotCanonical { field } => write!(
                formatter,
                "{field} must be a non-empty printable ASCII token without whitespace"
            ),
            Self::BatchDigestNotCanonical => write!(
                formatter,
                "batch_digest must be the canonical batch digest: {BATCH_DIGEST_TOKEN_LEN_V1} lowercase hex characters of SHA-256 over the canonical body"
            ),
        }
    }
}

impl std::error::Error for SearchCorpusBatchShapeErrorV1 {}

fn is_canonical_digest_token(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_graphic())
}

impl SearchCorpusIngestBatch {
    /// Validate everything about the batch that needs no storage access:
    /// mode/base shape, base ordering and the digests' shapes. The
    /// materializer runs this before taking any lock and the ingest
    /// dispatcher before it records durable intent, so a malformed batch
    /// changes zero bytes on either track and leaves no idempotency record.
    /// Whether `batch_digest` is *the* digest of this body is the
    /// dispatcher's recomputation to prove; this only checks its shape.
    pub fn validate_v1(&self) -> Result<(), SearchCorpusBatchShapeErrorV1> {
        match (self.mode, self.base_generation) {
            (BatchIngestMode::ReplaceGeneration, None) => {}
            (BatchIngestMode::Delta, Some(base_generation)) => {
                if base_generation >= self.generation {
                    return Err(SearchCorpusBatchShapeErrorV1::BaseNotOlderThanTarget {
                        base_generation,
                        generation: self.generation,
                    });
                }
            }
            (mode, base_generation) => {
                return Err(SearchCorpusBatchShapeErrorV1::ModeBaseMismatch {
                    mode,
                    base_generation,
                });
            }
        }
        if !is_canonical_digest_token(&self.manifest_digest) {
            return Err(SearchCorpusBatchShapeErrorV1::DigestNotCanonical {
                field: "manifest_digest",
            });
        }
        if !is_canonical_batch_digest_token_v1(&self.batch_digest) {
            return Err(SearchCorpusBatchShapeErrorV1::BatchDigestNotCanonical);
        }
        if !self.seal {
            return Err(SearchCorpusBatchShapeErrorV1::UnsealedSourceEvent);
        }
        self.source_event
            .validate()
            .map_err(|_invalid| SearchCorpusBatchShapeErrorV1::SourceEventInvalid)?;
        if crate::source_event_payload_sha256(self)
            .map_err(|_invalid| SearchCorpusBatchShapeErrorV1::SourceEventInvalid)?
            != self.source_event.payload_sha256
        {
            return Err(SearchCorpusBatchShapeErrorV1::SourceEventPayloadMismatch);
        }
        Ok(())
    }

    /// Validate the mutation authority before any adapter observes the batch.
    /// A whole-surface clear and a scope mutation on that surface cannot be
    /// ordered safely without creating producer-dependent semantics.
    pub fn validate_surface_mutations_v1(
        &self,
    ) -> Result<(), SearchCorpusSurfaceMutationConflictV1> {
        let clear_surfaces = validated_search_corpus_clear_surfaces_v1(&self.clear_surfaces)?;
        validate_lexical_file_mutations_v1(
            &self.clear_surfaces,
            &self.replace_scopes,
            &self.tombstone_scopes,
        )?;
        validate_search_corpus_semantic_scope_mutations_v1(self)?;
        validate_search_corpus_clear_disjoint_v1(self, &clear_surfaces)
    }
}

fn validated_search_corpus_clear_surfaces_v1(
    surfaces: &[SearchScopeSurface],
) -> Result<BTreeSet<SearchScopeSurface>, SearchCorpusSurfaceMutationConflictV1> {
    let mut clear_surfaces = BTreeSet::new();
    for surface in surfaces {
        if !clear_surfaces.insert(*surface) {
            return Err(SearchCorpusSurfaceMutationConflictV1::DuplicateClear(
                *surface,
            ));
        }
    }
    if !surfaces
        .windows(2)
        .all(|pair| matches!(pair, [left, right] if left < right))
    {
        return Err(SearchCorpusSurfaceMutationConflictV1::NonCanonicalClearOrder);
    }
    Ok(clear_surfaces)
}

/// Pure admission for typed batches and fully decoded raw channel operations.
/// Validate the entire list before preparing a generation or acquiring a writer.
pub fn validate_lexical_file_mutations_v1(
    clear: &[SearchScopeSurface],
    replace: &[SearchCorpusReplaceScope],
    tombstone: &[SearchCorpusTombstoneScope],
) -> Result<(), SearchCorpusSurfaceMutationConflictV1> {
    let clear_surfaces = validated_search_corpus_clear_surfaces_v1(clear)?;
    let mut replace_scope_keys = BTreeSet::new();
    let mut candidate_ids = BTreeSet::new();
    for scope in replace {
        scope
            .coverage
            .validate()
            .map_err(|_invalid| SearchCorpusSurfaceMutationConflictV1::InvalidCoverage)?;
        if <[u8; 32]>::from(sha2::Sha256::digest(&scope.source_bytes))
            != scope.coverage.source.source_sha256
        {
            return Err(SearchCorpusSurfaceMutationConflictV1::SourceBytesDigestMismatch);
        }
        let key = &scope.coverage.source.file;
        if !replace_scope_keys.insert(key) {
            return Err(
                SearchCorpusSurfaceMutationConflictV1::DuplicateReplaceScope(
                    SearchScopeSurface::Chunk,
                ),
            );
        }
        for chunk in &scope.chunks {
            if chunk.repo_relative_path != key.repo_relative_path {
                return Err(SearchCorpusSurfaceMutationConflictV1::RecordPathMismatch(
                    SearchScopeSurface::Chunk,
                ));
            }
            if chunk
                .source_repo_id
                .as_ref()
                .is_some_and(|repo| repo != &key.source_repo_id)
            {
                return Err(SearchCorpusSurfaceMutationConflictV1::RecordSourceMismatch);
            }
            if chunk.language != scope.coverage.language {
                return Err(SearchCorpusSurfaceMutationConflictV1::RecordLanguageMismatch);
            }
            let text_bytes = u64::try_from(chunk.text.len())
                .map_err(|_overflow| SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange)?;
            if chunk.end_byte.checked_sub(chunk.start_byte).map(u64::from) != Some(text_bytes)
                || chunk.end_line < chunk.start_line
            {
                return Err(SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange);
            }
            let start = usize::try_from(chunk.start_byte)
                .map_err(|_overflow| SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange)?;
            let end = usize::try_from(chunk.end_byte)
                .map_err(|_overflow| SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange)?;
            if scope.source_bytes.get(start..end) != Some(chunk.text.as_bytes()) {
                return Err(SearchCorpusSurfaceMutationConflictV1::ChunkSourceMismatch);
            }
            if !candidate_ids.insert(chunk.chunk_id.as_str()) {
                return Err(SearchCorpusSurfaceMutationConflictV1::DuplicateCandidateId);
            }
        }
        for symbol in &scope.symbols {
            if symbol.repo_relative_path != key.repo_relative_path
                || symbol.definition_span.path.as_ref() != key.repo_relative_path.as_str()
            {
                return Err(SearchCorpusSurfaceMutationConflictV1::RecordPathMismatch(
                    SearchScopeSurface::Symbol,
                ));
            }
            if symbol.language != scope.coverage.language {
                return Err(SearchCorpusSurfaceMutationConflictV1::RecordLanguageMismatch);
            }
            if symbol.definition_span.byte_end < symbol.definition_span.byte_start
                || usize::try_from(symbol.definition_span.byte_end)
                    .map_or(true, |end| end > scope.source_bytes.len())
                || symbol.definition_span.line_end < symbol.definition_span.line_start
            {
                return Err(SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange);
            }
            if scope.coverage.symbol_name_source_policy
                == crate::SymbolNameSourcePolicyV1::RawAsciiLocalName
                && symbol.local_name.is_ascii()
            {
                let start =
                    usize::try_from(symbol.definition_span.byte_start).map_err(|_overflow| {
                        SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange
                    })?;
                let end =
                    usize::try_from(symbol.definition_span.byte_end).map_err(|_overflow| {
                        SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange
                    })?;
                let span_bytes = scope
                    .source_bytes
                    .get(start..end)
                    .ok_or(SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange)?;
                if memchr::memmem::find(span_bytes, symbol.local_name.as_bytes()).is_none() {
                    return Err(SearchCorpusSurfaceMutationConflictV1::SymbolNameSourceMismatch);
                }
            }
            if !candidate_ids.insert(symbol.symbol_id.as_str()) {
                return Err(SearchCorpusSurfaceMutationConflictV1::DuplicateCandidateId);
            }
        }
        let count = u64::try_from(scope.symbols.len())
            .map_err(|_overflow| SearchCorpusSurfaceMutationConflictV1::CoverageUnitMismatch)?;
        let symbols_match = match scope.coverage.symbols {
            crate::SymbolCoverage::Complete { symbol_count } => symbol_count == count,
            crate::SymbolCoverage::NotRequested
            | crate::SymbolCoverage::Unsupported
            | crate::SymbolCoverage::ParseFailed
            | crate::SymbolCoverage::ProducerFailed => count == 0,
        };
        if !symbols_match
            || (!scope.coverage.text_admitted && !scope.chunks.is_empty())
            || crate::source_file_unit_set_sha256(&scope.chunks, &scope.symbols)
                .map_err(|_invalid| SearchCorpusSurfaceMutationConflictV1::CoverageUnitMismatch)?
                != scope.coverage.unit_set_sha256
        {
            return Err(SearchCorpusSurfaceMutationConflictV1::CoverageUnitMismatch);
        }
    }
    let mut tombstone_scope_keys = BTreeSet::new();
    for scope in tombstone {
        scope
            .file
            .validate()
            .map_err(|_invalid| SearchCorpusSurfaceMutationConflictV1::InvalidCoverage)?;
        let key = &scope.file;
        if !tombstone_scope_keys.insert(key) {
            return Err(
                SearchCorpusSurfaceMutationConflictV1::DuplicateTombstoneScope(
                    SearchScopeSurface::Chunk,
                ),
            );
        }
        if replace_scope_keys.contains(&key) {
            return Err(SearchCorpusSurfaceMutationConflictV1::ReplaceAndTombstone(
                SearchScopeSurface::Chunk,
            ));
        }
    }
    for surface in &clear_surfaces {
        let deletes_lexical_kind = matches!(
            surface,
            SearchScopeSurface::Chunk | SearchScopeSurface::Symbol
        );
        if deletes_lexical_kind && !replace.is_empty() {
            return Err(SearchCorpusSurfaceMutationConflictV1::ClearAndReplace(
                *surface,
            ));
        }
        if deletes_lexical_kind && !tombstone.is_empty() {
            return Err(SearchCorpusSurfaceMutationConflictV1::ClearAndTombstone(
                *surface,
            ));
        }
    }
    Ok(())
}

fn validate_search_corpus_semantic_scope_mutations_v1(
    batch: &SearchCorpusIngestBatch,
) -> Result<(), SearchCorpusSurfaceMutationConflictV1> {
    let mut replace_scope_keys = BTreeSet::new();
    for scope in &batch.semantic_replace_scopes {
        let key = (
            scope.scope.corpus_kind.as_code_str(),
            scope.scope.owner_kind.as_code_str(),
            scope.scope.owner_id.as_str(),
        );
        let surface = SearchScopeSurface::for_semantic_owner_v1(
            scope.scope.owner_kind,
            scope.scope.corpus_kind,
        );
        if !replace_scope_keys.insert(key) {
            return Err(SearchCorpusSurfaceMutationConflictV1::DuplicateReplaceScope(surface));
        }
    }
    let mut tombstone_scope_keys = BTreeSet::new();
    for scope in &batch.semantic_tombstone_scopes {
        let key = (
            scope.corpus_kind.as_code_str(),
            scope.owner_kind.as_code_str(),
            scope.owner_id.as_str(),
        );
        let surface =
            SearchScopeSurface::for_semantic_owner_v1(scope.owner_kind, scope.corpus_kind);
        if !tombstone_scope_keys.insert(key) {
            return Err(SearchCorpusSurfaceMutationConflictV1::DuplicateTombstoneScope(surface));
        }
        if replace_scope_keys.contains(&key) {
            return Err(SearchCorpusSurfaceMutationConflictV1::ReplaceAndTombstone(
                surface,
            ));
        }
    }
    Ok(())
}

fn validate_search_corpus_clear_disjoint_v1(
    batch: &SearchCorpusIngestBatch,
    clear_surfaces: &BTreeSet<SearchScopeSurface>,
) -> Result<(), SearchCorpusSurfaceMutationConflictV1> {
    for surface in batch.semantic_replace_scopes.iter().map(|scope| {
        SearchScopeSurface::for_semantic_owner_v1(scope.scope.owner_kind, scope.scope.corpus_kind)
    }) {
        if clear_surfaces.contains(&surface) {
            return Err(SearchCorpusSurfaceMutationConflictV1::ClearAndReplace(
                surface,
            ));
        }
    }
    for surface in batch
        .semantic_tombstone_scopes
        .iter()
        .map(|scope| SearchScopeSurface::for_semantic_owner_v1(scope.owner_kind, scope.corpus_kind))
    {
        if clear_surfaces.contains(&surface) {
            return Err(SearchCorpusSurfaceMutationConflictV1::ClearAndTombstone(
                surface,
            ));
        }
    }
    Ok(())
}
