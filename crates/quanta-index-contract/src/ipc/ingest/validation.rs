//! Storage-independent search-corpus batch admission.

use core::fmt;
use std::collections::BTreeSet;

use sha2::Digest as _;

use super::super::batch_body::{BATCH_DIGEST_TOKEN_LEN_V1, is_canonical_batch_digest_token_v1};
use super::{
    BatchIngestMode, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
    SearchCorpusSurfaceMutationConflictV1, SearchCorpusTombstoneScope, SearchScopeSurface,
};
use crate::lex::SymbolRecord;
use crate::{ChunkRecord, ManifestGeneration, SourceFileKey};

#[path = "validation/native_scratch_v3.rs"]
mod native_scratch_v3;
pub use native_scratch_v3::CorpusSurfaceOrderScratchV3;

trait MutationSetPolicyV3<'a> {
    fn replace_file(&mut self, key: &'a SourceFileKey) -> bool;
    fn tombstone_file(&mut self, key: &'a SourceFileKey) -> bool;
    fn replaced_file(&self, key: &'a SourceFileKey) -> bool;
    fn candidate(&mut self, id: &'a str) -> bool;
    fn replace_semantic(&mut self, key: (&'a str, &'a str, &'a str)) -> bool;
    fn tombstone_semantic(&mut self, key: (&'a str, &'a str, &'a str)) -> bool;
    fn replaced_semantic(&self, key: (&'a str, &'a str, &'a str)) -> bool;
}

#[derive(Default)]
struct OrdinaryMutationSetsV3<'a> {
    replace_files: BTreeSet<&'a SourceFileKey>,
    tombstone_files: BTreeSet<&'a SourceFileKey>,
    candidates: BTreeSet<&'a str>,
    replace_semantic: BTreeSet<(&'a str, &'a str, &'a str)>,
    tombstone_semantic: BTreeSet<(&'a str, &'a str, &'a str)>,
}

impl<'a> MutationSetPolicyV3<'a> for OrdinaryMutationSetsV3<'a> {
    fn replace_file(&mut self, key: &'a SourceFileKey) -> bool {
        self.replace_files.insert(key)
    }
    fn tombstone_file(&mut self, key: &'a SourceFileKey) -> bool {
        self.tombstone_files.insert(key)
    }
    fn replaced_file(&self, key: &'a SourceFileKey) -> bool {
        self.replace_files.contains(key)
    }
    fn candidate(&mut self, id: &'a str) -> bool {
        self.candidates.insert(id)
    }
    fn replace_semantic(&mut self, key: (&'a str, &'a str, &'a str)) -> bool {
        self.replace_semantic.insert(key)
    }
    fn tombstone_semantic(&mut self, key: (&'a str, &'a str, &'a str)) -> bool {
        self.tombstone_semantic.insert(key)
    }
    fn replaced_semantic(&self, key: (&'a str, &'a str, &'a str)) -> bool {
        self.replace_semantic.contains(&key)
    }
}

/// Constructed only after the borrowed scratch certifies an exact sorted
/// permutation of every input key and cross-category disjointness.
struct CertifiedMutationSetsV3;

impl<'a> MutationSetPolicyV3<'a> for CertifiedMutationSetsV3 {
    fn replace_file(&mut self, _: &'a SourceFileKey) -> bool {
        true
    }
    fn tombstone_file(&mut self, _: &'a SourceFileKey) -> bool {
        true
    }
    fn replaced_file(&self, _: &'a SourceFileKey) -> bool {
        false
    }
    fn candidate(&mut self, _: &'a str) -> bool {
        true
    }
    fn replace_semantic(&mut self, _: (&'a str, &'a str, &'a str)) -> bool {
        true
    }
    fn tombstone_semantic(&mut self, _: (&'a str, &'a str, &'a str)) -> bool {
        true
    }
    fn replaced_semantic(&self, _: (&'a str, &'a str, &'a str)) -> bool {
        false
    }
}

#[derive(Debug)]
pub enum SearchCorpusValidationProgressError<E, V> {
    Invalid(V),
    Progress(E),
}

impl<E, V> From<V> for SearchCorpusValidationProgressError<E, V> {
    fn from(error: V) -> Self {
        Self::Invalid(error)
    }
}

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
        match self.validate_with_progress_v1(&mut || Ok::<_, std::convert::Infallible>(())) {
            Ok(()) => Ok(()),
            Err(SearchCorpusValidationProgressError::Invalid(error)) => Err(error),
            Err(SearchCorpusValidationProgressError::Progress(never)) => match never {},
        }
    }

    pub fn validate_with_progress_v1<E, F>(
        &self,
        progress: &mut F,
    ) -> Result<(), SearchCorpusValidationProgressError<E, SearchCorpusBatchShapeErrorV1>>
    where
        F: FnMut() -> Result<(), E>,
    {
        self.validate_with_work_v3(progress, &mut |_| Ok(()))
    }

    /// Same shape and source-event commitment with actual encoded hash-write
    /// work admitted by the caller before each SHA-256 update.
    pub fn validate_with_work_v3<E, F, W>(
        &self,
        progress: &mut F,
        work: &mut W,
    ) -> Result<(), SearchCorpusValidationProgressError<E, SearchCorpusBatchShapeErrorV1>>
    where
        F: FnMut() -> Result<(), E>,
        W: FnMut(usize) -> Result<(), E>,
    {
        progress().map_err(SearchCorpusValidationProgressError::Progress)?;
        match (self.mode, self.base_generation) {
            (BatchIngestMode::ReplaceGeneration, None) => {}
            (BatchIngestMode::Delta, Some(base_generation)) => {
                if base_generation >= self.generation {
                    return Err(SearchCorpusBatchShapeErrorV1::BaseNotOlderThanTarget {
                        base_generation,
                        generation: self.generation,
                    }
                    .into());
                }
            }
            (mode, base_generation) => {
                return Err(SearchCorpusBatchShapeErrorV1::ModeBaseMismatch {
                    mode,
                    base_generation,
                }
                .into());
            }
        }
        if !is_canonical_digest_token(&self.manifest_digest) {
            return Err(SearchCorpusBatchShapeErrorV1::DigestNotCanonical {
                field: "manifest_digest",
            }
            .into());
        }
        if !is_canonical_batch_digest_token_v1(&self.batch_digest) {
            return Err(SearchCorpusBatchShapeErrorV1::BatchDigestNotCanonical.into());
        }
        if !self.seal {
            return Err(SearchCorpusBatchShapeErrorV1::UnsealedSourceEvent.into());
        }
        self.source_event
            .validate()
            .map_err(|_invalid| SearchCorpusBatchShapeErrorV1::SourceEventInvalid)?;
        let event_digest =
            super::payload_digest::source_event_payload_sha256_with_work_v3(self, progress, work)
                .map_err(|error| match error {
                crate::SourceCoverageProgressError::Invalid(_invalid) => {
                    SearchCorpusValidationProgressError::Invalid(
                        SearchCorpusBatchShapeErrorV1::SourceEventInvalid,
                    )
                }
                crate::SourceCoverageProgressError::Progress(interrupted) => {
                    SearchCorpusValidationProgressError::Progress(interrupted)
                }
            })?;
        if event_digest != self.source_event.payload_sha256 {
            return Err(SearchCorpusBatchShapeErrorV1::SourceEventPayloadMismatch.into());
        }
        progress().map_err(SearchCorpusValidationProgressError::Progress)?;
        Ok(())
    }

    /// Validate the mutation authority before any adapter observes the batch.
    /// A whole-surface clear and a scope mutation on that surface cannot be
    /// ordered safely without creating producer-dependent semantics.
    pub fn validate_surface_mutations_v1(
        &self,
    ) -> Result<(), SearchCorpusSurfaceMutationConflictV1> {
        match self.validate_surface_mutations_with_progress_v1(&mut || {
            Ok::<_, std::convert::Infallible>(())
        }) {
            Ok(()) => Ok(()),
            Err(SearchCorpusValidationProgressError::Invalid(error)) => Err(error),
            Err(SearchCorpusValidationProgressError::Progress(never)) => match never {},
        }
    }

    pub fn validate_surface_mutations_with_progress_v1<E, F>(
        &self,
        progress: &mut F,
    ) -> Result<(), SearchCorpusValidationProgressError<E, SearchCorpusSurfaceMutationConflictV1>>
    where
        F: FnMut() -> Result<(), E>,
    {
        let mut sets = OrdinaryMutationSetsV3::default();
        validate_surface_mutations_with_policy_v3(
            self,
            progress,
            &mut sets,
            &mut |_| Ok(()),
            &mut |chunks, symbols, progress| {
                crate::source_file_unit_set_sha256_with_progress(chunks, symbols, progress)
            },
        )
    }

    /// Execute the ordinary record predicates with certified caller-owned
    /// sorted scratch. No `BTreeSet` or unit-set index `Vec` is born here; the
    /// caller must fund scratch and the per-file digest callback beforehand.
    pub fn validate_surface_mutations_with_borrowed_scratch_v3<E, F, W, U>(
        &self,
        scratch: &mut CorpusSurfaceOrderScratchV3<'_>,
        progress: &mut F,
        work: &mut W,
        unit_hash: &mut U,
    ) -> Result<(), SearchCorpusValidationProgressError<E, SearchCorpusSurfaceMutationConflictV1>>
    where
        F: FnMut() -> Result<(), E>,
        W: FnMut(usize) -> Result<(), E>,
        U: FnMut(
            &[ChunkRecord],
            &[SymbolRecord],
            &mut F,
        ) -> Result<[u8; 32], crate::SourceCoverageProgressError<E>>,
    {
        progress().map_err(SearchCorpusValidationProgressError::Progress)?;
        let _validated_clear_surfaces =
            validated_search_corpus_clear_surfaces_v1(&self.clear_surfaces)?;
        native_scratch_v3::certify_v3(self, scratch, progress, work)?;
        let mut certified = CertifiedMutationSetsV3;
        validate_surface_mutations_with_policy_v3(self, progress, &mut certified, work, unit_hash)
    }
}

fn validate_surface_mutations_with_policy_v3<'a, E, F, P, W, U>(
    batch: &'a SearchCorpusIngestBatch,
    progress: &mut F,
    sets: &mut P,
    work: &mut W,
    unit_hash: &mut U,
) -> Result<(), SearchCorpusValidationProgressError<E, SearchCorpusSurfaceMutationConflictV1>>
where
    F: FnMut() -> Result<(), E>,
    P: MutationSetPolicyV3<'a>,
    W: FnMut(usize) -> Result<(), E>,
    U: FnMut(
        &[ChunkRecord],
        &[SymbolRecord],
        &mut F,
    ) -> Result<[u8; 32], crate::SourceCoverageProgressError<E>>,
{
    progress().map_err(SearchCorpusValidationProgressError::Progress)?;
    let clear_surfaces = validated_search_corpus_clear_surfaces_v1(&batch.clear_surfaces)?;
    validate_lexical_file_mutations_with_policy_v3(
        &batch.clear_surfaces,
        &batch.replace_scopes,
        &batch.tombstone_scopes,
        progress,
        sets,
        work,
        unit_hash,
    )?;
    progress().map_err(SearchCorpusValidationProgressError::Progress)?;
    validate_search_corpus_semantic_scope_mutations_with_policy_v3(batch, progress, sets)?;
    progress().map_err(SearchCorpusValidationProgressError::Progress)?;
    validate_search_corpus_clear_disjoint_with_progress_v1(batch, clear_surfaces, progress)?;
    progress().map_err(SearchCorpusValidationProgressError::Progress)?;
    Ok(())
}

fn validated_search_corpus_clear_surfaces_v1(
    surfaces: &[SearchScopeSurface],
) -> Result<&[SearchScopeSurface], SearchCorpusSurfaceMutationConflictV1> {
    // SearchScopeSurface is a closed enum. A duplicate-free prefix is bounded
    // by its variants; the next repeated member is refused before order checks.
    // Borrow the canonical input instead of allocating another tree backing.
    for (index, surface) in surfaces.iter().enumerate() {
        let prefix = surfaces
            .get(..index)
            .ok_or(SearchCorpusSurfaceMutationConflictV1::NonCanonicalClearOrder)?;
        if prefix.contains(surface) {
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
    Ok(surfaces)
}

/// Pure admission for typed batches and fully decoded raw channel operations.
/// Validate the entire list before preparing a generation or acquiring a writer.
pub fn validate_lexical_file_mutations_v1(
    clear: &[SearchScopeSurface],
    replace: &[SearchCorpusReplaceScope],
    tombstone: &[SearchCorpusTombstoneScope],
) -> Result<(), SearchCorpusSurfaceMutationConflictV1> {
    match validate_lexical_file_mutations_with_progress_v1(clear, replace, tombstone, &mut || {
        Ok::<_, std::convert::Infallible>(())
    }) {
        Ok(()) => Ok(()),
        Err(SearchCorpusValidationProgressError::Invalid(error)) => Err(error),
        Err(SearchCorpusValidationProgressError::Progress(never)) => match never {},
    }
}

pub fn validate_lexical_file_mutations_with_progress_v1<E, F>(
    clear: &[SearchScopeSurface],
    replace: &[SearchCorpusReplaceScope],
    tombstone: &[SearchCorpusTombstoneScope],
    progress: &mut F,
) -> Result<(), SearchCorpusValidationProgressError<E, SearchCorpusSurfaceMutationConflictV1>>
where
    F: FnMut() -> Result<(), E>,
{
    let mut sets = OrdinaryMutationSetsV3::default();
    validate_lexical_file_mutations_with_policy_v3(
        clear,
        replace,
        tombstone,
        progress,
        &mut sets,
        &mut |_| Ok(()),
        &mut |chunks, symbols, progress| {
            crate::source_file_unit_set_sha256_with_progress(chunks, symbols, progress)
        },
    )
}

fn validate_lexical_file_mutations_with_policy_v3<'a, E, F, P, W, U>(
    clear: &[SearchScopeSurface],
    replace: &'a [SearchCorpusReplaceScope],
    tombstone: &'a [SearchCorpusTombstoneScope],
    progress: &mut F,
    sets: &mut P,
    work: &mut W,
    unit_hash: &mut U,
) -> Result<(), SearchCorpusValidationProgressError<E, SearchCorpusSurfaceMutationConflictV1>>
where
    F: FnMut() -> Result<(), E>,
    P: MutationSetPolicyV3<'a>,
    W: FnMut(usize) -> Result<(), E>,
    U: FnMut(
        &[ChunkRecord],
        &[SymbolRecord],
        &mut F,
    ) -> Result<[u8; 32], crate::SourceCoverageProgressError<E>>,
{
    macro_rules! invalid {
        ($error:expr $(,)?) => {
            return Err(SearchCorpusValidationProgressError::Invalid($error))
        };
    }

    progress().map_err(SearchCorpusValidationProgressError::Progress)?;
    let clear_surfaces = validated_search_corpus_clear_surfaces_v1(clear)?;
    for (scope_index, scope) in replace.iter().enumerate() {
        if scope_index != 0 && scope_index % 32 == 0 {
            progress().map_err(SearchCorpusValidationProgressError::Progress)?;
        }
        scope
            .coverage
            .validate()
            .map_err(|_invalid| SearchCorpusSurfaceMutationConflictV1::InvalidCoverage)?;
        let mut source_hasher = sha2::Sha256::new();
        for bytes in scope.source_bytes.chunks(64 * 1024) {
            progress().map_err(SearchCorpusValidationProgressError::Progress)?;
            work(bytes.len()).map_err(SearchCorpusValidationProgressError::Progress)?;
            source_hasher.update(bytes);
        }
        if <[u8; 32]>::from(source_hasher.finalize()) != scope.coverage.source.source_sha256 {
            invalid!(SearchCorpusSurfaceMutationConflictV1::SourceBytesDigestMismatch);
        }
        let key = &scope.coverage.source.file;
        if !sets.replace_file(key) {
            invalid!(
                SearchCorpusSurfaceMutationConflictV1::DuplicateReplaceScope(
                    SearchScopeSurface::Chunk,
                ),
            );
        }
        for (chunk_index, chunk) in scope.chunks.iter().enumerate() {
            if chunk_index != 0 && chunk_index % 32 == 0 {
                progress().map_err(SearchCorpusValidationProgressError::Progress)?;
            }
            if chunk.repo_relative_path != key.repo_relative_path {
                invalid!(SearchCorpusSurfaceMutationConflictV1::RecordPathMismatch(
                    SearchScopeSurface::Chunk,
                ));
            }
            if chunk
                .source_repo_id
                .as_ref()
                .is_some_and(|repo| repo != &key.source_repo_id)
            {
                invalid!(SearchCorpusSurfaceMutationConflictV1::RecordSourceMismatch);
            }
            if chunk.language != scope.coverage.language {
                invalid!(SearchCorpusSurfaceMutationConflictV1::RecordLanguageMismatch);
            }
            let text_bytes = u64::try_from(chunk.text.len())
                .map_err(|_overflow| SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange)?;
            if chunk.end_byte.checked_sub(chunk.start_byte).map(u64::from) != Some(text_bytes)
                || chunk.end_line < chunk.start_line
            {
                invalid!(SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange);
            }
            let start = usize::try_from(chunk.start_byte)
                .map_err(|_overflow| SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange)?;
            let end = usize::try_from(chunk.end_byte)
                .map_err(|_overflow| SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange)?;
            if scope.source_bytes.get(start..end) != Some(chunk.text.as_bytes()) {
                invalid!(SearchCorpusSurfaceMutationConflictV1::ChunkSourceMismatch);
            }
            if !sets.candidate(chunk.chunk_id.as_str()) {
                invalid!(SearchCorpusSurfaceMutationConflictV1::DuplicateCandidateId);
            }
        }
        for (symbol_index, symbol) in scope.symbols.iter().enumerate() {
            if symbol_index != 0 && symbol_index % 32 == 0 {
                progress().map_err(SearchCorpusValidationProgressError::Progress)?;
            }
            if symbol.repo_relative_path != key.repo_relative_path
                || symbol.definition_span.path.as_ref() != key.repo_relative_path.as_str()
            {
                invalid!(SearchCorpusSurfaceMutationConflictV1::RecordPathMismatch(
                    SearchScopeSurface::Symbol,
                ));
            }
            if symbol.language != scope.coverage.language {
                invalid!(SearchCorpusSurfaceMutationConflictV1::RecordLanguageMismatch);
            }
            if symbol.definition_span.byte_end < symbol.definition_span.byte_start
                || usize::try_from(symbol.definition_span.byte_end)
                    .map_or(true, |end| end > scope.source_bytes.len())
                || symbol.definition_span.line_end < symbol.definition_span.line_start
            {
                invalid!(SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange);
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
                    invalid!(SearchCorpusSurfaceMutationConflictV1::SymbolNameSourceMismatch);
                }
            }
            if !sets.candidate(symbol.symbol_id.as_str()) {
                invalid!(SearchCorpusSurfaceMutationConflictV1::DuplicateCandidateId);
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
            || unit_hash(&scope.chunks, &scope.symbols, progress).map_err(|error| match error {
                crate::SourceCoverageProgressError::Invalid(_invalid) => {
                    SearchCorpusValidationProgressError::Invalid(
                        SearchCorpusSurfaceMutationConflictV1::CoverageUnitMismatch,
                    )
                }
                crate::SourceCoverageProgressError::Progress(interrupted) => {
                    SearchCorpusValidationProgressError::Progress(interrupted)
                }
            })? != scope.coverage.unit_set_sha256
        {
            invalid!(SearchCorpusSurfaceMutationConflictV1::CoverageUnitMismatch);
        }
    }
    for (scope_index, scope) in tombstone.iter().enumerate() {
        if scope_index != 0 && scope_index % 32 == 0 {
            progress().map_err(SearchCorpusValidationProgressError::Progress)?;
        }
        scope
            .file
            .validate()
            .map_err(|_invalid| SearchCorpusSurfaceMutationConflictV1::InvalidCoverage)?;
        let key = &scope.file;
        if !sets.tombstone_file(key) {
            invalid!(
                SearchCorpusSurfaceMutationConflictV1::DuplicateTombstoneScope(
                    SearchScopeSurface::Chunk,
                ),
            );
        }
        if sets.replaced_file(key) {
            invalid!(SearchCorpusSurfaceMutationConflictV1::ReplaceAndTombstone(
                SearchScopeSurface::Chunk,
            ));
        }
    }
    for surface in clear_surfaces {
        let deletes_lexical_kind = matches!(
            surface,
            SearchScopeSurface::Chunk | SearchScopeSurface::Symbol
        );
        if deletes_lexical_kind && !replace.is_empty() {
            invalid!(SearchCorpusSurfaceMutationConflictV1::ClearAndReplace(
                *surface,
            ));
        }
        if deletes_lexical_kind && !tombstone.is_empty() {
            invalid!(SearchCorpusSurfaceMutationConflictV1::ClearAndTombstone(
                *surface,
            ));
        }
    }
    progress().map_err(SearchCorpusValidationProgressError::Progress)?;
    Ok(())
}

fn validate_search_corpus_semantic_scope_mutations_with_policy_v3<'a, E, F, P>(
    batch: &'a SearchCorpusIngestBatch,
    progress: &mut F,
    sets: &mut P,
) -> Result<(), SearchCorpusValidationProgressError<E, SearchCorpusSurfaceMutationConflictV1>>
where
    F: FnMut() -> Result<(), E>,
    P: MutationSetPolicyV3<'a>,
{
    for (index, scope) in batch.semantic_replace_scopes.iter().enumerate() {
        if index % 32 == 0 {
            progress().map_err(SearchCorpusValidationProgressError::Progress)?;
        }
        let key = (
            scope.scope.corpus_kind.as_code_str(),
            scope.scope.owner_kind.as_code_str(),
            scope.scope.owner_id.as_str(),
        );
        let surface = SearchScopeSurface::for_semantic_owner_v1(
            scope.scope.owner_kind,
            scope.scope.corpus_kind,
        );
        if !sets.replace_semantic(key) {
            return Err(
                SearchCorpusSurfaceMutationConflictV1::DuplicateReplaceScope(surface).into(),
            );
        }
    }
    for (index, scope) in batch.semantic_tombstone_scopes.iter().enumerate() {
        if index % 32 == 0 {
            progress().map_err(SearchCorpusValidationProgressError::Progress)?;
        }
        let key = (
            scope.corpus_kind.as_code_str(),
            scope.owner_kind.as_code_str(),
            scope.owner_id.as_str(),
        );
        let surface =
            SearchScopeSurface::for_semantic_owner_v1(scope.owner_kind, scope.corpus_kind);
        if !sets.tombstone_semantic(key) {
            return Err(
                SearchCorpusSurfaceMutationConflictV1::DuplicateTombstoneScope(surface).into(),
            );
        }
        if sets.replaced_semantic(key) {
            return Err(SearchCorpusSurfaceMutationConflictV1::ReplaceAndTombstone(surface).into());
        }
    }
    Ok(())
}

fn validate_search_corpus_clear_disjoint_with_progress_v1<E, F>(
    batch: &SearchCorpusIngestBatch,
    clear_surfaces: &[SearchScopeSurface],
    progress: &mut F,
) -> Result<(), SearchCorpusValidationProgressError<E, SearchCorpusSurfaceMutationConflictV1>>
where
    F: FnMut() -> Result<(), E>,
{
    for (index, surface) in batch
        .semantic_replace_scopes
        .iter()
        .map(|scope| {
            SearchScopeSurface::for_semantic_owner_v1(
                scope.scope.owner_kind,
                scope.scope.corpus_kind,
            )
        })
        .enumerate()
    {
        if index % 32 == 0 {
            progress().map_err(SearchCorpusValidationProgressError::Progress)?;
        }
        if clear_surfaces.contains(&surface) {
            return Err(SearchCorpusSurfaceMutationConflictV1::ClearAndReplace(surface).into());
        }
    }
    for (index, surface) in batch
        .semantic_tombstone_scopes
        .iter()
        .map(|scope| SearchScopeSurface::for_semantic_owner_v1(scope.owner_kind, scope.corpus_kind))
        .enumerate()
    {
        if index % 32 == 0 {
            progress().map_err(SearchCorpusValidationProgressError::Progress)?;
        }
        if clear_surfaces.contains(&surface) {
            return Err(SearchCorpusSurfaceMutationConflictV1::ClearAndTombstone(surface).into());
        }
    }
    Ok(())
}
