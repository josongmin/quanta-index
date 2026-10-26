//! Region retirement and sparse-ordinal regressions using real managed Lance files.

use super::*;
use crate::manifest::VectorIndexSealV1;
use crate::native::{
    ann_files, ann_lance, ann_session::AnnLanceSession, ann_store::AnnStoreLimits,
};
use quanta_index_core::index::{MapBatchRequest, MapBatchWrite, MapValue, StagedMapRoots};
use quanta_index_core::{
    DiagnosticAnnIndexKindV1, DiagnosticAnnRegionPhaseV1, DiagnosticCaptureIdV1, DiagnosticClockV1,
    DiagnosticContextV1, DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1,
    DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticStageV1, DiagnosticTerminalV1,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

type MemberScan = (Option<Vec<u8>>, u32, usize, bool);

#[derive(Default)]
struct FileBatchCustody {
    reserves: AtomicU64,
    confirms: AtomicU64,
}

impl ObjectCustodyPort for FileBatchCustody {
    fn reserve_objects(
        &self,
        _attempt: AttemptRef,
        _objects: &[RegisteredObject],
    ) -> Result<(), IndexError> {
        let _previous = self.reserves.fetch_add(1, AtomicOrdering::SeqCst);
        Ok(())
    }

    fn register_objects(
        &self,
        _attempt: AttemptRef,
        _objects: &[RegisteredObject],
    ) -> Result<(), IndexError> {
        let _previous = self.confirms.fetch_add(1, AtomicOrdering::SeqCst);
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum FileInstallFault {
    None,
    FailAfterBatch,
    CancelAfterBatch,
    FailBeforeDescriptor,
}

struct FileInstallSpy<'a> {
    inner: &'a MemoryStore,
    scalar: AtomicU64,
    batch: AtomicU64,
    fault: FileInstallFault,
}

impl ImmutableObjectPort for FileInstallSpy<'_> {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        let _previous = self.scalar.fetch_add(1, AtomicOrdering::SeqCst);
        self.inner.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        let _previous = self.batch.fetch_add(1, AtomicOrdering::SeqCst);
        if matches!(self.fault, FileInstallFault::FailBeforeDescriptor) {
            let (_descriptor, chunks) = inputs
                .split_last()
                .ok_or_else(|| IndexError::BackendViolation("empty ANN final test batch".into()))?;
            let rows: Vec<_> = inputs
                .iter()
                .map(|input| self.inner.identify(&input.payload, &input.children, budget))
                .collect::<Result<_, _>>()?;
            sink.reserve_batch(&rows)?;
            for chunk in chunks {
                let _partial = self
                    .inner
                    .insert(chunk.payload.clone(), chunk.children.clone())?;
            }
            return Err(IndexError::Storage(
                "injected failure before descriptor install".into(),
            ));
        }
        let installed = self.inner.install_batch(inputs, sink, budget)?;
        match self.fault {
            FileInstallFault::None => Ok(installed),
            FileInstallFault::FailAfterBatch => {
                Err(IndexError::Storage("injected post-batch failure".into()))
            }
            FileInstallFault::CancelAfterBatch => {
                budget.request().cancel_handle().cancel();
                Ok(installed)
            }
            FileInstallFault::FailBeforeDescriptor => Err(IndexError::BackendViolation(
                "ANN partial fault escaped its pre-descriptor guard".into(),
            )),
        }
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        self.inner.get(reference, budget)
    }
}

fn file_install_spy(inner: &MemoryStore, fault: FileInstallFault) -> FileInstallSpy<'_> {
    FileInstallSpy {
        inner,
        scalar: AtomicU64::new(0),
        batch: AtomicU64::new(0),
        fault,
    }
}

fn file_batch_seal() -> ann_lance::RegionSeal {
    ann_lance::RegionSeal {
        generation: 1,
        recipe: Digest([9; 32]),
        dimension: 2,
        rows: 1,
        rows_digest: Digest([8; 32]),
        vector_index: VectorIndexSealV1 {
            mode: "exact".into(),
            library: "lancedb".into(),
            library_version: "test".into(),
            index_min_rows: 2,
            ann: None,
        },
    }
}

#[test]
fn ann_fileset_cross_file_batch_preserves_fixed_manifest_and_readback() -> TestResult {
    let store = MemoryStore::default();
    let spy = file_install_spy(&store, FileInstallFault::None);
    let maps = GraphMap::new(&store);
    let custody = FileBatchCustody::default();
    let files: Vec<_> = (0_u8..5)
        .map(|ordinal| (format!("data/{ordinal:03}.lance"), vec![ordinal]))
        .collect();
    let inputs: Vec<_> = files
        .iter()
        .map(|(path, bytes)| ann_files::AnnFileInput { path, bytes })
        .collect();
    let memory = Arc::new(QueryMemory::default());
    let mut work = ann_budget()?;
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&memory),
    )))));
    let seal = file_batch_seal();
    let fileset = ann_files::install_fileset(
        &mut AttemptContext {
            attempt: attempt(),
            objects: &spy,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
        seal.clone(),
        &inputs,
    )?;
    check_eq!(spy.batch.load(AtomicOrdering::SeqCst), 1);
    check_eq!(spy.scalar.load(AtomicOrdering::SeqCst), 0);
    check_eq!(custody.reserves.load(AtomicOrdering::SeqCst), 1);
    check_eq!(custody.confirms.load(AtomicOrdering::SeqCst), 1);

    let expected_files: Vec<_> = files
        .iter()
        .map(|(path, bytes)| -> Result<ann_files::AnnFile, IndexError> {
            Ok(ann_files::AnnFile {
                path: path.clone(),
                length: u64::try_from(bytes.len()).map_err(|_error| IndexError::ResourceLimited)?,
                sha256: Digest(Sha256::digest(bytes).into()),
                chunks: vec![ann_files::AnnChunk {
                    reference: MemoryStore::reference(bytes, &[]),
                    length: u32::try_from(bytes.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                }],
            })
        })
        .collect::<Result<_, _>>()?;
    let mut expected_children: Vec<_> = expected_files
        .iter()
        .flat_map(|file| file.chunks.iter().map(|chunk| chunk.reference))
        .collect();
    expected_children.sort_unstable();
    expected_children.dedup();
    let expected_manifest = ann_files::AnnFileSet {
        format: 1,
        recipe: seal.recipe,
        seal,
        total_length: 5,
        files: expected_files,
    };
    let expected_payload = serde_json::to_vec(&expected_manifest)?;
    check_eq!(
        fileset,
        MemoryStore::reference(&expected_payload, &expected_children)
    );
    let stored = spy.get(fileset, &mut work)?;
    check_eq!(stored.payload, expected_payload);
    check_eq!(stored.registration.children, expected_children);
    let loaded = ann_files::load_fileset(&spy, fileset, &mut work)?;
    check_eq!(loaded.total_length, 5);
    check_eq!(loaded.files.len(), files.len());
    for (actual, (path, bytes)) in loaded.files.iter().zip(&files) {
        check_eq!(&actual.path, path);
        check_eq!(actual.chunks.len(), 1);
        check_eq!(
            &actual
                .chunks
                .first()
                .ok_or("fixed manifest chunk missing")?
                .object
                .payload,
            bytes
        );
    }
    drop(loaded);
    drop(work);
    check_query_memory_released(&memory)
}

#[test]
fn ann_fileset_chunk_batch_flushes_at_bytes_and_slots_without_widening_policy() -> TestResult {
    let store = MemoryStore::default();
    let spy = file_install_spy(&store, FileInstallFault::None);
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let mut work = ann_budget()?;
    let empty = ann_files::install_fileset(
        &mut AttemptContext {
            attempt: attempt(),
            objects: &spy,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
        file_batch_seal(),
        &[],
    );
    check!(matches!(empty, Err(IndexError::InvalidInput(_))));
    check_eq!(spy.batch.load(AtomicOrdering::SeqCst), 0);
    check_eq!(spy.scalar.load(AtomicOrdering::SeqCst), 0);

    let store = MemoryStore::default();
    let spy = file_install_spy(&store, FileInstallFault::None);
    let maps = GraphMap::new(&store);
    let files: Vec<_> = (0..65)
        .map(|ordinal| (format!("data/{ordinal:03}.lance"), vec![b'x']))
        .collect();
    let inputs: Vec<_> = files
        .iter()
        .map(|(path, bytes)| ann_files::AnnFileInput { path, bytes })
        .collect();
    let mut work = ann_budget()?;
    let fileset = ann_files::install_fileset(
        &mut AttemptContext {
            attempt: attempt(),
            objects: &spy,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
        file_batch_seal(),
        &inputs,
    )?;
    check_eq!(spy.batch.load(AtomicOrdering::SeqCst), 2);
    check_eq!(spy.scalar.load(AtomicOrdering::SeqCst), 0);
    check_eq!(
        ann_files::load_fileset(&spy, fileset, &mut work)?
            .files
            .len(),
        65
    );

    let store = MemoryStore::default();
    let spy = file_install_spy(&store, FileInstallFault::None);
    let maps = GraphMap::new(&store);
    let full: Vec<_> = (0..64)
        .map(|ordinal| (format!("data/{ordinal:03}.lance"), vec![b'x']))
        .collect();
    let inputs: Vec<_> = full
        .iter()
        .map(|(path, bytes)| ann_files::AnnFileInput { path, bytes })
        .collect();
    let mut work = ann_budget()?;
    let fileset = ann_files::install_fileset(
        &mut AttemptContext {
            attempt: attempt(),
            objects: &spy,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
        file_batch_seal(),
        &inputs,
    )?;
    check_eq!(spy.batch.load(AtomicOrdering::SeqCst), 1);
    check_eq!(spy.scalar.load(AtomicOrdering::SeqCst), 1);
    check_eq!(
        ann_files::load_fileset(&spy, fileset, &mut work)?
            .files
            .len(),
        64
    );

    let store = MemoryStore::default();
    let spy = file_install_spy(&store, FileInstallFault::None);
    let maps = GraphMap::new(&store);
    let large = vec![b'y'; 700_000];
    let files = [
        ann_files::AnnFileInput {
            path: "data/a.lance",
            bytes: &large,
        },
        ann_files::AnnFileInput {
            path: "data/b.lance",
            bytes: &large,
        },
    ];
    let mut work = ann_budget()?;
    let fileset = ann_files::install_fileset(
        &mut AttemptContext {
            attempt: attempt(),
            objects: &spy,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
        file_batch_seal(),
        &files,
    )?;
    check_eq!(spy.batch.load(AtomicOrdering::SeqCst), 0);
    check_eq!(spy.scalar.load(AtomicOrdering::SeqCst), 3);
    check_eq!(
        ann_files::load_fileset(&spy, fileset, &mut work)?.total_length,
        1_400_000
    );
    Ok(())
}

#[test]
fn ann_fileset_batch_failure_and_cancel_do_not_return_descriptor_or_leak_memory() -> TestResult {
    for fault in [
        FileInstallFault::FailAfterBatch,
        FileInstallFault::CancelAfterBatch,
        FileInstallFault::FailBeforeDescriptor,
    ] {
        let store = MemoryStore::default();
        let spy = file_install_spy(&store, fault);
        let maps = GraphMap::new(&store);
        let custody = FileBatchCustody::default();
        let files = [
            ann_files::AnnFileInput {
                path: "data/a.lance",
                bytes: b"a",
            },
            ann_files::AnnFileInput {
                path: "data/b.lance",
                bytes: b"b",
            },
        ];
        let memory = Arc::new(QueryMemory::default());
        let mut work = ann_budget()?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
            Arc::clone(&memory),
        )))));
        let result = ann_files::install_fileset(
            &mut AttemptContext {
                attempt: attempt(),
                objects: &spy,
                maps: &maps,
                custody: &custody,
                scratch: &NoScratch,
                budget: &mut work,
            },
            file_batch_seal(),
            &files,
        );
        check!(matches!(
            (fault, result),
            (
                FileInstallFault::FailAfterBatch | FileInstallFault::FailBeforeDescriptor,
                Err(IndexError::Storage(_))
            ) | (
                FileInstallFault::CancelAfterBatch,
                Err(IndexError::Cancelled)
            )
        ));
        check_eq!(spy.batch.load(AtomicOrdering::SeqCst), 1);
        check_eq!(spy.scalar.load(AtomicOrdering::SeqCst), 0);
        check_eq!(custody.reserves.load(AtomicOrdering::SeqCst), 1);
        check_eq!(
            custody.confirms.load(AtomicOrdering::SeqCst),
            u64::from(!matches!(fault, FileInstallFault::FailBeforeDescriptor))
        );
        let (stored_objects, only_chunks) = {
            let objects = store
                .0
                .lock()
                .map_err(|error| IndexError::Storage(format!("ANN test object lock: {error}")))?;
            (
                objects.len(),
                objects
                    .values()
                    .all(|object| object.registration.children.is_empty()),
            )
        };
        check_eq!(
            stored_objects,
            if matches!(fault, FileInstallFault::FailBeforeDescriptor) {
                2
            } else {
                3
            }
        );
        if matches!(fault, FileInstallFault::FailBeforeDescriptor) {
            check!(only_chunks);
        }
        drop(work);
        check_query_memory_released(&memory)?;
    }
    Ok(())
}

// Fail if the non-seal owner path opens either authenticated input a second
// time. All other test-object behavior, including install custody, is real.
struct SingleReadMemberObjects<'a> {
    inner: &'a MemoryStore,
    member: ObjectRef,
    artifact: ObjectRef,
    member_reads: AtomicU64,
    artifact_reads: AtomicU64,
}

impl ImmutableObjectPort for SingleReadMemberObjects<'_> {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        self.inner.install_batch(inputs, sink, budget)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        let counter = if reference == self.member {
            Some(&self.member_reads)
        } else if reference == self.artifact {
            Some(&self.artifact_reads)
        } else {
            None
        };
        if counter.is_some_and(|reads| reads.fetch_add(1, AtomicOrdering::SeqCst) != 0) {
            return Err(IndexError::BackendViolation(
                "non-seal owner reopened an authenticated member or artifact".into(),
            ));
        }
        self.inner.get(reference, budget)
    }
}

#[test]
fn native_ann_nonseal_member_is_authenticated_once_and_checks_children() -> TestResult {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let selected_owner = owner(37);
    let (source, payload) = semantic_source(&store, "src/once.rs", b"one semantic owner")?;
    let (key, coverage_ref) = semantic_owner_vector_for_owner(
        &store,
        &spec,
        &recipe,
        source,
        payload,
        selected_owner,
        vec![1.0, 0.0],
    )?;
    let owners = maps.seed(BTreeMap::from([(key, coverage_ref)]))?;
    let mut work = ann_budget()?;
    let coverage_object = store.get(coverage_ref, &mut work)?;
    let coverage: OwnerCoverage = serde_json::from_slice(&coverage_object.payload)?;
    let member = coverage.searchable.ok_or("ANN member missing")?;
    let member_object = store.get(member, &mut work)?;
    let binding: OwnerMemberBinding = serde_json::from_slice(&member_object.payload)?;
    let objects = SingleReadMemberObjects {
        inner: &store,
        member,
        artifact: binding.artifact,
        member_reads: AtomicU64::new(0),
        artifact_reads: AtomicU64::new(0),
    };
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &objects,
        maps: &maps,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    };
    let searchable = crate::run_blocking(
        &runtime,
        ann_materialization::prepare(
            &spec,
            &recipe,
            None,
            owners,
            Some((selected_owner, member)),
            None,
            &mut context,
        ),
    )?
    .install(&mut context)?
    .ok_or("ANN directory missing")?;
    let directory = ann_directory::AnnDirectory::load(
        searchable,
        recipe.fingerprint(),
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    check_eq!(directory.delta_owners, 1);
    check_eq!(directory.delta_rows, 1);
    check_eq!(objects.member_reads.load(AtomicOrdering::SeqCst), 1);
    check_eq!(objects.artifact_reads.load(AtomicOrdering::SeqCst), 1);

    let malformed = store.insert(member_object.payload.clone(), vec![binding.artifact])?;
    let refused = crate::run_blocking(
        &runtime,
        ann_materialization::prepare(
            &spec,
            &recipe,
            None,
            owners,
            Some((selected_owner, malformed)),
            None,
            &mut AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &maps,
                custody: &custody,
                scratch: &NoScratch,
                budget: &mut work,
            },
        ),
    );
    check!(matches!(refused, Err(IndexError::CorruptObject(_))));
    Ok(())
}

// Observe the existing GraphMap fixture's page contract without replacing its
// lookup, COW, custody, or budget behavior.
struct RecordingMap<'a, 'b> {
    inner: &'a GraphMap<'b>,
    members: MapRoot,
    overfull_first: bool,
    scans: Mutex<Vec<MemberScan>>,
    applies: Mutex<Vec<(MapRoot, Vec<MapEdit>)>>,
}

impl<'a, 'b> RecordingMap<'a, 'b> {
    fn new(inner: &'a GraphMap<'b>, members: MapRoot, overfull_first: bool) -> Self {
        Self {
            inner,
            members,
            overfull_first,
            scans: Mutex::new(Vec::new()),
            applies: Mutex::new(Vec::new()),
        }
    }

    fn take_member_scans(&self) -> TestResult<Vec<MemberScan>> {
        Ok(std::mem::take(
            &mut *self
                .scans
                .lock()
                .map_err(|_error| "ANN member scan log poisoned")?,
        ))
    }

    fn take_applies(&self) -> TestResult<Vec<(MapRoot, Vec<MapEdit>)>> {
        Ok(std::mem::take(
            &mut *self
                .applies
                .lock()
                .map_err(|_error| "ANN map apply log poisoned")?,
        ))
    }
}

impl PersistentMapPort for RecordingMap<'_, '_> {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        self.inner.lookup(root, key, budget)
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        let actual_limit = if self.overfull_first && root == self.members && limit == 1 {
            2
        } else {
            limit
        };
        let page = self.inner.scan(root, after, actual_limit, budget)?;
        if root == self.members {
            self.scans
                .lock()
                .map_err(|_error| IndexError::Storage("ANN member scan log poisoned".into()))?
                .push((
                    after.map(<[u8]>::to_vec),
                    limit,
                    page.entries.len(),
                    page.has_more,
                ));
        }
        Ok(page)
    }

    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        self.applies
            .lock()
            .map_err(|_error| IndexError::Storage("ANN map apply log poisoned".into()))?
            .push((base, edits.to_vec()));
        self.inner.apply(base, edits, sink, budget)
    }

    fn apply_batch(
        &self,
        requests: &[MapBatchRequest],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        self.inner.apply_batch(requests, sink, budget)
    }

    fn apply_stream(
        &self,
        bases: &[MapRoot],
        edits: &mut dyn MapEditCursor,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        self.inner.apply_stream(bases, edits, sink, budget)
    }
}

const INSTALL_TRACE_EVENT_CAP: usize = 8;

#[derive(Debug, Default)]
struct InstallTraceClock(AtomicU64);

impl DiagnosticClockV1 for InstallTraceClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, AtomicOrdering::Relaxed)
    }
}

#[derive(Debug)]
struct InstallTraceEvents {
    rows: Mutex<Vec<DiagnosticEmitV1>>,
    rejected: AtomicU64,
}

impl InstallTraceEvents {
    fn new() -> TestResult<Self> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(INSTALL_TRACE_EVENT_CAP)?;
        Ok(Self {
            rows: Mutex::new(rows),
            rejected: AtomicU64::new(0),
        })
    }
}

impl DiagnosticEventSinkV1 for InstallTraceEvents {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        if let Ok(mut rows) = self.rows.try_lock()
            && rows.len() < INSTALL_TRACE_EVENT_CAP
        {
            rows.push(event);
            true
        } else {
            let _prior = self.rejected.fetch_add(1, AtomicOrdering::Relaxed);
            false
        }
    }
}

#[derive(Clone, Copy)]
enum InstallTraceCase {
    Valid,
    InvalidFiles,
    ExhaustedBytes,
}

struct AnnTraceObservation {
    result: Result<ObjectRef, IndexError>,
    usage: WorkUsage,
    clock_calls: u64,
    events: Vec<DiagnosticEmitV1>,
    rejected: u64,
}

fn observe_fileset_install(
    case: InstallTraceCase,
    record: bool,
) -> TestResult<AnnTraceObservation> {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let clock = Arc::new(InstallTraceClock::default());
    let sink = Arc::new(InstallTraceEvents::new()?);
    let capture =
        DiagnosticContextV1::new(DiagnosticCaptureIdV1([61; 16]), clock.clone(), sink.clone());
    let root = if record {
        Some(capture.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))?)
    } else {
        None
    };
    let diagnostic = root.as_ref().map(|root| capture.child_context(root));
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let request = match diagnostic {
        Some(diagnostic) => request.with_diagnostic_context(diagnostic),
        None => request,
    };
    let mut budget = WorkBudget::with_request(
        WorkLimits {
            reads: 16,
            writes: 16,
            bytes: if matches!(case, InstallTraceCase::ExhaustedBytes) {
                0
            } else {
                1_000_000
            },
            mutations: 16,
        },
        request,
    );
    let seal = ann_lance::RegionSeal {
        generation: 1,
        recipe: Digest([9; 32]),
        dimension: 2,
        rows: 1,
        rows_digest: Digest([8; 32]),
        vector_index: VectorIndexSealV1 {
            mode: "exact".into(),
            library: "lancedb".into(),
            library_version: "test".into(),
            index_min_rows: 2,
            ann: None,
        },
    };
    let valid_files = [ann_files::AnnFileInput {
        path: "data/part-1.lance",
        bytes: b"abc",
    }];
    let files = if matches!(case, InstallTraceCase::InvalidFiles) {
        &[][..]
    } else {
        &valid_files[..]
    };
    let result = ann_files::install_fileset(
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        },
        seal,
        files,
    );
    if let Some(root) = root {
        root.finish(DiagnosticTerminalV1::from_result(&result))?;
    }
    let events = sink
        .rows
        .lock()
        .map_err(|_error| "ANN install trace sink poisoned")?
        .clone();
    Ok(AnnTraceObservation {
        result,
        usage: budget.usage(),
        clock_calls: clock.0.load(AtomicOrdering::Relaxed),
        events,
        rejected: sink.rejected.load(AtomicOrdering::Relaxed),
    })
}

#[test]
fn fileset_install_trace_has_balanced_terminal_without_changing_result_or_work() -> TestResult {
    for case in [
        InstallTraceCase::Valid,
        InstallTraceCase::InvalidFiles,
        InstallTraceCase::ExhaustedBytes,
    ] {
        let off = observe_fileset_install(case, false)?;
        let record = observe_fileset_install(case, true)?;
        check_eq!(record.result, off.result);
        check_eq!(record.usage, off.usage);
        check_eq!(off.clock_calls, 0);
        check!(off.events.is_empty());
        check_eq!(off.rejected, 0);
        check_eq!(record.rejected, 0);
        check!(record.clock_calls > 0);
        let expected_event_count = if matches!(case, InstallTraceCase::Valid) {
            5
        } else {
            4
        };
        check_eq!(record.events.len(), expected_event_count);
        check!(!record.events.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::Health { .. }
                | DiagnosticEventKindV1::SpanEnd {
                    outcome: DiagnosticTerminalV1::Aborted,
                    ..
                }
        )));
        let seals: Vec<_> = record
            .events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id: Some(_),
                    stage: DiagnosticStageV1::SemanticSeal,
                    ..
                } = event.kind
                {
                    Some(span_id)
                } else {
                    None
                }
            })
            .collect();
        check_eq!(seals.len(), 1);
        let seal_span = *seals.first().ok_or("missing ANN install seal span")?;
        let region_observations: Vec<_> = record
            .events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::AnnRegionObservation {
                    span_id,
                    observation,
                } = event.kind
                {
                    Some((span_id, observation))
                } else {
                    None
                }
            })
            .collect();
        if matches!(case, InstallTraceCase::Valid) {
            check_eq!(region_observations.len(), 1);
            let (span_id, observation) = *region_observations
                .first()
                .ok_or("missing installed ANN region observation")?;
            let reference = record.result.as_ref().map_err(|error| {
                format!("successful ANN install lacked a file-set reference: {error}")
            })?;
            check_eq!(span_id, seal_span);
            check_eq!(observation.phase, DiagnosticAnnRegionPhaseV1::Installed);
            check_eq!(observation.fileset_ref, reference.0.0);
            check_eq!(observation.descriptor_ref, None);
            check_eq!(observation.ordinal, 0);
            check_eq!(observation.rows, 1);
            check_eq!(observation.index_kind, DiagnosticAnnIndexKindV1::Flat);
            check_eq!(observation.files, 1);
            check_eq!(observation.chunks, 1);
        } else {
            check!(region_observations.is_empty());
        }
        let terminals: Vec<_> = record
            .events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::SpanEnd { span_id, outcome } = event.kind
                    && span_id == seal_span
                {
                    Some(outcome)
                } else {
                    None
                }
            })
            .collect();
        let expected = match case {
            InstallTraceCase::Valid => {
                check!(record.result.is_ok());
                check_eq!(record.usage.writes, 2);
                check_eq!(record.usage.reads, 2);
                check!(record.usage.bytes > 3);
                DiagnosticTerminalV1::Ok
            }
            InstallTraceCase::InvalidFiles => {
                check!(matches!(record.result, Err(IndexError::InvalidInput(_))));
                check_eq!(record.usage, WorkUsage::default());
                DiagnosticTerminalV1::Error { code: 1 }
            }
            InstallTraceCase::ExhaustedBytes => {
                check_eq!(record.result, Err(IndexError::ResourceLimited));
                check_eq!(record.usage, WorkUsage::default());
                DiagnosticTerminalV1::Error { code: 1 }
            }
        };
        check_eq!(terminals, vec![expected]);
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum CarryTraceCase {
    Valid,
    InvalidRecipe,
    ExhaustedBytes,
}

fn observe_carry(case: CarryTraceCase, record: bool) -> TestResult<AnnTraceObservation> {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let selected = selected_owner(&store, &instance(&recipe), &recipe, 1, b"X", [1.0, 0.0])?;
    let (_, owners) = owner_roots(&maps, &[selected])?;
    let delta = maps.seed(BTreeMap::from([(
        owner_map_key(selected.owner).to_vec(),
        selected.member,
    )]))?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    directory.delta = delta;
    directory.delta_rows = 1;
    directory.delta_owners = 1;
    if matches!(case, CarryTraceCase::InvalidRecipe) {
        directory.recipe = Digest([0; 32]);
    }
    let clock = Arc::new(InstallTraceClock::default());
    let sink = Arc::new(InstallTraceEvents::new()?);
    let capture =
        DiagnosticContextV1::new(DiagnosticCaptureIdV1([62; 16]), clock.clone(), sink.clone());
    let root = if record {
        Some(capture.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))?)
    } else {
        None
    };
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let request = match root.as_ref() {
        Some(root) => request.with_diagnostic_context(capture.child_context(root)),
        None => request,
    };
    let mut budget = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000,
            writes: 1_000,
            bytes: if matches!(case, CarryTraceCase::ExhaustedBytes) {
                0
            } else {
                1_000_000
            },
            mutations: 1_000,
        },
        request,
    );
    let result = directory.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        },
    );
    if let Ok(members) = &result {
        // The one selected member is already the exact base. Carry must
        // preserve its root, not merely agree between Off and Record.
        check_eq!(members.root, delta);
        check_eq!(members.owners, 1);
        check_eq!(members.rows, 1);
    }
    let result = result.and_then(|members| members.root.0.ok_or(IndexError::ResourceLimited));
    if let Some(root) = root {
        root.finish(DiagnosticTerminalV1::from_result(&result))?;
    }
    let events = sink
        .rows
        .lock()
        .map_err(|_error| "ANN carry trace sink poisoned")?
        .clone();
    Ok(AnnTraceObservation {
        result,
        usage: budget.usage(),
        clock_calls: clock.0.load(AtomicOrdering::Relaxed),
        events,
        rejected: sink.rejected.load(AtomicOrdering::Relaxed),
    })
}

#[test]
fn carry_trace_preserves_root_refusals_and_work_with_balanced_terminals() -> TestResult {
    for case in [
        CarryTraceCase::Valid,
        CarryTraceCase::InvalidRecipe,
        CarryTraceCase::ExhaustedBytes,
    ] {
        let off = observe_carry(case, false)?;
        let record = observe_carry(case, true)?;
        check_eq!(record.result, off.result);
        check_eq!(record.usage, off.usage);
        check_eq!(off.clock_calls, 0);
        check!(off.events.is_empty());
        check_eq!(off.rejected, 0);
        check_eq!(record.rejected, 0);
        let expected = match case {
            CarryTraceCase::Valid => {
                check!(record.result.is_ok());
                DiagnosticTerminalV1::Ok
            }
            CarryTraceCase::InvalidRecipe => {
                check!(matches!(record.result, Err(IndexError::InvalidInput(_))));
                DiagnosticTerminalV1::Error { code: 1 }
            }
            CarryTraceCase::ExhaustedBytes => {
                check_eq!(record.result, Err(IndexError::ResourceLimited));
                DiagnosticTerminalV1::Error { code: 1 }
            }
        };
        let spans: Vec<_> = record
            .events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id: Some(_),
                    stage: DiagnosticStageV1::Compaction,
                    ..
                } = event.kind
                {
                    Some(span_id)
                } else {
                    None
                }
            })
            .collect();
        check_eq!(spans.len(), 1);
        let span = *spans.first().ok_or("missing ANN carry span")?;
        let terminals: Vec<_> = record
            .events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::SpanEnd { span_id, outcome } = event.kind
                    && span_id == span
                {
                    Some(outcome)
                } else {
                    None
                }
            })
            .collect();
        check_eq!(terminals, vec![expected]);
        check_eq!(record.events.len(), 4);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct SelectedOwner {
    owner: OwnerKey,
    source: ObjectRef,
    member: ObjectRef,
    coverage: ObjectRef,
    input: OwnerInputBinding,
    commitment: Digest,
    vector: [f32; 2],
}

fn selected_owner(
    store: &MemoryStore,
    spec: &InstanceSpec,
    recipe: &EmbeddingRecipe,
    id: u8,
    source_bytes: &[u8],
    vector: [f32; 2],
) -> TestResult<SelectedOwner> {
    selected_owner_for_key(
        store,
        spec,
        recipe,
        owner(id),
        &format!("src/{id}.rs"),
        source_bytes,
        vector,
    )
}

fn selected_owner_for_key(
    store: &MemoryStore,
    spec: &InstanceSpec,
    recipe: &EmbeddingRecipe,
    selected: OwnerKey,
    path: &str,
    source_bytes: &[u8],
    vector: [f32; 2],
) -> TestResult<SelectedOwner> {
    let (source, payload) = semantic_source(store, path, source_bytes)?;
    let (_, coverage_ref) = semantic_owner_vector_for_owner(
        store,
        spec,
        recipe,
        source,
        payload,
        selected,
        vector.to_vec(),
    )?;
    let mut budget = ann_budget()?;
    let coverage_object = store.get(coverage_ref, &mut budget)?;
    let coverage: OwnerCoverage = serde_json::from_slice(&coverage_object.payload)?;
    let member = coverage.searchable.ok_or("selected ANN member absent")?;
    let member_object = store.get(member, &mut budget)?;
    let binding: OwnerMemberBinding = serde_json::from_slice(&member_object.payload)?;
    check_eq!(binding.owner, selected);
    check_eq!(binding.input.source, Some(source));
    Ok(SelectedOwner {
        owner: selected,
        source,
        member,
        coverage: coverage_ref,
        input: binding.input,
        commitment: binding.logical_commitment,
        vector,
    })
}

fn owner_roots(maps: &GraphMap<'_>, selected: &[SelectedOwner]) -> TestResult<(MapRoot, MapRoot)> {
    let mut sources = BTreeMap::new();
    let mut coverage = BTreeMap::new();
    for item in selected {
        check_eq!(
            sources.insert(owner_map_key(item.owner).to_vec(), item.source),
            None
        );
        check_eq!(
            coverage.insert(owner_map_key(item.owner).to_vec(), item.coverage),
            None
        );
    }
    Ok((maps.seed(sources)?, maps.seed(coverage)?))
}

pub(super) fn binding(
    spec: &InstanceSpec,
    recipe: &EmbeddingRecipe,
    sources: MapRoot,
    owners: MapRoot,
    directory: ObjectRef,
) -> CoverageBinding {
    let desired = quanta_index_core::index::InputBinding {
        root: sources,
        schema: spec.schema,
        policy: recipe.fingerprint(),
        context: Digest([0; 32]),
    };
    CoverageBinding {
        instance: spec.instance,
        desired,
        covered: Some(desired),
        searchable: Some(directory),
        status: CoverageStatus::Complete,
        owners,
    }
}

fn seal_one(
    runtime: &tokio::runtime::Runtime,
    store: &MemoryStore,
    maps: &GraphMap<'_>,
    custody: &TestCustody,
    recipe: &EmbeddingRecipe,
    directory: &mut ann_directory::AnnDirectory,
    item: &SelectedOwner,
    budget: &mut WorkBudget,
) -> TestResult<(ObjectRef, ObjectRef, u64)> {
    let members = maps.seed(BTreeMap::from([(
        owner_map_key(item.owner).to_vec(),
        item.member,
    )]))?;
    let session = AnnLanceSession::new(AnnStoreLimits::default(), budget)?;
    let connection = crate::run_blocking(runtime, session.connect())?;
    let generation = directory
        .next_region
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    let rows = [ann_lance::RegionRow {
        owner: item.owner,
        member: item.commitment,
        vector_id: b"whole-file",
        input: item.input,
        vector: &item.vector,
    }];
    let (table, seal) = crate::run_blocking(
        runtime,
        ann_lance::build_region(
            &connection,
            "region",
            &rows,
            recipe.fingerprint(),
            generation,
            2,
            budget,
        ),
    )?;
    drop(table);
    drop(connection);
    let files = crate::run_blocking(runtime, session.freeze_and_snapshot())?;
    let inputs: Vec<_> = files
        .iter()
        .map(|file| ann_files::AnnFileInput {
            path: &file.path,
            bytes: &file.bytes,
        })
        .collect();
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: store,
        maps,
        custody,
        scratch: &NoScratch,
        budget,
    };
    let fileset = ann_files::install_fileset(&mut context, seal, &inputs)?;
    let descriptor = ann_directory::RegionDescriptor {
        fileset,
        members,
        center: item.vector.to_vec(),
        rows: 1,
    };
    let region = directory.append_region(&descriptor, &mut context)?;
    Ok((region, fileset, generation))
}

fn seal_many(
    runtime: &tokio::runtime::Runtime,
    store: &MemoryStore,
    maps: &GraphMap<'_>,
    custody: &TestCustody,
    recipe: &EmbeddingRecipe,
    directory: &mut ann_directory::AnnDirectory,
    selected: &[SelectedOwner],
    budget: &mut WorkBudget,
) -> TestResult<MapRoot> {
    let members = maps.seed(
        selected
            .iter()
            .map(|item| (owner_map_key(item.owner).to_vec(), item.member))
            .collect(),
    )?;
    let rows: Vec<_> = selected
        .iter()
        .map(|item| ann_lance::RegionRow {
            owner: item.owner,
            member: item.commitment,
            vector_id: b"whole-file",
            input: item.input,
            vector: &item.vector,
        })
        .collect();
    let session = AnnLanceSession::new(AnnStoreLimits::default(), budget)?;
    let connection = crate::run_blocking(runtime, session.connect())?;
    let generation = directory
        .next_region
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    let (table, seal) = crate::run_blocking(
        runtime,
        ann_lance::build_region(
            &connection,
            "region",
            &rows,
            recipe.fingerprint(),
            generation,
            2,
            budget,
        ),
    )?;
    drop(table);
    drop(connection);
    let files = crate::run_blocking(runtime, session.freeze_and_snapshot())?;
    let inputs: Vec<_> = files
        .iter()
        .map(|file| ann_files::AnnFileInput {
            path: &file.path,
            bytes: &file.bytes,
        })
        .collect();
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: store,
        maps,
        custody,
        scratch: &NoScratch,
        budget,
    };
    let fileset = ann_files::install_fileset(&mut context, seal, &inputs)?;
    let descriptor = ann_directory::RegionDescriptor {
        fileset,
        members,
        center: vec![1.0, 0.0],
        rows: u32::try_from(selected.len())?,
    };
    let _region = directory.append_region(&descriptor, &mut context)?;
    Ok(members)
}

#[test]
fn retirement_probes_one_live_member_then_preserves_bounded_dead_region_walk() -> TestResult {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let mut budget = ann_budget()?;
    let mut selected = Vec::new();
    for id in 0..66_u8 {
        selected.push(selected_owner(
            &store,
            &spec,
            &recipe,
            id,
            format!("selected source {id}").as_bytes(),
            [1.0, 0.0],
        )?);
    }
    selected.sort_unstable_by_key(|item| item.owner);
    let first = *selected.first().ok_or("ANN first selected owner absent")?;
    let second = *selected.get(1).ok_or("ANN second selected owner absent")?;
    let sixty_fifth = *selected.get(64).ok_or("ANN selected owner 64 absent")?;
    let (_, all_owners) = owner_roots(&maps, &selected)?;
    let (_, first_owner) = owner_roots(&maps, &[first])?;
    let (_, second_owner) = owner_roots(&maps, &[second])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), all_owners);
    let members = seal_many(
        &runtime,
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &selected,
        &mut budget,
    )?;
    let original = directory;
    let observed = RecordingMap::new(&maps, members, false);

    directory.owners = first_owner;
    directory.retire_obsolete(&mut AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &observed,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut budget,
    })?;
    check_eq!(directory.region_count, 1);
    check_eq!(observed.take_member_scans()?, vec![(None, 1, 1, true)]);

    directory.owners = second_owner;
    directory.retire_obsolete(&mut AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &observed,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut budget,
    })?;
    check_eq!(directory.region_count, 1);
    check_eq!(
        observed.take_member_scans()?,
        vec![
            (None, 1, 1, true),
            (Some(owner_map_key(first.owner).to_vec()), 64, 64, true),
        ]
    );

    directory.owners = MapRoot(None);
    directory.retire_obsolete(&mut AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &observed,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut budget,
    })?;
    check_eq!(directory.region_count, 0);
    check_eq!(directory.next_region, 1);
    check_eq!(
        observed.take_member_scans()?,
        vec![
            (None, 1, 1, true),
            (Some(owner_map_key(first.owner).to_vec()), 64, 64, true),
            (
                Some(owner_map_key(sixty_fifth.owner).to_vec()),
                64,
                1,
                false
            ),
        ]
    );
    check_eq!(
        live_region_ids(&store, &maps, &original, &mut budget)?,
        vec![0]
    );

    let forged_map = RecordingMap::new(&maps, members, true);
    let mut forged = original;
    forged.owners = first_owner;
    let before = forged;
    let refusal = forged.retire_obsolete(&mut AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &forged_map,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut budget,
    });
    check!(matches!(refusal, Err(IndexError::BackendViolation(_))));
    check_eq!(forged, before);
    check_eq!(forged_map.take_member_scans()?, vec![(None, 1, 2, true)]);
    Ok(())
}

fn file_chunks(
    store: &MemoryStore,
    fileset: ObjectRef,
    budget: &mut WorkBudget,
) -> TestResult<Vec<ObjectRef>> {
    let files = ann_files::load_fileset(store, fileset, budget)?;
    let chunks: Vec<_> = files
        .files
        .iter()
        .flat_map(|file| &file.chunks)
        .map(|chunk| chunk.object.registration.reference)
        .collect();
    check!(!chunks.is_empty());
    Ok(chunks)
}

fn save_directory(
    store: &MemoryStore,
    maps: &GraphMap<'_>,
    custody: &TestCustody,
    directory: &ann_directory::AnnDirectory,
    budget: &mut WorkBudget,
) -> TestResult<ObjectRef> {
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: store,
        maps,
        custody,
        scratch: &NoScratch,
        budget,
    };
    Ok(directory.save(&mut context)?)
}

fn live_region_ids(
    store: &MemoryStore,
    maps: &GraphMap<'_>,
    directory: &ann_directory::AnnDirectory,
    budget: &mut WorkBudget,
) -> TestResult<Vec<u64>> {
    let page = directory.region_page(
        None,
        64,
        &mut ReadContext {
            objects: store,
            maps,
            budget,
        },
    )?;
    check!(!page.has_more);
    let mut ids = Vec::new();
    for (key, _) in &page.entries {
        let raw: [u8; 8] = key.as_slice().try_into()?;
        ids.push(u64::from_be_bytes(raw));
    }
    Ok(ids)
}

#[test]
fn legacy_directory_requires_rebuild_and_active_count_must_match_map() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let recipe = recipe();
    let mut budget = ann_budget()?;

    // This is a persisted format-1 header, not a format-2 value rewritten by
    // the current serializer. The version gate must precede shape decoding.
    let legacy = store.insert(br#"{"format":1}"#.to_vec(), Vec::new())?;
    let legacy_error = ann_directory::AnnDirectory::load(
        legacy,
        recipe.fingerprint(),
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut budget,
        },
    );
    check!(matches!(legacy_error, Err(IndexError::RebuildRequired)));

    let mut invalid = ann_directory::AnnDirectory::empty(recipe.fingerprint(), MapRoot(None));
    invalid.region_count = 1;
    let malformed = store.insert(serde_json::to_vec(&invalid)?, Vec::new())?;
    let malformed_error = ann_directory::AnnDirectory::load(
        malformed,
        recipe.fingerprint(),
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut budget,
        },
    );
    check!(matches!(malformed_error, Err(IndexError::CorruptObject(_))));
    Ok(())
}

#[test]
fn delta_retirement_preserves_pending_member_and_old_view() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let mut budget = ann_budget()?;
    let first = selected_owner(&store, &spec, &recipe, 1, b"X first", [1.0, 0.0])?;
    let second = selected_owner(&store, &spec, &recipe, 2, b"Y second", [0.0, 1.0])?;
    let third = selected_owner(&store, &spec, &recipe, 3, b"X third", [1.0, 0.0])?;
    let (_, old_owners) = owner_roots(&maps, &[first, second, third])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), old_owners);
    {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        };
        for item in [first, second, third] {
            directory.put_delta(item.owner, item.member, &recipe, &mut context)?;
        }
    }
    check_eq!(directory.delta_owners, 3);
    check_eq!(directory.delta_rows, 3);
    let old_directory = save_directory(&store, &maps, &custody, &directory, &mut budget)?;

    let desired_first = selected_owner(&store, &spec, &recipe, 1, b"Y pending", [0.0, 1.0])?;
    let replacement_second =
        selected_owner(&store, &spec, &recipe, 2, b"X replacement", [1.0, 0.0])?;
    let pending = OwnerCoverage {
        owner: first.owner,
        desired: desired_first.input,
        covered: Some(first.input),
        searchable: Some(first.member),
    };
    let pending_children = ObjectChildren::from_references([
        desired_first.input.source,
        None,
        Some(first.member),
        None,
    ]);
    let pending_ref = store.insert(
        serde_json::to_vec(&pending)?,
        pending_children.as_slice().to_vec(),
    )?;
    let selected_owners = maps.seed(BTreeMap::from([
        (owner_map_key(first.owner).to_vec(), pending_ref),
        (
            owner_map_key(second.owner).to_vec(),
            replacement_second.coverage,
        ),
    ]))?;
    directory.owners = selected_owners;
    let before_prune = directory;

    let mut cancelled_budget = ann_budget()?;
    cancelled_budget.request().cancel_handle().cancel();
    let cancelled = {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut cancelled_budget,
        };
        directory.retire_delta(&recipe, &mut context)
    };
    check!(matches!(cancelled, Err(IndexError::Cancelled)));
    check_eq!(directory, before_prune);

    {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        };
        directory.retire_delta(&recipe, &mut context)?;
    }
    check_eq!(directory.delta_owners, 1);
    check_eq!(directory.delta_rows, 1);
    check_eq!(directory.owners, selected_owners);
    check_eq!(
        maps.lookup_object(directory.delta, &owner_map_key(first.owner), &mut budget)?,
        Some(first.member)
    );
    for removed in [second, third] {
        check_eq!(
            maps.lookup_object(directory.delta, &owner_map_key(removed.owner), &mut budget)?,
            None
        );
        check!(graph_reaches(&store, old_directory, removed.member)?);
    }
    check_eq!(
        maps.lookup_object(directory.owners, &owner_map_key(first.owner), &mut budget)?,
        Some(pending_ref)
    );
    check_eq!(
        maps.lookup_object(directory.owners, &owner_map_key(second.owner), &mut budget)?,
        Some(replacement_second.coverage)
    );
    check_eq!(
        maps.lookup_object(directory.owners, &owner_map_key(third.owner), &mut budget)?,
        None
    );
    let new_directory = save_directory(&store, &maps, &custody, &directory, &mut budget)?;
    check!(graph_reaches(&store, new_directory, first.member)?);
    check!(!graph_reaches(&store, new_directory, second.member)?);
    check!(!graph_reaches(&store, new_directory, third.member)?);
    Ok(())
}

#[test]
fn obsolete_and_absent_regions_retire_but_pending_searchable_and_old_view_survive() -> TestResult {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let mut budget = ann_budget()?;
    let old = selected_owner(&store, &spec, &recipe, 1, b"X old", [1.0, 0.0])?;
    let (old_sources, old_owners) = owner_roots(&maps, &[old])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), old_owners);
    let (_, old_fileset, first_generation) = seal_one(
        &runtime,
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &old,
        &mut budget,
    )?;
    check_eq!(first_generation, 1);
    let old_chunks = file_chunks(&store, old_fileset, &mut budget)?;
    let old_directory = save_directory(&store, &maps, &custody, &directory, &mut budget)?;
    let old_binding = binding(&spec, &recipe, old_sources, old_owners, old_directory);

    let replacement = selected_owner(&store, &spec, &recipe, 1, b"Y new", [0.0, 1.0])?;
    let (new_sources, new_owners) = owner_roots(&maps, &[replacement])?;
    // Pending coverage still selects the old searchable member. A changed
    // desired input alone must never license retirement.
    let pending = OwnerCoverage {
        owner: old.owner,
        desired: replacement.input,
        covered: Some(old.input),
        searchable: Some(old.member),
    };
    let pending_children =
        ObjectChildren::from_references([replacement.input.source, None, Some(old.member), None]);
    let pending_ref = store.insert(
        serde_json::to_vec(&pending)?,
        pending_children.as_slice().to_vec(),
    )?;
    let pending_owners = maps.seed(BTreeMap::from([(
        owner_map_key(old.owner).to_vec(),
        pending_ref,
    )]))?;
    let mut pending_directory = directory;
    pending_directory.owners = pending_owners;
    {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        };
        pending_directory.retire_obsolete(&mut context)?;
    }
    check_eq!(pending_directory.region_count, 1);
    check_eq!(
        live_region_ids(&store, &maps, &pending_directory, &mut budget)?,
        vec![0]
    );

    directory.owners = new_owners;
    let (_, new_fileset, second_generation) = seal_one(
        &runtime,
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &replacement,
        &mut budget,
    )?;
    check_eq!(second_generation, 2);
    {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        };
        directory.retire_obsolete(&mut context)?;
    }
    check_eq!(directory.region_count, 1);
    check_eq!(directory.next_region, 2);
    check_eq!(
        live_region_ids(&store, &maps, &directory, &mut budget)?,
        vec![1]
    );
    let new_directory = save_directory(&store, &maps, &custody, &directory, &mut budget)?;
    let new_binding = binding(&spec, &recipe, new_sources, new_owners, new_directory);
    let new_chunks = file_chunks(&store, new_fileset, &mut budget)?;
    let old_unique_chunk = *old_chunks
        .iter()
        .find(|chunk| !new_chunks.contains(chunk))
        .ok_or("replacement ANN files shared every old data chunk")?;
    check!(graph_reaches(&store, old_directory, old_unique_chunk)?);
    check!(!graph_reaches(&store, new_directory, old_unique_chunk)?);
    check!(graph_reaches(
        &store,
        new_directory,
        *new_chunks.first().ok_or("new ANN file chunk absent")?
    )?);
    let (old_hits, old_memory) = selected_search(
        &runtime,
        &store,
        &maps,
        &old_binding,
        old_sources,
        &spec,
        &recipe,
    )?;
    let (new_hits, new_memory) = selected_search(
        &runtime,
        &store,
        &maps,
        &new_binding,
        new_sources,
        &spec,
        &recipe,
    )?;
    check_eq!(
        old_hits.hits.first().ok_or("old ANN hit missing")?.owner,
        old.owner
    );
    check_eq!(
        old_hits
            .hits
            .first()
            .ok_or("old ANN hit missing")?
            .cosine
            .total_cmp(&1.0),
        Ordering::Equal
    );
    check_eq!(
        new_hits.hits.first().ok_or("new ANN hit missing")?.owner,
        old.owner
    );
    check_eq!(
        new_hits
            .hits
            .first()
            .ok_or("new ANN hit missing")?
            .cosine
            .total_cmp(&0.0),
        Ordering::Equal
    );
    drop(old_hits);
    drop(new_hits);
    check_query_memory_released(&old_memory)?;
    check_query_memory_released(&new_memory)?;

    let mut absent = pending_directory;
    absent.owners = MapRoot(None);
    {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        };
        absent.retire_obsolete(&mut context)?;
    }
    check_eq!(absent.region_count, 0);
    check_eq!(absent.next_region, 1);
    check!(live_region_ids(&store, &maps, &absent, &mut budget)?.is_empty());
    Ok(())
}

#[test]
fn capacity_sweep_retires_later_obsolete_without_reusing_sparse_generation() -> TestResult {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let mut budget = ann_budget()?;
    let first = selected_owner(&store, &spec, &recipe, 1, b"X first", [1.0, 0.0])?;
    let second = selected_owner(&store, &spec, &recipe, 2, b"Y second", [0.0, 1.0])?;
    let third = selected_owner(&store, &spec, &recipe, 3, b"Y third", [0.0, 1.0])?;
    let (_, all_owners) = owner_roots(&maps, &[first, second, third])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), all_owners);
    for item in [first, second, third] {
        let _sealed = seal_one(
            &runtime,
            &store,
            &maps,
            &custody,
            &recipe,
            &mut directory,
            &item,
            &mut budget,
        )?;
    }
    check_eq!(directory.region_count, 3);
    let before_objects = store.0.lock().map_err(|_poison| "ANN object lock")?.len();
    let before_regions = directory.regions;
    {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        };
        check!(matches!(
            directory.ensure_region_capacity_for_test(&mut context, 3),
            Err(IndexError::ResourceLimited)
        ));
    }
    check_eq!(directory.regions, before_regions);
    check_eq!(directory.region_count, 3);
    let observed_objects = store.0.lock().map_err(|_poison| "ANN object lock")?.len();
    check_eq!(observed_objects, before_objects);

    let (selected_sources, selected_owners) = owner_roots(&maps, &[first, second])?;
    directory.owners = selected_owners;
    {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        };
        directory.retire_obsolete(&mut context)?; // first region is live
    }
    check_eq!(directory.region_count, 3);
    {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        };
        directory.ensure_region_capacity_for_test(&mut context, 3)?;
    }
    check_eq!(directory.region_count, 2);
    check_eq!(directory.next_region, 3);
    check_eq!(
        live_region_ids(&store, &maps, &directory, &mut budget)?,
        vec![0, 1]
    );

    // A retired historical run can leave a high watermark with no active
    // region. Build genuine regions at the high ordinals so the Lance seals
    // still bind generation = ordinal + 1.
    let mut sparse = ann_directory::AnnDirectory::empty(recipe.fingerprint(), selected_owners);
    sparse.next_region = 4096;
    let (_, old_sparse_fileset, old_sparse_generation) = seal_one(
        &runtime,
        &store,
        &maps,
        &custody,
        &recipe,
        &mut sparse,
        &first,
        &mut budget,
    )?;
    check_eq!(old_sparse_generation, 4097);
    check_eq!(
        ann_files::load_fileset(&store, old_sparse_fileset, &mut budget)?
            .seal
            .generation,
        4097
    );
    {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        };
        sparse.ensure_region_capacity(&mut context)?;
    }
    let (_, new_fileset, generation) = seal_one(
        &runtime,
        &store,
        &maps,
        &custody,
        &recipe,
        &mut sparse,
        &second,
        &mut budget,
    )?;
    check_eq!(generation, 4098);
    let loaded = ann_files::load_fileset(&store, new_fileset, &mut budget)?;
    check_eq!(loaded.seal.generation, 4098);
    check_eq!(sparse.next_region, 4098);
    check_eq!(sparse.region_count, 2);
    check_eq!(
        live_region_ids(&store, &maps, &sparse, &mut budget)?,
        vec![4096, 4097]
    );
    let sparse_ref = save_directory(&store, &maps, &custody, &sparse, &mut budget)?;
    let sparse_binding = binding(
        &spec,
        &recipe,
        selected_sources,
        selected_owners,
        sparse_ref,
    );
    let (hits, memory) = selected_search(
        &runtime,
        &store,
        &maps,
        &sparse_binding,
        selected_sources,
        &spec,
        &recipe,
    )?;
    check_eq!(
        hits.hits.first().ok_or("sparse ANN hit absent")?.owner,
        first.owner
    );
    check_eq!(
        hits.hits
            .first()
            .ok_or("sparse ANN hit absent")?
            .cosine
            .total_cmp(&1.0),
        Ordering::Equal
    );
    drop(hits);
    check_query_memory_released(&memory)?;

    // A cancelled original attempt cannot save or publish a modified root.
    let mut cancelled = sparse;
    let original = save_directory(&store, &maps, &custody, &cancelled, &mut budget)?;
    let mut cancelled_budget = ann_budget()?;
    cancelled_budget.request().cancel_handle().cancel();
    let before = cancelled;
    let refusal = {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut cancelled_budget,
        };
        cancelled.retire_obsolete(&mut context)
    };
    check!(matches!(refusal, Err(IndexError::Cancelled)));
    check_eq!(cancelled, before);
    let still_selected = save_directory(&store, &maps, &custody, &before, &mut budget)?;
    check_eq!(original, still_selected);
    Ok(())
}

#[test]
fn consolidation_bounds_foreground_rows_and_preserves_the_larger_live_run() -> TestResult {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let selected: Vec<_> = (0..1024_u16)
        .map(|id| {
            selected_owner_for_key(
                &store,
                &spec,
                &recipe,
                OwnerKey {
                    producer: owner(0).producer,
                    stable_id: StableOwnerId(u128::from(id).to_be_bytes()),
                },
                &format!("carry/{id}.rs"),
                format!("carry owner {id}").as_bytes(),
                [1.0, 0.0],
            )
        })
        .collect::<TestResult<_>>()?;
    let (_, owners) = owner_roots(&maps, &selected)?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let mut setup = ann_budget()?;
    for (start, end) in [(64, 128), (128, 256), (256, 512), (512, 1024)] {
        let _members = seal_many(
            &runtime,
            &store,
            &maps,
            &custody,
            &recipe,
            &mut directory,
            selected
                .get(start..end)
                .ok_or("carry source slice absent")?,
            &mut setup,
        )?;
    }
    for item in selected.get(..64).ok_or("carry delta absent")? {
        directory.put_delta(
            item.owner,
            item.member,
            &recipe,
            &mut AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &maps,
                custody: &custody,
                scratch: &NoScratch,
                budget: &mut setup,
            },
        )?;
    }
    let baseline = directory;
    let pinned = save_directory(&store, &maps, &custody, &directory, &mut setup)?;
    let old = directory
        .region_at(
            0,
            &mut ReadContext {
                objects: &store,
                maps: &maps,
                budget: &mut setup,
            },
        )?
        .descriptor
        .fileset;
    let mut work = ann_budget()?;
    let memory = Arc::new(QueryMemory::default());
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        memory.clone(),
    )))));
    let result = directory.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
    )?;
    // A 64-row delta carries 64+128+256 into exactly 512, leaving the next
    // 512-row run live. The old fan-in-only policy instead rewrites all1024.
    check_eq!(result.rows, 512);
    check_eq!(result.owners, 512);
    check_eq!(directory.region_count, 1);
    check_eq!(
        live_region_ids(&store, &maps, &directory, &mut work)?,
        vec![3]
    );
    let expected: BTreeMap<_, _> = selected
        .get(..512)
        .ok_or("carry gold absent")?
        .iter()
        .map(|item| (owner_map_key(item.owner).to_vec(), item.member))
        .collect();
    check_eq!(maps.state(result.root)?, expected);
    let retained = directory.region_at(
        3,
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    let mut union = maps.state(retained.descriptor.members)?;
    union.extend(maps.state(result.root)?);
    let all: BTreeMap<_, _> = selected
        .iter()
        .map(|item| (owner_map_key(item.owner).to_vec(), item.member))
        .collect();
    check_eq!(union, all);
    check!(graph_reaches(&store, pinned, old)?);
    drop(retained);
    drop(result);
    check_query_memory_released(&memory)?;

    // The ordinary work cap must not shrink the original region-capacity
    // contract. Full sentinel runs are metadata-only and never selected, as
    // in the separate capacity-candidate regression below.
    let sentinel = store.insert(b"unopened full-run sentinel".to_vec(), Vec::new())?;
    let descriptor = ann_directory::RegionDescriptor {
        fileset: sentinel,
        members: baseline.delta,
        center: vec![1.0, 0.0],
        rows: 8192,
    };
    let children = ObjectChildren::from_references([Some(sentinel), baseline.delta.0, None, None]);
    let reference = store.insert(
        serde_json::to_vec(&descriptor)?,
        children.as_slice().to_vec(),
    )?;
    let mut entries = maps.state(baseline.regions)?;
    for id in 4..ann_directory::MAX_REGIONS {
        check!(
            entries
                .insert(id.to_be_bytes().to_vec(), reference)
                .is_none()
        );
    }
    let mut full = baseline;
    full.regions = maps.seed(entries)?;
    full.region_count = u32::try_from(ann_directory::MAX_REGIONS)?;
    full.next_region = ann_directory::MAX_REGIONS;
    let pressure = full.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
    )?;
    check_eq!(pressure.rows, 1024);
    check_eq!(pressure.owners, 1024);
    check_eq!(full.region_count, 4092);
    check_eq!(maps.state(pressure.root)?, all);
    drop(pressure);
    check_query_memory_released(&memory)?;
    Ok(())
}

#[test]
fn consolidation_rotates_past_full_runs_and_finds_capacity_candidate() -> TestResult {
    // Metadata-only sentinel runs cannot fit beside the delta. Their file-set
    // is deliberately unopened; the one eligible run is a real Lance file-set.
    // This tests selection/admission, not physical correctness of the sentinels.
    for (full_runs, cursor) in [(64u64, Some(63u64)), (4095, None)] {
        let runtime = test_runtime()?;
        let store = MemoryStore::default();
        let maps = GraphMap::new(&store);
        let custody = TestCustody;
        let recipe = recipe();
        let spec = instance(&recipe);
        let first = selected_owner(&store, &spec, &recipe, 1, b"X prior", [1.0, 0.0])?;
        let second = selected_owner(&store, &spec, &recipe, 2, b"Y delta", [0.0, 1.0])?;
        let (_, owners) = owner_roots(&maps, &[first, second])?;
        let members = maps.seed(BTreeMap::from([(
            owner_map_key(first.owner).to_vec(),
            first.member,
        )]))?;
        let sentinel = store.insert(b"unopened full-run sentinel".to_vec(), Vec::new())?;
        let descriptor = ann_directory::RegionDescriptor {
            fileset: sentinel,
            members,
            center: vec![1.0, 0.0],
            rows: 8192,
        };
        let children = ObjectChildren::from_references([Some(sentinel), members.0, None, None]);
        let reference = store.insert(
            serde_json::to_vec(&descriptor)?,
            children.as_slice().to_vec(),
        )?;
        let regions = maps.seed(
            (0..full_runs)
                .map(|id| (id.to_be_bytes().to_vec(), reference))
                .collect(),
        )?;
        let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
        directory.regions = regions;
        directory.region_count = u32::try_from(full_runs)?;
        directory.next_region = full_runs;
        directory.maintenance_after = cursor;
        let mut work = ann_budget()?;
        let (_region, _fileset, _generation) = seal_one(
            &runtime,
            &store,
            &maps,
            &custody,
            &recipe,
            &mut directory,
            &first,
            &mut work,
        )?;
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        directory.put_delta(second.owner, second.member, &recipe, &mut context)?;
        let selected = directory.consolidated_members(&recipe, &mut context)?;
        check_eq!(selected.owners, 2);
        check_eq!(selected.rows, 2);
        check_eq!(directory.region_count, u32::try_from(full_runs)?);
        check_eq!(
            context.maps.lookup_object(
                selected.root,
                &owner_map_key(first.owner),
                context.budget
            )?,
            Some(first.member)
        );
        check_eq!(
            context.maps.lookup_object(
                selected.root,
                &owner_map_key(second.owner),
                context.budget
            )?,
            Some(second.member)
        );
    }
    Ok(())
}

#[test]
fn consolidation_multi_page_carry_installs_one_final_member_map() -> TestResult {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let selected: Vec<_> = (0..192u8)
        .map(|id| {
            let x = (f32::from(id) + 1.0) / 193.0;
            selected_owner(
                &store,
                &spec,
                &recipe,
                id,
                format!("selected owner {id}").as_bytes(),
                [x, (1.0 - x * x).sqrt()],
            )
        })
        .collect::<TestResult<Vec<_>>>()?;
    let (_, owners) = owner_roots(&maps, &selected)?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let mut setup = ann_budget()?;
    let _first_members = seal_many(
        &runtime,
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        selected.get(..64).ok_or("first 64 owners missing")?,
        &mut setup,
    )?;
    let second_members = seal_many(
        &runtime,
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        selected.get(64..129).ok_or("next 65 owners missing")?,
        &mut setup,
    )?;
    // The delta repeats the first run's exact selected member, then adds 63
    // owners. The 64-row run grows it to 127, so the 65-row run fits the carry.
    for item in selected
        .get(..1)
        .ok_or("repeated owner missing")?
        .iter()
        .chain(selected.get(129..192).ok_or("final 63 owners missing")?)
    {
        directory.put_delta(
            item.owner,
            item.member,
            &recipe,
            &mut AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &maps,
                custody: &custody,
                scratch: &NoScratch,
                budget: &mut setup,
            },
        )?;
    }
    check_eq!(directory.delta_owners, 64);
    check_eq!(directory.delta_rows, 64);
    check_eq!(directory.region_count, 2);
    let pinned = save_directory(&store, &maps, &custody, &directory, &mut setup)?;
    let first_fileset = directory
        .region_at(
            0,
            &mut ReadContext {
                objects: &store,
                maps: &maps,
                budget: &mut setup,
            },
        )?
        .descriptor
        .fileset;

    // The public map port charges one write per COW application. The old
    // page-by-page implementation exceeds three writes; one final member map
    // plus the two region unlinks fits exactly without changing the oracle.
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(90))
        .ok_or("ANN consolidation test deadline overflow")?;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 3,
            bytes: 1_500_000_000,
            mutations: 100_000,
        },
        deadline,
    );
    let memory = Arc::new(QueryMemory::default());
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&memory),
    )))));
    let recording = RecordingMap::new(&maps, second_members, false);
    let result = directory.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &recording,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
    )?;
    let scan_boundary_owner = selected
        .get(127)
        .ok_or("scan boundary owner missing")?
        .owner;
    // Authenticate the source, then read that same bounded keyset as the
    // immutable COW base. Both passes keep the original 64-entry boundary.
    check_eq!(
        recording.take_member_scans()?,
        vec![
            (None, 64, 64, true),
            (
                Some(owner_map_key(scan_boundary_owner).to_vec()),
                64,
                1,
                false,
            ),
            (None, 64, 64, true),
            (
                Some(owner_map_key(scan_boundary_owner).to_vec()),
                64,
                1,
                false,
            ),
        ]
    );
    check_eq!(result.owners, 192);
    check_eq!(result.rows, 192);
    let expected: BTreeMap<_, _> = selected
        .iter()
        .map(|item| (owner_map_key(item.owner).to_vec(), item.member))
        .collect();
    check_eq!(maps.state(result.root)?, expected);
    check_eq!(work.usage().writes, 3);
    check_eq!(directory.region_count, 0);
    check!(graph_reaches(&store, pinned, first_fileset)?);
    drop(result);
    check_query_memory_released(&memory)?;
    Ok(())
}

#[test]
fn consolidation_reuses_largest_selected_root_with_exact_sparse_diff() -> TestResult {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let selected: Vec<_> = (0..22u8)
        .map(|id| {
            selected_owner(
                &store,
                &spec,
                &recipe,
                id,
                format!("selected owner {id}").as_bytes(),
                [1.0, 0.0],
            )
        })
        .collect::<TestResult<Vec<_>>>()?;
    let replacement = selected_owner(
        &store,
        &spec,
        &recipe,
        7,
        b"replacement selected owner 7",
        [0.0, 1.0],
    )?;
    let old_replaced = *selected.get(7).ok_or("old replacement member missing")?;
    let stale = *selected.get(6).ok_or("stale member missing")?;
    check!(replacement.member != old_replaced.member);
    let (_, original_owners) = owner_roots(&maps, &selected)?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), original_owners);
    let mut setup = ann_budget()?;
    let _first_members = seal_many(
        &runtime,
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        selected.get(..6).ok_or("first six owners missing")?,
        &mut setup,
    )?;
    let largest_members = seal_many(
        &runtime,
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        selected.get(6..17).ok_or("next eleven owners missing")?,
        &mut setup,
    )?;
    let old_pin = save_directory(&store, &maps, &custody, &directory, &mut setup)?;

    let selected_owners = selected
        .iter()
        .filter(|item| item.owner != stale.owner && item.owner != owner(21))
        .map(|item| {
            (
                owner_map_key(item.owner).to_vec(),
                if item.owner == replacement.owner {
                    replacement.coverage
                } else {
                    item.coverage
                },
            )
        })
        .collect();
    directory.owners = maps.seed(selected_owners)?;
    // The delta duplicates one exact selected first-run member, replaces one
    // second-run member and adds four owners. The selected first run grows it
    // to eleven rows, allowing the eleven-row prior run into this carry.
    for item in selected
        .get(..1)
        .ok_or("repeated owner missing")?
        .iter()
        .chain(selected.get(17..21).ok_or("four new owners missing")?)
    {
        directory.put_delta(
            item.owner,
            item.member,
            &recipe,
            &mut AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &maps,
                custody: &custody,
                scratch: &NoScratch,
                budget: &mut setup,
            },
        )?;
    }
    directory.put_delta(
        replacement.owner,
        replacement.member,
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut setup,
        },
    )?;
    check_eq!(directory.delta_owners, 6);
    check_eq!(directory.region_count, 2);
    let prior_directory = directory;
    let delta_members = directory.delta;

    let expected = selected
        .iter()
        .filter(|item| item.owner != stale.owner && item.owner != owner(21))
        .map(|item| {
            (
                owner_map_key(item.owner).to_vec(),
                if item.owner == replacement.owner {
                    replacement.member
                } else {
                    item.member
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    check_eq!(expected.len(), 20);
    let base_model = maps.state(largest_members)?;
    let expected_edits = base_model
        .keys()
        .chain(expected.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|key| {
            let prior = base_model.get(&key).copied();
            let next = expected.get(&key).copied();
            (prior != next).then_some(MapEdit {
                key,
                expected: prior.map(Into::into),
                next: next.map(Into::into),
            })
        })
        .collect::<Vec<_>>();
    check_eq!(expected_edits.len(), 12);

    let deadline = Instant::now()
        .checked_add(Duration::from_secs(90))
        .ok_or("ANN sparse carry test deadline overflow")?;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 3,
            bytes: 1_500_000_000,
            mutations: 100_000,
        },
        deadline,
    );
    let memory = Arc::new(QueryMemory::default());
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&memory),
    )))));
    let recording = RecordingMap::new(&maps, largest_members, false);
    let result = directory.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &recording,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
    )?;
    let member_applications = recording
        .take_applies()?
        .into_iter()
        .filter(|(_base, edits)| edits.iter().any(|edit| edit.key.len() == 38))
        .collect::<Vec<_>>();
    check_eq!(member_applications.len(), 1);
    let (applied_base, applied_edits) = member_applications
        .first()
        .ok_or("ANN selected member application missing")?;
    check_eq!(*applied_base, largest_members);
    check_eq!(applied_edits, &expected_edits);
    check_eq!(result.owners, 20);
    check_eq!(result.rows, 20);
    check_eq!(maps.state(result.root)?, expected);
    check_eq!(result.root, maps.seed(expected.clone())?);
    check_eq!(
        result
            .artifacts
            .iter()
            .map(|entry| (owner_map_key(entry.binding.owner).to_vec(), entry.reference))
            .collect::<BTreeMap<_, _>>(),
        expected
    );
    check_eq!(work.usage().writes, 3);
    check_eq!(directory.region_count, 0);
    check!(graph_reaches(&store, old_pin, stale.member)?);
    check!(graph_reaches(&store, old_pin, old_replaced.member)?);
    drop(result);
    check_query_memory_released(&memory)?;

    // Reuse the authenticated sources with a different current selection.
    // None of the eleven-row region's old members is selected now, so its
    // larger map must not replace the six-owner delta as the COW base.
    let mut stale_heavy = prior_directory;
    let sparse_selection = selected
        .get(..6)
        .ok_or("sparse first six owners missing")?
        .iter()
        .chain(selected.get(17..21).ok_or("sparse new owners missing")?)
        .map(|item| (owner_map_key(item.owner).to_vec(), item.coverage))
        .chain(std::iter::once((
            owner_map_key(replacement.owner).to_vec(),
            replacement.coverage,
        )))
        .collect();
    stale_heavy.owners = maps.seed(sparse_selection)?;
    let sparse_expected = selected
        .get(..6)
        .ok_or("sparse expected first six owners missing")?
        .iter()
        .chain(
            selected
                .get(17..21)
                .ok_or("sparse expected new owners missing")?,
        )
        .map(|item| (owner_map_key(item.owner).to_vec(), item.member))
        .chain(std::iter::once((
            owner_map_key(replacement.owner).to_vec(),
            replacement.member,
        )))
        .collect::<BTreeMap<_, _>>();
    check_eq!(sparse_expected.len(), 11);
    let delta_model = maps.state(delta_members)?;
    let sparse_edits = delta_model
        .keys()
        .chain(sparse_expected.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|key| {
            let prior = delta_model.get(&key).copied();
            let next = sparse_expected.get(&key).copied();
            (prior != next).then_some(MapEdit {
                key,
                expected: prior.map(Into::into),
                next: next.map(Into::into),
            })
        })
        .collect::<Vec<_>>();
    check_eq!(sparse_edits.len(), 5);
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(90))
        .ok_or("ANN stale-heavy carry test deadline overflow")?;
    let mut sparse_work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 3,
            bytes: 1_500_000_000,
            mutations: 100_000,
        },
        deadline,
    );
    let sparse_memory = Arc::new(QueryMemory::default());
    sparse_work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&sparse_memory),
    )))));
    let sparse_recording = RecordingMap::new(&maps, delta_members, false);
    let sparse_result = stale_heavy.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &sparse_recording,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut sparse_work,
        },
    )?;
    let sparse_member_applications = sparse_recording
        .take_applies()?
        .into_iter()
        .filter(|(_base, edits)| edits.iter().any(|edit| edit.key.len() == 38))
        .collect::<Vec<_>>();
    check_eq!(sparse_member_applications.len(), 1);
    let (sparse_base, actual_edits) = sparse_member_applications
        .first()
        .ok_or("ANN stale-heavy selected member application missing")?;
    check_eq!(*sparse_base, delta_members);
    check_eq!(actual_edits, &sparse_edits);
    check_eq!(sparse_result.owners, 11);
    check_eq!(sparse_result.rows, 11);
    check_eq!(maps.state(sparse_result.root)?, sparse_expected);
    check_eq!(sparse_result.root, maps.seed(sparse_expected.clone())?);
    check_eq!(
        sparse_result
            .artifacts
            .iter()
            .map(|entry| (owner_map_key(entry.binding.owner).to_vec(), entry.reference))
            .collect::<BTreeMap<_, _>>(),
        sparse_expected
    );
    check_eq!(sparse_work.usage().writes, 3);
    check_eq!(stale_heavy.region_count, 0);
    check!(graph_reaches(&store, old_pin, stale.member)?);
    check!(graph_reaches(&store, old_pin, old_replaced.member)?);
    drop(sparse_result);
    check_query_memory_released(&sparse_memory)?;
    Ok(())
}

#[test]
fn consolidation_rejects_wrong_delta_owner_count_before_region_build() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let first = selected_owner(&store, &spec, &recipe, 1, b"X first", [1.0, 0.0])?;
    let second = selected_owner(&store, &spec, &recipe, 2, b"Y second", [0.0, 1.0])?;
    let (_, owners) = owner_roots(&maps, &[first, second])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    directory.delta = maps.seed(BTreeMap::from([
        (owner_map_key(first.owner).to_vec(), first.member),
        (owner_map_key(second.owner).to_vec(), second.member),
    ]))?;
    // Two single-vector owners, but a canonical-yet-false declared owner count.
    directory.delta_rows = 2;
    directory.delta_owners = 1;
    let before = directory;
    let mut work = ann_budget()?;
    let result = directory.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
    );
    check!(matches!(result, Err(IndexError::CorruptObject(message))
        if message == "ANN consolidation delta owner count differs"));
    check_eq!(directory, before);
    Ok(())
}

#[test]
fn superseded_delta_does_not_read_unrelated_live_regions() -> TestResult {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let obsolete = selected_owner(&store, &spec, &recipe, 1, b"X obsolete", [1.0, 0.0])?;
    let current = selected_owner(&store, &spec, &recipe, 2, b"Y current", [0.0, 1.0])?;
    let (_, owners) = owner_roots(&maps, &[current])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let mut setup = ann_budget()?;
    let (_region, _fileset, _generation) = seal_one(
        &runtime,
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &current,
        &mut setup,
    )?;
    directory.put_delta(
        obsolete.owner,
        obsolete.member,
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut setup,
        },
    )?;
    let before = directory;
    let mut without_regions = directory;
    without_regions.regions = MapRoot(None);
    without_regions.region_count = 0;
    without_regions.maintenance_after = None;
    let mut baseline_budget = ann_budget()?;
    let baseline = without_regions.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut baseline_budget,
        },
    )?;
    let mut live_budget = ann_budget()?;
    let selected = directory.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut live_budget,
        },
    )?;
    check_eq!(selected.rows, 0);
    check_eq!(baseline.rows, 0);
    // No unrelated region-map reads or COW edits for an empty selected delta.
    check_eq!(live_budget.usage(), baseline_budget.usage());
    check_eq!(directory, before);
    Ok(())
}

struct ProviderSpanProbe {
    fail: bool,
    refusal_code: Option<quanta_index_contract::SearchPlaneErrorCodeV2>,
    cancel_during_call: bool,
}

impl TextEmbeddingProvider for ProviderSpanProbe {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        if self.fail {
            Err(self.refusal_code.map_or_else(
                || CoreError::InvalidContract("provider span probe refusal".into()),
                |code| CoreError::Typed {
                    code,
                    message: "provider span probe refusal".into(),
                },
            ))
        } else {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        let child = budget
            .diagnostic_context()
            .map(|diagnostic| diagnostic.enter(DiagnosticStageV1::QueryVerify, DiagnosticLaneV1(3)))
            .transpose()
            .map_err(|error| {
                CoreError::InvalidContract(format!("provider probe diagnostic span: {error:?}"))
            })?;
        if self.cancel_during_call {
            budget.cancel_handle().cancel();
        }
        let result = self.embed_batch(texts);
        if let Some(child) = child {
            let _ended = child.finish(DiagnosticTerminalV1::from_result(&result));
        }
        result
    }

    fn model_id(&self) -> &'static str {
        "provider-span-probe"
    }

    fn model_revision(&self) -> &'static str {
        "revision-1"
    }

    fn dimension(&self) -> usize {
        2
    }

    fn normalization(&self) -> quanta_index_contract::EmbeddingNormalization {
        quanta_index_contract::EmbeddingNormalization::L2Unit
    }
}

struct ProviderSpanObservation {
    outcome: Result<(), IndexError>,
    usage: WorkUsage,
    clock_calls: u64,
    events: Vec<DiagnosticEmitV1>,
    rejected: u64,
}

fn observe_provider_span(
    overlay: bool,
    fail: bool,
    record: bool,
) -> TestResult<ProviderSpanObservation> {
    observe_provider_span_case(
        overlay,
        ProviderSpanProbe {
            fail,
            refusal_code: None,
            cancel_during_call: false,
        },
        record,
    )
}

fn observe_provider_span_case(
    overlay: bool,
    provider: ProviderSpanProbe,
    record: bool,
) -> TestResult<ProviderSpanObservation> {
    let clock = Arc::new(InstallTraceClock::default());
    let sink = Arc::new(InstallTraceEvents::new()?);
    let capture =
        DiagnosticContextV1::new(DiagnosticCaptureIdV1([93; 16]), clock.clone(), sink.clone());
    let root = if record {
        Some(capture.enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(3))?)
    } else {
        None
    };
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let request = match root.as_ref() {
        Some(root) => request.with_diagnostic_context(capture.child_context(root)),
        None => request,
    };
    let bridge = TextEmbeddingBridge::new(provider)?;
    let (outcome, usage) = if overlay {
        let recipe = bridge.recipe(1024)?;
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 1_000,
                writes: 1_000,
                bytes: 1_000_000,
                mutations: 1_000,
            },
            request,
        );
        let outcome =
            overlay::OverlayEmbeddingPort::embed_overlay(&bridge, "row-0", &recipe, &mut budget)
                .map(|_vectors| ());
        (outcome, budget.usage())
    } else {
        (
            bridge.embed_query("query-0", &request).map(|_vectors| ()),
            WorkUsage::default(),
        )
    };
    if let Some(root) = root {
        root.finish(DiagnosticTerminalV1::from_result(&outcome))?;
    }
    let events = sink
        .rows
        .lock()
        .map_err(|_error| "provider span sink poisoned")?
        .clone();
    Ok(ProviderSpanObservation {
        outcome,
        usage,
        clock_calls: clock.0.load(AtomicOrdering::Relaxed),
        events,
        rejected: sink.rejected.load(AtomicOrdering::Relaxed),
    })
}

#[test]
fn provider_invocation_spans_preserve_query_and_overlay_results_and_parent_provider_calls()
-> TestResult {
    for overlay in [false, true] {
        for fail in [false, true] {
            let off = observe_provider_span(overlay, fail, false)?;
            let record = observe_provider_span(overlay, fail, true)?;
            check_eq!(record.outcome, off.outcome);
            check_eq!(record.usage, off.usage);
            check_eq!(off.clock_calls, 0);
            check!(off.events.is_empty());
            check_eq!(off.rejected, 0);
            check_eq!(record.rejected, 0);
            check!(record.clock_calls > 0);
            let stage = if overlay {
                DiagnosticStageV1::QueryOverlayEmbedding
            } else {
                DiagnosticStageV1::QueryEmbedding
            };
            let measured = record.events.iter().find_map(|event| {
                if let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id: Some(_),
                    stage: actual,
                    ..
                } = event.kind
                    && actual == stage
                {
                    Some(span_id)
                } else {
                    None
                }
            });
            let measured = measured.ok_or("provider invocation span missing")?;
            check!(record.events.iter().any(|event| matches!(
                event.kind,
                DiagnosticEventKindV1::SpanBegin {
                    parent_span_id: Some(parent),
                    stage: DiagnosticStageV1::QueryVerify,
                    ..
                } if parent == measured
            )));
            let expected = if fail {
                DiagnosticTerminalV1::Error { code: 1 }
            } else {
                DiagnosticTerminalV1::Ok
            };
            check!(record.events.iter().any(|event| matches!(
                event.kind,
                DiagnosticEventKindV1::SpanEnd {
                    span_id,
                    outcome,
                } if span_id == measured && outcome == expected
            )));
            check!(!record.events.iter().any(|event| matches!(
                event.kind,
                DiagnosticEventKindV1::Health { .. }
                    | DiagnosticEventKindV1::SpanEnd {
                        outcome: DiagnosticTerminalV1::Aborted,
                        ..
                    }
            )));
        }
    }
    Ok(())
}

#[test]
fn provider_query_spans_classify_typed_and_budget_cancellation_without_changing_results()
-> TestResult {
    use quanta_index_contract::SearchPlaneErrorCodeV2;

    for overlay in [false, true] {
        for (code, cancel_during_call) in [
            (SearchPlaneErrorCodeV2::RequestCancelled, false),
            (SearchPlaneErrorCodeV2::ServerOverloaded, true),
        ] {
            let off = observe_provider_span_case(
                overlay,
                ProviderSpanProbe {
                    fail: true,
                    refusal_code: Some(code),
                    cancel_during_call,
                },
                false,
            )?;
            let record = observe_provider_span_case(
                overlay,
                ProviderSpanProbe {
                    fail: true,
                    refusal_code: Some(code),
                    cancel_during_call,
                },
                true,
            )?;
            check_eq!(off.outcome, Err(IndexError::Cancelled));
            check_eq!(record.outcome, off.outcome);
            check_eq!(record.usage, off.usage);
            check_eq!(off.clock_calls, 0);
            check!(off.events.is_empty());
            check_eq!(off.rejected, 0);
            check_eq!(record.rejected, 0);
            let stage = if overlay {
                DiagnosticStageV1::QueryOverlayEmbedding
            } else {
                DiagnosticStageV1::QueryEmbedding
            };
            let measured = record.events.iter().find_map(|event| {
                if let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id: Some(_),
                    stage: actual,
                    ..
                } = event.kind
                    && actual == stage
                {
                    Some(span_id)
                } else {
                    None
                }
            });
            let measured = measured.ok_or("cancelled provider invocation span missing")?;
            check!(record.events.iter().any(|event| matches!(
                event.kind,
                DiagnosticEventKindV1::SpanEnd {
                    span_id,
                    outcome: DiagnosticTerminalV1::Cancelled { code: 1 },
                } if span_id == measured
            )));
            check!(!record.events.iter().any(|event| matches!(
                event.kind,
                DiagnosticEventKindV1::Health { .. }
                    | DiagnosticEventKindV1::SpanEnd {
                        outcome: DiagnosticTerminalV1::Aborted,
                        ..
                    }
            )));
        }
    }
    Ok(())
}
