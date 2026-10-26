//! Compare virtual nibble positions, independent of physical prefix compression.
//! One path and bounded stack retain shared decoded nodes without copying tails.

use std::{cmp::Ordering, sync::Arc};

use quanta_index_core::index::{
    MemoryLease,
    ports::{MapDiffRange, MapDiffVisitor},
};

use super::{
    BranchValue, FileObjectStore, IndexError, MAX_KEY_BYTES, MAX_NIBBLES, MAX_SCAN_LIMIT,
    MapBuffer, MapRoot, MapValue, MemoryLifetime, Node, ObjectRef, WorkBudget, WorkUsage,
    nibbles_to_key,
};

struct SharedNode {
    reference: ObjectRef,
    node: Node,
    // The Arc allocation outlives its data; reserve it before Arc::new.
    _memory: Option<MemoryLease>,
}

enum Position {
    Stored(ObjectRef),
    Node(Arc<SharedNode>, usize),
    Terminal(Arc<SharedNode>, usize, usize),
}

impl Position {
    fn identity(&self) -> (ObjectRef, Option<usize>, usize) {
        match self {
            Self::Stored(reference) => (*reference, None, 0),
            Self::Node(node, offset) => (node.reference, None, *offset),
            Self::Terminal(node, edge, offset) => (node.reference, Some(*edge), *offset),
        }
    }

    fn resolve(
        &mut self,
        read_node: &mut impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if let Self::Stored(reference) = self {
            let reference = *reference;
            // Arc's two atomic counters, payload alignment and trailing padding.
            let header = std::alloc::Layout::new::<[std::sync::atomic::AtomicUsize; 2]>();
            let (layout, _) = header
                .extend(std::alloc::Layout::new::<SharedNode>())
                .map_err(|_error| IndexError::ResourceLimited)?;
            let bytes = u64::try_from(layout.pad_to_align().size())
                .map_err(|_error| IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                bytes,
                ..WorkUsage::default()
            })?;
            let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
            let node = read_node(reference, budget)?;
            *self = Self::Node(
                Arc::new(SharedNode {
                    reference,
                    node,
                    _memory: memory,
                }),
                0,
            );
        }
        Ok(())
    }

    fn value(&self) -> Result<Option<MapValue>, IndexError> {
        match self {
            Self::Node(shared, offset) => Ok((*offset == shared.node.prefix.len())
                .then_some(shared.node.value)
                .flatten()),
            Self::Terminal(shared, edge, offset) => {
                let branch = shared
                    .node
                    .branches
                    .get(*edge)?
                    .ok_or_else(invalid_position)?;
                if *offset == branch.prefix.len() {
                    match branch.value {
                        BranchValue::Terminal(value) => Ok(Some(value)),
                        BranchValue::Indirect(_) => Err(invalid_position()),
                    }
                } else {
                    Ok(None)
                }
            }
            Self::Stored(_) => Err(invalid_position()),
        }
    }

    fn child(&self, edge: usize) -> Result<Option<Self>, IndexError> {
        match self {
            Self::Node(shared, offset) => {
                if let Some(nibble) = shared.node.prefix.get(*offset) {
                    let next_offset = offset.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    return Ok((usize::from(*nibble) == edge)
                        .then(|| Self::Node(Arc::clone(shared), next_offset)));
                }
                Ok(shared
                    .node
                    .branches
                    .get(edge)?
                    .map(|branch| match branch.value {
                        BranchValue::Indirect(reference) => Self::Stored(reference),
                        BranchValue::Terminal(_) => Self::Terminal(Arc::clone(shared), edge, 0),
                    }))
            }
            Self::Terminal(shared, branch_edge, offset) => {
                let branch = shared
                    .node
                    .branches
                    .get(*branch_edge)?
                    .ok_or_else(invalid_position)?;
                let next_offset = offset.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                Ok(branch
                    .prefix
                    .get(*offset)
                    .filter(|nibble| usize::from(**nibble) == edge)
                    .map(|_| Self::Terminal(Arc::clone(shared), *branch_edge, next_offset)))
            }
            Self::Stored(_) => Err(invalid_position()),
        }
    }
}

fn invalid_position() -> IndexError {
    IndexError::BackendViolation("invalid structural map diff position".into())
}

struct Frame {
    before: Option<Position>,
    after: Option<Position>,
    path_len: usize,
    after_order: Ordering,
    entered: bool,
    next_edge: usize,
}

impl Frame {
    fn new(
        before: Option<Position>,
        after: Option<Position>,
        path_len: usize,
        after_order: Ordering,
    ) -> Self {
        Self {
            before,
            after,
            path_len,
            after_order,
            entered: false,
            next_edge: 0,
        }
    }

    fn equal(&self) -> bool {
        self.before.as_ref().map(Position::identity) == self.after.as_ref().map(Position::identity)
    }
}

fn charge_growth<T>(buffer: &MapBuffer<T>, budget: &mut WorkBudget) -> Result<(), IndexError> {
    let bytes =
        u64::try_from(buffer.growth_work_bytes()?).map_err(|_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes,
        ..WorkUsage::default()
    })
}

fn nibble(key: &[u8], depth: usize) -> Option<u8> {
    key.get(depth >> 1).map(|byte| {
        if depth.is_multiple_of(2) {
            byte >> 4
        } else {
            byte & 15
        }
    })
}

pub(super) fn visit(
    store: &FileObjectStore,
    before: MapRoot,
    after: MapRoot,
    range: MapDiffRange<'_>,
    visitor: &mut dyn MapDiffVisitor,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    visit_with(
        before,
        after,
        range,
        visitor,
        budget,
        |reference, budget| store.read_node(reference, budget),
    )
}

pub(super) fn visit_with(
    before: MapRoot,
    after: MapRoot,
    range: MapDiffRange<'_>,
    visitor: &mut dyn MapDiffVisitor,
    budget: &mut WorkBudget,
    mut read_node: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<Node, IndexError>,
) -> Result<bool, IndexError> {
    budget.checkpoint()?;
    if range.limit == 0
        || range.limit > MAX_SCAN_LIMIT
        || range.prefix.len() > MAX_KEY_BYTES
        || range.after.is_some_and(|key| key.len() > MAX_KEY_BYTES)
    {
        return Err(IndexError::InvalidInput(
            "map diff range is out of bounds".into(),
        ));
    }
    // Validate even an equal-root request, but need no object read/allocation.
    if before == after {
        return Ok(false);
    }
    let prefix_len = range
        .prefix
        .len()
        .checked_mul(2)
        .ok_or(IndexError::ResourceLimited)?;
    let mut path = MapBuffer::new(MAX_NIBBLES);
    let mut stack = MapBuffer::new(MAX_NIBBLES + 1);
    charge_growth(&stack, budget)?;
    stack.push(
        Frame::new(
            before.0.map(Position::Stored),
            after.0.map(Position::Stored),
            0,
            if range.after.is_some() {
                Ordering::Equal
            } else {
                Ordering::Greater
            },
        ),
        budget,
    )?;
    let mut emitted = 0_u32;
    while let Some(frame) = stack.last_mut() {
        budget.checkpoint()?;
        path.truncate(frame.path_len);
        if !frame.entered {
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<Frame>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            if frame.equal() {
                let _finished = stack.pop();
                continue;
            }
            for position in [&mut frame.before, &mut frame.after].into_iter().flatten() {
                position.resolve(&mut read_node, budget)?;
            }
            frame.entered = true;
            let expected = frame
                .before
                .as_ref()
                .map(Position::value)
                .transpose()?
                .flatten();
            let next = frame
                .after
                .as_ref()
                .map(Position::value)
                .transpose()?
                .flatten();
            if (expected.is_some() || next.is_some()) && !path.len().is_multiple_of(2) {
                return Err(IndexError::CorruptObject(
                    "map diff value ends at an incomplete byte".into(),
                ));
            }
            if expected != next && path.len() >= prefix_len && frame.after_order.is_gt() {
                if emitted == range.limit {
                    return Ok(true);
                }
                let bytes =
                    u64::try_from(path.len() >> 1).map_err(|_error| IndexError::ResourceLimited)?;
                budget.charge(WorkUsage {
                    bytes,
                    ..WorkUsage::default()
                })?;
                let memory = if bytes == 0 {
                    None
                } else {
                    budget.reserve_memory(bytes, MemoryLifetime::Temporary)?
                };
                let key = nibbles_to_key(&path)?;
                visitor.changed(&key, expected, next, budget)?;
                drop(key);
                drop(memory);
                emitted = emitted.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
        }
        if frame.next_edge == 16 {
            let _finished = stack.pop();
            continue;
        }
        let edge = frame.next_edge;
        frame.next_edge = frame
            .next_edge
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: 1,
            ..WorkUsage::default()
        })?;
        let edge_byte = u8::try_from(edge).map_err(|_error| IndexError::ResourceLimited)?;
        if nibble(range.prefix, frame.path_len).is_some_and(|required| required != edge_byte) {
            continue;
        }
        let after_order = if frame.after_order.is_eq() {
            range
                .after
                .and_then(|key| nibble(key, frame.path_len))
                .map_or(Ordering::Greater, |bound| edge_byte.cmp(&bound))
        } else {
            frame.after_order
        };
        if after_order.is_lt() {
            continue;
        }
        let before = frame
            .before
            .as_ref()
            .map(|p| p.child(edge))
            .transpose()?
            .flatten();
        let after = frame
            .after
            .as_ref()
            .map(|p| p.child(edge))
            .transpose()?
            .flatten();
        if before.is_none() && after.is_none() {
            continue;
        }
        if path.len() == MAX_NIBBLES {
            return Err(IndexError::CorruptObject(
                "map diff branch exceeds key limit".into(),
            ));
        }
        charge_growth(&path, budget)?;
        path.push(edge_byte, budget)?;
        charge_growth(&stack, budget)?;
        stack.push(Frame::new(before, after, path.len(), after_order), budget)?;
    }
    budget.checkpoint()?;
    Ok(false)
}
