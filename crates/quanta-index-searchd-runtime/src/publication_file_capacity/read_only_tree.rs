//! Existing-directory metadata under the actual Common root and Original work.
use super::*;
use quanta_index_core::index::{IndexError, WorkBudget, existing_file::ExistingFileReadFailureV1};
use quanta_index_core::index::{MemoryLifetime, WorkUsage};
use quanta_index_core::{ExistingTreeEntryKindV1, ExistingTreeMetadataIoPortV1};
use std::{ffi::OsStr, fmt};

#[derive(Clone)]
pub(crate) struct ExistingTreeReadIoV1 {
    common: PublicationFileCapacity,
}
impl fmt::Debug for ExistingTreeReadIoV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExistingTreeReadIoV1")
            .finish_non_exhaustive()
    }
}
impl PublicationFileCapacity {
    /// Borrow a read-only metadata facet from THIS already inventoried root.
    /// A StateRootLease alone cannot construct this Common authority.
    pub(crate) fn existing_tree_read_io_v1(&self) -> ExistingTreeReadIoV1 {
        ExistingTreeReadIoV1 {
            common: self.clone(),
        }
    }
}
struct MetadataHandle {
    common: PublicationFileCapacity,
    retired: bool,
}
impl MetadataHandle {
    fn acquire(common: &PublicationFileCapacity) -> Result<Self, CoreError> {
        let mut state = lock(&common.shared.state)?;
        require_publication_open(&state)?;
        let used = state
            .total_pins
            .checked_add(state.total_opening_readers)
            .and_then(|n| n.checked_add(state.total_metadata_readers))
            .ok_or_else(|| capacity("metadata reader count overflow"))?;
        if used >= common.shared.max_open_handles {
            return Err(capacity("metadata reader ceiling exceeded"));
        }
        state.total_metadata_readers = state
            .total_metadata_readers
            .checked_add(1)
            .ok_or_else(|| capacity("metadata reader count overflow"))?;
        Ok(Self {
            common: common.clone(),
            retired: false,
        })
    }
    fn retire(mut self) -> Result<(), CoreError> {
        let mut state = lock(&self.common.shared.state)?;
        if state.close_unwitnessed {
            return Err(contract("metadata directory close was not witnessed"));
        }
        state.total_metadata_readers = state
            .total_metadata_readers
            .checked_sub(1)
            .ok_or_else(|| contract("metadata reader count underflow"))?;
        self.retired = true;
        Ok(())
    }
}
impl Drop for MetadataHandle {
    fn drop(&mut self) {
        if !self.retired {
            self.common.mark_close_unwitnessed();
        }
    }
}

fn visit_existing_entry(
    file: &File,
    raw: &[u8],
    inode: u64,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(
        &OsStr,
        ExistingTreeEntryKindV1,
        u64,
        &mut WorkBudget,
    ) -> Result<(), ExistingFileReadFailureV1>,
) -> Result<(), ExistingFileReadFailureV1> {
    if raw == b"." || raw == b".." {
        return Ok(());
    }
    let name = OsStr::from_bytes(raw);
    budget.charge(WorkUsage {
        reads: 1,
        bytes: u64::try_from(std::mem::size_of::<rustix::fs::Stat>())
            .map_err(|_| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let stat =
        statat(file, Path::new(name), AtFlags::SYMLINK_NOFOLLOW).map_err(std::io::Error::from)?;
    let file_type = FileType::from_raw_mode(stat.st_mode);
    // Symlink permission bits do not control access. Report the unfollowed
    // kind so callers can classify the refusal.
    if stat.st_ino != inode
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || (file_type != FileType::Symlink && stat.st_mode & 0o022 != 0)
    {
        return Err(IndexError::DigestConflict.into());
    }
    let (kind, bytes) = match file_type {
        FileType::Directory => (ExistingTreeEntryKindV1::Directory, 0),
        FileType::RegularFile => (
            ExistingTreeEntryKindV1::File,
            u64::try_from(stat.st_size).map_err(|_| IndexError::DigestConflict)?,
        ),
        FileType::Symlink => (ExistingTreeEntryKindV1::Symlink, 0),
        FileType::Socket => (ExistingTreeEntryKindV1::Socket, 0),
        _ => return Err(IndexError::DigestConflict.into()),
    };
    visit(name, kind, bytes, budget)?;
    if file_type == FileType::Symlink {
        return Err(IndexError::DigestConflict.into());
    }
    Ok(())
}

impl ExistingTreeMetadataIoPortV1 for ExistingTreeReadIoV1 {
    fn visit_directory_v1(
        &self,
        path: &Path,
        budget: &mut WorkBudget,
        visit: &mut dyn FnMut(
            &OsStr,
            ExistingTreeEntryKindV1,
            u64,
            &mut WorkBudget,
        ) -> Result<(), ExistingFileReadFailureV1>,
    ) -> Result<(), ExistingFileReadFailureV1> {
        #[cfg(target_os = "linux")]
        use rustix::fs::RawDir;
        const BUFFER_BYTES: usize = 65_536;
        budget.checkpoint()?;
        // Account the fixed directory buffer and path/anchor copies BEFORE the
        // common namespace traversal opens any directory. This is logical
        // allocation custody, not an RSS or process-wide FD measurement.
        let paths = path
            .as_os_str()
            .len()
            .checked_mul(8)
            .and_then(|n| n.checked_add(std::mem::size_of::<Anchor>().checked_mul(2)?))
            .and_then(|n| n.checked_add(BUFFER_BYTES))
            .ok_or(IndexError::ResourceLimited)?;
        let _memory = budget
            .reserve_memory(
                u64::try_from(paths).map_err(|_| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
            .ok_or(IndexError::ResourceLimited)?;
        let components =
            u64::try_from(path.components().count()).map_err(|_| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            reads: components
                .checked_mul(4)
                .and_then(|n| n.checked_add(4))
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let handle = MetadataHandle::acquire(&self.common)?;
        let result = self.common.with_existing_list_scope_v1(path, true, |file| {
            let before = file.metadata()?;
            let mut buffer = Vec::<u8>::new();
            buffer
                .try_reserve_exact(BUFFER_BYTES)
                .map_err(|_| IndexError::ResourceLimited)?;
            #[cfg(target_os = "linux")]
            {
                let batch = buffer
                    .spare_capacity_mut()
                    .get_mut(..BUFFER_BYTES)
                    .ok_or(IndexError::ResourceLimited)?;
                let mut entries = RawDir::new(file, batch);
                loop {
                    budget.checkpoint()?;
                    if entries.is_buffer_empty() {
                        // RawDir exposes its refill boundary. Charge the complete
                        // possible batch before getdents, rather than charging one
                        // 64KiB batch for every borrowed entry.
                        budget.charge(WorkUsage {
                            reads: 1,
                            bytes: u64::try_from(BUFFER_BYTES)
                                .map_err(|_| IndexError::ResourceLimited)?,
                            ..WorkUsage::default()
                        })?;
                    }
                    let Some(entry) = entries.next() else {
                        break;
                    };
                    let entry = entry.map_err(std::io::Error::from)?;
                    visit_existing_entry(
                        file,
                        entry.file_name().to_bytes(),
                        entry.ino(),
                        budget,
                        visit,
                    )?;
                }
                drop(entries);
            }
            #[cfg(target_os = "macos")]
            {
                buffer.resize(BUFFER_BYTES, 0);
                loop {
                    budget.checkpoint()?;
                    budget.charge(WorkUsage {
                        reads: 1,
                        bytes: u64::try_from(BUFFER_BYTES)
                            .map_err(|_| IndexError::ResourceLimited)?,
                        ..WorkUsage::default()
                    })?;
                    let count = quanta_explicit_fd_process::visit_darwin_directory_batch_v1(
                        file.as_fd(),
                        &mut buffer,
                        |raw, inode| visit_existing_entry(file, raw, inode, budget, visit),
                    )
                    .map_err(|error| match error {
                        quanta_explicit_fd_process::DarwinDirectoryVisitErrorV1::Io(error) => {
                            ExistingFileReadFailureV1::from(error)
                        }
                        quanta_explicit_fd_process::DarwinDirectoryVisitErrorV1::Visit(error) => {
                            error
                        }
                    })?;
                    if count == 0 {
                        break;
                    }
                }
            }
            budget.charge(WorkUsage {
                reads: 2,
                ..WorkUsage::default()
            })?;
            let after = file.metadata()?;
            let current = std::fs::symlink_metadata(path)?;
            fn same(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
                left.dev() == right.dev()
                    && left.ino() == right.ino()
                    && left.len() == right.len()
                    && left.nlink() == right.nlink()
                    && left.uid() == right.uid()
                    && left.mode() == right.mode()
                    && left.mtime() == right.mtime()
                    && left.mtime_nsec() == right.mtime_nsec()
                    && left.ctime() == right.ctime()
                    && left.ctime_nsec() == right.ctime_nsec()
            }
            if !current.is_dir()
                || current.file_type().is_symlink()
                || !same(&before, &after)
                || !same(&before, &current)
            {
                return Err(IndexError::DigestConflict.into());
            }
            budget.checkpoint()?;
            Ok::<(), ExistingFileReadFailureV1>(())
        });
        // The canonical namespace helper explicitly closes every traversed FD.
        // A failed close poisons Common and keeps this original handle charged.
        let retired = handle.retire();
        match (result, retired) {
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error.into()),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use crate::native_resources::{NativeResourceClass, NativeResourceLedger, NativeResourcePlan};
    use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
    use quanta_index_core::index::{
        WorkLimits,
        effort::{ReservationGuard, ResourceNeed},
    };
    use quanta_index_core::{
        RequestBudgetV1, SealedArtifactCommitmentV1, verify_tree_layout_admitted_v1,
    };
    use quanta_index_searchd::app::runtime::StateRootLease;
    type TestResult = Result<(), Box<dyn std::error::Error>>;
    fn require(value: bool, message: &str) -> TestResult {
        if value { Ok(()) } else { Err(message.into()) }
    }
    fn gold() -> Vec<SealedArtifactCommitmentV1> {
        vec![
            SealedArtifactCommitmentV1 {
                name: "dataset/a".into(),
                bytes: 3,
                sha256: [1; 32],
            },
            SealedArtifactCommitmentV1 {
                name: "dataset/nested/b".into(),
                bytes: 5,
                sha256: [2; 32],
            },
        ]
    }
    fn with_fixture(
        run: impl FnOnce(
            &Path,
            &Arc<PublicationFileCapacity>,
            &NativeResourceLedger,
            &mut WorkBudget,
        ) -> TestResult,
    ) -> TestResult {
        let directory = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&directory.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        std::fs::create_dir_all(root.join("dataset/nested"))?;
        std::fs::write(root.join("dataset/a"), b"abc")?;
        std::fs::write(root.join("dataset/nested/b"), b"12345")?;
        let request = RequestBudgetV1::for_duration(std::time::Duration::from_secs(30));
        let unit = ResourceNeed {
            cpu_units: 1,
            io_bytes: 8 * 1024 * 1024,
            memory_bytes: 8 * 1024 * 1024,
            provider_units: 0,
        };
        let plan = NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 4,
                io_bytes: 64 * 1024 * 1024,
                memory_bytes: 64 * 1024 * 1024,
                provider_units: 0,
            },
            control: unit,
            interactive: unit,
            core: unit,
            enrichment: unit,
        };
        let ledger = NativeResourceLedger::new(plan)?;
        let scheduler = ledger.scheduler(NativeResourceClass::Control);
        let control = ReservationGuard::try_acquire(scheduler, plan.control)
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 4096,
                writes: 0,
                bytes: 8 * 1024 * 1024,
                mutations: 0,
            },
            request.clone(),
        );
        budget.bind_memory_scope(control.memory_scope());
        // The fixture's real canonical inventory establishes Common custody.
        // These bodies qualify directory/layout admission, not the complete
        // installed Legacy producer or inventory-memory bootstrap contract.
        let capacity = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes: 1024 * 1024,
                max_files: 16,
                max_pending: 16,
                max_entries: 32,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &request,
        )?;
        let common = PublicationFileCapacity::new(lease, capacity, 4, 64, 256)?;
        run(&root, &common, &ledger, &mut budget)?;
        require(
            common.is_quiescent()?,
            "metadata or close custody remained after the test",
        )?;
        drop(control);
        Ok(())
    }
    #[test]
    fn admitted_existing_tree_holds_original_memory_and_returns_fixed_layout() -> TestResult {
        with_fixture(|root, common, ledger, budget| {
            let baseline = ledger.snapshot()?.live_memory_bytes;
            let io = common.existing_tree_read_io_v1();
            let layout = verify_tree_layout_admitted_v1(
                &root.join("dataset"),
                "dataset",
                &gold(),
                &io,
                budget,
            )?;
            require(
                matches!(layout.outcome_v1(), Ok(8)),
                "fixed two-file layout differed",
            )?;
            require(
                ledger.snapshot()?.live_memory_bytes > baseline,
                "observed allocations escaped their original grants",
            )?;
            require(
                common.is_quiescent()?,
                "directory FD remained open after its visitor",
            )?;
            require(
                std::fs::read(root.join("dataset/a"))? == b"abc"
                    && std::fs::read(root.join("dataset/nested/b"))? == b"12345",
                "read-only inspection changed file contents",
            )?;
            drop(layout);
            require(
                ledger.snapshot()?.live_memory_bytes == baseline,
                "layout data did not release its original memory",
            )
        })
    }
    #[test]
    fn admitted_existing_tree_refuses_missing_extra_resized_and_symlink_data() -> TestResult {
        use quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt;

        for case in 0..4 {
            with_fixture(|root, common, _ledger, budget| {
                let target = root.join("dataset/a");
                match case {
                    0 => std::fs::remove_file(&target)?,
                    1 => std::fs::write(root.join("dataset/extra"), b"unexpected")?,
                    2 => std::fs::write(&target, b"resized")?,
                    _ => {
                        std::fs::remove_file(&target)?;
                        std::os::unix::fs::symlink(root.join("dataset/nested/b"), &target)?;
                        let mut observed = None;
                        let metadata = common.existing_tree_read_io_v1().visit_directory_v1(
                            &root.join("dataset"),
                            budget,
                            &mut |name, kind, bytes, _| {
                                if name != OsStr::new("a") {
                                    return Ok(());
                                }
                                observed = Some((kind, bytes));
                                Err(CoreError::Typed {
                                    code: GenerationSidecarCorrupt,
                                    message: "fixture symlink refusal".into(),
                                }
                                .into())
                            },
                        );
                        require(
                            matches!(observed, Some((ExistingTreeEntryKindV1::Symlink, 0)))
                                && matches!(
                                    metadata,
                                    Err(ExistingFileReadFailureV1::Canonical(CoreError::Typed {
                                        code: GenerationSidecarCorrupt,
                                        message,
                                    })) if message == "fixture symlink refusal"
                                ),
                            "the unfollowed symlink did not preserve the visitor's typed refusal",
                        )?;
                    }
                }
                let result = verify_tree_layout_admitted_v1(
                    &root.join("dataset"),
                    "dataset",
                    &gold(),
                    &common.existing_tree_read_io_v1(),
                    budget,
                );
                let refused = match &result {
                    Err(_) => true,
                    Ok(layout) => layout.outcome_v1().is_err(),
                };
                require(refused, "damaged fixed layout was accepted")?;
                drop(result);
                require(
                    common.is_quiescent()?,
                    "failed layout leaked a directory reader",
                )
            })?;
        }
        Ok(())
    }
    #[test]
    fn admitted_existing_tree_refuses_absent_original_memory_and_cancellation() -> TestResult {
        with_fixture(|root, common, _ledger, budget| {
            let mut unfunded = WorkBudget::with_request(
                WorkLimits {
                    reads: 4096,
                    writes: 0,
                    bytes: 8 * 1024 * 1024,
                    mutations: 0,
                },
                budget.request().clone(),
            );
            require(
                verify_tree_layout_admitted_v1(
                    &root.join("dataset"),
                    "dataset",
                    &gold(),
                    &common.existing_tree_read_io_v1(),
                    &mut unfunded,
                )
                .is_err(),
                "missing original memory fell back to unqualified reads",
            )?;
            require(common.is_quiescent()?, "unfunded read opened a directory")?;
            budget.request().cancel_handle().cancel();
            require(
                verify_tree_layout_admitted_v1(
                    &root.join("dataset"),
                    "dataset",
                    &gold(),
                    &common.existing_tree_read_io_v1(),
                    budget,
                )
                .is_err(),
                "cancelled original read was accepted",
            )
        })
    }
    #[test]
    fn admitted_existing_tree_root_scan_is_repeatable_without_shared_cursor() -> TestResult {
        with_fixture(|root, common, _ledger, budget| {
            let io = common.existing_tree_read_io_v1();
            for _ in 0..2 {
                let mut saw_dataset = false;
                io.visit_directory_v1(root, budget, &mut |name, _, _, _| {
                    if name == OsStr::new("dataset") {
                        saw_dataset = true;
                    }
                    Ok(())
                })?;
                require(
                    saw_dataset,
                    "root enumeration reused an exhausted lease FD cursor",
                )?;
                require(
                    common.is_quiescent()?,
                    "root metadata read retained an open descriptor",
                )?;
            }
            Ok(())
        })
    }
    #[test]
    fn admitted_existing_tree_rejects_replaced_listed_inode_before_visiting() -> TestResult {
        with_fixture(|root, _common, _ledger, budget| {
            let path = root.join("dataset/a");
            let listed_inode = std::fs::symlink_metadata(&path)?.ino();
            std::fs::rename(&path, root.join("dataset/a-old"))?;
            std::fs::write(&path, b"replacement")?;
            let held = File::open(root.join("dataset"))?;
            let mut visited = false;
            let result = visit_existing_entry(
                &held,
                b"a",
                listed_inode,
                budget,
                &mut |_name, _kind, _bytes, _budget| {
                    visited = true;
                    Ok(())
                },
            );
            require(
                result.is_err() && !visited,
                "changed FILEID reached visitor",
            )
        })
    }
}
