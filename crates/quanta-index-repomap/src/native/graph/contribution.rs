use quanta_index_contract::index::ProjectionOwnerContribution;
pub use quanta_index_contract::native_protocol::{GraphEdge, GraphNode, GraphOwnerOutput};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey, StoredObject, WorkBudget,
    ports::ImmutableObjectPort,
};

pub(super) struct LoadedGraphOutput {
    pub output: GraphOwnerOutput,
    _memory: Option<MemoryLease>,
}
pub(super) struct LoadedGraphSupport {
    pub support: GraphSupport,
    // Keep the decoded identity allocation charged through its last use.
    // Fields drop in declaration order, so the support dies before this lease.
    _memory: Option<MemoryLease>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OwnerGraphRecord {
    pub owner: OwnerKey,
    pub projection: ObjectRef,
    pub output: Option<ObjectRef>,
}
quanta_index_contract::native_serde_struct!(OwnerGraphRecord {
    owner: OwnerKey,
    projection: ObjectRef,
    output: Option<ObjectRef>,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct GraphSupport {
    pub owner: OwnerKey,
    pub element: GraphElement,
    pub projection: Option<ObjectRef>,
}
quanta_index_contract::native_serde_struct!(GraphSupport {
    owner: OwnerKey,
    element: GraphElement,
    projection: Option<ObjectRef>,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum GraphElement {
    Node(GraphNode),
    Edge(GraphEdge),
}
quanta_index_contract::native_serde_enum!(GraphElement {
    units {}
    newtypes { Node = 0 => GraphNode, Edge = 1 => GraphEdge }
    records {}
});

pub(super) fn load_projection(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<ProjectionOwnerContribution, IndexError> {
    let stored = objects.get(reference, budget)?;
    let contribution: ProjectionOwnerContribution = decode_small(&stored, reference, budget)?;
    if stored.registration.children != contribution.object_children() {
        return Err(IndexError::CorruptObject(
            "graph projection children mismatch".into(),
        ));
    }
    Ok(contribution)
}

pub(super) fn load_output(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<LoadedGraphOutput, IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.payload.is_empty() {
        return Err(IndexError::CorruptObject(
            "graph output payload is empty".into(),
        ));
    }
    if stored.payload.len() > 4 * 1024 * 1024 {
        return Err(IndexError::ResourceLimited);
    }
    let allowance = stored
        .payload
        .len()
        .checked_mul(8)
        .ok_or(IndexError::ResourceLimited)?;
    let memory = budget.reserve_memory(
        u64::try_from(allowance).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let output: GraphOwnerOutput = decode(&stored, reference)?;
    let mut children: Vec<ObjectRef> = output.projection.iter().map(|row| row.row).collect();
    children.sort_unstable();
    children.dedup();
    if stored.registration.children != children {
        return Err(IndexError::CorruptObject(
            "graph output children mismatch".into(),
        ));
    }
    validate_output(&output)?;
    Ok(LoadedGraphOutput {
        output,
        _memory: memory,
    })
}

pub(super) fn load_owner(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<OwnerGraphRecord, IndexError> {
    let stored = objects.get(reference, budget)?;
    let record: OwnerGraphRecord = decode_small(&stored, reference, budget)?;
    let mut children = vec![record.projection];
    children.extend(record.output);
    children.sort_unstable();
    children.dedup();
    if stored.registration.children != children {
        return Err(IndexError::CorruptObject(
            "graph owner children mismatch".into(),
        ));
    }
    Ok(record)
}

pub(super) fn load_support(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<LoadedGraphSupport, IndexError> {
    let stored = objects.get(reference, budget)?;
    let (support, memory) = decode_small_with_memory(&stored, reference, budget)?;
    let loaded = LoadedGraphSupport {
        support,
        _memory: memory,
    };
    let expected = loaded.support.projection;
    if stored.registration.children.iter().copied().ne(expected)
        || (matches!(loaded.support.element, GraphElement::Edge(_)) && expected.is_some())
    {
        return Err(IndexError::CorruptObject(
            "graph supporter children mismatch".into(),
        ));
    }
    Ok(loaded)
}

fn decode<T: serde::de::DeserializeOwned + serde::Serialize>(
    stored: &StoredObject,
    reference: ObjectRef,
) -> Result<T, IndexError> {
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "graph object reference mismatch".into(),
        ));
    }
    let value: T = serde_json::from_slice(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("graph object: {error}")))?;
    let canonical = serde_json::to_vec(&value)
        .map_err(|error| IndexError::CorruptObject(format!("graph object: {error}")))?;
    if canonical != stored.payload {
        return Err(IndexError::CorruptObject(
            "graph object encoding mismatch".into(),
        ));
    }
    Ok(value)
}

fn decode_small<T: Copy + serde::de::DeserializeOwned + serde::Serialize>(
    stored: &StoredObject,
    reference: ObjectRef,
    budget: &WorkBudget,
) -> Result<T, IndexError> {
    decode_small_with_memory(stored, reference, budget).map(|(value, _memory)| value)
}

fn decode_small_with_memory<T: serde::de::DeserializeOwned + serde::Serialize>(
    stored: &StoredObject,
    reference: ObjectRef,
    budget: &WorkBudget,
) -> Result<(T, Option<MemoryLease>), IndexError> {
    if stored.payload.len() > quanta_index_core::index::MAX_BINDING_METADATA_BYTES {
        return Err(IndexError::CorruptObject(
            "graph metadata exceeds fixed bound".into(),
        ));
    }
    let allowance = stored
        .payload
        .len()
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(512))
        .ok_or(IndexError::ResourceLimited)?;
    let memory = budget.reserve_memory(
        u64::try_from(allowance).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    Ok((decode(stored, reference)?, memory))
}

pub(super) fn validate_output(output: &GraphOwnerOutput) -> Result<(), IndexError> {
    let mut previous = None;
    for node in &output.nodes {
        if previous.is_some_and(|prior| prior >= &node.identity) {
            return Err(IndexError::InvalidInput(
                "graph nodes are not sorted and unique".into(),
            ));
        }
        previous = Some(&node.identity);
    }
    let mut previous = None;
    for row in &output.projection {
        if previous.is_some_and(|prior| prior >= &row.subject)
            || output
                .nodes
                .binary_search_by(|node| node.identity.cmp(&row.subject))
                .is_err()
            || !matches!(
                row.subject,
                quanta_index_contract::RepoMapNodeRef::File(_)
                    | quanta_index_contract::RepoMapNodeRef::Symbol(_)
            )
        {
            return Err(IndexError::InvalidInput(
                "graph projection identity mismatch or duplicate".into(),
            ));
        }
        previous = Some(&row.subject);
    }
    let mut previous = None;
    for edge in &output.edges {
        let identity = (edge.kind.as_code_str(), &edge.source, &edge.target);
        if previous.is_some_and(|prior| prior >= identity) {
            return Err(IndexError::InvalidInput(
                "graph edges are not sorted and unique".into(),
            ));
        }
        previous = Some(identity);
    }
    Ok(())
}

/// A recognized rank producer cannot publish an unsearchable File/Symbol row.
pub(super) fn validate_rank_coverage(
    output: &GraphOwnerOutput,
    producer_policy: quanta_index_core::index::Digest,
) -> Result<(), IndexError> {
    if producer_policy != quanta_index_contract::native_protocol::native_repomap_rank_policy() {
        return Ok(());
    }
    for node in &output.nodes {
        if matches!(
            node.identity,
            quanta_index_contract::RepoMapNodeRef::File(_)
                | quanta_index_contract::RepoMapNodeRef::Symbol(_)
        ) && output
            .projection
            .binary_search_by(|row| row.subject.cmp(&node.identity))
            .is_err()
        {
            return Err(IndexError::InvalidInput(
                "rank policy requires projection for every searchable node".into(),
            ));
        }
    }
    Ok(())
}

/// Canonical rank row and its decoded backing stay charged together.
pub(super) struct LoadedRankProjection {
    pub entry: quanta_index_contract::CompiledRepoMapProjectionEntryV1,
    _memory: Option<MemoryLease>,
}

pub(super) fn load_rank_projection(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<LoadedRankProjection, IndexError> {
    let stored = objects.get(reference, budget)?;
    // One preview is capped at 1MiB, with bounded path/identity and JSON escaping.
    if stored.payload.len()
        > quanta_index_contract::native_protocol::MAX_REPOMAP_RANK_PROJECTION_LEAF_BYTES
    {
        return Err(IndexError::ResourceLimited);
    }
    let bytes = stored
        .payload
        .len()
        .checked_mul(8)
        .and_then(|value| value.checked_add(2048))
        .ok_or(IndexError::ResourceLimited)?;
    let memory = budget.reserve_memory(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let entry = decode(&stored, reference)?;
    let loaded = LoadedRankProjection {
        entry,
        _memory: memory,
    };
    if !stored.registration.children.is_empty() {
        return Err(IndexError::CorruptObject(
            "rank projection is not a leaf".into(),
        ));
    }
    Ok(loaded)
}
