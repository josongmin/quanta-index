use super::super::super::text_query::{Bm25Policy, NativeTextRanking, TextContentExecution};
use super::*;
use quanta_index_contract::{
    LqCase, LqCountBound, LqExpr, LqLeaf, LqQuery, LqSpan, parse_native_content_query,
};
use quanta_index_core::index::FiniteScore;

fn ranked_page(
    fixture: &VisitorFixture,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    after: Option<(FiniteScore, OwnerKey)>,
) -> Result<NativeTextRanking, IndexError> {
    ranked_page_with_plan(fixture, &keyword_plan(None)?, constraints, after)
}

fn ranked_page_with_plan(
    fixture: &VisitorFixture,
    plan: &crate::plan::LexicalPlan,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    after: Option<(FiniteScore, OwnerKey)>,
) -> Result<NativeTextRanking, IndexError> {
    NativeTextIndex::new(fixture.recipe).search_content_with_overlay(
        &fixture.member,
        plan,
        constraints,
        TextContentExecution {
            top_k: 1,
            policy: Bm25Policy { k1: 1.2, b: 0.75 },
            rank_after: after,
        },
        None,
        TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut budget()?,
        },
    )
}

fn keyword_plan(count: Option<LqCountBound>) -> Result<crate::plan::LexicalPlan, IndexError> {
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = LqExpr::Leaf(LqLeaf::Keyword("one".into()));
    query.options.case = Some(LqCase::Sensitive);
    query.options.count = count;
    crate::planner::LexicalPlanner::plan(&query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))
}

#[test]
fn bounded_count_uses_real_postings_and_keeps_full_match_total() -> Result<(), IndexError> {
    let (fixture, second) = two_owner_phrase_fixture()?;
    let plan = keyword_plan(None)?;
    let complete = search_phrase_plan(&fixture, &plan, 2)?;
    if complete.matched != 2
        || complete.hits.len() != 2
        || !complete.hits.iter().any(|hit| hit.owner == fixture.owner)
        || !complete.hits.iter().any(|hit| hit.owner == second)
    {
        return Err(IndexError::BackendViolation(
            "two-owner posting fixture lost independent complete gold".into(),
        ));
    }
    let plan = keyword_plan(Some(LqCountBound::Bounded(1)))?;
    let bounded = search_phrase_plan(&fixture, &plan, 2)?;
    if bounded.matched != 2
        || bounded.hits.len() != 1
        || bounded.hits.first() != complete.hits.first()
    {
        return Err(IndexError::BackendViolation(
            "count:1 changed the posting universe or top-ranked hit".into(),
        ));
    }
    Ok(())
}

#[test]
fn bounded_count_discards_provisional_prefix_on_late_posting_corruption() -> Result<(), IndexError>
{
    let (mut fixture, _second) = two_owner_phrase_fixture()?;
    let plan = keyword_plan(Some(LqCountBound::Bounded(1)))?;
    let second_occurrence = fixture
        .maps
        .lookup_object(
            MapRoot(Some(reference(72))),
            &stored_term_key("one", CaseMode::Sensitive)?,
            &mut budget()?,
        )?
        .ok_or_else(|| IndexError::BackendViolation("second posting absent".into()))?;
    fixture.objects.counted_occurrence = Some(second_occurrence);
    fixture
        .objects
        .add(second_occurrence, b"{".to_vec(), Vec::new())?;
    let observed = search_phrase_plan(&fixture, &plan, 2);
    if !matches!(observed, Err(IndexError::CorruptObject(_))) {
        return Err(IndexError::BackendViolation(
            "count:1 returned a prefix despite late posting corruption".into(),
        ));
    }
    if fixture.objects.occurrence_reads.load(Ordering::Relaxed) == 0 {
        return Err(IndexError::BackendViolation(
            "late posting corruption fixture was not read".into(),
        ));
    }
    Ok(())
}

#[test]
fn rank_after_pages_keep_full_match_total_and_authoritative_remaining() -> Result<(), IndexError> {
    let (fixture, second) = two_owner_phrase_fixture()?;
    let constraints = quanta_index_contract::QueryConstraintSetV1::unconstrained();
    // Independent body gold: first has three `one` occurrences in six tokens,
    // second has one in two. Corpus N=2, df=2, avgdl=4; BM25 ranks first above
    // second for this single keyword, unlike the two-token phrase query.
    let first = ranked_page(&fixture, &constraints, None)?;
    if first.matched != 2
        || first.remaining != 2
        || first.hits.len() != 1
        || first
            .hits
            .first()
            .is_none_or(|hit| hit.owner != fixture.owner)
    {
        return Err(IndexError::BackendViolation(
            "first keyword page differs from fixed owner gold".into(),
        ));
    }
    let hit = first.hits.first().ok_or(IndexError::Conflict)?;
    let next = ranked_page(
        &fixture,
        &constraints,
        Some((
            FiniteScore::new(hit.score)
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?,
            hit.owner,
        )),
    )?;
    if next.matched != 2
        || next.remaining != 1
        || next.hits.len() != 1
        || next.hits.first().is_none_or(|hit| hit.owner != second)
    {
        return Err(IndexError::BackendViolation(
            "continuation duplicated or skipped a fixed owner".into(),
        ));
    }
    let hit = next.hits.first().ok_or(IndexError::Conflict)?;
    let terminal = ranked_page(
        &fixture,
        &constraints,
        Some((
            FiniteScore::new(hit.score)
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?,
            hit.owner,
        )),
    )?;
    if terminal.matched != 2 || terminal.remaining != 0 || !terminal.hits.is_empty() {
        return Err(IndexError::BackendViolation(
            "terminal page inferred remaining from full matched total".into(),
        ));
    }
    Ok(())
}

#[test]
fn boolean_rank_after_pages_keep_only_scores_from_matched_branches() -> Result<(), IndexError> {
    let (fixture, second) = two_owner_phrase_fixture()?;
    let constraints = quanta_index_contract::QueryConstraintSetV1::unconstrained();
    // N=2, avgdl=4. The first body has three `one` and one `gap` in
    // six tokens; the second has one `one` and no `gap` in two tokens.
    let bm25 = |df: f64, tf: f64, length: f64| {
        let idf = ((2.0 - df + 0.5) / (df + 0.5)).ln_1p();
        let norm = 0.25 + 0.75 * length / 4.0;
        idf * ((1.0 + 1.2_f64.recip()) / (1.2_f64.recip() + norm / tf))
    };
    let common_first = bm25(2.0, 3.0, 6.0);
    let common_second = bm25(2.0, 1.0, 2.0);
    let first_with_gap = bm25(1.0, 1.0, 6.0) + common_first;
    let cursor = |score, owner| {
        FiniteScore::new(score)
            .map(|score| (score, owner))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))
    };
    for (raw, first_score) in [
        ("one OR gap case:yes", first_with_gap),
        (
            "(gap AND file.contains('absent')) OR one case:yes",
            common_first,
        ),
    ] {
        let parsed = parse_native_content_query(raw)
            .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let plan = crate::planner::LexicalPlanner::plan(&parsed)
            .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let first = ranked_page_with_plan(&fixture, &plan, &constraints, None)?;
        let after = cursor(first_score, fixture.owner)?;
        let next = ranked_page_with_plan(&fixture, &plan, &constraints, Some(after))?;
        for (page, expected_remaining, expected_owner, expected_score) in [
            (first, 2, fixture.owner, first_score),
            (next, 1, second, common_second),
        ] {
            let [hit] = page.hits.as_slice() else {
                return Err(IndexError::BackendViolation(format!(
                    "{raw}: expected one hit at remaining {expected_remaining}"
                )));
            };
            check_eq!((page.matched, page.remaining), (2, expected_remaining));
            check_eq!(
                (hit.owner, hit.score.to_bits()),
                (expected_owner, expected_score.to_bits())
            );
        }
        let terminal = ranked_page_with_plan(
            &fixture,
            &plan,
            &constraints,
            Some(cursor(common_second, second)?),
        )?;
        check_eq!((terminal.matched, terminal.remaining), (2, 0));
        check!(terminal.hits.is_empty());

        let forged = cursor(f64::from_bits(first_score.to_bits() + 1), fixture.owner)?;
        check!(matches!(
            ranked_page_with_plan(&fixture, &plan, &constraints, Some(forged)),
            Err(IndexError::InvalidInput(_))
        ));
    }
    Ok(())
}

#[test]
fn rank_after_filters_before_page_heap_and_still_drains_late_corruption() -> Result<(), IndexError>
{
    let (mut fixture, second) = two_owner_phrase_fixture()?;
    seed_phrase_source_paths(&mut fixture, second)?;
    let selected = exact_path_constraint("b")?;
    let first = ranked_page(&fixture, &selected, None)?;
    if first.matched != 1
        || first.remaining != 1
        || first.hits.first().is_none_or(|hit| hit.owner != second)
    {
        return Err(IndexError::BackendViolation(
            "path filter was applied after the page window".into(),
        ));
    }
    let hit = first.hits.first().ok_or(IndexError::Conflict)?;
    let after = Some((
        FiniteScore::new(hit.score)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?,
        hit.owner,
    ));
    let terminal = ranked_page(&fixture, &selected, after)?;
    if terminal.matched != 1 || terminal.remaining != 0 || !terminal.hits.is_empty() {
        return Err(IndexError::BackendViolation(
            "selected terminal page changed full matched count".into(),
        ));
    }
    let occurrence = fixture
        .maps
        .lookup_object(
            MapRoot(Some(reference(72))),
            &stored_term_key("one", CaseMode::Sensitive)?,
            &mut budget()?,
        )?
        .ok_or(IndexError::Conflict)?;
    fixture.objects.counted_occurrence = Some(occurrence);
    fixture.objects.add(occurrence, b"{".to_vec(), Vec::new())?;
    if !matches!(
        ranked_page(&fixture, &selected, after),
        Err(IndexError::CorruptObject(_))
    ) || fixture.objects.occurrence_reads.load(Ordering::Relaxed) == 0
    {
        return Err(IndexError::BackendViolation(
            "rank-after hid corruption behind the cursor".into(),
        ));
    }
    Ok(())
}

#[test]
fn rank_after_refuses_negative_score_and_count_without_posting_lookup() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let before = fixture.maps.term_lookups.load(Ordering::Relaxed);
    for (score, count) in [(-1.0, None), (0.0, Some(LqCountBound::Bounded(1)))] {
        let plan = keyword_plan(count)?;
        let after = FiniteScore::new(score)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let result = NativeTextIndex::new(fixture.recipe).search_content_with_overlay(
            &fixture.member,
            &plan,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            TextContentExecution {
                top_k: 1,
                policy: Bm25Policy { k1: 1.2, b: 0.75 },
                rank_after: Some((after, fixture.owner)),
            },
            None,
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut budget()?,
            },
        );
        if !matches!(result, Err(IndexError::InvalidInput(_)))
            || fixture.maps.term_lookups.load(Ordering::Relaxed) != before
        {
            return Err(IndexError::BackendViolation(
                "invalid rank-after input reached posting lookup".into(),
            ));
        }
    }
    Ok(())
}

#[test]
fn rank_after_rejects_forged_anchor_after_full_eligible_scan() -> Result<(), IndexError> {
    let (mut fixture, second) = two_owner_phrase_fixture()?;
    seed_phrase_source_paths(&mut fixture, second)?;
    let unconstrained = quanta_index_contract::QueryConstraintSetV1::unconstrained();
    let first = ranked_page(&fixture, &unconstrained, None)?;
    let hit = first.hits.first().ok_or(IndexError::Conflict)?;
    let score = FiniteScore::new(hit.score)
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let foreign = OwnerKey {
        producer: ProducerId([99; 16]),
        stable_id: hit.owner.stable_id,
    };
    let zero =
        FiniteScore::new(0.0).map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    for (anchor, constraints) in [
        ((zero, hit.owner), unconstrained.clone()),
        ((score, foreign), unconstrained),
        ((score, hit.owner), exact_path_constraint("b")?),
    ] {
        if !matches!(
            ranked_page(&fixture, &constraints, Some(anchor)),
            Err(IndexError::InvalidInput(_))
        ) {
            return Err(IndexError::BackendViolation(
                "forged or ineligible rank anchor returned a successful suffix".into(),
            ));
        }
    }
    Ok(())
}
