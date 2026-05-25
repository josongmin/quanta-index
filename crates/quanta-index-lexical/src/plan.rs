//! Planner IR datatypes for the lexical adapter (ticket LXE-02).
//!
//! Defines the typed boundary that [`crate::planner::LexicalPlanner`] produces
//! and downstream execution paths (LXE-03..LXE-06) will populate and consume.

use std::collections::BTreeSet;

use quanta_index_contract::{LqDirective, LqFilter, LqOptions};

use crate::filters::FilterPlan;
use crate::phrase::PhrasePlan;
use crate::regex::RegexPlan;
use crate::symbol::SymbolPlan;
use crate::trigram_plan::TrigramPlan;

/// The single engine an execution path can route a leaf to.
///
/// Adding a new variant must be a deliberate planner change: the planner's
/// engine-requirements set ([`LexicalPlan::engines`]) is the source of truth
/// for which adapters get woken up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EngineKind {
    /// Tantivy text/BM25 over the content field.
    Tantivy,
    /// Trigram index (LXE-04).
    Trigram,
    /// Compiled regex over a candidate set (LXE-04).
    Regex,
    /// Positional / phrase index (LXE-05).
    Positions,
    /// Symbol-graph index (LXE-06).
    Symbol,
    /// Commit/diff history adapter.
    History,
    /// Structural pattern match adapter.
    Structural,
}

/// Per-leaf cap on candidate document IDs produced before merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateCap(pub u32);

/// A named checkpoint at which the executor must observe cancellation.
///
/// Carries no execution behavior yet: LXE-02 only fixes the *shape* so
/// LXE-03+ executors can hang cancellation tokens off the plan tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancellationCheckpoint {
    /// Stable label for diagnostics / explain trace; not user-visible.
    pub label: &'static str,
}

/// Typed leaf in the planner boolean tree.
///
/// One variant per engine route. Variants are deliberately empty payload
/// records today: LXE-03..LXE-06 fill in the per-engine field shape (compiled
/// terms, regex AST, phrase positions, symbol kind filter, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanLeaf {
    /// Tokenized content match — Tantivy BM25 over `chunk_text`.
    Content { term: String },
    /// Path / file-name predicate (LXE-03).
    Path { pattern: String },
    /// Executable predicate that still routes through the Tantivy adapter.
    ///
    /// The planner validates the predicate shape up-front; execution stays on
    /// the adapter's predicate-preparation path (`prepare_predicate_plan`).
    Predicate { name: String },
    /// Symbol-name predicate (LXE-06).
    ///
    /// `name` is preserved for diagnostics; `plan` carries the typed
    /// [`SymbolPlan`] (needle, kind filter, candidate cap, trace builder)
    /// so the executor does not re-plan on the hot path.
    Symbol { name: String, plan: SymbolPlan },
    /// Regex match over a content candidate set (LXE-04).
    ///
    /// `source` is preserved for diagnostics; `plan` carries the typed
    /// `RegexPlan` (compiled-once metadata and trace builder) so the
    /// executor does not re-plan on the hot path.
    Regex { source: String, plan: RegexPlan },
    /// Raw substring match (literal, no tokenization).
    ///
    /// `needle` is preserved for diagnostics; `plan` carries the typed
    /// `TrigramPlan` (per-leaf cap and trace builder) so the executor
    /// does not re-plan on the hot path.
    RawSubstring { needle: String, plan: TrigramPlan },
    /// Phrase / positional match (LXE-05).
    ///
    /// `phrase` is preserved for diagnostics; `plan` carries the typed
    /// [`PhrasePlan`] (normalized tokens, case carryover, candidate cap,
    /// trace builder) so the executor does not re-plan on the hot path.
    Phrase { phrase: String, plan: PhrasePlan },
    /// Structural-pattern reference (resolved by the structural adapter).
    StructuralRef { handle: String },
    /// Semantic-vector reference (resolved by the semantic adapter).
    SemanticVectorRef { handle: String },
}

impl PlanLeaf {
    /// Which engine owns execution of this leaf.
    #[must_use]
    pub const fn engine(&self) -> EngineKind {
        match self {
            Self::Symbol { .. } => EngineKind::Symbol,
            Self::Regex { .. } => EngineKind::Regex,
            Self::RawSubstring { .. } => EngineKind::Trigram,
            Self::Phrase { .. } => EngineKind::Positions,
            Self::StructuralRef { .. } => EngineKind::Structural,
            // Content, Path, and SemanticVectorRef all currently flow through
            // the Tantivy reader (content/path as schema fields, semantic via
            // a Tantivy-side stored handle). A future split lands when the
            // semantic adapter takes over its own route.
            Self::Content { .. }
            | Self::Path { .. }
            | Self::Predicate { .. }
            | Self::SemanticVectorRef { .. } => EngineKind::Tantivy,
        }
    }
}

/// Normalized boolean tree node. Mirrors the lq-norm AST shape but is
/// engine-aware: leaves carry [`PlanLeaf`] and the executor can dispatch
/// directly without re-inspecting raw AST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanNode {
    /// Empty plan (no rows). Distinct from `All(vec![])` (= match-everything)
    /// because LXE-02 treats `LqExpr::Empty` as the inert input.
    Empty,
    /// Leaf with optional per-leaf cap.
    Leaf {
        leaf: PlanLeaf,
        cap: Option<CandidateCap>,
    },
    /// Logical NOT over a subtree.
    Not(Box<PlanNode>),
    /// Conjunction of children. n-ary; `[]` collapses upstream.
    All(Vec<PlanNode>),
    /// Disjunction of children. n-ary; `[]` collapses upstream.
    Any(Vec<PlanNode>),
}

/// Explain-trace node: mirrors plan structure, used for diagnostics.
///
/// `SearchExplanation` (see `core::domains::lexical`) will pull these directly
/// from the plan rather than reconstructing them from display strings (per
/// the LXE-02 definition-of-done).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanTraceNode {
    Empty,
    Leaf { engine: EngineKind, summary: String },
    Not(Box<PlanTraceNode>),
    All(Vec<PlanTraceNode>),
    Any(Vec<PlanTraceNode>),
}

/// The planner's IR output. One value per planned query.
///
/// Construction is the planner's responsibility: callers outside the lexical
/// crate must not synthesize plans directly (the public surface is
/// [`crate::planner::LexicalPlanner::plan`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalPlan {
    /// Normalized boolean tree the executor walks.
    pub root: PlanNode,
    /// Pre-candidate filters resolved into executable constraints (repo /
    /// path / lang), case+count policy, terminal result surface, and the
    /// set of typed-unavailable codes the executor must surface in lieu of
    /// silent ignore. See [`FilterPlan`] for the full shape.
    pub filters: FilterPlan,
    /// Raw input filters preserved unchanged so the executor still has
    /// access to the original boolean shape (`LqFilter::Content`) when it
    /// walks the boolean tree. The planner does NOT route execution
    /// through this field; `filters` (above) is the executable surface.
    pub raw_filters: Vec<LqFilter>,
    /// Set of engines this plan touches. Sorted/de-duplicated via `BTreeSet`.
    pub engines: BTreeSet<EngineKind>,
    /// Plan-wide candidate cap (final merge cap). `None` means use the
    /// executor's policy default.
    pub plan_cap: Option<CandidateCap>,
    /// Cancellation checkpoints the executor must observe.
    pub checkpoints: Vec<CancellationCheckpoint>,
    /// Explain trace mirroring the plan root.
    pub trace: PlanTraceNode,
    /// Carrier of normalizer-level options (case, `pattern_type`, count). The
    /// executor reads these alongside the plan rather than re-deriving them
    /// from the raw query.
    pub options: LqOptions,
    /// Directives carried through unchanged (codeql, scope, with).
    pub directives: Vec<LqDirective>,
}

impl LexicalPlan {
    /// Construct an inert plan equivalent to `LqExpr::Empty`.
    ///
    /// Used as the canonical zero value and as the result of planning an
    /// empty query. The caller supplies the already-planned [`FilterPlan`]
    /// because filter planning is independent of expression planning and
    /// the planner runs it first.
    #[must_use]
    pub fn empty(
        options: LqOptions,
        directives: Vec<LqDirective>,
        filters: FilterPlan,
        raw_filters: Vec<LqFilter>,
    ) -> Self {
        Self {
            root: PlanNode::Empty,
            filters,
            raw_filters,
            engines: BTreeSet::new(),
            plan_cap: None,
            checkpoints: Vec::new(),
            trace: PlanTraceNode::Empty,
            options,
            directives,
        }
    }
}
