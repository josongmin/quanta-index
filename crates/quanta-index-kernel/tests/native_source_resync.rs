//! Producer-wide Native source reset with a complete known-owner baseline.
//! The fixture uses managed ingress, immutable objects, the real catalog, and
//! independently asserted path/literal results at historical exact views.

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
        PersistentMapPort, SourceAuthorityPort,
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

fn budget() -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        },
        quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
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

fn operation(producer: u8, stream: u8, stream_epoch: u64, id: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: ProducerId([producer; 16]),
        stream: StreamId([stream; 16]),
        stream_epoch,
        sequence: id,
    }
}

fn owner(producer: u8, id: u8) -> OwnerKey {
    OwnerKey {
        producer: ProducerId([producer; 16]),
        stable_id: StableOwnerId([id; 16]),
    }
}

fn lexical_spec() -> InstanceSpec {
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
        ..lexical_spec()
    }
}

struct Fixture {
    engine: Kernel<SqliteCatalog>,
    objects: Arc<FileObjectStore>,
    catalog: Arc<SqliteCatalog>,
}

struct Change<'a> {
    owner: OwnerKey,
    expected: Option<ObjectRef>,
    next: Option<(&'a str, &'a [u8])>,
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
                kind: lexical_spec().kind,
                schema: lexical_spec().schema,
                format: lexical_spec().format,
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
        let operation = operation(1, 1, 1, 1);
        let ingress = self.engine.begin_input(operation)?;
        let profile = self.engine.stage_profile(
            ingress,
            &IndexProfile {
                instances: vec![lexical_spec(), file_spec()],
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
        view: ViewRef,
        changes: &[Change<'_>],
        predecessor: u64,
        sequence: u64,
        prior_epoch: Option<u64>,
    ) -> TestResult<(IngressRef, UpdateRequest, Vec<Option<ObjectRef>>)> {
        let ingress = self.engine.begin_input(operation)?;
        let mut mutations = Vec::with_capacity(changes.len());
        let mut records = Vec::with_capacity(changes.len());
        for change in changes {
            let next = match change.next {
                Some((path, bytes)) => Some(self.engine.stage_source_text(
                    ingress,
                    path,
                    bytes,
                    SourceTextAdmission::Utf8 {
                        policy: Digest([42; 32]),
                    },
                    &mut budget(),
                )?),
                None => None,
            };
            records.push(next);
            mutations.push(Ok(OwnerMutation {
                owner: change.owner,
                expected: change.expected,
                next,
            }));
        }
        let owners = self
            .engine
            .stage_mutations(ingress, mutations, &mut budget())?;
        let mut transition = SourceTransition {
            expected_source: self.source(view)?,
            owners,
            events: Vec::new(),
        };
        transition.events.push(SourceEvent {
            producer: operation.producer,
            stream: operation.stream,
            stream_epoch: operation.stream_epoch,
            predecessor,
            sequence,
            payload: transition.owner_digest()?,
        });
        let kind = match prior_epoch {
            Some(prior_epoch) => UpdateKind::SourceResync {
                prior_epoch,
                transition,
            },
            None => UpdateKind::SourceAdvance(transition),
        };
        Ok((
            ingress,
            UpdateRequest {
                operation,
                expected_view: view,
                kind,
            },
            records,
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
            .ok_or("required member absent")?;
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

    fn path_count(&self, view: ViewRef) -> TestResult<usize> {
        let ticket = self
            .engine
            .pin_view(ViewSelection::Exact(view), &budget())?;
        let member = self.member(view, file_spec().instance)?;
        let mut after: Option<String> = None;
        let mut count = 0_usize;
        loop {
            let page = query_paths(
                &FilePathQuery {
                    member: &member,
                    prefix: "",
                    after: after.as_deref(),
                    limit: 64,
                },
                &mut FileMapReadContext {
                    ticket: ticket.record(),
                    objects: self.objects.as_ref(),
                    maps: self.objects.as_ref(),
                    budget: &mut budget(),
                },
            )?;
            count = count
                .checked_add(page.entries.len())
                .ok_or("path count overflow")?;
            if !page.has_more {
                return Ok(count);
            }
            after = Some(
                page.entries
                    .last()
                    .ok_or("file page has continuation but no entries")?
                    .path
                    .clone(),
            );
        }
    }

    fn path_exact(&self, view: ViewRef, path: &str) -> TestResult<Option<FilePathEntry>> {
        let ticket = self
            .engine
            .pin_view(ViewSelection::Exact(view), &budget())?;
        let member = self.member(view, file_spec().instance)?;
        let page = query_paths(
            &FilePathQuery {
                member: &member,
                prefix: path,
                after: None,
                limit: 1,
            },
            &mut FileMapReadContext {
                ticket: ticket.record(),
                objects: self.objects.as_ref(),
                maps: self.objects.as_ref(),
                budget: &mut budget(),
            },
        )?;
        Ok(page.entries.into_iter().find(|entry| entry.path == path))
    }

    fn hits(&self, view: ViewRef, term: &[u8]) -> TestResult<Vec<OwnerKey>> {
        let ticket = self
            .engine
            .pin_view(ViewSelection::Exact(view), &budget())?;
        let member = self.member(view, lexical_spec().instance)?;
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
    let (UpdateKind::SourceAdvance(transition) | UpdateKind::SourceResync { transition, .. }) =
        &request.kind
    else {
        return Err("manual admission requires a source transition".into());
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
        "source operation was not admitted"
    );
    check!(
        matches!(
            fixture.catalog.inspect(
                request.operation,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
            )?,
            OperationStatus::Pending(_)
        ),
        "source admission did not remain Pending"
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
        | SubmitOutcome::Unknown(_)) => Err(format!("expected Published, got {other:?}").into()),
    }
}

fn latest(fixture: &Fixture) -> TestResult<ViewRef> {
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

type FrontierRow = (Vec<u8>, Vec<u8>, Vec<u8>, i64, i64, Vec<u8>);

fn frontier_rows(catalog: &SqliteCatalog) -> TestResult<Vec<FrontierRow>> {
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

type AuthorityRow = (Vec<u8>, i64, Vec<u8>, i64, Vec<u8>);

fn authority_rows(catalog: &SqliteCatalog) -> TestResult<Vec<AuthorityRow>> {
    let connection = rusqlite::Connection::open_with_flags(
        catalog.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut statement = connection.prepare(
        "SELECT workspace,workspace_epoch,producer,source_epoch,row_digest \
         FROM native_source_authority_v1 ORDER BY workspace,workspace_epoch,producer",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn sql_epoch(rows: &[AuthorityRow], producer: u8) -> Option<i64> {
    rows.iter()
        .find(|row| row.2.as_slice() == [producer; 16])
        .map(|row| row.3)
}

fn event_claim_count(catalog: &SqliteCatalog) -> TestResult<i64> {
    let connection = rusqlite::Connection::open_with_flags(
        catalog.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    Ok(
        connection.query_row("SELECT COUNT(*) FROM native_event_claim_v1", [], |row| {
            row.get(0)
        })?,
    )
}

fn first_record(records: &[Option<ObjectRef>]) -> TestResult<ObjectRef> {
    records
        .first()
        .copied()
        .flatten()
        .ok_or_else(|| "first source record absent".into())
}

fn path_is(
    fixture: &Fixture,
    view: ViewRef,
    expected: &[(&str, OwnerKey, ObjectRef)],
) -> TestResult {
    let paths = fixture.paths(view)?;
    check_eq!(
        paths.len(),
        expected.len(),
        "path count differs from fixed gold"
    );
    for (actual, (name, owner, source)) in paths.iter().zip(expected) {
        check_eq!(actual.path.as_str(), *name, "path name differs from gold");
        check_eq!(actual.owner, *owner, "path owner differs from gold");
        check_eq!(actual.source, *source, "path source differs from gold");
    }
    Ok(())
}

#[test]
fn first_pending_capture_refused_then_resyncs_without_a_published_frontier() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = Fixture::open(root.path())?;
    let configured = fixture.configure()?;
    let primary = owner(5, 1);
    let intruder = owner(7, 1);
    let old_operation = operation(5, 6, 1, 1);
    let (old_ingress, old_request, _old_records) = fixture.capture(
        old_operation,
        configured,
        &[Change {
            owner: primary,
            expected: None,
            next: Some(("occupied.rs", b"old never published")),
        }],
        0,
        1,
        None,
    )?;
    admit_without_driving(&fixture, old_ingress, &old_request)?;
    check_eq!(
        sql_epoch(&authority_rows(fixture.catalog.as_ref())?, 5),
        Some(1),
        "first Pending capture did not own producer epoch one"
    );
    check!(
        frontier_rows(fixture.catalog.as_ref())?.is_empty(),
        "first Pending capture published a frontier"
    );

    let (_, intruder_request, intruder_records) = fixture.capture(
        operation(7, 8, 1, 1),
        configured,
        &[Change {
            owner: intruder,
            expected: None,
            next: Some(("occupied.rs", b"intruder literal")),
        }],
        0,
        1,
        None,
    )?;
    let occupied = published(submit(&fixture.engine, intruder_request)?)?;
    let intruder_record = first_record(&intruder_records)?;
    check_eq!(
        resume(&fixture.engine, old_operation)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "old conflicting Pending capture did not become Refused"
    );
    let pre_reset_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    check_eq!(
        pre_reset_frontiers.len(),
        1,
        "unpublished producer acquired a frontier"
    );
    check_eq!(
        fixture
            .catalog
            .source_epoch(occupied, ProducerId([5; 16]), &mut budget())?,
        Some(1),
        "producer authority omitted its admitted first epoch"
    );

    let (_, reset_request, reset_records) = fixture.capture(
        operation(5, 6, 2, 9),
        occupied,
        &[Change {
            owner: primary,
            expected: None,
            next: Some(("recovered.rs", b"resynced literal")),
        }],
        0,
        1,
        Some(1),
    )?;
    let recovered_record = first_record(&reset_records)?;
    let recovered = published(submit(&fixture.engine, reset_request)?)?;
    check_eq!(
        sql_epoch(&authority_rows(fixture.catalog.as_ref())?, 5),
        Some(2),
        "resync did not durably advance producer epoch"
    );
    check_eq!(
        fixture.owner_record(recovered, primary)?,
        Some(recovered_record),
        "new producer baseline owner missing"
    );
    check_eq!(
        fixture.owner_record(recovered, intruder)?,
        Some(intruder_record),
        "other producer owner changed"
    );
    path_is(
        &fixture,
        recovered,
        &[
            ("occupied.rs", intruder, intruder_record),
            ("recovered.rs", primary, recovered_record),
        ],
    )?;
    check_eq!(
        fixture.hits(recovered, b"resynced")?,
        vec![primary],
        "resync literal absent"
    );
    check_eq!(
        fixture.hits(occupied, b"resynced")?,
        Vec::<OwnerKey>::new(),
        "historical exact view saw future text"
    );
    check_eq!(
        submit(&fixture.engine, old_request.clone())?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "original Refused receipt changed after resync"
    );

    let (_, next_request, next_records) = fixture.capture(
        operation(5, 6, 2, 10),
        recovered,
        &[Change {
            owner: primary,
            expected: Some(recovered_record),
            next: Some(("successor.rs", b"successor literal")),
        }],
        1,
        2,
        None,
    )?;
    let successor_record = first_record(&next_records)?;
    let next = published(submit(&fixture.engine, next_request)?)?;
    check_eq!(
        fixture.owner_record(next, primary)?,
        Some(successor_record),
        "normal sequence two after resync failed"
    );
    check_eq!(
        fixture.hits(next, b"successor")?,
        vec![primary],
        "post-resync ordinal did not index text"
    );
    let final_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    check!(
        final_frontiers.iter().any(|row| {
            row.1.as_slice() == [5; 16] && row.2.as_slice() == [6; 16] && (row.3, row.4) == (2, 2)
        }),
        "new producer frontier did not reach epoch two sequence two"
    );
    drop(fixture);

    let reopened = Fixture::open(root.path())?;
    check_eq!(
        reopened.catalog.inspect(
            old_operation,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        OperationStatus::Refused(Refusal::Conflict),
        "reopen changed original Refused operation"
    );
    check_eq!(
        submit(&reopened.engine, old_request)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "reopen did not replay immutable Refused receipt"
    );
    check_eq!(
        frontier_rows(reopened.catalog.as_ref())?,
        final_frontiers,
        "reopen changed source frontiers"
    );
    check_eq!(
        sql_epoch(&authority_rows(reopened.catalog.as_ref())?, 5),
        Some(2),
        "reopen lost producer epoch"
    );
    check_eq!(latest(&reopened)?, next, "reopen changed Latest");
    Ok(())
}

#[test]
fn resync_replaces_a_complete_owner_prefix_and_fences_every_old_stream() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = Fixture::open(root.path())?;
    let configured = fixture.configure()?;
    let alpha = owner(5, 1);
    let bravo = owner(5, 2);
    let charlie = owner(5, 3);

    let (_, first_request, first_records) = fixture.capture(
        operation(5, 6, 1, 1),
        configured,
        &[
            Change {
                owner: alpha,
                expected: None,
                next: Some(("alpha.rs", b"alpha original")),
            },
            Change {
                owner: bravo,
                expected: None,
                next: Some(("bravo.rs", b"bravo original")),
            },
        ],
        0,
        1,
        None,
    )?;
    let first = published(submit(&fixture.engine, first_request.clone())?)?;
    let alpha_record = first_record(&first_records)?;
    let bravo_record = first_records
        .get(1)
        .copied()
        .flatten()
        .ok_or("bravo source record absent")?;
    let (_, second_request, second_records) = fixture.capture(
        operation(5, 8, 1, 1),
        first,
        &[Change {
            owner: charlie,
            expected: None,
            next: Some(("charlie.rs", b"charlie unchanged")),
        }],
        0,
        1,
        None,
    )?;
    let baseline = published(submit(&fixture.engine, second_request.clone())?)?;
    let charlie_record = first_record(&second_records)?;
    path_is(
        &fixture,
        baseline,
        &[
            ("alpha.rs", alpha, alpha_record),
            ("bravo.rs", bravo, bravo_record),
            ("charlie.rs", charlie, charlie_record),
        ],
    )?;
    let pre_reset_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    check_eq!(
        pre_reset_frontiers.len(),
        2,
        "baseline did not publish both source streams"
    );

    let old_pending = operation(5, 8, 1, 2);
    let (old_ingress, old_request, _old_records) = fixture.capture(
        old_pending,
        baseline,
        &[Change {
            owner: charlie,
            expected: Some(charlie_record),
            next: Some(("obsolete.rs", b"obsolete pending")),
        }],
        1,
        2,
        None,
    )?;
    admit_without_driving(&fixture, old_ingress, &old_request)?;

    // Every known producer owner is enumerated, including a deletion and an
    // explicit unchanged row. The latter must retain its exact ObjectRef.
    let (reset_ingress, reset_request, reset_records) = fixture.capture(
        operation(5, 6, 2, 1),
        baseline,
        &[
            Change {
                owner: alpha,
                expected: Some(alpha_record),
                next: Some(("alpha-new.rs", b"alpha replacement")),
            },
            Change {
                owner: bravo,
                expected: Some(bravo_record),
                next: None,
            },
            Change {
                owner: charlie,
                expected: Some(charlie_record),
                next: Some(("charlie.rs", b"charlie unchanged")),
            },
        ],
        0,
        1,
        Some(1),
    )?;
    let alpha_replacement = first_record(&reset_records)?;
    check_eq!(
        reset_records.get(2).copied().flatten(),
        Some(charlie_record),
        "unchanged row did not retain the original ObjectRef"
    );
    admit_without_driving(&fixture, reset_ingress, &reset_request)?;
    check_eq!(
        sql_epoch(&authority_rows(fixture.catalog.as_ref())?, 5),
        Some(1),
        "Pending reset prematurely exposed the next producer epoch"
    );
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        pre_reset_frontiers,
        "Pending reset prematurely changed source frontiers"
    );
    check_eq!(latest(&fixture)?, baseline, "Pending reset advanced Latest");
    let (_, premature_new_stream, _) = fixture.capture(
        operation(5, 10, 2, 1),
        baseline,
        &[Change {
            owner: charlie,
            expected: Some(charlie_record),
            next: Some(("premature.rs", b"premature new epoch")),
        }],
        0,
        1,
        None,
    )?;
    check!(
        matches!(
            submit(&fixture.engine, premature_new_stream),
            Err(IndexError::Conflict)
        ),
        "another stream used the Pending reset epoch before publication"
    );
    check_eq!(
        sql_epoch(&authority_rows(fixture.catalog.as_ref())?, 5),
        Some(1),
        "premature new-stream probe changed producer epoch"
    );
    check_eq!(
        latest(&fixture)?,
        baseline,
        "premature stream changed Latest"
    );
    let reset = published(resume(&fixture.engine, reset_request.operation)?)?;
    check_eq!(
        sql_epoch(&authority_rows(fixture.catalog.as_ref())?, 5),
        Some(2),
        "producer epoch did not advance once"
    );
    check_eq!(
        fixture.owner_record(reset, alpha)?,
        Some(alpha_replacement),
        "replacement owner absent"
    );
    check_eq!(
        fixture.owner_record(reset, bravo)?,
        None,
        "deleted owner survived resync"
    );
    check_eq!(
        fixture.owner_record(reset, charlie)?,
        Some(charlie_record),
        "explicit unchanged owner was lost"
    );
    path_is(
        &fixture,
        reset,
        &[
            ("alpha-new.rs", alpha, alpha_replacement),
            ("charlie.rs", charlie, charlie_record),
        ],
    )?;
    path_is(
        &fixture,
        baseline,
        &[
            ("alpha.rs", alpha, alpha_record),
            ("bravo.rs", bravo, bravo_record),
            ("charlie.rs", charlie, charlie_record),
        ],
    )?;
    check_eq!(
        fixture.hits(reset, b"bravo")?,
        Vec::<OwnerKey>::new(),
        "deleted source remains searchable at current view"
    );
    check_eq!(
        fixture.hits(baseline, b"bravo")?,
        vec![bravo],
        "historical exact view lost deleted source"
    );
    check_eq!(
        fixture.hits(reset, b"alpha replacement")?,
        vec![alpha],
        "replacement source not searchable"
    );
    check_eq!(
        fixture.hits(reset, b"charlie unchanged")?,
        vec![charlie],
        "unchanged source not searchable"
    );
    let reset_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    check!(
        reset_frontiers.iter().any(|row| {
            row.1.as_slice() == [5; 16] && row.2.as_slice() == [6; 16] && (row.3, row.4) == (2, 1)
        }),
        "reset stream did not restart at epoch two sequence one"
    );
    check!(
        reset_frontiers.iter().any(|row| {
            row.1.as_slice() == [5; 16] && row.2.as_slice() == [8; 16] && (row.3, row.4) == (1, 1)
        }),
        "old second-stream history was deleted instead of fenced"
    );

    drop(fixture);
    let reopened = Fixture::open(root.path())?;
    check_eq!(
        submit(&reopened.engine, first_request)?,
        SubmitOutcome::Terminal(OperationStatus::Published(first)),
        "old Published first-stream receipt changed after reset"
    );
    check_eq!(
        submit(&reopened.engine, second_request)?,
        SubmitOutcome::Terminal(OperationStatus::Published(baseline)),
        "old Published second-stream receipt changed after reset"
    );
    check_eq!(
        submit(&reopened.engine, reset_request)?,
        SubmitOutcome::Terminal(OperationStatus::Published(reset)),
        "ACK-loss replay changed resync receipt after restart"
    );
    check_eq!(
        resume(&reopened.engine, old_pending)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "old-epoch Pending operation survived producer fence"
    );
    check_eq!(
        submit(&reopened.engine, old_request)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "old-epoch Refused receipt changed"
    );
    let (_, stale_rebase, _) = reopened.capture(
        operation(5, 8, 1, 3),
        reset,
        &[Change {
            owner: charlie,
            expected: Some(charlie_record),
            next: Some(("obsolete.rs", b"obsolete pending")),
        }],
        1,
        2,
        None,
    )?;
    check!(
        matches!(
            submit(&reopened.engine, stale_rebase),
            Err(IndexError::Conflict)
        ),
        "fresh old-epoch operation was admitted after resync"
    );
    check_eq!(latest(&reopened)?, reset, "old-epoch replay changed Latest");
    check_eq!(
        frontier_rows(reopened.catalog.as_ref())?,
        reset_frontiers,
        "old-epoch replay changed frontier rows"
    );

    let (_, new_stream_request, new_stream_records) = reopened.capture(
        operation(5, 8, 2, 1),
        reset,
        &[Change {
            owner: charlie,
            expected: Some(charlie_record),
            next: Some(("charlie-new.rs", b"charlie second epoch")),
        }],
        0,
        1,
        None,
    )?;
    let new_stream_record = first_record(&new_stream_records)?;
    let next = published(submit(&reopened.engine, new_stream_request)?)?;
    check_eq!(
        reopened.owner_record(next, charlie)?,
        Some(new_stream_record),
        "second stream could not start its new producer epoch"
    );
    check_eq!(
        reopened.hits(next, b"charlie second")?,
        vec![charlie],
        "second stream new-epoch text missing"
    );
    check_eq!(
        sql_epoch(&authority_rows(reopened.catalog.as_ref())?, 5),
        Some(2),
        "normal second stream changed producer epoch"
    );
    Ok(())
}

fn unchanged_authority_and_head(
    fixture: &Fixture,
    head: ViewRef,
    frontiers: &[FrontierRow],
    authority: &[AuthorityRow],
) -> TestResult {
    check_eq!(latest(fixture)?, head, "rejected resync advanced Latest");
    check_eq!(
        frontier_rows(fixture.catalog.as_ref())?,
        frontiers,
        "rejected resync changed SQL frontiers"
    );
    check_eq!(
        authority_rows(fixture.catalog.as_ref())?,
        authority,
        "rejected resync changed producer epoch authority"
    );
    Ok(())
}

#[test]
fn incomplete_or_invalid_resync_never_advances_head_or_producer_authority() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = Fixture::open(root.path())?;
    let configured = fixture.configure()?;
    let alpha = owner(5, 1);
    let bravo = owner(5, 2);
    let (_, original, records) = fixture.capture(
        operation(5, 6, 1, 1),
        configured,
        &[
            Change {
                owner: alpha,
                expected: None,
                next: Some(("alpha.rs", b"alpha original")),
            },
            Change {
                owner: bravo,
                expected: None,
                next: Some(("bravo.rs", b"bravo original")),
            },
        ],
        0,
        1,
        None,
    )?;
    let head = published(submit(&fixture.engine, original)?)?;
    let alpha_record = first_record(&records)?;
    let bravo_record = records
        .get(1)
        .copied()
        .flatten()
        .ok_or("bravo source record absent")?;
    let frontiers = frontier_rows(fixture.catalog.as_ref())?;
    let authority = authority_rows(fixture.catalog.as_ref())?;
    check_eq!(
        sql_epoch(&authority, 5),
        Some(1),
        "baseline producer epoch differs from fixed gold"
    );

    // A reset must account for bravo even when bravo is to be deleted or
    // unchanged. Listing only alpha is not a complete known-owner prefix.
    let (_, incomplete, _) = fixture.capture(
        operation(5, 6, 2, 1),
        head,
        &[Change {
            owner: alpha,
            expected: Some(alpha_record),
            next: Some(("alpha-new.rs", b"alpha replacement")),
        }],
        0,
        1,
        Some(1),
    )?;
    check_eq!(
        submit(&fixture.engine, incomplete)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "incomplete owner baseline was not refused"
    );
    unchanged_authority_and_head(&fixture, head, &frontiers, &authority)?;
    check_eq!(
        fixture.owner_record(head, bravo)?,
        Some(bravo_record),
        "incomplete reset deleted an omitted owner"
    );

    let (_, wrong_prior, _) = fixture.capture(
        operation(5, 7, 3, 1),
        head,
        &[Change {
            owner: alpha,
            expected: Some(alpha_record),
            next: Some(("alpha-new.rs", b"alpha replacement")),
        }],
        0,
        1,
        Some(2),
    )?;
    check!(
        matches!(
            submit(&fixture.engine, wrong_prior),
            Err(IndexError::Conflict)
        ),
        "wrong prior epoch passed admission"
    );
    unchanged_authority_and_head(&fixture, head, &frontiers, &authority)?;

    let (_, nonincreasing, _) = fixture.capture(
        operation(5, 8, 1, 1),
        head,
        &[Change {
            owner: alpha,
            expected: Some(alpha_record),
            next: Some(("alpha-new.rs", b"alpha replacement")),
        }],
        0,
        1,
        Some(1),
    )?;
    check!(
        matches!(
            submit(&fixture.engine, nonincreasing),
            Err(IndexError::InvalidInput(_))
        ),
        "non-increasing producer epoch passed admission"
    );
    unchanged_authority_and_head(&fixture, head, &frontiers, &authority)?;

    let (_, mut multi_event, _) = fixture.capture(
        operation(5, 9, 2, 1),
        head,
        &[Change {
            owner: alpha,
            expected: Some(alpha_record),
            next: Some(("alpha-new.rs", b"alpha replacement")),
        }],
        0,
        1,
        Some(1),
    )?;
    let UpdateKind::SourceResync { transition, .. } = &mut multi_event.kind else {
        return Err("test reset request lost its SourceResync kind".into());
    };
    transition.events.push(SourceEvent {
        producer: ProducerId([5; 16]),
        stream: StreamId([10; 16]),
        stream_epoch: 2,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    check!(
        matches!(
            submit(&fixture.engine, multi_event),
            Err(IndexError::InvalidInput(_))
        ),
        "multi-event reset passed admission"
    );
    unchanged_authority_and_head(&fixture, head, &frontiers, &authority)?;

    let (_, absent_prior, _) = fixture.capture(
        operation(9, 11, 2, 1),
        head,
        &[Change {
            owner: owner(9, 1),
            expected: None,
            next: Some(("unowned.rs", b"unowned literal")),
        }],
        0,
        1,
        Some(1),
    )?;
    check!(
        matches!(
            submit(&fixture.engine, absent_prior),
            Err(IndexError::Conflict)
        ),
        "reset without prior producer authority passed admission"
    );
    unchanged_authority_and_head(&fixture, head, &frontiers, &authority)?;
    check_eq!(
        fixture.hits(head, b"alpha original")?,
        vec![alpha],
        "rejected reset changed historical literal"
    );
    check_eq!(
        fixture.hits(head, b"unowned")?,
        Vec::<OwnerKey>::new(),
        "rejected reset made an unowned source searchable"
    );
    Ok(())
}

#[test]
fn resync_crosses_owner_and_path_page_boundaries_without_stealing_neighbor_producers() -> TestResult
{
    let root = tempfile::tempdir()?;
    let fixture = Fixture::open(root.path())?;
    let configured = fixture.configure()?;
    let (_, low_request, low_records) = fixture.capture(
        operation(4, 2, 1, 1),
        configured,
        &[Change {
            owner: owner(4, 1),
            expected: None,
            next: Some(("outside-low.rs", b"low producer")),
        }],
        0,
        1,
        None,
    )?;
    let with_low = published(submit(&fixture.engine, low_request)?)?;
    let low_record = first_record(&low_records)?;

    let original_paths: Vec<String> = (1_u8..=129)
        .map(|id| format!("target/{id:03}.rs"))
        .collect();
    let baseline_changes: Vec<Change<'_>> = original_paths
        .iter()
        .zip(1_u8..=129)
        .map(|(path, id)| Change {
            owner: owner(5, id),
            expected: None,
            next: Some((path.as_str(), b"target baseline")),
        })
        .collect();
    let (_, target_request, records) = fixture.capture(
        operation(5, 6, 1, 1),
        with_low,
        &baseline_changes,
        0,
        1,
        None,
    )?;
    let with_target = published(submit(&fixture.engine, target_request)?)?;
    let original_refs: Vec<ObjectRef> = records
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or("target source record absent")?;
    check_eq!(original_refs.len(), 129, "target fixture lost an owner");
    let (_, high_request, high_records) = fixture.capture(
        operation(6, 8, 1, 1),
        with_target,
        &[Change {
            owner: owner(6, 1),
            expected: None,
            next: Some(("outside-high.rs", b"high producer")),
        }],
        0,
        1,
        None,
    )?;
    let baseline = published(submit(&fixture.engine, high_request)?)?;
    let high_record = first_record(&high_records)?;
    check_eq!(
        fixture.path_count(baseline)?,
        131,
        "baseline path count differs"
    );
    for id in [1_u8, 64, 65, 66, 129] {
        let index = usize::from(id.checked_sub(1).ok_or("invalid baseline owner id")?);
        let path = original_paths
            .get(index)
            .ok_or("baseline path index absent")?;
        let record = *original_refs
            .get(index)
            .ok_or("baseline record index absent")?;
        let entry = fixture
            .path_exact(baseline, path)?
            .ok_or("baseline path missing at page boundary")?;
        check_eq!(entry.owner, owner(5, id), "baseline path owner differs");
        check_eq!(entry.source, record, "baseline path source differs");
    }
    for (path, expected_owner, expected_ref) in [
        ("outside-low.rs", owner(4, 1), low_record),
        ("outside-high.rs", owner(6, 1), high_record),
    ] {
        let entry = fixture
            .path_exact(baseline, path)?
            .ok_or("neighbor path absent")?;
        check_eq!(entry.owner, expected_owner, "neighbor path owner differs");
        check_eq!(entry.source, expected_ref, "neighbor path ref differs");
    }
    let old_frontiers = frontier_rows(fixture.catalog.as_ref())?;
    let old_authority = authority_rows(fixture.catalog.as_ref())?;
    check_eq!(
        sql_epoch(&old_authority, 5),
        Some(1),
        "baseline target epoch differs"
    );

    // Omit the first row after a 64-entry prefix page. The other 128 rows
    // cannot make an incomplete producer baseline look complete.
    let incomplete_changes: Vec<Change<'_>> = original_paths
        .iter()
        .zip(1_u8..=129)
        .enumerate()
        .filter(|(_, (_, id))| *id != 65)
        .map(|(index, (path, id))| Change {
            owner: owner(5, id),
            expected: original_refs.get(index).copied(),
            next: Some((path.as_str(), b"target baseline")),
        })
        .collect();
    let (_, incomplete, _) = fixture.capture(
        operation(5, 7, 2, 1),
        baseline,
        &incomplete_changes,
        0,
        1,
        Some(1),
    )?;
    check_eq!(
        submit(&fixture.engine, incomplete)?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
        "missing page-boundary owner was accepted"
    );
    unchanged_authority_and_head(&fixture, baseline, &old_frontiers, &old_authority)?;
    check_eq!(
        fixture.path_count(baseline)?,
        131,
        "refused reset changed paths"
    );

    let reset_changes: Vec<Change<'_>> = original_paths
        .iter()
        .zip(1_u8..=129)
        .enumerate()
        .map(|(index, (path, id))| Change {
            owner: owner(5, id),
            expected: original_refs.get(index).copied(),
            next: match id {
                1 => Some(("target/replaced-001.rs", b"target replacement")),
                65 => None,
                _ => Some((path.as_str(), b"target baseline")),
            },
        })
        .collect();
    let (_, reset_request, reset_records) = fixture.capture(
        operation(5, 6, 2, 9),
        baseline,
        &reset_changes,
        0,
        1,
        Some(1),
    )?;
    let replacement = first_record(&reset_records)?;
    let reset = published(submit(&fixture.engine, reset_request)?)?;
    check_eq!(fixture.path_count(reset)?, 130, "reset path count differs");
    check_eq!(
        fixture.path_count(baseline)?,
        131,
        "historical paths changed"
    );
    check_eq!(
        sql_epoch(&authority_rows(fixture.catalog.as_ref())?, 5),
        Some(2),
        "target epoch did not advance"
    );
    check_eq!(
        fixture.owner_record(reset, owner(5, 1))?,
        Some(replacement),
        "replacement owner missing"
    );
    check_eq!(
        fixture.owner_record(reset, owner(5, 65))?,
        None,
        "deleted owner survived"
    );
    check_eq!(
        fixture.path_exact(reset, "target/001.rs")?.is_some(),
        false,
        "old replacement path survived"
    );
    check_eq!(
        fixture.path_exact(reset, "target/065.rs")?.is_some(),
        false,
        "deleted path survived"
    );
    let replaced_entry = fixture
        .path_exact(reset, "target/replaced-001.rs")?
        .ok_or("replacement path missing")?;
    check_eq!(
        replaced_entry.owner,
        owner(5, 1),
        "replacement path owner differs"
    );
    check_eq!(
        replaced_entry.source,
        replacement,
        "replacement path source differs"
    );
    for id in [64_u8, 66, 129] {
        let index = usize::from(id.checked_sub(1).ok_or("invalid unchanged owner id")?);
        let path = original_paths
            .get(index)
            .ok_or("unchanged path index absent")?;
        let record = *original_refs
            .get(index)
            .ok_or("unchanged record index absent")?;
        check_eq!(
            fixture.owner_record(reset, owner(5, id))?,
            Some(record),
            "unchanged owner ref differs"
        );
        let entry = fixture
            .path_exact(reset, path)?
            .ok_or("unchanged path missing")?;
        check_eq!(entry.owner, owner(5, id), "unchanged path owner differs");
        check_eq!(entry.source, record, "unchanged path source differs");
    }
    check_eq!(
        fixture.path_exact(baseline, "target/065.rs")?.is_some(),
        true,
        "historical deletion disappeared"
    );
    for (path, expected_owner, expected_ref) in [
        ("outside-low.rs", owner(4, 1), low_record),
        ("outside-high.rs", owner(6, 1), high_record),
    ] {
        let entry = fixture
            .path_exact(reset, path)?
            .ok_or("neighbor path lost")?;
        check_eq!(entry.owner, expected_owner, "neighbor owner changed");
        check_eq!(entry.source, expected_ref, "neighbor ref changed");
    }
    check!(
        frontier_rows(fixture.catalog.as_ref())?.iter().any(|row| {
            row.1.as_slice() == [5; 16] && row.2.as_slice() == [6; 16] && (row.3, row.4) == (2, 1)
        }),
        "reset event ordinal did not restart independently of operation sequence"
    );
    Ok(())
}

#[test]
fn malformed_initial_advance_never_seeds_source_authority_or_event_claim() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = Fixture::open(root.path())?;
    let configured = fixture.configure()?;
    for variant in 0_u8..4 {
        let stream = 6_u8.checked_add(variant).ok_or("test stream overflow")?;
        let (_, mut request, _) = fixture.capture(
            operation(5, stream, 1, 1),
            configured,
            &[Change {
                owner: owner(5, 1),
                expected: None,
                next: Some(("never-published.rs", b"malformed initial event")),
            }],
            0,
            1,
            None,
        )?;
        let UpdateKind::SourceAdvance(transition) = &mut request.kind else {
            return Err("test request lost SourceAdvance kind".into());
        };
        match variant {
            0 => transition.events.clear(),
            1 => {
                let event = transition.events.first_mut().ok_or("bound event missing")?;
                event.stream = StreamId([15; 16]);
            }
            2 => {
                let mut earlier = *transition.events.first().ok_or("bound event missing")?;
                earlier.producer = ProducerId([4; 16]);
                transition.events.push(earlier);
            }
            3 => {
                let event = transition.events.first_mut().ok_or("bound event missing")?;
                event.sequence = 3;
            }
            _ => return Err("invalid malformed event variant".into()),
        }
        check!(
            matches!(
                submit(&fixture.engine, request),
                Err(IndexError::InvalidInput(_) | IndexError::Conflict)
            ),
            "malformed source event passed public Submit"
        );
        check_eq!(
            latest(&fixture)?,
            configured,
            "malformed event advanced Latest"
        );
        check_eq!(
            frontier_rows(fixture.catalog.as_ref())?.len(),
            0,
            "malformed event changed frontier"
        );
        check_eq!(
            authority_rows(fixture.catalog.as_ref())?.len(),
            0,
            "malformed event seeded producer epoch"
        );
        check_eq!(
            event_claim_count(fixture.catalog.as_ref())?,
            0,
            "malformed event wrote a claim"
        );
    }
    Ok(())
}
