//! `RequiredDomainsV1` as a pure function of the route and the lowered
//! plan (plan §5.6).
//!
//! A route fixes the domains it always reads; the lowered plan adds the
//! authorities its leaves and filters read. Nothing here consults a
//! ledger, a catalog or a handle: the declaration is decided from the
//! plan alone, before anything is acquired, so the read view can pin
//! exactly the declared set and refuse a missing domain typed before any
//! lane runs.

use quanta_index_contract::{LqExpr, LqFilter, LqLeaf, LqQuery, LqSelect};

use super::domain::{ReadDomainV1, RepoMetadataAuthorityV1, RequiredDomainsV1};
use super::predicate::lexical_predicate_v1;
use crate::timeref::parse_rev_at_time_spec;

/// The query routes of the search plane, as the declaration sees them.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum QueryRouteV1 {
    /// The lexical text route: a lexical plan over the pinned generation.
    Lexical,
    /// The symbol route: a lexical plan over the symbol index.
    Symbol,
    /// The semantic route: a dense lane, optionally scoped by a lexical
    /// plan.
    Semantic,
    /// The hybrid route: an independent lexical lane and dense lane.
    Hybrid,
    /// The hybrid-seed route: a lexical lane and per-corpus dense lanes.
    HybridSeed,
    /// The history route: commits and diff hunks of one history epoch.
    History,
    /// The structural route: parse-tree matches over the structural
    /// snapshot, joined with lexical leaves when the tree has them.
    Structural,
    /// The runtime-metadata route: runtime predicates over the chunk
    /// universe.
    RuntimeMetadata,
    /// The `RepoMap` route.
    RepoMap,
    /// The explain route: a lexical plan re-scored for one candidate, or a
    /// presence lookup.
    Explain,
    /// The cluster-membership batch read over the semantic generation.
    ClusterMembershipRead,
}

impl QueryRouteV1 {
    /// The domains the route reads whatever the plan says.
    const fn base_domains(self) -> RequiredDomainsV1 {
        match self {
            Self::Lexical | Self::Symbol | Self::Explain => {
                RequiredDomainsV1::of(ReadDomainV1::LexicalTrack)
            }
            Self::Semantic | Self::ClusterMembershipRead => {
                RequiredDomainsV1::of(ReadDomainV1::SemanticTrack)
            }
            Self::Hybrid | Self::HybridSeed => {
                RequiredDomainsV1::of(ReadDomainV1::LexicalTrack).with(ReadDomainV1::SemanticTrack)
            }
            Self::History => RequiredDomainsV1::of(ReadDomainV1::History),
            Self::Structural => RequiredDomainsV1::of(ReadDomainV1::StructuralChunkUniverse),
            Self::RuntimeMetadata => RequiredDomainsV1::of(ReadDomainV1::RuntimeOverlay)
                .with(ReadDomainV1::StructuralChunkUniverse),
            Self::RepoMap => RequiredDomainsV1::of(ReadDomainV1::RepoMap),
        }
    }

    /// Whether the plan the route executes is a lexical plan whose leaves
    /// and filters name the authorities they read.
    ///
    /// The history and runtime-metadata routes refuse predicate leaves
    /// before execution and evaluate their filter families inside their
    /// base domain; `RepoMap` and the cluster read carry no text plan.
    const fn executes_lexical_plan(self) -> bool {
        match self {
            Self::Lexical
            | Self::Symbol
            | Self::Semantic
            | Self::Hybrid
            | Self::HybridSeed
            | Self::Structural
            | Self::Explain => true,
            Self::History | Self::RuntimeMetadata | Self::RepoMap | Self::ClusterMembershipRead => {
                false
            }
        }
    }

    /// Whether the route rebinds its generation for `rev:at.time(...)`
    /// through the history authority of the requested generation.
    const fn selects_rev_at_time(self) -> bool {
        match self {
            Self::Lexical | Self::Explain => true,
            Self::Symbol
            | Self::Semantic
            | Self::Hybrid
            | Self::HybridSeed
            | Self::History
            | Self::Structural
            | Self::RuntimeMetadata
            | Self::RepoMap
            | Self::ClusterMembershipRead => false,
        }
    }
}

/// The domains one route reads to execute `plan`.
///
/// `plan` is the lowered query the route executes: the text plan of the
/// lexical, symbol, hybrid, hybrid-seed and explain routes, the lexical
/// *scope* of the semantic route (`None` when unscoped), the structural
/// tree of the structural route, and the filter plan of the history and
/// runtime-metadata routes; `None` for `RepoMap`, the cluster read and a
/// presence-only explain.
///
/// On a route that executes a lexical plan every predicate leaf adds the
/// domain it reads, `select:file.owners` adds the ownership authority,
/// and — on the routes that rebind for it — `rev:at.time(...)` adds the
/// history authority the selection walks. A structural tree adds the
/// lexical track only when it has a leaf that is not a `match { ... }`
/// block. An unregistered predicate name adds nothing: it cannot execute
/// and the adapter refuses it typed.
#[must_use]
pub fn declare_required_domains_v1(
    route: QueryRouteV1,
    plan: Option<&LqQuery>,
) -> RequiredDomainsV1 {
    let mut domains = route.base_domains();
    let Some(query) = plan else {
        return domains;
    };
    if !route.executes_lexical_plan() {
        return domains;
    }
    let mut leaves = LeafReads::default();
    collect_expr_reads(&query.expr, &mut leaves);
    for filter in &query.filters {
        match filter {
            LqFilter::Content { leaf } => collect_leaf_reads(leaf, &mut leaves),
            LqFilter::Select {
                dim: LqSelect::FileOwners,
            } => leaves
                .domains
                .insert(ReadDomainV1::RepoMetadata(RepoMetadataAuthorityV1::FileOwnership)),
            LqFilter::Rev { spec } => {
                if route.selects_rev_at_time() && parse_rev_at_time_spec(spec).is_some() {
                    leaves.domains.insert(ReadDomainV1::History);
                }
            }
            LqFilter::Select {
                dim:
                    LqSelect::Repo
                    | LqSelect::File
                    | LqSelect::Path
                    | LqSelect::Symbol
                    | LqSelect::Content
                    | LqSelect::ContentMatch,
            }
            | LqFilter::Repo { .. }
            | LqFilter::File { .. }
            | LqFilter::Lang { .. }
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
            | LqFilter::Context { .. } => {}
        }
    }
    if route == QueryRouteV1::Structural {
        if leaves.text_leaf {
            domains.insert(ReadDomainV1::LexicalTrack);
        }
    } else if route == QueryRouteV1::Semantic {
        // A scope plan is a lexical lane.
        domains.insert(ReadDomainV1::LexicalTrack);
    }
    domains.union(leaves.domains)
}

/// What the leaves of a plan read.
#[derive(Default)]
struct LeafReads {
    /// The authorities the leaves name.
    domains: RequiredDomainsV1,
    /// Whether any leaf is a text leaf rather than a structural block.
    text_leaf: bool,
}

fn collect_expr_reads(expr: &LqExpr, reads: &mut LeafReads) {
    match expr {
        LqExpr::Empty => {}
        LqExpr::Leaf(leaf) => collect_leaf_reads(leaf, reads),
        LqExpr::Not(inner) => collect_expr_reads(inner, reads),
        LqExpr::All(children) | LqExpr::Any(children) => {
            for child in children {
                collect_expr_reads(child, reads);
            }
        }
    }
}

fn collect_leaf_reads(leaf: &LqLeaf, reads: &mut LeafReads) {
    match leaf {
        LqLeaf::Keyword(_) | LqLeaf::Phrase(_) | LqLeaf::RawString(_) | LqLeaf::Regex(_) => {
            reads.text_leaf = true;
        }
        LqLeaf::Predicate { name, args: _ } => {
            reads.text_leaf = true;
            if let Some(predicate) = lexical_predicate_v1(name) {
                reads.domains.insert(predicate.read_domain());
            }
        }
        LqLeaf::StructuralBlock(_) => {}
    }
}
