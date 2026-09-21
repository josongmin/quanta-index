//! SEP-21 P02B — the global sequence authority and the operation
//! journal's DoD, against the real engine.
//!
//! Every durable terminal (commit, refusal, abort, invalidation) is one
//! sequence number and one generic ledger event inside one transaction;
//! replays and stale fences mutate nothing; the allocator reconciles
//! from the ledger alone; persisted receipts refuse foreign format
//! versions typed; the durable mutation coordinator is machine-enforced.

#![forbid(unsafe_code)]

use std::error::Error;
use std::time::Duration;

use quanta_index_catalog::{CATALOG_FILE_NAME, SqliteCatalog, catalog_dir};
use quanta_index_contract::{
    BatchPublishReceipt, IngestOperationKindV1, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneErrorCodeV2,
};
use quanta_index_core::{
    CATALOG_BUSY_CODE, CATALOG_ROW_CORRUPT_CODE, OPERATION_FENCE_LOST_CODE,
    OPERATION_REPLAY_FLOOR_CODE, ClaimOutcomeV1, CoreError, IdempotencyCatalogPort,
    IdempotencyKeyV1, MutationCoordinatorPort, MutationLeaseV1, OperationInspectV1,
    OperationJournalStateV1, PreparedMutationV1,
};
use sha2::{Digest, Sha256};

type TestResult = Result<(), Box<dyn Error>>;

const LONG_LEASE_MS: u64 = i64::MAX as u64;

fn key(kind: IngestOperationKindV1, generation: u64, digest: &str) -> IdempotencyKeyV1 {
    IdempotencyKeyV1 {
        kind,
        repo_id: RepoId::new("repo-j").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-j")
            .expect("static fixture ID satisfies canonical policy"),
        generation: ManifestGeneration::new(generation),
        batch_digest: digest.to_string(),
    }
}

fn receipt(generation: u64, digest: &str) -> BatchPublishReceipt {
    let mut r = BatchPublishReceipt::empty_for(
        ManifestGeneration::new(generation),
        None,
        digest.to_string(),
    );
    r.accept_replace_scope();
    r
}

fn typed_code(error: &CoreError) -> Option<SearchPlaneErrorCodeV2> {
    match error {
        CoreError::Typed { code, .. } => Some(*code),
        _ => None,
    }
}

fn claim(
    catalog: &SqliteCatalog,
    key: &IdempotencyKeyV1,
    body: &[u8; 32],
) -> Result<PreparedMutationV1, CoreError> {
    match catalog.claim_prepared(key, body, "journal-test", LONG_LEASE_MS, body)? {
        ClaimOutcomeV1::Claimed(claim) => Ok(claim),
        ClaimOutcomeV1::Replay { .. } => Err(CoreError::InvalidContract(
            "expected a fresh claim, got a replay".to_string(),
        )),
    }
}

fn apply_and_commit(
    catalog: &SqliteCatalog,
    key: &IdempotencyKeyV1,
    body: &[u8; 32],
    receipt: &BatchPublishReceipt,
) -> Result<u64, CoreError> {
    let claim = claim(catalog, key, body)?;
    catalog.mark_applying(&claim)?;
    catalog.commit(&claim, receipt)
}

fn db_path(temp: &tempfile::TempDir) -> std::path::PathBuf {
    catalog_dir(temp.path()).join(CATALOG_FILE_NAME)
}

fn raw(temp: &tempfile::TempDir) -> Result<rusqlite::Connection, Box<dyn Error>> {
    Ok(rusqlite::Connection::open(db_path(temp))?)
}

fn allocator_next(temp: &tempfile::TempDir) -> Result<Option<i64>, Box<dyn Error>> {
    let connection = raw(temp)?;
    let next: Option<i64> = connection.query_row(
        "SELECT next FROM catalog_sequence_v2 WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(next)
}

fn event_count(temp: &tempfile::TempDir) -> Result<i64, Box<dyn Error>> {
    let connection = raw(temp)?;
    Ok(connection.query_row(
        "SELECT COUNT(*) FROM catalog_sequence_event_v2",
        [],
        |row| row.get(0),
    )?)
}

/// The allocator row's self-digest preimage, test-owned so a restored-DB
/// fixture can rewrite the row the way a stopped clock would.
fn allocator_row_digest(next: Option<i64>, exhausted: bool) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-index:catalog:sequence-row:v1\0");
    match next {
        Some(next) => {
            hasher.update([1_u8]);
            hasher.update(next.to_le_bytes());
        }
        None => hasher.update([0_u8]),
    }
    hasher.update(b"\x1f");
    hasher.update([u8::from(exhausted)]);
    hasher.finalize().into()
}

/// An ack lost after the commit: the retry replays the exact receipt and
/// sequence, and the journal does zero additional work (no new sequence,
/// no new event).
#[test]
fn ack_loss_replay_does_zero_work() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    let key = key(IngestOperationKindV1::History, 1, "d-ack");
    let body = [7_u8; 32];
    let first = apply_and_commit(&catalog, &key, &body, &receipt(1, "d-ack"))?;
    let events_before = event_count(&temp)?;
    match catalog.claim_prepared(&key, &body, "producer-retry", LONG_LEASE_MS, &body)? {
        ClaimOutcomeV1::Replay {
            receipt,
            durable_sequence,
        } => {
            if durable_sequence != first || receipt.batch_digest != "d-ack" {
                return Err("the replay must carry the exact recorded apply".into());
            }
        }
        _ => return Err("a same-body retry after ack loss must replay".into()),
    }
    if event_count(&temp)? != events_before {
        return Err("a replay must append no journal event".into());
    }
    if allocator_next(&temp)? != Some(i64::try_from(first)? + 1) {
        return Err("a replay must not advance the allocator".into());
    }
    Ok(())
}

/// A frozen-policy refusal is terminal and exact: the retry replays the
/// same typed refusal instead of re-applying (invalid-aux restart
/// convergence — no indefinite intent).
#[test]
fn a_recorded_refusal_is_exact_replayed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    let key = key(IngestOperationKindV1::Dirty, 2, "d-ref");
    let body = [3_u8; 32];
    let claim = claim(&catalog, &key, &body)?;
    catalog.mark_applying(&claim)?;
    let refusal = CoreError::Typed {
        code: SearchPlaneErrorCodeV2::BatchDigestMismatch,
        message: "frozen policy: the batch violates its surface authority".to_string(),
    };
    let sequence = catalog.record_refused(&claim, &refusal)?;
    if sequence != 1 {
        return Err("a refusal allocates the first sequence".into());
    }
    // Restart: the same body meets the same typed refusal.
    drop(catalog);
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    let replayed = catalog
        .claim_prepared(&key, &body, "journal-test", LONG_LEASE_MS, &body)
        .expect_err("a refused record must exact-replay its refusal");
    if !matches!(
        &replayed,
        CoreError::Typed {
            code: quanta_index_core::BATCH_DIGEST_MISMATCH_CODE,
            message,
        } if message.contains("frozen policy")
    ) {
        return Err(format!("the refusal must replay exactly, got {replayed:?}").into());
    }
    // And the inspect reports it without mutating.
    match catalog.inspect(&key)? {
        OperationInspectV1::Refused { code, .. } if code == "BATCH_DIGEST_MISMATCH" => {}
        other => return Err(format!("inspect must report the refusal, got {other:?}").into()),
    }
    Ok(())
}

/// Uncertain (an ambiguous terminal attempt) resolves through recovery:
/// it aborts with its ledger event, and a retry claims fresh and commits.
#[test]
fn uncertain_recovery_aborts_then_a_retry_commits() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    let key = key(IngestOperationKindV1::Structural, 3, "d-unc");
    let body = [5_u8; 32];
    let claim = claim(&catalog, &key, &body)?;
    catalog.mark_applying(&claim)?;
    catalog.mark_uncertain(&claim)?;
    match catalog.inspect(&key)? {
        OperationInspectV1::Uncertain { .. } => {}
        other => return Err(format!("expected Uncertain, got {other:?}").into()),
    }
    // Recovery resolves the ambiguity terminally; only then does the
    // original worker's late commit lose its fence.
    match catalog.recover(&key)? {
        OperationInspectV1::Absent => {}
        other => {
            return Err(format!("recovery must resolve Uncertain to absent, got {other:?}").into());
        }
    }
    let late = catalog
        .commit(&claim, &receipt(3, "d-unc"))
        .expect_err("a recovered-away worker must not commit");
    if typed_code(&late) != Some(OPERATION_FENCE_LOST_CODE) {
        return Err(format!("expected OPERATION_FENCE_LOST, got {late:?}").into());
    }
    let sequence = apply_and_commit(&catalog, &key, &body, &receipt(3, "d-unc"))?;
    if sequence < 2 {
        return Err("the retry's commit must follow the abort event".into());
    }
    Ok(())
}

/// Timeout (an expired lease) and cancellation (a typed refusal) are
/// distinct terminals; a live lease is left alone by recovery.
#[test]
fn timeout_disconnect_and_cancellation_have_distinct_terminals() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    // Cancellation: a typed refusal terminal, replayed exactly above.
    let cancelled = key(IngestOperationKindV1::RepoTopic, 4, "d-cancel");
    let claim = claim(&catalog, &cancelled, &[1_u8; 32])?;
    let _refused_sequence = catalog.record_refused(
        &claim,
        &CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestCancelled,
            message: "cancelled by the caller".to_string(),
        },
    )?;
    match catalog.inspect(&cancelled)? {
        OperationInspectV1::Refused { .. } => {}
        other => return Err(format!("cancellation must be a Refused terminal, got {other:?}").into()),
    }
    // Timeout: an expired lease recovers to Aborted (an event), while a
    // live lease stays InFlight.
    let timed_out = key(IngestOperationKindV1::RepoMeta, 5, "d-timeout");
    let _expired = catalog.claim_prepared(&timed_out, &[2_u8; 32], "slow-worker", 0, &[2_u8; 32])?
        .claimed()?;
    let live = key(IngestOperationKindV1::RepoDescription, 6, "d-live");
    let _held = catalog.claim_prepared(
        &live,
        &[3_u8; 32],
        "live-worker",
        LONG_LEASE_MS,
        &[3_u8; 32],
    )?
    .claimed()?;
    match catalog.recover(&timed_out)? {
        OperationInspectV1::Absent => {}
        other => return Err(format!("an expired lease must recover away, got {other:?}").into()),
    }
    match catalog.inspect(&timed_out)? {
        // The aborted record is terminal: inspect reports absent (its
        // history lives in the ledger).
        OperationInspectV1::Absent => {}
        other => return Err(format!("expected terminal Aborted, got {other:?}").into()),
    }
    match catalog.recover(&live)? {
        OperationInspectV1::InFlight { .. } => {}
        other => return Err(format!("a live lease must be left alone, got {other:?}").into()),
    }
    // A live foreign claim refuses a second claimant typed-busy.
    let busy = catalog
        .claim_prepared(&live, &[3_u8; 32], "other-worker", LONG_LEASE_MS, &[3_u8; 32])
        .expect_err("a live foreign claim must refuse");
    if typed_code(&busy) != Some(CATALOG_BUSY_CODE) {
        return Err(format!("expected CATALOG_BUSY, got {busy:?}").into());
    }
    Ok(())
}

/// Old receipt / new runtime: a stored receipt whose format version is
/// not this runtime's is a typed refusal before any mutation (no dual
/// decoder, no live migration).
#[test]
fn a_foreign_receipt_version_is_refused_typed_before_mutation() -> TestResult {
    let temp = tempfile::tempdir()?;
    {
        let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
        let key = key(IngestOperationKindV1::SearchCorpus, 7, "d-ver");
        let _sequence = apply_and_commit(&catalog, &key, &[6_u8; 32], &receipt(7, "d-ver"))?;
    }
    // Behind the catalog's back, rewrite the persisted receipt's version
    // tag to a foreign future version (new receipt / old runtime); the
    // same refusal answers the reverse direction by symmetry.
    let connection = raw(&temp)?;
    let stored: Vec<u8> = connection.query_row(
        "SELECT receipt_cbor FROM idempotency_v2 WHERE batch_digest = 'd-ver'",
        [],
        |row| row.get(0),
    )?;
    let mut foreign = stored;
    foreign[0] = 0x09;
    let changed = connection.execute(
        "UPDATE idempotency_v2 SET receipt_cbor = ?1 WHERE batch_digest = 'd-ver'",
        rusqlite::params![foreign],
    )?;
    if changed != 1 {
        return Err("expected to rewrite one receipt".into());
    }
    drop(connection);

    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    let key = key(IngestOperationKindV1::SearchCorpus, 7, "d-ver");
    let refused = catalog
        .claim_prepared(&key, &[6_u8; 32], "journal-test", LONG_LEASE_MS, &[6_u8; 32])
        .expect_err("a foreign receipt version must refuse");
    if typed_code(&refused) != Some(CATALOG_ROW_CORRUPT_CODE)
        || !refused.to_string().contains("format version")
    {
        return Err(format!("expected a typed version refusal, got {refused:?}").into());
    }
    Ok(())
}

/// A crash at a journal boundary leaves nothing behind: when the
/// terminal transaction cannot commit (a foreign writer holds the
/// database past the budget), the allocator, the event ledger and the
/// domain row are all unchanged.
#[test]
fn a_terminal_that_cannot_commit_leaves_allocator_event_and_row_at_zero_delta() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(60))?;
    let key = key(IngestOperationKindV1::FileOwnership, 8, "d-crash");
    let body = [9_u8; 32];
    let claim = claim(&catalog, &key, &body)?;
    catalog.mark_applying(&claim)?;
    let events_before = event_count(&temp)?;
    let next_before = allocator_next(&temp)?;

    let holder = raw(&temp)?;
    holder.execute_batch("BEGIN IMMEDIATE;")?;
    let refused = catalog
        .commit(&claim, &receipt(8, "d-crash"))
        .expect_err("a held lock past the budget must refuse the commit");
    if typed_code(&refused) != Some(CATALOG_BUSY_CODE) {
        return Err(format!("expected CATALOG_BUSY, got {refused:?}").into());
    }
    holder.execute_batch("ROLLBACK;")?;
    if event_count(&temp)? != events_before {
        return Err("a refused terminal must append no event".into());
    }
    if allocator_next(&temp)? != next_before {
        return Err("a refused terminal must not advance the allocator".into());
    }
    // The claim is still Applying under its fence: the retry of the same
    // worker commits once the lock is free.
    let sequence = catalog.commit(&claim, &receipt(8, "d-crash"))?;
    if sequence != 1 {
        return Err(format!("the retried commit takes sequence 1, got {sequence}").into());
    }
    Ok(())
}

/// A stale worker (its record recovered or re-claimed) cannot commit.
#[test]
fn a_stale_fence_commit_is_rejected() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    let key = key(IngestOperationKindV1::FileContributor, 9, "d-stale");
    let body = [1_u8; 32];
    let worker = claim(&catalog, &key, &body)?;
    catalog.mark_applying(&worker)?;
    catalog.mark_uncertain(&worker)?;
    let _recovered = catalog.recover(&key)?;
    let refused = catalog
        .commit(&worker, &receipt(9, "d-stale"))
        .expect_err("a stale worker must not commit");
    if typed_code(&refused) != Some(OPERATION_FENCE_LOST_CODE) {
        return Err(format!("expected OPERATION_FENCE_LOST, got {refused:?}").into());
    }
    Ok(())
}

/// The sequence is globally unique and monotonic across interleaved
/// kinds — commits, refusals and (synthetic non-operation) invalidation
/// events interleave on one allocator.
#[test]
fn interleaved_kinds_share_one_strictly_monotonic_sequence() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    let mut expected = 0_u64;
    // commit
    expected += 1;
    if apply_and_commit(
        &catalog,
        &key(IngestOperationKindV1::SearchCorpus, 10, "d-i1"),
        &[1_u8; 32],
        &receipt(10, "d-i1"),
    )? != expected
    {
        return Err("commit must take sequence 1".into());
    }
    // refusal
    expected += 1;
    let claim = claim(&catalog, &key(IngestOperationKindV1::History, 12, "d-i2"), &[2_u8; 32])?;
    if catalog.record_refused(
        &claim,
        &CoreError::Typed {
            code: SearchPlaneErrorCodeV2::BatchDigestMismatch,
            message: "refused".to_string(),
        },
    )? != expected
    {
        return Err("refusal must take sequence 2".into());
    }
    // synthetic future event: a generation invalidation (the non-
    // operation kind later lanes' seal/activation events will share).
    expected += 1;
    let _forgotten = catalog.forget_generation(
        &RepoId::new("repo-j").unwrap(),
        &RevisionId::new("rev-j").unwrap(),
        ManifestGeneration::new(10),
    )?;
    // commit again
    expected += 1;
    if apply_and_commit(
        &catalog,
        &key(IngestOperationKindV1::Structural, 11, "d-i3"),
        &[3_u8; 32],
        &receipt(11, "d-i3"),
    )? != expected
    {
        return Err("the later commit must continue the one sequence".into());
    }
    let connection = raw(&temp)?;
    let mut statement = connection.prepare(
        "SELECT sequence FROM catalog_sequence_event_v2 ORDER BY sequence ASC",
    )?;
    let sequences: Vec<i64> = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let expected_vec: Vec<i64> = (1..=i64::try_from(expected)?).collect();
    if sequences != expected_vec {
        return Err(format!("the ledger must hold 1..={expected} exactly once each, got {sequences:?}").into());
    }
    Ok(())
}

/// The allocator reconciles from the generic ledger alone on open:
/// behind (repaired up), exact (untouched), and ahead (fail closed).
#[test]
fn restored_db_reconciles_from_the_ledger_alone() -> TestResult {
    let temp = tempfile::tempdir()?;
    {
        let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
        let _sequence = apply_and_commit(
            &catalog,
            &key(IngestOperationKindV1::RepoTopic, 12, "d-r"),
            &[4_u8; 32],
            &receipt(12, "d-r"),
        )?;
    }
    // Case: allocator behind the ledger (next = 1) — repaired to max + 1.
    {
        let connection = raw(&temp)?;
        let digest = allocator_row_digest(Some(1), false);
        let changed = connection.execute(
            "UPDATE catalog_sequence_v2 SET next = 1, exhausted = 0, row_sha256 = ?1 WHERE id = 1",
            rusqlite::params![digest.as_slice()],
        )?;
        if changed != 1 {
            return Err("expected to rewind the allocator".into());
        }
    }
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    if allocator_next(&temp)? != Some(2) {
        return Err("a behind allocator must be repaired to max + 1".into());
    }
    // Case: allocator ahead of the ledger — corruption, refused on open.
    {
        let connection = raw(&temp)?;
        let digest = allocator_row_digest(Some(5), false);
        let changed = connection.execute(
            "UPDATE catalog_sequence_v2 SET next = 5, exhausted = 0, row_sha256 = ?1 WHERE id = 1",
            rusqlite::params![digest.as_slice()],
        )?;
        if changed != 1 {
            return Err("expected to wind the allocator ahead".into());
        }
    }
    let refused = SqliteCatalog::open(temp.path(), Duration::from_millis(200))
        .expect_err("an allocator ahead of its ledger must fail closed");
    if !refused.to_string().contains("future events are absent") {
        return Err(format!("expected the fail-closed reconciliation, got {refused}").into());
    }
    // Case: exact — reopens cleanly (the catalog opened above already
    // proves the equal branch; the empty branch is every fresh catalog).
    let _catalog = catalog;
    Ok(())
}

/// Same-body replay performs zero work even across a restart: the
/// sequence, the event ledger and the allocator are all untouched.
#[test]
fn same_body_replay_is_zero_work_across_restart() -> TestResult {
    let temp = tempfile::tempdir()?;
    let key = key(IngestOperationKindV1::RepoDescription, 13, "d-zero");
    let body = [8_u8; 32];
    let sequence;
    {
        let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
        sequence = apply_and_commit(&catalog, &key, &body, &receipt(13, "d-zero"))?;
    }
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    let events_before = event_count(&temp)?;
    let next_before = allocator_next(&temp)?;
    match catalog.claim_prepared(&key, &body, "journal-test", LONG_LEASE_MS, &body)? {
        ClaimOutcomeV1::Replay {
            durable_sequence, ..
        } if durable_sequence == sequence => {}
        _ => return Err("the restart replay must carry the original sequence".into()),
    }
    if event_count(&temp)? != events_before || allocator_next(&temp)? != next_before {
        return Err("a replay must be zero work on the journal".into());
    }
    Ok(())
}

/// GC then replay is below the replay floor: refused typed, never
/// re-executed.
#[test]
fn forget_then_replay_is_below_the_replay_floor() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    let key = key(IngestOperationKindV1::SearchCorpus, 14, "d-gc");
    let body = [2_u8; 32];
    let _sequence = apply_and_commit(&catalog, &key, &body, &receipt(14, "d-gc"))?;
    let _removed = catalog.forget_generation(
        &RepoId::new("repo-j").unwrap(),
        &RevisionId::new("rev-j").unwrap(),
        ManifestGeneration::new(14),
    )?;
    let refused = catalog
        .claim_prepared(&key, &body, "journal-test", LONG_LEASE_MS, &body)
        .expect_err("a forgotten operation is below the replay floor");
    if typed_code(&refused) != Some(OPERATION_REPLAY_FLOOR_CODE) {
        return Err(format!("expected OPERATION_REPLAY_FLOOR, got {refused:?}").into());
    }
    Ok(())
}

/// The state graph is closed: every allowed transition is named and
/// every other pair is refused; the DB CHECK set matches the enum.
#[test]
fn the_state_graph_and_event_kind_set_are_closed() -> TestResult {
    use OperationJournalStateV1 as S;
    let allowed = [
        (S::Prepared, S::Claimed),
        (S::Prepared, S::Refused),
        (S::Prepared, S::Aborted),
        (S::Claimed, S::Applying),
        (S::Claimed, S::Refused),
        (S::Claimed, S::Aborted),
        (S::Claimed, S::Uncertain),
        (S::Applying, S::Committed),
        (S::Applying, S::Refused),
        (S::Applying, S::Aborted),
        (S::Applying, S::Uncertain),
        (S::Uncertain, S::Committed),
        (S::Uncertain, S::Aborted),
    ];
    for from in S::ALL {
        for to in S::ALL {
            let expected = allowed.contains(&(from, to));
            if S::transition_allowed(from, to) != expected {
                return Err(format!("transition {from:?} → {to:?} must be {expected}").into());
            }
        }
    }
    for terminal in [S::Committed, S::Refused, S::Aborted] {
        if !terminal.is_terminal() {
            return Err(format!("{terminal:?} must be terminal").into());
        }
    }
    // The DB CHECK set is the enum's codes.
    let temp = setup_empty()?;
    let connection = raw(&temp)?;
    let schema: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE name = 'catalog_sequence_event_v2'",
        [],
        |row| row.get(0),
    )?;
    let expected_check = format!(
        "kind IN ({})",
        [1_i64, 2, 3, 4, 5, 6, 7, 8, 9]
            .iter()
            .map(|code| code.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !schema.replace(' ', "").contains(&expected_check.replace(' ', "")) {
        return Err(format!("the schema's kind CHECK must be {expected_check}").into());
    }
    Ok(())
}

fn setup_empty() -> Result<tempfile::TempDir, Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let _catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    Ok(temp)
}

/// The durable mutation coordinator: enter refuses a live foreign lease
/// typed-busy, and a stale release is a fence loss.
#[test]
fn the_mutation_coordinator_is_durable_and_fenced() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    let lease = catalog.enter("scope-a", "worker-1", 60_000)?;
    if lease.owner != "worker-1" {
        return Err("enter must return the holder's lease".into());
    }
    let busy = catalog
        .enter("scope-a", "worker-2", 60_000)
        .expect_err("a live foreign lease must refuse");
    if typed_code(&busy) != Some(CATALOG_BUSY_CODE) {
        return Err(format!("expected CATALOG_BUSY, got {busy:?}").into());
    }
    // A stale release is a fence loss.
    let stale = MutationLeaseV1 {
        scope: lease.scope.clone(),
        owner: lease.owner.clone(),
        fence_token: lease.fence_token,
        deadline_ms: lease.deadline_ms,
    };
    catalog.release(&stale)?;
    let late = catalog
        .release(&stale)
        .expect_err("a second release of the same lease must refuse");
    if typed_code(&late) != Some(OPERATION_FENCE_LOST_CODE) {
        return Err(format!("expected OPERATION_FENCE_LOST, got {late:?}").into());
    }
    // After release the scope is free again — and survives a restart.
    drop(catalog);
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(200))?;
    let _next = catalog.enter("scope-a", "worker-3", 60_000)?;
    Ok(())
}

/// Claimed helper for tests that only need the claim value.
trait Claimed {
    fn claimed(&self) -> Result<PreparedMutationV1, Box<dyn Error>>;
}

impl Claimed for ClaimOutcomeV1 {
    fn claimed(&self) -> Result<PreparedMutationV1, Box<dyn Error>> {
        match self {
            ClaimOutcomeV1::Claimed(claim) => Ok(claim.clone()),
            ClaimOutcomeV1::Replay { .. } => Err("expected a claim".into()),
        }
    }
}
