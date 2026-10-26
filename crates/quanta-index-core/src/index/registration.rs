//! Immutable registration metadata retains its own allocation custody.

use std::{ops::Deref, sync::Arc};

use super::{IndexError, MemoryLease, MemoryLifetime, ObjectRef, WorkBudget};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredObject {
    inner: Arc<RegisteredObjectData>,
}

#[derive(Debug)]
pub struct RegisteredObjectData {
    pub reference: ObjectRef,
    pub bytes: u64,
    pub children: Vec<ObjectRef>,
    memory: Option<MemoryLease>,
}

impl RegisteredObject {
    /// Explicit caller-owned input or wire decoding without a work scope.
    #[must_use]
    pub fn unscoped(reference: ObjectRef, bytes: u64, children: Vec<ObjectRef>) -> Self {
        Self {
            inner: Arc::new(RegisteredObjectData {
                reference,
                bytes,
                children,
                memory: None,
            }),
        }
    }

    pub fn allocation_bytes(children: usize) -> Result<u64, IndexError> {
        let bytes = children
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<RegisteredObjectData>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[usize; 2]>()))
            .ok_or(IndexError::ResourceLimited)?;
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
    }

    /// The producer admits backing before allocating; this transfers its custody.
    pub fn from_parts(
        reference: ObjectRef,
        bytes: u64,
        children: Vec<ObjectRef>,
        memory: Option<MemoryLease>,
    ) -> Result<Self, IndexError> {
        let value = RegisteredObjectData {
            reference,
            bytes,
            children,
            memory,
        };
        let required = Self::allocation_bytes(value.children.len())?;
        if value
            .memory
            .as_ref()
            .is_some_and(|lease| lease.bytes() < required)
        {
            return Err(IndexError::BackendViolation(
                "registration backing exceeds allocation custody".into(),
            ));
        }
        Ok(Self {
            inner: Arc::new(value),
        })
    }

    pub fn copy_children(
        reference: ObjectRef,
        bytes: u64,
        children: &[ObjectRef],
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        Self::copy_from_iter(
            reference,
            bytes,
            children.len(),
            children.iter().copied().map(Ok),
            budget,
        )
    }

    pub fn copy_from_iter(
        reference: ObjectRef,
        bytes: u64,
        count: usize,
        items: impl Iterator<Item = Result<ObjectRef, IndexError>>,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        let memory =
            budget.reserve_memory(Self::allocation_bytes(count)?, MemoryLifetime::Retained)?;
        let mut children = Vec::new();
        children
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        for item in items {
            budget.checkpoint()?;
            if children.len() == count {
                return Err(IndexError::BackendViolation(
                    "registration child iterator exceeds admitted count".into(),
                ));
            }
            children.push(item?);
        }
        if children.len() != count {
            return Err(IndexError::BackendViolation(
                "registration child iterator lost a child".into(),
            ));
        }
        budget.checkpoint()?;
        Ok(Self {
            inner: Arc::new(RegisteredObjectData {
                reference,
                bytes,
                children,
                memory,
            }),
        })
    }

    #[must_use]
    pub fn memory_bytes(&self) -> Option<u64> {
        self.inner.memory.as_ref().map(MemoryLease::bytes)
    }
}

impl Deref for RegisteredObject {
    type Target = RegisteredObjectData;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl PartialEq for RegisteredObjectData {
    fn eq(&self, other: &Self) -> bool {
        self.reference == other.reference
            && self.bytes == other.bytes
            && self.children == other.children
    }
}
impl Eq for RegisteredObjectData {}

/// Bounded returned registration slots retain memory through iterator drop.
#[derive(Debug)]
pub struct RegisteredObjectBatch {
    objects: Vec<RegisteredObject>,
    maximum: usize,
    memory: Option<MemoryLease>,
}

impl RegisteredObjectBatch {
    pub fn allocation_bytes(count: usize) -> Result<u64, IndexError> {
        let bytes = count
            .checked_mul(std::mem::size_of::<RegisteredObject>())
            .ok_or(IndexError::ResourceLimited)?;
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
    }

    pub fn with_capacity(count: usize, budget: &WorkBudget) -> Result<Self, IndexError> {
        let bytes = Self::allocation_bytes(count)?;
        let memory = if count == 0 {
            budget.checkpoint()?;
            None
        } else {
            budget.reserve_memory(bytes, MemoryLifetime::Retained)?
        };
        let mut objects = Vec::new();
        objects
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            objects,
            maximum: count,
            memory,
        })
    }

    #[must_use]
    pub fn unscoped(objects: Vec<RegisteredObject>) -> Self {
        Self {
            maximum: objects.len(),
            objects,
            memory: None,
        }
    }

    pub fn push(&mut self, object: RegisteredObject) -> Result<(), IndexError> {
        if self.objects.len() == self.maximum {
            return Err(IndexError::BackendViolation(
                "registration batch exceeds admitted slots".into(),
            ));
        }
        self.objects.push(object);
        Ok(())
    }

    #[must_use]
    pub fn as_slice(&self) -> &[RegisteredObject] {
        &self.objects
    }
}

impl Deref for RegisteredObjectBatch {
    type Target = [RegisteredObject];
    fn deref(&self) -> &Self::Target {
        &self.objects
    }
}
impl PartialEq for RegisteredObjectBatch {
    fn eq(&self, other: &Self) -> bool {
        self.objects == other.objects
    }
}
impl Eq for RegisteredObjectBatch {}

#[derive(Debug)]
pub struct RegisteredObjectIter {
    objects: std::vec::IntoIter<RegisteredObject>,
    _memory: Option<MemoryLease>,
}
impl Iterator for RegisteredObjectIter {
    type Item = RegisteredObject;
    fn next(&mut self) -> Option<Self::Item> {
        self.objects.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.objects.size_hint()
    }
}
impl ExactSizeIterator for RegisteredObjectIter {}
impl IntoIterator for RegisteredObjectBatch {
    type Item = RegisteredObject;
    type IntoIter = RegisteredObjectIter;
    fn into_iter(self) -> Self::IntoIter {
        RegisteredObjectIter {
            objects: self.objects.into_iter(),
            _memory: self.memory,
        }
    }
}
impl<'a> IntoIterator for &'a RegisteredObjectBatch {
    type Item = &'a RegisteredObject;
    type IntoIter = std::slice::Iter<'a, RegisteredObject>;
    fn into_iter(self) -> Self::IntoIter {
        self.objects.iter()
    }
}

// The wire-only decode bridge preserves the existing closed field contract.
// It does not provide native allocation admission; its decoder owns that scope.
struct RegistrationWire {
    reference: ObjectRef,
    bytes: u64,
    children: Vec<ObjectRef>,
}
quanta_index_contract::native_serde_struct!(RegistrationWire { reference: ObjectRef, bytes: u64, children: Vec<ObjectRef> });

impl serde::Serialize for RegisteredObject {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;
        let mut state = serializer.serialize_struct("RegisteredObject", 3)?;
        state.serialize_field("reference", &self.reference)?;
        state.serialize_field("bytes", &self.bytes)?;
        state.serialize_field("children", &self.children)?;
        state.end()
    }
}
impl<'de> serde::Deserialize<'de> for RegisteredObject {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = <RegistrationWire as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Self::unscoped(wire.reference, wire.bytes, wire.children))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{Digest, MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits};
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    struct Ledger {
        live: AtomicU64,
        refunds: AtomicU64,
        ceiling: u64,
    }
    impl MemoryLeaseOwner for Ledger {
        fn release(&self, bytes: u64) {
            let _prior = self.live.fetch_sub(bytes, Ordering::SeqCst);
            let _prior = self.refunds.fetch_add(1, Ordering::SeqCst);
        }
    }
    struct Scope(Arc<Ledger>);
    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime != MemoryLifetime::Retained {
                return Err(IndexError::BackendViolation(
                    "registration escaped with temporary custody".into(),
                ));
            }
            let next = self
                .0
                .live
                .load(Ordering::SeqCst)
                .checked_add(bytes)
                .ok_or(IndexError::ResourceLimited)?;
            if next > self.0.ceiling {
                return Err(IndexError::ResourceLimited);
            }
            self.0.live.store(next, Ordering::SeqCst);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }
    fn budget(ceiling: u64) -> Result<(WorkBudget, Arc<Ledger>), IndexError> {
        let ledger = Arc::new(Ledger {
            live: AtomicU64::new(0),
            refunds: AtomicU64::new(0),
            ceiling,
        });
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 10,
                writes: 10,
                bytes: 1_000_000,
                mutations: 10,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
        Ok((budget, ledger))
    }
    fn require(value: bool) -> Result<(), IndexError> {
        if value {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "registration custody oracle mismatch".into(),
            ))
        }
    }

    #[test]
    fn shared_registration_and_consuming_batch_keep_distinct_charges_until_final_drop()
    -> Result<(), IndexError> {
        let children = [
            ObjectRef(Digest([3; 32])),
            ObjectRef(Digest([4; 32])),
            ObjectRef(Digest([3; 32])),
        ];
        let metadata = RegisteredObject::allocation_bytes(3)?;
        let slots = RegisteredObjectBatch::allocation_bytes(2)?;
        let (work, ledger) = budget(
            metadata
                .checked_add(slots)
                .ok_or(IndexError::ResourceLimited)?,
        )?;
        let object =
            RegisteredObject::copy_children(ObjectRef(Digest([9; 32])), 128, &children, &work)?;
        let shared = object.clone();
        require(std::ptr::eq(
            object.children.as_ptr(),
            shared.children.as_ptr(),
        ))?;
        require(
            object.children == children
                && object.bytes == 128
                && object.reference == ObjectRef(Digest([9; 32])),
        )?;
        let mut batch = RegisteredObjectBatch::with_capacity(2, &work)?;
        batch.push(object)?;
        batch.push(shared.clone())?;
        require(batch.push(shared.clone()).is_err())?;
        drop(shared);
        require(ledger.live.load(Ordering::SeqCst) == metadata + slots)?;
        let mut items = batch.into_iter();
        let first = items
            .next()
            .ok_or_else(|| IndexError::BackendViolation("registration ordinal missing".into()))?;
        require(items.len() == 1)?;
        require(ledger.live.load(Ordering::SeqCst) == metadata + slots)?;
        drop(items);
        require(ledger.live.load(Ordering::SeqCst) == metadata)?;
        require(first.children == children)?;
        drop(first);
        require(
            ledger.live.load(Ordering::SeqCst) == 0 && ledger.refunds.load(Ordering::SeqCst) == 2,
        )
    }

    #[test]
    fn registration_floor_denial_precedes_child_iteration_and_empty_batch_birth()
    -> Result<(), IndexError> {
        let (work, ledger) = budget(0)?;
        let visited = AtomicU64::new(0);
        let children = std::iter::once_with(|| {
            let _prior = visited.fetch_add(1, Ordering::SeqCst);
            Ok(ObjectRef(Digest([3; 32])))
        });
        require(matches!(
            RegisteredObject::copy_from_iter(ObjectRef(Digest([9; 32])), 128, 1, children, &work),
            Err(IndexError::ResourceLimited)
        ))?;
        require(visited.load(Ordering::SeqCst) == 0)?;
        require(RegisteredObjectBatch::with_capacity(0, &work)?.is_empty())?;
        require(matches!(
            RegisteredObjectBatch::with_capacity(1, &work),
            Err(IndexError::ResourceLimited)
        ))?;
        require(
            ledger.live.load(Ordering::SeqCst) == 0 && ledger.refunds.load(Ordering::SeqCst) == 0,
        )
    }

    #[test]
    fn registration_wire_keeps_fixed_json_fields_and_closed_decode()
    -> Result<(), Box<dyn std::error::Error>> {
        let object = RegisteredObject::unscoped(ObjectRef(Digest([0; 32])), 1, Vec::new());
        let key = "[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]";
        let gold = format!("{{\"reference\":{key},\"bytes\":1,\"children\":[]}}");
        require(serde_json::to_string(&object)? == gold)?;
        require(serde_json::from_str::<RegisteredObject>(&gold)? == object)?;
        for invalid in [
            format!("{{\"reference\":{key},\"bytes\":1}}"),
            format!("{{\"reference\":{key},\"bytes\":1,\"bytes\":1,\"children\":[]}}"),
            format!("{{\"reference\":{key},\"bytes\":1,\"children\":[],\"memory\":null}}"),
            format!("[{key},1,[],0]"),
        ] {
            require(serde_json::from_str::<RegisteredObject>(&invalid).is_err())?;
        }
        Ok(())
    }
}
