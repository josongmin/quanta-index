//! One catalog-bound lexical publication; no ID or incarnation inference.

use std::fs::File;
use std::path::{Component, Path};
use std::sync::Arc;

use rustix::fs::{Mode, OFlags, mkdirat, open, openat};

use quanta_index_contract::{BatchIngestMode, SearchCorpusIngestBatch};
use quanta_index_core::publication_admission::{
    PublicationControlOutputAdmission, PublicationObjectLifetimePort,
    PublicationWorkOutputAdmission, validate_control_object_lifetime, validate_object_lifetime,
};
use quanta_index_core::{
    CoreError, DiagnosticContextV1, DiagnosticStageV1, LexicalDocumentVersionReservationV1,
};
use sha2::{Digest as _, Sha256};

use crate::file_authority::paged::{AuthorityRoot, FileAuthorityMutation};
use crate::lexical_root_names::MAX_ROOT_BYTES;
pub(crate) use crate::lexical_root_names::{FILE_ROOT_NAME, NATIVE_ROOT_NAME};
use crate::lexical_store::{AttemptBinding, AttemptLease, ObjectStore};
use crate::text_authority::{AddedTextDoc, NativeRoot, NativeSourceEdit};

/// Establish the object store track directory after publication admission.
///
/// Pin the nearest existing state-root ancestor. Create missing descendants
/// through directory descriptors without following links.
fn ensure_publication_store_root(
    state_root: &Path,
    sync: Option<&crate::causal_profile::SyncTotalsV1>,
) -> Result<(), CoreError> {
    let absolute = if state_root.is_absolute() {
        state_root.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| CoreError::Storage(format!("lexical: current directory: {error}")))?
            .join(state_root)
    };
    if absolute
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(CoreError::InvalidContract(
            "lexical publication state root contains parent traversal".into(),
        ));
    }
    let flags =
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let mode = Mode::RUSR | Mode::WUSR | Mode::XUSR;
    let mut anchor = absolute.as_path();
    loop {
        match std::fs::symlink_metadata(anchor) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => break,
            Ok(_) => {
                return Err(CoreError::Storage(format!(
                    "lexical: publication state-root ancestor {} is not a real directory",
                    anchor.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(Component::Normal(_name)) = anchor.components().next_back() else {
                    return Err(CoreError::InvalidContract(
                        "lexical publication state root has no existing ancestor".into(),
                    ));
                };
                anchor = anchor.parent().ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical publication state root has no existing ancestor".into(),
                    )
                })?;
            }
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: inspect publication ancestor {}: {error}",
                    anchor.display()
                )));
            }
        }
    }
    let mut directory = File::from(open(anchor, flags, Mode::empty()).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: pin publication ancestor {}: {error}",
            anchor.display()
        ))
    })?);
    let relative = absolute.strip_prefix(anchor).map_err(|error| {
        CoreError::InvalidContract(format!(
            "lexical publication state root differs from ancestor: {error}"
        ))
    })?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(CoreError::InvalidContract(
                "lexical publication state root has non-directory component".into(),
            ));
        };
        let child = Path::new(name);
        let created = match mkdirat(&directory, child, mode) {
            Ok(()) => true,
            Err(rustix::io::Errno::EXIST) => false,
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: create publication directory {}: {error}",
                    absolute.display()
                )));
            }
        };
        let next = File::from(openat(&directory, child, flags, Mode::empty()).map_err(
            |error| {
                CoreError::Storage(format!(
                    "lexical: pin nofollow publication directory {}: {error}",
                    absolute.display()
                ))
            },
        )?);
        if created {
            crate::causal_profile::observed_sync_legacy(
                sync,
                quanta_index_core::DiagnosticSyncLabelV1::PublicationDirectory,
                || next.sync_all(),
            )
            .map_err(|error| {
                CoreError::Storage(format!("lexical: sync publication directory: {error}"))
            })?;
            crate::causal_profile::observed_sync_legacy(
                sync,
                quanta_index_core::DiagnosticSyncLabelV1::PublicationParent,
                || directory.sync_all(),
            )
            .map_err(|error| {
                CoreError::Storage(format!("lexical: sync publication parent: {error}"))
            })?;
        }
        directory = next;
    }
    Ok(())
}

pub(crate) struct PublicationContext {
    // Field order matters: join read-only preparation before releasing the lease.
    pub(crate) compaction_worker: Option<crate::lexical_compaction::CompactionWorker>,
    pub(crate) store: ObjectStore,
    pub(crate) lease: Option<AttemptLease>,
    pub(crate) reservation: LexicalDocumentVersionReservationV1,
    pub(crate) file_base: Option<AuthorityRoot>,
    pub(crate) native_base: Option<NativeRoot>,
    pub(crate) native_plan: Option<crate::TextAuthorityPlan>,
    pub(crate) file_mutation: Option<FileAuthorityMutation>,
    pub(crate) file_root: Option<AuthorityRoot>,
    pub(crate) native_root: Option<NativeRoot>,
    pub(crate) native_mutation: Option<NativeMutation>,
    pub(crate) operations_sha256: [u8; 32],
    pub(crate) maintenance_policy:
        Option<quanta_index_core::lexical_maintenance::LexicalMaintenancePolicy>,
    pub(crate) hard_scan: Option<crate::lexical_store::CheckpointPayload>,
}

pub(crate) struct PublicationAdmission {
    pub(crate) maintenance_policy:
        Option<quanta_index_core::lexical_maintenance::LexicalMaintenancePolicy>,
    pub(crate) pool_write:
        Option<Arc<dyn quanta_index_core::pool_write_admission::PoolWriteAdmissionPort>>,
    pub(crate) object_lifetime: Option<Arc<dyn PublicationObjectLifetimePort>>,
    pub(crate) control_output: Option<PublicationControlOutputAdmission>,
    pub(crate) work_output: Option<PublicationWorkOutputAdmission>,
}

pub(crate) struct NativeMutation {
    pub(crate) sources: Vec<NativeSourceEdit>,
    pub(crate) retired: Vec<u64>,
    pub(crate) added: Vec<AddedTextDoc>,
    pub(crate) clear_all: bool,
}

/// Admit both authorities together. The cursor exception is available only
/// to a source no-op with identical serving roots and counters.
///
/// The seal still
/// independently derives the permitted cursor transition before publication.
pub(crate) fn admit_maintenance_roots(
    store: &ObjectStore,
    policy: &quanta_index_core::lexical_maintenance::LexicalMaintenancePolicy,
    file: (Option<&AuthorityRoot>, &AuthorityRoot),
    native: (Option<&NativeRoot>, &NativeRoot),
    source_no_op: bool,
) -> Result<(), CoreError> {
    use quanta_index_core::lexical_maintenance::TargetCounters;

    let empty = TargetCounters::new(0, 0, 0)?;
    let file_before = file
        .0
        .map(|root| root.maintenance_snapshot(store))
        .transpose()?
        .unwrap_or(empty);
    let file_after = file.1.maintenance_snapshot(store)?;
    let native_before = native
        .0
        .map(|root| root.run_registry.snapshot())
        .transpose()?
        .unwrap_or(empty);
    let native_after = native.1.run_registry.snapshot()?;
    let ordinary = policy
        .admit_target(file_before, file_after)
        .and_then(|()| policy.admit_target(native_before, native_after));
    if ordinary.is_ok() || !source_no_op {
        return ordinary;
    }
    if policy.hard_pressure(file_before) || policy.hard_pressure(native_before) {
        return ordinary;
    }
    let (Some(file_base), Some(native_base)) = (file.0, native.0) else {
        return ordinary;
    };
    let file_progressed =
        file.1.run_registry.selection_progress() != file_base.run_registry.selection_progress();
    let native_progressed =
        native.1.run_registry.selection_progress() != native_base.run_registry.selection_progress();
    if !file_progressed && !native_progressed {
        return ordinary;
    }
    // A merge's inherited proof witness may be discarded on the next no-op;
    // it is not a serving root. A cursor-only target cannot add a new witness.
    if file.1.premerge_run_registry.is_some()
        || !file
            .1
            .run_registry
            .same_authority_as(&file_base.run_registry)
    {
        return ordinary;
    }
    let mut projected_file = file.1.clone();
    projected_file.run_registry = file_base.run_registry.clone();
    let mut projected_base = file_base.clone();
    projected_base.premerge_run_registry = None;
    if projected_file != projected_base
        || (native.1 != native_base && !native.1.is_cursor_only_successor_of(native_base)?)
    {
        return ordinary;
    }
    policy.admit_cursor_only_target(file_before, file_after)?;
    policy.admit_cursor_only_target(native_before, native_after)
}

pub(crate) fn read_root_bytes(directory: &Path, name: &str) -> Result<Vec<u8>, CoreError> {
    let mut file = crate::sealed_generation::open_regular_nofollow(directory, Path::new(name))
        .map_err(|error| {
            crate::index_store::sidecar_corrupt(directory, name, &error.to_string())
        })?;
    crate::sealed_generation::read_opened_bounded(&mut file, MAX_ROOT_BYTES)
        .map_err(|error| crate::index_store::sidecar_corrupt(directory, name, &error.to_string()))
}

pub(crate) fn read_native_root(directory: &Path) -> Result<NativeRoot, CoreError> {
    let bytes = read_root_bytes(directory, NATIVE_ROOT_NAME)?;
    let root: NativeRoot = crate::channel_payloads::decode_cbor_exact(&bytes).map_err(|error| {
        crate::index_store::sidecar_corrupt(directory, NATIVE_ROOT_NAME, &error)
    })?;
    let canonical = crate::channel_payloads::encode_cbor(&root, "Native root")?;
    if canonical != bytes {
        return Err(crate::index_store::sidecar_corrupt(
            directory,
            NATIVE_ROOT_NAME,
            "noncanonical root",
        ));
    }
    Ok(root)
}

pub(crate) fn write_root_observed(
    directory: &Path,
    name: &str,
    bytes: &[u8],
    sync: Option<&crate::causal_profile::SyncTotalsV1>,
) -> Result<(), CoreError> {
    if bytes.len() > MAX_ROOT_BYTES {
        return Err(CoreError::InvalidContract(
            "lexical root exceeds control-file admission".into(),
        ));
    }
    let (parent, leaf) = name
        .split_once('/')
        .ok_or_else(|| CoreError::InvalidContract("lexical root has no family".into()))?;
    if !matches!(
        (parent, leaf),
        ("file-authority", "root.cbor") | ("text-authority", "manifest.cbor")
    ) {
        return Err(CoreError::InvalidContract(
            "unknown lexical root filename".into(),
        ));
    }
    let root = crate::sealed_generation::open_generation_dir_nofollow(directory)
        .map_err(|error| CoreError::Storage(format!("lexical: pin root directory: {error}")))?;
    match mkdirat(&root, parent, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
        Ok(()) => crate::causal_profile::observed_sync_legacy(
            sync,
            quanta_index_core::DiagnosticSyncLabelV1::AtomicAtParent,
            || root.sync_all(),
        )
        .map_err(|error| CoreError::Storage(format!("lexical: sync root family: {error}")))?,
        Err(rustix::io::Errno::EXIST) => {}
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "lexical: create root family: {error}"
            )));
        }
    }
    let child = std::fs::File::from(
        openat(
            &root,
            parent,
            OFlags::RDONLY
                | OFlags::CLOEXEC
                | OFlags::DIRECTORY
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|error| CoreError::Storage(format!("lexical: pin root family: {error}")))?,
    );
    crate::index_store::write_atomic_durable_at_observed(
        &child,
        &directory.join(parent),
        leaf,
        bytes,
        "lexical authority root",
        sync,
    )
}

impl PublicationContext {
    pub(crate) fn prepare_or_load_hard_scan(
        &mut self,
        mutation: &FileAuthorityMutation,
        native_plan: &crate::TextAuthorityPlan,
        budget: Option<&quanta_index_core::RequestBudgetV1>,
        stage: crate::lexical_hard_scan::HardScanStage,
    ) -> Result<(), CoreError> {
        let Some(policy) = self.maintenance_policy.as_ref() else {
            return Ok(());
        };
        let (clear_all, retired) = match &native_plan.write {
            crate::TextAuthorityWrite::None => (false, &[][..]),
            crate::TextAuthorityWrite::Delta {
                clear_all, retired, ..
            } => (*clear_all, retired.as_slice()),
        };
        let inputs = crate::lexical_hard_scan::HardScanInputs {
            file: self.file_base.as_ref().map(|base| (base, mutation)),
            native: self
                .native_base
                .as_ref()
                .filter(|_base| !clear_all)
                .map(|base| (base, retired)),
        };
        let lease = self.lease.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("hard scan has no publication adoption lease".into())
        })?;
        self.hard_scan = crate::lexical_hard_scan::prepare_or_load(
            &self.store,
            lease,
            &inputs,
            policy,
            budget,
            stage,
        )?;
        Ok(())
    }

    /// Reauthenticate persisted progress and costs before accepting a seal.
    pub(crate) fn verify_hard_scan_for_seal(&self) -> Result<(), CoreError> {
        let Some(policy) = self.maintenance_policy.as_ref() else {
            return Ok(());
        };
        let file = self.file_mutation.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("hard scan seal File mutation is absent".into())
        })?;
        let native = self.native_mutation.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("hard scan seal Native mutation is absent".into())
        })?;
        let inputs = crate::lexical_hard_scan::HardScanInputs {
            file: self.file_base.as_ref().map(|base| (base, file)),
            native: self
                .native_base
                .as_ref()
                .filter(|_base| !native.clear_all)
                .map(|base| (base, native.retired.as_slice())),
        };
        let lease = self.lease.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("hard scan seal adoption lease is absent".into())
        })?;
        let current = crate::lexical_hard_scan::prepare_or_load(
            &self.store,
            lease,
            &inputs,
            policy,
            None,
            crate::lexical_hard_scan::HardScanStage::VerifyForSeal,
        )?;
        if current != self.hard_scan {
            return Err(CoreError::InvalidContract(
                "hard scan checkpoint differs from its pre-mutation custody".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn begin(
        state_root: &Path,
        batch: &SearchCorpusIngestBatch,
        reservation: &LexicalDocumentVersionReservationV1,
        base_directory: Option<&Path>,
        sync_totals: Option<Arc<crate::causal_profile::SyncTotalsV1>>,
        diagnostic: Option<&DiagnosticContextV1>,
        admission: PublicationAdmission,
    ) -> Result<Self, CoreError> {
        reservation.validate()?;
        if reservation.allocation.request.repo_id != batch.repo_id
            || reservation.allocation.request.revision_id != batch.revision_id
            || reservation.allocation.request.event != batch.source_event
            || reservation.allocation.generation != batch.generation
            || (batch.mode == BatchIngestMode::Delta) != base_directory.is_some()
        {
            return Err(CoreError::InvalidContract(
                "lexical publication differs from its catalog allocation or base".into(),
            ));
        }
        let base_roots = base_directory.map(read_committed_roots).transpose()?;
        let file_base = base_roots.as_ref().map(|(file, _)| file.clone());
        let native_base = base_roots.map(|(_, native)| native);
        if file_base
            .as_ref()
            .is_some_and(|root| root.allocation_scope_sha256 != reservation.file.scope_sha256)
            || native_base.as_ref().is_some_and(|root| {
                root.allocation_scope_sha256 != reservation.native_text.scope_sha256
            })
        {
            return Err(CoreError::InvalidContract(
                "lexical delta crosses allocation scopes; publish a full replacement".into(),
            ));
        }
        let operations_sha256 =
            quanta_index_contract::source_event_payload_sha256(batch).map_err(|error| {
                CoreError::InvalidContract(format!("lexical publication body: {error}"))
            })?;
        let base_root_sha256 = if let (Some(file), Some(native)) = (&file_base, &native_base) {
            let mut hash = Sha256::new();
            hash.update(b"lexical-publication-base-roots-v1\0");
            hash.update(file.encode()?);
            hash.update(crate::channel_payloads::encode_cbor(
                native,
                "Native base root",
            )?);
            Some(hash.finalize().into())
        } else {
            None
        };
        let binding = AttemptBinding {
            root_incarnation: reservation.allocation.root_incarnation,
            repository_sha256: Sha256::digest(batch.repo_id.as_str().as_bytes()).into(),
            revision_sha256: Sha256::digest(batch.revision_id.as_str().as_bytes()).into(),
            source_event_sha256: Sha256::digest(crate::channel_payloads::encode_cbor(
                &batch.source_event,
                "lexical source event",
            )?)
            .into(),
            target_generation: batch.generation.get(),
            base_root_sha256,
            operations_sha256,
            format: u16::try_from(
                crate::lexical_root_names::LEXICAL_SEALED_MANIFEST_FORMAT_VERSION,
            )
            .map_err(|_conversion_error| {
                CoreError::InvalidContract("lexical publication format exceeds u16".into())
            })?,
        };
        let (store, lease) = crate::causal_profile::observe_result(
            diagnostic,
            DiagnosticStageV1::Operation,
            |_diagnostic| {
                validate_object_lifetime(
                    admission.work_output.as_ref(),
                    admission.object_lifetime.as_ref(),
                )?;
                validate_control_object_lifetime(
                    admission.control_output.as_ref(),
                    admission.object_lifetime.as_ref(),
                )?;
                if let Some(lifetime) = &admission.object_lifetime {
                    lifetime.validate_state_root(state_root)?;
                }
                ensure_publication_store_root(state_root, sync_totals.as_deref())?;
                let store = ObjectStore::open_observed_with_control(
                    state_root,
                    sync_totals,
                    admission.control_output,
                    admission.object_lifetime,
                )?
                .with_pool_write_admission(admission.pool_write)
                .with_work_output_admission(admission.work_output);
                let mut inherited = file_base
                    .as_ref()
                    .map_or_else(Vec::new, AuthorityRoot::object_roots);
                if let Some(native) = &native_base {
                    inherited.extend(native.root_objects());
                }
                let lease = store.begin_attempt(&binding, &inherited)?;
                Ok::<_, CoreError>((store, lease))
            },
        )?;
        Ok(Self {
            compaction_worker: None,
            store,
            lease: Some(lease),
            reservation: reservation.clone(),
            file_base,
            native_base,
            native_plan: None,
            file_mutation: None,
            file_root: None,
            native_root: None,
            native_mutation: None,
            operations_sha256,
            maintenance_policy: admission.maintenance_policy,
            hard_scan: None,
        })
    }

    pub(crate) fn write_cost_snapshot(
        &self,
    ) -> Result<crate::lexical_store::AttemptWriteCost, CoreError> {
        self.lease
            .as_ref()
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical publication has no adoption lease".into())
            })?
            .write_cost_snapshot()
    }

    /// Capture bounded root summaries without enumerating the object closure.
    /// The caller publishes this observation only after the independent seal
    /// and durable identity succeed.
    pub(crate) fn maintenance_observation(
        &self,
        file_merge: crate::FileRunMergeFacts,
        native_merge: crate::NativeRunMergeFacts,
    ) -> Result<crate::LexicalMaintenanceObservation, CoreError> {
        use quanta_index_core::lexical_maintenance::TargetCounters;

        let file = self.file_root.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("lexical observation has no File target".into())
        })?;
        let native = self.native_root.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("lexical observation has no Native target".into())
        })?;
        let empty = TargetCounters::new(0, 0, 0)?;
        Ok(crate::LexicalMaintenanceObservation {
            file_before: self
                .file_base
                .as_ref()
                .map(|root| root.maintenance_snapshot(&self.store))
                .transpose()?
                .unwrap_or(empty),
            file_after: file.maintenance_snapshot(&self.store)?,
            native_before: self
                .native_base
                .as_ref()
                .map(|root| root.run_registry.snapshot())
                .transpose()?
                .unwrap_or(empty),
            native_after: native.run_registry.snapshot()?,
            file_debt_entries: file.compaction_debt.entries,
            native_debt_entries: native.compaction_due.entries,
            file_merge,
            native_merge,
        })
    }

    /// Enforce logical target limits after prepared compaction has been
    /// consumed, before a sealed identity can make the target servable.
    pub(crate) fn admit_maintenance_target(
        &self,
        policy: &quanta_index_core::lexical_maintenance::LexicalMaintenancePolicy,
    ) -> Result<(), CoreError> {
        let file = self.file_root.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("lexical publication has no File target".into())
        })?;
        let native = self.native_root.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("lexical publication has no Native target".into())
        })?;
        let source_no_op = self
            .file_mutation
            .as_ref()
            .is_some_and(|mutation| !mutation.clear_all && mutation.edits.is_empty())
            && self.native_mutation.as_ref().is_some_and(|mutation| {
                !mutation.clear_all
                    && mutation.sources.is_empty()
                    && mutation.retired.is_empty()
                    && mutation.added.is_empty()
            });
        admit_maintenance_roots(
            &self.store,
            policy,
            (self.file_base.as_ref(), file),
            (self.native_base.as_ref(), native),
            source_no_op,
        )
    }

    pub(crate) fn finish(mut self) -> Result<crate::lexical_store::AttemptWriteCost, CoreError> {
        let file = self.file_root.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("lexical publication has no File root".into())
        })?;
        let native = self.native_root.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("lexical publication has no Native root".into())
        })?;
        let mut roots = file.object_roots();
        roots.extend(native.root_objects());
        let cost = self.write_cost_snapshot()?;
        self.lease
            .take()
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical publication has no adoption lease".into())
            })?
            .finish_published(&roots)?;
        Ok(cost)
    }
}

pub(crate) fn read_committed_roots(
    directory: &Path,
) -> Result<(AuthorityRoot, NativeRoot), CoreError> {
    let root = crate::sealed_generation::open_generation_dir_nofollow(directory)
        .map_err(|error| CoreError::Storage(format!("lexical: pin committed roots: {error}")))?;
    let identity = crate::index_store::read_lexical_sealed_identity_at(directory, &root)?;
    crate::sealed_generation::refuse_if_quarantined_at(&root, directory)?;
    let manifest = crate::sealed_generation::read_bound_manifest_at(
        directory,
        &root,
        &identity.manifest_digest,
    )?;
    let file = manifest.file_authority.first().ok_or_else(|| {
        crate::index_store::sidecar_corrupt(directory, FILE_ROOT_NAME, "root commitment missing")
    })?;
    let native = manifest
        .text_authority
        .as_ref()
        .and_then(|files| files.first())
        .ok_or_else(|| {
            crate::index_store::sidecar_corrupt(
                directory,
                NATIVE_ROOT_NAME,
                "root commitment missing",
            )
        })?;
    let file_bytes = crate::sealed_generation::read_committed(&root, directory, file, None)?;
    let native_bytes = crate::sealed_generation::read_committed(&root, directory, native, None)?;
    let native: NativeRoot =
        crate::channel_payloads::decode_cbor_exact(&native_bytes).map_err(|error| {
            crate::index_store::sidecar_corrupt(directory, NATIVE_ROOT_NAME, &error)
        })?;
    native.validate()?;
    Ok((AuthorityRoot::decode(&file_bytes)?, native))
}
