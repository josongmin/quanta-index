//! Existing-only immutable reader. No scratch, writer, registration or reclaim
//! capability is constructed. It reuses the canonical immutable object reader.

use std::{
    hash::{BuildHasher, RandomState},
    path::{Path, PathBuf},
    sync::Arc,
};

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectNamespaceBinding, ObjectReadFile,
    ObjectReadIoPort, ObjectRef, RegisteredObject, StoredObject, WorkBudget, WorkUsage,
};

use super::{namespace::NamespaceLease, read_io};

use quanta_index_core::index::{
    ObjectBatchInput,
    ports::{ImmutableObjectPort, ObjectWriteSink},
};

mod source_files;

pub struct FileObjectReader {
    objects: PathBuf,
    namespace: NamespaceLease,
    io: Arc<dyn ObjectReadIoPort>,
    _allocation: Option<MemoryLease>,
}

impl ImmutableObjectPort for FileObjectReader {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        FileObjectReader::identify(self, payload, children, budget)
    }

    fn put(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "read-only object port cannot write".into(),
        ))
    }

    fn install(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _sink: &dyn ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "read-only object port cannot write".into(),
        ))
    }

    fn install_batch(
        &self,
        _inputs: &[ObjectBatchInput],
        _sink: &dyn ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        Err(IndexError::BackendViolation(
            "read-only object port cannot write".into(),
        ))
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        FileObjectReader::get(self, reference, budget)
    }
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

    /// Effect-free identity using the same canonical encoding as the writer.
    /// This neither opens storage nor requires retained child objects.
    pub fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObject, IndexError> {
        super::write_preparation::identify(payload, children, budget)
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
        let stored = read_io::read_stored(
            &self.objects,
            Some(&self.namespace),
            Some(self.io.as_ref()),
            reference,
            budget,
        )?;
        let _binding = self.namespace_binding()?;
        budget.checkpoint()?;
        Ok(stored)
    }

    /// Authenticate one immutable object's payload and ordered child edges
    /// using bounded scratch, without retaining its payload or visiting children.
    pub fn verify_object(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.verified_registration(reference, budget).map(|_| ())
    }

    fn verified_registration(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        let registration = read_io::read_registration(
            &self.objects,
            &self.namespace,
            self.io.as_ref(),
            reference,
            budget,
        )?;
        let _binding = self.namespace_binding()?;
        budget.checkpoint()?;
        Ok(registration)
    }

    /// Authenticate every reachable immutable object, including leaf bytes.
    ///
    /// The caller bounds the pending DFS frontier independently of the original
    /// work budget, which bounds all visits/bytes and preserves cancellation.
    /// A funded visit table authenticates shared children once across all roots.
    /// This proves physical wire closure only, not domain/config semantics or
    /// Catalog selection. The caller must retain root custody through use.
    pub fn verify_closure(
        &self,
        roots: &[ObjectRef],
        max_pending: u32,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.verify_closure_inner(roots, None, max_pending, budget)
            .map(|_| ())
    }

    /// Authenticate the complete closure and retain two protected root bodies
    /// from that same traversal for the caller's domain validation. The two
    /// roots may alias; `StoredObject` clones share their charged allocation.
    pub fn verify_closure_with_selected_roots(
        &self,
        roots: &[ObjectRef],
        selected: [ObjectRef; 2],
        max_pending: u32,
        budget: &mut WorkBudget,
    ) -> Result<[StoredObject; 2], IndexError> {
        budget.checkpoint()?;
        if selected.iter().any(|reference| !roots.contains(reference)) {
            return Err(IndexError::CorruptObject(
                "selected object absent from protected roots".into(),
            ));
        }
        self.verify_closure_inner(roots, Some(selected), max_pending, budget)?
            .ok_or_else(|| IndexError::CorruptObject("selected closure roots missing".into()))
    }

    fn verify_closure_inner(
        &self,
        roots: &[ObjectRef],
        selected: Option<[ObjectRef; 2]>,
        max_pending: u32,
        budget: &mut WorkBudget,
    ) -> Result<Option<[StoredObject; 2]>, IndexError> {
        budget.checkpoint()?;
        let maximum = usize::try_from(max_pending).map_err(|_error| IndexError::ResourceLimited)?;
        if maximum == 0 || roots.len() > maximum {
            return Err(IndexError::ResourceLimited);
        }
        let mut frontier = ClosureFrontier::default();
        frontier.reserve(roots.len(), budget)?;
        frontier
            .pending
            .extend(roots.iter().rev().copied().map(ClosureStep::Read));
        let mut outstanding = roots.len();
        let mut visits = ClosureVisits::default();
        let mut selected_objects = [None, None];
        while let Some(step) = frontier.pending.pop() {
            budget.checkpoint()?;
            let reference = match step {
                ClosureStep::Read(reference) => reference,
                ClosureStep::Finish(reference) => {
                    visits.finish(reference, budget)?;
                    continue;
                }
            };
            outstanding = outstanding
                .checked_sub(1)
                .ok_or(IndexError::DigestConflict)?;
            if !visits.begin(reference, budget)? {
                continue;
            }
            let retain = selected.filter(|targets| {
                targets
                    .iter()
                    .zip(&selected_objects)
                    .any(|(target, slot)| *target == reference && slot.is_none())
            });
            let registration = if let Some(targets) = retain {
                let stored = self.get(reference, budget)?;
                for (target, slot) in targets.into_iter().zip(&mut selected_objects) {
                    if reference == target && slot.is_none() {
                        *slot = Some(stored.clone());
                    }
                }
                stored.registration.clone()
            } else {
                self.verified_registration(reference, budget)?
            };
            outstanding = outstanding
                .checked_add(registration.children.len())
                .ok_or(IndexError::ResourceLimited)?;
            if outstanding > maximum {
                return Err(IndexError::ResourceLimited);
            }
            let needed = frontier
                .pending
                .len()
                .checked_add(registration.children.len())
                .and_then(|count| count.checked_add(1))
                .ok_or(IndexError::ResourceLimited)?;
            frontier.reserve(needed, budget)?;
            frontier.pending.push(ClosureStep::Finish(reference));
            frontier.pending.extend(
                registration
                    .children
                    .iter()
                    .rev()
                    .copied()
                    .map(ClosureStep::Read),
            );
        }
        let _binding = self.namespace_binding()?;
        budget.checkpoint()?;
        match selected {
            Some(_) => Ok(Some([
                selected_objects[0].take().ok_or_else(|| {
                    IndexError::CorruptObject("selected closure request missing".into())
                })?,
                selected_objects[1].take().ok_or_else(|| {
                    IndexError::CorruptObject("selected closure profile missing".into())
                })?,
            ])),
            None => Ok(None),
        }
    }
}

#[derive(Default)]
struct ClosureFrontier {
    pending: Vec<ClosureStep>,
    // The pending allocation drops before its lease.
    _memory: Option<MemoryLease>,
}

impl ClosureFrontier {
    fn reserve(&mut self, needed: usize, budget: &WorkBudget) -> Result<(), IndexError> {
        if needed <= self.pending.capacity() {
            return Ok(());
        }
        let capacity = self.pending.capacity().saturating_mul(2).max(needed);
        let bytes = capacity
            .checked_mul(std::mem::size_of::<ClosureStep>())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if pending.capacity() > capacity {
            return Err(IndexError::ResourceLimited);
        }
        pending.extend(self.pending.iter().copied());
        self.pending = pending;
        self._memory = memory;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum ClosureStep {
    Read(ObjectRef),
    Finish(ObjectRef),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ClosureVisitState {
    Visiting,
    Verified,
}

#[derive(Clone, Copy)]
struct ClosureVisit {
    reference: ObjectRef,
    state: ClosureVisitState,
}

#[derive(Default)]
struct ClosureVisits {
    slots: Vec<Option<ClosureVisit>>,
    count: usize,
    hash: RandomState,
    _memory: Option<MemoryLease>,
}

impl ClosureVisits {
    fn probe(
        slots: &[Option<ClosureVisit>],
        hash: &RandomState,
        reference: ObjectRef,
        budget: &WorkBudget,
    ) -> Result<usize, IndexError> {
        let mask = slots
            .len()
            .checked_sub(1)
            .ok_or(IndexError::DigestConflict)?;
        let mask_bits = u64::try_from(mask).map_err(|_| IndexError::ResourceLimited)?;
        let mut index = usize::try_from(hash.hash_one(reference) & mask_bits)
            .map_err(|_| IndexError::ResourceLimited)?;
        for _ in 0..slots.len() {
            budget.checkpoint()?;
            let slot = slots.get(index).ok_or(IndexError::DigestConflict)?;
            if slot.is_none_or(|visit| visit.reference == reference) {
                return Ok(index);
            }
            index = index.wrapping_add(1) & mask;
        }
        Err(IndexError::DigestConflict)
    }

    fn grow(&mut self, budget: &WorkBudget) -> Result<(), IndexError> {
        let capacity = self
            .slots
            .len()
            .max(1)
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = capacity
            .checked_mul(std::mem::size_of::<Option<ClosureVisit>>())
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if slots.capacity() > capacity {
            return Err(IndexError::ResourceLimited);
        }
        slots.resize(capacity, None);
        for visit in self.slots.iter().flatten().copied() {
            let index = Self::probe(&slots, &self.hash, visit.reference, budget)?;
            *slots.get_mut(index).ok_or(IndexError::DigestConflict)? = Some(visit);
        }
        self.slots = slots;
        self._memory = memory;
        Ok(())
    }

    fn begin(&mut self, reference: ObjectRef, budget: &WorkBudget) -> Result<bool, IndexError> {
        if !self.slots.is_empty() {
            let index = Self::probe(&self.slots, &self.hash, reference, budget)?;
            if let Some(visit) = self.slots.get(index).and_then(Option::as_ref) {
                return match visit.state {
                    ClosureVisitState::Visiting => Err(IndexError::DigestConflict),
                    ClosureVisitState::Verified => Ok(false),
                };
            }
        }
        if self.count == self.slots.len() >> 1 {
            self.grow(budget)?;
        }
        let index = Self::probe(&self.slots, &self.hash, reference, budget)?;
        *self
            .slots
            .get_mut(index)
            .ok_or(IndexError::DigestConflict)? = Some(ClosureVisit {
            reference,
            state: ClosureVisitState::Visiting,
        });
        self.count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(true)
    }

    fn finish(&mut self, reference: ObjectRef, budget: &WorkBudget) -> Result<(), IndexError> {
        let index = Self::probe(&self.slots, &self.hash, reference, budget)?;
        let visit = self
            .slots
            .get_mut(index)
            .and_then(Option::as_mut)
            .filter(|visit| visit.state == ClosureVisitState::Visiting)
            .ok_or(IndexError::DigestConflict)?;
        visit.state = ClosureVisitState::Verified;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
