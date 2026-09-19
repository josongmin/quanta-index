//! LXE-01 / LXE-07 contract-surface unification tests.
//!
//! These tests fence the canonical post-LXE shapes:
//!
//! * `SemanticQueryRequest.lexical_scope: Option<TextQueryRequest>` —
//!   the `SemanticCandidateScope` dual surface has been deleted.
//! * `HybridQueryRequest.text_query: TextQueryRequest` —
//!   hybrid lexical intake uses the same canonical text carrier.
//! * `SearchExplanation` (canonical at
//!   [`quanta_index_contract::results::SearchExplanation`], re-exported as
//!   `lex::SearchExplanation`) carries `planner_trace` of typed
//!   `PlannerTraceEntry { stage: PlannerStage, detail: String }`, an
//!   `engines_touched` vector of typed `EngineTouched`, optional typed
//!   `early_stop_reason`, and `summary` (LXE-07 planner provenance).
//! * `SearchExplanationBuilder::ranker_weights_hash` accepts
//!   `Result<[u8; 32], WeightsHashError>` so callers cannot silently
//!   substitute a zero placeholder (CLAUDE.md digest-fallibility rule).
//!
//! Each test fails fast on shape drift; rerunning is the contract gate.

#![forbid(unsafe_code)]

use quanta_index_contract::lex::{
    EarlyStopReason, EngineTouched, ExplanationRow, PlannerStage, PlannerTraceEntry,
    SearchExplanation, SearchExplanationBuilder, WeightsHashError,
};
use quanta_index_contract::{
    GenerationPin, HybridQueryRequest, ManifestGeneration, RepoId, RevisionId,
    SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
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

fn generation_pin() -> GenerationPin {
    GenerationPin::new(
        RepoId::new("repo-lxe"),
        RevisionId::new("rev-lxe"),
        ManifestGeneration::new(42),
    )
}

fn lexical_scope_text_query() -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "repo:quanta-index lang:rust SemanticQuery".to_owned(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(generation_pin()),
        generation_selector: None,
        top_k: 64,
        cursor: None,
    }
}

fn semantic_query_text() -> String {
    "semantic meaning for SearchPlane".to_owned()
}

// --- LXE-01: SemanticQueryRequest carries lexical_scope, not scope ---------

#[test]
fn semantic_query_request_lexical_scope_round_trips() -> TestRes {
    let original = SemanticQueryRequest {
        query_text: semantic_query_text(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(generation_pin()),
        generation_selector: None,
        lexical_scope: Some(lexical_scope_text_query()),
        top_k: 10,
    };
    let bytes = encode(&original)?;
    let decoded: SemanticQueryRequest = decode(&bytes)?;
    if decoded != original {
        return Err(
            format!("roundtrip mismatch: original={original:?} decoded={decoded:?}").into(),
        );
    }
    let scope = decoded
        .lexical_scope
        .ok_or_else(|| "lexical_scope missing after roundtrip".to_string())?;
    if scope.syntax != TextQuerySyntax::Sourcegraph {
        return Err("lexical_scope.syntax not preserved".into());
    }
    if scope.top_k != 64 {
        return Err("lexical_scope.top_k not preserved".into());
    }
    Ok(())
}

#[test]
fn semantic_query_request_none_lexical_scope_round_trips() -> TestRes {
    let original = SemanticQueryRequest {
        query_text: semantic_query_text(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        lexical_scope: None,
        top_k: 5,
    };
    let bytes = encode(&original)?;
    let decoded: SemanticQueryRequest = decode(&bytes)?;
    if decoded != original {
        return Err("roundtrip mismatch on None lexical_scope".into());
    }
    Ok(())
}

#[test]
fn semantic_query_request_rejects_legacy_scope_field() -> TestRes {
    // Build a wire map with the legacy "scope" field name; the new decoder
    // must reject it as an unknown field. This fences the dual-surface
    // deletion at the wire level.
    let original = SemanticQueryRequest {
        query_text: semantic_query_text(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        lexical_scope: Some(lexical_scope_text_query()),
        top_k: 7,
    };
    let bytes = encode(&original)?;
    let mut wire: ciborium::Value = decode(&bytes)?;
    let ciborium::Value::Map(fields) = &mut wire else {
        return Err("expected map".into());
    };
    // Rename `lexical_scope` -> `scope` to simulate a producer still using
    // the deleted dual surface.
    for (key, _value) in fields.iter_mut() {
        if let ciborium::Value::Text(text) = key
            && text == "lexical_scope"
        {
            *text = "scope".to_owned();
        }
    }
    let mutated = encode(&wire)?;
    let result: Result<SemanticQueryRequest, _> =
        ciborium::de::from_reader::<SemanticQueryRequest, _>(mutated.as_slice());
    if result.is_ok() {
        return Err("decoder must reject legacy `scope` field".into());
    }
    Ok(())
}

// --- LXE-01: assert the wire-field name remains `lexical_scope` -----------

#[test]
fn semantic_query_request_wire_field_is_lexical_scope() -> TestRes {
    // Compile-time + runtime fence: the canonical field is `lexical_scope`.
    // If a future commit re-introduces a `scope` field literal, the struct
    // literal below fails to compile; if the wire field is renamed, the
    // map-key assertion below catches the regression.
    let sentinel = SemanticQueryRequest {
        query_text: semantic_query_text(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        lexical_scope: Some(lexical_scope_text_query()),
        top_k: 1,
    };
    let bytes = encode(&sentinel)?;
    let wire: ciborium::Value = decode(&bytes)?;
    let ciborium::Value::Map(fields) = &wire else {
        return Err("expected SemanticQueryRequest to encode as a map".into());
    };
    let saw_lexical_scope = fields
        .iter()
        .any(|(key, _)| matches!(key, ciborium::Value::Text(text) if text == "lexical_scope"));
    if !saw_lexical_scope {
        return Err("wire map missing `lexical_scope` field".into());
    }
    let saw_scope = fields
        .iter()
        .any(|(key, _)| matches!(key, ciborium::Value::Text(text) if text == "scope"));
    if saw_scope {
        return Err("wire map must not contain legacy `scope` field".into());
    }
    Ok(())
}

#[test]
fn hybrid_query_request_text_query_round_trips() -> TestRes {
    let original = HybridQueryRequest {
        text_query: lexical_scope_text_query(),
        semantic_query_text: "hybrid semantic meaning".to_owned(),
        generation: Some(generation_pin()),
        generation_selector: None,
        top_k: 11,
    };
    let bytes = encode(&original)?;
    let decoded: HybridQueryRequest = decode(&bytes)?;
    if decoded != original {
        return Err(
            format!("roundtrip mismatch: original={original:?} decoded={decoded:?}").into(),
        );
    }
    if decoded.text_query.syntax != TextQuerySyntax::Sourcegraph {
        return Err("hybrid text_query.syntax not preserved".into());
    }
    if decoded.text_query.top_k != 64 {
        return Err("hybrid text_query.top_k not preserved".into());
    }
    Ok(())
}

#[test]
fn hybrid_query_request_wire_field_is_text_query() -> TestRes {
    let sentinel = HybridQueryRequest {
        text_query: lexical_scope_text_query(),
        semantic_query_text: "hybrid semantic meaning".to_owned(),
        generation: Some(generation_pin()),
        generation_selector: None,
        top_k: 11,
    };
    let bytes = encode(&sentinel)?;
    let wire: ciborium::Value = decode(&bytes)?;
    let ciborium::Value::Map(fields) = &wire else {
        return Err("expected HybridQueryRequest to encode as a map".into());
    };
    let saw_text_query = fields
        .iter()
        .any(|(key, _)| matches!(key, ciborium::Value::Text(text) if text == "text_query"));
    if !saw_text_query {
        return Err("wire map missing `text_query` field".into());
    }
    let saw_legacy_lexical = fields
        .iter()
        .any(|(key, _)| matches!(key, ciborium::Value::Text(text) if text == "lexical"));
    if saw_legacy_lexical {
        return Err("wire map must not contain legacy `lexical` field".into());
    }
    Ok(())
}

// --- LXE-07: SearchExplanation augmented with planner-provenance fields ----

fn sample_explanation_full() -> SearchExplanation {
    SearchExplanation {
        planner_trace: vec![
            PlannerTraceEntry {
                stage: PlannerStage::Filter,
                detail: "repo:quanta-index".to_owned(),
            },
            PlannerTraceEntry {
                stage: PlannerStage::LeafRegex,
                detail: "Search.*Plane".to_owned(),
            },
        ],
        engines_touched: vec![EngineTouched::Lexical, EngineTouched::Semantic],
        early_stop_reason: Some(EarlyStopReason::CountReached),
        contributions: vec![ExplanationRow {
            signal_name: Box::from("bm25"),
            signal_value: 1.0,
            weight: 0.5,
            contribution: 0.5,
        }],
        ranker_weights_hash: [0xab; 32],
        strategy: "hybrid-v1".to_owned(),
        summary: "regex narrowed by repo filter".to_owned(),
    }
}

#[test]
fn search_explanation_round_trips_all_seven_fields() -> TestRes {
    let original = sample_explanation_full();
    let bytes = encode(&original)?;
    let decoded: SearchExplanation = decode(&bytes)?;
    if decoded != original {
        return Err(format!(
            "search_explanation roundtrip mismatch: original={original:?} decoded={decoded:?}"
        )
        .into());
    }
    // Explicit per-field guards: catch a serializer that silently drops a
    // field but happens to round-trip via Default.
    if decoded.planner_trace.len() != 2 {
        return Err("planner_trace dropped on roundtrip".into());
    }
    let first_stage = decoded
        .planner_trace
        .first()
        .map(|entry| entry.stage)
        .ok_or("planner_trace[0] missing")?;
    if first_stage != PlannerStage::Filter {
        return Err("planner_trace[0].stage not preserved as typed enum".into());
    }
    let second_stage = decoded
        .planner_trace
        .get(1)
        .map(|entry| entry.stage)
        .ok_or("planner_trace[1] missing")?;
    if second_stage != PlannerStage::LeafRegex {
        return Err("planner_trace[1].stage not preserved as typed enum".into());
    }
    if decoded.engines_touched != vec![EngineTouched::Lexical, EngineTouched::Semantic] {
        return Err("engines_touched dropped on roundtrip".into());
    }
    if decoded.early_stop_reason != Some(EarlyStopReason::CountReached) {
        return Err("early_stop_reason dropped on roundtrip".into());
    }
    if decoded.summary != "regex narrowed by repo filter" {
        return Err("summary dropped on roundtrip".into());
    }
    if decoded.contributions.len() != 1 {
        return Err("contributions dropped on roundtrip".into());
    }
    if decoded.ranker_weights_hash != [0xab; 32] {
        return Err("ranker_weights_hash dropped on roundtrip".into());
    }
    if decoded.strategy != "hybrid-v1" {
        return Err("strategy dropped on roundtrip".into());
    }
    Ok(())
}

#[test]
fn search_explanation_empty_round_trips() -> TestRes {
    let original = SearchExplanation::empty();
    let bytes = encode(&original)?;
    let decoded: SearchExplanation = decode(&bytes)?;
    if decoded != original {
        return Err("empty SearchExplanation roundtrip mismatch".into());
    }
    if !decoded.contributions.is_empty()
        || !decoded.planner_trace.is_empty()
        || !decoded.engines_touched.is_empty()
        || decoded.early_stop_reason.is_some()
        || !decoded.summary.is_empty()
    {
        return Err("SearchExplanation::empty did not produce empty fields".into());
    }
    Ok(())
}

#[test]
fn search_explanation_builder_pushes_typed_fields() -> TestRes {
    let built = SearchExplanationBuilder::new()
        .strategy("lexical-only".to_owned())
        .ranker_weights_hash(Ok([0x77; 32]))?
        .push_trace(PlannerTraceEntry {
            stage: PlannerStage::LeafPhrase,
            detail: "\"index lookup\"".to_owned(),
        })
        .push_engine(EngineTouched::Lexical)
        .push_engine(EngineTouched::Bridge)
        .early_stop_reason(Some(EarlyStopReason::Unsupported))
        .summary("phrase resolved against positions index".to_owned())
        .build();
    if built.engines_touched.len() != 2 {
        return Err("builder.push_engine did not append".into());
    }
    if built.planner_trace.len() != 1 {
        return Err("builder.push_trace did not append".into());
    }
    let built_stage = built
        .planner_trace
        .first()
        .map(|entry| entry.stage)
        .ok_or("builder.push_trace did not append a typed entry")?;
    if built_stage != PlannerStage::LeafPhrase {
        return Err("builder.push_trace stage not preserved".into());
    }
    if built.strategy != "lexical-only" {
        return Err("builder.strategy not propagated".into());
    }
    if built.ranker_weights_hash != [0x77; 32] {
        return Err("builder.ranker_weights_hash not propagated".into());
    }
    if built.early_stop_reason != Some(EarlyStopReason::Unsupported) {
        return Err("builder.early_stop_reason not propagated".into());
    }
    if built.summary != "phrase resolved against positions index" {
        return Err("builder.summary not propagated".into());
    }
    // Final value must also round-trip cleanly through CBOR.
    let bytes = encode(&built)?;
    let decoded: SearchExplanation = decode(&bytes)?;
    if decoded != built {
        return Err("builder output failed CBOR roundtrip".into());
    }
    Ok(())
}

#[test]
fn search_explanation_builder_forwards_weights_hash_error() -> TestRes {
    // The fallible setter must forward a producer error rather than swallow
    // it or substitute a default. This is the digest-fallibility fence.
    let result = SearchExplanationBuilder::new()
        .ranker_weights_hash(Err(WeightsHashError::new("codec step failed")));
    match result {
        Err(err) => {
            if !err.message().contains("codec step failed") {
                return Err(format!(
                    "WeightsHashError did not forward producer diagnostic: {}",
                    err.message()
                )
                .into());
            }
            // Display surface must also include the diagnostic.
            let rendered = format!("{err}");
            if !rendered.contains("codec step failed") {
                return Err(format!(
                    "WeightsHashError Display did not include diagnostic: {rendered}",
                )
                .into());
            }
            Ok(())
        }
        Ok(_) => Err("WeightsHashError setter must forward producer error".into()),
    }
}

#[test]
fn search_explanation_rejects_unknown_field() -> TestRes {
    let bytes = encode(&sample_explanation_full())?;
    let mut wire: ciborium::Value = decode(&bytes)?;
    let ciborium::Value::Map(fields) = &mut wire else {
        return Err("expected map".into());
    };
    fields.push((
        ciborium::Value::Text("__never_field".to_owned()),
        ciborium::Value::Bool(true),
    ));
    let mutated = encode(&wire)?;
    let result: Result<SearchExplanation, _> =
        ciborium::de::from_reader::<SearchExplanation, _>(mutated.as_slice());
    if result.is_ok() {
        return Err("decoder must reject unknown SearchExplanation fields".into());
    }
    Ok(())
}

#[test]
fn planner_trace_entry_round_trips_typed_stage() -> TestRes {
    // Replaces the legacy stringly-typed `PlannerTraceNode` roundtrip.
    // Asserts that `stage` is the typed `PlannerStage` enum (not `Box<str>`)
    // by exercising every variant the builder uses.
    for stage in [
        PlannerStage::Filter,
        PlannerStage::LeafRegex,
        PlannerStage::LeafPhrase,
        PlannerStage::Parse,
        PlannerStage::Normalize,
        PlannerStage::Plan,
        PlannerStage::ExecFanout,
        PlannerStage::Merge,
        PlannerStage::Rerank,
        PlannerStage::Bridge,
    ] {
        let original = PlannerTraceEntry {
            stage,
            detail: format!("detail-for-{}", stage.as_str()),
        };
        let bytes = encode(&original)?;
        let decoded: PlannerTraceEntry = decode(&bytes)?;
        if decoded != original {
            return Err(format!(
                "PlannerTraceEntry roundtrip mismatch for {stage:?}: decoded={decoded:?}"
            )
            .into());
        }
    }
    Ok(())
}
