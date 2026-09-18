//! `RequiredDomainsV1` is a pure function of the route and the lowered
//! plan (plan §5.6).
//!
//! Every predicate the plane executes declares the domain it reads, and
//! every route declares the domains it always reads.

#![forbid(unsafe_code)]

use quanta_index_contract::{
    LQ_VERSION_TAG, LqExpr, LqFilter, LqLeaf, LqOptions, LqPredicateArg, LqQuery, LqSelect, LqSpan,
    LqStructuralBlock, LqStructuralExpr, LqStructuralNode, LqType, LqYesNoOnly,
};
use quanta_index_core::{
    LexicalPredicateAliasV1, LexicalPredicateV1, QueryRouteV1, ReadDomainV1,
    RepoMetadataAuthorityV1, RequiredDomainsV1, declare_required_domains_v1, lexical_predicate_v1,
};

fn query(expr: LqExpr, filters: Vec<LqFilter>) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr,
        filters,
        directives: Vec::new(),
        options: LqOptions::defaults(),
        source_span: LqSpan::synthetic(0),
    }
}

fn keyword(text: &str) -> LqExpr {
    LqExpr::Leaf(LqLeaf::Keyword(text.to_string()))
}

fn predicate(name: &str) -> LqExpr {
    LqExpr::Leaf(LqLeaf::Predicate {
        name: name.to_string(),
        args: vec![LqPredicateArg::Keyword("x".to_string())],
    })
}

fn structural_block() -> LqExpr {
    let nodes = vec![LqStructuralNode::Literal("fn".into())];
    LqExpr::Leaf(LqLeaf::StructuralBlock(LqStructuralBlock {
        lang: Some("rust".to_string()),
        nodes: nodes.clone(),
        exprs: vec![LqStructuralExpr::Pattern(nodes)],
    }))
}

const LEXICAL_PLAN_ROUTES: [QueryRouteV1; 5] = [
    QueryRouteV1::Lexical,
    QueryRouteV1::Symbol,
    QueryRouteV1::Hybrid,
    QueryRouteV1::HybridSeed,
    QueryRouteV1::Explain,
];

/// The domain a predicate leaf adds, as the declaration computes it on the
/// lexical route, isolated from the route's base set.
fn predicate_domain_on_lexical_route(name: &str) -> RequiredDomainsV1 {
    let declared =
        declare_required_domains_v1(QueryRouteV1::Lexical, Some(&query(predicate(name), vec![])));
    declared
        .iter()
        .filter(|domain| *domain != ReadDomainV1::LexicalTrack)
        .collect()
}

/// Every predicate name the plane executes declares the domain it reads.
///
/// Canonical or alias, each name adds exactly the domain its variant
/// reads, and the declaration is total over the enumeration: a variant
/// without a domain cannot exist because `read_domain` is an exhaustive
/// match, and this test proves the plan walk reaches it for every name.
#[test]
fn every_registered_predicate_declares_the_domain_it_reads() {
    for predicate_kind in LexicalPredicateV1::ALL {
        let expected = match predicate_kind.read_domain() {
            ReadDomainV1::LexicalTrack => RequiredDomainsV1::NONE,
            other @ (ReadDomainV1::SemanticTrack
            | ReadDomainV1::StructuralChunkUniverse
            | ReadDomainV1::RuntimeOverlay
            | ReadDomainV1::History
            | ReadDomainV1::RepoMap
            | ReadDomainV1::RepoMetadata(_)) => RequiredDomainsV1::of(other),
        };
        assert_eq!(
            predicate_domain_on_lexical_route(predicate_kind.name()),
            expected,
            "{}",
            predicate_kind.name()
        );
        assert_eq!(
            lexical_predicate_v1(predicate_kind.name()),
            Some(predicate_kind)
        );
    }
    for alias in LexicalPredicateAliasV1::ALL {
        assert_eq!(
            predicate_domain_on_lexical_route(alias.name()),
            predicate_domain_on_lexical_route(alias.canonical().name()),
            "{} reads what {} reads",
            alias.name(),
            alias.canonical().name()
        );
    }
}

/// The repo-metadata predicates each name their own authority; the
/// index-backed ones name none beyond the lexical track.
#[test]
fn repo_metadata_predicates_name_their_authority() {
    let cases = [
        (
            "repo.has.commit.after",
            RepoMetadataAuthorityV1::CommitRecency,
        ),
        (
            "repo.contains.commit.after",
            RepoMetadataAuthorityV1::CommitRecency,
        ),
        ("repo.has.meta", RepoMetadataAuthorityV1::Meta),
        ("repo.has.topic", RepoMetadataAuthorityV1::Topic),
        ("repo.has.description", RepoMetadataAuthorityV1::Description),
        ("file.has.owner", RepoMetadataAuthorityV1::FileOwnership),
        ("file.has.contributor", RepoMetadataAuthorityV1::Contributor),
    ];
    for (name, authority) in cases {
        assert_eq!(
            predicate_domain_on_lexical_route(name),
            RequiredDomainsV1::of(ReadDomainV1::RepoMetadata(authority)),
            "{name}"
        );
    }
    for name in [
        "file.contains",
        "file.has.content",
        "file.contains.content",
        "repo.has.file",
        "repo.has.path",
        "repo.contains.file",
        "repo.contains.path",
        "repo.has.content",
        "repo.contains.content",
        "symbol.has.name",
    ] {
        assert_eq!(
            predicate_domain_on_lexical_route(name),
            RequiredDomainsV1::NONE,
            "{name}"
        );
    }
}

#[test]
fn a_plain_keyword_reads_the_lexical_track_only() {
    let plan = query(keyword("needle"), vec![]);
    for route in [
        QueryRouteV1::Lexical,
        QueryRouteV1::Symbol,
        QueryRouteV1::Explain,
    ] {
        assert_eq!(
            declare_required_domains_v1(route, Some(&plan)),
            RequiredDomainsV1::of(ReadDomainV1::LexicalTrack),
            "{route:?}"
        );
    }
    // A presence-only explain has no plan and still reads the track.
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::Explain, None),
        RequiredDomainsV1::of(ReadDomainV1::LexicalTrack)
    );
}

#[test]
fn an_unregistered_predicate_adds_no_domain() {
    let plan = query(predicate("repo.has.tag"), vec![]);
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::Lexical, Some(&plan)),
        RequiredDomainsV1::of(ReadDomainV1::LexicalTrack)
    );
}

#[test]
fn predicates_are_found_anywhere_in_the_tree_and_in_content_filters() {
    let plan = query(
        LqExpr::All(vec![
            keyword("needle"),
            LqExpr::Not(Box::new(LqExpr::Any(vec![
                predicate("repo.has.meta"),
                keyword("other"),
            ]))),
        ]),
        vec![LqFilter::Content {
            leaf: LqLeaf::Predicate {
                name: "file.has.owner".to_string(),
                args: Vec::new(),
            },
        }],
    );
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::Lexical, Some(&plan)),
        RequiredDomainsV1::of(ReadDomainV1::LexicalTrack)
            .with(ReadDomainV1::RepoMetadata(RepoMetadataAuthorityV1::Meta))
            .with(ReadDomainV1::RepoMetadata(
                RepoMetadataAuthorityV1::FileOwnership
            ))
    );
}

#[test]
fn select_file_owners_reads_the_ownership_authority() {
    let plan = query(
        keyword("needle"),
        vec![LqFilter::Select {
            dim: LqSelect::FileOwners,
        }],
    );
    for route in LEXICAL_PLAN_ROUTES {
        assert!(
            declare_required_domains_v1(route, Some(&plan)).contains(ReadDomainV1::RepoMetadata(
                RepoMetadataAuthorityV1::FileOwnership
            )),
            "{route:?}"
        );
    }
    let other_selects = query(
        keyword("needle"),
        vec![LqFilter::Select {
            dim: LqSelect::File,
        }],
    );
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::Lexical, Some(&other_selects)),
        RequiredDomainsV1::of(ReadDomainV1::LexicalTrack)
    );
}

/// `rev:at.time(...)` rebinds the generation through the history
/// authority on the routes that select by it; a plain `rev:` and the
/// routes that do not select add nothing.
#[test]
fn rev_at_time_reads_history_where_the_route_selects_by_it() {
    let at_time = query(
        keyword("needle"),
        vec![LqFilter::Rev {
            spec: "at.time(2024-01-01)".to_string(),
        }],
    );
    for route in [QueryRouteV1::Lexical, QueryRouteV1::Explain] {
        assert_eq!(
            declare_required_domains_v1(route, Some(&at_time)),
            RequiredDomainsV1::of(ReadDomainV1::LexicalTrack).with(ReadDomainV1::History),
            "{route:?}"
        );
    }
    for route in [
        QueryRouteV1::Symbol,
        QueryRouteV1::Hybrid,
        QueryRouteV1::HybridSeed,
    ] {
        assert!(
            !declare_required_domains_v1(route, Some(&at_time)).contains(ReadDomainV1::History),
            "{route:?}"
        );
    }
    let plain_rev = query(
        keyword("needle"),
        vec![LqFilter::Rev {
            spec: "main".to_string(),
        }],
    );
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::Lexical, Some(&plain_rev)),
        RequiredDomainsV1::of(ReadDomainV1::LexicalTrack)
    );
}

#[test]
fn hybrid_routes_read_both_tracks_plus_their_predicates() {
    let plan = query(predicate("repo.has.topic"), vec![]);
    for route in [QueryRouteV1::Hybrid, QueryRouteV1::HybridSeed] {
        assert_eq!(
            declare_required_domains_v1(route, Some(&plan)),
            RequiredDomainsV1::of(ReadDomainV1::LexicalTrack)
                .with(ReadDomainV1::SemanticTrack)
                .with(ReadDomainV1::RepoMetadata(RepoMetadataAuthorityV1::Topic)),
            "{route:?}"
        );
    }
}

#[test]
fn semantic_reads_the_lexical_track_only_when_scoped() {
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::Semantic, None),
        RequiredDomainsV1::of(ReadDomainV1::SemanticTrack)
    );
    let scope = query(predicate("file.has.contributor"), vec![]);
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::Semantic, Some(&scope)),
        RequiredDomainsV1::of(ReadDomainV1::SemanticTrack)
            .with(ReadDomainV1::LexicalTrack)
            .with(ReadDomainV1::RepoMetadata(
                RepoMetadataAuthorityV1::Contributor
            ))
    );
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::ClusterMembershipRead, None),
        RequiredDomainsV1::of(ReadDomainV1::SemanticTrack)
    );
}

/// A runtime predicate reads the overlay and the chunk universe it is
/// joined with; the route's filter families add nothing beyond that.
#[test]
fn runtime_predicates_read_the_overlay_and_the_chunk_universe() {
    let plan = query(
        keyword("needle"),
        vec![
            LqFilter::Dirty {
                mode: LqYesNoOnly::Yes,
            },
            LqFilter::Changed {
                scope: "since=1970-01-01T00:00:00Z".to_string(),
            },
            LqFilter::MetaOwner {
                id: "team".to_string(),
            },
        ],
    );
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::RuntimeMetadata, Some(&plan)),
        RequiredDomainsV1::of(ReadDomainV1::RuntimeOverlay)
            .with(ReadDomainV1::StructuralChunkUniverse)
    );
}

/// A history query reads the history authority and nothing else, whatever
/// its filters say.
#[test]
fn history_reads_the_history_authority_only() {
    let plan = query(
        keyword("fix"),
        vec![
            LqFilter::Type {
                kind: LqType::Commit,
            },
            LqFilter::Author {
                pattern: "alice".to_string(),
            },
            LqFilter::Rev {
                spec: "at.time(2024-01-01)".to_string(),
            },
        ],
    );
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::History, Some(&plan)),
        RequiredDomainsV1::of(ReadDomainV1::History)
    );
}

#[test]
fn structural_adds_the_lexical_track_only_for_a_text_leaf() {
    let pure = query(structural_block(), vec![]);
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::Structural, Some(&pure)),
        RequiredDomainsV1::of(ReadDomainV1::StructuralChunkUniverse)
    );
    let mixed = query(
        LqExpr::All(vec![structural_block(), predicate("repo.has.file")]),
        vec![],
    );
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::Structural, Some(&mixed)),
        RequiredDomainsV1::of(ReadDomainV1::StructuralChunkUniverse)
            .with(ReadDomainV1::LexicalTrack)
    );
}

#[test]
fn repo_map_reads_the_repo_map_store() {
    assert_eq!(
        declare_required_domains_v1(QueryRouteV1::RepoMap, None),
        RequiredDomainsV1::of(ReadDomainV1::RepoMap)
    );
}
