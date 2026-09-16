//! QI-BB-004 — a semantic query's lexical scope honors `scope_top_k`.
//!
//! The contract says the scope request's `top_k` is the lexical candidate cap:
//! the semantic lane may only rank documents that the lexical lane placed in
//! its top `scope_top_k`. The dispatcher used to ignore the value and
//! materialize the scope query's full recall, so with a broad scope the
//! semantic answer could contain documents far outside the cap — and the
//! allowlist grew with the corpus.
//!
//! Oracle: the lexical route itself, asked for the same query at
//! `top_k = scope_top_k` on the same pinned generation. Whatever it ranks in
//! its top `scope_top_k` is the only set the scoped semantic answer may draw
//! from. The two routes share the lexical adapter but nothing else in the
//! path under test, so agreement is an observable property, not a mirror.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;

use quanta_index_contract::{
    PUBLIC_TOP_K_MAX, PlannerStage, TOP_K_OUT_OF_RANGE_CODE, TextQuerySyntax,
};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::{E2eQueryResult, E2eRuntime};

type TestResult = Result<(), Box<dyn Error>>;

const MATCHING_FILES: u32 = 100;
const OUTER_TOP_K: u32 = 10;
const NARROW_SCOPE: u32 = 2;
const WIDE_SCOPE: u32 = MATCHING_FILES;
const QUERY: &str = "needle";

/// One hundred files that all match the scope query with different lexical
/// weight, so the lexical top-2 is a strict, non-trivial subset.
fn seeded_runtime() -> Result<E2eRuntime, Box<dyn Error>> {
    let mut rt = E2eRuntime::boot()?;
    for index in 0..MATCHING_FILES {
        // Vary term frequency and length so BM25 has something to rank on.
        let repeats = usize::try_from(index % 7)?.saturating_add(1);
        let needles = vec!["needle"; repeats].join(" ");
        let filler = "filler ".repeat(usize::try_from(index % 5)?.saturating_add(1));
        let content = format!("fn item_{index}() {{ {needles} {filler}}}");
        rt.ingest_text("repo", &format!("src/item_{index:03}.rs"), &content)?;
    }
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn served(result: &E2eQueryResult, what: &str) -> Result<(), Box<dyn Error>> {
    result.typed_error.as_ref().map_or_else(
        || Ok(()),
        |error| Err(format!("{what} was refused: {error}").into()),
    )
}

fn trace_details(result: &E2eQueryResult) -> Vec<(PlannerStage, String)> {
    result
        .explanation
        .as_ref()
        .map_or_else(Vec::new, |explanation| {
            explanation
                .planner_trace
                .iter()
                .map(|entry| (entry.stage, entry.detail.clone()))
                .collect()
        })
}

/// With `scope_top_k = 2` and `top_k = 10`, the semantic answer never leaves
/// the lexical top-2, and the explanation names both cap and candidate count.
#[test]
fn scoped_semantic_results_stay_inside_the_lexical_top_scope() -> TestResult {
    let mut rt = seeded_runtime()?;

    let lexical_top = rt.query_text(TextQuerySyntax::Native, QUERY, NARROW_SCOPE);
    served(&lexical_top, "lexical oracle")?;
    let allowed: BTreeSet<String> = lexical_top.candidate_ids.iter().cloned().collect();
    if allowed.len() != usize::try_from(NARROW_SCOPE)? {
        return Err(format!(
            "lexical oracle returned {} ids for top_k={NARROW_SCOPE}; fixture is not load-bearing",
            allowed.len()
        )
        .into());
    }

    let scoped = rt.query_semantic(
        QUERY,
        OUTER_TOP_K,
        Some((TextQuerySyntax::Native, QUERY, NARROW_SCOPE)),
    );
    served(&scoped, "scoped semantic")?;
    if scoped.candidate_ids.is_empty() {
        return Err("scoped semantic returned nothing; fixture proved nothing".into());
    }
    let escaped: Vec<&String> = scoped
        .candidate_ids
        .iter()
        .filter(|id| !allowed.contains(*id))
        .collect();
    if !escaped.is_empty() {
        return Err(format!(
            "semantic results left the lexical top-{NARROW_SCOPE}: escaped={escaped:?} allowed={allowed:?}"
        )
        .into());
    }
    if scoped.candidate_ids.len() > usize::try_from(NARROW_SCOPE)? {
        return Err(format!(
            "scoped semantic returned {} rows from a scope of {NARROW_SCOPE}",
            scoped.candidate_ids.len()
        )
        .into());
    }

    let trace = trace_details(&scoped);
    let has_cap = trace.iter().any(|(stage, detail)| {
        *stage == PlannerStage::Plan && detail == &format!("semantic.scope.cap={NARROW_SCOPE}")
    });
    let has_count = trace.iter().any(|(stage, detail)| {
        *stage == PlannerStage::ExecFanout
            && detail == &format!("semantic.scope.text_candidates={NARROW_SCOPE}")
    });
    if !has_cap || !has_count {
        return Err(format!(
            "explanation does not name the scope cap and candidate count: {trace:?}"
        )
        .into());
    }
    Ok(())
}

/// The cap is what narrows the answer: the same query with a scope wide
/// enough to admit every match fills the outer `top_k`.
#[test]
fn a_wide_scope_fills_the_outer_top_k() -> TestResult {
    let mut rt = seeded_runtime()?;
    let wide = rt.query_semantic(
        QUERY,
        OUTER_TOP_K,
        Some((TextQuerySyntax::Native, QUERY, WIDE_SCOPE)),
    );
    served(&wide, "wide-scope semantic")?;
    if wide.candidate_ids.len() != usize::try_from(OUTER_TOP_K)? {
        return Err(format!(
            "wide scope of {WIDE_SCOPE} over {MATCHING_FILES} matches returned {} rows, expected {OUTER_TOP_K}",
            wide.candidate_ids.len()
        )
        .into());
    }
    Ok(())
}

/// `scope_top_k` is a public `top_k` and shares the one gate: zero and
/// above-maximum are refused under the shared code, the maximum is accepted.
#[test]
fn scope_top_k_shares_the_public_gate() -> TestResult {
    let mut rt = seeded_runtime()?;
    for refused in [0, PUBLIC_TOP_K_MAX + 1, u32::MAX] {
        let result = rt.query_semantic(
            QUERY,
            OUTER_TOP_K,
            Some((TextQuerySyntax::Native, QUERY, refused)),
        );
        match result.typed_error {
            Some(error) if error.code == TOP_K_OUT_OF_RANGE_CODE => {}
            Some(error) => {
                return Err(format!(
                    "scope_top_k={refused} refused under `{}` instead of `{TOP_K_OUT_OF_RANGE_CODE}`",
                    error.code
                )
                .into());
            }
            None => {
                return Err(format!(
                    "scope_top_k={refused} was accepted and returned {} rows",
                    result.candidate_ids.len()
                )
                .into());
            }
        }
    }
    let at_max = rt.query_semantic(
        QUERY,
        OUTER_TOP_K,
        Some((TextQuerySyntax::Native, QUERY, PUBLIC_TOP_K_MAX)),
    );
    served(&at_max, "scope_top_k at the public maximum")?;
    Ok(())
}
