//! Native Large source, repository ranks and global rank pages across two OS restarts.

use super::*;
use std::{
    cmp::Ordering as SortOrder,
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
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

const LARGE_SOURCE_COUNT: usize = 4_096;
const LARGE_SEED: u64 = 5_864_059_738_136_528_177;
const REPO_PAGE_SIZE: u32 = 256;
const PATH_PAGE_SIZE: u32 = 128;

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

fn large_budget() -> RequestBudgetV1 {
    RequestBudgetV1::for_duration(Duration::from_secs(300))
}

fn large_config(root: &Path, socket: &Path) -> TestResult<serde_json::Value> {
    let mut value = config_with_reclaim(root, socket)?;
    value["read_sessions"] = serde_json::json!({
        "global_limit": 2, "per_uid_limit": 2,
        "terminal_ttl_millis": 300_000_u64, "sweep_limit": 2,
    });
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
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let operation = request.operation;
    let started = Instant::now();
    let submitted: TestResult<_> = client.submit(request, &large_budget()).map_err(Into::into);
    match submitted.and_then(|outcome| published(&outcome)) {
        Ok(view) => Ok(view),
        Err(error) => {
            let status = client.status(
                operation,
                &RequestBudgetV1::for_duration(Duration::from_secs(10)),
            );
            let mut diagnostics = String::new();
            let read = daemon
                .stderr_reader()?
                .take(64 * 1024)
                .read_to_string(&mut diagnostics);
            Err(format!(
                "Native Large SourceAdvance failed after {:?}: {error:?}; \
                 original operation status {status:?}; server diagnostics ({read:?}): {diagnostics}",
                started.elapsed(),
            )
            .into())
        }
    }
}

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
    files: &[ScopedFile],
    open_key: [u8; 32],
) -> TestResult<Observation> {
    let mut paths = BTreeMap::new();
    let mut after = None;
    for _ in 0..=LARGE_SOURCE_COUNT / usize::try_from(PATH_PAGE_SIZE)? {
        let page = client.query_paths(
            ViewSelection::Exact(view),
            file_spec().instance,
            "",
            after.as_deref(),
            PATH_PAGE_SIZE,
            &large_budget(),
        )?;
        require_equal(&page.view, &view, "large path view")?;
        require(!page.entries.is_empty(), "large path page is empty")?;
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
    }
    require_equal(&paths.len(), &LARGE_SOURCE_COUNT, "large path inventory")?;

    let mut ranked = Vec::with_capacity(LARGE_SOURCE_COUNT);
    let mut seen = BTreeSet::new();
    for repo_index in 0..params_for(ScaleTier::Large).repo_count {
        let repo = format!("repo{repo_index}");
        let result = client.search_text(
            ViewSelection::Exact(view),
            text_spec().instance,
            TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: repo_query_token(repo_index),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: REPO_PAGE_SIZE,
                cursor: None,
            },
            &large_budget(),
        )?;
        require_equal(&result.view, &view, "large text view")?;
        require_equal(
            &result.matched,
            &u64::from(REPO_PAGE_SIZE),
            "repo match count",
        )?;
        require(result.exhaustive, "repo text search was partial")?;
        require_equal(
            &result.hits.len(),
            &usize::try_from(REPO_PAGE_SIZE)?,
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
    require_equal(&seen.len(), &LARGE_SOURCE_COUNT, "all sources ranked once")?;
    let global_ranked =
        global_page::observe(client, view, files, expected_owners, source_refs, open_key)?;
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

fn verify_source_bodies(
    state_root: &Path,
    observed: &Observation,
    expected: &BTreeMap<String, [u8; 32]>,
) -> TestResult {
    let object_root = state_root.join("native-objects");
    let io: Arc<dyn ObjectReadIoPort> = Arc::new(TestReadIo);
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
    let lock = io.open(&object_root.join(".object-namespace.lease"), &mut work)?;
    let reader = FileObjectReader::open_existing(&object_root, lock, io, &mut work)?;
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

#[test]
fn native_large_scoped_corpus_two_process_restarts_preserve_sources_and_repo_ranks() -> TestResult {
    let release = LargeDaemonBinary::from_environment()?;
    if let Some(binary) = &release {
        binary.verify()?;
    }
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let state_root: PathBuf = temp.path().join("state");
    fs::create_dir(&state_root)?;
    fs::set_permissions(&state_root, fs::Permissions::from_mode(0o700))?;
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-config.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&large_config(&state_root, &socket)?)?,
    )?;
    let start = || match &release {
        Some(binary) => binary.start(&config_path, &socket),
        None => Daemon::start(&config_path, &socket),
    };

    let files = generate_scoped_corpus(ScaleTier::Large, LARGE_SEED)?;
    require_equal(&files.len(), &LARGE_SOURCE_COUNT, "large fixture count")?;
    let mut expected = BTreeMap::new();
    let mut expected_owners = BTreeMap::new();
    let mut paths = Vec::with_capacity(LARGE_SOURCE_COUNT);
    for (index, file) in files.iter().enumerate() {
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

    let preparation = start()?;
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &large_budget(),
    )?
    else {
        return Err("large workspace creation failed".into());
    };
    let profile_ingress = client.begin_input(operation(1), &large_budget())?;
    let profile = client.stage_profile(
        profile_ingress,
        IndexProfile {
            instances: vec![file_spec(), text_spec()],
        },
        &large_budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &large_budget(),
    )?)?;

    let ingress = client.begin_input(operation(2), &large_budget())?;
    let mut source_refs = BTreeMap::new();
    for (file_page, path_page) in files.chunks(64).zip(paths.chunks(64)) {
        let inputs = file_page
            .iter()
            .zip(path_page)
            .map(|(file, path)| BorrowedSourceInput {
                path,
                bytes: file.content.as_bytes(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
            })
            .collect::<Vec<_>>();
        let references = client.stage_sources_borrowed(ingress, &inputs, &large_budget())?;
        require_equal(&references.len(), &inputs.len(), "large source batch count")?;
        for (path, reference) in path_page.iter().zip(references) {
            require(
                source_refs.insert(path.clone(), reference).is_none(),
                "duplicate source ref",
            )?;
        }
    }
    require_equal(
        &source_refs.len(),
        &LARGE_SOURCE_COUNT,
        "staged source count",
    )?;
    let before = client.read_view(ViewSelection::Exact(configured), &large_budget())?;
    let mut mutations = Vec::with_capacity(LARGE_SOURCE_COUNT);
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
    let owners = client.stage_mutations(ingress, mutations, &large_budget())?;
    require_equal(
        &owners.count,
        &u64::try_from(LARGE_SOURCE_COUNT)?,
        "owner count",
    )?;
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
    let view = publish_sources(
        &client,
        &preparation,
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
    )?;
    preparation.stop()?;

    let first = start()?;
    let first_observed = observe(
        &NativeUdsClient::new(socket.clone()),
        view,
        &expected,
        &expected_owners,
        &source_refs,
        &files,
        [0x47; 32],
    );
    let first_stopped = first.stop();
    let first_observed = first_observed?;
    first_stopped?;
    verify_source_bodies(&state_root, &first_observed, &expected)?;

    let second = start()?;
    let second_observed = observe(
        &NativeUdsClient::new(socket.clone()),
        view,
        &expected,
        &expected_owners,
        &source_refs,
        &files,
        [0x48; 32],
    );
    let second_stopped = second.stop();
    let second_observed = second_observed?;
    second_stopped?;
    if let Some(binary) = &release {
        binary.verify()?;
    }
    verify_source_bodies(&state_root, &second_observed, &expected)?;
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
    )
}
