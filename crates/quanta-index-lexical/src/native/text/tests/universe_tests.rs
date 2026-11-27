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
    insert_empty_source(
        fixture,
        owner,
        path,
        [reference(id), reference(id.wrapping_add(1))],
        admitted.then(|| reference(id.wrapping_add(2))),
    )?;
    Ok(owner)
}

fn insert_empty_source(
    fixture: &mut VisitorFixture,
    owner: OwnerKey,
    path: &str,
    [body_ref, source_ref]: [ObjectRef; 2],
    contribution_ref: Option<ObjectRef>,
) -> Result<(), IndexError> {
    fixture.objects.add(body_ref, Vec::new(), Vec::new())?;
    let record = SourceRecord {
        path: path.into(),
        bytes: body_ref,
        byte_len: 0,
        text: if contribution_ref.is_some() {
            SourceTextAdmission::Utf8 {
                policy: fixture.recipe.admission_policy,
            }
        } else {
            SourceTextAdmission::Excluded
        },
        source_repo_id: None,
        language: None,
    };
    fixture
        .objects
        .add(source_ref, encode(&record)?, vec![body_ref])?;
    fixture.maps.add(
        fixture.ticket.view.source.owners,
        owner_map_key(owner).to_vec(),
        source_ref,
    )?;
    if let Some(contribution_ref) = contribution_ref {
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
    Ok(())
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
fn rank_after_zero_score_universe_keeps_strict_owner_ties_and_remaining() -> Result<(), IndexError>
{
    use super::super::super::text_query::{Bm25Policy, TextContentExecution};
    use quanta_index_core::index::FiniteScore;

    let (fixture, first_empty, second_empty, _excluded) = finite_universe()?;
    let plan = planned_phrase_query(negate(leaf("one")))?;
    let page = |score, owner| {
        NativeTextIndex::new(fixture.recipe).search_content_with_overlay(
            &fixture.member,
            &plan,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            TextContentExecution {
                top_k: 1,
                policy: Bm25Policy { k1: 1.2, b: 0.75 },
                rank_after: Some((score, owner)),
            },
            None,
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut budget()?,
            },
        )
    };
    let zero =
        FiniteScore::new(-0.0).map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    for (anchor, expected, remaining) in [
        (first_empty, Some(second_empty), 1),
        (second_empty, None, 0),
    ] {
        let ranking = page(zero, anchor)?;
        if ranking.matched != 2
            || ranking.remaining != remaining
            || ranking.hits.len() != usize::from(expected.is_some())
            || ranking.hits.first().map(|hit| hit.owner) != expected
            || ranking.hits.iter().any(|hit| hit.score != 0.0)
        {
            return Err(IndexError::BackendViolation(
                "rank-after zero-score complement violated fixed admitted-owner gold".into(),
            ));
        }
    }
    let wrong_score =
        FiniteScore::new(1.0).map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    for (score, owner) in [(wrong_score, first_empty), (zero, fixture.owner)] {
        check!(matches!(
            page(score, owner),
            Err(IndexError::InvalidInput(message))
                if message.contains("rank-after anchor is absent")
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

fn boundary_owner(producer: ProducerId, ordinal: u16) -> OwnerKey {
    let mut stable_id = [0; 16];
    for (slot, byte) in stable_id.iter_mut().zip(ordinal.to_be_bytes()) {
        *slot = byte;
    }
    OwnerKey {
        producer,
        stable_id: StableOwnerId(stable_id),
    }
}

fn boundary_reference(tag: u8, ordinal: u16) -> ObjectRef {
    let mut digest = [tag; 32];
    for (slot, byte) in digest.iter_mut().skip(1).zip(ordinal.to_be_bytes()) {
        *slot = byte;
    }
    ObjectRef(Digest(digest))
}

fn add_boundary_owner(fixture: &mut VisitorFixture, ordinal: u16) -> Result<(), IndexError> {
    insert_empty_source(
        fixture,
        boundary_owner(fixture.owner.producer, ordinal),
        &format!("boundary/{ordinal:04}.rs"),
        [reference(200), boundary_reference(201, ordinal)],
        Some(boundary_reference(202, ordinal)),
    )
}

fn boundary_fixture() -> Result<VisitorFixture, IndexError> {
    let mut fixture = phrase_fixture()?;
    fixture.maps.counted_inventory = MapRoot(Some(reference(3)));
    // Insert in reverse rank order. Each owner has its own Source record and
    // contribution. Only the original fixture contains the word "one".
    for ordinal in (0..4_095).rev() {
        add_boundary_owner(&mut fixture, ordinal)?;
    }
    phrase_fixture_root(&mut fixture, 4_096, 6)?;
    Ok(fixture)
}

fn boundary_work(reads: u64, deadline: Instant) -> WorkBudget {
    WorkBudget::new(
        WorkLimits {
            reads,
            writes: 0,
            bytes: 40_000_000,
            mutations: 0,
        },
        deadline,
    )
}

fn boundary_deadline() -> Result<Instant, IndexError> {
    Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or(IndexError::ResourceLimited)
}

#[test]
fn unanchored_rank_pages_cover_all_4096_source_rows_and_stop_at_the_cap() -> Result<(), IndexError>
{
    use super::super::super::text_query::{Bm25Policy, NativeTextHitSource, TextContentExecution};
    use quanta_index_core::index::FiniteScore;

    let mut fixture = boundary_fixture()?;
    let complement = planned_phrase_query(negate(leaf("one")))?;
    let score =
        FiniteScore::new(0.0).map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let mut after = None;
    for (start, end, remaining) in [
        (0u16, 1_024u16, 4_095),
        (1_024, 2_048, 3_071),
        (2_048, 3_072, 2_047),
        (3_072, 4_095, 1_023),
        (4_095, 4_095, 0),
    ] {
        let page = NativeTextIndex::new(fixture.recipe).search_content_with_overlay(
            &fixture.member,
            &complement,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            TextContentExecution {
                top_k: 1_024,
                policy: Bm25Policy { k1: 1.2, b: 0.75 },
                rank_after: after,
            },
            None,
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut boundary_work(100_000, boundary_deadline()?),
            },
        )?;
        check_eq!(page.matched, 4_095);
        check_eq!(page.remaining, remaining);
        check_eq!(page.hits.len(), (start..end).len());
        for (hit, ordinal) in page.hits.iter().zip(start..end) {
            check_eq!(hit.owner, boundary_owner(fixture.owner.producer, ordinal));
            check_eq!(
                hit.source,
                NativeTextHitSource::Durable(boundary_reference(201, ordinal))
            );
            check!(hit.score == 0.0);
        }
        after = page.hits.last().map(|hit| (score, hit.owner));
    }

    let regex = planned_phrase_query(LqExpr::Leaf(LqLeaf::Regex("one".into())))?;
    let contains = planned_phrase_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains".into(),
        args: vec![quanta_index_contract::LqPredicateArg::RawString(
            "one".into(),
        )],
    }))?;
    for plan in [&regex, &contains] {
        let mut work = boundary_work(100_000, boundary_deadline()?);
        let result = search_phrase_plan_with_budget(&fixture, plan, 1, &mut work)?;
        check_eq!(result.matched, 1);
        check_eq!(result.remaining, 1);
        check_eq!(result.hits.len(), 1);
        check_eq!(
            result.hits.first().map(|hit| (hit.owner, hit.source)),
            Some((
                fixture.owner,
                NativeTextHitSource::Durable(fixture.source_ref)
            ))
        );
    }

    add_boundary_owner(&mut fixture, 4_095)?;
    phrase_fixture_root(&mut fixture, 4_097, 6)?;
    fixture.maps.inventory_scans.store(0, Ordering::Relaxed);
    for plan in [&complement, &regex, &contains] {
        let mut work = boundary_work(100_000, boundary_deadline()?);
        check!(matches!(
            search_phrase_plan_with_budget(&fixture, plan, 1, &mut work),
            Err(IndexError::ResourceLimited)
        ));
        // This refusal belongs to the document census, not a spent Work axis.
        check_eq!(work.first_denial(), None);
        check_eq!(fixture.maps.inventory_scans.load(Ordering::Relaxed), 0);
    }
    let anchored = planned_phrase_query(leaf("one"))?;
    let hit = search_phrase_plan(&fixture, &anchored, 1)?;
    check_eq!(hit.matched, 1);
    check_eq!(hit.hits.first().map(|hit| hit.owner), Some(fixture.owner));
    check_eq!(fixture.maps.inventory_scans.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn unanchored_rank_never_returns_a_full_top_k_after_late_denial() -> Result<(), IndexError> {
    use quanta_index_core::index::WorkBudgetDenial;

    let mut fixture = boundary_fixture()?;
    let plan = planned_phrase_query(negate(leaf("one")))?;
    let mut expired = boundary_work(100_000, Instant::now());
    check!(matches!(
        search_phrase_plan_with_budget(&fixture, &plan, 1, &mut expired),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(expired.first_denial(), Some(WorkBudgetDenial::Deadline));
    check_eq!(fixture.maps.inventory_scans.load(Ordering::Relaxed), 0);

    let mut limited = boundary_work(450, boundary_deadline()?);
    check!(matches!(
        search_phrase_plan_with_budget(&fixture, &plan, 1, &mut limited),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(limited.first_denial(), Some(WorkBudgetDenial::Reads));
    check!(fixture.maps.inventory_scans.load(Ordering::Relaxed) > 1);

    fixture.maps.inventory_scans.store(0, Ordering::Relaxed);
    fixture.objects.cancel_after_get = Some(boundary_reference(201, 4_094));
    let mut cancelled = boundary_work(100_000, boundary_deadline()?);
    check!(matches!(
        search_phrase_plan_with_budget(&fixture, &plan, 1, &mut cancelled),
        Err(IndexError::Cancelled)
    ));
    check_eq!(cancelled.first_denial(), Some(WorkBudgetDenial::Cancelled));
    check_eq!(fixture.maps.inventory_scans.load(Ordering::Relaxed), 64);

    fixture.objects.cancel_after_get = None;
    let last_empty = boundary_owner(fixture.owner.producer, 4_094);
    fixture.maps.add(
        fixture.ticket.view.source.owners,
        owner_map_key(last_empty).to_vec(),
        boundary_reference(201, 4_093),
    )?;
    let mut work = boundary_work(100_000, boundary_deadline()?);
    check!(matches!(
        search_phrase_plan_with_budget(&fixture, &plan, 1, &mut work),
        Err(IndexError::CorruptObject(message)) if message.contains("stale against pinned source")
    ));
    check_eq!(work.first_denial(), None);
    Ok(())
}
