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

#[test]
fn compiled_payload_rejects_noncanonical_widths() {
    use quanta_index_repomap::decode_compiled_payload;

    // These fixed wire fixtures vary only the width of one argument.
    assert!(decode_compiled_payload(&[2, 0, 0, 0]).is_ok());
    assert!(decode_compiled_payload(&[2, 0x18, 0, 0, 0]).is_err());
    const CANONICAL_FILE_NODE: &[u8] = b"\x02\x01\x03\x02\x64File\x61x\x00\x00\x00\x00";
    const WIDE_TEXT: &[u8] = b"\x02\x01\x03\x02\x78\x04File\x61x\x00\x00\x00\x00";
    assert!(decode_compiled_payload(CANONICAL_FILE_NODE).is_ok());
    assert!(decode_compiled_payload(WIDE_TEXT).is_err());
}

#[test]
fn compiled_payload_round_trips_multibyte_table_counts() {
    use quanta_index_repomap::decode_compiled_payload;

    // 256 is the first canonical two-byte unsigned argument. A valid table
    // of this size used to be rejected by the decoder's threshold.
    let source = (0..256).fold(bundle(), |source, index| {
        source.with_node(file(&format!("src/region_{index}.rs"), 10))
    });
    let compiled = compile(&source).expect("compile 256 distinct files");
    let (graph, projection) =
        decode_compiled_payload(compiled.compiled_payload()).expect("canonical 256 nodes");
    assert_eq!(graph.nodes.len(), 256);
    assert_eq!(projection.len(), 256);
}

#[test]
fn compiled_payload_uses_the_persisted_signed_score_encoding() {
    use quanta_index_repomap::decode_compiled_payload;

    // One projection with fixed metadata and one already persisted score byte.
    let mut payload = b"\x02\x00\x00\x01\x05\x02\x64File\x61x\x64File\x61x\x60".to_vec();
    let mut decode_score = |encoded: &[u8]| {
        let offset = payload.len();
        payload.extend_from_slice(encoded);
        let decoded = decode_compiled_payload(&payload);
        payload.truncate(offset);
        decoded.map(|(_graph, projection)| projection.first().map(|entry| entry.final_score_millis))
    };
    assert_eq!(decode_score(&[0x21]).expect("score -1"), Some(-1));
    assert_eq!(decode_score(&[0x38, 24]).expect("score -24"), Some(-24));
    assert_eq!(
        decode_score(&[0x3b, 0x80, 0, 0, 0, 0, 0, 0, 0]).expect("minimum score"),
        Some(i64::MIN)
    );
    assert!(decode_score(&[0x20, 0]).is_err());
    assert!(decode_score(&[0x20]).is_err());
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
fn native_owner_binds_compiled_ranks_and_canonical_graph() -> Result<(), Box<dyn std::error::Error>>
{
    use quanta_index_contract::{
        RepoMapContainsEdge, RepoMapEdge, RepoMapEdgeKind,
        index::{Digest, ObjectRef},
        native_protocol::GraphProjectionRow,
    };
    use quanta_index_repomap::materializer::NativeRepoMapOwnerError;

    let compiler = RepoMapGraphCompiler::with_default_budget();
    let file_id = RepoMapNodeRef::File(FileId::new("src/lib.rs"));
    let symbol_id = RepoMapNodeRef::Symbol(SymbolId::new("src/lib.rs::Owner"));
    let input = |line_count| {
        bundle()
            .with_node(symbol("src/lib.rs::Owner", "src/lib.rs", "Owner"))
            .with_node(file("src/lib.rs", line_count))
            .with_edge(RepoMapEdge::Contains(RepoMapContainsEdge {
                container: file_id.clone(),
                contained: symbol_id.clone(),
            }))
    };
    let leaves = [
        GraphProjectionRow {
            subject: file_id.clone(),
            row: ObjectRef(Digest([1; 32])),
        },
        GraphProjectionRow {
            subject: symbol_id.clone(),
            row: ObjectRef(Digest([2; 32])),
        },
    ];
    let prepared = compiler.compile_native_owner(&input(40))?;
    let rows = prepared.rank_rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (&rows[0].subject, rows[0].final_score_millis),
        (&file_id, 281)
    );
    assert_eq!(
        (&rows[1].subject, rows[1].final_score_millis),
        (&symbol_id, 397)
    );
    let (output, commitments) = prepared.bind_rank_leaves(&leaves)?;
    assert_eq!(commitments.producer_authority, [0xbb; 32]);
    assert_eq!(
        output.nodes[0].payload_digest,
        Digest([
            0x23, 0x19, 0x1a, 0x75, 0xd8, 0x65, 0x8d, 0x19, 0xd1, 0xb6, 0xce, 0xb4, 0xf9, 0x92,
            0x64, 0x9e, 0x51, 0x38, 0x85, 0x7e, 0x08, 0xea, 0x39, 0xe9, 0xf5, 0x16, 0xce, 0x49,
            0x44, 0x36, 0xc4, 0x3f,
        ])
    );
    assert_eq!(output.projection, leaves);
    assert_eq!(output.edges.len(), 1);
    let edge = &output.edges[0];
    assert_eq!(
        (edge.kind, &edge.source, &edge.target),
        (RepoMapEdgeKind::Contains, &file_id, &symbol_id)
    );

    let mut permuted = input(40);
    permuted.nodes.reverse();
    let reordered = compiler
        .compile_native_owner(&permuted)?
        .bind_rank_leaves(&leaves)?;
    assert_eq!(reordered.0, output);
    assert_eq!(reordered.1, commitments);
    let (changed, _) = compiler
        .compile_native_owner(&input(48))?
        .bind_rank_leaves(&leaves)?;
    assert_ne!(
        changed.nodes[0].payload_digest,
        output.nodes[0].payload_digest
    );
    assert_eq!(changed.nodes[1], output.nodes[1]);
    assert!(matches!(
        compiler
            .compile_native_owner(&input(40))?
            .bind_rank_leaves(&[]),
        Err(NativeRepoMapOwnerError::RankLeafBinding)
    ));
    let mut invalid = leaves;
    invalid.swap(0, 1);
    assert!(matches!(
        compiler
            .compile_native_owner(&input(40))?
            .bind_rank_leaves(&invalid),
        Err(NativeRepoMapOwnerError::RankLeafBinding)
    ));
    Ok(())
}

#[test]
fn native_owner_refuses_graph_exceeding_encoded_byte_bound() {
    use quanta_index_contract::{RepoMapModuleId, RepoMapModuleNode};
    use quanta_index_repomap::materializer::NativeRepoMapOwnerError;

    let mut input = bundle().with_node(file("src/lib.rs", 40));
    let suffix = "a".repeat(2_000);
    input.nodes.extend((0..2_100).map(|index| {
        RepoMapNode::Module(RepoMapModuleNode {
            module_id: RepoMapModuleId::new(format!("module://{index:04}/{suffix}")),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            qualified_name: format!("module_{index:04}"),
        })
    }));
    assert!(matches!(
        RepoMapGraphCompiler::with_default_budget().compile_native_owner(&input),
        Err(NativeRepoMapOwnerError::GraphLimit)
    ));
}

#[test]
fn an_empty_bundle_is_refused() {
    assert_refusal(
        compile(&bundle()),
        Stage::BundleValidation,
        Code::EmptyBundle,
    );
}

#[test]
fn native_file_owners_accept_only_a_valid_truly_empty_successor() {
    use quanta_index_contract::{
        RepoMapContainsEdge, RepoMapEdge, native_protocol::NativeRepoMapOwnerError,
    };

    let compiler = RepoMapGraphCompiler::with_default_budget();
    assert!(
        compiler
            .compile_native_file_owners_v1(&bundle())
            .expect("the final Source file can be removed")
            .is_empty()
    );
    assert!(matches!(
        compiler.compile_native_owner(&bundle()),
        Err(NativeRepoMapOwnerError::Compile(cause)) if cause.code() == Code::EmptyBundle
    ));

    for invalid_manifest in [true, false] {
        let mut invalid = bundle();
        if invalid_manifest {
            invalid.manifest_digest = "not-a-digest".to_string();
        } else {
            invalid.authority_digest = "not-a-digest".to_string();
        }
        assert!(matches!(
            compiler.compile_native_file_owners_v1(&invalid),
            Err(NativeRepoMapOwnerError::Compile(cause))
                if cause.code() == Code::ProducerDigestInvalid
        ));
    }

    let phantom_edge = bundle().with_edge(RepoMapEdge::Contains(RepoMapContainsEdge {
        container: RepoMapNodeRef::File(FileId::new("src/removed.rs")),
        contained: RepoMapNodeRef::Symbol(SymbolId::new("src/removed.rs::gone")),
    }));
    assert!(matches!(
        compiler.compile_native_file_owners_v1(&phantom_edge),
        Err(NativeRepoMapOwnerError::Compile(cause)) if cause.code() == Code::EmptyBundle
    ));
}

#[test]
fn native_graph_item_bound_is_per_file_owner() {
    use quanta_index_contract::{
        RepoMapModuleId, RepoMapModuleNode,
        native_protocol::{MAX_NATIVE_GRAPH_ITEMS, NativeRepoMapOwnerError},
    };
    use std::collections::BTreeSet;

    let first = "src/a.rs";
    let second = "src/b.rs";
    let mut chunked = bundle()
        .with_node(file(first, 10))
        .with_node(file(second, 10));
    chunked
        .nodes
        .extend((0..MAX_NATIVE_GRAPH_ITEMS).map(|index| {
            let owner = if index.is_multiple_of(2) {
                first
            } else {
                second
            };
            RepoMapNode::Module(RepoMapModuleNode {
                module_id: RepoMapModuleId::new(format!("module://{index}")),
                repo_relative_path: RepoRelativePath::new(owner),
                qualified_name: format!("module_{index}"),
            })
        }));

    let compiler = RepoMapGraphCompiler::with_default_budget();
    assert!(matches!(
        compiler.compile_native_owner(&chunked),
        Err(NativeRepoMapOwnerError::GraphLimit)
    ));
    assert_eq!(
        compiler
            .compile_native_file_owners_v1(&chunked)
            .expect("two bounded native graph file owners")
            .len(),
        2
    );
    let selected = BTreeSet::from([RepoRelativePath::new(first)]);
    assert_eq!(
        compiler
            .compile_native_selected_file_owners_v1(&chunked, &selected)
            .expect("selected native graph file owner")
            .len(),
        1
    );

    for node in &mut chunked.nodes {
        if let RepoMapNode::Module(module) = node {
            module.repo_relative_path = RepoRelativePath::new(first);
        }
    }
    assert!(matches!(
        compiler.compile_native_file_owners_v1(&chunked),
        Err(NativeRepoMapOwnerError::GraphLimit)
    ));
}

#[test]
fn native_file_fact_child_keeps_the_existing_child_bound() {
    use quanta_index_contract::native_protocol::NativeRepoMapOwnerError;

    let input = (0..4095).fold(bundle().with_node(file("src/a.rs", 1)), |source, index| {
        source.with_node(symbol(
            &format!("src/a.rs::S{index:04}"),
            "src/a.rs",
            &format!("S{index:04}"),
        ))
    });
    assert!(matches!(
        RepoMapGraphCompiler::with_default_budget().compile_native_file_owners_v1(&input),
        Err(NativeRepoMapOwnerError::GraphLimit)
    ));
}

#[test]
fn an_invalid_producer_digest_is_refused() {
    let source = bundle().with_node(file("src/lib.rs", 10));
    let mut bad = source;
    bad.manifest_digest = "not-a-digest".to_string();
    assert_refusal(
        compile(&bad),
        Stage::BundleValidation,
        Code::ProducerDigestInvalid,
    );
}

#[test]
fn duplicate_typed_nodes_are_refused() {
    let source = bundle()
        .with_node(file("src/lib.rs", 10))
        .with_node(file("src/lib.rs", 20));
    assert_refusal(
        compile(&source),
        Stage::GraphValidation,
        Code::DuplicateNode,
    );
}

#[test]
fn cross_variant_identity_collision_is_refused() {
    let source = bundle().with_node(file("shared-id", 10)).with_node(symbol(
        "shared-id",
        "src/lib.rs",
        "Owner",
    ));
    assert_refusal(
        compile(&source),
        Stage::GraphValidation,
        Code::CrossVariantIdentityCollision,
    );
}

#[test]
fn dangling_edge_endpoints_are_refused() {
    let source = bundle().with_node(file("src/lib.rs", 10)).with_edge(
        quanta_index_contract::RepoMapEdge::Call(quanta_index_contract::RepoMapCallEdge {
            caller: RepoMapNodeRef::Symbol(SymbolId::new("ghost")),
            callee: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
        }),
    );
    assert_refusal(
        compile(&source),
        Stage::GraphValidation,
        Code::DanglingEdgeEndpoint,
    );
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
    assert_refusal(
        compile(&source),
        Stage::GraphValidation,
        Code::IllegalEdgeVariant,
    );
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
fn preview_cap_counts_all_projection_rows() {
    use quanta_index_repomap::materializer::NativeRepoMapOwnerError;

    let source = bundle().with_node(file("a", 10)).with_node(file("b", 10));
    let no_rank_outputs = std::collections::BTreeSet::new();
    // Each row has ten bytes ("0:a a file" and "0:b b file").
    let mut budget = RepoMapCompilerBudgetV1::default_ceiling();
    budget.max_preview_bytes = 20;
    let candidate = RepoMapGraphCompiler::new(budget)
        .compile(&source)
        .expect("exact combined preview budget");
    assert_eq!(candidate.resource_receipt().preview_bytes, 20);
    assert!(
        RepoMapGraphCompiler::new(budget)
            .compile_native_selected_file_owners_v1(&source, &no_rank_outputs)
            .expect("empty selection still has exact preview budget")
            .is_empty()
    );

    budget.max_preview_bytes = 19;
    let refusal = RepoMapGraphCompiler::new(budget)
        .compile(&source)
        .expect_err("the combined preview budget must refuse");
    assert_eq!(refusal.code(), Code::PreviewLimitExceeded);
    assert_eq!(refusal.limit(), Some(19));
    assert_eq!(refusal.observed(), 20);
    assert!(matches!(
        RepoMapGraphCompiler::new(budget)
            .compile_native_selected_file_owners_v1(&source, &no_rank_outputs),
        Err(NativeRepoMapOwnerError::Compile(native)) if native == refusal
    ));
}

#[test]
fn native_count_only_payload_keeps_exact_materialized_bound()
-> Result<(), Box<dyn std::error::Error>> {
    use quanta_index_repomap::materializer::NativeRepoMapOwnerError;

    let source = bundle().with_node(file("src/a.rs", 10));
    let full = RepoMapGraphCompiler::with_default_budget().compile(&source)?;
    let exact = u64::try_from(full.compiled_payload().len())?;
    let no_rank_outputs = std::collections::BTreeSet::new();
    assert_eq!(full.resource_receipt().materialized_bytes, exact);
    let mut budget = RepoMapCompilerBudgetV1::default_ceiling();
    budget.max_materialized_bytes = exact;
    assert_eq!(
        RepoMapGraphCompiler::new(budget)
            .compile_native_file_owners_v1(&source)?
            .len(),
        1
    );
    assert!(
        RepoMapGraphCompiler::new(budget)
            .compile_native_selected_file_owners_v1(&source, &no_rank_outputs)?
            .is_empty()
    );

    assert!(exact > 0);
    budget.max_materialized_bytes = exact.saturating_sub(1);
    let compiler = RepoMapGraphCompiler::new(budget);
    let legacy = compiler
        .compile(&source)
        .expect_err("legacy byte cap must refuse");
    assert_eq!(legacy.code(), Code::MaterializedByteLimitExceeded);
    assert_eq!(legacy.observed(), exact);
    assert!(matches!(
        compiler.compile_native_file_owners_v1(&source),
        Err(NativeRepoMapOwnerError::Compile(refusal)) if refusal == legacy
    ));
    assert!(matches!(
        compiler.compile_native_selected_file_owners_v1(&source, &no_rank_outputs),
        Err(NativeRepoMapOwnerError::Compile(refusal)) if refusal == legacy
    ));
    let long_path =
        "x".repeat(quanta_index_contract::native_protocol::MAX_REPOMAP_RANK_PATH_BYTES + 1);
    let invalid_native = bundle().with_node(RepoMapNode::File(RepoMapFileNode {
        file_id: FileId::new("a"),
        repo_relative_path: RepoRelativePath::new(long_path),
        line_count: 1,
    }));
    assert!(matches!(
        RepoMapGraphCompiler::with_default_budget()
            .compile_native_selected_file_owners_v1(&invalid_native, &no_rank_outputs),
        Err(NativeRepoMapOwnerError::InvalidRankRow)
    ));
    Ok(())
}

#[test]
fn native_selected_byte_count_matches_cbor_text_width_boundaries()
-> Result<(), Box<dyn std::error::Error>> {
    use quanta_index_repomap::materializer::NativeRepoMapOwnerError;

    let none = std::collections::BTreeSet::new();
    // Identity, owner path, and rank search text cross independent CBOR
    // 1/2/3/5-byte text-header boundaries at these input lengths.
    for length in [7, 8, 23, 24, 123, 124, 255, 256, 32_763, 32_764] {
        let path = "p".repeat(length);
        let symbol_id = format!("{path}::A");
        let source = bundle()
            .with_node(file(&path, u32::MAX))
            .with_node(symbol(&symbol_id, &path, "A"));
        let exact = u64::try_from(
            RepoMapGraphCompiler::with_default_budget()
                .compile(&source)?
                .compiled_payload()
                .len(),
        )?;
        let mut budget = RepoMapCompilerBudgetV1::default_ceiling();
        budget.max_materialized_bytes = exact - 1;
        let compiler = RepoMapGraphCompiler::new(budget);
        let legacy = compiler
            .compile(&source)
            .expect_err("legacy exact byte cap");
        assert!(
            matches!(
                compiler.compile_native_selected_file_owners_v1(&source, &none),
                Err(NativeRepoMapOwnerError::Compile(native)) if native == legacy
            ),
            "CBOR width mismatch at input length {length}"
        );
    }
    Ok(())
}

#[test]
fn owner_symbol_cap_counts_distinct_owners() {
    let source = bundle()
        .with_node(symbol("a::A", "a", "A"))
        .with_node(symbol("b::B", "b", "B"));
    let mut budget = RepoMapCompilerBudgetV1::default_ceiling();
    budget.max_owner_symbols = 1;
    let refusal = RepoMapGraphCompiler::new(budget)
        .compile(&source)
        .expect_err("the combined symbol budget must refuse");
    assert_eq!(refusal.stage(), Stage::Projection);
    assert_eq!(refusal.code(), Code::OwnerSymbolLimitExceeded);
    assert_eq!(refusal.limit(), Some(1));
    assert_eq!(refusal.observed(), 2);
}

#[test]
fn zero_ceiling_budgets_are_refused_at_construction() {
    assert!(RepoMapCompilerBudgetV1::new(0, 1, 1, 1, 1, 1).is_err());
}

#[test]
fn commitment_is_independent_of_input_order() {
    use quanta_index_contract::{RepoMapEdge, RepoMapOwnsChunkEdge};

    let node_a = file("src/lib.rs", 100);
    let node_b = symbol("src/lib.rs::Owner", "src/lib.rs", "Owner");
    let node_c = chunk("chunk://a", "src/lib.rs", 42);
    let node_d = chunk("chunk://b", "src/lib.rs", 15);
    let edge =
        quanta_index_contract::RepoMapEdge::Contains(quanta_index_contract::RepoMapContainsEdge {
            container: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
            contained: RepoMapNodeRef::Symbol(SymbolId::new("src/lib.rs::Owner")),
        });
    let owns_a = RepoMapEdge::OwnsChunk(RepoMapOwnsChunkEdge {
        owner: RepoMapNodeRef::Symbol(SymbolId::new("src/lib.rs::Owner")),
        chunk: RepoMapNodeRef::Chunk(ChunkId::new("chunk://a")),
    });
    let owns_b = RepoMapEdge::OwnsChunk(RepoMapOwnsChunkEdge {
        owner: RepoMapNodeRef::Symbol(SymbolId::new("src/lib.rs::Owner")),
        chunk: RepoMapNodeRef::Chunk(ChunkId::new("chunk://b")),
    });
    let forward = bundle()
        .with_node(node_a.clone())
        .with_node(node_b.clone())
        .with_node(node_c.clone())
        .with_node(node_d.clone())
        .with_edge(edge.clone())
        .with_edge(owns_a.clone())
        .with_edge(owns_b.clone());
    let reverse = bundle()
        .with_node(node_d)
        .with_node(node_c)
        .with_node(node_b)
        .with_node(node_a)
        .with_edge(owns_b)
        .with_edge(owns_a)
        .with_edge(edge);
    let forward_candidate = compile(&forward).expect("forward bundle compiles");
    let reverse_candidate = compile(&reverse).expect("reverse bundle compiles");
    assert_eq!(
        forward_candidate.commitments(),
        reverse_candidate.commitments()
    );
    assert_eq!(
        forward_candidate.compiled_payload(),
        reverse_candidate.compiled_payload()
    );
    let duplicate_edge = forward
        .edges
        .first()
        .expect("the fixture has edges")
        .clone();
    assert_refusal(
        compile(&forward.with_edge(duplicate_edge)),
        Stage::GraphValidation,
        Code::DuplicateNode,
    );
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
    assert_eq!(
        hex,
        "7820937c0e7474a7fc412239557df9ee7f468c6aaebf9bd5db6cd53c4437113a"
    );
}

#[test]
fn multilingual_bundles_compile_through_the_shared_tokenizer() {
    let source = bundle()
        .with_node(file("src/한글모듈.rs", 10))
        .with_node(symbol("src/한글모듈.rs::함수", "src/한글모듈.rs", "함수"));
    let candidate = compile(&source).expect("multilingual fixture compiles");
    assert_eq!(candidate.projection().len(), 2);
}

#[test]
fn native_file_owners_preserve_global_ranks_and_cross_file_edges()
-> Result<(), Box<dyn std::error::Error>> {
    use quanta_index_contract::{
        RepoMapCallEdge, RepoMapContainsEdge, RepoMapDependsOnEdge, RepoMapEdge, RepoMapEdgeKind,
        RepoMapImportEdge,
        index::{Digest, ObjectRef},
        native_protocol::{GraphOwnerOutput, GraphProjectionRow},
    };

    let a_file = RepoMapNodeRef::File(FileId::new("src/a.rs"));
    let b_file = RepoMapNodeRef::File(FileId::new("src/b.rs"));
    let a_symbol = RepoMapNodeRef::Symbol(SymbolId::new("src/a.rs::A"));
    let b_symbol = RepoMapNodeRef::Symbol(SymbolId::new("src/b.rs::B"));
    let input = |extra_b: bool| {
        let mut source = bundle()
            .with_node(file("src/a.rs", 20))
            .with_node(file("src/b.rs", 20))
            .with_node(symbol("src/a.rs::A", "src/a.rs", "A"))
            .with_node(symbol("src/b.rs::B", "src/b.rs", "B"))
            .with_edge(RepoMapEdge::Contains(RepoMapContainsEdge {
                container: a_file.clone(),
                contained: a_symbol.clone(),
            }))
            .with_edge(RepoMapEdge::Contains(RepoMapContainsEdge {
                container: b_file.clone(),
                contained: b_symbol.clone(),
            }))
            .with_edge(RepoMapEdge::Call(RepoMapCallEdge {
                caller: a_symbol.clone(),
                callee: b_symbol.clone(),
            }))
            .with_edge(RepoMapEdge::Import(RepoMapImportEdge {
                importer: a_file.clone(),
                imported: b_file.clone(),
            }))
            .with_edge(RepoMapEdge::DependsOn(RepoMapDependsOnEdge {
                dependent: b_symbol.clone(),
                dependency: a_symbol.clone(),
            }));
        if extra_b {
            let extra = RepoMapNodeRef::Symbol(SymbolId::new("src/b.rs::Extra"));
            source = source
                .with_node(symbol("src/b.rs::Extra", "src/b.rs", "Extra"))
                .with_edge(RepoMapEdge::Contains(RepoMapContainsEdge {
                    container: b_file.clone(),
                    contained: extra,
                }));
        }
        source
    };
    let compiler = RepoMapGraphCompiler::with_default_budget();
    let original = input(false);
    let global = compiler.compile_native_owner(&original)?;
    let global_rows = global.rank_rows().to_vec();
    let mut owners = compiler.compile_native_file_owners_v1(&original)?;
    assert_eq!(owners.len(), 2);
    assert_eq!(owners[0].owner_path().as_str(), "src/a.rs");
    assert_eq!(owners[1].owner_path().as_str(), "src/b.rs");
    let a_stamp = owners[0].output_stamp_v1()?;
    let b_stamp = owners[1].output_stamp_v1()?;
    let mut all_rows = owners
        .iter()
        .flat_map(|owner| owner.rank_rows().iter().cloned())
        .collect::<Vec<_>>();
    all_rows.sort_unstable_by(|left, right| left.subject.cmp(&right.subject));
    assert_eq!(all_rows, global_rows);
    let bind = |owner: quanta_index_repomap::materializer::NativeRepoMapFileOwnerProjectionV1| -> Result<
        (
            GraphOwnerOutput,
            quanta_index_contract::RepoMapCandidateCommitmentsV1,
        ),
        Box<dyn std::error::Error>,
    > {
        let owner_path = owner.owner_path().as_str();
        assert_eq!(owner.file_facts().owner_path.as_str(), owner_path);
        let facts = owner.file_facts().clone();
        let facts_ref = ObjectRef(Digest([if owner_path == "src/a.rs" { 9 } else { 10 }; 32]));
        let leaves = owner
            .rank_rows()
            .iter()
            .enumerate()
            .map(|(index, row)| {
                Ok(GraphProjectionRow {
                    subject: row.subject.clone(),
                    row: ObjectRef(Digest([u8::try_from(index + 1)?; 32])),
                })
            })
            .collect::<Result<Vec<_>, std::num::TryFromIntError>>()?;
        let (output, commitments) = owner.bind_rank_leaves(&leaves, facts_ref)?;
        assert_eq!(output.facts, Some(facts_ref));
        assert!(facts.matches_graph_nodes_v1(&output.nodes)?);
        Ok((output, commitments))
    };
    let (a_output, a_commitments) = bind(owners.remove(0))?;
    let (b_output, b_commitments) = bind(owners.remove(0))?;
    let only_a = std::collections::BTreeSet::from([RepoRelativePath::new("src/a.rs")]);
    let mut selected = compiler.compile_native_selected_file_owners_v1(&original, &only_a)?;
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].owner_path().as_str(), "src/a.rs");
    assert_eq!(selected[0].output_stamp_v1()?, a_stamp);
    assert_eq!(bind(selected.remove(0))?, (a_output.clone(), a_commitments));
    let mut permuted = original.clone();
    permuted.nodes.reverse();
    permuted.edges.reverse();
    let mut selected_permuted =
        compiler.compile_native_selected_file_owners_v1(&permuted, &only_a)?;
    assert_eq!(selected_permuted.len(), 1);
    assert_eq!(
        bind(selected_permuted.remove(0))?,
        (a_output.clone(), a_commitments)
    );

    // An omitted owner's outgoing edge still has to pass complete-graph validation.
    let invalid_omitted_owner =
        original
            .clone()
            .with_edge(RepoMapEdge::DependsOn(RepoMapDependsOnEdge {
                dependent: b_symbol.clone(),
                dependency: RepoMapNodeRef::Symbol(SymbolId::new("src/b.rs::absent")),
            }));
    assert!(matches!(
        compiler.compile_native_selected_file_owners_v1(&invalid_omitted_owner, &only_a),
        Err(quanta_index_repomap::materializer::NativeRepoMapOwnerError::Compile(refusal))
            if refusal.code() == Code::DanglingEdgeEndpoint
    ));
    assert!(
        compiler
            .compile_native_selected_file_owners_v1(&original, &std::collections::BTreeSet::new())?
            .is_empty()
    );
    let mut invalid = original.clone();
    invalid.edges.push(original.edges[0].clone());
    assert!(
        compiler
            .compile_native_selected_file_owners_v1(&invalid, &std::collections::BTreeSet::new())
            .is_err(),
        "even an empty selection must validate the whole bundle"
    );
    assert_eq!(a_commitments, b_commitments);
    assert!(
        a_output
            .edges
            .iter()
            .any(|edge| edge.kind == RepoMapEdgeKind::Call
                && edge.source == a_symbol
                && edge.target == b_symbol)
    );
    assert!(
        a_output
            .edges
            .iter()
            .any(|edge| edge.kind == RepoMapEdgeKind::Import
                && edge.source == a_file
                && edge.target == b_file)
    );
    assert!(!a_output.nodes.iter().any(|node| node.identity == b_symbol));
    assert!(
        b_output
            .edges
            .iter()
            .any(|edge| edge.kind == RepoMapEdgeKind::DependsOn
                && edge.source == b_symbol
                && edge.target == a_symbol)
    );

    let mut permuted = original.clone();
    permuted.nodes.reverse();
    permuted.edges.reverse();
    let mut reordered = compiler.compile_native_file_owners_v1(&permuted)?;
    assert_eq!(reordered[0].output_stamp_v1()?, a_stamp);
    assert_eq!(reordered[1].output_stamp_v1()?, b_stamp);
    assert_eq!(bind(reordered.remove(0))?.0, a_output);
    assert_eq!(bind(reordered.remove(0))?.0, b_output);

    let mut updated = compiler.compile_native_file_owners_v1(&input(true))?;
    assert_eq!(updated[0].output_stamp_v1()?, a_stamp);
    assert_ne!(updated[1].output_stamp_v1()?, b_stamp);
    let (a_after, updated_commitments) = bind(updated.remove(0))?;
    let (b_after, _) = bind(updated.remove(0))?;
    assert_eq!(
        a_output, a_after,
        "unaffected file output remains content-addressable"
    );
    assert_ne!(b_output, b_after);
    assert_ne!(
        a_commitments.compiled_graph,
        updated_commitments.compiled_graph
    );
    Ok(())
}

#[test]
fn cross_file_chunk_changes_other_files_rank_inputs() -> Result<(), Box<dyn std::error::Error>> {
    use quanta_index_contract::{RepoMapEdge, RepoMapOwnsChunkEdge};

    let input = |tokens| {
        bundle()
            .with_node(file("src/a.rs", 20))
            .with_node(symbol("src/a.rs::A", "src/a.rs", "A"))
            .with_node(file("src/b.rs", 20))
            .with_node(chunk("src/b.rs::chunk", "src/b.rs", tokens))
            .with_edge(RepoMapEdge::OwnsChunk(RepoMapOwnsChunkEdge {
                owner: RepoMapNodeRef::Symbol(SymbolId::new("src/a.rs::A")),
                chunk: RepoMapNodeRef::Chunk(ChunkId::new("src/b.rs::chunk")),
            }))
    };
    let compiler = RepoMapGraphCompiler::with_default_budget();
    let before = compiler.compile_native_file_owners_v1(&input(10))?;
    let after = compiler.compile_native_file_owners_v1(&input(20))?;
    assert_eq!(before[0].owner_path().as_str(), "src/a.rs");
    assert_eq!(before[0].file_facts(), after[0].file_facts());
    assert_ne!(before[0].rank_rows(), after[0].rank_rows());
    assert_ne!(before[0].output_stamp_v1()?, after[0].output_stamp_v1()?);
    Ok(())
}

#[test]
fn incoming_cross_file_edge_changes_ranked_owner_stamp() -> Result<(), Box<dyn std::error::Error>> {
    use quanta_index_contract::{RepoMapEdge, RepoMapImportEdge};

    let a = RepoMapNodeRef::File(FileId::new("src/a.rs"));
    let b = RepoMapNodeRef::File(FileId::new("src/b.rs"));
    let source = bundle()
        .with_node(file("src/a.rs", 20))
        .with_node(file("src/b.rs", 20));
    let compiler = RepoMapGraphCompiler::with_default_budget();
    let before = compiler.compile_native_file_owners_v1(&source)?;
    let after = compiler.compile_native_file_owners_v1(&source.with_edge(RepoMapEdge::Import(
        RepoMapImportEdge {
            importer: b,
            imported: a,
        },
    )))?;
    assert_eq!(before[0].file_facts(), after[0].file_facts());
    assert_ne!(before[0].rank_rows(), after[0].rank_rows());
    assert_ne!(before[0].output_stamp_v1()?, after[0].output_stamp_v1()?);
    Ok(())
}

#[test]
fn local_fact_change_with_same_rank_changes_owner_stamp() -> Result<(), Box<dyn std::error::Error>>
{
    let compiler = RepoMapGraphCompiler::with_default_budget();
    let input = |lines| {
        bundle()
            .with_node(file("src/a.rs", lines))
            .with_node(file("src/b.rs", 20))
    };
    let before = compiler.compile_native_file_owners_v1(&input(40))?;
    let after = compiler.compile_native_file_owners_v1(&input(41))?;
    assert_eq!(before[0].rank_rows(), after[0].rank_rows());
    assert_ne!(before[0].output_stamp_v1()?, after[0].output_stamp_v1()?);
    assert_eq!(before[1].output_stamp_v1()?, after[1].output_stamp_v1()?);
    Ok(())
}

#[test]
fn native_rank_input_stamps_cover_remote_chunks_incidents_and_permutation()
-> Result<(), Box<dyn std::error::Error>> {
    use quanta_index_contract::{
        RepoMapContainsEdge, RepoMapEdge, RepoMapImportEdge, RepoMapOwnsChunkEdge,
    };

    let a_path = RepoRelativePath::new("src/a.rs");
    let b_path = RepoRelativePath::new("src/b.rs");
    let c_path = RepoRelativePath::new("src/c.rs");
    let a_file = RepoMapNodeRef::File(FileId::new("src/a.rs"));
    let a_symbol = RepoMapNodeRef::Symbol(SymbolId::new("src/a.rs::A"));
    let b_file = RepoMapNodeRef::File(FileId::new("src/b.rs"));
    let b_chunk = RepoMapNodeRef::Chunk(ChunkId::new("src/b.rs::chunk"));
    let original = bundle()
        .with_node(file("src/a.rs", 20))
        .with_node(symbol("src/a.rs::A", "src/a.rs", "A"))
        .with_node(file("src/b.rs", 20))
        .with_node(chunk("src/b.rs::chunk", "src/b.rs", 10))
        .with_node(file("src/c.rs", 20))
        .with_edge(RepoMapEdge::Contains(RepoMapContainsEdge {
            container: a_file.clone(),
            contained: a_symbol.clone(),
        }))
        .with_edge(RepoMapEdge::OwnsChunk(RepoMapOwnsChunkEdge {
            owner: a_symbol,
            chunk: b_chunk,
        }));
    let compiler = RepoMapGraphCompiler::with_default_budget();
    let before = compiler.native_file_rank_input_stamps_v1(&original)?;
    assert_eq!(before.len(), 3);

    let mut permuted = original.clone();
    permuted.nodes.reverse();
    permuted.edges.reverse();
    assert_eq!(
        compiler.native_file_rank_input_stamps_v1(&permuted)?,
        before
    );

    let mut remote_changed = original.clone();
    for node in &mut remote_changed.nodes {
        if let RepoMapNode::Chunk(chunk) = node {
            chunk.token_count = 20;
            chunk.end_byte = 20;
        }
    }
    let changed = compiler.native_file_rank_input_stamps_v1(&remote_changed)?;
    assert_ne!(changed[&a_path], before[&a_path]);
    assert_ne!(changed[&b_path], before[&b_path]);
    assert_eq!(changed[&c_path], before[&c_path]);

    let mut edge_removed = original.clone();
    assert!(edge_removed.edges.pop().is_some());
    let removed = compiler.native_file_rank_input_stamps_v1(&edge_removed)?;
    assert_ne!(removed[&a_path], before[&a_path]);
    assert_ne!(removed[&b_path], before[&b_path]);
    assert_eq!(removed[&c_path], before[&c_path]);

    let incoming = original.with_edge(RepoMapEdge::Import(RepoMapImportEdge {
        importer: b_file,
        imported: a_file,
    }));
    let incident = compiler.native_file_rank_input_stamps_v1(&incoming)?;
    assert_ne!(incident[&a_path], before[&a_path]);
    assert_ne!(incident[&b_path], before[&b_path]);
    assert_eq!(incident[&c_path], before[&c_path]);
    Ok(())
}

#[test]
fn native_rank_input_stamps_refuse_missing_or_duplicate_owners() {
    use quanta_index_contract::{RepoMapEdge, RepoMapImportEdge};
    use quanta_index_repomap::materializer::NativeRepoMapOwnerError;

    let compiler = RepoMapGraphCompiler::with_default_budget();
    assert!(
        compiler
            .native_file_rank_input_stamps_v1(&bundle())
            .expect("valid empty successor")
            .is_empty()
    );
    let duplicate = bundle()
        .with_node(file("src/a.rs", 20))
        .with_node(file("src/a.rs", 21));
    assert!(matches!(
        compiler.native_file_rank_input_stamps_v1(&duplicate),
        Err(NativeRepoMapOwnerError::OwnerPartition)
    ));
    let dangling = bundle()
        .with_node(file("src/a.rs", 20))
        .with_edge(RepoMapEdge::Import(RepoMapImportEdge {
            importer: RepoMapNodeRef::File(FileId::new("src/a.rs")),
            imported: RepoMapNodeRef::File(FileId::new("src/missing.rs")),
        }));
    assert!(matches!(
        compiler.native_file_rank_input_stamps_v1(&dangling),
        Err(NativeRepoMapOwnerError::OwnerPartition)
    ));
    let edge = RepoMapEdge::Import(RepoMapImportEdge {
        importer: RepoMapNodeRef::File(FileId::new("src/a.rs")),
        imported: RepoMapNodeRef::File(FileId::new("src/b.rs")),
    });
    let duplicate_edge = bundle()
        .with_node(file("src/a.rs", 20))
        .with_node(file("src/b.rs", 20))
        .with_edge(edge.clone())
        .with_edge(edge);
    assert!(matches!(
        compiler.native_file_rank_input_stamps_v1(&duplicate_edge),
        Err(NativeRepoMapOwnerError::OwnerPartition)
    ));
}
