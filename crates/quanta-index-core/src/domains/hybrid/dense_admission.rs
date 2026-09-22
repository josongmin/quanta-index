//! The dense-lane filter push-down contract (QI-BB-018 보완 #3).
//!
//! A hybrid query carries one DSL filter set. The lexical lane compiles it
//! natively; the dense lane ranks vectors and cannot compile the DSL, so a
//! filter the dense lane does not apply lets a row the query excluded leak
//! into the fused result. Every [`LqFilter`] variant therefore reaches the
//! dense lane by exactly one of three routes, decided here, exhaustively:
//!
//! | class | how the dense lane applies it | filters |
//! |---|---|---|
//! | `pushdown` | lowered into the typed `QueryConstraintSetV1` the semantic adapter evaluates natively before ranking | `lang:` |
//! | `exact` | evaluated per dense candidate through the lexical lane's own compiled plan, so both lanes run the same predicate | `repo:`, `file:`, `type:file`, `type:symbol`, `content:`, `fork:`, `archived:`, `visibility:`, `context:` |
//! | `unsupported` | no lane can apply it to dense rows; the route refuses typed with [`HYBRID_FILTER_UNSUPPORTED_CODE`] before any lane runs | `rev:`, `author:`, `committer:`, `message:`, `before:`, `after:`, `since:`, `until:`, `diff.added:`, `diff.removed:`, `diff.touched:`, `type:path`, `type:repo`, `type:commit`, `type:diff`, `select:`, `dirty:`, `changed:`, `stale:`, `snapshot:`, `meta.*:`, `affected:`, `invalidated_by:`, and the `count:` option |
//!
//! `exact` filters are the ones the lexical adapter executes as per-document
//! predicates or as a generation-level gate: the admission plan
//! ([`HybridFilterPlanV1::admission_query`]) is the query's filters with an
//! empty expression, and [`LexicalSearcher::admitted_candidates`] answers,
//! for a set of dense candidate ids, which of them name a live document the
//! plan matches. A dense row this generation does not index cannot be
//! proven admitted and is not admitted; a filter the plan cannot compile is
//! the same typed refusal the lexical lane gives. The dense lane over-fetches
//! and refills under [`HybridOrchestratorPolicy::next_dense_admission_fetch`]
//! until the admitted rows are as deep as an unfiltered lane, the generation
//! runs out of rows, or [`HybridOrchestratorPolicy::dense_admission_examine_ceiling`]
//! rows were examined; the route's trace names which of the three ended it.
//!
//! `unsupported` names filters whose surface a dense row does not have
//! (history, working tree, runtime catalog) or that change the lexical row
//! universe (projections collapse rows per path, repo, or file; `count:`
//! bounds the lexical page and reports a lexical total) so that fusing by
//! candidate identity would be undefined. Refusing them is what keeps a
//! filter from being silently lexical-only.
//!
//! [`LexicalSearcher::admitted_candidates`]: crate::domains::lexical::LexicalSearcher::admitted_candidates
//! [`HybridOrchestratorPolicy::next_dense_admission_fetch`]: crate::domains::hybrid::HybridOrchestratorPolicy::next_dense_admission_fetch
//! [`HybridOrchestratorPolicy::dense_admission_examine_ceiling`]: crate::domains::hybrid::HybridOrchestratorPolicy::dense_admission_examine_ceiling

use core::fmt;

use quanta_index_contract::{LqExpr, LqFilter, LqQuery, LqSelect, LqType};

use crate::error::CoreError;

/// Wire code for a hybrid query carrying a filter or option no lane can
/// apply to dense rows.
pub const HYBRID_FILTER_UNSUPPORTED_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::HybridFilterUnsupported;

/// How the dense lane applies one DSL filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DenseLaneFilterClassV1 {
    /// Lowered into the typed constraint set the semantic adapter evaluates
    /// natively before ranking.
    Pushdown,
    /// Evaluated exactly per dense candidate through the lexical lane's
    /// compiled plan.
    Exact,
    /// No lane can apply it to dense rows; refused typed before any lane
    /// runs.
    Unsupported,
}

impl DenseLaneFilterClassV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pushdown => "pushdown",
            Self::Exact => "exact",
            Self::Unsupported => "unsupported",
        }
    }
}

/// The DSL name of a filter, as the trace and the typed refusal spell it.
#[must_use]
pub const fn hybrid_filter_name_v1(filter: &LqFilter) -> &'static str {
    match filter {
        LqFilter::Repo { .. } => "repo",
        LqFilter::File { .. } => "file",
        LqFilter::Lang { .. } => "lang",
        LqFilter::Rev { .. } => "rev",
        LqFilter::Author { .. } => "author",
        LqFilter::Committer { .. } => "committer",
        LqFilter::Message { .. } => "message",
        LqFilter::Before { .. } => "before",
        LqFilter::After { .. } => "after",
        LqFilter::Since { .. } => "since",
        LqFilter::Until { .. } => "until",
        LqFilter::DiffAdded { .. } => "diff.added",
        LqFilter::DiffRemoved { .. } => "diff.removed",
        LqFilter::DiffTouched { .. } => "diff.touched",
        LqFilter::Type { kind } => match kind {
            LqType::File => "type:file",
            LqType::Path => "type:path",
            LqType::Symbol => "type:symbol",
            LqType::Commit => "type:commit",
            LqType::Diff => "type:diff",
            LqType::Repo => "type:repo",
        },
        LqFilter::Select { dim } => match dim {
            LqSelect::Repo => "select:repo",
            LqSelect::File => "select:file",
            LqSelect::FileOwners => "select:file.owners",
            LqSelect::Path => "select:path",
            LqSelect::Symbol => "select:symbol",
            LqSelect::Content => "select:content",
            LqSelect::ContentMatch => "select:content.match",
        },
        LqFilter::Dirty { .. } => "dirty",
        LqFilter::Changed { .. } => "changed",
        LqFilter::Stale { .. } => "stale",
        LqFilter::Snapshot { .. } => "snapshot",
        LqFilter::MetaOwner { .. } => "meta.owner",
        LqFilter::MetaService { .. } => "meta.service",
        LqFilter::MetaLayer { .. } => "meta.layer",
        LqFilter::MetaSurface { .. } => "meta.surface",
        LqFilter::Affected { .. } => "affected",
        LqFilter::InvalidatedBy { .. } => "invalidated_by",
        LqFilter::Fork { .. } => "fork",
        LqFilter::Archived { .. } => "archived",
        LqFilter::Visibility { .. } => "visibility",
        LqFilter::Context { .. } => "context",
        LqFilter::Content { .. } => "content",
    }
}

/// The class the dense lane applies one filter under (the table in the
/// module doc).
#[must_use]
pub const fn classify_hybrid_filter_v1(filter: &LqFilter) -> DenseLaneFilterClassV1 {
    match filter {
        LqFilter::Lang { .. } => DenseLaneFilterClassV1::Pushdown,
        LqFilter::Repo { .. }
        | LqFilter::File { .. }
        | LqFilter::Content { .. }
        | LqFilter::Fork { .. }
        | LqFilter::Archived { .. }
        | LqFilter::Visibility { .. }
        | LqFilter::Context { .. } => DenseLaneFilterClassV1::Exact,
        LqFilter::Type { kind } => match kind {
            LqType::File | LqType::Symbol => DenseLaneFilterClassV1::Exact,
            LqType::Path | LqType::Repo | LqType::Commit | LqType::Diff => {
                DenseLaneFilterClassV1::Unsupported
            }
        },
        LqFilter::Select { .. }
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
        | LqFilter::InvalidatedBy { .. } => DenseLaneFilterClassV1::Unsupported,
    }
}

/// The dense-lane admission plan for one hybrid query: which filters each
/// class carries and the filter-only query the exact ones are evaluated
/// through.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridFilterPlanV1 {
    pushdown: Vec<&'static str>,
    exact: Vec<&'static str>,
    admission: Option<LqQuery>,
}

impl HybridFilterPlanV1 {
    /// Classify every filter and option of `query`.
    ///
    /// Refuses typed with [`HYBRID_FILTER_UNSUPPORTED_CODE`] on the first
    /// unsupported filter or a `count:` option, naming it; the caller runs no
    /// lane after a refusal. The admission query is `query` with an empty
    /// expression and only its exact filters, in their original order, under
    /// the same options and directives; it is `None` when no filter is exact.
    pub fn plan(query: &LqQuery) -> Result<Self, CoreError> {
        if query.options.count.is_some() {
            return Err(unsupported("count", "option"));
        }
        let mut pushdown = Vec::new();
        let mut exact = Vec::new();
        let mut exact_filters = Vec::new();
        for filter in &query.filters {
            let name = hybrid_filter_name_v1(filter);
            match classify_hybrid_filter_v1(filter) {
                DenseLaneFilterClassV1::Pushdown => push_unique(&mut pushdown, name),
                DenseLaneFilterClassV1::Exact => {
                    push_unique(&mut exact, name);
                    exact_filters.push(filter.clone());
                }
                DenseLaneFilterClassV1::Unsupported => {
                    return Err(unsupported(name, "filter"));
                }
            }
        }
        let admission = (!exact_filters.is_empty()).then(|| LqQuery {
            lq_version: query.lq_version,
            expr: LqExpr::Empty,
            filters: exact_filters,
            directives: query.directives.clone(),
            options: query.options.clone(),
            source_span: query.source_span,
        });
        Ok(Self {
            pushdown,
            exact,
            admission,
        })
    }

    /// The filter-only query the dense candidates are admitted through, or
    /// `None` when no filter needs per-candidate evaluation.
    #[must_use]
    pub const fn admission_query(&self) -> Option<&LqQuery> {
        self.admission.as_ref()
    }

    /// Filter names the dense lane applies natively, in query order.
    #[must_use]
    pub fn pushdown(&self) -> &[&'static str] {
        &self.pushdown
    }

    /// Filter names the dense lane evaluates per candidate, in query order.
    #[must_use]
    pub fn exact(&self) -> &[&'static str] {
        &self.exact
    }
}

impl fmt::Display for HybridFilterPlanV1 {
    /// The push-down class per filter, as the planner trace states it
    /// after its route key: `pushdown:lang; exact:file,repo`, or `none` for
    /// a query with no filter.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.pushdown.is_empty() && self.exact.is_empty() {
            return formatter.write_str("none");
        }
        let mut wrote_class = false;
        for (class, names) in [
            (DenseLaneFilterClassV1::Pushdown, &self.pushdown),
            (DenseLaneFilterClassV1::Exact, &self.exact),
        ] {
            if names.is_empty() {
                continue;
            }
            if wrote_class {
                formatter.write_str("; ")?;
            }
            wrote_class = true;
            write!(formatter, "{}:{}", class.as_str(), names.join(","))?;
        }
        Ok(())
    }
}

fn push_unique(names: &mut Vec<&'static str>, name: &'static str) {
    if !names.contains(&name) {
        names.push(name);
    }
}

fn unsupported(name: &str, kind: &str) -> CoreError {
    CoreError::Typed {
        code: HYBRID_FILTER_UNSUPPORTED_CODE,
        message: format!(
            "hybrid: the `{name}` {kind} has no dense-lane semantics; no lane can apply it to dense rows, so the query is refused rather than served lexical-only"
        ),
    }
}

/// How the dense lane's admission loop ended (QI-BB-018 보완 #3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DenseAdmissionOutcomeV1 {
    /// No filter needed per-candidate evaluation; the dense lane is the raw
    /// fetch.
    NotNeeded,
    /// The admitted rows reached the lane's target depth.
    Filled,
    /// The generation ran out of rows under the pushed-down constraints;
    /// every dense row was examined and the admitted set is complete.
    Exhausted,
    /// The examine ceiling was reached before the target depth; rows the
    /// dense engine ranks below the ceiling were not examined.
    Capped,
}

impl DenseAdmissionOutcomeV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotNeeded => "not_needed",
            Self::Filled => "filled",
            Self::Exhausted => "exhausted",
            Self::Capped => "capped",
        }
    }
}

/// Whether one admission round ends the loop, and how.
///
/// `admitted` rows passed among the `examined` rows one fetch of
/// `fetch_size` returned; `target` is the depth an unfiltered lane has.
/// `None` means the loop fetches again, at the next size.
#[must_use]
pub fn dense_admission_round_outcome_v1(
    admitted: usize,
    examined: usize,
    fetch_size: u32,
    target: u32,
) -> Option<DenseAdmissionOutcomeV1> {
    let target = usize::try_from(target).map_or(usize::MAX, core::convert::identity);
    let fetch_size = usize::try_from(fetch_size).map_or(usize::MAX, core::convert::identity);
    if admitted >= target {
        return Some(DenseAdmissionOutcomeV1::Filled);
    }
    if examined < fetch_size {
        return Some(DenseAdmissionOutcomeV1::Exhausted);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        DenseAdmissionOutcomeV1, DenseLaneFilterClassV1, HYBRID_FILTER_UNSUPPORTED_CODE,
        HybridFilterPlanV1, classify_hybrid_filter_v1, dense_admission_round_outcome_v1,
        hybrid_filter_name_v1,
    };
    use crate::error::CoreError;
    use quanta_index_contract::{
        LQ_VERSION_TAG, LqCountBound, LqExpr, LqFileScope, LqFilter, LqLeaf, LqOptions, LqQuery,
        LqSelect, LqSpan, LqType, LqVisibility, LqYesNoOnly,
    };

    fn query(filters: Vec<LqFilter>) -> LqQuery {
        LqQuery {
            lq_version: LQ_VERSION_TAG,
            expr: LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            filters,
            directives: Vec::new(),
            options: LqOptions::defaults(),
            source_span: LqSpan::new(0, 6),
        }
    }

    /// Every variant of every filter, with the class the table promises.
    ///
    /// A new `LqFilter` variant fails to compile here until it is placed in
    /// the table.
    fn every_filter_with_its_class() -> Vec<(LqFilter, DenseLaneFilterClassV1)> {
        use DenseLaneFilterClassV1::{Exact, Pushdown, Unsupported};
        let pattern = || "src/lib.rs".to_string();
        let mut table = vec![
            (
                LqFilter::Repo {
                    pattern: "^repo$".to_string(),
                    revs: Vec::new(),
                },
                Exact,
            ),
            (
                LqFilter::File {
                    pattern: pattern(),
                    scope: LqFileScope::NameAndPath,
                },
                Exact,
            ),
            (
                LqFilter::Lang {
                    id: "rust".to_string(),
                },
                Pushdown,
            ),
            (
                LqFilter::Rev {
                    spec: "main".to_string(),
                },
                Unsupported,
            ),
            (LqFilter::Author { pattern: pattern() }, Unsupported),
            (LqFilter::Committer { pattern: pattern() }, Unsupported),
            (LqFilter::Message { pattern: pattern() }, Unsupported),
            (
                LqFilter::Before {
                    timeref: "2026".to_string(),
                },
                Unsupported,
            ),
            (
                LqFilter::After {
                    timeref: "2026".to_string(),
                },
                Unsupported,
            ),
            (
                LqFilter::Since {
                    timeref: "2026".to_string(),
                },
                Unsupported,
            ),
            (
                LqFilter::Until {
                    timeref: "2026".to_string(),
                },
                Unsupported,
            ),
            (LqFilter::DiffAdded { pattern: pattern() }, Unsupported),
            (LqFilter::DiffRemoved { pattern: pattern() }, Unsupported),
            (LqFilter::DiffTouched { pattern: pattern() }, Unsupported),
            (
                LqFilter::Dirty {
                    mode: LqYesNoOnly::Only,
                },
                Unsupported,
            ),
            (
                LqFilter::Changed {
                    scope: "x".to_string(),
                },
                Unsupported,
            ),
            (
                LqFilter::Stale {
                    scope: "x".to_string(),
                },
                Unsupported,
            ),
            (
                LqFilter::Snapshot {
                    name: "x".to_string(),
                },
                Unsupported,
            ),
            (
                LqFilter::MetaOwner {
                    id: "x".to_string(),
                },
                Unsupported,
            ),
            (
                LqFilter::MetaService {
                    id: "x".to_string(),
                },
                Unsupported,
            ),
            (
                LqFilter::MetaLayer {
                    id: "x".to_string(),
                },
                Unsupported,
            ),
            (
                LqFilter::MetaSurface {
                    id: "x".to_string(),
                },
                Unsupported,
            ),
            (
                LqFilter::Affected {
                    scope: "x".to_string(),
                },
                Unsupported,
            ),
            (
                LqFilter::InvalidatedBy {
                    source: "x".to_string(),
                },
                Unsupported,
            ),
            (
                LqFilter::Fork {
                    mode: LqYesNoOnly::No,
                },
                Exact,
            ),
            (
                LqFilter::Archived {
                    mode: LqYesNoOnly::Only,
                },
                Exact,
            ),
            (
                LqFilter::Visibility {
                    mode: LqVisibility::Public,
                },
                Exact,
            ),
            (
                LqFilter::Context {
                    name: "x".to_string(),
                },
                Exact,
            ),
            (
                LqFilter::Content {
                    leaf: LqLeaf::Keyword("needle".to_string()),
                },
                Exact,
            ),
        ];
        for (kind, class) in [
            (LqType::File, Exact),
            (LqType::Symbol, Exact),
            (LqType::Path, Unsupported),
            (LqType::Repo, Unsupported),
            (LqType::Commit, Unsupported),
            (LqType::Diff, Unsupported),
        ] {
            table.push((LqFilter::Type { kind }, class));
        }
        for dim in [
            LqSelect::Repo,
            LqSelect::File,
            LqSelect::FileOwners,
            LqSelect::Path,
            LqSelect::Symbol,
            LqSelect::Content,
            LqSelect::ContentMatch,
        ] {
            table.push((LqFilter::Select { dim }, Unsupported));
        }
        table
    }

    #[test]
    fn every_filter_variant_has_exactly_the_class_the_table_promises() {
        let table = every_filter_with_its_class();
        // The exhaustive `match` in `classify_hybrid_filter_v1` is what
        // keeps a new variant from compiling unclassified; this pins the
        // class of every existing one and the name the trace uses.
        for (filter, expected) in &table {
            assert_eq!(
                classify_hybrid_filter_v1(filter),
                *expected,
                "{}",
                hybrid_filter_name_v1(filter)
            );
        }
        let mut names: Vec<&str> = table
            .iter()
            .map(|(f, _)| hybrid_filter_name_v1(f))
            .collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "every filter name must be distinct");
    }

    #[test]
    fn plan_partitions_filters_and_builds_the_filter_only_admission_query() {
        let file = LqFilter::File {
            pattern: "src/lib.rs".to_string(),
            scope: LqFileScope::PathOnly,
        };
        let repo = LqFilter::Repo {
            pattern: "^other$".to_string(),
            revs: Vec::new(),
        };
        let lang = LqFilter::Lang {
            id: "rust".to_string(),
        };
        let query = query(vec![file.clone(), lang, repo.clone(), file.clone()]);
        let plan = HybridFilterPlanV1::plan(&query).expect("supported filters plan");
        assert_eq!(plan.pushdown(), ["lang"]);
        assert_eq!(plan.exact(), ["file", "repo"]);
        assert_eq!(plan.to_string(), "pushdown:lang; exact:file,repo");
        let admission = plan
            .admission_query()
            .expect("exact filters need admission");
        assert_eq!(admission.expr, LqExpr::Empty);
        assert_eq!(admission.filters, vec![file.clone(), repo, file]);
        assert_eq!(admission.options, query.options);
    }

    #[test]
    fn plan_without_exact_filters_needs_no_admission() {
        let plan = HybridFilterPlanV1::plan(&query(Vec::new())).expect("no filters plan");
        assert!(plan.admission_query().is_none());
        assert_eq!(plan.to_string(), "none");
        let plan = HybridFilterPlanV1::plan(&query(vec![LqFilter::Lang {
            id: "rust".to_string(),
        }]))
        .expect("lang plans");
        assert!(plan.admission_query().is_none());
        assert_eq!(plan.to_string(), "pushdown:lang");
    }

    #[test]
    fn plan_refuses_every_unsupported_filter_typed_by_name() {
        for (filter, class) in every_filter_with_its_class() {
            if class != DenseLaneFilterClassV1::Unsupported {
                continue;
            }
            let name = hybrid_filter_name_v1(&filter);
            match HybridFilterPlanV1::plan(&query(vec![filter])) {
                Err(CoreError::Typed { code, message }) => {
                    assert_eq!(code, HYBRID_FILTER_UNSUPPORTED_CODE, "{name}");
                    assert!(
                        message.contains(&format!("`{name}` filter")),
                        "{name}: {message}"
                    );
                }
                other => panic!("{name}: expected a typed refusal, got {other:?}"),
            }
        }
    }

    #[test]
    fn plan_refuses_the_count_option_typed() {
        let mut query = query(Vec::new());
        query.options.count = Some(LqCountBound::Bounded(5));
        match HybridFilterPlanV1::plan(&query) {
            Err(CoreError::Typed { code, message }) => {
                assert_eq!(code, HYBRID_FILTER_UNSUPPORTED_CODE);
                assert!(message.contains("`count` option"), "{message}");
            }
            other => panic!("expected a typed refusal, got {other:?}"),
        }
        query.options.count = Some(LqCountBound::All);
        assert!(HybridFilterPlanV1::plan(&query).is_err());
    }

    #[test]
    fn a_round_ends_filled_exhausted_or_continues() {
        assert_eq!(
            dense_admission_round_outcome_v1(100, 100, 100, 100),
            Some(DenseAdmissionOutcomeV1::Filled)
        );
        assert_eq!(
            dense_admission_round_outcome_v1(3, 7, 100, 100),
            Some(DenseAdmissionOutcomeV1::Exhausted),
            "fewer rows than asked means the generation has no more"
        );
        assert_eq!(
            dense_admission_round_outcome_v1(3, 100, 100, 100),
            None,
            "a full fetch with too few admitted rows refills"
        );
        assert_eq!(
            dense_admission_round_outcome_v1(0, 0, 100, 100),
            Some(DenseAdmissionOutcomeV1::Exhausted)
        );
    }
}
