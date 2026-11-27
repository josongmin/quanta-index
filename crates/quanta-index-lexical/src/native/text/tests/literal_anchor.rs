//! `file.contains` anchored on the pinned Literal sibling.
//!
//! Every expected owner below follows from the fixed bodies alone; read
//! counts follow from which bodies hold every gram of the needle.

use super::super::super::literal_blocks::{
    KEY_BYTES, LiteralBlockBuilder, MembershipCache, PushResult, literal_key,
};
use super::super::super::literal_packed::PackedRoot;
use super::*;
use quanta_index_contract::parse_native_content_query;
use quanta_index_core::index::{IndexInstanceId, InputBinding, SchemaId, StorageFormatId};
use std::collections::BTreeSet;

pub(super) const LITERAL_FORMAT: StorageFormatId = StorageFormatId([0x4c; 16]);
pub(super) const LITERAL_INSTANCE: IndexInstanceId = IndexInstanceId([0x4d; 16]);

pub(super) struct Corpus {
    pub(super) fixture: VisitorFixture,
    pub(super) owners: Vec<OwnerKey>,
    pub(super) bodies: Vec<Vec<u8>>,
}

impl Corpus {
    pub(super) fn owner(&self, ordinal: usize) -> Result<OwnerKey, IndexError> {
        self.owners
            .get(ordinal)
            .copied()
            .ok_or_else(|| IndexError::BackendViolation("fixture owner ordinal".into()))
    }
}

pub(super) fn large_budget() -> Result<WorkBudget, IndexError> {
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
    digest[1] = high;
    digest[2] = low;
    Ok(ObjectRef(Digest(digest)))
}

fn digest_ref(payload: &[u8]) -> ObjectRef {
    ObjectRef(Digest(Sha256::digest(payload).into()))
}

/// A real segmented Text member over `bodies`, one source each.
pub(super) fn corpus(bodies: &[Vec<u8>]) -> Result<Corpus, IndexError> {
    let docs: Vec<(String, Vec<u8>)> = bodies
        .iter()
        .enumerate()
        .map(|(ordinal, body)| (format!("src/f{ordinal:04}.rs"), body.clone()))
        .collect();
    corpus_with_paths(&docs)
}

/// A real segmented Text member over `(path, body)` sources, with a legacy
/// owner marker per path in the pinned Source path map.
pub(super) fn corpus_with_paths(docs: &[(String, Vec<u8>)]) -> Result<Corpus, IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.recipe = TextRecipe::segmented_full(fixture.recipe.admission_policy);
    fixture.objects.generated = Some(Mutex::default());
    let source_root = fixture.ticket.view.source.owners;
    let source_key = source_root
        .0
        .ok_or_else(|| IndexError::BackendViolation("fixture source root missing".into()))?;
    let _previous = fixture.maps.roots.insert(source_key, BTreeMap::new());
    let mut owners = Vec::new();
    let mut tracked = BTreeSet::new();
    let paths = MapRoot(Some(tagged_ref(0xe0, 0)?));
    for (ordinal, (path, body)) in docs.iter().enumerate() {
        let [high, low] = u16::try_from(ordinal)
            .map_err(|_width| IndexError::ResourceLimited)?
            .to_be_bytes();
        let mut stable = [0x5a; 16];
        stable[0] = high;
        stable[1] = low;
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
            source_repo_id: None,
            language: None,
        };
        fixture.objects.add(body_ref, body.clone(), Vec::new())?;
        fixture
            .objects
            .add(source_ref, encode(&record)?, vec![body_ref])?;
        fixture
            .maps
            .add(source_root, owner_map_key(owner).to_vec(), source_ref)?;
        let mut path_key = b"path\0".to_vec();
        path_key.extend_from_slice(path.as_bytes());
        fixture.maps.add(
            paths,
            path_key,
            quanta_index_core::index::path_owner_map_value(owner)?,
        )?;
        owners.push(owner);
        let _inserted = tracked.insert(body_ref);
    }
    // The Source cut, including its path map, is fixed before the build.
    fixture.ticket.view.source.paths = paths;
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
    Ok(Corpus {
        fixture,
        owners,
        bodies: docs.iter().map(|(_path, body)| body.clone()).collect(),
    })
}

fn owner_bytes(owner: OwnerKey) -> [u8; 32] {
    let mut key = [0_u8; 32];
    for (target, byte) in key
        .iter_mut()
        .zip(owner.producer.0.iter().chain(owner.stable_id.0.iter()))
    {
        *target = *byte;
    }
    key
}

/// One Literal membership: a raw byte trigram and its owner key.
type GramKey = ([u8; 3], [u8; 32]);

/// Every distinct raw byte trigram of every body, keyed `(gram, owner)`.
fn gram_keys(corpus: &Corpus) -> Result<Vec<GramKey>, IndexError> {
    let mut keys = BTreeSet::new();
    for (owner, body) in corpus.owners.iter().zip(&corpus.bodies) {
        for window in body.windows(3) {
            let gram = <[u8; 3]>::try_from(window).map_err(|_width| IndexError::ResourceLimited)?;
            let _inserted = keys.insert((gram, owner_bytes(*owner)));
        }
    }
    Ok(keys.into_iter().collect())
}

#[derive(Clone, Copy)]
pub(super) enum Layout {
    Packed,
    Tree,
}

/// Publishes a Literal member for the corpus's Source cut under `instance`.
/// `input_root` differs from the view's owners only in mismatch fixtures.
pub(super) fn publish_sibling(
    corpus: &mut Corpus,
    layout: Layout,
    instance: IndexInstanceId,
    format: StorageFormatId,
    input_root: MapRoot,
    tag: u8,
) -> Result<(), IndexError> {
    let keys = gram_keys(corpus)?;
    let mut work = large_budget()?;
    let fixture = &mut corpus.fixture;
    let sources = fixture.ticket.view.source.owners;
    let (payload, children) = match layout {
        Layout::Packed => {
            let blocks = MapRoot(Some(tagged_ref(tag, 0)?));
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
            (root.fixture_payload()?, root.children().as_slice().to_vec())
        }
        Layout::Tree => {
            let grams = MapRoot(Some(tagged_ref(tag, 0)?));
            let owners = MapRoot(Some(tagged_ref(tag, 1)?));
            let mut postings: BTreeMap<[u8; 3], Vec<[u8; 32]>> = BTreeMap::new();
            for (gram, owner) in &keys {
                postings.entry(*gram).or_default().push(*owner);
            }
            for (ordinal, (gram, members)) in postings.iter().enumerate() {
                let root = MapRoot(Some(tagged_ref(tag.wrapping_add(1), ordinal)?));
                for owner in members {
                    fixture.maps.add(root, owner.to_vec(), reference(0x77))?;
                }
                let payload = super::super::super::literal::tree_fixture_posting(
                    root,
                    u64::try_from(members.len()).map_err(|_width| IndexError::ResourceLimited)?,
                )?;
                let posting = digest_ref(&payload);
                let root_ref = root
                    .0
                    .ok_or_else(|| IndexError::BackendViolation("posting root".into()))?;
                fixture.objects.add(posting, payload, vec![root_ref])?;
                fixture.maps.add(grams, gram.to_vec(), posting)?;
            }
            // The owner map is only an artifact edge for these reads.
            fixture.maps.add(owners, vec![0; 32], reference(0x77))?;
            let payload = super::super::super::literal::tree_fixture_payloads(owners, grams)?;
            (payload, [owners.0, grams.0].into_iter().flatten().collect())
        }
    };
    let artifact = digest_ref(&payload);
    let logical = Digest(Sha256::digest(&payload).into());
    fixture.objects.add(artifact, payload, children)?;
    let member = MemberBinding {
        instance,
        format,
        artifact,
        input: InputBinding {
            root: input_root,
            schema: SchemaId([0x4e; 16]),
            policy: Digest([0x4f; 32]),
            context: Digest([0x50; 32]),
        },
        logical_commitment: logical,
    };
    let member_payload = encode(&member)?;
    let member_ref = digest_ref(&member_payload);
    fixture.objects.add(
        member_ref,
        member_payload,
        [Some(artifact), input_root.0]
            .into_iter()
            .flatten()
            .collect(),
    )?;
    fixture.maps.add(
        fixture.ticket.view.members,
        member_map_key(instance).to_vec(),
        member_ref,
    )?;
    Ok(())
}

pub(super) fn search(
    corpus: &Corpus,
    index: NativeTextIndex,
    query: &str,
    top_k: u32,
) -> Result<crate::native::text_query::NativeTextRanking, IndexError> {
    search_with_budget(corpus, index, query, top_k, &mut large_budget()?)
}

fn search_with_budget(
    corpus: &Corpus,
    index: NativeTextIndex,
    query: &str,
    top_k: u32,
    budget: &mut WorkBudget,
) -> Result<crate::native::text_query::NativeTextRanking, IndexError> {
    let parsed = parse_native_content_query(query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let plan = crate::planner::LexicalPlanner::plan(&parsed)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let fixture = &corpus.fixture;
    index.search_content(
        &fixture.member,
        &plan,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
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

pub(super) fn anchored_index(corpus: &Corpus) -> NativeTextIndex {
    NativeTextIndex::new(corpus.fixture.recipe).with_literal_sibling(LITERAL_FORMAT)
}

pub(super) fn take_reads(corpus: &Corpus) -> usize {
    corpus
        .fixture
        .objects
        .tracked_body_reads
        .swap(0, Ordering::Relaxed)
}

/// Matched owners, sorted, and the source bodies read to answer.
pub(super) fn answer(
    corpus: &Corpus,
    index: NativeTextIndex,
    query: &str,
) -> Result<(Vec<OwnerKey>, usize), IndexError> {
    let _stale = take_reads(corpus);
    let ranking = search(corpus, index, query, 64).map_err(|error| {
        IndexError::BackendViolation(format!("{query} was not answered: {error:?}"))
    })?;
    let mut owners: Vec<OwnerKey> = ranking.hits.iter().map(|hit| hit.owner).collect();
    check_eq!(
        ranking.matched,
        u64::try_from(owners.len()).map_err(|_width| IndexError::ResourceLimited)?
    );
    owners.sort_unstable();
    Ok((owners, take_reads(corpus)))
}

const NEEDLE_DOCS: [usize; 3] = [17, 2_048, 4_000];

fn large_bodies() -> Vec<Vec<u8>> {
    (0..4_097_usize)
        .map(|ordinal| {
            if NEEDLE_DOCS.contains(&ordinal) {
                b"let haystack = needle;".to_vec()
            } else {
                format!("let value{ordinal} = common;").into_bytes()
            }
        })
        .collect()
}

fn funded_large_budget() -> Result<WorkBudget, IndexError> {
    let memory = Arc::new(TokenMemory {
        ceiling: 1 << 32,
        ..TokenMemory::default()
    });
    let mut work = large_budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BatchOutputMemoryScope(memory),
    ))));
    Ok(work)
}

#[test]
fn packed_literal_membership_reads_blocks_not_owners() -> Result<(), IndexError> {
    let mut corpus = corpus(&vec![b"common".to_vec(); 128])?;
    let sources = corpus.fixture.ticket.view.source.owners;
    let tag = 0xd0;
    publish_sibling(
        &mut corpus,
        Layout::Packed,
        LITERAL_INSTANCE,
        LITERAL_FORMAT,
        sources,
        tag,
    )?;
    corpus.fixture.objects.tracked_bodies = corpus
        .fixture
        .maps
        .roots
        .get(&tagged_ref(tag, 0)?)
        .ok_or_else(|| IndexError::BackendViolation("literal block directory absent".into()))?
        .values()
        .map(|value| value.object_ref())
        .collect::<Result<_, _>>()?;

    let index = anchored_index(&corpus);
    let query = "file.contains('common') case:yes";
    let uncached = search_with_budget(&corpus, index, query, 4, &mut large_budget()?)?;
    let uncached_reads = take_reads(&corpus);
    let mut work = funded_large_budget()?;
    let cached = search_with_budget(&corpus, index, query, 4, &mut work)?;
    let cached_reads = take_reads(&corpus);
    check_eq!(cached.matched, 128);
    check_eq!(cached.hits, uncached.hits);
    check!(uncached_reads > 0, "fixture did not reach packed objects");
    // Owners are probed in ascending order through one merge-join probe per
    // gram, which authenticates each of its blocks at most once whether or
    // not the request has spare memory. A per-owner lookup reads a block per
    // owner and gram, so its reads grow with the 128 matching owners.
    check_eq!(cached_reads, uncached_reads);
    check!(
        uncached_reads < 128,
        "{uncached_reads} packed reads for 128 matching owners"
    );
    Ok(())
}

#[test]
fn literal_anchor_answers_rare_needle_over_a_large_view() -> Result<(), IndexError> {
    let mut bodies = large_bodies();
    let [first, second, third] = NEEDLE_DOCS;
    *bodies.get_mut(first).ok_or(IndexError::Conflict)? =
        b"let haystack = needle common rare;".to_vec();
    *bodies.get_mut(second).ok_or(IndexError::Conflict)? =
        b"let haystack = needle common;".to_vec();
    *bodies.get_mut(third).ok_or(IndexError::Conflict)? = b"let haystack = needle filler;".to_vec();
    let mut corpus = corpus(&bodies)?;
    let expected: Vec<OwnerKey> = NEEDLE_DOCS
        .iter()
        .map(|ordinal| corpus.owner(*ordinal))
        .collect::<Result<_, _>>()?;
    let sources = corpus.fixture.ticket.view.source.owners;
    let plain = NativeTextIndex::new(corpus.fixture.recipe);

    // No sibling is configured: today's census refusal over 4,097 docs.
    check!(matches!(
        search(&corpus, plain, "file.contains('needle')", 8),
        Err(IndexError::ResourceLimited)
    ));
    // Configured but absent from the view: still not anchorable.
    check!(matches!(
        search(
            &corpus,
            anchored_index(&corpus),
            "file.contains('needle')",
            8
        ),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(take_reads(&corpus), 0);

    publish_sibling(
        &mut corpus,
        Layout::Packed,
        LITERAL_INSTANCE,
        LITERAL_FORMAT,
        sources,
        0xd0,
    )?;
    // DF 3: every candidate holds all needle grams, so at most 3 bodies.
    let (owners, reads) = answer(&corpus, anchored_index(&corpus), "file.contains('needle')")?;
    check_eq!(owners, expected);
    check!(reads <= 3, "anchored needle read {reads} bodies");
    let mut work = funded_large_budget()?;
    let broad = search_with_budget(
        &corpus,
        anchored_index(&corpus),
        "file.contains('common') OR file.contains('filler')",
        4,
        &mut work,
    );
    check!(matches!(broad, Err(IndexError::ResourceLimited)));
    check_eq!(work.first_denial(), None);
    check_eq!(take_reads(&corpus), 0);
    // Test both input orders with a one-owner supplier and with a three-owner
    // supplier. Only two of the three needle owners also contain `common`.
    let one = expected.get(..1).ok_or(IndexError::Conflict)?;
    let two = expected.get(..2).ok_or(IndexError::Conflict)?;
    for (query, expected, reads) in [
        ("file.contains('common') AND file.contains('rare')", one, 1),
        ("file.contains('rare') AND file.contains('common')", one, 1),
        (
            "file.contains('common') AND file.contains('needle')",
            two,
            3,
        ),
        (
            "file.contains('needle') AND file.contains('common')",
            two,
            3,
        ),
    ] {
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 512,
                writes: 0,
                bytes: 512 * 1024 * 1024,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(30),
        );
        let ranking = search_with_budget(&corpus, anchored_index(&corpus), query, 4, &mut work)
            .map_err(|error| {
                IndexError::BackendViolation(format!(
                    "{query}: {error:?}; work={:?} denial={:?}",
                    work.usage(),
                    work.first_denial()
                ))
            })?;
        check_eq!(
            ranking.matched,
            u64::try_from(expected.len()).map_err(|_width| IndexError::ResourceLimited)?
        );
        check_eq!(
            ranking.hits.iter().map(|hit| hit.owner).collect::<Vec<_>>(),
            expected
        );
        check_eq!(take_reads(&corpus), reads);
    }
    // Only the newly added overlay matches both terms. The unselected
    // conjunct still filters overlay-only rows and masked durable hits.
    {
        use super::super::super::text_overlay::TextOverlayInput;
        use super::super::super::text_query::{
            Bm25Policy, NativeTextHitSource, TextContentExecution, TextOverlaySource,
        };

        let first_owner = corpus.owner(first)?;
        let second_owner = corpus.owner(second)?;
        let added = OwnerKey {
            producer: first_owner.producer,
            stable_id: StableOwnerId([0xff; 16]),
        };
        let first_path = format!("src/f{first:04}.rs");
        let sources = [
            TextOverlaySource {
                owner: first_owner,
                revision: 1,
                path: &first_path,
                body: "needle",
                repository: None,
                language: None,
            },
            TextOverlaySource {
                owner: added,
                revision: 1,
                path: "src/added.rs",
                body: "common needle",
                repository: None,
                language: None,
            },
        ];
        let inputs = [
            TextOverlayInput {
                owner: first_owner,
                revision: 1,
                text: Some("needle"),
            },
            TextOverlayInput {
                owner: second_owner,
                revision: 1,
                text: None,
            },
            TextOverlayInput {
                owner: added,
                revision: 1,
                text: Some("common needle"),
            },
        ];
        let fixture = &corpus.fixture;
        let index = anchored_index(&corpus);
        let mut work = large_budget()?;
        let mut context = TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut work,
        };
        let overlay = index.prepare_overlay_corpus(&fixture.member, &inputs, &mut context)?;
        let parsed =
            parse_native_content_query("file.contains('common') AND file.contains('needle')")
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let plan = crate::planner::LexicalPlanner::plan(&parsed)
            .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let ranking = index.search_content_with_overlay(
            &fixture.member,
            &plan,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            TextContentExecution {
                top_k: 4,
                policy: Bm25Policy { k1: 1.2, b: 0.75 },
                rank_after: None,
            },
            Some((&overlay, &sources)),
            context,
        )?;
        check_eq!(ranking.matched, 1);
        check_eq!(
            ranking.hits.first().map(|hit| (hit.owner, hit.source)),
            Some((added, NativeTextHitSource::Overlay { revision: 1 }))
        );
    }
    // Case-folded variants of the same needle reach the same owners.
    let (owners, reads) = answer(&corpus, anchored_index(&corpus), "file.contains('NEEDLE')")?;
    check_eq!(owners, expected);
    check!(reads <= 3);
    // An anchored contains supplies candidates inside a disjunction.
    let (owners, _reads) = answer(
        &corpus,
        anchored_index(&corpus),
        "file.contains('needle') OR file.contains('haystack')",
    )?;
    check_eq!(owners, expected);
    // A needle shorter than one gram stays unanchored and is refused.
    check!(matches!(
        search(&corpus, anchored_index(&corpus), "file.contains('ne')", 8),
        Err(IndexError::ResourceLimited)
    ));
    // A gram shared by every body cannot anchor.
    check!(matches!(
        search(&corpus, anchored_index(&corpus), "file.contains('let')", 8),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(take_reads(&corpus), 0);

    // A second member in the same format makes the sibling ambiguous.
    publish_sibling(
        &mut corpus,
        Layout::Packed,
        IndexInstanceId([0x4e; 16]),
        LITERAL_FORMAT,
        sources,
        0xd4,
    )?;
    check!(matches!(
        search(
            &corpus,
            anchored_index(&corpus),
            "file.contains('needle')",
            8
        ),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(take_reads(&corpus), 0);
    Ok(())
}

#[test]
fn packed_membership_cache_reselects_descending_fence_and_binds_root() -> Result<(), IndexError> {
    fn insert_block(
        objects: &mut TestObjects,
        maps: &mut TestMaps,
        root: MapRoot,
        keys: &[[u8; KEY_BYTES]],
        work: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let mut builder = LiteralBlockBuilder::new(work, None)?;
        for key in keys {
            check_eq!(builder.try_push(*key, work)?, PushResult::Added);
        }
        let block = builder.finish()?;
        let last = block.last_key;
        let (input, _memory) = block.into_batch_input();
        let reference = digest_ref(&input.payload);
        objects.add(reference, input.payload, Vec::new())?;
        maps.add(root, last.to_vec(), reference)?;
        Ok(())
    }

    let key = |ordinal: u8| {
        let mut owner = [0; 32];
        owner[31] = ordinal;
        literal_key(*b"abc", owner)
    };
    let (mut work, memory) = funded_batch_budget(1_000_000)?;
    let mut objects = TestObjects::default();
    let mut maps = TestMaps::default();
    let first_root = MapRoot(Some(tagged_ref(0xd8, 0)?));
    let other_root = MapRoot(Some(tagged_ref(0xd9, 0)?));
    insert_block(
        &mut objects,
        &mut maps,
        first_root,
        &[key(1), key(2)],
        &mut work,
    )?;
    // Defensive non-disjoint fixture: the later block starts at 1, but the
    // canonical directory lower bound for 2 is still the earlier fence at 2.
    insert_block(
        &mut objects,
        &mut maps,
        first_root,
        &[key(1), key(4), key(5)],
        &mut work,
    )?;
    // The same target is absent in this independent root; its next fence is 5.
    insert_block(&mut objects, &mut maps, other_root, &[key(5)], &mut work)?;

    let mut cache = MembershipCache::default();
    let before = work.usage().reads;
    check!(cache.contains(&maps, &objects, first_root, key(4), &mut work)?);
    check_eq!(work.usage().reads - before, 2); // one directory + one object
    let before = work.usage().reads;
    check!(cache.contains(&maps, &objects, first_root, key(5), &mut work)?);
    check_eq!(work.usage().reads - before, 1); // authenticated block reuse

    // Target 2 is inside the cached block's first..last, but only the
    // earlier directory fence is authoritative for this descending probe.
    let before = work.usage().reads;
    check!(cache.contains(&maps, &objects, first_root, key(2), &mut work)?);
    check_eq!(work.usage().reads - before, 2);

    // A block from first_root cannot answer an overlapping key in other_root.
    let before = work.usage().reads;
    check!(!cache.contains(&maps, &objects, other_root, key(4), &mut work)?);
    check_eq!(work.usage().reads - before, 2);
    drop(cache);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn literal_sibling_must_bind_the_view_source_cut() -> Result<(), IndexError> {
    let mut corpus = corpus(&large_bodies())?;
    // A sibling over another Source cut is never used.
    publish_sibling(
        &mut corpus,
        Layout::Packed,
        LITERAL_INSTANCE,
        LITERAL_FORMAT,
        MapRoot(Some(reference(0x99))),
        0xd0,
    )?;
    check!(matches!(
        search(
            &corpus,
            anchored_index(&corpus),
            "file.contains('needle')",
            8
        ),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(take_reads(&corpus), 0);
    Ok(())
}

const CONTEXT_WITH: usize = 0;
const CONTEXT_UPPER: usize = 1;
const CONTEXT_LOWER: usize = 2;
const KELVIN: usize = 4;
const GREEK_QUESTION: usize = 5;
const DECOMPOSED_CAFE: usize = 6;
const GREEK_WORD: usize = 7;
const DOTTED_I: usize = 8;
const DECOMPOSED_XYZ: usize = 9;
const HAN: usize = 10;

fn unicode_bodies() -> Vec<Vec<u8>> {
    [
        "fn WithContext() {}",
        "CONTEXT = 1",
        "the context here",
        "unrelated text",
        "temperature 3\u{212A}elvin",
        "a\u{37E}b x",
        "cafe\u{301} menu",
        "ΟΔΟΣ road",
        "abİ tail",
        "e\u{301}xyz",
        "中文字 sample",
    ]
    .iter()
    .map(|body| body.as_bytes().to_vec())
    .collect()
}

#[test]
fn literal_anchor_preserves_case_and_normalization_semantics() -> Result<(), IndexError> {
    for layout in [Layout::Packed, Layout::Tree] {
        let mut corpus = corpus(&unicode_bodies())?;
        let sources = corpus.fixture.ticket.view.source.owners;
        publish_sibling(
            &mut corpus,
            layout,
            LITERAL_INSTANCE,
            LITERAL_FORMAT,
            sources,
            0xd0,
        )?;
        let index = anchored_index(&corpus);
        let owners = |ordinals: &[usize]| -> Result<Vec<OwnerKey>, IndexError> {
            let mut owners = ordinals
                .iter()
                .map(|ordinal| corpus.owner(*ordinal))
                .collect::<Result<Vec<_>, _>>()?;
            owners.sort_unstable();
            Ok(owners)
        };
        let admitted = corpus.owners.len();

        // case:no reaches every case variant; each is a true match.
        let (found, reads) = answer(&corpus, index, "file.contains('Context')")?;
        check_eq!(
            found,
            owners(&[CONTEXT_WITH, CONTEXT_UPPER, CONTEXT_LOWER])?
        );
        check_eq!(reads, 3);
        let (found, reads) = answer(&corpus, index, "file.contains('Context') case:yes")?;
        check_eq!(found, owners(&[CONTEXT_WITH])?);
        check_eq!(reads, 1);
        // KELVIN SIGN normalizes to K: found folded and sensitive.
        let (found, reads) = answer(&corpus, index, "file.contains('kelvin')")?;
        check_eq!(found, owners(&[KELVIN])?);
        check_eq!(reads, 1);
        let (found, _reads) = answer(&corpus, index, "file.contains('3Kelvin') case:yes")?;
        check_eq!(found, owners(&[KELVIN])?);
        // GREEK QUESTION MARK normalizes to a semicolon.
        let (found, reads) = answer(&corpus, index, "file.contains('a;b') case:yes")?;
        check_eq!(found, owners(&[GREEK_QUESTION])?);
        check_eq!(reads, 1);
        // A decomposed é is anchored only on its safe window `caf`.
        let (found, reads) = answer(&corpus, index, "file.contains('café menu')")?;
        check_eq!(found, owners(&[DECOMPOSED_CAFE])?);
        check_eq!(reads, 1);
        let (found, reads) = answer(&corpus, index, "file.contains('éxyz')")?;
        check_eq!(found, owners(&[DECOMPOSED_XYZ])?);
        check_eq!(reads, 1);
        // Uncased and cased non-ASCII letters.
        let (found, reads) = answer(&corpus, index, "file.contains('οδοσ')")?;
        check_eq!(found, owners(&[GREEK_WORD])?);
        check_eq!(reads, 1);
        let (found, reads) = answer(&corpus, index, "file.contains('中文字')")?;
        check_eq!(found, owners(&[HAN])?);
        check_eq!(reads, 1);
        // Folded `i` can come from İ: no safe window, so the census answers.
        let (found, reads) = answer(&corpus, index, "file.contains('abi')")?;
        check_eq!(found, owners(&[DOTTED_I])?);
        check_eq!(reads, admitted);
        // Absent grams: an empty anchor answers without reading a body.
        let (found, reads) = answer(&corpus, index, "file.contains('zzqq')")?;
        check!(found.is_empty());
        check_eq!(reads, 0);
    }
    Ok(())
}

/// A bound memory scope refuses zero-byte reservations, as production does.
/// An empty anchor must therefore reserve nothing for its candidate rows.
#[test]
fn empty_literal_anchor_reserves_no_memory_under_a_bound_scope() -> Result<(), IndexError> {
    let mut corpus = corpus(&unicode_bodies())?;
    let sources = corpus.fixture.ticket.view.source.owners;
    publish_sibling(
        &mut corpus,
        Layout::Packed,
        LITERAL_INSTANCE,
        LITERAL_FORMAT,
        sources,
        0xd0,
    )?;
    let memory = Arc::new(TokenMemory {
        ceiling: 1 << 32,
        ..TokenMemory::default()
    });
    let mut work = large_budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BatchOutputMemoryScope(Arc::clone(&memory)),
    ))));
    let parsed = parse_native_content_query("file.contains('zzqq')")
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let plan = crate::planner::LexicalPlanner::plan(&parsed)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let fixture = &corpus.fixture;
    let ranking = anchored_index(&corpus).search_content(
        &fixture.member,
        &plan,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        4,
        crate::native::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
        TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )?;
    check_eq!(ranking.matched, 0);
    check!(ranking.hits.is_empty());
    check_eq!(take_reads(&corpus), 0);
    Ok(())
}

/// A fixed-seed linear congruential generator; tests need no entropy.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let bound = u64::try_from(bound.max(1)).map_or(1, |bound| bound);
        let value = self
            .0
            .checked_shr(33)
            .and_then(|high| high.checked_rem(bound))
            .map_or(0, |value| value);
        usize::try_from(value).map_or(0, |value| value)
    }
}

/// Every fold class the anchor must not lose: ASCII case, NFC and NFD
/// accents, the dotted capital I, sharp s, the Kelvin sign and `ss`/`k`.
const FOLD_PIECES: [&str; 14] = [
    "k", "K", "\u{212A}", "ss", "ß", "S", "i", "I", "İ", "é", "e\u{301}", "E", "ab", " ",
];

/// Literal-anchored answers equal a brute-force scan over every fold class.
///
/// Random needles over random fold-class texts: the anchored answer is
/// exactly the owners whose body contains the needle after NFC and the case
/// mode, computed here. A missing owner would be an anchor that lost a match.
#[test]
#[expect(
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    reason = "the generator indexes its own fixed piece table; fixture bodies are UTF-8"
)]
fn anchored_answers_equal_a_brute_force_scan_across_fold_classes() -> Result<(), IndexError> {
    let mut random = Lcg(0x5eed_0009_0001);
    let docs: Vec<(String, Vec<u8>)> = (0..30_usize)
        .map(|ordinal| {
            let body: String = (0..10)
                .map(|_| FOLD_PIECES[random.below(FOLD_PIECES.len())])
                .collect();
            (format!("src/d{ordinal:02}.rs"), body.into_bytes())
        })
        .collect();
    let mut corpus = corpus_with_paths(&docs)?;
    let sources = corpus.fixture.ticket.view.source.owners;
    publish_sibling(
        &mut corpus,
        Layout::Packed,
        LITERAL_INSTANCE,
        LITERAL_FORMAT,
        sources,
        0xc0,
    )?;
    let mut answered = 0usize;
    for _case in 0..120 {
        let needle: String = (0..2 + random.below(3))
            .map(|_| FOLD_PIECES[random.below(FOLD_PIECES.len() - 1)])
            .collect();
        let sensitive = random.below(2) == 0;
        let case = if sensitive {
            CaseMode::Sensitive
        } else {
            CaseMode::Folded
        };
        let surface = |value: &str| {
            crate::normalize::apply_case(&crate::normalize::nfc(value), case).into_owned()
        };
        let target = surface(&needle);
        let mut expected: Vec<OwnerKey> = docs
            .iter()
            .enumerate()
            .filter(|(_, (_, body))| {
                surface(std::str::from_utf8(body).unwrap_or_default()).contains(&target)
            })
            .map(|(ordinal, _)| corpus.owner(ordinal))
            .collect::<Result<_, _>>()?;
        expected.sort_unstable();
        let query = format!(
            "file.contains('{needle}'){}",
            if sensitive { " case:yes" } else { "" }
        );
        let (found, _reads) = answer(&corpus, anchored_index(&corpus), &query)?;
        check_eq!(found, expected);
        answered += 1;
    }
    check_eq!(answered, 120);
    Ok(())
}
