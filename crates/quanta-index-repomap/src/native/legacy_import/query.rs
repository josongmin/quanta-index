//! Exact-row Native reads over the imported compiler payload and its three
//! persistent digest indexes. Hash keys select candidates; original typed
//! identities always decide equality.

use quanta_index_contract::{
    CompiledRepoMapEdgeV1, CompiledRepoMapNodeV1, CompiledRepoMapProjectionEntryV1, RepoMapNodeRef,
    index::{MapRoot, MemberBinding, SourceCut},
};
use quanta_index_core::index::{
    IndexError, MapValue, MemoryLease, MemoryLifetime, WorkBudget,
    ports::{ImmutableObjectPort, PersistentMapPort},
};

use crate::materializer::{CompiledPayloadEntry, decode_compiled_payload_span_v1};

use super::build::{LegacyImportRootV1, digest_key, row_digest_key};
use super::{VerifiedLegacyRepoMapImportV1, load_verified_legacy_repomap_member_with_get_v1};

pub struct NativeLegacyRepoMapReadV1 {
    root: LegacyImportRootV1,
    import: VerifiedLegacyRepoMapImportV1,
}

/// Decoded row and the retained Original grant that admitted its owned fields.
pub struct LoadedLegacyRepoMapRowV1<T> {
    pub value: T,
    pub(super) _memory: Option<MemoryLease>,
}

impl NativeLegacyRepoMapReadV1 {
    pub fn payload_sha256(&self) -> quanta_index_contract::index::Digest {
        self.import.header.payload_sha256
    }

    pub fn open(
        member: &MemberBinding,
        expected_source: SourceCut,
        objects: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let (root, import) = load_verified_legacy_repomap_member_with_get_v1(
            member,
            Some(expected_source),
            &mut |reference, budget| objects.get(reference, budget),
            budget,
        )?;
        Ok(Self { root, import })
    }

    pub fn lookup_node(
        &self,
        identity: &RepoMapNodeRef,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
    ) -> Result<Option<LoadedLegacyRepoMapRowV1<CompiledRepoMapNodeV1>>, IndexError> {
        let key = digest_key(0, identity)?;
        match self.lookup(self.root.nodes, 0, &key, maps, budget)? {
            Some(LoadedLegacyRepoMapRowV1 {
                value: CompiledPayloadEntry::Node(node),
                _memory,
            }) if &node.identity == identity => Ok(Some(LoadedLegacyRepoMapRowV1 {
                value: node,
                _memory,
            })),
            Some(_) => Err(IndexError::DigestConflict),
            None => Ok(None),
        }
    }

    pub fn lookup_rank_row(
        &self,
        subject: &RepoMapNodeRef,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
    ) -> Result<Option<LoadedLegacyRepoMapRowV1<CompiledRepoMapProjectionEntryV1>>, IndexError>
    {
        let key = digest_key(2, subject)?;
        match self.lookup(self.root.rank_rows, 2, &key, maps, budget)? {
            Some(LoadedLegacyRepoMapRowV1 {
                value: CompiledPayloadEntry::Projection(row),
                _memory,
            }) if &row.subject == subject => Ok(Some(LoadedLegacyRepoMapRowV1 {
                value: row,
                _memory,
            })),
            Some(_) => Err(IndexError::DigestConflict),
            None => Ok(None),
        }
    }

    pub fn visit_outgoing_edges(
        &self,
        source: &RepoMapNodeRef,
        limit: u32,
        after_start: Option<u64>,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
        visit: impl FnMut(
            u64,
            LoadedLegacyRepoMapRowV1<CompiledRepoMapEdgeV1>,
            &mut WorkBudget,
        ) -> Result<(), IndexError>,
    ) -> Result<Option<u64>, IndexError> {
        self.visit_edges(
            self.root.edges,
            digest_key(3, source)?,
            source,
            false,
            limit,
            after_start,
            maps,
            budget,
            visit,
        )
    }

    pub fn visit_incoming_edges(
        &self,
        target: &RepoMapNodeRef,
        limit: u32,
        after_start: Option<u64>,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
        visit: impl FnMut(
            u64,
            LoadedLegacyRepoMapRowV1<CompiledRepoMapEdgeV1>,
            &mut WorkBudget,
        ) -> Result<(), IndexError>,
    ) -> Result<Option<u64>, IndexError> {
        self.visit_edges(
            self.root.incoming,
            digest_key(4, target)?,
            target,
            true,
            limit,
            after_start,
            maps,
            budget,
            visit,
        )
    }

    fn visit_edges(
        &self,
        root: MapRoot,
        prefix: Vec<u8>,
        endpoint: &RepoMapNodeRef,
        incoming: bool,
        limit: u32,
        after_start: Option<u64>,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
        mut visit: impl FnMut(
            u64,
            LoadedLegacyRepoMapRowV1<CompiledRepoMapEdgeV1>,
            &mut WorkBudget,
        ) -> Result<(), IndexError>,
    ) -> Result<Option<u64>, IndexError> {
        if limit == 0 || limit > quanta_index_contract::native_protocol::MAX_NATIVE_GRAPH_EDGES {
            return Err(IndexError::InvalidInput(
                "Legacy RepoMap edge page limit differs".into(),
            ));
        }
        let _cursor_memory = budget.reserve_memory(128, MemoryLifetime::Temporary)?;
        let mut after = if let Some(start) = after_start {
            let mut cursor = prefix.clone();
            cursor.extend(start.to_be_bytes());
            let value = maps
                .lookup(root, &cursor, budget)?
                .ok_or(IndexError::SnapshotUnavailable)?;
            let LoadedLegacyRepoMapRowV1 {
                value: CompiledPayloadEntry::Edge(edge),
                ..
            } = self.decode(if incoming { 3 } else { 1 }, &cursor, value, budget)?
            else {
                return Err(IndexError::DigestConflict);
            };
            if (incoming && &edge.target != endpoint) || (!incoming && &edge.source != endpoint) {
                return Err(IndexError::DigestConflict);
            }
            Some(cursor)
        } else {
            Some(prefix.clone())
        };
        let mut delivered = 0_u32;
        let mut last_delivered_start = None;
        loop {
            budget.checkpoint()?;
            let page = maps.scan(root, after.as_deref(), 128, budget)?;
            if page.entries.is_empty() && page.has_more {
                return Err(IndexError::BackendViolation(
                    "Legacy RepoMap edge scan stalled".into(),
                ));
            }
            for (key, value) in &page.entries {
                if !key.starts_with(&prefix) {
                    return Ok(None);
                }
                let start = u64::from_be_bytes(
                    key.get(32..40)
                        .ok_or(IndexError::DigestConflict)?
                        .try_into()
                        .map_err(|_| IndexError::DigestConflict)?,
                );
                let column = if incoming { 3 } else { 1 };
                let LoadedLegacyRepoMapRowV1 {
                    value: CompiledPayloadEntry::Edge(edge),
                    _memory,
                } = self.decode(column, key, *value, budget)?
                else {
                    return Err(IndexError::CorruptObject(
                        "Legacy RepoMap edge row type differs".into(),
                    ));
                };
                if (incoming && &edge.target != endpoint) || (!incoming && &edge.source != endpoint)
                {
                    return Err(IndexError::DigestConflict);
                }
                if delivered == limit {
                    return last_delivered_start
                        .ok_or(IndexError::DigestConflict)
                        .map(Some);
                }
                visit(
                    start,
                    LoadedLegacyRepoMapRowV1 {
                        value: edge,
                        _memory,
                    },
                    budget,
                )?;
                delivered = delivered
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                last_delivered_start = Some(start);
            }
            if !page.has_more {
                return Ok(None);
            }
            after = Some(
                page.entries
                    .last()
                    .ok_or_else(|| {
                        IndexError::BackendViolation("Legacy RepoMap edge page empty".into())
                    })?
                    .0
                    .clone(),
            );
        }
    }

    pub fn visit_rank_rows(
        &self,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
        mut visit: impl FnMut(
            LoadedLegacyRepoMapRowV1<CompiledRepoMapProjectionEntryV1>,
            &mut WorkBudget,
        ) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        let _cursor_memory = budget.reserve_memory(64, MemoryLifetime::Temporary)?;
        let mut after: Option<Vec<u8>> = None;
        loop {
            budget.checkpoint()?;
            let page = maps.scan(self.root.rank_rows, after.as_deref(), 128, budget)?;
            if page.entries.is_empty() && page.has_more {
                return Err(IndexError::BackendViolation(
                    "Legacy RepoMap rank scan stalled".into(),
                ));
            }
            for (key, value) in &page.entries {
                let LoadedLegacyRepoMapRowV1 {
                    value: CompiledPayloadEntry::Projection(row),
                    _memory,
                } = self.decode(2, key, *value, budget)?
                else {
                    return Err(IndexError::CorruptObject(
                        "Legacy RepoMap rank row type differs".into(),
                    ));
                };
                visit(
                    LoadedLegacyRepoMapRowV1 {
                        value: row,
                        _memory,
                    },
                    budget,
                )?;
            }
            if !page.has_more {
                break;
            }
            after = Some(
                page.entries
                    .last()
                    .ok_or_else(|| {
                        IndexError::BackendViolation("Legacy RepoMap rank page empty".into())
                    })?
                    .0
                    .clone(),
            );
        }
        budget.checkpoint()
    }

    fn lookup(
        &self,
        root: MapRoot,
        column: usize,
        key: &[u8],
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
    ) -> Result<Option<LoadedLegacyRepoMapRowV1<CompiledPayloadEntry>>, IndexError> {
        maps.lookup(root, key, budget)?
            .map(|value| self.decode(column, key, value, budget))
            .transpose()
    }

    fn decode(
        &self,
        column: usize,
        key: &[u8],
        value: MapValue,
        budget: &mut WorkBudget,
    ) -> Result<LoadedLegacyRepoMapRowV1<CompiledPayloadEntry>, IndexError> {
        let MapValue::Inline(inline) = value else {
            return Err(IndexError::CorruptObject(
                "Legacy RepoMap map value is not a span".into(),
            ));
        };
        if !inline.children()?.is_empty() || inline.data()?.len() != 16 {
            return Err(IndexError::CorruptObject(
                "Legacy RepoMap span shape differs".into(),
            ));
        }
        let data = inline.data()?;
        let start = usize::try_from(u64::from_be_bytes(
            data[..8]
                .try_into()
                .map_err(|_| IndexError::ResourceLimited)?,
        ))
        .map_err(|_| IndexError::ResourceLimited)?;
        let end = usize::try_from(u64::from_be_bytes(
            data[8..]
                .try_into()
                .map_err(|_| IndexError::ResourceLimited)?,
        ))
        .map_err(|_| IndexError::ResourceLimited)?;
        if start >= end || end > self.import.payload().len() {
            return Err(IndexError::CorruptObject(
                "Legacy RepoMap span bounds differ".into(),
            ));
        }
        let allowance = u64::try_from(end - start)
            .map_err(|_| IndexError::ResourceLimited)?
            .checked_mul(8)
            .and_then(|bytes| bytes.checked_add(4096))
            .ok_or(IndexError::ResourceLimited)?;
        let row_memory = budget.reserve_memory(allowance, MemoryLifetime::Retained)?;
        let payload_column = if column == 3 { 1 } else { column };
        let row =
            decode_compiled_payload_span_v1(self.import.payload(), payload_column, start, end)
                .map_err(|error| IndexError::CorruptObject(error.to_string()))?;
        let (actual_column, actual_key) = if column == 3 {
            let CompiledPayloadEntry::Edge(edge) = &row else {
                return Err(IndexError::CorruptObject(
                    "Legacy RepoMap incoming row type differs".into(),
                ));
            };
            let mut key = digest_key(4, &edge.target)?;
            key.extend(
                u64::try_from(start)
                    .map_err(|_| IndexError::ResourceLimited)?
                    .to_be_bytes(),
            );
            (3, key)
        } else {
            row_digest_key(&row, start)?
        };
        if actual_column != column || actual_key.as_slice() != key {
            return Err(IndexError::DigestConflict);
        }
        Ok(LoadedLegacyRepoMapRowV1 {
            value: row,
            _memory: row_memory,
        })
    }
}
