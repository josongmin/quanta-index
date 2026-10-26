use super::*;
use quanta_index_contract::{LqCase, LqCountBound, LqExpr, LqLeaf, LqQuery, LqSpan};

fn leaf(term: &str) -> LqExpr {
    LqExpr::Leaf(LqLeaf::Keyword(term.into()))
}

fn negate(expr: LqExpr) -> LqExpr {
    LqExpr::Not(Box::new(expr))
}

fn search(
    fixture: &VisitorFixture,
    expr: LqExpr,
    top_k: u32,
    count: Option<LqCountBound>,
) -> Result<super::super::super::text_query::NativeTextRanking, IndexError> {
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = expr;
    query.options.case = Some(LqCase::Sensitive);
    query.options.count = count;
    let plan = crate::planner::LexicalPlanner::plan(&query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    search_phrase_plan(fixture, &plan, top_k)
}

fn add_source(
    fixture: &mut VisitorFixture,
    id: u8,
    path: &str,
    admitted: bool,
) -> Result<OwnerKey, IndexError> {
    let owner = OwnerKey {
        producer: fixture.owner.producer,
        stable_id: StableOwnerId([id; 16]),
    };
    let body_ref = reference(id);
    let source_ref = reference(id.wrapping_add(1));
    let contribution_ref = reference(id.wrapping_add(2));
    fixture.objects.add(body_ref, Vec::new(), Vec::new())?;
    let record = SourceRecord {
        path: path.into(),
        bytes: body_ref,
        byte_len: 0,
        text: if admitted {
            SourceTextAdmission::Utf8 {
                policy: fixture.recipe.admission_policy,
            }
        } else {
            SourceTextAdmission::Excluded
        },
    };
    fixture
        .objects
        .add(source_ref, encode(&record)?, vec![body_ref])?;
    fixture.maps.add(
        fixture.ticket.view.source.owners,
        owner_map_key(owner).to_vec(),
        source_ref,
    )?;
    if admitted {
        let contribution = Contribution {
            record: source_ref,
            inventory: MapRoot(None).into(),
            indexed_tokens: 0,
        };
        fixture.objects.add(
            contribution_ref,
            encode(&contribution)?,
            contribution_children(&contribution)?.as_slice().to_vec(),
        )?;
        fixture.maps.add(
            MapRoot(Some(reference(3))),
            owner_key(owner).to_vec(),
            contribution_ref,
        )?;
    }
    Ok(owner)
}

fn finite_universe() -> Result<(VisitorFixture, OwnerKey, OwnerKey, OwnerKey), IndexError> {
    let mut fixture = visitor_fixture()?;
    let first_empty = add_source(&mut fixture, 91, "b", true)?;
    let second_empty = add_source(&mut fixture, 101, "c", true)?;
    let excluded = add_source(&mut fixture, 111, "excluded", false)?;
    let paths = MapRoot(Some(reference(140)));
    for (owner, path) in [
        (fixture.owner, b"path\0a".as_slice()),
        (first_empty, b"path\0b".as_slice()),
        (second_empty, b"path\0c".as_slice()),
        (excluded, b"path\0excluded".as_slice()),
    ] {
        fixture.maps.add(
            paths,
            path.to_vec(),
            quanta_index_core::index::path_owner_map_value(owner)?,
        )?;
    }
    fixture.ticket.view.source.paths = paths;
    phrase_fixture_root(&mut fixture, 3, 2)?;
    Ok((fixture, first_empty, second_empty, excluded))
}

#[test]
fn unanchored_not_and_or_not_include_only_admitted_docs_with_fixed_scores() -> Result<(), IndexError>
{
    let (fixture, first_empty, second_empty, excluded) = finite_universe()?;
    let negated = search(&fixture, negate(leaf("one")), 3, None)?;
    if negated.matched != 2
        || negated.hits.len() != 2
        || negated
            .hits
            .first()
            .is_none_or(|hit| hit.owner != first_empty || hit.score != 0.0)
        || negated
            .hits
            .get(1)
            .is_none_or(|hit| hit.owner != second_empty || hit.score != 0.0)
        || negated.hits.iter().any(|hit| hit.owner == excluded)
    {
        return Err(IndexError::BackendViolation(
            "NOT failed admitted empty-doc or zero-score tie gold".into(),
        ));
    }

    let disjunction = search(
        &fixture,
        LqExpr::Any(vec![leaf("one"), negate(leaf("one"))]),
        3,
        None,
    )?;
    let first = disjunction
        .hits
        .first()
        .ok_or_else(|| IndexError::BackendViolation("OR NOT lost its positive hit".into()))?;
    // Fixed three-document BM25 oracle: tf=2, dl=2, df=1, total tokens=2.
    if disjunction.matched != 3
        || disjunction.hits.len() != 3
        || first.owner != fixture.owner
        || (first.score - 0.863_129_742_64).abs() > 1e-10
        || disjunction
            .hits
            .get(1)
            .is_none_or(|hit| hit.owner != first_empty || hit.score != 0.0)
        || disjunction
            .hits
            .get(2)
            .is_none_or(|hit| hit.owner != second_empty || hit.score != 0.0)
    {
        return Err(IndexError::BackendViolation(
            "OR NOT changed full-corpus match, BM25, or zero tie gold".into(),
        ));
    }

    let double = search(&fixture, negate(negate(leaf("one"))), 3, None)?;
    if double.matched != 1
        || double
            .hits
            .first()
            .is_none_or(|hit| hit.owner != fixture.owner || hit.score != 0.0)
    {
        return Err(IndexError::BackendViolation(
            "double NOT changed filter-only zero-score contract".into(),
        ));
    }
    Ok(())
}

#[test]
fn unanchored_count_and_path_filter_keep_full_matched_count() -> Result<(), IndexError> {
    let (fixture, first_empty, _second_empty, _excluded) = finite_universe()?;
    let limited = search(
        &fixture,
        LqExpr::Any(vec![leaf("one"), negate(leaf("one"))]),
        3,
        Some(LqCountBound::Bounded(1)),
    )?;
    if limited.matched != 3
        || limited.hits.len() != 1
        || limited
            .hits
            .first()
            .is_none_or(|hit| hit.owner != fixture.owner)
    {
        return Err(IndexError::BackendViolation(
            "count truncated the match universe instead of returned rows".into(),
        ));
    }
    let plan = planned_phrase_query(negate(leaf("one")))?;
    let constrained =
        search_phrase_with_constraints(&fixture, &plan, &exact_path_constraint("b")?, 1)?;
    // The exact-path owner value must be present in the pinned source path map.
    if constrained.matched != 1
        || constrained
            .hits
            .first()
            .is_none_or(|hit| hit.owner != first_empty)
    {
        return Err(IndexError::BackendViolation(
            "exact path did not filter before top-k".into(),
        ));
    }
    Ok(())
}

#[test]
fn unanchored_universe_refuses_oversize_before_scan_and_late_corruption() -> Result<(), IndexError>
{
    let (mut fixture, first_empty, _second_empty, _excluded) = finite_universe()?;
    fixture.maps.counted_inventory = MapRoot(Some(reference(3)));
    fixture.maps.malformed_page = Some(MapPage::new(
        vec![
            (
                owner_key(fixture.owner).to_vec(),
                fixture.occurrence_ref.into()
            );
            65
        ],
        false,
        Vec::new(),
        None,
    ));
    phrase_fixture_root(&mut fixture, 4_097, 2)?;
    let oversized = search(&fixture, negate(leaf("one")), 1, None);
    if !matches!(oversized, Err(IndexError::ResourceLimited))
        || fixture.maps.inventory_scans.load(Ordering::Relaxed) != 0
    {
        return Err(IndexError::BackendViolation(
            "oversize universe scanned before admission".into(),
        ));
    }
    fixture.maps.malformed_page = None;

    phrase_fixture_root(&mut fixture, 3, 3)?;
    if !matches!(
        search(&fixture, negate(leaf("one")), 3, None),
        Err(IndexError::CorruptObject(_))
    ) {
        return Err(IndexError::BackendViolation(
            "indexed-token aggregate mismatch returned a successful result".into(),
        ));
    }

    phrase_fixture_root(&mut fixture, 2, 2)?;
    if !matches!(
        search(&fixture, negate(leaf("one")), 3, None),
        Err(IndexError::CorruptObject(_))
    ) {
        return Err(IndexError::BackendViolation(
            "owner cardinality mismatch returned a successful prefix".into(),
        ));
    }
    phrase_fixture_root(&mut fixture, 3, 2)?;
    fixture.maps.add(
        fixture.ticket.view.source.owners,
        owner_map_key(first_empty).to_vec(),
        reference(210),
    )?;
    if !matches!(
        search(&fixture, negate(leaf("one")), 3, None),
        Err(IndexError::CorruptObject(_))
    ) {
        return Err(IndexError::BackendViolation(
            "stale admitted owner escaped pinned-source validation".into(),
        ));
    }
    fixture.maps.add(
        fixture.ticket.view.source.owners,
        owner_map_key(first_empty).to_vec(),
        reference(92),
    )?;
    let forged = Posting {
        root: fixture.posting_root.into(),
        df: 2,
    };
    fixture.objects.add(
        fixture.posting_ref,
        encode(&forged)?,
        posting_children(&forged)?.as_slice().to_vec(),
    )?;
    if !matches!(
        search(&fixture, negate(leaf("one")), 3, None),
        Err(IndexError::CorruptObject(_))
    ) {
        return Err(IndexError::BackendViolation(
            "late posting frequency mismatch returned a successful prefix".into(),
        ));
    }
    Ok(())
}
