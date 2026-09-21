//! QI-BB-022 — the hybrid result row carries its lane provenance, and the
//! wire refuses every shape that is not a hybrid ranking.
//!
//! Every refusal here is proven on the wire: a valid row or response is
//! encoded, one thing is changed in the encoded value, and the decoder must
//! name what is wrong. The encoder is held to the same invariants, so a
//! plane cannot emit what a client would refuse.

#![forbid(unsafe_code)]

use quanta_index_contract::{
    EngineTouched, GenerationPin, HybridCandidatePolicyErrorV1, HybridCandidateV1,
    HybridLaneContributionV1, HybridLaneV1, HybridQueryResponse, LexicalCandidate,
    ManifestGeneration, QueryResultWindowV1, RepoId, RepoRelativePath, RevisionId,
    SearchExplanation, SearchPlaneQueryIpcResponse, validate_hybrid_results_v1,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

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
fn lexical_row(id: &str, score: f32) -> LexicalCandidate {
    LexicalCandidate {
        candidate_id: id.to_owned(),
        repo_id: RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-1")
            .expect("static fixture ID satisfies canonical policy"),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/search.rs"),
        start_line: 10,
        end_line: 18,
        score,
        snippet: "fn search_plane() {}".to_owned(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    }
}

fn contribution(lane: HybridLaneV1, rank: u32, raw_score: f32) -> HybridLaneContributionV1 {
    HybridLaneContributionV1 {
        lane,
        rank,
        raw_score,
    }
}

/// The RRF sum the plane would carry for these ranks, under its k = 60.
fn rrf(ranks: &[u32]) -> f64 {
    ranks
        .iter()
        .map(|rank| 1.0 / (60.0 + f64::from(*rank)))
        .sum()
}

fn both_lanes(id: &str, lexical_rank: u32, dense_rank: u32) -> HybridCandidateV1 {
    HybridCandidateV1 {
        candidate: lexical_row(id, 2.5),
        fused_score: rrf(&[lexical_rank, dense_rank]),
        contributions: vec![
            contribution(HybridLaneV1::Lexical, lexical_rank, 2.5),
            contribution(HybridLaneV1::Dense, dense_rank, 0.75),
        ],
    }
}

fn dense_only(id: &str, dense_rank: u32) -> HybridCandidateV1 {
    HybridCandidateV1 {
        candidate: lexical_row(id, -0.125),
        fused_score: rrf(&[dense_rank]),
        contributions: vec![contribution(HybridLaneV1::Dense, dense_rank, -0.125)],
    }
}

fn lexical_only(id: &str, lexical_rank: u32) -> HybridCandidateV1 {
    HybridCandidateV1 {
        candidate: lexical_row(id, 1.5),
        fused_score: rrf(&[lexical_rank]),
        contributions: vec![contribution(HybridLaneV1::Lexical, lexical_rank, 1.5)],
    }
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn response(results: Vec<HybridCandidateV1>) -> SearchPlaneQueryIpcResponse {
    let returned = u32::try_from(results.len()).map_or(u32::MAX, |n| n);
    SearchPlaneQueryIpcResponse::Hybrid(HybridQueryResponse {
        generation: GenerationPin::new(
            RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("rev-1").expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(7),
        ),
        results,
        window: QueryResultWindowV1::exact(returned),
        explanation: SearchExplanation {
            planner_trace: Vec::new(),
            engines_touched: vec![EngineTouched::Lexical, EngineTouched::Semantic],
            early_stop_reason: None,
            contributions: Vec::new(),
            ranker_weights_hash: [0u8; 32],
            strategy: "rrf".to_owned(),
            summary: "two lanes".to_owned(),
        },
    })
}

/// Decode `bytes` as `T` and require a refusal naming `fragment`.
fn expect_refusal<T>(bytes: &[u8], fragment: &str) -> TestResult
where
    T: for<'de> serde::Deserialize<'de> + core::fmt::Debug,
{
    match ciborium::de::from_reader::<T, _>(bytes) {
        Ok(decoded) => Err(format!("decode must refuse ({fragment}); got {decoded:?}").into()),
        Err(err) if err.to_string().contains(fragment) => Ok(()),
        Err(err) => Err(format!("refusal must name `{fragment}`; got {err}").into()),
    }
}

/// Encode a valid row, apply `mutate` to its CBOR map, and re-encode.
fn mutated_row<F>(row: &HybridCandidateV1, mutate: F) -> Result<Vec<u8>, Box<dyn std::error::Error>>
where
    F: FnOnce(&mut Vec<(ciborium::Value, ciborium::Value)>) -> TestResult,
{
    let mut wire: ciborium::Value = decode(&encode(row)?)?;
    let ciborium::Value::Map(fields) = &mut wire else {
        return Err("a hybrid row encodes as a map".into());
    };
    mutate(fields)?;
    encode(&wire)
}

fn text(name: &str) -> ciborium::Value {
    ciborium::Value::Text(name.to_owned())
}

fn set_field(
    fields: &mut [(ciborium::Value, ciborium::Value)],
    name: &str,
    value: ciborium::Value,
) -> TestResult {
    let slot = fields
        .iter_mut()
        .find(|(key, _)| *key == text(name))
        .ok_or_else(|| format!("field `{name}` present"))?;
    slot.1 = value;
    Ok(())
}

fn contributions_mut(
    fields: &mut [(ciborium::Value, ciborium::Value)],
) -> Result<&mut Vec<ciborium::Value>, Box<dyn std::error::Error>> {
    let slot = fields
        .iter_mut()
        .find(|(key, _)| *key == text("contributions"))
        .ok_or("contributions present")?;
    let ciborium::Value::Array(items) = &mut slot.1 else {
        return Err("contributions encode as an array".into());
    };
    Ok(items)
}

fn first_contribution_mut(
    fields: &mut [(ciborium::Value, ciborium::Value)],
) -> Result<&mut Vec<(ciborium::Value, ciborium::Value)>, Box<dyn std::error::Error>> {
    let first = contributions_mut(fields)?
        .first_mut()
        .ok_or("one contribution")?;
    let ciborium::Value::Map(contribution) = first else {
        return Err("a contribution encodes as a map".into());
    };
    Ok(contribution)
}

#[test]
fn hybrid_rows_round_trip_through_cbor_and_json_for_every_lane_shape() -> TestResult {
    for row in [
        both_lanes("both", 1, 3),
        dense_only("dense", 2),
        lexical_only("lexical", 4),
    ] {
        let cbor: HybridCandidateV1 = decode(&encode(&row)?)?;
        if cbor != row {
            return Err(format!("cbor round trip: {row:?} != {cbor:?}").into());
        }
        let json = serde_json::to_string(&row)?;
        let back: HybridCandidateV1 = serde_json::from_str(&json)?;
        if back != row {
            return Err(format!("json round trip: {row:?} != {back:?}").into());
        }
    }
    // The wire shape is the documented one: the lane row, the f64 ranking
    // key, and string-tagged lanes.
    let json: serde_json::Value = serde_json::to_value(both_lanes("both", 1, 3))?;
    let lanes = json
        .pointer("/contributions")
        .and_then(serde_json::Value::as_array)
        .ok_or("contributions array")?
        .iter()
        .map(|entry| entry.pointer("/lane").cloned())
        .collect::<Vec<_>>();
    if lanes != [Some("Lexical".into()), Some("Dense".into())] {
        return Err(format!("lanes are string-tagged in lane order: {json}").into());
    }
    // JSON carries the f64 exactly (shortest round-trip representation).
    if json.pointer("/candidate/candidate_id") != Some(&"both".into())
        || json
            .pointer("/fused_score")
            .and_then(serde_json::Value::as_f64)
            .map(f64::to_bits)
            != Some(rrf(&[1, 3]).to_bits())
    {
        return Err(format!("row shape: {json}").into());
    }
    Ok(())
}

#[test]
fn a_hybrid_response_round_trips_in_ranking_order() -> TestResult {
    // fused desc; on a tie the lexical-seen row first; then id asc.
    let ordered = vec![
        both_lanes("b", 1, 1),
        lexical_only("d", 1),
        dense_only("e", 1),
        dense_only("a", 2),
    ];
    let wire = response(ordered.clone());
    let decoded: SearchPlaneQueryIpcResponse = decode(&encode(&wire)?)?;
    if decoded != wire {
        return Err(format!("response round trip: {wire:?} != {decoded:?}").into());
    }
    validate_hybrid_results_v1(&ordered)?;
    Ok(())
}

#[test]
fn every_row_invariant_is_refused_on_decode_and_on_encode() -> TestResult {
    let row = both_lanes("both", 1, 3);
    // Zero contributions.
    let bytes = mutated_row(&row, |fields| {
        contributions_mut(fields)?.clear();
        Ok(())
    })?;
    expect_refusal::<HybridCandidateV1>(&bytes, "one or two lane contributions")?;
    // Three contributions.
    let bytes = mutated_row(&row, |fields| {
        let items = contributions_mut(fields)?;
        let extra = items.first().cloned().ok_or("one contribution")?;
        items.push(extra);
        Ok(())
    })?;
    expect_refusal::<HybridCandidateV1>(&bytes, "one or two lane contributions")?;
    // The same lane twice.
    let bytes = mutated_row(&row, |fields| {
        let items = contributions_mut(fields)?;
        let first = items.first().cloned().ok_or("one contribution")?;
        *items = vec![first.clone(), first];
        Ok(())
    })?;
    expect_refusal::<HybridCandidateV1>(&bytes, "lane order")?;
    // Lanes out of order (dense before lexical); the row's score still
    // matches the lexical raw score, so lane order is the only defect.
    let bytes = mutated_row(&row, |fields| {
        contributions_mut(fields)?.reverse();
        Ok(())
    })?;
    expect_refusal::<HybridCandidateV1>(&bytes, "lane order")?;
    // Rank zero.
    let bytes =
        mutated_row(&row, |fields| set_field(first_contribution_mut(fields)?, "rank", 0.into()))?;
    expect_refusal::<HybridCandidateV1>(&bytes, "rank must be at least 1")?;
    // A non-finite raw score.
    let bytes = mutated_row(&row, |fields| {
        set_field(first_contribution_mut(fields)?, "raw_score", ciborium::Value::Float(f64::NAN))
    })?;
    expect_refusal::<HybridCandidateV1>(&bytes, "raw_score must be finite")?;
    // A fused score that is zero, negative, or not finite.
    for bad in [0.0, -0.5, f64::INFINITY, f64::NAN] {
        let bytes = mutated_row(&row, |fields| {
            set_field(fields, "fused_score", ciborium::Value::Float(bad))
        })?;
        expect_refusal::<HybridCandidateV1>(&bytes, "fused_score must be finite and positive")?;
    }
    // The row's score is not its preferred lane's raw score.
    let bytes = mutated_row(&row, |fields| {
        let candidate = fields
            .iter_mut()
            .find(|(key, _)| *key == text("candidate"))
            .ok_or("candidate present")?;
        let ciborium::Value::Map(candidate_fields) = &mut candidate.1 else {
            return Err("candidate encodes as a map".into());
        };
        set_field(candidate_fields, "score", ciborium::Value::Float(9.0))
    })?;
    expect_refusal::<HybridCandidateV1>(&bytes, "not its preferred lane's raw_score")?;
    // Field discipline on the contribution: unknown, duplicate, missing.
    let bytes = mutated_row(&row, |fields| {
        first_contribution_mut(fields)?.push((text("corpus_kind"), text("raw_code")));
        Ok(())
    })?;
    expect_refusal::<HybridCandidateV1>(&bytes, "unknown field")?;
    let bytes = mutated_row(&row, |fields| {
        let contribution = first_contribution_mut(fields)?;
        let rank = contribution
            .iter()
            .find(|(key, _)| *key == text("rank"))
            .cloned()
            .ok_or("rank present")?;
        contribution.push(rank);
        Ok(())
    })?;
    expect_refusal::<HybridCandidateV1>(&bytes, "duplicate field")?;
    let bytes = mutated_row(&row, |fields| {
        first_contribution_mut(fields)?.retain(|(key, _)| *key != text("raw_score"));
        Ok(())
    })?;
    expect_refusal::<HybridCandidateV1>(&bytes, "missing field")?;
    // An unknown lane (a seed lane is not a hybrid lane).
    let bytes = mutated_row(&row, |fields| {
        set_field(first_contribution_mut(fields)?, "lane", text("Bm25"))
    })?;
    expect_refusal::<HybridCandidateV1>(&bytes, "unknown variant")?;
    // Field discipline on the row itself.
    let bytes = mutated_row(&row, |fields| {
        fields.push((text("lane"), text("Lexical")));
        Ok(())
    })?;
    expect_refusal::<HybridCandidateV1>(&bytes, "unknown field")?;
    let bytes = mutated_row(&row, |fields| {
        fields.retain(|(key, _)| *key != text("fused_score"));
        Ok(())
    })?;
    expect_refusal::<HybridCandidateV1>(&bytes, "missing field")?;
    // The encoder refuses the same defects, so a plane cannot emit them.
    let mut unfused = both_lanes("both", 1, 3);
    unfused.contributions.clear();
    if encode(&unfused).is_ok() {
        return Err("a row without contributions must not encode".into());
    }
    let mut mispriced = both_lanes("both", 1, 3);
    mispriced.candidate.score = 9.0;
    if encode(&mispriced).is_ok() {
        return Err("a row whose score is not its lane's must not encode".into());
    }
    Ok(())
}

#[test]
fn the_response_refuses_a_list_that_is_not_a_hybrid_ranking() -> TestResult {
    // Out of fused order.
    let wire = response(vec![dense_only("a", 2), both_lanes("b", 1, 1)]);
    let bytes = encode(&wire);
    if bytes.is_ok() {
        return Err("an unranked response must not encode".into());
    }
    let ranked = response(vec![both_lanes("b", 1, 1), dense_only("a", 2)]);
    let mut value: ciborium::Value = decode(&encode(&ranked)?)?;
    swap_first_two_results(&mut value)?;
    expect_refusal::<SearchPlaneQueryIpcResponse>(&encode(&value)?, "ranking order")?;
    // Equal fused scores: the lexical-seen row must come first ...
    let tie = response(vec![dense_only("a", 1), lexical_only("b", 1)]);
    if encode(&tie).is_ok() {
        return Err("a tie must put the lexical-seen row first".into());
    }
    if !matches!(
        validate_hybrid_results_v1(&[dense_only("a", 1), lexical_only("b", 1)]),
        Err(HybridCandidatePolicyErrorV1::ResultsNotInRankingOrder { position: 1 })
    ) {
        return Err("the tie-break defect is named at its position".into());
    }
    // ... and among rows the same lanes saw, ids ascend.
    let tie = response(vec![lexical_only("b", 1), lexical_only("a", 1)]);
    if encode(&tie).is_ok() {
        return Err("tied rows must be in id order".into());
    }
    // An identity twice.
    let repeated = vec![both_lanes("x", 1, 1), lexical_only("x", 5)];
    if !matches!(
        validate_hybrid_results_v1(&repeated),
        Err(HybridCandidatePolicyErrorV1::DuplicateCandidateId { .. })
    ) {
        return Err("a repeated identity is named".into());
    }
    let ranked = response(vec![both_lanes("x", 1, 1), lexical_only("y", 5)]);
    let mut value: ciborium::Value = decode(&encode(&ranked)?)?;
    rename_second_result(&mut value, "x")?;
    expect_refusal::<SearchPlaneQueryIpcResponse>(&encode(&value)?, "more than once")?;
    Ok(())
}

fn results_mut(
    value: &mut ciborium::Value,
) -> Result<&mut Vec<ciborium::Value>, Box<dyn std::error::Error>> {
    let ciborium::Value::Map(response_fields) = value else {
        return Err("a response encodes as a map".into());
    };
    let payload = response_fields
        .iter_mut()
        .find(|(key, _)| *key == text("payload"))
        .ok_or("payload present")?;
    let ciborium::Value::Map(payload_fields) = &mut payload.1 else {
        return Err("payload encodes as a map".into());
    };
    let results = payload_fields
        .iter_mut()
        .find(|(key, _)| *key == text("results"))
        .ok_or("results present")?;
    let ciborium::Value::Array(rows) = &mut results.1 else {
        return Err("results encode as an array".into());
    };
    Ok(rows)
}

fn swap_first_two_results(value: &mut ciborium::Value) -> TestResult {
    let rows = results_mut(value)?;
    if rows.len() < 2 {
        return Err("two rows".into());
    }
    rows.swap(0, 1);
    Ok(())
}

fn rename_second_result(value: &mut ciborium::Value, id: &str) -> TestResult {
    let rows = results_mut(value)?;
    let second = rows.get_mut(1).ok_or("two rows")?;
    let ciborium::Value::Map(row_fields) = second else {
        return Err("a row encodes as a map".into());
    };
    let candidate = row_fields
        .iter_mut()
        .find(|(key, _)| *key == text("candidate"))
        .ok_or("candidate present")?;
    let ciborium::Value::Map(candidate_fields) = &mut candidate.1 else {
        return Err("candidate encodes as a map".into());
    };
    set_field(candidate_fields, "candidate_id", text(id))
}
