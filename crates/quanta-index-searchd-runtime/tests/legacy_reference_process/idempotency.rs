//! The durable operation journal of an externally pinned Legacy process.

use std::path::Path;

use quanta_index_contract::{IngestOperationKindV1, RepoId, RevisionId, SearchPlaneErrorCodeV2};
use quanta_index_core::OperationJournalStateV1;
use quanta_index_sdk::SdkError;
use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, legacy_source_batch};
use rusqlite::{Connection, OpenFlags, params};

use super::TestResult;

const GENERATION: u64 = 1;
const PATH: &str = "src/process-replay.rs";

#[derive(Debug, PartialEq, Eq)]
struct CommittedRow {
    batch_digest: String,
    durable_sequence: i64,
    commitment: Vec<u8>,
}

fn read_single_commit(
    root: &Path,
    repo: &RepoId,
    revision: &RevisionId,
) -> TestResult<CommittedRow> {
    let catalog =
        quanta_index_catalog::catalog_dir(root).join(quanta_index_catalog::CATALOG_FILE_NAME);
    let connection = Connection::open_with_flags(catalog, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(connection.query_row(
        "SELECT batch_digest, durable_sequence, row_sha256
         FROM idempotency_v2
         WHERE kind = ?1 AND repo_id = ?2 AND revision_id = ?3 AND generation = ?4
           AND state = ?5 AND (SELECT COUNT(*) FROM idempotency_v2) = 1",
        params![
            IngestOperationKindV1::SearchCorpus.as_code_str(),
            repo.as_str(),
            revision.as_str(),
            i64::try_from(GENERATION)?,
            OperationJournalStateV1::Committed.as_code(),
        ],
        |row| {
            Ok(CommittedRow {
                batch_digest: row.get(0)?,
                durable_sequence: row.get(1)?,
                commitment: row.get(2)?,
            })
        },
    )?)
}

#[test]
fn binary_restart_replays_original_operation_and_refuses_conflicting_source_digest() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let original = legacy_source_batch(
        &repo,
        &revision,
        GENERATION,
        None,
        &[(PATH, "fn original_source() { os_restart_replay }", 1)],
    )?;
    let first_process = binary.start(&state, 8)?;
    let first = first_process
        .connect()?
        .search_corpus()
        .publish_outcome(&original)?
        .receipt;
    first_process.stop()?;
    let committed = read_single_commit(&state, &repo, &revision)?;
    if !first.applied
        || first.durable_sequence == 0
        || first.batch_digest != committed.batch_digest
        || first.durable_sequence != u64::try_from(committed.durable_sequence)?
        || committed.commitment.len() != 32
    {
        return Err(format!(
            "first SDK receipt did not bind the durable operation: {first:?}, {committed:?}"
        )
        .into());
    }

    let second_process = binary.start(&state, 8)?;
    let client = second_process.connect()?;
    let replay = client.search_corpus().publish_outcome(&original)?.receipt;
    if replay != first.replayed() {
        return Err(format!("binary restart changed the replay receipt: {replay:?}").into());
    }
    // Same source event and generation, with a distinct body for the same file.
    let conflicting = legacy_source_batch(
        &repo,
        &revision,
        GENERATION,
        None,
        &[(PATH, "fn alternate_source() { os_restart_replay }", 1)],
    )?;
    match client.search_corpus().publish_outcome(&conflicting) {
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::BatchDigestConflict,
            ..
        }) => {}
        other => {
            return Err(
                format!("different source body did not cause a typed conflict: {other:?}").into(),
            );
        }
    }
    second_process.stop()?;
    let after = read_single_commit(&state, &repo, &revision)?;
    super::require_equal(
        &after,
        &committed,
        "durable apply after conflict and restart",
    )?;
    binary.verify()
}
