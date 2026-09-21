#![forbid(unsafe_code)]

use quanta_index_contract::lex::{
    CommitSha, ExplanationRow, LanguageCode, SymbolKindCode, SymbolKindFamily,
};
use quanta_index_contract::results::{
    EngineTouched, PlannerStage, PlannerTraceEntry, SearchExplanation,
};
use quanta_index_contract::{
    AuxEpochV1, DiffCandidate, DiffHunkSide, ExplainCandidateV1, GenerationPin, HighlightSpan,
    HybridCandidateV1, HybridLaneContributionV1, HybridLaneV1, HybridQueryRequest,
    HybridQueryResponse, HybridSeedQueryRequest, LexicalCandidate, LqQuery, LqSpan,
    ManifestGeneration, QueryConstraintSetV1, QueryResultWindowV1, QueryResultWindowV2, RepoId,
    RepoRelativePath, RevisionId, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope,
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

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn generation_pin() -> GenerationPin {
    GenerationPin::new(
        RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("rev-1").expect("static fixture ID satisfies canonical policy"),
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
        cursor: None,
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
        cursor: None,
    }
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn lexical_candidate() -> LexicalCandidate {
    LexicalCandidate {
        candidate_id: "cand-1".to_owned(),
        repo_id: RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-1")
            .expect("static fixture ID satisfies canonical policy"),
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
        repo_id: RepoId::new("repo-1")?,
        revision_id: RevisionId::new("rev-1")?,
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
        cursor: None,
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
            cursor: None,
        },
        cursor: None,
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
        window_v2: QueryResultWindowV2::exact_probe(1),
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
        window_v2: QueryResultWindowV2::exact_probe(1),
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
            next_cursor: None,
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
        next_cursor: None,
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
        score: None,
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
        score: None,
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
            order: quanta_index_contract::HistoryOrderV1::Recency,
            commits: vec![history_commit_candidate()],
            diffs: Vec::new(),
            window: QueryResultWindowV1::new(
                1,
                quanta_index_contract::CandidateCountV1::Exact(3),
                true,
            )?,
            read_epoch: quanta_index_contract::AuxEpochV1::new(9),
            examined: 7,
            next_cursor: Some(quanta_index_contract::HistoryCursor {
                order: quanta_index_contract::HistoryCursorOrderV1::Recency,
                committer_time_ms: 1_717_171_717_000,
                sha: CommitSha::from_bytes([1u8; 20]),
                file_path: None,
                aux_epoch: quanta_index_contract::AuxEpochV1::new(9),
            }),
        },
    );
    roundtrip_eq(&commit_page)?;
    let diff_page = SearchPlaneQueryIpcResponse::History(
        quanta_index_contract::SearchPlaneHistoryQueryResponse {
            generation: generation_pin(),
            order: quanta_index_contract::HistoryOrderV1::Recency,
            commits: Vec::new(),
            diffs: vec![history_diff_candidate()],
            window: QueryResultWindowV1::exact(1),
            read_epoch: quanta_index_contract::AuxEpochV1::GENESIS,
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
    let read_epoch = quanta_index_contract::AuxEpochV1::new(4);
    let cursor = quanta_index_contract::HistoryCursor {
        order: quanta_index_contract::HistoryCursorOrderV1::Recency,
        committer_time_ms: 5,
        sha: CommitSha::from_bytes([1u8; 20]),
        file_path: None,
        aux_epoch: read_epoch,
    };
    let cases: Vec<(&str, quanta_index_contract::SearchPlaneHistoryQueryResponse)> = vec![
        (
            "has_more without a cursor",
            quanta_index_contract::SearchPlaneHistoryQueryResponse {
                generation: generation.clone(),
                order: quanta_index_contract::HistoryOrderV1::Recency,
                commits: vec![history_commit_candidate()],
                diffs: Vec::new(),
                window: QueryResultWindowV1::new(
                    1,
                    quanta_index_contract::CandidateCountV1::Exact(3),
                    true,
                )?,
                read_epoch,
                examined: 3,
                next_cursor: None,
            },
        ),
        (
            "a cursor without has_more",
            quanta_index_contract::SearchPlaneHistoryQueryResponse {
                generation: generation.clone(),
                order: quanta_index_contract::HistoryOrderV1::Recency,
                commits: vec![history_commit_candidate()],
                diffs: Vec::new(),
                window: QueryResultWindowV1::exact(1),
                read_epoch,
                examined: 1,
                next_cursor: Some(cursor.clone()),
            },
        ),
        (
            "both row kinds",
            quanta_index_contract::SearchPlaneHistoryQueryResponse {
                generation: generation.clone(),
                order: quanta_index_contract::HistoryOrderV1::Recency,
                commits: vec![history_commit_candidate()],
                diffs: vec![history_diff_candidate()],
                window: QueryResultWindowV1::exact(2),
                read_epoch,
                examined: 2,
                next_cursor: None,
            },
        ),
        (
            "a window that does not count the rows",
            quanta_index_contract::SearchPlaneHistoryQueryResponse {
                generation: generation.clone(),
                order: quanta_index_contract::HistoryOrderV1::Recency,
                commits: vec![history_commit_candidate()],
                diffs: Vec::new(),
                window: QueryResultWindowV1::exact(2),
                read_epoch,
                examined: 2,
                next_cursor: None,
            },
        ),
        (
            "a cursor naming another epoch than the page read",
            quanta_index_contract::SearchPlaneHistoryQueryResponse {
                generation,
                order: quanta_index_contract::HistoryOrderV1::Recency,
                commits: vec![history_commit_candidate()],
                diffs: Vec::new(),
                window: QueryResultWindowV1::new(
                    1,
                    quanta_index_contract::CandidateCountV1::Exact(3),
                    true,
                )?,
                read_epoch: quanta_index_contract::AuxEpochV1::new(5),
                examined: 3,
                next_cursor: Some(cursor),
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

fn history_score(
    value: f32,
) -> Result<quanta_index_contract::HistoryScoreV1, Box<dyn std::error::Error>> {
    Ok(quanta_index_contract::HistoryScoreV1::try_new(value)?)
}

/// QI-BB-023 follow-up #1 — the relevance order on the wire.
///
/// A relevance page carries a score on every row and a relevance cursor
/// that carries the last row's score; both round-trip bit for bit. The
/// request says its order and has no default.
#[test]
fn search_plane_ipc_v2_history_relevance_pages_and_requests_round_trip() -> TestRes {
    use quanta_index_contract::{
        HistoryCursor, HistoryCursorOrderV1, HistoryOrderV1, HistoryQueryRequest,
        SearchPlaneHistoryQueryResponse, SearchPlaneQueryIpcRequest,
    };

    let top = history_score(2.625)?;
    let last = history_score(1.000_000_1)?;
    let mut first = history_commit_candidate();
    first.score = Some(top);
    let mut second = history_commit_candidate();
    second.sha = CommitSha::from_bytes([7u8; 20]);
    second.score = Some(last);
    let page = SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
        generation: generation_pin(),
        order: HistoryOrderV1::Relevance,
        commits: vec![first, second],
        diffs: Vec::new(),
        window: QueryResultWindowV1::new(
            2,
            quanta_index_contract::CandidateCountV1::Exact(9),
            true,
        )?,
        read_epoch: quanta_index_contract::AuxEpochV1::new(3),
        examined: 20,
        next_cursor: Some(HistoryCursor {
            order: HistoryCursorOrderV1::Relevance { score: last },
            committer_time_ms: 1_717_171_717_000,
            sha: CommitSha::from_bytes([7u8; 20]),
            file_path: None,
            aux_epoch: quanta_index_contract::AuxEpochV1::new(3),
        }),
    });
    roundtrip_eq(&page)?;
    let decoded: SearchPlaneQueryIpcResponse = decode(&encode(&page)?)?;
    let SearchPlaneQueryIpcResponse::History(decoded) = decoded else {
        return Err("a history page decodes as one".into());
    };
    let Some(HistoryCursor {
        order: HistoryCursorOrderV1::Relevance { score },
        ..
    }) = decoded.next_cursor
    else {
        return Err("the cursor carries its score".into());
    };
    if score.get().to_bits() != last.get().to_bits() {
        return Err("the cursor's score round-trips bit for bit".into());
    }
    let mut diff = history_diff_candidate();
    diff.score = Some(top);
    let diff_page = SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
        generation: generation_pin(),
        order: HistoryOrderV1::Relevance,
        commits: Vec::new(),
        diffs: vec![diff],
        window: QueryResultWindowV1::exact(1),
        read_epoch: quanta_index_contract::AuxEpochV1::new(3),
        examined: 1,
        next_cursor: None,
    });
    roundtrip_eq(&diff_page)?;

    for order in HistoryOrderV1::ALL {
        let request = SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
            text_query: quanta_index_contract::TextQueryRequest {
                syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
                query_text: "type:commit needle".to_owned(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(generation_pin()),
                generation_selector: None,
                top_k: 5,
                cursor: None,
            },
            order,
            cursor: None,
        });
        roundtrip_eq(&request)?;
        let mut value = serde_json::to_value(&request)?;
        let payload = value
            .get_mut("payload")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("the request is a tagged map")?;
        if payload.remove("order").is_none() {
            return Err("the request serializes its order".into());
        }
        if serde_json::from_value::<SearchPlaneQueryIpcRequest>(value).is_ok() {
            return Err("a history request without an order must not decode".into());
        }
    }
    Ok(())
}

/// A page whose rows, order and cursor disagree fails to decode: the
/// score is present exactly under relevance, and a cursor continues the
/// order of the page that issued it.
#[test]
fn search_plane_ipc_v2_history_refuses_order_and_score_disagreements() -> TestRes {
    use quanta_index_contract::{
        HistoryCursor, HistoryCursorOrderV1, HistoryOrderV1, SearchPlaneHistoryQueryResponse,
    };

    let score = history_score(1.5)?;
    let mut scored = history_commit_candidate();
    scored.score = Some(score);
    let page = |order: HistoryOrderV1,
                commit: quanta_index_contract::CommitCandidate,
                cursor: Option<HistoryCursor>|
     -> Result<SearchPlaneHistoryQueryResponse, Box<dyn std::error::Error>> {
        let has_more = cursor.is_some();
        Ok(SearchPlaneHistoryQueryResponse {
            generation: generation_pin(),
            order,
            commits: vec![commit],
            diffs: Vec::new(),
            window: QueryResultWindowV1::new(
                1,
                quanta_index_contract::CandidateCountV1::Exact(if has_more { 3 } else { 1 }),
                has_more,
            )?,
            read_epoch: quanta_index_contract::AuxEpochV1::new(2),
            examined: 3,
            next_cursor: cursor,
        })
    };
    let recency_cursor = HistoryCursor {
        order: HistoryCursorOrderV1::Recency,
        committer_time_ms: 5,
        sha: CommitSha::from_bytes([1u8; 20]),
        file_path: None,
        aux_epoch: quanta_index_contract::AuxEpochV1::new(2),
    };
    let relevance_cursor = HistoryCursor {
        order: HistoryCursorOrderV1::Relevance { score },
        ..recency_cursor.clone()
    };
    let cases: Vec<(&str, SearchPlaneHistoryQueryResponse)> = vec![
        (
            "a relevance page with an unscored row",
            page(HistoryOrderV1::Relevance, history_commit_candidate(), None)?,
        ),
        (
            "a recency page with a scored row",
            page(HistoryOrderV1::Recency, scored.clone(), None)?,
        ),
        (
            "a recency page continued by a relevance cursor",
            page(
                HistoryOrderV1::Recency,
                history_commit_candidate(),
                Some(relevance_cursor.clone()),
            )?,
        ),
        (
            "a relevance page continued by a recency cursor",
            page(HistoryOrderV1::Relevance, scored, Some(recency_cursor))?,
        ),
    ];
    for (label, page) in cases {
        let bytes = encode(&SearchPlaneQueryIpcResponse::History(page))?;
        if decode::<SearchPlaneQueryIpcResponse>(&bytes).is_ok() {
            return Err(format!("{label}: must not decode").into());
        }
    }

    // A cursor's score is present exactly under relevance, and finite.
    let mut with_score = serde_json::to_value(&relevance_cursor)?;
    let map = with_score.as_object_mut().ok_or("a cursor is a map")?;
    let _order = map.insert("order".to_owned(), serde_json::json!("recency"));
    if serde_json::from_value::<HistoryCursor>(with_score.clone()).is_ok() {
        return Err("a recency cursor with a score must not decode".into());
    }
    let mut without_score = serde_json::to_value(&relevance_cursor)?;
    let map = without_score.as_object_mut().ok_or("a cursor is a map")?;
    if map.remove("score").is_none() {
        return Err("a relevance cursor serializes its score".into());
    }
    if serde_json::from_value::<HistoryCursor>(without_score).is_ok() {
        return Err("a relevance cursor without a score must not decode".into());
    }
    let mut unknown_order = serde_json::to_value(&relevance_cursor)?;
    let map = unknown_order.as_object_mut().ok_or("a cursor is a map")?;
    let _order = map.insert("order".to_owned(), serde_json::json!("newest"));
    if serde_json::from_value::<HistoryCursor>(unknown_order).is_ok() {
        return Err("a cursor with an unknown order must not decode".into());
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
        next_cursor: None,
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
            repo_id: RepoId::new("repo-a").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-a")
                .expect("static fixture ID satisfies canonical policy"),
            manifest_generation: ManifestGeneration::new(7),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            owners: vec!["@alice".to_string(), "@acme/platform".to_string()],
        }]),
        next_cursor: None,
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
            next_cursor: None,
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
        window_v2: QueryResultWindowV2::exact_probe(1),
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
        window_v2: QueryResultWindowV2::exact_probe(1),
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
        next_cursor: None,
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
// none), the dense query for a hybrid row, and the response carries a typed
// presence beside the explanation.
#[test]
fn explain_request_round_trips_with_and_without_its_query() -> TestRes {
    use quanta_index_contract::SearchPlaneExplainQueryRequest;
    let presence_only = SearchPlaneExplainQueryRequest {
        generation: generation_pin(),
        candidate: ExplainCandidateV1::Lexical(lexical_candidate()),
        text_query: None,
        semantic_query_text: None,
    };
    roundtrip_eq(&SearchPlaneQueryIpcRequest::Explain(presence_only))?;
    let scored = SearchPlaneExplainQueryRequest {
        generation: generation_pin(),
        candidate: ExplainCandidateV1::Lexical(lexical_candidate()),
        text_query: Some(sourcegraph_text_request()),
        semantic_query_text: None,
    };
    roundtrip_eq(&SearchPlaneQueryIpcRequest::Explain(scored))?;
    // QI-BB-022: a hybrid row explains with its lane provenance, and only
    // under both queries it was fused for.
    let hybrid = SearchPlaneExplainQueryRequest {
        generation: generation_pin(),
        candidate: ExplainCandidateV1::Hybrid(hybrid_candidate()),
        text_query: Some(sourcegraph_text_request()),
        semantic_query_text: Some("needle".to_string()),
    };
    roundtrip_eq(&SearchPlaneQueryIpcRequest::Explain(hybrid.clone()))?;
    for (dropped, fragment) in [
        ("text_query", "text_query is required"),
        ("semantic_query_text", "semantic_query_text is required"),
    ] {
        let bytes = mutate_ipc_request_wire(
            &SearchPlaneQueryIpcRequest::Explain(hybrid.clone()),
            |wire| {
                let request_fields = map_fields_mut(wire)?;
                let payload = field_value_mut(request_fields, "payload")?;
                let payload_fields = map_fields_mut(payload)?;
                payload_fields.retain(
                    |(key, _)| !matches!(key, ciborium::Value::Text(name) if name == dropped),
                );
                Ok(())
            },
        )?;
        expect_decode_error_contains::<SearchPlaneQueryIpcRequest>(&bytes, fragment)?;
    }
    let unqueried = SearchPlaneExplainQueryRequest {
        generation: generation_pin(),
        candidate: ExplainCandidateV1::Hybrid(hybrid_candidate()),
        text_query: None,
        semantic_query_text: None,
    };
    if encode(&SearchPlaneQueryIpcRequest::Explain(unqueried)).is_ok() {
        return Err("a hybrid candidate without its query must not encode".into());
    }
    let half_queried = SearchPlaneExplainQueryRequest {
        generation: generation_pin(),
        candidate: ExplainCandidateV1::Hybrid(hybrid_candidate()),
        text_query: Some(sourcegraph_text_request()),
        semantic_query_text: None,
    };
    if encode(&SearchPlaneQueryIpcRequest::Explain(half_queried)).is_ok() {
        return Err("a hybrid candidate without its dense query must not encode".into());
    }
    // A lexical row has no dense lane: a dense query beside it is refused
    // on encode and on decode.
    let lexical_with_dense = SearchPlaneExplainQueryRequest {
        generation: generation_pin(),
        candidate: ExplainCandidateV1::Lexical(lexical_candidate()),
        text_query: Some(sourcegraph_text_request()),
        semantic_query_text: Some("needle".to_string()),
    };
    if encode(&SearchPlaneQueryIpcRequest::Explain(lexical_with_dense)).is_ok() {
        return Err("a lexical candidate with a dense query must not encode".into());
    }
    let bytes = mutate_ipc_request_wire(&SearchPlaneQueryIpcRequest::Explain(hybrid), |wire| {
        let request_fields = map_fields_mut(wire)?;
        let payload = field_value_mut(request_fields, "payload")?;
        let payload_fields = map_fields_mut(payload)?;
        let candidate = field_value_mut(payload_fields, "candidate")?;
        let candidate_fields = map_fields_mut(candidate)?;
        for (key, value) in candidate_fields.iter_mut() {
            if matches!(key, ciborium::Value::Text(name) if name == "kind") {
                *value = ciborium::Value::Text("Lexical".to_string());
            }
            if matches!(key, ciborium::Value::Text(name) if name == "payload") {
                let hybrid_fields = map_fields_mut(value)?;
                let lexical_row = field_value_mut(hybrid_fields, "candidate")?.clone();
                *value = lexical_row;
            }
        }
        Ok(())
    })?;
    expect_decode_error_contains::<SearchPlaneQueryIpcRequest>(
        &bytes,
        "semantic_query_text must be absent",
    )?;
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

/// QI-BB-020 W2 — the auxiliary read epoch is required and typed.
///
/// It is part of every history / runtime-metadata / structural page and
/// of the history cursor: it round-trips, a cursor or page without it
/// does not decode, and the runtime-metadata / structural pages carry it
/// beside their window.
#[test]
fn search_plane_ipc_response_v2_aux_read_epoch_is_required_and_round_trips() -> TestRes {
    use quanta_index_contract::{
        HistoryCursor, SearchPlaneHistoryQueryResponse, SearchPlaneRuntimeMetadataQueryResponse,
        SearchPlaneStructuralQueryResponse,
    };

    let cursor = HistoryCursor {
        order: quanta_index_contract::HistoryCursorOrderV1::Recency,
        committer_time_ms: 42,
        sha: CommitSha::from_bytes([3u8; 20]),
        file_path: Some("src/lib.rs".to_string()),
        aux_epoch: AuxEpochV1::new(17),
    };
    roundtrip_eq(&cursor)?;
    let mut without_epoch = serde_json::to_value(&cursor)?;
    if without_epoch
        .as_object_mut()
        .ok_or("cursor is a map")?
        .remove("aux_epoch")
        .is_none()
    {
        return Err("the cursor serializes its epoch".into());
    }
    if serde_json::from_value::<HistoryCursor>(without_epoch).is_ok() {
        return Err("a cursor without an epoch must not decode".into());
    }

    let runtime_page =
        SearchPlaneQueryIpcResponse::RuntimeMetadata(SearchPlaneRuntimeMetadataQueryResponse {
            generation: generation_pin(),
            results: vec![lexical_candidate()],
            window: QueryResultWindowV1::exact(1),
            read_epoch: AuxEpochV1::new(3),
            universe_epoch: AuxEpochV1::new(2),
            examined: 1,
            next_cursor: None,
        });
    roundtrip_eq(&runtime_page)?;
    let structural_page =
        SearchPlaneQueryIpcResponse::Structural(SearchPlaneStructuralQueryResponse {
            generation: generation_pin(),
            results: Vec::new(),
            window: QueryResultWindowV1::exact(0),
            read_epoch: AuxEpochV1::GENESIS,
            examined: 0,
            next_cursor: None,
        });
    roundtrip_eq(&structural_page)?;
    let history_page = SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
        generation: generation_pin(),
        order: quanta_index_contract::HistoryOrderV1::Recency,
        commits: Vec::new(),
        diffs: vec![history_diff_candidate()],
        window: QueryResultWindowV1::exact(1),
        read_epoch: AuxEpochV1::new(17),
        examined: 1,
        next_cursor: None,
    });
    roundtrip_eq(&history_page)?;

    // Without `read_epoch` none of the three pages decodes: a page that
    // does not say which snapshot it read is not a page.
    for (label, page) in [
        ("runtime-metadata", runtime_page),
        ("structural", structural_page),
        ("history", history_page),
    ] {
        let mut value = serde_json::to_value(&page)?;
        let payload = value
            .get_mut("payload")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| format!("{label}: the response is a tagged map"))?;
        if payload.remove("read_epoch").is_none() {
            return Err(format!("{label}: the page serializes its read epoch").into());
        }
        if serde_json::from_value::<SearchPlaneQueryIpcResponse>(value).is_ok() {
            return Err(format!("{label}: a page without a read epoch must not decode").into());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// QI-BB-025 W4 — runtime-metadata / structural keyset cursors and pages.
// ---------------------------------------------------------------------------

fn lexical_candidate_named(candidate_id: &str) -> LexicalCandidate {
    LexicalCandidate {
        candidate_id: candidate_id.to_owned(),
        ..lexical_candidate()
    }
}

fn structural_candidate_named(candidate_id: &str) -> quanta_index_contract::StructuralCandidate {
    quanta_index_contract::StructuralCandidate {
        candidate_id: candidate_id.to_owned(),
        bindings: vec![quanta_index_contract::StructuralBinding {
            metavariable: "name".to_owned(),
            start_byte: 3,
            end_byte: 9,
            start_line: 1,
            end_line: 1,
        }],
    }
}

fn runtime_cursor(candidate_id: &str) -> quanta_index_contract::RuntimeMetadataCursorV1 {
    quanta_index_contract::RuntimeMetadataCursorV1 {
        candidate_id: candidate_id.to_owned(),
        aux_epoch: AuxEpochV1::new(4),
        universe_epoch: AuxEpochV1::new(11),
    }
}

fn structural_cursor(candidate_id: &str) -> quanta_index_contract::StructuralCursorV1 {
    quanta_index_contract::StructuralCursorV1 {
        candidate_id: candidate_id.to_owned(),
        aux_epoch: AuxEpochV1::new(6),
    }
}

/// A well-formed runtime-metadata page: rows ascending by candidate id,
/// a continuation naming the last row in the epochs the page read.
fn runtime_page_with_continuation() -> Result<
    quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse,
    Box<dyn std::error::Error>,
> {
    Ok(
        quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse {
            generation: generation_pin(),
            results: vec![lexical_candidate_named("a"), lexical_candidate_named("b")],
            window: QueryResultWindowV1::new(
                2,
                quanta_index_contract::CandidateCountV1::AtLeast(3),
                true,
            )?,
            read_epoch: AuxEpochV1::new(4),
            universe_epoch: AuxEpochV1::new(11),
            examined: 3,
            next_cursor: Some(runtime_cursor("b")),
        },
    )
}

/// A well-formed structural page with a continuation.
fn structural_page_with_continuation()
-> Result<quanta_index_contract::SearchPlaneStructuralQueryResponse, Box<dyn std::error::Error>> {
    Ok(quanta_index_contract::SearchPlaneStructuralQueryResponse {
        generation: generation_pin(),
        results: vec![
            structural_candidate_named("a"),
            structural_candidate_named("b"),
        ],
        window: QueryResultWindowV1::new(
            2,
            quanta_index_contract::CandidateCountV1::Exact(5),
            true,
        )?,
        read_epoch: AuxEpochV1::new(6),
        examined: 5,
        next_cursor: Some(structural_cursor("b")),
    })
}

/// The two cursors round-trip and decode fail-closed.
///
/// They round-trip through CBOR and JSON and are absent from a request
/// that does not carry them; a missing or duplicated field, an unknown
/// field, or an epoch that is not an unsigned integer is refused.
#[test]
fn keyset_cursors_round_trip_and_decode_fail_closed() -> TestRes {
    roundtrip_eq(&runtime_cursor("chunk://alpha"))?;
    roundtrip_eq(&structural_cursor("chunk://alpha"))?;

    let runtime_json = serde_json::to_value(runtime_cursor("chunk://alpha"))?;
    if runtime_json
        != serde_json::json!({
            "candidate_id": "chunk://alpha",
            "aux_epoch": 4,
            "universe_epoch": 11
        })
    {
        return Err(format!("runtime cursor wire shape drifted: {runtime_json}").into());
    }
    let structural_json = serde_json::to_value(structural_cursor("chunk://alpha"))?;
    if structural_json != serde_json::json!({ "candidate_id": "chunk://alpha", "aux_epoch": 6 }) {
        return Err(format!("structural cursor wire shape drifted: {structural_json}").into());
    }

    let runtime_refusals = [
        (
            "missing aux_epoch",
            serde_json::json!({ "candidate_id": "a", "universe_epoch": 1 }),
        ),
        (
            "missing universe_epoch",
            serde_json::json!({ "candidate_id": "a", "aux_epoch": 1 }),
        ),
        (
            "missing candidate_id",
            serde_json::json!({ "aux_epoch": 1, "universe_epoch": 1 }),
        ),
        (
            "unknown field",
            serde_json::json!({ "candidate_id": "a", "aux_epoch": 1, "universe_epoch": 1, "offset": 3 }),
        ),
        (
            "signed epoch",
            serde_json::json!({ "candidate_id": "a", "aux_epoch": -1, "universe_epoch": 1 }),
        ),
        (
            "string epoch",
            serde_json::json!({ "candidate_id": "a", "aux_epoch": "1", "universe_epoch": 1 }),
        ),
        (
            "numeric candidate id",
            serde_json::json!({ "candidate_id": 7, "aux_epoch": 1, "universe_epoch": 1 }),
        ),
    ];
    for (label, wrong) in runtime_refusals {
        if serde_json::from_value::<quanta_index_contract::RuntimeMetadataCursorV1>(wrong).is_ok() {
            return Err(format!("runtime cursor: {label} must not decode").into());
        }
    }
    let structural_refusals = [
        (
            "missing aux_epoch",
            serde_json::json!({ "candidate_id": "a" }),
        ),
        (
            "missing candidate_id",
            serde_json::json!({ "aux_epoch": 1 }),
        ),
        (
            "unknown field",
            serde_json::json!({ "candidate_id": "a", "aux_epoch": 1, "universe_epoch": 1 }),
        ),
        (
            "null epoch",
            serde_json::json!({ "candidate_id": "a", "aux_epoch": null }),
        ),
    ];
    for (label, wrong) in structural_refusals {
        if serde_json::from_value::<quanta_index_contract::StructuralCursorV1>(wrong).is_ok() {
            return Err(format!("structural cursor: {label} must not decode").into());
        }
    }
    // A duplicated field is refused on the CBOR wire too.
    let bytes = mutate_ipc_request_wire(
        &SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
            cursor: Some(structural_cursor("a")),
            ..sourcegraph_structural_request()
        }),
        |wire| {
            let request_fields = map_fields_mut(wire)?;
            let payload = field_value_mut(request_fields, "payload")?;
            let payload_fields = map_fields_mut(payload)?;
            let cursor = field_value_mut(payload_fields, "cursor")?;
            let cursor_fields = map_fields_mut(cursor)?;
            cursor_fields.push((
                ciborium::Value::Text("aux_epoch".to_owned()),
                ciborium::Value::Integer(6.into()),
            ));
            Ok(())
        },
    )?;
    expect_decode_error_contains::<SearchPlaneQueryIpcRequest>(&bytes, "duplicate field")
}

/// A page request carries its cursor only when it has one, and the two
/// routes do not accept each other's cursor shape.
#[test]
fn keyset_page_requests_carry_the_cursor_only_when_present() -> TestRes {
    let fresh = quanta_index_contract::RuntimeMetadataQueryRequest {
        text_query: sourcegraph_text_request(),
        cursor: None,
    };
    roundtrip_eq(&fresh)?;
    let fresh_json = serde_json::to_value(&fresh)?;
    if fresh_json
        .as_object()
        .is_some_and(|fields| fields.contains_key("cursor"))
    {
        return Err(format!("a fresh walk carries no cursor on the wire: {fresh_json}").into());
    }
    let continued = quanta_index_contract::RuntimeMetadataQueryRequest {
        text_query: sourcegraph_text_request(),
        cursor: Some(runtime_cursor("chunk://alpha")),
    };
    roundtrip_eq(&SearchPlaneQueryIpcRequest::RuntimeMetadata(continued))?;
    let continued = StructuralQueryRequest {
        cursor: Some(structural_cursor("chunk://alpha")),
        ..sourcegraph_structural_request()
    };
    roundtrip_eq(&SearchPlaneQueryIpcRequest::Structural(continued))?;

    // A structural request with a runtime-metadata cursor (which carries
    // `universe_epoch`) is refused, not narrowed.
    let mut crossed = serde_json::to_value(sourcegraph_structural_request())?;
    let fields = crossed.as_object_mut().ok_or("the request is a map")?;
    if fields
        .insert(
            "cursor".to_owned(),
            serde_json::to_value(runtime_cursor("chunk://alpha"))?,
        )
        .is_some()
    {
        return Err("the fixture carries no cursor".into());
    }
    if serde_json::from_value::<StructuralQueryRequest>(crossed).is_ok() {
        return Err("a structural request must not accept a runtime-metadata cursor".into());
    }
    Ok(())
}

/// Both page responses round-trip with and without a continuation, and
/// `examined` / `next_cursor` / the epochs are on the wire by name.
#[test]
fn keyset_pages_round_trip_with_and_without_a_continuation() -> TestRes {
    let runtime_page = runtime_page_with_continuation()?;
    roundtrip_eq(&SearchPlaneQueryIpcResponse::RuntimeMetadata(
        runtime_page.clone(),
    ))?;
    let json = serde_json::to_value(&runtime_page)?;
    let fields = json.as_object().ok_or("the page is a map")?;
    for name in [
        "generation",
        "results",
        "window",
        "read_epoch",
        "universe_epoch",
        "examined",
        "next_cursor",
    ] {
        if !fields.contains_key(name) {
            return Err(format!("runtime page serializes `{name}`: {json}").into());
        }
    }
    let final_page = quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse {
        results: vec![lexical_candidate_named("z")],
        window: QueryResultWindowV1::exact(1),
        examined: 9,
        next_cursor: None,
        ..runtime_page
    };
    roundtrip_eq(&SearchPlaneQueryIpcResponse::RuntimeMetadata(
        final_page.clone(),
    ))?;
    let json = serde_json::to_value(&final_page)?;
    if json
        .as_object()
        .is_some_and(|fields| fields.contains_key("next_cursor"))
    {
        return Err(format!("a final page carries no cursor on the wire: {json}").into());
    }

    let structural_page = structural_page_with_continuation()?;
    roundtrip_eq(&SearchPlaneQueryIpcResponse::Structural(
        structural_page.clone(),
    ))?;
    let final_page = quanta_index_contract::SearchPlaneStructuralQueryResponse {
        results: Vec::new(),
        window: QueryResultWindowV1::exact(0),
        examined: 0,
        next_cursor: None,
        ..structural_page
    };
    roundtrip_eq(&SearchPlaneQueryIpcResponse::Structural(final_page))
}

/// A page whose window, rows, order, cursor and epochs disagree fails to
/// decode.
///
/// A page cannot claim a continuation it does not position, position one
/// that is not its last row, count rows it does not hold, leave its
/// order, or hand out a cursor for an epoch it did not read.
#[test]
fn keyset_pages_reject_inconsistent_shapes() -> TestRes {
    let base = runtime_page_with_continuation()?;
    let runtime_cases: Vec<(
        &str,
        quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse,
    )> = vec![
        (
            "has_more without a cursor",
            quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse {
                next_cursor: None,
                ..base.clone()
            },
        ),
        (
            "a cursor without has_more",
            quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse {
                window: QueryResultWindowV1::exact(2),
                ..base.clone()
            },
        ),
        (
            "a window that does not count the rows",
            quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse {
                window: QueryResultWindowV1::new(
                    1,
                    quanta_index_contract::CandidateCountV1::AtLeast(3),
                    true,
                )?,
                ..base.clone()
            },
        ),
        (
            "rows out of candidate-id order",
            quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse {
                results: vec![lexical_candidate_named("b"), lexical_candidate_named("a")],
                next_cursor: Some(runtime_cursor("a")),
                ..base.clone()
            },
        ),
        (
            "a duplicated row",
            quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse {
                results: vec![lexical_candidate_named("b"), lexical_candidate_named("b")],
                ..base.clone()
            },
        ),
        (
            "a cursor that is not the last row",
            quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse {
                next_cursor: Some(runtime_cursor("a")),
                ..base.clone()
            },
        ),
        (
            "a cursor naming another runtime epoch than the page read",
            quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse {
                read_epoch: AuxEpochV1::new(5),
                ..base.clone()
            },
        ),
        (
            "a cursor naming another universe epoch than the page read",
            quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse {
                universe_epoch: AuxEpochV1::new(12),
                ..base
            },
        ),
    ];
    for (label, page) in runtime_cases {
        let bytes = encode(&SearchPlaneQueryIpcResponse::RuntimeMetadata(page))?;
        if decode::<SearchPlaneQueryIpcResponse>(&bytes).is_ok() {
            return Err(
                format!("runtime-metadata: {label}: an inconsistent page must not decode").into(),
            );
        }
    }

    let base = structural_page_with_continuation()?;
    let structural_cases: Vec<(
        &str,
        quanta_index_contract::SearchPlaneStructuralQueryResponse,
    )> = vec![
        (
            "has_more without a cursor",
            quanta_index_contract::SearchPlaneStructuralQueryResponse {
                next_cursor: None,
                ..base.clone()
            },
        ),
        (
            "a cursor without has_more",
            quanta_index_contract::SearchPlaneStructuralQueryResponse {
                window: QueryResultWindowV1::exact(2),
                ..base.clone()
            },
        ),
        (
            "a window that does not count the rows",
            quanta_index_contract::SearchPlaneStructuralQueryResponse {
                window: QueryResultWindowV1::new(
                    3,
                    quanta_index_contract::CandidateCountV1::Exact(5),
                    true,
                )?,
                ..base.clone()
            },
        ),
        (
            "rows out of candidate-id order",
            quanta_index_contract::SearchPlaneStructuralQueryResponse {
                results: vec![
                    structural_candidate_named("b"),
                    structural_candidate_named("a"),
                ],
                next_cursor: Some(structural_cursor("a")),
                ..base.clone()
            },
        ),
        (
            "a cursor that is not the last row",
            quanta_index_contract::SearchPlaneStructuralQueryResponse {
                next_cursor: Some(structural_cursor("a")),
                ..base.clone()
            },
        ),
        (
            "a cursor naming another epoch than the page read",
            quanta_index_contract::SearchPlaneStructuralQueryResponse {
                read_epoch: AuxEpochV1::new(7),
                ..base
            },
        ),
    ];
    for (label, page) in structural_cases {
        let bytes = encode(&SearchPlaneQueryIpcResponse::Structural(page))?;
        if decode::<SearchPlaneQueryIpcResponse>(&bytes).is_ok() {
            return Err(
                format!("structural: {label}: an inconsistent page must not decode").into(),
            );
        }
    }

    // Every required field is required: dropping `examined` or an epoch
    // from the wire refuses the page.
    for name in ["examined", "universe_epoch", "read_epoch", "window"] {
        let mut value = serde_json::to_value(SearchPlaneQueryIpcResponse::RuntimeMetadata(
            runtime_page_with_continuation()?,
        ))?;
        let payload = value
            .get_mut("payload")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("the response is a tagged map")?;
        if payload.remove(name).is_none() {
            return Err(format!("the runtime page serializes `{name}`").into());
        }
        if serde_json::from_value::<SearchPlaneQueryIpcResponse>(value).is_ok() {
            return Err(format!("a runtime page without `{name}` must not decode").into());
        }
    }
    for name in ["examined", "read_epoch", "window"] {
        let mut value = serde_json::to_value(SearchPlaneQueryIpcResponse::Structural(
            structural_page_with_continuation()?,
        ))?;
        let payload = value
            .get_mut("payload")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("the response is a tagged map")?;
        if payload.remove(name).is_none() {
            return Err(format!("the structural page serializes `{name}`").into());
        }
        if serde_json::from_value::<SearchPlaneQueryIpcResponse>(value).is_ok() {
            return Err(format!("a structural page without `{name}` must not decode").into());
        }
    }
    Ok(())
}

// QI-BB-025: `top_k` is gated at the wire on both sides. Every request
// variant that carries one — including a `top_k` nested in a page request's
// `text_query`, a semantic scope, or a hybrid-seed corpus budget — refuses
// to encode out of range, and raw bytes carrying an out-of-range value that
// no encoder would have produced are refused at decode, under the one code.
#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn top_k_bearing_requests(top_k: u32) -> Vec<(&'static str, SearchPlaneQueryIpcRequest)> {
    let text = TextQueryRequest {
        top_k,
        ..lexical_request()
    };
    vec![
        ("lexical", SearchPlaneQueryIpcRequest::Text(text.clone())),
        (
            "symbol",
            SearchPlaneQueryIpcRequest::Symbol(quanta_index_contract::SymbolQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "symbol.has.name(needle)".to_owned(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(generation_pin()),
                generation_selector: None,
                top_k,
                cursor: None,
            }),
        ),
        (
            "semantic",
            SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                top_k,
                lexical_scope: None,
                ..semantic_request()
            }),
        ),
        (
            "semantic-scope",
            SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                lexical_scope: Some(TextQueryRequest {
                    top_k,
                    ..semantic_scope()
                }),
                ..semantic_request()
            }),
        ),
        (
            "hybrid",
            SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                top_k,
                ..hybrid_request()
            }),
        ),
        (
            "hybrid-seed",
            SearchPlaneQueryIpcRequest::HybridSeed(HybridSeedQueryRequest {
                top_k,
                ..hybrid_seed_request_with_corpus_budgets()
            }),
        ),
        (
            "hybrid-seed-corpus-budget",
            SearchPlaneQueryIpcRequest::HybridSeed(HybridSeedQueryRequest {
                dense_corpora: vec![SemanticSeedCorpusBudgetV1 {
                    corpus_kind: SemanticCorpusKindV1::SymbolCard,
                    top_k,
                }],
                ..hybrid_seed_request_with_corpus_budgets()
            }),
        ),
        (
            "history",
            SearchPlaneQueryIpcRequest::History(quanta_index_contract::HistoryQueryRequest {
                text_query: text.clone(),
                order: quanta_index_contract::HistoryOrderV1::Recency,
                cursor: None,
            }),
        ),
        (
            "runtime-metadata",
            SearchPlaneQueryIpcRequest::RuntimeMetadata(
                quanta_index_contract::RuntimeMetadataQueryRequest {
                    text_query: text.clone(),
                    cursor: None,
                },
            ),
        ),
        (
            "structural",
            SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: text,
                cursor: None,
            }),
        ),
        (
            "repo-map",
            SearchPlaneQueryIpcRequest::RepoMapQuery(quanta_index_contract::RepoMapQueryRequest {
                repo_id: RepoId::new("repo-x")
                    .expect("static fixture ID satisfies canonical policy"),
                revision_id: RevisionId::new("rev-y")
                    .expect("static fixture ID satisfies canonical policy"),
                manifest_generation: ManifestGeneration::new(1),
                query_text: "needle".to_owned(),
                top_k,
                token_budget: 100,
                focus_subjects: Vec::new(),
            }),
        ),
    ]
}

/// Replace every `top_k` entry in a decoded CBOR tree with `top_k`, so the
/// bytes carry a value no encoder would emit.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "ciborium::Value is #[non_exhaustive]; the leaves and any future variant carry no top_k"
)]
fn patch_every_top_k(value: &mut ciborium::Value, top_k: u32) -> usize {
    match value {
        ciborium::Value::Map(fields) => fields
            .iter_mut()
            .map(|(key, entry)| {
                if matches!(key, ciborium::Value::Text(name) if name == "top_k") {
                    *entry = ciborium::Value::Integer(top_k.into());
                    1
                } else {
                    patch_every_top_k(entry, top_k)
                }
            })
            .sum(),
        ciborium::Value::Array(items) => items
            .iter_mut()
            .map(|item| patch_every_top_k(item, top_k))
            .sum(),
        // Every leaf variant, and — `ciborium::Value` being
        // `#[non_exhaustive]` — any variant this codec version does not
        // name, carries no `top_k`.
        _ => 0,
    }
}

/// QI-BB-025: out-of-range `top_k` does not encode, and raw bytes still decode.
///
/// A typed client cannot encode it on any route; raw bytes carrying one
/// decode, so the dispatcher answers the raw caller typed under the same
/// code instead of the connection closing.
#[test]
fn every_top_k_bearing_request_refuses_out_of_range_on_encode_and_decodes_it_for_the_dispatcher()
-> TestRes {
    let refused_values = [0, quanta_index_contract::PUBLIC_TOP_K_MAX + 1, u32::MAX];
    for (route, in_range) in top_k_bearing_requests(1) {
        roundtrip_eq(&in_range)?;
        let mut wire: ciborium::Value = decode(&encode(&in_range)?)?;
        for refused in refused_values {
            let (name, out_of_range) = top_k_bearing_requests(refused)
                .into_iter()
                .find(|(name, _)| *name == route)
                .ok_or("the route table is stable across values")?;
            if encode(&out_of_range).is_ok() {
                return Err(format!("{name}: top_k={refused} must not encode").into());
            }
            let patched = patch_every_top_k(&mut wire, refused);
            if patched == 0 {
                return Err(format!("{name}: the wire carries no top_k to patch").into());
            }
            let bytes = encode(&wire)?;
            let _decoded: SearchPlaneQueryIpcRequest = decode(&bytes)
                .map_err(|err| format!("{name}: top_k={refused} must decode: {err}"))?;
        }
    }
    Ok(())
}
