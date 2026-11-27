use super::super::super::text_overlay::{TextOverlayCorpus, TextOverlayDocument, TextOverlayInput};
use super::super::super::text_query::{
    Bm25Policy, NativeTextHitSource, NativeTextRanking, TextContentExecution, TextOverlaySource,
};
use super::super::super::text_reader::{
    NativeTextReader, PlannedTextRead, TextOverlayReadDocument, TextReadResult,
};
use super::*;
use quanta_index_contract::{LqCase, LqCountBound, LqExpr, LqLeaf, LqQuery, LqSpan, RepoId};
use quanta_index_core::index::FiniteScore;
use quanta_index_core::index::{IndexQueryPort, QueryReadContext, effort::ResourceNeed};

fn fixture_and_corpus() -> Result<
    (
        VisitorFixture,
        OwnerKey,
        OwnerKey,
        OwnerKey,
        OwnerKey,
        TextOverlayCorpus,
    ),
    IndexError,
> {
    let (mut fixture, deleted) = two_owner_phrase_fixture()?;
    seed_phrase_source_paths(&mut fixture, deleted)?;
    let owner = |id| OwnerKey {
        producer: fixture.owner.producer,
        stable_id: StableOwnerId([id; 16]),
    };
    let added = owner(32);
    let empty_a = owner(33);
    let empty_b = owner(34);
    let inputs = [
        TextOverlayInput {
            owner: fixture.owner,
            revision: 7,
            text: Some("one three"),
        },
        TextOverlayInput {
            owner: deleted,
            revision: 8,
            text: None,
        },
        TextOverlayInput {
            owner: added,
            revision: 9,
            text: Some("one one"),
        },
        TextOverlayInput {
            owner: empty_a,
            revision: 10,
            text: Some(""),
        },
        TextOverlayInput {
            owner: empty_b,
            revision: 11,
            text: Some(""),
        },
    ];
    let index = NativeTextIndex::new(fixture.recipe);
    let mut work = budget()?;
    let corpus = index.prepare_overlay_corpus(
        &fixture.member,
        &inputs,
        &mut TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )?;
    Ok((fixture, deleted, added, empty_a, empty_b, corpus))
}

fn overlay_sources(
    fixture: &VisitorFixture,
    added: OwnerKey,
    empty_a: OwnerKey,
    empty_b: OwnerKey,
) -> [TextOverlaySource<'static>; 4] {
    [
        TextOverlaySource {
            owner: fixture.owner,
            revision: 7,
            path: "a",
            body: "one three",
            repository: None,
            language: None,
        },
        TextOverlaySource {
            owner: added,
            revision: 9,
            path: "c",
            body: "one one",
            repository: None,
            language: None,
        },
        TextOverlaySource {
            owner: empty_a,
            revision: 10,
            path: "d",
            body: "",
            repository: None,
            language: None,
        },
        TextOverlaySource {
            owner: empty_b,
            revision: 11,
            path: "e",
            body: "",
            repository: None,
            language: None,
        },
    ]
}

fn plan(expr: LqExpr, count: Option<LqCountBound>) -> Result<crate::plan::LexicalPlan, IndexError> {
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = expr;
    query.options.case = Some(LqCase::Sensitive);
    query.options.count = count;
    crate::planner::LexicalPlanner::plan(&query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))
}

fn keyword(term: &str) -> LqExpr {
    LqExpr::Leaf(LqLeaf::Keyword(term.into()))
}

fn search(
    fixture: &VisitorFixture,
    corpus: &TextOverlayCorpus,
    sources: &[TextOverlaySource<'_>],
    expr: LqExpr,
    count: Option<LqCountBound>,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
) -> Result<NativeTextRanking, IndexError> {
    let plan = plan(expr, count)?;
    let mut work = budget()?;
    NativeTextIndex::new(fixture.recipe).search_content_with_overlay(
        &fixture.member,
        &plan,
        constraints,
        TextContentExecution {
            top_k: 5,
            policy: Bm25Policy { k1: 1.2, b: 0.75 },
            rank_after: None,
        },
        Some((corpus, sources)),
        TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )
}

fn search_durable(fixture: &VisitorFixture, expr: LqExpr) -> Result<NativeTextRanking, IndexError> {
    let plan = plan(expr, None)?;
    let mut work = budget()?;
    NativeTextIndex::new(fixture.recipe).search_content(
        &fixture.member,
        &plan,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        5,
        Bm25Policy { k1: 1.2, b: 0.75 },
        TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )
}

fn exact_path(path: &str) -> Result<quanta_index_contract::QueryConstraintSetV1, IndexError> {
    let path = quanta_index_contract::ExactRepoRelativePathV1::new(path)
        .map_err(|message| IndexError::InvalidInput(message.into()))?;
    Ok(quanta_index_contract::QueryConstraintSetV1::from_exact_repo_relative_path(path))
}

#[test]
fn overlay_file_contains_reads_replacement_bytes_and_masks_deleted_owners() -> Result<(), IndexError>
{
    let (fixture, _, added, empty_a, empty_b, corpus) = fixture_and_corpus()?;
    let mut sources = overlay_sources(&fixture, added, empty_a, empty_b);
    let constraints = quanta_index_contract::QueryConstraintSetV1::unconstrained();
    let contains = |needle: &str| {
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "file.contains".into(),
            args: vec![quanta_index_contract::LqPredicateArg::RawString(
                needle.into(),
            )],
        })
    };
    // Both durable owners contained `two`; replacement and deletion remove it.
    assert_eq!(
        search(
            &fixture,
            &corpus,
            &sources,
            contains("two"),
            None,
            &constraints
        )?
        .matched,
        0
    );
    let replacement = search(
        &fixture,
        &corpus,
        &sources,
        contains("ne th"),
        None,
        &constraints,
    )?;
    assert_eq!(replacement.matched, 1);
    assert_eq!(replacement.hits[0].owner, fixture.owner);
    assert_eq!(
        replacement.hits[0].source,
        NativeTextHitSource::Overlay { revision: 7 }
    );
    let selected = search(
        &fixture,
        &corpus,
        &sources,
        contains("one"),
        None,
        &exact_path("c")?,
    )?;
    assert_eq!(selected.matched, 1);
    assert_eq!(selected.hits[0].owner, added);
    assert_eq!(
        selected.hits[0].source,
        NativeTextHitSource::Overlay { revision: 9 }
    );
    sources[0].body = "changed after overlay admission";
    assert!(matches!(
        search(
            &fixture,
            &corpus,
            &sources,
            contains("changed"),
            None,
            &constraints
        ),
        Err(IndexError::Conflict)
    ));
    Ok(())
}

#[test]
fn replacement_delete_new_and_empty_adjust_the_selected_virtual_corpus() -> Result<(), IndexError> {
    let (fixture, deleted, added, empty_a, empty_b, corpus) = fixture_and_corpus()?;
    if corpus.statistics()
        != (TextStatistics {
            docs: 4,
            indexed_tokens: 4,
        })
        || !corpus.masks(deleted)
        || corpus.document(deleted).is_some()
        || corpus.document(empty_a)
            != Some(TextOverlayDocument {
                owner: empty_a,
                revision: 10,
                indexed_tokens: 0,
            })
    {
        return Err(IndexError::BackendViolation(
            "virtual corpus census differs from fixed document gold".into(),
        ));
    }
    let one = stored_term_key("one", CaseMode::Sensitive)?;
    let two = stored_term_key("two", CaseMode::Sensitive)?;
    let three = stored_term_key("three", CaseMode::Sensitive)?;
    let mut work = budget()?;
    let mut context = TextReadContext {
        ticket: &fixture.ticket,
        objects: &fixture.objects,
        maps: &fixture.maps,
        budget: &mut work,
    };
    if corpus.adjusted_df(&fixture.member, &one, 2, &mut context)? != 2
        || corpus.adjusted_df(&fixture.member, &two, 2, &mut context)? != 0
        || corpus.adjusted_df(&fixture.member, &three, 1, &mut context)? != 1
        || corpus
            .term_hit(added, &one)
            .is_none_or(|hit| hit.positions != [0, 1])
        || corpus
            .term_hit(fixture.owner, &three)
            .is_none_or(|hit| hit.positions != [1])
        || corpus.term_hit(empty_b, &one).is_some()
    {
        return Err(IndexError::BackendViolation(
            "virtual term frequencies or positions differ from fixed gold".into(),
        ));
    }
    Ok(())
}

#[test]
fn overlay_boolean_phrase_count_and_path_use_one_virtual_ranking() -> Result<(), IndexError> {
    let (fixture, deleted, added, empty_a, empty_b, corpus) = fixture_and_corpus()?;
    let sources = overlay_sources(&fixture, added, empty_a, empty_b);
    let unconstrained = quanta_index_contract::QueryConstraintSetV1::unconstrained();
    let one = search(
        &fixture,
        &corpus,
        &sources,
        keyword("one"),
        None,
        &unconstrained,
    )?;
    if one.matched != 2
        || one.hits.len() != 2
        || one.hits.first().is_none_or(|hit| {
            hit.owner != added
                || hit.source != (NativeTextHitSource::Overlay { revision: 9 })
                || (hit.score - 0.743_865_266_942_380_4).abs() > 1e-10
        })
        || one.hits.get(1).is_none_or(|hit| {
            hit.owner != fixture.owner
                || hit.source != (NativeTextHitSource::Overlay { revision: 7 })
                || (hit.score - 0.491_910_902_332_864_4).abs() > 1e-10
        })
        || one.hits.iter().any(|hit| hit.owner == deleted)
    {
        return Err(IndexError::BackendViolation(
            "overlay BM25 order or fixed independent score gold changed".into(),
        ));
    }
    let bounded = search(
        &fixture,
        &corpus,
        &sources,
        keyword("one"),
        Some(LqCountBound::Bounded(1)),
        &unconstrained,
    )?;
    if bounded.matched != 2 || bounded.hits.len() != 1 || bounded.hits.first() != one.hits.first() {
        return Err(IndexError::BackendViolation(
            "count truncated virtual match census".into(),
        ));
    }
    let two = search(
        &fixture,
        &corpus,
        &sources,
        keyword("two"),
        None,
        &unconstrained,
    )?;
    if two.matched != 0 || !two.hits.is_empty() {
        return Err(IndexError::BackendViolation(
            "masked durable term leaked into virtual result".into(),
        ));
    }
    let phrase = search(
        &fixture,
        &corpus,
        &sources,
        LqExpr::Leaf(LqLeaf::Phrase("one three".into())),
        None,
        &unconstrained,
    )?;
    if phrase.matched != 1
        || phrase
            .hits
            .first()
            .is_none_or(|hit| hit.owner != fixture.owner)
    {
        return Err(IndexError::BackendViolation(
            "overlay phrase positions lost adjacency gold".into(),
        ));
    }
    let complement = search(
        &fixture,
        &corpus,
        &sources,
        LqExpr::Any(vec![keyword("one"), LqExpr::Not(Box::new(keyword("one")))]),
        None,
        &unconstrained,
    )?;
    if complement.matched != 4
        || complement.hits.len() != 4
        || complement
            .hits
            .get(2)
            .is_none_or(|hit| hit.owner != empty_a || hit.score != 0.0)
        || complement
            .hits
            .get(3)
            .is_none_or(|hit| hit.owner != empty_b || hit.score != 0.0)
    {
        return Err(IndexError::BackendViolation(
            "OR NOT omitted empty docs or changed zero-score owner tie".into(),
        ));
    }
    let selected = search(
        &fixture,
        &corpus,
        &sources,
        keyword("one"),
        None,
        &exact_path("c")?,
    )?;
    if selected.matched != 1 || selected.hits.first().is_none_or(|hit| hit.owner != added) {
        return Err(IndexError::BackendViolation(
            "new overlay path failed exact selection".into(),
        ));
    }
    let removed = search(
        &fixture,
        &corpus,
        &sources,
        keyword("one"),
        None,
        &exact_path("b")?,
    )?;
    if removed.matched != 0 || !removed.hits.is_empty() {
        return Err(IndexError::BackendViolation(
            "deleted durable path leaked into overlay selection".into(),
        ));
    }
    Ok(())
}

#[test]
fn content_keyword_punctuation_uses_canonical_phrase_positions_in_durable_and_overlay_corpus()
-> Result<(), IndexError> {
    let (fixture, deleted, added, empty_a, empty_b, corpus) = fixture_and_corpus()?;
    let sources = overlay_sources(&fixture, added, empty_a, empty_b);
    let unconstrained = quanta_index_contract::QueryConstraintSetV1::unconstrained();
    let phrase = search(
        &fixture,
        &corpus,
        &sources,
        LqExpr::Leaf(LqLeaf::Phrase("one three".into())),
        None,
        &unconstrained,
    )?;
    if phrase.matched != 1
        || phrase.hits.len() != 1
        || phrase.hits.first().is_none_or(|hit| {
            hit.owner != fixture.owner
                || hit.source != (NativeTextHitSource::Overlay { revision: 7 })
        })
    {
        return Err(IndexError::BackendViolation(
            "fixed overlay phrase owner or revision differs".into(),
        ));
    }
    for literal in ["one-three", "one.three"] {
        let keyword = search(
            &fixture,
            &corpus,
            &sources,
            keyword(literal),
            None,
            &unconstrained,
        )?;
        if keyword != phrase || keyword.hits.iter().any(|hit| hit.owner == deleted) {
            return Err(IndexError::BackendViolation(
                "punctuated keyword differs from fixed overlay phrase gold".into(),
            ));
        }
    }
    for literal in ["three-one", "one-absent-three", "one-two"] {
        let negative = search(
            &fixture,
            &corpus,
            &sources,
            keyword(literal),
            None,
            &unconstrained,
        )?;
        if negative.matched != 0 || !negative.hits.is_empty() {
            return Err(IndexError::BackendViolation(
                "reversed, gapped, or masked keyword matched overlay".into(),
            ));
        }
    }
    let durable = search_durable(&fixture, keyword("one-two"))?;
    if durable.matched != 2
        || durable.hits.len() != 2
        || !durable.hits.iter().any(|hit| hit.owner == fixture.owner)
        || !durable.hits.iter().any(|hit| hit.owner == deleted)
        || durable
            .hits
            .iter()
            .any(|hit| !matches!(hit.source, NativeTextHitSource::Durable(_)))
    {
        return Err(IndexError::BackendViolation(
            "durable punctuated keyword lost fixed two-owner phrase gold".into(),
        ));
    }
    let durable_reverse = search_durable(&fixture, keyword("two-one"))?;
    if durable_reverse.matched != 0 || !durable_reverse.hits.is_empty() {
        return Err(IndexError::BackendViolation(
            "reversed durable keyword matched".into(),
        ));
    }
    // Isolate term admission from the fixture's 100-mutation positional budget.
    // A missing term keeps both fixed owners out of positional verification.
    let at_limit = format!("{}absent", "one-".repeat(63));
    let beyond_limit = format!("{}absent", "one-".repeat(64));
    let at_limit = search_durable(&fixture, keyword(&at_limit))?;
    if at_limit.matched != 0 || !at_limit.hits.is_empty() {
        return Err(IndexError::BackendViolation(
            "64-token keyword changed positional gold".into(),
        ));
    }
    if !matches!(
        search_durable(&fixture, keyword(&beyond_limit)),
        Err(IndexError::InvalidInput(_))
    ) {
        return Err(IndexError::BackendViolation(
            "65-token keyword escaped the original term limit".into(),
        ));
    }
    Ok(())
}

#[test]
fn overlay_corpus_rejects_foreign_selected_view_and_member() -> Result<(), IndexError> {
    let (fixture, _deleted, added, empty_a, empty_b, corpus) = fixture_and_corpus()?;
    let sources = overlay_sources(&fixture, added, empty_a, empty_b);
    let plan = plan(keyword("one"), None)?;
    let constraints = quanta_index_contract::QueryConstraintSetV1::unconstrained();
    let execution = TextContentExecution {
        top_k: 5,
        policy: Bm25Policy { k1: 1.2, b: 0.75 },
        rank_after: None,
    };
    let mut foreign_ticket = fixture.ticket.clone();
    foreign_ticket.view.identity.sequence = foreign_ticket
        .view
        .identity
        .sequence
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    let mut work = budget()?;
    let foreign_view = NativeTextIndex::new(fixture.recipe).search_content_with_overlay(
        &fixture.member,
        &plan,
        &constraints,
        execution,
        Some((&corpus, &sources)),
        TextReadContext {
            ticket: &foreign_ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut work,
        },
    );
    if !matches!(foreign_view, Err(IndexError::Conflict)) {
        return Err(IndexError::BackendViolation(
            "overlay corpus accepted a foreign selected view".into(),
        ));
    }
    let mut foreign_member = fixture.member.clone();
    foreign_member.artifact = reference(199);
    let mut work = budget()?;
    let foreign_artifact = NativeTextIndex::new(fixture.recipe).search_content_with_overlay(
        &foreign_member,
        &plan,
        &constraints,
        execution,
        Some((&corpus, &sources)),
        TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut work,
        },
    );
    if !matches!(foreign_artifact, Err(IndexError::Conflict)) {
        return Err(IndexError::BackendViolation(
            "overlay corpus accepted a foreign member artifact".into(),
        ));
    }
    Ok(())
}

#[test]
fn masked_owner_df_refuses_inventory_without_selected_inverse_posting() -> Result<(), IndexError> {
    let (mut fixture, _deleted, _added, _empty_a, _empty_b, corpus) = fixture_and_corpus()?;
    let phantom = stored_term_key("phantom", CaseMode::Sensitive)?;
    fixture.maps.add(
        fixture.inventory_root,
        phantom.clone(),
        fixture.occurrence_ref,
    )?;
    let mut work = budget()?;
    let result = corpus.adjusted_df(
        &fixture.member,
        &phantom,
        0,
        &mut TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut work,
        },
    );
    if !matches!(result, Err(IndexError::CorruptObject(_))) {
        return Err(IndexError::BackendViolation(
            "inventory-only masked term changed virtual DF".into(),
        ));
    }
    Ok(())
}

#[test]
fn masked_owner_df_refuses_inverse_posting_without_inventory() -> Result<(), IndexError> {
    let (mut fixture, _deleted, _added, _empty_a, _empty_b, corpus) = fixture_and_corpus()?;
    let one = stored_term_key("one", CaseMode::Sensitive)?;
    let inventory = fixture
        .inventory_root
        .0
        .ok_or_else(|| IndexError::BackendViolation("missing test inventory root".into()))?;
    let entries = fixture
        .maps
        .roots
        .get_mut(&inventory)
        .ok_or_else(|| IndexError::BackendViolation("missing test inventory entries".into()))?;
    if entries.remove(&one).is_none() {
        return Err(IndexError::BackendViolation(
            "missing fixture term inventory entry".into(),
        ));
    }
    let mut work = budget()?;
    let result = corpus.adjusted_df(
        &fixture.member,
        &one,
        2,
        &mut TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut work,
        },
    );
    if !matches!(result, Err(IndexError::CorruptObject(_))) {
        return Err(IndexError::BackendViolation(
            "inverse-only masked term changed virtual DF".into(),
        ));
    }
    Ok(())
}

pub(super) fn reader_overlay(
    fixture: &VisitorFixture,
    documents: Vec<TextOverlayReadDocument>,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    top_k: u32,
) -> Result<TextReadResult, IndexError> {
    reader_overlay_after(fixture, documents, constraints, top_k, None)
}

fn reader_overlay_after(
    fixture: &VisitorFixture,
    documents: Vec<TextOverlayReadDocument>,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    top_k: u32,
    after: Option<(FiniteScore, OwnerKey)>,
) -> Result<TextReadResult, IndexError> {
    reader_overlay_plan(
        fixture,
        plan(keyword("one"), None)?,
        documents,
        constraints,
        (top_k, after),
        budget()?,
    )
}

fn reader_overlay_plan(
    fixture: &VisitorFixture,
    plan: crate::plan::LexicalPlan,
    documents: Vec<TextOverlayReadDocument>,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    (top_k, after): (u32, Option<(FiniteScore, OwnerKey)>),
    mut budget: WorkBudget,
) -> Result<TextReadResult, IndexError> {
    let reader = NativeTextReader::new(
        fixture.recipe,
        Bm25Policy { k1: 1.2, b: 0.75 },
        ResourceNeed {
            cpu_units: 1,
            io_bytes: 64 * 1024,
            memory_bytes: 4 * 1024 * 1024,
            provider_units: 0,
        },
    )?;
    let request = PlannedTextRead::new(plan, top_k, constraints, |_bytes, _lifetime| Ok(None))?
        .with_overlay(documents, None)
        .with_rank_after(after);
    let instance = context(fixture.recipe, fixture.ticket.view.source.owners).instance;
    let outcome = reader.query(
        &request,
        &mut QueryReadContext {
            ticket: &fixture.ticket,
            instance: &instance,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut budget,
        },
    )?;
    if outcome.view() != fixture.ticket.view.identity || !outcome.exhaustive() {
        return Err(IndexError::BackendViolation(
            "reader overlay changed selected view or exhaustive traversal".into(),
        ));
    }
    Ok(outcome.into_result())
}

pub(super) fn captured_overlay(
    owner: OwnerKey,
    path: Option<&str>,
    text: Option<&str>,
) -> TextOverlayReadDocument {
    TextOverlayReadDocument {
        owner,
        revision: 19,
        path: path.map(Arc::<str>::from),
        text: text.map(Arc::<str>::from),
        repository_witness: None,
    }
}

/// Publish the visitor fixture owner's `a` path so it can act as an
/// authenticated repository witness (the unscoped repository slot).
fn witness_ready_visitor_fixture() -> Result<VisitorFixture, IndexError> {
    let mut fixture = visitor_fixture()?;
    let paths = MapRoot(Some(reference(80)));
    fixture.maps.add(
        paths,
        b"path\0a".to_vec(),
        quanta_index_core::index::path_owner_map_value(fixture.owner)?,
    )?;
    fixture.ticket.view.source.paths = paths;
    Ok(fixture)
}

/// A new-file row whose repository is established by a durable witness.
fn witnessed_overlay(
    owner: OwnerKey,
    path: &str,
    text: &str,
    witness: OwnerKey,
) -> TextOverlayReadDocument {
    TextOverlayReadDocument {
        repository_witness: Some(witness),
        ..captured_overlay(owner, Some(path), Some(text))
    }
}

#[test]
fn reader_keeps_producer_identity_in_equal_score_file_ties() -> Result<(), IndexError> {
    let fixture = witness_ready_visitor_fixture()?;
    let low = OwnerKey {
        producer: ProducerId([10; 16]),
        stable_id: StableOwnerId([44; 16]),
    };
    let high = OwnerKey {
        producer: ProducerId([11; 16]),
        stable_id: low.stable_id,
    };
    let documents = || {
        vec![
            witnessed_overlay(low, "left/shared.rs", "one", fixture.owner),
            witnessed_overlay(high, "right/shared.rs", "one", fixture.owner),
            captured_overlay(fixture.owner, None, None),
        ]
    };
    let constraints = quanta_index_contract::QueryConstraintSetV1::unconstrained();
    let result = reader_overlay(&fixture, documents(), &constraints, 2)?;
    if result.matched != 2
        || result.hits.len() != 2
        || result
            .hits
            .first()
            .is_none_or(|hit| hit.owner != low || hit.path != "left/shared.rs")
        || result
            .hits
            .get(1)
            .is_none_or(|hit| hit.owner != high || hit.path != "right/shared.rs")
        || result
            .hits
            .first()
            .zip(result.hits.get(1))
            .is_none_or(|(a, b)| a.score != b.score)
    {
        return Err(IndexError::BackendViolation(
            "reader conflated producers or changed fixed equal-score order".into(),
        ));
    }
    let top = reader_overlay(&fixture, documents(), &constraints, 1)?;
    if top.matched != 2
        || top.hits.len() != 1
        || top.hits.first().is_none_or(|hit| hit.owner != low)
    {
        return Err(IndexError::BackendViolation(
            "bounded heap changed full-producer tie gold".into(),
        ));
    }
    let hit = top.hits.first().ok_or(IndexError::Conflict)?;
    let next = reader_overlay_after(
        &fixture,
        documents(),
        &constraints,
        1,
        Some((hit.score, hit.owner)),
    )?;
    if next.matched != 2
        || next.remaining != 1
        || next.hits.len() != 1
        || next
            .hits
            .first()
            .is_none_or(|hit| hit.owner != high || hit.path != "right/shared.rs")
    {
        return Err(IndexError::BackendViolation(
            "rank-after skipped another producer with the same stable ID".into(),
        ));
    }
    Ok(())
}

#[test]
fn reader_refuses_two_producers_claiming_one_selected_file_path() -> Result<(), IndexError> {
    let fixture = witness_ready_visitor_fixture()?;
    let low = OwnerKey {
        producer: ProducerId([10; 16]),
        stable_id: StableOwnerId([44; 16]),
    };
    let high = OwnerKey {
        producer: ProducerId([11; 16]),
        stable_id: low.stable_id,
    };
    let documents = vec![
        witnessed_overlay(low, "shared.rs", "one", fixture.owner),
        witnessed_overlay(high, "shared.rs", "one", fixture.owner),
    ];
    if !matches!(
        reader_overlay(
            &fixture,
            documents,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            2
        ),
        Err(IndexError::Conflict)
    ) {
        return Err(IndexError::BackendViolation(
            "two producers claimed the same source path without refusal".into(),
        ));
    }
    Ok(())
}

#[test]
fn reader_rename_filters_current_path_without_mutating_durable_source() -> Result<(), IndexError> {
    let (mut fixture, second) = two_owner_phrase_fixture()?;
    seed_phrase_source_paths(&mut fixture, second)?;
    let renamed = || {
        vec![captured_overlay(
            fixture.owner,
            Some("renamed.rs"),
            Some("one"),
        )]
    };
    let old = reader_overlay(&fixture, renamed(), &exact_path("a")?, 1)?;
    if old.matched != 0 || !old.hits.is_empty() {
        return Err(IndexError::BackendViolation(
            "renamed overlay leaked its durable path".into(),
        ));
    }
    let current = reader_overlay(&fixture, renamed(), &exact_path("renamed.rs")?, 1)?;
    if current.matched != 1
        || current.hits.first().is_none_or(|hit| {
            hit.owner != fixture.owner
                || hit.path != "renamed.rs"
                || hit.source != (NativeTextHitSource::Overlay { revision: 19 })
        })
    {
        return Err(IndexError::BackendViolation(
            "reader rename lost canonical owner/path/revision".into(),
        ));
    }
    let durable = reader_overlay(&fixture, Vec::new(), &exact_path("a")?, 1)?;
    if durable.matched != 1
        || durable.hits.first().is_none_or(|hit| {
            hit.owner != fixture.owner
                || hit.path != "a"
                || hit.source != NativeTextHitSource::Durable(fixture.source_ref)
        })
    {
        return Err(IndexError::BackendViolation(
            "overlay rename mutated the selected durable source".into(),
        ));
    }
    Ok(())
}

/// Durable owner A is `corp-a:shared.rs` (Rust) and durable owner B is
/// `corp-b:shared.rs` (Python); both durable bodies contain `one`.
fn two_repository_fixture() -> Result<(VisitorFixture, OwnerKey), IndexError> {
    use quanta_index_core::index::{SourcePathOwners, SourcePathRepositoryKey};

    let (mut fixture, second) = two_owner_phrase_fixture()?;
    let paths = MapRoot(Some(reference(80)));
    let bucket = MapRoot(Some(reference(81)));
    for (owner, source, repo, language) in [
        (fixture.owner, fixture.source_ref, "corp-a", "rust"),
        (second, reference(71), "corp-b", "python"),
    ] {
        let stored = fixture
            .objects
            .stored
            .get(&source)
            .ok_or_else(|| IndexError::CorruptObject("fixture source missing".into()))?;
        let mut record: SourceRecord = decode(&stored.payload)?;
        record.path = "shared.rs".into();
        record.source_repo_id =
            Some(RepoId::new(repo).map_err(|error| IndexError::InvalidInput(error.to_string()))?);
        record.language = Some(
            quanta_index_contract::lex::LanguageCode::new(language)
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?,
        );
        let key = SourcePathRepositoryKey::new(record.source_repo_id.as_ref())?;
        fixture.maps.add(
            bucket,
            key.as_slice()?.to_vec(),
            quanta_index_core::index::path_owner_map_value(owner)?,
        )?;
        fixture
            .objects
            .add(source, encode(&record)?, vec![record.bytes])?;
    }
    fixture.maps.add(
        paths,
        b"path\0shared.rs".to_vec(),
        SourcePathOwners::Repositories(reference(81)).encode()?,
    )?;
    fixture.ticket.view.source.paths = paths;
    Ok((fixture, second))
}

fn new_owner(fixture: &VisitorFixture, id: u8) -> OwnerKey {
    OwnerKey {
        producer: fixture.owner.producer,
        stable_id: StableOwnerId([id; 16]),
    }
}

/// Every hit of an exhaustive keyword read as `(owner, origin,
/// "repository:path")`, ordered by owner; the unscoped slot is empty.
fn owner_hits(
    fixture: &VisitorFixture,
    documents: Vec<TextOverlayReadDocument>,
) -> Result<Vec<(OwnerKey, NativeTextHitSource, String)>, IndexError> {
    owner_hits_planned(fixture, documents, plan(keyword("one"), None)?, budget()?)
}

/// `owner_hits` for a parsed native content query such as `one repo:x`.
fn query_hits(
    fixture: &VisitorFixture,
    documents: Vec<TextOverlayReadDocument>,
    query: &str,
) -> Result<Vec<(OwnerKey, NativeTextHitSource, String)>, IndexError> {
    let parsed = quanta_index_contract::parse_native_content_query(query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let plan = crate::planner::LexicalPlanner::plan(&parsed)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    // Compiled `repo:` engines need more byte budget than a keyword read.
    let budget = WorkBudget::new(
        WorkLimits {
            reads: 10_000,
            writes: 100,
            bytes: 256 * 1024 * 1024,
            mutations: 100,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    owner_hits_planned(fixture, documents, plan, budget)
}

fn owner_hits_planned(
    fixture: &VisitorFixture,
    documents: Vec<TextOverlayReadDocument>,
    plan: crate::plan::LexicalPlan,
    budget: WorkBudget,
) -> Result<Vec<(OwnerKey, NativeTextHitSource, String)>, IndexError> {
    let result = reader_overlay_plan(
        fixture,
        plan,
        documents,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        (8, None),
        budget,
    )?;
    if usize::try_from(result.matched).ok() != Some(result.hits.len()) {
        return Err(IndexError::BackendViolation(
            "repository overlay gold expects an exhaustive first page".into(),
        ));
    }
    let mut hits = Vec::new();
    for mut hit in result.hits {
        // Overlay reads project every hit's authenticated repository.
        let (repository, _memory) = hit.take_overlay_repository().ok_or_else(|| {
            IndexError::BackendViolation("overlay hit lacks its repository".into())
        })?;
        let repository = repository.as_ref().map_or("", RepoId::as_str);
        hits.push((hit.owner, hit.source, format!("{repository}:{}", hit.path)));
    }
    hits.sort_by_key(|(owner, _, _)| *owner);
    Ok(hits)
}

fn expect_hits(
    actual: Vec<(OwnerKey, NativeTextHitSource, String)>,
    expected: &[(OwnerKey, NativeTextHitSource, &str)],
    message: &str,
) -> Result<(), IndexError> {
    let expected: Vec<_> = expected
        .iter()
        .map(|(owner, source, path)| (*owner, *source, (*path).to_owned()))
        .collect();
    if actual != expected {
        return Err(IndexError::BackendViolation(format!(
            "{message}: {actual:?} != {expected:?}"
        )));
    }
    Ok(())
}

const UNSAVED: NativeTextHitSource = NativeTextHitSource::Overlay { revision: 19 };

#[test]
fn unsaved_modify_of_a_shared_path_masks_only_its_repository() -> Result<(), IndexError> {
    let (fixture, second) = two_repository_fixture()?;
    let durable_a = NativeTextHitSource::Durable(fixture.source_ref);
    let durable_b = NativeTextHitSource::Durable(reference(71));
    expect_hits(
        owner_hits(&fixture, Vec::new())?,
        &[
            (fixture.owner, durable_a, "corp-a:shared.rs"),
            (second, durable_b, "corp-b:shared.rs"),
        ],
        "durable two-repository gold",
    )?;
    expect_hits(
        owner_hits(
            &fixture,
            vec![captured_overlay(fixture.owner, None, Some("one"))],
        )?,
        &[
            (fixture.owner, UNSAVED, "corp-a:shared.rs"),
            (second, durable_b, "corp-b:shared.rs"),
        ],
        "corp-a modify masked another repository",
    )?;
    expect_hits(
        owner_hits(&fixture, vec![captured_overlay(second, None, Some("one"))])?,
        &[
            (fixture.owner, durable_a, "corp-a:shared.rs"),
            (second, UNSAVED, "corp-b:shared.rs"),
        ],
        "corp-b modify masked another repository",
    )?;
    expect_hits(
        owner_hits(
            &fixture,
            vec![
                captured_overlay(fixture.owner, None, Some("one")),
                captured_overlay(second, None, Some("one")),
            ],
        )?,
        &[
            (fixture.owner, UNSAVED, "corp-a:shared.rs"),
            (second, UNSAVED, "corp-b:shared.rs"),
        ],
        "same relative path edited in both repositories",
    )
}

#[test]
fn unsaved_delete_of_a_shared_path_masks_only_its_repository() -> Result<(), IndexError> {
    let (fixture, second) = two_repository_fixture()?;
    expect_hits(
        owner_hits(&fixture, vec![captured_overlay(fixture.owner, None, None)])?,
        &[(
            second,
            NativeTextHitSource::Durable(reference(71)),
            "corp-b:shared.rs",
        )],
        "corp-a delete removed another repository",
    )?;
    expect_hits(
        owner_hits(&fixture, vec![captured_overlay(second, None, None)])?,
        &[(
            fixture.owner,
            NativeTextHitSource::Durable(fixture.source_ref),
            "corp-a:shared.rs",
        )],
        "corp-b delete removed another repository",
    )
}

#[test]
fn unsaved_create_takes_the_witness_repository_for_path_identity() -> Result<(), IndexError> {
    let (fixture, second) = two_repository_fixture()?;
    let durable_a = NativeTextHitSource::Durable(fixture.source_ref);
    let durable_b = NativeTextHitSource::Durable(reference(71));
    let (first_new, second_new) = (new_owner(&fixture, 40), new_owner(&fixture, 41));
    // One new relative path created unsaved in each repository.
    expect_hits(
        owner_hits(
            &fixture,
            vec![
                witnessed_overlay(first_new, "fresh.rs", "one", fixture.owner),
                witnessed_overlay(second_new, "fresh.rs", "one", second),
            ],
        )?,
        &[
            (fixture.owner, durable_a, "corp-a:shared.rs"),
            (second, durable_b, "corp-b:shared.rs"),
            (first_new, UNSAVED, "corp-a:fresh.rs"),
            (second_new, UNSAVED, "corp-b:fresh.rs"),
        ],
        "same new path in two repositories",
    )?;
    // Recreating corp-a's deleted path keeps corp-b's durable file.
    expect_hits(
        owner_hits(
            &fixture,
            vec![
                captured_overlay(fixture.owner, None, None),
                witnessed_overlay(first_new, "shared.rs", "one", fixture.owner),
            ],
        )?,
        &[
            (second, durable_b, "corp-b:shared.rs"),
            (first_new, UNSAVED, "corp-a:shared.rs"),
        ],
        "corp-a replacement masked corp-b",
    )?;
    // The durable owner of the witness repository's path is still live.
    for (deleted, witness) in [(fixture.owner, second), (second, fixture.owner)] {
        let refused = reader_overlay(
            &fixture,
            vec![
                captured_overlay(deleted, None, None),
                witnessed_overlay(first_new, "shared.rs", "one", witness),
            ],
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            8,
        );
        if !matches!(refused, Err(IndexError::Conflict)) {
            return Err(IndexError::BackendViolation(
                "new file claimed a live durable path in its witness repository".into(),
            ));
        }
    }
    Ok(())
}

#[test]
fn duplicate_unsaved_rows_conflict_only_within_one_repository() -> Result<(), IndexError> {
    let (fixture, second) = two_repository_fixture()?;
    let (first_new, second_new) = (new_owner(&fixture, 40), new_owner(&fixture, 41));
    for documents in [
        vec![
            witnessed_overlay(first_new, "fresh.rs", "one", fixture.owner),
            witnessed_overlay(second_new, "fresh.rs", "one", fixture.owner),
        ],
        vec![
            witnessed_overlay(first_new, "fresh.rs", "one", second),
            witnessed_overlay(second_new, "fresh.rs", "one", second),
        ],
        // A durable rename keeps its own repository.
        vec![
            captured_overlay(second, Some("fresh.rs"), Some("one")),
            witnessed_overlay(first_new, "fresh.rs", "one", second),
        ],
    ] {
        let refused = reader_overlay(
            &fixture,
            documents,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            8,
        );
        if !matches!(refused, Err(IndexError::Conflict)) {
            return Err(IndexError::BackendViolation(
                "duplicate repository/path overlay rows were not refused".into(),
            ));
        }
    }
    expect_hits(
        owner_hits(
            &fixture,
            vec![
                captured_overlay(second, Some("fresh.rs"), Some("one")),
                witnessed_overlay(first_new, "fresh.rs", "one", fixture.owner),
            ],
        )?,
        &[
            (
                fixture.owner,
                NativeTextHitSource::Durable(fixture.source_ref),
                "corp-a:shared.rs",
            ),
            (second, UNSAVED, "corp-b:fresh.rs"),
            (first_new, UNSAVED, "corp-a:fresh.rs"),
        ],
        "corp-b rename conflicted with a corp-a new file",
    )
}

#[test]
fn overlay_repo_filter_selects_rows_by_their_authenticated_repository() -> Result<(), IndexError> {
    let (fixture, second) = two_repository_fixture()?;
    let created = new_owner(&fixture, 40);
    // Both repositories edit `shared.rs` unsaved; corp-b also creates a file.
    let documents = || {
        vec![
            captured_overlay(fixture.owner, None, Some("one")),
            captured_overlay(second, None, Some("one")),
            witnessed_overlay(created, "fresh.rs", "one", second),
        ]
    };
    expect_hits(
        query_hits(&fixture, documents(), "one case:yes repo:corp-b")?,
        &[
            (second, UNSAVED, "corp-b:shared.rs"),
            (created, UNSAVED, "corp-b:fresh.rs"),
        ],
        "repo:corp-b over overlay rows",
    )?;
    expect_hits(
        query_hits(&fixture, documents(), "one case:yes repo:corp-a")?,
        &[(fixture.owner, UNSAVED, "corp-a:shared.rs")],
        "repo:corp-a over overlay rows",
    )?;
    expect_hits(
        query_hits(
            &fixture,
            documents(),
            "one case:yes repo:corp-a repo:corp-b",
        )?,
        &[],
        "every repo: filter must match",
    )
}

#[test]
fn overlay_lang_filter_uses_the_durable_owner_record_language() -> Result<(), IndexError> {
    let (fixture, second) = two_repository_fixture()?;
    let durable_b = NativeTextHitSource::Durable(reference(71));
    let documents = || vec![captured_overlay(fixture.owner, None, Some("one"))];
    expect_hits(
        query_hits(&fixture, documents(), "one case:yes lang:rust")?,
        &[(fixture.owner, UNSAVED, "corp-a:shared.rs")],
        "lang:rust over a durable-owner overlay row",
    )?;
    expect_hits(
        query_hits(&fixture, documents(), "one case:yes lang:python")?,
        &[(second, durable_b, "corp-b:shared.rs")],
        "lang:python excludes the Rust overlay row",
    )?;
    // A rename keeps the durable record's language.
    expect_hits(
        query_hits(
            &fixture,
            vec![captured_overlay(
                fixture.owner,
                Some("renamed.py"),
                Some("one"),
            )],
            "one case:yes lang:rust",
        )?,
        &[(fixture.owner, UNSAVED, "corp-a:renamed.py")],
        "renamed durable-owner row",
    )
}

#[test]
fn overlay_lang_filter_refuses_new_file_rows() -> Result<(), IndexError> {
    let (fixture, _second) = two_repository_fixture()?;
    let created = new_owner(&fixture, 40);
    let documents = || vec![witnessed_overlay(created, "fresh.rs", "one", fixture.owner)];
    // The witness lends its repository, never its language.
    for query in [
        "one case:yes lang:rust",
        "one case:yes repo:corp-a lang:rust",
    ] {
        let refused = query_hits(&fixture, documents(), query);
        if !matches!(&refused, Err(IndexError::InvalidInput(message)) if message.contains("language"))
        {
            return Err(IndexError::BackendViolation(format!(
                "{query} admitted a new-file row without language: {refused:?}"
            )));
        }
    }
    expect_hits(
        query_hits(&fixture, documents(), "one case:yes repo:corp-a")?,
        &[
            (
                fixture.owner,
                NativeTextHitSource::Durable(fixture.source_ref),
                "corp-a:shared.rs",
            ),
            (created, UNSAVED, "corp-a:fresh.rs"),
        ],
        "repo: still applies to a new-file row",
    )
}

#[test]
fn overlay_repository_must_be_an_authenticated_member_of_the_view() -> Result<(), IndexError> {
    let (fixture, second) = two_repository_fixture()?;
    let created = new_owner(&fixture, 40);
    let outside = new_owner(&fixture, 99);
    let deletion_with_witness = TextOverlayReadDocument {
        repository_witness: Some(second),
        ..captured_overlay(created, None, None)
    };
    for documents in [
        // The witness has no durable record in the selected view.
        vec![witnessed_overlay(created, "fresh.rs", "one", outside)],
        // A new file without a witness has no authenticated repository.
        vec![captured_overlay(created, Some("fresh.rs"), Some("one"))],
        // A durable owner cannot be moved to another repository.
        vec![TextOverlayReadDocument {
            repository_witness: Some(second),
            ..captured_overlay(fixture.owner, None, Some("one"))
        }],
        vec![deletion_with_witness],
    ] {
        let refused = reader_overlay(
            &fixture,
            documents,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            8,
        );
        if !matches!(refused, Err(IndexError::InvalidInput(_))) {
            return Err(IndexError::BackendViolation(format!(
                "unauthenticated overlay repository was admitted: {refused:?}"
            )));
        }
    }
    Ok(())
}

#[test]
fn overlay_hits_project_the_unscoped_repository_slot() -> Result<(), IndexError> {
    let fixture = witness_ready_visitor_fixture()?;
    let created = new_owner(&fixture, 40);
    expect_hits(
        owner_hits(
            &fixture,
            vec![witnessed_overlay(created, "left.rs", "one", fixture.owner)],
        )?,
        &[
            (
                fixture.owner,
                NativeTextHitSource::Durable(fixture.source_ref),
                ":a",
            ),
            (created, UNSAVED, ":left.rs"),
        ],
        "unscoped durable or overlay hit gained a repository",
    )
}

/// Overlay reads project file hits with their repository. Path grouping and
/// previews are durable-only contracts, refused even for an empty corpus.
#[test]
fn overlay_reads_refuse_path_projection_and_preview_explicitly() -> Result<(), IndexError> {
    let fixture = witness_ready_visitor_fixture()?;
    let reader = NativeTextReader::new(
        fixture.recipe,
        Bm25Policy { k1: 1.2, b: 0.75 },
        ResourceNeed {
            cpu_units: 1,
            io_bytes: 64 * 1024,
            memory_bytes: 4 * 1024 * 1024,
            provider_units: 0,
        },
    )?;
    let constraints = quanta_index_contract::QueryConstraintSetV1::unconstrained();
    for (path_surface, preview) in [(true, false), (false, true)] {
        let mut plan = plan(keyword("one"), None)?;
        if path_surface {
            plan.filters.surface = crate::symbol::ResultSurface::Path;
        }
        let mut request =
            PlannedTextRead::new(plan, 4, &constraints, |_bytes, _lifetime| Ok(None))?
                .with_overlay(Vec::new(), None);
        if preview {
            request = request.with_preview();
        }
        if !matches!(
            reader.reservation(&request),
            Err(IndexError::InvalidInput(_))
        ) {
            return Err(IndexError::BackendViolation(format!(
                "overlay read admitted path={path_surface} preview={preview}"
            )));
        }
    }
    Ok(())
}
