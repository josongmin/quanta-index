//! Native source-event rebase across a durably admitted conflicting operation.
//! Uses the managed object store and the same ingress custody as Kernel submit.

use std::{error::Error, path::Path, sync::Arc, time::Duration};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::index::{
    AdmissionOutcome, CollectionId, Digest, GroupActivation, ImmutableOperation, IndexError,
    IndexInstanceId, IndexKind, IndexProfile, IngressRef, InputDomain, InstanceSpec, MemberBinding,
    ObjectInstallScope, ObjectRef, OperationKey, OperationStatus, OwnerKey, OwnerMutation,
    ProducerId, PublicationGroupId, Refusal, RegisteredObject, SchemaId, SlotId, SourceCut,
    SourceEvent, SourceTextAdmission, SourceTransition, StableOwnerId, StorageFormatId, StreamId,
    SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewRef, ViewSelection, WorkBudget,
    WorkLimits, WorkspaceKey, WorktreeId, owner_map_key,
    ports::{
        ImmutableObjectPort, InputStagingPort, ObjectWriteSink, OperationJournalPort,
        PersistentMapPort,
    },
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, Kernel, KernelServices};
use quanta_index_lexical::native::{LiteralQuery, LiteralReadContext, NativeLiteralIndex};
use quanta_index_repomap::native::{
    FileMapReadContext, FilePathEntry, FilePathQuery, NativeFileMap, query_paths,
};
use quanta_index_storage::{FileObjectStore, FileScratchSpace};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

macro_rules! check {
    ($condition:expr, $message:literal) => {
        if !$condition {
            return Err($message.into());
        }
    };
}

macro_rules! check_eq {
    ($actual:expr, $expected:expr, $message:literal) => {
        if $actual != $expected {
            return Err($message.into());
        }
    };
}

#[path = "native_source_rebase/source_trace.rs"]
mod source_trace;

fn budget() -> WorkBudget {
    budget_with_request(quanta_index_core::RequestBudgetV1::for_duration(
        Duration::from_secs(30),
    ))
}

fn budget_with_request(request: quanta_index_core::RequestBudgetV1) -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        },
        request,
    )
}

fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}

fn key(producer: u8, stream: u8, sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: ProducerId([producer; 16]),
        stream: StreamId([stream; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn owner(producer: u8, id: u8) -> OwnerKey {
    OwnerKey {
        producer: ProducerId([producer; 16]),
        stable_id: StableOwnerId([id; 16]),
    }
}

fn literal_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([7; 16]),
        kind: IndexKind([8; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: Digest([11; 32]),
        format: StorageFormatId([12; 16]),
    }
}

fn file_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([17; 16]),
        kind: IndexKind([18; 16]),
        format: StorageFormatId([19; 16]),
        ..literal_spec()
    }
}

struct Fixture {
    engine: Kernel<SqliteCatalog>,
    objects: Arc<FileObjectStore>,
    catalog: Arc<SqliteCatalog>,
}

impl Fixture {
    fn open(root: &Path) -> TestResult<Self> {
        let scratch = Arc::new(FileScratchSpace::open_standalone(
            root.join("scratch"),
            16 * 1024 * 1024 * 1024,
        )?);
        let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
        let objects = Arc::new(FileObjectStore::open_managed_with_scratch(
            root.join("native-objects"),
            scratch.clone(),
            catalog.clone(),
        )?);
        let mut registry = AdapterRegistry::new();
        registry.register(
            AdapterDescriptor {
                kind: literal_spec().kind,
                schema: literal_spec().schema,
                format: literal_spec().format,
            },
            NativeLiteralIndex,
            |delta| Ok(delta.clone()),
        )?;
        registry.register(
            AdapterDescriptor {
                kind: file_spec().kind,
                schema: file_spec().schema,
                format: file_spec().format,
            },
            NativeFileMap,
            |delta| Ok(delta.clone()),
        )?;
        let services = KernelServices::new(
            objects.clone(),
            objects.clone(),
            catalog.clone(),
            scratch,
            Duration::from_secs(30),
        )?;
        Ok(Self {
            engine: Kernel::new(services, registry),
            objects,
            catalog,
        })
    }

    fn configure(&self) -> TestResult<ViewRef> {
        let genesis = self
            .engine
            .create_workspace(workspace(), 1, &mut budget())?;
        let operation = key(5, 6, 1);
        let ingress = self.engine.begin_input(operation)?;
        let profile = self.engine.stage_profile(
            ingress,
            &IndexProfile {
                instances: vec![literal_spec(), file_spec()],
            },
            &mut budget(),
        )?;
        published(submit(
            &self.engine,
            UpdateRequest {
                operation,
                expected_view: genesis,
                kind: UpdateKind::Reconfigure { profile },
            },
        )?)
    }

    fn source(&self, view: ViewRef) -> TestResult<SourceCut> {
        Ok(self
            .engine
            .pin_view(ViewSelection::Exact(view), &budget())?
            .view()
            .source)
    }

    fn capture(
        &self,
        operation: OperationKey,
        expected_view: ViewRef,
        changed_owner: OwnerKey,
        expected_record: Option<ObjectRef>,
        path: &str,
        bytes: &[u8],
        event_sequence: u64,
    ) -> TestResult<(IngressRef, UpdateRequest, ObjectRef)> {
        let ingress = self.engine.begin_input(operation)?;
        let record = self.engine.stage_source_text(
            ingress,
            path,
            bytes,
            SourceTextAdmission::Utf8 {
                policy: Digest([42; 32]),
            },
            &mut budget(),
        )?;
        let owners = self.engine.stage_mutations(
            ingress,
            std::iter::once(Ok(OwnerMutation {
                owner: changed_owner,
                expected: expected_record,
                next: Some(record),
            })),
            &mut budget(),
        )?;
        let mut transition = SourceTransition {
            expected_source: self.source(expected_view)?,
            owners,
            events: Vec::new(),
        };
        transition.events.push(SourceEvent {
            producer: operation.producer,
            stream: operation.stream,
            stream_epoch: operation.stream_epoch,
            predecessor: event_sequence
                .checked_sub(1)
                .ok_or("source event predecessor underflow")?,
            sequence: event_sequence,
            payload: transition.owner_digest()?,
        });
        Ok((
            ingress,
            UpdateRequest {
                operation,
                expected_view,
                kind: UpdateKind::SourceAdvance(transition),
            },
            record,
        ))
    }

    fn capture_path_handoff(
        &self,
        operation: OperationKey,
        expected_view: ViewRef,
        old_owner: OwnerKey,
        old_record: ObjectRef,
        new_owner: OwnerKey,
        bytes: &[u8],
        event_sequence: u64,
    ) -> TestResult<(IngressRef, UpdateRequest, ObjectRef)> {
        check!(
            old_owner != new_owner,
            "path handoff requires different owners"
        );
        let ingress = self.engine.begin_input(operation)?;
        let record = self.engine.stage_source_text(
            ingress,
            "handoff.rs",
            bytes,
            SourceTextAdmission::Utf8 {
                policy: Digest([42; 32]),
            },
            &mut budget(),
        )?;
        let mut mutations = vec![
            OwnerMutation {
                owner: old_owner,
                expected: Some(old_record),
                next: None,
            },
            OwnerMutation {
                owner: new_owner,
                expected: None,
                next: Some(record),
            },
        ];
        mutations.sort_by_key(|mutation| mutation.owner);
        let owners =
            self.engine
                .stage_mutations(ingress, mutations.into_iter().map(Ok), &mut budget())?;
        let mut transition = SourceTransition {
            expected_source: self.source(expected_view)?,
            owners,
            events: Vec::new(),
        };
        transition.events.push(SourceEvent {
            producer: operation.producer,
            stream: operation.stream,
            stream_epoch: operation.stream_epoch,
            predecessor: event_sequence
                .checked_sub(1)
                .ok_or("source event predecessor underflow")?,
            sequence: event_sequence,
            payload: transition.owner_digest()?,
        });
        Ok((
            ingress,
            UpdateRequest {
                operation,
                expected_view,
                kind: UpdateKind::SourceAdvance(transition),
            },
            record,
        ))
    }

    fn owner_record(&self, view: ViewRef, selected: OwnerKey) -> TestResult<Option<ObjectRef>> {
        Ok(self.objects.lookup_object(
            self.source(view)?.owners,
            &owner_map_key(selected),
            &mut budget(),
        )?)
    }

    fn member(&self, view: ViewRef, instance: IndexInstanceId) -> TestResult<MemberBinding> {
        let ticket = self
            .engine
            .pin_view(ViewSelection::Exact(view), &budget())?;
        let mut key = b"member\0".to_vec();
        key.extend_from_slice(&instance.0);
        let reference = self
            .objects
            .lookup_object(ticket.view().members, &key, &mut budget())?
            .ok_or("required native member absent")?;
        Ok(serde_json::from_slice(
            &self.objects.get(reference, &mut budget())?.payload,
        )?)
    }

    fn paths(&self, view: ViewRef) -> TestResult<Vec<FilePathEntry>> {
        let ticket = self
            .engine
            .pin_view(ViewSelection::Exact(view), &budget())?;
        let member = self.member(view, file_spec().instance)?;
        Ok(query_paths(
            &FilePathQuery {
                member: &member,
                prefix: "",
                after: None,
                limit: 16,
            },
            &mut FileMapReadContext {
                ticket: ticket.record(),
                objects: self.objects.as_ref(),
                maps: self.objects.as_ref(),
                budget: &mut budget(),
            },
        )?
        .entries)
    }

    fn literal_hits(&self, view: ViewRef, term: &[u8]) -> TestResult<Vec<OwnerKey>> {
        let ticket = self
            .engine
            .pin_view(ViewSelection::Exact(view), &budget())?;
        let member = self.member(view, literal_spec().instance)?;
        let page = NativeLiteralIndex.search(
            &member,
            LiteralQuery {
                literal: term,
                after: None,
                limit: 16,
            },
            LiteralReadContext {
                ticket: ticket.record(),
                input: member.input,
                objects: self.objects.as_ref(),
                maps: self.objects.as_ref(),
                budget: &mut budget(),
            },
        )?;
        Ok(page.hits.into_iter().map(|hit| hit.owner).collect())
    }
}

struct IngressSink<'a> {
    catalog: &'a SqliteCatalog,
    ingress: IngressRef,
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

fn admit_without_driving(
    fixture: &Fixture,
    ingress: IngressRef,
    request: &UpdateRequest,
) -> TestResult {
    let UpdateKind::SourceAdvance(transition) = &request.kind else {
        return Err("manual admission requires SourceAdvance".into());
    };
    let wire = serde_json::to_vec(request)?;
    let stored = fixture.objects.install(
        &wire,
        &transition.owners.object_children(),
        &IngressSink {
            catalog: fixture.catalog.as_ref(),
            ingress,
        },
        &mut budget(),
    )?;
    fixture.catalog.seal_inputs(ingress, stored.reference)?;
    let admitted = fixture.catalog.admit(
        ImmutableOperation {
            key: request.operation,
            request: stored.reference,
            source_mode: request.kind.source_mode(),
        },
        ingress,
        &transition.events,
        &mut budget(),
    )?;
    check!(
        matches!(admitted, AdmissionOutcome::New(_)),
        "source claim was not durably admitted"
    );
    check!(
        matches!(
            fixture.catalog.inspect(
                request.operation,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
            )?,
            OperationStatus::Pending(_)
        ),
        "admitted source operation did not remain pending"
    );
    Ok(())
}

#[expect(
    clippy::disallowed_methods,
    reason = "test-only synchronous driver exercises the real Kernel future"
)]
fn submit(
    engine: &Kernel<SqliteCatalog>,
    request: UpdateRequest,
) -> Result<SubmitOutcome, IndexError> {
    futures::executor::block_on(engine.submit(request, &mut budget()))
}

#[expect(
    clippy::disallowed_methods,
    reason = "test-only synchronous driver exercises the real Kernel recovery future"
)]
fn resume(
    engine: &Kernel<SqliteCatalog>,
    operation: OperationKey,
) -> Result<SubmitOutcome, IndexError> {
    futures::executor::block_on(engine.resume(operation, &mut budget()))
}

fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other @ (SubmitOutcome::Accepted(_)
        | SubmitOutcome::Terminal(_)
        | SubmitOutcome::Unknown(_)) => {
            Err(format!("expected published view, got {other:?}").into())
        }
    }
}

fn capture_payload(request: &UpdateRequest) -> TestResult<Digest> {
    let UpdateKind::SourceAdvance(transition) = &request.kind else {
        return Err("capture payload requires SourceAdvance".into());
    };
    Ok(transition.owner_digest()?)
}

type DurableFrontierRow = (Vec<u8>, Vec<u8>, Vec<u8>, i64, i64, Vec<u8>);

fn frontier_rows(catalog: &SqliteCatalog) -> TestResult<Vec<DurableFrontierRow>> {
    let connection = rusqlite::Connection::open_with_flags(
        catalog.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut statement = connection.prepare(
        "SELECT workspace,producer,stream,stream_epoch,sequence,row_digest \
         FROM native_frontier_v1 ORDER BY workspace,producer,stream",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        ))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn latest_view(fixture: &Fixture) -> TestResult<ViewRef> {
    Ok(fixture
        .engine
        .pin_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?
        .view()
        .identity)
}

fn run_unrelated_source_rebase() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = Fixture::open(root.path())?;
    let configured = fixture.configure()?;
    let initial_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    check!(
        initial_frontiers.is_empty(),
        "configured root advanced a source frontier"
    );
    let primary = owner(5, 1);
    let secondary = owner(7, 1);
    let old_key = key(5, 6, 2);
    let (old_ingress, old_request, primary_record) = fixture.capture(
        old_key,
        configured,
        primary,
        None,
        "alpha.rs",
        b"alpha unique",
        1,
    )?;
    admit_without_driving(&fixture, old_ingress, &old_request)?;
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        initial_frontiers,
        "pending admission advanced source frontier"
    );

    let (_pending_ingress, pending_request, _record) = fixture.capture(
        key(5, 6, 3),
        configured,
        primary,
        None,
        "alpha.rs",
        b"alpha unique",
        1,
    )?;
    check_eq!(
        capture_payload(&pending_request)?,
        capture_payload(&old_request)?,
        "same capture changed payload"
    );
    check!(
        matches!(
            submit(&fixture.engine, pending_request),
            Err(IndexError::Conflict)
        ),
        "another operation stole a pending event claim"
    );
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        initial_frontiers,
        "pending takeover changed source frontier"
    );

    let (_secondary_ingress, secondary_request, secondary_record) = fixture.capture(
        key(7, 8, 1),
        configured,
        secondary,
        None,
        "beta.rs",
        b"beta unique",
        1,
    )?;
    let secondary_view = published(submit(&fixture.engine, secondary_request)?)?;
    let secondary_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    check_eq!(
        secondary_frontiers.len(),
        1,
        "unrelated producer frontier missing"
    );
    let unrelated_frontier = secondary_frontiers
        .first()
        .ok_or("unrelated frontier absent")?;
    check_eq!(
        unrelated_frontier.1.as_slice(),
        &[7; 16],
        "unrelated producer frontier identity changed"
    );
    check_eq!(
        unrelated_frontier.2.as_slice(),
        &[8; 16],
        "unrelated stream frontier identity changed"
    );
    check_eq!(
        (unrelated_frontier.3, unrelated_frontier.4),
        (1, 1),
        "unrelated frontier ordinal changed"
    );
    check_eq!(
        fixture.owner_record(secondary_view, secondary)?,
        Some(secondary_record),
        "secondary owner missing"
    );
    check_eq!(
        fixture.owner_record(secondary_view, primary)?,
        None,
        "pending primary owner became visible"
    );

    check_eq!(
        resume(&fixture.engine, old_key)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "stale admitted operation did not retain Conflict"
    );
    check_eq!(
        fixture.catalog.inspect(
            old_key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        OperationStatus::Refused(Refusal::Conflict),
        "old receipt changed"
    );
    check_eq!(
        fixture.owner_record(secondary_view, primary)?,
        None,
        "refused primary owner became visible"
    );
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        secondary_frontiers,
        "old Conflict advanced source frontier"
    );

    let (_changed_ingress, changed_request, _changed_record) = fixture.capture(
        key(5, 6, 4),
        secondary_view,
        primary,
        None,
        "alpha.rs",
        b"changed capture",
        1,
    )?;
    check!(
        matches!(
            submit(&fixture.engine, changed_request),
            Err(IndexError::DigestConflict)
        ),
        "refused event accepted a changed mutation digest"
    );
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        secondary_frontiers,
        "changed payload advanced source frontier"
    );

    let (_rebase_ingress, rebased_request, rebased_record) = fixture.capture(
        key(5, 6, 5),
        secondary_view,
        primary,
        None,
        "alpha.rs",
        b"alpha unique",
        1,
    )?;
    check_eq!(
        rebased_record,
        primary_record,
        "rebase changed captured source record"
    );
    check_eq!(
        capture_payload(&rebased_request)?,
        capture_payload(&old_request)?,
        "rebase changed logical event payload"
    );
    let current = published(submit(&fixture.engine, rebased_request.clone())?)?;
    let published_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    check_eq!(
        published_frontiers.len(),
        2,
        "rebased publication did not advance one frontier"
    );
    check!(
        published_frontiers
            .iter()
            .any(|row| row.1.as_slice() == [5; 16]
                && row.2.as_slice() == [6; 16]
                && (row.3, row.4) == (1, 1)),
        "rebased primary frontier missing"
    );
    check_eq!(
        fixture.owner_record(current, primary)?,
        Some(primary_record),
        "rebased primary owner missing"
    );
    check_eq!(
        fixture.owner_record(current, secondary)?,
        Some(secondary_record),
        "unrelated owner lost"
    );
    check_eq!(
        fixture.owner_record(secondary_view, primary)?,
        None,
        "historical source changed"
    );
    check_eq!(
        fixture.literal_hits(secondary_view, b"alpha")?,
        Vec::<OwnerKey>::new(),
        "old view saw late literal"
    );
    check_eq!(
        fixture.literal_hits(current, b"alpha")?,
        vec![primary],
        "rebased literal missing"
    );
    check_eq!(
        fixture.literal_hits(current, b"beta")?,
        vec![secondary],
        "unrelated literal lost"
    );
    let old_paths = fixture.paths(secondary_view)?;
    check_eq!(old_paths.len(), 1, "old path count changed");
    let old_path = old_paths.first().ok_or("old path absent")?;
    check_eq!(old_path.path.as_str(), "beta.rs", "old path changed");
    check_eq!(old_path.owner, secondary, "old path owner changed");
    check_eq!(old_path.source, secondary_record, "old path source changed");
    let current_paths = fixture.paths(current)?;
    check_eq!(current_paths.len(), 2, "rebased path count changed");
    let first = current_paths.first().ok_or("first current path absent")?;
    let second = current_paths.get(1).ok_or("second current path absent")?;
    check_eq!(first.path.as_str(), "alpha.rs", "primary path missing");
    check_eq!(first.owner, primary, "primary path owner changed");
    check_eq!(first.source, primary_record, "primary path source changed");
    check_eq!(second.path.as_str(), "beta.rs", "secondary path changed");
    check_eq!(second.owner, secondary, "secondary path owner changed");
    check_eq!(
        second.source,
        secondary_record,
        "secondary path source changed"
    );

    check_eq!(
        submit(&fixture.engine, rebased_request)?,
        SubmitOutcome::Terminal(OperationStatus::Published(current)),
        "published ACK replay changed"
    );
    check_eq!(
        submit(&fixture.engine, old_request.clone())?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "original refused receipt changed"
    );
    let (_duplicate_ingress, duplicate_request, duplicate_record) = fixture.capture(
        key(5, 6, 6),
        current,
        primary,
        None,
        "alpha.rs",
        b"alpha unique",
        1,
    )?;
    check_eq!(
        duplicate_record,
        primary_record,
        "duplicate capture changed source record"
    );
    check_eq!(
        capture_payload(&duplicate_request)?,
        capture_payload(&old_request)?,
        "duplicate event payload changed"
    );
    check!(
        matches!(
            submit(&fixture.engine, duplicate_request),
            Err(IndexError::Conflict)
        ),
        "committed event was claimed again"
    );
    check_eq!(
        latest_view(&fixture)?,
        current,
        "committed duplicate advanced Latest"
    );
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        published_frontiers,
        "committed duplicate advanced source frontier"
    );
    check_eq!(
        fixture.catalog.inspect(
            old_key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        OperationStatus::Refused(Refusal::Conflict),
        "old receipt lost after duplicate"
    );
    drop(fixture);

    let reopened = Fixture::open(root.path())?;
    check_eq!(
        frontier_rows(reopened.catalog.as_ref())?,
        published_frontiers,
        "reopen changed durable frontiers"
    );
    check_eq!(
        reopened.owner_record(secondary_view, primary)?,
        None,
        "reopen changed old source"
    );
    check_eq!(
        reopened.owner_record(current, primary)?,
        Some(primary_record),
        "reopen lost rebased owner"
    );
    check_eq!(
        reopened.owner_record(current, secondary)?,
        Some(secondary_record),
        "reopen lost unrelated owner"
    );
    check_eq!(
        reopened.literal_hits(current, b"alpha")?,
        vec![primary],
        "reopen lost rebased literal"
    );
    check_eq!(
        reopened.paths(current)?.len(),
        2,
        "reopen lost path inventory"
    );
    check_eq!(
        reopened.catalog.inspect(
            old_key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        OperationStatus::Refused(Refusal::Conflict),
        "reopen changed original receipt"
    );
    check_eq!(
        submit(&reopened.engine, old_request)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "reopen failed to replay original refused operation"
    );
    check_eq!(
        latest_view(&reopened)?,
        current,
        "reopen replay changed Latest"
    );
    check_eq!(
        frontier_rows(reopened.catalog.as_ref())?,
        published_frontiers,
        "reopen replay changed source frontiers"
    );
    Ok(())
}

#[test]
fn refused_source_event_rebases_after_unrelated_producer_publication() -> TestResult {
    run_unrelated_source_rebase()
}

fn run_touched_owner_refusal() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = Fixture::open(root.path())?;
    let configured = fixture.configure()?;
    let initial_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    let changed_owner = owner(5, 1);
    let old_key = key(5, 6, 2);
    let (old_ingress, old_request, _old_record) = fixture.capture(
        old_key,
        configured,
        changed_owner,
        None,
        "same.rs",
        b"first capture",
        1,
    )?;
    admit_without_driving(&fixture, old_ingress, &old_request)?;
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        initial_frontiers,
        "pending touched-owner claim advanced frontier"
    );
    let (_other_ingress, other_request, other_record) = fixture.capture(
        key(5, 8, 1),
        configured,
        changed_owner,
        None,
        "same.rs",
        b"concurrent stream wrote owner",
        1,
    )?;
    let current = published(submit(&fixture.engine, other_request)?)?;
    let occupied_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    check_eq!(
        occupied_frontiers.len(),
        1,
        "touched-owner baseline frontier missing"
    );
    check_eq!(
        resume(&fixture.engine, old_key)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "stale owner operation did not refuse"
    );
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        occupied_frontiers,
        "old touched-owner Conflict advanced frontier"
    );
    let (_rebased_ingress, rebased_request, _record) = fixture.capture(
        key(5, 6, 3),
        current,
        changed_owner,
        None,
        "same.rs",
        b"first capture",
        1,
    )?;
    check_eq!(
        capture_payload(&rebased_request)?,
        capture_payload(&old_request)?,
        "touched-owner retry changed capture"
    );
    check_eq!(
        submit(&fixture.engine, rebased_request)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "touched owner was silently overwritten"
    );
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        occupied_frontiers,
        "rebased touched-owner refusal advanced frontier"
    );
    check_eq!(
        fixture.owner_record(current, changed_owner)?,
        Some(other_record),
        "touched owner changed on refusal"
    );
    let latest = fixture.engine.pin_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    check_eq!(
        latest.view().identity,
        current,
        "head changed after touched-owner refusal"
    );
    latest.release()?;
    Ok(())
}

#[test]
fn same_event_rebase_refuses_a_touched_owner_without_advancing_head() -> TestResult {
    run_touched_owner_refusal()
}

fn run_path_collision_refusal() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = Fixture::open(root.path())?;
    let configured = fixture.configure()?;
    let initial_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    let primary = owner(5, 1);
    let secondary = owner(7, 1);
    let old_key = key(5, 6, 2);
    let (old_ingress, old_request, _old_record) = fixture.capture(
        old_key,
        configured,
        primary,
        None,
        "collision.rs",
        b"first capture",
        1,
    )?;
    admit_without_driving(&fixture, old_ingress, &old_request)?;
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        initial_frontiers,
        "pending path claim advanced frontier"
    );
    let (_other_ingress, other_request, secondary_record) = fixture.capture(
        key(7, 8, 1),
        configured,
        secondary,
        None,
        "collision.rs",
        b"other source",
        1,
    )?;
    let current = published(submit(&fixture.engine, other_request)?)?;
    let occupied_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    check_eq!(
        occupied_frontiers.len(),
        1,
        "path baseline frontier missing"
    );
    check_eq!(
        resume(&fixture.engine, old_key)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "stale path operation did not refuse"
    );
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        occupied_frontiers,
        "old path Conflict advanced frontier"
    );
    let (_rebased_ingress, rebased_request, _record) = fixture.capture(
        key(5, 6, 3),
        current,
        primary,
        None,
        "collision.rs",
        b"first capture",
        1,
    )?;
    check_eq!(
        capture_payload(&rebased_request)?,
        capture_payload(&old_request)?,
        "path retry changed captured event"
    );
    check_eq!(
        submit(&fixture.engine, rebased_request)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "path collision was silently overwritten"
    );
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        occupied_frontiers,
        "rebased path collision advanced frontier"
    );
    let latest = fixture.engine.pin_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    check_eq!(
        latest.view().identity,
        current,
        "path refusal advanced the head"
    );
    check_eq!(
        fixture.owner_record(current, primary)?,
        None,
        "path refusal installed primary owner"
    );
    check_eq!(
        fixture.owner_record(current, secondary)?,
        Some(secondary_record),
        "path refusal removed secondary owner"
    );
    let paths = fixture.paths(current)?;
    check_eq!(
        paths.len(),
        1,
        "path collision changed inventory cardinality"
    );
    let path = paths.first().ok_or("path collision baseline absent")?;
    check_eq!(
        path.path.as_str(),
        "collision.rs",
        "path collision changed path"
    );
    check_eq!(path.owner, secondary, "path collision changed owner");
    check_eq!(
        path.source,
        secondary_record,
        "path collision changed source"
    );
    latest.release()?;
    Ok(())
}

#[test]
fn same_event_rebase_refuses_a_path_collision_without_advancing_head() -> TestResult {
    run_path_collision_refusal()
}

#[test]
fn one_transition_path_handoff_preserves_exact_owners_and_cold_reopen() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = Fixture::open(root.path())?;
    let configured = fixture.configure()?;
    let first_owner = owner(5, 2);
    let lower_owner = owner(5, 1);
    let higher_owner = owner(5, 3);
    let stale_owner = owner(5, 4);
    check!(
        lower_owner < first_owner && higher_owner > lower_owner,
        "handoff fixture does not cover both mutation orders"
    );
    let (_first_ingress, first_request, first_record) = fixture.capture(
        key(5, 6, 2),
        configured,
        first_owner,
        None,
        "handoff.rs",
        b"first anchor",
        1,
    )?;
    let first_view = published(submit(&fixture.engine, first_request.clone())?)?;

    // Insert-before-delete in canonical owner order.
    let (_lower_ingress, lower_request, lower_record) = fixture.capture_path_handoff(
        key(5, 6, 3),
        first_view,
        first_owner,
        first_record,
        lower_owner,
        b"second anchor",
        2,
    )?;
    let lower_view = published(submit(&fixture.engine, lower_request.clone())?)?;

    // Admit a competing replacement from the old cut, then publish the
    // delete-before-insert transition on the original stream.
    let stale_key = key(5, 8, 1);
    let (stale_ingress, stale_request, _stale_record) = fixture.capture_path_handoff(
        stale_key,
        lower_view,
        lower_owner,
        lower_record,
        stale_owner,
        b"stale anchor",
        1,
    )?;
    admit_without_driving(&fixture, stale_ingress, &stale_request)?;
    let before_higher = frontier_rows(fixture.catalog.as_ref())?;
    check_eq!(
        before_higher.len(),
        1,
        "pending competing handoff advanced a source frontier"
    );
    let (_higher_ingress, higher_request, higher_record) = fixture.capture_path_handoff(
        key(5, 6, 4),
        lower_view,
        lower_owner,
        lower_record,
        higher_owner,
        b"third anchor",
        3,
    )?;
    let higher_view = published(submit(&fixture.engine, higher_request.clone())?)?;
    let published_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    check_eq!(
        published_frontiers.len(),
        1,
        "path handoff changed frontier cardinality"
    );
    let frontier = published_frontiers
        .first()
        .ok_or("path handoff frontier absent")?;
    check_eq!(
        (
            frontier.1.as_slice(),
            frontier.2.as_slice(),
            frontier.3,
            frontier.4
        ),
        ([5; 16].as_slice(), [6; 16].as_slice(), 1, 3),
        "path handoff source event ordinal changed"
    );
    check_eq!(
        resume(&fixture.engine, stale_key)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "stale handoff published after another owner took the path"
    );
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        published_frontiers,
        "stale handoff changed durable frontier"
    );
    check_eq!(
        latest_view(&fixture)?,
        higher_view,
        "stale handoff changed Latest"
    );

    for (view, expected_owner, expected_record) in [
        (first_view, first_owner, first_record),
        (lower_view, lower_owner, lower_record),
        (higher_view, higher_owner, higher_record),
    ] {
        let paths = fixture.paths(view)?;
        let [entry] = paths.as_slice() else {
            return Err("path handoff did not leave exactly one path".into());
        };
        check_eq!(entry.path.as_str(), "handoff.rs", "handoff path changed");
        check_eq!(entry.owner, expected_owner, "handoff path owner changed");
        check_eq!(entry.source, expected_record, "handoff source ref changed");
        check_eq!(
            fixture.owner_record(view, expected_owner)?,
            Some(expected_record),
            "handoff owner map differs from path map"
        );
    }
    check_eq!(
        fixture.owner_record(lower_view, first_owner)?,
        None,
        "first owner survived first handoff"
    );
    check_eq!(
        fixture.owner_record(higher_view, lower_owner)?,
        None,
        "lower owner survived second handoff"
    );
    check_eq!(
        fixture.owner_record(higher_view, stale_owner)?,
        None,
        "refused stale owner became visible"
    );
    check_eq!(
        fixture.literal_hits(first_view, b"first")?,
        vec![first_owner],
        "historical first content changed"
    );
    check_eq!(
        fixture.literal_hits(lower_view, b"second")?,
        vec![lower_owner],
        "middle content changed"
    );
    check_eq!(
        fixture.literal_hits(higher_view, b"third")?,
        vec![higher_owner],
        "current content changed"
    );
    check!(
        fixture.literal_hits(higher_view, b"first")?.is_empty(),
        "deleted first content remains current"
    );
    drop(fixture);

    let reopened = Fixture::open(root.path())?;
    check_eq!(
        frontier_rows(reopened.catalog.as_ref())?,
        published_frontiers,
        "reopen changed handoff frontier"
    );
    for (view, expected_owner, expected_record) in [
        (first_view, first_owner, first_record),
        (lower_view, lower_owner, lower_record),
        (higher_view, higher_owner, higher_record),
    ] {
        let paths = reopened.paths(view)?;
        let [entry] = paths.as_slice() else {
            return Err("reopen changed handoff path cardinality".into());
        };
        check_eq!(entry.path.as_str(), "handoff.rs", "reopen changed path");
        check_eq!(entry.owner, expected_owner, "reopen changed path owner");
        check_eq!(entry.source, expected_record, "reopen changed path ref");
    }
    check_eq!(
        submit(&reopened.engine, first_request)?,
        SubmitOutcome::Terminal(OperationStatus::Published(first_view)),
        "reopen changed first handoff prerequisite receipt"
    );
    check_eq!(
        submit(&reopened.engine, lower_request)?,
        SubmitOutcome::Terminal(OperationStatus::Published(lower_view)),
        "reopen changed lower handoff receipt"
    );
    check_eq!(
        submit(&reopened.engine, higher_request)?,
        SubmitOutcome::Terminal(OperationStatus::Published(higher_view)),
        "reopen changed higher handoff receipt"
    );
    check_eq!(
        submit(&reopened.engine, stale_request)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "reopen changed stale refusal receipt"
    );
    check_eq!(
        latest_view(&reopened)?,
        higher_view,
        "reopen replay changed Latest"
    );
    check_eq!(
        frontier_rows(reopened.catalog.as_ref())?,
        published_frontiers,
        "reopen replay changed frontier"
    );
    Ok(())
}
