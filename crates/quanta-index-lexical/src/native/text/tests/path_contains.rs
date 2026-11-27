//! `path.contains`, `path.regex` and path-only queries over Source path
//! metadata.
//!
//! Expected owners follow from the fixed `(path, body)` table alone. Path
//! leaves are decided from the pinned path map, so they read no body.

use super::literal_anchor::{
    Corpus, LITERAL_FORMAT, LITERAL_INSTANCE, Layout, anchored_index, answer, corpus_with_paths,
    large_budget, publish_sibling, search, take_reads,
};
use super::*;
use quanta_index_contract::{LqExpr, LqFileScope, LqFilter, LqQuery, LqSpan};

const UPPER_ALPHA: usize = 0;
const ALPHA_NOTES: usize = 1;
const BODY_ALPHA: usize = 2;
const LIB_GAMMA: usize = 3;
const LIB_DELTA: usize = 4;

fn small_docs() -> Vec<(String, Vec<u8>)> {
    [
        ("src/Alpha.rs", "fn main() {}"),
        ("docs/alpha_notes.md", "notes"),
        ("src/beta.rs", "let alpha = 1;"),
        ("lib/gamma.rs", "gamma"),
        ("lib/delta.rs", "delta"),
    ]
    .iter()
    .map(|(path, body)| ((*path).to_owned(), body.as_bytes().to_vec()))
    .collect()
}

fn owners(corpus: &Corpus, ordinals: &[usize]) -> Result<Vec<OwnerKey>, IndexError> {
    let mut owners = ordinals
        .iter()
        .map(|ordinal| corpus.owner(*ordinal))
        .collect::<Result<Vec<_>, _>>()?;
    owners.sort_unstable();
    Ok(owners)
}

fn plain(corpus: &Corpus) -> NativeTextIndex {
    NativeTextIndex::new(corpus.fixture.recipe)
}

/// Plans an empty expression with only `file:` path filters.
fn path_only_plan(patterns: &[&str]) -> Result<crate::plan::LexicalPlan, IndexError> {
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = LqExpr::Empty;
    for pattern in patterns {
        query.filters.push(LqFilter::File {
            pattern: (*pattern).to_owned(),
            scope: LqFileScope::PathOnly,
        });
    }
    let _backing = crate::native::validate_native_text_input(&query)?;
    crate::planner::LexicalPlanner::plan(&query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))
}

fn run_plan(
    corpus: &Corpus,
    plan: &crate::plan::LexicalPlan,
) -> Result<crate::native::text_query::NativeTextRanking, IndexError> {
    let fixture = &corpus.fixture;
    plain(corpus).search_content(
        &fixture.member,
        plan,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        64,
        crate::native::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
        TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut large_budget()?,
        },
    )
}

#[test]
fn path_contains_matches_path_metadata_without_reading_bodies() -> Result<(), IndexError> {
    let corpus = corpus_with_paths(&small_docs())?;
    // case:no folds the path like file.contains folds the body.
    let (found, reads) = answer(&corpus, plain(&corpus), "path.contains('alpha')")?;
    check_eq!(found, owners(&corpus, &[UPPER_ALPHA, ALPHA_NOTES])?);
    check_eq!(reads, 0);
    let (found, reads) = answer(&corpus, plain(&corpus), "path.contains('alpha') case:yes")?;
    check_eq!(found, owners(&corpus, &[ALPHA_NOTES])?);
    check_eq!(reads, 0);
    // Substring of the repository-relative path, not of the body.
    let (found, _reads) = answer(&corpus, plain(&corpus), "path.contains('lib/')")?;
    check_eq!(found, owners(&corpus, &[LIB_GAMMA, LIB_DELTA])?);
    let (found, _reads) = answer(&corpus, plain(&corpus), "path.contains('nowhere')")?;
    check!(found.is_empty());
    // Composition: body OR path, and path AND NOT body.
    let (found, _reads) = answer(
        &corpus,
        plain(&corpus),
        "file.contains('alpha') OR path.contains('alpha')",
    )?;
    check_eq!(
        found,
        owners(&corpus, &[UPPER_ALPHA, ALPHA_NOTES, BODY_ALPHA])?
    );
    let (found, _reads) = answer(
        &corpus,
        plain(&corpus),
        "path.contains('lib/') AND NOT file.contains('gamma')",
    )?;
    check_eq!(found, owners(&corpus, &[LIB_DELTA])?);
    Ok(())
}

#[test]
fn path_only_query_runs_over_path_metadata() -> Result<(), IndexError> {
    let corpus = corpus_with_paths(&small_docs())?;
    let _stale = take_reads(&corpus);
    let ranking = run_plan(&corpus, &path_only_plan(&["^lib/"])?)?;
    let mut found: Vec<OwnerKey> = ranking.hits.iter().map(|hit| hit.owner).collect();
    found.sort_unstable();
    check_eq!(found, owners(&corpus, &[LIB_GAMMA, LIB_DELTA])?);
    check_eq!(ranking.matched, 2);
    check_eq!(take_reads(&corpus), 0);
    // Several path filters must all match.
    let ranking = run_plan(&corpus, &path_only_plan(&["^lib/", "delta"])?)?;
    check_eq!(ranking.matched, 1);
    // An empty expression without a path filter has no candidate supplier.
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = LqExpr::Empty;
    check!(matches!(
        crate::native::validate_native_text_input(&query),
        Err(IndexError::InvalidInput(_))
    ));
    Ok(())
}

const PATH_NEEDLES: [usize; 2] = [100, 3_000];
const BODY_NEEDLES: [usize; 3] = [7, 2_048, 4_090];

fn large_docs() -> Vec<(String, Vec<u8>)> {
    (0..4_097_usize)
        .map(|ordinal| {
            let path = if PATH_NEEDLES.contains(&ordinal) {
                format!("src/needle_{ordinal:04}.rs")
            } else {
                format!("src/file_{ordinal:04}.rs")
            };
            let body = if BODY_NEEDLES.contains(&ordinal) {
                format!("let found = needle{ordinal};")
            } else {
                format!("let value{ordinal} = common;")
            };
            (path, body.into_bytes())
        })
        .collect()
}

#[test]
fn path_contains_is_a_finite_candidate_supplier_over_a_large_view() -> Result<(), IndexError> {
    let mut corpus = corpus_with_paths(&large_docs())?;
    // Path metadata alone answers over more than 4,096 files.
    let (found, reads) = answer(&corpus, plain(&corpus), "path.contains('needle')")?;
    check_eq!(found, owners(&corpus, &PATH_NEEDLES)?);
    check_eq!(reads, 0);
    let ranking = run_plan(&corpus, &path_only_plan(&["needle_"])?)?;
    check_eq!(ranking.matched, 2);
    check_eq!(take_reads(&corpus), 0);
    // Without a Literal anchor the body side is unbounded: refused.
    check!(matches!(
        search(
            &corpus,
            plain(&corpus),
            "file.contains('needle') OR path.contains('needle')",
            8
        ),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(take_reads(&corpus), 0);
    // With it, the bound is 3 body candidates + 2 path candidates.
    let sources = corpus.fixture.ticket.view.source.owners;
    publish_sibling(
        &mut corpus,
        Layout::Packed,
        LITERAL_INSTANCE,
        LITERAL_FORMAT,
        sources,
        0xd0,
    )?;
    let (found, reads) = answer(
        &corpus,
        anchored_index(&corpus),
        "file.contains('needle') OR path.contains('needle')",
    )?;
    let mut expected: Vec<usize> = PATH_NEEDLES.to_vec();
    expected.extend(BODY_NEEDLES);
    check_eq!(found, owners(&corpus, &expected)?);
    check!(reads <= 5, "verified {reads} bodies above the bound 5");
    Ok(())
}

/// One overlay row: `text: None` deletes the owner.
struct Edit {
    owner: OwnerKey,
    path: &'static str,
    text: Option<&'static str>,
}

/// Runs `plan` with the overlay `edits` over `corpus`; returns matched owners.
fn overlay_answer(
    corpus: &Corpus,
    index: NativeTextIndex,
    plan: &crate::plan::LexicalPlan,
    edits: &[Edit],
    repository: Option<&quanta_index_contract::RepoId>,
) -> Result<(Vec<OwnerKey>, usize), IndexError> {
    use super::super::super::text_overlay::TextOverlayInput;
    use super::super::super::text_query::{Bm25Policy, TextContentExecution, TextOverlaySource};

    let fixture = &corpus.fixture;
    let mut edits: Vec<&Edit> = edits.iter().collect();
    edits.sort_unstable_by_key(|edit| edit.owner);
    let inputs: Vec<TextOverlayInput<'_>> = edits
        .iter()
        .map(|edit| TextOverlayInput {
            owner: edit.owner,
            revision: 9,
            text: edit.text,
        })
        .collect();
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
    let sources: Vec<TextOverlaySource<'_>> = edits
        .iter()
        .filter_map(|edit| {
            edit.text.map(|body| TextOverlaySource {
                owner: edit.owner,
                revision: 9,
                path: edit.path,
                body,
                repository,
                language: None,
            })
        })
        .collect();
    let _stale = take_reads(corpus);
    let ranking = index.search_content_with_overlay(
        &fixture.member,
        plan,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        TextContentExecution {
            top_k: 64,
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
    )?;
    let mut found: Vec<OwnerKey> = ranking.hits.iter().map(|hit| hit.owner).collect();
    check_eq!(
        ranking.matched,
        u64::try_from(found.len()).map_err(|_width| IndexError::ResourceLimited)?
    );
    found.sort_unstable();
    Ok((found, take_reads(corpus)))
}

fn text_plan(query: &str) -> Result<crate::plan::LexicalPlan, IndexError> {
    let parsed = quanta_index_contract::parse_native_content_query(query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let _backing = crate::native::validate_native_text_input(&parsed)?;
    crate::planner::LexicalPlanner::plan(&parsed)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))
}

fn created(corpus: &Corpus) -> OwnerKey {
    OwnerKey {
        producer: corpus.fixture.owner.producer,
        stable_id: StableOwnerId([0x77; 16]),
    }
}

/// Modify keeps the path, a rename moves it, a delete hides the durable
/// path, and a create adds one. Durable rows the overlay masks never count.
#[test]
fn overlay_rows_decide_path_leaves_on_their_own_path() -> Result<(), IndexError> {
    let corpus = corpus_with_paths(&small_docs())?;
    let new_owner = created(&corpus);
    let edits = [
        Edit {
            owner: corpus.owner(LIB_GAMMA)?,
            path: "lib/gamma.rs",
            text: Some("gamma edited"),
        },
        Edit {
            owner: corpus.owner(LIB_DELTA)?,
            path: "src/alpha_delta.rs",
            text: Some("delta"),
        },
        Edit {
            owner: corpus.owner(ALPHA_NOTES)?,
            path: "docs/alpha_notes.md",
            text: None,
        },
        Edit {
            owner: new_owner,
            path: "lib/alpha_new.rs",
            text: Some("new"),
        },
    ];
    let mut expected = owners(&corpus, &[UPPER_ALPHA, LIB_DELTA])?;
    expected.push(new_owner);
    expected.sort_unstable();
    let (found, reads) = overlay_answer(
        &corpus,
        plain(&corpus),
        &text_plan("path.contains('alpha')")?,
        &edits,
        None,
    )?;
    check_eq!(found, expected);
    check_eq!(reads, 0);
    let mut lib = owners(&corpus, &[LIB_GAMMA])?;
    lib.push(new_owner);
    lib.sort_unstable();
    let (found, reads) = overlay_answer(
        &corpus,
        plain(&corpus),
        &text_plan("path.contains('lib/')")?,
        &edits,
        None,
    )?;
    check_eq!(found, lib);
    check_eq!(reads, 0);
    let (found, reads) = overlay_answer(
        &corpus,
        plain(&corpus),
        &path_only_plan(&["^lib/"])?,
        &edits,
        None,
    )?;
    check_eq!(found, lib);
    check_eq!(reads, 0);
    // The CodeSearch bare-term shape: body OR path.
    let mut either = owners(&corpus, &[UPPER_ALPHA, BODY_ALPHA, LIB_DELTA])?;
    either.push(new_owner);
    either.sort_unstable();
    let (found, _reads) = overlay_answer(
        &corpus,
        plain(&corpus),
        &text_plan("file.contains('alpha') OR path.contains('alpha')")?,
        &edits,
        None,
    )?;
    check_eq!(found, either);
    Ok(())
}

#[test]
fn overlay_path_match_keeps_repositories_with_the_same_path_apart() -> Result<(), IndexError> {
    let corpus = super::metadata_filters::corpus(&[
        super::metadata_filters::Doc {
            path: "src/x.rs",
            body: "one",
            repository: Some("org/alpha"),
            language: None,
        },
        super::metadata_filters::Doc {
            path: "src/x.rs",
            body: "two",
            repository: Some("org/beta"),
            language: None,
        },
    ])?;
    let alpha = corpus.owner(0)?;
    let beta = corpus.owner(1)?;
    let repository = quanta_index_contract::RepoId::new("org/alpha")
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let as_literal = Corpus {
        fixture: corpus.fixture,
        owners: vec![alpha, beta],
        bodies: Vec::new(),
    };
    let edits = [Edit {
        owner: alpha,
        path: "src/x.rs",
        text: Some("one edited"),
    }];
    let mut both = vec![alpha, beta];
    both.sort_unstable();
    let (found, _reads) = overlay_answer(
        &as_literal,
        plain(&as_literal),
        &text_plan("path.contains('src/x')")?,
        &edits,
        Some(&repository),
    )?;
    check_eq!(found, both);
    let (found, _reads) = overlay_answer(
        &as_literal,
        plain(&as_literal),
        &text_plan("path.contains('src/x') repo:beta")?,
        &edits,
        Some(&repository),
    )?;
    check_eq!(found, vec![beta]);
    let (found, _reads) = overlay_answer(
        &as_literal,
        plain(&as_literal),
        &text_plan("path.contains('src/x') repo:alpha")?,
        &edits,
        Some(&repository),
    )?;
    check_eq!(found, vec![alpha]);
    Ok(())
}

/// Body-or-path search stays answerable with unsaved edits over a large view.
///
/// Over more than 4,096 files, durable Literal anchors and the path census
/// exclude masked owners, and overlay rows are decided exactly on their own
/// text.
#[test]
fn overlay_keeps_body_or_path_search_bounded_over_a_large_view() -> Result<(), IndexError> {
    let mut corpus = corpus_with_paths(&large_docs())?;
    let sources = corpus.fixture.ticket.view.source.owners;
    publish_sibling(
        &mut corpus,
        Layout::Packed,
        LITERAL_INSTANCE,
        LITERAL_FORMAT,
        sources,
        0xd0,
    )?;
    let new_owner = created(&corpus);
    let [edited, kept_one, kept_two] = BODY_NEEDLES;
    let edits = [
        Edit {
            owner: corpus.owner(edited)?,
            path: "src/file_0007.rs",
            text: Some("no match here"),
        },
        Edit {
            owner: new_owner,
            path: "lib/needle_new.rs",
            text: Some("fresh"),
        },
    ];
    let mut expected = owners(
        &corpus,
        &[PATH_NEEDLES[0], PATH_NEEDLES[1], kept_one, kept_two],
    )?;
    expected.push(new_owner);
    expected.sort_unstable();
    let (found, reads) = overlay_answer(
        &corpus,
        anchored_index(&corpus),
        &text_plan("file.contains('needle') OR path.contains('needle')")?,
        &edits,
        None,
    )?;
    check_eq!(found, expected);
    check!(
        reads <= 5,
        "verified {reads} durable bodies above the bound"
    );
    Ok(())
}

#[test]
fn path_regex_matches_path_metadata_without_reading_bodies() -> Result<(), IndexError> {
    let corpus = corpus_with_paths(&small_docs())?;
    let (found, reads) = answer(&corpus, plain(&corpus), "path.regex('^lib/')")?;
    check_eq!(found, owners(&corpus, &[LIB_GAMMA, LIB_DELTA])?);
    check_eq!(reads, 0);
    // case:no folds the pattern like a Content regex; case:yes does not.
    let (found, reads) = answer(&corpus, plain(&corpus), "path.regex('alpha[._]')")?;
    check_eq!(found, owners(&corpus, &[UPPER_ALPHA, ALPHA_NOTES])?);
    check_eq!(reads, 0);
    let (found, _reads) = answer(&corpus, plain(&corpus), "path.regex('alpha[._]') case:yes")?;
    check_eq!(found, owners(&corpus, &[ALPHA_NOTES])?);
    // The unscoped CodeSearch regex shape: body regex OR path regex.
    let (found, _reads) = answer(&corpus, plain(&corpus), "/alpha/ OR path.regex('alpha')")?;
    check_eq!(
        found,
        owners(&corpus, &[UPPER_ALPHA, ALPHA_NOTES, BODY_ALPHA])?
    );
    let (found, _reads) = answer(
        &corpus,
        plain(&corpus),
        "path.regex('^lib/') AND NOT file.contains('gamma')",
    )?;
    check_eq!(found, owners(&corpus, &[LIB_DELTA])?);
    // Literal and regex path leaves are decided by one census.
    let (found, reads) = answer(
        &corpus,
        plain(&corpus),
        "path.contains('lib/') path.regex('delta')",
    )?;
    check_eq!(found, owners(&corpus, &[LIB_DELTA])?);
    check_eq!(reads, 0);
    Ok(())
}

#[test]
fn path_regex_refuses_invalid_patterns_and_extra_leaves() -> Result<(), IndexError> {
    let corpus = corpus_with_paths(&small_docs())?;
    check!(matches!(
        search(&corpus, plain(&corpus), "path.regex('(')", 8),
        Err(IndexError::InvalidInput(_))
    ));
    let five = (0..5)
        .map(|ordinal| format!("path.regex('x{ordinal}')"))
        .collect::<Vec<_>>()
        .join(" OR ");
    check!(matches!(text_plan(&five), Err(IndexError::ResourceLimited)));
    Ok(())
}

#[test]
fn path_regex_is_a_finite_candidate_supplier_over_a_large_view() -> Result<(), IndexError> {
    let corpus = corpus_with_paths(&large_docs())?;
    let (found, reads) = answer(&corpus, plain(&corpus), "path.regex('needle_[0-9]+')")?;
    check_eq!(found, owners(&corpus, &PATH_NEEDLES)?);
    check_eq!(reads, 0);
    Ok(())
}

/// Overlay rows decide a path regex on their own path, as a path substring.
#[test]
fn overlay_rows_decide_path_regex_on_their_own_path() -> Result<(), IndexError> {
    let corpus = corpus_with_paths(&small_docs())?;
    let new_owner = created(&corpus);
    let edits = [
        Edit {
            owner: corpus.owner(LIB_GAMMA)?,
            path: "lib/gamma.rs",
            text: Some("gamma edited"),
        },
        Edit {
            owner: corpus.owner(LIB_DELTA)?,
            path: "src/alpha_delta.rs",
            text: Some("delta"),
        },
        Edit {
            owner: corpus.owner(ALPHA_NOTES)?,
            path: "docs/alpha_notes.md",
            text: None,
        },
        Edit {
            owner: new_owner,
            path: "lib/alpha_new.rs",
            text: Some("new"),
        },
    ];
    let mut expected = owners(&corpus, &[UPPER_ALPHA, LIB_DELTA])?;
    expected.push(new_owner);
    expected.sort_unstable();
    let (found, reads) = overlay_answer(
        &corpus,
        plain(&corpus),
        &text_plan("path.regex('alpha')")?,
        &edits,
        None,
    )?;
    check_eq!(found, expected);
    check_eq!(reads, 0);
    let mut lib = owners(&corpus, &[LIB_GAMMA])?;
    lib.push(new_owner);
    lib.sort_unstable();
    let (found, reads) = overlay_answer(
        &corpus,
        plain(&corpus),
        &text_plan("path.regex('^lib/')")?,
        &edits,
        None,
    )?;
    check_eq!(found, lib);
    check_eq!(reads, 0);
    Ok(())
}

/// More matches than the order bound: the page is answered, without an order.
#[test]
fn rank_order_capture_is_dropped_above_its_bound() -> Result<(), IndexError> {
    let corpus = corpus_with_paths(&large_docs())?;
    let fixture = &corpus.fixture;
    let ranking = plain(&corpus).search_content_capturing(
        &fixture.member,
        &text_plan("path.contains('src/')")?,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        8,
        crate::native::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
        TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut large_budget()?,
        },
    )?;
    check_eq!(ranking.matched, 4_097);
    check_eq!(ranking.hits.len(), 8);
    check!(ranking.order.is_none());
    Ok(())
}

/// A path leaf beside an admitted Literal anchor only filters: it is decided
/// on each candidate's authenticated path and never enumerates the path map,
/// whether it is broad, selective or negated.
#[test]
fn a_non_driving_path_leaf_filters_candidates_without_a_census() -> Result<(), IndexError> {
    let mut corpus = corpus_with_paths(&large_docs())?;
    let sources = corpus.fixture.ticket.view.source.owners;
    publish_sibling(
        &mut corpus,
        Layout::Packed,
        LITERAL_INSTANCE,
        LITERAL_FORMAT,
        sources,
        0xd0,
    )?;
    corpus.fixture.maps.counted_inventory = corpus.fixture.ticket.view.source.paths;
    let scans = |corpus: &Corpus| {
        corpus
            .fixture
            .maps
            .inventory_scans
            .swap(0, std::sync::atomic::Ordering::Relaxed)
    };
    // Every one of the 4,097 paths contains `src/`; the body needle is in 3.
    let (found, reads) = answer(
        &corpus,
        anchored_index(&corpus),
        "file.contains('needle') path.contains('src/')",
    )?;
    check_eq!(found, owners(&corpus, &BODY_NEEDLES)?);
    check!(reads <= 3, "verified {reads} bodies above the bound 3");
    check_eq!(scans(&corpus), 0);
    // The filter decides on the candidate's own path, also under NOT.
    let (found, _reads) = answer(
        &corpus,
        anchored_index(&corpus),
        "file.contains('needle') path.contains('needle_')",
    )?;
    check!(found.is_empty());
    let (found, _reads) = answer(
        &corpus,
        anchored_index(&corpus),
        "file.contains('needle') AND NOT path.contains('file_0007')",
    )?;
    check_eq!(found, owners(&corpus, &BODY_NEEDLES[1..])?);
    check_eq!(scans(&corpus), 0);
    // A path leaf that drives still uses the full census.
    let (found, _reads) = answer(&corpus, anchored_index(&corpus), "path.contains('needle_')")?;
    check_eq!(found, owners(&corpus, &PATH_NEEDLES)?);
    check!(scans(&corpus) > 0);
    // When the anchors alone exceed the 4,096 bound, a path census capped at
    // that bound runs: `let ` is in all 4,097 bodies, and one path bounds the
    // conjunction, so the path leaf drives it.
    let (found, reads) = answer(
        &corpus,
        anchored_index(&corpus),
        "file.contains('let ') path.contains('needle_0100')",
    )?;
    check_eq!(found, owners(&corpus, &PATH_NEEDLES[..1])?);
    check!(reads <= 1, "verified {reads} bodies above the bound 1");
    Ok(())
}

/// Overlay path leaves equal a brute-force scan of the virtual file set.
///
/// Random overlay edit sets over the small view: the virtual file set is the
/// durable paths, replaced by each modify or rename, hidden by each delete
/// and extended by each create.
#[test]
#[expect(
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::similar_names,
    reason = "the generator indexes its own fixed tables"
)]
fn overlay_path_leaves_equal_a_brute_force_virtual_scan() -> Result<(), IndexError> {
    const PATHS: [&str; 6] = [
        "src/alpha_x.rs",
        "lib/Alpha.rs",
        "docs/gamma.md",
        "lib/new_alpha.rs",
        "src/b.rs",
        "tools/A_lpha.rs",
    ];
    const NEEDLES: [&str; 6] = ["alpha", "lib/", "src/", "gamma", "a_", ".rs"];
    let corpus = corpus_with_paths(&small_docs())?;
    // The paths of `small_docs`, in owner order.
    let durable: [&'static str; 5] = [
        "src/Alpha.rs",
        "docs/alpha_notes.md",
        "src/beta.rs",
        "lib/gamma.rs",
        "lib/delta.rs",
    ];
    check_eq!(
        small_docs()
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        durable.to_vec()
    );
    let mut seed = 0x5eed_0009_0004_u64;
    let mut below = |bound: usize| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from((seed >> 33) % u64::try_from(bound).unwrap_or(1)).unwrap_or(0)
    };
    for _case in 0..40 {
        let mut edits = Vec::new();
        // The virtual file set: (owner, path) after the edits.
        let mut files: Vec<(OwnerKey, &str)> = Vec::new();
        for (ordinal, path) in durable.iter().enumerate() {
            let owner = corpus.owner(ordinal)?;
            match below(4) {
                0 => files.push((owner, path)),
                1 => {
                    edits.push(Edit {
                        owner,
                        path,
                        text: Some("modified"),
                    });
                    files.push((owner, path));
                }
                2 => {
                    let renamed = PATHS[below(PATHS.len())];
                    edits.push(Edit {
                        owner,
                        path: renamed,
                        text: Some("renamed"),
                    });
                    files.push((owner, renamed));
                }
                _ => edits.push(Edit {
                    owner,
                    path,
                    text: None,
                }),
            }
        }
        if below(2) == 0 {
            let path = PATHS[below(PATHS.len())];
            if files.iter().all(|(_, existing)| *existing != path) {
                let owner = created(&corpus);
                edits.push(Edit {
                    owner,
                    path,
                    text: Some("created"),
                });
                files.push((owner, path));
            }
        }
        // Two rows of one path conflict; the generator keeps paths distinct.
        let mut seen: Vec<&str> = files.iter().map(|(_, path)| *path).collect();
        seen.sort_unstable();
        if seen.windows(2).any(|pair| pair[0] == pair[1]) {
            continue;
        }
        let needle = NEEDLES[below(NEEDLES.len())];
        let regex = below(2) == 0;
        let folded = |value: &str| {
            crate::normalize::apply_case(&crate::normalize::nfc(value), CaseMode::Folded)
                .into_owned()
        };
        let mut expected: Vec<OwnerKey> = files
            .iter()
            .filter(|(_, path)| folded(path).contains(&folded(needle)))
            .map(|(owner, _)| *owner)
            .collect();
        expected.sort_unstable();
        let query = if regex {
            format!("path.regex('{}')", needle.replace('.', "[.]"))
        } else {
            format!("path.contains('{needle}')")
        };
        let (found, reads) =
            overlay_answer(&corpus, plain(&corpus), &text_plan(&query)?, &edits, None)?;
        check_eq!(found, expected);
        check_eq!(reads, 0);
    }
    Ok(())
}
