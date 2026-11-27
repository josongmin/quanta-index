//! Composition-owned factory: one canonical binder, distinct held work parent
//! and Lance Session for each concurrent Semantic publication.

use std::fs::Metadata;
use std::io::Write as _;
use std::path::{Component, Components, Path, PathBuf};
use std::sync::Arc;

use quanta_index_core::CoreError;
use quanta_index_core::domains::generation::GenerationStorageKeyV1;
use quanta_index_core::publication_admission::{
    PublicationControlOpenTicket, PublicationObjectDeleteTicket, PublicationObjectLifetimePort,
    PublicationObjectOpenTicket, PublicationObjectReader, PublicationOutputClass,
    PublicationOutputPermit, PublicationOutputReservation,
};
use quanta_index_core::publication_capacity::PublicationAllowance;
use quanta_index_semantic::legacy::layout::{
    BUILD_CONTRACT_FILE_NAME, DATASET_DIR_NAME, DATASET_STAGING_DIR_NAME, MANIFEST_FILE_NAME,
    MARKER_SEALED_FILE_NAME, QUARANTINE_RECEIPT_FILE_NAME, SCRUB_RECEIPT_FILE_NAME,
    SEALED_MANIFEST_FILE_NAME,
};
use quanta_index_semantic::{
    SemanticFileAdmissionFactory, SemanticFileMove, SemanticGenerationFileIo,
    SemanticMaintenanceFileFactory, SemanticStagingTargetMode, SemanticStagingUpload,
    SemanticWorkListEntry,
};

use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::{OwnedNamespaceTicket, PublicationFileDeletions};
use crate::publication_sidecar::sidecar_output_permit;

#[path = "semantic_file_factory/staged_copy.rs"]
mod staged_copy;
#[path = "semantic_file_factory/staged_file.rs"]
mod staged_file;
#[path = "semantic_file_factory/staged_multipart.rs"]
mod staged_multipart;
#[cfg(test)]
#[path = "semantic_file_factory/staging_recovery_tests.rs"]
mod staging_recovery_tests;

pub struct CanonicalSemanticFileAdmissionFactory {
    deletions: Arc<PublicationFileDeletions>,
}

impl CanonicalSemanticFileAdmissionFactory {
    #[must_use]
    pub fn new(deletions: Arc<PublicationFileDeletions>) -> Self {
        Self { deletions }
    }
}

impl SemanticFileAdmissionFactory for CanonicalSemanticFileAdmissionFactory {
    fn control_reader_lifetime(&self) -> Arc<dyn PublicationObjectLifetimePort> {
        Arc::new(SemanticControlReaderLifetime {
            binder: self.deletions.binder(),
            deletions: Arc::clone(&self.deletions),
        })
    }

    fn for_work(
        &self,
        held_work: Arc<PublicationAllowance>,
        generation_dir: &Path,
        base_generation_dir: Option<&Path>,
    ) -> Result<Arc<dyn SemanticGenerationFileIo>, CoreError> {
        let binder = self.deletions.binder();
        let root = quanta_index_semantic::semantic_state_root(binder.state_root());
        if canonical_generation_tail(&root, generation_dir)?
            .next()
            .is_some()
        {
            return Err(CoreError::InvalidContract(
                "semantic promotion authority requires a generation directory".into(),
            ));
        }
        let base_dataset_dir = if let Some(base) = base_generation_dir {
            if canonical_generation_tail(&root, base)?.next().is_some()
                || base.parent() != generation_dir.parent()
            {
                return Err(CoreError::InvalidContract(
                    "semantic build requires a canonical base in the target generation family"
                        .into(),
                ));
            }
            Some(base.join(DATASET_DIR_NAME))
        } else {
            None
        };
        binder.validate_held_allowance(&held_work)?;
        Ok(Arc::new(SemanticGenerationFiles {
            binder,
            deletions: Arc::clone(&self.deletions),
            generation_dir: generation_dir.to_path_buf(),
            dataset_dir: generation_dir.join(DATASET_DIR_NAME),
            staging_dir: generation_dir.join(DATASET_STAGING_DIR_NAME),
            base_dataset_dir,
            held_work,
        }))
    }
}

struct SemanticGenerationFiles {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
    generation_dir: PathBuf,
    dataset_dir: PathBuf,
    staging_dir: PathBuf,
    base_dataset_dir: Option<PathBuf>,
    held_work: Arc<PublicationAllowance>,
}

impl SemanticGenerationFiles {
    fn require_entry(&self, path: &Path) -> Result<(), CoreError> {
        if !is_canonical_child(&self.generation_dir, path) {
            return Err(CoreError::InvalidContract(
                "semantic promotion entry is outside its admitted generation".into(),
            ));
        }
        self.binder.validate_held_allowance(&self.held_work)
    }

    fn require_staging_file(&self, path: &Path) -> Result<(), CoreError> {
        if !canonical_suffix(&self.staging_dir, path)
            .is_some_and(|suffix| !suffix.as_os_str().is_empty())
        {
            return Err(CoreError::InvalidContract(
                "semantic staging file is outside its admitted dataset".into(),
            ));
        }
        self.binder.validate_held_allowance(&self.held_work)
    }

    fn work_dataset_suffix<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        canonical_suffix(&self.staging_dir, path)
            .or_else(|| canonical_suffix(self.base_dataset_dir.as_deref()?, path))
    }

    fn prepare_staging_file(&self, path: &Path) -> Result<(), CoreError> {
        self.require_staging_file(path)?;
        self.reconcile_namespace()?;
        self.binder.ensure_private_parent_directories(path)
    }

    fn prepare_staging_namespace_pair(
        &self,
        source: &Path,
        target: &Path,
    ) -> Result<(), CoreError> {
        self.require_staging_file(source)?;
        self.require_staging_file(target)?;
        if source == target {
            return Err(CoreError::InvalidContract(
                "semantic staging source equals target".into(),
            ));
        }
        // The exact namespace begin below reconciles a started ticket for the
        // same paths. A general sweep would consume the caller's outcome.
        self.binder.ensure_private_parent_directories(target)
    }

    fn prepare_inherited_file(&self, source: &Path, target: &Path) -> Result<(), CoreError> {
        if !inherited_paths_match(
            &self.dataset_dir,
            self.base_dataset_dir.as_deref(),
            &self.staging_dir,
            source,
            target,
        ) {
            return Err(CoreError::InvalidContract(
                "semantic inherited file is outside the admitted datasets or changes its relative path".into(),
            ));
        }
        self.binder.validate_held_allowance(&self.held_work)?;
        self.binder.ensure_private_parent_directories(target)
    }
}

fn is_canonical_child(parent: &Path, path: &Path) -> bool {
    path.parent() == Some(parent)
        && path
            .file_name()
            .is_some_and(|name| parent.join(name).as_os_str() == path.as_os_str())
}

fn canonical_suffix<'a>(root: &Path, path: &'a Path) -> Option<&'a Path> {
    let suffix = path.strip_prefix(root).ok()?;
    let mut canonical = root.to_path_buf();
    for component in suffix.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        canonical.push(name);
    }
    (canonical.as_os_str() == path.as_os_str()).then_some(suffix)
}

fn inherited_paths_match(
    dataset: &Path,
    base: Option<&Path>,
    staging: &Path,
    source: &Path,
    target: &Path,
) -> bool {
    let Some(suffix) = canonical_suffix(staging, target) else {
        return false;
    };
    !suffix.as_os_str().is_empty()
        && (source.as_os_str() == dataset.join(suffix).as_os_str()
            || base.is_some_and(|root| source.as_os_str() == root.join(suffix).as_os_str()))
}

impl SemanticGenerationFileIo for SemanticGenerationFiles {
    fn reconcile_namespace(&self) -> Result<(), CoreError> {
        self.binder.validate_held_allowance(&self.held_work)?;
        self.deletions
            .reconcile_pending_namespace_under(&self.generation_dir)?;
        self.deletions
            .reconcile_retired_creates_under(&self.staging_dir)
    }

    fn prepare_entry_parent(&self, target: &Path) -> Result<(), CoreError> {
        self.require_entry(target)?;
        self.binder.ensure_private_parent_directories(target)
    }

    fn prepare_staging_directory(&self, directory: &Path) -> Result<(), CoreError> {
        if canonical_suffix(&self.staging_dir, directory).is_none() {
            return Err(CoreError::InvalidContract(
                "semantic staging directory escapes the admitted generation".into(),
            ));
        }
        self.binder.validate_held_allowance(&self.held_work)?;
        self.binder
            .ensure_private_parent_directories(&directory.join(".directory-parent-witness"))
    }

    fn link_inherited_file(&self, source: &Path, target: &Path) -> Result<(), CoreError> {
        self.prepare_inherited_file(source, target)?;
        self.deletions
            .begin_link(source, target)?
            .perform_and_sync()
    }

    fn copy_inherited_file(&self, source: &Path, target: &Path) -> Result<(), CoreError> {
        self.prepare_inherited_file(source, target)?;
        self.reconcile_namespace()?;
        staged_copy::copy_staged_file(&self.binder, source, target, Arc::clone(&self.held_work))
    }

    fn begin_move(
        &self,
        source: &Path,
        target: &Path,
    ) -> Result<Box<dyn SemanticFileMove>, CoreError> {
        self.require_entry(source)?;
        self.require_entry(target)?;
        Ok(Box::new(self.deletions.begin_move(source, target)?))
    }

    fn remove_file_and_sync(&self, path: &Path) -> Result<(), CoreError> {
        self.require_entry(path)?;
        self.deletions.delete_file_and_sync(path)
    }

    fn open_work_dataset_reader(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationObjectReader>, CoreError> {
        if !self
            .work_dataset_suffix(path)
            .is_some_and(|suffix| !suffix.as_os_str().is_empty())
        {
            return Err(CoreError::InvalidContract(
                "semantic work reader is outside its staging or base dataset".into(),
            ));
        }
        self.binder.validate_held_allowance(&self.held_work)?;
        // An observer must leave a failed mutation's receipt with its owner.
        self.deletions.require_namespace_settled_for_read()?;
        let lease = self.binder.begin_reader_open(path)?.into_held_reader()?;
        Ok(crate::publication_lexical_lifetime::object_reader(lease))
    }

    fn list_work_dataset(
        &self,
        prefix: &Path,
        recursive: bool,
    ) -> Result<Vec<SemanticWorkListEntry>, CoreError> {
        if self.work_dataset_suffix(prefix).is_none() {
            return Err(CoreError::InvalidContract(
                "semantic work listing is outside its staging or base dataset".into(),
            ));
        }
        self.binder.validate_held_allowance(&self.held_work)?;
        self.deletions.require_namespace_settled_for_read()?;
        self.binder.list_work_dataset(prefix, recursive)
    }

    fn put_staging_file(
        &self,
        path: &Path,
        mode: SemanticStagingTargetMode,
        chunks: &[&[u8]],
    ) -> Result<Option<Metadata>, CoreError> {
        let bytes = staged_file::payload_width(chunks)?;
        self.prepare_staging_file(path)?;
        let held_work = Arc::clone(&self.held_work);
        let mutation = match mode {
            SemanticStagingTargetMode::Create => self
                .binder
                .begin_create_if_absent_with_allowance(path, bytes.max(1), held_work)?,
            SemanticStagingTargetMode::Overwrite => Some(self.binder.begin_create_with_allowance(
                path,
                bytes.max(1),
                held_work,
            )?),
        };
        let Some(mutation) = mutation else {
            return Ok(None);
        };
        staged_file::publish(mutation, path, mode, "semantic staging PUT", |writer| {
            for chunk in chunks {
                writer
                    .write_all(chunk)
                    .map_err(|error| CoreError::storage_io("semantic staging PUT write", &error))?;
            }
            let metadata = writer.metadata().map_err(|error| {
                CoreError::storage_io("semantic staging PUT writer metadata", &error)
            })?;
            if metadata.len() != bytes {
                return Err(CoreError::Storage(
                    "semantic staging PUT writer width differs from payload".into(),
                ));
            }
            Ok(metadata)
        })
    }

    fn begin_staging_upload(
        &self,
        path: &Path,
    ) -> Result<Box<dyn SemanticStagingUpload>, CoreError> {
        self.prepare_staging_file(path)?;
        let mutation =
            self.binder
                .begin_create_with_allowance(path, 1, Arc::clone(&self.held_work))?;
        let upload = staged_multipart::StagedMultipart::open(mutation, path.to_path_buf())?;
        Ok(Box::new(upload))
    }

    fn copy_staging_file(
        &self,
        source: &Path,
        target: &Path,
        mode: SemanticStagingTargetMode,
    ) -> Result<(), CoreError> {
        self.prepare_staging_namespace_pair(source, target)?;
        let mut ticket = match mode {
            SemanticStagingTargetMode::Create => self.deletions.begin_link(source, target)?,
            SemanticStagingTargetMode::Overwrite => {
                self.deletions.begin_copy_overwrite(source, target)?
            }
        };
        ticket.perform_and_sync()
    }

    fn rename_staging_file(
        &self,
        source: &Path,
        target: &Path,
        mode: SemanticStagingTargetMode,
    ) -> Result<(), CoreError> {
        self.prepare_staging_namespace_pair(source, target)?;
        self.deletions
            .begin_file_move(source, target, mode == SemanticStagingTargetMode::Overwrite)?
            .perform_and_sync()
    }

    fn remove_staging_file_and_sync(&self, path: &Path) -> Result<(), CoreError> {
        self.require_staging_file(path)?;
        self.deletions.delete_file_and_sync(path)
    }

    fn remove_directory_and_sync(&self, path: &Path) -> Result<(), CoreError> {
        self.require_entry(path)?;
        self.deletions
            .begin_directory_delete(path)?
            .perform_and_sync()
    }
}

fn canonical_generation_tail<'a>(root: &Path, path: &'a Path) -> Result<Components<'a>, CoreError> {
    let relative = canonical_suffix(root, path).ok_or_else(|| {
        CoreError::InvalidContract("semantic path escapes the canonical held root".into())
    })?;
    let mut parts = relative.components();
    let family = parts.next();
    let generation = parts.next();
    let valid_generation = matches!(family, Some(Component::Normal(name)) if {
        name.to_str().is_some_and(GenerationStorageKeyV1::is_canonical_name)
    }) && matches!(generation, Some(Component::Normal(name)) if {
        name.to_str().and_then(|name| name.strip_prefix('g')).is_some_and(|generation| {
            generation.parse::<u64>().ok().is_some_and(|value| value.to_string() == generation)
        })
    });
    if !valid_generation {
        return Err(CoreError::InvalidContract(
            "semantic read path is not beneath a canonical generation".into(),
        ));
    }
    Ok(parts)
}

/// Read-only custody for generation Control, receipt, and dataset files.
/// The canonical binder owns the open guard and exact reader FD lifetime.
struct SemanticControlReaderLifetime {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
}

impl SemanticControlReaderLifetime {
    fn semantic_root(&self) -> std::path::PathBuf {
        quanta_index_semantic::semantic_state_root(self.binder.state_root())
    }

    fn generation_tail<'a>(&self, path: &'a Path) -> Result<Components<'a>, CoreError> {
        canonical_generation_tail(&self.semantic_root(), path)
    }

    fn require_sealed_control_path(&self, path: &Path) -> Result<(), CoreError> {
        let mut parts = self.generation_tail(path)?;
        if !matches!(parts.next(), Some(Component::Normal(name)) if {
            name == MARKER_SEALED_FILE_NAME
                || name == MANIFEST_FILE_NAME
                || name == BUILD_CONTRACT_FILE_NAME
                || name == SEALED_MANIFEST_FILE_NAME
                || name == SCRUB_RECEIPT_FILE_NAME
                || name == QUARANTINE_RECEIPT_FILE_NAME
        }) || parts.next().is_some()
        {
            return Err(CoreError::InvalidContract(
                "semantic control path is not an approved generation control file".into(),
            ));
        }
        Ok(())
    }

    fn require_dataset_path(&self, path: &Path) -> Result<(), CoreError> {
        let mut parts = self.generation_tail(path)?;
        if !matches!(parts.next(), Some(Component::Normal(name)) if {
            name == quanta_index_semantic::legacy::layout::DATASET_DIR_NAME
        }) {
            return Err(CoreError::InvalidContract(
                "semantic reader path is outside its generation dataset".into(),
            ));
        }
        let mut has_file = false;
        for component in parts {
            if !matches!(component, Component::Normal(_)) {
                return Err(CoreError::InvalidContract(
                    "semantic dataset path has a non-normal component".into(),
                ));
            }
            has_file = true;
        }
        if !has_file {
            return Err(CoreError::InvalidContract(
                "semantic dataset reader has no file name".into(),
            ));
        }
        Ok(())
    }
}

impl PublicationObjectLifetimePort for SemanticControlReaderLifetime {
    fn validate_state_root(&self, root: &Path) -> Result<(), CoreError> {
        if root.as_os_str() != self.semantic_root().as_os_str() {
            return Err(CoreError::InvalidContract(
                "semantic root differs from held file authority".into(),
            ));
        }
        self.binder.validate_current_state_root()
    }

    fn validate_parent(&self, parent: &Arc<PublicationAllowance>) -> Result<(), CoreError> {
        self.binder.validate_held_allowance(parent)
    }

    fn begin_control_reader_open(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationControlOpenTicket>, CoreError> {
        self.validate_state_root(&self.semantic_root())?;
        self.require_sealed_control_path(path)?;
        self.deletions.require_namespace_settled_for_read()?;
        Ok(crate::publication_control_files::cold_control_open(
            self.binder.begin_reader_open(path)?,
        ))
    }

    fn begin_reader_open(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationObjectOpenTicket>, CoreError> {
        self.validate_state_root(&self.semantic_root())?;
        self.require_dataset_path(path)?;
        self.deletions.require_namespace_settled_for_read()?;
        Ok(crate::publication_lexical_lifetime::object_open(
            self.binder.begin_reader_open(path)?,
        ))
    }

    fn begin_delete(
        &self,
        _path: &Path,
    ) -> Result<Box<dyn PublicationObjectDeleteTicket>, CoreError> {
        Err(CoreError::InvalidContract(
            "semantic Control lifetime has no delete authority".into(),
        ))
    }
}

/// Reserve maintenance receipts under the canonical authority.
///
/// This factory creates one parent, then one child in the common inode binder.
/// Its `SidecarOutputPermit` retains the parent `Arc`.
pub struct CanonicalSemanticMaintenanceFileFactory {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
}

fn require_retirement_entry(semantic_root: &Path, entry: &Path) -> Result<(), CoreError> {
    let reclaim_area = semantic_root.join(quanta_index_core::RECLAIM_AREA_DIR_NAME);
    if !is_canonical_child(&reclaim_area, entry) {
        return Err(CoreError::InvalidContract(
            "semantic retirement target is not a direct reclaim-area entry".into(),
        ));
    }
    Ok(())
}

fn require_retirement_source(semantic_root: &Path, source: &Path) -> Result<(), CoreError> {
    let relative = source.strip_prefix(semantic_root).map_err(|_error| {
        CoreError::InvalidContract("semantic retirement source escapes semantic root".into())
    })?;
    let components = relative.components().collect::<Vec<_>>();
    if source.as_os_str() != semantic_root.join(relative).as_os_str()
        || !matches!(
            components.as_slice(),
            [Component::Normal(_)] | [Component::Normal(_), Component::Normal(_)]
        )
        || relative.starts_with(quanta_index_core::RECLAIM_AREA_DIR_NAME)
    {
        return Err(CoreError::InvalidContract(
            "semantic retirement source is not a direct generation path".into(),
        ));
    }
    Ok(())
}

impl CanonicalSemanticMaintenanceFileFactory {
    #[must_use]
    pub fn new(deletions: Arc<PublicationFileDeletions>) -> Self {
        Self {
            binder: deletions.binder(),
            deletions,
        }
    }

    fn semantic_root(&self) -> std::path::PathBuf {
        quanta_index_semantic::semantic_state_root(self.binder.state_root())
    }
}

impl SemanticFileMove for OwnedNamespaceTicket {
    fn perform_and_sync(&mut self) -> Result<(), CoreError> {
        OwnedNamespaceTicket::perform_and_sync(self)
    }
}

impl SemanticMaintenanceFileFactory for CanonicalSemanticMaintenanceFileFactory {
    fn reserve_sidecar_output(
        &self,
        max_encoded_bytes: u64,
    ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
        let bound = max_encoded_bytes.max(1);
        self.deletions.reconcile_pending_namespace()?;
        let parent = self.binder.reserve_maintenance_allowance(bound)?;
        let request =
            PublicationOutputReservation::new(PublicationOutputClass::Semantic, bound, 1)?;
        sidecar_output_permit(Arc::clone(&self.deletions), parent, request)
    }

    fn reconcile_retirement_namespace(&self) -> Result<(), CoreError> {
        self.deletions.reconcile_pending_namespace()
    }

    fn begin_retirement_directory_move(
        &self,
        source: &Path,
        target: &Path,
    ) -> Result<Box<dyn SemanticFileMove>, CoreError> {
        let semantic_root = self.semantic_root();
        require_retirement_source(&semantic_root, source)?;
        require_retirement_entry(&semantic_root, target)?;
        Ok(Box::new(
            self.deletions.begin_directory_move(source, target)?,
        ))
    }

    fn delete_retirement_directory_and_sync(&self, path: &Path) -> Result<(), CoreError> {
        require_retirement_entry(&self.semantic_root(), path)?;
        self.deletions
            .begin_directory_delete(path)?
            .perform_and_sync()
    }

    fn prepare_retirement_area(&self, entry: &Path) -> Result<(), CoreError> {
        require_retirement_entry(&self.semantic_root(), entry)?;
        self.binder.ensure_private_parent_directories(entry)
    }
}

#[cfg(test)]
mod retirement_scope_tests {
    use super::{
        canonical_generation_tail, inherited_paths_match, is_canonical_child,
        require_retirement_entry, require_retirement_source,
    };
    use std::path::Path;

    #[test]
    fn promotion_entry_must_be_a_direct_child_of_its_bound_generation() {
        let root = Path::new("/held/indexes/semantic");
        let family = format!("generation-v1-{}", "a".repeat(64));
        let generation = root.join(&family).join("g7");
        assert!(
            canonical_generation_tail(root, &generation)
                .is_ok_and(|mut tail| tail.next().is_none())
        );
        for invalid in [
            root.join(&family).join("./g7"),
            root.join(&family).join("g07"),
            root.join(&family).join("../g7"),
        ] {
            assert!(canonical_generation_tail(root, &invalid).is_err());
        }
        assert!(is_canonical_child(
            &generation,
            &generation.join("dataset.staging")
        ));
        for entry in [
            generation.to_path_buf(),
            generation.join("dataset/staging"),
            generation.join("./dataset.staging"),
            generation.join("../g8/dataset.staging"),
        ] {
            assert!(!is_canonical_child(&generation, &entry));
        }
    }

    #[test]
    fn inherited_file_must_keep_its_path_from_the_target_or_its_declared_base() {
        let dataset = Path::new("/held/semantic/family/g7/dataset");
        let base = Path::new("/held/semantic/family/g6/dataset");
        let staging = Path::new("/held/semantic/family/g7/dataset.staging");
        let relative = "main.lance/data/part.lance";
        let target = staging.join(relative);
        for source in [dataset.join(relative), base.join(relative)] {
            assert!(inherited_paths_match(
                dataset,
                Some(base),
                staging,
                &source,
                &target
            ));
        }
        for (source, target) in [
            (base.join("main.lance/data/different.lance"), target.clone()),
            (base.join("../dataset/part.lance"), target.clone()),
            (
                base.join(relative),
                staging.join("./main.lance/data/part.lance"),
            ),
            (base.join(relative), dataset.join(relative)),
            (base.join(relative), staging.to_path_buf()),
            (
                Path::new("/held/semantic/family/g5/dataset").join(relative),
                target.clone(),
            ),
        ] {
            assert!(!inherited_paths_match(
                dataset,
                Some(base),
                staging,
                &source,
                &target
            ));
        }
        assert!(!inherited_paths_match(
            dataset,
            None,
            staging,
            &base.join(relative),
            &target
        ));
    }

    #[test]
    fn retirement_paths_stay_in_semantic_generation_and_direct_reclaim_entry() {
        let root = Path::new("/held/indexes/semantic");
        assert!(require_retirement_source(root, &root.join("family/g7")).is_ok());
        assert!(require_retirement_source(root, &root.join("family")).is_ok());
        assert!(require_retirement_entry(root, &root.join(".reclaim/family.g7")).is_ok());

        for source in [
            root.join(".reclaim/family.g7"),
            root.join("family/g7/dataset"),
            root.join("../catalog"),
            Path::new("/held/catalog").to_path_buf(),
        ] {
            assert!(require_retirement_source(root, &source).is_err());
        }
        for entry in [
            root.join("family/g7"),
            root.join(".reclaim/family.g7/child"),
            root.join(".reclaim/../family.g7"),
            Path::new("/held/catalog").to_path_buf(),
        ] {
            assert!(require_retirement_entry(root, &entry).is_err());
        }
    }
}
