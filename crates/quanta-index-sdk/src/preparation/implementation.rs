use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use quanta_index_contract::lex::{LanguageCode, SymbolRecord};
use quanta_index_contract::{
    ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusReplaceScope, SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1,
    SourceFileCoverage, SourceFileKey, SourceFileRevision, SymbolCoverage,
    SymbolNameSourcePolicyV1, batch_digest_token_v1, source_file_unit_set_sha256,
    validate_lexical_file_mutations_v1, validate_semantic_source_record_v1,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest as _, Sha256};

use crate::{BatchMode, SearchCorpusBatch};

/// Independent admission limits for source bytes, emitted text and serialized batches.
///
/// Emitted text bytes bound chunk and semantic text together. Batch bytes also
/// count metadata. All limits are effective inputs to the recipe digest.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct PreparationBudgets {
    input_bytes: usize,
    emitted_text_bytes: usize,
    chunk_bytes: usize,
    chunks: usize,
    batch_bytes: usize,
}

impl PreparationBudgets {
    pub fn new(
        max_input_bytes: usize,
        max_emitted_text_bytes: usize,
        max_chunk_bytes: usize,
        max_chunks: usize,
        max_batch_bytes: usize,
    ) -> Result<Self, PreparationError> {
        let result = Self {
            input_bytes: max_input_bytes,
            emitted_text_bytes: max_emitted_text_bytes,
            chunk_bytes: max_chunk_bytes,
            chunks: max_chunks,
            batch_bytes: max_batch_bytes,
        };
        result.validate()?;
        Ok(result)
    }

    #[must_use]
    pub fn max_input_bytes(self) -> usize {
        self.input_bytes
    }
    #[must_use]
    pub fn max_emitted_text_bytes(self) -> usize {
        self.emitted_text_bytes
    }
    #[must_use]
    pub fn max_chunk_bytes(self) -> usize {
        self.chunk_bytes
    }
    #[must_use]
    pub fn max_chunks(self) -> usize {
        self.chunks
    }
    #[must_use]
    pub fn max_batch_bytes(self) -> usize {
        self.batch_bytes
    }
    pub fn with_max_emitted_text_bytes(mut self, value: usize) -> Result<Self, PreparationError> {
        self.emitted_text_bytes = value;
        self.validate()?;
        Ok(self)
    }
    pub fn with_max_chunk_bytes(mut self, value: usize) -> Result<Self, PreparationError> {
        self.chunk_bytes = value;
        self.validate()?;
        Ok(self)
    }
    pub fn with_max_chunks(mut self, value: usize) -> Result<Self, PreparationError> {
        self.chunks = value;
        self.validate()?;
        Ok(self)
    }
    pub fn with_max_batch_bytes(mut self, value: usize) -> Result<Self, PreparationError> {
        self.batch_bytes = value;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(self) -> Result<(), PreparationError> {
        if self.input_bytes == 0
            || self.emitted_text_bytes == 0
            || self.chunk_bytes == 0
            || self.chunks == 0
            || self.batch_bytes == 0
            || u32::try_from(self.input_bytes).is_err()
            || [
                self.emitted_text_bytes,
                self.chunk_bytes,
                self.chunks,
                self.batch_bytes,
            ]
            .into_iter()
            .any(|value| u64::try_from(value).is_err())
            || self.emitted_text_bytes > self.batch_bytes
            || self.chunk_bytes > self.emitted_text_bytes
        {
            return Err(PreparationError::InvalidBudget);
        }
        Ok(())
    }
}

/// A revisioned, deterministic preparation recipe. Caller supplied values are
/// validated; changing any field changes the producer policy digest.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PreparationProfile {
    adapter_id: String,
    recipe_revision: String,
    budgets: PreparationBudgets,
    sha256: [u8; 32],
}

impl PreparationProfile {
    pub fn new(
        adapter_id: impl Into<String>,
        recipe_revision: impl Into<String>,
        budgets: PreparationBudgets,
    ) -> Result<Self, PreparationError> {
        budgets.validate()?;
        let adapter_id = adapter_id.into();
        let recipe_revision = recipe_revision.into();
        for token in [&adapter_id, &recipe_revision] {
            if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(PreparationError::InvalidProfile);
            }
        }
        let mut hash = Sha256::new();
        hash.update(b"quanta-index:preparation-profile:v1\0");
        digest_field(&mut hash, adapter_id.as_bytes())?;
        digest_field(&mut hash, recipe_revision.as_bytes())?;
        for value in [
            budgets.input_bytes,
            budgets.emitted_text_bytes,
            budgets.chunk_bytes,
            budgets.chunks,
            budgets.batch_bytes,
        ] {
            hash.update(
                u64::try_from(value)
                    .map_err(|_error| PreparationError::InvalidBudget)?
                    .to_be_bytes(),
            );
        }
        Ok(Self {
            adapter_id,
            recipe_revision,
            budgets,
            sha256: hash.finalize().into(),
        })
    }

    #[must_use]
    pub fn adapter_id(&self) -> &str {
        &self.adapter_id
    }
    #[must_use]
    pub fn recipe_revision(&self) -> &str {
        &self.recipe_revision
    }
    #[must_use]
    pub fn budgets(&self) -> PreparationBudgets {
        self.budgets
    }

    #[must_use]
    pub fn sha256(&self) -> [u8; 32] {
        self.sha256
    }
}

fn digest_field(hash: &mut Sha256, bytes: &[u8]) -> Result<(), PreparationError> {
    let length = u64::try_from(bytes.len())
        .map_err(|_error| PreparationError::LimitExceeded("identity byte length"))?;
    hash.update(length.to_be_bytes());
    hash.update(bytes);
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PreparationError {
    InvalidBudget,
    InvalidProfile,
    AdapterMismatch,
    InvalidSource(&'static str),
    SourceDigestMismatch,
    InvalidUtf8,
    LimitExceeded(&'static str),
    InvalidContribution(String),
    DuplicateSource,
    PathOwnershipConflict,
    ManifestConflict,
}

impl std::fmt::Display for PreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PreparationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceContext {
    /// Caller-owned identity that remains stable across a path move.
    stable_key: String,
    source: SourceFileRevision,
    profile: PreparationProfile,
}

impl SourceContext {
    pub fn new(
        stable_key: impl Into<String>,
        source: SourceFileRevision,
        profile: PreparationProfile,
    ) -> Result<Self, PreparationError> {
        let stable_key = stable_key.into();
        validate_stable_key(&stable_key)?;
        source.validate().map_err(PreparationError::InvalidSource)?;
        Ok(Self {
            stable_key,
            source,
            profile,
        })
    }
    #[must_use]
    pub fn stable_key(&self) -> &str {
        &self.stable_key
    }
    #[must_use]
    pub fn source(&self) -> &SourceFileRevision {
        &self.source
    }
    #[must_use]
    pub fn profile(&self) -> &PreparationProfile {
        &self.profile
    }

    fn validate(&self, bytes: &[u8]) -> Result<(), PreparationError> {
        validate_stable_key(&self.stable_key)?;
        self.source
            .validate()
            .map_err(PreparationError::InvalidSource)?;
        self.profile.budgets.validate()?;
        if bytes.len() > self.profile.budgets.input_bytes {
            return Err(PreparationError::LimitExceeded("input bytes"));
        }
        if <[u8; 32]>::from(Sha256::digest(bytes)) != self.source.source_sha256 {
            return Err(PreparationError::SourceDigestMismatch);
        }
        Ok(())
    }
}

fn validate_stable_key(value: &str) -> Result<(), PreparationError> {
    if value.is_empty() || value.len() > 512 || !value.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(PreparationError::InvalidSource(
            "stable key must be 1..=512 printable ASCII bytes",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparationCapabilities {
    lexical: bool,
    typed_semantic: bool,
    symbols: bool,
}

impl PreparationCapabilities {
    #[must_use]
    pub const fn new(lexical: bool, typed_semantic: bool, symbols: bool) -> Self {
        Self {
            lexical,
            typed_semantic,
            symbols,
        }
    }
    #[must_use]
    pub const fn lexical(self) -> bool {
        self.lexical
    }
    #[must_use]
    pub const fn typed_semantic(self) -> bool {
        self.typed_semantic
    }
    #[must_use]
    pub const fn symbols(self) -> bool {
        self.symbols
    }
}

pub trait SourceAdapter<Input> {
    fn capabilities(&self) -> PreparationCapabilities;
    fn prepare(&self, input: Input) -> Result<PreparedSource, PreparationError>;
}

pub struct TextSource<'a> {
    context: SourceContext,
    bytes: &'a [u8],
}

impl<'a> TextSource<'a> {
    #[must_use]
    pub fn new(context: SourceContext, bytes: &'a [u8]) -> Self {
        Self { context, bytes }
    }
    #[must_use]
    pub fn into_parts(self) -> (SourceContext, &'a [u8]) {
        (self.context, self.bytes)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PlainTextAdapter;
#[derive(Clone, Copy, Debug, Default)]
pub struct MarkdownAdapter;

impl PlainTextAdapter {
    pub const ADAPTER_ID: &'static str = "text/plain;utf8";
    pub const RECIPE_REVISION: &'static str = "plain-lines-v1";
    pub fn profile(budgets: PreparationBudgets) -> Result<PreparationProfile, PreparationError> {
        PreparationProfile::new(Self::ADAPTER_ID, Self::RECIPE_REVISION, budgets)
    }
}

impl MarkdownAdapter {
    pub const ADAPTER_ID: &'static str = "text/markdown;utf8";
    pub const RECIPE_REVISION: &'static str = "markdown-source-lines-v1";
    pub fn profile(budgets: PreparationBudgets) -> Result<PreparationProfile, PreparationError> {
        PreparationProfile::new(Self::ADAPTER_ID, Self::RECIPE_REVISION, budgets)
    }
}

impl<'a> SourceAdapter<TextSource<'a>> for PlainTextAdapter {
    fn capabilities(&self) -> PreparationCapabilities {
        lexical_only()
    }
    fn prepare(&self, input: TextSource<'a>) -> Result<PreparedSource, PreparationError> {
        prepare_text(input, Self::ADAPTER_ID, Self::RECIPE_REVISION, "text")
    }
}
impl<'a> SourceAdapter<TextSource<'a>> for MarkdownAdapter {
    fn capabilities(&self) -> PreparationCapabilities {
        lexical_only()
    }
    fn prepare(&self, input: TextSource<'a>) -> Result<PreparedSource, PreparationError> {
        prepare_text(input, Self::ADAPTER_ID, Self::RECIPE_REVISION, "markdown")
    }
}
fn lexical_only() -> PreparationCapabilities {
    PreparationCapabilities::new(true, false, false)
}

fn prepare_text(
    input: TextSource<'_>,
    adapter: &str,
    recipe: &str,
    language: &str,
) -> Result<PreparedSource, PreparationError> {
    if input.context.profile.adapter_id() != adapter
        || input.context.profile.recipe_revision() != recipe
    {
        return Err(PreparationError::AdapterMismatch);
    }
    input.context.validate(input.bytes)?;
    let text = std::str::from_utf8(input.bytes).map_err(|_error| PreparationError::InvalidUtf8)?;
    let budget = input.context.profile.budgets();
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut start_line = 1u32;
    let mut cursor = 0usize;
    let mut line = 1u32;
    // Every emitted chunk is a whole-line, exact byte slice. No Markdown
    // rendering, syntax inference, or lossy normalization is performed.
    for part in text.split_inclusive('\n') {
        if part.len() > budget.chunk_bytes {
            return Err(PreparationError::LimitExceeded("line bytes"));
        }
        let next_cursor = cursor
            .checked_add(part.len())
            .ok_or(PreparationError::LimitExceeded("byte offset"))?;
        let span_bytes = next_cursor
            .checked_sub(start)
            .ok_or_else(|| PreparationError::InvalidContribution("chunk cursor order".into()))?;
        if cursor > start && span_bytes > budget.chunk_bytes {
            push_chunk(
                &mut chunks,
                &input.context,
                text,
                start,
                cursor,
                start_line,
                line,
            )?;
            start = cursor;
            start_line = line;
        }
        cursor = next_cursor;
        line = line
            .checked_add(u32::from(part.ends_with('\n')))
            .ok_or(PreparationError::LimitExceeded("line number"))?;
    }
    if cursor > start {
        push_chunk(
            &mut chunks,
            &input.context,
            text,
            start,
            cursor,
            start_line,
            line,
        )?;
    }
    PreparedSource::from_lexical(
        input.context,
        input.bytes.to_vec(),
        LanguageCode::new(language).map_err(PreparationError::InvalidSource)?,
        chunks,
        Vec::new(),
        SymbolCoverage::NotRequested,
        Vec::new(),
    )
}

fn push_chunk(
    chunks: &mut Vec<ChunkRecord>,
    context: &SourceContext,
    text: &str,
    start: usize,
    end: usize,
    start_line: u32,
    end_line: u32,
) -> Result<(), PreparationError> {
    if chunks.len() >= context.profile.budgets.chunks {
        return Err(PreparationError::LimitExceeded("chunk count"));
    }
    let slice = text
        .get(start..end)
        .ok_or_else(|| PreparationError::InvalidContribution("non-UTF-8 chunk boundary".into()))?;
    let mut hash = Sha256::new();
    hash.update(b"quanta-index:prepared-chunk:v1\0");
    digest_field(
        &mut hash,
        context.source.file.source_repo_id.as_str().as_bytes(),
    )?;
    digest_field(&mut hash, context.stable_key.as_bytes())?;
    digest_field(
        &mut hash,
        context.source.file.repo_relative_path.as_str().as_bytes(),
    )?;
    digest_field(&mut hash, context.source.revision_id.as_str().as_bytes())?;
    hash.update(context.source.source_sha256);
    hash.update(context.profile.sha256());
    hash.update(
        u64::try_from(start)
            .map_err(|_error| PreparationError::LimitExceeded("byte offset"))?
            .to_be_bytes(),
    );
    hash.update(
        u64::try_from(end)
            .map_err(|_error| PreparationError::LimitExceeded("byte offset"))?
            .to_be_bytes(),
    );
    digest_field(&mut hash, slice.as_bytes())?;
    let id = format!("prep:{}", batch_digest_token_v1(&hash.finalize().into()));
    chunks.push(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: context.source.file.repo_relative_path.clone(),
        language: LanguageCode::new(if context.profile.adapter_id() == "text/markdown;utf8" {
            "markdown"
        } else {
            "text"
        })
        .map_err(PreparationError::InvalidSource)?,
        start_byte: u32::try_from(start)
            .map_err(|_error| PreparationError::LimitExceeded("byte offset"))?,
        end_byte: u32::try_from(end)
            .map_err(|_error| PreparationError::LimitExceeded("byte offset"))?,
        start_line,
        end_line,
        text: slice.into(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: Some(context.source.file.source_repo_id.clone()),
    });
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedSource {
    context: SourceContext,
    bytes: Vec<u8>,
    coverage: SourceFileCoverage,
    chunks: Vec<ChunkRecord>,
    symbols: Vec<SymbolRecord>,
    semantic: Vec<SemanticSourceReplaceScopeV1>,
    measured_batch_bytes: usize,
}

impl PreparedSource {
    /// Checked constructor for custom compile-time adapters. All contributions
    /// remain typed and are checked against the existing lexical contract.
    pub fn from_lexical(
        context: SourceContext,
        bytes: Vec<u8>,
        language: LanguageCode,
        chunks: Vec<ChunkRecord>,
        symbols: Vec<SymbolRecord>,
        symbol_coverage: SymbolCoverage,
        mut semantic: Vec<SemanticSourceReplaceScopeV1>,
    ) -> Result<Self, PreparationError> {
        context.validate(&bytes)?;
        let budget = context.profile.budgets();
        if chunks.len() > budget.chunks
            || chunks
                .iter()
                .any(|chunk| chunk.text.len() > budget.chunk_bytes)
            || symbols.len() > budget.chunks
            || semantic.len() > budget.chunks
        {
            return Err(PreparationError::LimitExceeded("prepared output"));
        }
        let semantic_record_count = checked_sum(semantic.iter().map(|scope| scope.sources.len()))?;
        let membership_count =
            checked_sum(semantic.iter().map(|scope| scope.cluster_memberships.len()))?;
        if semantic_record_count > budget.chunks || membership_count > budget.chunks {
            return Err(PreparationError::LimitExceeded("semantic record count"));
        }
        let lexical_bytes = checked_sum(chunks.iter().map(|chunk| chunk.text.len()))?;
        let semantic_bytes = checked_sum(
            semantic
                .iter()
                .flat_map(|scope| scope.sources.iter())
                .map(|record| record.text.len()),
        )?;
        if lexical_bytes
            .checked_add(semantic_bytes)
            .ok_or(PreparationError::LimitExceeded(
                "output byte count overflow",
            ))?
            > budget.emitted_text_bytes
        {
            return Err(PreparationError::LimitExceeded("emitted text bytes"));
        }
        crate::lexical::validate_semantic_cluster_membership_authority_v1(&semantic)
            .map_err(|error| PreparationError::InvalidContribution(error.to_string()))?;
        // Match SearchCorpusBatch::replace_semantic_scope before measuring or
        // hashing: a prior stamp must describe the same contribution as wire.
        for item in &mut semantic {
            crate::lexical::canonicalize_semantic_cluster_memberships_v1(
                &mut item.cluster_memberships,
            );
        }
        let mut semantic_keys = BTreeSet::new();
        for item in &semantic {
            if invalid_semantic_key(&item.scope)
                || item.scope_digest.is_empty()
                || !item.scope_digest.bytes().all(|b| b.is_ascii_graphic())
                || !semantic_keys.insert(semantic_key(&item.scope))
            {
                return Err(PreparationError::InvalidContribution(
                    "semantic scope digest or duplicate key".into(),
                ));
            }
            for record in &item.sources {
                validate_semantic_source_record_v1(record)
                    .map_err(|error| PreparationError::InvalidContribution(error.into()))?;
                if record.repo_relative_path != context.source.file.repo_relative_path
                    || record.corpus_kind != item.scope.corpus_kind
                    || record.owner_kind != item.scope.owner_kind
                    || record.owner_id != item.scope.owner_id
                {
                    return Err(PreparationError::InvalidContribution(
                        "semantic source scope mismatch".into(),
                    ));
                }
            }
        }
        let unit_set_sha256 = source_file_unit_set_sha256(&chunks, &symbols)
            .map_err(|error| PreparationError::InvalidContribution(error.to_string()))?;
        let coverage = SourceFileCoverage {
            source: context.source.clone(),
            language,
            producer_policy_sha256: context.profile.sha256(),
            symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
            unit_set_sha256,
            text_admitted: true,
            symbols: symbol_coverage,
        };
        let scope = SearchCorpusReplaceScope {
            coverage,
            source_bytes: bytes,
            chunks,
            symbols,
        };
        validate_lexical_file_mutations_v1(&[], std::slice::from_ref(&scope), &[])
            .map_err(|error| PreparationError::InvalidContribution(error.to_string()))?;
        let mut measured = LimitedWriter {
            count: 0,
            limit: budget.batch_bytes,
            exceeded: false,
        };
        encode_bounded(&scope, &mut measured)?;
        encode_bounded(&semantic, &mut measured)?;
        let SearchCorpusReplaceScope {
            coverage,
            source_bytes: bytes,
            chunks,
            symbols,
        } = scope;
        Ok(Self {
            context,
            bytes,
            coverage,
            chunks,
            symbols,
            semantic,
            measured_batch_bytes: measured.count,
        })
    }

    #[must_use]
    pub fn source(&self) -> &SourceFileRevision {
        &self.context.source
    }
    #[must_use]
    pub fn profile_sha256(&self) -> [u8; 32] {
        self.context.profile.sha256()
    }
    #[must_use]
    pub fn coverage(&self) -> &SourceFileCoverage {
        &self.coverage
    }
    #[must_use]
    pub fn chunks(&self) -> &[ChunkRecord] {
        &self.chunks
    }
    #[must_use]
    pub fn semantic(&self) -> &[SemanticSourceReplaceScopeV1] {
        &self.semantic
    }
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Caller-persisted prior state, scoped to exactly the file universe being
/// reconciled. An omitted entry means delete, including the old side of move.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriorSourceEntry {
    stable_key: String,
    coverage: SourceFileCoverage,
    /// Typed scope key and independently hashed canonical contribution bytes.
    /// A delete cannot infer old owners from new input.
    semantic_scopes: Vec<(SemanticSourceScopeKeyV1, [u8; 32])>,
}

impl PriorSourceEntry {
    pub fn new(
        stable_key: impl Into<String>,
        coverage: SourceFileCoverage,
        semantic_scopes: Vec<(SemanticSourceScopeKeyV1, [u8; 32])>,
    ) -> Result<Self, PreparationError> {
        let entry = Self {
            stable_key: stable_key.into(),
            coverage,
            semantic_scopes,
        };
        validate_stable_key(&entry.stable_key)?;
        entry
            .coverage
            .source
            .validate()
            .map_err(PreparationError::InvalidSource)?;
        validate_semantic_stamps(&entry.semantic_scopes)?;
        Ok(entry)
    }

    #[must_use]
    pub fn stable_key(&self) -> &str {
        &self.stable_key
    }
    #[must_use]
    pub fn source(&self) -> &SourceFileRevision {
        &self.coverage.source
    }
    #[must_use]
    pub fn coverage(&self) -> &SourceFileCoverage {
        &self.coverage
    }
    #[must_use]
    pub fn profile_sha256(&self) -> [u8; 32] {
        self.coverage.producer_policy_sha256
    }
    #[must_use]
    pub fn unit_set_sha256(&self) -> [u8; 32] {
        self.coverage.unit_set_sha256
    }
    #[must_use]
    pub fn semantic_scopes(&self) -> &[(SemanticSourceScopeKeyV1, [u8; 32])] {
        &self.semantic_scopes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriorSourceManifest {
    entries: Vec<PriorSourceEntry>,
}

impl Serialize for PriorSourceManifest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let rows: Vec<_> = self
            .entries
            .iter()
            .map(|entry| (&entry.stable_key, &entry.coverage, &entry.semantic_scopes))
            .collect();
        rows.serialize(serializer)
    }
}

type PriorSourceRow = (
    String,
    SourceFileCoverage,
    Vec<(SemanticSourceScopeKeyV1, [u8; 32])>,
);

impl<'de> Deserialize<'de> for PriorSourceManifest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let rows: Vec<PriorSourceRow> = Vec::deserialize(deserializer)?;
        let entries: Vec<_> = rows
            .into_iter()
            .map(|(stable_key, coverage, semantic_scopes)| PriorSourceEntry {
                stable_key,
                coverage,
                semantic_scopes,
            })
            .collect();
        let canonical = Self::new(entries.clone()).map_err(de::Error::custom)?;
        if entries != canonical.entries {
            return Err(de::Error::custom(
                "prior-source manifest is not canonically ordered",
            ));
        }
        Ok(canonical)
    }
}

impl PriorSourceManifest {
    pub fn new(mut entries: Vec<PriorSourceEntry>) -> Result<Self, PreparationError> {
        let mut keys = BTreeSet::new();
        let mut stable_keys = BTreeSet::new();
        let mut paths = BTreeMap::new();
        let mut semantic = BTreeSet::new();
        for entry in &entries {
            validate_stable_key(&entry.stable_key)?;
            if !stable_keys.insert(entry.stable_key.clone()) {
                return Err(PreparationError::DuplicateSource);
            }
            entry
                .coverage
                .source
                .validate()
                .map_err(PreparationError::InvalidSource)?;
            if !keys.insert(entry.source().file.clone()) {
                return Err(PreparationError::DuplicateSource);
            }
            check_path_owner(&mut paths, &entry.source().file)?;
            validate_semantic_stamps(&entry.semantic_scopes)?;
            for (scope, _) in &entry.semantic_scopes {
                if invalid_semantic_key(scope) || !semantic.insert(semantic_key(scope)) {
                    return Err(PreparationError::ManifestConflict);
                }
            }
        }
        entries.sort_by(|left, right| left.stable_key.cmp(&right.stable_key));
        Ok(Self { entries })
    }

    #[must_use]
    pub fn entries(&self) -> &[PriorSourceEntry] {
        &self.entries
    }
}

fn check_path_owner(
    paths: &mut BTreeMap<RepoRelativePath, quanta_index_contract::RepoId>,
    key: &SourceFileKey,
) -> Result<(), PreparationError> {
    match paths.insert(key.repo_relative_path.clone(), key.source_repo_id.clone()) {
        Some(previous) if previous != key.source_repo_id => {
            Err(PreparationError::PathOwnershipConflict)
        }
        _ => Ok(()),
    }
}

pub struct PreparedChanges {
    replacements: Vec<PreparedSource>,
    tombstones: Vec<SourceFileKey>,
    semantic_tombstones: Vec<SemanticSourceScopeKeyV1>,
    next: PriorSourceManifest,
    effective_batch_limit: usize,
    intent: ReconcileIntent,
}

/// Caller-declared target for a full-universe reconciliation. A prior snapshot
/// requires a base generation because its unchanged files are delta omissions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconcileIntent {
    repo_id: RepoId,
    revision_id: RevisionId,
    base_generation: Option<ManifestGeneration>,
}

impl ReconcileIntent {
    #[must_use]
    pub fn new(
        repo_id: RepoId,
        revision_id: RevisionId,
        base_generation: Option<ManifestGeneration>,
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            base_generation,
        }
    }
    #[must_use]
    pub fn repo_id(&self) -> &RepoId {
        &self.repo_id
    }
    #[must_use]
    pub fn revision_id(&self) -> &RevisionId {
        &self.revision_id
    }
    #[must_use]
    pub fn base_generation(&self) -> Option<ManifestGeneration> {
        self.base_generation
    }
}

impl PreparedChanges {
    #[must_use]
    pub fn replacements(&self) -> &[PreparedSource] {
        &self.replacements
    }
    #[must_use]
    pub fn tombstones(&self) -> &[SourceFileKey] {
        &self.tombstones
    }
    #[must_use]
    pub fn semantic_tombstones(&self) -> &[SemanticSourceScopeKeyV1] {
        &self.semantic_tombstones
    }
    /// Proposed caller-owned snapshot. Persist it with the original publication
    /// event, expected-base event/generation, and receipt only after the owner
    /// confirms the intended generation. An ambiguous publish or activation
    /// outcome must not advance this snapshot; recover the original operation.
    #[must_use]
    pub fn next_manifest(&self) -> &PriorSourceManifest {
        &self.next
    }

    /// Lower once to the existing SDK batch. The returned manifest is planned
    /// state, never proof of publish or activation. Caller retains the original
    /// event, expected base, generation and receipt to recover ambiguous outcomes.
    pub fn apply_to_batch(
        self,
        mut batch: SearchCorpusBatch,
    ) -> Result<(SearchCorpusBatch, PriorSourceManifest), PreparationError> {
        if batch.repo_id() != &self.intent.repo_id
            || batch.revision_id() != &self.intent.revision_id
            || batch.base_generation() != self.intent.base_generation
            || batch.mode()
                != if self.intent.base_generation.is_some() {
                    BatchMode::Delta
                } else {
                    BatchMode::ReplaceGeneration
                }
        {
            return Err(PreparationError::ManifestConflict);
        }
        // The complete-universe manifest describes every source mutation in
        // this batch. Existing mutations would make its planned next state
        // disagree with the published content, even for a no-op delta.
        if !batch.clear_surfaces().is_empty()
            || !batch.replace_scopes().is_empty()
            || !batch.tombstone_scopes().is_empty()
            || !batch.semantic_replace_scopes().is_empty()
            || !batch.semantic_tombstone_scopes().is_empty()
        {
            return Err(PreparationError::ManifestConflict);
        }
        let limit = self.effective_batch_limit;
        for source in self.replacements {
            batch =
                batch.replace_scope(source.coverage, source.bytes, source.chunks, source.symbols);
            for semantic in source.semantic {
                batch = batch.replace_semantic_scope(
                    semantic.scope,
                    semantic.scope_digest,
                    semantic.sources,
                    semantic.cluster_memberships,
                );
            }
        }
        for file in self.tombstones {
            batch = batch.tombstone_scope(file);
        }
        for scope in self.semantic_tombstones {
            batch = batch.tombstone_semantic_scope(scope);
        }
        let wire = batch
            .to_wire_batch()
            .map_err(|error| PreparationError::InvalidContribution(error.to_string()))?;
        wire.validate_v1()
            .map_err(|error| PreparationError::InvalidContribution(error.to_string()))?;
        wire.validate_surface_mutations_v1()
            .map_err(|error| PreparationError::InvalidContribution(error.to_string()))?;
        crate::lexical::validate_semantic_cluster_membership_authority_v1(
            &wire.semantic_replace_scopes,
        )
        .map_err(|error| PreparationError::InvalidContribution(error.to_string()))?;
        let body_bytes = quanta_index_ipc::cbor_payload_len(&wire)
            .map_err(|error| PreparationError::InvalidContribution(error.to_string()))?;
        if body_bytes > u64::try_from(limit).map_err(|_error| PreparationError::InvalidBudget)? {
            return Err(PreparationError::LimitExceeded("batch bytes"));
        }
        Ok((batch, self.next))
    }
}

/// The caller asserts this is the complete source contribution universe for
/// the supplied prior manifest. Partial updates must use a different owner API.
pub struct CompleteSourceSet {
    sources: Vec<PreparedSource>,
}

impl CompleteSourceSet {
    #[must_use]
    pub fn new(sources: Vec<PreparedSource>) -> Self {
        Self { sources }
    }
}

/// Omission means deletion only under the caller's complete-universe assertion.
pub fn reconcile_complete_universe(
    prior: &PriorSourceManifest,
    current_set: CompleteSourceSet,
    intent: ReconcileIntent,
    aggregate_batch_bytes: usize,
) -> Result<PreparedChanges, PreparationError> {
    if aggregate_batch_bytes == 0 {
        return Err(PreparationError::InvalidBudget);
    }
    if !prior.entries().is_empty() && intent.base_generation.is_none() {
        return Err(PreparationError::ManifestConflict);
    }
    let prior_by_key: BTreeMap<_, _> = prior
        .entries
        .iter()
        .map(|entry| (entry.source().file.clone(), entry))
        .collect();
    let prior_by_stable: BTreeMap<_, _> = prior
        .entries
        .iter()
        .map(|entry| (entry.stable_key.as_str(), entry))
        .collect();
    let mut current = BTreeMap::new();
    let mut stable_keys = BTreeSet::new();
    let mut prior_paths = BTreeMap::new();
    for entry in prior.entries() {
        check_path_owner(&mut prior_paths, &entry.source().file)?;
    }
    let mut current_paths = BTreeMap::new();
    for source in current_set.sources {
        check_path_owner(&mut current_paths, &source.source().file)?;
        if !stable_keys.insert(source.context.stable_key.clone()) {
            return Err(PreparationError::DuplicateSource);
        }
        if current
            .insert(source.source().file.clone(), source)
            .is_some()
        {
            return Err(PreparationError::DuplicateSource);
        }
    }
    let mut next_entries = Vec::with_capacity(current.len());
    let mut next_source_keys = BTreeSet::new();
    let mut replacements = Vec::new();
    let mut next_semantic = BTreeSet::new();
    let mut total_batch_bytes = 0usize;
    let mut max_batch_bytes = aggregate_batch_bytes;
    for source in current.into_values() {
        let mut semantic_scopes: Vec<_> = source
            .semantic
            .iter()
            .map(|item| semantic_content_sha256(item).map(|digest| (item.scope.clone(), digest)))
            .collect::<Result<_, _>>()?;
        semantic_scopes.sort_by_key(|item| semantic_key(&item.0));
        for (scope, _) in &semantic_scopes {
            if !next_semantic.insert(semantic_key(scope)) {
                return Err(PreparationError::ManifestConflict);
            }
        }
        let unchanged = prior_by_stable
            .get(source.context.stable_key.as_str())
            .is_some_and(|previous| {
                previous.coverage == source.coverage && previous.semantic_scopes == semantic_scopes
            });
        next_entries.push(PriorSourceEntry {
            stable_key: source.context.stable_key.clone(),
            coverage: source.coverage.clone(),
            semantic_scopes,
        });
        if !next_source_keys.insert(source.source().file.clone()) {
            return Err(PreparationError::DuplicateSource);
        }
        if !unchanged {
            total_batch_bytes = total_batch_bytes
                .checked_add(source.measured_batch_bytes)
                .ok_or(PreparationError::LimitExceeded("batch byte count overflow"))?;
            max_batch_bytes = max_batch_bytes.min(source.context.profile.budgets.batch_bytes);
            replacements.push(source);
        }
    }
    let tombstones = prior_by_key
        .keys()
        .filter(|key| !next_source_keys.contains(*key))
        .cloned()
        .collect();
    let semantic_tombstones = prior
        .entries()
        .iter()
        .flat_map(|entry| entry.semantic_scopes.iter())
        .filter(|(scope, _)| !next_semantic.contains(&semantic_key(scope)))
        .map(|(scope, _)| scope.clone())
        .collect();
    if total_batch_bytes > max_batch_bytes {
        return Err(PreparationError::LimitExceeded("batch bytes"));
    }
    Ok(PreparedChanges {
        replacements,
        tombstones,
        semantic_tombstones,
        next: PriorSourceManifest::new(next_entries)?,
        effective_batch_limit: max_batch_bytes,
        intent,
    })
}

fn semantic_key(scope: &SemanticSourceScopeKeyV1) -> (String, String, String) {
    (
        scope.corpus_kind.as_code_str().into(),
        scope.owner_kind.as_code_str().into(),
        scope.owner_id.clone(),
    )
}

fn invalid_semantic_key(scope: &SemanticSourceScopeKeyV1) -> bool {
    scope.owner_id.is_empty() || scope.owner_id.chars().any(char::is_control)
}

fn validate_semantic_stamps(
    stamps: &[(SemanticSourceScopeKeyV1, [u8; 32])],
) -> Result<(), PreparationError> {
    if stamps.iter().any(|(scope, _)| invalid_semantic_key(scope))
        || !stamps
            .iter()
            .zip(stamps.iter().skip(1))
            .all(|(left, right)| semantic_key(&left.0) < semantic_key(&right.0))
    {
        return Err(PreparationError::ManifestConflict);
    }
    Ok(())
}

struct LimitedWriter {
    count: usize,
    limit: usize,
    exceeded: bool,
}

impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.count = if let Some(value) = self.count.checked_add(bytes.len()) {
            value
        } else {
            self.exceeded = true;
            return Err(io::Error::other("prepared batch byte count overflow"));
        };
        if self.count > self.limit {
            self.exceeded = true;
            return Err(io::Error::other("prepared batch byte budget exceeded"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode_bounded<T: serde::Serialize>(
    value: &T,
    writer: &mut LimitedWriter,
) -> Result<(), PreparationError> {
    ciborium::into_writer(value, &mut *writer).map_err(|error| {
        if writer.exceeded {
            PreparationError::LimitExceeded("batch bytes")
        } else {
            PreparationError::InvalidContribution(format!("bounded contribution encoding: {error}"))
        }
    })
}

fn checked_sum(mut values: impl Iterator<Item = usize>) -> Result<usize, PreparationError> {
    values.try_fold(0usize, |sum, value| {
        sum.checked_add(value)
            .ok_or(PreparationError::LimitExceeded(
                "byte or record count overflow",
            ))
    })
}

struct DigestWriter(Sha256);

impl Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn semantic_content_sha256(
    scope: &SemanticSourceReplaceScopeV1,
) -> Result<[u8; 32], PreparationError> {
    let mut writer = DigestWriter(Sha256::new());
    writer
        .0
        .update(b"quanta-index:prepared-semantic-scope:v1\0");
    ciborium::into_writer(scope, &mut writer).map_err(|error| {
        PreparationError::InvalidContribution(format!("semantic content encoding: {error}"))
    })?;
    Ok(writer.0.finalize().into())
}

#[cfg(test)]
mod tests;
