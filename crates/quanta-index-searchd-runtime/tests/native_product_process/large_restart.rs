//! Native Large source, repository ranks and global rank pages across two OS restarts.

use super::*;
use std::{
    cmp::Ordering as SortOrder,
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom},
    os::unix::fs::OpenOptionsExt as _,
    path::PathBuf,
};

use super::scale_source_fixture::{
    ScaleTier, ScopedFile, generate_scoped_corpus, params_for, repo_query_token,
};
use quanta_index_core::index::{IndexError, ObjectReadFile, ObjectReadIoPort};
use quanta_index_sdk::native_uds::BorrowedSourceInput;
use quanta_index_storage::FileObjectReader;
use sha2::{Digest as _, Sha256};

// Reuse the corpus runner's path and content binding without importing its
// Cargo crate, which still builds the Legacy benchmark harness.
#[path = "../../../quanta-index-searchd-harness/src/scale/native_index/executable.rs"]
mod executable;
use executable::NativeExecutable;

#[path = "large_restart/global_page.rs"]
mod global_page;
#[path = "large_restart/local_delta.rs"]
mod local_delta;

const LARGE_SOURCE_COUNT: usize = 4_096;
const LARGE_SEED: u64 = 5_864_059_738_136_528_177;
const REPO_PAGE_SIZE: u32 = 256;
const PATH_PAGE_SIZE: u32 = 128;

#[derive(Clone)]
struct RestartFixture {
    name: &'static str,
    files: Vec<ScopedFile>,
    repo_count: u32,
    repo_page_size: u32,
    path_page_size: u32,
    rank_page_size: u32,
    global_matches: usize,
    request_timeout: Duration,
}

impl RestartFixture {
    fn local() -> Self {
        const REPOS: u32 = 2;
        const FILES_PER_REPO: u32 = 5;
        let mut files = Vec::new();
        for repo in 0..REPOS {
            for file in 0..FILES_PER_REPO {
                let common = if file >= 2 { "scale_needle_token " } else { "" };
                let extra = if repo == 0 && file == 4 {
                    "longer body "
                } else {
                    ""
                };
                files.push(ScopedFile {
                    source_repo_id: format!("repo{repo}"),
                    repo_relative_path: format!("src/file_{file}.rs"),
                    content: format!(
                        "{} scale_needle_token {common}{extra}marker\n",
                        repo_query_token(repo)
                    ),
                });
            }
        }
        Self {
            global_matches: files.len(),
            name: "local",
            files,
            repo_count: REPOS,
            repo_page_size: FILES_PER_REPO,
            path_page_size: 4,
            rank_page_size: 4,
            request_timeout: Duration::from_secs(15),
        }
    }

    fn large() -> TestResult<Self> {
        let files = generate_scoped_corpus(ScaleTier::Large, LARGE_SEED)?;
        require_equal(&files.len(), &LARGE_SOURCE_COUNT, "large fixture count")?;
        Ok(Self {
            name: "large",
            files,
            repo_count: params_for(ScaleTier::Large).repo_count,
            repo_page_size: REPO_PAGE_SIZE,
            path_page_size: PATH_PAGE_SIZE,
            rank_page_size: 256,
            global_matches: LARGE_SOURCE_COUNT,
            request_timeout: Duration::from_secs(300),
        })
    }

    fn budget(&self) -> RequestBudgetV1 {
        RequestBudgetV1::for_duration(self.request_timeout)
    }
}

fn fixture_phase<T>(stage: &str, action: impl FnOnce() -> TestResult<T>) -> TestResult<T> {
    eprintln!("native lexical {stage}: start");
    let started = Instant::now();
    let result = action();
    eprintln!(
        "native lexical {stage}: {:?} ({})",
        started.elapsed(),
        if result.is_ok() { "ok" } else { "failed" }
    );
    result.map_err(|error| format!("{stage}: {error}").into())
}

fn daemon_stderr_tail(daemon: &Daemon) -> TestResult<String> {
    const LIMIT: u64 = 64 * 1024;
    let mut stderr = daemon.stderr_reader()?;
    let _offset = stderr.seek(SeekFrom::Start(
        stderr.metadata()?.len().saturating_sub(LIMIT),
    ))?;
    let mut buffer = Vec::new();
    let _read = stderr.take(LIMIT).read_to_end(&mut buffer)?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

struct LargeDaemonBinary(NativeExecutable);

impl LargeDaemonBinary {
    fn from_environment() -> TestResult<Option<Self>> {
        let path = std::env::var_os("QIDX_NATIVE_LARGE_RELEASE_BIN");
        let digest = std::env::var_os("QIDX_NATIVE_LARGE_RELEASE_SHA256");
        match (path, digest) {
            (None, None) => Ok(None),
            (Some(path), Some(digest)) => Ok(Some(Self::with_expected_digest(
                PathBuf::from(path),
                digest
                    .into_string()
                    .map_err(|_| "Native Large release SHA-256 must be UTF-8")?,
            )?)),
            _ => Err("Native Large release binary and SHA-256 must be supplied together".into()),
        }
    }

    fn with_expected_digest(path: PathBuf, expected_sha256: String) -> TestResult<Self> {
        require(
            expected_sha256.len() == 64
                && expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Native Large release SHA-256 must have 64 hex digits",
        )?;
        let binary = NativeExecutable::capture(&path)?;
        require_equal(
            &binary.sha256(),
            &expected_sha256.to_ascii_lowercase().as_str(),
            "Native Large release SHA-256 mismatch",
        )?;
        Ok(Self(binary))
    }

    fn verify(&self) -> TestResult {
        self.0.verify()?;
        Ok(())
    }

    fn start(&self, config: &Path, socket: &Path) -> TestResult<Daemon> {
        self.verify()?;
        Daemon::start_binary(self.0.path(), config, socket, None, None)
    }
}

fn file_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([17; 16]),
        kind: IndexKind([18; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: Digest([21; 32]),
        format: StorageFormatId([19; 16]),
    }
}

#[derive(Clone, Copy, Debug)]
enum LargeTextLayout {
    TreeV1,
    SegmentedFullV1,
    SegmentedLayeredV2,
}

fn large_text_layout() -> TestResult<LargeTextLayout> {
    match std::env::var("QIDX_NATIVE_LARGE_TEXT_LAYOUT") {
        Ok(layout) if layout == "segmented-full-v1" => Ok(LargeTextLayout::SegmentedFullV1),
        Ok(layout) if layout == "segmented-layered-v2" => Ok(LargeTextLayout::SegmentedLayeredV2),
        Err(std::env::VarError::NotPresent) => Ok(LargeTextLayout::TreeV1),
        _ => {
            Err("Native Large Text layout must be segmented-full-v1 or segmented-layered-v2".into())
        }
    }
}

fn large_text_spec(layout: LargeTextLayout) -> InstanceSpec {
    let mut spec = text_spec();
    match layout {
        LargeTextLayout::TreeV1 => {}
        LargeTextLayout::SegmentedFullV1 => {
            spec.policy = TextRecipe::segmented_full(Digest([31; 32])).output_policy();
            spec.format = StorageFormatId([76; 16]);
        }
        LargeTextLayout::SegmentedLayeredV2 => {
            spec.policy = TextRecipe::segmented_layered(Digest([31; 32])).output_policy();
            spec.format = StorageFormatId([77; 16]);
        }
    }
    spec
}

fn restart_config(
    root: &Path,
    socket: &Path,
    layout: LargeTextLayout,
    large: bool,
) -> TestResult<serde_json::Value> {
    let mut value = config_with_reclaim(root, socket)?;
    match layout {
        LargeTextLayout::TreeV1 => {}
        LargeTextLayout::SegmentedFullV1 => {
            value["text"]["layout"] = serde_json::json!("segmented-full-v1");
            value["text"]["descriptor"]["format"] = serde_json::json!(StorageFormatId([76; 16]).0);
        }
        LargeTextLayout::SegmentedLayeredV2 => {
            value["text"]["layout"] = serde_json::json!("segmented-layered-v2");
            value["text"]["descriptor"]["format"] = serde_json::json!(StorageFormatId([77; 16]).0);
        }
    }
    value["read_sessions"] = serde_json::json!({
        "global_limit": 2, "per_uid_limit": 2,
        "terminal_ttl_millis": 300_000_u64, "sweep_limit": 2,
    });
    if !large {
        return Ok(value);
    }
    // Keep the pin alive through one admitted request and its finalization.
    set_fixture_json(
        &mut value,
        "/resources/read_ticket_lease_ms",
        serde_json::json!(300_000_u64),
    )?;
    for axis in ["reads", "writes", "mutations"] {
        set_fixture_json(
            &mut value,
            &format!("/work_limits/{axis}"),
            serde_json::json!(67_108_864_u64),
        )?;
    }
    set_fixture_json(
        &mut value,
        "/work_limits/bytes",
        serde_json::json!(549_755_813_888_u64),
    )?;
    set_fixture_json(
        &mut value,
        "/resources/physical/inventory/max_files",
        serde_json::json!(30_000_u64),
    )?;
    Ok(value)
}

fn publish_sources(
    client: &NativeUdsClient,
    daemon: &Daemon,
    request: UpdateRequest,
    budget: &RequestBudgetV1,
) -> TestResult<(quanta_index_contract::index::ViewRef, Duration)> {
    let operation = request.operation;
    eprintln!("native lexical SourceAdvance: start");
    let started = Instant::now();
    let submitted: TestResult<_> = client.submit(request, budget).map_err(Into::into);
    match submitted.and_then(|outcome| published(&outcome)) {
        Ok(view) => Ok((view, started.elapsed())),
        Err(error) => {
            let failed_after = started.elapsed();
            let status = client.status(
                operation,
                &RequestBudgetV1::for_duration(Duration::from_secs(10)),
            );
            let stderr =
                daemon_stderr_tail(daemon).unwrap_or_else(|read| format!("unavailable: {read}"));
            Err(format!(
                "SourceAdvance failed after {failed_after:?}: {error:?}; \
                 operation {operation:?} status {status:?}; server stderr tail: {stderr}",
            )
            .into())
        }
    }
}

#[derive(Debug, PartialEq)]
struct Observation {
    paths: BTreeMap<String, quanta_index_contract::native_protocol::NativeFilePathEntry>,
    ranked: Vec<(String, OwnerKey, ObjectRef, u64)>,
    global_ranked: Vec<(String, OwnerKey, ObjectRef, u64)>,
}

fn observe(
    client: &NativeUdsClient,
    view: quanta_index_contract::index::ViewRef,
    expected: &BTreeMap<String, [u8; 32]>,
    expected_owners: &BTreeMap<String, OwnerKey>,
    source_refs: &BTreeMap<String, ObjectRef>,
    fixture: &RestartFixture,
    open_key: [u8; 32],
) -> TestResult<Observation> {
    let mut paths = BTreeMap::new();
    let mut after = None;
    let source_count = fixture.files.len();
    // A FileMap continuation may return a short page. Every advancing page
    // must yield a new path, so source_count bounds the traversal.
    for page_index in 0..source_count {
        let page = client
            .query_paths(
                ViewSelection::Exact(view),
                file_spec().instance,
                "",
                after.as_deref(),
                fixture.path_page_size,
                &fixture.budget(),
            )
            .map_err(|error| format!("FileMap page {page_index}: {error}"))?;
        require_equal(&page.view, &view, "FileMap selected view")?;
        require(
            !page.entries.is_empty(),
            format!("FileMap page {page_index} is empty"),
        )?;
        let has_more = page.has_more;
        for entry in page.entries {
            require(expected.contains_key(&entry.path), "foreign large path")?;
            require_equal(
                &Some(entry.owner),
                &expected_owners.get(&entry.path).copied(),
                "independent source owner",
            )?;
            require_equal(
                &Some(entry.source),
                &source_refs.get(&entry.path).copied(),
                "large source record",
            )?;
            if let Some(prior) = &after {
                require(prior < &entry.path, "large path order or cursor repeated")?;
            }
            after = Some(entry.path.clone());
            require(
                paths.insert(entry.path.clone(), entry).is_none(),
                "duplicate large path",
            )?;
        }
        if !has_more {
            break;
        }
        require(
            paths.len() < source_count,
            format!("FileMap page {page_index} continued past the source inventory"),
        )?;
    }
    require_equal(&paths.len(), &source_count, "source path inventory")?;

    let mut ranked = Vec::with_capacity(source_count);
    let mut seen = BTreeSet::new();
    for repo_index in 0..fixture.repo_count {
        let repo = format!("repo{repo_index}");
        let result = client
            .search_text(
                ViewSelection::Exact(view),
                text_spec().instance,
                TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: repo_query_token(repo_index),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: None,
                    generation_selector: None,
                    top_k: fixture.repo_page_size,
                    cursor: None,
                },
                &fixture.budget(),
            )
            .map_err(|error| format!("repository {repo} text query: {error}"))?;
        require_equal(&result.view, &view, "large text view")?;
        require_equal(
            &result.matched,
            &u64::from(fixture.repo_page_size),
            "repo match count",
        )?;
        require(result.exhaustive, "repo text search was partial")?;
        require_equal(
            &result.hits.len(),
            &usize::try_from(fixture.repo_page_size)?,
            "repo ranked page size",
        )?;
        let mut previous: Option<&quanta_index_contract::native_protocol::NativeTextHit> = None;
        for hit in &result.hits {
            let entry = paths
                .get(&hit.path)
                .ok_or("ranked path absent from inventory")?;
            require(
                hit.path.starts_with(&format!("{repo}/")),
                "ranked hit escaped its source repository",
            )?;
            require_equal(&hit.owner, &entry.owner, "ranked source owner")?;
            require_equal(&hit.source, &entry.source, "ranked source reference")?;
            require(seen.insert(hit.path.clone()), "duplicate ranked source")?;
            if let Some(prior) = previous {
                let score_order = prior.score.get().total_cmp(&hit.score.get());
                require(score_order != SortOrder::Less, "ranked score order")?;
                if score_order == SortOrder::Equal {
                    require(prior.owner < hit.owner, "ranked owner tie order")?;
                }
            }
            previous = Some(hit);
            ranked.push((
                hit.path.clone(),
                hit.owner,
                hit.source,
                hit.score.get().to_bits(),
            ));
        }
    }
    require_equal(&seen.len(), &source_count, "all sources ranked once")?;
    let global_ranked = global_page::observe(
        client,
        view,
        fixture,
        expected_owners,
        source_refs,
        open_key,
    )?;
    Ok(Observation {
        paths,
        ranked,
        global_ranked,
    })
}

#[derive(Debug)]
struct TestReadIo;

struct TestReadFile(File);

impl ObjectReadFile for TestReadFile {
    fn file(&self) -> Result<&File, IndexError> {
        Ok(&self.0)
    }
}

impl ObjectReadIoPort for TestReadIo {
    fn open(
        &self,
        path: &Path,
        work: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
        work.checkpoint()?;
        let nofollow = i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits())
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(nofollow)
            .open(path)
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        Ok(Box::new(TestReadFile(file)))
    }
}

pub(super) fn open_test_object_reader(
    state_root: &Path,
    work: &mut WorkBudget,
) -> TestResult<FileObjectReader> {
    let object_root = state_root.join("native-objects");
    let io: Arc<dyn ObjectReadIoPort> = Arc::new(TestReadIo);
    let lock = io.open(&object_root.join(".object-namespace.lease"), work)?;
    Ok(FileObjectReader::open_existing(
        &object_root,
        lock,
        io,
        work,
    )?)
}

fn verify_source_bodies(
    state_root: &Path,
    observed: &Observation,
    expected: &BTreeMap<String, [u8; 32]>,
) -> TestResult {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(300))
        .ok_or("large read deadline overflow")?;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 0,
            bytes: 536_870_912,
            mutations: 0,
        },
        deadline,
    );
    let reader = open_test_object_reader(state_root, &mut work)?;
    for (path, entry) in &observed.paths {
        let body = reader.get(entry.bytes, &mut work)?;
        require(
            body.registration.children.is_empty(),
            "source body has child references",
        )?;
        require_equal(
            &u64::try_from(body.payload.len())?,
            &entry.byte_len,
            "source body length",
        )?;
        let actual: [u8; 32] = Sha256::digest(&body.payload).into();
        require_equal(
            &actual,
            expected.get(path).ok_or("source oracle path missing")?,
            "independent source SHA-256",
        )?;
    }
    Ok(())
}

#[test]
fn native_large_release_digest_mismatch_refuses_before_spawn() -> TestResult {
    let temp = tempfile::tempdir()?;
    let candidate = temp.path().join("candidate");
    let marker = temp.path().join("spawned");
    fs::write(&candidate, b"#!/bin/sh\ntouch \"$3\"\n")?;
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700))?;
    let error = LargeDaemonBinary::with_expected_digest(candidate, "0".repeat(64))
        .and_then(|release| release.start(&marker, &temp.path().join("native.sock")))
        .err()
        .ok_or("wrong release binary SHA-256 started a daemon")?;
    require(
        error.to_string().contains("SHA-256 mismatch"),
        error.to_string(),
    )?;
    require(
        !marker.exists(),
        "release binary was spawned before verification",
    )
}

#[test]
fn native_large_release_digest_verifies_bytes_and_detects_replacement() -> TestResult {
    let temp = tempfile::tempdir()?;
    let candidate = temp.path().join("candidate");
    fs::write(&candidate, b"abc")?;
    let release = LargeDaemonBinary::with_expected_digest(
        candidate.clone(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
    )?;
    release.verify()?;
    fs::write(candidate, b"abd")?;
    require(
        release.verify().is_err(),
        "changed release binary was accepted",
    )
}

enum RestartMode {
    Local,
    DeltaCrash,
    Large,
}

fn run_restart_fixture(
    fixture: &RestartFixture,
    text_layout: LargeTextLayout,
    release: Option<LargeDaemonBinary>,
    mode: RestartMode,
) -> TestResult<Duration> {
    if let Some(binary) = &release {
        binary.verify()?;
    }
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let state_root: PathBuf = temp.path().join("state");
    fs::create_dir(&state_root)?;
    fs::set_permissions(&state_root, fs::Permissions::from_mode(0o700))?;
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-config.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&restart_config(
            &state_root,
            &socket,
            text_layout,
            matches!(mode, RestartMode::Large),
        )?)?,
    )?;
    let start = || match &release {
        Some(binary) => binary.start(&config_path, &socket),
        None => Daemon::start(&config_path, &socket),
    };

    let source_count = fixture.files.len();
    eprintln!(
        "native lexical fixture={} layout={text_layout:?} sources={source_count} repositories={} ranking_page={} request_budget={:?}",
        fixture.name, fixture.repo_count, fixture.rank_page_size, fixture.request_timeout,
    );
    let mut expected = BTreeMap::new();
    let mut expected_owners = BTreeMap::new();
    let mut paths = Vec::with_capacity(source_count);
    for (index, file) in fixture.files.iter().enumerate() {
        let path = format!("{}/{}", file.source_repo_id, file.repo_relative_path);
        let digest: [u8; 32] = Sha256::digest(file.content.as_bytes()).into();
        require(
            expected.insert(path.clone(), digest).is_none(),
            "duplicate fixture path",
        )?;
        let owner = OwnerKey {
            producer: operation(2).producer,
            stable_id: StableOwnerId(u128::try_from(index + 1)?.to_be_bytes()),
        };
        require(
            expected_owners.insert(path.clone(), owner).is_none(),
            "duplicate fixture owner path",
        )?;
        paths.push(path);
    }

    let preparation = fixture_phase("initial daemon start", start)?;
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &fixture.budget(),
    )?
    else {
        return Err("large workspace creation failed".into());
    };
    let profile_ingress = client.begin_input(operation(1), &fixture.budget())?;
    let profile = client.stage_profile(
        profile_ingress,
        IndexProfile {
            instances: vec![literal_spec(), file_spec(), large_text_spec(text_layout)],
        },
        &fixture.budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &fixture.budget(),
    )?)?;

    let ingress = client.begin_input(operation(2), &fixture.budget())?;
    let mut source_refs = BTreeMap::new();
    eprintln!("native lexical stage {source_count} sources: start");
    let stage_started = Instant::now();
    for (batch, (file_page, path_page)) in
        fixture.files.chunks(64).zip(paths.chunks(64)).enumerate()
    {
        let inputs = file_page
            .iter()
            .zip(path_page)
            .map(|(file, path)| BorrowedSourceInput {
                path,
                bytes: file.content.as_bytes(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
                language: None,
            })
            .collect::<Vec<_>>();
        let references = client
            .stage_sources_borrowed(ingress, &inputs, &fixture.budget())
            .map_err(|error| format!("stage Source batch {batch}: {error}"))?;
        require_equal(&references.len(), &inputs.len(), "large source batch count")?;
        for (path, reference) in path_page.iter().zip(references) {
            require(
                source_refs.insert(path.clone(), reference).is_none(),
                "duplicate source ref",
            )?;
        }
    }
    require_equal(&source_refs.len(), &source_count, "staged source count")?;
    eprintln!(
        "native lexical stage {source_count} sources: {:?}",
        stage_started.elapsed()
    );
    let before = client.read_view(ViewSelection::Exact(configured), &fixture.budget())?;
    let mut mutations = Vec::with_capacity(source_count);
    for path in &paths {
        mutations.push(OwnerMutation {
            owner: *expected_owners.get(path).ok_or("fixture owner missing")?,
            expected: None,
            next: Some(
                *source_refs
                    .get(path)
                    .ok_or("staged source reference missing")?,
            ),
        });
    }
    let owners = client.stage_mutations(ingress, mutations, &fixture.budget())?;
    require_equal(&owners.count, &u64::try_from(source_count)?, "owner count")?;
    let mut transition = SourceTransition {
        expected_source: before.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(2).producer,
        stream: operation(2).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let (view, submit_to_published) = publish_sources(
        &client,
        &preparation,
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &fixture.budget(),
    )?;
    eprintln!("native lexical SourceAdvance published in {submit_to_published:?}");
    let successor = if matches!(mode, RestartMode::DeltaCrash) {
        Some(fixture_phase("live DELTA between pinned pages", || {
            local_delta::interleave(
                &client,
                &preparation,
                view,
                fixture,
                &expected_owners,
                &source_refs,
            )
        })?)
    } else {
        None
    };
    let killed_pid = if successor.is_some() {
        Some(fixture_phase("SIGKILL after DELTA ACK", || {
            super::text_shard_contract::kill_fixture_daemon(preparation)
        })?)
    } else {
        fixture_phase("stop initial daemon", || preparation.stop())?;
        None
    };

    let observe_reopened = |mut daemon: Daemon, open_key, stage: &str| {
        let observed = fixture_phase(stage, || {
            let client = NativeUdsClient::new(socket.clone());
            let old = observe(
                &client,
                view,
                &expected,
                &expected_owners,
                &source_refs,
                fixture,
                open_key,
            )?;
            let updated = successor
                .as_ref()
                .map(|next| -> TestResult<Observation> {
                    let latest = ViewSelection::Latest {
                        workspace: next.operation.workspace,
                        epoch: next.operation.epoch,
                    };
                    require_equal(
                        &client.read_view(latest, &next.fixture.budget())?.identity,
                        &next.view,
                        "recovered DELTA head",
                    )?;
                    require_equal(
                        &client.status(next.operation, &next.fixture.budget())?,
                        &OperationStatus::Published(next.view),
                        "recovered DELTA operation terminal",
                    )?;
                    observe(
                        &client,
                        next.view,
                        &next.hashes,
                        &next.owners,
                        &next.refs,
                        &next.fixture,
                        [0x49; 32],
                    )
                    .map_err(|error| format!("reopened DELTA view: {error}").into())
                })
                .transpose()?;
            Ok((old, updated))
        });
        let stopped = daemon.stop_in_place();
        let observed = observed.map_err(|error| {
            let stderr =
                daemon_stderr_tail(&daemon).unwrap_or_else(|read| format!("unavailable: {read}"));
            format!("{error}; daemon stop {stopped:?}; server stderr tail: {stderr}")
        })?;
        stopped.map_err(|error| format!("{stage} daemon shutdown: {error}"))?;
        Ok::<_, Box<dyn std::error::Error>>(observed)
    };
    let first = fixture_phase("first reopen start", start)?;
    require(
        killed_pid.is_none_or(|pid| pid != first.0.id()),
        "hard reopen reused the killed daemon PID",
    )?;
    let (first_observed, first_delta) =
        observe_reopened(first, [0x47; 32], "first reopen queries")?;

    fixture_phase("first reopen Source digests", || {
        verify_source_bodies(&state_root, &first_observed, &expected)?;
        if let (Some(next), Some(observed)) = (&successor, &first_delta) {
            verify_source_bodies(&state_root, observed, &next.hashes)?;
        }
        Ok(())
    })?;

    let second = fixture_phase("second reopen start", start)?;
    let (second_observed, second_delta) =
        observe_reopened(second, [0x48; 32], "second reopen queries")?;

    if let Some(binary) = &release {
        binary.verify()?;
    }
    fixture_phase("second reopen Source digests", || {
        verify_source_bodies(&state_root, &second_observed, &expected)?;
        if let (Some(next), Some(observed)) = (&successor, &second_delta) {
            verify_source_bodies(&state_root, observed, &next.hashes)?;
        }
        Ok(())
    })?;
    for (path, first_entry) in &first_observed.paths {
        require_equal(
            &second_observed.paths.get(path),
            &Some(first_entry),
            "source identity across two restarts",
        )?;
    }
    for (index, (first_rank, second_rank)) in first_observed
        .ranked
        .iter()
        .zip(&second_observed.ranked)
        .enumerate()
    {
        require_equal(
            second_rank,
            first_rank,
            &format!("ranked row {index} across restarts"),
        )?;
    }
    require_equal(
        &second_observed.ranked.len(),
        &first_observed.ranked.len(),
        "ranked row count across restarts",
    )?;
    require_equal(
        &second_observed.global_ranked,
        &first_observed.global_ranked,
        "global ranked pages across two restarts",
    )?;
    require_equal(
        &second_delta,
        &first_delta,
        "DELTA observations across restarts",
    )?;
    Ok(submit_to_published)
}

#[test]
fn native_local_lexical_publish_rank_and_reopen() -> TestResult {
    let fixture = RestartFixture::local();
    let _submit_to_published = run_restart_fixture(
        &fixture,
        LargeTextLayout::SegmentedLayeredV2,
        None,
        RestartMode::Local,
    )?;
    Ok(())
}

#[test]
fn native_local_lexical_delta_interleaves_ties_and_survives_sigkill() -> TestResult {
    let fixture = RestartFixture::local();
    let _submit_to_published = run_restart_fixture(
        &fixture,
        LargeTextLayout::SegmentedLayeredV2,
        None,
        RestartMode::DeltaCrash,
    )?;
    Ok(())
}

#[test]
fn native_large_scoped_corpus_two_process_restarts_preserve_sources_and_repo_ranks() -> TestResult {
    let submit_to_published = run_restart_fixture(
        &RestartFixture::large()?,
        large_text_layout()?,
        LargeDaemonBinary::from_environment()?,
        RestartMode::Large,
    )?;
    if let Some(receipt_path) = std::env::var_os("QIDX_NATIVE_LARGE_ACK_RECEIPT") {
        let receipt_path = PathBuf::from(receipt_path);
        require(
            receipt_path.is_absolute(),
            "Native Large ACK receipt path must be absolute",
        )?;
        let mut receipt = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(receipt_path)?;
        serde_json::to_writer(
            &mut receipt,
            &serde_json::json!({
                "kind": "quanta-index-native-large-full-ack-v1",
                "source_count": LARGE_SOURCE_COUNT,
                "submit_to_published_ms": submit_to_published.as_secs_f64() * 1000.0,
            }),
        )?;
        receipt.sync_all()?;
    }
    Ok(())
}
