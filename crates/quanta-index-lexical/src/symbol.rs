//! Symbol-route planning and `select:`/`type:` surface resolution.
//!
//! This module owns three live concerns:
//!
//! 1. Symbol-route planning — turns a symbol needle (+ optional kind)
//!    into a typed [`SymbolPlan`] for the symbol index, not content
//!    substring search.
//! 2. Result-surface resolution — collapses
//!    [`LqFilter::Select`] / [`LqFilter::Type`] into one
//!    [`ResultSurface`] per dsl.md §6.4–§6.5. `Select` wins for
//!    rendering; `Type` constrains engine routing.
//! 3. Typed-unavailable bookkeeping — `Commit` / `Diff` / `Repo` /
//!    `Structural` surfaces still have no producer on the lexical rail,
//!    so the trace records intent and the executor emits the typed
//!    response instead of silently degrading.

use core::fmt;

use quanta_index_contract::{LqFilter, LqSelect, LqType};

/// Closed set of v1 symbol kinds accepted as a `kind_filter`.
///
/// Canonical 12 v1 kinds (originally from LEX-05 §3.3). This is the
/// authoritative enum on the lexical plane; no mirror crate exists.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SymbolKindFilter {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Variable,
    Constant,
    Module,
    Macro,
    TypeAlias,
}

impl SymbolKindFilter {
    /// Stable lowercase wire form used in DSL-facing diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Interface => "interface",
            Self::Variable => "variable",
            Self::Constant => "constant",
            Self::Module => "module",
            Self::Macro => "macro",
            Self::TypeAlias => "type_alias",
        }
    }
}

// DSL-string -> `SymbolKindFilter` parsing belongs upstream in
// lq-norm; the planner consumes already-typed values. Adding a
// `from_dsl_str` here would create dead port surface per CLAUDE.md.

/// Terminal result-rendering surface.
///
/// One variant per dsl.md §6.5 `select:` dimension, plus `Structural` so the
/// typed-unavailable path is exhaustive. `Commit`, `Diff`, `Repo`, and
/// `Structural` have no producer on the lexical rail today.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ResultSurface {
    Content,
    Path,
    Symbol,
    Repo,
    Commit,
    Diff,
    Structural,
}

impl ResultSurface {
    /// Short stable label for the explain trace.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Path => "path",
            Self::Symbol => "symbol",
            Self::Repo => "repo",
            Self::Commit => "commit",
            Self::Diff => "diff",
            Self::Structural => "structural",
        }
    }

    /// `true` for surfaces whose producer is not wired on today's
    /// lexical rail; the executor must emit a typed-unavailable
    /// response rather than an empty success.
    #[must_use]
    pub const fn is_typed_unavailable(self) -> bool {
        matches!(
            self,
            Self::Commit | Self::Diff | Self::Repo | Self::Structural
        )
    }
}

/// Planner-side configuration knobs.
///
/// Kept tiny: new knobs land alongside their first executor caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SymbolPolicy {
    /// Per-leaf candidate cap fed into the symbol-index lookup.
    pub default_candidate_cap: u32,
    /// Reject needles longer than this byte length up-front.
    pub max_needle_bytes: usize,
}

impl SymbolPolicy {
    /// v1 defaults. `default_candidate_cap: 10_000` matches dsl.md
    /// §6.2 `count:` bounded cap; `max_needle_bytes: 4_096` bounds
    /// worst-case allocations on adversarial input.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            default_candidate_cap: 10_000,
            max_needle_bytes: 4_096,
        }
    }
}

/// Typed symbol-planner errors.
///
/// Manual `Display` and `Error` impls keep the workspace
/// no-proc-macro-derive build-hygiene rule intact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SymbolPlannerError {
    /// `symbol:""` or whitespace-only needle.
    EmptyNeedle,
    /// Needle length exceeds [`SymbolPolicy::max_needle_bytes`].
    NeedleTooLong { len: usize, cap: usize },
    /// `kind:` value the planner does not recognize. `name` is the
    /// raw DSL token so callers can include it verbatim in their
    /// typed error code.
    UnsupportedKind { name: &'static str },
    /// `select:` + `type:` resolved to incompatible surfaces, or a
    /// surface that cannot be honored at all on this rail.
    UnsupportedSurface {
        surface: ResultSurface,
        reason: &'static str,
    },
}

impl fmt::Display for SymbolPlannerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyNeedle => f.write_str("symbol planner: needle must be non-empty"),
            Self::NeedleTooLong { len, cap } => write!(
                f,
                "symbol planner: needle length {len} bytes exceeds cap {cap}"
            ),
            Self::UnsupportedKind { name } => {
                write!(f, "symbol planner: unsupported kind filter `{name}`")
            }
            Self::UnsupportedSurface { surface, reason } => write!(
                f,
                "symbol planner: unsupported result surface `{}` ({reason})",
                surface.as_str()
            ),
        }
    }
}

impl std::error::Error for SymbolPlannerError {}

/// Accumulates trace fields across planner and executor.
///
/// Planner sets `resolved_surface` and `kind_filter` at construction.
/// Executor records `candidate_count` and `verify_count` post-exec.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolTraceBuilder {
    resolved_surface: ResultSurface,
    surface_unavailable: bool,
    kind_filter: Option<SymbolKindFilter>,
    candidate_count: Option<u32>,
    verify_count: Option<u32>,
}

impl SymbolTraceBuilder {
    /// Construct a fresh builder bound to `surface`.
    #[must_use]
    pub fn new(surface: ResultSurface) -> Self {
        let surface_unavailable = surface.is_typed_unavailable();
        Self {
            resolved_surface: surface,
            surface_unavailable,
            kind_filter: None,
            candidate_count: None,
            verify_count: None,
        }
    }

    /// Set the kind filter (called by [`plan_symbol`]).
    #[must_use]
    pub fn with_kind_filter(mut self, kind: Option<SymbolKindFilter>) -> Self {
        self.kind_filter = kind;
        self
    }

    /// Executor-side post-exec setter.
    pub fn record_candidate_count(&mut self, count: u32) {
        self.candidate_count = Some(count);
    }

    /// Executor-side post-exec setter.
    pub fn record_verify_count(&mut self, count: u32) {
        self.verify_count = Some(count);
    }

    /// Finalize into the immutable trace.
    #[must_use]
    pub fn build(self) -> SymbolTrace {
        SymbolTrace {
            resolved_surface: self.resolved_surface,
            surface_unavailable: self.surface_unavailable,
            kind_filter: self.kind_filter,
            candidate_count: self.candidate_count,
            verify_count: self.verify_count,
        }
    }
}

/// Immutable symbol-route explain trace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolTrace {
    /// The terminal result-rendering surface chosen by the planner.
    pub resolved_surface: ResultSurface,
    /// `true` iff [`ResultSurface::is_typed_unavailable`] held when
    /// the trace was created — surfaces whose producer is not wired.
    pub surface_unavailable: bool,
    /// Optional kind filter the executor must apply.
    pub kind_filter: Option<SymbolKindFilter>,
    /// Post-exec: number of candidates the symbol index returned.
    pub candidate_count: Option<u32>,
    /// Post-exec: number of candidates that survived verification.
    pub verify_count: Option<u32>,
}

/// Planner output for the symbol route.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolPlan {
    /// Symbol-name fragment to look up in the symbol index.
    pub needle: String,
    /// Optional kind constraint (function / struct / …).
    pub kind_filter: Option<SymbolKindFilter>,
    /// Per-leaf candidate cap fed into the symbol-index lookup.
    pub candidate_cap: u32,
    /// Pre-populated explain-trace builder.
    pub trace: SymbolTraceBuilder,
}

/// Plan a symbol-route lookup.
///
/// Rejects empty / whitespace-only needles and bounds byte length
/// against `policy.max_needle_bytes`. Kind input is the closed
/// [`SymbolKindFilter`] enum; lq-norm owns DSL-string parsing
/// upstream and surfaces [`SymbolPlannerError::UnsupportedKind`]
/// there.
pub fn plan_symbol(
    needle: &str,
    kind_filter: Option<SymbolKindFilter>,
    policy: &SymbolPolicy,
) -> Result<SymbolPlan, SymbolPlannerError> {
    let trimmed = needle.trim();
    if trimmed.is_empty() {
        return Err(SymbolPlannerError::EmptyNeedle);
    }
    if trimmed.len() > policy.max_needle_bytes {
        return Err(SymbolPlannerError::NeedleTooLong {
            len: trimmed.len(),
            cap: policy.max_needle_bytes,
        });
    }
    let trace = SymbolTraceBuilder::new(ResultSurface::Symbol).with_kind_filter(kind_filter);
    Ok(SymbolPlan {
        needle: trimmed.to_owned(),
        kind_filter,
        candidate_cap: policy.default_candidate_cap,
        trace,
    })
}

/// Map a single [`LqType`] value to the corresponding terminal
/// [`ResultSurface`].
const fn surface_for_type(kind: LqType) -> ResultSurface {
    match kind {
        LqType::File => ResultSurface::Content,
        LqType::Path => ResultSurface::Path,
        LqType::Symbol => ResultSurface::Symbol,
        LqType::Commit => ResultSurface::Commit,
        LqType::Diff => ResultSurface::Diff,
        LqType::Repo => ResultSurface::Repo,
    }
}

/// Map a single [`LqSelect`] value to the corresponding terminal
/// [`ResultSurface`].
const fn surface_for_select(dim: LqSelect) -> ResultSurface {
    match dim {
        // `Content`, `ContentMatch`, and `File` all render content
        // rows on the lexical rail:
        // - `Content` / `ContentMatch` is whole-row vs match-span
        //   (downstream of surface choice),
        // - `File` selects the file row, content-shaped per the
        //   existing `LqSelect::File => QueryDocKind::Text` mapping
        //   at [lib.rs:1132].
        LqSelect::Content | LqSelect::ContentMatch | LqSelect::File => ResultSurface::Content,
        LqSelect::Path => ResultSurface::Path,
        LqSelect::Symbol => ResultSurface::Symbol,
        LqSelect::Repo => ResultSurface::Repo,
    }
}

/// Resolve the terminal [`ResultSurface`] from a filter list.
///
/// Precedence (dsl.md §6.4 / §6.5):
/// - `select:` and `type:` pointing at different surfaces ->
///   `UnsupportedSurface { reason: "conflicting select+type" }`.
/// - When only one is present, its surface wins.
/// - When neither is present, defaults to [`ResultSurface::Content`]
///   (matches `type:file` implicit default per dsl.md §6.4).
///
/// Typed-unavailable surfaces (`Commit` / `Diff` / `Repo` /
/// `Structural`) are returned as `Ok`; the executor emits the
/// typed-unavailable response — planning is the wrong layer for that.
pub fn resolve_result_surface(filters: &[LqFilter]) -> Result<ResultSurface, SymbolPlannerError> {
    let mut type_surface: Option<ResultSurface> = None;
    let mut select_surface: Option<ResultSurface> = None;
    for filter in filters {
        match filter {
            LqFilter::Type { kind } => {
                let next = surface_for_type(*kind);
                if let Some(existing) = type_surface
                    && existing != next
                {
                    // dsl.md §6.4: "Two `type:` filters in one query
                    // -> PARSE_INVALID_FILTER_VALUE{filter=type}".
                    // The normalizer should have already rejected
                    // this, but the planner re-checks fail-closed.
                    return Err(SymbolPlannerError::UnsupportedSurface {
                        surface: next,
                        reason: "multiple conflicting type: values",
                    });
                }
                type_surface = Some(next);
            }
            LqFilter::Select { dim } => {
                let next = surface_for_select(*dim);
                if let Some(existing) = select_surface
                    && existing != next
                {
                    return Err(SymbolPlannerError::UnsupportedSurface {
                        surface: next,
                        reason: "multiple conflicting select: values",
                    });
                }
                select_surface = Some(next);
            }
            LqFilter::Repo { .. }
            | LqFilter::File { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Rev { .. }
            | LqFilter::Author { .. }
            | LqFilter::Committer { .. }
            | LqFilter::Message { .. }
            | LqFilter::Dirty { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. }
            | LqFilter::Content { .. } => {}
        }
    }
    match (select_surface, type_surface) {
        (Some(select), Some(t)) if select != t => Err(SymbolPlannerError::UnsupportedSurface {
            surface: select,
            reason: "conflicting select+type",
        }),
        (Some(select), _) => Ok(select),
        (None, Some(t)) => Ok(t),
        (None, None) => Ok(ResultSurface::Content),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ResultSurface, SymbolPlannerError, SymbolPolicy, plan_symbol, resolve_result_surface,
    };
    use quanta_index_contract::{LqFilter, LqSelect, LqType};

    fn policy() -> SymbolPolicy {
        SymbolPolicy::defaults()
    }

    #[test]
    fn plan_symbol_simple_needle_ok() {
        let outcome = plan_symbol("foo", None, &policy());
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            assert_eq!(plan.needle, "foo");
            assert_eq!(plan.kind_filter, None);
            assert_eq!(
                plan.candidate_cap,
                SymbolPolicy::defaults().default_candidate_cap
            );
            let trace = plan.trace.build();
            assert_eq!(trace.resolved_surface, ResultSurface::Symbol);
            assert!(!trace.surface_unavailable);
            assert_eq!(trace.kind_filter, None);
        }
    }

    #[test]
    fn plan_symbol_empty_needle_rejected() {
        let outcome = plan_symbol("", None, &policy());
        assert_eq!(outcome, Err(SymbolPlannerError::EmptyNeedle));
    }

    #[test]
    fn resolve_type_symbol_resolves_to_symbol_surface() {
        let filters = vec![LqFilter::Type {
            kind: LqType::Symbol,
        }];
        let outcome = resolve_result_surface(&filters);
        assert_eq!(outcome, Ok(ResultSurface::Symbol));
    }

    #[test]
    fn resolve_select_path_resolves_to_path_surface() {
        let filters = vec![LqFilter::Select {
            dim: LqSelect::Path,
        }];
        let outcome = resolve_result_surface(&filters);
        assert_eq!(outcome, Ok(ResultSurface::Path));
    }

    #[test]
    fn resolve_type_symbol_with_conflicting_select_file_rejected() {
        let filters = vec![
            LqFilter::Type {
                kind: LqType::Symbol,
            },
            LqFilter::Select {
                dim: LqSelect::File,
            },
        ];
        let outcome = resolve_result_surface(&filters);
        let matched = matches!(
            &outcome,
            Err(SymbolPlannerError::UnsupportedSurface { reason, .. })
                if *reason == "conflicting select+type"
        );
        assert!(
            matched,
            "expected UnsupportedSurface conflicting select+type, got {outcome:?}"
        );
    }

    #[test]
    fn resolve_type_commit_is_typed_unavailable_surface() {
        let filters = vec![LqFilter::Type {
            kind: LqType::Commit,
        }];
        let outcome = resolve_result_surface(&filters);
        assert_eq!(outcome, Ok(ResultSurface::Commit));
        // The planner records the surface as typed-unavailable; the
        // executor consumes that bit to decide whether to emit a
        // typed-unavailable response.
        if let Ok(surface) = outcome {
            let trace = super::SymbolTraceBuilder::new(surface).build();
            assert!(
                trace.surface_unavailable,
                "Commit surface must be flagged unavailable in trace"
            );
        }
    }
}
