//! Exact, bounded prior RepoMap facts for one Source-owned file contribution.

use quanta_index_contract::{
    RepoMapNodeRef,
    index::{MemberBinding, OwnerKey},
    native_protocol::{
        MAX_NATIVE_GRAPH_ITEMS, NativeRepoMapPriorFileV1, NativeRepoMapPriorNodeDegreeV1,
        native_repomap_rank_policy,
    },
};
use quanta_index_core::index::{
    IndexError, IndexQueryPort, MemoryLease, MemoryLifetime, QueryOutcome,
    QueryReadContext, WorkBudget, owner_map_key,
    effort::ResourceNeed,
    ports::{ImmutableObjectPort, PersistentMapPort},
};

use super::{NativeGraph, read_root_for_query};
use super::affected::{
    edge_key, edge_prefix, incoming_key, incoming_prefix, node_key,
};
use super::contribution::{
    GraphElement, LoadedGraphOutput, load_output_retained, load_owner, load_projection,
    load_support,
    validate_rank_coverage, validate_source_owned_facts,
};
use super::query::{NativeGraphReader, scan_prefix};

/// Owns the decoded fact/edge backing and its original WorkBudget leases until
/// the caller has finished emitting the borrowed response.
pub struct NativeRepoMapPriorFileContextV1 {
    owner: OwnerKey,
    wire: NativeRepoMapPriorFileV1,
    _output_memory: Option<MemoryLease>,
    _facts_memory: Option<MemoryLease>,
    _degrees_memory: Option<MemoryLease>,
}

impl NativeRepoMapPriorFileContextV1 {
    pub fn owner(&self) -> OwnerKey {
        self.owner
    }

    pub fn wire(&self) -> &NativeRepoMapPriorFileV1 {
        &self.wire
    }
}

pub struct NativeRepoMapPriorFileReaderV1 {
    need: ResourceNeed,
}

impl NativeRepoMapPriorFileReaderV1 {
    pub fn new(need: ResourceNeed) -> Result<Self, IndexError> {
        if need.cpu_units == 0
            || need.io_bytes == 0
            || need.memory_bytes == 0
            || need.provider_units != 0
        {
            return Err(IndexError::InvalidInput(
                "prior RepoMap file read reservation is invalid".into(),
            ));
        }
        Ok(Self { need })
    }
}

impl IndexQueryPort<OwnerKey, Option<NativeRepoMapPriorFileContextV1>>
    for NativeRepoMapPriorFileReaderV1
{
    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        quanta_index_core::index::ports::IndexProjectionBuildPort::validate_instance(
            &NativeGraph,
            instance,
        )?;
        if instance.policy != native_repomap_rank_policy() {
            return Err(IndexError::InvalidInput(
                "prior RepoMap file requires the rank policy".into(),
            ));
        }
        Ok(())
    }

    fn reservation(&self, _request: &OwnerKey) -> Result<ResourceNeed, IndexError> {
        Ok(self.need)
    }

    fn query(
        &self,
        request: &OwnerKey,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<Option<NativeRepoMapPriorFileContextV1>>, IndexError> {
        self.validate_instance(context.instance)?;
        let _need = self.reservation(request)?;
        context.budget.checkpoint()?;
        let member = NativeGraphReader::pinned_member(context)?;
        let result = NativeGraph.read_prior_file_context_v1(
            &member,
            *request,
            context.maps,
            context.objects,
            context.budget,
        )?;
        QueryOutcome::core_scanned(context.ticket, context.instance, true, result)
    }
}

impl NativeGraph {
    /// Reopen one file owner from the selected immutable Graph member. The
    /// caller must supply the member from an exact read ticket; the root,
    /// projection and Source record are independently checked below.
    fn read_prior_file_context_v1(
        &self,
        member: &MemberBinding,
        owner: OwnerKey,
        maps: &dyn PersistentMapPort,
        objects: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<Option<NativeRepoMapPriorFileContextV1>, IndexError> {
        let root = read_root_for_query(member, objects, budget)?;
        if root.projection.producer_policy != native_repomap_rank_policy()
            || owner.producer != root.projection.producer
            || root.input.schema != root.projection.schema
            || root.input.context != root.projection.materialization_context()
        {
            return Err(IndexError::InvalidInput(
                "prior RepoMap file projection authority differs".into(),
            ));
        }
        let key = owner_map_key(owner);
        let projected = maps.lookup_object(root.projection.owners, &key, budget)?;
        let recorded = maps.lookup_object(root.owners, &key, budget)?;
        let (Some(contribution_ref), Some(record_ref)) = (projected, recorded) else {
            if projected.is_none() && recorded.is_none() {
                return Ok(None);
            }
            return Err(IndexError::CorruptObject(
                "prior RepoMap owner and Graph reverse index differ".into(),
            ));
        };
        let record = load_owner(record_ref, objects, budget)?;
        let contribution = load_projection(contribution_ref, objects, budget)?;
        if record.owner != owner
            || record.projection != contribution_ref
            || contribution.owner != owner
            || contribution.projection != root.projection.projection
            || contribution.producer != root.projection.producer
            || record.output != contribution.output
        {
            return Err(IndexError::CorruptObject(
                "prior RepoMap owner contribution differs".into(),
            ));
        }
        let source_owner = contribution.source_owner.ok_or(IndexError::InvalidInput(
            "prior RepoMap owner is not Source-owned".into(),
        ))?;
        let source = contribution.source.ok_or(IndexError::InvalidInput(
            "prior RepoMap file lacks Source record".into(),
        ))?;
        if maps.lookup_object(
                root.projection.source.owners,
                &owner_map_key(source_owner),
                budget,
            )? != Some(source)
        {
            return Err(IndexError::CorruptObject(
                "prior RepoMap file Source cut differs".into(),
            ));
        }
        let output_ref = contribution.output.ok_or(IndexError::InvalidInput(
            "prior RepoMap file has no graph output".into(),
        ))?;
        let loaded = load_output_retained(output_ref, objects, budget)?;
        validate_source_owned_facts(&loaded, &contribution, objects, budget)?;
        validate_rank_coverage(&loaded.output, root.projection.producer_policy)?;
        validate_indexed_supports(&root, owner, &loaded, maps, objects, budget)?;
        let (degrees, degrees_memory) = incident_degrees(
            &root,
            &loaded,
            maps,
            objects,
            budget,
        )?;
        let (output, facts, output_memory, facts_memory) = loaded.into_prior_parts()?;
        Ok(Some(NativeRepoMapPriorFileContextV1 {
            owner,
            wire: NativeRepoMapPriorFileV1 {
                contribution: contribution_ref,
                source_owner,
                source,
                output: output_ref,
                facts,
                edges: output.edges,
                degrees,
            },
            _output_memory: output_memory,
            _facts_memory: facts_memory,
            _degrees_memory: degrees_memory,
        }))
    }
}

fn validate_indexed_supports(
    root: &super::GraphRoot,
    owner: OwnerKey,
    loaded: &LoadedGraphOutput,
    maps: &dyn PersistentMapPort,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    for node in &loaded.output.nodes {
        let reference = maps
            .lookup_object(root.nodes, &node_key(&node.identity, owner)?, budget)?
            .ok_or(IndexError::CorruptObject(
                "prior RepoMap node is absent from Graph root".into(),
            ))?;
        let support = load_support(reference, objects, budget)?;
        let rank = loaded
            .output
            .projection
            .binary_search_by(|row| row.subject.cmp(&node.identity))
            .ok()
            .and_then(|position| loaded.output.projection.get(position).map(|row| row.row));
        if support.support.owner != owner
            || support.support.projection != rank
            || !matches!(&support.support.element, GraphElement::Node(found) if found == node)
        {
            return Err(IndexError::CorruptObject(
                "prior RepoMap node support differs".into(),
            ));
        }
    }
    for edge in &loaded.output.edges {
        if loaded
            .output
            .nodes
            .binary_search_by(|node| node.identity.cmp(&edge.source))
            .is_err()
        {
            return Err(IndexError::CorruptObject(
                "prior RepoMap file does not own an outgoing edge source".into(),
            ));
        }
        for (root, key) in [
            (root.edges, edge_key(edge, owner)?),
            (root.incoming, incoming_key(edge, owner)?),
        ] {
            let reference = maps.lookup_object(root, &key, budget)?.ok_or(
                IndexError::CorruptObject("prior RepoMap edge is absent from Graph root".into()),
            )?;
            let support = load_support(reference, objects, budget)?;
            if support.support.owner != owner
                || support.support.projection.is_some()
                || !matches!(&support.support.element, GraphElement::Edge(found) if found == edge)
            {
                return Err(IndexError::CorruptObject(
                    "prior RepoMap edge support differs".into(),
                ));
            }
        }
    }
    Ok(())
}

fn incident_degrees(
    root: &super::GraphRoot,
    loaded: &LoadedGraphOutput,
    maps: &dyn PersistentMapPort,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<(Vec<NativeRepoMapPriorNodeDegreeV1>, Option<MemoryLease>), IndexError> {
    let count = loaded.output.nodes.len();
    let bytes = count
        .checked_mul(std::mem::size_of::<NativeRepoMapPriorNodeDegreeV1>())
        .ok_or(IndexError::ResourceLimited)?;
    let memory = budget.reserve_memory(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let mut degrees = Vec::new();
    degrees
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut scanned = 0usize;
    for node in &loaded.output.nodes {
        let outgoing = scan_degree(
            root.edges,
            &node.identity,
            false,
            &mut scanned,
            maps,
            objects,
            budget,
        )?;
        let incoming = scan_degree(
            root.incoming,
            &node.identity,
            true,
            &mut scanned,
            maps,
            objects,
            budget,
        )?;
        degrees.push(NativeRepoMapPriorNodeDegreeV1 { incoming, outgoing });
    }
    Ok((degrees, memory))
}

fn scan_degree(
    root: quanta_index_core::index::MapRoot,
    identity: &RepoMapNodeRef,
    incoming: bool,
    scanned: &mut usize,
    maps: &dyn PersistentMapPort,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<u32, IndexError> {
    let prefix = if incoming {
        incoming_prefix(identity)?
    } else {
        edge_prefix(identity)?
    };
    let remaining = MAX_NATIVE_GRAPH_ITEMS
        .checked_sub(*scanned)
        .ok_or(IndexError::ResourceLimited)?;
    let refs = scan_prefix(
        root,
        &prefix,
        u32::try_from(remaining).map_err(|_error| IndexError::ResourceLimited)?,
        maps,
        budget,
    )?;
    *scanned = (*scanned)
        .checked_add(refs.entries.len())
        .ok_or(IndexError::ResourceLimited)?;
    let mut last: Option<&[u8]> = None;
    let mut degree = 0u32;
    for (key, reference) in &refs.entries {
        let loaded = load_support(*reference, objects, budget)?;
        let support = loaded.support;
        let GraphElement::Edge(edge) = support.element else {
            return Err(IndexError::CorruptObject(
                "prior RepoMap adjacency contains node".into(),
            ));
        };
        let expected = if incoming {
            incoming_key(&edge, support.owner)?
        } else {
            edge_key(&edge, support.owner)?
        };
        if expected.as_slice() != key.as_slice()
            || if incoming {
                &edge.target != identity
            } else {
                &edge.source != identity
            }
        {
            return Err(IndexError::CorruptObject(
                "prior RepoMap adjacency key or endpoint differs".into(),
            ));
        }
        let owner_suffix = owner_map_key(support.owner).len();
        let canonical = key
            .get(..key.len().checked_sub(owner_suffix).ok_or(IndexError::CorruptObject(
                "prior RepoMap adjacency key is short".into(),
            ))?)
            .ok_or(IndexError::CorruptObject(
                "prior RepoMap adjacency key is short".into(),
            ))?;
        if last != Some(canonical) {
            degree = degree.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            last = Some(canonical);
        }
    }
    Ok(degree)
}
