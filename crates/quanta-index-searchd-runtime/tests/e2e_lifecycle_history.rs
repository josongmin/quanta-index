//! Real SDK/UDS lifecycle observations judged by an independent history model.
//! Caller gates model scheduling, not server admission or physical storage cuts.

#![forbid(unsafe_code)]

#[path = "lifecycle_history/model.rs"]
mod model;

use std::error::Error;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use quanta_index_contract::{
    OwnerDocKind, SearchCorpusActiveHeadV1, SearchPlaneErrorCodeV2,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SemanticCorpusKindV1,
    SemanticSourceScopeKeyV1,
};
use quanta_index_sdk::{
    ChunkId, ChunkRecord, GenerationPin, LanguageCode, ManifestGeneration,
    PublishedBatchFailureStage, QuantaIndex, RepoId, RepoRelativePath, RevisionId, SdkError,
    SearchCorpusBatch, SearchPlaneTrackKind, SourceFileKey, SourcePublicationEvent,
};
use quanta_index_searchd_harness::{
    fixture_source_scope_v1, private_tempdir, semantic_source_scopes_for_chunk_records,
};

use crate::searchd_binary_process::SearchdBinaryProcess;
use model::{Cas, Event, Head, Observation, Operation, Publication, Row};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const REPO: &str = "repo-lifecycle-history";
const REVISION: &str = "rev-lifecycle-history";
const TEXT: &str = "quartz";
type Gate<'a> = Option<(&'a SyncSender<()>, &'a Receiver<()>)>;

fn wait_at_gate(gate: Gate<'_>) -> TestResult {
    if let Some((ready, release)) = gate {
        ready.send(())?;
        release.recv_timeout(Duration::from_secs(30))?;
    }
    Ok(())
}

#[derive(Clone, Default)]
struct History(Arc<Mutex<Vec<Event>>>);

impl History {
    fn record<T>(
        &self,
        operation: Operation,
        call: impl FnOnce() -> TestResult<(Observation, T)>,
    ) -> TestResult<T> {
        let label = format!("{operation:?}");
        let id = {
            let mut events = self.0.lock().map_err(|e| e.to_string())?;
            let id = events.len();
            events.push(Event::Invoke { id, operation });
            id
        };
        let outcome = call();
        let observation = match &outcome {
            Ok((observation, _)) => observation.clone(),
            Err(error) => Observation::Failure(error.to_string()),
        };
        self.0
            .lock()
            .map_err(|e| e.to_string())?
            .push(Event::Respond { id, observation });
        outcome
            .map(|(_, value)| value)
            .map_err(|error| format!("history operation {id} {label}: {error}").into())
    }

    fn check(&self) -> TestResult {
        let events = self.0.lock().map_err(|e| e.to_string())?;
        model::check(&events, 100_000).map_err(Into::into)
    }
}

fn row(id: &str) -> Row {
    Row {
        id: id.into(),
        path: format!("src/{id}.rs"),
    }
}

fn publication(generation: u64, base: Option<u64>, clear: bool, ids: &[&str]) -> Publication {
    Publication {
        generation,
        base,
        clear,
        rows: ids.iter().map(|id| row(id)).collect(),
    }
}

fn batch(input: &Publication, stream: &str) -> TestResult<SearchCorpusBatch> {
    let repo = RepoId::new(REPO)?;
    let revision = RevisionId::new(REVISION)?;
    let generation = ManifestGeneration::new(input.generation);
    let digest = format!("manifest:history:{}", input.generation);
    let mut batch = match input.base {
        Some(base) => SearchCorpusBatch::delta(
            repo.clone(),
            revision.clone(),
            generation,
            ManifestGeneration::new(base),
            digest,
        ),
        None => SearchCorpusBatch::replace_generation(
            repo.clone(),
            revision.clone(),
            generation,
            digest,
        ),
    }
    .source_event(SourcePublicationEvent {
        // A delta must inherit the same producer stream's accepted parent event.
        stream_id: format!("fixture:lifecycle-history:stream:{stream}"),
        event_id: format!("fixture:lifecycle-history:{}", input.generation),
        expected_base_event_id: input
            .base
            .map(|base| format!("fixture:lifecycle-history:{base}")),
        payload_sha256: [0; 32],
    });
    if input.clear {
        // Empty this fixed corpus through coverage-coherent file and semantic
        // deletion. Independent Chunk/Symbol clear is forbidden on this API.
        for id in ["a", "b", "c"] {
            batch = batch
                .tombstone_scope(SourceFileKey {
                    source_repo_id: repo.clone(),
                    repo_relative_path: RepoRelativePath::new(row(id).path),
                })
                .tombstone_semantic_scope(SemanticSourceScopeKeyV1 {
                    corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                    owner_kind: OwnerDocKind::Chunk,
                    owner_id: id.into(),
                });
        }
    }
    for row in &input.rows {
        let scope = fixture_source_scope_v1(
            SourceFileKey {
                source_repo_id: repo.clone(),
                repo_relative_path: RepoRelativePath::new(row.path.clone()),
            },
            revision.clone(),
            vec![ChunkRecord {
                chunk_id: ChunkId::new(row.id.clone()),
                repo_relative_path: RepoRelativePath::new(row.path.clone()),
                language: LanguageCode::new("rust")?,
                start_byte: 0,
                end_byte: u32::try_from(TEXT.len())?,
                start_line: 1,
                end_line: 1,
                text: TEXT.into(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            }],
            Vec::new(),
        )?;
        for semantic in semantic_source_scopes_for_chunk_records(&scope.chunks) {
            batch = batch.replace_semantic_scope(
                semantic.scope,
                semantic.scope_digest,
                semantic.sources,
                semantic.cluster_memberships,
            );
        }
        batch = batch.replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );
    }
    Ok(batch)
}

fn head(wire: &SearchCorpusActiveHeadV1) -> TestResult<Head> {
    let generation = wire.generation.lexical.manifest_generation.get();
    if wire.generation.lexical.track != SearchPlaneTrackKind::Lexical
        || wire.generation.semantic.track != SearchPlaneTrackKind::Semantic
    {
        return Err(format!("wrong composite tracks: {wire:?}").into());
    }
    for snapshot in [&wire.generation.lexical, &wire.generation.semantic] {
        if snapshot.repo_id.as_str() != REPO
            || snapshot.revision_id.as_str() != REVISION
            || snapshot.manifest_generation.get() != generation
            || snapshot.manifest_digest != format!("manifest:history:{generation}")
        {
            return Err(format!("foreign composite identity: {wire:?}").into());
        }
    }
    Ok(Head {
        generation,
        incarnation: wire.activation_token.root_incarnation(),
        sequence: wire.activation_token.activation_sequence().get(),
    })
}

fn read_head(history: &History, client: &QuantaIndex) -> TestResult<SearchCorpusActiveHeadV1> {
    history.record(Operation::ReadHead, || {
        let wire = client
            .generations()
            .active_head(RepoId::new(REPO)?, RevisionId::new(REVISION)?)?
            .ok_or("missing active head")?;
        Ok((Observation::Head(Some(head(&wire)?)), wire))
    })
}

fn activate(
    history: &History,
    client: &QuantaIndex,
    generation: u64,
    batch: &SearchCorpusBatch,
    expected: Option<SearchCorpusActiveHeadV1>,
) -> TestResult<Option<SearchCorpusActiveHeadV1>> {
    activate_at_gate(history, client, generation, batch, expected, None)
}

fn activate_at_gate(
    history: &History,
    client: &QuantaIndex,
    generation: u64,
    batch: &SearchCorpusBatch,
    expected: Option<SearchCorpusActiveHeadV1>,
    gate: Gate<'_>,
) -> TestResult<Option<SearchCorpusActiveHeadV1>> {
    let expected_model = expected.as_ref().map(head).transpose()?;
    history.record(
        Operation::Change {
            kind: Cas::Activate,
            generation,
            expected: expected_model,
        },
        || {
            wait_at_gate(gate)?;
            match client.search_corpus().publish_and_activate(batch, expected) {
                Ok((_, ack)) => Ok((
                    Observation::Changed {
                        previous: ack.previous_sealed_active.as_ref().map(head).transpose()?,
                        active: head(&ack.active)?,
                    },
                    Some(ack.active),
                )),
                Err(SdkError::AfterPublish {
                    stage: PublishedBatchFailureStage::Activation,
                    evidence,
                    source,
                }) if matches!(
                    *source,
                    SdkError::Remote {
                        code: SearchPlaneErrorCodeV2::CompositeActivationCasConflict,
                        ..
                    }
                ) =>
                {
                    evidence.publication.validate_published_receipt(
                        &evidence.publication,
                        true,
                        &evidence.receipt,
                    )?;
                    if evidence.publication.target.repo_id != *batch.repo_id()
                        || evidence.publication.target.revision_id != *batch.revision_id()
                        || evidence.publication.target.manifest_generation != batch.generation()
                        || evidence.publication.target.manifest_digest != batch.manifest_digest()
                        || evidence.publication.batch_digest != batch.batch_digest()?
                        || evidence.receipt.generation.get() != generation
                    {
                        return Err(
                            "activation conflict lost the fixture publication identity".into()
                        );
                    }
                    Ok((Observation::Conflict(Cas::Activate), None))
                }
                Err(SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::NotReady,
                    ..
                }) => Ok((Observation::SourceEventRefusal, None)),
                Err(error) => Err(error.into()),
            }
        },
    )
}

#[derive(Clone, Copy)]
enum Plane {
    Lexical,
    Semantic,
}

fn query(history: &History, client: &QuantaIndex, pinned: Option<u64>, plane: Plane) -> TestResult {
    query_at_gate(history, client, pinned, plane, None)
}

fn query_at_gate(
    history: &History,
    client: &QuantaIndex,
    pinned: Option<u64>,
    plane: Plane,
    gate: Gate<'_>,
) -> TestResult {
    history.record(Operation::Query { pinned }, || {
        wait_at_gate(gate)?;
        let repo = RepoId::new(REPO)?;
        let revision = RevisionId::new(REVISION)?;
        let (pin, selected, results) = match plane {
            Plane::Lexical => {
                let builder = client.lexical().query().sourcegraph(TEXT).top_k(16);
                let response = match pinned {
                    Some(generation) => builder
                        .pinned(GenerationPin::new(
                            repo,
                            revision,
                            ManifestGeneration::new(generation),
                        ))
                        .execute()?,
                    None => builder.active(repo, revision).execute()?,
                };
                (
                    response.generation,
                    response.selected_active_head,
                    response.results,
                )
            }
            Plane::Semantic => {
                let builder = client.semantic().query().text(TEXT).top_k(16);
                let response = match pinned {
                    Some(generation) => builder
                        .pinned(GenerationPin::new(
                            repo,
                            revision,
                            ManifestGeneration::new(generation),
                        ))
                        .execute()?,
                    None => builder.active(repo, revision).execute()?,
                };
                (
                    response.generation,
                    response.selected_active_head,
                    response.results,
                )
            }
        };
        if pin.repo_id.as_str() != REPO || pin.revision_id.as_str() != REVISION {
            return Err(format!("foreign query pin: {pin:?}").into());
        }
        let mut rows: Vec<_> = results
            .into_iter()
            .map(|candidate| Row {
                id: candidate.candidate_id,
                path: candidate.repo_relative_path.as_str().into(),
            })
            .collect();
        rows.sort();
        Ok((
            Observation::Rows {
                generation: pin.manifest_generation.get(),
                selected: selected.as_ref().map(head).transpose()?,
                rows,
            },
            (),
        ))
    })
}

#[test]
fn sdk_source_delete_append_pin_cas_duplicate_reorder_rollback_restart_history() -> TestResult {
    let directory = private_tempdir()?;
    let process = SearchdBinaryProcess::start(directory.path())?;
    let client = process.connect()?;
    let history = History::default();
    let inputs = [
        publication(1, None, false, &["a", "b"]),
        publication(2, Some(1), false, &["c"]),
        publication(3, Some(2), true, &[]),
        publication(4, Some(3), false, &["d"]),
        publication(5, None, false, &["e"]),
        publication(6, None, false, &["f"]),
    ];
    let streams = [
        "lineage",
        "lineage",
        "lineage",
        "lineage",
        "reordered",
        "post-restart",
    ];
    let batches = inputs
        .iter()
        .zip(streams)
        .map(|(input, stream)| batch(input, stream))
        .collect::<TestResult<Vec<_>>>()?;
    let mut expected = None;
    let mut origin = None;
    for (input, batch) in inputs.iter().zip(&batches) {
        if input.generation > 4 {
            break;
        }
        history.record(Operation::Publish(input.clone()), || {
            let _receipt = client.search_corpus().publish(batch)?;
            Ok((Observation::Published, ()))
        })?;
        if input.generation <= 3 {
            let active = activate(&history, &client, input.generation, batch, expected)?
                .ok_or("lineage activation refused")?;
            if input.generation == 1 {
                origin = Some(active.clone());
            }
            expected = Some(active);
        }
    }
    let origin = origin.ok_or("missing original activation")?;
    let initial = expected.ok_or("missing lineage head")?;
    let deferred_batch = batches.get(4).ok_or("missing deferred batch")?;
    let deferred_input = inputs.get(4).ok_or("missing deferred input")?;
    let prematurely_sealed =
        history.record(Operation::Publish(deferred_input.clone()), || match client
            .search_corpus()
            .publish(deferred_batch)
        {
            Ok(_) => Ok((Observation::Published, true)),
            Err(SdkError::Remote {
                code: SearchPlaneErrorCodeV2::NotReady,
                ..
            }) => Ok((Observation::SourceEventRefusal, false)),
            Err(error) => Err(error.into()),
        })?;
    if prematurely_sealed {
        return Err("higher source publication sealed before its predecessor activated".into());
    }
    // Replay exactly the sealed source event; this must not append C twice or change the active head.
    let append_input = inputs.get(1).ok_or("missing append input")?;
    let append_batch = batches.get(1).ok_or("missing append batch")?;
    history.record(Operation::Publish(append_input.clone()), || {
        let _receipt = client.search_corpus().publish(append_batch)?;
        Ok((Observation::Published, ()))
    })?;
    for generation in [1, 2, 3] {
        for plane in [Plane::Lexical, Plane::Semantic] {
            query(&history, &client, Some(generation), plane)?;
        }
    }
    // Connect before spawning participants. Bounded gates also release on caller failure.
    let contenders = [3_usize, 3, 3]
        .into_iter()
        .map(|index| {
            Ok((
                process.connect()?,
                batches.get(index).ok_or("missing contender batch")?.clone(),
                inputs
                    .get(index)
                    .ok_or("missing contender input")?
                    .generation,
            ))
        })
        .collect::<TestResult<Vec<_>>>()?;
    let delayed_client = process.connect()?;
    let successes = thread::scope(|scope| -> Result<usize, String> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let delayed_history = history.clone();
        let delayed = scope.spawn(move || {
            query_at_gate(
                &delayed_history,
                &delayed_client,
                None,
                Plane::Lexical,
                Some((&ready_tx, &release_rx)),
            )
            .map_err(|e| e.to_string())
        });
        let mut workers = Vec::new();
        let (contender_ready_tx, contender_ready_rx) = mpsc::sync_channel(3);
        let mut releases = Vec::new();
        // History retention admits only one unaccepted sealed candidate per pair.
        // Two duplicates compare G3; the third reuses the stale complete G1 head.
        for (position, (worker, candidate, generation)) in contenders.into_iter().enumerate() {
            let expected = if position == 2 {
                origin.clone()
            } else {
                initial.clone()
            };
            let history = history.clone();
            let ready = contender_ready_tx.clone();
            let (signal, permit) = mpsc::sync_channel(1);
            releases.push(signal);
            workers.push(scope.spawn(move || -> Result<bool, String> {
                activate_at_gate(
                    &history,
                    &worker,
                    generation,
                    &candidate,
                    Some(expected),
                    Some((&ready, &permit)),
                )
                .map(|head| head.is_some())
                .map_err(|e| e.to_string())
            }));
        }
        ready_rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|e| e.to_string())?;
        for _ in 0..3 {
            contender_ready_rx
                .recv_timeout(Duration::from_secs(30))
                .map_err(|e| e.to_string())?;
        }
        for release in releases {
            release.send(()).map_err(|e| e.to_string())?;
        }
        // Each RPC is a distinct observed operation; two reader planes may resolve at different instants.
        for plane in [Plane::Lexical, Plane::Semantic] {
            query(&history, &client, None, plane).map_err(|e| e.to_string())?;
            query(&history, &client, Some(1), plane).map_err(|e| e.to_string())?;
        }
        let mut successes = 0_usize;
        for worker in workers {
            if worker
                .join()
                .map_err(|_panic_payload| "CAS worker panicked")??
            {
                successes = successes.checked_add(1).ok_or("success count overflow")?;
            }
        }
        // This invocation began before the contenders, but its RPC completes after them.
        release_tx.send(()).map_err(|e| e.to_string())?;
        delayed
            .join()
            .map_err(|_panic_payload| "delayed reader panicked")??;
        Ok(successes)
    })?;
    if successes != 1 {
        return Err(format!("CAS had {successes} winners").into());
    }
    let winner = read_head(&history, &client)?;
    history.record(Operation::Publish(deferred_input.clone()), || {
        let _receipt = client.search_corpus().publish(deferred_batch)?;
        Ok((Observation::Published, ()))
    })?;
    let advanced = activate(&history, &client, 5, deferred_batch, Some(winner))?
        .ok_or("deferred candidate refused after predecessor activation")?;
    for plane in [Plane::Lexical, Plane::Semantic] {
        query(&history, &client, Some(5), plane)?;
    }
    let rolled = history.record(
        Operation::Change {
            kind: Cas::Rollback,
            generation: 1,
            expected: Some(head(&advanced)?),
        },
        || {
            let ack = client.generations().rollback(
                SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                    expected_active: advanced.clone(),
                    target: origin.generation.clone(),
                },
            )?;
            Ok((
                Observation::Changed {
                    previous: Some(head(&ack.previous_sealed_active)?),
                    active: head(&ack.active)?,
                },
                ack.active,
            ))
        },
    )?;
    // Restart is recorded as one operation only after all racing SDK calls have completed.
    let (process, client) = history.record(Operation::Restart, || {
        process.stop()?;
        let restarted = SearchdBinaryProcess::start(directory.path())?;
        let client = restarted.connect()?;
        let wire = client
            .generations()
            .active_head(RepoId::new(REPO)?, RevisionId::new(REVISION)?)?;
        Ok((
            Observation::Head(wire.as_ref().map(head).transpose()?),
            (restarted, client),
        ))
    })?;
    let next_batch = batches.get(5).ok_or("missing post-restart batch")?;
    let next_input = inputs.get(5).ok_or("missing post-restart input")?;
    history.record(Operation::Publish(next_input.clone()), || {
        let _receipt = client.search_corpus().publish(next_batch)?;
        Ok((Observation::Published, ()))
    })?;
    let _stale = activate(&history, &client, 6, next_batch, Some(origin))?;
    let _current = activate(&history, &client, 6, next_batch, Some(rolled))?;
    for plane in [Plane::Lexical, Plane::Semantic] {
        query(&history, &client, None, plane)?;
        for generation in [1, 2, 3, 4, 5] {
            query(&history, &client, Some(generation), plane)?;
        }
    }
    history.check()?;
    process.stop()
}

#[path = "lifecycle_history/oracle_tests.rs"]
mod oracle_tests;
