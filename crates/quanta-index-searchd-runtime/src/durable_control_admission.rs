//! Service-owned lifecycle Control file admission from the canonical binder.

use std::path::Path;
use std::sync::Arc;

use quanta_index_core::publication_admission::{
    DurableControlFileAdmissionPort, PublicationControlOpenTicket, PublicationOutputClass,
    PublicationOutputPermit, PublicationOutputReservation,
};
use quanta_index_core::{CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE};

use crate::publication_control_files::cold_control_open;
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_sidecar::sidecar_output_permit;

pub struct CanonicalDurableControlFileAdmission {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
    max_file_bytes: u64,
}

impl CanonicalDurableControlFileAdmission {
    pub fn new(
        deletions: Arc<PublicationFileDeletions>,
        max_file_bytes: u64,
    ) -> Result<Self, CoreError> {
        if max_file_bytes == 0 {
            return Err(CoreError::InvalidContract(
                "durable Control file maximum must be positive".into(),
            ));
        }
        Ok(Self {
            binder: deletions.binder(),
            deletions,
            max_file_bytes,
        })
    }
}

impl DurableControlFileAdmissionPort for CanonicalDurableControlFileAdmission {
    fn begin_reader_open(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationControlOpenTicket>, CoreError> {
        let key_path = self.binder.state_root().join("keys").join("cursor-v2.key");
        if path != key_path {
            return Err(CoreError::InvalidContract(
                "durable Control cold reader path is not the cursor key".into(),
            ));
        }
        // The common open guard authenticates the current leased root and
        // holds the nofollow parent and original file identity across open.
        Ok(cold_control_open(self.binder.begin_reader_open(path)?))
    }

    fn reconcile_deletions(&self) -> Result<(), CoreError> {
        self.deletions.reconcile_retired_control_creates()
    }

    fn delete_file_and_sync(&self, path: &std::path::Path) -> Result<(), CoreError> {
        self.deletions.delete_file_and_sync(path)
    }

    fn reserve_output(
        &self,
        max_encoded_bytes: u64,
    ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
        if max_encoded_bytes == 0 {
            return Err(CoreError::InvalidContract(
                "durable Control file bound must be positive".into(),
            ));
        }
        if max_encoded_bytes > self.max_file_bytes {
            return Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                message: format!(
                    "durable Control file bound {max_encoded_bytes} exceeds configured maximum {}",
                    self.max_file_bytes
                ),
            });
        }
        self.deletions.reconcile_retired_control_creates()?;
        let parent = self
            .binder
            .reserve_publication_allowance(max_encoded_bytes, 1)?;
        let request = PublicationOutputReservation::new(
            PublicationOutputClass::Control,
            max_encoded_bytes,
            1,
        )?;
        // The sidecar retains the parent until physical custody settles.
        sidecar_output_permit(Arc::clone(&self.deletions), parent, request)
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Write as _};
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _, symlink};
    use std::path::PathBuf;
    use std::sync::Arc;

    use quanta_index_core::publication_admission::DurableControlFileAdmissionPort as _;
    use quanta_index_core::publication_capacity::PublicationCapacity;
    use quanta_index_core::{CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, RequestBudgetV1};
    use quanta_index_search_plane::CursorKeyStore;
    use quanta_index_searchd::app::runtime::StateRootLease;

    use super::CanonicalDurableControlFileAdmission;
    use crate::publication_file_capacity::PublicationFileCapacity;
    use crate::publication_file_deletions::PublicationFileDeletions;
    use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn require(condition: bool, message: &'static str) -> io::Result<()> {
        if condition {
            Ok(())
        } else {
            Err(io::Error::other(message))
        }
    }

    fn require_eq<T: std::fmt::Debug + PartialEq>(actual: &T, expected: &T) -> io::Result<()> {
        if actual == expected {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "actual={actual:?}, expected={expected:?}"
            )))
        }
    }

    type Fixture = (
        tempfile::TempDir,
        PathBuf,
        PublicationCapacity,
        Arc<PublicationFileCapacity>,
    );

    fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
        fixture_with_staged_files(false)
    }

    fn fixture_with_staged_files(staged: bool) -> Result<Fixture, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let requested_root = directory.path().join("state");
        fs::create_dir(&requested_root)?;
        fs::set_permissions(&requested_root, fs::Permissions::from_mode(0o700))?;
        let lease = StateRootLease::acquire(&requested_root)?;
        let root = lease.state_root_identity_v1().to_path_buf();
        if staged {
            let activations = root.join("activations");
            let staging = activations.join(".staging");
            fs::create_dir_all(&staging)?;
            fs::write(staging.join("scv1-activations-old.tmp-7-1"), b"old")?;
            fs::write(activations.join(".old--corpus.json.tmp-7-2"), b"old")?;
            fs::write(activations.join("unrelated"), b"keep")?;
        }
        let authority = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes: 1024,
                max_files: 32,
                max_pending: 32,
                max_entries: 64,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &RequestBudgetV1::unbounded(),
        )?;
        let binder = PublicationFileCapacity::new(lease, authority.clone(), 16, 64, 256)?;
        Ok((directory, root, authority, binder))
    }

    fn actual_inventory(root: &std::path::Path) -> Result<(u64, u64), Box<dyn std::error::Error>> {
        use std::os::unix::fs::MetadataExt as _;

        let mut directories = vec![root.to_path_buf()];
        let mut inodes = std::collections::BTreeSet::new();
        let mut bytes = 0_u64;
        let mut files = 0_u64;
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let metadata = fs::symlink_metadata(entry.path())?;
                if metadata.is_dir() {
                    directories.push(entry.path());
                } else if metadata.is_file() {
                    if inodes.insert((metadata.dev(), metadata.ino())) {
                        bytes = bytes
                            .checked_add(metadata.len())
                            .ok_or("inventory bytes overflow")?;
                        files = files.checked_add(1).ok_or("inventory files overflow")?;
                    }
                } else {
                    return Err("unexpected non-regular fixture entry".into());
                }
            }
        }
        Ok((bytes, files))
    }

    #[test]
    fn cold_control_reader_accepts_only_the_leased_cursor_key_path() -> TestResult {
        let (directory, root, authority, binder) = fixture()?;
        let admission = CanonicalDurableControlFileAdmission::new(
            Arc::new(PublicationFileDeletions::new(Arc::clone(&binder))),
            64,
        )?;
        let before = authority.snapshot()?;
        let key = root.join("keys/cursor-v2.key");
        for rejected in [
            root.join("keys/other.key"),
            root.join("keys/../keys/cursor-v2.key"),
            root.join("catalog/catalog-v1.sqlite"),
            directory.path().join("foreign/keys/cursor-v2.key"),
        ] {
            require(
                matches!(
                    admission.begin_reader_open(&rejected),
                    Err(CoreError::InvalidContract(_))
                ),
                "cold reader accepted a path outside its exact key",
            )?;
        }
        require(
            matches!(
                admission.begin_reader_open(&key),
                Err(CoreError::NotFound(_))
            ),
            "missing exact key did not return a source-absence error",
        )?;
        require_eq(&authority.snapshot()?, &before)?;
        require(binder.is_quiescent()?, "cold reader retained a claim")?;
        Ok(())
    }

    #[test]
    fn admitted_cursor_key_refuses_39_bytes_then_commits_and_reopens_exactly_40() -> TestResult {
        let (_directory, root, authority, binder) = fixture()?;
        let deletions = Arc::new(PublicationFileDeletions::new(Arc::clone(&binder)));
        let too_small = CanonicalDurableControlFileAdmission::new(Arc::clone(&deletions), 39)?;
        let baseline = authority.snapshot()?;
        let key_path = root.join("keys/cursor-v2.key");
        require(
            matches!(
                CursorKeyStore::open_with_durable_file_admission(&root, &too_small),
                Err(CoreError::Typed { code, .. })
                    if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
            ),
            "39-byte file bound accepted the fixed 40-byte cursor key",
        )?;
        require(!key_path.exists(), "refusal created a cursor key")?;
        require_eq(&authority.snapshot()?, &baseline)?;
        require_eq(
            &actual_inventory(&root)?,
            &(baseline.used_bytes, baseline.used_files),
        )?;
        require(
            binder.is_quiescent()?,
            "refusal retained a reader or writer",
        )?;

        let admitted = CanonicalDurableControlFileAdmission::new(deletions, 40)?;
        let first = CursorKeyStore::open_with_durable_file_admission(&root, &admitted)?;
        let original = fs::read(&key_path)?;
        let metadata = fs::symlink_metadata(&key_path)?;
        require(
            metadata.is_file(),
            "published cursor key is not a regular file",
        )?;
        require(
            !metadata.file_type().is_symlink(),
            "published key is a symlink",
        )?;
        require(
            metadata.len() == 40,
            "published cursor key has the wrong width",
        )?;
        require(
            metadata.permissions().mode() & 0o7777 == 0o600,
            "published cursor key is not owner-only",
        )?;
        let committed = authority.snapshot()?;
        require(
            committed.used_bytes
                == baseline
                    .used_bytes
                    .checked_add(40)
                    .ok_or("byte oracle overflow")?,
            "published cursor key did not commit exactly 40 bytes",
        )?;
        require(
            committed.used_files
                == baseline
                    .used_files
                    .checked_add(1)
                    .ok_or("file oracle overflow")?,
            "published cursor key did not commit one file",
        )?;
        require(
            committed.retained_inodes
                == baseline
                    .retained_inodes
                    .checked_add(1)
                    .ok_or("inode oracle overflow")?,
            "published cursor key did not retain one inode",
        )?;
        require_eq(
            &actual_inventory(&root)?,
            &(committed.used_bytes, committed.used_files),
        )?;
        require(binder.is_quiescent()?, "key creation retained FD custody")?;

        let reopened = CursorKeyStore::open_with_durable_file_admission(&root, &admitted)?;
        require(
            first.key_id() == reopened.key_id(),
            "reopen rotated the key id",
        )?;
        require_eq(&fs::read(&key_path)?, &original)?;
        require_eq(&authority.snapshot()?, &committed)?;
        require(binder.is_quiescent()?, "key reopen retained a reader pin")?;
        Ok(())
    }

    #[test]
    fn admitted_cursor_reader_ceiling_and_mode_refusal_preserve_charge() -> TestResult {
        let (_directory, root, authority, binder) = fixture()?;
        let admission = CanonicalDurableControlFileAdmission::new(
            Arc::new(PublicationFileDeletions::new(Arc::clone(&binder))),
            40,
        )?;
        let _created = CursorKeyStore::open_with_durable_file_admission(&root, &admission)?;
        let key_path = root.join("keys/cursor-v2.key");
        let committed = authority.snapshot()?;
        let mut readers = Vec::new();
        for _ in 0..16 {
            let ticket = admission.begin_reader_open(&key_path)?;
            readers.push(ticket.opened_owned(File::open(&key_path)?)?);
        }
        require(
            matches!(
                admission.begin_reader_open(&key_path),
                Err(CoreError::Typed { code, .. })
                    if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
            ),
            "seventeenth reader exceeded the configured handle ceiling",
        )?;
        require_eq(&authority.snapshot()?, &committed)?;
        while let Some(reader) = readers.pop() {
            reader.retire()?;
        }
        require(binder.is_quiescent()?, "reader retirement retained a pin")?;

        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o644))?;
        require(
            matches!(
                CursorKeyStore::open_with_durable_file_admission(&root, &admission),
                Err(CoreError::Storage(message))
                    if message.contains("original FD is not a private 40-byte regular key")
            ),
            "loose cursor key mode was accepted",
        )?;
        require_eq(&authority.snapshot()?, &committed)?;
        require(
            binder.is_quiescent()?,
            "rejected key mode retained a reader",
        )?;
        Ok(())
    }

    #[test]
    fn admitted_cursor_key_refuses_symlink_without_creating_or_refunding() -> TestResult {
        let (directory, root, authority, binder) = fixture()?;
        let admission = CanonicalDurableControlFileAdmission::new(
            Arc::new(PublicationFileDeletions::new(Arc::clone(&binder))),
            40,
        )?;
        let baseline = authority.snapshot()?;
        let keys = root.join("keys");
        fs::create_dir(&keys)?;
        fs::set_permissions(&keys, fs::Permissions::from_mode(0o700))?;
        let key_path = keys.join("cursor-v2.key");
        symlink(directory.path(), &key_path)?;
        let expected_loop = std::io::Error::from(rustix::io::Errno::LOOP)
            .raw_os_error()
            .ok_or("ELOOP lacks raw errno")?;
        require(
            matches!(
                CursorKeyStore::open_with_durable_file_admission(&root, &admission),
                Err(CoreError::StorageIo {
                    raw_os_error: Some(errno),
                    ..
                }) if errno == expected_loop
            ),
            "symlink cursor key did not fail at nofollow open",
        )?;
        require(
            fs::symlink_metadata(&key_path)?.file_type().is_symlink(),
            "refusal replaced the foreign symlink",
        )?;
        require_eq(&authority.snapshot()?, &baseline)?;
        require(binder.is_quiescent()?, "symlink refusal retained a claim")?;

        fs::remove_file(&key_path)?;
        fs::remove_dir_all(&keys)?;
        symlink(directory.path(), &keys)?;
        require(
            CursorKeyStore::open_with_durable_file_admission(&root, &admission).is_err(),
            "symlink key directory was admitted",
        )?;
        require(
            fs::symlink_metadata(&keys)?.file_type().is_symlink(),
            "parent refusal replaced the foreign symlink",
        )?;
        require_eq(&authority.snapshot()?, &baseline)?;
        require(binder.is_quiescent()?, "parent refusal retained a claim")?;
        Ok(())
    }

    #[derive(Debug)]
    struct UnusedIndexBytes;

    impl quanta_index_search_plane::SearchCorpusIndexBytesPort for UnusedIndexBytes {
        fn measurement_policy(
            &self,
        ) -> quanta_index_core::domains::generation::RetentionMeasurementPolicy {
            quanta_index_core::domains::generation::RetentionMeasurementPolicy::DEFAULT
        }

        fn measure_index_bytes(
            &self,
            _repo_id: &quanta_index_contract::RepoId,
            _revision_id: &quanta_index_contract::RevisionId,
            _generations: &std::collections::BTreeSet<quanta_index_contract::ManifestGeneration>,
            _budget: &RequestBudgetV1,
            _diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
        ) -> Result<quanta_index_core::SealedGenerationBytesV1, CoreError> {
            Err(CoreError::NotImplemented(
                "bootstrap must not measure generations".into(),
            ))
        }
    }

    #[test]
    fn qualified_lifecycle_boot_reclaims_inventory_and_creates_one_durable_identity() -> TestResult
    {
        use quanta_index_search_plane::SearchCorpusLifecycleOwner;
        use quanta_index_search_plane::readiness::SearchCorpusHistoryRetentionPolicyV1;

        let (_directory, root, authority, binder) = fixture_with_staged_files(true)?;
        let baseline = authority.snapshot()?;
        let admission: Arc<
            dyn quanta_index_core::publication_admission::DurableControlFileAdmissionPort,
        > = Arc::new(CanonicalDurableControlFileAdmission::new(
            Arc::new(PublicationFileDeletions::new(Arc::clone(&binder))),
            16,
        )?);
        let retention =
            SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 8, 8 * 1024 * 1024)?;
        let lifecycle = SearchCorpusLifecycleOwner::open_with_durable_file_admission(
            &root,
            retention,
            Arc::new(UnusedIndexBytes),
            Arc::clone(&admission),
        )?;
        let activations = root.join("activations");
        let identity_path = activations.join(".activation-root-incarnation-v1");
        let identity = fs::read(&identity_path)?;
        require(
            identity.len() == 16 && identity.iter().any(|byte| *byte != 0),
            "bootstrap identity is not sixteen nonzero-incarnation bytes",
        )?;
        require(
            !activations
                .join(".staging/scv1-activations-old.tmp-7-1")
                .exists()
                && !activations.join(".old--corpus.json.tmp-7-2").exists(),
            "qualified bootstrap retained owned abandoned files",
        )?;
        require_eq(&fs::read(activations.join("unrelated"))?, &b"keep".to_vec())?;
        let settled = authority.snapshot()?;
        require_eq(
            &settled.used_bytes,
            &baseline
                .used_bytes
                .checked_add(10)
                .ok_or("oracle byte overflow")?,
        )?;
        require_eq(
            &settled.used_files,
            &baseline
                .used_files
                .checked_sub(1)
                .ok_or("oracle file underflow")?,
        )?;
        require_eq(&settled.uncertain_pending, &baseline.uncertain_pending)?;
        require_eq(
            &actual_inventory(&root)?,
            &(settled.used_bytes, settled.used_files),
        )?;
        require(
            binder.is_quiescent()?,
            "qualified bootstrap retained a live mutation or FD pin",
        )?;
        drop(lifecycle);
        let reopened = SearchCorpusLifecycleOwner::open_with_durable_file_admission(
            &root,
            retention,
            Arc::new(UnusedIndexBytes),
            admission,
        )?;
        require_eq(&fs::read(&identity_path)?, &identity)?;
        require_eq(&authority.snapshot()?, &settled)?;
        drop(reopened);
        Ok(())
    }

    #[test]
    fn exact_bound_is_from_one_canonical_ledger_and_commits_actual_fd() -> TestResult {
        let (_directory, root, authority, binder) = fixture()?;
        let baseline = authority.snapshot()?;
        let admission = CanonicalDurableControlFileAdmission::new(
            Arc::new(PublicationFileDeletions::new(Arc::clone(&binder))),
            3,
        )?;
        require(
            matches!(
                CanonicalDurableControlFileAdmission::new(
                    Arc::new(PublicationFileDeletions::new(Arc::clone(&binder))),
                    0
                ),
                Err(CoreError::InvalidContract(_))
            ),
            "zero configured file maximum was accepted",
        )?;
        require(
            matches!(
                admission.reserve_output(0),
                Err(CoreError::InvalidContract(_))
            ),
            "zero encoded bound was accepted",
        )?;
        require(
            matches!(
                admission.reserve_output(4),
                Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
            ),
            "configured operational maximum did not return typed resource refusal",
        )?;
        require_eq(&authority.snapshot()?, &baseline)?;

        let mut permit = admission.reserve_output(3)?;
        let reserved = authority.snapshot()?;
        require_eq(&reserved.used_bytes, &(baseline.used_bytes + 3))?;
        require_eq(&reserved.used_files, &(baseline.used_files + 1))?;
        let staging = root.join("control.tmp");
        let target = root.join("control.json");
        permit.bind_output(&staging, &target)?;
        permit.creation_started()?;
        let mut writer = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)?;
        permit.created(&writer, &staging)?;
        writer.write_all(b"abc")?;
        writer.sync_all()?;
        permit.before_replace()?;
        fs::rename(&staging, &target)?;
        File::open(&root)?.sync_all()?;
        permit.committed(writer, 3)?;
        require_eq(&fs::read(&target)?, &b"abc".to_vec())?;
        require_eq(
            &authority.snapshot()?.used_bytes,
            &(baseline.used_bytes + 3),
        )?;
        drop(permit);
        require_eq(
            &authority.snapshot()?.used_files,
            &(baseline.used_files + 1),
        )?;
        Ok(())
    }

    #[test]
    fn before_replace_refusal_retires_only_after_absence_and_parent_sync() -> TestResult {
        let (_directory, root, authority, binder) = fixture()?;
        let baseline = authority.snapshot()?;
        let admission = CanonicalDurableControlFileAdmission::new(
            Arc::new(PublicationFileDeletions::new(binder)),
            3,
        )?;
        let mut permit = admission.reserve_output(3)?;
        let staging = root.join("refused.tmp");
        let target = root.join("refused.json");
        permit.bind_output(&staging, &target)?;
        permit.creation_started()?;
        let mut writer = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)?;
        permit.created(&writer, &staging)?;
        writer.write_all(b"abc")?;
        writer.sync_all()?;
        symlink("foreign-target", &target)?;
        require(
            permit.before_replace().is_err(),
            "symlink target was accepted",
        )?;
        fs::remove_file(&target)?;
        File::open(&root)?.sync_all()?;
        fs::remove_file(&staging)?;
        File::open(&root)?.sync_all()?;
        permit.confirmed_safe_absent_after_writer_close(writer)?;
        drop(permit);
        require_eq(&authority.snapshot()?, &baseline)?;
        Ok(())
    }

    #[test]
    fn postrename_without_parent_sync_keeps_capacity_charged() -> TestResult {
        let (_directory, root, authority, binder) = fixture()?;
        let baseline = authority.snapshot()?;
        let admission = CanonicalDurableControlFileAdmission::new(
            Arc::new(PublicationFileDeletions::new(binder)),
            3,
        )?;
        let mut permit = admission.reserve_output(3)?;
        let staging = root.join("uncertain.tmp");
        let target = root.join("uncertain.json");
        permit.bind_output(&staging, &target)?;
        permit.creation_started()?;
        let mut writer = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)?;
        permit.created(&writer, &staging)?;
        writer.write_all(b"abc")?;
        writer.sync_all()?;
        permit.before_replace()?;
        fs::rename(&staging, &target)?;
        permit.retire_writer_preserving_pending(writer)?;
        drop(permit);
        require_eq(&fs::read(&target)?, &b"abc".to_vec())?;
        require_eq(
            &authority.snapshot()?.used_bytes,
            &(baseline.used_bytes + 3),
        )?;
        require_eq(
            &authority.snapshot()?.used_files,
            &(baseline.used_files + 1),
        )?;
        Ok(())
    }

    #[test]
    fn committed_width_mismatch_closes_writer_and_retains_canonical_charge() -> TestResult {
        let (_directory, root, authority, binder) = fixture()?;
        let baseline = authority.snapshot()?;
        let admission = CanonicalDurableControlFileAdmission::new(
            Arc::new(PublicationFileDeletions::new(binder)),
            3,
        )?;
        let mut permit = admission.reserve_output(3)?;
        let staging = root.join("mismatch.tmp");
        let target = root.join("mismatch.json");
        permit.bind_output(&staging, &target)?;
        permit.creation_started()?;
        let mut writer = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)?;
        permit.created(&writer, &staging)?;
        writer.write_all(b"abc")?;
        writer.sync_all()?;
        permit.before_replace()?;
        fs::rename(&staging, &target)?;
        File::open(&root)?.sync_all()?;
        let actual_writer_fd = writer.as_raw_fd();
        require(
            matches!(
                permit.committed(writer, 2),
                Err(CoreError::InvalidContract(_))
            ),
            "committed accepted width differing from the actual writer FD",
        )?;
        require(
            fs::metadata(format!("/dev/fd/{actual_writer_fd}")).is_err(),
            "failed committed path retained the actual writer FD",
        )?;
        drop(permit);
        require_eq(&fs::read(&target)?, &b"abc".to_vec())?;
        require_eq(&fs::metadata(&target)?.len(), &3)?;
        let observed = authority.snapshot()?;
        require_eq(&observed.used_bytes, &(baseline.used_bytes + 3))?;
        require_eq(&observed.used_files, &(baseline.used_files + 1))?;
        require(
            observed.uncertain_pending > baseline.uncertain_pending,
            "failed committed path refunded its uncertain pending charge",
        )?;
        Ok(())
    }
}
