//! Managed physical reclamation. One object gate covers a catalog phase change
//! and its filesystem side effect; catalog SQL transactions never cover IO.
use rustix::{
    fs::{self as rfs, AtFlags, Mode, OFlags},
    io::Errno,
};
use sha2::{Digest as _, Sha256};
use std::{
    fs::{self, File},
    io::{ErrorKind, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use quanta_index_core::index::{
    Digest, IndexError, MAX_RECLAIM_DRAIN_PAGE, ObjectNamespaceBinding, ObjectRef,
    PhysicalIncarnation, ReclaimDrainPage, ReclaimKind, ReclaimPermit, ReclaimTarget, ReclaimWork,
    RegisteredObject, WorkBudget, WorkUsage, ports::ManagedObjectReclaimPort,
};

use super::{
    FileObjectStore, HASH_DOMAIN, HEADER_LEN, MAGIC, MAX_CHILDREN, MAX_PAYLOAD, MAX_WIRE,
    namespace::validate_private_root, read_io::ObjectIoFile, read_u32, storage_error,
    validate_private_file,
};

impl FileObjectStore {
    fn reclaim_authority(
        &self,
    ) -> Result<&dyn quanta_index_core::index::ports::ManagedObjectAuthority, IndexError> {
        self.authority.as_deref().ok_or_else(|| {
            IndexError::BackendViolation("standalone object store cannot reclaim".into())
        })
    }

    fn current_namespace(&self) -> Result<ObjectNamespaceBinding, IndexError> {
        self.namespace.validate_objects(&self.objects)
    }

    fn incarnation(
        namespace: ObjectNamespaceBinding,
        shard: &File,
        metadata: &fs::Metadata,
    ) -> Result<PhysicalIncarnation, IndexError> {
        validate_private_file(metadata)?;
        let shard_metadata = shard.metadata().map_err(|error| storage_error(&error))?;
        validate_private_root(&shard_metadata)?;
        let changed_nanos = i128::from(metadata.ctime())
            .checked_mul(1_000_000_000)
            .and_then(|nanos| nanos.checked_add(i128::from(metadata.ctime_nsec())))
            .ok_or(IndexError::ResourceLimited)?;
        Ok(PhysicalIncarnation {
            namespace,
            shard_device: shard_metadata.dev(),
            shard_inode: shard_metadata.ino(),
            device: metadata.dev(),
            inode: metadata.ino(),
            changed_nanos,
            bytes: metadata.len(),
        })
    }

    fn verify_target_shard(
        &self,
        shard: &File,
        reference: ObjectRef,
        expected: &PhysicalIncarnation,
    ) -> Result<(), IndexError> {
        self.verify_shard_link(shard, reference)?;
        let metadata = shard.metadata().map_err(|error| storage_error(&error))?;
        if metadata.dev() != expected.shard_device || metadata.ino() != expected.shard_inode {
            return Err(IndexError::FenceLost);
        }
        Ok(())
    }

    fn open_shard(&self, reference: ObjectRef) -> Result<(Option<File>, PathBuf), IndexError> {
        let binding = self.current_namespace()?;
        let shard_name = format!("{:02x}", (reference.0).0[0]);
        let file_name = self
            .path_for(reference)
            .file_name()
            .ok_or_else(|| IndexError::Storage("object has no file name".into()))?
            .to_os_string();
        let objects = self.namespace.held_objects()?;
        let shard = match rfs::openat(
            objects,
            Path::new(&shard_name),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(descriptor) => File::from(descriptor),
            Err(Errno::NOENT) => return Ok((None, PathBuf::from(file_name))),
            Err(error) => {
                return Err(IndexError::Storage(format!(
                    "open held object shard: {error}"
                )));
            }
        };
        let metadata = shard.metadata().map_err(|error| storage_error(&error))?;
        validate_private_root(&metadata)?;
        if metadata.dev() != binding.objects_device {
            return Err(IndexError::FenceLost);
        }
        self.verify_shard_link(&shard, reference)?;
        Ok((Some(shard), PathBuf::from(file_name)))
    }

    fn verify_shard_link(&self, shard: &File, reference: ObjectRef) -> Result<(), IndexError> {
        let shard_name = format!("{:02x}", (reference.0).0[0]);
        let objects = self.namespace.held_objects()?;
        let linked = rfs::statat(objects, Path::new(&shard_name), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_error| IndexError::FenceLost)?;
        let held = shard.metadata().map_err(|error| storage_error(&error))?;
        #[cfg(target_os = "linux")]
        let linked_device = linked.st_dev;
        #[cfg(not(target_os = "linux"))]
        let linked_device = u64::try_from(linked.st_dev).map_err(|_error| IndexError::FenceLost)?;
        let linked_inode = linked.st_ino;
        if linked_device != held.dev() || linked_inode != held.ino() {
            return Err(IndexError::FenceLost);
        }
        Ok(())
    }

    fn file_entry_identity(shard: &File, name: &Path) -> Result<Option<(u64, u64)>, IndexError> {
        match rfs::statat(shard, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => {
                #[cfg(target_os = "linux")]
                let device = stat.st_dev;
                #[cfg(not(target_os = "linux"))]
                let device = u64::try_from(stat.st_dev).map_err(|_error| IndexError::FenceLost)?;
                Ok(Some((device, stat.st_ino)))
            }
            Err(Errno::NOENT) => Ok(None),
            Err(error) => Err(IndexError::Storage(format!(
                "inspect held object entry: {error}"
            ))),
        }
    }

    fn verify_missing_shard(&self, reference: ObjectRef) -> Result<(), IndexError> {
        let shard_name = format!("{:02x}", (reference.0).0[0]);
        match rfs::statat(
            self.namespace.held_objects()?,
            Path::new(&shard_name),
            AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Err(Errno::NOENT) => Ok(()),
            Ok(_) => Err(IndexError::FenceLost),
            Err(error) => Err(IndexError::Storage(format!(
                "inspect absent object shard: {error}"
            ))),
        }
    }

    /// Decode only the fixed header and bounded child list; hash the payload
    /// through a fixed 64 KiB buffer. Never retain a 16 MiB payload for GC.
    fn stream_registration_in(
        &self,
        shard: &File,
        name: &Path,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(RegisteredObject, fs::Metadata), IndexError> {
        self.verify_shard_link(shard, reference)?;
        let linked = Self::file_entry_identity(shard, name)?.ok_or_else(|| {
            IndexError::CorruptObject("object vanished before stream open".into())
        })?;
        let owner = if let Some(io) = &self.read_io {
            ObjectIoFile::Admitted(io.open(&self.path_for(reference), budget)?)
        } else {
            ObjectIoFile::Plain(File::from(
                rfs::openat(
                    shard,
                    name,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|error| IndexError::Storage(format!("open held object entry: {error}")))?,
            ))
        };
        let mut file = owner.file()?;
        let opened = file.metadata().map_err(|error| storage_error(&error))?;
        validate_private_file(&opened)?;
        if linked != (opened.dev(), opened.ino()) {
            return Err(IndexError::CorruptObject(
                "object changed during stream open".into(),
            ));
        }
        let file_len = usize::try_from(opened.len()).map_err(|_conversion| {
            IndexError::CorruptObject("object length overflows address space".into())
        })?;
        if !(HEADER_LEN..=MAX_WIRE).contains(&file_len) {
            return Err(IndexError::CorruptObject(
                "object length outside wire bounds".into(),
            ));
        }
        budget.charge(WorkUsage {
            reads: 1,
            bytes: opened.len(),
            ..WorkUsage::default()
        })?;
        let mut header = [0u8; HEADER_LEN];
        read_exact_checked(&mut file, &mut header)?;
        if header.get(..8) != Some(MAGIC.as_slice()) {
            return Err(IndexError::CorruptObject("object magic mismatch".into()));
        }
        let payload_len = usize::try_from(read_u32(&header, 8)?)
            .map_err(|_conversion| IndexError::ResourceLimited)?;
        let children_len = usize::try_from(read_u32(&header, 12)?)
            .map_err(|_conversion| IndexError::ResourceLimited)?;
        if payload_len > MAX_PAYLOAD || children_len > MAX_CHILDREN {
            return Err(IndexError::CorruptObject(
                "object lengths exceed limits".into(),
            ));
        }
        let expected_len = HEADER_LEN
            .checked_add(
                children_len
                    .checked_mul(32)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .and_then(|count| count.checked_add(payload_len))
            .ok_or(IndexError::ResourceLimited)?;
        if expected_len != file_len {
            return Err(IndexError::CorruptObject(
                "object wire length mismatch".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(
                children_len
                    .checked_mul(32)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_conversion| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut hasher = Sha256::new();
        hasher.update(HASH_DOMAIN);
        hasher.update(header);
        let registration_memory = budget.reserve_memory(
            RegisteredObject::allocation_bytes(children_len)?,
            quanta_index_core::index::MemoryLifetime::Retained,
        )?;
        let _buffer_memory = budget.reserve_memory(
            64 * 1024,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        let mut children = Vec::new();
        children
            .try_reserve_exact(children_len)
            .map_err(|_allocation| IndexError::ResourceLimited)?;
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(64 * 1024)
            .map_err(|_allocation| IndexError::ResourceLimited)?;
        buffer.resize(64 * 1024, 0u8);
        let mut children_remaining = children_len;
        while children_remaining > 0 {
            budget.checkpoint()?;
            let block_children = buffer
                .len()
                .checked_div(32)
                .ok_or(IndexError::ResourceLimited)?;
            let count = children_remaining.min(block_children);
            let bytes = count.checked_mul(32).ok_or(IndexError::ResourceLimited)?;
            let chunk_bytes = buffer
                .get_mut(..bytes)
                .ok_or_else(|| IndexError::CorruptObject("child block exceeds buffer".into()))?;
            read_exact_checked(&mut file, &mut *chunk_bytes)?;
            hasher.update(&*chunk_bytes);
            for chunk in chunk_bytes.chunks_exact(32) {
                let digest: [u8; 32] = chunk.try_into().map_err(|_conversion| {
                    IndexError::CorruptObject("invalid child reference".into())
                })?;
                children.push(ObjectRef(Digest(digest)));
            }
            children_remaining = children_remaining
                .checked_sub(count)
                .ok_or(IndexError::ResourceLimited)?;
        }
        let mut remaining = payload_len;
        while remaining > 0 {
            budget.checkpoint()?;
            let count = remaining.min(buffer.len());
            let payload_bytes = buffer
                .get_mut(..count)
                .ok_or_else(|| IndexError::CorruptObject("payload block exceeds buffer".into()))?;
            read_exact_checked(&mut file, &mut *payload_bytes)?;
            hasher.update(&*payload_bytes);
            remaining = remaining
                .checked_sub(count)
                .ok_or(IndexError::ResourceLimited)?;
        }
        let mut extra = [0u8; 1];
        if file
            .read(&mut extra)
            .map_err(|error| storage_error(&error))?
            != 0
        {
            return Err(IndexError::CorruptObject(
                "object grew during stream read".into(),
            ));
        }
        let after = file.metadata().map_err(|error| storage_error(&error))?;
        if after.dev() != opened.dev()
            || after.ino() != opened.ino()
            || after.len() != opened.len()
            || after.ctime() != opened.ctime()
            || after.ctime_nsec() != opened.ctime_nsec()
        {
            return Err(IndexError::CorruptObject(
                "object changed during stream read".into(),
            ));
        }
        if ObjectRef(Digest(hasher.finalize().into())) != reference {
            return Err(IndexError::CorruptObject("object digest mismatch".into()));
        }
        self.verify_shard_link(shard, reference)?;
        if Self::file_entry_identity(shard, name)? != Some((opened.dev(), opened.ino())) {
            return Err(IndexError::FenceLost);
        }
        Ok((
            RegisteredObject::from_parts(reference, opened.len(), children, registration_memory)?,
            opened,
        ))
    }

    #[cfg(test)]
    fn stream_registration(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        let (shard, name) = self.open_shard(reference)?;
        let shard = shard.ok_or_else(|| IndexError::CorruptObject("object shard absent".into()))?;
        self.stream_registration_in(&shard, &name, reference, budget)
            .map(|(registration, _metadata)| registration)
    }

    fn inspect_target(
        &self,
        reference: ObjectRef,
        kind: ReclaimKind,
        budget: &mut WorkBudget,
    ) -> Result<ReclaimTarget, IndexError> {
        let namespace = self.current_namespace()?;
        let (shard, name) = self.open_shard(reference)?;
        if let Some(shard) = shard {
            if Self::file_entry_identity(&shard, &name)?.is_some() {
                let (registration, metadata) =
                    self.stream_registration_in(&shard, &name, reference, budget)?;
                let incarnation = Self::incarnation(namespace, &shard, &metadata)?;
                if registration.bytes != incarnation.bytes {
                    return Err(IndexError::CorruptObject(
                        "object changed during reclaim inspection".into(),
                    ));
                }
                return Ok(ReclaimTarget::Present {
                    incarnation,
                    registration,
                });
            }
            self.verify_shard_link(&shard, reference)?;
        } else {
            self.verify_missing_shard(reference)?;
        }
        if kind == ReclaimKind::Provisional {
            Ok(ReclaimTarget::AbsentProvisional(namespace))
        } else {
            Err(IndexError::CorruptObject(
                "registered object disappeared before reclaim".into(),
            ))
        }
    }

    fn sync_absence(
        &self,
        shard: Option<&File>,
        name: &Path,
        reference: ObjectRef,
        shard_already_synced: bool,
    ) -> Result<(), IndexError> {
        if let Some(shard) = shard {
            if !shard_already_synced {
                shard.sync_all().map_err(|error| storage_error(&error))?;
            }
            self.verify_shard_link(shard, reference)?;
            if Self::file_entry_identity(shard, name)?.is_some() {
                return Err(IndexError::FenceLost);
            }
        } else {
            self.verify_missing_shard(reference)?;
        }
        self.namespace
            .held_objects()?
            .sync_all()
            .map_err(|error| storage_error(&error))
    }

    fn physical_unlink(
        &self,
        permit: &ReclaimPermit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let reference = permit.retiring.candidate.object;
        let namespace = self.current_namespace()?;
        let (shard, name) = self.open_shard(reference)?;
        let mut shard_already_synced = false;
        match &permit.target {
            ReclaimTarget::Present {
                incarnation: expected,
                registration,
            } => {
                if expected.namespace != namespace || registration.reference != reference {
                    return Err(IndexError::FenceLost);
                }
                let shard = shard.as_ref().ok_or(IndexError::FenceLost)?;
                self.verify_target_shard(shard, reference, expected)?;
                if Self::file_entry_identity(shard, &name)?.is_some() {
                    let (actual_registration, metadata) =
                        self.stream_registration_in(shard, &name, reference, budget)?;
                    if Self::incarnation(namespace, shard, &metadata)? != *expected
                        || &actual_registration != registration
                        || Self::file_entry_identity(shard, &name)?
                            != Some((expected.device, expected.inode))
                    {
                        return Err(IndexError::FenceLost);
                    }
                    self.verify_target_shard(shard, reference, expected)?;
                    budget.charge(WorkUsage {
                        writes: 1,
                        ..WorkUsage::default()
                    })?;
                    budget.checkpoint()?;
                    // Once unlink starts, sync must run even when cancellation
                    // arrives. Reclaiming remains durable until mark_unlinked.
                    if let Some(io) = &self.reclaim_io {
                        io.unlink_and_sync(&self.path_for(reference), expected)?;
                        shard_already_synced = true;
                    } else {
                        rfs::unlinkat(shard, &name, AtFlags::empty()).map_err(|error| {
                            IndexError::Storage(format!("unlink held object entry: {error}"))
                        })?;
                    }
                } else if let Some(io) = &self.reclaim_io {
                    io.confirm_absence_and_sync(&self.path_for(reference), expected)?;
                    shard_already_synced = true;
                }
            }
            ReclaimTarget::AbsentProvisional(expected) => {
                if permit.retiring.candidate.kind != ReclaimKind::Provisional
                    || *expected != namespace
                {
                    return Err(IndexError::FenceLost);
                }
                if let Some(shard) = shard.as_ref() {
                    if Self::file_entry_identity(shard, &name)?.is_some() {
                        return Err(IndexError::FenceLost);
                    }
                    self.verify_shard_link(shard, reference)?;
                }
            }
        }
        self.sync_absence(shard.as_ref(), &name, reference, shard_already_synced)?;
        self.reclaim_authority()?
            .mark_unlinked(permit.clone(), budget)
    }

    fn verify_draining_absence(&self, permit: &ReclaimPermit) -> Result<(), IndexError> {
        let namespace = self.current_namespace()?;
        let expected = match &permit.target {
            ReclaimTarget::Present { incarnation, .. } => incarnation.namespace,
            ReclaimTarget::AbsentProvisional(binding) => *binding,
        };
        if namespace != expected {
            return Err(IndexError::FenceLost);
        }
        let reference = permit.retiring.candidate.object;
        let (shard, name) = self.open_shard(reference)?;
        match (&permit.target, shard) {
            (ReclaimTarget::Present { incarnation, .. }, Some(shard)) => {
                self.verify_target_shard(&shard, reference, incarnation)?;
                if Self::file_entry_identity(&shard, &name)?.is_some() {
                    return Err(IndexError::FenceLost);
                }
            }
            (ReclaimTarget::Present { .. }, None) => return Err(IndexError::FenceLost),
            (ReclaimTarget::AbsentProvisional(_), Some(shard)) => {
                if Self::file_entry_identity(&shard, &name)?.is_some() {
                    return Err(IndexError::FenceLost);
                }
                self.verify_shard_link(&shard, reference)?;
            }
            (ReclaimTarget::AbsentProvisional(_), None) => self.verify_missing_shard(reference)?,
        }
        Ok(())
    }
}

impl ManagedObjectReclaimPort for FileObjectStore {
    fn reclaim_one(
        &self,
        work: ReclaimWork,
        child_limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<Option<ReclaimDrainPage>, IndexError> {
        if !(1..=MAX_RECLAIM_DRAIN_PAGE).contains(&child_limit) {
            return Err(IndexError::InvalidInput(
                "reclaim child page limit out of range".into(),
            ));
        }
        budget.checkpoint()?;
        let _gate = self.lock_gate(Self::gate_index(work.object()), budget)?;
        let authority = self.reclaim_authority()?;
        let permit = match work {
            ReclaimWork::Candidate(candidate) => {
                let Some(retiring) = authority.retire_candidate(candidate, budget)? else {
                    return Ok(None);
                };
                let target = self.inspect_target(candidate.object, candidate.kind, budget)?;
                authority.enter_reclaiming(retiring, target, budget)?
            }
            ReclaimWork::Reclaiming(permit) => {
                authority.validate_reclaiming(permit.clone(), budget)?;
                permit
            }
            ReclaimWork::Draining(permit) => {
                self.verify_draining_absence(&permit)?;
                return authority
                    .drain_unlinked(permit, child_limit, budget)
                    .map(Some);
            }
        };
        self.physical_unlink(&permit, budget)?;
        authority
            .drain_unlinked(permit, child_limit, budget)
            .map(Some)
    }
}

fn read_exact_checked(file: &mut impl Read, buffer: &mut [u8]) -> Result<(), IndexError> {
    file.read_exact(buffer).map_err(|error| {
        if error.kind() == ErrorKind::UnexpectedEof {
            IndexError::CorruptObject("object truncated during stream read".into())
        } else {
            storage_error(&error)
        }
    })
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fs::{self, File, OpenOptions},
        io::Write,
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{
        ObjectInstallScope, ReclaimCandidate, ReclaimDrainPage, ReclaimKind, ReclaimPermit,
        ReclaimTarget, ReclaimWork, ReclaimWorkPage, RegisteredObject, RetiringRef, WorkLimits,
        ports::{
            CatalogReclaimPort, ImmutableObjectPort, ManagedObjectAuthority,
            ManagedObjectInstallPort, ManagedObjectReclaimPort,
        },
    };

    use super::*;

    fn assertion_error<E: From<IndexError>>(detail: String) -> E {
        IndexError::BackendViolation(detail).into()
    }

    // Fallible assertions preserve test predicates in Result-returning fixtures.
    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(assertion_error(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(assertion_error(format!(
                    "assertion failed: left {left:?} != right {right:?}"
                )));
            }
        }};
    }

    macro_rules! check_ne {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left == right {
                return Err(assertion_error(format!(
                    "assertion failed: left {left:?} == right {right:?}"
                )));
            }
        }};
    }
    use crate::FileScratchSpace;

    type TestResult = Result<(), Box<dyn Error>>;

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 1_024,
                writes: 256,
                bytes: 67_108_864,
                mutations: 1_024,
            },
            Instant::now()
                .checked_add(Duration::from_secs(60))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[derive(Default)]
    struct ReclaimFixtureAuthority {
        marks: AtomicUsize,
        drains: AtomicUsize,
        cancel_mark_once: AtomicBool,
        reject_generation: AtomicU64,
    }

    fn fixture_unavailable<T>() -> Result<T, IndexError> {
        Err(IndexError::BackendViolation(
            "unused fixture catalog path".into(),
        ))
    }

    impl ManagedObjectInstallPort for ReclaimFixtureAuthority {
        fn reserve_managed(
            &self,
            _scope: ObjectInstallScope,
            _objects: &[RegisteredObject],
            _budget: &mut WorkBudget,
        ) -> Result<(), IndexError> {
            fixture_unavailable()
        }
        fn confirm_managed(
            &self,
            _scope: ObjectInstallScope,
            _objects: &[RegisteredObject],
            _budget: &mut WorkBudget,
        ) -> Result<(), IndexError> {
            fixture_unavailable()
        }
    }

    impl CatalogReclaimPort for ReclaimFixtureAuthority {
        fn reclaim_work_page(
            &self,
            _after: Option<ObjectRef>,
            _limit: u32,
            _budget: &mut WorkBudget,
        ) -> Result<ReclaimWorkPage, IndexError> {
            fixture_unavailable()
        }
        fn retire_candidate(
            &self,
            _candidate: ReclaimCandidate,
            _budget: &mut WorkBudget,
        ) -> Result<Option<RetiringRef>, IndexError> {
            Ok(None)
        }
        fn enter_reclaiming(
            &self,
            _retiring: RetiringRef,
            _target: ReclaimTarget,
            _budget: &mut WorkBudget,
        ) -> Result<ReclaimPermit, IndexError> {
            fixture_unavailable()
        }
        fn validate_reclaiming(
            &self,
            permit: ReclaimPermit,
            _budget: &mut WorkBudget,
        ) -> Result<(), IndexError> {
            if self.reject_generation.load(Ordering::SeqCst) == permit.retiring.candidate.generation
            {
                return Err(IndexError::FenceLost);
            }
            Ok(())
        }
        fn mark_unlinked(
            &self,
            _permit: ReclaimPermit,
            budget: &mut WorkBudget,
        ) -> Result<(), IndexError> {
            if self.cancel_mark_once.swap(false, Ordering::SeqCst) {
                budget.request().cancel_handle().cancel();
                return Err(IndexError::Cancelled);
            }
            let _prior = self.marks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn drain_unlinked(
            &self,
            _permit: ReclaimPermit,
            _limit: u32,
            _budget: &mut WorkBudget,
        ) -> Result<ReclaimDrainPage, IndexError> {
            let _prior = self.drains.fetch_add(1, Ordering::SeqCst);
            Ok(ReclaimDrainPage {
                released_children: 0,
                enqueued_children: 0,
                has_more: false,
            })
        }
    }

    impl ManagedObjectAuthority for ReclaimFixtureAuthority {
        fn bind_namespace(&self, _binding: ObjectNamespaceBinding) -> Result<(), IndexError> {
            Ok(())
        }
    }

    fn managed_store(
        parent: &tempfile::TempDir,
        authority: Arc<ReclaimFixtureAuthority>,
    ) -> Result<FileObjectStore, IndexError> {
        let scratch = Arc::new(FileScratchSpace::open_standalone(
            parent.path().join("managed-scratch"),
            1024 * 1024,
        )?);
        FileObjectStore::open_managed_with_scratch(parent.path().join("store"), scratch, authority)
    }

    fn seed(parent: &tempfile::TempDir) -> Result<RegisteredObject, IndexError> {
        let standalone = FileObjectStore::open(parent.path().join("store"))?;
        let registration = standalone.put(b"fixed-wire", &[], &mut budget()?)?;
        drop(standalone);
        Ok(registration)
    }

    fn seed_with_two_children(parent: &tempfile::TempDir) -> Result<RegisteredObject, IndexError> {
        let standalone = FileObjectStore::open(parent.path().join("store"))?;
        let first = standalone.put(b"first-child", &[], &mut budget()?)?;
        let second = standalone.put(b"second-child", &[], &mut budget()?)?;
        let registration = standalone.put(
            b"parent",
            &[first.reference, second.reference],
            &mut budget()?,
        )?;
        drop(standalone);
        Ok(registration)
    }

    fn permit(
        store: &FileObjectStore,
        registration: RegisteredObject,
    ) -> Result<ReclaimPermit, IndexError> {
        let namespace = store.current_namespace()?;
        let (shard, _name) = store.open_shard(registration.reference)?;
        let shard = shard.ok_or(IndexError::FenceLost)?;
        let metadata = fs::symlink_metadata(store.path_for(registration.reference))
            .map_err(|error| storage_error(&error))?;
        let incarnation = FileObjectStore::incarnation(namespace, &shard, &metadata)?;
        Ok(ReclaimPermit {
            retiring: RetiringRef {
                candidate: ReclaimCandidate {
                    object: registration.reference,
                    generation: 1,
                    kind: ReclaimKind::Registered,
                },
                direct_children: 0,
            },
            target: ReclaimTarget::Present {
                incarnation,
                registration,
            },
        })
    }

    #[test]
    fn read_owner_refusal_never_falls_back_to_raw_get_or_gc_read() -> TestResult {
        #[derive(Debug)]
        struct Refusal(Arc<AtomicUsize>);
        impl quanta_index_core::index::ObjectReadIoPort for Refusal {
            fn open(
                &self,
                _: &Path,
                _: &mut WorkBudget,
            ) -> Result<Box<dyn quanta_index_core::index::ObjectReadFile>, IndexError> {
                let _previous = self.0.fetch_add(1, Ordering::SeqCst);
                Err(IndexError::ResourceLimited)
            }
        }
        let parent = tempfile::tempdir()?;
        let registration = seed(&parent)?;
        let calls = Arc::new(AtomicUsize::new(0));
        let store = managed_store(&parent, Arc::new(ReclaimFixtureAuthority::default()))?
            .with_read_io(Arc::new(Refusal(Arc::clone(&calls))))?;
        let path = store.path_for(registration.reference);
        let golden = fs::read(&path)?;
        check!(matches!(
            store.get(registration.reference, &mut budget()?),
            Err(IndexError::ResourceLimited)
        ));
        check!(matches!(
            store.stream_registration(registration.reference, &mut budget()?),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(calls.load(Ordering::SeqCst), 2);
        check_eq!(fs::read(path)?, golden);
        Ok(())
    }

    #[test]
    fn substituted_read_inode_is_rejected_and_owner_drops_once() -> TestResult {
        struct HeldRead {
            file: File,
            drops: Arc<AtomicUsize>,
        }
        impl quanta_index_core::index::ObjectReadFile for HeldRead {
            fn file(&self) -> Result<&File, IndexError> {
                Ok(&self.file)
            }
        }
        impl Drop for HeldRead {
            fn drop(&mut self) {
                let _previous = self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }
        #[derive(Debug)]
        struct SubstitutedRead {
            path: PathBuf,
            drops: Arc<AtomicUsize>,
        }
        impl quanta_index_core::index::ObjectReadIoPort for SubstitutedRead {
            fn open(
                &self,
                _: &Path,
                _: &mut WorkBudget,
            ) -> Result<Box<dyn quanta_index_core::index::ObjectReadFile>, IndexError> {
                Ok(Box::new(HeldRead {
                    file: File::open(&self.path).map_err(|error| storage_error(&error))?,
                    drops: Arc::clone(&self.drops),
                }))
            }
        }
        let parent = tempfile::tempdir()?;
        let registration = seed(&parent)?;
        let store = managed_store(&parent, Arc::new(ReclaimFixtureAuthority::default()))?;
        let path = store.path_for(registration.reference);
        let golden = fs::read(&path)?;
        let substitute = parent.path().join("different-inode");
        fs::write(&substitute, &golden)?;
        fs::set_permissions(&substitute, fs::Permissions::from_mode(0o600))?;
        let drops = Arc::new(AtomicUsize::new(0));
        let store = store.with_read_io(Arc::new(SubstitutedRead {
            path: substitute,
            drops: Arc::clone(&drops),
        }))?;
        check!(matches!(
            store.get(registration.reference, &mut budget()?),
            Err(IndexError::CorruptObject(_))
        ));
        check!(matches!(
            store.stream_registration(registration.reference, &mut budget()?),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(drops.load(Ordering::SeqCst), 2);
        check_eq!(fs::read(path)?, golden);
        Ok(())
    }

    #[test]
    fn stream_rejects_changed_wire_header() -> TestResult {
        let parent = tempfile::tempdir()?;
        let registration = seed(&parent)?;
        let store = managed_store(&parent, Arc::new(ReclaimFixtureAuthority::default()))?;
        let path = store.path_for(registration.reference);
        let mut file = OpenOptions::new().write(true).open(path)?;
        file.write_all(b"BADMAGIC")?;
        file.sync_all()?;
        check!(matches!(
            store.stream_registration(registration.reference, &mut budget()?),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    #[test]
    fn physical_owner_refusal_or_false_success_cannot_unlink_or_mark_catalog() -> TestResult {
        #[derive(Debug)]
        struct UnlinkFixture {
            calls: Arc<AtomicUsize>,
            refuse: bool,
        }
        impl quanta_index_core::index::ObjectUnlinkPort for UnlinkFixture {
            fn unlink_and_sync(&self, _: &Path, _: &PhysicalIncarnation) -> Result<(), IndexError> {
                let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
                if self.refuse {
                    Err(IndexError::ResourceLimited)
                } else {
                    Ok(())
                }
            }
            fn confirm_absence_and_sync(
                &self,
                _: &Path,
                _: &PhysicalIncarnation,
            ) -> Result<(), IndexError> {
                Err(IndexError::BackendViolation(
                    "unexpected fixture absence".into(),
                ))
            }
        }
        for refuse in [false, true] {
            let parent = tempfile::tempdir()?;
            let registration = seed(&parent)?;
            let authority = Arc::new(ReclaimFixtureAuthority::default());
            let calls = Arc::new(AtomicUsize::new(0));
            let store = managed_store(&parent, Arc::clone(&authority))?.with_reclaim_io(
                Arc::new(UnlinkFixture {
                    calls: Arc::clone(&calls),
                    refuse,
                }),
            )?;
            let path = store.path_for(registration.reference);
            let golden = fs::read(&path)?;
            let work = ReclaimWork::Reclaiming(permit(&store, registration)?);
            let result = store.reclaim_one(work, 1, &mut budget()?);
            if refuse {
                check!(matches!(result, Err(IndexError::ResourceLimited)));
            } else {
                check!(matches!(result, Err(IndexError::FenceLost)));
            }
            check_eq!(calls.load(Ordering::SeqCst), 1);
            check_eq!(fs::read(&path)?, golden);
            check_eq!(authority.marks.load(Ordering::SeqCst), 0);
            check_eq!(authority.drains.load(Ordering::SeqCst), 0);
        }
        Ok(())
    }

    #[test]
    fn old_physical_stamp_cannot_unlink_reinstalled_same_digest() -> TestResult {
        let parent = tempfile::tempdir()?;
        let registration = seed(&parent)?;
        let authority = Arc::new(ReclaimFixtureAuthority::default());
        let store = managed_store(&parent, authority.clone())?;
        let permit = permit(&store, registration.clone())?;
        let path = store.path_for(registration.reference);
        let old_file = File::open(&path)?;
        let wire = fs::read(&path)?;
        fs::remove_file(&path)?;
        let mut replacement = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        replacement.write_all(&wire)?;
        replacement.sync_all()?;
        check_ne!(old_file.metadata()?.ino(), replacement.metadata()?.ino());
        check!(matches!(
            store.reclaim_one(ReclaimWork::Reclaiming(permit), 1, &mut budget()?),
            Err(IndexError::FenceLost)
        ));
        check!(path.exists());
        check_eq!(authority.marks.load(Ordering::SeqCst), 0);
        check_eq!(authority.drains.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn same_child_count_with_different_order_cannot_unlink() -> TestResult {
        let parent = tempfile::tempdir()?;
        let registration = seed_with_two_children(&parent)?;
        let authority = Arc::new(ReclaimFixtureAuthority::default());
        let store = managed_store(&parent, authority.clone())?;
        let mut permit = permit(&store, registration.clone())?;
        permit.retiring.direct_children = 2;
        if let ReclaimTarget::Present { registration, .. } = &mut permit.target {
            let mut children = registration.children.clone();
            children.swap(0, 1);
            *registration =
                RegisteredObject::unscoped(registration.reference, registration.bytes, children);
        }
        let path = store.path_for(registration.reference);
        check!(matches!(
            store.reclaim_one(ReclaimWork::Reclaiming(permit), 1, &mut budget()?),
            Err(IndexError::FenceLost)
        ));
        check!(path.exists());
        check_eq!(authority.marks.load(Ordering::SeqCst), 0);
        check_eq!(authority.drains.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn shard_symlink_cannot_redirect_reclaim_to_detached_object() -> TestResult {
        let parent = tempfile::tempdir()?;
        let registration = seed(&parent)?;
        let authority = Arc::new(ReclaimFixtureAuthority::default());
        let store = managed_store(&parent, authority.clone())?;
        let permit = permit(&store, registration.clone())?;
        let path = store.path_for(registration.reference);
        let shard = path
            .parent()
            .ok_or(IndexError::ResourceLimited)?
            .to_path_buf();
        let name = path
            .file_name()
            .ok_or(IndexError::ResourceLimited)?
            .to_os_string();
        let detached = parent.path().join("detached-shard");
        fs::rename(&shard, &detached)?;
        symlink(&detached, &shard)?;
        let result = store.reclaim_one(ReclaimWork::Reclaiming(permit), 1, &mut budget()?);
        check!(matches!(
            result,
            Err(IndexError::Storage(_) | IndexError::FenceLost)
        ));
        check!(detached.join(name).exists());
        check_eq!(authority.marks.load(Ordering::SeqCst), 0);
        check_eq!(authority.drains.load(Ordering::SeqCst), 0);
        fs::remove_file(&shard)?;
        fs::rename(detached, shard)?;
        Ok(())
    }

    #[test]
    fn replaced_empty_shard_cannot_fake_completed_unlink() -> TestResult {
        let parent = tempfile::tempdir()?;
        let registration = seed(&parent)?;
        let authority = Arc::new(ReclaimFixtureAuthority::default());
        let store = managed_store(&parent, authority.clone())?;
        let permit = permit(&store, registration.clone())?;
        let path = store.path_for(registration.reference);
        let shard = path
            .parent()
            .ok_or(IndexError::ResourceLimited)?
            .to_path_buf();
        let name = path
            .file_name()
            .ok_or(IndexError::ResourceLimited)?
            .to_os_string();
        let detached = parent.path().join("detached-shard");
        fs::rename(&shard, &detached)?;
        fs::DirBuilder::new().mode(0o700).create(&shard)?;
        let result = store.reclaim_one(ReclaimWork::Reclaiming(permit), 1, &mut budget()?);
        check!(matches!(result, Err(IndexError::FenceLost)));
        check!(detached.join(name).exists());
        check_eq!(authority.marks.load(Ordering::SeqCst), 0);
        check_eq!(authority.drains.load(Ordering::SeqCst), 0);
        fs::remove_dir(&shard)?;
        fs::rename(detached, shard)?;
        Ok(())
    }

    #[test]
    fn mismatched_namespace_cannot_unlink_or_release_children() -> TestResult {
        let parent = tempfile::tempdir()?;
        let registration = seed(&parent)?;
        let authority = Arc::new(ReclaimFixtureAuthority::default());
        let store = managed_store(&parent, authority.clone())?;
        let mut permit = permit(&store, registration.clone())?;
        if let ReclaimTarget::Present { incarnation, .. } = &mut permit.target {
            incarnation.namespace.lease_inode = incarnation
                .namespace
                .lease_inode
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        let path = store.path_for(registration.reference);
        check!(matches!(
            store.reclaim_one(ReclaimWork::Reclaiming(permit), 1, &mut budget()?),
            Err(IndexError::FenceLost)
        ));
        check!(path.exists());
        check_eq!(authority.marks.load(Ordering::SeqCst), 0);
        check_eq!(authority.drains.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn stale_catalog_generation_cannot_unlink_or_release_children() -> TestResult {
        let parent = tempfile::tempdir()?;
        let registration = seed(&parent)?;
        let authority = Arc::new(ReclaimFixtureAuthority::default());
        authority.reject_generation.store(1, Ordering::SeqCst);
        let store = managed_store(&parent, authority.clone())?;
        let permit = permit(&store, registration.clone())?;
        let path = store.path_for(registration.reference);
        check!(matches!(
            store.reclaim_one(ReclaimWork::Reclaiming(permit), 1, &mut budget()?),
            Err(IndexError::FenceLost)
        ));
        check!(path.exists());
        check_eq!(authority.marks.load(Ordering::SeqCst), 0);
        check_eq!(authority.drains.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn unsafe_shard_permissions_cannot_release_children() -> TestResult {
        let parent = tempfile::tempdir()?;
        let registration = seed(&parent)?;
        let authority = Arc::new(ReclaimFixtureAuthority::default());
        let store = managed_store(&parent, authority.clone())?;
        let permit = permit(&store, registration.clone())?;
        let path = store.path_for(registration.reference);
        let shard = path.parent().ok_or(IndexError::ResourceLimited)?;
        fs::set_permissions(shard, fs::Permissions::from_mode(0o500))?;
        let result = store.reclaim_one(ReclaimWork::Reclaiming(permit), 1, &mut budget()?);
        fs::set_permissions(shard, fs::Permissions::from_mode(0o700))?;
        check!(matches!(result, Err(IndexError::Storage(_))));
        check!(path.exists());
        check_eq!(authority.marks.load(Ordering::SeqCst), 0);
        check_eq!(authority.drains.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn cancellation_after_unlink_preserves_reclaiming_for_next_call() -> TestResult {
        let parent = tempfile::tempdir()?;
        let registration = seed(&parent)?;
        let authority = Arc::new(ReclaimFixtureAuthority::default());
        authority.cancel_mark_once.store(true, Ordering::SeqCst);
        let store = managed_store(&parent, authority.clone())?;
        let permit = permit(&store, registration.clone())?;
        let path = store.path_for(registration.reference);
        check!(matches!(
            store.reclaim_one(ReclaimWork::Reclaiming(permit.clone()), 1, &mut budget()?),
            Err(IndexError::Cancelled)
        ));
        check!(!path.exists());
        check_eq!(authority.drains.load(Ordering::SeqCst), 0);
        let resumed = store.reclaim_one(ReclaimWork::Reclaiming(permit), 1, &mut budget()?)?;
        check!(matches!(resumed, Some(page) if !page.has_more));
        check_eq!(authority.marks.load(Ordering::SeqCst), 1);
        check_eq!(authority.drains.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn reclaim_waits_for_held_object_gate() -> TestResult {
        let parent = tempfile::tempdir()?;
        let registration = seed(&parent)?;
        let authority = Arc::new(ReclaimFixtureAuthority::default());
        let store = Arc::new(managed_store(&parent, authority)?);
        let gate = store
            .gates
            .get(FileObjectStore::gate_index(registration.reference))
            .ok_or_else(|| IndexError::BackendViolation("fixture gate missing".into()))?
            .lock()
            .map_err(|_poison| IndexError::BackendViolation("fixture gate poisoned".into()))?;
        let candidate = ReclaimCandidate {
            object: registration.reference,
            generation: 1,
            kind: ReclaimKind::Registered,
        };
        let (sender, receiver) = mpsc::channel();
        let worker_store = store.clone();
        let worker = thread::spawn(move || {
            let result = budget().and_then(|mut work| {
                worker_store.reclaim_one(ReclaimWork::Candidate(candidate), 1, &mut work)
            });
            let _sent = sender.send(result);
        });
        check!(matches!(
            receiver.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(gate);
        let result = receiver.recv_timeout(Duration::from_secs(5))??;
        check!(result.is_none());
        worker
            .join()
            .map_err(|_panic| IndexError::BackendViolation("reclaim worker panicked".into()))?;
        Ok(())
    }

    #[test]
    fn held_gate_respects_original_request_deadline() -> TestResult {
        let parent = tempfile::tempdir()?;
        let registration = seed(&parent)?;
        let authority = Arc::new(ReclaimFixtureAuthority::default());
        let store = Arc::new(managed_store(&parent, authority.clone())?);
        let gate = store
            .gates
            .get(FileObjectStore::gate_index(registration.reference))
            .ok_or_else(|| IndexError::BackendViolation("fixture gate missing".into()))?
            .lock()
            .map_err(|_poison| IndexError::BackendViolation("fixture gate poisoned".into()))?;
        let permit = permit(&store, registration.clone())?;
        let worker_store = store.clone();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let deadline = Instant::now()
                .checked_add(Duration::from_millis(50))
                .ok_or(IndexError::ResourceLimited)?;
            let mut original_budget = WorkBudget::new(
                WorkLimits {
                    reads: 1_024,
                    writes: 256,
                    bytes: 67_108_864,
                    mutations: 1_024,
                },
                deadline,
            );
            let result =
                worker_store.reclaim_one(ReclaimWork::Reclaiming(permit), 1, &mut original_budget);
            let _sent = sender.send(result);
            Ok::<(), IndexError>(())
        });
        let before_unlock = receiver.recv_timeout(Duration::from_secs(2));
        drop(gate);
        worker
            .join()
            .map_err(|_panic| IndexError::BackendViolation("reclaim worker panicked".into()))??;
        let result = before_unlock?;
        check!(matches!(result, Err(IndexError::ResourceLimited)));
        check!(store.path_for(registration.reference).exists());
        check_eq!(authority.marks.load(Ordering::SeqCst), 0);
        check_eq!(authority.drains.load(Ordering::SeqCst), 0);
        Ok(())
    }
}
