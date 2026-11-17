//! Exact physical wire for one radix node. QIMAP006 embeds bounded nested
//! branching nodes; QIMAP004/005 remain bit-exact readers and writers.

use super::{
    Branch, BranchValue, Branches, IndexError, MAX_INLINE_NIBBLES, MAX_NIBBLES, MAX_NODE_CHILDREN,
    MAX_NODE_PAYLOAD, MapBuffer, NODE_HEADER_BYTES, Node, ObjectRef, RetainedMapValue,
    StoredObject, WorkBudget, WorkUsage, value_codec,
};
use quanta_index_core::index::{MemoryLease, MemoryLifetime};

const MAGIC_004: &[u8; 8] = b"QIMAP004";
const MAGIC_005: &[u8; 8] = b"QIMAP005";
const MAGIC_006: &[u8; 8] = b"QIMAP006";
/// Inline child depth is counted from a physical root at depth zero.
const MAX_PACKED_DEPTH: usize = 8;
fn corrupt(reason: &str) -> IndexError {
    IndexError::CorruptObject(reason.into())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PackedShape {
    body_len: usize,
    child_count: usize,
    depth: usize,
}

pub(super) struct Layout<'a> {
    node: &'a Node,
    /// Packed branches requiring their own immutable node installation.
    pub(super) spill_mask: u16,
    embed_mask: u16,
    nested_embed: bool,
    payload_capacity: usize,
    child_count: usize,
}

pub(super) struct EncodedNode {
    pub(super) payload: Vec<u8>,
    pub(super) children: Vec<ObjectRef>,
    // The backing vectors die before this allocation grant.
    _memory: Option<MemoryLease>,
}

fn add(left: usize, right: usize) -> Result<usize, IndexError> {
    left.checked_add(right).ok_or(IndexError::ResourceLimited)
}

fn value_children(value: &RetainedMapValue) -> Result<usize, IndexError> {
    Ok(value.children()?.len())
}

fn checked_nibbles(prefix: &[u8], bound: usize) -> Result<(), IndexError> {
    if prefix.len() > bound || prefix.iter().any(|nibble| *nibble > 15) {
        return Err(corrupt("invalid map node nibble prefix"));
    }
    Ok(())
}

pub(super) fn packed_shape(node: &Node) -> Result<PackedShape, IndexError> {
    checked_nibbles(&node.prefix, MAX_INLINE_NIBBLES)?;
    if node.branches.len() == 0 || (node.value.is_none() && node.branches.len() == 1) {
        return Err(corrupt("noncanonical packed map branch"));
    }
    let nested = node
        .branches
        .iter()
        .any(|(_, branch)| matches!(branch.value, BranchValue::Packed(_)));
    let mut payload = add(
        NODE_HEADER_BYTES + if nested { 2 } else { 0 },
        node.prefix.len(),
    )?;
    let mut children = 0;
    let mut depth = 1;
    if let Some(value) = node.value.as_ref() {
        payload = add(payload, value_codec::payload_len(value)?)?;
        children = add(children, value_children(value)?)?;
    }
    for (_, branch) in node.branches.iter() {
        match &branch.value {
            BranchValue::Terminal(value) => {
                checked_nibbles(&branch.prefix, MAX_INLINE_NIBBLES)?;
                payload = add(
                    add(add(payload, 2)?, branch.prefix.len())?,
                    value_codec::payload_len(value)?,
                )?;
                children = add(children, value_children(value)?)?;
            }
            BranchValue::Packed(packed) if branch.prefix.is_empty() => {
                payload = add(add(payload, 2)?, packed.shape.body_len)?;
                children = add(children, packed.shape.child_count)?;
                depth = depth.max(add(1, packed.shape.depth)?);
            }
            BranchValue::Indirect(_) | BranchValue::Packed(_) => {
                return Err(corrupt("packed map body has an indirect or tailed child"));
            }
        }
    }
    if depth > MAX_PACKED_DEPTH
        || payload > usize::from(u16::MAX)
        || payload > MAX_NODE_PAYLOAD
        || children > MAX_NODE_CHILDREN
    {
        return Err(corrupt("packed map branch exceeds body bound"));
    }
    Ok(PackedShape {
        body_len: payload,
        child_count: children,
        depth,
    })
}

/// Start with every packed branch as one indirect physical edge, then embed
/// eligible branches in ascending nibble order while both old node bounds hold.
pub(super) fn layout(node: &Node) -> Result<Layout<'_>, IndexError> {
    checked_nibbles(&node.prefix, MAX_NIBBLES)?;
    if node.value.is_none() && (node.branches.len() == 0 || node.branches.len() == 1) {
        return Err(corrupt("empty or uncompressed unary map node"));
    }
    let mut payload = add(NODE_HEADER_BYTES, node.prefix.len())?;
    let mut children = 0;
    if let Some(value) = node.value.as_ref() {
        payload = add(payload, value_codec::payload_len(value)?)?;
        children = add(children, value_children(value)?)?;
    }
    let mut spill_mask = 0u16;
    for (edge, branch) in node.branches.iter() {
        match &branch.value {
            BranchValue::Indirect(_) => children = add(children, 1)?,
            BranchValue::Terminal(value) => {
                checked_nibbles(&branch.prefix, MAX_INLINE_NIBBLES)?;
                payload = add(
                    add(add(payload, 2)?, branch.prefix.len())?,
                    value_codec::payload_len(value)?,
                )?;
                children = add(children, value_children(value)?)?;
            }
            BranchValue::Packed(_) => {
                if !branch.prefix.is_empty() {
                    return Err(corrupt("packed branch carries an outer tail"));
                }
                spill_mask |= 1u16 << edge;
                children = add(children, 1)?;
            }
        }
    }
    if payload > MAX_NODE_PAYLOAD || children > MAX_NODE_CHILDREN {
        return Err(IndexError::ResourceLimited);
    }
    let mut embed_mask = 0u16;
    let mut nested_embed = false;
    if spill_mask != 0 {
        for (edge, branch) in node.branches.iter() {
            let BranchValue::Packed(packed) = &branch.value else {
                continue;
            };
            let shape = packed.shape;
            let added_payload = add(add(2, shape.body_len)?, if embed_mask == 0 { 2 } else { 0 })?;
            let proposed_payload = add(payload, added_payload)?;
            let proposed_children = add(
                children.checked_sub(1).ok_or(IndexError::ResourceLimited)?,
                shape.child_count,
            )?;
            if proposed_payload <= MAX_NODE_PAYLOAD && proposed_children <= MAX_NODE_CHILDREN {
                payload = proposed_payload;
                children = proposed_children;
                let bit = 1u16 << edge;
                embed_mask |= bit;
                spill_mask &= !bit;
                nested_embed |= shape.depth > 1;
            }
        }
    }
    Ok(Layout {
        node,
        spill_mask,
        embed_mask,
        nested_embed,
        payload_capacity: payload,
        child_count: children,
    })
}

fn write_u16(payload: &mut Vec<u8>, value: usize) -> Result<(), IndexError> {
    payload.extend_from_slice(
        &u16::try_from(value)
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    Ok(())
}

fn write_value(
    value: &RetainedMapValue,
    payload: &mut Vec<u8>,
    children: &mut Vec<ObjectRef>,
) -> Result<(), IndexError> {
    children.extend_from_slice(value.children()?);
    value_codec::write(value, payload)
}

fn write_packed(
    node: &Node,
    payload: &mut Vec<u8>,
    children: &mut Vec<ObjectRef>,
    budget: &mut WorkBudget,
    depth: usize,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    if depth > MAX_PACKED_DEPTH {
        return Err(IndexError::ResourceLimited);
    }
    let nested = node
        .branches
        .iter()
        .any(|(_, branch)| matches!(branch.value, BranchValue::Packed(_)));
    payload.extend_from_slice(if nested { MAGIC_006 } else { MAGIC_004 });
    write_u16(payload, node.prefix.len())?;
    payload.extend_from_slice(&node.prefix);
    payload.push(u8::from(node.value.is_some()));
    let mut mask = 0u16;
    for (edge, _) in node.branches.iter() {
        mask |= 1u16 << edge;
    }
    let mut terminal = 0u16;
    let mut packed = 0u16;
    for (edge, branch) in node.branches.iter() {
        let bit = 1u16 << edge;
        match &branch.value {
            BranchValue::Terminal(_) => terminal |= bit,
            BranchValue::Packed(_) => packed |= bit,
            BranchValue::Indirect(_) => {
                return Err(corrupt("packed map body has an indirect child"));
            }
        }
    }
    write_u16(payload, usize::from(mask))?;
    write_u16(payload, usize::from(terminal))?;
    if nested {
        write_u16(payload, usize::from(packed))?;
    }
    if let Some(value) = node.value.as_ref() {
        write_value(value, payload, children)?;
    }
    for (_, branch) in node.branches.iter() {
        match &branch.value {
            BranchValue::Terminal(value) => {
                write_u16(payload, branch.prefix.len())?;
                payload.extend_from_slice(&branch.prefix);
                write_value(value, payload, children)?;
            }
            BranchValue::Packed(packed) if branch.prefix.is_empty() => {
                write_u16(payload, packed.shape.body_len)?;
                write_packed(&packed.node, payload, children, budget, add(depth, 1)?)?;
            }
            BranchValue::Indirect(_) | BranchValue::Packed(_) => {
                return Err(corrupt("packed map branch changed during encoding"));
            }
        }
    }
    Ok(())
}

pub(super) fn encode(
    selected: Layout<'_>,
    spills: &[Option<ObjectRef>; 16],
    budget: &mut WorkBudget,
) -> Result<EncodedNode, IndexError> {
    budget.checkpoint()?;
    let node = selected.node;
    for (edge, reference) in spills.iter().enumerate() {
        if reference.is_some() != (selected.spill_mask & (1u16 << edge) != 0) {
            return Err(IndexError::BackendViolation(
                "map node packed spill set differs".into(),
            ));
        }
    }
    let child_bytes = selected
        .child_count
        .checked_mul(std::mem::size_of::<ObjectRef>())
        .ok_or(IndexError::ResourceLimited)?;
    let allocation = add(selected.payload_capacity, child_bytes)?;
    budget.charge(WorkUsage {
        bytes: u64::try_from(allocation).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let memory = budget.reserve_memory(
        u64::try_from(allocation).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut children = Vec::new();
    children
        .try_reserve_exact(selected.child_count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(selected.payload_capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    payload.extend_from_slice(if selected.nested_embed {
        MAGIC_006
    } else if selected.embed_mask != 0 {
        MAGIC_005
    } else {
        MAGIC_004
    });
    write_u16(&mut payload, node.prefix.len())?;
    payload.extend_from_slice(&node.prefix);
    payload.push(u8::from(node.value.is_some()));
    let mut mask = 0u16;
    let mut terminal = 0u16;
    for (edge, branch) in node.branches.iter() {
        let bit = 1u16 << edge;
        mask |= bit;
        if matches!(&branch.value, BranchValue::Terminal(_)) {
            terminal |= bit;
        }
    }
    write_u16(&mut payload, usize::from(mask))?;
    write_u16(&mut payload, usize::from(terminal))?;
    if selected.embed_mask != 0 {
        write_u16(&mut payload, usize::from(selected.embed_mask))?;
    }
    if let Some(value) = node.value.as_ref() {
        write_value(value, &mut payload, &mut children)?;
    }
    for (edge, branch) in node.branches.iter() {
        match &branch.value {
            BranchValue::Indirect(reference) => children.push(*reference),
            BranchValue::Terminal(value) => {
                write_u16(&mut payload, branch.prefix.len())?;
                payload.extend_from_slice(&branch.prefix);
                write_value(value, &mut payload, &mut children)?;
            }
            BranchValue::Packed(packed) if selected.embed_mask & (1u16 << edge) != 0 => {
                write_u16(&mut payload, packed.shape.body_len)?;
                let start = payload.len();
                let child_start = children.len();
                write_packed(&packed.node, &mut payload, &mut children, budget, 1)?;
                if payload.len().checked_sub(start) != Some(packed.shape.body_len)
                    || children.len().checked_sub(child_start) != Some(packed.shape.child_count)
                {
                    return Err(IndexError::BackendViolation(
                        "cached packed map shape differs from encoded body".into(),
                    ));
                }
            }
            BranchValue::Packed(_) => children.push(spills[edge].ok_or_else(|| {
                IndexError::BackendViolation("packed map branch lacks spill".into())
            })?),
        }
    }
    if payload.len() != selected.payload_capacity || children.len() != selected.child_count {
        return Err(IndexError::BackendViolation(
            "map node layout differs from encoded wire".into(),
        ));
    }
    Ok(EncodedNode {
        payload,
        children,
        _memory: memory,
    })
}

fn read_u16(payload: &[u8], at: usize) -> Result<usize, IndexError> {
    let end = at.checked_add(2).ok_or(IndexError::ResourceLimited)?;
    let raw: [u8; 2] = payload
        .get(at..end)
        .ok_or_else(|| corrupt("truncated map length or mask"))?
        .try_into()
        .map_err(|_error| corrupt("invalid map length"))?;
    Ok(usize::from(u16::from_be_bytes(raw)))
}

fn read_wire(
    payload: &[u8],
    references: &mut impl Iterator<Item = ObjectRef>,
    budget: &mut WorkBudget,
    depth: usize,
) -> Result<Node, IndexError> {
    budget.checkpoint()?;
    if depth > MAX_PACKED_DEPTH || payload.len() > MAX_NODE_PAYLOAD {
        return Err(corrupt("packed map depth or body exceeds bound"));
    }
    let nested = depth != 0;
    let magic = payload
        .get(..8)
        .ok_or_else(|| corrupt("truncated map magic"))?;
    let version5 = magic == MAGIC_005;
    let version6 = magic == MAGIC_006;
    if magic != MAGIC_004 && !version5 && !version6 {
        return Err(corrupt("invalid map node format"));
    }
    if nested && version5 {
        return Err(corrupt("legacy packed map body has nested format"));
    }
    let prefix_len = read_u16(payload, 8)?;
    if prefix_len
        > if nested {
            MAX_INLINE_NIBBLES
        } else {
            MAX_NIBBLES
        }
    {
        return Err(corrupt("map prefix exceeds key limit"));
    }
    let flag_at = add(10, prefix_len)?;
    let flag = *payload
        .get(flag_at)
        .ok_or_else(|| corrupt("truncated map value flag"))?;
    if flag > 1 {
        return Err(corrupt("invalid map value flag"));
    }
    let mask_at = add(flag_at, 1)?;
    let mask =
        u16::try_from(read_u16(payload, mask_at)?).map_err(|_error| IndexError::ResourceLimited)?;
    let inline_at = add(mask_at, 2)?;
    let inline = u16::try_from(read_u16(payload, inline_at)?)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let packed_at = add(inline_at, 2)?;
    let packed = if version5 || version6 {
        u16::try_from(read_u16(payload, packed_at)?)
            .map_err(|_error| IndexError::ResourceLimited)?
    } else {
        0
    };
    if inline & !mask != 0 || packed & !mask != 0 || packed & inline != 0 {
        return Err(corrupt("map branch masks disagree"));
    }
    if (version5 || version6) && packed == 0 {
        return Err(corrupt("empty packed bitmap is not canonical"));
    }
    if flag == 0 && mask == 0 {
        return Err(corrupt("empty map node"));
    }
    if flag == 0 && mask.is_power_of_two() {
        return Err(corrupt("uncompressed unary map node"));
    }
    if nested && (mask == 0 || mask != (inline | packed)) {
        return Err(corrupt("packed map body has an indirect branch"));
    }
    if nested && magic == MAGIC_004 && inline != mask {
        return Err(corrupt(
            "legacy packed map body is not terminal-only branching",
        ));
    }
    let prefix = payload
        .get(10..flag_at)
        .ok_or_else(|| corrupt("truncated map prefix"))?;
    checked_nibbles(
        prefix,
        if nested {
            MAX_INLINE_NIBBLES
        } else {
            MAX_NIBBLES
        },
    )?;
    let mut at = add(packed_at, if version5 || version6 { 2 } else { 0 })?;
    let value = if flag == 1 {
        Some(value_codec::read(payload, &mut at, references, budget)?)
    } else {
        None
    };
    let mut branches = Branches::for_decode(mask, budget)?;
    let mut has_nested_child = false;
    for edge in 0..16 {
        budget.checkpoint()?;
        let bit = 1u16 << edge;
        if mask & bit == 0 {
            continue;
        }
        let branch = if inline & bit != 0 {
            let length = read_u16(payload, at)?;
            at = add(at, 2)?;
            let end = add(at, length)?;
            let tail = payload
                .get(at..end)
                .ok_or_else(|| corrupt("truncated inline map tail"))?;
            checked_nibbles(tail, MAX_INLINE_NIBBLES)?;
            at = end;
            Branch::terminal(
                MapBuffer::copy(tail, MAX_INLINE_NIBBLES, budget)?,
                value_codec::read(payload, &mut at, references, budget)?,
            )
        } else if packed & bit != 0 {
            let length = read_u16(payload, at)?;
            at = add(at, 2)?;
            if length == 0 {
                return Err(corrupt("empty packed map body"));
            }
            let end = add(at, length)?;
            let body = payload
                .get(at..end)
                .ok_or_else(|| corrupt("truncated packed map body"))?;
            let child_magic = body
                .get(..8)
                .ok_or_else(|| corrupt("truncated packed map magic"))?;
            if (version5 && child_magic != MAGIC_004)
                || (version6 && child_magic != MAGIC_004 && child_magic != MAGIC_006)
            {
                return Err(corrupt("packed map body version disagrees"));
            }
            has_nested_child |= child_magic == MAGIC_006;
            let node = read_wire(body, references, budget, add(depth, 1)?)?;
            at = end;
            Branch::packed(node, budget)?
        } else {
            Branch::indirect(
                references
                    .next()
                    .ok_or_else(|| corrupt("missing map edge"))?,
            )
        };
        branches.append_decoded(edge, branch, budget)?;
    }
    if at != payload.len() {
        return Err(corrupt("map node has trailing bytes"));
    }
    if version6 && !nested && !has_nested_child {
        return Err(corrupt("nested map version lacks a nested child"));
    }
    let node = Node {
        prefix: MapBuffer::copy(prefix, MAX_NIBBLES, budget)?,
        value,
        branches,
    };
    Ok(node)
}

pub(super) fn decode(stored: &StoredObject, budget: &mut WorkBudget) -> Result<Node, IndexError> {
    let payload = &stored.payload;
    if matches!(payload.get(..8), Some(magic) if magic == b"QIMAP001" || magic == b"QIMAP002" || magic == b"QIMAP003")
    {
        return Err(IndexError::RebuildRequired);
    }
    if payload.len() > MAX_NODE_PAYLOAD || stored.registration.children.len() > MAX_NODE_CHILDREN {
        return Err(corrupt("map node exceeds physical bound"));
    }
    let mut references = stored.registration.children.iter().copied();
    let node = read_wire(payload, &mut references, budget, 0)?;
    if references.next().is_some() {
        return Err(corrupt("map node has trailing physical edges"));
    }
    Ok(node)
}
