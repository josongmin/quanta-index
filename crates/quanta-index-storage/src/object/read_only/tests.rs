//! Physical closure laws use actual immutable files and an existing-only FD
//! port. Common-capacity lifetime qualification stays with Runtime's IO laws.

use std::{
    fs::{self, File},
    io::Write as _,
    os::unix::fs::{FileExt as _, OpenOptionsExt as _, PermissionsExt as _, symlink},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use quanta_index_core::{
    RequestBudgetV1,
    index::{
        MapEdit, MapRoot, MapValue, MemoryLeaseOwner, MemoryScope, MemoryScopePort,
        RegisteredObject, WorkLimits,
        ports::{ObjectWriteSink, PersistentMapPort as _},
    },
};

use super::*;
use crate::FileObjectStore;
use quanta_index_core::index::ports::ImmutableObjectPort as _;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn require(value: bool) -> Result<(), IndexError> {
    if value {
        Ok(())
    } else {
        Err(IndexError::BackendViolation("closure law failed".into()))
    }
}

fn budget(reads: u64) -> Result<WorkBudget, IndexError> {
    budget_with_bytes(reads, 1_000_000)
}

fn budget_with_bytes(reads: u64, bytes: u64) -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads,
            writes: 100,
            bytes,
            mutations: 100,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn private_root() -> Result<tempfile::TempDir, std::io::Error> {
    let root = tempfile::tempdir()?;
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
    Ok(root)
}

#[derive(Debug, Default)]
struct Reads(Mutex<Vec<PathBuf>>);

struct Held(File);

impl ObjectReadFile for Held {
    fn file(&self) -> Result<&File, IndexError> {
        Ok(&self.0)
    }
}

impl ObjectReadIoPort for Reads {
    fn open(
        &self,
        path: &Path,
        work: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
        work.checkpoint()?;
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(super::super::nofollow_flag()?)
            .open(path)
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        self.0
            .lock()
            .map_err(|_error| IndexError::Conflict)?
            .push(path.to_path_buf());
        Ok(Box::new(Held(file)))
    }
}

fn reader(root: &Path, reads: Arc<Reads>) -> Result<FileObjectReader, IndexError> {
    let lock = reads.open(&root.join(".object-namespace.lease"), &mut budget(100)?)?;
    FileObjectReader::open_existing(root, lock, reads, &mut budget(100)?)
}

struct AcceptMapInstall;

impl ObjectWriteSink for AcceptMapInstall {
    fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
        Ok(())
    }

    fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
        Ok(())
    }

    fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
        Ok(())
    }

    fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
        Ok(())
    }
}

#[test]
fn existing_only_reader_scans_immutable_map_pages_without_writer() -> TestResult {
    let root = private_root()?;
    let store = FileObjectStore::open(root.path())?;
    let mut build = budget(100)?;
    let value = store.put(b"history row", &[], &mut build)?.reference;
    let keys = [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()];
    let edits = keys
        .iter()
        .map(|key| MapEdit::objects(key.clone(), None, Some(value)))
        .collect::<Vec<_>>();
    let map = store
        .apply(MapRoot(None), &edits, &AcceptMapInstall, &mut build)?
        .root;
    drop(store);

    let reads = Arc::new(Reads::default());
    let reader = reader(root.path(), Arc::clone(&reads))?;
    let mut work = budget(100)?;
    let first = reader.scan_map(map, None, 2, &mut work)?;
    require(
        first.entries
            == keys[..2]
                .iter()
                .cloned()
                .map(|key| (key, MapValue::Object(value)))
                .collect::<Vec<_>>(),
    )?;
    require(first.has_more)?;
    let second = reader.scan_map(map, Some(b"b"), 2, &mut work)?;
    require(second.entries == vec![(b"c".to_vec(), MapValue::Object(value))])?;
    require(!second.has_more && work.usage().writes == 0)?;
    require(matches!(
        reader.scan_map(map, None, 0, &mut work),
        Err(IndexError::InvalidInput(_))
    ))?;
    require(reads.0.lock().map_err(|_error| IndexError::Conflict)?.len() > 1)?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum NamespaceTamper {
    RootReplacement,
    RootSymlink,
    RootMode,
    ObjectsReplacement,
    ObjectsSymlink,
    ObjectsMode,
}

impl NamespaceTamper {
    fn apply(self, root: &Path) -> Result<(), std::io::Error> {
        let objects = root.join("objects");
        match self {
            Self::RootReplacement => {
                fs::rename(root, root.with_file_name("moved-root"))?;
                fs::create_dir(root)?;
                fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            }
            Self::RootSymlink => {
                fs::rename(root, root.with_file_name("moved-root"))?;
                symlink(root.with_file_name("moved-root"), root)
            }
            Self::RootMode => fs::set_permissions(root, fs::Permissions::from_mode(0o755)),
            Self::ObjectsReplacement => {
                fs::rename(&objects, root.join("objects-moved"))?;
                fs::create_dir(&objects)?;
                fs::set_permissions(objects, fs::Permissions::from_mode(0o700))
            }
            Self::ObjectsSymlink => {
                fs::rename(&objects, root.join("objects-moved"))?;
                symlink(root.join("objects-moved"), objects)
            }
            Self::ObjectsMode => fs::set_permissions(objects, fs::Permissions::from_mode(0o755)),
        }
    }

    fn restore(self, root: &Path) -> Result<(), std::io::Error> {
        let objects = root.join("objects");
        match self {
            Self::RootReplacement => {
                fs::remove_dir(root)?;
                fs::rename(root.with_file_name("moved-root"), root)
            }
            Self::RootSymlink => {
                fs::remove_file(root)?;
                fs::rename(root.with_file_name("moved-root"), root)
            }
            Self::RootMode => fs::set_permissions(root, fs::Permissions::from_mode(0o700)),
            Self::ObjectsReplacement => {
                fs::remove_dir(&objects)?;
                fs::rename(root.join("objects-moved"), objects)
            }
            Self::ObjectsSymlink => {
                fs::remove_file(&objects)?;
                fs::rename(root.join("objects-moved"), objects)
            }
            Self::ObjectsMode => fs::set_permissions(objects, fs::Permissions::from_mode(0o700)),
        }
    }
}

#[test]
fn reader_rechecks_live_root_and_objects_before_opening_data_fd() -> TestResult {
    for tamper in [
        NamespaceTamper::RootReplacement,
        NamespaceTamper::RootSymlink,
        NamespaceTamper::RootMode,
        NamespaceTamper::ObjectsReplacement,
        NamespaceTamper::ObjectsSymlink,
        NamespaceTamper::ObjectsMode,
    ] {
        let parent = private_root()?;
        let root = parent.path().join("store");
        let store = FileObjectStore::open(&root)?;
        let object = store.put(b"held namespace", &[], &mut budget(100)?)?;
        drop(store);
        let reads = Arc::new(Reads::default());
        let reader = reader(&root, Arc::clone(&reads))?;
        let opened_before = reads.0.lock().map_err(|_error| IndexError::Conflict)?.len();
        require(opened_before == 1)?; // Only the held namespace-lock FD.

        tamper.apply(&root)?;
        let mut work = budget(100)?;
        require(matches!(
            reader.get(object.reference, &mut work),
            Err(IndexError::Storage(_))
        ))?;
        require(work.usage().reads == 1)?;
        require(reads.0.lock().map_err(|_error| IndexError::Conflict)?.len() == opened_before)?;

        tamper.restore(&root)?;
        require(reader.get(object.reference, &mut budget(100)?)?.payload == b"held namespace")?;
        require(reads.0.lock().map_err(|_error| IndexError::Conflict)?.len() == opened_before + 1)?;
    }
    Ok(())
}

#[test]
fn closure_reads_all_roots_shared_children_and_nonempty_leaf_bytes() -> TestResult {
    let root = private_root()?;
    let store = FileObjectStore::open(root.path())?;
    let mut work = budget(100)?;
    let leaf = store.put(b"fixed leaf bytes", &[], &mut work)?.reference;
    let left = store.put(b"left", &[leaf], &mut work)?.reference;
    let right = store.put(b"right", &[leaf], &mut work)?.reference;
    let parent = store.put(b"parent", &[left, right], &mut work)?.reference;
    let independent = store.put(b"independent", &[], &mut work)?.reference;
    drop(store);
    let reads = Arc::new(Reads::default());
    let reader = reader(root.path(), Arc::clone(&reads))?;
    let mut verify = budget(100)?;
    reader.verify_closure(&[parent, independent], 3, &mut verify)?;
    // Parent, two branches, shared leaf twice, and the independent root.
    require(verify.usage().reads == 6 && verify.usage().writes == 0)?;
    require(reads.0.lock().map_err(|_error| IndexError::Conflict)?.len() == 7)?;
    Ok(())
}

#[test]
fn single_object_verification_streams_payload_and_authenticates_edges() -> TestResult {
    let root = private_root()?;
    let store = FileObjectStore::open(root.path())?;
    let mut build = budget(100)?;
    let child = store.put(b"child", &[], &mut build)?.reference;
    let payload = vec![0xa5; 128 * 1024];
    let object = store.put(&payload, &[child], &mut build)?.reference;
    drop(store);

    let reader = reader(root.path(), Arc::new(Reads::default()))?;
    let live = Arc::new(ReadMemory::default());
    let mut verify = budget(3)?;
    verify.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedReadMemory {
        live: Arc::clone(&live),
        limit: 96 * 1024,
    }))));
    reader.verify_object(object, &mut verify)?;
    require(verify.usage().reads == 1 && verify.usage().writes == 0)?;
    require(live.0.load(Ordering::SeqCst) == 0)?;

    let path = super::super::read_io::object_path(&root.path().join("objects"), object);
    let original = fs::read(&path)?;
    for offset in [super::super::HEADER_LEN, original.len() - 1] {
        let mut corrupted = original.clone();
        corrupted[offset] ^= 1;
        fs::write(&path, corrupted)?;
        require(matches!(
            reader.verify_object(object, &mut verify),
            Err(IndexError::CorruptObject(message)) if message == "object digest or size mismatch"
        ))?;
        require(live.0.load(Ordering::SeqCst) == 0)?;
    }
    Ok(())
}

#[test]
fn closure_selected_roots_reuse_authenticated_bodies_without_extra_reads() -> TestResult {
    let root = private_root()?;
    let store = FileObjectStore::open(root.path())?;
    let mut work = budget(100)?;
    let leaf = store.put(b"fixed leaf", &[], &mut work)?.reference;
    let request = store.put(b"request body", &[leaf], &mut work)?.reference;
    let profile = store.put(b"profile body", &[], &mut work)?.reference;
    let large = vec![0xa5; 128 * 1024];
    let independent = store.put(&large, &[], &mut work)?.reference;
    drop(store);
    let reads = Arc::new(Reads::default());
    let reader = reader(root.path(), Arc::clone(&reads))?;
    let mut verify = budget(100)?;
    let live = Arc::new(ReadMemory::default());
    verify.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedReadMemory {
        live: Arc::clone(&live),
        limit: 96 * 1024,
    }))));
    let [stored_request, stored_profile] = reader.verify_closure_with_selected_roots(
        &[request, profile, independent],
        [request, profile],
        3,
        &mut verify,
    )?;
    require(stored_request.registration.reference == request)?;
    require(stored_request.payload == b"request body")?;
    require(stored_profile.registration.reference == profile)?;
    require(stored_profile.payload == b"profile body")?;
    // Request, leaf, profile, independent. The two returned bodies need no get.
    require(verify.usage().reads == 4)?;
    require(reads.0.lock().map_err(|_error| IndexError::Conflict)?.len() == 5)?;
    drop(stored_request);
    drop(stored_profile);
    require(live.0.load(Ordering::SeqCst) == 0)?;
    Ok(())
}

#[test]
fn closure_selected_roots_refuse_corrupt_transitive_child() -> TestResult {
    let root = private_root()?;
    let store = FileObjectStore::open(root.path())?;
    let mut work = budget(100)?;
    let large = vec![0xa5; 128 * 1024];
    let leaf = store.put(&large, &[], &mut work)?.reference;
    let request = store.put(b"request body", &[leaf], &mut work)?.reference;
    let profile = store.put(b"profile body", &[], &mut work)?.reference;
    drop(store);
    let leaf_path = super::super::read_io::object_path(&root.path().join("objects"), leaf);
    let mut wire = fs::read(&leaf_path)?;
    let byte = wire.last_mut().ok_or(IndexError::Conflict)?;
    *byte ^= 1;
    fs::write(&leaf_path, wire)?;
    let reader = reader(root.path(), Arc::new(Reads::default()))?;
    let live = Arc::new(ReadMemory::default());
    let mut verify = budget(100)?;
    verify.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedReadMemory {
        live: Arc::clone(&live),
        limit: 96 * 1024,
    }))));
    require(matches!(
        reader.verify_closure_with_selected_roots(&[request, profile], [request, profile], 2, &mut verify),
        Err(IndexError::CorruptObject(message)) if message == "object digest or size mismatch"
    ))?;
    require(live.0.load(Ordering::SeqCst) == 0)?;
    Ok(())
}

#[test]
fn closure_refuses_missing_or_corrupt_descendant_and_never_accepts_a_prefix() -> TestResult {
    for missing in [false, true] {
        let root = private_root()?;
        let store = FileObjectStore::open(root.path())?;
        let mut work = budget(100)?;
        let leaf = store.put(b"fixed child", &[], &mut work)?.reference;
        let parent = store.put(b"fixed parent", &[leaf], &mut work)?.reference;
        let leaf_path = super::super::read_io::object_path(&root.path().join("objects"), leaf);
        drop(store);
        if missing {
            fs::remove_file(&leaf_path)?;
        } else {
            let mut wire = fs::read(&leaf_path)?;
            let byte = wire.last_mut().ok_or(IndexError::Conflict)?;
            *byte ^= 1;
            fs::write(&leaf_path, wire)?;
        }
        let reads = Arc::new(Reads::default());
        let reader = reader(root.path(), reads)?;
        let before = if missing {
            None
        } else {
            Some(fs::read(&leaf_path)?)
        };
        require(matches!(
            reader.verify_closure(&[parent], 2, &mut budget(100)?),
            Err(IndexError::CorruptObject(_))
        ))?;
        let after = match fs::read(&leaf_path) {
            Ok(bytes) => Some(bytes),
            Err(error) if missing && error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        require(after == before)?;
    }
    Ok(())
}

struct DenyMemory;
impl MemoryScopePort for DenyMemory {
    fn reserve(&self, _: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        Err(IndexError::ResourceLimited)
    }
}

#[test]
fn closure_frontier_memory_work_and_original_cancel_refuse_without_reset() -> TestResult {
    let root = private_root()?;
    let store = FileObjectStore::open(root.path())?;
    let mut work = budget(100)?;
    let left = store.put(b"left", &[], &mut work)?.reference;
    let right = store.put(b"right", &[], &mut work)?.reference;
    let parent = store.put(b"parent", &[left, right], &mut work)?.reference;
    drop(store);
    let reads = Arc::new(Reads::default());
    let reader = reader(root.path(), Arc::clone(&reads))?;
    let live = Arc::new(ReadMemory::default());
    let mut bounded = budget(3)?;
    bounded.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedReadMemory {
        live: Arc::clone(&live),
        limit: 64 * 1024,
    }))));
    // The installed fixture's entry ceiling bounds pending references, even
    // when this closure needs only two slots. All three files must be read.
    reader.verify_closure(&[parent], 500_000, &mut bounded)?;
    require(bounded.usage().reads == 3)?;
    require(live.0.load(Ordering::SeqCst) == 0)?;
    require(matches!(
        reader.verify_closure(&[parent], 1, &mut budget(100)?),
        Err(IndexError::ResourceLimited)
    ))?;
    let before = reads.0.lock().map_err(|_error| IndexError::Conflict)?.len();
    let mut denied = budget(100)?;
    denied.bind_memory_scope(Some(MemoryScope::new(Arc::new(DenyMemory))));
    require(matches!(
        reader.verify_closure(&[parent], 2, &mut denied),
        Err(IndexError::ResourceLimited)
    ))?;
    require(reads.0.lock().map_err(|_error| IndexError::Conflict)?.len() == before)?;
    let mut one_read = budget(1)?;
    require(matches!(
        reader.verify_closure(&[parent], 2, &mut one_read),
        Err(IndexError::ResourceLimited)
    ))?;
    require(one_read.usage().reads == 1)?;
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let mut cancelled = WorkBudget::with_request(
        WorkLimits {
            reads: 100,
            writes: 0,
            bytes: 1_000_000,
            mutations: 0,
        },
        request.clone(),
    );
    request.cancel_handle().cancel();
    let before = reads.0.lock().map_err(|_error| IndexError::Conflict)?.len();
    require(matches!(
        reader.verify_closure(&[parent], 2, &mut cancelled),
        Err(IndexError::Cancelled)
    ))?;
    require(reads.0.lock().map_err(|_error| IndexError::Conflict)?.len() == before)?;
    Ok(())
}

#[derive(Clone, Copy)]
enum ReadMutation {
    Truncate,
    Append,
}

#[derive(Default)]
struct ReadMemory(AtomicU64);

impl MemoryLeaseOwner for ReadMemory {
    fn release(&self, bytes: u64) {
        let _previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
    }
}

struct MutateOnBodyReserve {
    path: PathBuf,
    mutation: ReadMutation,
    calls: AtomicUsize,
    memory: Arc<ReadMemory>,
}

impl MemoryScopePort for MutateOnBodyReserve {
    fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        // Mutate the opened inode at payload admission, after the header read.
        if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
            match self.mutation {
                ReadMutation::Truncate => {
                    fs::OpenOptions::new()
                        .write(true)
                        .open(&self.path)
                        .and_then(|file| file.set_len(super::super::HEADER_LEN as u64))
                        .map_err(|error| IndexError::Storage(error.to_string()))?;
                }
                ReadMutation::Append => {
                    fs::OpenOptions::new()
                        .append(true)
                        .open(&self.path)
                        .and_then(|mut file| file.write_all(b"!"))
                        .map_err(|error| IndexError::Storage(error.to_string()))?;
                }
            }
        }
        let _previous = self.memory.0.fetch_add(bytes, Ordering::SeqCst);
        Ok(MemoryLease::new(bytes, self.memory.clone()))
    }
}

#[test]
fn physical_get_refuses_short_read_and_appended_eof_after_opened_metadata() -> TestResult {
    for read_only in [false, true] {
        for mutation in [ReadMutation::Truncate, ReadMutation::Append] {
            let root = private_root()?;
            let store = FileObjectStore::open(root.path())?;
            let reference = store
                .put(b"immutable payload", &[], &mut budget(100)?)?
                .reference;
            let path = super::super::read_io::object_path(&root.path().join("objects"), reference);
            let original_len = fs::metadata(&path)?.len();
            let memory = Arc::new(ReadMemory::default());
            let hook = Arc::new(MutateOnBodyReserve {
                path: path.clone(),
                mutation,
                calls: AtomicUsize::new(0),
                memory: Arc::clone(&memory),
            });
            let mut work = budget(100)?;
            work.bind_memory_scope(Some(MemoryScope::new(hook.clone())));
            let result = if read_only {
                drop(store);
                let reader = reader(root.path(), Arc::new(Reads::default()))?;
                reader.get(reference, &mut work)
            } else {
                store.get(reference, &mut work)
            };
            require(match (mutation, result) {
                (ReadMutation::Truncate, Err(IndexError::CorruptObject(message))) => {
                    message == "object truncated during read"
                }
                (ReadMutation::Append, Err(IndexError::CorruptObject(message))) => {
                    message == "object digest or size mismatch"
                }
                _ => false,
            })?;
            require(hook.calls.load(Ordering::SeqCst) == 2)?;
            require(memory.0.load(Ordering::SeqCst) == 0)?;
            let observed_len = fs::metadata(path)?.len();
            require(match mutation {
                ReadMutation::Truncate => observed_len == super::super::HEADER_LEN as u64,
                ReadMutation::Append => observed_len == original_len + 1,
            })?;
        }
    }
    Ok(())
}

struct BoundedReadMemory {
    live: Arc<ReadMemory>,
    limit: u64,
}

impl MemoryScopePort for BoundedReadMemory {
    fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous = self
            .live
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                live.checked_add(bytes).filter(|total| *total <= self.limit)
            })
            .map_err(|_live| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.live.clone()))
    }
}

#[test]
fn physical_get_fits_one_payload_and_refunds_at_last_drop() -> TestResult {
    const PAYLOAD_BYTES: usize = 128 * 1024;
    const ONE_PAYLOAD_LIMIT: u64 = 192 * 1024;
    let payload = vec![0xa5; PAYLOAD_BYTES];
    for read_only in [false, true] {
        let root = private_root()?;
        let store = FileObjectStore::open(root.path())?;
        let mut build = budget(100)?;
        let child = store.put(b"child", &[], &mut build)?.reference;
        let expected = store.put(&payload, &[child], &mut build)?;
        let live = Arc::new(ReadMemory::default());
        let mut work = budget(100)?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedReadMemory {
            live: Arc::clone(&live),
            limit: ONE_PAYLOAD_LIMIT,
        }))));
        let stored = if read_only {
            drop(store);
            reader(root.path(), Arc::new(Reads::default()))?.get(expected.reference, &mut work)?
        } else {
            store.get(expected.reference, &mut work)?
        };
        require(stored.payload == payload && stored.registration == expected)?;
        let retained = StoredObject::allocation_bytes(PAYLOAD_BYTES, 1)?;
        let last = stored.clone();
        drop(stored);
        require(live.0.load(Ordering::SeqCst) == retained)?;
        drop(last);
        require(live.0.load(Ordering::SeqCst) == 0)?;
    }
    Ok(())
}

#[test]
fn physical_get_reports_digest_corruption_even_without_decode_budget() -> TestResult {
    for read_only in [false, true] {
        for offset in [0, u64::try_from(super::super::HEADER_LEN)?] {
            let root = private_root()?;
            let store = FileObjectStore::open(root.path())?;
            let expected = store.put(b"immutable wire corruption", &[], &mut budget(100)?)?;
            let path = super::super::read_io::object_path(
                &root.path().join("objects"),
                expected.reference,
            );
            fs::OpenOptions::new()
                .write(true)
                .open(path)?
                .write_all_at(b"!", offset)?;
            let mut work = budget_with_bytes(1, expected.bytes)?;
            let result = if read_only {
                drop(store);
                reader(root.path(), Arc::new(Reads::default()))?.get(expected.reference, &mut work)
            } else {
                store.get(expected.reference, &mut work)
            };
            require(matches!(
                result,
                Err(IndexError::CorruptObject(message)) if message == "object digest or size mismatch"
            ))?;
        }
    }
    Ok(())
}
