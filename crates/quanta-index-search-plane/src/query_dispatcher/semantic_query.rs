//! Semantic / hybrid query-path helpers for the search-plane dispatcher.
//!
//! Selection resolution, the query-time model-identity gate, RRF/seed
//! candidate assembly, and response-explanation builders for the semantic,
//! hybrid, and hybrid-seed query paths. The items are `pub(super)` so the
//! route bodies under `routes/` keep calling them unchanged; this module
//! depends only on `selection` and the contract/core crates.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    EarlyStopReason, GenerationPin, HybridCandidateV1, HybridQueryRequest, HybridSeedQueryRequest,
    LexicalCandidate, OwnerDocKind, PlannerStage, PlannerTraceEntry, QueryResultWindowV2,
    SearchExplanation, SearchPlaneTrackKind, SeedCandidate, SeedContribution, SeedFusionIdentity,
    SeedLane, SemanticCorpusKindV1, SemanticQueryRequest, SemanticSeedCorpusBudgetV1,
};
use quanta_index_core::{
    CoreError, DenseLaneContractV1, HybridOrchestratorPolicy, SemanticPolicy, SemanticSearchHitV1,
};

use crate::ActivationCatalog;
use crate::query_dispatcher::execution_trace::LaneExecutionSummaryV1;
use crate::query_dispatcher::selection::{
    SemanticSelection, explicit_pin_mismatch_error, resolve_joint_active_selection,
    resolve_lexical_request_pin, resolve_semantic_selector_selection, selection_mismatch_error,
    validate_generation_scope,
};

/// The plan-stage trace entry naming the dense lane every semantic route
/// ran through (QI-BB-027): its index, whether the seal proved it, and the
/// effort it spent.
fn dense_lane_trace_entry_v1(dense_lane: &DenseLaneContractV1) -> PlannerTraceEntry {
    PlannerTraceEntry {
        stage: PlannerStage::Plan,
        detail: dense_lane.trace_detail(),
    }
}

/// Outcome of the shared hybrid lexical+semantic fusion, before the caller
/// wraps it in its path-specific response (plain hybrid vs. hybrid-seed).
pub(super) struct HybridFusion {
    pub(super) pin: GenerationPin,
    pub(super) fused: Vec<HybridCandidateV1>,
    /// Typed outcome and coverage (S21-06): the dense admission outcome
    /// survives fusion here instead of collapsing into row counts.
    pub(super) window_v2: QueryResultWindowV2,
    pub(super) explanation: SearchExplanation,
}

pub(super) fn canonical_dense_corpus_budgets_v1(
    budgets: &[SemanticSeedCorpusBudgetV1],
) -> Result<Vec<SemanticSeedCorpusBudgetV1>, CoreError> {
    let mut canonical = budgets.to_vec();
    canonical.sort_by_key(|budget| budget.corpus_kind.as_code_str());
    for budget in &canonical {
        SemanticPolicy::validate_top_k(budget.top_k)?;
    }
    for pair in canonical.windows(2) {
        let (Some(left), Some(right)) = (pair.first(), pair.get(1)) else {
            return Err(CoreError::Storage(
                "hybrid seed: canonical budget window was not two entries".to_string(),
            ));
        };
        if left.corpus_kind == right.corpus_kind {
            return Err(CoreError::InvalidContract(format!(
                "hybrid seed: duplicate dense corpus budget {:?}",
                left.corpus_kind
            )));
        }
    }
    Ok(canonical)
}

pub(super) fn resolve_semantic_request_selection(
    activation_catalog: &ActivationCatalog,
    request: &SemanticQueryRequest,
) -> Result<SemanticSelection, CoreError> {
    validate_generation_scope(
        &[
            request.generation.as_ref(),
            request
                .lexical_scope
                .as_ref()
                .and_then(|scope| scope.generation.as_ref()),
        ],
        &[
            request.generation_selector.as_ref(),
            request
                .lexical_scope
                .as_ref()
                .and_then(|scope| scope.generation_selector.as_ref()),
        ],
        "semantic",
    )?;
    let joint = resolve_joint_active_selection(
        activation_catalog,
        request
            .lexical_scope
            .as_ref()
            .and_then(|scope| scope.generation_selector.as_ref()),
        request.generation_selector.as_ref(),
        request
            .lexical_scope
            .as_ref()
            .and_then(|scope| scope.generation.as_ref()),
        "semantic",
    )?;
    let outer_selection = match &joint {
        Some(selection) => Some(selection.clone()),
        None => match request.generation_selector.as_ref() {
            Some(selector) => Some(resolve_semantic_selector_selection(
                activation_catalog,
                selector,
                "semantic",
            )?),
            None => None,
        },
    };
    let scope_pin = match (&joint, request.lexical_scope.as_ref()) {
        (Some(selection), Some(_)) => Some(selection.pin.clone()),
        (None, Some(scope)) => Some(resolve_lexical_request_pin(
            activation_catalog,
            scope,
            SearchPlaneTrackKind::Lexical,
            "semantic scope",
        )?),
        (_, None) => None,
    };
    match (request.generation.clone(), outer_selection, scope_pin) {
        (Some(pin), Some(selection), Some(scope_pin))
            if pin != selection.pin || pin != scope_pin =>
        {
            Err(explicit_pin_mismatch_error(
                &pin,
                &[
                    (&selection.pin, request.generation_selector.as_ref()),
                    (
                        &scope_pin,
                        request
                            .lexical_scope
                            .as_ref()
                            .and_then(|scope| scope.generation_selector.as_ref()),
                    ),
                ],
                "semantic: scope generation does not match semantic request generation".to_string(),
            ))
        }
        (Some(pin), Some(selection), None) if pin != selection.pin => {
            Err(selection_mismatch_error(
                (&pin, None),
                (&selection.pin, request.generation_selector.as_ref()),
                "semantic: explicit generation pin does not match generation selector resolution"
                    .to_string(),
            ))
        }
        (Some(pin), None, Some(scope_pin)) if pin != scope_pin => Err(selection_mismatch_error(
            (&pin, None),
            (
                &scope_pin,
                request
                    .lexical_scope
                    .as_ref()
                    .and_then(|scope| scope.generation_selector.as_ref()),
            ),
            "semantic: scope generation does not match semantic request generation".to_string(),
        )),
        (None, Some(selection), Some(scope_pin)) if selection.pin != scope_pin => {
            Err(selection_mismatch_error(
                (&selection.pin, request.generation_selector.as_ref()),
                (
                    &scope_pin,
                    request
                        .lexical_scope
                        .as_ref()
                        .and_then(|scope| scope.generation_selector.as_ref()),
                ),
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
    validate_generation_scope(
        &[
            request.generation.as_ref(),
            request.text_query.generation.as_ref(),
        ],
        &[
            request.generation_selector.as_ref(),
            request.text_query.generation_selector.as_ref(),
        ],
        "hybrid",
    )?;
    let joint = resolve_joint_active_selection(
        activation_catalog,
        request.text_query.generation_selector.as_ref(),
        request.generation_selector.as_ref(),
        request.text_query.generation.as_ref(),
        "hybrid",
    )?;
    let lexical_pin = match &joint {
        Some(selection) => selection.pin.clone(),
        None => resolve_lexical_request_pin(
            activation_catalog,
            &request.text_query,
            SearchPlaneTrackKind::Lexical,
            "hybrid text_query",
        )?,
    };
    let semantic_selection = match joint {
        Some(selection) => Some(selection),
        None => match request.generation_selector.as_ref() {
            Some(selector) => Some(resolve_semantic_selector_selection(
                activation_catalog,
                selector,
                "hybrid",
            )?),
            None => None,
        },
    };
    match (request.generation.clone(), semantic_selection) {
        (Some(pin), Some(selection)) if pin != selection.pin || pin != lexical_pin => {
            Err(explicit_pin_mismatch_error(
                &pin,
                &[
                    (&selection.pin, request.generation_selector.as_ref()),
                    (
                        &lexical_pin,
                        request.text_query.generation_selector.as_ref(),
                    ),
                ],
                "hybrid: lexical generation does not match semantic generation".to_string(),
            ))
        }
        (Some(pin), None) if pin != lexical_pin => Err(selection_mismatch_error(
            (&pin, None),
            (
                &lexical_pin,
                request.text_query.generation_selector.as_ref(),
            ),
            "hybrid: lexical generation does not match semantic generation".to_string(),
        )),
        (None, Some(selection)) if selection.pin != lexical_pin => Err(selection_mismatch_error(
            (&selection.pin, request.generation_selector.as_ref()),
            (
                &lexical_pin,
                request.text_query.generation_selector.as_ref(),
            ),
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

fn lane_order_key(lane: SeedLane) -> u8 {
    match lane {
        SeedLane::Exact => 0,
        SeedLane::Bm25 => 1,
        SeedLane::Dense => 2,
    }
}

/// The corpus a lexical chunk hit is a member of (QI-BB-019).
///
/// The lexical index and the raw-code dense corpus index the same chunk
/// records, so a BM25 hit and a dense hit for one chunk are one seed, not
/// two.
const LEXICAL_LANE_CORPUS: SemanticCorpusKindV1 = SemanticCorpusKindV1::RawCodeFallback;

fn lexical_lane_seed_candidates(
    lexical: &[LexicalCandidate],
) -> Result<Vec<SeedCandidate>, CoreError> {
    let mut seen_identities = BTreeSet::new();
    let mut collapsed = Vec::new();
    for (index, candidate) in lexical.iter().enumerate() {
        let entity_id = candidate.candidate_id.clone();
        let identity = SeedFusionIdentity::new_with_corpus(
            OwnerDocKind::Chunk,
            entity_id.clone(),
            Some(LEXICAL_LANE_CORPUS),
        );
        if !seen_identities.insert(identity) {
            continue;
        }
        collapsed.push(SeedCandidate {
            record_id: candidate.candidate_id.clone(),
            entity_id,
            owner_kind: OwnerDocKind::Chunk,
            corpus_kind: Some(LEXICAL_LANE_CORPUS),
            authority_digest: None,
            repo_relative_path: candidate.repo_relative_path.clone(),
            snippet: candidate.snippet.clone(),
            seed_rank: checked_rank_u32_v1(index, "hybrid seed v2 lexical")?,
            contributions: vec![SeedContribution {
                lane: SeedLane::Bm25,
                rank: checked_rank_u32_v1(index, "hybrid seed v2 lexical")?,
                raw_score: Some(candidate.score),
                corpus_kind: None,
            }],
            degraded_reasons: Vec::new(),
        });
    }
    Ok(collapsed)
}

fn one_semantic_lane_seed_candidates(
    semantic_hits: &[SemanticSearchHitV1],
) -> Result<Vec<SeedCandidate>, CoreError> {
    let mut seen_identities = BTreeSet::new();
    let mut collapsed = Vec::new();
    for (index, hit) in semantic_hits.iter().enumerate() {
        let entity_id = hit.owner_id.clone();
        let identity =
            SeedFusionIdentity::new_with_corpus(hit.owner_kind, entity_id.clone(), hit.corpus_kind);
        if !seen_identities.insert(identity) {
            continue;
        }
        let mut degraded_reasons = Vec::new();
        if hit.corpus_kind.is_none() {
            degraded_reasons.push("semantic_corpus_kind_missing".to_string());
        }
        collapsed.push(SeedCandidate {
            record_id: hit.record_id.clone(),
            entity_id,
            owner_kind: hit.owner_kind,
            corpus_kind: hit.corpus_kind,
            authority_digest: Some(hit.authority_digest.clone()),
            repo_relative_path: hit.candidate.repo_relative_path.clone(),
            snippet: hit.candidate.snippet.clone(),
            seed_rank: checked_rank_u32_v1(index, "hybrid seed v2 semantic")?,
            contributions: vec![SeedContribution {
                lane: SeedLane::Dense,
                rank: checked_rank_u32_v1(index, "hybrid seed v2 semantic")?,
                raw_score: Some(hit.candidate.score),
                corpus_kind: hit.corpus_kind,
            }],
            degraded_reasons,
        });
    }
    Ok(collapsed)
}

fn fusion_identities(seed_candidates: &[SeedCandidate]) -> Vec<SeedFusionIdentity> {
    seed_candidates
        .iter()
        .map(SeedFusionIdentity::from)
        .collect()
}

fn merge_seed_candidate(acc: &mut SeedCandidate, incoming: SeedCandidate) {
    let prefers_incoming_identity = incoming
        .contributions
        .iter()
        .any(|contribution| contribution.lane == SeedLane::Dense)
        || acc.corpus_kind.is_none();
    if prefers_incoming_identity {
        acc.record_id = incoming.record_id;
        acc.owner_kind = incoming.owner_kind;
        acc.corpus_kind = incoming.corpus_kind;
        acc.authority_digest = incoming.authority_digest;
        acc.repo_relative_path = incoming.repo_relative_path;
        acc.snippet = incoming.snippet;
    }
    acc.contributions.extend(incoming.contributions);
    acc.degraded_reasons.extend(incoming.degraded_reasons);
}

pub(super) fn build_hybrid_seed_candidates(
    lexical: &[LexicalCandidate],
    semantic_lanes: &[Vec<SemanticSearchHitV1>],
    unavailable_corpus_reasons: &[String],
    top_k: u32,
) -> Result<Vec<SeedCandidate>, CoreError> {
    let collapsed_lexical = lexical_lane_seed_candidates(lexical)?;
    let collapsed_semantic_lanes = semantic_lanes
        .iter()
        .map(|lane| one_semantic_lane_seed_candidates(lane))
        .collect::<Result<Vec<_>, CoreError>>()?;
    let mut identity_lanes = Vec::with_capacity(collapsed_semantic_lanes.len().saturating_add(1));
    identity_lanes.push(fusion_identities(&collapsed_lexical));
    identity_lanes.extend(
        collapsed_semantic_lanes
            .iter()
            .map(|lane| fusion_identities(lane)),
    );
    let lane_refs = identity_lanes.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let fused = HybridOrchestratorPolicy::fuse_rrf_key_lanes(&lane_refs, top_k);

    let mut by_identity = BTreeMap::<SeedFusionIdentity, SeedCandidate>::new();
    for candidate in collapsed_lexical
        .into_iter()
        .chain(collapsed_semantic_lanes.into_iter().flatten())
    {
        let identity = SeedFusionIdentity::from(&candidate);
        if let Some(existing) = by_identity.get_mut(&identity) {
            merge_seed_candidate(existing, candidate);
        } else if by_identity.insert(identity, candidate).is_some() {
            return Err(CoreError::Storage(
                "hybrid seed v2: duplicate typed identity inserted after collapse".to_string(),
            ));
        }
    }

    fused
        .into_iter()
        .enumerate()
        .map(|(index, identity)| {
            let mut candidate = by_identity.remove(&identity).ok_or_else(|| {
                CoreError::Storage(format!(
                    "hybrid seed v2: fused typed identity {:?}/{} missing merged candidate payload",
                    identity.owner_kind(),
                    identity.entity_id()
                ))
            })?;
            candidate.seed_rank = checked_rank_u32_v1(index, "hybrid seed v2 fused")?;
            candidate.contributions.sort_by(|left, right| {
                lane_order_key(left.lane)
                    .cmp(&lane_order_key(right.lane))
                    .then(left.rank.cmp(&right.rank))
            });
            candidate
                .degraded_reasons
                .extend(unavailable_corpus_reasons.iter().cloned());
            candidate.degraded_reasons.sort();
            candidate.degraded_reasons.dedup();
            Ok(candidate)
        })
        .collect()
}

/// What each seed lane produced, as hits and as fused entities, plus the
/// observed backend-invocation truth the engine lists derive from.
pub(super) struct SeedLaneTallyV1 {
    pub(super) lexical_hits: usize,
    pub(super) lexical_entities: usize,
    pub(super) semantic_hits: usize,
    pub(super) semantic_entities: usize,
    pub(super) fused_hits: usize,
    pub(super) execution: LaneExecutionSummaryV1,
}

/// How one hybrid execution bound its DSL filters to the dense lane.
///
/// Planner trace entries (QI-BB-018 보완 #3): the push-down class per
/// filter (`hybrid.filters=pushdown:lang; exact:file`) and, per dense lane,
/// how the admission loop ended.
pub(super) struct HybridFilterTraceV1 {
    pub(super) filters: String,
    pub(super) admission: Vec<String>,
}

impl HybridFilterTraceV1 {
    fn trace_entries(&self) -> impl Iterator<Item = PlannerTraceEntry> + '_ {
        std::iter::once(PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail: self.filters.clone(),
        })
        .chain(self.admission.iter().map(|detail| PlannerTraceEntry {
            stage: PlannerStage::ExecFanout,
            detail: detail.clone(),
        }))
    }
}

pub(super) fn build_hybrid_seed_response_explanation(
    tally: &SeedLaneTallyV1,
    internal_top_k: u32,
    unavailable_corpus_reasons: &[String],
    early_stop_reason: Option<EarlyStopReason>,
    dense_lane: &DenseLaneContractV1,
    filters: &HybridFilterTraceV1,
    request_id: u64,
) -> SearchExplanation {
    let SeedLaneTallyV1 {
        lexical_hits,
        lexical_entities,
        semantic_hits,
        semantic_entities,
        fused_hits,
        execution,
    } = *tally;
    // Single-sourced (W10-R1): both engine lists derive from the observed
    // invocation truth, never from hit counts or plan shape.
    let engines_touched = execution.touched_engines();
    let engines_executed = execution.executed_engines();
    let strategy = match (lexical_entities > 0, semantic_entities > 0) {
        (true, true) => "rrf_entity",
        (true, false) => "bm25_entity_only",
        (false, true) => "dense_entity_only",
        (false, false) => "empty",
    }
    .to_string();
    let mut planner_trace = vec![
        PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail: format!("hybrid_seed.internal_top_k={internal_top_k}"),
        },
        dense_lane_trace_entry_v1(dense_lane),
    ];
    planner_trace.extend(filters.trace_entries());
    planner_trace.extend([
        PlannerTraceEntry {
            stage: PlannerStage::ExecFanout,
            detail: format!(
                "hybrid_seed.semantic_scoped_to_lexical=false; lexical_hits={lexical_hits}; lexical_entities={lexical_entities}; semantic_hits={semantic_hits}; semantic_entities={semantic_entities}"
            ),
        },
        PlannerTraceEntry {
            stage: PlannerStage::Merge,
            detail: format!("hybrid_seed.fused_entities={fused_hits}"),
        },
        PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail: format!(
                "hybrid_seed.unavailable_corpora={}",
                unavailable_corpus_reasons.join(",")
            ),
        },
    ]);
    SearchExplanation {
        planner_trace,
        engines_touched,
        engines_executed,
        // W10-R2: the route's budget correlation; 0 only off-transport.
        request_id,
        stage_timings: None,
        early_stop_reason,
        contributions: Vec::new(),
        ranker_weights_hash: [0u8; 32],
        strategy,
        summary: format!(
            "hybrid seed fused {lexical_entities} lexical entities and {semantic_entities} dense entities into {fused_hits} seed candidates"
        ),
    }
}

/// Reject a query whose embedder model identity differs from the indexed model.
///
/// Even at equal dimension, query vectors from a different model are not
/// cosine-comparable, so a same-dimension model swap would otherwise produce
/// silent garbage rankings. Fails closed (`SEM_MODEL_MISMATCH`).
pub(super) fn ensure_query_model_matches_index_v1(
    embedder_model_id: &str,
    embedder_model_revision: &str,
    index_model_id: &str,
    index_model_revision: Option<&str>,
    plane: &str,
) -> Result<(), CoreError> {
    // A generation sealed without a revision cannot prove it was embedded
    // by this revision; it is refused, not assumed (QI-BB-028).
    let Some(index_model_revision) = index_model_revision else {
        return Err(CoreError::Typed {
            code: LexicalErrorCode::SemModelMismatch.into(),
            message: format!(
                "{plane}: index model {index_model_id} was sealed without a model revision and cannot be compared to query embedder {embedder_model_id}/{embedder_model_revision}; reseal the generation"
            ),
        });
    };
    if embedder_model_id == index_model_id && embedder_model_revision == index_model_revision {
        return Ok(());
    }
    Err(CoreError::Typed {
        code: LexicalErrorCode::SemModelMismatch.into(),
        message: format!(
            "{plane}: query embedder model {embedder_model_id}/{embedder_model_revision} is not comparable to index model {index_model_id}/{index_model_revision} (equal dimension is insufficient; vectors from different models or revisions are not cosine-comparable)"
        ),
    })
}

#[cfg(test)]
mod corpus_budget_tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "test assertions index the canonical budget output after constructing a fixed two-entry fixture"
    )]
    use quanta_index_contract::SemanticCorpusKindV1;

    use super::{SemanticSeedCorpusBudgetV1, canonical_dense_corpus_budgets_v1};

    #[test]
    fn corpus_budgets_are_canonical_and_duplicate_or_zero_budgets_fail_closed() {
        let canonical = canonical_dense_corpus_budgets_v1(&[
            SemanticSeedCorpusBudgetV1 {
                corpus_kind: SemanticCorpusKindV1::SymbolCard,
                top_k: 40,
            },
            SemanticSeedCorpusBudgetV1 {
                corpus_kind: SemanticCorpusKindV1::ModuleCard,
                top_k: 20,
            },
        ])
        .expect("valid budgets");
        assert_eq!(canonical[0].corpus_kind, SemanticCorpusKindV1::ModuleCard);
        assert_eq!(canonical[1].corpus_kind, SemanticCorpusKindV1::SymbolCard);

        assert!(
            canonical_dense_corpus_budgets_v1(&[
                SemanticSeedCorpusBudgetV1 {
                    corpus_kind: SemanticCorpusKindV1::SymbolCard,
                    top_k: 40,
                },
                SemanticSeedCorpusBudgetV1 {
                    corpus_kind: SemanticCorpusKindV1::SymbolCard,
                    top_k: 20,
                },
            ])
            .is_err()
        );
        assert!(
            canonical_dense_corpus_budgets_v1(&[SemanticSeedCorpusBudgetV1 {
                corpus_kind: SemanticCorpusKindV1::ModuleCard,
                top_k: 0,
            },])
            .is_err()
        );
    }
}

#[cfg(test)]
mod seed_fusion_tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "test assertions index fusion output after asserting its expected cardinality"
    )]
    use quanta_index_contract::{
        LexicalCandidate, ManifestGeneration, OwnerDocKind, RepoId, RepoRelativePath, RevisionId,
        SeedFusionIdentity, SeedLane, SemanticCorpusKindV1,
    };
    use quanta_index_core::domains::semantic::SemanticSearchHitV1;

    use super::build_hybrid_seed_candidates;

    const EXACT_SYMBOL_ID: &str =
        "runtime_symbol_id_v1:src/session.rs:Function:RuntimeSession::commit:17";

    fn lexical_candidate(id: &str) -> LexicalCandidate {
        LexicalCandidate {
            candidate_id: id.to_string(),
            repo_id: RepoId::new("repo-seed-fusion")
                .expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-seed-fusion")
                .expect("static fixture ID satisfies canonical policy"),
            manifest_generation: ManifestGeneration::new(1),
            repo_relative_path: RepoRelativePath::new("src/session.rs"),
            start_line: 0,
            end_line: 0,
            score: 1.0,
            snippet: "seed fusion fixture".to_string(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        }
    }

    fn semantic_hit(
        record_id: &str,
        owner_id: &str,
        owner_kind: OwnerDocKind,
        corpus_kind: SemanticCorpusKindV1,
    ) -> SemanticSearchHitV1 {
        SemanticSearchHitV1 {
            candidate: lexical_candidate(record_id),
            record_id: record_id.to_string(),
            owner_id: owner_id.to_string(),
            owner_kind,
            corpus_kind: Some(corpus_kind),
            authority_digest: format!("authority:{record_id}"),
        }
    }

    fn symbol_hit(record_id: &str) -> SemanticSearchHitV1 {
        semantic_hit(
            record_id,
            EXACT_SYMBOL_ID,
            OwnerDocKind::Symbol,
            SemanticCorpusKindV1::SymbolCard,
        )
    }

    /// One chunk seen by both lanes is one seed (QI-BB-019).
    ///
    /// A BM25 hit and a raw-code dense hit for the same chunk are one seed
    /// with both contributions, and that seed outranks a chunk only one
    /// lane found: the fusion sees both lanes, not a lexical seed and an
    /// unrelated dense seed side by side.
    #[test]
    fn seed_fusion_merges_a_chunk_across_the_bm25_and_raw_code_lanes() {
        let lexical = [lexical_candidate("beta"), lexical_candidate("alpha")];
        let dense = vec![
            semantic_hit(
                "alpha",
                "alpha",
                OwnerDocKind::Chunk,
                SemanticCorpusKindV1::RawCodeFallback,
            ),
            semantic_hit(
                "gamma",
                "gamma",
                OwnerDocKind::Chunk,
                SemanticCorpusKindV1::RawCodeFallback,
            ),
        ];
        let seeds = build_hybrid_seed_candidates(&lexical, &[dense], &[], 3)
            .expect("chunk seeds fuse across lanes");
        let alpha = seeds
            .iter()
            .find(|seed| seed.entity_id == "alpha")
            .expect("alpha is a seed");
        let lanes: Vec<SeedLane> = alpha
            .contributions
            .iter()
            .map(|contribution| contribution.lane)
            .collect();
        assert_eq!(lanes, vec![SeedLane::Bm25, SeedLane::Dense], "{alpha:?}");
        assert_eq!(
            alpha.corpus_kind,
            Some(SemanticCorpusKindV1::RawCodeFallback)
        );
        assert_eq!(alpha.seed_rank, 1, "two lanes outrank one: {seeds:?}");
        assert_eq!(
            seeds
                .iter()
                .filter(|seed| seed.entity_id == "alpha")
                .count(),
            1,
            "one chunk is one seed: {seeds:?}"
        );
        assert_eq!(seeds.len(), 3, "{seeds:?}");
    }

    #[test]
    fn seed_fusion_identity_contract_orders_owner_before_entity() {
        let symbol_z = SeedFusionIdentity::new(OwnerDocKind::Symbol, "z".to_string());
        let chunk_a = SeedFusionIdentity::new(OwnerDocKind::Chunk, "a".to_string());
        let symbol_a = SeedFusionIdentity::new(OwnerDocKind::Symbol, "a".to_string());

        let mut identities = [chunk_a, symbol_z, symbol_a];
        identities.sort();

        assert_eq!(identities[0].owner_kind(), OwnerDocKind::Symbol);
        assert_eq!(identities[0].entity_id(), "a");
        assert_eq!(identities[1].owner_kind(), OwnerDocKind::Symbol);
        assert_eq!(identities[1].entity_id(), "z");
        assert_eq!(identities[2].owner_kind(), OwnerDocKind::Chunk);
        assert_eq!(identities[2].entity_id(), "a");
    }

    #[test]
    fn module_and_cluster_cards_with_one_owner_keep_distinct_authority() {
        let owner_id = "runtime_module_id_v1:src/shared.rs";
        let semantic_lanes = vec![
            vec![SemanticSearchHitV1 {
                candidate: lexical_candidate("module-record"),
                record_id: "module-record".to_string(),
                owner_id: owner_id.to_string(),
                owner_kind: OwnerDocKind::Module,
                corpus_kind: Some(SemanticCorpusKindV1::ModuleCard),
                authority_digest: "module-authority".to_string(),
            }],
            vec![SemanticSearchHitV1 {
                candidate: lexical_candidate("cluster-record"),
                record_id: "cluster-record".to_string(),
                owner_id: owner_id.to_string(),
                owner_kind: OwnerDocKind::Module,
                corpus_kind: Some(SemanticCorpusKindV1::ClusterCard),
                authority_digest: "cluster-authority".to_string(),
            }],
        ];

        let seeds = build_hybrid_seed_candidates(&[], &semantic_lanes, &[], 2)
            .expect("typed corpus identities must remain independently ranked");
        assert_eq!(seeds.len(), 2);
        assert!(seeds.iter().any(|seed| {
            seed.corpus_kind == Some(SemanticCorpusKindV1::ModuleCard)
                && seed.authority_digest.as_deref() == Some("module-authority")
        }));
        assert!(seeds.iter().any(|seed| {
            seed.corpus_kind == Some(SemanticCorpusKindV1::ClusterCard)
                && seed.authority_digest.as_deref() == Some("cluster-authority")
        }));
    }

    #[test]
    fn seed_fusion_merges_same_exact_symbol_identity_across_dense_lanes() {
        let seeds = build_hybrid_seed_candidates(
            &[],
            &[
                vec![symbol_hit("symbol-card-primary")],
                vec![symbol_hit("symbol-card-secondary")],
            ],
            &[],
            2,
        )
        .expect("same symbol identity must be fusable");

        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].entity_id, EXACT_SYMBOL_ID);
        assert_eq!(seeds[0].owner_kind, OwnerDocKind::Symbol);
        assert_eq!(seeds[0].contributions.len(), 2);
    }

    #[test]
    fn seed_fusion_keeps_chunk_and_symbol_with_same_opaque_text_independent() {
        let seeds = build_hybrid_seed_candidates(
            &[lexical_candidate(EXACT_SYMBOL_ID)],
            &[vec![symbol_hit("symbol-card")]],
            &[],
            2,
        )
        .expect("different owner domains must not collapse");

        assert_eq!(seeds.len(), 2);
        assert_eq!(seeds[0].owner_kind, OwnerDocKind::Chunk);
        assert_eq!(seeds[0].entity_id, EXACT_SYMBOL_ID);
        assert_eq!(seeds[0].record_id, EXACT_SYMBOL_ID);
        assert_eq!(seeds[1].owner_kind, OwnerDocKind::Symbol);
        assert_eq!(seeds[1].entity_id, EXACT_SYMBOL_ID);
        assert_eq!(seeds[1].record_id, "symbol-card");
    }

    #[test]
    fn seed_fusion_keeps_cross_owner_identity_distinct_within_one_dense_lane() {
        let seeds = build_hybrid_seed_candidates(
            &[],
            &[vec![
                semantic_hit(
                    "module-card",
                    EXACT_SYMBOL_ID,
                    OwnerDocKind::Module,
                    SemanticCorpusKindV1::ModuleCard,
                ),
                symbol_hit("symbol-card"),
            ]],
            &[],
            2,
        )
        .expect("one dense lane must retain cross-owner identities");

        assert_eq!(seeds.len(), 2);
        assert_eq!(seeds[0].owner_kind, OwnerDocKind::Module);
        assert_eq!(seeds[0].entity_id, EXACT_SYMBOL_ID);
        assert_eq!(seeds[1].owner_kind, OwnerDocKind::Symbol);
        assert_eq!(seeds[1].entity_id, EXACT_SYMBOL_ID);
    }

    #[test]
    fn seed_fusion_semantic_tie_uses_canonical_typed_identity_order() {
        let seeds = build_hybrid_seed_candidates(
            &[],
            &[
                vec![symbol_hit("symbol-card")],
                vec![semantic_hit(
                    "module-card",
                    EXACT_SYMBOL_ID,
                    OwnerDocKind::Module,
                    SemanticCorpusKindV1::ModuleCard,
                )],
            ],
            &[],
            1,
        )
        .expect("semantic-only rank ties must use the contract identity order");

        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].owner_kind, OwnerDocKind::Module);
        assert_eq!(seeds[0].entity_id, EXACT_SYMBOL_ID);
        assert_eq!(seeds[0].record_id, "module-card");
    }
}

/// The lexical scope a semantic query was narrowed to: the cap the caller
/// asked for and the ranked candidate ids the lexical lane produced under it.
///
/// `candidate_ids.len() <= requested_cap` by construction; the dispatcher
/// refuses an adapter answer that exceeds the cap instead of truncating it.
pub(super) struct SemanticScopeV1 {
    pub(super) requested_cap: u32,
    pub(super) candidate_ids: BTreeSet<String>,
}

pub(super) fn build_semantic_response_explanation(
    scope: Option<&SemanticScopeV1>,
    result_count: usize,
    early_stop_reason: Option<EarlyStopReason>,
    dense_lane: &DenseLaneContractV1,
    execution: &LaneExecutionSummaryV1,
    request_id: u64,
) -> SearchExplanation {
    let scoped = scope.is_some();
    let scope_candidate_count = scope.map_or(0, |scope| scope.candidate_ids.len());
    let mut planner_trace = vec![
        PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail: format!("semantic.scope={scoped}"),
        },
        dense_lane_trace_entry_v1(dense_lane),
    ];
    if let Some(scope) = scope {
        planner_trace.push(PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail: format!("semantic.scope.cap={}", scope.requested_cap),
        });
        planner_trace.push(PlannerTraceEntry {
            stage: PlannerStage::ExecFanout,
            detail: format!("semantic.scope.text_candidates={scope_candidate_count}"),
        });
    }
    planner_trace.push(PlannerTraceEntry {
        stage: PlannerStage::Merge,
        detail: format!("semantic.results={result_count}"),
    });
    // Single-sourced (W10-R1): both engine lists derive from the observed
    // invocation truth, never from hit counts or plan shape.
    let engines_touched = execution.touched_engines();
    let engines_executed = execution.executed_engines();
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
        engines_executed,
        // W10-R2: the route's budget correlation; 0 only off-transport.
        request_id,
        stage_timings: None,
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

/// What each hybrid lane produced and what the fusion made of it, plus
/// the observed backend-invocation truth the engine lists derive from.
pub(super) struct HybridLaneTallyV1 {
    pub(super) lexical_hits: usize,
    pub(super) semantic_hits: usize,
    pub(super) fused_universe: usize,
    pub(super) fused_hits: usize,
    pub(super) execution: LaneExecutionSummaryV1,
}

pub(super) fn build_hybrid_response_explanation(
    tally: &HybridLaneTallyV1,
    internal_top_k: u32,
    early_stop_reason: Option<EarlyStopReason>,
    dense_lane: &DenseLaneContractV1,
    filters: &HybridFilterTraceV1,
    request_id: u64,
) -> SearchExplanation {
    let HybridLaneTallyV1 {
        lexical_hits,
        semantic_hits,
        fused_universe,
        fused_hits,
        execution,
    } = *tally;
    // Strategy still reflects which lanes returned candidates (a genuine
    // two-lane RRF only when BOTH did); the engine lists below are
    // single-sourced from the observed invocation truth (W10-R1).
    let engines_touched = execution.touched_engines();
    let strategy = match (lexical_hits > 0, semantic_hits > 0) {
        (true, true) => "rrf",
        (true, false) => "lexical_only",
        (false, true) => "semantic_only",
        (false, false) => "empty",
    }
    .to_string();
    let mut planner_trace = vec![
        PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail: format!("hybrid.internal_top_k={internal_top_k}"),
        },
        dense_lane_trace_entry_v1(dense_lane),
    ];
    planner_trace.extend(filters.trace_entries());
    planner_trace.extend([
        PlannerTraceEntry {
            stage: PlannerStage::ExecFanout,
            detail: format!(
                "hybrid.lanes=independent; lexical_hits={lexical_hits}; semantic_hits={semantic_hits}; fused_universe={fused_universe}"
            ),
        },
        PlannerTraceEntry {
            stage: PlannerStage::Merge,
            detail: format!("hybrid.fused_results={fused_hits}"),
        },
    ]);
    SearchExplanation {
        planner_trace,
        engines_touched,
        engines_executed: execution.executed_engines(),
        // W10-R2: the route's budget correlation; 0 only off-transport.
        request_id,
        stage_timings: None,
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
