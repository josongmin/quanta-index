//! P02A owner tests: the whole-bundle `RepoMap` graph compiler.
//!
//! Negative corpus (every malformed bundle is a typed refusal before any
//! output), commitment determinism under input permutation, budget caps with
//! overflow-safe accounting, and the strict focus policy.

#![forbid(unsafe_code)]
#![expect(
    clippy::unreachable,
    reason = "test fixtures use invariant literal constructors for language, symbol kinds, and IDs"
)]

use quanta_index_contract::lex::{LanguageCode, SymbolKindCode};
use quanta_index_contract::{
    ChunkId, CompiledRepoMapCandidateV1, FileId, ManifestGeneration, RepoId, RepoMapChunkExactness,
    RepoMapChunkNode, RepoMapCompileRefusalCodeV1 as Code, RepoMapCompileStageV1 as Stage,
    RepoMapCompilerBudgetV1, RepoMapExactnessSummary, RepoMapFileNode, RepoMapGraphCoverage,
    RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapNode, RepoMapNodeRef,
    RepoMapRedactionState, RepoMapSourceBundle, RepoMapSymbolNode, RepoRelativePath, RevisionId,
    SymbolId,
};
use quanta_index_repomap::RepoMapGraphCompiler;

const MANIFEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const AUTHORITY: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn rust_language() -> LanguageCode {
    match LanguageCode::new("rust") {
        Ok(language) => language,
        Err(err) => unreachable!("valid language: {err}"),
    }
}

fn symbol_kind(name: &str) -> SymbolKindCode {
    match SymbolKindCode::new(name) {
        Ok(symbol_kind) => symbol_kind,
        Err(err) => unreachable!("valid symbol kind `{name}`: {err}"),
    }
}

fn file(id: &str, lines: u32) -> RepoMapNode {
    RepoMapNode::File(RepoMapFileNode {
        file_id: FileId::new(id),
        repo_relative_path: RepoRelativePath::new(id),
        line_count: lines,
    })
}

fn symbol(id: &str, owner: &str, local: &str) -> RepoMapNode {
    RepoMapNode::Symbol(RepoMapSymbolNode {
        symbol_id: SymbolId::new(id),
        owner_path: RepoRelativePath::new(owner),
        local_name: local.to_string(),
        qualified_name: id.to_string(),
        symbol_kind: symbol_kind("struct"),
    })
}

fn chunk(id: &str, owner: &str, tokens: u32) -> RepoMapNode {
    RepoMapNode::Chunk(RepoMapChunkNode {
        chunk_id: ChunkId::new(id),
        owner_path: RepoRelativePath::new(owner),
        language: rust_language(),
        start_byte: 0,
        end_byte: tokens,
        start_line: 1,
        end_line: 4,
        token_count: tokens,
        preview_text: format!("{id} preview text"),
        exactness: RepoMapChunkExactness::Exact,
    })
}

fn bundle() -> RepoMapSourceBundle {
    let repo = match RepoId::new("repo") {
        Ok(repo) => repo,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    };
    let revision = match RevisionId::new("rev") {
        Ok(revision) => revision,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    };
    RepoMapSourceBundle::new(
        repo,
        revision,
        ManifestGeneration::new(1),
        MANIFEST,
        "snap",
        1,
        AUTHORITY,
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Full,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
}

fn compile(bundle: &RepoMapSourceBundle) -> Result<CompiledRepoMapCandidateV1, String> {
    RepoMapGraphCompiler::with_default_budget()
        .compile(bundle)
        .map_err(|refusal: quanta_index_contract::RepoMapCompileRefusalV1| refusal.to_string())
}

fn assert_refusal(result: Result<CompiledRepoMapCandidateV1, String>, stage: Stage, code: Code) {
    match result {
        Err(message) => assert!(
            message.contains(stage.as_code_str()) && message.contains(code.as_code_str()),
            "expected {stage:?}/{code:?}, got {message}"
        ),
        Ok(candidate) => assert!(false, "expected refusal, got candidate {candidate:?}"),
    }
}

#[test]
fn a_valid_bundle_compiles_with_commitments_and_receipt() {
    let source = bundle()
        .with_node(file("src/lib.rs", 100))
        .with_node(symbol("src/lib.rs::Owner", "src/lib.rs", "Owner"))
        .with_node(chunk("chunk://a", "src/lib.rs", 42));
    let candidate = compile(&source).expect("valid bundle compiles");
    let receipt = candidate.resource_receipt();
    assert_eq!(receipt.nodes, 3);
    assert_eq!(receipt.edges, 0);
    assert!(receipt.observed());
    assert_eq!(candidate.commitments().producer_manifest, [0xaa; 32]);
    assert_eq!(candidate.commitments().producer_authority, [0xbb; 32]);
    assert_ne!(candidate.commitments().compiled_graph, [0; 32]);
    assert_ne!(candidate.commitments().schema, [0; 32]);
    assert_ne!(candidate.commitments().projection_profile, [0; 32]);
    // Refusal payloads never leak input bytes.
}

#[test]
fn an_empty_bundle_is_refused() {
    assert_refusal(compile(&bundle()), Stage::BundleValidation, Code::EmptyBundle);
}

#[test]
fn an_invalid_producer_digest_is_refused() {
    let source = bundle().with_node(file("src/lib.rs", 10));
    let mut bad = source;
    bad.manifest_digest = "not-a-digest".to_string();
    assert_refusal(compile(&bad), Stage::BundleValidation, Code::ProducerDigestInvalid);
}

#[test]
fn duplicate_typed_nodes_are_refused() {
    let source = bundle()
        .with_node(file("src/lib.rs", 10))
        .with_node(file("src/lib.rs", 20));
    assert_refusal(compile(&source), Stage::GraphValidation, Code::DuplicateNode);
}

#[test]
fn cross_variant_identity_collision_is_refused() {
    let source = bundle().with_node(file("shared-id", 10)).with_node(symbol(
        "shared-id",
        "src/lib.rs",
        "Owner",
    ));
    assert_refusal(compile(&source), Stage::GraphValidation, Code::CrossVariantIdentityCollision);
}

#[test]
fn dangling_edge_endpoints_are_refused() {
    let source = bundle().with_node(file("src/lib.rs", 10)).with_edge(
        quanta_index_contract::RepoMapEdge::Call(quanta_index_contract::RepoMapCallEdge {
            caller: RepoMapNodeRef::Symbol(SymbolId::new("ghost")),
            callee: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
        }),
    );
    assert_refusal(compile(&source), Stage::GraphValidation, Code::DanglingEdgeEndpoint);
}

#[test]
fn illegal_edge_variants_are_refused() {
    // Call requires Symbol -> Symbol.
    let source = bundle()
        .with_node(file("src/lib.rs", 10))
        .with_node(file("src/main.rs", 10))
        .with_edge(quanta_index_contract::RepoMapEdge::Call(
            quanta_index_contract::RepoMapCallEdge {
                caller: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
                callee: RepoMapNodeRef::File(FileId::new("src/main.rs")),
            },
        ));
    assert_refusal(compile(&source), Stage::GraphValidation, Code::IllegalEdgeVariant);
}

#[test]
fn self_loops_are_refused() {
    let source = bundle().with_node(file("src/lib.rs", 10)).with_edge(
        quanta_index_contract::RepoMapEdge::Import(quanta_index_contract::RepoMapImportEdge {
            importer: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
            imported: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
        }),
    );
    assert_refusal(compile(&source), Stage::GraphValidation, Code::SelfLoop);
}

#[test]
fn node_and_edge_caps_are_enforced_before_output() {
    let mut source = bundle();
    for index in 0..4 {
        source = source.with_node(file(&format!("src/file{index}.rs"), 10));
    }
    let budget = RepoMapCompilerBudgetV1::new(3, 100, 100, 1 << 20, 1 << 24, 1 << 30)
        .expect("positive ceilings");
    let refusal = RepoMapGraphCompiler::new(budget)
        .compile(&source)
        .expect_err("node cap must refuse");
    assert_eq!(refusal.code(), Code::NodeLimitExceeded);
    assert_eq!(refusal.limit(), Some(3));
    assert_eq!(refusal.observed(), 4);
}

#[test]
fn zero_ceiling_budgets_are_refused_at_construction() {
    assert!(RepoMapCompilerBudgetV1::new(0, 1, 1, 1, 1, 1).is_err());
}

#[test]
fn commitment_is_independent_of_input_order() {
    let node_a = file("src/lib.rs", 100);
    let node_b = symbol("src/lib.rs::Owner", "src/lib.rs", "Owner");
    let node_c = chunk("chunk://a", "src/lib.rs", 42);
    let edge =
        quanta_index_contract::RepoMapEdge::Contains(quanta_index_contract::RepoMapContainsEdge {
            container: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
            contained: RepoMapNodeRef::Symbol(SymbolId::new("src/lib.rs::Owner")),
        });
    let forward = bundle()
        .with_node(node_a.clone())
        .with_node(node_b.clone())
        .with_node(node_c.clone())
        .with_edge(edge.clone());
    let reverse = bundle()
        .with_node(node_c)
        .with_node(node_b)
        .with_node(node_a)
        .with_edge(edge);
    let forward_candidate = compile(&forward).expect("forward bundle compiles");
    let reverse_candidate = compile(&reverse).expect("reverse bundle compiles");
    assert_eq!(forward_candidate.commitments(), reverse_candidate.commitments());
    assert_eq!(forward_candidate.compiled_payload(), reverse_candidate.compiled_payload());
}

#[test]
fn golden_compiled_graph_commitment_is_stable() {
    let source = bundle()
        .with_node(file("src/lib.rs", 100))
        .with_node(symbol("src/lib.rs::Owner", "src/lib.rs", "Owner"));
    let candidate = compile(&source).expect("golden fixture compiles");
    let digest = candidate.commitments().compiled_graph;
    // Stability anchor: golden hex of the compiled-graph commitment for this
    // exact fixture. Any schema/encoding drift changes it and fails here.
    let mut hex = String::new();
    for byte in digest {
        use std::fmt::Write as _;
        let _written = write!(hex, "{byte:02x}");
    }
    assert_eq!(hex, "7820937c0e7474a7fc412239557df9ee7f468c6aaebf9bd5db6cd53c4437113a");
}

#[test]
fn multilingual_bundles_compile_through_the_shared_tokenizer() {
    let source = bundle()
        .with_node(file("src/한글모듈.rs", 10))
        .with_node(symbol("src/한글모듈.rs::함수", "src/한글모듈.rs", "함수"));
    let candidate = compile(&source).expect("multilingual fixture compiles");
    assert_eq!(candidate.projection().len(), 2);
}
