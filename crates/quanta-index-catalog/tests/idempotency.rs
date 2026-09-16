//! QI-BB-032 — the idempotency catalog's contract, against the real engine.
//!
//! 1. intent → apply → finalize: a fresh key is begun, finalized with a
//!    receipt, and every later `begin` of the same body is a replay carrying
//!    that receipt and sequence; a different body is a typed conflict.
//! 2. A begun-but-unfinalized key (a crash before finalize) resumes.
//! 3. Rows verify their own digest: a bit flipped in the stored body hash is
//!    a typed `CATALOG_ROW_CORRUPT`, never a silent replay or conflict.
//! 4. Sequences are unique and monotonic across keys and survive reopen.
//! 5. A second writer holding the database past the busy budget is a typed
//!    `CATALOG_BUSY`.
//! 6. Forgetting a generation drops exactly its records.

#![forbid(unsafe_code)]

use std::error::Error;
use std::time::Duration;

use quanta_index_catalog::{IDEMPOTENCY_CATALOG_FILE_NAME, SqliteIdempotencyCatalog, catalog_dir};
use quanta_index_contract::{BatchPublishReceipt, ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{
    BATCH_DIGEST_CONFLICT_CODE, CATALOG_BUSY_CODE, CATALOG_ROW_CORRUPT_CODE, CoreError,
    IdempotencyBeginV1, IdempotencyCatalogPort, IdempotencyKeyV1, IngestOperationKindV1,
};

type TestResult = Result<(), Box<dyn Error>>;

fn key(kind: IngestOperationKindV1, generation: u64, digest: &str) -> IdempotencyKeyV1 {
    IdempotencyKeyV1 {
        kind,
        repo_id: RepoId::new("repo-cat"),
        revision_id: RevisionId::new("rev-cat"),
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

fn typed_code(error: &CoreError) -> Option<&str> {
    match error {
        CoreError::Typed { code, .. } => Some(code.as_str()),
        CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_) => None,
    }
}

#[test]
fn a_finalized_record_replays_and_a_different_body_conflicts() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteIdempotencyCatalog::open(temp.path(), Duration::from_millis(100))?;
    let key = key(IngestOperationKindV1::SearchCorpus, 3, "digest-a");
    let body = [1_u8; 32];

    if catalog.begin(&key, &body)? != IdempotencyBeginV1::Fresh {
        return Err("a new key must be fresh".into());
    }
    let sequence = catalog.finalize(&key, &body, &receipt(3, "digest-a", 2))?;
    if sequence != 1 {
        return Err(format!("first apply must be sequence 1, got {sequence}").into());
    }
    match catalog.begin(&key, &body)? {
        IdempotencyBeginV1::Replay {
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
        other @ (IdempotencyBeginV1::Fresh | IdempotencyBeginV1::Resume) => {
            return Err(format!("expected a replay, got {other:?}").into());
        }
    }
    let different = [2_u8; 32];
    let conflict = catalog
        .begin(&key, &different)
        .expect_err("a different body under the same key must be refused");
    if typed_code(&conflict) != Some(BATCH_DIGEST_CONFLICT_CODE) {
        return Err(format!("expected a typed conflict, got {conflict:?}").into());
    }
    // The conflict wrote nothing: the original record still replays.
    if !matches!(
        catalog.begin(&key, &body)?,
        IdempotencyBeginV1::Replay { .. }
    ) {
        return Err("a refused conflict must leave the record untouched".into());
    }
    // Finalizing an applied record again is a caller defect.
    if catalog
        .finalize(&key, &body, &receipt(3, "digest-a", 2))
        .is_ok()
    {
        return Err("finalizing an applied record twice must fail".into());
    }
    Ok(())
}

#[test]
fn a_crash_before_finalize_resumes_and_then_finalizes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let key = key(IngestOperationKindV1::History, 5, "digest-h");
    let body = [9_u8; 32];
    {
        let catalog = SqliteIdempotencyCatalog::open(temp.path(), Duration::from_millis(100))?;
        if catalog.begin(&key, &body)? != IdempotencyBeginV1::Fresh {
            return Err("fresh".into());
        }
        // The process dies here: the record is in progress on disk.
    }
    let catalog = SqliteIdempotencyCatalog::open(temp.path(), Duration::from_millis(100))?;
    if catalog.begin(&key, &body)? != IdempotencyBeginV1::Resume {
        return Err("an in-progress record must resume".into());
    }
    let sequence = catalog.finalize(&key, &body, &receipt(5, "digest-h", 1))?;
    if sequence != 1 {
        return Err(format!("resumed apply must take the next sequence, got {sequence}").into());
    }
    if !matches!(
        catalog.begin(&key, &body)?,
        IdempotencyBeginV1::Replay {
            durable_sequence: 1,
            ..
        }
    ) {
        return Err("a finalized resume replays like any apply".into());
    }
    Ok(())
}

#[test]
fn a_row_that_does_not_match_its_digest_is_refused_typed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteIdempotencyCatalog::open(temp.path(), Duration::from_millis(100))?;
    let key = key(IngestOperationKindV1::Dirty, 7, "digest-d");
    let body = [4_u8; 32];
    let _fresh = catalog.begin(&key, &body)?;
    let _sequence = catalog.finalize(&key, &body, &receipt(7, "digest-d", 0))?;
    drop(catalog);

    // Flip one byte of the stored body hash behind the catalog's back, as
    // bit-rot would; the engine's own integrity check does not notice.
    let path = catalog_dir(temp.path()).join(IDEMPOTENCY_CATALOG_FILE_NAME);
    let connection = rusqlite::Connection::open(&path)?;
    let changed = connection.execute(
        "UPDATE idempotency_v1
         SET body_sha256 = CAST(X'0404040404040404040404040404040404040404040404040404040404040405' AS BLOB)
         WHERE batch_digest = 'digest-d'",
        [],
    )?;
    if changed != 1 {
        return Err(format!("expected to corrupt one row, changed {changed}").into());
    }
    drop(connection);

    let catalog = SqliteIdempotencyCatalog::open(temp.path(), Duration::from_millis(100))?;
    let refused = catalog
        .begin(&key, &body)
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
        let catalog = SqliteIdempotencyCatalog::open(temp.path(), Duration::from_millis(100))?;
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
            let _fresh = catalog.begin(&key, &body)?;
            sequences.push(catalog.finalize(&key, &body, &receipt(generation, "digest", 0))?);
        }
    }
    let catalog = SqliteIdempotencyCatalog::open(temp.path(), Duration::from_millis(100))?;
    let key = key(IngestOperationKindV1::RepoMeta, 9, "digest-late");
    let body = [8_u8; 32];
    let _fresh = catalog.begin(&key, &body)?;
    sequences.push(catalog.finalize(&key, &body, &receipt(9, "digest-late", 0))?);
    if sequences != vec![1, 2, 3, 4] {
        return Err(format!("sequences must be 1..=4 in order, got {sequences:?}").into());
    }
    Ok(())
}

#[test]
fn a_held_write_lock_past_the_busy_budget_is_typed_busy() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteIdempotencyCatalog::open(temp.path(), Duration::from_millis(60))?;
    let path = catalog_dir(temp.path()).join(IDEMPOTENCY_CATALOG_FILE_NAME);
    // A foreign writer holds the database.
    let holder = rusqlite::Connection::open(&path)?;
    holder.execute_batch("BEGIN IMMEDIATE;")?;

    let key = key(IngestOperationKindV1::Dirty, 1, "digest-busy");
    let started = std::time::Instant::now();
    let refused = catalog
        .begin(&key, &[0_u8; 32])
        .expect_err("a held lock past the budget must be refused");
    let waited = started.elapsed();
    if typed_code(&refused) != Some(CATALOG_BUSY_CODE) {
        return Err(format!("expected CATALOG_BUSY, got {refused:?}").into());
    }
    if waited < Duration::from_millis(60) || waited > Duration::from_secs(5) {
        return Err(format!("the busy budget was not honored: waited {waited:?}").into());
    }
    holder.execute_batch("ROLLBACK;")?;
    if catalog.begin(&key, &[0_u8; 32])? != IdempotencyBeginV1::Fresh {
        return Err("once released, the write proceeds".into());
    }
    Ok(())
}

#[test]
fn forgetting_a_generation_drops_exactly_its_records() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = SqliteIdempotencyCatalog::open(temp.path(), Duration::from_millis(100))?;
    for (generation, digest) in [(1, "a"), (1, "b"), (2, "c")] {
        let key = key(IngestOperationKindV1::SearchCorpus, generation, digest);
        let body = [u8::try_from(generation)?; 32];
        let _fresh = catalog.begin(&key, &body)?;
        let _sequence = catalog.finalize(&key, &body, &receipt(generation, digest, 0))?;
    }
    let removed = catalog.forget_generation(
        &RepoId::new("repo-cat"),
        &RevisionId::new("rev-cat"),
        ManifestGeneration::new(1),
    )?;
    if removed != 2 {
        return Err(format!("expected 2 records forgotten, got {removed}").into());
    }
    if catalog.begin(
        &key(IngestOperationKindV1::SearchCorpus, 1, "a"),
        &[1_u8; 32],
    )? != IdempotencyBeginV1::Fresh
    {
        return Err("a forgotten key is fresh again".into());
    }
    if !matches!(
        catalog.begin(
            &key(IngestOperationKindV1::SearchCorpus, 2, "c"),
            &[2_u8; 32]
        )?,
        IdempotencyBeginV1::Replay { .. }
    ) {
        return Err("another generation's record must survive".into());
    }
    Ok(())
}
