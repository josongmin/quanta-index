//! Existing-only immutable reader. No scratch, writer, registration or reclaim
//! capability is constructed. It reuses the canonical wire reader and decoder.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectNamespaceBinding, ObjectReadFile,
    ObjectReadIoPort, ObjectRef, StoredObject, WorkBudget, WorkUsage,
};

use super::{decode_stored_wire, namespace::NamespaceLease, read_io};

pub struct FileObjectReader {
    objects: PathBuf,
    namespace: NamespaceLease,
    io: Arc<dyn ObjectReadIoPort>,
    _allocation: Option<MemoryLease>,
}

impl FileObjectReader {
    /// The supplied reader owns the already-existing namespace lock FD. Both
    /// that FD and each data FD must retain the same existing-root authority.
    /// Missing/private/held-inode/live-owner checks refuse without any mkdir,
    /// create, fsync, Catalog bind, scratch cleanup or recovery.
    pub fn open_existing(
        root: &Path,
        lock: Box<dyn ObjectReadFile>,
        io: Arc<dyn ObjectReadIoPort>,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.charge(WorkUsage {
            reads: 4,
            ..WorkUsage::default()
        })?;
        let bytes = u64::try_from(root.as_os_str().len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(4)
            .and_then(|bytes| bytes.checked_add(1024))
            .ok_or(IndexError::ResourceLimited)?;
        let allocation = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
        let mut namespace = NamespaceLease::existing_with_io(root, lock)?;
        let objects = root.join("objects");
        namespace.bind_objects(&objects)?;
        budget.checkpoint()?;
        Ok(Self {
            objects,
            namespace,
            io,
            _allocation: allocation,
        })
    }

    pub fn namespace_binding(&self) -> Result<ObjectNamespaceBinding, IndexError> {
        self.namespace.validate_objects(&self.objects)
    }

    /// Authentication, bounded decoding and returned allocation custody are
    /// identical to the regular `FileObjectStore` read path.
    pub fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        let wire = read_io::read_wire(
            &self.objects,
            Some(&self.namespace),
            Some(self.io.as_ref()),
            reference,
            budget,
        )?;
        let stored = decode_stored_wire(reference, &wire.bytes, budget)?;
        let _binding = self.namespace_binding()?;
        budget.checkpoint()?;
        Ok(stored)
    }

    /// Authenticate every reachable immutable object, including leaf bytes.
    ///
    /// The caller bounds the pending DFS frontier independently of the original
    /// work budget, which bounds all visits/bytes and preserves cancellation.
    /// Shared children are read again; no unbounded visited set is constructed.
    /// This proves physical wire closure only, not domain/config semantics or
    /// Catalog selection. The caller must retain root custody through use.
    pub fn verify_closure(
        &self,
        roots: &[ObjectRef],
        max_pending: u32,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let maximum = usize::try_from(max_pending).map_err(|_error| IndexError::ResourceLimited)?;
        if maximum == 0 || roots.len() > maximum {
            return Err(IndexError::ResourceLimited);
        }
        let bytes = u64::from(max_pending)
            .checked_mul(
                u64::try_from(std::mem::size_of::<ObjectRef>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        // Locals drop in reverse order: the backing dies before its lease.
        let _pending_memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(maximum)
            .map_err(|_error| IndexError::ResourceLimited)?;
        pending.extend(roots.iter().rev().copied());
        while let Some(reference) = pending.pop() {
            let stored = self.get(reference, budget)?;
            let needed = pending
                .len()
                .checked_add(stored.registration.children.len())
                .ok_or(IndexError::ResourceLimited)?;
            if needed > maximum {
                return Err(IndexError::ResourceLimited);
            }
            pending.extend(stored.registration.children.iter().rev().copied());
        }
        let _binding = self.namespace_binding()?;
        budget.checkpoint()
    }
}

#[cfg(test)]
mod tests;
