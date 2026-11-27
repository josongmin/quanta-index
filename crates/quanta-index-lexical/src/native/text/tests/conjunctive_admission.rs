//! Structural candidate-bound admission for anchored source predicates.
//!
//! The fixed corpus has 4,097 admitted documents: 4,096 contain `common`,
//! exactly one of those also contains `rare`, and the last one contains only
//! `filler`. Every expected answer below follows from those bodies alone.

use super::*;
use quanta_index_contract::parse_native_content_query;

const COMMON_DOCS: u16 = 4_096;
const RARE_ORDINAL: u16 = 1_234;

struct Corpus {
    fixture: VisitorFixture,
    rare: OwnerKey,
}

fn ordinal_ref(tag: u8, ordinal: u16) -> ObjectRef {
    let mut digest = [tag; 32];
    let [high, low] = ordinal.to_be_bytes();
    digest[1] = high;
    digest[2] = low;
    ObjectRef(Digest(digest))
}

fn ordinal_owner(fixture: &VisitorFixture, ordinal: u16) -> OwnerKey {
    let mut stable = [0x5a; 16];
    let [high, low] = ordinal.to_be_bytes();
    stable[0] = high;
    stable[1] = low;
    OwnerKey {
        producer: fixture.owner.producer,
        stable_id: StableOwnerId(stable),
    }
}

fn large_budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 10_000_000,
            writes: 10_000_000,
            bytes: 1024 * 1024 * 1024,
            mutations: 10_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(600))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn corpus() -> Result<Corpus, IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.recipe = TextRecipe::segmented_full(fixture.recipe.admission_policy);
    fixture.objects.generated = Some(Mutex::default());
    let source_root = fixture.ticket.view.source.owners;
    let source_key = source_root
        .0
        .ok_or_else(|| IndexError::BackendViolation("fixture source root missing".into()))?;
    let _previous = fixture.maps.roots.insert(source_key, BTreeMap::new());
    let mut bodies = Vec::new();
    for ordinal in 0..=COMMON_DOCS {
        let body: &[u8] = if ordinal == COMMON_DOCS {
            b"filler"
        } else if ordinal == RARE_ORDINAL {
            b"rare common"
        } else {
            b"common"
        };
        let owner = ordinal_owner(&fixture, ordinal);
        let body_ref = ordinal_ref(0xb0, ordinal);
        let source_ref = ordinal_ref(0xc0, ordinal);
        let record = SourceRecord {
            path: format!("src/f{ordinal:04}.rs"),
            bytes: body_ref,
            byte_len: u64::try_from(body.len()).map_err(|_error| IndexError::ResourceLimited)?,
            text: SourceTextAdmission::Utf8 {
                policy: fixture.recipe.admission_policy,
            },
            source_repo_id: None,
            language: None,
        };
        fixture.objects.add(body_ref, body.to_vec(), Vec::new())?;
        fixture
            .objects
            .add(source_ref, encode(&record)?, vec![body_ref])?;
        fixture
            .maps
            .add(source_root, owner_map_key(owner).to_vec(), source_ref)?;
        bodies.push(body_ref);
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
    // Count only query-time source reads; the build above read every body.
    fixture.objects.tracked_bodies = bodies.into_iter().collect();
    let rare = ordinal_owner(&fixture, RARE_ORDINAL);
    Ok(Corpus { fixture, rare })
}

fn search(
    corpus: &Corpus,
    query: &str,
    top_k: u32,
) -> Result<crate::native::text_query::NativeTextRanking, IndexError> {
    let parsed = parse_native_content_query(query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let plan = crate::planner::LexicalPlanner::plan(&parsed)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let fixture = &corpus.fixture;
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
            budget: &mut large_budget()?,
        },
    )
}

fn answered(
    corpus: &Corpus,
    query: &str,
    top_k: u32,
) -> Result<crate::native::text_query::NativeTextRanking, IndexError> {
    search(corpus, query, top_k).map_err(|error| {
        IndexError::BackendViolation(format!("{query} was not answered: {error:?}"))
    })
}

/// Owner and exact score bits of the top-ranked hit.
fn top(
    ranking: &crate::native::text_query::NativeTextRanking,
) -> Result<(OwnerKey, u64), IndexError> {
    let hit = ranking
        .hits
        .first()
        .ok_or_else(|| IndexError::BackendViolation("ranking has no top hit".into()))?;
    Ok((hit.owner, hit.score.to_bits()))
}

fn take_source_reads(corpus: &Corpus) -> usize {
    corpus
        .fixture
        .objects
        .tracked_body_reads
        .swap(0, Ordering::Relaxed)
}

/// One test owns the 4,097-document build; each block states its own gold.
#[test]
fn conjunction_admission_uses_structural_candidate_bound() -> Result<(), IndexError> {
    let corpus = corpus()?;

    // Without a source predicate there is no admission bound; this is the
    // filter-free reference for owner, score and matched count.
    let reference = answered(&corpus, "rare AND common", 4)?;
    check_eq!(reference.matched, 1);
    check_eq!(reference.hits.len(), 1);
    check_eq!(top(&reference)?.0, corpus.rare);
    check_eq!(take_source_reads(&corpus), 0);

    // (a)+(e) AND bound = min(DF rare = 1, DF common = 4,096) = 1. The old
    // DF sum (4,097) refused this one-file answer. At most one owner may be
    // verified against source bytes.
    let conjunction = answered(&corpus, "rare AND common AND file.contains('rare com')", 4)?;
    check_eq!(conjunction.matched, 1);
    check_eq!(conjunction.hits.len(), 1);
    check_eq!(top(&conjunction)?.0, corpus.rare);
    // A filter-only source predicate keeps the reference BM25 score exactly.
    check_eq!(top(&conjunction)?.1, top(&reference)?.1);
    let verified = take_source_reads(&corpus);
    check!(
        verified == 1,
        "AND verified {verified} source bodies; bound is min DF = 1"
    );

    // (b) OR bound = 1 + 4,096 = 4,097 > 4,096: refused before any source
    // read. A complement-only branch has no finite bound and needs the full
    // census, which also exceeds 4,096.
    for refused in [
        "(rare OR common) AND file.contains('common')",
        "NOT rare AND file.contains('filler')",
    ] {
        check!(
            matches!(
                search(&corpus, refused, 4),
                Err(IndexError::ResourceLimited)
            ),
            "{refused} was not refused"
        );
        check_eq!(take_source_reads(&corpus), 0);
    }

    // (c) A single term with DF 4,096 is exactly at the bound.
    let common = answered(&corpus, "common AND file.contains('common')", 1)?;
    check_eq!(common.matched, u64::from(COMMON_DOCS));
    check_eq!(common.hits.len(), 1);
    let verified = take_source_reads(&corpus);
    check!(
        verified <= usize::from(COMMON_DOCS),
        "single-term query verified {verified} bodies above DF 4,096"
    );

    // (d) Complements and regexes only filter the bounded conjunction.
    let not_filler = answered(&corpus, "rare AND NOT filler AND file.contains('rare')", 4)?;
    check_eq!(not_filler.matched, 1);
    check_eq!(top(&not_filler)?.0, corpus.rare);
    check!(take_source_reads(&corpus) <= 1);
    let not_common = answered(&corpus, "rare AND NOT common AND file.contains('rare')", 4)?;
    check_eq!(not_common.matched, 0);
    check!(not_common.hits.is_empty());
    check!(take_source_reads(&corpus) <= 1);
    let regex = answered(&corpus, "rare AND common AND /ra.e/", 4)?;
    check_eq!(regex.matched, 1);
    check_eq!(top(&regex)?.0, corpus.rare);
    check_eq!(top(&regex)?.1, top(&reference)?.1);
    check!(take_source_reads(&corpus) <= 1);
    Ok(())
}
