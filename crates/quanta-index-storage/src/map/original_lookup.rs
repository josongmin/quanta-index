//! Attempt-local immutable-root reads. Retain one admitted decoded path,
//! never replacement nodes or a cache of mutable authority.

use super::{
    BranchValue, FileObjectStore, IndexError, MAX_NIBBLES, MapBuffer, MapRoot, MapValue, Node,
    ObjectRef, PackedBranch, WorkBudget, WorkUsage, charge_key_allocation, key_nibbles,
};
use quanta_index_core::index::ports::MAX_MAP_LOOKUP_BATCH;

pub(super) fn lookup_batch_with(
    root: MapRoot,
    keys: &[&[u8]],
    values: &mut [Option<MapValue>],
    budget: &mut WorkBudget,
    mut read_node: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
) -> Result<(), IndexError> {
    if keys.len() > MAX_MAP_LOOKUP_BATCH || keys.len() != values.len() {
        return Err(IndexError::InvalidInput(
            "map lookup batch shape exceeds bound".into(),
        ));
    }
    budget.charge(WorkUsage {
        bytes: u64::try_from(
            keys.len()
                .checked_mul(std::mem::size_of::<Option<MapValue>>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let mut lookup = OriginalLookup::new();
    for (key, value) in keys.iter().zip(values) {
        charge_key_allocation(key, budget)?;
        let nibbles = key_nibbles(key, budget)?;
        *value = lookup.lookup_with(root, &nibbles, budget, &mut read_node)?;
    }
    Ok(())
}

struct Frame {
    reference: ObjectRef,
    offset: usize,
    node: Node,
}

/// Packed DATA is terminal-only and belongs to its enclosing physical frame.
/// A borrowed lookup avoids retaining another frame or charging a decoded read.
fn lookup_packed(
    packed: &PackedBranch,
    nibbles: &[u8],
    offset: usize,
    budget: &mut WorkBudget,
) -> Result<Option<MapValue>, IndexError> {
    budget.checkpoint()?;
    let node = &packed.node;
    budget.charge(WorkUsage {
        bytes: u64::try_from(node.prefix.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let remaining = nibbles
        .get(offset..)
        .ok_or_else(|| IndexError::CorruptObject("map traversal passed key end".into()))?;
    if !remaining.starts_with(&node.prefix) {
        return Ok(None);
    }
    let offset = offset
        .checked_add(node.prefix.len())
        .ok_or(IndexError::ResourceLimited)?;
    if offset > MAX_NIBBLES {
        return Err(IndexError::CorruptObject(
            "packed map traversal exceeds key limit".into(),
        ));
    }
    if offset == nibbles.len() {
        return Ok(node.value);
    }
    let edge = usize::from(
        *nibbles
            .get(offset)
            .ok_or_else(|| IndexError::CorruptObject("map traversal passed key end".into()))?,
    );
    let offset = offset.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    if offset > MAX_NIBBLES {
        return Err(IndexError::CorruptObject(
            "packed map traversal exceeds key limit".into(),
        ));
    }
    let Some(branch) = node.branches.get(edge)? else {
        return Ok(None);
    };
    let BranchValue::Terminal(value) = &branch.value else {
        return Err(IndexError::CorruptObject(
            "packed map branch has a nested child".into(),
        ));
    };
    budget.charge(WorkUsage {
        bytes: u64::try_from(branch.prefix.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    Ok((nibbles.get(offset..) == Some(branch.prefix.as_slice())).then_some(*value))
}

pub(super) struct OriginalLookup {
    root: Option<MapRoot>,
    frames: MapBuffer<Frame>,
}

impl OriginalLookup {
    pub(super) fn new() -> Self {
        Self {
            root: None,
            frames: MapBuffer::new(MAX_NIBBLES + 1),
        }
    }

    /// The caller owns the immutable base throughout the bounded read. Each cached
    /// frame must match both the actual parent edge and its path offset. Root
    /// changes discard the path; divergent children discard the old suffix.
    /// No CAS result is cached: every predicate compares its actual value.
    pub(super) fn lookup(
        &mut self,
        store: &FileObjectStore,
        root: MapRoot,
        nibbles: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        self.lookup_with(root, nibbles, budget, |reference, budget| {
            store.read_node(reference, budget)
        })
    }

    pub(super) fn lookup_with(
        &mut self,
        root: MapRoot,
        nibbles: &[u8],
        budget: &mut WorkBudget,
        mut read_node: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
    ) -> Result<Option<MapValue>, IndexError> {
        budget.checkpoint()?;
        if self.root != Some(root) {
            self.frames.truncate(0);
            self.root = Some(root);
        }
        let Some(reference) = root.0 else {
            return Ok(None);
        };
        let mut reference = reference;
        let mut offset = 0usize;
        let mut depth = 0usize;
        loop {
            budget.checkpoint()?;
            if !self
                .frames
                .get(depth)
                .is_some_and(|frame| frame.reference == reference && frame.offset == offset)
            {
                self.frames.truncate(depth);
                budget.charge(WorkUsage {
                    bytes: u64::try_from(
                        self.frames
                            .growth_work_bytes()?
                            .checked_add(std::mem::size_of::<Frame>())
                            .ok_or(IndexError::ResourceLimited)?,
                    )
                    .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                let node = read_node(reference, budget)?;
                self.frames.push(
                    Frame {
                        reference,
                        node,
                        offset,
                    },
                    budget,
                )?;
            }
            let frame = self.frames.get(depth).ok_or_else(|| {
                IndexError::BackendViolation("original lookup lost its frame".into())
            })?;
            let node = &frame.node;
            budget.charge(WorkUsage {
                bytes: u64::try_from(node.prefix.len())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let remaining = nibbles
                .get(offset..)
                .ok_or_else(|| IndexError::CorruptObject("map traversal passed key end".into()))?;
            let result = if remaining.starts_with(&node.prefix) {
                offset = offset
                    .checked_add(node.prefix.len())
                    .ok_or(IndexError::ResourceLimited)?;
                if offset == nibbles.len() {
                    Some(node.value)
                } else {
                    let edge = usize::from(*nibbles.get(offset).ok_or_else(|| {
                        IndexError::CorruptObject("map traversal passed key end".into())
                    })?);
                    offset = offset.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    if offset > MAX_NIBBLES {
                        return Err(IndexError::CorruptObject(
                            "map traversal exceeds key limit".into(),
                        ));
                    }
                    match node.branches.get(edge)? {
                        None => Some(None),
                        Some(branch) => match &branch.value {
                            BranchValue::Indirect(next) => {
                                reference = *next;
                                None
                            }
                            BranchValue::Packed(child) => {
                                if !branch.prefix.is_empty() {
                                    return Err(IndexError::CorruptObject(
                                        "packed map branch has an external prefix".into(),
                                    ));
                                }
                                Some(lookup_packed(child, nibbles, offset, budget)?)
                            }
                            BranchValue::Terminal(value) => {
                                budget.charge(WorkUsage {
                                    bytes: u64::try_from(branch.prefix.len())
                                        .map_err(|_error| IndexError::ResourceLimited)?,
                                    ..WorkUsage::default()
                                })?;
                                Some(
                                    (nibbles.get(offset..) == Some(branch.prefix.as_slice()))
                                        .then_some(*value),
                                )
                            }
                        },
                    }
                }
            } else {
                Some(None)
            };
            depth = depth.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            if let Some(value) = result {
                self.frames.truncate(depth);
                return Ok(value);
            }
        }
    }
}
