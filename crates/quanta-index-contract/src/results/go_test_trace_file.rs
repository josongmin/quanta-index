//! Typed file-ranking result for the explicitly selected Go test trace route.

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeStruct};

use crate::{
    GenerationPin, GoTestTraceFileQueryRequestV1, GoTestTraceSourceFramesV1, LexicalCandidate,
    QueryResultWindowV2, SearchCorpusActiveHeadV1, SourceFileKey,
};

use super::SearchExplanation;

/// Whether raw failure text contains zero, one, or multiple parsed source-frame hints.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoTestTraceFrameClassV1 {
    Unresolved,
    Exact,
    Ambiguous,
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

/// First returned chunk of a file in one lane; ranks are one-based.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoTestTraceFileLaneWitnessV1 {
    pub file_rank: u32,
    pub chunk_rank: u32,
    pub candidate: LexicalCandidate,
}

/// Distinct file with a file-RRF score and the original lane evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoTestTraceFileHitV1 {
    pub file: SourceFileKey,
    #[serde(deserialize_with = "required_option")]
    pub lexical: Option<GoTestTraceFileLaneWitnessV1>,
    #[serde(deserialize_with = "required_option")]
    pub dense: Option<GoTestTraceFileLaneWitnessV1>,
    pub file_rrf_score: f64,
    pub production_source: bool,
    /// Query-bound parsed hint; does not certify a call-stack frame.
    pub source_frame: bool,
}

/// Serde treats absent `Option<T>` as `None`; this adapter requires an actual
/// field while retaining an explicit JSON/CBOR null as a valid value.
fn required_option<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    decoder: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(decoder)
}

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
    pub frame_class: GoTestTraceFrameClassV1,
    pub explanation: SearchExplanation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireResponse {
    generation: GenerationPin,
    #[serde(deserialize_with = "required_option")]
    selected_active_head: Option<SearchCorpusActiveHeadV1>,
    results: Vec<GoTestTraceFileHitV1>,
    lexical_window: QueryResultWindowV2,
    dense_window: QueryResultWindowV2,
    frame_class: GoTestTraceFrameClassV1,
    explanation: SearchExplanation,
}

impl From<WireResponse> for GoTestTraceFileQueryResponseV1 {
    fn from(response: WireResponse) -> Self {
        Self {
            generation: response.generation,
            selected_active_head: response.selected_active_head,
            results: response.results,
            lexical_window: response.lexical_window,
            dense_window: response.dense_window,
            frame_class: response.frame_class,
            explanation: response.explanation,
        }
    }
}

impl GoTestTraceFileQueryResponseV1 {
    pub fn validate_v1(&self) -> Result<(), String> {
        let mut files = BTreeSet::new();
        let mut lexical_ranks = BTreeSet::new();
        let mut dense_ranks = BTreeSet::new();
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
            let is_source = path.ends_with(".go") && !path.ends_with("_test.go");
            if hit.production_source != is_source || (hit.source_frame && !is_source) {
                return Err("Go trace file source flags disagree with path".to_string());
            }
            if hit.source_frame {
                source_frames += 1;
            }
            let mut expected_score = 0.0;
            for (witness, ranks, returned) in [
                (
                    hit.lexical.as_ref(),
                    &mut lexical_ranks,
                    self.lexical_window.returned(),
                ),
                (
                    hit.dense.as_ref(),
                    &mut dense_ranks,
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
                expected_score += 1.0 / (60.0 + f64::from(witness.file_rank));
            }
            if !hit.file_rrf_score.is_finite()
                || (hit.file_rrf_score - expected_score).abs() > 1e-12
            {
                return Err("Go trace file RRF score disagrees with lane ranks".to_string());
            }
        }
        if self.frame_class != GoTestTraceFrameClassV1::Exact && source_frames != 0
            || source_frames > 1
        {
            return Err("Go trace frame class disagrees with promoted files".to_string());
        }
        for pair in self.results.windows(2) {
            let left = &pair[0];
            let right = &pair[1];
            let order = right
                .source_frame
                .cmp(&left.source_frame)
                .then(right.file_rrf_score.total_cmp(&left.file_rrf_score))
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
        self.validate_v1()?;
        let frames = match &request.failure_module_context {
            Some(module) => {
                GoTestTraceSourceFramesV1::from_module_cache_panic(&request.failure_excerpt, module)
            }
            None => GoTestTraceSourceFramesV1::from_failure_excerpt(&request.failure_excerpt),
        };
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
        let result_count = u32::try_from(self.results.len())
            .map_err(|_| "Go trace file response result count overflow".to_string())?;
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
        let mut state = serializer.serialize_struct("GoTestTraceFileQueryResponseV1", 7)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("selected_active_head", &self.selected_active_head)?;
        state.serialize_field("results", &self.results)?;
        state.serialize_field("lexical_window", &self.lexical_window)?;
        state.serialize_field("dense_window", &self.dense_window)?;
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
    use crate::{ManifestGeneration, RepoId, RepoRelativePath, RevisionId};

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
            start_line: 1,
            end_line: 2,
            score,
            snippet: String::new(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
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
                    chunk_rank: 2,
                    candidate: candidate("binding.go", 4.0),
                }),
                dense: Some(GoTestTraceFileLaneWitnessV1 {
                    file_rank: 2,
                    chunk_rank: 3,
                    candidate: candidate("binding.go", 0.8),
                }),
                file_rrf_score: 1.0 / 61.0 + 1.0 / 62.0,
                production_source: true,
                source_frame: true,
            }],
            lexical_window: QueryResultWindowV2::exact_probe(3),
            dense_window: QueryResultWindowV2::exact_probe(3),
            frame_class: GoTestTraceFrameClassV1::Exact,
            explanation: SearchExplanation::empty(),
        }
    }

    #[test]
    fn response_roundtrips_with_separate_lane_windows() {
        let expected = response();
        let encoded = serde_json::to_string(&expected).expect("response encode");
        assert_eq!(
            serde_json::from_str::<GoTestTraceFileQueryResponseV1>(&encoded)
                .expect("response decode"),
            expected
        );
        assert!(expected.results[0].representative().is_some());
        assert_eq!(expected.frame_class.as_str(), "exact");
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
        bad.dense_window = QueryResultWindowV2::exact_probe(2);
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
    fn unanchored_file_rrf_can_rank_a_test_above_production_source() {
        let mut response = response();
        response.frame_class = GoTestTraceFrameClassV1::Unresolved;
        response.lexical_window = QueryResultWindowV2::exact_probe(2);
        response.dense_window = QueryResultWindowV2::exact_probe(2);
        let production = &mut response.results[0];
        production.source_frame = false;
        production.lexical.as_mut().expect("lexical").file_rank = 2;
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
