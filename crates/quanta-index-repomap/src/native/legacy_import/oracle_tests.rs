use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use quanta_index_contract::{
    CandidateCommitmentV1, CompiledRepoMapEdgeV1, CompiledRepoMapGraphV1, CompiledRepoMapNodeV1,
    CompiledRepoMapProjectionEntryV1, FileId, ManifestGeneration, RepoId, RepoMapDocType,
    RepoMapEdgeKind, RepoMapNodeRef, RepoRelativePath, RevisionId,
    index::{
        AttemptId, AttemptRef, CollectionId, Digest, GroupActivation, IndexInstanceId, InputDomain,
        LegacyRepoMapImportPageV1, LegacyRepoMapImportV1, MapRoot, ObjectRef, OperationKey,
        OwnerKey, ProducerId, ProjectionBinding, ProjectionId, ProjectionOwnerContribution,
        PublicationGroupId, SlotId, SourceCut, StableOwnerId, StreamId, TenantId, WorktreeId,
        native_immutable_object_ref_v1,
    },
    native_protocol::native_repomap_rank_policy,
};
use quanta_index_core::index::{
    AttemptContext, IndexError, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapValue,
    MapWrite, ObjectBatchInput, RegisteredObject, RegisteredObjectBatch, ScratchAllocation,
    ScratchRequest, ScratchSpacePort, StoredObject, WorkBudget, WorkLimits,
    ports::{
        ImmutableObjectPort, MapEditCursor, ObjectCustodyPort, ObjectWriteSink, PersistentMapPort,
    },
};
use sha2::{Digest as _, Sha256};

use super::*;
use crate::materializer::{
    CompiledPayloadEntry, encode_compiled_payload, visit_compiled_payload_with_span,
};
use crate::native::legacy_import::{
    NativeLegacyRepoMapReadV1, with_verified_native_legacy_repomap_member_v1,
};

#[derive(Default)]
struct Objects(Mutex<BTreeMap<ObjectRef, StoredObject>>);

impl Objects {
    fn store(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
    ) -> Result<RegisteredObject, IndexError> {
        let reference =
            native_immutable_object_ref_v1(payload, children).ok_or(IndexError::ResourceLimited)?;
        let registration = RegisteredObject::unscoped(
            reference,
            u64::try_from(payload.len() + children.len() * 32 + 16)
                .map_err(|_| IndexError::ResourceLimited)?,
            children.to_vec(),
        );
        let _old = self.0.lock().map_err(|_| IndexError::Conflict)?.insert(
            reference,
            StoredObject::new(registration.clone(), payload.to_vec(), None),
        );
        Ok(registration)
    }
}

impl ImmutableObjectPort for Objects {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        _: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        let reference =
            native_immutable_object_ref_v1(payload, children).ok_or(IndexError::ResourceLimited)?;
        Ok(RegisteredObject::unscoped(
            reference,
            u64::try_from(payload.len() + children.len() * 32 + 16)
                .map_err(|_| IndexError::ResourceLimited)?,
            children.to_vec(),
        ))
    }
    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        _: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.store(payload, children)
    }
    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        let registration = self.identify(payload, children, budget)?;
        sink.reserve(&registration)?;
        let stored = self.store(payload, children)?;
        sink.installed(&stored)?;
        Ok(stored)
    }
    fn install_batch(
        &self,
        _: &[ObjectBatchInput],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected object batch".into(),
        ))
    }
    fn get(&self, reference: ObjectRef, _: &mut WorkBudget) -> Result<StoredObject, IndexError> {
        self.0
            .lock()
            .map_err(|_| IndexError::Conflict)?
            .get(&reference)
            .cloned()
            .ok_or(IndexError::SnapshotUnavailable)
    }
}

#[derive(Default)]
struct Maps {
    next: AtomicU64,
    roots: Mutex<BTreeMap<ObjectRef, BTreeMap<Vec<u8>, MapValue>>>,
}

impl Maps {
    fn seed(&self, rows: BTreeMap<Vec<u8>, MapValue>) -> Result<MapRoot, IndexError> {
        let next = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let mut hash = Sha256::new();
        hash.update(b"repomap-import-oracle-map\0");
        hash.update(next.to_be_bytes());
        let reference = ObjectRef(Digest(hash.finalize().into()));
        let _old = self
            .roots
            .lock()
            .map_err(|_| IndexError::Conflict)?
            .insert(reference, rows);
        Ok(MapRoot(Some(reference)))
    }
    fn rows(&self, root: MapRoot) -> Result<BTreeMap<Vec<u8>, MapValue>, IndexError> {
        match root.0 {
            Some(reference) => self
                .roots
                .lock()
                .map_err(|_| IndexError::Conflict)?
                .get(&reference)
                .cloned()
                .ok_or(IndexError::SnapshotUnavailable),
            None => Ok(BTreeMap::new()),
        }
    }
    fn tamper(&self, root: MapRoot, key: Vec<u8>, value: MapValue) -> Result<(), IndexError> {
        let reference = root.0.ok_or(IndexError::SnapshotUnavailable)?;
        let mut roots = self.roots.lock().map_err(|_| IndexError::Conflict)?;
        let rows = roots
            .get_mut(&reference)
            .ok_or(IndexError::SnapshotUnavailable)?;
        let _old = rows.insert(key, value);
        Ok(())
    }
}

impl PersistentMapPort for Maps {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        _: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        let Some(reference) = root.0 else {
            return Ok(None);
        };
        let roots = self.roots.lock().map_err(|_| IndexError::Conflict)?;
        let rows = roots
            .get(&reference)
            .ok_or(IndexError::SnapshotUnavailable)?;
        Ok(rows.get(key).copied())
    }
    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        _: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        let rows = self.rows(root)?;
        let mut selected = rows
            .into_iter()
            .filter(|(key, _)| after.is_none_or(|after| key.as_slice() > after))
            .take(usize::try_from(limit).map_err(|_| IndexError::ResourceLimited)? + 1)
            .collect::<Vec<_>>();
        let has_more =
            selected.len() > usize::try_from(limit).map_err(|_| IndexError::ResourceLimited)?;
        if has_more {
            let _extra = selected.pop();
        }
        Ok(MapPage::new(selected, has_more, Vec::new(), None))
    }
    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        let mut rows = self.rows(base)?;
        for edit in edits {
            if rows.get(&edit.key).copied() != edit.expected {
                return Err(IndexError::Conflict);
            }
            if let Some(next) = edit.next {
                let _old = rows.insert(edit.key.clone(), next);
            } else {
                let _removed = rows.remove(&edit.key);
            }
        }
        Ok(MapWrite {
            root: self.seed(rows)?,
        })
    }
    fn apply_batch(
        &self,
        _: &[MapBatchRequest],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        Err(IndexError::BackendViolation("unexpected map batch".into()))
    }
    fn apply_stream(
        &self,
        _: &[MapRoot],
        _: &mut dyn MapEditCursor,
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
        Err(IndexError::BackendViolation("unexpected map stream".into()))
    }
}

struct Custody;
impl ObjectCustodyPort for Custody {
    fn reserve_objects(&self, _: AttemptRef, _: &[RegisteredObject]) -> Result<(), IndexError> {
        Ok(())
    }
    fn register_objects(&self, _: AttemptRef, _: &[RegisteredObject]) -> Result<(), IndexError> {
        Ok(())
    }
}
struct Scratch;
impl ScratchSpacePort for Scratch {
    fn allocate(
        &self,
        _: ScratchRequest,
        _: &mut WorkBudget,
    ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
        Err(IndexError::BackendViolation("unexpected scratch".into()))
    }
}

fn budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 1 << 28,
            mutations: 1_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn source() -> SourceCut {
    SourceCut {
        owners: MapRoot(None),
        paths: MapRoot(None),
        frontiers: MapRoot(None),
    }
}

fn node(number: usize) -> RepoMapNodeRef {
    RepoMapNodeRef::File(FileId::new(format!("src/{number:04}.rs")))
}

#[test]
fn imported_compiler_graph_builds_exact_maps_and_reopens_paginated_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let objects = Objects::default();
    let maps = Maps::default();
    let custody = Custody;
    let scratch = Scratch;
    let mut work = budget()?;
    let root_node = node(0);
    let mut nodes = Vec::new();
    nodes.push(CompiledRepoMapNodeV1 {
        identity: root_node.clone(),
        degree_in: 0,
        degree_out: 1025,
    });
    let mut edges = Vec::new();
    for ordinal in 1..=1025 {
        let target = node(ordinal);
        nodes.push(CompiledRepoMapNodeV1 {
            identity: target.clone(),
            degree_in: 1,
            degree_out: 0,
        });
        edges.push(CompiledRepoMapEdgeV1 {
            kind: RepoMapEdgeKind::Contains,
            source: root_node.clone(),
            target,
        });
    }
    let graph = CompiledRepoMapGraphV1 { nodes, edges };
    let projection_rows = vec![CompiledRepoMapProjectionEntryV1 {
        subject: root_node.clone(),
        doc_type: RepoMapDocType::File,
        owner_path: RepoRelativePath::new("src/0000.rs"),
        search_text: "fixed oracle needle".into(),
        final_score_millis: 500,
    }];
    let payload = encode_compiled_payload(&graph, &projection_rows);
    crate::materializer::validate_compiled_payload(&payload)?;
    let page = objects.store(&payload, &[])?;
    let header = LegacyRepoMapImportV1 {
        wire_version: 1,
        repo_id: RepoId::new("repo")?,
        revision_id: RevisionId::new("rev")?,
        generation: ManifestGeneration::new(1),
        old_active: quanta_index_contract::RepoMapExpectedActiveV2::new(
            std::num::NonZeroU64::new(1).ok_or("epoch")?,
            CandidateCommitmentV1::from_bytes([9; 32]),
        ),
        payload_bytes: u64::try_from(payload.len())?,
        payload_sha256: Digest(Sha256::digest(&payload).into()),
        pages: vec![LegacyRepoMapImportPageV1 {
            object: page.reference,
            bytes: u32::try_from(payload.len())?,
        }],
    };
    header.verify_children_v1(&[page.reference])?;
    let header_bytes = serde_json::to_vec(&header)?;
    let header_ref = objects.store(&header_bytes, &[page.reference])?.reference;
    let owner = OwnerKey {
        producer: ProducerId([5; 16]),
        stable_id: StableOwnerId([6; 16]),
    };
    let contribution = ProjectionOwnerContribution {
        projection: ProjectionId([7; 16]),
        producer: owner.producer,
        owner,
        source_owner: None,
        source: None,
        dependency_stamp: Digest([8; 32]),
        output: Some(header_ref),
    };
    let contribution_ref = objects
        .store(&serde_json::to_vec(&contribution)?, &[header_ref])?
        .reference;
    let owners = maps.seed(BTreeMap::from([(
        owner_map_key(owner).to_vec(),
        MapValue::Object(contribution_ref),
    )]))?;
    let certificate = objects.store(b"fixed certificate", &[])?.reference;
    let projection = ProjectionBinding {
        projection: contribution.projection,
        producer: contribution.producer,
        owners,
        source: source(),
        schema: NATIVE_LEGACY_REPOMAP_SCHEMA_V1,
        producer_policy: native_repomap_rank_policy(),
        dependency_context: Digest([10; 32]),
        certificate,
    };
    let instance = quanta_index_contract::index::InstanceSpec {
        instance: IndexInstanceId([11; 16]),
        kind: NATIVE_LEGACY_REPOMAP_KIND_V1,
        group: PublicationGroupId([12; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::Projection {
            projection: contribution.projection,
            producer: contribution.producer,
        },
        schema: NATIVE_LEGACY_REPOMAP_SCHEMA_V1,
        policy: native_repomap_rank_policy(),
        format: NATIVE_LEGACY_REPOMAP_FORMAT_V1,
    };
    let attempt = AttemptRef {
        operation: OperationKey {
            workspace: quanta_index_contract::index::WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            producer: owner.producer,
            stream: StreamId([13; 16]),
            stream_epoch: 1,
            sequence: 1,
        },
        attempt: AttemptId([14; 16]),
        fence: 1,
    };
    let demand = ResourceNeed {
        cpu_units: 1,
        io_bytes: 1 << 25,
        memory_bytes: 1 << 25,
        provider_units: 0,
    };
    let required_read_credit = minimum_query_open_credit_v1(header.payload_bytes)?;
    let undersized = NativeLegacyRepoMapBuildV1::new(demand, required_read_credit - 1)?;
    assert!(matches!(
        undersized.prepare_full(
            &instance,
            projection,
            &mut AttemptContext {
                attempt,
                objects: &objects,
                maps: &maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            },
        ),
        Err(IndexError::ResourceLimited)
    ));
    let builder = NativeLegacyRepoMapBuildV1::new(demand, required_read_credit)?;
    let candidate = builder.prepare_full(
        &instance,
        projection,
        &mut AttemptContext {
            attempt,
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        },
    )?;
    let member = candidate.binding;
    with_verified_native_legacy_repomap_member_v1(
        &member,
        &instance,
        source(),
        |reference, budget| objects.get(reference, budget),
        &mut work,
        |observed, _| {
            assert_eq!(observed, &header);
            Ok(())
        },
    )?;
    let read = NativeLegacyRepoMapReadV1::open(&member, source(), &objects, &mut work)?;
    assert_eq!(
        read.lookup_node(&root_node, &maps, &mut work)?
            .ok_or("root node")?
            .value
            .degree_out,
        1025
    );
    let mut first = Vec::new();
    let next =
        read.visit_outgoing_edges(&root_node, 1024, None, &maps, &mut work, |start, row, _| {
            first.push((start, row.value.target));
            Ok(())
        })?;
    assert_eq!(first.len(), 1024);
    let mut second = Vec::new();
    let last =
        read.visit_outgoing_edges(&root_node, 1024, next, &maps, &mut work, |start, row, _| {
            second.push((start, row.value.target));
            Ok(())
        })?;
    assert!(last.is_none());
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].1, node(1025));
    assert!(first.last().ok_or("first page")?.0 < second[0].0);
    let rank = read
        .lookup_rank_row(&root_node, &maps, &mut work)?
        .ok_or("rank row")?;
    assert_eq!(rank.value.search_text, "fixed oracle needle");
    let mut wrong = source();
    wrong.owners = MapRoot(Some(page.reference));
    assert!(
        with_verified_native_legacy_repomap_member_v1(
            &member,
            &instance,
            wrong,
            |reference, budget| objects.get(reference, budget),
            &mut work,
            |_, _| Ok(())
        )
        .is_err()
    );
    assert!(
        with_verified_native_legacy_repomap_member_v1(
            &member,
            &instance,
            source(),
            |reference, budget| {
                let stored = objects.get(reference, budget)?;
                if reference == header_ref {
                    Ok(StoredObject::new(
                        stored.registration.clone(),
                        b"{}".to_vec(),
                        None,
                    ))
                } else {
                    Ok(stored)
                }
            },
            &mut work,
            |_, _| Ok(())
        )
        .is_err()
    );
    assert!(
        with_verified_native_legacy_repomap_member_v1(
            &member,
            &instance,
            source(),
            |reference, budget| {
                let stored = objects.get(reference, budget)?;
                if reference == page.reference {
                    let mut bytes = stored.payload.to_vec();
                    bytes[0] ^= 1;
                    Ok(StoredObject::new(stored.registration.clone(), bytes, None))
                } else {
                    Ok(stored)
                }
            },
            &mut work,
            |_, _| Ok(())
        )
        .is_err()
    );
    let member_root: LegacyImportRootV1 =
        serde_json::from_slice(&objects.get(member.artifact, &mut work)?.payload)?;
    let mut second_span = None;
    visit_compiled_payload_with_span(
        &payload,
        |error| error,
        |row, span| {
            if matches!(row, CompiledPayloadEntry::Node(ref row_node) if row_node.identity == node(1))
            {
                second_span = Some(span);
            }
            Ok::<_, quanta_index_core::CoreError>(())
        },
    )?;
    let span = second_span.ok_or("second span")?;
    let mut inline = [0_u8; 16];
    inline[..8].copy_from_slice(&u64::try_from(span.start)?.to_be_bytes());
    inline[8..].copy_from_slice(&u64::try_from(span.end)?.to_be_bytes());
    maps.tamper(
        member_root.nodes,
        digest_key(0, &root_node)?,
        MapValue::inline(&inline, [])?,
    )?;
    assert!(matches!(
        read.lookup_node(&root_node, &maps, &mut work),
        Err(IndexError::DigestConflict)
    ));
    Ok(())
}
