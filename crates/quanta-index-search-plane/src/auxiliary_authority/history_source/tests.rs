use super::*;
use quanta_index_contract::{HistoryRefDelete, lex::CommitRecord};
use quanta_index_core::index::WorkLimits;
use quanta_index_ipc::stamp_batch_digest_v1;
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn std::error::Error>>;

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
    let fence = HistoryReplayFenceV1 {
        generation,
        manifest_digest: None,
        publication_count: 2,
        first_replay: Digest([1; 32]),
        final_replay: Digest([3; 32]),
        final_durable_sequence: 17,
    };
    Ok((fence, batches, acks))
}

#[test]
fn complete_source_replay_keeps_moves_deletes_and_independent_row_digest() -> TestResult {
    let (fence, batches, acks) = fixture()?;
    let source = HistorySourceReplayV1::new(fence.clone())?
        .apply(
            1,
            fence.first_replay,
            Digest([2; 32]),
            &batches[0],
            &acks[0],
            &mut budget(),
        )?
        .apply(
            2,
            Digest([2; 32]),
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
fn source_replay_refuses_missing_duplicate_foreign_body_and_incomplete_boundary() -> TestResult {
    let (fence, batches, acks) = fixture()?;
    let first = || {
        HistorySourceReplayV1::new(fence.clone())?.apply(
            1,
            fence.first_replay,
            Digest([2; 32]),
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
                Digest([2; 32]),
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
                Digest([2; 32]),
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
                Digest([2; 32]),
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
                Digest([2; 32]),
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
                Digest([2; 32]),
                fence.final_replay,
                &batches[1],
                &acks[1],
                &mut exhausted
            )
            .is_err()
    );
    Ok(())
}
