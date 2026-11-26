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
            facts: None,
            nodes: vec![node.clone()],
            edges: vec![edge.clone()]
        })
        .is_ok()
    );
    assert!(
        validate_output(&GraphOwnerOutput {
            projection: Vec::new(),
            facts: None,
            nodes: vec![node.clone(), node],
            edges: vec![]
        })
        .is_err()
    );
    assert!(
        validate_output(&GraphOwnerOutput {
            projection: Vec::new(),
            facts: None,
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
        facts: None,
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
        OwnerMutation, OwnerMutationSet, ProjectionBinding, ProjectionId,
        ProjectionOwnerContribution, PublicationGroupId, SchemaId, SlotId, SourceCut,
        StorageFormatId, StreamId, TenantId, WorkspaceKey, WorktreeId,
    };
    use quanta_index_core::index::{
        AttemptContext, IndexError, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapValue,
        MapWrite, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
        ObjectBatchInput, OwnerMutationDigest, RegisteredObject, RegisteredObjectBatch,
        ScratchAllocation, ScratchRequest, ScratchSpacePort, StoredObject, WorkBudget, WorkLimits,
        owner_map_key, owner_mutation_value,
        ports::{
            ImmutableObjectPort, MapEditCursor, ObjectCustodyPort, ObjectWriteSink,
            PersistentMapPort,
        },
    };
    use sha2::{Digest as _, Sha256};

    use super::super::{
        GraphRoot, LegacyGraphRoot, NativeGraph,
        contribution::{
            GraphElement, GraphSupport, OwnerGraphRecord, load_legacy_nonrank_output, load_output,
            load_owner, load_support,
        },
    };
    use super::node_key;
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
            let payload = br#"{"nodes":[],"edges":[],"projection":[],"facts":null}"#;
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
            facts: None,
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

    #[test]
    fn typed_file_facts_child_must_match_canonical_graph_nodes()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::super::contribution::load_output;
        use quanta_index_contract::{
            RepoMapFileNode, RepoMapNode, RepoRelativePath,
            native_protocol::NativeRepoMapFileFactsV1,
        };

        let objects = Objects::default();
        let mut work = budget()?;
        let node = RepoMapNode::File(RepoMapFileNode {
            file_id: super::FileId::new("src/a.rs"),
            repo_relative_path: RepoRelativePath::new("src/a.rs"),
            line_count: 10,
        });
        let facts = NativeRepoMapFileFactsV1 {
            owner_path: RepoRelativePath::new("src/a.rs"),
            nodes: vec![node.clone()],
        };
        let mut hash = Sha256::new();
        hash.update(b"quanta-index/native-repomap-node/v1\0");
        serde_json::to_writer(&mut hash, &node)?;
        let graph_node = super::GraphNode {
            identity: super::file("src/a.rs"),
            payload_digest: Digest(hash.finalize().into()),
        };
        let mut check = |facts: &NativeRepoMapFileFactsV1, graph: &super::GraphNode|
            -> Result<bool, Box<dyn std::error::Error>> {
            let facts_ref = objects.store(&serde_json::to_vec(facts)?, &[])?.reference;
            let output = super::GraphOwnerOutput {
                nodes: vec![graph.clone()],
                edges: Vec::new(),
                projection: Vec::new(),
                facts: Some(facts_ref),
            };
            let bytes = serde_json::to_vec(&output)?;
            let output_ref = objects.store(&bytes, &[facts_ref])?.reference;
            Ok(load_output(output_ref, &objects, &mut work).is_ok())
        };
        assert!(check(&facts, &graph_node)?);

        let mut wrong_path = facts.clone();
        wrong_path.owner_path = RepoRelativePath::new("src/b.rs");
        assert!(!check(&wrong_path, &graph_node)?);
        let mut wrong_payload = facts;
        let RepoMapNode::File(ref mut file) = wrong_payload.nodes[0] else {
            return Err("test file node changed variant".into());
        };
        file.line_count = 11;
        assert!(!check(&wrong_payload, &graph_node)?);
        let mut noncanonical = serde_json::to_vec(&wrong_payload)?;
        noncanonical.push(b' ');
        let facts_ref = objects.store(&noncanonical, &[])?.reference;
        let output = super::GraphOwnerOutput {
            nodes: vec![graph_node],
            edges: Vec::new(),
            projection: Vec::new(),
            facts: Some(facts_ref),
        };
        let output_ref = objects
            .store(&serde_json::to_vec(&output)?, &[facts_ref])?
            .reference;
        assert!(matches!(
            load_output(output_ref, &objects, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
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
    fn legacy_nonrank_output_uses_fixed_prior_wire() -> Result<(), Box<dyn std::error::Error>> {
        // Exact three-field wire emitted by the v20 writer before typed facts.
        const BASE_OUTPUT: &[u8] = b"{\"nodes\":[],\"edges\":[],\"projection\":[]}";
        let objects = Objects::default();
        let reference = objects.store(BASE_OUTPUT, &[])?.reference;
        let old = load_legacy_nonrank_output(reference, &objects, &mut budget()?)?;
        check_equal(&old.output.nodes.len(), &0, "legacy nodes")?;
        check_equal(&old.output.facts, &None, "legacy facts")?;
        if !matches!(
            load_output(reference, &objects, &mut budget()?),
            Err(IndexError::CorruptObject(_))
        ) {
            return Err("strict output reader accepted old wire".into());
        }
        Ok(())
    }

    #[test]
    fn legacy_nonrank_root_queries_and_patches_in_prior_wire()
    -> Result<(), Box<dyn std::error::Error>> {
        let objects = Objects::default();
        let instance = instance();
        let old_projection = projection(
            SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            reference(21),
        );
        let old_root = LegacyGraphRoot {
            input: InputBinding {
                root: old_projection.owners,
                schema: instance.schema,
                policy: instance.policy,
                context: old_projection.materialization_context(),
            },
            projection: old_projection,
            owners: MapRoot(None),
            nodes: MapRoot(None),
            edges: MapRoot(None),
            incoming: MapRoot(None),
        };
        let old_bytes = serde_json::to_vec(&old_root)?;
        // Independently assembled from the v20 six-field JSON schema and the
        // fixed fixture IDs above, including its SHA-256 projection context.
        const BASE_ROOT_SHA256: [u8; 32] = [
            0xc1, 0x7e, 0x86, 0xce, 0x7a, 0x2b, 0x55, 0x0f, 0x1a, 0x42, 0x75, 0x6d, 0x4e,
            0x43, 0xb5, 0xcf, 0x20, 0x5c, 0x06, 0xb3, 0x4f, 0x5f, 0x36, 0x76, 0x78, 0xce,
            0x5f, 0x70, 0x30, 0x9b, 0x11, 0x57,
        ];
        let old_digest: [u8; 32] = Sha256::digest(&old_bytes).into();
        check_equal(&old_digest, &BASE_ROOT_SHA256, "v20 root wire digest")?;
        if old_bytes
            .windows(b"facts_epoch".len())
            .any(|part| part == b"facts_epoch")
        {
            return Err("old root wire contains typed facts epoch".into());
        }
        let old_children = [reference(20), reference(21)];
        let old_artifact = objects.store(&old_bytes, &old_children)?.reference;
        let prior = MemberBinding {
            instance: instance.instance,
            format: instance.format,
            artifact: old_artifact,
            input: old_root.input,
            logical_commitment: Digest(Sha256::digest(&old_bytes).into()),
        };
        let rank_policy = quanta_index_contract::native_protocol::native_repomap_rank_policy();
        let rank_projection = ProjectionBinding {
            producer_policy: rank_policy,
            ..old_projection
        };
        let rank_root = LegacyGraphRoot {
            input: InputBinding {
                root: rank_projection.owners,
                schema: rank_projection.schema,
                policy: rank_policy,
                context: rank_projection.materialization_context(),
            },
            projection: rank_projection,
            ..old_root
        };
        let rank_bytes = serde_json::to_vec(&rank_root)?;
        let rank_artifact = objects.store(&rank_bytes, &old_children)?.reference;
        let rank_binding = MemberBinding {
            artifact: rank_artifact,
            input: rank_root.input,
            logical_commitment: Digest(Sha256::digest(&rank_bytes).into()),
            ..prior.clone()
        };
        if !matches!(
            super::super::read_root_for_query(&rank_binding, &objects, &mut budget()?),
            Err(IndexError::RebuildRequired)
        ) {
            return Err("legacy rank root did not require rebuild".into());
        }
        if !matches!(
            super::super::read_root_for_query(
                &MemberBinding {
                    logical_commitment: prior.logical_commitment,
                    ..rank_binding
                },
                &objects,
                &mut budget()?,
            ),
            Err(IndexError::CorruptObject(_))
        ) {
            return Err("corrupt legacy rank root hid behind rebuild requirement".into());
        }
        let queried = super::super::read_root_for_query(&prior, &objects, &mut budget()?)?;
        check_equal(&queried.facts_epoch, &0, "legacy query epoch")?;
        let page = super::super::query::query_graph(
            &prior,
            &quanta_index_contract::native_protocol::NativeGraphQuery {
                source: super::file("src/a.rs"),
                direction: quanta_index_contract::native_protocol::NativeGraphDirection::Outgoing,
                max_supporters: 1,
                max_edges: 1,
            },
            &EmptyMaps,
            &objects,
            &mut budget()?,
        )?;
        check_equal(&page.source, &None, "legacy empty query source")?;
        check_equal(&page.edges.len(), &0, "legacy empty query edges")?;
        let selected = ProjectionBinding {
            certificate: reference(22),
            ..old_projection
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
            maps: &EmptyMaps,
            custody: &Custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        let writes_before_rank_transition = objects.writes();
        if !matches!(
            NativeGraph.prepare_projection_delta(
                &instance,
                Some(&prior),
                &rank_projection,
                Some(affected),
                &mut context,
            ),
            Err(IndexError::RebuildRequired)
        ) {
            return Err("legacy nonrank root accepted a rank-policy delta".into());
        }
        check_equal(
            &objects.writes(),
            &writes_before_rank_transition,
            "rank-policy rejection writes",
        )?;
        let candidate = NativeGraph.prepare_projection_delta(
            &instance,
            Some(&prior),
            &selected,
            Some(affected),
            &mut context,
        )?;
        let binding = NativeGraph.verify_and_seal(candidate, &mut context)?;
        let posted = objects.get(binding.artifact, context.budget)?;
        if posted
            .payload
            .windows(b"facts_epoch".len())
            .any(|part| part == b"facts_epoch")
        {
            return Err("legacy delta upgraded root without typed facts".into());
        }
        let patched = super::super::read_root_for_query(&binding, &objects, context.budget)?;
        check_equal(&patched.facts_epoch, &0, "patched legacy epoch")?;
        check_equal(&patched.projection, &selected, "patched projection")?;
        check_equal(
            &posted.registration.children,
            &[reference(20), reference(22)].to_vec(),
            "patched root children",
        )?;
        Ok(())
    }

    struct OwnerRefreshMaps {
        key: Vec<u8>,
        selected: [(MapRoot, ObjectRef); 3],
        support: Option<(MapRoot, Vec<u8>, ObjectRef)>,
        mutations: MapRoot,
        mutation: MapValue,
        updated_owners: Mutex<Option<ObjectRef>>,
    }

    impl PersistentMapPort for OwnerRefreshMaps {
        fn lookup(
            &self,
            root: MapRoot,
            key: &[u8],
            _: &mut WorkBudget,
        ) -> Result<Option<MapValue>, IndexError> {
            if let Some((support_root, support_key, support)) = &self.support
                && root == *support_root
                && key == support_key
            {
                return Ok(Some((*support).into()));
            }
            self.selected
                .iter()
                .find(|(base, _)| *base == root && key == self.key.as_slice())
                .map(|(_, value)| Some((*value).into()))
                .ok_or_else(|| {
                    IndexError::BackendViolation("unexpected graph support lookup".into())
                })
        }

        fn scan(
            &self,
            root: MapRoot,
            after: Option<&[u8]>,
            limit: u32,
            _: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            if root == self.mutations && after.is_none() && limit == 1 {
                return Ok(MapPage::new(
                    vec![(self.key.clone(), self.mutation)],
                    false,
                    Vec::new(),
                    None,
                ));
            }
            if let Some((support_root, support_key, support)) = &self.support
                && root == *support_root
            {
                let entries = if after.is_some_and(|cursor| cursor < support_key.as_slice()) {
                    vec![(support_key.clone(), (*support).into())]
                } else {
                    Vec::new()
                };
                return Ok(MapPage::new(entries, false, Vec::new(), None));
            }
            if self.support.is_some()
                && (root == MapRoot(Some(reference(98))) || root == MapRoot(None))
            {
                return Ok(MapPage::new(Vec::new(), false, Vec::new(), None));
            }
            Err(IndexError::BackendViolation(
                "unexpected graph support scan".into(),
            ))
        }

        fn apply(
            &self,
            root: MapRoot,
            edits: &[MapEdit],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<MapWrite, IndexError> {
            let (owner_root, owner_record) = self.selected[2];
            let [edit] = edits else {
                return Err(IndexError::BackendViolation(
                    "expected one graph owner edit".into(),
                ));
            };
            if let Some((support_root, support_key, support)) = &self.support
                && root == *support_root
            {
                if edit.key != *support_key
                    || edit.expected != Some((*support).into())
                    || edit.next.is_some()
                {
                    return Err(IndexError::BackendViolation(
                        "unexpected graph support removal".into(),
                    ));
                }
                return Ok(MapWrite {
                    root: MapRoot(Some(reference(98))),
                });
            }
            if root != owner_root
                || edit.key != self.key
                || edit.expected != Some(owner_record.into())
            {
                return Err(IndexError::BackendViolation(
                    "unexpected graph support edit".into(),
                ));
            }
            let Some(MapValue::Object(next)) = edit.next else {
                return Err(IndexError::BackendViolation(
                    "missing refreshed graph owner".into(),
                ));
            };
            *self
                .updated_owners
                .lock()
                .map_err(|_| IndexError::Storage("test lock".into()))? = Some(next);
            Ok(MapWrite {
                root: MapRoot(Some(reference(99))),
            })
        }

        fn apply_batch(
            &self,
            requests: &[MapBatchRequest],
            sink: &dyn ObjectWriteSink,
            budget: &mut WorkBudget,
        ) -> Result<MapBatchWrite, IndexError> {
            EmptyMaps.apply_batch(requests, sink, budget)
        }

        fn apply_stream(
            &self,
            roots: &[MapRoot],
            cursor: &mut dyn MapEditCursor,
            sink: &dyn ObjectWriteSink,
            budget: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
            EmptyMaps.apply_stream(roots, cursor, sink, budget)
        }
    }

    #[test]
    fn identical_graph_output_refreshes_only_owner_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        refreshes_only_owner_metadata(false)
    }

    #[test]
    fn legacy_graph_output_deletion_reads_prior_wire() -> Result<(), Box<dyn std::error::Error>> {
        refreshes_only_owner_metadata(true)
    }

    fn refreshes_only_owner_metadata(legacy: bool) -> Result<(), Box<dyn std::error::Error>> {
        let objects = Objects::default();
        let output = super::GraphOwnerOutput {
            nodes: vec![super::GraphNode {
                identity: super::file("src/unchanged.rs"),
                payload_digest: Digest([60; 32]),
            }],
            edges: Vec::new(),
            projection: Vec::new(),
            facts: None,
        };
        let output_bytes = if legacy {
            // The one-owner deletion below must traverse the old output decoder.
            let current = serde_json::to_vec(&output)?;
            let mut old = current
                .strip_suffix(b",\"facts\":null}")
                .ok_or("current output lost its trailing facts field")?
                .to_vec();
            old.push(b'}');
            old
        } else {
            serde_json::to_vec(&output)?
        };
        let output_ref = objects.store(&output_bytes, &[])?.reference;
        let source = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let before = projection(source, reference(21));
        let after = ProjectionBinding {
            owners: MapRoot(Some(reference(22))),
            ..before
        };
        let owner = quanta_index_contract::index::OwnerKey {
            producer: before.producer,
            ..super::owner(1)
        };
        let contribution = |stamp, has_output| -> Result<ObjectRef, Box<dyn std::error::Error>> {
            let contribution = ProjectionOwnerContribution {
                projection: before.projection,
                producer: before.producer,
                owner,
                source_owner: None,
                source: None,
                dependency_stamp: Digest([stamp; 32]),
                output: has_output.then_some(output_ref),
            };
            Ok(objects
                .store(
                    &serde_json::to_vec(&contribution)?,
                    &contribution.object_children(),
                )?
                .reference)
        };
        let (previous, next) = (contribution(1, true)?, contribution(2, !legacy)?);
        let old_record = OwnerGraphRecord {
            owner,
            projection: previous,
            output: Some(output_ref),
        };
        let mut children = [previous, output_ref];
        children.sort_unstable();
        let old_record = objects
            .store(&serde_json::to_vec(&old_record)?, &children)?
            .reference;
        let support = if legacy {
            let node = output.nodes[0].clone();
            let support = GraphSupport {
                owner,
                element: GraphElement::Node(node.clone()),
                projection: None,
            };
            let support_ref = objects
                .store(&serde_json::to_vec(&support)?, &[])?
                .reference;
            Some((
                MapRoot(Some(reference(32))),
                node_key(&node.identity, owner)?,
                support_ref,
            ))
        } else {
            None
        };
        let mut root = GraphRoot {
            facts_epoch: if legacy {
                0
            } else {
                super::super::TYPED_FACTS_ROOT_EPOCH
            },
            input: InputBinding {
                root: before.owners,
                schema: before.schema,
                policy: instance().policy,
                context: before.materialization_context(),
            },
            projection: before,
            owners: MapRoot(Some(reference(31))),
            nodes: MapRoot(Some(reference(32))),
            edges: if legacy { MapRoot(None) } else { MapRoot(Some(reference(33))) },
            incoming: if legacy { MapRoot(None) } else { MapRoot(Some(reference(34))) },
        };
        let mutation = OwnerMutation {
            owner,
            expected: Some(previous),
            next: Some(next),
        };
        let maps = OwnerRefreshMaps {
            key: owner_map_key(owner).to_vec(),
            selected: [
                (before.owners, previous),
                (after.owners, next),
                (root.owners, old_record),
            ],
            support,
            mutations: MapRoot(Some(reference(35))),
            mutation: owner_mutation_value(&mutation)?,
            updated_owners: Mutex::new(None),
        };
        let mut digest = OwnerMutationDigest::default();
        digest.push(&mutation)?;
        let affected = OwnerMutationSet {
            root: maps.mutations,
            count: 1,
            digest: digest.finish(),
        };
        let mut work = budget()?;
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &objects,
            maps: &maps,
            custody: &Custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        let before_writes = objects.writes();
        let supports = if legacy {
            (MapRoot(Some(reference(98))), root.edges, root.incoming)
        } else {
            (root.nodes, root.edges, root.incoming)
        };
        super::super::affected::apply(&mut root, &after, Some(affected), &mut context)?;
        check_equal(
            &(root.nodes, root.edges, root.incoming),
            &supports,
            "reused graph support roots",
        )?;
        check_equal(
            &objects.writes(),
            &(before_writes + 1),
            "owner-only object write",
        )?;
        let updated = maps
            .updated_owners
            .lock()
            .map_err(|_| "map update lock")?
            .ok_or("graph owner was not refreshed")?;
        let updated = load_owner(updated, &objects, context.budget)?;
        check_equal(&updated.projection, &next, "refreshed graph contribution")?;
        check_equal(
            &updated.output,
            &(!legacy).then_some(output_ref),
            "selected immutable graph output",
        )?;
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
            facts_epoch: super::super::TYPED_FACTS_ROOT_EPOCH,
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
        let mut stale_root = serde_json::to_value(old_root)?;
        let _removed_epoch = stale_root
            .as_object_mut()
            .ok_or("graph root must serialize as a record")?
            .remove("facts_epoch")
            .ok_or("graph root must serialize its facts epoch")?;
        if serde_json::from_value::<GraphRoot>(stale_root).is_ok() {
            return Err("graph root accepted old wire without typed-facts epoch".into());
        }
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
        let wrong_epoch_bytes = serde_json::to_vec(&GraphRoot {
            facts_epoch: 0,
            ..old_root
        })?;
        let wrong_epoch_artifact = objects.store(&wrong_epoch_bytes, &old_children)?.reference;
        let wrong_epoch_binding = MemberBinding {
            artifact: wrong_epoch_artifact,
            logical_commitment: Digest(Sha256::digest(&wrong_epoch_bytes).into()),
            ..prior.clone()
        };
        if !matches!(
            super::super::load_root(&wrong_epoch_binding, &objects, &mut budget()?),
            Err(IndexError::CorruptObject(_))
        ) {
            return Err("graph root accepted prior typed-facts epoch".into());
        }
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
            if root != self.root || (limit != 1 && limit != 128) {
                return Err(IndexError::BackendViolation(
                    "unexpected variant probe shape".into(),
                ));
            }
            let selected: Vec<_> = self
                .entries
                .iter()
                .filter(|(key, _)| after.is_none_or(|cursor| key.as_slice() > cursor))
                .take(limit as usize)
                .cloned()
                .collect();
            Ok(MapPage::new(
                selected
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
    fn cross_file_endpoint_scan_resolves_only_its_exact_graph_supporter()
    -> Result<(), Box<dyn std::error::Error>> {
        use quanta_index_contract::{ChunkId, SymbolId};

        use super::super::affected::node_key;
        use super::super::prior_file::scan_prior_endpoint_supports_v1;

        let remote = super::RepoMapNodeRef::Symbol(SymbolId::new("src/a.rs::A"));
        let chunk = super::RepoMapNodeRef::Chunk(ChunkId::new("src/b.rs::chunk"));
        let cross_file = super::GraphEdge {
            kind: quanta_index_contract::RepoMapEdgeKind::OwnsChunk,
            source: remote.clone(),
            target: chunk.clone(),
        };
        let nodes = MapRoot(Some(reference(60)));
        let objects = Objects::default();
        let remote_support = GraphSupport {
            owner: super::owner(1),
            element: GraphElement::Node(super::GraphNode {
                identity: cross_file.source.clone(),
                payload_digest: Digest([61; 32]),
            }),
            projection: Some(reference(62)),
        };
        let chunk_support = GraphSupport {
            owner: super::owner(2),
            element: GraphElement::Node(super::GraphNode {
                identity: cross_file.target.clone(),
                payload_digest: Digest([63; 32]),
            }),
            projection: None,
        };
        let remote_ref = objects
            .store(
                &serde_json::to_vec(&remote_support)
                    .map_err(|error| IndexError::Storage(error.to_string()))?,
                &[reference(62)],
            )?
            .reference;
        let chunk_ref = objects
            .store(
                &serde_json::to_vec(&chunk_support)
                    .map_err(|error| IndexError::Storage(error.to_string()))?,
                &[],
            )?
            .reference;
        let mut entries = vec![
            (node_key(&remote, super::owner(1))?, remote_ref),
            (node_key(&chunk, super::owner(2))?, chunk_ref),
        ];
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let maps = PrefixMaps {
            root: nodes,
            entries,
        };
        let mut work = budget()?;
        let scanned =
            scan_prior_endpoint_supports_v1(nodes, &cross_file.source, &maps, &objects, &mut work)?;
        check_equal(
            &scanned.owners,
            &vec![(super::owner(1), Some(reference(62)))],
            "cross-file source owner",
        )?;
        let chunk_scanned =
            scan_prior_endpoint_supports_v1(nodes, &cross_file.target, &maps, &objects, &mut work)?;
        check_equal(
            &chunk_scanned.owners,
            &vec![(super::owner(2), None)],
            "cross-file target owner",
        )?;
        let foreign = GraphSupport {
            owner: super::owner(3),
            ..remote_support
        };
        let foreign_ref = objects
            .store(
                &serde_json::to_vec(&foreign)
                    .map_err(|error| IndexError::Storage(error.to_string()))?,
                &[reference(62)],
            )?
            .reference;
        let foreign_maps = PrefixMaps {
            root: nodes,
            entries: vec![(node_key(&remote, super::owner(1))?, foreign_ref)],
        };
        if scan_prior_endpoint_supports_v1(nodes, &remote, &foreign_maps, &objects, &mut work)
            .is_ok()
        {
            return Err("foreign graph supporter owner passed exact endpoint lookup".into());
        }
        Ok(())
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
