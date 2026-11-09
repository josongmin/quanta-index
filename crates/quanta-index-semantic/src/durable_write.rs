//! Crash-atomic, crash-durable file replacement for generation sidecars.
//!
//! The lowest layer the build and the integrity receipts both write
//! through: a unique temporary file, synced, renamed over the target, and
//! the parent directory synced.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::{
    PublicationOutputAdmission, PublicationOutputClass, PublicationOutputPermit,
    PublicationOutputReservation,
};

use crate::errors::fs_err;

#[cfg(test)]
static ATOMIC_WRITE_FAIL_BEFORE_RENAME_ACTION: OnceLock<Mutex<Option<String>>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn set_atomic_write_fail_before_rename_action(action: Option<&str>) {
    let slot = ATOMIC_WRITE_FAIL_BEFORE_RENAME_ACTION.get_or_init(|| Mutex::new(None));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(err) => err.into_inner(),
    };
    *guard = action.map(str::to_owned);
}

#[cfg(test)]
fn should_fail_atomic_write_before_rename(action: &str) -> bool {
    let slot = ATOMIC_WRITE_FAIL_BEFORE_RENAME_ACTION.get_or_init(|| Mutex::new(None));
    let guard = match slot.lock() {
        Ok(guard) => guard,
        Err(err) => err.into_inner(),
    };
    guard.as_deref() == Some(action)
}

static ATOMIC_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Custody of one committed sidecar output through later promotion or removal.
/// Dropping this handle does not prove deletion; the permit retains its charge.
pub(crate) struct AtomicOutput {
    path: PathBuf,
    permit: Option<Box<dyn PublicationOutputPermit>>,
    rename_settlement_pending: bool,
}

impl AtomicOutput {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Transfer the same committed output to a new path, then sync both parents.
    pub(crate) fn rename_to(&mut self, target: &Path, action: &str) -> Result<(), CoreError> {
        if let Some(permit) = self.permit.as_deref_mut() {
            if self.rename_settlement_pending {
                if target != self.path {
                    return Err(CoreError::InvalidContract(
                        "sidecar must settle its move before changing targets".into(),
                    ));
                }
            } else {
                permit.move_output(target)?;
                self.path = target.to_path_buf();
                self.rename_settlement_pending = true;
            }
            permit.renamed_and_synced()?;
            self.rename_settlement_pending = false;
            return Ok(());
        }
        let source = self.path.clone();
        let source_parent = source.parent().ok_or_else(|| {
            CoreError::Storage(format!(
                "semantic: {action} source has no parent: {}",
                source.display()
            ))
        })?;
        let target_parent = target.parent().ok_or_else(|| {
            CoreError::Storage(format!(
                "semantic: {action} target has no parent: {}",
                target.display()
            ))
        })?;
        fs::rename(&source, target).map_err(|error| fs_err(action, target, &error))?;
        self.path = target.to_path_buf();
        File::open(source_parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| fs_err(action, source_parent, &error))?;
        if source_parent != target_parent {
            File::open(target_parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| fs_err(action, target_parent, &error))?;
        }
        Ok(())
    }

    /// Remove the currently owned output and settle its charge only after sync.
    pub(crate) fn remove_and_sync(mut self, action: &str) -> Result<(), CoreError> {
        let parent = self.path.parent().ok_or_else(|| {
            CoreError::Storage(format!(
                "semantic: {action} output has no parent: {}",
                self.path.display()
            ))
        })?;
        if let Some(permit) = self.permit.as_deref_mut() {
            return permit.delete_file_and_sync();
        }
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) => return Err(fs_err(action, &self.path, &error)),
        }
        match fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(fs_err(action, &self.path, &error)),
            Ok(_) => {
                return Err(CoreError::Storage(format!(
                    "semantic: {action} output still exists: {}",
                    self.path.display()
                )));
            }
        }
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| fs_err(action, parent, &error))?;
        Ok(())
    }
}

/// Crash-atomic and crash-durable file replacement for generation sidecars.
///
/// The file payload is synced before rename and the parent directory is synced
/// after rename. Pre-rename failures remove the unique temporary file so a
/// failed build cannot accumulate or later promote stale staging artifacts.
pub(crate) fn write_atomic(
    path: &Path,
    bytes: &[u8],
    action: &str,
    publication: Option<PublicationOutputAdmission<'_>>,
) -> Result<AtomicOutput, CoreError> {
    let actual_bytes = u64::try_from(bytes.len()).map_err(|error| {
        CoreError::InvalidContract(format!("semantic: {action} payload width: {error}"))
    })?;
    let permit = if let Some(admission) = publication {
        let request = PublicationOutputReservation::new(
            PublicationOutputClass::Semantic,
            actual_bytes.max(1),
            1,
        )?;
        admission.reserve(request)?
    } else {
        None
    };
    write_atomic_with_reserved_permit(path, bytes, action, permit)
}

/// A maintenance receipt owns a distinct root reservation only when there is
/// no publication parent. The factory supplies that child before this call.
pub(crate) fn write_atomic_with_reserved_permit(
    path: &Path,
    bytes: &[u8],
    action: &str,
    mut permit: Option<Box<dyn PublicationOutputPermit>>,
) -> Result<AtomicOutput, CoreError> {
    let parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: {action} target has no parent: {}",
            path.display()
        ))
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: {action} target has no file name: {}",
            path.display()
        ))
    })?;
    let sequence = ATOMIC_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut staging_name = OsString::from(".");
    staging_name.push(file_name);
    staging_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
    let staging = parent.join(staging_name);

    let actual_bytes = u64::try_from(bytes.len()).map_err(|error| {
        CoreError::InvalidContract(format!("semantic: {action} payload width: {error}"))
    })?;
    if let Some(output) = permit.as_deref_mut() {
        output.bind_output(&staging, path)?;
        let mut file = output.create_staging()?;
        let renamed = (|| {
            output.created(&file, &staging)?;
            write_staging_bytes(&mut file, &staging, bytes, action)?;
            output.rename_staging(&file)
        })();
        if let Err(primary) = renamed {
            return Err(match output.cancel_rejected_created(file) {
                Ok(()) => primary,
                Err(error) => with_cleanup_context(primary, error),
            });
        }
        if let Err(primary) = output.sync_promoted_parents() {
            return Err(match output.retire_writer_preserving_pending(file) {
                Ok(()) => primary,
                Err(error) => with_cleanup_context(primary, error),
            });
        }
        output.committed(file, actual_bytes)?;
    } else {
        let mut file = create_unmanaged_staging(&staging, parent, action)?;
        let renamed = write_staging_bytes(&mut file, &staging, bytes, action).and_then(|()| {
            fs::rename(&staging, path).map_err(|error| fs_err(action, path, &error))
        });
        if let Err(primary) = renamed {
            return Err(match cleanup_unmanaged_temporary(&staging, parent) {
                Ok(()) => primary,
                Err(error) => with_cleanup_context(primary, error),
            });
        }
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| fs_err(action, parent, &error))?;
    }
    Ok(AtomicOutput {
        path: path.to_path_buf(),
        permit,
        rename_settlement_pending: false,
    })
}

fn write_staging_bytes(
    file: &mut File,
    staging: &Path,
    bytes: &[u8],
    action: &str,
) -> Result<(), CoreError> {
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| fs_err(action, staging, &error))?;
    #[cfg(test)]
    if should_fail_atomic_write_before_rename(action) {
        return Err(CoreError::Storage(format!(
            "semantic: injected {action} failure before rename"
        )));
    }
    Ok(())
}

fn create_unmanaged_staging(
    staging: &Path,
    parent: &Path,
    action: &str,
) -> Result<File, CoreError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging)
        .map_err(|error| {
            let primary = fs_err(action, staging, &error);
            // An occupied name may belong to another invocation.
            let absence = match fs::symlink_metadata(staging) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    File::open(parent).and_then(|directory| directory.sync_all())
                }
                Err(error) => Err(error),
                Ok(_) => Ok(()),
            };
            match absence {
                Ok(()) => primary,
                Err(error) => with_cleanup_context(primary, error),
            }
        })
}

pub(crate) fn with_cleanup_context(
    mut primary: CoreError,
    context: impl std::fmt::Display,
) -> CoreError {
    let context = format!("; cleanup failed: {context}");
    let message = match &mut primary {
        CoreError::InvalidContract(message)
        | CoreError::Typed { message, .. }
        | CoreError::NotReady(message)
        | CoreError::NotImplemented(message)
        | CoreError::NotFound(message)
        | CoreError::Storage(message)
        | CoreError::StorageIo { message, .. } => message,
    };
    message.push_str(&context);
    primary
}

fn cleanup_unmanaged_temporary(staging: &Path, parent: &Path) -> std::io::Result<()> {
    match fs::remove_file(staging) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    match fs::symlink_metadata(staging) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
        Ok(_) => {
            return Err(std::io::Error::other(format!(
                "temporary {} still exists",
                staging.display()
            )));
        }
    }
    File::open(parent).and_then(|directory| directory.sync_all())
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::os::unix::fs::MetadataExt as _;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use quanta_index_contract::{
        BatchIngestMode, ManifestGeneration, RepoId, RevisionId, SearchCorpusIngestBatch,
        SourcePublicationEvent,
    };
    use quanta_index_core::publication_admission::{
        PublicationAdmissionRequest, PublicationBuildWork, PublicationResourceAdmissionPort,
        PublicationResourcePermit,
    };
    use quanta_index_core::{
        INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, IngestBatchFootprint, PublicationValidationOwner,
        RequestBudgetV1,
    };

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(CoreError::InvalidContract(format!(
                    "test assertion failed: {} at {}:{}",
                    stringify!($condition),
                    file!(),
                    line!()
                ))
                .into());
            }
        }};
    }

    #[test]
    fn uncreated_temporary_never_unlinks_an_occupied_name() -> TestResult {
        let directory = tempfile::tempdir()?;
        let occupied = directory.path().join(".sidecar.tmp-foreign");
        fs::write(&occupied, b"foreign-winner")?;
        check!(matches!(
            create_unmanaged_staging(&occupied, directory.path(), "create staging"),
            Err(CoreError::StorageIo {
                kind: std::io::ErrorKind::AlreadyExists,
                ..
            })
        ));
        check!(fs::read(&occupied)? == b"foreign-winner");
        Ok(())
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if *left != *right {
                return Err(CoreError::InvalidContract(format!(
                    "test equality failed at {}:{}: left={left:?}, right={right:?}",
                    file!(),
                    line!()
                ))
                .into());
            }
        }};
    }

    macro_rules! check_ne {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if *left == *right {
                return Err(CoreError::InvalidContract(format!(
                    "test inequality failed at {}:{}: left={left:?}, right={right:?}",
                    file!(),
                    line!()
                ))
                .into());
            }
        }};
    }

    fn events_snapshot(events: &Arc<Mutex<Vec<Event>>>) -> Result<Vec<Event>, CoreError> {
        Ok(events
            .lock()
            .map_err(|error| CoreError::Storage(format!("test log poisoned: {error}")))?
            .clone())
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum Event {
        Reserved(u64, u64),
        Bound,
        Started,
        Absent,
        Committed(u64, u64),
        RenameStarted,
        Renamed,
    }

    struct TestParent {
        events: Arc<Mutex<Vec<Event>>>,
        target: PathBuf,
        refuse: bool,
        rename_refuse: bool,
        rename_finish_error: bool,
    }

    impl PublicationResourcePermit for TestParent {
        fn held_file_allowance(
            &self,
        ) -> Result<
            Option<Arc<quanta_index_core::publication_capacity::PublicationAllowance>>,
            CoreError,
        > {
            Ok(None) // This fixture tests output routing, not physical capacity.
        }

        fn reserve_output(
            &self,
            request: PublicationOutputReservation,
        ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
            self.events
                .lock()
                .map_err(|error| CoreError::Storage(format!("test log poisoned: {error}")))?
                .push(Event::Reserved(
                    request.max_encoded_bytes(),
                    request.max_files(),
                ));
            check_eq!(request.class(), PublicationOutputClass::Semantic);
            if self.refuse {
                return Err(CoreError::Typed {
                    code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                    message: "fixed refusal".into(),
                });
            }
            Ok(Box::new(TestOutput {
                events: Arc::clone(&self.events),
                target: self.target.clone(),
                staging: None,
                pending_target: None,
                rename_refuse: self.rename_refuse,
                rename_finish_error: self.rename_finish_error,
            }))
        }
    }

    struct TestOutput {
        events: Arc<Mutex<Vec<Event>>>,
        target: PathBuf,
        staging: Option<PathBuf>,
        pending_target: Option<PathBuf>,
        rename_refuse: bool,
        rename_finish_error: bool,
    }

    impl TestOutput {
        fn staging(&self) -> Result<&Path, CoreError> {
            self.staging.as_deref().ok_or_else(|| {
                CoreError::InvalidContract("test output has no bound staging path".into())
            })
        }

        fn record(&self, event: Event) -> Result<(), CoreError> {
            self.events
                .lock()
                .map_err(|error| CoreError::Storage(format!("test log poisoned: {error}")))?
                .push(event);
            Ok(())
        }

        fn record_absent(&self) -> Result<(), CoreError> {
            check!(
                !self
                    .staging
                    .as_ref()
                    .ok_or_else(|| CoreError::InvalidContract(
                        "test output has no bound staging path".into()
                    ))?
                    .exists()
            );
            self.record(Event::Absent)
        }
    }

    impl PublicationOutputPermit for TestOutput {
        fn cancel_rejected_created(&mut self, writer: std::fs::File) -> Result<(), CoreError> {
            let staging = self.staging()?;
            let parent = staging
                .parent()
                .ok_or_else(|| CoreError::InvalidContract("test staging has no parent".into()))?;
            cleanup_unmanaged_temporary(staging, parent)
                .map_err(|error| fs_err("test cancel staging", staging, &error))?;
            self.confirmed_safe_absent_after_writer_close(writer)
        }
        fn retire_writer_preserving_pending(&mut self, writer: File) -> Result<(), CoreError> {
            // Explicitly unqualified sequencing fixture; physical FD custody
            // is covered by Runtime's actual binder regressions.
            drop(writer);
            Ok(())
        }

        fn bind_output(&mut self, staging: &Path, target: &Path) -> Result<(), CoreError> {
            check_eq!(target, self.target);
            check_eq!(staging.parent(), target.parent());
            check!(!staging.exists());
            self.staging = Some(staging.to_path_buf());
            self.record(Event::Bound)
        }

        fn creation_started(&mut self) -> Result<(), CoreError> {
            check!(
                !self
                    .staging
                    .as_ref()
                    .ok_or_else(|| CoreError::InvalidContract(
                        "test output has no bound staging path".into()
                    ))?
                    .exists()
            );
            self.record(Event::Started)
        }

        fn create_staging(&mut self) -> Result<File, CoreError> {
            self.creation_started()?;
            let staging = self.staging()?;
            let parent = staging
                .parent()
                .ok_or_else(|| CoreError::InvalidContract("test staging has no parent".into()))?;
            create_unmanaged_staging(staging, parent, "test create staging")
        }

        fn created(&mut self, writer: &File, staging: &Path) -> Result<(), CoreError> {
            check_eq!(self.staging.as_deref(), Some(staging));
            check!(writer.metadata().is_ok());
            Ok(())
        }

        fn before_replace(&mut self) -> Result<(), CoreError> {
            Ok(())
        }

        fn rename_staging(&mut self, _writer: &File) -> Result<(), CoreError> {
            self.before_replace()?;
            fs::rename(self.staging()?, &self.target)
                .map_err(|error| fs_err("test rename staging", &self.target, &error))
        }

        fn sync_promoted_parents(&mut self) -> Result<(), CoreError> {
            let parent = self
                .target
                .parent()
                .ok_or_else(|| CoreError::InvalidContract("test target has no parent".into()))?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| fs_err("test sync", parent, &error))
        }

        fn committed(&mut self, writer: File, actual_encoded_bytes: u64) -> Result<(), CoreError> {
            check!(
                !self
                    .staging
                    .as_ref()
                    .ok_or_else(|| CoreError::InvalidContract(
                        "test output has no bound staging path".into()
                    ))?
                    .exists()
            );
            check!(self.target.is_file());
            check_eq!(
                writer
                    .metadata()
                    .map_err(|e| CoreError::Storage(e.to_string()))?
                    .len(),
                actual_encoded_bytes
            );
            drop(writer);
            self.record(Event::Committed(actual_encoded_bytes, 1))
        }

        fn rename_started(&mut self, target: &Path) -> Result<(), CoreError> {
            check_ne!(target, self.target);
            self.record(Event::RenameStarted)?;
            if self.rename_refuse {
                return Err(CoreError::Typed {
                    code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                    message: "fixed rename refusal".into(),
                });
            }
            self.pending_target = Some(target.to_path_buf());
            Ok(())
        }

        fn move_output(&mut self, target: &Path) -> Result<(), CoreError> {
            self.rename_started(target)?;
            fs::rename(&self.target, target)
                .map_err(|error| fs_err("test move output", target, &error))
        }

        fn renamed_and_synced(&mut self) -> Result<(), CoreError> {
            if std::mem::take(&mut self.rename_finish_error) {
                return Err(CoreError::Storage("fixed rename settlement error".into()));
            }
            self.target = self
                .pending_target
                .take()
                .ok_or_else(|| CoreError::InvalidContract("test rename target missing".into()))?;
            self.record(Event::Renamed)
        }

        fn delete_file_and_sync(&mut self) -> Result<(), CoreError> {
            fs::remove_file(&self.target)
                .map_err(|error| fs_err("test unlink", &self.target, &error))?;
            let parent = self
                .target
                .parent()
                .ok_or_else(|| CoreError::InvalidContract("test parent missing".into()))?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| fs_err("test sync", parent, &error))?;
            self.record_absent()
        }

        fn confirmed_uncreated_absent_and_synced(&mut self) -> Result<(), CoreError> {
            self.record_absent()
        }

        fn confirmed_safe_absent_after_writer_close(
            &mut self,
            writer: File,
        ) -> Result<(), CoreError> {
            drop(writer);
            self.record_absent()
        }
    }

    struct TestAdmission {
        events: Arc<Mutex<Vec<Event>>>,
        target: PathBuf,
        refuse: bool,
        rename_refuse: bool,
        rename_finish_error: bool,
    }

    impl PublicationResourceAdmissionPort for TestAdmission {
        fn try_acquire_control(
            &self,
            _batch: &SearchCorpusIngestBatch,
            _budget: &RequestBudgetV1,
        ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
            Err(CoreError::InvalidContract(
                "test does not acquire control".into(),
            ))
        }

        fn try_acquire(
            &self,
            _request: &PublicationAdmissionRequest<'_>,
            _budget: &RequestBudgetV1,
        ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
            Ok(Box::new(TestParent {
                events: Arc::clone(&self.events),
                target: self.target.clone(),
                refuse: self.refuse,
                rename_refuse: self.rename_refuse,
                rename_finish_error: self.rename_finish_error,
            }))
        }
    }

    fn batch() -> Result<SearchCorpusIngestBatch, CoreError> {
        Ok(SearchCorpusIngestBatch {
            source_event: SourcePublicationEvent {
                stream_id: "sidecar-stream".into(),
                event_id: "sidecar-event".into(),
                expected_base_event_id: None,
                payload_sha256: [0; 32],
            },
            repo_id: RepoId::new("repo")
                .map_err(|error| CoreError::InvalidContract(format!("fixture repo: {error}")))?,
            revision_id: RevisionId::new("revision").map_err(|error| {
                CoreError::InvalidContract(format!("fixture revision: {error}"))
            })?,
            generation: ManifestGeneration::new(1),
            base_generation: None,
            manifest_digest: "sidecar-manifest".into(),
            batch_digest: "0".repeat(64),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: true,
        })
    }

    fn admitted_owner(
        admission: &TestAdmission,
        budget: &RequestBudgetV1,
    ) -> Result<PublicationValidationOwner, CoreError> {
        let batch = batch()?;
        let request = PublicationAdmissionRequest {
            batch: &batch,
            footprint: IngestBatchFootprint {
                carried_records: 0,
                embedded_records: 0,
                source_bytes: 0,
                text_bytes: 0,
                vector_bytes: 0,
            },
            work: PublicationBuildWork::Materialize {
                lexical: false,
                semantic: true,
            },
        };
        let mut owner = PublicationValidationOwner::default();
        owner.admit_publication_resources(&request, budget, admission)?;
        Ok(owner)
    }

    #[test]
    fn refusal_precedes_creation_and_preserves_existing_target() -> TestResult {
        let directory = tempfile::tempdir()?;
        let target = directory.path().join("seal");
        fs::write(&target, b"old")?;
        let events = Arc::new(Mutex::new(Vec::new()));
        let admission = TestAdmission {
            events: Arc::clone(&events),
            target: target.clone(),
            refuse: true,
            rename_refuse: false,
            rename_finish_error: false,
        };
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let owner = admitted_owner(&admission, &budget)?;
        check!(matches!(
            write_atomic(
                &target,
                b"replacement",
                "fixed refusal",
                Some(owner.output_admission()),
            ),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        check_eq!(fs::read(&target)?, b"old");
        check_eq!(fs::read_dir(directory.path())?.count(), 1);
        check_eq!(events_snapshot(&events)?, [Event::Reserved(11, 1)]);
        Ok(())
    }

    #[test]
    fn real_rename_refusal_cleans_owned_temp_and_preserves_target_directory() -> TestResult {
        let directory = tempfile::tempdir()?;
        let target = directory.path().join("seal");
        fs::create_dir(&target)?;
        fs::write(target.join("keep"), b"old")?;
        let events = Arc::new(Mutex::new(Vec::new()));
        let admission = TestAdmission {
            events: Arc::clone(&events),
            target: target.clone(),
            refuse: false,
            rename_refuse: false,
            rename_finish_error: false,
        };
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let owner = admitted_owner(&admission, &budget)?;

        let failed = write_atomic(
            &target,
            b"replacement",
            "real rename refusal",
            Some(owner.output_admission()),
        );
        check!(matches!(
            failed,
            Err(CoreError::StorageIo {
                kind: std::io::ErrorKind::IsADirectory,
                raw_os_error: Some(21), // EISDIR on the supported macOS/Linux targets.
                ..
            })
        ));
        check_eq!(fs::read(target.join("keep"))?, b"old");
        check_eq!(fs::read_dir(directory.path())?.count(), 1);
        check_eq!(
            events_snapshot(&events)?,
            [
                Event::Reserved(11, 1),
                Event::Bound,
                Event::Started,
                Event::Absent,
            ]
        );
        Ok(())
    }

    #[test]
    fn successful_replacement_and_empty_payload_account_exact_bytes() -> TestResult {
        let directory = tempfile::tempdir()?;
        let target = directory.path().join("seal");
        fs::write(&target, b"old")?;
        let events = Arc::new(Mutex::new(Vec::new()));
        let admission = TestAdmission {
            events: Arc::clone(&events),
            target: target.clone(),
            refuse: false,
            rename_refuse: false,
            rename_finish_error: false,
        };
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let owner = admitted_owner(&admission, &budget)?;

        drop(write_atomic(
            &target,
            b"new",
            "fixed successful replacement",
            Some(owner.output_admission()),
        )?);
        check_eq!(fs::read(&target)?, b"new");
        check_eq!(fs::read_dir(directory.path())?.count(), 1);
        check_eq!(
            events_snapshot(&events)?,
            [
                Event::Reserved(3, 1),
                Event::Bound,
                Event::Started,
                Event::Committed(3, 1),
            ]
        );

        drop(write_atomic(
            &target,
            b"",
            "fixed empty replacement",
            Some(owner.output_admission()),
        )?);
        check!(fs::read(&target)?.is_empty());
        check_eq!(fs::read_dir(directory.path())?.count(), 1);
        check_eq!(
            events_snapshot(&events)?,
            [
                Event::Reserved(3, 1),
                Event::Bound,
                Event::Started,
                Event::Committed(3, 1),
                Event::Reserved(1, 1),
                Event::Bound,
                Event::Started,
                Event::Committed(0, 1),
            ]
        );
        Ok(())
    }

    #[test]
    fn rename_admission_refusal_does_not_move_or_replace_either_file() -> TestResult {
        let directory = tempfile::tempdir()?;
        let staged = directory.path().join("staged-contract");
        let target = directory.path().join("contract");
        fs::write(&target, b"old")?;
        let events = Arc::new(Mutex::new(Vec::new()));
        let admission = TestAdmission {
            events: Arc::clone(&events),
            target: staged.clone(),
            refuse: false,
            rename_refuse: true,
            rename_finish_error: false,
        };
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let owner = admitted_owner(&admission, &budget)?;
        let mut output = write_atomic(
            &staged,
            b"new",
            "staged contract",
            Some(owner.output_admission()),
        )?;
        let identity = fs::metadata(&staged)?;
        check!(matches!(
            output.rename_to(&target, "promote contract"),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        check_eq!(output.path(), staged);
        check_eq!(fs::read(&target)?, b"old");
        check_eq!(fs::read(&staged)?, b"new");
        check_eq!(fs::metadata(&staged)?.ino(), identity.ino());
        output.remove_and_sync("remove staged contract")?;
        check!(!staged.exists());
        check_eq!(fs::read(&target)?, b"old");
        check_eq!(
            events_snapshot(&events)?,
            [
                Event::Reserved(3, 1),
                Event::Bound,
                Event::Started,
                Event::Committed(3, 1),
                Event::RenameStarted,
                Event::Absent,
            ]
        );
        Ok(())
    }

    #[test]
    fn post_rename_settlement_error_retries_only_the_original_move() -> TestResult {
        let directory = tempfile::tempdir()?;
        let staged = directory.path().join("staged-contract");
        let target = directory.path().join("contract");
        fs::write(&target, b"old")?;
        let events = Arc::new(Mutex::new(Vec::new()));
        let admission = TestAdmission {
            events: Arc::clone(&events),
            target: staged.clone(),
            refuse: false,
            rename_refuse: false,
            rename_finish_error: true,
        };
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let owner = admitted_owner(&admission, &budget)?;
        let mut output = write_atomic(
            &staged,
            b"new",
            "staged contract",
            Some(owner.output_admission()),
        )?;
        let identity = fs::metadata(&staged)?;
        check!(matches!(
            output.rename_to(&target, "promote contract"),
            Err(CoreError::Storage(message)) if message == "fixed rename settlement error"
        ));
        check_eq!(output.path(), target);
        check!(!staged.exists());
        check_eq!(fs::read(&target)?, b"new");
        let observed = fs::metadata(&target)?;
        check_eq!(
            (observed.dev(), observed.ino()),
            (identity.dev(), identity.ino())
        );
        let foreign = directory.path().join("foreign");
        check!(matches!(
            output.rename_to(&foreign, "change pending target"),
            Err(CoreError::InvalidContract(_))
        ));
        check!(!foreign.exists());
        output.rename_to(&target, "settle contract promotion")?;
        check_eq!(output.path(), target);
        check_eq!(fs::metadata(&target)?.ino(), identity.ino());
        drop(output);
        check_eq!(
            events_snapshot(&events)?,
            [
                Event::Reserved(3, 1),
                Event::Bound,
                Event::Started,
                Event::Committed(3, 1),
                Event::RenameStarted,
                Event::Renamed,
            ]
        );
        Ok(())
    }

    #[test]
    fn cross_parent_rename_and_remove_preserve_one_inode_custody() -> TestResult {
        let directory = tempfile::tempdir()?;
        let source_parent = directory.path().join("staging");
        let target_parent = directory.path().join("final");
        fs::create_dir(&source_parent)?;
        fs::create_dir(&target_parent)?;
        let staged = source_parent.join("contract");
        let target = target_parent.join("contract");
        let events = Arc::new(Mutex::new(Vec::new()));
        let admission = TestAdmission {
            events: Arc::clone(&events),
            target: staged.clone(),
            refuse: false,
            rename_refuse: false,
            rename_finish_error: false,
        };
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let owner = admitted_owner(&admission, &budget)?;
        let mut output = write_atomic(
            &staged,
            b"new",
            "staged contract",
            Some(owner.output_admission()),
        )?;
        let identity = fs::metadata(&staged)?;
        output.rename_to(&target, "promote contract")?;
        check_eq!(output.path(), target);
        check!(!staged.exists());
        check_eq!(fs::metadata(&target)?.ino(), identity.ino());
        output.remove_and_sync("remove committed contract")?;
        check!(!target.exists());
        check_eq!(
            events_snapshot(&events)?,
            [
                Event::Reserved(3, 1),
                Event::Bound,
                Event::Started,
                Event::Committed(3, 1),
                Event::RenameStarted,
                Event::Renamed,
                Event::Absent,
            ]
        );
        Ok(())
    }
}
