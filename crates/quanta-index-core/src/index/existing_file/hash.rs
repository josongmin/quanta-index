//! One admitted bridge to the canonical sealed-artifact hasher.
use super::ExistingFileReadFailureV1 as Failure;
use crate::{
    CoreError, SealedArtifactCommitmentV1, TreeScrubVerdictV1, hash_committed_step_opened_v1,
    index::{IndexError, MemoryLifetime, ObjectReadFile, ObjectReadIoPort, WorkBudget, WorkUsage},
};
use quanta_index_contract::SearchPlaneErrorCodeV2;
use std::{
    cell::RefCell,
    fs::Metadata,
    io::{self, Read},
    os::unix::fs::{FileExt as _, MetadataExt as _},
    path::{Path, PathBuf},
};
fn corrupt(message: impl Into<String>) -> Failure {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        message: message.into(),
    }
    .into()
}
fn memory(budget: &WorkBudget, bytes: u64) -> Result<crate::index::MemoryLease, Failure> {
    budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited.into())
}
fn width(bytes: usize) -> Result<u64, Failure> {
    u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited.into())
}
fn canonical_relative(name: &str) -> bool {
    !name.is_empty() && name.split('/').all(|part| !matches!(part, "" | "." | ".."))
}
struct HashReader<'a, 'b> {
    opened: Box<dyn ObjectReadFile>,
    path: &'a Path,
    before: Metadata,
    io: &'a dyn ObjectReadIoPort,
    budget: &'a RefCell<&'b mut WorkBudget>,
    failure: &'a RefCell<Option<Failure>>,
    position: u64,
}

impl HashReader<'_, '_> {
    fn read_checked(&mut self, bytes: &mut [u8]) -> Result<usize, Failure> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let mut budget = self
            .budget
            .try_borrow_mut()
            .map_err(|_| IndexError::Conflict)?;
        // Admit the requested read and canonical SHA work before the syscall.
        let work = width(bytes.len())?
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?;
        let file = self.opened.file()?;
        let count = loop {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: work,
                ..WorkUsage::default()
            })?;
            match file.read_at(bytes, self.position) {
                Ok(count) => break count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        };
        self.position = self
            .position
            .checked_add(width(count)?)
            .ok_or(IndexError::ResourceLimited)?;
        if count == 0 {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            if super::Identity::from(&self.before) != super::Identity::from(&file.metadata()?) {
                return Err(corrupt("committed payload changed during hashing"));
            }
            // Reopen through the SAME genuine port, never clone/extract an FD
            // or perform a raw path open/stat. Keep the original open through
            // this admitted path-to-inode comparison. Actual capacity can refuse.
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let current = self.io.open(self.path, &mut budget)?;
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            if super::Identity::from(&self.before)
                != super::Identity::from(&current.file()?.metadata()?)
            {
                return Err(corrupt("committed payload path changed during hashing"));
            }
        }
        Ok(count)
    }
}

impl Read for HashReader<'_, '_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self.read_checked(bytes) {
            Ok(count) => Ok(count),
            Err(error) => {
                let mut failure = self
                    .failure
                    .try_borrow_mut()
                    .map_err(|_| io::Error::other("committed failure slot is borrowed"))?;
                if failure.is_none() {
                    *failure = Some(error);
                }
                Err(io::Error::other("committed admitted hash read refused"))
            }
        }
    }
}

fn committed_path(
    generation: &Path,
    name: &str,
    budget: &WorkBudget,
) -> Result<(PathBuf, crate::index::MemoryLease), Failure> {
    if !canonical_relative(name) {
        return Err(corrupt("committed payload path is not canonical"));
    }
    let path_bytes = width(generation.as_os_str().len())?
        .checked_add(width(name.len())?)
        .and_then(|n| n.checked_add(1))
        .ok_or(IndexError::ResourceLimited)?;
    let grant = memory(budget, path_bytes)?;
    let mut path = PathBuf::new();
    path.try_reserve_exact(usize::try_from(path_bytes).map_err(|_| IndexError::ResourceLimited)?)
        .map_err(|_| IndexError::ResourceLimited)?;
    path.push(generation);
    path.push(name);
    Ok((path, grant))
}

/// Read and authenticate bounded committed DATA for a canonical owner decoder.
/// The same physical reader and Core hasher are reused; this admits inherited
/// hardlinks and empty payloads while the sidecar API retains nlink==1/nonempty.
/// The returned original input grant stays live with its bytes; the owner
/// decoder must separately fund its stronger decoded representation. A final
/// same-port reopen checks the path/inode again after hashing. The caller must
/// still retain its actual root and require physical retirement before publish.
pub fn read_existing_committed_file_v1(
    generation: &Path,
    artifact: &SealedArtifactCommitmentV1,
    owner: u32,
    io: &dyn ObjectReadIoPort,
    budget: &mut WorkBudget,
) -> Result<super::ExistingFileBytesV1, Failure> {
    let _hash_memory = memory(budget, 1 << 16)?;
    let (path, _path_memory) = committed_path(generation, &artifact.name, budget)?;
    let input = super::read_existing_file_with_policy(
        &path,
        owner,
        artifact.bytes,
        io,
        budget,
        super::ReadPolicy::Committed,
    )?;
    if width(input.bytes_v1().len())? != artifact.bytes {
        return Err(corrupt("committed input length differs from its seal"));
    }
    // Charge the canonical in-memory SHA transcript before CPU work. All
    // underlying partial reads and EOF/fstats were charged by the shared reader.
    budget.charge(WorkUsage {
        bytes: artifact.bytes,
        ..Default::default()
    })?;
    let result = hash_committed_step_opened_v1(
        &|name| {
            if name != artifact.name {
                return Err(io::Error::other("canonical hash opener changed artifact"));
            }
            Ok(io::Cursor::new(input.bytes_v1()))
        },
        std::slice::from_ref(artifact),
        0,
        u64::MAX,
    )?;
    if result.files_verified != 1 || result.verdict != TreeScrubVerdictV1::Completed {
        return Err(corrupt("committed input bytes differ from their seal"));
    }
    budget.charge(WorkUsage {
        reads: 1,
        ..Default::default()
    })?;
    let current = io.open(&path, budget)?;
    budget.charge(WorkUsage {
        reads: 1,
        ..Default::default()
    })?;
    if super::Identity::from(&current.file()?.metadata()?) != input.identity {
        return Err(corrupt("committed input path changed while hashing"));
    }
    drop(current);
    budget.checkpoint()?;
    Ok(input)
}

/// Authenticate each sorted commitment through the caller's actual read port.
/// `required_prefix` is empty or a canonical relative directory ending in `/`.
/// Payload hardlinks remain valid. This proves committed bytes only, not
/// directory exhaustiveness, an engine's interpretation, Source or publication.
/// The owner must keep the same root and require retirement after this call.
pub fn verify_existing_committed_files_v1(
    generation: &Path,
    required_prefix: &str,
    committed: &[SealedArtifactCommitmentV1],
    owner: u32,
    io: &dyn ObjectReadIoPort,
    budget: &mut WorkBudget,
) -> Result<(), Failure> {
    if !required_prefix.is_empty()
        && (!required_prefix.ends_with('/')
            || !canonical_relative(required_prefix.trim_end_matches('/'))
            || required_prefix.ends_with("//"))
    {
        return Err(corrupt("committed payload prefix is not canonical"));
    }
    // The unchanged Core hasher allocates a 64KiB buffer. Reserve it BEFORE
    // entering the hasher and retain the grant until its buffer is gone.
    let _hash_memory = memory(budget, 1 << 16)?;
    let budget = RefCell::new(budget);
    let mut previous = None;
    for artifact in committed {
        let work = budget.try_borrow().map_err(|_| IndexError::Conflict)?;
        work.checkpoint()?;
        let name = artifact.name.as_str();
        if previous.is_some_and(|previous: &str| previous >= name)
            || !name.starts_with(required_prefix)
            || !canonical_relative(name)
        {
            return Err(corrupt(
                "committed payload names are not canonical sorted unique paths",
            ));
        }
        previous = Some(name);
        let (path, _path_memory) = committed_path(generation, name, &work)?;
        drop(work);
        let failure = RefCell::new(None);
        // One artifact per canonical step avoids a second name index or an
        // O(N^2) opener lookup. Core retains its original SHA/length/+1-byte
        // transcript; no hashing implementation or acceptance floor is copied.
        let result = hash_committed_step_opened_v1(
            &|requested| {
                let open = || -> Result<HashReader<'_, '_>, Failure> {
                    if requested != name {
                        return Err(corrupt("canonical hash opener changed artifact"));
                    }
                    let mut work = budget.try_borrow_mut().map_err(|_| IndexError::Conflict)?;
                    work.charge(WorkUsage {
                        reads: 1,
                        ..WorkUsage::default()
                    })?;
                    let opened = io.open(&path, &mut work)?;
                    work.charge(WorkUsage {
                        reads: 1,
                        ..WorkUsage::default()
                    })?;
                    let before = opened.file()?.metadata()?;
                    // Inherited Legacy dataset hardlinks remain valid. The
                    // sidecar-only helper's nlink==1 rule is NOT applied here.
                    if !before.is_file() || before.uid() != owner || before.len() != artifact.bytes
                    {
                        return Err(corrupt(
                            "committed payload metadata differs from commitment",
                        ));
                    }
                    Ok(HashReader {
                        opened,
                        path: &path,
                        before,
                        io,
                        budget: &budget,
                        failure: &failure,
                        position: 0,
                    })
                };
                open().map_err(|error| {
                    if let Ok(mut first) = failure.try_borrow_mut()
                        && first.is_none()
                    {
                        *first = Some(error);
                    }
                    io::Error::other("committed admitted hash open refused")
                })
            },
            std::slice::from_ref(artifact),
            0,
            u64::MAX,
        );
        if let Some(error) = failure.into_inner() {
            return Err(error);
        }
        let result = result?;
        if result.files_verified != 1 || result.verdict != TreeScrubVerdictV1::Completed {
            return Err(corrupt(
                "committed committed payload does not match its seal",
            ));
        }
    }
    budget.into_inner().checkpoint()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{MemoryLease, MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits};
    use std::{
        fs::File,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    // Finite unit-test allocation accounting only. These tests do not supply
    // the recovery caller's genuine root/physical authority or qualify the recovery consumer.
    #[derive(Default)]
    struct LiveMemory {
        live: AtomicU64,
        bad_release: AtomicBool,
    }
    impl MemoryLeaseOwner for LiveMemory {
        fn release(&self, bytes: u64) {
            if self
                .live
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                    live.checked_sub(bytes)
                })
                .is_err()
            {
                self.bad_release.store(true, Ordering::SeqCst);
            }
        }
    }
    struct BoundedMemory(Arc<LiveMemory>);
    impl MemoryScopePort for BoundedMemory {
        fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            let _previous = self
                .0
                .live
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                    live.checked_add(bytes)
                        .filter(|total| *total <= 1024 * 1024)
                })
                .map_err(|_| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }
    #[derive(Debug)]
    struct DataIo {
        opens: AtomicU64,
        refuse_file_call: Option<u64>,
    }
    struct DataFile {
        file: File,
        calls: AtomicU64,
        refusal: Option<u64>,
        // Actual File drops before its finite test allocation is released.
        _memory: MemoryLease,
    }
    impl ObjectReadFile for DataFile {
        fn file(&self) -> Result<&File, IndexError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.refusal == Some(call) {
                return Err(IndexError::WorkLimitExceeded);
            }
            Ok(&self.file)
        }
    }
    impl ObjectReadIoPort for DataIo {
        fn open(
            &self,
            path: &Path,
            budget: &mut WorkBudget,
        ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
            let backing = u64::try_from(std::mem::size_of::<DataFile>())
                .map_err(|_| IndexError::ResourceLimited)?;
            let memory = budget
                .reserve_memory(backing, MemoryLifetime::Temporary)?
                .ok_or(IndexError::ResourceLimited)?;
            let _previous = self.opens.fetch_add(1, Ordering::SeqCst);
            let file = File::open(path).map_err(|error| IndexError::Storage(error.to_string()))?;
            Ok(Box::new(DataFile {
                file,
                calls: AtomicU64::new(0),
                refusal: self.refuse_file_call,
                _memory: memory,
            }))
        }
    }
    fn work() -> Result<(WorkBudget, Arc<LiveMemory>), Box<dyn std::error::Error>> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("test deadline overflow")?;
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 1000,
                writes: 0,
                bytes: 1024 * 1024,
                mutations: 0,
            },
            deadline,
        );
        let live = Arc::new(LiveMemory::default());
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedMemory(
            live.clone(),
        )))));
        Ok((work, live))
    }
    fn abc() -> SealedArtifactCommitmentV1 {
        // Independent published SHA-256("abc"), not generated by the adapter.
        SealedArtifactCommitmentV1 {
            name: "dataset/a".into(),
            bytes: 3,
            sha256: [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad,
            ],
        }
    }
    #[test]
    fn existing_payload_fixed_sha_accepts_inherited_hardlink_and_refuses_same_length_rewrite()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("dataset"))?;
        let path = root.path().join("dataset/a");
        std::fs::write(&path, b"abc")?;
        std::fs::hard_link(&path, root.path().join("previous-generation-a"))?;
        let owner = std::fs::metadata(&path)?.uid();
        let io = DataIo {
            opens: AtomicU64::new(0),
            refuse_file_call: None,
        };
        let (mut budget, live) = work()?;
        verify_existing_committed_files_v1(
            root.path(),
            "dataset/",
            &[abc()],
            owner,
            &io,
            &mut budget,
        )?;
        assert!(io.opens.load(Ordering::SeqCst) >= 2);
        assert_eq!(live.live.load(Ordering::SeqCst), 0);
        std::fs::write(&path, b"abd")?;
        assert!(matches!(
            verify_existing_committed_files_v1(
                root.path(),
                "dataset/",
                &[abc()],
                owner,
                &io,
                &mut budget
            ),
            Err(Failure::Canonical(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            }))
        ));
        assert_eq!(live.live.load(Ordering::SeqCst), 0);
        assert!(!live.bad_release.load(Ordering::SeqCst));
        Ok(())
    }
    #[test]
    fn committed_payload_same_bytes_new_inode_refuses_at_same_port_reopen()
    -> Result<(), Box<dyn std::error::Error>> {
        #[derive(Debug)]
        struct SwapOnSecondOpen {
            inner: DataIo,
            replacement: PathBuf,
            calls: AtomicU64,
        }
        impl ObjectReadIoPort for SwapOnSecondOpen {
            fn open(
                &self,
                path: &Path,
                budget: &mut WorkBudget,
            ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
                    std::fs::rename(&self.replacement, path)
                        .map_err(|error| IndexError::Storage(error.to_string()))?;
                }
                self.inner.open(path, budget)
            }
        }
        for committed_input in [false, true] {
            let root = tempfile::tempdir()?;
            std::fs::create_dir(root.path().join("dataset"))?;
            let path = root.path().join("dataset/a");
            let replacement = root.path().join("dataset/replacement");
            std::fs::write(&path, b"abc")?;
            std::fs::write(&replacement, b"abc")?;
            assert_ne!(
                std::fs::metadata(&path)?.ino(),
                std::fs::metadata(&replacement)?.ino()
            );
            let owner = std::fs::metadata(&path)?.uid();
            let io = SwapOnSecondOpen {
                inner: DataIo {
                    opens: AtomicU64::new(0),
                    refuse_file_call: None,
                },
                replacement,
                calls: AtomicU64::new(0),
            };
            let (mut budget, live) = work()?;
            let result = if committed_input {
                read_existing_committed_file_v1(root.path(), &abc(), owner, &io, &mut budget)
                    .map(|_| ())
            } else {
                verify_existing_committed_files_v1(
                    root.path(),
                    "dataset/",
                    &[abc()],
                    owner,
                    &io,
                    &mut budget,
                )
            };
            assert!(matches!(
                result,
                Err(Failure::Canonical(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    ..
                }))
            ));
            assert_eq!(io.calls.load(Ordering::SeqCst), 2);
            assert_eq!(io.inner.opens.load(Ordering::SeqCst), 2);
            assert_eq!(live.live.load(Ordering::SeqCst), 0);
            assert!(!live.bad_release.load(Ordering::SeqCst));
        }
        Ok(())
    }

    #[test]
    fn existing_payload_read_bridge_retains_original_work_refusal()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("dataset"))?;
        let path = root.path().join("dataset/a");
        std::fs::write(&path, b"abc")?;
        let owner = std::fs::metadata(&path)?.uid();
        // First file() validates metadata; second is the actual Read bridge.
        let io = DataIo {
            opens: AtomicU64::new(0),
            refuse_file_call: Some(1),
        };
        let (mut budget, live) = work()?;
        assert!(matches!(
            verify_existing_committed_files_v1(
                root.path(),
                "dataset/",
                &[abc()],
                owner,
                &io,
                &mut budget
            ),
            Err(Failure::AdmissionOrIo(IndexError::WorkLimitExceeded))
        ));
        assert_eq!(live.live.load(Ordering::SeqCst), 0);
        assert!(!live.bad_release.load(Ordering::SeqCst));
        Ok(())
    }
    #[test]
    fn existing_payload_missing_memory_refuses_before_any_open()
    -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("test deadline overflow")?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1000,
                writes: 0,
                bytes: 1024 * 1024,
                mutations: 0,
            },
            deadline,
        );
        let io = DataIo {
            opens: AtomicU64::new(0),
            refuse_file_call: None,
        };
        assert!(matches!(
            verify_existing_committed_files_v1(
                Path::new("never-opened"),
                "dataset/",
                &[abc()],
                0,
                &io,
                &mut budget
            ),
            Err(Failure::AdmissionOrIo(IndexError::ResourceLimited))
        ));
        assert_eq!(io.opens.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn existing_payload_generic_prefix_and_canonical_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("a"), b"abc")?;
        let owner = std::fs::metadata(root.path().join("a"))?.uid();
        let io = DataIo {
            opens: AtomicU64::new(0),
            refuse_file_call: None,
        };
        let (mut budget, live) = work()?;
        let mut artifact = abc();
        artifact.name = "a".into();
        verify_existing_committed_files_v1(root.path(), "", &[artifact], owner, &io, &mut budget)?;
        let opened = io.opens.load(Ordering::SeqCst);
        for name in ["", "/a", "./a", "dataset/../a", "dataset//a", "dataset/a/"] {
            let mut artifact = abc();
            artifact.name = name.into();
            assert!(
                verify_existing_committed_files_v1(
                    root.path(),
                    "",
                    &[artifact],
                    owner,
                    &io,
                    &mut budget
                )
                .is_err()
            );
            assert_eq!(io.opens.load(Ordering::SeqCst), opened);
        }
        assert!(
            verify_existing_committed_files_v1(
                root.path(),
                "../",
                &[abc()],
                owner,
                &io,
                &mut budget
            )
            .is_err()
        );
        assert_eq!(io.opens.load(Ordering::SeqCst), opened);
        assert_eq!(live.live.load(Ordering::SeqCst), 0);
        assert!(!live.bad_release.load(Ordering::SeqCst));
        Ok(())
    }
    #[test]
    fn committed_input_retains_original_bytes_accepts_hardlink_and_refuses_rewrite()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("dataset"))?;
        let path = root.path().join("dataset/a");
        std::fs::write(&path, b"abc")?;
        std::fs::hard_link(&path, root.path().join("prior-a"))?;
        let owner = std::fs::metadata(&path)?.uid();
        let io = DataIo {
            opens: AtomicU64::new(0),
            refuse_file_call: None,
        };
        let (mut budget, live) = work()?;
        let data = read_existing_committed_file_v1(root.path(), &abc(), owner, &io, &mut budget)?;
        assert_eq!(data.bytes_v1(), b"abc");
        assert!(live.live.load(Ordering::SeqCst) > 0);
        assert!(io.opens.load(Ordering::SeqCst) >= 2);
        // The ordinary sidecar entry remains strict for this same hardlink.
        assert!(super::super::read_existing_file_v1(&path, owner, 3, &io, &mut budget).is_err());
        drop(data);
        assert_eq!(live.live.load(Ordering::SeqCst), 0);
        std::fs::write(&path, b"abd")?;
        assert!(
            read_existing_committed_file_v1(root.path(), &abc(), owner, &io, &mut budget).is_err()
        );
        assert_eq!(live.live.load(Ordering::SeqCst), 0);
        assert!(!live.bad_release.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn committed_input_empty_payload_uses_fixed_sha256_golden()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("empty"), b"")?;
        let owner = std::fs::metadata(root.path().join("empty"))?.uid();
        let artifact = SealedArtifactCommitmentV1 {
            name: "empty".into(),
            bytes: 0,
            sha256: [
                0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
                0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
                0x78, 0x52, 0xb8, 0x55,
            ],
        };
        let io = DataIo {
            opens: AtomicU64::new(0),
            refuse_file_call: None,
        };
        let (mut budget, live) = work()?;
        let input =
            read_existing_committed_file_v1(root.path(), &artifact, owner, &io, &mut budget)?;
        assert!(input.bytes_v1().is_empty());
        drop(input);
        assert_eq!(live.live.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn committed_input_missing_memory_refuses_before_any_open()
    -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("test deadline overflow")?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1000,
                writes: 0,
                bytes: 1 << 20,
                mutations: 0,
            },
            deadline,
        );
        let io = DataIo {
            opens: AtomicU64::new(0),
            refuse_file_call: None,
        };
        assert!(matches!(
            read_existing_committed_file_v1(Path::new("never-opened"), &abc(), 0, &io, &mut budget),
            Err(Failure::AdmissionOrIo(IndexError::ResourceLimited))
        ));
        assert_eq!(io.opens.load(Ordering::SeqCst), 0);
        Ok(())
    }
}
