use std::time::{Duration, Instant};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    AuxEpochV1, BatchPublishReceipt, HistoryDiffHunkUpsert, HistoryIngestBatch, HistoryRefDelete,
    HistoryRefMutation, HistoryRefUpsert, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneErrorCodeV2, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse,
    index::Digest,
    lex::{CommitRecord, CommitSha, DiffHunkRecord},
    results::DiffHunkSide,
};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError,
    domains::auxiliary::{HistoryReadbackRowV1, HistorySourceCutV1},
    index::{IndexError, WorkBudget, WorkLimits, WorkUsage},
};
use quanta_index_ipc::encode_cbor_payload;
use quanta_index_search_plane::DirectHistoryMaterializer;
use rusqlite::{Connection, params};
use sha2::{Digest as _, Sha256};

use super::{E2eRuntime, TestResult};

const PATH: &str = "src/history_readback.rs";
const MAIN: &str = "refs/heads/main";
const OLD_REF: &str = "refs/heads/deleted";
const TAG: &str = "move-me";
const OLD_TAG: &str = "deleted-tag";

pub(super) fn commit(sha: CommitSha, parents: Vec<CommitSha>, message: &str) -> CommitRecord {
    CommitRecord {
        wire_version: 1,
        sha,
        parents,
        author_time_ms: 11,
        committer_time_ms: 12,
        applied_at_ms: 99,
        author: "author".into(),
        author_name: Some("Published Author".into()),
        author_email: Some("author@example.invalid".into()),
        committer: "committer".into(),
        committer_name: Some("Published Committer".into()),
        committer_email: Some("committer@example.invalid".into()),
        message: message.into(),
        is_merge: false,
        tags: vec!["tag-at-publication".into()],
    }
}

pub(super) fn diff(side: DiffHunkSide, text: &str) -> DiffHunkRecord {
    DiffHunkRecord {
        wire_version: 1,
        hunk_header: "@@ -5 +5 @@".into(),
        side,
        added_text: text.into(),
        removed_text: "published deletion".into(),
        touched_text: "published context".into(),
        byte_start: 21,
        byte_end: 42,
    }
}

fn upsert(name: &str, sha: CommitSha) -> HistoryRefMutation {
    HistoryRefMutation::Upsert(HistoryRefUpsert {
        name: name.into(),
        sha,
    })
}

fn delete(name: &str) -> HistoryRefMutation {
    HistoryRefMutation::Delete(HistoryRefDelete { name: name.into() })
}

pub(super) fn budget() -> WorkBudget {
    read_budget(1_000_000, 512 * 1024 * 1024)
}

fn read_budget(reads: u64, bytes: u64) -> WorkBudget {
    WorkBudget::new(
        WorkLimits {
            reads,
            writes: 1_000_000,
            mutations: 1_000_000,
            bytes,
        },
        Instant::now() + Duration::from_secs(60),
    )
}

fn digest_fixture(
    key: &AuxiliaryGenerationKeyV1,
    rows: &[(&str, Vec<u8>, Vec<u8>)],
) -> Result<Digest, std::num::TryFromIntError> {
    let mut hash = Sha256::new();
    hash.update(b"quanta-index:history-source-snapshot:v1\0");
    for field in [key.repo_id.as_str(), key.revision_id.as_str()] {
        hash.update(u64::try_from(field.len())?.to_le_bytes());
        hash.update(field.as_bytes());
    }
    hash.update(key.generation.get().to_le_bytes());
    for (family, key, body) in rows {
        for field in [family.as_bytes(), key.as_slice(), body.as_slice()] {
            hash.update(u64::try_from(field.len())?.to_le_bytes());
            hash.update(field);
        }
    }
    Ok(Digest(hash.finalize().into()))
}

// Fixture boundary: the real Legacy ACK binds a replay position for these
// two batches. The installed producer supplies its own fenced cursor format.
fn replay_start_fixture(receipt: &BatchPublishReceipt) -> Digest {
    let mut hash = Sha256::new();
    hash.update(b"quanta-index:test:legacy-history-ack:v1\0");
    hash.update(receipt.generation.get().to_le_bytes());
    hash.update(receipt.durable_sequence.to_le_bytes());
    hash.update(receipt.batch_digest.as_bytes());
    Digest(hash.finalize().into())
}

fn assert_refused(
    catalog: &SqliteCatalog,
    source: &HistorySourceCutV1,
    replay: &HistorySourceCutV1,
    expected: SearchPlaneErrorCodeV2,
) -> TestResult {
    let outcome = DirectHistoryMaterializer::read_source_snapshot(
        catalog,
        source,
        replay,
        &mut budget(),
        &mut |_, _budget| Ok(()),
    );
    match outcome {
        Err(error) if error.clone().into_search_plane_wire().0 == expected => Ok(()),
        observed => Err(format!("expected History refusal {expected:?}, got {observed:?}").into()),
    }
}

#[test]
fn history_source_readback_binds_two_legacy_publications_after_reopen() -> TestResult {
    let mut rt = E2eRuntime::boot_with_client_request_timeout(Duration::from_secs(60))?;
    rt.ingest_text("repo", PATH, "fn history_readback() {}")?;
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
    };
    let first = commit(
        CommitSha::from_bytes([1; 20]),
        Vec::new(),
        "first publication",
    );
    let second = commit(
        CommitSha::from_bytes([2; 20]),
        vec![first.sha],
        "second publication",
    );
    let first_diff = diff(DiffHunkSide::After, "original addition");
    let second_diff = diff(DiffHunkSide::After, "next addition");
    let mut prior_publication = None::<(_, BatchPublishReceipt)>;
    let mut last_predecessor = None;
    for (commit, refs, tags, record, replaces, tombstones) in [
        (
            &first,
            vec![upsert(MAIN, first.sha), upsert(OLD_REF, first.sha)],
            vec![upsert(TAG, first.sha), upsert(OLD_TAG, first.sha)],
            &first_diff,
            6,
            0,
        ),
        (
            &second,
            vec![upsert(MAIN, second.sha), delete(OLD_REF)],
            vec![upsert(TAG, second.sha), delete(OLD_TAG)],
            &second_diff,
            4,
            2,
        ),
    ] {
        let batch = HistoryIngestBatch {
            repo_id: generation.repo_id.clone(),
            revision_id: generation.revision_id.clone(),
            generation: generation.generation,
            manifest_digest: None,
            batch_digest: String::new(),
            commits: vec![commit.clone()],
            refs,
            tags,
            diff_hunks: vec![HistoryDiffHunkUpsert {
                commit_sha: commit.sha,
                file_path: PATH.into(),
                record: record.clone(),
            }],
        };
        let request = super::e2e_harness::stamped_ingest_request(
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch),
        )?;
        let response = rt.ingest_once(request.clone())?;
        let SearchPlaneIngestIpcResponse::HistoryReceipt(receipt) = response else {
            return Err(
                format!("Legacy ingest did not return a History receipt: {response:?}").into(),
            );
        };
        if !receipt.applied
            || receipt.generation != generation.generation
            || receipt.durable_sequence == 0
            || prior_publication
                .as_ref()
                .is_some_and(|(_, prior)| receipt.durable_sequence <= prior.durable_sequence)
            || receipt.accepted_replace_scopes != replaces
            || receipt.accepted_tombstone_scopes != tombstones
        {
            return Err(format!("unexpected Legacy History receipt: {receipt:?}").into());
        }
        if let Some((_, prior_receipt)) = prior_publication.replace((request, receipt)) {
            last_predecessor = Some(replay_start_fixture(&prior_receipt));
        }
    }

    let sealed = rt.seal()?;
    if sealed != generation.generation {
        return Err("Legacy fixture sealed a different generation from the History batches".into());
    }
    rt.activate_last_sealed_generation()?;

    // Retry the exact body across boot as if the terminal ACK had been lost.
    // A new apply would overwrite the epoch even though its final rows match.
    rt.try_reopen_in_place()?;
    let (request, receipt) = prior_publication.ok_or("missing second History publication")?;
    let replay_start = replay_start_fixture(&receipt);
    let response = rt.ingest_once(request)?;
    let SearchPlaneIngestIpcResponse::HistoryReceipt(replayed) = response else {
        return Err(format!("Legacy ingest did not return a replay receipt: {response:?}").into());
    };
    if replayed != receipt.replayed() {
        return Err(format!("Legacy History retry diverged after restart: {replayed:?}").into());
    }
    rt.try_reopen_in_place()?;

    let catalog = SqliteCatalog::open(rt.state_root(), Duration::from_millis(500))?;
    let epoch = AuxEpochV1::new(2);
    let diff_key = |sha: CommitSha| [sha.as_bytes().as_slice(), b"\x1f", PATH.as_bytes()].concat();
    // Independent fixture in the specified stream order. Never read the
    // database to obtain the expected commitment.
    // StateMeta uses declaration order on disk. The sequence below mirrors
    // that wire layout without using the History writer or its row helpers.
    let mut state_meta = vec![0xa4];
    for field in [
        "commits_materialized",
        "refs_materialized",
        "tags_materialized",
        "diff_hunks_materialized",
    ] {
        state_meta.extend(encode_cbor_payload(&field)?);
        state_meta.push(0xf5);
    }
    let snapshot_digest = digest_fixture(
        &generation,
        &[
            ("epoch", vec![], encode_cbor_payload(&epoch)?),
            (
                "commit",
                first.sha.as_bytes().to_vec(),
                encode_cbor_payload(&first)?,
            ),
            (
                "commit",
                second.sha.as_bytes().to_vec(),
                encode_cbor_payload(&second)?,
            ),
            (
                "diff-hunk",
                diff_key(first.sha),
                encode_cbor_payload(&first_diff)?,
            ),
            (
                "diff-hunk",
                diff_key(second.sha),
                encode_cbor_payload(&second_diff)?,
            ),
            (
                "ref",
                MAIN.as_bytes().to_vec(),
                encode_cbor_payload(&second.sha)?,
            ),
            ("state-meta", vec![], state_meta),
            (
                "tag",
                TAG.as_bytes().to_vec(),
                encode_cbor_payload(&second.sha)?,
            ),
        ],
    )?;
    let source = HistorySourceCutV1 {
        generation,
        durable_epoch: epoch,
        snapshot_digest,
        replay_start,
    };
    let replay = HistorySourceCutV1 {
        replay_start: replay_start_fixture(&replayed),
        ..source.clone()
    };
    let mut rows = Vec::new();
    let mut scan_budget = budget();
    DirectHistoryMaterializer::read_source_snapshot(
        &catalog,
        &source,
        &replay,
        &mut scan_budget,
        &mut |row, _budget| {
            rows.push(row);
            Ok(())
        },
    )?;
    let expected_rows = [
        HistoryReadbackRowV1::Commit(first.clone()),
        HistoryReadbackRowV1::Commit(second.clone()),
        HistoryReadbackRowV1::DiffHunk(HistoryDiffHunkUpsert {
            commit_sha: first.sha,
            file_path: PATH.into(),
            record: first_diff,
        }),
        HistoryReadbackRowV1::DiffHunk(HistoryDiffHunkUpsert {
            commit_sha: second.sha,
            file_path: PATH.into(),
            record: second_diff,
        }),
        HistoryReadbackRowV1::Ref(HistoryRefUpsert {
            name: MAIN.into(),
            sha: second.sha,
        }),
        HistoryReadbackRowV1::Materialization {
            commits: true,
            refs: true,
            tags: true,
            diff_hunks: true,
        },
        HistoryReadbackRowV1::Tag(HistoryRefUpsert {
            name: TAG.into(),
            sha: second.sha,
        }),
    ];
    if rows != expected_rows {
        return Err(format!("History source projection drifted: {rows:?}").into());
    }
    if !matches!(
        scan_budget.usage(),
        WorkUsage {
            reads: 8,
            writes: 0,
            mutations: 0,
            ..
        }
    ) {
        return Err(format!("unexpected History read work: {:?}", scan_budget.usage()).into());
    }

    let mut stage_budget = WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1,
            mutations: 1,
            bytes: 512 * 1024 * 1024,
        },
        Instant::now() + Duration::from_secs(60),
    );
    let mut staged = 0_u64;
    let staging_refusal = CoreError::Storage("History fixture staging exhausted its Work".into());
    let outcome = DirectHistoryMaterializer::read_source_snapshot(
        &catalog,
        &source,
        &replay,
        &mut stage_budget,
        &mut |_, budget| {
            budget
                .charge(WorkUsage {
                    writes: 1,
                    mutations: 1,
                    ..WorkUsage::default()
                })
                .map_err(|error| {
                    if error == IndexError::ResourceLimited {
                        staging_refusal.clone()
                    } else {
                        CoreError::Storage(format!("unexpected staging result: {error}"))
                    }
                })?;
            staged = staged.saturating_add(1);
            Ok(())
        },
    );
    if outcome != Err(staging_refusal)
        || staged != 1
        || !matches!(
            stage_budget.usage(),
            WorkUsage {
                reads: 3,
                writes: 1,
                mutations: 1,
                ..
            }
        )
    {
        return Err(format!("History reader lost staging budget or refusal: {outcome:?}").into());
    }

    for mut limited in [read_budget(1, 512 * 1024 * 1024), read_budget(1_000_000, 1)] {
        let mut unfunded_visits = 0_u64;
        let outcome = DirectHistoryMaterializer::read_source_snapshot(
            &catalog,
            &source,
            &source,
            &mut limited,
            &mut |_, _budget| {
                unfunded_visits = unfunded_visits.saturating_add(1);
                Ok(())
            },
        );
        if unfunded_visits != 0
            || !matches!(
                outcome,
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::ServerOverloaded,
                    ..
                })
            )
        {
            return Err(format!("History scan crossed the Work limit: {outcome:?}").into());
        }
    }

    let mut cancelled = budget();
    cancelled.request().cancel_handle().cancel();
    let outcome = DirectHistoryMaterializer::read_source_snapshot(
        &catalog,
        &source,
        &source,
        &mut cancelled,
        &mut |_, _budget| {
            Err(CoreError::Storage(
                "cancelled read reached the visitor".into(),
            ))
        },
    );
    if !matches!(
        outcome,
        Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestCancelled,
            ..
        })
    ) || cancelled.usage() != WorkUsage::default()
    {
        return Err(format!("History read missed cancellation before work: {outcome:?}").into());
    }

    let wrong_replay = HistorySourceCutV1 {
        replay_start: last_predecessor.ok_or("missing first History publication")?,
        ..source.clone()
    };
    assert_refused(
        &catalog,
        &source,
        &wrong_replay,
        SearchPlaneErrorCodeV2::InvalidRequest,
    )?;
    let missing_replay = HistorySourceCutV1 {
        replay_start: Digest([0; 32]),
        ..source.clone()
    };
    assert_refused(
        &catalog,
        &missing_replay,
        &missing_replay,
        SearchPlaneErrorCodeV2::InvalidRequest,
    )?;
    let missing_snapshot = HistorySourceCutV1 {
        snapshot_digest: Digest([0; 32]),
        ..source.clone()
    };
    let mut missing_budget = budget();
    let outcome = DirectHistoryMaterializer::read_source_snapshot(
        &catalog,
        &missing_snapshot,
        &missing_snapshot,
        &mut missing_budget,
        &mut |_, _budget| {
            Err(CoreError::Storage(
                "uncommitted snapshot reached the visitor".into(),
            ))
        },
    );
    if !matches!(outcome, Err(CoreError::InvalidContract(_)))
        || missing_budget.usage() != WorkUsage::default()
    {
        return Err(format!("absent History digest crossed admission: {outcome:?}").into());
    }
    for (epoch, code) in [
        (0, SearchPlaneErrorCodeV2::AuxEpochUnknown),
        (1, SearchPlaneErrorCodeV2::AuxEpochExpired),
        (3, SearchPlaneErrorCodeV2::AuxEpochUnknown),
    ] {
        let wrong = HistorySourceCutV1 {
            durable_epoch: AuxEpochV1::new(epoch),
            ..source.clone()
        };
        assert_refused(&catalog, &wrong, &wrong, code)?;
    }
    for foreign in [
        AuxiliaryGenerationKeyV1 {
            repo_id: RepoId::new("foreign")?,
            ..source.generation.clone()
        },
        AuxiliaryGenerationKeyV1 {
            revision_id: RevisionId::new("foreign")?,
            ..source.generation.clone()
        },
        AuxiliaryGenerationKeyV1 {
            generation: ManifestGeneration::new(999),
            ..source.generation.clone()
        },
    ] {
        let wrong = HistorySourceCutV1 {
            generation: foreign,
            ..source.clone()
        };
        assert_refused(
            &catalog,
            &wrong,
            &wrong,
            SearchPlaneErrorCodeV2::AuxEpochUnknown,
        )?;
    }

    let sqlite = Connection::open(catalog.path())?;
    let key = source.generation.clone();
    let address = params![
        key.repo_id.as_str(),
        key.revision_id.as_str(),
        i64::try_from(key.generation.get())?
    ];
    if sqlite.execute(
        "DELETE FROM auxiliary_rows_v1 WHERE domain='history' AND repo_id=?1
          AND revision_id=?2 AND generation=?3 AND family='tag'",
        address,
    )? != 1
    {
        return Err("expected to remove exactly one History tag".into());
    }
    assert_refused(
        &catalog,
        &source,
        &source,
        SearchPlaneErrorCodeV2::CatalogRowCorrupt,
    )?;
    if sqlite.execute(
        "UPDATE auxiliary_rows_v1 SET value=x'00' WHERE domain='history' AND repo_id=?1
          AND revision_id=?2 AND generation=?3 AND family='diff-hunk'",
        address,
    )? != 2
    {
        return Err("expected to corrupt exactly two History diffs".into());
    }
    let outcome = DirectHistoryMaterializer::read_source_snapshot(
        &catalog,
        &source,
        &source,
        &mut budget(),
        &mut |_, _budget| Ok(()),
    );
    if !matches!(&outcome, Err(CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CatalogRowCorrupt, message
    }) if message.contains("its own digest"))
    {
        return Err(
            format!("History scan did not detect row digest corruption: {outcome:?}").into(),
        );
    }
    if sqlite.execute(
        "DELETE FROM auxiliary_rows_v1 WHERE domain='history' AND repo_id=?1
          AND revision_id=?2 AND generation=?3 AND family='epoch'",
        address,
    )? != 1
    {
        return Err("expected to remove exactly one History epoch".into());
    }
    assert_refused(
        &catalog,
        &source,
        &source,
        SearchPlaneErrorCodeV2::AuxEpochUnknown,
    )?;
    Ok(())
}
