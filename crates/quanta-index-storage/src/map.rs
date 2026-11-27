//! A deterministic compressed radix tree. A mutation reads and rewrites only
//! the changed key path; retained roots keep their immutable subtrees.

mod branches;
mod buffer;
mod cold_stream;
mod completed_group;
mod diff;
mod node_wire;
mod object_batch;
mod original_lookup;
mod owner_scope;
mod resident;
mod retained_value;
mod scalar_pair;
mod scoped_reads;
mod scratch;
mod staged_children;
mod value_codec;
use branches::Branches;
use buffer::MapBuffer;
use completed_group::CompletedGroup;
use quanta_index_core::{
    DiagnosticContextV1, DiagnosticCounterV1, DiagnosticHealthCodeV1, DiagnosticLaneV1,
    DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1, DiagnosticTraceErrorV1,
};
use resident::ResidentStage;
use retained_value::RetainedMapValue;
use scratch::MapScratch;
use sha2::{Digest as _, Sha256};
use staged_children::StagedChildren;
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

use quanta_index_core::index::{
    IndexError, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapRoot, MapValue, MapWrite,
    MemoryCreditLimits, MemoryLease, MemoryLifetime, ObjectRef, OrdinalMapEdit, StagedMapRoots,
    StoredObject, WorkBudget, WorkUsage,
    ports::{
        ImmutableObjectPort, MapEditCursor, ObjectWriteSink, PersistentMapPort, RootPackingMode,
        RootPackingPlan,
    },
};

use crate::{
    FileObjectStore,
    object::{
        MAX_BATCH_BYTES, MAX_INSTALL_BATCH_INPUTS, MAX_PREPARED_INSTALL_GROUPS, PreparedObject,
    },
};

#[cfg(test)]
const NODE_MAGIC: &[u8; 8] = b"QIMAP004";
const MAX_KEY_BYTES: usize = 4096;
const MAX_NIBBLES: usize = MAX_KEY_BYTES * 2;
const MAX_INLINE_NIBBLES: usize = 256;
const NODE_HEADER_BYTES: usize = 15;
const MAX_NODE_PAYLOAD: usize = NODE_HEADER_BYTES
    + MAX_NIBBLES
    + 16 * (2 + MAX_INLINE_NIBBLES + value_codec::MAX_VALUE_PAYLOAD)
    + value_codec::MAX_VALUE_PAYLOAD;
const MAX_SCAN_LIMIT: u32 = 4096;
const MAX_BATCH_ROOTS: usize = 64;
const MAX_EMPTY_STREAM_ROOTS: usize = MAX_INSTALL_BATCH_INPUTS;
// Each of sixteen terminal slots and the own value carries at most five
// explicit children. Indirect branches carry one. Bound the deepest admitted
// node's complete physical children before depth validation on pop.
const MAX_NODE_CHILDREN: usize = 17 * quanta_index_core::index::MAX_MAP_INLINE_CHILDREN;
const MAX_INSTALL_PENDING: usize = 1 + MAX_NODE_CHILDREN * (MAX_NIBBLES + 1);

/// Credit for one DFS over a same-attempt canonical radix map. A canonical
/// tree with E values has fewer than 2E nodes (including packed nodes); the
/// caller must prove its value count before selecting this optimized path.
fn cold_scan_memory_credit_limits(
    store: &FileObjectStore,
    max_entries: usize,
) -> Result<Option<MemoryCreditLimits>, IndexError> {
    if max_entries == 0 {
        return Ok(None);
    }
    let nodes = max_entries
        .checked_mul(2)
        .ok_or(IndexError::ResourceLimited)?;
    let Some(read) =
        store.canonical_map_read_memory_credit_limits(MAX_NODE_PAYLOAD, MAX_NODE_CHILDREN)?
    else {
        return Ok(None);
    };
    let per_node = MAX_NIBBLES
        .checked_add(16 * MAX_INLINE_NIBBLES)
        .and_then(|bytes| {
            16_usize
                .checked_mul(std::mem::size_of::<Branch>())
                .and_then(|backing| bytes.checked_add(backing))
        })
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Node>()))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PackedBranch>()))
        .and_then(|bytes| {
            17_usize
                .checked_mul(std::mem::size_of::<retained_value::ManyChildren>())
                .and_then(|backing| bytes.checked_add(backing))
        })
        .and_then(|bytes| {
            17_usize
                .checked_mul(std::mem::size_of::<[usize; 2]>())
                .and_then(|backing| bytes.checked_add(backing))
        })
        .ok_or(IndexError::ResourceLimited)?;
    // MapBuffer grows with old and new backings live. The stack's geometric
    // next capacity can be twice its live depth, then coexist with the old.
    let stack_peak = nodes
        .checked_mul(4)
        .and_then(|slots| slots.checked_mul(std::mem::size_of::<ScanFrame>()))
        .ok_or(IndexError::ResourceLimited)?;
    // A shared packed branch may copy one selected node while its original
    // Arc remains on the traversal stack.
    let temporary = nodes
        .checked_add(1)
        .and_then(|count| count.checked_mul(per_node))
        .and_then(|bytes| bytes.checked_add(stack_peak))
        .and_then(|bytes| bytes.checked_add(2 * MAX_NIBBLES))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ScanSuspend>()))
        .ok_or(IndexError::ResourceLimited)?;
    let retained = MapPage::allocation_bytes(1)?
        .checked_add(u64::try_from(MAX_KEY_BYTES).map_err(|_error| IndexError::ResourceLimited)?)
        .ok_or(IndexError::ResourceLimited)?;
    // Per decoded node: prefix, branch vector, sixteen terminal tails,
    // seventeen inline values and sixteen packed-branch Arc grants.
    let temporary_leases = nodes
        .checked_add(1)
        .and_then(|count| count.checked_mul(51))
        .and_then(|slots| slots.checked_add(5))
        .ok_or(IndexError::ResourceLimited)?;
    Ok(Some(MemoryCreditLimits {
        temporary_data_bytes: u64::try_from(temporary)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(read.temporary_data_bytes)
            .ok_or(IndexError::ResourceLimited)?,
        retained_data_bytes: retained
            .checked_add(read.retained_data_bytes)
            .ok_or(IndexError::ResourceLimited)?,
        max_live_temporary_leases: u32::try_from(temporary_leases)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(read.max_live_temporary_leases)
            .ok_or(IndexError::ResourceLimited)?,
        max_live_retained_leases: 2_u32
            .checked_add(read.max_live_retained_leases)
            .ok_or(IndexError::ResourceLimited)?,
    }))
}

#[repr(C)]
struct InstallFrame {
    reference: ObjectRef,
    tagged_depth: u16,
}

impl InstallFrame {
    const EXIT: u16 = 1 << 15;

    fn enter(reference: ObjectRef, depth: usize) -> Result<Self, IndexError> {
        let tagged_depth = u16::try_from(depth).map_err(|_depth| IndexError::ResourceLimited)?;
        if tagged_depth & Self::EXIT != 0 {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            reference,
            tagged_depth,
        })
    }

    fn exit(mut self) -> Self {
        self.tagged_depth |= Self::EXIT;
        self
    }

    fn exiting(&self) -> bool {
        self.tagged_depth & Self::EXIT != 0
    }

    fn depth(&self) -> usize {
        usize::from(self.tagged_depth & !Self::EXIT)
    }

    fn push(
        self,
        pending: &mut MapBuffer<Self>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            bytes: u64::try_from(std::mem::size_of::<Self>())
                .map_err(|_width| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        pending.push(self, budget)
    }
}

/// Fingerprint the canonical edit, excluding only its root ordinal. Scratch
/// rows are decoded through the same edit cursor before a fingerprint can
/// authorize reuse of a previously built root.
fn fingerprint_position(
    digest: &mut Sha256,
    edit: &OrdinalMapEdit,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    fingerprint_wires(
        digest,
        edit.key(),
        edit.expected_wire(),
        edit.next_wire(),
        budget,
    )
}

fn fingerprint_wires(
    digest: &mut Sha256,
    key: &[u8],
    expected_wire: Option<&[u8]>,
    next_wire: Option<&[u8]>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let key_len = u32::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let expected_len = u64::try_from(expected_wire.map_or(0, <[u8]>::len))
        .map_err(|_error| IndexError::ResourceLimited)?;
    let next_len = u64::try_from(next_wire.map_or(0, <[u8]>::len))
        .map_err(|_error| IndexError::ResourceLimited)?;
    let header_bytes = 6u64
        .checked_add(if expected_wire.is_some() { 2 } else { 0 })
        .and_then(|bytes| bytes.checked_add(if next_wire.is_some() { 2 } else { 0 }))
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = header_bytes
        .checked_add(u64::from(key_len))
        .and_then(|bytes| bytes.checked_add(expected_len))
        .and_then(|bytes| bytes.checked_add(next_len))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes,
        ..WorkUsage::default()
    })?;
    digest.update(key_len.to_le_bytes());
    digest.update(key);
    for wire in [expected_wire, next_wire] {
        if let Some(wire) = wire {
            digest.update([1]);
            let length = u16::try_from(wire.len()).map_err(|_error| IndexError::ResourceLimited)?;
            digest.update(length.to_le_bytes());
            digest.update(wire);
        } else {
            digest.update([0]);
        }
    }
    Ok(())
}

fn scratch_edit_fingerprint(
    scratch: &MapScratch,
    root: u32,
    budget: &mut WorkBudget,
) -> Result<([u8; 32], u64), IndexError> {
    let mut digest = Sha256::new();
    let mut edits = scratch.edits(root);
    let mut count = 0u64;
    while let Some(position) = edits.next_edit(budget)? {
        fingerprint_position(&mut digest, &position, budget)?;
        count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    Ok((digest.finalize().into(), count))
}

/// Terminal and packed branches hold DATA whose physical edges belong to the
/// enclosing object. Indirect branches hold a separately installed object.
#[derive(Eq, PartialEq)]
enum BranchValue {
    Indirect(ObjectRef),
    Terminal(RetainedMapValue),
    Packed(Arc<PackedBranch>),
}

struct PackedBranch {
    node: Node,
    shape: node_wire::PackedShape,
    _memory: Option<MemoryLease>,
}

impl PartialEq for PackedBranch {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node
    }
}
impl Eq for PackedBranch {}

struct Branch {
    prefix: MapBuffer<u8>,
    value: BranchValue,
}
impl PartialEq for Branch {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && self.prefix.as_slice() == other.prefix.as_slice()
    }
}
impl Eq for Branch {}
impl Branch {
    fn indirect(reference: ObjectRef) -> Self {
        Self {
            prefix: MapBuffer::new(0),
            value: BranchValue::Indirect(reference),
        }
    }
    fn terminal(prefix: MapBuffer<u8>, value: RetainedMapValue) -> Self {
        Self {
            prefix,
            value: BranchValue::Terminal(value),
        }
    }

    fn packed(node: Node, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let shape = node_wire::packed_shape(&node)?;
        let header = std::alloc::Layout::new::<[std::sync::atomic::AtomicUsize; 2]>();
        let (layout, _) = header
            .extend(std::alloc::Layout::new::<PackedBranch>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let bytes = u64::try_from(layout.pad_to_align().size())
            .map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        Ok(Self {
            prefix: MapBuffer::new(0),
            value: BranchValue::Packed(Arc::new(PackedBranch {
                node,
                shape,
                _memory: memory,
            })),
        })
    }

    fn copy(&self, budget: &WorkBudget) -> Result<Self, IndexError> {
        match &self.value {
            BranchValue::Indirect(reference) => Ok(Self::indirect(*reference)),
            BranchValue::Terminal(value) => Ok(Self::terminal(
                MapBuffer::copy(&self.prefix, MAX_INLINE_NIBBLES, budget)?,
                value.clone(),
            )),
            BranchValue::Packed(packed) => Ok(Self {
                prefix: MapBuffer::new(0),
                value: BranchValue::Packed(Arc::clone(packed)),
            }),
        }
    }

    fn into_node(
        self,
        budget: &mut WorkBudget,
        read: impl FnOnce(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
    ) -> Result<Node, IndexError> {
        match self.value {
            BranchValue::Indirect(reference) => read(reference, budget),
            BranchValue::Terminal(value) => Ok(Node {
                prefix: self.prefix,
                value: Some(value),
                branches: Branches::new(),
            }),
            BranchValue::Packed(packed) => match Arc::try_unwrap(packed) {
                Ok(packed) => Ok(packed.node),
                Err(packed) => packed.node.copy(budget),
            },
        }
    }
}

fn copy_branch(branch: Option<&Branch>, budget: &WorkBudget) -> Result<Option<Branch>, IndexError> {
    branch.map(|branch| branch.copy(budget)).transpose()
}

pub(crate) struct Node {
    /// Nibbles following the parent's branch nibble.
    prefix: MapBuffer<u8>,
    value: Option<RetainedMapValue>,
    branches: Branches,
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.prefix.as_slice() == other.prefix.as_slice()
            && self.value == other.value
            && self.branches.iter().eq(other.branches.iter())
    }
}
impl Eq for Node {}

impl Node {
    fn leaf(
        prefix: &[u8],
        value: RetainedMapValue,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        Ok(Self {
            prefix: MapBuffer::copy(prefix, MAX_NIBBLES, budget)?,
            value: Some(value),
            branches: Branches::new(),
        })
    }

    fn branch_count(&self) -> usize {
        self.branches.len()
    }

    fn sole_branch(&self) -> Option<(usize, &Branch)> {
        if self.branches.len() == 1 {
            self.branches.iter().next()
        } else {
            None
        }
    }

    fn packable(&self, mode: RootPackingMode) -> bool {
        self.prefix.len() <= MAX_INLINE_NIBBLES
            && self.branch_count() != 0
            && self.branches.iter().all(|(_, branch)| match mode {
                RootPackingMode::None => false,
                RootPackingMode::Terminal => matches!(branch.value, BranchValue::Terminal(_)),
                RootPackingMode::Nested => !matches!(branch.value, BranchValue::Indirect(_)),
            })
            && node_wire::packed_shape(self).is_ok()
    }

    fn copy(&self, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let (mask, bytes) = self.branches.iter().try_fold(
            (0u16, self.prefix.len()),
            |(mask, bytes), (edge, branch)| {
                let bytes = bytes
                    .checked_add(std::mem::size_of::<Branch>())
                    .and_then(|bytes| bytes.checked_add(branch.prefix.len()))
                    .ok_or(IndexError::ResourceLimited)?;
                Ok::<_, IndexError>((mask | (1u16 << edge), bytes))
            },
        )?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut branches = Branches::for_decode(mask, budget)?;
        for (edge, branch) in self.branches.iter() {
            branches.append_decoded(edge, branch.copy(budget)?, budget)?;
        }
        Ok(Self {
            prefix: MapBuffer::copy(&self.prefix, MAX_NIBBLES, budget)?,
            value: self.value.clone(),
            branches,
        })
    }
}

/// DFS retains one path and one admitted frame per depth, never sibling path copies.
struct ScanFrame {
    node: ScanNode,
    entered: bool,
    path_end: usize,
    next_edge: usize,
}

enum ScanNode {
    Owned(Node),
    Packed(Arc<PackedBranch>),
}

impl std::ops::Deref for ScanNode {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(node) => node,
            Self::Packed(packed) => &packed.node,
        }
    }
}

impl ScanFrame {
    fn new(node: ScanNode) -> Self {
        Self {
            node,
            entered: false,
            path_end: 0,
            next_edge: 0,
        }
    }
}

struct ScanPageBuilder {
    entries: Vec<(Vec<u8>, MapValue)>,
    // Data fields precede leases on all early-return paths as well as success.
    leases: Vec<quanta_index_core::index::MemoryLease>,
    allocation_memory: Option<quanta_index_core::index::MemoryLease>,
}

impl ScanPageBuilder {
    fn new(capacity: usize, budget: &WorkBudget) -> Result<Self, IndexError> {
        let allocation_memory = budget.reserve_memory(
            MapPage::allocation_bytes(capacity)?,
            MemoryLifetime::Retained,
        )?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut leases = Vec::new();
        leases
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            entries,
            leases,
            allocation_memory,
        })
    }

    fn finish(self, has_more: bool) -> MapPage {
        MapPage::new(self.entries, has_more, self.leases, self.allocation_memory)
    }
}

/// One attempt-local DFS. A full page leaves its next value pending, so the
/// following page does not reread the root or skip the lookahead value.
struct ScanContinuation {
    after_nibbles: Option<MapBuffer<u8>>,
    path: MapBuffer<u8>,
    stack: MapBuffer<ScanFrame>,
    first_page: Option<ScanPageBuilder>,
    limit: usize,
    pending_value: bool,
    finished: bool,
    poisoned: bool,
}

/// The optional box and its admission exist only while a descriptor visitor
/// may need the Original memory held by the DFS for mandatory child work.
struct ScanSuspend {
    scan: ScanContinuation,
    _memory: MemoryLease,
}

impl ScanContinuation {
    fn new(
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
        read_node: &mut impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
    ) -> Result<Self, IndexError> {
        if limit == 0 || limit > MAX_SCAN_LIMIT {
            return Err(IndexError::InvalidInput(
                "map scan limit is out of bounds".into(),
            ));
        }
        budget.checkpoint()?;
        let after_nibbles = if let Some(after) = after {
            charge_key_allocation(after, budget)?;
            Some(key_nibbles(after, budget)?)
        } else {
            None
        };
        let limit = usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?;
        let capacity = if root.0.is_some() { limit } else { 0 };
        let first_page = ScanPageBuilder::new(capacity, budget)?;
        let path = MapBuffer::new(MAX_NIBBLES);
        let mut stack = MapBuffer::new(MAX_NIBBLES + 1);
        if let Some(reference) = root.0 {
            stack.push(
                ScanFrame::new(ScanNode::Owned(read_node(reference, budget)?)),
                budget,
            )?;
        }
        Ok(Self {
            after_nibbles,
            path,
            stack,
            first_page: Some(first_page),
            limit,
            pending_value: false,
            finished: false,
            poisoned: false,
        })
    }

    fn next_page(
        &mut self,
        budget: &mut WorkBudget,
        read_node: &mut impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
    ) -> Result<Option<MapPage>, IndexError> {
        if self.poisoned {
            return Err(IndexError::BackendViolation(
                "failed map scan continuation reused".into(),
            ));
        }
        if self.finished {
            return Ok(None);
        }
        let result = self.next_page_inner(budget, read_node);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn next_page_inner(
        &mut self,
        budget: &mut WorkBudget,
        read_node: &mut impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
    ) -> Result<Option<MapPage>, IndexError> {
        let mut page = if let Some(page) = self.first_page.take() {
            page
        } else {
            budget.checkpoint()?;
            ScanPageBuilder::new(self.limit, budget)?
        };
        while let Some(frame) = self.stack.last_mut() {
            budget.checkpoint()?;
            if self.pending_value {
                self.pending_value = false;
                let value = frame.node.value.as_ref().ok_or_else(|| {
                    IndexError::BackendViolation("pending scan value is missing".into())
                })?;
                scan_emit_value(&self.path, value, &mut page, budget)?;
            } else if !frame.entered {
                let length = self
                    .path
                    .len()
                    .checked_add(frame.node.prefix.len())
                    .filter(|length| *length <= MAX_NIBBLES)
                    .ok_or_else(|| {
                        IndexError::CorruptObject("map path exceeds key limit".into())
                    })?;
                budget.charge(WorkUsage {
                    bytes: u64::try_from(frame.node.prefix.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                self.path.extend_from_slice(&frame.node.prefix, budget)?;
                frame.path_end = length;
                frame.entered = true;
                if !subtree_can_follow(&self.path, self.after_nibbles.as_deref()) {
                    let _finished = self.stack.pop();
                    continue;
                }
                if let Some(value) = frame.node.value.as_ref() {
                    if !self.path.len().is_multiple_of(2) {
                        return Err(IndexError::CorruptObject(
                            "map value ends at an incomplete byte".into(),
                        ));
                    }
                    if self
                        .after_nibbles
                        .as_ref()
                        .is_none_or(|after| self.path.as_slice() > after.as_slice())
                    {
                        if page.entries.len() == self.limit {
                            self.pending_value = true;
                            return Ok(Some(page.finish(true)));
                        }
                        scan_emit_value(&self.path, value, &mut page, budget)?;
                    }
                }
            }
            self.path.truncate(frame.path_end);
            let next = loop {
                let edge = frame.next_edge;
                if edge >= 16 {
                    break None;
                }
                let child = frame.node.branches.get(edge)?;
                frame.next_edge = edge.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                if let Some(child) = child {
                    break Some((edge, child.copy(budget)?));
                }
            };
            let Some((edge, reference)) = next else {
                let _finished = self.stack.pop();
                continue;
            };
            let length = self
                .path
                .len()
                .checked_add(1)
                .filter(|length| *length <= MAX_NIBBLES)
                .ok_or_else(|| IndexError::CorruptObject("map branch exceeds key limit".into()))?;
            budget.charge(WorkUsage {
                bytes: u64::try_from(length).map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            self.path.push(
                u8::try_from(edge).map_err(|_error| IndexError::ResourceLimited)?,
                budget,
            )?;
            if subtree_can_follow(&self.path, self.after_nibbles.as_deref()) {
                let node = match reference {
                    Branch {
                        value: BranchValue::Packed(packed),
                        ..
                    } => ScanNode::Packed(packed),
                    branch => ScanNode::Owned(branch.into_node(budget, &mut *read_node)?),
                };
                self.stack.push(ScanFrame::new(node), budget)?;
            }
        }
        self.finished = true;
        Ok(Some(page.finish(false)))
    }

    /// Carry only buffers whose capacities a fresh scan would already need
    /// to reach the pending value. An earlier, deeper branch can leave unused
    /// capacity in the DFS and would otherwise raise the next page's peak.
    fn fits_fresh_path(&self) -> Result<bool, IndexError> {
        let fresh_stack = self.stack.len().next_power_of_two().min(MAX_NIBBLES + 1);
        if self.stack.capacity() > fresh_stack {
            return Ok(false);
        }
        if self.path.capacity() == self.path.len() {
            return Ok(true);
        }
        let mut prefix_end = 0_usize;
        let mut fresh_path = 0_usize;
        for (depth, frame) in self.stack.iter().enumerate() {
            if depth != 0 {
                fresh_path =
                    MapBuffer::<u8>::capacity_for(fresh_path, prefix_end + 1, MAX_NIBBLES)?;
            }
            prefix_end = frame.path_end;
            fresh_path = MapBuffer::<u8>::capacity_for(fresh_path, prefix_end, MAX_NIBBLES)?;
        }
        Ok(self.path.capacity() <= fresh_path)
    }
}

fn scan_emit_value(
    path: &[u8],
    value: &RetainedMapValue,
    page: &mut ScanPageBuilder,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let key_bytes = u64::try_from(path.len() >> 1).map_err(|_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: key_bytes
            .checked_add(
                u64::try_from(std::mem::size_of::<(Vec<u8>, MapValue)>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let key_memory = if key_bytes == 0 {
        None
    } else {
        budget.reserve_memory(key_bytes, MemoryLifetime::Retained)?
    };
    let key = nibbles_to_key(path)?;
    page.entries.push((key, value.to_public(budget)?));
    page.leases.extend(key_memory);
    Ok(())
}

struct StagedMap {
    scratch: MapScratch,
    packing_mode: Cell<RootPackingMode>,
    staged_children: Option<RefCell<StagedChildren>>,
}

/// One diagnostic packet for a scratch-backed map batch. All counters are
/// attempts, including work reached before a failed batch returned.
struct MapApplyObservation {
    context: Option<DiagnosticContextV1>,
    span: Option<DiagnosticSpanGuardV1>,
    candidates: u64,
    reuses: u64,
    builds: u64,
}

/// Diagnostic-only next-value shape of positions yielded into scratch-backed
/// MapApply. It is neither a persisted-node census nor a copy count.
struct MapInputShape {
    counts: Option<[u64; 6]>,
}

impl MapInputShape {
    fn new(enabled: bool) -> Self {
        Self {
            counts: enabled.then_some([0; 6]),
        }
    }

    fn record(&mut self, position: &OrdinalMapEdit) -> Option<DiagnosticHealthCodeV1> {
        let prior = self.counts?;
        let next = (|| {
            let shape = match position.next_wire() {
                None => [1, 0, 0, 0, 0, 0],
                Some(wire) => {
                    let bytes = u64::try_from(wire.len())
                        .map_err(|_error| DiagnosticHealthCodeV1::CounterOverflow)?;
                    match MapValue::decode(wire)
                        .map_err(|_error| DiagnosticHealthCodeV1::CounterUnavailable)?
                    {
                        MapValue::Object(_) => [0, 1, 0, 0, 0, bytes],
                        MapValue::Inline(value) => {
                            let data = u64::try_from(
                                value
                                    .data()
                                    .map_err(|_error| DiagnosticHealthCodeV1::CounterUnavailable)?
                                    .len(),
                            )
                            .map_err(|_error| DiagnosticHealthCodeV1::CounterOverflow)?;
                            let children = u64::try_from(
                                value
                                    .children()
                                    .map_err(|_error| DiagnosticHealthCodeV1::CounterUnavailable)?
                                    .len(),
                            )
                            .map_err(|_error| DiagnosticHealthCodeV1::CounterOverflow)?;
                            [0, 0, 1, data, children, bytes]
                        }
                    }
                }
            };
            let mut total = prior;
            for (target, add) in total.iter_mut().zip(shape) {
                *target = target
                    .checked_add(add)
                    .ok_or(DiagnosticHealthCodeV1::CounterOverflow)?;
            }
            Ok::<_, DiagnosticHealthCodeV1>(total)
        })();
        match next {
            Ok(total) => {
                self.counts = Some(total);
                None
            }
            Err(health) => {
                self.counts = None;
                Some(health)
            }
        }
    }
}

struct ShapeObservedCursor<'a> {
    input: &'a mut dyn MapEditCursor,
    shape: &'a mut MapInputShape,
    diagnostic: Option<DiagnosticContextV1>,
}

impl MapEditCursor for ShapeObservedCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        let position = self.input.next_edit(budget)?;
        if let Some(position) = position.as_ref()
            && let Some(health) = self.shape.record(position)
            && let Some(diagnostic) = self.diagnostic.as_ref()
        {
            diagnostic.health(health);
        }
        Ok(position)
    }

    fn subtree_packing(&self) -> Result<RootPackingPlan, IndexError> {
        self.input.subtree_packing()
    }
}

impl MapApplyObservation {
    fn new(budget: &WorkBudget) -> Self {
        let context = budget.request().diagnostic_context().cloned();
        let span = context.as_ref().and_then(|context| {
            match context.enter(DiagnosticStageV1::MapApply, DiagnosticLaneV1(0)) {
                Ok(span) => Some(span),
                Err(error) => {
                    context.health(match error {
                        DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                            DiagnosticHealthCodeV1::CounterOverflow
                        }
                        DiagnosticTraceErrorV1::ClockRegressed => {
                            DiagnosticHealthCodeV1::ClockRegressed
                        }
                        DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                            DiagnosticHealthCodeV1::ForeignCaptureSpan
                        }
                        DiagnosticTraceErrorV1::MissingParentSpan => {
                            DiagnosticHealthCodeV1::MissingParentSpan
                        }
                    });
                    None
                }
            }
        });
        Self {
            context,
            span,
            candidates: 0,
            reuses: 0,
            builds: 0,
        }
    }

    fn child_context(&self) -> Option<DiagnosticContextV1> {
        self.context
            .as_ref()
            .zip(self.span.as_ref())
            .map(|(context, span)| context.child_context(span))
    }

    fn finish<T>(&mut self, result: &Result<T, IndexError>, shape: MapInputShape) {
        if let (Some(context), Some(span)) = (self.context.as_ref(), self.span.as_ref()) {
            for (key, value) in [
                (DiagnosticCounterV1::MapRootReuseCandidates, self.candidates),
                (DiagnosticCounterV1::MapRootReuses, self.reuses),
                (DiagnosticCounterV1::MapRootBuilds, self.builds),
            ] {
                context.counter(span, key, value);
            }
            if let Some(counts) = shape.counts {
                for (key, value) in [
                    DiagnosticCounterV1::MapInputNextAbsentValues,
                    DiagnosticCounterV1::MapInputNextObjectValues,
                    DiagnosticCounterV1::MapInputNextInlineValues,
                    DiagnosticCounterV1::MapInputNextInlineDataBytes,
                    DiagnosticCounterV1::MapInputNextInlineChildRefs,
                    DiagnosticCounterV1::MapInputNextWireBytes,
                ]
                .into_iter()
                .zip(counts)
                {
                    context.counter(span, key, value);
                }
            }
        }
        if let Some(span) = self.span.take() {
            let _finished = span.finish(DiagnosticTerminalV1::from_result(result));
        }
    }
}

/// Attribute one map batch phase beneath the existing MapApply packet. The
/// request's diagnostic context is absent in Off, so product work is unchanged.
fn observe_map_phase<T>(
    budget: &mut WorkBudget,
    lane: u16,
    operation: impl FnOnce(&mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let Some(context) = budget.request().diagnostic_context().cloned() else {
        return operation(budget);
    };
    let Some(span) = begin_map_phase(&context, lane) else {
        return operation(budget);
    };
    let before = budget.usage();
    let result = {
        let mut scoped_budget =
            budget.scoped_diagnostic_context(Some(context.child_context(&span)));
        operation(&mut scoped_budget)
    };
    budget.record_diagnostic_usage_delta(before, &span);
    let _finished = span.finish(DiagnosticTerminalV1::from_result(&result));
    result
}

fn begin_map_phase(context: &DiagnosticContextV1, lane: u16) -> Option<DiagnosticSpanGuardV1> {
    match context.enter(DiagnosticStageV1::MapApply, DiagnosticLaneV1(lane)) {
        Ok(span) => Some(span),
        Err(error) => {
            context.health(match error {
                DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                    DiagnosticHealthCodeV1::CounterOverflow
                }
                DiagnosticTraceErrorV1::ClockRegressed => DiagnosticHealthCodeV1::ClockRegressed,
                DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                    DiagnosticHealthCodeV1::ForeignCaptureSpan
                }
                DiagnosticTraceErrorV1::MissingParentSpan => {
                    DiagnosticHealthCodeV1::MissingParentSpan
                }
            });
            None
        }
    }
}

/// One open path in a sorted, empty-root radix build. Completed siblings are
/// already content-addressed in scratch; no intermediate root is installed.
struct BootstrapFrame {
    full_prefix: MapBuffer<u8>,
    value: Option<RetainedMapValue>,
    branches: Branches,
}

/// A radix node on the last changed key's open path. Sorted edits cannot
/// return to a closed child, so shared ancestors are encoded only on close.
struct EditFrame {
    node: Node,
    reference: Option<ObjectRef>,
    end: usize,
    parent_edge: Option<usize>,
    dirty: bool,
}

struct OpenPathStart {
    reference: Option<Branch>,
    offset: usize,
    parent_edge: Option<usize>,
}

#[derive(Clone, Copy)]
enum EditExpectation {
    // Sorted stream edits were authenticated against their original base.
    AlreadyChecked,
    Exact(Option<MapValue>),
}

impl EditExpectation {
    fn require(self, actual: Option<&RetainedMapValue>) -> Result<(), IndexError> {
        if let Self::Exact(expected) = self {
            let equal = match (expected, actual) {
                (None, None) => true,
                (Some(expected), Some(actual)) => actual.equals_public(expected)?,
                _ => false,
            };
            if !equal {
                return Err(IndexError::Conflict);
            }
        }
        Ok(())
    }
}

impl BootstrapFrame {
    fn new(full_prefix: MapBuffer<u8>, value: Option<RetainedMapValue>) -> Self {
        Self {
            full_prefix,
            value,
            branches: Branches::new(),
        }
    }
}

fn charge_bootstrap_frame(budget: &mut WorkBudget, prefix_len: usize) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        bytes: u64::try_from(
            std::mem::size_of::<BootstrapFrame>()
                .checked_add(prefix_len)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })
}

impl StagedMap {
    fn open(store: &FileObjectStore, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        Ok(Self {
            scratch: MapScratch::open(store, budget)?,
            packing_mode: Cell::new(RootPackingMode::None),
            staged_children: None,
        })
    }

    fn track_staged_children(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        self.staged_children = StagedChildren::try_new(budget)?.map(RefCell::new);
        Ok(())
    }
}

/// The radix algorithm and child-first installer use the same canonical
/// encoding with either disposable `SQLite` or bounded, request-owned staging.
trait MapStage {
    const DEFERRED_COMPLETION: bool = false;

    fn packing_mode(&self) -> RootPackingMode {
        RootPackingMode::None
    }

    fn read_node(
        &self,
        store: &FileObjectStore,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Node, IndexError>;

    fn insert(
        &self,
        object: PreparedObject,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError>;
    /// Installation begins only after construction. Return owned canonical
    /// wire, transferring its existing lease when staging already owns it.
    fn take_prepared(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<PreparedObject>, IndexError>;
    fn may_contain_staged_child(
        &self,
        _reference: ObjectRef,
        _budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        Ok(true)
    }
    fn mark(&self, reference: ObjectRef, budget: &mut WorkBudget)
    -> Result<Option<u8>, IndexError>;
    fn set_mark(
        &self,
        reference: ObjectRef,
        state: u8,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>;
    fn complete_batch(
        &self,
        _objects: &[&PreparedObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()
    }
    fn begin_batch(&self, budget: &WorkBudget) -> Result<(), IndexError>;
    fn commit_batch(&self, budget: &WorkBudget) -> Result<(), IndexError>;
}

impl MapStage for StagedMap {
    const DEFERRED_COMPLETION: bool = true;

    fn packing_mode(&self) -> RootPackingMode {
        self.packing_mode.get()
    }

    fn read_node(
        &self,
        store: &FileObjectStore,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Node, IndexError> {
        self.scratch.read_node(store, reference, budget)
    }

    fn insert(
        &self,
        object: PreparedObject,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        if self.staged_children.is_some() {
            StagedChildren::charge_insert(budget)?;
        }
        self.scratch.insert_node(&object, budget)?;
        let reference = object.registration.reference;
        if let Some(children) = &self.staged_children {
            children.borrow_mut().record(reference);
        }
        Ok(reference)
    }

    fn take_prepared(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<PreparedObject>, IndexError> {
        self.scratch.prepared(reference, budget)
    }

    fn may_contain_staged_child(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        if let Some(children) = &self.staged_children {
            return children.borrow().may_contain(reference, budget);
        }
        Ok(true)
    }

    fn mark(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<u8>, IndexError> {
        self.scratch.mark(reference, budget)
    }

    fn set_mark(
        &self,
        reference: ObjectRef,
        state: u8,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.scratch.set_mark(reference, state, budget)
    }

    fn complete_batch(
        &self,
        objects: &[&PreparedObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        // A threshold flush may have committed the transaction opened when
        // this node was entered. Completion must own one transaction too.
        self.scratch.begin_batch(budget)?;
        self.scratch.complete_batch(objects, budget)
    }

    fn begin_batch(&self, budget: &WorkBudget) -> Result<(), IndexError> {
        self.scratch.begin_batch(budget)
    }

    fn commit_batch(&self, budget: &WorkBudget) -> Result<(), IndexError> {
        self.scratch.commit_batch(budget)
    }
}

fn reachable_chunk_references<'a>(
    chunk: &'a [PreparedObject],
    budget: &mut WorkBudget,
) -> Result<MapBuffer<&'a PreparedObject>, IndexError> {
    budget.charge(WorkUsage {
        bytes: u64::try_from(chunk.len())
            .map_err(|_conversion_error| IndexError::ResourceLimited)?
            .checked_mul(
                u64::try_from(std::mem::size_of::<&PreparedObject>())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let mut references = MapBuffer::new(MAX_INSTALL_BATCH_INPUTS);
    for object in chunk {
        references.push(object, budget)?;
    }
    Ok(references)
}

fn flush_reachable_chunk(
    store: &FileObjectStore,
    chunk: &mut MapBuffer<PreparedObject>,
    sink: &dyn ObjectWriteSink,
    budget: &mut WorkBudget,
    before_install: impl FnOnce(&[&PreparedObject], &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    if chunk.is_empty() {
        return Ok(());
    }
    let references = reachable_chunk_references(chunk, budget)?;
    // Keep the original reference-array refusal ahead of the commit boundary.
    before_install(&references, budget)?;
    store.install_prepared_batch(&references, sink, budget)?;
    // Borrowed slots must die before their pointees are cleared. Keep the
    // admitted chunk backing for the next bounded installation group.
    drop(references);
    chunk.truncate(0);
    Ok(())
}

fn flush_reachable_run(
    store: &FileObjectStore,
    ready: &mut [MapBuffer<PreparedObject>; MAX_PREPARED_INSTALL_GROUPS - 1],
    ready_count: usize,
    current: &mut MapBuffer<PreparedObject>,
    sink: &dyn ObjectWriteSink,
    budget: &mut WorkBudget,
    complete_current: impl FnOnce(&[&PreparedObject], &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let group_count = ready_count
        .checked_add(usize::from(!current.is_empty()))
        .ok_or(IndexError::ResourceLimited)?;
    if ready_count > ready.len() || group_count > MAX_PREPARED_INSTALL_GROUPS {
        return Err(IndexError::ResourceLimited);
    }
    if group_count == 0 {
        return Ok(());
    }
    let mut references: [MapBuffer<&PreparedObject>; MAX_PREPARED_INSTALL_GROUPS] =
        std::array::from_fn(|_| MapBuffer::new(MAX_INSTALL_BATCH_INPUTS));
    for (index, group) in ready.iter().take(ready_count).enumerate() {
        references[index] = reachable_chunk_references(group, budget)?;
    }
    if !current.is_empty() {
        references[ready_count] = reachable_chunk_references(current, budget)?;
        complete_current(&references[ready_count], budget)?;
    }
    {
        let groups: [&[&PreparedObject]; MAX_PREPARED_INSTALL_GROUPS] =
            std::array::from_fn(|index| references[index].as_slice());
        if group_count == 1 {
            store.install_prepared_batch(groups[0], sink, budget)?;
        } else {
            store.install_prepared_run(&groups[..group_count], sink, budget)?;
        }
    }
    drop(references);
    for group in ready.iter_mut().take(ready_count) {
        group.truncate(0);
    }
    current.truncate(0);
    Ok(())
}

impl FileObjectStore {
    /// The final reachable DAG is already deduplicated and child-first. Commit
    /// its objects in bounded custody groups without publishing intermediate
    /// map roots. A later group may depend only on earlier registered groups.
    fn install_reachable_map_nodes<S: MapStage>(
        &self,
        roots: &[MapRoot],
        staged: &S,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.install_reachable_objects(roots, &[], staged, sink, budget)
    }

    fn install_reachable_objects<S: MapStage>(
        &self,
        roots: &[MapRoot],
        objects: &[ObjectRef],
        staged: &S,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        // The two logical roots keep their original ordinals even if one is
        // empty. A staged object reached by both roots belongs to the first
        // traversal that queues it. Physical batches can cross that boundary.
        if roots.len() == 2
            && objects.is_empty()
            && let Some(context) = budget.request().diagnostic_context().cloned()
        {
            let mut counts = [0, 0];
            let result = self.install_reachable_objects_inner(
                roots,
                objects,
                staged,
                sink,
                budget,
                Some(&mut counts),
            );
            for (lane, count) in [6, 7].into_iter().zip(counts) {
                if let Some(span) = begin_map_phase(&context, lane) {
                    context.counter(&span, DiagnosticCounterV1::NewObjects, count);
                    let _finished = span.finish(DiagnosticTerminalV1::from_result(&result));
                }
            }
            return result;
        }
        self.install_reachable_objects_inner(roots, objects, staged, sink, budget, None)
    }

    fn install_reachable_objects_inner<S: MapStage>(
        &self,
        roots: &[MapRoot],
        objects: &[ObjectRef],
        staged: &S,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
        mut counts: Option<&mut [u64; 2]>,
    ) -> Result<(), IndexError> {
        let mut pending = MapBuffer::new(MAX_INSTALL_PENDING);
        // One already-verified, owned wire per active depth. Its original
        // allocation grant stays attached until child-first installation.
        let mut active = MapBuffer::<PreparedObject>::new(MAX_NIBBLES + 1);
        let mut chunk = MapBuffer::<PreparedObject>::new(MAX_INSTALL_BATCH_INPUTS);
        let mut ready: [MapBuffer<PreparedObject>; MAX_PREPARED_INSTALL_GROUPS - 1] =
            std::array::from_fn(|_| MapBuffer::new(MAX_INSTALL_BATCH_INPUTS));
        let mut ready_count = 0usize;
        let mut ready_bytes = 0u64;
        let run_enabled = S::DEFERRED_COMPLETION && self.can_run_prepared_installs(sink);
        let mut completed = CompletedGroup::new();
        let mut chunk_bytes = 0u64;
        let complete_chunk = |objects: &[&PreparedObject], budget: &mut WorkBudget| {
            staged.complete_batch(objects, budget)?;
            staged.commit_batch(budget)
        };
        for (ordinal, reference) in roots
            .iter()
            .map(|root| root.0)
            .chain(objects.iter().copied().map(Some))
            .enumerate()
        {
            let Some(reference) = reference else { continue };
            InstallFrame::enter(reference, 0)?.push(&mut pending, budget)?;
            while let Some(frame) = pending.pop() {
                budget.checkpoint()?;
                let reference = frame.reference;
                let exiting = frame.exiting();
                if S::DEFERRED_COMPLETION
                    && !exiting
                    && completed.contains(reference, &chunk, budget)?
                {
                    continue;
                }
                let object = if exiting {
                    let object = active.pop().ok_or_else(|| {
                        IndexError::BackendViolation("missing staged map ancestor".into())
                    })?;
                    if object.registration.reference != reference {
                        return Err(IndexError::BackendViolation(
                            "staged map ancestor identity changed".into(),
                        ));
                    }
                    object
                } else {
                    let Some(object) = staged.take_prepared(reference, budget)? else {
                        continue;
                    };
                    object
                };
                if exiting {
                    if staged.mark(reference, budget)? != Some(1) {
                        return Err(IndexError::CorruptObject(
                            "invalid staged map traversal state".into(),
                        ));
                    }
                    staged.begin_batch(budget)?;
                    if !S::DEFERRED_COMPLETION {
                        staged.set_mark(reference, 2, budget)?;
                    }
                    let next_bytes = chunk_bytes
                        .checked_add(object.registration.bytes)
                        .ok_or(IndexError::ResourceLimited)?;
                    let run_bytes = ready_bytes
                        .checked_add(next_bytes)
                        .ok_or(IndexError::ResourceLimited)?;
                    if chunk.len() == MAX_INSTALL_BATCH_INPUTS || run_bytes > MAX_BATCH_BYTES {
                        if run_enabled && ready_count < ready.len() && run_bytes <= MAX_BATCH_BYTES
                        {
                            // Scratch completion is private until the bounded
                            // physical run has custody of every group.
                            let references = reachable_chunk_references(&chunk, budget)?;
                            complete_chunk(&references, budget)?;
                            drop(references);
                            std::mem::swap(&mut ready[ready_count], &mut chunk);
                            ready_count += 1;
                            ready_bytes = ready_bytes
                                .checked_add(chunk_bytes)
                                .ok_or(IndexError::ResourceLimited)?;
                        } else {
                            if ready_count == 0 {
                                flush_reachable_chunk(
                                    self,
                                    &mut chunk,
                                    sink,
                                    budget,
                                    complete_chunk,
                                )?;
                            } else {
                                flush_reachable_run(
                                    self,
                                    &mut ready,
                                    ready_count,
                                    &mut chunk,
                                    sink,
                                    budget,
                                    complete_chunk,
                                )?;
                            }
                            ready_count = 0;
                            ready_bytes = 0;
                        }
                        if S::DEFERRED_COMPLETION {
                            completed.clear(budget)?;
                        }
                        chunk_bytes = 0;
                    }
                    chunk_bytes = chunk_bytes
                        .checked_add(object.registration.bytes)
                        .ok_or(IndexError::ResourceLimited)?;
                    if chunk_bytes > MAX_BATCH_BYTES {
                        return Err(IndexError::ResourceLimited);
                    }
                    budget.charge(WorkUsage {
                        bytes: object.registration.bytes,
                        ..WorkUsage::default()
                    })?;
                    chunk.push(object, budget)?;
                    if let Some(count) = counts
                        .as_deref_mut()
                        .and_then(|counts| counts.get_mut(ordinal))
                    {
                        *count += 1;
                    }
                    if S::DEFERRED_COMPLETION {
                        completed.insert(
                            reference,
                            chunk
                                .len()
                                .checked_sub(1)
                                .ok_or(IndexError::ResourceLimited)?,
                            &chunk,
                            budget,
                        )?;
                    }
                    continue;
                }
                match staged.mark(reference, budget)? {
                    Some(2) => continue,
                    Some(1) => {
                        return Err(IndexError::CorruptObject("cyclic staged map DAG".into()));
                    }
                    Some(_) => {
                        return Err(IndexError::CorruptObject("invalid staged map mark".into()));
                    }
                    None => {}
                }
                let depth = frame.depth();
                if depth > MAX_NIBBLES {
                    return Err(IndexError::CorruptObject(
                        "staged map DAG exceeds key depth".into(),
                    ));
                }
                staged.begin_batch(budget)?;
                staged.set_mark(reference, 1, budget)?;
                frame.exit().push(&mut pending, budget)?;
                for child in object.registration.children.iter().rev() {
                    // A missing staged prefix has no node to install. Every
                    // external child still goes through physical validation.
                    if !staged.may_contain_staged_child(*child, budget)? {
                        continue;
                    }
                    let next_depth = depth.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    InstallFrame::enter(*child, next_depth)?.push(&mut pending, budget)?;
                }
                budget.charge(WorkUsage {
                    bytes: u64::try_from(std::mem::size_of::<PreparedObject>())
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                active.push(object, budget)?;
            }
        }
        if ready_count == 0 {
            flush_reachable_chunk(self, &mut chunk, sink, budget, complete_chunk)
        } else {
            flush_reachable_run(
                self,
                &mut ready,
                ready_count,
                &mut chunk,
                sink,
                budget,
                complete_chunk,
            )
        }
    }

    pub(crate) fn read_node(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Node, IndexError> {
        decode_node(&self.get(reference, budget)?, budget)
    }
}

pub(crate) fn decode_node(
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<Node, IndexError> {
    node_wire::decode(stored, budget)
}

impl crate::FileObjectReader {
    /// Read a canonical inline value or object edge under an existing namespace.
    pub fn lookup_map_value_v1(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        let mut result = [None];
        original_lookup::lookup_batch_with(
            root,
            &[key],
            &mut result,
            budget,
            |reference, budget| decode_node(&self.get(reference, budget)?, budget),
        )?;
        Ok(result[0])
    }

    /// Resolve an object edge using the existing immutable-only map path.
    pub fn lookup_map_object(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        match self.lookup_map_value_v1(root, key, budget)? {
            Some(MapValue::Object(reference)) => Ok(Some(reference)),
            None => Ok(None),
            Some(MapValue::Inline(_)) => Err(IndexError::CorruptObject(
                "map object lookup resolved to an inline value".into(),
            )),
        }
    }

    /// Scan an existing immutable map without constructing a writer or scratch scope.
    /// The returned page retains its original memory reservations.
    pub fn scan_map(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        FileObjectStore::scan_with(root, after, limit, budget, |reference, budget| {
            decode_node(&self.get(reference, budget)?, budget)
        })
    }
}

impl PersistentMapPort for crate::FileObjectReader {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        self.lookup_map_value_v1(root, key, budget)
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        self.scan_map(root, after, limit, budget)
    }

    fn apply(
        &self,
        _base: MapRoot,
        _edits: &[MapEdit],
        _sink: &dyn ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        Err(IndexError::BackendViolation("read-only map port cannot write".into()))
    }

    fn apply_batch(
        &self,
        _requests: &[MapBatchRequest],
        _sink: &dyn ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        Err(IndexError::BackendViolation("read-only map port cannot write".into()))
    }

    fn apply_stream(
        &self,
        _bases: &[MapRoot],
        _edits: &mut dyn MapEditCursor,
        _sink: &dyn ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        Err(IndexError::BackendViolation("read-only map port cannot write".into()))
    }
}

impl FileObjectStore {
    fn materialize_root(
        &self,
        child: Option<Branch>,
        staged: &impl MapStage,
        budget: &mut WorkBudget,
    ) -> Result<MapRoot, IndexError> {
        let reference = match child {
            None => None,
            Some(Branch {
                value: BranchValue::Indirect(reference),
                ..
            }) => Some(reference),
            Some(Branch {
                prefix,
                value: BranchValue::Terminal(value),
            }) => Some(self.write_encoded(
                &Node {
                    prefix,
                    value: Some(value),
                    branches: Branches::new(),
                },
                staged,
                budget,
            )?),
            Some(Branch {
                value: BranchValue::Packed(packed),
                ..
            }) => Some(self.write_encoded(&packed.node, staged, budget)?),
        };
        Ok(MapRoot(reference))
    }

    /// Seal one completed sorted-build path and attach it to its immediate
    /// open parent. The stack length, rather than recursion depth, bounds the
    /// build even for maximum-length keys.
    fn seal_bootstrap_frame(
        &self,
        frames: &mut MapBuffer<BootstrapFrame>,
        staged: &StagedMap,
        budget: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        budget.checkpoint()?;
        let frame = frames.pop().ok_or_else(|| {
            IndexError::BackendViolation("sorted map build has no frame to seal".into())
        })?;
        let parent_len = frames.last().map_or(0, |parent| parent.full_prefix.len());
        let suffix_at = if frames.is_empty() {
            0
        } else {
            parent_len
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?
        };
        let prefix = frame
            .full_prefix
            .get(suffix_at..)
            .ok_or_else(|| IndexError::BackendViolation("invalid sorted map frame path".into()))?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(prefix.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let reference = self
            .write_node(
                Node {
                    prefix: MapBuffer::copy(prefix, MAX_NIBBLES, budget)?,
                    value: frame.value,
                    branches: frame.branches,
                },
                staged,
                budget,
            )?
            .ok_or_else(|| IndexError::BackendViolation("empty sorted map frame".into()))?;
        if let Some(parent) = frames.last_mut() {
            if !frame.full_prefix.starts_with(&parent.full_prefix) {
                return Err(IndexError::BackendViolation(
                    "sorted map frame is outside its parent".into(),
                ));
            }
            let edge = usize::from(*frame.full_prefix.get(parent_len).ok_or_else(|| {
                IndexError::BackendViolation("sorted map frame has no parent edge".into())
            })?);
            if parent
                .branches
                .replace(edge, Some(reference), budget)?
                .is_some()
            {
                return Err(IndexError::BackendViolation(
                    "sorted map parent edge was assigned twice".into(),
                ));
            }
            Ok(None)
        } else {
            Ok(self.materialize_root(Some(reference), staged, budget)?.0)
        }
    }

    /// Sorted initial ingest builds each affected radix prefix once. The
    /// cursor is already validated and spooled against the fixed empty base.
    fn build_empty_root(
        &self,
        ordinal: u32,
        staged: &StagedMap,
        budget: &mut WorkBudget,
        verify: Option<([u8; 32], u64)>,
    ) -> Result<MapRoot, IndexError> {
        let mut edits = staged.scratch.edits(ordinal);
        self.build_empty_root_from_cursor(&mut edits, staged, budget, verify)
    }

    fn build_empty_root_from_cursor(
        &self,
        edits: &mut dyn MapEditCursor,
        staged: &StagedMap,
        budget: &mut WorkBudget,
        verify: Option<([u8; 32], u64)>,
    ) -> Result<MapRoot, IndexError> {
        let mut frames = MapBuffer::<BootstrapFrame>::new(MAX_NIBBLES + 1);
        let mut fingerprint = verify.map(|_| Sha256::new());
        let mut edit_count = 0u64;
        while let Some(position) = edits.next_edit(budget)? {
            budget.checkpoint()?;
            if let Some(fingerprint) = fingerprint.as_mut() {
                fingerprint_position(fingerprint, &position, budget)?;
                edit_count = edit_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            budget.charge(WorkUsage {
                bytes: u64::try_from(position.key().len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let Some(value) = position.next()? else {
                continue;
            };
            let value = RetainedMapValue::from_public(value, budget)?;
            charge_key_allocation(position.key(), budget)?;
            let nibbles = key_nibbles(position.key(), budget)?;
            let shared = frames.last().map_or(0, |last| {
                last.full_prefix
                    .iter()
                    .zip(nibbles.iter())
                    .take_while(|(left, right)| left == right)
                    .count()
            });
            while frames
                .last()
                .is_some_and(|frame| frame.full_prefix.len() > shared)
            {
                let parent_len = frames
                    .len()
                    .checked_sub(2)
                    .and_then(|index| frames.get(index))
                    .map(|parent| parent.full_prefix.len());
                if parent_len.is_none_or(|len| len < shared) {
                    let child = frames.pop().ok_or_else(|| {
                        IndexError::BackendViolation("sorted map child is missing".into())
                    })?;
                    charge_bootstrap_frame(budget, shared)?;
                    frames.push(
                        BootstrapFrame::new(
                            MapBuffer::copy(
                                nibbles.get(..shared).ok_or(IndexError::ResourceLimited)?,
                                MAX_NIBBLES,
                                budget,
                            )?,
                            None,
                        ),
                        budget,
                    )?;
                    frames.push(child, budget)?;
                }
                let _attached = self.seal_bootstrap_frame(&mut frames, staged, budget)?;
            }
            if frames
                .last()
                .is_some_and(|parent| parent.full_prefix.len() >= nibbles.len())
            {
                return Err(IndexError::BackendViolation(
                    "sorted map keys do not extend the open parent".into(),
                ));
            }
            charge_bootstrap_frame(budget, nibbles.len())?;
            frames.push(BootstrapFrame::new(nibbles, Some(value)), budget)?;
        }
        if let Some((expected_fingerprint, expected_count)) = verify {
            let actual_fingerprint: [u8; 32] = fingerprint
                .ok_or(IndexError::ResourceLimited)?
                .finalize()
                .into();
            if (actual_fingerprint, edit_count) != (expected_fingerprint, expected_count) {
                return Err(IndexError::CorruptObject(
                    "scratch map edits changed after validation".into(),
                ));
            }
        }
        let mut root = None;
        while !frames.is_empty() {
            if let Some(reference) = self.seal_bootstrap_frame(&mut frames, staged, budget)? {
                root = Some(reference);
            }
        }
        Ok(MapRoot(root))
    }

    fn write_node(
        &self,
        node: Node,
        staged: &impl MapStage,
        budget: &mut WorkBudget,
    ) -> Result<Option<Branch>, IndexError> {
        if node.value.is_none() {
            match node.branch_count() {
                0 => return Ok(None),
                1 => {
                    let edge = node.sole_branch().map(|(edge, _)| edge).ok_or_else(|| {
                        IndexError::BackendViolation("unary map branch missing".into())
                    })?;
                    let child = node.branches.into_child(edge)?.ok_or_else(|| {
                        IndexError::BackendViolation("unary map branch missing".into())
                    })?;
                    let child = child.into_node(budget, |reference, budget| {
                        staged.read_node(self, reference, budget)
                    })?;
                    let capacity = node
                        .prefix
                        .len()
                        .checked_add(1)
                        .and_then(|len| len.checked_add(child.prefix.len()))
                        .ok_or(IndexError::ResourceLimited)?;
                    if capacity > MAX_NIBBLES {
                        return Err(IndexError::CorruptObject(
                            "map path exceeds key limit".into(),
                        ));
                    }
                    budget.charge(WorkUsage {
                        bytes: u64::try_from(capacity)
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                        ..WorkUsage::default()
                    })?;
                    let mut prefix = MapBuffer::new(capacity);
                    prefix.extend_from_slice(&node.prefix, budget)?;
                    prefix.push(
                        u8::try_from(edge)
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                        budget,
                    )?;
                    prefix.extend_from_slice(&child.prefix, budget)?;
                    return self.write_node(
                        Node {
                            prefix,
                            value: child.value,
                            branches: child.branches,
                        },
                        staged,
                        budget,
                    );
                }
                _ => {}
            }
        }
        if node.branch_count() == 0
            && node.prefix.len() <= MAX_INLINE_NIBBLES
            && let Some(value) = node.value.as_ref()
        {
            return Ok(Some(Branch::terminal(
                MapBuffer::copy(&node.prefix, MAX_INLINE_NIBBLES, budget)?,
                value.clone(),
            )));
        }
        if node.packable(staged.packing_mode()) {
            return Branch::packed(node, budget).map(Some);
        }
        self.write_encoded(&node, staged, budget)
            .map(Branch::indirect)
            .map(Some)
    }

    fn write_encoded(
        &self,
        node: &Node,
        staged: &impl MapStage,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        let layout = node_wire::layout(node)?;
        let mut spills = [None; 16];
        if layout.spill_mask != 0 {
            for (edge, branch) in node.branches.iter() {
                if layout.spill_mask & (1u16 << edge) != 0 {
                    let BranchValue::Packed(packed) = &branch.value else {
                        return Err(IndexError::BackendViolation(
                            "map layout spilled a non-packed branch".into(),
                        ));
                    };
                    // Every packed child has a bounded inline depth. A spill
                    // consumes one level and is installed child-first.
                    spills[edge] = Some(self.write_encoded(&packed.node, staged, budget)?);
                }
            }
        }
        let encoded = node_wire::encode(layout, &spills, budget)?;
        let object = self.prepare_object(&encoded.payload, &encoded.children, budget)?;
        staged.insert(object, budget)
    }

    fn scan_with(
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
        mut read_node: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
    ) -> Result<MapPage, IndexError> {
        let mut scan = ScanContinuation::new(root, after, limit, budget, &mut read_node)?;
        scan.next_page(budget, &mut read_node)?
            .ok_or_else(|| IndexError::BackendViolation("new map scan has no first page".into()))
    }

    /// Seek the greatest entry by descending one rightmost branch per level.
    /// No sibling is decoded or retained, regardless of map cardinality.
    fn last_entry_with(
        root: MapRoot,
        budget: &mut WorkBudget,
        mut read_node: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
    ) -> Result<MapPage, IndexError> {
        budget.checkpoint()?;
        let mut page = ScanPageBuilder::new(usize::from(root.0.is_some()), budget)?;
        let mut path = MapBuffer::<u8>::new(MAX_NIBBLES);
        let Some(reference) = root.0 else {
            return Ok(page.finish(false));
        };
        let mut node = read_node(reference, budget)?;
        loop {
            budget.checkpoint()?;
            let length = path
                .len()
                .checked_add(node.prefix.len())
                .filter(|length| *length <= MAX_NIBBLES)
                .ok_or_else(|| IndexError::CorruptObject("map path exceeds key limit".into()))?;
            budget.charge(WorkUsage {
                bytes: u64::try_from(node.prefix.len())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            path.extend_from_slice(&node.prefix, budget)?;
            debug_assert_eq!(path.len(), length);

            let mut rightmost = None;
            for edge in (0..16).rev() {
                if node.branches.get(edge)?.is_some() {
                    rightmost = Some(edge);
                    break;
                }
            }
            if let Some(edge) = rightmost {
                let length = path
                    .len()
                    .checked_add(1)
                    .filter(|length| *length <= MAX_NIBBLES)
                    .ok_or_else(|| {
                        IndexError::CorruptObject("map branch exceeds key limit".into())
                    })?;
                budget.charge(WorkUsage {
                    bytes: u64::try_from(length).map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                path.push(
                    u8::try_from(edge).map_err(|_error| IndexError::ResourceLimited)?,
                    budget,
                )?;
                let branch = node.branches.into_child(edge)?.ok_or_else(|| {
                    IndexError::BackendViolation("rightmost map branch disappeared".into())
                })?;
                node = branch.into_node(budget, &mut read_node)?;
                continue;
            }
            if !path.len().is_multiple_of(2) {
                return Err(IndexError::CorruptObject(
                    "map value ends at an incomplete byte".into(),
                ));
            }
            let value = node.value.as_ref().ok_or_else(|| {
                IndexError::CorruptObject("rightmost map node has no value".into())
            })?;
            scan_emit_value(&path, value, &mut page, budget)?;
            return Ok(page.finish(false));
        }
    }

    /// The caller has already consumed `last`; only strictly later rows are
    /// scanned here after an optional DFS was discarded during child work.
    fn visit_scalar_scan_tail_with(
        root: MapRoot,
        mut last: MapPage,
        budget: &mut WorkBudget,
        read_node: &mut impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
        visitor: &mut dyn FnMut(MapPage, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        loop {
            let key = &last
                .entries
                .last()
                .ok_or_else(|| {
                    IndexError::BackendViolation("map scan promised an empty continuation".into())
                })?
                .0;
            let page = Self::scan_with(root, Some(key), 1, budget, &mut *read_node)?;
            drop(last);
            if !page.has_more {
                return visitor(page, budget);
            }
            last = page.clone();
            visitor(page, budget)?;
        }
    }

    /// Visit one-row pages with a single DFS while Original grants permit it.
    /// The first page retains the ordinary scan's allocation and read order.
    /// The optional boxed stack is discarded before a mandatory child grant
    /// would be refused; a miss resumes after the last delivered fence.
    fn try_visit_scan_pages_with(
        root: MapRoot,
        budget: &mut WorkBudget,
        mut read_node: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
        visitor: &mut dyn FnMut(MapPage, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<bool, IndexError> {
        let mut scan = ScanContinuation::new(root, None, 1, budget, &mut read_node)?;
        loop {
            let page = scan.next_page(budget, &mut read_node)?.ok_or_else(|| {
                IndexError::BackendViolation("map scan ended without an EOF page".into())
            })?;
            if !page.has_more {
                drop(scan);
                visitor(page, budget)?;
                return Ok(true);
            }
            let last = page.clone();
            let bytes = u64::try_from(std::mem::size_of::<ScanSuspend>())
                .map_err(|_error| IndexError::ResourceLimited)?;
            if scan.fits_fresh_path()?
                && let Some(memory) =
                    budget.try_reserve_memory_for_optimization(bytes, MemoryLifetime::Temporary)?
            {
                let suspended = Box::new(ScanSuspend {
                    scan,
                    _memory: memory,
                });
                let (result, retained) =
                    budget.with_discardable_memory(suspended, |budget| visitor(page, budget));
                result?;
                if let Some(retained) = retained {
                    let ScanSuspend {
                        scan: resumed,
                        _memory: memory,
                    } = *retained;
                    drop(memory);
                    scan = resumed;
                    continue;
                }
            } else {
                drop(scan);
                visitor(page, budget)?;
            }
            Self::visit_scalar_scan_tail_with(root, last, budget, &mut read_node, visitor)?;
            return Ok(true);
        }
    }

    fn lookup_key(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
        read_node: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
    ) -> Result<Option<MapValue>, IndexError> {
        budget.checkpoint()?;
        let Some(nibbles) = lookup_nibbles(root, key, budget)? else {
            return Ok(None);
        };
        self.lookup_inner_with(root, &nibbles, budget, read_node)
    }

    fn lookup_inner(
        &self,
        root: MapRoot,
        nibbles: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        self.lookup_inner_with(root, nibbles, budget, |reference, budget| {
            self.read_node(reference, budget)
        })
    }

    fn lookup_inner_with(
        &self,
        root: MapRoot,
        nibbles: &[u8],
        budget: &mut WorkBudget,
        mut read_node: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
    ) -> Result<Option<MapValue>, IndexError> {
        let mut cursor = root.0.map(Branch::indirect);
        let mut at = 0usize;
        while let Some(branch) = cursor {
            let node = branch.into_node(budget, &mut read_node)?;
            if !nibbles
                .get(at..)
                .is_some_and(|remaining| remaining.starts_with(&node.prefix))
            {
                return Ok(None);
            }
            at = at
                .checked_add(node.prefix.len())
                .ok_or(IndexError::ResourceLimited)?;
            if at == nibbles.len() {
                return node
                    .value
                    .as_ref()
                    .map(|value| value.to_public(budget))
                    .transpose();
            }
            let branch =
                usize::from(*nibbles.get(at).ok_or_else(|| {
                    IndexError::CorruptObject("map traversal passed key end".into())
                })?);
            at = at.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            cursor = node.branches.into_child(branch)?;
            if at > MAX_NIBBLES {
                return Err(IndexError::CorruptObject(
                    "map traversal exceeds key limit".into(),
                ));
            }
        }
        Ok(None)
    }

    fn update_one(
        &self,
        root: MapRoot,
        nibbles: &[u8],
        next: Option<MapValue>,
        staged: &impl MapStage,
        budget: &mut WorkBudget,
    ) -> Result<MapRoot, IndexError> {
        let child = self.update_child(
            root.0.map(Branch::indirect),
            nibbles,
            EditExpectation::AlreadyChecked,
            next,
            staged,
            budget,
        )?;
        self.materialize_root(child, staged, budget)
    }

    fn update_one_checked(
        &self,
        root: MapRoot,
        nibbles: &[u8],
        expected: Option<MapValue>,
        next: Option<MapValue>,
        staged: &impl MapStage,
        budget: &mut WorkBudget,
    ) -> Result<MapRoot, IndexError> {
        let child = self.update_child(
            root.0.map(Branch::indirect),
            nibbles,
            EditExpectation::Exact(expected),
            next,
            staged,
            budget,
        )?;
        self.materialize_root(child, staged, budget)
    }

    fn update_child(
        &self,
        root: Option<Branch>,
        nibbles: &[u8],
        expected: EditExpectation,
        next: Option<MapValue>,
        staged: &impl MapStage,
        budget: &mut WorkBudget,
    ) -> Result<Option<Branch>, IndexError> {
        let mut trail = MapBuffer::<(Node, usize)>::new(MAX_NIBBLES + 1);
        let mut at = 0usize;
        let mut cursor = root;
        let mut replacement =
            loop {
                let Some(branch) = cursor else {
                    expected.require(None)?;
                    let new_leaf = match next {
                        Some(value) => {
                            let value = RetainedMapValue::from_public(value, budget)?;
                            let suffix = nibbles.get(at..).ok_or_else(|| {
                                IndexError::CorruptObject("map traversal passed key end".into())
                            })?;
                            budget.charge(WorkUsage {
                                bytes: u64::try_from(suffix.len())
                                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                                ..WorkUsage::default()
                            })?;
                            self.write_node(Node::leaf(suffix, value, budget)?, staged, budget)?
                        }
                        None => None,
                    };
                    break new_leaf;
                };
                let mut node = branch.into_node(budget, |reference, budget| {
                    staged.read_node(self, reference, budget)
                })?;
                let remaining = nibbles.get(at..).ok_or_else(|| {
                    IndexError::CorruptObject("map traversal passed key end".into())
                })?;
                let shared = node
                    .prefix
                    .iter()
                    .zip(remaining)
                    .take_while(|(left, right)| left == right)
                    .count();
                if shared < node.prefix.len() {
                    expected.require(None)?;
                    let Some(value) = next else {
                        break self.write_node(node, staged, budget)?;
                    };
                    let value = RetainedMapValue::from_public(value, budget)?;
                    let old_edge = usize::from(*node.prefix.get(shared).ok_or_else(|| {
                        IndexError::CorruptObject("missing map split edge".into())
                    })?);
                    budget.charge(WorkUsage {
                        bytes: u64::try_from(node.prefix.len())
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                        ..WorkUsage::default()
                    })?;
                    let common = MapBuffer::copy(
                        node.prefix.get(..shared).ok_or_else(|| {
                            IndexError::CorruptObject("missing map common prefix".into())
                        })?,
                        MAX_NIBBLES,
                        budget,
                    )?;
                    let suffix_at = shared.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    node.prefix = MapBuffer::copy(
                        node.prefix.get(suffix_at..).ok_or_else(|| {
                            IndexError::CorruptObject("missing old map suffix".into())
                        })?,
                        MAX_NIBBLES,
                        budget,
                    )?;
                    let old_reference = self.write_node(node, staged, budget)?;
                    let mut parent = Node {
                        prefix: common,
                        value: None,
                        branches: Branches::new(),
                    };
                    let _old = parent.branches.replace(old_edge, old_reference, budget)?;
                    let new_at = at.checked_add(shared).ok_or(IndexError::ResourceLimited)?;
                    if new_at == nibbles.len() {
                        parent.value = Some(value);
                    } else {
                        let edge = usize::from(*nibbles.get(new_at).ok_or_else(|| {
                            IndexError::CorruptObject("missing new map edge".into())
                        })?);
                        let suffix_at = new_at.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                        let suffix = nibbles.get(suffix_at..).ok_or_else(|| {
                            IndexError::CorruptObject("missing new map suffix".into())
                        })?;
                        budget.charge(WorkUsage {
                            bytes: u64::try_from(suffix.len())
                                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                            ..WorkUsage::default()
                        })?;
                        let child =
                            self.write_node(Node::leaf(suffix, value, budget)?, staged, budget)?;
                        let _old = parent.branches.replace(edge, child, budget)?;
                    }
                    break self.write_node(parent, staged, budget)?;
                }
                at = at
                    .checked_add(node.prefix.len())
                    .ok_or(IndexError::ResourceLimited)?;
                if at == nibbles.len() {
                    expected.require(node.value.as_ref())?;
                    node.value = next
                        .map(|value| RetainedMapValue::from_public(value, budget))
                        .transpose()?;
                    break self.write_node(node, staged, budget)?;
                }
                let edge =
                    usize::from(*nibbles.get(at).ok_or_else(|| {
                        IndexError::CorruptObject("missing map branch edge".into())
                    })?);
                at = at.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                let child = node.branches.replace(edge, None, budget)?;
                if child.is_none() {
                    expected.require(None)?;
                    let Some(value) = next else {
                        break self.write_node(node, staged, budget)?;
                    };
                    let value = RetainedMapValue::from_public(value, budget)?;
                    let suffix = nibbles.get(at..).ok_or_else(|| {
                        IndexError::CorruptObject("missing map child suffix".into())
                    })?;
                    budget.charge(WorkUsage {
                        bytes: u64::try_from(suffix.len())
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                        ..WorkUsage::default()
                    })?;
                    let child =
                        self.write_node(Node::leaf(suffix, value, budget)?, staged, budget)?;
                    let _old = node.branches.replace(edge, child, budget)?;
                    break self.write_node(node, staged, budget)?;
                }
                budget.charge(WorkUsage {
                    bytes: u64::try_from(
                        std::mem::size_of::<(Node, usize)>()
                            .checked_add(node.branches.capacity_bytes()?)
                            .ok_or(IndexError::ResourceLimited)?,
                    )
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                trail.push((node, edge), budget)?;
                cursor = child;
            };
        while let Some((mut parent, edge)) = trail.pop() {
            let _old = parent.branches.replace(edge, replacement, budget)?;
            replacement = self.write_node(parent, staged, budget)?;
        }
        Ok(replacement)
    }

    fn close_edit_frame(
        &self,
        frames: &mut MapBuffer<EditFrame>,
        root: &mut MapRoot,
        staged: &StagedMap,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let frame = frames
            .pop()
            .ok_or_else(|| IndexError::BackendViolation("open map edit path is empty".into()))?;
        let replacement = if frame.dirty {
            self.write_node(frame.node, staged, budget)?
        } else {
            Some(Branch::indirect(frame.reference.ok_or_else(|| {
                IndexError::BackendViolation("unchanged map frame lacks physical identity".into())
            })?))
        };
        if let Some(parent) = frames.last_mut() {
            let edge = frame.parent_edge.ok_or_else(|| {
                IndexError::BackendViolation("open map child has no parent edge".into())
            })?;
            if parent.node.branches.get(edge)? != replacement.as_ref() {
                let _old = parent.node.branches.replace(edge, replacement, budget)?;
                parent.dirty = true;
            }
        } else {
            if frame.parent_edge.is_some() {
                return Err(IndexError::BackendViolation(
                    "open map root retained a parent edge".into(),
                ));
            }
            *root = self.materialize_root(replacement, staged, budget)?;
        }
        Ok(())
    }

    fn open_edit_path(
        &self,
        start: OpenPathStart,
        nibbles: &[u8],
        frames: &mut MapBuffer<EditFrame>,
        staged: &StagedMap,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let mut cursor = start.reference;
        let mut offset = start.offset;
        let mut parent_edge = start.parent_edge;
        while let Some(branch) = cursor {
            budget.checkpoint()?;
            if offset > MAX_NIBBLES {
                return Err(IndexError::CorruptObject(
                    "open map edit path exceeds key limit".into(),
                ));
            }
            let reference = match &branch {
                Branch {
                    value: BranchValue::Indirect(reference),
                    ..
                } => Some(*reference),
                Branch {
                    value: BranchValue::Terminal(_) | BranchValue::Packed(_),
                    ..
                } => None,
            };
            let node = branch.into_node(budget, |reference, budget| {
                staged.read_node(self, reference, budget)
            })?;
            let remaining = nibbles.get(offset..).ok_or_else(|| {
                IndexError::CorruptObject("open map edit path passed key end".into())
            })?;
            if !remaining.starts_with(&node.prefix) {
                break;
            }
            let end = offset
                .checked_add(node.prefix.len())
                .ok_or(IndexError::ResourceLimited)?;
            if end > MAX_NIBBLES {
                return Err(IndexError::CorruptObject(
                    "open map edit path exceeds key limit".into(),
                ));
            }
            let child = if end < nibbles.len() {
                let edge = usize::from(*nibbles.get(end).ok_or_else(|| {
                    IndexError::CorruptObject("open map branch edge is missing".into())
                })?);
                Some((edge, copy_branch(node.branches.get(edge)?, budget)?))
            } else {
                None
            };
            budget.charge(WorkUsage {
                bytes: u64::try_from(
                    std::mem::size_of::<EditFrame>()
                        .checked_add(node.branches.capacity_bytes()?)
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            frames.push(
                EditFrame {
                    node,
                    reference,
                    end,
                    parent_edge,
                    dirty: reference.is_none(),
                },
                budget,
            )?;
            let Some((edge, next)) = child else {
                break;
            };
            parent_edge = Some(edge);
            offset = end.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            cursor = next;
        }
        Ok(())
    }

    /// The original-base CAS has already checked every edit. Keep the last
    /// changed path open while consuming sorted edits; a closed sibling can
    /// never be revisited, and untouched child references remain immutable.
    fn apply_sorted_existing_root(
        &self,
        ordinal: u32,
        base: MapRoot,
        staged: &StagedMap,
        budget: &mut WorkBudget,
    ) -> Result<MapRoot, IndexError> {
        let mut root = base;
        let mut frames = MapBuffer::<EditFrame>::new(MAX_NIBBLES + 1);
        let mut previous_nibbles: Option<MapBuffer<u8>> = None;
        let mut edits = staged.scratch.edits(ordinal);
        while let Some(position) = edits.next_edit(budget)? {
            budget.checkpoint()?;
            let expected = position.expected()?;
            let next = position.next()?;
            if expected != next {
                charge_key_allocation(position.key(), budget)?;
                let nibbles = key_nibbles(position.key(), budget)?;
                while frames.last().is_some_and(|frame| {
                    !previous_nibbles.as_ref().is_some_and(|previous| {
                        previous
                            .get(..frame.end)
                            .is_some_and(|prefix| nibbles.starts_with(prefix))
                    })
                }) {
                    self.close_edit_frame(&mut frames, &mut root, staged, budget)?;
                }
                if frames.is_empty() {
                    root = self.update_one(root, &nibbles, next, staged, budget)?;
                    self.open_edit_path(
                        OpenPathStart {
                            reference: root.0.map(Branch::indirect),
                            offset: 0,
                            parent_edge: None,
                        },
                        &nibbles,
                        &mut frames,
                        staged,
                        budget,
                    )?;
                } else {
                    let end = frames
                        .last()
                        .map(|frame| frame.end)
                        .ok_or_else(|| IndexError::BackendViolation("open map path lost".into()))?;
                    if end == nibbles.len() {
                        let frame = frames.last_mut().ok_or_else(|| {
                            IndexError::BackendViolation("open map value path lost".into())
                        })?;
                        frame.node.value = next
                            .map(|value| RetainedMapValue::from_public(value, budget))
                            .transpose()?;
                        frame.dirty = true;
                    } else {
                        let edge = usize::from(*nibbles.get(end).ok_or_else(|| {
                            IndexError::CorruptObject("open map child edge is missing".into())
                        })?);
                        let child = copy_branch(
                            frames
                                .last()
                                .ok_or_else(|| {
                                    IndexError::BackendViolation("open map parent path lost".into())
                                })?
                                .node
                                .branches
                                .get(edge)?,
                            budget,
                        )?;
                        let offset = end.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                        let suffix = nibbles.get(offset..).ok_or_else(|| {
                            IndexError::CorruptObject("open map child suffix is missing".into())
                        })?;
                        let replacement = self.update_child(
                            child,
                            suffix,
                            EditExpectation::AlreadyChecked,
                            next,
                            staged,
                            budget,
                        )?;
                        let frame = frames.last_mut().ok_or_else(|| {
                            IndexError::BackendViolation("open map parent path lost".into())
                        })?;
                        if frame.node.branches.get(edge)? != replacement.as_ref() {
                            let _old = frame.node.branches.replace(edge, replacement, budget)?;
                            frame.dirty = true;
                        }
                        let reference = copy_branch(frame.node.branches.get(edge)?, budget)?;
                        self.open_edit_path(
                            OpenPathStart {
                                reference,
                                offset,
                                parent_edge: Some(edge),
                            },
                            &nibbles,
                            &mut frames,
                            staged,
                            budget,
                        )?;
                    }
                }
                previous_nibbles = Some(nibbles);
            }
        }
        while !frames.is_empty() {
            self.close_edit_frame(&mut frames, &mut root, staged, budget)?;
        }
        Ok(root)
    }
}

impl FileObjectStore {
    /// Empty bases admit each original edit once and build sorted radix paths
    /// directly. The disposable node scratch remains the only staging area;
    /// no edit SQL row or second owned edit is created for replay.
    fn apply_empty_stream(
        &self,
        bases: &[MapRoot],
        edits: &mut dyn MapEditCursor,
        mut roots: quanta_index_core::index::MapRootBuilder,
        packing: RootPackingPlan,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        let mut observation = MapApplyObservation::new(budget);
        let mut shape = MapInputShape::new(observation.span.is_some());
        let diagnostic = observation
            .context
            .as_ref()
            .filter(|_| observation.span.is_some())
            .cloned();
        let result = {
            let mut observed = ShapeObservedCursor {
                input: edits,
                shape: &mut shape,
                diagnostic,
            };
            let mut scoped_budget = budget.scoped_diagnostic_context(observation.child_context());
            let budget = &mut *scoped_budget;
            (|| {
                let mut staged = StagedMap::open(self, budget)?;
                if (0..bases.len()).any(|index| packing.mode(index) == RootPackingMode::Nested) {
                    staged.track_staged_children(budget)?;
                }
                staged.scratch.begin_batch(budget)?;
                let mut stream = cold_stream::EmptyStream::new(&mut observed, bases.len());
                observe_map_phase(budget, 3, |budget| {
                    for index in 0..bases.len() {
                        budget.checkpoint()?;
                        let ordinal =
                            u32::try_from(index).map_err(|_error| IndexError::ResourceLimited)?;
                        staged.packing_mode.set(packing.mode(index));
                        observation.builds += 1;
                        let root = self.build_empty_root_from_cursor(
                            &mut stream.root_cursor(ordinal),
                            &staged,
                            budget,
                            None,
                        )?;
                        roots.push(root)?;
                    }
                    // No root may be returned until the source cursor has reached
                    // EOF, including its late malformed-row and cancellation checks.
                    stream.finish(budget)
                })?;
                let roots = roots.finish()?;
                observe_map_phase(budget, 4, |budget| staged.scratch.commit_batch(budget))?;
                observe_map_phase(budget, 5, |budget| {
                    self.install_reachable_map_nodes(&roots, &staged, sink, budget)
                })?;
                budget.checkpoint()?;
                Ok(roots)
            })()
        };
        observation.finish(&result, shape);
        result
    }
}

impl PersistentMapPort for FileObjectStore {
    fn cold_scan_memory_credit_limits(
        &self,
        max_entries: usize,
    ) -> Result<Option<MemoryCreditLimits>, IndexError> {
        cold_scan_memory_credit_limits(self, max_entries)
    }

    fn scoped_reads<'a>(
        &'a self,
        budget: &WorkBudget,
    ) -> Result<Option<Box<dyn PersistentMapPort + 'a>>, IndexError> {
        Ok(Some(Box::new(scoped_reads::ScopedReads::new(
            self, budget,
        )?)))
    }

    fn visit_diff(
        &self,
        before: MapRoot,
        after: MapRoot,
        range: quanta_index_core::index::ports::MapDiffRange<'_>,
        visitor: &mut dyn quanta_index_core::index::ports::MapDiffVisitor,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        diff::visit(self, before, after, range, visitor, budget)
    }

    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        self.lookup_key(root, key, budget, |reference, budget| {
            self.read_node(reference, budget)
        })
    }

    fn lookup_batch(
        &self,
        root: MapRoot,
        keys: &[&[u8]],
        values: &mut [Option<MapValue>],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        original_lookup::lookup_batch_with(root, keys, values, budget, |reference, budget| {
            self.read_node(reference, budget)
        })
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        Self::scan_with(root, after, limit, budget, |reference, budget| {
            self.read_node(reference, budget)
        })
    }

    fn last_entry(&self, root: MapRoot, budget: &mut WorkBudget) -> Result<MapPage, IndexError> {
        Self::last_entry_with(root, budget, |reference, budget| {
            self.read_node(reference, budget)
        })
    }

    fn try_visit_scan_pages(
        &self,
        root: MapRoot,
        budget: &mut WorkBudget,
        visitor: &mut dyn FnMut(MapPage, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<bool, IndexError> {
        Self::try_visit_scan_pages_with(
            root,
            budget,
            |reference, budget| self.read_node(reference, budget),
            visitor,
        )
    }

    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        if let [edit] = edits
            && edit.key.len() <= resident::MAX_SINGLE_KEY_BYTES
        {
            budget.checkpoint()?;
            budget.charge(WorkUsage {
                mutations: 1,
                bytes: u64::try_from(edit.key.len())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            charge_key_allocation(&edit.key, budget)?;
            let nibbles = key_nibbles(&edit.key, budget)?;
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<MapRoot>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            if edit.expected == edit.next {
                if self.lookup_inner(base, &nibbles, budget)? != edit.expected {
                    return Err(IndexError::Conflict);
                }
                budget.checkpoint()?;
                return Ok(MapWrite { root: base });
            }
            let staged = ResidentStage::new();
            let root =
                self.update_one_checked(base, &nibbles, edit.expected, edit.next, &staged, budget)?;
            self.install_reachable_map_nodes(&[root], &staged, sink, budget)?;
            budget.checkpoint()?;
            return Ok(MapWrite { root });
        }
        let slices = [edits];
        let mut cursor = SliceEditCursor::new(&slices);
        let roots = self.apply_stream(&[base], &mut cursor, sink, budget)?;
        let root = roots
            .into_iter()
            .next()
            .ok_or_else(|| IndexError::BackendViolation("single map output root missing".into()))?;
        Ok(MapWrite { root })
    }

    fn apply_batch(
        &self,
        requests: &[MapBatchRequest],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        if requests.len() > MAX_BATCH_ROOTS {
            return Err(IndexError::ResourceLimited);
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(requests.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?
                .checked_mul(64)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut bases = MapBuffer::new(MAX_BATCH_ROOTS);
        let mut edits = MapBuffer::new(MAX_BATCH_ROOTS);
        for request in requests {
            bases.push(request.base, budget)?;
            edits.push(request.edits.as_slice(), budget)?;
        }
        let mut cursor = SliceEditCursor::new(&edits);
        Ok(MapBatchWrite {
            roots: self.apply_stream(&bases, &mut cursor, sink, budget)?,
        })
    }

    fn apply_stream(
        &self,
        bases: &[MapRoot],
        edits: &mut dyn MapEditCursor,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
        if bases.is_empty()
            || bases.len() > MAX_EMPTY_STREAM_ROOTS
            || (bases.len() > MAX_BATCH_ROOTS && bases.iter().any(|base| base.0.is_some()))
        {
            return Err(IndexError::ResourceLimited);
        }
        let packing = edits.subtree_packing()?;
        packing.validate_for_roots(bases.len())?;
        let mut roots = StagedMapRoots::prepare(bases.len(), budget)?;
        let mut buffered = scalar_pair::Input::new(edits, bases.len() == 2, budget)?;
        if let Some(pair) = buffered.pair()? {
            return self.apply_resident_pair(bases, pair, roots, packing, sink, budget);
        }
        if bases.iter().all(|base| base.0.is_none()) {
            return self.apply_empty_stream(bases, &mut buffered, roots, packing, sink, budget);
        }
        let mut observation = MapApplyObservation::new(budget);
        let mut shape = MapInputShape::new(observation.span.is_some());
        let diagnostic = observation
            .context
            .as_ref()
            .filter(|_| observation.span.is_some())
            .cloned();
        let result = {
            let mut observed = ShapeObservedCursor {
                input: &mut buffered,
                shape: &mut shape,
                diagnostic,
            };
            let mut scoped_budget = budget.scoped_diagnostic_context(observation.child_context());
            let budget = &mut *scoped_budget;
            (|| {
                let edits: &mut dyn MapEditCursor = &mut observed;
                let staged = StagedMap::open(self, budget)?;
                staged.scratch.begin_batch(budget)?;
                let mut candidate_roots = [false; MAX_EMPTY_STREAM_ROOTS];
                for (index, base) in bases.iter().enumerate() {
                    let has_peer = bases.iter().enumerate().any(|(other, candidate)| {
                        other != index
                            && candidate == base
                            && packing.mode(index) == packing.mode(other)
                    });
                    *candidate_roots
                        .get_mut(index)
                        .ok_or(IndexError::ResourceLimited)? = has_peer;
                }
                let reuse_possible = candidate_roots.iter().any(|candidate| *candidate);
                let mut previous: Option<OrdinalMapEdit> = None;
                let mut edit_batch =
                    MapBuffer::new(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS);
                // The fixed root bound keeps identity state on the stack. Fingerprints
                // select candidates only; the original cursor still reaches EOF with
                // every ordinal's ordering and original-base CAS checked below.
                let mut identities = [[0u8; 32]; MAX_EMPTY_STREAM_ROOTS];
                let mut edit_counts = [0u64; MAX_EMPTY_STREAM_ROOTS];
                let mut identity = Sha256::new();
                let mut identity_root = 0usize;
                let mut original_lookup = original_lookup::OriginalLookup::new();
                // Lane 1 includes cursor reads, original-base validation and
                // restaging. Lane 2 isolates the edit scratch writes within it.
                observe_map_phase(budget, 1, |budget| {
                    while let Some(ordinal) = edits.next_edit(budget)? {
                        budget.checkpoint()?;
                        let root_index = usize::try_from(ordinal.root())
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?;
                        let base = *bases.get(root_index).ok_or_else(|| {
                            IndexError::InvalidInput(
                                "map edit root ordinal is out of bounds".into(),
                            )
                        })?;
                        if ordinal.key().len() > MAX_KEY_BYTES
                            || previous.as_ref().is_some_and(|prior| {
                                prior.root() > ordinal.root()
                                    || (prior.root() == ordinal.root()
                                        && prior.key() >= ordinal.key())
                            })
                        {
                            return Err(IndexError::InvalidInput(
                                "map edits must be globally sorted and unique".into(),
                            ));
                        }
                        budget.charge(WorkUsage {
                            mutations: 1,
                            bytes: u64::try_from(ordinal.key().len())
                                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                            ..WorkUsage::default()
                        })?;
                        let original = if base.0.is_none() {
                            None
                        } else {
                            charge_key_allocation(ordinal.key(), budget)?;
                            let nibbles = key_nibbles(ordinal.key(), budget)?;
                            original_lookup.lookup(self, base, &nibbles, budget)?
                        };
                        if original != ordinal.expected()? {
                            return Err(IndexError::Conflict);
                        }
                        if reuse_possible {
                            while identity_root < root_index {
                                let finished: [u8; 32] =
                                    std::mem::take(&mut identity).finalize().into();
                                if *candidate_roots
                                    .get(identity_root)
                                    .ok_or(IndexError::ResourceLimited)?
                                {
                                    *identities
                                        .get_mut(identity_root)
                                        .ok_or(IndexError::ResourceLimited)? = finished;
                                }
                                identity_root = identity_root
                                    .checked_add(1)
                                    .ok_or(IndexError::ResourceLimited)?;
                            }
                            if *candidate_roots
                                .get(root_index)
                                .ok_or(IndexError::ResourceLimited)?
                            {
                                fingerprint_position(&mut identity, &ordinal, budget)?;
                                let count = edit_counts
                                    .get_mut(root_index)
                                    .ok_or(IndexError::ResourceLimited)?;
                                *count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                            }
                        }
                        previous = Some(ordinal.clone());
                        budget.charge(WorkUsage {
                            bytes: u64::try_from(
                                edit_batch
                                    .growth_work_bytes()?
                                    .checked_add(std::mem::size_of::<OrdinalMapEdit>())
                                    .ok_or(IndexError::ResourceLimited)?,
                            )
                            .map_err(|_error| IndexError::ResourceLimited)?,
                            ..WorkUsage::default()
                        })?;
                        edit_batch.push(ordinal, budget)?;
                        if edit_batch.len() == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
                            observe_map_phase(budget, 2, |budget| {
                                staged.scratch.insert_edits(&edit_batch, budget)
                            })?;
                            edit_batch.truncate(0);
                        }
                    }
                    // Validation retains only an authenticated original path. Release its
                    // admitted backing before constructing any replacement DAG.
                    drop(original_lookup);
                    if reuse_possible {
                        while identity_root < bases.len() {
                            let finished: [u8; 32] =
                                std::mem::take(&mut identity).finalize().into();
                            if *candidate_roots
                                .get(identity_root)
                                .ok_or(IndexError::ResourceLimited)?
                            {
                                *identities
                                    .get_mut(identity_root)
                                    .ok_or(IndexError::ResourceLimited)? = finished;
                            }
                            identity_root = identity_root
                                .checked_add(1)
                                .ok_or(IndexError::ResourceLimited)?;
                        }
                    }
                    if !edit_batch.is_empty() {
                        observe_map_phase(budget, 2, |budget| {
                            staged.scratch.insert_edits(&edit_batch, budget)
                        })?;
                    }
                    drop(edit_batch);
                    Ok(())
                })?;
                budget.charge(WorkUsage {
                    bytes: u64::try_from(
                        bases
                            .len()
                            .checked_mul(std::mem::size_of::<MapRoot>())
                            .ok_or(IndexError::ResourceLimited)?,
                    )
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                let mut built_roots = [MapRoot(None); MAX_EMPTY_STREAM_ROOTS];
                let mut verified_scratch = [false; MAX_EMPTY_STREAM_ROOTS];
                // Lane 3 is scratch edit replay plus radix construction.
                observe_map_phase(budget, 3, |budget| {
                    for (index, base) in bases.iter().enumerate() {
                        budget.checkpoint()?;
                        let ordinal = u32::try_from(index)
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?;
                        let pack = packing.mode(index);
                        staged.packing_mode.set(pack);
                        let has_peer = *candidate_roots
                            .get(index)
                            .ok_or(IndexError::ResourceLimited)?;
                        let matches_identity = |other| {
                            bases.get(other) == Some(base)
                                && pack == packing.mode(other)
                                && edit_counts.get(other) == edit_counts.get(index)
                                && identities.get(other) == identities.get(index)
                        };
                        let prior = if has_peer {
                            (0..index).find(|&prior| matches_identity(prior))
                        } else {
                            None
                        };
                        let root = if let Some(prior) = prior {
                            observation.candidates += 1;
                            // A matching input digest alone does not authorize skipping a
                            // scratch cursor. Decode both actual rows through EOF, including
                            // all shape and wire checks, before sharing a built root.
                            if !*verified_scratch
                                .get(prior)
                                .ok_or(IndexError::ResourceLimited)?
                            {
                                let prior_ordinal = u32::try_from(prior)
                                    .map_err(|_error| IndexError::ResourceLimited)?;
                                let prior_identity =
                                    *identities.get(prior).ok_or(IndexError::ResourceLimited)?;
                                let prior_count =
                                    *edit_counts.get(prior).ok_or(IndexError::ResourceLimited)?;
                                if scratch_edit_fingerprint(&staged.scratch, prior_ordinal, budget)?
                                    != (prior_identity, prior_count)
                                {
                                    return Err(IndexError::CorruptObject(
                                        "scratch map edits changed after validation".into(),
                                    ));
                                }
                                *verified_scratch
                                    .get_mut(prior)
                                    .ok_or(IndexError::ResourceLimited)? = true;
                            }
                            let current_identity =
                                *identities.get(index).ok_or(IndexError::ResourceLimited)?;
                            let current_count =
                                *edit_counts.get(index).ok_or(IndexError::ResourceLimited)?;
                            if scratch_edit_fingerprint(&staged.scratch, ordinal, budget)?
                                != (current_identity, current_count)
                            {
                                return Err(IndexError::CorruptObject(
                                    "scratch map edits changed after validation".into(),
                                ));
                            }
                            *verified_scratch
                                .get_mut(index)
                                .ok_or(IndexError::ResourceLimited)? = true;
                            let reused =
                                *built_roots.get(prior).ok_or(IndexError::ResourceLimited)?;
                            observation.reuses += 1;
                            reused
                        } else {
                            observation.builds += 1;
                            if base.0.is_none() {
                                let has_later_alias =
                                    has_peer && (index + 1..bases.len()).any(matches_identity);
                                let verify = if has_later_alias {
                                    Some((
                                        *identities
                                            .get(index)
                                            .ok_or(IndexError::ResourceLimited)?,
                                        *edit_counts
                                            .get(index)
                                            .ok_or(IndexError::ResourceLimited)?,
                                    ))
                                } else {
                                    None
                                };
                                let root =
                                    self.build_empty_root(ordinal, &staged, budget, verify)?;
                                if verify.is_some() {
                                    *verified_scratch
                                        .get_mut(index)
                                        .ok_or(IndexError::ResourceLimited)? = true;
                                }
                                root
                            } else {
                                self.apply_sorted_existing_root(ordinal, *base, &staged, budget)?
                            }
                        };
                        *built_roots
                            .get_mut(index)
                            .ok_or(IndexError::ResourceLimited)? = root;
                        roots.push(root)?;
                    }
                    Ok(())
                })?;
                let roots = roots.finish()?;
                observe_map_phase(budget, 4, |budget| staged.scratch.commit_batch(budget))?;
                // Lane 5 includes reachable installation and its physical children.
                observe_map_phase(budget, 5, |budget| {
                    self.install_reachable_map_nodes(&roots, &staged, sink, budget)
                })?;
                budget.checkpoint()?;
                Ok(roots)
            })()
        };
        observation.finish(&result, shape);
        result
    }
}

struct SliceEditCursor<'a> {
    requests: &'a [&'a [MapEdit]],
    root: usize,
    edit: usize,
}

impl<'a> SliceEditCursor<'a> {
    fn new(requests: &'a [&'a [MapEdit]]) -> Self {
        Self {
            requests,
            root: 0,
            edit: 0,
        }
    }
}

impl MapEditCursor for SliceEditCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        while let Some(request) = self.requests.get(self.root) {
            if let Some(edit) = request.get(self.edit) {
                budget.charge(WorkUsage {
                    bytes: u64::try_from(edit.key.len())
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?
                        .checked_add(128)
                        .ok_or(IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                let root = u32::try_from(self.root)
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?;
                self.edit = self
                    .edit
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                return Ok(Some(OrdinalMapEdit::copy_key(
                    root,
                    &edit.key,
                    edit.expected,
                    edit.next,
                    budget,
                )?));
            }
            self.root = self
                .root
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            self.edit = 0;
        }
        Ok(None)
    }
}

fn subtree_can_follow(prefix: &[u8], after: Option<&[u8]>) -> bool {
    after.is_none_or(|after| prefix > after || after.starts_with(prefix))
}

fn nibbles_to_key(nibbles: &[u8]) -> Result<Vec<u8>, IndexError> {
    if !nibbles.len().is_multiple_of(2) || nibbles.len() > MAX_NIBBLES {
        return Err(IndexError::CorruptObject("invalid map key path".into()));
    }
    let mut key = Vec::new();
    key.try_reserve_exact(nibbles.len() >> 1)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for pair in nibbles.chunks_exact(2) {
        let high = *pair
            .first()
            .ok_or_else(|| IndexError::CorruptObject("missing high nibble".into()))?;
        let low = *pair
            .get(1)
            .ok_or_else(|| IndexError::CorruptObject("missing low nibble".into()))?;
        if high > 15 || low > 15 {
            return Err(IndexError::CorruptObject("invalid map key nibble".into()));
        }
        key.push((high << 4) | low);
    }
    Ok(key)
}

fn key_nibbles(key: &[u8], budget: &WorkBudget) -> Result<MapBuffer<u8>, IndexError> {
    if key.len() > MAX_KEY_BYTES {
        return Err(IndexError::InvalidInput(
            "map key exceeds physical limit".into(),
        ));
    }
    let capacity = key
        .len()
        .checked_mul(2)
        .ok_or(IndexError::ResourceLimited)?;
    let mut nibbles = MapBuffer::with_capacity(capacity, capacity, budget)?;
    for byte in key {
        nibbles.push(byte >> 4, budget)?;
        nibbles.push(byte & 0x0f, budget)?;
    }
    Ok(nibbles)
}

fn charge_key_allocation(key: &[u8], budget: &mut WorkBudget) -> Result<(), IndexError> {
    if key.len() > MAX_KEY_BYTES {
        return Err(IndexError::InvalidInput(
            "map key exceeds physical limit".into(),
        ));
    }
    budget.charge(WorkUsage {
        bytes: u64::try_from(
            key.len()
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })
}

fn lookup_nibbles(
    root: MapRoot,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<Option<MapBuffer<u8>>, IndexError> {
    // Empty roots still enforce the original key bound and logical charge.
    charge_key_allocation(key, budget)?;
    if root.0.is_none() {
        return Ok(None);
    }
    key_nibbles(key, budget).map(Some)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        path::PathBuf,
        sync::{
            Mutex,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{RegisteredObject, WorkLimits, ports::ImmutableObjectPort};

    use super::*;

    fn assertion_error(detail: String) -> Box<dyn std::error::Error> {
        std::io::Error::other(detail).into()
    }

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(assertion_error(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        }};
        ($condition:expr, $($message:tt)+) => {{
            if !$condition {
                return Err(assertion_error(format!($($message)+)));
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
        ($left:expr, $right:expr, $($message:tt)+) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(assertion_error(format!(
                    "assertion failed: left {left:?} != right {right:?}: {}",
                    format_args!($($message)+)
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

    mod completed_group;
    mod inline_terminals;
    mod input_shape_observation;
    mod original_lookup;
    mod packed_subtrees;
    mod resident_single;
    mod retained_value_cas;
    mod scoped_reads;
    mod sparse_branches;
    mod structural_diff;

    #[cfg(unix)]
    mod scratch_transactions;

    #[test]
    fn map_phase_work_delta_preserves_off_cost_and_closes_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        use quanta_index_core::{
            DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticEmitV1, DiagnosticEventKindV1,
            DiagnosticEventSinkV1, DiagnosticRootKindV1, RequestBudgetV1,
        };
        use std::sync::Arc;

        #[derive(Debug, Default)]
        struct StepClock(AtomicU64);
        impl DiagnosticClockV1 for StepClock {
            fn now_ns(&self) -> u64 {
                self.0.fetch_add(1, Ordering::Relaxed)
            }
        }

        #[derive(Debug, Default)]
        struct Events(Mutex<Vec<DiagnosticEmitV1>>);
        impl DiagnosticEventSinkV1 for Events {
            fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
                let Ok(mut rows) = self.0.try_lock() else {
                    return false;
                };
                rows.push(event);
                true
            }
        }

        struct Run {
            result: Result<(), IndexError>,
            usage: WorkUsage,
            clock_calls: u64,
            events: Vec<DiagnosticEmitV1>,
        }

        fn run(record: bool, behavior: u8) -> Result<Run, Box<dyn std::error::Error>> {
            let clock = Arc::new(StepClock::default());
            let events = Arc::new(Events::default());
            let context = DiagnosticContextV1::new(
                DiagnosticCaptureIdV1([73; 16]),
                Arc::clone(&clock) as Arc<dyn DiagnosticClockV1>,
                Arc::clone(&events) as Arc<dyn DiagnosticEventSinkV1>,
            );
            let root = if record {
                Some(context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?)
            } else {
                None
            };
            let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
            let cancel = request.cancel_handle();
            let request = match root.as_ref() {
                Some(root) => request.with_diagnostic_context(context.child_context(root)),
                None => request,
            };
            let mut budget = WorkBudget::with_request(
                WorkLimits {
                    reads: 10,
                    writes: 10,
                    bytes: 10,
                    mutations: 10,
                },
                request,
            );
            let result = observe_map_phase(&mut budget, 5, |budget| {
                budget.charge(WorkUsage {
                    reads: 2,
                    writes: 3,
                    bytes: 5,
                    mutations: 7,
                })?;
                match behavior {
                    0 => Ok(()),
                    1 => Err(IndexError::InvalidInput(
                        "injected map phase failure".into(),
                    )),
                    2 => {
                        cancel.cancel();
                        budget.checkpoint()
                    }
                    _ => Err(IndexError::BackendViolation("invalid test behavior".into())),
                }
            });
            let usage = budget.usage();
            if let Some(root) = root {
                let _finished = root.finish(DiagnosticTerminalV1::from_result(&result));
            }
            let captured = events
                .0
                .lock()
                .map_err(|_| assertion_error("map phase event sink poisoned".into()))?
                .clone();
            Ok(Run {
                result,
                usage,
                clock_calls: clock.0.load(Ordering::Relaxed),
                events: captured,
            })
        }

        for behavior in 0..=2 {
            let off = run(false, behavior)?;
            let recorded = run(true, behavior)?;
            check_eq!(off.usage, recorded.usage);
            check_eq!(off.clock_calls, 0);
            check!(off.events.is_empty());
            check_eq!(
                recorded.usage,
                WorkUsage {
                    reads: 2,
                    writes: 3,
                    bytes: 5,
                    mutations: 7,
                }
            );
            match behavior {
                0 => check!(off.result.is_ok() && recorded.result.is_ok()),
                1 => check!(
                    matches!(off.result, Err(IndexError::InvalidInput(_)))
                        && matches!(recorded.result, Err(IndexError::InvalidInput(_)))
                ),
                _ => check!(
                    matches!(off.result, Err(IndexError::Cancelled))
                        && matches!(recorded.result, Err(IndexError::Cancelled))
                ),
            }
            let spans = recorded
                .events
                .iter()
                .filter_map(|event| match event.kind {
                    DiagnosticEventKindV1::SpanBegin {
                        span_id,
                        stage: DiagnosticStageV1::MapApply,
                        lane: DiagnosticLaneV1(5),
                        ..
                    } => Some(span_id),
                    _ => None,
                })
                .collect::<Vec<_>>();
            check_eq!(spans.len(), 1);
            let span = spans[0];
            let end = recorded
                .events
                .iter()
                .position(|event| {
                    matches!(event.kind, DiagnosticEventKindV1::SpanEnd { span_id, .. } if span_id == span)
                })
                .ok_or_else(|| assertion_error("map phase terminal missing".into()))?;
            for (key, expected) in [
                (DiagnosticCounterV1::WorkReads, 2),
                (DiagnosticCounterV1::WorkWrites, 3),
                (DiagnosticCounterV1::WorkBytes, 5),
                (DiagnosticCounterV1::WorkMutations, 7),
            ] {
                let matching = recorded
                    .events
                    .iter()
                    .enumerate()
                    .filter_map(|(index, event)| match event.kind {
                        DiagnosticEventKindV1::Counter {
                            span_id,
                            key: observed,
                            value,
                        } if span_id == span && observed == key => Some((index, value)),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                check_eq!(matching.len(), 1);
                check_eq!(matching[0].1, expected);
                check!(matching[0].0 < end);
            }
            let expected_terminal = if behavior == 0 {
                DiagnosticTerminalV1::Ok
            } else {
                DiagnosticTerminalV1::Error { code: 1 }
            };
            check_eq!(
                recorded
                    .events
                    .iter()
                    .filter(|event| {
                        matches!(event.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome } if span_id == span && outcome == expected_terminal)
                    })
                    .count(),
                1
            );
        }
        Ok(())
    }

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    #[derive(Default)]
    struct RecordingSink(Mutex<BTreeMap<ObjectRef, bool>>, AtomicU64, AtomicU64);

    impl ObjectWriteSink for RecordingSink {
        fn reserve(&self, object: &RegisteredObject) -> Result<(), IndexError> {
            let mut registrations = self
                .0
                .lock()
                .map_err(|_conversion_error| IndexError::Storage("test sink poisoned".into()))?;
            let _prior = registrations.insert(object.reference, false);
            drop(registrations);
            Ok(())
        }

        fn installed(&self, object: &RegisteredObject) -> Result<(), IndexError> {
            let mut registrations = self
                .0
                .lock()
                .map_err(|_conversion_error| IndexError::Storage("test sink poisoned".into()))?;
            let state = registrations.get_mut(&object.reference).ok_or_else(|| {
                IndexError::BackendViolation("installed before reservation".into())
            })?;
            *state = true;
            drop(registrations);
            Ok(())
        }

        fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
            let _previous = self.1.fetch_add(1, Ordering::Relaxed);
            for object in objects {
                self.reserve(object)?;
            }
            Ok(())
        }

        fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
            let _previous = self.2.fetch_add(1, Ordering::Relaxed);
            for object in objects {
                self.installed(object)?;
            }
            Ok(())
        }
    }

    struct RejectingSink;

    struct ArrayAdmission {
        lifetime: MemoryLifetime,
        bytes: Option<u64>,
        refused: AtomicU64,
        live: AtomicU64,
    }
    impl quanta_index_core::index::MemoryLeaseOwner for ArrayAdmission {
        fn release(&self, bytes: u64) {
            let _prior = self.live.fetch_sub(bytes, Ordering::Relaxed);
        }
    }
    struct ArrayScope(std::sync::Arc<ArrayAdmission>);
    impl quanta_index_core::index::MemoryScopePort for ArrayScope {
        fn reserve(
            &self,
            bytes: u64,
            lifetime: MemoryLifetime,
        ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
            if lifetime == self.0.lifetime && self.0.bytes.is_none_or(|denied| denied == bytes) {
                let _prior = self.0.refused.fetch_add(1, Ordering::Relaxed);
                return Err(IndexError::ResourceLimited);
            }
            let _prior = self.0.live.fetch_add(bytes, Ordering::Relaxed);
            Ok(quanta_index_core::index::MemoryLease::new(
                bytes,
                self.0.clone(),
            ))
        }
    }
    fn array_scope(
        lifetime: MemoryLifetime,
        bytes: Option<u64>,
        work: &mut WorkBudget,
    ) -> std::sync::Arc<ArrayAdmission> {
        let tracker = std::sync::Arc::new(ArrayAdmission {
            lifetime,
            bytes,
            refused: AtomicU64::new(0),
            live: AtomicU64::new(0),
        });
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(
            std::sync::Arc::new(ArrayScope(tracker.clone())),
        )));
        tracker
    }

    #[test]
    fn input_and_output_array_refusal_precede_cursor_scratch_or_install()
    -> Result<(), Box<dyn std::error::Error>> {
        struct UnexpectedCursor;
        impl MapEditCursor for UnexpectedCursor {
            fn next_edit(
                &mut self,
                _: &mut WorkBudget,
            ) -> Result<Option<OrdinalMapEdit>, IndexError> {
                Err(IndexError::BackendViolation(
                    "unfunded map consumed cursor".into(),
                ))
            }
        }
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        for lifetime in [MemoryLifetime::Temporary, MemoryLifetime::Retained] {
            let sink = RecordingSink::default();
            let mut work = budget()?;
            let tracker = array_scope(lifetime, None, &mut work);
            let outcome = if lifetime == MemoryLifetime::Temporary {
                store
                    .apply_batch(
                        &[MapBatchRequest {
                            base: MapRoot(None),
                            edits: vec![],
                        }],
                        &sink,
                        &mut work,
                    )
                    .map(|_output| ())
            } else {
                store
                    .apply_stream(&[MapRoot(None)], &mut UnexpectedCursor, &sink, &mut work)
                    .map(|_output| ())
            };
            check!(matches!(outcome, Err(IndexError::ResourceLimited)));
            check_eq!(tracker.refused.load(Ordering::Relaxed), 1);
            check_eq!(tracker.live.load(Ordering::Relaxed), 0);
            check_eq!(work.usage().reads, 0);
            check_eq!(work.usage().writes, 0);
            check_eq!(work.usage().mutations, 0);
            check_eq!(sink.1.load(Ordering::Relaxed), 0);
            check_eq!(sink.2.load(Ordering::Relaxed), 0);
        }
        Ok(())
    }

    #[test]
    fn reachable_install_admits_one_existing_root_with_34_work_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut setup = budget()?;
        let reference = value(&store, b"existing-map-dependency", &mut setup)?;
        let staged = ResidentStage::new();
        let sink = RecordingSink::default();
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 1,
                writes: 0,
                bytes: 34,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        store.install_reachable_map_nodes(
            &[MapRoot(Some(reference))],
            &staged,
            &sink,
            &mut work,
        )?;
        check_eq!(work.usage().bytes, 34);
        check_eq!(work.usage().reads, 1);
        check_eq!(sink.1.load(Ordering::Relaxed), 0);
        check_eq!(sink.2.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn install_reference_array_refusal_preserves_chunk_and_precedes_custody()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let object = store.prepare_object(b"independent-install-payload", &[], &mut work)?;
        let reference = object.registration.reference;
        let mut chunk = MapBuffer::new(MAX_INSTALL_BATCH_INPUTS);
        chunk.push(object, &work)?;
        let tracker = array_scope(
            MemoryLifetime::Temporary,
            Some(u64::try_from(std::mem::size_of::<&PreparedObject>())?),
            &mut work,
        );
        let before_install_calls = std::cell::Cell::new(0_u8);
        check!(matches!(
            flush_reachable_chunk(&store, &mut chunk, &sink, &mut work, |_objects, _budget| {
                before_install_calls.set(1);
                Err(IndexError::Storage(
                    "commit callback ran before reference-array admission".into(),
                ))
            }),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(chunk.len(), 1);
        check_eq!(before_install_calls.get(), 0);
        check_eq!(tracker.refused.load(Ordering::Relaxed), 1);
        check_eq!(tracker.live.load(Ordering::Relaxed), 0);
        check_eq!(sink.1.load(Ordering::Relaxed), 0);
        check_eq!(sink.2.load(Ordering::Relaxed), 0);
        work.bind_memory_scope(None);
        flush_reachable_chunk(&store, &mut chunk, &sink, &mut work, |_objects, _budget| {
            before_install_calls.set(1);
            Ok(())
        })?;
        check_eq!(before_install_calls.get(), 1);
        check!(chunk.is_empty());
        check_eq!(sink.1.load(Ordering::Relaxed), 1);
        check_eq!(sink.2.load(Ordering::Relaxed), 1);
        let installed = store.get(reference, &mut work)?;
        check_eq!(installed.payload.as_slice(), b"independent-install-payload");
        Ok(())
    }

    struct TestEditCursor(std::vec::IntoIter<OrdinalMapEdit>);

    impl MapEditCursor for TestEditCursor {
        fn next_edit(
            &mut self,
            budget: &mut WorkBudget,
        ) -> Result<Option<OrdinalMapEdit>, IndexError> {
            budget.checkpoint()?;
            Ok(self.0.next())
        }
    }

    #[test]
    fn empty_stream_validates_rows_without_edit_sql_replay()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let rows = (0u16..512)
            .map(|number| {
                OrdinalMapEdit::unscoped(
                    0,
                    MapEdit::objects(number.to_be_bytes().to_vec(), None, None),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut cursor = TestEditCursor(rows.into_iter());
        let roots = store.apply_stream(&[MapRoot(None)], &mut cursor, &sink, &mut work)?;
        check_eq!(roots.len(), 1);
        check_eq!(*roots.first().ok_or("empty root missing")?, MapRoot(None));
        check_eq!(work.usage().mutations, 512);
        // A per-row edit INSERT and replay SELECT would each exceed this bound.
        // No map nodes are produced by absent-to-absent edits.
        check!(work.usage().reads < 64);
        check!(work.usage().writes < 64);
        check_eq!(sink.1.load(Ordering::Relaxed), 0);
        check_eq!(sink.2.load(Ordering::Relaxed), 0);
        assert_clean_scratch(&directory)?;
        Ok(())
    }

    #[test]
    fn wide_empty_stream_preserves_ordinals_and_refuses_nonempty_base()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let value = store
            .put(b"wide empty stream value", &[], &mut work)?
            .reference;
        let mut bases = [MapRoot(None); MAX_BATCH_ROOTS + 1];
        let edits = (0..bases.len())
            .map(|ordinal| {
                OrdinalMapEdit::unscoped(
                    u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?,
                    MapEdit::objects(
                        vec![u8::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?],
                        None,
                        Some(value),
                    ),
                )
            })
            .collect::<Result<Vec<_>, IndexError>>()?;
        let mut cursor = TestEditCursor(edits.into_iter());
        let roots = store.apply_stream(&bases, &mut cursor, &sink, &mut work)?;
        check_eq!(roots.len(), bases.len());
        for (ordinal, root) in roots.iter().enumerate() {
            check_eq!(
                store.lookup(*root, &[u8::try_from(ordinal)?], &mut work)?,
                Some(value.into())
            );
        }

        bases[0] = *roots.first().ok_or("wide empty stream returned no roots")?;
        let mut cursor = TestEditCursor(Vec::new().into_iter());
        check!(matches!(
            store.apply_stream(&bases, &mut cursor, &sink, &mut work),
            Err(IndexError::ResourceLimited)
        ));
        Ok(())
    }

    impl ObjectWriteSink for RejectingSink {
        fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::ResourceLimited)
        }
        fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::ResourceLimited)
        }
        fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            Err(IndexError::ResourceLimited)
        }
        fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            Err(IndexError::ResourceLimited)
        }
    }

    struct BoundedCustodySink {
        state: Mutex<BoundedCustodyState>,
        reject_reservation: Option<usize>,
    }

    struct BoundedCustodyState {
        registered: BTreeSet<ObjectRef>,
        reserve_attempts: usize,
        installed_groups: Vec<Vec<ObjectRef>>,
    }

    impl BoundedCustodySink {
        fn new(external_child: ObjectRef, reject_reservation: Option<usize>) -> Self {
            Self {
                state: Mutex::new(BoundedCustodyState {
                    registered: BTreeSet::from([external_child]),
                    reserve_attempts: 0,
                    installed_groups: Vec::new(),
                }),
                reject_reservation,
            }
        }
    }

    impl ObjectWriteSink for BoundedCustodySink {
        fn reserve(&self, object: &RegisteredObject) -> Result<(), IndexError> {
            self.reserve_batch(std::slice::from_ref(object))
        }

        fn installed(&self, object: &RegisteredObject) -> Result<(), IndexError> {
            self.installed_batch(std::slice::from_ref(object))
        }

        fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
            let mut state = self
                .state
                .lock()
                .map_err(|_error| IndexError::Storage("test sink poisoned".into()))?;
            state.reserve_attempts = state
                .reserve_attempts
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if self.reject_reservation == Some(state.reserve_attempts) {
                return Err(IndexError::ResourceLimited);
            }
            let bytes = objects.iter().try_fold(0u64, |total, object| {
                total
                    .checked_add(object.bytes)
                    .ok_or(IndexError::ResourceLimited)
            })?;
            if objects.len() > 256 || bytes > MAX_BATCH_BYTES {
                return Err(IndexError::BackendViolation(
                    "unbounded map custody group".into(),
                ));
            }
            let mut earlier_in_group = BTreeSet::new();
            for object in objects {
                for child in &object.children {
                    if !state.registered.contains(child) && !earlier_in_group.contains(child) {
                        return Err(IndexError::BackendViolation(
                            "map child was not registered before parent".into(),
                        ));
                    }
                }
                let _was_new = earlier_in_group.insert(object.reference);
            }
            Ok(())
        }

        fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
            let mut state = self
                .state
                .lock()
                .map_err(|_error| IndexError::Storage("test sink poisoned".into()))?;
            let group = objects.iter().map(|object| object.reference).collect();
            for object in objects {
                let _was_new = state.registered.insert(object.reference);
            }
            state.installed_groups.push(group);
            drop(state);
            Ok(())
        }
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let sequence = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "qidx-native-storage-{}-{sequence}",
                std::process::id(),
            ));
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 256 * 1024 * 1024,
                mutations: 100_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[test]
    fn empty_scalar_lookup_keeps_key_admission_without_temporary_backing()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let key = b"repo0/src/file.rs";
        let charged_bytes = 2 * u64::try_from(key.len())?;
        let mut work = budget()?;
        let admission = array_scope(MemoryLifetime::Temporary, None, &mut work);
        check_eq!(store.lookup(MapRoot(None), key, &mut work)?, None);
        check_eq!(admission.refused.load(Ordering::Relaxed), 0);
        check_eq!(work.usage().bytes, charged_bytes);
        check_eq!(work.usage().reads, 0);

        let too_long = [0; MAX_KEY_BYTES + 1];
        check!(matches!(
            store.lookup(MapRoot(None), &too_long, &mut work),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(work.usage().bytes, charged_bytes);

        work.request().cancel_handle().cancel();
        check!(matches!(
            store.lookup(MapRoot(None), key, &mut work),
            Err(IndexError::Cancelled)
        ));
        check_eq!(work.usage().bytes, charged_bytes);
        Ok(())
    }

    fn value(
        store: &FileObjectStore,
        bytes: &[u8],
        work: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        store
            .put(bytes, &[], work)
            .map(|registration| registration.reference)
    }

    fn physical_file_count(directory: &TestDir) -> Result<usize, std::io::Error> {
        let mut count = 0usize;
        for shard in fs::read_dir(directory.0.join("objects"))? {
            for object in fs::read_dir(shard?.path())? {
                if object?.file_type()?.is_file() {
                    count = count
                        .checked_add(1)
                        .ok_or_else(|| std::io::Error::other("physical object count overflow"))?;
                }
            }
        }
        Ok(count)
    }

    fn assert_clean_scratch(directory: &TestDir) -> Result<(), Box<dyn std::error::Error>> {
        let scratch = directory.0.join("scratch");
        let mut entries = fs::read_dir(&scratch)?.collect::<Result<Vec<_>, _>>()?;
        check_eq!(
            entries.len(),
            1,
            "scratch allocations or unexpected files remain"
        );
        let lease = entries.pop().ok_or("scratch lease is missing")?;
        check_eq!(lease.path(), scratch.join(".lease"));
        let metadata = fs::symlink_metadata(lease.path())?;
        check!(metadata.file_type().is_file());
        check_eq!(metadata.permissions().mode() & 0o7777, 0o600);
        Ok(())
    }

    #[test]
    fn storage_rejects_public_and_symlinked_paths() -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let reference = value(&store, b"private", &mut work)?;
        let shard = store.path_for(reference);
        let object_metadata = fs::metadata(&shard)?;
        check_eq!(object_metadata.permissions().mode() & 0o7777, 0o600);
        check_eq!(
            fs::metadata(&directory.0)?.permissions().mode() & 0o7777,
            0o700
        );
        check_eq!(
            fs::metadata(directory.0.join("objects"))?
                .permissions()
                .mode()
                & 0o7777,
            0o700
        );
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755))?;
        check!(FileObjectStore::open(&directory.0).is_err());
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700))?;

        let link = TestDir::new();
        symlink(&directory.0, &link.0)?;
        check!(FileObjectStore::open(&link.0).is_err());
        fs::remove_file(&link.0)?;

        let linked_shard = TestDir::new();
        let linked_store = FileObjectStore::open(&linked_shard.0)?;
        let mut linked_work = budget()?;
        let prepared = linked_store.prepare_object(b"shard", &[], &mut linked_work)?;
        let shard_path = linked_store.path_for(prepared.registration.reference);
        let parent = shard_path.parent().ok_or("missing shard")?;
        symlink(directory.0.join("objects"), parent)?;
        check!(linked_store.put(b"shard", &[], &mut linked_work).is_err());
        Ok(())
    }

    #[test]
    fn exact_edit_delete_and_historical_roots_survive_reopen()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let first = value(&store, b"first", &mut work)?;
        let second = value(&store, b"second", &mut work)?;
        let third = value(&store, b"third", &mut work)?;
        let keys = [vec![], vec![0x10], vec![0x10, 0x20], vec![0x11], vec![0x20]];
        let mut model = BTreeMap::new();
        let mut root = MapRoot(None);
        let mut history = Vec::new();
        for (index, key) in keys.iter().enumerate() {
            let next = if index % 2 == 0 { first } else { second };
            let result = store.apply(
                root,
                &[MapEdit {
                    key: key.clone(),
                    expected: None,
                    next: Some((next).into()),
                }],
                &sink,
                &mut work,
            )?;
            check!(result.root.0.is_some());
            root = result.root;
            let _previous = model.insert(key.clone(), next);
            history.push((root, model.clone()));
        }

        let stale = store.apply(
            root,
            &[MapEdit {
                key: vec![0x10],
                expected: None,
                next: Some((third).into()),
            }],
            &sink,
            &mut work,
        );
        check!(matches!(stale, Err(IndexError::Conflict)));
        let before_delete = sink.0.lock().map_err(|_error| "test sink poisoned")?.len();
        let deleted = store.apply(
            root,
            &[MapEdit {
                key: vec![0x10],
                expected: Some((second).into()),
                next: None,
            }],
            &sink,
            &mut work,
        )?;
        let after_delete = sink.0.lock().map_err(|_error| "test sink poisoned")?.len();
        check!(
            after_delete
                .checked_sub(before_delete)
                .ok_or("test custody count decreased")?
                <= 8,
            "one-key edit rewrote too many nodes"
        );
        root = deleted.root;
        let _removed = model.remove(&vec![0x10]);
        let updated = store.apply(
            root,
            &[MapEdit {
                key: vec![0x11],
                expected: Some((second).into()),
                next: Some((third).into()),
            }],
            &sink,
            &mut work,
        )?;
        root = updated.root;
        let _previous = model.insert(vec![0x11], third);
        check!(
            sink.0
                .lock()
                .is_ok_and(|registrations| registrations.values().all(|installed| *installed))
        );
        check_eq!(sink.1.load(Ordering::Relaxed), 7);
        check_eq!(sink.2.load(Ordering::Relaxed), 7);
        drop(store);

        let reopened = FileObjectStore::open(&directory.0)?;
        for (historical_root, historical_model) in history {
            for key in &keys {
                check_eq!(
                    reopened.lookup_object(historical_root, key, &mut work)?,
                    historical_model.get(key).copied()
                );
            }
        }
        for key in &keys {
            check_eq!(
                reopened.lookup_object(root, key, &mut work)?,
                model.get(key).copied()
            );
        }
        Ok(())
    }

    #[test]
    fn batch_preconditions_are_checked_against_base_before_writes()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let first = value(&store, b"one", &mut work)?;
        let result = store.apply(
            MapRoot(None),
            &[MapEdit {
                key: b"b".to_vec(),
                expected: None,
                next: Some((first).into()),
            }],
            &sink,
            &mut work,
        )?;
        let base = result.root;
        let failed = store.apply(
            base,
            &[
                MapEdit {
                    key: b"a".to_vec(),
                    expected: None,
                    next: Some((first).into()),
                },
                MapEdit {
                    key: b"b".to_vec(),
                    expected: None,
                    next: None,
                },
            ],
            &sink,
            &mut work,
        );
        check!(matches!(failed, Err(IndexError::Conflict)));
        check_eq!(store.lookup_object(base, b"a", &mut work)?, None);
        check_eq!(store.lookup_object(base, b"b", &mut work)?, Some(first));

        let duplicate = store.apply(
            base,
            &[
                MapEdit {
                    key: b"a".to_vec(),
                    expected: None,
                    next: Some((first).into()),
                },
                MapEdit {
                    key: b"a".to_vec(),
                    expected: None,
                    next: Some((first).into()),
                },
            ],
            &sink,
            &mut work,
        );
        check!(matches!(duplicate, Err(IndexError::InvalidInput(_))));
        Ok(())
    }

    #[test]
    fn pages_match_an_independent_ordered_map_and_late_scan_prunes()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let value = value(&store, b"shared", &mut work)?;
        let mut root = MapRoot(None);
        let mut model = BTreeMap::new();
        for byte in 0u8..16 {
            let key = vec![byte];
            root = store
                .apply(
                    root,
                    &[MapEdit {
                        key: key.clone(),
                        expected: None,
                        next: Some((value).into()),
                    }],
                    &sink,
                    &mut work,
                )?
                .root;
            let _previous = model.insert(key, value);
        }

        let mut actual = Vec::new();
        let mut after: Option<Vec<u8>> = None;
        loop {
            let page = store.scan(root, after.as_deref(), 3, &mut work)?;
            check!(page.entries.len() <= 3);
            if page.has_more {
                check_eq!(page.entries.len(), 3);
            }
            after = page.entries.last().map(|(key, _)| key.clone());
            actual.extend(page.entries.iter().cloned());
            if !page.has_more {
                break;
            }
        }
        check_eq!(
            actual,
            model
                .into_iter()
                .map(|(key, value)| (key, value.into()))
                .collect::<Vec<_>>()
        );

        let before = work.usage().reads;
        let late = store.scan(root, Some(&[12]), 2, &mut work)?;
        check_eq!(
            late.entries
                .iter()
                .map(|(key, _)| key.as_slice())
                .collect::<Vec<_>>(),
            vec![&[13][..], &[14][..]]
        );
        check!(late.has_more);
        check!(
            work.usage().reads - before <= 6,
            "late page visited unrelated subtrees"
        );
        Ok(())
    }

    #[test]
    fn descriptor_scan_releases_a_deep_branch_before_a_shallow_tail()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut setup = budget()?;
        let child = value(&store, b"descriptor-child", &mut setup)?;
        let descriptor = MapValue::inline(&[1, 0, 1], [child])?;
        let keys = [vec![0; 512], vec![1], vec![2], vec![3]];
        let edits = keys.map(|key| MapEdit {
            key,
            expected: None,
            next: Some(descriptor),
        });
        let root = store.apply(MapRoot(None), &edits, &sink, &mut setup)?.root;

        let mut scalar = budget()?;
        let mut rows = Vec::new();
        let mut after = None;
        loop {
            let page = store.scan(root, after.as_deref(), 1, &mut scalar)?;
            after = page.entries.last().map(|(key, _)| key.clone());
            rows.extend(page.entries.iter().cloned());
            if !page.has_more {
                break;
            }
        }

        let mut visited = budget()?;
        let grant = array_scope(MemoryLifetime::Temporary, Some(0), &mut visited);
        let mut actual = Vec::new();
        check!(
            store.try_visit_scan_pages(root, &mut visited, &mut |page, _budget| {
                actual.extend(page.entries.iter().cloned());
                Ok(())
            })?
        );
        check_eq!(actual, rows);
        check_eq!(visited.usage(), scalar.usage());
        check_eq!(visited.first_denial(), None);
        check_eq!(grant.live.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn cold_scan_credit_counts_values_beyond_one_root_batch()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let at_64 = cold_scan_memory_credit_limits(&store, 64)?.ok_or("64-value credit absent")?;
        let at_65 = cold_scan_memory_credit_limits(&store, 65)?.ok_or("65-value credit absent")?;
        let at_121 =
            cold_scan_memory_credit_limits(&store, 121)?.ok_or("121-value credit absent")?;
        check!(at_65.temporary_data_bytes > at_64.temporary_data_bytes);
        check!(at_121.temporary_data_bytes > at_65.temporary_data_bytes);
        check!(at_121.max_live_temporary_leases > at_65.max_live_temporary_leases);
        Ok(())
    }

    #[test]
    fn descriptor_scan_continuation_preserves_pages_work_and_refunds()
    -> Result<(), Box<dyn std::error::Error>> {
        use quanta_index_core::index::WorkBudgetDenial;

        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut setup = budget()?;
        let child = value(&store, b"descriptor-child", &mut setup)?;
        let descriptor = MapValue::inline(&[1, 0, 1], [child])?;
        let mut root = MapRoot(None);
        let mut expected = Vec::new();
        for ordinal in 0_u8..32 {
            let key = vec![0, b't', ordinal];
            root = store
                .apply(
                    root,
                    &[MapEdit {
                        key: key.clone(),
                        expected: None,
                        next: Some(descriptor),
                    }],
                    &sink,
                    &mut setup,
                )?
                .root;
            expected.push((key, descriptor));
        }

        let mut scalar = budget()?;
        let scalar_grant = array_scope(MemoryLifetime::Temporary, Some(0), &mut scalar);
        let mut after = None;
        let mut scalar_rows = Vec::new();
        let mut scalar_pages = 0_usize;
        loop {
            let page = store.scan(root, after.as_deref(), 1, &mut scalar)?;
            check_eq!(page.entries.len(), 1);
            after = page.entries.last().map(|(key, _)| key.clone());
            scalar_rows.extend(page.entries.iter().cloned());
            scalar_pages += 1;
            if !page.has_more {
                break;
            }
        }
        check_eq!(scalar_rows, expected);
        check_eq!(scalar_pages, 32);
        check_eq!(scalar_grant.live.load(Ordering::Relaxed), 0);

        let mut visited = budget()?;
        let visit_grant = array_scope(MemoryLifetime::Temporary, Some(0), &mut visited);
        let mut visited_rows = Vec::new();
        let mut visited_pages = 0_usize;
        check!(
            store.try_visit_scan_pages(root, &mut visited, &mut |page, _budget| {
                visited_pages += 1;
                if page.entries.len() != 1 || page.has_more != (visited_pages < 32) {
                    return Err(IndexError::BackendViolation(
                        "descriptor continuation page shape differs".into(),
                    ));
                }
                visited_rows.extend(page.entries.iter().cloned());
                Ok(())
            },)?
        );
        check_eq!(visited_rows, expected);
        check_eq!(visited_pages, 32);
        check_eq!(visited.usage().writes, 0);
        check_eq!(visited.usage().mutations, 0);
        check!(visited.usage().reads < scalar.usage().reads);
        check!(visited.usage().bytes < scalar.usage().bytes);
        check_eq!(visit_grant.live.load(Ordering::Relaxed), 0);

        let mut scoped_scalar = budget()?;
        let scoped_scalar_grant =
            array_scope(MemoryLifetime::Temporary, Some(0), &mut scoped_scalar);
        let scalar_maps = store
            .scoped_reads(&scoped_scalar)?
            .ok_or("scoped map reader absent")?;
        let mut after = None;
        let mut scoped_scalar_rows = Vec::new();
        loop {
            let page = scalar_maps.scan(root, after.as_deref(), 1, &mut scoped_scalar)?;
            after = page.entries.last().map(|(key, _)| key.clone());
            scoped_scalar_rows.extend(page.entries.iter().cloned());
            if !page.has_more {
                break;
            }
        }
        drop(scalar_maps);
        check_eq!(scoped_scalar_rows, expected);
        check_eq!(scoped_scalar_grant.live.load(Ordering::Relaxed), 0);

        let mut scoped_visited = budget()?;
        let scoped_visited_grant =
            array_scope(MemoryLifetime::Temporary, Some(0), &mut scoped_visited);
        let visited_maps = store
            .scoped_reads(&scoped_visited)?
            .ok_or("scoped map reader absent")?;
        let mut scoped_rows = Vec::new();
        check!(visited_maps.try_visit_scan_pages(
            root,
            &mut scoped_visited,
            &mut |page, _budget| {
                scoped_rows.extend(page.entries.iter().cloned());
                Ok(())
            },
        )?);
        drop(visited_maps);
        check_eq!(scoped_rows, expected);
        check!(scoped_visited.usage().reads < scoped_scalar.usage().reads);
        check!(scoped_visited.usage().bytes < scoped_scalar.usage().bytes);
        check_eq!(scoped_visited_grant.live.load(Ordering::Relaxed), 0);

        struct EvictionAdmission {
            box_bytes: u64,
            box_live: AtomicU64,
            refused_child: AtomicU64,
            callbacks: AtomicU64,
            live: AtomicU64,
        }
        impl quanta_index_core::index::MemoryLeaseOwner for EvictionAdmission {
            fn release(&self, bytes: u64) {
                let _prior = self.live.fetch_sub(bytes, Ordering::Relaxed);
                if bytes == self.box_bytes {
                    self.box_live.store(0, Ordering::Relaxed);
                }
            }
        }
        struct EvictionScope(Arc<EvictionAdmission>);
        impl quanta_index_core::index::MemoryScopePort for EvictionScope {
            fn reserve(
                &self,
                bytes: u64,
                lifetime: MemoryLifetime,
            ) -> Result<MemoryLease, IndexError> {
                if lifetime == MemoryLifetime::Temporary && bytes == 777 {
                    if self.0.box_live.load(Ordering::Relaxed) != 0 {
                        let _prior = self.0.refused_child.fetch_add(1, Ordering::Relaxed);
                        return Err(IndexError::ResourceLimited);
                    }
                }
                if lifetime == MemoryLifetime::Temporary && bytes == self.0.box_bytes {
                    self.0.box_live.store(1, Ordering::Relaxed);
                }
                let _prior = self.0.live.fetch_add(bytes, Ordering::Relaxed);
                Ok(MemoryLease::new(bytes, self.0.clone()))
            }
        }
        let box_bytes = u64::try_from(std::mem::size_of::<ScanSuspend>())?;
        let eviction = Arc::new(EvictionAdmission {
            box_bytes,
            box_live: AtomicU64::new(0),
            refused_child: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
            live: AtomicU64::new(0),
        });
        let mut evicted_work = budget()?;
        evicted_work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            EvictionScope(Arc::clone(&eviction)),
        ))));
        let mut evicted_rows = Vec::new();
        let mut one_page_live = 0;
        check!(
            store.try_visit_scan_pages(root, &mut evicted_work, &mut |page, budget| {
                let prior = eviction.callbacks.fetch_add(1, Ordering::Relaxed);
                if prior == 0 {
                    let child = budget.reserve_memory(777, MemoryLifetime::Temporary)?;
                    drop(child);
                    one_page_live = eviction.live.load(Ordering::Relaxed);
                } else if eviction.live.load(Ordering::Relaxed) != one_page_live {
                    return Err(IndexError::BackendViolation(
                        "scalar visit retained a previous descriptor page".into(),
                    ));
                }
                evicted_rows.extend(page.entries.iter().cloned());
                Ok(())
            })?
        );
        check_eq!(evicted_rows, expected);
        check_eq!(eviction.callbacks.load(Ordering::Relaxed), 32);
        check_eq!(eviction.refused_child.load(Ordering::Relaxed), 1);
        check_eq!(eviction.live.load(Ordering::Relaxed), 0);
        check_eq!(evicted_work.first_denial(), None);
        check_eq!(evicted_work.usage(), scalar.usage());

        let mut first_work = budget()?;
        let first = store.scan(root, Some(&[0, b't', 29]), 1, &mut first_work)?;
        let mut resumed_work = budget()?;
        let resumed_grant = array_scope(MemoryLifetime::Temporary, Some(0), &mut resumed_work);
        let mut reader = |reference, budget: &mut WorkBudget| store.read_node(reference, budget);
        let mut scan = ScanContinuation::new(
            root,
            Some(&[0, b't', 29]),
            1,
            &mut resumed_work,
            &mut reader,
        )?;
        let page = scan
            .next_page(&mut resumed_work, &mut reader)?
            .ok_or("missing first resumed page")?;
        check_eq!(page, first);
        check_eq!(resumed_work.usage(), first_work.usage());
        drop(page);
        resumed_work.request().cancel_handle().cancel();
        let before_cancel = resumed_work.usage();
        check!(matches!(
            scan.next_page(&mut resumed_work, &mut reader),
            Err(IndexError::Cancelled)
        ));
        check_eq!(resumed_work.usage(), before_cancel);
        check_eq!(
            resumed_work.first_denial(),
            Some(WorkBudgetDenial::Cancelled)
        );
        check!(matches!(
            scan.next_page(&mut resumed_work, &mut reader),
            Err(IndexError::BackendViolation(_))
        ));
        drop(scan);
        check_eq!(resumed_grant.live.load(Ordering::Relaxed), 0);

        let mut eof_work = budget()?;
        let mut reader = |reference, budget: &mut WorkBudget| store.read_node(reference, budget);
        let mut scan =
            ScanContinuation::new(root, Some(&[0, b't', 31]), 1, &mut eof_work, &mut reader)?;
        let empty = scan
            .next_page(&mut eof_work, &mut reader)?
            .ok_or("missing EOF page")?;
        check!(empty.entries.is_empty());
        check!(!empty.has_more);
        let before_eof = eof_work.usage();
        check!(scan.next_page(&mut eof_work, &mut reader)?.is_none());
        check_eq!(eof_work.usage(), before_eof);

        let mut denied = budget()?;
        let capacity = MapPage::allocation_bytes(8)?;
        let grant = array_scope(MemoryLifetime::Retained, Some(capacity), &mut denied);
        check!(matches!(
            store.scan(root, None, 8, &mut denied),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(denied.usage(), WorkUsage::default());
        check_eq!(denied.first_denial(), Some(WorkBudgetDenial::Memory));
        check_eq!(grant.refused.load(Ordering::Relaxed), 1);
        check_eq!(grant.live.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn scan_rejects_hash_valid_paths_beyond_the_key_limit() -> Result<(), Box<dyn std::error::Error>>
    {
        fn raw_node(prefix: &[u8], value: bool, branches: u16) -> Result<Vec<u8>, IndexError> {
            let mut payload = Vec::new();
            payload.extend_from_slice(NODE_MAGIC);
            payload.extend_from_slice(
                &u16::try_from(prefix.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?
                    .to_be_bytes(),
            );
            payload.extend_from_slice(prefix);
            payload.push(u8::from(value));
            payload.extend_from_slice(&branches.to_be_bytes());
            payload.extend_from_slice(&0_u16.to_be_bytes());
            if value {
                payload.push(0);
            }
            Ok(payload)
        }

        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let value = value(&store, b"value", &mut work)?;
        let long_prefix = vec![0; MAX_NIBBLES];

        // The branch of this otherwise valid root exceeds the canonical key
        // length before a child path may be allocated or traversed.
        let overlong_branch = store
            .put(
                &raw_node(&long_prefix, true, 1)?,
                &[value, value],
                &mut work,
            )?
            .reference;
        check!(matches!(
            store.scan(MapRoot(Some(overlong_branch)), None, 2, &mut work),
            Err(IndexError::CorruptObject(_))
        ));

        // Each node prefix is individually within the limit, but the parent
        // edge plus child prefix exceeds it. Both nodes have valid digests.
        let child = store
            .put(&raw_node(&long_prefix, true, 0)?, &[value], &mut work)?
            .reference;
        let parent = store
            .put(&raw_node(&[], true, 1)?, &[value, child], &mut work)?
            .reference;
        check!(matches!(
            store.scan(MapRoot(Some(parent)), None, 2, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    #[test]
    fn object_hash_binds_ordered_children_and_read_detects_corruption()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let first = value(&store, b"one", &mut work)?;
        let second = value(&store, b"two", &mut work)?;
        let left = store.put(b"parent", &[first, second], &mut work)?;
        let right = store.put(b"parent", &[second, first], &mut work)?;
        check_ne!(left.reference, right.reference);
        check_eq!(
            store.get(left.reference, &mut work)?.registration.children,
            vec![first, second]
        );
        check_eq!(
            store.put(b"parent", &[first, second], &mut work)?.reference,
            left.reference
        );
        fs::write(store.path_for(left.reference), b"broken")?;
        check!(matches!(
            store.get(left.reference, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    #[test]
    fn budget_refusal_precedes_read_or_mutation() -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut no_work = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        check!(matches!(
            store.apply(
                MapRoot(None),
                &[MapEdit {
                    key: b"a".to_vec(),
                    expected: None,
                    next: None
                }],
                &sink,
                &mut no_work,
            ),
            Err(IndexError::ResourceLimited)
        ));
        Ok(())
    }

    #[test]
    fn failed_batch_reservation_installs_no_map_object() -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let value = value(&store, b"value", &mut work)?;
        let before = physical_file_count(&directory)?;
        let result = store.apply(
            MapRoot(None),
            &[MapEdit {
                key: b"key".to_vec(),
                expected: None,
                next: Some((value).into()),
            }],
            &RejectingSink,
            &mut work,
        );
        check!(matches!(result, Err(IndexError::ResourceLimited)));
        check_eq!(physical_file_count(&directory)?, before);
        Ok(())
    }

    #[test]
    fn reachable_child_consumption_fits_read_bound_and_refuses_corrupt_wire()
    -> Result<(), Box<dyn std::error::Error>> {
        for corrupt in [false, true] {
            let directory = TestDir::new();
            let store = FileObjectStore::open(&directory.0)?;
            let mut preparing = budget()?;
            let left_value = value(&store, b"left", &mut preparing)?;
            let right_value = value(&store, b"right", &mut preparing)?;
            let _memory = array_scope(MemoryLifetime::Temporary, Some(0), &mut preparing);
            let mut staged = StagedMap::open(&store, &mut preparing)?;
            staged.track_staged_children(&mut preparing)?;
            check!(staged.staged_children.is_some());
            let left = store.write_encoded(
                &Node::leaf(&[0], left_value.into(), &preparing)?,
                &staged,
                &mut preparing,
            )?;
            let right = store.write_encoded(
                &Node::leaf(&[0], right_value.into(), &preparing)?,
                &staged,
                &mut preparing,
            )?;
            let root = store.write_encoded(
                &Node {
                    prefix: MapBuffer::new(0),
                    value: None,
                    branches: {
                        let mut branches = Branches::new();
                        let _old =
                            branches.replace(0, Some(Branch::indirect(left)), &mut preparing)?;
                        let _old =
                            branches.replace(1, Some(Branch::indirect(right)), &mut preparing)?;
                        branches
                    },
                },
                &staged,
                &mut preparing,
            )?;
            if corrupt {
                let paths = fs::read_dir(directory.0.join("scratch"))?
                    .map(|entry| entry.map(|entry| entry.path().join("scratch.sqlite")))
                    .collect::<Result<Vec<_>, _>>()?;
                let path = paths
                    .iter()
                    .find(|path| path.is_file())
                    .ok_or("scratch database missing")?;
                let connection = rusqlite::Connection::open(path)?;
                check_eq!(
                    connection.execute(
                        "UPDATE node SET wire=zeroblob(length(wire)) WHERE reference=?1",
                        rusqlite::params![(right.0).0.as_slice()]
                    )?,
                    1
                );
            }
            let before = physical_file_count(&directory)?;
            // Sixteen read-work units are the regression ceiling for this
            // child-first DAG, not a requirement to spend the whole allowance.
            let mut work = WorkBudget::new(
                WorkLimits {
                    reads: 16,
                    writes: 100_000,
                    bytes: 256 * 1024 * 1024,
                    mutations: 100_000,
                },
                Instant::now()
                    .checked_add(Duration::from_secs(30))
                    .ok_or(IndexError::ResourceLimited)?,
            );
            let result = store.install_reachable_map_nodes(
                &[MapRoot(Some(root))],
                &staged,
                &RecordingSink::default(),
                &mut work,
            );
            if corrupt {
                check!(matches!(result, Err(IndexError::CorruptObject(_))));
                check_eq!(physical_file_count(&directory)?, before);
            } else {
                result?;
                check_eq!(
                    store.lookup_object(MapRoot(Some(root)), &[0], &mut preparing)?,
                    Some(left_value)
                );
                check_eq!(
                    store.lookup_object(MapRoot(Some(root)), &[16], &mut preparing)?,
                    Some(right_value)
                );
            }
        }
        Ok(())
    }

    #[test]
    fn final_reachable_map_dag_installs_in_bounded_child_first_groups()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let value = value(&store, b"value", &mut work)?;
        // Track every valid backing allocation without imposing another limit.
        let memory = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);
        let edits: Vec<_> = (0_u16..512)
            .map(|key| {
                let [high, low] = key.to_be_bytes();
                // A 140-byte tail stays indirect under the persisted256-nibble
                // terminal envelope, so this fixture still spans three groups.
                let mut key = vec![high, low, 0xff, high, 0xa5, low];
                key.extend_from_slice(&[0x5a; 140]);
                MapEdit {
                    key,
                    expected: None,
                    next: Some((value).into()),
                }
            })
            .collect();
        let sink = BoundedCustodySink::new(value, None);
        let output = store.apply(MapRoot(None), &edits, &sink, &mut work)?;
        let state = sink.state.lock().map_err(|_error| "test sink poisoned")?;
        check!(state.installed_groups.len() >= 3);
        check_eq!(state.reserve_attempts, state.installed_groups.len());
        check!(state.installed_groups.iter().map(Vec::len).sum::<usize>() > 512);
        drop(state);
        for edit in &edits {
            check_eq!(
                store.lookup_object(output.root, &edit.key, &mut work)?,
                Some(value)
            );
        }
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn later_reachable_group_reservation_failure_does_not_return_a_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let value = value(&store, b"value", &mut work)?;
        // Track every valid backing allocation without imposing another limit.
        let memory = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);
        let edits: Vec<_> = (0_u16..512)
            .map(|key| {
                let [high, low] = key.to_be_bytes();
                // A 140-byte tail stays indirect under the persisted256-nibble
                // terminal envelope, so this fixture still spans three groups.
                let mut key = vec![high, low, 0xff, high, 0xa5, low];
                key.extend_from_slice(&[0x5a; 140]);
                MapEdit {
                    key,
                    expected: None,
                    next: Some((value).into()),
                }
            })
            .collect();
        let before = physical_file_count(&directory)?;
        let sink = BoundedCustodySink::new(value, Some(2));
        check!(matches!(
            store.apply(MapRoot(None), &edits, &sink, &mut work),
            Err(IndexError::ResourceLimited)
        ));
        let state = sink.state.lock().map_err(|_error| "test sink poisoned")?;
        check_eq!(state.reserve_attempts, 2);
        check_eq!(state.installed_groups.len(), 1);
        check!(
            state
                .installed_groups
                .first()
                .ok_or("first custody group missing")?
                .iter()
                .all(|reference| state.registered.contains(reference))
        );
        drop(state);
        check!(physical_file_count(&directory)? > before);
        check_eq!(
            store.lookup_object(MapRoot(None), &[0, 0, 0xff, 0, 0xa5, 0], &mut work)?,
            None
        );
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn multi_root_batch_matches_independent_models_and_keeps_history()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let values = [
            value(&store, b"one", &mut work)?,
            value(&store, b"two", &mut work)?,
            value(&store, b"three", &mut work)?,
        ];
        let mut initial_roots = StagedMapRoots::prepare(3, &work)?;
        for _ordinal in 0..3 {
            initial_roots.push(MapRoot(None))?;
        }
        let mut roots = initial_roots.finish()?;
        let mut models = vec![BTreeMap::<Vec<u8>, ObjectRef>::new(); 3];
        let mut history = Vec::new();
        for step in 0u8..9 {
            let mut requests = Vec::new();
            for (index, (root, model)) in roots.iter().zip(&models).enumerate() {
                let key = vec![b'a' + u8::try_from(index)?, step % 4];
                let expected = model.get(&key).copied();
                let next = Some(
                    *values
                        .get((usize::from(step) + index) % values.len())
                        .ok_or("value missing")?,
                );
                requests.push(MapBatchRequest {
                    base: *root,
                    edits: vec![MapEdit {
                        key,
                        expected: expected.map(Into::into),
                        next: next.map(Into::into),
                    }],
                });
            }
            let output = store.apply_batch(&requests, &sink, &mut work)?;
            check_eq!(output.roots.len(), models.len());
            for ((request, model), root) in requests.iter().zip(&mut models).zip(&output.roots) {
                let edit = request.edits.first().ok_or("edit missing")?;
                let next = edit.next.ok_or("next missing")?;
                let _old = model.insert(edit.key.clone(), next.object_ref()?);
                for (key, value) in model {
                    check_eq!(store.lookup_object(*root, key, &mut work)?, Some(*value));
                }
            }
            history.push((output.roots.clone(), models.clone()));
            roots = output.roots;
        }
        check_eq!(sink.1.load(Ordering::Relaxed), 9);
        check_eq!(sink.2.load(Ordering::Relaxed), 9);
        for (old_roots, old_models) in history {
            for (root, model) in old_roots.into_iter().zip(old_models) {
                for (key, value) in model {
                    check_eq!(store.lookup_object(root, &key, &mut work)?, Some(value));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn identical_roots_share_physical_nodes_in_one_custody_batch()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let object = value(&store, b"same", &mut work)?;
        let request = MapBatchRequest {
            base: MapRoot(None),
            edits: vec![MapEdit {
                key: b"key".to_vec(),
                expected: None,
                next: Some((object).into()),
            }],
        };
        let output = store.apply_batch(&[request.clone(), request], &sink, &mut work)?;
        check_eq!(output.roots.first(), output.roots.get(1));
        check_eq!(
            sink.0.lock().map_err(|_error| "test sink poisoned")?.len(),
            1
        );
        check_eq!(sink.1.load(Ordering::Relaxed), 1);
        check_eq!(sink.2.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn streamed_multi_root_edits_match_independent_models_and_clean_scratch()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let value = value(&store, b"streamed", &mut work)?;
        let mut edits = Vec::new();
        let mut models = [BTreeMap::new(), BTreeMap::new()];
        for root in 0u32..2 {
            for number in 0u16..192 {
                let key = number.to_be_bytes().to_vec();
                edits.push(OrdinalMapEdit::unscoped(
                    root,
                    MapEdit {
                        key: key.clone(),
                        expected: None,
                        next: Some((value).into()),
                    },
                )?);
                let model = models
                    .get_mut(usize::try_from(root)?)
                    .ok_or("model root missing")?;
                let _prior = model.insert(key, value);
            }
        }
        let mut cursor = TestEditCursor(edits.into_iter());
        let roots = store.apply_stream(
            &[MapRoot(None), MapRoot(None)],
            &mut cursor,
            &sink,
            &mut work,
        )?;
        check_eq!(roots.len(), models.len());
        for (root, model) in roots.into_iter().zip(models) {
            for (key, expected) in model {
                check_eq!(store.lookup_object(root, &key, &mut work)?, Some(expected));
            }
        }
        assert_clean_scratch(&directory)?;
        Ok(())
    }

    #[test]
    fn spooled_empty_root_refuses_changed_edit_scratch_before_install()
    -> Result<(), Box<dyn std::error::Error>> {
        // Mixed-base batches still stage edits and authenticate the replayed
        // empty-root cursor before any physical installation.
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let target = value(&store, b"target", &mut work)?;
        let physical_before = physical_file_count(&directory)?;
        let staged = StagedMap::open(&store, &mut work)?;
        staged.scratch.begin_batch(&work)?;
        let edits = [b"a".as_slice(), b"z".as_slice()]
            .into_iter()
            .map(|key| {
                OrdinalMapEdit::unscoped(
                    0,
                    MapEdit {
                        key: key.to_vec(),
                        expected: None,
                        next: Some(target.into()),
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        staged.scratch.insert_edits(&edits, &mut work)?;
        let failure = store.build_empty_root(0, &staged, &mut work, Some(([0; 32], 2)));
        check!(matches!(failure, Err(IndexError::CorruptObject(_))));
        check_eq!(physical_file_count(&directory)?, physical_before);
        let empty_fingerprint: [u8; 32] = Sha256::new().finalize().into();
        let empty = store.build_empty_root(1, &staged, &mut work, Some((empty_fingerprint, 0)))?;
        check_eq!(empty, MapRoot(None));
        drop(staged);
        assert_clean_scratch(&directory)?;
        Ok(())
    }

    #[test]
    fn identical_stream_roots_reuse_build_but_distinct_values_do_not()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut setup = budget()?;
        let first = value(&store, b"first", &mut setup)?;
        let second = value(&store, b"second", &mut setup)?;
        let edits = |different_last: bool| -> Result<Vec<OrdinalMapEdit>, IndexError> {
            (0u32..2)
                .flat_map(|root| {
                    (0u16..192).map(move |number| {
                        let next = if different_last && root == 1 && number == 191 {
                            second
                        } else {
                            first
                        };
                        OrdinalMapEdit::unscoped(
                            root,
                            MapEdit {
                                key: number.to_be_bytes().to_vec(),
                                expected: None,
                                next: Some(next.into()),
                            },
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        };
        let mut reused_work = budget()?;
        let reused = store.apply_stream(
            &[MapRoot(None), MapRoot(None)],
            &mut TestEditCursor(edits(false)?.into_iter()),
            &sink,
            &mut reused_work,
        )?;
        check_eq!(
            *reused.first().ok_or("test root absent")?,
            *reused.get(1).ok_or("test root absent")?
        );
        let reused_writes = reused_work.usage().writes;
        for number in 0u16..192 {
            check_eq!(
                store.lookup_object(
                    *reused.get(1).ok_or("test root absent")?,
                    &number.to_be_bytes(),
                    &mut setup
                )?,
                Some(first)
            );
        }

        let mut distinct_work = budget()?;
        let distinct = store.apply_stream(
            &[MapRoot(None), MapRoot(None)],
            &mut TestEditCursor(edits(true)?.into_iter()),
            &sink,
            &mut distinct_work,
        )?;
        check_ne!(
            *distinct.first().ok_or("test root absent")?,
            *distinct.get(1).ok_or("test root absent")?
        );
        check!(reused_writes < distinct_work.usage().writes);
        check_eq!(
            store.lookup_object(
                *distinct.first().ok_or("test root absent")?,
                &191u16.to_be_bytes(),
                &mut setup
            )?,
            Some(first)
        );
        check_eq!(
            store.lookup_object(
                *distinct.get(1).ok_or("test root absent")?,
                &191u16.to_be_bytes(),
                &mut setup
            )?,
            Some(second)
        );
        assert_clean_scratch(&directory)?;
        Ok(())
    }

    #[test]
    fn wide_empty_alias_batch_compares_complete_values_before_install()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let first = value(&store, b"first", &mut work)?;
        let last = value(&store, b"last", &mut work)?;
        let before = work.usage();
        let edits = (0u32..8)
            .flat_map(|root| {
                (0u16..65).map(move |number| {
                    let next = if root == 7 && number == 64 {
                        last
                    } else {
                        first
                    };
                    OrdinalMapEdit::unscoped(
                        root,
                        MapEdit::objects(number.to_be_bytes().to_vec(), None, Some(next)),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut cursor = TestEditCursor(edits.into_iter());
        let roots = store.apply_stream(&[MapRoot(None); 8], &mut cursor, &sink, &mut work)?;
        check_eq!(roots.len(), 8);
        check_eq!(work.usage().mutations - before.mutations, 8 * 65);
        let canonical = *roots.first().ok_or("first root missing")?;
        for index in 1..7 {
            check_eq!(*roots.get(index).ok_or("alias root missing")?, canonical);
        }
        let changed = *roots.get(7).ok_or("changed root missing")?;
        check_ne!(changed, canonical);
        check_eq!(
            store.lookup_object(canonical, &64u16.to_be_bytes(), &mut work)?,
            Some(first)
        );
        check_eq!(
            store.lookup_object(changed, &64u16.to_be_bytes(), &mut work)?,
            Some(last)
        );
        assert_clean_scratch(&directory)?;
        Ok(())
    }

    #[test]
    fn identical_edits_on_different_bases_and_noops_keep_their_own_results()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let old = value(&store, b"old", &mut work)?;
        let added = value(&store, b"added", &mut work)?;
        let base = store
            .apply(
                MapRoot(None),
                &[MapEdit::objects(b"old".to_vec(), None, Some(old))],
                &sink,
                &mut work,
            )?
            .root;
        let edit = MapEdit::objects(b"new".to_vec(), None, Some(added));
        let roots = store.apply_batch(
            &[
                MapBatchRequest {
                    base: MapRoot(None),
                    edits: vec![edit.clone()],
                },
                MapBatchRequest {
                    base,
                    edits: vec![edit.clone()],
                },
                MapBatchRequest {
                    base: MapRoot(None),
                    edits: vec![edit],
                },
            ],
            &sink,
            &mut work,
        )?;
        check_ne!(
            *roots.roots.first().ok_or("test root absent")?,
            *roots.roots.get(1).ok_or("test root absent")?
        );
        check_eq!(
            *roots.roots.first().ok_or("test root absent")?,
            *roots.roots.get(2).ok_or("test root absent")?
        );
        check_eq!(
            store.lookup_object(
                *roots.roots.first().ok_or("test root absent")?,
                b"old",
                &mut work
            )?,
            None
        );
        check_eq!(
            store.lookup_object(
                *roots.roots.get(1).ok_or("test root absent")?,
                b"old",
                &mut work
            )?,
            Some(old)
        );
        check_eq!(
            store.lookup_object(
                *roots.roots.get(2).ok_or("test root absent")?,
                b"old",
                &mut work
            )?,
            None
        );
        for root in roots.roots.as_slice() {
            check_eq!(store.lookup_object(*root, b"new", &mut work)?, Some(added));
        }
        let noop = MapEdit::objects(b"old".to_vec(), Some(old), Some(old));
        let unchanged = store.apply_batch(
            &[
                MapBatchRequest {
                    base,
                    edits: vec![noop.clone()],
                },
                MapBatchRequest {
                    base,
                    edits: vec![noop],
                },
            ],
            &sink,
            &mut work,
        )?;
        check_eq!(unchanged.roots.as_slice(), &[base, base]);
        assert_clean_scratch(&directory)?;
        Ok(())
    }

    #[test]
    fn duplicate_root_candidate_cannot_skip_late_source_cursor_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        struct LateFailure<I>(I);
        impl<I: Iterator<Item = OrdinalMapEdit>> MapEditCursor for LateFailure<I> {
            fn next_edit(
                &mut self,
                budget: &mut WorkBudget,
            ) -> Result<Option<OrdinalMapEdit>, IndexError> {
                budget.checkpoint()?;
                self.0.next().map_or_else(
                    || {
                        Err(IndexError::InvalidInput(
                            "late source cursor failure".into(),
                        ))
                    },
                    |edit| Ok(Some(edit)),
                )
            }
        }

        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let object = value(&store, b"source", &mut work)?;
        let before = physical_file_count(&directory)?;
        let edits = (0u32..2)
            .flat_map(|root| {
                (0u8..65).map(move |key| {
                    OrdinalMapEdit::unscoped(root, MapEdit::objects(vec![key], None, Some(object)))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut cursor = LateFailure(edits.into_iter());
        check!(matches!(
            store.apply_stream(
                &[MapRoot(None), MapRoot(None)],
                &mut cursor,
                &sink,
                &mut work,
            ),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(sink.1.load(Ordering::Relaxed), 0);
        check_eq!(physical_file_count(&directory)?, before);
        assert_clean_scratch(&directory)?;
        Ok(())
    }

    #[test]
    fn sorted_bootstrap_matches_sparse_canonical_tree_across_prefix_splits()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let object = value(&store, b"bootstrap", &mut work)?;
        let keys = [
            vec![],
            vec![0],
            vec![0, 0],
            vec![0, 1],
            vec![0x10],
            vec![0x10, 0],
            vec![0x11],
            vec![0xff],
            vec![0xff; MAX_KEY_BYTES],
        ];
        let edits = keys
            .iter()
            .map(|key| MapEdit {
                key: key.clone(),
                expected: None,
                next: Some((object).into()),
            })
            .collect::<Vec<_>>();
        let built = store.apply(MapRoot(None), &edits, &sink, &mut work)?.root;
        let mut sparse = MapRoot(None);
        for edit in &edits {
            sparse = store
                .apply(sparse, std::slice::from_ref(edit), &sink, &mut work)?
                .root;
        }
        check_eq!(built, sparse);
        for key in &keys {
            check_eq!(store.lookup_object(built, key, &mut work)?, Some(object));
        }
        assert_clean_scratch(&directory)?;
        Ok(())
    }

    #[test]
    fn sorted_existing_roots_match_serial_updates_and_independent_models()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let first = value(&store, b"first", &mut work)?;
        let second = value(&store, b"second", &mut work)?;
        let third = value(&store, b"third", &mut work)?;
        let initial_keys = [
            b"".to_vec(),
            b"a".to_vec(),
            b"aa".to_vec(),
            b"ab".to_vec(),
            b"b".to_vec(),
            b"ba".to_vec(),
            b"c".to_vec(),
            b"z".to_vec(),
        ];
        let initial = initial_keys
            .iter()
            .map(|key| MapEdit {
                key: key.clone(),
                expected: None,
                next: Some((first).into()),
            })
            .collect::<Vec<_>>();
        let base = store.apply(MapRoot(None), &initial, &sink, &mut work)?.root;
        let baseline = initial_keys
            .iter()
            .map(|key| (key.clone(), first))
            .collect::<BTreeMap<_, _>>();
        let requests = [
            MapBatchRequest {
                base,
                edits: vec![
                    MapEdit {
                        key: b"".to_vec(),
                        expected: Some((first).into()),
                        next: None,
                    },
                    MapEdit {
                        key: b"a".to_vec(),
                        expected: Some((first).into()),
                        next: Some((second).into()),
                    },
                    MapEdit {
                        key: b"a\0".to_vec(),
                        expected: None,
                        next: Some((third).into()),
                    },
                    MapEdit {
                        key: b"aa".to_vec(),
                        expected: Some((first).into()),
                        next: None,
                    },
                    MapEdit {
                        key: b"aaa".to_vec(),
                        expected: None,
                        next: Some((second).into()),
                    },
                    MapEdit {
                        key: b"b".to_vec(),
                        expected: Some((first).into()),
                        next: Some((third).into()),
                    },
                    MapEdit {
                        key: b"ba".to_vec(),
                        expected: Some((first).into()),
                        next: Some((second).into()),
                    },
                    MapEdit {
                        key: b"bb".to_vec(),
                        expected: None,
                        next: Some((first).into()),
                    },
                    MapEdit {
                        key: b"c".to_vec(),
                        expected: Some((first).into()),
                        next: None,
                    },
                ],
            },
            MapBatchRequest {
                base,
                edits: vec![
                    MapEdit {
                        key: b"ab".to_vec(),
                        expected: Some((first).into()),
                        next: None,
                    },
                    MapEdit {
                        key: b"az".to_vec(),
                        expected: None,
                        next: Some((second).into()),
                    },
                    MapEdit {
                        key: b"z".to_vec(),
                        expected: Some((first).into()),
                        next: Some((third).into()),
                    },
                ],
            },
        ];
        let output = store.apply_batch(&requests, &sink, &mut work)?;
        for (request, root) in requests.iter().zip(&output.roots) {
            let mut model = baseline.clone();
            let mut serial = base;
            for edit in &request.edits {
                check_eq!(model.get(&edit.key).copied().map(Into::into), edit.expected);
                match edit.next {
                    Some(next) => {
                        let _prior = model.insert(edit.key.clone(), next.object_ref()?);
                    }
                    None => {
                        let _prior = model.remove(&edit.key);
                    }
                }
                serial = store
                    .apply(serial, std::slice::from_ref(edit), &sink, &mut work)?
                    .root;
            }
            check_eq!(*root, serial);
            let page = store.scan(*root, None, 128, &mut work)?;
            check!(!page.has_more);
            check_eq!(
                page.entries,
                model
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect::<Vec<_>>()
            );
        }
        let historical = store.scan(base, None, 128, &mut work)?;
        check_eq!(
            historical.entries,
            baseline
                .into_iter()
                .map(|(key, value)| (key, value.into()))
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn sorted_existing_root_closes_shared_ancestors_once() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let old = value(&store, b"old", &mut work)?;
        let new = value(&store, b"new", &mut work)?;
        let edits = (0u16..256)
            .map(|index| {
                let mut key = b"owner/shared/prefix/".to_vec();
                key.extend_from_slice(&index.to_be_bytes());
                key
            })
            .collect::<Vec<_>>();
        let initial = edits
            .iter()
            .map(|key| MapEdit {
                key: key.clone(),
                expected: None,
                next: Some((old).into()),
            })
            .collect::<Vec<_>>();
        let base = store.apply(MapRoot(None), &initial, &sink, &mut work)?.root;
        let optimized = StagedMap::open(&store, &mut work)?;
        for edit in &edits {
            optimized.scratch.insert_edit(
                &OrdinalMapEdit::unscoped(
                    0,
                    MapEdit {
                        key: edit.clone(),
                        expected: Some((old).into()),
                        next: Some((new).into()),
                    },
                )?,
                &mut work,
            )?;
        }
        let before_optimized = work.usage().writes;
        let optimized_root = store.apply_sorted_existing_root(0, base, &optimized, &mut work)?;
        let optimized_writes = work.usage().writes - before_optimized;

        let serial = StagedMap::open(&store, &mut work)?;
        let before_serial = work.usage().writes;
        let mut serial_root = base;
        for key in &edits {
            serial_root = store.update_one(
                serial_root,
                &key_nibbles(key, &work)?,
                Some(new.into()),
                &serial,
                &mut work,
            )?;
        }
        let serial_writes = work.usage().writes - before_serial;
        check_eq!(optimized_root, serial_root);
        check!(
            optimized_writes
                .checked_mul(2)
                .ok_or("write count overflow")?
                < serial_writes,
            "shared ancestors were rewritten too often: optimized={optimized_writes}, serial={serial_writes}"
        );
        Ok(())
    }

    #[test]
    fn sorted_existing_root_differential_replay_over_binary_prefix_keys()
    -> Result<(), Box<dyn std::error::Error>> {
        fn next_seed(seed: &mut u64) -> u64 {
            *seed = (*seed)
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            *seed
        }

        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut setup_budget = budget()?;
        let values = [
            value(&store, b"value-zero", &mut setup_budget)?,
            value(&store, b"value-one", &mut setup_budget)?,
            value(&store, b"value-two", &mut setup_budget)?,
        ];
        let domain = [
            vec![],
            vec![0],
            vec![0, 0],
            vec![0, 1],
            vec![1],
            vec![1, 0],
            vec![1, 1],
            vec![1, 1, 0],
            vec![0xff],
            vec![0xff, 0],
        ];
        let mut bases = Vec::new();
        for variant in 0usize..8 {
            let mut initial = Vec::new();
            let mut model = BTreeMap::new();
            for (index, key) in domain.iter().enumerate() {
                if (index + variant) % 3 == 0 {
                    let reference = *values
                        .get((index + variant) % values.len())
                        .ok_or("base value missing")?;
                    initial.push(MapEdit {
                        key: key.clone(),
                        expected: None,
                        next: Some((reference).into()),
                    });
                    let _prior = model.insert(key.clone(), reference);
                }
            }
            let root = store
                .apply(MapRoot(None), &initial, &sink, &mut setup_budget)?
                .root;
            bases.push((root, model));
        }

        let mut seed = 0x81f3_2d6a_450c_b719_u64;
        let mut insertions = 0usize;
        let mut deletions = 0usize;
        let mut noops = 0usize;
        for case in 0usize..128 {
            let (base, base_model) = bases.get(case % bases.len()).ok_or("base state missing")?;
            let mut chosen = BTreeSet::new();
            for _selection in 0..7 {
                let index = usize::try_from(
                    next_seed(&mut seed)
                        % u64::try_from(domain.len()).map_err(|_error| "domain too large")?,
                )?;
                let key = domain.get(index).ok_or("domain key missing")?.clone();
                let _was_new = chosen.insert(key);
            }
            let mut edits = Vec::new();
            let mut expected_model = base_model.clone();
            for key in chosen {
                let expected = base_model.get(&key).copied();
                let next = match next_seed(&mut seed) % 5 {
                    0 => None,
                    1 => expected,
                    number => Some(
                        *values
                            .get(usize::try_from(number - 2)?)
                            .ok_or("next value missing")?,
                    ),
                };
                if expected.is_none() && next.is_some() {
                    insertions += 1;
                } else if expected.is_some() && next.is_none() {
                    deletions += 1;
                } else if expected == next {
                    noops += 1;
                }
                match next {
                    Some(reference) => {
                        let _prior = expected_model.insert(key.clone(), reference);
                    }
                    None => {
                        let _prior = expected_model.remove(&key);
                    }
                }
                edits.push(MapEdit {
                    key,
                    expected: expected.map(Into::into),
                    next: next.map(Into::into),
                });
            }

            let mut work = budget()?;
            for edit in &edits {
                check_eq!(store.lookup(*base, &edit.key, &mut work)?, edit.expected);
            }
            let optimized = StagedMap::open(&store, &mut work)?;
            for edit in &edits {
                optimized
                    .scratch
                    .insert_edit(&OrdinalMapEdit::unscoped(0, edit.clone())?, &mut work)?;
            }
            let actual = store.apply_sorted_existing_root(0, *base, &optimized, &mut work)?;
            let serial = StagedMap::open(&store, &mut work)?;
            let mut expected_root = *base;
            for edit in &edits {
                if edit.expected != edit.next {
                    expected_root = store.update_one(
                        expected_root,
                        &key_nibbles(&edit.key, &work)?,
                        edit.next,
                        &serial,
                        &mut work,
                    )?;
                }
            }
            check_eq!(actual, expected_root, "differential case {case}");
            if case % 16 == 0 {
                store.install_reachable_map_nodes(&[actual], &optimized, &sink, &mut work)?;
                let page = store.scan(actual, None, 32, &mut work)?;
                check!(!page.has_more);
                check_eq!(
                    page.entries,
                    expected_model
                        .into_iter()
                        .map(|(key, value)| (key, value.into()))
                        .collect::<Vec<_>>()
                );
                let historical = store.scan(*base, None, 32, &mut work)?;
                check_eq!(
                    historical.entries,
                    base_model
                        .clone()
                        .into_iter()
                        .map(|(key, value)| (key, value.into()))
                        .collect::<Vec<_>>()
                );
            }
        }
        check!(insertions > 0 && deletions > 0 && noops > 0);
        Ok(())
    }

    #[test]
    fn late_existing_root_conflict_preserves_all_original_roots()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let old = value(&store, b"old", &mut work)?;
        let new = value(&store, b"new", &mut work)?;
        let base = store
            .apply(
                MapRoot(None),
                &[MapEdit {
                    key: b"existing".to_vec(),
                    expected: None,
                    next: Some((old).into()),
                }],
                &sink,
                &mut work,
            )?
            .root;
        let before_objects = physical_file_count(&directory)?;
        let before_reservations = sink.1.load(Ordering::Relaxed);
        let mut edits = (0u16..128)
            .map(|number| {
                OrdinalMapEdit::unscoped(
                    0,
                    MapEdit {
                        key: number.to_be_bytes().to_vec(),
                        expected: None,
                        next: Some((new).into()),
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        edits.push(OrdinalMapEdit::unscoped(
            1,
            MapEdit {
                key: b"existing".to_vec(),
                expected: Some((new).into()),
                next: None,
            },
        )?);
        let mut cursor = TestEditCursor(edits.into_iter());
        check!(matches!(
            store.apply_stream(&[base, base], &mut cursor, &sink, &mut work),
            Err(IndexError::Conflict)
        ));
        check_eq!(sink.1.load(Ordering::Relaxed), before_reservations);
        check_eq!(physical_file_count(&directory)?, before_objects);
        check_eq!(
            store.lookup_object(base, b"existing", &mut work)?,
            Some(old)
        );
        Ok(())
    }

    #[test]
    fn late_stream_conflict_or_duplicate_never_reserves_a_map_object()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let value = value(&store, b"streamed", &mut work)?;
        let before = physical_file_count(&directory)?;
        let make_edits = || -> Result<Vec<OrdinalMapEdit>, IndexError> {
            (0u16..192)
                .map(|number| {
                    OrdinalMapEdit::unscoped(
                        0,
                        MapEdit {
                            key: number.to_be_bytes().to_vec(),
                            expected: None,
                            next: Some((value).into()),
                        },
                    )
                })
                .collect::<Result<Vec<_>, _>>()
        };
        let mut conflict = make_edits()?;
        conflict.push(OrdinalMapEdit::unscoped(
            0,
            MapEdit {
                key: 192u16.to_be_bytes().to_vec(),
                expected: Some((value).into()),
                next: None,
            },
        )?);
        let mut conflict_cursor = TestEditCursor(conflict.into_iter());
        check!(matches!(
            store.apply_stream(&[MapRoot(None)], &mut conflict_cursor, &sink, &mut work),
            Err(IndexError::Conflict)
        ));
        let mut duplicate = make_edits()?;
        let final_edit = duplicate.last().ok_or("missing final test edit")?.clone();
        duplicate.push(final_edit);
        let mut duplicate_cursor = TestEditCursor(duplicate.into_iter());
        check!(matches!(
            store.apply_stream(&[MapRoot(None)], &mut duplicate_cursor, &sink, &mut work),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(sink.1.load(Ordering::Relaxed), 0);
        check_eq!(sink.2.load(Ordering::Relaxed), 0);
        check_eq!(physical_file_count(&directory)?, before);
        assert_clean_scratch(&directory)?;
        Ok(())
    }

    #[test]
    fn later_batch_conflict_refuses_before_any_reservation()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let object = value(&store, b"value", &mut work)?;
        let before = physical_file_count(&directory)?;
        let requests = [
            MapBatchRequest {
                base: MapRoot(None),
                edits: vec![MapEdit {
                    key: b"valid".to_vec(),
                    expected: None,
                    next: Some((object).into()),
                }],
            },
            MapBatchRequest {
                base: MapRoot(None),
                edits: vec![MapEdit {
                    key: b"conflict".to_vec(),
                    expected: Some((object).into()),
                    next: None,
                }],
            },
        ];
        check!(matches!(
            store.apply_batch(&requests, &sink, &mut work),
            Err(IndexError::Conflict)
        ));
        check_eq!(physical_file_count(&directory)?, before);
        check_eq!(sink.1.load(Ordering::Relaxed), 0);
        check_eq!(sink.2.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn fixed_width_single_edit_cost_stays_bounded_across_size_and_history()
    -> Result<(), Box<dyn std::error::Error>> {
        const KEY_NIBBLES: u64 = 8;
        const MAX_DIRECT_CHILDREN: u64 = 17;
        const MAX_CHANGED_PATH_NODES: usize = 10;

        fn cost_budget() -> Result<WorkBudget, IndexError> {
            Ok(WorkBudget::new(
                WorkLimits {
                    reads: 100_000,
                    writes: 100_000,
                    bytes: 256 * 1024 * 1024,
                    mutations: 100_000,
                },
                Instant::now()
                    .checked_add(Duration::from_secs(180))
                    .ok_or(IndexError::ResourceLimited)?,
            ))
        }

        macro_rules! check_eq {
            ($actual:expr, $expected:expr, $label:expr $(,)?) => {{
                match (&$actual, &$expected) {
                    (actual, expected) if !(*actual == *expected) => {
                        return Err(
                            format!("{}: expected {expected:?}, got {actual:?}", $label).into()
                        );
                    }
                    _ => {}
                }
            }};
        }

        let mut measured = BTreeMap::<(usize, u32), WorkUsage>::new();
        for owners in [1_usize, 64, 512] {
            let directory = TestDir::new();
            let store = FileObjectStore::open(&directory.0)?;
            let sink = RecordingSink::default();
            let mut setup_budget = cost_budget()?;
            let original = value(&store, b"bootstrap", &mut setup_budget)?;
            let keys = (0..owners)
                .map(|owner| Ok(u32::try_from(owner)?.to_be_bytes().to_vec()))
                .collect::<Result<Vec<_>, std::num::TryFromIntError>>()?;
            let initial = keys
                .iter()
                .map(|key| MapEdit {
                    key: key.clone(),
                    expected: None,
                    next: Some((original).into()),
                })
                .collect::<Vec<_>>();
            let mut root = store
                .apply(MapRoot(None), &initial, &sink, &mut setup_budget)?
                .root;
            let bootstrap = root;
            let hot = keys.first().ok_or("missing hot owner")?.clone();
            let cold = keys.last().ok_or("missing cold owner")?.clone();
            let mut prior_value = original;

            // Every edit changes the same four-byte key. The intervening
            // versions are distinct so retained immutable history grows.
            for history in 0_u32..=32 {
                let mut preparation_budget = cost_budget()?;
                let next = value(
                    &store,
                    format!("hot/{history:02}").as_bytes(),
                    &mut preparation_budget,
                )?;
                let old_root = root;
                let installed_before = sink
                    .0
                    .lock()
                    .map_err(|_error| IndexError::Storage("test sink poisoned".into()))?
                    .len();
                let mut update_budget = cost_budget()?;
                root = store
                    .apply(
                        root,
                        &[MapEdit {
                            key: hot.clone(),
                            expected: Some((prior_value).into()),
                            next: Some((next).into()),
                        }],
                        &sink,
                        &mut update_budget,
                    )?
                    .root;
                let usage = update_budget.usage();
                let installed_after = sink
                    .0
                    .lock()
                    .map_err(|_error| IndexError::Storage("test sink poisoned".into()))?
                    .len();
                let installed = installed_after
                    .checked_sub(installed_before)
                    .ok_or("installed object count regressed")?;
                if installed > MAX_CHANGED_PATH_NODES {
                    return Err(format!("one fixed-width edit installed {installed} map nodes for N={owners}, H={history}").into());
                }
                if [0, 8, 32].contains(&history) {
                    let _previous_measurement = measured.insert((owners, history), usage);
                    let mut verification_budget = cost_budget()?;
                    check_eq!(
                        store.lookup_object(old_root, &hot, &mut verification_budget)?,
                        Some(prior_value),
                        format!("old hot value: N={owners}, H={history}")
                    );
                    check_eq!(
                        store.lookup_object(root, &hot, &mut verification_budget)?,
                        Some(next),
                        format!("current hot value: N={owners}, H={history}")
                    );
                    check_eq!(
                        store.lookup_object(root, &cold, &mut verification_budget)?,
                        Some(if owners == 1 { next } else { original }),
                        format!("current cold value: N={owners}, H={history}")
                    );
                    check_eq!(
                        store.lookup_object(bootstrap, &hot, &mut verification_budget)?,
                        Some(original),
                        format!("bootstrap hot value: N={owners}, H={history}")
                    );
                }
                prior_value = next;
            }
        }

        for owners in [1_usize, 64, 512] {
            let baseline = *measured
                .get(&(owners, 0))
                .ok_or("missing H=0 measurement")?;
            for history in [8_u32, 32] {
                let later = *measured
                    .get(&(owners, history))
                    .ok_or("missing retained-history measurement")?;
                // Fixed key width and unchanged key set preserve radix shape.
                // Retaining older roots must not add reads, writes or bytes to
                // this one-key update's charged work.
                check_eq!(later, baseline, format!("N={owners}, H={history}"));
            }
            if baseline.writes > 32 {
                return Err(format!("N={owners}: {baseline:?}").into());
            }
        }
        let medium = measured.get(&(64, 0)).ok_or("missing N=64 measurement")?;
        let large = measured.get(&(512, 0)).ok_or("missing N=512 measurement")?;
        // A four-byte key has at most eight radix nibbles. Even if every
        // changed level probes each of its 17 direct references twice, a
        // larger unrelated owner set must not add a corpus-sized read pass.
        let path_slack = KEY_NIBBLES
            .checked_mul(MAX_DIRECT_CHILDREN)
            .and_then(|bound| bound.checked_mul(2))
            .ok_or(IndexError::ResourceLimited)?;
        let max_reads = medium
            .reads
            .checked_add(path_slack)
            .ok_or(IndexError::ResourceLimited)?;
        if large.reads > max_reads {
            return Err(format!(
                "single-key reads grew with unrelated owners: N=64 {medium:?}, N=512 {large:?}"
            )
            .into());
        }
        Ok(())
    }
}
