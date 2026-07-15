//! Semantic / hybrid query-path helpers for the search-plane dispatcher.
//!
//! Selection resolution, the query-time model-identity gate, RRF/seed
//! candidate assembly, and response-explanation builders for the semantic,
//! hybrid, and hybrid-seed query paths. Split out of `query_dispatcher` so the
//! dispatcher file is not the sole home for every query mode. This is a child
//! module: `use super::*` pulls in the shared resolvers, policies, and types it
//! builds on, and the moved items are `pub(super)` so the dispatcher methods
//! (which stay in `query_dispatcher`) keep calling them unchanged.

use super::{
    ActivationCatalog, BTreeMap, BTreeSet, CoreError, EarlyStopReason, EngineTouched,
    GenerationPin, HybridOrchestratorPolicy, HybridQueryRequest, HybridSeedCandidate,
    HybridSeedLane, HybridSeedQueryRequest, LexicalCandidate, LexicalErrorCode, OwnerDocKind,
    PlannerStage, PlannerTraceEntry, QueryResultWindowV1, SearchExplanation, SearchPlaneTrackKind,
    SeedCandidateV2, SeedContributionV2, SeedLaneV2, SemanticPolicy, SemanticQueryRequest,
    SemanticSearchHitV1, resolve_lexical_request_pin, resolve_semantic_selector_selection,
};
use quanta_index_contract::{SeedFusionIdentityV2, SemanticSeedCorpusBudgetV1};

#[derive(Clone, Debug)]
pub(super) struct SemanticSelection {
    pub(super) pin: GenerationPin,
    pub(super) expected_manifest_digest: Option<String>,
}

/// Outcome of the shared hybrid lexical+semantic fusion, before the caller
/// wraps it in its path-specific response (plain hybrid vs. hybrid-seed).
pub(super) struct HybridFusion {
    pub(super) pin: GenerationPin,
    pub(super) fused: Vec<LexicalCandidate>,
    pub(super) window: QueryResultWindowV1,
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

fn lane_order_key_v2(lane: SeedLaneV2) -> u8 {
    match lane {
        SeedLaneV2::Exact => 0,
        SeedLaneV2::Bm25 => 1,
        SeedLaneV2::Dense => 2,
    }
}

fn lexical_lane_seed_candidates_v2(
    lexical: &[LexicalCandidate],
) -> Result<Vec<SeedCandidateV2>, CoreError> {
    let mut seen_identities = BTreeSet::new();
    let mut collapsed = Vec::new();
    for (index, candidate) in lexical.iter().enumerate() {
        let entity_id = candidate.candidate_id.clone();
        let identity = SeedFusionIdentityV2::new(OwnerDocKind::Chunk, entity_id.clone());
        if !seen_identities.insert(identity) {
            continue;
        }
        collapsed.push(SeedCandidateV2 {
            record_id: candidate.candidate_id.clone(),
            entity_id,
            owner_kind: OwnerDocKind::Chunk,
            corpus_kind: None,
            repo_relative_path: candidate.repo_relative_path.clone(),
            snippet: candidate.snippet.clone(),
            seed_rank: checked_rank_u32_v1(index, "hybrid seed v2 lexical")?,
            contributions: vec![SeedContributionV2 {
                lane: SeedLaneV2::Bm25,
                rank: checked_rank_u32_v1(index, "hybrid seed v2 lexical")?,
                raw_score: Some(candidate.score),
                corpus_kind: None,
            }],
            degraded_reasons: Vec::new(),
        });
    }
    Ok(collapsed)
}

fn one_semantic_lane_seed_candidates_v2(
    semantic_hits: &[SemanticSearchHitV1],
) -> Result<Vec<SeedCandidateV2>, CoreError> {
    let mut seen_identities = BTreeSet::new();
    let mut collapsed = Vec::new();
    for (index, hit) in semantic_hits.iter().enumerate() {
        let entity_id = hit.owner_id.clone();
        let identity = SeedFusionIdentityV2::new(hit.owner_kind, entity_id.clone());
        if !seen_identities.insert(identity) {
            continue;
        }
        let mut degraded_reasons = Vec::new();
        if hit.corpus_kind.is_none() {
            degraded_reasons.push("semantic_corpus_kind_missing".to_string());
        }
        collapsed.push(SeedCandidateV2 {
            record_id: hit.record_id.clone(),
            entity_id,
            owner_kind: hit.owner_kind,
            corpus_kind: hit.corpus_kind,
            repo_relative_path: hit.candidate.repo_relative_path.clone(),
            snippet: hit.candidate.snippet.clone(),
            seed_rank: checked_rank_u32_v1(index, "hybrid seed v2 semantic")?,
            contributions: vec![SeedContributionV2 {
                lane: SeedLaneV2::Dense,
                rank: checked_rank_u32_v1(index, "hybrid seed v2 semantic")?,
                raw_score: Some(hit.candidate.score),
                corpus_kind: hit.corpus_kind,
            }],
            degraded_reasons,
        });
    }
    Ok(collapsed)
}

fn fusion_identities_v2(seed_candidates: &[SeedCandidateV2]) -> Vec<SeedFusionIdentityV2> {
    seed_candidates
        .iter()
        .map(SeedFusionIdentityV2::from)
        .collect()
}

fn merge_seed_candidate_v2(acc: &mut SeedCandidateV2, incoming: SeedCandidateV2) {
    let prefers_incoming_identity = incoming
        .contributions
        .iter()
        .any(|contribution| contribution.lane == SeedLaneV2::Dense)
        || acc.corpus_kind.is_none();
    if prefers_incoming_identity {
        acc.record_id = incoming.record_id;
        acc.owner_kind = incoming.owner_kind;
        acc.corpus_kind = incoming.corpus_kind;
        acc.repo_relative_path = incoming.repo_relative_path;
        acc.snippet = incoming.snippet;
    }
    acc.contributions.extend(incoming.contributions);
    acc.degraded_reasons.extend(incoming.degraded_reasons);
}

pub(super) fn build_hybrid_seed_candidates_v2(
    lexical: &[LexicalCandidate],
    semantic_lanes: &[Vec<SemanticSearchHitV1>],
    unavailable_corpus_reasons: &[String],
    top_k: u32,
) -> Result<Vec<SeedCandidateV2>, CoreError> {
    let collapsed_lexical = lexical_lane_seed_candidates_v2(lexical)?;
    let collapsed_semantic_lanes = semantic_lanes
        .iter()
        .map(|lane| one_semantic_lane_seed_candidates_v2(lane))
        .collect::<Result<Vec<_>, CoreError>>()?;
    let mut identity_lanes = Vec::with_capacity(collapsed_semantic_lanes.len().saturating_add(1));
    identity_lanes.push(fusion_identities_v2(&collapsed_lexical));
    identity_lanes.extend(
        collapsed_semantic_lanes
            .iter()
            .map(|lane| fusion_identities_v2(lane)),
    );
    let lane_refs = identity_lanes.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let fused = HybridOrchestratorPolicy::fuse_rrf_key_lanes(&lane_refs, top_k);

    let mut by_identity = BTreeMap::<SeedFusionIdentityV2, SeedCandidateV2>::new();
    for candidate in collapsed_lexical
        .into_iter()
        .chain(collapsed_semantic_lanes.into_iter().flatten())
    {
        let identity = SeedFusionIdentityV2::from(&candidate);
        if let Some(existing) = by_identity.get_mut(&identity) {
            merge_seed_candidate_v2(existing, candidate);
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
                lane_order_key_v2(left.lane)
                    .cmp(&lane_order_key_v2(right.lane))
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

pub(super) fn build_hybrid_seed_response_explanation_v2(
    lexical_hits: usize,
    lexical_entities: usize,
    semantic_hits: usize,
    semantic_entities: usize,
    fused_hits: usize,
    internal_top_k: u32,
    unavailable_corpus_reasons: &[String],
    early_stop_reason: Option<EarlyStopReason>,
) -> SearchExplanation {
    let mut engines_touched = Vec::new();
    if lexical_hits > 0 {
        engines_touched.push(EngineTouched::Lexical);
    }
    if semantic_hits > 0 {
        engines_touched.push(EngineTouched::Semantic);
    }
    let strategy = match (lexical_entities > 0, semantic_entities > 0) {
        (true, true) => "rrf_entity",
        (true, false) => "bm25_entity_only",
        (false, true) => "dense_entity_only",
        (false, false) => "empty",
    }
    .to_string();
    SearchExplanation {
        planner_trace: vec![
            PlannerTraceEntry {
                stage: PlannerStage::Plan,
                detail: format!("hybrid_seed.internal_top_k={internal_top_k}"),
            },
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
        ],
        engines_touched,
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
        SeedFusionIdentityV2, SemanticCorpusKindV1,
    };
    use quanta_index_core::domains::semantic::SemanticSearchHitV1;

    use super::build_hybrid_seed_candidates_v2;

    const EXACT_SYMBOL_ID: &str =
        "runtime_symbol_id_v1:src/session.rs:Function:RuntimeSession::commit:17";

    fn lexical_candidate(id: &str) -> LexicalCandidate {
        LexicalCandidate {
            candidate_id: id.to_string(),
            repo_id: RepoId::new("repo-seed-fusion"),
            revision_id: RevisionId::new("rev-seed-fusion"),
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

    #[test]
    fn seed_fusion_identity_contract_orders_owner_before_entity_v2() {
        let symbol_z = SeedFusionIdentityV2::new(OwnerDocKind::Symbol, "z".to_string());
        let chunk_a = SeedFusionIdentityV2::new(OwnerDocKind::Chunk, "a".to_string());
        let symbol_a = SeedFusionIdentityV2::new(OwnerDocKind::Symbol, "a".to_string());

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
    fn seed_fusion_merges_same_exact_symbol_identity_across_dense_lanes() {
        let seeds = build_hybrid_seed_candidates_v2(
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
        let seeds = build_hybrid_seed_candidates_v2(
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
        let seeds = build_hybrid_seed_candidates_v2(
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
        let seeds = build_hybrid_seed_candidates_v2(
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
