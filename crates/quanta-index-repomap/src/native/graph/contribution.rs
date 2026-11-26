use quanta_index_contract::index::MAX_SOURCE_PATH_BYTES;
use quanta_index_contract::index::ProjectionOwnerContribution;
use quanta_index_contract::native_protocol::GraphProjectionRow;
pub use quanta_index_contract::native_protocol::{GraphEdge, GraphNode, GraphOwnerOutput};
use quanta_index_contract::native_protocol::{
    MAX_NATIVE_GRAPH_ITEMS, MAX_NATIVE_GRAPH_OUTPUT_BYTES, NativeRepoMapFileFactsV1,
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey, SourceRecordOwner, StoredObject,
    WorkBudget, ports::ImmutableObjectPort,
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, SeqAccess, Visitor},
};
use std::{fmt, marker::PhantomData};

/// The v20 output wire before typed facts. Keep its bounds and canonical
/// encoding independent of the current four-field output contract.
struct LegacyGraphOwnerOutput {
    nodes: LimitedGraphItems<GraphNode>,
    edges: LimitedGraphItems<GraphEdge>,
    projection: LimitedGraphItems<GraphProjectionRow>,
}
quanta_index_contract::native_serde_struct!(LegacyGraphOwnerOutput {
    nodes: LimitedGraphItems<GraphNode>,
    edges: LimitedGraphItems<GraphEdge>,
    projection: LimitedGraphItems<GraphProjectionRow>,
});

struct LimitedGraphItems<T>(Vec<T>);

impl<T: Serialize> Serialize for LimitedGraphItems<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for LimitedGraphItems<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Items<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Items<T> {
            type Value = LimitedGraphItems<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded graph array")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<T>()? {
                    if items.len() >= MAX_NATIVE_GRAPH_ITEMS {
                        return Err(de::Error::custom("graph output exceeds item bound"));
                    }
                    items
                        .try_reserve(1)
                        .map_err(|_| de::Error::custom("graph output allocation refused"))?;
                    items.push(item);
                }
                Ok(LimitedGraphItems(items))
            }
        }
        deserializer.deserialize_seq(Items(PhantomData))
    }
}

pub(super) struct LoadedGraphOutput {
    pub output: GraphOwnerOutput,
    pub facts: Option<NativeRepoMapFileFactsV1>,
    _memory: Option<MemoryLease>,
    _facts_memory: Option<MemoryLease>,
}
impl LoadedGraphOutput {
    pub(super) fn into_prior_parts(
        self,
    ) -> Result<
        (
            GraphOwnerOutput,
            NativeRepoMapFileFactsV1,
            Option<MemoryLease>,
            Option<MemoryLease>,
        ),
        IndexError,
    > {
        Ok((
            self.output,
            self.facts.ok_or(IndexError::CorruptObject(
                "prior RepoMap file lacks typed facts".into(),
            ))?,
            self._memory,
            self._facts_memory,
        ))
    }
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
    load_output_with_lifetime(reference, objects, budget, MemoryLifetime::Temporary, false)
}

/// The old three-field output is valid only under a retained nonrank Graph
/// root. The current output decoder remains strict for all other callers.
pub(super) fn load_legacy_nonrank_output(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<LoadedGraphOutput, IndexError> {
    load_output_with_lifetime(reference, objects, budget, MemoryLifetime::Temporary, true)
}

pub(super) fn load_output_retained(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<LoadedGraphOutput, IndexError> {
    load_output_with_lifetime(reference, objects, budget, MemoryLifetime::Retained, false)
}

fn load_output_with_lifetime(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
    lifetime: MemoryLifetime,
    allow_legacy_nonrank: bool,
) -> Result<LoadedGraphOutput, IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.payload.is_empty() {
        return Err(IndexError::CorruptObject(
            "graph output payload is empty".into(),
        ));
    }
    if stored.payload.len() > MAX_NATIVE_GRAPH_OUTPUT_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let allowance = stored
        .payload
        .len()
        .checked_mul(8)
        .ok_or(IndexError::ResourceLimited)?;
    let memory = budget.reserve_memory(
        u64::try_from(allowance).map_err(|_error| IndexError::ResourceLimited)?,
        lifetime,
    )?;
    let output: GraphOwnerOutput = if allow_legacy_nonrank {
        decode(&stored, reference).or_else(|_| decode_legacy_output(&stored, reference))?
    } else {
        decode(&stored, reference)?
    };
    let mut children: Vec<ObjectRef> = output.projection.iter().map(|row| row.row).collect();
    children.extend(output.facts);
    children.sort_unstable();
    children.dedup();
    if stored.registration.children != children {
        return Err(IndexError::CorruptObject(
            "graph output children mismatch".into(),
        ));
    }
    validate_output(&output)?;
    let (facts, facts_memory) = if let Some(facts_ref) = output.facts {
        let (facts, memory) = load_file_facts(facts_ref, &output, objects, budget, lifetime)?;
        (Some(facts), memory)
    } else {
        (None, None)
    };
    Ok(LoadedGraphOutput {
        output,
        facts,
        _memory: memory,
        _facts_memory: facts_memory,
    })
}

fn decode_legacy_output(
    stored: &StoredObject,
    reference: ObjectRef,
) -> Result<GraphOwnerOutput, IndexError> {
    let old: LegacyGraphOwnerOutput = decode(stored, reference)?;
    if old
        .nodes
        .0
        .len()
        .checked_add(old.edges.0.len())
        .is_none_or(|count| count > MAX_NATIVE_GRAPH_ITEMS)
    {
        return Err(IndexError::CorruptObject(
            "legacy graph output exceeds item bound".into(),
        ));
    }
    Ok(GraphOwnerOutput {
        nodes: old.nodes.0,
        edges: old.edges.0,
        projection: old.projection.0,
        facts: None,
    })
}

fn load_file_facts(
    reference: ObjectRef,
    output: &GraphOwnerOutput,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
    lifetime: MemoryLifetime,
) -> Result<(NativeRepoMapFileFactsV1, Option<MemoryLease>), IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.payload.is_empty() || stored.payload.len() > MAX_NATIVE_GRAPH_OUTPUT_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    if !stored.registration.children.is_empty() {
        return Err(IndexError::CorruptObject(
            "graph file facts have children".into(),
        ));
    }
    let allowance = stored
        .payload
        .len()
        .checked_mul(8)
        .ok_or(IndexError::ResourceLimited)?;
    let memory = budget.reserve_memory(
        u64::try_from(allowance).map_err(|_error| IndexError::ResourceLimited)?,
        lifetime,
    )?;
    let facts: NativeRepoMapFileFactsV1 = decode(&stored, reference)?;
    budget.charge(quanta_index_core::index::WorkUsage {
        bytes: u64::try_from(stored.payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..quanta_index_core::index::WorkUsage::default()
    })?;
    if facts.nodes.len() > MAX_NATIVE_GRAPH_ITEMS
        || !facts
            .matches_graph_nodes_v1(&output.nodes)
            .map_err(|error| IndexError::CorruptObject(format!("graph file facts: {error}")))?
    {
        return Err(IndexError::CorruptObject(
            "graph file facts differ from node identities, paths, or payload digests".into(),
        ));
    }
    Ok((facts, memory))
}

pub(super) fn load_source_record(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<SourceRecordOwner, IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.registration.reference != reference
        || stored.payload.is_empty()
        || stored.payload.len() > quanta_index_core::index::MAX_BINDING_METADATA_BYTES
    {
        return Err(IndexError::CorruptObject(
            "graph file Source record reference or bound differs".into(),
        ));
    }
    let record =
        SourceRecordOwner::decode_json(&stored.payload, budget, MemoryLifetime::Temporary)?;
    if record.path.is_empty()
        || record.path.len() > MAX_SOURCE_PATH_BYTES
        || record.path.starts_with('/')
        || record.path.contains('\0')
        || record
            .path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || stored.registration.children.as_slice() != [record.bytes]
    {
        return Err(IndexError::CorruptObject(
            "graph file Source record path or children differ".into(),
        ));
    }
    Ok(record)
}

pub(super) fn validate_source_owned_facts(
    graph: &LoadedGraphOutput,
    contribution: &ProjectionOwnerContribution,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let facts = graph.facts.as_ref().ok_or(IndexError::InvalidInput(
        "source-owned RepoMap file lacks typed facts".into(),
    ))?;
    let source_ref = contribution.source.ok_or(IndexError::InvalidInput(
        "source-owned RepoMap file lacks exact Source record".into(),
    ))?;
    let source = load_source_record(source_ref, objects, budget)?;
    if facts.owner_path.as_str() != source.path.as_str() {
        return Err(IndexError::InvalidInput(
            "RepoMap file facts path differs from exact Source record".into(),
        ));
    }
    Ok(())
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
