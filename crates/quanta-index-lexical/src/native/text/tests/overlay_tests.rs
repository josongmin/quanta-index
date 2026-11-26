use super::super::super::text_overlay::{TextOverlayCorpus, TextOverlayDocument, TextOverlayInput};
use super::super::super::text_query::{
    Bm25Policy, NativeTextHitSource, NativeTextRanking, TextContentExecution, TextOverlaySource,
};
use super::super::super::text_reader::{
    NativeTextReader, PlannedTextRead, TextOverlayReadDocument, TextReadResult,
};
use super::*;
use quanta_index_contract::{LqCase, LqCountBound, LqExpr, LqLeaf, LqQuery, LqSpan};
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
        },
        TextOverlaySource {
            owner: added,
            revision: 9,
            path: "c",
            body: "one one",
        },
        TextOverlaySource {
            owner: empty_a,
            revision: 10,
            path: "d",
            body: "",
        },
        TextOverlaySource {
            owner: empty_b,
            revision: 11,
            path: "e",
            body: "",
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
    let request = PlannedTextRead::new(
        plan(keyword("one"), None)?,
        top_k,
        constraints,
        |_bytes, _lifetime| Ok(None),
    )?
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
            budget: &mut budget()?,
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
    }
}

#[test]
fn reader_keeps_producer_identity_in_equal_score_file_ties() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
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
            captured_overlay(low, Some("left/shared.rs"), Some("one")),
            captured_overlay(high, Some("right/shared.rs"), Some("one")),
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
    let fixture = visitor_fixture()?;
    let low = OwnerKey {
        producer: ProducerId([10; 16]),
        stable_id: StableOwnerId([44; 16]),
    };
    let high = OwnerKey {
        producer: ProducerId([11; 16]),
        stable_id: low.stable_id,
    };
    let documents = vec![
        captured_overlay(low, Some("shared.rs"), Some("one")),
        captured_overlay(high, Some("shared.rs"), Some("one")),
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
