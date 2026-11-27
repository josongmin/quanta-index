//! Bounded Native regex verification over authenticated source bytes.

use crate::{
    filters::{FilePathScope, PathConstraint, RepoConstraint},
    normalize,
};
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{MAX_NATIVE_CONTENT_REGEX_LEAVES, MAX_NATIVE_PATH_FILTERS, RepoId};
use quanta_index_core::index::{
    IndexError, MAX_SOURCE_PATH_BYTES, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey,
    SourceRecordOwner, WorkBudget, WorkUsage, owner_map_key,
};
use quanta_index_lq_regex::{
    FieldRegex, RegexCompilationPlan, RegexError, RegexErrorCode, RegexExecutor,
};
use quanta_index_lq_text_normalizer::CaseMode;

use super::text::{NativeTextIndex, TextReadContext};
use super::text_code_search_rank::{CodeSearchRank, RankedFile};
use super::text_overlay::{MAX_TEXT_OVERLAY_DOCUMENT_BYTES, TextOverlaySource};

/// Each `repo:` pattern owns one compiled engine, like a `file:` filter.
pub(super) const MAX_NATIVE_REPO_FILTERS: usize = 4;
pub(super) const MAX_NATIVE_REGEX_PATTERN_BYTES: usize = 4_096;

// The parser and its HIR are admitted while an engine is built. The state
// estimator is structural, so this transient grant is a logical policy, not
// heap proof; the HIR an executor keeps is part of its measured retained heap.
const PREPARE_LOGICAL_BYTES: u64 = quanta_index_lq_regex::MAX_NFA_STATES * 32;

/// The transient grant for building one document engine.
///
/// It covers every NFA the configured engine can build for one pattern, the
/// builder of the one in flight, and the one-pass DFA (see
/// `engine_construction_bytes`).
pub(super) fn document_construction_bytes() -> Result<u64, IndexError> {
    let bytes =
        quanta_index_lq_regex::engine_construction_bytes(&quanta_index_lq_regex::engine_config())
            .ok_or(IndexError::ResourceLimited)?;
    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
}

/// The grants for parsing one pattern and holding its HIR while an engine is
/// built. They end when the compilation does.
pub(super) struct ParseMemory {
    _pattern: Option<MemoryLease>,
    _prepare: Option<MemoryLease>,
}

/// Admit the parse of a pattern of `len` bytes before the parser runs: its
/// text and AST (eight bytes per pattern byte) and the logical HIR policy.
/// Returns the pattern charge with the grants.
pub(super) fn admit_parse(
    len: usize,
    budget: &WorkBudget,
) -> Result<(u64, ParseMemory), IndexError> {
    let pattern_bytes = u64::try_from(len)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(4))
        .ok_or(IndexError::ResourceLimited)?;
    let pattern = budget.reserve_memory(pattern_bytes, MemoryLifetime::Temporary)?;
    let prepare = budget.reserve_memory(PREPARE_LOGICAL_BYTES, MemoryLifetime::Temporary)?;
    budget.checkpoint()?;
    Ok((
        pattern_bytes,
        ParseMemory {
            _pattern: pattern,
            _prepare: prepare,
        },
    ))
}

struct ContentRegex {
    executor: RegexExecutor,
    estimated_states: u64,
}

/// A `file:` filter searches one authoritative path or file name, a short
/// single-line value, so it is a field engine like a `repo:` filter.
struct PathRegex {
    executor: FieldRegex,
    scope: FilePathScope,
    estimated_states: u64,
}

// The grant for an engine's measured retained heap, which includes the
// pattern and HIR an executor keeps. It lives as long as the engine.
pub(super) struct RegexMemory {
    _engine: Option<MemoryLease>,
}

/// A `repo:` filter searches one short repository id, so it is a field
/// engine charged its measured retained heap (see `compile_field`).
struct RepositoryRegex {
    executor: FieldRegex,
    estimated_states: u64,
}

/// Filters over authenticated Source record metadata, applied with the
/// `file:` filters before any candidate can reach ranking or top-k.
///
/// - `repo:<pattern>`: an unanchored, case-sensitive regex search over the
///   record's repository id (`RepoId` text). Several `repo:` filters must all
///   match. Revision-qualified patterns are refused before compilation.
/// - `lang:<id>`: the id is trimmed and ASCII-lowercased by the planner and
///   must be a canonical `LanguageCode`; it is compared for exact equality
///   with the record's language, without alias expansion. Several `lang:`
///   filters form one any-of set.
///
/// A selected record that lacks the metadata a present filter needs is a
/// refusal (`InvalidInput`), never a silent non-match: legacy and restored
/// records cannot prove they are outside the requested set.
#[derive(Default)]
struct RecordMetadataFilters {
    repositories: Vec<RepositoryRegex>,
    languages: Vec<LanguageCode>,
    _memory: Option<MemoryLease>,
}

pub(super) struct NativeRegexProgram {
    content: Vec<ContentRegex>,
    paths: Vec<PathRegex>,
    metadata: RecordMetadataFilters,
    engine_memory: Vec<RegexMemory>,
    _container_memory: Option<MemoryLease>,
}

fn map_regex_error(error: &RegexError) -> IndexError {
    if error.code == RegexErrorCode::PlanLimitExceeded {
        IndexError::ResourceLimited
    } else {
        IndexError::InvalidInput(format!("native regex: {error}"))
    }
}

pub(super) fn compile_one(
    source: &str,
    case: Option<CaseMode>,
    full_match: bool,
    budget: &mut WorkBudget,
) -> Result<(RegexExecutor, u64, RegexMemory), IndexError> {
    if source.len() > MAX_NATIVE_REGEX_PATTERN_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let construction_bytes = document_construction_bytes()?;
    // The parser and the NFAs under construction are admitted only until the
    // engine exists; the engine then holds exactly its retained heap.
    let (pattern_bytes, _parse_memory) = admit_parse(source.len(), budget)?;
    let pattern = match case {
        Some(CaseMode::Folded) => format!("(?i){}", normalize::nfc(source)),
        Some(CaseMode::Sensitive) => normalize::nfc(source).into_owned(),
        None => source.to_owned(),
    };
    let prepared: RegexCompilationPlan =
        RegexExecutor::prepare(&pattern).map_err(|error| map_regex_error(&error))?;
    let prepared = if full_match {
        prepared
            .into_full_match()
            .map_err(|error| map_regex_error(&error))?
    } else {
        prepared
    };
    let states = prepared.estimated_states();
    let _construction_memory =
        budget.reserve_memory(construction_bytes, MemoryLifetime::Temporary)?;
    budget.checkpoint()?;
    let executor =
        RegexExecutor::compile_prepared(prepared).map_err(|error| map_regex_error(&error))?;
    let retained =
        u64::try_from(executor.retained_bytes()).map_err(|_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: pattern_bytes
            .checked_add(states)
            .and_then(|bytes| bytes.checked_add(retained))
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let engine_memory = budget.reserve_memory(retained, MemoryLifetime::Temporary)?;
    budget.checkpoint()?;
    Ok((
        executor,
        states.max(1),
        RegexMemory {
            _engine: engine_memory,
        },
    ))
}

/// Compile one `repo:`, `file:` or `repo.has.file` path pattern as a field
/// engine, matching anywhere in the value or, with `full_match`, the whole
/// value.
///
/// The parser and the NFA under construction are admitted only until the
/// engine exists: `PREPARE_LOGICAL_BYTES` for the HIR, and
/// `FieldRegex::CONSTRUCTION_BYTES` for the NFA builder, capped by the field
/// `nfa_size_limit`, with the NFA it emits.
/// The engine is then charged and holds exactly its reported retained heap,
/// not a document engine's worst-case grant.
pub(super) fn compile_field(
    source: &str,
    full_match: bool,
    budget: &mut WorkBudget,
) -> Result<(FieldRegex, u64, RegexMemory), IndexError> {
    if source.len() > MAX_NATIVE_REGEX_PATTERN_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let construction_bytes = u64::try_from(FieldRegex::CONSTRUCTION_BYTES)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let (pattern_bytes, _parse_memory) = admit_parse(source.len(), budget)?;
    let plan = RegexExecutor::prepare(source).map_err(|error| map_regex_error(&error))?;
    let plan = if full_match {
        plan.into_full_match()
            .map_err(|error| map_regex_error(&error))?
    } else {
        plan
    };
    let states = plan.estimated_states();
    let _construction_memory =
        budget.reserve_memory(construction_bytes, MemoryLifetime::Temporary)?;
    budget.checkpoint()?;
    let executor = FieldRegex::compile_prepared(&plan).map_err(|error| map_regex_error(&error))?;
    drop(plan);
    let retained =
        u64::try_from(executor.retained_bytes()).map_err(|_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: pattern_bytes
            .checked_add(states)
            .and_then(|bytes| bytes.checked_add(retained))
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let engine_memory = budget.reserve_memory(retained, MemoryLifetime::Temporary)?;
    budget.checkpoint()?;
    Ok((
        executor,
        states.max(1),
        RegexMemory {
            _engine: engine_memory,
        },
    ))
}

/// A `path.regex` leaf: a field engine over the NFC repository-relative
/// path, under the query's case mode like a Content regex.
pub(super) struct PathLeafRegex {
    executor: FieldRegex,
    estimated_states: u64,
    _memory: RegexMemory,
}

impl PathLeafRegex {
    pub(super) fn compile(
        source: &str,
        case: CaseMode,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let pattern = match case {
            CaseMode::Folded => format!("(?i){}", normalize::nfc(source)),
            CaseMode::Sensitive => normalize::nfc(source).into_owned(),
        };
        let (executor, estimated_states, memory) = compile_field(&pattern, false, budget)?;
        Ok(Self {
            executor,
            estimated_states,
            _memory: memory,
        })
    }

    /// Whether the NFC form of `path` contains a match.
    pub(super) fn is_match(&self, path: &str, budget: &mut WorkBudget) -> Result<bool, IndexError> {
        let normalized = normalize::nfc(path);
        let scan = u64::try_from(normalized.len().max(1))
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(self.estimated_states)
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: scan,
            ..WorkUsage::default()
        })?;
        Ok(self.executor.is_match(normalized.as_bytes()))
    }
}

impl NativeRegexProgram {
    pub(super) fn content_executor(
        &self,
        index: usize,
    ) -> Result<(&RegexExecutor, u64), IndexError> {
        self.content
            .get(index)
            .map(|regex| (&regex.executor, regex.estimated_states))
            .ok_or_else(|| {
                IndexError::BackendViolation(
                    "native preview regex is missing from prepared program".into(),
                )
            })
    }

    pub(super) fn compile(
        content: &[&str],
        paths: &[PathConstraint],
        case: CaseMode,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        if content.len() > MAX_NATIVE_CONTENT_REGEX_LEAVES || paths.len() > MAX_NATIVE_PATH_FILTERS
        {
            return Err(IndexError::ResourceLimited);
        }
        let count = content
            .len()
            .checked_add(paths.len())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = content
            .len()
            .checked_mul(std::mem::size_of::<ContentRegex>())
            .and_then(|bytes| {
                paths
                    .len()
                    .checked_mul(std::mem::size_of::<PathRegex>())
                    .and_then(|path| bytes.checked_add(path))
            })
            .and_then(|bytes| {
                count
                    .checked_mul(std::mem::size_of::<RegexMemory>())
                    .and_then(|leases| bytes.checked_add(leases))
            })
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let container_memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(bytes, MemoryLifetime::Temporary)?
        };
        let mut program = Self {
            content: Vec::new(),
            paths: Vec::new(),
            metadata: RecordMetadataFilters::default(),
            engine_memory: Vec::new(),
            _container_memory: container_memory,
        };
        program
            .content
            .try_reserve_exact(content.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        program
            .paths
            .try_reserve_exact(paths.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        program
            .engine_memory
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        for source in content {
            let (executor, estimated_states, memory) =
                compile_one(source, Some(case), false, budget)?;
            program.content.push(ContentRegex {
                executor,
                estimated_states,
            });
            program.engine_memory.push(memory);
        }
        for path in paths {
            let (executor, estimated_states, memory) = compile_field(&path.pattern, false, budget)?;
            program.paths.push(PathRegex {
                executor,
                scope: path.scope,
                estimated_states,
            });
            program.engine_memory.push(memory);
        }
        Ok(program)
    }

    pub(super) fn has_content(&self) -> bool {
        !self.content.is_empty()
    }

    /// Whether a candidate's Source record must be read to decide its
    /// `file:`, `repo:` or `lang:` filters.
    pub(super) fn has_record_filters(&self) -> bool {
        !self.paths.is_empty()
            || !self.metadata.repositories.is_empty()
            || !self.metadata.languages.is_empty()
    }

    /// Adds the `repo:` and `lang:` record filters; see
    /// [`RecordMetadataFilters`] for their semantics.
    pub(super) fn with_record_metadata(
        mut self,
        repositories: &[RepoConstraint],
        languages: &[String],
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        if repositories.len() > MAX_NATIVE_REPO_FILTERS {
            return Err(IndexError::ResourceLimited);
        }
        if repositories
            .iter()
            .any(|repository| !repository.revs.is_empty())
        {
            return Err(IndexError::InvalidInput(
                "native repo filter revisions require a history producer".into(),
            ));
        }
        let language_bytes = languages.iter().try_fold(0usize, |total, language| {
            total
                .checked_add(language.len())
                .ok_or(IndexError::ResourceLimited)
        })?;
        let bytes = repositories
            .len()
            .checked_mul(std::mem::size_of::<RepositoryRegex>())
            .and_then(|bytes| {
                repositories
                    .len()
                    .checked_mul(std::mem::size_of::<RegexMemory>())
                    .and_then(|leases| bytes.checked_add(leases))
            })
            .and_then(|bytes| {
                languages
                    .len()
                    .checked_mul(std::mem::size_of::<LanguageCode>())
                    .and_then(|codes| bytes.checked_add(codes))
            })
            .and_then(|bytes| bytes.checked_add(language_bytes))
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(bytes, MemoryLifetime::Temporary)?
        };
        let mut compiled = Vec::new();
        compiled
            .try_reserve_exact(repositories.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut codes = Vec::new();
        codes
            .try_reserve_exact(languages.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        self.engine_memory
            .try_reserve(repositories.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        for repository in repositories {
            if repository.pattern.is_empty() {
                return Err(IndexError::InvalidInput(
                    "native repo filter requires a pattern".into(),
                ));
            }
            let (executor, estimated_states, memory) =
                compile_field(&repository.pattern, false, budget)?;
            compiled.push(RepositoryRegex {
                executor,
                estimated_states,
            });
            self.engine_memory.push(memory);
        }
        for language in languages {
            budget.checkpoint()?;
            let code = LanguageCode::new(language.as_str()).map_err(|error| {
                IndexError::InvalidInput(format!("native lang filter: {error}"))
            })?;
            codes.push(code);
        }
        self.metadata = RecordMetadataFilters {
            repositories: compiled,
            languages: codes,
            _memory: memory,
        };
        Ok(self)
    }

    fn matches_metadata(
        &self,
        repository: Option<&RepoId>,
        language: Option<&LanguageCode>,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        let mut matched = true;
        if !self.metadata.repositories.is_empty() {
            let repository = repository.ok_or_else(|| {
                IndexError::InvalidInput(
                    "native repo filter requires repository metadata on every selected source record"
                        .into(),
                )
            })?;
            let scan_len = u64::try_from(repository.as_str().len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            let bytes = self
                .metadata
                .repositories
                .iter()
                .try_fold(0u64, |total, filter| {
                    scan_len
                        .max(1)
                        .checked_mul(filter.estimated_states)
                        .and_then(|work| total.checked_add(work))
                        .ok_or(IndexError::ResourceLimited)
                })?;
            budget.charge(WorkUsage {
                bytes,
                ..WorkUsage::default()
            })?;
            for filter in &self.metadata.repositories {
                budget.checkpoint()?;
                matched &= filter.executor.is_match(repository.as_str().as_bytes());
            }
        }
        if !self.metadata.languages.is_empty() {
            let language = language.ok_or_else(|| {
                IndexError::InvalidInput(
                    "native lang filter requires language metadata on every selected source record"
                        .into(),
                )
            })?;
            let bytes = self
                .metadata
                .languages
                .iter()
                .try_fold(0usize, |total, code| {
                    total
                        .checked_add(code.as_str().len())
                        .ok_or(IndexError::ResourceLimited)
                })
                .and_then(|bytes| {
                    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
                })?;
            budget.charge(WorkUsage {
                bytes,
                ..WorkUsage::default()
            })?;
            budget.checkpoint()?;
            matched &= self.metadata.languages.contains(language);
        }
        Ok(matched)
    }

    /// Whether `path` satisfies every `file:` filter, for the path census.
    pub(super) fn matches_path_filters(
        &self,
        path: &str,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        self.matches_path(path, budget)
    }

    fn matches_path(&self, path: &str, budget: &mut WorkBudget) -> Result<bool, IndexError> {
        let file_name = path
            .rsplit('/')
            .next()
            .ok_or_else(|| IndexError::CorruptObject("native path lost file name".into()))?;
        let scan_len = path
            .len()
            .checked_add(file_name.len())
            .ok_or(IndexError::ResourceLimited)?;
        let scan_len = u64::try_from(scan_len).map_err(|_error| IndexError::ResourceLimited)?;
        let bytes = self.paths.iter().try_fold(0u64, |total, filter| {
            scan_len
                .checked_mul(filter.estimated_states)
                .and_then(|work| total.checked_add(work))
                .ok_or(IndexError::ResourceLimited)
        })?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let mut matched = true;
        for filter in &self.paths {
            budget.checkpoint()?;
            let path_match = filter.executor.is_match(path.as_bytes());
            let name_match = filter.executor.is_match(file_name.as_bytes());
            matched &= match filter.scope {
                FilePathScope::NameAndPath => path_match || name_match,
                FilePathScope::NameOnly => name_match,
                FilePathScope::PathOnly => path_match,
            };
        }
        budget.checkpoint()?;
        Ok(matched)
    }

    fn evaluate_text(
        &self,
        source_text: &str,
        path_match: bool,
        budget: &mut WorkBudget,
    ) -> Result<(u8, bool), IndexError> {
        if !path_match || self.content.is_empty() {
            return Ok((0, path_match));
        }
        let source_len =
            u64::try_from(source_text.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let normalization_bytes = source_len
            .checked_mul(8)
            .ok_or(IndexError::ResourceLimited)?;
        let _normalized_memory = if normalization_bytes == 0 {
            None
        } else {
            budget.reserve_memory(normalization_bytes, MemoryLifetime::Temporary)?
        };
        budget.checkpoint()?;
        let normalized = normalize::nfc(source_text);
        let normalized_len =
            u64::try_from(normalized.len()).map_err(|_error| IndexError::ResourceLimited)?;
        if normalized_len > normalization_bytes {
            return Err(IndexError::ResourceLimited);
        }
        let scan_bytes = self.content.iter().try_fold(0u64, |total, filter| {
            normalized_len
                .max(1)
                .checked_mul(filter.estimated_states)
                .and_then(|work| total.checked_add(work))
                .ok_or(IndexError::ResourceLimited)
        })?;
        budget.charge(WorkUsage {
            bytes: scan_bytes,
            ..WorkUsage::default()
        })?;
        let mut mask = 0u8;
        for (ordinal, filter) in self.content.iter().enumerate() {
            budget.checkpoint()?;
            if filter.executor.verify(normalized.as_bytes()) {
                mask |= 1u8
                    .checked_shl(
                        u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        budget.checkpoint()?;
        Ok((mask, path_match))
    }

    /// Evaluate one overlay row. `repo:` and `lang:` use the row's
    /// authenticated metadata under the same fail-closed rule as a durable
    /// record: a filter whose metadata the row lacks refuses the query.
    ///
    /// With `rank`, the row's body and path are also ranked.
    pub(super) fn evaluate_overlay(
        &self,
        source: &TextOverlaySource<'_>,
        rank: Option<&CodeSearchRank<'_>>,
        budget: &mut WorkBudget,
    ) -> Result<(u8, bool, Option<RankedFile>), IndexError> {
        budget.checkpoint()?;
        let (path, body) = (source.path, source.body);
        if path.is_empty()
            || path.len() > MAX_SOURCE_PATH_BYTES
            || path.starts_with('/')
            || path.contains('\0')
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || body.len() > MAX_TEXT_OVERLAY_DOCUMENT_BYTES
        {
            return Err(IndexError::InvalidInput(
                "native regex overlay source exceeds admitted shape".into(),
            ));
        }
        let metadata_match = self.matches_metadata(source.repository, source.language, budget)?;
        let path_match = self.matches_path(path, budget)? && metadata_match;
        let (mask, path_match) = self.evaluate_text(body, path_match, budget)?;
        let ranked = rank
            .map(|rank| rank.score(Some(body), path, budget))
            .transpose()?
            .flatten();
        Ok((mask, path_match, ranked))
    }

    /// The posting or admitted-universe cursor established `source`; recheck
    /// its selected owner and immutable edges before inspecting path or bytes.
    ///
    /// With `rank`, the verified body and authenticated path are also ranked
    /// when the body is read.
    pub(super) fn evaluate(
        &self,
        index: &NativeTextIndex,
        owner: OwnerKey,
        source: ObjectRef,
        allow_excluded_path: bool,
        rank: Option<&CodeSearchRank<'_>>,
        context: &mut TextReadContext<'_>,
    ) -> Result<(u8, bool, Option<RankedFile>), IndexError> {
        context.budget.checkpoint()?;
        if context.maps.lookup_object(
            context.ticket.view.source.owners,
            &owner_map_key(owner),
            context.budget,
        )? != Some(source)
        {
            return Err(IndexError::CorruptObject(
                "native regex source owner differs from selected view".into(),
            ));
        }
        let stored = context.objects.get(source, context.budget)?;
        let record = SourceRecordOwner::decode_json(
            &stored.payload,
            context.budget,
            MemoryLifetime::Temporary,
        )?;
        let admitted = index.validate_selected_source(&record)?;
        if stored.registration.reference != source
            || stored.registration.children.as_slice() != [record.bytes]
            || (!admitted && !allow_excluded_path)
            || record.path.is_empty()
            || record.path.starts_with('/')
            || record.path.contains('\0')
            || record
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(IndexError::CorruptObject(
                "native regex source record differs from admitted selection".into(),
            ));
        }
        let metadata_match = self.matches_metadata(
            record.source_repo_id.as_ref(),
            record.language.as_ref(),
            context.budget,
        )?;
        let path_match = self.matches_path(&record.path, context.budget)? && metadata_match;
        // Only admitted sources that passed record and path filters need a body read.
        if !path_match || self.content.is_empty() || !admitted {
            return Ok((0, path_match, None));
        }
        let body = context.objects.get(record.bytes, context.budget)?;
        if body.registration.reference != record.bytes
            || !body.registration.children.is_empty()
            || u64::try_from(body.payload.len()).map_err(|_error| IndexError::ResourceLimited)?
                != record.byte_len
        {
            return Err(IndexError::CorruptObject(
                "native regex source bytes differ from admitted leaf".into(),
            ));
        }
        let source_text = std::str::from_utf8(&body.payload).map_err(|_error| {
            IndexError::CorruptObject(
                "native regex admitted UTF-8 source contains invalid bytes".into(),
            )
        })?;
        let (mask, path_match) = self.evaluate_text(source_text, path_match, context.budget)?;
        let ranked = rank
            .map(|rank| rank.score(Some(source_text), &record.path, context.budget))
            .transpose()?
            .flatten();
        Ok((mask, path_match, ranked))
    }
}

#[cfg(test)]
#[path = "text_regex/tests.rs"]
mod tests;
