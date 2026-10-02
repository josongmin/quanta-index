//! UDS E2E coverage for readers racing a composite generation activation.
//!
//! The two generations deliberately use disjoint source-repository IDs and
//! predicate authority. A response can therefore only be the complete G1 or
//! G2 result set: stale authority, a partially opened corpus, or a mixed read
//! produces a third shape and fails this test.

#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    reason = "integration-test helpers outside `#[test]` fns assert fixture setup with `expect`; the workspace already permits this inside test fns and a helper that cannot set up its fixture has no caller to propagate to"
)]

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::{
    ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusActiveHeadV1, SearchPlaneErrorCodeV2, SourceFileKey, SourcePublicationEvent,
    lex::LanguageCode,
};
use quanta_index_sdk::{ConnectOptions, QuantaIndex, RepoMetaBatch, SdkError, SearchCorpusBatch};
use quanta_index_searchd_harness::{E2eRuntime, fixture_source_scope_v1};

type TestResult = Result<(), Box<dyn Error>>;

const REPO: &str = "repo-generation-activation-concurrency";
const REVISION: &str = "revision-generation-activation-concurrency";
const G1: u64 = 101;
const G2: u64 = 102;
const RESULT_COUNT: usize = 32;
const NEEDLE: &str = "generation_activation_concurrency_needle";
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

/// Harness-owned concurrency fixture (TOPT-03: runtime fixture ownership).
///
/// `E2eRuntime::boot` binds query/control/ingest under the same retention
/// policy the old `config_for` spelled out (8 generations, 16 MiB pair
/// bytes, 128 pairs, 256 MiB total). The test keeps its own query loop
/// and transition-window waits: those are scenario synchronization, not
/// lifecycle. Explicit `stop` surfaces a driver failure; drop remains the
/// unwind path.
struct RunningRuntime {
    runtime: E2eRuntime,
    query_socket: std::path::PathBuf,
    control_socket: std::path::PathBuf,
    ingest_socket: std::path::PathBuf,
}

impl RunningRuntime {
    fn start() -> Result<Self, Box<dyn Error>> {
        let mut runtime = E2eRuntime::boot()?;
        runtime.start()?;
        let (query_socket, control_socket, ingest_socket) = runtime
            .socket_paths()
            .ok_or_else(|| "fixture: driver started without socket paths".to_string())
            .map(|(query, control, ingest)| {
                (
                    query.to_path_buf(),
                    control.to_path_buf(),
                    ingest.to_path_buf(),
                )
            })?;
        Ok(Self {
            runtime,
            query_socket,
            control_socket,
            ingest_socket,
        })
    }

    fn connect(&self) -> Result<QuantaIndex, Box<dyn Error>> {
        Ok(QuantaIndex::connect(
            ConnectOptions::from_state_root(self.runtime.state_root())
                .with_query_socket(&self.query_socket)
                .with_control_socket(&self.control_socket)
                .with_ingest_socket(&self.ingest_socket),
        )?)
    }

    fn stop(self) -> TestResult {
        Ok(self.runtime.stop()?)
    }
}

fn repo() -> RepoId {
    RepoId::new(REPO).expect("test fixture ID satisfies canonical policy")
}

fn revision() -> RevisionId {
    RevisionId::new(REVISION).expect("test fixture ID satisfies canonical policy")
}

fn generation(raw: u64) -> ManifestGeneration {
    ManifestGeneration::new(raw)
}

#[derive(Debug, Eq, PartialEq)]
enum QueryEvent {
    Generation(u64),
    NotReady,
    TransitionQueryCompleted,
    Failed(String),
}

fn expect_query_event(events: &mpsc::Receiver<QueryEvent>, expected: &QueryEvent) -> TestResult {
    let deadline = Instant::now()
        .checked_add(SOCKET_TIMEOUT)
        .ok_or_else(|| "query event deadline overflow".to_string())?;
    let mut saw_not_ready = false;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(remaining) {
            Ok(QueryEvent::NotReady) => saw_not_ready = true,
            Ok(QueryEvent::Failed(error)) => {
                return Err(format!("query loop failed before {expected:?}: {error}").into());
            }
            Ok(observed) if &observed == expected => return Ok(()),
            Ok(observed) => {
                return Err(format!("expected {expected:?}, observed {observed:?}").into());
            }
            Err(error) => {
                return Err(format!(
                    "waiting for {expected:?}: {error}; saw_not_ready={saw_not_ready}"
                )
                .into());
            }
        }
    }
}

fn label(raw_generation: u64) -> String {
    format!("g{raw_generation}")
}

fn source_repo(raw_generation: u64, index: usize) -> RepoId {
    RepoId::new(format!("generation-{raw_generation}-source-{index:02}"))
        .expect("test fixture ID satisfies canonical policy")
}

fn expected_ids(raw_generation: u64) -> Vec<String> {
    let mut ids = (0..RESULT_COUNT)
        .map(|index| format!("generation-{raw_generation}-chunk-{index:02}"))
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

fn corpus_batch(raw_generation: u64) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let label = label(raw_generation);
    let mut batch = SearchCorpusBatch::replace_generation(
        repo(),
        revision(),
        generation(raw_generation),
        format!("manifest:generation-activation-concurrency:{label}"),
    )
    .source_event(SourcePublicationEvent {
        stream_id: "fixture:generation-activation-concurrency".to_string(),
        event_id: format!("fixture:generation-activation-concurrency:{raw_generation}"),
        expected_base_event_id: (raw_generation == G2)
            .then(|| format!("fixture:generation-activation-concurrency:{G1}")),
        payload_sha256: [0; 32],
    });

    for index in 0..RESULT_COUNT {
        let path = format!("src/{label}/file_{index:02}.rs");
        let text = format!("{NEEDLE} {label} source {index}");
        let end_byte = u32::try_from(text.len())?;
        let file_v1 = SourceFileKey {
            source_repo_id: source_repo(raw_generation, index),
            repo_relative_path: RepoRelativePath::new(path.clone()),
        };
        let scope_v1 = fixture_source_scope_v1(
            file_v1,
            revision(),
            vec![ChunkRecord {
                chunk_id: ChunkId::new(format!("generation-{raw_generation}-chunk-{index:02}")),
                repo_relative_path: RepoRelativePath::new(path),
                language: LanguageCode::new("rust")?,
                start_byte: 0,
                end_byte,
                start_line: 1,
                end_line: 1,
                text: text.into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: Some(source_repo(raw_generation, index)),
            }],
            Vec::new(),
        )?;
        batch = batch.replace_scope(
            scope_v1.coverage,
            scope_v1.source_bytes,
            scope_v1.chunks,
            scope_v1.symbols,
        );
    }
    Ok(batch)
}

fn authority_batch(raw_generation: u64) -> RepoMetaBatch {
    let label = label(raw_generation);
    (0..RESULT_COUNT).fold(
        RepoMetaBatch::new(repo(), revision(), generation(raw_generation)),
        |batch, index| batch.entry(source_repo(raw_generation, index), "epoch", label.clone()),
    )
}

fn publish_and_activate(
    client: &QuantaIndex,
    raw_generation: u64,
    expected_active: Option<SearchCorpusActiveHeadV1>,
) -> Result<SearchCorpusActiveHeadV1, Box<dyn Error>> {
    let _receipt = client
        .history()
        .publish_repo_meta(&authority_batch(raw_generation))?;
    let (_receipt, activation) = client
        .search_corpus()
        .publish_and_activate(&corpus_batch(raw_generation)?, expected_active)?;
    if activation.active.generation.lexical.manifest_generation != generation(raw_generation)
        || activation.active.generation.semantic.manifest_generation != generation(raw_generation)
    {
        return Err(format!(
            "activation did not promote complete generation {raw_generation}: {:?}",
            activation.active
        )
        .into());
    }
    Ok(activation.active)
}

fn assert_complete_generation(response: &quanta_index_contract::TextQueryResponse) -> TestResult {
    let observed_generation = response.generation.manifest_generation.get();
    let expected = match observed_generation {
        G1 | G2 => expected_ids(observed_generation),
        other => {
            return Err(format!(
                "query observed unexpected active generation {other}, expected {G1} or {G2}"
            )
            .into());
        }
    };
    let mut ids = response
        .results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    if ids != expected {
        return Err(format!(
            "generation {observed_generation} returned a partial, stale-authority, or mixed set: {ids:?}"
        )
        .into());
    }
    if response.results.iter().any(|candidate| {
        candidate.manifest_generation != generation(observed_generation)
            || candidate.repo_id != repo()
            || candidate.revision_id != revision()
    }) {
        return Err(format!(
            "generation {observed_generation} response contains a candidate from another authority"
        )
        .into());
    }
    Ok(())
}

fn query_active_generation(client: &QuantaIndex) -> Result<Option<u64>, Box<dyn Error>> {
    let response = match client
        .lexical()
        .query()
        .sourcegraph(format!(
            "repo:has.meta(epoch:g{G1}) OR repo:has.meta(epoch:g{G2}) {NEEDLE}"
        ))
        .active(repo(), revision())
        .top_k(u32::try_from(RESULT_COUNT)?)
        .execute()
    {
        Ok(response) => response,
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::NotReady,
            ..
        }) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    assert_complete_generation(&response)?;
    Ok(Some(response.generation.manifest_generation.get()))
}

#[test]
fn concurrent_queries_observe_only_complete_predicate_authority_generations() -> TestResult {
    let runtime = RunningRuntime::start()?;
    let publisher = runtime.connect()?;
    let query_client = runtime.connect()?;
    let active_g1 = publish_and_activate(&publisher, G1, None)?;

    let stop = Arc::new(AtomicBool::new(false));
    let transition_open = Arc::new(AtomicBool::new(false));
    let transition_queries = Arc::new(AtomicU64::new(0));
    let (events_tx, events_rx) = mpsc::channel();
    let query_stop = Arc::clone(&stop);
    let query_transition_open = Arc::clone(&transition_open);
    let query_transition_queries = Arc::clone(&transition_queries);
    let query_loop = thread::Builder::new()
        .name("generation-activation-concurrency-query".to_string())
        .spawn(move || -> Result<(), String> {
            let mut last_generation = None;
            let mut not_ready_reported = false;
            while !query_stop.load(Ordering::Acquire) {
                let observed = match query_active_generation(&query_client) {
                    Ok(Some(observed)) => {
                        not_ready_reported = false;
                        observed
                    }
                    Ok(None) => {
                        if !not_ready_reported {
                            let _reported = events_tx.send(QueryEvent::NotReady);
                            not_ready_reported = true;
                        }
                        continue;
                    }
                    Err(error) => {
                        let message = error.to_string();
                        let _reported = events_tx.send(QueryEvent::Failed(message.clone()));
                        return Err(message);
                    }
                };
                if query_transition_open.load(Ordering::Acquire) {
                    let previous = query_transition_queries.fetch_add(1, Ordering::AcqRel);
                    if previous == 0 {
                        let _reported = events_tx.send(QueryEvent::TransitionQueryCompleted);
                    }
                }
                if last_generation != Some(observed) {
                    let _reported = events_tx.send(QueryEvent::Generation(observed));
                    last_generation = Some(observed);
                }
            }
            Ok(())
        })?;

    let test_result = (|| -> TestResult {
        expect_query_event(&events_rx, &QueryEvent::Generation(G1))?;

        transition_open.store(true, Ordering::Release);
        expect_query_event(&events_rx, &QueryEvent::TransitionQueryCompleted)?;
        let active_g2 = publish_and_activate(&publisher, G2, Some(active_g1.clone()))?;
        if active_g2.generation.lexical.manifest_generation != generation(G2) {
            return Err("generation two activation acknowledgement did not select G2".into());
        }
        expect_query_event(&events_rx, &QueryEvent::Generation(G2))?;
        Ok(())
    })();

    transition_open.store(false, Ordering::Release);
    stop.store(true, Ordering::Release);
    let query_result: TestResult = match query_loop.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.into()),
        Err(panic) => Err(format!("query loop panicked: {panic:?}").into()),
    };
    let stop_result = runtime.stop();
    query_result?;
    test_result?;
    stop_result?;
    if transition_queries.load(Ordering::Acquire) == 0 {
        return Err("no query completed during the generation-two transition window".into());
    }
    Ok(())
}
