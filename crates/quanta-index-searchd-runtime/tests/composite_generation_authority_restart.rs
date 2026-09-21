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

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use quanta_index_contract::{
    SearchPlaneErrorCodeV2, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
};
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
use crate::searchd_lease_probe;

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
const ERR_ROLLBACK_TARGET_UNOPENABLE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::RollbackTargetUnopenable;
const ERR_ROLLBACK_CAS_CONFLICT: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::RollbackCasConflict;
const ERR_STATE_ROOT_IN_USE: SearchPlaneErrorCodeV2 = SearchPlaneErrorCodeV2::StateRootInUse;

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
    RepoId::new(REPO).expect("test fixture ID satisfies canonical policy")
}

fn revision() -> RevisionId {
    RevisionId::new(REVISION).expect("test fixture ID satisfies canonical policy")
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
        RepoId::new(repo_id).expect("test fixture ID satisfies canonical policy"),
        RevisionId::new(revision_id).expect("test fixture ID satisfies canonical policy"),
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

/// The identities the daemon activated, roots included (QI-BB-028).
///
/// The semantic content roots are what the plane sealed and can only be
/// learned from its receipts, so every later comparison and rollback names
/// them from here.
struct Activated {
    g0: Option<SearchCorpusGenerationIdentityV1>,
    g1: SearchCorpusGenerationIdentityV1,
    g2: SearchCorpusGenerationIdentityV1,
}

/// Publish and activate `raw_generation`, checking the ack names the
/// batch's tracks and the roots the sealed receipt attested.
fn publish_generation(
    client: &QuantaIndex,
    raw_generation: u64,
    digest: &str,
    expected_active: Option<SearchCorpusGenerationIdentityV1>,
) -> Result<SearchCorpusGenerationIdentityV1, Box<dyn Error>> {
    let (receipt, activation) = client
        .search_corpus()
        .publish_and_activate(&batch(raw_generation, digest)?, expected_active)?;
    let expected_tracks = composite_identity_for(REPO, REVISION, raw_generation, digest);
    if activation.active.lexical != expected_tracks.lexical
        || activation.active.semantic != expected_tracks.semantic
    {
        return Err(format!(
            "activation of G{raw_generation} selected foreign tracks: {:?}",
            activation.active
        )
        .into());
    }
    if Some(&activation.active.semantic_content) != receipt.semantic_content.as_ref() {
        return Err(format!(
            "activation of G{raw_generation} names roots the sealed receipt did not attest: ack={:?} receipt={:?}",
            activation.active.semantic_content, receipt.semantic_content
        )
        .into());
    }
    Ok(activation.active)
}

fn publish_two_generations(client: &QuantaIndex) -> Result<Activated, Box<dyn Error>> {
    let g1 = publish_generation(client, G1, G1_DIGEST, None)?;
    let g2 = publish_generation(client, G2, G2_DIGEST, Some(g1.clone()))?;
    Ok(Activated { g0: None, g1, g2 })
}

fn publish_three_generations(client: &QuantaIndex) -> Result<Activated, Box<dyn Error>> {
    let g0 = publish_generation(client, G0, G0_DIGEST, None)?;
    let g1 = publish_generation(client, G1, G1_DIGEST, Some(g0.clone()))?;
    let g2 = publish_generation(client, G2, G2_DIGEST, Some(g1.clone()))?;
    Ok(Activated {
        g0: Some(g0),
        g1,
        g2,
    })
}

fn rollback_g2_to_g1(client: &QuantaIndex, activated: &Activated) -> Result<(), SdkError> {
    client
        .generations()
        .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: activated.g2.clone(),
            target: activated.g1.clone(),
        })
        .map(|_ack| ())
}

/// The two track snapshots of one composite identity, with placeholder
/// roots: only its tracks are compared against a live identity, whose
/// roots the daemon sealed.
fn composite_identity_for(
    repo_id: &str,
    revision_id: &str,
    raw_generation: u64,
    digest: &str,
) -> SearchCorpusGenerationIdentityV1 {
    SearchCorpusGenerationIdentityV1 {
        lexical: quanta_index_sdk::GenerationSnapshot {
            repo_id: RepoId::new(repo_id).expect("test fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new(revision_id)
                .expect("test fixture ID satisfies canonical policy"),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: generation(raw_generation),
            manifest_digest: digest.to_string(),
        },
        semantic: quanta_index_sdk::GenerationSnapshot {
            repo_id: RepoId::new(repo_id).expect("test fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new(revision_id)
                .expect("test fixture ID satisfies canonical policy"),
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: generation(raw_generation),
            manifest_digest: digest.to_string(),
        },
        semantic_content: quanta_index_contract::SemanticContentRootsV1 {
            row_root_digest: format!("sha256:{:0>64x}", 0_u64),
            membership_root_digest: format!("sha256:{:0>64x}", 0_u64),
        },
    }
}

fn current_composite(client: &QuantaIndex) -> Result<SearchCorpusGenerationIdentityV1, SdkError> {
    current_composite_for(client, REPO, REVISION)
}

/// The active composite identity as the daemon reports it: both track
/// snapshots plus the semantic content roots from the status report.
fn current_composite_for(
    client: &QuantaIndex,
    repo_id: &str,
    revision_id: &str,
) -> Result<SearchCorpusGenerationIdentityV1, SdkError> {
    let lexical = client.generations().current(
        RepoId::new(repo_id).expect("test fixture ID satisfies canonical policy"),
        RevisionId::new(revision_id).expect("test fixture ID satisfies canonical policy"),
        SearchPlaneTrackKind::Lexical,
    )?;
    let semantic = client.generations().current(
        RepoId::new(repo_id).expect("test fixture ID satisfies canonical policy"),
        RevisionId::new(revision_id).expect("test fixture ID satisfies canonical policy"),
        SearchPlaneTrackKind::Semantic,
    )?;
    let semantic_content = client
        .generations()
        .status(
            RepoId::new(repo_id).expect("test fixture ID satisfies canonical policy"),
            RevisionId::new(revision_id).expect("test fixture ID satisfies canonical policy"),
        )?
        .semantic_content
        .ok_or_else(|| {
            SdkError::Protocol("an active pair reports no semantic content roots".to_string())
        })?;
    Ok(SearchCorpusGenerationIdentityV1 {
        lexical,
        semantic,
        semantic_content,
    })
}

fn publish_and_activate_for(
    client: &QuantaIndex,
    repo_id: &str,
    revision_id: &str,
    raw_generation: u64,
    digest: &str,
    expected_active: Option<SearchCorpusGenerationIdentityV1>,
) -> Result<SearchCorpusGenerationIdentityV1, Box<dyn Error>> {
    let (receipt, activation) = client.search_corpus().publish_and_activate(
        &batch_for(repo_id, revision_id, raw_generation, digest)?,
        expected_active,
    )?;
    let expected = composite_identity_for(repo_id, revision_id, raw_generation, digest);
    if activation.active.lexical != expected.lexical
        || activation.active.semantic != expected.semantic
        || Some(&activation.active.semantic_content) != receipt.semantic_content.as_ref()
    {
        return Err(format!(
            "publish-and-activate selected a foreign composite identity: expected tracks={expected:?} receipt roots={:?} observed={:?}",
            receipt.semantic_content, activation.active
        )
        .into());
    }
    Ok(activation.active)
}

#[test]
fn sealed_composite_history_survives_restart_and_admits_predecessor_rollback() -> TestResult {
    let directory = quanta_index_searchd_harness::private_tempdir()?;
    let first_process = RunningRuntime::start(directory.path(), "composite-history-first")?;
    let activated = publish_two_generations(&first_process.client)?;
    first_process.stop()?;

    let second_process = RunningRuntime::start(directory.path(), "composite-history-second")?;
    rollback_g2_to_g1(&second_process.client, &activated)?;
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
    let activated = publish_two_generations(&first_process.client)?;
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
    let rollback = rollback_g2_to_g1(&second_process.client, &activated);
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
    let activated = publish_three_generations(&first_client)?;
    let g0 = activated
        .g0
        .clone()
        .ok_or("three generations were published")?;

    let rollback =
        first_client
            .generations()
            .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active: activated.g1.clone(),
                target: g0,
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
    if current != activated.g2 {
        return Err(format!("stale rollback changed active G2: {current:?}").into());
    }
    drop(first_client);
    first_process.stop()?;

    let second_process = SearchdBinaryProcess::start(directory.path())?;
    let second_client = second_process.connect()?;
    let reopened = current_composite(&second_client)?;
    if reopened != activated.g2 {
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
    let activated = publish_two_generations(&first_client)?;
    if current_composite(&first_client)? != activated.g2 {
        return Err("child process did not activate exact G2 composite identity".into());
    }
    drop(first_client);
    first_process.stop()?;

    let second_process = SearchdBinaryProcess::start(directory.path())?;
    let second_client = second_process.connect()?;
    if current_composite(&second_client)? != activated.g2 {
        return Err("child process restart did not recover exact G2 composite identity".into());
    }
    rollback_g2_to_g1(&second_client, &activated)?;
    if current_composite(&second_client)? != activated.g1 {
        return Err("child process rollback did not activate exact G1 composite identity".into());
    }
    drop(second_client);
    second_process.stop()?;

    let third_process = SearchdBinaryProcess::start(directory.path())?;
    let third_client = third_process.connect()?;
    if current_composite(&third_client)? != activated.g1 {
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
    if rejected.status.success() || !rejection_text.contains(ERR_STATE_ROOT_IN_USE.as_wire_str()) {
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
    if !rejection_text.contains(ERR_STATE_ROOT_IN_USE.as_wire_str()) {
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
    if *code != ERR_STATE_ROOT_IN_USE {
        return Err(format!("expected {ERR_STATE_ROOT_IN_USE}, got {code}").into());
    }

    drop(first);
    let released = build_runtime(config_for(directory.path()))?;
    drop(released);
    Ok(())
}
