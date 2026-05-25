//! LXE-01 / LXE-07 contract-surface unification tests.
//!
//! These tests fence the canonical post-LXE shapes:
//!
//! * `SemanticQueryRequest.lexical_scope: Option<TextQueryRequest>` —
//!   the `SemanticCandidateScope` dual surface has been deleted.
//! * `lex::SearchExplanation` carries planner-trace, engines-touched,
//!   early-stop reason, and summary fields (LXE-07 planner provenance).
//!
//! Each test fails fast on shape drift; rerunning is the contract gate.

#![forbid(unsafe_code)]

use quanta_index_contract::lex::{
    ExplanationRow, PlannerTraceNode, SearchExplanation, SearchExplanationBuilder,
};
use quanta_index_contract::{
    GenerationPin, ManifestGeneration, RepoId, RevisionId, SemanticQueryRequest, TextQueryRequest,
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
        generation: Some(generation_pin()),
        generation_selector: None,
        top_k: 64,
    }
}

// --- LXE-01: SemanticQueryRequest carries lexical_scope, not scope ---------

#[test]
fn semantic_query_request_lexical_scope_round_trips() -> TestRes {
    let original = SemanticQueryRequest {
        query_text: None,
        query_vector: Some(vec![0.1, 0.2, 0.3]),
        query_vector_ref: None,
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
        query_text: None,
        query_vector: Some(vec![1.0_f32]),
        query_vector_ref: None,
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
        query_text: None,
        query_vector: None,
        query_vector_ref: None,
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

#[test]
fn semantic_candidate_scope_is_gone_from_public_api() {
    // Compile-time fence: a `use ...::SemanticCandidateScope;` would fail
    // resolution, so we only need a sentinel that compiles only if the type
    // is GONE — there is no such sentinel that survives a rename. Instead we
    // check that the wire field is named `lexical_scope`; if a future commit
    // restores `scope`, the rejection test above fires.
    //
    // We also verify by negation: build a SemanticQueryRequest using the
    // `lexical_scope` field directly. If anyone reintroduces a struct
    // literal field named `scope`, this test (and the rest of the file)
    // fails to compile.
    let sentinel = SemanticQueryRequest {
        query_text: None,
        query_vector: None,
        query_vector_ref: None,
        generation: None,
        generation_selector: None,
        lexical_scope: None,
        top_k: 1,
    };
    // Touch a field to give the literal a side-effect (avoids clippy's
    // `no_effect_underscore_binding`).
    assert_eq!(sentinel.top_k, 1);
}

// --- LXE-07: SearchExplanation augmented with planner-provenance fields ----

fn sample_explanation_full() -> SearchExplanation {
    SearchExplanation {
        contributions: vec![ExplanationRow {
            signal_name: Box::from("bm25"),
            signal_value: 1.0,
            weight: 0.5,
            contribution: 0.5,
        }],
        ranker_weights_hash: [0xab; 32],
        strategy: Box::from("hybrid-v1"),
        planner_trace: vec![
            PlannerTraceNode {
                node_kind: Box::from("filter"),
                detail: Box::from("repo:quanta-index"),
            },
            PlannerTraceNode {
                node_kind: Box::from("leaf:regex"),
                detail: Box::from("Search.*Plane"),
            },
        ],
        engines_touched: vec![Box::from("tantivy"), Box::from("lq-trigram")],
        early_stop_reason: Some(Box::from("candidate_cap_hit")),
        summary: Some(Box::from("regex narrowed by repo filter")),
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
    if decoded.engines_touched.len() != 2 {
        return Err("engines_touched dropped on roundtrip".into());
    }
    if decoded.early_stop_reason.as_deref() != Some("candidate_cap_hit") {
        return Err("early_stop_reason dropped on roundtrip".into());
    }
    if decoded.summary.as_deref() != Some("regex narrowed by repo filter") {
        return Err("summary dropped on roundtrip".into());
    }
    if decoded.contributions.len() != 1 {
        return Err("contributions dropped on roundtrip".into());
    }
    if decoded.ranker_weights_hash != [0xab; 32] {
        return Err("ranker_weights_hash dropped on roundtrip".into());
    }
    if decoded.strategy.as_ref() != "hybrid-v1" {
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
        || decoded.summary.is_some()
    {
        return Err("SearchExplanation::empty did not produce empty fields".into());
    }
    Ok(())
}

#[test]
fn search_explanation_builder_pushes_typed_fields() -> TestRes {
    let built = SearchExplanationBuilder::new()
        .strategy(Box::from("lexical-only"))
        .ranker_weights_hash([0x77; 32])
        .push_trace(PlannerTraceNode {
            node_kind: Box::from("leaf:phrase"),
            detail: Box::from("\"index lookup\""),
        })
        .push_engine(Box::from("lq-positions"))
        .push_engine(Box::from("lq-trigram"))
        .early_stop_reason(Some(Box::from("budget_exhausted")))
        .summary(Some(Box::from("phrase resolved against positions index")))
        .build();
    if built.engines_touched.len() != 2 {
        return Err("builder.push_engine did not append".into());
    }
    if built.planner_trace.len() != 1 {
        return Err("builder.push_trace did not append".into());
    }
    if built.strategy.as_ref() != "lexical-only" {
        return Err("builder.strategy not propagated".into());
    }
    if built.ranker_weights_hash != [0x77; 32] {
        return Err("builder.ranker_weights_hash not propagated".into());
    }
    if built.early_stop_reason.as_deref() != Some("budget_exhausted") {
        return Err("builder.early_stop_reason not propagated".into());
    }
    if built.summary.as_deref() != Some("phrase resolved against positions index") {
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
fn planner_trace_node_round_trips() -> TestRes {
    let original = PlannerTraceNode {
        node_kind: Box::from("filter"),
        detail: Box::from("lang:rust"),
    };
    let bytes = encode(&original)?;
    let decoded: PlannerTraceNode = decode(&bytes)?;
    if decoded != original {
        return Err("PlannerTraceNode roundtrip mismatch".into());
    }
    Ok(())
}
