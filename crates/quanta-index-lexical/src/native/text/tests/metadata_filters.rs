//! `repo:` and `lang:` filters over authenticated Source metadata.
//!
//! Every expected owner below follows from the fixed source table alone:
//! two repositories publish the same `src/lib.rs`, and one low-scoring Python
//! file sits beside higher-scoring Rust files.

use super::*;
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{RepoId, parse_native_content_query};
use quanta_index_core::index::{SourcePathOwners, SourcePathRepositoryKey};

pub(super) struct Doc {
    pub(super) path: &'static str,
    pub(super) body: &'static str,
    pub(super) repository: Option<&'static str>,
    pub(super) language: Option<&'static str>,
}

const ALPHA_LIB: usize = 0;
const BETA_LIB: usize = 1;
const ALPHA_PYTHON: usize = 2;

// Rust files score higher than the Python file: same term frequency per
// token is far lower in the long Python body.
const DOCS: [Doc; 3] = [
    Doc {
        path: "src/lib.rs",
        body: "needle needle needle",
        repository: Some("org/alpha"),
        language: Some("rust"),
    },
    Doc {
        path: "src/lib.rs",
        body: "needle needle needle",
        repository: Some("org/beta"),
        language: Some("rust"),
    },
    Doc {
        path: "tools/gen.py",
        body: "needle filler filler filler filler filler filler filler filler filler filler",
        repository: Some("org/alpha"),
        language: Some("python"),
    },
];

pub(super) struct Corpus {
    pub(super) fixture: VisitorFixture,
    pub(super) owners: Vec<OwnerKey>,
}

impl Corpus {
    pub(super) fn owner(&self, ordinal: usize) -> Result<OwnerKey, IndexError> {
        self.owners
            .get(ordinal)
            .copied()
            .ok_or_else(|| IndexError::BackendViolation("fixture owner ordinal".into()))
    }
}

fn large_budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 256 * 1024 * 1024,
            mutations: 1_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(120))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn ordinal_byte(ordinal: usize) -> Result<u8, IndexError> {
    u8::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)
}

fn tagged_ref(tag: u8, ordinal: usize) -> Result<ObjectRef, IndexError> {
    let mut digest = [tag; 32];
    digest[1] = ordinal_byte(ordinal)?;
    Ok(ObjectRef(Digest(digest)))
}

fn repo_id(value: &str) -> Result<RepoId, IndexError> {
    RepoId::new(value).map_err(|error| IndexError::InvalidInput(error.to_string()))
}

/// Builds a real segmented member over `docs`, with repository path markers
/// so an exact-path constraint selects every repository's copy.
pub(super) fn corpus(docs: &[Doc]) -> Result<Corpus, IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.recipe = TextRecipe::segmented_full(fixture.recipe.admission_policy);
    fixture.objects.generated = Some(Mutex::default());
    let source_root = fixture.ticket.view.source.owners;
    let source_key = source_root
        .0
        .ok_or_else(|| IndexError::BackendViolation("fixture source root missing".into()))?;
    let _previous = fixture.maps.roots.insert(source_key, BTreeMap::new());
    let mut owners = Vec::new();
    let mut markers = Vec::new();
    for (ordinal, doc) in docs.iter().enumerate() {
        let owner = OwnerKey {
            producer: fixture.owner.producer,
            stable_id: StableOwnerId([0x60_u8.wrapping_add(ordinal_byte(ordinal)?); 16]),
        };
        let body_ref = tagged_ref(0xa0, ordinal)?;
        let source_ref = tagged_ref(0xa1, ordinal)?;
        let repository = doc.repository.map(repo_id).transpose()?;
        let record = SourceRecord {
            path: doc.path.into(),
            bytes: body_ref,
            byte_len: u64::try_from(doc.body.len())
                .map_err(|_error| IndexError::ResourceLimited)?,
            text: SourceTextAdmission::Utf8 {
                policy: fixture.recipe.admission_policy,
            },
            source_repo_id: repository.clone(),
            language: doc
                .language
                .map(|code| {
                    LanguageCode::new(code)
                        .map_err(|error| IndexError::InvalidInput(error.to_string()))
                })
                .transpose()?,
        };
        fixture
            .objects
            .add(body_ref, doc.body.as_bytes().to_vec(), Vec::new())?;
        fixture
            .objects
            .add(source_ref, encode(&record)?, vec![body_ref])?;
        fixture
            .maps
            .add(source_root, owner_map_key(owner).to_vec(), source_ref)?;
        owners.push(owner);
        markers.push((doc.path, repository, owner));
    }

    // One repository bucket per distinct path, keyed by repository. The
    // source cut, including its path map, is fixed before the build binds it.
    let paths = MapRoot(Some(reference(0xa2)));
    let mut buckets: Vec<&str> = Vec::new();
    for (path, repository, owner) in &markers {
        let Some(repository) = repository else {
            continue;
        };
        if !buckets.contains(path) {
            buckets.push(path);
        }
        let bucket = buckets
            .iter()
            .position(|known| known == path)
            .ok_or_else(|| IndexError::BackendViolation("path bucket missing".into()))?;
        let bucket = MapRoot(Some(tagged_ref(0xa3, bucket)?));
        let mut path_key = b"path\0".to_vec();
        path_key.extend_from_slice(path.as_bytes());
        let bucket_ref = bucket
            .0
            .ok_or_else(|| IndexError::BackendViolation("bucket root missing".into()))?;
        fixture.maps.add(
            paths,
            path_key,
            SourcePathOwners::Repositories(bucket_ref).encode()?,
        )?;
        let repo_key = SourcePathRepositoryKey::new(Some(repository))?;
        fixture.maps.add(
            bucket,
            repo_key.as_slice()?.to_vec(),
            quanta_index_core::index::path_owner_map_value(*owner)?,
        )?;
    }
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

    Ok(Corpus { fixture, owners })
}

fn search_with(
    corpus: &Corpus,
    query: &str,
    top_k: u32,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
) -> Result<crate::native::text_query::NativeTextRanking, IndexError> {
    let parsed = parse_native_content_query(query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    // The route preflight runs before planning, exactly as in production.
    let _plan_backing = crate::native::validate_native_text_input(&parsed)?;
    let plan = crate::planner::LexicalPlanner::plan(&parsed)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let fixture = &corpus.fixture;
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
            budget: &mut large_budget()?,
        },
    )
}

/// Owners of every returned hit, in rank order, with the matched total.
fn answered(
    corpus: &Corpus,
    query: &str,
    top_k: u32,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
) -> Result<(u64, Vec<OwnerKey>), IndexError> {
    let ranking = search_with(corpus, query, top_k, constraints).map_err(|error| {
        IndexError::BackendViolation(format!("{query} was not answered: {error:?}"))
    })?;
    Ok((
        ranking.matched,
        ranking.hits.iter().map(|hit| hit.owner).collect(),
    ))
}

fn unconstrained() -> quanta_index_contract::QueryConstraintSetV1 {
    quanta_index_contract::QueryConstraintSetV1::unconstrained()
}

fn sorted(mut owners: Vec<OwnerKey>) -> Vec<OwnerKey> {
    owners.sort_unstable();
    owners
}

#[test]
fn repo_filter_separates_repositories_sharing_a_relative_path() -> Result<(), IndexError> {
    let corpus = corpus(&DOCS)?;
    let alpha = corpus.owner(ALPHA_LIB)?;
    let beta = corpus.owner(BETA_LIB)?;
    let python = corpus.owner(ALPHA_PYTHON)?;

    // The repository pattern is an unanchored regex over the repository id.
    check_eq!(
        answered(&corpus, "needle repo:beta", 4, &unconstrained())?,
        (1, vec![beta])
    );
    let (matched, owners) = answered(&corpus, "needle repo:alpha", 4, &unconstrained())?;
    check_eq!(matched, 2);
    check_eq!(sorted(owners), sorted(vec![alpha, python]));
    check_eq!(
        answered(
            &corpus,
            "needle repo:^org/alpha$ file:lib",
            4,
            &unconstrained()
        )?,
        (1, vec![alpha])
    );

    // The exact path selects both repositories' copies; only the filter
    // separates them, before top-k.
    let exact = exact_path_constraint("src/lib.rs")?;
    let (matched, owners) = answered(&corpus, "needle", 4, &exact)?;
    check_eq!(matched, 2);
    check_eq!(sorted(owners), sorted(vec![alpha, beta]));
    check_eq!(
        answered(&corpus, "needle repo:beta", 1, &exact)?,
        (1, vec![beta])
    );
    check_eq!(
        answered(&corpus, "needle repo:alpha", 1, &exact)?,
        (1, vec![alpha])
    );
    // Several repo: filters must all match.
    check_eq!(
        answered(&corpus, "needle repo:alpha repo:beta", 4, &unconstrained())?,
        (0, Vec::new())
    );
    Ok(())
}

#[test]
fn language_filter_applies_before_top_k() -> Result<(), IndexError> {
    let corpus = corpus(&DOCS)?;
    let python = corpus.owner(ALPHA_PYTHON)?;

    // Without the filter the Python file ranks last.
    let (matched, owners) = answered(&corpus, "needle", 3, &unconstrained())?;
    check_eq!(matched, 3);
    check_eq!(owners.last().copied(), Some(python));
    check!(answered(&corpus, "needle", 1, &unconstrained())?.1 != vec![python]);

    // The sole allowed-language file is returned even at top_k = 1.
    check_eq!(
        answered(&corpus, "needle lang:python", 1, &unconstrained())?,
        (1, vec![python])
    );
    // Language ids are trimmed and ASCII-lowercased before comparison.
    check_eq!(
        answered(&corpus, "needle lang:Python", 1, &unconstrained())?,
        (1, vec![python])
    );
    // Several lang: filters admit any of the listed languages.
    check_eq!(
        answered(&corpus, "needle lang:python lang:rust", 4, &unconstrained())?.0,
        3
    );
    check_eq!(
        answered(&corpus, "needle lang:go", 4, &unconstrained())?,
        (0, Vec::new())
    );
    check_eq!(
        answered(
            &corpus,
            "needle repo:alpha lang:python",
            4,
            &unconstrained()
        )?,
        (1, vec![python])
    );
    // A filter-only source predicate composes with the metadata filter.
    check_eq!(
        answered(
            &corpus,
            "file.contains('filler') lang:python",
            4,
            &unconstrained()
        )?,
        (1, vec![python])
    );
    Ok(())
}

#[test]
fn metadata_filters_refuse_records_without_metadata() -> Result<(), IndexError> {
    let legacy = [
        Doc {
            path: "src/lib.rs",
            body: "needle",
            repository: Some("org/alpha"),
            language: None,
        },
        Doc {
            path: "src/main.rs",
            body: "needle",
            repository: None,
            language: Some("rust"),
        },
    ];
    let corpus = corpus(&legacy)?;
    for query in ["needle lang:rust", "needle repo:alpha"] {
        let outcome = search_with(&corpus, query, 4, &unconstrained());
        check!(
            matches!(&outcome, Err(IndexError::InvalidInput(message)) if message.contains("metadata")),
            "{query} did not refuse missing metadata: {outcome:?}"
        );
    }
    // Without a metadata filter the same records remain searchable.
    check_eq!(answered(&corpus, "needle", 4, &unconstrained())?.0, 2);
    // Revision-qualified repository filters have no history producer.
    check!(matches!(
        search_with(&corpus, "needle repo:alpha@main", 4, &unconstrained()),
        Err(IndexError::InvalidInput(_))
    ));
    Ok(())
}

/// An overlay row is filtered by its authenticated repository and language.
/// A row without the metadata a filter needs refuses the query, as a durable
/// record without it does; a new file has no authenticated language.
#[test]
fn metadata_filters_apply_authenticated_overlay_metadata() -> Result<(), IndexError> {
    use super::super::super::text_overlay::TextOverlayInput;
    use super::super::super::text_query::{Bm25Policy, TextContentExecution, TextOverlaySource};

    let corpus = corpus(&DOCS)?;
    let fixture = &corpus.fixture;
    let index = NativeTextIndex::new(fixture.recipe);
    let alpha = repo_id("org/alpha")?;
    let rust =
        LanguageCode::new("rust").map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let durable_owner = corpus.owner(ALPHA_LIB)?;
    let new_owner = OwnerKey {
        producer: fixture.owner.producer,
        stable_id: StableOwnerId([0x7f; 16]),
    };
    // Only the overlay row holds `marker`, so it alone is evaluated.
    let run = |owner: OwnerKey, language: Option<&LanguageCode>, query: &str| {
        let inputs = [TextOverlayInput {
            owner,
            revision: 7,
            text: Some("marker"),
        }];
        let overlay = index.prepare_overlay_corpus(
            &fixture.member,
            &inputs,
            &mut TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut large_budget()?,
            },
        )?;
        let sources = [TextOverlaySource {
            owner,
            revision: 7,
            path: "src/lib.rs",
            body: "marker",
            repository: Some(&alpha),
            language,
        }];
        let parsed = parse_native_content_query(query)
            .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let plan = crate::planner::LexicalPlanner::plan(&parsed)
            .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        index
            .search_content_with_overlay(
                &fixture.member,
                &plan,
                &unconstrained(),
                TextContentExecution {
                    top_k: 5,
                    policy: Bm25Policy { k1: 1.2, b: 0.75 },
                    rank_after: None,
                },
                Some((&overlay, &sources)),
                TextReadContext {
                    ticket: &fixture.ticket,
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    budget: &mut large_budget()?,
                },
            )
            .map(|ranking| ranking.matched)
    };
    for (query, expected) in [
        ("marker", 1),
        ("marker lang:rust", 1),
        ("marker repo:alpha", 1),
        ("marker repo:alpha lang:rust", 1),
        ("marker lang:python", 0),
        ("marker repo:beta", 0),
        ("marker repo:alpha lang:python", 0),
    ] {
        let matched = run(durable_owner, Some(&rust), query)?;
        check!(matched == expected, "{query} matched {matched}");
    }
    // The new-file row keeps its repository but has no language.
    check_eq!(run(new_owner, None, "marker repo:alpha")?, 1);
    let outcome = run(new_owner, None, "marker lang:rust");
    check!(
        matches!(&outcome, Err(IndexError::InvalidInput(message)) if message.contains("language metadata")),
        "lang: admitted an overlay row without language: {outcome:?}"
    );
    Ok(())
}
