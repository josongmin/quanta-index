use std::{sync::Arc, time::Duration};

use quanta_index_core::DiagnosticStageV1;
use quanta_index_core::index::{
    AdmissionOutcome, AttemptContext, AttemptRef, BootstrapRef, CatalogReadSettlementPort,
    CatalogView, CoverageBinding, CoverageStatus, Digest, GroupActivation, ImmutableOperation,
    IndexError, IndexInstanceId, IndexProfile, IngressRef, InputBinding, InputDomain, MapRoot,
    MapValue, MemberBinding, NativeReadReleaseObservation, NativeReadSettlement,
    NativeReadSettlementScope, OWNED_WORK_PRODUCER, ObjectBatchInput, ObjectInstallScope,
    ObjectRef, OperationKey, OperationStatus, OrdinalMapEdit, OwnedReadyOutcome,
    OwnedWorkAssignment, OwnedWorkBegin, OwnedWorkGrant, OwnedWorkPolicy, OwnedWorkReport,
    OwnerCoverage, OwnerInputBinding, OwnerKey, OwnerMemberBinding, OwnerMutation,
    OwnerMutationCursor, OwnerMutationSet, PreparedPublication, ProjectionBinding,
    ProjectionCertificate, ProjectionId, ProjectionOwnerContribution, ProjectionTransition,
    Refusal, RegisteredObject, SourceCut, SourceInput, SourceOperationMode, SourceRecord,
    SourceTextAdmission, StagedObjectReferences, SubmitOutcome, UpdateKind, UpdateRequest, ViewRef,
    ViewSelection, WorkAction, WorkBudget, WorkClaim, WorkIntent, WorkScope, WorkStatus, WorkUsage,
    WorkspaceKey, member_map_key as member_key, owner_map_key,
    ports::{
        CatalogDiagnosticPort, CatalogGenesisPort, CatalogPublicationPort, CatalogReadPort,
        ImmutableObjectPort, InputStagingPort, MapEditCursor, ObjectCustodyPort,
        ObjectMapBatchPort, ObjectWriteSink, OperationInspectAccess, OperationJournalPort,
        PersistentMapPort, SourceAuthorityPort, WorkClaimPort,
    },
    validate_read_ticket_lease,
};

use crate::{
    binding_metadata::{self, coverage_children, owner_coverage_children, owner_member_children},
    build_staging::PreparedBaseMember,
    diagnostic, identity,
    member_staging::{MemberEdits, PreparedEdit, ProfileIndex},
    mutation_staging::StageMutationCursor,
    profile::{LoadedProfile, MAX_PROFILE_INSTANCES},
    projection,
    registry::AdapterRegistry,
    request_decode::{self, LoadedRequest},
    request_preparation::{self, PreparedEvents},
    source,
    spool::{EditSpool, WorkActionBatch, WorkSpool},
    ticket::{ReadTicket, TicketCatalog},
};

/// Execution lane hint derived from the immutable operation kind. It does not
/// change journal admission, publication ordering, or correctness gates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationExecutionClass {
    Core,
    Enrichment,
}

impl OperationExecutionClass {
    #[must_use]
    pub fn for_request(request: &UpdateRequest) -> Self {
        match &request.kind {
            UpdateKind::Materialize { .. } => Self::Enrichment,
            UpdateKind::SourceAdvance(_)
            | UpdateKind::SourceResync { .. }
            | UpdateKind::ProjectionAdvance(_)
            | UpdateKind::Reconfigure { .. } => Self::Core,
        }
    }
}

/// A read-only resume decision. Unknown and terminal observations are replied
/// to immediately, so a later admission cannot change the queued lane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResumeDispatch {
    Execute(OperationExecutionClass),
    Reply(SubmitOutcome),
}

pub trait ControlCatalog:
    ObjectCustodyPort
    + InputStagingPort
    + OperationJournalPort
    + CatalogGenesisPort
    + CatalogPublicationPort
    + CatalogReadPort
    + CatalogDiagnosticPort
    + SourceAuthorityPort
    + WorkClaimPort
    + Send
    + Sync
{
}

impl<T> ControlCatalog for T where
    T: ObjectCustodyPort
        + InputStagingPort
        + OperationJournalPort
        + CatalogGenesisPort
        + CatalogPublicationPort
        + CatalogReadPort
        + CatalogDiagnosticPort
        + SourceAuthorityPort
        + WorkClaimPort
        + Send
        + Sync
{
}

pub struct KernelServices<C: ControlCatalog + 'static> {
    objects: Arc<dyn ImmutableObjectPort>,
    maps: Arc<dyn PersistentMapPort>,
    object_maps: Option<Arc<dyn ObjectMapBatchPort>>,
    catalog: Arc<C>,
    scratch: Arc<dyn quanta_index_core::index::ScratchSpacePort>,
    read_session: Arc<dyn quanta_index_core::index::ReadSessionLease>,
    read_lease: Duration,
}

impl<C: ControlCatalog + 'static> KernelServices<C> {
    pub fn new(
        objects: Arc<dyn ImmutableObjectPort>,
        maps: Arc<dyn PersistentMapPort>,
        catalog: Arc<C>,
        scratch: Arc<dyn quanta_index_core::index::ScratchSpacePort>,
        read_lease: Duration,
    ) -> Result<Self, IndexError> {
        let _validated_lease_ms = validate_read_ticket_lease(read_lease)?;
        let read_session = catalog.open_read_session()?;
        Ok(Self {
            objects,
            maps,
            object_maps: None,
            catalog,
            scratch,
            read_session,
            read_lease,
        })
    }

    /// Rebind all storage facets from one backend at construction time.
    /// A selected joined bank never retries through independent ports.
    #[must_use]
    pub fn with_object_map_backend<S>(mut self, backend: Arc<S>) -> Self
    where
        S: ImmutableObjectPort + PersistentMapPort + ObjectMapBatchPort + 'static,
    {
        self.objects = backend.clone();
        self.maps = backend.clone();
        self.object_maps = Some(backend);
        self
    }
}

pub struct Kernel<C: ControlCatalog + 'static> {
    services: KernelServices<C>,
    registry: AdapterRegistry,
}

struct IngressSink<'a> {
    ingress: IngressRef,
    catalog: &'a dyn InputStagingPort,
}

struct GenesisSink<'a> {
    stage: BootstrapRef,
    catalog: &'a dyn CatalogGenesisPort,
}

impl ObjectWriteSink for GenesisSink<'_> {
    fn managed_scope(&self) -> Option<ObjectInstallScope> {
        Some(ObjectInstallScope::Bootstrap(self.stage))
    }

    fn reserve(&self, object: &RegisteredObject) -> Result<(), IndexError> {
        self.catalog.reserve_bootstrap(self.stage, object)
    }

    fn installed(&self, object: &RegisteredObject) -> Result<(), IndexError> {
        self.catalog.confirm_bootstrap(self.stage, object)
    }

    fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
        Err(IndexError::InvalidInput(
            "genesis profile requires one object installation".into(),
        ))
    }

    fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
        Err(IndexError::InvalidInput(
            "genesis profile requires one object installation".into(),
        ))
    }
}

const MAX_SOURCE_BATCH_INPUTS: usize = 32;
const MAX_SOURCE_BATCH_BYTES: usize = 8 * 1024 * 1024;

fn source_record_payload(
    path: &str,
    content: ObjectRef,
    byte_len: usize,
    text: SourceTextAdmission,
    budget: &WorkBudget,
) -> Result<crate::staging_payload::StagedPayload, IndexError> {
    let _path_memory = budget.reserve_memory(
        u64::try_from(path.len()).map_err(|_error| IndexError::ResourceLimited)?,
        quanta_index_core::index::MemoryLifetime::Temporary,
    )?;
    let mut owned_path = String::new();
    owned_path
        .try_reserve_exact(path.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    owned_path.push_str(path);
    let record = SourceRecord {
        path: owned_path,
        bytes: content,
        byte_len: u64::try_from(byte_len).map_err(|_error| IndexError::ResourceLimited)?,
        text,
    };
    crate::staging_payload::encode(&record, MAX_SOURCE_BATCH_BYTES, budget)
}

fn source_batch_metadata_bytes(count: usize) -> Result<u64, IndexError> {
    let row = [
        // Body/record inputs, predicted pairs and confirmed object output.
        (std::mem::size_of::<ObjectBatchInput>(), 3usize),
        (std::mem::size_of::<RegisteredObject>(), 4),
        (
            std::mem::size_of::<Option<quanta_index_core::index::MemoryLease>>(),
            1,
        ),
        // One child for input, predicted and installed record respectively.
        (std::mem::size_of::<ObjectRef>(), 3),
    ]
    .into_iter()
    .try_fold(0usize, |sum, (bytes, count)| {
        sum.checked_add(
            bytes
                .checked_mul(count)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)
    })?;
    u64::try_from(row.checked_mul(count).ok_or(IndexError::ResourceLimited)?)
        .map_err(|_error| IndexError::ResourceLimited)
}

impl ObjectWriteSink for IngressSink<'_> {
    fn managed_scope(&self) -> Option<ObjectInstallScope> {
        Some(ObjectInstallScope::Ingress(self.ingress))
    }

    fn reserve(&self, object: &RegisteredObject) -> Result<(), IndexError> {
        self.catalog
            .reserve_inputs(self.ingress, std::slice::from_ref(object))
    }
    fn installed(&self, object: &RegisteredObject) -> Result<(), IndexError> {
        self.catalog
            .confirm_inputs(self.ingress, std::slice::from_ref(object))
    }
    fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
        self.catalog.reserve_inputs(self.ingress, objects)
    }
    fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
        self.catalog.confirm_inputs(self.ingress, objects)
    }
}

/// One selected view and one request budget. A borrowed ticket remains owned
/// by its caller across requests; stages never repin a newer view.
pub struct PinnedQuerySession<'a, C: ControlCatalog + 'static> {
    kernel: &'a Kernel<C>,
    ticket: SessionTicket<'a>,
    profile: LoadedProfile,
    budget: &'a mut WorkBudget,
}

// Keep the owned ticket inline: ordinary queries must not add a heap allocation
// solely to support callers that borrow an already admitted retained ticket.
#[expect(
    clippy::large_enum_variant,
    reason = "retained query support must not add an unadmitted heap allocation to ordinary queries"
)]
enum SessionTicket<'a> {
    Owned(ReadTicket),
    Borrowed(&'a mut ReadTicket),
}

impl std::ops::Deref for SessionTicket<'_> {
    type Target = ReadTicket;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(ticket) => ticket,
            Self::Borrowed(ticket) => ticket,
        }
    }
}

impl std::ops::DerefMut for SessionTicket<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Owned(ticket) => ticket,
            Self::Borrowed(ticket) => ticket,
        }
    }
}

impl SessionTicket<'_> {
    fn close(self) -> Result<(), IndexError> {
        match self {
            Self::Owned(ticket) => ticket.release(),
            Self::Borrowed(_ticket) => Ok(()),
        }
    }

    fn finish<R>(
        self,
        result: Result<R, IndexError>,
        budget: &WorkBudget,
    ) -> Result<R, IndexError> {
        match self {
            Self::Owned(ticket) => ticket.finish(result, budget),
            Self::Borrowed(ticket) => {
                let validated = ticket.validate().and_then(|()| budget.checkpoint());
                result.and_then(|value| validated.map(|()| value))
            }
        }
    }
}

impl<'a, C: ControlCatalog + 'static> PinnedQuerySession<'a, C> {
    /// Bind preparation to an admitted live stage without renewing work or time.
    pub fn bind_query_memory_scope(
        &mut self,
        memory: quanta_index_core::index::MemoryScope,
    ) -> Result<(), IndexError> {
        self.budget.checkpoint()?;
        self.budget.bind_memory_scope(Some(memory));
        self.budget.checkpoint()
    }

    /// Fund result conversion while its original producing execution is still
    /// active. An ended scope refuses; this never renews usage or the deadline.
    pub fn reserve_query_memory(
        &self,
        bytes: u64,
        lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<Option<quanta_index_core::index::MemoryLease>, IndexError> {
        self.budget.reserve_memory(bytes, lifetime)
    }

    #[must_use]
    pub fn view(&self) -> ViewRef {
        self.ticket.view().identity
    }

    /// Resolve the unique compatible reader in this session's selected profile.
    ///
    /// Descriptor errors remain errors even when another reader is compatible.
    /// This neither obtains another ticket nor reserves a query stage.
    pub fn unique_compatible_instance<Q, R>(
        &self,
        registry: &crate::QueryAdapterRegistry<Q, R>,
    ) -> Result<IndexInstanceId, IndexError> {
        let resolved = (|| {
            self.budget.checkpoint()?;
            self.ticket.validate()?;
            let mut selected = None;
            let mut ambiguous = false;
            for instance in &self.profile.instances {
                self.budget.checkpoint()?;
                if registry.resolve_optional(instance)?.is_some() {
                    if selected.is_some() {
                        ambiguous = true;
                    } else {
                        selected = Some(instance.instance);
                    }
                }
            }
            if ambiguous {
                return Err(IndexError::InvalidInput(
                    "multiple compatible selected query instances".into(),
                ));
            }
            selected.ok_or(IndexError::SnapshotUnavailable)
        })();
        self.budget.checkpoint()?;
        self.ticket.validate()?;
        self.budget.checkpoint()?;
        resolved
    }

    /// Extend the same ticket lease, subject to the original request budget.
    /// This never acquires a new ticket or changes the selected view.
    pub fn renew(&mut self) -> Result<(), IndexError> {
        self.budget.checkpoint()?;
        self.ticket.renew()?;
        self.budget.checkpoint()?;
        self.ticket.validate()?;
        self.budget.checkpoint()
    }

    /// End this request's session. An owned ticket is released; a borrowed
    /// ticket remains with its caller for later requests or explicit release.
    pub fn close(self) -> Result<(), IndexError> {
        self.ticket.close()
    }

    fn resolve_stage<Q, R>(
        &self,
        registry: &crate::QueryAdapterRegistry<Q, R>,
        instance_id: IndexInstanceId,
        request: Q,
    ) -> Result<FrozenQuery<Q, R>, IndexError> {
        self.resolve_optional_stage(registry, instance_id, request)?
            .ok_or(IndexError::SnapshotUnavailable)
    }

    fn resolve_optional_stage<Q, R>(
        &self,
        registry: &crate::QueryAdapterRegistry<Q, R>,
        instance_id: IndexInstanceId,
        request: Q,
    ) -> Result<Option<FrozenQuery<Q, R>>, IndexError> {
        let resolved = (|| {
            self.budget.checkpoint()?;
            self.ticket.validate()?;
            let Some(instance) = self
                .profile
                .instances
                .iter()
                .find(|instance| instance.instance == instance_id)
                .cloned()
            else {
                return Ok(None);
            };
            let Some((reader, ceiling)) = registry.resolve_optional(&instance)? else {
                return Ok(None);
            };
            let need = reader.reservation(&request)?;
            crate::query::validate_request_need(need, ceiling)?;
            Ok(Some(FrozenQuery {
                instance,
                reader,
                request,
                need,
            }))
        })();
        self.budget.checkpoint()?;
        self.ticket.validate()?;
        self.budget.checkpoint()?;
        resolved
    }

    /// Resolve a typed reader from the pinned profile. Execution capacity is
    /// still owned by the caller's exact scheduler reservation.
    pub fn prepare_stage<'s, Q, R>(
        &'s mut self,
        registry: &crate::QueryAdapterRegistry<Q, R>,
        instance_id: IndexInstanceId,
        request: Q,
    ) -> Result<PreparedQueryStage<'s, 'a, C, Q, R>, IndexError> {
        let query = self.resolve_stage(registry, instance_id, request)?;
        Ok(PreparedQueryStage {
            session: self,
            query,
            diagnostic_context: None,
        })
    }

    /// An absent selected instance or registered reader skips an optional
    /// stage.
    ///
    /// Descriptor, policy, reservation, ticket and budget errors remain
    /// errors; the retained session keeps its original view and budget.
    pub fn prepare_optional_stage<'s, Q, R>(
        &'s mut self,
        registry: &crate::QueryAdapterRegistry<Q, R>,
        instance_id: IndexInstanceId,
        request: Q,
    ) -> Result<Option<PreparedQueryStage<'s, 'a, C, Q, R>>, IndexError> {
        Ok(self
            .resolve_optional_stage(registry, instance_id, request)?
            .map(|query| PreparedQueryStage {
                session: self,
                query,
                diagnostic_context: None,
            }))
    }
}

struct FrozenQuery<Q, R> {
    instance: quanta_index_core::index::InstanceSpec,
    reader: Arc<dyn quanta_index_core::index::IndexQueryPort<Q, R>>,
    request: Q,
    need: quanta_index_core::index::effort::ResourceNeed,
}

/// A single stage of a retained session. Its drop or failure leaves the
/// session's original ticket and budget available for the next stage.
pub struct PreparedQueryStage<'s, 'a, C: ControlCatalog + 'static, Q, R> {
    session: &'s mut PinnedQuerySession<'a, C>,
    query: FrozenQuery<Q, R>,
    diagnostic_context: Option<quanta_index_core::DiagnosticContextV1>,
}

impl<C: ControlCatalog + 'static, Q, R> PreparedQueryStage<'_, '_, C, Q, R> {
    /// Parent this execution to an already opened diagnostic stage.
    /// The original ticket, request budget, custody and outcome remain unchanged.
    #[must_use]
    pub fn with_diagnostic_context(
        mut self,
        context: Option<quanta_index_core::DiagnosticContextV1>,
    ) -> Self {
        self.diagnostic_context = context;
        self
    }

    #[must_use]
    pub const fn required_resources(&self) -> quanta_index_core::index::effort::ResourceNeed {
        self.query.need
    }

    /// Abandon this stage without releasing its retained session.
    pub fn abort(self) {}

    pub fn execute<S: quanta_index_core::index::effort::ResourceScheduler>(
        self,
        guard: &quanta_index_core::index::effort::ReservationGuard<S>,
    ) -> Result<quanta_index_core::index::QueryOutcome<R>, IndexError> {
        let PreparedQueryStage {
            session,
            query,
            diagnostic_context,
        } = self;
        let mut budget = session.budget.scoped_diagnostic_context(diagnostic_context);
        execute_pinned_query(session.kernel, &session.ticket, &query, &mut budget, guard)
    }

    /// Provider IO retains the same admitted stage grant through physical
    /// completion.
    ///
    /// The scoped request cannot inherit another stage's custody.
    pub fn execute_with_provider_custody<S: quanta_index_core::index::effort::ResourceScheduler>(
        self,
        guard: &quanta_index_core::index::effort::ReservationGuard<S>,
        custody: &Arc<dyn quanta_index_core::ProviderAttemptCustodyPortV1>,
    ) -> Result<quanta_index_core::index::QueryOutcome<R>, IndexError> {
        let PreparedQueryStage {
            session,
            query,
            diagnostic_context,
        } = self;
        session
            .budget
            .with_provider_attempt_custody(custody, |budget| {
                let mut budget = budget.scoped_diagnostic_context(diagnostic_context);
                execute_pinned_query(session.kernel, &session.ticket, &query, &mut budget, guard)
            })
    }
}

/// One-shot compatibility facade over the same pinned-session execution path.
/// It releases its ticket after execution or explicit abort.
pub struct PreparedQuery<'a, C: ControlCatalog + 'static, Q, R> {
    session: PinnedQuerySession<'a, C>,
    query: FrozenQuery<Q, R>,
}

impl<C: ControlCatalog + 'static, Q, R> PreparedQuery<'_, C, Q, R> {
    #[must_use]
    pub const fn required_resources(&self) -> quanta_index_core::index::effort::ResourceNeed {
        self.query.need
    }

    /// Release a prepared query which could not obtain execution capacity.
    pub fn abort(self) -> Result<(), IndexError> {
        self.session.close()
    }

    pub fn execute<S: quanta_index_core::index::effort::ResourceScheduler>(
        self,
        guard: &quanta_index_core::index::effort::ReservationGuard<S>,
    ) -> Result<quanta_index_core::index::QueryOutcome<R>, IndexError> {
        let PreparedQuery { session, query } = self;
        let result = execute_pinned_query(
            session.kernel,
            &session.ticket,
            &query,
            &mut *session.budget,
            guard,
        );
        // Cleanup can wait for Catalog IO as well. Check the original budget
        // after release, without preventing cleanup of an already cancelled read.
        let PinnedQuerySession { ticket, budget, .. } = session;
        ticket.finish(result, budget)
    }

    /// One-shot reader composition under the same physical provider custody
    /// as retained-session stages.
    ///
    /// Ticket cleanup remains unconditional.
    pub fn execute_with_provider_custody<S: quanta_index_core::index::effort::ResourceScheduler>(
        self,
        guard: &quanta_index_core::index::effort::ReservationGuard<S>,
        custody: &Arc<dyn quanta_index_core::ProviderAttemptCustodyPortV1>,
    ) -> Result<quanta_index_core::index::QueryOutcome<R>, IndexError> {
        let PreparedQuery { session, query } = self;
        let result = session
            .budget
            .with_provider_attempt_custody(custody, |budget| {
                execute_pinned_query(session.kernel, &session.ticket, &query, budget, guard)
            });
        let PinnedQuerySession { ticket, budget, .. } = session;
        ticket.finish(result, budget)
    }
}

fn execute_pinned_query<
    C: ControlCatalog + 'static,
    Q,
    R,
    S: quanta_index_core::index::effort::ResourceScheduler,
>(
    kernel: &Kernel<C>,
    ticket: &ReadTicket,
    query: &FrozenQuery<Q, R>,
    budget: &mut WorkBudget,
    guard: &quanta_index_core::index::effort::ReservationGuard<S>,
) -> Result<quanta_index_core::index::QueryOutcome<R>, IndexError> {
    let result = (|| {
        budget.bind_memory_scope(guard.memory_scope());
        budget.checkpoint()?;
        ticket.validate()?;
        budget.checkpoint()?;
        if guard.need() != query.need || query.reader.reservation(&query.request)? != query.need {
            return Err(IndexError::BackendViolation(
                "query capability differs from the held service reservation".into(),
            ));
        }
        // This window borrows the original map port for this one funded call.
        // Its DATA is dropped before final ticket validation and cannot escape
        // into another stage, request or pin. Independent checks still run.
        let scoped_maps = kernel.services.maps.scoped_reads(budget)?;
        let maps = scoped_maps
            .as_deref()
            .unwrap_or_else(|| kernel.services.maps.as_ref());
        if query.instance.activation == GroupActivation::CoreRequired
            && let InputDomain::Projection {
                projection: projection_id,
                producer,
            } = query.instance.input_domain
        {
            let reference = maps
                .lookup_object(
                    ticket.view().projections,
                    &projection::projection_key(projection_id),
                    budget,
                )?
                .ok_or(IndexError::SnapshotUnavailable)?;
            let binding = projection::read_binding_from(
                reference,
                projection_id,
                kernel.services.objects.as_ref(),
                budget,
            )?;
            if binding.producer != producer || binding.schema != query.instance.schema {
                return Err(IndexError::CorruptObject(
                    "selected projection differs from reader profile".into(),
                ));
            }
            if binding.source != ticket.view().source {
                return Err(IndexError::SnapshotUnavailable);
            }
        }
        let outcome = query.reader.query(
            &query.request,
            &mut quanta_index_core::index::QueryReadContext {
                ticket: ticket.record(),
                instance: &query.instance,
                objects: kernel.services.objects.as_ref(),
                maps,
                budget: &mut *budget,
            },
        )?;
        if outcome.view() != ticket.view().identity {
            return Err(IndexError::BackendViolation(
                "query result differs from the selected view".into(),
            ));
        }
        match (query.instance.activation, outcome.coverage()) {
            (GroupActivation::CoreRequired, quanta_index_core::index::QueryCoverage::Core) => {}
            (
                GroupActivation::AsyncCoverage,
                quanta_index_core::index::QueryCoverage::Async(binding),
            ) => {
                let reference = maps
                    .lookup_object(
                        ticket.view().coverage,
                        &member_key(query.instance.instance),
                        budget,
                    )?
                    .ok_or(IndexError::SnapshotUnavailable)?;
                let stored = kernel.services.objects.get(reference, budget)?;
                let expected =
                    quanta_index_core::index::decode_coverage_binding(&stored.payload, budget)?;
                if stored.registration.reference != reference
                    || stored.registration.children.as_slice()
                        != coverage_children(&expected).as_slice()
                    || expected.instance != query.instance.instance
                    || expected != **binding
                {
                    return Err(IndexError::BackendViolation(
                        "query coverage differs from the selected stored coverage".into(),
                    ));
                }
            }
            (GroupActivation::CoreRequired, quanta_index_core::index::QueryCoverage::Async(_))
            | (GroupActivation::AsyncCoverage, quanta_index_core::index::QueryCoverage::Core) => {
                return Err(IndexError::BackendViolation(
                    "query coverage differs from the selected activation".into(),
                ));
            }
        }
        budget.checkpoint()?;
        Ok(outcome)
    })();
    // Preserve a reader's typed failure while still completing final lease
    // validation. A successful read must pass both checks before it escapes.
    let validated = ticket.validate().and_then(|()| budget.checkpoint());
    result.and_then(|outcome| validated.map(|()| outcome))
}

impl<C: ControlCatalog + 'static> Kernel<C> {
    #[must_use]
    pub fn new(services: KernelServices<C>, registry: AdapterRegistry) -> Self {
        Self { services, registry }
    }

    /// Starts or fences a durable, operation-scoped input stage. The returned
    /// ingress is required for every typed input write before `submit`.
    pub fn begin_input(&self, key: OperationKey) -> Result<IngressRef, IndexError> {
        if key.producer == OWNED_WORK_PRODUCER {
            return Err(IndexError::InvalidInput(
                "reserved owned work producer".into(),
            ));
        }
        self.services.catalog.open_ingress(key)
    }

    pub fn discard_input(
        &self,
        ingress: IngressRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.services.catalog.discard_inputs(ingress, budget)
    }

    fn stage_object(
        &self,
        ingress: IngressRef,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        let sink = IngressSink {
            ingress,
            catalog: self.services.catalog.as_ref(),
        };
        Ok(self
            .services
            .objects
            .install(payload, children, &sink, budget)?
            .reference)
    }

    pub fn stage_source(
        &self,
        ingress: IngressRef,
        path: &str,
        bytes: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        self.stage_source_text(ingress, path, bytes, SourceTextAdmission::Excluded, budget)
    }

    /// Stores the producer's explicit text admission without deriving it from
    /// decodable bytes. Validation precedes either body or record installation.
    pub fn stage_source_text(
        &self,
        ingress: IngressRef,
        path: &str,
        bytes: &[u8],
        text: SourceTextAdmission,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        source::validate_path(path)?;
        if matches!(text, SourceTextAdmission::Utf8 { .. }) && std::str::from_utf8(bytes).is_err() {
            return Err(IndexError::InvalidInput(
                "source text admission requires UTF-8".into(),
            ));
        }
        let predicted = self.services.objects.identify(bytes, &[], budget)?;
        let encoded = source_record_payload(path, predicted.reference, bytes.len(), text, budget)?;
        let content = self.stage_object(ingress, bytes, &[], budget)?;
        if content != predicted.reference {
            return Err(IndexError::BackendViolation(
                "source content changed during installation".into(),
            ));
        }
        self.stage_object(ingress, &encoded.bytes, &[content], budget)
    }

    /// Installs up to 32 source files under one ingress custody batch. All
    /// source bodies precede their records, so repeated bodies can be deduped
    /// without changing the returned record order.
    pub fn stage_sources(
        &self,
        ingress: IngressRef,
        inputs: &[SourceInput],
        budget: &mut WorkBudget,
    ) -> Result<StagedObjectReferences, IndexError> {
        budget.checkpoint()?;
        if inputs.is_empty() || inputs.len() > MAX_SOURCE_BATCH_INPUTS {
            return Err(IndexError::ResourceLimited);
        }
        let mut preflight_bytes = 0_usize;
        for input in inputs {
            source::validate_path(&input.path)?;
            preflight_bytes = preflight_bytes
                .checked_add(input.path.len())
                .and_then(|bytes| bytes.checked_add(input.bytes.len()))
                .and_then(|bytes| bytes.checked_add(256))
                .ok_or(IndexError::ResourceLimited)?;
            if preflight_bytes > MAX_SOURCE_BATCH_BYTES {
                return Err(IndexError::ResourceLimited);
            }
            if matches!(input.text, SourceTextAdmission::Utf8 { .. })
                && std::str::from_utf8(&input.bytes).is_err()
            {
                return Err(IndexError::InvalidInput(
                    "source text admission requires UTF-8".into(),
                ));
            }
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(preflight_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            mutations: u64::try_from(inputs.len()).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;

        let _metadata_memory = budget.reserve_memory(
            source_batch_metadata_bytes(inputs.len())?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        let result_memory = budget.reserve_memory(
            u64::try_from(
                inputs
                    .len()
                    .checked_mul(std::mem::size_of::<ObjectRef>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            quanta_index_core::index::MemoryLifetime::Retained,
        )?;
        let body_bytes = inputs.iter().try_fold(0u64, |sum, input| {
            sum.checked_add(
                u64::try_from(input.bytes.len()).map_err(|_error| IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)
        })?;
        let _body_memory = if body_bytes == 0 {
            None
        } else {
            budget.reserve_memory(
                body_bytes,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )?
        };
        // Declared first: packet backing dies before these JSON leases.
        let mut record_memory = Vec::new();
        record_memory
            .try_reserve_exact(inputs.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut bodies = Vec::new();
        bodies
            .try_reserve_exact(
                inputs
                    .len()
                    .checked_mul(2)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut records = Vec::new();
        records
            .try_reserve_exact(inputs.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut predicted = Vec::new();
        predicted
            .try_reserve_exact(inputs.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut wire_bytes = 0_u64;
        for input in inputs {
            budget.checkpoint()?;
            let body = self.services.objects.identify(&input.bytes, &[], budget)?;
            let record_payload = source_record_payload(
                &input.path,
                body.reference,
                input.bytes.len(),
                input.text,
                budget,
            )?;
            let record =
                self.services
                    .objects
                    .identify(&record_payload.bytes, &[body.reference], budget)?;
            wire_bytes = wire_bytes
                .checked_add(body.bytes)
                .and_then(|bytes| bytes.checked_add(record.bytes))
                .ok_or(IndexError::ResourceLimited)?;
            if wire_bytes
                > u64::try_from(MAX_SOURCE_BATCH_BYTES)
                    .map_err(|_error| IndexError::ResourceLimited)?
            {
                return Err(IndexError::ResourceLimited);
            }
            let mut body_payload = Vec::new();
            body_payload
                .try_reserve_exact(input.bytes.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            body_payload.extend_from_slice(&input.bytes);
            let mut children = Vec::new();
            children
                .try_reserve_exact(1)
                .map_err(|_error| IndexError::ResourceLimited)?;
            children.push(body.reference);
            bodies.push(ObjectBatchInput {
                payload: body_payload,
                children: Vec::new(),
            });
            records.push(ObjectBatchInput {
                payload: record_payload.bytes,
                children,
            });
            record_memory.push(record_payload.memory);
            predicted.push((body, record));
        }
        bodies.extend(records);
        let sink = IngressSink {
            ingress,
            catalog: self.services.catalog.as_ref(),
        };
        let installed = self
            .services
            .objects
            .install_batch(&bodies, &sink, budget)?;
        if installed.len() != bodies.len() {
            return Err(IndexError::BackendViolation(
                "source batch returned the wrong number of objects".into(),
            ));
        }
        let mut result = Vec::new();
        result
            .try_reserve_exact(inputs.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        for (index, (body, record)) in predicted.iter().enumerate() {
            if installed.get(index) != Some(body)
                || installed.get(
                    inputs
                        .len()
                        .checked_add(index)
                        .ok_or(IndexError::ResourceLimited)?,
                ) != Some(record)
            {
                return Err(IndexError::BackendViolation(
                    "source batch returned an identity or order mismatch".into(),
                ));
            }
            result.push(record.reference);
        }
        StagedObjectReferences::from_parts(result, result_memory)
    }

    pub fn stage_projection_output(
        &self,
        ingress: IngressRef,
        output: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        self.stage_projection_output_with_children(ingress, output, &[], budget)
    }

    /// Stages an immutable projection output with its exact, bounded direct
    /// child set. The projection adapter validates which children the output
    /// payload actually names; the object store and Catalog validate existence
    /// and retain the child edges under this ingress fence.
    pub fn stage_projection_output_with_children(
        &self,
        ingress: IngressRef,
        output: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        Self::validate_projection_output_children(children)?;
        self.stage_object(ingress, output, children, budget)
    }

    fn validate_projection_output_children(children: &[ObjectRef]) -> Result<(), IndexError> {
        if children.len() > quanta_index_core::index::MAX_RECLAIM_CHILDREN {
            return Err(IndexError::ResourceLimited);
        }
        if children
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left >= right))
        {
            return Err(IndexError::InvalidInput(
                "projection output children must be sorted and unique".into(),
            ));
        }
        Ok(())
    }

    pub fn stage_projection_owner(
        &self,
        ingress: IngressRef,
        contribution: ProjectionOwnerContribution,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        if contribution.source_owner.is_some() != contribution.source.is_some()
            || contribution.owner.producer != contribution.producer
        {
            return Err(IndexError::InvalidInput(
                "projection owner identity or source pair is invalid".into(),
            ));
        }
        if let Some(output) = contribution.output {
            let stored = self.services.objects.get(output, budget)?;
            if stored.registration.reference != output {
                return Err(IndexError::InvalidInput(
                    "projection output reference mismatch".into(),
                ));
            }
            Self::validate_projection_output_children(&stored.registration.children)?;
        }
        let payload = binding_metadata::encode_projection_owner(&contribution, budget)?;
        self.stage_object(
            ingress,
            &payload.bytes,
            projection::contribution_children(contribution).as_slice(),
            budget,
        )
    }

    pub fn stage_projection_certificate(
        &self,
        ingress: IngressRef,
        certificate: &ProjectionCertificate,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        let payload = binding_metadata::encode_projection_certificate(certificate, budget)?;
        self.stage_object(
            ingress,
            &payload.bytes,
            projection::certificate_children(certificate).as_slice(),
            budget,
        )
    }

    pub fn stage_profile(
        &self,
        ingress: IngressRef,
        profile: &IndexProfile,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        if profile.instances.len() > MAX_PROFILE_INSTANCES {
            return Err(IndexError::ResourceLimited);
        }
        self.registry.validate_profile(&profile.instances, budget)?;
        let payload = binding_metadata::encode_profile(profile, budget)?;
        self.stage_object(ingress, &payload.bytes, &[], budget)
    }

    /// Ingests a fallible stream without retaining the mutation set in RAM.
    /// Mutation values join their next child through the map installer under
    /// the same ingress fence; historical expected stamps remain data.
    pub fn stage_mutations<I>(
        &self,
        ingress: IngressRef,
        mutations: I,
        budget: &mut WorkBudget,
    ) -> Result<OwnerMutationSet, IndexError>
    where
        I: IntoIterator<Item = Result<OwnerMutation, IndexError>>,
    {
        let sink = IngressSink {
            ingress,
            catalog: self.services.catalog.as_ref(),
        };
        let mut cursor = StageMutationCursor::new(mutations.into_iter());
        let roots =
            self.services
                .maps
                .apply_stream(&[MapRoot(None)], &mut cursor, &sink, budget)?;
        let [root]: [MapRoot; 1] = roots.as_slice().try_into().map_err(|_roots| {
            IndexError::BackendViolation("staged mutation manifest lost root".into())
        })?;
        let (count, digest) = cursor.sealed_digest()?;
        Ok(OwnerMutationSet {
            root,
            count,
            digest,
        })
    }

    /// Bounded network page. Every page starts from the previous returned
    /// immutable root; absent-CAS on owner keys also rejects cross-page reuse.
    pub fn stage_mutation_page(
        &self,
        ingress: IngressRef,
        base: MapRoot,
        mutations: &[OwnerMutation],
        budget: &mut WorkBudget,
    ) -> Result<MapRoot, IndexError> {
        if mutations.is_empty() || mutations.len() > 128 {
            return Err(IndexError::ResourceLimited);
        }
        let sink = IngressSink {
            ingress,
            catalog: self.services.catalog.as_ref(),
        };
        let mut cursor = StageMutationCursor::new(mutations.iter().cloned().map(Ok));
        let roots = self
            .services
            .maps
            .apply_stream(&[base], &mut cursor, &sink, budget)?;
        let [root]: [MapRoot; 1] = roots
            .as_slice()
            .try_into()
            .map_err(|_roots| IndexError::BackendViolation("mutation page lost root".into()))?;
        let (_count, _digest) = cursor.sealed_digest()?;
        Ok(root)
    }

    /// Seals the complete staged map only after a bounded full cursor reaches
    /// EOF, and verifies the caller's count independently of page boundaries.
    pub fn seal_mutations(
        &self,
        ingress: IngressRef,
        root: MapRoot,
        count: u64,
        budget: &mut WorkBudget,
    ) -> Result<OwnerMutationSet, IndexError> {
        self.services
            .catalog
            .adopt_registered_inputs(ingress, &[], budget)?;
        if let Some(reference) = root.0 {
            let stored = self.services.objects.get(reference, budget)?;
            if stored.registration.reference != reference {
                return Err(IndexError::CorruptObject(
                    "staged mutation root identity mismatch".into(),
                ));
            }
            self.services.catalog.adopt_registered_inputs(
                ingress,
                std::slice::from_ref(&stored.registration),
                budget,
            )?;
        }
        let mut cursor = OwnerMutationCursor::unsealed(
            root,
            self.services.maps.as_ref(),
            self.services.objects.as_ref(),
        );
        while cursor.next_mutation(budget)?.is_some() {}
        let sealed = cursor.sealed_set().ok_or_else(|| {
            IndexError::BackendViolation("mutation seal did not reach EOF".into())
        })?;
        if sealed.count != count {
            return Err(IndexError::InvalidInput(
                "mutation page count disagrees with complete map".into(),
            ));
        }
        Ok(sealed)
    }

    /// Pin one immutable view and decode its profile once for all stages of
    /// the original request. Failed setup releases the newly acquired ticket.
    pub fn begin_query_session<'a>(
        &'a self,
        selection: ViewSelection,
        budget: &'a mut WorkBudget,
    ) -> Result<PinnedQuerySession<'a, C>, IndexError> {
        budget.checkpoint()?;
        let ticket = self.pin_view(selection, budget)?;
        let profile = (|| {
            ticket.validate()?;
            let profile = load_profile_object(
                ticket.view().profile,
                self.services.objects.as_ref(),
                budget,
            )?;
            budget.checkpoint()?;
            ticket.validate()?;
            budget.checkpoint()?;
            Ok(profile)
        })();
        match profile {
            Ok(profile) => Ok(PinnedQuerySession {
                kernel: self,
                ticket: SessionTicket::Owned(ticket),
                profile,
                budget,
            }),
            Err(error) => ticket.finish(Err(error), budget),
        }
    }

    /// Run one request against a ticket already held by this Kernel service.
    /// The caller retains the ticket across requests, including after an
    /// unsuccessful query; no new Exact acquisition is attempted.
    pub fn begin_query_session_with_ticket<'a>(
        &'a self,
        ticket: &'a mut ReadTicket,
        budget: &'a mut WorkBudget,
    ) -> Result<PinnedQuerySession<'a, C>, IndexError> {
        budget.checkpoint()?;
        self.require_read_ticket_owner(ticket)?;
        ticket.validate()?;
        let profile = load_profile_object(
            ticket.view().profile,
            self.services.objects.as_ref(),
            budget,
        )?;
        budget.checkpoint()?;
        ticket.validate()?;
        budget.checkpoint()?;
        Ok(PinnedQuerySession {
            kernel: self,
            ticket: SessionTicket::Borrowed(ticket),
            profile,
            budget,
        })
    }

    fn require_read_ticket_owner(&self, ticket: &ReadTicket) -> Result<(), IndexError> {
        let catalog: Arc<dyn CatalogReadPort> = self.services.catalog.clone();
        if !ticket.belongs_to(&catalog, &self.services.read_session) {
            return Err(IndexError::InvalidInput(
                "read ticket belongs to another Kernel service".into(),
            ));
        }
        Ok(())
    }

    fn require_read_settlement_scope(
        &self,
        ticket: &ReadTicket,
        scope: NativeReadSettlementScope,
    ) -> Result<(), IndexError> {
        self.require_read_ticket_owner(ticket)?;
        scope.validate()?;
        if scope.target != ticket.view().identity {
            return Err(IndexError::InvalidInput(
                "read settlement target differs from the held ticket".into(),
            ));
        }
        Ok(())
    }

    /// Release the original held pin and obtain its Catalog-owned custody
    /// observation. A failed release leaves the ticket with its caller.
    pub fn release_read_ticket(
        &self,
        ticket: &mut ReadTicket,
        scope: NativeReadSettlementScope,
        budget: &mut WorkBudget,
    ) -> Result<NativeReadReleaseObservation, IndexError>
    where
        C: CatalogReadSettlementPort,
    {
        self.require_read_settlement_scope(ticket, scope)?;
        ticket.release_held(self.services.catalog.as_ref(), scope, budget)
    }

    /// Observe logical settlement after release without reviving read access.
    /// This is one Catalog snapshot, not physical file settlement.
    pub fn observe_read_settlement(
        &self,
        ticket: &ReadTicket,
        scope: NativeReadSettlementScope,
        budget: &mut WorkBudget,
    ) -> Result<NativeReadSettlement, IndexError>
    where
        C: CatalogReadSettlementPort,
    {
        budget.checkpoint()?;
        self.require_read_settlement_scope(ticket, scope)?;
        let observation =
            ticket.observe_settlement(self.services.catalog.as_ref(), scope, budget)?;
        budget.checkpoint()?;
        Ok(observation)
    }

    /// One-shot compatibility path. It uses the same pinned profile and
    /// validation as a retained stage, then releases its ticket on completion.
    pub fn prepare_query<'a, Q, R>(
        &'a self,
        registry: &crate::QueryAdapterRegistry<Q, R>,
        selection: ViewSelection,
        instance_id: IndexInstanceId,
        request: Q,
        budget: &'a mut WorkBudget,
    ) -> Result<PreparedQuery<'a, C, Q, R>, IndexError> {
        let session = self.begin_query_session(selection, budget)?;
        let resolved = session.resolve_stage(registry, instance_id, request);
        match resolved {
            Ok(query) => Ok(PreparedQuery { session, query }),
            Err(error) => {
                let PinnedQuerySession { ticket, budget, .. } = session;
                ticket.finish(Err(error), budget)
            }
        }
    }

    /// Authenticate a source-local synchronous searchable frontier under the
    /// caller's existing ticket and resource grant. No synthetic query runs.
    pub fn validate_source_searchable<Q, R>(
        &self,
        registry: &crate::QueryAdapterRegistry<Q, R>,
        ticket: &ReadTicket,
        instance_id: IndexInstanceId,
        owner: OwnerKey,
        source: Option<ObjectRef>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        ticket.validate()?;
        let profile = load_profile_object(
            ticket.view().profile,
            self.services.objects.as_ref(),
            budget,
        )?;
        let instance = profile
            .instances
            .iter()
            .find(|spec| spec.instance == instance_id)
            .ok_or(IndexError::SnapshotUnavailable)?;
        if instance.activation != GroupActivation::CoreRequired
            || instance.input_domain != InputDomain::SourceLocal
        {
            return Err(IndexError::SnapshotUnavailable);
        }
        let (reader, _ceiling) = registry
            .resolve_optional(instance)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        reader.validate_searchable(
            owner,
            source,
            &mut quanta_index_core::index::QueryReadContext {
                ticket: ticket.record(),
                instance,
                objects: self.services.objects.as_ref(),
                maps: self.services.maps.as_ref(),
                budget: &mut *budget,
            },
        )?;
        ticket.validate()?;
        budget.checkpoint()
    }

    /// Genesis has an empty source and reserved empty Core profile. The catalog
    /// alone decides whether this workspace epoch can be created.
    pub fn create_workspace(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        budget: &mut WorkBudget,
    ) -> Result<ViewRef, IndexError> {
        budget.checkpoint()?;
        let profile = IndexProfile {
            instances: Vec::new(),
        };
        let encoded = binding_metadata::encode_profile(&profile, budget)?;
        let stage = self.services.catalog.begin_bootstrap(
            workspace,
            epoch,
            self.services.objects.as_ref(),
            budget,
        )?;
        let sink = GenesisSink {
            stage,
            catalog: self.services.catalog.as_ref(),
        };
        let object = self
            .services
            .objects
            .install(&encoded.bytes, &[], &sink, budget)?;
        if object.reference != stage.profile || !object.children.is_empty() {
            return Err(IndexError::BackendViolation(
                "genesis installation differs from frozen bootstrap identity".into(),
            ));
        }
        let identity = ViewRef {
            workspace,
            epoch,
            sequence: 0,
            digest: Digest([0; 32]),
        };
        let mut view = CatalogView {
            identity,
            profile: object.reference,
            source: SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            projections: MapRoot(None),
            members: MapRoot(None),
            coverage: MapRoot(None),
        };
        view.identity = identity::view_identity(&view)?;
        self.services.catalog.publish_bootstrap(stage, view, budget)
    }

    pub fn pin_view(
        &self,
        selection: ViewSelection,
        budget: &WorkBudget,
    ) -> Result<ReadTicket, IndexError> {
        let record = self.services.catalog.acquire_ticket(
            self.services.read_session.as_ref(),
            selection,
            self.services.read_lease,
            budget,
        )?;
        let catalog: Arc<dyn TicketCatalog> = self.services.catalog.clone();
        // A read session may retain this ticket after the issuing request ends.
        // Keep its diagnostic context scoped to acquisition, not ticket custody.
        Ok(ReadTicket::new(
            record,
            catalog,
            self.services.read_session.clone(),
            self.services.read_lease,
        ))
    }

    fn pin_materialization(
        &self,
        request: &UpdateRequest,
        budget: &WorkBudget,
    ) -> Result<ReadTicket, IndexError> {
        let span = diagnostic::begin(budget, DiagnosticStageV1::SqliteWork);
        let record = diagnostic::finish(
            span,
            diagnostic::catalog_call(self.services.catalog.as_ref(), budget, || {
                self.services.catalog.acquire_materialization_ticket(
                    self.services.read_session.as_ref(),
                    request,
                    self.services.read_lease,
                )
            }),
        )?;
        let catalog: Arc<dyn TicketCatalog> = self.services.catalog.clone();
        Ok(ReadTicket::new(
            record,
            catalog,
            self.services.read_session.clone(),
            self.services.read_lease,
        )
        .with_diagnostics(budget))
    }

    /// Resolve the producer CAS stamp under one immutable selected view.
    /// A stale projection source cut is returned unchanged: it is the prior
    /// stamp for a producer delta, not current searchable coverage.
    pub fn read_projection_binding(
        &self,
        selection: ViewSelection,
        projection_id: ProjectionId,
        budget: &mut WorkBudget,
    ) -> Result<(ViewRef, Option<ProjectionBinding>), IndexError> {
        budget.checkpoint()?;
        let ticket = self.pin_view(selection, budget)?;
        let result = (|| {
            ticket.validate()?;
            let reference = self.services.maps.lookup_object(
                ticket.view().projections,
                &projection::projection_key(projection_id),
                budget,
            )?;
            let binding = reference
                .map(|reference| {
                    projection::read_binding_from(
                        reference,
                        projection_id,
                        self.services.objects.as_ref(),
                        budget,
                    )
                })
                .transpose()?;
            ticket.validate()?;
            budget.checkpoint()?;
            Ok((ticket.view().identity, binding))
        })();
        ticket.finish(result, budget)
    }

    /// Bounded, explicit recovery of sessions whose process lock is gone.
    /// A live session is never retired solely because its ticket expired.
    pub fn recover_read_sessions(
        &self,
        after: Option<quanta_index_core::index::ReadSessionRef>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::ReadSessionRecoveryPage, IndexError> {
        self.services
            .catalog
            .recover_read_sessions(after, limit, budget)
    }

    pub fn status(
        &self,
        key: OperationKey,
        access: OperationInspectAccess<'_>,
    ) -> Result<OperationStatus, IndexError> {
        self.services.catalog.inspect(key, access)
    }

    /// Preview the exact immutable operation identity without publishing or
    /// staging it. Consumers can bind a local intent to the journal's later
    /// request reference even after terminal request bytes are reclaimed.
    pub fn identify_update_request(
        &self,
        request: &UpdateRequest,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        let upper_bound = charge_request_metadata(request, budget)?;
        let payload = request_preparation::encode(request, upper_bound, budget)?;
        let children = request_preparation::children(request);
        let identified =
            self.services
                .objects
                .identify(&payload.bytes, children.as_slice(), budget)?;
        budget.checkpoint()?;
        Ok(identified.reference)
    }

    /// Decide a recovery from its observed status and original immutable
    /// request. This reads custody only and never rotates the attempt fence.
    pub fn resume_dispatch(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
    ) -> Result<ResumeDispatch, IndexError> {
        if key.producer == OWNED_WORK_PRODUCER {
            return Err(IndexError::InvalidInput(
                "reserved owned work producer".into(),
            ));
        }
        match self
            .services
            .catalog
            .inspect(key, OperationInspectAccess::Request(budget))?
        {
            OperationStatus::Published(view) => {
                return Ok(ResumeDispatch::Reply(SubmitOutcome::Terminal(
                    OperationStatus::Published(view),
                )));
            }
            OperationStatus::Refused(reason) => {
                return Ok(ResumeDispatch::Reply(SubmitOutcome::Terminal(
                    OperationStatus::Refused(reason),
                )));
            }
            OperationStatus::Unknown => {
                return Ok(ResumeDispatch::Reply(SubmitOutcome::Unknown(key)));
            }
            OperationStatus::Pending(_) => {}
        }
        budget.checkpoint()?;
        let request = (|| {
            let immutable = self.services.catalog.operation(key)?;
            if immutable.key != key {
                return Err(IndexError::BackendViolation(
                    "journal operation key mismatch".into(),
                ));
            }
            let request = self.load_request(immutable, budget)?;
            Ok(OperationExecutionClass::for_request(&request))
        })();
        match request {
            Ok(class) => Ok(ResumeDispatch::Execute(class)),
            Err(error) => match self
                .services
                .catalog
                .inspect(key, OperationInspectAccess::PostFailureReconcile)
            {
                Ok(OperationStatus::Published(view)) => Ok(ResumeDispatch::Reply(
                    SubmitOutcome::Terminal(OperationStatus::Published(view)),
                )),
                Ok(OperationStatus::Refused(reason)) => Ok(ResumeDispatch::Reply(
                    SubmitOutcome::Terminal(OperationStatus::Refused(reason)),
                )),
                Ok(OperationStatus::Pending(_)) => Err(error),
                Ok(OperationStatus::Unknown) => {
                    Ok(ResumeDispatch::Reply(SubmitOutcome::Unknown(key)))
                }
                Err(status_error) => Err(status_error),
            },
        }
    }

    pub fn ready_work(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<Vec<WorkIntent>, IndexError> {
        self.services
            .catalog
            .ready_work(workspace, epoch, limit, budget)
    }

    pub fn claim_work(
        &self,
        intent: &WorkIntent,
        lease: Duration,
        budget: &WorkBudget,
    ) -> Result<WorkClaim, IndexError> {
        self.services.catalog.claim_work(intent, lease, budget)
    }

    pub fn renew_work(
        &self,
        claim: &WorkClaim,
        lease: Duration,
        budget: &WorkBudget,
    ) -> Result<WorkClaim, IndexError> {
        self.services.catalog.renew_work(claim, lease, budget)
    }

    pub fn renew_owned_work(&self, grant: &OwnedWorkGrant) -> Result<WorkClaim, IndexError> {
        self.services.catalog.renew_owned_work(grant)
    }

    pub fn work_status(
        &self,
        intent: &WorkIntent,
        budget: &WorkBudget,
    ) -> Result<WorkStatus, IndexError> {
        self.services.catalog.work_status(intent, budget)
    }

    pub fn ready_owned_work(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<OwnedReadyOutcome, IndexError> {
        self.services
            .catalog
            .ready_owned_work(workspace, epoch, limit, budget)
    }

    pub fn acquire_owned_work(
        &self,
        intent: &WorkIntent,
        policy: OwnedWorkPolicy,
        budget: &WorkBudget,
    ) -> Result<Option<OwnedWorkAssignment>, IndexError> {
        self.services
            .catalog
            .acquire_owned_work(intent, policy, budget)
    }

    pub fn begin_owned_work(
        &self,
        assignment: &OwnedWorkAssignment,
    ) -> Result<OwnedWorkBegin, IndexError> {
        self.services.catalog.begin_owned_work(assignment)
    }

    pub fn finish_owned_work(
        &self,
        grant: &OwnedWorkGrant,
        report: OwnedWorkReport,
    ) -> Result<(), IndexError> {
        self.services.catalog.finish_owned_work(grant, report)
    }

    /// Internal execution requires the exact live catalog-owned request.
    pub async fn submit_owned_work(
        &self,
        request: UpdateRequest,
        budget: &mut WorkBudget,
    ) -> Result<SubmitOutcome, IndexError> {
        if request.operation.producer != OWNED_WORK_PRODUCER {
            return Err(IndexError::InvalidInput(
                "owned request producer mismatch".into(),
            ));
        }
        let span = diagnostic::begin(budget, DiagnosticStageV1::SqliteWork);
        let status = diagnostic::finish(
            span,
            diagnostic::catalog_call(self.services.catalog.as_ref(), budget, || {
                self.services.catalog.validate_owned_work(&request)
            }),
        )?;
        self.submit_authorized(request, budget, Some(status)).await
    }

    /// Store one immutable request and submit its operation key. Already
    /// admitted keys return their original status; no new attempt is invented.
    pub async fn submit(
        &self,
        request: UpdateRequest,
        budget: &mut WorkBudget,
    ) -> Result<SubmitOutcome, IndexError> {
        if request.operation.producer == OWNED_WORK_PRODUCER {
            return Err(IndexError::InvalidInput(
                "reserved owned work producer".into(),
            ));
        }
        self.submit_authorized(request, budget, None).await
    }

    async fn submit_authorized(
        &self,
        request: UpdateRequest,
        budget: &mut WorkBudget,
        owned_entry_status: Option<OperationStatus>,
    ) -> Result<SubmitOutcome, IndexError> {
        if request.operation.workspace != request.expected_view.workspace
            || request.operation.epoch != request.expected_view.epoch
        {
            return Err(IndexError::InvalidInput(
                "operation and expected view belong to different workspace epochs".into(),
            ));
        }
        if let UpdateKind::Materialize { claim } = &request.kind
            && (claim.intent.workspace != request.operation.workspace
                || claim.intent.epoch != request.operation.epoch)
        {
            return Err(IndexError::InvalidInput(
                "claim belongs to another workspace epoch".into(),
            ));
        }
        if owned_entry_status.is_none() && matches!(&request.kind, UpdateKind::Materialize { .. }) {
            // Terminal receipts are replayable without retaining a live claim.
            if matches!(
                self.services
                    .catalog
                    .inspect(request.operation, OperationInspectAccess::Request(budget))?,
                OperationStatus::Unknown | OperationStatus::Pending(_)
            ) {
                let span = diagnostic::begin(budget, DiagnosticStageV1::SqliteWork);
                diagnostic::finish(
                    span,
                    self.services.catalog.validate_materialization(&request),
                )?;
            }
        }
        // This entry observation is consumed before any await. It is not a
        // grant: prepare_and_commit revalidates the durable request and claim.
        let existing = match owned_entry_status {
            Some(status) => status,
            None => self
                .services
                .catalog
                .inspect(request.operation, OperationInspectAccess::Request(budget))?,
        };
        match existing {
            OperationStatus::Unknown => {}
            existing @ (OperationStatus::Pending(_)
            | OperationStatus::Published(_)
            | OperationStatus::Refused(_)) => {
                let prior = self.services.catalog.operation(request.operation)?;
                if prior.source_mode != request.kind.source_mode() {
                    return Err(IndexError::CorruptObject(
                        "journal source mode differs from request kind".into(),
                    ));
                }
                self.validate_request_identity(&request, prior.request, budget)?;
                return Ok(match existing {
                    OperationStatus::Pending(attempt) => SubmitOutcome::Accepted(attempt),
                    status @ (OperationStatus::Published(_) | OperationStatus::Refused(_)) => {
                        SubmitOutcome::Terminal(status)
                    }
                    OperationStatus::Unknown => SubmitOutcome::Unknown(request.operation),
                });
            }
        }
        // Trace new-request metadata and encoding, excluding replay/claim lookup.
        let preflight = diagnostic::begin(budget, DiagnosticStageV1::BuildPreflight);
        let prepared = (|| {
            // Charge conservative envelope/input work before validating events.
            // Physical writes and later edits share this cumulative budget.
            let encoded_upper_bound = charge_request_metadata(&request, budget)?;
            match &request.kind {
                UpdateKind::ProjectionAdvance(transition) => {
                    projection::validate_events(request.operation, transition)?;
                }
                UpdateKind::SourceAdvance(transition)
                | UpdateKind::SourceResync { transition, .. } => {
                    crate::frontier::validate_event_shape(request.operation, &transition.events)?;
                    let digest = transition.owner_digest().map_err(|error| {
                        IndexError::InvalidInput(format!("source owner digest encoding: {error}"))
                    })?;
                    if transition
                        .events
                        .iter()
                        .any(|event| event.payload != digest)
                    {
                        return Err(IndexError::InvalidInput(
                            "source event payload does not bind owner manifest".into(),
                        ));
                    }
                }
                UpdateKind::Reconfigure { .. } | UpdateKind::Materialize { .. } => {}
            }
            let payload = request_preparation::encode(&request, encoded_upper_bound, budget)?;
            let children = request_preparation::children(&request);
            Ok((payload, children))
        })();
        let (payload, children) = diagnostic::finish(preflight, prepared)?;
        // Catalog API scopes include lock/row work, not SQL statements alone.
        let ingress_span = diagnostic::begin(budget, DiagnosticStageV1::SqliteWork);
        let ingress = diagnostic::finish(
            ingress_span,
            diagnostic::catalog_call(self.services.catalog.as_ref(), budget, || {
                self.services.catalog.open_ingress(request.operation)
            }),
        )?;
        let sink = IngressSink {
            ingress,
            catalog: self.services.catalog.as_ref(),
        };
        let stored =
            self.services
                .objects
                .install(&payload.bytes, children.as_slice(), &sink, budget)?;
        drop(payload);
        let operation = ImmutableOperation {
            key: request.operation,
            request: stored.reference,
            source_mode: request.kind.source_mode(),
        };
        // Owned admission performs the original seal and admission atomically.
        // Ordinary admission retains its explicit seal contract.
        if request.operation.producer != OWNED_WORK_PRODUCER {
            let seal_span = diagnostic::begin(budget, DiagnosticStageV1::SqliteWork);
            diagnostic::finish(
                seal_span,
                diagnostic::catalog_call(self.services.catalog.as_ref(), budget, || {
                    self.services.catalog.seal_inputs(ingress, stored.reference)
                }),
            )?;
        }
        let events = match &request.kind {
            UpdateKind::SourceAdvance(transition) | UpdateKind::SourceResync { transition, .. } => {
                transition.events.as_slice()
            }
            UpdateKind::ProjectionAdvance(transition) => transition.events.as_slice(),
            UpdateKind::Reconfigure { .. } | UpdateKind::Materialize { .. } => &[],
        };
        // Owned admission also identifies object bytes; its mixed API is not
        // included in this observation of ordinary Catalog admission.
        let admitted = if request.operation.producer == OWNED_WORK_PRODUCER {
            self.services.catalog.admit_owned(
                operation,
                ingress,
                &request,
                self.services.objects.as_ref(),
                budget,
            )?
        } else {
            let admit_span = diagnostic::begin(budget, DiagnosticStageV1::SqliteWork);
            diagnostic::finish(
                admit_span,
                self.services
                    .catalog
                    .admit(operation, ingress, events, budget),
            )?
        };
        match admitted {
            AdmissionOutcome::New(attempt) => self.drive(attempt, budget).await,
            AdmissionOutcome::Existing(OperationStatus::Pending(attempt)) => {
                Ok(SubmitOutcome::Accepted(attempt))
            }
            AdmissionOutcome::Existing(OperationStatus::Unknown) => {
                Ok(SubmitOutcome::Unknown(operation.key))
            }
            AdmissionOutcome::Existing(status) => Ok(SubmitOutcome::Terminal(status)),
        }
    }

    /// Recovery requires a live assignment or an exact retained terminal receipt.
    pub async fn resume_owned_work(
        &self,
        request: &UpdateRequest,
        budget: &mut WorkBudget,
    ) -> Result<SubmitOutcome, IndexError> {
        if request.operation.producer != OWNED_WORK_PRODUCER {
            return Err(IndexError::InvalidInput(
                "owned request producer mismatch".into(),
            ));
        }
        budget.checkpoint()?;
        let _validated_status =
            diagnostic::catalog_call(self.services.catalog.as_ref(), budget, || {
                self.services.catalog.validate_owned_work(request)
            })?;
        let immutable = self.services.catalog.operation(request.operation)?;
        match self
            .services
            .catalog
            .inspect(request.operation, OperationInspectAccess::Request(budget))?
        {
            OperationStatus::Published(_) | OperationStatus::Refused(_) => {
                self.validate_request_identity(request, immutable.request, budget)?;
            }
            OperationStatus::Pending(_) | OperationStatus::Unknown => {
                if *self.load_request(immutable, budget)? != *request {
                    return Err(IndexError::DigestConflict);
                }
            }
        }
        self.resume_authorized(request.operation, budget).await
    }

    /// Recovery reloads the original immutable operation, never caller input.
    pub async fn resume(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
    ) -> Result<SubmitOutcome, IndexError> {
        if key.producer == OWNED_WORK_PRODUCER {
            return Err(IndexError::InvalidInput(
                "reserved owned work producer".into(),
            ));
        }
        self.resume_authorized(key, budget).await
    }

    async fn resume_authorized(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
    ) -> Result<SubmitOutcome, IndexError> {
        match self
            .services
            .catalog
            .inspect(key, OperationInspectAccess::Request(budget))?
        {
            OperationStatus::Published(view) => {
                return Ok(SubmitOutcome::Terminal(OperationStatus::Published(view)));
            }
            OperationStatus::Refused(reason) => {
                return Ok(SubmitOutcome::Terminal(OperationStatus::Refused(reason)));
            }
            OperationStatus::Unknown => return Ok(SubmitOutcome::Unknown(key)),
            OperationStatus::Pending(_) => {}
        }
        budget.checkpoint()?;
        match diagnostic::catalog_call(self.services.catalog.as_ref(), budget, || {
            self.services.catalog.claim_recovery(key)
        }) {
            Ok(attempt) => self.drive(attempt, budget).await,
            Err(IndexError::FenceLost | IndexError::Conflict | IndexError::Storage(_)) => {
                Ok(self.reconcile(key))
            }
            Err(error) => Err(error),
        }
    }

    fn reconcile(&self, key: OperationKey) -> SubmitOutcome {
        match self
            .services
            .catalog
            .inspect(key, OperationInspectAccess::PostFailureReconcile)
        {
            Ok(status @ (OperationStatus::Published(_) | OperationStatus::Refused(_))) => {
                SubmitOutcome::Terminal(status)
            }
            Ok(OperationStatus::Pending(_) | OperationStatus::Unknown) | Err(_) => {
                SubmitOutcome::Unknown(key)
            }
        }
    }

    async fn drive(
        &self,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<SubmitOutcome, IndexError> {
        let key = attempt.operation;
        let result = self.prepare_and_commit(attempt, budget).await;
        match result {
            Ok(OperationStatus::Published(view)) => {
                Ok(SubmitOutcome::Terminal(OperationStatus::Published(view)))
            }
            Ok(OperationStatus::Refused(reason)) => {
                Ok(SubmitOutcome::Terminal(OperationStatus::Refused(reason)))
            }
            Ok(OperationStatus::Pending(_) | OperationStatus::Unknown) => Ok(self.reconcile(key)),
            Err(error) => refusal_for(&error).map_or_else(
                || Ok(self.reconcile(key)),
                |reason| match diagnostic::catalog_call(
                    self.services.catalog.as_ref(),
                    budget,
                    || self.services.catalog.refuse(attempt, reason),
                ) {
                    Ok(status @ (OperationStatus::Published(_) | OperationStatus::Refused(_))) => {
                        Ok(SubmitOutcome::Terminal(status))
                    }
                    Ok(_) | Err(_) => Ok(self.reconcile(key)),
                },
            ),
        }
    }

    async fn prepare_and_commit(
        &self,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<OperationStatus, IndexError> {
        let durable = self.services.catalog.operation(attempt.operation)?;
        if durable.key != attempt.operation {
            return Err(IndexError::BackendViolation(
                "journal operation key mismatch".into(),
            ));
        }
        let request = self.load_request(durable, budget)?;
        let source_count = match &request.kind {
            UpdateKind::SourceAdvance(transition) | UpdateKind::SourceResync { transition, .. } => {
                transition.owners.count
            }
            UpdateKind::ProjectionAdvance(_)
            | UpdateKind::Reconfigure { .. }
            | UpdateKind::Materialize { .. } => 0,
        };
        let reads = crate::attempt_reads::AttemptObjectReads::new(
            self.services.objects.as_ref(),
            source_count,
        )?;
        let mut context = AttemptContext {
            attempt,
            objects: &reads,
            maps: self.services.maps.as_ref(),
            custody: self.services.catalog.as_ref(),
            scratch: self.services.scratch.as_ref(),
            budget,
        };
        let materialization_ticket = if matches!(&request.kind, UpdateKind::Materialize { .. }) {
            Some(self.pin_materialization(&request, context.budget)?)
        } else {
            if request.operation.producer == OWNED_WORK_PRODUCER {
                let _validated_status = diagnostic::catalog_call(
                    self.services.catalog.as_ref(),
                    context.budget,
                    || self.services.catalog.validate_owned_work(&request),
                )?;
            }
            None
        };
        let admitted_operation = attempt.operation;
        let requested_view = request.expected_view;
        if request.operation != admitted_operation
            || requested_view.workspace != admitted_operation.workspace
            || requested_view.epoch != admitted_operation.epoch
        {
            return Err(IndexError::InvalidInput(
                "durable request identity mismatch".into(),
            ));
        }
        let mut ticket = match materialization_ticket {
            Some(ticket) => ticket,
            None => {
                diagnostic::catalog_call(self.services.catalog.as_ref(), context.budget, || {
                    self.pin_view(
                        ViewSelection::Latest {
                            workspace: request.expected_view.workspace,
                            epoch: request.expected_view.epoch,
                        },
                        context.budget,
                    )
                })?
            }
        };
        if matches!(&request.kind, UpdateKind::Materialize { .. }) {
            let UpdateKind::Materialize { claim } = &request.kind else {
                return Err(IndexError::BackendViolation("materialize dispatch".into()));
            };
            loop {
                let base = ticket.view();
                let result = self.materialize(attempt, base, claim, &mut context).await;
                match result {
                    Err(IndexError::Conflict) => {
                        context.budget.checkpoint()?;
                        let latest = self.pin_materialization(&request, context.budget)?;
                        if latest.view().identity == base.identity {
                            return Err(IndexError::Conflict);
                        }
                        ticket = latest;
                    }
                    result => return result,
                }
            }
        }
        let base = ticket.view();
        if base.identity != request.expected_view {
            return Err(IndexError::Conflict);
        }
        if let UpdateKind::ProjectionAdvance(transition) = &request.kind {
            return self
                .projection_advance(attempt, base, transition, &mut context)
                .await;
        }
        let (next_source, target_profile, delta, mut events, expected_source) = match &request.kind
        {
            UpdateKind::SourceAdvance(transition) | UpdateKind::SourceResync { transition, .. } => {
                let delta = source::apply_source(
                    attempt.operation,
                    base.identity,
                    base.source,
                    transition,
                    request.kind.source_mode(),
                    self.services.catalog.as_ref(),
                    &mut context,
                )?;
                (
                    delta.delta().next,
                    base.profile,
                    Some(delta),
                    PreparedEvents::copy(&transition.events, context.budget)?,
                    Some(transition.expected_source),
                )
            }
            UpdateKind::Reconfigure { profile } => (
                base.source,
                *profile,
                None,
                PreparedEvents::copy(&[], context.budget)?,
                None,
            ),
            UpdateKind::Materialize { .. } => {
                return Err(IndexError::BackendViolation("materialize dispatch".into()));
            }
            UpdateKind::ProjectionAdvance(_) => {
                return Err(IndexError::BackendViolation("projection dispatch".into()));
            }
        };
        let profile = self.load_profile(target_profile, &mut context)?;
        let prior_profile = if target_profile == base.profile {
            profile.clone()
        } else {
            self.load_profile(base.profile, &mut context)?
        };
        let build_span = crate::diagnostic::begin(
            context.budget,
            quanta_index_core::DiagnosticStageV1::BuildPrepare,
        );
        let built = self
            .build_members(
                base,
                &prior_profile,
                &profile,
                next_source,
                delta.as_ref(),
                &mut context,
            )
            .await;
        let (members, coverage, work) = crate::diagnostic::finish_work(
            context.budget,
            DiagnosticStageV1::BuildPrepare,
            build_span,
            built,
        )?;
        let sequence = base
            .identity
            .sequence
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let mut next = CatalogView {
            identity: ViewRef {
                sequence,
                digest: Digest([0; 32]),
                ..base.identity
            },
            profile: target_profile,
            source: next_source,
            projections: base.projections,
            members,
            coverage,
        };
        next.identity = identity::view_identity(&next)?;
        let (work_actions, mut actions) =
            WorkSpool::seal_lazy(work, context.objects, context.budget)?;
        let publication = PreparedPublication {
            attempt,
            source_mode: request.kind.source_mode(),
            expected: base.identity,
            next,
            work_actions,
            complete_work: None,
            source_events: events.take(),
            projection_events: Vec::new(),
            expected_source,
        };
        // The read ticket is allowed to outlive the commit; it pins immutable
        // base roots while the catalog transfers protection to the next view.
        let publish_span = crate::diagnostic::begin(
            context.budget,
            quanta_index_core::DiagnosticStageV1::NativePublish,
        );
        let published =
            self.services
                .catalog
                .commit_transition(publication, &mut actions, context.budget);
        crate::diagnostic::finish_publication(publish_span, published)
    }

    fn validate_request_identity(
        &self,
        request: &UpdateRequest,
        expected: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let encoded_upper_bound = charge_request_metadata(request, budget)?;
        let payload = request_preparation::encode(request, encoded_upper_bound, budget)?;
        let children = request_preparation::children(request);
        // Terminal replay compares canonical identity without requiring the
        // physical request retained only for pending execution.
        let identified =
            self.services
                .objects
                .identify(&payload.bytes, children.as_slice(), budget)?;
        if expected != identified.reference {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    }

    fn load_request(
        &self,
        operation: ImmutableOperation,
        budget: &mut WorkBudget,
    ) -> Result<LoadedRequest, IndexError> {
        let stored = self.services.objects.get(operation.request, budget)?;
        if stored.registration.reference != operation.request {
            return Err(IndexError::CorruptObject(
                "journal request reference mismatch".into(),
            ));
        }
        let request = request_decode::decode(&stored.payload, budget)?;
        if request.operation != operation.key
            || request.kind.source_mode() != operation.source_mode
            || request.expected_view.workspace != operation.key.workspace
            || request.expected_view.epoch != operation.key.epoch
            || stored.registration.children.as_slice()
                != request_preparation::children(&request).as_slice()
        {
            return Err(IndexError::CorruptObject(
                "journal request body/edge mismatch".into(),
            ));
        }
        Ok(request)
    }

    fn load_profile(
        &self,
        reference: ObjectRef,
        context: &mut AttemptContext<'_>,
    ) -> Result<LoadedProfile, IndexError> {
        let profile = load_profile_object(reference, context.objects, context.budget)?;
        self.registry
            .validate_profile(&profile.instances, context.budget)?;
        Ok(profile)
    }

    async fn build_members(
        &self,
        base: &CatalogView,
        prior_profile: &IndexProfile,
        profile: &IndexProfile,
        source: SourceCut,
        applied_delta: Option<&source::AppliedSource>,
        context: &mut AttemptContext<'_>,
    ) -> Result<(MapRoot, MapRoot, Option<WorkSpool>), IndexError> {
        let delta = applied_delta.map(source::AppliedSource::delta);
        let mut work = None;
        let mut work_batch = WorkActionBatch::default();
        let prior = ProfileIndex::new(&prior_profile.instances, context.budget)?;
        let target = ProfileIndex::new(&profile.instances, context.budget)?;
        let edit_limit = prior_profile
            .instances
            .len()
            .checked_add(profile.instances.len())
            .ok_or(IndexError::ResourceLimited)?;
        let mut edits = MemberEdits::new(edit_limit, context.budget)?;
        let mut coverage_edits = MemberEdits::new(edit_limit, context.budget)?;
        let source_context = identity::source_context(source)?;
        // Refuse an incompatible published owner layout before any adapter
        // build or coverage edit in this attempt.
        for spec in &profile.instances {
            if spec.activation != GroupActivation::AsyncCoverage
                || !prior
                    .get(&spec.instance)
                    .is_some_and(|previous| previous.activation == GroupActivation::AsyncCoverage)
            {
                continue;
            }
            let key = member_key(spec.instance);
            if let Some(reference) =
                context
                    .maps
                    .lookup_object(base.coverage, &key, context.budget)?
            {
                let coverage = load_coverage(reference, spec.instance, context)?;
                self.registry.validate_coverage(spec, &coverage)?;
            }
        }
        for instance in prior.union(&target) {
            let key = member_key(instance);
            let old_ref = context
                .maps
                .lookup_object(base.members, &key, context.budget)?;
            let old_binding = old_ref
                .map(|reference| load_member(reference, instance, context))
                .transpose()?;
            match prior.get(&instance) {
                Some(spec)
                    if spec.activation == GroupActivation::CoreRequired
                        && old_binding.is_none() =>
                {
                    return Err(IndexError::CorruptObject(
                        "published required member is absent".into(),
                    ));
                }
                None if old_binding.is_some() => {
                    return Err(IndexError::CorruptObject(
                        "member exists outside published profile".into(),
                    ));
                }
                _ => {}
            }
            let next_ref = match target.get(&instance) {
                Some(spec)
                    if spec.activation == GroupActivation::CoreRequired
                        && matches!(spec.input_domain, InputDomain::Projection { .. }) =>
                {
                    let InputDomain::Projection {
                        projection: projection_id,
                        producer,
                    } = spec.input_domain
                    else {
                        return Err(IndexError::BackendViolation(
                            "projection input dispatch".into(),
                        ));
                    };
                    let (_, projection_binding) =
                        projection::lookup_binding(base.projections, projection_id, context)?
                            .ok_or(IndexError::SnapshotUnavailable)?;
                    if projection_binding.producer != producer
                        || projection_binding.schema != spec.schema
                    {
                        return Err(IndexError::InvalidInput(
                            "required projection producer/schema mismatch".into(),
                        ));
                    }
                    let input = InputBinding {
                        root: projection_binding.owners,
                        schema: spec.schema,
                        policy: spec.policy,
                        context: projection_binding.materialization_context(),
                    };
                    let unchanged = prior.get(&instance).is_some_and(|old| *old == *spec);
                    let binding = if unchanged {
                        let old = old_binding.ok_or_else(|| {
                            IndexError::CorruptObject("required projection member absent".into())
                        })?;
                        if old.input != input || old.format != spec.format {
                            return Err(IndexError::CorruptObject(
                                "required projection member input mismatch".into(),
                            ));
                        }
                        old
                    } else {
                        if projection_binding.source != source {
                            return Err(IndexError::SnapshotUnavailable);
                        }
                        self.registry
                            .build_projection(spec, None, &projection_binding, None, context)?
                            .await?
                    };
                    Some(stage_required_member(&binding, context)?)
                }
                Some(spec) if spec.activation == GroupActivation::CoreRequired => {
                    if delta.is_some() && old_binding.is_none() {
                        return Err(IndexError::CorruptObject(
                            "required member absent before source advance".into(),
                        ));
                    }
                    let input = InputBinding {
                        root: source.owners,
                        schema: spec.schema,
                        policy: spec.policy,
                        context: source_context,
                    };
                    let binding = if delta.is_none()
                        && prior.get(&instance).is_some_and(|prior| *prior == *spec)
                    {
                        // Unchanged required instances retain their exact member
                        // during profile changes. A bad old binding is corruption,
                        // never an invitation to rebuild and hide the mismatch.
                        let binding = old_binding.ok_or_else(|| {
                            IndexError::CorruptObject("unchanged required member is absent".into())
                        })?;
                        if binding.instance != spec.instance
                            || binding.format != spec.format
                            || binding.input != input
                        {
                            return Err(IndexError::CorruptObject(
                                "unchanged required member input mismatch".into(),
                            ));
                        }
                        binding
                    } else {
                        let mut base_member = if delta.is_none() {
                            // New or explicitly changed required instances build
                            // from the selected immutable snapshot. Old roots stay
                            // protected by the old view until one Catalog commit.
                            PreparedBaseMember::new(None, context.budget)?
                        } else {
                            PreparedBaseMember::new(old_binding, context.budget)?
                        };
                        self.registry
                            .build(spec, base_member.take(), delta, source, input, context)?
                            .await?
                    };
                    Some(stage_required_member(&binding, context)?)
                }
                Some(spec) if spec.activation == GroupActivation::AsyncCoverage => None,
                None => None,
                Some(_) => {
                    return Err(IndexError::InvalidInput(
                        "unsupported group activation".into(),
                    ));
                }
            };
            if old_ref != next_ref {
                edits.push(&key, old_ref, next_ref, context.budget)?;
            }
            let coverage_key = member_key(instance);
            let covered =
                context
                    .maps
                    .lookup_object(base.coverage, &coverage_key, context.budget)?;
            let prior_async = prior
                .get(&instance)
                .copied()
                .filter(|spec| spec.activation == GroupActivation::AsyncCoverage);
            let unchanged_async = delta.is_none()
                && prior_async
                    .is_some_and(|old| target.get(&instance).is_some_and(|next| *next == old));
            let next_coverage = if unchanged_async {
                let reference = covered.ok_or_else(|| {
                    IndexError::CorruptObject("unchanged async coverage is absent".into())
                })?;
                let binding = load_coverage(reference, instance, context)?;
                if let Some(spec) = prior_async
                    && matches!(spec.input_domain, InputDomain::SourceLocal)
                    && binding.desired
                        != (InputBinding {
                            root: source.owners,
                            schema: spec.schema,
                            policy: spec.policy,
                            context: source_context,
                        })
                {
                    return Err(IndexError::CorruptObject(
                        "unchanged async source input mismatch".into(),
                    ));
                }
                Some(reference)
            } else {
                if delta.is_none()
                    && let Some(old_spec) = prior_async
                {
                    let reference = covered.ok_or_else(|| {
                        IndexError::CorruptObject("published async coverage is absent".into())
                    })?;
                    let binding = load_coverage(reference, instance, context)?;
                    if let Some(next_spec) = target.get(&instance)
                        && next_spec.activation == GroupActivation::AsyncCoverage
                        && binding.owners.0.is_some()
                        && (old_spec.kind != next_spec.kind
                            || old_spec.group != next_spec.group
                            || old_spec.input_domain != next_spec.input_domain
                            || old_spec.schema != next_spec.schema
                            || old_spec.format != next_spec.format
                            || old_spec.policy == next_spec.policy)
                    {
                        return Err(IndexError::RebuildRequired);
                    }
                    retire_unfinished_coverage(
                        base,
                        old_spec,
                        &binding,
                        &mut work,
                        &mut work_batch,
                        context,
                    )?;
                }
                match target.get(&instance) {
                    Some(spec)
                        if spec.activation == GroupActivation::AsyncCoverage
                            && matches!(spec.input_domain, InputDomain::Projection { .. }) =>
                    {
                        let InputDomain::Projection {
                            projection: projection_id,
                            producer,
                        } = spec.input_domain
                        else {
                            return Err(IndexError::BackendViolation("projection domain".into()));
                        };
                        let prior_coverage = if delta.is_some() {
                            covered
                                .map(|reference| load_coverage(reference, instance, context))
                                .transpose()?
                        } else {
                            None
                        };
                        if prior.get(&instance).is_some_and(|old| *old != *spec)
                            && prior_coverage.is_some_and(|binding| binding.owners.0.is_some())
                        {
                            return Err(IndexError::RebuildRequired);
                        }
                        let current_projection =
                            projection::lookup_binding(base.projections, projection_id, context)?;
                        if current_projection
                            .is_some_and(|(_, binding)| binding.producer != producer)
                        {
                            return Err(IndexError::InvalidInput(
                                "projection producer differs from index profile".into(),
                            ));
                        }
                        if current_projection.is_some_and(|(_, binding)| binding.owners.0.is_some())
                            && prior_coverage.is_none_or(|binding| binding.owners.0.is_none())
                        {
                            return Err(IndexError::RebuildRequired);
                        }
                        let prior_missing = match prior_coverage.map(|binding| binding.status) {
                            Some(CoverageStatus::Partial { missing }) => missing,
                            Some(CoverageStatus::Pending { previous_missing }) => previous_missing,
                            Some(CoverageStatus::Complete) | None => MapRoot(None),
                        };
                        let desired = prior_coverage.map_or(
                            InputBinding {
                                root: MapRoot(None),
                                schema: spec.schema,
                                policy: spec.policy,
                                context: Digest([0; 32]),
                            },
                            |binding| binding.desired,
                        );
                        let binding = CoverageBinding {
                            instance,
                            desired,
                            covered: prior_coverage.and_then(|binding| binding.covered),
                            searchable: prior_coverage.and_then(|binding| binding.searchable),
                            status: CoverageStatus::Pending {
                                previous_missing: prior_missing,
                            },
                            owners: prior_coverage.map_or(MapRoot(None), |binding| binding.owners),
                        };
                        Some(save_coverage(&binding, context)?)
                    }
                    Some(spec) if spec.activation == GroupActivation::AsyncCoverage => {
                        let desired = InputBinding {
                            root: source.owners,
                            schema: spec.schema,
                            policy: spec.policy,
                            context: source_context,
                        };
                        let prior_coverage = if delta.is_some() {
                            covered
                                .map(|reference| load_coverage(reference, instance, context))
                                .transpose()?
                        } else {
                            None
                        };
                        if prior.get(&instance).is_some_and(|old| {
                            old.input_domain != spec.input_domain
                                || old.kind != spec.kind
                                || old.format != spec.format
                        }) && prior_coverage.is_some_and(|binding| binding.owners.0.is_some())
                        {
                            return Err(IndexError::RebuildRequired);
                        }
                        if delta.is_some()
                            && prior
                                .get(&instance)
                                .is_some_and(|old| old.activation == GroupActivation::AsyncCoverage)
                            && prior_coverage.is_none()
                        {
                            return Err(IndexError::CorruptObject(
                                "published async coverage is absent".into(),
                            ));
                        }
                        let prior_owners =
                            prior_coverage.map_or(MapRoot(None), |binding| binding.owners);
                        let prior_missing = match prior_coverage.map(|binding| binding.status) {
                            Some(CoverageStatus::Partial { missing }) => missing,
                            Some(CoverageStatus::Pending { .. }) => {
                                return Err(IndexError::RebuildRequired);
                            }
                            Some(CoverageStatus::Complete) | None => MapRoot(None),
                        };
                        let mut edits = None;
                        // Empty coverage has no cached or stale owner member:
                        // every accepted addition enters both maps as pending.
                        // Produce one edit stream instead of duplicating it for
                        // Storage's existing identical-root reuse to rediscover.
                        let mut pending_coverage = match applied_delta {
                            Some(applied) => PendingCoverageBatch::with_known_count(
                                applied.delta().changed.count,
                            )?,
                            None => PendingCoverageBatch::default(),
                        };
                        pending_coverage.shared_pending_roots =
                            prior_owners.0.is_none() && prior_missing.0.is_none();
                        let scan = diagnostic::WorkStage::begin(
                            context.budget,
                            DiagnosticStageV1::NativeCoverageScan,
                        );
                        let mut changes = match source::CoverageChanges::new(
                            source,
                            applied_delta,
                            context.attempt,
                            context.maps,
                            context.objects,
                        ) {
                            Ok(changes) => changes,
                            Err(error) => return scan.finish(context.budget, Err(error)),
                        };
                        let scanned = (|| -> Result<(), IndexError> {
                            while let Some(change) = changes.next_mutation(context.budget)? {
                                if edits.is_none() {
                                    edits = Some(EditSpool::open(context.scratch, context.budget)?);
                                }
                                let edits = edits.as_ref().ok_or_else(|| {
                                    IndexError::BackendViolation("async edit spool missing".into())
                                })?;
                                let owner_key = owner_map_key(change.owner);
                                let old_ref = context.maps.lookup(
                                    prior_owners,
                                    &owner_key,
                                    context.budget,
                                )?;
                                let old = old_ref
                                    .map(|reference| {
                                        load_owner_coverage(reference, change.owner, context)
                                    })
                                    .transpose()?;
                                let missing_ref = context.maps.lookup(
                                    prior_missing,
                                    &owner_key,
                                    context.budget,
                                )?;
                                if old.is_none() && change.expected.is_some() {
                                    return Err(IndexError::CorruptObject(
                                        "async owner coverage is absent".into(),
                                    ));
                                }
                                let next_input = change.next.map(|record| OwnerInputBinding {
                                    source: Some(record),
                                    projection: None,
                                    producer_context: Digest([0; 32]),
                                    derivation_policy: spec.policy,
                                });
                                if let Some(previous) = old
                                    && previous.covered != Some(previous.desired)
                                    && Some(previous.desired) != next_input
                                {
                                    work_batch.push(
                                        &WorkAction::Retire(owner_work(
                                            base,
                                            instance,
                                            change.owner,
                                            previous.desired,
                                        )),
                                        &mut work,
                                        context.scratch,
                                        context.budget,
                                    )?;
                                }
                                let next_ref = match next_input {
                                    Some(owner_input)
                                        if old
                                            .is_some_and(|prior| prior.desired == owner_input) =>
                                    {
                                        old_ref
                                    }
                                    Some(owner_input) => {
                                        let cached =
                                            old.filter(|prior| prior.covered == Some(owner_input));
                                        let unchanged_instance = prior
                                            .get(&instance)
                                            .is_some_and(|old_spec| *old_spec == *spec);
                                        let previous_searchable = old.filter(|previous| {
                                        cached.is_none()
                                            && previous.covered == Some(previous.desired)
                                            && previous.searchable.is_some()
                                            && unchanged_instance
                                            && matches!(
                                                (change.expected, change.next),
                                                (Some(prior_ref), Some(_next_ref))
                                                    if previous.desired.source == Some(prior_ref)
                                            )
                                    });
                                        // A stale member is an authenticated build candidate,
                                        // not current searchable coverage. The adapter decides
                                        // full-payload or member reuse against the new input.
                                        let owner_coverage = OwnerCoverage {
                                            owner: change.owner,
                                            desired: owner_input,
                                            covered: cached
                                                .or(previous_searchable)
                                                .and_then(|prior| prior.covered),
                                            searchable: cached
                                                .or(previous_searchable)
                                                .and_then(|prior| prior.searchable),
                                        };
                                        if cached.is_none() {
                                            work_batch.push(
                                                &WorkAction::Create(owner_work(
                                                    base,
                                                    instance,
                                                    change.owner,
                                                    owner_input,
                                                )),
                                                &mut work,
                                                context.scratch,
                                                context.budget,
                                            )?;
                                        }
                                        pending_coverage.push(
                                            PendingCoverageLeaf {
                                                owner_key,
                                                old_ref,
                                                old_missing: missing_ref,
                                                next: owner_coverage,
                                                missing: cached.is_none(),
                                            },
                                            context.budget,
                                        )?;
                                        if pending_coverage.len() == COVERAGE_LEAF_BATCH {
                                            flush_coverage_leaves(
                                                &mut pending_coverage,
                                                edits,
                                                context,
                                            )?;
                                        }
                                        continue;
                                    }
                                    None => None,
                                };
                                if old_ref != next_ref {
                                    let edit = PreparedEdit::values(
                                        &owner_key,
                                        old_ref,
                                        next_ref,
                                        context.budget,
                                    )?;
                                    edits.push(0, edit.as_edit(), context.budget)?;
                                }
                                let next_missing = if next_ref == old_ref {
                                    missing_ref
                                } else {
                                    None
                                };
                                if missing_ref != next_missing {
                                    let edit = PreparedEdit::values(
                                        &owner_key,
                                        missing_ref,
                                        next_missing,
                                        context.budget,
                                    )?;
                                    edits.push(1, edit.as_edit(), context.budget)?;
                                }
                            }
                            Ok(())
                        })();
                        scan.finish(context.budget, scanned)?;
                        let map_stage = diagnostic::WorkStage::begin(
                            context.budget,
                            DiagnosticStageV1::NativeCoverageMapApply,
                        );
                        let mapped = (|| -> Result<(MapRoot, MapRoot), IndexError> {
                            Ok(if let Some(edits) = edits {
                                flush_coverage_leaves(&mut pending_coverage, &edits, context)?;
                                if pending_coverage.shared_pending_roots {
                                    let roots = context
                                        .apply_stream(&[prior_owners], &mut edits.cursor())?;
                                    let [root]: [MapRoot; 1] =
                                        roots.as_slice().try_into().map_err(|_roots| {
                                            IndexError::BackendViolation(
                                                "source coverage map stream lost shared root"
                                                    .into(),
                                            )
                                        })?;
                                    (root, root)
                                } else {
                                    let roots = context.apply_stream(
                                        &[prior_owners, prior_missing],
                                        &mut edits.cursor(),
                                    )?;
                                    let roots: [MapRoot; 2] =
                                        roots.as_slice().try_into().map_err(|_roots| {
                                            IndexError::BackendViolation(
                                                "source coverage map stream lost roots".into(),
                                            )
                                        })?;
                                    roots.into()
                                }
                            } else {
                                // Reaching cursor EOF verified the sealed patch. No owner
                                // map edits exist, but the desired source context still
                                // changes and must be published below.
                                (prior_owners, prior_missing)
                            })
                        })();
                        let (owners, missing) = map_stage.finish(context.budget, mapped)?;
                        let status = if missing.0.is_some() {
                            CoverageStatus::Partial { missing }
                        } else {
                            CoverageStatus::Complete
                        };
                        let search_stage = diagnostic::WorkStage::begin(
                            context.budget,
                            DiagnosticStageV1::NativeSearchablePrepare,
                        );
                        let prepared = self
                            .registry
                            .prepare_searchable(
                                spec,
                                prior_coverage.as_ref(),
                                owners,
                                None,
                                self.services.object_maps.as_ref(),
                                context,
                            )?
                            .await;
                        let searchable = search_stage.finish(context.budget, prepared)?;
                        let binding = CoverageBinding {
                            instance,
                            desired,
                            covered: if missing.0.is_some() {
                                None
                            } else {
                                Some(desired)
                            },
                            searchable: searchable.reference(),
                            status,
                            owners,
                        };
                        let save_stage = diagnostic::WorkStage::begin(
                            context.budget,
                            DiagnosticStageV1::NativeCoverageSave,
                        );
                        let saved = save_prepared_coverage(&binding, searchable, context);
                        Some(save_stage.finish(context.budget, saved)?)
                    }
                    _ => None,
                }
            };
            if covered != next_coverage {
                coverage_edits.push(&coverage_key, covered, next_coverage, context.budget)?;
            }
        }
        let assembly =
            diagnostic::WorkStage::begin(context.budget, DiagnosticStageV1::NativeMemberAssemble);
        let assembled = (|| {
            // Sorted unique member keys are required by the persistent map port.
            work_batch.flush(&mut work, context.scratch, context.budget)?;
            edits.sort();
            coverage_edits.sort();
            let members = context.apply(base.members, edits.as_slice())?;
            let coverage = context.apply(base.coverage, coverage_edits.as_slice())?;
            Ok((members, coverage, work))
        })();
        assembly.finish(context.budget, assembled)
    }

    async fn projection_advance(
        &self,
        attempt: AttemptRef,
        base: &CatalogView,
        transition: &ProjectionTransition,
        context: &mut AttemptContext<'_>,
    ) -> Result<OperationStatus, IndexError> {
        let profile = self.load_profile(base.profile, context)?;
        for spec in &profile.instances {
            if spec.activation != GroupActivation::AsyncCoverage
                || !matches!(spec.input_domain, InputDomain::Projection { projection, producer }
                    if projection == transition.projection && producer == transition.producer)
            {
                continue;
            }
            let key = member_key(spec.instance);
            let reference = context
                .maps
                .lookup_object(base.coverage, &key, context.budget)?
                .ok_or_else(|| {
                    IndexError::CorruptObject("projection instance coverage missing".into())
                })?;
            let prior = load_coverage(reference, spec.instance, context)?;
            self.registry.validate_coverage(spec, &prior)?;
        }
        let delta = projection::apply_projection(
            attempt.operation,
            base.projections,
            base.source,
            transition,
            context,
        )?;
        let mut coverage_edits = MemberEdits::new(profile.instances.len(), context.budget)?;
        let mut member_edits = MemberEdits::new(profile.instances.len(), context.budget)?;
        let mut work = None;
        let mut work_batch = WorkActionBatch::default();
        for spec in &profile.instances {
            let InputDomain::Projection {
                projection: projection_id,
                producer,
            } = spec.input_domain
            else {
                continue;
            };
            if projection_id != transition.projection {
                continue;
            }
            if producer != transition.producer {
                return Err(IndexError::InvalidInput(
                    "projection instance producer mismatch".into(),
                ));
            }
            if spec.activation == GroupActivation::CoreRequired {
                let key = member_key(spec.instance);
                let old_ref = context
                    .maps
                    .lookup_object(base.members, &key, context.budget)?
                    .ok_or_else(|| {
                        IndexError::CorruptObject("required projection member missing".into())
                    })?;
                let old = load_member(old_ref, spec.instance, context)?;
                let previous = transition.expected.ok_or_else(|| {
                    IndexError::CorruptObject("required projection prior stamp missing".into())
                })?;
                let expected = InputBinding {
                    root: previous.owners,
                    schema: spec.schema,
                    policy: spec.policy,
                    context: previous.materialization_context(),
                };
                if old.input != expected
                    || old.format != spec.format
                    || delta.next.schema != spec.schema
                {
                    return Err(IndexError::CorruptObject(
                        "required projection prior member mismatch".into(),
                    ));
                }
                let binding = self
                    .registry
                    .build_projection(spec, Some(old), &delta.next, Some(delta.changed), context)?
                    .await?;
                let next_ref = stage_required_member(&binding, context)?;
                if old_ref != next_ref {
                    member_edits.push(&key, Some(old_ref), Some(next_ref), context.budget)?;
                }
                continue;
            }
            if spec.activation != GroupActivation::AsyncCoverage {
                return Err(IndexError::InvalidInput(
                    "projection instance producer or activation mismatch".into(),
                ));
            }
            let key = member_key(spec.instance);
            let old_ref = context
                .maps
                .lookup_object(base.coverage, &key, context.budget)?;
            let old_ref = old_ref.ok_or_else(|| {
                IndexError::CorruptObject("projection instance coverage missing".into())
            })?;
            let prior = load_coverage(old_ref, spec.instance, context)?;
            let prior_missing = match prior.status {
                CoverageStatus::Complete => MapRoot(None),
                CoverageStatus::Partial { missing } => missing,
                CoverageStatus::Pending { previous_missing } => previous_missing,
            };
            let desired = InputBinding {
                root: delta.next.owners,
                schema: spec.schema,
                policy: spec.policy,
                context: projection::projection_context(&delta.next),
            };
            let edits = EditSpool::open(context.scratch, context.budget)?;
            let mut pending_coverage = PendingCoverageBatch::with_known_count(delta.changed.count)?;
            let mut changes =
                OwnerMutationCursor::new(delta.changed, context.maps, context.objects);
            while let Some(change) = projection::next_change(&mut changes, &delta, context)? {
                let owner_key = owner_map_key(change.owner);
                let prior_ref = context
                    .maps
                    .lookup(prior.owners, &owner_key, context.budget)?;
                let old = prior_ref
                    .map(|reference| load_owner_coverage(reference, change.owner, context))
                    .transpose()?;
                if change.prior.is_some() && old.is_none() {
                    return Err(IndexError::RebuildRequired);
                }
                if old.is_some_and(|previous| previous.desired.projection != change.prior)
                    || (change.prior.is_none() && old.is_some())
                {
                    return Err(IndexError::CorruptObject(
                        "projection coverage disagrees with prior owner root".into(),
                    ));
                }
                let old_missing = context
                    .maps
                    .lookup(prior_missing, &owner_key, context.budget)?;
                if old_missing.is_some_and(|reference| Some(reference) != prior_ref) {
                    return Err(IndexError::CorruptObject(
                        "projection missing owner marker mismatch".into(),
                    ));
                }
                let next_input = change
                    .next
                    .map(|(reference, contribution)| OwnerInputBinding {
                        source: contribution.source,
                        projection: Some(reference),
                        producer_context: projection::owner_context(contribution),
                        derivation_policy: spec.policy,
                    });
                if let Some(previous) = old
                    && previous.covered != Some(previous.desired)
                    && Some(previous.desired) != next_input
                {
                    work_batch.push(
                        &WorkAction::Retire(owner_work(
                            base,
                            spec.instance,
                            change.owner,
                            previous.desired,
                        )),
                        &mut work,
                        context.scratch,
                        context.budget,
                    )?;
                }
                let next_ref = match (next_input, change.next) {
                    (Some(input), Some((_, contribution)))
                        if old.is_some_and(|previous| previous.desired == input)
                            && (contribution.output.is_some()
                                || old.is_some_and(|previous| previous.covered == Some(input))) =>
                    {
                        prior_ref
                    }
                    (Some(input), Some((_, contribution))) => {
                        let cached = old.filter(|previous| previous.covered == Some(input));
                        let empty = contribution.output.is_none();
                        let previous_searchable = if !empty
                            && cached.is_none()
                            && old.is_some_and(|previous| {
                                previous.covered == Some(previous.desired)
                                    && previous.searchable.is_some()
                                    && previous.desired.projection == change.prior
                                    && previous.desired.derivation_policy == spec.policy
                            })
                            && let Some(prior_ref) = change.prior
                        {
                            let prior_contribution =
                                projection::read_contribution(prior_ref, context)?;
                            old.filter(|previous| {
                                prior_contribution.owner == change.owner
                                    && prior_contribution.projection == projection_id
                                    && prior_contribution.producer == producer
                                    && prior_contribution.source == previous.desired.source
                                    && projection::owner_context(prior_contribution)
                                        == previous.desired.producer_context
                            })
                        } else {
                            None
                        };
                        let owner_coverage = OwnerCoverage {
                            owner: change.owner,
                            desired: input,
                            covered: if empty {
                                Some(input)
                            } else {
                                cached.or(previous_searchable).and_then(|v| v.covered)
                            },
                            searchable: if empty {
                                None
                            } else {
                                cached.or(previous_searchable).and_then(|v| v.searchable)
                            },
                        };
                        if !empty && cached.is_none() {
                            work_batch.push(
                                &WorkAction::Create(owner_work(
                                    base,
                                    spec.instance,
                                    change.owner,
                                    input,
                                )),
                                &mut work,
                                context.scratch,
                                context.budget,
                            )?;
                        }
                        let covered = empty
                            || old.is_some_and(|previous| {
                                next_input == previous.covered && previous.searchable.is_some()
                            });
                        pending_coverage.push(
                            PendingCoverageLeaf {
                                owner_key,
                                old_ref: prior_ref,
                                old_missing,
                                next: owner_coverage,
                                missing: !covered,
                            },
                            context.budget,
                        )?;
                        if pending_coverage.len() == COVERAGE_LEAF_BATCH {
                            flush_coverage_leaves(&mut pending_coverage, &edits, context)?;
                        }
                        continue;
                    }
                    (None, None) => None,
                    _ => return Err(IndexError::BackendViolation("projection input".into())),
                };
                if prior_ref != next_ref {
                    let edit =
                        PreparedEdit::values(&owner_key, prior_ref, next_ref, context.budget)?;
                    edits.push(0, edit.as_edit(), context.budget)?;
                }
                let next_missing = next_ref.filter(|reference| {
                    let covered = if Some(*reference) == prior_ref {
                        old.is_some_and(|previous| previous.covered == Some(previous.desired))
                    } else {
                        let contribution = change.next.map(|(_, value)| value);
                        contribution.is_some_and(|value| value.output.is_none())
                            || old.is_some_and(|previous| {
                                next_input == previous.covered && previous.searchable.is_some()
                            })
                    };
                    !covered
                });
                if old_missing != next_missing {
                    let edit = PreparedEdit::values(
                        &owner_key,
                        old_missing,
                        next_missing,
                        context.budget,
                    )?;
                    edits.push(1, edit.as_edit(), context.budget)?;
                }
            }
            flush_coverage_leaves(&mut pending_coverage, &edits, context)?;
            let roots =
                context.apply_stream(&[prior.owners, prior_missing], &mut edits.cursor())?;
            let [owners, missing]: [MapRoot; 2] =
                roots.as_slice().try_into().map_err(|_roots| {
                    IndexError::BackendViolation("projection coverage map stream lost roots".into())
                })?;
            let status = if missing.0.is_some() {
                CoverageStatus::Partial { missing }
            } else {
                CoverageStatus::Complete
            };
            let searchable = self
                .registry
                .prepare_searchable(
                    spec,
                    Some(&prior),
                    owners,
                    None,
                    self.services.object_maps.as_ref(),
                    context,
                )?
                .await?;
            let next_coverage = CoverageBinding {
                instance: spec.instance,
                desired,
                covered: if missing.0.is_some() {
                    None
                } else {
                    Some(desired)
                },
                searchable: searchable.reference(),
                status,
                owners,
            };
            let next_ref = save_prepared_coverage(&next_coverage, searchable, context)?;
            coverage_edits.push(&key, Some(old_ref), Some(next_ref), context.budget)?;
        }
        work_batch.flush(&mut work, context.scratch, context.budget)?;
        member_edits.sort();
        let members = if member_edits.is_empty() {
            base.members
        } else {
            context.apply(base.members, member_edits.as_slice())?
        };
        coverage_edits.sort();
        let coverage = if coverage_edits.is_empty() {
            base.coverage
        } else {
            context.apply(base.coverage, coverage_edits.as_slice())?
        };
        let sequence = base
            .identity
            .sequence
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let mut next = CatalogView {
            identity: ViewRef {
                sequence,
                digest: Digest([0; 32]),
                ..base.identity
            },
            profile: base.profile,
            source: base.source,
            projections: delta.root,
            members,
            coverage,
        };
        next.identity = identity::view_identity(&next)?;
        let (work_actions, mut actions) =
            WorkSpool::seal_lazy(work, context.objects, context.budget)?;
        let mut projection_events = PreparedEvents::copy(&transition.events, context.budget)?;
        self.services.catalog.commit_transition(
            PreparedPublication {
                attempt,
                source_mode: SourceOperationMode::Unchanged,
                expected: base.identity,
                next,
                work_actions,
                complete_work: None,
                source_events: Vec::new(),
                projection_events: projection_events.take(),
                expected_source: None,
            },
            &mut actions,
            context.budget,
        )
    }

    async fn materialize(
        &self,
        attempt: AttemptRef,
        base: &CatalogView,
        claim: &WorkClaim,
        context: &mut AttemptContext<'_>,
    ) -> Result<OperationStatus, IndexError> {
        if claim.intent.workspace != base.identity.workspace
            || claim.intent.epoch != base.identity.epoch
        {
            return Err(IndexError::Conflict);
        }
        let WorkScope::Owner { owner, expected } = claim.intent.scope else {
            return Err(IndexError::InvalidInput(
                "projection-domain materialization is unsupported".into(),
            ));
        };
        let profile = self.load_profile(base.profile, context)?;
        let spec = profile
            .instances
            .iter()
            .find(|spec| spec.instance == claim.intent.instance)
            .ok_or(IndexError::Conflict)?;
        if spec.activation != GroupActivation::AsyncCoverage
            || spec.policy != expected.derivation_policy
        {
            return Err(IndexError::Conflict);
        }
        match spec.input_domain {
            InputDomain::SourceLocal => {
                if expected.projection.is_some() || expected.source.is_none() {
                    return Err(IndexError::Conflict);
                }
                let actual = context.maps.lookup_object(
                    base.source.owners,
                    &owner_map_key(owner),
                    context.budget,
                )?;
                if actual != expected.source {
                    return Err(IndexError::Conflict);
                }
            }
            InputDomain::Projection {
                projection: projection_id,
                producer,
            } => {
                let (_, binding) =
                    projection::lookup_binding(base.projections, projection_id, context)?
                        .ok_or(IndexError::Conflict)?;
                if binding.producer != producer || binding.source != base.source {
                    return Err(IndexError::Conflict);
                }
                let actual = context.maps.lookup_object(
                    binding.owners,
                    &owner_map_key(owner),
                    context.budget,
                )?;
                if actual != expected.projection {
                    return Err(IndexError::Conflict);
                }
                let contribution = actual
                    .map(|reference| projection::read_contribution(reference, context))
                    .transpose()?
                    .ok_or(IndexError::Conflict)?;
                if contribution.owner != owner
                    || contribution.projection != projection_id
                    || contribution.producer != producer
                    || contribution.source != expected.source
                    || projection::owner_context(contribution) != expected.producer_context
                {
                    return Err(IndexError::Conflict);
                }
                if let (Some(source_owner), Some(source_reference)) =
                    (contribution.source_owner, contribution.source)
                {
                    let actual = context.maps.lookup_object(
                        base.source.owners,
                        &owner_map_key(source_owner),
                        context.budget,
                    )?;
                    if actual != Some(source_reference) {
                        return Err(IndexError::Conflict);
                    }
                }
            }
        }
        let coverage_key = member_key(spec.instance);
        let coverage_ref = context
            .maps
            .lookup_object(base.coverage, &coverage_key, context.budget)?
            .ok_or_else(|| IndexError::CorruptObject("async coverage missing".into()))?;
        let coverage = load_coverage(coverage_ref, spec.instance, context)?;
        self.registry.validate_coverage(spec, &coverage)?;
        if coverage.desired.schema != spec.schema || coverage.desired.policy != spec.policy {
            return Err(IndexError::Conflict);
        }
        if matches!(spec.input_domain, InputDomain::SourceLocal)
            && (coverage.desired.root != base.source.owners
                || coverage.desired.context != identity::source_context(base.source)?)
        {
            return Err(IndexError::Conflict);
        }
        if let InputDomain::Projection {
            projection: projection_id,
            ..
        } = spec.input_domain
        {
            let (_, binding) =
                projection::lookup_binding(base.projections, projection_id, context)?
                    .ok_or(IndexError::Conflict)?;
            let target = InputBinding {
                root: binding.owners,
                schema: spec.schema,
                policy: spec.policy,
                context: projection::projection_context(&binding),
            };
            if binding.source != base.source || coverage.desired != target {
                return Err(IndexError::Conflict);
            }
        }
        let owner_key = owner_map_key(owner);
        let owner_ref = context
            .maps
            .lookup(coverage.owners, &owner_key, context.budget)?
            .ok_or(IndexError::Conflict)?;
        let owner_coverage = load_owner_coverage(owner_ref, owner, context)?;
        if owner_coverage.desired != expected {
            return Err(IndexError::Conflict);
        }
        let old_missing = match coverage.status {
            CoverageStatus::Partial { missing } => {
                context.maps.lookup(missing, &owner_key, context.budget)?
            }
            CoverageStatus::Pending { .. } | CoverageStatus::Complete => None,
        };
        if old_missing != Some(owner_ref) {
            return Err(IndexError::Conflict);
        }
        let reuse_span =
            diagnostic::begin_sqlite_call(context.budget, quanta_index_core::DiagnosticLaneV1(23));
        let prepared = diagnostic::finish(
            reuse_span,
            diagnostic::catalog_call(self.services.catalog.as_ref(), context.budget, || {
                self.services.catalog.prepared_work(claim)
            }),
        )?;
        let (binding, newly_prepared) = if let Some(binding) = prepared {
            (binding, false)
        } else {
            let prior = match (owner_coverage.covered, owner_coverage.searchable) {
                (Some(covered), Some(reference)) if covered != expected => {
                    if covered.derivation_policy == spec.policy {
                        Some(load_prior_owner_member(
                            reference, spec, owner, covered, context,
                        )?)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            let build_span = diagnostic::begin(context.budget, DiagnosticStageV1::BuildPrepare);
            let built = {
                let mut phase_budget =
                    diagnostic::phase_budget(context.budget, build_span.as_ref());
                let mut phase_context = AttemptContext {
                    attempt: context.attempt,
                    objects: context.objects,
                    maps: context.maps,
                    custody: context.custody,
                    scratch: context.scratch,
                    budget: &mut phase_budget,
                };
                match self
                    .registry
                    .build_owner(spec, owner, expected, prior, &mut phase_context)
                {
                    Ok(future) => future.await,
                    Err(error) => Err(error),
                }
            };
            let binding = diagnostic::finish_work(
                context.budget,
                DiagnosticStageV1::BuildPrepare,
                build_span,
                built,
            )?;
            (binding, true)
        };
        crate::registry::validate_owner_member(&binding, spec, owner, expected)?;
        let artifact = context.objects.get(binding.artifact, context.budget)?;
        if artifact.registration.reference != binding.artifact {
            return Err(IndexError::BackendViolation(
                "owner adapter artifact reference mismatch".into(),
            ));
        }
        // The original authenticated artifact remains live through this
        // materialization. Reuse only this selected DATA under the same pin;
        // all other identities still use the original object port. No table or
        // additional memory allowance is born at this boundary.
        let selected_reads = crate::attempt_reads::AttemptObjectReads::selected(
            self.services.objects.as_ref(),
            &artifact,
        );
        let mut selected_context = AttemptContext {
            attempt: context.attempt,
            objects: &selected_reads,
            maps: context.maps,
            custody: context.custody,
            scratch: context.scratch,
            budget: &mut *context.budget,
        };
        let context = &mut selected_context;
        if newly_prepared {
            let stage_span = diagnostic::begin_sqlite_call(
                context.budget,
                quanta_index_core::DiagnosticLaneV1(24),
            );
            diagnostic::finish(
                stage_span,
                diagnostic::catalog_call(self.services.catalog.as_ref(), context.budget, || {
                    self.services.catalog.stage_work(claim, binding.clone())
                }),
            )?;
        }
        // Finalize observes owner metadata preparation and the paired coverage
        // map edit only; searchable preparation and publication have their own
        // stages below.
        let finalize_span = diagnostic::begin(context.budget, DiagnosticStageV1::Finalize);
        let owner_metadata = {
            let mut phase_budget = diagnostic::phase_budget(context.budget, finalize_span.as_ref());
            let mut phase_context = AttemptContext {
                attempt: context.attempt,
                objects: context.objects,
                maps: context.maps,
                custody: context.custody,
                scratch: context.scratch,
                budget: &mut phase_budget,
            };
            let context = &mut phase_context;
            (|| -> Result<MaterializedOwnerMaps, IndexError> {
                let (binding_ref, next_owner_ref, inputs) =
                    prepare_materialized_owner_metadata(&binding, owner, expected, context)?;
                let CoverageStatus::Partial {
                    missing: old_missing_root,
                } = coverage.status
                else {
                    return Err(IndexError::Conflict);
                };
                // Both edits target independent immutable roots. Hold their original
                // key leases until the single staged map transaction has consumed both
                // original-base preconditions and returned both roots in ordinal order.
                let owner_edit = PreparedEdit::values(
                    &owner_key,
                    Some(owner_ref),
                    Some(next_owner_ref.into()),
                    context.budget,
                )?;
                let missing_edit =
                    PreparedEdit::values(&owner_key, Some(owner_ref), None, context.budget)?;
                let bases = [coverage.owners, old_missing_root];
                let edits = [owner_edit.as_edit(), missing_edit.as_edit()];
                let scope = self
                    .services
                    .object_maps
                    .as_ref()
                    .map(|writer| {
                        Arc::clone(writer).prepare_owner_maps(
                            inputs.inputs(),
                            &bases,
                            &edits,
                            context.attempt,
                            context.budget,
                        )
                    })
                    .transpose()?
                    .flatten();
                if let Some(scope) = scope {
                    if scope.references() != [binding_ref, next_owner_ref] {
                        return Err(IndexError::BackendViolation(
                            "materialize prepared owner metadata changed identity/order".into(),
                        ));
                    }
                    let [owners, missing] = scope.roots() else {
                        return Err(IndexError::BackendViolation(
                            "materialize prepared owner maps lost ordinal roots".into(),
                        ));
                    };
                    Ok(MaterializedOwnerMaps {
                        binding_ref,
                        owners: *owners,
                        missing: *missing,
                        scope: Some(scope),
                    })
                } else {
                    // Only unsupported linked backends use the original install.
                    // A failed preparation never retries through another path.
                    let roots = install_metadata_maps(
                        self.services.object_maps.as_deref(),
                        &inputs,
                        &[binding_ref, next_owner_ref],
                        &bases,
                        &edits,
                        context,
                    )?;
                    let [owners, missing]: [MapRoot; 2] =
                        roots.as_slice().try_into().map_err(|_roots| {
                            IndexError::BackendViolation(
                                "materialize coverage map stream lost roots".into(),
                            )
                        })?;
                    Ok(MaterializedOwnerMaps {
                        binding_ref,
                        owners,
                        missing,
                        scope: None,
                    })
                }
            })()
        };
        let MaterializedOwnerMaps {
            binding_ref,
            owners,
            missing,
            scope: owner_scope,
        } = diagnostic::finish_work(
            context.budget,
            DiagnosticStageV1::Finalize,
            finalize_span,
            owner_metadata,
        )?;
        let status = if missing.0.is_some() {
            CoverageStatus::Partial { missing }
        } else {
            CoverageStatus::Complete
        };
        let scoped_objects = owner_scope.as_ref().map(|scope| {
            crate::attempt_reads::AttemptObjectReads::selected(scope.objects(), &artifact)
        });
        let semantic_objects: &dyn ImmutableObjectPort = match scoped_objects.as_ref() {
            Some(objects) => objects,
            None => context.objects,
        };
        let semantic_maps: &dyn PersistentMapPort = match owner_scope.as_ref() {
            Some(scope) => scope.maps(),
            None => context.maps,
        };
        let scoped_writer = owner_scope.as_ref().map(|scope| Arc::clone(scope).writer());
        let semantic_writer = scoped_writer
            .as_ref()
            .or(self.services.object_maps.as_ref());
        let prepare_span = diagnostic::begin(context.budget, DiagnosticStageV1::SemanticPrepare);
        let prepared = {
            let mut phase_budget = diagnostic::phase_budget(context.budget, prepare_span.as_ref());
            let mut phase_context = AttemptContext {
                attempt: context.attempt,
                objects: semantic_objects,
                maps: semantic_maps,
                custody: context.custody,
                scratch: context.scratch,
                budget: &mut phase_budget,
            };
            match self.registry.prepare_searchable(
                spec,
                Some(&coverage),
                owners,
                Some((owner, binding_ref)),
                semantic_writer,
                &mut phase_context,
            ) {
                Ok(future) => future.await,
                Err(error) => Err(error),
            }
        };
        let mut searchable = diagnostic::finish_work(
            context.budget,
            DiagnosticStageV1::SemanticPrepare,
            prepare_span,
            prepared,
        )?;
        if let Some(scope) = owner_scope.as_ref() {
            use quanta_index_core::index::ports::OwnerMapCompletion;
            let installed = match scope.take_prepared(context.attempt, context.budget)? {
                OwnerMapCompletion::Prepared(owner_maps) => {
                    searchable = searchable.with_owner_maps(owner_maps, context)?;
                    false
                }
                OwnerMapCompletion::Transferred => false,
                OwnerMapCompletion::Installed => true,
            };
            searchable.validate_owner_maps(scope.references(), scope.roots(), installed)?;
        }
        let next_coverage = CoverageBinding {
            instance: coverage.instance,
            desired: coverage.desired,
            covered: if missing.0.is_some() {
                None
            } else {
                Some(coverage.desired)
            },
            searchable: searchable.reference(),
            status,
            owners,
        };
        let save_span = diagnostic::begin(context.budget, DiagnosticStageV1::NativeCoverageSave);
        let coverage_root = {
            let mut phase_budget = diagnostic::phase_budget(context.budget, save_span.as_ref());
            let mut phase_context = AttemptContext {
                attempt: context.attempt,
                objects: context.objects,
                maps: context.maps,
                custody: context.custody,
                scratch: context.scratch,
                budget: &mut phase_budget,
            };
            let context = &mut phase_context;
            (|| -> Result<MapRoot, IndexError> {
                if let Some(writer) = self.services.object_maps.as_deref() {
                    let payload = prepared_coverage_payload(&next_coverage, &searchable, context)?;
                    let parent = searchable.prepare_parent(
                        payload,
                        coverage_children(&next_coverage).as_slice(),
                        context,
                    )?;
                    let edit = PreparedEdit::new(
                        &coverage_key,
                        Some(coverage_ref),
                        Some(parent.reference()),
                        context.budget,
                    )?;
                    parent.install_with_map(Some(writer), base.coverage, edit.as_edit(), context)
                } else {
                    let next_coverage_ref =
                        save_prepared_coverage(&next_coverage, searchable, context)?;
                    let edit = PreparedEdit::new(
                        &coverage_key,
                        Some(coverage_ref),
                        Some(next_coverage_ref),
                        context.budget,
                    )?;
                    context.apply(base.coverage, edit.as_slice())
                }
            })()
        };
        let coverage_root = diagnostic::finish_work(
            context.budget,
            DiagnosticStageV1::NativeCoverageSave,
            save_span,
            coverage_root,
        )?;
        let sequence = base
            .identity
            .sequence
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let mut next = CatalogView {
            identity: ViewRef {
                sequence,
                digest: Digest([0; 32]),
                ..base.identity
            },
            profile: base.profile,
            source: base.source,
            projections: base.projections,
            members: base.members,
            coverage: coverage_root,
        };
        next.identity = identity::view_identity(&next)?;
        let (work_actions, mut actions) =
            WorkSpool::seal_lazy(None, context.objects, context.budget)?;
        let publish_span = diagnostic::begin(context.budget, DiagnosticStageV1::NativePublish);
        let published = self.services.catalog.commit_transition(
            PreparedPublication {
                attempt,
                source_mode: SourceOperationMode::Unchanged,
                expected: base.identity,
                next,
                work_actions,
                complete_work: Some(*claim),
                source_events: Vec::new(),
                projection_events: Vec::new(),
                expected_source: None,
            },
            &mut actions,
            context.budget,
        );
        if !matches!(&published, Ok(OperationStatus::Published(_))) {
            context
                .budget
                .record_failed_stage(DiagnosticStageV1::NativePublish);
        }
        diagnostic::finish_publication(publish_span, published)
    }
}

/// Exactly one edit per independent Materialize coverage root, ordered by
/// root ordinal. The staged map port checks both original bases before IO.
struct MaterializedOwnerMaps {
    binding_ref: ObjectRef,
    owners: MapRoot,
    missing: MapRoot,
    scope: Option<Arc<dyn quanta_index_core::index::ports::PreparedOwnerMapScope>>,
}

struct MetadataMapEdits<'a> {
    edits: &'a [&'a quanta_index_core::index::MapEdit],
    next: usize,
}

impl MapEditCursor for MetadataMapEdits<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        let Some(edit) = self.edits.get(self.next) else {
            return Ok(None);
        };
        let root = u32::try_from(self.next).map_err(|_width| IndexError::ResourceLimited)?;
        let copied = OrdinalMapEdit::copy_key(root, &edit.key, edit.expected, edit.next, budget)?;
        self.next = self
            .next
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(Some(copied))
    }
}

fn install_metadata_maps(
    writer: Option<&dyn ObjectMapBatchPort>,
    inputs: &crate::staging_payload::StagedObjectBatch,
    expected: &[ObjectRef],
    bases: &[MapRoot],
    edits: &[&quanta_index_core::index::MapEdit],
    context: &mut AttemptContext<'_>,
) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
    let (actual, roots) = if let Some(writer) = writer {
        context.put_and_apply(writer, inputs.inputs(), bases, edits)?
    } else {
        // This backend was composed without the linked installation facet.
        // Preserve its original object and map ports, never retry a bank error.
        let actual = context.put_batch(inputs.inputs())?;
        if &*actual != expected {
            return Err(IndexError::BackendViolation(
                "metadata batch identity/order mismatch".into(),
            ));
        }
        let roots = context.apply_stream(bases, &mut MetadataMapEdits { edits, next: 0 })?;
        (actual, roots)
    };
    if &*actual != expected {
        return Err(IndexError::BackendViolation(
            "metadata/map batch identity/order mismatch".into(),
        ));
    }
    Ok(roots)
}

fn owner_work(
    base: &CatalogView,
    instance: IndexInstanceId,
    owner: OwnerKey,
    expected: OwnerInputBinding,
) -> WorkIntent {
    WorkIntent {
        workspace: base.identity.workspace,
        epoch: base.identity.epoch,
        instance,
        scope: WorkScope::Owner { owner, expected },
    }
}

const COVERAGE_RETIRE_PAGE: u32 = 128;

fn retire_unfinished_coverage(
    base: &CatalogView,
    spec: &quanta_index_core::index::InstanceSpec,
    coverage: &CoverageBinding,
    work: &mut Option<WorkSpool>,
    work_batch: &mut WorkActionBatch,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let mut after: Option<quanta_index_core::index::MapPageEntry> = None;
    loop {
        context.budget.checkpoint()?;
        let page = context.maps.scan(
            coverage.owners,
            after
                .as_ref()
                .map(quanta_index_core::index::MapPageEntry::key)
                .transpose()?,
            COVERAGE_RETIRE_PAGE,
            context.budget,
        )?;
        if page.entries.len()
            > usize::try_from(COVERAGE_RETIRE_PAGE).map_err(|_error| IndexError::ResourceLimited)?
            || (page.entries.is_empty() && page.has_more)
        {
            return Err(IndexError::BackendViolation(
                "async coverage scan exceeded limit or did not advance".into(),
            ));
        }
        if page.entries.is_empty() {
            if after.is_some() {
                return Err(IndexError::BackendViolation(
                    "async coverage scan ended before its declared continuation".into(),
                ));
            }
            return Ok(());
        }
        for entry in page.clone().cursor() {
            let (key, reference) = entry.get()?;
            let reference = *reference;
            if after
                .as_ref()
                .map(quanta_index_core::index::MapPageEntry::key)
                .transpose()?
                .is_some_and(|previous| key.as_slice() <= previous)
            {
                return Err(IndexError::CorruptObject(
                    "async coverage owner keys are not strictly ordered".into(),
                ));
            }
            let binding = quanta_index_core::index::read_owner_coverage(
                reference,
                key,
                context.objects,
                context.budget,
            )?;
            if binding.desired.derivation_policy != spec.policy {
                return Err(IndexError::CorruptObject(
                    "async coverage owner policy mismatch".into(),
                ));
            }
            if binding.covered != Some(binding.desired) {
                work_batch.push(
                    &WorkAction::Retire(owner_work(
                        base,
                        spec.instance,
                        binding.owner,
                        binding.desired,
                    )),
                    work,
                    context.scratch,
                    context.budget,
                )?;
            }
            after = Some(entry);
        }
        if !page.has_more {
            return Ok(());
        }
    }
}

/// Common immutable profile decoding for build and read composition.
fn load_profile_object(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<LoadedProfile, IndexError> {
    crate::profile::load(reference, objects, budget)
}

fn load_coverage(
    reference: ObjectRef,
    instance: IndexInstanceId,
    context: &mut AttemptContext<'_>,
) -> Result<CoverageBinding, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "coverage reference mismatch".into(),
        ));
    }
    let binding =
        quanta_index_core::index::decode_coverage_binding(&stored.payload, context.budget)?;
    if binding.instance != instance
        || stored.registration.children.as_slice() != coverage_children(&binding).as_slice()
    {
        return Err(IndexError::CorruptObject(
            "coverage identity/edges mismatch".into(),
        ));
    }
    Ok(binding)
}

fn prepared_coverage_payload(
    binding: &CoverageBinding,
    searchable: &quanta_index_core::index::SearchablePreparation,
    context: &AttemptContext<'_>,
) -> Result<crate::staging_payload::StagedPayload, IndexError> {
    if binding.searchable != searchable.reference() {
        return Err(IndexError::BackendViolation(
            "coverage preparation identity differs".into(),
        ));
    }
    binding_metadata::encode_coverage(binding, context.budget)
}

fn save_prepared_coverage(
    binding: &CoverageBinding,
    searchable: quanta_index_core::index::SearchablePreparation,
    context: &mut AttemptContext<'_>,
) -> Result<ObjectRef, IndexError> {
    let result = (|| {
        let payload = prepared_coverage_payload(binding, &searchable, context)?;
        searchable.install_parent(payload, coverage_children(binding).as_slice(), context)
    })();
    diagnostic::finish_work(context.budget, DiagnosticStageV1::Finalize, None, result)
}

fn save_coverage(
    binding: &CoverageBinding,
    context: &mut AttemptContext<'_>,
) -> Result<ObjectRef, IndexError> {
    let payload = binding_metadata::encode_coverage(binding, context.budget)?;
    context.put(&payload.bytes, coverage_children(binding).as_slice())
}

fn load_prior_owner_member(
    reference: ObjectRef,
    spec: &quanta_index_core::index::InstanceSpec,
    owner: OwnerKey,
    covered: OwnerInputBinding,
    context: &mut AttemptContext<'_>,
) -> Result<OwnerMemberBinding, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "prior owner member reference mismatch".into(),
        ));
    }
    let binding =
        quanta_index_core::index::decode_owner_member_binding(&stored.payload, context.budget)?;
    if binding.instance != spec.instance
        || binding.owner != owner
        || binding.input != covered
        || binding.input.derivation_policy != spec.policy
        || binding.format != spec.format
    {
        return Err(IndexError::CorruptObject(
            "prior owner member identity/input mismatch".into(),
        ));
    }
    let children = owner_member_children(&binding);
    if stored.registration.children.as_slice() != children.as_slice() {
        return Err(IndexError::CorruptObject(
            "prior owner member edges mismatch".into(),
        ));
    }
    for child in children.as_slice() {
        let registered = context.objects.get(*child, context.budget)?;
        if registered.registration.reference != *child {
            return Err(IndexError::CorruptObject(
                "prior owner member child reference mismatch".into(),
            ));
        }
    }
    Ok(binding)
}

fn stage_required_member(
    binding: &MemberBinding,
    context: &mut AttemptContext<'_>,
) -> Result<ObjectRef, IndexError> {
    let artifact = context.objects.get(binding.artifact, context.budget)?;
    if artifact.registration.reference != binding.artifact {
        return Err(IndexError::BackendViolation(
            "sealed artifact reference mismatch".into(),
        ));
    }
    let children = binding_metadata::member_children(binding);
    let payload = binding_metadata::encode_member(binding, context.budget)?;
    context.put(&payload.bytes, children.as_slice())
}

fn load_owner_coverage(
    value: MapValue,
    owner: OwnerKey,
    context: &mut AttemptContext<'_>,
) -> Result<OwnerCoverage, IndexError> {
    quanta_index_core::index::read_owner_coverage(
        value,
        &owner_map_key(owner),
        context.objects,
        context.budget,
    )
}

fn prepare_materialized_owner_metadata(
    binding: &OwnerMemberBinding,
    owner: OwnerKey,
    expected: OwnerInputBinding,
    context: &mut AttemptContext<'_>,
) -> Result<
    (
        ObjectRef,
        ObjectRef,
        crate::staging_payload::StagedObjectBatch,
    ),
    IndexError,
> {
    let mut inputs = crate::staging_payload::StagedObjectBatch::new(2, context.budget)?;
    let binding_children = owner_member_children(binding);
    let binding_payload = binding_metadata::encode_owner_member(binding, context.budget)?;
    let binding_ref = context
        .objects
        .identify(
            &binding_payload.bytes,
            binding_children.as_slice(),
            context.budget,
        )?
        .reference;
    inputs.push(binding_payload, binding_children.as_slice(), context.budget)?;

    let next_owner = OwnerCoverage {
        owner,
        desired: expected,
        covered: Some(expected),
        searchable: Some(binding_ref),
    };
    if quanta_index_core::index::pending_owner_coverage_value(&next_owner)?.is_some() {
        return Err(IndexError::BackendViolation(
            "materialized owner coverage unexpectedly inlined".into(),
        ));
    }
    let owner_children = owner_coverage_children(&next_owner);
    let owner_payload = binding_metadata::encode_owner_coverage(&next_owner, context.budget)?;
    let owner_ref = context
        .objects
        .identify(
            &owner_payload.bytes,
            owner_children.as_slice(),
            context.budget,
        )?
        .reference;
    inputs.push(owner_payload, owner_children.as_slice(), context.budget)?;

    Ok((binding_ref, owner_ref, inputs))
}

const COVERAGE_LEAF_BATCH: usize = 64;
const COVERAGE_LEAF_BATCH_BYTES: u64 = 8 * 1024 * 1024;
// OwnerCoverage contains only fixed-width identities and bindings.

struct PendingCoverageLeaf {
    owner_key: [u8; 38],
    old_ref: Option<MapValue>,
    old_missing: Option<MapValue>,
    next: OwnerCoverage,
    missing: bool,
}

struct PendingCoverageBatch {
    leaves: Vec<PendingCoverageLeaf>,
    // Leaf backing drops before its preparation reservation.
    memory: Option<quanta_index_core::index::MemoryLease>,
    shared_pending_roots: bool,
    reservation_slots: usize,
}

impl Default for PendingCoverageBatch {
    fn default() -> Self {
        Self {
            leaves: Vec::new(),
            memory: None,
            shared_pending_roots: false,
            reservation_slots: COVERAGE_LEAF_BATCH,
        }
    }
}

impl PendingCoverageBatch {
    fn with_known_count(count: u64) -> Result<Self, IndexError> {
        let slots = usize::try_from(count.min(
            u64::try_from(COVERAGE_LEAF_BATCH).map_err(|_error| IndexError::ResourceLimited)?,
        ))
        .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            reservation_slots: slots,
            ..Self::default()
        })
    }

    fn push(&mut self, leaf: PendingCoverageLeaf, budget: &WorkBudget) -> Result<(), IndexError> {
        if self.shared_pending_roots
            && (leaf.old_ref.is_some()
                || leaf.old_missing.is_some()
                || !leaf.missing
                || leaf.next.covered.is_some()
                || leaf.next.searchable.is_some())
        {
            return Err(IndexError::BackendViolation(
                "shared pending coverage requires fresh uncovered owners".into(),
            ));
        }
        // A sealed count may reserve fewer slots; never let Vec grow beyond its
        // original memory lease if a caller unexpectedly produces more rows.
        if self.leaves.len() >= self.reservation_slots {
            return Err(IndexError::ResourceLimited);
        }
        if self.leaves.capacity() == 0 {
            let bytes = self
                .reservation_slots
                .checked_mul(std::mem::size_of::<PendingCoverageLeaf>())
                .ok_or(IndexError::ResourceLimited)?;
            let memory = budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )?;
            self.leaves
                .try_reserve_exact(self.reservation_slots)
                .map_err(|_error| IndexError::ResourceLimited)?;
            self.memory = memory;
        }
        budget.checkpoint()?;
        self.leaves.push(leaf);
        Ok(())
    }

    fn len(&self) -> usize {
        self.leaves.len()
    }
}

fn flush_coverage_leaves(
    pending: &mut PendingCoverageBatch,
    edits: &EditSpool,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if pending.leaves.is_empty() {
        return Ok(());
    }
    if pending.len() > COVERAGE_LEAF_BATCH {
        return Err(IndexError::ResourceLimited);
    }
    let object_count = pending.leaves.iter().try_fold(0usize, |count, leaf| {
        context.budget.checkpoint()?;
        if quanta_index_core::index::pending_owner_coverage_value(&leaf.next)?.is_none() {
            count.checked_add(1).ok_or(IndexError::ResourceLimited)
        } else {
            Ok(count)
        }
    })?;
    let mut inputs = crate::staging_payload::StagedObjectBatch::new(object_count, context.budget)?;
    let mut predicted = [ObjectRef(Digest([0; 32])); COVERAGE_LEAF_BATCH];
    let mut slot = 0usize;
    let mut aggregate = 0u64;
    for leaf in &pending.leaves {
        context.budget.checkpoint()?;
        if quanta_index_core::index::pending_owner_coverage_value(&leaf.next)?.is_some() {
            continue;
        }
        let payload = binding_metadata::encode_owner_coverage(&leaf.next, context.budget)?;
        let children = owner_coverage_children(&leaf.next);
        let children = children.as_slice();
        let child_bytes = children
            .len()
            .checked_mul(32)
            .ok_or(IndexError::ResourceLimited)?;
        let input_bytes = payload
            .bytes
            .len()
            .checked_add(child_bytes)
            .and_then(|bytes| bytes.checked_add(256))
            .ok_or(IndexError::ResourceLimited)?;
        aggregate = aggregate
            .checked_add(u64::try_from(input_bytes).map_err(|_error| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        if aggregate > COVERAGE_LEAF_BATCH_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        *predicted.get_mut(slot).ok_or(IndexError::ResourceLimited)? = context
            .objects
            .identify(&payload.bytes, children, context.budget)?
            .reference;
        slot = slot.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        inputs.push(payload, children, context.budget)?;
    }
    let installed = if object_count == 0 {
        None
    } else {
        Some(context.put_batch(inputs.inputs())?)
    };
    if installed.as_ref().map_or(0, |refs| refs.len()) != object_count
        || installed.as_ref().is_some_and(|refs| {
            refs.iter()
                .zip(&predicted)
                .any(|(actual, expected)| actual != expected)
        })
    {
        return Err(IndexError::BackendViolation(
            "coverage leaf batch identity/order/count mismatch".into(),
        ));
    }
    let mut reference_iter = installed.as_ref().map(|refs| refs.iter());
    let edit_limit = pending
        .len()
        .checked_mul(if pending.shared_pending_roots { 1 } else { 2 })
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = edit_limit
        .checked_mul(std::mem::size_of::<OrdinalMapEdit>())
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
    let edit_memory = context
        .budget
        .reserve_memory(bytes, quanta_index_core::index::MemoryLifetime::Temporary)?;
    let mut rows = Vec::new();
    rows.try_reserve_exact(edit_limit)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for leaf in pending.leaves.drain(..) {
        let value = if let Some(value) =
            quanta_index_core::index::pending_owner_coverage_value(&leaf.next)?
        {
            value
        } else {
            (*reference_iter
                .as_mut()
                .and_then(Iterator::next)
                .ok_or_else(|| {
                    IndexError::BackendViolation("coverage ordinal result missing".into())
                })?)
            .into()
        };
        let next_ref = Some(value);
        if leaf.old_ref != next_ref {
            rows.push(OrdinalMapEdit::copy_key(
                0,
                &leaf.owner_key,
                leaf.old_ref,
                next_ref,
                context.budget,
            )?);
        }
        let next_missing = if leaf.missing { next_ref } else { None };
        if !pending.shared_pending_roots && leaf.old_missing != next_missing {
            rows.push(OrdinalMapEdit::copy_key(
                1,
                &leaf.owner_key,
                leaf.old_missing,
                next_missing,
                context.budget,
            )?);
        }
    }
    for batch in rows.chunks(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS) {
        edits.push_batch(batch, context.budget)?;
    }
    drop(rows);
    drop(edit_memory);
    Ok(())
}

#[cfg(test)]
mod coverage_staging_tests {
    use super::*;
    use crate::staging_payload::tests::{require, scoped_budget};
    use quanta_index_core::index::{ProducerId, StableOwnerId};

    fn binding() -> OwnerCoverage {
        OwnerCoverage {
            owner: OwnerKey {
                producer: ProducerId([255; 16]),
                stable_id: StableOwnerId([255; 16]),
            },
            desired: OwnerInputBinding {
                source: Some(ObjectRef(Digest([2; 32]))),
                projection: Some(ObjectRef(Digest([1; 32]))),
                producer_context: Digest([255; 32]),
                derivation_policy: Digest([255; 32]),
            },
            covered: Some(OwnerInputBinding {
                source: Some(ObjectRef(Digest([254; 32]))),
                projection: Some(ObjectRef(Digest([253; 32]))),
                producer_context: Digest([255; 32]),
                derivation_policy: Digest([255; 32]),
            }),
            searchable: Some(ObjectRef(Digest([2; 32]))),
        }
    }

    fn leaf() -> PendingCoverageLeaf {
        PendingCoverageLeaf {
            owner_key: owner_map_key(binding().owner),
            old_ref: None,
            old_missing: None,
            next: binding(),
            missing: true,
        }
    }

    #[test]
    fn shared_pending_coverage_rejects_divergent_rows_before_allocation() -> Result<(), IndexError>
    {
        for mode in 0..5 {
            let (budget, ledger) = scoped_budget(0)?;
            let mut pending = PendingCoverageBatch {
                shared_pending_roots: true,
                ..PendingCoverageBatch::default()
            };
            let mut row = leaf();
            row.next.covered = None;
            row.next.searchable = None;
            match mode {
                0 => row.old_ref = Some(ObjectRef(Digest([1; 32])).into()),
                1 => row.old_missing = Some(ObjectRef(Digest([1; 32])).into()),
                2 => row.missing = false,
                3 => row.next.covered = Some(row.next.desired),
                _ => row.next.searchable = Some(ObjectRef(Digest([1; 32]))),
            }
            require(matches!(
                pending.push(row, &budget),
                Err(IndexError::BackendViolation(_))
            ))?;
            require(pending.leaves.capacity() == 0 && pending.len() == 0 && ledger.live() == 0)?;
        }
        Ok(())
    }

    #[test]
    fn pending_coverage_refuses_floor_without_eager_or_partial_array_birth()
    -> Result<(), IndexError> {
        let (budget, ledger) = scoped_budget(0)?;
        let mut pending = PendingCoverageBatch::default();
        require(pending.len() == 0 && ledger.live() == 0)?;
        require(matches!(
            pending.push(leaf(), &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(pending.leaves.capacity() == 0 && pending.len() == 0 && ledger.live() == 0)
    }

    #[test]
    fn pending_coverage_known_one_keeps_exact_charge_and_refuses_overpush() -> Result<(), IndexError>
    {
        let bytes = u64::try_from(std::mem::size_of::<PendingCoverageLeaf>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let (budget, ledger) = scoped_budget(bytes)?;
        let mut pending = PendingCoverageBatch::with_known_count(1)?;
        pending.push(leaf(), &budget)?;
        require(pending.len() == 1 && ledger.live() == bytes)?;
        require(matches!(
            pending.push(leaf(), &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(pending.len() == 1 && ledger.live() == bytes)?;
        drop(pending);
        require(ledger.live() == 0)
    }

    #[test]
    fn pending_coverage_known_count_refuses_zero_or_insufficient_floor_before_heap_birth()
    -> Result<(), IndexError> {
        let bytes = u64::try_from(std::mem::size_of::<PendingCoverageLeaf>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let (zero_budget, zero_ledger) = scoped_budget(0)?;
        let mut zero = PendingCoverageBatch::with_known_count(0)?;
        require(matches!(
            zero.push(leaf(), &zero_budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(zero.len() == 0 && zero.leaves.capacity() == 0 && zero_ledger.live() == 0)?;

        let (short_budget, short_ledger) = scoped_budget(bytes.saturating_sub(1))?;
        let mut short = PendingCoverageBatch::with_known_count(1)?;
        require(matches!(
            short.push(leaf(), &short_budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(short.len() == 0 && short.leaves.capacity() == 0 && short_ledger.live() == 0)
    }

    #[test]
    fn pending_coverage_keeps_64_slot_charge_through_drain_and_reuse() -> Result<(), IndexError> {
        let bytes = COVERAGE_LEAF_BATCH
            .checked_mul(std::mem::size_of::<PendingCoverageLeaf>())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        let (budget, ledger) = scoped_budget(bytes)?;
        let mut pending = PendingCoverageBatch::default();
        for _row in 0..64 {
            pending.push(leaf(), &budget)?;
        }
        require(pending.len() == 64 && ledger.live() == bytes)?;
        require(matches!(
            pending.push(leaf(), &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(pending.leaves.drain(..).count() == 64 && ledger.live() == bytes)?;
        for _row in 0..64 {
            pending.push(leaf(), &budget)?;
        }
        require(pending.len() == 64 && ledger.live() == bytes)?;
        drop(pending);
        require(ledger.live() == 0)
    }

    #[test]
    fn pending_coverage_known_4k_retains_64_slot_batch_boundary() -> Result<(), IndexError> {
        let bytes = COVERAGE_LEAF_BATCH
            .checked_mul(std::mem::size_of::<PendingCoverageLeaf>())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        let (budget, ledger) = scoped_budget(bytes)?;
        let mut pending = PendingCoverageBatch::with_known_count(4_096)?;
        for _row in 0..COVERAGE_LEAF_BATCH {
            pending.push(leaf(), &budget)?;
        }
        require(pending.len() == COVERAGE_LEAF_BATCH && ledger.live() == bytes)?;
        require(matches!(
            pending.push(leaf(), &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(pending.leaves.drain(..).count() == COVERAGE_LEAF_BATCH)?;
        for _row in 0..COVERAGE_LEAF_BATCH {
            pending.push(leaf(), &budget)?;
        }
        require(pending.len() == COVERAGE_LEAF_BATCH && ledger.live() == bytes)?;
        drop(pending);
        require(ledger.live() == 0)
    }

    #[test]
    fn coverage_json_and_edges_preserve_old_wire_and_exclude_historical_stamps()
    -> Result<(), Box<dyn std::error::Error>> {
        let (budget, ledger) = scoped_budget(u64::MAX)?;
        let mut coverage = binding();
        coverage.desired.source = Some(ObjectRef(Digest([255; 32])));
        coverage.desired.projection = Some(ObjectRef(Digest([255; 32])));
        coverage.searchable = Some(ObjectRef(Digest([255; 32])));
        let old_wire = serde_json::to_vec(&coverage)?;
        let encoded = binding_metadata::encode_owner_coverage(&coverage, &budget)?;
        require(old_wire.len() <= binding_metadata::MAX_BYTES && encoded.bytes == old_wire)?;
        require(ledger.live() == u64::try_from(old_wire.len())?)?;
        drop(encoded);
        require(ledger.live() == 0)?;
        let references = owner_coverage_children(&binding());
        require(references.as_slice() == [ObjectRef(Digest([1; 32])), ObjectRef(Digest([2; 32]))])?;
        Ok(())
    }
}

fn load_member(
    reference: ObjectRef,
    instance: IndexInstanceId,
    context: &mut AttemptContext<'_>,
) -> Result<MemberBinding, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "member object reference mismatch".into(),
        ));
    }
    let binding = quanta_index_core::index::decode_member_binding(&stored.payload, context.budget)?;
    if binding.instance != instance
        || stored.registration.children.as_slice()
            != binding_metadata::member_children(&binding).as_slice()
    {
        return Err(IndexError::CorruptObject(
            "member key or artifact edge mismatch".into(),
        ));
    }
    Ok(binding)
}

fn charge_request_metadata(
    request: &UpdateRequest,
    budget: &mut WorkBudget,
) -> Result<u64, IndexError> {
    // The request carries a fixed-size mutation descriptor, not the corpus
    // mutation array. Only event metadata scales in this envelope.
    const FIXED_REQUEST_BYTES: u64 = 4_096;
    const EVENT_BYTES: u64 = 1_024;
    let (mutations, bytes) = match &request.kind {
        UpdateKind::SourceAdvance(transition) | UpdateKind::SourceResync { transition, .. } => {
            let events = u64::try_from(transition.events.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?;
            let mutations = events.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            let bytes = events
                .checked_mul(EVENT_BYTES)
                .and_then(|value| value.checked_add(FIXED_REQUEST_BYTES))
                .ok_or(IndexError::ResourceLimited)?;
            (mutations, bytes)
        }
        UpdateKind::ProjectionAdvance(transition) => {
            let events = u64::try_from(transition.events.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?;
            let mutations = events.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            let bytes = events
                .checked_mul(EVENT_BYTES)
                .and_then(|value| value.checked_add(FIXED_REQUEST_BYTES))
                .ok_or(IndexError::ResourceLimited)?;
            (mutations, bytes)
        }
        UpdateKind::Reconfigure { .. } => (2, FIXED_REQUEST_BYTES),
        UpdateKind::Materialize { .. } => (3, FIXED_REQUEST_BYTES),
    };
    budget.charge(WorkUsage {
        mutations,
        bytes,
        ..WorkUsage::default()
    })?;
    Ok(bytes)
}

fn refusal_for(error: &IndexError) -> Option<Refusal> {
    match error {
        IndexError::Conflict | IndexError::DigestConflict => Some(Refusal::Conflict),
        IndexError::InvalidInput(_) | IndexError::FocusSubjectNotFound => {
            Some(Refusal::InvalidInput)
        }
        IndexError::ResourceLimited | IndexError::WorkLimitExceeded => {
            Some(Refusal::ResourceLimited)
        }
        IndexError::BackendViolation(_) => Some(Refusal::BackendViolation),
        IndexError::RebuildRequired => Some(Refusal::RebuildRequired),
        IndexError::MaintenanceRequired => Some(Refusal::MaintenanceRequired),
        IndexError::WorkFenceLost => Some(Refusal::FenceLost),
        IndexError::Cancelled
        | IndexError::FenceLost
        | IndexError::SnapshotUnavailable
        | IndexError::CorruptObject(_)
        | IndexError::Storage(_) => None,
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn work_claim_fence_loss_is_terminal_but_operation_fence_loss_is_not() {
        assert_eq!(
            refusal_for(&IndexError::WorkFenceLost),
            Some(Refusal::FenceLost)
        );
        assert_eq!(refusal_for(&IndexError::FenceLost), None);
    }
}
