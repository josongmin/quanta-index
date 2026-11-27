//! Native text match enumeration over a real segmented Text member.
//!
//! Every expected row is derived by hand from the fixed bodies below under
//! ripgrep's default line-oriented, leftmost-first semantics.

use super::super::super::literal_blocks::{LiteralBlockBuilder, PushResult, literal_key};
use super::super::super::literal_packed::PackedRoot;
use super::super::super::text_literal_anchor::{CharVariants, SimpleCaseFold};
use super::super::super::text_match_scan::{
    ScannedTextMatches, compile_match_pattern, scan_text_matches,
};
use super::anchor_merge::Rng;
use super::*;
use quanta_index_contract::RepoId;
use quanta_index_contract::index::ViewSelection;
use quanta_index_contract::native_protocol::{
    NativeTextMatchCase, NativeTextMatchCompletion, NativeTextMatchCursor, NativeTextMatchPattern,
    NativeTextMatchQuery, NativeTextMatchesRequest,
};
use quanta_index_core::index::{
    IndexInstanceId, InputBinding, MemoryLease, MemoryLeaseOwner, MemoryScope, MemoryScopePort,
    SchemaId, StorageFormatId,
};
use regex_syntax::hir::{Class, HirKind};
use std::collections::BTreeSet;
use std::fmt::Write as _;

const LITERAL_FORMAT: StorageFormatId = StorageFormatId([0x4c; 16]);
const LITERAL_INSTANCE: IndexInstanceId = IndexInstanceId([0x4d; 16]);

struct World {
    fixture: VisitorFixture,
    owners: Vec<OwnerKey>,
    bodies: Vec<Vec<u8>>,
}

type Row = (Option<String>, String, u64, u64);

fn large_budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 50_000_000,
            writes: 10_000_000,
            bytes: 4 * 1024 * 1024 * 1024,
            mutations: 10_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(600))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn tagged_ref(tag: u8, ordinal: usize) -> Result<ObjectRef, IndexError> {
    let [high, low] = u16::try_from(ordinal)
        .map_err(|_width| IndexError::ResourceLimited)?
        .to_be_bytes();
    let mut digest = [tag; 32];
    for (slot, byte) in digest.iter_mut().skip(1).zip([high, low]) {
        *slot = byte;
    }
    Ok(ObjectRef(Digest(digest)))
}

fn digest_ref(payload: &[u8]) -> ObjectRef {
    ObjectRef(Digest(Sha256::digest(payload).into()))
}

fn repo(name: &str) -> Result<RepoId, IndexError> {
    RepoId::new(name).map_err(|error| IndexError::InvalidInput(error.to_string()))
}

/// A real segmented Text member: one admitted source per `(repo, path, body)`.
fn world(files: &[(Option<&str>, String, Vec<u8>)]) -> Result<World, IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.recipe = TextRecipe::segmented_full(fixture.recipe.admission_policy);
    fixture.objects.generated = Some(Mutex::default());
    let source_root = fixture.ticket.view.source.owners;
    let source_key = source_root
        .0
        .ok_or_else(|| IndexError::BackendViolation("fixture source root missing".into()))?;
    let _previous = fixture.maps.roots.insert(source_key, BTreeMap::new());
    let mut owners = Vec::new();
    let mut bodies = Vec::new();
    let mut tracked = BTreeSet::new();
    for (ordinal, (repository, path, body)) in files.iter().enumerate() {
        let [high, low] = u16::try_from(ordinal)
            .map_err(|_width| IndexError::ResourceLimited)?
            .to_be_bytes();
        let mut stable = [0x5a; 16];
        for (slot, byte) in stable.iter_mut().zip([high, low]) {
            *slot = byte;
        }
        let owner = OwnerKey {
            producer: fixture.owner.producer,
            stable_id: StableOwnerId(stable),
        };
        let body_ref = tagged_ref(0xb0, ordinal)?;
        let source_ref = tagged_ref(0xc0, ordinal)?;
        let record = SourceRecord {
            path: path.clone(),
            bytes: body_ref,
            byte_len: u64::try_from(body.len()).map_err(|_error| IndexError::ResourceLimited)?,
            text: SourceTextAdmission::Utf8 {
                policy: fixture.recipe.admission_policy,
            },
            source_repo_id: repository.map(repo).transpose()?,
            language: None,
        };
        fixture.objects.add(body_ref, body.clone(), Vec::new())?;
        fixture
            .objects
            .add(source_ref, encode(&record)?, vec![body_ref])?;
        fixture
            .maps
            .add(source_root, owner_map_key(owner).to_vec(), source_ref)?;
        owners.push(owner);
        bodies.push(body.clone());
        let _inserted = tracked.insert(body_ref);
    }
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = large_budget()?;
    let source = fixture.ticket.view.source;
    let mut attempt = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let (artifact, logical) =
        text_segment::build::build(fixture.recipe, source, DeltaInput::Bootstrap, &mut attempt)?;
    fixture.member.artifact = artifact;
    fixture.member.logical_commitment = logical;
    fixture.member.input.policy = fixture.recipe.output_policy();
    let index = NativeTextIndex::new(fixture.recipe);
    let member = ready(index.verify_and_seal(
        TextCandidate {
            binding: fixture.member.clone(),
            prepared_attempt: None,
        },
        &mut attempt,
    ))?;
    check_eq!(member, fixture.member);
    fixture.objects.add(
        fixture.member_ref,
        encode(&member)?,
        std::iter::once(artifact).chain(source.owners.0).collect(),
    )?;
    fixture.objects.tracked_bodies = tracked;
    Ok(World {
        fixture,
        owners,
        bodies,
    })
}

fn text_files(
    files: &[(Option<&str>, &str, &str)],
) -> Vec<(Option<&'static str>, String, Vec<u8>)> {
    files
        .iter()
        .map(|(repository, path, body)| {
            (
                repository.map(|name| match name {
                    "corp-a" => "corp-a",
                    "corp-b" => "corp-b",
                    _ => "corp-x",
                }),
                (*path).to_owned(),
                body.as_bytes().to_vec(),
            )
        })
        .collect()
}

/// Publishes a packed Literal member over every raw byte trigram of the view.
fn publish_sibling(world: &mut World) -> Result<(), IndexError> {
    let mut keys = BTreeSet::new();
    for (owner, body) in world.owners.iter().zip(&world.bodies) {
        let mut owner_bytes = [0_u8; 32];
        for (slot, byte) in owner_bytes
            .iter_mut()
            .zip(owner.producer.0.iter().chain(owner.stable_id.0.iter()))
        {
            *slot = *byte;
        }
        for window in body.windows(3) {
            let gram = <[u8; 3]>::try_from(window).map_err(|_width| IndexError::ResourceLimited)?;
            let _inserted = keys.insert((gram, owner_bytes));
        }
    }
    let mut work = large_budget()?;
    let fixture = &mut world.fixture;
    let sources = fixture.ticket.view.source.owners;
    let blocks = MapRoot(Some(tagged_ref(0xd0, 0)?));
    let mut builder = LiteralBlockBuilder::new(&work, None)?;
    let mut pending = Vec::new();
    for (gram, owner) in &keys {
        let key = literal_key(*gram, *owner);
        if builder.try_push(key, &mut work)? == PushResult::Full {
            pending.push(builder.finish()?);
            builder = LiteralBlockBuilder::new(&work, None)?;
            check!(builder.try_push(key, &mut work)? == PushResult::Added);
        }
    }
    if !builder.is_empty() {
        pending.push(builder.finish()?);
    }
    for block in pending {
        let last = block.last_key;
        let (input, _memory) = block.into_batch_input();
        let reference = digest_ref(&input.payload);
        fixture.objects.add(reference, input.payload, Vec::new())?;
        fixture.maps.add(blocks, last.to_vec(), reference)?;
    }
    let root = PackedRoot { sources, blocks };
    let payload = root.fixture_payload()?;
    let artifact = digest_ref(&payload);
    fixture.objects.add(
        artifact,
        payload.clone(),
        root.children().as_slice().to_vec(),
    )?;
    let member = MemberBinding {
        instance: LITERAL_INSTANCE,
        format: LITERAL_FORMAT,
        artifact,
        input: InputBinding {
            root: sources,
            schema: SchemaId([0x4e; 16]),
            policy: Digest([0x4f; 32]),
            context: Digest([0x50; 32]),
        },
        logical_commitment: Digest(Sha256::digest(&payload).into()),
    };
    let member_payload = encode(&member)?;
    let member_ref = digest_ref(&member_payload);
    fixture.objects.add(
        member_ref,
        member_payload,
        [Some(artifact), sources.0].into_iter().flatten().collect(),
    )?;
    fixture.maps.add(
        fixture.ticket.view.members,
        member_map_key(LITERAL_INSTANCE).to_vec(),
        member_ref,
    )?;
    Ok(())
}

fn query(pattern: NativeTextMatchPattern, case: NativeTextMatchCase) -> NativeTextMatchQuery {
    NativeTextMatchQuery {
        pattern,
        case,
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
    }
}

fn literal(text: &str) -> NativeTextMatchQuery {
    query(
        NativeTextMatchPattern::Literal(text.into()),
        NativeTextMatchCase::Sensitive,
    )
}

fn regex(text: &str) -> NativeTextMatchQuery {
    query(
        NativeTextMatchPattern::Regex(text.into()),
        NativeTextMatchCase::Sensitive,
    )
}

fn request(
    world: &World,
    query: NativeTextMatchQuery,
    after: Option<NativeTextMatchCursor>,
    limit: u32,
) -> NativeTextMatchesRequest {
    NativeTextMatchesRequest {
        selection: ViewSelection::Exact(world.fixture.ticket.view.identity),
        instance: world.fixture.member.instance,
        query,
        after,
        limit,
    }
}

fn take_reads(world: &World) -> usize {
    world
        .fixture
        .objects
        .tracked_body_reads
        .swap(0, Ordering::Relaxed)
}

fn run(
    world: &World,
    index: NativeTextIndex,
    request: &NativeTextMatchesRequest,
    budget: &mut WorkBudget,
) -> Result<ScannedTextMatches, IndexError> {
    let fixture = &world.fixture;
    let bound = BoundTextMember {
        member: fixture.member.clone(),
        view: fixture.ticket.view.identity,
    };
    scan_text_matches(
        &index,
        &bound,
        request,
        &mut TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget,
        },
    )
}

/// One page as `(repository, path, start, end)`, after proving every range
/// against its own source bytes: in bounds and UTF-8 aligned.
fn page(
    world: &World,
    request: &NativeTextMatchesRequest,
) -> Result<
    (
        Vec<Row>,
        NativeTextMatchCompletion,
        Option<NativeTextMatchCursor>,
    ),
    IndexError,
> {
    let scanned = run(
        world,
        NativeTextIndex::new(world.fixture.recipe),
        request,
        &mut large_budget()?,
    )?;
    let mut rows = Vec::new();
    for row in &scanned.page.matches {
        let ordinal = world
            .owners
            .iter()
            .position(|owner| *owner == row.native_source.owner)
            .ok_or_else(|| IndexError::BackendViolation("row names an unknown owner".into()))?;
        let body = world
            .bodies
            .get(ordinal)
            .ok_or_else(|| IndexError::BackendViolation("fixture body missing".into()))?;
        let text = std::str::from_utf8(body)
            .map_err(|_utf8| IndexError::BackendViolation("fixture body is not UTF-8".into()))?;
        let start =
            usize::try_from(row.range.start).map_err(|_width| IndexError::ResourceLimited)?;
        let end = usize::try_from(row.range.end).map_err(|_width| IndexError::ResourceLimited)?;
        check!(
            text.get(start..end)
                .is_some_and(|matched| !matched.is_empty()),
            "match range is out of bounds, empty or not UTF-8 aligned"
        );
        rows.push((
            row.repository.as_ref().map(|name| name.as_str().to_owned()),
            row.path.clone(),
            row.range.start,
            row.range.end,
        ));
    }
    Ok((rows, scanned.page.completion, scanned.page.next.clone()))
}

fn rows_of(rows: &[(Option<&str>, &str, u64, u64)]) -> Vec<Row> {
    rows.iter()
        .map(|(repository, path, start, end)| {
            (
                repository.map(str::to_owned),
                (*path).to_owned(),
                *start,
                *end,
            )
        })
        .collect()
}

fn all(world: &World, query: NativeTextMatchQuery) -> Result<Vec<Row>, IndexError> {
    let (rows, completion, next) = page(world, &request(world, query, None, 64))?;
    check!(completion == NativeTextMatchCompletion::Exhausted && next.is_none());
    Ok(rows)
}

#[test]
fn repeated_adjacent_and_overlapping_candidates_are_leftmost_first() -> Result<(), IndexError> {
    let world = world(&text_files(&[
        (None, "src/a.rs", "aaaa ab abab\nxx aaa\n"),
        (None, "src/b.rs", "ababab abab"),
    ]))?;
    // Adjacent `aa` matches, and a third `a` left over on the next line.
    check_eq!(
        all(&world, literal("aa"))?,
        rows_of(&[
            (None, "src/a.rs", 0, 2),
            (None, "src/a.rs", 2, 4),
            (None, "src/a.rs", 16, 18),
        ])
    );
    // Overlapping candidates: `aba` then `bab`, never both `aba`s.
    check_eq!(
        all(&world, regex("aba|bab"))?,
        rows_of(&[
            (None, "src/a.rs", 8, 11),
            (None, "src/b.rs", 0, 3),
            (None, "src/b.rs", 3, 6),
            (None, "src/b.rs", 7, 10),
        ])
    );
    // Leftmost-first prefers the first alternative, not the longest match.
    check_eq!(
        all(&world, regex("ab|abab"))?,
        rows_of(&[
            (None, "src/a.rs", 5, 7),
            (None, "src/a.rs", 8, 10),
            (None, "src/a.rs", 10, 12),
            (None, "src/b.rs", 0, 2),
            (None, "src/b.rs", 2, 4),
            (None, "src/b.rs", 4, 6),
            (None, "src/b.rs", 7, 9),
            (None, "src/b.rs", 9, 11),
        ])
    );
    // Empty matches are dropped; `^` and `$` anchor each line.
    check_eq!(
        all(&world, regex("x*"))?,
        rows_of(&[(None, "src/a.rs", 13, 15)])
    );
    // No whole line consists only of `a`.
    check_eq!(all(&world, regex("^a+$"))?, Vec::<Row>::new());
    check_eq!(
        all(&world, regex("a+$"))?,
        rows_of(&[(None, "src/a.rs", 16, 19)])
    );
    Ok(())
}

#[test]
fn raw_utf8_nfc_and_crlf_bytes_are_matched_as_written() -> Result<(), IndexError> {
    let world = world(&text_files(&[
        (None, "src/nfc.rs", "café ☕ café"),
        (None, "src/raw.rs", "cafe\u{301} x café"),
        (None, "src/crlf.rs", "foo\r\nbar\r\n"),
    ]))?;
    // Multibyte scalars keep byte offsets: é is 2 bytes and ☕ is 3.
    check_eq!(
        all(&world, literal("café"))?,
        rows_of(&[
            (None, "src/nfc.rs", 0, 5),
            (None, "src/nfc.rs", 10, 15),
            (None, "src/raw.rs", 9, 14),
        ])
    );
    // The decomposed spelling matches only its own raw bytes.
    check_eq!(
        all(&world, literal("cafe\u{301}"))?,
        rows_of(&[(None, "src/raw.rs", 0, 6)])
    );
    check_eq!(
        all(&world, regex("☕"))?,
        rows_of(&[(None, "src/nfc.rs", 6, 9)])
    );
    // `\r` stays in its line: `.` reaches it and `r$` cannot end a CRLF line.
    check_eq!(
        all(&world, regex("o.$"))?,
        rows_of(&[(None, "src/crlf.rs", 2, 4)])
    );
    check_eq!(all(&world, regex("r$"))?, Vec::<Row>::new());
    check_eq!(
        all(&world, literal("\r"))?,
        rows_of(&[(None, "src/crlf.rs", 3, 4), (None, "src/crlf.rs", 8, 9)])
    );
    Ok(())
}

#[test]
fn case_policy_folds_kelvin_only_when_insensitive() -> Result<(), IndexError> {
    let world = world(&text_files(&[(None, "src/k.rs", "K k \u{212A} x")]))?;
    let folded = |text: &str| {
        query(
            NativeTextMatchPattern::Literal(text.into()),
            NativeTextMatchCase::Insensitive,
        )
    };
    let every_k = rows_of(&[
        (None, "src/k.rs", 0, 1),
        (None, "src/k.rs", 2, 3),
        (None, "src/k.rs", 4, 7),
    ]);
    check_eq!(all(&world, folded("k"))?, every_k);
    check_eq!(all(&world, folded("\u{212A}"))?, every_k);
    check_eq!(
        all(&world, literal("k"))?,
        rows_of(&[(None, "src/k.rs", 2, 3)])
    );
    check_eq!(
        all(&world, literal("\u{212A}"))?,
        rows_of(&[(None, "src/k.rs", 4, 7)])
    );
    Ok(())
}

#[test]
fn pages_order_by_repository_path_start_and_continue_by_cursor() -> Result<(), IndexError> {
    let mut world = world(&text_files(&[
        (Some("corp-b"), "src/a.rs", "x"),
        (None, "z.rs", "x"),
        (Some("corp-a"), "src/a.rs", "x x"),
    ]))?;
    let every = rows_of(&[
        (None, "z.rs", 0, 1),
        (Some("corp-a"), "src/a.rs", 0, 1),
        (Some("corp-a"), "src/a.rs", 2, 3),
        (Some("corp-b"), "src/a.rs", 0, 1),
    ]);
    check_eq!(all(&world, literal("x"))?, every);
    // A page that ends inside one file continues after its last start.
    let (first, completion, cursor) = page(&world, &request(&world, literal("x"), None, 2))?;
    check_eq!(
        first,
        every.get(..2).ok_or(IndexError::ResourceLimited)?.to_vec()
    );
    check!(completion == NativeTextMatchCompletion::More);
    let cursor =
        cursor.ok_or_else(|| IndexError::BackendViolation("More lacks a cursor".into()))?;
    let (second, completion, next) = page(
        &world,
        &request(&world, literal("x"), Some(cursor.clone()), 2),
    )?;
    check_eq!(
        second,
        every.get(2..).ok_or(IndexError::ResourceLimited)?.to_vec()
    );
    // Exactly `limit` rows with nothing after them is a complete answer.
    check!(completion == NativeTextMatchCompletion::Exhausted && next.is_none());
    let (single, completion, _next) = page(
        &world,
        &request(&world, literal("x"), Some(cursor.clone()), 1),
    )?;
    check_eq!(
        single,
        every.get(2..3).ok_or(IndexError::ResourceLimited)?.to_vec()
    );
    check!(completion == NativeTextMatchCompletion::More);

    // The cursor's file must keep its owner and its Source in the view.
    let mut moved = cursor.clone();
    moved.native_source.owner = OwnerKey {
        producer: moved.native_source.owner.producer,
        stable_id: StableOwnerId([0x77; 16]),
    };
    let mut edited = cursor.clone();
    edited.native_source.source = ObjectRef(Digest([0x77; 32]));
    for forged in [moved, edited] {
        check!(matches!(
            run(
                &world,
                NativeTextIndex::new(world.fixture.recipe),
                &request(&world, literal("x"), Some(forged), 2),
                &mut large_budget()?
            ),
            Err(IndexError::Conflict)
        ));
    }
    // A later view (changed sources) refuses the earlier view's cursor.
    let earlier = request(&world, literal("x"), Some(cursor), 2);
    world.fixture.ticket.view.identity.sequence = world
        .fixture
        .ticket
        .view
        .identity
        .sequence
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    check!(matches!(
        run(
            &world,
            NativeTextIndex::new(world.fixture.recipe),
            &earlier,
            &mut large_budget()?
        ),
        Err(IndexError::Conflict)
    ));
    Ok(())
}

#[test]
fn refused_patterns_read_no_body() -> Result<(), IndexError> {
    let world = world(&text_files(&[(None, "src/a.rs", "a\nb ÿ")]))?;
    let _stale = take_reads(&world);
    for refused in [
        // Byte-level constructs could split a UTF-8 scalar.
        regex("(?-u:\\xFF)"),
        regex("(?-u:.)"),
        // A pattern that needs a line terminator cannot match one line.
        regex("a\\nb"),
        regex("[\\n]"),
        literal("a\nb"),
        regex("("),
    ] {
        check!(matches!(
            run(
                &world,
                NativeTextIndex::new(world.fixture.recipe),
                &request(&world, refused, None, 4),
                &mut large_budget()?
            ),
            Err(IndexError::InvalidInput(_))
        ));
    }
    check_eq!(take_reads(&world), 0);
    // Classes that merely include `\n` are allowed and never match it.
    check_eq!(
        all(&world, regex("[\\na]"))?,
        rows_of(&[(None, "src/a.rs", 0, 1)])
    );
    // Newline in an unused branch cannot suppress the line-local match.
    check_eq!(
        all(&world, regex("a|\\n"))?,
        rows_of(&[(None, "src/a.rs", 0, 1)])
    );
    check_eq!(
        all(&world, regex("\\n?a"))?,
        rows_of(&[(None, "src/a.rs", 0, 1)])
    );
    let optional = self::world(&text_files(&[(None, "src/ab.rs", "ab")]))?;
    check_eq!(
        all(&optional, regex("a\\n?b"))?,
        rows_of(&[(None, "src/ab.rs", 0, 2)])
    );
    Ok(())
}

#[test]
fn budget_and_cancellation_refuse_the_whole_page() -> Result<(), IndexError> {
    let world = world(&text_files(&[
        (None, "src/a.rs", "x"),
        (None, "src/b.rs", "x"),
    ]))?;
    let request = request(&world, literal("x"), None, 4);
    let index = NativeTextIndex::new(world.fixture.recipe);
    let mut cancelled = large_budget()?;
    cancelled.request().cancel_handle().cancel();
    check!(matches!(
        run(&world, index, &request, &mut cancelled),
        Err(IndexError::Cancelled)
    ));
    // An exhausted budget refuses the request, never a shorter page.
    let mut tight = WorkBudget::new(
        WorkLimits {
            reads: 1_000,
            writes: 0,
            bytes: 64,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .ok_or(IndexError::ResourceLimited)?,
    );
    check!(matches!(
        run(&world, index, &request, &mut tight),
        Err(IndexError::ResourceLimited)
    ));
    Ok(())
}

#[test]
fn more_than_the_candidate_bound_is_refused_before_any_body_read() -> Result<(), IndexError> {
    let files: Vec<(Option<&str>, String, Vec<u8>)> = (0..4_097_usize)
        .map(|ordinal| {
            let body = if [17, 2_048, 4_000].contains(&ordinal) {
                "let haystack = needle;".to_owned()
            } else {
                format!("let value{ordinal} = common;")
            };
            (None, format!("src/f{ordinal:04}.rs"), body.into_bytes())
        })
        .collect();
    let mut world = world(&files)?;
    let _stale = take_reads(&world);
    let anchored = NativeTextIndex::new(world.fixture.recipe).with_literal_sibling(LITERAL_FORMAT);
    // The census of 4,097 admitted files is refused for every query kind.
    for (index, refused) in [
        (
            NativeTextIndex::new(world.fixture.recipe),
            literal("needle"),
        ),
        (NativeTextIndex::new(world.fixture.recipe), regex("needle")),
        // Configured, but no sibling is published in the view yet.
        (anchored, literal("needle")),
    ] {
        check!(matches!(
            run(
                &world,
                index,
                &request(&world, refused, None, 8),
                &mut large_budget()?
            ),
            Err(IndexError::ResourceLimited)
        ));
    }
    check_eq!(take_reads(&world), 0);

    publish_sibling(&mut world)?;
    // A byte-exact literal is anchored: only bodies holding its grams are read.
    let scanned = run(
        &world,
        anchored,
        &request(&world, literal("needle"), None, 8),
        &mut large_budget()?,
    )?;
    let paths: Vec<(String, u64, u64)> = scanned
        .page
        .matches
        .iter()
        .map(|row| (row.path.clone(), row.range.start, row.range.end))
        .collect();
    check_eq!(
        paths,
        vec![
            ("src/f0017.rs".to_owned(), 15, 21),
            ("src/f2048.rs".to_owned(), 15, 21),
            ("src/f4000.rs".to_owned(), 15, 21),
        ]
    );
    check!(scanned.page.completion == NativeTextMatchCompletion::Exhausted);
    let reads = take_reads(&world);
    check!(reads <= 3, "anchored literal read {reads} bodies");
    // A regex requiring the same literal is anchored the same way.
    let (rows, reads) = anchored_rows(&world, regex("needle"))?;
    check_eq!(
        rows,
        path_rows(&[
            ("src/f0017.rs", 15, 21),
            ("src/f2048.rs", 15, 21),
            ("src/f4000.rs", 15, 21),
        ])
    );
    check!(reads <= 3, "anchored regex read {reads} bodies");
    // A regex without a required literal keeps the census and is refused.
    check!(matches!(
        run(
            &world,
            anchored,
            &request(&world, regex("\\w+;"), None, 8),
            &mut large_budget()?
        ),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(take_reads(&world), 0);
    Ok(())
}

/// A published view over the candidate bound: filler files plus `marked`
/// bodies at fixed ordinals, with the Literal sibling over all of them.
fn bounded_out_world(marked: &[(usize, &str)]) -> Result<World, IndexError> {
    let files: Vec<(Option<&str>, String, Vec<u8>)> = (0..4_097_usize)
        .map(|ordinal| {
            let body = marked
                .iter()
                .find(|(at, _body)| *at == ordinal)
                .map_or_else(
                    || format!("let value{ordinal} = common;"),
                    |(_at, body)| (*body).to_owned(),
                );
            (None, format!("src/f{ordinal:04}.rs"), body.into_bytes())
        })
        .collect();
    let mut world = world(&files)?;
    publish_sibling(&mut world)?;
    let _stale = take_reads(&world);
    Ok(world)
}

/// Rows of one anchored page over a bounded-out view and its body reads.
fn anchored_rows(
    world: &World,
    query: NativeTextMatchQuery,
) -> Result<(Vec<(String, u64, u64)>, usize), IndexError> {
    let scanned = run(
        world,
        NativeTextIndex::new(world.fixture.recipe).with_literal_sibling(LITERAL_FORMAT),
        &request(world, query, None, 64),
        &mut large_budget()?,
    )?;
    check!(scanned.page.completion == NativeTextMatchCompletion::Exhausted);
    let rows = scanned
        .page
        .matches
        .iter()
        .map(|row| (row.path.clone(), row.range.start, row.range.end))
        .collect();
    Ok((rows, take_reads(world)))
}

fn path_rows(rows: &[(&str, u64, u64)]) -> Vec<(String, u64, u64)> {
    rows.iter()
        .map(|(path, start, end)| ((*path).to_owned(), *start, *end))
        .collect()
}

#[test]
fn insensitive_literals_are_anchored_by_simple_case_folding() -> Result<(), IndexError> {
    let world = bounded_out_world(&[
        (17, "let a = NEEDLE_K;"),
        (2_048, "let b = needle_\u{212A};"),
        (4_000, "let c = Needle_k + nEEDLE_\u{212A};"),
        // Decoys miss one window of the needle and are never read.
        (100, "let d = needle_x;"),
        (200, "let e = NEEDLE;"),
        (3_000, "let s = \"cafe\u{301}\";"),
    ])?;
    let folded = |text: &str| {
        query(
            NativeTextMatchPattern::Literal(text.into()),
            NativeTextMatchCase::Insensitive,
        )
    };
    let expected = path_rows(&[
        ("src/f0017.rs", 8, 16),
        ("src/f2048.rs", 8, 18),
        ("src/f4000.rs", 8, 16),
        ("src/f4000.rs", 19, 29),
    ]);
    for needle in ["needle_k", "NEEDLE_\u{212A}"] {
        let (rows, reads) = anchored_rows(&world, folded(needle))?;
        check_eq!(rows, expected);
        check!(reads <= 3, "folded {needle:?} read {reads} bodies");
    }
    // A byte-exact literal needs no NFC image: its raw bytes anchor it.
    let (rows, reads) = anchored_rows(&world, literal("cafe\u{301}"))?;
    check_eq!(rows, path_rows(&[("src/f3000.rs", 9, 15)]));
    check!(reads <= 1, "decomposed literal read {reads} bodies");
    Ok(())
}

/// Over every Unicode scalar: each character the regex translator makes a
/// case-insensitive literal character match is one of that character's
/// `SimpleCaseFold` variants. For every folded character the compiled engine
/// then matches exactly its variants among all folded characters.
#[test]
fn simple_case_fold_variants_cover_every_case_insensitive_match() -> Result<(), IndexError> {
    // A parser keeps position state between calls, so each scalar gets its
    // own, configured as the match engine configures its regex.
    let mut builder = regex_syntax::ParserBuilder::new();
    let _configured = builder.case_insensitive(true).utf8(true).unicode(true);
    let mut buffer = [0_u8; 4];
    let mut variants = Vec::new();
    let mut folded: Vec<(char, Vec<char>)> = Vec::new();
    for character in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
        let pattern = regex::escape(character.encode_utf8(&mut buffer));
        let hir = builder
            .build()
            .parse(&pattern)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let matched: Vec<char> = match hir.kind() {
            HirKind::Literal(literal) => std::str::from_utf8(&literal.0)
                .map_err(|_utf8| IndexError::BackendViolation("literal is not UTF-8".into()))?
                .chars()
                .collect(),
            HirKind::Class(Class::Unicode(class)) => class
                .iter()
                .flat_map(|range| range.start()..=range.end())
                .collect(),
            other => {
                return Err(IndexError::BackendViolation(format!(
                    "{character:?} translated to {other:?}"
                )));
            }
        };
        check!(SimpleCaseFold.expand(character, &mut variants));
        check!(variants.contains(&character));
        for scalar in &matched {
            check!(
                variants.contains(scalar),
                "{character:?} matches {scalar:?} outside {variants:?}"
            );
        }
        if matched.len() > 1 {
            folded.push((character, variants.clone()));
        }
    }
    check!(folded.len() > 2_000, "only {} folded scalars", folded.len());
    let haystack: String = folded.iter().map(|(character, _)| *character).collect();
    for (character, variants) in &folded {
        let engine = regex::RegexBuilder::new(&regex::escape(character.encode_utf8(&mut buffer)))
            .case_insensitive(true)
            .unicode(true)
            .build()
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let mut seen = Vec::new();
        for found in engine.find_iter(&haystack) {
            let mut scalars = found.as_str().chars();
            let (Some(scalar), None) = (scalars.next(), scalars.next()) else {
                return Err(IndexError::BackendViolation(
                    "a literal character matched several scalars".into(),
                ));
            };
            check!(
                variants.contains(&scalar),
                "{character:?} engine-matches {scalar:?} outside {variants:?}"
            );
            seen.push(scalar);
        }
        seen.sort_unstable();
        let mut expected = variants.clone();
        expected.sort_unstable();
        check!(seen == expected, "{character:?}: {seen:?} != {expected:?}");
    }
    // Fixed classes: Kelvin sign, long s, final sigma and the theta symbols.
    for (character, class) in [
        ('k', &['K', 'k', '\u{212A}'][..]),
        ('s', &['S', 's', '\u{17F}'][..]),
        ('ς', &['Σ', 'σ', 'ς'][..]),
        ('θ', &['Θ', 'θ', 'ϑ', 'ϴ'][..]),
        ('i', &['I', 'i'][..]),
        ('中', &['中'][..]),
    ] {
        check!(SimpleCaseFold.expand(character, &mut variants));
        variants.sort_unstable();
        let mut expected = class.to_vec();
        expected.sort_unstable();
        check!(
            variants == expected,
            "{character:?}: {variants:?} != {expected:?}"
        );
    }
    Ok(())
}

/// Required-literal anchoring of regexes over a view past the candidate
/// bound. Every expected row is the byte range `rg --json --no-config
/// --no-ignore --hidden --sort path [-i] -e <pattern>` reported over the same
/// 4,097 files written to disk (line `absolute_offset` plus submatch bounds).
#[test]
fn regexes_are_anchored_by_required_literals_like_ripgrep() -> Result<(), IndexError> {
    let coffee = "\u{2615}".repeat(40);
    let truncated = format!("{coffee}7\n");
    let trailing = format!("x{coffee}");
    let world = bounded_out_world(&[
        (11, "foo12bar foo bar foobar foo7bar\nxfoo3bar\n"),
        // A decoy holding the prefix literal but no match.
        (1_234, "food12bar\n"),
        (500, "alpha_x beta_x gamma_x alpha_y\nbeta_xx\n"),
        (900, "KELVIN  temp\n\u{212A}elvin\tscale\nkelvin\n"),
        (901, "kElViN x1"),
        (902, "Kelvin\r\n"),
        (3_000, &truncated),
        (3_001, &trailing),
    ])?;
    let folded = |text: &str| {
        query(
            NativeTextMatchPattern::Regex(text.into()),
            NativeTextMatchCase::Insensitive,
        )
    };
    let kelvin = path_rows(&[
        ("src/f0900.rs", 0, 12),
        ("src/f0900.rs", 13, 27),
        ("src/f0901.rs", 0, 9),
    ]);
    let cases = [
        // Prefix `foo` (inexact) is required.
        (
            regex("foo\\d+bar"),
            path_rows(&[
                ("src/f0011.rs", 0, 8),
                ("src/f0011.rs", 24, 31),
                ("src/f0011.rs", 33, 40),
            ]),
            2,
        ),
        // Exact alternatives `alpha_x` and `beta_x`.
        (
            regex("(alpha|beta)_x"),
            path_rows(&[
                ("src/f0500.rs", 0, 7),
                ("src/f0500.rs", 8, 14),
                ("src/f0500.rs", 31, 37),
            ]),
            1,
        ),
        // Inline folding expands to every raw case variant of `Kelvin`.
        (regex("(?i)Kelvin\\s+\\w+"), kelvin.clone(), 3),
        // The folded policy anchors one canonical member by its fold classes.
        (folded("kelvin\\s+\\w+"), kelvin, 3),
        // No prefix literal: the suffix `_xx` is required.
        (
            regex("[a-z]+_xx"),
            path_rows(&[("src/f0500.rs", 31, 38)]),
            1,
        ),
        // An empty literal set proves that nothing matches.
        (regex("[a&&b]"), Vec::new(), 0),
        // Members cut at the literal length limit are trimmed to whole
        // scalars, from the end for a prefix and from the start for a suffix.
        (
            regex(&format!("{coffee}\\d")),
            path_rows(&[("src/f3000.rs", 0, 121)]),
            2,
        ),
        (
            regex(&format!("\\w+{coffee}")),
            path_rows(&[("src/f3001.rs", 0, 121)]),
            2,
        ),
    ];
    for (query, expected, bound) in cases {
        let pattern = format!("{:?}", query.pattern);
        let (rows, reads) = anchored_rows(&world, query)?;
        check!(rows == expected, "{pattern}: {rows:?} != {expected:?}");
        check!(reads <= bound, "{pattern} read {reads} bodies");
    }
    // Without an anchorable required literal the census is refused before
    // any body is read.
    for refused in [regex("\\w+\\s+\\w+"), regex("\\d+"), regex("(?i)ab")] {
        check!(matches!(
            run(
                &world,
                NativeTextIndex::new(world.fixture.recipe).with_literal_sibling(LITERAL_FORMAT),
                &request(&world, refused, None, 8),
                &mut large_budget()?
            ),
            Err(IndexError::ResourceLimited)
        ));
    }
    check_eq!(take_reads(&world), 0);
    Ok(())
}

/// `len` pseudo-random `a`/`b` characters on one line.
fn ab_line(len: usize) -> String {
    let mut state = 0x2545_f491_u32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if state & 1 == 0 { 'a' } else { 'b' }
        })
        .collect()
}

/// Every live byte reserved through a metered budget.
struct Meter(AtomicU64);

impl MemoryLeaseOwner for Meter {
    fn release(&self, bytes: u64) {
        let _prior = self.0.fetch_sub(bytes, Ordering::SeqCst);
    }
}

struct MeterScope(Arc<Meter>);

impl MemoryScopePort for MeterScope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _prior = self.0.0.fetch_add(bytes, Ordering::SeqCst);
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

fn metered_budget() -> Result<(WorkBudget, Arc<Meter>), IndexError> {
    let meter = Arc::new(Meter(AtomicU64::new(0)));
    let mut budget = large_budget()?;
    budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(MeterScope(meter.clone())))));
    Ok((budget, meter))
}

/// After heavy searches an engine and its cache stay within their grant.
///
/// A lazy-DFA-thrashing regex and a long literal search a 1 MiB line, and a
/// Unicode word regex a long mixed line. The grant the compiled pattern holds
/// covers both, and it ends with the pattern.
#[test]
fn match_engine_and_search_cache_stay_within_their_grant() -> Result<(), IndexError> {
    let thrash = ab_line(1 << 20);
    let mut words = String::new();
    for line in 0..20_000 {
        write!(
            words,
            "pub fn step_{line}(budget) -> café_bytes {{ naïve_{line}ing }} "
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
    }
    for (query, line, thrashes) in [
        (regex("(?:a|b)*a(?:a|b){14}b"), &thrash, true),
        (literal("abbabaabbbab"), &thrash, false),
        (
            query(
                NativeTextMatchPattern::Regex(r"\b\w+ing\b".into()),
                NativeTextMatchCase::Insensitive,
            ),
            &words,
            false,
        ),
    ] {
        let label = format!("{:?}", query.pattern);
        let (mut budget, meter) = metered_budget()?;
        let mut pattern = compile_match_pattern(&query, &mut budget)?;
        check!(!pattern.line_matches_for_test(line)?.is_empty(), "{label}");
        let (engine, cache, granted) = pattern.heap_for_test();
        let held = u64::try_from(
            engine
                .checked_add(cache)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_width| IndexError::ResourceLimited)?;
        check!(
            held <= granted,
            "{label}: engine {engine} + cache {cache} > grant {granted}"
        );
        if thrashes {
            check!(
                cache.saturating_mul(2) > quanta_index_lq_regex::ENGINE_DFA_CACHE_LIMIT_BYTES,
                "{label}: cache {cache} never grew"
            );
        }
        // The engine grant is live with the pattern, beside the grants of
        // its required-literal sets.
        let live = meter.0.load(Ordering::SeqCst);
        check!(live >= granted, "{label}: {live} live < grant {granted}");
        drop(pattern);
        check_eq!(meter.0.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

/// Every refused compilation, by syntax, by line terminator, by byte class
/// or by the engine size limit, releases every grant it took.
#[test]
fn refused_compilations_release_every_grant() -> Result<(), IndexError> {
    for (refused, limited) in [
        (regex("("), false),
        (regex("a\\nb"), false),
        (regex("(?-u:\\xFF)"), false),
        (literal("a\nb"), false),
        (regex(r"[\x{80}-\x{10FFFF}]{100000}"), true),
    ] {
        let label = format!("{:?}", refused.pattern);
        let (mut budget, meter) = metered_budget()?;
        let outcome = compile_match_pattern(&refused, &mut budget);
        check!(
            matches!(
                (&outcome, limited),
                (Err(IndexError::ResourceLimited), true)
                    | (Err(IndexError::InvalidInput(_)), false)
            ),
            "{label}: {:?}",
            outcome.as_ref().err()
        );
        drop(outcome);
        let live = meter.0.load(Ordering::SeqCst);
        check!(live == 0, "{label}: {live} bytes still reserved");
    }
    Ok(())
}

/// Every page of `query` over `index`, continued by cursor with `limit`
/// rows per page, with each range proved against its own source bytes.
fn scan_all(
    world: &World,
    index: NativeTextIndex,
    query: &NativeTextMatchQuery,
    limit: u32,
) -> Result<Vec<Row>, IndexError> {
    let mut rows = Vec::new();
    let mut after = None;
    for _ in 0..1_000 {
        let scanned = run(
            world,
            index,
            &request(world, query.clone(), after, limit),
            &mut large_budget()?,
        )?;
        for row in &scanned.page.matches {
            let body = world
                .owners
                .iter()
                .position(|owner| *owner == row.native_source.owner)
                .and_then(|ordinal| world.bodies.get(ordinal))
                .ok_or_else(|| IndexError::BackendViolation("row names an unknown owner".into()))?;
            let text = std::str::from_utf8(body).map_err(|_utf8| {
                IndexError::BackendViolation("fixture body is not UTF-8".into())
            })?;
            let start =
                usize::try_from(row.range.start).map_err(|_width| IndexError::ResourceLimited)?;
            let end =
                usize::try_from(row.range.end).map_err(|_width| IndexError::ResourceLimited)?;
            check!(text.get(start..end).is_some_and(|found| !found.is_empty()));
            rows.push((
                row.repository.as_ref().map(|name| name.as_str().to_owned()),
                row.path.clone(),
                row.range.start,
                row.range.end,
            ));
        }
        match scanned.page.completion {
            NativeTextMatchCompletion::Exhausted => return Ok(rows),
            NativeTextMatchCompletion::More => after = scanned.page.next.clone(),
        }
        check!(after.is_some(), "More without a cursor");
    }
    Err(IndexError::BackendViolation("pages never ended".into()))
}

/// The contract restated naively: split each body at `\n`, run the `regex`
/// crate's `find_iter` on every line and keep the non-empty matches, in
/// `(repository, path, start)` order.
fn oracle_rows(
    files: &[(Option<&str>, String, Vec<u8>)],
    query: &NativeTextMatchQuery,
) -> Result<Vec<Row>, IndexError> {
    let source = match &query.pattern {
        NativeTextMatchPattern::Literal(text) => regex::escape(text),
        NativeTextMatchPattern::Regex(text) => text.clone(),
    };
    let oracle = regex::RegexBuilder::new(&source)
        .case_insensitive(query.case == NativeTextMatchCase::Insensitive)
        .build()
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let mut rows = Vec::new();
    for (repository, path, body) in files {
        let text = std::str::from_utf8(body)
            .map_err(|_utf8| IndexError::BackendViolation("fixture body is not UTF-8".into()))?;
        let mut offset = 0_usize;
        for line in text.split('\n') {
            for found in oracle.find_iter(line) {
                if !found.is_empty() {
                    rows.push((
                        repository.map(str::to_owned),
                        path.clone(),
                        u64::try_from(offset.saturating_add(found.start()))
                            .map_err(|_width| IndexError::ResourceLimited)?,
                        u64::try_from(offset.saturating_add(found.end()))
                            .map_err(|_width| IndexError::ResourceLimited)?,
                    ));
                }
            }
            offset = offset.saturating_add(line.len()).saturating_add(1);
        }
    }
    rows.sort_by(|left, right| (&left.0, &left.1, left.2).cmp(&(&right.0, &right.1, right.2)));
    Ok(rows)
}

/// Line text units with every line-splitting and folding hazard: `\r`, CRLF,
/// empty lines, Kelvin sign, sharp s, long s, a precomposed and a decomposed
/// é, dotted and dotless i, and Greek sigma forms.
const UNITS: [&str; 21] = [
    "a", "b", "ab", "K", "k", "\u{212A}", "é", "e\u{301}", "ß", "ſ", "S", "\r", "\n", "\n", "\r\n",
    " ", "_", "\u{130}", "\u{131}", "Σ", "ς",
];

/// One `(repository, path, body)` fixture file.
type FileSpec = (Option<&'static str>, String, Vec<u8>);

fn random_files(rng: &mut Rng) -> Result<Vec<FileSpec>, IndexError> {
    let mut files = Vec::new();
    for ordinal in 0..=rng.below(5)? {
        let repository = [None, Some("corp-a"), Some("corp-b")]
            .get(rng.below(3)?)
            .copied()
            .flatten();
        let mut body = String::new();
        for _ in 0..rng.below(40)? {
            body.push_str(UNITS.get(rng.below(UNITS.len())?).copied().unwrap_or("a"));
        }
        files.push((repository, format!("src/f{ordinal}.rs"), body.into_bytes()));
    }
    Ok(files)
}

fn random_query(rng: &mut Rng) -> Result<NativeTextMatchQuery, IndexError> {
    const ATOMS: [&str; 16] = [
        "a", "b", "K", "é", "ß", "S", r"\w", r"\s", r"\r", "[ab]", "[^a]", ".", "(?:ab|b)", "^",
        "$", r"\b",
    ];
    const REPEATS: [&str; 5] = ["", "", "*", "+", "?"];
    let case = if rng.below(2)? == 0 {
        NativeTextMatchCase::Sensitive
    } else {
        NativeTextMatchCase::Insensitive
    };
    let mut text = String::new();
    let pattern = if rng.below(3)? == 0 {
        for _ in 0..=rng.below(3)? {
            // A literal never holds a line terminator.
            let unit = UNITS.get(rng.below(UNITS.len())?).copied().unwrap_or("a");
            text.push_str(if unit.contains('\n') { "a" } else { unit });
        }
        NativeTextMatchPattern::Literal(text)
    } else {
        for _ in 0..=rng.below(3)? {
            let atom = ATOMS.get(rng.below(ATOMS.len())?).copied().unwrap_or("a");
            text.push_str(atom);
            if !matches!(atom, "^" | "$" | r"\b") {
                text.push_str(
                    REPEATS
                        .get(rng.below(REPEATS.len())?)
                        .copied()
                        .unwrap_or(""),
                );
            }
        }
        NativeTextMatchPattern::Regex(text)
    };
    Ok(query(pattern, case))
}

/// On random small corpora and patterns, every paged scan, by census and
/// anchored by the Literal sibling, equals the naive line-by-line oracle.
#[test]
fn match_scan_equals_a_line_by_line_regex_oracle() -> Result<(), IndexError> {
    let mut rng = Rng(0x0dd5_ca1a_b1ef_00d5);
    let mut nonempty = 0_usize;
    for _ in 0..8 {
        let files = random_files(&mut rng)?;
        let mut world = world(&files)?;
        publish_sibling(&mut world)?;
        for _ in 0..24 {
            let query = random_query(&mut rng)?;
            let label = format!("{:?} {:?}", query.pattern, query.case);
            let expected = oracle_rows(&files, &query)?;
            let limit =
                u32::try_from(1 + rng.below(4)?).map_err(|_width| IndexError::ResourceLimited)?;
            for index in [
                NativeTextIndex::new(world.fixture.recipe),
                NativeTextIndex::new(world.fixture.recipe).with_literal_sibling(LITERAL_FORMAT),
            ] {
                let rows = scan_all(&world, index, &query, limit)?;
                check!(
                    rows == expected,
                    "{label} limit {limit}: {rows:?} != {expected:?}"
                );
            }
            nonempty = nonempty.saturating_add(usize::from(!expected.is_empty()));
        }
    }
    check!(nonempty > 60, "only {nonempty} queries matched anything");
    Ok(())
}

/// A scope that refuses every reservation.
struct RefuseAll;

impl MemoryScopePort for RefuseAll {
    fn reserve(&self, _bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        Err(IndexError::ResourceLimited)
    }
}

/// A pattern is parsed only under its parse grant: when no grant is
/// admitted the compilation is refused before the parser runs, so even a
/// syntax error reports the refusal.
#[test]
fn pattern_parse_waits_for_its_grant() -> Result<(), IndexError> {
    for pattern in [regex("("), regex("a+b"), literal("needle")] {
        let label = format!("{:?}", pattern.pattern);
        let mut budget = large_budget()?;
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(RefuseAll))));
        let outcome = compile_match_pattern(&pattern, &mut budget);
        check!(
            matches!(outcome, Err(IndexError::ResourceLimited)),
            "{label}: {:?}",
            outcome.as_ref().err()
        );
    }
    Ok(())
}
