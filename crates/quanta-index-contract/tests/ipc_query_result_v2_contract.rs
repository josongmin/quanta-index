#![forbid(unsafe_code)]

use quanta_index_contract::lex::{
    CommitSha, ExplanationRow, LanguageCode, SymbolKindCode, SymbolKindFamily,
};
use quanta_index_contract::results::{
    EngineTouched, PlannerStage, PlannerTraceEntry, SearchExplanation,
};
use quanta_index_contract::{
    DiffCandidate, DiffHunkSide, ExplainCandidateV1, GenerationPin, HighlightSpan,
    HybridCandidateV1, HybridLaneContributionV1, HybridLaneV1, HybridQueryRequest,
    HybridQueryResponse, HybridSeedQueryRequest, LexicalCandidate, LqQuery, LqSpan,
    ManifestGeneration, QueryConstraintSetV1, QueryResultWindowV1, RepoId, RepoRelativePath,
    RevisionId, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcResponse, SemanticCorpusKindV1, SemanticQueryRequest, SemanticQueryResponse,
    SemanticSeedCorpusBudgetV1, StructuralQueryRequest, SymbolCandidate, TextQueryRequest,
    TextQuerySyntax,
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

fn first_candidate_fields_mut(
    results: &mut ciborium::Value,
) -> Result<&mut Vec<(ciborium::Value, ciborium::Value)>, Box<dyn std::error::Error>> {
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
        .ok_or_else(|| "expected first lexical result".to_string())?;
    map_fields_mut(first)
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
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(generation_pin()),
        generation_selector: None,
        top_k: 50,
    }
}

fn semantic_scope() -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "repo:quanta-index lang:rust SearchPlane".to_owned(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
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
        // J7Q-07: a concrete hit offset + span so the wire round-trip proves the
        // new UI highlight-anchor fields survive serialize -> deserialize.
        snippet_hit_offset: Some(3),
        highlights: vec![HighlightSpan { start: 3, len: 4 }],
    }
}

/// A hybrid row both lanes saw: the lexical lane's row (so its score is the
/// BM25 score) at lexical rank 1 and dense rank 2.
fn hybrid_candidate() -> HybridCandidateV1 {
    let candidate = lexical_candidate();
    HybridCandidateV1 {
        fused_score: 1.0 / 61.0 + 1.0 / 62.0,
        contributions: vec![
            HybridLaneContributionV1 {
                lane: HybridLaneV1::Lexical,
                rank: 1,
                raw_score: candidate.score,
            },
            HybridLaneContributionV1 {
                lane: HybridLaneV1::Dense,
                rank: 2,
                raw_score: 0.5,
            },
        ],
        candidate,
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
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
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
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
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

fn hybrid_seed_request_with_corpus_budgets() -> HybridSeedQueryRequest {
    HybridSeedQueryRequest {
        text_query: lexical_request(),
        semantic_query_text: "semantic seed".to_string(),
        generation: Some(generation_pin()),
        generation_selector: None,
        dense_corpora: vec![
            SemanticSeedCorpusBudgetV1 {
                corpus_kind: SemanticCorpusKindV1::SymbolCard,
                top_k: 40,
            },
            SemanticSeedCorpusBudgetV1 {
                corpus_kind: SemanticCorpusKindV1::ModuleCard,
                top_k: 20,
            },
        ],
        top_k: 15,
    }
}

fn sourcegraph_text_request() -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "repo:quanta-index lang:rust SearchPlane".to_owned(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
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
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
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
fn hybrid_seed_corpus_budgets_round_trip_losslessly() -> TestRes {
    let request = hybrid_seed_request_with_corpus_budgets();
    let decoded: HybridSeedQueryRequest = decode(&encode(&request)?)?;
    if decoded != request {
        return Err(format!("hybrid seed corpus budget round-trip mismatch: {decoded:?}").into());
    }
    Ok(())
}

#[test]
fn hybrid_seed_legacy_wire_defaults_missing_corpus_budgets_to_global_lane() -> TestRes {
    let request = hybrid_seed_request_with_corpus_budgets();
    let mut value = serde_json::to_value(request)?;
    let fields = value
        .as_object_mut()
        .ok_or_else(|| "expected hybrid seed request object".to_string())?;
    if fields.remove("dense_corpora").is_none() {
        return Err("dense_corpora field missing from new wire".into());
    }
    let decoded: HybridSeedQueryRequest = serde_json::from_value(value)?;
    if !decoded.dense_corpora.is_empty() {
        return Err("legacy wire must decode to the migration global dense lane".into());
    }
    Ok(())
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
fn query_constraint_wire_is_order_invariant_and_deduplicated() -> TestRes {
    let rust = LanguageCode::new("rust").map_err(str::to_string)?;
    let python = LanguageCode::new("python").map_err(str::to_string)?;
    let mut left = sourcegraph_text_request();
    left.constraints =
        QueryConstraintSetV1::from_languages([rust.clone(), python.clone(), rust.clone()]);
    let mut right = sourcegraph_text_request();
    right.constraints = QueryConstraintSetV1::from_languages([python, rust]);
    if encode(&left)? != encode(&right)? {
        return Err(
            "query constraint wire encoding must be order-invariant and deduplicated".into(),
        );
    }
    Ok(())
}

#[test]
fn text_request_rejects_missing_mandatory_constraints() -> TestRes {
    let bytes = mutate_ipc_request_wire(
        &SearchPlaneQueryIpcRequest::Text(sourcegraph_text_request()),
        |wire| {
            let request_fields = map_fields_mut(wire)?;
            let payload = field_value_mut(request_fields, "payload")?;
            let payload_fields = map_fields_mut(payload)?;
            payload_fields.retain(
                |(key, _)| !matches!(key, ciborium::Value::Text(name) if name == "constraints"),
            );
            Ok(())
        },
    )?;
    expect_decode_error_contains::<SearchPlaneQueryIpcRequest>(&bytes, "constraints")
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
        window: QueryResultWindowV1::exact(1),
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
        results: vec![hybrid_candidate()],
        window: QueryResultWindowV1::exact(1),
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
            window: QueryResultWindowV1::exact(1),
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
        window: QueryResultWindowV1::exact(1),
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

fn history_commit_candidate() -> quanta_index_contract::CommitCandidate {
    quanta_index_contract::CommitCandidate {
        sha: CommitSha::from_bytes([1u8; 20]),
        parent_ids: vec![CommitSha::from_bytes([2u8; 20])],
        committed_at_unix_s: 1_717_171_717,
        author: "alice@example.com".to_owned(),
        committer: "bob@example.com".to_owned(),
        message: "bridge request landed".to_owned(),
        is_merge: false,
        tags: vec!["v2".to_owned()],
    }
}

fn history_diff_candidate() -> DiffCandidate {
    DiffCandidate {
        repo_relative_path: "src/search.rs".to_owned(),
        hunk_header: "@@ -10,4 +10,7 @@".to_owned(),
        side: DiffHunkSide::After,
        line_start: 10,
        line_end: 16,
        snippet: "+ bridge_search(query);".to_owned(),
    }
}

/// A commit page with a continuation and a final diff page both round-trip
/// (QI-BB-023): the window counts the page's rows, `has_more` and
/// `next_cursor` agree, and a diff cursor carries its path.
#[test]
fn search_plane_ipc_response_v2_history_variant_roundtrips() -> TestRes {
    let commit_page = SearchPlaneQueryIpcResponse::History(
        quanta_index_contract::SearchPlaneHistoryQueryResponse {
            generation: generation_pin(),
            commits: vec![history_commit_candidate()],
            diffs: Vec::new(),
            window: QueryResultWindowV1::new(
                1,
                quanta_index_contract::CandidateCountV1::Exact(3),
                true,
            )?,
            examined: 7,
            next_cursor: Some(quanta_index_contract::HistoryCursor {
                committer_time_ms: 1_717_171_717_000,
                sha: CommitSha::from_bytes([1u8; 20]),
                file_path: None,
            }),
        },
    );
    roundtrip_eq(&commit_page)?;
    let diff_page = SearchPlaneQueryIpcResponse::History(
        quanta_index_contract::SearchPlaneHistoryQueryResponse {
            generation: generation_pin(),
            commits: Vec::new(),
            diffs: vec![history_diff_candidate()],
            window: QueryResultWindowV1::exact(1),
            examined: 1,
            next_cursor: None,
        },
    );
    roundtrip_eq(&diff_page)
}

/// A history page whose window, rows and cursor disagree fails to decode
/// (QI-BB-023): a page cannot claim a continuation it does not position,
/// carry both row kinds, or count rows it does not hold.
#[test]
fn search_plane_ipc_response_v2_history_page_rejects_inconsistent_shapes() -> TestRes {
    let generation = generation_pin();
    let cursor = quanta_index_contract::HistoryCursor {
        committer_time_ms: 5,
        sha: CommitSha::from_bytes([1u8; 20]),
        file_path: None,
    };
    let cases: Vec<(&str, quanta_index_contract::SearchPlaneHistoryQueryResponse)> = vec![
        (
            "has_more without a cursor",
            quanta_index_contract::SearchPlaneHistoryQueryResponse {
                generation: generation.clone(),
                commits: vec![history_commit_candidate()],
                diffs: Vec::new(),
                window: QueryResultWindowV1::new(
                    1,
                    quanta_index_contract::CandidateCountV1::Exact(3),
                    true,
                )?,
                examined: 3,
                next_cursor: None,
            },
        ),
        (
            "a cursor without has_more",
            quanta_index_contract::SearchPlaneHistoryQueryResponse {
                generation: generation.clone(),
                commits: vec![history_commit_candidate()],
                diffs: Vec::new(),
                window: QueryResultWindowV1::exact(1),
                examined: 1,
                next_cursor: Some(cursor),
            },
        ),
        (
            "both row kinds",
            quanta_index_contract::SearchPlaneHistoryQueryResponse {
                generation: generation.clone(),
                commits: vec![history_commit_candidate()],
                diffs: vec![history_diff_candidate()],
                window: QueryResultWindowV1::exact(2),
                examined: 2,
                next_cursor: None,
            },
        ),
        (
            "a window that does not count the rows",
            quanta_index_contract::SearchPlaneHistoryQueryResponse {
                generation,
                commits: vec![history_commit_candidate()],
                diffs: Vec::new(),
                window: QueryResultWindowV1::exact(2),
                examined: 2,
                next_cursor: None,
            },
        ),
    ];
    for (label, page) in cases {
        let bytes = encode(&SearchPlaneQueryIpcResponse::History(page))?;
        if decode::<SearchPlaneQueryIpcResponse>(&bytes).is_ok() {
            return Err(format!("{label}: an inconsistent history page must not decode").into());
        }
    }
    Ok(())
}

#[test]
fn search_plane_ipc_response_v2_lexical_rejects_duplicate_results() -> TestRes {
    let response = SearchPlaneQueryIpcResponse::Text(quanta_index_contract::TextQueryResponse {
        generation: generation_pin(),
        results: vec![lexical_candidate()],
        window: QueryResultWindowV1::exact(1),
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
fn text_response_rejects_missing_or_contradictory_window() -> TestRes {
    let response = text_response_with_lexical_candidate();
    let missing = mutate_ipc_response_wire(&response, |wire| {
        let response_fields = map_fields_mut(wire)?;
        let payload = field_value_mut(response_fields, "payload")?;
        let payload_fields = map_fields_mut(payload)?;
        payload_fields
            .retain(|(key, _)| !matches!(key, ciborium::Value::Text(name) if name == "window"));
        Ok(())
    })?;
    expect_decode_error_contains::<SearchPlaneQueryIpcResponse>(&missing, "window")?;

    let contradictory = mutate_ipc_response_wire(&response, |wire| {
        let response_fields = map_fields_mut(wire)?;
        let payload = field_value_mut(response_fields, "payload")?;
        let payload_fields = map_fields_mut(payload)?;
        let window = field_value_mut(payload_fields, "window")?;
        let window_fields = map_fields_mut(window)?;
        let has_more = field_value_mut(window_fields, "has_more")?;
        *has_more = ciborium::Value::Bool(true);
        Ok(())
    })?;
    expect_decode_error_contains::<SearchPlaneQueryIpcResponse>(&contradictory, "contradict")
}

#[test]
fn search_plane_ipc_response_v2_roundtrips_file_owner_projection_rows() -> TestRes {
    let response = SearchPlaneQueryIpcResponse::Text(quanta_index_contract::TextQueryResponse {
        generation: generation_pin(),
        results: vec![lexical_candidate()],
        window: QueryResultWindowV1::exact(1),
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
            window: QueryResultWindowV1::exact(1),
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
        window: QueryResultWindowV1::exact(1),
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
        results: vec![hybrid_candidate()],
        window: QueryResultWindowV1::exact(1),
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

fn text_response_with_lexical_candidate() -> SearchPlaneQueryIpcResponse {
    SearchPlaneQueryIpcResponse::Text(quanta_index_contract::TextQueryResponse {
        generation: generation_pin(),
        results: vec![lexical_candidate()],
        window: QueryResultWindowV1::exact(1),
        file_owner_rows: None,
    })
}

#[test]
fn search_plane_ipc_response_v2_lexical_rejects_duplicate_snippet_hit_offset() -> TestRes {
    let bytes = mutate_ipc_response_wire(&text_response_with_lexical_candidate(), |wire| {
        let response_fields = map_fields_mut(wire)?;
        let payload = field_value_mut(response_fields, "payload")?;
        let payload_fields = map_fields_mut(payload)?;
        let results = field_value_mut(payload_fields, "results")?;
        let candidate_fields = first_candidate_fields_mut(results)?;
        duplicate_text_field(candidate_fields, "snippet_hit_offset")?;
        Ok(())
    })?;

    expect_decode_error_contains::<SearchPlaneQueryIpcResponse>(&bytes, "snippet_hit_offset")
}

#[test]
fn search_plane_ipc_response_v2_lexical_rejects_missing_snippet_hit_offset() -> TestRes {
    let bytes = mutate_ipc_response_wire(&text_response_with_lexical_candidate(), |wire| {
        let response_fields = map_fields_mut(wire)?;
        let payload = field_value_mut(response_fields, "payload")?;
        let payload_fields = map_fields_mut(payload)?;
        let results = field_value_mut(payload_fields, "results")?;
        let candidate_fields = first_candidate_fields_mut(results)?;
        candidate_fields.retain(
            |(key, _value)| !matches!(key, ciborium::Value::Text(text) if text == "snippet_hit_offset"),
        );
        Ok(())
    })?;

    expect_decode_error_contains::<SearchPlaneQueryIpcResponse>(&bytes, "snippet_hit_offset")
}

#[test]
fn search_plane_ipc_response_v2_lexical_rejects_duplicate_highlights() -> TestRes {
    let bytes = mutate_ipc_response_wire(&text_response_with_lexical_candidate(), |wire| {
        let response_fields = map_fields_mut(wire)?;
        let payload = field_value_mut(response_fields, "payload")?;
        let payload_fields = map_fields_mut(payload)?;
        let results = field_value_mut(payload_fields, "results")?;
        let candidate_fields = first_candidate_fields_mut(results)?;
        duplicate_text_field(candidate_fields, "highlights")?;
        Ok(())
    })?;

    expect_decode_error_contains::<SearchPlaneQueryIpcResponse>(&bytes, "highlights")
}

#[test]
fn search_plane_ipc_response_v2_lexical_rejects_missing_highlights() -> TestRes {
    let bytes = mutate_ipc_response_wire(&text_response_with_lexical_candidate(), |wire| {
        let response_fields = map_fields_mut(wire)?;
        let payload = field_value_mut(response_fields, "payload")?;
        let payload_fields = map_fields_mut(payload)?;
        let results = field_value_mut(payload_fields, "results")?;
        let candidate_fields = first_candidate_fields_mut(results)?;
        candidate_fields.retain(
            |(key, _value)| !matches!(key, ciborium::Value::Text(text) if text == "highlights"),
        );
        Ok(())
    })?;

    expect_decode_error_contains::<SearchPlaneQueryIpcResponse>(&bytes, "highlights")
}

// QI-BB-022: the explain request carries the query it explains under (or
// none), and the response carries a typed presence beside the explanation.
#[test]
fn explain_request_round_trips_with_and_without_its_query() -> TestRes {
    use quanta_index_contract::SearchPlaneExplainQueryRequest;
    let presence_only = SearchPlaneExplainQueryRequest {
        generation: generation_pin(),
        candidate: ExplainCandidateV1::Lexical(lexical_candidate()),
        text_query: None,
    };
    roundtrip_eq(&SearchPlaneQueryIpcRequest::Explain(presence_only))?;
    let scored = SearchPlaneExplainQueryRequest {
        generation: generation_pin(),
        candidate: ExplainCandidateV1::Lexical(lexical_candidate()),
        text_query: Some(sourcegraph_text_request()),
    };
    roundtrip_eq(&SearchPlaneQueryIpcRequest::Explain(scored))?;
    // QI-BB-022: a hybrid row explains with its lane provenance, and only
    // under the query it was fused for.
    let hybrid = SearchPlaneExplainQueryRequest {
        generation: generation_pin(),
        candidate: ExplainCandidateV1::Hybrid(hybrid_candidate()),
        text_query: Some(sourcegraph_text_request()),
    };
    roundtrip_eq(&SearchPlaneQueryIpcRequest::Explain(hybrid.clone()))?;
    let bytes = mutate_ipc_request_wire(&SearchPlaneQueryIpcRequest::Explain(hybrid), |wire| {
        let request_fields = map_fields_mut(wire)?;
        let payload = field_value_mut(request_fields, "payload")?;
        let payload_fields = map_fields_mut(payload)?;
        payload_fields
            .retain(|(key, _)| !matches!(key, ciborium::Value::Text(name) if name == "text_query"));
        Ok(())
    })?;
    expect_decode_error_contains::<SearchPlaneQueryIpcRequest>(&bytes, "text_query is required")?;
    let unqueried = SearchPlaneExplainQueryRequest {
        generation: generation_pin(),
        candidate: ExplainCandidateV1::Hybrid(hybrid_candidate()),
        text_query: None,
    };
    if encode(&SearchPlaneQueryIpcRequest::Explain(unqueried)).is_ok() {
        return Err("a hybrid candidate without its query must not encode".into());
    }
    Ok(())
}

// The explain candidate is adjacently tagged, kind before payload, and the
// kind is closed.
#[test]
fn explain_candidate_refuses_payload_before_kind_and_unknown_kinds() -> TestRes {
    let tagged: serde_json::Value =
        serde_json::to_value(ExplainCandidateV1::Lexical(lexical_candidate()))?;
    let serde_json::Value::Object(fields) = &tagged else {
        return Err("an explain candidate encodes as a map".into());
    };
    if fields.keys().collect::<Vec<_>>() != ["kind", "payload"] {
        return Err(format!("kind must precede payload: {tagged}").into());
    }
    let payload = fields.get("payload").ok_or("payload present")?;
    let payload_first = format!("{{\"payload\":{payload},\"kind\":\"Lexical\"}}");
    expect_json_refusal::<ExplainCandidateV1>(&payload_first, "payload arrived before kind")?;
    let unknown_kind = format!("{{\"kind\":\"Symbol\",\"payload\":{payload}}}");
    expect_json_refusal::<ExplainCandidateV1>(&unknown_kind, "unknown variant")?;
    let kind_only = "{\"kind\":\"Symbol\"}";
    expect_json_refusal::<ExplainCandidateV1>(kind_only, "unknown variant")?;
    let payload_only = format!("{{\"payload\":{payload}}}");
    expect_json_refusal::<ExplainCandidateV1>(&payload_only, "payload arrived before kind")
}

/// Decode `json` as `T` and require a refusal naming `expected_fragment`.
fn expect_json_refusal<T>(json: &str, expected_fragment: &str) -> TestRes
where
    T: serde::de::DeserializeOwned + core::fmt::Debug,
{
    match serde_json::from_str::<T>(json) {
        Ok(decoded) => Err(format!("decode unexpectedly succeeded: {decoded:?}").into()),
        Err(err) if err.to_string().contains(expected_fragment) => Ok(()),
        Err(err) => {
            Err(format!("decode error did not mention `{expected_fragment}`: {err}").into())
        }
    }
}

#[test]
fn explain_response_round_trips_its_presence_and_rejects_a_missing_one() -> TestRes {
    use quanta_index_contract::{CandidatePresenceV1, SearchPlaneExplainQueryResponse};
    for presence in [
        CandidatePresenceV1::Indexed,
        CandidatePresenceV1::NotIndexed,
    ] {
        roundtrip_eq(&SearchPlaneQueryIpcResponse::Explain(
            SearchPlaneExplainQueryResponse {
                generation: generation_pin(),
                presence,
                explanation: explanation_v2(),
            },
        ))?;
    }
    let bytes = mutate_ipc_response_wire(
        &SearchPlaneQueryIpcResponse::Explain(SearchPlaneExplainQueryResponse {
            generation: generation_pin(),
            presence: CandidatePresenceV1::Indexed,
            explanation: explanation_v2(),
        }),
        |wire| {
            let response_fields = map_fields_mut(wire)?;
            let payload = field_value_mut(response_fields, "payload")?;
            let payload_fields = map_fields_mut(payload)?;
            payload_fields.retain(
                |(key, _)| !matches!(key, ciborium::Value::Text(name) if name == "presence"),
            );
            Ok(())
        },
    )?;
    expect_decode_error_contains::<SearchPlaneQueryIpcResponse>(&bytes, "presence")?;
    let bytes = mutate_ipc_response_wire(
        &SearchPlaneQueryIpcResponse::Explain(SearchPlaneExplainQueryResponse {
            generation: generation_pin(),
            presence: CandidatePresenceV1::Indexed,
            explanation: explanation_v2(),
        }),
        |wire| {
            let response_fields = map_fields_mut(wire)?;
            let payload = field_value_mut(response_fields, "payload")?;
            let payload_fields = map_fields_mut(payload)?;
            *field_value_mut(payload_fields, "presence")? =
                ciborium::Value::Text("maybe".to_owned());
            Ok(())
        },
    )?;
    expect_decode_error_contains::<SearchPlaneQueryIpcResponse>(&bytes, "maybe")
}
