use quanta_index_contract::{
    FileId, RepoMapEdgeKind, RepoMapNodeRef,
    index::{Digest, OwnerKey, ProducerId, StableOwnerId},
};

use super::affected::{edge_prefix, node_key, node_prefix};
use super::contribution::validate_output;
use super::{GraphEdge, GraphNode, GraphOwnerOutput};

fn file(value: &str) -> RepoMapNodeRef {
    RepoMapNodeRef::File(FileId::new(value))
}

fn owner(value: u8) -> OwnerKey {
    OwnerKey {
        producer: ProducerId([1; 16]),
        stable_id: StableOwnerId([value; 16]),
    }
}

#[test]
fn certified_output_requires_unique_canonical_members() {
    let node = GraphNode {
        identity: file("src/a.rs"),
        payload_digest: Digest([1; 32]),
    };
    let edge = GraphEdge {
        kind: RepoMapEdgeKind::DependsOn,
        source: file("src/a.rs"),
        target: file("src/b.rs"),
    };
    assert!(
        validate_output(&GraphOwnerOutput {
            projection: Vec::new(),
            nodes: vec![node.clone()],
            edges: vec![edge.clone()]
        })
        .is_ok()
    );
    assert!(
        validate_output(&GraphOwnerOutput {
            projection: Vec::new(),
            nodes: vec![node.clone(), node],
            edges: vec![]
        })
        .is_err()
    );
    assert!(
        validate_output(&GraphOwnerOutput {
            projection: Vec::new(),
            nodes: vec![],
            edges: vec![edge.clone(), edge]
        })
        .is_err()
    );
}

#[test]
fn supporter_keys_are_owner_distinct_under_one_exact_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let identity = file("src/a.rs");
    let first = node_key(&identity, owner(1))?;
    let second = node_key(&identity, owner(2))?;
    if first == second
        || !first.starts_with(&node_prefix(&identity)?)
        || !second.starts_with(&node_prefix(&identity)?)
        || first.starts_with(&node_prefix(&file("src/ab.rs"))?)
        || edge_prefix(&identity)? == edge_prefix(&file("src/ab.rs"))?
    {
        return Err("graph key identity or owner isolation failed".into());
    }
    Ok(())
}

#[test]
fn rank_policy_requires_a_projection_for_each_searchable_node()
-> Result<(), quanta_index_core::index::IndexError> {
    use super::contribution::validate_rank_coverage;
    use quanta_index_contract::{
        index::ObjectRef,
        native_protocol::{GraphProjectionRow, native_repomap_rank_policy},
    };
    use quanta_index_core::index::IndexError;

    let identity = file("src/a.rs");
    let mut output = GraphOwnerOutput {
        nodes: vec![GraphNode {
            identity: identity.clone(),
            payload_digest: Digest([1; 32]),
        }],
        edges: Vec::new(),
        projection: Vec::new(),
    };
    if !matches!(
        validate_rank_coverage(&output, native_repomap_rank_policy()),
        Err(IndexError::InvalidInput(_))
    ) || validate_rank_coverage(&output, Digest([77; 32])).is_err()
    {
        return Err(IndexError::BackendViolation(
            "rank policy coverage did not distinguish adjacency-only output".into(),
        ));
    }
    output.projection.push(GraphProjectionRow {
        subject: identity,
        row: ObjectRef(Digest([2; 32])),
    });
    validate_rank_coverage(&output, native_repomap_rank_policy())
}

#[cfg(test)]
mod projection_retention {
    use std::{
        collections::BTreeMap,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    use quanta_index_contract::index::{
        AttemptId, AttemptRef, CollectionId, GroupActivation, IndexInstanceId, IndexKind,
        InputBinding, InputDomain, InstanceSpec, MapRoot, MemberBinding, ObjectRef, OperationKey,
        OwnerMutationSet, ProjectionBinding, ProjectionId, PublicationGroupId, SchemaId, SlotId,
        SourceCut, StorageFormatId, StreamId, TenantId, WorkspaceKey, WorktreeId,
    };
    use quanta_index_core::index::{
        AttemptContext, IndexError, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapWrite,
        MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
        ObjectBatchInput, OwnerMutationDigest, RegisteredObject, RegisteredObjectBatch,
        ScratchAllocation, ScratchRequest, ScratchSpacePort, StoredObject, WorkBudget, WorkLimits,
        ports::{
            ImmutableObjectPort, MapEditCursor, ObjectCustodyPort, ObjectWriteSink,
            PersistentMapPort,
        },
    };
    use sha2::{Digest as _, Sha256};

    use super::super::{
        GraphRoot, NativeGraph,
        contribution::{GraphElement, GraphSupport, load_support},
    };
    use super::Digest;

    fn reference(byte: u8) -> ObjectRef {
        ObjectRef(Digest([byte; 32]))
    }

    #[derive(Default)]
    struct Objects {
        values: Mutex<BTreeMap<ObjectRef, StoredObject>>,
        writes: AtomicUsize,
    }

    impl Objects {
        fn writes(&self) -> usize {
            self.writes.load(Ordering::Relaxed)
        }

        fn store(
            &self,
            payload: &[u8],
            children: &[ObjectRef],
        ) -> Result<RegisteredObject, IndexError> {
            let mut hash = Sha256::new();
            hash.update(b"graph-retention-test-object\0");
            hash.update(payload);
            for child in children {
                hash.update((child.0).0);
            }
            let reference = ObjectRef(Digest(hash.finalize().into()));
            let registration = RegisteredObject::unscoped(
                reference,
                u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
                children.to_vec(),
            );
            let _previous = self
                .values
                .lock()
                .map_err(|_error| IndexError::Storage("test lock".into()))?
                .insert(
                    reference,
                    StoredObject::new(registration.clone(), payload.to_vec(), None),
                );
            let _previous = self.writes.fetch_add(1, Ordering::Relaxed);
            Ok(registration)
        }
    }

    impl ImmutableObjectPort for Objects {
        fn identify(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected identify".into()))
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
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            let installed = self.store(payload, children)?;
            sink.reserve(&installed)?;
            sink.installed(&installed)?;
            Ok(installed)
        }
        fn install_batch(
            &self,
            _: &[ObjectBatchInput],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected batch install".into(),
            ))
        }
        fn get(
            &self,
            reference: ObjectRef,
            _: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            self.values
                .lock()
                .map_err(|_error| IndexError::Storage("test lock".into()))?
                .get(&reference)
                .cloned()
                .ok_or_else(|| IndexError::CorruptObject("test object absent".into()))
        }
    }

    struct EmptyMaps;
    impl PersistentMapPort for EmptyMaps {
        fn lookup_object(
            &self,
            _: MapRoot,
            _: &[u8],
            _: &mut WorkBudget,
        ) -> Result<Option<ObjectRef>, IndexError> {
            Err(IndexError::BackendViolation("unexpected map lookup".into()))
        }
        fn lookup(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
            self.lookup_object(root, key, budget)
                .map(|value| value.map(Into::into))
        }

        fn scan(
            &self,
            root: MapRoot,
            _: Option<&[u8]>,
            _: u32,
            _: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            if root.0.is_some() {
                return Err(IndexError::BackendViolation(
                    "unexpected nonempty map scan".into(),
                ));
            }
            Ok(MapPage::new(Vec::new(), false, Vec::new(), None))
        }
        fn apply(
            &self,
            _: MapRoot,
            _: &[MapEdit],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<MapWrite, IndexError> {
            Err(IndexError::BackendViolation("unexpected map apply".into()))
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
        fn register_objects(
            &self,
            _: AttemptRef,
            _: &[RegisteredObject],
        ) -> Result<(), IndexError> {
            Ok(())
        }
    }

    struct NoScratch;
    impl ScratchSpacePort for NoScratch {
        fn allocate(
            &self,
            _: ScratchRequest,
            _: &mut WorkBudget,
        ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected scratch allocation".into(),
            ))
        }
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1 << 20,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    struct DecodeLedger {
        live: AtomicU64,
        limit: u64,
    }

    impl MemoryLeaseOwner for DecodeLedger {
        fn release(&self, bytes: u64) {
            let previous = self.live.fetch_sub(bytes, Ordering::SeqCst);
            assert!(previous >= bytes, "graph decode lease released too much");
        }
    }

    struct DecodeScope(Arc<DecodeLedger>);

    impl MemoryScopePort for DecodeScope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime != MemoryLifetime::Temporary {
                return Err(IndexError::BackendViolation(
                    "graph decode requested retained memory".into(),
                ));
            }
            let _previous_live = self
                .0
                .live
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                    live.checked_add(bytes)
                        .filter(|total| *total <= self.0.limit)
                })
                .map_err(|_live| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    #[test]
    fn malformed_graph_output_classification_is_scope_independent() -> Result<(), IndexError> {
        use super::super::contribution::load_output;

        let objects = Objects::default();
        for scoped in [false, true] {
            let ledger = Arc::new(DecodeLedger {
                live: AtomicU64::new(0),
                limit: 1 << 20,
            });
            let mut work = budget()?;
            if scoped {
                work.bind_memory_scope(Some(MemoryScope::new(Arc::new(DecodeScope(Arc::clone(
                    &ledger,
                ))))));
            }
            for payload in [b"".as_slice(), b"{".as_slice()] {
                let reference = objects.store(payload, &[])?.reference;
                match load_output(reference, &objects, &mut work) {
                    Err(IndexError::CorruptObject(_)) => {}
                    Err(error) => {
                        return Err(IndexError::BackendViolation(format!(
                            "malformed output changed classification: scoped={scoped}, payload={payload:?}, error={error}",
                        )));
                    }
                    Ok(_loaded) => {
                        return Err(IndexError::BackendViolation(format!(
                            "malformed output was accepted: scoped={scoped}, payload={payload:?}",
                        )));
                    }
                }
                if ledger.live.load(Ordering::SeqCst) != 0 {
                    return Err(IndexError::BackendViolation(
                        "malformed output leaked its decode grant".into(),
                    ));
                }
            }

            // An empty graph is valid JSON, unlike an empty output body.
            let payload = br#"{"nodes":[],"edges":[],"projection":[]}"#;
            let reference = objects.store(payload, &[])?.reference;
            let loaded = load_output(reference, &objects, &mut work)?;
            if !loaded.output.nodes.is_empty()
                || !loaded.output.edges.is_empty()
                || !loaded.output.projection.is_empty()
                || (scoped && ledger.live.load(Ordering::SeqCst) == 0)
            {
                return Err(IndexError::BackendViolation(
                    "valid empty graph was rejected or lost decode custody".into(),
                ));
            }
            drop(loaded);
            if ledger.live.load(Ordering::SeqCst) != 0 {
                return Err(IndexError::BackendViolation(
                    "valid empty graph retained its decode grant".into(),
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn decoded_graph_support_keeps_its_temporary_grant_through_partial_move()
    -> Result<(), IndexError> {
        let objects = Objects::default();
        let support = GraphSupport {
            owner: super::owner(1),
            projection: None,
            element: GraphElement::Node(super::GraphNode {
                identity: super::file("src/long_graph_identity.rs"),
                payload_digest: Digest([8; 32]),
            }),
        };
        let payload = serde_json::to_vec(&support)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let support_ref = objects.store(&payload, &[])?.reference;
        let allowance = payload
            .len()
            .checked_mul(8)
            .and_then(|bytes| bytes.checked_add(512))
            .ok_or(IndexError::ResourceLimited)?;
        let grant = u64::try_from(allowance).map_err(|_error| IndexError::ResourceLimited)?;
        let ledger = Arc::new(DecodeLedger {
            live: AtomicU64::new(0),
            limit: grant,
        });
        let mut work = budget()?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(DecodeScope(Arc::clone(
            &ledger,
        ))))));
        {
            let loaded = load_support(support_ref, &objects, &mut work)?;
            if ledger.live.load(Ordering::SeqCst) != grant {
                return Err(IndexError::BackendViolation(
                    "decoded graph support lost its grant on return".into(),
                ));
            }
            let decoded = loaded.support;
            let GraphElement::Node(node) = decoded.element else {
                return Err(IndexError::BackendViolation(
                    "graph support lost node".into(),
                ));
            };
            if node.identity != super::file("src/long_graph_identity.rs")
                || ledger.live.load(Ordering::SeqCst) != grant
            {
                return Err(IndexError::BackendViolation(
                    "partially moved graph support lost its grant".into(),
                ));
            }
            drop(node);
        }
        if ledger.live.load(Ordering::SeqCst) != 0 {
            return Err(IndexError::BackendViolation(
                "graph support decode grant was not released".into(),
            ));
        }

        let non_leaf = objects.store(&payload, &[reference(99)])?.reference;
        if !matches!(
            load_support(non_leaf, &objects, &mut work),
            Err(IndexError::CorruptObject(_))
        ) || ledger.live.load(Ordering::SeqCst) != 0
        {
            return Err(IndexError::BackendViolation(
                "invalid graph support retained its decode grant".into(),
            ));
        }

        let denied = Arc::new(DecodeLedger {
            live: AtomicU64::new(0),
            limit: grant.saturating_sub(1),
        });
        let mut work = budget()?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(DecodeScope(Arc::clone(
            &denied,
        ))))));
        if !matches!(
            load_support(support_ref, &objects, &mut work),
            Err(IndexError::ResourceLimited)
        ) || denied.live.load(Ordering::SeqCst) != 0
        {
            return Err(IndexError::BackendViolation(
                "graph support decoded without required admission".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn rank_projection_children_are_exact_and_support_retains_the_row() -> Result<(), IndexError> {
        use super::super::contribution::{load_output, load_rank_projection};
        use quanta_index_contract::native_protocol::GraphProjectionRow;
        use quanta_index_contract::{
            CompiledRepoMapProjectionEntryV1, RepoMapDocType, RepoRelativePath,
        };

        let objects = Objects::default();
        let mut work = budget()?;
        let entry = CompiledRepoMapProjectionEntryV1 {
            subject: super::file("src/a.rs"),
            doc_type: RepoMapDocType::File,
            owner_path: RepoRelativePath::new("src/a.rs"),
            search_text: "café needle".to_string(),
            final_score_millis: 450,
        };
        let row_bytes = serde_json::to_vec(&entry)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let row_ref = objects.store(&row_bytes, &[])?.reference;
        let output = super::GraphOwnerOutput {
            nodes: vec![super::GraphNode {
                identity: entry.subject.clone(),
                payload_digest: Digest([7; 32]),
            }],
            edges: Vec::new(),
            projection: vec![GraphProjectionRow {
                subject: entry.subject.clone(),
                row: row_ref,
            }],
        };
        let output_bytes = serde_json::to_vec(&output)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let output_ref = objects.store(&output_bytes, &[row_ref])?.reference;
        if load_output(output_ref, &objects, &mut work)?.output != output
            || load_rank_projection(row_ref, &objects, &mut work)?.entry != entry
        {
            return Err(IndexError::BackendViolation(
                "rank projection roundtrip changed".into(),
            ));
        }
        let missing_child = objects.store(&output_bytes, &[])?.reference;
        if !matches!(
            load_output(missing_child, &objects, &mut work),
            Err(IndexError::CorruptObject(_))
        ) {
            return Err(IndexError::BackendViolation(
                "rank output accepted missing child".into(),
            ));
        }
        let node = output
            .nodes
            .first()
            .ok_or_else(|| IndexError::BackendViolation("missing test node".into()))?
            .clone();
        let support = GraphSupport {
            owner: super::owner(1),
            element: GraphElement::Node(node),
            projection: Some(row_ref),
        };
        let support_bytes = serde_json::to_vec(&support)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let support_ref = objects.store(&support_bytes, &[row_ref])?.reference;
        if load_support(support_ref, &objects, &mut work)?.support != support {
            return Err(IndexError::BackendViolation(
                "support lost its row reference".into(),
            ));
        }
        let missing_child = objects.store(&support_bytes, &[])?.reference;
        if !matches!(
            load_support(missing_child, &objects, &mut work),
            Err(IndexError::CorruptObject(_))
        ) {
            return Err(IndexError::BackendViolation(
                "support accepted missing row child".into(),
            ));
        }
        Ok(())
    }

    fn attempt() -> AttemptRef {
        AttemptRef {
            operation: OperationKey {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                epoch: 1,
                producer: super::ProducerId([5; 16]),
                stream: StreamId([6; 16]),
                stream_epoch: 1,
                sequence: 1,
            },
            attempt: AttemptId([7; 16]),
            fence: 1,
        }
    }

    fn instance() -> InstanceSpec {
        InstanceSpec {
            instance: IndexInstanceId([8; 16]),
            kind: IndexKind([9; 16]),
            group: PublicationGroupId([10; 16]),
            activation: GroupActivation::CoreRequired,
            input_domain: InputDomain::Projection {
                projection: ProjectionId([11; 16]),
                producer: super::ProducerId([5; 16]),
            },
            schema: SchemaId([12; 16]),
            policy: Digest([13; 32]),
            format: StorageFormatId([14; 16]),
        }
    }

    fn projection(source: SourceCut, certificate: ObjectRef) -> ProjectionBinding {
        ProjectionBinding {
            projection: ProjectionId([11; 16]),
            producer: super::ProducerId([5; 16]),
            owners: MapRoot(Some(reference(20))),
            source,
            schema: SchemaId([12; 16]),
            producer_policy: Digest([15; 32]),
            dependency_context: Digest([16; 32]),
            certificate,
        }
    }

    fn check_equal<T: PartialEq + std::fmt::Debug>(
        actual: &T,
        expected: &T,
        label: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if actual != expected {
            return Err(std::io::Error::other(format!(
                "{label}: actual {actual:?}, expected {expected:?}"
            ))
            .into());
        }
        Ok(())
    }

    #[test]
    fn unchanged_graph_refreshes_projection_children_without_rebuilding_maps()
    -> Result<(), Box<dyn std::error::Error>> {
        let objects = Objects::default();
        let maps = EmptyMaps;
        let custody = Custody;
        let scratch = NoScratch;
        let instance = instance();
        let old_source = SourceCut {
            owners: MapRoot(Some(reference(31))),
            paths: MapRoot(Some(reference(32))),
            frontiers: MapRoot(Some(reference(33))),
        };
        let new_source = SourceCut {
            owners: MapRoot(Some(reference(41))),
            paths: MapRoot(Some(reference(42))),
            frontiers: MapRoot(Some(reference(43))),
        };
        let old_projection = projection(old_source, reference(34));
        let graph_roots = (
            MapRoot(Some(reference(51))),
            MapRoot(Some(reference(52))),
            MapRoot(Some(reference(53))),
            MapRoot(Some(reference(54))),
        );
        let old_root = GraphRoot {
            input: InputBinding {
                root: old_projection.owners,
                schema: instance.schema,
                policy: instance.policy,
                context: old_projection.materialization_context(),
            },
            projection: old_projection,
            owners: graph_roots.0,
            nodes: graph_roots.1,
            edges: graph_roots.2,
            incoming: graph_roots.3,
        };
        let old_bytes = serde_json::to_vec(&old_root)?;
        let old_children = [20, 31, 32, 33, 34, 51, 52, 53, 54].map(reference).to_vec();
        let old_artifact = objects.store(&old_bytes, &old_children)?.reference;
        let prior = MemberBinding {
            instance: instance.instance,
            format: instance.format,
            artifact: old_artifact,
            input: old_root.input,
            logical_commitment: Digest(Sha256::digest(&old_bytes).into()),
        };
        let affected = OwnerMutationSet {
            root: MapRoot(None),
            count: 0,
            digest: OwnerMutationDigest::default().finish(),
        };
        let mut work = budget()?;
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        // Each case starts from the same old artifact. Fixed child goldens are
        // independent of ProjectionBinding::object_children and the writer's assembly.
        for (selected_source, selected_certificate, expected, source_changed, cert_changed) in [
            (
                old_source,
                reference(44),
                [20, 31, 32, 33, 44, 51, 52, 53, 54],
                false,
                true,
            ),
            (
                new_source,
                reference(34),
                [20, 34, 41, 42, 43, 51, 52, 53, 54],
                true,
                false,
            ),
            (
                new_source,
                reference(44),
                [20, 41, 42, 43, 44, 51, 52, 53, 54],
                true,
                true,
            ),
        ] {
            let selected = projection(selected_source, selected_certificate);
            let writes_before = objects.writes();
            let candidate = NativeGraph.prepare_projection_delta(
                &instance,
                Some(&prior),
                &selected,
                Some(affected),
                &mut context,
            )?;
            let refreshed = NativeGraph.verify_and_seal(candidate, &mut context)?;
            check_equal(
                &objects.writes(),
                &writes_before
                    .checked_add(1)
                    .ok_or("test write count overflow")?,
                "projection refresh write count",
            )?;
            if refreshed.artifact == prior.artifact {
                return Err("projection refresh reused prior artifact".into());
            }
            let new_object = objects.get(refreshed.artifact, context.budget)?;
            let new_root: GraphRoot = serde_json::from_slice(&new_object.payload)?;
            check_equal(
                &new_root.projection,
                &selected,
                "selected projection binding",
            )?;
            check_equal(
                &(
                    new_root.owners,
                    new_root.nodes,
                    new_root.edges,
                    new_root.incoming,
                ),
                &graph_roots,
                "unchanged graph roots",
            )?;
            check_equal(
                &new_object.registration.children,
                &expected.map(reference).to_vec(),
                "projection child registration",
            )?;
            if source_changed {
                for historical in [reference(31), reference(32), reference(33)] {
                    if new_object.registration.children.contains(&historical) {
                        return Err("stale source child retained".into());
                    }
                }
            }
            if cert_changed && new_object.registration.children.contains(&reference(34)) {
                return Err("stale certificate child retained".into());
            }
        }
        check_equal(
            &objects.get(old_artifact, context.budget)?.payload,
            &old_bytes,
            "prior artifact payload",
        )?;
        check_equal(
            &objects
                .get(old_artifact, context.budget)?
                .registration
                .children,
            &old_children,
            "prior artifact children",
        )?;

        let writes_before = objects.writes();
        let same = NativeGraph.prepare_projection_delta(
            &instance,
            Some(&prior),
            &old_projection,
            Some(affected),
            &mut context,
        )?;
        check_equal(
            &NativeGraph.verify_and_seal(same, &mut context)?,
            &prior,
            "unchanged projection member binding",
        )?;
        check_equal(
            &objects.writes(),
            &writes_before,
            "unchanged projection write count",
        )?;
        Ok(())
    }

    struct PrefixMaps {
        root: MapRoot,
        entries: Vec<(Vec<u8>, ObjectRef)>,
    }

    impl PersistentMapPort for PrefixMaps {
        fn lookup_object(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<ObjectRef>, IndexError> {
            <EmptyMaps as PersistentMapPort>::lookup_object(&EmptyMaps, root, key, budget)
        }
        fn lookup(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
            self.lookup_object(root, key, budget)
                .map(|value| value.map(Into::into))
        }

        fn scan(
            &self,
            root: MapRoot,
            after: Option<&[u8]>,
            limit: u32,
            _: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            if root != self.root || limit != 1 {
                return Err(IndexError::BackendViolation(
                    "unexpected variant probe shape".into(),
                ));
            }
            let first = self
                .entries
                .iter()
                .find(|(key, _)| after.is_none_or(|cursor| key.as_slice() > cursor))
                .cloned();
            Ok(MapPage::new(
                first
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
                false,
                Vec::new(),
                None,
            ))
        }

        fn apply(
            &self,
            root: MapRoot,
            edits: &[MapEdit],
            sink: &dyn ObjectWriteSink,
            budget: &mut WorkBudget,
        ) -> Result<MapWrite, IndexError> {
            <EmptyMaps as PersistentMapPort>::apply(&EmptyMaps, root, edits, sink, budget)
        }

        fn apply_batch(
            &self,
            requests: &[MapBatchRequest],
            sink: &dyn ObjectWriteSink,
            budget: &mut WorkBudget,
        ) -> Result<MapBatchWrite, IndexError> {
            <EmptyMaps as PersistentMapPort>::apply_batch(&EmptyMaps, requests, sink, budget)
        }

        fn apply_stream(
            &self,
            roots: &[MapRoot],
            cursor: &mut dyn MapEditCursor,
            sink: &dyn ObjectWriteSink,
            budget: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
            <EmptyMaps as PersistentMapPort>::apply_stream(&EmptyMaps, roots, cursor, sink, budget)
        }
    }

    #[test]
    fn raw_identity_variants_conflict_across_owners_but_distinct_raw_ids_survive()
    -> Result<(), IndexError> {
        use quanta_index_contract::SymbolId;

        use super::super::affected::{node_key, validate_variant_identity};

        let nodes = MapRoot(Some(reference(60)));
        let shared_file = super::file("shared-id");
        let shared_symbol = super::RepoMapNodeRef::Symbol(SymbolId::new("shared-id"));
        let distinct_file = super::file("distinct-id");
        let mut entries = vec![
            (node_key(&shared_file, super::owner(1))?, reference(71)),
            (node_key(&shared_symbol, super::owner(2))?, reference(72)),
            (node_key(&distinct_file, super::owner(3))?, reference(73)),
        ];
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let maps = PrefixMaps {
            root: nodes,
            entries,
        };
        let objects = Objects::default();
        let custody = Custody;
        let scratch = NoScratch;
        let mut work = budget()?;
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        for conflicting in [&shared_file, &shared_symbol] {
            if !matches!(
                validate_variant_identity(nodes, conflicting, &mut context),
                Err(IndexError::InvalidInput(_))
            ) {
                return Err(IndexError::BackendViolation(
                    "cross-owner raw identity collision was accepted".into(),
                ));
            }
        }
        if validate_variant_identity(nodes, &distinct_file, &mut context).is_err() {
            return Err(IndexError::BackendViolation(
                "distinct raw identity was rejected".into(),
            ));
        }
        Ok(())
    }
}
