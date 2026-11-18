//! Bounded ticket-pinned adjacency over the exact old compiled graph tables.

use quanta_index_contract::native_protocol::{
    MAX_NATIVE_GRAPH_EDGES, NativeGraphDirection, NativeImportedRepoMapCursorV1,
    NativeImportedRepoMapEdgeV1, NativeImportedRepoMapNodeV1, NativeImportedRepoMapQueryV1,
};
use quanta_index_core::index::{
    IndexError, IndexQueryPort, MemoryLease, MemoryLifetime, QueryOutcome, QueryReadContext,
    effort::ResourceNeed,
};

use crate::native::graph::NativeGraphReader;

use super::{NativeLegacyRepoMapBuildV1, NativeLegacyRepoMapReadV1};

pub struct NativeLegacyRepoMapGraphPageV1 {
    pub node: Option<NativeImportedRepoMapNodeV1>,
    pub edges: Vec<NativeImportedRepoMapEdgeV1>,
    pub exhaustive: bool,
    pub next: Option<NativeImportedRepoMapCursorV1>,
    // Result fields die before all retained Original grants.
    _row_memory: Vec<Option<MemoryLease>>,
    _vector_memory: Option<MemoryLease>,
}

impl NativeLegacyRepoMapGraphPageV1 {
    pub fn into_parts(
        self,
    ) -> (
        Option<NativeImportedRepoMapNodeV1>,
        Vec<NativeImportedRepoMapEdgeV1>,
        bool,
        Option<NativeImportedRepoMapCursorV1>,
        Vec<Option<MemoryLease>>,
    ) {
        let Self {
            node,
            edges,
            exhaustive,
            next,
            mut _row_memory,
            _vector_memory,
        } = self;
        _row_memory.push(_vector_memory);
        (node, edges, exhaustive, next, _row_memory)
    }
}

pub struct NativeLegacyRepoMapGraphReaderV1 {
    need: ResourceNeed,
}

impl NativeLegacyRepoMapGraphReaderV1 {
    pub fn new(need: ResourceNeed) -> Result<Self, IndexError> {
        if need.cpu_units == 0
            || need.io_bytes == 0
            || need.memory_bytes == 0
            || need.provider_units != 0
        {
            return Err(IndexError::InvalidInput(
                "Legacy RepoMap graph reservation differs".into(),
            ));
        }
        Ok(Self { need })
    }
}

impl IndexQueryPort<NativeImportedRepoMapQueryV1, NativeLegacyRepoMapGraphPageV1>
    for NativeLegacyRepoMapGraphReaderV1
{
    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        NativeLegacyRepoMapBuildV1::validate_profile(instance)
    }

    fn reservation(
        &self,
        request: &NativeImportedRepoMapQueryV1,
    ) -> Result<ResourceNeed, IndexError> {
        if !request.valid_v1() {
            return Err(IndexError::InvalidInput(
                "Legacy RepoMap graph request differs".into(),
            ));
        }
        Ok(self.need)
    }

    fn query(
        &self,
        request: &NativeImportedRepoMapQueryV1,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<NativeLegacyRepoMapGraphPageV1>, IndexError> {
        self.validate_instance(context.instance)?;
        let _need = self.reservation(request)?;
        let member = NativeGraphReader::pinned_member(context)?;
        let read = NativeLegacyRepoMapReadV1::open(
            &member,
            context.ticket.view.source,
            context.objects,
            context.budget,
        )?;
        let source_digest = {
            let bytes = super::build::digest_key(5, &request.source)?;
            quanta_index_contract::index::Digest(
                bytes.try_into().map_err(|_| IndexError::DigestConflict)?,
            )
        };
        let after_start = request
            .after
            .map(|cursor| {
                if cursor.member != member.artifact
                    || cursor.payload_sha256 != read.payload_sha256()
                    || cursor.source_digest != source_digest
                    || cursor.direction != request.direction
                {
                    return Err(IndexError::DigestConflict);
                }
                Ok(cursor.start)
            })
            .transpose()?;
        let capacity =
            usize::try_from(request.max_edges).map_err(|_| IndexError::ResourceLimited)?;
        if capacity
            > usize::try_from(MAX_NATIVE_GRAPH_EDGES).map_err(|_| IndexError::ResourceLimited)?
        {
            return Err(IndexError::ResourceLimited);
        }
        let bytes = capacity
            .checked_mul(
                std::mem::size_of::<NativeImportedRepoMapEdgeV1>()
                    + std::mem::size_of::<Option<MemoryLease>>(),
            )
            .and_then(|bytes| bytes.checked_add(4096))
            .ok_or(IndexError::ResourceLimited)?;
        let vector_memory = context.budget.reserve_memory(
            u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?;
        let mut edges = Vec::new();
        edges
            .try_reserve_exact(capacity)
            .map_err(|_| IndexError::ResourceLimited)?;
        if edges.capacity() > capacity {
            return Err(IndexError::ResourceLimited);
        }
        let mut row_memory = Vec::new();
        row_memory
            .try_reserve_exact(capacity.checked_add(2).ok_or(IndexError::ResourceLimited)?)
            .map_err(|_| IndexError::ResourceLimited)?;
        if row_memory.capacity() > capacity + 2 {
            return Err(IndexError::ResourceLimited);
        }
        let node = read
            .lookup_node(&request.source, context.maps, context.budget)?
            .map(|loaded| {
                row_memory.push(loaded._memory);
                NativeImportedRepoMapNodeV1 {
                    identity: loaded.value.identity,
                    degree_in: loaded.value.degree_in,
                    degree_out: loaded.value.degree_out,
                }
            });
        let visit = |start: u64,
                     loaded: super::LoadedLegacyRepoMapRowV1<
            quanta_index_contract::CompiledRepoMapEdgeV1,
        >,
                     _budget: &mut quanta_index_core::index::WorkBudget| {
            row_memory.push(loaded._memory);
            edges.push(NativeImportedRepoMapEdgeV1 {
                cursor_start: start,
                kind: loaded.value.kind,
                source: loaded.value.source,
                target: loaded.value.target,
            });
            Ok(())
        };
        let next_start = match request.direction {
            NativeGraphDirection::Outgoing => read.visit_outgoing_edges(
                &request.source,
                request.max_edges,
                after_start,
                context.maps,
                context.budget,
                visit,
            )?,
            NativeGraphDirection::Incoming => read.visit_incoming_edges(
                &request.source,
                request.max_edges,
                after_start,
                context.maps,
                context.budget,
                visit,
            )?,
        };
        if node.is_none() && (!edges.is_empty() || next_start.is_some()) {
            return Err(IndexError::CorruptObject(
                "Legacy RepoMap edge has no node".into(),
            ));
        }
        let page = NativeLegacyRepoMapGraphPageV1 {
            node,
            edges,
            exhaustive: next_start.is_none(),
            next: next_start.map(|start| NativeImportedRepoMapCursorV1 {
                member: member.artifact,
                payload_sha256: read.payload_sha256(),
                source_digest,
                direction: request.direction,
                start,
            }),
            _row_memory: row_memory,
            _vector_memory: vector_memory,
        };
        QueryOutcome::core_scanned(context.ticket, context.instance, true, page)
    }
}
