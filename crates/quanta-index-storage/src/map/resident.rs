//! Bounded, request-owned staging for one short scalar map edit.
//!
//! `update_one` walks at most one node per key nibble. Each unwound frame
//! writes at most one node; a split writes at most three more, and root
//! materialization at most one. Unary compression rewrites its result, not an
//! additional level. Thus a 64-byte key can stage no more than 2*64+4 nodes.
//! Existing nodes may still have the full physical prefix/inline payload, so
//! wire capacity is bounded by the physical node limit, never by key length.

use std::cell::RefCell;

use quanta_index_core::index::{IndexError, ObjectRef, WorkBudget, WorkUsage};

use super::{MAX_NODE_CHILDREN, MAX_NODE_PAYLOAD, MapBuffer, MapStage, Node, decode_node};
use crate::{
    FileObjectStore,
    object::{PreparedObject, decode_stored_wire},
};

pub(super) const MAX_SINGLE_KEY_BYTES: usize =
    quanta_index_core::index::ports::MAX_OBJECT_MAP_BATCH_KEY_BYTES;
const MAX_RESIDENT_NODES: usize = MAX_SINGLE_KEY_BYTES * 2 + 4;
const MAX_NODE_WIRE_BYTES: usize = 16 + MAX_NODE_CHILDREN * 32 + MAX_NODE_PAYLOAD;
const MAX_RESIDENT_WIRE_BYTES: usize = MAX_RESIDENT_NODES * MAX_NODE_WIRE_BYTES;

struct Entry {
    reference: ObjectRef,
    // Transfer the original admitted wire once construction has finished.
    // Identity and traversal state remain until the installation walk ends.
    object: Option<PreparedObject>,
    mark: u8,
}

struct State {
    entries: MapBuffer<Entry>,
    wire_bytes: usize,
}

pub(super) struct ResidentStage {
    state: RefCell<State>,
    node_limit: usize,
    wire_limit: usize,
    object_wire_allowance_added: bool,
}

impl ResidentStage {
    pub(super) fn new() -> Self {
        Self::with_bounds(MAX_RESIDENT_NODES, MAX_RESIDENT_WIRE_BYTES)
    }

    /// Two original short-key paths contribute at most twice the single-root
    /// bound. One shared stage still deduplicates their reachable DAG/installs.
    pub(super) fn pair() -> Self {
        Self::with_bounds(2 * MAX_RESIDENT_NODES, 2 * MAX_RESIDENT_WIRE_BYTES)
    }

    pub(super) fn with_objects(count: usize, bytes: usize) -> Result<Self, IndexError> {
        let nodes = MAX_RESIDENT_NODES
            .checked_mul(2)
            .and_then(|nodes| nodes.checked_add(count))
            .ok_or(IndexError::ResourceLimited)?;
        let wire = MAX_RESIDENT_WIRE_BYTES
            .checked_mul(2)
            .and_then(|wire| wire.checked_add(bytes))
            .ok_or(IndexError::ResourceLimited)?;
        Ok(Self::with_bounds(nodes, wire))
    }

    /// Four independent short-key roots and four opaque DATA children.
    pub(super) fn owner() -> Result<Self, IndexError> {
        let nodes = MAX_RESIDENT_NODES
            .checked_mul(4)
            .and_then(|n| n.checked_add(4))
            .ok_or(IndexError::ResourceLimited)?;
        let object_bytes = usize::try_from(crate::object::MAX_BATCH_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let wire = MAX_RESIDENT_WIRE_BYTES
            .checked_mul(4)
            .and_then(|bytes| bytes.checked_add(object_bytes))
            .ok_or(IndexError::ResourceLimited)?;
        Ok(Self::with_bounds(nodes, wire))
    }

    pub(super) fn read_stored(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<quanta_index_core::index::StoredObject>, IndexError> {
        let state = self.state.try_borrow().map_err(|_error| {
            IndexError::BackendViolation("resident map staging was reentered".into())
        })?;
        let Some(entry) = state
            .entries
            .iter()
            .find(|entry| entry.reference == reference)
        else {
            return Ok(None);
        };
        let object = entry.object.as_ref().ok_or_else(|| {
            IndexError::BackendViolation("resident object read after installation started".into())
        })?;
        budget.charge(WorkUsage {
            reads: 1,
            bytes: object.registration.bytes,
            ..WorkUsage::default()
        })?;
        decode_stored_wire(reference, &object.wire, budget).map(Some)
    }

    /// The two short original-base paths were bounded at preparation. Add
    /// only the exact already-prepared DATA wire allowance before insertion.
    pub(super) fn admit_object_wires(&mut self, bytes: usize) -> Result<(), IndexError> {
        if self.object_wire_allowance_added {
            return Err(IndexError::BackendViolation(
                "resident DATA allowance was already added".into(),
            ));
        }
        self.wire_limit = self
            .wire_limit
            .checked_add(bytes)
            .ok_or(IndexError::ResourceLimited)?;
        self.object_wire_allowance_added = true;
        Ok(())
    }

    /// Added after all COW reads. Installation treats the wire as opaque DATA.
    pub(super) fn insert_object(
        &self,
        object: PreparedObject,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        let maximum = object.wire.len();
        self.insert_prepared(object, maximum, budget)
    }

    fn with_bounds(node_limit: usize, wire_limit: usize) -> Self {
        Self {
            state: RefCell::new(State {
                entries: MapBuffer::new(node_limit),
                wire_bytes: 0,
            }),
            node_limit,
            wire_limit,
            object_wire_allowance_added: false,
        }
    }
    fn insert_prepared(
        &self,
        object: PreparedObject,
        maximum_wire: usize,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        let reference = object.registration.reference;
        budget.charge(WorkUsage {
            writes: 1,
            bytes: object.registration.bytes,
            ..WorkUsage::default()
        })?;
        let mut state = self.state.try_borrow_mut().map_err(|_error| {
            IndexError::BackendViolation("resident map staging was reentered".into())
        })?;
        if let Some(existing) = state
            .entries
            .iter()
            .find(|entry| entry.reference == reference)
        {
            let existing = existing.object.as_ref().ok_or_else(|| {
                IndexError::BackendViolation("resident insertion after installation started".into())
            })?;
            if existing.wire != object.wire {
                return Err(IndexError::CorruptObject(
                    "staged object digest collision".into(),
                ));
            }
            return Ok(reference);
        }
        let next_bytes = state
            .wire_bytes
            .checked_add(object.wire.len())
            .ok_or(IndexError::ResourceLimited)?;
        // These limits are implied by the scalar radix algorithm and physical
        // codec. Reaching one is an implementation error, not a new admission
        // refusal for an otherwise valid short-key update.
        if state.entries.len() >= self.node_limit
            || object.wire.len() > maximum_wire
            || next_bytes > self.wire_limit
        {
            return Err(IndexError::BackendViolation(
                "resident map staging exceeded its proved bound".into(),
            ));
        }
        state.entries.push(
            Entry {
                reference,
                object: Some(object),
                mark: 0,
            },
            budget,
        )?;
        state.wire_bytes = next_bytes;
        Ok(reference)
    }
}

impl MapStage for ResidentStage {
    fn read_node(
        &self,
        store: &FileObjectStore,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Node, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let state = self.state.try_borrow().map_err(|_error| {
            IndexError::BackendViolation("resident map staging was reentered".into())
        })?;
        if let Some(entry) = state
            .entries
            .iter()
            .find(|entry| entry.reference == reference)
        {
            let object = entry.object.as_ref().ok_or_else(|| {
                IndexError::BackendViolation("resident node read after installation started".into())
            })?;
            budget.charge(WorkUsage {
                reads: 1,
                bytes: object.registration.bytes,
                ..WorkUsage::default()
            })?;
            let stored = decode_stored_wire(reference, &object.wire, budget)?;
            return decode_node(&stored, budget);
        }
        drop(state);
        store.read_node(reference, budget)
    }

    fn insert(
        &self,
        object: PreparedObject,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        self.insert_prepared(object, MAX_NODE_WIRE_BYTES, budget)
    }

    fn take_prepared(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<PreparedObject>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let mut state = self.state.try_borrow_mut().map_err(|_error| {
            IndexError::BackendViolation("resident map staging was reentered".into())
        })?;
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.reference == reference)
        else {
            return Ok(None);
        };
        match entry.mark {
            0 => entry.object.take().map(Some).ok_or_else(|| {
                IndexError::BackendViolation(
                    "resident node was taken without an active mark".into(),
                )
            }),
            1 => Err(IndexError::CorruptObject("cyclic staged map DAG".into())),
            2 => Ok(None),
            _ => Err(IndexError::CorruptObject("invalid staged map mark".into())),
        }
    }

    fn mark(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<u8>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let state = self.state.try_borrow().map_err(|_error| {
            IndexError::BackendViolation("resident map staging was reentered".into())
        })?;
        Ok(state
            .entries
            .iter()
            .find(|entry| entry.reference == reference)
            .and_then(|entry| (entry.mark != 0).then_some(entry.mark)))
    }

    fn set_mark(
        &self,
        reference: ObjectRef,
        mark: u8,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            ..WorkUsage::default()
        })?;
        let mut state = self.state.try_borrow_mut().map_err(|_error| {
            IndexError::BackendViolation("resident map staging was reentered".into())
        })?;
        let entry = state
            .entries
            .iter_mut()
            .find(|entry| entry.reference == reference)
            .ok_or_else(|| {
                IndexError::BackendViolation("resident map mark has no staged node".into())
            })?;
        entry.mark = mark;
        Ok(())
    }

    fn begin_batch(&self, budget: &WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()
    }

    fn commit_batch(&self, budget: &WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()
    }
}
