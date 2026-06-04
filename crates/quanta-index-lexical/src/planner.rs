//! Lexical planner — engine-selection boundary for ticket LXE-02.
//!
//! Consumes [`LqQuery`] (the normalized contract AST) and produces a typed
//! [`LexicalPlan`]. Unsupported IR shapes return a typed
//! [`LexicalPlannerError`] naming the owning follow-up ticket; the executor
//! never inspects raw AST behind the planner.

use core::fmt;
use std::collections::BTreeSet;

use quanta_index_contract::{LqExpr, LqLeaf, LqPredicateArg, LqQuery};

use crate::filters::{FilterPlannerError, plan_filters};
use crate::phrase::{PhraseField, PhrasePlannerError, PhrasePolicy, plan_phrase};
use crate::plan::{CandidateCap, EngineKind, LexicalPlan, PlanLeaf, PlanNode, PlanTraceNode};
use crate::predicate_registry::{
    PREDICATE_OWNER, PredicateKind, canonical_predicate_name, canonicalize_predicate_call, kind_of,
    parse_content_predicate_constraint, parse_content_scalar_arg, parse_repo_file_matchers,
};
use crate::regex::{RegexPlannerError, RegexPolicy, plan_regex};
use crate::symbol::{SymbolPlannerError, SymbolPolicy, plan_symbol};
use crate::trigram_plan::{TrigramPlannerError, TrigramPolicy, plan_raw_substring};

type PlannedExpr = (PlanNode, PlanTraceNode, BTreeSet<EngineKind>);
type PlannedChildren = (Vec<PlanNode>, Vec<PlanTraceNode>, BTreeSet<EngineKind>);

/// Typed planner errors.
///
/// `Unimplemented` is the placeholder variant for IR shapes that the
/// downstream LXE tickets own; carrying `owner_ticket` makes the
/// extension-point explicit and grep-able.
///
/// `Display` / `std::error::Error` are implemented by hand in this module
/// (per the workspace no-proc-macro-derive build-hygiene rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexicalPlannerError {
    /// `NOT` scoped over a subtree the planner does not yet support.
    UnsupportedNotScope,
    /// `OR` scoped over a subtree the planner does not yet support.
    UnsupportedOrScope,
    /// Filter combination not yet expressible in the IR.
    UnsupportedFilterCombo,
    /// IR node that a follow-up ticket owns; carries the ticket id.
    Unimplemented {
        /// Short stable label for the unimplemented shape (e.g. `regex_leaf`).
        node: &'static str,
        /// Ticket id that owns implementing this shape (e.g. `LXE-04`).
        owner_ticket: &'static str,
    },
    /// Regex leaf planning failed (LXE-04). Carries the typed reason.
    RegexPlan(RegexPlannerError),
    /// Raw-substring leaf planning failed (LXE-04). Carries the typed reason.
    TrigramPlan(TrigramPlannerError),
    /// Phrase leaf planning failed (LXE-05). Carries the typed reason.
    PhrasePlan(PhrasePlannerError),
    /// Symbol-route planning failed (LXE-06). Carries the typed reason.
    SymbolPlan(SymbolPlannerError),
    /// Filter-list planning failed (LXE-03). Carries the typed reason.
    FilterPlan(FilterPlannerError),
}

impl fmt::Display for LexicalPlannerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedNotScope => f.write_str("planner: unsupported NOT scope"),
            Self::UnsupportedOrScope => f.write_str("planner: unsupported OR scope"),
            Self::UnsupportedFilterCombo => f.write_str("planner: unsupported filter combination"),
            Self::Unimplemented { node, owner_ticket } => write!(
                f,
                "planner: IR node '{node}' is unimplemented (owner: {owner_ticket})",
            ),
            Self::RegexPlan(e) => write!(f, "planner: {e}"),
            Self::TrigramPlan(e) => write!(f, "planner: {e}"),
            Self::PhrasePlan(e) => write!(f, "planner: {e}"),
            Self::SymbolPlan(e) => write!(f, "planner: {e}"),
            Self::FilterPlan(e) => write!(f, "planner: {e}"),
        }
    }
}

impl std::error::Error for LexicalPlannerError {}

/// Stateless lexical planner.
///
/// The planner is a pure function from `LqQuery` to `LexicalPlan`; it owns
/// no adapter state, no I/O, and no policy decisions other than engine
/// selection. Concrete adapters (Tantivy, trigram, …) are referenced only
/// indirectly via [`EngineKind`] on the produced plan.
pub struct LexicalPlanner;

impl LexicalPlanner {
    /// Plan a normalized query.
    ///
    /// LXE-02 scope: accepts only `LqExpr::Empty` and a single
    /// `LqExpr::Leaf(LqLeaf::Keyword(_))`. All other shapes return a typed
    /// [`LexicalPlannerError::Unimplemented`] naming the follow-up ticket.
    pub fn plan(query: &LqQuery) -> Result<LexicalPlan, LexicalPlannerError> {
        // LXE-03: plan the filter list before walking the expression tree.
        // The boolean tree walk consumes the resolved [`FilterPlan`] so
        // unsupported / producer-dependent filters surface as typed
        // unavailable rather than getting silently dropped on the search
        // path.
        let filter_plan = plan_filters(&query.filters, &query.options)
            .map_err(LexicalPlannerError::FilterPlan)?;
        let (root, trace, engines) = Self::plan_expr(query, &query.expr)?;
        Ok(LexicalPlan {
            root,
            filters: filter_plan,
            raw_filters: query.filters.clone(),
            engines,
            plan_cap: None,
            checkpoints: Vec::new(),
            trace,
            options: query.options.clone(),
            directives: query.directives.clone(),
        })
    }

    fn plan_expr(query: &LqQuery, expr: &LqExpr) -> Result<PlannedExpr, LexicalPlannerError> {
        match expr {
            LqExpr::Empty => Ok((PlanNode::Empty, PlanTraceNode::Empty, BTreeSet::new())),
            LqExpr::Leaf(leaf) => {
                let planned_leaf = Self::plan_leaf(query, leaf)?;
                let engine = planned_leaf.engine();
                let mut engines = BTreeSet::new();
                let _newly_inserted = engines.insert(engine);
                Ok((
                    PlanNode::Leaf {
                        leaf: planned_leaf.clone(),
                        cap: default_leaf_cap(engine),
                    },
                    PlanTraceNode::Leaf {
                        engine,
                        summary: describe_leaf(&planned_leaf),
                    },
                    engines,
                ))
            }
            LqExpr::Not(inner) => {
                let (child, trace, engines) = Self::plan_expr(query, inner)?;
                Ok((
                    PlanNode::Not(Box::new(child)),
                    PlanTraceNode::Not(Box::new(trace)),
                    engines,
                ))
            }
            LqExpr::All(children) => {
                let (nodes, traces, engines) = Self::plan_children(query, children)?;
                Ok((PlanNode::All(nodes), PlanTraceNode::All(traces), engines))
            }
            LqExpr::Any(children) => {
                let (nodes, traces, engines) = Self::plan_children(query, children)?;
                Ok((PlanNode::Any(nodes), PlanTraceNode::Any(traces), engines))
            }
        }
    }

    fn plan_children(
        query: &LqQuery,
        children: &[LqExpr],
    ) -> Result<PlannedChildren, LexicalPlannerError> {
        let mut nodes: Vec<PlanNode> = Vec::with_capacity(children.len());
        let mut traces: Vec<PlanTraceNode> = Vec::with_capacity(children.len());
        let mut engines: BTreeSet<EngineKind> = BTreeSet::new();
        for child in children {
            let (node, trace, child_engines) = Self::plan_expr(query, child)?;
            nodes.push(node);
            traces.push(trace);
            engines.extend(child_engines);
        }
        Ok((nodes, traces, engines))
    }

    /// Plan a single leaf at the root.
    ///
    /// Split out so the LXE-03 work (boolean composition) can call this from
    /// inside `All` / `Any` / `Not` walkers without duplicating the leaf
    /// dispatch table.
    fn plan_leaf(query: &LqQuery, leaf: &LqLeaf) -> Result<PlanLeaf, LexicalPlannerError> {
        let plan_leaf = match leaf {
            LqLeaf::Keyword(term) => PlanLeaf::Content {
                term: term.to_owned(),
            },
            LqLeaf::Phrase(text) => {
                let plan = plan_phrase(
                    text,
                    &query.options,
                    &PhrasePolicy::defaults(),
                    PhraseField::Content,
                )
                .map_err(LexicalPlannerError::PhrasePlan)?;
                PlanLeaf::Phrase {
                    phrase: text.to_owned(),
                    plan,
                }
            }
            LqLeaf::RawString(needle) => {
                let plan = plan_raw_substring(needle, &query.options, &TrigramPolicy::defaults())
                    .map_err(LexicalPlannerError::TrigramPlan)?;
                PlanLeaf::RawSubstring {
                    needle: needle.to_owned(),
                    plan,
                }
            }
            LqLeaf::Regex(source) => {
                let plan = plan_regex(source, &query.options, &RegexPolicy::defaults())
                    .map_err(LexicalPlannerError::RegexPlan)?;
                PlanLeaf::Regex {
                    source: source.to_owned(),
                    plan,
                }
            }
            LqLeaf::StructuralBlock(_) => {
                return Err(LexicalPlannerError::Unimplemented {
                    node: "structural_block_leaf",
                    owner_ticket: "LXE-08",
                });
            }
            LqLeaf::Predicate { name, args } => Self::plan_predicate_leaf(name, args)?,
        };
        Ok(plan_leaf)
    }

    /// Plan a predicate leaf (`<scope>:<head>.<tail>(...)`).
    ///
    /// Today only `symbol.has.name` is wired through the symbol-route
    /// planner; most predicate forms are converted into filters by the
    /// search-plane / core predicate lowering before they reach the
    /// planner, so this arm only sees true leaf-shape predicates.
    fn plan_predicate_leaf(
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<PlanLeaf, LexicalPlannerError> {
        // `symbol.has.name` lowers through the symbol planner, not the lexical
        // content/repo seam, so it is handled here before consulting the
        // lexical predicate registry (which intentionally does not own it).
        if name == "symbol.has.name" {
            let needle = single_string_arg(name, args)?;
            let plan = plan_symbol(&needle, None, &SymbolPolicy::defaults())
                .map_err(LexicalPlannerError::SymbolPlan)?;
            return Ok(PlanLeaf::Symbol { name: needle, plan });
        }
        let Some(canonical) = canonicalize_predicate_call(name, args).map_err(|_err| {
            LexicalPlannerError::Unimplemented {
                node: predicate_arity_label(name),
                owner_ticket: PREDICATE_OWNER,
            }
        })?
        else {
            return Err(LexicalPlannerError::Unimplemented {
                node: "predicate_leaf",
                owner_ticket: PREDICATE_OWNER,
            });
        };
        match kind_of(canonical.name) {
            Some(PredicateKind::RepoFileGate) => {
                validate_repo_has_file_args(canonical.name, &canonical.args)?;
                Ok(PlanLeaf::Predicate {
                    name: canonical.name.to_owned(),
                })
            }
            Some(PredicateKind::ContentLeaf) => {
                let _constraint =
                    parse_content_predicate_constraint(&canonical.args).map_err(|_err| {
                        LexicalPlannerError::Unimplemented {
                            node: predicate_arity_label(name),
                            owner_ticket: PREDICATE_OWNER,
                        }
                    })?;
                Ok(PlanLeaf::Predicate {
                    name: canonical.name.to_owned(),
                })
            }
            Some(PredicateKind::RepoContentGate) => {
                let _arg = parse_content_scalar_arg(&canonical.args).map_err(|_err| {
                    LexicalPlannerError::Unimplemented {
                        node: predicate_arity_label(name),
                        owner_ticket: PREDICATE_OWNER,
                    }
                })?;
                Ok(PlanLeaf::Predicate {
                    name: canonical.name.to_owned(),
                })
            }
            None => Err(LexicalPlannerError::Unimplemented {
                node: "predicate_leaf",
                owner_ticket: PREDICATE_OWNER,
            }),
        }
    }
}

/// Extract a single keyword / raw-string / phrase argument as `String`.
///
/// `symbol.has.name(foo)` is the only predicate form wired today; the helper
/// is narrow on purpose so the predicate arm rejects shapes outside the
/// single-string contract via typed error.
fn single_string_arg(name: &str, args: &[LqPredicateArg]) -> Result<String, LexicalPlannerError> {
    if args.len() != 1 {
        return Err(LexicalPlannerError::Unimplemented {
            node: predicate_arity_label(name),
            owner_ticket: PREDICATE_OWNER,
        });
    }
    let Some(arg) = args.first() else {
        return Err(LexicalPlannerError::Unimplemented {
            node: predicate_arity_label(name),
            owner_ticket: PREDICATE_OWNER,
        });
    };
    match arg {
        LqPredicateArg::Keyword(v) | LqPredicateArg::Phrase(v) | LqPredicateArg::RawString(v) => {
            Ok(v.clone())
        }
        LqPredicateArg::Number(_) | LqPredicateArg::Filter { .. } => {
            Err(LexicalPlannerError::Unimplemented {
                node: predicate_arity_label(name),
                owner_ticket: PREDICATE_OWNER,
            })
        }
    }
}

/// Stable diagnostic label for a predicate arity / shape rejection.
///
/// `static`-lifetime guarantee comes from the closed set of predicate names
/// the planner accepts today; unknown names route through the
/// `Unimplemented { node: "predicate_leaf" }` arm above without this helper.
fn predicate_arity_label(name: &str) -> &'static str {
    if name == "symbol.has.name" {
        "predicate_symbol_has_name_arity"
    } else if matches!(canonical_predicate_name(name), Some("repo.has.file")) {
        "predicate_repo_has_file_arity"
    } else if matches!(canonical_predicate_name(name), Some("repo.has.content")) {
        "predicate_repo_has_content_arity"
    } else if matches!(canonical_predicate_name(name), Some("file.contains")) {
        "predicate_file_contains_arity"
    } else if matches!(canonical_predicate_name(name), Some("file.has.content")) {
        "predicate_file_has_content_arity"
    } else {
        "predicate_leaf_arity"
    }
}

/// Validate `repo.has.file(...)` arguments by delegating to the registry-owned
/// matcher parser, so the planner and lexical lowering share one argument
/// contract. The parsed matchers are discarded here; the planner only needs to
/// confirm the shape is admissible before emitting `PlanLeaf::Predicate`.
fn validate_repo_has_file_args(
    name: &str,
    args: &[LqPredicateArg],
) -> Result<(), LexicalPlannerError> {
    parse_repo_file_matchers(args)
        .map(|_constraint| ())
        .map_err(|_err| LexicalPlannerError::Unimplemented {
            node: predicate_arity_label(name),
            owner_ticket: PREDICATE_OWNER,
        })
}

/// Default per-leaf candidate cap by engine.
///
/// `None` today — caps land with LXE-04 (trigram) / LXE-06 (symbol) when the
/// engines that actually need a cap come online. Defined here so the cap
/// policy is in one place rather than scattered across leaf constructors.
const fn default_leaf_cap(_engine: EngineKind) -> Option<CandidateCap> {
    None
}

/// Render a `PlanLeaf` to a short diagnostic summary for the explain trace.
fn describe_leaf(leaf: &PlanLeaf) -> String {
    match leaf {
        PlanLeaf::Content { term } => format!("content:{term}"),
        PlanLeaf::Path { pattern } => format!("path:{pattern}"),
        PlanLeaf::Predicate { name } => format!("predicate:{name}"),
        PlanLeaf::Symbol { name, .. } => format!("symbol:{name}"),
        PlanLeaf::Regex { source, .. } => format!("regex:{source}"),
        PlanLeaf::RawSubstring { needle, .. } => format!("raw:{needle}"),
        PlanLeaf::Phrase { phrase, .. } => format!("phrase:{phrase}"),
        PlanLeaf::StructuralRef { handle } => format!("structural:{handle}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{LexicalPlanner, LexicalPlannerError};
    use crate::plan::{EngineKind, LexicalPlan, PlanLeaf, PlanNode, PlanTraceNode};
    use crate::predicate_registry::PREDICATE_OWNER;
    use quanta_index_contract::{LqExpr, LqLeaf, LqQuery, LqSpan};

    fn empty_query() -> LqQuery {
        LqQuery::empty(LqSpan::synthetic(0))
    }

    fn query_with_expr(expr: LqExpr) -> LqQuery {
        let mut q = empty_query();
        q.expr = expr;
        q
    }

    #[test]
    fn empty_query_produces_empty_plan() {
        let q = empty_query();
        let outcome: Result<LexicalPlan, LexicalPlannerError> = LexicalPlanner::plan(&q);
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            assert_eq!(plan.root, PlanNode::Empty);
            assert!(plan.engines.is_empty());
            assert_eq!(plan.trace, PlanTraceNode::Empty);
        }
    }

    #[test]
    fn single_keyword_leaf_produces_content_plan() {
        let q = query_with_expr(LqExpr::Leaf(LqLeaf::Keyword("alpha".to_owned())));
        let outcome = LexicalPlanner::plan(&q);
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            let matched_leaf = matches!(
                &plan.root,
                PlanNode::Leaf { leaf: PlanLeaf::Content { term }, .. } if term == "alpha"
            );
            assert!(matched_leaf, "expected content leaf, got {:?}", plan.root);
            assert!(plan.engines.contains(&EngineKind::Tantivy));
            assert_eq!(plan.engines.len(), 1);
            let matched_trace = matches!(
                &plan.trace,
                PlanTraceNode::Leaf { engine: EngineKind::Tantivy, summary } if summary == "content:alpha"
            );
            assert!(
                matched_trace,
                "expected leaf trace summary, got {:?}",
                plan.trace
            );
        }
    }

    #[test]
    fn regex_leaf_with_extractable_literal_plans_ok() {
        // `foo.bar` has the mandatory literal `foo` / `bar` segments the
        // `regex_syntax::hir::literal::Extractor` should surface; the
        // planner accepts it under LXE-04 and routes to the Regex engine.
        let q = query_with_expr(LqExpr::Leaf(LqLeaf::Regex(r"foo.bar".to_owned())));
        let outcome = LexicalPlanner::plan(&q);
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            let matched_leaf = matches!(
                &plan.root,
                PlanNode::Leaf { leaf: PlanLeaf::Regex { source, plan: regex_plan }, .. }
                    if source == "foo.bar" && !regex_plan.required_literals().is_empty()
            );
            assert!(
                matched_leaf,
                "expected regex leaf with non-empty literals, got {:?}",
                plan.root
            );
            assert!(plan.engines.contains(&EngineKind::Regex));
        }
    }

    #[test]
    fn regex_leaf_with_lookbehind_is_rejected_typed() {
        // `(?<=x)y` is a lookbehind — forbidden by the LQ regex dialect.
        let q = query_with_expr(LqExpr::Leaf(LqLeaf::Regex(r"(?<=x)y".to_owned())));
        let outcome = LexicalPlanner::plan(&q);
        let is_lookbehind = matches!(
            &outcome,
            Err(LexicalPlannerError::RegexPlan(
                crate::regex::RegexPlannerError::UnsupportedFeature {
                    feature: "lookbehind"
                }
            ))
        );
        assert!(
            is_lookbehind,
            "expected lookbehind rejection, got {outcome:?}"
        );
    }

    #[test]
    fn raw_substring_too_short_is_rejected_typed() {
        // 2-byte needle is below the trigram-width minimum.
        let q = query_with_expr(LqExpr::Leaf(LqLeaf::RawString("ab".to_owned())));
        let outcome = LexicalPlanner::plan(&q);
        let is_too_short = matches!(
            &outcome,
            Err(LexicalPlannerError::TrigramPlan(
                crate::trigram_plan::TrigramPlannerError::NeedleTooShort {
                    len: 2,
                    min_required: 3,
                }
            ))
        );
        assert!(
            is_too_short,
            "expected NeedleTooShort rejection, got {outcome:?}"
        );
    }

    #[test]
    fn raw_substring_plans_ok_on_minimum_needle() {
        // Exactly 3 bytes satisfies the trigram minimum.
        let q = query_with_expr(LqExpr::Leaf(LqLeaf::RawString("foo".to_owned())));
        let outcome = LexicalPlanner::plan(&q);
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            let matched_leaf = matches!(
                &plan.root,
                PlanNode::Leaf { leaf: PlanLeaf::RawSubstring { needle, .. }, .. }
                    if needle == "foo"
            );
            assert!(
                matched_leaf,
                "expected raw_substring leaf, got {:?}",
                plan.root
            );
            assert!(plan.engines.contains(&EngineKind::Trigram));
        }
    }

    #[test]
    fn phrase_leaf_plans_ok_with_phrase_plan_payload() {
        // LXE-05 wiring: phrase leaf now plans into a `PhrasePlan` rather
        // than returning `Unimplemented`.
        let q = query_with_expr(LqExpr::Leaf(LqLeaf::Phrase("hello world".to_owned())));
        let outcome = LexicalPlanner::plan(&q);
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            let matched_leaf = matches!(
                &plan.root,
                PlanNode::Leaf { leaf: PlanLeaf::Phrase { phrase, plan: phrase_plan }, .. }
                    if phrase == "hello world" && phrase_plan.tokens.len() == 2
            );
            assert!(
                matched_leaf,
                "expected phrase leaf with two normalized tokens, got {:?}",
                plan.root
            );
            assert!(plan.engines.contains(&EngineKind::Positions));
        }
    }

    #[test]
    fn predicate_symbol_has_name_plans_through_symbol_route() {
        // LXE-06 wiring: `symbol.has.name(foo)` lands on `PlanLeaf::Symbol`
        // with a `SymbolPlan` payload.
        use quanta_index_contract::LqPredicateArg;
        let q = query_with_expr(LqExpr::Leaf(LqLeaf::Predicate {
            name: "symbol.has.name".to_owned(),
            args: vec![LqPredicateArg::Keyword("foo".to_owned())],
        }));
        let outcome = LexicalPlanner::plan(&q);
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            let matched_leaf = matches!(
                &plan.root,
                PlanNode::Leaf { leaf: PlanLeaf::Symbol { name, plan: symbol_plan }, .. }
                    if name == "foo" && symbol_plan.needle == "foo"
            );
            assert!(
                matched_leaf,
                "expected symbol leaf with needle `foo`, got {:?}",
                plan.root
            );
            assert!(plan.engines.contains(&EngineKind::Symbol));
        }
    }

    #[test]
    fn predicate_repo_has_file_plans_through_tantivy_route() {
        use quanta_index_contract::LqPredicateArg;
        for args in [
            vec![LqPredicateArg::Filter {
                name: "path".to_owned(),
                value: "src/lib.rs".to_owned(),
            }],
            vec![LqPredicateArg::Keyword("src/lib.rs".to_owned())],
        ] {
            let q = query_with_expr(LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.has.file".to_owned(),
                args,
            }));
            let outcome = LexicalPlanner::plan(&q);
            assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
            if let Ok(plan) = outcome {
                let matched_leaf = matches!(
                    &plan.root,
                    PlanNode::Leaf { leaf: PlanLeaf::Predicate { name }, .. }
                        if name == "repo.has.file"
                );
                assert!(
                    matched_leaf,
                    "expected predicate leaf for repo.has.file, got {:?}",
                    plan.root
                );
                assert!(plan.engines.contains(&EngineKind::Tantivy));
            }
        }
    }

    #[test]
    fn predicate_repo_has_content_plans_through_tantivy_route() {
        use quanta_index_contract::LqPredicateArg;
        let q = query_with_expr(LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.content".to_owned(),
            args: vec![LqPredicateArg::Keyword("needle".to_owned())],
        }));
        let outcome = LexicalPlanner::plan(&q);
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            let matched_leaf = matches!(
                &plan.root,
                PlanNode::Leaf { leaf: PlanLeaf::Predicate { name }, .. }
                    if name == "repo.has.content"
            );
            assert!(
                matched_leaf,
                "expected predicate leaf for repo.has.content, got {:?}",
                plan.root
            );
            assert!(plan.engines.contains(&EngineKind::Tantivy));
        }
    }

    #[test]
    fn predicate_native_aliases_plan_through_canonical_tantivy_route() {
        use quanta_index_contract::LqPredicateArg;
        for (name, args, expected_name) in [
            (
                "repo.has.path",
                vec![LqPredicateArg::Keyword("src/lib.rs".to_owned())],
                "repo.has.file",
            ),
            (
                "file.contains.content",
                vec![LqPredicateArg::Phrase("needle".to_owned())],
                "file.contains",
            ),
            (
                "repo.contains.content",
                vec![LqPredicateArg::RawString("needle".to_owned())],
                "repo.has.content",
            ),
        ] {
            let q = query_with_expr(LqExpr::Leaf(LqLeaf::Predicate {
                name: name.to_owned(),
                args,
            }));
            let outcome = LexicalPlanner::plan(&q);
            assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
            if let Ok(plan) = outcome {
                let matched_leaf = matches!(
                    &plan.root,
                    PlanNode::Leaf { leaf: PlanLeaf::Predicate { name }, .. }
                        if name == expected_name
                );
                assert!(
                    matched_leaf,
                    "expected canonical predicate leaf for {name}, got {:?}",
                    plan.root
                );
            }
        }
    }

    #[test]
    fn predicate_content_number_and_scope_shapes_plan_through_tantivy_route() {
        use quanta_index_contract::LqPredicateArg;
        for (name, args) in [
            ("file.contains", vec![LqPredicateArg::Number(1)]),
            ("file.has.content", vec![LqPredicateArg::Number(1)]),
            ("repo.has.content", vec![LqPredicateArg::Number(123)]),
            (
                "file.contains",
                vec![
                    LqPredicateArg::Filter {
                        name: "path".to_owned(),
                        value: "src".to_owned(),
                    },
                    LqPredicateArg::Phrase("needle".to_owned()),
                ],
            ),
            (
                "file.has.content",
                vec![
                    LqPredicateArg::Filter {
                        name: "lang".to_owned(),
                        value: "rust".to_owned(),
                    },
                    LqPredicateArg::Keyword("/v\\d+/".to_owned()),
                ],
            ),
        ] {
            let q = query_with_expr(LqExpr::Leaf(LqLeaf::Predicate {
                name: name.to_owned(),
                args,
            }));
            let outcome = LexicalPlanner::plan(&q);
            assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
            if let Ok(plan) = outcome {
                assert!(
                    matches!(
                        &plan.root,
                        PlanNode::Leaf {
                            leaf: PlanLeaf::Predicate { .. },
                            ..
                        }
                    ),
                    "expected predicate leaf, got {:?}",
                    plan.root
                );
            }
        }
    }

    #[test]
    fn predicate_repo_has_content_rejects_scoped_args() {
        use quanta_index_contract::LqPredicateArg;
        let q = query_with_expr(LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.content".to_owned(),
            args: vec![
                LqPredicateArg::Filter {
                    name: "path".to_owned(),
                    value: "src".to_owned(),
                },
                LqPredicateArg::Number(7),
            ],
        }));
        let outcome = LexicalPlanner::plan(&q);
        assert!(
            matches!(
                outcome,
                Err(LexicalPlannerError::Unimplemented {
                    node: "predicate_repo_has_content_arity",
                    owner_ticket,
                })
                if owner_ticket == PREDICATE_OWNER
            ),
            "expected typed planner rejection, got {outcome:?}"
        );
    }

    #[test]
    fn repo_filter_and_leaf_compose_into_one_plan() {
        // LXE-03 wiring: `repo:foo bar` should produce a plan whose
        // `filters.repo` is populated AND whose `root` carries the leaf
        // for `bar` — neither path silently drops the other.
        use quanta_index_contract::LqFilter;
        let mut q = empty_query();
        q.expr = LqExpr::Leaf(LqLeaf::Keyword("bar".to_owned()));
        q.filters = vec![LqFilter::Repo {
            pattern: "foo".to_owned(),
            revs: Vec::new(),
        }];
        let outcome = LexicalPlanner::plan(&q);
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            assert_eq!(plan.filters.repo.len(), 1, "expected one repo constraint");
            let leaf_present = matches!(
                &plan.root,
                PlanNode::Leaf { leaf: PlanLeaf::Content { term }, .. } if term == "bar"
            );
            assert!(leaf_present, "expected content leaf, got {:?}", plan.root);
        }
    }
}
