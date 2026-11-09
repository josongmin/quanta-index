//! Explicit Go test-trace file projection over two complete, pinned SDK lanes.
//!
//! This is a diagnostic rank profile, separate from the chunk RRF wire order.
//! Its file RRF score must never be substituted for `HybridCandidateV1.fused_score`.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::{
    ExactRepoRelativePathV1, GenerationPin, GoTestTraceModuleContextV1, GoTestTraceSourceFramesV1,
    LexicalCandidate, RepoId, SourceFileKey,
};

use crate::{domains::semantic::SemanticPolicy, error::CoreError};

use super::service::HybridOrchestratorPolicy;

/// A file identity scoped to the exact joint generation of both input lanes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceFileKeyV1 {
    pub pin: GenerationPin,
    pub file: SourceFileKey,
}

/// The first chunk of a file in one lane, and its rank among distinct files.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceFileLaneWitnessV1 {
    pub file_rank: u32,
    pub chunk_rank: u32,
    pub candidate: LexicalCandidate,
}

/// Query-bound source-frame hints parsed from admitted Go test failure text.
/// They are never inferred from gold labels or result paths, and do not prove
/// that a matching token came from an actual call stack.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceFileEvidenceV1 {
    source_frames: BTreeSet<ExactRepoRelativePathV1>,
    failure_source_repo_id: Option<RepoId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceSourceFrameClassV1 {
    Unresolved,
    Exact,
    Ambiguous,
}

impl TraceFileEvidenceV1 {
    /// Extract repository-relative Go source-frame hints from a selected Go
    /// test failure query. Absolute paths, parent traversal, and test frames
    /// do not become production-source ranking hints.
    #[must_use]
    pub fn from_go_test_failure_text(text: &str, failure_source_repo_id: Option<RepoId>) -> Self {
        let source_frames = GoTestTraceSourceFramesV1::from_failure_excerpt(text)
            .paths()
            .clone();
        Self {
            source_frames,
            failure_source_repo_id,
        }
    }

    /// Use an exact module-cache frame from the raw panic stack. The caller
    /// must have checked the module version against the frozen source pin.
    #[must_use]
    pub fn from_go_test_module_cache_panic(
        text: &str,
        failure_source_repo_id: RepoId,
        module: &GoTestTraceModuleContextV1,
    ) -> Self {
        let source_frames = GoTestTraceSourceFramesV1::from_module_cache_panic(text, module)
            .paths()
            .clone();
        Self {
            source_frames,
            failure_source_repo_id: Some(failure_source_repo_id),
        }
    }

    #[must_use]
    pub fn frame_class(&self) -> TraceSourceFrameClassV1 {
        match self.source_frames.len() {
            0 => TraceSourceFrameClassV1::Unresolved,
            1 => TraceSourceFrameClassV1::Exact,
            _ => TraceSourceFrameClassV1::Ambiguous,
        }
    }

    #[must_use]
    pub fn source_frames(&self) -> &BTreeSet<ExactRepoRelativePathV1> {
        &self.source_frames
    }
}

/// One ranked file with both original lane witnesses. `file_rrf_score` is the
/// sum of `1 / (60 + file_rank)` for lanes that found the file. It is not the
/// score of the representative chunk and is not a hybrid wire result.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceFileHitV1 {
    pub key: TraceFileKeyV1,
    pub lexical: Option<TraceFileLaneWitnessV1>,
    pub dense: Option<TraceFileLaneWitnessV1>,
    pub file_rrf_score: f64,
    pub production_source: bool,
    pub source_frame: bool,
}

impl TraceFileHitV1 {
    /// The lexical chunk owns the displayed span when both lanes saw a file.
    #[must_use]
    pub fn representative(&self) -> &LexicalCandidate {
        &self
            .lexical
            .as_ref()
            .or(self.dense.as_ref())
            .expect("every ranked file has at least one lane witness")
            .candidate
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TraceFileRankPolicyV1;

impl TraceFileRankPolicyV1 {
    /// Rank distinct files from two complete, individually ranked SDK lanes.
    /// An exact, owner-bound frame hint precedes file RRF. Without that hint,
    /// files rank only by file RRF, lexical presence, and file identity. The
    /// Go production-source flag is reported as metadata and never changes
    /// rank. A parsed frame is a query hint, not verified stack authority.
    /// No label or lane raw-score threshold participates. The original chunk
    /// RRF endpoint is unchanged.
    pub fn rank_go_test_failure(
        pin: &GenerationPin,
        lexical: &[LexicalCandidate],
        dense: &[LexicalCandidate],
        evidence: &TraceFileEvidenceV1,
        top_k: u32,
    ) -> Result<Vec<TraceFileHitV1>, CoreError> {
        HybridOrchestratorPolicy::validate_top_k(top_k)?;
        let lexical_files = first_file_witnesses(pin, lexical, "lexical")?;
        let dense_files = first_file_witnesses(pin, dense, "dense")?;
        // The RRF authority owns arithmetic and rank provenance. Here its
        // inputs are files in first-chunk order, not chunk identities.
        let mut lexical_order = lexical_files.values().collect::<Vec<_>>();
        lexical_order.sort_by_key(|witness| witness.file_rank);
        let mut dense_order = dense_files.values().collect::<Vec<_>>();
        dense_order.sort_by_key(|witness| witness.file_rank);
        let lexical_files_ordered = lexical_order
            .into_iter()
            .map(|witness| candidate_file_key(&witness.candidate))
            .collect::<Vec<_>>();
        let dense_files_ordered = dense_order
            .into_iter()
            .map(|witness| candidate_file_key(&witness.candidate))
            .collect::<Vec<_>>();
        let fused = HybridOrchestratorPolicy::fuse_rrf_key_lanes_with_provenance(
            &[&lexical_files_ordered, &dense_files_ordered],
            u32::MAX,
        );
        let mut files = fused
            .into_iter()
            .map(|row| {
                let file = row.key;
                let path = &file.repo_relative_path;
                let production_source = is_go_production_source(path.as_str());
                // A raw relative path cannot identify a source repo by itself.
                // Only the explicit request owner binds a frame hint to a file.
                let source_frame = evidence.frame_class() == TraceSourceFrameClassV1::Exact
                    && evidence.failure_source_repo_id.as_ref() == Some(&file.source_repo_id)
                    && evidence
                        .source_frames
                        .iter()
                        .any(|frame| frame.as_str() == path.as_str());
                TraceFileHitV1 {
                    key: TraceFileKeyV1 {
                        pin: pin.clone(),
                        file: file.clone(),
                    },
                    lexical: lexical_files.get(&file).cloned(),
                    dense: dense_files.get(&file).cloned(),
                    file_rrf_score: row.fused_score,
                    production_source,
                    source_frame,
                }
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| {
            right
                .source_frame
                .cmp(&left.source_frame)
                .then(right.file_rrf_score.total_cmp(&left.file_rrf_score))
                .then(right.lexical.is_some().cmp(&left.lexical.is_some()))
                .then(left.key.file.cmp(&right.key.file))
        });
        let limit = usize::try_from(top_k).map_or(usize::MAX, |value| value);
        files.truncate(limit);
        Ok(files)
    }
}

fn is_go_production_source(path: &str) -> bool {
    path.ends_with(".go") && !path.ends_with("_test.go")
}

fn candidate_file_key(candidate: &LexicalCandidate) -> SourceFileKey {
    SourceFileKey {
        source_repo_id: candidate.source_repo_id.clone(),
        repo_relative_path: candidate.repo_relative_path.clone(),
    }
}

fn first_file_witnesses(
    pin: &GenerationPin,
    lane: &[LexicalCandidate],
    lane_name: &str,
) -> Result<BTreeMap<SourceFileKey, TraceFileLaneWitnessV1>, CoreError> {
    if lane.len() > usize::try_from(SemanticPolicy::max_top_k()).map_or(usize::MAX, |n| n) {
        return Err(CoreError::InvalidContract(format!(
            "trace file rank: {lane_name} lane exceeds public result cap"
        )));
    }
    let mut files = BTreeMap::new();
    for (index, candidate) in lane.iter().enumerate() {
        if candidate.repo_id != pin.repo_id
            || candidate.revision_id != pin.revision_id
            || candidate.manifest_generation != pin.manifest_generation
        {
            return Err(CoreError::InvalidContract(format!(
                "trace file rank: {lane_name} candidate has a different generation pin"
            )));
        }
        if !candidate.score.is_finite() {
            return Err(CoreError::InvalidContract(format!(
                "trace file rank: {lane_name} candidate score is not finite"
            )));
        }
        if ExactRepoRelativePathV1::new(candidate.repo_relative_path.as_str().to_owned()).is_err() {
            return Err(CoreError::InvalidContract(format!(
                "trace file rank: {lane_name} candidate path is not repository-relative"
            )));
        }
        candidate.validate_source_metadata().map_err(|reason| {
            CoreError::InvalidContract(format!(
                "trace file rank: {lane_name} candidate source metadata invalid: {reason}"
            ))
        })?;
        let file = candidate_file_key(candidate);
        if files.contains_key(&file) {
            continue;
        }
        let file_rank = u32::try_from(files.len().saturating_add(1)).map_err(|error| {
            CoreError::InvalidContract(format!("trace file rank: file rank overflow: {error}"))
        })?;
        let chunk_rank = u32::try_from(index.saturating_add(1)).map_err(|error| {
            CoreError::InvalidContract(format!("trace file rank: chunk rank overflow: {error}"))
        })?;
        let _previous = files.insert(
            file,
            TraceFileLaneWitnessV1 {
                file_rank,
                chunk_rank,
                candidate: candidate.clone(),
            },
        );
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{ManifestGeneration, RepoId, RepoRelativePath, RevisionId};

    fn pin() -> GenerationPin {
        GenerationPin::new(
            RepoId::new("trace-rank").expect("fixture repo"),
            RevisionId::new("rev-1").expect("fixture revision"),
            ManifestGeneration::new(7),
        )
    }

    fn candidate(id: &str, path: &str, score: f32) -> LexicalCandidate {
        let pin = pin();
        LexicalCandidate {
            source_repo_id: pin.repo_id.clone(),
            source: None,
            preview: None,
            candidate_id: id.to_owned(),
            repo_id: pin.repo_id,
            revision_id: pin.revision_id,
            manifest_generation: pin.manifest_generation,
            repo_relative_path: RepoRelativePath::new(path),
            start_line: 10,
            end_line: 20,
            score,
            snippet: String::new(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        }
    }

    #[test]
    fn source_frames_are_exact_ambiguous_or_unresolved() {
        let exact = TraceFileEvidenceV1::from_go_test_failure_text(
            "./router_test.go:8: failure at ./tree.go:869:2",
            None,
        );
        assert_eq!(exact.frame_class(), TraceSourceFrameClassV1::Exact);
        assert_eq!(
            exact
                .source_frames()
                .iter()
                .next()
                .map(|path| path.as_str()),
            Some("tree.go")
        );
        let ambiguous = TraceFileEvidenceV1::from_go_test_failure_text(
            "tree.go:869:2 render/data.go:4:1 ./router_test.go:8",
            None,
        );
        assert_eq!(ambiguous.frame_class(), TraceSourceFrameClassV1::Ambiguous);
        assert_eq!(ambiguous.source_frames().len(), 2);
        let unresolved = TraceFileEvidenceV1::from_go_test_failure_text(
            "/tmp/foreign.go:8 ../escape.go:9 ./router_test.go:10 no.go:word",
            None,
        );
        assert_eq!(
            unresolved.frame_class(),
            TraceSourceFrameClassV1::Unresolved
        );
    }

    #[test]
    fn file_rank_collapses_chunks_and_keeps_each_lane_witness() {
        let lexical = vec![
            candidate("doc-1", "README.md", 10.0),
            candidate("doc-2", "README.md", 9.0),
            candidate("src-1", "tree.go", 8.0),
            candidate("test-1", "tree_test.go", 7.0),
        ];
        let dense = vec![
            candidate("dense-1", "render/data.go", 0.9),
            candidate("dense-2", "tree.go", 0.8),
            candidate("dense-doc", "README.md", 0.7),
        ];
        let evidence = TraceFileEvidenceV1::from_go_test_failure_text(
            "./tree_test.go:4: panic at ./tree.go:869:2",
            Some(pin().repo_id),
        );
        let hits =
            TraceFileRankPolicyV1::rank_go_test_failure(&pin(), &lexical, &dense, &evidence, 4)
                .expect("ranked files");
        let paths = hits
            .iter()
            .map(|hit| hit.key.file.repo_relative_path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            ["tree.go", "README.md", "render/data.go", "tree_test.go"]
        );
        let tree = &hits[0];
        assert_eq!(tree.lexical.as_ref().map(|w| w.file_rank), Some(2));
        assert_eq!(tree.lexical.as_ref().map(|w| w.chunk_rank), Some(3));
        assert_eq!(tree.dense.as_ref().map(|w| w.file_rank), Some(2));
        assert_eq!(tree.representative().candidate_id, "src-1");
        assert_eq!(
            tree.file_rrf_score,
            HybridOrchestratorPolicy::rrf_score([2, 2])
        );
        assert_eq!(hits[2].lexical, None);
        assert_eq!(hits[2].representative().candidate_id, "dense-1");
        assert_eq!(hits[2].key.pin, pin());
    }

    #[test]
    fn unanchored_equal_file_rrf_uses_lexical_presence_then_path() {
        let lexical = [candidate("lex", "src/lex.go", 1.0)];
        let dense = [candidate("dense", "src/dense.go", 0.8)];
        let evidence =
            TraceFileEvidenceV1::from_go_test_failure_text("./case_test.go:1: bad", None);
        let hits =
            TraceFileRankPolicyV1::rank_go_test_failure(&pin(), &lexical, &dense, &evidence, 2)
                .expect("ranked files");
        assert_eq!(hits[0].key.file.repo_relative_path.as_str(), "src/lex.go");
        assert_eq!(hits[1].key.file.repo_relative_path.as_str(), "src/dense.go");
    }

    #[test]
    fn unanchored_test_and_document_files_follow_file_rrf_above_go_source() {
        let lexical = [
            candidate("test", "src/main_test.go", 3.0),
            candidate("readme", "README.md", 2.0),
            candidate("source", "src/main.go", 1.0),
        ];
        let dense = [
            candidate("test-dense", "src/main_test.go", 0.9),
            candidate("readme-dense", "README.md", 0.8),
        ];
        let evidence = TraceFileEvidenceV1::from_go_test_failure_text("panic", None);
        let hits =
            TraceFileRankPolicyV1::rank_go_test_failure(&pin(), &lexical, &dense, &evidence, 3)
                .expect("file RRF");
        assert_eq!(
            hits.iter()
                .map(|hit| hit.key.file.repo_relative_path.as_str())
                .collect::<Vec<_>>(),
            ["src/main_test.go", "README.md", "src/main.go"]
        );
        assert_eq!(hits[0].file_rrf_score, 2.0 / 61.0);
        assert_eq!(hits[1].file_rrf_score, 2.0 / 62.0);
        assert_eq!(hits[2].file_rrf_score, 1.0 / 63.0);
        assert!(!hits[0].production_source);
        assert!(hits[2].production_source);
    }

    #[test]
    fn unanchored_file_rrf_does_not_prioritize_go_or_path_classes() {
        let lexical = vec![
            candidate("example-a", "_examples/a.go", 7.0),
            candidate("doc", "doc.go", 6.0),
            candidate("example-b", "examples/b.go", 5.0),
            candidate("ordinary", "src/main.go", 4.0),
            candidate("similar-name", "internal_examples/main.go", 3.0),
            candidate("readme", "README.md", 2.0),
            candidate("test", "src/main_test.go", 1.0),
        ];
        let dense = vec![
            candidate("dense-example-a", "_examples/a.go", 0.9),
            candidate("dense-doc", "doc.go", 0.8),
            candidate("dense-example-b", "examples/b.go", 0.7),
        ];
        let unanchored =
            TraceFileEvidenceV1::from_go_test_failure_text("src/main_test.go:8: failure", None);
        let hits =
            TraceFileRankPolicyV1::rank_go_test_failure(&pin(), &lexical, &dense, &unanchored, 7)
                .expect("ranked files");
        let paths = hits
            .iter()
            .map(|hit| hit.key.file.repo_relative_path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [
                "_examples/a.go",
                "doc.go",
                "examples/b.go",
                "src/main.go",
                "internal_examples/main.go",
                "README.md",
                "src/main_test.go",
            ]
        );
        assert_eq!(hits[0].file_rrf_score, 2.0 / 61.0);
        assert_eq!(hits[1].file_rrf_score, 2.0 / 62.0);
        assert_eq!(hits[2].file_rrf_score, 2.0 / 63.0);
        assert!(hits[0].production_source);
        assert!(!hits[5].production_source);
        assert!(!hits[6].production_source);

        let anchored = TraceFileEvidenceV1::from_go_test_failure_text(
            "examples/b.go:12: failure",
            Some(pin().repo_id),
        );
        let anchored_hits =
            TraceFileRankPolicyV1::rank_go_test_failure(&pin(), &lexical, &dense, &anchored, 7)
                .expect("owner-bound frame");
        assert_eq!(
            anchored_hits[0].key.file.repo_relative_path.as_str(),
            "examples/b.go"
        );
        assert!(anchored_hits[0].source_frame);
        assert!(anchored_hits[0].production_source);
        assert_eq!(anchored_hits[0].file_rrf_score, 2.0 / 63.0);
    }

    #[test]
    fn ambiguous_source_frames_do_not_promote_one_matching_file() {
        let lexical = [
            candidate("a", "src/a.go", 2.0),
            candidate("b", "src/b.go", 1.0),
        ];
        let dense = [candidate("b-dense", "src/b.go", 0.8)];
        let evidence = TraceFileEvidenceV1::from_go_test_failure_text(
            "src/a.go:1:1 src/not-returned.go:2:1",
            Some(pin().repo_id),
        );
        assert_eq!(evidence.frame_class(), TraceSourceFrameClassV1::Ambiguous);
        let hits =
            TraceFileRankPolicyV1::rank_go_test_failure(&pin(), &lexical, &dense, &evidence, 2)
                .expect("ranked files");
        assert_eq!(hits[0].key.file.repo_relative_path.as_str(), "src/b.go");
        assert!(!hits[1].source_frame);
    }

    #[test]
    fn candidate_from_wrong_generation_or_nonfinite_lane_refuses() {
        let mut wrong = candidate("wrong", "src/a.go", 1.0);
        wrong.manifest_generation = ManifestGeneration::new(8);
        let evidence = TraceFileEvidenceV1::from_go_test_failure_text("./case_test.go:1", None);
        assert!(matches!(
            TraceFileRankPolicyV1::rank_go_test_failure(&pin(), &[wrong], &[], &evidence, 1),
            Err(CoreError::InvalidContract(_))
        ));
        let nonfinite = candidate("nan", "src/a.go", f32::NAN);
        assert!(matches!(
            TraceFileRankPolicyV1::rank_go_test_failure(&pin(), &[], &[nonfinite], &evidence, 1),
            Err(CoreError::InvalidContract(_))
        ));
    }

    #[test]
    fn same_path_from_two_source_repositories_remains_two_unpromoted_files() {
        let mut first = candidate("a", "src/main.go", 2.0);
        first.source_repo_id = RepoId::new("source-a").expect("source a");
        let mut second = candidate("b", "src/main.go", 1.0);
        second.source_repo_id = RepoId::new("source-b").expect("source b");
        let evidence =
            TraceFileEvidenceV1::from_go_test_failure_text("src/main.go:42: panic", None);
        let hits = TraceFileRankPolicyV1::rank_go_test_failure(
            &pin(),
            &[first.clone(), second.clone()],
            &[],
            &evidence,
            2,
        )
        .expect("two distinct source files");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].key.file.source_repo_id.as_str(), "source-a");
        assert_eq!(hits[1].key.file.source_repo_id.as_str(), "source-b");
        assert_eq!(hits[0].lexical.as_ref().map(|w| w.file_rank), Some(1));
        assert_eq!(hits[1].lexical.as_ref().map(|w| w.file_rank), Some(2));
        assert!(hits.iter().all(|hit| !hit.source_frame));

        let owner = RepoId::new("source-a").expect("explicit source");
        let evidence =
            TraceFileEvidenceV1::from_go_test_failure_text("src/main.go:42: panic", Some(owner));
        let with_owner = TraceFileRankPolicyV1::rank_go_test_failure(
            &pin(),
            &[first, second],
            &[],
            &evidence,
            2,
        )
        .expect("explicit owner hint");
        assert!(with_owner[0].source_frame);
        assert!(!with_owner[1].source_frame);
    }
}
