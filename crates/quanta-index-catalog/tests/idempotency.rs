//! QI-BB-032 / SEP-21 P02B — the operation journal's contract, against
//! the real engine.
//!
//! 1. inspect → prepare → claim → apply → fenced commit: a fresh key is
//!    prepared, claimed, committed with a receipt, and every later
//!    attempt of the same body replays that receipt and sequence; a
//!    different body is a typed conflict before anything mutable.
//! 2. A claim left in progress by a crash (recovered) is claimed fresh
//!    and then commits.
//! 3. Rows verify their own digest: a bit flipped in the stored body hash
//!    is a typed `CATALOG_ROW_CORRUPT`, never a silent replay.
//! 4. Sequences are unique and monotonic across keys and survive reopen.
//! 5. A second writer holding the database past the busy budget is a
//!    typed `CATALOG_BUSY`.
//! 6. Forgetting a generation drops exactly its records, records one
//!    invalidation per dropped key, and a retry of a forgotten key is
//!    refused below the replay floor.
//! 7. Lease decisions read a scripted clock: claim, recover, and
//!    mutation-enter pin now < deadline, == deadline, and > deadline
//!    exactly, and each transaction samples the clock exactly once
//!    (TOPT-01 / PO-3).

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quanta_index_catalog::{CATALOG_FILE_NAME, CatalogClockPort, SqliteCatalog, catalog_dir};
use quanta_index_contract::{
    BatchPublishReceipt, IngestOperationKindV1, ManifestGeneration, RepoId, RepoMapMutationAck,
    RepoMapMutationPhaseV2, RepoMapTerminalReceiptV2, RevisionId, SearchPlaneErrorCodeV2,
};
use quanta_index_core::{
    BATCH_DIGEST_CONFLICT_CODE, CATALOG_BUSY_CODE, CATALOG_ROW_CORRUPT_CODE, ClaimOutcomeV1,
    CoreError, IdempotencyCatalogPort, IdempotencyKeyV1, MutationCoordinatorPort,
    OPERATION_FENCE_LOST_CODE, OPERATION_REPLAY_FLOOR_CODE, OperationInspectV1,
    OperationJournalStateV1, PreparedMutationV1,
};

const LONG_LEASE_MS: u64 = i64::MAX.unsigned_abs();

type TestResult = Result<(), Box<dyn Error>>;

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn key(kind: IngestOperationKindV1, generation: u64, digest: &str) -> IdempotencyKeyV1 {
    IdempotencyKeyV1 {
        kind,
        repo_id: RepoId::new("repo-cat").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-cat")
            .expect("static fixture ID satisfies canonical policy"),
        generation: ManifestGeneration::new(generation),
        batch_digest: digest.to_string(),
    }
}

fn receipt(generation: u64, digest: &str, replace: u32) -> BatchPublishReceipt {
    let mut receipt = BatchPublishReceipt::empty_for(
        ManifestGeneration::new(generation),
        Some(format!("manifest:{generation}")),
        digest,
    );
    for _ in 0..replace {
        receipt.accept_replace_scope();
    }
    receipt
}

fn typed_code(error: &CoreError) -> Option<quanta_index_contract::SearchPlaneErrorCodeV2> {
    match error {
        CoreError::Typed { code, .. } => Some(*code),
        CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_) => None,
    }
}

/// Claim, mark applying, and commit in one helper: the dispatcher's
/// happy path.
fn claim_and_commit(
    catalog: &SqliteCatalog,
    key: &IdempotencyKeyV1,
    body: &[u8; 32],
    receipt: &BatchPublishReceipt,
) -> Result<u64, CoreError> {
    let claim = match catalog.claim_prepared(key, body, "test", LONG_LEASE_MS, body)? {
        ClaimOutcomeV1::Claimed(claim) => claim,
        ClaimOutcomeV1::Replay { .. } | ClaimOutcomeV1::ReplayRepoMap { .. } => {
            return Err(CoreError::InvalidContract(
                "expected a fresh claim, got a replay".to_string(),
            ));
        }
    };
    catalog.mark_applying(&claim)?;
    catalog.commit(&claim, receipt)
}

#[test]
fn a_committed_record_replays_and_a_different_body_conflicts() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
    let key = key(IngestOperationKindV1::SearchCorpus, 3, "digest-a");
    let body = [1_u8; 32];

    let sequence = claim_and_commit(&catalog, &key, &body, &receipt(3, "digest-a", 2))?;
    if sequence != 1 {
        return Err(format!("first apply must be sequence 1, got {sequence}").into());
    }
    // inspect answers the replay read-only.
    match catalog.inspect(&key)? {
        OperationInspectV1::Committed {
            receipt: stored,
            durable_sequence,
        } => {
            if durable_sequence != 1 || stored.accepted_replace_scopes != 2 {
                return Err(format!(
                    "replay must carry the recorded apply: {stored:?} @ {durable_sequence}"
                )
                .into());
            }
        }
        other @ (OperationInspectV1::Absent
        | OperationInspectV1::CommittedRepoMap { .. }
        | OperationInspectV1::InFlight { .. }
        | OperationInspectV1::Refused { .. }
        | OperationInspectV1::Uncertain { .. }) => {
            return Err(format!("expected a committed inspect, got {other:?}").into());
        }
    }
    // And so does a claim: same outcome, no mutation.
    match catalog.claim_prepared(&key, &body, "test", LONG_LEASE_MS, &body)? {
        ClaimOutcomeV1::Replay {
            durable_sequence: 1,
            ..
        } => {}
        other @ (ClaimOutcomeV1::Claimed(_)
        | ClaimOutcomeV1::Replay { .. }
        | ClaimOutcomeV1::ReplayRepoMap { .. }) => {
            return Err(format!("expected a replay claim, got {other:?}").into());
        }
    }
    let different = [2_u8; 32];
    let conflict = catalog
        .claim_prepared(&key, &different, "test", LONG_LEASE_MS, &different)
        .expect_err("a different body under the same key must be refused");
    if typed_code(&conflict) != Some(BATCH_DIGEST_CONFLICT_CODE) {
        return Err(format!("expected a typed conflict, got {conflict:?}").into());
    }
    // The conflict wrote nothing: the original record still replays.
    if !matches!(catalog.inspect(&key)?, OperationInspectV1::Committed { .. }) {
        return Err("a refused conflict must leave the record untouched".into());
    }
    // Committing again is a stale-fence caller defect.
    let stale = PreparedMutationV1 {
        key,
        body_sha256: body,
        owner: "test".to_string(),
        fence_token: u64::MAX,
        lease_deadline_ms: u64::MAX,
        epoch_commitment: body,
    };
    if catalog.commit(&stale, &receipt(3, "digest-a", 2)).is_ok() {
        return Err("committing an already-committed record must fail".into());
    }
    Ok(())
}

#[test]
fn a_crashed_claim_is_recovered_then_claimed_fresh_and_commits() -> TestResult {
    let temp = tempfile::tempdir()?;
    let key = key(IngestOperationKindV1::History, 5, "digest-h");
    let body = [9_u8; 32];
    {
        let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
        let _claim = catalog
            .claim_prepared(&key, &body, "crashed", 0, &body)?
            .claimed_or_fail()?;
        // The process dies here: the record is claimed on disk with an
        // expired lease.
    }
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
    if !matches!(catalog.recover(&key)?, OperationInspectV1::Absent) {
        return Err("an expired claim must recover to an absent record".into());
    }
    let sequence = claim_and_commit(&catalog, &key, &body, &receipt(5, "digest-h", 1))?;
    // The ledger holds, in order: the abort of the crashed claim (1), the
    // invalidation that attributes it (2), and the recovered commit (3).
    if sequence != 3 {
        return Err(format!(
            "the recovered apply commits after its abort and invalidation events, got {sequence}"
        )
        .into());
    }
    match catalog.inspect(&key)? {
        OperationInspectV1::Committed {
            durable_sequence: 3,
            ..
        } => {}
        other @ (OperationInspectV1::Absent
        | OperationInspectV1::Committed { .. }
        | OperationInspectV1::CommittedRepoMap { .. }
        | OperationInspectV1::InFlight { .. }
        | OperationInspectV1::Refused { .. }
        | OperationInspectV1::Uncertain { .. }) => {
            return Err(
                format!("a committed recovery replays like any apply, got {other:?}").into(),
            );
        }
    }
    Ok(())
}

#[test]
fn a_row_that_does_not_match_its_digest_is_refused_typed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
    let key = key(IngestOperationKindV1::Dirty, 7, "digest-d");
    let body = [4_u8; 32];
    let _sequence = claim_and_commit(&catalog, &key, &body, &receipt(7, "digest-d", 0))?;
    drop(catalog);

    // Flip one byte of the stored body hash behind the catalog's back, as
    // bit-rot would; the engine's own integrity check does not notice.
    let path = catalog_dir(temp.path()).join(CATALOG_FILE_NAME);
    let connection = rusqlite::Connection::open(&path)?;
    let changed = connection.execute(
        "UPDATE idempotency_v2
          SET body_sha256 = CAST(X'0404040404040404040404040404040404040404040404040404040404040405' AS BLOB)
          WHERE batch_digest = 'digest-d'",
        [],
    )?;
    if changed != 1 {
        return Err(format!("expected to corrupt one row, changed {changed}").into());
    }
    drop(connection);

    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
    let refused = catalog
        .claim_prepared(&key, &body, "test", LONG_LEASE_MS, &body)
        .expect_err("a corrupt row must not be served as replay or conflict");
    if typed_code(&refused) != Some(CATALOG_ROW_CORRUPT_CODE) {
        return Err(format!("expected CATALOG_ROW_CORRUPT, got {refused:?}").into());
    }
    Ok(())
}

#[test]
fn sequences_are_unique_and_monotonic_across_keys_and_reopens() -> TestResult {
    let temp = tempfile::tempdir()?;
    let mut sequences = Vec::new();
    {
        let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
        for (index, kind) in [
            IngestOperationKindV1::SearchCorpus,
            IngestOperationKindV1::RepoTopic,
            IngestOperationKindV1::Structural,
        ]
        .into_iter()
        .enumerate()
        {
            let generation = u64::try_from(index)?.saturating_add(1);
            let key = key(kind, generation, "digest");
            let body = [u8::try_from(index)?; 32];
            sequences.push(claim_and_commit(
                &catalog,
                &key,
                &body,
                &receipt(generation, "digest", 0),
            )?);
        }
    }
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
    let key = key(IngestOperationKindV1::RepoMeta, 9, "digest-late");
    let body = [8_u8; 32];
    sequences.push(claim_and_commit(
        &catalog,
        &key,
        &body,
        &receipt(9, "digest-late", 0),
    )?);
    if sequences != vec![1, 2, 3, 4] {
        return Err(format!("sequences must be 1..=4 in order, got {sequences:?}").into());
    }
    Ok(())
}

#[test]
fn a_held_write_lock_past_the_busy_budget_is_typed_busy() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(60))?;
    let path = catalog_dir(temp.path()).join(CATALOG_FILE_NAME);
    // A foreign writer holds the database.
    let holder = rusqlite::Connection::open(&path)?;
    holder.execute_batch("BEGIN IMMEDIATE;")?;

    let key = key(IngestOperationKindV1::Dirty, 1, "digest-busy");
    let started = std::time::Instant::now();
    let refused = catalog
        .claim_prepared(&key, &[0_u8; 32], "test", LONG_LEASE_MS, &[0_u8; 32])
        .expect_err("a held lock past the budget must be refused");
    let waited = started.elapsed();
    if typed_code(&refused) != Some(CATALOG_BUSY_CODE) {
        return Err(format!("expected CATALOG_BUSY, got {refused:?}").into());
    }
    if waited < Duration::from_millis(60) || waited > Duration::from_secs(5) {
        return Err(format!("the busy budget was not honored: waited {waited:?}").into());
    }
    holder.execute_batch("ROLLBACK;")?;
    match catalog.claim_prepared(&key, &[0_u8; 32], "test", LONG_LEASE_MS, &[0_u8; 32])? {
        ClaimOutcomeV1::Claimed(_) => {}
        other @ (ClaimOutcomeV1::Replay { .. } | ClaimOutcomeV1::ReplayRepoMap { .. }) => {
            return Err(format!("once released, the write proceeds, got {other:?}").into());
        }
    }
    Ok(())
}

#[test]
fn forgetting_a_generation_drops_exactly_its_records_and_raises_the_floor() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
    for (generation, digest) in [(1, "a"), (1, "b"), (2, "c")] {
        let key = key(IngestOperationKindV1::SearchCorpus, generation, digest);
        let body = [u8::try_from(generation)?; 32];
        let _sequence = claim_and_commit(&catalog, &key, &body, &receipt(generation, digest, 0))?;
    }
    let removed = catalog.forget_generation(
        &RepoId::new("repo-cat").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev-cat").expect("static fixture ID satisfies canonical policy"),
        ManifestGeneration::new(1),
    )?;
    if removed != 2 {
        return Err(format!("expected 2 records forgotten, got {removed}").into());
    }
    // A retry of a forgotten key is below the replay floor: refused, not
    // silently re-executed.
    let refused = catalog
        .claim_prepared(
            &key(IngestOperationKindV1::SearchCorpus, 1, "a"),
            &[1_u8; 32],
            "test",
            u64::MAX,
            &[1_u8; 32],
        )
        .expect_err("a forgotten key must refuse below the replay floor");
    if typed_code(&refused) != Some(OPERATION_REPLAY_FLOOR_CODE) {
        return Err(format!("expected OPERATION_REPLAY_FLOOR, got {refused:?}").into());
    }
    // Another generation's record survives untouched.
    if !matches!(
        catalog.inspect(&key(IngestOperationKindV1::SearchCorpus, 2, "c"))?,
        OperationInspectV1::Committed { .. }
    ) {
        return Err("another generation's record must survive".into());
    }
    Ok(())
}

/// Records of every route die with their generation, the listing names
/// each generation once in ascending order, and a second forget is a
/// no-op rather than an error (QI-BB-032 retention).
#[test]
fn the_pair_listing_and_forget_cover_every_route_and_forget_is_idempotent() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
    let repo = RepoId::new("repo-cat").expect("static fixture ID satisfies canonical policy");
    let revision =
        RevisionId::new("rev-cat").expect("static fixture ID satisfies canonical policy");
    for (kind, generation, digest) in [
        (IngestOperationKindV1::SearchCorpus, 4, "s"),
        (IngestOperationKindV1::History, 4, "h"),
        (IngestOperationKindV1::Dirty, 2, "d"),
        (IngestOperationKindV1::Structural, 9, "t"),
    ] {
        let key = key(kind, generation, digest);
        let body = [u8::try_from(generation)?; 32];
        let claim = catalog
            .claim_prepared(&key, &body, "test", LONG_LEASE_MS, &body)?
            .claimed_or_fail()?;
        if kind != IngestOperationKindV1::Dirty {
            catalog.mark_applying(&claim)?;
            let receipt = receipt(generation, digest, 0);
            let _sequence = catalog.commit(&claim, &receipt)?;
        }
    }
    let listed: Vec<u64> = catalog
        .generations_for_pair(&repo, &revision)?
        .into_iter()
        .map(ManifestGeneration::get)
        .collect();
    if listed != vec![2, 4, 9] {
        return Err(format!("expected generations [2, 4, 9], got {listed:?}").into());
    }
    if !catalog
        .generations_for_pair(
            &RepoId::new("other").expect("static fixture ID satisfies canonical policy"),
            &revision,
        )?
        .is_empty()
    {
        return Err("another pair's listing must be empty".into());
    }
    let removed = catalog.forget_generation(&repo, &revision, ManifestGeneration::new(4))?;
    if removed != 2 {
        return Err(
            format!("both routes' records of generation 4 must go, removed {removed}").into(),
        );
    }
    let again = catalog.forget_generation(&repo, &revision, ManifestGeneration::new(4))?;
    if again != 0 {
        return Err(format!("a second forget must be a no-op, removed {again}").into());
    }
    let listed: Vec<u64> = catalog
        .generations_for_pair(&repo, &revision)?
        .into_iter()
        .map(ManifestGeneration::get)
        .collect();
    if listed != vec![2, 9] {
        return Err(format!("expected generations [2, 9] after forget, got {listed:?}").into());
    }
    Ok(())
}

fn db_path(temp: &tempfile::TempDir) -> std::path::PathBuf {
    catalog_dir(temp.path()).join(CATALOG_FILE_NAME)
}

fn event_count(temp: &tempfile::TempDir) -> Result<i64, Box<dyn Error>> {
    let connection = rusqlite::Connection::open(db_path(temp))?;
    Ok(connection.query_row(
        "SELECT COUNT(*) FROM catalog_sequence_event_v2",
        [],
        |row| row.get(0),
    )?)
}

fn allocator_next(temp: &tempfile::TempDir) -> Result<Option<i64>, Box<dyn Error>> {
    let connection = rusqlite::Connection::open(db_path(temp))?;
    Ok(connection.query_row(
        "SELECT next FROM catalog_sequence_v2 WHERE id = 1",
        [],
        |row| row.get(0),
    )?)
}

fn repomap_receipt(generation: u64, digest: &str) -> RepoMapTerminalReceiptV2 {
    RepoMapTerminalReceiptV2 {
        phase: RepoMapMutationPhaseV2::Publish,
        mutation: RepoMapMutationAck {
            repo_id: RepoId::new("repo-cat").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-cat")
                .expect("static fixture ID satisfies canonical policy"),
            manifest_generation: ManifestGeneration::new(generation),
            prior_candidate_commitment: None,
            new_candidate_commitment: "c".repeat(64),
            activation_epoch: 0,
            terminal_sequence: 41,
            replayed: false,
        },
        manifest_digest: "m".repeat(64),
        snapshot_id: format!("snap-{generation}"),
        projection_version: 3,
        authority_digest: "a".repeat(64),
        source_bundle_digest: digest.to_string(),
    }
}

/// A same-key/different-body prepare is a typed conflict with zero
/// mutations: no event, no allocator advance, and the prepared row is
/// untouched.
#[test]
fn prepare_conflicts_before_any_mutation() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
    let key = key(IngestOperationKindV1::History, 21, "prepare-conflict");
    let body = [11_u8; 32];
    let _prepared = catalog.prepare(&key, &body, "test", LONG_LEASE_MS, &body)?;
    match catalog.inspect(&key)? {
        OperationInspectV1::InFlight { .. } => {}
        other => return Err(format!("a prepare must read back in-flight, got {other:?}").into()),
    }
    let events_before = event_count(&temp)?;
    let next_before = allocator_next(&temp)?;
    let conflict = catalog
        .prepare(&key, &[12_u8; 32], "test", LONG_LEASE_MS, &[12_u8; 32])
        .expect_err("a different body under the same key must conflict");
    if typed_code(&conflict) != Some(BATCH_DIGEST_CONFLICT_CODE) {
        return Err(format!("expected BATCH_DIGEST_CONFLICT, got {conflict:?}").into());
    }
    if event_count(&temp)? != events_before || allocator_next(&temp)? != next_before {
        return Err("a conflict must append no event and advance no sequence".into());
    }
    // The original prepare is untouched: the same owner still claims.
    match catalog.claim_prepared(&key, &body, "test", LONG_LEASE_MS, &body)? {
        ClaimOutcomeV1::Claimed(_) => {}
        other => {
            return Err(format!("the original prepare must still claim, got {other:?}").into());
        }
    }
    Ok(())
}

/// A frozen-policy refusal records atomically from the prepared
/// mutation: no claim is ever held, and the retry replays the same
/// typed refusal and terminal sequence.
#[test]
fn a_prepared_mutation_records_a_frozen_refusal_without_a_claim() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
    let key = key(IngestOperationKindV1::Dirty, 22, "prepare-refuse");
    let body = [13_u8; 32];
    let prepared = catalog.prepare(&key, &body, "test", LONG_LEASE_MS, &body)?;
    let refusal = CoreError::Typed {
        code: SearchPlaneErrorCodeV2::SearchCorpusBatchShapeInvalid,
        message: "frozen: shape".to_string(),
    };
    let sequence = catalog.record_refused(&prepared, &refusal)?;
    if sequence != 1 {
        return Err(format!("the frozen refusal takes sequence 1, got {sequence}").into());
    }
    match catalog.inspect(&key)? {
        OperationInspectV1::Refused {
            code,
            message,
            durable_sequence,
        } => {
            if code != SearchPlaneErrorCodeV2::SearchCorpusBatchShapeInvalid
                || message != "frozen: shape"
                || durable_sequence != sequence
            {
                return Err("the inspect must replay the frozen refusal and its sequence".into());
            }
        }
        other => return Err(format!("expected a refused inspect, got {other:?}").into()),
    }
    // The prepared mutation is spent: refusing twice loses the fence.
    let again = catalog
        .record_refused(&prepared, &refusal)
        .expect_err("a spent prepared mutation must not refuse twice");
    if typed_code(&again) != Some(OPERATION_FENCE_LOST_CODE) {
        return Err(format!("expected OPERATION_FENCE_LOST, got {again:?}").into());
    }
    // A retry meets the same typed refusal, not a claim.
    let replayed = catalog
        .claim_prepared(&key, &body, "test", LONG_LEASE_MS, &body)
        .expect_err("a refused record must exact-replay its refusal");
    if typed_code(&replayed) != Some(SearchPlaneErrorCodeV2::SearchCorpusBatchShapeInvalid) {
        return Err(format!("the refusal must replay exactly, got {replayed:?}").into());
    }
    Ok(())
}

/// A bare claim can neither refuse nor go uncertain: those terminals
/// are unrepresentable before the worker starts applying.
#[test]
fn a_bare_claim_cannot_refuse_or_go_uncertain() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
    let key = key(IngestOperationKindV1::Structural, 23, "bare-claim");
    let body = [14_u8; 32];
    let prepared = catalog.prepare(&key, &body, "test", LONG_LEASE_MS, &body)?;
    let claim = match catalog.claim_prepared(&key, &body, "test", LONG_LEASE_MS, &body)? {
        ClaimOutcomeV1::Claimed(claim) => claim,
        other => return Err(format!("expected a fresh claim, got {other:?}").into()),
    };
    // The prepared mutation and the claim differ by fence rotation, but
    // neither may refuse from `Claimed`.
    let _prepared = prepared;
    let refusal = CoreError::Typed {
        code: SearchPlaneErrorCodeV2::RequestCancelled,
        message: "cancelled".to_string(),
    };
    let refused = catalog
        .record_refused(&claim, &refusal)
        .expect_err("a bare claim must not refuse");
    if typed_code(&refused) != Some(OPERATION_FENCE_LOST_CODE) {
        return Err(format!("expected OPERATION_FENCE_LOST, got {refused:?}").into());
    }
    let uncertain = catalog
        .mark_uncertain(&claim)
        .expect_err("a bare claim must not go uncertain");
    if typed_code(&uncertain) != Some(OPERATION_FENCE_LOST_CODE) {
        return Err(format!("expected OPERATION_FENCE_LOST, got {uncertain:?}").into());
    }
    // The claim is still live: applying proceeds normally.
    catalog.mark_applying(&claim)?;
    let sequence = catalog.record_refused(&claim, &refusal)?;
    if sequence != 1 {
        return Err(format!("the applying refusal takes sequence 1, got {sequence}").into());
    }
    Ok(())
}

/// Prepare is idempotent for the same owner and busy for a live
/// foreign prepare.
#[test]
fn prepare_is_idempotent_for_the_same_owner_and_busy_for_foreign() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
    let key = key(IngestOperationKindV1::RepoTopic, 24, "prepare-idem");
    let body = [15_u8; 32];
    let first = catalog.prepare(&key, &body, "worker-a", LONG_LEASE_MS, &body)?;
    let second = catalog.prepare(&key, &body, "worker-a", LONG_LEASE_MS, &body)?;
    if first.fence_token != second.fence_token {
        return Err("the same owner's prepare must reuse its row".into());
    }
    let busy = catalog
        .prepare(&key, &body, "worker-b", LONG_LEASE_MS, &body)
        .expect_err("a live foreign prepare must refuse");
    if typed_code(&busy) != Some(CATALOG_BUSY_CODE) {
        return Err(format!("expected CATALOG_BUSY, got {busy:?}").into());
    }
    Ok(())
}

/// A repo-map bundle key travels the same journal: prepare, claim,
/// apply, then a repo-map terminal commit — and the replay carries the
/// same receipt and sequence with zero new work.
#[test]
fn a_repomap_bundle_key_travels_the_same_journal() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteCatalog::open(temp.path(), Duration::from_millis(100))?;
    let rm_key = key(IngestOperationKindV1::RepoMapBundle, 25, "rm-bundle");
    let body = [16_u8; 32];
    let rm_receipt = repomap_receipt(25, "rm-bundle");
    let prepared = catalog.prepare(&rm_key, &body, "test", LONG_LEASE_MS, &body)?;
    if prepared.operation_kind() != IngestOperationKindV1::RepoMapBundle {
        return Err("the prepared mutation must name its operation kind".into());
    }
    let (target_repo, _revision, target_generation) = prepared.target_identity();
    if target_repo.as_str() != "repo-cat" || target_generation.get() != 25 {
        return Err("the prepared mutation must name its target identity".into());
    }
    let claim = match catalog.claim_prepared(&rm_key, &body, "test", LONG_LEASE_MS, &body)? {
        ClaimOutcomeV1::Claimed(claim) => claim,
        other => return Err(format!("expected a fresh claim, got {other:?}").into()),
    };
    catalog.mark_applying(&claim)?;
    // A batch commit on a repo-map key is a caller defect.
    let batch_refused = catalog
        .commit(&claim, &receipt(25, "rm-bundle", 0))
        .expect_err("a batch commit must not take a repo-map key");
    if typed_code(&batch_refused).is_some() {
        return Err(format!(
            "a batch commit on a repo-map key must be a contract refusal, got {batch_refused:?}"
        )
        .into());
    }
    let sequence = catalog.commit_repomap(&claim, &rm_receipt)?;
    if sequence != 1 {
        return Err(format!("the repo-map commit takes sequence 1, got {sequence}").into());
    }
    let events_before = event_count(&temp)?;
    match catalog.inspect(&rm_key)? {
        OperationInspectV1::CommittedRepoMap {
            receipt: stored,
            durable_sequence,
        } => {
            if stored != rm_receipt || durable_sequence != sequence {
                return Err("the replay must carry the recorded terminal receipt".into());
            }
        }
        other => {
            return Err(format!("expected a repo-map committed inspect, got {other:?}").into());
        }
    }
    match catalog.claim_prepared(&rm_key, &body, "test", LONG_LEASE_MS, &body)? {
        ClaimOutcomeV1::ReplayRepoMap {
            receipt: stored,
            durable_sequence,
        } => {
            if stored != rm_receipt || durable_sequence != sequence {
                return Err("the claim replay must carry the recorded terminal receipt".into());
            }
        }
        other => return Err(format!("expected a repo-map replay, got {other:?}").into()),
    }
    if event_count(&temp)? != events_before {
        return Err("a repo-map replay must append no journal event".into());
    }
    // A repo-map commit on a batch key is the symmetric caller defect.
    let batch_key = key(IngestOperationKindV1::History, 26, "not-rm");
    let batch_body = [17_u8; 32];
    let _batch_prepared =
        catalog.prepare(&batch_key, &batch_body, "test", LONG_LEASE_MS, &batch_body)?;
    let batch_claim = match catalog.claim_prepared(
        &batch_key,
        &batch_body,
        "test",
        LONG_LEASE_MS,
        &batch_body,
    )? {
        ClaimOutcomeV1::Claimed(claim) => claim,
        other => return Err(format!("expected a fresh claim, got {other:?}").into()),
    };
    catalog.mark_applying(&batch_claim)?;
    if catalog.commit_repomap(&batch_claim, &rm_receipt).is_ok() {
        return Err("a repo-map commit must not take a batch key".into());
    }
    Ok(())
}

/// Test helper: a claim outcome that must be a claim.
trait ClaimedOrFail {
    fn claimed_or_fail(&self) -> Result<PreparedMutationV1, Box<dyn Error>>;
}

impl ClaimedOrFail for ClaimOutcomeV1 {
    fn claimed_or_fail(&self) -> Result<PreparedMutationV1, Box<dyn Error>> {
        match self {
            ClaimOutcomeV1::Claimed(claim) => Ok(claim.clone()),
            ClaimOutcomeV1::Replay { .. } | ClaimOutcomeV1::ReplayRepoMap { .. } => {
                Err("expected a fresh claim, got a replay".into())
            }
        }
    }
}

/// Scripted Unix-millisecond clock: each transaction consumes the next
/// sample, exhaustion repeats the last sample so the script stays total,
/// and every sample is counted.
struct ScriptedClock {
    script: Mutex<VecDeque<u64>>,
    samples: AtomicU64,
}

impl ScriptedClock {
    fn new(script: &[u64]) -> Self {
        Self {
            script: Mutex::new(script.iter().copied().collect()),
            samples: AtomicU64::new(0),
        }
    }

    fn samples(&self) -> u64 {
        self.samples.load(Ordering::SeqCst)
    }
}

impl CatalogClockPort for ScriptedClock {
    fn now_unix_ms(&self) -> u64 {
        let _prior = self.samples.fetch_add(1, Ordering::SeqCst);
        self.script
            .lock()
            .expect("scripted catalog clock poisoned")
            .pop_front()
            .expect("scripted catalog clock exhausted")
    }
}

#[test]
#[should_panic(expected = "scripted catalog clock exhausted")]
fn scripted_clock_exhaustion_is_not_a_repeated_time_value() {
    let clock = ScriptedClock::new(&[7]);
    assert_eq!(clock.now_unix_ms(), 7);
    let _unexpected = clock.now_unix_ms();
}

#[test]
#[should_panic(expected = "scripted catalog clock poisoned")]
fn scripted_clock_poison_is_not_a_repeated_time_value() {
    let clock = Arc::new(ScriptedClock::new(&[7]));
    let worker_clock = Arc::clone(&clock);
    let _worker = std::thread::spawn(move || {
        let _guard = worker_clock.script.lock().expect("first lock succeeds");
        panic!("poison the scripted clock");
    })
    .join();
    let _unexpected = clock.now_unix_ms();
}

fn open_scripted(
    temp: &tempfile::TempDir,
    clock: Arc<ScriptedClock>,
) -> Result<SqliteCatalog, CoreError> {
    SqliteCatalog::open_with_clock(temp.path(), Duration::from_millis(100), clock)
}

fn busy_code(error: &CoreError) -> Result<(), Box<dyn Error>> {
    match typed_code(error) {
        Some(code) if code == CATALOG_BUSY_CODE => Ok(()),
        other => Err(format!("expected CATALOG_BUSY, got {other:?}").into()),
    }
}

#[test]
fn claim_boundary_follows_the_scripted_now() -> TestResult {
    let temp = tempfile::tempdir()?;
    let clock = Arc::new(ScriptedClock::new(&[1_000, 1_999, 2_000, 2_001]));
    let catalog = open_scripted(&temp, Arc::clone(&clock))?;
    let key = key(
        IngestOperationKindV1::SearchCorpus,
        11,
        "digest-clock-claim",
    );
    let body = [7_u8; 32];

    // Fresh claim at t=1000 with deadline 2000.
    match catalog.claim_prepared(&key, &body, "clock-owner", 2_000, &body)? {
        ClaimOutcomeV1::Claimed(claim) => {
            if claim.lease_deadline_ms != 2_000 {
                return Err("fresh claim must persist the caller deadline".into());
            }
        }
        ClaimOutcomeV1::Replay { .. } | ClaimOutcomeV1::ReplayRepoMap { .. } => {
            return Err("expected a fresh claim, got a replay".into());
        }
    }
    // now < deadline: the live claim refuses its own owner as busy.
    match catalog.claim_prepared(&key, &body, "clock-owner", 2_000, &body) {
        Err(error) => busy_code(&error)?,
        Ok(outcome) => {
            return Err(format!("a live claim must refuse busy, got {outcome:?}").into());
        }
    }
    // now == deadline: the lease lapsed exactly, so the owner takes over.
    match catalog.claim_prepared(&key, &body, "clock-owner", 2_000, &body)? {
        ClaimOutcomeV1::Claimed(_) => {}
        ClaimOutcomeV1::Replay { .. } | ClaimOutcomeV1::ReplayRepoMap { .. } => {
            return Err("an exactly-lapsed claim must take over, got a replay".into());
        }
    }
    // now > deadline: the owner takes over again.
    match catalog.claim_prepared(&key, &body, "clock-owner", 2_000, &body)? {
        ClaimOutcomeV1::Claimed(_) => {}
        ClaimOutcomeV1::Replay { .. } | ClaimOutcomeV1::ReplayRepoMap { .. } => {
            return Err("a lapsed claim must take over, got a replay".into());
        }
    }
    if clock.samples() != 4 {
        return Err(format!(
            "four transactions must sample exactly four times, got {}",
            clock.samples()
        )
        .into());
    }
    Ok(())
}

#[test]
fn recover_boundary_follows_the_scripted_now() -> TestResult {
    let temp = tempfile::tempdir()?;
    // Each key is claimed fresh (t=1000, deadline 2000), then recovered
    // at its probe time; recovery mutates expired rows, so each probe
    // needs its own key.
    let clock = Arc::new(ScriptedClock::new(&[
        1_000, 1_999, //
        1_000, 2_000, //
        1_000, 2_001, //
    ]));
    let catalog = open_scripted(&temp, Arc::clone(&clock))?;
    let body = [9_u8; 32];
    for (index, digest) in ["digest-live", "digest-equal", "digest-past"]
        .iter()
        .enumerate()
    {
        let key = key(IngestOperationKindV1::SearchCorpus, 21, digest);
        match catalog.claim_prepared(&key, &body, "clock-owner", 2_000, &body)? {
            ClaimOutcomeV1::Claimed(_) => {}
            ClaimOutcomeV1::Replay { .. } | ClaimOutcomeV1::ReplayRepoMap { .. } => {
                return Err("expected a fresh claim, got a replay".into());
            }
        }
        match catalog.recover(&key)? {
            OperationInspectV1::InFlight {
                state,
                owner,
                lease_deadline_ms,
                ..
            } if index == 0 => {
                if state != OperationJournalStateV1::Claimed
                    || owner != "clock-owner"
                    || lease_deadline_ms != 2_000
                {
                    return Err("a live recover must report the intact claim".into());
                }
            }
            OperationInspectV1::Absent if index > 0 => {}
            other => {
                return Err(format!("probe {index} recovered wrong: {other:?}").into());
            }
        }
    }
    if clock.samples() != 6 {
        return Err(format!(
            "six transactions must sample exactly six times, got {}",
            clock.samples()
        )
        .into());
    }
    Ok(())
}

#[test]
fn mutation_enter_boundary_and_deadline_arithmetic() -> TestResult {
    let temp = tempfile::tempdir()?;
    let clock = Arc::new(ScriptedClock::new(&[1_000, 1_499, 1_500, 2_001, 2_002]));
    let catalog = open_scripted(&temp, Arc::clone(&clock))?;

    // Enter at t=1000 with a 500ms lease: the deadline is exactly 1500.
    let first = catalog.enter("scope-clock", "worker-1", 500)?;
    if first.deadline_ms != 1_500 {
        return Err(format!("deadline must be now + lease, got {}", first.deadline_ms).into());
    }
    // now < deadline with another owner: busy.
    match catalog.enter("scope-clock", "worker-2", 500) {
        Err(error) => busy_code(&error)?,
        Ok(lease) => {
            return Err(format!("a live lease must refuse busy, got {lease:?}").into());
        }
    }
    // now == deadline: the lease lapsed exactly, so the rival takes over
    // with a deadline computed from the probe time.
    let second = catalog.enter("scope-clock", "worker-2", 500)?;
    if second.deadline_ms != 2_000 {
        return Err(format!(
            "takeover deadline must be now + lease, got {}",
            second.deadline_ms
        )
        .into());
    }
    // now > deadline: a third worker takes over.
    let third = catalog.enter("scope-clock", "worker-3", 500)?;
    if third.deadline_ms != 2_501 || third.owner != "worker-3" {
        return Err(format!("a lapsed lease must take over, got {third:?}").into());
    }
    // The same owner re-enters a live lease without waiting for expiry.
    let fourth = catalog.enter("scope-clock", "worker-3", 100)?;
    if fourth.deadline_ms != 2_102 {
        return Err(format!(
            "same-owner re-enter must recompute the deadline, got {}",
            fourth.deadline_ms
        )
        .into());
    }
    if clock.samples() != 5 {
        return Err(format!(
            "five transactions must sample exactly five times, got {}",
            clock.samples()
        )
        .into());
    }
    Ok(())
}

#[test]
fn scripted_clock_steps_between_transactions_without_resampling() -> TestResult {
    let temp = tempfile::tempdir()?;
    let clock = Arc::new(ScriptedClock::new(&[1_000, 2_000]));
    let catalog = open_scripted(&temp, Arc::clone(&clock))?;
    let first = catalog.enter("scope-a", "worker-1", 500)?;
    let second = catalog.enter("scope-b", "worker-1", 500)?;
    if first.deadline_ms != 1_500 || second.deadline_ms != 2_500 {
        return Err(format!(
            "independent transactions must see stepped times, got {} and {}",
            first.deadline_ms, second.deadline_ms
        )
        .into());
    }
    if clock.samples() != 2 {
        return Err(format!(
            "two transactions must sample exactly twice, got {}",
            clock.samples()
        )
        .into());
    }
    Ok(())
}
