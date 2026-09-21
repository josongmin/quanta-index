use std::collections::BTreeSet;
use std::error::Error;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use quanta_index_contract::lex::DirtyRecord;
use quanta_index_contract::{
    CandidateCountV1, ChunkId, DirtyIngestBatch, DirtyMutation, LqExpr, LqFilter, LqLeaf,
    LqPredicateArg, LqQuery, ManifestGeneration, RepoId, RevisionId, RuntimeMetadataCursorV1,
    RuntimeMetadataQueryRequest, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{CoreError, RUNTIME_NOT_READY_CODE, RequestBudgetV1};

use crate::Ledger;
use crate::observability::BoundedQueryObsStore;
use crate::query_dispatcher::routes::runtime_metadata::{
    RuntimeMetadataPage, RuntimeMetadataRead, execute_runtime_metadata_query,
    runtime_generation_is_stale, validate_runtime_metadata_query,
};
use crate::query_dispatcher::tests::support::common::{
    TestResult, dispatcher_with_obs, ipc_error_from, manual_query, ready_ledger, ready_pin,
};
use crate::query_dispatcher::tests::support::lexical::RejectLexicalOpener;
use crate::query_dispatcher::tests::support::runtime_metadata::{
    ready_runtime_metadata_ledger, runtime_metadata_dispatcher_with_ledger, runtime_query_request,
};
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;
use crate::query_dispatcher::tests::support::structural::install_structural_test_chunk;

#[test]
fn runtime_metadata_dispatch_not_ready_emits_closed_obs_metric() -> TestResult {
    let obs_sink = Arc::new(BoundedQueryObsStore::default());
    let dispatcher = dispatcher_with_obs(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        obs_sink.clone(),
    )?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "changed:since=1970-01-01T00:00:00.010Z runtime".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 5,
                cursor: None,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );
    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != RUNTIME_NOT_READY_CODE {
        return Err(format!("expected {RUNTIME_NOT_READY_CODE}, got {code}").into());
    }
    let names = obs_sink
        .snapshot()
        .into_iter()
        .map(|sample| sample.name.into_string())
        .collect::<Vec<_>>();
    let expected = vec![
        "lq_query_intake_total".to_string(),
        "lq_typed_error_not_ready_total".to_string(),
        "lq_route_runtime_metadata_latency_ms".to_string(),
        "lq_route_runtime_metadata_errors_total".to_string(),
    ];
    if names != expected {
        return Err(format!("unexpected runtime-metadata obs metric names: {names:?}").into());
    }
    let errors = obs_sink.errors();
    if !errors.is_empty() {
        return Err(format!("unexpected runtime-metadata obs errors: {errors:?}").into());
    }
    Ok(())
}

#[test]
fn runtime_metadata_dispatch_dirty_only_executes_like_dirty_yes() -> TestResult {
    let dispatcher =
        runtime_metadata_dispatcher_with_ledger(ready_runtime_metadata_ledger(100, 20))?;
    let response = dispatcher.dispatch(
        runtime_query_request(TextQuerySyntax::Native, "dirty:only todo"),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::RuntimeMetadata(response) = response else {
        return Err("expected RuntimeMetadata response".into());
    };
    let candidate_ids = response
        .results
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect::<Vec<_>>();
    if candidate_ids != ["chunk-dirty"] {
        return Err(format!("expected [\"chunk-dirty\"], got {candidate_ids:?}").into());
    }
    // The page names the runtime authority's epoch (QI-BB-020 W2): the
    // fixture applies one dirty batch and one catalog batch, so the
    // runtime domain is at epoch 2 — not the structural domain's epoch,
    // which the six chunk installs advanced further.
    if response.read_epoch != quanta_index_contract::AuxEpochV1::new(2) {
        return Err(format!(
            "the runtime page reads the runtime epoch 2, got {:?}",
            response.read_epoch
        )
        .into());
    }
    // The chunk universe it was joined from is named too (QI-BB-025 W4):
    // the six installs are structural epoch 6.
    if response.universe_epoch != quanta_index_contract::AuxEpochV1::new(6) {
        return Err(format!(
            "the runtime page names the structural epoch 6 of its universe, got {:?}",
            response.universe_epoch
        )
        .into());
    }
    if response.examined != 1 || response.next_cursor.is_some() {
        return Err(format!(
            "one dirty chunk is examined and the page is final: examined={} cursor={:?}",
            response.examined, response.next_cursor
        )
        .into());
    }
    Ok(())
}

#[test]
fn runtime_metadata_dispatch_rejects_predicate_leaf_typed_error() -> TestResult {
    let dispatcher = runtime_metadata_dispatcher_with_ledger(ready_ledger())?;
    let response = dispatcher.dispatch(
        runtime_query_request(
            TextQuerySyntax::Native,
            "changed:since=1970-01-01T00:00:00.010Z file.contains('catalog_changed_needle')",
        ),
        &RequestBudgetV1::unbounded(),
    );
    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::NotImplemented {
        return Err(format!("expected NOT_IMPLEMENTED, got {code}").into());
    }
    if !message.contains("runtime metadata: predicate leaves are not executable") {
        return Err(format!("unexpected predicate-leaf rejection message: {message}").into());
    }
    Ok(())
}

#[test]
fn runtime_metadata_validate_rejects_content_predicate_leaf_upfront() -> TestResult {
    let query = manual_query(
        LqExpr::Leaf(LqLeaf::Keyword("catalog".to_string())),
        vec![
            LqFilter::Changed {
                scope: "since=1970-01-01T00:00:00.010Z".to_string(),
            },
            LqFilter::Content {
                leaf: LqLeaf::Predicate {
                    name: "file.contains".to_string(),
                    args: vec![LqPredicateArg::RawString("catalog".to_string())],
                },
            },
        ],
    );
    match validate_runtime_metadata_query(&query) {
        Err(CoreError::NotImplemented(message))
            if message.contains("runtime metadata: predicate leaves are not executable") =>
        {
            Ok(())
        }
        other => Err(format!("expected predicate content reject, got {other:?}").into()),
    }
}

#[test]
fn runtime_generation_is_stale_requires_producer_head_ahead() -> TestResult {
    let ledger = ready_runtime_metadata_ledger(20, 20);
    let guard = ledger
        .read()
        .map_err(|err| format!("runtime metadata test ledger poisoned: {err}"))?;
    let runtime = guard
        .runtime_state(
            &RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(9),
        )
        .ok_or("missing runtime metadata state")?;
    let is_stale = runtime_generation_is_stale(runtime, 30)?;
    drop(guard);
    if is_stale {
        return Err(
            "stale relation unexpectedly matched when producer head did not advance".into(),
        );
    }
    Ok(())
}

/// The two snapshots the route's read view pins for the fixture
/// generation, read straight from the ledger under one guard: the
/// current epochs, or the two a `cursor` names.
fn read_at(
    ledger: &Arc<RwLock<Ledger>>,
    cursor: Option<&RuntimeMetadataCursorV1>,
) -> Result<RuntimeMetadataRead, CoreError> {
    let guard = ledger.read().map_err(|_poisoned| {
        CoreError::Storage("runtime metadata test ledger poisoned".to_string())
    })?;
    let pin = ready_pin();
    let now = Instant::now();
    let runtime = guard
        .runtime_read_at(
            &pin.repo_id,
            &pin.revision_id,
            pin.manifest_generation,
            cursor.map(|cursor| cursor.aux_epoch),
            now,
        )?
        .ok_or_else(|| CoreError::Storage("runtime state missing".to_string()))?;
    let universe = guard
        .structural_read_at(
            &pin.repo_id,
            &pin.revision_id,
            pin.manifest_generation,
            cursor.map(|cursor| cursor.universe_epoch),
            now,
        )?
        .ok_or_else(|| CoreError::Storage("structural state missing".to_string()))?;
    drop(guard);
    Ok(RuntimeMetadataRead { runtime, universe })
}

/// The read of the fixture generation: both snapshots, the current
/// epochs, taken under one guard.
fn read_current(ledger: &Arc<RwLock<Ledger>>) -> Result<RuntimeMetadataRead, Box<dyn Error>> {
    Ok(read_at(ledger, None)?)
}

/// The read a continuation makes: the two epochs `cursor` names.
fn read_at_cursor(
    ledger: &Arc<RwLock<Ledger>>,
    cursor: &RuntimeMetadataCursorV1,
) -> Result<RuntimeMetadataRead, CoreError> {
    read_at(ledger, Some(cursor))
}

fn lowered(query_text: &str) -> Result<LqQuery, CoreError> {
    crate::lower_lexical_text_query(&TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: query_text.to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(ready_pin()),
        generation_selector: None,
        top_k: 5,
        cursor: None,
    })
}

/// One page of `top_k` after `cursor` over `read`.
fn page(
    read: &RuntimeMetadataRead,
    query: &LqQuery,
    top_k: u32,
    cursor: Option<&RuntimeMetadataCursorV1>,
) -> Result<RuntimeMetadataPage, CoreError> {
    execute_runtime_metadata_query(
        &ready_pin(),
        query,
        &read.runtime.state,
        &read.universe.state,
        read.epochs(),
        top_k,
        cursor,
    )
}

fn ids(page: &RuntimeMetadataPage) -> Vec<String> {
    page.results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect()
}

/// The dirty batch marking `chunk_ids` dirty in the fixture generation.
fn dirty_batch(digest: &str, chunk_ids: &[&str]) -> DirtyIngestBatch {
    DirtyIngestBatch {
        repo_id: RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-map-ipc")
            .expect("static fixture ID satisfies canonical policy"),
        generation: ManifestGeneration::new(9),
        overlay_epoch_ms: 100,
        batch_digest: digest.to_string(),
        entries: chunk_ids
            .iter()
            .map(|chunk_id| {
                DirtyMutation::Upsert(DirtyRecord {
                    wire_version: 1,
                    doc_id: ChunkId::new(*chunk_id),
                    applied_at_ms: 100,
                    payload_hash: [0x5a; 32],
                })
            })
            .collect(),
    }
}

/// Mutate the ledger under its write lock, released as `mutate` returns.
fn mutate_ledger(
    ledger: &Arc<RwLock<Ledger>>,
    mutate: impl FnOnce(&mut Ledger) -> TestResult,
) -> TestResult {
    let mut guard = ledger
        .write()
        .map_err(|err| format!("runtime metadata test ledger poisoned: {err}"))?;
    mutate(&mut guard)
}

/// A generation of `count` chunks named `chunk-NN`, every one dirty.
fn ledger_with_dirty_chunks(count: u32) -> Result<Arc<RwLock<Ledger>>, Box<dyn Error>> {
    let ledger = ready_ledger();
    let names: Vec<String> = (0..count)
        .map(|index| format!("chunk-{index:02}"))
        .collect();
    mutate_ledger(&ledger, |ledger| {
        for name in &names {
            install_structural_test_chunk(ledger, name, &format!("src/{name}.rs"), "todo walk")?;
        }
        let dirty: Vec<&str> = names.iter().map(String::as_str).collect();
        ledger.apply_runtime_batch(&dirty_batch("dirty:all", &dirty), Instant::now())?;
        Ok(())
    })?;
    Ok(ledger)
}

/// The narrowest authority set drives the walk, but the complete
/// predicate decides membership.
///
/// A changed-docs walk yields the changed chunk, a `dirty:no` walk over
/// the universe excludes the dirty one, an edge-authority walk yields
/// exactly the edge's chunk.
#[test]
fn runtime_drivers_narrow_the_walk_without_changing_its_meaning() -> TestResult {
    let ledger = ready_runtime_metadata_ledger(100, 20);
    let read = read_current(&ledger)?;
    let cases = [
        (
            "changed:since=1970-01-01T00:00:00.010Z catalog",
            vec!["chunk-changed"],
        ),
        ("dirty:no todo", vec!["chunk-clean"]),
        ("affected:rebuild=lexical catalog", vec!["chunk-changed"]),
        ("snapshot:active catalog", vec!["chunk-snapshot"]),
        ("meta.owner:team-a catalog", vec!["chunk-owner"]),
        ("dirty:only todo", vec!["chunk-dirty"]),
    ];
    for (query_text, expected) in cases {
        let query = lowered(query_text)?;
        let first = page(&read, &query, 5, None)?;
        if ids(&first) != expected {
            return Err(
                format!("`{query_text}` yields {expected:?}, got {:?}", ids(&first)).into(),
            );
        }
        if first.window.has_more() || first.next_cursor.is_some() {
            return Err(format!("`{query_text}` fits one page: {:?}", first.window).into());
        }
        if first.window.candidate_count() != CandidateCountV1::Exact(1) {
            return Err(format!(
                "`{query_text}` exhausts its stream with an exact count, got {:?}",
                first.window
            )
            .into());
        }
    }
    Ok(())
}

/// Pages of `top_k` partition the matching chunks in candidate-id order
/// with no gap and no overlap.
///
/// Every page but the last stops at its probe row with a lower bound,
/// the last exhausts the stream with an exact count and no cursor, and
/// each page examines exactly the rows after its cursor up to the probe.
#[test]
fn pages_partition_the_dirty_overlay_in_candidate_id_order_without_gaps_or_overlap() -> TestResult {
    const ROWS: u32 = 23;
    const TOP_K: u32 = 5;
    let ledger = ledger_with_dirty_chunks(ROWS)?;
    let read = read_current(&ledger)?;
    let query = lowered("dirty:only walk")?;
    let mut walked: Vec<String> = Vec::new();
    let mut examined: Vec<u64> = Vec::new();
    let mut cursor: Option<RuntimeMetadataCursorV1> = None;
    for _page in 0..8 {
        let current = page(&read, &query, TOP_K, cursor.as_ref())?;
        examined.push(current.examined);
        walked.extend(ids(&current));
        match (current.window.has_more(), current.next_cursor) {
            (true, Some(next)) => {
                if current.window.candidate_count() != CandidateCountV1::AtLeast(6)
                    || current.window.returned() != TOP_K
                {
                    return Err(format!(
                        "a full page stops at its probe with a lower bound: {:?}",
                        current.window
                    )
                    .into());
                }
                if Some(next.candidate_id.as_str()) != walked.last().map(String::as_str) {
                    return Err(format!("the cursor is the last row: {next:?}").into());
                }
                if next.aux_epoch != read.epochs().runtime
                    || next.universe_epoch != read.epochs().universe
                {
                    return Err(format!("the cursor names both epochs read: {next:?}").into());
                }
                cursor = Some(next);
            }
            (false, None) => {
                if current.window.candidate_count() != CandidateCountV1::Exact(3) {
                    return Err(format!(
                        "the last page exhausts the stream exactly: {:?}",
                        current.window
                    )
                    .into());
                }
                break;
            }
            (has_more, next) => {
                return Err(format!("has_more={has_more} and cursor={next:?} disagree").into());
            }
        }
    }
    let expected: Vec<String> = (0..ROWS).map(|index| format!("chunk-{index:02}")).collect();
    if walked != expected {
        return Err(format!("the walk is every row once, in order: {walked:?}").into());
    }
    // Each page visits the rows after its cursor up to and including the
    // probe row, which the next page visits again; the last page visits
    // what is left.
    if examined != [6, 6, 6, 6, 3] {
        return Err(format!("examined per page drifted: {examined:?}").into());
    }
    Ok(())
}

/// A cursor key that names no row is a boundary, not a lookup: the page
/// after it starts at the first row past it.
#[test]
fn a_forged_cursor_key_is_a_boundary_not_a_lookup() -> TestResult {
    let ledger = ledger_with_dirty_chunks(6)?;
    let read = read_current(&ledger)?;
    let query = lowered("dirty:only walk")?;
    let forged = RuntimeMetadataCursorV1 {
        // Sorts between `chunk-02` and `chunk-03`; no chunk has this id.
        candidate_id: "chunk-02-and-a-half".to_string(),
        aux_epoch: read.epochs().runtime,
        universe_epoch: read.epochs().universe,
    };
    let current = page(&read, &query, 2, Some(&forged))?;
    if ids(&current) != ["chunk-03", "chunk-04"] {
        return Err(format!("the page after the boundary: {:?}", ids(&current)).into());
    }
    if current.window.candidate_count() != CandidateCountV1::AtLeast(3)
        || !current.window.has_more()
    {
        return Err(format!("three rows follow the boundary: {:?}", current.window).into());
    }
    let past_the_end = RuntimeMetadataCursorV1 {
        candidate_id: "chunk-99".to_string(),
        ..forged
    };
    let empty = page(&read, &query, 2, Some(&past_the_end))?;
    if !empty.results.is_empty()
        || empty.window.candidate_count() != CandidateCountV1::Exact(0)
        || empty.next_cursor.is_some()
    {
        return Err(format!("nothing follows a boundary past the end: {empty:?}").into());
    }
    Ok(())
}

/// A continuation reads the two epochs its cursor names.
///
/// The pages of one walk partition the row set of that cut even when a
/// chunk install (universe epoch) and a dirty ingest (runtime epoch)
/// land between them; a fresh walk sees both; a cursor whose epoch has
/// been pruned — in either domain — is refused, never served from a
/// newer snapshot.
#[test]
fn a_continuation_reads_the_epochs_its_cursor_names() -> TestResult {
    use quanta_index_core::{AUX_EPOCH_EXPIRED_CODE, AUX_EPOCH_RETAIN};

    // Ten chunks, the even ones dirty: an overlay walk (`dirty:only`)
    // and a universe walk (`dirty:no`) of five rows each.
    let ledger = ready_ledger();
    let names: Vec<String> = (0..10).map(|index| format!("chunk-{index:02}")).collect();
    mutate_ledger(&ledger, |ledger| {
        for name in &names {
            install_structural_test_chunk(ledger, name, &format!("src/{name}.rs"), "todo walk")?;
        }
        let even: Vec<&str> = names.iter().step_by(2).map(String::as_str).collect();
        ledger.apply_runtime_batch(&dirty_batch("dirty:even", &even), Instant::now())?;
        Ok(())
    })?;
    let dirty_rows: BTreeSet<String> = names.iter().step_by(2).cloned().collect();
    let clean_rows: BTreeSet<String> = names.iter().skip(1).step_by(2).cloned().collect();
    let read = read_current(&ledger)?;
    let walk_epochs = read.epochs();

    let overlay_query = lowered("dirty:only walk")?;
    let universe_query = lowered("dirty:no walk")?;
    let overlay_first = page(&read, &overlay_query, 2, None)?;
    let universe_first = page(&read, &universe_query, 2, None)?;
    if ids(&overlay_first) != ["chunk-00", "chunk-02"]
        || ids(&universe_first) != ["chunk-01", "chunk-03"]
    {
        return Err(format!(
            "page one of each walk: {:?} / {:?}",
            ids(&overlay_first),
            ids(&universe_first)
        )
        .into());
    }
    let overlay_cursor = overlay_first
        .next_cursor
        .clone()
        .ok_or("five rows continue")?;
    let universe_cursor = universe_first
        .next_cursor
        .clone()
        .ok_or("five rows continue")?;

    // Between page one and page two: a clean chunk that would sort into
    // the universe walk's page two (universe epoch advances), and a dirty
    // chunk that would sort into the overlay walk's page two (both
    // epochs advance).
    mutate_ledger(&ledger, |ledger| {
        install_structural_test_chunk(ledger, "chunk-04b", "src/chunk-04b.rs", "todo walk")?;
        install_structural_test_chunk(ledger, "chunk-04c", "src/chunk-04c.rs", "todo walk")?;
        ledger.apply_runtime_batch(&dirty_batch("dirty:late", &["chunk-04c"]), Instant::now())?;
        Ok(())
    })?;

    for (label, query, first, cursor, expected, late) in [
        (
            "overlay",
            &overlay_query,
            overlay_first,
            overlay_cursor.clone(),
            &dirty_rows,
            "chunk-04c",
        ),
        (
            "universe",
            &universe_query,
            universe_first,
            universe_cursor.clone(),
            &clean_rows,
            "chunk-04b",
        ),
    ] {
        let mut walked = ids(&first);
        let mut next = Some(cursor);
        while let Some(cursor) = next.take() {
            let read = read_at_cursor(&ledger, &cursor)?;
            if read.epochs() != walk_epochs {
                return Err(format!(
                    "{label}: a continuation reads its own epochs {walk_epochs:?}, read {:?}",
                    read.epochs()
                )
                .into());
            }
            let current = page(&read, query, 2, Some(&cursor))?;
            walked.extend(ids(&current));
            next = current.next_cursor;
        }
        let distinct: BTreeSet<String> = walked.iter().cloned().collect();
        if distinct.len() != walked.len() {
            return Err(format!("{label}: a row appeared on two pages: {walked:?}").into());
        }
        if &distinct != expected {
            return Err(format!(
                "{label}: the walk must yield the first cut's rows exactly, got {walked:?}"
            )
            .into());
        }
        // A fresh walk reads the current cut and sees the late chunk.
        let fresh = read_current(&ledger)?;
        if fresh.epochs().runtime <= walk_epochs.runtime
            || fresh.epochs().universe <= walk_epochs.universe
        {
            return Err("the late ingests advanced both epochs".into());
        }
        let whole = page(&fresh, query, 10, None)?;
        if !ids(&whole).iter().any(|id| id == late) || whole.results.len() != 6 {
            return Err(format!("{label}: a fresh walk sees six rows: {:?}", ids(&whole)).into());
        }
    }

    // `AUX_EPOCH_RETAIN` more mutations in each domain push the walk's
    // epochs out of retention: both cursors are refused expired, the
    // overlay one for its runtime epoch, the universe one for its
    // structural epoch.
    mutate_ledger(&ledger, |ledger| {
        for step in 0..AUX_EPOCH_RETAIN {
            ledger.apply_runtime_batch(
                &dirty_batch(&format!("dirty:churn-{step}"), &["chunk-00"]),
                Instant::now(),
            )?;
            let name = format!("chunk-churn-{step:02}");
            install_structural_test_chunk(ledger, &name, &format!("src/{name}.rs"), "churn")?;
        }
        Ok(())
    })?;
    for (label, cursor) in [("overlay", overlay_cursor), ("universe", universe_cursor)] {
        match read_at_cursor(&ledger, &cursor) {
            Err(CoreError::Typed { code, .. }) if code == AUX_EPOCH_EXPIRED_CODE => {}
            other => {
                return Err(
                    format!("{label}: a pruned epoch is refused expired, got {other:?}").into(),
                );
            }
        }
    }
    Ok(())
}
