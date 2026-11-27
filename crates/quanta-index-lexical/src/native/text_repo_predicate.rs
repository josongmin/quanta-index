//! Same-cut repository witnesses for Native text Boolean clauses.

use super::text::{NativeTextIndex, TextReadContext};
use super::text_regex::{RegexMemory, compile_field, compile_one};
use crate::plan::{RepoFileConstraint, RepoFileMatcher};
use quanta_index_contract::RepoId;
use quanta_index_core::index::{
    IndexError, MAX_SOURCE_PATH_BYTES, MapPageEntry, MemoryLease, MemoryLifetime, ObjectRef,
    OwnerKey, SourcePathOwners, SourceRecordOwner, WorkBudget, WorkUsage, owner_map_key,
    visit_repository_path_sources,
};
use quanta_index_lq_regex::{FieldRegex, RegexExecutor};
use quanta_index_lq_text_normalizer::{CaseMode, Token, TokenCursor, apply_case, query_tokens};

const PATH_PAGE: u32 = 128;
const MAX_REPO_LEAVES: usize = 4;
const MAX_MATCHERS: usize = 4;
/// A `content:` regex: a document engine searching one source body.
struct PreparedRegex {
    executor: RegexExecutor,
    states: u64,
    _engine: RegexMemory,
}

impl PreparedRegex {
    fn new(source: &str, case: CaseMode, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let (executor, states, engine) = compile_one(source, Some(case), false, budget)?;
        Ok(Self {
            executor,
            states,
            _engine: engine,
        })
    }

    fn matches(&self, source: &[u8], budget: &mut WorkBudget) -> Result<bool, IndexError> {
        let bytes = u64::try_from(source.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(self.states)
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        budget.checkpoint()?;
        Ok(self.executor.verify(source))
    }
}

/// A whole-value `path:` or `name:` pattern. It tests one short path or file
/// name, so it is a field engine charged its measured heap.
struct PreparedField {
    executor: FieldRegex,
    states: u64,
    _engine: RegexMemory,
}

impl PreparedField {
    fn new(source: &str, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let (executor, states, engine) = compile_field(source, true, budget)?;
        Ok(Self {
            executor,
            states,
            _engine: engine,
        })
    }

    fn matches(&self, value: &[u8], budget: &mut WorkBudget) -> Result<bool, IndexError> {
        let bytes = u64::try_from(value.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(self.states)
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        budget.checkpoint()?;
        Ok(self.executor.is_match(value))
    }
}

enum Matcher {
    Path(PreparedField),
    Name(PreparedField),
    Language {
        value: String,
        _memory: Option<MemoryLease>,
    },
    /// Boxed: a document engine with its search cache is far larger than
    /// the other matchers.
    ContentRegex(Box<PreparedRegex>),
    ContentTokens {
        tokens: Vec<Token>,
        _memory: Option<MemoryLease>,
    },
}

impl Matcher {
    fn new(
        matcher: &RepoFileMatcher,
        case: CaseMode,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        match matcher {
            RepoFileMatcher::Path(value) => Ok(Self::Path(PreparedField::new(value, budget)?)),
            RepoFileMatcher::Name(value) => Ok(Self::Name(PreparedField::new(value, budget)?)),
            RepoFileMatcher::Language(value) => {
                // Fund both trim scans and ASCII normalization before inspecting
                // caller text, including whitespace removed from the result.
                let work_bytes = u64::try_from(value.len())
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .checked_mul(3)
                    .ok_or(IndexError::ResourceLimited)?;
                budget.charge(WorkUsage {
                    bytes: work_bytes,
                    ..WorkUsage::default()
                })?;
                let bytes = u64::try_from(value.trim().len())
                    .map_err(|_error| IndexError::ResourceLimited)?;
                let memory = if bytes == 0 {
                    None
                } else {
                    budget.reserve_memory(bytes, MemoryLifetime::Temporary)?
                };
                let value =
                    crate::metadata_normalize::normalize_language(value).ok_or_else(|| {
                        IndexError::InvalidInput("native repo.has.file language is empty".into())
                    })?;
                if u64::try_from(value.capacity()).map_err(|_error| IndexError::ResourceLimited)?
                    > bytes
                {
                    return Err(IndexError::ResourceLimited);
                }
                Ok(Self::Language {
                    value,
                    _memory: memory,
                })
            }
            RepoFileMatcher::Content(value) => {
                if let Some(pattern) = value
                    .strip_prefix('/')
                    .and_then(|body| body.strip_suffix('/'))
                {
                    Ok(Self::ContentRegex(Box::new(PreparedRegex::new(
                        pattern, case, budget,
                    )?)))
                } else {
                    let token_bytes = u64::try_from(value.len())
                        .map_err(|_error| IndexError::ResourceLimited)?
                        .checked_mul(64)
                        .ok_or(IndexError::ResourceLimited)?;
                    budget.charge(WorkUsage {
                        bytes: token_bytes,
                        ..WorkUsage::default()
                    })?;
                    let memory = budget.reserve_memory(token_bytes, MemoryLifetime::Temporary)?;
                    let tokens = query_tokens(value, case)
                        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
                    if tokens.len() > 64 {
                        return Err(IndexError::ResourceLimited);
                    }
                    Ok(Self::ContentTokens {
                        tokens,
                        _memory: memory,
                    })
                }
            }
        }
    }

    fn matches_path(&self, path: &str, budget: &mut WorkBudget) -> Result<bool, IndexError> {
        match self {
            Self::Path(regex) => regex.matches(path.as_bytes(), budget),
            Self::Name(regex) => {
                regex.matches(path.rsplit('/').next().unwrap_or(path).as_bytes(), budget)
            }
            Self::Language { .. } | Self::ContentRegex(_) | Self::ContentTokens { .. } => Ok(true),
        }
    }

    fn matches_language(
        &self,
        record: &SourceRecordOwner,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        let Self::Language { value, .. } = self else {
            return Ok(true);
        };
        let language = record.language.as_ref().ok_or_else(|| {
            IndexError::InvalidInput(
                "native repo.has.file requires authenticated language metadata".into(),
            )
        })?;
        let bytes = value
            .len()
            .checked_add(language.as_str().len())
            .ok_or(IndexError::ResourceLimited)
            .and_then(|bytes| u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited))?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        budget.checkpoint()?;
        Ok(value == language.as_str())
    }

    fn matches_content(
        &self,
        body: &str,
        case: CaseMode,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        match self {
            Self::Path(_) | Self::Name(_) | Self::Language { .. } => Ok(true),
            Self::ContentRegex(regex) => {
                let body_bytes =
                    u64::try_from(body.len()).map_err(|_error| IndexError::ResourceLimited)?;
                let backing = body_bytes
                    .checked_mul(8)
                    .ok_or(IndexError::ResourceLimited)?;
                budget.charge(WorkUsage {
                    bytes: backing,
                    ..WorkUsage::default()
                })?;
                let _memory = if backing == 0 {
                    None
                } else {
                    budget.reserve_memory(backing, MemoryLifetime::Temporary)?
                };
                budget.checkpoint()?;
                let normalized = crate::normalize::nfc(body);
                if u64::try_from(normalized.len()).map_err(|_error| IndexError::ResourceLimited)?
                    > backing
                {
                    return Err(IndexError::ResourceLimited);
                }
                regex.matches(normalized.as_bytes(), budget)
            }
            Self::ContentTokens { tokens, .. } => {
                let admission = std::cell::RefCell::new(&mut *budget);
                let mut cursor = TokenCursor::try_new_bounded(
                    body,
                    16 * 1024 * 1024,
                    64 * 1024 * 1024,
                    |units| {
                        admission.borrow_mut().charge(WorkUsage {
                            bytes: units,
                            ..WorkUsage::default()
                        })
                    },
                    |bytes| {
                        admission.borrow().reserve_memory(
                            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                            MemoryLifetime::Temporary,
                        )
                    },
                )
                .map_err(|error| {
                    error
                        .into_checkpoint()
                        .map_or(IndexError::ResourceLimited, std::convert::identity)
                })?;
                let mut states = 0u64;
                let last = 1u64
                    .checked_shl(
                        u32::try_from(
                            tokens
                                .len()
                                .checked_sub(1)
                                .ok_or(IndexError::ResourceLimited)?,
                        )
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?;
                while let Some(run) = cursor.next_run() {
                    budget.checkpoint()?;
                    if !run.is_indexable() {
                        states = 0;
                        continue;
                    }
                    let scan = u64::try_from(run.text.len())
                        .map_err(|_error| IndexError::ResourceLimited)?
                        .checked_mul(
                            u64::try_from(tokens.len())
                                .map_err(|_error| IndexError::ResourceLimited)?,
                        )
                        .ok_or(IndexError::ResourceLimited)?;
                    budget.charge(WorkUsage {
                        bytes: scan,
                        ..WorkUsage::default()
                    })?;
                    let _case_memory = budget.reserve_memory(scan, MemoryLifetime::Temporary)?;
                    let comparable = apply_case(run.text, case);
                    let mut equal = 0u64;
                    for (position, token) in tokens.iter().enumerate() {
                        if token.text == comparable {
                            equal |= 1u64 << position;
                        }
                    }
                    states = ((states << 1) | 1) & equal;
                    if states & last != 0 {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
}

struct Witnesses {
    rows: Vec<(RepoId, Option<MemoryLease>)>,
    rows_memory: Option<MemoryLease>,
}

impl Witnesses {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            rows_memory: None,
        }
    }

    fn insert(&mut self, repo: &RepoId, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if self.rows.len() == self.rows.capacity() {
            let capacity = self
                .rows
                .capacity()
                .max(4)
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?;
            let bytes = capacity
                .checked_mul(std::mem::size_of::<(RepoId, Option<MemoryLease>)>())
                .ok_or(IndexError::ResourceLimited)
                .and_then(|bytes| {
                    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
                })?;
            let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
            let mut replacement = Vec::new();
            replacement
                .try_reserve_exact(capacity)
                .map_err(|_error| IndexError::ResourceLimited)?;
            if replacement.capacity() > capacity {
                return Err(IndexError::ResourceLimited);
            }
            replacement.append(&mut self.rows);
            self.rows = replacement;
            self.rows_memory = memory;
        }
        let bytes =
            u64::try_from(repo.as_str().len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        self.rows.push((repo.clone(), memory));
        Ok(())
    }

    fn finish(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        let rows = u64::try_from(self.rows.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let key_bytes = self.rows.iter().try_fold(0u64, |sum, (repo, _)| {
            sum.checked_add(
                u64::try_from(repo.as_str().len()).map_err(|_error| IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)
        })?;
        let comparisons = u64::from(rows.checked_ilog2().unwrap_or(0))
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let work = rows
            .checked_add(key_bytes)
            .and_then(|bytes| bytes.checked_mul(comparisons))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: work,
            ..WorkUsage::default()
        })?;
        budget.checkpoint()?;
        self.rows
            .sort_unstable_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
        self.rows.dedup_by(|left, right| left.0 == right.0);
        Ok(())
    }

    fn contains(&self, repo: &RepoId, budget: &mut WorkBudget) -> Result<bool, IndexError> {
        let comparisons = u64::from(self.rows.len().checked_ilog2().unwrap_or(0))
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = comparisons
            .checked_mul(
                u64::try_from(repo.as_str().len()).map_err(|_error| IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        Ok(self
            .rows
            .binary_search_by(|entry| entry.0.as_str().cmp(repo.as_str()))
            .is_ok())
    }
}

struct Predicate {
    matchers: Vec<Matcher>,
    witnesses: Witnesses,
    _matchers_memory: Option<MemoryLease>,
}

pub(super) struct RepoPredicateProgram {
    predicates: Vec<Predicate>,
    _predicates_memory: Option<MemoryLease>,
}

impl RepoPredicateProgram {
    pub(super) fn prepare(
        constraints: &[Option<&RepoFileConstraint>],
        case: CaseMode,
        index: &NativeTextIndex,
        context: &mut TextReadContext<'_>,
    ) -> Result<Self, IndexError> {
        if constraints.len() > MAX_REPO_LEAVES {
            return Err(IndexError::ResourceLimited);
        }
        if constraints.is_empty() {
            return Ok(Self {
                predicates: Vec::new(),
                _predicates_memory: None,
            });
        }
        let bytes = constraints
            .len()
            .checked_mul(std::mem::size_of::<Predicate>())
            .ok_or(IndexError::ResourceLimited)
            .and_then(|bytes| u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited))?;
        let memory = context
            .budget
            .reserve_memory(bytes, MemoryLifetime::Temporary)?;
        let mut predicates = Vec::new();
        predicates
            .try_reserve_exact(constraints.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        if predicates.capacity() > constraints.len() {
            return Err(IndexError::ResourceLimited);
        }
        for constraint in constraints {
            let constraint = constraint.ok_or_else(|| {
                IndexError::BackendViolation("repository constraint missing".into())
            })?;
            if constraint.matchers.is_empty() || constraint.matchers.len() > MAX_MATCHERS {
                return Err(IndexError::InvalidInput(
                    "native repo.has.file matcher count is unsupported".into(),
                ));
            }
            let bytes = constraint
                .matchers
                .len()
                .checked_mul(std::mem::size_of::<Matcher>())
                .ok_or(IndexError::ResourceLimited)
                .and_then(|bytes| {
                    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
                })?;
            let matcher_memory = context
                .budget
                .reserve_memory(bytes, MemoryLifetime::Temporary)?;
            let mut matchers = Vec::new();
            matchers
                .try_reserve_exact(constraint.matchers.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            if matchers.capacity() > constraint.matchers.len() {
                return Err(IndexError::ResourceLimited);
            }
            for matcher in &constraint.matchers {
                matchers.push(Matcher::new(matcher, case, context.budget)?);
            }
            predicates.push(Predicate {
                matchers,
                witnesses: Witnesses::new(),
                _matchers_memory: matcher_memory,
            });
        }
        let mut output = Self {
            predicates,
            _predicates_memory: memory,
        };
        output.scan_witnesses(index, case, context)?;
        for predicate in &mut output.predicates {
            predicate.witnesses.finish(context.budget)?;
        }
        Ok(output)
    }

    fn scan_witnesses(
        &mut self,
        index: &NativeTextIndex,
        case: CaseMode,
        context: &mut TextReadContext<'_>,
    ) -> Result<(), IndexError> {
        if self.predicates.is_empty() {
            return Ok(());
        }
        let source = context.ticket.view.source;
        let mut after: Option<MapPageEntry> = None;
        loop {
            context.budget.checkpoint()?;
            let page = context.maps.scan(
                source.paths,
                after.as_ref().map(MapPageEntry::key).transpose()?,
                PATH_PAGE,
                context.budget,
            )?;
            if page.entries.is_empty() && page.has_more
                || page.entries.len()
                    > usize::try_from(PATH_PAGE).map_err(|_error| IndexError::ResourceLimited)?
            {
                return Err(IndexError::BackendViolation(
                    "repository path scan did not advance".into(),
                ));
            }
            let has_more = page.has_more;
            for entry in page.cursor() {
                let (key, marker) = entry.get()?;
                if after
                    .as_ref()
                    .map(MapPageEntry::key)
                    .transpose()?
                    .is_some_and(|previous| key.as_slice() <= previous)
                {
                    return Err(IndexError::CorruptObject(
                        "repository path order changed".into(),
                    ));
                }
                let path_bytes = key.strip_prefix(b"path\0").ok_or_else(|| {
                    IndexError::CorruptObject("repository path key is invalid".into())
                })?;
                let path = std::str::from_utf8(path_bytes).map_err(|_error| {
                    IndexError::CorruptObject("repository path key is invalid".into())
                })?;
                if path.is_empty()
                    || path.len() > MAX_SOURCE_PATH_BYTES
                    || path.starts_with('/')
                    || path.contains('\0')
                    || path
                        .split('/')
                        .any(|part| part.is_empty() || part == "." || part == "..")
                {
                    return Err(IndexError::CorruptObject(
                        "repository path has invalid relative shape".into(),
                    ));
                }
                match SourcePathOwners::decode(*marker, context.budget)? {
                    SourcePathOwners::LegacyOwner(_) => {
                        return Err(IndexError::InvalidInput(
                            "native repo.has.file requires repository identity in selected cut"
                                .into(),
                        ));
                    }
                    SourcePathOwners::Repositories(root) => {
                        let objects = context.objects;
                        let maps = context.maps;
                        visit_repository_path_sources(
                            root,
                            path,
                            source.owners,
                            maps,
                            objects,
                            context.budget,
                            &mut |_owner, _reference, record, budget| {
                                let repo = record.source_repo_id.as_ref().ok_or_else(|| {
                                    IndexError::InvalidInput(
                                        "native repo.has.file requires repository identity".into(),
                                    )
                                })?;
                                let mut admitted_body = None;
                                for predicate in &mut self.predicates {
                                    if !predicate
                                        .matchers
                                        .iter()
                                        .filter(|matcher| {
                                            matches!(matcher, Matcher::Path(_) | Matcher::Name(_))
                                        })
                                        .try_fold(true, |matched, matcher| {
                                            Ok::<_, IndexError>(
                                                matched && matcher.matches_path(path, budget)?,
                                            )
                                        })?
                                    {
                                        continue;
                                    }
                                    let mut language_matches = true;
                                    for matcher in &predicate.matchers {
                                        if !matcher.matches_language(&record, budget)? {
                                            language_matches = false;
                                            break;
                                        }
                                    }
                                    if !language_matches {
                                        continue;
                                    }
                                    let needs_content = predicate.matchers.iter().any(|matcher| {
                                        matches!(
                                            matcher,
                                            Matcher::ContentRegex(_)
                                                | Matcher::ContentTokens { .. }
                                        )
                                    });
                                    if needs_content {
                                        if !index.validate_selected_source(&record)? {
                                            continue;
                                        }
                                        if admitted_body.is_none() {
                                            admitted_body =
                                                Some(objects.get(record.bytes, budget)?);
                                        }
                                        let body = admitted_body.as_ref().ok_or_else(|| {
                                            IndexError::BackendViolation(
                                                "repository witness body missing".into(),
                                            )
                                        })?;
                                        if body.registration.reference != record.bytes
                                            || !body.registration.children.is_empty()
                                            || u64::try_from(body.payload.len())
                                                .map_err(|_error| IndexError::ResourceLimited)?
                                                != record.byte_len
                                        {
                                            return Err(IndexError::CorruptObject("repository witness bytes differ from selected source".into()));
                                        }
                                        let text = std::str::from_utf8(&body.payload).map_err(
                                            |_error| {
                                                IndexError::CorruptObject(
                                                    "repository witness admitted text is not UTF-8"
                                                        .into(),
                                                )
                                            },
                                        )?;
                                        let mut matched = true;
                                        for matcher in &predicate.matchers {
                                            if !matcher.matches_content(text, case, budget)? {
                                                matched = false;
                                                break;
                                            }
                                        }
                                        if !matched {
                                            continue;
                                        }
                                    }
                                    predicate.witnesses.insert(repo, budget)?;
                                }
                                Ok(())
                            },
                        )?;
                    }
                }
                after = Some(entry);
            }
            if !has_more {
                break;
            }
        }
        Ok(())
    }

    pub(super) fn mask(
        &self,
        index: &NativeTextIndex,
        owner: OwnerKey,
        source: ObjectRef,
        context: &mut TextReadContext<'_>,
    ) -> Result<u8, IndexError> {
        if self.predicates.is_empty() {
            return Ok(0);
        }
        if context.maps.lookup_object(
            context.ticket.view.source.owners,
            &owner_map_key(owner),
            context.budget,
        )? != Some(source)
        {
            return Err(IndexError::CorruptObject(
                "repository candidate differs from selected source".into(),
            ));
        }
        let stored = context.objects.get(source, context.budget)?;
        let record = SourceRecordOwner::decode_json(
            &stored.payload,
            context.budget,
            MemoryLifetime::Temporary,
        )?;
        if stored.registration.reference != source
            || stored.registration.children.as_slice() != [record.bytes]
            || !index.validate_selected_source(&record)?
        {
            return Err(IndexError::CorruptObject(
                "repository candidate source is not admitted".into(),
            ));
        }
        let repo = record.source_repo_id.as_ref().ok_or_else(|| {
            IndexError::InvalidInput(
                "native repo.has.file requires repository identity in selected cut".into(),
            )
        })?;
        let mut mask = 0u8;
        for (ordinal, predicate) in self.predicates.iter().enumerate() {
            if predicate.witnesses.contains(repo, context.budget)? {
                mask |= 1u8 << ordinal;
            }
        }
        Ok(mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::WorkLimits;
    use std::time::{Duration, Instant};

    #[test]
    fn repo_file_language_normalization_funds_removed_input_before_allocation()
    -> Result<(), IndexError> {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1,
                writes: 0,
                bytes: 32,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let caller_text = format!("{}RuSt{}", " ".repeat(128), " ".repeat(128));
        // A four-byte result cannot hide the work needed to inspect the much
        // larger input. Refusal happens before normalization backing is built.
        assert!(matches!(
            Matcher::new(
                &RepoFileMatcher::Language(caller_text),
                CaseMode::Sensitive,
                &mut budget
            ),
            Err(IndexError::ResourceLimited)
        ));
        Ok(())
    }

    struct Meter(std::sync::atomic::AtomicU64);

    impl quanta_index_core::index::MemoryLeaseOwner for Meter {
        fn release(&self, bytes: u64) {
            let _prior = self.0.fetch_sub(bytes, std::sync::atomic::Ordering::SeqCst);
        }
    }

    struct MeterScope(std::sync::Arc<Meter>);

    impl quanta_index_core::index::MemoryScopePort for MeterScope {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let _prior = self
                .0
                .0
                .fetch_add(bytes, std::sync::atomic::Ordering::SeqCst);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    /// `path:` and `name:` matchers test one short path or file name, so they
    /// are field engines charged their measured heap, far below one document
    /// engine's grant. They keep whole-value matching, and every grant ends
    /// with the matchers.
    #[test]
    fn path_and_name_matchers_charge_their_measured_field_engines() -> Result<(), IndexError> {
        let meter = std::sync::Arc::new(Meter(std::sync::atomic::AtomicU64::new(0)));
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: u64::MAX,
                writes: u64::MAX,
                bytes: u64::MAX,
                mutations: u64::MAX,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        budget.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(
            std::sync::Arc::new(MeterScope(meter.clone())),
        )));
        let matchers = [
            Matcher::new(
                &RepoFileMatcher::Path(r"src/.*[.]rs".into()),
                CaseMode::Sensitive,
                &mut budget,
            )?,
            Matcher::new(
                &RepoFileMatcher::Path("src/lib.rs".into()),
                CaseMode::Folded,
                &mut budget,
            )?,
            Matcher::new(
                &RepoFileMatcher::Name(r"\w+[.]rs".into()),
                CaseMode::Sensitive,
                &mut budget,
            )?,
            Matcher::new(
                &RepoFileMatcher::Name("main.rs".into()),
                CaseMode::Sensitive,
                &mut budget,
            )?,
        ];
        let reserved = meter.0.load(std::sync::atomic::Ordering::SeqCst);
        if reserved >= 1 << 20 || budget.usage().bytes >= 1 << 20 {
            return Err(IndexError::BackendViolation(format!(
                "four path matchers reserved {reserved} and charged {} bytes",
                budget.usage().bytes
            )));
        }
        let [nested, exact, any_name, main] = &matchers;
        for (matcher, path, expected) in [
            (nested, "src/lib.rs", true),
            (nested, "src/a/b.rs", true),
            (nested, "x/src/lib.rs", false),
            (nested, "src/lib.rs.bak", false),
            (exact, "src/lib.rs", true),
            (exact, "SRC/LIB.RS", false),
            (any_name, "src/lib.rs", true),
            (any_name, "src/lib.rs/x", false),
            (main, "src/main.rs", true),
            (main, "src/domain.rs", false),
        ] {
            if matcher.matches_path(path, &mut budget)? != expected {
                return Err(IndexError::BackendViolation(format!(
                    "path matcher disagrees on {path:?}"
                )));
            }
        }
        drop(matchers);
        if meter.0.load(std::sync::atomic::Ordering::SeqCst) != 0 {
            return Err(IndexError::BackendViolation(
                "path matcher grant outlived its matcher".into(),
            ));
        }
        Ok(())
    }
}
