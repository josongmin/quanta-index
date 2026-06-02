//! LXE-03 — lexical filter-execution planner.
//!
//! Owns the plan-time shape produced from [`LqFilter`] inputs: repo / file /
//! lang constraints become executable [`FilterPlan`] fields; surface routing
//! (`type:` / `select:`) delegates to [`crate::symbol::resolve_result_surface`];
//! producer-dependent filters (`rev:`, `fork:`, `archived:`, `visibility:`,
//! `context:`, history-only filters like `author:` / `committer:` / `message:`,
//! runtime-only filters like `dirty:`, and `type:commit/diff/repo` via the
//! surface resolver) are recorded as typed unavailable so the executor can emit
//! a typed-unavailable response — no filter is ever silently ignored.
//!
//! Display / `std::error::Error` impls are hand-rolled per the workspace
//! no-proc-macro-derive build-hygiene rule (`thiserror` may not be added to
//! this crate).

use core::fmt;

use quanta_index_contract::{
    LqCase, LqCountBound, LqFileScope, LqFilter, LqOptions, LqType, LqVisibility, LqYesNoOnly,
};

use crate::symbol::{ResultSurface, SymbolPlannerError, resolve_result_surface};

/// Pre-candidate repo constraint.
///
/// Today the executor lowers `pattern` into a regex match over the indexed
/// `repo_id` facet (`ChunkRecord::source_repo_id` when present, otherwise the
/// batch `repo_id`); `revs` is recorded but is not executable on the live
/// Tantivy adapter until a history producer exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoConstraint {
    pub pattern: String,
    pub revs: Vec<String>,
}

/// Pre-candidate path / file-name constraint.
///
/// `scope` mirrors the DSL `file:.../path:...` distinction
/// (`NameAndPath` includes file-name regex match; `PathOnly` restricts to the
/// path-only field).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathConstraint {
    pub pattern: String,
    pub scope: FilePathScope,
}

/// Local mirror of [`LqFileScope`] kept at the planner boundary so callers
/// outside the lexical crate do not need to import the contract enum just to
/// inspect a [`PathConstraint`]. Conversion is total.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FilePathScope {
    /// Match file name OR repo-relative path.
    NameAndPath,
    /// Match only the repo-relative path.
    PathOnly,
}

impl FilePathScope {
    #[must_use]
    pub const fn from_lq(scope: LqFileScope) -> Self {
        match scope {
            LqFileScope::NameAndPath => Self::NameAndPath,
            LqFileScope::PathOnly => Self::PathOnly,
        }
    }

    /// Stable short label for explain traces.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NameAndPath => "name_and_path",
            Self::PathOnly => "path_only",
        }
    }
}

/// Case-fold policy carried through to text/regex/phrase matching.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CasePolicy {
    /// Default: case-insensitive (the lq DSL default).
    Insensitive,
    /// `case:yes` was set explicitly.
    Sensitive,
}

impl CasePolicy {
    /// Lower an [`LqOptions::case`] into the planner's case policy.
    #[must_use]
    pub const fn from_options(options: &LqOptions) -> Self {
        match options.case {
            Some(LqCase::Sensitive) => Self::Sensitive,
            Some(LqCase::Insensitive) | None => Self::Insensitive,
        }
    }

    /// Stable short label for explain traces.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Insensitive => "insensitive",
            Self::Sensitive => "sensitive",
        }
    }
}

/// Top-N cap policy. Unbounded mirrors `count:all`; Bounded carries the cap.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CountPolicy {
    /// `count:all` requested.
    Unbounded,
    /// `count:<n>` (or default cap).
    Bounded(u32),
}

impl CountPolicy {
    /// Default cap when the DSL did not specify one. Pinned at 50 today —
    /// matches the conservative cap used by upstream producer scenarios and
    /// keeps test fixtures deterministic.
    pub const DEFAULT_BOUNDED: u32 = 50;

    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Unbounded => "unbounded",
            Self::Bounded(_) => "bounded",
        }
    }
}

/// Filters whose producer is not wired on today's lexical rail.
///
/// Carrying these in [`FilterPlan::typed_unavailable`] lets the executor emit
/// a typed-unavailable response instead of silently dropping the filter — the
/// CLAUDE.md "no silent fallback" rule.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TypedUnavailable {
    /// Stable wire code (e.g. `LEX_FILTER_FORK_UNAVAILABLE`).
    pub code: &'static str,
    /// Operator-facing reason.
    pub reason: &'static str,
}

/// Stable typed-unavailable codes. Kept as constants so call sites can name
/// them without re-typing the string and so the executor can match on them.
pub mod codes {
    pub const FORK_UNAVAILABLE: &str = "LEX_FILTER_FORK_UNAVAILABLE";
    pub const ARCHIVED_UNAVAILABLE: &str = "LEX_FILTER_ARCHIVED_UNAVAILABLE";
    pub const VISIBILITY_UNAVAILABLE: &str = "LEX_FILTER_VISIBILITY_UNAVAILABLE";
    pub const CONTEXT_UNAVAILABLE: &str = "LEX_FILTER_CONTEXT_UNAVAILABLE";
    pub const REV_UNAVAILABLE: &str = "LEX_FILTER_REV_UNAVAILABLE";
    pub const AUTHOR_UNAVAILABLE: &str = "LEX_FILTER_AUTHOR_UNAVAILABLE";
    pub const COMMITTER_UNAVAILABLE: &str = "LEX_FILTER_COMMITTER_UNAVAILABLE";
    pub const MESSAGE_UNAVAILABLE: &str = "LEX_FILTER_MESSAGE_UNAVAILABLE";
    pub const DIRTY_UNAVAILABLE: &str = "LEX_FILTER_DIRTY_UNAVAILABLE";
    pub const RUNTIME_CATALOG_UNAVAILABLE: &str = "LEX_FILTER_RUNTIME_CATALOG_UNAVAILABLE";
    /// Matches the existing dispatcher code emitted for commit/diff/repo
    /// surfaces so producers and search-plane callers see one code per cause.
    pub const HISTORY_PRODUCER_UNAVAILABLE: &str = "HISTORY_PRODUCER_UNAVAILABLE";
}

/// Resolved, executable plan for a query's filter list.
///
/// One value per planned query. The executor walks this rather than the raw
/// `LqFilter` slice so unsupported / producer-dependent variants cannot leak
/// to the search path silently.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilterPlan {
    pub repo: Vec<RepoConstraint>,
    pub paths: Vec<PathConstraint>,
    pub langs: Vec<String>,
    pub case: CasePolicy,
    pub count: CountPolicy,
    pub surface: ResultSurface,
    pub typed_unavailable: Vec<TypedUnavailable>,
}

impl FilterPlan {
    /// Inert plan: no filters, default case/count, default `Content` surface.
    ///
    /// Used as the canonical zero value when planning an empty filter list.
    #[must_use]
    pub fn empty(options: &LqOptions) -> Self {
        Self {
            repo: Vec::new(),
            paths: Vec::new(),
            langs: Vec::new(),
            case: CasePolicy::from_options(options),
            count: count_from_options(options),
            surface: ResultSurface::Content,
            typed_unavailable: Vec::new(),
        }
    }
}

/// Typed planner errors for cases the planner truly cannot honor.
///
/// Producer-unavailable filters are NOT errors — they are recorded as
/// [`TypedUnavailable`] in the plan so the executor can surface them as
/// typed responses. This enum is reserved for input shapes the planner
/// rejects outright (conflicting select+type, count:0, etc.).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilterPlannerError {
    /// `select:` and `type:` resolved to incompatible surfaces.
    ConflictingResultSurface { detail: &'static str },
    /// `count:0` (or another structurally-invalid count).
    InvalidCount { detail: &'static str },
    /// Reserved for true conflicts inside the filter set (e.g. nested
    /// content-leaf under negation) — wired with payload so callers can
    /// surface a stable diagnostic.
    UnsupportedFilterCombo { detail: &'static str },
}

impl fmt::Display for FilterPlannerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConflictingResultSurface { detail } => {
                write!(f, "filter planner: conflicting result surface ({detail})")
            }
            Self::InvalidCount { detail } => {
                write!(f, "filter planner: invalid count ({detail})")
            }
            Self::UnsupportedFilterCombo { detail } => {
                write!(
                    f,
                    "filter planner: unsupported filter combination ({detail})"
                )
            }
        }
    }
}

impl std::error::Error for FilterPlannerError {}

/// Lower [`LqOptions::count`] into the planner's [`CountPolicy`].
fn count_from_options(options: &LqOptions) -> CountPolicy {
    match options.count {
        Some(LqCountBound::All) => CountPolicy::Unbounded,
        Some(LqCountBound::Bounded(n)) => CountPolicy::Bounded(n),
        None => CountPolicy::Bounded(CountPolicy::DEFAULT_BOUNDED),
    }
}

/// Translate a [`SymbolPlannerError`] from `resolve_result_surface` into the
/// filter planner's typed error space.
fn map_surface_error(err: &SymbolPlannerError) -> FilterPlannerError {
    match err {
        SymbolPlannerError::UnsupportedSurface { reason, .. } => {
            FilterPlannerError::ConflictingResultSurface { detail: reason }
        }
        // The symbol planner's other variants come from `plan_symbol`, not
        // surface resolution; map them through as conflict-shaped so the
        // caller still sees a typed error rather than a panic.
        SymbolPlannerError::EmptyNeedle => FilterPlannerError::ConflictingResultSurface {
            detail: "empty symbol needle from surface resolver",
        },
        SymbolPlannerError::NeedleTooLong { .. } => FilterPlannerError::ConflictingResultSurface {
            detail: "needle too long from surface resolver",
        },
        SymbolPlannerError::UnsupportedKind { .. } => {
            FilterPlannerError::ConflictingResultSurface {
                detail: "unsupported kind from surface resolver",
            }
        }
    }
}

/// Normalize a language identifier to the lowercase form the predicate
/// lowering / index ingest emit. Returns `None` for empty/whitespace.
fn normalize_lang_id(id: &str) -> Option<String> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

/// Emit a [`TypedUnavailable`] for a [`LqType`] whose producer is not wired
/// today. Returns `None` for kinds the lexical rail can execute.
const fn typed_unavailable_for_type(kind: LqType) -> Option<TypedUnavailable> {
    match kind {
        LqType::Commit | LqType::Diff => Some(TypedUnavailable {
            code: codes::HISTORY_PRODUCER_UNAVAILABLE,
            reason: "type: filter targets a surface with no producer on the lexical rail",
        }),
        LqType::File | LqType::Path | LqType::Symbol | LqType::Repo => None,
    }
}

/// Emit a [`TypedUnavailable`] for `fork:` / `archived:` modes that require
/// repo metadata the producer does not yet ship.
const fn typed_unavailable_for_fork(_mode: LqYesNoOnly) -> TypedUnavailable {
    TypedUnavailable {
        code: codes::FORK_UNAVAILABLE,
        reason: "fork filter requires repo-metadata producer",
    }
}

const fn typed_unavailable_for_archived(_mode: LqYesNoOnly) -> TypedUnavailable {
    TypedUnavailable {
        code: codes::ARCHIVED_UNAVAILABLE,
        reason: "archived filter requires repo-metadata producer",
    }
}

const fn typed_unavailable_for_visibility(_mode: LqVisibility) -> TypedUnavailable {
    TypedUnavailable {
        code: codes::VISIBILITY_UNAVAILABLE,
        reason: "visibility filter requires repo-metadata producer",
    }
}

const fn typed_unavailable_for_context() -> TypedUnavailable {
    TypedUnavailable {
        code: codes::CONTEXT_UNAVAILABLE,
        reason: "context filter requires repo-metadata producer",
    }
}

const fn typed_unavailable_for_rev() -> TypedUnavailable {
    TypedUnavailable {
        code: codes::REV_UNAVAILABLE,
        reason: "rev filter requires history producer",
    }
}

const fn typed_unavailable_for_author() -> TypedUnavailable {
    TypedUnavailable {
        code: codes::AUTHOR_UNAVAILABLE,
        reason: "author filter is not executable on the lexical rail",
    }
}

const fn typed_unavailable_for_committer() -> TypedUnavailable {
    TypedUnavailable {
        code: codes::COMMITTER_UNAVAILABLE,
        reason: "committer filter is not executable on the lexical rail",
    }
}

const fn typed_unavailable_for_message() -> TypedUnavailable {
    TypedUnavailable {
        code: codes::MESSAGE_UNAVAILABLE,
        reason: "message filter is not executable on the lexical rail",
    }
}

const fn typed_unavailable_for_history_date_diff() -> TypedUnavailable {
    TypedUnavailable {
        code: codes::HISTORY_PRODUCER_UNAVAILABLE,
        reason: "history date/diff filters require history producer",
    }
}

const fn typed_unavailable_for_dirty() -> TypedUnavailable {
    TypedUnavailable {
        code: codes::DIRTY_UNAVAILABLE,
        reason: "dirty filter is not executable on the lexical rail",
    }
}

const fn typed_unavailable_for_runtime_catalog() -> TypedUnavailable {
    TypedUnavailable {
        code: codes::RUNTIME_CATALOG_UNAVAILABLE,
        reason: "runtime catalog filters are not executable on the lexical rail",
    }
}

/// Plan a flat filter list into an executable [`FilterPlan`].
///
/// Single pass over `filters`; surface resolution is delegated to
/// [`resolve_result_surface`] (called once on the full slice, not per
/// iteration). Nested `LqFilter::Content { leaf }` filters are recorded as a
/// planner concern but not lowered here — the content-leaf is handled at the
/// boolean-tree level by [`crate::planner::LexicalPlanner`], not at the
/// filter level.
pub fn plan_filters(
    filters: &[LqFilter],
    options: &LqOptions,
) -> Result<FilterPlan, FilterPlannerError> {
    let case = CasePolicy::from_options(options);
    let count = count_from_options(options);
    if count == CountPolicy::Bounded(0) {
        return Err(FilterPlannerError::InvalidCount {
            detail: "count:0 is not a valid result cap",
        });
    }

    let surface = resolve_result_surface(filters).map_err(|err| map_surface_error(&err))?;

    let mut repo: Vec<RepoConstraint> = Vec::new();
    let mut paths: Vec<PathConstraint> = Vec::new();
    let mut langs: Vec<String> = Vec::new();
    let mut typed_unavailable: Vec<TypedUnavailable> = Vec::new();

    for filter in filters {
        match filter {
            LqFilter::Repo { pattern, revs } => {
                repo.push(RepoConstraint {
                    pattern: pattern.clone(),
                    revs: revs.clone(),
                });
            }
            LqFilter::File { pattern, scope } => {
                paths.push(PathConstraint {
                    pattern: pattern.clone(),
                    scope: FilePathScope::from_lq(*scope),
                });
            }
            LqFilter::Lang { id } => {
                if let Some(normalized) = normalize_lang_id(id) {
                    langs.push(normalized);
                }
            }
            LqFilter::Type { kind } => {
                if let Some(unavailable) = typed_unavailable_for_type(*kind) {
                    typed_unavailable.push(unavailable);
                }
            }
            // - `Select`: surface resolution handled in one pass above; no
            //   per-iteration work needed.
            // - `Content`: nested-content filter handling is the boolean-tree
            //   level concern (`LexicalPlanner::plan`); the executor still
            //   sees the leaf via `LqQuery::expr` lowering.
            LqFilter::Select { .. } | LqFilter::Content { .. } => {}
            LqFilter::Rev { .. } => {
                typed_unavailable.push(typed_unavailable_for_rev());
            }
            LqFilter::Author { .. } => {
                typed_unavailable.push(typed_unavailable_for_author());
            }
            LqFilter::Committer { .. } => {
                typed_unavailable.push(typed_unavailable_for_committer());
            }
            LqFilter::Message { .. } => {
                typed_unavailable.push(typed_unavailable_for_message());
            }
            LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. } => {
                typed_unavailable.push(typed_unavailable_for_history_date_diff());
            }
            LqFilter::Dirty { .. } => {
                typed_unavailable.push(typed_unavailable_for_dirty());
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
                typed_unavailable.push(typed_unavailable_for_runtime_catalog());
            }
            LqFilter::Fork { mode } => {
                typed_unavailable.push(typed_unavailable_for_fork(*mode));
            }
            LqFilter::Archived { mode } => {
                typed_unavailable.push(typed_unavailable_for_archived(*mode));
            }
            LqFilter::Visibility { mode } => {
                typed_unavailable.push(typed_unavailable_for_visibility(*mode));
            }
            LqFilter::Context { .. } => {
                typed_unavailable.push(typed_unavailable_for_context());
            }
        }
    }

    Ok(FilterPlan {
        repo,
        paths,
        langs,
        case,
        count,
        surface,
        typed_unavailable,
    })
}

#[cfg(test)]
mod tests {
    use super::{CasePolicy, CountPolicy, FilePathScope, FilterPlannerError, codes, plan_filters};
    use crate::symbol::ResultSurface;
    use quanta_index_contract::{
        LqCase, LqCountBound, LqFileScope, LqFilter, LqOptions, LqType, LqVisibility, LqYesNoOnly,
    };

    fn opts() -> LqOptions {
        LqOptions::defaults()
    }

    #[test]
    fn repo_filter_plans_into_repo_constraint() {
        let filters = vec![LqFilter::Repo {
            pattern: "my-repo".to_string(),
            revs: Vec::new(),
        }];
        let outcome = plan_filters(&filters, &opts());
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            assert_eq!(plan.repo.len(), 1);
            let Some(repo_constraint) = plan.repo.first() else {
                assert!(false, "expected one repo constraint");
                return;
            };
            assert_eq!(repo_constraint.pattern, "my-repo");
            assert!(repo_constraint.revs.is_empty());
            assert!(plan.typed_unavailable.is_empty());
        }
    }

    #[test]
    fn file_filter_plans_into_path_constraint_default_scope() {
        let filters = vec![LqFilter::File {
            pattern: "src/foo.rs".to_string(),
            scope: LqFileScope::NameAndPath,
        }];
        let outcome = plan_filters(&filters, &opts());
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            assert_eq!(plan.paths.len(), 1);
            let Some(path_constraint) = plan.paths.first() else {
                assert!(false, "expected one path constraint");
                return;
            };
            assert_eq!(path_constraint.pattern, "src/foo.rs");
            assert_eq!(path_constraint.scope, FilePathScope::NameAndPath);
        }
    }

    #[test]
    fn lang_filter_normalizes_to_lowercase() {
        let filters = vec![LqFilter::Lang {
            id: "RUST".to_string(),
        }];
        let outcome = plan_filters(&filters, &opts());
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            assert_eq!(plan.langs, vec!["rust".to_string()]);
        }
    }

    #[test]
    fn case_yes_flips_case_policy_to_sensitive() {
        let mut o = opts();
        o.case = Some(LqCase::Sensitive);
        let outcome = plan_filters(&[], &o);
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            assert_eq!(plan.case, CasePolicy::Sensitive);
        }
    }

    #[test]
    fn count_zero_is_invalid() {
        let mut o = opts();
        o.count = Some(LqCountBound::Bounded(0));
        let outcome = plan_filters(&[], &o);
        let is_invalid = matches!(outcome, Err(FilterPlannerError::InvalidCount { .. }));
        assert!(is_invalid, "expected InvalidCount, got {outcome:?}");
    }

    #[test]
    fn fork_only_records_typed_unavailable() {
        let filters = vec![LqFilter::Fork {
            mode: LqYesNoOnly::Only,
        }];
        let outcome = plan_filters(&filters, &opts());
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            let has_fork_code = plan
                .typed_unavailable
                .iter()
                .any(|u| u.code == codes::FORK_UNAVAILABLE);
            assert!(
                has_fork_code,
                "expected LEX_FILTER_FORK_UNAVAILABLE, got {:?}",
                plan.typed_unavailable
            );
        }
    }

    #[test]
    fn count_all_lowers_to_unbounded() {
        let mut o = opts();
        o.count = Some(LqCountBound::All);
        let outcome = plan_filters(&[], &o);
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            assert_eq!(plan.count, CountPolicy::Unbounded);
        }
    }

    #[test]
    fn type_commit_records_typed_unavailable_history_code() {
        let filters = vec![LqFilter::Type {
            kind: LqType::Commit,
        }];
        let outcome = plan_filters(&filters, &opts());
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            let has_code = plan
                .typed_unavailable
                .iter()
                .any(|u| u.code == codes::HISTORY_PRODUCER_UNAVAILABLE);
            assert!(
                has_code,
                "expected HISTORY_PRODUCER_UNAVAILABLE, got {:?}",
                plan.typed_unavailable
            );
            // Commit surface is recognized but typed-unavailable; resolver
            // still returns the surface and the executor consumes the trace.
            assert_eq!(plan.surface, ResultSurface::Commit);
        }
    }

    #[test]
    fn rev_filter_records_typed_unavailable() {
        let filters = vec![LqFilter::Rev {
            spec: "main".to_string(),
        }];
        let outcome = plan_filters(&filters, &opts());
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            let has_rev = plan
                .typed_unavailable
                .iter()
                .any(|u| u.code == codes::REV_UNAVAILABLE);
            assert!(
                has_rev,
                "expected REV_UNAVAILABLE, got {:?}",
                plan.typed_unavailable
            );
        }
    }

    #[test]
    fn visibility_public_records_typed_unavailable() {
        let filters = vec![LqFilter::Visibility {
            mode: LqVisibility::Public,
        }];
        let outcome = plan_filters(&filters, &opts());
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            let has_visibility = plan
                .typed_unavailable
                .iter()
                .any(|u| u.code == codes::VISIBILITY_UNAVAILABLE);
            assert!(
                has_visibility,
                "expected VISIBILITY_UNAVAILABLE, got {:?}",
                plan.typed_unavailable
            );
        }
    }
}
