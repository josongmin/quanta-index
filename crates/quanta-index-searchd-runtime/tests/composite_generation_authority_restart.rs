//! Regression rails for composite generation authority across process boundaries.
//!
//! These tests intentionally use only the public SDK/runtime front doors. They
//! freeze three production invariants before the durable authority/lease repair:
//! historical rollback survives restart, rollback never admits an unopenable
//! track, and one state root has exactly one live daemon owner.

#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    reason = "integration-test helpers outside `#[test]` fns assert fixture setup with `expect`; the workspace already permits this inside test fns and a helper that cannot set up its fixture has no caller to propagate to"
)]

#[path = "common/searchd_binary_process.rs"]
mod searchd_binary_process;
#[path = "common/searchd_lease_probe.rs"]
mod searchd_lease_probe;

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use quanta_index_contract::SearchPlaneRollbackSearchCorpusGenerationCasRequest;
use quanta_index_core::{CoreError, GenerationStorageKeyV1};
use quanta_index_sdk::{
    ChunkId, ChunkRecord, ConnectOptions, LanguageCode, ManifestGeneration, QuantaIndex, RepoId,
    RepoRelativePath, RevisionId, SdkError, SearchCorpusBatch, SearchCorpusGenerationIdentityV1,
    SearchPlaneTrackKind, SearchScopeKey, SearchScopeSurface,
};
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd_runtime::build_runtime;

use crate::searchd_binary_process::SearchdBinaryProcess;

type TestResult = Result<(), Box<dyn Error>>;
type DriverJoin = thread::JoinHandle<anyhow::Result<()>>;

const REPO: &str = "repo-composite-restart";
const REVISION: &str = "revision-composite-restart";
const G0: u64 = 40;
const G1: u64 = 41;
const G2: u64 = 42;
const G0_DIGEST: &str = "manifest:composite-restart:g0";
const G1_DIGEST: &str = "manifest:composite-restart:g1";
const G2_DIGEST: &str = "manifest:composite-restart:g2";
const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);
const ERR_ROLLBACK_TARGET_UNOPENABLE: &str = "ROLLBACK_TARGET_UNOPENABLE";
const ERR_ROLLBACK_CAS_CONFLICT: &str = "ROLLBACK_CAS_CONFLICT";
const ERR_STATE_ROOT_IN_USE: &str = "STATE_ROOT_IN_USE";

static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);

struct RunningRuntime {
    client: QuantaIndex,
    shutdown: Arc<AtomicBool>,
    join: Option<DriverJoin>,
}

impl RunningRuntime {
    fn start(state_root: &Path, label: &str) -> Result<Self, Box<dyn Error>> {
        let runtime = build_runtime(config_for(state_root))?;
        let query_socket = runtime.query_server.socket_path().to_path_buf();
        let control_socket = runtime.control_server.socket_path().to_path_buf();
        let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
        let shutdown = Arc::new(AtomicBool::new(false));
        let driver_shutdown = Arc::clone(&shutdown);
        let join = thread::Builder::new()
            .name(label.to_string())
            .spawn(move || drive(runtime, &driver_shutdown))?;

        if !wait_until(SOCKET_TIMEOUT, || {
            query_socket.exists() && control_socket.exists() && ingest_socket.exists()
        }) {
            shutdown.store(true, Ordering::Release);
            return match join.join() {
                Ok(Ok(())) => Err("searchd sockets were not published before timeout".into()),
                Ok(Err(error)) => Err(error.into()),
                Err(panic) => Err(format!("searchd driver panicked: {panic:?}").into()),
            };
        }

        let client = QuantaIndex::connect(
            ConnectOptions::from_state_root(state_root)
                .with_query_socket(query_socket)
                .with_control_socket(control_socket)
                .with_ingest_socket(ingest_socket),
        )?;
        Ok(Self {
            client,
            shutdown,
            join: Some(join),
        })
    }

    fn stop(mut self) -> TestResult {
        self.shutdown.store(true, Ordering::Release);
        let join = self
            .join
            .take()
            .ok_or("searchd driver join handle was already consumed")?;
        match join.join() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(error.into()),
            Err(panic) => Err(format!("searchd driver panicked: {panic:?}").into()),
        }
    }
}

impl Drop for RunningRuntime {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            // Explicit tests call `stop` and surface driver failures. This is
            // only the unwind/early-return cleanup path where Drop cannot
            // return a second error without masking the primary failure.
            drop(join.join());
        }
    }
}

fn repo() -> RepoId {
    RepoId::new(REPO)
}

fn revision() -> RevisionId {
    RevisionId::new(REVISION)
}

fn generation(raw: u64) -> ManifestGeneration {
    ManifestGeneration::new(raw)
}

fn config_for(state_root: &Path) -> SearchdConfig {
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let prefix = format!(
        "qi-composite-authority-{}-{nanos}-{sequence}",
        std::process::id()
    );
    let temp = std::env::temp_dir();
    SearchdConfig::from_state_root(state_root.to_path_buf())
        .try_with_search_corpus_history_retention_limits(
            8,
            16 * 1024 * 1024,
            128,
            256 * 1024 * 1024,
        )
        .expect("valid test retention policy")
        .with_socket_overrides(
            temp.join(format!("{prefix}-query.sock")),
            temp.join(format!("{prefix}-control.sock")),
        )
        .with_ingest_socket_override(temp.join(format!("{prefix}-ingest.sock")))
}

fn wait_until<F>(timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if predicate() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

fn batch(raw_generation: u64, digest: &str) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let path = format!("src/generation_{raw_generation}.rs");
    let text = format!("fn generation_{raw_generation}() {{}}");
    let end_byte = u32::try_from(text.len())?;
    let language = LanguageCode::new("rust")?;
    Ok(SearchCorpusBatch::replace_generation(
        repo(),
        revision(),
        generation(raw_generation),
        digest,
    )
    .replace_scope(
        SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new(path.clone()),
        },
        format!("scope:composite-restart:{raw_generation}"),
        vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("chunk-generation-{raw_generation}")),
            repo_relative_path: RepoRelativePath::new(path),
            language,
            start_byte: 0,
            end_byte,
            start_line: 1,
            end_line: 1,
            text: text.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        Vec::new(),
    ))
}

fn batch_for(
    repo_id: &str,
    revision_id: &str,
    raw_generation: u64,
    digest: &str,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let path = format!("src/{repo_id}_generation_{raw_generation}.rs");
    let text = format!("fn generation_{raw_generation}() {{}}");
    let end_byte = u32::try_from(text.len())?;
    let language = LanguageCode::new("rust")?;
    Ok(SearchCorpusBatch::replace_generation(
        RepoId::new(repo_id),
        RevisionId::new(revision_id),
        generation(raw_generation),
        digest,
    )
    .replace_scope(
        SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new(path.clone()),
        },
        format!("scope:composite-restart:{raw_generation}"),
        vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("chunk-generation-{raw_generation}")),
            repo_relative_path: RepoRelativePath::new(path),
            language,
            start_byte: 0,
            end_byte,
            start_line: 1,
            end_line: 1,
            text: text.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        Vec::new(),
    ))
}

fn publish_two_generations(client: &QuantaIndex) -> Result<(), Box<dyn Error>> {
    let first = client
        .search_corpus()
        .publish_and_activate(&batch(G1, G1_DIGEST)?, None)?;
    if first.1.active.lexical.manifest_generation != generation(G1) {
        return Err("first composite activation did not select G1".into());
    }
    let second = client
        .search_corpus()
        .publish_and_activate(&batch(G2, G2_DIGEST)?, Some(first.1.active))?;
    if second.1.active.lexical.manifest_generation != generation(G2) {
        return Err("second composite activation did not select G2".into());
    }
    Ok(())
}

fn publish_three_generations(client: &QuantaIndex) -> Result<(), Box<dyn Error>> {
    let zero = client
        .search_corpus()
        .publish_and_activate(&batch(G0, G0_DIGEST)?, None)?;
    let one = client
        .search_corpus()
        .publish_and_activate(&batch(G1, G1_DIGEST)?, Some(zero.1.active))?;
    let two = client
        .search_corpus()
        .publish_and_activate(&batch(G2, G2_DIGEST)?, Some(one.1.active))?;
    if two.1.active != composite_identity(G2, G2_DIGEST) {
        return Err("three-generation setup did not activate exact G2 identity".into());
    }
    Ok(())
}

fn rollback_g2_to_g1(client: &QuantaIndex) -> Result<(), SdkError> {
    client
        .generations()
        .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: composite_identity(G2, G2_DIGEST),
            target: composite_identity(G1, G1_DIGEST),
        })
        .map(|_ack| ())
}

fn composite_identity(raw_generation: u64, digest: &str) -> SearchCorpusGenerationIdentityV1 {
    composite_identity_for(REPO, REVISION, raw_generation, digest)
}

fn composite_identity_for(
    repo_id: &str,
    revision_id: &str,
    raw_generation: u64,
    digest: &str,
) -> SearchCorpusGenerationIdentityV1 {
    SearchCorpusGenerationIdentityV1 {
        lexical: quanta_index_sdk::GenerationSnapshot {
            repo_id: RepoId::new(repo_id),
            revision_id: RevisionId::new(revision_id),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: generation(raw_generation),
            manifest_digest: digest.to_string(),
        },
        semantic: quanta_index_sdk::GenerationSnapshot {
            repo_id: RepoId::new(repo_id),
            revision_id: RevisionId::new(revision_id),
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: generation(raw_generation),
            manifest_digest: digest.to_string(),
        },
    }
}

fn current_composite(client: &QuantaIndex) -> Result<SearchCorpusGenerationIdentityV1, SdkError> {
    current_composite_for(client, REPO, REVISION)
}

fn current_composite_for(
    client: &QuantaIndex,
    repo_id: &str,
    revision_id: &str,
) -> Result<SearchCorpusGenerationIdentityV1, SdkError> {
    let lexical = client.generations().current(
        RepoId::new(repo_id),
        RevisionId::new(revision_id),
        SearchPlaneTrackKind::Lexical,
    )?;
    let semantic = client.generations().current(
        RepoId::new(repo_id),
        RevisionId::new(revision_id),
        SearchPlaneTrackKind::Semantic,
    )?;
    Ok(SearchCorpusGenerationIdentityV1 { lexical, semantic })
}

fn publish_and_activate_for(
    client: &QuantaIndex,
    repo_id: &str,
    revision_id: &str,
    raw_generation: u64,
    digest: &str,
    expected_active: Option<SearchCorpusGenerationIdentityV1>,
) -> Result<SearchCorpusGenerationIdentityV1, Box<dyn Error>> {
    let (_publish, activation) = client.search_corpus().publish_and_activate(
        &batch_for(repo_id, revision_id, raw_generation, digest)?,
        expected_active,
    )?;
    let expected = composite_identity_for(repo_id, revision_id, raw_generation, digest);
    if activation.active != expected {
        return Err(format!(
            "publish-and-activate selected a foreign composite identity: expected={expected:?} observed={:?}",
            activation.active
        )
        .into());
    }
    Ok(activation.active)
}

#[test]
fn sealed_composite_history_survives_restart_and_admits_predecessor_rollback() -> TestResult {
    let directory = quanta_index_searchd_harness::private_tempdir()?;
    let first_process = RunningRuntime::start(directory.path(), "composite-history-first")?;
    publish_two_generations(&first_process.client)?;
    first_process.stop()?;

    let second_process = RunningRuntime::start(directory.path(), "composite-history-second")?;
    rollback_g2_to_g1(&second_process.client)?;
    let current = current_composite(&second_process.client)?;
    current
        .validate_v1()
        .map_err(|error| format!("rollback produced split composite identity: {error}"))?;
    if current.lexical.manifest_generation != generation(G1)
        || current.lexical.manifest_digest != G1_DIGEST
    {
        return Err(format!("rollback did not restore G1: {current:?}").into());
    }
    second_process.stop()
}

#[derive(Clone, Copy)]
enum MissingTargetTrack {
    Lexical,
    Semantic,
}

fn assert_missing_target_rejected(track: MissingTargetTrack) -> TestResult {
    let directory = quanta_index_searchd_harness::private_tempdir()?;
    let first_process = RunningRuntime::start(directory.path(), "missing-target-first")?;
    publish_two_generations(&first_process.client)?;
    first_process.stop()?;

    let target_root = match track {
        MissingTargetTrack::Lexical => {
            GenerationStorageKeyV1::for_repo_revision(&repo(), &revision())
                .generation_dir(&directory.path().join("indexes/lexical"), generation(G1))
        }
        MissingTargetTrack::Semantic => {
            GenerationStorageKeyV1::for_repo_revision(&repo(), &revision()).generation_dir(
                &quanta_index_semantic::semantic_state_root(directory.path()),
                generation(G1),
            )
        }
    };
    std::fs::remove_dir_all(&target_root)?;

    let second_process = RunningRuntime::start(directory.path(), "missing-target-second")?;
    let rollback = rollback_g2_to_g1(&second_process.client);
    let Err(SdkError::Remote { code, .. }) = rollback else {
        return Err(format!(
            "rollback with missing target track did not return a typed remote error: {rollback:?}"
        )
        .into());
    };
    if code != ERR_ROLLBACK_TARGET_UNOPENABLE {
        return Err(format!(
            "missing target must report {ERR_ROLLBACK_TARGET_UNOPENABLE}, got {code}"
        )
        .into());
    }
    let current = current_composite(&second_process.client)?;
    current
        .validate_v1()
        .map_err(|error| format!("failed rollback split active authority: {error}"))?;
    if current.lexical.manifest_generation != generation(G2)
        || current.lexical.manifest_digest != G2_DIGEST
    {
        return Err(format!("failed rollback changed active G2: {current:?}").into());
    }
    second_process.stop()
}

#[test]
fn rollback_rejects_missing_lexical_target_and_preserves_active_composite() -> TestResult {
    assert_missing_target_rejected(MissingTargetTrack::Lexical)
}

#[test]
fn rollback_rejects_missing_semantic_target_and_preserves_active_composite() -> TestResult {
    assert_missing_target_rejected(MissingTargetTrack::Semantic)
}

#[test]
fn rollback_rejects_stale_expected_active_and_preserves_current_composite_v1() -> TestResult {
    let directory = quanta_index_searchd_harness::private_tempdir()?;
    let first_process = SearchdBinaryProcess::start(directory.path())?;
    let first_client = first_process.connect()?;
    publish_three_generations(&first_client)?;

    let rollback =
        first_client
            .generations()
            .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active: composite_identity(G1, G1_DIGEST),
                target: composite_identity(G0, G0_DIGEST),
            });
    let Err(SdkError::Remote { code, .. }) = rollback else {
        return Err(format!(
            "rollback with stale expected active did not return a typed remote error: {rollback:?}"
        )
        .into());
    };
    if code != ERR_ROLLBACK_CAS_CONFLICT {
        return Err(format!(
            "stale expected active must report {ERR_ROLLBACK_CAS_CONFLICT}, got {code}"
        )
        .into());
    }

    let current = current_composite(&first_client)?;
    current
        .validate_v1()
        .map_err(|error| format!("stale rollback split active authority: {error}"))?;
    if current != composite_identity(G2, G2_DIGEST) {
        return Err(format!("stale rollback changed active G2: {current:?}").into());
    }
    drop(first_client);
    first_process.stop()?;

    let second_process = SearchdBinaryProcess::start(directory.path())?;
    let second_client = second_process.connect()?;
    let reopened = current_composite(&second_client)?;
    if reopened != composite_identity(G2, G2_DIGEST) {
        return Err(format!("stale rollback changed reopened G2: {reopened:?}").into());
    }
    drop(second_client);
    second_process.stop()
}

#[test]
fn real_child_process_restart_preserves_and_rolls_back_composite_generation_v1() -> TestResult {
    let directory = quanta_index_searchd_harness::private_tempdir()?;

    let first_process = SearchdBinaryProcess::start(directory.path())?;
    let first_client = first_process.connect()?;
    publish_two_generations(&first_client)?;
    if current_composite(&first_client)? != composite_identity(G2, G2_DIGEST) {
        return Err("child process did not activate exact G2 composite identity".into());
    }
    drop(first_client);
    first_process.stop()?;

    let second_process = SearchdBinaryProcess::start(directory.path())?;
    let second_client = second_process.connect()?;
    if current_composite(&second_client)? != composite_identity(G2, G2_DIGEST) {
        return Err("child process restart did not recover exact G2 composite identity".into());
    }
    rollback_g2_to_g1(&second_client)?;
    if current_composite(&second_client)? != composite_identity(G1, G1_DIGEST) {
        return Err("child process rollback did not activate exact G1 composite identity".into());
    }
    drop(second_client);
    second_process.stop()?;

    let third_process = SearchdBinaryProcess::start(directory.path())?;
    let third_client = third_process.connect()?;
    if current_composite(&third_client)? != composite_identity(G1, G1_DIGEST) {
        return Err(
            "second child process restart did not preserve rolled-back G1 authority".into(),
        );
    }
    drop(third_client);
    third_process.stop()
}

#[test]
fn real_child_process_cross_repo_restart_retains_and_rolls_back_each_composite_v1() -> TestResult {
    const REPO_B: &str = "repo-composite-restart-b";
    const REVISION_B: &str = "revision-composite-restart-b";
    const A0_DIGEST: &str = "manifest:cross-repo:a:g0";
    const A1_DIGEST: &str = "manifest:cross-repo:a:g1";
    const A2_DIGEST: &str = "manifest:cross-repo:a:g2";
    const B0_DIGEST: &str = "manifest:cross-repo:b:g0";
    const B1_DIGEST: &str = "manifest:cross-repo:b:g1";
    const B2_DIGEST: &str = "manifest:cross-repo:b:g2";

    let directory = quanta_index_searchd_harness::private_tempdir()?;
    let first_process =
        SearchdBinaryProcess::start_with_history_max_generations(directory.path(), 2)?;
    let first_client = first_process.connect()?;

    let a0 = publish_and_activate_for(&first_client, REPO, REVISION, G0, A0_DIGEST, None)?;
    let b0 = publish_and_activate_for(&first_client, REPO_B, REVISION_B, G0, B0_DIGEST, None)?;
    let a1 = publish_and_activate_for(&first_client, REPO, REVISION, G1, A1_DIGEST, Some(a0))?;
    let b1 = publish_and_activate_for(&first_client, REPO_B, REVISION_B, G1, B1_DIGEST, Some(b0))?;
    let a2 = publish_and_activate_for(
        &first_client,
        REPO,
        REVISION,
        G2,
        A2_DIGEST,
        Some(a1.clone()),
    )?;
    let b2 = publish_and_activate_for(
        &first_client,
        REPO_B,
        REVISION_B,
        G2,
        B2_DIGEST,
        Some(b1.clone()),
    )?;

    if current_composite_for(&first_client, REPO, REVISION)? != a2
        || current_composite_for(&first_client, REPO_B, REVISION_B)? != b2
    {
        return Err("cross-repo setup did not preserve two independent active composites".into());
    }
    let rejected = searchd_lease_probe::require_start_failure(directory.path())?;
    let rejection_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    if rejected.status.success() || !rejection_text.contains(ERR_STATE_ROOT_IN_USE) {
        return Err(format!(
            "second child did not reject the live cross-repo state root with {ERR_STATE_ROOT_IN_USE}: {rejection_text}"
        )
        .into());
    }
    drop(first_client);
    first_process.stop()?;

    let second_process =
        SearchdBinaryProcess::start_with_history_max_generations(directory.path(), 2)?;
    let second_client = second_process.connect()?;
    if current_composite_for(&second_client, REPO, REVISION)? != a2
        || current_composite_for(&second_client, REPO_B, REVISION_B)? != b2
    {
        return Err("restart aliased or lost one repo's active composite".into());
    }

    let a_rollback = second_client.generations().rollback(
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: a2.clone(),
            target: a1.clone(),
        },
    )?;
    if a_rollback.active != a1 || a_rollback.previous_sealed_active != a2 {
        return Err("repo A rollback ack did not bind the exact CAS transition".into());
    }
    if current_composite_for(&second_client, REPO, REVISION)? != a1
        || current_composite_for(&second_client, REPO_B, REVISION_B)? != b2
    {
        return Err("repo A rollback changed repo B or failed to select retained A1".into());
    }
    let b_rollback = second_client.generations().rollback(
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: b2.clone(),
            target: b1.clone(),
        },
    )?;
    if b_rollback.active != b1 || b_rollback.previous_sealed_active != b2 {
        return Err("repo B rollback ack did not bind the exact CAS transition".into());
    }
    if current_composite_for(&second_client, REPO, REVISION)? != a1
        || current_composite_for(&second_client, REPO_B, REVISION_B)? != b1
    {
        return Err("repo B rollback changed repo A or failed to select retained B1".into());
    }
    drop(second_client);
    second_process.stop()?;

    let third_process =
        SearchdBinaryProcess::start_with_history_max_generations(directory.path(), 2)?;
    let third_client = third_process.connect()?;
    if current_composite_for(&third_client, REPO, REVISION)? != a1
        || current_composite_for(&third_client, REPO_B, REVISION_B)? != b1
    {
        return Err(
            "second restart did not preserve both independently rolled-back composites".into(),
        );
    }
    drop(third_client);
    third_process.stop()
}

#[test]
fn real_child_process_state_root_lease_rejects_second_owner_and_releases_v1() -> TestResult {
    let directory = quanta_index_searchd_harness::private_tempdir()?;
    let first_process = SearchdBinaryProcess::start(directory.path())?;

    let rejected = searchd_lease_probe::require_start_failure(directory.path())?;
    if rejected.status.success() {
        return Err("second child process unexpectedly acquired the live state root".into());
    }
    let rejection_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    if !rejection_text.contains(ERR_STATE_ROOT_IN_USE) {
        return Err(format!(
            "second child process did not report {ERR_STATE_ROOT_IN_USE}: {rejection_text}"
        )
        .into());
    }

    first_process.stop()?;
    let admitted_after_release = SearchdBinaryProcess::start(directory.path())?;
    admitted_after_release.stop()
}

#[test]
fn state_root_has_one_live_runtime_owner_and_releases_lease_on_drop() -> TestResult {
    let directory = quanta_index_searchd_harness::private_tempdir()?;
    let first = build_runtime(config_for(directory.path()))?;

    let second = build_runtime(config_for(directory.path()));
    let Err(error) = second else {
        return Err("a second runtime concurrently acquired the same state root".into());
    };
    let Some(CoreError::Typed { code, .. }) = error.downcast_ref::<CoreError>() else {
        return Err(format!("state-root conflict was not a typed CoreError: {error:#}").into());
    };
    if code != ERR_STATE_ROOT_IN_USE {
        return Err(format!("expected {ERR_STATE_ROOT_IN_USE}, got {code}").into());
    }

    drop(first);
    let released = build_runtime(config_for(directory.path()))?;
    drop(released);
    Ok(())
}
