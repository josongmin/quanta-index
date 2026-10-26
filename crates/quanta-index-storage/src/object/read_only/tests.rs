//! Physical closure laws use actual immutable files and an existing-only FD
//! port. Common-capacity lifetime qualification stays with Runtime's IO laws.

use std::{
    fs::{self, File},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use quanta_index_core::{
    RequestBudgetV1,
    index::{MemoryScope, MemoryScopePort, WorkLimits},
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
    Ok(WorkBudget::new(
        WorkLimits {
            reads,
            writes: 100,
            bytes: 1_000_000,
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
