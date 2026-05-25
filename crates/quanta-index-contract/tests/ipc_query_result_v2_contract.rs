#![forbid(unsafe_code)]

use quanta_index_contract::lex::{CommitSha, ExplanationRow};
use quanta_index_contract::results::{
    EngineTouched, PlannerStage, PlannerTraceEntry, SearchExplanation,
};
use quanta_index_contract::{
    BridgeCandidatePacket, BridgeQueryRequest, BridgeScope, BridgeTarget, DiffCandidate,
    DiffHunkSide, GenerationPin, HybridQueryRequest, HybridQueryResponse, LexicalCandidate,
    LqQuery, LqSpan, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchPlaneBridgeQueryResponse, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcResponse, SemanticCandidateScope, SemanticQueryRequest,
    SemanticQueryResponse, SemanticVectorRef, TextQueryRequest, TextQuerySyntax,
};

type TestRes = Result<(), Box<dyn std::error::Error>>;

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buf: Vec<u8> = Vec::new();
    ciborium::ser::into_writer(value, &mut buf)?;
    Ok(buf)
}

fn decode<T>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>>
where
    T: for<'de> serde::Deserialize<'de>,
{
    Ok(ciborium::de::from_reader(bytes)?)
}

fn roundtrip_eq<T>(value: &T) -> TestRes
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de> + core::fmt::Debug + PartialEq,
{
    let bytes = encode(value)?;
    let decoded: T = decode(&bytes)?;
    if &decoded != value {
        return Err(format!("round-trip mismatch: original={value:?}, decoded={decoded:?}").into());
    }
    Ok(())
}

fn map_fields_mut(
    value: &mut ciborium::Value,
) -> Result<&mut Vec<(ciborium::Value, ciborium::Value)>, Box<dyn std::error::Error>> {
    if let ciborium::Value::Map(fields) = value {
        Ok(fields)
    } else {
        Err(format!("expected CBOR map, got {value:?}").into())
    }
}

fn field_value_mut<'a>(
    fields: &'a mut [(ciborium::Value, ciborium::Value)],
    field_name: &str,
) -> Result<&'a mut ciborium::Value, Box<dyn std::error::Error>> {
    fields
        .iter_mut()
        .find_map(|(key, value)| {
            if let ciborium::Value::Text(text) = key {
                (text == field_name).then_some(value)
            } else {
                None
            }
        })
        .ok_or_else(|| format!("field `{field_name}` not found").into())
}

fn duplicate_text_field(
    fields: &mut Vec<(ciborium::Value, ciborium::Value)>,
    field_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let duplicate = fields
        .iter()
        .find_map(|(key, value)| {
            if let ciborium::Value::Text(text) = key {
                (text == field_name).then_some((key.clone(), value.clone()))
            } else {
                None
            }
        })
        .ok_or_else(|| format!("field `{field_name}` not found"))?;
    fields.push(duplicate);
    Ok(())
}

fn mutate_ipc_request_wire<F>(
    request: &SearchPlaneQueryIpcRequest,
    mutate: F,
) -> Result<Vec<u8>, Box<dyn std::error::Error>>
where
    F: FnOnce(&mut ciborium::Value) -> Result<(), Box<dyn std::error::Error>>,
{
    let mut wire: ciborium::Value = decode(&encode(request)?)?;
    mutate(&mut wire)?;
    encode(&wire)
}

fn mutate_ipc_response_wire<F>(
    response: &SearchPlaneQueryIpcResponse,
    mutate: F,
) -> Result<Vec<u8>, Box<dyn std::error::Error>>
where
    F: FnOnce(&mut ciborium::Value) -> Result<(), Box<dyn std::error::Error>>,
{
    let mut wire: ciborium::Value = decode(&encode(response)?)?;
    mutate(&mut wire)?;
    encode(&wire)
}

fn expect_decode_error_contains<T>(bytes: &[u8], expected_fragment: &str) -> TestRes
where
    T: for<'de> serde::Deserialize<'de> + core::fmt::Debug,
{
    let result: Result<T, _> = ciborium::de::from_reader(bytes);
    match result {
        Ok(decoded) => Err(format!("decode unexpectedly succeeded: {decoded:?}").into()),
        Err(err) => {
            let message = err.to_string();
            if !message.contains(expected_fragment) {
                return Err(format!(
                    "decode error did not mention `{expected_fragment}`: {message}"
                )
                .into());
            }
            Ok(())
        }
    }
}

fn generation_pin() -> GenerationPin {
    GenerationPin::new(
        RepoId::new("repo-1"),
        RevisionId::new("rev-1"),
        ManifestGeneration::new(7),
    )
}

fn lexical_request() -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "repo:quanta-index lang:rust SearchPlane".to_owned(),
        generation: Some(generation_pin()),
        generation_selector: None,
    }
}

fn semantic_scope() -> SemanticCandidateScope {
    SemanticCandidateScope {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "repo:quanta-index lang:rust SearchPlane".to_owned(),
        generation: Some(generation_pin()),
        generation_selector: None,
    }
}

fn lexical_candidate() -> LexicalCandidate {
    LexicalCandidate {
        candidate_id: "cand-1".to_owned(),
        repo_id: RepoId::new("repo-1"),
        revision_id: RevisionId::new("rev-1"),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/search.rs"),
        start_line: 10,
        end_line: 18,
        score: 0.875,
        snippet: "fn search_plane() {}".to_owned(),
    }
}

fn semantic_request() -> SemanticQueryRequest {
    SemanticQueryRequest {
        query_text: "1.0 0.0".to_owned(),
        query_vector: None,
        query_vector_ref: None,
        generation: Some(generation_pin()),
        generation_selector: None,
        scope: Some(semantic_scope()),
        top_k: 25,
    }
}

fn hybrid_request() -> HybridQueryRequest {
    HybridQueryRequest {
        text_query: lexical_request(),
        semantic_query_text: "0.0 1.0".to_owned(),
        semantic_vector: None,
        semantic_vector_ref: None,
        generation: Some(generation_pin()),
        generation_selector: None,
        top_k: 50,
    }
}

fn semantic_request_with_vector_ref(vector_ref: SemanticVectorRef) -> SemanticQueryRequest {
    SemanticQueryRequest {
        query_text: "semantic-explicit-vector".to_owned(),
        query_vector: None,
        query_vector_ref: Some(vector_ref),
        generation: Some(generation_pin()),
        generation_selector: None,
        scope: Some(semantic_scope()),
        top_k: 25,
    }
}

fn hybrid_request_with_vector_ref(vector_ref: SemanticVectorRef) -> HybridQueryRequest {
    HybridQueryRequest {
        text_query: lexical_request(),
        semantic_query_text: "hybrid-explicit-vector".to_owned(),
        semantic_vector: None,
        semantic_vector_ref: Some(vector_ref),
        generation: Some(generation_pin()),
        generation_selector: None,
        top_k: 50,
    }
}

fn explanation_v2() -> SearchExplanation {
    SearchExplanation {
        planner_trace: vec![
            PlannerTraceEntry {
                stage: PlannerStage::Normalize,
                detail: "syntax=sourcegraph".to_owned(),
            },
            PlannerTraceEntry {
                stage: PlannerStage::ExecFanout,
                detail: "bridge and history enabled".to_owned(),
            },
        ],
        engines_touched: vec![EngineTouched::History, EngineTouched::Bridge],
        early_stop_reason: None,
        contributions: vec![ExplanationRow {
            signal_name: "lexical.score".into(),
            signal_value: 0.875,
            weight: 1.25,
            contribution: 1.09375,
        }],
        ranker_weights_hash: [9u8; 32],
        strategy: "hybrid-v2".to_owned(),
        summary: "bridge rerank kept one history-backed candidate".to_owned(),
    }
}

fn mutate_query_wire<F>(mutate: F) -> Result<Vec<u8>, Box<dyn std::error::Error>>
where
    F: FnOnce(&mut ciborium::Value) -> Result<(), Box<dyn std::error::Error>>,
{
    let query = LqQuery::empty(LqSpan::eof(0));
    let mut wire: ciborium::Value = decode(&encode(&query)?)?;
    mutate(&mut wire)?;
    encode(&wire)
}

#[test]
fn lq_query_cbor_decode_rejects_missing_lq_version() -> TestRes {
    let bytes = mutate_query_wire(|wire| {
        let ciborium::Value::Map(fields) = wire else {
            return Err(format!("expected query map, got {wire:?}").into());
        };
        fields.retain(
            |(key, _value)| !matches!(key, ciborium::Value::Text(text) if text == "lq_version"),
        );
        Ok(())
    })?;

    let result: Result<LqQuery, _> = ciborium::de::from_reader(bytes.as_slice());
    match result {
        Ok(decoded) => Err(format!("decode unexpectedly succeeded: {decoded:?}").into()),
        Err(err) => {
            let message = err.to_string();
            if !message.contains("lq_version") {
                return Err(
                    format!("missing-field error did not mention lq_version: {message}").into(),
                );
            }
            Ok(())
        }
    }
}

#[test]
fn lq_query_cbor_decode_rejects_unknown_lq_version() -> TestRes {
    let bytes = mutate_query_wire(|wire| {
        let ciborium::Value::Map(fields) = wire else {
            return Err(format!("expected query map, got {wire:?}").into());
        };
        for (key, value) in fields {
            if matches!(key, ciborium::Value::Text(text) if text == "lq_version") {
                *value = ciborium::Value::Text("2.0".to_owned());
                return Ok(());
            }
        }
        Err("lq_version field not found in encoded query".into())
    })?;

    let result: Result<LqQuery, _> = ciborium::de::from_reader(bytes.as_slice());
    match result {
        Ok(decoded) => Err(format!("decode unexpectedly succeeded: {decoded:?}").into()),
        Err(err) => {
            let message = err.to_string();
            if !message.contains("2.0") {
                return Err(format!(
                    "unknown-variant error did not mention rejected version: {message}"
                )
                .into());
            }
            Ok(())
        }
    }
}

#[test]
fn search_plane_ipc_request_v2_bridge_roundtrips_sourcegraph_syntax() -> TestRes {
    let request = SearchPlaneQueryIpcRequest::Bridge(BridgeQueryRequest {
        text_query: lexical_request(),
        target: BridgeTarget::CodeQl,
    });

    roundtrip_eq(&request)?;

    let decoded: SearchPlaneQueryIpcRequest = decode(&encode(&request)?)?;
    if let SearchPlaneQueryIpcRequest::Bridge(inner) = decoded {
        if inner.text_query.syntax != TextQuerySyntax::Sourcegraph {
            return Err(format!(
                "expected sourcegraph syntax, got {:?}",
                inner.text_query.syntax
            )
            .into());
        }
        if inner.target != BridgeTarget::CodeQl {
            return Err(format!("expected codeql target, got {:?}", inner.target).into());
        }
        Ok(())
    } else {
        Err(format!("expected Bridge request, got {decoded:?}").into())
    }
}

#[test]
fn search_plane_ipc_request_v2_semantic_roundtrips_nested_lexical_scope() -> TestRes {
    let request = SearchPlaneQueryIpcRequest::Semantic(semantic_request_with_vector_ref(
        SemanticVectorRef::Inline(vec![1.0, 0.0, -1.0]),
    ));

    roundtrip_eq(&request)?;

    let decoded: SearchPlaneQueryIpcRequest = decode(&encode(&request)?)?;
    if let SearchPlaneQueryIpcRequest::Semantic(inner) = decoded {
        let lexical_scope = inner.scope.ok_or_else(|| "expected scope".to_string())?;
        if lexical_scope.syntax != TextQuerySyntax::Sourcegraph {
            return Err(format!(
                "expected sourcegraph syntax, got {:?}",
                lexical_scope.syntax
            )
            .into());
        }
        if inner.top_k != 25 {
            return Err(format!("expected top_k=25, got {}", inner.top_k).into());
        }
        if inner.query_vector_ref != Some(SemanticVectorRef::Inline(vec![1.0, 0.0, -1.0])) {
            return Err(format!(
                "unexpected semantic query_vector_ref: {:?}",
                inner.query_vector_ref
            )
            .into());
        }
        Ok(())
    } else {
        Err(format!("expected Semantic request, got {decoded:?}").into())
    }
}

#[test]
fn search_plane_ipc_request_v2_hybrid_roundtrips_lexical_subquery() -> TestRes {
    let request = SearchPlaneQueryIpcRequest::Hybrid(hybrid_request_with_vector_ref(
        SemanticVectorRef::Handle("vec-handle-1".into()),
    ));

    roundtrip_eq(&request)?;

    let decoded: SearchPlaneQueryIpcRequest = decode(&encode(&request)?)?;
    if let SearchPlaneQueryIpcRequest::Hybrid(inner) = decoded {
        if inner.text_query.syntax != TextQuerySyntax::Sourcegraph {
            return Err(format!(
                "expected sourcegraph syntax, got {:?}",
                inner.text_query.syntax
            )
            .into());
        }
        if inner.top_k != 50 {
            return Err(format!("expected top_k=50, got {}", inner.top_k).into());
        }
        if inner.semantic_vector_ref != Some(SemanticVectorRef::Handle("vec-handle-1".into())) {
            return Err(format!(
                "unexpected hybrid semantic_vector_ref: {:?}",
                inner.semantic_vector_ref
            )
            .into());
        }
        Ok(())
    } else {
        Err(format!("expected Hybrid request, got {decoded:?}").into())
    }
}

#[test]
fn search_plane_query_ipc_request_envelope_semantic_roundtrips_inline_vector_ref() -> TestRes {
    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 41,
        payload: SearchPlaneQueryIpcRequest::Semantic(semantic_request_with_vector_ref(
            SemanticVectorRef::Inline(vec![0.5, 0.25, -0.75]),
        )),
    };

    roundtrip_eq(&request)?;

    let decoded: SearchPlaneQueryIpcRequestEnvelope = decode(&encode(&request)?)?;
    if let SearchPlaneQueryIpcRequest::Semantic(inner) = decoded.payload {
        if inner.query_vector_ref != Some(SemanticVectorRef::Inline(vec![0.5, 0.25, -0.75])) {
            return Err(format!(
                "unexpected semantic query envelope query_vector_ref: {:?}",
                inner.query_vector_ref
            )
            .into());
        }
        Ok(())
    } else {
        Err(format!("expected split Semantic request, got {:?}", decoded.payload).into())
    }
}

#[test]
fn search_plane_query_ipc_request_envelope_hybrid_roundtrips_handle_vector_ref() -> TestRes {
    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 42,
        payload: SearchPlaneQueryIpcRequest::Hybrid(hybrid_request_with_vector_ref(
            SemanticVectorRef::Handle("vec-handle-query".into()),
        )),
    };

    roundtrip_eq(&request)?;

    let decoded: SearchPlaneQueryIpcRequestEnvelope = decode(&encode(&request)?)?;
    if let SearchPlaneQueryIpcRequest::Hybrid(inner) = decoded.payload {
        if inner.semantic_vector_ref != Some(SemanticVectorRef::Handle("vec-handle-query".into())) {
            return Err(format!(
                "unexpected hybrid query envelope semantic_vector_ref: {:?}",
                inner.semantic_vector_ref
            )
            .into());
        }
        Ok(())
    } else {
        Err(format!("expected split Hybrid request, got {:?}", decoded.payload).into())
    }
}

#[test]
fn search_plane_ipc_request_v2_semantic_rejects_duplicate_nested_lexical_syntax() -> TestRes {
    let bytes = mutate_ipc_request_wire(
        &SearchPlaneQueryIpcRequest::Semantic(semantic_request()),
        |wire| {
            let request_fields = map_fields_mut(wire)?;
            let payload = field_value_mut(request_fields, "payload")?;
            let payload_fields = map_fields_mut(payload)?;
            let lexical_scope = field_value_mut(payload_fields, "scope")?;
            let lexical_scope_fields = map_fields_mut(lexical_scope)?;
            duplicate_text_field(lexical_scope_fields, "syntax")?;
            Ok(())
        },
    )?;

    expect_decode_error_contains::<SearchPlaneQueryIpcRequest>(&bytes, "syntax")
}

#[test]
fn search_plane_ipc_request_v2_hybrid_rejects_duplicate_top_k() -> TestRes {
    let bytes = mutate_ipc_request_wire(
        &SearchPlaneQueryIpcRequest::Hybrid(hybrid_request()),
        |wire| {
            let request_fields = map_fields_mut(wire)?;
            let payload = field_value_mut(request_fields, "payload")?;
            let payload_fields = map_fields_mut(payload)?;
            duplicate_text_field(payload_fields, "top_k")?;
            Ok(())
        },
    )?;

    expect_decode_error_contains::<SearchPlaneQueryIpcRequest>(&bytes, "top_k")
}

#[test]
fn search_plane_ipc_response_v2_bridge_roundtrips_nested_payload() -> TestRes {
    let response = SearchPlaneQueryIpcResponse::Bridge(SearchPlaneBridgeQueryResponse {
        generation: generation_pin(),
        packet: BridgeCandidatePacket {
            target: BridgeTarget::CodeQl,
            scope: BridgeScope::Hybrid,
            repo_id: RepoId::new("repo-1"),
            revision_id: RevisionId::new("rev-1"),
            manifest_generation: ManifestGeneration::new(7),
            source_syntax: Some("sourcegraph".to_owned()),
            translator_version: Some("bridge-v2".to_owned()),
            candidates: vec![lexical_candidate()],
        },
    });

    roundtrip_eq(&response)?;

    let decoded: SearchPlaneQueryIpcResponse = decode(&encode(&response)?)?;
    if let SearchPlaneQueryIpcResponse::Bridge(inner) = decoded {
        if inner.packet.scope != BridgeScope::Hybrid {
            return Err(
                format!("expected hybrid bridge scope, got {:?}", inner.packet.scope).into(),
            );
        }
        if inner.packet.source_syntax.as_deref() != Some("sourcegraph") {
            return Err(format!(
                "expected source_syntax=sourcegraph, got {:?}",
                inner.packet.source_syntax
            )
            .into());
        }
        Ok(())
    } else {
        Err(format!("expected Bridge response, got {decoded:?}").into())
    }
}

#[test]
fn search_plane_ipc_response_v2_semantic_roundtrips_explanation() -> TestRes {
    let response = SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
        generation: generation_pin(),
        results: vec![lexical_candidate()],
        explanation: explanation_v2(),
    });

    roundtrip_eq(&response)?;

    let decoded: SearchPlaneQueryIpcResponse = decode(&encode(&response)?)?;
    if let SearchPlaneQueryIpcResponse::Semantic(inner) = decoded {
        if inner.explanation.strategy != "hybrid-v2" {
            return Err(format!(
                "unexpected semantic explanation strategy: {}",
                inner.explanation.strategy
            )
            .into());
        }
        Ok(())
    } else {
        Err(format!("expected Semantic response, got {decoded:?}").into())
    }
}

#[test]
fn search_plane_ipc_response_v2_hybrid_roundtrips_explanation() -> TestRes {
    let response = SearchPlaneQueryIpcResponse::Hybrid(HybridQueryResponse {
        generation: generation_pin(),
        results: vec![lexical_candidate()],
        explanation: explanation_v2(),
    });

    roundtrip_eq(&response)?;

    let decoded: SearchPlaneQueryIpcResponse = decode(&encode(&response)?)?;
    if let SearchPlaneQueryIpcResponse::Hybrid(inner) = decoded {
        if inner.explanation.planner_trace.len() != 2 {
            return Err(format!(
                "expected 2 planner_trace entries, got {}",
                inner.explanation.planner_trace.len()
            )
            .into());
        }
        Ok(())
    } else {
        Err(format!("expected Hybrid response, got {decoded:?}").into())
    }
}

#[test]
fn results_search_explanation_v2_roundtrips_trace_engines_and_summary() -> TestRes {
    let explanation = explanation_v2();
    roundtrip_eq(&explanation)?;

    let decoded: SearchExplanation = decode(&encode(&explanation)?)?;
    if decoded.planner_trace.len() != 2 {
        return Err(format!(
            "expected 2 planner_trace entries, got {}",
            decoded.planner_trace.len()
        )
        .into());
    }
    if decoded.engines_touched != vec![EngineTouched::History, EngineTouched::Bridge] {
        return Err(format!("unexpected engines_touched: {:?}", decoded.engines_touched).into());
    }
    if decoded.summary != "bridge rerank kept one history-backed candidate" {
        return Err(format!("unexpected summary: {}", decoded.summary).into());
    }
    Ok(())
}

#[test]
fn search_plane_ipc_response_v2_history_variant_roundtrips() -> TestRes {
    let response = SearchPlaneQueryIpcResponse::History(
        quanta_index_contract::SearchPlaneHistoryQueryResponse {
            generation: generation_pin(),
            commits: vec![quanta_index_contract::CommitCandidate {
                sha: CommitSha::from_bytes([1u8; 20]),
                parent_ids: vec![CommitSha::from_bytes([2u8; 20])],
                committed_at_unix_s: 1_717_171_717,
                author: "alice@example.com".to_owned(),
                committer: "bob@example.com".to_owned(),
                message: "bridge request landed".to_owned(),
                is_merge: false,
                tags: vec!["v2".to_owned()],
            }],
            diffs: vec![DiffCandidate {
                repo_relative_path: "src/search.rs".to_owned(),
                hunk_header: "@@ -10,4 +10,7 @@".to_owned(),
                side: DiffHunkSide::After,
                line_start: 10,
                line_end: 16,
                snippet: "+ bridge_search(query);".to_owned(),
            }],
        },
    );

    roundtrip_eq(&response)
}

#[test]
fn search_plane_ipc_response_v2_lexical_rejects_duplicate_results() -> TestRes {
    let response = SearchPlaneQueryIpcResponse::Text(quanta_index_contract::TextQueryResponse {
        generation: generation_pin(),
        results: vec![lexical_candidate()],
    });
    let bytes = mutate_ipc_response_wire(&response, |wire| {
        let response_fields = map_fields_mut(wire)?;
        let payload = field_value_mut(response_fields, "payload")?;
        let payload_fields = map_fields_mut(payload)?;
        duplicate_text_field(payload_fields, "results")?;
        Ok(())
    })?;

    expect_decode_error_contains::<SearchPlaneQueryIpcResponse>(&bytes, "results")
}

#[test]
fn search_plane_ipc_response_v2_semantic_rejects_duplicate_generation() -> TestRes {
    let response = SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
        generation: generation_pin(),
        results: vec![lexical_candidate()],
        explanation: explanation_v2(),
    });
    let bytes = mutate_ipc_response_wire(&response, |wire| {
        let response_fields = map_fields_mut(wire)?;
        let payload = field_value_mut(response_fields, "payload")?;
        let payload_fields = map_fields_mut(payload)?;
        duplicate_text_field(payload_fields, "generation")?;
        Ok(())
    })?;

    expect_decode_error_contains::<SearchPlaneQueryIpcResponse>(&bytes, "generation")
}

#[test]
fn search_plane_ipc_response_v2_hybrid_rejects_duplicate_explanation() -> TestRes {
    let response = SearchPlaneQueryIpcResponse::Hybrid(HybridQueryResponse {
        generation: generation_pin(),
        results: vec![lexical_candidate()],
        explanation: explanation_v2(),
    });
    let bytes = mutate_ipc_response_wire(&response, |wire| {
        let response_fields = map_fields_mut(wire)?;
        let payload = field_value_mut(response_fields, "payload")?;
        let payload_fields = map_fields_mut(payload)?;
        duplicate_text_field(payload_fields, "explanation")?;
        Ok(())
    })?;

    expect_decode_error_contains::<SearchPlaneQueryIpcResponse>(&bytes, "explanation")
}
