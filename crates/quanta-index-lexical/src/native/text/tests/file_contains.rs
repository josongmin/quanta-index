//! Published-source byte gold for the Native file.contains leaf.

use super::*;
use quanta_index_contract::parse_native_content_query;

fn substring_fixture() -> Result<(VisitorFixture, OwnerKey), IndexError> {
    let (mut fixture, other) = two_owner_phrase_fixture()?;
    let mut term_postings = Vec::new();
    let term_pages = fixture
        .maps
        .roots
        .get(&reference(4))
        .ok_or(IndexError::ResourceLimited)?;
    for (fence, value) in term_pages {
        let page_ref = value.object_ref()?;
        let stored = fixture
            .objects
            .stored
            .get(&page_ref)
            .ok_or(IndexError::ResourceLimited)?;
        let page = term_dictionary::CheckedPage::parse(stored.clone(), page_ref, &mut budget()?)?;
        page.validate_fence(fence)?;
        for ordinal in 0..page.len() {
            let (key, posting) = page.entry(ordinal)?;
            term_postings.push((key.to_vec(), posting.object_ref()?));
        }
    }
    // The inherited phrase fixture stores only sensitive term keys. The
    // public parser defaults to folded queries, so publish the same verified
    // `one` occurrences on the folded surface before checking the baseline.
    let folded_one = stored_term_key("one", CaseMode::Folded)?;
    term_postings.push((folded_one.clone(), fixture.posting_ref));
    fixture.maps.add(
        fixture.inventory_root,
        folded_one.clone(),
        fixture.occurrence_ref,
    )?;
    fixture
        .maps
        .add(MapRoot(Some(reference(72))), folded_one, reference(74))?;
    let first_body = "one two gap one one three foo_bar cafe\u{301}".as_bytes();
    let mut first = fixture_record(&fixture)?;
    first.path = "src/lib.rs".into();
    first.byte_len = u64::try_from(first_body.len()).map_err(|_| IndexError::ResourceLimited)?;
    fixture
        .objects
        .add(first.bytes, first_body.to_vec(), Vec::new())?;
    fixture
        .objects
        .add(fixture.source_ref, encode(&first)?, vec![first.bytes])?;

    let second_ref = reference(71);
    let stored = fixture
        .objects
        .stored
        .get(&second_ref)
        .ok_or(IndexError::ResourceLimited)?;
    let mut second: SourceRecord = decode(&stored.payload)?;
    second.path = "src/other.rs".into();
    fixture
        .objects
        .add(second_ref, encode(&second)?, vec![second.bytes])?;

    let occurrence = reference(83);
    fixture.objects.add(
        occurrence,
        encode(&Occurrence {
            positions: Positions(vec![6]),
        })?,
        Vec::new(),
    )?;
    let key = stored_term_key("foo_bar", CaseMode::Sensitive)?;
    fixture
        .maps
        .add(fixture.inventory_root, key.clone(), occurrence)?;
    let posting_root = MapRoot(Some(reference(84)));
    fixture.maps.add(
        posting_root,
        owner_key(fixture.owner).to_vec(),
        OccurrenceDigestWitness::from_reference(occurrence).into_value()?,
    )?;
    let posting = Posting {
        root: posting_root.into(),
        df: 1,
    };
    let posting_ref = reference(85);
    fixture.objects.add(
        posting_ref,
        encode(&posting)?,
        posting_children(&posting)?.as_slice().to_vec(),
    )?;
    term_postings.push((key, posting_ref));
    let accent_occurrence = reference(86);
    fixture.objects.add(
        accent_occurrence,
        encode(&Occurrence {
            positions: Positions(vec![7]),
        })?,
        Vec::new(),
    )?;
    let accent_key = stored_term_key("café", CaseMode::Sensitive)?;
    fixture.maps.add(
        fixture.inventory_root,
        accent_key.clone(),
        accent_occurrence,
    )?;
    let accent_root = MapRoot(Some(reference(87)));
    fixture.maps.add(
        accent_root,
        owner_key(fixture.owner).to_vec(),
        OccurrenceDigestWitness::from_reference(accent_occurrence).into_value()?,
    )?;
    let accent_posting = Posting {
        root: accent_root.into(),
        df: 1,
    };
    let accent_posting_ref = reference(88);
    fixture.objects.add(
        accent_posting_ref,
        encode(&accent_posting)?,
        posting_children(&accent_posting)?.as_slice().to_vec(),
    )?;
    term_postings.push((accent_key, accent_posting_ref));
    seed_fixture_term_dictionary(&mut fixture, &term_postings)?;
    let first_contribution = Contribution {
        record: fixture.source_ref,
        inventory: fixture.inventory_root.into(),
        indexed_tokens: 8,
    };
    fixture.objects.add(
        reference(10),
        encode(&first_contribution)?,
        contribution_children(&first_contribution)?
            .as_slice()
            .to_vec(),
    )?;
    phrase_fixture_root(&mut fixture, 2, 10)?;
    seed_phrase_source_paths(&mut fixture, other)?;
    Ok((fixture, other))
}

fn search(
    fixture: &VisitorFixture,
    query: &str,
    top_k: u32,
) -> Result<crate::native::text_query::NativeTextRanking, IndexError> {
    search_with_constraints(
        fixture,
        query,
        top_k,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        &mut budget()?,
    )
}

fn search_with_constraints(
    fixture: &VisitorFixture,
    query: &str,
    top_k: u32,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    budget: &mut WorkBudget,
) -> Result<crate::native::text_query::NativeTextRanking, IndexError> {
    let parsed = parse_native_content_query(query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let plan = crate::planner::LexicalPlanner::plan(&parsed)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    NativeTextIndex::new(fixture.recipe).search_content(
        &fixture.member,
        &plan,
        constraints,
        top_k,
        crate::native::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
        TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget,
        },
    )
}

fn regex_budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 40_000_000,
            mutations: 100,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

#[test]
fn file_contains_skips_body_of_ineligible_exact_path_owner() -> Result<(), IndexError> {
    let (mut fixture, _) = substring_fixture()?;
    let second = fixture
        .objects
        .stored
        .get(&reference(71))
        .ok_or(IndexError::ResourceLimited)?;
    let second: SourceRecord = decode(&second.payload)?;
    fixture.objects.counted_body = Some(second.bytes);
    let selected = quanta_index_contract::ExactRepoRelativePathV1::new("src/lib.rs")
        .map_err(|error| IndexError::InvalidInput(error.into()))?;
    let constraints =
        quanta_index_contract::QueryConstraintSetV1::from_exact_repo_relative_path(selected);
    let result = search_with_constraints(
        &fixture,
        "file.contains('oo_ba')",
        1,
        &constraints,
        &mut budget()?,
    )?;
    assert_eq!(result.matched, 1);
    assert_eq!(result.hits[0].owner, fixture.owner);
    assert_eq!(fixture.objects.body_reads.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn regex_skips_bodies_of_ineligible_paths() -> Result<(), IndexError> {
    let (mut fixture, _) = substring_fixture()?;
    let second = fixture
        .objects
        .stored
        .get(&reference(71))
        .ok_or(IndexError::ResourceLimited)?;
    let second: SourceRecord = decode(&second.payload)?;
    fixture.objects.counted_body = Some(second.bytes);
    let selected = quanta_index_contract::ExactRepoRelativePathV1::new("src/lib.rs")
        .map_err(|error| IndexError::InvalidInput(error.into()))?;
    let constraints =
        quanta_index_contract::QueryConstraintSetV1::from_exact_repo_relative_path(selected);
    for (query, constraints) in [
        ("/foo_bar/", &constraints),
        (
            "file:lib[.]rs /foo_bar/",
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        ),
    ] {
        // This case can compile a content regex and a path regex. Fund both
        // admissions so the assertion measures source filtering.
        let mut work = regex_budget()?;
        let result = search_with_constraints(&fixture, query, 1, constraints, &mut work)?;
        if result.matched != 1
            || result
                .hits
                .first()
                .is_none_or(|hit| hit.owner != fixture.owner)
            || fixture.objects.body_reads.load(Ordering::Relaxed) != 0
        {
            return Err(IndexError::BackendViolation(format!(
                "regex matched the wrong source or read an ineligible body: {query}"
            )));
        }
    }
    Ok(())
}

#[test]
fn regex_boolean_queries_ignore_corrupt_bodies_outside_the_exact_path() -> Result<(), IndexError> {
    let (mut fixture, _) = substring_fixture()?;
    let second = fixture
        .objects
        .stored
        .get(&reference(71))
        .ok_or(IndexError::ResourceLimited)?;
    let second: SourceRecord = decode(&second.payload)?;
    let invalid_length = usize::try_from(second.byte_len)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    fixture
        .objects
        .add(second.bytes, vec![b'x'; invalid_length], Vec::new())?;
    fixture.objects.counted_body = Some(second.bytes);

    // The unrestricted query proves that the selected corpus contains a
    // malformed body. Exact-path selection must exclude that candidate before
    // regex evaluation, including under Boolean composition.
    let unrestricted = search_with_constraints(
        &fixture,
        "/foo_bar/",
        2,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        &mut regex_budget()?,
    );
    assert!(matches!(unrestricted, Err(IndexError::CorruptObject(_))));
    fixture.objects.body_reads.store(0, Ordering::Relaxed);

    let selected = quanta_index_contract::ExactRepoRelativePathV1::new("src/lib.rs")
        .map_err(|error| IndexError::InvalidInput(error.into()))?;
    let constraints =
        quanta_index_contract::QueryConstraintSetV1::from_exact_repo_relative_path(selected);
    for query in [
        "/foo_bar/",
        "one AND /foo_bar/",
        "one OR /foo_bar/",
        "one AND NOT /absent/",
    ] {
        let result =
            search_with_constraints(&fixture, query, 1, &constraints, &mut regex_budget()?)?;
        assert_eq!(result.matched, 1, "{query}");
        assert_eq!(result.hits.len(), 1, "{query}");
        assert_eq!(
            result.hits.first().map(|hit| hit.owner),
            Some(fixture.owner),
            "{query}"
        );
        assert_eq!(
            fixture.objects.body_reads.load(Ordering::Relaxed),
            0,
            "{query}"
        );
    }
    Ok(())
}

#[test]
fn regex_path_filter_skips_corrupt_body_before_boolean_evaluation() -> Result<(), IndexError> {
    let (mut fixture, other) = substring_fixture()?;
    let baseline = search_with_constraints(
        &fixture,
        "/foo_bar/",
        2,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        &mut regex_budget()?,
    )?;
    assert_eq!(baseline.matched, 1);
    assert_eq!(
        baseline.hits.first().map(|hit| hit.owner),
        Some(fixture.owner)
    );
    let first = fixture_record(&fixture)?;
    let invalid_length = usize::try_from(first.byte_len)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    fixture
        .objects
        .add(first.bytes, vec![b'x'; invalid_length], Vec::new())?;
    fixture.objects.counted_body = Some(first.bytes);

    // The first owner supplied the only baseline match, but its path fails the
    // filter. A rejected path must not open its corrupt body or contribute a
    // positive regex term to an OR branch.
    for (query, expected_matched, expected_owners) in [
        ("file:other[.]rs /foo_bar/", 0, Vec::new()),
        ("file:other[.]rs (one OR /foo_bar/)", 1, vec![other]),
    ] {
        let result = search_with_constraints(
            &fixture,
            query,
            2,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            &mut regex_budget()?,
        )?;
        assert_eq!(result.matched, expected_matched, "{query}");
        assert_eq!(
            result.hits.iter().map(|hit| hit.owner).collect::<Vec<_>>(),
            expected_owners,
            "{query}"
        );
        assert_eq!(
            fixture.objects.body_reads.load(Ordering::Relaxed),
            0,
            "{query}"
        );
    }
    Ok(())
}

#[test]
fn regex_selected_body_preserves_cancellation_after_read() -> Result<(), IndexError> {
    let (mut fixture, _) = substring_fixture()?;
    let first = fixture_record(&fixture)?;
    fixture.objects.cancel_after_get = Some(first.bytes);
    fixture.objects.counted_body = Some(first.bytes);
    let selected = quanta_index_contract::ExactRepoRelativePathV1::new("src/lib.rs")
        .map_err(|error| IndexError::InvalidInput(error.into()))?;
    let constraints =
        quanta_index_contract::QueryConstraintSetV1::from_exact_repo_relative_path(selected);
    let result =
        search_with_constraints(&fixture, "/foo_bar/", 1, &constraints, &mut regex_budget()?);
    assert!(matches!(result, Err(IndexError::Cancelled)));
    assert_eq!(fixture.objects.body_reads.load(Ordering::Relaxed), 1);
    Ok(())
}

#[test]
fn file_contains_matches_published_non_token_bytes_before_boolean_and_top_k()
-> Result<(), IndexError> {
    let (fixture, other) = substring_fixture()?;
    let baseline = search(&fixture, "one", 2)?;
    assert_eq!(baseline.matched, 2);
    assert_eq!(baseline.hits.len(), 2);
    assert!(baseline.hits.iter().any(|hit| hit.owner == fixture.owner));
    assert!(baseline.hits.iter().any(|hit| hit.owner == other));
    // `oo_ba` is inside `foo_bar`; it is not a token in either document.
    let only = search(&fixture, "file.contains('oo_ba')", 1)?;
    assert_eq!(only.matched, 1);
    assert_eq!(only.hits.len(), 1);
    assert_eq!(only.hits[0].owner, fixture.owner);
    assert_eq!(search(&fixture, "file.contains('oo')", 1)?.matched, 1);
    let crate::native::text_query::NativeTextHitSource::Durable(source_ref) = only.hits[0].source
    else {
        return Err(IndexError::CorruptObject(
            "file.contains lost published source".into(),
        ));
    };
    let stored = fixture
        .objects
        .stored
        .get(&source_ref)
        .ok_or(IndexError::ResourceLimited)?;
    let selected: SourceRecord = decode(&stored.payload)?;
    assert_eq!(selected.path, "src/lib.rs");

    let both = search(&fixture, "one OR file.contains('oo_ba')", 1)?;
    assert_eq!(both.matched, 2);
    assert_eq!(both.hits.len(), 1);
    let included = search(&fixture, "one AND file.contains('oo_ba')", 1)?;
    assert_eq!(included.matched, 1);
    assert_eq!(included.hits[0].owner, fixture.owner);
    let same_source = search(
        &fixture,
        "file.contains('oo_ba') AND file.contains('café')",
        1,
    )?;
    assert_eq!(same_source.matched, 1);
    assert_eq!(same_source.hits[0].owner, fixture.owner);
    let one_missing = search(
        &fixture,
        "file.contains('oo_ba') AND file.contains('absent')",
        1,
    )?;
    assert_eq!(one_missing.matched, 0);
    let excluded = search(&fixture, "one AND NOT file.contains('oo_ba')", 1)?;
    assert_eq!(excluded.matched, 1);
    assert_eq!(excluded.hits[0].owner, other);
    Ok(())
}

#[test]
fn file_contains_preserves_case_and_nfc_source_semantics() -> Result<(), IndexError> {
    let (fixture, _) = substring_fixture()?;
    assert_eq!(search(&fixture, "file.contains('OO_BA')", 2)?.matched, 1);
    assert_eq!(
        search(&fixture, "file.contains('OO_BA') case:yes", 2)?.matched,
        0
    );
    assert_eq!(
        search(&fixture, "file.contains('oo_ba') case:yes", 2)?.matched,
        1
    );
    // The published UTF-8 body uses decomposed e + acute; the query is NFC.
    assert_eq!(search(&fixture, "file.contains('café')", 2)?.matched, 1);
    Ok(())
}

#[test]
fn file_contains_refuses_changed_admission_and_declared_body_length() -> Result<(), IndexError> {
    let (mut fixture, _) = substring_fixture()?;
    let original = fixture_record(&fixture)?;
    let mut excluded = original.clone();
    excluded.text = SourceTextAdmission::Excluded;
    fixture
        .objects
        .add(fixture.source_ref, encode(&excluded)?, vec![excluded.bytes])?;
    assert!(matches!(
        search(&fixture, "file.contains('oo_ba')", 1),
        Err(IndexError::CorruptObject(_))
    ));
    let mut wrong_length = original;
    wrong_length.byte_len += 1;
    fixture.objects.add(
        fixture.source_ref,
        encode(&wrong_length)?,
        vec![wrong_length.bytes],
    )?;
    assert!(matches!(
        search(&fixture, "file.contains('oo_ba')", 1),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn file_contains_pays_query_literal_work_and_refunds_temporary_memory() -> Result<(), IndexError> {
    let long_needle = "x".repeat(4_096);
    let (mut refused, refused_memory) = funded_token_budget(1)?;
    assert!(matches!(
        crate::native::text_query::file_contains_mask(
            "x",
            &[long_needle.as_str()],
            CaseMode::Sensitive,
            &mut refused,
        ),
        Err(IndexError::ResourceLimited)
    ));
    assert_eq!(refused_memory.live.load(Ordering::Relaxed), 0);

    let (mut funded, memory) = funded_token_budget(100_000)?;
    assert_eq!(
        crate::native::text_query::file_contains_mask(
            "x",
            &[long_needle.as_str()],
            CaseMode::Sensitive,
            &mut funded,
        )?,
        0
    );
    assert!(funded.usage().bytes >= long_needle.len() as u64);
    assert_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn file_contains_single_slash_is_a_literal_substring() -> Result<(), IndexError> {
    assert_eq!(
        crate::native::text_query::file_contains_mask(
            "a/b",
            &["/"],
            CaseMode::Sensitive,
            &mut budget()?,
        )?,
        1
    );
    Ok(())
}
