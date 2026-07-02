//! Semantic / hybrid query-path helpers for the search-plane dispatcher.
//!
//! Selection resolution, the query-time model-identity gate, RRF/seed
//! candidate assembly, and response-explanation builders for the semantic,
//! hybrid, and hybrid-seed query paths. Split out of `query_dispatcher` so the
//! dispatcher file is not the sole home for every query mode. This is a child
//! module: `use super::*` pulls in the shared resolvers, policies, and types it
//! builds on, and the moved items are `pub(super)` so the dispatcher methods
//! (which stay in `query_dispatcher`) keep calling them unchanged.

#[expect(
    clippy::wildcard_imports,
    reason = "child module split out of query_dispatcher for size; it builds directly on the parent's shared resolvers, policies, and types"
)]
use super::*;

#[derive(Clone, Debug)]
pub(super) struct SemanticSelection {
    pub(super) pin: GenerationPin,
    pub(super) expected_manifest_digest: Option<String>,
}

/// Outcome of the shared hybrid lexical+semantic fusion, before the caller
/// wraps it in its path-specific response (plain hybrid vs. hybrid-seed).
pub(super) struct HybridFusion {
    pub(super) pin: GenerationPin,
    pub(super) lex_results: Vec<LexicalCandidate>,
    pub(super) sem_results: Vec<LexicalCandidate>,
    pub(super) fused: Vec<LexicalCandidate>,
    pub(super) explanation: SearchExplanation,
}

pub(super) fn resolve_semantic_request_selection(
    activation_catalog: &ActivationCatalog,
    request: &SemanticQueryRequest,
) -> Result<SemanticSelection, CoreError> {
    let outer_selection = match request.generation_selector.as_ref() {
        Some(selector) => Some(resolve_semantic_selector_selection(
            activation_catalog,
            selector,
            "semantic",
        )?),
        None => None,
    };
    let scope_pin = match request.lexical_scope.as_ref() {
        Some(scope) => Some(resolve_lexical_request_pin(
            activation_catalog,
            scope,
            SearchPlaneTrackKind::Lexical,
            "semantic scope",
        )?),
        None => None,
    };
    match (request.generation.clone(), outer_selection, scope_pin) {
        (Some(pin), Some(selection), Some(scope_pin))
            if pin != selection.pin || pin != scope_pin =>
        {
            Err(CoreError::InvalidContract(
                "semantic: scope generation does not match semantic request generation".to_string(),
            ))
        }
        (Some(pin), Some(selection), None) if pin != selection.pin => {
            Err(CoreError::InvalidContract(
                "semantic: explicit generation pin does not match generation selector resolution"
                    .to_string(),
            ))
        }
        (Some(pin), None, Some(scope_pin)) if pin != scope_pin => Err(CoreError::InvalidContract(
            "semantic: scope generation does not match semantic request generation".to_string(),
        )),
        (None, Some(selection), Some(scope_pin)) if selection.pin != scope_pin => {
            Err(CoreError::InvalidContract(
                "semantic: scope generation does not match semantic request generation".to_string(),
            ))
        }
        (Some(pin), Some(selection), _) => Ok(SemanticSelection {
            pin,
            expected_manifest_digest: selection.expected_manifest_digest,
        }),
        (Some(pin), None, _) | (None, None, Some(pin)) => Ok(SemanticSelection {
            pin,
            expected_manifest_digest: None,
        }),
        (None, Some(selection), _) => Ok(selection),
        (None, None, None) => Err(CoreError::InvalidContract(
            "semantic: generation pin required".to_string(),
        )),
    }
}

pub(super) fn resolve_hybrid_request_selection(
    activation_catalog: &ActivationCatalog,
    request: &HybridQueryRequest,
) -> Result<SemanticSelection, CoreError> {
    let lexical_pin = resolve_lexical_request_pin(
        activation_catalog,
        &request.text_query,
        SearchPlaneTrackKind::Lexical,
        "hybrid text_query",
    )?;
    let semantic_selection = match request.generation_selector.as_ref() {
        Some(selector) => Some(resolve_semantic_selector_selection(
            activation_catalog,
            selector,
            "hybrid",
        )?),
        None => None,
    };
    match (request.generation.clone(), semantic_selection) {
        (Some(pin), Some(selection)) if pin != selection.pin || pin != lexical_pin => {
            Err(CoreError::InvalidContract(
                "hybrid: lexical generation does not match semantic generation".to_string(),
            ))
        }
        (Some(pin), None) if pin != lexical_pin => Err(CoreError::InvalidContract(
            "hybrid: lexical generation does not match semantic generation".to_string(),
        )),
        (None, Some(selection)) if selection.pin != lexical_pin => Err(CoreError::InvalidContract(
            "hybrid: lexical generation does not match semantic generation".to_string(),
        )),
        (Some(pin), Some(selection)) => Ok(SemanticSelection {
            pin,
            expected_manifest_digest: selection.expected_manifest_digest,
        }),
        (Some(pin), None) => Ok(SemanticSelection {
            pin,
            expected_manifest_digest: None,
        }),
        (None, Some(selection)) => Ok(selection),
        (None, None) => Ok(SemanticSelection {
            pin: lexical_pin,
            expected_manifest_digest: None,
        }),
    }
}

pub(super) fn resolve_hybrid_seed_request_selection(
    activation_catalog: &ActivationCatalog,
    request: &HybridSeedQueryRequest,
) -> Result<SemanticSelection, CoreError> {
    let legacy_request = HybridQueryRequest {
        text_query: request.text_query.clone(),
        semantic_query_text: request.semantic_query_text.clone(),
        generation: request.generation.clone(),
        generation_selector: request.generation_selector.clone(),
        top_k: request.top_k,
    };
    resolve_hybrid_request_selection(activation_catalog, &legacy_request)
}

pub(super) fn checked_rank_u32_v1(index: usize, label: &'static str) -> Result<u32, CoreError> {
    let ordinal = index
        .checked_add(1)
        .ok_or_else(|| CoreError::Storage(format!("{label}: rank index overflow")))?;
    u32::try_from(ordinal)
        .map_err(|err| CoreError::Storage(format!("{label}: rank exceeds u32: {err}")))
}

pub(super) fn build_hybrid_seed_candidates_v1(
    fused: &[LexicalCandidate],
    lex_results: &[LexicalCandidate],
    sem_results: &[LexicalCandidate],
) -> Result<Vec<HybridSeedCandidate>, CoreError> {
    let lexical_positions = lex_results
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            Ok((
                candidate.candidate_id.clone(),
                (
                    checked_rank_u32_v1(index, "hybrid seed lexical")?,
                    candidate.score,
                ),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, CoreError>>()?;
    let semantic_positions = sem_results
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            Ok((
                candidate.candidate_id.clone(),
                (
                    checked_rank_u32_v1(index, "hybrid seed semantic")?,
                    candidate.score,
                ),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, CoreError>>()?;

    fused
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let lexical = lexical_positions
                .get(candidate.candidate_id.as_str())
                .copied();
            let semantic = semantic_positions
                .get(candidate.candidate_id.as_str())
                .copied();
            let mut source_lanes = Vec::new();
            if lexical.is_some() {
                source_lanes.push(HybridSeedLane::Lexical);
            }
            if semantic.is_some() {
                source_lanes.push(HybridSeedLane::Semantic);
            }
            Ok(HybridSeedCandidate {
                candidate: candidate.clone(),
                seed_rank: checked_rank_u32_v1(index, "hybrid seed fused")?,
                lexical_rank: lexical.map(|(rank, _score)| rank),
                lexical_score_raw: lexical.map(|(_rank, score)| score),
                semantic_rank: semantic.map(|(rank, _score)| rank),
                semantic_score_raw: semantic.map(|(_rank, score)| score),
                source_lanes,
            })
        })
        .collect()
}

/// Reject a query whose embedder model identity differs from the indexed model.
///
/// Even at equal dimension, query vectors from a different model are not
/// cosine-comparable, so a same-dimension model swap would otherwise produce
/// silent garbage rankings. Fails closed (`SEM_MODEL_MISMATCH`).
pub(super) fn ensure_query_model_matches_index_v1(
    embedder_model_id: &str,
    embedder_model_version: Option<&str>,
    index_model_id: &str,
    index_model_version: Option<&str>,
    plane: &str,
) -> Result<(), CoreError> {
    if embedder_model_id == index_model_id && embedder_model_version == index_model_version {
        return Ok(());
    }
    Err(CoreError::Typed {
        code: LexicalErrorCode::SemModelMismatch.as_code_str().to_string(),
        message: format!(
            "{plane}: query embedder model {embedder_model_id}/{embedder_model_version:?} is not comparable to index model {index_model_id}/{index_model_version:?} (equal dimension is insufficient; vectors from different models are not cosine-comparable)"
        ),
    })
}

pub(super) fn build_semantic_response_explanation(
    scope_candidate_count: usize,
    scoped: bool,
    result_count: usize,
    early_stop_reason: Option<EarlyStopReason>,
) -> SearchExplanation {
    let mut planner_trace = vec![PlannerTraceEntry {
        stage: PlannerStage::Plan,
        detail: format!("semantic.scope={scoped}"),
    }];
    if scoped {
        planner_trace.push(PlannerTraceEntry {
            stage: PlannerStage::ExecFanout,
            detail: format!("semantic.scope.text_candidates={scope_candidate_count}"),
        });
    }
    planner_trace.push(PlannerTraceEntry {
        stage: PlannerStage::Merge,
        detail: format!("semantic.results={result_count}"),
    });
    // Honest engine contribution (consistent with the hybrid path): report a lane
    // only when it actually contributed, never as a fixed capability claim. The
    // lexical scope contributed only if it narrowed to candidates; the semantic
    // engine only if it returned results.
    let mut engines_touched = Vec::new();
    if scoped && scope_candidate_count > 0 {
        engines_touched.push(EngineTouched::Lexical);
    }
    if result_count > 0 {
        engines_touched.push(EngineTouched::Semantic);
    }
    let summary = if scoped {
        format!(
            "semantic scoped query returned {result_count} candidates from text scope of {scope_candidate_count}"
        )
    } else {
        format!("semantic query returned {result_count} candidates")
    };
    SearchExplanation {
        planner_trace,
        engines_touched,
        early_stop_reason,
        contributions: Vec::new(),
        ranker_weights_hash: [0u8; 32],
        strategy: if result_count == 0 {
            "empty".to_string()
        } else if scoped {
            "semantic_scoped".to_string()
        } else {
            "semantic".to_string()
        },
        summary,
    }
}

pub(super) fn build_hybrid_response_explanation(
    lexical_universe_size: usize,
    lexical_hits: usize,
    semantic_hits: usize,
    fused_hits: usize,
    internal_top_k: u32,
    early_stop_reason: Option<EarlyStopReason>,
) -> SearchExplanation {
    // The hybrid semantic lane is scoped to the lexical candidate universe
    // (`search_scoped` over `lexical_ids`), so it re-ranks lexical recall and can
    // never surface a semantic-only hit. Report the honest lane contribution and
    // strategy: a genuine two-lane RRF only when BOTH lanes contributed; otherwise
    // the degraded single-lane reality (or empty), never a symmetric "rrf" over a
    // starved lane.
    let mut engines_touched = Vec::new();
    if lexical_hits > 0 {
        engines_touched.push(EngineTouched::Lexical);
    }
    if semantic_hits > 0 {
        engines_touched.push(EngineTouched::Semantic);
    }
    let strategy = match (lexical_hits > 0, semantic_hits > 0) {
        (true, true) => "rrf",
        (true, false) => "lexical_only",
        // Production-unreachable: the semantic lane is scoped to lexical recall, so
        // semantic_hits > 0 requires lexical_hits > 0. Kept self-consistent with
        // engines_touched (= [Semantic]) rather than collapsing to the nonsensical
        // "empty"-with-Semantic-touched state, so strategy and engines never disagree.
        (false, true) => "semantic_only",
        (false, false) => "empty",
    }
    .to_string();
    SearchExplanation {
        planner_trace: vec![
            PlannerTraceEntry {
                stage: PlannerStage::Plan,
                detail: format!("hybrid.internal_top_k={internal_top_k}"),
            },
            PlannerTraceEntry {
                stage: PlannerStage::ExecFanout,
                detail: format!(
                    "hybrid.semantic_scoped_to_lexical=true; hybrid.lexical_universe={lexical_universe_size}; lexical_hits={lexical_hits}; semantic_hits={semantic_hits}"
                ),
            },
            PlannerTraceEntry {
                stage: PlannerStage::Merge,
                detail: format!("hybrid.fused_results={fused_hits}"),
            },
        ],
        engines_touched,
        early_stop_reason,
        contributions: Vec::new(),
        ranker_weights_hash: [0u8; 32],
        strategy,
        summary: format!(
            "hybrid fused {lexical_hits} lexical and {semantic_hits} semantic candidates into {fused_hits} results"
        ),
    }
}

pub(super) fn prefix_semantic_query_error(plane: &str, err: CoreError) -> CoreError {
    match err {
        CoreError::Typed { code, message } => CoreError::Typed {
            code,
            message: format!("{plane}: {message}"),
        },
        other @ (CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => other,
    }
}
