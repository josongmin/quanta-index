//! QI-BB-025 W4 — runtime-metadata and structural pages walk by keyset
//! cursor at the daemon front door.
//!
//! Each route's pages partition its result set in candidate-id order,
//! once each, and every page names the epochs it was cut from: a
//! continuation is served from exactly those epochs, so an ingest that
//! lands between two pages — rows that would sort into the next page —
//! is invisible to the walk while a fresh walk sees it; and a cursor
//! whose epoch the plane no longer retains is refused typed rather than
//! served from a newer snapshot.
//!
//! The oracle is the fixture's own chunk ids and their byte order;
//! nothing here reads the daemon's clock or waits on timing.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;

use quanta_index_contract::{
    CandidateCountV1, ManifestGeneration, QueryResultWindowV1, RuntimeMetadataCursorV1,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneStructuralQueryResponse,
    StructuralCursorV1, TextQuerySyntax,
};
use quanta_index_core::{AUX_EPOCH_EXPIRED_CODE, AUX_EPOCH_RETAIN};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::{E2eRoutePage, E2eRuntime};

type TestResult = Result<(), Box<dyn Error>>;

/// Chunks in the generation.
const CHUNKS: usize = 30;
/// Of those, the rows in the walked set before the walk starts.
const ORIGINAL: usize = 25;
/// The rows that land between page one and page two: five ids that, in
/// candidate-id order, sort into page two of the original walk.
const LATE_START: usize = 12;
const LATE_END: usize = 17;
/// The page size: three pages of 10 / 10 / 5.
const PAGE: u32 = 10;
/// Wide enough to hold the whole set in one page.
const WHOLE: u32 = 100;
const RUNTIME_QUERY: &str = "dirty:only needle";
const STRUCTURAL_QUERY: &str = "match { function_item { { identifier :[name] } } }";

fn content_of(index: usize) -> String {
    format!("fn walk_{index}() {{ let needle = {index}; }}")
}

fn identifier_of(index: usize) -> String {
    format!("walk_{index}")
}

/// The generation's chunks: every candidate id with the path and index
/// it was ingested for, in candidate-id (byte) order.
struct Chunks {
    by_id: BTreeMap<String, (String, usize)>,
}

impl Chunks {
    fn ingest(rt: &mut E2eRuntime) -> Result<Self, Box<dyn Error>> {
        let mut by_id = BTreeMap::new();
        for index in 0..CHUNKS {
            let path = format!("src/walk_{index:02}.rs");
            let id = rt.ingest_text_with_candidate_id("repo", &path, &content_of(index))?;
            if by_id.insert(id, (path, index)).is_some() {
                return Err("the harness hands out distinct candidate ids".into());
            }
        }
        Ok(Self { by_id })
    }

    fn ids(&self) -> Vec<String> {
        self.by_id.keys().cloned().collect()
    }

    /// The ids of the walked set before the walk: every id but the late
    /// ones.
    fn original(&self) -> Vec<String> {
        self.ids()
            .into_iter()
            .enumerate()
            .filter(|(position, _)| !Self::is_late(*position))
            .map(|(_, id)| id)
            .collect()
    }

    /// The ids that land between page one and page two.
    fn late(&self) -> Vec<String> {
        self.ids()
            .into_iter()
            .enumerate()
            .filter(|(position, _)| Self::is_late(*position))
            .map(|(_, id)| id)
            .collect()
    }

    const fn is_late(position: usize) -> bool {
        position >= LATE_START && position < LATE_END
    }

    fn path_and_index(&self, id: &str) -> Result<(&str, usize), Box<dyn Error>> {
        self.by_id
            .get(id)
            .map(|(path, index)| (path.as_str(), *index))
            .ok_or_else(|| format!("unknown candidate id {id}").into())
    }
}

fn distinct(rows: &[String]) -> BTreeSet<&String> {
    rows.iter().collect()
}

/// The rows a page of the original walk must hold: `rows[range]`, or the
/// fixture is not the size the walk assumes.
fn page_rows(rows: &[String], range: std::ops::Range<usize>) -> Result<&[String], Box<dyn Error>> {
    rows.get(range.clone())
        .ok_or_else(|| format!("the original set has no rows {range:?}").into())
}

/// What one served page must satisfy against the oracle: its rows, its
/// window and whether it continues.
fn check_page(
    what: &str,
    rows: &[String],
    window: QueryResultWindowV1,
    expected_rows: &[String],
    expected_count: CandidateCountV1,
    cursor_id: Option<&str>,
) -> TestResult {
    if rows != expected_rows {
        return Err(
            format!("{what}: rows drifted: got {rows:?}, expected {expected_rows:?}").into(),
        );
    }
    if window.candidate_count() != expected_count {
        return Err(format!(
            "{what}: candidate count {:?}, expected {expected_count:?}",
            window.candidate_count()
        )
        .into());
    }
    let continues = cursor_id.is_some();
    if window.has_more() != continues {
        return Err(format!(
            "{what}: has_more={} with cursor={cursor_id:?}",
            window.has_more()
        )
        .into());
    }
    if continues && cursor_id != rows.last().map(String::as_str) {
        return Err(format!("{what}: the cursor names the last row, got {cursor_id:?}").into());
    }
    Ok(())
}

// ---------------------------------------------------------------------
// runtime-metadata
// ---------------------------------------------------------------------

fn runtime_ids(page: &SearchPlaneRuntimeMetadataQueryResponse) -> Vec<String> {
    page.results
        .iter()
        .map(|row| row.candidate_id.clone())
        .collect()
}

fn runtime_page(
    rt: &mut E2eRuntime,
    top_k: u32,
    cursor: Option<RuntimeMetadataCursorV1>,
    what: &str,
) -> Result<SearchPlaneRuntimeMetadataQueryResponse, Box<dyn Error>> {
    Ok(rt
        .query_runtime_metadata_page(TextQuerySyntax::Sourcegraph, RUNTIME_QUERY, top_k, cursor)?
        .served(what)?)
}

#[test]
fn runtime_metadata_pages_walk_the_dirty_overlay_once_and_pin_their_epochs() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let chunks = Chunks::ingest(&mut rt)?;
    let original = chunks.original();
    let late = chunks.late();
    if original.len() != ORIGINAL || original.len().saturating_add(late.len()) != CHUNKS {
        return Err(format!("fixture split drifted: {} / {}", original.len(), late.len()).into());
    }
    for id in &original {
        let (path, _) = chunks.path_and_index(id)?;
        rt.ingest_dirty_for_path(path, 5)?;
    }
    let sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    // The whole set in one page: the oracle for the walk.
    let whole = runtime_page(&mut rt, WHOLE, None, "whole")?;
    check_page(
        "whole",
        &runtime_ids(&whole),
        whole.window,
        &original,
        CandidateCountV1::Exact(u64::try_from(ORIGINAL)?),
        None,
    )?;
    if whole.examined != u64::try_from(ORIGINAL)? {
        return Err(format!(
            "the whole walk examines every dirty row: {}",
            whole.examined
        )
        .into());
    }

    // Page one stops at its probe row.
    let first = runtime_page(&mut rt, PAGE, None, "page one")?;
    let walk_epochs = (first.read_epoch, first.universe_epoch);
    let cursor = first
        .next_cursor
        .clone()
        .ok_or("25 rows continue past page one")?;
    check_page(
        "page one",
        &runtime_ids(&first),
        first.window,
        page_rows(&original, 0..10)?,
        CandidateCountV1::AtLeast(11),
        Some(cursor.candidate_id.as_str()),
    )?;
    if first.examined != 11 {
        return Err(format!(
            "page one examines the page and its probe: {}",
            first.examined
        )
        .into());
    }
    if (cursor.aux_epoch, cursor.universe_epoch) != walk_epochs {
        return Err(format!("the cursor names the epochs page one read: {cursor:?}").into());
    }

    // Five rows that sort into page two land between page one and two.
    for id in &late {
        let (path, _) = chunks.path_and_index(id)?;
        let batch = rt.dirty_batch(path, 6, sealed)?;
        rt.publish_dirty_batch(batch)?;
    }

    // Pages two and three read the walk's epochs: exactly the original
    // rows page one did not return, none of the late ones.
    let second = runtime_page(&mut rt, PAGE, Some(cursor.clone()), "page two")?;
    if (second.read_epoch, second.universe_epoch) != walk_epochs {
        return Err(format!(
            "page two reads the walk's epochs {walk_epochs:?}, read {:?}",
            (second.read_epoch, second.universe_epoch)
        )
        .into());
    }
    let second_cursor = second.next_cursor.clone().ok_or("page two continues")?;
    check_page(
        "page two",
        &runtime_ids(&second),
        second.window,
        page_rows(&original, 10..20)?,
        CandidateCountV1::AtLeast(11),
        Some(second_cursor.candidate_id.as_str()),
    )?;
    let third = runtime_page(&mut rt, PAGE, Some(second_cursor), "page three")?;
    if (third.read_epoch, third.universe_epoch) != walk_epochs {
        return Err("page three reads the walk's epochs".into());
    }
    check_page(
        "page three",
        &runtime_ids(&third),
        third.window,
        page_rows(&original, 20..ORIGINAL)?,
        CandidateCountV1::Exact(5),
        None,
    )?;
    if third.examined != 5 {
        return Err(format!("the last page examines what is left: {}", third.examined).into());
    }
    let mut walked = runtime_ids(&first);
    walked.extend(runtime_ids(&second));
    walked.extend(runtime_ids(&third));
    if walked != original || distinct(&walked).len() != walked.len() {
        return Err(format!("the walk is the original rows once each: {walked:?}").into());
    }
    if walked.iter().any(|id| late.contains(id)) {
        return Err("a row ingested after the walk started leaked into it".into());
    }

    // A fresh walk reads the current epochs and sees all thirty.
    let fresh = runtime_page(&mut rt, WHOLE, None, "fresh whole")?;
    if fresh.read_epoch <= walk_epochs.0 {
        return Err(format!(
            "five ingests advance the runtime epoch past {:?}, fresh read {:?}",
            walk_epochs.0, fresh.read_epoch
        )
        .into());
    }
    check_page(
        "fresh whole",
        &runtime_ids(&fresh),
        fresh.window,
        &chunks.ids(),
        CandidateCountV1::Exact(u64::try_from(CHUNKS)?),
        None,
    )?;

    // `AUX_EPOCH_RETAIN` more overlay mutations push the walk's runtime
    // epoch out of retention: the old cursor is refused typed, never
    // served from the current snapshot.
    for step in 0..AUX_EPOCH_RETAIN {
        let id = original.first().ok_or("the original set is not empty")?;
        let (path, _) = chunks.path_and_index(id)?;
        let batch = rt.dirty_batch(path, 7u64.saturating_add(u64::try_from(step)?), sealed)?;
        rt.publish_dirty_batch(batch)?;
    }
    match rt.query_runtime_metadata_page(
        TextQuerySyntax::Sourcegraph,
        RUNTIME_QUERY,
        PAGE,
        Some(cursor),
    )? {
        E2eRoutePage::Refused(error) if error.code == AUX_EPOCH_EXPIRED_CODE => Ok(()),
        E2eRoutePage::Refused(error) => {
            Err(format!("a pruned epoch is refused {AUX_EPOCH_EXPIRED_CODE}, got {error}").into())
        }
        E2eRoutePage::Served(page) => Err(format!(
            "a pruned epoch must not be served from the current snapshot: {:?}",
            runtime_ids(&page)
        )
        .into()),
    }
}

// ---------------------------------------------------------------------
// structural
// ---------------------------------------------------------------------

fn structural_ids(page: &SearchPlaneStructuralQueryResponse) -> Vec<String> {
    page.results
        .iter()
        .map(|row| row.candidate_id.clone())
        .collect()
}

fn structural_page(
    rt: &mut E2eRuntime,
    top_k: u32,
    cursor: Option<StructuralCursorV1>,
    what: &str,
) -> Result<SearchPlaneStructuralQueryResponse, Box<dyn Error>> {
    Ok(rt
        .query_structural_page(TextQuerySyntax::Native, STRUCTURAL_QUERY, top_k, cursor)?
        .served(what)?)
}

/// Publish the function tree of the chunk `id` into `generation`.
fn publish_tree(
    rt: &mut E2eRuntime,
    chunks: &Chunks,
    id: &str,
    generation: ManifestGeneration,
) -> TestResult {
    let (path, index) = chunks.path_and_index(id)?;
    let tree = E2eRuntime::structural_function_tree_record(
        path,
        &content_of(index),
        &identifier_of(index),
    )?;
    let batch = rt.structural_tree_batch(path, tree, generation)?;
    rt.publish_structural_batch(batch)?;
    Ok(())
}

#[test]
fn structural_pages_walk_the_match_set_once_and_pin_their_epoch() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let chunks = Chunks::ingest(&mut rt)?;
    let original = chunks.original();
    let late = chunks.late();
    for id in &original {
        let (path, index) = chunks.path_and_index(id)?;
        rt.ingest_structural_function_tree(path, &content_of(index), &identifier_of(index))?;
    }
    let sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    // The whole match set in one page: the oracle for the walk.
    let whole = structural_page(&mut rt, WHOLE, None, "whole")?;
    check_page(
        "whole",
        &structural_ids(&whole),
        whole.window,
        &original,
        CandidateCountV1::Exact(u64::try_from(ORIGINAL)?),
        None,
    )?;

    // Page one: the count after the cursor is exact, the whole match set
    // is walked.
    let first = structural_page(&mut rt, PAGE, None, "page one")?;
    let walk_epoch = first.read_epoch;
    let cursor = first
        .next_cursor
        .clone()
        .ok_or("25 matches continue past page one")?;
    check_page(
        "page one",
        &structural_ids(&first),
        first.window,
        page_rows(&original, 0..10)?,
        CandidateCountV1::Exact(u64::try_from(ORIGINAL)?),
        Some(cursor.candidate_id.as_str()),
    )?;
    if first.examined != u64::try_from(ORIGINAL)? {
        return Err(format!("page one walks the whole match set: {}", first.examined).into());
    }
    if cursor.aux_epoch != walk_epoch {
        return Err(format!("the cursor names the epoch page one read: {cursor:?}").into());
    }

    // Five matches that sort into page two land between page one and two.
    for id in &late {
        publish_tree(&mut rt, &chunks, id, sealed)?;
    }

    // Pages two and three are evaluated against the walk's epoch.
    let second = structural_page(&mut rt, PAGE, Some(cursor.clone()), "page two")?;
    if second.read_epoch != walk_epoch {
        return Err(format!(
            "page two reads the walk's epoch {walk_epoch}, read {}",
            second.read_epoch
        )
        .into());
    }
    let second_cursor = second.next_cursor.clone().ok_or("page two continues")?;
    check_page(
        "page two",
        &structural_ids(&second),
        second.window,
        page_rows(&original, 10..20)?,
        CandidateCountV1::Exact(15),
        Some(second_cursor.candidate_id.as_str()),
    )?;
    if second.examined != u64::try_from(ORIGINAL)? {
        return Err(format!(
            "page two walks the walk's match set, not the current one: {}",
            second.examined
        )
        .into());
    }
    let third = structural_page(&mut rt, PAGE, Some(second_cursor), "page three")?;
    if third.read_epoch != walk_epoch {
        return Err("page three reads the walk's epoch".into());
    }
    check_page(
        "page three",
        &structural_ids(&third),
        third.window,
        page_rows(&original, 20..ORIGINAL)?,
        CandidateCountV1::Exact(5),
        None,
    )?;
    let mut walked = structural_ids(&first);
    walked.extend(structural_ids(&second));
    walked.extend(structural_ids(&third));
    if walked != original || distinct(&walked).len() != walked.len() {
        return Err(format!("the walk is the original matches once each: {walked:?}").into());
    }
    if walked.iter().any(|id| late.contains(id)) {
        return Err("a match ingested after the walk started leaked into it".into());
    }

    // A fresh walk reads the current epoch and sees all thirty.
    let fresh = structural_page(&mut rt, WHOLE, None, "fresh whole")?;
    if fresh.read_epoch <= walk_epoch {
        return Err(format!(
            "five ingests advance the structural epoch past {walk_epoch}, fresh read {}",
            fresh.read_epoch
        )
        .into());
    }
    check_page(
        "fresh whole",
        &structural_ids(&fresh),
        fresh.window,
        &chunks.ids(),
        CandidateCountV1::Exact(u64::try_from(CHUNKS)?),
        None,
    )?;

    // `AUX_EPOCH_RETAIN` more structural mutations push the walk's epoch
    // out of retention: the old cursor is refused typed.
    for _step in 0..AUX_EPOCH_RETAIN {
        let id = original.first().ok_or("the original set is not empty")?;
        publish_tree(&mut rt, &chunks, id, sealed)?;
    }
    match rt.query_structural_page(
        TextQuerySyntax::Native,
        STRUCTURAL_QUERY,
        PAGE,
        Some(cursor),
    )? {
        E2eRoutePage::Refused(error) if error.code == AUX_EPOCH_EXPIRED_CODE => Ok(()),
        E2eRoutePage::Refused(error) => {
            Err(format!("a pruned epoch is refused {AUX_EPOCH_EXPIRED_CODE}, got {error}").into())
        }
        E2eRoutePage::Served(page) => Err(format!(
            "a pruned epoch must not be served from the current snapshot: {:?}",
            structural_ids(&page)
        )
        .into()),
    }
}
