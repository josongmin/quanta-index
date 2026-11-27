//! The full ranked order a first page captures for later pages.
//!
//! The oracle is the existing page walk: repeated `rank_after` scans of the
//! same plan. The captured order must list exactly the same `(score, owner)`
//! rows, in the same order, with the exact `matched` and artifact.

use super::literal_anchor::{corpus_with_paths, large_budget};
use super::*;
use crate::native::text_query::{Bm25Policy, TextContentExecution};
use quanta_index_core::index::FiniteScore;

/// `alpha` one to three times and `beta` zero to four times per body.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "fixed small repetition counts"
)]
fn ranked_docs() -> Vec<(String, Vec<u8>)> {
    (0..12_usize)
        .map(|ordinal| {
            let body = format!(
                "{}{}",
                "alpha ".repeat(ordinal % 3 + 1),
                "beta ".repeat(ordinal % 5)
            );
            (format!("src/r{ordinal:02}.rs"), body.into_bytes())
        })
        .collect()
}

fn plan(query: &str) -> Result<crate::plan::LexicalPlan, IndexError> {
    let parsed = quanta_index_contract::parse_native_content_query(query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    crate::planner::LexicalPlanner::plan(&parsed)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))
}

const POLICY: Bm25Policy = Bm25Policy { k1: 1.2, b: 0.75 };

#[test]
fn captured_order_equals_the_rank_after_page_walk() -> Result<(), IndexError> {
    let corpus = corpus_with_paths(&ranked_docs())?;
    let fixture = &corpus.fixture;
    let index = NativeTextIndex::new(fixture.recipe);
    let plan = plan("alpha")?;
    let constraints = quanta_index_contract::QueryConstraintSetV1::unconstrained();
    let mut budget = large_budget()?;
    let first = index.search_content_capturing(
        &fixture.member,
        &plan,
        &constraints,
        3,
        POLICY,
        TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut budget,
        },
    )?;
    let order = first
        .order
        .as_ref()
        .ok_or_else(|| IndexError::BackendViolation("first page captured no order".into()))?;
    check_eq!(first.matched, 12);
    check_eq!(
        u64::try_from(order.len()).map_err(|_width| IndexError::ResourceLimited)?,
        first.matched
    );
    check_eq!(order.artifact(), first.artifact);
    check!(!order.is_path());
    // The page walk: rank_after scans until no row remains.
    let mut walked: Vec<(FiniteScore, OwnerKey)> = Vec::new();
    let mut after = None;
    loop {
        let mut budget = large_budget()?;
        let page = index.search_content_with_overlay(
            &fixture.member,
            &plan,
            &constraints,
            TextContentExecution {
                top_k: 3,
                policy: POLICY,
                rank_after: after,
            },
            None,
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut budget,
            },
        )?;
        check!(page.order.is_none());
        for hit in &page.hits {
            let score = FiniteScore::new(hit.score)
                .map_err(|_error| IndexError::BackendViolation("nonfinite score".into()))?;
            walked.push((score, hit.owner));
        }
        if u64::try_from(page.hits.len()).is_ok_and(|rows| page.remaining <= rows) {
            break;
        }
        after = walked.last().copied();
    }
    let captured: Vec<(FiniteScore, OwnerKey)> = order
        .entries()
        .iter()
        .map(|entry| (entry.score, entry.owner))
        .collect();
    check_eq!(captured, walked);
    // The first page's rows are the order's prefix.
    let prefix: Vec<OwnerKey> = first.hits.iter().map(|hit| hit.owner).collect();
    check_eq!(
        prefix,
        captured
            .iter()
            .take(3)
            .map(|(_, owner)| *owner)
            .collect::<Vec<_>>()
    );
    Ok(())
}
