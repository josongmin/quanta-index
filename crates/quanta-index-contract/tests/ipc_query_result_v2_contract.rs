#![forbid(unsafe_code)]

use quanta_index_contract::lex::{CommitSha, ExplanationRow, SymbolKindCode, SymbolKindFamily};
use quanta_index_contract::results::{
    EngineTouched, PlannerStage, PlannerTraceEntry, SearchExplanation,
};
use quanta_index_contract::{
    DiffCandidate, DiffHunkSide, GenerationPin, HybridQueryRequest, HybridQueryResponse,
    LexicalCandidate, LqQuery, LqSpan, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SemanticQueryRequest, SemanticQueryResponse, StructuralQueryRequest, SymbolCandidate,
    TextQueryRequest, TextQuerySyntax,
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
        top_k: 50,
    }
}

fn semantic_scope() -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "repo:quanta-index lang:rust SearchPlane".to_owned(),
        generation: Some(generation_pin()),
        generation_selector: None,
        // LXE-01 §3: lexical scope unified on TextQueryRequest. The lexical
        // candidate cap for the semantic pre-filter; tests use a tight cap.
        top_k: 50,
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
        // J7Q-07: a concrete hit offset so the wire round-trip proves the new
        // UI highlight-anchor field survives serialize -> deserialize.
        snippet_hit_offset: Some(3),
    }
}

fn symbol_candidate() -> Result<SymbolCandidate, Box<dyn std::error::Error>> {
    Ok(SymbolCandidate {
        candidate_id: "sym-1".to_owned(),
        repo_id: RepoId::new("repo-1"),
        revision_id: RevisionId::new("rev-1"),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/search.rs"),
        start_line: 10,
        end_line: 18,
        score: 0.875,
        snippet: "search_plane crate".to_owned(),
        symbol_kind: SymbolKindCode::new("function")?,
        symbol_kind_family: Some(SymbolKindFamily::Callable),
    })
}

fn semantic_request() -> SemanticQueryRequest {
    SemanticQueryRequest {
        query_text: "1.0 0.0".to_owned(),
        generation: Some(generation_pin()),
        generation_selector: None,
        lexical_scope: Some(semantic_scope()),
        top_k: 25,
    }
}

fn hybrid_request() -> HybridQueryRequest {
    HybridQueryRequest {
        text_query: lexical_request(),
        semantic_query_text: "0.0 1.0".to_owned(),
        generation: Some(generation_pin()),
        generation_selector: None,
        top_k: 50,
    }
}

fn semantic_request_with_query_text(query_text: &str) -> SemanticQueryRequest {
    SemanticQueryRequest {
        query_text: query_text.to_owned(),
        generation: Some(generation_pin()),
        generation_selector: None,
        lexical_scope: Some(semantic_scope()),
        top_k: 25,
    }
}

fn hybrid_request_with_query_text(query_text: &str) -> HybridQueryRequest {
    HybridQueryRequest {
        text_query: lexical_request(),
        semantic_query_text: query_text.to_owned(),
        generation: Some(generation_pin()),
        generation_selector: None,
        top_k: 50,
    }
}

fn sourcegraph_text_request() -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "repo:quanta-index lang:rust SearchPlane".to_owned(),
        generation: Some(generation_pin()),
        generation_selector: None,
        top_k: 25,
    }
}

fn sourcegraph_structural_request() -> StructuralQueryRequest {
    StructuralQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: r#"repo:quanta-index lang:rust patterntype:structural "function_item""#
                .to_owned(),
            generation: Some(generation_pin()),
            generation_selector: None,
            top_k: 25,
        },
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
fn search_plane_ipc_request_v2_semantic_roundtrips_nested_lexical_scope() -> TestRes {
    let request =
        SearchPlaneQueryIpcRequest::Semantic(semantic_request_with_query_text("1.0 0.0 -1.0"));

    roundtrip_eq(&request)?;

    let decoded: SearchPlaneQueryIpcRequest = decode(&encode(&request)?)?;
    if let SearchPlaneQueryIpcRequest::Semantic(inner) = decoded {
        let lexical_scope = inner
            .lexical_scope
            .ok_or_else(|| "expected lexical_scope".to_string())?;
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
        if inner.query_text.as_str() != "1.0 0.0 -1.0" {
            return Err(format!("unexpected semantic query_text: {:?}", inner.query_text).into());
        }
        Ok(())
    } else {
        Err(format!("expected Semantic request, got {decoded:?}").into())
    }
}

#[test]
fn search_plane_ipc_request_v2_hybrid_roundtrips_lexical_subquery() -> TestRes {
    let request =
        SearchPlaneQueryIpcRequest::Hybrid(hybrid_request_with_query_text("vec-handle-1"));

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
        if inner.semantic_query_text.as_str() != "vec-handle-1" {
            return Err(format!(
                "unexpected hybrid semantic_query_text: {:?}",
                inner.semantic_query_text
            )
            .into());
        }
        Ok(())
    } else {
        Err(format!("expected Hybrid request, got {decoded:?}").into())
    }
}

#[test]
fn search_plane_ipc_request_v2_sourcegraph_roundtrips_text_variant() -> TestRes {
    let request = SearchPlaneQueryIpcRequest::Text(sourcegraph_text_request());

    roundtrip_eq(&request)?;

    let decoded: SearchPlaneQueryIpcRequest = decode(&encode(&request)?)?;
    if let SearchPlaneQueryIpcRequest::Text(inner) = decoded {
        if inner.syntax != TextQuerySyntax::Sourcegraph {
            return Err(format!("unexpected sourcegraph syntax: {:?}", inner.syntax).into());
        }
        if inner.query_text.as_str() != "repo:quanta-index lang:rust SearchPlane" {
            return Err(
                format!("unexpected sourcegraph query_text: {:?}", inner.query_text).into(),
            );
        }
        if inner.generation != Some(generation_pin()) || inner.generation_selector.is_some() {
            return Err(format!("unexpected sourcegraph pin: {inner:?}").into());
        }
        if inner.top_k != 25 {
            return Err(format!("expected top_k=25, got {}", inner.top_k).into());
        }
        Ok(())
    } else {
        Err(format!("expected Text request, got {decoded:?}").into())
    }
}

#[test]
fn search_plane_ipc_request_v2_sourcegraph_roundtrips_structural_variant() -> TestRes {
    let request = SearchPlaneQueryIpcRequest::Structural(sourcegraph_structural_request());

    roundtrip_eq(&request)?;

    let decoded: SearchPlaneQueryIpcRequest = decode(&encode(&request)?)?;
    if let SearchPlaneQueryIpcRequest::Structural(inner) = decoded {
        if inner.text_query.syntax != TextQuerySyntax::Sourcegraph {
            return Err(format!(
                "unexpected structural sourcegraph syntax: {:?}",
                inner.text_query.syntax
            )
            .into());
        }
        if inner.text_query.query_text.as_str()
            != r#"repo:quanta-index lang:rust patterntype:structural "function_item""#
        {
            return Err(format!(
                "unexpected structural sourcegraph query_text: {:?}",
                inner.text_query.query_text
            )
            .into());
        }
        if inner.text_query.generation != Some(generation_pin())
            || inner.text_query.generation_selector.is_some()
        {
            return Err(format!("unexpected structural sourcegraph pin: {inner:?}").into());
        }
        if inner.text_query.top_k != 25 {
            return Err(format!("expected top_k=25, got {}", inner.text_query.top_k).into());
        }
        Ok(())
    } else {
        Err(format!("expected Structural request, got {decoded:?}").into())
    }
}

#[test]
fn search_plane_query_ipc_request_envelope_semantic_roundtrips_query_text() -> TestRes {
    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 41,
        payload: SearchPlaneQueryIpcRequest::Semantic(semantic_request_with_query_text(
            "0.5 0.25 -0.75",
        )),
    };

    roundtrip_eq(&request)?;

    let decoded: SearchPlaneQueryIpcRequestEnvelope = decode(&encode(&request)?)?;
    if let SearchPlaneQueryIpcRequest::Semantic(inner) = decoded.payload {
        if inner.query_text.as_str() != "0.5 0.25 -0.75" {
            return Err(format!(
                "unexpected semantic query envelope query_text: {:?}",
                inner.query_text
            )
            .into());
        }
        Ok(())
    } else {
        Err(format!("expected split Semantic request, got {:?}", decoded.payload).into())
    }
}

#[test]
fn search_plane_query_ipc_request_envelope_hybrid_roundtrips_semantic_text() -> TestRes {
    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 42,
        payload: SearchPlaneQueryIpcRequest::Hybrid(hybrid_request_with_query_text(
            "vec-handle-query",
        )),
    };

    roundtrip_eq(&request)?;

    let decoded: SearchPlaneQueryIpcRequestEnvelope = decode(&encode(&request)?)?;
    if let SearchPlaneQueryIpcRequest::Hybrid(inner) = decoded.payload {
        if inner.semantic_query_text.as_str() != "vec-handle-query" {
            return Err(format!(
                "unexpected hybrid query envelope semantic_query_text: {:?}",
                inner.semantic_query_text
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
            let lexical_scope = field_value_mut(payload_fields, "lexical_scope")?;
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
fn search_plane_ipc_request_v2_semantic_rejects_legacy_query_vector_ref_field() -> TestRes {
    let bytes = mutate_ipc_request_wire(
        &SearchPlaneQueryIpcRequest::Semantic(semantic_request()),
        |wire| {
            let request_fields = map_fields_mut(wire)?;
            let payload = field_value_mut(request_fields, "payload")?;
            let payload_fields = map_fields_mut(payload)?;
            payload_fields.push((
                ciborium::Value::Text("query_vector_ref".to_owned()),
                ciborium::Value::Text("legacy-inline-vector".to_owned()),
            ));
            Ok(())
        },
    )?;

    expect_decode_error_contains::<SearchPlaneQueryIpcRequest>(&bytes, "query_vector_ref")
}

#[test]
fn search_plane_ipc_request_v2_hybrid_rejects_legacy_semantic_vector_ref_field() -> TestRes {
    let bytes = mutate_ipc_request_wire(
        &SearchPlaneQueryIpcRequest::Hybrid(hybrid_request()),
        |wire| {
            let request_fields = map_fields_mut(wire)?;
            let payload = field_value_mut(request_fields, "payload")?;
            let payload_fields = map_fields_mut(payload)?;
            payload_fields.push((
                ciborium::Value::Text("semantic_vector_ref".to_owned()),
                ciborium::Value::Text("legacy-handle".to_owned()),
            ));
            Ok(())
        },
    )?;

    expect_decode_error_contains::<SearchPlaneQueryIpcRequest>(&bytes, "semantic_vector_ref")
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
fn search_plane_ipc_response_v2_symbol_roundtrips_kind_truth() -> TestRes {
    let response =
        SearchPlaneQueryIpcResponse::Symbol(quanta_index_contract::SymbolQueryResponse {
            generation: generation_pin(),
            results: vec![symbol_candidate()?],
        });

    roundtrip_eq(&response)?;

    let decoded: SearchPlaneQueryIpcResponse = decode(&encode(&response)?)?;
    if let SearchPlaneQueryIpcResponse::Symbol(inner) = decoded {
        if inner.results.len() != 1 {
            return Err(
                format!("expected one symbol candidate, got {}", inner.results.len()).into(),
            );
        }
        let first = inner
            .results
            .first()
            .ok_or_else(|| "missing symbol candidate".to_string())?;
        if first.symbol_kind.as_str() != "function"
            || first.symbol_kind_family != Some(SymbolKindFamily::Callable)
        {
            return Err(format!("unexpected symbol candidate: {first:?}").into());
        }
        Ok(())
    } else {
        Err(format!("expected Symbol response, got {decoded:?}").into())
    }
}

#[test]
fn search_plane_ipc_response_v2_sourcegraph_roundtrips_text_candidates() -> TestRes {
    let response = SearchPlaneQueryIpcResponse::Text(quanta_index_contract::TextQueryResponse {
        generation: generation_pin(),
        results: vec![lexical_candidate()],
        file_owner_rows: None,
    });

    roundtrip_eq(&response)?;

    let decoded: SearchPlaneQueryIpcResponse = decode(&encode(&response)?)?;
    if let SearchPlaneQueryIpcResponse::Text(inner) = decoded {
        if inner.generation != generation_pin() {
            return Err(
                format!("unexpected sourcegraph generation: {:?}", inner.generation).into(),
            );
        }
        if inner.results.len() != 1 {
            return Err(format!(
                "expected one sourcegraph candidate, got {}",
                inner.results.len()
            )
            .into());
        }
        Ok(())
    } else {
        Err(format!("expected Text response, got {decoded:?}").into())
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
        file_owner_rows: None,
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
fn search_plane_ipc_response_v2_roundtrips_file_owner_projection_rows() -> TestRes {
    let response = SearchPlaneQueryIpcResponse::Text(quanta_index_contract::TextQueryResponse {
        generation: generation_pin(),
        results: vec![lexical_candidate()],
        file_owner_rows: Some(vec![quanta_index_contract::FileOwnerProjectionRow {
            candidate_id: "lex-1".to_string(),
            repo_id: RepoId::new("repo-a"),
            revision_id: RevisionId::new("rev-a"),
            manifest_generation: ManifestGeneration::new(7),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            owners: vec!["@alice".to_string(), "@acme/platform".to_string()],
        }]),
    });

    roundtrip_eq(&response)?;

    let decoded: SearchPlaneQueryIpcResponse = decode(&encode(&response)?)?;
    let SearchPlaneQueryIpcResponse::Text(inner) = decoded else {
        return Err("expected Text response".into());
    };
    let Some(file_owner_rows) = inner.file_owner_rows else {
        return Err("missing file_owner_rows".into());
    };
    let [file_owner_row] = file_owner_rows.as_slice() else {
        return Err(format!(
            "expected one file owner projection row, got {}",
            file_owner_rows.len()
        )
        .into());
    };
    if file_owner_row.owners != vec!["@alice".to_string(), "@acme/platform".to_string()] {
        return Err(format!("unexpected owners: {:?}", file_owner_row.owners).into());
    }
    Ok(())
}

#[test]
fn search_plane_ipc_response_v2_symbol_rejects_duplicate_symbol_kind() -> TestRes {
    let response =
        SearchPlaneQueryIpcResponse::Symbol(quanta_index_contract::SymbolQueryResponse {
            generation: generation_pin(),
            results: vec![symbol_candidate()?],
        });
    let bytes = mutate_ipc_response_wire(&response, |wire| {
        let response_fields = map_fields_mut(wire)?;
        let payload = field_value_mut(response_fields, "payload")?;
        let payload_fields = map_fields_mut(payload)?;
        let results = field_value_mut(payload_fields, "results")?;
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "ciborium::Value is non_exhaustive; keep a future-variant rejection arm"
        )]
        let array = match results {
            ciborium::Value::Array(values) => values,
            other @ (ciborium::Value::Integer(_)
            | ciborium::Value::Bytes(_)
            | ciborium::Value::Float(_)
            | ciborium::Value::Text(_)
            | ciborium::Value::Bool(_)
            | ciborium::Value::Null
            | ciborium::Value::Tag(_, _)
            | ciborium::Value::Map(_)) => {
                return Err(format!("expected results array, got {other:?}").into());
            }
            other => {
                return Err(format!(
                    "expected results array, got future/non-exhaustive value {other:?}"
                )
                .into());
            }
        };
        let first = array
            .first_mut()
            .ok_or_else(|| "expected first symbol result".to_string())?;
        let symbol_fields = map_fields_mut(first)?;
        duplicate_text_field(symbol_fields, "symbol_kind")?;
        Ok(())
    })?;

    expect_decode_error_contains::<SearchPlaneQueryIpcResponse>(&bytes, "symbol_kind")
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
