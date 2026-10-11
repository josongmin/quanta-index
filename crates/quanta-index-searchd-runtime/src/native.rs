//! Explicit opt-in native incremental service. This does not alter the
//! existing daemon routes or write the legacy index formats.

use std::{
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{ErrorKind, Read as _, Write as _},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    index::{
        IndexInstanceId, MemberBinding, OperationKey, OperationStatus, SubmitOutcome, ViewSelection,
    },
    native_protocol::{
        NativeFilePathEntry, NativeIndexErrorCode, NativeIndexRequest, NativeIndexRequestEnvelope,
        NativeIndexResponse, NativeIndexResponseEnvelope, NativeIndexWireError, NativeLiteralHit,
        NativeLiteralPage, NativePathPage,
    },
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        IndexError, TicketRecord, WorkBudget, WorkLimits, owner_map_key,
        ports::{ImmutableObjectPort, PersistentMapPort},
    },
};
use quanta_index_ipc::{
    DispatchContextV1, IpcDispatcher, IpcPlane, IpcServerCounters, ServerAdmissionPolicy,
    ShutdownHandle, SocketAccessPolicy, UdsServer,
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, Kernel, KernelServices};
use quanta_index_lexical::native::{LiteralQuery, LiteralReadContext, NativeLiteralIndex};
use quanta_index_repomap::native::{FileMapReadContext, FilePathQuery, NativeFileMap, query_paths};
use quanta_index_searchd::app::{
    runtime::StateRootLease, state_format::STATE_ROOT_LEASE_FILE_NAME,
};
use quanta_index_storage::FileObjectStore;

use crate::native_worker::NativeOperationWorker;
pub use crate::native_worker::{
    NativeLaneSnapshot, NativeWorkerObservation, NativeWorkerSnapshot, NativeWorkerStatus,
};

const MAX_STAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_STAGE_PATH_BYTES: usize = quanta_index_core::index::MAX_SOURCE_PATH_BYTES;
const MAX_QUERY_PAGE: u32 = 128;
const MAX_LITERAL_BYTES: usize = 4096;
/// Distinct from the legacy daemon's current-format root.
pub const NATIVE_MARKER_NAME: &str = ".native-index-v2";
pub const NATIVE_MARKER_CONTENT: &[u8] = b"quanta-index-native-state-v2\n";
const PRIOR_NATIVE_MARKER_NAME: &str = ".native-index-v1";

fn resolved_socket_entry(socket: &Path) -> Result<PathBuf> {
    let name = socket
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("native socket has no file name"))?;
    let mut parent = socket
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut missing: Vec<OsString> = Vec::new();
    loop {
        match fs::canonicalize(parent) {
            Ok(mut resolved) => {
                for component in missing.into_iter().rev() {
                    if component.as_os_str() != OsStr::new(".") {
                        resolved.push(component);
                        match fs::canonicalize(&resolved) {
                            Ok(existing) => resolved = existing,
                            Err(error) if error.kind() == ErrorKind::NotFound => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
                resolved.push(name);
                return Ok(resolved);
            }
            Err(error) if error.kind() == ErrorKind::NotFound && parent != Path::new(".") => {
                let component = parent
                    .components()
                    .next_back()
                    .ok_or_else(|| anyhow::anyhow!("native socket parent is invalid"))?;
                match component {
                    Component::Normal(_) | Component::CurDir => {
                        missing.push(component.as_os_str().to_os_string());
                    }
                    Component::ParentDir => {
                        return Err(anyhow::anyhow!(
                            "native socket has unresolved parent traversal"
                        ));
                    }
                    Component::RootDir | Component::Prefix(_) => return Err(error.into()),
                }
                parent = parent
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn check_native_layout(root: &Path, socket_entry: &Path, initialized: bool) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == STATE_ROOT_LEASE_FILE_NAME || name == NATIVE_MARKER_NAME {
            continue;
        }
        if initialized
            && (name == "catalog" || name == "native-objects")
            && entry.file_type()?.is_dir()
        {
            continue;
        }
        if initialized && entry.path() == socket_entry && entry.file_type()?.is_socket() {
            continue;
        }
        return Err(anyhow::anyhow!(
            "native index root contains foreign entry: {}",
            entry.path().display()
        ));
    }
    Ok(())
}

fn open_native_format(root: &Path, socket_entry: &Path) -> Result<()> {
    match fs::symlink_metadata(root.join(PRIOR_NATIVE_MARKER_NAME)) {
        Ok(_) => {
            return Err(anyhow::anyhow!(
                "native index v1 state requires an explicit rebuild or new incarnation"
            ));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let marker = root.join(NATIVE_MARKER_NAME);
    match fs::symlink_metadata(&marker) {
        Ok(metadata) => {
            if !metadata.file_type().is_file()
                || metadata.permissions().mode() & 0o7777 != 0o600
                || metadata.uid() != fs::metadata(root)?.uid()
                || metadata.nlink() != 1
            {
                return Err(anyhow::anyhow!(
                    "native index marker is not a private regular file"
                ));
            }
            let mut content = Vec::new();
            let _marker_bytes_read = fs::File::open(&marker)?
                .take(64)
                .read_to_end(&mut content)?;
            if content != NATIVE_MARKER_CONTENT {
                return Err(anyhow::anyhow!("native index marker version mismatch"));
            }
            check_native_layout(root, socket_entry, true)?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            check_native_layout(root, socket_entry, false)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&marker)?;
            file.write_all(NATIVE_MARKER_CONTENT)?;
            file.sync_all()?;
            fs::File::open(root)?.sync_all()?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

struct NativeDispatcher {
    kernel: Arc<Kernel<SqliteCatalog>>,
    objects: Arc<FileObjectStore>,
    limits: WorkLimits,
    worker: NativeOperationWorker,
}

impl IpcDispatcher<NativeIndexRequest, NativeIndexResponse> for NativeDispatcher {
    fn dispatch(
        &self,
        context: &DispatchContextV1,
        request: NativeIndexRequest,
        budget: &RequestBudgetV1,
    ) -> NativeIndexResponse {
        // Payload fields never grant operator authority. The credential came
        // from the kernel at accept, and in-process calls have no principal.
        if !matches!(context.principal, Some(peer) if peer.uid == context.owner_uid || peer.uid == 0)
            || context.plane != IpcPlane::Control
        {
            return NativeIndexResponse::Error(NativeIndexWireError {
                code: NativeIndexErrorCode::Denied,
                message: "native index control requires a kernel-authenticated operator".into(),
            });
        }
        let result = match request {
            NativeIndexRequest::Submit(request) => {
                let key = request.operation;
                let work = WorkBudget::with_request(self.limits, budget.clone());
                self.submitted_or_reconcile(
                    key,
                    self.worker.submit(Arc::clone(&self.kernel), request, work),
                )
            }
            NativeIndexRequest::Resume(key) => {
                let work = WorkBudget::with_request(self.limits, budget.clone());
                self.submitted_or_reconcile(
                    key,
                    self.worker.resume(Arc::clone(&self.kernel), key, work),
                )
            }
            other @ (NativeIndexRequest::CreateWorkspace { .. }
            | NativeIndexRequest::BeginInput(_)
            | NativeIndexRequest::StageSource { .. }
            | NativeIndexRequest::StageSources { .. }
            | NativeIndexRequest::StageProfile { .. }
            | NativeIndexRequest::StageProjectionOutput { .. }
            | NativeIndexRequest::StageProjectionOwner { .. }
            | NativeIndexRequest::StageProjectionCertificate { .. }
            | NativeIndexRequest::StageMutationPage { .. }
            | NativeIndexRequest::SealMutations { .. }
            | NativeIndexRequest::DiscardInput(_)
            | NativeIndexRequest::SearchLiteral { .. }
            | NativeIndexRequest::QueryPaths { .. }
            | NativeIndexRequest::ReadView(_)
            | NativeIndexRequest::Status(_)
            | NativeIndexRequest::ReadyWork { .. }
            | NativeIndexRequest::ClaimWork { .. }
            | NativeIndexRequest::RenewWork { .. }
            | NativeIndexRequest::WorkStatus(_)) => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                self.execute(other, &mut work)
            }
        };
        match result {
            Ok(response) => response,
            Err(error) => NativeIndexResponse::Error(wire_error(&error)),
        }
    }
}

impl NativeDispatcher {
    fn query_member(
        &self,
        ticket: &TicketRecord,
        instance: IndexInstanceId,
        budget: &mut WorkBudget,
    ) -> Result<MemberBinding, IndexError> {
        let mut key = b"member\0".to_vec();
        key.extend_from_slice(&instance.0);
        let reference = self
            .objects
            .lookup(ticket.view.members, &key, budget)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let stored = self.objects.get(reference, budget)?;
        if stored.registration.reference != reference {
            return Err(IndexError::CorruptObject(
                "query member reference mismatch".into(),
            ));
        }
        let member: MemberBinding = serde_json::from_slice(&stored.payload)
            .map_err(|error| IndexError::CorruptObject(format!("query member: {error}")))?;
        let mut children = vec![member.artifact];
        children.extend(member.input.root.0);
        if member.instance != instance || stored.registration.children != children {
            return Err(IndexError::CorruptObject(
                "query member identity or child edges mismatch".into(),
            ));
        }
        Ok(member)
    }

    fn search_literal(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        literal: &[u8],
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<NativeLiteralPage, IndexError> {
        if literal.is_empty()
            || literal.len() > MAX_LITERAL_BYTES
            || !(1..=MAX_QUERY_PAGE).contains(&limit)
            || after.is_some_and(|cursor| cursor.len() != 32)
        {
            return Err(IndexError::InvalidInput(
                "native literal query exceeds bounded capability".into(),
            ));
        }
        let ticket = self.kernel.pin_view(selection)?;
        let view = ticket.view().identity;
        let result = (|| {
            let member = self.query_member(ticket.record(), instance, budget)?;
            NativeLiteralIndex.search(
                &member,
                LiteralQuery {
                    literal,
                    after,
                    limit,
                },
                LiteralReadContext {
                    ticket: ticket.record(),
                    input: member.input,
                    objects: self.objects.as_ref(),
                    maps: self.objects.as_ref(),
                    budget,
                },
            )
        })();
        ticket.release()?;
        let page = result?;
        Ok(NativeLiteralPage {
            view,
            hits: page
                .hits
                .into_iter()
                .map(|hit| NativeLiteralHit {
                    owner: hit.owner,
                    record: hit.record,
                })
                .collect(),
            after: page.after,
            has_more: page.has_more,
        })
    }

    fn query_paths(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        prefix: &str,
        after: Option<&str>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<NativePathPage, IndexError> {
        if prefix.len() > MAX_STAGE_PATH_BYTES
            || after.is_some_and(|cursor| cursor.len() > MAX_STAGE_PATH_BYTES)
            || !(1..=MAX_QUERY_PAGE).contains(&limit)
        {
            return Err(IndexError::InvalidInput(
                "native file path query exceeds bounded capability".into(),
            ));
        }
        let ticket = self.kernel.pin_view(selection)?;
        let view = ticket.view().identity;
        let result = (|| {
            let member = self.query_member(ticket.record(), instance, budget)?;
            query_paths(
                &FilePathQuery {
                    member: &member,
                    prefix,
                    after,
                    limit,
                },
                &mut FileMapReadContext {
                    ticket: ticket.record(),
                    objects: self.objects.as_ref(),
                    maps: self.objects.as_ref(),
                    budget,
                },
            )
        })();
        ticket.release()?;
        let page = result?;
        Ok(NativePathPage {
            view,
            entries: page
                .entries
                .into_iter()
                .map(|entry| NativeFilePathEntry {
                    owner: entry.owner,
                    path: entry.path,
                    source: entry.source,
                    bytes: entry.bytes,
                    byte_len: entry.byte_len,
                })
                .collect(),
            has_more: page.has_more,
        })
    }

    fn execute(
        &self,
        request: NativeIndexRequest,
        budget: &mut WorkBudget,
    ) -> Result<NativeIndexResponse, IndexError> {
        budget.checkpoint()?;
        match request {
            NativeIndexRequest::CreateWorkspace { workspace, epoch } => self
                .kernel
                .create_workspace(workspace, epoch, budget)
                .map(NativeIndexResponse::Created),
            NativeIndexRequest::BeginInput(key) => self
                .kernel
                .begin_input(key)
                .map(NativeIndexResponse::InputOpened),
            NativeIndexRequest::StageSource {
                ingress,
                path,
                bytes,
            } => {
                if path.is_empty()
                    || path.len() > MAX_STAGE_PATH_BYTES
                    || bytes.len() > MAX_STAGE_BYTES
                    || path.starts_with('/')
                    || path.contains('\0')
                    || path
                        .split('/')
                        .any(|part| part.is_empty() || part == "." || part == "..")
                {
                    return Err(IndexError::InvalidInput(
                        "source stage exceeds native bounds".into(),
                    ));
                }
                let reference = self.kernel.stage_source(ingress, &path, &bytes, budget)?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageSources { ingress, inputs } => self
                .kernel
                .stage_sources(ingress, &inputs, budget)
                .map(NativeIndexResponse::StagedSources),
            NativeIndexRequest::StageProfile { ingress, profile } => {
                let reference = self.kernel.stage_profile(ingress, &profile, budget)?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageProjectionOutput { ingress, bytes } => {
                if bytes.len() > MAX_STAGE_BYTES {
                    return Err(IndexError::ResourceLimited);
                }
                let reference = self
                    .kernel
                    .stage_projection_output(ingress, &bytes, budget)?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageProjectionOwner {
                ingress,
                contribution,
            } => {
                let ticket = self.kernel.pin_view(ViewSelection::Latest {
                    workspace: ingress.operation.workspace,
                    epoch: ingress.operation.epoch,
                })?;
                if contribution.owner.producer != contribution.producer {
                    return Err(IndexError::InvalidInput(
                        "projection owner must belong to its producer".into(),
                    ));
                }
                match (contribution.source_owner, contribution.source) {
                    (Some(source_owner), Some(source)) => {
                        let expected = self.objects.lookup(
                            ticket.view().source.owners,
                            &owner_map_key(source_owner),
                            budget,
                        )?;
                        if expected != Some(source) {
                            return Err(IndexError::Conflict);
                        }
                    }
                    (None, None) => {}
                    _ => {
                        return Err(IndexError::InvalidInput(
                            "projection source owner and record must be paired".into(),
                        ));
                    }
                }
                if let Some(output) = contribution.output {
                    let stored = self.objects.get(output, budget)?;
                    if stored.registration.reference != output
                        || !stored.registration.children.is_empty()
                    {
                        return Err(IndexError::InvalidInput(
                            "projection output must be a staged leaf".into(),
                        ));
                    }
                }
                let reference =
                    self.kernel
                        .stage_projection_owner(ingress, contribution, budget)?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageProjectionCertificate {
                ingress,
                certificate,
            } => {
                let ticket = self.kernel.pin_view(ViewSelection::Latest {
                    workspace: ingress.operation.workspace,
                    epoch: ingress.operation.epoch,
                })?;
                if certificate.target_source != ticket.view().source {
                    return Err(IndexError::Conflict);
                }
                let reference =
                    self.kernel
                        .stage_projection_certificate(ingress, &certificate, budget)?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageMutationPage {
                ingress,
                base,
                mutations,
            } => {
                if mutations.is_empty() || mutations.len() > 128 {
                    return Err(IndexError::ResourceLimited);
                }
                self.kernel
                    .stage_mutation_page(ingress, base, &mutations, budget)
                    .map(NativeIndexResponse::MutationPageStaged)
            }
            NativeIndexRequest::SealMutations {
                ingress,
                root,
                count,
            } => self
                .kernel
                .seal_mutations(ingress, root, count, budget)
                .map(NativeIndexResponse::MutationsStaged),
            NativeIndexRequest::DiscardInput(ingress) => {
                self.kernel.discard_input(ingress, budget)?;
                Ok(NativeIndexResponse::InputDiscarded(ingress))
            }
            NativeIndexRequest::SearchLiteral {
                selection,
                instance,
                literal,
                after,
                limit,
            } => self
                .search_literal(
                    selection,
                    instance,
                    &literal,
                    after.as_deref(),
                    limit,
                    budget,
                )
                .map(NativeIndexResponse::LiteralPage),
            NativeIndexRequest::QueryPaths {
                selection,
                instance,
                prefix,
                after,
                limit,
            } => self
                .query_paths(
                    selection,
                    instance,
                    &prefix,
                    after.as_deref(),
                    limit,
                    budget,
                )
                .map(NativeIndexResponse::PathPage),
            NativeIndexRequest::Submit(_) | NativeIndexRequest::Resume(_) => Err(
                IndexError::BackendViolation("async native operation routed to sync path".into()),
            ),
            NativeIndexRequest::Status(key) => self.kernel.status(key).map(|payload| {
                NativeIndexResponse::Status(quanta_index_core::index::OperationReply {
                    operation: key,
                    payload,
                })
            }),
            NativeIndexRequest::ReadView(selection) => {
                let ticket = self.kernel.pin_view(selection)?;
                let view = ticket.view().clone();
                ticket.release()?;
                Ok(NativeIndexResponse::View(view))
            }
            NativeIndexRequest::ReadyWork {
                workspace,
                epoch,
                limit,
            } => {
                if limit == 0 || limit > 1024 {
                    return Err(IndexError::ResourceLimited);
                }
                self.kernel
                    .ready_work(workspace, epoch, limit, budget)
                    .map(NativeIndexResponse::ReadyWork)
            }
            NativeIndexRequest::ClaimWork {
                intent,
                lease_millis,
            } => {
                if lease_millis == 0 || lease_millis > 600_000 {
                    return Err(IndexError::InvalidInput(
                        "work lease outside 1..=600000 ms".into(),
                    ));
                }
                self.kernel
                    .claim_work(&intent, Duration::from_millis(lease_millis))
                    .map(NativeIndexResponse::Claimed)
            }
            NativeIndexRequest::RenewWork {
                claim,
                lease_millis,
            } => {
                if lease_millis == 0 || lease_millis > 600_000 {
                    return Err(IndexError::InvalidInput(
                        "work lease outside 1..=600000 ms".into(),
                    ));
                }
                self.kernel
                    .renew_work(&claim, Duration::from_millis(lease_millis))
                    .map(NativeIndexResponse::Renewed)
            }
            NativeIndexRequest::WorkStatus(intent) => self
                .kernel
                .work_status(&intent)
                .map(NativeIndexResponse::WorkStatus),
        }
    }

    fn submitted_or_reconcile(
        &self,
        key: OperationKey,
        result: Result<SubmitOutcome, IndexError>,
    ) -> Result<NativeIndexResponse, IndexError> {
        let payload = match result {
            Ok(outcome) => outcome,
            Err(error)
                if !matches!(
                    &error,
                    IndexError::Storage(_)
                        | IndexError::Cancelled
                        | IndexError::ResourceLimited
                        | IndexError::FenceLost
                ) =>
            {
                // A different immutable payload under this key and other
                // deterministic violations must never become an ACK-loss hint.
                return Err(error);
            }
            Err(error) => match self.kernel.status(key) {
                // No admission is known, so preserve the concrete refusal.
                Ok(OperationStatus::Unknown) => return Err(error),
                // An admitted operation may have committed despite this error.
                Ok(
                    OperationStatus::Pending(_)
                    | OperationStatus::Published(_)
                    | OperationStatus::Refused(_),
                )
                | Err(_) => SubmitOutcome::Unknown(key),
            },
        };
        Ok(NativeIndexResponse::Submitted(
            quanta_index_core::index::OperationReply {
                operation: key,
                payload,
            },
        ))
    }
}

fn wire_error(error: &IndexError) -> NativeIndexWireError {
    let code = match error {
        IndexError::InvalidInput(_) => NativeIndexErrorCode::InvalidInput,
        IndexError::Conflict => NativeIndexErrorCode::Conflict,
        IndexError::DigestConflict => NativeIndexErrorCode::DigestConflict,
        IndexError::FenceLost | IndexError::WorkFenceLost => NativeIndexErrorCode::FenceLost,
        IndexError::ResourceLimited => NativeIndexErrorCode::ResourceLimited,
        IndexError::Cancelled => NativeIndexErrorCode::Cancelled,
        IndexError::SnapshotUnavailable => NativeIndexErrorCode::SnapshotUnavailable,
        IndexError::CorruptObject(_) => NativeIndexErrorCode::CorruptObject,
        IndexError::BackendViolation(_) => NativeIndexErrorCode::BackendViolation,
        IndexError::Storage(_) => NativeIndexErrorCode::Storage,
        IndexError::RebuildRequired => NativeIndexErrorCode::RebuildRequired,
        IndexError::MaintenanceRequired => NativeIndexErrorCode::MaintenanceRequired,
    };
    NativeIndexWireError {
        code,
        message: error.to_string(),
    }
}

/// Holds the sole state-root lease for the service's full listener lifetime.
pub struct NativeIndexService {
    server: UdsServer,
    dispatcher: Arc<NativeDispatcher>,
    _lease: StateRootLease,
}

/// Deployment-owned schema/format identifiers for the two installed native
/// adapters. No test identifiers or implicit profile activation are baked in.
#[derive(Clone, Copy)]
pub struct NativeAdapterDescriptors {
    pub literal: AdapterDescriptor,
    pub file_map: AdapterDescriptor,
}

impl NativeIndexService {
    pub fn bind(
        state_root: &Path,
        socket_path: &Path,
        limits: WorkLimits,
        descriptors: NativeAdapterDescriptors,
    ) -> Result<Self> {
        let mut registry = AdapterRegistry::new();
        // These are typed descriptors, not implicit default profile members.
        registry.register(descriptors.literal, NativeLiteralIndex, |delta| {
            Ok(delta.clone())
        })?;
        registry.register(descriptors.file_map, NativeFileMap, |delta| {
            Ok(delta.clone())
        })?;
        Self::bind_with_registry(state_root, socket_path, limits, registry)
    }

    /// Binds a native service with a caller-composed typed adapter registry.
    /// The same registry serves every operation and work claim on this root.
    pub fn bind_with_registry(
        state_root: &Path,
        socket_path: &Path,
        limits: WorkLimits,
        registry: AdapterRegistry,
    ) -> Result<Self> {
        let lease = StateRootLease::acquire(state_root)?;
        let root = lease.state_root_identity_v1();
        let socket_entry = resolved_socket_entry(socket_path)?;
        if socket_entry
            .strip_prefix(root)
            .is_ok_and(|relative| relative.components().count() > 1)
        {
            return Err(anyhow::anyhow!(
                "native socket must not use a nested directory inside the state root"
            ));
        }
        // Runtime construction fails before the marker, catalog or object
        // store may be created. The manager owns its Tokio runtime lifetime.
        let worker = NativeOperationWorker::start()?;
        open_native_format(root, &socket_entry)?;
        let objects = Arc::new(FileObjectStore::open(root.join("native-objects"))?);
        let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_secs(2))?);
        let services = KernelServices::new(objects.clone(), objects.clone(), catalog);
        let dispatcher = Arc::new(NativeDispatcher {
            kernel: Arc::new(Kernel::new(services, registry)),
            objects,
            limits,
            worker,
        });
        // Each execution class can hold one active and one queued dispatch.
        // Four transport slots let Core reach its independent worker even
        // while Enrichment occupies both of its slots. Publication remains
        // serialized by the catalog's CAS and attempt/work fences.
        let admission = ServerAdmissionPolicy::new(
            64,
            4,
            4,
            Duration::from_secs(10),
            Duration::from_secs(120),
            Duration::from_secs(30),
        )?;
        let server = UdsServer::bind_observed(
            socket_path,
            admission,
            SocketAccessPolicy::Private,
            Arc::new(IpcServerCounters::for_plane("native-index")),
        )?;
        Ok(Self {
            server,
            dispatcher,
            _lease: lease,
        })
    }

    #[must_use]
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.server.shutdown_handle()
    }

    /// The handle remains readable after `run` consumes this service. Its
    /// monotonic timestamps all use the same worker-local origin.
    #[must_use]
    pub fn worker_observation(&self) -> NativeWorkerObservation {
        self.dispatcher.worker.observation()
    }

    pub fn run(self, accept_idle: Duration) -> Result<()> {
        self.server.run::<NativeIndexRequestEnvelope, NativeIndexRequest,
            NativeIndexResponseEnvelope, NativeIndexResponse, NativeDispatcher>(
                &self.dispatcher, IpcPlane::Control, accept_idle,
            )?;
        Ok(())
    }
}
