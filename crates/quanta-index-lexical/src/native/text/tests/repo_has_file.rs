//! Fixed two-repository gold for the Native repository file predicate.

use super::*;
use quanta_index_contract::{RepoId, parse_native_content_query};
use quanta_index_core::index::{SourcePathOwners, SourcePathRepositoryKey};

fn repo_id(value: &str) -> Result<RepoId, IndexError> {
    RepoId::new(value).map_err(|error| IndexError::InvalidInput(error.to_string()))
}

fn add_repository_source(
    fixture: &mut VisitorFixture,
    paths: MapRoot,
    bucket: u8,
    owner: OwnerKey,
    source: ObjectRef,
    path: &str,
    repository: &RepoId,
) -> Result<(), IndexError> {
    let stored = fixture
        .objects
        .stored
        .get(&source)
        .ok_or(IndexError::ResourceLimited)?;
    let mut record: SourceRecord = decode(&stored.payload)?;
    record.path = path.into();
    record.source_repo_id = Some(repository.clone());
    fixture
        .objects
        .add(source, encode(&record)?, vec![record.bytes])?;
    let mut path_key = b"path\0".to_vec();
    path_key.extend_from_slice(path.as_bytes());
    fixture.maps.add(
        paths,
        path_key,
        SourcePathOwners::Repositories(reference(bucket)).encode()?,
    )?;
    let repo_key = SourcePathRepositoryKey::new(Some(repository))?;
    fixture.maps.add(
        MapRoot(Some(reference(bucket))),
        repo_key.as_slice()?.to_vec(),
        quanta_index_core::index::path_owner_map_value(owner)?,
    )?;
    Ok(())
}

fn repo_fixture() -> Result<(VisitorFixture, OwnerKey), IndexError> {
    repo_fixture_with_second_path("body/b.rs")
}

fn repo_fixture_with_second_path(
    second_path: &str,
) -> Result<(VisitorFixture, OwnerKey), IndexError> {
    let (mut fixture, other) = two_owner_phrase_fixture()?;
    let first_repo = repo_id("corp-a")?;
    let other_repo = repo_id("corp-b")?;
    let paths = MapRoot(Some(reference(80)));
    let first_owner = fixture.owner;
    let first_source = fixture.source_ref;
    add_repository_source(
        &mut fixture,
        paths,
        81,
        first_owner,
        first_source,
        "body/a.rs",
        &first_repo,
    )?;
    add_repository_source(
        &mut fixture,
        paths,
        if second_path == "body/a.rs" { 81 } else { 82 },
        other,
        reference(71),
        second_path,
        &other_repo,
    )?;

    // This published source is a witness, not a Text member candidate. A
    // matching file in corp-a must gate corp-a's different body/a.rs file.
    let witness = OwnerKey {
        producer: fixture.owner.producer,
        stable_id: StableOwnerId([93; 16]),
    };
    let witness_body = reference(91);
    let witness_source = reference(92);
    fixture
        .objects
        .add(witness_body, b"needle alpha".to_vec(), Vec::new())?;
    fixture.objects.add(
        witness_source,
        encode(&SourceRecord {
            path: "src/lib.rs".into(),
            bytes: witness_body,
            byte_len: 12,
            text: SourceTextAdmission::Utf8 {
                policy: fixture.recipe.admission_policy,
            },
            source_repo_id: Some(first_repo.clone()),
            language: None,
        })?,
        vec![witness_body],
    )?;
    fixture.maps.add(
        fixture.ticket.view.source.owners,
        owner_map_key(witness).to_vec(),
        witness_source,
    )?;
    add_repository_source(
        &mut fixture,
        paths,
        83,
        witness,
        witness_source,
        "src/lib.rs",
        &first_repo,
    )?;
    fixture.ticket.view.source.paths = paths;
    Ok((fixture, other))
}

#[test]
fn overlay_search_checks_every_selected_path_owner_in_a_repository_bucket() -> Result<(), IndexError>
{
    use super::overlay_tests::{captured_overlay, reader_overlay};
    use quanta_index_core::index::NativeTextHitSource;

    let (fixture, other) = repo_fixture()?;
    let query = |fixture: &VisitorFixture, mask_other| {
        let mut overlay = vec![captured_overlay(
            fixture.owner,
            Some("body/a.rs"),
            Some("one draft"),
        )];
        if mask_other {
            overlay.push(captured_overlay(other, None, None));
            overlay.sort_unstable_by_key(|row| row.owner);
        }
        let constraints = quanta_index_contract::QueryConstraintSetV1::unconstrained();
        reader_overlay(fixture, overlay, &constraints, 2)
    };
    let require_overlay_hit = |fixture: &VisitorFixture, mask_other, matched| {
        let result = query(fixture, mask_other)?;
        check_eq!(result.matched, matched);
        check!(result.hits.iter().any(|hit| {
            hit.owner == fixture.owner
                && hit.source == NativeTextHitSource::Overlay { revision: 19 }
                && hit.path == "body/a.rs"
        }));
        Ok(())
    };
    require_overlay_hit(&fixture, false, 2)?;

    let (shared, _) = repo_fixture_with_second_path("body/a.rs")?;
    check!(matches!(query(&shared, false), Err(IndexError::Conflict)));
    require_overlay_hit(&shared, true, 1)
}

#[test]
fn overlay_rename_in_repository_bucket_keeps_stable_results() -> Result<(), IndexError> {
    use super::overlay_tests::{captured_overlay, reader_overlay};
    use quanta_index_core::index::NativeTextHitSource;

    let (fixture, other) = repo_fixture_with_second_path("body/a.rs")?;
    let overlay = || {
        let mut rows = vec![
            captured_overlay(fixture.owner, Some("body/a.rs"), Some("one draft")),
            captured_overlay(other, Some("body/renamed.rs"), Some("one revised")),
        ];
        rows.sort_unstable_by_key(|row| row.owner);
        rows
    };
    let constraints = quanta_index_contract::QueryConstraintSetV1::unconstrained();
    let first = reader_overlay(&fixture, overlay(), &constraints, 2)?;
    let repeated = reader_overlay(&fixture, overlay(), &constraints, 2)?;
    check_eq!(first.matched, 2);
    check_eq!(first.remaining, 2);
    check_eq!(first.hits.len(), 2);
    check_eq!(repeated.matched, first.matched);
    check_eq!(repeated.remaining, first.remaining);
    for (actual, again) in first.hits.iter().zip(&repeated.hits) {
        check_eq!(actual.owner, again.owner);
        check_eq!(actual.path, again.path);
        check_eq!(actual.score, again.score);
        check_eq!(actual.source, NativeTextHitSource::Overlay { revision: 19 });
    }
    check!(
        first
            .hits
            .iter()
            .any(|hit| { hit.owner == fixture.owner && hit.path == "body/a.rs" })
    );
    check!(
        first
            .hits
            .iter()
            .any(|hit| { hit.owner == other && hit.path == "body/renamed.rs" })
    );
    Ok(())
}

#[test]
fn overlay_repository_bucket_rejects_corrupt_source_repository_and_path() -> Result<(), IndexError>
{
    use super::overlay_tests::{captured_overlay, reader_overlay};

    let (mut fixture, other) = repo_fixture_with_second_path("body/a.rs")?;
    let source = reference(71);
    let stored = fixture
        .objects
        .stored
        .get(&source)
        .ok_or(IndexError::ResourceLimited)?;
    let original: SourceRecord = decode(&stored.payload)?;
    let overlay = || {
        let mut rows = vec![
            captured_overlay(fixture.owner, Some("body/a.rs"), Some("one draft")),
            captured_overlay(other, None, None),
        ];
        rows.sort_unstable_by_key(|row| row.owner);
        rows
    };
    let constraints = quanta_index_contract::QueryConstraintSetV1::unconstrained();

    let mut wrong_repository = original.clone();
    wrong_repository.source_repo_id = Some(repo_id("corp-a")?);
    fixture.objects.add(
        source,
        encode(&wrong_repository)?,
        vec![wrong_repository.bytes],
    )?;
    check!(matches!(
        reader_overlay(&fixture, overlay(), &constraints, 2),
        Err(IndexError::CorruptObject(_))
    ));

    let mut wrong_path = original;
    wrong_path.path = "body/foreign.rs".into();
    fixture
        .objects
        .add(source, encode(&wrong_path)?, vec![wrong_path.bytes])?;
    check!(matches!(
        reader_overlay(&fixture, overlay(), &constraints, 2),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn select_path_unions_only_singleton_keyword_path_postings() -> Result<(), IndexError> {
    use quanta_index_contract::{LqCase, LqExpr, LqFilter, LqLeaf, LqQuery, LqSelect, LqSpan};

    let (mut fixture, other) = repo_fixture()?;
    let path_root = MapRoot(Some(reference(94)));
    for (owner, record, name) in [
        (fixture.owner, fixture.source_ref, b"a".as_slice()),
        (other, reference(71), b"b".as_slice()),
    ] {
        for term in [b"body".as_slice(), name, b"rs".as_slice()] {
            for case in [0, 1] {
                let length = u16::try_from(term.len().saturating_add(1))
                    .map_err(|_error| IndexError::ResourceLimited)?;
                let mut key = length.to_be_bytes().to_vec();
                key.push(case);
                key.extend_from_slice(term);
                key.extend_from_slice(&owner_key(owner));
                fixture.maps.add(path_root, key, record)?;
            }
        }
    }
    let artifact = fixture.member.artifact;
    let stored = fixture
        .objects
        .stored
        .get(&artifact)
        .ok_or(IndexError::ResourceLimited)?;
    let mut root: Root = decode(&stored.payload)?;
    root.path_postings = path_root;
    root.path_indexed_tokens = 6;
    let payload = encode(&root)?;
    fixture.member.logical_commitment = Digest(Sha256::digest(&payload).into());
    fixture
        .objects
        .add(artifact, payload, root_children(root).as_slice().to_vec())?;
    fixture.objects.add(
        fixture.member_ref,
        encode(&fixture.member)?,
        vec![fixture.member.artifact, reference(1)],
    )?;

    let keyword = |value: &str| LqExpr::Leaf(LqLeaf::Keyword(value.into()));
    let search = |expr, project| {
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = expr;
        query.options.case = Some(LqCase::Sensitive);
        if project {
            query.filters.push(LqFilter::Select {
                dim: LqSelect::Path,
            });
        }
        let plan = crate::planner::LexicalPlanner::plan(&query)
            .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        search_phrase_plan(&fixture, &plan, 1)
    };
    check_eq!(search(keyword("body"), false)?.matched, 0);
    let ranked = search(keyword("body"), true)?;
    check_eq!(ranked.matched, 2);
    check_eq!(ranked.remaining, 2);
    check_eq!(ranked.hits.len(), 1);
    check_eq!(
        ranked.hits.first().map(|hit| hit.owner),
        Some(fixture.owner)
    );
    check_eq!(search(keyword("body/a"), true)?.matched, 1);
    check_eq!(search(keyword("a/body"), true)?.matched, 0);
    check_eq!(search(keyword("one"), true)?.matched, 2);
    check_eq!(
        search(LqExpr::All(vec![keyword("body"), keyword("one")]), true)?.matched,
        0
    );
    Ok(())
}

fn search(
    fixture: &VisitorFixture,
    source: &str,
    top_k: u32,
) -> Result<crate::native::text_query::NativeTextRanking, IndexError> {
    search_with_budget(fixture, source, top_k, &mut repo_budget()?)
}

fn search_with_budget(
    fixture: &VisitorFixture,
    source: &str,
    top_k: u32,
    work: &mut WorkBudget,
) -> Result<crate::native::text_query::NativeTextRanking, IndexError> {
    let query = parse_native_content_query(source)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let plan = crate::planner::LexicalPlanner::plan(&query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    NativeTextIndex::new(fixture.recipe).search_content(
        &fixture.member,
        &plan,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        top_k,
        crate::native::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
        TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: work,
        },
    )
}

fn repo_budget() -> Result<WorkBudget, IndexError> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or(IndexError::ResourceLimited)?;
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 0,
            bytes: 256 * 1024 * 1024,
            mutations: 0,
        },
        deadline,
    ))
}

fn funded_repo_budget(ceiling: u64) -> Result<(WorkBudget, Arc<TokenMemory>), IndexError> {
    let memory = Arc::new(TokenMemory {
        ceiling,
        ..TokenMemory::default()
    });
    let mut work = repo_budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BatchOutputMemoryScope(Arc::clone(&memory)),
    ))));
    Ok((work, memory))
}

fn set_source_language(
    fixture: &mut VisitorFixture,
    source: ObjectRef,
    language: &str,
) -> Result<(), IndexError> {
    let stored = fixture
        .objects
        .stored
        .get(&source)
        .ok_or(IndexError::ResourceLimited)?;
    let mut record: SourceRecord = decode(&stored.payload)?;
    record.language = Some(
        quanta_index_contract::lex::LanguageCode::new(language)
            .map_err(|error| IndexError::InvalidInput(error.into()))?,
    );
    fixture
        .objects
        .add(source, encode(&record)?, vec![record.bytes])?;
    Ok(())
}

#[test]
fn repo_file_language_matches_same_file_authenticated_metadata_without_extension_inference()
-> Result<(), IndexError> {
    let (mut fixture, _) = repo_fixture()?;
    let body = fixture.source_ref;
    set_source_language(&mut fixture, body, "rust")?;
    // The authenticated language deliberately disagrees with the .rs suffix.
    set_source_language(&mut fixture, reference(92), "python")?;
    let result = search(
        &fixture,
        "one AND repo:has.file(path:src/lib.rs, lang:PyThOn, content:needle) case:yes",
        1,
    )?;
    assert_eq!(result.matched, 1);
    assert_eq!(result.hits[0].owner, fixture.owner);
    assert_eq!(
        search(
            &fixture,
            "one AND repo:has.file(path:src/lib.rs, lang:python, lang:rust) case:yes",
            1
        )?
        .matched,
        0
    );
    assert_eq!(
        search(
            &fixture,
            "one AND repo:has.file(path:src/lib.rs, lang:rust, content:needle) case:yes",
            1
        )?
        .matched,
        0
    );
    assert_eq!(
        search(
            &fixture,
            "one AND repo:has.file(path:body/a.rs, lang:rust, content:needle) case:yes",
            1
        )?
        .matched,
        0
    );
    Ok(())
}

#[test]
fn repo_file_language_preserves_boolean_gates_and_refunds_funded_memory() -> Result<(), IndexError>
{
    let (mut fixture, other) = repo_fixture()?;
    let body = fixture.source_ref;
    set_source_language(&mut fixture, body, "rust")?;
    set_source_language(&mut fixture, reference(71), "typescript")?;
    set_source_language(&mut fixture, reference(92), "python")?;
    let (mut work, memory) = funded_repo_budget(64 * 1024 * 1024)?;
    let result = search_with_budget(
        &fixture,
        "one AND repo:has.file(lang:python) case:yes",
        1,
        &mut work,
    )?;
    assert_eq!(result.matched, 1);
    assert_eq!(result.hits[0].owner, fixture.owner);
    drop(result);
    assert_eq!(memory.live.load(Ordering::Relaxed), 0);
    let complement = search(
        &fixture,
        "one AND NOT repo:has.file(lang:python) case:yes",
        1,
    )?;
    assert_eq!(complement.matched, 1);
    assert_eq!(complement.hits[0].owner, other);
    assert_eq!(
        search(&fixture, "one OR repo:has.file(lang:python) case:yes", 1)?.matched,
        2
    );
    let (mut denied, memory) = funded_repo_budget(0)?;
    assert!(matches!(
        search_with_budget(
            &fixture,
            "one AND repo:has.file(lang:python) case:yes",
            1,
            &mut denied
        ),
        Err(IndexError::ResourceLimited)
    ));
    assert_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn repo_file_language_refuses_unknown_metadata_instead_of_a_false_empty_result()
-> Result<(), IndexError> {
    let (fixture, _) = repo_fixture()?;
    for input in [
        "one AND repo:has.file(path:src/lib.rs, lang:rust) case:yes",
        "one AND NOT repo:has.file(path:src/lib.rs, lang:rust) case:yes",
    ] {
        let (mut work, memory) = funded_repo_budget(64 * 1024 * 1024)?;
        assert!(matches!(search_with_budget(&fixture, input, 1, &mut work),
            Err(IndexError::InvalidInput(message)) if message.contains("authenticated language")));
        assert_eq!(memory.live.load(Ordering::Relaxed), 0);
    }
    assert_eq!(
        search(
            &fixture,
            "one AND repo:has.file(path:missing.rs, lang:rust) case:yes",
            1
        )?
        .matched,
        0
    );
    Ok(())
}

#[test]
fn repo_predicate_absence_never_requests_zero_byte_memory_grant() -> Result<(), IndexError> {
    let (fixture, _) = two_owner_phrase_fixture()?;
    let query = parse_native_content_query("one case:yes")
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let plan = crate::planner::LexicalPlanner::plan(&query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let (mut work, memory) = funded_batch_budget(64 * 1024 * 1024)?;
    let result = NativeTextIndex::new(fixture.recipe).search_content(
        &fixture.member,
        &plan,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        2,
        crate::native::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
        TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )?;
    assert_eq!(result.matched, 2);
    drop(result);
    assert_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn repo_file_witness_gates_other_file_in_only_its_repository_before_top_k() -> Result<(), IndexError>
{
    let (fixture, other) = repo_fixture()?;
    let baseline = search(&fixture, "one case:yes", 2)?;
    assert_eq!(baseline.matched, 2);
    assert!(baseline.hits.iter().any(|hit| hit.owner == fixture.owner));
    assert!(baseline.hits.iter().any(|hit| hit.owner == other));

    let ungated_top = search(&fixture, "two case:yes", 1)?;
    assert_eq!(ungated_top.matched, 2);
    assert_eq!(ungated_top.hits[0].owner, other);
    let gated = search(
        &fixture,
        "two AND repo:has.file(path:src/lib.rs) case:yes",
        1,
    )?;
    assert_eq!(gated.matched, 1);
    assert_eq!(gated.hits.len(), 1);
    assert_eq!(gated.hits[0].owner, fixture.owner);
    Ok(())
}

#[test]
fn repo_file_regex_name_and_content_match_the_same_published_file() -> Result<(), IndexError> {
    let (fixture, _) = repo_fixture()?;
    let matched = search(
        &fixture,
        r"one AND repo:has.file(path:src/.*, name:lib\.rs, content:needle) case:yes",
        1,
    )?;
    assert_eq!(matched.matched, 1);
    assert_eq!(matched.hits[0].owner, fixture.owner);
    let wrong_file = search(
        &fixture,
        r"one AND repo:has.file(path:body/a\.rs, content:needle) case:yes",
        1,
    )?;
    assert_eq!(wrong_file.matched, 0);
    // Tantivy RegexQuery matches the indexed field as a whole. A substring
    // verify on the Native path/name would silently expand this scope.
    assert_eq!(
        search(&fixture, "one AND repo:has.file(path:lib) case:yes", 1)?.matched,
        0
    );
    assert_eq!(
        search(&fixture, "one AND repo:has.file(name:lib) case:yes", 1)?.matched,
        0
    );
    Ok(())
}

#[test]
fn repo_file_content_uses_shared_nfc_and_query_case_for_keyword_and_regex() -> Result<(), IndexError>
{
    let (mut fixture, _) = repo_fixture()?;
    let body = "cafe\u{301} NEEDLE";
    fixture
        .objects
        .add(reference(91), body.as_bytes().to_vec(), Vec::new())?;
    let source = reference(92);
    let stored = fixture
        .objects
        .stored
        .get(&source)
        .ok_or(IndexError::ResourceLimited)?;
    let mut record: SourceRecord = decode(&stored.payload)?;
    record.byte_len = u64::try_from(body.len()).map_err(|_| IndexError::ResourceLimited)?;
    fixture
        .objects
        .add(source, encode(&record)?, vec![record.bytes])?;

    assert_eq!(
        search(&fixture, "repo:has.file(content:café) case:no", 1)?.matched,
        1
    );
    assert_eq!(
        search(&fixture, "repo:has.file(content:/café/) case:no", 1)?.matched,
        1
    );
    assert_eq!(
        search(&fixture, "repo:has.file(content:needle) case:yes", 1)?.matched,
        0
    );
    Ok(())
}

#[test]
fn repo_file_empty_utf8_body_is_a_valid_nonmatch_for_content_regex() -> Result<(), IndexError> {
    let (mut fixture, _) = repo_fixture()?;
    fixture.objects.add(reference(91), Vec::new(), Vec::new())?;
    let source = reference(92);
    let stored = fixture
        .objects
        .stored
        .get(&source)
        .ok_or(IndexError::ResourceLimited)?;
    let mut witness: SourceRecord = decode(&stored.payload)?;
    witness.byte_len = 0;
    fixture
        .objects
        .add(source, encode(&witness)?, vec![witness.bytes])?;
    let input = "one AND repo:has.file(path:src/lib.rs, content:/needle/) case:yes";
    let parsed = parse_native_content_query(input)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let _plan = crate::planner::LexicalPlanner::plan(&parsed)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let (mut work, memory) = funded_repo_budget(64 * 1024 * 1024)?;
    let result = search_with_budget(&fixture, input, 1, &mut work)?;
    assert_eq!(result.matched, 0);
    assert!(result.hits.is_empty());
    drop(result);
    assert_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn repo_file_scope_preserves_leading_flags_and_full_alternative_match() -> Result<(), IndexError> {
    let (mut fixture, _) = repo_fixture()?;
    assert_eq!(
        search(
            &fixture,
            r"one AND repo:has.file(name:(?i)LIB\.RS) case:yes",
            1
        )?
        .matched,
        1
    );
    assert_eq!(
        search(
            &fixture,
            r"one AND repo:has.file(path:(?i)SRC/LIB\.RS) case:yes",
            1
        )?
        .matched,
        1
    );

    let source = reference(92);
    let stored = fixture
        .objects
        .stored
        .get(&source)
        .ok_or(IndexError::ResourceLimited)?;
    let mut witness: SourceRecord = decode(&stored.payload)?;
    witness.path = "src/aa".into();
    fixture
        .objects
        .add(source, encode(&witness)?, vec![witness.bytes])?;
    let paths = MapRoot(Some(reference(80)));
    let rows = fixture
        .maps
        .roots
        .get_mut(&reference(80))
        .ok_or(IndexError::ResourceLimited)?;
    let previous = rows
        .remove(b"path\0src/lib.rs".as_slice())
        .ok_or(IndexError::ResourceLimited)?;
    fixture
        .maps
        .add(paths, b"path\0src/aa".to_vec(), previous)?;
    assert_eq!(
        search(&fixture, "one AND repo:has.file(name:a|aa) case:yes", 1)?.matched,
        1
    );
    Ok(())
}

#[test]
fn repo_file_path_witness_keeps_excluded_source_but_content_requires_text_admission()
-> Result<(), IndexError> {
    let (mut fixture, _) = repo_fixture()?;
    let source = reference(92);
    let stored = fixture
        .objects
        .stored
        .get(&source)
        .ok_or(IndexError::ResourceLimited)?;
    let mut witness: SourceRecord = decode(&stored.payload)?;
    witness.text = SourceTextAdmission::Excluded;
    fixture
        .objects
        .add(source, encode(&witness)?, vec![witness.bytes])?;
    assert_eq!(
        search(
            &fixture,
            "one AND repo:has.file(path:src/lib.rs) case:yes",
            1
        )?
        .matched,
        1
    );
    assert_eq!(
        search(
            &fixture,
            "one AND repo:has.file(path:src/lib.rs, content:needle) case:yes",
            1
        )?
        .matched,
        0
    );
    Ok(())
}

#[test]
fn repo_file_gate_preserves_or_not_and_counts_before_top_k() -> Result<(), IndexError> {
    let (fixture, other) = repo_fixture()?;
    let complement = search(
        &fixture,
        "one AND NOT repo:has.file(path:src/lib.rs) case:yes",
        1,
    )?;
    assert_eq!(complement.matched, 1);
    assert_eq!(complement.hits[0].owner, other);
    let disjunction = search(
        &fixture,
        "one OR repo:has.file(path:src/lib.rs) case:yes",
        1,
    )?;
    assert_eq!(disjunction.matched, 2);
    assert_eq!(disjunction.hits.len(), 1);
    Ok(())
}

#[test]
fn repo_file_witness_rejects_wrong_repository_and_path_binding() -> Result<(), IndexError> {
    let (mut fixture, _) = repo_fixture()?;
    let source = reference(92);
    let stored = fixture
        .objects
        .stored
        .get(&source)
        .ok_or(IndexError::ResourceLimited)?;
    let original: SourceRecord = decode(&stored.payload)?;
    let mut wrong_repo = original.clone();
    wrong_repo.source_repo_id = Some(repo_id("corp-b")?);
    fixture
        .objects
        .add(source, encode(&wrong_repo)?, vec![wrong_repo.bytes])?;
    let query = "one AND repo:has.file(path:src/lib.rs) case:yes";
    assert!(matches!(
        search(&fixture, query, 1),
        Err(IndexError::CorruptObject(_))
    ));

    let mut wrong_path = original;
    wrong_path.path = "src/other.rs".into();
    fixture
        .objects
        .add(source, encode(&wrong_path)?, vec![wrong_path.bytes])?;
    assert!(matches!(
        search(&fixture, query, 1),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn repo_file_refuses_missing_repository_identity_in_old_cut() -> Result<(), IndexError> {
    let (mut fixture, other) = two_owner_phrase_fixture()?;
    seed_phrase_source_paths(&mut fixture, other)?;
    let input = "one AND repo:has.file(path:src/lib.rs) case:yes";
    let query = parse_native_content_query(input)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let _plan = crate::planner::LexicalPlanner::plan(&query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    assert!(matches!(
        search(&fixture, input, 1),
        Err(IndexError::InvalidInput(message)) if message.contains("repository")
    ));
    Ok(())
}

#[test]
fn repo_file_refuses_overlay_without_authenticated_repository_identity() -> Result<(), IndexError> {
    use crate::native::text_overlay::TextOverlayInput;
    use crate::native::text_query::{Bm25Policy, TextContentExecution, TextOverlaySource};

    let (fixture, _) = repo_fixture()?;
    let index = NativeTextIndex::new(fixture.recipe);
    let input = [TextOverlayInput {
        owner: fixture.owner,
        revision: 7,
        text: Some("one"),
    }];
    let mut preparation = budget()?;
    let corpus = index.prepare_overlay_corpus(
        &fixture.member,
        &input,
        &mut TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut preparation,
        },
    )?;
    let sources = [TextOverlaySource {
        owner: fixture.owner,
        revision: 7,
        path: "body/a.rs",
        body: "one",
    }];
    let query = parse_native_content_query("one AND repo:has.file(path:src/lib.rs) case:yes")
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let plan = crate::planner::LexicalPlanner::plan(&query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let mut work = budget()?;
    assert!(matches!(
        index.search_content_with_overlay(
            &fixture.member,
            &plan,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            TextContentExecution {
                top_k: 1,
                policy: Bm25Policy { k1: 1.2, b: 0.75 },
                rank_after: None,
            },
            Some((&corpus, &sources)),
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut work,
            },
        ),
        Err(IndexError::InvalidInput(_))
    ));
    Ok(())
}

#[test]
fn repo_file_selected_witness_obeys_work_and_refunds_source_memory() -> Result<(), IndexError> {
    let (fixture, _) = repo_fixture()?;
    let mut observed = 0usize;
    let (mut admitted, admitted_memory) = funded_batch_budget(1_000_000)?;
    quanta_index_core::index::visit_repository_path_sources(
        reference(83),
        "src/lib.rs",
        fixture.ticket.view.source.owners,
        &fixture.maps,
        &fixture.objects,
        &mut admitted,
        &mut |_owner, _source, record, _budget| {
            assert_eq!(record.source_repo_id.as_ref(), Some(&repo_id("corp-a")?));
            observed += 1;
            Ok(())
        },
    )?;
    assert_eq!(observed, 1);
    assert!(admitted.usage().reads > 0);
    assert_eq!(admitted_memory.live.load(Ordering::Relaxed), 0);

    let (mut denied, denied_memory) = funded_batch_budget(0)?;
    assert!(matches!(
        quanta_index_core::index::visit_repository_path_sources(
            reference(83),
            "src/lib.rs",
            fixture.ticket.view.source.owners,
            &fixture.maps,
            &fixture.objects,
            &mut denied,
            &mut |_owner, _source, _record, _budget| Ok(()),
        ),
        Err(IndexError::ResourceLimited)
    ));
    assert_eq!(denied_memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}
