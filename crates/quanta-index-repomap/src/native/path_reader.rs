//! Typed, ticket-bound read adapter for the existing native file/path inventory.

use super::{FileMapReadContext, FilePathPage, FilePathQuery, NativeFileMap, query_paths};
use quanta_index_core::index::{
    IndexError, IndexQueryPort, MAX_SOURCE_PATH_BYTES, MemberBinding, QueryOutcome,
    QueryReadContext, effort::ResourceNeed, member_map_key, ports::IndexBuildPort,
};

const MAX_PAGE: u32 = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathReadRequest {
    pub prefix: String,
    pub after: Option<String>,
    pub limit: u32,
}

impl PathReadRequest {
    pub fn validate(&self) -> Result<(), IndexError> {
        if self.limit == 0
            || self.limit > MAX_PAGE
            || self.prefix.len() > MAX_SOURCE_PATH_BYTES
            || self.prefix.starts_with('/')
            || self.prefix.contains('\0')
            || self.after.as_ref().is_some_and(|after| {
                after.is_empty()
                    || after.len() > MAX_SOURCE_PATH_BYTES
                    || after.starts_with('/')
                    || after.contains('\0')
                    || !after.starts_with(&self.prefix)
                    || after
                        .split('/')
                        .any(|part| part.is_empty() || part == "." || part == "..")
            })
        {
            return Err(IndexError::InvalidInput(
                "native file path request exceeds its bounded capability".into(),
            ));
        }
        Ok(())
    }
}

pub struct NativePathReader {
    index: NativeFileMap,
    need: ResourceNeed,
}

impl NativePathReader {
    pub fn new(need: ResourceNeed) -> Result<Self, IndexError> {
        if need.cpu_units == 0
            || need.io_bytes == 0
            || need.memory_bytes == 0
            || need.provider_units != 0
        {
            return Err(IndexError::InvalidInput(
                "native file path read reservation is invalid".into(),
            ));
        }
        Ok(Self {
            index: NativeFileMap,
            need,
        })
    }

    fn pinned_member(context: &mut QueryReadContext<'_>) -> Result<MemberBinding, IndexError> {
        let key = member_map_key(context.instance.instance);
        let reference = context
            .maps
            .lookup_object(context.ticket.view.members, &key, context.budget)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let stored = context.objects.get(reference, context.budget)?;
        let member =
            quanta_index_core::index::decode_member_binding(&stored.payload, context.budget)?;
        let children = [Some(member.artifact), member.input.root.0];
        if stored.registration.reference != reference
            || !stored
                .registration
                .children
                .iter()
                .copied()
                .eq(children.into_iter().flatten())
            || member.instance != context.instance.instance
            || member.format != context.instance.format
            || member.input.schema != context.instance.schema
            || member.input.policy != context.instance.policy
            || member.input.root != context.ticket.view.source.owners
        {
            return Err(IndexError::CorruptObject(
                "native file member differs from selected ticket or instance".into(),
            ));
        }
        Ok(member)
    }
}

impl IndexQueryPort<PathReadRequest, FilePathPage> for NativePathReader {
    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        self.index.validate_instance(instance)
    }

    fn reservation(&self, request: &PathReadRequest) -> Result<ResourceNeed, IndexError> {
        request.validate()?;
        Ok(self.need)
    }

    fn query(
        &self,
        request: &PathReadRequest,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<FilePathPage>, IndexError> {
        self.validate_instance(context.instance)?;
        let _need = self.reservation(request)?;
        context.budget.checkpoint()?;
        let member = Self::pinned_member(context)?;
        let page = query_paths(
            &FilePathQuery {
                member: &member,
                prefix: &request.prefix,
                after: request.after.as_deref(),
                limit: request.limit,
            },
            &mut FileMapReadContext {
                ticket: context.ticket,
                objects: context.objects,
                maps: context.maps,
                budget: &mut *context.budget,
            },
        )?;
        QueryOutcome::core_scanned(context.ticket, context.instance, !page.has_more, page)
    }
}

#[cfg(test)]
mod tests {
    use super::{NativePathReader, PathReadRequest};
    use quanta_index_core::index::{
        CatalogView, CollectionId, Digest, GroupActivation, IndexError, IndexInstanceId, IndexKind,
        IndexQueryPort, InputBinding, InputDomain, InstanceSpec, MapBatchRequest, MapBatchWrite,
        MapEdit, MapPage, MapRoot, MapWrite, MemberBinding, ObjectBatchInput, ObjectRef,
        PublicationGroupId, QueryReadContext, ReadSessionRef, ReadTicketLease, RegisteredObject,
        SchemaId, SlotId, SourceCut, StorageFormatId, StoredObject, TenantId, TicketId,
        TicketRecord, ViewRef, WorkBudget, WorkLimits, WorkspaceKey, WorktreeId,
        effort::ResourceNeed,
        ports::{ImmutableObjectPort, MapEditCursor, ObjectWriteSink, PersistentMapPort},
    };
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    fn need() -> ResourceNeed {
        ResourceNeed {
            cpu_units: 1,
            io_bytes: 4096,
            memory_bytes: 4096,
            provider_units: 0,
        }
    }

    fn selected() -> Result<(TicketRecord, InstanceSpec), IndexError> {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let mut view = CatalogView {
            identity: ViewRef {
                workspace,
                epoch: 1,
                sequence: 1,
                digest: Digest([0; 32]),
            },
            profile: ObjectRef(Digest([5; 32])),
            source: SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            projections: MapRoot(None),
            members: MapRoot(None),
            coverage: MapRoot(None),
        };
        view.identity.digest = view
            .identity_digest()
            .map_err(|error| IndexError::CorruptObject(error.to_string()))?;
        Ok((
            TicketRecord {
                lease: ReadTicketLease {
                    id: TicketId([6; 16]),
                    session: ReadSessionRef { id: 1, fence: 1 },
                    fence: 1,
                    expires_at_ms: 10_000,
                },
                view,
            },
            InstanceSpec {
                instance: IndexInstanceId([7; 16]),
                kind: IndexKind([8; 16]),
                group: PublicationGroupId([9; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: SchemaId([10; 16]),
                policy: Digest([11; 32]),
                format: StorageFormatId([12; 16]),
            },
        ))
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or(IndexError::ResourceLimited)?;
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 100_000,
                mutations: 100,
            },
            deadline,
        ))
    }

    struct SpyObjects {
        member_ref: ObjectRef,
        member: StoredObject,
        gets: AtomicUsize,
    }
    impl ImmutableObjectPort for SpyObjects {
        fn identify(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("identify".into()))
        }
        fn put(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("put".into()))
        }
        fn install(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("install".into()))
        }
        fn install_batch(
            &self,
            _: &[ObjectBatchInput],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation("install_batch".into()))
        }
        fn get(
            &self,
            reference: ObjectRef,
            _: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            let _prior = self.gets.fetch_add(1, Ordering::Relaxed);
            if reference == self.member_ref {
                Ok(self.member.clone())
            } else {
                Err(IndexError::BackendViolation(
                    "artifact read forbidden".into(),
                ))
            }
        }
    }
    struct MemberMap {
        reference: ObjectRef,
        lookups: AtomicUsize,
    }
    impl PersistentMapPort for MemberMap {
        fn lookup_object(
            &self,
            _: MapRoot,
            key: &[u8],
            _: &mut WorkBudget,
        ) -> Result<Option<ObjectRef>, IndexError> {
            let _prior = self.lookups.fetch_add(1, Ordering::Relaxed);
            if key.starts_with(b"member\0") {
                Ok(Some(self.reference))
            } else {
                Err(IndexError::BackendViolation("unexpected map lookup".into()))
            }
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
            _: MapRoot,
            _: Option<&[u8]>,
            _: u32,
            _: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            Err(IndexError::BackendViolation("scan".into()))
        }
        fn apply(
            &self,
            _: MapRoot,
            _: &[MapEdit],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<MapWrite, IndexError> {
            Err(IndexError::BackendViolation("apply".into()))
        }
        fn apply_batch(
            &self,
            _: &[MapBatchRequest],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<MapBatchWrite, IndexError> {
            Err(IndexError::BackendViolation("apply_batch".into()))
        }
        fn apply_stream(
            &self,
            _: &[MapRoot],
            _: &mut dyn MapEditCursor,
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
            Err(IndexError::BackendViolation("apply_stream".into()))
        }
    }

    #[test]
    fn invalid_request_and_wrong_pinned_member_never_read_artifact()
    -> Result<(), Box<dyn std::error::Error>> {
        let (ticket, instance) = selected()?;
        let artifact = ObjectRef(Digest([31; 32]));
        let member_ref = ObjectRef(Digest([32; 32]));
        let member = MemberBinding {
            instance: instance.instance,
            format: instance.format,
            artifact,
            input: InputBinding {
                root: ticket.view.source.owners,
                schema: SchemaId([99; 16]), // selected profile says [10;16]
                policy: instance.policy,
                context: Digest([33; 32]),
            },
            logical_commitment: Digest([34; 32]),
        };
        let payload = serde_json::to_vec(&member)?;
        let objects = SpyObjects {
            member_ref,
            member: StoredObject::new(
                RegisteredObject::unscoped(member_ref, 0, vec![artifact]),
                payload,
                None,
            ),
            gets: AtomicUsize::new(0),
        };
        let maps = MemberMap {
            reference: member_ref,
            lookups: AtomicUsize::new(0),
        };
        let reader = NativePathReader::new(need())?;
        let mut budget = budget()?;
        let mut context = QueryReadContext {
            ticket: &ticket,
            instance: &instance,
            objects: &objects,
            maps: &maps,
            budget: &mut budget,
        };
        for invalid in [
            PathReadRequest {
                prefix: String::new(),
                after: None,
                limit: 0,
            },
            PathReadRequest {
                prefix: "src".into(),
                after: None,
                limit: 129,
            },
            PathReadRequest {
                prefix: "/src".into(),
                after: None,
                limit: 1,
            },
            PathReadRequest {
                prefix: "s\0rc".into(),
                after: None,
                limit: 1,
            },
            PathReadRequest {
                prefix: "x".repeat(quanta_index_core::index::MAX_SOURCE_PATH_BYTES + 1),
                after: None,
                limit: 1,
            },
            PathReadRequest {
                prefix: "src".into(),
                after: Some("other/file".into()),
                limit: 1,
            },
            PathReadRequest {
                prefix: "src".into(),
                after: Some("src//bad".into()),
                limit: 1,
            },
        ] {
            if !matches!(
                reader.query(&invalid, &mut context),
                Err(IndexError::InvalidInput(_))
            ) || objects.gets.load(Ordering::Relaxed) != 0
                || maps.lookups.load(Ordering::Relaxed) != 0
            {
                return Err(IndexError::BackendViolation(
                    "invalid path request reached storage".into(),
                )
                .into());
            }
        }
        PathReadRequest {
            prefix: "src".into(),
            after: None,
            limit: 128,
        }
        .validate()?;
        for invalid_need in [
            ResourceNeed {
                cpu_units: 0,
                ..need()
            },
            ResourceNeed {
                io_bytes: 0,
                ..need()
            },
            ResourceNeed {
                memory_bytes: 0,
                ..need()
            },
            ResourceNeed {
                provider_units: 1,
                ..need()
            },
        ] {
            if !matches!(
                NativePathReader::new(invalid_need),
                Err(IndexError::InvalidInput(_))
            ) || objects.gets.load(Ordering::Relaxed) != 0
                || maps.lookups.load(Ordering::Relaxed) != 0
            {
                return Err(IndexError::BackendViolation(
                    "invalid path reader reservation reached storage".into(),
                )
                .into());
            }
        }
        let valid = PathReadRequest {
            prefix: "src".into(),
            after: None,
            limit: 1,
        };
        if !matches!(
            reader.query(&valid, &mut context),
            Err(IndexError::CorruptObject(_))
        ) || objects.gets.load(Ordering::Relaxed) != 1
            || maps.lookups.load(Ordering::Relaxed) != 1
        {
            return Err(IndexError::BackendViolation(
                "wrong selected path member reached artifact".into(),
            )
            .into());
        }
        context.budget.request().cancel_handle().cancel();
        if !matches!(
            reader.query(&valid, &mut context),
            Err(IndexError::Cancelled)
        ) || objects.gets.load(Ordering::Relaxed) != 1
            || maps.lookups.load(Ordering::Relaxed) != 1
        {
            return Err(
                IndexError::BackendViolation("cancelled path read entered storage".into()).into(),
            );
        }
        Ok(())
    }
}
