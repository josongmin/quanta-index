//! One catalog-bound lexical publication; no ID or incarnation inference.

use std::fs::File;
use std::path::{Component, Path};
use std::sync::Arc;

use rustix::fs::{Mode, OFlags, mkdirat, open, openat};

use quanta_index_contract::{BatchIngestMode, SearchCorpusIngestBatch};
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
}

pub(crate) struct NativeMutation {
    pub(crate) sources: Vec<NativeSourceEdit>,
    pub(crate) retired: Vec<u64>,
    pub(crate) added: Vec<AddedTextDoc>,
    pub(crate) clear_all: bool,
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
    pub(crate) fn begin(
        state_root: &Path,
        batch: &SearchCorpusIngestBatch,
        reservation: &LexicalDocumentVersionReservationV1,
        base_directory: Option<&Path>,
        sync_totals: Option<Arc<crate::causal_profile::SyncTotalsV1>>,
        diagnostic: Option<&DiagnosticContextV1>,
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
                ensure_publication_store_root(state_root, sync_totals.as_deref())?;
                let store = ObjectStore::open_observed(state_root, sync_totals)?;
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
        })
    }

    pub(crate) fn finish(mut self) -> Result<(), CoreError> {
        let file = self.file_root.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("lexical publication has no File root".into())
        })?;
        let native = self.native_root.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("lexical publication has no Native root".into())
        })?;
        let mut roots = file.object_roots();
        roots.extend(native.root_objects());
        self.lease
            .take()
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical publication has no adoption lease".into())
            })?
            .finish_published(&roots)
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
