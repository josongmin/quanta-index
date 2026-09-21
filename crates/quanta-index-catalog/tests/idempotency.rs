//! QI-BB-032 / SEP-21 P02B — the operation journal's contract, against
//! the real engine.
//!
//! 1. inspect → claim → apply → fenced commit: a fresh key is claimed,
//!    committed with a receipt, and every later attempt of the same body
//!    replays that receipt and sequence; a different body is a typed
//!    conflict.
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

#![forbid(unsafe_code)]

use std::error::Error;
use std::time::Duration;

use quanta_index_catalog::{CATALOG_FILE_NAME, SqliteCatalog, catalog_dir};
use quanta_index_contract::{
    BatchPublishReceipt, IngestOperationKindV1, ManifestGeneration, RepoId, RevisionId,
};
use quanta_index_core::{
    BATCH_DIGEST_CONFLICT_CODE, CATALOG_BUSY_CODE, CATALOG_ROW_CORRUPT_CODE, ClaimOutcomeV1,
    CoreError, IdempotencyCatalogPort, IdempotencyKeyV1, OPERATION_REPLAY_FLOOR_CODE,
    OperationInspectV1, PreparedMutationV1,
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
        ClaimOutcomeV1::Replay { .. } => {
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
        other @ (ClaimOutcomeV1::Claimed(_) | ClaimOutcomeV1::Replay { .. }) => {
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
    if !matches!(
        catalog.inspect(&key)?,
        OperationInspectV1::Committed {
            durable_sequence: 3,
            ..
        }
    ) {
        return Err("a committed recovery replays like any apply".into());
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
    if !matches!(
        catalog.claim_prepared(&key, &[0_u8; 32], "test", LONG_LEASE_MS, &[0_u8; 32])?,
        ClaimOutcomeV1::Claimed(_)
    ) {
        return Err("once released, the write proceeds".into());
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

/// Test helper: a claim outcome that must be a claim.
trait ClaimedOrFail {
    fn claimed_or_fail(&self) -> Result<PreparedMutationV1, Box<dyn Error>>;
}

impl ClaimedOrFail for ClaimOutcomeV1 {
    fn claimed_or_fail(&self) -> Result<PreparedMutationV1, Box<dyn Error>> {
        match self {
            ClaimOutcomeV1::Claimed(claim) => Ok(claim.clone()),
            ClaimOutcomeV1::Replay { .. } => Err("expected a fresh claim, got a replay".into()),
        }
    }
}
