//! A deterministic compressed radix tree. A mutation reads and rewrites only
//! the changed key path; retained roots keep their immutable subtrees.

mod scratch;
use scratch::MapScratch;

use quanta_index_core::index::{
    IndexError, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapRoot, MapWrite, ObjectRef,
    OrdinalMapEdit, StoredObject, WorkBudget, WorkUsage,
    ports::{ImmutableObjectPort, MapEditCursor, ObjectWriteSink, PersistentMapPort},
};

use crate::{
    FileObjectStore,
    object::{MAX_BATCH_BYTES, MAX_BATCH_INPUTS, PreparedObject},
};

const NODE_MAGIC: &[u8; 8] = b"QIMAP001";
const MAX_KEY_BYTES: usize = 4096;
const MAX_NIBBLES: usize = MAX_KEY_BYTES * 2;
const MAX_SCAN_LIMIT: u32 = 4096;
const MAX_BATCH_ROOTS: usize = 64;

#[derive(Clone)]
pub(crate) struct Node {
    /// Nibbles following the parent's branch nibble.
    prefix: Vec<u8>,
    value: Option<ObjectRef>,
    branches: [Option<ObjectRef>; 16],
}

impl Node {
    fn leaf(prefix: &[u8], value: ObjectRef) -> Self {
        Self {
            prefix: prefix.to_vec(),
            value: Some(value),
            branches: [None; 16],
        }
    }

    fn branch_count(&self) -> usize {
        self.branches
            .iter()
            .filter(|branch| branch.is_some())
            .count()
    }

    fn sole_branch(&self) -> Option<(usize, ObjectRef)> {
        let mut found = None;
        for (index, branch) in self.branches.iter().enumerate() {
            if let Some(reference) = branch {
                if found.is_some() {
                    return None;
                }
                found = Some((index, *reference));
            }
        }
        found
    }
}

struct StagedMap {
    scratch: MapScratch,
}

/// One open path in a sorted, empty-root radix build. Completed siblings are
/// already content-addressed in scratch; no intermediate root is installed.
struct BootstrapFrame {
    full_prefix: Vec<u8>,
    value: Option<ObjectRef>,
    branches: [Option<ObjectRef>; 16],
}

/// A radix node on the last changed key's open path. Sorted edits cannot
/// return to a closed child, so shared ancestors are encoded only on close.
struct EditFrame {
    node: Node,
    reference: ObjectRef,
    end: usize,
    parent_edge: Option<usize>,
    dirty: bool,
}

struct OpenPathStart {
    reference: Option<ObjectRef>,
    offset: usize,
    parent_edge: Option<usize>,
}

impl BootstrapFrame {
    fn new(full_prefix: Vec<u8>, value: Option<ObjectRef>) -> Self {
        Self {
            full_prefix,
            value,
            branches: [None; 16],
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
        })
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
        object: &PreparedObject,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        self.scratch.insert_node(object, budget)?;
        Ok(object.registration.reference)
    }
}

fn flush_reachable_chunk(
    store: &FileObjectStore,
    chunk: &mut Vec<PreparedObject>,
    sink: &dyn ObjectWriteSink,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if chunk.is_empty() {
        return Ok(());
    }
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
    let references: Vec<&PreparedObject> = chunk.iter().collect();
    store.install_prepared_batch(&references, sink, budget)?;
    chunk.clear();
    Ok(())
}

impl FileObjectStore {
    /// The final reachable DAG is already deduplicated and child-first. Commit
    /// its objects in bounded custody groups without publishing intermediate
    /// map roots. A later group may depend only on earlier registered groups.
    fn install_reachable_map_nodes(
        &self,
        roots: &[MapRoot],
        staged: &StagedMap,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let mut pending = Vec::<(ObjectRef, bool, usize)>::new();
        let mut chunk = Vec::<PreparedObject>::new();
        let mut chunk_bytes = 0u64;
        for root in roots {
            if let Some(reference) = root.0 {
                budget.charge(WorkUsage {
                    bytes: u64::try_from(std::mem::size_of::<(ObjectRef, bool, usize)>())
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                pending.push((reference, false, 0));
            }
            while let Some((reference, exiting, depth)) = pending.pop() {
                budget.checkpoint()?;
                let Some(object) = staged.scratch.prepared(reference, budget)? else {
                    continue;
                };
                if exiting {
                    if staged.scratch.mark(reference, budget)? != Some(1) {
                        return Err(IndexError::CorruptObject(
                            "invalid staged map traversal state".into(),
                        ));
                    }
                    staged.scratch.set_mark(reference, 2, budget)?;
                    let next_bytes = chunk_bytes
                        .checked_add(object.registration.bytes)
                        .ok_or(IndexError::ResourceLimited)?;
                    if chunk.len() == MAX_BATCH_INPUTS || next_bytes > MAX_BATCH_BYTES {
                        flush_reachable_chunk(self, &mut chunk, sink, budget)?;
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
                    chunk.push(object);
                    continue;
                }
                match staged.scratch.mark(reference, budget)? {
                    Some(2) => continue,
                    Some(1) => {
                        return Err(IndexError::CorruptObject("cyclic staged map DAG".into()));
                    }
                    Some(_) => {
                        return Err(IndexError::CorruptObject("invalid staged map mark".into()));
                    }
                    None => {}
                }
                if depth > MAX_NIBBLES {
                    return Err(IndexError::CorruptObject(
                        "staged map DAG exceeds key depth".into(),
                    ));
                }
                staged.scratch.set_mark(reference, 1, budget)?;
                budget.charge(WorkUsage {
                    bytes: u64::try_from(std::mem::size_of::<(ObjectRef, bool, usize)>())
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                pending.push((reference, true, depth));
                for child in object.registration.children.iter().rev() {
                    if staged.scratch.prepared(*child, budget)?.is_some() {
                        let next_depth = depth.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                        budget.charge(WorkUsage {
                            bytes: u64::try_from(std::mem::size_of::<(ObjectRef, bool, usize)>())
                                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                            ..WorkUsage::default()
                        })?;
                        pending.push((*child, false, next_depth));
                    }
                }
            }
        }
        flush_reachable_chunk(self, &mut chunk, sink, budget)
    }

    pub(crate) fn read_node(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Node, IndexError> {
        decode_node(self.get(reference, budget)?)
    }
}

pub(crate) fn decode_node(stored: StoredObject) -> Result<Node, IndexError> {
    let payload = &stored.payload;
    if payload.get(..8) != Some(NODE_MAGIC.as_slice()) {
        return Err(IndexError::CorruptObject(
            "map root does not reference a map node".into(),
        ));
    }
    let prefix_len = usize::from(u16::from_be_bytes(
        payload
            .get(8..10)
            .ok_or_else(|| IndexError::CorruptObject("truncated map header".into()))?
            .try_into()
            .map_err(|_conversion_error| {
                IndexError::CorruptObject("invalid map prefix length".into())
            })?,
    ));
    if prefix_len > MAX_NIBBLES {
        return Err(IndexError::CorruptObject(
            "map prefix exceeds key limit".into(),
        ));
    }
    let flag_at = 10usize
        .checked_add(prefix_len)
        .ok_or(IndexError::ResourceLimited)?;
    let flag = *payload
        .get(flag_at)
        .ok_or_else(|| IndexError::CorruptObject("truncated map value flag".into()))?;
    if flag > 1 {
        return Err(IndexError::CorruptObject("invalid map value flag".into()));
    }
    let mask_at = flag_at.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    let mask_end = mask_at.checked_add(2).ok_or(IndexError::ResourceLimited)?;
    let mask = u16::from_be_bytes(
        payload
            .get(mask_at..mask_end)
            .ok_or_else(|| IndexError::CorruptObject("truncated map branch mask".into()))?
            .try_into()
            .map_err(|_conversion_error| {
                IndexError::CorruptObject("invalid map branch mask".into())
            })?,
    );
    if payload.len() != mask_end {
        return Err(IndexError::CorruptObject(
            "map node has trailing bytes".into(),
        ));
    }
    let prefix = payload
        .get(10..flag_at)
        .ok_or_else(|| IndexError::CorruptObject("truncated map prefix".into()))?
        .to_vec();
    if prefix.iter().any(|nibble| *nibble > 15) {
        return Err(IndexError::CorruptObject(
            "map prefix contains a non-nibble".into(),
        ));
    }
    let child_count = usize::from(flag)
        .checked_add(
            usize::try_from(mask.count_ones())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?;
    if stored.registration.children.len() != child_count || child_count == 0 {
        return Err(IndexError::CorruptObject(
            "map child count does not match node".into(),
        ));
    }
    if flag == 0 && mask.is_power_of_two() {
        return Err(IndexError::CorruptObject(
            "uncompressed unary map node".into(),
        ));
    }
    let mut references = stored.registration.children.into_iter();
    let value = if flag == 1 { references.next() } else { None };
    let mut branches = [None; 16];
    for (index, branch) in branches.iter_mut().enumerate() {
        if mask & (1u16 << index) != 0 {
            *branch = references.next();
        }
    }
    Ok(Node {
        prefix,
        value,
        branches,
    })
}

impl FileObjectStore {
    /// Seal one completed sorted-build path and attach it to its immediate
    /// open parent. The stack length, rather than recursion depth, bounds the
    /// build even for maximum-length keys.
    fn seal_bootstrap_frame(
        &self,
        frames: &mut Vec<BootstrapFrame>,
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
                &Node {
                    prefix: prefix.to_vec(),
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
            let branch = parent.branches.get_mut(edge).ok_or_else(|| {
                IndexError::BackendViolation("sorted map parent edge is invalid".into())
            })?;
            if branch.replace(reference).is_some() {
                return Err(IndexError::BackendViolation(
                    "sorted map parent edge was assigned twice".into(),
                ));
            }
            Ok(None)
        } else {
            Ok(Some(reference))
        }
    }

    /// Sorted initial ingest builds each affected radix prefix once. The
    /// cursor is already validated and spooled against the fixed empty base.
    fn build_empty_root(
        &self,
        ordinal: u32,
        staged: &StagedMap,
        budget: &mut WorkBudget,
    ) -> Result<MapRoot, IndexError> {
        let mut frames = Vec::<BootstrapFrame>::new();
        let mut after = None;
        while let Some(edit) = staged
            .scratch
            .next_edit(ordinal, after.as_deref(), budget)?
        {
            budget.checkpoint()?;
            budget.charge(WorkUsage {
                bytes: u64::try_from(edit.key.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            after = Some(edit.key.clone());
            let Some(value) = edit.next else {
                continue;
            };
            charge_key_allocation(&edit.key, budget)?;
            let nibbles = key_nibbles(&edit.key)?;
            let shared = frames.last().map_or(0, |last| {
                last.full_prefix
                    .iter()
                    .zip(&nibbles)
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
                    frames.push(BootstrapFrame::new(
                        nibbles
                            .get(..shared)
                            .ok_or(IndexError::ResourceLimited)?
                            .to_vec(),
                        None,
                    ));
                    frames.push(child);
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
            frames.push(BootstrapFrame::new(nibbles, Some(value)));
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
        node: &Node,
        staged: &StagedMap,
        budget: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        if node.value.is_none() {
            match node.branch_count() {
                0 => return Ok(None),
                1 => {
                    let (edge, reference) = node.sole_branch().ok_or_else(|| {
                        IndexError::BackendViolation("unary map branch missing".into())
                    })?;
                    let child = staged.read_node(self, reference, budget)?;
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
                    let mut prefix = Vec::with_capacity(capacity);
                    prefix.extend_from_slice(&node.prefix);
                    prefix.push(
                        u8::try_from(edge)
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    );
                    prefix.extend_from_slice(&child.prefix);
                    return self
                        .write_encoded(
                            &Node {
                                prefix,
                                value: child.value,
                                branches: child.branches,
                            },
                            staged,
                            budget,
                        )
                        .map(Some);
                }
                _ => {}
            }
        }
        self.write_encoded(node, staged, budget).map(Some)
    }

    fn write_encoded(
        &self,
        node: &Node,
        staged: &StagedMap,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        if node.prefix.len() > MAX_NIBBLES || node.prefix.iter().any(|nibble| *nibble > 15) {
            return Err(IndexError::BackendViolation(
                "invalid map node prefix".into(),
            ));
        }
        let payload_capacity = 13usize
            .checked_add(node.prefix.len())
            .ok_or(IndexError::ResourceLimited)?;
        let children_capacity = 17usize
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(
                payload_capacity
                    .checked_add(children_capacity)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut mask = 0u16;
        let mut children = Vec::with_capacity(17);
        if let Some(value) = node.value {
            children.push(value);
        }
        for (index, branch) in node.branches.iter().enumerate() {
            if let Some(reference) = branch {
                mask |= 1u16 << index;
                children.push(*reference);
            }
        }
        let mut payload = Vec::with_capacity(payload_capacity);
        payload.extend_from_slice(NODE_MAGIC);
        payload.extend_from_slice(
            &u16::try_from(node.prefix.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        payload.extend_from_slice(&node.prefix);
        payload.push(u8::from(node.value.is_some()));
        payload.extend_from_slice(&mask.to_be_bytes());
        let object = self.prepare_object(&payload, &children, budget)?;
        staged.insert(&object, budget)
    }

    fn lookup_inner(
        &self,
        root: MapRoot,
        nibbles: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        let mut cursor = root.0;
        let mut at = 0usize;
        while let Some(reference) = cursor {
            let node = self.read_node(reference, budget)?;
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
                return Ok(node.value);
            }
            let branch =
                usize::from(*nibbles.get(at).ok_or_else(|| {
                    IndexError::CorruptObject("map traversal passed key end".into())
                })?);
            at = at.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            cursor = *node
                .branches
                .get(branch)
                .ok_or_else(|| IndexError::CorruptObject("map branch out of bounds".into()))?;
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
        next: Option<ObjectRef>,
        staged: &StagedMap,
        budget: &mut WorkBudget,
    ) -> Result<MapRoot, IndexError> {
        let mut trail: Vec<(Node, usize)> = Vec::new();
        let mut at = 0usize;
        let mut cursor = root.0;
        let mut replacement = loop {
            let Some(reference) = cursor else {
                let new_leaf = match next {
                    Some(value) => {
                        let suffix = nibbles.get(at..).ok_or_else(|| {
                            IndexError::CorruptObject("map traversal passed key end".into())
                        })?;
                        budget.charge(WorkUsage {
                            bytes: u64::try_from(suffix.len())
                                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                            ..WorkUsage::default()
                        })?;
                        self.write_node(&Node::leaf(suffix, value), staged, budget)?
                    }
                    None => None,
                };
                break new_leaf;
            };
            let mut node = staged.read_node(self, reference, budget)?;
            let remaining = nibbles
                .get(at..)
                .ok_or_else(|| IndexError::CorruptObject("map traversal passed key end".into()))?;
            let shared = node
                .prefix
                .iter()
                .zip(remaining)
                .take_while(|(left, right)| left == right)
                .count();
            if shared < node.prefix.len() {
                let Some(value) = next else {
                    return Ok(root);
                };
                let old_edge =
                    usize::from(*node.prefix.get(shared).ok_or_else(|| {
                        IndexError::CorruptObject("missing map split edge".into())
                    })?);
                budget.charge(WorkUsage {
                    bytes: u64::try_from(node.prefix.len())
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                let common = node
                    .prefix
                    .get(..shared)
                    .ok_or_else(|| IndexError::CorruptObject("missing map common prefix".into()))?
                    .to_vec();
                let suffix_at = shared.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                node.prefix = node
                    .prefix
                    .get(suffix_at..)
                    .ok_or_else(|| IndexError::CorruptObject("missing old map suffix".into()))?
                    .to_vec();
                let old_reference = self.write_encoded(&node, staged, budget)?;
                let mut parent = Node {
                    prefix: common,
                    value: None,
                    branches: [None; 16],
                };
                *parent.branches.get_mut(old_edge).ok_or_else(|| {
                    IndexError::CorruptObject("old map split edge out of bounds".into())
                })? = Some(old_reference);
                let new_at = at.checked_add(shared).ok_or(IndexError::ResourceLimited)?;
                if new_at == nibbles.len() {
                    parent.value = Some(value);
                } else {
                    let edge =
                        usize::from(*nibbles.get(new_at).ok_or_else(|| {
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
                    *parent.branches.get_mut(edge).ok_or_else(|| {
                        IndexError::CorruptObject("new map split edge out of bounds".into())
                    })? = self.write_node(&Node::leaf(suffix, value), staged, budget)?;
                }
                break self.write_node(&parent, staged, budget)?;
            }
            at = at
                .checked_add(node.prefix.len())
                .ok_or(IndexError::ResourceLimited)?;
            if at == nibbles.len() {
                node.value = next;
                break self.write_node(&node, staged, budget)?;
            }
            let edge = usize::from(
                *nibbles
                    .get(at)
                    .ok_or_else(|| IndexError::CorruptObject("missing map branch edge".into()))?,
            );
            at = at.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            let child = *node
                .branches
                .get(edge)
                .ok_or_else(|| IndexError::CorruptObject("map branch out of bounds".into()))?;
            if child.is_none() {
                let Some(value) = next else {
                    return Ok(root);
                };
                let suffix = nibbles
                    .get(at..)
                    .ok_or_else(|| IndexError::CorruptObject("missing map child suffix".into()))?;
                budget.charge(WorkUsage {
                    bytes: u64::try_from(suffix.len())
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                *node.branches.get_mut(edge).ok_or_else(|| {
                    IndexError::CorruptObject("map branch out of bounds".into())
                })? = self.write_node(&Node::leaf(suffix, value), staged, budget)?;
                break self.write_node(&node, staged, budget)?;
            }
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<(Node, usize)>())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            trail.push((node, edge));
            cursor = child;
        };
        while let Some((mut parent, edge)) = trail.pop() {
            *parent.branches.get_mut(edge).ok_or_else(|| {
                IndexError::CorruptObject("map trail branch out of bounds".into())
            })? = replacement;
            replacement = self.write_node(&parent, staged, budget)?;
        }
        Ok(MapRoot(replacement))
    }

    fn close_edit_frame(
        &self,
        frames: &mut Vec<EditFrame>,
        root: &mut MapRoot,
        staged: &StagedMap,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let frame = frames
            .pop()
            .ok_or_else(|| IndexError::BackendViolation("open map edit path is empty".into()))?;
        let replacement = if frame.dirty {
            self.write_node(&frame.node, staged, budget)?
        } else {
            Some(frame.reference)
        };
        if let Some(parent) = frames.last_mut() {
            let edge = frame.parent_edge.ok_or_else(|| {
                IndexError::BackendViolation("open map child has no parent edge".into())
            })?;
            let branch = parent.node.branches.get_mut(edge).ok_or_else(|| {
                IndexError::CorruptObject("open map parent edge out of bounds".into())
            })?;
            if *branch != replacement {
                *branch = replacement;
                parent.dirty = true;
            }
        } else {
            if frame.parent_edge.is_some() {
                return Err(IndexError::BackendViolation(
                    "open map root retained a parent edge".into(),
                ));
            }
            *root = MapRoot(replacement);
        }
        Ok(())
    }

    fn open_edit_path(
        &self,
        start: &OpenPathStart,
        nibbles: &[u8],
        frames: &mut Vec<EditFrame>,
        staged: &StagedMap,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let mut cursor = start.reference;
        let mut offset = start.offset;
        let mut parent_edge = start.parent_edge;
        while let Some(reference) = cursor {
            budget.checkpoint()?;
            if offset > MAX_NIBBLES {
                return Err(IndexError::CorruptObject(
                    "open map edit path exceeds key limit".into(),
                ));
            }
            let node = staged.read_node(self, reference, budget)?;
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
                Some((
                    edge,
                    *node.branches.get(edge).ok_or_else(|| {
                        IndexError::CorruptObject("open map branch edge out of bounds".into())
                    })?,
                ))
            } else {
                None
            };
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<EditFrame>())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            frames.push(EditFrame {
                node,
                reference,
                end,
                parent_edge,
                dirty: false,
            });
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
        let mut frames = Vec::<EditFrame>::new();
        let mut previous_nibbles: Option<Vec<u8>> = None;
        let mut after = None;
        while let Some(edit) = staged
            .scratch
            .next_edit(ordinal, after.as_deref(), budget)?
        {
            budget.checkpoint()?;
            if edit.expected != edit.next {
                charge_key_allocation(&edit.key, budget)?;
                let nibbles = key_nibbles(&edit.key)?;
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
                    root = self.update_one(root, &nibbles, edit.next, staged, budget)?;
                    self.open_edit_path(
                        &OpenPathStart {
                            reference: root.0,
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
                        frame.node.value = edit.next;
                        frame.dirty = true;
                    } else {
                        let edge = usize::from(*nibbles.get(end).ok_or_else(|| {
                            IndexError::CorruptObject("open map child edge is missing".into())
                        })?);
                        let child = *frames
                            .last()
                            .and_then(|frame| frame.node.branches.get(edge))
                            .ok_or_else(|| {
                                IndexError::CorruptObject(
                                    "open map child edge out of bounds".into(),
                                )
                            })?;
                        let offset = end.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                        let suffix = nibbles.get(offset..).ok_or_else(|| {
                            IndexError::CorruptObject("open map child suffix is missing".into())
                        })?;
                        let replacement = self
                            .update_one(MapRoot(child), suffix, edit.next, staged, budget)?
                            .0;
                        let frame = frames.last_mut().ok_or_else(|| {
                            IndexError::BackendViolation("open map parent path lost".into())
                        })?;
                        if child != replacement {
                            *frame.node.branches.get_mut(edge).ok_or_else(|| {
                                IndexError::CorruptObject(
                                    "open map child edge out of bounds".into(),
                                )
                            })? = replacement;
                            frame.dirty = true;
                        }
                        self.open_edit_path(
                            &OpenPathStart {
                                reference: replacement,
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
            after = Some(edit.key);
        }
        while !frames.is_empty() {
            self.close_edit_frame(&mut frames, &mut root, staged, budget)?;
        }
        Ok(root)
    }
}

impl PersistentMapPort for FileObjectStore {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        budget.checkpoint()?;
        charge_key_allocation(key, budget)?;
        let nibbles = key_nibbles(key)?;
        self.lookup_inner(root, &nibbles, budget)
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        if limit == 0 || limit > MAX_SCAN_LIMIT {
            return Err(IndexError::InvalidInput(
                "map scan limit is out of bounds".into(),
            ));
        }
        budget.checkpoint()?;
        let after_nibbles = if let Some(after) = after {
            charge_key_allocation(after, budget)?;
            Some(key_nibbles(after)?)
        } else {
            None
        };
        let mut stack = Vec::new();
        if let Some(reference) = root.0 {
            stack.push((reference, Vec::new()));
        }
        let mut entries = Vec::new();
        let limit =
            usize::try_from(limit).map_err(|_conversion_error| IndexError::ResourceLimited)?;
        while let Some((reference, mut path)) = stack.pop() {
            let node = self.read_node(reference, budget)?;
            if path
                .len()
                .checked_add(node.prefix.len())
                .is_none_or(|length| length > MAX_NIBBLES)
            {
                return Err(IndexError::CorruptObject(
                    "map path exceeds key limit".into(),
                ));
            }
            budget.charge(WorkUsage {
                bytes: u64::try_from(node.prefix.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            path.extend_from_slice(&node.prefix);
            if !subtree_can_follow(&path, after_nibbles.as_deref()) {
                continue;
            }
            if let Some(value) = node.value {
                if !path.len().is_multiple_of(2) {
                    return Err(IndexError::CorruptObject(
                        "map value ends at an incomplete byte".into(),
                    ));
                }
                if after_nibbles
                    .as_ref()
                    .is_none_or(|after| path.as_slice() > after.as_slice())
                {
                    if entries.len() == limit {
                        return Ok(MapPage {
                            entries,
                            has_more: true,
                        });
                    }
                    budget.charge(WorkUsage {
                        bytes: u64::try_from(path.len() >> 1)
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?
                            .checked_add(
                                u64::try_from(std::mem::size_of::<(Vec<u8>, ObjectRef)>())
                                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                            )
                            .ok_or(IndexError::ResourceLimited)?,
                        ..WorkUsage::default()
                    })?;
                    entries.push((nibbles_to_key(&path)?, value));
                }
            }
            for (edge, child) in node.branches.iter().enumerate().rev() {
                if let Some(child) = child {
                    let child_path_len = path.len().checked_add(1).ok_or_else(|| {
                        IndexError::CorruptObject("map path length overflow".into())
                    })?;
                    if child_path_len > MAX_NIBBLES {
                        return Err(IndexError::CorruptObject(
                            "map branch exceeds key limit".into(),
                        ));
                    }
                    budget.charge(WorkUsage {
                        bytes: u64::try_from(child_path_len)
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                        ..WorkUsage::default()
                    })?;
                    let mut child_path = path.clone();
                    child_path.push(
                        u8::try_from(edge)
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    );
                    if subtree_can_follow(&child_path, after_nibbles.as_deref()) {
                        stack.push((*child, child_path));
                    }
                }
            }
        }
        Ok(MapPage {
            entries,
            has_more: false,
        })
    }

    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
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
        let bases: Vec<MapRoot> = requests.iter().map(|request| request.base).collect();
        let edits: Vec<&[MapEdit]> = requests
            .iter()
            .map(|request| request.edits.as_slice())
            .collect();
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
    ) -> Result<Vec<MapRoot>, IndexError> {
        if bases.is_empty() || bases.len() > MAX_BATCH_ROOTS {
            return Err(IndexError::ResourceLimited);
        }
        let staged = StagedMap::open(self, budget)?;
        let mut previous: Option<(u32, Vec<u8>)> = None;
        while let Some(ordinal) = edits.next_edit(budget)? {
            budget.checkpoint()?;
            let root_index = usize::try_from(ordinal.root)
                .map_err(|_conversion_error| IndexError::ResourceLimited)?;
            let base = *bases.get(root_index).ok_or_else(|| {
                IndexError::InvalidInput("map edit root ordinal is out of bounds".into())
            })?;
            if ordinal.edit.key.len() > MAX_KEY_BYTES
                || previous.as_ref().is_some_and(|(root, key)| {
                    *root > ordinal.root
                        || (*root == ordinal.root && key.as_slice() >= ordinal.edit.key.as_slice())
                })
            {
                return Err(IndexError::InvalidInput(
                    "map edits must be globally sorted and unique".into(),
                ));
            }
            budget.charge(WorkUsage {
                mutations: 1,
                bytes: u64::try_from(ordinal.edit.key.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            charge_key_allocation(&ordinal.edit.key, budget)?;
            let nibbles = key_nibbles(&ordinal.edit.key)?;
            if self.lookup_inner(base, &nibbles, budget)? != ordinal.edit.expected {
                return Err(IndexError::Conflict);
            }
            previous = Some((ordinal.root, ordinal.edit.key.clone()));
            staged.scratch.insert_edit(&ordinal, budget)?;
        }
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
        let mut roots = Vec::with_capacity(bases.len());
        for (index, base) in bases.iter().enumerate() {
            budget.checkpoint()?;
            let ordinal =
                u32::try_from(index).map_err(|_conversion_error| IndexError::ResourceLimited)?;
            if base.0.is_none() {
                roots.push(self.build_empty_root(ordinal, &staged, budget)?);
                continue;
            }
            roots.push(self.apply_sorted_existing_root(ordinal, *base, &staged, budget)?);
        }
        self.install_reachable_map_nodes(&roots, &staged, sink, budget)?;
        Ok(roots)
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
                return Ok(Some(OrdinalMapEdit {
                    root,
                    edit: edit.clone(),
                }));
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
    let mut key = Vec::with_capacity(nibbles.len() >> 1);
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

fn key_nibbles(key: &[u8]) -> Result<Vec<u8>, IndexError> {
    if key.len() > MAX_KEY_BYTES {
        return Err(IndexError::InvalidInput(
            "map key exceeds physical limit".into(),
        ));
    }
    let capacity = key
        .len()
        .checked_mul(2)
        .ok_or(IndexError::ResourceLimited)?;
    let mut nibbles = Vec::with_capacity(capacity);
    for byte in key {
        nibbles.push(byte >> 4);
        nibbles.push(byte & 0x0f);
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
            if objects.len() > MAX_BATCH_INPUTS || bytes > MAX_BATCH_BYTES {
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

    fn budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 256 * 1024 * 1024,
                mutations: 100_000,
            },
            Instant::now() + Duration::from_secs(30),
        )
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
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    #[test]
    fn storage_rejects_public_and_symlinked_paths() -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget();
        let reference = value(&store, b"private", &mut work)?;
        let shard = store.path_for(reference);
        let object_metadata = fs::metadata(&shard)?;
        assert_eq!(object_metadata.permissions().mode() & 0o7777, 0o600);
        assert_eq!(
            fs::metadata(&directory.0)?.permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(
            fs::metadata(directory.0.join("objects"))?
                .permissions()
                .mode()
                & 0o7777,
            0o700
        );
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755))?;
        assert!(FileObjectStore::open(&directory.0).is_err());
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700))?;

        let link = TestDir::new();
        symlink(&directory.0, &link.0)?;
        assert!(FileObjectStore::open(&link.0).is_err());
        fs::remove_file(&link.0)?;

        let linked_shard = TestDir::new();
        let linked_store = FileObjectStore::open(&linked_shard.0)?;
        let mut linked_work = budget();
        let prepared = linked_store.prepare_object(b"shard", &[], &mut linked_work)?;
        let shard_path = linked_store.path_for(prepared.registration.reference);
        let parent = shard_path.parent().ok_or("missing shard")?;
        symlink(directory.0.join("objects"), parent)?;
        assert!(linked_store.put(b"shard", &[], &mut linked_work).is_err());
        Ok(())
    }

    #[test]
    fn exact_edit_delete_and_historical_roots_survive_reopen()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget();
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
                    next: Some(next),
                }],
                &sink,
                &mut work,
            )?;
            assert!(result.root.0.is_some());
            root = result.root;
            let _previous = model.insert(key.clone(), next);
            history.push((root, model.clone()));
        }

        let stale = store.apply(
            root,
            &[MapEdit {
                key: vec![0x10],
                expected: None,
                next: Some(third),
            }],
            &sink,
            &mut work,
        );
        assert!(matches!(stale, Err(IndexError::Conflict)));
        let before_delete = sink.0.lock().map_err(|_error| "test sink poisoned")?.len();
        let deleted = store.apply(
            root,
            &[MapEdit {
                key: vec![0x10],
                expected: Some(second),
                next: None,
            }],
            &sink,
            &mut work,
        )?;
        let after_delete = sink.0.lock().map_err(|_error| "test sink poisoned")?.len();
        assert!(
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
                expected: Some(second),
                next: Some(third),
            }],
            &sink,
            &mut work,
        )?;
        root = updated.root;
        let _previous = model.insert(vec![0x11], third);
        assert!(
            sink.0
                .lock()
                .is_ok_and(|registrations| registrations.values().all(|installed| *installed))
        );
        assert_eq!(sink.1.load(Ordering::Relaxed), 7);
        assert_eq!(sink.2.load(Ordering::Relaxed), 7);
        drop(store);

        let reopened = FileObjectStore::open(&directory.0)?;
        for (historical_root, historical_model) in history {
            for key in &keys {
                assert_eq!(
                    reopened.lookup(historical_root, key, &mut work)?,
                    historical_model.get(key).copied()
                );
            }
        }
        for key in &keys {
            assert_eq!(
                reopened.lookup(root, key, &mut work)?,
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
        let mut work = budget();
        let first = value(&store, b"one", &mut work)?;
        let result = store.apply(
            MapRoot(None),
            &[MapEdit {
                key: b"b".to_vec(),
                expected: None,
                next: Some(first),
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
                    next: Some(first),
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
        assert!(matches!(failed, Err(IndexError::Conflict)));
        assert_eq!(store.lookup(base, b"a", &mut work)?, None);
        assert_eq!(store.lookup(base, b"b", &mut work)?, Some(first));

        let duplicate = store.apply(
            base,
            &[
                MapEdit {
                    key: b"a".to_vec(),
                    expected: None,
                    next: Some(first),
                },
                MapEdit {
                    key: b"a".to_vec(),
                    expected: None,
                    next: Some(first),
                },
            ],
            &sink,
            &mut work,
        );
        assert!(matches!(duplicate, Err(IndexError::InvalidInput(_))));
        Ok(())
    }

    #[test]
    fn pages_match_an_independent_ordered_map_and_late_scan_prunes()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget();
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
                        next: Some(value),
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
            assert!(page.entries.len() <= 3);
            if page.has_more {
                assert_eq!(page.entries.len(), 3);
            }
            after = page.entries.last().map(|(key, _)| key.clone());
            actual.extend(page.entries);
            if !page.has_more {
                break;
            }
        }
        assert_eq!(actual, model.into_iter().collect::<Vec<_>>());

        let before = work.usage().reads;
        let late = store.scan(root, Some(&[12]), 2, &mut work)?;
        assert_eq!(
            late.entries
                .iter()
                .map(|(key, _)| key.as_slice())
                .collect::<Vec<_>>(),
            vec![&[13][..], &[14][..]]
        );
        assert!(late.has_more);
        assert!(
            work.usage().reads - before <= 6,
            "late page visited unrelated subtrees"
        );
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
            Ok(payload)
        }

        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget();
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
        assert!(matches!(
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
        assert!(matches!(
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
        let mut work = budget();
        let first = value(&store, b"one", &mut work)?;
        let second = value(&store, b"two", &mut work)?;
        let left = store.put(b"parent", &[first, second], &mut work)?;
        let right = store.put(b"parent", &[second, first], &mut work)?;
        assert_ne!(left.reference, right.reference);
        assert_eq!(
            store.get(left.reference, &mut work)?.registration.children,
            vec![first, second]
        );
        assert_eq!(
            store.put(b"parent", &[first, second], &mut work)?.reference,
            left.reference
        );
        fs::write(store.path_for(left.reference), b"broken")?;
        assert!(matches!(
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
            Instant::now() + Duration::from_secs(30),
        );
        assert!(matches!(
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
        let mut work = budget();
        let value = value(&store, b"value", &mut work)?;
        let before = physical_file_count(&directory)?;
        let result = store.apply(
            MapRoot(None),
            &[MapEdit {
                key: b"key".to_vec(),
                expected: None,
                next: Some(value),
            }],
            &RejectingSink,
            &mut work,
        );
        assert!(matches!(result, Err(IndexError::ResourceLimited)));
        assert_eq!(physical_file_count(&directory)?, before);
        Ok(())
    }

    #[test]
    fn final_reachable_map_dag_installs_in_bounded_child_first_groups()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget();
        let value = value(&store, b"value", &mut work)?;
        let edits: Vec<_> = (0u8..128)
            .map(|key| MapEdit {
                key: vec![key, 0xff, key, 0xa5, key],
                expected: None,
                next: Some(value),
            })
            .collect();
        let sink = BoundedCustodySink::new(value, None);
        let output = store.apply(MapRoot(None), &edits, &sink, &mut work)?;
        let state = sink.state.lock().map_err(|_| "test sink poisoned")?;
        assert!(state.installed_groups.len() > 1);
        assert_eq!(state.reserve_attempts, state.installed_groups.len());
        assert!(state.installed_groups.iter().map(Vec::len).sum::<usize>() > MAX_BATCH_INPUTS);
        drop(state);
        for edit in &edits {
            assert_eq!(
                store.lookup(output.root, &edit.key, &mut work)?,
                Some(value)
            );
        }
        Ok(())
    }

    #[test]
    fn later_reachable_group_reservation_failure_does_not_return_a_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget();
        let value = value(&store, b"value", &mut work)?;
        let edits: Vec<_> = (0u8..128)
            .map(|key| MapEdit {
                key: vec![key, 0xff, key, 0xa5, key],
                expected: None,
                next: Some(value),
            })
            .collect();
        let before = physical_file_count(&directory)?;
        let sink = BoundedCustodySink::new(value, Some(2));
        assert!(matches!(
            store.apply(MapRoot(None), &edits, &sink, &mut work),
            Err(IndexError::ResourceLimited)
        ));
        let state = sink.state.lock().map_err(|_| "test sink poisoned")?;
        assert_eq!(state.reserve_attempts, 2);
        assert_eq!(state.installed_groups.len(), 1);
        assert!(
            state
                .installed_groups
                .first()
                .ok_or("first custody group missing")?
                .iter()
                .all(|reference| state.registered.contains(reference))
        );
        drop(state);
        assert!(physical_file_count(&directory)? > before);
        assert_eq!(
            store.lookup(MapRoot(None), &[0, 0xff, 0, 0xa5, 0], &mut work)?,
            None
        );
        Ok(())
    }

    #[test]
    fn multi_root_batch_matches_independent_models_and_keeps_history()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget();
        let values = [
            value(&store, b"one", &mut work)?,
            value(&store, b"two", &mut work)?,
            value(&store, b"three", &mut work)?,
        ];
        let mut roots = vec![MapRoot(None); 3];
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
                        expected,
                        next,
                    }],
                });
            }
            let output = store.apply_batch(&requests, &sink, &mut work)?;
            assert_eq!(output.roots.len(), models.len());
            for ((request, model), root) in requests.iter().zip(&mut models).zip(&output.roots) {
                let edit = request.edits.first().ok_or("edit missing")?;
                let next = edit.next.ok_or("next missing")?;
                let _old = model.insert(edit.key.clone(), next);
                for (key, value) in model {
                    assert_eq!(store.lookup(*root, key, &mut work)?, Some(*value));
                }
            }
            history.push((output.roots.clone(), models.clone()));
            roots = output.roots;
        }
        assert_eq!(sink.1.load(Ordering::Relaxed), 9);
        assert_eq!(sink.2.load(Ordering::Relaxed), 9);
        for (old_roots, old_models) in history {
            for (root, model) in old_roots.into_iter().zip(old_models) {
                for (key, value) in model {
                    assert_eq!(store.lookup(root, &key, &mut work)?, Some(value));
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
        let mut work = budget();
        let object = value(&store, b"same", &mut work)?;
        let request = MapBatchRequest {
            base: MapRoot(None),
            edits: vec![MapEdit {
                key: b"key".to_vec(),
                expected: None,
                next: Some(object),
            }],
        };
        let output = store.apply_batch(&[request.clone(), request], &sink, &mut work)?;
        assert_eq!(output.roots.first(), output.roots.get(1));
        assert_eq!(
            sink.0.lock().map_err(|_error| "test sink poisoned")?.len(),
            1
        );
        assert_eq!(sink.1.load(Ordering::Relaxed), 1);
        assert_eq!(sink.2.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn streamed_multi_root_edits_match_independent_models_and_clean_scratch()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget();
        let value = value(&store, b"streamed", &mut work)?;
        let mut edits = Vec::new();
        let mut models = [BTreeMap::new(), BTreeMap::new()];
        for root in 0u32..2 {
            for number in 0u16..192 {
                let key = number.to_be_bytes().to_vec();
                edits.push(OrdinalMapEdit {
                    root,
                    edit: MapEdit {
                        key: key.clone(),
                        expected: None,
                        next: Some(value),
                    },
                });
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
        assert_eq!(roots.len(), models.len());
        for (root, model) in roots.into_iter().zip(models) {
            for (key, expected) in model {
                assert_eq!(store.lookup(root, &key, &mut work)?, Some(expected));
            }
        }
        assert_eq!(fs::read_dir(directory.0.join("scratch"))?.count(), 0);
        Ok(())
    }

    #[test]
    fn sorted_bootstrap_matches_sparse_canonical_tree_across_prefix_splits()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget();
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
                next: Some(object),
            })
            .collect::<Vec<_>>();
        let built = store.apply(MapRoot(None), &edits, &sink, &mut work)?.root;
        let mut sparse = MapRoot(None);
        for edit in &edits {
            sparse = store
                .apply(sparse, std::slice::from_ref(edit), &sink, &mut work)?
                .root;
        }
        assert_eq!(built, sparse);
        for key in &keys {
            assert_eq!(store.lookup(built, key, &mut work)?, Some(object));
        }
        assert_eq!(fs::read_dir(directory.0.join("scratch"))?.count(), 0);
        Ok(())
    }

    #[test]
    fn sorted_existing_roots_match_serial_updates_and_independent_models()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget();
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
                next: Some(first),
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
                        expected: Some(first),
                        next: None,
                    },
                    MapEdit {
                        key: b"a".to_vec(),
                        expected: Some(first),
                        next: Some(second),
                    },
                    MapEdit {
                        key: b"a\0".to_vec(),
                        expected: None,
                        next: Some(third),
                    },
                    MapEdit {
                        key: b"aa".to_vec(),
                        expected: Some(first),
                        next: None,
                    },
                    MapEdit {
                        key: b"aaa".to_vec(),
                        expected: None,
                        next: Some(second),
                    },
                    MapEdit {
                        key: b"b".to_vec(),
                        expected: Some(first),
                        next: Some(third),
                    },
                    MapEdit {
                        key: b"ba".to_vec(),
                        expected: Some(first),
                        next: Some(second),
                    },
                    MapEdit {
                        key: b"bb".to_vec(),
                        expected: None,
                        next: Some(first),
                    },
                    MapEdit {
                        key: b"c".to_vec(),
                        expected: Some(first),
                        next: None,
                    },
                ],
            },
            MapBatchRequest {
                base,
                edits: vec![
                    MapEdit {
                        key: b"ab".to_vec(),
                        expected: Some(first),
                        next: None,
                    },
                    MapEdit {
                        key: b"az".to_vec(),
                        expected: None,
                        next: Some(second),
                    },
                    MapEdit {
                        key: b"z".to_vec(),
                        expected: Some(first),
                        next: Some(third),
                    },
                ],
            },
        ];
        let output = store.apply_batch(&requests, &sink, &mut work)?;
        for (request, root) in requests.iter().zip(&output.roots) {
            let mut model = baseline.clone();
            let mut serial = base;
            for edit in &request.edits {
                assert_eq!(model.get(&edit.key).copied(), edit.expected);
                match edit.next {
                    Some(next) => {
                        let _prior = model.insert(edit.key.clone(), next);
                    }
                    None => {
                        let _prior = model.remove(&edit.key);
                    }
                }
                serial = store
                    .apply(serial, std::slice::from_ref(edit), &sink, &mut work)?
                    .root;
            }
            assert_eq!(*root, serial);
            let page = store.scan(*root, None, 128, &mut work)?;
            assert!(!page.has_more);
            assert_eq!(page.entries, model.into_iter().collect::<Vec<_>>());
        }
        let historical = store.scan(base, None, 128, &mut work)?;
        assert_eq!(historical.entries, baseline.into_iter().collect::<Vec<_>>());
        Ok(())
    }

    #[test]
    fn sorted_existing_root_closes_shared_ancestors_once() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget();
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
                next: Some(old),
            })
            .collect::<Vec<_>>();
        let base = store.apply(MapRoot(None), &initial, &sink, &mut work)?.root;
        let optimized = StagedMap::open(&store, &mut work)?;
        for edit in &edits {
            optimized.scratch.insert_edit(
                &OrdinalMapEdit {
                    root: 0,
                    edit: MapEdit {
                        key: edit.clone(),
                        expected: Some(old),
                        next: Some(new),
                    },
                },
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
                &key_nibbles(key)?,
                Some(new),
                &serial,
                &mut work,
            )?;
        }
        let serial_writes = work.usage().writes - before_serial;
        assert_eq!(optimized_root, serial_root);
        assert!(
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
        let mut setup_budget = budget();
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
                        next: Some(reference),
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
                    expected,
                    next,
                });
            }

            let mut work = budget();
            for edit in &edits {
                assert_eq!(store.lookup(*base, &edit.key, &mut work)?, edit.expected);
            }
            let optimized = StagedMap::open(&store, &mut work)?;
            for edit in &edits {
                optimized.scratch.insert_edit(
                    &OrdinalMapEdit {
                        root: 0,
                        edit: edit.clone(),
                    },
                    &mut work,
                )?;
            }
            let actual = store.apply_sorted_existing_root(0, *base, &optimized, &mut work)?;
            let serial = StagedMap::open(&store, &mut work)?;
            let mut expected_root = *base;
            for edit in &edits {
                if edit.expected != edit.next {
                    expected_root = store.update_one(
                        expected_root,
                        &key_nibbles(&edit.key)?,
                        edit.next,
                        &serial,
                        &mut work,
                    )?;
                }
            }
            assert_eq!(actual, expected_root, "differential case {case}");
            if case % 16 == 0 {
                store.install_reachable_map_nodes(&[actual], &optimized, &sink, &mut work)?;
                let page = store.scan(actual, None, 32, &mut work)?;
                assert!(!page.has_more);
                assert_eq!(page.entries, expected_model.into_iter().collect::<Vec<_>>());
                let historical = store.scan(*base, None, 32, &mut work)?;
                assert_eq!(
                    historical.entries,
                    base_model.clone().into_iter().collect::<Vec<_>>()
                );
            }
        }
        assert!(insertions > 0 && deletions > 0 && noops > 0);
        Ok(())
    }

    #[test]
    fn late_existing_root_conflict_preserves_all_original_roots()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget();
        let old = value(&store, b"old", &mut work)?;
        let new = value(&store, b"new", &mut work)?;
        let base = store
            .apply(
                MapRoot(None),
                &[MapEdit {
                    key: b"existing".to_vec(),
                    expected: None,
                    next: Some(old),
                }],
                &sink,
                &mut work,
            )?
            .root;
        let before_objects = physical_file_count(&directory)?;
        let before_reservations = sink.1.load(Ordering::Relaxed);
        let mut edits = (0u16..128)
            .map(|number| OrdinalMapEdit {
                root: 0,
                edit: MapEdit {
                    key: number.to_be_bytes().to_vec(),
                    expected: None,
                    next: Some(new),
                },
            })
            .collect::<Vec<_>>();
        edits.push(OrdinalMapEdit {
            root: 1,
            edit: MapEdit {
                key: b"existing".to_vec(),
                expected: Some(new),
                next: None,
            },
        });
        let mut cursor = TestEditCursor(edits.into_iter());
        assert!(matches!(
            store.apply_stream(&[base, base], &mut cursor, &sink, &mut work),
            Err(IndexError::Conflict)
        ));
        assert_eq!(sink.1.load(Ordering::Relaxed), before_reservations);
        assert_eq!(physical_file_count(&directory)?, before_objects);
        assert_eq!(store.lookup(base, b"existing", &mut work)?, Some(old));
        Ok(())
    }

    #[test]
    fn late_stream_conflict_or_duplicate_never_reserves_a_map_object()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget();
        let value = value(&store, b"streamed", &mut work)?;
        let before = physical_file_count(&directory)?;
        let make_edits = || {
            (0u16..192)
                .map(|number| OrdinalMapEdit {
                    root: 0,
                    edit: MapEdit {
                        key: number.to_be_bytes().to_vec(),
                        expected: None,
                        next: Some(value),
                    },
                })
                .collect::<Vec<_>>()
        };
        let mut conflict = make_edits();
        conflict.push(OrdinalMapEdit {
            root: 0,
            edit: MapEdit {
                key: 192u16.to_be_bytes().to_vec(),
                expected: Some(value),
                next: None,
            },
        });
        let mut conflict_cursor = TestEditCursor(conflict.into_iter());
        assert!(matches!(
            store.apply_stream(&[MapRoot(None)], &mut conflict_cursor, &sink, &mut work),
            Err(IndexError::Conflict)
        ));
        let mut duplicate = make_edits();
        let final_edit = duplicate.last().ok_or("missing final test edit")?.clone();
        duplicate.push(final_edit);
        let mut duplicate_cursor = TestEditCursor(duplicate.into_iter());
        assert!(matches!(
            store.apply_stream(&[MapRoot(None)], &mut duplicate_cursor, &sink, &mut work),
            Err(IndexError::InvalidInput(_))
        ));
        assert_eq!(sink.1.load(Ordering::Relaxed), 0);
        assert_eq!(sink.2.load(Ordering::Relaxed), 0);
        assert_eq!(physical_file_count(&directory)?, before);
        assert_eq!(fs::read_dir(directory.0.join("scratch"))?.count(), 0);
        Ok(())
    }

    #[test]
    fn later_batch_conflict_refuses_before_any_reservation()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget();
        let object = value(&store, b"value", &mut work)?;
        let before = physical_file_count(&directory)?;
        let requests = [
            MapBatchRequest {
                base: MapRoot(None),
                edits: vec![MapEdit {
                    key: b"valid".to_vec(),
                    expected: None,
                    next: Some(object),
                }],
            },
            MapBatchRequest {
                base: MapRoot(None),
                edits: vec![MapEdit {
                    key: b"conflict".to_vec(),
                    expected: Some(object),
                    next: None,
                }],
            },
        ];
        assert!(matches!(
            store.apply_batch(&requests, &sink, &mut work),
            Err(IndexError::Conflict)
        ));
        assert_eq!(physical_file_count(&directory)?, before);
        assert_eq!(sink.1.load(Ordering::Relaxed), 0);
        assert_eq!(sink.2.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
