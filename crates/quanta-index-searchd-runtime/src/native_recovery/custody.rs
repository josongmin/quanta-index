//! Retained original descriptors. These checks observe custody; they do not
//! authenticate an installed producer or turn evidence data into permission.

use anyhow::{Result, anyhow, ensure};
use quanta_explicit_fd_process::{
    try_duplicate_inherited_read_only_regular_file_v1, try_duplicate_inherited_regular_file_v1,
};
use quanta_index_contract::{index::Digest, native_cutover_evidence_v1 as wire};
use quanta_index_core::index::{MemoryLease, MemoryLifetime, WorkBudget, WorkUsage};
use rustix::fs::{FlockOperation, Mode, OFlags, flock, open, openat};
use sha2::{Digest as _, Sha256};
use std::{
    fs::File,
    os::unix::fs::{FileExt, MetadataExt},
    path::Path,
};

// The 1 MiB CBOR body, bounded typed vectors/strings, and streaming scratch
// remain covered by this one retained grant. No allocation fallback exists.
const EVIDENCE_MEMORY: u64 = 16 * wire::MAX_NATIVE_CUTOVER_EVIDENCE_BYTES_V1 as u64;

pub(super) struct InheritedCustodyV1 {
    host: File,
    sdk: File,
    evidence: File,
    history: Option<File>,
}

pub(super) struct RetainedEvidenceV1 {
    pub(super) facts: wire::NativeCutoverEvidenceV1,
    _memory: MemoryLease,
}

const PUBLISHED_BEFORE_BYTES: u64 = 64 * 1024;

pub(super) fn read_published_before_v1(
    length: u64,
    expected: Digest,
    budget: &mut WorkBudget,
) -> Result<(wire::NativeCutoverEvidenceBeforeSelectionV1, MemoryLease)> {
    ensure!(
        length > 0 && length <= PUBLISHED_BEFORE_BYTES + 512,
        "published recovery intent exceeds its physical bound"
    );
    let memory = budget
        .reserve_memory(
            4 * (PUBLISHED_BEFORE_BYTES + 512),
            MemoryLifetime::Temporary,
        )?
        .ok_or_else(|| anyhow!("published recovery intent requires admitted memory"))?;
    charge(budget, 4, 0)?;
    let file = try_duplicate_inherited_read_only_regular_file_v1(6)?;
    let bytes = read_sealed_evidence_v1(&file, length, expected, budget)?;
    wire::validate_native_cutover_evidence_cbor_v1(&bytes)?;
    let before: wire::NativeCutoverEvidenceBeforeSelectionV1 =
        ciborium::de::from_reader_with_recursion_limit(bytes.as_slice(), 8)?;
    ensure!(
        before.file_version.is_some()
            && !before.document.0.is_empty()
            && u64::try_from(before.document.0.len())? <= PUBLISHED_BEFORE_BYTES,
        "published recovery intent has no exact before selection"
    );
    budget.checkpoint()?;
    Ok((before, memory))
}

impl InheritedCustodyV1 {
    pub(super) fn import_v1(budget: &mut WorkBudget, history: Option<File>) -> Result<Self> {
        charge(budget, 10, 0)?;
        Ok(Self {
            host: try_duplicate_inherited_regular_file_v1(4)?,
            sdk: try_duplicate_inherited_regular_file_v1(5)?,
            evidence: try_duplicate_inherited_read_only_regular_file_v1(6)?,
            history,
        })
    }

    pub(super) fn read_evidence_v1(
        &self,
        length: u64,
        expected: Digest,
        budget: &mut WorkBudget,
    ) -> Result<RetainedEvidenceV1> {
        ensure!(
            length > 0 && length <= wire::MAX_NATIVE_CUTOVER_EVIDENCE_BYTES_V1 as u64,
            "cutover evidence length exceeds its physical bound"
        );
        let memory = budget
            .reserve_memory(EVIDENCE_MEMORY, MemoryLifetime::Temporary)?
            .ok_or_else(|| anyhow!("cutover evidence requires original admitted memory"))?;
        let bytes = read_sealed_evidence_v1(&self.evidence, length, expected, budget)?;
        let facts = wire::decode_native_cutover_evidence_v1(&bytes)?;
        drop(bytes);
        budget.checkpoint()?;
        Ok(RetainedEvidenceV1 {
            facts,
            _memory: memory,
        })
    }

    /// Repeated immediately before publication and after directory fsync.
    /// Never explicitly unlock these descriptors: parent and child share OFDs.
    pub(super) fn revalidate_locks_v1(
        &self,
        evidence: &wire::NativeCutoverEvidenceV1,
        sdk_lock_path: &Path,
        budget: &mut WorkBudget,
    ) -> Result<()> {
        let header = &evidence.header;
        require_original_host_namespace_lock_v1(&self.host, &header.host, budget)?;
        require_original_exclusive_lock_v1(&self.sdk, sdk_lock_path, header.sdk_lock_file, budget)?;
        ensure!(
            header.host.namespace_file != header.sdk_lock_file,
            "producer and SDK descriptors alias the same namespace"
        );
        if let Some(history) = &self.history {
            require_original_history_writer_fence_v1(
                history,
                Path::new(&header.writer_state_root),
                budget,
            )?;
        }
        budget.checkpoint()?;
        Ok(())
    }

    pub(super) fn require_history_writer_fence_v1(
        &self,
        writer_root: &Path,
        budget: &mut WorkBudget,
    ) -> Result<()> {
        let history = self.history.as_ref().ok_or_else(|| {
            anyhow!("Legacy History rows require inherited original writer fence")
        })?;
        require_original_history_writer_fence_v1(history, writer_root, budget)
    }
}

fn read_sealed_evidence_v1(
    file: &File,
    length: u64,
    expected: Digest,
    budget: &mut WorkBudget,
) -> Result<Vec<u8>> {
    charge(budget, 1, 0)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() == length,
        "cutover evidence descriptor length/type differs from Begin"
    );
    require_immutable_evidence_v1(file)?;
    let mut bytes = vec![0; usize::try_from(length)?];
    read_exact_at_v1(file, &mut bytes, budget)?;
    ensure!(
        Digest(Sha256::digest(&bytes).into()) == expected,
        "cutover evidence digest differs from Begin"
    );
    Ok(bytes)
}

fn require_original_history_writer_fence_v1(
    held: &File,
    writer_root: &Path,
    budget: &mut WorkBudget,
) -> Result<()> {
    const NAME: &str = ".history-source-writer.lock";
    charge(budget, 4, u64::try_from(writer_root.as_os_str().as_encoded_bytes().len())?)?;
    ensure!(
        writer_root.is_absolute()
            && !writer_root.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::CurDir | std::path::Component::ParentDir
                )
            })
            && writer_root.canonicalize()?.as_path() == writer_root,
        "History writer root is not one canonical existing directory"
    );
    let root = File::from(open(
        writer_root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let root_metadata = root.metadata()?;
    ensure!(
        root_metadata.is_dir() && root_metadata.uid() == rustix::process::geteuid().as_raw(),
        "History writer root is not owned by the recovery user"
    );
    let probe = File::from(openat(
        &root,
        NAME,
        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let original = held.metadata()?;
    let named = probe.metadata()?;
    let uid = rustix::process::geteuid().as_raw();
    for metadata in [&original, &named] {
        ensure!(
            metadata.is_file()
                && metadata.uid() == uid
                && metadata.mode() & 0o077 == 0
                && metadata.nlink() == 1
                && metadata.len() == 0,
            "History writer fence is not one private empty regular file"
        );
    }
    ensure!(
        (original.dev(), original.ino()) == (named.dev(), named.ino()),
        "History writer fence differs from the named original"
    );
    // A fresh file description must be excluded by the inherited EX lock.
    match flock(&probe, FlockOperation::NonBlockingLockShared) {
        Err(rustix::io::Errno::WOULDBLOCK) => {}
        Ok(()) => return Err(anyhow!("inherited History fence did not exclude another reader")),
        Err(error) => return Err(error.into()),
    }
    flock(held, FlockOperation::NonBlockingLockExclusive)?;
    budget.checkpoint()?;
    Ok(())
}

fn require_original_host_namespace_lock_v1(
    held: &File,
    host: &wire::NativeCutoverEvidenceHostSealV1,
    budget: &mut WorkBudget,
) -> Result<()> {
    let namespace = Path::new(&host.namespace_root);
    charge(budget, 0, u64::try_from(host.namespace_root.len())?)?;
    ensure!(
        namespace.is_absolute()
            && !namespace
                .components()
                .any(|part| part == std::path::Component::ParentDir)
            && namespace.file_name().is_some_and(|name| {
                let bytes = name.as_encoded_bytes();
                bytes.len() == 64 && bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            }),
        "Host custody namespace is not an absolute V3 digest directory"
    );
    // The producer holds one EX lease for the state root, above the
    // per-workspace inbox selected by its signed namespace digest.
    let state_root = namespace
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "namespaces-v3"))
        .and_then(Path::parent)
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name == "delivery-inbox-v2")
        })
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("Host custody namespace is outside the V3 inbox layout"))?;
    let path = state_root.join("delivery-inbox-v3.lease");
    require_original_exclusive_lock_v1(held, &path, host.namespace_file, budget)
}

#[cfg(target_os = "linux")]
fn require_immutable_evidence_v1(file: &File) -> Result<()> {
    use rustix::fs::{SealFlags, fcntl_get_seals};
    let required = SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE;
    ensure!(
        fcntl_get_seals(file)?.contains(required),
        "cutover evidence is not the immutable pinned FD6 artifact"
    );
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn require_immutable_evidence_v1(_file: &File) -> Result<()> {
    anyhow::bail!("installed recovery requires Linux immutable descriptor authentication")
}

fn require_original_exclusive_lock_v1(
    held: &File,
    path: &Path,
    expected: wire::NativeCutoverEvidenceFileIdentityV1,
    budget: &mut WorkBudget,
) -> Result<()> {
    ensure!(path.is_absolute(), "custody lock path must be absolute");
    charge(budget, 3, 0)?;
    let original = held.metadata()?;
    let named = std::fs::symlink_metadata(path)?;
    let uid = rustix::process::geteuid().as_raw();
    for metadata in [&original, &named] {
        ensure!(
            metadata.is_file()
                && metadata.uid() == uid
                && metadata.mode() & 0o077 == 0
                && metadata.nlink() == 1,
            "custody lock is not a private original regular file"
        );
        ensure!(
            (metadata.dev(), metadata.ino()) == (expected.device, expected.inode),
            "custody lock identity differs from the authenticated producer"
        );
    }
    let probe = File::from(open(
        path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?);
    let observed = probe.metadata()?;
    ensure!(
        (observed.dev(), observed.ino()) == (expected.device, expected.inode),
        "custody lock pathname changed during revalidation"
    );
    // A fresh shared description must be excluded. Merely taking EX on the
    // inherited file could silently upgrade an ordinary shared SDK reader.
    match flock(&probe, FlockOperation::NonBlockingLockShared) {
        Err(rustix::io::Errno::WOULDBLOCK) => {}
        Ok(()) => return Err(anyhow!("inherited custody did not exclude another reader")),
        Err(error) => return Err(error.into()),
    }
    flock(held, FlockOperation::NonBlockingLockExclusive)?;
    budget.checkpoint()?;
    Ok(())
}

fn read_exact_at_v1(file: &File, bytes: &mut [u8], budget: &mut WorkBudget) -> Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        let end = bytes.len().min(offset + 64 * 1024);
        charge(budget, 1, u64::try_from(end - offset)?)?;
        let count = file.read_at(&mut bytes[offset..end], u64::try_from(offset)?)?;
        ensure!(
            count > 0,
            "cutover evidence ended before its declared length"
        );
        offset += count;
    }
    charge(budget, 1, 1)?;
    let mut extra = [0];
    ensure!(
        file.read_at(&mut extra, u64::try_from(offset)?)? == 0,
        "cutover evidence has an unaccounted trailing body"
    );
    Ok(())
}

pub(super) fn charge(budget: &mut WorkBudget, reads: u64, bytes: u64) -> Result<()> {
    budget.charge(WorkUsage {
        reads,
        bytes,
        ..WorkUsage::default()
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, OpenOptions},
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        path: PathBuf,
    }
    impl Fixture {
        fn new() -> Result<Self> {
            let path = std::env::temp_dir().join(format!(
                "qidx-retained-cutover-lock-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path)?;
            Ok(Self { path })
        }
        fn file(&self, name: &str) -> Result<(File, PathBuf)> {
            let path = self.path.join(name);
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?;
            Ok((file, path))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _removed = fs::remove_dir_all(&self.path);
        }
    }
    fn budget() -> WorkBudget {
        WorkBudget::new(
            quanta_index_core::index::WorkLimits {
                reads: 100,
                writes: 0,
                bytes: 4096,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(10),
        )
    }
    fn identity(file: &File) -> Result<wire::NativeCutoverEvidenceFileIdentityV1> {
        let metadata = file.metadata()?;
        Ok(wire::NativeCutoverEvidenceFileIdentityV1 {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[test]
    fn history_writer_fence_requires_same_held_private_original() -> Result<()> {
        let fixture = Fixture::new()?;
        let root = fixture.path.canonicalize()?;
        let (owner, path) = fixture.file(".history-source-writer.lock")?;
        let inherited = owner.try_clone()?;
        let mut budget = budget();
        assert!(require_original_history_writer_fence_v1(&inherited, &root, &mut budget).is_err());
        flock(&owner, FlockOperation::NonBlockingLockExclusive)?;
        require_original_history_writer_fence_v1(&inherited, &root, &mut budget)?;

        let alias = root.join("another-link");
        fs::hard_link(&path, &alias)?;
        assert!(require_original_history_writer_fence_v1(&inherited, &root, &mut budget).is_err());
        fs::remove_file(alias)?;

        fs::rename(&path, root.join("old-lock"))?;
        let (_replacement, _) = fixture.file(".history-source-writer.lock")?;
        assert!(require_original_history_writer_fence_v1(&inherited, &root, &mut budget).is_err());
        Ok(())
    }
    #[test]
    fn host_namespace_revalidates_original_state_root_lease() -> Result<()> {
        let fixture = Fixture::new()?;
        let host_root = fixture.path.canonicalize()?;
        let namespace = host_root
            .join("delivery-inbox-v2/namespaces-v3")
            .join("a".repeat(64));
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&namespace)?;
        let (file, _) = fixture.file("delivery-inbox-v3.lease")?;
        flock(&file, FlockOperation::NonBlockingLockExclusive)?;
        let mut host = wire::NativeCutoverEvidenceHostSealV1 {
            canonical_workspace_root: "/workspace".into(),
            host_instance: 1,
            host_generation: 1,
            watch_mode: wire::NativeCutoverHostWatchModeV1::Polling,
            custody_mode: wire::NativeCutoverHostCustodyModeV1::DurableInbox,
            namespace_root: namespace
                .to_str()
                .ok_or_else(|| anyhow!("namespace is not UTF-8"))?
                .into(),
            namespace_file: identity(&file)?,
        };
        require_original_host_namespace_lock_v1(&file, &host, &mut budget())?;

        let wrong_path = namespace.join("delivery-inbox-v3.lease");
        let wrong = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(wrong_path)?;
        flock(&wrong, FlockOperation::NonBlockingLockExclusive)?;
        host.namespace_file = identity(&wrong)?;
        ensure!(require_original_host_namespace_lock_v1(&wrong, &host, &mut budget()).is_err());
        host.namespace_root = host_root
            .join("other/namespaces-v3")
            .join("a".repeat(64))
            .to_str()
            .ok_or_else(|| anyhow!("namespace is not UTF-8"))?
            .into();
        host.namespace_file = identity(&file)?;
        ensure!(require_original_host_namespace_lock_v1(&file, &host, &mut budget()).is_err());
        Ok(())
    }
    #[test]
    fn original_exclusive_ofd_excludes_an_independent_shared_reader() -> Result<()> {
        let fixture = Fixture::new()?;
        let (file, path) = fixture.file("exclusive.lock")?;
        flock(&file, FlockOperation::NonBlockingLockExclusive)?;
        require_original_exclusive_lock_v1(&file, &path, identity(&file)?, &mut budget())?;
        let duplicate = file.try_clone()?;
        drop(file);
        require_original_exclusive_lock_v1(
            &duplicate,
            &path,
            identity(&duplicate)?,
            &mut budget(),
        )?;
        Ok(())
    }
    #[test]
    fn ordinary_shared_reader_cannot_be_upgraded_into_offline_authority() -> Result<()> {
        let fixture = Fixture::new()?;
        let (file, path) = fixture.file("shared.lock")?;
        flock(&file, FlockOperation::NonBlockingLockShared)?;
        let error =
            require_original_exclusive_lock_v1(&file, &path, identity(&file)?, &mut budget())
                .err()
                .ok_or_else(|| {
                    anyhow!("shared reader incorrectly admitted as offline exclusive")
                })?;
        ensure!(error.to_string().contains("did not exclude another reader"));
        Ok(())
    }
    #[test]
    fn replacing_lock_path_does_not_transfer_original_descriptor_custody() -> Result<()> {
        let fixture = Fixture::new()?;
        let (file, path) = fixture.file("original.lock")?;
        flock(&file, FlockOperation::NonBlockingLockExclusive)?;
        let expected = identity(&file)?;
        fs::rename(&path, fixture.path.join("retired.lock"))?;
        let (_replacement, _) = fixture.file("original.lock")?;
        ensure!(require_original_exclusive_lock_v1(&file, &path, expected, &mut budget()).is_err());
        Ok(())
    }
}
