//! Region retirement and sparse-ordinal regressions using real managed Lance files.

use super::*;
use crate::manifest::VectorIndexSealV1;
use crate::native::{ann_files, ann_lance};
use quanta_index_core::index::{
    MapBatchRequest, MapBatchWrite, MapValue, StagedMapRoots, WorkBudgetDenial,
};
use quanta_index_core::{
    DiagnosticAnnIndexKindV1, DiagnosticAnnRegionPhaseV1, DiagnosticCaptureIdV1, DiagnosticClockV1,
    DiagnosticContextV1, DiagnosticCounterV1, DiagnosticEmitV1, DiagnosticEventKindV1,
    DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticStageV1,
    DiagnosticTerminalV1,
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
            ann_directory::AnnBackend::Graph,
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
            ann_directory::AnnBackend::Graph,
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
pub(crate) struct RecordingMap<'a> {
    inner: &'a dyn PersistentMapPort,
    members: MapRoot,
    overfull_first: bool,
    scans: Mutex<Vec<MemberScan>>,
    applies: Mutex<Vec<(MapRoot, Vec<MapEdit>)>>,
    member_lookups: AtomicU64,
    lookup_key: Option<[u8; 38]>,
    refuse_apply: AtomicBool,
    empty_apply_root: bool,
    reuse_apply_root: bool,
}

impl<'a> RecordingMap<'a> {
    pub(crate) fn new(
        inner: &'a dyn PersistentMapPort,
        members: MapRoot,
        overfull_first: bool,
    ) -> Self {
        Self {
            inner,
            members,
            overfull_first,
            scans: Mutex::new(Vec::new()),
            applies: Mutex::new(Vec::new()),
            member_lookups: AtomicU64::new(0),
            lookup_key: None,
            refuse_apply: AtomicBool::new(false),
            empty_apply_root: false,
            reuse_apply_root: false,
        }
    }

    pub(crate) fn observe_lookup_key(&mut self, key: [u8; 38]) {
        self.lookup_key = Some(key);
    }

    pub(crate) fn take_lookup_count(&self) -> u64 {
        self.member_lookups.swap(0, AtomicOrdering::SeqCst)
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

impl PersistentMapPort for RecordingMap<'_> {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        if root == self.members
            && self
                .lookup_key
                .is_none_or(|wanted| wanted.as_slice() == key)
        {
            let _previous = self.member_lookups.fetch_add(1, AtomicOrdering::SeqCst);
        }
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
        if self.refuse_apply.load(AtomicOrdering::SeqCst) {
            return Err(IndexError::ResourceLimited);
        }
        if self.empty_apply_root {
            return Ok(MapWrite {
                root: MapRoot(None),
            });
        }
        if self.reuse_apply_root {
            return Ok(MapWrite { root: base });
        }
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

// Operation, Compaction with CPU, and the two exact parent markers.
const INSTALL_TRACE_EVENT_CAP: usize = 9;

#[derive(Debug, Default)]
struct InstallTraceClock(AtomicU64, AtomicU64, AtomicU64);

impl DiagnosticClockV1 for InstallTraceClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, AtomicOrdering::Relaxed)
    }

    fn thread_cpu_now_ns(&self) -> Option<u64> {
        let _previous = self.2.fetch_add(1, AtomicOrdering::Relaxed);
        Some(self.1.fetch_add(7, AtomicOrdering::Relaxed))
    }
}

#[derive(Debug)]
struct InstallTraceEvents {
    rows: Mutex<Vec<DiagnosticEmitV1>>,
    rejected: AtomicU64,
    limit: usize,
}

impl InstallTraceEvents {
    fn new() -> TestResult<Self> {
        Self::with_capacity(INSTALL_TRACE_EVENT_CAP)
    }

    fn with_capacity(capacity: usize) -> TestResult<Self> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(capacity)?;
        Ok(Self {
            rows: Mutex::new(rows),
            rejected: AtomicU64::new(0),
            limit: capacity,
        })
    }
}

impl DiagnosticEventSinkV1 for InstallTraceEvents {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        if let Ok(mut rows) = self.rows.try_lock()
            && rows.len() < self.limit
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

#[test]
fn native_materialization_installed_region_observation_has_seal_owner() -> TestResult {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let clock = Arc::new(InstallTraceClock::default());
    let sink = Arc::new(InstallTraceEvents::with_capacity(256)?);
    let capture = DiagnosticContextV1::new(DiagnosticCaptureIdV1([63; 16]), clock, sink.clone());
    let root = capture.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))?;
    let root_span = root.span_id();

    let mut selected = Vec::new();
    let mut prior = None;
    for id in 0..64_u8 {
        let item = selected_owner(
            &store,
            &spec,
            &recipe,
            id,
            b"native materialization trace",
            if id % 2 == 0 { [1.0, 0.0] } else { [0.0, 1.0] },
        )?;
        selected.push(item);
        let (sources, owners) = owner_roots(&maps, &selected)?;
        let mut work = if id == 63 {
            WorkBudget::with_request(
                WorkLimits {
                    reads: 100_000,
                    writes: 100_000,
                    bytes: 1_500_000_000,
                    mutations: 100_000,
                },
                RequestBudgetV1::for_duration(Duration::from_secs(90))
                    .with_diagnostic_context(capture.child_context(&root)),
            )
        } else {
            ann_budget()?
        };
        let directory_ref = {
            let mut context = AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &maps,
                custody: &custody,
                scratch: &NoScratch,
                budget: &mut work,
            };
            let prepared = crate::run_blocking(
                &runtime,
                ann_materialization::prepare(
                    &spec,
                    &recipe,
                    ann_directory::AnnBackend::Graph,
                    prior.as_ref(),
                    owners,
                    Some((item.owner, item.member)),
                    None,
                    &mut context,
                ),
            )?;
            prepared
                .install(&mut context)?
                .ok_or("materialized ANN directory absent")?
        };
        prior = Some(binding(&spec, &recipe, sources, owners, directory_ref));
    }
    root.finish(DiagnosticTerminalV1::Ok)?;

    let final_coverage = prior.ok_or("materialized ANN coverage absent")?;
    let mut read_work = ann_budget()?;
    let directory = ann_directory::AnnDirectory::load(
        final_coverage
            .searchable
            .ok_or("materialized ANN directory reference absent")?,
        recipe.fingerprint(),
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut read_work,
        },
    )?;
    check_eq!(directory.region_count, 1);
    check_eq!(directory.delta_owners, 0);
    let region = directory.region_at(
        0,
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut read_work,
        },
    )?;
    check_eq!(region.descriptor.rows, 64);

    let events = sink
        .rows
        .lock()
        .map_err(|_error| "native materialization trace sink poisoned")?
        .clone();
    check_eq!(sink.rejected.load(AtomicOrdering::Relaxed), 0);
    let observations: Vec<_> = events
        .iter()
        .filter_map(|event| match event.kind {
            DiagnosticEventKindV1::AnnRegionObservation {
                span_id,
                observation,
            } if observation.phase == DiagnosticAnnRegionPhaseV1::Installed => {
                Some((span_id, observation))
            }
            _ => None,
        })
        .collect();
    check_eq!(observations.len(), 1);
    let (seal_span, observation) = observations[0];
    check!(events.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::SpanBegin {
            span_id,
            parent_span_id: Some(_),
            stage: DiagnosticStageV1::SemanticSeal,
            ..
        } if span_id == seal_span
    )));
    check!(events.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::SpanEnd {
            span_id,
            outcome: DiagnosticTerminalV1::Ok,
        } if span_id == seal_span
    )));
    check_eq!(observation.fileset_ref, region.descriptor.fileset.0.0);
    check_eq!(observation.descriptor_ref, Some(region.reference().0.0));
    check_eq!(observation.ordinal, 0);
    check_eq!(observation.rows, 64);
    check_eq!(observation.index_kind, DiagnosticAnnIndexKindV1::Hnsw);
    check_eq!(observation.files, 1);
    check_eq!(observation.chunks, 1);
    check!(events.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::SpanEnd {
            span_id,
            outcome: DiagnosticTerminalV1::Ok,
        } if span_id == root_span
    )));
    Ok(())
}

#[test]
fn native_append_rejected_descriptor_has_failed_seal_without_installed_observation() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let selected = selected_owner(
        &store,
        &spec,
        &recipe,
        1,
        b"rejected descriptor",
        [1.0, 0.0],
    )?;
    let (_, owners) = owner_roots(&maps, &[selected])?;
    let members = maps.seed(BTreeMap::from([(
        owner_map_key(selected.owner).to_vec(),
        selected.member,
    )]))?;
    let mut setup = ann_budget()?;
    let fileset = native_graph::install(
        &[native_graph::RegionRow {
            owner: selected.owner,
            member: selected.commitment,
            vector_id: b"whole-file",
            vector: &selected.vector,
        }],
        recipe.fingerprint(),
        1,
        2,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut setup,
        },
    )?;
    let descriptor = ann_directory::RegionDescriptor {
        fileset,
        members,
        center: selected.vector.to_vec(),
        rows: 2, // The authenticated graph has exactly one row.
    };
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let before = directory;

    let clock = Arc::new(InstallTraceClock::default());
    let sink = Arc::new(InstallTraceEvents::new()?);
    let capture = DiagnosticContextV1::new(DiagnosticCaptureIdV1([64; 16]), clock, sink.clone());
    let root = capture.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))?;
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 1_500_000_000,
            mutations: 100_000,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(90))
            .with_diagnostic_context(capture.child_context(&root)),
    );
    let result = directory.append_region(
        &descriptor,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
    );
    root.finish(DiagnosticTerminalV1::from_result(&result))?;

    check!(matches!(result, Err(IndexError::CorruptObject(_))));
    check_eq!(directory, before);
    check_eq!(work.usage().writes, 0);
    check_eq!(sink.rejected.load(AtomicOrdering::Relaxed), 0);
    let events = sink
        .rows
        .lock()
        .map_err(|_error| "rejected native append trace sink poisoned")?
        .clone();
    check!(!events.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::AnnRegionObservation {
            observation: quanta_index_core::DiagnosticAnnRegionObservationV1 {
                phase: DiagnosticAnnRegionPhaseV1::Installed,
                ..
            },
            ..
        }
    )));
    let seal_spans: Vec<_> = events
        .iter()
        .filter_map(|event| match event.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id: Some(_),
                stage: DiagnosticStageV1::SemanticSeal,
                ..
            } => Some(span_id),
            _ => None,
        })
        .collect();
    check_eq!(seal_spans.len(), 1);
    let seal_span = seal_spans[0];
    check!(events.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::SpanEnd {
            span_id,
            outcome: DiagnosticTerminalV1::Error { code: 1 },
        } if span_id == seal_span
    )));
    check!(!events.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::SpanEnd {
            span_id,
            outcome: DiagnosticTerminalV1::Aborted,
        } if span_id == seal_span
    )));
    Ok(())
}

#[derive(Clone, Copy)]
enum CarryTraceCase {
    Valid,
    InvalidRecipe,
    ExhaustedBytes,
}

// The marker is emitted by a real object-port read during consolidation, not
// by the caller immediately after opening the Compaction span.
struct CarryTraceObjects<'a> {
    inner: &'a MemoryStore,
    mark_first_get: bool,
    marked: AtomicBool,
}

impl ImmutableObjectPort for CarryTraceObjects<'_> {
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
        let mark = self.mark_first_get && !self.marked.swap(true, AtomicOrdering::Relaxed);
        let span = if mark {
            crate::build_diagnostic::begin_build_stage(
                budget.request().diagnostic_context(),
                DiagnosticStageV1::Backend,
            )
        } else {
            None
        };
        let result = self.inner.get(reference, budget);
        crate::build_diagnostic::finish_build_stage(span, result.is_ok());
        result
    }
}

fn observe_carry(
    case: CarryTraceCase,
    record: bool,
    mark_port_and_restore: bool,
) -> TestResult<AnnTraceObservation> {
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
    let objects = CarryTraceObjects {
        inner: &store,
        mark_first_get: mark_port_and_restore,
        marked: AtomicBool::new(false),
    };
    let result = directory.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        },
    );
    if mark_port_and_restore {
        let restored = crate::build_diagnostic::begin_build_stage(
            budget.request().diagnostic_context(),
            DiagnosticStageV1::Backend,
        );
        crate::build_diagnostic::finish_build_stage(restored, true);
    }
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
    check_eq!(
        clock.2.load(AtomicOrdering::Relaxed),
        if record { 2 } else { 0 }
    );
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
        let off = observe_carry(case, false, false)?;
        let record = observe_carry(case, true, false)?;
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
        let cpu: Vec<_> = record
            .events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::Counter {
                    span_id,
                    key: DiagnosticCounterV1::ThreadCpuNs,
                    value,
                } = event.kind
                {
                    Some((span_id, value))
                } else {
                    None
                }
            })
            .collect();
        check_eq!(cpu, vec![(span, 7)]);
        check_eq!(record.events.len(), 5);
    }
    Ok(())
}

#[test]
fn carry_trace_scopes_real_object_read_and_restores_parent_after_refusal() -> TestResult {
    for case in [CarryTraceCase::Valid, CarryTraceCase::ExhaustedBytes] {
        let off = observe_carry(case, false, true)?;
        let record = observe_carry(case, true, true)?;
        check_eq!(record.result, off.result);
        check_eq!(record.usage, off.usage);
        check_eq!(off.clock_calls, 0);
        check!(off.events.is_empty());
        check_eq!(off.rejected, 0);
        check_eq!(record.rejected, 0);

        let root = record.events.iter().find_map(|event| {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id: None,
                root_kind: Some(DiagnosticRootKindV1::Operation),
                ..
            } = event.kind
            {
                Some(span_id)
            } else {
                None
            }
        });
        let root = root.ok_or("ANN carry operation root missing")?;
        let compaction = record.events.iter().find_map(|event| {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id: Some(parent),
                stage: DiagnosticStageV1::Compaction,
                ..
            } = event.kind
                && parent == root
            {
                Some(span_id)
            } else {
                None
            }
        });
        let compaction = compaction.ok_or("ANN carry Compaction span missing")?;
        let marker_spans: Vec<_> = record
            .events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id: Some(parent),
                    stage: DiagnosticStageV1::Backend,
                    ..
                } = event.kind
                {
                    Some((span_id, parent))
                } else {
                    None
                }
            })
            .collect();
        let marker_parents: Vec<_> = marker_spans.iter().map(|(_, parent)| *parent).collect();
        match case {
            CarryTraceCase::Valid => {
                check_eq!(marker_parents, vec![compaction, root]);
                check_eq!(record.events.len(), 9);
            }
            CarryTraceCase::ExhaustedBytes => {
                check_eq!(marker_parents, vec![root]);
                check_eq!(record.events.len(), 7);
            }
            CarryTraceCase::InvalidRecipe => return Err("unrequested carry trace case".into()),
        }
        for (span, _) in marker_spans {
            check!(record.events.iter().any(|event| matches!(
                event.kind,
                DiagnosticEventKindV1::SpanEnd {
                    span_id,
                    outcome: DiagnosticTerminalV1::Ok,
                } if span_id == span
            )));
        }
        let cpu: Vec<_> = record
            .events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::Counter {
                    span_id,
                    key: DiagnosticCounterV1::ThreadCpuNs,
                    value,
                } = event.kind
                {
                    Some((span_id, value))
                } else {
                    None
                }
            })
            .collect();
        check_eq!(cpu, vec![(compaction, 7)]);
        check!(record.events.iter().all(|event| !matches!(
            event.kind,
            DiagnosticEventKindV1::Health { .. }
                | DiagnosticEventKindV1::SpanEnd {
                    outcome: DiagnosticTerminalV1::Aborted,
                    ..
                }
        )));
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

#[test]
fn empty_delta_member_refuses_before_map_lookup_or_staging() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let selected = selected_owner(&store, &spec, &recipe, 1, b"empty member", [1.0, 0.0])?;
    let mut work = ann_budget()?;
    let object = store.get(selected.member, &mut work)?;
    let mut binding: OwnerMemberBinding = serde_json::from_slice(&object.payload)?;
    let object = store.get(binding.artifact, &mut work)?;
    let mut artifact: OwnerArtifact = serde_json::from_slice(&object.payload)?;
    artifact.vectors.clear();
    let bytes = serde_json::to_vec(&artifact)?;
    binding.artifact = store.insert(
        bytes.clone(),
        artifact_children(&artifact).as_ref().to_vec(),
    )?;
    binding.logical_commitment = commitment(&bytes);
    let children = ObjectChildren::from_references([
        Some(binding.artifact),
        binding.input.source,
        binding.input.projection,
        None,
    ]);
    let reference = store.insert(serde_json::to_vec(&binding)?, children.as_slice().to_vec())?;
    let (_, owners) = owner_roots(&maps, &[selected])?;
    let directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let recorded = RecordingMap::new(&maps, MapRoot(None), false);
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &recorded,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    };
    let validated = ann_directory::ValidatedDeltaMember::load(
        reference,
        selected.owner,
        &recipe,
        &mut context,
    )?;
    check_eq!(validated.rows()?, 0);
    check!(matches!(
        directory.prepare_validated_delta(validated, &recipe, &mut context),
        Err(IndexError::InvalidInput(_))
    ));
    check_eq!(recorded.member_lookups.load(AtomicOrdering::SeqCst), 0);
    check!(recorded.take_applies()?.is_empty());
    Ok(())
}

fn member_with_vectors(
    store: &MemoryStore,
    reference: ObjectRef,
    vectors: Vec<EmbeddingVector>,
    work: &mut WorkBudget,
) -> TestResult<ObjectRef> {
    let object = store.get(reference, work)?;
    let mut binding: OwnerMemberBinding = serde_json::from_slice(&object.payload)?;
    let object = store.get(binding.artifact, work)?;
    let mut artifact: OwnerArtifact = serde_json::from_slice(&object.payload)?;
    artifact.vectors = vectors;
    let bytes = serde_json::to_vec(&artifact)?;
    binding.artifact = store.insert(
        bytes.clone(),
        artifact_children(&artifact).as_ref().to_vec(),
    )?;
    binding.logical_commitment = commitment(&bytes);
    let children = ObjectChildren::from_references([
        Some(binding.artifact),
        binding.input.source,
        binding.input.projection,
        None,
    ]);
    Ok(store.insert(serde_json::to_vec(&binding)?, children.as_slice().to_vec())?)
}

fn two_vectors() -> Vec<EmbeddingVector> {
    vec![
        EmbeddingVector {
            id: b"a".to_vec(),
            values: vec![1.0, 0.0],
        },
        EmbeddingVector {
            id: b"b".to_vec(),
            values: vec![0.0, 1.0],
        },
    ]
}

#[test]
fn delta_atomic_remove_refuses_bad_row_count_before_cow_and_preserves_directory() -> TestResult {
    for refuse_apply in [false, true] {
        let store = MemoryStore::default();
        let maps = GraphMap::new(&store);
        let custody = TestCustody;
        let recipe = recipe();
        let spec = instance(&recipe);
        let selected = selected_owner(&store, &spec, &recipe, 1, b"two rows", [1.0, 0.0])?;
        let mut work = ann_budget()?;
        let two = member_with_vectors(&store, selected.member, two_vectors(), &mut work)?;
        let root = maps.seed(BTreeMap::from([(
            owner_map_key(selected.owner).to_vec(),
            two,
        )]))?;
        let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), MapRoot(None));
        directory.delta = root;
        directory.delta_owners = 1;
        directory.delta_rows = 1; // Shape-valid, but below the authenticated member's two rows.
        let before = directory;
        let recorded = RecordingMap::new(&maps, root, false);
        recorded
            .refuse_apply
            .store(refuse_apply, AtomicOrdering::SeqCst);
        let result = directory.remove_delta(
            selected.owner,
            &recipe,
            &mut AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &recorded,
                custody: &custody,
                scratch: &NoScratch,
                budget: &mut work,
            },
        );
        check!(matches!(result, Err(IndexError::CorruptObject(_))));
        check_eq!(directory, before);
        check!(recorded.take_applies()?.is_empty());
    }
    Ok(())
}

#[test]
fn delta_atomic_prune_refuses_impossible_remainder_before_cow() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let removed = selected_owner(&store, &spec, &recipe, 1, b"removed", [1.0, 0.0])?;
    let retained = selected_owner(&store, &spec, &recipe, 2, b"retained", [0.0, 1.0])?;
    let mut work = ann_budget()?;
    let two = member_with_vectors(&store, removed.member, two_vectors(), &mut work)?;
    let root = maps.seed(BTreeMap::from([
        (owner_map_key(removed.owner).to_vec(), two),
        (owner_map_key(retained.owner).to_vec(), retained.member),
    ]))?;
    let (_, owners) = owner_roots(&maps, &[retained])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    directory.delta = root;
    directory.delta_owners = 2;
    directory.delta_rows = 2; // Removing two rows would leave one nonempty owner with zero rows.
    let before = directory;
    let recorded = RecordingMap::new(&maps, root, false);
    let result = directory.retire_delta(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &recorded,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
    );
    check!(matches!(result, Err(IndexError::CorruptObject(_))));
    check_eq!(directory, before);
    check!(recorded.take_applies()?.is_empty());
    Ok(())
}

#[test]
fn delta_atomic_replacement_refuses_impossible_counters_before_prepared_edit() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let replaced = selected_owner(&store, &spec, &recipe, 1, b"replaced", [1.0, 0.0])?;
    let retained = selected_owner(&store, &spec, &recipe, 2, b"retained", [0.0, 1.0])?;
    let mut work = ann_budget()?;
    let two = member_with_vectors(&store, replaced.member, two_vectors(), &mut work)?;
    let root = maps.seed(BTreeMap::from([
        (owner_map_key(replaced.owner).to_vec(), two),
        (owner_map_key(retained.owner).to_vec(), retained.member),
    ]))?;
    let (_, owners) = owner_roots(&maps, &[replaced, retained])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    directory.delta = root;
    directory.delta_owners = 2;
    directory.delta_rows = 2;
    let recorded = RecordingMap::new(&maps, root, false);
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &recorded,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    };
    let validated = ann_directory::ValidatedDeltaMember::load(
        replaced.member,
        replaced.owner,
        &recipe,
        &mut context,
    )?;
    let result = directory.prepare_validated_delta(validated, &recipe, &mut context);
    check!(matches!(result, Err(IndexError::CorruptObject(_))));
    check!(recorded.take_applies()?.is_empty());
    Ok(())
}

#[test]
fn consolidation_atomic_late_failure_preserves_caller_directory() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let first = selected_owner(&store, &spec, &recipe, 1, b"first region", [1.0, 0.0])?;
    let second = selected_owner(&store, &spec, &recipe, 2, b"later bad region", [0.0, 1.0])?;
    let delta = selected_owner(&store, &spec, &recipe, 3, b"current delta", [1.0, 0.0])?;
    let (_, owners) = owner_roots(&maps, &[first, second, delta])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let mut work = ann_budget()?;
    let (first_ref, _, _) = seal_one(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &first,
        &mut work,
    )?;
    let (second_ref, _, _) = seal_one(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &second,
        &mut work,
    )?;
    // Valid canonical descriptor DATA points at an unsupported later graph.
    // The first equal-size region must already have entered the carry loop.
    let object = store.get(second_ref, &mut work)?;
    let mut descriptor: ann_directory::RegionDescriptor = serde_json::from_slice(&object.payload)?;
    descriptor.fileset = store.insert(b"BADGRAPH".to_vec(), Vec::new())?;
    let bad_second = store.insert(
        serde_json::to_vec(&descriptor)?,
        ObjectChildren::from_references([
            Some(descriptor.fileset),
            descriptor.members.0,
            None,
            None,
        ])
        .as_slice()
        .to_vec(),
    )?;
    directory.regions = maps.seed(BTreeMap::from([
        (0_u64.to_be_bytes().to_vec(), first_ref),
        (1_u64.to_be_bytes().to_vec(), bad_second),
    ]))?;
    directory.put_delta(
        delta.owner,
        delta.member,
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
    let before = directory;
    let recorded = RecordingMap::new(&maps, directory.regions, false);
    let result = directory.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &recorded,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
    );
    check!(matches!(result, Err(IndexError::RebuildRequired)));
    check!(recorded.take_applies()?.is_empty());
    check_eq!(directory, before);
    Ok(())
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

#[test]
fn ann_publication_rejects_missing_or_foreign_prior_layout() -> TestResult {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let item = selected_owner(&store, &spec, &recipe, 1, b"existing input", [1.0, 0.0])?;
    let (sources, owners) = owner_roots(&maps, &[item])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let mut work = ann_budget()?;
    let _sealed = seal_one(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &item,
        &mut work,
    )?;
    let searchable = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let healthy = binding(&spec, &recipe, sources, owners, searchable);
    let missing = CoverageBinding {
        searchable: None,
        ..healthy
    };
    let foreign = CoverageBinding {
        instance: IndexInstanceId([99; 16]),
        ..healthy
    };
    let foreign_policy = quanta_index_core::index::InputBinding {
        policy: Digest([99; 32]),
        ..healthy.desired
    };
    let foreign_schema = quanta_index_core::index::InputBinding {
        schema: SchemaId([99; 16]),
        ..healthy.desired
    };
    let adapter = NativeSemantic::new(
        SourceVectorProvider {
            recipe: recipe.clone(),
            empty: false,
        },
        recipe,
    )?
    .with_ann();
    let mut accepted = Vec::new();
    if !matches!(
        adapter.validate_coverage(&missing),
        Err(IndexError::CorruptObject(_))
    ) {
        accepted.push("missing directory in coverage validation");
    }
    for (name, prior) in [
        ("missing directory", missing),
        ("foreign instance", foreign),
        (
            "foreign policy",
            CoverageBinding {
                desired: foreign_policy,
                covered: Some(foreign_policy),
                ..healthy
            },
        ),
        (
            "foreign schema",
            CoverageBinding {
                desired: foreign_schema,
                covered: Some(foreign_schema),
                ..healthy
            },
        ),
    ] {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        let prepared = crate::run_blocking(
            &runtime,
            adapter.prepare_searchable(&spec, Some(&prior), owners, None, None, &mut context),
        );
        if !matches!(prepared, Err(IndexError::CorruptObject(_))) {
            accepted.push(name);
        }
    }
    if !accepted.is_empty() {
        return Err(format!("ANN prior layout escaped validation: {accepted:?}").into());
    }
    Ok(())
}

#[test]
fn ann_publication_rejects_member_outside_current_coverage() -> TestResult {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let old = selected_owner(&store, &spec, &recipe, 1, b"old input", [1.0, 0.0])?;
    let current = selected_owner(&store, &spec, &recipe, 1, b"current input", [0.0, 1.0])?;
    let other = selected_owner(&store, &spec, &recipe, 2, b"other owner", [1.0, 0.0])?;
    let key = owner_map_key(old.owner);
    let (_, current_owners) = owner_roots(&maps, &[current])?;
    let (_, other_owners) = owner_roots(&maps, &[other])?;
    let mut fixture_work = ann_budget()?;
    let old_member = store.get(old.member, &mut fixture_work)?;
    let mut foreign_member: OwnerMemberBinding = serde_json::from_slice(&old_member.payload)?;
    let stored_artifact = store.get(foreign_member.artifact, &mut fixture_work)?;
    let mut foreign_artifact: OwnerArtifact = serde_json::from_slice(&stored_artifact.payload)?;
    foreign_member.input.derivation_policy = Digest([129; 32]);
    foreign_artifact.input = foreign_member.input;
    let bytes = serde_json::to_vec(&foreign_artifact)?;
    foreign_member.logical_commitment = commitment(&bytes);
    foreign_member.artifact = store.insert(
        bytes,
        artifact_children(&foreign_artifact).as_ref().to_vec(),
    )?;
    let foreign_member_ref = store.insert(
        serde_json::to_vec(&foreign_member)?,
        ObjectChildren::from_references([
            Some(foreign_member.artifact),
            Some(old.source),
            None,
            None,
        ])
        .as_slice()
        .to_vec(),
    )?;
    let foreign_coverage = OwnerCoverage {
        owner: old.owner,
        desired: foreign_member.input,
        covered: Some(foreign_member.input),
        searchable: Some(foreign_member_ref),
    };
    let foreign_coverage_ref = store.insert(
        serde_json::to_vec(&foreign_coverage)?,
        ObjectChildren::from_references([Some(old.source), Some(foreign_member_ref), None, None])
            .as_slice()
            .to_vec(),
    )?;
    let foreign_owners = maps.seed(BTreeMap::from([(key.to_vec(), foreign_coverage_ref)]))?;
    let forged_owner_root = |covered| -> TestResult<MapRoot> {
        let binding = OwnerCoverage {
            owner: old.owner,
            desired: current.input,
            covered: Some(covered),
            searchable: Some(old.member),
        };
        let reference = store.insert(
            serde_json::to_vec(&binding)?,
            ObjectChildren::from_references([Some(current.source), None, Some(old.member), None])
                .as_slice()
                .to_vec(),
        )?;
        Ok(maps.seed(BTreeMap::from([(key.to_vec(), reference)]))?)
    };
    let recorded = RecordingMap::new(&maps, MapRoot(None), false);
    let mut work = ann_budget()?;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &recorded,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    };
    let mut accepted = Vec::new();
    for (name, owners, member) in [
        ("absent owner", other_owners, old.member),
        ("superseded member", current_owners, old.member),
        ("pending owner", forged_owner_root(old.input)?, old.member),
        (
            "wrong member input",
            forged_owner_root(current.input)?,
            old.member,
        ),
        (
            "foreign derivation policy",
            foreign_owners,
            foreign_member_ref,
        ),
    ] {
        let result = crate::run_blocking(
            &runtime,
            ann_materialization::prepare(
                &spec,
                &recipe,
                ann_directory::AnnBackend::Graph,
                None,
                owners,
                Some((old.owner, member)),
                None,
                &mut context,
            ),
        );
        if !matches!(result, Err(IndexError::CorruptObject(_))) {
            accepted.push(name);
        }
    }
    if !accepted.is_empty() {
        return Err(
            format!("ANN publication accepted invalid selected bindings: {accepted:?}").into(),
        );
    }
    check!(recorded.take_applies()?.is_empty());
    Ok(())
}

#[test]
fn whole_file_artifact_rejects_payload_outside_selected_source() -> TestResult {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = whole_file_recipe_gold();
    let spec = instance(&recipe);
    let (source, _) = semantic_source(&store, "src/selected.rs", b"selected source")?;
    let (_, other_payload) = semantic_source(&store, "src/other.rs", b"unrelated source")?;
    let forged_owner = owner(1);
    let (key, coverage) = semantic_owner_vector_for_owner(
        &store,
        &spec,
        &recipe,
        source,
        other_payload,
        forged_owner,
        vec![1.0, 0.0],
    )?;
    let mut work = ann_budget()?;
    let coverage_object = store.get(coverage, &mut work)?;
    let member_ref = serde_json::from_slice::<OwnerCoverage>(&coverage_object.payload)?
        .searchable
        .ok_or("missing forged member")?;
    let member_object = store.get(member_ref, &mut work)?;
    let member: OwnerMemberBinding = serde_json::from_slice(&member_object.payload)?;
    let forged = SelectedOwner {
        owner: forged_owner,
        source,
        member: member_ref,
        coverage,
        input: member.input,
        commitment: member.logical_commitment,
        vector: [1.0, 0.0],
    };
    let other = selected_owner(&store, &spec, &recipe, 2, b"valid region", [0.0, 1.0])?;
    let (sources, owners) = owner_roots(&maps, &[forged, other])?;
    let publication = crate::run_blocking(
        &runtime,
        ann_materialization::prepare(
            &spec,
            &recipe,
            ann_directory::AnnBackend::Graph,
            None,
            owners,
            Some((forged.owner, forged.member)),
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
    let mut accepted = Vec::new();
    if !matches!(publication, Err(IndexError::CorruptObject(_))) {
        accepted.push("ANN publication");
    }

    for mode in ["exact file search", "ANN delta", "ANN sealed region"] {
        let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
        if mode == "ANN delta" {
            let _ = seal_one(
                &store,
                &maps,
                &custody,
                &recipe,
                &mut directory,
                &other,
                &mut work,
            )?;
            directory.delta = maps.seed(BTreeMap::from([(key.clone(), forged.member)]))?;
            directory.delta_owners = 1;
            directory.delta_rows = 1;
        } else if mode == "ANN sealed region" {
            let _ = seal_one(
                &store,
                &maps,
                &custody,
                &recipe,
                &mut directory,
                &forged,
                &mut work,
            )?;
        }
        let reference = if mode == "exact file search" {
            owners.0.ok_or("missing exact layout")?
        } else {
            save_directory(&store, &maps, &custody, &directory, &mut work)?
        };
        let coverage = binding(&spec, &recipe, sources, owners, reference);
        let query = SearchQuery {
            source_files: true,
            lexical_eligibility: None,
            exact_path: None,
            coverage: &coverage,
            projection: None,
            source_owners: sources,
            source_paths: MapRoot(None),
            instance: spec.instance,
            format: spec.format,
            recipe: &recipe,
            vector: &[1.0, 0.0],
            top_k: 2,
            page_size: 2,
            max_owners: 8,
            overlay: None,
        };
        let mut context = ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        };
        let result = if mode == "exact file search" {
            search_exact(&query, &mut context)
        } else {
            ann_query::search(&query, &mut context)
        };
        if !matches!(result, Err(IndexError::CorruptObject(_))) {
            accepted.push(mode);
        }
    }
    if !accepted.is_empty() {
        return Err(
            format!("artifact with unrelated payload escaped validation: {accepted:?}").into(),
        );
    }
    Ok(())
}

#[test]
fn mixed_ann_ranks_current_delta_member_without_reopening_it() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = whole_file_recipe_gold();
    let spec = instance(&recipe);
    let delta = selected_owner(&store, &spec, &recipe, 1, b"exact delta", [1.0, 0.0])?;
    let sealed = selected_owner(&store, &spec, &recipe, 2, b"sealed region", [0.0, 1.0])?;
    let (sources, owners) = owner_roots(&maps, &[delta, sealed])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let mut work = ann_budget()?;
    let _ = seal_one(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &sealed,
        &mut work,
    )?;
    directory.delta = maps.seed(BTreeMap::from([(
        owner_map_key(delta.owner).to_vec(),
        delta.member,
    )]))?;
    directory.delta_owners = 1;
    directory.delta_rows = 1;
    let reference = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let coverage = binding(&spec, &recipe, sources, owners, reference);
    let query = SearchQuery {
        source_files: true,
        lexical_eligibility: None,
        exact_path: None,
        coverage: &coverage,
        projection: None,
        source_owners: sources,
        source_paths: MapRoot(None),
        instance: spec.instance,
        format: spec.format,
        recipe: &recipe,
        vector: &[1.0, 0.0],
        top_k: 2,
        page_size: 2,
        max_owners: 8,
        overlay: None,
    };
    for reference in [delta.member, delta.source] {
        let objects = ReadOnceDescriptor::new(&store, reference);
        let result = ann_query::search(
            &query,
            &mut ReadContext {
                objects: &objects,
                maps: &maps,
                budget: &mut ann_budget()?,
            },
        )?;
        check_eq!(objects.reads(), 1);
        check_eq!(
            result.hits.iter().map(|hit| hit.owner).collect::<Vec<_>>(),
            vec![delta.owner, sealed.owner]
        );
        let first = result.hits.first().ok_or("missing delta result")?;
        check_eq!(
            first.source_file().ok_or("missing delta source")?.path(),
            "src/1.rs"
        );
    }
    Ok(())
}

#[test]
fn small_ann_exact_fallback_reuses_delta_members_and_checks_unselected_rows() -> TestResult {
    struct NoFullRankWindow {
        meter: QueryMemoryScope,
        full_window_bytes: u64,
    }

    impl MemoryScopePort for NoFullRankWindow {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if bytes == self.full_window_bytes {
                return Err(IndexError::ResourceLimited);
            }
            self.meter.reserve(bytes, lifetime)
        }
    }

    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = whole_file_recipe_gold();
    let spec = instance(&recipe);
    let old = selected_owner(&store, &spec, &recipe, 1, b"old input", [1.0, 0.0])?;
    let current = selected_owner(&store, &spec, &recipe, 1, b"new input", [0.0, 1.0])?;
    let delta = selected_owner(&store, &spec, &recipe, 2, b"current delta", [1.0, 0.0])?;
    let (sources, owners) = owner_roots(&maps, &[current, delta])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    directory.delta = maps.seed(BTreeMap::from([
        (owner_map_key(old.owner).to_vec(), old.member),
        (owner_map_key(delta.owner).to_vec(), delta.member),
    ]))?;
    directory.delta_owners = 2;
    directory.delta_rows = 2;
    let mut work = ann_budget()?;
    let reference = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let coverage = binding(&spec, &recipe, sources, owners, reference);
    let mut query = SearchQuery {
        source_files: true,
        lexical_eligibility: None,
        exact_path: None,
        coverage: &coverage,
        projection: None,
        source_owners: sources,
        source_paths: MapRoot(None),
        instance: spec.instance,
        format: spec.format,
        recipe: &recipe,
        vector: &[1.0, 0.0],
        // Only two hits can be selected. A reservation proportional to this
        // window would refuse the search even though both hits fit.
        top_k: 4093,
        page_size: 1,
        max_owners: 2,
        overlay: None,
    };
    let memory = Arc::new(QueryMemory::default());
    let scope = Arc::new(NoFullRankWindow {
        meter: QueryMemoryScope(Arc::clone(&memory)),
        full_window_bytes: u64::try_from(
            usize::try_from(query.top_k)? * std::mem::size_of::<SearchHit>(),
        )?,
    });
    let mut query_work = ann_budget()?;
    query_work.bind_memory_scope(Some(MemoryScope::new(scope)));
    let objects = ReadOnceDescriptor::new(&store, delta.member);
    let mut source_reads = RecordingMap::new(&maps, sources, false);
    source_reads.observe_lookup_key(owner_map_key(delta.owner));
    let result = ann_query::search(
        &query,
        &mut ReadContext {
            objects: &objects,
            maps: &source_reads,
            budget: &mut query_work,
        },
    )?;
    check_eq!(objects.reads(), 1);
    check_eq!(source_reads.take_lookup_count(), 1);
    check!(result.exhaustive);
    check_eq!(result.traversal, NativeSemanticTraversal::ExactExhausted);
    check_eq!(
        result.hits.iter().map(|hit| hit.owner).collect::<Vec<_>>(),
        vec![delta.owner, current.owner]
    );

    check_eq!(result.hits.capacity(), 2);
    check_eq!(query_work.first_denial(), None);
    drop(result);
    check_query_memory_released(&memory)?;

    let mut context = ReadContext {
        objects: &store,
        maps: &maps,
        budget: &mut work,
    };
    let selected = selected_owner::SelectedOwner::load(
        &owner_map_key(delta.owner),
        delta.coverage,
        &query,
        &mut context,
    )?;
    let member = selected.current_member(&query, &mut context)?;
    query.source_owners = MapRoot(None);
    let wrong_root = source_file::load(&member, &selected, None, &query, &mut context);
    query.source_owners = sources;
    check_eq!(
        wrong_root.err(),
        Some(IndexError::CorruptObject(
            "file semantic source is outside the selected view".into()
        ))
    );

    // Wrong owner under the last delta key must fail even if exact ranking is capped earlier.
    directory.delta = maps.seed(BTreeMap::from([
        (owner_map_key(old.owner).to_vec(), old.member),
        (owner_map_key(delta.owner).to_vec(), old.member),
    ]))?;
    let forged = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let forged_coverage = binding(&spec, &recipe, sources, owners, forged);
    query.coverage = &forged_coverage;
    query.max_owners = 1;
    check!(matches!(
        ann_query::search(
            &query,
            &mut ReadContext {
                objects: &store,
                maps: &maps,
                budget: &mut work
            }
        ),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn routed_ann_releases_candidate_scratch_before_overlay() -> TestResult {
    struct OverlayMemoryCheck<'a> {
        bridge: &'a TextEmbeddingBridge<FixedTextProvider>,
        memory: &'a QueryMemory,
        slots: u64,
    }

    impl overlay::OverlayEmbeddingPort for OverlayMemoryCheck<'_> {
        fn embed_overlay(
            &self,
            text: &str,
            recipe: &EmbeddingRecipe,
            budget: &mut WorkBudget,
        ) -> Result<overlay::OverlayVectors, IndexError> {
            if self.memory.live.load(AtomicOrdering::SeqCst) != self.slots {
                return Err(IndexError::BackendViolation(
                    "ANN candidate memory is live at overlay embedding".into(),
                ));
            }
            overlay::OverlayEmbeddingPort::embed_overlay(self.bridge, text, recipe, budget)
        }
    }

    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let mut work = ann_budget()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?;
    let recipe = bridge.recipe(1024)?;
    let spec = instance(&recipe);
    let retired = selected_owner(&store, &spec, &recipe, 1, b"removed", [1.0, 0.0])?;
    // Immutable region rows can remain after their source owner is deleted.
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), MapRoot(None));
    let (_region, fileset, _generation) = seal_one(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &retired,
        &mut work,
    )?;
    let reference = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let coverage = binding(&spec, &recipe, MapRoot(None), MapRoot(None), reference);
    let documents = [SemanticOverlayDocument {
        owner: owner(2),
        revision: 1,
        path: None,
        text: Some(Arc::from("editor change")),
    }];
    let memory = Arc::new(QueryMemory::default());
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&memory),
    )))));
    let bridge = OverlayMemoryCheck {
        bridge: &bridge,
        memory: memory.as_ref(),
        slots: u64::try_from(std::mem::size_of::<SearchHit>())?,
    };
    let objects = ReadOnceDescriptor::new(&store, fileset);
    let result = ann_query::search(
        &SearchQuery {
            source_files: false,
            lexical_eligibility: None,
            exact_path: None,
            coverage: &coverage,
            projection: None,
            source_owners: MapRoot(None),
            source_paths: MapRoot(None),
            instance: spec.instance,
            format: spec.format,
            recipe: &recipe,
            vector: &[1.0, 0.0],
            // This selected view contains only one live whole-file overlay.
            // An oversized requested window must still admit one hit slot.
            top_k: 4093,
            page_size: 1,
            max_owners: 2,
            overlay: Some(overlay::OverlaySearch {
                documents: &documents,
                bridge: &bridge,
            }),
        },
        &mut ReadContext {
            objects: &objects,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    check_eq!(objects.reads(), 1);
    check_eq!(result.scanned_owners, 1);
    check_eq!(result.hits.len(), 1);
    let hit = result.hits.first().ok_or("overlay hit missing")?;
    check_eq!(hit.owner, owner(2));
    check!(hit.overlay.is_some());
    check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    drop(result);
    check_query_memory_released(&memory)?;
    Ok(())
}

#[test]
fn superseded_ann_region_member_does_not_consume_current_owner_slot() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = whole_file_recipe_gold();
    let spec = instance(&recipe);
    let old = selected_owner(&store, &spec, &recipe, 1, b"old entry", [1.0, 0.0])?;
    let current = selected_owner(&store, &spec, &recipe, 1, b"replacement", [0.0, 1.0])?;
    let winner = selected_owner(&store, &spec, &recipe, 2, b"nearest current", [0.8, 0.6])?;
    let (sources, owners) = owner_roots(&maps, &[current, winner])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let mut work = ann_budget()?;
    for row in [&old, &winner, &current] {
        let _ = seal_one(
            &store,
            &maps,
            &custody,
            &recipe,
            &mut directory,
            row,
            &mut work,
        )?;
    }
    let reference = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let coverage = binding(&spec, &recipe, sources, owners, reference);
    let query = SearchQuery {
        source_files: true,
        lexical_eligibility: None,
        exact_path: None,
        coverage: &coverage,
        projection: None,
        source_owners: sources,
        source_paths: MapRoot(None),
        instance: spec.instance,
        format: spec.format,
        recipe: &recipe,
        vector: &[1.0, 0.0],
        top_k: 1,
        page_size: 1,
        max_owners: 1,
        overlay: None,
    };
    let result = ann_query::search(
        &query,
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    check_eq!(
        result.hits.iter().map(|hit| hit.owner).collect::<Vec<_>>(),
        vec![winner.owner]
    );
    check_eq!(result.scanned_owners, 1);
    check_eq!(result.skipped_stale, 0);
    check_eq!(
        result.traversal,
        NativeSemanticTraversal::Approximate { capped: true }
    );
    Ok(())
}

#[test]
fn only_superseded_rows_beyond_ann_owner_limit_do_not_report_cap() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = whole_file_recipe_gold();
    let spec = instance(&recipe);
    let old = selected_owner(&store, &spec, &recipe, 1, b"old entry", [0.0, 1.0])?;
    let current = selected_owner(&store, &spec, &recipe, 1, b"replacement", [0.6, 0.8])?;
    let winner = selected_owner(&store, &spec, &recipe, 2, b"nearest current", [1.0, 0.0])?;
    let (sources, owners) = owner_roots(&maps, &[current, winner])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let mut work = ann_budget()?;
    let _ = seal_many(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &[old, winner],
        &mut work,
    )?;
    let reference = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let coverage = binding(&spec, &recipe, sources, owners, reference);
    let query = SearchQuery {
        source_files: false,
        lexical_eligibility: None,
        exact_path: None,
        coverage: &coverage,
        projection: None,
        source_owners: sources,
        source_paths: MapRoot(None),
        instance: spec.instance,
        format: spec.format,
        recipe: &recipe,
        vector: &[1.0, 0.0],
        top_k: 1,
        page_size: 1,
        max_owners: 1,
        overlay: None,
    };
    let result = ann_query::search(
        &query,
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    check_eq!(
        result.hits.first().ok_or("missing current result")?.owner,
        winner.owner
    );
    check_eq!(result.scanned_owners, 1);
    check_eq!(
        result.traversal,
        NativeSemanticTraversal::Approximate { capped: false }
    );
    Ok(())
}

#[test]
fn projected_file_window_ignores_source_free_and_certified_empty_rows() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let projection_id = ProjectionId([41; 16]);
    let producer = ProducerId([42; 16]);
    let schema = SchemaId([43; 16]);
    let mut recipe = whole_file_recipe_gold();
    recipe.input_policy = EmbeddingInputPolicy::ProjectionUtf8 {
        projection: projection_id,
        producer,
        schema,
    };
    let mut spec = instance(&recipe);
    spec.input_domain = InputDomain::Projection {
        projection: projection_id,
        producer,
    };
    spec.schema = schema;
    let mut work = ann_budget()?;
    let (source, _) = semantic_source(&store, "eligible.rs", b"eligible source")?;
    let source_owner = owner(10);
    let sources = maps.seed(BTreeMap::from([(
        owner_map_key(source_owner).to_vec(),
        source,
    )]))?;
    let mut projection_rows = BTreeMap::new();
    let mut coverage_rows = BTreeMap::new();
    let mut graph_rows = Vec::new();
    for (semantic_owner, source_pair, vector, empty_now) in [
        (owner(1), Some((source_owner, source)), [1.0, 0.0], true),
        (owner(2), None, [0.8, 0.6], false),
        (owner(3), Some((source_owner, source)), [0.0, 1.0], false),
    ] {
        let make_row = |empty| {
            super::super::lexical_projected_tests::projected_owner_vectors(
                &store,
                &spec,
                &recipe,
                projection_id,
                producer,
                semantic_owner,
                source_pair,
                vec![EmbeddingVector {
                    id: b"whole-file".to_vec(),
                    values: vector.to_vec(),
                }],
                empty,
            )
        };
        let (contribution, graph_coverage) = make_row(false)?;
        let (selected_contribution, coverage) = if empty_now {
            make_row(true)?
        } else {
            (contribution, graph_coverage)
        };
        let graph_coverage_object = store.get(graph_coverage, &mut work)?;
        let graph_input: OwnerCoverage = serde_json::from_slice(&graph_coverage_object.payload)?;
        let member = graph_input.searchable.ok_or("missing graph member")?;
        let stored = store.get(member, &mut work)?;
        let binding: OwnerMemberBinding = serde_json::from_slice(&stored.payload)?;
        graph_rows.push(SelectedOwner {
            owner: semantic_owner,
            source,
            member,
            coverage,
            input: binding.input,
            commitment: binding.logical_commitment,
            vector,
        });
        let key = owner_map_key(semantic_owner);
        check_eq!(
            projection_rows.insert(key.to_vec(), selected_contribution),
            None
        );
        check_eq!(coverage_rows.insert(key.to_vec(), coverage), None);
    }
    let projection_owners = maps.seed(projection_rows)?;
    let owners = maps.seed(coverage_rows)?;
    let projection = ProjectionBinding {
        projection: projection_id,
        producer,
        owners: projection_owners,
        source: SourceCut {
            owners: sources,
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        },
        schema,
        producer_policy: Digest([44; 32]),
        dependency_context: Digest([45; 32]),
        certificate: store.insert(b"projected coverage certificate".to_vec(), vec![])?,
    };
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let _ = seal_many(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &graph_rows,
        &mut work,
    )?;
    let reference = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let mut coverage = binding(&spec, &recipe, sources, owners, reference);
    coverage.desired.root = projection_owners;
    coverage.desired.context = projection.materialization_context();
    coverage.covered = Some(coverage.desired);
    let mut exact_directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    exact_directory.delta = maps.seed(BTreeMap::from([
        (owner_map_key(owner(2)).to_vec(), graph_rows[1].member),
        (owner_map_key(owner(3)).to_vec(), graph_rows[2].member),
    ]))?;
    exact_directory.delta_owners = 2;
    exact_directory.delta_rows = 2;
    let fallback = save_directory(&store, &maps, &custody, &exact_directory, &mut work)?;
    for (source_files, expected, capped) in [(true, owner(3), false), (false, owner(2), true)] {
        let query = SearchQuery {
            source_files,
            lexical_eligibility: None,
            exact_path: None,
            coverage: &coverage,
            projection: Some(projection),
            source_owners: sources,
            source_paths: MapRoot(None),
            instance: spec.instance,
            format: spec.format,
            recipe: &recipe,
            vector: &[1.0, 0.0],
            top_k: 1,
            page_size: 2,
            max_owners: 1,
            overlay: None,
        };
        let result = ann_query::search(
            &query,
            &mut ReadContext {
                objects: &store,
                maps: &maps,
                budget: &mut work,
            },
        )?;
        check_eq!(result.hits.first().map(|hit| hit.owner), Some(expected));
        check_eq!(result.scanned_owners, 1);
        check_eq!(result.skipped_stale, 0);
        check_eq!(
            result.traversal,
            NativeSemanticTraversal::Approximate { capped }
        );
    }
    for (ann, searchable) in [(false, owners.0), (true, Some(fallback))] {
        let exact_coverage = CoverageBinding {
            searchable,
            ..coverage
        };
        let query = SearchQuery {
            source_files: true,
            lexical_eligibility: None,
            exact_path: None,
            coverage: &exact_coverage,
            projection: Some(projection),
            source_owners: sources,
            source_paths: MapRoot(None),
            instance: spec.instance,
            format: spec.format,
            recipe: &recipe,
            vector: &[1.0, 0.0],
            top_k: 1,
            page_size: 1,
            max_owners: 1,
            overlay: None,
        };
        let mut context = ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        };
        let result = if ann {
            ann_query::search(&query, &mut context)?
        } else {
            search_exact(&query, &mut context)?
        };
        check_eq!(result.hits.first().map(|hit| hit.owner), Some(owner(3)));
        check_eq!(result.scanned_owners, 1);
        check_eq!(result.skipped_stale, 0);
        check_eq!(result.traversal, NativeSemanticTraversal::ExactExhausted);
        check!(result.exhaustive);
    }
    Ok(())
}

#[test]
fn sealed_ann_shared_projection_source_reuses_retained_witness() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let projection_id = ProjectionId([41; 16]);
    let producer = ProducerId([42; 16]);
    let schema = SchemaId([43; 16]);
    let mut recipe = whole_file_recipe_gold();
    recipe.input_policy = EmbeddingInputPolicy::ProjectionUtf8 {
        projection: projection_id,
        producer,
        schema,
    };
    let mut spec = instance(&recipe);
    spec.input_domain = InputDomain::Projection {
        projection: projection_id,
        producer,
    };
    spec.schema = schema;
    let mut work = ann_budget()?;
    let source_owner = owner(10);
    let (source, _) = semantic_source(&store, "src/shared.rs", b"shared source")?;
    let sources = maps.seed(BTreeMap::from([(
        owner_map_key(source_owner).to_vec(),
        source,
    )]))?;
    let mut projection_rows = BTreeMap::new();
    let mut coverage_rows = BTreeMap::new();
    let mut graph_rows = Vec::new();
    // Graph order favors A, whose original score is lower. B must replace A
    // while both candidates describe the same source file.
    for (semantic_owner, original, graph_vector) in [
        (owner(1), [0.0, 1.0], [1.0, 0.0]),
        (owner(2), [1.0, 0.0], [0.0, 1.0]),
    ] {
        let (contribution, coverage) =
            super::super::lexical_projected_tests::projected_owner_vectors(
                &store,
                &spec,
                &recipe,
                projection_id,
                producer,
                semantic_owner,
                Some((source_owner, source)),
                vec![EmbeddingVector {
                    id: b"whole-file".to_vec(),
                    values: original.to_vec(),
                }],
                false,
            )?;
        let owner_coverage: OwnerCoverage =
            serde_json::from_slice(&store.get(coverage, &mut work)?.payload)?;
        let member = owner_coverage
            .searchable
            .ok_or("missing projected graph member")?;
        let binding: OwnerMemberBinding =
            serde_json::from_slice(&store.get(member, &mut work)?.payload)?;
        graph_rows.push(SelectedOwner {
            owner: semantic_owner,
            source,
            member,
            coverage,
            input: binding.input,
            commitment: binding.logical_commitment,
            vector: graph_vector,
        });
        let key = owner_map_key(semantic_owner).to_vec();
        check_eq!(projection_rows.insert(key.clone(), contribution), None);
        check_eq!(coverage_rows.insert(key, coverage), None);
    }
    let projection_owners = maps.seed(projection_rows)?;
    let owners = maps.seed(coverage_rows)?;
    let projection = ProjectionBinding {
        projection: projection_id,
        producer,
        owners: projection_owners,
        source: SourceCut {
            owners: sources,
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        },
        schema,
        producer_policy: Digest([44; 32]),
        dependency_context: Digest([45; 32]),
        certificate: store.insert(b"shared source projection certificate".to_vec(), vec![])?,
    };
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let _ = seal_many(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &graph_rows,
        &mut work,
    )?;
    let reference = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let mut coverage = binding(&spec, &recipe, sources, owners, reference);
    coverage.desired.root = projection_owners;
    coverage.desired.context = projection.materialization_context();
    coverage.covered = Some(coverage.desired);
    for exact_path in [None, Some("src/shared.rs")] {
        let mut query_coverage = coverage;
        if exact_path.is_some() {
            query_coverage.status = CoverageStatus::Partial { missing: owners };
            query_coverage.covered = None;
        }
        let memory = Arc::new(QueryMemory::default());
        let mut query_budget = ann_budget()?;
        query_budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
            Arc::clone(&memory),
        )))));
        let objects = ReadOnceDescriptor::new(&store, source);
        let result = ann_query::search(
            &SearchQuery {
                source_files: true,
                lexical_eligibility: None,
                exact_path,
                coverage: &query_coverage,
                projection: Some(projection),
                source_owners: sources,
                source_paths: MapRoot(None),
                instance: spec.instance,
                format: spec.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 1,
                page_size: 2,
                max_owners: 2,
                overlay: None,
            },
            &mut ReadContext {
                objects: &objects,
                maps: &maps,
                budget: &mut query_budget,
            },
        )?;
        check_eq!(objects.reads(), 1);
        check_eq!(result.hits.len(), 1);
        let hit = result.hits.first().ok_or("missing shared file hit")?;
        let file = hit
            .source_file()
            .ok_or("missing authenticated shared file")?;
        check_eq!(
            (hit.owner, file.owner, file.source, file.path()),
            (owner(2), source_owner, source, "src/shared.rs")
        );
        check_eq!(hit.cosine.to_bits(), 1.0_f64.to_bits());
        drop(result);
        check_query_memory_released(&memory)?;
    }
    Ok(())
}

#[test]
fn lance_file_refill_across_shared_projection_source() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let projection_id = ProjectionId([51; 16]);
    let producer = ProducerId([52; 16]);
    let schema = SchemaId([53; 16]);
    let mut recipe = whole_file_recipe_gold();
    recipe.input_policy = EmbeddingInputPolicy::ProjectionUtf8 {
        projection: projection_id,
        producer,
        schema,
    };
    let mut spec = instance(&recipe);
    spec.input_domain = InputDomain::Projection {
        projection: projection_id,
        producer,
    };
    spec.schema = schema;

    let shared_owner = owner(200);
    let tail_owner = owner(201);
    let weak_owner = owner(202);
    let weakest_owner = owner(203);
    let (shared_source, _) = semantic_source(&store, "src/shared.rs", b"shared source")?;
    let (tail_source, _) = semantic_source(&store, "src/tail.rs", b"tail source")?;
    let (weak_source, _) = semantic_source(&store, "src/weak.rs", b"weak source")?;
    let (weakest_source, _) = semantic_source(&store, "src/weakest.rs", b"weakest source")?;
    let sources = maps.seed(BTreeMap::from([
        (owner_map_key(shared_owner).to_vec(), shared_source),
        (owner_map_key(tail_owner).to_vec(), tail_source),
        (owner_map_key(weak_owner).to_vec(), weak_source),
        (owner_map_key(weakest_owner).to_vec(), weakest_source),
    ]))?;
    let mut projection_rows = BTreeMap::new();
    let mut coverage_rows = BTreeMap::new();
    let mut selected_owners = Vec::new();
    let mut work = ann_budget()?;

    // Two exact delta files fill the global heap before the Lance region.
    // Its first 64 nearest vectors share a file. The 65th vector belongs to
    // another file and outranks both delta files, so refill must reach it.
    for ordinal in 1..=67u8 {
        let semantic_owner = owner(ordinal);
        let (file_owner, source, vector) = match ordinal {
            65 => (tail_owner, tail_source, [0.0, 1.0]),
            66 => (weak_owner, weak_source, [-0.8, 0.6]),
            67 => (weakest_owner, weakest_source, [-1.0, 0.0]),
            _ => (shared_owner, shared_source, [1.0, 0.0]),
        };
        let (contribution, coverage) =
            super::super::lexical_projected_tests::projected_owner_vectors(
                &store,
                &spec,
                &recipe,
                projection_id,
                producer,
                semantic_owner,
                Some((file_owner, source)),
                vec![EmbeddingVector {
                    id: b"whole-file".to_vec(),
                    values: vector.to_vec(),
                }],
                false,
            )?;
        let owner_coverage: OwnerCoverage =
            serde_json::from_slice(&store.get(coverage, &mut work)?.payload)?;
        let member = owner_coverage
            .searchable
            .ok_or("missing projected Lance member")?;
        let binding: OwnerMemberBinding =
            serde_json::from_slice(&store.get(member, &mut work)?.payload)?;
        let selected = SelectedOwner {
            owner: semantic_owner,
            source,
            member,
            coverage,
            input: binding.input,
            commitment: binding.logical_commitment,
            vector,
        };
        selected_owners.push(selected);
        let key = owner_map_key(semantic_owner).to_vec();
        check_eq!(projection_rows.insert(key.clone(), contribution), None);
        check_eq!(coverage_rows.insert(key, coverage), None);
    }
    let projection_owners = maps.seed(projection_rows)?;
    let owners = maps.seed(coverage_rows)?;
    let projection = ProjectionBinding {
        projection: projection_id,
        producer,
        owners: projection_owners,
        source: SourceCut {
            owners: sources,
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        },
        schema,
        producer_policy: Digest([54; 32]),
        dependency_context: Digest([55; 32]),
        certificate: store.insert(b"shared file refill certificate".to_vec(), vec![])?,
    };
    let mut directory = ann_directory::AnnDirectory::empty_with_backend(
        recipe.fingerprint(),
        owners,
        ann_directory::AnnBackend::LanceDb,
    );
    let _members = seal_many(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &selected_owners[..65],
        &mut work,
    )?;
    for item in &selected_owners[65..] {
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
                budget: &mut work,
            },
        )?;
    }
    let reference = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let mut coverage = binding(&spec, &recipe, sources, owners, reference);
    coverage.desired.root = projection_owners;
    coverage.desired.context = projection.materialization_context();
    coverage.covered = Some(coverage.desired);

    let memory = Arc::new(QueryMemory::default());
    let mut query_budget = ann_budget()?;
    query_budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&memory),
    )))));
    let query = SearchQuery {
        source_files: true,
        lexical_eligibility: None,
        exact_path: None,
        coverage: &coverage,
        projection: Some(projection),
        source_owners: sources,
        source_paths: MapRoot(None),
        instance: spec.instance,
        format: spec.format,
        recipe: &recipe,
        vector: &[1.0, 0.0],
        top_k: 2,
        page_size: 67,
        max_owners: 67,
        overlay: None,
    };
    let result = {
        let mut read = ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut query_budget,
        };
        let selected =
            ann_directory::AnnDirectory::load_for_coverage(&coverage, &recipe, &mut read)?;
        let runtime = test_runtime()?;
        ann_query::search_loaded(&query, selected.as_ref(), None, &mut read, Some(&runtime))?
    };
    check_eq!(result.hits.len(), 2);
    check_eq!(result.scanned_owners, 67);
    check_eq!(
        result.traversal,
        NativeSemanticTraversal::Approximate { capped: false }
    );
    check!(!result.exhaustive);
    let first = result.hits.first().ok_or("shared file hit absent")?;
    let second = result.hits.get(1).ok_or("tail file hit absent")?;
    let first_file = first.source_file().ok_or("shared file witness absent")?;
    let second_file = second.source_file().ok_or("tail file witness absent")?;
    check_eq!(
        (
            first.owner,
            first_file.owner,
            first_file.source,
            first_file.path()
        ),
        (owner(1), shared_owner, shared_source, "src/shared.rs")
    );
    check_eq!(
        (
            second.owner,
            second_file.owner,
            second_file.source,
            second_file.path()
        ),
        (owner(65), tail_owner, tail_source, "src/tail.rs")
    );
    check_eq!(first.cosine.to_bits(), 1.0_f64.to_bits());
    check_eq!(second.cosine.to_bits(), 0.0_f64.to_bits());
    drop(result);
    check_query_memory_released(&memory)?;
    Ok(())
}

#[test]
fn partial_path_file_search_filters_before_owner_limit() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = whole_file_recipe_gold();
    let spec = instance(&recipe);
    let excluded = selected_owner(
        &store,
        &spec,
        &recipe,
        1,
        b"outside requested path",
        [1.0, 0.0],
    )?;
    let wanted = selected_owner(
        &store,
        &spec,
        &recipe,
        2,
        b"inside requested path",
        [0.0, 1.0],
    )?;
    let (sources, owners) = owner_roots(&maps, &[excluded, wanted])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let mut work = ann_budget()?;
    let _ = seal_many(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &[excluded, wanted],
        &mut work,
    )?;
    let sealed = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let mut coverage = binding(&spec, &recipe, sources, owners, sealed);
    let missing_owner = owner(3);
    let (missing_source, _) = semantic_source(&store, "src/3.rs", b"pending third source")?;
    let missing = maps.seed(BTreeMap::from([(
        owner_map_key(missing_owner).to_vec(),
        missing_source,
    )]))?;
    coverage.status = CoverageStatus::Partial { missing };
    coverage.covered = None;
    let mut failures = Vec::new();
    for sealed_ann in [false, true] {
        let query = SearchQuery {
            source_files: true,
            lexical_eligibility: None,
            exact_path: Some("src/2.rs"),
            coverage: &coverage,
            projection: None,
            source_owners: sources,
            source_paths: MapRoot(None),
            instance: spec.instance,
            format: spec.format,
            recipe: &recipe,
            vector: &[1.0, 0.0],
            top_k: 1,
            page_size: 1,
            max_owners: 1,
            overlay: None,
        };
        let objects = ReadOnceDescriptor::new(&store, wanted.source);
        let mut context = ReadContext {
            objects: &objects,
            maps: &maps,
            budget: &mut work,
        };
        let result = if sealed_ann {
            ann_query::search(&query, &mut context)
        } else {
            search_exact(&query, &mut context)
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                failures.push(format!(
                    "sealed ANN={sealed_ann}: {error}; reads={}",
                    objects.reads()
                ));
                continue;
            }
        };
        let traversal = if sealed_ann {
            NativeSemanticTraversal::Approximate { capped: false }
        } else {
            NativeSemanticTraversal::ExactExhausted
        };
        if objects.reads() != 1
            || result.hits.first().map(|hit| hit.owner) != Some(wanted.owner)
            || result
                .hits
                .first()
                .and_then(SearchHit::source_file)
                .map(SemanticSourceFile::path)
                != Some("src/2.rs")
            || result.scanned_owners != 1
            || result.traversal != traversal
            || result.exhaustive
        {
            failures.push(format!("sealed ANN={sealed_ann}: {result:?}"));
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("\n").into());
    }
    Ok(())
}

#[test]
fn delta_edits_reject_unchanged_map_root() -> TestResult {
    use quanta_index_core::index::StagedObjectReferences;
    use quanta_index_core::index::ports::{ObjectMapBatchPort, PreparedObjectMapBatch};

    struct UnchangedDelta(MapRoot);

    impl ObjectMapBatchPort for UnchangedDelta {
        fn install_objects_and_maps(
            &self,
            _objects: &[ObjectBatchInput],
            _bases: &[MapRoot],
            _edits: &[&MapEdit],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<(StagedObjectReferences, StagedMapRoots), IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected delta installation".into(),
            ))
        }

        fn prepare_one_map(
            self: Arc<Self>,
            base: MapRoot,
            _edit: &MapEdit,
            _attempt: AttemptRef,
            _budget: &mut WorkBudget,
        ) -> Result<Box<dyn PreparedObjectMapBatch>, IndexError> {
            Ok(Box::new(Self(base)))
        }
    }

    impl PreparedObjectMapBatch for UnchangedDelta {
        fn root(&self) -> MapRoot {
            self.0
        }

        fn install(
            self: Box<Self>,
            _objects: &[ObjectBatchInput],
            _additional: Option<(MapRoot, &MapEdit)>,
            _attempt: AttemptRef,
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<(StagedObjectReferences, StagedMapRoots), IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected delta installation".into(),
            ))
        }
    }

    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let first = selected_owner(&store, &spec, &recipe, 1, b"first", [1.0, 0.0])?;
    let second = selected_owner(&store, &spec, &recipe, 2, b"second", [0.0, 1.0])?;
    let replacement = selected_owner(&store, &spec, &recipe, 1, b"replacement", [0.0, 1.0])?;
    let (sources, owners) = owner_roots(&maps, &[second])?;
    let (_, replaced_owners) = owner_roots(&maps, &[replacement, second])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    directory.delta = maps.seed(BTreeMap::from([
        (owner_map_key(first.owner).to_vec(), first.member),
        (owner_map_key(second.owner).to_vec(), second.member),
    ]))?;
    directory.delta_owners = 2;
    directory.delta_rows = 2;
    let mut work = ann_budget()?;
    let searchable = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let prior = binding(&spec, &recipe, sources, owners, searchable);
    let mut recorded = RecordingMap::new(&maps, directory.delta, false);
    recorded.reuse_apply_root = true;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &recorded,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    };
    let before = directory;
    let removal = directory.remove_delta(first.owner, &recipe, &mut context);
    let after_removal = directory;
    directory = before;
    let pruning = directory.retire_delta(&recipe, &mut context);
    let after_pruning = directory;
    directory = before;
    let consolidation = directory.consolidated_members(&recipe, &mut context);
    let runtime = test_runtime()?;
    let direct_replacement = crate::run_blocking(
        &runtime,
        ann_materialization::prepare(
            &spec,
            &recipe,
            ann_directory::AnnBackend::Graph,
            Some(&prior),
            replaced_owners,
            Some((replacement.owner, replacement.member)),
            None,
            &mut context,
        ),
    );
    let writer: Arc<dyn ObjectMapBatchPort> = Arc::new(UnchangedDelta(before.delta));
    let prepared_replacement = crate::run_blocking(
        &runtime,
        ann_materialization::prepare(
            &spec,
            &recipe,
            ann_directory::AnnBackend::Graph,
            Some(&prior),
            replaced_owners,
            Some((replacement.owner, replacement.member)),
            Some(&writer),
            &mut context,
        ),
    );
    check_eq!(recorded.take_applies()?.len(), 4);
    let accepted: Vec<_> = [
        (
            "remove",
            matches!(removal, Err(IndexError::CorruptObject(_))),
        ),
        (
            "prune",
            matches!(pruning, Err(IndexError::CorruptObject(_))),
        ),
        (
            "consolidate",
            matches!(consolidation, Err(IndexError::CorruptObject(_))),
        ),
        (
            "direct replace",
            matches!(direct_replacement, Err(IndexError::CorruptObject(_))),
        ),
        (
            "prepared replace",
            matches!(prepared_replacement, Err(IndexError::CorruptObject(_))),
        ),
    ]
    .into_iter()
    .filter_map(|(operation, refused)| (!refused).then_some(operation))
    .collect();
    if !accepted.is_empty() {
        return Err(format!("delta edits accepted the unchanged map root: {accepted:?}").into());
    }
    check_eq!(after_removal, before);
    check_eq!(after_pruning, before);
    check_eq!(directory, before);
    Ok(())
}

#[test]
fn selected_owner_scans_refuse_invalid_pages_and_replayed_cursor() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let first = selected_owner(&store, &spec, &recipe, 1, b"first", [1.0, 0.0])?;
    let second = selected_owner(&store, &spec, &recipe, 2, b"second", [0.0, 1.0])?;
    let (sources, owners) = owner_roots(&maps, &[first, second])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    directory.delta = maps.seed(BTreeMap::from([
        (owner_map_key(first.owner).to_vec(), first.member),
        (owner_map_key(second.owner).to_vec(), second.member),
    ]))?;
    directory.delta_owners = 2;
    directory.delta_rows = 2;
    let mut work = ann_budget()?;
    let searchable = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let coverage = binding(&spec, &recipe, sources, owners, searchable);
    for (name, selected, page_size, has_more) in [
        ("empty first page", [].as_slice(), 2, false),
        ("duplicate", [first, first].as_slice(), 2, false),
        ("reversed", [second, first].as_slice(), 2, false),
        ("oversized", [first, second].as_slice(), 1, false),
        ("replayed cursor", [first].as_slice(), 1, true),
    ] {
        for ann in [false, true] {
            *maps
                .scan_override
                .lock()
                .map_err(|_| "scan override lock poisoned")? = Some((
                if ann { directory.delta } else { owners },
                MapPage::new(
                    selected
                        .iter()
                        .map(|item| {
                            (
                                owner_map_key(item.owner).to_vec(),
                                if ann {
                                    item.member.into()
                                } else {
                                    item.coverage.into()
                                },
                            )
                        })
                        .collect(),
                    has_more,
                    Vec::new(),
                    None,
                ),
            ));
            let query = SearchQuery {
                source_files: false,
                lexical_eligibility: None,
                exact_path: None,
                coverage: &coverage,
                projection: None,
                source_owners: sources,
                source_paths: MapRoot(None),
                instance: spec.instance,
                format: spec.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 2,
                page_size,
                max_owners: 8,
                overlay: None,
            };
            let mut read = ReadContext {
                objects: &store,
                maps: &maps,
                budget: &mut work,
            };
            let result = if ann {
                ann_query::search(&query, &mut read)
            } else {
                search_exact(&query, &mut read)
            };
            let refused = match name {
                "oversized" => matches!(result, Err(IndexError::BackendViolation(_))),
                _ => matches!(result, Err(IndexError::CorruptObject(_))),
            };
            if !refused {
                return Err(format!(
                    "{name} owner page escaped the query scan (ANN={ann}): {result:?}"
                )
                .into());
            }
        }
    }
    Ok(())
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
    let generation = directory
        .next_region
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    let rows = [native_graph::RegionRow {
        owner: item.owner,
        member: item.commitment,
        vector_id: b"whole-file",
        vector: &item.vector,
    }];
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: store,
        maps,
        custody,
        scratch: &NoScratch,
        budget,
    };
    let fileset = native_graph::install(&rows, recipe.fingerprint(), generation, 2, &mut context)?;
    let descriptor = ann_directory::RegionDescriptor {
        fileset,
        members,
        center: item.vector.to_vec(),
        rows: 1,
    };
    let region = directory.append_region(&descriptor, &mut context)?;
    Ok((region, fileset, generation))
}

#[test]
fn region_append_rejects_missing_map_root_without_changing_directory() -> TestResult {
    check_region_append_failure(false)
}

#[test]
fn region_append_admits_map_key_before_installing_descriptor() -> TestResult {
    check_region_append_failure(true)
}

fn check_region_append_failure(deny_key: bool) -> TestResult {
    struct DenyKeyMemory(Arc<QueryMemory>);
    impl MemoryScopePort for DenyKeyMemory {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if bytes == 8 {
                return Err(IndexError::ResourceLimited);
            }
            QueryMemoryScope(Arc::clone(&self.0)).reserve(bytes, lifetime)
        }
    }

    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let item = selected_owner(&store, &spec, &recipe, 1, b"region append", [1.0, 0.0])?;
    let (_, owners) = owner_roots(&maps, &[item])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let mut setup = ann_budget()?;
    let (_, _, _) = seal_one(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &item,
        &mut setup,
    )?;
    let members = maps.seed(BTreeMap::from([(
        owner_map_key(item.owner).to_vec(),
        item.member,
    )]))?;
    let fileset = native_graph::install(
        &[native_graph::RegionRow {
            owner: item.owner,
            member: item.commitment,
            vector_id: b"whole-file",
            vector: &item.vector,
        }],
        recipe.fingerprint(),
        2,
        2,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut setup,
        },
    )?;
    let descriptor = ann_directory::RegionDescriptor {
        fileset,
        members,
        center: item.vector.to_vec(),
        rows: 1,
    };
    let before = directory;
    let mut recorded = RecordingMap::new(&maps, directory.regions, false);
    recorded.empty_apply_root = !deny_key;
    let mut work = ann_budget()?;
    let memory = Arc::new(QueryMemory::default());
    if deny_key {
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(DenyKeyMemory(Arc::clone(
            &memory,
        ))))));
    }
    let result = directory.append_region(
        &descriptor,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &recorded,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
    );
    if deny_key {
        check!(matches!(result, Err(IndexError::ResourceLimited)));
        check_eq!(work.usage().writes, 0);
        check!(recorded.take_applies()?.is_empty());
        check_query_memory_released(&memory)?;
    } else {
        check!(matches!(result, Err(IndexError::CorruptObject(_))));
        check_eq!(recorded.take_applies()?.len(), 1);
    }
    check_eq!(directory, before);
    Ok(())
}

fn seal_many(
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
    let generation = directory
        .next_region
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: store,
        maps,
        custody,
        scratch: &NoScratch,
        budget,
    };
    let fileset = match directory.backend()? {
        ann_directory::AnnBackend::Graph => {
            let rows: Vec<_> = selected
                .iter()
                .map(|item| native_graph::RegionRow {
                    owner: item.owner,
                    member: item.commitment,
                    vector_id: b"whole-file",
                    vector: &item.vector,
                })
                .collect();
            native_graph::install(&rows, recipe.fingerprint(), generation, 2, &mut context)?
        }
        ann_directory::AnnBackend::LanceDb => {
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
            crate::run_blocking(
                &test_runtime()?,
                ann_materialization::install_lance_region(
                    &rows,
                    recipe.fingerprint(),
                    generation,
                    2,
                    &mut context,
                ),
            )?
        }
    };
    let mut center = vec![0.0, 0.0];
    for item in selected {
        center[0] += item.vector[0];
        center[1] += item.vector[1];
    }
    if center.iter().all(|value| *value == 0.0) {
        center.copy_from_slice(&selected.first().ok_or("empty region fixture")?.vector);
    }
    let descriptor = ann_directory::RegionDescriptor {
        fileset,
        members,
        center,
        rows: u32::try_from(selected.len())?,
    };
    let _region = directory.append_region(&descriptor, &mut context)?;
    Ok(members)
}

#[test]
fn region_edits_reject_unchanged_map_root() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let item = selected_owner(&store, &spec, &recipe, 1, b"retired region", [1.0, 0.0])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), MapRoot(None));
    let mut work = ann_budget()?;
    for _ in 0..2 {
        let _sealed = seal_one(
            &store,
            &maps,
            &custody,
            &recipe,
            &mut directory,
            &item,
            &mut work,
        )?;
    }
    let before = directory;
    let mut recorded = RecordingMap::new(&maps, directory.regions, false);
    recorded.reuse_apply_root = true;
    let result = directory.retire_obsolete(&mut AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &recorded,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    });
    check_eq!(recorded.take_applies()?.len(), 1);
    check!(matches!(result, Err(IndexError::CorruptObject(_))));
    check_eq!(directory, before);
    Ok(())
}

#[test]
fn empty_promised_continuation_refuses_search_and_retirement() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let first = selected_owner(&store, &spec, &recipe, 1, b"first", [0.0, 1.0])?;
    let second = selected_owner(&store, &spec, &recipe, 2, b"second", [1.0, 0.0])?;
    let (sources, owners) = owner_roots(&maps, &[first, second])?;
    let (_, surviving_owners) = owner_roots(&maps, &[second])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), surviving_owners);
    let mut work = ann_budget()?;
    let members = seal_many(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &[first, second],
        &mut work,
    )?;
    let cursor = owner_map_key(first.owner).to_vec();
    let inject = |root| -> TestResult {
        *maps
            .empty_scan_after
            .lock()
            .map_err(|_| "scan cursor lock poisoned")? = Some((root, cursor.clone()));
        Ok(())
    };
    inject(members)?;
    let before = directory;
    let recorded = RecordingMap::new(&maps, members, false);
    let retirement = directory.retire_obsolete(&mut AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &recorded,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    });
    let after_retirement = directory;

    directory = before;
    directory.owners = owners;
    directory.delta = members;
    directory.delta_owners = 2;
    directory.delta_rows = 2;
    let searchable = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let coverage = binding(&spec, &recipe, sources, owners, searchable);
    let query = SearchQuery {
        source_files: false,
        lexical_eligibility: None,
        exact_path: None,
        coverage: &coverage,
        projection: None,
        source_owners: sources,
        source_paths: MapRoot(None),
        instance: spec.instance,
        format: spec.format,
        recipe: &recipe,
        vector: &[1.0, 0.0],
        top_k: 2,
        page_size: 1,
        max_owners: 8,
        overlay: None,
    };
    let mut read = ReadContext {
        objects: &store,
        maps: &maps,
        budget: &mut work,
    };
    inject(owners)?;
    let exact = search_exact(&query, &mut read);
    inject(members)?;
    let ann = ann_query::search(&query, &mut read);
    check!(matches!(ann, Err(IndexError::CorruptObject(_))));
    if let Ok(result) = exact {
        return Err(format!(
            "missing exact continuation returned {result:?}; retirement={retirement:?}"
        )
        .into());
    }
    check!(matches!(exact, Err(IndexError::CorruptObject(_))));
    check!(matches!(retirement, Err(IndexError::CorruptObject(_))));
    check_eq!(after_retirement, before);
    check!(recorded.take_applies()?.is_empty());
    Ok(())
}

#[test]
fn retirement_refuses_false_region_continuation_before_unlink() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let item = selected_owner(&store, &spec, &recipe, 1, b"retired region", [1.0, 0.0])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), MapRoot(None));
    let mut work = ann_budget()?;
    let (reference, _, _) = seal_one(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &item,
        &mut work,
    )?;
    let before = directory;
    *maps
        .scan_override
        .lock()
        .map_err(|_| "scan override lock poisoned")? = Some((
        directory.regions,
        MapPage::new(
            vec![(0_u64.to_be_bytes().to_vec(), reference.into())],
            true,
            Vec::new(),
            None,
        ),
    ));
    let recorded = RecordingMap::new(&maps, directory.regions, false);
    let result = directory.retire_obsolete(&mut AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &recorded,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    });
    check!(matches!(result, Err(IndexError::CorruptObject(_))));
    check_eq!(directory, before);
    check!(recorded.take_applies()?.is_empty());
    Ok(())
}

#[test]
fn retirement_probes_one_live_member_then_preserves_bounded_dead_region_walk() -> TestResult {
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
    let _graph = native_graph::load(store, fileset, budget)?;
    Ok(vec![fileset])
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
        false,
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

    let second_member = store.get(second.member, &mut budget)?;
    let second_binding: OwnerMemberBinding = serde_json::from_slice(&second_member.payload)?;
    let checked_objects = SingleReadMemberObjects {
        inner: &store,
        member: second.member,
        artifact: second_binding.artifact,
        member_reads: AtomicU64::new(0),
        artifact_reads: AtomicU64::new(0),
    };
    let recorded = RecordingMap::new(&maps, directory.delta, false);

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

    recorded.refuse_apply.store(true, AtomicOrdering::SeqCst);
    let mut refused_work = ann_budget()?;
    let refused = directory.retire_delta(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &recorded,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut refused_work,
        },
    );
    check!(matches!(refused, Err(IndexError::ResourceLimited)));
    check_eq!(directory, before_prune);
    let refused_applies = recorded.take_applies()?;
    let [(refused_base, refused_edits)] = refused_applies.as_slice() else {
        return Err("delta retirement must attempt one bounded apply".into());
    };
    check_eq!(*refused_base, before_prune.delta);
    check_eq!(refused_edits.len(), 2);
    recorded.refuse_apply.store(false, AtomicOrdering::SeqCst);

    {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &checked_objects,
            maps: &recorded,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut budget,
        };
        directory.retire_delta(&recipe, &mut context)?;
    }
    let applies = recorded.take_applies()?;
    let [(applied_base, applied_edits)] = applies.as_slice() else {
        return Err("delta retirement must install one bounded apply".into());
    };
    check_eq!(*applied_base, before_prune.delta);
    let [second_edit, third_edit] = applied_edits.as_slice() else {
        return Err("delta retirement must remove exactly two stale owners".into());
    };
    check_eq!(
        second_edit.key.as_slice(),
        owner_map_key(second.owner).as_slice()
    );
    check_eq!(
        third_edit.key.as_slice(),
        owner_map_key(third.owner).as_slice()
    );
    check_eq!(second_edit.expected, Some(MapValue::from(second.member)));
    check_eq!(third_edit.expected, Some(MapValue::from(third.member)));
    check_eq!(second_edit.next, None);
    check_eq!(third_edit.next, None);
    check_eq!(recorded.member_lookups.load(AtomicOrdering::SeqCst), 0);
    check_eq!(checked_objects.member_reads.load(AtomicOrdering::SeqCst), 1);
    check_eq!(
        checked_objects.artifact_reads.load(AtomicOrdering::SeqCst),
        1
    );
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
fn delta_retirement_late_artifact_and_count_fail_before_cow_install() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let first = selected_owner(&store, &spec, &recipe, 1, b"first", [1.0, 0.0])?;
    let second = selected_owner(&store, &spec, &recipe, 2, b"second", [0.0, 1.0])?;
    let third = selected_owner(&store, &spec, &recipe, 3, b"third", [1.0, 0.0])?;
    let mut setup = ann_budget()?;
    let original_member = store.get(third.member, &mut setup)?;
    let mut binding: OwnerMemberBinding = serde_json::from_slice(&original_member.payload)?;
    binding.artifact = store.insert(b"{".to_vec(), Vec::new())?;
    let children = ObjectChildren::from_references([
        Some(binding.artifact),
        binding.input.source,
        binding.input.projection,
        None,
    ]);
    let malformed_member =
        store.insert(serde_json::to_vec(&binding)?, children.as_slice().to_vec())?;
    let checked_objects = SingleReadMemberObjects {
        inner: &store,
        member: malformed_member,
        artifact: binding.artifact,
        member_reads: AtomicU64::new(0),
        artifact_reads: AtomicU64::new(0),
    };
    let malformed_root = maps.seed(BTreeMap::from([
        (owner_map_key(first.owner).to_vec(), first.member),
        (owner_map_key(second.owner).to_vec(), second.member),
        (owner_map_key(third.owner).to_vec(), malformed_member),
    ]))?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), MapRoot(None));
    directory.delta = malformed_root;
    directory.delta_owners = 3;
    directory.delta_rows = 3;
    let before = directory;
    let recorded = RecordingMap::new(&maps, malformed_root, false);
    let memory = Arc::new(QueryMemory::default());
    let mut work = ann_budget()?;
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&memory),
    )))));
    let refused = directory.retire_delta(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &checked_objects,
            maps: &recorded,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
    );
    check!(matches!(refused, Err(IndexError::CorruptObject(reason))
        if reason == "owner vector commitment mismatch"));
    check_eq!(checked_objects.member_reads.load(AtomicOrdering::SeqCst), 1);
    check_eq!(
        checked_objects.artifact_reads.load(AtomicOrdering::SeqCst),
        1
    );
    check_eq!(directory, before);
    check!(recorded.take_applies()?.is_empty());
    drop(work);
    check_query_memory_released(&memory)?;

    let valid_root = maps.seed(BTreeMap::from([
        (owner_map_key(first.owner).to_vec(), first.member),
        (owner_map_key(second.owner).to_vec(), second.member),
        (owner_map_key(third.owner).to_vec(), third.member),
    ]))?;
    for declared_rows in [2, 4] {
        let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), MapRoot(None));
        directory.delta = valid_root;
        directory.delta_owners = 3;
        directory.delta_rows = declared_rows;
        let before = directory;
        let recorded = RecordingMap::new(&maps, valid_root, false);
        let mut work = ann_budget()?;
        let refused = directory.retire_delta(
            &recipe,
            &mut AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &recorded,
                custody: &custody,
                scratch: &NoScratch,
                budget: &mut work,
            },
        );
        check!(matches!(refused, Err(IndexError::CorruptObject(_))));
        check_eq!(directory, before);
        check!(recorded.take_applies()?.is_empty());
    }
    Ok(())
}

#[test]
fn obsolete_and_absent_regions_retire_but_pending_searchable_and_old_view_survive() -> TestResult {
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
    let (old_hits, old_memory) =
        selected_search(&store, &maps, &old_binding, old_sources, &spec, &recipe)?;
    let (new_hits, new_memory) =
        selected_search(&store, &maps, &new_binding, new_sources, &spec, &recipe)?;
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
        native_graph::load(&store, old_sparse_fileset, &mut budget)?
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
        &store,
        &maps,
        &custody,
        &recipe,
        &mut sparse,
        &second,
        &mut budget,
    )?;
    check_eq!(generation, 4098);
    let loaded = native_graph::load(&store, new_fileset, &mut budget)?;
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
fn full_consolidation_scan_rejects_underdeclared_region_count() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let recipe = recipe();
    let spec = instance(&recipe);
    let owner = selected_owner(&store, &spec, &recipe, 1, b"delta", [1.0, 0.0])?;
    let (_, owners) = owner_roots(&maps, &[owner])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let mut work = ann_budget()?;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &maps,
        custody: &TestCustody,
        scratch: &NoScratch,
        budget: &mut work,
    };
    directory.put_delta(owner.owner, owner.member, &recipe, &mut context)?;
    // These runs are too large to be merged with the delta. The candidate
    // scan still has to validate the complete directory count.
    let fileset = store.insert(b"unopened region".to_vec(), Vec::new())?;
    let descriptor = ann_directory::RegionDescriptor {
        fileset,
        members: directory.delta,
        center: vec![1.0, 0.0],
        rows: 8192,
    };
    let children = ObjectChildren::from_references([Some(fileset), directory.delta.0, None, None]);
    let reference = store.insert(
        serde_json::to_vec(&descriptor)?,
        children.as_slice().to_vec(),
    )?;
    // The old rotation saw 2, 3, then 0, and stopped before seeing ordinal 1.
    let regions = maps.seed(
        (0..4u64)
            .map(|id| (id.to_be_bytes().to_vec(), reference))
            .collect(),
    )?;
    directory.regions = regions;
    directory.region_count = 3;
    directory.next_region = 4;
    directory.maintenance_after = Some(1);
    let delta = directory.delta;
    check!(matches!(
        directory.consolidated_members(&recipe, &mut context),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(directory.regions, regions);
    check_eq!(directory.region_count, 3);
    check_eq!(directory.maintenance_after, Some(1));
    check_eq!(directory.delta, delta);
    Ok(())
}

#[test]
fn consolidation_multi_page_carry_installs_one_final_member_map() -> TestResult {
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
    let larger_members = seal_many(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        selected.get(64..129).ok_or("next 65 owners missing")?,
        &mut setup,
    )?;
    // The smaller run has the higher ordinal, so carry order differs from
    // region-map key order.
    let _smaller_members = seal_many(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        selected.get(..64).ok_or("first 64 owners missing")?,
        &mut setup,
    )?;
    // The delta repeats the smaller run's exact selected member, then adds 63
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
    let pinned_regions = directory.regions;
    let expected_unlinks = maps
        .state(pinned_regions)?
        .into_iter()
        .map(|(key, reference)| MapEdit {
            key,
            expected: Some(reference.into()),
            next: None,
        })
        .collect::<Vec<_>>();
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

    // One final member map and one original region-map edit must fit in two
    // COW applications, including both authenticated region removals.
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(90))
        .ok_or("ANN consolidation test deadline overflow")?;
    let limits = WorkLimits {
        reads: 100_000,
        writes: 2,
        bytes: 1_500_000_000,
        mutations: 100_000,
    };
    let before = directory;
    let mut refused_work = WorkBudget::new(
        WorkLimits {
            writes: 1,
            ..limits
        },
        deadline,
    );
    let refused_memory = Arc::new(QueryMemory::default());
    refused_work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&refused_memory),
    )))));
    let refused_recording = RecordingMap::new(&maps, larger_members, false);
    let refused = directory.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &refused_recording,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut refused_work,
        },
    );
    check!(matches!(refused, Err(IndexError::ResourceLimited)));
    check_eq!(refused_work.first_denial(), Some(WorkBudgetDenial::Writes));
    check_eq!(refused_work.usage().writes, 1);
    let refused_applies = refused_recording.take_applies()?;
    let [(member_base, _), (region_base, removed)] = refused_applies.as_slice() else {
        return Err("ANN carry did not reach the final region CAS after member staging".into());
    };
    check_eq!(*member_base, larger_members);
    check_eq!(*region_base, pinned_regions);
    check_eq!(removed, &expected_unlinks);
    check_eq!(directory, before);
    check!(graph_reaches(&store, pinned, first_fileset)?);
    check_query_memory_released(&refused_memory)?;

    let repeated = selected.first().ok_or("repeated owner missing")?;
    let repeated_artifact =
        ann_directory::load_member(repeated.member, repeated.owner, &store, &mut setup)?.artifact;
    let objects = ReadOnceDescriptor::new(&store, repeated_artifact);
    let mut work = WorkBudget::new(limits, deadline);
    let memory = Arc::new(QueryMemory::default());
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&memory),
    )))));
    let recording = RecordingMap::new(&maps, larger_members, false);
    let result = directory.consolidated_members(
        &recipe,
        &mut AttemptContext {
            attempt: attempt(),
            objects: &objects,
            maps: &recording,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
    )?;
    check_eq!(objects.reads(), 1);
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
    let applies = recording.take_applies()?;
    let [(_, _), (region_base, removed)] = applies.as_slice() else {
        return Err("ANN carry did not install two final maps".into());
    };
    check_eq!(*region_base, pinned_regions);
    check_eq!(removed, &expected_unlinks);
    check_eq!(work.usage().writes, 2);
    check_eq!(directory.region_count, 0);
    check!(graph_reaches(&store, pinned, first_fileset)?);
    drop(result);
    check_query_memory_released(&memory)?;
    Ok(())
}

#[test]
fn consolidation_does_not_grow_artifact_table_for_repeated_run() -> TestResult {
    struct DenyArtifactBacking {
        inner: QueryMemoryScope,
        bytes: u64,
        refused: AtomicBool,
    }

    impl MemoryScopePort for DenyArtifactBacking {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if bytes == self.bytes && lifetime == MemoryLifetime::Temporary {
                self.refused.store(true, AtomicOrdering::SeqCst);
                return Err(IndexError::ResourceLimited);
            }
            self.inner.reserve(bytes, lifetime)
        }
    }

    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let selected = (0..64u8)
        .map(|id| selected_owner(&store, &spec, &recipe, id, b"selected input", [1.0, 0.0]))
        .collect::<TestResult<Vec<_>>>()?;
    let (_, owners) = owner_roots(&maps, &selected)?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    let mut setup = ann_budget()?;
    let prior_members = seal_many(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &selected,
        &mut setup,
    )?;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &maps,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut setup,
    };
    for item in &selected {
        directory.put_delta(item.owner, item.member, &recipe, &mut context)?;
    }
    let mut delta_only = directory;
    delta_only.regions = MapRoot(None);
    delta_only.region_count = 0;
    let baseline = delta_only.consolidated_members(&recipe, &mut context)?;
    check_eq!(baseline.artifacts.len(), selected.len());
    let delta_backing = u64::try_from(std::mem::size_of_val(baseline.artifacts.as_slice()))?;
    let duplicate_backing = delta_backing
        .checked_mul(2)
        .ok_or("duplicate carry metadata bound overflow")?;
    drop(baseline);

    check_eq!(directory.region_count, 1);
    // One stale delta member leaves 63 selected owners. Refusing the optional
    // 64-slot allocation must still admit the exact required backing.
    let live = selected.get(..63).ok_or("missing sparse delta fixture")?;
    delta_only.owners = owner_roots(&maps, live)?.1;
    let live_members = maps.seed(
        live.iter()
            .map(|item| (owner_map_key(item.owner).to_vec(), item.member))
            .collect(),
    )?;
    for (mut directory, expected_count, expected_map, denied_bytes, expect_refusal) in [
        (
            directory,
            selected.len(),
            prior_members,
            duplicate_backing,
            false,
        ),
        (delta_only, live.len(), live_members, delta_backing, true),
    ] {
        let mut work = ann_budget()?;
        let memory = Arc::new(QueryMemory::default());
        let scope = Arc::new(DenyArtifactBacking {
            inner: QueryMemoryScope(Arc::clone(&memory)),
            bytes: denied_bytes,
            refused: AtomicBool::new(false),
        });
        work.bind_memory_scope(Some(MemoryScope::new(scope.clone())));
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
        let result = result.map_err(|error| {
            format!(
                "carry allocation refused with {:?}: {error}",
                work.first_denial()
            )
        })?;
        check_eq!(work.first_denial(), None);
        check_eq!(scope.refused.load(AtomicOrdering::SeqCst), expect_refusal);
        check_eq!(result.artifacts.len(), expected_count);
        check_eq!(result.owners, u32::try_from(expected_count)?);
        check_eq!(result.rows, result.owners);
        check_eq!(maps.state(result.root)?, maps.state(expected_map)?);
        check_eq!(directory.region_count, 0);
        drop(result);
        check_query_memory_released(&memory)?;
    }
    Ok(())
}

#[test]
fn consolidation_reuses_largest_selected_root_with_exact_sparse_diff() -> TestResult {
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
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        selected.get(..6).ok_or("first six owners missing")?,
        &mut setup,
    )?;
    let largest_members = seal_many(
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
            writes: 2,
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
    check_eq!(work.usage().writes, 2);
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
            writes: 2,
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
    check_eq!(sparse_work.usage().writes, 2);
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

#[test]
fn native_region_routing_reports_ninth_region_loss_against_independent_exact() -> TestResult {
    for backend in [
        ann_directory::AnnBackend::Graph,
        ann_directory::AnnBackend::LanceDb,
    ] {
        check_ninth_region_routing(backend)?;
    }
    Ok(())
}

fn check_ninth_region_routing(backend: ann_directory::AnnBackend) -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let mut work = ann_budget()?;
    let mut selected = Vec::new();
    for id in 1..=8 {
        selected.push(selected_owner(
            &store,
            &spec,
            &recipe,
            id,
            &[id],
            [0.8, 0.6],
        )?);
    }
    selected.push(selected_owner(
        &store,
        &spec,
        &recipe,
        9,
        b"nearest",
        [1.0, 0.0],
    )?);
    selected.push(selected_owner(
        &store,
        &spec,
        &recipe,
        10,
        b"opposed",
        [-0.99995, 0.01],
    )?);
    let (sources, owners) = owner_roots(&maps, &selected)?;
    let mut directory =
        ann_directory::AnnDirectory::empty_with_backend(recipe.fingerprint(), owners, backend);
    for item in &selected[..8] {
        let _members = seal_many(
            &store,
            &maps,
            &custody,
            &recipe,
            &mut directory,
            std::slice::from_ref(item),
            &mut work,
        )?;
    }
    // The ninth region's actual center points almost perpendicular to query,
    // although its first original vector is the global nearest neighbor.
    let _members = seal_many(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &selected[8..],
        &mut work,
    )?;
    let reference = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let coverage = binding(&spec, &recipe, sources, owners, reference);
    let query = SearchQuery {
        source_files: matches!(backend, ann_directory::AnnBackend::LanceDb),
        lexical_eligibility: None,
        exact_path: None,
        coverage: &coverage,
        projection: None,
        source_owners: sources,
        source_paths: MapRoot(None),
        instance: spec.instance,
        format: spec.format,
        recipe: &recipe,
        vector: &[1.0, 0.0],
        top_k: 1,
        page_size: 64,
        max_owners: 80,
        overlay: None,
    };
    let exact = search_exact(
        &query,
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    check_eq!(exact.hits.first().ok_or("exact answer")?.owner, owner(9));
    let mut read = ReadContext {
        objects: &store,
        maps: &maps,
        budget: &mut work,
    };
    let directory = ann_directory::AnnDirectory::load_for_coverage(&coverage, &recipe, &mut read)?;
    let runtime = test_runtime()?;
    let ann =
        ann_query::search_loaded(&query, directory.as_ref(), None, &mut read, Some(&runtime))?;
    check_eq!(ann.hits.first().ok_or("routed answer")?.owner, owner(1));
    check_eq!(
        ann.traversal,
        NativeSemanticTraversal::Approximate { capped: true }
    );
    check!(!ann.exhaustive);
    Ok(())
}

#[test]
fn sealed_ann_reuses_member_and_artifact_across_vector_candidates() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let mut work = ann_budget()?;
    let mut chosen = selected_owner(&store, &spec, &recipe, 1, b"two vectors", [1.0, 0.0])?;
    chosen.member = member_with_vectors(&store, chosen.member, two_vectors(), &mut work)?;
    let stored_member = store.get(chosen.member, &mut work)?;
    let member: OwnerMemberBinding = serde_json::from_slice(&stored_member.payload)?;
    chosen.coverage = store.insert(
        serde_json::to_vec(&OwnerCoverage {
            owner: chosen.owner,
            desired: chosen.input,
            covered: Some(chosen.input),
            searchable: Some(chosen.member),
        })?,
        ObjectChildren::from_references([Some(chosen.source), Some(chosen.member), None, None])
            .as_slice()
            .to_vec(),
    )?;
    let (sources, owners) = owner_roots(&maps, &[chosen])?;
    let members = maps.seed(BTreeMap::from([(
        owner_map_key(chosen.owner).to_vec(),
        chosen.member,
    )]))?;
    for final_candidate in [b"a", b"0"] {
        let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
        let mut attempt_context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        let rows = [
            native_graph::RegionRow {
                owner: chosen.owner,
                member: member.logical_commitment,
                vector_id: final_candidate,
                vector: &[0.0, 1.0],
            },
            native_graph::RegionRow {
                owner: chosen.owner,
                member: member.logical_commitment,
                vector_id: b"b",
                vector: &[1.0, 0.0],
            },
        ];
        let fileset =
            native_graph::install(&rows, recipe.fingerprint(), 1, 2, &mut attempt_context)?;
        let _ = directory.append_region(
            &ann_directory::RegionDescriptor {
                fileset,
                members,
                center: vec![1.0, 0.0],
                rows: 2,
            },
            &mut attempt_context,
        )?;
        let reference = save_directory(&store, &maps, &custody, &directory, &mut work)?;
        let coverage = binding(&spec, &recipe, sources, owners, reference);
        for (source_files, path, reference) in [
            (false, None, chosen.member),
            (false, None, member.artifact),
            (false, None, chosen.source),
            (false, Some("src/1.rs"), chosen.source),
            (true, None, chosen.member),
            (true, None, member.artifact),
            (true, None, chosen.source),
            (true, Some("src/1.rs"), chosen.source),
        ] {
            if final_candidate == b"0" && reference != member.artifact {
                continue;
            }
            let mut partial_coverage = coverage;
            if path.is_some() {
                partial_coverage.covered = None;
                partial_coverage.status = CoverageStatus::Partial { missing: owners };
            }
            let memory = Arc::new(QueryMemory::default());
            let mut query_budget = ann_budget()?;
            query_budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
                Arc::clone(&memory),
            )))));
            let objects = ReadOnceDescriptor::new(&store, reference);
            let mut coverage_reads = RecordingMap::new(&maps, owners, false);
            coverage_reads.observe_lookup_key(owner_map_key(chosen.owner));
            let query = SearchQuery {
                source_files,
                lexical_eligibility: None,
                exact_path: path,
                coverage: &partial_coverage,
                projection: None,
                source_owners: sources,
                source_paths: MapRoot(None),
                instance: spec.instance,
                format: spec.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 2,
                page_size: 2,
                max_owners: 1,
                overlay: None,
            };
            let result = ann_query::search(
                &query,
                &mut ReadContext {
                    objects: &objects,
                    maps: &coverage_reads,
                    budget: &mut query_budget,
                },
            );
            check_eq!(objects.reads(), 1);
            check_eq!(coverage_reads.take_lookup_count(), 1);
            if final_candidate == b"0" {
                check_eq!(
                    result.err(),
                    Some(IndexError::CorruptObject(
                        "ANN candidate vector is absent from its member".into()
                    ))
                );
                check_query_memory_released(&memory)?;
                continue;
            }
            let result = result?;
            check_eq!(result.scanned_owners, 1);
            check_eq!(
                result.traversal,
                NativeSemanticTraversal::Approximate { capped: false }
            );
            check_eq!(
                result
                    .hits
                    .iter()
                    .map(|hit| (hit.vector_id.as_slice(), hit.cosine.to_bits()))
                    .collect::<Vec<_>>(),
                if source_files {
                    vec![(b"a".as_slice(), 1.0_f64.to_bits())]
                } else {
                    vec![
                        (b"a".as_slice(), 1.0_f64.to_bits()),
                        (b"b".as_slice(), 0.0_f64.to_bits()),
                    ]
                }
            );
            drop(result);
            check_query_memory_released(&memory)?;
            if !source_files && path.is_none() && reference == member.artifact {
                let objects = ReadOnceDescriptor::new(&store, member.artifact);
                let mut member_reads = RecordingMap::new(&maps, members, false);
                member_reads.observe_lookup_key(owner_map_key(chosen.owner));
                check_ann_scratch_before_overlay(
                    SearchQuery {
                        max_owners: 2,
                        ..query
                    },
                    owner(2),
                    &mut ReadContext {
                        objects: &objects,
                        maps: &member_reads,
                        budget: &mut ann_budget()?,
                    },
                    ann_query::search,
                )?;
                check_eq!(objects.reads(), 1);
                check_eq!(member_reads.take_lookup_count(), 2);
            }
        }
    }
    Ok(())
}

#[test]
fn sealed_ann_interleaved_members_fit_single_decode_memory_envelope() -> TestResult {
    struct RankingMemory {
        meter: QueryMemoryScope,
        first_decode: u64,
        largest_decode: u64,
        ranking_headroom: u64,
        ceiling: AtomicU64,
    }

    impl MemoryScopePort for RankingMemory {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            let live = self.meter.0.live.load(AtomicOrdering::SeqCst);
            if bytes == self.first_decode {
                let _previous = self.ceiling.compare_exchange(
                    u64::MAX,
                    live + self.largest_decode + self.ranking_headroom,
                    AtomicOrdering::SeqCst,
                    AtomicOrdering::SeqCst,
                );
            }
            if live + bytes > self.ceiling.load(AtomicOrdering::SeqCst) {
                return Err(IndexError::ResourceLimited);
            }
            self.meter.reserve(bytes, lifetime)
        }
    }

    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let mut work = ann_budget()?;
    let mut selected = [
        selected_owner(&store, &spec, &recipe, 1, b"first", [1.0, 0.0])?,
        selected_owner(&store, &spec, &recipe, 2, b"second", [1.0, 0.0])?,
    ];
    let mut decode = [0; 2];
    let mut source_decode = 0;
    let mut largest_metadata = 0;
    let mut artifacts = [None; 2];
    for (index, ((owner, allowance), artifact)) in selected
        .iter_mut()
        .zip(&mut decode)
        .zip(&mut artifacts)
        .enumerate()
    {
        let mut vectors = two_vectors();
        if index == 1 {
            vectors[1].values = vec![0.6, 0.8];
        }
        owner.member = member_with_vectors(&store, owner.member, vectors, &mut work)?;
        let member: OwnerMemberBinding =
            serde_json::from_slice(&store.get(owner.member, &mut work)?.payload)?;
        *artifact = Some(member.artifact);
        owner.commitment = member.logical_commitment;
        owner.coverage = store.insert(
            serde_json::to_vec(&OwnerCoverage {
                owner: owner.owner,
                desired: owner.input,
                covered: Some(owner.input),
                searchable: Some(owner.member),
            })?,
            ObjectChildren::from_references([Some(owner.source), Some(owner.member), None, None])
                .as_slice()
                .to_vec(),
        )?;
        *allowance = u64::try_from(store.get(member.artifact, &mut work)?.payload.len())? * 8;
        source_decode += u64::try_from(store.get(owner.source, &mut work)?.payload.len())? * 8;
        for reference in [owner.coverage, owner.member] {
            let bytes = u64::try_from(store.get(reference, &mut work)?.payload.len())?;
            largest_metadata = largest_metadata.max(bytes * 8 + 512);
        }
    }
    let (sources, owners) = owner_roots(&maps, &selected)?;
    let members = maps.seed(BTreeMap::from(
        selected.map(|owner| (owner_map_key(owner.owner).to_vec(), owner.member)),
    ))?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    // Graph order is A:b, B:b, A:a, B:a. The original a vectors score first.
    let rows = [
        (0, b"a", [0.6, 0.8]),
        (0, b"b", [1.0, 0.0]),
        (1, b"a", [0.0, 1.0]),
        (1, b"b", [0.8, 0.6]),
    ];
    let rows = rows
        .each_ref()
        .map(|(index, id, vector)| native_graph::RegionRow {
            owner: selected[*index].owner,
            member: selected[*index].commitment,
            vector_id: *id,
            vector,
        });
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &maps,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    };
    let fileset = native_graph::install(&rows, recipe.fingerprint(), 1, 2, &mut context)?;
    let _ = directory.append_region(
        &ann_directory::RegionDescriptor {
            fileset,
            members,
            center: vec![1.0, 0.0],
            rows: 4,
        },
        &mut context,
    )?;
    let reference = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let coverage = binding(&spec, &recipe, sources, owners, reference);
    let graph = native_graph::load(&store, fileset, &mut work)?;
    check_eq!(
        graph.candidates(&[1.0, 0.0], 128, 128, &mut work)?.indices,
        [1, 3, 0, 2]
    );
    let [Some(first_artifact), Some(second_artifact)] = artifacts else {
        return Err("ANN fixture is missing member artifacts".into());
    };
    for (source_files, top_k, artifact) in [
        (false, 4, first_artifact),
        (false, 4, second_artifact),
        (true, 2, first_artifact),
        (true, 2, second_artifact),
        // B:b evicts A:b before A:a wins. The decoded member must remain
        // reusable even after the result heap drops A's source metadata.
        (true, 1, first_artifact),
    ] {
        let memory = Arc::new(QueryMemory::default());
        let scope = Arc::new(RankingMemory {
            meter: QueryMemoryScope(Arc::clone(&memory)),
            first_decode: decode[0],
            largest_decode: largest_metadata.max(decode[0]).max(decode[1]),
            // One source per owner and room for the ASCII result IDs.
            ranking_headroom: source_decode + 16,
            ceiling: AtomicU64::new(u64::MAX),
        });
        let mut budget = ann_budget()?;
        budget.bind_memory_scope(Some(MemoryScope::new(scope.clone())));
        let objects = ReadOnceDescriptor::new(&store, artifact);
        let result = ann_query::search(
            &SearchQuery {
                source_files,
                lexical_eligibility: None,
                exact_path: None,
                coverage: &coverage,
                projection: None,
                source_owners: sources,
                source_paths: MapRoot(None),
                instance: spec.instance,
                format: spec.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k,
                page_size: 2,
                max_owners: 2,
                overlay: None,
            },
            &mut ReadContext {
                objects: &objects,
                maps: &maps,
                budget: &mut budget,
            },
        );
        check!(scope.ceiling.load(AtomicOrdering::SeqCst) < u64::MAX);
        if let Err(ref error) = result {
            return Err(format!("source_files={source_files}: {error:?}; {budget:?}").into());
        }
        check_eq!(budget.first_denial(), None);
        let result = result?;
        check_eq!(objects.reads(), 1);
        let mut expected = vec![(owner(1), b"a", 1.0_f64)];
        if top_k > 1 {
            expected.push((owner(2), b"a", 1.0_f64));
        }
        if !source_files {
            expected.extend([(owner(2), b"b", 0.6_f64), (owner(1), b"b", 0.0_f64)]);
        }
        check_eq!(result.hits.len(), expected.len());
        for (hit, (owner, id, score)) in result.hits.iter().zip(expected) {
            check_eq!(
                (hit.owner, hit.vector_id.as_slice()),
                (owner, id.as_slice())
            );
            check!((hit.cosine - score).abs() < 1.0e-7);
        }
        drop(result);
        check_query_memory_released(&memory)?;
    }
    Ok(())
}

#[test]
fn native_graph_candidates_are_reranked_from_original_member_vectors() -> TestResult {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let first = selected_owner(&store, &spec, &recipe, 1, b"original nearest", [1.0, 0.0])?;
    let second = selected_owner(
        &store,
        &spec,
        &recipe,
        2,
        b"graph-space nearest",
        [0.0, 1.0],
    )?;
    let (sources, owners) = owner_roots(&maps, &[first, second])?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), owners);
    // The search layout can order candidates; only authenticated original
    // member vectors can authorize their final returned scores.
    let distorted = [
        SelectedOwner {
            vector: [0.0, 1.0],
            ..first
        },
        SelectedOwner {
            vector: [1.0, 0.0],
            ..second
        },
    ];
    let mut work = ann_budget()?;
    let _members = seal_many(
        &store,
        &maps,
        &custody,
        &recipe,
        &mut directory,
        &distorted,
        &mut work,
    )?;
    let descriptor = directory.region_at(
        0,
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    let graph = native_graph::load(&store, descriptor.descriptor.fileset, &mut work)?;
    let raw = graph.candidates(&[1.0, 0.0], 128, 128, &mut work)?;
    let index = *raw.indices.first().ok_or("raw graph candidate")?;
    check_eq!(graph.rows[index].owner, owner(2));
    let reference = save_directory(&store, &maps, &custody, &directory, &mut work)?;
    let coverage = binding(&spec, &recipe, sources, owners, reference);
    let result = ann_query::search(
        &SearchQuery {
            source_files: true,
            lexical_eligibility: None,
            exact_path: None,
            coverage: &coverage,
            projection: None,
            source_owners: sources,
            source_paths: MapRoot(None),
            instance: spec.instance,
            format: spec.format,
            recipe: &recipe,
            vector: &[1.0, 0.0],
            top_k: 1,
            page_size: 64,
            max_owners: 16,
            overlay: None,
        },
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    let hit = result.hits.first().ok_or("authoritative reranked file")?;
    check_eq!(hit.owner, owner(1));
    check_eq!(hit.cosine.total_cmp(&1.0), Ordering::Equal);
    let file = hit.source_file().ok_or("original source witness")?;
    check_eq!(file.owner, owner(1));
    check_eq!(file.source, first.source);
    check!(!result.exhaustive);
    Ok(())
}
