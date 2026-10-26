//! Fault and SIGKILL cuts at the current shared-object publication seams.
//!
//! Fixed source strings and a separately rebuilt generation are the query
//! goldens. A crash establishes process recovery, not power-loss durability.

#![expect(
    clippy::panic_in_result_fn,
    clippy::print_stderr,
    reason = "fault regressions assert exact cuts and report subprocess cleanup"
)]

use crate::publication_fixture::FixtureBatchBuildPort;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::Write as _;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, GenerationSnapshot, LQ_VERSION_TAG, LqExpr, LqFilter,
    LqLeaf, LqOptions, LqPatternType, LqQuery, LqSelect, LqSpan, ManifestGeneration,
    QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SearchCorpusTombstoneScope, SearchPlaneTrackKind, SourceFileKey,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, GenerationStorageKeyV1, LexicalIndexOpenPort,
    LexicalPageSpec, LexicalSearcher, MetricSourcePort, MetricValueV1, RequestBudgetV1,
};
use sha2::{Digest as _, Sha256};

use super::{Case, Cut, Guard, Mode, Side, WaitPoint};
use crate::LexicalAdapter;
use crate::lexical_store::ObjectStore;

use crate::current_source_fixture;

type TestResult = Result<(), Box<dyn Error>>;
type CustodyTree = BTreeMap<PathBuf, (u64, u64, [u8; 32])>;

const KEEP: &str = "src/keep.rs";
const EDIT: &str = "src/edit.rs";
const RETIRE: &str = "src/retire.rs";
const KEEP_BODY: &str = "keepneedle stable\n";
const OLD_BODY: &str = "oldneedle original\n";
const NEW_BODY: &str = "newneedle updated\n";
const RETIRE_BODY: &str = "retiredneedle gone\n";
const CHILD_TEST: &str = "publication_faults::tests::publication_crash_child";

fn repo() -> Result<RepoId, Box<dyn Error>> {
    Ok(RepoId::new("publication-pool")?)
}
fn revision() -> Result<RevisionId, Box<dyn Error>> {
    Ok(RevisionId::new("revision")?)
}

fn source_scope(path: &str, body: &str) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust")?;
    let chunk = ChunkRecord {
        chunk_id: ChunkId::new(format!("publication-{path}")),
        repo_relative_path: RepoRelativePath::new(path),
        language: language.clone(),
        start_byte: 0,
        end_byte: u32::try_from(body.len())?,
        start_line: 1,
        end_line: 1,
        text: body.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: Some(RepoId::new("source")?),
    };
    current_source_fixture::text_scope(
        &RepoId::new("source")?,
        &RevisionId::new("source-revision")?,
        path,
        language,
        body,
        vec![chunk],
    )
}

fn batch(updated: bool) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut batch = SearchCorpusIngestBatch {
        source_event: current_source_fixture::empty_event(),
        repo_id: repo()?,
        revision_id: revision()?,
        generation: ManifestGeneration::new(if updated { 2 } else { 1 }),
        base_generation: updated.then(|| ManifestGeneration::new(1)),
        manifest_digest: format!("publication-manifest-{}", if updated { 2 } else { 1 }),
        batch_digest: "0".repeat(64),
        mode: if updated {
            BatchIngestMode::Delta
        } else {
            BatchIngestMode::ReplaceGeneration
        },
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: if updated {
            vec![source_scope(EDIT, NEW_BODY)?]
        } else {
            vec![
                source_scope(KEEP, KEEP_BODY)?,
                source_scope(EDIT, OLD_BODY)?,
                source_scope(RETIRE, RETIRE_BODY)?,
            ]
        },
        tombstone_scopes: if updated {
            vec![SearchCorpusTombstoneScope {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("source")?,
                    repo_relative_path: RepoRelativePath::new(RETIRE),
                },
            }]
        } else {
            Vec::new()
        },
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    current_source_fixture::finish_batch(&mut batch)?;
    Ok(batch)
}

fn candidate(updated: bool) -> Result<GenerationSnapshot, Box<dyn Error>> {
    let batch = batch(updated)?;
    Ok(GenerationSnapshot {
        repo_id: batch.repo_id,
        revision_id: batch.revision_id,
        track: SearchPlaneTrackKind::Lexical,
        manifest_generation: batch.generation,
        manifest_digest: batch.manifest_digest,
    })
}

fn generation(root: &Path, updated: bool) -> Result<PathBuf, Box<dyn Error>> {
    Ok(
        GenerationStorageKeyV1::for_repo_revision(&repo()?, &revision()?)
            .generation_dir(root, ManifestGeneration::new(if updated { 2 } else { 1 })),
    )
}

fn file_tree(directory: &Path) -> Result<CustodyTree, Box<dyn Error>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let path = entry.path();
                let meta = entry.metadata()?;
                let inserted = result.insert(
                    path.strip_prefix(directory)?.to_path_buf(),
                    (
                        meta.dev(),
                        meta.ino(),
                        Sha256::digest(std::fs::read(&path)?).into(),
                    ),
                );
                assert!(inserted.is_none(), "fixture file repeats");
            } else {
                return Err("non-regular fixture entry".into());
            }
        }
    }
    Ok(result)
}

// Count committed digest files and their encoded file lengths, independently
// of the adapter's write-cost accumulator. Control files are outside objects/.
fn physical_pool_objects(root: &Path) -> Result<(u64, u64), Box<dyn Error>> {
    let directory = root.join("lexical-object-pool/objects");
    if !directory.exists() {
        return Ok((0, 0));
    }
    let mut count = 0_u64;
    let mut bytes = 0_u64;
    for shard in std::fs::read_dir(directory)? {
        let shard = shard?;
        let shard_name = shard.file_name();
        let shard_name = shard_name.to_str().ok_or("non-UTF8 object shard")?;
        if shard_name.len() != 2
            || !shard_name
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || !shard.file_type()?.is_dir()
        {
            return Err("unexpected object shard entry".into());
        }
        for object in std::fs::read_dir(shard.path())? {
            let object = object?;
            let name = object.file_name();
            let name = name.to_str().ok_or("non-UTF8 pool object")?;
            if name.len() != 64
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                || name.get(..2) != Some(shard_name)
                || !object.file_type()?.is_file()
            {
                return Err("unexpected pool object entry".into());
            }
            count = count.checked_add(1).ok_or("pool object count overflow")?;
            bytes = bytes
                .checked_add(object.metadata()?.len())
                .ok_or("pool encoded byte count overflow")?;
        }
    }
    Ok((count, bytes))
}

fn no_temporary_files(directory: &Path) -> TestResult {
    if !directory.exists() {
        return Ok(());
    }
    let mut pending = vec![directory.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else {
                let name = entry.file_name().to_string_lossy().into_owned();
                assert!(
                    !name.contains(".tmp-"),
                    "temporary survived retry: {}",
                    entry.path().display()
                );
            }
        }
    }
    Ok(())
}

fn pool_temporary_bytes(root: &Path) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let mut bytes = Vec::new();
    let mut pending = vec![root.join("lexical-object-pool/objects")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else if entry.file_name().to_string_lossy().contains(".tmp-") {
                bytes.push(std::fs::read(entry.path())?);
            }
        }
    }
    Ok(bytes)
}

fn assert_base_view(searcher: &dyn LexicalSearcher) -> TestResult {
    let budget = RequestBudgetV1::unbounded();
    for (needle, expected) in [
        ("keepneedle", Some(KEEP)),
        ("oldneedle", Some(EDIT)),
        ("retiredneedle", Some(RETIRE)),
        ("newneedle", None),
    ] {
        for file_surface in [false, true] {
            let mut options = LqOptions::defaults();
            if file_surface {
                options.pattern_type = LqPatternType::CodeSearch;
            }
            let request = LqQuery {
                lq_version: LQ_VERSION_TAG,
                expr: LqExpr::Leaf(if file_surface {
                    LqLeaf::RawString(needle.to_string())
                } else {
                    LqLeaf::Keyword(needle.to_string())
                }),
                filters: if file_surface {
                    vec![LqFilter::Select {
                        dim: LqSelect::File,
                    }]
                } else {
                    Vec::new()
                },
                options,
                directives: Vec::new(),
                source_span: LqSpan::eof(0),
            };
            let results = searcher
                .search_constrained(
                    &request,
                    &QueryConstraintSetV1::default(),
                    &LexicalPageSpec::first(10),
                    &budget,
                )?
                .candidates;
            assert_eq!(results.len(), usize::from(expected.is_some()));
            assert_eq!(
                results.first().map(|row| row.repo_relative_path.as_str()),
                expected
            );
        }
    }
    Ok(())
}

fn assert_not_admitted(adapter: &LexicalAdapter) -> TestResult {
    assert!(
        adapter
            .validate_generation_identity(&candidate(true)?)
            .is_err()
    );
    assert!(
        adapter
            .open(
                &repo()?,
                &revision()?,
                ManifestGeneration::new(2),
                &RequestBudgetV1::unbounded()
            )
            .is_err()
    );
    adapter.validate_generation_identity(&candidate(false)?)?;
    assert!(
        adapter
            .open(
                &repo()?,
                &revision()?,
                ManifestGeneration::new(1),
                &RequestBudgetV1::unbounded()
            )
            .is_ok()
    );
    Ok(())
}

fn assert_recovered_queries_match_fresh(adapter: &LexicalAdapter) -> TestResult {
    let fresh_root = tempfile::tempdir()?;
    let fresh = LexicalAdapter::with_state_root(fresh_root.path().to_path_buf());
    let mut full = batch(false)?;
    full.generation = ManifestGeneration::new(2);
    full.manifest_digest = "publication-manifest-2".to_string();
    full.replace_scopes = vec![
        source_scope(KEEP, KEEP_BODY)?,
        source_scope(EDIT, NEW_BODY)?,
    ];
    current_source_fixture::finish_batch(&mut full)?;
    assert!(fresh.build_fixture_batch(&full)?.is_some());
    let budget = RequestBudgetV1::unbounded();
    let recovered = adapter.open(&repo()?, &revision()?, ManifestGeneration::new(2), &budget)?;
    let rebuilt = fresh.open(&repo()?, &revision()?, ManifestGeneration::new(2), &budget)?;
    for (needle, expected_path) in [
        ("keepneedle", Some(KEEP)),
        ("newneedle", Some(EDIT)),
        ("oldneedle", None),
        ("retiredneedle", None),
    ] {
        for file_surface in [false, true] {
            let mut options = LqOptions::defaults();
            if file_surface {
                options.pattern_type = LqPatternType::CodeSearch;
            }
            let request = LqQuery {
                lq_version: LQ_VERSION_TAG,
                expr: LqExpr::Leaf(if file_surface {
                    LqLeaf::RawString(needle.to_string())
                } else {
                    LqLeaf::Keyword(needle.to_string())
                }),
                filters: if file_surface {
                    vec![LqFilter::Select {
                        dim: LqSelect::File,
                    }]
                } else {
                    Vec::new()
                },
                options,
                directives: Vec::new(),
                source_span: LqSpan::eof(0),
            };
            let observed = recovered
                .search_constrained(
                    &request,
                    &QueryConstraintSetV1::default(),
                    &LexicalPageSpec::first(10),
                    &budget,
                )?
                .candidates;
            let reference = rebuilt
                .search_constrained(
                    &request,
                    &QueryConstraintSetV1::default(),
                    &LexicalPageSpec::first(10),
                    &budget,
                )?
                .candidates;
            assert_eq!(observed.len(), usize::from(expected_path.is_some()));
            assert_eq!(reference.len(), usize::from(expected_path.is_some()));
            for (actual, expected) in observed.iter().zip(&reference) {
                assert_eq!(Some(actual.repo_relative_path.as_str()), expected_path);
                assert_eq!(actual.source_repo_id.as_str(), "source");
                assert_eq!(actual.candidate_id, expected.candidate_id);
                assert_eq!(actual.source, expected.source);
                assert_eq!(actual.score.to_bits(), expected.score.to_bits());
            }
        }
    }
    Ok(())
}

fn setup(root: &Path) -> Result<CustodyTree, Box<dyn Error>> {
    let adapter = LexicalAdapter::with_state_root(root.to_path_buf());
    assert!(adapter.build_fixture_batch(&batch(false)?)?.is_some());
    adapter.validate_generation_identity(&candidate(false)?)?;
    drop(adapter);
    file_tree(&generation(root, false)?)
}

fn recover(root: &Path, base_tree: &CustodyTree) -> TestResult {
    let adapter = LexicalAdapter::with_state_root(root.to_path_buf());
    assert!(adapter.build_fixture_batch(&batch(true)?)?.is_some());
    adapter.validate_generation_identity(&candidate(true)?)?;
    assert_recovered_queries_match_fresh(&adapter)?;
    assert_eq!(
        &file_tree(&generation(root, false)?)?,
        base_tree,
        "sealed base changed"
    );
    let target = generation(root, true)?;
    assert!(target.join("file-authority/root.cbor").is_file());
    assert!(target.join("text-authority/manifest.cbor").is_file());
    no_temporary_files(&target)?;
    // Retry publishes the correct generation. Maintenance, rather than retry,
    // owns orphan physical temporaries left by a pre-rename crash cut.
    let store = ObjectStore::open_existing(root)?;
    store.visit_adopted_roots(&mut |_reference| Ok(()))?;
    store.visit_object_digests(&mut |_digest| Ok(()))?;
    no_temporary_files(&root.join("lexical-object-pool"))?;
    Ok(())
}

fn native_current_ids(root: &Path, updated: bool) -> Result<BTreeSet<u64>, Box<dyn Error>> {
    let native = crate::lexical_publication::read_native_root(&generation(root, updated)?)?;
    let store = ObjectStore::open_existing(root)?;
    let mut cursor = store.map_cursor(&native.current, &[], None, None)?;
    let mut ids = BTreeSet::new();
    while let Some((key, _)) = cursor.next()? {
        let key: [u8; 8] = key
            .try_into()
            .map_err(|error| format!("invalid Native current key: {error:?}"))?;
        assert!(ids.insert(u64::from_be_bytes(key)), "duplicate current ID");
    }
    Ok(ids)
}

fn io_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for cut in [
        Cut::PoolObjectWrite,
        Cut::PoolObjectFileSync,
        Cut::PoolObjectRename,
        Cut::PoolObjectDirectorySync,
        Cut::PoolFullBarrier,
        Cut::AttemptJournalWrite,
        Cut::AttemptJournalFileSync,
        Cut::AttemptJournalRename,
        Cut::AttemptJournalDirectorySync,
        Cut::AttemptJournalAppendWrite,
        Cut::AttemptJournalAppendFileSync,
        Cut::RootWrite,
        Cut::RootFileSync,
        Cut::RootRename,
        Cut::RootDirectorySync,
    ] {
        for side in [Side::Before, Side::After] {
            cases.push(Case {
                cut,
                side,
                occurrence: 1,
            });
        }
    }
    // The initial clean head is occurrence 1. Exercise the first adoption's
    // pending and committed heads separately, then the second authority root.
    for (cut, occurrence) in [
        (Cut::AttemptJournalRename, 2),
        (Cut::AttemptJournalDirectorySync, 3),
        (Cut::RootRename, 2),
        (Cut::RootDirectorySync, 2),
    ] {
        for side in [Side::Before, Side::After] {
            cases.push(Case {
                cut,
                side,
                occurrence,
            });
        }
    }
    cases
}

fn crash_cases() -> Vec<Case> {
    [
        (Cut::PoolObjectRename, Side::After, 1),
        (Cut::PoolObjectDirectorySync, Side::After, 1),
        (Cut::PoolFullBarrier, Side::Before, 1),
        (Cut::PoolFullBarrier, Side::After, 1),
        (Cut::AttemptJournalRename, Side::After, 1),
        (Cut::AttemptJournalRename, Side::After, 2),
        (Cut::AttemptJournalDirectorySync, Side::After, 3),
        (Cut::AttemptJournalAppendFileSync, Side::After, 1),
        (Cut::RootWrite, Side::Before, 1),
        (Cut::RootRename, Side::After, 1),
        (Cut::RootDirectorySync, Side::After, 2),
    ]
    .into_iter()
    .map(|(cut, side, occurrence)| Case {
        cut,
        side,
        occurrence,
    })
    .collect()
}

struct ReleaseWaitPoint(Arc<WaitPoint>);

impl Drop for ReleaseWaitPoint {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::SeqCst);
    }
}

fn join_root_writer(
    handle: std::thread::JoinHandle<Result<(), quanta_index_core::CoreError>>,
    label: &str,
) -> TestResult {
    let result = handle.join().map_err(|payload| {
        let detail = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic payload");
        format!("{label} root writer panicked: {detail}")
    })?;
    result?;
    Ok(())
}

#[test]
fn independent_root_directory_writers_preserve_a_live_temporary() -> TestResult {
    let root = tempfile::tempdir()?;
    let family = root.path().join("text-authority");
    std::fs::create_dir(&family)?;
    let first_directory = std::fs::File::open(&family)?;
    let second_directory = std::fs::File::open(&family)?;
    let wait_point = Arc::new(WaitPoint::default());
    let release = ReleaseWaitPoint(Arc::clone(&wait_point));
    let first_point = Arc::clone(&wait_point);
    let first_family = family.clone();
    let first = std::thread::spawn(move || {
        let _guard = Guard::install(
            Case {
                cut: Cut::RootWrite,
                side: Side::Before,
                occurrence: 1,
            },
            Mode::Wait(first_point),
        );
        crate::index_store::write_atomic_durable_at(
            &first_directory,
            &first_family,
            "manifest.cbor",
            b"first",
            "first Native root",
        )
    });
    let started = Instant::now();
    while !wait_point.reached.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(10)
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let first_reached = wait_point.reached.load(Ordering::SeqCst);
    let first_temporary = std::fs::read_dir(&family)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".manifest.cbor.tmp-"))
        });
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let second_family = family.clone();
    let second = std::thread::spawn(move || {
        let _sent = started_tx.send(());
        let result = crate::index_store::write_atomic_durable_at(
            &second_directory,
            &second_family,
            "manifest.cbor",
            b"second",
            "second Native root",
        );
        let _sent = done_tx.send(());
        result
    });
    let second_started = started_rx.recv_timeout(Duration::from_secs(5)).is_ok();
    let second_blocked = matches!(
        done_rx.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    );
    let first_temporary_survived = first_temporary.as_ref().is_some_and(|path| path.exists());
    drop(release);
    join_root_writer(first, "first")?;
    join_root_writer(second, "second")?;
    assert!(first_reached && second_started && second_blocked);
    assert!(
        first_temporary_survived,
        "second writer removed a live temp"
    );
    assert_eq!(std::fs::read(family.join("manifest.cbor"))?, b"second");
    no_temporary_files(&family)
}

#[test]
fn completed_build_write_metrics_match_pool_files_and_skip_replay_and_refusals() -> TestResult {
    let root = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
    let initial = batch(false)?;
    let mut unsealed = initial.clone();
    unsealed.seal = false;
    assert!(matches!(
        adapter.build_fixture_batch(&unsealed),
        Err(CoreError::InvalidContract(message))
            if message.contains("source-event publication requires a sealed batch")
    ));
    assert_eq!(adapter.publication_write_stats()?.completed_builds, 0);
    assert_eq!(physical_pool_objects(root.path())?, (0, 0));

    assert!(adapter.build_fixture_batch(&initial)?.is_some());
    // Empty-pool first build: every digest file, including adoption manifests,
    // is a new object from this completed invocation; file length is encoded size.
    let physical = physical_pool_objects(root.path())?;
    assert!(
        physical.0 > 0 && physical.1 > 0,
        "fixture wrote no pool objects"
    );
    let completed = adapter.publication_write_stats()?;
    assert_eq!(completed.completed_builds, 1);
    assert_eq!(completed.new_objects, physical.0);
    assert_eq!(completed.new_encoded_bytes, physical.1);

    assert!(adapter.build_fixture_batch(&initial)?.is_none());
    assert_eq!(adapter.publication_write_stats()?, completed);
    assert_eq!(physical_pool_objects(root.path())?, physical);

    let guard = Guard::install(
        Case {
            cut: Cut::PoolObjectWriteCall,
            side: Side::Before,
            occurrence: 1,
        },
        Mode::Error,
    );
    assert!(adapter.build_fixture_batch(&batch(true)?).is_err());
    assert!(
        guard.fired(),
        "the failed delta did not reach pool object I/O"
    );
    drop(guard);
    assert_eq!(adapter.publication_write_stats()?, completed);

    let metrics = adapter.scrape()?;
    for (name, expected) in [
        ("lexical_completed_builds_with_pool_write_cost_total", 1),
        ("lexical_completed_build_pool_new_objects_total", physical.0),
        (
            "lexical_completed_build_pool_new_encoded_bytes_total",
            physical.1,
        ),
    ] {
        let observed = metrics
            .iter()
            .filter(|point| point.name == name)
            .map(|point| point.value)
            .collect::<Vec<_>>();
        assert_eq!(observed, vec![MetricValueV1::Counter(expected)], "{name}");
    }
    Ok(())
}

#[test]
fn pool_and_root_io_refusals_never_admit_unsealed_target() -> TestResult {
    for case in io_cases() {
        eprintln!("POOL-IO-CUT {case:?}");
        let root = tempfile::tempdir()?;
        let base_tree = setup(root.path())?;
        let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
        let guard = Guard::install(case, Mode::Error);
        let result = adapter.build_fixture_batch(&batch(true)?);
        assert!(guard.fired(), "cut not reached: {case:?}");
        let error = result.err().ok_or("injected failure was suppressed")?;
        assert!(
            error
                .to_string()
                .contains("injected publication I/O refusal"),
            "wrong error at {case:?}: {error}"
        );
        drop(guard);
        drop(adapter);
        let cold = LexicalAdapter::with_state_root(root.path().to_path_buf());
        assert_not_admitted(&cold)?;
        assert_eq!(file_tree(&generation(root.path(), false)?)?, base_tree);
        drop(cold);
        recover(root.path(), &base_tree)?;
    }
    Ok(())
}

#[test]
fn pool_errno_and_partial_write_faults_preserve_base_and_retry() -> TestResult {
    const PREFIX_BYTES: usize = 7;
    let enospc = nix::libc::ENOSPC;
    let eio = nix::libc::EIO;
    let mut exhausted_output = &mut [][..];
    let write_zero = match std::io::Write::write_all(&mut exhausted_output, b"x") {
        Err(error) => error.to_string(),
        Ok(()) => return Err("empty output admitted a nonempty write".into()),
    };
    for (name, case, mode, expected_context, expected_error, partial) in [
        (
            "pool-write-enospc",
            Case {
                cut: Cut::PoolObjectWriteCall,
                side: Side::Before,
                occurrence: 1,
            },
            Mode::OsError(enospc),
            "write object temporary",
            std::io::Error::from_raw_os_error(enospc).to_string(),
            false,
        ),
        (
            "pool-fsync-eio",
            Case {
                cut: Cut::PoolObjectFileSyncCall,
                side: Side::Before,
                occurrence: 1,
            },
            Mode::OsError(eio),
            "object temporary",
            std::io::Error::from_raw_os_error(eio).to_string(),
            false,
        ),
        (
            "pool-prefix-then-enospc",
            Case {
                cut: Cut::PoolObjectPartialWrite,
                side: Side::Before,
                occurrence: 1,
            },
            Mode::PartialWrite {
                prefix_bytes: PREFIX_BYTES,
                failure: super::PartialWriteFailure::OsError(enospc),
            },
            "write object temporary",
            std::io::Error::from_raw_os_error(enospc).to_string(),
            true,
        ),
        (
            "pool-prefix-then-write-zero",
            Case {
                cut: Cut::PoolObjectPartialWrite,
                side: Side::Before,
                occurrence: 1,
            },
            Mode::PartialWrite {
                prefix_bytes: PREFIX_BYTES,
                failure: super::PartialWriteFailure::WriteZero,
            },
            "write object temporary",
            write_zero,
            true,
        ),
    ] {
        let root = tempfile::tempdir()?;
        let base_tree = setup(root.path())?;
        let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
        let held = adapter.open(
            &repo()?,
            &revision()?,
            ManifestGeneration::new(1),
            &RequestBudgetV1::unbounded(),
        )?;
        assert_base_view(held.as_ref())?;
        let guard = Guard::install(case, mode);
        let result = adapter.build_fixture_batch(&batch(true)?);
        assert!(guard.fired(), "fault was not reached: {name}");
        let error = result.err().ok_or("injected fault was suppressed")?;
        assert!(
            error.to_string().contains(&expected_error),
            "wrong error at {name}: {error}"
        );
        assert!(
            error.to_string().contains(expected_context),
            "missing storage context at {name}: {error}"
        );
        if case.cut == Cut::PoolObjectFileSyncCall {
            assert!(
                error.to_string().contains("sync object temporary")
                    || error.to_string().contains("sync batched object temporary"),
                "missing pool file-sync context at {name}: {error}"
            );
        }
        drop(guard);
        assert_base_view(held.as_ref())?;
        if partial {
            assert_eq!(
                pool_temporary_bytes(root.path())?,
                vec![b"QLSTOR0".to_vec()],
                "the object temporary did not retain exactly the written prefix at {name}"
            );
        }
        drop(held);
        drop(adapter);
        let cold = LexicalAdapter::with_state_root(root.path().to_path_buf());
        assert_not_admitted(&cold)?;
        let cold_base = cold.open(
            &repo()?,
            &revision()?,
            ManifestGeneration::new(1),
            &RequestBudgetV1::unbounded(),
        )?;
        assert_base_view(cold_base.as_ref())?;
        assert_eq!(file_tree(&generation(root.path(), false)?)?, base_tree);
        drop(cold_base);
        drop(cold);
        recover(root.path(), &base_tree)?;
    }
    Ok(())
}

#[test]
fn retry_preserves_the_attempts_original_native_document_ids() -> TestResult {
    let root = tempfile::tempdir()?;
    let base_tree = setup(root.path())?;
    let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
    // Native root is durable at this cut; the outer seal has not been written.
    let guard = Guard::install(
        Case {
            cut: Cut::RootDirectorySync,
            side: Side::After,
            occurrence: 1,
        },
        Mode::Error,
    );
    assert!(adapter.build_fixture_batch(&batch(true)?).is_err());
    assert!(guard.fired(), "Native root cut was not reached");
    drop(guard);
    drop(adapter);
    let original_ids = native_current_ids(root.path(), true)?;
    let base_ids = native_current_ids(root.path(), false)?;
    assert_eq!(original_ids.len(), 2);
    assert_eq!(base_ids.len(), 3);
    assert_eq!(original_ids.intersection(&base_ids).count(), 1);
    let cold = LexicalAdapter::with_state_root(root.path().to_path_buf());
    assert_not_admitted(&cold)?;
    drop(cold);
    recover(root.path(), &base_tree)?;
    assert_eq!(native_current_ids(root.path(), true)?, original_ids);
    Ok(())
}

#[test]
fn target_only_pool_object_tamper_is_rejected_without_damaging_base() -> TestResult {
    let root = tempfile::tempdir()?;
    let base_tree = setup(root.path())?;
    recover(root.path(), &base_tree)?;
    let target_root =
        crate::lexical_publication::read_native_root(&generation(root.path(), true)?)?;
    let base_root = crate::lexical_publication::read_native_root(&generation(root.path(), false)?)?;
    let target_page = target_root
        .current
        .page
        .as_ref()
        .ok_or("target current map has no page")?;
    let target_id = *native_current_ids(root.path(), true)?
        .iter()
        .next()
        .ok_or("empty target current map")?;
    assert!(
        !base_root.root_objects().contains(&target_page.object),
        "tamper fixture must select a target-only object"
    );
    let mut digest = String::with_capacity(64);
    for byte in target_page.object.sha256 {
        write!(&mut digest, "{byte:02x}")?;
    }
    let object_path = root
        .path()
        .join("lexical-object-pool/objects")
        .join(digest.get(..2).ok_or("short digest")?)
        .join(digest);
    let mut bytes = std::fs::read(&object_path)?;
    let last = bytes.last_mut().ok_or("empty pool object")?;
    *last ^= 1;
    std::fs::write(&object_path, bytes)?;
    let store = ObjectStore::open_existing(root.path())?;
    assert!(
        store
            .map_get(&target_root.current, &target_id.to_be_bytes(), None)
            .is_err(),
        "tampered selected map object was accepted"
    );
    let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
    adapter.validate_generation_identity(&candidate(false)?)?;
    assert!(
        adapter
            .open(
                &repo()?,
                &revision()?,
                ManifestGeneration::new(1),
                &RequestBudgetV1::unbounded()
            )
            .is_ok()
    );
    assert_eq!(file_tree(&generation(root.path(), false)?)?, base_tree);
    Ok(())
}

struct ReapedChild(Child);
impl Drop for ReapedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            if let Err(error) = self.0.kill() {
                eprintln!("crash child cleanup kill: {error}");
            }
            if let Err(error) = self.0.wait() {
                eprintln!("crash child cleanup reap: {error}");
            }
        }
    }
}

#[test]
fn pool_and_root_sigkill_cuts_resume_without_admitting_staging() -> TestResult {
    for (index, case) in crash_cases().into_iter().enumerate() {
        eprintln!("POOL-SIGKILL-CUT {case:?}");
        let root = tempfile::tempdir()?;
        let base_tree = setup(root.path())?;
        let control = tempfile::tempdir()?;
        let marker = control.path().join("reached");
        let log = control.path().join("child.log");
        let output = std::fs::File::create(&log)?;
        let mut child = ReapedChild(
            Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    CHILD_TEST,
                    "--ignored",
                    "--test-threads",
                    "1",
                    "--nocapture",
                ])
                .env("QI_POOL_TEST_ROOT", root.path())
                .env("QI_POOL_TEST_MARKER", &marker)
                .env("QI_POOL_TEST_CASE", index.to_string())
                .stdin(Stdio::null())
                .stdout(output.try_clone()?)
                .stderr(output)
                .spawn()?,
        );
        let started = Instant::now();
        while !marker.exists() {
            if let Some(status) = child.0.try_wait()? {
                return Err(format!(
                    "child exited before {case:?}: {status}; {}",
                    std::fs::read_to_string(&log)?
                )
                .into());
            }
            if started.elapsed() > Duration::from_secs(30) {
                return Err(format!(
                    "child did not reach {case:?}; {}",
                    std::fs::read_to_string(&log)?
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(&marker)?,
            format!("{:?}/{:?}", case.cut, case.side)
        );
        child.0.kill()?;
        assert_eq!(child.0.wait()?.signal(), Some(9));
        let cold = LexicalAdapter::with_state_root(root.path().to_path_buf());
        assert_not_admitted(&cold)?;
        assert_eq!(file_tree(&generation(root.path(), false)?)?, base_tree);
        drop(cold);
        recover(root.path(), &base_tree)?;
    }
    Ok(())
}

#[test]
#[ignore = "private subprocess entrypoint; parent matrix supplies cut and kills process"]
fn publication_crash_child() -> TestResult {
    let root =
        PathBuf::from(std::env::var_os("QI_POOL_TEST_ROOT").ok_or("missing parent fixture")?);
    let marker =
        PathBuf::from(std::env::var_os("QI_POOL_TEST_MARKER").ok_or("missing parent marker")?);
    let index: usize = std::env::var("QI_POOL_TEST_CASE")?.parse()?;
    let case = *crash_cases().get(index).ok_or("unknown crash cut")?;
    let _guard = Guard::install(case, Mode::Pause(marker));
    let adapter = LexicalAdapter::with_state_root(root);
    let result = adapter.build_fixture_batch(&batch(true)?);
    Err(format!("crash cut was not reached: {case:?}; {result:?}").into())
}
