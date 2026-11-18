//! FULL-only Native member for one independently verified Legacy compiled
//! RepoMap candidate. Three digest-keyed persistent maps index canonical CBOR
//! row spans; the complete original key is checked when a row is read.

use quanta_index_contract::{
    index::{
        GroupActivation, InputBinding, InputDomain, InstanceSpec, MapRoot, MemberBinding,
        ObjectRef, OwnerMutationSet, ProjectionBinding,
    },
    native_protocol::native_repomap_rank_policy,
};
use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MapEdit, MapValue, MemoryLifetime, WorkUsage,
    effort::ResourceNeed,
    owner_map_key,
    ports::{IndexFuture, IndexProjectionBuildPort},
};
use sha2::{Digest as _, Sha256};

use crate::materializer::{CompiledPayloadEntry, visit_compiled_payload_with_span};

use super::{load_verified_legacy_repomap_import_v1, minimum_query_open_credit_v1};

pub use quanta_index_contract::native_protocol::{
    NATIVE_LEGACY_REPOMAP_FORMAT_V1, NATIVE_LEGACY_REPOMAP_KIND_V1, NATIVE_LEGACY_REPOMAP_SCHEMA_V1,
};

const WINDOW: usize = 128;
const INDEX_DOMAIN: &[u8] = b"qidx.native.legacy-repomap-key.v1\0";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LegacyImportRootV1 {
    pub input: InputBinding,
    pub projection: ProjectionBinding,
    pub header: ObjectRef,
    pub nodes: MapRoot,
    pub edges: MapRoot,
    pub incoming: MapRoot,
    pub rank_rows: MapRoot,
}
quanta_index_contract::native_serde_struct!(LegacyImportRootV1 {
    input: InputBinding,
    projection: ProjectionBinding,
    header: ObjectRef,
    nodes: MapRoot,
    edges: MapRoot,
    incoming: MapRoot,
    rank_rows: MapRoot,
});

impl LegacyImportRootV1 {
    pub(super) fn children(self) -> Vec<ObjectRef> {
        let mut children = vec![self.header];
        children.extend(self.nodes.0);
        children.extend(self.edges.0);
        children.extend(self.incoming.0);
        children.extend(self.rank_rows.0);
        children.extend(self.projection.object_children());
        children.sort_unstable();
        children.dedup();
        children
    }
}

pub struct LegacyImportCandidateV1 {
    binding: MemberBinding,
}

#[derive(Clone, Copy)]
pub struct NativeLegacyRepoMapBuildV1 {
    demand: ResourceNeed,
    interactive_memory_bytes: u64,
}

impl NativeLegacyRepoMapBuildV1 {
    pub fn new(demand: ResourceNeed, interactive_memory_bytes: u64) -> Result<Self, IndexError> {
        if demand.provider_units != 0
            || demand.cpu_units == 0
            || demand.memory_bytes == 0
            || interactive_memory_bytes == 0
        {
            return Err(IndexError::InvalidInput(
                "Legacy RepoMap build needs admitted CPU and Core/Interactive memory without provider".into(),
            ));
        }
        Ok(Self {
            demand,
            interactive_memory_bytes,
        })
    }

    pub(crate) fn validate_profile(instance: &InstanceSpec) -> Result<(), IndexError> {
        if instance.kind != NATIVE_LEGACY_REPOMAP_KIND_V1
            || instance.schema != NATIVE_LEGACY_REPOMAP_SCHEMA_V1
            || instance.format != NATIVE_LEGACY_REPOMAP_FORMAT_V1
            || instance.activation != GroupActivation::CoreRequired
            || !matches!(instance.input_domain, InputDomain::Projection { .. })
            || instance.policy != native_repomap_rank_policy()
        {
            return Err(IndexError::InvalidInput(
                "Legacy RepoMap member profile differs".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn prepare_full(
        &self,
        instance: &InstanceSpec,
        projection: ProjectionBinding,
        context: &mut AttemptContext<'_>,
    ) -> Result<LegacyImportCandidateV1, IndexError> {
        self.validate_instance(instance)?;
        if projection.schema != instance.schema
            || projection.producer_policy != native_repomap_rank_policy()
            || instance.input_domain
                != (InputDomain::Projection {
                    projection: projection.projection,
                    producer: projection.producer,
                })
        {
            return Err(IndexError::InvalidInput(
                "Legacy RepoMap projection binding differs".into(),
            ));
        }
        let header_ref = one_certified_import_header(&projection, context)?;
        let import =
            load_verified_legacy_repomap_import_v1(header_ref, context.objects, context.budget)?;
        if minimum_query_open_credit_v1(import.header.payload_bytes)?
            > self.interactive_memory_bytes
        {
            return Err(IndexError::ResourceLimited);
        }
        let allowance = import
            .header
            .payload_bytes
            .checked_mul(8)
            .and_then(|bytes| bytes.checked_add(131_072))
            .ok_or(IndexError::ResourceLimited)?;
        let _parse_memory = context
            .budget
            .reserve_memory(allowance, MemoryLifetime::Temporary)?;
        let _window_memory = context.budget.reserve_memory(
            u64::try_from(
                WINDOW
                    .checked_mul(4)
                    .and_then(|count| count.checked_mul(std::mem::size_of::<MapEdit>() + 128))
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut windows = MapWindows::new()?;
        visit_compiled_payload_with_span(
            import.payload(),
            |error| IndexError::CorruptObject(error.to_string()),
            |row, span| {
                context.budget.checkpoint()?;
                let (column, key) = row_digest_key(&row, span.start)?;
                let start = u64::try_from(span.start).map_err(|_| IndexError::ResourceLimited)?;
                let end = u64::try_from(span.end).map_err(|_| IndexError::ResourceLimited)?;
                if end <= start || end > import.header.payload_bytes {
                    return Err(IndexError::CorruptObject(
                        "Legacy RepoMap row span differs".into(),
                    ));
                }
                let mut offset = [0_u8; 16];
                offset[..8].copy_from_slice(&start.to_be_bytes());
                offset[8..].copy_from_slice(&end.to_be_bytes());
                let value = MapValue::inline(&offset, [])?;
                windows.push(column, key, value, context)?;
                if let CompiledPayloadEntry::Edge(edge) = &row {
                    let mut incoming_key = digest_key(4, &edge.target)?;
                    incoming_key.extend(
                        u64::try_from(span.start)
                            .map_err(|_| IndexError::ResourceLimited)?
                            .to_be_bytes(),
                    );
                    windows.push(3, incoming_key, value, context)?;
                }
                Ok(())
            },
        )?;
        windows.flush_all(context)?;
        let input = InputBinding {
            root: projection.owners,
            schema: instance.schema,
            policy: instance.policy,
            context: projection.materialization_context(),
        };
        let root = LegacyImportRootV1 {
            input,
            projection,
            header: header_ref,
            nodes: windows.roots[0],
            edges: windows.roots[1],
            incoming: windows.roots[3],
            rank_rows: windows.roots[2],
        };
        let encoded_len = canonical_json_len(&root)?;
        let _root_memory = context.budget.reserve_memory(
            u64::try_from(
                encoded_len
                    .checked_mul(2)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let bytes = serde_json::to_vec(&root)
            .map_err(|_| IndexError::InvalidInput("Legacy RepoMap member encode refused".into()))?;
        if bytes.len() != encoded_len || bytes.capacity() > encoded_len * 2 {
            return Err(IndexError::ResourceLimited);
        }
        let artifact = context.put(&bytes, &root.children())?;
        Ok(LegacyImportCandidateV1 {
            binding: MemberBinding {
                instance: instance.instance,
                format: instance.format,
                artifact,
                input,
                logical_commitment: Digest(Sha256::digest(&bytes).into()),
            },
        })
    }
}

impl IndexProjectionBuildPort for NativeLegacyRepoMapBuildV1 {
    type Candidate = LegacyImportCandidateV1;

    fn resource_demand(&self) -> ResourceNeed {
        self.demand
    }

    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        Self::validate_profile(instance)
    }

    fn prepare_projection<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<MemberBinding>,
        projection: ProjectionBinding,
        affected: Option<OwnerMutationSet>,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            if prior.is_some() || affected.is_some() {
                return Err(IndexError::InvalidInput(
                    "Legacy RepoMap import requires FULL".into(),
                ));
            }
            self.prepare_full(instance, projection, context)
        })
    }

    fn verify_projection<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        Box::pin(async move {
            let stored = context
                .objects
                .get(candidate.binding.artifact, context.budget)?;
            if stored.registration.reference != candidate.binding.artifact
                || stored.payload.len() > quanta_index_core::index::MAX_BINDING_METADATA_BYTES
            {
                return Err(IndexError::CorruptObject(
                    "Legacy RepoMap member root differs".into(),
                ));
            }
            let _root_memory = context.budget.reserve_memory(
                u64::try_from(
                    stored
                        .payload
                        .len()
                        .checked_mul(8)
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?;
            let root: LegacyImportRootV1 =
                serde_json::from_slice(&stored.payload).map_err(|_| {
                    IndexError::CorruptObject("Legacy RepoMap member decode refused".into())
                })?;
            if stored.registration.children != root.children()
                || root.input != candidate.binding.input
                || Digest(Sha256::digest(&stored.payload).into())
                    != candidate.binding.logical_commitment
            {
                return Err(IndexError::CorruptObject(
                    "Legacy RepoMap member binding differs".into(),
                ));
            }
            context.budget.checkpoint()?;
            Ok(candidate.binding)
        })
    }
}

fn one_certified_import_header(
    projection: &ProjectionBinding,
    context: &mut AttemptContext<'_>,
) -> Result<ObjectRef, IndexError> {
    let page = context
        .maps
        .scan(projection.owners, None, 2, context.budget)?;
    if page.entries.len() != 1 || page.has_more {
        return Err(IndexError::InvalidInput(
            "Legacy RepoMap requires one certified owner".into(),
        ));
    }
    let (key, value) = &page.entries[0];
    let reference = value.object_ref()?;
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference
        || stored.payload.len() > quanta_index_core::index::MAX_BINDING_METADATA_BYTES
    {
        return Err(IndexError::CorruptObject(
            "Legacy RepoMap contribution differs".into(),
        ));
    }
    let _contribution_memory = context.budget.reserve_memory(
        u64::try_from(
            stored
                .payload
                .len()
                .checked_mul(8)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let contribution: quanta_index_contract::index::ProjectionOwnerContribution =
        serde_json::from_slice(&stored.payload).map_err(|_| {
            IndexError::CorruptObject("Legacy RepoMap contribution decode refused".into())
        })?;
    if stored.registration.children != contribution.object_children()
        || serde_json::to_vec(&contribution).ok().as_deref() != Some(stored.payload.as_slice())
        || key.as_slice() != owner_map_key(contribution.owner).as_slice()
        || contribution.projection != projection.projection
        || contribution.producer != projection.producer
    {
        return Err(IndexError::DigestConflict);
    }
    contribution
        .output
        .ok_or_else(|| IndexError::InvalidInput("Legacy RepoMap contribution has no import".into()))
}

pub(super) fn digest_key<T: serde::Serialize>(
    domain: u8,
    value: &T,
) -> Result<Vec<u8>, IndexError> {
    let key_bytes = serde_json::to_vec(value)
        .map_err(|_| IndexError::InvalidInput("Legacy RepoMap key encode refused".into()))?;
    let mut hash = Sha256::new();
    hash.update(INDEX_DOMAIN);
    hash.update([domain]);
    hash.update(&key_bytes);
    Ok(hash.finalize().to_vec())
}

pub(super) fn row_digest_key(
    row: &CompiledPayloadEntry,
    start: usize,
) -> Result<(usize, Vec<u8>), IndexError> {
    match row {
        CompiledPayloadEntry::Node(node) => Ok((0, digest_key(0, &node.identity)?)),
        CompiledPayloadEntry::Edge(edge) => {
            let mut key = digest_key(3, &edge.source)?;
            key.extend(
                u64::try_from(start)
                    .map_err(|_| IndexError::ResourceLimited)?
                    .to_be_bytes(),
            );
            Ok((1, key))
        }
        CompiledPayloadEntry::Projection(row) => Ok((2, digest_key(2, &row.subject)?)),
    }
}

struct MapWindows {
    roots: [MapRoot; 4],
    edits: [Vec<MapEdit>; 4],
}

impl MapWindows {
    fn new() -> Result<Self, IndexError> {
        let edits = std::array::from_fn(|_| Vec::with_capacity(WINDOW));
        if edits.iter().any(|column| column.capacity() > WINDOW) {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            roots: [MapRoot(None); 4],
            edits,
        })
    }

    fn push(
        &mut self,
        column: usize,
        key: Vec<u8>,
        value: MapValue,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        if self.edits[column].len() == WINDOW {
            self.flush(column, context)?;
        }
        if self.edits[column].iter().any(|edit| edit.key == key)
            || context
                .maps
                .lookup(self.roots[column], &key, context.budget)?
                .is_some()
        {
            // Includes a distinct full-key SHA collision. A digest never
            // silently overwrites or authorizes the other old row.
            return Err(IndexError::DigestConflict);
        }
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(key.len()).map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.edits[column].push(MapEdit {
            key,
            expected: None,
            next: Some(value),
        });
        Ok(())
    }

    fn flush(&mut self, column: usize, context: &mut AttemptContext<'_>) -> Result<(), IndexError> {
        if self.edits[column].is_empty() {
            return Ok(());
        }
        self.edits[column].sort_unstable_by(|left, right| left.key.cmp(&right.key));
        self.roots[column] = context.apply(self.roots[column], &self.edits[column])?;
        self.edits[column].clear();
        Ok(())
    }

    fn flush_all(&mut self, context: &mut AttemptContext<'_>) -> Result<(), IndexError> {
        for column in 0..self.edits.len() {
            self.flush(column, context)?;
        }
        Ok(())
    }
}

fn canonical_json_len(value: &impl serde::Serialize) -> Result<usize, IndexError> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("RepoMap JSON length overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Counter(0);
    serde_json::to_writer(&mut count, value).map_err(|_| IndexError::ResourceLimited)?;
    Ok(count.0)
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::{CompiledRepoMapNodeV1, FileId, RepoMapNodeRef};

    use crate::materializer::CompiledPayloadEntry;

    use super::row_digest_key;

    #[test]
    fn long_original_identity_uses_bounded_map_key_without_aliasing() {
        let first = RepoMapNodeRef::File(FileId::new(format!("src/{}.rs", "a".repeat(5000))));
        let second = RepoMapNodeRef::File(FileId::new(format!("src/{}.rs", "b".repeat(5000))));
        let (first_column, first_key) = row_digest_key(
            &CompiledPayloadEntry::Node(CompiledRepoMapNodeV1 {
                identity: first,
                degree_in: 0,
                degree_out: 0,
            }),
            64,
        )
        .expect("first exact identity");
        let (second_column, second_key) = row_digest_key(
            &CompiledPayloadEntry::Node(CompiledRepoMapNodeV1 {
                identity: second,
                degree_in: 0,
                degree_out: 0,
            }),
            128,
        )
        .expect("second exact identity");
        assert_eq!(first_column, 0);
        assert_eq!(second_column, 0);
        assert_eq!(first_key.len(), 32);
        assert_eq!(second_key.len(), 32);
        assert_ne!(first_key, second_key);
    }
}

#[cfg(test)]
#[path = "oracle_tests.rs"]
mod oracle_tests;
