use super::*;
use quanta_index_contract::{HistoryRefDelete, lex::CommitRecord};
use quanta_index_core::index::{
    MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkBudgetDenial, WorkLimits,
};
use quanta_index_ipc::{history_replay_advance_with_progress_v1, stamp_batch_digest_v1};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct ReplayMemoryLedger {
    live: AtomicU64,
    ceiling: AtomicU64,
    reservations: Mutex<Vec<(u64, MemoryLifetime, bool)>>,
}

impl MemoryLeaseOwner for ReplayMemoryLedger {
    fn release(&self, bytes: u64) {
        assert!(self.live.fetch_sub(bytes, Ordering::AcqRel) >= bytes);
    }
}

struct ReplayMemoryScope(Arc<ReplayMemoryLedger>);

impl MemoryScopePort for ReplayMemoryScope {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let ceiling = self.0.ceiling.load(Ordering::Acquire);
        let admitted = self
            .0
            .live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(bytes).filter(|value| *value <= ceiling)
            });
        self.0
            .reservations
            .lock()
            .expect("reservation ledger poisoned")
            .push((bytes, lifetime, admitted.is_ok()));
        let _previous_live = admitted.map_err(|_| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

fn replay_memory_budget(ceiling: u64) -> (WorkBudget, Arc<ReplayMemoryLedger>) {
    let ledger = Arc::new(ReplayMemoryLedger {
        live: AtomicU64::new(0),
        ceiling: AtomicU64::new(ceiling),
        reservations: Mutex::new(Vec::new()),
    });
    let mut work = budget();
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(ReplayMemoryScope(
        ledger.clone(),
    )))));
    (work, ledger)
}

fn budget() -> WorkBudget {
    WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        },
        Instant::now() + Duration::from_secs(60),
    )
}

fn replay_advance(
    before: Digest,
    ordinal: u64,
    batch: &HistoryIngestBatch,
    ack: &BatchPublishReceipt,
) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(history_replay_advance_with_progress_v1(
        before,
        ordinal,
        batch,
        ack,
        &mut |_| Ok::<(), std::convert::Infallible>(()),
    )?)
}

fn fixture() -> Result<
    (
        HistoryReplayFenceV1,
        [HistoryIngestBatch; 2],
        [BatchPublishReceipt; 2],
    ),
    Box<dyn std::error::Error>,
> {
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("history-source")?,
        revision_id: RevisionId::new("main")?,
        generation: ManifestGeneration::new(1),
    };
    let commit = |byte, parents| CommitRecord {
        wire_version: 1,
        sha: CommitSha::from_bytes([byte; 20]),
        parents,
        author_time_ms: 1,
        committer_time_ms: 2,
        applied_at_ms: 3,
        author: "author".into(),
        author_name: None,
        author_email: None,
        committer: "committer".into(),
        committer_name: None,
        committer_email: None,
        message: "source message".into(),
        is_merge: false,
        tags: Vec::new(),
    };
    let first = commit(1, Vec::new());
    let second = commit(2, vec![first.sha]);
    let mut batches = [
        HistoryIngestBatch {
            repo_id: generation.repo_id.clone(),
            revision_id: generation.revision_id.clone(),
            generation: generation.generation,
            manifest_digest: None,
            batch_digest: String::new(),
            commits: vec![first.clone()],
            refs: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: "main".into(),
                sha: first.sha,
            })],
            tags: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: "old".into(),
                sha: first.sha,
            })],
            diff_hunks: Vec::new(),
        },
        HistoryIngestBatch {
            repo_id: generation.repo_id.clone(),
            revision_id: generation.revision_id.clone(),
            generation: generation.generation,
            manifest_digest: None,
            batch_digest: String::new(),
            commits: vec![second.clone()],
            refs: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: "main".into(),
                sha: second.sha,
            })],
            tags: vec![HistoryRefMutation::Delete(HistoryRefDelete {
                name: "old".into(),
            })],
            diff_hunks: Vec::new(),
        },
    ];
    for batch in &mut batches {
        let sha = batch.commits[0].sha;
        batch
            .tags
            .push(HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: "kept".into(),
                sha,
            }));
        batch.diff_hunks.push(HistoryDiffHunkUpsert {
            commit_sha: sha,
            file_path: "file.rs".into(),
            record: quanta_index_contract::lex::DiffHunkRecord {
                wire_version: 1,
                hunk_header: "@@ -1 +1 @@".into(),
                side: quanta_index_contract::DiffHunkSide::After,
                added_text: "addition".into(),
                removed_text: "deletion".into(),
                touched_text: "context".into(),
                byte_start: 0,
                byte_end: 1,
            },
        });
        stamp_batch_digest_v1(batch)?;
    }
    let mut acks = [
        BatchPublishReceipt::empty_for(
            generation.generation,
            None,
            batches[0].batch_digest.clone(),
        ),
        BatchPublishReceipt::empty_for(
            generation.generation,
            None,
            batches[1].batch_digest.clone(),
        ),
    ];
    acks[0].accepted_replace_scopes = 5;
    acks[0].durable_sequence = 5;
    acks[1].accepted_replace_scopes = 4;
    acks[1].accepted_tombstone_scopes = 1;
    // Root-global durable sequence may have other domains between History rows.
    acks[1].durable_sequence = 17;
    let first_replay = Digest([1; 32]);
    let second_replay = replay_advance(first_replay, 1, &batches[0], &acks[0])?;
    let final_replay = replay_advance(second_replay, 2, &batches[1], &acks[1])?;
    let fence = HistoryReplayFenceV1 {
        generation,
        manifest_digest: None,
        publication_count: 2,
        first_replay,
        final_replay,
        final_durable_sequence: 17,
    };
    Ok((fence, batches, acks))
}

#[test]
fn complete_source_replay_keeps_moves_deletes_and_independent_row_digest() -> TestResult {
    let (fence, batches, acks) = fixture()?;
    let second_replay = replay_advance(fence.first_replay, 1, &batches[0], &acks[0])?;
    let source = HistorySourceReplayV1::new(fence.clone())?
        .apply(
            1,
            fence.first_replay,
            second_replay,
            &batches[0],
            &acks[0],
            &mut budget(),
        )?
        .apply(
            2,
            second_replay,
            fence.final_replay,
            &batches[1],
            &acks[1],
            &mut budget(),
        )?
        .finish(&mut budget())?;
    assert_eq!(source.state.commits().len(), 2);
    assert_eq!(
        source.state.refs().get("main"),
        Some(&batches[1].commits[0].sha)
    );
    assert_eq!(source.state.tags().len(), 1);
    assert_eq!(
        source.state.tags().get("kept"),
        Some(&batches[1].commits[0].sha)
    );
    assert_eq!(source.state.diff_hunks().len(), 2);
    assert!(source.state.tags_materialized());
    let mut rows = Vec::new();
    source.visit_rows(&mut budget(), &mut |row, _| {
        rows.push(row.clone());
        Ok(())
    })?;
    assert_eq!(
        rows.iter()
            .map(|row| row.key.family.as_code_str())
            .collect::<Vec<_>>(),
        [
            "epoch",
            "commit",
            "commit",
            "diff-hunk",
            "diff-hunk",
            "ref",
            "state-meta",
            "tag"
        ]
    );
    // Independent wire oracle: fixed canonical field order and length framing;
    // do not compute expected with HistorySnapshotDigestV1 or the target Catalog.
    let mut hash = sha2::Sha256::new();
    use sha2::Digest as _;
    hash.update(b"quanta-index:history-source-snapshot:v1\0");
    for field in ["history-source".as_bytes(), b"main"] {
        hash.update(u64::try_from(field.len())?.to_le_bytes());
        hash.update(field);
    }
    hash.update(1_u64.to_le_bytes());
    // Meta uses the owning struct's declared field order, independently named.
    struct Meta {
        commits_materialized: bool,
        refs_materialized: bool,
        tags_materialized: bool,
        diff_hunks_materialized: bool,
    }
    impl Serialize for Meta {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeStruct;
            let mut flags = serializer.serialize_struct("Meta", 4)?;
            flags.serialize_field("commits_materialized", &self.commits_materialized)?;
            flags.serialize_field("refs_materialized", &self.refs_materialized)?;
            flags.serialize_field("tags_materialized", &self.tags_materialized)?;
            flags.serialize_field("diff_hunks_materialized", &self.diff_hunks_materialized)?;
            flags.end()
        }
    }
    let expected = [
        (
            "epoch",
            Vec::new(),
            encode_cbor_payload(&AuxEpochV1::new(2))?,
        ),
        (
            "commit",
            vec![1; 20],
            encode_cbor_payload(&batches[0].commits[0])?,
        ),
        (
            "commit",
            vec![2; 20],
            encode_cbor_payload(&batches[1].commits[0])?,
        ),
        (
            "diff-hunk",
            [vec![1; 20], vec![0x1f], b"file.rs".to_vec()].concat(),
            encode_cbor_payload(&batches[0].diff_hunks[0].record)?,
        ),
        (
            "diff-hunk",
            [vec![2; 20], vec![0x1f], b"file.rs".to_vec()].concat(),
            encode_cbor_payload(&batches[1].diff_hunks[0].record)?,
        ),
        (
            "ref",
            b"main".to_vec(),
            encode_cbor_payload(&batches[1].commits[0].sha)?,
        ),
        (
            "state-meta",
            Vec::new(),
            encode_cbor_payload(&Meta {
                commits_materialized: true,
                refs_materialized: true,
                tags_materialized: true,
                diff_hunks_materialized: true,
            })?,
        ),
        (
            "tag",
            b"kept".to_vec(),
            encode_cbor_payload(&batches[1].commits[0].sha)?,
        ),
    ];
    for (family, key, value) in expected {
        for field in [family.as_bytes(), key.as_slice(), value.as_slice()] {
            hash.update(u64::try_from(field.len())?.to_le_bytes());
            hash.update(field);
        }
    }
    assert_eq!(source.cut.snapshot_digest, Digest(hash.finalize().into()));
    assert_eq!(source.cut.replay_start, fence.final_replay);
    Ok(())
}

#[test]
fn source_replay_admits_large_delta_and_refunds_a_partial_refusal() -> TestResult {
    const MIB: usize = 1024 * 1024;
    let (mut fence, mut batches, mut acks) = fixture()?;
    fence.manifest_digest = Some("a".repeat(64));
    for (batch, ack) in batches.iter_mut().zip(&mut acks) {
        batch.manifest_digest.clone_from(&fence.manifest_digest);
        ack.manifest_digest.clone_from(&fence.manifest_digest);
    }
    let first_sha = batches[0].commits[0].sha;
    batches[0]
        .tags
        .push(HistoryRefMutation::Upsert(HistoryRefUpsert {
            name: "".into(),
            sha: first_sha,
        }));
    stamp_batch_digest_v1(&mut batches[0])?;
    acks[0].batch_digest.clone_from(&batches[0].batch_digest);
    acks[0].accepted_replace_scopes += 1;
    batches[1]
        .tags
        .push(HistoryRefMutation::Delete(HistoryRefDelete {
            name: "".into(),
        }));
    batches[1].commits[0].message = "c".repeat(3 * MIB).into_boxed_str();
    batches[1].diff_hunks[0].record.added_text = "d".repeat(5 * MIB).into_boxed_str();
    stamp_batch_digest_v1(&mut batches[1])?;
    acks[1].batch_digest.clone_from(&batches[1].batch_digest);
    acks[1].accepted_tombstone_scopes += 1;
    let second_replay = replay_advance(fence.first_replay, 1, &batches[0], &acks[0])?;
    fence.final_replay = replay_advance(second_replay, 2, &batches[1], &acks[1])?;
    let full_fence = HistoryReplayFenceV1 {
        publication_count: 1,
        final_replay: second_replay,
        final_durable_sequence: acks[0].durable_sequence,
        ..fence.clone()
    };
    fence.first_replay = second_replay;
    let replay = |work: &mut WorkBudget| {
        HistorySourceReplayV1::new(full_fence.clone())?
            .apply(
                1,
                full_fence.first_replay,
                second_replay,
                &batches[0],
                &acks[0],
                work,
            )?
            .finish(work)?
            .resume(fence.clone())?
            .apply(
                2,
                second_replay,
                fence.final_replay,
                &batches[1],
                &acks[1],
                work,
            )?
            .finish(work)
    };

    let original = u64::try_from(8 * MIB + 16 * 1024)?;
    let (mut work, ledger) = replay_memory_budget(u64::try_from(64 * MIB)?);
    let input = work.reserve_memory(original, MemoryLifetime::Retained)?;
    let complete = replay(&mut work)?;
    let identity_bytes = u64::try_from(
        fence.generation.repo_id.as_str().len()
            + fence.generation.revision_id.as_str().len()
            + fence.manifest_digest.as_ref().map_or(0, String::len),
    )?;
    assert_eq!(
        complete
            ._custody
            .generation_identity
            .as_ref()
            .map(MemoryLease::bytes),
        Some(identity_bytes)
    );
    assert_eq!(
        ledger
            .reservations
            .lock()
            .expect("reservation ledger poisoned")
            .iter()
            .filter(|&&reservation| reservation == (identity_bytes, MemoryLifetime::Retained, true))
            .count(),
        1
    );
    let sha = batches[1].commits[0].sha;
    let key = HistoryDiffKey::new(sha, "file.rs");
    assert_eq!(complete.cut().durable_epoch, AuxEpochV1::new(2));
    assert!(!complete.state.tags().contains_key(""));
    assert_eq!(
        complete
            .state
            .commits()
            .get(&sha)
            .map(|row| row.message.len()),
        Some(3 * MIB)
    );
    assert_eq!(
        complete
            .state
            .diff_hunks()
            .get(&key)
            .map(|row| row.added_text.len()),
        Some(5 * MIB)
    );
    drop(complete);
    assert_eq!(ledger.live.load(Ordering::Acquire), original);
    drop(input);
    assert_eq!(ledger.live.load(Ordering::Acquire), 0);

    let (mut low, ledger) = replay_memory_budget(u64::try_from(14 * MIB)?);
    let input = low.reserve_memory(original, MemoryLifetime::Retained)?;
    assert!(matches!(
        replay(&mut low),
        Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::ServerOverloaded,
            ..
        })
    ));
    let reservations = ledger
        .reservations
        .lock()
        .expect("reservation ledger poisoned");
    let commit_backing = u64::try_from(batches[1].commits[0].message.len())?;
    let diff_backing = u64::try_from(batches[1].diff_hunks[0].record.added_text.len())?;
    let row_metadata_allowance = 64 * 1024;
    let commit_position = reservations
        .iter()
        .position(|&(bytes, lifetime, admitted)| {
            lifetime == MemoryLifetime::Retained
                && admitted
                && (commit_backing..commit_backing + row_metadata_allowance).contains(&bytes)
        })
        .ok_or("the later Commit backing was never admitted")?;
    let diff_position = reservations
        .iter()
        .position(|&(bytes, lifetime, admitted)| {
            lifetime == MemoryLifetime::Retained
                && !admitted
                && (diff_backing..diff_backing + row_metadata_allowance).contains(&bytes)
        })
        .ok_or("the later DiffHunk backing was not refused")?;
    assert!(
        commit_position < diff_position,
        "DiffHunk refusal preceded the Commit grant"
    );
    assert_eq!(
        reservations
            .iter()
            .filter(|(_, _, admitted)| !admitted)
            .count(),
        1
    );
    drop(reservations);
    // The admitted Commit and the previous complete fold are freed after the
    // observed DiffHunk refusal. The independently funded input lives.
    assert_eq!(ledger.live.load(Ordering::Acquire), original);
    drop(input);
    assert_eq!(ledger.live.load(Ordering::Acquire), 0);
    Ok(())
}

#[test]
fn native_history_successor_funds_next_identity_before_releasing_prior() -> TestResult {
    use quanta_index_core::index::{
        CollectionId, ProducerId, SlotId, StreamId, TenantId, WorkspaceKey, WorktreeId,
    };

    let (mut fence, batches, acks) = fixture()?;
    fence.publication_count = 1;
    fence.final_durable_sequence = acks[0].durable_sequence;
    fence.final_replay = replay_advance(fence.first_replay, 1, &batches[0], &acks[0])?;
    let full = |work: &mut WorkBudget| {
        HistorySourceReplayV1::new(fence.clone())?
            .apply(
                1,
                fence.first_replay,
                fence.final_replay,
                &batches[0],
                &acks[0],
                work,
            )?
            .finish(work)
    };
    let mut successor = batches[1].clone();
    successor.revision_id = RevisionId::new("next-revision")?;
    successor.generation = ManifestGeneration::new(2);
    successor.manifest_digest = Some("b".repeat(64));
    stamp_batch_digest_v1(&mut successor)?;
    let identity_bytes = u64::try_from(
        successor.repo_id.as_str().len() + successor.revision_id.as_str().len() + 64,
    )?;
    let operation = OperationKey {
        workspace: WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        },
        epoch: 1,
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence: 2,
    };
    let input = ObjectRef(Digest([7; 32]));

    let (mut limited, ledger) = replay_memory_budget(u64::MAX);
    let prior = full(&mut limited)?;
    let prior_live = ledger.live.load(Ordering::Acquire);
    let before = ledger
        .reservations
        .lock()
        .expect("reservation ledger poisoned")
        .len();
    ledger
        .ceiling
        .store(prior_live + identity_bytes - 1, Ordering::Release);
    assert!(matches!(
        prior.advance_native_generation(operation, 2, input, &successor, &mut limited),
        Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::ServerOverloaded,
            ..
        })
    ));
    assert_eq!(limited.first_denial(), Some(WorkBudgetDenial::Memory));
    assert_eq!(
        &ledger
            .reservations
            .lock()
            .expect("reservation ledger poisoned")[before..],
        &[(identity_bytes, MemoryLifetime::Retained, false)]
    );
    assert_eq!(ledger.live.load(Ordering::Acquire), 0);

    let (mut work, ledger) = replay_memory_budget(u64::MAX);
    let next =
        full(&mut work)?.advance_native_generation(operation, 2, input, &successor, &mut work)?;
    assert_eq!(next.cut().generation.revision_id, successor.revision_id);
    assert_eq!(next.cut().generation.generation, successor.generation);
    assert_eq!(next.cut().durable_epoch, AuxEpochV1::new(1));
    assert_eq!(next.state.commits().len(), 2);
    assert_eq!(
        next._custody
            .generation_identity
            .as_ref()
            .map(MemoryLease::bytes),
        Some(identity_bytes)
    );
    let row_bytes: u64 = next
        ._custody
        .rows
        .values()
        .flatten()
        .map(MemoryLease::bytes)
        .sum();
    assert_eq!(
        ledger.live.load(Ordering::Acquire),
        row_bytes + identity_bytes
    );
    drop(next);
    assert_eq!(ledger.live.load(Ordering::Acquire), 0);
    Ok(())
}

#[test]
fn source_replay_refuses_forged_advance_even_when_fence_agrees() -> TestResult {
    let (mut fence, batches, acks) = fixture()?;
    let second_replay = replay_advance(fence.first_replay, 1, &batches[0], &acks[0])?;
    let mut forged = fence.final_replay;
    forged.0[0] ^= 1;
    if forged == second_replay {
        forged.0[1] ^= 1;
    }
    fence.final_replay = forged;
    let first = HistorySourceReplayV1::new(fence.clone())?.apply(
        1,
        fence.first_replay,
        second_replay,
        &batches[0],
        &acks[0],
        &mut budget(),
    )?;
    let error = match first.apply(
        2,
        second_replay,
        forged,
        &batches[1],
        &acks[1],
        &mut budget(),
    ) {
        Ok(_) => panic!("forged replay advance was accepted"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message,
        } if message == "History replay advance differs from the original batch and ACK"
    ));
    Ok(())
}

#[test]
fn source_replay_charges_advance_before_applying_rows() -> TestResult {
    let (fence, batches, acks) = fixture()?;
    let before = fence.first_replay;
    let after = replay_advance(before, 1, &batches[0], &acks[0])?;
    // Cover the batch's own digest, leaving no credit for the replay hash.
    let mut batch_digest_bytes = 0;
    let measured_digest = canonical_batch_digest_with_progress_v1(&batches[0], &mut |bytes| {
        batch_digest_bytes += u64::try_from(bytes)?;
        Ok::<(), std::num::TryFromIntError>(())
    })?;
    assert!(matches_batch_digest_token_v1(
        &batches[0].batch_digest,
        &measured_digest
    ));
    let mut budget = WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: batch_digest_bytes,
            mutations: 1_000_000,
        },
        Instant::now() + Duration::from_secs(60),
    );
    let replay = HistorySourceReplayV1::new(fence)?;
    assert!(matches!(
        replay.apply(1, before, after, &batches[0], &acks[0], &mut budget),
        Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::ServerOverloaded,
            ..
        })
    ));
    assert_eq!(budget.first_denial(), Some(WorkBudgetDenial::Bytes));
    assert_eq!(
        budget.usage(),
        WorkUsage {
            bytes: batch_digest_bytes,
            ..WorkUsage::default()
        }
    );
    Ok(())
}

#[test]
fn source_replay_refuses_missing_duplicate_foreign_body_and_incomplete_boundary() -> TestResult {
    let (fence, batches, acks) = fixture()?;
    let second_replay = replay_advance(fence.first_replay, 1, &batches[0], &acks[0])?;
    let first = || {
        HistorySourceReplayV1::new(fence.clone())?.apply(
            1,
            fence.first_replay,
            second_replay,
            &batches[0],
            &acks[0],
            &mut budget(),
        )
    };
    assert!(first()?.finish(&mut budget()).is_err());
    assert!(
        first()?
            .apply(
                1,
                second_replay,
                fence.final_replay,
                &batches[1],
                &acks[1],
                &mut budget()
            )
            .is_err()
    );
    assert!(
        first()?
            .apply(
                3,
                second_replay,
                fence.final_replay,
                &batches[1],
                &acks[1],
                &mut budget()
            )
            .is_err()
    );
    assert!(
        first()?
            .apply(
                2,
                Digest([9; 32]),
                fence.final_replay,
                &batches[1],
                &acks[1],
                &mut budget()
            )
            .is_err()
    );
    let mut foreign = batches[1].clone();
    foreign.commits[0].message = "changed after signing".into();
    assert!(
        first()?
            .apply(
                2,
                second_replay,
                fence.final_replay,
                &foreign,
                &acks[1],
                &mut budget()
            )
            .is_err()
    );
    let mut ack = acks[1].clone();
    ack.accepted_tombstone_scopes = 0;
    assert!(
        first()?
            .apply(
                2,
                second_replay,
                fence.final_replay,
                &batches[1],
                &ack,
                &mut budget()
            )
            .is_err()
    );
    let mut exhausted = budget();
    exhausted.charge(WorkUsage {
        reads: 1_000_000,
        ..WorkUsage::default()
    })?;
    assert!(
        first()?
            .apply(
                2,
                second_replay,
                fence.final_replay,
                &batches[1],
                &acks[1],
                &mut exhausted
            )
            .is_err()
    );
    Ok(())
}
