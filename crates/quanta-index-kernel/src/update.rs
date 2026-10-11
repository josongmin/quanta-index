use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use quanta_index_core::index::{
    AdmissionOutcome, AttemptContext, AttemptRef, BaseMember, CatalogView, CoverageBinding,
    CoverageStatus, Digest, GroupActivation, ImmutableOperation, IndexError, IndexInstanceId,
    IndexProfile, IngressRef, InputBinding, InputDomain, MapEdit, MapRoot, MemberBinding,
    ObjectBatchInput, ObjectRef, OperationKey, OperationStatus, OrdinalMapEdit, OwnerCoverage,
    OwnerInputBinding, OwnerKey, OwnerMutation, OwnerMutationCursor, OwnerMutationDigest,
    OwnerMutationSet, PreparedPublication, ProducerCompleteness, ProjectionCertificate,
    ProjectionOwnerContribution, ProjectionTransition, Refusal, RegisteredObject,
    SourceChangeCursor, SourceCut, SourceDelta, SourceInput, SourceRecord, SubmitOutcome,
    UpdateKind, UpdateRequest, ViewRef, ViewSelection, WorkAction, WorkActionManifestCursor,
    WorkActionSet, WorkBudget, WorkClaim, WorkIntent, WorkScope, WorkStatus, WorkUsage,
    WorkspaceKey, owner_map_key,
    ports::{
        CatalogPublicationPort, CatalogReadPort, ImmutableObjectPort, InputStagingPort,
        MapEditCursor, ObjectCustodyPort, ObjectWriteSink, OperationJournalPort, PersistentMapPort,
        WorkClaimPort,
    },
};

use crate::{
    identity, projection,
    registry::AdapterRegistry,
    source,
    spool::{EditSpool, WorkSpool},
    ticket::ReadTicket,
};

const MAX_PROFILE_INSTANCES: usize = 4096;

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
    + CatalogPublicationPort
    + CatalogReadPort
    + WorkClaimPort
    + Send
    + Sync
{
}

impl<T> ControlCatalog for T where
    T: ObjectCustodyPort
        + InputStagingPort
        + OperationJournalPort
        + CatalogPublicationPort
        + CatalogReadPort
        + WorkClaimPort
        + Send
        + Sync
{
}

pub struct KernelServices<C: ControlCatalog + 'static> {
    objects: Arc<dyn ImmutableObjectPort>,
    maps: Arc<dyn PersistentMapPort>,
    catalog: Arc<C>,
}

impl<C: ControlCatalog + 'static> KernelServices<C> {
    #[must_use]
    pub fn new(
        objects: Arc<dyn ImmutableObjectPort>,
        maps: Arc<dyn PersistentMapPort>,
        catalog: Arc<C>,
    ) -> Self {
        Self {
            objects,
            maps,
            catalog,
        }
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

const MAX_SOURCE_BATCH_INPUTS: usize = 32;
const MAX_SOURCE_BATCH_BYTES: usize = 8 * 1024 * 1024;

fn source_record_payload(
    path: &str,
    content: ObjectRef,
    byte_len: usize,
) -> Result<Vec<u8>, IndexError> {
    encode(&SourceRecord {
        path: path.to_owned(),
        bytes: content,
        byte_len: u64::try_from(byte_len).map_err(|_error| IndexError::ResourceLimited)?,
    })
}

impl ObjectWriteSink for IngressSink<'_> {
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

struct StageMutationCursor<'a, I> {
    input: I,
    objects: &'a dyn ImmutableObjectPort,
    sink: &'a dyn ObjectWriteSink,
    digest: OwnerMutationDigest,
    previous: Option<OwnerKey>,
}

impl<I: Iterator<Item = Result<OwnerMutation, IndexError>>> MapEditCursor
    for StageMutationCursor<'_, I>
{
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        let Some(mutation) = self.input.next().transpose()? else {
            return Ok(None);
        };
        if self.previous.is_some_and(|prior| prior >= mutation.owner) {
            return Err(IndexError::InvalidInput(
                "owner mutation stream must be sorted and unique".into(),
            ));
        }
        self.digest.push(&mutation)?;
        let bytes = encode(&mutation)?;
        let stored =
            self.objects
                .install(&bytes, &mutation.object_children(), self.sink, budget)?;
        self.previous = Some(mutation.owner);
        Ok(Some(OrdinalMapEdit {
            root: 0,
            edit: MapEdit {
                key: owner_map_key(mutation.owner),
                expected: None,
                next: Some(stored.reference),
            },
        }))
    }
}

impl<C: ControlCatalog + 'static> Kernel<C> {
    #[must_use]
    pub fn new(services: KernelServices<C>, registry: AdapterRegistry) -> Self {
        Self { services, registry }
    }

    /// Starts or fences a durable, operation-scoped input stage. The returned
    /// ingress is required for every typed input write before `submit`.
    pub fn begin_input(&self, key: OperationKey) -> Result<IngressRef, IndexError> {
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
        source::validate_path(path)?;
        let content = self.stage_object(ingress, bytes, &[], budget)?;
        self.stage_object(
            ingress,
            &source_record_payload(path, content, bytes.len())?,
            &[content],
            budget,
        )
    }

    /// Installs up to 32 source files under one ingress custody batch. All
    /// source bodies precede their records, so repeated bodies can be deduped
    /// without changing the returned record order.
    pub fn stage_sources(
        &self,
        ingress: IngressRef,
        inputs: &[SourceInput],
        budget: &mut WorkBudget,
    ) -> Result<Vec<ObjectRef>, IndexError> {
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
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(preflight_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            mutations: u64::try_from(inputs.len()).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;

        let mut bodies = Vec::with_capacity(inputs.len());
        let mut records = Vec::with_capacity(inputs.len());
        let mut predicted = Vec::with_capacity(inputs.len());
        let mut wire_bytes = 0_u64;
        for input in inputs {
            budget.checkpoint()?;
            let body = self.services.objects.identify(&input.bytes, &[], budget)?;
            let record_payload =
                source_record_payload(&input.path, body.reference, input.bytes.len())?;
            let record =
                self.services
                    .objects
                    .identify(&record_payload, &[body.reference], budget)?;
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
            bodies.push(ObjectBatchInput {
                payload: input.bytes.clone(),
                children: Vec::new(),
            });
            records.push(ObjectBatchInput {
                payload: record_payload,
                children: vec![body.reference],
            });
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
        let mut result = Vec::with_capacity(inputs.len());
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
        Ok(result)
    }

    pub fn stage_projection_output(
        &self,
        ingress: IngressRef,
        output: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        self.stage_object(ingress, output, &[], budget)
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
            if stored.registration.reference != output || !stored.registration.children.is_empty() {
                return Err(IndexError::InvalidInput(
                    "projection output must be an immutable leaf".into(),
                ));
            }
        }
        self.stage_object(
            ingress,
            &encode(&contribution)?,
            &contribution.object_children(),
            budget,
        )
    }

    pub fn stage_projection_certificate(
        &self,
        ingress: IngressRef,
        certificate: &ProjectionCertificate,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        self.stage_object(
            ingress,
            &encode(certificate)?,
            &certificate.object_children(),
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
        self.registry.validate_profile(&profile.instances)?;
        self.stage_object(ingress, &encode(profile)?, &[], budget)
    }

    /// Ingests a fallible stream without retaining the mutation set in RAM.
    /// Each mutation is installed child-first under the same ingress fence.
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
        let mut cursor = StageMutationCursor {
            input: mutations.into_iter(),
            objects: self.services.objects.as_ref(),
            sink: &sink,
            digest: OwnerMutationDigest::default(),
            previous: None,
        };
        let roots =
            self.services
                .maps
                .apply_stream(&[MapRoot(None)], &mut cursor, &sink, budget)?;
        let [root]: [MapRoot; 1] = roots.try_into().map_err(|_roots| {
            IndexError::BackendViolation("staged mutation manifest lost root".into())
        })?;
        Ok(OwnerMutationSet {
            root,
            count: cursor.digest.count(),
            digest: cursor.digest.finish(),
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
        let mut edits = Vec::with_capacity(mutations.len());
        let mut previous = None;
        for mutation in mutations {
            budget.checkpoint()?;
            if previous.is_some_and(|owner| owner >= mutation.owner) {
                return Err(IndexError::InvalidInput(
                    "mutation page must be sorted and unique".into(),
                ));
            }
            let payload = encode(mutation)?;
            let stored = self.services.objects.install(
                &payload,
                &mutation.object_children(),
                &sink,
                budget,
            )?;
            edits.push(MapEdit {
                key: owner_map_key(mutation.owner),
                expected: None,
                next: Some(stored.reference),
            });
            previous = Some(mutation.owner);
        }
        Ok(self.services.maps.apply(base, &edits, &sink, budget)?.root)
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
        self.services.catalog.reserve_inputs(ingress, &[])?;
        if let Some(reference) = root.0 {
            let stored = self.services.objects.get(reference, budget)?;
            if stored.registration.reference != reference {
                return Err(IndexError::CorruptObject(
                    "staged mutation root identity mismatch".into(),
                ));
            }
            self.services
                .catalog
                .reserve_inputs(ingress, std::slice::from_ref(&stored.registration))?;
            self.services
                .catalog
                .confirm_inputs(ingress, std::slice::from_ref(&stored.registration))?;
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
        let encoded = encode(&profile)?;
        let object = self.services.objects.put(&encoded, &[], budget)?;
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
        self.services.catalog.create_workspace(view, &[object])
    }

    pub fn pin_view(&self, selection: ViewSelection) -> Result<ReadTicket, IndexError> {
        let record = self.services.catalog.acquire_ticket(selection)?;
        let catalog: Arc<dyn CatalogReadPort> = self.services.catalog.clone();
        Ok(ReadTicket::new(record, catalog))
    }

    pub fn status(&self, key: OperationKey) -> Result<OperationStatus, IndexError> {
        self.services.catalog.inspect(key)
    }

    /// Decide a recovery from its observed status and original immutable
    /// request. This reads custody only and never rotates the attempt fence.
    pub fn resume_dispatch(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
    ) -> Result<ResumeDispatch, IndexError> {
        match self.services.catalog.inspect(key)? {
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
            Err(error) => match self.services.catalog.inspect(key) {
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
    ) -> Result<WorkClaim, IndexError> {
        self.services.catalog.claim_work(intent, lease)
    }

    pub fn renew_work(&self, claim: &WorkClaim, lease: Duration) -> Result<WorkClaim, IndexError> {
        self.services.catalog.renew_work(claim, lease)
    }

    pub fn work_status(&self, intent: &WorkIntent) -> Result<WorkStatus, IndexError> {
        self.services.catalog.work_status(intent)
    }

    /// Store one immutable request and submit its operation key. Already
    /// admitted keys return their original status; no new attempt is invented.
    pub async fn submit(
        &self,
        request: UpdateRequest,
        budget: &mut WorkBudget,
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
        match self.services.catalog.inspect(request.operation)? {
            OperationStatus::Unknown => {}
            existing @ (OperationStatus::Pending(_)
            | OperationStatus::Published(_)
            | OperationStatus::Refused(_)) => {
                let prior = self.services.catalog.operation(request.operation)?;
                let encoded_upper_bound = charge_request_metadata(&request, budget)?;
                let bytes = encode(&request)?;
                if u64::try_from(bytes.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?
                    > encoded_upper_bound
                {
                    return Err(IndexError::ResourceLimited);
                }
                // Replays compare the canonical immutable request identity;
                // terminal receipts do not depend on retained physical input.
                let identified =
                    self.services
                        .objects
                        .identify(&bytes, &request_children(&request), budget)?;
                if prior.request != identified.reference {
                    return Err(IndexError::DigestConflict);
                }
                return Ok(match existing {
                    OperationStatus::Pending(attempt) => SubmitOutcome::Accepted(attempt),
                    status @ (OperationStatus::Published(_) | OperationStatus::Refused(_)) => {
                        SubmitOutcome::Terminal(status)
                    }
                    OperationStatus::Unknown => SubmitOutcome::Unknown(request.operation),
                });
            }
        }
        match &request.kind {
            UpdateKind::ProjectionAdvance(transition) => {
                projection::validate_events(request.operation, transition)?;
            }
            UpdateKind::SourceAdvance(transition) => {
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
        // Reserve a conservative fixed-field JSON envelope and input closure
        // work before serde allocates a corpus-sized request. Physical writes
        // and later source edits remain separately charged to one cumulative
        // budget; this reservation is intentionally not refunded.
        let encoded_upper_bound = charge_request_metadata(&request, budget)?;
        let bytes = encode(&request)?;
        if u64::try_from(bytes.len()).map_err(|_conversion_error| IndexError::ResourceLimited)?
            > encoded_upper_bound
        {
            return Err(IndexError::ResourceLimited);
        }
        let children = request_children(&request);
        let ingress = self.services.catalog.open_ingress(request.operation)?;
        let sink = IngressSink {
            ingress,
            catalog: self.services.catalog.as_ref(),
        };
        let stored = self
            .services
            .objects
            .install(&bytes, &children, &sink, budget)?;
        let operation = ImmutableOperation {
            key: request.operation,
            request: stored.reference,
        };
        self.services
            .catalog
            .seal_inputs(ingress, stored.reference)?;
        let events = match &request.kind {
            UpdateKind::SourceAdvance(transition) => transition.events.as_slice(),
            UpdateKind::ProjectionAdvance(transition) => transition.events.as_slice(),
            UpdateKind::Reconfigure { .. } | UpdateKind::Materialize { .. } => &[],
        };
        let admitted = self
            .services
            .catalog
            .admit(operation, ingress, events, budget)?;
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

    /// Recovery reloads the original immutable operation, never caller input.
    pub async fn resume(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
    ) -> Result<SubmitOutcome, IndexError> {
        match self.services.catalog.inspect(key)? {
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
        match self.services.catalog.claim_recovery(key) {
            Ok(attempt) => self.drive(attempt, budget).await,
            Err(IndexError::FenceLost | IndexError::Conflict | IndexError::Storage(_)) => {
                Ok(self.reconcile(key))
            }
            Err(error) => Err(error),
        }
    }

    fn reconcile(&self, key: OperationKey) -> SubmitOutcome {
        match self.services.catalog.inspect(key) {
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
                |reason| match self.services.catalog.refuse(attempt, reason) {
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
        let mut context = AttemptContext {
            attempt,
            objects: self.services.objects.as_ref(),
            maps: self.services.maps.as_ref(),
            custody: self.services.catalog.as_ref(),
            budget,
        };
        let request = self.load_request(durable, context.budget)?;
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
        let mut ticket = self.pin_view(ViewSelection::Latest {
            workspace: request.expected_view.workspace,
            epoch: request.expected_view.epoch,
        })?;
        if matches!(&request.kind, UpdateKind::Materialize { .. }) {
            let UpdateKind::Materialize { claim } = request.kind else {
                return Err(IndexError::BackendViolation("materialize dispatch".into()));
            };
            loop {
                let base = ticket.view();
                match self.materialize(attempt, base, &claim, &mut context).await {
                    Err(IndexError::Conflict) => {
                        context.budget.checkpoint()?;
                        let latest = self.pin_view(ViewSelection::Latest {
                            workspace: request.expected_view.workspace,
                            epoch: request.expected_view.epoch,
                        })?;
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
            return self.projection_advance(attempt, base, transition, &mut context);
        }
        let (next_source, target_profile, delta, events, expected_source) = match &request.kind {
            UpdateKind::SourceAdvance(transition) => {
                let delta =
                    source::apply_source(attempt.operation, base.source, transition, &mut context)?;
                (
                    delta.next,
                    base.profile,
                    Some(delta),
                    transition.events.clone(),
                    Some(transition.expected_source),
                )
            }
            UpdateKind::Reconfigure { profile } => {
                if base.source.owners.0.is_some() || base.source.paths.0.is_some() {
                    return Err(IndexError::RebuildRequired);
                }
                (base.source, *profile, None, Vec::new(), None)
            }
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
        let (members, coverage, work) = self
            .build_members(
                base,
                &prior_profile,
                &profile,
                next_source,
                delta.as_ref(),
                &mut context,
            )
            .await?;
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
        let work_actions = work.stage(&mut context)?;
        let publication = PreparedPublication {
            attempt,
            expected: base.identity,
            next,
            work_actions,
            complete_work: None,
            source_events: events,
            projection_events: Vec::new(),
            expected_source,
        };
        // The read ticket is allowed to outlive the commit; it pins immutable
        // base roots while the catalog transfers protection to the next view.
        let mut actions =
            WorkActionManifestCursor::new(work_actions, context.maps, context.objects);
        self.services
            .catalog
            .commit_transition(publication, &mut actions, context.budget)
    }

    fn load_request(
        &self,
        operation: ImmutableOperation,
        budget: &mut WorkBudget,
    ) -> Result<UpdateRequest, IndexError> {
        let stored = self.services.objects.get(operation.request, budget)?;
        if stored.registration.reference != operation.request {
            return Err(IndexError::CorruptObject(
                "journal request reference mismatch".into(),
            ));
        }
        let request: UpdateRequest = decode(&stored.payload, "durable request")?;
        if request.operation != operation.key
            || request.expected_view.workspace != operation.key.workspace
            || request.expected_view.epoch != operation.key.epoch
            || stored.registration.children != request_children(&request)
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
    ) -> Result<IndexProfile, IndexError> {
        let stored = context.objects.get(reference, context.budget)?;
        if stored.registration.reference != reference || !stored.registration.children.is_empty() {
            return Err(IndexError::CorruptObject("invalid profile object".into()));
        }
        let profile: IndexProfile = decode(&stored.payload, "index profile")?;
        if profile.instances.len() > MAX_PROFILE_INSTANCES {
            return Err(IndexError::ResourceLimited);
        }
        context.budget.charge(WorkUsage {
            mutations: u64::try_from(profile.instances.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.registry.validate_profile(&profile.instances)?;
        let mut groups = BTreeMap::new();
        for instance in &profile.instances {
            if groups
                .insert(instance.group, instance.activation)
                .is_some_and(|prior| prior != instance.activation)
            {
                return Err(IndexError::InvalidInput(
                    "publication group has mixed activation".into(),
                ));
            }
        }
        Ok(profile)
    }

    async fn build_members(
        &self,
        base: &CatalogView,
        prior_profile: &IndexProfile,
        profile: &IndexProfile,
        source: SourceCut,
        delta: Option<&SourceDelta>,
        context: &mut AttemptContext<'_>,
    ) -> Result<(MapRoot, MapRoot, WorkSpool), IndexError> {
        let work = WorkSpool::open()?;
        let prior: BTreeMap<_, _> = prior_profile
            .instances
            .iter()
            .map(|spec| (spec.instance, spec))
            .collect();
        let target: BTreeMap<_, _> = profile
            .instances
            .iter()
            .map(|spec| (spec.instance, spec))
            .collect();
        let mut edits = Vec::new();
        let mut coverage_edits = Vec::new();
        let source_context = identity::source_context(source)?;
        let instances: BTreeSet<_> = prior.keys().chain(target.keys()).copied().collect();
        for instance in instances {
            let key = member_key(instance);
            let old_ref = context.maps.lookup(base.members, &key, context.budget)?;
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
                    let base_member = old_binding.map_or(BaseMember::Absent, |binding| {
                        BaseMember::Present(Box::new(binding))
                    });
                    let binding = self
                        .registry
                        .build(spec, base_member, delta, source, input, context)
                        .await?;
                    let artifact = context.objects.get(binding.artifact, context.budget)?;
                    if artifact.registration.reference != binding.artifact {
                        return Err(IndexError::BackendViolation(
                            "sealed artifact reference mismatch".into(),
                        ));
                    }
                    let mut children = vec![binding.artifact];
                    if let Some(root) = binding.input.root.0 {
                        children.push(root);
                    }
                    Some(context.put(&encode(&binding)?, &children)?)
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
                edits.push(MapEdit {
                    key,
                    expected: old_ref,
                    next: next_ref,
                });
            }
            let coverage_key = member_key(instance);
            let covered = context
                .maps
                .lookup(base.coverage, &coverage_key, context.budget)?;
            let next_coverage = match target.get(&instance) {
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
                    let prior_coverage = covered
                        .map(|reference| load_coverage(reference, instance, context))
                        .transpose()?;
                    if prior.get(&instance).is_some_and(|old| *old != *spec)
                        && prior_coverage.is_some_and(|binding| binding.owners.0.is_some())
                    {
                        return Err(IndexError::RebuildRequired);
                    }
                    let current_projection =
                        projection::lookup_binding(base.projections, projection_id, context)?;
                    if current_projection.is_some_and(|(_, binding)| binding.producer != producer) {
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
                    let prior_coverage = covered
                        .map(|reference| load_coverage(reference, instance, context))
                        .transpose()?;
                    if prior.get(&instance).is_some_and(|old| {
                        old.input_domain != spec.input_domain
                            || old.kind != spec.kind
                            || old.format != spec.format
                    }) && prior_coverage.is_some_and(|binding| binding.owners.0.is_some())
                    {
                        return Err(IndexError::RebuildRequired);
                    }
                    if prior
                        .get(&instance)
                        .is_some_and(|old| old.activation == GroupActivation::AsyncCoverage)
                        && prior_coverage.is_none()
                    {
                        return Err(IndexError::CorruptObject(
                            "published async coverage is absent".into(),
                        ));
                    }
                    if delta.is_none() && source.owners.0.is_some() {
                        return Err(IndexError::RebuildRequired);
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
                    let edits = EditSpool::open()?;
                    let mut pending_coverage = Vec::with_capacity(COVERAGE_LEAF_BATCH);
                    if let Some(delta) = delta {
                        let mut changes = SourceChangeCursor::new(
                            delta.changed,
                            delta.prior,
                            delta.next,
                            context.maps,
                            context.objects,
                        );
                        while let Some(change) = changes.next_change(context.budget)? {
                            let owner_key = owner_map_key(change.owner);
                            let old_ref =
                                context
                                    .maps
                                    .lookup(prior_owners, &owner_key, context.budget)?;
                            let old = old_ref
                                .map(|reference| {
                                    load_owner_coverage(reference, change.owner, context)
                                })
                                .transpose()?;
                            let missing_ref =
                                context
                                    .maps
                                    .lookup(prior_missing, &owner_key, context.budget)?;
                            if old.is_none() && change.prior.is_some() {
                                return Err(IndexError::CorruptObject(
                                    "async owner coverage is absent".into(),
                                ));
                            }
                            let next_input =
                                change.next.as_ref().map(|(record, _)| OwnerInputBinding {
                                    source: Some(*record),
                                    projection: None,
                                    producer_context: Digest([0; 32]),
                                    derivation_policy: spec.policy,
                                });
                            if let Some(previous) = old
                                && previous.covered != Some(previous.desired)
                                && Some(previous.desired) != next_input
                            {
                                work.push(
                                    &WorkAction::Retire(owner_work(
                                        base,
                                        instance,
                                        change.owner,
                                        previous.desired,
                                    )),
                                    context.budget,
                                )?;
                            }
                            let next_ref = match next_input {
                                Some(owner_input)
                                    if old.is_some_and(|prior| prior.desired == owner_input) =>
                                {
                                    old_ref
                                }
                                Some(owner_input) => {
                                    let cached =
                                        old.filter(|prior| prior.covered == Some(owner_input));
                                    let owner_coverage = OwnerCoverage {
                                        owner: change.owner,
                                        desired: owner_input,
                                        covered: cached.and_then(|prior| prior.covered),
                                        searchable: cached.and_then(|prior| prior.searchable),
                                    };
                                    if cached.is_none() {
                                        work.push(
                                            &WorkAction::Create(owner_work(
                                                base,
                                                instance,
                                                change.owner,
                                                owner_input,
                                            )),
                                            context.budget,
                                        )?;
                                    }
                                    pending_coverage.push(PendingCoverageLeaf {
                                        owner_key,
                                        old_ref,
                                        old_missing: missing_ref,
                                        next: owner_coverage,
                                        missing: cached.is_none(),
                                    });
                                    if pending_coverage.len() == COVERAGE_LEAF_BATCH {
                                        flush_coverage_leaves(
                                            &mut pending_coverage,
                                            &edits,
                                            context,
                                        )?;
                                    }
                                    continue;
                                }
                                None => None,
                            };
                            if old_ref != next_ref {
                                edits.push(
                                    0,
                                    &MapEdit {
                                        key: owner_key.clone(),
                                        expected: old_ref,
                                        next: next_ref,
                                    },
                                    context.budget,
                                )?;
                            }
                            let next_missing = if next_ref == old_ref {
                                missing_ref
                            } else {
                                None
                            };
                            if missing_ref != next_missing {
                                edits.push(
                                    1,
                                    &MapEdit {
                                        key: owner_key,
                                        expected: missing_ref,
                                        next: next_missing,
                                    },
                                    context.budget,
                                )?;
                            }
                        }
                    }
                    flush_coverage_leaves(&mut pending_coverage, &edits, context)?;
                    let roots = context
                        .apply_stream(&[prior_owners, prior_missing], &mut edits.cursor())?;
                    let [owners, missing]: [MapRoot; 2] = roots.try_into().map_err(|_roots| {
                        IndexError::BackendViolation("source coverage map stream lost roots".into())
                    })?;
                    let status = if missing.0.is_some() {
                        CoverageStatus::Partial { missing }
                    } else {
                        CoverageStatus::Complete
                    };
                    let binding = CoverageBinding {
                        instance,
                        desired,
                        covered: if missing.0.is_some() {
                            None
                        } else {
                            Some(desired)
                        },
                        searchable: owners.0,
                        status,
                        owners,
                    };
                    Some(save_coverage(&binding, context)?)
                }
                _ => None,
            };
            if covered != next_coverage {
                coverage_edits.push(MapEdit {
                    key: coverage_key,
                    expected: covered,
                    next: next_coverage,
                });
            }
        }
        // Sorted unique member keys are required by the persistent map port.
        edits.sort_by(|a, b| a.key.cmp(&b.key));
        coverage_edits.sort_by(|a, b| a.key.cmp(&b.key));
        let members = context.apply(base.members, &edits)?;
        let coverage = context.apply(base.coverage, &coverage_edits)?;
        Ok((members, coverage, work))
    }

    fn projection_advance(
        &self,
        attempt: AttemptRef,
        base: &CatalogView,
        transition: &ProjectionTransition,
        context: &mut AttemptContext<'_>,
    ) -> Result<OperationStatus, IndexError> {
        let delta = projection::apply_projection(
            attempt.operation,
            base.projections,
            base.source,
            transition,
            context,
        )?;
        let profile = self.load_profile(base.profile, context)?;
        let mut coverage_edits = Vec::new();
        let work = WorkSpool::open()?;
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
            if producer != transition.producer || spec.activation != GroupActivation::AsyncCoverage
            {
                return Err(IndexError::InvalidInput(
                    "projection instance producer or activation mismatch".into(),
                ));
            }
            let key = member_key(spec.instance);
            let old_ref = context.maps.lookup(base.coverage, &key, context.budget)?;
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
            let edits = EditSpool::open()?;
            let mut pending_coverage = Vec::with_capacity(COVERAGE_LEAF_BATCH);
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
                    work.push(
                        &WorkAction::Retire(owner_work(
                            base,
                            spec.instance,
                            change.owner,
                            previous.desired,
                        )),
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
                        let owner_coverage = OwnerCoverage {
                            owner: change.owner,
                            desired: input,
                            covered: if empty {
                                Some(input)
                            } else {
                                cached.and_then(|v| v.covered)
                            },
                            searchable: if empty {
                                None
                            } else {
                                cached.and_then(|v| v.searchable)
                            },
                        };
                        if !empty && cached.is_none() {
                            work.push(
                                &WorkAction::Create(owner_work(
                                    base,
                                    spec.instance,
                                    change.owner,
                                    input,
                                )),
                                context.budget,
                            )?;
                        }
                        let covered = empty
                            || old.is_some_and(|previous| {
                                next_input == previous.covered && previous.searchable.is_some()
                            });
                        pending_coverage.push(PendingCoverageLeaf {
                            owner_key,
                            old_ref: prior_ref,
                            old_missing,
                            next: owner_coverage,
                            missing: !covered,
                        });
                        if pending_coverage.len() == COVERAGE_LEAF_BATCH {
                            flush_coverage_leaves(&mut pending_coverage, &edits, context)?;
                        }
                        continue;
                    }
                    (None, None) => None,
                    _ => return Err(IndexError::BackendViolation("projection input".into())),
                };
                if prior_ref != next_ref {
                    edits.push(
                        0,
                        &MapEdit {
                            key: owner_key.clone(),
                            expected: prior_ref,
                            next: next_ref,
                        },
                        context.budget,
                    )?;
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
                    edits.push(
                        1,
                        &MapEdit {
                            key: owner_key,
                            expected: old_missing,
                            next: next_missing,
                        },
                        context.budget,
                    )?;
                }
            }
            flush_coverage_leaves(&mut pending_coverage, &edits, context)?;
            let roots =
                context.apply_stream(&[prior.owners, prior_missing], &mut edits.cursor())?;
            let [owners, missing]: [MapRoot; 2] = roots.try_into().map_err(|_roots| {
                IndexError::BackendViolation("projection coverage map stream lost roots".into())
            })?;
            let status = if missing.0.is_some() {
                CoverageStatus::Partial { missing }
            } else {
                CoverageStatus::Complete
            };
            let next_coverage = CoverageBinding {
                instance: spec.instance,
                desired,
                covered: if missing.0.is_some() {
                    None
                } else {
                    Some(desired)
                },
                searchable: owners.0,
                status,
                owners,
            };
            let next_ref = save_coverage(&next_coverage, context)?;
            coverage_edits.push(MapEdit {
                key,
                expected: Some(old_ref),
                next: Some(next_ref),
            });
        }
        coverage_edits.sort_by(|a, b| a.key.cmp(&b.key));
        let coverage = if coverage_edits.is_empty() {
            base.coverage
        } else {
            context.apply(base.coverage, &coverage_edits)?
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
            members: base.members,
            coverage,
        };
        next.identity = identity::view_identity(&next)?;
        let work_actions = work.stage(context)?;
        let mut actions =
            WorkActionManifestCursor::new(work_actions, context.maps, context.objects);
        self.services.catalog.commit_transition(
            PreparedPublication {
                attempt,
                expected: base.identity,
                next,
                work_actions,
                complete_work: None,
                source_events: Vec::new(),
                projection_events: transition.events.clone(),
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
                let actual = context.maps.lookup(
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
                let actual =
                    context
                        .maps
                        .lookup(binding.owners, &owner_map_key(owner), context.budget)?;
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
                    let actual = context.maps.lookup(
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
            .lookup(base.coverage, &coverage_key, context.budget)?
            .ok_or_else(|| IndexError::CorruptObject("async coverage missing".into()))?;
        let coverage = load_coverage(coverage_ref, spec.instance, context)?;
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
        let (binding, newly_prepared) =
            if let Some(binding) = self.services.catalog.prepared_work(claim)? {
                (binding, false)
            } else {
                let binding = self
                    .registry
                    .build_owner(spec, owner, expected, context)
                    .await?;
                (binding, true)
            };
        crate::registry::validate_owner_member(&binding, spec, owner, expected)?;
        let artifact = context.objects.get(binding.artifact, context.budget)?;
        if artifact.registration.reference != binding.artifact {
            return Err(IndexError::BackendViolation(
                "owner adapter artifact reference mismatch".into(),
            ));
        }
        if newly_prepared {
            self.services.catalog.stage_work(claim, binding.clone())?;
        }
        let mut binding_children = vec![binding.artifact];
        binding_children.extend(binding.input.source);
        binding_children.extend(binding.input.projection);
        binding_children.sort_unstable();
        binding_children.dedup();
        let binding_ref = context.put(&encode(&binding)?, &binding_children)?;
        let next_owner = OwnerCoverage {
            owner,
            desired: expected,
            covered: Some(expected),
            searchable: Some(binding_ref),
        };
        let next_owner_ref = save_owner_coverage(&next_owner, context)?;
        let owners = context.apply(
            coverage.owners,
            &[MapEdit {
                key: owner_key.clone(),
                expected: Some(owner_ref),
                next: Some(next_owner_ref),
            }],
        )?;
        let CoverageStatus::Partial {
            missing: old_missing_root,
        } = coverage.status
        else {
            return Err(IndexError::Conflict);
        };
        let missing = context.apply(
            old_missing_root,
            &[MapEdit {
                key: owner_key,
                expected: Some(owner_ref),
                next: None,
            }],
        )?;
        let status = if missing.0.is_some() {
            CoverageStatus::Partial { missing }
        } else {
            CoverageStatus::Complete
        };
        let next_coverage = CoverageBinding {
            instance: coverage.instance,
            desired: coverage.desired,
            covered: if missing.0.is_some() {
                None
            } else {
                Some(coverage.desired)
            },
            searchable: owners.0,
            status,
            owners,
        };
        let next_coverage_ref = save_coverage(&next_coverage, context)?;
        let coverage_root = context.apply(
            base.coverage,
            &[MapEdit {
                key: coverage_key,
                expected: Some(coverage_ref),
                next: Some(next_coverage_ref),
            }],
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
        let work_actions = WorkActionSet::empty();
        let mut actions =
            WorkActionManifestCursor::new(work_actions, context.maps, context.objects);
        self.services.catalog.commit_transition(
            PreparedPublication {
                attempt,
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
        )
    }
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
    let binding: CoverageBinding = decode(&stored.payload, "coverage binding")?;
    if binding.instance != instance || stored.registration.children != coverage_children(&binding) {
        return Err(IndexError::CorruptObject(
            "coverage identity/edges mismatch".into(),
        ));
    }
    Ok(binding)
}

fn coverage_children(binding: &CoverageBinding) -> Vec<ObjectRef> {
    let mut children = Vec::new();
    children.extend(binding.desired.root.0);
    children.extend(binding.owners.0);
    if let CoverageStatus::Partial { missing } = binding.status {
        children.extend(missing.0);
    }
    children.extend(binding.searchable);
    children.sort_unstable();
    children.dedup();
    children
}

fn save_coverage(
    binding: &CoverageBinding,
    context: &mut AttemptContext<'_>,
) -> Result<ObjectRef, IndexError> {
    context.put(&encode(binding)?, &coverage_children(binding))
}

fn load_owner_coverage(
    reference: ObjectRef,
    owner: OwnerKey,
    context: &mut AttemptContext<'_>,
) -> Result<OwnerCoverage, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "owner coverage reference mismatch".into(),
        ));
    }
    let binding: OwnerCoverage = decode(&stored.payload, "owner coverage")?;
    if binding.owner != owner || stored.registration.children != owner_coverage_children(&binding) {
        return Err(IndexError::CorruptObject(
            "owner coverage identity/edges mismatch".into(),
        ));
    }
    Ok(binding)
}

fn owner_coverage_children(binding: &OwnerCoverage) -> Vec<ObjectRef> {
    let mut children = Vec::new();
    children.extend(binding.desired.source);
    children.extend(binding.desired.projection);
    children.extend(binding.searchable);
    children.sort_unstable();
    children.dedup();
    children
}

fn save_owner_coverage(
    binding: &OwnerCoverage,
    context: &mut AttemptContext<'_>,
) -> Result<ObjectRef, IndexError> {
    context.put(&encode(binding)?, &owner_coverage_children(binding))
}

const COVERAGE_LEAF_BATCH: usize = 64;
const COVERAGE_LEAF_BATCH_BYTES: u64 = 8 * 1024 * 1024;

struct PendingCoverageLeaf {
    owner_key: Vec<u8>,
    old_ref: Option<ObjectRef>,
    old_missing: Option<ObjectRef>,
    next: OwnerCoverage,
    missing: bool,
}

fn flush_coverage_leaves(
    pending: &mut Vec<PendingCoverageLeaf>,
    edits: &EditSpool,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if pending.is_empty() {
        return Ok(());
    }
    if pending.len() > COVERAGE_LEAF_BATCH {
        return Err(IndexError::ResourceLimited);
    }
    let mut inputs = Vec::with_capacity(pending.len());
    let mut predicted = Vec::with_capacity(pending.len());
    let mut aggregate = 0_u64;
    for leaf in pending.iter() {
        let payload = encode(&leaf.next)?;
        let children = owner_coverage_children(&leaf.next);
        let child_bytes = children
            .len()
            .checked_mul(32)
            .ok_or(IndexError::ResourceLimited)?;
        let input_bytes = payload
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
        predicted.push(
            context
                .objects
                .identify(&payload, &children, context.budget)?
                .reference,
        );
        inputs.push(ObjectBatchInput { payload, children });
    }
    let installed = context.put_batch(&inputs)?;
    if installed.len() != pending.len() || predicted.len() != pending.len() {
        return Err(IndexError::BackendViolation(
            "coverage leaf batch lost an ordinal result".into(),
        ));
    }
    for ((leaf, reference), expected_identity) in pending.drain(..).zip(installed).zip(predicted) {
        if reference != expected_identity {
            return Err(IndexError::BackendViolation(
                "coverage leaf batch identity or order mismatch".into(),
            ));
        }
        let next_ref = Some(reference);
        if leaf.old_ref != next_ref {
            edits.push(
                0,
                &MapEdit {
                    key: leaf.owner_key.clone(),
                    expected: leaf.old_ref,
                    next: next_ref,
                },
                context.budget,
            )?;
        }
        let next_missing = if leaf.missing { next_ref } else { None };
        if leaf.old_missing != next_missing {
            edits.push(
                1,
                &MapEdit {
                    key: leaf.owner_key,
                    expected: leaf.old_missing,
                    next: next_missing,
                },
                context.budget,
            )?;
        }
    }
    Ok(())
}

fn member_key(instance: IndexInstanceId) -> Vec<u8> {
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&instance.0);
    key
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
    let binding: MemberBinding = decode(&stored.payload, "member binding")?;
    if binding.instance != instance || !stored.registration.children.contains(&binding.artifact) {
        return Err(IndexError::CorruptObject(
            "member key or artifact edge mismatch".into(),
        ));
    }
    Ok(binding)
}

fn request_children(request: &UpdateRequest) -> Vec<ObjectRef> {
    let mut children = match &request.kind {
        UpdateKind::SourceAdvance(transition) => transition.owners.object_children(),
        UpdateKind::ProjectionAdvance(transition) => {
            let mut refs = transition.owners.object_children();
            if let ProducerCompleteness::CompleteAffectedDomain { certificate } =
                transition.completeness
            {
                refs.push(certificate);
            }
            refs
        }
        UpdateKind::Reconfigure { profile } => vec![*profile],
        UpdateKind::Materialize { claim } => match claim.intent.scope {
            WorkScope::Owner { expected, .. } => {
                let mut refs = Vec::new();
                refs.extend(expected.source);
                refs.extend(expected.projection);
                refs
            }
            WorkScope::ProjectionDomain { .. } => Vec::new(),
        },
    };
    children.sort_unstable();
    children.dedup();
    children
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
        UpdateKind::SourceAdvance(transition) => {
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
        IndexError::InvalidInput(_) => Some(Refusal::InvalidInput),
        IndexError::ResourceLimited => Some(Refusal::ResourceLimited),
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

fn decode<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    what: &'static str,
) -> Result<T, IndexError> {
    serde_json::from_slice(bytes)
        .map_err(|error| IndexError::CorruptObject(format!("{what}: {error}")))
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, IndexError> {
    serde_json::to_vec(value)
        .map_err(|error| IndexError::InvalidInput(format!("value encoding: {error}")))
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
