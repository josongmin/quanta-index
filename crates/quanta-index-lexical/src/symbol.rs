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

use quanta_index_contract::{LqExpr, LqFilter, LqPredicateArg, LqQuery, LqSelect, LqType};

/// Symbol names have keyword postings, not the chunk content authority.
pub(crate) fn symbol_name_keyword(args: &[LqPredicateArg]) -> Option<&str> {
    match args {
        [LqPredicateArg::Keyword(value)] => Some(value),
        _ => None,
    }
}

/// Exact name equality uses dedicated STRING fields and the shared NFC/case policy.
pub(crate) fn exact_symbol_name_query(
    fields: &crate::SchemaFields,
    name: &str,
    args: &[LqPredicateArg],
    options: &quanta_index_contract::LqOptions,
) -> Result<Option<Box<dyn tantivy::query::Query>>, quanta_index_core::CoreError> {
    use crate::normalize::{CaseMode, apply_case, nfc};
    use quanta_index_core::LexicalPredicateV1;
    let Some(predicate) = LexicalPredicateV1::from_canonical_name(name) else {
        return Ok(None);
    };
    let Some(value) = predicate.exact_symbol_name_argument(args)? else {
        return Ok(None);
    };
    let sensitive = options.case_mode() == CaseMode::Sensitive;
    let field = match (predicate, sensitive) {
        (LexicalPredicateV1::SymbolLocalNameExact, true) => fields.symbol_local_name,
        (LexicalPredicateV1::SymbolLocalNameExact, false) => fields.symbol_local_name_folded,
        (LexicalPredicateV1::SymbolQualifiedNameExact, true) => fields.symbol_qualified_name,
        (LexicalPredicateV1::SymbolQualifiedNameExact, false) => {
            fields.symbol_qualified_name_folded
        }
        _ => return Ok(None),
    };
    let normalized = nfc(value);
    let term = apply_case(&normalized, options.case_mode());
    Ok(Some(Box::new(tantivy::query::TermQuery::new(
        tantivy::Term::from_field_text(field, &term),
        tantivy::schema::IndexRecordOption::Basic,
    ))))
}

/// Refuse unsupported symbol text before predicate materialization can return empty.
///
/// Content predicates own their chunk domain; do not reinterpret their arguments.
pub(crate) fn unsupported_symbol_query_text(
    query: &LqQuery,
    expr: &LqExpr,
    symbol_domain: bool,
) -> bool {
    quanta_index_core::LexicalPolicy::unsupported_symbol_query_text(query, expr, symbol_domain)
}

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
            Self::NeedleTooLong { len, cap } => {
                write!(
                    f,
                    "symbol planner: needle length {len} bytes exceeds cap {cap}"
                )
            }
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
        LqSelect::Content | LqSelect::ContentMatch | LqSelect::File | LqSelect::FileOwners => {
            ResultSurface::Content
        }
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
            | LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. }
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
        ResultSurface, SymbolKindFilter, SymbolPlannerError, SymbolPolicy, plan_symbol,
        resolve_result_surface,
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

    #[test]
    fn symbol_kind_filter_stays_in_lockstep_with_contract_v1_wire_codes() {
        let planner_codes = [
            SymbolKindFilter::Function.as_str(),
            SymbolKindFilter::Method.as_str(),
            SymbolKindFilter::Class.as_str(),
            SymbolKindFilter::Struct.as_str(),
            SymbolKindFilter::Enum.as_str(),
            SymbolKindFilter::Trait.as_str(),
            SymbolKindFilter::Interface.as_str(),
            SymbolKindFilter::Variable.as_str(),
            SymbolKindFilter::Constant.as_str(),
            SymbolKindFilter::Module.as_str(),
            SymbolKindFilter::Macro.as_str(),
            SymbolKindFilter::TypeAlias.as_str(),
        ];
        assert_eq!(planner_codes.len(), 12);
        for code in planner_codes {
            assert!(
                quanta_index_contract::lex::SymbolKindCode::new(code).is_ok(),
                "planner code `{code}` must remain valid in the contract v1 wire set"
            );
        }
    }
}

#[cfg(test)]
mod l3_exact_tests {
    #![expect(
        clippy::panic_in_result_fn,
        reason = "assertions are test failure reporting"
    )]
    use super::*;
    use quanta_index_contract::{LqCase, LqOptions};
    use tantivy::collector::Count;
    use tantivy::{Index, IndexWriter, TantivyDocument, doc};

    #[test]
    fn l3_exact_names_are_separate_whole_string_fields_with_nfc_and_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let fields = crate::SchemaFields::build();
        let index = Index::create_in_ram(fields.schema.clone());
        crate::analyzer::register_analyzers(&index);
        let mut writer: IndexWriter<TantivyDocument> =
            index.writer_with_num_threads(1, 15_000_000)?;
        // Fixed postings are the oracle; no production name extractor builds these.
        for (local, folded, qualified, qualified_folded) in [
            ("Café", "café", "One::Café", "one::café"),
            ("Café", "café", "Two::Café", "two::café"),
            ("CaféExtra", "caféextra", "One::CaféExtra", "one::caféextra"),
            ("Café", "café", "One::Café", "one::café"), // Overload remains a distinct row.
            ("Inner", "inner", "Café::Inner", "café::inner"),
        ] {
            let _op = writer.add_document(doc!(fields.symbol_local_name => local,
                fields.symbol_local_name_folded => folded, fields.symbol_qualified_name => qualified,
                fields.symbol_qualified_name_folded => qualified_folded))?;
        }
        let _commit = writer.commit()?;
        writer.wait_merging_threads()?;
        let searcher = index.reader()?.searcher();
        for (name, value, case, expected) in [
            ("symbol.local_name.exact", "CAFE\u{301}", None, 3),
            (
                "symbol.local_name.exact",
                "Café",
                Some(LqCase::Sensitive),
                3,
            ),
            (
                "symbol.local_name.exact",
                "café",
                Some(LqCase::Sensitive),
                0,
            ),
            ("symbol.local_name.exact", "Caf", None, 0),
            ("symbol.local_name.exact", "One::Café", None, 0),
            ("symbol.qualified_name.exact", "ONE::CAFE\u{301}", None, 2),
            ("symbol.qualified_name.exact", "Café", None, 0),
            ("symbol.qualified_name.exact", "Café::Inner", None, 1),
            ("symbol.local_name.exact", "NoSymbol", None, 0),
        ] {
            let options = LqOptions {
                case,
                ..LqOptions::defaults()
            };
            let query = exact_symbol_name_query(
                &fields,
                name,
                &[LqPredicateArg::Keyword(value.into())],
                &options,
            )?
            .expect("exact");
            assert_eq!(
                searcher.search(query.as_ref(), &Count)?,
                expected,
                "{name} {value}"
            );
        }
        assert!(
            exact_symbol_name_query(
                &fields,
                "symbol.has.name",
                &[LqPredicateArg::Keyword("Caf".into())],
                &LqOptions::defaults()
            )?
            .is_none()
        );
        assert!(
            exact_symbol_name_query(
                &fields,
                "symbol.local_name.exact",
                &[LqPredicateArg::Phrase("Café".into())],
                &LqOptions::defaults()
            )
            .is_err()
        );
        Ok(())
    }
}

#[cfg(test)]
mod l3_definition_facts_tests {
    #![expect(
        clippy::panic_in_result_fn,
        reason = "assertions are test failure reporting"
    )]
    use quanta_index_contract::lex::{
        LanguageCode, SymbolKindCode, SymbolRecord, SymbolRelationship, SymbolSpan,
    };
    use quanta_index_contract::{RepoRelativePath, SymbolId};
    use tantivy::schema::{TantivyDocument, Value};

    #[test]
    fn l3_document_preserves_original_spelling_signature_and_definition_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let fields = crate::SchemaFields::build();
        let symbol = SymbolRecord {
            symbol_id: SymbolId::new("definition"),
            repo_relative_path: RepoRelativePath::new("a.rs"),
            language: LanguageCode::new("rust").map_err(str::to_string)?,
            symbol_kind: SymbolKindCode::new("function").map_err(str::to_string)?,
            symbol_kind_family: None,
            local_name: "Cafe\u{301}".into(),
            qualified_name: "Outer::Cafe\u{301}".into(),
            signature: Some("fn Cafe\u{301}(x: u32)".into()),
            visibility: None,
            definition_span: SymbolSpan {
                path: "a.rs".into(),
                byte_start: 17,
                byte_end: 43,
                line_start: 2,
                line_end: 3,
            },
            container_qualified_name: Some("Outer".into()),
            relationship: SymbolRelationship::Def,
        };
        let mut doc = TantivyDocument::default();
        crate::documents::add_symbol_fields(&fields, &mut doc, &symbol);
        for (field, expected) in [
            (fields.symbol_local_name_original, "Cafe\u{301}"),
            (fields.symbol_qualified_name_original, "Outer::Cafe\u{301}"),
            (fields.symbol_signature, "fn Cafe\u{301}(x: u32)"),
            (fields.symbol_local_name, "Café"),
            (fields.symbol_local_name_folded, "café"),
        ] {
            assert_eq!(
                doc.get_first(field).and_then(|value| value.as_str()),
                Some(expected)
            );
        }
        assert_eq!(
            doc.get_first(fields.symbol_definition_start_byte)
                .and_then(|value| value.as_u64()),
            Some(17)
        );
        assert_eq!(
            doc.get_first(fields.symbol_definition_end_byte)
                .and_then(|value| value.as_u64()),
            Some(43)
        );
        Ok(())
    }
}
