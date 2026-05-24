//! Roundtrip tests for every manually serialized contract DTO.
//!
//! Each fixture exercises every variant/field combination and is roundtripped
//! through both JSON (control-plane `SQLite` persistence) and CBOR (UDS wire
//! transport). Tests use `Result<(), Box<dyn Error>>` so the `?` operator
//! surfaces encode/decode failures without panicking.

#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

use std::error::Error;

use quanta_index_contract::{
    BuildContextOutput, BundleArtifactRef, BundleEncoding, BundleMode, BundleNotify, CallEdge,
    FileId, FileMaterializationPacket, GenerationId, HirOutput, ImportEdge, ItemIndexOutput,
    LexicalCandidate, LqDirective, LqDirectiveSet, LqExpr, LqFilter, LqFilterSet, LqOptionSet,
    LqQuery, ManifestDigest, ManifestGeneration, ParseOutput, PreparedBundleOutbox,
    PublishedGenerationSet, PublishedSearchBundleDeltaApplyRequest,
    PublishedSearchBundleDeltaApplyResponse, PublishedSearchBundleInspectRequest,
    PublishedSearchBundleInspectResponse, PublishedSearchBundleManifest,
    PublishedSearchBundlePrepareRequest, PublishedSearchBundlePrepareResponse,
    PublishedSearchGenerationActivateRequest, PublishedSearchGenerationActivateResponse,
    PublishedSearchGenerationReadinessResponse, RepoId, RepoRelativePath, RevisionId,
    SearchBundleMutationDelta, SearchBundleMutationOp, SearchExplanation,
    SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse, SearchPlaneHybridQueryRequest,
    SearchPlaneHybridQueryResponse, SearchPlaneIpcError, SearchPlaneIpcRequest,
    SearchPlaneIpcRequestEnvelope, SearchPlaneIpcResponse, SearchPlaneIpcResponseEnvelope,
    SearchPlaneLexicalQueryRequest, SearchPlaneLexicalQueryResponse,
    SearchPlaneSemanticQueryRequest, SearchPlaneSemanticQueryResponse,
};
use serde::{Serialize, de::DeserializeOwned};

type DynErr = Box<dyn Error>;

fn roundtrip_json<T>(value: &T) -> Result<(), DynErr>
where
    T: Serialize + DeserializeOwned + PartialEq + core::fmt::Debug,
{
    let encoded = serde_json::to_vec(value)?;
    let decoded: T = serde_json::from_slice(&encoded)?;
    if &decoded != value {
        return Err(format!("JSON roundtrip mismatch for {value:?} -> {decoded:?}").into());
    }
    Ok(())
}

fn roundtrip_cbor<T>(value: &T) -> Result<(), DynErr>
where
    T: Serialize + DeserializeOwned + PartialEq + core::fmt::Debug,
{
    let mut buffer: Vec<u8> = Vec::new();
    ciborium::into_writer(value, &mut buffer)?;
    let decoded: T = ciborium::from_reader(buffer.as_slice())?;
    if &decoded != value {
        return Err(format!("CBOR roundtrip mismatch for {value:?} -> {decoded:?}").into());
    }
    Ok(())
}

fn roundtrip<T>(value: &T) -> Result<(), DynErr>
where
    T: Serialize + DeserializeOwned + PartialEq + core::fmt::Debug,
{
    roundtrip_json(value)?;
    roundtrip_cbor(value)?;
    Ok(())
}

fn sample_manifest_digest() -> ManifestDigest {
    ManifestDigest::new("sha256:dead-beef")
}

fn sample_repo_id() -> RepoId {
    RepoId::new("repo-alpha")
}

fn sample_revision_id() -> RevisionId {
    RevisionId::new("rev-001")
}

fn sample_artifact_ref(kind: &str) -> BundleArtifactRef {
    BundleArtifactRef {
        relative_path: format!("bundle/{kind}.feather"),
        encoding: BundleEncoding::Feather,
        byte_length: 4096,
        content_digest: ManifestDigest::new(format!("sha256:{kind}")),
    }
}

fn sample_published_generation_set() -> PublishedGenerationSet {
    PublishedGenerationSet {
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        lexical_generation: GenerationId::new(10),
        symbol_generation: GenerationId::new(11),
        structural_generation: Some(GenerationId::new(12)),
        history_generation: None,
        semantic_generation: Some(GenerationId::new(13)),
        metadata_generation: None,
    }
}

fn sample_published_manifest() -> PublishedSearchBundleManifest {
    PublishedSearchBundleManifest {
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        bundle_schema_version: 1,
        lexical_chunk_rows: sample_artifact_ref("lexical_chunk_rows"),
        symbol_rows: sample_artifact_ref("symbol_rows"),
        metadata_rows: Some(sample_artifact_ref("metadata_rows")),
        graph_rows: None,
        embedding_input_views: Some(sample_artifact_ref("embedding_input_views")),
        embedding_records: None,
        mutation_delta: Some(sample_artifact_ref("mutation_delta")),
    }
}

fn sample_prepared_outbox() -> PreparedBundleOutbox {
    PreparedBundleOutbox {
        outbox_id: "outbox-7".to_owned(),
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        manifest_digest: sample_manifest_digest(),
        bundle_schema_version: 1,
        prepared_at_ms: 1_700_000_000_000,
        mode: BundleMode::ServeOnly,
        manifest_ref: sample_artifact_ref("manifest"),
        base_generation: Some(ManifestGeneration::new(6)),
        changed_artifact_mask: 0xFF,
    }
}

fn sample_lq_query() -> LqQuery {
    LqQuery {
        expr: LqExpr::All(vec![
            LqExpr::MatchAll,
            LqExpr::Raw("foo".to_owned()),
            LqExpr::Any(vec![
                LqExpr::Raw("bar".to_owned()),
                LqExpr::Not(Box::new(LqExpr::Raw("baz".to_owned()))),
            ]),
        ]),
        filters: LqFilterSet {
            filters: vec![
                LqFilter::Repo("repo-alpha".to_owned()),
                LqFilter::Custom {
                    key: "k".to_owned(),
                    value: "v".to_owned(),
                },
            ],
        },
        options: LqOptionSet {
            limit: Some(100),
            count_all: true,
            timeout_ms: Some(5_000),
        },
        directives: LqDirectiveSet {
            directives: vec![
                LqDirective::IntoCodeQl,
                LqDirective::ScopeResults,
                LqDirective::WithLexical,
                LqDirective::Custom("trace".to_owned()),
            ],
        },
    }
}

fn sample_lexical_candidate() -> LexicalCandidate {
    LexicalCandidate {
        candidate_id: "cand-1".to_owned(),
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        start_line: 10,
        end_line: 20,
        score: 0.875,
        snippet: "fn main() {}".to_owned(),
    }
}

#[test]
fn repo_id_roundtrip() -> Result<(), DynErr> {
    roundtrip(&sample_repo_id())
}

#[test]
fn revision_id_roundtrip() -> Result<(), DynErr> {
    roundtrip(&sample_revision_id())
}

#[test]
fn manifest_generation_roundtrip() -> Result<(), DynErr> {
    roundtrip(&ManifestGeneration::new(42))
}

#[test]
fn generation_id_roundtrip() -> Result<(), DynErr> {
    roundtrip(&GenerationId::new(99))
}

#[test]
fn manifest_digest_roundtrip() -> Result<(), DynErr> {
    roundtrip(&sample_manifest_digest())
}

#[test]
fn file_id_roundtrip() -> Result<(), DynErr> {
    roundtrip(&FileId::new("file-xyz"))
}

#[test]
fn repo_relative_path_roundtrip() -> Result<(), DynErr> {
    roundtrip(&RepoRelativePath::new("src/main.rs"))
}

#[test]
fn bundle_mode_variants_roundtrip() -> Result<(), DynErr> {
    roundtrip(&BundleMode::ServeOnly)?;
    roundtrip(&BundleMode::IndexBuild)
}

#[test]
fn bundle_encoding_variants_roundtrip() -> Result<(), DynErr> {
    roundtrip(&BundleEncoding::ArrowIpc)?;
    roundtrip(&BundleEncoding::Feather)?;
    roundtrip(&BundleEncoding::Json)?;
    roundtrip(&BundleEncoding::RawF32)?;
    roundtrip(&BundleEncoding::TantivyDirectory)?;
    roundtrip(&BundleEncoding::LanceDirectory)?;
    roundtrip(&BundleEncoding::Opaque)
}

#[test]
fn bundle_artifact_ref_roundtrip() -> Result<(), DynErr> {
    roundtrip(&sample_artifact_ref("foo"))
}

#[test]
fn prepared_bundle_outbox_roundtrip() -> Result<(), DynErr> {
    roundtrip(&sample_prepared_outbox())?;
    // also exercise the None branch on Option<ManifestGeneration>.
    let mut without_base = sample_prepared_outbox();
    without_base.base_generation = None;
    roundtrip(&without_base)
}

#[test]
fn published_generation_set_roundtrip() -> Result<(), DynErr> {
    roundtrip(&sample_published_generation_set())?;
    let fully_populated = PublishedGenerationSet {
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        manifest_generation: ManifestGeneration::new(1),
        lexical_generation: GenerationId::new(2),
        symbol_generation: GenerationId::new(3),
        structural_generation: Some(GenerationId::new(4)),
        history_generation: Some(GenerationId::new(5)),
        semantic_generation: Some(GenerationId::new(6)),
        metadata_generation: Some(GenerationId::new(7)),
    };
    roundtrip(&fully_populated)
}

#[test]
fn published_search_bundle_manifest_roundtrip() -> Result<(), DynErr> {
    roundtrip(&sample_published_manifest())?;
    let bare = PublishedSearchBundleManifest {
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        manifest_generation: ManifestGeneration::new(1),
        bundle_schema_version: 1,
        lexical_chunk_rows: sample_artifact_ref("lex"),
        symbol_rows: sample_artifact_ref("sym"),
        metadata_rows: None,
        graph_rows: None,
        embedding_input_views: None,
        embedding_records: None,
        mutation_delta: None,
    };
    roundtrip(&bare)
}

#[test]
fn search_bundle_mutation_delta_roundtrip() -> Result<(), DynErr> {
    let delta = SearchBundleMutationDelta {
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        operations: vec![
            SearchBundleMutationOp::UpsertChunk {
                chunk_identity: "c1".to_owned(),
                text_digest: "td1".to_owned(),
            },
            SearchBundleMutationOp::DeleteChunk {
                chunk_identity: "c2".to_owned(),
            },
            SearchBundleMutationOp::UpsertSymbol {
                symbol_id: "s1".to_owned(),
                symbol_digest: "sd1".to_owned(),
            },
            SearchBundleMutationOp::DeleteSymbol {
                symbol_id: "s2".to_owned(),
            },
            SearchBundleMutationOp::UpsertEmbedding {
                entity_id: "e1".to_owned(),
                input_digest: "id1".to_owned(),
            },
            SearchBundleMutationOp::DeleteEmbedding {
                entity_id: "e2".to_owned(),
            },
        ],
    };
    roundtrip(&delta)
}

#[test]
fn search_bundle_mutation_op_variants_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchBundleMutationOp::UpsertChunk {
        chunk_identity: "c".to_owned(),
        text_digest: "td".to_owned(),
    })?;
    roundtrip(&SearchBundleMutationOp::DeleteChunk {
        chunk_identity: "c".to_owned(),
    })?;
    roundtrip(&SearchBundleMutationOp::UpsertSymbol {
        symbol_id: "s".to_owned(),
        symbol_digest: "sd".to_owned(),
    })?;
    roundtrip(&SearchBundleMutationOp::DeleteSymbol {
        symbol_id: "s".to_owned(),
    })?;
    roundtrip(&SearchBundleMutationOp::UpsertEmbedding {
        entity_id: "e".to_owned(),
        input_digest: "id".to_owned(),
    })?;
    roundtrip(&SearchBundleMutationOp::DeleteEmbedding {
        entity_id: "e".to_owned(),
    })
}

#[test]
fn import_edge_roundtrip() -> Result<(), DynErr> {
    roundtrip(&ImportEdge {
        from_symbol: "a".to_owned(),
        to_symbol: "b".to_owned(),
    })
}

#[test]
fn call_edge_roundtrip() -> Result<(), DynErr> {
    roundtrip(&CallEdge {
        caller_symbol: "a".to_owned(),
        callee_symbol: "b".to_owned(),
    })
}

#[test]
fn parse_output_roundtrip() -> Result<(), DynErr> {
    roundtrip(&ParseOutput {
        language: "rust".to_owned(),
        parser_revision: "tree-sitter-rust@0.21".to_owned(),
    })
}

#[test]
fn hir_output_roundtrip() -> Result<(), DynErr> {
    roundtrip(&HirOutput {
        root_kind: "module".to_owned(),
        digest: "abc".to_owned(),
    })
}

#[test]
fn item_index_output_roundtrip() -> Result<(), DynErr> {
    roundtrip(&ItemIndexOutput {
        item_count: 7,
        digest: "def".to_owned(),
    })
}

#[test]
fn build_context_output_roundtrip() -> Result<(), DynErr> {
    roundtrip(&BuildContextOutput {
        profile_name: "release".to_owned(),
        digest: "ghi".to_owned(),
    })
}

#[test]
fn file_materialization_packet_roundtrip() -> Result<(), DynErr> {
    let packet = FileMaterializationPacket {
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        file_id: FileId::new("file-1"),
        file_path: RepoRelativePath::new("src/lib.rs"),
        file_text: "fn main() {}".to_owned(),
        parse_output: ParseOutput {
            language: "rust".to_owned(),
            parser_revision: "ts-rust".to_owned(),
        },
        hir_output: HirOutput {
            root_kind: "crate".to_owned(),
            digest: "h1".to_owned(),
        },
        item_index: ItemIndexOutput {
            item_count: 4,
            digest: "i1".to_owned(),
        },
        build_context: Some(BuildContextOutput {
            profile_name: "debug".to_owned(),
            digest: "b1".to_owned(),
        }),
        import_edges: vec![ImportEdge {
            from_symbol: "x".to_owned(),
            to_symbol: "y".to_owned(),
        }],
        call_edges: vec![CallEdge {
            caller_symbol: "p".to_owned(),
            callee_symbol: "q".to_owned(),
        }],
    };
    roundtrip(&packet)?;
    let mut without_ctx = packet;
    without_ctx.build_context = None;
    roundtrip(&without_ctx)
}

#[test]
fn bundle_notify_roundtrip() -> Result<(), DynErr> {
    roundtrip(&BundleNotify {
        outbox_id: "outbox-1".to_owned(),
    })
}

#[test]
fn published_search_bundle_prepare_request_roundtrip() -> Result<(), DynErr> {
    roundtrip(&PublishedSearchBundlePrepareRequest {
        outbox: sample_prepared_outbox(),
    })
}

#[test]
fn published_search_bundle_prepare_response_roundtrip() -> Result<(), DynErr> {
    roundtrip(&PublishedSearchBundlePrepareResponse {
        accepted: true,
        external_bundle_id: "ext-1".to_owned(),
        state: "prepared".to_owned(),
        reason: Some("ok".to_owned()),
    })?;
    roundtrip(&PublishedSearchBundlePrepareResponse {
        accepted: false,
        external_bundle_id: "ext-2".to_owned(),
        state: "rejected".to_owned(),
        reason: None,
    })
}

#[test]
fn published_search_bundle_delta_apply_request_roundtrip() -> Result<(), DynErr> {
    roundtrip(&PublishedSearchBundleDeltaApplyRequest {
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        generation: sample_published_generation_set(),
        delta: SearchBundleMutationDelta {
            repo_id: sample_repo_id(),
            revision_id: sample_revision_id(),
            manifest_generation: ManifestGeneration::new(7),
            operations: vec![SearchBundleMutationOp::DeleteChunk {
                chunk_identity: "c".to_owned(),
            }],
        },
    })
}

#[test]
fn published_search_bundle_delta_apply_response_roundtrip() -> Result<(), DynErr> {
    roundtrip(&PublishedSearchBundleDeltaApplyResponse {
        applied: true,
        indexed_generation: sample_published_generation_set(),
        reason: Some("ok".to_owned()),
    })?;
    roundtrip(&PublishedSearchBundleDeltaApplyResponse {
        applied: false,
        indexed_generation: sample_published_generation_set(),
        reason: None,
    })
}

#[test]
fn published_search_generation_activate_request_roundtrip() -> Result<(), DynErr> {
    roundtrip(&PublishedSearchGenerationActivateRequest {
        generation: sample_published_generation_set(),
        lexical_ready: true,
        semantic_ready: false,
        active_at_ms: 1_700_000_000_000,
    })
}

#[test]
fn published_search_generation_activate_response_roundtrip() -> Result<(), DynErr> {
    roundtrip(&PublishedSearchGenerationActivateResponse {
        activated: true,
        active_generation: Some(sample_published_generation_set()),
        reason: Some("ok".to_owned()),
    })?;
    roundtrip(&PublishedSearchGenerationActivateResponse {
        activated: false,
        active_generation: None,
        reason: None,
    })
}

#[test]
fn published_search_generation_readiness_response_roundtrip() -> Result<(), DynErr> {
    roundtrip(&PublishedSearchGenerationReadinessResponse {
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        prepared_bundle_count: 3,
        active_generation: Some(sample_published_generation_set()),
        lexical_ready: true,
        semantic_ready: true,
        mode: "ServeOnly".to_owned(),
        reason: Some("ok".to_owned()),
    })?;
    roundtrip(&PublishedSearchGenerationReadinessResponse {
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        prepared_bundle_count: 0,
        active_generation: None,
        lexical_ready: false,
        semantic_ready: false,
        mode: "IndexBuild".to_owned(),
        reason: None,
    })
}

#[test]
fn published_search_bundle_inspect_request_roundtrip() -> Result<(), DynErr> {
    roundtrip(&PublishedSearchBundleInspectRequest {
        generation: sample_published_generation_set(),
    })
}

#[test]
fn published_search_bundle_inspect_response_roundtrip() -> Result<(), DynErr> {
    roundtrip(&PublishedSearchBundleInspectResponse {
        manifest: sample_published_manifest(),
        mode: "ServeOnly".to_owned(),
        artifacts: vec![sample_artifact_ref("a"), sample_artifact_ref("b")],
        state: "ready".to_owned(),
    })
}

#[test]
fn lq_query_roundtrip() -> Result<(), DynErr> {
    roundtrip(&sample_lq_query())?;
    roundtrip(&LqQuery {
        expr: LqExpr::MatchAll,
        filters: LqFilterSet::default(),
        options: LqOptionSet::default(),
        directives: LqDirectiveSet::default(),
    })
}

#[test]
fn lq_expr_variants_roundtrip() -> Result<(), DynErr> {
    roundtrip(&LqExpr::MatchAll)?;
    roundtrip(&LqExpr::Raw("hi".to_owned()))?;
    roundtrip(&LqExpr::All(vec![LqExpr::MatchAll]))?;
    roundtrip(&LqExpr::Any(vec![LqExpr::Raw("a".to_owned())]))?;
    roundtrip(&LqExpr::Not(Box::new(LqExpr::MatchAll)))?;
    // deep recursion
    let nested = LqExpr::All(vec![
        LqExpr::Any(vec![
            LqExpr::Not(Box::new(LqExpr::Raw("x".to_owned()))),
            LqExpr::All(vec![LqExpr::MatchAll]),
        ]),
        LqExpr::Raw("y".to_owned()),
    ]);
    roundtrip(&nested)
}

#[test]
fn lq_filter_set_roundtrip() -> Result<(), DynErr> {
    roundtrip(&LqFilterSet::default())?;
    roundtrip(&LqFilterSet {
        filters: vec![
            LqFilter::Repo("repo".to_owned()),
            LqFilter::File("file".to_owned()),
            LqFilter::Path("path".to_owned()),
            LqFilter::Lang("rust".to_owned()),
            LqFilter::Rev("rev".to_owned()),
            LqFilter::Select("snippet".to_owned()),
            LqFilter::Type("fn".to_owned()),
            LqFilter::Custom {
                key: "k".to_owned(),
                value: "v".to_owned(),
            },
        ],
    })
}

#[test]
fn lq_filter_variants_roundtrip() -> Result<(), DynErr> {
    roundtrip(&LqFilter::Repo("r".to_owned()))?;
    roundtrip(&LqFilter::File("f".to_owned()))?;
    roundtrip(&LqFilter::Path("p".to_owned()))?;
    roundtrip(&LqFilter::Lang("rust".to_owned()))?;
    roundtrip(&LqFilter::Rev("rev".to_owned()))?;
    roundtrip(&LqFilter::Select("s".to_owned()))?;
    roundtrip(&LqFilter::Type("t".to_owned()))?;
    roundtrip(&LqFilter::Custom {
        key: "k".to_owned(),
        value: "v".to_owned(),
    })
}

#[test]
fn lq_option_set_roundtrip() -> Result<(), DynErr> {
    roundtrip(&LqOptionSet::default())?;
    roundtrip(&LqOptionSet {
        limit: Some(50),
        count_all: true,
        timeout_ms: Some(1_000),
    })?;
    roundtrip(&LqOptionSet {
        limit: None,
        count_all: false,
        timeout_ms: None,
    })
}

#[test]
fn lq_directive_set_roundtrip() -> Result<(), DynErr> {
    roundtrip(&LqDirectiveSet::default())?;
    roundtrip(&LqDirectiveSet {
        directives: vec![
            LqDirective::IntoCodeQl,
            LqDirective::ScopeResults,
            LqDirective::WithLexical,
            LqDirective::Custom("trace".to_owned()),
        ],
    })
}

#[test]
fn lq_directive_variants_roundtrip() -> Result<(), DynErr> {
    roundtrip(&LqDirective::IntoCodeQl)?;
    roundtrip(&LqDirective::ScopeResults)?;
    roundtrip(&LqDirective::WithLexical)?;
    roundtrip(&LqDirective::Custom("c".to_owned()))
}

#[test]
fn lexical_candidate_roundtrip() -> Result<(), DynErr> {
    roundtrip(&sample_lexical_candidate())
}

#[test]
fn search_explanation_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchExplanation {
        summary: "explanation text".to_owned(),
    })
}

#[test]
fn search_plane_lexical_query_request_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneLexicalQueryRequest {
        query: sample_lq_query(),
        generation: Some(sample_published_generation_set()),
    })?;
    roundtrip(&SearchPlaneLexicalQueryRequest {
        query: sample_lq_query(),
        generation: None,
    })
}

#[test]
fn search_plane_semantic_query_request_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneSemanticQueryRequest {
        query_text: "needle".to_owned(),
        generation: Some(sample_published_generation_set()),
        lexical_filters: LqFilterSet {
            filters: vec![LqFilter::Repo("repo".to_owned())],
        },
        top_k: 10,
    })?;
    roundtrip(&SearchPlaneSemanticQueryRequest {
        query_text: "needle".to_owned(),
        generation: None,
        lexical_filters: LqFilterSet::default(),
        top_k: 5,
    })
}

#[test]
fn search_plane_hybrid_query_request_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneHybridQueryRequest {
        lexical_query: sample_lq_query(),
        semantic_query_text: "needle".to_owned(),
        generation: Some(sample_published_generation_set()),
        top_k: 20,
    })?;
    roundtrip(&SearchPlaneHybridQueryRequest {
        lexical_query: sample_lq_query(),
        semantic_query_text: "needle".to_owned(),
        generation: None,
        top_k: 1,
    })
}

#[test]
fn search_plane_explain_query_request_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneExplainQueryRequest {
        generation: sample_published_generation_set(),
        candidate: sample_lexical_candidate(),
    })
}

#[test]
fn search_plane_lexical_query_response_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneLexicalQueryResponse {
        generation: sample_published_generation_set(),
        results: vec![sample_lexical_candidate()],
    })
}

#[test]
fn search_plane_semantic_query_response_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneSemanticQueryResponse {
        generation: sample_published_generation_set(),
        results: vec![sample_lexical_candidate()],
    })
}

#[test]
fn search_plane_hybrid_query_response_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneHybridQueryResponse {
        generation: sample_published_generation_set(),
        results: vec![sample_lexical_candidate()],
    })
}

#[test]
fn search_plane_explain_query_response_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneExplainQueryResponse {
        generation: sample_published_generation_set(),
        explanation: SearchExplanation {
            summary: "why".to_owned(),
        },
    })
}

#[test]
fn search_plane_ipc_error_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneIpcError {
        code: "PLANE_BUSY".to_owned(),
        message: "search-plane is currently rebuilding".to_owned(),
    })
}

#[test]
fn search_plane_ipc_request_envelope_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneIpcRequestEnvelope {
        request_id: 17,
        payload: SearchPlaneIpcRequest::Lexical(SearchPlaneLexicalQueryRequest {
            query: sample_lq_query(),
            generation: Some(sample_published_generation_set()),
        }),
    })
}

#[test]
fn search_plane_ipc_request_variants_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneIpcRequest::Lexical(
        SearchPlaneLexicalQueryRequest {
            query: sample_lq_query(),
            generation: None,
        },
    ))?;
    roundtrip(&SearchPlaneIpcRequest::Semantic(
        SearchPlaneSemanticQueryRequest {
            query_text: "q".to_owned(),
            generation: None,
            lexical_filters: LqFilterSet::default(),
            top_k: 3,
        },
    ))?;
    roundtrip(&SearchPlaneIpcRequest::Hybrid(
        SearchPlaneHybridQueryRequest {
            lexical_query: sample_lq_query(),
            semantic_query_text: "q".to_owned(),
            generation: None,
            top_k: 2,
        },
    ))?;
    roundtrip(&SearchPlaneIpcRequest::Explain(
        SearchPlaneExplainQueryRequest {
            generation: sample_published_generation_set(),
            candidate: sample_lexical_candidate(),
        },
    ))
}

#[test]
fn search_plane_ipc_response_envelope_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneIpcResponseEnvelope {
        request_id: 17,
        payload: SearchPlaneIpcResponse::Error(SearchPlaneIpcError {
            code: "X".to_owned(),
            message: "boom".to_owned(),
        }),
    })
}

#[test]
fn search_plane_ipc_response_variants_roundtrip() -> Result<(), DynErr> {
    roundtrip(&SearchPlaneIpcResponse::Lexical(
        SearchPlaneLexicalQueryResponse {
            generation: sample_published_generation_set(),
            results: vec![sample_lexical_candidate()],
        },
    ))?;
    roundtrip(&SearchPlaneIpcResponse::Semantic(
        SearchPlaneSemanticQueryResponse {
            generation: sample_published_generation_set(),
            results: vec![],
        },
    ))?;
    roundtrip(&SearchPlaneIpcResponse::Hybrid(
        SearchPlaneHybridQueryResponse {
            generation: sample_published_generation_set(),
            results: vec![sample_lexical_candidate()],
        },
    ))?;
    roundtrip(&SearchPlaneIpcResponse::Explain(
        SearchPlaneExplainQueryResponse {
            generation: sample_published_generation_set(),
            explanation: SearchExplanation {
                summary: "y".to_owned(),
            },
        },
    ))?;
    roundtrip(&SearchPlaneIpcResponse::Error(SearchPlaneIpcError {
        code: "E".to_owned(),
        message: "m".to_owned(),
    }))
}
