use quanta_index_contract::{
    LqExpr, LqFilter, LqLeaf, LqPatternType, LqQuery, LqSelect, LqType, QueryConstraintSetV1,
};

use crate::error::CoreError;
use crate::timeref::is_rev_at_time_spec;

/// Wire code for a lexical execution that would have to examine more
/// candidates than the policy allows (QI-BB-005).
pub const LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE: &str = "LEXICAL_EXAMINED_BUDGET_EXCEEDED";

/// How much one lexical execution may materialize (QI-BB-005).
///
/// A page query never needs more than `top_k + 1` (the continuation probe),
/// but exact counts over projections, bounded counts with a deterministic
/// total order and explicit unindexed scans all need the whole match set.
/// Before this budget they collected `num_docs`, so a small request could
/// materialize the whole corpus. Now such an execution collects at most
/// `max_examined_candidates` documents plus one; if the extra one arrives,
/// the adapter refuses with [`LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE`] rather
/// than answer from a truncated set. Exact counts that need no
/// materialization (a count collector over an indexed query) are not subject
/// to it.
///
/// The field is private so every budget in existence is a valid one: zero
/// would refuse every exact-set query and is a configuration defect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LexicalExecutionBudgetV1 {
    max_examined_candidates: usize,
}

impl LexicalExecutionBudgetV1 {
    /// Deployment default. 250,000 keeps the worst case at a few hundred
    /// megabytes of document fetches, the ceiling one query may cost the
    /// process until W5's per-request byte ledger replaces the constant.
    pub const DEFAULT: Self = Self {
        max_examined_candidates: 250_000,
    };

    pub fn new(max_examined_candidates: usize) -> Result<Self, CoreError> {
        if max_examined_candidates == 0 {
            return Err(CoreError::InvalidContract(
                "lexical execution budget must allow at least one examined candidate".to_string(),
            ));
        }
        Ok(Self {
            max_examined_candidates,
        })
    }

    #[must_use]
    pub const fn max_examined_candidates(self) -> usize {
        self.max_examined_candidates
    }

    /// The typed refusal for an execution that would overrun this budget.
    /// `surface` names the execution (projection, bounded count, unindexed
    /// scan, ...) so the caller knows which part of the query to narrow.
    #[must_use]
    pub fn exceeded(self, surface: &str) -> CoreError {
        CoreError::Typed {
            code: LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE.to_string(),
            message: format!(
                "lexical: {surface} would examine more than {} candidates; narrow the query or drop the exact-set option",
                self.max_examined_candidates
            ),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LexicalPolicy;

impl LexicalPolicy {
    pub fn validate_query(query: &LqQuery) -> Result<(), CoreError> {
        Self::validate_query_inner(query, false)
    }

    /// Admit an empty symbol expression only when a validated exact-path
    /// constraint supplies the complete candidate-generation authority.
    pub fn validate_query_with_constraints(
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
    ) -> Result<(), CoreError> {
        Self::validate_query_inner(query, constraints.repo_relative_path_exact.is_some())
    }

    fn validate_query_inner(query: &LqQuery, allow_exact_path_only: bool) -> Result<(), CoreError> {
        let has_content_filter = query
            .filters
            .iter()
            .any(|filter| matches!(filter, LqFilter::Content { .. }));
        if matches!(query.expr, LqExpr::Empty) && !has_content_filter && !allow_exact_path_only {
            return Err(CoreError::InvalidContract(
                "lexical: empty query is rejected (must carry an expression or content filter)"
                    .to_string(),
            ));
        }
        if query.options.pattern_type == LqPatternType::Structural
            || expr_contains_structural(&query.expr)
            || filters_contain_structural(&query.filters)
        {
            return Err(CoreError::Typed {
                code: "STR_PRODUCER_PARSE_TREE_UNAVAILABLE".to_string(),
                message:
                    "lexical: structural execution is fail-closed until producer parse-tree ops land"
                        .to_string(),
                });
        }
        if query.options.timeout_ms.is_some() && !query_contains_timeout_executable_surface(query) {
            return Err(CoreError::InvalidContract(
                "lexical: timeout option is executable only for regex-backed lexical queries"
                    .to_string(),
            ));
        }
        validate_supported_filter_surface(query)?;
        Ok(())
    }
}

fn expr_contains_structural(expr: &LqExpr) -> bool {
    match expr {
        LqExpr::Empty => false,
        LqExpr::Leaf(leaf) => leaf_contains_structural(leaf),
        LqExpr::Not(inner) => expr_contains_structural(inner),
        LqExpr::All(children) | LqExpr::Any(children) => {
            children.iter().any(expr_contains_structural)
        }
    }
}

fn filters_contain_structural(filters: &[LqFilter]) -> bool {
    filters.iter().any(|filter| match filter {
        LqFilter::Content { leaf } => leaf_contains_structural(leaf),
        LqFilter::Repo { .. }
        | LqFilter::File { .. }
        | LqFilter::Lang { .. }
        | LqFilter::Rev { .. }
        | LqFilter::Author { .. }
        | LqFilter::Committer { .. }
        | LqFilter::Message { .. }
        | LqFilter::Before { .. }
        | LqFilter::After { .. }
        | LqFilter::Since { .. }
        | LqFilter::Until { .. }
        | LqFilter::DiffAdded { .. }
        | LqFilter::DiffRemoved { .. }
        | LqFilter::DiffTouched { .. }
        | LqFilter::Type { .. }
        | LqFilter::Select { .. }
        | LqFilter::Dirty { .. }
        | LqFilter::Changed { .. }
        | LqFilter::Stale { .. }
        | LqFilter::Snapshot { .. }
        | LqFilter::MetaOwner { .. }
        | LqFilter::MetaService { .. }
        | LqFilter::MetaLayer { .. }
        | LqFilter::MetaSurface { .. }
        | LqFilter::Affected { .. }
        | LqFilter::InvalidatedBy { .. }
        | LqFilter::Fork { .. }
        | LqFilter::Archived { .. }
        | LqFilter::Visibility { .. }
        | LqFilter::Context { .. } => false,
    })
}

fn leaf_contains_structural(leaf: &LqLeaf) -> bool {
    matches!(leaf, LqLeaf::StructuralBlock(_))
}

fn query_contains_timeout_executable_surface(query: &LqQuery) -> bool {
    expr_contains_timeout_executable_surface(&query.expr, query.options.pattern_type)
        || query.filters.iter().any(|filter| match filter {
            LqFilter::Content { leaf } => {
                leaf_contains_timeout_executable_surface(leaf, query.options.pattern_type)
            }
            LqFilter::Repo { .. }
            | LqFilter::File { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Rev { .. }
            | LqFilter::Author { .. }
            | LqFilter::Committer { .. }
            | LqFilter::Message { .. }
            | LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. }
            | LqFilter::Type { .. }
            | LqFilter::Select { .. }
            | LqFilter::Dirty { .. }
            | LqFilter::Changed { .. }
            | LqFilter::Stale { .. }
            | LqFilter::Snapshot { .. }
            | LqFilter::MetaOwner { .. }
            | LqFilter::MetaService { .. }
            | LqFilter::MetaLayer { .. }
            | LqFilter::MetaSurface { .. }
            | LqFilter::Affected { .. }
            | LqFilter::InvalidatedBy { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => false,
        })
}

fn expr_contains_timeout_executable_surface(expr: &LqExpr, pattern_type: LqPatternType) -> bool {
    match expr {
        LqExpr::Empty => false,
        LqExpr::Leaf(leaf) => leaf_contains_timeout_executable_surface(leaf, pattern_type),
        LqExpr::Not(inner) => expr_contains_timeout_executable_surface(inner, pattern_type),
        LqExpr::All(children) | LqExpr::Any(children) => children
            .iter()
            .any(|child| expr_contains_timeout_executable_surface(child, pattern_type)),
    }
}

fn leaf_contains_timeout_executable_surface(leaf: &LqLeaf, pattern_type: LqPatternType) -> bool {
    matches!(leaf, LqLeaf::Regex(_))
        || (pattern_type == LqPatternType::Regexp
            && matches!(leaf, LqLeaf::Keyword(_) | LqLeaf::RawString(_)))
}

fn validate_supported_filter_surface(query: &LqQuery) -> Result<(), CoreError> {
    for filter in &query.filters {
        match filter {
            LqFilter::Rev { spec } => {
                if is_rev_at_time_spec(spec) {
                    return Err(CoreError::NotImplemented(
                        "lexical: rev:at.time(...) requires revision-selection and pin rebinding before lexical execution".to_string(),
                    ));
                }
                return Err(CoreError::NotImplemented(
                    "lexical: rev filter is not executable on the current adapter set".to_string(),
                ));
            }
            LqFilter::Type { kind } => match kind {
                LqType::File | LqType::Path | LqType::Symbol | LqType::Repo => {}
                LqType::Commit | LqType::Diff => {
                    return Err(CoreError::NotImplemented(format!(
                        "lexical: type filter `{}` is not executable on the current adapter set",
                        kind.as_str()
                    )));
                }
            },
            LqFilter::Select { dim } => match dim {
                LqSelect::File
                | LqSelect::FileOwners
                | LqSelect::Path
                | LqSelect::Symbol
                | LqSelect::Content
                | LqSelect::ContentMatch
                | LqSelect::Repo => {}
            },
            LqFilter::Author { .. } => {
                return Err(CoreError::NotImplemented(
                    "lexical: author filter is not executable on the current adapter set"
                        .to_string(),
                ));
            }
            LqFilter::Committer { .. } => {
                return Err(CoreError::NotImplemented(
                    "lexical: committer filter is not executable on the current adapter set"
                        .to_string(),
                ));
            }
            LqFilter::Message { .. } => {
                return Err(CoreError::NotImplemented(
                    "lexical: message filter is not executable on the current adapter set"
                        .to_string(),
                ));
            }
            LqFilter::Dirty { .. } => {
                return Err(CoreError::NotImplemented(
                    "lexical: dirty filter is not executable on the current adapter set"
                        .to_string(),
                ));
            }
            LqFilter::Changed { .. }
            | LqFilter::Stale { .. }
            | LqFilter::Snapshot { .. }
            | LqFilter::MetaOwner { .. }
            | LqFilter::MetaService { .. }
            | LqFilter::MetaLayer { .. }
            | LqFilter::MetaSurface { .. }
            | LqFilter::Affected { .. }
            | LqFilter::InvalidatedBy { .. } => {
                return Err(CoreError::NotImplemented(
                    "lexical: runtime catalog filters are not executable on the current adapter set"
                        .to_string(),
                ));
            }
            LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. } => {
                return Err(CoreError::NotImplemented(
                    "lexical: history date/diff filters are not executable on the current adapter set"
                        .to_string(),
                ));
            }
            LqFilter::Repo { .. }
            | LqFilter::File { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Content { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! `cargo mutants` killed gaps:
    //! - line 16 `&& !has_content_filter` → `&& has_content_filter` (delete `!`)
    //! - line 23 first `||` → `&&` in the structural-rejection clause
    //! - line 24 second `||` → `&&` in the structural-rejection clause
    //!
    //! Each test below exercises an input where the original and mutated
    //! conditions disagree, so the observable `Ok`/`Err` outcome flips.

    use super::*;
    use quanta_index_contract::{
        LQ_VERSION_TAG, LqOptions, LqPatternType, LqQuery, LqSpan, LqStructuralBlock, LqYesNoOnly,
    };

    fn make_query(expr: LqExpr, filters: Vec<LqFilter>, options: LqOptions) -> LqQuery {
        LqQuery {
            lq_version: LQ_VERSION_TAG,
            expr,
            filters,
            directives: Vec::new(),
            options,
            source_span: LqSpan::eof(0),
        }
    }

    fn empty_query() -> LqQuery {
        make_query(LqExpr::Empty, Vec::new(), LqOptions::defaults())
    }

    #[test]
    fn empty_expr_without_content_filter_rejected_kills_bang_delete_mutation() {
        // Original: `Empty && !has_content_filter` = true && !false = true → Err.
        // Mutation (delete `!`): true && false = false → Ok. Asserting Err kills it.
        assert!(LexicalPolicy::validate_query(&empty_query()).is_err());
    }

    #[test]
    fn empty_expr_with_content_filter_accepted_kills_bang_delete_mutation_other_side() {
        // Original: true && !true = false → no early return → Ok.
        // Mutation (delete `!`): true && true = true → Err. Asserting Ok kills it.
        let mut q = empty_query();
        q.filters.push(LqFilter::Content {
            leaf: LqLeaf::Keyword("needle".to_string()),
        });
        assert!(LexicalPolicy::validate_query(&q).is_ok());
    }

    #[test]
    fn timeout_without_regex_backing_is_rejected() {
        let mut q = make_query(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            Vec::new(),
            LqOptions::defaults(),
        );
        q.options.timeout_ms = Some(1);
        let result = LexicalPolicy::validate_query(&q);
        assert!(matches!(result, Err(CoreError::InvalidContract(_))));
    }

    #[test]
    fn timeout_with_regex_backing_is_accepted() {
        let mut q = make_query(
            LqExpr::Leaf(LqLeaf::Regex("needle.*".to_string())),
            Vec::new(),
            LqOptions::defaults(),
        );
        q.options.timeout_ms = Some(0);
        assert!(LexicalPolicy::validate_query(&q).is_ok());
    }

    #[test]
    fn index_no_is_canonical_and_policy_admitted() {
        let mut q = make_query(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            Vec::new(),
            LqOptions::defaults(),
        );
        q.options.index_mode = Some(LqYesNoOnly::No);
        assert!(LexicalPolicy::validate_query(&q).is_ok());
    }

    #[test]
    fn boost_is_canonical_and_policy_admitted() {
        let mut q = make_query(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            Vec::new(),
            LqOptions::defaults(),
        );
        q.options.boost_millis = Some(2500);
        assert!(LexicalPolicy::validate_query(&q).is_ok());
    }

    #[test]
    fn structural_pattern_only_rejected_kills_first_or_to_and_mutation() {
        // Inputs: A=true (pattern_type==Structural), B=false (non-structural
        // expr), C=false (no structural filter).
        // Original: A || B || C = true → Err.
        // Mutation (first `||` → `&&`): (A && B) || C = (true && false) || false = false → Ok.
        let mut opts = LqOptions::defaults();
        opts.pattern_type = LqPatternType::Structural;
        let q = make_query(
            LqExpr::Leaf(LqLeaf::Keyword("anything".to_string())),
            Vec::new(),
            opts,
        );
        let result = LexicalPolicy::validate_query(&q);
        assert!(
            matches!(
                result,
                Err(CoreError::Typed { ref code, .. })
                    if code == "STR_PRODUCER_PARSE_TREE_UNAVAILABLE"
            ),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn structural_filter_only_rejected_kills_second_or_to_and_mutation() {
        // Inputs: A=false, B=false, C=true (structural leaf in a Content filter).
        // Original: false || false || true = true → Err.
        // Mutation (second `||` → `&&`): (A || B) && C = (false || false) && true = false → Ok.
        let q = make_query(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            vec![LqFilter::Content {
                leaf: LqLeaf::StructuralBlock(LqStructuralBlock {
                    lang: None,
                    nodes: Vec::new(),
                    exprs: Vec::new(),
                }),
            }],
            LqOptions::defaults(),
        );
        let result = LexicalPolicy::validate_query(&q);
        assert!(
            matches!(
                result,
                Err(CoreError::Typed { ref code, .. })
                    if code == "STR_PRODUCER_PARSE_TREE_UNAVAILABLE"
            ),
            "unexpected result: {result:?}"
        );
    }
}

/// Bounds for the regex match cache (QI-BB-024).
///
/// The cache used to be bounded by entry count alone, so 128 broad regexes
/// over a large corpus could own 128 copies of the corpus's candidate ids.
/// An entry is now a compressed bitmap of text-authority doc ids, and the
/// cache is bounded by the bytes its entries occupy and by the cardinality
/// of one entry; every bound is a refusal or an eviction the stats report,
/// never a silent growth. Fields are private so every policy is valid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegexMatchCachePolicy {
    entries: usize,
    resident_bytes: u64,
    matches_per_entry: usize,
}

impl RegexMatchCachePolicy {
    /// 128 entries, 64 MiB resident, and no single entry wider than the
    /// default examined-candidate budget: a regex that matched more than
    /// the budget allows to be examined would not have been served.
    pub const DEFAULT: Self = Self {
        entries: 128,
        resident_bytes: 64 * 1024 * 1024,
        matches_per_entry: 250_000,
    };

    pub fn new(
        max_entries: usize,
        max_resident_bytes: u64,
        max_matches_per_entry: usize,
    ) -> Result<Self, CoreError> {
        if max_entries == 0 || max_resident_bytes == 0 || max_matches_per_entry == 0 {
            return Err(CoreError::InvalidContract(
                "lexical: regex match cache policy limits must be non-zero".to_string(),
            ));
        }
        Ok(Self {
            entries: max_entries,
            resident_bytes: max_resident_bytes,
            matches_per_entry: max_matches_per_entry,
        })
    }

    #[must_use]
    pub const fn max_entries(self) -> usize {
        self.entries
    }

    #[must_use]
    pub const fn max_resident_bytes(self) -> u64 {
        self.resident_bytes
    }

    #[must_use]
    pub const fn max_matches_per_entry(self) -> usize {
        self.matches_per_entry
    }
}

/// The lexical writer envelope (QI-BB-016).
///
/// How much heap every open generation writer may hold together, how much
/// one writer takes, and how long an idle writer is kept before it is
/// committed and released.
///
/// The writer count is derived, never configured on its own: it is the
/// envelope divided by one writer's heap, so raising the per-writer heap
/// lowers the count and the process-wide bound holds either way. Fields are
/// private so every policy in existence is valid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LexicalWriterPolicy {
    envelope_bytes: u64,
    writer_heap_bytes: u64,
    idle_after: core::time::Duration,
}

/// The smallest heap the inverted-index writer accepts per indexing thread.
pub const LEXICAL_WRITER_HEAP_BYTES_MIN: u64 = 15_000_000;
/// One below the largest heap the writer's arena can address.
pub const LEXICAL_WRITER_HEAP_BYTES_MAX: u64 = 0xFFFF_FFFF - 1_000_000;

impl LexicalWriterPolicy {
    /// Sixteen writers of the minimum heap — the bound the adapter carried as
    /// two constants before it was one envelope — released after a minute
    /// idle.
    pub const DEFAULT: Self = Self {
        envelope_bytes: 16 * LEXICAL_WRITER_HEAP_BYTES_MIN,
        writer_heap_bytes: LEXICAL_WRITER_HEAP_BYTES_MIN,
        idle_after: core::time::Duration::from_secs(60),
    };

    pub fn new(
        envelope_bytes: u64,
        writer_heap_bytes: u64,
        idle_after: core::time::Duration,
    ) -> Result<Self, CoreError> {
        if !(LEXICAL_WRITER_HEAP_BYTES_MIN..=LEXICAL_WRITER_HEAP_BYTES_MAX)
            .contains(&writer_heap_bytes)
        {
            return Err(CoreError::InvalidContract(format!(
                "lexical: writer heap {writer_heap_bytes} bytes is outside {LEXICAL_WRITER_HEAP_BYTES_MIN}..={LEXICAL_WRITER_HEAP_BYTES_MAX}"
            )));
        }
        if envelope_bytes < writer_heap_bytes {
            return Err(CoreError::InvalidContract(format!(
                "lexical: writer envelope {envelope_bytes} bytes cannot hold one writer of {writer_heap_bytes} bytes"
            )));
        }
        if idle_after.is_zero() {
            return Err(CoreError::InvalidContract(
                "lexical: writer idle release interval must be non-zero".to_string(),
            ));
        }
        Ok(Self {
            envelope_bytes,
            writer_heap_bytes,
            idle_after,
        })
    }

    #[must_use]
    pub const fn envelope_bytes(self) -> u64 {
        self.envelope_bytes
    }

    #[must_use]
    pub const fn writer_heap_bytes(self) -> u64 {
        self.writer_heap_bytes
    }

    #[must_use]
    pub const fn idle_after(self) -> core::time::Duration {
        self.idle_after
    }

    /// How many writers the envelope holds at once; at least one by
    /// construction.
    #[must_use]
    pub fn max_writers(self) -> usize {
        let writers = self
            .envelope_bytes
            .checked_div(self.writer_heap_bytes)
            .map_or(1, |writers| writers.max(1));
        usize::try_from(writers).map_or(usize::MAX, |writers| writers)
    }
}

/// What the lexical writer cache holds and has done, for operators and
/// tests (QI-BB-016).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LexicalWriterCacheStats {
    pub open_writers: usize,
    pub max_writers: usize,
    /// Heap the open writers were granted together: `open_writers` times
    /// the per-writer heap, never above the envelope. This is the granted
    /// budget, not a measurement — the index library (Tantivy 0.22) reports
    /// arena use only inside its indexing workers and never on the writer
    /// handle — so the observed pressure signal is the process gauge
    /// `process_resident_bytes` the writer gate reads (QI-BB-016).
    pub allocated_heap_bytes: u64,
    /// Writers committed and released to make room for another.
    pub lru_releases: u64,
    /// Writers committed and released because nothing touched them for the
    /// policy's idle interval.
    pub idle_releases: u64,
    /// Writers committed and released because their generation sealed.
    pub seal_releases: u64,
}

/// How much text-authority derivation and writing the adapter has done so
/// far (QI-BB-006), for operators and tests.
///
/// A rebuild scans every live document; an incremental update derives
/// only the chunks a batch added and retires only those it replaced or
/// tombstoned. `docs_derived` is the tokenize-and-post work in documents,
/// whichever path ran, so a test can assert the cost shape without a
/// clock. The text authority is sharded by doc-id range: `shards_written`
/// counts the shard files a write produced and `shards_inherited` the
/// shards it listed unchanged, so the write-bytes shape is a count too.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TextAuthorityUpdateStats {
    /// Sidecar rebuilds from a full scan of the live index.
    pub rebuilds: u64,
    /// In-place updates from the prior generation's sidecars.
    pub incremental_updates: u64,
    /// Documents tokenized and posted, over both paths.
    pub docs_derived: u64,
    /// Documents removed from inherited sidecars by incremental updates.
    pub docs_retired: u64,
    /// Shard files written, over both paths.
    pub shards_written: u64,
    /// Shards listed unchanged — not read, serialized or written — over
    /// both paths.
    pub shards_inherited: u64,
}

/// What the regex match cache did so far, for operators and tests.
///
/// A hit hands out the resident set itself, so there is no clone traffic
/// to report; what a query allocates is the set it builds on a miss, which
/// the `built` counters account whether or not the set is then cached.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RegexMatchCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
    pub resident_bytes: u64,
    /// Entries evicted to make room under the entry or byte bound.
    pub evictions: u64,
    /// Results not cached because they matched more documents than one
    /// entry may hold.
    pub refused_cardinality: u64,
    /// Results not cached because they alone would exceed the byte bound.
    pub refused_bytes: u64,
    /// Match sets computed, cached or not.
    pub sets_built: u64,
    /// Documents those sets matched, summed: the match cardinality served.
    pub members_built: u64,
    /// Bytes those sets occupied when built, summed.
    pub bytes_built: u64,
}
