use quanta_index_contract::{
    index::{InputDomain, MemberBinding},
    native_protocol::{MAX_NATIVE_GRAPH_EDGES, MAX_NATIVE_GRAPH_ITEMS, NativeGraphDirection},
};
use quanta_index_core::index::{
    IndexError, IndexQueryPort, MemoryLease, MemoryLifetime, ObjectRef, QueryOutcome,
    QueryReadContext, WorkBudget, decode_member_binding, decode_projection_binding,
    effort::ResourceNeed,
    member_map_key,
    ports::{ImmutableObjectPort, PersistentMapPort},
};

use super::affected::{
    edge_key, edge_prefix, incoming_key, incoming_prefix, node_key, node_prefix,
};
use super::contribution::{GraphElement, load_support};
use super::{GraphNode, NativeGraph, read_root_for_query};

const PAGE: u32 = 128;
const MAX_QUERY_EDGES: u32 = MAX_NATIVE_GRAPH_EDGES;

pub use quanta_index_contract::native_protocol::{
    NativeGraphQuery as GraphQuery, NativeGraphRow as GraphRow,
};

#[derive(Debug)]
pub struct GraphPage {
    pub source: Option<GraphNode>,
    pub source_supporters: u32,
    pub edges: Vec<GraphRow>,
    memory: Vec<Option<MemoryLease>>,
}

impl GraphPage {
    /// Move the selected page and its retained allocation custody together.
    /// The caller must keep the leases alive until borrowed wire emission ends.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        Option<GraphNode>,
        u32,
        Vec<GraphRow>,
        Vec<Option<MemoryLease>>,
    ) {
        (self.source, self.source_supporters, self.edges, self.memory)
    }
}

impl PartialEq for GraphPage {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && self.source_supporters == other.source_supporters
            && self.edges == other.edges
    }
}
impl Eq for GraphPage {}

/// Ticket-bound adapter. The selected profile/member/projection are resolved
/// under the caller's existing read lease before entering the private index.
pub struct NativeGraphReader {
    need: ResourceNeed,
}

impl NativeGraphReader {
    pub fn new(need: ResourceNeed) -> Result<Self, IndexError> {
        if need.cpu_units == 0
            || need.io_bytes == 0
            || need.memory_bytes == 0
            || need.provider_units != 0
        {
            return Err(IndexError::InvalidInput(
                "native graph read reservation is invalid".into(),
            ));
        }
        Ok(Self { need })
    }

    pub(super) fn pinned_member(
        context: &mut QueryReadContext<'_>,
    ) -> Result<MemberBinding, IndexError> {
        let InputDomain::Projection {
            projection,
            producer,
        } = context.instance.input_domain
        else {
            return Err(IndexError::InvalidInput(
                "graph reader requires projection input".into(),
            ));
        };
        let mut projection_key = [0_u8; 27];
        projection_key[..11].copy_from_slice(b"projection\0");
        projection_key[11..].copy_from_slice(&projection.0);
        let projection_ref = context
            .maps
            .lookup_object(
                context.ticket.view.projections,
                &projection_key,
                context.budget,
            )?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let projection_object = context.objects.get(projection_ref, context.budget)?;
        let selected = decode_projection_binding(&projection_object.payload, context.budget)?;
        if projection_object.registration.reference != projection_ref
            || projection_object.registration.children != selected.object_children()
            || selected.projection != projection
            || selected.producer != producer
            || selected.schema != context.instance.schema
        {
            return Err(IndexError::CorruptObject(
                "selected graph projection differs".into(),
            ));
        }
        if selected.source != context.ticket.view.source {
            return Err(IndexError::SnapshotUnavailable);
        }
        let member_ref = context
            .maps
            .lookup_object(
                context.ticket.view.members,
                &member_map_key(context.instance.instance),
                context.budget,
            )?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let stored = context.objects.get(member_ref, context.budget)?;
        let member = decode_member_binding(&stored.payload, context.budget)?;
        let children = [Some(member.artifact), member.input.root.0];
        if stored.registration.reference != member_ref
            || !stored
                .registration
                .children
                .iter()
                .copied()
                .eq(children.into_iter().flatten())
            || member.instance != context.instance.instance
            || member.format != context.instance.format
            || member.input.root != selected.owners
            || member.input.schema != context.instance.schema
            || member.input.policy != context.instance.policy
            || member.input.context != selected.materialization_context()
        {
            return Err(IndexError::CorruptObject(
                "selected graph member differs".into(),
            ));
        }
        Ok(member)
    }
}

impl IndexQueryPort<GraphQuery, GraphPage> for NativeGraphReader {
    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        quanta_index_core::index::ports::IndexProjectionBuildPort::validate_instance(
            &NativeGraph,
            instance,
        )
    }

    fn reservation(&self, request: &GraphQuery) -> Result<ResourceNeed, IndexError> {
        if request.max_supporters == 0
            || !usize::try_from(request.max_supporters)
                .is_ok_and(|supporters| supporters <= MAX_NATIVE_GRAPH_ITEMS)
            || request.max_edges == 0
            || request.max_edges > MAX_QUERY_EDGES
        {
            return Err(IndexError::InvalidInput(
                "graph query bound outside service policy".into(),
            ));
        }
        Ok(self.need)
    }

    fn query(
        &self,
        request: &GraphQuery,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<GraphPage>, IndexError> {
        self.validate_instance(context.instance)?;
        let _need = self.reservation(request)?;
        context.budget.checkpoint()?;
        let member = Self::pinned_member(context)?;
        let page = query_graph(
            &member,
            request,
            context.maps,
            context.objects,
            context.budget,
        )?;
        QueryOutcome::core_scanned(context.ticket, context.instance, true, page)
    }
}

fn query_graph(
    selected: &MemberBinding,
    request: &GraphQuery,
    maps: &dyn PersistentMapPort,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<GraphPage, IndexError> {
    if request.max_supporters == 0
        || !usize::try_from(request.max_supporters)
            .is_ok_and(|supporters| supporters <= MAX_NATIVE_GRAPH_ITEMS)
        || request.max_edges == 0
        || request.max_edges > MAX_QUERY_EDGES
    {
        return Err(IndexError::InvalidInput(
            "graph query bound outside service policy".into(),
        ));
    }
    let root = read_root_for_query(selected, objects, budget)?;
    // All transient encoded keys are hard bounded by the index key contract.
    // Keep the corresponding budget lease through the query's last key copy.
    let key_bytes = super::affected::MAX_KEY
        .checked_mul(4)
        .ok_or(IndexError::ResourceLimited)?;
    let _key_memory = budget.reserve_memory(
        u64::try_from(key_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let node_prefix = node_prefix(&request.source)?;
    let node_refs = scan_prefix(
        root.nodes,
        &node_prefix,
        request.max_supporters,
        maps,
        budget,
    )?;
    // Keep result data and its leases in one value on every error path.
    // GraphPage drops its data fields before `memory`.
    let mut result = GraphPage {
        source: None,
        source_supporters: 0,
        edges: Vec::new(),
        memory: Vec::new(),
    };
    for (key, reference) in &node_refs.entries {
        let loaded = load_support(*reference, objects, budget)?;
        let support = loaded.support;
        let GraphElement::Node(node) = support.element else {
            return Err(IndexError::CorruptObject("node index contains edge".into()));
        };
        if key != &node_key(&node.identity, support.owner)?
            || node.identity != request.source
            || result
                .source
                .as_ref()
                .is_some_and(|prior: &GraphNode| prior != &node)
        {
            return Err(IndexError::CorruptObject(
                "graph node supporter mismatch".into(),
            ));
        }
        if result.source.is_none() {
            reserve_result(
                key.len(),
                std::mem::size_of::<GraphNode>(),
                budget,
                &mut result.memory,
            )?;
            result.source = Some(node);
        }
    }
    result.source_supporters =
        u32::try_from(node_refs.entries.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let (adjacency_root, adjacency_prefix) = match request.direction {
        NativeGraphDirection::Outgoing => (root.edges, edge_prefix(&request.source)?),
        NativeGraphDirection::Incoming => (root.incoming, incoming_prefix(&request.source)?),
    };
    // `max_supporters` is per identity. Distinct directed edges may each have
    // that many supporters, so the raw supporter scan needs the product bound.
    // Keep one hard global ceiling on the indexed work before grouping rows.
    let edge_support_limit = request
        .max_edges
        .checked_mul(request.max_supporters)
        .ok_or(IndexError::ResourceLimited)?
        .min(u32::try_from(MAX_NATIVE_GRAPH_ITEMS).map_err(|_error| IndexError::ResourceLimited)?);
    let edge_refs = scan_prefix(
        adjacency_root,
        &adjacency_prefix,
        edge_support_limit,
        maps,
        budget,
    )?;
    for (key, reference) in &edge_refs.entries {
        let loaded = load_support(*reference, objects, budget)?;
        let support = loaded.support;
        let GraphElement::Edge(edge) = support.element else {
            return Err(IndexError::CorruptObject("edge index contains node".into()));
        };
        let expected_key = match request.direction {
            NativeGraphDirection::Outgoing => edge_key(&edge, support.owner)?,
            NativeGraphDirection::Incoming => incoming_key(&edge, support.owner)?,
        };
        if key != &expected_key || request.direction.endpoint(&edge) != &request.source {
            return Err(IndexError::CorruptObject(
                "graph edge anchor mismatch".into(),
            ));
        }
        if let Some(last) = result.edges.last_mut().filter(|row| row.edge == edge) {
            let next = last
                .supporters
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if next > request.max_supporters {
                return Err(IndexError::ResourceLimited);
            }
            last.supporters = next;
        } else if result.edges.len()
            >= usize::try_from(request.max_edges).map_err(|_error| IndexError::ResourceLimited)?
        {
            return Err(IndexError::ResourceLimited);
        } else {
            reserve_result(
                key.len(),
                std::mem::size_of::<GraphRow>(),
                budget,
                &mut result.memory,
            )?;
            result
                .edges
                .try_reserve(1)
                .map_err(|_error| IndexError::ResourceLimited)?;
            result.edges.push(GraphRow {
                edge,
                supporters: 1,
            });
        }
    }
    Ok(result)
}

fn reserve_result(
    key_bytes: usize,
    value_bytes: usize,
    budget: &WorkBudget,
    leases: &mut Vec<Option<MemoryLease>>,
) -> Result<(), IndexError> {
    let bytes = key_bytes
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(value_bytes))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<MemoryLease>()))
        .ok_or(IndexError::ResourceLimited)?;
    if let Some(lease) = budget.reserve_memory(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )? {
        leases
            .try_reserve(1)
            .map_err(|_error| IndexError::ResourceLimited)?;
        leases.push(Some(lease));
    }
    Ok(())
}

struct PrefixRefs {
    entries: Vec<(Vec<u8>, ObjectRef)>,
    _memory: Vec<MemoryLease>,
}

fn scan_prefix(
    root: quanta_index_core::index::MapRoot,
    prefix: &[u8],
    limit: u32,
    maps: &dyn PersistentMapPort,
    budget: &mut WorkBudget,
) -> Result<PrefixRefs, IndexError> {
    let mut entries = Vec::new();
    let mut leases = Vec::new();
    loop {
        let after = entries
            .last()
            .map_or(prefix, |(key, _): &(Vec<u8>, ObjectRef)| key.as_slice());
        let page = maps.scan(root, Some(after), PAGE, budget)?;
        if page.entries.is_empty() && page.has_more {
            return Err(IndexError::BackendViolation(
                "graph prefix scan made no progress".into(),
            ));
        }
        for (key, reference) in &page.entries {
            if !key.starts_with(prefix) {
                return Ok(PrefixRefs {
                    entries,
                    _memory: leases,
                });
            }
            if entries.len()
                >= usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?
            {
                return Err(IndexError::ResourceLimited);
            }
            let bytes = key
                .len()
                .checked_mul(8)
                .and_then(|bytes| {
                    std::mem::size_of::<(Vec<u8>, ObjectRef)>()
                        .checked_add(std::mem::size_of::<MemoryLease>())
                        .and_then(|metadata| bytes.checked_add(metadata))
                })
                .ok_or(IndexError::ResourceLimited)?;
            let memory = budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?;
            entries
                .try_reserve(1)
                .map_err(|_error| IndexError::ResourceLimited)?;
            if let Some(lease) = memory {
                leases
                    .try_reserve(1)
                    .map_err(|_error| IndexError::ResourceLimited)?;
                leases.push(lease);
            }
            let mut copied = Vec::new();
            budget.charge(quanta_index_core::index::WorkUsage {
                bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
                ..Default::default()
            })?;
            copied
                .try_reserve_exact(key.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            copied.extend_from_slice(key);
            entries.push((copied, reference.object_ref()?));
        }
        if !page.has_more {
            return Ok(PrefixRefs {
                entries,
                _memory: leases,
            });
        }
    }
}
