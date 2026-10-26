//! Sparse, view-bound membership for producer-certified graph contributions.
//!
//! The Kernel owns publication. This module only prepares one immutable group
//! member from an already certified projection transition; it never extracts
//! graph facts from source bytes or scans old owners outside `affected`.

mod affected;
mod contribution;
mod query;
mod rank_query;

#[cfg(test)]
mod tests;

use quanta_index_contract::index::{
    GroupActivation, InputBinding, InputDomain, InstanceSpec, MemberBinding, OwnerMutationSet,
    ProjectionBinding,
};
use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MapRoot, MemoryLease, MemoryLifetime, WorkBudget,
    ports::{ImmutableObjectPort, IndexFuture, IndexProjectionBuildPort},
};
use sha2::{Digest as _, Sha256};

pub use contribution::{GraphEdge, GraphNode, GraphOwnerOutput};
pub use query::{GraphPage, GraphQuery, GraphRow, NativeGraphReader};
pub use rank_query::{NativeRepoMapRankPage, NativeRepoMapRankReader};

const MAX_AFFECTED_GRAPH_ITEMS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GraphRoot {
    input: InputBinding,
    projection: ProjectionBinding,
    owners: MapRoot,
    nodes: MapRoot,
    edges: MapRoot,
    incoming: MapRoot,
}
quanta_index_contract::native_serde_struct!(GraphRoot {
    input: InputBinding,
    projection: ProjectionBinding,
    owners: MapRoot,
    nodes: MapRoot,
    edges: MapRoot,
    incoming: MapRoot,
});

/// The shared Kernel hook supplies the selected group's old binding and its
/// already validated projection transition. `affected` is the original
/// certificate-bound mutation set, not a second delta format.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeGraph;

pub struct GraphCandidate {
    binding: MemberBinding,
}

impl NativeGraph {
    pub fn prepare_projection_delta(
        &self,
        instance: &InstanceSpec,
        prior: Option<&MemberBinding>,
        projection: &ProjectionBinding,
        affected: Option<OwnerMutationSet>,
        context: &mut AttemptContext<'_>,
    ) -> Result<GraphCandidate, IndexError> {
        let rank_policy = quanta_index_contract::native_protocol::native_repomap_rank_policy();
        if instance.policy == rank_policy && projection.producer_policy != rank_policy {
            return Err(IndexError::InvalidInput(
                "graph producer rank policy differs".into(),
            ));
        }
        let input = InputBinding {
            root: projection.owners,
            schema: instance.schema,
            policy: instance.policy,
            context: projection.materialization_context(),
        };
        if instance.schema != projection.schema
            || instance.activation != GroupActivation::CoreRequired
            || instance.input_domain
                != (InputDomain::Projection {
                    projection: projection.projection,
                    producer: projection.producer,
                })
        {
            return Err(IndexError::InvalidInput(
                "graph projection schema mismatch".into(),
            ));
        }
        let mut root = match prior {
            Some(binding) => load_root(binding, context.objects, context.budget)?,
            None => GraphRoot {
                input,
                projection: *projection,
                owners: MapRoot(None),
                nodes: MapRoot(None),
                edges: MapRoot(None),
                incoming: MapRoot(None),
            },
        };
        if let Some(binding) = prior
            && (binding.instance != instance.instance
                || binding.format != instance.format
                || binding.input != root.input
                || root.projection.projection != projection.projection
                || root.projection.producer != projection.producer)
        {
            return Err(IndexError::InvalidInput("graph predecessor differs".into()));
        }
        if prior.is_some() && affected.is_none() {
            return Err(IndexError::InvalidInput(
                "graph bootstrap has predecessor".into(),
            ));
        }
        if prior.is_none() && affected.is_some() {
            return Err(IndexError::InvalidInput(
                "graph delta has no predecessor".into(),
            ));
        }
        let prior_roots = (root.owners, root.nodes, root.edges, root.incoming);
        affected::apply(&mut root, projection, affected, context)?;
        if let Some(binding) = prior
            && binding.input == input
            && prior_roots == (root.owners, root.nodes, root.edges, root.incoming)
            && root.projection == *projection
        {
            // Reuse only when both graph data and its retained projection
            // dependencies are identical to the selected view.
            return Ok(GraphCandidate {
                binding: binding.clone(),
            });
        }
        root.input = input;
        root.projection = *projection;
        let bytes = encode_funded(&root, context.budget)?;
        let mut children = Vec::new();
        children.extend(root.owners.0);
        children.extend(root.nodes.0);
        children.extend(root.edges.0);
        children.extend(root.incoming.0);
        children.extend(root.projection.object_children());
        children.sort_unstable();
        children.dedup();
        let artifact = context.put(&bytes.bytes, &children)?;
        Ok(GraphCandidate {
            binding: MemberBinding {
                instance: instance.instance,
                format: instance.format,
                artifact,
                input,
                logical_commitment: Digest(Sha256::digest(&bytes.bytes).into()),
            },
        })
    }

    pub fn verify_and_seal(
        &self,
        candidate: GraphCandidate,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError> {
        context.budget.checkpoint()?;
        let root = load_root(&candidate.binding, context.objects, context.budget)?;
        let projected_owners = root.projection.owners;
        if root.input.root != projected_owners
            || root.input.schema != root.projection.schema
            || root.input.context != root.projection.materialization_context()
        {
            return Err(IndexError::CorruptObject(
                "graph candidate input mismatch".into(),
            ));
        }
        Ok(candidate.binding)
    }
}

impl IndexProjectionBuildPort for NativeGraph {
    type Candidate = GraphCandidate;

    fn resource_demand(&self) -> quanta_index_core::index::effort::ResourceNeed {
        quanta_index_core::index::effort::ResourceNeed::default()
    }

    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        if instance.activation != GroupActivation::CoreRequired
            || !matches!(instance.input_domain, InputDomain::Projection { .. })
        {
            return Err(IndexError::InvalidInput(
                "graph adapter requires a core projection instance".into(),
            ));
        }
        Ok(())
    }

    fn prepare_projection<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<MemberBinding>,
        projection: ProjectionBinding,
        affected: Option<OwnerMutationSet>,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, GraphCandidate> {
        Box::pin(async move {
            self.validate_instance(instance)?;
            self.prepare_projection_delta(instance, prior.as_ref(), &projection, affected, context)
        })
    }

    fn verify_projection<'a>(
        &'a self,
        candidate: GraphCandidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        Box::pin(async move { self.verify_and_seal(candidate, context) })
    }
}

fn load_root(
    binding: &MemberBinding,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<GraphRoot, IndexError> {
    let stored = objects.get(binding.artifact, budget)?;
    if stored.payload.len() > quanta_index_core::index::MAX_BINDING_METADATA_BYTES {
        return Err(IndexError::CorruptObject(
            "graph root exceeds fixed bound".into(),
        ));
    }
    let allowance = stored
        .payload
        .len()
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(512))
        .ok_or(IndexError::ResourceLimited)?;
    let _memory = budget.reserve_memory(
        u64::try_from(allowance).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    if stored.registration.reference != binding.artifact {
        return Err(IndexError::CorruptObject(
            "graph root reference mismatch".into(),
        ));
    }
    let root: GraphRoot = serde_json::from_slice(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("graph root: {error}")))?;
    if root.input != binding.input
        || binding.logical_commitment != Digest(Sha256::digest(&stored.payload).into())
    {
        return Err(IndexError::CorruptObject(
            "graph root binding mismatch".into(),
        ));
    }
    let mut children = Vec::new();
    children.extend(root.owners.0);
    children.extend(root.nodes.0);
    children.extend(root.edges.0);
    children.extend(root.incoming.0);
    children.extend(root.projection.object_children());
    children.sort_unstable();
    children.dedup();
    if stored.registration.children != children {
        return Err(IndexError::CorruptObject(
            "graph root children mismatch".into(),
        ));
    }
    Ok(root)
}

fn read_root_for_query(
    binding: &MemberBinding,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<GraphRoot, IndexError> {
    let root = load_root(binding, objects, budget)?;
    let projected_owners = root.projection.owners;
    if root.input.root != projected_owners {
        return Err(IndexError::CorruptObject(
            "graph root projection mismatch".into(),
        ));
    }
    Ok(root)
}

pub(super) struct FundedGraphBytes {
    pub bytes: Vec<u8>,
    _memory: Option<MemoryLease>,
}

pub(super) fn encode_funded<T: serde::Serialize>(
    value: &T,
    budget: &mut WorkBudget,
) -> Result<FundedGraphBytes, IndexError> {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("graph encoding length overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value)
        .map_err(|error| IndexError::InvalidInput(format!("graph encode: {error}")))?;
    if count.0 > quanta_index_core::index::MAX_BINDING_METADATA_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let bytes = u64::try_from(count.0).map_err(|_error| IndexError::ResourceLimited)?;
    budget.charge(quanta_index_core::index::WorkUsage {
        bytes,
        ..Default::default()
    })?;
    let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(count.0)
        .map_err(|_error| IndexError::ResourceLimited)?;
    serde_json::to_writer(&mut output, value)
        .map_err(|error| IndexError::InvalidInput(format!("graph encode: {error}")))?;
    if output.len() != count.0 {
        return Err(IndexError::BackendViolation(
            "graph encoding size changed".into(),
        ));
    }
    Ok(FundedGraphBytes {
        bytes: output,
        _memory: memory,
    })
}
