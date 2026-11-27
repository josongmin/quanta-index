//! Filesystem mutations around the Lance connection use its same authority.
//! Legacy callers without admission retain their existing syscall behavior.

use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::sync::Arc;

use lance::{dataset::DEFAULT_INDEX_CACHE_SIZE, session::Session};
use lance_io::object_store::ObjectStoreRegistry;
use quanta_index_core::CoreError;
#[cfg(feature = "qualified-memory-uri-candidate")]
use quanta_index_core::publication_admission::PublicationObjectLifetimePort;
use quanta_index_core::{DiagnosticContextV1, FinishedReclaims};
use sha2::{Digest as _, Sha256};

#[cfg(feature = "qualified-memory-uri-candidate")]
use crate::SemanticGenerationFileIo;
use crate::SemanticMaintenanceFileFactory;
use crate::errors::{admitted_fs_err, fs_err};

#[cfg(feature = "qualified-memory-uri-candidate")]
mod candidate_store;
#[cfg(feature = "qualified-memory-uri-candidate")]
pub use candidate_store::{AdmittedMemorySealedStore, AdmittedMemoryWorkStore};
#[cfg(feature = "qualified-memory-uri-candidate")]
mod candidate_view;
#[cfg(feature = "qualified-memory-uri-candidate")]
pub(crate) use candidate_view::{CandidateView, CandidateViews};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LanceUriMode {
    File,
    #[cfg(feature = "qualified-memory-uri-candidate")]
    AdmittedMemoryCandidate,
}

/// The upstream IVF metadata cache can retain a reader that owns the same
/// cache. A persisted session keeps the normal index cache and never stores
/// file metadata, so releasing its last owner also releases the readers.
pub(crate) fn new_persisted_lance_session() -> Arc<Session> {
    persisted_session(Arc::new(ObjectStoreRegistry::default()))
}

fn persisted_session(registry: Arc<ObjectStoreRegistry>) -> Arc<Session> {
    Arc::new(Session::new(DEFAULT_INDEX_CACHE_SIZE, 0, registry))
}

#[cfg(feature = "qualified-memory-uri-candidate")]
pub(crate) fn lance_candidate_work_session(
    generation: Arc<dyn SemanticGenerationFileIo>,
) -> Arc<Session> {
    // The adapter owns the registry and only passes the session to its own
    // Lance calls. Callers supply neither a registry nor object-store params.
    let registry = Arc::new(ObjectStoreRegistry::empty());
    registry.insert(
        "memory",
        Arc::new(candidate_store::WorkDatasetProvider(
            AdmittedMemoryWorkStore::new(generation),
        )),
    );
    persisted_session(registry)
}

/// Cold opens bind the already sealed dataset to the product FD lifetime.
#[cfg(feature = "qualified-memory-uri-candidate")]
pub(crate) fn lance_candidate_read_session(
    control_lifetime: &Arc<dyn PublicationObjectLifetimePort>,
) -> ReadSession {
    ReadSession::SealedCandidate(Arc::clone(control_lifetime))
}

pub(crate) enum ReadSession {
    Existing(Option<Arc<Session>>),
    #[cfg(feature = "qualified-memory-uri-candidate")]
    SealedCandidate(Arc<dyn PublicationObjectLifetimePort>),
}

impl ReadSession {
    pub(crate) fn resolve(
        self,
        dataset_dir: &Path,
        artifacts: Vec<quanta_index_core::domains::generation::SealedArtifactCommitmentV1>,
    ) -> Result<Arc<Session>, CoreError> {
        #[cfg(not(feature = "qualified-memory-uri-candidate"))]
        {
            let _ = dataset_dir;
            drop(artifacts);
        }
        match self {
            Self::Existing(session) => Ok(session.unwrap_or_else(new_persisted_lance_session)),
            #[cfg(feature = "qualified-memory-uri-candidate")]
            Self::SealedCandidate(lifetime) => {
                let store = AdmittedMemorySealedStore::new(dataset_dir, artifacts, lifetime)?;
                let registry = Arc::new(ObjectStoreRegistry::empty());
                registry.insert(
                    "memory",
                    Arc::new(candidate_store::SealedDatasetProvider(store)),
                );
                Ok(persisted_session(registry))
            }
        }
    }
}

/// Exercise the persistent admitted store through Lance's object-store path.
/// The default keeps the existing file URI until the candidate passes product
/// and upstream-package qualification.
pub(crate) fn lance_connection_uri(
    dataset_dir: &Path,
    mode: LanceUriMode,
) -> Result<String, CoreError> {
    match mode {
        LanceUriMode::File => crate::layout::dataset_dir_to_uri(dataset_dir),
        #[cfg(feature = "qualified-memory-uri-candidate")]
        LanceUriMode::AdmittedMemoryCandidate => {
            let file_url = url::Url::from_file_path(dataset_dir).map_err(|()| {
                CoreError::Storage(format!(
                    "semantic: candidate dataset path must be absolute: {}",
                    dataset_dir.display()
                ))
            })?;
            let candidate =
                url::Url::parse(&format!("memory://{}", file_url.path())).map_err(|error| {
                    CoreError::Storage(format!("semantic: invalid candidate dataset URI: {error}"))
                })?;
            let object = candidate_store::dataset_path_from_url(&candidate)
                .map_err(|error| CoreError::Storage(format!("semantic: {error}")))?;
            let physical = candidate_store::local_path(&object)
                .map_err(|error| CoreError::Storage(format!("semantic: {error}")))?;
            if physical != dataset_dir {
                return Err(CoreError::Storage(format!(
                    "semantic: candidate dataset URI changed physical path: {}",
                    dataset_dir.display()
                )));
            }
            Ok(candidate.into())
        }
    }
}

#[cfg(all(test, feature = "qualified-memory-uri-candidate"))]
mod uri_tests {
    use super::{LanceUriMode, Path, candidate_store, lance_connection_uri};

    #[test]
    fn candidate_uri_preserves_the_physical_path_and_table_suffix()
    -> Result<(), Box<dyn std::error::Error>> {
        for directory in [
            "/tmp/quanta/dataset",
            "/tmp/quanta with spaces/dataset",
            "/tmp/quanta%2Ffile/dataset",
            "/tmp/quanta#한글?/dataset",
        ] {
            let dataset = Path::new(directory);
            let uri = lance_connection_uri(dataset, LanceUriMode::AdmittedMemoryCandidate)?;
            // ListingDatabase appends the table name to the serialized URI.
            for (uri, expected) in [
                (uri.clone(), dataset.to_path_buf()),
                (
                    format!("{uri}/semantic.lance"),
                    dataset.join("semantic.lance"),
                ),
            ] {
                let url = lance_io::object_store::uri_to_url(&uri)?;
                let object = candidate_store::dataset_path_from_url(&url)?;
                if candidate_store::local_path(&object)? != expected {
                    return Err(format!("candidate URI selected a different file: {uri}").into());
                }
            }
        }
        Ok(())
    }

    #[test]
    fn candidate_uri_rejects_nonabsolute_or_ambiguous_roots()
    -> Result<(), Box<dyn std::error::Error>> {
        for directory in ["relative/dataset", "/tmp/quanta/../other/dataset"] {
            if lance_connection_uri(Path::new(directory), LanceUriMode::AdmittedMemoryCandidate)
                .is_ok()
            {
                return Err(format!("candidate URI accepted ambiguous root: {directory}").into());
            }
        }
        for uri in [
            "memory://host/dataset",
            "memory:///dataset?option=1",
            "memory:///dataset#anchor",
        ] {
            let url = url::Url::parse(uri)?;
            if candidate_store::dataset_path_from_url(&url).is_ok() {
                return Err(format!("candidate accepted non-path URI: {uri}").into());
            }
        }
        Ok(())
    }
}

/// Crash-atomic qualified reclaim: every moved or removed directory passes
/// through the same physical binder as Lance's files. A previous interrupted
/// reclaim of this exact generation is completed first.
pub(crate) fn reclaim_directory(
    track_root: &Path,
    source: &Path,
    entry_name: &str,
    factory: Option<&dyn SemanticMaintenanceFileFactory>,
) -> Result<(), CoreError> {
    let Some(factory) = factory else {
        return quanta_index_core::reclaim_directory(track_root, source, entry_name);
    };
    let relative = source.strip_prefix(track_root).map_err(|error| {
        CoreError::InvalidContract(format!(
            "semantic reclaim source outside track root: {error}"
        ))
    })?;
    if !matches!(
        relative.components().collect::<Vec<_>>().as_slice(),
        [std::path::Component::Normal(_)]
            | [
                std::path::Component::Normal(_),
                std::path::Component::Normal(_)
            ]
    ) || !matches!(
        Path::new(entry_name)
            .components()
            .collect::<Vec<_>>()
            .as_slice(),
        [std::path::Component::Normal(_)]
    ) {
        return Err(CoreError::InvalidContract(
            "semantic reclaim requires a family/generation source and one-component entry".into(),
        ));
    }
    let entry = quanta_index_core::reclaim_area(track_root).join(entry_name);
    factory.prepare_retirement_area(&entry)?;
    factory
        .reconcile_retirement_namespace()
        .map_err(|error| admitted_fs_err("reconcile earlier reclaim", error))?;
    match fs::symlink_metadata(&entry) {
        Ok(_) => factory
            .delete_retirement_directory_and_sync(&entry)
            .map_err(|error| admitted_fs_err("remove dataset tree", error))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(fs_err("inspect earlier reclaim", &entry, &error)),
    }
    let mut move_ticket = factory
        .begin_retirement_directory_move(source, &entry)
        .map_err(|error| admitted_fs_err("reserve reclaim directory move", error))?;
    move_ticket
        .perform_and_sync()
        .map_err(|error| admitted_fs_err("move reclaim directory", error))?;
    factory
        .delete_retirement_directory_and_sync(&entry)
        .map_err(|error| admitted_fs_err("remove dataset tree", error))
}

pub(crate) fn reclaim_quarantined_directory(
    track_root: &Path,
    source: &Path,
    factory: Option<&dyn SemanticMaintenanceFileFactory>,
) -> Result<(), CoreError> {
    let Some(factory) = factory else {
        return quanta_index_core::reclaim_quarantined_directory(track_root, source);
    };
    let relative = source.strip_prefix(track_root).map_err(|error| {
        CoreError::InvalidContract(format!(
            "semantic quarantine source outside track root: {error}"
        ))
    })?;
    let digest = Sha256::digest(relative.as_os_str().as_bytes());
    reclaim_directory(
        track_root,
        source,
        &format!("quarantine-{digest:x}"),
        Some(factory),
    )
}

pub(crate) fn finish_interrupted_reclaims(
    track_root: &Path,
    diagnostic: Option<&DiagnosticContextV1>,
    factory: Option<&dyn SemanticMaintenanceFileFactory>,
) -> Result<FinishedReclaims, CoreError> {
    let Some(factory) = factory else {
        return quanta_index_core::finish_interrupted_reclaims(track_root, diagnostic);
    };
    factory
        .reconcile_retirement_namespace()
        .map_err(|error| admitted_fs_err("reconcile interrupted reclaim", error))?;
    let area = quanta_index_core::reclaim_area(track_root);
    match fs::symlink_metadata(&area) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FinishedReclaims::default());
        }
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(CoreError::Storage(format!(
                "semantic reclaim area is not a directory: {}",
                area.display()
            )));
        }
        Err(error) => return Err(fs_err("inspect reclaim area", &area, &error)),
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&area).map_err(|error| fs_err("list reclaim area", &area, &error))? {
        entries.push(
            entry
                .map_err(|error| fs_err("read reclaim entry", &area, &error))?
                .path(),
        );
    }
    let (bytes, present) = quanta_index_core::unique_inode_tree_bytes_for_roots_below_track(
        track_root,
        &entries,
        &|_name| false,
        diagnostic,
    )?;
    let mut removed = 0_u64;
    for (entry, present) in entries.iter().zip(present) {
        if !present {
            continue;
        }
        factory
            .delete_retirement_directory_and_sync(entry)
            .map_err(|error| admitted_fs_err("remove dataset tree", error))?;
        removed = removed.checked_add(1).ok_or_else(|| {
            CoreError::InvalidContract("semantic reclaim entry count overflow".into())
        })?;
    }
    Ok(FinishedReclaims {
        entries: removed,
        bytes,
    })
}
