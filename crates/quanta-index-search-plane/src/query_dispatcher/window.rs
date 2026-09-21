//! Top-k probing and result-window finalization (`QueryResultWindowV1`) for
//! lexical, semantic, and fused lanes.

use quanta_index_contract::{
    CandidateCountV1, LqQuery, QueryResultWindowV1, continuation_fetch_size,
};
use quanta_index_core::{
    CoreError, HybridOrchestratorPolicy, LexicalSearchPageV1, validate_query_top_k,
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
    QueryResultWindowV1::new(returned, CandidateCountV1::Exact(total), total > u64::from(returned))
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

pub(super) fn fused_window_v1(
    top_k: u32,
    returned: usize,
    observed_universe: usize,
    lane_limit_reached: bool,
) -> Result<QueryResultWindowV1, CoreError> {
    let requested = usize::try_from(top_k)
        .map_err(|err| CoreError::InvalidContract(format!("query top_k overflow: {err}")))?;
    if lane_limit_reached && returned != requested {
        return Err(CoreError::InvalidContract(
            "hybrid result window observed a capped lane before filling the requested page"
                .to_string(),
        ));
    }
    let observed = if observed_universe > requested || lane_limit_reached {
        requested.saturating_add(1)
    } else {
        returned
    };
    QueryResultWindowV1::from_probe(top_k, observed)
        .map_err(|err| CoreError::InvalidContract(format!("query result window: {err}")))
}

pub(super) fn top_k_limit(top_k: u32) -> usize {
    usize::try_from(top_k).map_or(usize::MAX, core::convert::identity)
}
