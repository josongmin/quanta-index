//! Legacy SDK History publication through Native Kernel CAS, SDK and UDS.
//! The initial replay position remains a fixture input, not installed-producer qualification.
use super::*;
use crate::legacy_searchd_binary::LegacySearchdBinary;
use quanta_index_contract::GenerationPin;
use quanta_index_contract::{
    AuxEpochV1, BatchPublishReceipt, DiffHunkSide, FileContributorEntry,
    FileContributorIdentityEntry, FileContributorIngestBatch, HistoryDiffHunkUpsert,
    HistoryIngestBatch, HistoryOrderV1, HistoryQueryRequest, HistoryRefMutation, HistoryRefUpsert,
    ManifestGeneration, RepoCommitRecencyEntry, RepoCommitRecencyIngestBatch, RepoId,
    RepoRelativePath, RevisionId,
    index::{
        AuxiliaryGenerationKeyV1, HistoryAppendV1, HistoryGenerationAdvanceV1,
        HistoryGenerationInputV1, HistoryReplayFenceV1, HistoryReplayPublicationV1,
        HistorySnapshotCheckpointV1, HistorySnapshotPageV1, HistorySnapshotRowV1,
        HistorySourceCutV1, HistorySourceInputV1, HistoryTransition, Refusal,
        SourceHistoryTransitionV1, ViewRef,
    },
    lex::{CommitRecord, CommitSha, DiffHunkRecord},
    native_protocol::{NativeHistoryCursor, NativeHistoryPage, NativeReadSessionRequest},
};
use quanta_index_ipc::{
    encode_cbor_payload, history_replay_advance_with_progress_v1,
    history_replay_genesis_with_progress_v1, stamp_batch_digest_v1,
};
use quanta_index_sdk::{ConnectOptions, QuantaIndex};
use quanta_index_search_plane::native_history::{
    NATIVE_HISTORY_FORMAT_V1, NATIVE_HISTORY_KIND_V1, NATIVE_HISTORY_SCHEMA_V1,
    NativeHistoryBuildV1, native_history_policy_v1,
};
use quanta_index_searchd_harness::history_fixture::HISTORY_LEDGER;

#[path = "history/dsl_fixture.rs"]
mod dsl_fixture;
#[path = "history/native_protected.rs"]
mod native_protected;
#[path = "history/native_restore.rs"]
mod native_restore;

fn history_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([81; 16]),
        kind: NATIVE_HISTORY_KIND_V1,
        group: PublicationGroupId([82; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::History,
        schema: NATIVE_HISTORY_SCHEMA_V1,
        policy: native_history_policy_v1(),
        format: NATIVE_HISTORY_FORMAT_V1,
    }
}

fn history_service(
    root: &std::path::Path,
    socket: &std::path::Path,
) -> TestResult<NativeIndexService> {
    // Reuse the existing read-session capacity, grants and real retention rail.
    let resources = read_session_resources(Duration::from_secs(30));
    let mut capabilities = reclaim_capabilities(None)?;
    let text = native_text_spec();
    capabilities.builders.register(
        AdapterDescriptor {
            kind: text.kind,
            schema: text.schema,
            format: text.format,
        },
        NativeTextIndex::new(TextRecipe::new(Digest([31; 32]))),
        |delta| Ok(delta.clone()),
    )?;
    let spec = history_spec();
    capabilities.builders.register_history(
        AdapterDescriptor {
            kind: spec.kind,
            schema: spec.schema,
            format: spec.format,
        },
        NativeHistoryBuildV1::new(ResourceNeed {
            provider_units: 0,
            ..resources.execution.core
        })?,
    )?;
    let service = NativeIndexService::bind_with_retention(
        root,
        socket,
        work_limits(),
        capabilities,
        &resources,
        Some(test_reclaim_config()),
        Some(NativeRetentionConfig {
            scopes: vec![NativeRetentionScope {
                workspace: workspace(),
                epoch: 1,
            }],
            sequence_window: 1,
            page_limit: 8,
            custody_page_limit: 8,
            interval: Duration::from_millis(20),
            round_timeout: Duration::from_secs(2),
            work_limits: work_limits(),
            head_probe_every: 2,
        }),
    )?;
    service.configure_read_sessions(NativeReadSessionConfig::default())?;
    Ok(service)
}

fn commit(id: u8, time: u64, message: &str, author: &str) -> CommitRecord {
    CommitRecord {
        wire_version: 1,
        sha: CommitSha::from_bytes([id; 20]),
        parents: Vec::new(),
        author_time_ms: time,
        committer_time_ms: time,
        applied_at_ms: 99,
        author: author.into(),
        author_name: Some(author.into()),
        author_email: None,
        committer: "publisher".into(),
        committer_name: None,
        committer_email: None,
        message: message.into(),
        is_merge: false,
        tags: vec!["at-publication".into()],
    }
}

fn diff(id: u8, path: &str, text: &str) -> HistoryDiffHunkUpsert {
    HistoryDiffHunkUpsert {
        commit_sha: CommitSha::from_bytes([id; 20]),
        file_path: path.into(),
        record: DiffHunkRecord {
            wire_version: 1,
            hunk_header: "@@ -5 +5 @@".into(),
            side: DiffHunkSide::After,
            added_text: text.into(),
            removed_text: "published deletion".into(),
            touched_text: "published context".into(),
            byte_start: 21,
            byte_end: 42,
        },
    }
}

fn batch(
    delta: bool,
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
) -> TestResult<HistoryIngestBatch> {
    let mut value = HistoryIngestBatch {
        repo_id,
        revision_id,
        generation,
        manifest_digest: None,
        batch_digest: String::new(),
        commits: if delta {
            vec![commit(4, 40_000, "needle newest", "alice")]
        } else {
            vec![
                commit(1, 10_000, "needle needle", "alice"),
                commit(2, 30_000, "needle other words added here", "bob"),
                commit(3, 20_000, "needle", "alice"),
                commit(9, 5_000, "noise", "other"),
            ]
        },
        refs: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
            name: "main".into(),
            sha: CommitSha::from_bytes([if delta { 4 } else { 2 }; 20]),
        })],
        tags: if delta {
            Vec::new()
        } else {
            vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: "v1".into(),
                sha: CommitSha::from_bytes([1; 20]),
            })]
        },
        diff_hunks: if delta {
            vec![diff(4, "archive/new.rs", "needle newest addition")]
        } else {
            vec![diff(1, "archive/deleted.rs", "needle historical addition")]
        },
    };
    stamp_batch_digest_v1(&mut value)?;
    Ok(value)
}

fn carrier(
    batch: &HistoryIngestBatch,
    ack: BatchPublishReceipt,
    ordinal: u64,
    before: Digest,
) -> TestResult<HistorySourceInputV1> {
    let after = history_replay_advance_with_progress_v1(before, ordinal, batch, &ack, &mut |_| {
        Ok::<_, std::convert::Infallible>(())
    })?;
    Ok(HistorySourceInputV1 {
        wire_version: 1,
        fence: HistoryReplayFenceV1 {
            generation: AuxiliaryGenerationKeyV1 {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
            },
            manifest_digest: batch.manifest_digest.clone(),
            publication_count: ordinal,
            first_replay: before,
            final_replay: after,
            final_durable_sequence: ack.durable_sequence,
        },
        publications: vec![HistoryReplayPublicationV1 {
            ordinal,
            before,
            after,
            batch: batch.clone(),
            ack,
        }],
    })
}

type LegacyHistoryInputs = (
    HistoryIngestBatch,
    HistoryIngestBatch,
    HistorySourceInputV1,
    HistorySourceInputV1,
);

fn published_legacy_source(
    legacy_root: &std::path::Path,
    dsl_fixed_ledger: bool,
) -> TestResult<LegacyHistoryInputs> {
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(legacy_root, 8)?;
    let publisher = process.connect()?;
    // This fixture has always published History independently of Source
    // activation. These are declared input identities, checked against the
    // daemon's actual acknowledgements and query response below.
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let generation = ManifestGeneration::new(1);
    let full = if dsl_fixed_ledger {
        let mut fixed = HISTORY_LEDGER.build_batch(repo.clone(), revision.clone(), generation)?;
        stamp_batch_digest_v1(&mut fixed)?;
        fixed
    } else {
        batch(false, repo.clone(), revision.clone(), generation)?
    };
    let delta = batch(true, repo.clone(), revision.clone(), generation)?;
    let full_ack = publisher.history().publish_history_borrowed_v1(&full)?;
    let delta_ack = publisher.history().publish_history_borrowed_v1(&delta)?;
    assert!(full_ack.applied && delta_ack.applied);
    assert_eq!(full_ack.generation, full.generation);
    assert_eq!(delta_ack.generation, delta.generation);
    assert_eq!(full_ack.batch_digest, full.batch_digest);
    assert_eq!(delta_ack.batch_digest, delta.batch_digest);
    assert!(full_ack.durable_sequence > 0);
    assert!(delta_ack.durable_sequence > full_ack.durable_sequence);
    let pin = GenerationPin::new(repo, revision, generation);
    let readback = publisher
        .history()
        .query()
        .native("type:commit")
        .pinned(pin.clone())
        .top_k(64)
        .order(HistoryOrderV1::Recency)
        .execute()?;
    assert_eq!(readback.generation, pin);
    let expected = if dsl_fixed_ledger {
        vec![
            CommitSha::from_bytes([4; 20]),
            CommitSha::from_hex(HISTORY_LEDGER.commit_sha)?,
        ]
    } else {
        [4_u8, 2, 3, 1, 9]
            .map(|id| CommitSha::from_bytes([id; 20]))
            .to_vec()
    };
    assert_eq!(
        readback
            .commits
            .iter()
            .map(|row| row.sha)
            .collect::<Vec<_>>(),
        expected
    );
    assert!(readback.next_cursor.is_none());
    process.stop()?;
    binary.verify()?;

    let scope = legacy_root
        .to_str()
        .ok_or("Legacy fixture path is not UTF-8")?;
    let first = history_replay_genesis_with_progress_v1(
        [
            scope,
            scope,
            "history-fixture",
            full.repo_id.as_str(),
            full.revision_id.as_str(),
        ],
        &full,
        &full_ack,
        &mut |_| Ok::<_, std::convert::Infallible>(()),
    )?;
    let full_input = carrier(&full, full_ack, 1, first)?;
    let delta_input = carrier(&delta, delta_ack, 2, full_input.fence.final_replay)?;
    Ok((full, delta, full_input, delta_input))
}

fn publish_in_legacy_child() -> TestResult<LegacyHistoryInputs> {
    publish_in_legacy_child_with_fixture(false)
}

fn publish_in_legacy_child_with_fixture(dsl_fixed_ledger: bool) -> TestResult<LegacyHistoryInputs> {
    // The external daemon owns its VFS registration. The current test binary
    // contains only the Native consumer and the public SDK producer call.
    let root = private_root()?;
    published_legacy_source(root.path(), dsl_fixed_ledger)
}

// Independent fixed-row commitment: never calls the target replay/digest helper
// or reads a Catalog to manufacture the producer's expected cut.
fn cut(
    full: &HistoryIngestBatch,
    delta: Option<&HistoryIngestBatch>,
    replay_start: Digest,
) -> TestResult<HistorySourceCutV1> {
    Ok(fixture_history_snapshot(full, delta, replay_start)?.0)
}

fn fixture_history_snapshot(
    full: &HistoryIngestBatch,
    delta: Option<&HistoryIngestBatch>,
    replay_start: Digest,
) -> TestResult<(HistorySourceCutV1, Vec<HistorySnapshotRowV1>)> {
    fn frame(hash: &mut Sha256, bytes: &[u8]) -> TestResult {
        hash.update(u64::try_from(bytes.len())?.to_le_bytes());
        hash.update(bytes);
        Ok(())
    }
    fn row<T: serde::Serialize>(
        hash: &mut Sha256,
        rows: &mut Vec<HistorySnapshotRowV1>,
        family: &str,
        key: &[u8],
        value: &T,
    ) -> TestResult {
        frame(hash, family.as_bytes())?;
        frame(hash, key)?;
        let value = encode_cbor_payload(value)?;
        frame(hash, &value)?;
        rows.push(HistorySnapshotRowV1 {
            family: family.into(),
            row_key: key.to_vec(),
            value,
        });
        Ok(())
    }
    fn reference_rows<'a>(
        base: &'a [HistoryRefMutation],
        delta: Option<&'a [HistoryRefMutation]>,
    ) -> std::collections::BTreeMap<&'a str, CommitSha> {
        let mut output = std::collections::BTreeMap::new();
        for mutation in base.iter().chain(delta.into_iter().flatten()) {
            match mutation {
                HistoryRefMutation::Upsert(update) => {
                    let _replaced = output.insert(update.name.as_ref(), update.sha);
                }
                HistoryRefMutation::Delete(delete) => {
                    let _removed = output.remove(delete.name.as_ref());
                }
            }
        }
        output
    }
    struct Meta {
        commits_materialized: bool,
        refs_materialized: bool,
        tags_materialized: bool,
        diff_hunks_materialized: bool,
    }
    impl serde::Serialize for Meta {
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
    let epoch = if delta.is_some() { 2 } else { 1 };
    let mut hash = Sha256::new();
    let mut rows = Vec::new();
    hash.update(b"quanta-index:history-source-snapshot:v1\0");
    frame(&mut hash, full.repo_id.as_str().as_bytes())?;
    frame(&mut hash, full.revision_id.as_str().as_bytes())?;
    hash.update(full.generation.get().to_le_bytes());
    row(&mut hash, &mut rows, "epoch", &[], &AuxEpochV1::new(epoch))?;
    let mut records: Vec<_> = full
        .commits
        .iter()
        .chain(delta.into_iter().flat_map(|batch| &batch.commits))
        .collect();
    records.sort_by_key(|record| record.sha);
    for record in records {
        row(
            &mut hash,
            &mut rows,
            "commit",
            record.sha.as_bytes(),
            record,
        )?;
    }
    for hunk in full
        .diff_hunks
        .iter()
        .chain(delta.into_iter().flat_map(|batch| &batch.diff_hunks))
    {
        let mut key = hunk.commit_sha.as_bytes().to_vec();
        key.push(0x1f);
        key.extend_from_slice(hunk.file_path.as_bytes());
        row(&mut hash, &mut rows, "diff-hunk", &key, &hunk.record)?;
    }
    for (name, sha) in reference_rows(&full.refs, delta.map(|batch| batch.refs.as_slice())) {
        row(&mut hash, &mut rows, "ref", name.as_bytes(), &sha)?;
    }
    row(
        &mut hash,
        &mut rows,
        "state-meta",
        &[],
        &Meta {
            commits_materialized: true,
            refs_materialized: true,
            tags_materialized: true,
            diff_hunks_materialized: true,
        },
    )?;
    for (name, sha) in reference_rows(&full.tags, delta.map(|batch| batch.tags.as_slice())) {
        row(&mut hash, &mut rows, "tag", name.as_bytes(), &sha)?;
    }
    Ok((
        HistorySourceCutV1 {
            generation: AuxiliaryGenerationKeyV1 {
                repo_id: full.repo_id.clone(),
                revision_id: full.revision_id.clone(),
                generation: full.generation,
            },
            durable_epoch: AuxEpochV1::new(epoch),
            snapshot_digest: Digest(hash.finalize().into()),
            replay_start,
        },
        rows,
    ))
}

fn stage_history<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    expected_view: ViewRef,
    sequence: u64,
    carrier: &HistorySourceInputV1,
    expected: Option<HistorySourceCutV1>,
    next: HistorySourceCutV1,
) -> TestResult<UpdateRequest> {
    let ingress = client.begin_input(operation(sequence), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![native_text_spec(), history_spec()],
        },
        &budget(),
    )?;
    let input = client.stage_projection_output_with_children(
        ingress,
        &serde_json::to_vec(carrier)?,
        &[],
        &budget(),
    )?;
    Ok(UpdateRequest {
        operation: operation(sequence),
        expected_view,
        kind: UpdateKind::HistoryAdvance(HistoryTransition {
            instance: history_spec().instance,
            expected,
            next,
            input,
            profile,
        }),
    })
}

fn query(text: &str, order: HistoryOrderV1, top_k: u32) -> HistoryQueryRequest {
    HistoryQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: text.into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            cursor: None,
            top_k,
        },
        order,
        cursor: None,
    }
}

fn shas(page: &NativeHistoryPage) -> Vec<CommitSha> {
    page.commits.iter().map(|row| row.sha).collect()
}

fn assert_ids(page: &NativeHistoryPage, ids: &[u8]) {
    assert_eq!(
        shas(page),
        ids.iter()
            .map(|id| CommitSha::from_bytes([*id; 20]))
            .collect::<Vec<_>>()
    );
}

fn refuse_cursor<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    session: &quanta_index_contract::native_protocol::NativeReadSessionStatus,
    cursor: NativeHistoryCursor,
    query: HistoryQueryRequest,
) -> TestResult {
    let reply = client.request(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHistory {
            session: session.session,
            instance: history_spec().instance,
            query,
            after: Some(cursor),
        }),
        &budget(),
    );
    assert!(
        matches!(reply, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput)
    );
    Ok(())
}

#[test]
fn native_history_sdk_full_delta_retained_pages_and_reopen() -> TestResult {
    let (full, delta, full_input, delta_input) = publish_in_legacy_child()?;
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(history_service(root.path(), &socket)?);
    let primary = (|| -> TestResult<(ViewRef, ViewRef, UpdateRequest, UpdateRequest)> {
        let client = NativeUdsClient::new(&socket);
        let configured = create_text_workspace(&client)?;
        let (source_view, _) =
            publish_text_owner(&client, configured, 2, 1, "live.rs", "needle current file")?;
        let original_source = client
            .read_view(ViewSelection::Exact(source_view), &budget())?
            .source;
        let full_cut = cut(&full, None, full_input.fence.final_replay)?;
        let delta_cut = cut(&full, Some(&delta), delta_input.fence.final_replay)?;
        let full_request =
            stage_history(&client, source_view, 3, &full_input, None, full_cut.clone())?;
        let held = published(&client.submit(full_request.clone(), &budget())?)?;
        assert_eq!(
            published(&client.submit(full_request.clone(), &budget())?)?,
            held
        );
        let mut changed_full = full_request.clone();
        let UpdateKind::HistoryAdvance(transition) = &mut changed_full.kind else {
            return Err("expected History FULL transition".into());
        };
        assert_ne!(transition.input, transition.profile);
        transition.input = transition.profile;
        assert!(matches!(
            client.submit(changed_full.clone(), &budget()),
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::DigestConflict
        ));
        assert_eq!(
            client
                .read_view(ViewSelection::Exact(held), &budget())?
                .source,
            original_source
        );
        let sdk = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &socket,
            ViewSelection::Exact(held),
        ))?;
        let reader = sdk.reader().native()?;
        let mut session = reader.open([83; 32], &budget())?;
        let captured = session.load_profile(&budget())?;
        assert_eq!(captured.view.identity, held);
        assert_eq!(
            captured.profile.instances,
            vec![native_text_spec(), history_spec()]
        );
        let recency = query("type:commit needle", HistoryOrderV1::Recency, 2);
        let first = session.history(&history_spec(), recency.clone(), None, &budget())?;
        assert_eq!(
            (first.view, first.matched, first.remaining, first.read_epoch),
            (held, 3, 3, AuxEpochV1::new(1))
        );
        assert_ids(&first, &[2, 3]);
        assert!(first.commits.iter().all(|row| row.score.is_none()));
        assert_eq!(first.commits[0].message, "needle other words added here");
        assert_eq!(first.commits[0].committed_at_unix_s, 30);
        let anchor = first
            .after
            .clone()
            .ok_or("History first page lost continuation")?;
        let relevance = session.history(
            &history_spec(),
            query("type:commit needle", HistoryOrderV1::Relevance, 3),
            None,
            &budget(),
        )?;
        // All four commits define BM25 N/lengths, independently of keyword/filter hits.
        assert_ids(&relevance, &[1, 3, 2]);
        let idf = (1.0_f32 + 1.5 / 3.5).ln();
        for (row, dl, tf) in relevance
            .commits
            .iter()
            .zip([2.0_f32, 1.0, 5.0])
            .zip([2.0_f32, 1.0, 1.0])
            .map(|((row, dl), tf)| (row, dl, tf))
        {
            let gold = idf * 2.2 * tf / (tf + 1.2 * (0.25 + 0.75 * dl / 2.25));
            assert!((row.score.ok_or("relevance score missing")?.get() - gold).abs() < 0.00001);
        }
        let filtered = session.history(
            &history_spec(),
            query(
                "type:commit needle author:alice",
                HistoryOrderV1::Recency,
                4,
            ),
            None,
            &budget(),
        )?;
        assert_ids(&filtered, &[3, 1]);
        assert_eq!(filtered.matched, 2);
        for order in [HistoryOrderV1::Recency, HistoryOrderV1::Relevance] {
            let page = session.history(
                &history_spec(),
                query("type:diff needle file:archive/deleted.rs", order, 2),
                None,
                &budget(),
            )?;
            assert_eq!((page.matched, page.remaining, page.diffs.len()), (1, 1, 1));
            assert!(page.commits.is_empty());
            let row = &page.diffs[0];
            assert_eq!(row.repo_relative_path, "archive/deleted.rs");
            assert_eq!(row.hunk_header, "@@ -5 +5 @@");
            assert_eq!(row.side, DiffHunkSide::After);
            assert_eq!(
                row.snippet, "published context",
                "History diff snippet mismatch: order={order:?}; row={row:?}"
            );
            assert_eq!(row.score.is_some(), order == HistoryOrderV1::Relevance);
        }
        // Ordinary, unprefixed CLI crosses the real socket and shares this exact session.
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = quanta_index_searchctl::run(
            [
                "history".into(),
                "--socket".into(),
                socket.to_str().ok_or("socket is not UTF8")?.into(),
                "--session-json".into(),
                serde_json::to_string(session.status())?,
                "--instance".into(),
                "51".repeat(16),
                "--query-text".into(),
                "type:diff needle file:archive/deleted.rs".into(),
                "--top-k".into(),
                "2".into(),
                "--order".into(),
                "recency".into(),
                "--output".into(),
                "json".into(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(
            code,
            std::process::ExitCode::SUCCESS,
            "{}",
            String::from_utf8_lossy(&stderr)
        );
        let cli: NativeHistoryPage = serde_json::from_slice(&stdout)?;
        assert_eq!(
            (cli.view, cli.session, cli.matched),
            (held, session.status().session, 1)
        );
        assert_eq!(cli.diffs[0].repo_relative_path, "archive/deleted.rs");
        let request = stage_history(
            &client,
            held,
            4,
            &delta_input,
            Some(full_cut.clone()),
            delta_cut.clone(),
        )?;
        let latest = published(&client.submit(request, &budget())?)?;
        let second = session.history(
            &history_spec(),
            recency.clone(),
            Some(anchor.clone()),
            &budget(),
        )?;
        assert_eq!(
            (
                second.view,
                second.member,
                second.matched,
                second.remaining,
                second.read_epoch
            ),
            (held, first.member, 3, 1, AuxEpochV1::new(1))
        );
        assert_ids(&second, &[1]);
        assert_eq!(second.after, None);
        let new_sdk = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &socket,
            ViewSelection::Exact(latest),
        ))?;
        let mut fresh = new_sdk.reader().native()?.open([84; 32], &budget())?;
        assert_eq!(fresh.load_profile(&budget())?.view.identity, latest);
        let page = fresh.history(
            &history_spec(),
            query("type:commit needle", HistoryOrderV1::Recency, 4),
            None,
            &budget(),
        )?;
        assert_ids(&page, &[4, 2, 3, 1]);
        assert_eq!((page.matched, page.read_epoch), (4, AuxEpochV1::new(2)));
        assert_ne!(page.member, first.member);
        let pins = view_pin_count(root.path(), held.sequence)?;
        for field in 0..5 {
            let mut forged = anchor.clone();
            match field {
                0 => forged.view = latest,
                1 => forged.instance = native_text_spec().instance,
                2 => forged.member = page.member,
                3 => forged.query = Digest([99; 32]),
                _ => forged.boundary.aux_epoch = AuxEpochV1::new(2),
            }
            refuse_cursor(&client, session.status(), forged, recency.clone())?;
            assert_eq!(view_pin_count(root.path(), held.sequence)?, pins);
        }
        for changed in [
            query("type:commit noise", HistoryOrderV1::Recency, 2),
            query("type:commit needle", HistoryOrderV1::Relevance, 2),
            query("type:commit needle", HistoryOrderV1::Recency, 1),
        ] {
            refuse_cursor(&client, session.status(), anchor.clone(), changed)?;
            assert_eq!(view_pin_count(root.path(), held.sequence)?, pins);
        }
        // A stale producer cut under a current view must fail without a head move.
        let stale = stage_history(&client, latest, 5, &delta_input, Some(full_cut), delta_cut)?;
        let refused = client.submit(stale, &budget())?;
        assert!(matches!(
            refused,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict))
        ));
        assert_eq!(
            client
                .read_view(
                    ViewSelection::Latest {
                        workspace: workspace(),
                        epoch: 1
                    },
                    &budget()
                )?
                .identity,
            latest
        );
        let _remaining = session.release(&budget())?;
        assert_eq!(session.status().state, NativeReadSessionState::Released);
        let _remaining = fresh.release(&budget())?;
        assert_eq!(fresh.status().state, NativeReadSessionState::Released);
        let released = client.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHistory {
                session: session.status().session,
                instance: history_spec().instance,
                query: recency,
                after: None,
            }),
            &budget(),
        );
        assert!(
            matches!(released, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::SnapshotUnavailable)
        );
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or("History FULL retention deadline overflow")?;
        let db = rusqlite::Connection::open_with_flags(
            quanta_index_catalog::catalog_dir(root.path())
                .join(quanta_index_catalog::CATALOG_FILE_NAME),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let workspace = serde_json::to_vec(&held.workspace)?;
        let epoch = i64::try_from(held.epoch)?;
        let sequence = i64::try_from(held.sequence)?;
        loop {
            if Instant::now() >= deadline {
                return Err(
                    "History FULL Exact view and Catalog row did not retire after session release"
                        .into(),
                );
            }
            match client.read_view(
                ViewSelection::Exact(held),
                &RequestBudgetV1::until(deadline),
            ) {
                Err(NativeUdsError::Refused(error))
                    if error.code == NativeIndexErrorCode::SnapshotUnavailable =>
                {
                    // The floor can refuse Exact before the separate retirement
                    // transaction deletes the durable view row.
                    let stored: i64 = db.query_row(
                        "SELECT count(*) FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
                        rusqlite::params![&workspace, epoch, sequence],
                        |row| row.get(0),
                    )?;
                    match stored {
                        0 => break,
                        1 => {}
                        _ => {
                            return Err(format!(
                                "History FULL Catalog view count changed: {stored}"
                            )
                            .into());
                        }
                    }
                }
                Ok(view) if view.identity == held => {}
                Ok(view) => {
                    return Err(format!(
                        "History FULL Exact view changed before retirement: {view:?}"
                    )
                    .into());
                }
                Err(error) => return Err(error.into()),
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(db);
        Ok((latest, held, full_request, changed_full))
    })();
    let stopped = running.stop();
    let (latest, retired_full, full_request, changed_full) = primary?;
    stopped?;
    let reopened = NativeServiceGuard::start(history_service(root.path(), &socket)?);
    let result = (|| -> TestResult {
        let client = NativeUdsClient::new(&socket);
        assert!(matches!(
            client.read_view(ViewSelection::Exact(retired_full), &budget()),
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable
        ));
        assert_eq!(
            published(&client.submit(full_request, &budget())?)?,
            retired_full
        );
        assert!(matches!(
            client.submit(changed_full, &budget()),
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::DigestConflict
        ));
        let sdk = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &socket,
            ViewSelection::Exact(latest),
        ))?;
        let mut session = sdk.reader().native()?.open([85; 32], &budget())?;
        assert_eq!(session.load_profile(&budget())?.view.identity, latest);
        let page = session.history(
            &history_spec(),
            query("type:commit needle", HistoryOrderV1::Recency, 4),
            None,
            &budget(),
        )?;
        assert_eq!(
            (page.view, page.matched, page.read_epoch),
            (latest, 4, AuxEpochV1::new(2))
        );
        assert_ids(&page, &[4, 2, 3, 1]);
        let diff = session.history(
            &history_spec(),
            query("type:diff needle", HistoryOrderV1::Recency, 4),
            None,
            &budget(),
        )?;
        assert_eq!(
            diff.diffs
                .iter()
                .map(|row| row.repo_relative_path.as_str())
                .collect::<Vec<_>>(),
            ["archive/new.rs", "archive/deleted.rs"]
        );
        let _remaining = session.release(&budget())?;
        assert_eq!(session.status().state, NativeReadSessionState::Released);
        Ok(())
    })();
    let stopped = reopened.stop();
    result?;
    stopped?;
    Ok(())
}

#[test]
fn native_history_append_original_publishes_without_legacy_ack_and_reopens() -> TestResult {
    let (full, _, full_input, _) = publish_in_legacy_child()?;
    let (mut full_cut, rows) = fixture_history_snapshot(&full, None, Digest([0; 32]))?;
    let ack = &full_input.publications[0].ack;
    // The storage test stages rows with a real Legacy ACK. Installed scope
    // authentication is performed independently by the Catalog boot owner.
    full_cut.replay_start = quanta_index_ipc::history_snapshot_checkpoint_replay_with_progress_v1(
        [
            "/history-test/writer",
            "/history-test/workspace",
            "test",
            "/history-test/legacy",
        ],
        &full_cut.generation,
        full_cut.durable_epoch,
        full_cut.snapshot_digest,
        ack,
        &mut |_| Ok::<_, std::convert::Infallible>(()),
    )?;
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(history_service(root.path(), &socket)?);
    let result = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(&socket);
        let configured = create_text_workspace(&client)?;
        let full_operation = operation(2);
        let ingress = client.begin_input(full_operation, &budget())?;
        let profile = client.stage_profile(
            ingress,
            IndexProfile {
                instances: vec![native_text_spec(), history_spec()],
            },
            &budget(),
        )?;
        let mut pages = Vec::new();
        for (index, chunk) in rows.chunks(4).enumerate() {
            let page = HistorySnapshotPageV1 {
                wire_version: 1,
                generation: full_cut.generation.clone(),
                ordinal: u32::try_from(index + 1)?,
                rows: chunk.to_vec(),
            };
            pages.push(client.stage_projection_output_with_children(
                ingress,
                &serde_json::to_vec(&page)?,
                &[],
                &budget(),
            )?);
        }
        let mut children = pages.clone();
        children.sort_unstable();
        let source = HistorySnapshotCheckpointV1 {
            wire_version: 2,
            cut: full_cut.clone(),
            terminal_ack: ack.clone(),
            row_count: u64::try_from(rows.len())?,
            pages,
        };
        let input = client.stage_projection_output_with_children(
            ingress,
            &serde_json::to_vec(&source)?,
            &children,
            &budget(),
        )?;
        let full_update = UpdateRequest {
            operation: full_operation,
            expected_view: configured,
            kind: UpdateKind::HistoryAdvance(HistoryTransition {
                instance: history_spec().instance,
                expected: None,
                next: full_cut.clone(),
                input,
                profile,
            }),
        };
        let before = published(&client.submit(full_update, &budget())?)?;
        let baseline = client.read_view(ViewSelection::Exact(before), &budget())?;

        // The new original is produced after the Legacy fixture exited. Only
        // the active Native operation can commit it and issue a publication.
        let mut original = HistoryIngestBatch {
            repo_id: full.repo_id.clone(),
            revision_id: full.revision_id.clone(),
            generation: full.generation,
            manifest_digest: full.manifest_digest.clone(),
            batch_digest: String::new(),
            commits: vec![commit(8, 50_000, "needle native-only successor", "alice")],
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: Vec::new(),
        };
        stamp_batch_digest_v1(&mut original)?;
        let bytes = serde_json::to_vec(&original)?;
        let append = |sequence, expected: HistorySourceCutV1| -> TestResult<UpdateRequest> {
            let operation = operation(sequence);
            let ingress = client.begin_input(operation, &budget())?;
            let input =
                client.stage_projection_output_with_children(ingress, &bytes, &[], &budget())?;
            Ok(UpdateRequest {
                operation,
                expected_view: before,
                kind: UpdateKind::HistoryAppend(HistoryAppendV1 {
                    instance: history_spec().instance,
                    expected,
                    input,
                    profile: baseline.profile,
                }),
            })
        };
        let mut forged = full_cut.clone();
        forged.replay_start = Digest([99; 32]);
        assert!(matches!(
            client.submit(append(3, forged)?, &budget())?,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict))
        ));
        let request = append(4, full_cut.clone())?;
        let after = published(&client.submit(request.clone(), &budget())?)?;
        assert_eq!(after.sequence, before.sequence + 1);
        let published_view = client.read_view(ViewSelection::Exact(after), &budget())?;
        assert_eq!(published_view.source, baseline.source);
        assert_eq!(published_view.profile, baseline.profile);
        Ok((request, after, original))
    })();
    let stopped = running.stop();
    let (request, after, mut original) = result?;
    stopped?;

    let reopened = NativeServiceGuard::start(history_service(root.path(), &socket)?);
    let result = (|| -> TestResult {
        let client = NativeUdsClient::new(&socket);
        assert_eq!(published(&client.submit(request, &budget())?)?, after);
        let sdk = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &socket,
            ViewSelection::Exact(after),
        ))?;
        let mut session = sdk.reader().native()?.open([89; 32], &budget())?;
        let profile = session.load_profile(&budget())?.view.profile;
        let witness = session.history_cut(&history_spec(), &budget())?;
        assert_eq!(witness.view, after);
        assert_eq!(witness.cut.generation, full_cut.generation);
        assert_eq!(witness.cut.durable_epoch, AuxEpochV1::new(2));
        assert_ne!(witness.cut.replay_start, full_cut.replay_start);
        let page = session.history(
            &history_spec(),
            query("type:commit needle", HistoryOrderV1::Recency, 4),
            None,
            &budget(),
        )?;
        assert_eq!((page.view, page.read_epoch), (after, AuxEpochV1::new(2)));
        assert_eq!(page.member, witness.member);
        assert_ids(&page, &[8, 2, 3, 1]);
        let _released = session.release(&budget())?;

        original.commits = vec![commit(7, 60_000, "needle second native successor", "alice")];
        stamp_batch_digest_v1(&mut original)?;
        let operation = operation(5);
        let ingress = client.begin_input(operation, &budget())?;
        let input = client.stage_projection_output_with_children(
            ingress,
            &serde_json::to_vec(&original)?,
            &[],
            &budget(),
        )?;
        let next = published(&client.submit(
            UpdateRequest {
                operation,
                expected_view: after,
                kind: UpdateKind::HistoryAppend(HistoryAppendV1 {
                    instance: history_spec().instance,
                    expected: witness.cut,
                    input,
                    profile,
                }),
            },
            &budget(),
        )?)?;
        assert_eq!(next.sequence, after.sequence + 1);
        Ok(())
    })();
    let stopped = reopened.stop();
    result?;
    stopped?;
    Ok(())
}

#[test]
fn source_and_next_history_generation_publish_in_one_view_and_reopen() -> TestResult {
    source_and_next_history_generation_round_trip(false)?;
    source_and_next_history_generation_round_trip(true)
}

fn source_and_next_history_generation_round_trip(resync: bool) -> TestResult {
    let (full, _, full_input, _) = publish_in_legacy_child()?;
    let full_cut = cut(&full, None, full_input.fence.final_replay)?;
    let next_revision = RevisionId::new("next-source-revision")?;
    assert_ne!(next_revision, full.revision_id);
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(history_service(root.path(), &socket)?);
    let target = InstanceSpec {
        instance: IndexInstanceId([88; 16]),
        ..history_spec()
    };
    let setup = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(&socket);
        let configured = create_text_workspace(&client)?;
        let (source_view, old_source_ref) =
            publish_text_owner(&client, configured, 2, 1, "old.rs", "needle old source")?;
        let live = published(&client.submit(
            stage_history(&client, source_view, 3, &full_input, None, full_cut.clone())?,
            &budget(),
        )?)?;
        let old = client.read_view(ViewSelection::Exact(live), &budget())?;
        let next = ManifestGeneration::new(
            full.generation
                .get()
                .checked_add(1)
                .ok_or("generation overflow")?,
        );
        let prepare = |sequence,
                       generation,
                       forged_recency|
         -> TestResult<(UpdateRequest, HistoryGenerationInputV1, ObjectRef)> {
            let mut key = operation(sequence);
            if resync {
                key.stream_epoch = 2;
            }
            let ingress = client.begin_input(key, &budget())?;
            let profile = client.stage_profile(
                ingress,
                IndexProfile {
                    instances: vec![native_text_spec(), history_spec(), target.clone()],
                },
                &budget(),
            )?;
            let mut child = commit(8, 50_000, "needle next-generation History", "alice");
            child.parents.push(CommitSha::from_bytes([2; 20]));
            let mut original = HistoryIngestBatch {
                repo_id: full.repo_id.clone(),
                revision_id: next_revision.clone(),
                generation,
                manifest_digest: Some("next-manifest".into()),
                batch_digest: String::new(),
                commits: vec![child],
                refs: Vec::new(),
                tags: Vec::new(),
                diff_hunks: vec![diff(8, "src/next.rs", "needle changed source")],
            };
            stamp_batch_digest_v1(&mut original)?;
            let original = client.stage_projection_output_with_children(
                ingress,
                &serde_json::to_vec(&original)?,
                &[],
                &budget(),
            )?;
            let mut recency = RepoCommitRecencyIngestBatch {
                repo_id: full.repo_id.clone(),
                revision_id: next_revision.clone(),
                generation,
                batch_digest: String::new(),
                entries: vec![RepoCommitRecencyEntry {
                    source_repo_id: full.repo_id.clone(),
                    latest_committer_time_ms: if forged_recency { 49_999 } else { 50_000 },
                }],
            };
            stamp_batch_digest_v1(&mut recency)?;
            let recency = client.stage_projection_output_with_children(
                ingress,
                &serde_json::to_vec(&recency)?,
                &[],
                &budget(),
            )?;
            let mut contributor = FileContributorIngestBatch {
                repo_id: full.repo_id.clone(),
                revision_id: next_revision.clone(),
                generation,
                batch_digest: String::new(),
                entries: vec![FileContributorEntry {
                    source_repo_id: full.repo_id.clone(),
                    repo_relative_path: RepoRelativePath::new("src/next.rs".to_owned()),
                    contributors: vec![FileContributorIdentityEntry {
                        canonical: "alice".into(),
                        name: Some("alice".into()),
                        email: None,
                    }],
                }],
            };
            stamp_batch_digest_v1(&mut contributor)?;
            let contributor = client.stage_projection_output_with_children(
                ingress,
                &serde_json::to_vec(&contributor)?,
                &[],
                &budget(),
            )?;
            let closure = HistoryGenerationInputV1 {
                original,
                repo_commit_recency: Some(recency),
                file_contributor: Some(contributor),
            };
            let children = closure.child_references_v1().collect::<Vec<_>>();
            let input = client.stage_projection_output_with_children(
                ingress,
                &serde_json::to_vec(&closure)?,
                &children,
                &budget(),
            )?;
            let (request, next_source) = if resync {
                let next_source = stage_text_source_in_ingress(
                    &client, ingress, "new.rs", "needle next source", None,
                )?;
                let producer = ingress.operation.producer;
                let request = prepare_text_mutations_in_ingress(&client, live, ingress, [
                    OwnerMutation {
                        owner: OwnerKey { producer, stable_id: StableOwnerId([1; 16]) },
                        expected: Some(old_source_ref),
                        next: None,
                    },
                    OwnerMutation {
                        owner: OwnerKey { producer, stable_id: StableOwnerId([2; 16]) },
                        expected: None,
                        next: Some(next_source),
                    },
                ])?;
                (request, next_source)
            } else {
                prepare_text_owner_in_ingress(
                    &client, live, ingress, 2, "new.rs", "needle next source",
                )?
            };
            let UpdateKind::SourceAdvance(mut source) = request.kind else {
                return Err("expected the ordinary Source fixture request".into());
            };
            // The Source stream is still at event 1. History FULL and refused
            // compound operations do not consume a Source frontier position.
            let [event] = source.events.as_mut_slice() else {
                return Err("expected one Source fixture event".into());
            };
            (event.predecessor, event.sequence) = if resync { (0, 1) } else { (1, 2) };
            let transition = Box::new(SourceHistoryTransitionV1 {
                source,
                history: HistoryGenerationAdvanceV1 {
                    prior_instance: history_spec().instance,
                    expected: full_cut.clone(),
                    instance: target.instance,
                    input,
                    profile,
                },
            });
            let request = UpdateRequest {
                kind: if resync {
                    UpdateKind::SourceHistoryResync { prior_epoch: 1, transition }
                } else {
                    UpdateKind::SourceHistoryAdvance(transition)
                },
                expected_view: live,
                operation: request.operation,
            };
            Ok((request, closure, next_source))
        };
        assert!(matches!(
            client.submit(prepare(4, full.generation, false)?.0, &budget())?,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::InvalidInput))
        ));
        assert!(matches!(
            client.submit(prepare(5, next, true)?.0, &budget())?,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::InvalidInput))
        ));
        let (mut wrong_prior, _, _) = prepare(6, next, false)?;
        let transition = match &mut wrong_prior.kind {
            UpdateKind::SourceHistoryAdvance(transition)
            | UpdateKind::SourceHistoryResync { transition, .. } => transition,
            _ => return Err("expected a combined publication to test the prior cut".into()),
        };
        transition.history.expected.generation.revision_id = next_revision.clone();
        assert!(matches!(
            client.submit(wrong_prior, &budget())?,
            SubmitOutcome::Terminal(OperationStatus::Refused(_))
        ));
        if resync {
            let (mut stale, _, _) = prepare(7, next, false)?;
            let UpdateKind::SourceHistoryResync { prior_epoch, .. } = &mut stale.kind else {
                return Err("expected a resync to test the prior Source epoch".into());
            };
            *prior_epoch = 0;
            assert!(matches!(
                client.submit(stale, &budget()),
                Err(quanta_index_sdk::native_uds::NativeUdsError::Refused(error))
                    if error.code == NativeIndexErrorCode::Conflict
            ));
        }
        let latest = ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        };
        assert_eq!(client.read_view(latest, &budget())?, old);
        if resync {
            let page = client.list_producer_source_owners(
                ViewSelection::Exact(live), operation(2).producer, None, 2, &budget(),
            )?;
            assert_eq!(page.source_epoch, Some(1));
            let [owner] = page.rows.as_slice() else {
                return Err("refused History must retain exactly the old Source owner".into());
            };
            assert_eq!(owner.owner.stable_id, StableOwnerId([1; 16]));
            assert_eq!(owner.source, old_source_ref);
        }
        let (request, expected_inputs, next_source) =
            prepare(if resync { 8 } else { 7 }, next, false)?;
        let view = published(&client.submit(request.clone(), &budget())?)?;
        assert_eq!(view.sequence, live.sequence + 1);
        let published_view = client.read_view(latest, &budget())?;
        assert_eq!(published_view.identity, view);
        assert_ne!(published_view.source, old.source);
        Ok((request, published_view, next, expected_inputs, next_source))
    })();
    let stopped = running.stop();
    let (request, published_view, next, expected_inputs, next_source) = setup?;
    stopped?;

    let reopened = NativeServiceGuard::start(history_service(root.path(), &socket)?);
    let result = (|| -> TestResult {
        let client = NativeUdsClient::new(&socket);
        let view = published_view.identity;
        let latest = ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        };
        // Inspect the recovered Catalog before the original request can repair it.
        assert!(matches!(
            client.status(request.operation, &budget())?,
            OperationStatus::Published(observed) if observed == view
        ));
        assert_eq!(client.read_view(latest, &budget())?, published_view);
        let expected_source_input = request.kind.history_generation_advance_v1()
            .ok_or("expected the combined publication request")?.input;
        if resync {
            let page = client.list_producer_source_owners(
                ViewSelection::Exact(view), request.operation.producer, None, 2, &budget(),
            )?;
            assert_eq!(page.source_epoch, Some(2));
            assert_eq!(page.source, published_view.source);
            let [owner] = page.rows.as_slice() else {
                return Err("recovered SourceResync must retain exactly the replacement owner".into());
            };
            assert_eq!(owner.owner.stable_id, StableOwnerId([2; 16]));
            assert_eq!(owner.source, next_source);
        }
        let sdk = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &socket,
            ViewSelection::Exact(view),
        ))?;
        let mut session = sdk.reader().native()?.open([90; 32], &budget())?;
        let _profile = session.load_profile(&budget())?;
        let current = session.history_cut(&target, &budget())?;
        let NativeIndexResponse::HistoryGeneration(output) = client.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadHistoryGeneration {
                session: session.status().session,
                instance: target.instance,
            }),
            &budget(),
        )?
        else {
            return Err("missing published History generation outputs".into());
        };
        assert_eq!((output.view, output.member), (view, current.member));
        assert_eq!(output.source_input, expected_source_input);
        assert_eq!(output.inputs, expected_inputs);
        assert_eq!(output.predecessor_context, full_cut.input_context_v1()?);
        assert_eq!(current.cut.generation.generation, next);
        assert_eq!(current.cut.generation.revision_id, next_revision);
        assert_eq!(current.cut.durable_epoch, AuxEpochV1::new(1));
        let page = session.history(
            &target,
            query("type:commit needle", HistoryOrderV1::Recency, 4),
            None,
            &budget(),
        )?;
        assert_ids(&page, &[8, 2, 3, 1]);
        assert_eq!(
            session.history_cut(&history_spec(), &budget())?.cut,
            full_cut
        );
        let _released = session.release(&budget())?;
        assert_eq!(published(&client.submit(request, &budget())?)?, view);
        assert_eq!(client.read_view(latest, &budget())?, published_view);
        Ok(())
    })();
    let stopped = reopened.stop();
    result?;
    stopped?;
    Ok(())
}
