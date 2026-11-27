//! Typed file-ranking result for the explicitly selected Go test trace route.

use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeStruct};

use crate::{
    GenerationPin, GoTestTraceFileQueryRequestV1, GoTestTraceSourceFramesV1, LexicalCandidate,
    QueryResultWindowV2, SearchCorpusActiveHeadV1, SourceFileKey,
};

use super::SearchExplanation;

/// Whether raw failure text contains zero, one, or multiple parsed source-frame hints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GoTestTraceFrameClassV1 {
    Unresolved,
    Exact,
    Ambiguous,
}

impl Serialize for GoTestTraceFrameClassV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (index, name) = match self {
            Self::Unresolved => (0, "unresolved"),
            Self::Exact => (1, "exact"),
            Self::Ambiguous => (2, "ambiguous"),
        };
        serializer.serialize_unit_variant("GoTestTraceFrameClassV1", index, name)
    }
}

impl<'de> Deserialize<'de> for GoTestTraceFrameClassV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FrameVisitor;

        impl<'de> de::Visitor<'de> for FrameVisitor {
            type Value = GoTestTraceFrameClassV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a Go trace frame class")
            }

            fn visit_enum<A: de::EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
                let (name, variant) = de::EnumAccess::variant_seed(
                    data,
                    quanta_index_contract_base::ClosedFieldKeySeedV1(&VARIANTS).serde_identifier(),
                )?;
                let value = match name {
                    "unresolved" => GoTestTraceFrameClassV1::Unresolved,
                    "exact" => GoTestTraceFrameClassV1::Exact,
                    "ambiguous" => GoTestTraceFrameClassV1::Ambiguous,
                    _ => unreachable!("closed variant seed only returns schema names"),
                };
                de::VariantAccess::unit_variant(variant)?;
                Ok(value)
            }
        }

        const VARIANTS: [&str; 3] = ["unresolved", "exact", "ambiguous"];
        deserializer.deserialize_enum("GoTestTraceFrameClassV1", &VARIANTS, FrameVisitor)
    }
}

impl GoTestTraceFrameClassV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unresolved => "unresolved",
            Self::Exact => "exact",
            Self::Ambiguous => "ambiguous",
        }
    }
}

impl GoTestTraceSourceFramesV1 {
    #[must_use]
    pub fn frame_class(&self) -> GoTestTraceFrameClassV1 {
        match self.paths().len() {
            0 => GoTestTraceFrameClassV1::Unresolved,
            1 => GoTestTraceFrameClassV1::Exact,
            _ => GoTestTraceFrameClassV1::Ambiguous,
        }
    }
}

/// First returned chunk of a file in one lane; ranks are one-based.
#[derive(Clone, Debug, PartialEq)]
pub struct GoTestTraceFileLaneWitnessV1 {
    pub file_rank: u32,
    pub chunk_rank: u32,
    pub candidate: LexicalCandidate,
}

quanta_index_contract_base::native_serde_struct!([serde_identifier] GoTestTraceFileLaneWitnessV1 {
    file_rank: u32,
    chunk_rank: u32,
    candidate: LexicalCandidate,
});

/// Minimal ordered chunk evidence from one bounded file-ranking lane.
#[derive(Clone, Debug, PartialEq)]
pub struct GoTestTraceFilePoolRowV1 {
    pub file: SourceFileKey,
    pub candidate_id: String,
    pub score_bits: u32,
}

quanta_index_contract_base::native_serde_struct!([serde_identifier] GoTestTraceFilePoolRowV1 {
    file: SourceFileKey,
    candidate_id: String,
    score_bits: u32,
});

/// The exact two chunk prefixes consumed by the Go trace file ranker.
#[derive(Clone, Debug, PartialEq)]
pub struct GoTestTraceFileRankPoolsV1 {
    pub lexical: Vec<GoTestTraceFilePoolRowV1>,
    pub dense: Vec<GoTestTraceFilePoolRowV1>,
}

quanta_index_contract_base::native_serde_struct!([serde_identifier] GoTestTraceFileRankPoolsV1 {
    lexical: Vec<GoTestTraceFilePoolRowV1>,
    dense: Vec<GoTestTraceFilePoolRowV1>,
});

fn first_pool_witnesses(
    rows: &[GoTestTraceFilePoolRowV1],
    returned: u32,
) -> Result<BTreeMap<&SourceFileKey, (u32, u32, &GoTestTraceFilePoolRowV1)>, String> {
    if rows.len() != usize::try_from(returned).map_err(|error| error.to_string())? {
        return Err("Go trace rank pool count disagrees with lane window".to_string());
    }
    let mut first = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        if row.file.validate().is_err()
            || row.candidate_id.is_empty()
            || !f32::from_bits(row.score_bits).is_finite()
        {
            return Err("Go trace rank pool row is invalid".to_string());
        }
        let file_rank = u32::try_from(first.len().saturating_add(1))
            .map_err(|error| format!("Go trace file rank overflow: {error}"))?;
        let chunk_rank = u32::try_from(index.saturating_add(1))
            .map_err(|error| format!("Go trace chunk rank overflow: {error}"))?;
        if let Entry::Vacant(entry) = first.entry(&row.file) {
            let _inserted = entry.insert((file_rank, chunk_rank, row));
        }
    }
    Ok(first)
}

/// Check only positions observable in a returned file page. A cut page may
/// omit earlier lane files, but visible file ranks must retain chunk order.
pub(super) fn validate_file_witness_positions(
    positions: &mut [(u32, u32)],
    lane_returned: u32,
    complete_union: bool,
) -> Result<(), String> {
    if complete_union && positions.is_empty() != (lane_returned == 0) {
        return Err("complete file page omits a lane file rank".to_string());
    }
    positions.sort_unstable_by_key(|(file_rank, _)| *file_rank);
    let (mut previous_file, mut previous_chunk) = (0, 0);
    for &(file_rank, chunk_rank) in positions.iter() {
        let file_gap = file_rank.saturating_sub(previous_file);
        if file_rank == 1 && chunk_rank != 1 {
            return Err("file rank one must witness chunk rank one".to_string());
        }
        if file_gap == 0 || chunk_rank.saturating_sub(previous_chunk) < file_gap {
            return Err("file witness chunk order contradicts file rank".to_string());
        }
        if complete_union && file_gap != 1 {
            return Err("complete file page omits a lane file rank".to_string());
        }
        (previous_file, previous_chunk) = (file_rank, chunk_rank);
    }
    Ok(())
}

/// Distinct file with a file-RRF score and the original lane evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct GoTestTraceFileHitV1 {
    pub file: SourceFileKey,
    pub lexical: Option<GoTestTraceFileLaneWitnessV1>,
    pub dense: Option<GoTestTraceFileLaneWitnessV1>,
    pub file_rrf_score: f64,
    pub production_source: bool,
    /// Query-bound parsed hint; does not certify a call-stack frame.
    pub source_frame: bool,
}

quanta_index_contract_base::native_serde_struct!([serde_identifier] GoTestTraceFileHitV1 {
    file: SourceFileKey,
    lexical: Option<GoTestTraceFileLaneWitnessV1>,
    dense: Option<GoTestTraceFileLaneWitnessV1>,
    file_rrf_score: f64,
    production_source: bool,
    source_frame: bool,
});

impl GoTestTraceFileHitV1 {
    /// The lexical lane owns the display span when both lanes saw this file.
    #[must_use]
    pub fn representative(&self) -> Option<&LexicalCandidate> {
        self.lexical
            .as_ref()
            .or(self.dense.as_ref())
            .map(|w| &w.candidate)
    }
}

/// File projection plus each input lane's own completeness authority.
/// No file window is claimed: truncation of the union cannot prove exhaustion.
#[derive(Clone, Debug, PartialEq)]
pub struct GoTestTraceFileQueryResponseV1 {
    pub generation: GenerationPin,
    pub selected_active_head: Option<SearchCorpusActiveHeadV1>,
    pub results: Vec<GoTestTraceFileHitV1>,
    pub lexical_window: QueryResultWindowV2,
    pub dense_window: QueryResultWindowV2,
    pub rank_pools: Option<GoTestTraceFileRankPoolsV1>,
    pub frame_class: GoTestTraceFrameClassV1,
    pub explanation: SearchExplanation,
}

struct WireResponse {
    generation: GenerationPin,
    selected_active_head: Option<SearchCorpusActiveHeadV1>,
    results: Vec<GoTestTraceFileHitV1>,
    lexical_window: QueryResultWindowV2,
    dense_window: QueryResultWindowV2,
    rank_pools: Option<GoTestTraceFileRankPoolsV1>,
    frame_class: GoTestTraceFrameClassV1,
    explanation: SearchExplanation,
}

quanta_index_contract_base::native_serde_struct!([serde_identifier] WireResponse {
    generation: GenerationPin,
    selected_active_head: Option<SearchCorpusActiveHeadV1>,
    results: Vec<GoTestTraceFileHitV1>,
    lexical_window: QueryResultWindowV2,
    dense_window: QueryResultWindowV2,
    rank_pools: Option<GoTestTraceFileRankPoolsV1> = default,
    frame_class: GoTestTraceFrameClassV1,
    explanation: SearchExplanation,
});

impl From<WireResponse> for GoTestTraceFileQueryResponseV1 {
    fn from(response: WireResponse) -> Self {
        Self {
            generation: response.generation,
            selected_active_head: response.selected_active_head,
            results: response.results,
            lexical_window: response.lexical_window,
            dense_window: response.dense_window,
            rank_pools: response.rank_pools,
            frame_class: response.frame_class,
            explanation: response.explanation,
        }
    }
}

impl GoTestTraceFileQueryResponseV1 {
    pub fn validate_v1(&self) -> Result<(), String> {
        self.validate_file_page(false)?;
        self.validate_rank_pools()
    }

    fn validate_rank_pools(&self) -> Result<(), String> {
        let Some(pools) = &self.rank_pools else {
            return Ok(());
        };
        let lexical = first_pool_witnesses(&pools.lexical, self.lexical_window.returned())?;
        let dense = first_pool_witnesses(&pools.dense, self.dense_window.returned())?;
        if self.results.is_empty() && (!lexical.is_empty() || !dense.is_empty()) {
            return Err("Go trace rank pools contain files but the result is empty".to_string());
        }
        for hit in &self.results {
            for (witness, first) in [
                (hit.lexical.as_ref(), &lexical),
                (hit.dense.as_ref(), &dense),
            ] {
                match (witness, first.get(&hit.file)) {
                    (Some(witness), Some((file_rank, chunk_rank, row)))
                        if witness.file_rank == *file_rank
                            && witness.chunk_rank == *chunk_rank
                            && witness.candidate.candidate_id == row.candidate_id
                            && witness.candidate.score.to_bits() == row.score_bits => {}
                    (None, None) => {}
                    _ => return Err("Go trace file witness disagrees with rank pool".to_string()),
                }
            }
        }
        Ok(())
    }

    fn validate_file_page(&self, complete_union: bool) -> Result<(), String> {
        let mut files = BTreeSet::new();
        let mut lexical_ranks = BTreeSet::new();
        let mut dense_ranks = BTreeSet::new();
        let mut lexical_positions = Vec::new();
        let mut dense_positions = Vec::new();
        let mut rank_scores = Vec::with_capacity(self.results.len());
        let mut source_frames = 0_u32;
        for hit in &self.results {
            if hit.file.validate().is_err() {
                return Err("Go trace file identity is invalid".to_string());
            }
            if !files.insert(&hit.file) {
                return Err("Go trace file response repeats a source file".to_string());
            }
            if hit.representative().is_none() {
                return Err("Go trace file hit has no lane witness".to_string());
            }
            let path = hit.file.repo_relative_path.as_str();
            let is_source = path.strip_suffix(".go").is_some() && !path.ends_with("_test.go");
            if hit.production_source != is_source || (hit.source_frame && !is_source) {
                return Err("Go trace file source flags disagree with path".to_string());
            }
            if hit.source_frame {
                source_frames = source_frames
                    .checked_add(1)
                    .ok_or("Go trace source frame count overflow")?;
            }
            let mut expected_score = 0.0;
            for (witness, ranks, positions, returned) in [
                (
                    hit.lexical.as_ref(),
                    &mut lexical_ranks,
                    &mut lexical_positions,
                    self.lexical_window.returned(),
                ),
                (
                    hit.dense.as_ref(),
                    &mut dense_ranks,
                    &mut dense_positions,
                    self.dense_window.returned(),
                ),
            ] {
                let Some(witness) = witness else { continue };
                if witness.file_rank == 0
                    || witness.chunk_rank == 0
                    || witness.file_rank > witness.chunk_rank
                    || witness.chunk_rank > returned
                    || !ranks.insert(witness.file_rank)
                    || witness.candidate.repo_id != self.generation.repo_id
                    || witness.candidate.revision_id != self.generation.revision_id
                    || witness.candidate.manifest_generation != self.generation.manifest_generation
                    || witness.candidate.source_repo_id != hit.file.source_repo_id
                    || witness.candidate.repo_relative_path != hit.file.repo_relative_path
                    || witness.candidate.validate_source_metadata().is_err()
                    || !witness.candidate.score.is_finite()
                {
                    return Err("Go trace file lane witness is invalid".to_string());
                }
                positions.push((witness.file_rank, witness.chunk_rank));
                expected_score += 1.0 / (60.0 + f64::from(witness.file_rank));
            }
            if hit.file_rrf_score.to_bits() != expected_score.to_bits() {
                return Err("Go trace file RRF score disagrees with lane ranks".to_string());
            }
            rank_scores.push(expected_score);
        }
        validate_file_witness_positions(
            &mut lexical_positions,
            self.lexical_window.returned(),
            complete_union,
        )?;
        validate_file_witness_positions(
            &mut dense_positions,
            self.dense_window.returned(),
            complete_union,
        )?;
        if self.frame_class != GoTestTraceFrameClassV1::Exact && source_frames != 0
            || source_frames > 1
        {
            return Err("Go trace frame class disagrees with promoted files".to_string());
        }
        for (pair, scores) in self.results.windows(2).zip(rank_scores.windows(2)) {
            let [left, right] = pair else {
                return Err("Go trace files are not in rank order".to_string());
            };
            let [left_score, right_score] = scores else {
                return Err("Go trace files are not in rank order".to_string());
            };
            let order = right
                .source_frame
                .cmp(&left.source_frame)
                .then(right_score.total_cmp(left_score))
                .then(right.lexical.is_some().cmp(&left.lexical.is_some()))
                .then(left.file.cmp(&right.file));
            if order.is_gt() {
                return Err("Go trace files are not in rank order".to_string());
            }
        }
        Ok(())
    }

    pub fn validate_against_v1(
        &self,
        request: &GoTestTraceFileQueryRequestV1,
    ) -> Result<(), String> {
        request.validate_v1()?;
        let result_count = u32::try_from(self.results.len())
            .map_err(|error| format!("Go trace file response result count overflow: {error}"))?;
        if self.rank_pools.is_some() != request.include_rank_pools {
            return Err("Go trace rank pool presence disagrees with request".to_string());
        }
        self.validate_file_page(result_count < request.top_k)?;
        self.validate_rank_pools()?;
        if let Some(pools) = &self.rank_pools {
            let distinct_files = pools
                .lexical
                .iter()
                .chain(&pools.dense)
                .map(|row| &row.file)
                .collect::<BTreeSet<_>>();
            let file_cap = usize::try_from(request.top_k)
                .map_err(|error| format!("Go trace file top_k overflow: {error}"))?;
            if self.results.len() != distinct_files.len().min(file_cap) {
                return Err("Go trace result count disagrees with bounded file union".to_string());
            }
        }
        let frames = request.failure_module_context.as_ref().map_or_else(
            || GoTestTraceSourceFramesV1::from_failure_excerpt(&request.failure_excerpt),
            |module| {
                GoTestTraceSourceFramesV1::from_module_cache_panic(&request.failure_excerpt, module)
            },
        );
        if self.frame_class != frames.frame_class() {
            return Err("Go trace frame class disagrees with failure_excerpt".to_string());
        }
        for hit in &self.results {
            let expected_frame = frames.frame_class() == GoTestTraceFrameClassV1::Exact
                && request
                    .failure_source_repo_id
                    .as_ref()
                    .is_some_and(|owner| owner == &hit.file.source_repo_id)
                && frames.contains_path(hit.file.repo_relative_path.as_str());
            if hit.source_frame != expected_frame {
                return Err("Go trace source_frame disagrees with failure_excerpt".to_string());
            }
        }
        if result_count > request.top_k {
            return Err("Go trace file response exceeds top_k".to_string());
        }
        if self.lexical_window.returned() > request.query.top_k
            || self.dense_window.returned() > request.query.top_k
        {
            return Err("Go trace lane window exceeds its chunk cap".to_string());
        }
        if request
            .query
            .generation
            .as_ref()
            .is_some_and(|pin| pin != &self.generation)
            || request
                .query
                .text_query
                .generation
                .as_ref()
                .is_some_and(|pin| pin != &self.generation)
        {
            return Err("Go trace file response generation drift".to_string());
        }
        Ok(())
    }
}

impl Serialize for GoTestTraceFileQueryResponseV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        let field_count = 7usize
            .checked_add(usize::from(self.rank_pools.is_some()))
            .ok_or_else(|| serde::ser::Error::custom("Go trace response field count overflow"))?;
        let mut state =
            serializer.serialize_struct("GoTestTraceFileQueryResponseV1", field_count)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("selected_active_head", &self.selected_active_head)?;
        state.serialize_field("results", &self.results)?;
        state.serialize_field("lexical_window", &self.lexical_window)?;
        state.serialize_field("dense_window", &self.dense_window)?;
        if let Some(pools) = &self.rank_pools {
            state.serialize_field("rank_pools", pools)?;
        }
        state.serialize_field("frame_class", &self.frame_class)?;
        state.serialize_field("explanation", &self.explanation)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for GoTestTraceFileQueryResponseV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let response = Self::from(WireResponse::deserialize(deserializer)?);
        response.validate_v1().map_err(de::Error::custom)?;
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        HybridQueryRequest, ManifestGeneration, QueryConstraintSetV1, RepoId, RepoRelativePath,
        RevisionId, TextQueryRequest, TextQuerySyntax,
    };

    fn file(source_repo_id: &str, path: &str) -> SourceFileKey {
        SourceFileKey {
            source_repo_id: RepoId::new(source_repo_id).expect("source repo"),
            repo_relative_path: RepoRelativePath::new(path),
        }
    }

    fn pin() -> GenerationPin {
        GenerationPin::new(
            RepoId::new("trace-file").expect("repo"),
            RevisionId::new("rev-1").expect("revision"),
            ManifestGeneration::new(1),
        )
    }

    fn candidate(path: &str, score: f32) -> LexicalCandidate {
        let generation = pin();
        LexicalCandidate {
            source_repo_id: generation.repo_id.clone(),
            source: None,
            preview: None,
            candidate_id: format!("chunk:{path}"),
            repo_id: generation.repo_id,
            revision_id: generation.revision_id,
            manifest_generation: generation.manifest_generation,
            repo_relative_path: RepoRelativePath::new(path),
            lines: Some(crate::LexicalLineRange {
                start_line: 1,
                end_line: 2,
            }),
            excerpt: Some(crate::LexicalMatchExcerpt {
                snippet: String::new(),
                snippet_hit_offset: None,
                highlights: Vec::new(),
            }),
            score,
            native_source: None,
        }
    }

    fn response() -> GoTestTraceFileQueryResponseV1 {
        GoTestTraceFileQueryResponseV1 {
            generation: pin(),
            selected_active_head: None,
            results: vec![GoTestTraceFileHitV1 {
                file: file("trace-file", "binding.go"),
                lexical: Some(GoTestTraceFileLaneWitnessV1 {
                    file_rank: 1,
                    chunk_rank: 1,
                    candidate: candidate("binding.go", 4.0),
                }),
                dense: Some(GoTestTraceFileLaneWitnessV1 {
                    file_rank: 1,
                    chunk_rank: 1,
                    candidate: candidate("binding.go", 0.8),
                }),
                file_rrf_score: 2.0 / 61.0,
                production_source: true,
                source_frame: true,
            }],
            lexical_window: QueryResultWindowV2::exact_probe(3),
            dense_window: QueryResultWindowV2::exact_probe(3),
            rank_pools: None,
            frame_class: GoTestTraceFrameClassV1::Exact,
            explanation: SearchExplanation::empty(),
        }
    }

    fn request(top_k: u32) -> GoTestTraceFileQueryRequestV1 {
        GoTestTraceFileQueryRequestV1 {
            query: HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "panic".into(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin()),
                    generation_selector: None,
                    top_k: 3,
                    cursor: None,
                },
                semantic_query_text: "panic".into(),
                generation: Some(pin()),
                generation_selector: None,
                top_k: 3,
            },
            failure_excerpt: "binding.go:12".into(),
            failure_source_repo_id: Some(pin().repo_id),
            failure_module_context: None,
            top_k,
            include_rank_pools: false,
        }
    }

    #[test]
    fn response_roundtrips_with_separate_lane_windows() {
        let expected = response();
        let encoded = serde_json::to_string(&expected).expect("response encode");
        assert!(!encoded.contains("rank_pools"));
        assert_eq!(
            serde_json::from_str::<GoTestTraceFileQueryResponseV1>(&encoded)
                .expect("response decode"),
            expected
        );
        assert!(expected.results[0].representative().is_some());
        assert_eq!(expected.frame_class.as_str(), "exact");
    }

    #[test]
    fn trace_wire_keeps_required_nulls_optional_pools_and_cbor_field_order() {
        let expected = response();
        let mut json = serde_json::to_value(&expected).expect("response JSON");
        let object = json.as_object_mut().expect("response object");
        assert_eq!(
            object.get("selected_active_head"),
            Some(&serde_json::Value::Null)
        );
        assert!(!object.contains_key("rank_pools"));

        assert_eq!(
            object.insert("rank_pools".into(), serde_json::Value::Null),
            None
        );
        assert_eq!(
            serde_json::from_value::<GoTestTraceFileQueryResponseV1>(json.clone())
                .expect("explicit null pools"),
            expected
        );
        let removed_head = json
            .as_object_mut()
            .expect("response object")
            .remove("selected_active_head");
        assert_eq!(removed_head, Some(serde_json::Value::Null));
        assert!(serde_json::from_value::<GoTestTraceFileQueryResponseV1>(json).is_err());

        let mut hit = serde_json::to_value(&expected.results[0]).expect("hit JSON");
        let original_lane = hit
            .as_object_mut()
            .expect("hit object")
            .insert("lexical".into(), serde_json::Value::Null);
        assert!(original_lane.is_some_and(|lane| !lane.is_null()));
        assert!(
            serde_json::from_value::<GoTestTraceFileHitV1>(hit.clone())
                .expect("explicit null lane")
                .lexical
                .is_none()
        );
        let removed_lane = hit.as_object_mut().expect("hit object").remove("lexical");
        assert_eq!(removed_lane, Some(serde_json::Value::Null));
        assert!(serde_json::from_value::<GoTestTraceFileHitV1>(hit).is_err());

        let cbor_value = ciborium::value::Value::serialized(&expected).expect("response CBOR");
        let ciborium::value::Value::Map(fields) = cbor_value else {
            panic!("response must be a CBOR map");
        };
        let names = fields
            .iter()
            .map(|(key, _)| match key {
                ciborium::value::Value::Text(name) => name.as_str(),
                _ => panic!("response field key must be text"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "generation",
                "selected_active_head",
                "results",
                "lexical_window",
                "dense_window",
                "frame_class",
                "explanation",
            ]
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&expected, &mut cbor).expect("response CBOR encode");
        assert_eq!(
            ciborium::de::from_reader::<GoTestTraceFileQueryResponseV1, _>(cbor.as_slice())
                .expect("response CBOR decode"),
            expected
        );
    }

    #[test]
    fn frame_class_keeps_snake_case_unit_variants() {
        for (frame, wire) in [
            (GoTestTraceFrameClassV1::Unresolved, "\"unresolved\""),
            (GoTestTraceFrameClassV1::Exact, "\"exact\""),
            (GoTestTraceFrameClassV1::Ambiguous, "\"ambiguous\""),
        ] {
            assert_eq!(serde_json::to_string(&frame).expect("frame JSON"), wire);
            assert_eq!(
                serde_json::from_str::<GoTestTraceFrameClassV1>(wire).expect("frame JSON decode"),
                frame
            );
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&frame, &mut cbor).expect("frame CBOR");
            assert_eq!(
                ciborium::de::from_reader::<GoTestTraceFrameClassV1, _>(cbor.as_slice())
                    .expect("frame CBOR decode"),
                frame
            );
        }
        assert!(serde_json::from_str::<GoTestTraceFrameClassV1>("\"Exact\"").is_err());
    }

    #[test]
    fn trace_cbor_accepts_legacy_known_byte_keys_and_frame_variants() {
        let expected = response();
        let mut value = ciborium::value::Value::serialized(&expected).expect("response CBOR");
        let ciborium::value::Value::Map(fields) = &mut value else {
            panic!("response must be a CBOR map");
        };
        let (key, _) = fields
            .iter_mut()
            .find(|(key, _)| {
                matches!(key, ciborium::value::Value::Text(name) if name == "selected_active_head")
            })
            .expect("nullable response key");
        *key = ciborium::value::Value::Bytes(b"selected_active_head".to_vec());
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&value, &mut bytes).expect("byte-key CBOR");
        assert_eq!(
            ciborium::de::from_reader::<GoTestTraceFileQueryResponseV1, _>(bytes.as_slice())
                .expect("known byte key"),
            expected
        );

        bytes.clear();
        ciborium::ser::into_writer(
            &ciborium::value::Value::Map(vec![(
                ciborium::value::Value::Bytes(b"exact".to_vec()),
                ciborium::value::Value::Null,
            )]),
            &mut bytes,
        )
        .expect("byte-variant CBOR");
        assert_eq!(
            ciborium::de::from_reader::<GoTestTraceFrameClassV1, _>(bytes.as_slice())
                .expect("known byte variant"),
            GoTestTraceFrameClassV1::Exact
        );
    }

    #[test]
    fn opted_in_rank_pools_bind_lane_rows_and_returned_first_witnesses() {
        let mut response = response();
        response.lexical_window = QueryResultWindowV2::exact_probe(1);
        response.dense_window = QueryResultWindowV2::exact_probe(1);
        let row = |score: f32| GoTestTraceFilePoolRowV1 {
            file: file("trace-file", "binding.go"),
            candidate_id: "chunk:binding.go".into(),
            score_bits: score.to_bits(),
        };
        response.rank_pools = Some(GoTestTraceFileRankPoolsV1 {
            lexical: vec![row(4.0)],
            dense: vec![row(0.8)],
        });
        let mut request = request(1);
        request.include_rank_pools = true;
        assert!(response.validate_against_v1(&request).is_ok());
        let wire = serde_json::to_value(&response).expect("rank pool response encode");
        assert!(wire.get("rank_pools").is_some());
        let decoded: GoTestTraceFileQueryResponseV1 =
            serde_json::from_value(wire.clone()).expect("rank pool response decode");
        assert_eq!(decoded, response);

        request.include_rank_pools = false;
        assert!(response.validate_against_v1(&request).is_err());
        request.include_rank_pools = true;
        let mut missing = response.clone();
        missing.rank_pools = None;
        assert!(missing.validate_against_v1(&request).is_err());
        let mut short = response.clone();
        short.rank_pools.as_mut().expect("pools").dense.clear();
        assert!(short.validate_v1().is_err());
        let mut nonfinite = response.clone();
        nonfinite.rank_pools.as_mut().expect("pools").lexical[0].score_bits = f32::NAN.to_bits();
        assert!(nonfinite.validate_v1().is_err());
        let mut wrong_id = response.clone();
        wrong_id.rank_pools.as_mut().expect("pools").lexical[0].candidate_id = "wrong".into();
        assert!(wrong_id.validate_v1().is_err());
        let mut wrong_score = response.clone();
        wrong_score.rank_pools.as_mut().expect("pools").lexical[0].score_bits = 2.0_f32.to_bits();
        assert!(wrong_score.validate_v1().is_err());
        let mut wrong_owner = response.clone();
        wrong_owner.rank_pools.as_mut().expect("pools").dense[0]
            .file
            .source_repo_id = RepoId::new("other-source").expect("source repo");
        assert!(wrong_owner.validate_v1().is_err());
        let mut omitted = response.clone();
        omitted.lexical_window = QueryResultWindowV2::exact_probe(2);
        omitted
            .rank_pools
            .as_mut()
            .expect("pools")
            .lexical
            .push(GoTestTraceFilePoolRowV1 {
                file: file("trace-file", "omitted.go"),
                candidate_id: "chunk:omitted.go".into(),
                score_bits: 2.0_f32.to_bits(),
            });
        assert!(omitted.validate_v1().is_ok());
        let uncapped_request = GoTestTraceFileQueryRequestV1 {
            top_k: 2,
            ..request.clone()
        };
        assert!(omitted.validate_against_v1(&uncapped_request).is_err());
        assert!(omitted.validate_against_v1(&request).is_ok());
        let mut extra = wire;
        extra["rank_pools"]["lexical"][0]["unknown"] = serde_json::json!(1);
        assert!(serde_json::from_value::<GoTestTraceFileQueryResponseV1>(extra).is_err());
    }

    #[test]
    fn observed_witness_positions_follow_first_distinct_chunk_order() {
        let mut bad = response();
        bad.results[0].lexical.as_mut().expect("lexical").chunk_rank = 2;
        assert!(bad.validate_v1().is_err());

        let mut ordered = response();
        ordered.frame_class = GoTestTraceFrameClassV1::Unresolved;
        ordered.results[0].source_frame = false;
        ordered.results.push(GoTestTraceFileHitV1 {
            file: file("trace-file", "binding_test.go"),
            lexical: Some(GoTestTraceFileLaneWitnessV1 {
                file_rank: 2,
                chunk_rank: 2,
                candidate: candidate("binding_test.go", 2.0),
            }),
            dense: None,
            file_rrf_score: 1.0 / 62.0,
            production_source: false,
            source_frame: false,
        });
        assert!(ordered.validate_v1().is_ok());
        ordered.results[1]
            .lexical
            .as_mut()
            .expect("lexical")
            .chunk_rank = 1;
        assert!(ordered.validate_v1().is_err());
    }

    #[test]
    fn request_refuses_missing_lane_rank_only_on_complete_file_page() {
        let mut result = response();
        result.results[0].dense.as_mut().expect("dense").file_rank = 2;
        result.results[0].dense.as_mut().expect("dense").chunk_rank = 2;
        result.results[0].file_rrf_score = 1.0 / 61.0 + 1.0 / 62.0;
        assert!(result.validate_v1().is_ok());
        assert!(result.validate_against_v1(&request(2)).is_err());
        assert!(result.validate_against_v1(&request(1)).is_ok());

        result = response();
        result.results[0].dense = None;
        result.results[0].file_rrf_score = 1.0 / 61.0;
        assert!(result.validate_v1().is_ok());
        assert!(result.validate_against_v1(&request(2)).is_err());
    }

    #[test]
    fn response_refuses_score_pin_path_and_unknown_wire_fields() {
        let mut bad = response();
        bad.results[0].file_rrf_score = 0.5;
        assert!(bad.validate_v1().is_err());
        bad = response();
        bad.results[0]
            .lexical
            .as_mut()
            .expect("lexical")
            .candidate
            .repo_relative_path = RepoRelativePath::new("other.go");
        assert!(bad.validate_v1().is_err());
        bad = response();
        bad.results[0].file.repo_relative_path = RepoRelativePath::new("../binding.go");
        assert!(bad.validate_v1().is_err());
        bad = response();
        bad.results[0]
            .lexical
            .as_mut()
            .expect("lexical")
            .candidate
            .source_repo_id = RepoId::new("another-source").expect("source");
        assert!(bad.validate_v1().is_err());
        bad = response();
        bad.dense_window = QueryResultWindowV2::exact_probe(0);
        assert!(bad.validate_v1().is_err());
        bad = response();
        bad.results[0].source_frame = false;
        bad.frame_class = GoTestTraceFrameClassV1::Unresolved;
        assert!(bad.validate_v1().is_ok());
        let encoded = serde_json::to_string(&bad).expect("response encode");
        let unknown = encoded.replacen("\"frame_class\":", "\"unknown\":1,\"frame_class\":", 1);
        assert!(serde_json::from_str::<GoTestTraceFileQueryResponseV1>(&unknown).is_err());
        let duplicate = encoded.replacen(
            "\"frame_class\":",
            "\"frame_class\":\"exact\",\"frame_class\":",
            1,
        );
        assert!(serde_json::from_str::<GoTestTraceFileQueryResponseV1>(&duplicate).is_err());
    }

    #[test]
    fn response_refuses_file_order_and_duplicate_identity() {
        let mut bad = response();
        bad.results.push(GoTestTraceFileHitV1 {
            file: file("trace-file", "binding_test.go"),
            lexical: Some(GoTestTraceFileLaneWitnessV1 {
                file_rank: 2,
                chunk_rank: 3,
                candidate: candidate("binding_test.go", 2.0),
            }),
            dense: None,
            file_rrf_score: 1.0 / 62.0,
            production_source: false,
            source_frame: false,
        });
        assert!(bad.validate_v1().is_ok());
        bad.results.swap(0, 1);
        assert!(bad.validate_v1().is_err());
        bad.results.swap(0, 1);
        let duplicate = bad.results[0].clone();
        bad.results.push(duplicate);
        assert!(bad.validate_v1().is_err());
    }

    #[test]
    fn file_rrf_tie_cannot_be_reordered_by_a_nearby_reported_score() {
        let mut page = response();
        page.frame_class = GoTestTraceFrameClassV1::Unresolved;
        page.results[0].source_frame = false;
        page.lexical_window = QueryResultWindowV2::exact_probe(2);
        page.dense_window = QueryResultWindowV2::exact_probe(2);
        page.results[0].dense.as_mut().expect("dense").file_rank = 2;
        page.results[0].dense.as_mut().expect("dense").chunk_rank = 2;
        let rank_score = 1.0 / 61.0 + 1.0 / 62.0;
        page.results[0].file_rrf_score = rank_score;
        page.results.push(GoTestTraceFileHitV1 {
            file: file("trace-file", "other.go"),
            lexical: Some(GoTestTraceFileLaneWitnessV1 {
                file_rank: 2,
                chunk_rank: 2,
                candidate: candidate("other.go", 2.0),
            }),
            dense: Some(GoTestTraceFileLaneWitnessV1 {
                file_rank: 1,
                chunk_rank: 1,
                candidate: candidate("other.go", 0.9),
            }),
            file_rrf_score: rank_score,
            production_source: true,
            source_frame: false,
        });
        assert!(page.validate_v1().is_ok());
        let wire = serde_json::to_string(&page).expect("serialize exact RRF score");
        let decoded: GoTestTraceFileQueryResponseV1 =
            serde_json::from_str(&wire).expect("deserialize exact RRF score");
        assert_eq!(
            decoded.results[0].file_rrf_score.to_bits(),
            rank_score.to_bits()
        );

        page.results.swap(0, 1);
        assert!(page.validate_v1().is_err());
        page.results[0].file_rrf_score = f64::from_bits(rank_score.to_bits() + 1);
        assert!(page.validate_v1().is_err());
    }

    #[test]
    fn unanchored_file_rrf_can_rank_a_test_above_production_source() {
        let mut response = response();
        response.frame_class = GoTestTraceFrameClassV1::Unresolved;
        response.lexical_window = QueryResultWindowV2::exact_probe(2);
        response.dense_window = QueryResultWindowV2::exact_probe(2);
        let production = &mut response.results[0];
        production.source_frame = false;
        production.lexical.as_mut().expect("lexical").file_rank = 2;
        production.lexical.as_mut().expect("lexical").chunk_rank = 2;
        production.dense.as_mut().expect("dense").file_rank = 2;
        production.dense.as_mut().expect("dense").chunk_rank = 2;
        production.file_rrf_score = 2.0 / 62.0;
        let test = GoTestTraceFileHitV1 {
            file: file("trace-file", "binding_test.go"),
            lexical: Some(GoTestTraceFileLaneWitnessV1 {
                file_rank: 1,
                chunk_rank: 1,
                candidate: candidate("binding_test.go", 1.0),
            }),
            dense: Some(GoTestTraceFileLaneWitnessV1 {
                file_rank: 1,
                chunk_rank: 1,
                candidate: candidate("binding_test.go", 0.9),
            }),
            file_rrf_score: 2.0 / 61.0,
            production_source: false,
            source_frame: false,
        };
        response.results.insert(0, test);
        assert!(response.validate_v1().is_ok());
        response.results.swap(0, 1);
        assert!(response.validate_v1().is_err());
    }

    #[test]
    fn response_keeps_same_path_from_distinct_source_repositories() {
        let mut valid = response();
        valid.frame_class = GoTestTraceFrameClassV1::Unresolved;
        valid.results[0].source_frame = false;
        valid.results[0].file.source_repo_id = RepoId::new("source-a").expect("source a");
        valid.results[0]
            .lexical
            .as_mut()
            .expect("lexical")
            .candidate
            .source_repo_id = RepoId::new("source-a").expect("source a");
        valid.results[0]
            .dense
            .as_mut()
            .expect("dense")
            .candidate
            .source_repo_id = RepoId::new("source-a").expect("source a");
        let mut second = valid.results[0].clone();
        second.file.source_repo_id = RepoId::new("source-b").expect("source b");
        second.lexical.as_mut().expect("lexical").file_rank = 2;
        second.lexical.as_mut().expect("lexical").chunk_rank = 3;
        second
            .lexical
            .as_mut()
            .expect("lexical")
            .candidate
            .source_repo_id = RepoId::new("source-b").expect("source b");
        second.dense.as_mut().expect("dense").file_rank = 3;
        second.dense.as_mut().expect("dense").chunk_rank = 3;
        second
            .dense
            .as_mut()
            .expect("dense")
            .candidate
            .source_repo_id = RepoId::new("source-b").expect("source b");
        second.file_rrf_score = 1.0 / 62.0 + 1.0 / 63.0;
        valid.results.push(second);
        assert!(valid.validate_v1().is_ok());
    }

    #[test]
    fn response_requires_nullable_wire_fields_to_be_present() {
        let mut valid = serde_json::to_value(response()).expect("response JSON");
        assert!(
            valid
                .get("selected_active_head")
                .is_some_and(serde_json::Value::is_null)
        );
        let missing_head = {
            let mut value = valid.clone();
            let _removed = value
                .as_object_mut()
                .expect("response object")
                .remove("selected_active_head");
            value
        };
        assert!(serde_json::from_value::<GoTestTraceFileQueryResponseV1>(missing_head).is_err());

        let hit = valid["results"][0].as_object_mut().expect("hit object");
        let _previous = hit.insert("dense".to_string(), serde_json::Value::Null);
        let decoded = serde_json::from_value::<GoTestTraceFileHitV1>(valid["results"][0].clone())
            .expect("explicit null dense witness");
        assert!(decoded.dense.is_none());
        for field in ["lexical", "dense"] {
            let mut missing = valid["results"][0].clone();
            let _removed = missing.as_object_mut().expect("hit object").remove(field);
            assert!(serde_json::from_value::<GoTestTraceFileHitV1>(missing).is_err());
        }
    }

    #[test]
    fn response_binds_source_flags_to_raw_request_frames() {
        use crate::{HybridQueryRequest, QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax};

        let raw = |failure_excerpt: &str| GoTestTraceFileQueryRequestV1 {
            query: HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "panic".to_string(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin()),
                    generation_selector: None,
                    top_k: 100,
                    cursor: None,
                },
                semantic_query_text: "panic".to_string(),
                generation: Some(pin()),
                generation_selector: None,
                top_k: 100,
            },
            failure_excerpt: failure_excerpt.to_string(),
            failure_source_repo_id: Some(pin().repo_id),
            failure_module_context: None,
            top_k: 20,
            include_rank_pools: false,
        };
        let matched = raw("right.go:12");
        let mut response = response();
        response.results[0].file.repo_relative_path = RepoRelativePath::new("right.go");
        response.results[0]
            .lexical
            .as_mut()
            .expect("lexical")
            .candidate
            .repo_relative_path = RepoRelativePath::new("right.go");
        response.results[0]
            .dense
            .as_mut()
            .expect("dense")
            .candidate
            .repo_relative_path = RepoRelativePath::new("right.go");
        assert!(response.validate_against_v1(&matched).is_ok());
        let mut no_owner = matched.clone();
        no_owner.failure_source_repo_id = None;
        assert!(response.validate_against_v1(&no_owner).is_err());
        assert!(response.validate_against_v1(&raw("wrong.go:12")).is_err());
        response.results[0].file.source_repo_id = RepoId::new("foreign").expect("foreign");
        response.results[0]
            .lexical
            .as_mut()
            .expect("lexical")
            .candidate
            .source_repo_id = RepoId::new("foreign").expect("foreign");
        response.results[0]
            .dense
            .as_mut()
            .expect("dense")
            .candidate
            .source_repo_id = RepoId::new("foreign").expect("foreign");
        assert!(response.validate_against_v1(&matched).is_err());
        response.frame_class = GoTestTraceFrameClassV1::Unresolved;
        response.results[0].source_frame = false;
        assert!(response.validate_against_v1(&matched).is_err());
        assert!(
            response
                .validate_against_v1(&raw("/tmp/right.go:12"))
                .is_ok()
        );
        assert!(
            response
                .validate_against_v1(&raw("right.go:12 other.go:3"))
                .is_err()
        );
    }

    #[test]
    fn response_binds_module_cache_frame_to_raw_panic_and_version() {
        use crate::{
            GoTestTraceModuleContextV1, HybridQueryRequest, QueryConstraintSetV1, TextQueryRequest,
            TextQuerySyntax,
        };
        let mut response = response();
        let path = "middleware/encryptcookie/encryptcookie.go";
        response.results[0].file.repo_relative_path = RepoRelativePath::new(path);
        response.results[0]
            .lexical
            .as_mut()
            .expect("lexical")
            .candidate
            .repo_relative_path = RepoRelativePath::new(path);
        response.results[0]
            .dense
            .as_mut()
            .expect("dense")
            .candidate
            .repo_relative_path = RepoRelativePath::new(path);
        let request = GoTestTraceFileQueryRequestV1 {
            query: HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "panic".into(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin()),
                    generation_selector: None,
                    top_k: 100,
                    cursor: None,
                },
                semantic_query_text: "panic".into(),
                generation: Some(pin()),
                generation_selector: None,
                top_k: 100,
            },
            failure_excerpt: concat!(
                "panic: runtime error\n",
                "\t/tmp/pkg/mod/github.com/gofiber/fiber/v3@v3.0.0-rc.3/middleware/recover/recover.go:12 +0x24\n",
                "panic({0x1})\n",
                "\t/tmp/pkg/mod/github.com/valyala/fasthttp@v1.68.0/header.go:1163\n",
                "\t/tmp/pkg/mod/github.com/gofiber/fiber/v3@v3.0.0-rc.3/middleware/encryptcookie/encryptcookie.go:23 +0x83c\n",
                "\t/tmp/pkg/mod/github.com/gofiber/fiber/v3@v3.0.0-rc.3/ctx.go:235 +0x74",
            )
            .into(),
            failure_source_repo_id: Some(pin().repo_id),
            failure_module_context: Some(GoTestTraceModuleContextV1 {
                module_path: "github.com/gofiber/fiber/v3".into(),
                module_version: "v3.0.0-rc.3".into(),
            }),
            top_k: 20,
            include_rank_pools: false,
        };
        assert!(response.validate_against_v1(&request).is_ok());
        let mut wrong_version = request.clone();
        wrong_version
            .failure_module_context
            .as_mut()
            .expect("module")
            .module_version = "v3.0.0-rc.2".into();
        assert!(response.validate_against_v1(&wrong_version).is_err());
        let mut missing_panic = request;
        missing_panic.failure_excerpt = missing_panic
            .failure_excerpt
            .replace("panic({0x1})", "recovered");
        assert!(response.validate_against_v1(&missing_panic).is_err());
    }
}
