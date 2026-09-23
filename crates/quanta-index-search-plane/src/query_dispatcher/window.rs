//! Top-k probing and result-window finalization (`QueryResultWindowV1`,
//! `QueryResultWindowV2`) for lexical, semantic, and fused lanes.

use quanta_index_contract::{
    CandidateCountV1, CoverageV1, EmptyProvenanceV2, ExaminedUniverseV1, ExecutionOutcomeV2,
    ExhaustionProofV1, LaneTraceV1, LqQuery, QueryResultWindowV1, QueryResultWindowV2,
    continuation_fetch_size,
};
use quanta_index_core::{
    CoreError, DenseAdmissionOutcomeV1, DenseLaneContractV1, HybridOrchestratorPolicy,
    LexicalSearchPageV1, validate_query_top_k,
};

/// Rows to fetch for one query so the window can observe a continuation row.
///
/// The public cap and the internal fetch ceiling are different numbers owned
/// by the contract: `top_k = 10_000` is accepted and fetches 10,001. This used
/// to refuse the public maximum because it compared `top_k + 1` against the
/// public cap itself (QI-BB-025).
pub(super) fn probe_top_k_v1(top_k: u32) -> Result<u32, CoreError> {
    let accepted = validate_query_top_k(top_k)?;
    Ok(continuation_fetch_size(accepted))
}

/// Whether the query carries a `count` option, in which case the adapter
/// reports an exact total and the page needs no continuation probe.
fn requests_exact_total_v1(query: &LqQuery) -> bool {
    query.options.count.is_some()
}

/// Rows to ask the lexical adapter for.
///
/// The page plus one continuation probe, unless the adapter will report an
/// exact total anyway (QI-BB-005: `count:all` no longer widens the page; the
/// total comes from a count collector and the rows stay bounded by `top_k`).
pub(super) fn lexical_fetch_limit_v1(
    query: &LqQuery,
    requested_top_k: u32,
) -> Result<u32, CoreError> {
    if requests_exact_total_v1(query) {
        return validate_query_top_k(requested_top_k);
    }
    probe_top_k_v1(requested_top_k)
}

/// Window for a page whose adapter proved the exact match total.
pub(super) fn exact_total_window_v1(
    returned: usize,
    total: u64,
) -> Result<QueryResultWindowV1, CoreError> {
    let returned = u32::try_from(returned).map_err(|err| {
        CoreError::InvalidContract(format!("lexical page row count exceeds u32: {err}"))
    })?;
    if total < u64::from(returned) {
        return Err(CoreError::InvalidContract(format!(
            "lexical adapter reported an exact total of {total} below the {returned} rows it returned"
        )));
    }
    QueryResultWindowV1::new(
        returned,
        CandidateCountV1::Exact(total),
        total > u64::from(returned),
    )
    .map_err(|err| CoreError::InvalidContract(format!("lexical exact window: {err}")))
}

/// Window for one lexical page: exact when the adapter proved the total,
/// otherwise derived from the continuation probe.
///
/// `fetched_top_k` is what the adapter was asked for (the page, or the page
/// plus its probe row); more rows than that is a contract defect. With an
/// exact total the probe row, if any, is simply cut — the total already
/// says whether more exist.
pub(super) fn lexical_page_window_v1(
    page: &mut LexicalSearchPageV1,
    requested_top_k: u32,
    fetched_top_k: u32,
) -> Result<QueryResultWindowV1, CoreError> {
    let fetched = top_k_limit(fetched_top_k);
    if page.candidates.len() > fetched {
        return Err(CoreError::InvalidContract(format!(
            "lexical adapter returned {} rows for a fetch of {fetched}",
            page.candidates.len()
        )));
    }
    match page.exact_total {
        Some(total) => {
            page.candidates.truncate(top_k_limit(requested_top_k));
            exact_total_window_v1(page.candidates.len(), total)
        }
        None => finalize_probe_window_v1(&mut page.candidates, requested_top_k),
    }
}

pub(super) fn hybrid_probe_top_k_v1(top_k: u32) -> Result<u32, CoreError> {
    Ok(HybridOrchestratorPolicy::over_fetch_top_k(top_k).max(probe_top_k_v1(top_k)?))
}

pub(super) fn finalize_probe_window_v1<T>(
    results: &mut Vec<T>,
    top_k: u32,
) -> Result<QueryResultWindowV1, CoreError> {
    let observed = results.len();
    let requested = usize::try_from(top_k)
        .map_err(|err| CoreError::InvalidContract(format!("query top_k overflow: {err}")))?;
    if results.len() > requested {
        results.truncate(requested);
    }
    QueryResultWindowV1::from_probe(top_k, observed)
        .map_err(|err| CoreError::InvalidContract(format!("query result window: {err}")))
}

/// Lift the legacy page arithmetic into the single V2 completeness
/// authority. This is the only bridge while adapters still return V1
/// probe/count facts; public responses never expose both windows.
pub(super) fn pageable_window_v2(
    window: QueryResultWindowV1,
    lane: &'static str,
) -> Result<QueryResultWindowV2, CoreError> {
    let returned = window.returned();
    let candidate_count = window.candidate_count();
    QueryResultWindowV2::pageable(
        returned,
        candidate_count,
        window.has_more(),
        vec![LaneTraceV1::new(lane, true, returned > 0).with_candidates(candidate_count)],
    )
    .map_err(|error| CoreError::InvalidContract(format!("pageable result window v2: {error}")))
}

/// Reframe an already valid pageable window after the cut.
///
/// The response byte budget cuts it to a strict prefix. The cut proves a
/// continuation regardless of whether the pre-cut page had exhausted its
/// backend universe.
pub(super) fn cut_pageable_window_v2(
    window: &QueryResultWindowV2,
    returned: usize,
) -> Result<QueryResultWindowV2, CoreError> {
    let returned = u32::try_from(returned)
        .map_err(|error| CoreError::InvalidContract(format!("cut page rows: {error}")))?;
    let minimum = u64::from(returned).saturating_add(1);
    let candidate_count = match window.candidate_count() {
        CandidateCountV1::Exact(total) => CandidateCountV1::Exact(total.max(minimum)),
        CandidateCountV1::AtLeast(lower) => CandidateCountV1::AtLeast(lower.max(minimum)),
    };
    QueryResultWindowV2::new(
        returned,
        candidate_count,
        ExecutionOutcomeV2::LowerBound { continuation: true },
        CoverageV1::new(
            match window.coverage().examined() {
                ExaminedUniverseV1::Exact(exact) => ExaminedUniverseV1::Exact(exact),
                ExaminedUniverseV1::AtLeast(lower) => ExaminedUniverseV1::AtLeast(lower),
                ExaminedUniverseV1::Unknown => ExaminedUniverseV1::Unknown,
            },
            None,
            window.coverage().lanes().to_vec(),
        ),
        None,
    )
    .map_err(|error| CoreError::InvalidContract(format!("cut page window v2: {error}")))
}

/// A lane count as `u64`, failing closed instead of saturating.
pub(super) fn lane_count_u64(count: usize) -> Result<u64, CoreError> {
    u64::try_from(count).map_err(|err| {
        CoreError::InvalidContract(format!("lane candidate count exceeds u64: {err}"))
    })
}

pub(super) fn top_k_limit(top_k: u32) -> usize {
    usize::try_from(top_k).map_or(usize::MAX, core::convert::identity)
}

/// V2 window for one fused (hybrid / hybrid-seed) page.
///
/// The V1 probe arithmetic is unchanged; what V2 adds is that the dense
/// admission outcome survives to the outer window: a capped admission is
/// reported as `CappedUnknown`, a filled lane as a lower bound with an
/// observed continuation, and only a genuinely exhausted universe as
/// `ExactExhausted` with its proof.
pub(super) fn fused_window_v2(
    top_k: u32,
    returned: usize,
    observed_universe: usize,
    lane_limit_reached: bool,
    dense_outcome: Option<DenseAdmissionOutcomeV1>,
    internal_cap: u32,
    lanes: Vec<LaneTraceV1>,
) -> Result<QueryResultWindowV2, CoreError> {
    let requested = usize::try_from(top_k)
        .map_err(|err| CoreError::InvalidContract(format!("query top_k overflow: {err}")))?;
    let returned_u32 = u32::try_from(returned)
        .map_err(|err| CoreError::InvalidContract(format!("fused rows exceed u32: {err}")))?;
    let universe_u64 = u64::try_from(observed_universe)
        .map_err(|err| CoreError::InvalidContract(format!("fused universe exceeds u64: {err}")))?;
    let filtered_ran = dense_outcome.is_some();
    // The outcome the lanes proved, most to least specific.
    // A continuation can only be reported when the fused universe holds
    // more distinct rows than the page returned: a lane at its internal
    // limit whose rows all deduplicated into the page proves nothing
    // about further distinct rows, so it stays `CappedUnknown`.
    let fused_continuation = observed_universe > returned;
    let outcome = match dense_outcome {
        Some(DenseAdmissionOutcomeV1::Capped) => {
            ExecutionOutcomeV2::CappedUnknown { cap: internal_cap }
        }
        Some(DenseAdmissionOutcomeV1::Exhausted) => ExecutionOutcomeV2::LowerBound {
            continuation: fused_continuation,
        },
        Some(DenseAdmissionOutcomeV1::NotNeeded) => {
            if returned < requested {
                // No filter was evaluated per candidate, so the fetch is a
                // plain probe: observing fewer rows than requested proves
                // the universe end.
                ExecutionOutcomeV2::ExactExhausted
            } else if fused_continuation {
                ExecutionOutcomeV2::LowerBound { continuation: true }
            } else {
                ExecutionOutcomeV2::CappedUnknown { cap: internal_cap }
            }
        }
        Some(DenseAdmissionOutcomeV1::Filled) => {
            if fused_continuation {
                ExecutionOutcomeV2::LowerBound { continuation: true }
            } else {
                ExecutionOutcomeV2::CappedUnknown { cap: internal_cap }
            }
        }
        None => {
            if returned < requested {
                ExecutionOutcomeV2::ExactExhausted
            } else if lane_limit_reached && fused_continuation {
                ExecutionOutcomeV2::LowerBound { continuation: true }
            } else {
                // The page filled without an observable continuation row:
                // nothing proves exhaustion either way past the internal
                // fetch ceiling.
                ExecutionOutcomeV2::CappedUnknown { cap: internal_cap }
            }
        }
    };
    let candidate_count = match outcome {
        ExecutionOutcomeV2::ExactExhausted => CandidateCountV1::Exact(u64::from(returned_u32)),
        ExecutionOutcomeV2::LowerBound { .. }
        | ExecutionOutcomeV2::CappedUnknown { .. }
        | ExecutionOutcomeV2::InterruptedPartial { .. }
        | ExecutionOutcomeV2::Approximate { .. } => {
            CandidateCountV1::AtLeast(universe_u64.max(u64::from(returned_u32)))
        }
    };
    let exhaustion_proof = match outcome {
        ExecutionOutcomeV2::ExactExhausted => Some(ExhaustionProofV1::ProbeExhausted {
            fetched: returned_u32,
        }),
        ExecutionOutcomeV2::LowerBound { .. }
        | ExecutionOutcomeV2::CappedUnknown { .. }
        | ExecutionOutcomeV2::InterruptedPartial { .. }
        | ExecutionOutcomeV2::Approximate { .. } => None,
    };
    let examined = match outcome {
        ExecutionOutcomeV2::ExactExhausted => ExaminedUniverseV1::Exact(u64::from(returned_u32)),
        ExecutionOutcomeV2::LowerBound { .. }
        | ExecutionOutcomeV2::CappedUnknown { .. }
        | ExecutionOutcomeV2::InterruptedPartial { .. }
        | ExecutionOutcomeV2::Approximate { .. } => ExaminedUniverseV1::AtLeast(universe_u64),
    };
    let empty_provenance = if returned_u32 == 0 {
        let any_executed = lanes.iter().any(LaneTraceV1::executed);
        if any_executed {
            Some(EmptyProvenanceV2::ZeroHitExecuted)
        } else if filtered_ran {
            Some(EmptyProvenanceV2::FilteredEmpty)
        } else {
            Some(EmptyProvenanceV2::AvailableEmpty)
        }
    } else {
        None
    };
    QueryResultWindowV2::new(
        returned_u32,
        candidate_count,
        outcome,
        CoverageV1::new(examined, exhaustion_proof, lanes),
        empty_provenance,
    )
    .map_err(|err| CoreError::InvalidContract(format!("fused result window v2: {err}")))
}

/// V2 window for the bounded semantic top-k route.
///
/// The dense lane is a top-k fetch with a probe row: exact only when the
/// probe observed the universe end (`observed <= requested`), a lower
/// bound when the probe saw a continuation row, and `CappedUnknown` under
/// the internal fetch ceiling when the page filled without either proof.
/// The lane trace records the model profile so a zero-hit page still
/// carries its executed backend and cost class.
pub(super) fn semantic_window_v2(
    top_k: u32,
    observed: usize,
    dense_lane: &DenseLaneContractV1,
) -> Result<QueryResultWindowV2, CoreError> {
    let requested = usize::try_from(top_k)
        .map_err(|err| CoreError::InvalidContract(format!("query top_k overflow: {err}")))?;
    let returned = observed.min(requested);
    let continuation = observed > requested;
    let returned_u32 = u32::try_from(returned)
        .map_err(|err| CoreError::InvalidContract(format!("page rows exceed u32: {err}")))?;
    let observed_u64 = u64::try_from(observed)
        .map_err(|err| CoreError::InvalidContract(format!("page rows exceed u64: {err}")))?;
    let lane = LaneTraceV1::new("semantic.dense", true, returned > 0)
        .with_candidates(CandidateCountV1::AtLeast(observed_u64))
        .with_profile(dense_lane.trace_detail());
    let (outcome, proof) = if continuation {
        (ExecutionOutcomeV2::LowerBound { continuation: true }, None)
    } else {
        (
            ExecutionOutcomeV2::ExactExhausted,
            Some(ExhaustionProofV1::ProbeExhausted {
                fetched: returned_u32,
            }),
        )
    };
    let candidate_count = match outcome {
        ExecutionOutcomeV2::ExactExhausted => CandidateCountV1::Exact(u64::from(returned_u32)),
        ExecutionOutcomeV2::LowerBound { .. } | ExecutionOutcomeV2::CappedUnknown { .. } => {
            CandidateCountV1::AtLeast(observed_u64)
        }
        ExecutionOutcomeV2::InterruptedPartial { .. } | ExecutionOutcomeV2::Approximate { .. } => {
            return Err(CoreError::InvalidContract(
                "semantic window v2: unreachable outcome for a probe lane".to_string(),
            ));
        }
    };
    let empty_provenance = (returned_u32 == 0).then_some(EmptyProvenanceV2::ZeroHitExecuted);
    QueryResultWindowV2::new(
        returned_u32,
        candidate_count,
        outcome,
        CoverageV1::new(
            if continuation {
                ExaminedUniverseV1::AtLeast(observed_u64)
            } else {
                ExaminedUniverseV1::Exact(u64::from(returned_u32))
            },
            proof,
            vec![lane],
        ),
        empty_provenance,
    )
    .map_err(|err| CoreError::InvalidContract(format!("semantic result window v2: {err}")))
}
