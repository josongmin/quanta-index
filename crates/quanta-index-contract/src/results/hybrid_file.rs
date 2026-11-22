//! Ordinary file retrieval from two bounded hybrid chunk lanes.

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeStruct};
use sha2::{Digest as _, Sha256};

use crate::{
    GenerationPin, HybridFileQueryRequestV1, LexicalCandidate, QueryResultWindowV2,
    SearchCorpusActiveHeadV1, SourceFileKey, validate_public_top_k,
};

use super::go_test_trace_file::validate_file_witness_positions;
use super::{GoTestTraceFileLaneWitnessV1, SearchExplanation};

/// The first chunk for one file in one lane. The wire is shared with the
/// trace-file route, but this route has no trace evidence or path priority.
pub type HybridFileLaneWitnessV1 = GoTestTraceFileLaneWitnessV1;

/// Hash the ordered, bounded chunk prefix before file projection. The caller
/// supplies the exact lane order; this function never sorts or cuts rows.
pub fn hybrid_file_candidate_pool_sha256_v1(
    pin: &GenerationPin,
    rows: &[LexicalCandidate],
) -> Result<[u8; 32], String> {
    fn string(hash: &mut Sha256, value: &str) -> Result<(), String> {
        let bytes = value.as_bytes();
        let len = u64::try_from(bytes.len())
            .map_err(|error| format!("pool string length overflow: {error}"))?;
        hash.update(len.to_le_bytes());
        hash.update(bytes);
        Ok(())
    }

    if pin.manifest_generation.get() == 0 {
        return Err("pool generation is zero".into());
    }
    let mut hash = Sha256::new();
    hash.update(b"quanta-index/hybrid-file/ordered-pool/v1\0");
    string(&mut hash, pin.repo_id.as_str())?;
    string(&mut hash, pin.revision_id.as_str())?;
    hash.update(pin.manifest_generation.get().to_le_bytes());
    hash.update(
        u64::try_from(rows.len())
            .map_err(|error| format!("pool row count overflow: {error}"))?
            .to_le_bytes(),
    );
    for row in rows {
        if row.repo_id != pin.repo_id
            || row.revision_id != pin.revision_id
            || row.manifest_generation != pin.manifest_generation
            || !row.score.is_finite()
            || row.validate_source_metadata().is_err()
            || (SourceFileKey {
                source_repo_id: row.source_repo_id.clone(),
                repo_relative_path: row.repo_relative_path.clone(),
            })
            .validate()
            .is_err()
        {
            return Err("pool candidate pin, score, or source metadata is invalid".into());
        }
        string(&mut hash, row.source_repo_id.as_str())?;
        string(&mut hash, row.repo_relative_path.as_str())?;
        string(&mut hash, &row.candidate_id)?;
        hash.update(row.score.to_bits().to_le_bytes());
    }
    Ok(hash.finalize().into())
}

#[derive(Clone, Debug, PartialEq)]
pub struct HybridFileHitV1 {
    pub file: SourceFileKey,
    pub lexical: Option<HybridFileLaneWitnessV1>,
    pub dense: Option<HybridFileLaneWitnessV1>,
    pub file_rrf_score: f64,
}
crate::native_serde_struct!([serde_identifier] HybridFileHitV1 {
    file: SourceFileKey,
    lexical: Option<HybridFileLaneWitnessV1>,
    dense: Option<HybridFileLaneWitnessV1>,
    file_rrf_score: f64,
});

impl HybridFileHitV1 {
    #[must_use]
    pub fn representative(&self) -> Option<&LexicalCandidate> {
        self.lexical
            .as_ref()
            .or(self.dense.as_ref())
            .map(|witness| &witness.candidate)
    }
}

/// File top-k is distinct from the two chunk-lane windows. A short file page
/// makes no claim that the generation's files were exhausted.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridFileQueryResponseV1 {
    pub generation: GenerationPin,
    pub selected_active_head: Option<SearchCorpusActiveHeadV1>,
    /// Echo of the requested returned-file cap.
    pub file_top_k: u32,
    /// Distinct files in the two bounded chunk lanes before this file cut.
    /// This does not prove generation-wide file exhaustion.
    pub bounded_file_count: u32,
    /// SHA-256 of each ordered bounded chunk pool before file projection.
    pub lexical_pool_sha256: [u8; 32],
    pub dense_pool_sha256: [u8; 32],
    pub results: Vec<HybridFileHitV1>,
    pub lexical_window: QueryResultWindowV2,
    pub dense_window: QueryResultWindowV2,
    pub explanation: SearchExplanation,
}

struct WireResponse {
    generation: GenerationPin,
    selected_active_head: Option<SearchCorpusActiveHeadV1>,
    file_top_k: u32,
    bounded_file_count: u32,
    lexical_pool_sha256: [u8; 32],
    dense_pool_sha256: [u8; 32],
    results: Vec<HybridFileHitV1>,
    lexical_window: QueryResultWindowV2,
    dense_window: QueryResultWindowV2,
    explanation: SearchExplanation,
}
crate::native_serde_struct!([serde_identifier] WireResponse {
    generation: GenerationPin,
    selected_active_head: Option<SearchCorpusActiveHeadV1>,
    file_top_k: u32,
    bounded_file_count: u32,
    lexical_pool_sha256: [u8; 32],
    dense_pool_sha256: [u8; 32],
    results: Vec<HybridFileHitV1>,
    lexical_window: QueryResultWindowV2,
    dense_window: QueryResultWindowV2,
    explanation: SearchExplanation,
});

impl HybridFileQueryResponseV1 {
    pub fn validate_v1(&self) -> Result<(), String> {
        let file_cap = validate_public_top_k(self.file_top_k)
            .map_err(|error| format!("{}: {error}", error.code()))?;
        let bounded_count = usize::try_from(self.bounded_file_count)
            .map_err(|error| format!("bounded file count exceeds usize: {error}"))?;
        let file_cap = usize::try_from(file_cap)
            .map_err(|error| format!("file cap exceeds usize: {error}"))?;
        let lane_returned = self
            .lexical_window
            .returned()
            .saturating_add(self.dense_window.returned());
        if self.bounded_file_count > lane_returned
            || (self.bounded_file_count == 0)
                != (self.lexical_window.returned() == 0 && self.dense_window.returned() == 0)
            || self.results.len() != bounded_count.min(file_cap)
        {
            return Err("hybrid bounded file count or returned prefix differs".into());
        }
        let empty_pool_sha256 = hybrid_file_candidate_pool_sha256_v1(&self.generation, &[])?;
        if (self.lexical_window.returned() == 0 && self.lexical_pool_sha256 != empty_pool_sha256)
            || (self.dense_window.returned() == 0 && self.dense_pool_sha256 != empty_pool_sha256)
        {
            return Err("empty hybrid chunk pool digest differs".into());
        }
        let mut files = BTreeSet::new();
        let mut lexical_file_ranks = BTreeSet::new();
        let mut dense_file_ranks = BTreeSet::new();
        let mut lexical_chunk_ranks = BTreeSet::new();
        let mut dense_chunk_ranks = BTreeSet::new();
        let mut lexical_positions = Vec::new();
        let mut dense_positions = Vec::new();
        let mut rank_scores = Vec::with_capacity(self.results.len());
        for hit in &self.results {
            if hit.file.validate().is_err() || !files.insert(&hit.file) {
                return Err("hybrid file identity is invalid or repeated".into());
            }
            if hit.representative().is_none() {
                return Err("hybrid file hit has no lane witness".into());
            }
            let mut expected = 0.0_f64;
            for (witness, file_ranks, chunk_ranks, positions, returned) in [
                (
                    hit.lexical.as_ref(),
                    &mut lexical_file_ranks,
                    &mut lexical_chunk_ranks,
                    &mut lexical_positions,
                    self.lexical_window.returned(),
                ),
                (
                    hit.dense.as_ref(),
                    &mut dense_file_ranks,
                    &mut dense_chunk_ranks,
                    &mut dense_positions,
                    self.dense_window.returned(),
                ),
            ] {
                let Some(witness) = witness else { continue };
                if witness.file_rank == 0
                    || witness.chunk_rank == 0
                    || witness.file_rank > witness.chunk_rank
                    || witness.chunk_rank > returned
                    || !file_ranks.insert(witness.file_rank)
                    || !chunk_ranks.insert(witness.chunk_rank)
                    || witness.candidate.repo_id != self.generation.repo_id
                    || witness.candidate.revision_id != self.generation.revision_id
                    || witness.candidate.manifest_generation != self.generation.manifest_generation
                    || witness.candidate.source_repo_id != hit.file.source_repo_id
                    || witness.candidate.repo_relative_path != hit.file.repo_relative_path
                    || witness.candidate.validate_source_metadata().is_err()
                    || !witness.candidate.score.is_finite()
                {
                    return Err("hybrid file lane witness is invalid".into());
                }
                positions.push((witness.file_rank, witness.chunk_rank));
                expected += 1.0 / (60.0 + f64::from(witness.file_rank));
            }
            if hit.file_rrf_score.to_bits() != expected.to_bits() {
                return Err("hybrid file RRF score disagrees with lane ranks".into());
            }
            rank_scores.push(expected);
        }
        if lexical_file_ranks
            .iter()
            .chain(&dense_file_ranks)
            .any(|rank| *rank > self.bounded_file_count)
        {
            return Err("hybrid file rank exceeds bounded file count".into());
        }
        let complete_union = self.results.len() == bounded_count;
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
        for (pair, scores) in self.results.windows(2).zip(rank_scores.windows(2)) {
            let [left, right] = pair else {
                return Err("hybrid file results are outside file-RRF order".into());
            };
            let [left_score, right_score] = scores else {
                return Err("hybrid file results are outside file-RRF order".into());
            };
            let order = right_score
                .total_cmp(left_score)
                .then(right.lexical.is_some().cmp(&left.lexical.is_some()))
                .then(left.file.cmp(&right.file));
            if order.is_gt() {
                return Err("hybrid file results are outside file-RRF order".into());
            }
        }
        Ok(())
    }

    pub fn validate_against_v1(&self, request: &HybridFileQueryRequestV1) -> Result<(), String> {
        request.validate_v1()?;
        self.validate_v1()?;
        if self.file_top_k != request.file_top_k
            || self.results.len()
                > usize::try_from(request.file_top_k)
                    .map_err(|error| format!("file cap overflow: {error}"))?
            || self.lexical_window.returned() > request.query.top_k
            || self.dense_window.returned() > request.query.top_k
        {
            return Err("hybrid file cap echo, result, or lane exceeds its separate cap".into());
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
            return Err("hybrid file response generation drift".into());
        }
        Ok(())
    }
}

impl Serialize for HybridFileQueryResponseV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("HybridFileQueryResponseV1", 10)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("selected_active_head", &self.selected_active_head)?;
        state.serialize_field("file_top_k", &self.file_top_k)?;
        state.serialize_field("bounded_file_count", &self.bounded_file_count)?;
        state.serialize_field("lexical_pool_sha256", &self.lexical_pool_sha256)?;
        state.serialize_field("dense_pool_sha256", &self.dense_pool_sha256)?;
        state.serialize_field("results", &self.results)?;
        state.serialize_field("lexical_window", &self.lexical_window)?;
        state.serialize_field("dense_window", &self.dense_window)?;
        state.serialize_field("explanation", &self.explanation)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for HybridFileQueryResponseV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireResponse::deserialize(deserializer)?;
        let response = Self {
            generation: wire.generation,
            selected_active_head: wire.selected_active_head,
            file_top_k: wire.file_top_k,
            bounded_file_count: wire.bounded_file_count,
            lexical_pool_sha256: wire.lexical_pool_sha256,
            dense_pool_sha256: wire.dense_pool_sha256,
            results: wire.results,
            lexical_window: wire.lexical_window,
            dense_window: wire.dense_window,
            explanation: wire.explanation,
        };
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

    fn pin() -> GenerationPin {
        GenerationPin::new(
            RepoId::new("generic-file").expect("repo"),
            RevisionId::new("rev-1").expect("revision"),
            ManifestGeneration::new(1),
        )
    }

    fn candidate(path: &str, id: &str) -> LexicalCandidate {
        let generation = pin();
        LexicalCandidate {
            source_repo_id: generation.repo_id.clone(),
            source: None,
            preview: None,
            candidate_id: id.to_string(),
            repo_id: generation.repo_id,
            revision_id: generation.revision_id,
            manifest_generation: generation.manifest_generation,
            repo_relative_path: RepoRelativePath::new(path),
            start_line: 1,
            end_line: 2,
            score: 1.0,
            snippet: String::new(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        }
    }

    fn response() -> HybridFileQueryResponseV1 {
        let lexical_pool = [candidate("src/a.go", "a-1"), candidate("src/a.go", "a-0")];
        let dense_pool = [candidate("src/a.go", "a-2"), candidate("src/a.go", "a-3")];
        HybridFileQueryResponseV1 {
            generation: pin(),
            selected_active_head: None,
            file_top_k: 1,
            bounded_file_count: 1,
            lexical_pool_sha256: hybrid_file_candidate_pool_sha256_v1(&pin(), &lexical_pool)
                .expect("lexical fixture digest"),
            dense_pool_sha256: hybrid_file_candidate_pool_sha256_v1(&pin(), &dense_pool)
                .expect("dense fixture digest"),
            results: vec![HybridFileHitV1 {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("generic-file").expect("source"),
                    repo_relative_path: RepoRelativePath::new("src/a.go"),
                },
                lexical: Some(HybridFileLaneWitnessV1 {
                    file_rank: 1,
                    chunk_rank: 1,
                    candidate: candidate("src/a.go", "a-1"),
                }),
                dense: Some(HybridFileLaneWitnessV1 {
                    file_rank: 1,
                    chunk_rank: 1,
                    candidate: candidate("src/a.go", "a-2"),
                }),
                file_rrf_score: 2.0 / 61.0,
            }],
            lexical_window: QueryResultWindowV2::exact_probe(2),
            dense_window: QueryResultWindowV2::exact_probe(2),
            explanation: SearchExplanation::empty(),
        }
    }

    fn request() -> HybridFileQueryRequestV1 {
        HybridFileQueryRequestV1 {
            query: HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "handler".into(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin()),
                    generation_selector: None,
                    top_k: 2,
                    cursor: None,
                },
                semantic_query_text: "handler".into(),
                generation: Some(pin()),
                generation_selector: None,
                top_k: 2,
            },
            file_top_k: 1,
        }
    }

    #[test]
    fn generic_file_wire_and_witness_validation() {
        let good = response();
        assert!(good.validate_against_v1(&request()).is_ok());
        let wire = serde_json::to_string(&good).expect("encode");
        assert_eq!(
            serde_json::from_str::<HybridFileQueryResponseV1>(&wire).expect("decode"),
            good
        );
        let mut bad = response();
        bad.results[0].file_rrf_score += 0.01;
        assert!(bad.validate_v1().is_err());
        bad = response();
        bad.bounded_file_count = 0;
        assert!(bad.validate_v1().is_err());
        bad = response();
        bad.dense_pool_sha256 = [0; 32];
        assert_ne!(bad, good);
        bad = response();
        bad.file_top_k = 2;
        assert!(bad.validate_against_v1(&request()).is_err());
        bad = response();
        bad.results[0].dense.as_mut().expect("dense").file_rank = 2;
        assert!(bad.validate_v1().is_err());
        bad = response();
        bad.results[0].dense.as_mut().expect("dense").chunk_rank = 3;
        assert!(bad.validate_v1().is_err());
        bad = response();
        bad.results[0]
            .lexical
            .as_mut()
            .expect("lexical")
            .candidate
            .source_repo_id = RepoId::new("foreign").expect("source");
        assert!(bad.validate_v1().is_err());
        bad = response();
        bad.results[0]
            .lexical
            .as_mut()
            .expect("lexical")
            .candidate
            .revision_id = RevisionId::new("foreign").expect("revision");
        assert!(bad.validate_v1().is_err());
        bad = response();
        bad.results.push(bad.results[0].clone());
        assert!(bad.validate_v1().is_err());
        let mut bad_request = request();
        bad_request.file_top_k = 0;
        assert!(good.validate_against_v1(&bad_request).is_err());
        bad_request = request();
        bad_request.query.top_k = 1;
        assert!(good.validate_against_v1(&bad_request).is_err());
        bad_request = request();
        bad_request.query.generation = Some(GenerationPin::new(
            RepoId::new("generic-file").expect("repo"),
            RevisionId::new("foreign").expect("revision"),
            ManifestGeneration::new(1),
        ));
        assert!(good.validate_against_v1(&bad_request).is_err());
        let mut missing_witness: serde_json::Value =
            serde_json::from_str(&wire).expect("wire value");
        let _removed = missing_witness["results"][0]
            .as_object_mut()
            .expect("hit map")
            .remove("lexical");
        assert!(serde_json::from_value::<HybridFileQueryResponseV1>(missing_witness).is_err());
        let unknown = wire.replacen("\"results\":", "\"unknown\":1,\"results\":", 1);
        assert!(serde_json::from_str::<HybridFileQueryResponseV1>(&unknown).is_err());
    }

    #[test]
    fn complete_file_page_requires_every_nonempty_lane_rank() {
        let mut missing = response();
        missing.file_top_k = 2;
        missing.bounded_file_count = 2;
        missing.results[0].dense = None;
        missing.results[0].file_rrf_score = 1.0 / 61.0;
        missing.results.push(HybridFileHitV1 {
            file: SourceFileKey {
                source_repo_id: RepoId::new("generic-file").expect("source"),
                repo_relative_path: RepoRelativePath::new("src/b.go"),
            },
            lexical: None,
            dense: Some(HybridFileLaneWitnessV1 {
                file_rank: 2,
                chunk_rank: 2,
                candidate: candidate("src/b.go", "b-1"),
            }),
            file_rrf_score: 1.0 / 62.0,
        });
        assert!(missing.validate_v1().is_err());

        let mut no_dense = response();
        no_dense.results[0].dense = None;
        no_dense.results[0].file_rrf_score = 1.0 / 61.0;
        assert!(no_dense.validate_v1().is_err());
    }

    #[test]
    fn file_rrf_tie_cannot_be_reordered_by_a_nearby_reported_score() {
        let mut page = response();
        page.file_top_k = 2;
        page.bounded_file_count = 2;
        page.results[0].dense.as_mut().expect("dense").file_rank = 2;
        page.results[0].dense.as_mut().expect("dense").chunk_rank = 2;
        let rank_score = 1.0 / 61.0 + 1.0 / 62.0;
        page.results[0].file_rrf_score = rank_score;
        page.results.push(HybridFileHitV1 {
            file: SourceFileKey {
                source_repo_id: RepoId::new("generic-file").expect("source"),
                repo_relative_path: RepoRelativePath::new("src/b.go"),
            },
            lexical: Some(HybridFileLaneWitnessV1 {
                file_rank: 2,
                chunk_rank: 2,
                candidate: candidate("src/b.go", "b-lexical"),
            }),
            dense: Some(HybridFileLaneWitnessV1 {
                file_rank: 1,
                chunk_rank: 1,
                candidate: candidate("src/b.go", "b-dense"),
            }),
            file_rrf_score: rank_score,
        });
        assert!(page.validate_v1().is_ok());
        let wire = serde_json::to_string(&page).expect("serialize exact RRF score");
        let decoded: HybridFileQueryResponseV1 =
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
    fn cut_file_page_can_omit_earlier_lane_file_rank() {
        let mut page = response();
        page.file_top_k = 2;
        page.bounded_file_count = 3;
        page.lexical_window = QueryResultWindowV2::exact_probe(2);
        page.dense_window = QueryResultWindowV2::exact_probe(3);
        page.results[0].dense.as_mut().expect("dense").file_rank = 2;
        page.results[0].dense.as_mut().expect("dense").chunk_rank = 2;
        page.results[0].file_rrf_score = 1.0 / 61.0 + 1.0 / 62.0;
        page.results.push(HybridFileHitV1 {
            file: SourceFileKey {
                source_repo_id: RepoId::new("generic-file").expect("source"),
                repo_relative_path: RepoRelativePath::new("src/b.go"),
            },
            lexical: Some(HybridFileLaneWitnessV1 {
                file_rank: 2,
                chunk_rank: 2,
                candidate: candidate("src/b.go", "b-lexical"),
            }),
            dense: Some(HybridFileLaneWitnessV1 {
                file_rank: 3,
                chunk_rank: 3,
                candidate: candidate("src/b.go", "b-dense"),
            }),
            file_rrf_score: 1.0 / 62.0 + 1.0 / 63.0,
        });
        assert!(page.validate_v1().is_ok());
        page.results[1].dense.as_mut().expect("dense").chunk_rank = 2;
        assert!(page.validate_v1().is_err());
    }

    #[test]
    fn ordered_pool_digest_distinguishes_source_repositories_within_one_index_pin() {
        let original = candidate("src/a.go", "same-id");
        let mut other_source = original.clone();
        other_source.source_repo_id = RepoId::new("other-source").expect("source id");
        let first = hybrid_file_candidate_pool_sha256_v1(&pin(), &[original])
            .expect("index-owned candidate");
        let second = hybrid_file_candidate_pool_sha256_v1(&pin(), &[other_source])
            .expect("another source within the same index generation");
        assert_ne!(first, second);
        let hex = |digest: [u8; 32]| {
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        assert_eq!(
            hex(first),
            "a912a7180ed38cfdbc27b6435369605b8dfe1d279098cc0e6336a65793318d96"
        );
        assert_eq!(
            hex(second),
            "d084e5a4974f7fe1c4393e26acb8dc97e018f24ad1996f3a531dca69329f2eb0"
        );
    }

    #[test]
    fn ordered_pool_digest_preserves_f32_json_roundtrip_and_negative_zero() {
        let mut positive = candidate("src/a.go", "positive");
        positive.source_repo_id = RepoId::new("other-source").expect("source id");
        positive.score = 0.1_f32;
        let mut negative = candidate("src/b.go", "negative");
        negative.score = -0.0_f32;
        let original = vec![positive, negative];
        let wire = serde_json::to_string(&original).expect("candidate JSON");
        let decoded: Vec<LexicalCandidate> = serde_json::from_str(&wire).expect("candidate decode");
        assert_eq!(decoded[0].score.to_bits(), 0.1_f32.to_bits());
        assert_eq!(decoded[1].score.to_bits(), (-0.0_f32).to_bits());
        let digest = hybrid_file_candidate_pool_sha256_v1(&pin(), &original).expect("pool digest");
        assert_eq!(
            digest,
            hybrid_file_candidate_pool_sha256_v1(&pin(), &decoded).expect("decoded pool digest")
        );
        assert_eq!(
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            "5569c8cd927f2483a724f8b5622fde71b45797ae70dc50cb59dff88657023bdc"
        );
        let mut reversed = decoded.clone();
        reversed.reverse();
        assert_ne!(
            digest,
            hybrid_file_candidate_pool_sha256_v1(&pin(), &reversed).expect("reordered pool")
        );
        let mut score_changed = decoded.clone();
        score_changed[0].score = 0.2_f32;
        assert_ne!(
            digest,
            hybrid_file_candidate_pool_sha256_v1(&pin(), &score_changed).expect("changed score")
        );
        let mut signed_zero_changed = decoded;
        signed_zero_changed[1].score = 0.0_f32;
        assert_ne!(
            digest,
            hybrid_file_candidate_pool_sha256_v1(&pin(), &signed_zero_changed)
                .expect("changed zero sign")
        );
    }
}
