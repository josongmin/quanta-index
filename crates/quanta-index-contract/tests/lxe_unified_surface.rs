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
    GenerationPin, HybridQueryRequest, ManifestGeneration, QueryStageKindV1, QueryStageTimingV1,
    RepoId, RevisionId, SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use std::fmt;

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

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn generation_pin() -> GenerationPin {
    GenerationPin::new(
        RepoId::new("repo-lxe").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("rev-lxe").expect("static fixture ID satisfies canonical policy"),
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
        // Deliberately distinct from `engines_touched`: a serializer that
        // swaps the two vec fields must fail the roundtrip below.
        engines_executed: vec![EngineTouched::Lexical],
        request_id: 41,
        stage_timings: None,
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
fn search_explanation_round_trips_all_ten_fields() -> TestRes {
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
    if decoded.engines_executed != vec![EngineTouched::Lexical] {
        return Err("engines_executed dropped on roundtrip".into());
    }
    if decoded.request_id != 41 {
        return Err("request_id dropped on roundtrip".into());
    }
    if decoded.stage_timings.is_some() {
        return Err("unmeasured stage timings acquired a synthetic value".into());
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
fn server_stage_timing_round_trips_and_refuses_false_shapes() -> TestRes {
    let stage = QueryStageTimingV1 {
        stage: QueryStageKindV1::HybridDenseFetch,
        elapsed_ns: 1_234,
        calls: 2,
        returned_candidates: Some(13),
    };
    let original = SearchExplanation {
        stage_timings: Some(vec![stage]),
        ..sample_explanation_full()
    };
    let decoded: SearchExplanation = decode(&encode(&original)?)?;
    if decoded != original {
        return Err("typed stage timing disappeared over CBOR".into());
    }
    let invalid = [
        r#"{"stage":"hybrid.dense_fetch","elapsed_ns":1,"calls":0,"returned_candidates":1}"#,
        r#"{"stage":"","elapsed_ns":1,"calls":1,"returned_candidates":1}"#,
        r#"{"stage":"hybrid.unknown","elapsed_ns":1,"calls":1,"returned_candidates":1}"#,
        r#"{"stage":"hybrid.dense_fetch","elapsed_ns":1,"calls":1}"#,
        r#"{"stage":"hybrid.dense_fetch","elapsed_ns":1,"calls":1,"returned_candidates":1,"stage":"hybrid.fusion"}"#,
        r#"{"stage":"hybrid.dense_fetch","elapsed_ns":1,"calls":1,"returned_candidates":1,"unknown":1}"#,
    ];
    for payload in invalid {
        if serde_json::from_str::<QueryStageTimingV1>(payload).is_ok() {
            return Err(format!("invalid stage timing decoded: {payload}").into());
        }
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

// --- S21-10 compat: V0 payloads (pre-`engines_executed`/`request_id`) ------
//
// The current reader defaults the two S21-10 fields (`[]`, `0`) so
// pre-change payloads stay readable; a pinned V0 decoder — the exact
// pre-change field set with unknown fields denied — rejects every new
// field instead of misreading. No binary fixtures: the V0 bytes below
// are a freshly encoded current payload with the two fields stripped,
// and the stripped key set is asserted exactly, so test-construction
// drift fails the test rather than weakening the oracle.

/// Pinned pre-S21-10 `SearchExplanation` reader: the exact V0 field set.
///
/// This mirrors the decoder every pre-change consumer ran. The
/// component types are unchanged since V0; only `engines_executed` and
/// `request_id` are absent, and any other field is an unknown-field
/// refusal. Manual `Deserialize` per CLAUDE.md D18 — no derives.
#[derive(Debug)]
struct SearchExplanationV0Pin {
    planner_trace: Vec<PlannerTraceEntry>,
    engines_touched: Vec<EngineTouched>,
    early_stop_reason: Option<EarlyStopReason>,
    contributions: Vec<ExplanationRow>,
    ranker_weights_hash: [u8; 32],
    strategy: String,
    summary: String,
}

const SEARCH_EXPLANATION_V0_FIELDS: &[&str] = &[
    "planner_trace",
    "engines_touched",
    "early_stop_reason",
    "contributions",
    "ranker_weights_hash",
    "strategy",
    "summary",
];

struct SearchExplanationV0Visitor;

impl<'de> Visitor<'de> for SearchExplanationV0Visitor {
    type Value = SearchExplanationV0Pin;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a V0 SearchExplanation map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut planner_trace: Option<Vec<PlannerTraceEntry>> = None;
        let mut engines_touched: Option<Vec<EngineTouched>> = None;
        let mut early_stop_reason: Option<Option<EarlyStopReason>> = None;
        let mut contributions: Option<Vec<ExplanationRow>> = None;
        let mut ranker_weights_hash: Option<[u8; 32]> = None;
        let mut strategy: Option<String> = None;
        let mut summary: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "planner_trace" => {
                    if planner_trace.is_some() {
                        return Err(de::Error::duplicate_field("planner_trace"));
                    }
                    planner_trace = Some(map.next_value()?);
                }
                "engines_touched" => {
                    if engines_touched.is_some() {
                        return Err(de::Error::duplicate_field("engines_touched"));
                    }
                    engines_touched = Some(map.next_value()?);
                }
                "early_stop_reason" => {
                    if early_stop_reason.is_some() {
                        return Err(de::Error::duplicate_field("early_stop_reason"));
                    }
                    early_stop_reason = Some(Some(map.next_value()?));
                }
                "contributions" => {
                    if contributions.is_some() {
                        return Err(de::Error::duplicate_field("contributions"));
                    }
                    contributions = Some(map.next_value()?);
                }
                "ranker_weights_hash" => {
                    if ranker_weights_hash.is_some() {
                        return Err(de::Error::duplicate_field("ranker_weights_hash"));
                    }
                    ranker_weights_hash = Some(map.next_value()?);
                }
                "strategy" => {
                    if strategy.is_some() {
                        return Err(de::Error::duplicate_field("strategy"));
                    }
                    strategy = Some(map.next_value()?);
                }
                "summary" => {
                    if summary.is_some() {
                        return Err(de::Error::duplicate_field("summary"));
                    }
                    summary = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_EXPLANATION_V0_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchExplanationV0Pin {
            planner_trace: planner_trace
                .ok_or_else(|| de::Error::missing_field("planner_trace"))?,
            engines_touched: engines_touched
                .ok_or_else(|| de::Error::missing_field("engines_touched"))?,
            early_stop_reason: early_stop_reason.unwrap_or(None),
            contributions: contributions
                .ok_or_else(|| de::Error::missing_field("contributions"))?,
            ranker_weights_hash: ranker_weights_hash
                .ok_or_else(|| de::Error::missing_field("ranker_weights_hash"))?,
            strategy: strategy.ok_or_else(|| de::Error::missing_field("strategy"))?,
            summary: summary.ok_or_else(|| de::Error::missing_field("summary"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchExplanationV0Pin {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchExplanationV0Pin",
            SEARCH_EXPLANATION_V0_FIELDS,
            SearchExplanationV0Visitor,
        )
    }
}

/// Strip `fields` from a freshly encoded explanation's CBOR map. Every
/// named field must be present exactly once; anything else is fixture
/// drift, not a V0 payload.
fn strip_explanation_fields(
    value: &SearchExplanation,
    fields: &[&str],
) -> Result<ciborium::Value, Box<dyn std::error::Error>> {
    let wire: ciborium::Value = decode(&encode(value)?)?;
    let ciborium::Value::Map(mut entries) = wire else {
        return Err("expected SearchExplanation to encode as a map".into());
    };
    for field in fields {
        let before = entries.len();
        entries.retain(|(key, _)| !matches!(key, ciborium::Value::Text(name) if name == field));
        if entries.len() != before.saturating_sub(1) {
            return Err(format!("wire map carries `{field}` exactly once").into());
        }
    }
    Ok(ciborium::Value::Map(entries))
}

/// Sorted text keys of a CBOR map.
fn wire_map_keys(wire: &ciborium::Value) -> Result<Vec<&str>, Box<dyn std::error::Error>> {
    let ciborium::Value::Map(entries) = wire else {
        return Err("expected a CBOR map".into());
    };
    let mut keys = Vec::new();
    for (key, _) in entries {
        let ciborium::Value::Text(name) = key else {
            return Err(format!("expected text map keys, got {key:?}").into());
        };
        keys.push(name.as_str());
    }
    keys.sort_unstable();
    Ok(keys)
}

#[test]
fn search_explanation_current_reader_refuses_v0_payload() -> TestRes {
    let full = sample_explanation_full();
    for field in ["engines_executed", "request_id", "stage_timings"] {
        let old = strip_explanation_fields(&full, &[field])?;
        let error = decode::<SearchExplanation>(&encode(&old)?)
            .expect_err("old explanation missing current field must refuse");
        if !error.to_string().contains(field) {
            return Err(format!("wrong refusal for {field}: {error}").into());
        }
    }
    Ok(())
}

#[test]
fn search_explanation_pinned_v0_decoder_rejects_each_new_field() -> TestRes {
    // The full current payload trips on the first new field in wire
    // order; each single-field payload trips on exactly its new field.
    // Every refusal must be an unknown-field refusal naming the field.
    let full = sample_explanation_full();
    // Positive control: a true V0 payload reads cleanly, so the
    // rejections below prove the pin trips on the new fields — not that
    // the pin rejects everything.
    let v0 = strip_explanation_fields(&full, &["engines_executed", "request_id", "stage_timings"])?;
    let pinned: SearchExplanationV0Pin = decode(&encode(&v0)?)?;
    if pinned.strategy != "hybrid-v1"
        || pinned.summary != "regex narrowed by repo filter"
        || pinned.engines_touched != vec![EngineTouched::Lexical, EngineTouched::Semantic]
        || pinned.early_stop_reason != Some(EarlyStopReason::CountReached)
        || pinned.planner_trace.len() != 2
        || pinned.contributions.len() != 1
        || pinned.ranker_weights_hash != [0xab; 32]
    {
        return Err(format!("pinned V0 decoder misread a V0 payload: {pinned:?}").into());
    }
    let cases: Vec<(&str, Vec<&str>, &str)> = vec![
        ("full current payload", vec![], "engines_executed"),
        (
            "current payload without engines_executed",
            vec!["engines_executed"],
            "request_id",
        ),
        (
            "current payload without request_id",
            vec!["request_id"],
            "engines_executed",
        ),
        (
            "current payload with only stage_timings new",
            vec!["engines_executed", "request_id"],
            "stage_timings",
        ),
    ];
    for (label, strip, expected) in cases {
        let wire = strip_explanation_fields(&full, &strip)?;
        let bytes = encode(&wire)?;
        let result: Result<SearchExplanationV0Pin, _> = ciborium::de::from_reader(bytes.as_slice());
        match result {
            Ok(pinned) => {
                return Err(format!(
                    "{label}: pinned V0 decoder accepted new field `{expected}`: {pinned:?}"
                )
                .into());
            }
            Err(err) => {
                let message = err.to_string();
                if !message.contains("unknown field") || !message.contains(expected) {
                    return Err(format!(
                        "{label}: V0 refusal missed unknown-field `{expected}`: {message}"
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}

/// A payload duplicating `field` must be refused as a duplicate field,
/// naming it.
fn duplicate_explanation_field_is_refused(field: &str) -> TestRes {
    let bytes = encode(&sample_explanation_full())?;
    let mut wire: ciborium::Value = decode(&bytes)?;
    let ciborium::Value::Map(entries) = &mut wire else {
        return Err("expected SearchExplanation to encode as a map".into());
    };
    let duplicate = entries
        .iter()
        .find(|(key, _)| matches!(key, ciborium::Value::Text(name) if name == field))
        .cloned()
        .ok_or_else(|| format!("wire map carries `{field}`"))?;
    entries.push(duplicate);
    let bytes = encode(&wire)?;
    let result: Result<SearchExplanation, _> = ciborium::de::from_reader(bytes.as_slice());
    match result {
        Ok(decoded) => Err(format!("duplicated `{field}` decoded: {decoded:?}").into()),
        Err(err) => {
            let expected = format!("duplicate field `{field}`");
            let message = err.to_string();
            if !message.contains(&expected) {
                return Err(format!("refusal missed `{expected}`: {message}").into());
            }
            Ok(())
        }
    }
}

#[test]
fn search_explanation_rejects_duplicate_engines_executed() -> TestRes {
    duplicate_explanation_field_is_refused("engines_executed")
}

#[test]
fn search_explanation_rejects_duplicate_request_id() -> TestRes {
    duplicate_explanation_field_is_refused("request_id")
}

#[test]
fn search_explanation_rejects_duplicate_stage_timings() -> TestRes {
    duplicate_explanation_field_is_refused("stage_timings")
}

#[test]
fn search_explanation_early_stop_reason_is_explicit_even_when_null() -> TestRes {
    let present = sample_explanation_full();
    let absent = SearchExplanation {
        early_stop_reason: None,
        ..present.clone()
    };
    let present_wire: ciborium::Value = decode(&encode(&present)?)?;
    let absent_wire: ciborium::Value = decode(&encode(&absent)?)?;
    let present_keys = wire_map_keys(&present_wire)?;
    let absent_keys = wire_map_keys(&absent_wire)?;
    if present_keys.len() != 10 {
        return Err(format!("present map must hold 10 fields: {present_keys:?}").into());
    }
    if absent_keys.len() != 10 {
        return Err(format!("null map must hold 10 fields: {absent_keys:?}").into());
    }
    if !present_keys.contains(&"early_stop_reason") {
        return Err(format!("present map must carry early_stop_reason: {present_keys:?}").into());
    }
    if !absent_keys.contains(&"early_stop_reason") {
        return Err(format!("null map must carry early_stop_reason: {absent_keys:?}").into());
    }
    let ciborium::Value::Map(fields) = absent_wire else {
        return Err("explanation must encode as a map".into());
    };
    if !fields.iter().any(|(key, value)| {
        key == &ciborium::Value::Text("early_stop_reason".to_owned())
            && value == &ciborium::Value::Null
    }) {
        return Err("empty early_stop_reason must encode as null".into());
    }
    let old_wire = ciborium::Value::Map(
        fields
            .into_iter()
            .filter(|(key, _)| key != &ciborium::Value::Text("early_stop_reason".to_owned()))
            .collect(),
    );
    if decode::<SearchExplanation>(&encode(&old_wire)?).is_ok() {
        return Err("missing early_stop_reason must be refused".into());
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
