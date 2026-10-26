use super::*;
use quanta_index_contract::{LqCase, LqCountBound, LqExpr, LqLeaf, LqQuery, LqSpan};

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
    let (mut fixture, second) = two_owner_phrase_fixture()?;
    let plan = keyword_plan(Some(LqCountBound::Bounded(1)))?;
    let second_occurrence = fixture
        .maps
        .lookup_object(fixture.posting_root, &owner_key(second), &mut budget()?)?
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
