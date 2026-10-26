use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::{
    RepoMapNodeRef,
    index::{OwnerMutation, OwnerMutationSet, ProjectionBinding},
};
use quanta_index_core::index::{
    AttemptContext, IndexError, MapEdit, MapPageCursor, MapPageEntry, MapRoot, MemoryLease,
    MemoryLifetime, ObjectRef, OwnerKey, OwnerMutationCursor, WorkBudget, owner_from_map_key,
    owner_map_key, ports::PersistentMapPort,
};

use super::contribution::{
    GraphElement, GraphSupport, OwnerGraphRecord, load_output, load_owner, load_projection,
    load_support,
};
use super::{GraphRoot, MAX_AFFECTED_GRAPH_ITEMS, encode_funded};

const PAGE: u32 = 128;
pub(super) const MAX_KEY: usize = 2048;

pub(super) fn apply(
    root: &mut GraphRoot,
    projection: &ProjectionBinding,
    affected: Option<OwnerMutationSet>,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let mut owners = BTreeMap::<Vec<u8>, MapEdit>::new();
    let mut nodes = BTreeMap::<Vec<u8>, MapEdit>::new();
    let mut edges = BTreeMap::<Vec<u8>, MapEdit>::new();
    let mut incoming = BTreeMap::<Vec<u8>, MapEdit>::new();
    let mut changed_nodes = BTreeSet::<Vec<u8>>::new();
    let mut endpoints = BTreeSet::<RepoMapNodeRef>::new();
    let mut leases = Vec::<MemoryLease>::new();
    let mut count = 0usize;
    let bootstrap = affected.is_none();
    let mut cursor = match affected {
        Some(set) => {
            GraphChanges::Delta(OwnerMutationCursor::new(set, context.maps, context.objects))
        }
        None => GraphChanges::Bootstrap(BootstrapOwners::new(projection.owners, context.maps)),
    };
    while let Some(mutation) = cursor.next(context.budget)? {
        context.budget.checkpoint()?;
        let key = owner_map_key(mutation.owner);
        let selected_old = if bootstrap {
            None
        } else {
            context
                .maps
                .lookup_object(root.projection.owners, &key, context.budget)?
        };
        let selected_new = context
            .maps
            .lookup_object(projection.owners, &key, context.budget)?;
        if selected_old != mutation.expected || selected_new != mutation.next {
            return Err(IndexError::CorruptObject(
                "graph affected owner stamp mismatch".into(),
            ));
        }
        let old_ref = context
            .maps
            .lookup_object(root.owners, &key, context.budget)?;
        let old = old_ref
            .map(|reference| load_owner(reference, context.objects, context.budget))
            .transpose()?;
        if old.as_ref().is_some_and(|record| {
            record.owner != mutation.owner || Some(record.projection) != mutation.expected
        }) || old.is_none() != mutation.expected.is_none()
        {
            return Err(IndexError::CorruptObject(
                "graph owner reverse index mismatch".into(),
            ));
        }
        if mutation.expected == mutation.next {
            continue;
        }
        if let Some(output) = old.as_ref().and_then(|previous| previous.output) {
            let prior = load_output(output, context.objects, context.budget)?;
            for node in prior.output.nodes {
                let rank_projection = match prior
                    .output
                    .projection
                    .binary_search_by(|row| row.subject.cmp(&node.identity))
                {
                    Ok(position) => prior.output.projection.get(position).map(|row| row.row),
                    Err(_insertion) => None,
                };
                bump(&mut count)?;
                let key = node_key(&node.identity, mutation.owner)?;
                let _inserted = changed_nodes.insert(node_prefix(&node.identity)?);
                let _inserted = endpoints.insert(node.identity.clone());
                require_existing_support(
                    root.nodes,
                    &key,
                    &GraphSupport {
                        owner: mutation.owner,
                        element: GraphElement::Node(node),
                        projection: rank_projection,
                    },
                    context,
                )?;
                push_edit(&mut nodes, root.nodes, key, None, context, &mut leases)?;
            }
            for edge in prior.output.edges {
                bump(&mut count)?;
                let key = edge_key(&edge, mutation.owner)?;
                let reverse_key = incoming_key(&edge, mutation.owner)?;
                let _inserted = endpoints.insert(edge.source.clone());
                let _inserted = endpoints.insert(edge.target.clone());
                require_existing_support(
                    root.edges,
                    &key,
                    &GraphSupport {
                        owner: mutation.owner,
                        element: GraphElement::Edge(edge.clone()),
                        projection: None,
                    },
                    context,
                )?;
                require_existing_support(
                    root.incoming,
                    &reverse_key,
                    &GraphSupport {
                        owner: mutation.owner,
                        element: GraphElement::Edge(edge),
                        projection: None,
                    },
                    context,
                )?;
                push_edit(&mut edges, root.edges, key, None, context, &mut leases)?;
                push_edit(
                    &mut incoming,
                    root.incoming,
                    reverse_key,
                    None,
                    context,
                    &mut leases,
                )?;
            }
        }
        let next_record = if let Some(reference) = mutation.next {
            let contribution = load_projection(reference, context.objects, context.budget)?;
            if contribution.owner != mutation.owner
                || contribution.projection != projection.projection
                || contribution.producer != projection.producer
            {
                return Err(IndexError::InvalidInput(
                    "graph contribution authority mismatch".into(),
                ));
            }
            if let Some(output) = contribution.output {
                let graph = load_output(output, context.objects, context.budget)?;
                super::contribution::validate_rank_coverage(
                    &graph.output,
                    projection.producer_policy,
                )?;
                for node in graph.output.nodes {
                    let rank_projection = match graph
                        .output
                        .projection
                        .binary_search_by(|row| row.subject.cmp(&node.identity))
                    {
                        Ok(position) => graph.output.projection.get(position).map(|row| row.row),
                        Err(_insertion) => None,
                    };
                    if let Some(reference) = rank_projection {
                        let loaded = super::contribution::load_rank_projection(
                            reference,
                            context.objects,
                            context.budget,
                        )?;
                        if loaded.entry.subject != node.identity {
                            return Err(IndexError::InvalidInput(
                                "graph rank row subject differs".into(),
                            ));
                        }
                    }
                    bump(&mut count)?;
                    let key = node_key(&node.identity, mutation.owner)?;
                    let _inserted = changed_nodes.insert(node_prefix(&node.identity)?);
                    let _inserted = endpoints.insert(node.identity.clone());
                    if !nodes.contains_key(&key)
                        && context
                            .maps
                            .lookup_object(root.nodes, &key, context.budget)?
                            .is_some()
                    {
                        return Err(IndexError::CorruptObject(
                            "graph supporter exists without prior contribution".into(),
                        ));
                    }
                    let support = GraphSupport {
                        owner: mutation.owner,
                        element: GraphElement::Node(node),
                        projection: rank_projection,
                    };
                    let bytes = encode_funded(&support, context.budget)?;
                    let children: Vec<ObjectRef> = support.projection.into_iter().collect();
                    let support_ref = context.put(&bytes.bytes, &children)?;
                    push_edit(
                        &mut nodes,
                        root.nodes,
                        key,
                        Some(support_ref),
                        context,
                        &mut leases,
                    )?;
                }
                for edge in graph.output.edges {
                    bump(&mut count)?;
                    let key = edge_key(&edge, mutation.owner)?;
                    let reverse_key = incoming_key(&edge, mutation.owner)?;
                    let _inserted = endpoints.insert(edge.source.clone());
                    let _inserted = endpoints.insert(edge.target.clone());
                    if !edges.contains_key(&key)
                        && context
                            .maps
                            .lookup_object(root.edges, &key, context.budget)?
                            .is_some()
                    {
                        return Err(IndexError::CorruptObject(
                            "graph supporter exists without prior contribution".into(),
                        ));
                    }
                    if !incoming.contains_key(&reverse_key)
                        && context
                            .maps
                            .lookup_object(root.incoming, &reverse_key, context.budget)?
                            .is_some()
                    {
                        return Err(IndexError::CorruptObject(
                            "graph reverse supporter exists without prior contribution".into(),
                        ));
                    }
                    let support = GraphSupport {
                        owner: mutation.owner,
                        element: GraphElement::Edge(edge),
                        projection: None,
                    };
                    let bytes = encode_funded(&support, context.budget)?;
                    let support_ref = context.put(&bytes.bytes, &[])?;
                    push_edit(
                        &mut edges,
                        root.edges,
                        key,
                        Some(support_ref),
                        context,
                        &mut leases,
                    )?;
                    push_edit(
                        &mut incoming,
                        root.incoming,
                        reverse_key,
                        Some(support_ref),
                        context,
                        &mut leases,
                    )?;
                }
            }
            let record = OwnerGraphRecord {
                owner: mutation.owner,
                projection: reference,
                output: contribution.output,
            };
            let bytes = encode_funded(&record, context.budget)?;
            let mut children = vec![reference];
            children.extend(contribution.output);
            children.sort_unstable();
            children.dedup();
            Some(context.put(&bytes.bytes, &children)?)
        } else {
            None
        };
        push_edit(
            &mut owners,
            root.owners,
            key.to_vec(),
            next_record,
            context,
            &mut leases,
        )?;
    }
    // Validate the final supporter set, including simultaneous remove/add of
    // the same identity, before any of the three immutable roots are advanced.
    for prefix in changed_nodes {
        validate_node_payloads(&prefix, root.nodes, &nodes, context)?;
    }
    if owners.is_empty() && nodes.is_empty() && edges.is_empty() && incoming.is_empty() {
        return Ok(());
    }
    root.owners = apply_edits(root.owners, owners, context)?;
    root.nodes = apply_edits(root.nodes, nodes, context)?;
    root.edges = apply_edits(root.edges, edges, context)?;
    root.incoming = apply_edits(root.incoming, incoming, context)?;
    for endpoint in endpoints {
        if has_prefix(root.nodes, &node_prefix(&endpoint)?, context)? {
            validate_variant_identity(root.nodes, &endpoint, context)?;
        }
        if !has_prefix(root.nodes, &node_prefix(&endpoint)?, context)?
            && (has_prefix(root.edges, &edge_prefix(&endpoint)?, context)?
                || has_prefix(root.incoming, &incoming_prefix(&endpoint)?, context)?)
        {
            return Err(IndexError::InvalidInput(
                "graph edge endpoint has no node supporter".into(),
            ));
        }
    }
    drop(leases);
    Ok(())
}

fn apply_edits(
    root: MapRoot,
    edits: BTreeMap<Vec<u8>, MapEdit>,
    context: &mut AttemptContext<'_>,
) -> Result<MapRoot, IndexError> {
    if edits.is_empty() {
        return Ok(root);
    }
    let bytes = edits
        .len()
        .checked_mul(std::mem::size_of::<MapEdit>())
        .ok_or(IndexError::ResourceLimited)?;
    let _memory = context.budget.reserve_memory(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut batch = Vec::new();
    batch
        .try_reserve_exact(edits.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    batch.extend(edits.into_values());
    context.apply(root, &batch)
}

fn bump(count: &mut usize) -> Result<(), IndexError> {
    *count = (*count).checked_add(1).ok_or(IndexError::ResourceLimited)?;
    if *count > MAX_AFFECTED_GRAPH_ITEMS {
        return Err(IndexError::ResourceLimited);
    }
    Ok(())
}

enum GraphChanges<'a> {
    Delta(OwnerMutationCursor<'a>),
    Bootstrap(BootstrapOwners<'a>),
}

impl GraphChanges<'_> {
    fn next(&mut self, budget: &mut WorkBudget) -> Result<Option<OwnerMutation>, IndexError> {
        match self {
            Self::Delta(cursor) => cursor.next_mutation(budget),
            Self::Bootstrap(cursor) => cursor.next(budget),
        }
    }
}

struct BootstrapOwners<'a> {
    root: MapRoot,
    maps: &'a dyn PersistentMapPort,
    page: MapPageCursor,
    after: Option<MapPageEntry>,
    more: bool,
}

impl<'a> BootstrapOwners<'a> {
    fn new(root: MapRoot, maps: &'a dyn PersistentMapPort) -> Self {
        Self {
            root,
            maps,
            page: MapPageCursor::default(),
            after: None,
            more: true,
        }
    }

    fn next(&mut self, budget: &mut WorkBudget) -> Result<Option<OwnerMutation>, IndexError> {
        budget.checkpoint()?;
        if self.page.is_empty() && self.more {
            let page = self.maps.scan(
                self.root,
                self.after.as_ref().map(MapPageEntry::key).transpose()?,
                PAGE,
                budget,
            )?;
            if page.entries.is_empty() && page.has_more {
                return Err(IndexError::BackendViolation(
                    "graph bootstrap made no progress".into(),
                ));
            }
            self.more = page.has_more;
            self.page = page.cursor();
        }
        let Some(entry) = self.page.next() else {
            return Ok(None);
        };
        let (key, reference) = entry.object_parts()?;
        if self
            .after
            .as_ref()
            .map(MapPageEntry::key)
            .transpose()?
            .is_some_and(|prior| prior >= key.as_slice())
        {
            return Err(IndexError::CorruptObject(
                "graph bootstrap owner order".into(),
            ));
        }
        let owner = owner_from_map_key(key)?;
        let result = OwnerMutation {
            owner,
            expected: None,
            next: Some(*reference),
        };
        self.after = Some(entry);
        Ok(Some(result))
    }
}

fn require_existing_support(
    root: MapRoot,
    key: &[u8],
    expected: &GraphSupport,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let reference = context
        .maps
        .lookup_object(root, key, context.budget)?
        .ok_or_else(|| IndexError::CorruptObject("graph prior supporter absent".into()))?;
    let loaded = load_support(reference, context.objects, context.budget)?;
    let support = loaded.support;
    if &support != expected {
        return Err(IndexError::CorruptObject(
            "graph prior supporter mismatch".into(),
        ));
    }
    Ok(())
}

fn push_edit(
    edits: &mut BTreeMap<Vec<u8>, MapEdit>,
    base: MapRoot,
    key: Vec<u8>,
    next: Option<ObjectRef>,
    context: &mut AttemptContext<'_>,
    leases: &mut Vec<MemoryLease>,
) -> Result<(), IndexError> {
    if key.len() > MAX_KEY {
        return Err(IndexError::ResourceLimited);
    }
    if let Some(edit) = edits.get_mut(&key) {
        edit.next = next.map(Into::into);
        return Ok(());
    }
    if edits.len() >= MAX_AFFECTED_GRAPH_ITEMS {
        return Err(IndexError::ResourceLimited);
    }
    let bytes = key
        .len()
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<MapEdit>()))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<MemoryLease>()))
        .and_then(|bytes| {
            std::mem::size_of::<usize>()
                .checked_mul(4)
                .and_then(|metadata| bytes.checked_add(metadata))
        })
        .ok_or(IndexError::ResourceLimited)?;
    let memory = context.budget.reserve_memory(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    context.budget.charge(quanta_index_core::index::WorkUsage {
        bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..Default::default()
    })?;
    if let Some(lease) = memory {
        leases
            .try_reserve(1)
            .map_err(|_error| IndexError::ResourceLimited)?;
        leases.push(lease);
    }
    let expected = context.maps.lookup_object(base, &key, context.budget)?;
    let previous = edits.insert(
        key.clone(),
        MapEdit {
            key,
            expected: expected.map(Into::into),
            next: next.map(Into::into),
        },
    );
    if previous.is_some() {
        return Err(IndexError::BackendViolation(
            "graph edit unexpectedly replaced".into(),
        ));
    }
    Ok(())
}

fn validate_node_payloads(
    prefix: &[u8],
    base: MapRoot,
    edits: &BTreeMap<Vec<u8>, MapEdit>,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let mut expected = None;
    let mut after = Some(prefix.to_vec());
    let mut seen = 0usize;
    'existing: loop {
        let page = context
            .maps
            .scan(base, after.as_deref(), PAGE, context.budget)?;
        for (key, reference) in &page.entries {
            if !key.starts_with(prefix) {
                break 'existing;
            }
            if edits.get(key).is_none() {
                let loaded =
                    load_support(reference.object_ref()?, context.objects, context.budget)?;
                let support = loaded.support;
                let GraphElement::Node(node) = support.element else {
                    return Err(IndexError::CorruptObject("node index contains edge".into()));
                };
                if node_prefix(&node.identity)?.as_slice() != prefix
                    || node_key(&node.identity, support.owner)?.as_slice() != key.as_slice()
                {
                    return Err(IndexError::CorruptObject(
                        "node supporter key mismatch".into(),
                    ));
                }
                match expected {
                    Some(value) if value != (node.payload_digest, support.projection) => {
                        return Err(IndexError::InvalidInput(
                            "conflicting graph node payload".into(),
                        ));
                    }
                    None => expected = Some((node.payload_digest, support.projection)),
                    _ => {}
                }
                seen = seen.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
        }
        if !page.has_more {
            break;
        }
        let last = page.entries.last().ok_or_else(|| {
            IndexError::BackendViolation("graph node scan made no progress".into())
        })?;
        after = Some(last.0.clone());
        if seen > MAX_AFFECTED_GRAPH_ITEMS {
            return Err(IndexError::ResourceLimited);
        }
    }
    for (key, edit) in edits.range(prefix.to_vec()..) {
        if !key.starts_with(prefix) {
            break;
        }
        let Some(reference) = edit.next else {
            continue;
        };
        let loaded = load_support(reference.object_ref()?, context.objects, context.budget)?;
        let support = loaded.support;
        let GraphElement::Node(node) = support.element else {
            return Err(IndexError::CorruptObject("node index contains edge".into()));
        };
        if node_prefix(&node.identity)?.as_slice() != prefix
            || node_key(&node.identity, support.owner)?.as_slice() != key.as_slice()
        {
            return Err(IndexError::CorruptObject(
                "node supporter key mismatch".into(),
            ));
        }
        match expected {
            Some(value) if value != (node.payload_digest, support.projection) => {
                return Err(IndexError::InvalidInput(
                    "conflicting graph node payload".into(),
                ));
            }
            None => expected = Some((node.payload_digest, support.projection)),
            _ => {}
        }
        seen = seen.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        if seen > MAX_AFFECTED_GRAPH_ITEMS {
            return Err(IndexError::ResourceLimited);
        }
    }
    Ok(())
}

pub(super) fn node_prefix(node: &RepoMapNodeRef) -> Result<Vec<u8>, IndexError> {
    let encoded = encoded_node_bounded(node)?;
    let additional = encoded
        .len()
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    let mut key = prefix_buffer(b"node\0", additional)?;
    key.extend(encoded);
    key.push(0);
    Ok(key)
}

pub(super) fn node_key(node: &RepoMapNodeRef, owner: OwnerKey) -> Result<Vec<u8>, IndexError> {
    let mut key = node_prefix(node)?;
    ensure_key_room(key.len(), owner_map_key(owner).len())?;
    key.extend(owner_map_key(owner));
    Ok(key)
}

pub(super) fn edge_prefix(source: &RepoMapNodeRef) -> Result<Vec<u8>, IndexError> {
    let encoded = encoded_node_bounded(source)?;
    let additional = encoded
        .len()
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    let mut key = prefix_buffer(b"edge\0", additional)?;
    key.extend(encoded);
    key.push(0);
    Ok(key)
}

pub(super) fn incoming_prefix(target: &RepoMapNodeRef) -> Result<Vec<u8>, IndexError> {
    let encoded = encoded_node_bounded(target)?;
    let additional = encoded
        .len()
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    let mut key = prefix_buffer(b"incoming\0", additional)?;
    key.extend(encoded);
    key.push(0);
    Ok(key)
}

pub(super) fn incoming_key(
    edge: &super::GraphEdge,
    owner: OwnerKey,
) -> Result<Vec<u8>, IndexError> {
    let mut key = incoming_prefix(&edge.target)?;
    let encoded_source = encoded_node_bounded(&edge.source)?;
    let additional = edge
        .kind
        .as_code_str()
        .len()
        .checked_add(1)
        .and_then(|len| len.checked_add(encoded_source.len()))
        .and_then(|len| len.checked_add(1))
        .and_then(|len| len.checked_add(owner_map_key(owner).len()))
        .ok_or(IndexError::ResourceLimited)?;
    ensure_key_room(key.len(), additional)?;
    key.extend(edge.kind.as_code_str().as_bytes());
    key.push(0);
    key.extend(encoded_source);
    key.push(0);
    key.extend(owner_map_key(owner));
    Ok(key)
}

fn has_prefix(
    root: MapRoot,
    prefix: &[u8],
    context: &mut AttemptContext<'_>,
) -> Result<bool, IndexError> {
    let page = context.maps.scan(root, Some(prefix), 1, context.budget)?;
    if page.entries.is_empty() && page.has_more {
        return Err(IndexError::BackendViolation(
            "graph endpoint scan made no progress".into(),
        ));
    }
    Ok(page
        .entries
        .first()
        .is_some_and(|(key, _)| key.starts_with(prefix)))
}

pub(super) fn edge_key(edge: &super::GraphEdge, owner: OwnerKey) -> Result<Vec<u8>, IndexError> {
    let mut key = edge_prefix(&edge.source)?;
    let encoded_target = encoded_node_bounded(&edge.target)?;
    let additional = edge
        .kind
        .as_code_str()
        .len()
        .checked_add(1)
        .and_then(|len| len.checked_add(encoded_target.len()))
        .and_then(|len| len.checked_add(1))
        .and_then(|len| len.checked_add(owner_map_key(owner).len()))
        .ok_or(IndexError::ResourceLimited)?;
    ensure_key_room(key.len(), additional)?;
    key.extend(edge.kind.as_code_str().as_bytes());
    key.push(0);
    key.extend(encoded_target);
    key.push(0);
    key.extend(owner_map_key(owner));
    Ok(key)
}

fn ensure_key_room(existing: usize, additional: usize) -> Result<(), IndexError> {
    if existing
        .checked_add(additional)
        .is_none_or(|len| len > MAX_KEY)
    {
        return Err(IndexError::ResourceLimited);
    }
    Ok(())
}

fn prefix_buffer(prefix: &[u8], additional: usize) -> Result<Vec<u8>, IndexError> {
    ensure_key_room(prefix.len(), additional)?;
    let mut key = Vec::new();
    let capacity = prefix
        .len()
        .checked_add(additional)
        .ok_or(IndexError::ResourceLimited)?;
    key.try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    key.extend_from_slice(prefix);
    Ok(key)
}

fn encoded_node_bounded(node: &RepoMapNodeRef) -> Result<Vec<u8>, IndexError> {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("graph key length overflow"))?;
            if self.0 > MAX_KEY {
                return Err(std::io::Error::other("graph key exceeds bound"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, node).map_err(|_error| IndexError::ResourceLimited)?;
    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(count.0)
        .map_err(|_error| IndexError::ResourceLimited)?;
    serde_json::to_writer(&mut encoded, node)
        .map_err(|error| IndexError::InvalidInput(format!("graph key: {error}")))?;
    if encoded.len() != count.0 {
        return Err(IndexError::BackendViolation(
            "graph key encoding changed".into(),
        ));
    }
    Ok(encoded)
}

pub(super) fn validate_variant_identity(
    nodes: MapRoot,
    identity: &RepoMapNodeRef,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if has_variant_conflict(nodes, identity, context.maps, context.budget)? {
        return Err(IndexError::InvalidInput(
            "graph raw identity has conflicting typed variants".into(),
        ));
    }
    Ok(())
}

pub(super) fn has_variant_conflict(
    nodes: MapRoot,
    identity: &RepoMapNodeRef,
    maps: &dyn quanta_index_core::index::ports::PersistentMapPort,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    use quanta_index_contract::{ChunkId, FileId, RepoMapModuleId, SymbolId};
    let raw = match identity {
        RepoMapNodeRef::File(id) => id.as_str(),
        RepoMapNodeRef::Module(id) => id.as_str(),
        RepoMapNodeRef::Symbol(id) => id.as_str(),
        RepoMapNodeRef::Chunk(id) => id.as_str(),
    };
    let _memory = budget.reserve_memory(
        u64::try_from(MAX_KEY.checked_mul(8).ok_or(IndexError::ResourceLimited)?)
            .map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    if raw.len() > MAX_KEY {
        return Err(IndexError::ResourceLimited);
    }
    for kind in 0..4 {
        let mut value = String::new();
        value
            .try_reserve_exact(raw.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        value.push_str(raw);
        let other = match kind {
            0 => RepoMapNodeRef::File(FileId::new(value)),
            1 => RepoMapNodeRef::Module(RepoMapModuleId::new(value)),
            2 => RepoMapNodeRef::Symbol(SymbolId::new(value)),
            _ => RepoMapNodeRef::Chunk(ChunkId::new(value)),
        };
        if &other != identity {
            let prefix = node_prefix(&other)?;
            let page = maps.scan(nodes, Some(&prefix), 1, budget)?;
            if page.entries.is_empty() && page.has_more {
                return Err(IndexError::BackendViolation(
                    "graph identity scan made no progress".into(),
                ));
            }
            if page
                .entries
                .first()
                .is_some_and(|(key, _)| key.starts_with(&prefix))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
