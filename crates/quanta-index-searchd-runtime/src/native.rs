//! Explicit opt-in native incremental service. This does not alter the
//! existing daemon routes or write the legacy index formats.

use std::{
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{ErrorKind, Read as _, Write as _},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::{Context as _, Result};
use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
    index::{IndexInstanceId, OperationKey, OperationStatus, SubmitOutcome, ViewSelection},
    native_protocol::{
        MAX_NATIVE_TEXT_TOP_K, NativeFilePathEntry, NativeIndexErrorCode, NativeIndexRequest,
        NativeIndexRequestEnvelope, NativeIndexResponse, NativeIndexWireError, NativeLiteralHit,
        NativeLiteralPage, NativePathPage, NativeProjectionRead,
    },
};
use quanta_index_core::RequestBudgetV1;
use quanta_index_core::index::{
    BootstrapRef, IndexError, MAX_RECLAIM_DRAIN_PAGE, ObjectRef, QueryCoverage, QueryOutcome,
    WorkBudget, WorkLimits,
    effort::{ReservationGuard, ResourceNeed},
    owner_map_key,
    ports::{
        CatalogGenesisPort, CatalogReadPort, ObjectReadIoPort, OperationInspectAccess,
        PersistentMapPort,
    },
    validate_read_ticket_lease,
};
use quanta_index_ipc::{
    IpcPlane, IpcServerCounters, ServerAdmissionPolicy, ShutdownHandle, SocketAccessPolicy,
    UdsServer,
};
use quanta_index_kernel::{
    AdapterDescriptor, AdapterRegistry, Kernel, KernelServices, QueryAdapterRegistry,
};
use quanta_index_lexical::{
    native::{
        LiteralPage, LiteralReadRequest, NativeLiteralIndex, NativeLiteralReader, PlannedTextRead,
        TextReadResult,
    },
    planner::LexicalPlanner,
};
use quanta_index_repomap::native::{
    FilePathPage, NativeFileMap, NativePathReader, PathReadRequest,
};
use quanta_index_searchd::app::{
    runtime::StateRootLease, state_format::STATE_ROOT_LEASE_FILE_NAME,
};
use quanta_index_semantic::native::{SearchResult, SemanticReadRequest};
use quanta_index_storage::{
    FileObjectStore, FileScratchSpace, ManagedObjectIo, ObjectIoObservation,
};

pub use crate::native_auto_signals::{
    NativeAutoSignalConfig, NativeAutoSignals, NativeInteractionActivity,
    NativeInteractionActivityPort,
};
pub use crate::native_interaction::NativeInteractionRecorder;
pub use crate::native_provider_signals::{NativeProviderPoolSignals, NativeProviderSignalPort};
pub use crate::native_pump::{
    FixedPumpPolicy, NativePumpConfig, NativePumpDecision, NativePumpObservation, NativePumpPolicy,
    NativePumpScope,
};
pub use crate::native_read_sessions::NativeReadSessionConfig;
pub use crate::native_reclaim_driver::{NativeReclaimConfig, NativeReclaimStep};
pub use crate::native_resources::{
    NativeClassSnapshot, NativeResourceObservation, NativeResourcePlan, NativeResourceSnapshot,
};
pub use crate::native_retention::{NativeRetentionConfig, NativeRetentionScope};
pub use crate::native_semantic_result::ScopedNativeSemanticResult;
pub use crate::native_text_result::ScopedNativeTextResult;
pub use crate::native_worker::{
    NativeLaneSnapshot, NativeWorkerObservation, NativeWorkerSnapshot, NativeWorkerStatus,
};
use crate::{
    native_object_read_io::NativeObjectReadIo,
    native_object_reclaim_io::NativeObjectReclaimIo,
    native_object_write_io::NativeObjectWriteIo,
    native_provider_custody::NativeProviderExecutionCustody,
    native_pump::{NativeOwnedPump, NativePumpServices},
    native_reclaim_driver::reclaim_one_step,
    native_resources::{NativeClassScheduler, NativeResourceClass, NativeResourceLedger},
    native_scratch_io::NativeScratchIo,
    native_work_diagnostic::{self, DiscardFailureStage},
    native_worker::{DIRECT_ENRICHMENT_DISPATCH_LIMIT, NativeOperationWorker},
    publication_file_capacity::{InodeObservation, PublicationFileCapacity},
    publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity},
};

const PROTECTED_CORE_DISPATCH_SLOTS: usize = 2;

fn native_server_admission() -> Result<ServerAdmissionPolicy> {
    let dispatch_slots = DIRECT_ENRICHMENT_DISPATCH_LIMIT
        .checked_add(PROTECTED_CORE_DISPATCH_SLOTS)
        .ok_or(IndexError::ResourceLimited)?;
    Ok(ServerAdmissionPolicy::new(
        64,
        dispatch_slots,
        dispatch_slots,
        Duration::from_secs(10),
        Duration::from_secs(120),
        Duration::from_secs(30),
    )?)
}

const MAX_STAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_STAGE_PATH_BYTES: usize = quanta_index_core::index::MAX_SOURCE_PATH_BYTES;
// One bounded GC step may decode a 1 MiB catalog registration and stream a
// 16 MiB object with a second physical recheck. The extra 64 KiB is the
// store's streaming hash buffer. These are logical service reservations,
// not a measured RSS or disk ceiling.
const GC_STEP_MIN_MEMORY_BYTES: u64 = 4_259_840;
const GC_STEP_MIN_IO_BYTES: u64 = 67_108_864;
// Service boot owns marker, scratch and Catalog recovery before foreground
// requests exist. One recovery page can inspect 256 open intents and retired
// locks, 32 sessions and up to 256 pins. Leave bounded room for scratch
// retirement; a larger dirty backlog fails closed under this original boot
// request instead of borrowing the caller's foreground per-request limits.
const NATIVE_BOOT_WORK_LIMITS: WorkLimits = WorkLimits {
    reads: 16_384,
    writes: 2_048,
    bytes: 134_217_728,
    mutations: 2_048,
};
/// Distinct from the legacy daemon's current-format root.
pub const NATIVE_MARKER_NAME: &str = ".native-index-v18";
pub const NATIVE_MARKER_CONTENT: &[u8] = b"quanta-index-native-state-v18\n";
const PRIOR_NATIVE_MARKER_NAMES: [&str; 17] = [
    ".native-index-v1",
    ".native-index-v2",
    ".native-index-v3",
    ".native-index-v4",
    ".native-index-v5",
    ".native-index-v6",
    ".native-index-v7",
    ".native-index-v8",
    ".native-index-v9",
    ".native-index-v10",
    ".native-index-v11",
    ".native-index-v12",
    ".native-index-v13",
    ".native-index-v14",
    ".native-index-v15",
    ".native-index-v16",
    ".native-index-v17",
];

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
            && (name == "catalog" || name == "native-objects" || name == "native-scratch-v1")
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

fn open_native_format(
    root: &Path,
    socket_entry: &Path,
    physical: &Arc<PublicationFileCapacity>,
    startup: &mut WorkBudget,
) -> Result<()> {
    for prior in PRIOR_NATIVE_MARKER_NAMES {
        match fs::symlink_metadata(root.join(prior)) {
            Ok(_) => {
                return Err(anyhow::anyhow!(
                    "prior native index state requires an explicit rebuild or new incarnation"
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
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
            let owner = NativeObjectReadIo::new(Arc::clone(physical)).open(&marker, startup)?;
            let _marker_bytes_read = owner.file()?.take(64).read_to_end(&mut content)?;
            if content != NATIVE_MARKER_CONTENT {
                return Err(anyhow::anyhow!("native index marker version mismatch"));
            }
            check_native_layout(root, socket_entry, true)?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            check_native_layout(root, socket_entry, false)?;
            let mut ticket = physical.begin_native_create(&marker)?;
            ticket.reserve_end(u64::try_from(NATIVE_MARKER_CONTENT.len())?)?;
            ticket.creation_started()?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&marker)?;
            let written = (|| -> Result<()> {
                ticket.observe_native_fd(InodeObservation::from_file(&file)?)?;
                file.write_all(NATIVE_MARKER_CONTENT)?;
                ticket.observe_native_fd(InodeObservation::from_file(&file)?)?;
                file.sync_all()?;
                let _retained =
                    ticket.settle_after_native_sync(InodeObservation::from_file(&file)?, None)?;
                Ok(())
            })();
            let observed = InodeObservation::from_file(&file);
            if let Err(error) = rusqlite::ffi::quanta_capacity_vfs::close_owned_file(file) {
                physical.mark_close_unwitnessed();
                return Err(anyhow::anyhow!("close native marker: {error}"));
            }
            if written.is_ok() {
                ticket.confirmed_native_fd_retired()?;
            } else if let Ok(observed) = observed {
                ticket.confirmed_unsynced_native_fd_retired(observed)?;
            }
            written?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub use crate::native_stub_certificate_projection_v1::StubGrammarPolicy;

pub(super) struct NativeDispatcher {
    pub(super) read_sessions: Arc<OnceLock<Arc<crate::native_read_sessions::NativeReadSessions>>>,
    pub(super) kernel: Arc<Kernel<SqliteCatalog>>,
    pub(super) catalog: Arc<SqliteCatalog>,
    pub(super) literal_queries: QueryAdapterRegistry<LiteralReadRequest, LiteralPage>,
    pub(super) path_queries: QueryAdapterRegistry<PathReadRequest, FilePathPage>,
    pub(super) text_queries: QueryAdapterRegistry<PlannedTextRead, TextReadResult>,
    pub(super) semantic_queries: QueryAdapterRegistry<SemanticReadRequest, SearchResult>,
    pub(super) objects: Arc<FileObjectStore>,
    pub(super) limits: WorkLimits,
    pub(super) worker: Arc<NativeOperationWorker>,
    pub(super) resources: Arc<NativeResourceLedger>,
    pub(super) control_scheduler: Arc<NativeClassScheduler>,
    pub(super) pump_control_scheduler: Arc<NativeClassScheduler>,
    pub(super) interactive_scheduler: Arc<NativeClassScheduler>,
    pub(super) enrichment_scheduler: Arc<NativeClassScheduler>,
    pub(super) control_need: ResourceNeed,
    pub(super) interactive_need: ResourceNeed,
    pub(super) enrichment_need: ResourceNeed,
    pub(super) effort: OnceLock<crate::native_effort_ports::NativeEffortConfiguration>,
    pub(super) graph: OnceLock<
        QueryAdapterRegistry<
            quanta_index_contract::native_protocol::NativeGraphQuery,
            quanta_index_repomap::native::GraphPage,
        >,
    >,
    pub(super) repomap: OnceLock<
        QueryAdapterRegistry<
            quanta_index_contract::native_protocol::NativeRepoMapQuery,
            quanta_index_repomap::native::NativeRepoMapRankPage,
        >,
    >,
    pub(super) overlay: OnceLock<Arc<crate::native_overlay::NativeOverlay>>,
    pub(super) stub_ast:
        OnceLock<Arc<dyn quanta_index_lexical::native::stub::StubAstCertificatePort>>,
    pub(super) interaction: OnceLock<Arc<crate::native_interaction::NativeInteractionRecorder>>,
    pub(super) root_lease: Arc<StateRootLease>,
    physical: Arc<PublicationFileCapacity>,
    pub(super) io: Arc<NativeScratchIo>,
    wal_checkpoint_bytes: u64,
}

impl NativeDispatcher {
    pub(super) fn search_semantic(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: SemanticQueryRequest,
        budget: &mut WorkBudget,
    ) -> Result<ScopedNativeSemanticResult, IndexError> {
        if query.lexical_scope.is_some() {
            return self.search_semantic_scoped(selection, instance, query, budget);
        }
        let top_k = query.top_k;
        let prepared = {
            let control = ReservationGuard::try_acquire(
                Arc::clone(&self.control_scheduler),
                self.control_need,
            )
            .ok_or_else(|| self.control_scheduler.denial())?;
            budget.bind_memory_scope(control.memory_scope());
            self.kernel.prepare_query(
                &self.semantic_queries,
                selection,
                instance,
                query.into(),
                budget,
            )?
        };
        let need = prepared.required_resources();
        let Some(reservation) =
            ReservationGuard::try_acquire(Arc::clone(&self.interactive_scheduler), need)
        else {
            let denial = self.interactive_scheduler.denial();
            prepared.abort()?;
            return Err(denial);
        };
        let result = if need.provider_units > 0 {
            let custody = NativeProviderExecutionCustody::new_with_owner(
                reservation,
                self.root_lease.clone(),
            )?;
            let outcome =
                prepared.execute_with_provider_custody(custody.reservation(), &custody.port())?;
            native_semantic_result(outcome, instance, top_k, |bytes, lifetime| {
                budget.reserve_memory(bytes, lifetime)
            })?
        } else {
            let outcome = prepared.execute(&reservation)?;
            native_semantic_result(outcome, instance, top_k, |bytes, lifetime| {
                budget.reserve_memory(bytes, lifetime)
            })?
        };
        budget.checkpoint()?;
        Ok(result)
    }

    pub(super) fn search_text(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: &TextQueryRequest,
        budget: &mut WorkBudget,
    ) -> Result<ScopedNativeTextResult, IndexError> {
        let (prepared, result_limit) = {
            let control = ReservationGuard::try_acquire(
                Arc::clone(&self.control_scheduler),
                self.control_need,
            )
            .ok_or_else(|| self.control_scheduler.denial())?;
            budget.bind_memory_scope(control.memory_scope());
            let planned = plan_native_text(query, |bytes, lifetime| {
                budget.reserve_memory(bytes, lifetime)
            })?;
            let result_limit = planned.result_limit()?;
            let prepared = self.kernel.prepare_query(
                &self.text_queries,
                selection,
                instance,
                planned,
                budget,
            )?;
            (prepared, result_limit)
        };
        let need = prepared.required_resources();
        let Some(reservation) =
            ReservationGuard::try_acquire(Arc::clone(&self.interactive_scheduler), need)
        else {
            let denial = self.interactive_scheduler.denial();
            prepared.abort()?;
            return Err(denial);
        };
        let outcome = prepared.execute(&reservation)?;
        let result = native_text_result(outcome, instance, result_limit, |bytes, lifetime| {
            budget.reserve_memory(bytes, lifetime)
        })?;
        budget.checkpoint()?;
        Ok(result)
    }

    pub(super) fn search_literal(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        literal: Vec<u8>,
        after: Option<Vec<u8>>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ScopedNativeLiteralPage, IndexError> {
        let request = LiteralReadRequest {
            literal,
            after,
            limit,
        };
        request.validate()?;
        let requested_after = request
            .after
            .as_deref()
            .map(<[u8; 32]>::try_from)
            .transpose()
            .map_err(|_error| IndexError::InvalidInput("invalid literal cursor".into()))?;
        let prepared = {
            let control = ReservationGuard::try_acquire(
                Arc::clone(&self.control_scheduler),
                self.control_need,
            )
            .ok_or_else(|| self.control_scheduler.denial())?;
            budget.bind_memory_scope(control.memory_scope());
            self.kernel.prepare_query(
                &self.literal_queries,
                selection,
                instance,
                request,
                budget,
            )?
        };
        let need = prepared.required_resources();
        let Some(reservation) =
            ReservationGuard::try_acquire(Arc::clone(&self.interactive_scheduler), need)
        else {
            let denial = self.interactive_scheduler.denial();
            prepared.abort()?;
            return Err(denial);
        };
        let outcome = prepared.execute(&reservation)?;
        native_literal_result(
            outcome,
            limit,
            requested_after.as_ref().map(<[u8; 32]>::as_slice),
            |bytes, lifetime| budget.reserve_memory(bytes, lifetime),
        )
    }

    pub(super) fn query_paths(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        prefix: String,
        after: Option<String>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ScopedNativePathPage, IndexError> {
        let request = PathReadRequest {
            prefix,
            after,
            limit,
        };
        request.validate()?;
        // Validation bounds these UTF-8 bytes. Keep borrowed validation metadata
        // on the stack rather than allocating copies before query admission.
        let mut prefix_bytes = [0; quanta_index_core::index::MAX_SOURCE_PATH_BYTES];
        let mut after_bytes = [0; quanta_index_core::index::MAX_SOURCE_PATH_BYTES];
        let prefix_slice = prefix_bytes
            .get_mut(..request.prefix.len())
            .ok_or(IndexError::ResourceLimited)?;
        prefix_slice.copy_from_slice(request.prefix.as_bytes());
        let requested_prefix = std::str::from_utf8(prefix_slice).map_err(|_error| {
            IndexError::BackendViolation("invalid validated path prefix".into())
        })?;
        let requested_after = if let Some(after) = &request.after {
            let slice = after_bytes
                .get_mut(..after.len())
                .ok_or(IndexError::ResourceLimited)?;
            slice.copy_from_slice(after.as_bytes());
            Some(std::str::from_utf8(slice).map_err(|_error| {
                IndexError::BackendViolation("invalid validated path cursor".into())
            })?)
        } else {
            None
        };
        let prepared = {
            let control = ReservationGuard::try_acquire(
                Arc::clone(&self.control_scheduler),
                self.control_need,
            )
            .ok_or_else(|| self.control_scheduler.denial())?;
            budget.bind_memory_scope(control.memory_scope());
            self.kernel
                .prepare_query(&self.path_queries, selection, instance, request, budget)?
        };
        let need = prepared.required_resources();
        let Some(reservation) =
            ReservationGuard::try_acquire(Arc::clone(&self.interactive_scheduler), need)
        else {
            let denial = self.interactive_scheduler.denial();
            prepared.abort()?;
            return Err(denial);
        };
        let outcome = prepared.execute(&reservation)?;
        native_path_result(
            outcome,
            requested_prefix,
            requested_after,
            limit,
            |bytes, lifetime| budget.reserve_memory(bytes, lifetime),
        )
    }

    fn prepare_discard(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<
        Box<dyn quanta_index_core::publication_admission::CatalogPublicationFundingScope>,
        IndexError,
    > {
        // A discard can grow WAL before reclaiming logical custody. Keep its
        // existing maintenance parent and original request through both steps.
        {
            let _checkpoint_scope = self.io.enter_maintenance_scope().inspect_err(|error| {
                native_work_diagnostic::record_discard_if_enabled(
                    DiscardFailureStage::PhysicalMaintenanceAdmission,
                    budget,
                    error,
                );
            })?;
            // A failed foreground transaction can leave an unsynced local WAL
            // ticket below the background size threshold. Further writes then
            // reuse that ticket, so an untouched maintenance parent alone does
            // not fund its growth. At this explicit cleanup boundary, settle
            // any nonempty WAL through SQLite's existing sync/checkpoint path
            // before deleting custody. Keep the original finite IO/work caps.
            match self
                .catalog
                .checkpoint_wal_for_reclaim(1, self.interactive_need.io_bytes, budget)
            {
                Ok(_) => {}
                Err(IndexError::ResourceLimited) => {
                    // A busy snapshot or finite checkpoint allowance can defer
                    // this optional step. An expired request cannot continue.
                    budget.checkpoint()?;
                }
                Err(error) => return Err(error),
            }
        }
        // Only actual freed headroom can refill the same held parent. This is
        // admission for the original discard, not a retry or a larger reserve.
        self.io.enter_maintenance_scope().inspect_err(|error| {
            native_work_diagnostic::record_discard_if_enabled(
                DiscardFailureStage::PhysicalMaintenanceAdmission,
                budget,
                error,
            );
        })
    }

    pub(super) fn execute(
        &self,
        request: NativeIndexRequest,
        budget: &mut WorkBudget,
    ) -> Result<crate::native_reply::NativeReply, IndexError> {
        let is_discard = matches!(&request, NativeIndexRequest::DiscardInput(_));
        budget.checkpoint().inspect_err(|error| {
            if is_discard {
                native_work_diagnostic::record_discard_if_enabled(
                    DiscardFailureStage::ExecuteCheckpoint,
                    budget,
                    error,
                );
            }
        })?;
        // Staged references already own their retained backing grant.
        if let NativeIndexRequest::StageSources { ingress, inputs } = request {
            return self
                .kernel
                .stage_sources(ingress, &inputs, budget)
                .map(crate::native_reply::NativeReply::StagedSources);
        }
        let reply_slot = crate::native_reply::NativeReply::reserve_canonical(|bytes| {
            budget.reserve_memory(bytes, quanta_index_core::index::MemoryLifetime::Retained)
        })
        .inspect_err(|error| {
            if is_discard {
                native_work_diagnostic::record_discard_if_enabled(
                    DiscardFailureStage::ReplyReservation,
                    budget,
                    error,
                );
            }
        })?;
        let result = match request {
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
                text,
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
                let reference = self
                    .kernel
                    .stage_source_text(ingress, &path, &bytes, text, budget)?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageProfile { ingress, profile } => {
                let reference = self.kernel.stage_profile(ingress, &profile, budget)?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageProjectionOutput {
                ingress,
                bytes,
                children,
            } => {
                if bytes.len() > MAX_STAGE_BYTES {
                    return Err(IndexError::ResourceLimited);
                }
                let reference = self.kernel.stage_projection_output_with_children(
                    ingress,
                    &bytes,
                    children.as_slice(),
                    budget,
                )?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageProjectionOwner {
                ingress,
                contribution,
            } => {
                let ticket = self.kernel.pin_view(
                    ViewSelection::Latest {
                        workspace: ingress.operation.workspace,
                        epoch: ingress.operation.epoch,
                    },
                    budget,
                )?;
                ticket.validate()?;
                if contribution.owner.producer != contribution.producer {
                    return Err(IndexError::InvalidInput(
                        "projection owner must belong to its producer".into(),
                    ));
                }
                match (contribution.source_owner, contribution.source) {
                    (Some(source_owner), Some(source)) => {
                        let expected = self.objects.lookup_object(
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
                let reference =
                    self.kernel
                        .stage_projection_owner(ingress, contribution, budget)?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageProjectionCertificate {
                ingress,
                certificate,
            } => {
                let ticket = self.kernel.pin_view(
                    ViewSelection::Latest {
                        workspace: ingress.operation.workspace,
                        epoch: ingress.operation.epoch,
                    },
                    budget,
                )?;
                ticket.validate()?;
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
                let _physical = self.prepare_discard(budget)?;
                self.kernel
                    .discard_input(ingress, budget)
                    .inspect_err(|error| {
                        native_work_diagnostic::record_discard_if_enabled(
                            DiscardFailureStage::CatalogDiscard,
                            budget,
                            error,
                        );
                        native_work_diagnostic::record_discard_physical_if_enabled(
                            self.catalog.as_ref(),
                            self.physical.as_ref(),
                        );
                    })?;
                Ok(NativeIndexResponse::InputDiscarded(ingress))
            }
            NativeIndexRequest::StageSources { .. }
            | NativeIndexRequest::Overlay(_)
            | NativeIndexRequest::Stub(_)
            | NativeIndexRequest::BindOverlaySave(_)
            | NativeIndexRequest::CompleteOverlaySave(_)
            | NativeIndexRequest::QueryGraph { .. }
            | NativeIndexRequest::QueryRepoMap { .. }
            | NativeIndexRequest::LookupSourceOwners { .. }
            | NativeIndexRequest::LookupProjectionOwners { .. }
            | NativeIndexRequest::ListProducerSourceOwners { .. }
            | NativeIndexRequest::DiffProjectionSourceOwners { .. }
            | NativeIndexRequest::SearchLiteral { .. }
            | NativeIndexRequest::QueryPaths { .. }
            | NativeIndexRequest::SearchOverlayText(_)
            | NativeIndexRequest::ReadSession(_)
            | NativeIndexRequest::SearchText { .. }
            | NativeIndexRequest::SearchSemantic { .. }
            | NativeIndexRequest::QueryEffortStream { .. } => Err(IndexError::BackendViolation(
                "native query bypassed selected reader admission".into(),
            )),
            NativeIndexRequest::Submit(_) | NativeIndexRequest::Resume(_) => Err(
                IndexError::BackendViolation("async native operation routed to sync path".into()),
            ),
            NativeIndexRequest::Status(key) => self
                .kernel
                .status(key, OperationInspectAccess::Request(budget))
                .map(|payload| {
                    NativeIndexResponse::Status(quanta_index_core::index::OperationReply {
                        operation: key,
                        payload,
                    })
                }),
            NativeIndexRequest::ReadProjection {
                selection,
                projection,
            } => {
                let (view, binding) = self
                    .kernel
                    .read_projection_binding(selection, projection, budget)?;
                Ok(NativeIndexResponse::Projection(NativeProjectionRead {
                    view,
                    binding,
                }))
            }
            NativeIndexRequest::ReadView(selection) => {
                let ticket = self.kernel.pin_view(selection, budget)?;
                let result = ticket.validate().map(|()| ticket.view().clone());
                let view = ticket.finish(result, budget)?;
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
                    .claim_work(&intent, Duration::from_millis(lease_millis), budget)
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
                    .renew_work(&claim, Duration::from_millis(lease_millis), budget)
                    .map(NativeIndexResponse::Renewed)
            }
            NativeIndexRequest::WorkStatus(intent) => self
                .kernel
                .work_status(&intent, budget)
                .map(NativeIndexResponse::WorkStatus),
            NativeIndexRequest::InteractionBegin { .. }
            | NativeIndexRequest::InteractionActivity { .. }
            | NativeIndexRequest::InteractionRenew { .. }
            | NativeIndexRequest::InteractionEnd { .. } => Err(IndexError::InvalidInput(
                "interaction publication requires a kernel-authenticated IPC principal".into(),
            )),
        };
        result.map(|payload| reply_slot.finish(payload))
    }

    pub(super) fn submitted_or_reconcile(
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
            Err(error) => match self
                .kernel
                .status(key, OperationInspectAccess::PostFailureReconcile)
            {
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

pub(super) fn plan_native_text(
    query: &TextQueryRequest,
    reserve: impl Fn(
        u64,
        quanta_index_core::index::MemoryLifetime,
    ) -> Result<Option<quanta_index_core::index::MemoryLease>, IndexError>,
) -> Result<PlannedTextRead, IndexError> {
    if query.syntax != TextQuerySyntax::Native
        || !query.constraints.language_any_of.is_empty()
        || query.generation.is_some()
        || query.generation_selector.is_some()
        || query.cursor.is_some()
        || !(1..=MAX_NATIVE_TEXT_TOP_K).contains(&query.top_k)
    {
        return Err(IndexError::InvalidInput(
            "native text route supports bounded Native queries with exact-path constraints".into(),
        ));
    }
    let (lowered, lowering_memory) =
        crate::native_query_lowering::lower_native_text_input(query, &reserve)?.into_parts();
    let backing = quanta_index_lexical::native::validate_native_text_input(&lowered)?;
    let plan_memory = reserve(backing, quanta_index_core::index::MemoryLifetime::Retained)?;
    let plan = LexicalPlanner::plan(&lowered)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    Ok(
        PlannedTextRead::new(plan, query.top_k, &query.constraints, reserve)?.with_plan_memory([
            plan_memory,
            lowering_memory[0].clone(),
            lowering_memory[1].clone(),
        ]),
    )
}

fn literal_owner_cursor(owner: &quanta_index_core::index::OwnerKey) -> [u8; 32] {
    let mut cursor = [0; 32];
    cursor[..16].copy_from_slice(&owner.producer.0);
    cursor[16..].copy_from_slice(&owner.stable_id.0);
    cursor
}

pub(crate) type ScopedNativeLiteralPage =
    crate::native_result::ScopedNativeResult<NativeLiteralPage>;
pub(crate) type ScopedNativePathPage = crate::native_result::ScopedNativeResult<NativePathPage>;

pub(super) fn native_literal_result(
    outcome: QueryOutcome<LiteralPage>,
    limit: u32,
    requested_after: Option<&[u8]>,
    reserve: impl Fn(
        u64,
        quanta_index_core::index::MemoryLifetime,
    ) -> Result<Option<quanta_index_core::index::MemoryLease>, IndexError>,
) -> Result<ScopedNativeLiteralPage, IndexError> {
    if !matches!(outcome.coverage(), QueryCoverage::Core) {
        return Err(IndexError::BackendViolation(
            "native literal reader did not return core coverage".into(),
        ));
    }
    let view = outcome.view();
    let exhaustive = outcome.exhaustive();
    let page = outcome.into_result();
    if exhaustive == page.has_more
        || page.hits.len() > usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?
        || page.has_more != page.after.is_some()
        || page.after.as_ref().is_some_and(|cursor| {
            cursor.len() != 32 || requested_after.is_some_and(|prior| cursor.as_slice() <= prior)
        })
        || page.has_more && page.hits.is_empty()
    {
        return Err(IndexError::BackendViolation(
            "native literal reader returned an invalid page".into(),
        ));
    }
    let mut prior = requested_after
        .map(<[u8; 32]>::try_from)
        .transpose()
        .map_err(|_error| {
            IndexError::BackendViolation("invalid requested literal cursor".into())
        })?;
    for hit in &page.hits {
        let cursor = literal_owner_cursor(&hit.owner);
        if !valid_native_source_path(&hit.record.path)
            || prior
                .as_ref()
                .is_some_and(|previous| cursor.as_slice() <= previous.as_slice())
        {
            return Err(IndexError::BackendViolation(
                "native literal reader returned unordered hits".into(),
            ));
        }
        prior = Some(cursor);
    }
    if page.has_more && page.after.as_deref() != prior.as_ref().map(<[u8; 32]>::as_slice) {
        return Err(IndexError::BackendViolation(
            "native literal reader cursor differs from last hit".into(),
        ));
    }
    let count = page.hits.len();
    let (hits, payload_memory, capacity) = result_backing::<NativeLiteralPage, NativeLiteralHit>(
        count,
        count.checked_add(1).ok_or(IndexError::ResourceLimited)?,
        &reserve,
    )?;
    let mut converted = crate::native_result::ResultValue {
        result: NativeLiteralPage {
            view,
            hits,
            after: None,
            has_more: page.has_more,
        },
        payload_memory,
        _capacity_memory: capacity,
    };
    let (input_hits, after, _more, old_hit_memory, after_memory) = page.into_parts();
    // Move the cursor before its retained charge; partial results always own
    // bytes before leases, including conversion errors and unwind.
    converted.result.after = after;
    converted.payload_memory.push(after_memory);
    for hit in input_hits {
        let (owner, record, memory) = hit.into_parts();
        converted
            .result
            .hits
            .push(NativeLiteralHit { owner, record });
        converted.payload_memory.push(memory);
    }
    drop(old_hit_memory);
    Ok(crate::native_result::ScopedNativeResult::new(converted))
}

pub(super) fn native_path_result(
    outcome: QueryOutcome<FilePathPage>,
    prefix: &str,
    requested_after: Option<&str>,
    limit: u32,
    reserve: impl Fn(
        u64,
        quanta_index_core::index::MemoryLifetime,
    ) -> Result<Option<quanta_index_core::index::MemoryLease>, IndexError>,
) -> Result<ScopedNativePathPage, IndexError> {
    if !matches!(outcome.coverage(), QueryCoverage::Core) {
        return Err(IndexError::BackendViolation(
            "native path reader did not return core coverage".into(),
        ));
    }
    let view = outcome.view();
    let exhaustive = outcome.exhaustive();
    let page = outcome.into_result();
    if exhaustive == page.has_more
        || page.entries.len()
            > usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?
        || page.has_more && page.entries.is_empty()
    {
        return Err(IndexError::BackendViolation(
            "native path reader returned an invalid page".into(),
        ));
    }
    let mut prior = requested_after;
    for entry in &page.entries {
        if !valid_native_source_path(&entry.path)
            || !entry.path.starts_with(prefix)
            || prior.is_some_and(|previous| entry.path.as_str() <= previous)
        {
            return Err(IndexError::BackendViolation(
                "native path reader returned unordered entries".into(),
            ));
        }
        prior = Some(&entry.path);
    }
    let count = page.entries.len();
    let (entries, payload_memory, capacity) =
        result_backing::<NativePathPage, NativeFilePathEntry>(count, count, &reserve)?;
    let mut converted = crate::native_result::ResultValue {
        result: NativePathPage {
            view,
            entries,
            has_more: page.has_more,
        },
        payload_memory,
        _capacity_memory: capacity,
    };
    let (input_entries, _more, old_entry_memory) = page.into_parts();
    for entry in input_entries {
        let (owner, path, source, bytes, byte_len, memory) = entry.into_parts();
        converted.result.entries.push(NativeFilePathEntry {
            owner,
            path,
            source,
            bytes,
            byte_len,
        });
        converted.payload_memory.push(memory);
    }
    drop(old_entry_memory);
    Ok(crate::native_result::ScopedNativeResult::new(converted))
}

type PageResultBacking<H> = (
    Vec<H>,
    Vec<Option<quanta_index_core::index::MemoryLease>>,
    Option<quanta_index_core::index::MemoryLease>,
);

fn result_backing<R, H>(
    count: usize,
    leases: usize,
    reserve: &impl Fn(
        u64,
        quanta_index_core::index::MemoryLifetime,
    ) -> Result<Option<quanta_index_core::index::MemoryLease>, IndexError>,
) -> Result<PageResultBacking<H>, IndexError> {
    let value_bytes = crate::native_result::shared_value_bytes::<R>()?;
    let bytes = count
        .checked_mul(std::mem::size_of::<H>())
        .and_then(|bytes| {
            leases
                .checked_mul(std::mem::size_of::<
                    Option<quanta_index_core::index::MemoryLease>,
                >())
                .and_then(|lease_bytes| bytes.checked_add(lease_bytes))
        })
        .and_then(|bytes| bytes.checked_add(value_bytes))
        .ok_or(IndexError::ResourceLimited)?;
    let memory = reserve(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        quanta_index_core::index::MemoryLifetime::Retained,
    )?;
    let mut hits = Vec::new();
    hits.try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut payload_memory = Vec::new();
    payload_memory
        .try_reserve_exact(leases)
        .map_err(|_error| IndexError::ResourceLimited)?;
    Ok((hits, payload_memory, memory))
}

pub(super) fn native_text_result(
    outcome: QueryOutcome<TextReadResult>,
    instance: IndexInstanceId,
    requested_top_k: u32,
    reserve: impl Fn(
        u64,
        quanta_index_core::index::MemoryLifetime,
    ) -> Result<Option<quanta_index_core::index::MemoryLease>, IndexError>,
) -> Result<ScopedNativeTextResult, IndexError> {
    crate::native_text_result::convert(outcome, instance, requested_top_k, reserve)
}

pub(super) fn valid_native_source_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_STAGE_PATH_BYTES
        && !path.starts_with('/')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

pub(super) fn native_semantic_result(
    outcome: QueryOutcome<SearchResult>,
    instance: IndexInstanceId,
    top_k: u32,
    reserve: impl Fn(
        u64,
        quanta_index_core::index::MemoryLifetime,
    ) -> Result<Option<quanta_index_core::index::MemoryLease>, IndexError>,
) -> Result<ScopedNativeSemanticResult, IndexError> {
    crate::native_semantic_result::convert(outcome, instance, top_k, reserve)
}

pub(super) fn wire_error(error: &IndexError) -> NativeIndexWireError {
    let code = match error {
        IndexError::InvalidInput(_) => NativeIndexErrorCode::InvalidInput,
        IndexError::FocusSubjectNotFound => NativeIndexErrorCode::FocusSubjectNotFound,
        IndexError::Conflict => NativeIndexErrorCode::Conflict,
        IndexError::DigestConflict => NativeIndexErrorCode::DigestConflict,
        IndexError::FenceLost | IndexError::WorkFenceLost => NativeIndexErrorCode::FenceLost,
        IndexError::ResourceLimited | IndexError::WorkLimitExceeded => {
            NativeIndexErrorCode::ResourceLimited
        }
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
///
/// One leased root and its existing Common/VFS owner, retained across offline
/// verification and service binding. This grants no receipt identity by itself.
pub(super) struct NativeBootAuthority {
    pub(super) lease: Arc<StateRootLease>,
    pub(super) physical: Arc<PublicationFileCapacity>,
    pub(super) io: Arc<NativeScratchIo>,
    pub(super) request: RequestBudgetV1,
}

pub(super) fn prepare_native_boot_authority(
    lease: Arc<StateRootLease>,
    socket_path: &Path,
    resources: NativeServiceResources,
    request: &RequestBudgetV1,
) -> Result<NativeBootAuthority> {
    let root = lease.state_root_identity_v1();
    lease.require_state_root_v1(root)?;
    let socket_entry = resolved_socket_entry(socket_path)?;
    if socket_entry
        .strip_prefix(root)
        .is_ok_and(|relative| relative.components().count() > 1)
    {
        return Err(anyhow::anyhow!(
            "native socket must not use a nested directory inside the state root"
        ));
    }
    let authority = seed_publication_capacity(
        &lease,
        root,
        resources.physical.inventory,
        std::slice::from_ref(&socket_entry),
        request,
    )?;
    let physical = PublicationFileCapacity::new_with_maintenance(
        lease.as_ref().clone(),
        authority,
        usize::try_from(resources.physical.max_reader_handles)?,
        usize::try_from(resources.physical.max_tracked_entries)?,
        usize::try_from(resources.physical.max_process_roots)?,
        resources.physical.maintenance_reserved_bytes,
        u64::from(resources.physical.maintenance_reserved_files),
    )?;
    let io = NativeScratchIo::register_with_maintenance(
        Arc::clone(&physical),
        std::ffi::CString::new("quanta-publication-capacity")?,
        resources.physical.global_max_sqlite_handles,
    )?;
    Ok(NativeBootAuthority {
        lease,
        physical,
        io,
        request: request.clone(),
    })
}

pub(super) fn verify_existing_native_format(
    authority: &NativeBootAuthority,
    socket: &Path,
    budget: &mut WorkBudget,
) -> Result<()> {
    let root = authority.lease.state_root_identity_v1();
    for name in PRIOR_NATIVE_MARKER_NAMES {
        match std::fs::symlink_metadata(root.join(name)) {
            Ok(_) => return Err(IndexError::RebuildRequired.into()),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let marker = root.join(NATIVE_MARKER_NAME);
    let owner = NativeObjectReadIo::new(Arc::clone(&authority.physical)).open(&marker, budget)?;
    let metadata = owner.file()?.metadata()?;
    if !metadata.is_file()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
        || metadata.len() != u64::try_from(NATIVE_MARKER_CONTENT.len())?
    {
        return Err(IndexError::RebuildRequired.into());
    }
    budget.charge(quanta_index_core::index::WorkUsage {
        reads: 1,
        bytes: metadata
            .len()
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?,
        ..Default::default()
    })?;
    let mut content = [0_u8; 64];
    let length = usize::try_from(metadata.len())?;
    let mut file = owner.file()?;
    file.read_exact(
        content
            .get_mut(..length)
            .ok_or(IndexError::ResourceLimited)?,
    )?;
    let mut extra = [0_u8; 1];
    let after = file.metadata()?;
    let linked = std::fs::symlink_metadata(&marker)?;
    let unchanged = |actual: &std::fs::Metadata| {
        actual.is_file()
            && actual.dev() == metadata.dev()
            && actual.ino() == metadata.ino()
            && actual.len() == metadata.len()
            && actual.mtime() == metadata.mtime()
            && actual.mtime_nsec() == metadata.mtime_nsec()
            && actual.mode() == metadata.mode()
            && actual.uid() == metadata.uid()
            && actual.nlink() == metadata.nlink()
    };
    if file.read(&mut extra)? != 0
        || !unchanged(&after)
        || !unchanged(&linked)
        || content.get(..length) != Some(NATIVE_MARKER_CONTENT)
    {
        return Err(IndexError::RebuildRequired.into());
    }
    drop(owner);
    authority.physical.sweep_retired()?;
    check_native_layout(root, &resolved_socket_entry(socket)?, true)?;
    authority.lease.require_state_root_v1(root)?;
    budget.checkpoint()?;
    Ok(())
}

pub struct NativeIndexService {
    server: UdsServer,
    pub(super) dispatcher: Arc<NativeDispatcher>,
    pump: Option<NativeOwnedPump>,
    pub(super) lease: Arc<StateRootLease>,
    provider_attempt_pool: Option<Arc<quanta_index_embed::ProviderAttemptPool>>,
    execution_plan: NativeResourcePlan,
    owned_work_enabled: bool,
    maintenance_enabled: bool,
    object_io_observation: Option<Arc<ObjectIoObservation>>,
}

/// In-process maintenance authority retained under the same state-root lease.
///
/// It remains usable while `NativeIndexService::run` owns the listener and
/// keeps the root locked if the listener has already stopped.
pub struct NativeIndexMaintenance {
    kernel: Arc<Kernel<SqliteCatalog>>,
    catalog: Arc<SqliteCatalog>,
    objects: Arc<FileObjectStore>,
    control_scheduler: Arc<NativeClassScheduler>,
    control_need: ResourceNeed,
    io: Arc<NativeScratchIo>,
    wal_checkpoint_bytes: u64,
    _lease: Arc<StateRootLease>,
}

impl NativeIndexMaintenance {
    pub fn recover_read_sessions(
        &self,
        after: Option<quanta_index_core::index::ReadSessionRef>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::ReadSessionRecoveryPage, IndexError> {
        budget.checkpoint()?;
        let reservation =
            ReservationGuard::try_acquire(Arc::clone(&self.control_scheduler), self.control_need)
                .ok_or_else(|| self.control_scheduler.denial())?;
        budget.bind_memory_scope(reservation.memory_scope());
        let _physical = self.io.enter_maintenance_scope()?;
        self.kernel.recover_read_sessions(after, limit, budget)
    }

    /// Advance at most one catalog work item and one bounded child-edge page.
    /// The caller owns the original budget and restarts a failed sweep at None.
    pub fn reclaim_step(
        &self,
        after: Option<ObjectRef>,
        child_limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<NativeReclaimStep, IndexError> {
        budget.checkpoint()?;
        if child_limit == 0 || child_limit > MAX_RECLAIM_DRAIN_PAGE {
            return Err(IndexError::InvalidInput(
                "reclaim child page limit is out of range".into(),
            ));
        }
        if self.control_need.memory_bytes < GC_STEP_MIN_MEMORY_BYTES
            || self.control_need.io_bytes < GC_STEP_MIN_IO_BYTES
        {
            return Err(IndexError::ResourceLimited);
        }
        let reservation =
            ReservationGuard::try_acquire(Arc::clone(&self.control_scheduler), self.control_need)
                .ok_or_else(|| self.control_scheduler.denial())?;
        budget.bind_memory_scope(reservation.memory_scope());
        budget.checkpoint()?;
        let _physical = self.io.enter_maintenance_scope()?;
        let _checkpointed = self.catalog.checkpoint_wal_for_reclaim(
            self.wal_checkpoint_bytes,
            self.control_need.io_bytes,
            budget,
        )?;
        reclaim_one_step(&self.catalog, &self.objects, after, child_limit, budget)
    }

    /// Explicitly abandon one fenced bootstrap. No stage TTL is inferred.
    pub fn abort_bootstrap(
        &self,
        stage: BootstrapRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let reservation =
            ReservationGuard::try_acquire(Arc::clone(&self.control_scheduler), self.control_need)
                .ok_or_else(|| self.control_scheduler.denial())?;
        budget.bind_memory_scope(reservation.memory_scope());
        budget.checkpoint()?;
        let _physical = self.io.enter_maintenance_scope()?;
        self.catalog.abort_bootstrap(stage, budget)
    }
}

pub(super) fn validate_automatic_reclaim(
    config: NativeReclaimConfig,
    plan: NativeResourcePlan,
    owned_pump: bool,
) -> Result<(), IndexError> {
    config.validate()?;
    if plan.control.memory_bytes < GC_STEP_MIN_MEMORY_BYTES
        || plan.control.io_bytes < GC_STEP_MIN_IO_BYTES
    {
        return Err(IndexError::InvalidInput(
            "native automatic reclaim Control envelope is insufficient".into(),
        ));
    }
    plan.validate_maintenance(owned_pump)
}

/// Deployment-owned limits for regular-file extents and controlled handles.
///
/// These are not allocated-block/free-space reserves, whole-process FD/RSS
/// limits or measured directory/allocator envelopes. No defaults are inferred.
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePhysicalProfile {
    pub inventory: PublicationBootInventoryLimits,
    pub max_reader_handles: u32,
    pub max_tracked_entries: u32,
    pub max_process_roots: u32,
    pub global_max_sqlite_handles: u32,
    pub maintenance_reserved_bytes: u64,
    pub maintenance_reserved_files: u32,
}

impl NativePhysicalProfile {
    pub fn validate(self) -> Result<()> {
        if self.inventory.max_bytes == 0
            || self.inventory.max_files == 0
            || self.inventory.max_pending == 0
            || self.inventory.max_entries == 0
            || self.inventory.max_pending_frames == 0
            || self.inventory.max_depth < 3
            || self.max_reader_handles == 0
            || self.max_tracked_entries == 0
            || self.max_process_roots == 0
            || self.global_max_sqlite_handles < self.max_reader_handles
            || self.maintenance_reserved_bytes == 0
            || self.maintenance_reserved_files == 0
            || self.maintenance_reserved_bytes > self.inventory.max_bytes
            || u64::from(self.maintenance_reserved_files) > self.inventory.max_files
        {
            return Err(anyhow::anyhow!(
                "native physical profile requires positive limits and maintenance reserve within its byte/file cap, inventory depth at least3 and a global SQLite handle ceiling covering the root ceiling"
            ));
        }
        Ok(())
    }
}

/// Explicit per-service physical, scratch, read-ticket, and execution limits.
#[derive(Clone, Copy)]
pub struct NativeServiceResources {
    pub physical: NativePhysicalProfile,
    pub scratch_reserved_bytes: u64,
    pub read_ticket_lease: Duration,
    /// Logical aggregate concurrency estimates. These are not measured RSS,
    /// WAL, disk or provider charges.
    pub execution: NativeResourcePlan,
}

/// Deployment-owned schema/format identifiers for the two installed native
/// adapters. No test identifiers or implicit profile activation are baked in.
#[derive(Clone, Copy)]
pub struct NativeAdapterDescriptors {
    pub literal: AdapterDescriptor,
    pub file_map: AdapterDescriptor,
}

/// Explicit composition of build adapters, typed readers and optional owned work.
/// Provider and recipe policy belong to the registered server-side capabilities.
pub struct NativeServiceCapabilities {
    pub builders: AdapterRegistry,
    pub literal: QueryAdapterRegistry<LiteralReadRequest, LiteralPage>,
    pub path: QueryAdapterRegistry<PathReadRequest, FilePathPage>,
    pub text: QueryAdapterRegistry<PlannedTextRead, TextReadResult>,
    pub semantic: QueryAdapterRegistry<SemanticReadRequest, SearchResult>,
    pub pump: Option<NativePumpConfig>,
    /// The same pool used by configured providers. Physical attempts retain
    /// their execution grant and root lease until exit, including during drain.
    pub provider_attempt_pool: Option<Arc<quanta_index_embed::ProviderAttemptPool>>,
}

impl NativeServiceCapabilities {
    /// Pure startup validation, shared by in-process and product composition.
    /// No state root, socket, worker or catalog is opened here.
    pub(super) fn validate_resources(
        &self,
        resources: NativeServiceResources,
        maintenance: Option<NativeReclaimConfig>,
        retention: Option<&NativeRetentionConfig>,
    ) -> Result<(ResourceNeed, ResourceNeed)> {
        resources.physical.validate()?;
        if let Some(config) = retention {
            config.validate()?;
            if maintenance.is_none() {
                return Err(anyhow::anyhow!(
                    "native retention requires automatic reclaim capacity"
                ));
            }
        }
        if let Some(config) = &self.pump {
            config.validate()?;
        }
        if resources.scratch_reserved_bytes == 0 {
            return Err(anyhow::anyhow!(
                "native scratch reservation must be positive"
            ));
        }
        let _validated_lease_ms = validate_read_ticket_lease(resources.read_ticket_lease)?;
        resources.execution.validate()?;
        if let Some(config) = maintenance {
            validate_automatic_reclaim(config, resources.execution, self.pump.is_some())?;
        }
        let (core_demand, enrichment_demand) = self.builders.resource_demands();
        if !core_demand.fits(resources.execution.core)
            || !enrichment_demand.fits(resources.execution.enrichment)
        {
            return Err(anyhow::anyhow!(
                "native builder resource demand exceeds its execution class"
            ));
        }
        if let Some(config) = &self.pump {
            if maintenance.is_some() {
                resources
                    .execution
                    .validate_parallel_maintenance(config.max_in_flight, enrichment_demand)?;
            } else {
                resources
                    .execution
                    .validate_parallel_owned_pump(config.max_in_flight, enrichment_demand)?;
            }
        }
        self.literal
            .validate_reservation(resources.execution.interactive)?;
        self.path
            .validate_reservation(resources.execution.interactive)?;
        self.text
            .validate_reservation(resources.execution.interactive)?;
        self.semantic
            .validate_reservation(resources.execution.interactive)?;
        Ok((core_demand, enrichment_demand))
    }
}

impl NativeIndexService {
    /// The optional install observer is shared with the object store. Snapshot
    /// only after the service has drained; constructor, GC and Catalog/WAL IO
    /// are outside its scope.
    pub(crate) fn object_io_observation(&self) -> Option<Arc<ObjectIoObservation>> {
        self.object_io_observation.clone()
    }

    pub(crate) fn with_diagnostic_context(
        mut self,
        context: &quanta_index_core::DiagnosticContextV1,
    ) -> Self {
        self.server = self.server.with_diagnostic_context(context.clone());
        self
    }

    pub fn configure_graph(
        &self,
        readers: QueryAdapterRegistry<
            quanta_index_contract::native_protocol::NativeGraphQuery,
            quanta_index_repomap::native::GraphPage,
        >,
    ) -> Result<(), IndexError> {
        readers.validate_reservation(self.dispatcher.interactive_need)?;
        self.dispatcher
            .graph
            .set(readers)
            .map_err(|_readers| IndexError::Conflict)
    }

    pub fn configure_repomap(
        &self,
        readers: QueryAdapterRegistry<
            quanta_index_contract::native_protocol::NativeRepoMapQuery,
            quanta_index_repomap::native::NativeRepoMapRankPage,
        >,
    ) -> Result<(), IndexError> {
        readers.validate_reservation(self.dispatcher.interactive_need)?;
        self.dispatcher
            .repomap
            .set(readers)
            .map_err(|_readers| IndexError::Conflict)
    }

    /// Configure bounded ephemeral text before serving. Authentication reuses
    /// the same finite publisher lease as activity, without renewing it.
    pub fn configure_overlay(
        &self,
        sessions: usize,
        documents: usize,
        document_bytes: usize,
        total_bytes: usize,
        text_instance: IndexInstanceId,
    ) -> Result<(), IndexError> {
        let recorder = self
            .dispatcher
            .interaction
            .get()
            .ok_or(IndexError::SnapshotUnavailable)?;
        let auth: Arc<dyn crate::native_overlay::OverlayPublisherAuthPort> = recorder.clone();
        let overlay = crate::native_overlay::NativeOverlay::new(
            crate::native_overlay::OverlayLimits {
                sessions,
                documents,
                document_bytes,
                total_bytes,
            },
            auth,
            text_instance,
        )?;
        self.dispatcher
            .overlay
            .set(Arc::new(overlay))
            .map_err(|_overlay| IndexError::Conflict)
    }

    /// Read published AST evidence only under explicitly approved grammar builds.
    pub fn configure_published_stub(
        &self,
        policies: Vec<StubGrammarPolicy>,
    ) -> Result<(), IndexError> {
        let ast =
            crate::native_stub_certificate_projection_v1::PublishedStubAstCertificatePort::new(
                Arc::clone(&self.dispatcher.kernel),
                Arc::clone(&self.dispatcher.objects),
                policies,
            )?;
        self.configure_stub(Arc::new(ast))
    }

    /// Install the trusted AST producer behind the saved-source action route.
    /// No language or parser is inferred when this authority is absent.
    pub fn configure_stub(
        &self,
        ast: Arc<dyn quanta_index_lexical::native::stub::StubAstCertificatePort>,
    ) -> Result<(), IndexError> {
        if self.dispatcher.overlay.get().is_none() {
            return Err(IndexError::SnapshotUnavailable);
        }
        self.dispatcher
            .stub_ast
            .set(ast)
            .map_err(|_ast| IndexError::Conflict)
    }

    /// Enable bounded product read leases on the existing maintenance pump.
    pub fn configure_read_sessions(
        &self,
        config: crate::native_read_sessions::NativeReadSessionConfig,
    ) -> Result<(), IndexError> {
        if !self.maintenance_enabled {
            return Err(IndexError::InvalidInput(
                "read sessions require configured maintenance".into(),
            ));
        }
        let bytes = crate::native_read_sessions::NativeReadSessions::required_memory(&config)?;
        let memory = self.dispatcher.resources.reserve_response_memory(bytes)?;
        let sessions = crate::native_read_sessions::NativeReadSessions::new(config, memory)?;
        self.dispatcher
            .read_sessions
            .set(Arc::new(sessions))
            .map_err(|_sessions| IndexError::Conflict)
    }

    /// Install the bounded foreground publisher source before serving requests.
    /// Publisher freshness is finite; an abrupt client crash becomes Unknown
    /// after expiry because individual SDK calls use separate UDS connections.
    pub fn configure_interaction_activity(
        &self,
        recorder: Arc<crate::native_interaction::NativeInteractionRecorder>,
    ) -> Result<(), IndexError> {
        self.dispatcher
            .interaction
            .set(recorder)
            .map_err(|_recorder| IndexError::Conflict)
    }
    /// Sample this service's actual worker and ledger. User/IDE activity is
    /// an explicit input; its absence never becomes a fabricated idle value.
    pub fn auto_signals(
        &self,
        interaction: Option<Arc<dyn NativeInteractionActivityPort>>,
        worker_lag_max_age: Duration,
        sample_ttl: Duration,
    ) -> Result<NativeAutoSignals, IndexError> {
        NativeAutoSignals::new(
            self.worker_observation(),
            self.resource_observation(),
            interaction,
            NativeAutoSignalConfig {
                resource_plan: self.execution_plan,
                background_control_enabled: self.owned_work_enabled,
                maintenance_control_enabled: self.maintenance_enabled,
                worker_lag_max_age,
                sample_ttl,
            },
        )
    }
    /// Install the policy, estimator and signal source once, before `run`.
    /// Streamed effort requests are refused while this remains unconfigured.
    pub fn configure_effort(
        &self,
        configuration: crate::native_effort_ports::NativeEffortConfiguration,
    ) -> Result<(), IndexError> {
        self.dispatcher
            .effort
            .set(configuration)
            .map_err(|_configuration| IndexError::Conflict)
    }

    /// Acquire a bounded recovery handle before moving the service into `run`.
    #[must_use]
    pub fn maintenance_handle(&self) -> NativeIndexMaintenance {
        NativeIndexMaintenance {
            kernel: Arc::clone(&self.dispatcher.kernel),
            catalog: Arc::clone(&self.dispatcher.catalog),
            objects: Arc::clone(&self.dispatcher.objects),
            control_scheduler: Arc::clone(&self.dispatcher.control_scheduler),
            control_need: self.dispatcher.control_need,
            io: Arc::clone(&self.dispatcher.io),
            wal_checkpoint_bytes: self.dispatcher.wal_checkpoint_bytes,
            _lease: Arc::clone(&self.lease),
        }
    }

    pub fn bind(
        state_root: &Path,
        socket_path: &Path,
        limits: WorkLimits,
        descriptors: NativeAdapterDescriptors,
        resources: NativeServiceResources,
    ) -> Result<Self> {
        let mut registry = AdapterRegistry::new();
        // These are typed descriptors, not implicit default profile members.
        registry.register(descriptors.literal, NativeLiteralIndex, |delta| {
            Ok(delta.clone())
        })?;
        registry.register(descriptors.file_map, NativeFileMap, |delta| {
            Ok(delta.clone())
        })?;
        let reader_need = ResourceNeed {
            provider_units: 0,
            ..resources.execution.interactive
        };
        let mut literal = QueryAdapterRegistry::new();
        literal.register(
            descriptors.literal,
            reader_need,
            NativeLiteralReader::new(reader_need)?,
        )?;
        let mut path = QueryAdapterRegistry::new();
        path.register(
            descriptors.file_map,
            reader_need,
            NativePathReader::new(reader_need)?,
        )?;
        Self::bind_with_capabilities(
            state_root,
            socket_path,
            limits,
            NativeServiceCapabilities {
                builders: registry,
                literal,
                path,
                text: QueryAdapterRegistry::new(),
                semantic: QueryAdapterRegistry::new(),
                pump: None,
                provider_attempt_pool: None,
            },
            resources,
        )
    }

    /// Binds a native service with a caller-composed typed adapter registry.
    /// The same registry serves every operation and work claim on this root.
    pub fn bind_with_registry(
        state_root: &Path,
        socket_path: &Path,
        limits: WorkLimits,
        registry: AdapterRegistry,
        resources: NativeServiceResources,
    ) -> Result<Self> {
        Self::bind_with_registry_and_pump(
            state_root,
            socket_path,
            limits,
            registry,
            resources,
            None,
        )
    }

    /// Binds separately registered native text read capabilities to the same
    /// service root and resource plan as its build adapters.
    pub fn bind_with_registry_and_queries(
        state_root: &Path,
        socket_path: &Path,
        limits: WorkLimits,
        registry: AdapterRegistry,
        queries: QueryAdapterRegistry<PlannedTextRead, TextReadResult>,
        resources: NativeServiceResources,
    ) -> Result<Self> {
        Self::bind_with_capabilities(
            state_root,
            socket_path,
            limits,
            NativeServiceCapabilities {
                builders: registry,
                literal: QueryAdapterRegistry::new(),
                path: QueryAdapterRegistry::new(),
                text: queries,
                semantic: QueryAdapterRegistry::new(),
                pump: None,
                provider_attempt_pool: None,
            },
            resources,
        )
    }

    /// Opt in to bounded catalog-owned background materialization.
    ///
    /// Only the
    /// listed workspace epochs are polled; the registry must contain their
    /// owner adapters. Existing bind factories leave the pump disabled.
    pub fn bind_with_registry_and_pump(
        state_root: &Path,
        socket_path: &Path,
        limits: WorkLimits,
        registry: AdapterRegistry,
        resources: NativeServiceResources,
        pump: Option<NativePumpConfig>,
    ) -> Result<Self> {
        Self::bind_with_capabilities(
            state_root,
            socket_path,
            limits,
            NativeServiceCapabilities {
                builders: registry,
                literal: QueryAdapterRegistry::new(),
                path: QueryAdapterRegistry::new(),
                text: QueryAdapterRegistry::new(),
                semantic: QueryAdapterRegistry::new(),
                pump,
                provider_attempt_pool: None,
            },
            resources,
        )
    }

    /// Binds all configured native capabilities under one state-root lease.
    /// Reader ceilings are validated before creating any persistent state.
    pub fn bind_with_capabilities(
        state_root: &Path,
        socket_path: &Path,
        limits: WorkLimits,
        capabilities: NativeServiceCapabilities,
        resources: NativeServiceResources,
    ) -> Result<Self> {
        Self::bind_with_capabilities_and_maintenance(
            state_root,
            socket_path,
            limits,
            capabilities,
            resources,
            None,
        )
    }

    /// Enable bounded service-owned reclaim of objects already queued by
    /// their custody authority. History retirement remains explicit.
    pub fn bind_with_capabilities_and_maintenance(
        state_root: &Path,
        socket_path: &Path,
        limits: WorkLimits,
        capabilities: NativeServiceCapabilities,
        resources: NativeServiceResources,
        maintenance: Option<NativeReclaimConfig>,
    ) -> Result<Self> {
        Self::bind_with_retention(
            state_root,
            socket_path,
            limits,
            capabilities,
            resources,
            maintenance,
            None,
        )
    }

    /// Install explicit sequence-window retirement before the service opens.
    /// The existing maintenance class and manager also own this work.
    pub fn bind_with_retention(
        state_root: &Path,
        socket_path: &Path,
        limits: WorkLimits,
        capabilities: NativeServiceCapabilities,
        resources: NativeServiceResources,
        maintenance: Option<NativeReclaimConfig>,
        retention: Option<NativeRetentionConfig>,
    ) -> Result<Self> {
        let _needs = capabilities.validate_resources(resources, maintenance, retention.as_ref())?;
        let lease = Arc::new(StateRootLease::acquire(state_root)?);
        let authority = prepare_native_boot_authority(
            lease,
            socket_path,
            resources,
            &RequestBudgetV1::unbounded(),
        )?;
        Self::bind_with_boot_authority(
            authority,
            socket_path,
            limits,
            capabilities,
            resources,
            maintenance,
            retention,
        )
    }

    pub(super) fn bind_with_boot_authority(
        authority: NativeBootAuthority,
        socket_path: &Path,
        limits: WorkLimits,
        capabilities: NativeServiceCapabilities,
        resources: NativeServiceResources,
        maintenance: Option<NativeReclaimConfig>,
        retention: Option<NativeRetentionConfig>,
    ) -> Result<Self> {
        authority.request.checkpoint("native service bind")?;
        let (core_demand, enrichment_demand) =
            capabilities.validate_resources(resources, maintenance, retention.as_ref())?;
        let owned_work_enabled = capabilities.pump.is_some();
        let maintenance_enabled = maintenance.is_some();
        let NativeServiceCapabilities {
            builders: registry,
            literal: literal_queries,
            path: path_queries,
            text: queries,
            semantic: semantic_queries,
            pump,
            provider_attempt_pool,
        } = capabilities;
        let ledger = NativeResourceLedger::with_maintenance(
            resources.execution,
            pump.is_some(),
            maintenance.is_some(),
        )?;
        let NativeBootAuthority {
            lease,
            physical,
            io,
            request,
        } = authority;
        let root = lease.state_root_identity_v1();
        let socket_entry = resolved_socket_entry(socket_path)?;
        lease.require_state_root_v1(root)?;
        let diagnostic_context = request.diagnostic_context().cloned();
        let object_io_observation = diagnostic_context
            .as_ref()
            .map(|_| Arc::new(ObjectIoObservation::default()));
        // Runtime construction fails before the marker, catalog or object
        // store may be created. The manager owns its Tokio runtime lifetime.
        let worker = NativeOperationWorker::start_with_resources(
            &ledger,
            resources.execution,
            core_demand,
            enrichment_demand,
            pump.as_ref().map_or(1, |config| config.max_in_flight),
        )?;
        worker.bind_provider_owner(lease.clone())?;
        let mut startup = WorkBudget::with_request(NATIVE_BOOT_WORK_LIMITS, request);
        open_native_format(root, &socket_entry, &physical, &mut startup)?;
        let scratch = Arc::new(
            FileScratchSpace::open_managed_with_io_budgeted(
                root.join("native-scratch-v1"),
                resources.scratch_reserved_bytes,
                io.clone(),
                &mut startup,
            )
            .context("open admitted native scratch namespace")?,
        );
        // Reopening may recreate WAL/SHM and run Catalog recovery while
        // foreground has no unassigned capacity. Use the already-held parent
        // through recovery and namespace binding, before admitting requests.
        let boot_funding = io
            .enter_maintenance_scope()
            .context("admit native Catalog recovery funding")?;
        let catalog = Arc::new(
            SqliteCatalog::open_with_io_admission(root, Duration::from_secs(2), io.clone())
                .context("open admitted native Catalog and recover")?,
        );
        // Retire one bounded page before creating this process's session.
        // OS lock custody decides death; remaining backlog retains the existing
        // recovery cursor contract and explicit maintenance authority.
        let _recovery = catalog
            .recover_read_sessions(None, MAX_RECLAIM_DRAIN_PAGE, &mut startup)
            .context("recover dead native read sessions before session birth")?;
        let object_root = root.join("native-objects");
        let lock = object_root.join(".object-namespace.lease");
        io.prepare_lock(&lock)
            .context("prepare native object namespace lock")?;
        let read = Arc::new(NativeObjectReadIo::new(Arc::clone(&physical)));
        let objects = Arc::new({
            let objects = FileObjectStore::open_managed_with_io(
                object_root,
                scratch.clone(),
                catalog.clone(),
                ManagedObjectIo {
                    namespace_lock: read.open(&lock, &mut startup)?,
                    read,
                    write: Arc::new(NativeObjectWriteIo::new(Arc::clone(&physical))),
                    reclaim: Arc::new(NativeObjectReclaimIo::new(Arc::clone(&physical))),
                },
            )
            .context("open admitted native object namespace")?;
            match &object_io_observation {
                Some(observer) => objects.with_io_observation(Arc::clone(observer)),
                None => objects,
            }
        });
        let services = KernelServices::new(
            objects.clone(),
            objects.clone(),
            catalog.clone(),
            scratch,
            resources.read_ticket_lease,
        )
        .context("open admitted native Kernel read session")?
        .with_object_map_backend(objects.clone());
        drop(boot_funding);
        let read_sessions = Arc::new(OnceLock::new());
        let dispatcher = Arc::new(NativeDispatcher {
            read_sessions: Arc::clone(&read_sessions),
            kernel: Arc::new(Kernel::new(services, registry)),
            catalog,
            literal_queries,
            path_queries,
            text_queries: queries,
            semantic_queries,
            objects,
            limits,
            worker: Arc::new(worker),
            control_scheduler: ledger.scheduler(NativeResourceClass::Control),
            pump_control_scheduler: ledger.scheduler(NativeResourceClass::BackgroundControl),
            interactive_scheduler: ledger.scheduler(NativeResourceClass::Interactive),
            enrichment_scheduler: ledger.scheduler(NativeResourceClass::Enrichment),
            resources: ledger,
            control_need: resources.execution.control,
            interactive_need: resources.execution.interactive,
            enrichment_need: resources.execution.enrichment,
            effort: OnceLock::new(),
            interaction: OnceLock::new(),
            graph: OnceLock::new(),
            repomap: OnceLock::new(),
            overlay: OnceLock::new(),
            stub_ast: OnceLock::new(),
            root_lease: Arc::clone(&lease),
            wal_checkpoint_bytes: resources.physical.maintenance_reserved_bytes,
            physical,
            io: Arc::clone(&io),
        });
        // Direct Enrichment alone cannot fill all transport slots, even when
        // several owned Enrichment jobs execute. Catalog CAS and fences
        // arbitrate publication.
        let admission = native_server_admission()?;
        let server = UdsServer::bind_observed(
            socket_path,
            admission,
            SocketAccessPolicy::Private,
            Arc::new(IpcServerCounters::for_plane("native-index")),
        )?;
        let pump = if pump.is_some() || maintenance.is_some() {
            Some(NativeOwnedPump::start(
                Arc::new(NativePumpServices {
                    read_sessions,
                    kernel: Arc::clone(&dispatcher.kernel),
                    worker: Arc::clone(&dispatcher.worker),
                    resources: Arc::clone(&dispatcher.resources),
                    pump_control_scheduler: Arc::clone(&dispatcher.pump_control_scheduler),
                    maintenance_scheduler: dispatcher
                        .resources
                        .scheduler(NativeResourceClass::MaintenanceControl),
                    catalog: Arc::clone(&dispatcher.catalog),
                    objects: Arc::clone(&dispatcher.objects),
                    control_need: dispatcher.control_need,
                    io: Arc::clone(&dispatcher.io),
                    wal_checkpoint_bytes: dispatcher.wal_checkpoint_bytes,
                    diagnostic_context,
                }),
                pump,
                maintenance,
                retention,
                server.shutdown_handle(),
            )?)
        } else {
            None
        };
        Ok(Self {
            server,
            dispatcher,
            pump,
            lease,
            provider_attempt_pool,
            execution_plan: resources.execution,
            owned_work_enabled,
            maintenance_enabled,
            object_io_observation,
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

    #[must_use]
    pub fn resource_observation(&self) -> NativeResourceObservation {
        self.dispatcher.resources.observation()
    }

    /// The configured regular-file authority; excludes device/RSS/total-FD claims.
    pub fn physical_observation(
        &self,
    ) -> Result<quanta_index_core::publication_capacity::CapacitySnapshot> {
        Ok(self.dispatcher.physical.capacity_snapshot()?)
    }

    /// Unassigned byte/file reserve; this is not actual device free space.
    pub fn maintenance_reserve_available(&self) -> Result<(u64, u64)> {
        Ok(self.dispatcher.physical.maintenance_available()?)
    }

    pub fn run(mut self, accept_idle: Duration) -> Result<()> {
        let accept_idle = if self.pump.is_some() {
            accept_idle.min(Duration::from_millis(100))
        } else {
            accept_idle
        };
        let server_result = self.server.run::<NativeIndexRequestEnvelope, NativeIndexRequest,
            crate::native_reply::NativeReplyEnvelope, crate::native_reply::NativeReply, NativeDispatcher>(
                &self.dispatcher, IpcPlane::Control, accept_idle,
            );
        let read_sessions_result = crate::native_read_session_transport::drain(&self.dispatcher);
        let pump_result = self.pump.take().map_or(Ok(()), |mut pump| pump.stop_join());
        // The worker joins after the pump releases its receiver. Physical
        // provider attempts retain their grant and root independently; logical
        // worker quiescence is not a claim that transport IO has completed.
        let worker_result = self.dispatcher.worker.stop_join();
        let provider_result = self.provider_attempt_pool.take().map_or(Ok(()), |pool| {
            // This is shutdown grace, never a renewed execution deadline.
            // An unfinished attempt still owns the root through its lease.
            let report = pool
                .drain_and_handoff(Duration::from_secs(5))
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            if report.unfinished.is_empty() && report.panicked.is_empty() {
                Ok(())
            } else {
                Err(IndexError::Storage(format!(
                    "native provider drain unfinished: [{}], panicked: [{}]",
                    report.unfinished.join(","),
                    report.panicked.join(",")
                )))
            }
        });
        finish_native_service(
            server_result.map_err(Into::into),
            read_sessions_result,
            pump_result,
            worker_result,
            provider_result,
        )
    }
}

fn finish_native_service(
    server: Result<()>,
    read_sessions: Result<(), IndexError>,
    pump: Result<(), IndexError>,
    worker: Result<(), IndexError>,
    provider: Result<(), IndexError>,
) -> Result<()> {
    let mut failures = Vec::new();
    if let Err(error) = server {
        failures.push(format!("native listener failed: {error:#}"));
    }
    if let Err(error) = read_sessions {
        failures.push(format!("native read-session drain failed: {error}"));
    }
    if let Err(error) = pump {
        failures.push(format!("native owned pump failed: {error}"));
    }
    if let Err(error) = worker {
        failures.push(format!("native operation worker failed: {error}"));
    }
    if let Err(error) = provider {
        failures.push(format!("native provider drain failed: {error}"));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(failures.join("; ")))
    }
}

#[cfg(test)]
mod native_closeout_tests {
    use super::*;

    use quanta_index_core::index::{
        CatalogView, CollectionId, Digest, GroupActivation, IndexKind, InputDomain, InstanceSpec,
        MapRoot, MemoryLifetime, OwnerKey, ProducerId, PublicationGroupId, ReadSessionRef,
        ReadTicketLease, SchemaId, SlotId, SourceCut, SourceRecord, SourceTextAdmission,
        StableOwnerId, StorageFormatId, TenantId, TicketId, TicketRecord, ViewRef, WorkspaceKey,
        WorktreeId,
    };

    fn page_fixture<R>(result: R, exhaustive: bool) -> Result<QueryOutcome<R>, IndexError> {
        let view = ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            sequence: 1,
            digest: Digest([5; 32]),
        };
        let ticket = TicketRecord {
            lease: ReadTicketLease {
                id: TicketId([6; 16]),
                session: ReadSessionRef { id: 1, fence: 1 },
                fence: 1,
                expires_at_ms: 10_000,
            },
            view: CatalogView {
                identity: view,
                profile: ObjectRef(Digest([7; 32])),
                source: SourceCut {
                    owners: MapRoot(None),
                    paths: MapRoot(None),
                    frontiers: MapRoot(None),
                },
                projections: MapRoot(None),
                members: MapRoot(None),
                coverage: MapRoot(None),
            },
        };
        let instance = InstanceSpec {
            instance: IndexInstanceId([8; 16]),
            kind: IndexKind([9; 16]),
            group: PublicationGroupId([10; 16]),
            activation: GroupActivation::CoreRequired,
            input_domain: InputDomain::SourceLocal,
            schema: SchemaId([11; 16]),
            policy: Digest([12; 32]),
            format: StorageFormatId([13; 16]),
        };
        QueryOutcome::core_scanned(&ticket, &instance, exhaustive, result)
    }

    fn page_ledger() -> Result<
        (
            Arc<NativeResourceLedger>,
            WorkBudget,
            ReservationGuard<NativeClassScheduler>,
        ),
        IndexError,
    > {
        let need = ResourceNeed {
            cpu_units: 1,
            io_bytes: 4096,
            memory_bytes: 65_536,
            provider_units: 0,
        };
        let ledger = NativeResourceLedger::new(NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 8,
                io_bytes: 32_768,
                memory_bytes: 1_048_576,
                provider_units: 0,
            },
            control: need,
            interactive: need,
            core: need,
            enrichment: need,
        })?;
        let guard =
            ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Interactive), need)
                .ok_or(IndexError::ResourceLimited)?;
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        work.bind_memory_scope(guard.memory_scope());
        Ok((ledger, work, guard))
    }

    fn admitted_page_reply(
        literal: bool,
        work: &WorkBudget,
        denied: bool,
    ) -> Result<crate::native_reply::NativeReply, IndexError> {
        let owner = OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([2; 16]),
        };
        let path_memory = work.reserve_memory(4, MemoryLifetime::Retained)?;
        let path = String::from("a.rs");
        let bytes = ObjectRef(Digest([3; 32]));
        let source = ObjectRef(Digest([4; 32]));
        let reserve = |bytes, lifetime| {
            if denied {
                Err(IndexError::ResourceLimited)
            } else {
                work.reserve_memory(bytes, lifetime)
            }
        };
        if literal {
            let hit_memory = work.reserve_memory(
                u64::try_from(std::mem::size_of::<quanta_index_lexical::native::LiteralHit>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?;
            let hits = vec![quanta_index_lexical::native::LiteralHit::from_parts(
                owner,
                SourceRecord {
                    path,
                    bytes,
                    byte_len: 9,
                    text: SourceTextAdmission::Excluded,
                },
                path_memory,
            )];
            let after_memory = work.reserve_memory(32, MemoryLifetime::Retained)?;
            let after = Some(literal_owner_cursor(&owner).to_vec());
            let page = LiteralPage::from_parts(hits, after, true, hit_memory, after_memory);
            native_literal_result(page_fixture(page, false)?, 1, None, reserve)
                .map(crate::native_reply::NativeReply::Literal)
        } else {
            let entry_memory = work.reserve_memory(
                u64::try_from(std::mem::size_of::<
                    quanta_index_repomap::native::FilePathEntry,
                >())
                .map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?;
            let entries = vec![quanta_index_repomap::native::FilePathEntry::from_parts(
                owner,
                path,
                source,
                bytes,
                9,
                path_memory,
            )];
            let page = FilePathPage::from_parts(entries, false, entry_memory);
            native_path_result(page_fixture(page, true)?, "a", None, 1, reserve)
                .map(crate::native_reply::NativeReply::Path)
        }
    }

    #[test]
    fn staged_source_reply_retains_only_memory_until_canonical_emission_ends()
    -> Result<(), Box<dyn std::error::Error>> {
        let (ledger, work, guard) = page_ledger()?;
        let bytes = u64::try_from(std::mem::size_of::<ObjectRef>())?;
        let memory = work.reserve_memory(bytes, MemoryLifetime::Retained)?;
        let reference = ObjectRef(Digest([91; 32]));
        let reply = crate::native_reply::NativeReply::StagedSources(
            quanta_index_core::index::StagedObjectReferences::from_parts(vec![reference], memory)?,
        );
        drop(guard);
        let snapshot = ledger.snapshot()?;
        if snapshot.live_memory_bytes != bytes
            || snapshot.retained_memory_bytes != bytes
            || snapshot.pending_escape_bytes != 0
            || snapshot.reserved != ResourceNeed::default()
        {
            return Err("staged source reply lost data-only retained custody".into());
        }
        let canonical = NativeIndexResponse::StagedSources(vec![reference]);
        let encoded = serde_json::to_vec(&reply)?;
        if encoded != serde_json::to_vec(&canonical)?
            || serde_json::from_slice::<NativeIndexResponse>(&encoded)? != canonical
        {
            return Err("retained source reply changed canonical wire identity".into());
        }
        if ledger.snapshot()?.live_memory_bytes != bytes {
            return Err("source serialization refunded live backing early".into());
        }
        drop(reply);
        let snapshot = ledger.snapshot()?;
        if snapshot.live_memory_bytes != 0
            || snapshot.retained_memory_bytes != 0
            || snapshot.reserved != ResourceNeed::default()
        {
            return Err("source reply did not release its final allocation owner".into());
        }
        Ok(())
    }

    #[test]
    fn ordinal_map_position_keeps_only_memory_after_execution_and_final_clone()
    -> Result<(), Box<dyn std::error::Error>> {
        let (ledger, work, guard) = page_ledger()?;
        let expected = quanta_index_core::index::OrdinalMapEdit::allocation_bytes(7)?;
        let position = quanta_index_core::index::OrdinalMapEdit::copy_object_key_parts(
            2,
            &[b"path\0", b"ab"],
            None,
            Some(ObjectRef(Digest([82; 32]))),
            &work,
        )?;
        let continuation = position.clone();
        drop(guard);
        let snapshot = ledger.snapshot()?;
        if snapshot.live_memory_bytes != expected
            || snapshot.retained_memory_bytes != expected
            || snapshot.reserved != ResourceNeed::default()
            || snapshot.pending_escape_bytes != 0
        {
            return Err("owned edit retained an execution slot or lost key custody".into());
        }
        drop(position);
        if ledger.snapshot()?.live_memory_bytes != expected || continuation.edit.key != b"path\0ab"
        {
            return Err("cursor continuation refunded live key backing".into());
        }
        drop(continuation);
        if ledger.snapshot()?.live_memory_bytes != 0 {
            return Err("final key clone did not refund".into());
        }
        Ok(())
    }

    #[test]
    fn literal_and_path_result_custody_survives_cancel_and_shared_wire_emission()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::native_reply::NativeReply;
        for literal in [true, false] {
            let (ledger, work, guard) = page_ledger()?;
            let reply = admitted_page_reply(literal, &work, false)?;
            drop(guard);
            let before = ledger.snapshot()?;
            if before.live_memory_bytes == 0
                || before.live_memory_bytes != before.retained_memory_bytes
                || before.reserved != ResourceNeed::default()
                || before.pending_escape_bytes != 0
            {
                return Err("page result lost data-only retained custody".into());
            }
            let shared = match &reply {
                NativeReply::Literal(result) => {
                    let shared = result.clone();
                    if shared.hits.as_ptr() != result.hits.as_ptr() {
                        return Err("literal sharing copied hits".into());
                    }
                    NativeReply::Literal(shared)
                }
                NativeReply::Path(result) => {
                    let shared = result.clone();
                    if shared.entries.as_ptr() != result.entries.as_ptr() {
                        return Err("path sharing copied entries".into());
                    }
                    NativeReply::Path(shared)
                }
                NativeReply::Plain(_)
                | NativeReply::Error(_)
                | NativeReply::Text(_)
                | NativeReply::Semantic(_)
                | NativeReply::Graph(_)
                | NativeReply::RepoMapRanked(_)
                | NativeReply::Stub(_)
                | NativeReply::Overlay(_)
                | NativeReply::OverlayText(_)
                | NativeReply::OverlaySemantic(_)
                | NativeReply::EffortDone(_)
                | NativeReply::SourceOwners(_)
                | NativeReply::ProducerSourceOwners(_)
                | NativeReply::ProjectionOwners(_)
                | NativeReply::ProjectionSourceDiff(_)
                | NativeReply::StagedSources(_) => {
                    return Err("unexpected page reply".into());
                }
            };
            work.request().cancel_handle().cancel();
            let encoded = serde_json::to_vec(&shared)?;
            let decoded: NativeIndexResponse = serde_json::from_slice(&encoded)?;
            let valid = if let NativeIndexResponse::LiteralPage(page) = decoded {
                literal && page.has_more && page.hits.len() == 1
            } else if let NativeIndexResponse::PathPage(page) = decoded {
                !literal && !page.has_more && page.entries.len() == 1
            } else {
                false
            };
            if !valid {
                return Err("page wire shape changed".into());
            }
            drop(reply);
            if ledger.snapshot()?.live_memory_bytes != before.live_memory_bytes {
                return Err("shared page charge changed during cancellation/emission".into());
            }
            drop(shared);
            if ledger.snapshot()?.live_memory_bytes != 0 {
                return Err("last page owner leaked memory".into());
            }
        }
        Ok(())
    }

    #[test]
    fn denied_literal_and_path_conversion_releases_input_custody()
    -> Result<(), Box<dyn std::error::Error>> {
        for literal in [true, false] {
            let (ledger, work, guard) = page_ledger()?;
            if !matches!(
                admitted_page_reply(literal, &work, true),
                Err(IndexError::ResourceLimited)
            ) {
                return Err("page mapping ignored capacity denial".into());
            }
            if ledger.snapshot()?.live_memory_bytes != 0 {
                return Err("denied conversion leaked input memory".into());
            }
            drop(guard);
            if ledger.snapshot()?.reserved != ResourceNeed::default() {
                return Err("denied conversion retained execution".into());
            }
        }
        Ok(())
    }

    #[test]
    fn native_ipc_slots_preserve_core_capacity_beyond_direct_enrichment() {
        let result = native_server_admission();
        assert!(result.is_ok(), "native admission policy: {result:?}");
        if let Ok(admission) = result {
            assert!(
                admission
                    .dispatch_slots()
                    .checked_sub(DIRECT_ENRICHMENT_DISPATCH_LIMIT)
                    .is_some_and(|remaining| remaining >= PROTECTED_CORE_DISPATCH_SLOTS)
            );
            assert_eq!(
                admission.max_in_flight_per_repo(),
                admission.dispatch_slots()
            );
        }
    }

    #[test]
    fn listener_pump_worker_and_provider_failures_remain_visible_after_all_joins()
    -> Result<(), Box<dyn std::error::Error>> {
        let result = finish_native_service(
            Err(anyhow::anyhow!("listener failed")),
            Err(IndexError::Storage("read lease drain failed".into())),
            Err(IndexError::Storage("owned settlement failed".into())),
            Err(IndexError::Storage("worker failed".into())),
            Err(IndexError::Storage("physical provider unfinished".into())),
        );
        let Err(error) = result else {
            return Err("all failures must be returned".into());
        };
        let message = error.to_string();
        if !message.contains("read lease drain failed")
            || !message.contains("listener failed")
            || !message.contains("owned settlement failed")
            || !message.contains("worker failed")
            || !message.contains("physical provider unfinished")
        {
            return Err(format!("native closeout lost a failure: {message}").into());
        }
        Ok(())
    }
}
