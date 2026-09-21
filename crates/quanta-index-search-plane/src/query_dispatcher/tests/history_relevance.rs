//! QI-BB-023 follow-up #1 — the history route's `relevance` order at the
//! route level.
//!
//! The oracle is an in-memory index double whose BM25 is independent of
//! the engine; the epochs are published through the real history
//! materializer, index and rows together.

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use quanta_index_contract::lex::{CommitRecord, CommitSha};
use quanta_index_contract::{
    AuxEpochV1, CandidateCountV1, HistoryCursor, HistoryCursorOrderV1, HistoryIngestBatch,
    HistoryOrderV1, HistoryQueryRequest, SearchPlaneHistoryQueryResponse,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{
    AUX_EPOCH_EXPIRED_CODE, AUX_EPOCH_RETAIN, AuxiliaryGenerationKeyV1, CoreError,
    HISTORY_TEXT_QUERY_UNSCORABLE_CODE, HistoryTextDocKeyV1, HistoryTextDocV1, RequestBudgetV1,
};

use crate::Ledger;
use crate::auxiliary_authority::testing::MemoryAuxiliaryCatalog;
use crate::history_text::HistoryTextIndexParts;
use crate::ingest_dispatcher::{
    AuxiliaryMaterializerParts, AuxiliaryMutationCoordinator, DirectHistoryMaterializer,
    HistoryIngestPort as _,
};
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::errors::{
    ERR_HISTORY_CURSOR_ORDER_MISMATCH, ERR_HISTORY_RELEVANCE_UNAVAILABLE,
};
use crate::query_dispatcher::tests::support::common::{
    TestResult, ready_ledger, ready_pin, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::history_text::{
    MemoryHistoryTextIndex, reference_bm25, tokens,
};
use crate::query_dispatcher::tests::support::lexical::RejectLexicalOpener;
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapQueryPort;
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

fn sha(byte: u8) -> CommitSha {
    CommitSha::from_bytes([byte; 20])
}

fn commit(byte: u8, time: u64, author: &str, message: &str) -> CommitRecord {
    CommitRecord {
        wire_version: 1,
        sha: sha(byte),
        parents: Vec::new(),
        author_time_ms: time,
        committer_time_ms: time,
        applied_at_ms: time,
        author: author.into(),
        author_name: None,
        author_email: None,
        committer: author.into(),
        committer_name: None,
        committer_email: None,
        message: message.into(),
        is_merge: false,
        tags: Vec::new(),
    }
}

/// The fixture: the newest commit mentions `needle` once in a long
/// message, older ones mention it more densely, so recency and relevance
/// disagree on the head of the list.
fn fixture() -> Vec<CommitRecord> {
    vec![
        commit(1, 100, "alice", "needle needle needle"),
        commit(2, 200, "bob", "needle needle in a haystack of words"),
        commit(
            3,
            300,
            "alice",
            "a long message about many things with one needle in it somewhere",
        ),
        commit(4, 400, "bob", "nothing to see here"),
        commit(5, 500, "alice", "needle"),
        commit(
            6,
            600,
            "bob",
            "the newest one mentions the needle once among a few other words",
        ),
    ]
}

fn generation_key() -> AuxiliaryGenerationKeyV1 {
    let pin = ready_pin();
    AuxiliaryGenerationKeyV1 {
        repo_id: pin.repo_id,
        revision_id: pin.revision_id,
        generation: pin.manifest_generation,
    }
}

fn batch(digest: &str, commits: Vec<CommitRecord>) -> HistoryIngestBatch {
    let pin = ready_pin();
    HistoryIngestBatch {
        repo_id: pin.repo_id,
        revision_id: pin.revision_id,
        generation: pin.manifest_generation,
        manifest_digest: None,
        batch_digest: digest.to_string(),
        commits,
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    }
}

/// A ledger, the materializer that publishes epochs and their indexes into
/// it, the index double, and a dispatcher over all three.
struct Plane {
    ledger: Arc<RwLock<Ledger>>,
    materializer: DirectHistoryMaterializer,
    index: Arc<MemoryHistoryTextIndex>,
    history_text: HistoryTextIndexParts,
    dispatcher: SearchPlaneDispatcher,
}

fn plane() -> Result<Plane, Box<dyn std::error::Error>> {
    let ledger = ready_ledger();
    let index = MemoryHistoryTextIndex::shared();
    let history_text = HistoryTextIndexParts::new(index.clone());
    let parts = AuxiliaryMaterializerParts {
        catalog: Arc::new(MemoryAuxiliaryCatalog::default()),
        coordinator: AuxiliaryMutationCoordinator::shared(),
        ledger: Arc::clone(&ledger),
    };
    let materializer =
        DirectHistoryMaterializer::new(parts).with_history_text(history_text.clone());
    let dispatcher = dispatcher_over(Arc::clone(&ledger))?.with_history_text(history_text.clone());
    Ok(Plane {
        ledger,
        materializer,
        index,
        history_text,
        dispatcher,
    })
}

fn dispatcher_over(
    ledger: Arc<RwLock<Ledger>>,
) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>> {
    Ok(SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ledger,
        test_activation_catalog()?,
    ))
}

fn request(
    query_text: &str,
    order: HistoryOrderV1,
    top_k: u32,
    cursor: Option<HistoryCursor>,
) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: query_text.to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k,
            cursor: None,
        },
        order,
        cursor,
    })
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every response that is not a history page or an error is the same test failure"
)]
fn page(
    dispatcher: &SearchPlaneDispatcher,
    query_text: &str,
    order: HistoryOrderV1,
    top_k: u32,
    cursor: Option<HistoryCursor>,
) -> Result<SearchPlaneHistoryQueryResponse, String> {
    match dispatcher.dispatch(
        request(query_text, order, top_k, cursor),
        &RequestBudgetV1::unbounded(),
    ) {
        SearchPlaneQueryIpcResponse::History(page) => Ok(page),
        SearchPlaneQueryIpcResponse::Error(err) => Err(format!("{}: {}", err.code, err.message)),
        other => Err(format!("unexpected response {other:?}")),
    }
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every response that is not an error is the same test failure"
)]
fn error_code(
    dispatcher: &SearchPlaneDispatcher,
    query_text: &str,
    order: HistoryOrderV1,
    cursor: Option<HistoryCursor>,
) -> Result<quanta_index_contract::SearchPlaneErrorCodeV2, String> {
    match dispatcher.dispatch(
        request(query_text, order, 3, cursor),
        &RequestBudgetV1::unbounded(),
    ) {
        SearchPlaneQueryIpcResponse::Error(err) => Ok(err.code),
        other => Err(format!("expected a typed error, got {other:?}")),
    }
}

/// The relevance ranking of the fixture for `terms`, from the oracle:
/// score descending, then time descending, then sha ascending.
fn reference_ranking(commits: &[CommitRecord], terms: &[&str]) -> Vec<(CommitSha, f32)> {
    let docs: Vec<HistoryTextDocV1> = commits
        .iter()
        .map(|record| HistoryTextDocV1 {
            key: HistoryTextDocKeyV1::Commit { sha: record.sha },
            committer_time_ms: record.committer_time_ms,
            text: record.message.to_string(),
        })
        .collect();
    let refs: Vec<&HistoryTextDocV1> = docs.iter().collect();
    let terms: Vec<String> = terms.iter().flat_map(|term| tokens(term)).collect();
    let mut ranked = reference_bm25(&refs, &terms);
    ranked.sort_by(|left, right| {
        right
            .2
            .total_cmp(&left.2)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| left.0.sha().cmp(&right.0.sha()))
    });
    ranked
        .into_iter()
        .map(|(key, _time, score)| (key.sha(), score))
        .collect()
}

#[test]
fn relevance_ranks_by_score_and_recency_by_time_on_one_fixture() -> TestResult {
    let plane = plane()?;
    let _receipt = plane
        .materializer
        .publish_batch(&batch("fixture", fixture()))?;
    let reference = reference_ranking(&fixture(), &["needle"]);
    if reference.len() != 5 {
        return Err(format!("five commits mention needle: {reference:?}").into());
    }

    let relevance = page(
        &plane.dispatcher,
        "type:commit needle",
        HistoryOrderV1::Relevance,
        3,
        None,
    )?;
    if relevance.order != HistoryOrderV1::Relevance {
        return Err("the page echoes the order it was cut under".into());
    }
    let observed: Vec<(CommitSha, f32)> = relevance
        .commits
        .iter()
        .map(|row| {
            row.score
                .map(|score| (row.sha, score.get()))
                .ok_or("a relevance row carries its score")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected: Vec<(CommitSha, f32)> = reference.iter().take(3).copied().collect();
    if observed.len() != 3
        || observed
            .iter()
            .zip(&expected)
            .any(|((sha, score), (want_sha, want))| {
                sha != want_sha || score.to_bits() != want.to_bits()
            })
    {
        return Err(format!(
            "relevance top-3 must be the oracle's top-3:\n  route  {observed:?}\n  oracle {expected:?}"
        )
        .into());
    }
    if relevance.window.candidate_count() != CandidateCountV1::Exact(5)
        || !relevance.window.has_more()
        || relevance.examined != 5
    {
        return Err(format!("the relevance window is exact: {relevance:?}").into());
    }
    match relevance.next_cursor {
        Some(HistoryCursor {
            order: HistoryCursorOrderV1::Relevance { score },
            sha,
            aux_epoch,
            ..
        }) if aux_epoch == AuxEpochV1::new(1) => {
            let last = expected.last().ok_or("three rows")?;
            if sha != last.0 || score.get().to_bits() != last.1.to_bits() {
                return Err("the relevance cursor names the last row with its score".into());
            }
        }
        other => return Err(format!("a relevance cursor is expected, got {other:?}").into()),
    }

    let recency = page(
        &plane.dispatcher,
        "type:commit needle",
        HistoryOrderV1::Recency,
        3,
        None,
    )?;
    let newest: Vec<CommitSha> = recency.commits.iter().map(|row| row.sha).collect();
    if newest != vec![sha(6), sha(5), sha(3)] {
        return Err(format!("recency serves the newest matches first: {newest:?}").into());
    }
    if recency.commits.iter().any(|row| row.score.is_some())
        || recency.order != HistoryOrderV1::Recency
    {
        return Err("a recency page carries no scores".into());
    }
    if observed.first().map(|(sha, _)| *sha) == newest.first().copied() {
        return Err("the fixture must make the two orders disagree at the head".into());
    }
    Ok(())
}

#[test]
fn a_cursor_of_the_other_order_is_refused_typed() -> TestResult {
    let plane = plane()?;
    let _receipt = plane
        .materializer
        .publish_batch(&batch("fixture", fixture()))?;
    let relevance = page(
        &plane.dispatcher,
        "type:commit needle",
        HistoryOrderV1::Relevance,
        2,
        None,
    )?;
    let relevance_cursor = relevance.next_cursor.ok_or("relevance continues")?;
    let recency = page(
        &plane.dispatcher,
        "type:commit needle",
        HistoryOrderV1::Recency,
        2,
        None,
    )?;
    let recency_cursor = recency.next_cursor.ok_or("recency continues")?;

    let code = error_code(
        &plane.dispatcher,
        "type:commit needle",
        HistoryOrderV1::Recency,
        Some(relevance_cursor.clone()),
    )?;
    if code != ERR_HISTORY_CURSOR_ORDER_MISMATCH {
        return Err(
            format!("a relevance cursor on a recency walk is refused typed, got {code}").into(),
        );
    }
    let code = error_code(
        &plane.dispatcher,
        "type:commit needle",
        HistoryOrderV1::Relevance,
        Some(recency_cursor.clone()),
    )?;
    if code != ERR_HISTORY_CURSOR_ORDER_MISMATCH {
        return Err(
            format!("a recency cursor on a relevance walk is refused typed, got {code}").into(),
        );
    }
    // Each cursor continues its own order.
    let second = page(
        &plane.dispatcher,
        "type:commit needle",
        HistoryOrderV1::Relevance,
        2,
        Some(relevance_cursor),
    )?;
    if second.commits.len() != 2 || second.order != HistoryOrderV1::Relevance {
        return Err(format!("the relevance walk continues: {second:?}").into());
    }
    let second = page(
        &plane.dispatcher,
        "type:commit needle",
        HistoryOrderV1::Recency,
        2,
        Some(recency_cursor),
    )?;
    if second.commits.len() != 2 || second.order != HistoryOrderV1::Recency {
        return Err(format!("the recency walk continues: {second:?}").into());
    }
    Ok(())
}

#[test]
fn relevance_pages_partition_the_ranking_and_a_pruned_epoch_is_expired() -> TestResult {
    let plane = plane()?;
    let _receipt = plane
        .materializer
        .publish_batch(&batch("fixture", fixture()))?;
    let reference: Vec<CommitSha> = reference_ranking(&fixture(), &["needle"])
        .into_iter()
        .map(|(sha, _score)| sha)
        .collect();
    let mut walked: Vec<CommitSha> = Vec::new();
    let mut cursor: Option<HistoryCursor> = None;
    let mut first_cursor: Option<HistoryCursor> = None;
    for _page in 0..8 {
        let page = page(
            &plane.dispatcher,
            "type:commit needle",
            HistoryOrderV1::Relevance,
            2,
            cursor.clone(),
        )?;
        let remaining = u64::try_from(reference.len().saturating_sub(walked.len()))?;
        if page.window.candidate_count() != CandidateCountV1::Exact(remaining) {
            return Err(format!(
                "each page counts exactly the admitted rows after its cursor: {:?} with {remaining} remaining",
                page.window
            )
            .into());
        }
        walked.extend(page.commits.iter().map(|row| row.sha));
        match (page.window.has_more(), page.next_cursor) {
            (true, Some(next)) => {
                if first_cursor.is_none() {
                    first_cursor = Some(next.clone());
                }
                cursor = Some(next);
            }
            (false, None) => break,
            (has_more, next) => {
                return Err(format!("has_more={has_more} and cursor={next:?} disagree").into());
            }
        }
    }
    if walked != reference {
        return Err(format!(
            "pages must partition the oracle's ranking in order:\n  walked {walked:?}\n  oracle {reference:?}"
        )
        .into());
    }
    let distinct: BTreeSet<CommitSha> = walked.iter().copied().collect();
    if distinct.len() != walked.len() {
        return Err("a commit appeared on two pages".into());
    }

    // An ingest between pages does not move a walk in flight: the
    // continuation is served at its epoch, whose index is unchanged.
    let first_cursor = first_cursor.ok_or("the walk had more than one page")?;
    let _receipt = plane.materializer.publish_batch(&batch(
        "late",
        vec![commit(9, 900, "carol", "needle needle needle needle")],
    ))?;
    let continued = page(
        &plane.dispatcher,
        "type:commit needle",
        HistoryOrderV1::Relevance,
        2,
        Some(first_cursor.clone()),
    )?;
    let continued_shas: Vec<CommitSha> = continued.commits.iter().map(|row| row.sha).collect();
    if continued_shas != reference.get(2..4).ok_or("four rows")?
        || continued.read_epoch != AuxEpochV1::new(1)
    {
        return Err(format!("the continuation is cut from epoch 1's index: {continued:?}").into());
    }
    let fresh = page(
        &plane.dispatcher,
        "type:commit needle",
        HistoryOrderV1::Relevance,
        1,
        None,
    )?;
    if fresh.read_epoch != AuxEpochV1::new(2)
        || fresh.commits.first().map(|row| row.sha) != Some(sha(9))
    {
        return Err(format!(
            "a fresh walk reads epoch 2 and ranks the new commit first: {fresh:?}"
        )
        .into());
    }

    // `AUX_EPOCH_RETAIN` more mutations prune epoch 1: its index is
    // discarded (no reader holds it) and the cursor is refused expired.
    for step in 0..AUX_EPOCH_RETAIN {
        let byte = u8::try_from(step.saturating_add(20))?;
        let _receipt = plane.materializer.publish_batch(&batch(
            &format!("more-{step}"),
            vec![commit(byte, 1_000, "dave", "unrelated")],
        ))?;
    }
    let code = error_code(
        &plane.dispatcher,
        "type:commit needle",
        HistoryOrderV1::Relevance,
        Some(first_cursor),
    )?;
    if code != AUX_EPOCH_EXPIRED_CODE {
        return Err(format!("a pruned epoch is refused expired, got {code}").into());
    }
    let generation = generation_key();
    let discarded: Vec<AuxEpochV1> = plane
        .index
        .discarded()?
        .into_iter()
        .filter(|(key, _epoch)| *key == generation)
        .map(|(_key, epoch)| epoch)
        .collect();
    if !discarded.contains(&AuxEpochV1::new(1)) {
        return Err(format!("the pruned epoch's index is discarded: {discarded:?}").into());
    }
    let retained = plane
        .ledger
        .read()
        .map_err(|err| format!("ledger poisoned: {err}"))?
        .history_retained_epochs(
            &generation.repo_id,
            &generation.revision_id,
            generation.generation,
        )
        .ok_or("the generation exists")?;
    let durable: BTreeSet<AuxEpochV1> = plane.index.epochs_of(&generation)?.into_iter().collect();
    let retained: BTreeSet<AuxEpochV1> = retained.into_iter().collect();
    if durable != retained {
        return Err(format!(
            "the durable epoch indexes are exactly the retained epochs: durable {durable:?} retained {retained:?}"
        )
        .into());
    }
    Ok(())
}

/// A discard that fails after the delta is durable fails nothing and is
/// retried by the next mutation (QI-BB-020).
///
/// The first discard of the generation — the one that prunes epoch 1 —
/// fails as an I/O error would. Every receipt still answers success, the
/// ledger reads the newest epoch, the failure is counted, and epoch 1's
/// index stays on disk though it is no longer retained; the next mutation
/// reconciles again and removes it, so the durable indexes are exactly the
/// retained epochs, and nothing more is counted.
#[test]
fn a_discard_that_fails_after_the_delta_is_durable_is_counted_and_retried() -> TestResult {
    use quanta_index_core::{MetricSourcePort, MetricValueV1};
    let plane = plane()?;
    let failures = |plane: &Plane| -> Result<u64, Box<dyn std::error::Error>> {
        match plane
            .history_text
            .scrape()?
            .into_iter()
            .find(|point| point.name == "history_text_gc_failures_total")
            .map(|point| point.value)
        {
            Some(MetricValueV1::Counter(count)) => Ok(count),
            other => Err(format!("the gc failure counter: {other:?}").into()),
        }
    };
    let generation = generation_key();
    let retained_and_durable = |plane: &Plane| -> Result<
        (BTreeSet<AuxEpochV1>, BTreeSet<AuxEpochV1>),
        Box<dyn std::error::Error>,
    > {
        let retained: BTreeSet<AuxEpochV1> = plane
            .ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?
            .history_retained_epochs(
                &generation.repo_id,
                &generation.revision_id,
                generation.generation,
            )
            .ok_or("the generation exists")?
            .into_iter()
            .collect();
        let durable: BTreeSet<AuxEpochV1> =
            plane.index.epochs_of(&generation)?.into_iter().collect();
        Ok((retained, durable))
    };
    let _receipt = plane
        .materializer
        .publish_batch(&batch("fixture", fixture()))?;
    plane.index.fail_next_discard();
    for step in 0..=AUX_EPOCH_RETAIN {
        let byte = u8::try_from(step.saturating_add(20))?;
        let _receipt = plane.materializer.publish_batch(&batch(
            &format!("more-{step}"),
            vec![commit(byte, 1_000, "dave", "unrelated")],
        ))?;
    }
    let (retained, durable) = retained_and_durable(&plane)?;
    if failures(&plane)? != 1
        || retained.contains(&AuxEpochV1::new(1))
        || !durable.contains(&AuxEpochV1::new(1))
    {
        return Err(format!(
            "the failed discard is counted and epoch 1 stays on disk unretained: failures={} retained={retained:?} durable={durable:?}",
            failures(&plane)?
        )
        .into());
    }

    let _receipt = plane.materializer.publish_batch(&batch(
        "after",
        vec![commit(99, 2_000, "erin", "unrelated")],
    ))?;
    let (retained, durable) = retained_and_durable(&plane)?;
    if failures(&plane)? != 1 || retained != durable {
        return Err(format!(
            "the next mutation removes the leftover: failures={} retained={retained:?} durable={durable:?}",
            failures(&plane)?
        )
        .into());
    }
    Ok(())
}

#[test]
fn relevance_without_a_wired_index_is_refused_typed_while_recency_serves() -> TestResult {
    let ledger = ready_ledger();
    {
        let mut guard = ledger
            .write()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        guard.apply_history_batch(&batch("fixture", fixture()), std::time::Instant::now())?;
    }
    let dispatcher = dispatcher_over(ledger)?;
    let code = error_code(
        &dispatcher,
        "type:commit needle",
        HistoryOrderV1::Relevance,
        None,
    )?;
    if code != ERR_HISTORY_RELEVANCE_UNAVAILABLE {
        return Err(format!("a plane without an index refuses relevance typed, got {code}").into());
    }
    let recency = page(
        &dispatcher,
        "type:commit needle",
        HistoryOrderV1::Recency,
        3,
        None,
    )?;
    if recency.commits.len() != 3 {
        return Err(format!("recency still serves: {recency:?}").into());
    }
    Ok(())
}

#[test]
fn an_unscorable_expression_is_refused_under_relevance_and_filters_under_recency() -> TestResult {
    let plane = plane()?;
    let _receipt = plane
        .materializer
        .publish_batch(&batch("fixture", fixture()))?;
    let code = error_code(
        &plane.dispatcher,
        "type:commit 'needle'",
        HistoryOrderV1::Relevance,
        None,
    )?;
    if code != HISTORY_TEXT_QUERY_UNSCORABLE_CODE {
        return Err(format!("a raw string has no score under relevance, got {code}").into());
    }
    let recency = page(
        &plane.dispatcher,
        "type:commit 'needle'",
        HistoryOrderV1::Recency,
        10,
        None,
    )?;
    if recency.commits.len() != 5 {
        return Err(format!("a raw string filters under recency: {recency:?}").into());
    }
    Ok(())
}

/// QI-BB-023 보완 #3 — one text predicate on both orders.
///
/// `fix` over `{"Fix typo", "prefix", "fix bug"}`: a keyword is a folded
/// whole-token match under both orders, so both count exactly the same
/// two commits (`Fix typo`, `fix bug`) — never `prefix`, which only a
/// substring would admit, and never a case-sensitive miss of `Fix`.
/// A raw string beside the keyword narrows both orders' row sets the
/// same way.
#[test]
fn recency_and_relevance_count_the_same_rows_for_the_same_text_query() -> TestResult {
    let plane = plane()?;
    let _receipt = plane.materializer.publish_batch(&batch(
        "predicate-parity",
        vec![
            commit(1, 100, "alice", "fix bug"),
            commit(2, 200, "alice", "prefix"),
            commit(3, 300, "alice", "Fix typo"),
            commit(4, 400, "alice", "fix the 'x.y' literal"),
        ],
    ))?;
    for (query_text, expected) in [
        ("type:commit fix", BTreeSet::from([sha(1), sha(3), sha(4)])),
        ("type:commit fix case:yes", BTreeSet::from([sha(1), sha(4)])),
        ("type:commit fix 'x.y'", BTreeSet::from([sha(4)])),
        ("type:commit \"fix bug\"", BTreeSet::from([sha(1)])),
    ] {
        let mut totals = Vec::new();
        for order in HistoryOrderV1::ALL {
            let served = page(&plane.dispatcher, query_text, order, 10, None)?;
            let rows: BTreeSet<CommitSha> = served.commits.iter().map(|row| row.sha).collect();
            if rows != expected {
                return Err(format!(
                    "{query_text} under {order}: rows {rows:?}, expected {expected:?}"
                )
                .into());
            }
            totals.push(served.window.candidate_count());
        }
        let want = CandidateCountV1::Exact(u64::try_from(expected.len())?);
        if totals.iter().any(|total| *total != want) {
            return Err(format!("{query_text}: totals differ across orders: {totals:?}").into());
        }
    }
    Ok(())
}

#[test]
fn filters_apply_to_relevance_rows_and_the_count_is_exact() -> TestResult {
    let plane = plane()?;
    let _receipt = plane
        .materializer
        .publish_batch(&batch("fixture", fixture()))?;
    let bob = page(
        &plane.dispatcher,
        "type:commit author:bob needle",
        HistoryOrderV1::Relevance,
        1,
        None,
    )?;
    let authors: BTreeSet<String> = bob.commits.iter().map(|row| row.author.clone()).collect();
    if authors != BTreeSet::from(["bob".to_string()]) {
        return Err(format!("only bob's commits pass the filter: {bob:?}").into());
    }
    // Two of bob's three commits mention needle; the index visited every
    // needle commit to know that.
    if bob.window.candidate_count() != CandidateCountV1::Exact(2) || bob.examined != 5 {
        return Err(format!("the count is exact after the filter: {bob:?}").into());
    }
    let expected: Vec<CommitSha> = reference_ranking(&fixture(), &["needle"])
        .into_iter()
        .map(|(sha, _score)| sha)
        .filter(|sha| *sha == self::sha(2) || *sha == self::sha(6))
        .collect();
    if bob.commits.first().map(|row| row.sha) != expected.first().copied() {
        return Err(
            format!("the filtered page keeps the oracle's order: {bob:?} vs {expected:?}").into(),
        );
    }
    Ok(())
}

#[test]
fn an_epoch_index_is_rebuilt_in_full_when_the_base_has_none() -> TestResult {
    let plane = plane()?;
    // A generation that exists without an index (it predates the index).
    {
        let mut guard = plane
            .ledger
            .write()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        guard.apply_history_batch(
            &batch(
                "pre-index",
                vec![commit(1, 100, "alice", "needle from before")],
            ),
            std::time::Instant::now(),
        )?;
    }
    let _receipt = plane.materializer.publish_batch(&batch(
        "first-indexed",
        vec![commit(2, 200, "bob", "needle now")],
    ))?;
    let _receipt = plane.materializer.publish_batch(&batch(
        "second-indexed",
        vec![commit(3, 300, "carol", "needle again")],
    ))?;
    let builds = plane.index.builds()?;
    if builds
        != vec![
            (AuxEpochV1::new(2), "full"),
            (AuxEpochV1::new(3), "incremental"),
        ]
    {
        return Err(format!(
            "the first indexed epoch is a full build over the whole state, the next incremental: {builds:?}"
        )
        .into());
    }
    let page = page(
        &plane.dispatcher,
        "type:commit needle",
        HistoryOrderV1::Relevance,
        10,
        None,
    )?;
    let shas: BTreeSet<CommitSha> = page.commits.iter().map(|row| row.sha).collect();
    if shas != BTreeSet::from([sha(1), sha(2), sha(3)]) || page.read_epoch != AuxEpochV1::new(3) {
        return Err(format!("the rebuilt index holds the pre-index rows too: {page:?}").into());
    }
    Ok(())
}

/// The index of an epoch is published before its rows: when that publish
/// fails, the rows never land and the ledger never advances, so no epoch
/// number is ever claimed without its index.
#[test]
fn a_failed_index_publish_leaves_the_ledger_and_catalog_untouched() -> TestResult {
    let ledger = ready_ledger();
    let index = MemoryHistoryTextIndex::shared();
    let catalog = Arc::new(MemoryAuxiliaryCatalog::default());
    let history_text = HistoryTextIndexParts::new(index.clone());
    let materializer = DirectHistoryMaterializer::new(AuxiliaryMaterializerParts {
        catalog: catalog.clone(),
        coordinator: AuxiliaryMutationCoordinator::shared(),
        ledger: Arc::clone(&ledger),
    })
    .with_history_text(history_text);
    let _receipt = materializer.publish_batch(&batch("fixture", fixture()))?;
    let applies_before = catalog.applies();
    let rows_before = catalog.row_count();
    index.fail_next_publish();
    match materializer.publish_batch(&batch("refused", vec![commit(9, 900, "carol", "needle")])) {
        Err(CoreError::Storage(message)) if message.contains("injected publish failure") => {}
        other => return Err(format!("the publish failure propagates, got {other:?}").into()),
    }
    if catalog.applies() != applies_before || catalog.row_count() != rows_before {
        return Err("no row reaches the catalog when the index publish fails".into());
    }
    let generation = generation_key();
    let read = ledger
        .read()
        .map_err(|err| format!("ledger poisoned: {err}"))?
        .history_read_at(
            &generation.repo_id,
            &generation.revision_id,
            generation.generation,
            None,
            std::time::Instant::now(),
        )?
        .ok_or("the generation exists")?;
    if read.epoch != AuxEpochV1::new(1) || read.state.commits().contains_key(&sha(9)) {
        return Err(format!(
            "the ledger stays at epoch 1 without the refused commit, read {}",
            read.epoch
        )
        .into());
    }
    if index.epochs_of(&generation)? != vec![AuxEpochV1::new(1)] {
        return Err("no epoch index beyond the published one exists".into());
    }
    // The next attempt lands and claims epoch 2 with its index.
    let _receipt =
        materializer.publish_batch(&batch("retry", vec![commit(9, 900, "carol", "needle")]))?;
    if index.epochs_of(&generation)? != vec![AuxEpochV1::new(1), AuxEpochV1::new(2)] {
        return Err("the retry publishes epoch 2".into());
    }
    Ok(())
}
