//! Pinned object pool, explicit dependency graph, and durable attempt adoption.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use quanta_index_core::publication_admission::{
    PublicationControlFileMutation, PublicationControlOutputAdmission,
    PublicationObjectLifetimePort, PublicationOutputClass, PublicationOutputPermit,
    PublicationOutputReservation, PublicationWorkOutputAdmission, validate_control_object_lifetime,
    validate_object_lifetime,
};
use quanta_index_core::{
    CoreError, DiagnosticAdoptionFlushReasonV1, DiagnosticObjectKindV1, RequestBudgetV1,
};
use rustix::fs::{AtFlags, Mode, OFlags, fsync, mkdirat, open, openat, renameat, unlinkat};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

use super::codec::{BlobRef, ObjectKind, ObjectRef, PageRef};

mod control_reader;
mod gc_lifetime;
mod hard_scan_checkpoint;
mod object_reader;
mod pool_accounting;

use control_reader::ControlReader;
pub(crate) use gc_lifetime::GcDeleteCustody;
#[cfg(test)]
pub(crate) use gc_lifetime::tests::fake_gc_delete_ticket;
use object_reader::ObjectReader;

pub(crate) use hard_scan_checkpoint::{
    Authority, CandidateWitness, CheckpointPayload, EligibilityWitness, IndependentWindowOracle,
    IneligibleReason, ScanLane, WindowWitness,
};

const POOL_DIR: &str = "lexical-object-pool";
const OBJECTS_DIR: &str = "objects";
const ATTEMPTS_DIR: &str = "attempts";
const GATE_FILE: &str = ".lifecycle-gate";
const EPOCH_FILE: &str = ".lifecycle-epoch";
const EPOCH_MAGIC: &[u8; 8] = b"QLEPOCH1";
const JOURNAL_HEAD_MAGIC: &[u8; 8] = b"QLAHEAD1";
const RETIREMENT_MAGIC: &[u8; 8] = b"QLRETIR1";
const OBJECT_MAGIC: &[u8; 8] = b"QLSTOR01";
const OBJECT_FORMAT: u16 = 1;
const MAX_OBJECT_PAYLOAD: usize = 64 * 1024;
const MAX_OBJECT_BYTES: usize = 96 * 1024;
const MAX_OBJECT_DEPS: usize = 128;
const REQUEST_OBJECT_CACHE_BYTES: usize = 4 * 1024 * 1024;
// Covers both ordered-tree nodes, their duplicated keys and allocator metadata.
const OBJECT_CACHE_ENTRY_OVERHEAD: usize = 256;
const BLOCK_BYTES: usize = 32 * 1024;
const BLOCK_BYTES_U64: u64 = 32 * 1024;
const NODE_FANOUT: usize = 64;
const MAX_ATTEMPT_BYTES: usize = 8 * 1024 * 1024;
const JOURNAL_ENTRY_BYTES: u64 = 73;
const MAX_ATTEMPTS: usize = 4096;
const MAX_ATTEMPT_CONTROL_ENTRIES: usize = MAX_ATTEMPTS
    .saturating_mul(5)
    .saturating_add(MAX_TEMP_CLEANUP_BATCH);
pub(crate) const MAX_CLOSURE_REFS: usize = 4_000_000;
const MAX_GC_BATCH: usize = 64;
const MAX_TEMP_CLEANUP_BATCH: usize = 64;
// A group uses one AttemptBatch object's dependency list. Keep both writer
// and pending-head recovery inside the existing object codec's bound.
const MAX_ADOPTION_BATCH_REFS: usize = MAX_OBJECT_DEPS;
const MAX_ADOPTION_BATCH_BYTES: u64 = 2 * 1024 * 1024;
// Object header (21) + 128 references (41 each) + fixed batch payload (82).
// The independent encoding golden below checks this reservation.
const MAX_ADOPTION_BATCH_MANIFEST_BYTES: u64 = 5_351;
const MAX_ADOPTION_BATCH_DATA_BYTES: u64 =
    MAX_ADOPTION_BATCH_BYTES.saturating_sub(MAX_ADOPTION_BATCH_MANIFEST_BYTES);
const ADOPTION_BATCH_MAGIC: &[u8; 8] = b"QLABAT01";

fn diagnostic_object_kind(kind: ObjectKind) -> DiagnosticObjectKindV1 {
    match kind {
        ObjectKind::FileBody => DiagnosticObjectKindV1::FileBody,
        ObjectKind::FilePostings => DiagnosticObjectKindV1::FilePostings,
        ObjectKind::NativeBody => DiagnosticObjectKindV1::NativeBody,
        ObjectKind::NativeTrigrams => DiagnosticObjectKindV1::NativeTrigrams,
        ObjectKind::NativePositions => DiagnosticObjectKindV1::NativePositions,
        ObjectKind::Segment => DiagnosticObjectKindV1::Segment,
        ObjectKind::Record | ObjectKind::HardScanAnchor => DiagnosticObjectKindV1::Record,
        ObjectKind::MapPage => DiagnosticObjectKindV1::MapPage,
        ObjectKind::BlobBlock => DiagnosticObjectKindV1::BlobBlock,
        ObjectKind::BlobNode => DiagnosticObjectKindV1::BlobNode,
        ObjectKind::AttemptBatch => DiagnosticObjectKindV1::AttemptBatch,
    }
}

#[cfg(test)]
mod admitted_pool_output_tests {
    use std::fs::{File, OpenOptions};
    use std::os::fd::{AsRawFd as _, RawFd};
    use std::os::unix::fs::MetadataExt as _;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use quanta_index_contract::{
        BatchIngestMode, ManifestGeneration, RepoId, RevisionId, SearchCorpusIngestBatch,
        SourcePublicationEvent,
    };
    use quanta_index_core::publication_admission::{
        PublicationAdmissionRequest, PublicationBuildWork, PublicationControlFileMutation,
        PublicationControlFileUpdatePort, PublicationControlOpenTicket,
        PublicationControlOutputAdmission, PublicationOutputClass, PublicationOutputPermit,
        PublicationOutputReservation, PublicationResourceAdmissionPort, PublicationResourcePermit,
        PublicationWorkOutputAdmission,
    };
    use quanta_index_core::publication_capacity::PublicationAllowance;
    use quanta_index_core::{
        CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, IngestBatchFootprint,
        PublicationValidationOwner, RequestBudgetV1,
    };

    use super::{
        AttemptBinding, EPOCH_FILE, GATE_FILE, OBJECTS_DIR, ObjectKind, ObjectStore, POOL_DIR,
        hex_digest,
    };

    const GOLD: &[u8; 24] = b"QLSTOR01\x00\x01\x07\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03abc";

    #[derive(Default)]
    struct Calls {
        available: u64,
        events: Vec<&'static str>,
        race_gate: bool,
        winner_inode: Option<u64>,
    }

    struct Admission(Arc<Mutex<Calls>>);
    struct Work(Arc<Mutex<Calls>>);
    struct ControlAdmission {
        calls: Arc<Mutex<Calls>>,
        updates: Option<Arc<dyn PublicationControlFileUpdatePort>>,
    }
    struct Control {
        calls: Arc<Mutex<Calls>>,
        updates: Option<Arc<dyn PublicationControlFileUpdatePort>>,
    }
    struct Output {
        calls: Arc<Mutex<Calls>>,
        staging: Option<PathBuf>,
        target: Option<PathBuf>,
        original_fd: Option<RawFd>,
    }

    struct ControlOutput {
        calls: Arc<Mutex<Calls>>,
        maximum: u64,
        staging: Option<PathBuf>,
        target: Option<PathBuf>,
        original_fd: Option<RawFd>,
        staged_bytes: Option<Vec<u8>>,
    }

    fn unexpected() -> CoreError {
        CoreError::InvalidContract("unexpected test output transition".into())
    }

    fn check(condition: bool, reason: &str) -> Result<(), CoreError> {
        if condition {
            Ok(())
        } else {
            Err(CoreError::InvalidContract(format!(
                "test output witness differs: {reason}"
            )))
        }
    }

    fn fixture_io(context: &str, error: impl std::fmt::Display) -> CoreError {
        CoreError::Storage(format!("test output {context}: {error}"))
    }

    fn absent(path: &Path) -> Result<bool, CoreError> {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Ok(_) => Ok(false),
            Err(error) => Err(fixture_io("inspect pathname", error)),
        }
    }

    impl Output {
        fn staging(&self) -> Result<&Path, CoreError> {
            self.staging.as_deref().ok_or_else(unexpected)
        }

        fn target(&self) -> Result<&Path, CoreError> {
            self.target.as_deref().ok_or_else(unexpected)
        }

        fn record(&self, event: &'static str) -> Result<(), CoreError> {
            self.calls
                .lock()
                .map_err(|error| fixture_io("lock calls", error))?
                .events
                .push(event);
            Ok(())
        }
    }

    impl PublicationResourceAdmissionPort for Admission {
        fn try_acquire_control(
            &self,
            _batch: &SearchCorpusIngestBatch,
            _budget: &RequestBudgetV1,
        ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
            Err(unexpected())
        }

        fn try_acquire(
            &self,
            _request: &PublicationAdmissionRequest<'_>,
            _budget: &RequestBudgetV1,
        ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
            Ok(Box::new(Work(Arc::clone(&self.0))))
        }
    }

    impl PublicationResourceAdmissionPort for ControlAdmission {
        fn try_acquire_control(
            &self,
            _batch: &SearchCorpusIngestBatch,
            _budget: &RequestBudgetV1,
        ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
            Ok(Box::new(Control {
                calls: Arc::clone(&self.calls),
                updates: self.updates.clone(),
            }))
        }

        fn try_acquire(
            &self,
            _request: &PublicationAdmissionRequest<'_>,
            _budget: &RequestBudgetV1,
        ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
            Err(unexpected())
        }
    }

    impl PublicationResourcePermit for Control {
        fn held_file_allowance(&self) -> Result<Option<Arc<PublicationAllowance>>, CoreError> {
            Ok(None) // Unqualified transition fixture; Runtime proves the physical ledger.
        }

        fn reserve_output(
            &self,
            request: PublicationOutputReservation,
        ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
            check(
                request.class() == PublicationOutputClass::Control,
                "Control class",
            )?;
            check(request.max_files() == 1, "one Control file")?;
            let mut calls = self
                .calls
                .lock()
                .map_err(|error| fixture_io("lock Control calls", error))?;
            if calls.available < request.max_encoded_bytes() {
                calls.events.push("control refused");
                return Err(CoreError::Typed {
                    code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                    message: "fixed Control output allowance exhausted".into(),
                });
            }
            calls.available = calls
                .available
                .checked_sub(request.max_encoded_bytes())
                .ok_or_else(unexpected)?;
            calls.events.push("control reserved");
            drop(calls);
            Ok(Box::new(ControlOutput {
                calls: Arc::clone(&self.calls),
                maximum: request.max_encoded_bytes(),
                staging: None,
                target: None,
                original_fd: None,
                staged_bytes: None,
            }))
        }

        fn control_file_updates(&self) -> Option<Arc<dyn PublicationControlFileUpdatePort>> {
            self.updates.clone()
        }
    }

    impl PublicationResourcePermit for Work {
        fn held_file_allowance(&self) -> Result<Option<Arc<PublicationAllowance>>, CoreError> {
            Ok(None) // Test-only event recorder; real ledger is covered by Runtime.
        }

        fn reserve_output(
            &self,
            request: PublicationOutputReservation,
        ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
            check(request.class() == PublicationOutputClass::Lexical, "class")?;
            check(request.max_encoded_bytes() == 24, "encoded bound")?;
            check(request.max_files() == 1, "file bound")?;
            let mut calls = self
                .0
                .lock()
                .map_err(|error| fixture_io("lock calls", error))?;
            if calls.available < 24 {
                calls.events.push("refused");
                return Err(CoreError::Typed {
                    code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                    message: "fixed lexical output allowance exhausted".into(),
                });
            }
            calls.available = calls.available.checked_sub(24).ok_or_else(unexpected)?;
            calls.events.push("reserved");
            drop(calls);
            Ok(Box::new(Output {
                calls: Arc::clone(&self.0),
                staging: None,
                target: None,
                original_fd: None,
            }))
        }
    }

    impl PublicationOutputPermit for Output {
        fn cancel_rejected_created(&mut self, writer: std::fs::File) -> Result<(), CoreError> {
            self.retire_writer_preserving_pending(writer)?;
            Err(CoreError::InvalidContract(
                "unqualified fixture cannot prove rejected birth cancellation".into(),
            ))
        }
        fn bind_output(&mut self, staging: &Path, target: &Path) -> Result<(), CoreError> {
            check(
                staging
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".tmp-")),
                "staging name",
            )?;
            check(target.parent() == staging.parent(), "shared parent")?;
            self.staging = Some(staging.to_owned());
            self.target = Some(target.to_owned());
            self.record("bound")
        }

        fn creation_started(&mut self) -> Result<(), CoreError> {
            check(absent(self.staging()?)?, "staging absent before start")?;
            self.record("started")
        }

        fn created(&mut self, writer: &File, staging: &Path) -> Result<(), CoreError> {
            check(self.staging()? == staging, "created staging identity")?;
            let width = writer
                .metadata()
                .map_err(|error| fixture_io("stat created writer", error))?
                .len();
            check(width == 0, "created writer starts empty")?;
            self.original_fd = Some(writer.as_raw_fd());
            self.record("created")
        }

        fn before_replace(&mut self) -> Result<(), CoreError> {
            let bytes = std::fs::read(self.staging()?)
                .map_err(|error| fixture_io("read staged bytes", error))?;
            check(bytes.as_slice() == GOLD, "staged fixed bytes")?;
            check(absent(self.target()?)?, "target absent before rename")?;
            self.record("before")
        }

        fn committed(&mut self, writer: File, actual_encoded_bytes: u64) -> Result<(), CoreError> {
            check(
                self.original_fd == Some(writer.as_raw_fd()),
                "original writer FD",
            )?;
            check(actual_encoded_bytes == 24, "committed encoded width")?;
            let width = writer
                .metadata()
                .map_err(|error| fixture_io("stat committed writer", error))?
                .len();
            check(width == 24, "committed writer width")?;
            let bytes = std::fs::read(self.target()?)
                .map_err(|error| fixture_io("read canonical bytes", error))?;
            check(bytes.as_slice() == GOLD, "canonical fixed bytes")?;
            check(absent(self.staging()?)?, "staging absent after rename")?;
            drop(writer);
            self.record("committed")
        }

        fn retire_writer_preserving_pending(&mut self, writer: File) -> Result<(), CoreError> {
            check(
                self.original_fd == Some(writer.as_raw_fd()),
                "retired original FD",
            )?;
            drop(writer);
            self.record("retired pending")
        }

        fn rename_started(&mut self, _target: &Path) -> Result<(), CoreError> {
            Err(unexpected())
        }

        fn renamed_and_synced(&mut self) -> Result<(), CoreError> {
            Err(unexpected())
        }

        fn delete_file_and_sync(&mut self) -> Result<(), CoreError> {
            let target = self.target()?.to_path_buf();
            std::fs::remove_file(&target)
                .map_err(|error| fixture_io("unlink committed output", error))?;
            let parent = target.parent().ok_or_else(unexpected)?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| fixture_io("sync output parent", error))?;
            self.record("deleted")
        }

        fn confirmed_uncreated_absent_and_synced(&mut self) -> Result<(), CoreError> {
            Err(unexpected())
        }

        fn confirmed_safe_absent_after_writer_close(
            &mut self,
            writer: File,
        ) -> Result<(), CoreError> {
            check(
                self.original_fd == Some(writer.as_raw_fd()),
                "absent original FD",
            )?;
            drop(writer);
            self.record("absent")
        }
    }

    impl ControlOutput {
        fn staging(&self) -> Result<&Path, CoreError> {
            self.staging.as_deref().ok_or_else(unexpected)
        }

        fn target(&self) -> Result<&Path, CoreError> {
            self.target.as_deref().ok_or_else(unexpected)
        }

        fn record(&self, event: &'static str) -> Result<(), CoreError> {
            self.calls
                .lock()
                .map_err(|error| fixture_io("lock Control calls", error))?
                .events
                .push(event);
            Ok(())
        }
    }

    impl PublicationOutputPermit for ControlOutput {
        fn cancel_rejected_created(&mut self, writer: std::fs::File) -> Result<(), CoreError> {
            self.retire_writer_preserving_pending(writer)?;
            Err(CoreError::InvalidContract(
                "unqualified fixture cannot prove rejected birth cancellation".into(),
            ))
        }
        fn bind_output(&mut self, staging: &Path, target: &Path) -> Result<(), CoreError> {
            check(staging.parent() == target.parent(), "Control output parent")?;
            match target.file_name().and_then(|name| name.to_str()) {
                Some(GATE_FILE) => check(self.maximum == 1, "gate reservation width")?,
                Some(EPOCH_FILE) => check(self.maximum == 48, "epoch reservation width")?,
                _ => return Err(unexpected()),
            }
            self.staging = Some(staging.to_owned());
            self.target = Some(target.to_owned());
            self.record("control bound")
        }

        fn creation_started(&mut self) -> Result<(), CoreError> {
            check(absent(self.staging()?)?, "Control staging absent")?;
            self.record("control started")
        }

        fn created(&mut self, writer: &File, staging: &Path) -> Result<(), CoreError> {
            check(self.staging()? == staging, "Control staging identity")?;
            check(
                writer
                    .metadata()
                    .map_err(|error| fixture_io("stat Control writer", error))?
                    .len()
                    == 0,
                "Control writer starts empty",
            )?;
            self.original_fd = Some(writer.as_raw_fd());
            self.record("control created")
        }

        fn before_replace(&mut self) -> Result<(), CoreError> {
            let bytes = std::fs::read(self.staging()?)
                .map_err(|error| fixture_io("read staged Control", error))?;
            check(
                u64::try_from(bytes.len()).map_err(|error| fixture_io("Control width", error))?
                    <= self.maximum,
                "Control bound",
            )?;
            self.staged_bytes = Some(bytes);
            let target = self.target()?.to_path_buf();
            let inject_gate = {
                let mut calls = self
                    .calls
                    .lock()
                    .map_err(|error| fixture_io("lock Control calls", error))?;
                let inject =
                    calls.race_gate && target.file_name().is_some_and(|name| name == GATE_FILE);
                calls.race_gate = false;
                inject
            };
            if inject_gate {
                let winner = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .map_err(|error| fixture_io("create competing lifecycle gate", error))?;
                winner
                    .sync_all()
                    .map_err(|error| fixture_io("sync competing lifecycle gate", error))?;
                let inode = winner
                    .metadata()
                    .map_err(|error| fixture_io("stat competing lifecycle gate", error))?
                    .ino();
                File::open(target.parent().ok_or_else(unexpected)?)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| fixture_io("sync competing gate parent", error))?;
                self.calls
                    .lock()
                    .map_err(|error| fixture_io("lock Control calls", error))?
                    .winner_inode = Some(inode);
            }
            self.record("control before")
        }

        fn committed(&mut self, writer: File, actual_encoded_bytes: u64) -> Result<(), CoreError> {
            check(
                self.original_fd == Some(writer.as_raw_fd()),
                "Control original writer FD",
            )?;
            let bytes = std::fs::read(self.target()?)
                .map_err(|error| fixture_io("read committed Control", error))?;
            check(
                self.staged_bytes.as_deref() == Some(bytes.as_slice()),
                "Control committed bytes",
            )?;
            check(
                actual_encoded_bytes
                    == u64::try_from(bytes.len())
                        .map_err(|error| fixture_io("committed Control width", error))?,
                "Control committed width",
            )?;
            check(absent(self.staging()?)?, "Control temporary retired")?;
            drop(writer);
            self.record("control committed")
        }

        fn retire_writer_preserving_pending(&mut self, writer: File) -> Result<(), CoreError> {
            check(
                self.original_fd == Some(writer.as_raw_fd()),
                "Control pending original FD",
            )?;
            drop(writer);
            self.record("control pending")
        }

        fn rename_started(&mut self, _target: &Path) -> Result<(), CoreError> {
            Err(unexpected())
        }

        fn renamed_and_synced(&mut self) -> Result<(), CoreError> {
            Err(unexpected())
        }

        fn delete_file_and_sync(&mut self) -> Result<(), CoreError> {
            let target = self.target()?.to_path_buf();
            std::fs::remove_file(&target)
                .map_err(|error| fixture_io("unlink committed Control", error))?;
            File::open(target.parent().ok_or_else(unexpected)?)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| fixture_io("sync Control parent", error))?;
            self.record("control deleted")
        }

        fn confirmed_uncreated_absent_and_synced(&mut self) -> Result<(), CoreError> {
            check(absent(self.staging()?)?, "uncreated Control absent")?;
            self.record("control uncreated")
        }

        fn confirmed_safe_absent_after_writer_close(
            &mut self,
            writer: File,
        ) -> Result<(), CoreError> {
            check(
                self.original_fd == Some(writer.as_raw_fd()),
                "Control failed original FD",
            )?;
            check(absent(self.staging()?)?, "failed Control temporary absent")?;
            drop(writer);
            let mut calls = self
                .calls
                .lock()
                .map_err(|error| fixture_io("lock Control calls", error))?;
            calls.available = calls
                .available
                .checked_add(self.maximum)
                .ok_or_else(unexpected)?;
            drop(calls);
            self.record("control absent")
        }
    }

    const FIXED_ATTEMPT: &str =
        "5855ae566586b5b763e0499337a26c8ff96b88572eebc7343f1971392d528b9b.cbor";

    #[derive(Default)]
    struct DeleteCalls {
        events: Vec<String>,
        refuse: Option<String>,
        require_marker: bool,
        fail_after_marker_unlink: bool,
        pending_marker: Option<PathBuf>,
    }

    struct DeletePort(Arc<Mutex<DeleteCalls>>);

    impl PublicationControlFileUpdatePort for DeletePort {
        fn begin_reader_open(
            &self,
            _path: &Path,
            _control: PublicationControlOutputAdmission,
        ) -> Result<Box<dyn PublicationControlOpenTicket>, CoreError> {
            Err(unexpected())
        }

        fn begin_update(
            &self,
            _path: &Path,
            _writer: File,
            _expected_length: u64,
            _target_end: u64,
            _control: PublicationControlOutputAdmission,
        ) -> Result<Box<dyn PublicationControlFileMutation>, CoreError> {
            Err(unexpected())
        }

        fn reconcile_deletions(&self) -> Result<(), CoreError> {
            let pending = {
                let mut calls = self
                    .0
                    .lock()
                    .map_err(|error| fixture_io("lock deletion calls", error))?;
                calls.events.push("reconcile".into());
                calls.pending_marker.clone()
            };
            if let Some(marker) = pending {
                check(absent(&marker)?, "reconcile absent marker")?;
                File::open(marker.parent().ok_or_else(unexpected)?)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| fixture_io("sync reconciled marker parent", error))?;
                self.0
                    .lock()
                    .map_err(|error| fixture_io("lock deletion calls", error))?
                    .pending_marker = None;
            }
            Ok(())
        }

        fn remove_file_and_sync(
            &self,
            path: &Path,
            _control: PublicationControlOutputAdmission,
        ) -> Result<(), CoreError> {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(unexpected)?;
            let marker = path
                .parent()
                .ok_or_else(unexpected)?
                .join(format!("{FIXED_ATTEMPT}.retired"));
            let is_marker = name == format!("{FIXED_ATTEMPT}.retired");
            let require_marker = self
                .0
                .lock()
                .map_err(|error| fixture_io("lock deletion calls", error))?
                .require_marker;
            if require_marker && !is_marker {
                check(
                    !absent(&marker)?,
                    "retirement marker precedes each deletion",
                )?;
            }
            let (refuse, fail_after) = {
                let mut calls = self
                    .0
                    .lock()
                    .map_err(|error| fixture_io("lock deletion calls", error))?;
                calls.events.push(format!("remove:{name}"));
                let refuse = calls.refuse.as_deref() == Some(name);
                let fail_after = is_marker && calls.fail_after_marker_unlink;
                if fail_after {
                    calls.fail_after_marker_unlink = false;
                }
                drop(calls);
                (refuse, fail_after)
            };
            if refuse {
                return Err(CoreError::Typed {
                    code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                    message: "fixed qualified Control deletion refusal".into(),
                });
            }
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(fixture_io("remove qualified Control", error)),
            }
            File::open(path.parent().ok_or_else(unexpected)?)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| fixture_io("sync qualified Control parent", error))?;
            if fail_after {
                self.0
                    .lock()
                    .map_err(|error| fixture_io("lock deletion calls", error))?
                    .pending_marker = Some(path.to_owned());
                return Err(CoreError::Storage(
                    "fixed ambiguous marker deletion after unlink".into(),
                ));
            }
            Ok(())
        }
    }

    fn fixed_attempt_binding() -> AttemptBinding {
        AttemptBinding {
            root_incarnation: [7; 16],
            repository_sha256: [8; 32],
            revision_sha256: [11; 32],
            source_event_sha256: [9; 32],
            target_generation: 1,
            base_root_sha256: None,
            operations_sha256: [10; 32],
            format: 1,
        }
    }

    fn qualified_deletion_store(
        dir: &tempfile::TempDir,
        deletion_calls: Arc<Mutex<DeleteCalls>>,
    ) -> ObjectStore {
        let mut store = ObjectStore::open(dir.path()).expect("unqualified fixture bootstrap");
        deletion_calls
            .lock()
            .expect("deletion calls")
            .require_marker = true;
        let binding = fixed_attempt_binding();
        assert_eq!(binding.identity_name(), FIXED_ATTEMPT);
        let lease = store.begin_attempt(&binding, &[]).expect("fixed attempt");
        let record = store
            .read_attempt(FIXED_ATTEMPT)
            .expect("fixed attempt read")
            .expect("fixed attempt record");
        store
            .write_retirement_marker(FIXED_ATTEMPT, &record)
            .expect("fixed retirement marker");
        drop(lease);
        let calls = Arc::new(Mutex::new(Calls {
            available: 256,
            ..Calls::default()
        }));
        store.control_output_admission = Some(control_admission_with_updates(
            calls,
            Some(Arc::new(DeletePort(deletion_calls))),
        ));
        store
    }

    fn fixture_batch() -> SearchCorpusIngestBatch {
        SearchCorpusIngestBatch {
            source_event: SourcePublicationEvent {
                stream_id: "fixed-stream".into(),
                event_id: "fixed-event".into(),
                expected_base_event_id: None,
                payload_sha256: [0; 32],
            },
            repo_id: RepoId::new("repo").expect("repo"),
            revision_id: RevisionId::new("revision").expect("revision"),
            generation: ManifestGeneration::new(1),
            base_generation: None,
            manifest_digest: "fixed-manifest".into(),
            batch_digest: "0".repeat(64),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: true,
        }
    }

    fn control_admission(calls: Arc<Mutex<Calls>>) -> PublicationControlOutputAdmission {
        control_admission_with_updates(calls, None)
    }

    fn control_admission_with_updates(
        calls: Arc<Mutex<Calls>>,
        updates: Option<Arc<dyn PublicationControlFileUpdatePort>>,
    ) -> PublicationControlOutputAdmission {
        let batch = fixture_batch();
        let mut owner = PublicationValidationOwner::default();
        owner
            .admit_publication_control(
                &batch,
                &RequestBudgetV1::unbounded(),
                &ControlAdmission { calls, updates },
            )
            .expect("Control admission");
        owner
            .output_admission()
            .owned_control_admission()
            .expect("owned Control")
            .expect("Control present")
    }

    fn work_admission(calls: Arc<Mutex<Calls>>) -> PublicationWorkOutputAdmission {
        let batch = fixture_batch();
        let mut owner = PublicationValidationOwner::default();
        owner
            .admit_publication_resources(
                &PublicationAdmissionRequest {
                    batch: &batch,
                    footprint: IngestBatchFootprint {
                        carried_records: 0,
                        embedded_records: 0,
                        source_bytes: 0,
                        text_bytes: 0,
                        vector_bytes: 0,
                    },
                    work: PublicationBuildWork::Materialize {
                        lexical: true,
                        semantic: false,
                    },
                },
                &RequestBudgetV1::unbounded(),
                &Admission(calls),
            )
            .expect("work admission");
        owner
            .output_admission()
            .owned_work_admission()
            .expect("owned work")
            .expect("work present")
    }

    #[test]
    fn admitted_io_boundary_keeps_typed_cause_and_rejects_lookalike_strings() {
        let primary = CoreError::Typed {
            code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
            message: "fixed Control writer refusal".into(),
        };
        let mapped = super::admitted_io_error("append", &std::io::Error::other(primary));
        assert!(matches!(mapped, CoreError::Typed { code, message }
            if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE && message == "fixed Control writer refusal"));
        let nested = super::admitted_io_error(
            "append",
            &std::io::Error::other(std::io::Error::other(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                message: "nested Control writer refusal".into(),
            })),
        );
        assert!(matches!(nested, CoreError::Typed { code, message }
            if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE && message == "nested Control writer refusal"));
        let lookalike = super::admitted_io_error(
            "append",
            &std::io::Error::other("IngestResourceBudgetExceeded fixed Control writer refusal"),
        );
        assert!(matches!(
            lookalike,
            CoreError::StorageIo {
                kind: std::io::ErrorKind::Other,
                raw_os_error: None,
                ..
            }
        ));
        let enospc = super::admitted_io_error(
            "append",
            &std::io::Error::from_raw_os_error(rustix::io::Errno::NOSPC.raw_os_error()),
        );
        assert!(
            matches!(enospc, CoreError::StorageIo { raw_os_error: Some(code), .. }
            if code == rustix::io::Errno::NOSPC.raw_os_error())
        );
    }

    #[test]
    fn control_gate_and_epoch_are_reserved_before_create_and_replacement() {
        let dir = tempfile::tempdir().expect("state root");
        let calls = Arc::new(Mutex::new(Calls {
            available: 49,
            events: Vec::new(),
            race_gate: false,
            winner_inode: None,
        }));
        let store = ObjectStore::open_observed_with_control(
            dir.path(),
            None,
            Some(control_admission(Arc::clone(&calls))),
            None,
        )
        .expect("admitted bootstrap controls");
        assert_eq!(store.read_epoch().expect("initial epoch"), 0);
        assert_eq!(calls.lock().expect("Control calls").available, 0);

        assert!(matches!(
            store.write_epoch(1),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        assert_eq!(store.read_epoch().expect("unchanged epoch"), 0);
        let pool = dir.path().join(POOL_DIR);
        let temporaries = std::fs::read_dir(&pool)
            .expect("pool inventory")
            .collect::<Result<Vec<_>, _>>()
            .expect("pool entries")
            .into_iter()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.contains(".tmp-"))
            })
            .count();
        assert_eq!(temporaries, 0);
        {
            let mut held = calls.lock().expect("Control calls");
            assert_eq!(held.events.last(), Some(&"control refused"));
            held.available = 48;
        }
        store.write_epoch(1).expect("admitted epoch replacement");
        assert_eq!(store.read_epoch().expect("new epoch"), 1);
        assert_eq!(calls.lock().expect("Control calls").available, 0);
    }

    #[test]
    fn competing_gate_keeps_winner_inode_after_loser_cleans_original_writer() {
        let dir = tempfile::tempdir().expect("state root");
        let calls = Arc::new(Mutex::new(Calls {
            available: 49,
            events: Vec::new(),
            race_gate: true,
            winner_inode: None,
        }));
        let store = ObjectStore::open_observed_with_control(
            dir.path(),
            None,
            Some(control_admission(Arc::clone(&calls))),
            None,
        )
        .expect("competing gate admits existing regular inode");
        let winner_inode = calls.lock().expect("Control calls").winner_inode;
        let gate = dir.path().join(POOL_DIR).join(GATE_FILE);
        assert_eq!(
            gate.metadata().expect("published gate metadata").ino(),
            winner_inode.expect("competitor published gate")
        );
        assert_eq!(store.read_epoch().expect("epoch after gate race"), 0);
        let recorded = calls.lock().expect("Control calls");
        assert_eq!(recorded.available, 1);
        assert!(recorded.events.contains(&"control absent"));
        assert_eq!(recorded.events.last(), Some(&"control committed"));
        drop(recorded);
        let temporaries = std::fs::read_dir(dir.path().join(POOL_DIR))
            .expect("pool entries")
            .collect::<Result<Vec<_>, _>>()
            .expect("pool inventory")
            .into_iter()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.contains(".tmp-"))
            })
            .count();
        assert_eq!(temporaries, 0);
    }

    #[test]
    fn qualified_retirement_refusal_keeps_marker_and_retry_removes_it_last() {
        let dir = tempfile::tempdir().expect("state root");
        let deletion_calls = Arc::new(Mutex::new(DeleteCalls {
            refuse: Some(FIXED_ATTEMPT.into()),
            ..DeleteCalls::default()
        }));
        let store = qualified_deletion_store(&dir, Arc::clone(&deletion_calls));
        let attempts = dir.path().join(POOL_DIR).join("attempts");
        let marker = attempts.join(format!("{FIXED_ATTEMPT}.retired"));
        assert!(matches!(
            store.recover_retirement_locked(FIXED_ATTEMPT),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        for name in [
            FIXED_ATTEMPT.to_owned(),
            format!("{FIXED_ATTEMPT}.log"),
            format!("{FIXED_ATTEMPT}.head"),
            format!("{FIXED_ATTEMPT}.retired"),
        ] {
            assert!(attempts.join(name).is_file());
        }
        assert_eq!(
            deletion_calls.lock().expect("deletion calls").events,
            ["reconcile".to_owned(), format!("remove:{FIXED_ATTEMPT}")]
        );
        {
            let mut calls = deletion_calls.lock().expect("deletion calls");
            calls.refuse = None;
            calls.events.clear();
        }
        store
            .recover_retirement_locked(FIXED_ATTEMPT)
            .expect("qualified retry");
        assert_eq!(
            deletion_calls.lock().expect("deletion calls").events,
            [
                "reconcile".to_owned(),
                format!("remove:{FIXED_ATTEMPT}"),
                format!("remove:{FIXED_ATTEMPT}.log"),
                format!("remove:{FIXED_ATTEMPT}.head"),
                format!("remove:{FIXED_ATTEMPT}.scan"),
                format!("remove:{FIXED_ATTEMPT}.retired"),
            ]
        );
        assert!(absent(&marker).expect("marker absence"));
        for suffix in ["", ".log", ".head", ".scan"] {
            assert!(absent(&attempts.join(format!("{FIXED_ATTEMPT}{suffix}"))).expect("absence"));
        }
    }

    #[test]
    fn qualified_retirement_reconciles_unlinked_marker_before_absent_probe() {
        let dir = tempfile::tempdir().expect("state root");
        let deletion_calls = Arc::new(Mutex::new(DeleteCalls {
            fail_after_marker_unlink: true,
            ..DeleteCalls::default()
        }));
        let store = qualified_deletion_store(&dir, Arc::clone(&deletion_calls));
        let marker = dir
            .path()
            .join(POOL_DIR)
            .join("attempts")
            .join(format!("{FIXED_ATTEMPT}.retired"));
        assert!(store.recover_retirement_locked(FIXED_ATTEMPT).is_err());
        assert!(absent(&marker).expect("marker removed before error"));
        {
            let mut calls = deletion_calls.lock().expect("deletion calls");
            assert_eq!(calls.pending_marker.as_deref(), Some(marker.as_path()));
            calls.events.clear();
        }
        store
            .recover_retirement_locked(FIXED_ATTEMPT)
            .expect("reconcile absent marker");
        let calls = deletion_calls.lock().expect("deletion calls");
        assert!(calls.pending_marker.is_none());
        assert_eq!(calls.events, ["reconcile"]);
        drop(calls);
    }

    #[test]
    fn unowned_cleanup_requires_absent_header_and_qualified_refusal_keeps_journal() {
        let dir = tempfile::tempdir().expect("state root");
        let mut store = ObjectStore::open(dir.path()).expect("unqualified bootstrap");
        let binding = fixed_attempt_binding();
        assert_eq!(binding.identity_name(), FIXED_ATTEMPT);
        let lease = store.begin_attempt(&binding, &[]).expect("fixed attempt");
        drop(lease);
        let deletion_calls = Arc::new(Mutex::new(DeleteCalls {
            refuse: Some(format!("{FIXED_ATTEMPT}.log")),
            ..DeleteCalls::default()
        }));
        let calls = Arc::new(Mutex::new(Calls {
            available: 256,
            ..Calls::default()
        }));
        store.control_output_admission = Some(control_admission_with_updates(
            calls,
            Some(Arc::new(DeletePort(Arc::clone(&deletion_calls)))),
        ));
        let attempts = dir.path().join(POOL_DIR).join("attempts");
        let header = attempts.join(FIXED_ATTEMPT);
        let journal = attempts.join(format!("{FIXED_ATTEMPT}.log"));
        assert!(
            !store
                .cleanup_unowned_attempt_locked(FIXED_ATTEMPT)
                .expect("owned attempt remains")
        );
        assert!(header.is_file());
        assert!(journal.is_file());
        assert_eq!(
            deletion_calls.lock().expect("deletion calls").events,
            ["reconcile"]
        );
        std::fs::remove_file(&header).expect("simulate missing incomplete header");
        File::open(&attempts)
            .and_then(|directory| directory.sync_all())
            .expect("sync missing header");
        deletion_calls
            .lock()
            .expect("deletion calls")
            .events
            .clear();
        assert!(matches!(
            store.cleanup_unowned_attempt_locked(FIXED_ATTEMPT),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        assert!(journal.is_file());
        assert_eq!(
            deletion_calls.lock().expect("deletion calls").events,
            [
                "reconcile".to_owned(),
                format!("remove:{FIXED_ATTEMPT}.log"),
            ]
        );
        {
            let mut calls = deletion_calls.lock().expect("deletion calls");
            calls.refuse = None;
            calls.events.clear();
        }
        assert!(
            store
                .cleanup_unowned_attempt_locked(FIXED_ATTEMPT)
                .expect("qualified orphan retry")
        );
        assert_eq!(
            deletion_calls.lock().expect("deletion calls").events,
            [
                "reconcile".to_owned(),
                format!("remove:{FIXED_ATTEMPT}.log"),
                format!("remove:{FIXED_ATTEMPT}.head"),
                format!("remove:{FIXED_ATTEMPT}.scan"),
            ]
        );
        assert!(absent(&journal).expect("journal absent"));
        assert!(absent(&attempts.join(format!("{FIXED_ATTEMPT}.head"))).expect("head absent"));
    }

    #[test]
    fn exhausted_work_refuses_before_object_temporary_creation() {
        let dir = tempfile::tempdir().expect("state root");
        let calls = Arc::new(Mutex::new(Calls::default()));
        let store = ObjectStore::open(dir.path())
            .expect("pool")
            .with_work_output_admission(Some(work_admission(Arc::clone(&calls))));
        let _gate = store.lock_gate().expect("pool gate");
        assert!(matches!(
            store.put_object_locked(ObjectKind::Record, &[], b"abc"),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        assert_eq!(calls.lock().expect("output calls").events, ["refused"]);
        let objects = dir.path().join(POOL_DIR).join(OBJECTS_DIR);
        let shards = std::fs::read_dir(objects)
            .expect("object shards")
            .collect::<Result<Vec<_>, _>>()
            .expect("shard entries");
        assert_eq!(shards.len(), 1);
        assert_eq!(
            std::fs::read_dir(shards.first().expect("shard").path())
                .expect("shard entries")
                .count(),
            0
        );
    }

    #[test]
    fn verified_digest_reuse_spends_no_second_child_and_keeps_original_fd_order() {
        let dir = tempfile::tempdir().expect("state root");
        let calls = Arc::new(Mutex::new(Calls {
            available: 24,
            events: Vec::new(),
            race_gate: false,
            winner_inode: None,
        }));
        let store = ObjectStore::open(dir.path())
            .expect("pool")
            .with_work_output_admission(Some(work_admission(Arc::clone(&calls))));
        let _gate = store.lock_gate().expect("pool gate");
        let first = store
            .put_object_locked(ObjectKind::Record, &[], b"abc")
            .expect("first object");
        let second = store
            .put_object_locked(ObjectKind::Record, &[], b"abc")
            .expect("verified reuse");
        assert_eq!(first, second);
        assert_eq!(first.bytes, 24);
        let digest = hex_digest(&first.sha256);
        let shard = digest.get(..2).expect("digest shard");
        let canonical = dir
            .path()
            .join(POOL_DIR)
            .join(OBJECTS_DIR)
            .join(shard)
            .join(digest);
        assert_eq!(std::fs::read(canonical).expect("canonical bytes"), GOLD);
        let calls = calls.lock().expect("output calls");
        assert_eq!(calls.available, 0);
        assert_eq!(
            calls.events,
            [
                "reserved",
                "bound",
                "started",
                "created",
                "before",
                "committed"
            ]
        );
        drop(calls);
    }
}

#[derive(Clone, Copy)]
enum AtomicPurpose {
    Other,
    PoolObject,
    AttemptJournal,
    HardScanCheckpoint,
}

#[derive(Clone, Copy)]
enum AtomicSync {
    Full,
    BatchedPoolData,
}

#[derive(Default)]
struct AtomicWriteAdmission {
    pool: Option<Box<dyn quanta_index_core::pool_write_admission::PoolWritePermit>>,
    output: Option<Box<dyn PublicationOutputPermit>>,
}

#[cfg(test)]
fn reach_atomic_cut(
    purpose: AtomicPurpose,
    operation: usize,
    side: crate::publication_faults::Side,
) -> Result<(), CoreError> {
    use crate::publication_faults::Cut;
    let cut = match (purpose, operation) {
        (AtomicPurpose::PoolObject, 0) => Some(Cut::PoolObjectWrite),
        (AtomicPurpose::PoolObject, 1) => Some(Cut::PoolObjectFileSync),
        (AtomicPurpose::PoolObject, 2) => Some(Cut::PoolObjectRename),
        (AtomicPurpose::PoolObject, 3) => Some(Cut::PoolObjectDirectorySync),
        (AtomicPurpose::AttemptJournal, 0) => Some(Cut::AttemptJournalWrite),
        (AtomicPurpose::AttemptJournal, 1) => Some(Cut::AttemptJournalFileSync),
        (AtomicPurpose::AttemptJournal, 2) => Some(Cut::AttemptJournalRename),
        (AtomicPurpose::AttemptJournal, 3) => Some(Cut::AttemptJournalDirectorySync),
        (AtomicPurpose::HardScanCheckpoint, 0) => Some(Cut::HardScanCheckpointWrite),
        (AtomicPurpose::HardScanCheckpoint, 1) => Some(Cut::HardScanCheckpointFileSync),
        (AtomicPurpose::HardScanCheckpoint, 2) => Some(Cut::HardScanCheckpointRename),
        (AtomicPurpose::HardScanCheckpoint, 3) => Some(Cut::HardScanCheckpointDirectorySync),
        (AtomicPurpose::Other, _) => None,
        _ => return Err(invalid("unknown atomic publication cut")),
    };
    if let Some(cut) = cut {
        crate::publication_faults::reach_core(cut, side)?;
    }
    Ok(())
}

pub(super) fn storage(context: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("lexical object store {context}: {error}"))
}

fn with_custody_context(mut primary: CoreError, context: impl std::fmt::Display) -> CoreError {
    let suffix = format!("; physical custody cleanup failed: {context}");
    let message = match &mut primary {
        CoreError::InvalidContract(message)
        | CoreError::Typed { message, .. }
        | CoreError::NotReady(message)
        | CoreError::NotImplemented(message)
        | CoreError::NotFound(message)
        | CoreError::Storage(message)
        | CoreError::StorageIo { message, .. } => message,
    };
    message.push_str(&suffix);
    primary
}

pub(super) fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        message: format!("lexical object store: {reason}"),
    }
}

pub(super) fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidContract(format!("lexical object store: {reason}"))
}

fn dir_flags() -> OFlags {
    OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK
}

fn ensure_dir(
    parent: &File,
    name: &str,
    sync_totals: Option<&crate::causal_profile::SyncTotalsV1>,
) -> Result<File, CoreError> {
    let created = match mkdirat(parent, name, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
        Ok(()) => true,
        Err(rustix::io::Errno::EXIST) => false,
        Err(error) => return Err(storage("create directory", error)),
    };
    let directory = File::from(
        openat(parent, name, dir_flags(), Mode::empty())
            .map_err(|error| storage("open nofollow directory", error))?,
    );
    if created {
        crate::causal_profile::observed_sync_legacy(
            sync_totals,
            quanta_index_core::DiagnosticSyncLabelV1::PoolNewDirectoryFull,
            || directory.sync_all(),
        )
        .map_err(|error| storage("sync new directory", error))?;
        crate::causal_profile::observed_sync_legacy(
            sync_totals,
            quanta_index_core::DiagnosticSyncLabelV1::PoolNewParentFull,
            || parent.sync_all(),
        )
        .map_err(|error| storage("sync new directory parent", error))?;
    }
    Ok(directory)
}

fn hex_digest(digest: &[u8; 32]) -> String {
    let mut text = String::with_capacity(64);
    for byte in digest {
        for nibble in [byte >> 4, byte & 0x0f] {
            let digit = if nibble < 10 {
                b'0'.saturating_add(nibble)
            } else {
                b'a'.saturating_add(nibble.saturating_sub(10))
            };
            text.push(char::from(digit));
        }
    }
    text
}

fn decode_hex_digit(byte: u8) -> Result<u8, CoreError> {
    match byte {
        b'0'..=b'9' => Ok(byte.wrapping_sub(b'0')),
        b'a'..=b'f' => Ok(byte.wrapping_sub(b'a').saturating_add(10)),
        _ => Err(corrupt("object name is not lowercase hexadecimal")),
    }
}

fn atomic_temporary_base(name: &str) -> Option<&str> {
    if name.len() > 128 {
        return None;
    }
    let rest = name.strip_prefix('.')?;
    let (digest, sequence) = rest.split_once(".tmp-")?;
    let (pid, counter) = sequence.split_once('-')?;
    if !pid.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_digit())
        && pid.parse::<u32>().is_ok()
        && !counter.is_empty()
        && counter.bytes().all(|byte| byte.is_ascii_digit())
        && counter.parse::<u64>().is_ok()
    {
        Some(digest)
    } else {
        None
    }
}

fn is_lower_digest_name(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn is_object_temporary_name(name: &str, shard: u8) -> bool {
    let Some(digest) = atomic_temporary_base(name) else {
        return false;
    };
    let Some(prefix) = digest.get(..2) else {
        return false;
    };
    is_lower_digest_name(digest)
        && matches!(u8::from_str_radix(prefix, 16), Ok(observed) if observed == shard)
}

fn is_attempt_temporary_name(name: &str) -> bool {
    let Some(base) = atomic_temporary_base(name) else {
        return false;
    };
    let Some(stem) = base.strip_suffix(".cbor.retired").or_else(|| {
        base.strip_suffix(".cbor.head")
            .or_else(|| base.strip_suffix(".cbor.log"))
            .or_else(|| base.strip_suffix(".cbor.scan"))
            .or_else(|| base.strip_suffix(".cbor"))
    }) else {
        return false;
    };
    is_lower_digest_name(stem)
}

fn is_pool_temporary_name(name: &str) -> bool {
    atomic_temporary_base(name).is_some_and(|base| base == EPOCH_FILE || base == GATE_FILE)
}

fn journal_name(header_name: &str) -> String {
    format!("{header_name}.log")
}

fn journal_head_name(header_name: &str) -> String {
    format!("{header_name}.head")
}

fn retirement_name(header_name: &str) -> String {
    format!("{header_name}.retired")
}

fn journal_seed(header_name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"lexical-attempt-journal-v1");
    hasher.update(header_name.as_bytes());
    hasher.finalize().into()
}

fn checkpoint(budget: Option<&RequestBudgetV1>, stage: &'static str) -> Result<(), CoreError> {
    if let Some(budget) = budget {
        budget.checkpoint(stage)?;
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AttemptBinding {
    pub(crate) root_incarnation: [u8; 16],
    pub(crate) repository_sha256: [u8; 32],
    pub(crate) revision_sha256: [u8; 32],
    pub(crate) source_event_sha256: [u8; 32],
    pub(crate) target_generation: u64,
    pub(crate) base_root_sha256: Option<[u8; 32]>,
    pub(crate) operations_sha256: [u8; 32],
    pub(crate) format: u16,
}

impl Serialize for AttemptBinding {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            self.root_incarnation,
            self.repository_sha256,
            self.revision_sha256,
            self.source_event_sha256,
            self.target_generation,
            self.base_root_sha256,
            self.operations_sha256,
            self.format,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AttemptBinding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (
            root_incarnation,
            repository_sha256,
            revision_sha256,
            source_event_sha256,
            target_generation,
            base_root_sha256,
            operations_sha256,
            format,
        ) = <(
            [u8; 16],
            [u8; 32],
            [u8; 32],
            [u8; 32],
            u64,
            Option<[u8; 32]>,
            [u8; 32],
            u16,
        )>::deserialize(deserializer)?;
        Ok(Self {
            root_incarnation,
            repository_sha256,
            revision_sha256,
            source_event_sha256,
            target_generation,
            base_root_sha256,
            operations_sha256,
            format,
        })
    }
}

impl AttemptBinding {
    fn validate(&self) -> Result<(), CoreError> {
        if self.root_incarnation == [0; 16]
            || self.repository_sha256 == [0; 32]
            || self.revision_sha256 == [0; 32]
            || self.target_generation == 0
            || self.format == 0
        {
            return Err(invalid(
                "attempt has zero incarnation, repository/revision digest, target generation or format",
            ));
        }
        Ok(())
    }

    fn identity_name(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"lexical-attempt-identity-v1");
        hasher.update(self.root_incarnation);
        hasher.update(self.repository_sha256);
        hasher.update(self.revision_sha256);
        hasher.update(self.source_event_sha256);
        hasher.update(self.target_generation.to_be_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        format!("{}.cbor", hex_digest(&digest))
    }
}

#[derive(Clone, Debug)]
struct AttemptRecord(AttemptBinding, Vec<ObjectRef>);

impl Serialize for AttemptRecord {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.0, &self.1).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AttemptRecord {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (binding, inherited) = <(AttemptBinding, Vec<ObjectRef>)>::deserialize(deserializer)?;
        Ok(Self(binding, inherited))
    }
}

struct JournalHead {
    committed_length: u64,
    committed_chain: [u8; 32],
    pending: Option<ObjectRef>,
    expected_chain: [u8; 32],
}

impl JournalHead {
    fn clean(committed_length: u64, committed_chain: [u8; 32]) -> Self {
        Self {
            committed_length,
            committed_chain,
            pending: None,
            expected_chain: committed_chain,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ObjectStore {
    state_root: PathBuf,
    pool: Arc<File>,
    objects: Arc<File>,
    objects_device: u64,
    objects_inode: u64,
    attempts: Arc<File>,
    read_meter: Option<Arc<ObjectReadMeter>>,
    adoption_batch: Option<Arc<Mutex<BatchState>>>,
    sync_totals: Option<Arc<crate::causal_profile::SyncTotalsV1>>,
    pool_write_admission:
        Option<Arc<dyn quanta_index_core::pool_write_admission::PoolWriteAdmissionPort>>,
    control_output_admission: Option<PublicationControlOutputAdmission>,
    work_output_admission: Option<PublicationWorkOutputAdmission>,
    object_lifetime: Option<Arc<dyn PublicationObjectLifetimePort>>,
}

struct BatchState {
    lease_name: String,
    binding: AttemptBinding,
    write_cost: Arc<Mutex<AttemptWriteCost>>,
    gate: Option<Gate>,
    references: Vec<ObjectRef>,
    encoded_new_bytes: u64,
    closed: bool,
}

struct BatchScope<'a> {
    store: &'a ObjectStore,
}

impl Drop for BatchScope<'_> {
    fn drop(&mut self) {
        if let Some(batch) = &self.store.adoption_batch {
            let mut state = match batch.lock() {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.closed = true;
            state.gate = None;
        }
    }
}

fn lock_batch_state(batch: &Mutex<BatchState>) -> Result<MutexGuard<'_, BatchState>, CoreError> {
    match batch.lock() {
        Ok(state) => Ok(state),
        Err(poisoned) => {
            let state = poisoned.into_inner();
            if state.closed {
                Ok(state)
            } else {
                Err(corrupt("adoption batch lock poisoned while active"))
            }
        }
    }
}

#[derive(Default)]
struct ObjectReadMeter {
    bytes: AtomicU64,
    objects: AtomicU64,
    fault: Mutex<Option<ObjectRef>>,
    objects_cache: Mutex<RequestObjectCache>,
}

type ObjectContents = (Vec<ObjectRef>, Vec<u8>);

// Reuse authenticated metadata within one metered request. Body/block payloads
// stay outside the shared 4 MiB bound; every new request authenticates again.
fn request_cache_eligible(kind: ObjectKind) -> bool {
    matches!(
        kind,
        ObjectKind::MapPage | ObjectKind::Record | ObjectKind::Segment | ObjectKind::FilePostings
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Page {
    Leaf(Vec<(Vec<u8>, BlobRef)>),
    Branch(Vec<PageRef>),
}

struct DecodedMapPage {
    descriptor: PageRef,
    page: Arc<Page>,
}

struct CachedObject {
    dependencies: Vec<ObjectRef>,
    payload: Vec<u8>,
    // Only this request/proof meter owns the decoded Arc. An evicted Arc held
    // briefly by a caller is traversal scratch, not retained cache capacity.
    decoded: Option<DecodedMapPage>,
    charged_bytes: usize,
    last_used: u64,
}

struct RequestObjectCache {
    entries: BTreeMap<ObjectRef, CachedObject>,
    lru: BTreeSet<(u64, ObjectRef)>,
    retained_bytes: usize,
    next_use: u64,
    limit_bytes: usize,
}

impl Default for RequestObjectCache {
    fn default() -> Self {
        Self::new(REQUEST_OBJECT_CACHE_BYTES)
    }
}

impl RequestObjectCache {
    fn new(limit_bytes: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            lru: BTreeSet::new(),
            retained_bytes: 0,
            next_use: 0,
            limit_bytes,
        }
    }

    fn next_use(&mut self) -> Result<u64, CoreError> {
        self.next_use = self
            .next_use
            .checked_add(1)
            .ok_or_else(|| invalid("request object cache use count overflow"))?;
        Ok(self.next_use)
    }

    fn touch(&mut self, reference: &ObjectRef) -> Result<(), CoreError> {
        let use_number = self.next_use()?;
        let Some(entry) = self.entries.get_mut(reference) else {
            return Err(invalid("request object cache entry disappeared"));
        };
        let _removed = self.lru.remove(&(entry.last_used, reference.clone()));
        entry.last_used = use_number;
        let _inserted = self.lru.insert((use_number, reference.clone()));
        Ok(())
    }

    fn get(&mut self, reference: &ObjectRef) -> Result<Option<ObjectContents>, CoreError> {
        if !self.entries.contains_key(reference) {
            return Ok(None);
        }
        self.touch(reference)?;
        let Some(entry) = self.entries.get(reference) else {
            return Err(invalid("request object cache entry disappeared"));
        };
        Ok(Some((entry.dependencies.clone(), entry.payload.clone())))
    }

    fn get_decoded(&mut self, descriptor: &PageRef) -> Result<Option<Arc<Page>>, CoreError> {
        let Some(entry) = self.entries.get(&descriptor.object) else {
            return Ok(None);
        };
        let Some(decoded) = &entry.decoded else {
            return Ok(None);
        };
        if decoded.descriptor != *descriptor {
            return Err(corrupt("cached map page descriptor differs"));
        }
        let page = Arc::clone(&decoded.page);
        self.touch(&descriptor.object)?;
        Ok(Some(page))
    }

    fn decoded_charge(descriptor: &PageRef, page: &Page) -> Option<usize> {
        let envelope = std::mem::size_of::<DecodedMapPage>()
            .checked_add(std::mem::size_of::<Page>())?
            .checked_add(std::mem::size_of::<[usize; 2]>())?
            .checked_add(descriptor.first.capacity())?
            .checked_add(descriptor.last.capacity())?;
        match page {
            Page::Leaf(rows) => rows.iter().try_fold(
                envelope.checked_add(
                    rows.capacity()
                        .checked_mul(std::mem::size_of::<(Vec<u8>, BlobRef)>())?,
                )?,
                |bytes, (key, _)| bytes.checked_add(key.capacity()),
            ),
            Page::Branch(children) => children.iter().try_fold(
                envelope.checked_add(
                    children
                        .capacity()
                        .checked_mul(std::mem::size_of::<PageRef>())?,
                )?,
                |bytes, child| {
                    bytes
                        .checked_add(child.first.capacity())?
                        .checked_add(child.last.capacity())
                },
            ),
        }
    }

    fn attach_decoded(&mut self, descriptor: &PageRef, page: &Arc<Page>) -> Result<(), CoreError> {
        let Some(entry) = self.entries.get(&descriptor.object) else {
            return Ok(());
        };
        if let Some(decoded) = &entry.decoded {
            if decoded.descriptor != *descriptor {
                return Err(corrupt("cached map page descriptor differs"));
            }
            self.touch(&descriptor.object)?;
            return Ok(());
        }
        let Some(extra) = Self::decoded_charge(descriptor, page) else {
            return Ok(());
        };
        if entry
            .charged_bytes
            .checked_add(extra)
            .is_none_or(|bytes| bytes > self.limit_bytes)
        {
            return Ok(());
        }
        self.touch(&descriptor.object)?;
        while self
            .retained_bytes
            .checked_add(extra)
            .is_none_or(|bytes| bytes > self.limit_bytes)
        {
            let Some((_, oldest)) = self.lru.pop_first() else {
                return Err(invalid("request object cache eviction order is empty"));
            };
            if oldest == descriptor.object {
                return Err(invalid("request object cache evicted the current page"));
            }
            let removed = self
                .entries
                .remove(&oldest)
                .ok_or_else(|| invalid("request object cache eviction entry is absent"))?;
            self.retained_bytes = self
                .retained_bytes
                .checked_sub(removed.charged_bytes)
                .ok_or_else(|| invalid("request object cache accounting underflow"))?;
        }
        let Some(entry) = self.entries.get_mut(&descriptor.object) else {
            return Err(invalid("request object cache entry disappeared"));
        };
        entry.decoded = Some(DecodedMapPage {
            descriptor: descriptor.clone(),
            page: Arc::clone(page),
        });
        entry.charged_bytes = entry
            .charged_bytes
            .checked_add(extra)
            .ok_or_else(|| invalid("request object cache entry charge overflow"))?;
        self.retained_bytes = self
            .retained_bytes
            .checked_add(extra)
            .ok_or_else(|| invalid("request object cache accounting overflow"))?;
        Ok(())
    }

    fn insert(
        &mut self,
        reference: &ObjectRef,
        dependencies: &[ObjectRef],
        payload: &[u8],
    ) -> Result<(), CoreError> {
        if self.entries.contains_key(reference) {
            let _cached = self.get(reference)?;
            return Ok(());
        }
        let dependencies = dependencies.to_vec();
        let payload = payload.to_vec();
        let charged_bytes = dependencies
            .capacity()
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .and_then(|bytes| bytes.checked_add(payload.capacity()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<CachedObject>()))
            .and_then(|bytes| {
                std::mem::size_of::<ObjectRef>()
                    .checked_mul(2)
                    .and_then(|keys| bytes.checked_add(keys))
            })
            .and_then(|bytes| bytes.checked_add(OBJECT_CACHE_ENTRY_OVERHEAD));
        let Some(charged_bytes) = charged_bytes else {
            return Ok(());
        };
        if charged_bytes > self.limit_bytes {
            return Ok(());
        }
        while self
            .retained_bytes
            .checked_add(charged_bytes)
            .is_none_or(|bytes| bytes > self.limit_bytes)
        {
            let Some((_, oldest)) = self.lru.pop_first() else {
                return Err(invalid("request object cache eviction order is empty"));
            };
            let removed = self
                .entries
                .remove(&oldest)
                .ok_or_else(|| invalid("request object cache eviction entry is absent"))?;
            self.retained_bytes = self
                .retained_bytes
                .checked_sub(removed.charged_bytes)
                .ok_or_else(|| invalid("request object cache accounting underflow"))?;
        }
        let use_number = self.next_use()?;
        self.retained_bytes = self
            .retained_bytes
            .checked_add(charged_bytes)
            .ok_or_else(|| invalid("request object cache accounting overflow"))?;
        let _previous = self.entries.insert(
            reference.clone(),
            CachedObject {
                dependencies,
                payload,
                decoded: None,
                charged_bytes,
                last_used: use_number,
            },
        );
        let _inserted = self.lru.insert((use_number, reference.clone()));
        Ok(())
    }
}

pub(crate) struct AttemptLease {
    store: ObjectStore,
    name: String,
    binding: AttemptBinding,
    write_cost: Arc<Mutex<AttemptWriteCost>>,
}

/// Authenticated pool write outcomes observed by one attempt invocation.
///
/// This is not retained disk usage or a capacity reservation: a retry starts
/// a new meter, while physical objects can survive either attempt.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AttemptWriteCost {
    pub(crate) new_objects: u64,
    pub(crate) new_encoded_bytes: u64,
    pub(crate) reused_objects: u64,
    pub(crate) reused_encoded_bytes: u64,
}

struct PreparedObject {
    reference: ObjectRef,
    encoded: Vec<u8>,
}

impl PreparedObject {
    fn new(
        kind: ObjectKind,
        dependencies: &[ObjectRef],
        payload: &[u8],
    ) -> Result<Self, CoreError> {
        let encoded = encode_object(kind, dependencies, payload)?;
        let reference = ObjectRef {
            kind,
            bytes: u64::try_from(encoded.len())
                .map_err(|error| storage("object byte width", error))?,
            sha256: Sha256::digest(&encoded).into(),
        };
        Ok(Self { reference, encoded })
    }
}

type TargetScope = ([u8; 32], [u8; 32], u64);

pub(crate) enum AdoptedRootAdvance {
    Root(ObjectRef),
    Advanced,
    Done,
}

struct AdoptedJournal {
    file: ControlReader,
    remaining: u64,
    previous: [u8; 32],
    expected_chain: [u8; 32],
}

/// Retains directory and journal positions between bounded GC steps. A caller
/// must discard this cursor when the store epoch changes.
pub(crate) struct AdoptedRootCursor {
    directory: rustix::fs::Dir,
    attempts_seen: usize,
    header_roots: std::vec::IntoIter<ObjectRef>,
    journal: Option<AdoptedJournal>,
}

pub(crate) enum ObjectInventoryAdvance {
    Digest([u8; 32]),
    Advanced,
    Done,
}

/// One directory stream is held at a time; progress does not rescan earlier
/// entries in a large shard on every maintenance tick.
pub(crate) struct ObjectInventoryCursor {
    next_prefix: u16,
    directory: Option<(u8, File, rustix::fs::Dir)>,
}

enum BlobFrame {
    Object {
        reference: ObjectRef,
        root: bool,
        depth: u8,
        expected_len: u64,
    },
    Finish {
        start: u64,
        expected_len: u64,
    },
}

/// In-process authenticated traversal of one logical blob.
///
/// Completion is
/// established only when `next` returns `None` after checking the logical
/// length and SHA-256; a dropped cursor proves no complete blob.
pub(crate) struct BlobCursor {
    frames: Vec<BlobFrame>,
    expected_len: u64,
    expected_sha256: [u8; 32],
    produced: u64,
    digest: Sha256,
    complete: bool,
}

impl BlobCursor {
    pub(crate) fn next(
        &mut self,
        store: &ObjectStore,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Option<(Vec<u8>, u64)>, CoreError> {
        if self.complete {
            return Ok(None);
        }
        let mut charged = 0_u64;
        while let Some(frame) = self.frames.pop() {
            checkpoint(budget, "lexical:blob:cursor")?;
            match frame {
                BlobFrame::Finish {
                    start,
                    expected_len,
                } => {
                    if self.produced.checked_sub(start) != Some(expected_len) {
                        return Err(corrupt("blob cursor subtree length differs"));
                    }
                }
                BlobFrame::Object {
                    reference,
                    root,
                    depth,
                    expected_len,
                } => {
                    if depth > 16 {
                        return Err(corrupt("blob cursor tree exceeds maximum depth"));
                    }
                    let (dependencies, payload) = store.read_object(&reference, budget)?;
                    charged = charged
                        .checked_add(reference.bytes)
                        .ok_or_else(|| corrupt("blob cursor byte charge overflow"))?;
                    if !root && reference.kind == ObjectKind::BlobBlock {
                        if !dependencies.is_empty()
                            || payload.is_empty()
                            || payload.len() > BLOCK_BYTES
                            || u64::try_from(payload.len())
                                .map_err(|_conversion_error| corrupt("blob block width"))?
                                != expected_len
                        {
                            return Err(corrupt("blob cursor block shape or length differs"));
                        }
                        return self.emit(payload, charged).map(Some);
                    }
                    if !(root && reference.kind.is_payload()
                        || !root && reference.kind == ObjectKind::BlobNode)
                    {
                        return Err(corrupt("blob cursor tree contains a wrong object kind"));
                    }
                    let Some((&tag, rest)) = payload.split_first() else {
                        return Err(corrupt("blob cursor node has no payload tag"));
                    };
                    if tag == 0 {
                        if !root
                            || rest.len() > BLOCK_BYTES
                            || u64::try_from(rest.len())
                                .map_err(|_conversion_error| corrupt("inline blob width"))?
                                != expected_len
                        {
                            return Err(corrupt("blob cursor inline node is noncanonical"));
                        }
                        return self.emit(rest.to_vec(), charged).map(Some);
                    }
                    if tag != 1 || root && self.expected_len <= BLOCK_BYTES_U64 {
                        return Err(corrupt("blob cursor node tag or shape differs"));
                    }
                    let children = decode_blob_node(rest)?;
                    let mut child_refs = Vec::new();
                    child_refs
                        .try_reserve_exact(children.len())
                        .map_err(|error| storage("reserve blob cursor children", error))?;
                    child_refs.extend(children.iter().map(|(child, _)| child.clone()));
                    if child_refs.iter().any(|child| !dependencies.contains(child))
                        || !root
                            && dependencies
                                .iter()
                                .any(|dependency| !child_refs.contains(dependency))
                    {
                        return Err(corrupt("blob cursor dependency set differs"));
                    }
                    self.frames
                        .try_reserve(children.len().saturating_add(1))
                        .map_err(|error| storage("reserve blob cursor frames", error))?;
                    self.frames.push(BlobFrame::Finish {
                        start: self.produced,
                        expected_len,
                    });
                    for (child, length) in children.into_iter().rev() {
                        self.frames.push(BlobFrame::Object {
                            reference: child,
                            root: false,
                            depth: depth.saturating_add(1),
                            expected_len: length,
                        });
                    }
                }
            }
        }
        if self.produced != self.expected_len
            || <[u8; 32]>::from(self.digest.clone().finalize()) != self.expected_sha256
        {
            return Err(corrupt("blob cursor logical length or digest differs"));
        }
        self.complete = true;
        Ok(None)
    }

    fn emit(&mut self, bytes: Vec<u8>, charged: u64) -> Result<(Vec<u8>, u64), CoreError> {
        let width =
            u64::try_from(bytes.len()).map_err(|_conversion_error| corrupt("blob chunk width"))?;
        self.produced = self
            .produced
            .checked_add(width)
            .ok_or_else(|| corrupt("blob cursor length overflow"))?;
        if self.produced > self.expected_len {
            return Err(corrupt("blob cursor exceeds admitted logical length"));
        }
        self.digest.update(&bytes);
        Ok((bytes, charged))
    }
}

pub(crate) struct Gate {
    reader: ControlReader,
}

impl Gate {
    fn retire(self) -> Result<(), CoreError> {
        self.reader.retire()
    }
}

fn finish_gate<T>(gate: Gate, result: Result<T, CoreError>) -> Result<T, CoreError> {
    match (result, gate.retire()) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(retirement)) => Err(retirement),
        (Err(primary), Err(retirement)) => Err(with_custody_context(primary, retirement)),
    }
}

fn finish_control_read<T>(
    reader: ControlReader,
    result: Result<T, CoreError>,
) -> Result<T, CoreError> {
    match (result, reader.retire()) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(retirement)) => Err(retirement),
        (Err(primary), Err(retirement)) => Err(with_custody_context(primary, retirement)),
    }
}

enum JournalWriter {
    Unqualified(File),
    Admitted(Box<dyn PublicationControlFileMutation>),
}

impl Read for JournalWriter {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Unqualified(file) => file.read(bytes),
            Self::Admitted(mutation) => mutation.read(bytes),
        }
    }
}

impl Write for JournalWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Unqualified(file) => file.write(bytes),
            Self::Admitted(mutation) => mutation.write(bytes),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Unqualified(file) => file.flush(),
            Self::Admitted(mutation) => mutation.flush(),
        }
    }
}

impl Seek for JournalWriter {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::Unqualified(file) => file.seek(position),
            Self::Admitted(mutation) => mutation.seek(position),
        }
    }
}

impl JournalWriter {
    fn set_len(&mut self, length: u64) -> Result<(), CoreError> {
        match self {
            Self::Unqualified(file) => file
                .set_len(length)
                .map_err(|error| storage("truncate pending journal", error)),
            Self::Admitted(mutation) => mutation.set_len(length),
        }
    }

    fn retire_preserving_pending(self) -> Result<(), CoreError> {
        match self {
            Self::Unqualified(file) => {
                drop(file);
                Ok(())
            }
            Self::Admitted(mutation) => mutation.retire_preserving_pending(),
        }
    }
}

fn admitted_io_error(context: &str, error: &std::io::Error) -> CoreError {
    let context = format!("lexical object store {context}");
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(cause) = source {
        if let Some(primary) = cause.downcast_ref::<CoreError>() {
            return primary.clone();
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>()
            && io.raw_os_error().is_some()
        {
            return CoreError::storage_io(&context, io);
        }
        source = cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .map(|inner| -> &(dyn std::error::Error + 'static) { inner })
            .or_else(|| cause.source());
    }
    CoreError::storage_io(&context, error)
}

fn retire_failed_journal(journal: JournalWriter, primary: CoreError) -> CoreError {
    match journal.retire_preserving_pending() {
        Ok(()) => primary,
        Err(retirement) => with_custody_context(primary, retirement),
    }
}

impl ObjectStore {
    /// Fixture-only late binding. Production constructors install lifetime
    /// custody before opening Control readers.
    #[cfg(test)]
    pub(crate) fn with_object_lifetime(
        mut self,
        object_lifetime: Option<Arc<dyn PublicationObjectLifetimePort>>,
    ) -> Result<Self, CoreError> {
        if let Some(lifetime) = &object_lifetime {
            lifetime.validate_state_root(&self.state_root)?;
        }
        self.object_lifetime = object_lifetime;
        Ok(self)
    }

    pub(crate) fn with_work_output_admission(
        mut self,
        admission: Option<PublicationWorkOutputAdmission>,
    ) -> Self {
        self.work_output_admission = admission;
        self
    }

    pub(crate) fn sync_totals(&self) -> Option<&crate::causal_profile::SyncTotalsV1> {
        self.sync_totals.as_deref()
    }

    pub(crate) fn record_compaction(&self, segments: u64, documents: u64, bytes: u64) {
        if let Some(totals) = self.sync_totals.as_deref() {
            totals.record_compaction(segments, documents, bytes);
        }
    }

    /// Groups physical publication under one durable epoch and adoption head.
    /// The callback must use the scoped store for all writes.
    pub(crate) fn with_adoption_batch<T>(
        &self,
        lease: &AttemptLease,
        operation: impl FnOnce(&ObjectStore) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        lease.ensure_store(self)?;
        if let Some(batch) = &self.adoption_batch {
            let state = lock_batch_state(batch)?;
            if state.closed || state.lease_name != lease.name || state.binding != lease.binding {
                return Err(invalid(
                    "nested adoption batch has another or closed attempt",
                ));
            }
            drop(state);
            return operation(self);
        }
        let mut scoped = self.clone();
        scoped.adoption_batch = Some(Arc::new(Mutex::new(BatchState {
            lease_name: lease.name.clone(),
            binding: lease.binding.clone(),
            write_cost: Arc::clone(&lease.write_cost),
            gate: None,
            references: Vec::new(),
            encoded_new_bytes: 0,
            closed: false,
        })));
        let guard = BatchScope { store: &scoped };
        let result = operation(&scoped);
        let result = match result {
            Ok(value) => {
                scoped.flush_adoption_batch()?;
                Ok(value)
            }
            Err(error) => Err(error),
        };
        drop(guard);
        result
    }

    /// Completes the current group and releases the physical write gate.
    /// A subsequent scoped write starts another group lazily.
    pub(crate) fn flush_adoption_batch(&self) -> Result<(), CoreError> {
        let Some(batch) = &self.adoption_batch else {
            return Ok(());
        };
        let mut state = lock_batch_state(batch)?;
        if state.closed {
            return Err(invalid("adoption batch view is closed"));
        }
        let result =
            self.flush_adoption_batch_locked(&mut state, DiagnosticAdoptionFlushReasonV1::Explicit);
        drop(state);
        result
    }

    fn flush_adoption_batch_locked(
        &self,
        state: &mut BatchState,
        reason: DiagnosticAdoptionFlushReasonV1,
    ) -> Result<(), CoreError> {
        if !state.references.is_empty() {
            let refs = crate::causal_profile::occupancy_bucket(
                u64::try_from(state.references.len())
                    .map_err(|_overflow| invalid("adoption reference count overflow"))?,
                u64::try_from(MAX_ADOPTION_BATCH_REFS)
                    .map_err(|_overflow| invalid("adoption reference cap overflow"))?,
            );
            let bytes = crate::causal_profile::occupancy_bucket(
                state.encoded_new_bytes,
                MAX_ADOPTION_BATCH_DATA_BYTES,
            );
            // The batched macOS object path uses POSIX fsync on each file and
            // shard. Drain the drive before any adoption head can name them.
            let result = (|| {
                self.sync_pool_before_adoption()?;
                let lease = AttemptLease {
                    store: self.clone(),
                    name: state.lease_name.clone(),
                    binding: state.binding.clone(),
                    write_cost: Arc::clone(&state.write_cost),
                };
                lease.adopt_batch_locked(&state.references)
            })();
            if let Some(totals) = self.sync_totals.as_deref() {
                totals.record_adoption(reason, refs, bytes, result.is_ok());
            }
            result?;
            state.references.clear();
            state.encoded_new_bytes = 0;
        }
        state.gate = None;
        Ok(())
    }

    fn sync_pool_before_adoption(&self) -> Result<(), CoreError> {
        #[cfg(test)]
        crate::publication_faults::reach_core(
            crate::publication_faults::Cut::PoolFullBarrier,
            crate::publication_faults::Side::Before,
        )?;
        crate::causal_profile::observed_sync_legacy(
            self.sync_totals.as_deref(),
            quanta_index_core::DiagnosticSyncLabelV1::PoolAdoptionFullBarrier,
            || self.objects.sync_all(),
        )
        .map_err(|error| storage("sync object pool before adoption", error))?;
        #[cfg(test)]
        crate::publication_faults::reach_core(
            crate::publication_faults::Cut::PoolFullBarrier,
            crate::publication_faults::Side::After,
        )?;
        Ok(())
    }

    fn check_batch_lease(&self, state: &BatchState, lease: &AttemptLease) -> Result<(), CoreError> {
        if state.closed || state.lease_name != lease.name || state.binding != lease.binding {
            return Err(invalid("adoption batch view has another or closed attempt"));
        }
        Ok(())
    }

    fn start_batch_locked(&self, state: &mut BatchState) -> Result<(), CoreError> {
        if state.gate.is_none() {
            let gate = self.lock_gate()?;
            let record = self
                .read_attempt(&state.lease_name)?
                .ok_or_else(|| corrupt("batched attempt record is absent"))?;
            if record.0 != state.binding {
                return Err(corrupt("batched attempt binding changed"));
            }
            self.open_attempt_log(&state.lease_name)?.retire()?;
            let _epoch = self.bump_epoch_locked()?;
            state.gate = Some(gate);
        }
        Ok(())
    }

    fn queue_batch_reference(
        &self,
        state: &mut BatchState,
        reference: &ObjectRef,
        newly_written_bytes: Option<u64>,
    ) -> Result<(), CoreError> {
        if !state.references.contains(reference) {
            state
                .references
                .try_reserve(1)
                .map_err(|error| storage("reserve batched adoption reference", error))?;
            state.references.push(reference.clone());
            if let Some(width) = newly_written_bytes {
                state.encoded_new_bytes = state
                    .encoded_new_bytes
                    .checked_add(width)
                    .ok_or_else(|| invalid("batched object byte count overflow"))?;
            }
        }
        if state.references.len() >= MAX_ADOPTION_BATCH_REFS
            || state.encoded_new_bytes >= MAX_ADOPTION_BATCH_DATA_BYTES
        {
            let reason = if state.references.len() >= MAX_ADOPTION_BATCH_REFS {
                DiagnosticAdoptionFlushReasonV1::RefLimit
            } else {
                DiagnosticAdoptionFlushReasonV1::ByteLimit
            };
            self.flush_adoption_batch_locked(state, reason)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn open(state_root: &Path) -> Result<Self, CoreError> {
        Self::open_observed(state_root, None)
    }

    #[cfg(test)]
    pub(crate) fn open_observed(
        state_root: &Path,
        sync_totals: Option<Arc<crate::causal_profile::SyncTotalsV1>>,
    ) -> Result<Self, CoreError> {
        Self::open_observed_with_control(state_root, sync_totals, None, None)
    }

    /// Install the physical Control parent before creating the first pool
    /// regular file. The unqualified constructor remains for local fixtures.
    pub(crate) fn open_observed_with_control(
        state_root: &Path,
        sync_totals: Option<Arc<crate::causal_profile::SyncTotalsV1>>,
        control_output_admission: Option<PublicationControlOutputAdmission>,
        object_lifetime: Option<Arc<dyn PublicationObjectLifetimePort>>,
    ) -> Result<Self, CoreError> {
        validate_control_object_lifetime(
            control_output_admission.as_ref(),
            object_lifetime.as_ref(),
        )?;
        if let Some(lifetime) = &object_lifetime {
            lifetime.validate_state_root(state_root)?;
        }
        let state = File::from(
            open(state_root, dir_flags(), Mode::empty())
                .map_err(|error| storage("open state root", error))?,
        );
        let pool = ensure_dir(&state, POOL_DIR, sync_totals.as_deref())?;
        let objects = ensure_dir(&pool, OBJECTS_DIR, sync_totals.as_deref())?;
        let attempts = ensure_dir(&pool, ATTEMPTS_DIR, sync_totals.as_deref())?;
        crate::causal_profile::observed_sync(
            sync_totals.as_deref(),
            quanta_index_core::DiagnosticSyncLabelV1::PoolSyncAll,
            || pool.sync_all(),
        )
        .map_err(|error| storage("sync pool directory", error))?;
        crate::causal_profile::observed_sync(
            sync_totals.as_deref(),
            quanta_index_core::DiagnosticSyncLabelV1::StateSyncAll,
            || state.sync_all(),
        )
        .map_err(|error| storage("sync state root", error))?;
        let objects_metadata = objects
            .metadata()
            .map_err(|error| storage("stat object pool identity", error))?;
        let store = Self {
            state_root: state_root.to_owned(),
            pool: Arc::new(pool),
            objects: Arc::new(objects),
            objects_device: objects_metadata.dev(),
            objects_inode: objects_metadata.ino(),
            attempts: Arc::new(attempts),
            read_meter: None,
            adoption_batch: None,
            sync_totals,
            pool_write_admission: None,
            control_output_admission,
            work_output_admission: None,
            object_lifetime,
        };
        if store.control_output_admission.is_some() {
            if !store.existing_regular_gate(store.pool.as_ref())? {
                store.write_atomic_at(store.pool.as_ref(), GATE_FILE, &[], AtomicPurpose::Other)?;
            }
        } else {
            // Preserve the original shared-inode bootstrap for unqualified fixtures.
            let gate = File::from(
                openat(
                    store.pool.as_ref(),
                    GATE_FILE,
                    OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::CREATE,
                    Mode::RUSR | Mode::WUSR,
                )
                .map_err(|error| storage("open lifecycle gate", error))?,
            );
            drop(gate);
            store
                .pool
                .sync_all()
                .map_err(|error| storage("sync lifecycle gate parent", error))?;
        }
        let gate = store.lock_gate()?;
        let result = (|| {
            if let Some(reader) = store.open_control_reader(
                store.pool.as_ref(),
                EPOCH_FILE,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                true,
                "probe lifecycle epoch",
            )? {
                reader.retire()?;
            } else {
                store.write_epoch(0)?;
            }
            let _epoch = store.read_epoch()?;
            Ok(())
        })();
        finish_gate(gate, result)?;
        Ok(store)
    }

    /// Opens an existing pool without creating directories or control files.
    pub(crate) fn open_existing(state_root: &Path) -> Result<Self, CoreError> {
        Self::open_existing_with_lifetime(state_root, None)
    }

    /// Install read-only physical lifetime custody before opening Control FDs.
    pub(crate) fn open_existing_with_lifetime(
        state_root: &Path,
        object_lifetime: Option<Arc<dyn PublicationObjectLifetimePort>>,
    ) -> Result<Self, CoreError> {
        if let Some(lifetime) = &object_lifetime {
            lifetime.validate_state_root(state_root)?;
        }
        let state = File::from(
            open(state_root, dir_flags(), Mode::empty())
                .map_err(|error| storage("open existing state root", error))?,
        );
        let pool = File::from(
            openat(&state, POOL_DIR, dir_flags(), Mode::empty())
                .map_err(|error| corrupt(&format!("existing object pool absent: {error}")))?,
        );
        let objects = File::from(
            openat(&pool, OBJECTS_DIR, dir_flags(), Mode::empty())
                .map_err(|error| corrupt(&format!("existing object directory absent: {error}")))?,
        );
        let attempts = File::from(
            openat(&pool, ATTEMPTS_DIR, dir_flags(), Mode::empty())
                .map_err(|error| corrupt(&format!("existing attempt directory absent: {error}")))?,
        );
        let objects_metadata = objects
            .metadata()
            .map_err(|error| storage("stat existing object pool identity", error))?;
        let store = Self {
            state_root: state_root.to_owned(),
            pool: Arc::new(pool),
            objects: Arc::new(objects),
            objects_device: objects_metadata.dev(),
            objects_inode: objects_metadata.ino(),
            attempts: Arc::new(attempts),
            read_meter: None,
            adoption_batch: None,
            sync_totals: None,
            pool_write_admission: None,
            control_output_admission: None,
            work_output_admission: None,
            object_lifetime,
        };
        let gate = store
            .open_control_reader(
                store.pool.as_ref(),
                GATE_FILE,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                false,
                "open existing lifecycle gate",
            )?
            .ok_or_else(|| corrupt("existing lifecycle gate absent"))?;
        let gate_check = gate.metadata().and_then(|metadata| {
            if metadata.is_file() {
                Ok(())
            } else {
                Err(corrupt("existing lifecycle gate is not a regular file"))
            }
        });
        finish_control_read(gate, gate_check)?;
        let _epoch = store.read_epoch()?;
        Ok(store)
    }

    fn write_epoch(&self, epoch: u64) -> Result<(), CoreError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(48)
            .map_err(|error| storage("reserve lifecycle epoch", error))?;
        bytes.extend_from_slice(EPOCH_MAGIC);
        bytes.extend_from_slice(&epoch.to_be_bytes());
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        bytes.extend_from_slice(&digest);
        self.write_atomic_at(self.pool.as_ref(), EPOCH_FILE, &bytes, AtomicPurpose::Other)
    }

    fn read_epoch(&self) -> Result<u64, CoreError> {
        let mut file = self
            .open_control_reader(
                self.pool.as_ref(),
                EPOCH_FILE,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                false,
                "open lifecycle epoch",
            )?
            .ok_or_else(|| corrupt("lifecycle epoch absent"))?;
        let result = (|| {
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.len() != 48 {
                return Err(corrupt("lifecycle epoch is not a fixed regular file"));
            }
            let mut bytes = [0_u8; 48];
            file.read_exact(&mut bytes)
                .map_err(|error| admitted_io_error("read lifecycle epoch", &error))?;
            let header = bytes
                .get(..16)
                .ok_or_else(|| corrupt("epoch header truncated"))?;
            let expected: [u8; 32] = Sha256::digest(header).into();
            if header.get(..8) != Some(EPOCH_MAGIC.as_slice())
                || bytes.get(16..) != Some(expected.as_slice())
            {
                return Err(corrupt("lifecycle epoch digest or magic differs"));
            }
            let raw: [u8; 8] = header
                .get(8..16)
                .ok_or_else(|| corrupt("epoch counter truncated"))?
                .try_into()
                .map_err(|_conversion_error| corrupt("epoch counter width"))?;
            Ok(u64::from_be_bytes(raw))
        })();
        finish_control_read(file, result)
    }

    fn bump_epoch_locked(&self) -> Result<u64, CoreError> {
        let next = self
            .read_epoch()?
            .checked_add(1)
            .ok_or_else(|| invalid("lifecycle epoch exhausted"))?;
        self.write_epoch(next)?;
        Ok(next)
    }

    pub(super) fn lock_gate(&self) -> Result<Gate, CoreError> {
        let reader = self
            .open_control_reader(
                self.pool.as_ref(),
                GATE_FILE,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                false,
                "open lifecycle gate",
            )?
            .ok_or_else(|| corrupt("lifecycle gate absent"))?;
        if let Err(error) = reader.lock_exclusive() {
            return finish_control_read(reader, Err(error));
        }
        Ok(Gate { reader })
    }

    /// Freeze all pool namespace writes, including GC, without waiting on a
    /// writer that may need the adapter's lifecycle boundary held by a scan.
    pub(crate) fn try_physical_snapshot_gate(&self) -> Result<Gate, CoreError> {
        let reader = self
            .open_control_reader(
                self.pool.as_ref(),
                GATE_FILE,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                false,
                "open physical snapshot gate",
            )?
            .ok_or_else(|| corrupt("physical snapshot gate absent"))?;
        let result = (|| {
            if !reader.metadata()?.is_file() {
                return Err(corrupt("physical snapshot gate is not a regular file"));
            }
            if !reader.try_lock_exclusive()? {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
                    message: "lexical physical snapshot: object pool is being mutated; retry measurement".into(),
                });
            }
            Ok(())
        })();
        if let Err(error) = result {
            return finish_control_read(reader, Err(error));
        }
        Ok(Gate { reader })
    }

    /// A handle with counters private to this clone and any clones derived
    /// from it. Query and maintenance callers each create their own meter.
    pub(crate) fn metered_clone(&self) -> Self {
        let mut clone = self.clone();
        clone.read_meter = Some(Arc::new(ObjectReadMeter::default()));
        clone
    }

    pub(crate) fn read_meter_snapshot(&self) -> (u64, u64) {
        self.read_meter.as_ref().map_or((0, 0), |meter| {
            (
                meter.bytes.load(Ordering::Relaxed),
                meter.objects.load(Ordering::Relaxed),
            )
        })
    }

    pub(crate) fn has_read_meter(&self) -> bool {
        self.read_meter.is_some()
    }

    pub(crate) fn fault_reference(&self) -> Result<Option<ObjectRef>, CoreError> {
        self.read_meter.as_ref().map_or(Ok(None), |meter| {
            meter
                .fault
                .lock()
                .map(|slot| slot.clone())
                .map_err(|error| storage("read scrub object fault", error))
        })
    }

    /// Adopt inherited roots before any writer reuses their objects. A retry
    /// with the same attempt identity but changed inputs is refused.
    pub(crate) fn begin_attempt(
        &self,
        binding: &AttemptBinding,
        inherited: &[ObjectRef],
    ) -> Result<AttemptLease, CoreError> {
        binding.validate()?;
        let _gate = self.lock_gate()?;
        let mut inherited = inherited.to_vec();
        inherited.sort();
        inherited.dedup();
        for root in &inherited {
            let _verified = self.read_object(root, None)?;
        }
        let name = binding.identity_name();
        self.recover_retirement_locked(&name)?;
        match self.read_attempt_header(&name)? {
            Some(record) if record.0 == *binding && record.1 == inherited => {
                self.recover_attempt_log(&name)?;
                self.visit_attempt_log(&name, |_reference| Ok(()))?;
            }
            Some(_) => return Err(invalid("attempt identity is bound to different inputs")),
            None => {
                let _epoch = self.bump_epoch_locked()?;
                // The header is the commit marker. No lease can be issued
                // until its empty journal and clean head are durable.
                let _cleared = self.cleanup_unowned_attempt_locked(&name)?;
                self.write_atomic_at(
                    self.attempts.as_ref(),
                    &journal_name(&name),
                    &[],
                    AtomicPurpose::Other,
                )?;
                self.write_journal_head(&name, &JournalHead::clean(0, journal_seed(&name)))?;
                self.write_attempt(&name, &AttemptRecord(binding.clone(), inherited))?;
            }
        }
        Ok(AttemptLease {
            store: self.clone(),
            name,
            binding: binding.clone(),
            write_cost: Arc::new(Mutex::new(AttemptWriteCost::default())),
        })
    }

    /// Caller must hold the lifecycle/writer-free fence and prove this exact
    /// target was discarded without a sealed publication. A scope search or
    /// age heuristic is not sufficient authority to retire an attempt.
    pub(crate) fn discard_exact_attempt(
        &self,
        binding: &AttemptBinding,
    ) -> Result<bool, CoreError> {
        binding.validate()?;
        let name = binding.identity_name();
        {
            let _gate = self.lock_gate()?;
            self.recover_retirement_locked(&name)?;
            let Some(record) = self.read_attempt_header(&name)? else {
                return Ok(false);
            };
            if record.0 != *binding {
                return Err(invalid("discard target attempt binding differs"));
            }
            self.recover_attempt_log(&name)?;
            self.visit_attempt_log(&name, |_reference| Ok(()))?;
        }
        AttemptLease {
            store: self.clone(),
            name,
            binding: binding.clone(),
            write_cost: Arc::new(Mutex::new(AttemptWriteCost::default())),
        }
        .discard_unsealed()?;
        Ok(true)
    }

    /// Retire every prepared attempt for an exact absent target namespace.
    /// The caller must hold its exclusive lifecycle/writer-free fence and
    /// prove no sealed or current generation owns that repo/revision/number.
    /// Incarnations are deliberately included: none can own an absent target.
    pub(crate) fn discard_target_attempts(
        &self,
        repository_sha256: [u8; 32],
        revision_sha256: [u8; 32],
        target_generation: u64,
    ) -> Result<u64, CoreError> {
        let matching =
            self.target_attempt_bindings(repository_sha256, revision_sha256, target_generation)?;
        let mut retired = 0_u64;
        for binding in matching {
            if self.discard_exact_attempt(&binding)? {
                retired = retired
                    .checked_add(1)
                    .ok_or_else(|| invalid("retired attempt count overflow"))?;
            }
        }
        Ok(retired)
    }

    /// Preserve the one original pending publication while its incomplete
    /// target directory is removed for an asymmetric retry. The catalog owns
    /// the Pending proof; this store independently checks every durable
    /// target attempt against the immutable event, body and sealed base.
    /// A different incarnation cannot be selected without the catalog's
    /// allocation, so multiple target attempts are refused rather than
    /// guessing which one to keep.
    pub(crate) fn preserve_target_attempt_for_pending_batch(
        &self,
        target: ([u8; 32], [u8; 32], u64),
        original: ([u8; 32], Option<[u8; 32]>, [u8; 32], u16),
        inherited_roots: &[ObjectRef],
    ) -> Result<bool, CoreError> {
        let (repository_sha256, revision_sha256, target_generation) = target;
        let (source_event_sha256, base_root_sha256, operations_sha256, format) = original;
        let matching =
            self.target_attempt_bindings(repository_sha256, revision_sha256, target_generation)?;
        if matching.len() > 1 {
            return Err(invalid(
                "pending target has multiple attempt incarnations; catalog allocation is required",
            ));
        }
        let Some(binding) = matching.first() else {
            return Ok(false);
        };
        if binding.source_event_sha256 != source_event_sha256
            || binding.base_root_sha256 != base_root_sha256
            || binding.operations_sha256 != operations_sha256
            || binding.format != format
        {
            return Err(invalid(
                "pending target attempt differs from original batch",
            ));
        }
        let name = binding.identity_name();
        let _gate = self.lock_gate()?;
        self.recover_retirement_locked(&name)?;
        let record = self
            .read_attempt_header(&name)?
            .ok_or_else(|| corrupt("pending target attempt disappeared during custody check"))?;
        let mut expected_inherited = inherited_roots.to_vec();
        expected_inherited.sort();
        expected_inherited.dedup();
        if record.0 != *binding || record.1 != expected_inherited {
            return Err(invalid(
                "pending target attempt inherited roots differ from sealed base",
            ));
        }
        for root in &expected_inherited {
            let _verified = self.read_object(root, None)?;
        }
        self.recover_attempt_log(&name)?;
        self.visit_attempt_log(&name, |_root| Ok(()))?;
        Ok(true)
    }

    /// Settle abandoned attempts after their immutable sealed target was
    /// independently authenticated. Caller holds the target's writer-free
    /// generation fence and has checked its source event and root descriptors.
    /// The sealed root, rather than an old attempt, owns the published closure.
    /// A conflicting event refuses before retirement.
    pub(crate) fn finish_target_attempts(
        &self,
        repository_sha256: [u8; 32],
        revision_sha256: [u8; 32],
        target_generation: u64,
        source_event_sha256: [u8; 32],
        committed_roots: &[ObjectRef],
    ) -> Result<u64, CoreError> {
        let matching =
            self.target_attempt_bindings(repository_sha256, revision_sha256, target_generation)?;
        if matching
            .iter()
            .any(|binding| binding.source_event_sha256 != source_event_sha256)
        {
            return Err(invalid(
                "sealed target has conflicting source-event attempt",
            ));
        }
        for root in committed_roots {
            let _verified = self.read_object(root, None)?;
        }
        let mut ready = Vec::new();
        ready
            .try_reserve_exact(matching.len())
            .map_err(|error| storage("reserve sealed attempt settlement", error))?;
        for binding in matching {
            if let Some(lease) = self.resume_existing_attempt(&binding)? {
                ready.push(lease);
            }
        }
        let mut retired = 0_u64;
        for lease in ready {
            lease.retire_after_sealed_settlement()?;
            retired = retired
                .checked_add(1)
                .ok_or_else(|| invalid("finished attempt count overflow"))?;
        }
        Ok(retired)
    }

    /// Enumerate only target namespaces with durable attempts. The caller
    /// uses this bounded set to select independently authenticated sealed
    /// roots for settlement instead of scanning every retained generation.
    pub(crate) fn pending_target_scopes(&self) -> Result<BTreeSet<TargetScope>, CoreError> {
        use std::os::unix::ffi::OsStringExt as _;

        self.recover_retirements()?;
        let mut directory = rustix::fs::Dir::read_from(self.attempts.as_ref())
            .map_err(|error| storage("enumerate pending target scopes", error))?;
        let mut scopes = BTreeSet::new();
        let mut inspected = 0_usize;
        while let Some(entry) = directory.read() {
            let entry = entry.map_err(|error| storage("read pending target entry", error))?;
            let name = std::ffi::OsString::from_vec(entry.file_name().to_bytes().to_vec());
            let Some(name) = name.to_str() else {
                return Err(corrupt("non-UTF8 pending target entry"));
            };
            if name.strip_suffix(".cbor").is_none() {
                continue;
            }
            inspected = inspected
                .checked_add(1)
                .ok_or_else(|| invalid("pending target count overflow"))?;
            if inspected > MAX_ATTEMPTS {
                return Err(invalid("pending target inventory exceeds bound"));
            }
            let record = self
                .read_attempt_header(name)?
                .ok_or_else(|| corrupt("listed pending target disappeared"))?;
            let _inserted = scopes.insert((
                record.0.repository_sha256,
                record.0.revision_sha256,
                record.0.target_generation,
            ));
        }
        Ok(scopes)
    }

    fn resume_existing_attempt(
        &self,
        binding: &AttemptBinding,
    ) -> Result<Option<AttemptLease>, CoreError> {
        let name = binding.identity_name();
        let _gate = self.lock_gate()?;
        self.recover_retirement_locked(&name)?;
        let Some(record) = self.read_attempt_header(&name)? else {
            return Ok(None);
        };
        if record.0 != *binding {
            return Err(corrupt("sealed target attempt binding changed"));
        }
        self.recover_attempt_log(&name)?;
        self.visit_attempt_log(&name, |_root| Ok(()))?;
        Ok(Some(AttemptLease {
            store: self.clone(),
            name,
            binding: binding.clone(),
            write_cost: Arc::new(Mutex::new(AttemptWriteCost::default())),
        }))
    }

    fn target_attempt_bindings(
        &self,
        repository_sha256: [u8; 32],
        revision_sha256: [u8; 32],
        target_generation: u64,
    ) -> Result<Vec<AttemptBinding>, CoreError> {
        use std::os::unix::ffi::OsStringExt as _;

        if repository_sha256 == [0; 32] || revision_sha256 == [0; 32] || target_generation == 0 {
            return Err(invalid("discard target scope has a zero identifier"));
        }
        self.recover_retirements()?;
        let mut directory = rustix::fs::Dir::read_from(self.attempts.as_ref())
            .map_err(|error| storage("enumerate target attempts", error))?;
        let mut matching = Vec::new();
        let mut inspected = 0_usize;
        while let Some(entry) = directory.read() {
            let entry = entry.map_err(|error| storage("read target attempt entry", error))?;
            let name = std::ffi::OsString::from_vec(entry.file_name().to_bytes().to_vec());
            let Some(name) = name.to_str() else {
                return Err(corrupt("non-UTF8 target attempt entry"));
            };
            if name.strip_suffix(".cbor").is_none() {
                continue;
            }
            inspected = inspected
                .checked_add(1)
                .ok_or_else(|| invalid("target attempt count overflow"))?;
            if inspected > MAX_ATTEMPTS {
                return Err(invalid("target attempt inventory exceeds bound"));
            }
            let record = self
                .read_attempt_header(name)?
                .ok_or_else(|| corrupt("listed target attempt disappeared"))?;
            let binding = record.0;
            if binding.repository_sha256 == repository_sha256
                && binding.revision_sha256 == revision_sha256
                && binding.target_generation == target_generation
            {
                matching
                    .try_reserve(1)
                    .map_err(|error| storage("reserve matching attempts", error))?;
                matching.push(binding);
            }
        }
        Ok(matching)
    }

    fn read_attempt(&self, name: &str) -> Result<Option<AttemptRecord>, CoreError> {
        let record = self.read_attempt_header(name)?;
        if record.is_some() {
            self.open_attempt_log(name)?.retire()?;
        }
        Ok(record)
    }

    /// Only a missing header authorizes removal of incomplete attempt control.
    /// The caller holds the physical gate; GC rechecks under
    /// that gate because the header may have appeared since readdir.
    fn cleanup_unowned_attempt_locked(&self, name: &str) -> Result<bool, CoreError> {
        self.reconcile_control_deletions()?;
        if self.read_attempt_header(name)?.is_some() {
            return Ok(false);
        }
        let mut removed = false;
        for component in [
            journal_name(name),
            journal_head_name(name),
            hard_scan_checkpoint::sidecar_name(name),
        ] {
            removed |= self.remove_attempt_control_locked(&component, true)?;
        }
        if removed {
            self.attempts
                .sync_all()
                .map_err(|error| storage("sync incomplete attempt cleanup", error))?;
        }
        Ok(true)
    }

    fn reconcile_control_deletions(&self) -> Result<(), CoreError> {
        let Some(control) = &self.control_output_admission else {
            return Ok(());
        };
        validate_control_object_lifetime(Some(control), self.object_lifetime.as_ref())?;
        if let Some(lifetime) = &self.object_lifetime {
            lifetime.validate_state_root(&self.state_root)?;
        }
        if let Some(port) = control.file_updates()? {
            port.reconcile_deletions()?;
        }
        Ok(())
    }

    fn require_current_control_parent(parent: &File, directory: &Path) -> Result<(), CoreError> {
        let current = File::from(
            open(directory, dir_flags(), Mode::empty())
                .map_err(|error| storage("open current Control parent", error))?,
        );
        let current = current
            .metadata()
            .map_err(|error| storage("stat current Control parent", error))?;
        let held = parent
            .metadata()
            .map_err(|error| storage("stat held Control parent", error))?;
        if current.dev() != held.dev() || current.ino() != held.ino() {
            return Err(invalid("Control parent incarnation changed"));
        }
        Ok(())
    }

    /// Claim physical Control reader custody before opening the anchored FD.
    /// Legacy fixtures without a held physical parent retain their raw path.
    fn open_control_reader(
        &self,
        parent: &File,
        name: &str,
        flags: OFlags,
        optional: bool,
        context: &str,
    ) -> Result<Option<ControlReader>, CoreError> {
        let qualified_updates = if let Some(control) = &self.control_output_admission {
            validate_control_object_lifetime(Some(control), self.object_lifetime.as_ref())?;
            if let Some(lifetime) = &self.object_lifetime {
                lifetime.validate_state_root(&self.state_root)?;
            }
            if control.held_file_allowance()?.is_some() {
                let updates = control.file_updates()?.ok_or_else(|| {
                    invalid("physical Control reader lacks its file update authority")
                })?;
                let directory = if std::ptr::eq(parent, self.pool.as_ref()) {
                    self.state_root.join(POOL_DIR)
                } else if std::ptr::eq(parent, self.attempts.as_ref()) {
                    self.state_root.join(POOL_DIR).join(ATTEMPTS_DIR)
                } else {
                    return Err(invalid("Control reader parent differs from pool"));
                };
                if optional {
                    Self::require_current_control_parent(parent, &directory)?;
                }
                Some((updates, directory))
            } else {
                None
            }
        } else {
            None
        };
        let lifetime_only_path = if qualified_updates.is_none() {
            if let Some(lifetime) = &self.object_lifetime {
                lifetime.validate_state_root(&self.state_root)?;
                let directory = if std::ptr::eq(parent, self.pool.as_ref()) {
                    self.state_root.join(POOL_DIR)
                } else if std::ptr::eq(parent, self.attempts.as_ref()) {
                    self.state_root.join(POOL_DIR).join(ATTEMPTS_DIR)
                } else {
                    return Err(invalid("Control reader parent differs from pool"));
                };
                Self::require_current_control_parent(parent, &directory)?;
                Some(directory.join(name))
            } else {
                None
            }
        } else {
            None
        };
        if optional && qualified_updates.is_none() && lifetime_only_path.is_none() {
            match rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
                Err(rustix::io::Errno::NOENT) => return Ok(None),
                Ok(_) => {}
                Err(error) => return Err(storage(context, error)),
            }
        }
        let ticket = if let Some((updates, directory)) = qualified_updates {
            if let Some(control) = &self.control_output_admission {
                match updates.begin_reader_open(&directory.join(name), control.clone()) {
                    Ok(ticket) => Some(ticket),
                    Err(CoreError::NotFound(_)) if optional => return Ok(None),
                    Err(error) => return Err(error),
                }
            } else {
                return Err(invalid("Control reader admission missing"));
            }
        } else if let Some(path) = lifetime_only_path {
            let lifetime = self
                .object_lifetime
                .as_ref()
                .ok_or_else(|| invalid("Control reader lifetime authority missing"))?;
            match lifetime.begin_control_reader_open(&path) {
                Ok(ticket) => Some(ticket),
                Err(CoreError::NotFound(_)) if optional => return Ok(None),
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let opened = match openat(parent, name, flags, Mode::empty()) {
            Ok(file) => File::from(file),
            Err(rustix::io::Errno::NOENT) if optional => return Ok(None),
            Err(error) => return Err(storage(context, error)),
        };
        match ticket {
            Some(ticket) => ticket.opened_owned(opened).map(ControlReader::admitted),
            None => Ok(ControlReader::unqualified(opened)),
        }
        .map(Some)
    }

    fn existing_regular_gate(&self, parent: &File) -> Result<bool, CoreError> {
        let Some(reader) = self.open_control_reader(
            parent,
            GATE_FILE,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            true,
            "open existing lifecycle gate",
        )?
        else {
            return Ok(false);
        };
        let result = reader.metadata().and_then(|metadata| {
            if metadata.is_file() {
                Ok(true)
            } else {
                Err(corrupt("existing lifecycle gate is not a regular file"))
            }
        });
        finish_control_read(reader, result)
    }

    /// Canonical attempt files use the same retained deletion owner as their
    /// Control writers. Uncommitted temporaries require create reconciliation
    /// and must never enter this path.
    fn remove_attempt_control_locked(
        &self,
        name: &str,
        allow_absent_unqualified: bool,
    ) -> Result<bool, CoreError> {
        if let Some(control) = &self.control_output_admission {
            validate_control_object_lifetime(Some(control), self.object_lifetime.as_ref())?;
            if let Some(port) = control.file_updates()? {
                let path = self.state_root.join(POOL_DIR).join(ATTEMPTS_DIR).join(name);
                port.remove_file_and_sync(&path, control.clone())?;
                // The port also accepts an already absent canonical file. The
                // following shared barrier is harmless in that retry case.
                return Ok(true);
            }
        }
        match unlinkat(self.attempts.as_ref(), name, AtFlags::empty()) {
            Ok(()) => Ok(true),
            Err(rustix::io::Errno::NOENT) if allow_absent_unqualified => Ok(false),
            Err(error) => Err(storage("remove attempt control", error)),
        }
    }

    fn read_attempt_header(&self, name: &str) -> Result<Option<AttemptRecord>, CoreError> {
        let Some(mut file) = self.open_control_reader(
            self.attempts.as_ref(),
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            true,
            "open attempt",
        )?
        else {
            return Ok(None);
        };
        let result = (|| {
            let metadata = file.metadata()?;
            let attempt_bound = u64::try_from(MAX_ATTEMPT_BYTES)
                .map_err(|error| storage("attempt bound width", error))?;
            if !metadata.is_file() || metadata.len() > attempt_bound {
                return Err(corrupt("attempt record is not a bounded regular file"));
            }
            let expected = usize::try_from(metadata.len())
                .map_err(|_conversion_error| corrupt("attempt byte width"))?;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(
                    expected
                        .checked_add(1)
                        .ok_or_else(|| corrupt("attempt length overflow"))?,
                )
                .map_err(|error| storage("reserve attempt bytes", error))?;
            let _read = (&mut file)
                .take(
                    attempt_bound
                        .checked_add(1)
                        .ok_or_else(|| corrupt("attempt bound overflow"))?,
                )
                .read_to_end(&mut bytes)
                .map_err(|error| admitted_io_error("read attempt", &error))?;
            if bytes.len() != expected {
                return Err(corrupt("attempt record changed during read"));
            }
            let record: AttemptRecord = crate::channel_payloads::decode_cbor_exact(&bytes)
                .map_err(|error| corrupt(&format!("decode attempt record: {error}")))?;
            let canonical = crate::channel_payloads::encode_cbor(&record, "lexical attempt")?;
            if canonical != bytes {
                return Err(corrupt("attempt record CBOR is not canonical"));
            }
            if !strictly_sorted(&record.1) {
                return Err(corrupt(
                    "attempt root inventory is not canonical or bounded",
                ));
            }
            if record.0.identity_name() != name {
                return Err(corrupt("attempt identity does not match its file name"));
            }
            Ok(Some(record))
        })();
        finish_control_read(file, result)
    }

    fn open_attempt_log(&self, name: &str) -> Result<ControlReader, CoreError> {
        let head = self.read_journal_head(name)?;
        if head.pending.is_some() {
            return Err(corrupt(
                "attempt journal has an unrecovered pending adoption",
            ));
        }
        let file = self
            .open_control_reader(
                self.attempts.as_ref(),
                &journal_name(name),
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                false,
                "open attempt journal",
            )?
            .ok_or_else(|| corrupt("attempt journal is missing"))?;
        let result = file.metadata().and_then(|metadata| {
            if metadata.is_file()
                && metadata.len() % JOURNAL_ENTRY_BYTES == 0
                && metadata.len() == head.committed_length
            {
                Ok(())
            } else {
                Err(corrupt("attempt journal is not a complete regular file"))
            }
        });
        if let Err(primary) = result {
            return finish_control_read(file, Err(primary));
        }
        Ok(file)
    }

    fn begin_journal_update(
        &self,
        name: &str,
        expected_length: u64,
        target_end: u64,
    ) -> Result<JournalWriter, CoreError> {
        validate_control_object_lifetime(
            self.control_output_admission.as_ref(),
            self.object_lifetime.as_ref(),
        )?;
        // A physical Control parent without an update port refuses before
        // opening a writable FD. A test-only unqualified parent may fall back.
        let updates = self
            .control_output_admission
            .as_ref()
            .map(PublicationControlOutputAdmission::file_updates)
            .transpose()?
            .flatten();
        let journal_name = journal_name(name);
        let file = File::from(
            openat(
                self.attempts.as_ref(),
                journal_name.as_str(),
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(|error| storage("open pending attempt journal for update", error))?,
        );
        if let (Some(updates), Some(control)) = (updates, &self.control_output_admission) {
            let path = self
                .state_root
                .join(POOL_DIR)
                .join(ATTEMPTS_DIR)
                .join(journal_name);
            return updates
                .begin_update(&path, file, expected_length, target_end, control.clone())
                .map(JournalWriter::Admitted);
        }
        let metadata = file
            .metadata()
            .map_err(|error| storage("stat pending attempt journal for update", error))?;
        if !metadata.is_file() || metadata.len() != expected_length {
            return Err(corrupt("pending attempt journal changed before update"));
        }
        Ok(JournalWriter::Unqualified(file))
    }

    fn sync_and_settle_journal(
        &self,
        journal: JournalWriter,
        label: quanta_index_core::DiagnosticSyncLabelV1,
        context: &str,
    ) -> Result<(), CoreError> {
        match journal {
            JournalWriter::Unqualified(file) => crate::causal_profile::observed_sync_legacy(
                self.sync_totals.as_deref(),
                label,
                || file.sync_all(),
            )
            .map_err(|error| storage(context, error)),
            JournalWriter::Admitted(mutation) => {
                let started = self.sync_totals.as_ref().map(|_| Instant::now());
                let result = mutation.sync_and_settle();
                if let (Some(totals), Some(started)) = (self.sync_totals.as_ref(), started) {
                    match u64::try_from(started.elapsed().as_nanos()) {
                        Ok(elapsed_ns) => totals.record(label, Some(elapsed_ns), result.is_ok()),
                        Err(_overflow) => totals.record(label, None, result.is_ok()),
                    }
                }
                result
            }
        }
    }

    fn write_journal_head(&self, name: &str, head: &JournalHead) -> Result<(), CoreError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(154)
            .map_err(|error| storage("reserve attempt journal head", error))?;
        bytes.extend_from_slice(JOURNAL_HEAD_MAGIC);
        bytes.extend_from_slice(&head.committed_length.to_be_bytes());
        bytes.extend_from_slice(&head.committed_chain);
        if let Some(reference) = &head.pending {
            bytes.push(1);
            encode_ref(&mut bytes, reference);
        } else {
            bytes.push(0);
            bytes.extend_from_slice(&[0_u8; 41]);
        }
        bytes.extend_from_slice(&head.expected_chain);
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        bytes.extend_from_slice(&digest);
        self.write_atomic_at(
            self.attempts.as_ref(),
            &journal_head_name(name),
            &bytes,
            AtomicPurpose::AttemptJournal,
        )
    }

    fn read_journal_head(&self, name: &str) -> Result<JournalHead, CoreError> {
        let mut file = self
            .open_control_reader(
                self.attempts.as_ref(),
                &journal_head_name(name),
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                false,
                "open attempt journal head",
            )?
            .ok_or_else(|| corrupt("attempt journal head absent"))?;
        let result = (|| {
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.len() != 154 {
                return Err(corrupt("attempt journal head is not a fixed regular file"));
            }
            let mut bytes = [0_u8; 154];
            file.read_exact(&mut bytes)
                .map_err(|error| admitted_io_error("read attempt journal head", &error))?;
            let committed = bytes
                .get(..122)
                .ok_or_else(|| corrupt("journal head truncated"))?;
            let expected: [u8; 32] = Sha256::digest(committed).into();
            if committed.get(..8) != Some(JOURNAL_HEAD_MAGIC.as_slice())
                || bytes.get(122..) != Some(expected.as_slice())
            {
                return Err(corrupt("attempt journal head digest or magic differs"));
            }
            let length = u64::from_be_bytes(
                committed
                    .get(8..16)
                    .ok_or_else(|| corrupt("journal head length truncated"))?
                    .try_into()
                    .map_err(|_conversion_error| corrupt("journal head length width"))?,
            );
            let committed_chain: [u8; 32] = committed
                .get(16..48)
                .ok_or_else(|| corrupt("journal chain truncated"))?
                .try_into()
                .map_err(|_conversion_error| corrupt("journal chain width"))?;
            if length % JOURNAL_ENTRY_BYTES != 0 {
                return Err(corrupt("attempt journal head length is not aligned"));
            }
            let pending = match committed
                .get(48)
                .copied()
                .ok_or_else(|| corrupt("journal pending tag absent"))?
            {
                0 => {
                    if committed
                        .get(49..90)
                        .is_none_or(|bytes| bytes.iter().any(|byte| *byte != 0))
                    {
                        return Err(corrupt("clean journal head has nonzero pending bytes"));
                    }
                    None
                }
                1 => {
                    let mut offset = 0;
                    Some(decode_ref(
                        committed
                            .get(49..90)
                            .ok_or_else(|| corrupt("pending reference truncated"))?,
                        &mut offset,
                    )?)
                }
                _ => return Err(corrupt("unknown journal pending tag")),
            };
            let expected_chain: [u8; 32] = committed
                .get(90..122)
                .ok_or_else(|| corrupt("expected journal chain truncated"))?
                .try_into()
                .map_err(|_conversion_error| corrupt("expected journal chain width"))?;
            if pending.is_none() && expected_chain != committed_chain {
                return Err(corrupt("clean journal head chains differ"));
            }
            Ok(JournalHead {
                committed_length: length,
                committed_chain,
                pending,
                expected_chain,
            })
        })();
        finish_control_read(file, result)
    }

    fn visit_attempt_log(
        &self,
        name: &str,
        mut visitor: impl FnMut(&ObjectRef) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let mut file = self.open_attempt_log(name)?;
        let result = (|| {
            let length = file.metadata()?.len();
            let mut previous = journal_seed(name);
            let mut entry = [0_u8; 73];
            let entries = length
                .checked_div(JOURNAL_ENTRY_BYTES)
                .ok_or_else(|| corrupt("attempt journal entry width"))?;
            for _ in 0..entries {
                file.read_exact(&mut entry)
                    .map_err(|error| admitted_io_error("read attempt journal", &error))?;
                let mut hasher = Sha256::new();
                hasher.update(b"lexical-attempt-adoption-v1");
                hasher.update(previous);
                let reference_bytes = entry
                    .get(..41)
                    .ok_or_else(|| corrupt("attempt journal reference truncated"))?;
                hasher.update(reference_bytes);
                let expected: [u8; 32] = hasher.finalize().into();
                if entry
                    .get(41..)
                    .ok_or_else(|| corrupt("attempt journal chain absent"))?
                    != expected
                {
                    return Err(corrupt("attempt journal chain differs"));
                }
                previous = expected;
                let mut offset = 0;
                visitor(&decode_ref(reference_bytes, &mut offset)?)?;
            }
            let head = self.read_journal_head(name)?;
            if previous != head.committed_chain {
                return Err(corrupt("attempt journal final chain differs from head"));
            }
            Ok(())
        })();
        finish_control_read(file, result)
    }

    /// Recover only adoptions explicitly recorded by a pending head. Missing
    /// bytes from an earlier completed entry remain corruption.
    fn recover_attempt_log(&self, name: &str) -> Result<(), CoreError> {
        let Some(header) = self.open_control_reader(
            self.attempts.as_ref(),
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            true,
            "probe attempt recovery header",
        )?
        else {
            return Ok(());
        };
        let result = header.metadata().and_then(|metadata| {
            if metadata.is_file() {
                Ok(())
            } else {
                Err(corrupt("attempt recovery header is not regular"))
            }
        });
        finish_control_read(header, result)?;
        let head = self.read_journal_head(name)?;
        let Some(pending) = head.pending.as_ref() else {
            self.open_attempt_log(name)?.retire()?;
            return Ok(());
        };
        let mut journal = self
            .open_control_reader(
                self.attempts.as_ref(),
                &journal_name(name),
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                false,
                "open pending attempt journal",
            )?
            .ok_or_else(|| corrupt("pending attempt journal absent"))?;
        let result = (|| {
            let length = journal.metadata()?.len();
            let pending_references = if pending.kind == ObjectKind::AttemptBatch {
                let (references, payload) = self.read_object(pending, None)?;
                if references.is_empty() || references.len() > MAX_ADOPTION_BATCH_REFS {
                    return Err(corrupt("pending batch reference count differs"));
                }
                let expected_payload = batch_manifest_payload(name, &head, references.len())?;
                if payload != expected_payload {
                    return Err(corrupt("pending batch manifest binding differs"));
                }
                for reference in &references {
                    let _verified = self.read_object(reference, None)?;
                }
                references
            } else {
                vec![pending.clone()]
            };
            let (expected_entry, expected_chain) =
                adoption_entries(head.committed_chain, &pending_references)?;
            if expected_chain != head.expected_chain {
                return Err(corrupt("pending journal head chain is invalid"));
            }
            let expected_length = head
                .committed_length
                .checked_add(
                    u64::try_from(expected_entry.len())
                        .map_err(|error| storage("pending entry width", error))?,
                )
                .ok_or_else(|| corrupt("pending journal length overflow"))?;
            if length < head.committed_length || length > expected_length {
                return Err(corrupt("journal lost a completed entry or has extra bytes"));
            }
            let _offset = journal
                .seek(SeekFrom::Start(0))
                .map_err(|error| admitted_io_error("seek journal prefix", &error))?;
            let mut previous = journal_seed(name);
            let mut entry = [0_u8; 73];
            let prefix_entries = head
                .committed_length
                .checked_div(JOURNAL_ENTRY_BYTES)
                .ok_or_else(|| corrupt("journal prefix entry width"))?;
            for _ in 0..prefix_entries {
                journal
                    .read_exact(&mut entry)
                    .map_err(|error| admitted_io_error("read journal prefix", &error))?;
                let mut hasher = Sha256::new();
                hasher.update(b"lexical-attempt-adoption-v1");
                hasher.update(previous);
                hasher.update(
                    entry
                        .get(..41)
                        .ok_or_else(|| corrupt("prefix entry truncated"))?,
                );
                let expected: [u8; 32] = hasher.finalize().into();
                if entry.get(41..) != Some(expected.as_slice()) {
                    return Err(corrupt("completed journal prefix chain differs"));
                }
                previous = expected;
            }
            if previous != head.committed_chain {
                return Err(corrupt("completed journal prefix differs from head"));
            }
            let tail_len = usize::try_from(length.saturating_sub(head.committed_length))
                .map_err(|_conversion_error| corrupt("pending journal tail width"))?;
            let mut observed_tail = Vec::new();
            observed_tail
                .try_reserve_exact(tail_len)
                .map_err(|error| storage("reserve journal tail", error))?;
            let _read = journal
                .read_to_end(&mut observed_tail)
                .map_err(|error| admitted_io_error("read pending journal tail", &error))?;
            if observed_tail
                != expected_entry
                    .get(..tail_len)
                    .ok_or_else(|| corrupt("pending tail exceeds entry"))?
            {
                return Err(corrupt("pending journal tail differs from head"));
            }
            Ok((length, expected_length, expected_entry, expected_chain))
        })();
        let (length, expected_length, expected_entry, expected_chain) =
            finish_control_read(journal, result)?;
        let mut journal = self.begin_journal_update(name, length, expected_length)?;
        let repair = (|| {
            if length != expected_length {
                journal.set_len(head.committed_length)?;
                let _offset = journal
                    .seek(SeekFrom::End(0))
                    .map_err(|error| admitted_io_error("seek repaired journal end", &error))?;
                journal
                    .write_all(&expected_entry)
                    .map_err(|error| admitted_io_error("restore pending adoption", &error))?;
            }
            Ok::<(), CoreError>(())
        })();
        if let Err(primary) = repair {
            return Err(retire_failed_journal(journal, primary));
        }
        // A complete pending entry may have been written before a crash but
        // never synced. Make the log durable before clearing the pending head.
        self.sync_and_settle_journal(
            journal,
            quanta_index_core::DiagnosticSyncLabelV1::PoolAdoptionJournalFull,
            "sync restored adoption",
        )?;
        self.write_journal_head(name, &JournalHead::clean(expected_length, expected_chain))
    }

    fn write_attempt(&self, name: &str, record: &AttemptRecord) -> Result<(), CoreError> {
        let bytes = crate::channel_payloads::encode_cbor(record, "lexical attempt")?;
        if bytes.len() > MAX_ATTEMPT_BYTES {
            return Err(invalid("attempt record exceeds custody byte bound"));
        }
        self.write_atomic_at(self.attempts.as_ref(), name, &bytes, AtomicPurpose::Other)
    }

    fn write_retirement_marker(&self, name: &str, record: &AttemptRecord) -> Result<(), CoreError> {
        let header = crate::channel_payloads::encode_cbor(record, "lexical attempt")?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(72)
            .map_err(|error| storage("reserve retirement marker", error))?;
        bytes.extend_from_slice(RETIREMENT_MAGIC);
        bytes.extend_from_slice(&<[u8; 32]>::from(Sha256::digest(&header)));
        bytes.extend_from_slice(&<[u8; 32]>::from(Sha256::digest(&bytes)));
        self.write_atomic_at(
            self.attempts.as_ref(),
            &retirement_name(name),
            &bytes,
            AtomicPurpose::Other,
        )
    }

    fn read_retirement_marker(&self, name: &str) -> Result<Option<[u8; 32]>, CoreError> {
        let Some(mut file) = self.open_control_reader(
            self.attempts.as_ref(),
            &retirement_name(name),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            true,
            "open retirement marker",
        )?
        else {
            return Ok(None);
        };
        let result = (|| {
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.len() != 72 {
                return Err(corrupt("retirement marker is not a fixed regular file"));
            }
            let mut bytes = [0_u8; 72];
            file.read_exact(&mut bytes)
                .map_err(|error| admitted_io_error("read retirement marker", &error))?;
            let body = bytes
                .get(..40)
                .ok_or_else(|| corrupt("retirement marker truncated"))?;
            let digest: [u8; 32] = Sha256::digest(body).into();
            if body.get(..8) != Some(RETIREMENT_MAGIC.as_slice())
                || bytes.get(40..) != Some(digest.as_slice())
            {
                return Err(corrupt("retirement marker digest or magic differs"));
            }
            Ok(Some(
                body.get(8..40)
                    .ok_or_else(|| corrupt("retirement header digest absent"))?
                    .try_into()
                    .map_err(|_conversion_error| corrupt("retirement header digest width"))?,
            ))
        })();
        finish_control_read(file, result)
    }

    /// Complete a previously authorized retirement. The marker is durable
    /// before any of the mutable attempt files are unlinked.
    fn recover_retirement_locked(&self, name: &str) -> Result<(), CoreError> {
        // A previous unlink may have removed even the marker before its
        // deletion ticket settled, so drain before probing any pathname.
        self.reconcile_control_deletions()?;
        let Some(expected_header) = self.read_retirement_marker(name)? else {
            return Ok(());
        };
        if let Some(record) = self.read_attempt_header(name)? {
            let header = crate::channel_payloads::encode_cbor(&record, "lexical attempt")?;
            if <[u8; 32]>::from(Sha256::digest(&header)) != expected_header {
                return Err(corrupt("retiring attempt header differs from marker"));
            }
        }
        let _epoch = self.bump_epoch_locked()?;
        for component in [
            name.to_owned(),
            journal_name(name),
            journal_head_name(name),
            hard_scan_checkpoint::sidecar_name(name),
        ] {
            let _removed = self.remove_attempt_control_locked(&component, true)?;
        }
        self.attempts
            .sync_all()
            .map_err(|error| storage("sync recovered attempt retirement", error))?;
        let _removed = self.remove_attempt_control_locked(&retirement_name(name), false)?;
        self.attempts
            .sync_all()
            .map_err(|error| storage("sync retirement marker removal", error))
    }

    fn write_atomic_at(
        &self,
        parent: &File,
        name: &str,
        bytes: &[u8],
        purpose: AtomicPurpose,
    ) -> Result<(), CoreError> {
        validate_control_object_lifetime(
            self.control_output_admission.as_ref(),
            self.object_lifetime.as_ref(),
        )?;
        let output = self
            .control_output_admission
            .as_ref()
            .map(|admission| {
                admission.reserve(PublicationOutputReservation::new(
                    PublicationOutputClass::Control,
                    u64::try_from(bytes.len())
                        .map_err(|error| storage("control output width", error))?
                        .max(1),
                    1,
                )?)
            })
            .transpose()?;
        self.write_atomic_at_inner(
            parent,
            name,
            bytes,
            purpose,
            AtomicSync::Full,
            AtomicWriteAdmission { pool: None, output },
        )
    }

    fn write_atomic_at_inner(
        &self,
        parent: &File,
        name: &str,
        bytes: &[u8],
        purpose: AtomicPurpose,
        sync: AtomicSync,
        admission: AtomicWriteAdmission,
    ) -> Result<(), CoreError> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let AtomicWriteAdmission {
            pool: mut permit,
            mut output,
        } = admission;
        if output.is_some() && !matches!(sync, AtomicSync::Full) {
            return Err(invalid("admitted output requires full object sync"));
        }
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = format!(".{name}.tmp-{}-{sequence}", std::process::id());
        let encoded_width = u64::try_from(bytes.len())
            .map_err(|_error| invalid("encoded object width exceeds u64"))?;
        if let Some(permit) = &permit {
            permit.check_new_file_admitted()?;
        }
        let output_paths = if output.is_some() {
            let directory = match purpose {
                AtomicPurpose::PoolObject => {
                    let prefix = name
                        .get(..2)
                        .ok_or_else(|| invalid("object digest prefix"))?;
                    self.state_root
                        .join(POOL_DIR)
                        .join(OBJECTS_DIR)
                        .join(prefix)
                }
                AtomicPurpose::Other
                | AtomicPurpose::AttemptJournal
                | AtomicPurpose::HardScanCheckpoint => {
                    if std::ptr::eq(parent, self.pool.as_ref()) {
                        self.state_root.join(POOL_DIR)
                    } else if std::ptr::eq(parent, self.attempts.as_ref()) {
                        self.state_root.join(POOL_DIR).join(ATTEMPTS_DIR)
                    } else {
                        return Err(invalid("admitted control parent differs from pool"));
                    }
                }
            };
            Some((directory.join(&temp), directory.join(name)))
        } else {
            None
        };
        if let Some(output) = &mut output {
            let (staging, target) = output_paths
                .as_ref()
                .ok_or_else(|| invalid("admitted object paths missing"))?;
            output.bind_output(staging, target)?;
            output.creation_started()?;
        }
        let opened = openat(
            parent,
            temp.as_str(),
            OFlags::WRONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::CREATE | OFlags::EXCL,
            Mode::RUSR | Mode::WUSR,
        );
        let mut file = match opened {
            Ok(file) => File::from(file),
            Err(error) => {
                let mut primary = storage("create object temporary", error);
                if let Some(output) = &mut output {
                    let absence = (|| {
                        match rustix::fs::statat(parent, temp.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
                            Err(rustix::io::Errno::NOENT) => {}
                            Ok(_) => return Err(invalid("failed-create temporary is occupied")),
                            Err(inspect) => {
                                return Err(storage("inspect failed-create temporary", inspect));
                            }
                        }
                        parent
                            .sync_all()
                            .map_err(|sync| storage("sync failed-create parent", sync))?;
                        output.confirmed_uncreated_absent_and_synced()
                    })();
                    if let Err(cleanup) = absence {
                        primary = with_custody_context(primary, cleanup);
                    }
                }
                return Err(primary);
            }
        };
        let mut published = false;
        let mut created_confirmed = false;
        let mut gate_conflict = false;
        let operation = (|| {
            if let Some(output) = &mut output {
                let (staging, _target) = output_paths
                    .as_ref()
                    .ok_or_else(|| invalid("admitted object paths missing"))?;
                output.created(&file, staging)?;
                created_confirmed = true;
            }
            if let Some(permit) = &mut permit {
                permit.temp_created(self.pool_temp_identity(&temp))?;
            }
            #[cfg(test)]
            reach_atomic_cut(purpose, 0, crate::publication_faults::Side::Before)?;
            #[cfg(test)]
            let write = if matches!(purpose, AtomicPurpose::PoolObject) {
                crate::publication_faults::write_pool_object(&mut file, bytes)
            } else {
                file.write_all(bytes)
            };
            #[cfg(not(test))]
            let write = file.write_all(bytes);
            write.map_err(|error| storage("write object temporary", error))?;
            #[cfg(test)]
            reach_atomic_cut(purpose, 0, crate::publication_faults::Side::After)?;
            #[cfg(test)]
            reach_atomic_cut(purpose, 1, crate::publication_faults::Side::Before)?;
            let batched_data = cfg!(target_vendor = "apple")
                && matches!(purpose, AtomicPurpose::PoolObject)
                && matches!(sync, AtomicSync::BatchedPoolData);
            if batched_data {
                crate::causal_profile::observed_sync_legacy(
                    self.sync_totals.as_deref(),
                    quanta_index_core::DiagnosticSyncLabelV1::PoolDataFilePosix,
                    || {
                        #[cfg(test)]
                        crate::publication_faults::reach(
                            crate::publication_faults::Cut::PoolObjectFileSyncCall,
                            crate::publication_faults::Side::Before,
                        )?;
                        fsync(&file).map_err(std::io::Error::from)
                    },
                )
                .map_err(|error| storage("sync batched object temporary", error))?;
            } else {
                let label = match purpose {
                    AtomicPurpose::PoolObject => {
                        quanta_index_core::DiagnosticSyncLabelV1::PoolObjectFileFull
                    }
                    AtomicPurpose::AttemptJournal
                    | AtomicPurpose::HardScanCheckpoint
                    | AtomicPurpose::Other => {
                        quanta_index_core::DiagnosticSyncLabelV1::PoolControlFileFull
                    }
                };
                crate::causal_profile::observed_sync_legacy(
                    self.sync_totals.as_deref(),
                    label,
                    || {
                        #[cfg(test)]
                        if matches!(purpose, AtomicPurpose::PoolObject) {
                            crate::publication_faults::reach(
                                crate::publication_faults::Cut::PoolObjectFileSyncCall,
                                crate::publication_faults::Side::Before,
                            )?;
                        }
                        file.sync_all()
                    },
                )
                .map_err(|error| storage("sync object temporary", error))?;
            }
            #[cfg(test)]
            reach_atomic_cut(purpose, 1, crate::publication_faults::Side::After)?;
            #[cfg(test)]
            reach_atomic_cut(purpose, 2, crate::publication_faults::Side::Before)?;
            if let Some(output) = &mut output {
                output.before_replace()?;
                if matches!(purpose, AtomicPurpose::PoolObject) || name == GATE_FILE {
                    let rename = rustix::fs::renameat_with(
                        parent,
                        temp.as_str(),
                        parent,
                        name,
                        rustix::fs::RenameFlags::NOREPLACE,
                    );
                    if name == GATE_FILE && matches!(rename, Err(rustix::io::Errno::EXIST)) {
                        gate_conflict = true;
                    }
                    rename.map_err(|error| {
                        storage(
                            if name == GATE_FILE {
                                "publish lifecycle gate"
                            } else {
                                "publish object"
                            },
                            error,
                        )
                    })?;
                } else {
                    renameat(parent, temp.as_str(), parent, name)
                        .map_err(|error| storage("publish control", error))?;
                }
            } else {
                renameat(parent, temp.as_str(), parent, name)
                    .map_err(|error| storage("publish object", error))?;
            }
            published = true;
            if let Some(permit) = &mut permit {
                permit.canonical_renamed()?;
            }
            #[cfg(test)]
            reach_atomic_cut(purpose, 2, crate::publication_faults::Side::After)?;
            #[cfg(test)]
            reach_atomic_cut(purpose, 3, crate::publication_faults::Side::Before)?;
            if batched_data {
                crate::causal_profile::observed_sync_legacy(
                    self.sync_totals.as_deref(),
                    quanta_index_core::DiagnosticSyncLabelV1::PoolDataShardPosix,
                    || fsync(parent).map_err(std::io::Error::from),
                )
                .map_err(|error| storage("sync batched object directory", error))?;
            } else {
                let label = match purpose {
                    AtomicPurpose::PoolObject => {
                        quanta_index_core::DiagnosticSyncLabelV1::PoolObjectShardFull
                    }
                    AtomicPurpose::AttemptJournal
                    | AtomicPurpose::HardScanCheckpoint
                    | AtomicPurpose::Other => {
                        quanta_index_core::DiagnosticSyncLabelV1::PoolControlParentFull
                    }
                };
                crate::causal_profile::observed_sync_legacy(
                    self.sync_totals.as_deref(),
                    label,
                    || parent.sync_all(),
                )
                .map_err(|error| storage("sync object directory", error))?;
            }
            #[cfg(test)]
            reach_atomic_cut(purpose, 3, crate::publication_faults::Side::After)?;
            #[cfg(not(test))]
            let _: AtomicPurpose = purpose;
            Ok::<(), CoreError>(())
        })();
        match operation {
            Ok(()) => {
                if let Some(admitted) = &mut output {
                    admitted.committed(file, encoded_width)?;
                }
                Ok(())
            }
            Err(error) => {
                let mut primary = error;
                if let Some(admitted) = &mut output {
                    let absence = if published {
                        None
                    } else {
                        Some((|| {
                            match unlinkat(parent, temp.as_str(), AtFlags::empty()) {
                                Ok(()) | Err(rustix::io::Errno::NOENT) => {}
                                Err(unlink) => {
                                    return Err(storage("unlink failed object temporary", unlink));
                                }
                            }
                            parent
                                .sync_all()
                                .map_err(|sync| storage("sync failed object cleanup", sync))
                        })())
                    };
                    let absent_and_synced = match absence {
                        Some(Ok(())) => true,
                        Some(Err(cleanup)) => {
                            primary = with_custody_context(primary, cleanup);
                            false
                        }
                        None => false,
                    };
                    let retirement = if absent_and_synced && created_confirmed {
                        admitted.confirmed_safe_absent_after_writer_close(file)
                    } else {
                        admitted.retire_writer_preserving_pending(file)
                    };
                    let retired = retirement.is_ok();
                    if let Err(retirement) = retirement {
                        primary = with_custody_context(primary, retirement);
                    }
                    let mut cleanup_complete = absent_and_synced && created_confirmed && retired;
                    if cleanup_complete
                        && let Some(permit) = &mut permit
                        && let Err(cleanup) = permit.temp_unlinked_and_synced()
                    {
                        primary = with_custody_context(primary, cleanup);
                        cleanup_complete = false;
                    }
                    if gate_conflict && cleanup_complete {
                        // Release the settled creation's target claim before
                        // admitting a reader for the competing canonical gate.
                        drop(output.take());
                        match self.existing_regular_gate(parent) {
                            Ok(true) => return Ok(()),
                            Ok(false) => {
                                primary = with_custody_context(
                                    primary,
                                    corrupt("competing lifecycle gate disappeared"),
                                );
                            }
                            Err(inspection) => {
                                primary = with_custody_context(primary, inspection);
                            }
                        }
                    }
                }
                Err(primary)
            }
        }
    }

    fn object_dir(&self, digest: &[u8; 32], create: bool) -> Result<File, CoreError> {
        let hex = hex_digest(digest);
        let prefix = hex
            .get(..2)
            .ok_or_else(|| invalid("object digest prefix"))?;
        let directory = if create {
            ensure_dir(self.objects.as_ref(), prefix, self.sync_totals.as_deref())?
        } else {
            File::from(
                openat(self.objects.as_ref(), prefix, dir_flags(), Mode::empty()).map_err(
                    |error| match error {
                        rustix::io::Errno::NOENT
                        | rustix::io::Errno::LOOP
                        | rustix::io::Errno::NOTDIR => {
                            corrupt("referenced object shard is missing or not a real directory")
                        }
                        _ => storage("open object shard", error),
                    },
                )?,
            )
        };
        self.check_object_shard_device(&directory)?;
        Ok(directory)
    }

    fn open_object_reader(
        &self,
        directory: &File,
        digest: &[u8; 32],
    ) -> Result<ObjectReader, CoreError> {
        let name = hex_digest(digest);
        let opening = self
            .object_lifetime
            .as_ref()
            .map(|lifetime| {
                let prefix = name
                    .get(..2)
                    .ok_or_else(|| invalid("object digest prefix"))?;
                let path = self
                    .state_root
                    .join(POOL_DIR)
                    .join(OBJECTS_DIR)
                    .join(prefix)
                    .join(&name);
                lifetime
                    .begin_reader_open(&path)
                    .map_err(|error| match error {
                        CoreError::NotFound(_) => {
                            corrupt("referenced object is missing or not a real file")
                        }
                        other @ (CoreError::InvalidContract(_)
                        | CoreError::Typed { .. }
                        | CoreError::NotReady(_)
                        | CoreError::NotImplemented(_)
                        | CoreError::Storage(_)
                        | CoreError::StorageIo { .. }) => other,
                    })
            })
            .transpose()?;
        let actual = File::from(
            openat(
                directory,
                name.as_str(),
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(|error| match error {
                rustix::io::Errno::NOENT | rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR => {
                    corrupt("referenced object is missing or not a real file")
                }
                _ => storage("open referenced object", error),
            })?,
        );
        match opening {
            Some(opening) => opening.opened_owned(actual).map(ObjectReader::Admitted),
            None => Ok(ObjectReader::Unqualified(actual)),
        }
    }

    fn check_object_shard_device(&self, directory: &File) -> Result<(), CoreError> {
        let observed_device = directory
            .metadata()
            .map_err(|error| storage("stat object shard device", error))?
            .dev();
        self.check_object_device(observed_device)
    }

    fn check_object_device(&self, observed_device: u64) -> Result<(), CoreError> {
        if observed_device != self.objects_device {
            return Err(corrupt("object shard crosses object pool device"));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn put_object_locked(
        &self,
        kind: ObjectKind,
        dependencies: &[ObjectRef],
        payload: &[u8],
    ) -> Result<ObjectRef, CoreError> {
        self.put_object_locked_with_status(kind, dependencies, payload)
            .map(|(reference, _new)| reference)
    }

    fn put_object_locked_with_status(
        &self,
        kind: ObjectKind,
        dependencies: &[ObjectRef],
        payload: &[u8],
    ) -> Result<(ObjectRef, bool), CoreError> {
        let prepared = PreparedObject::new(kind, dependencies, payload)?;
        self.put_prepared_object_locked_with_status(&prepared, dependencies)
    }

    fn put_prepared_object_locked_with_status(
        &self,
        prepared: &PreparedObject,
        dependencies: &[ObjectRef],
    ) -> Result<(ObjectRef, bool), CoreError> {
        let reference = &prepared.reference;
        validate_object_lifetime(
            self.work_output_admission.as_ref(),
            self.object_lifetime.as_ref(),
        )?;
        if let Some(lifetime) = &self.object_lifetime {
            lifetime.validate_state_root(&self.state_root)?;
        }
        self.reconcile_pending_pool_unlinks_locked()?;
        let mut permit = self
            .pool_write_admission
            .as_ref()
            .map(|admission| admission.try_reserve(reference.sha256, reference.bytes))
            .transpose()?;
        let directory = self.object_dir(&reference.sha256, true)?;
        let name = hex_digest(&reference.sha256);
        match rustix::fs::statat(&directory, name.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_present) => {
                let (existing, _) = self.read_object_for_adoption(reference)?;
                if existing != canonical_deps(dependencies)? {
                    return Err(corrupt("existing digest object has different dependencies"));
                }
                if let Some(totals) = self.sync_totals.as_deref() {
                    totals.record_reused_object(diagnostic_object_kind(reference.kind));
                }
                if let Some(permit) = &mut permit {
                    permit.reused_verified()?;
                }
                return Ok((reference.clone(), false));
            }
            Err(rustix::io::Errno::NOENT) => {}
            Err(error) => return Err(storage("probe referenced object", error)),
        }
        let output = self
            .work_output_admission
            .as_ref()
            .map(|admission| {
                admission.reserve(PublicationOutputReservation::new(
                    PublicationOutputClass::Lexical,
                    reference.bytes,
                    1,
                )?)
            })
            .transpose()?;
        // A child may settle only after the shard's full durability barrier;
        // the unqualified pool keeps its existing batched data-sync path.
        let sync = if output.is_none()
            && self.adoption_batch.is_some()
            && reference.kind != ObjectKind::AttemptBatch
        {
            AtomicSync::BatchedPoolData
        } else {
            AtomicSync::Full
        };
        self.write_atomic_at_inner(
            &directory,
            &name,
            &prepared.encoded,
            AtomicPurpose::PoolObject,
            sync,
            AtomicWriteAdmission {
                pool: permit,
                output,
            },
        )?;
        let _verified = self.read_object(reference, None)?;
        if let Some(totals) = self.sync_totals.as_deref() {
            totals.record_new_object(diagnostic_object_kind(reference.kind));
        }
        Ok((reference.clone(), true))
    }

    pub(super) fn record_map_page_mutation(&self) {
        if let Some(totals) = self.sync_totals.as_deref() {
            totals.record_map_page_mutation();
        }
    }

    /// Publish one physical object and adopt it before releasing the gate.
    pub(super) fn put_adopted_object(
        &self,
        lease: &AttemptLease,
        kind: ObjectKind,
        dependencies: &[ObjectRef],
        payload: &[u8],
    ) -> Result<ObjectRef, CoreError> {
        lease.ensure_store(self)?;
        if let Some(batch) = &self.adoption_batch {
            let mut state = lock_batch_state(batch)?;
            self.check_batch_lease(&state, lease)?;
            // Flush before crossing either physical bound. Existing digest
            // objects conservatively trigger a flush near the byte bound.
            let prepared = PreparedObject::new(kind, dependencies, payload)?;
            let encoded_bytes = prepared.reference.bytes;
            if state.references.len() == MAX_ADOPTION_BATCH_REFS
                || state.encoded_new_bytes.saturating_add(encoded_bytes)
                    > MAX_ADOPTION_BATCH_DATA_BYTES
            {
                let reason = if state.references.len() == MAX_ADOPTION_BATCH_REFS {
                    DiagnosticAdoptionFlushReasonV1::RefLimit
                } else {
                    DiagnosticAdoptionFlushReasonV1::ByteLimit
                };
                self.flush_adoption_batch_locked(&mut state, reason)?;
            }
            self.start_batch_locked(&mut state)?;
            let (reference, newly_written) =
                self.put_prepared_object_locked_with_status(&prepared, dependencies)?;
            lease.record_write_cost(&reference, newly_written)?;
            self.queue_batch_reference(
                &mut state,
                &reference,
                newly_written.then_some(encoded_bytes),
            )?;
            self.record_posting_mutation(kind);
            drop(state);
            return Ok(reference);
        }
        let _gate = self.lock_gate()?;
        let (reference, newly_written) =
            self.put_object_locked_with_status(kind, dependencies, payload)?;
        lease.record_write_cost(&reference, newly_written)?;
        lease.adopt_locked(&reference)?;
        self.record_posting_mutation(kind);
        Ok(reference)
    }

    fn record_posting_mutation(&self, kind: ObjectKind) {
        if matches!(
            kind,
            ObjectKind::FilePostings | ObjectKind::NativeTrigrams | ObjectKind::NativePositions
        ) && let Some(totals) = self.sync_totals.as_deref()
        {
            totals.record_posting_mutation();
        }
    }

    pub(super) fn adopt_existing_root(
        &self,
        lease: &AttemptLease,
        reference: &ObjectRef,
    ) -> Result<(), CoreError> {
        lease.ensure_store(self)?;
        if let Some(batch) = &self.adoption_batch {
            let mut state = lock_batch_state(batch)?;
            self.check_batch_lease(&state, lease)?;
            if state.references.len() == MAX_ADOPTION_BATCH_REFS {
                self.flush_adoption_batch_locked(
                    &mut state,
                    DiagnosticAdoptionFlushReasonV1::RefLimit,
                )?;
            }
            self.start_batch_locked(&mut state)?;
            let _verified = self.read_object_for_adoption(reference)?;
            self.queue_batch_reference(&mut state, reference, None)?;
            drop(state);
            return Ok(());
        }
        let _gate = self.lock_gate()?;
        let _verified = self.read_object_for_adoption(reference)?;
        lease.adopt_locked(reference)
    }

    fn read_object_for_adoption(
        &self,
        reference: &ObjectRef,
    ) -> Result<(Vec<ObjectRef>, Vec<u8>), CoreError> {
        self.read_object_inner(reference, None, true, None)
    }

    pub(super) fn cached_decoded_map_page(
        &self,
        descriptor: &PageRef,
    ) -> Result<Option<Arc<Page>>, CoreError> {
        self.read_meter.as_ref().map_or(Ok(None), |meter| {
            meter
                .objects_cache
                .lock()
                .map_err(|error| storage("lock request object cache", error))?
                .get_decoded(descriptor)
        })
    }

    pub(super) fn remember_decoded_map_page(
        &self,
        descriptor: &PageRef,
        page: &Arc<Page>,
    ) -> Result<(), CoreError> {
        if let Some(meter) = &self.read_meter {
            meter
                .objects_cache
                .lock()
                .map_err(|error| storage("lock request object cache", error))?
                .attach_decoded(descriptor, page)?;
        }
        Ok(())
    }

    pub(super) fn read_object(
        &self,
        reference: &ObjectRef,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(Vec<ObjectRef>, Vec<u8>), CoreError> {
        if request_cache_eligible(reference.kind)
            && let Some(meter) = &self.read_meter
        {
            checkpoint(budget, "lexical:object:read")?;
            let cached = meter
                .objects_cache
                .lock()
                .map_err(|error| storage("lock request object cache", error))?
                .get(reference)?;
            if let Some(page) = cached {
                return Ok(page);
            }
        }
        let result = self.read_object_inner(reference, budget, false, None);
        if let Ok((dependencies, payload)) = &result
            && request_cache_eligible(reference.kind)
            && let Some(meter) = &self.read_meter
        {
            meter
                .objects_cache
                .lock()
                .map_err(|error| storage("lock request object cache", error))?
                .insert(reference, dependencies, payload)?;
        }
        if matches!(
            &result,
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ) && let Some(meter) = &self.read_meter
        {
            let mut fault = meter
                .fault
                .lock()
                .map_err(|error| storage("record scrub object fault", error))?;
            if fault.is_none() {
                *fault = Some(reference.clone());
            }
        }
        result
    }

    fn read_object_inner(
        &self,
        reference: &ObjectRef,
        budget: Option<&RequestBudgetV1>,
        repair_for_adoption: bool,
        mut closure_totals: Option<&mut crate::causal_profile::ClosureReadTotalsV1>,
    ) -> Result<(Vec<ObjectRef>, Vec<u8>), CoreError> {
        checkpoint(budget, "lexical:object:read")?;
        if reference.bytes
            > u64::try_from(MAX_OBJECT_BYTES)
                .map_err(|error| storage("object bound width", error))?
        {
            return Err(corrupt("object reference exceeds encoded byte bound"));
        }
        let directory = self.object_dir(&reference.sha256, false)?;
        let mut file = self.open_object_reader(&directory, &reference.sha256)?;
        let result = (|| {
            let metadata = file.metadata()?;
            if !metadata.is_file()
                || metadata.len() != reference.bytes
                || metadata.dev() != self.objects_device
            {
                return Err(corrupt(
                    "referenced object is missing its committed regular-file length",
                ));
            }
            let mut bytes = Vec::new();
            let expected_len = usize::try_from(reference.bytes)
                .map_err(|_conversion_error| corrupt("object byte width"))?;
            bytes
                .try_reserve_exact(
                    expected_len
                        .checked_add(1)
                        .ok_or_else(|| corrupt("object byte bound overflow"))?,
                )
                .map_err(|error| storage("reserve object bytes", error))?;
            let read_started = closure_totals.as_ref().map(|_| Instant::now());
            let read = (&mut file)
                .take(
                    reference
                        .bytes
                        .checked_add(1)
                        .ok_or_else(|| corrupt("object byte bound overflow"))?,
                )
                .read_to_end(&mut bytes);
            if let Some(totals) = closure_totals.as_deref_mut() {
                crate::causal_profile::ClosureReadTotalsV1::accumulate(
                    &mut totals.objects_read,
                    Some(1),
                    &mut totals.overflowed,
                    crate::causal_profile::ClosureReadTotalsV1::OBJECTS_READ,
                );
                crate::causal_profile::ClosureReadTotalsV1::accumulate_width(
                    &mut totals.bytes_read,
                    bytes.len(),
                    &mut totals.overflowed,
                    crate::causal_profile::ClosureReadTotalsV1::BYTES_READ,
                );
                crate::causal_profile::ClosureReadTotalsV1::accumulate_elapsed(
                    &mut totals.read_elapsed_ns,
                    read_started,
                    &mut totals.overflowed,
                    crate::causal_profile::ClosureReadTotalsV1::READ_ELAPSED,
                );
            }
            let _read = read.map_err(|error| storage("read referenced object", error))?;
            if let Some(meter) = &self.read_meter {
                let charged = u64::try_from(bytes.len())
                    .map_err(|_conversion_error| invalid("scrub read byte width overflow"))?;
                let _old = meter
                    .bytes
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                        value.checked_add(charged)
                    })
                    .map_err(|_conversion_error| invalid("scrub object byte meter overflow"))?;
                let _old = meter
                    .objects
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                        value.checked_add(1)
                    })
                    .map_err(|_conversion_error| invalid("scrub object read meter overflow"))?;
            }
            if bytes.len() != expected_len {
                return Err(corrupt(
                    "referenced object differs from committed digest or length",
                ));
            }
            let hash_started = closure_totals.as_ref().map(|_| Instant::now());
            let digest: [u8; 32] = Sha256::digest(&bytes).into();
            if let Some(totals) = closure_totals.as_deref_mut() {
                crate::causal_profile::ClosureReadTotalsV1::accumulate(
                    &mut totals.objects_hashed,
                    Some(1),
                    &mut totals.overflowed,
                    crate::causal_profile::ClosureReadTotalsV1::OBJECTS_HASHED,
                );
                crate::causal_profile::ClosureReadTotalsV1::accumulate_width(
                    &mut totals.bytes_hashed,
                    bytes.len(),
                    &mut totals.overflowed,
                    crate::causal_profile::ClosureReadTotalsV1::BYTES_HASHED,
                );
                crate::causal_profile::ClosureReadTotalsV1::accumulate_elapsed(
                    &mut totals.hash_elapsed_ns,
                    hash_started,
                    &mut totals.overflowed,
                    crate::causal_profile::ClosureReadTotalsV1::HASH_ELAPSED,
                );
            }
            if digest != reference.sha256 {
                return Err(corrupt(
                    "referenced object differs from committed digest or length",
                ));
            }
            let decode_started = closure_totals.as_ref().map(|_| Instant::now());
            let result = decode_object(&bytes);
            if let Some(totals) = closure_totals {
                crate::causal_profile::ClosureReadTotalsV1::accumulate(
                    &mut totals.objects_decoded,
                    Some(1),
                    &mut totals.overflowed,
                    crate::causal_profile::ClosureReadTotalsV1::OBJECTS_DECODED,
                );
                crate::causal_profile::ClosureReadTotalsV1::accumulate_width(
                    &mut totals.bytes_decoded,
                    bytes.len(),
                    &mut totals.overflowed,
                    crate::causal_profile::ClosureReadTotalsV1::BYTES_DECODED,
                );
                crate::causal_profile::ClosureReadTotalsV1::accumulate_elapsed(
                    &mut totals.decode_elapsed_ns,
                    decode_started,
                    &mut totals.overflowed,
                    crate::causal_profile::ClosureReadTotalsV1::DECODE_ELAPSED,
                );
            }
            let result = result?;
            if result.0 != reference.kind {
                return Err(corrupt("referenced object kind differs"));
            }
            if repair_for_adoption {
                // A prior attempt may have failed after rename and before syncing
                // the shard. Repair that host-side state on the authenticated FD.
                crate::causal_profile::observed_sync_legacy(
                    self.sync_totals.as_deref(),
                    quanta_index_core::DiagnosticSyncLabelV1::PoolReusedFilePosix,
                    || file.sync_posix().map_err(std::io::Error::other),
                )
                .map_err(|error| storage("sync adopted object", error))?;
                crate::causal_profile::observed_sync_legacy(
                    self.sync_totals.as_deref(),
                    quanta_index_core::DiagnosticSyncLabelV1::PoolReusedShardPosix,
                    || fsync(&directory).map_err(std::io::Error::from),
                )
                .map_err(|error| storage("sync adopted object shard", error))?;
            }
            checkpoint(budget, "lexical:object:read")?;
            Ok((result.1, result.2))
        })();
        match (result, file.retire()) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(retirement)) => Err(retirement),
            (Err(primary), Err(retirement)) => Err(with_custody_context(primary, retirement)),
        }
    }

    /// Returns the authenticated dependency inventory of one object.
    pub(crate) fn object_dependencies(
        &self,
        reference: &ObjectRef,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Vec<ObjectRef>, CoreError> {
        let (dependencies, _payload) = self.read_object(reference, budget)?;
        Ok(dependencies)
    }

    pub(crate) fn object_dependencies_for_closure(
        &self,
        reference: &ObjectRef,
        totals: &mut crate::causal_profile::ClosureReadTotalsV1,
    ) -> Result<Vec<ObjectRef>, CoreError> {
        let (dependencies, _payload) =
            self.read_object_inner(reference, None, false, Some(totals))?;
        Ok(dependencies)
    }

    pub(crate) fn put_blob(
        &self,
        lease: &AttemptLease,
        kind: ObjectKind,
        dependencies: &[ObjectRef],
        bytes: &[u8],
    ) -> Result<BlobRef, CoreError> {
        if !kind.is_payload() {
            return Err(invalid(
                "reserved structural object kind cannot be a logical blob",
            ));
        }
        lease.ensure_store(self)?;
        if dependencies.len() > MAX_OBJECT_DEPS {
            return Err(invalid("blob dependency count exceeds physical bound"));
        }
        for dependency in dependencies {
            self.adopt_existing_root(lease, dependency)?;
        }
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let root = if bytes.len() <= BLOCK_BYTES {
            let mut payload = Vec::new();
            payload
                .try_reserve_exact(
                    bytes
                        .len()
                        .checked_add(1)
                        .ok_or_else(|| invalid("inline blob length overflow"))?,
                )
                .map_err(|error| storage("reserve inline blob", error))?;
            payload.push(0);
            payload.extend_from_slice(bytes);
            self.put_adopted_object(lease, kind, dependencies, &payload)?
        } else {
            let mut level = Vec::new();
            level
                .try_reserve_exact(bytes.len().div_ceil(BLOCK_BYTES))
                .map_err(|error| storage("reserve blob blocks", error))?;
            for block in bytes.chunks(BLOCK_BYTES) {
                let reference =
                    self.put_adopted_object(lease, ObjectKind::BlobBlock, &[], block)?;
                level.push((
                    reference,
                    u64::try_from(block.len())
                        .map_err(|error| storage("block length width", error))?,
                ));
            }
            while level.len() > NODE_FANOUT {
                level = self.make_blob_level(lease, &level)?;
            }
            let (payload, mut child_refs) = encode_blob_node(&level)?;
            child_refs.extend_from_slice(dependencies);
            self.put_adopted_object(lease, kind, &child_refs, &payload)?
        };
        Ok(BlobRef {
            root,
            bytes: u64::try_from(bytes.len())
                .map_err(|error| storage("blob length width", error))?,
            sha256: digest,
        })
    }

    fn make_blob_level(
        &self,
        lease: &AttemptLease,
        children: &[(ObjectRef, u64)],
    ) -> Result<Vec<(ObjectRef, u64)>, CoreError> {
        let mut output = Vec::new();
        output
            .try_reserve_exact(children.len().div_ceil(NODE_FANOUT))
            .map_err(|error| storage("reserve blob nodes", error))?;
        for group in children.chunks(NODE_FANOUT) {
            let (payload, dependencies) = encode_blob_node(group)?;
            let total = group
                .iter()
                .try_fold(0_u64, |sum, (_, len)| sum.checked_add(*len))
                .ok_or_else(|| invalid("blob subtree length overflow"))?;
            output.push((
                self.put_adopted_object(lease, ObjectKind::BlobNode, &dependencies, &payload)?,
                total,
            ));
        }
        Ok(output)
    }

    pub(crate) fn read_blob(
        &self,
        blob: &BlobRef,
        max_bytes: u64,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Vec<u8>, CoreError> {
        if !blob.root.kind.is_payload() || blob.bytes > max_bytes {
            return Err(invalid("logical blob kind or byte admission exceeded"));
        }
        let len =
            usize::try_from(blob.bytes).map_err(|error| storage("blob length width", error))?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(len)
            .map_err(|error| storage("reserve blob body", error))?;
        self.read_blob_node(&blob.root, true, len, 0, &mut output, budget)?;
        if output.len() != len || <[u8; 32]>::from(Sha256::digest(&output)) != blob.sha256 {
            return Err(corrupt(
                "logical blob bytes differ from committed length or digest",
            ));
        }
        Ok(output)
    }

    /// Start a bounded physical-block traversal. The caller must drive it to
    /// `None` before treating the logical blob as authenticated.
    pub(crate) fn blob_cursor(
        &self,
        blob: &BlobRef,
        max_bytes: u64,
    ) -> Result<BlobCursor, CoreError> {
        if !blob.root.kind.is_payload() || blob.bytes > max_bytes {
            return Err(invalid("logical blob kind or byte admission exceeded"));
        }
        Ok(BlobCursor {
            frames: vec![BlobFrame::Object {
                reference: blob.root.clone(),
                root: true,
                depth: 0,
                expected_len: blob.bytes,
            }],
            expected_len: blob.bytes,
            expected_sha256: blob.sha256,
            produced: 0,
            digest: Sha256::new(),
            complete: false,
        })
    }

    fn read_blob_node(
        &self,
        reference: &ObjectRef,
        root: bool,
        max_len: usize,
        depth: u8,
        output: &mut Vec<u8>,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        checkpoint(budget, "lexical:blob:read")?;
        if depth > 16 {
            return Err(corrupt("blob tree exceeds maximum depth"));
        }
        let (deps, payload) = self.read_object(reference, budget)?;
        if !root && reference.kind == ObjectKind::BlobBlock {
            if !deps.is_empty() || payload.is_empty() || payload.len() > BLOCK_BYTES {
                return Err(corrupt("blob block shape or dependencies differ"));
            }
            if output
                .len()
                .checked_add(payload.len())
                .is_none_or(|length| length > max_len)
            {
                return Err(corrupt("blob subtree exceeds admitted logical length"));
            }
            output.extend_from_slice(&payload);
            return Ok(());
        }
        if !(root && reference.kind.is_payload() || !root && reference.kind == ObjectKind::BlobNode)
        {
            return Err(corrupt("blob tree contains a wrong object kind"));
        }
        let Some((&tag, rest)) = payload.split_first() else {
            return Err(corrupt("blob root has no payload tag"));
        };
        if tag == 0 {
            if !root || rest.len() > BLOCK_BYTES {
                return Err(corrupt("inline blob node is not a root"));
            }
            // Inline roots may carry caller dependencies outside the blob tree.
            if output
                .len()
                .checked_add(rest.len())
                .is_none_or(|length| length > max_len)
            {
                return Err(corrupt("inline blob exceeds admitted logical length"));
            }
            output.extend_from_slice(rest);
            return Ok(());
        }
        if tag != 1 {
            return Err(corrupt("unknown blob payload tag"));
        }
        if root && max_len <= BLOCK_BYTES {
            return Err(corrupt("small blob uses noncanonical tree encoding"));
        }
        let children = decode_blob_node(rest)?;
        let mut child_refs = Vec::new();
        child_refs
            .try_reserve_exact(children.len())
            .map_err(|error| storage("reserve blob child references", error))?;
        child_refs.extend(children.iter().map(|(reference, _)| reference.clone()));
        if child_refs.iter().any(|reference| !deps.contains(reference)) {
            return Err(corrupt("blob node omits an authenticated child dependency"));
        }
        if !root && deps.iter().any(|reference| !child_refs.contains(reference)) {
            return Err(corrupt("blob node has extraneous dependencies"));
        }
        for (child, length) in children {
            let before = output.len();
            self.read_blob_node(
                &child,
                false,
                max_len,
                depth
                    .checked_add(1)
                    .ok_or_else(|| corrupt("blob depth overflow"))?,
                output,
                budget,
            )?;
            if u64::try_from(output.len().saturating_sub(before))
                .map_err(|_conversion_error| corrupt("blob subtree width"))?
                != length
            {
                return Err(corrupt("blob subtree length differs"));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn walk_closure(
        &self,
        roots: &[ObjectRef],
        budget: Option<&RequestBudgetV1>,
        visitor: &mut dyn FnMut(&ObjectRef) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(roots.len())
            .map_err(|error| storage("reserve closure roots", error))?;
        pending.extend_from_slice(roots);
        let mut visited = BTreeSet::new();
        while let Some(reference) = pending.pop() {
            checkpoint(budget, "lexical:object:closure")?;
            if !visited.insert(reference.clone()) {
                continue;
            }
            if visited.len() > MAX_CLOSURE_REFS {
                return Err(invalid("object closure exceeds traversal bound"));
            }
            let (dependencies, _payload) = self.read_object(&reference, budget)?;
            visitor(&reference)?;
            if pending
                .len()
                .checked_add(dependencies.len())
                .is_none_or(|length| length > MAX_CLOSURE_REFS)
            {
                return Err(invalid("object closure pending work exceeds bound"));
            }
            pending
                .try_reserve(dependencies.len())
                .map_err(|error| storage("reserve closure work", error))?;
            pending.extend(dependencies);
        }
        Ok(())
    }
}

impl AttemptLease {
    pub(super) fn ensure_store(&self, store: &ObjectStore) -> Result<(), CoreError> {
        let pinned = [
            (&self.store.pool, &store.pool),
            (&self.store.objects, &store.objects),
            (&self.store.attempts, &store.attempts),
        ];
        if pinned.iter().all(|(left, right)| Arc::ptr_eq(left, right)) {
            return Ok(());
        }
        for (left, right) in pinned {
            let owned = left
                .metadata()
                .map_err(|error| storage("stat attempt store", error))?;
            let target = right
                .metadata()
                .map_err(|error| storage("stat target store", error))?;
            if owned.dev() != target.dev() || owned.ino() != target.ino() {
                return Err(invalid(
                    "attempt lease belongs to another object store handle",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn binding(&self) -> &AttemptBinding {
        &self.binding
    }

    pub(crate) fn write_cost_snapshot(&self) -> Result<AttemptWriteCost, CoreError> {
        self.write_cost
            .lock()
            .map(|cost| *cost)
            .map_err(|error| storage("read attempt write cost", error))
    }

    fn record_write_cost(
        &self,
        reference: &ObjectRef,
        newly_written: bool,
    ) -> Result<(), CoreError> {
        let mut cost = self
            .write_cost
            .lock()
            .map_err(|error| storage("record attempt write cost", error))?;
        if newly_written {
            let objects = cost
                .new_objects
                .checked_add(1)
                .ok_or_else(|| invalid("attempt write object count overflow"))?;
            let bytes = cost
                .new_encoded_bytes
                .checked_add(reference.bytes)
                .ok_or_else(|| invalid("attempt write encoded byte count overflow"))?;
            cost.new_objects = objects;
            cost.new_encoded_bytes = bytes;
        } else {
            let objects = cost
                .reused_objects
                .checked_add(1)
                .ok_or_else(|| invalid("attempt reuse object count overflow"))?;
            let bytes = cost
                .reused_encoded_bytes
                .checked_add(reference.bytes)
                .ok_or_else(|| invalid("attempt reuse encoded byte count overflow"))?;
            cost.reused_objects = objects;
            cost.reused_encoded_bytes = bytes;
        }
        drop(cost);
        Ok(())
    }

    fn adopt_batch_locked(&self, references: &[ObjectRef]) -> Result<(), CoreError> {
        if references.is_empty() || references.len() > MAX_ADOPTION_BATCH_REFS {
            return Err(invalid("batched adoption count exceeds bound"));
        }
        let record = self
            .store
            .read_attempt(&self.name)?
            .ok_or_else(|| corrupt("batched attempt record is absent"))?;
        if record.0 != self.binding {
            return Err(corrupt("batched attempt binding changed"));
        }
        let mut journal = self.store.open_attempt_log(&self.name)?;
        let read_result = (|| {
            let head = self.store.read_journal_head(&self.name)?;
            if head.committed_length > 0 {
                let _offset = journal
                    .seek(SeekFrom::End(-32))
                    .map_err(|error| admitted_io_error("seek batched attempt journal", &error))?;
                let mut chain = [0_u8; 32];
                journal
                    .read_exact(&mut chain)
                    .map_err(|error| admitted_io_error("read batched attempt chain", &error))?;
                if chain != head.committed_chain {
                    return Err(corrupt("batched attempt chain differs from head"));
                }
            }
            let sorted = canonical_deps(references)?;
            if sorted.len() != references.len() {
                return Err(invalid("batched adoption has repeated references"));
            }
            let manifest_payload = batch_manifest_payload(&self.name, &head, sorted.len())?;
            let (manifest, manifest_new) = self.store.put_object_locked_with_status(
                ObjectKind::AttemptBatch,
                &sorted,
                &manifest_payload,
            )?;
            self.record_write_cost(&manifest, manifest_new)?;
            if !manifest_new {
                // A reused manifest can have survived a failed rename-to-shard
                // sync. Its POSIX repair happened after the first batch barrier.
                self.store.sync_pool_before_adoption()?;
            }
            let (entries, final_chain) = adoption_entries(head.committed_chain, &sorted)?;
            let new_length = head
                .committed_length
                .checked_add(
                    u64::try_from(entries.len())
                        .map_err(|error| storage("batched entry width", error))?,
                )
                .ok_or_else(|| invalid("batched journal length overflow"))?;
            Ok::<_, CoreError>((head, manifest, entries, final_chain, new_length))
        })();
        let (head, manifest, entries, final_chain, new_length) =
            finish_control_read(journal, read_result)?;
        let mut journal =
            self.store
                .begin_journal_update(&self.name, head.committed_length, new_length)?;
        let before_sync = (|| {
            self.store.write_journal_head(
                &self.name,
                &JournalHead {
                    committed_length: head.committed_length,
                    committed_chain: head.committed_chain,
                    pending: Some(manifest),
                    expected_chain: final_chain,
                },
            )?;
            let _offset = journal
                .seek(SeekFrom::End(0))
                .map_err(|error| admitted_io_error("seek batched journal end", &error))?;
            #[cfg(test)]
            crate::publication_faults::reach_core(
                crate::publication_faults::Cut::AttemptJournalAppendWrite,
                crate::publication_faults::Side::Before,
            )?;
            journal
                .write_all(&entries)
                .map_err(|error| admitted_io_error("append batched journal", &error))?;
            #[cfg(test)]
            crate::publication_faults::reach_core(
                crate::publication_faults::Cut::AttemptJournalAppendWrite,
                crate::publication_faults::Side::After,
            )?;
            #[cfg(test)]
            crate::publication_faults::reach_core(
                crate::publication_faults::Cut::AttemptJournalAppendFileSync,
                crate::publication_faults::Side::Before,
            )?;
            Ok::<(), CoreError>(())
        })();
        if let Err(primary) = before_sync {
            return Err(retire_failed_journal(journal, primary));
        }
        self.store.sync_and_settle_journal(
            journal,
            quanta_index_core::DiagnosticSyncLabelV1::PoolAdoptionJournalFull,
            "sync batched journal",
        )?;
        #[cfg(test)]
        crate::publication_faults::reach_core(
            crate::publication_faults::Cut::AttemptJournalAppendFileSync,
            crate::publication_faults::Side::After,
        )?;
        self.store
            .write_journal_head(&self.name, &JournalHead::clean(new_length, final_chain))
    }

    pub(super) fn adopt_locked(&self, root: &ObjectRef) -> Result<(), CoreError> {
        let record = self
            .store
            .read_attempt(&self.name)?
            .ok_or_else(|| corrupt("active attempt record is absent"))?;
        if record.0 != self.binding {
            return Err(corrupt("attempt binding changed"));
        }
        let _epoch = self.store.bump_epoch_locked()?;
        let mut journal = self.store.open_attempt_log(&self.name)?;
        let read_result = (|| {
            let head = self.store.read_journal_head(&self.name)?;
            if head.committed_length > 0 {
                let _offset = journal
                    .seek(SeekFrom::End(-32))
                    .map_err(|error| admitted_io_error("seek attempt journal", &error))?;
                let mut chain = [0_u8; 32];
                journal
                    .read_exact(&mut chain)
                    .map_err(|error| admitted_io_error("read attempt journal chain", &error))?;
                if chain != head.committed_chain {
                    return Err(corrupt("attempt journal tail chain differs from head"));
                }
            }
            let mut entry = Vec::new();
            entry
                .try_reserve_exact(73)
                .map_err(|error| storage("reserve attempt journal entry", error))?;
            encode_ref(&mut entry, root);
            let mut hasher = Sha256::new();
            hasher.update(b"lexical-attempt-adoption-v1");
            hasher.update(head.committed_chain);
            hasher.update(&entry);
            let next: [u8; 32] = hasher.finalize().into();
            entry.extend_from_slice(&next);
            let new_length = head
                .committed_length
                .checked_add(JOURNAL_ENTRY_BYTES)
                .ok_or_else(|| invalid("attempt journal length overflow"))?;
            // An inherited root may be a verified orphan from an aborted batch.
            // Complete its physical barrier before the first durable head names it.
            self.store.sync_pool_before_adoption()?;
            Ok::<_, CoreError>((head, entry, next, new_length))
        })();
        let (head, entry, next, new_length) = finish_control_read(journal, read_result)?;
        let mut journal =
            self.store
                .begin_journal_update(&self.name, head.committed_length, new_length)?;
        let before_sync = (|| {
            self.store.write_journal_head(
                &self.name,
                &JournalHead {
                    committed_length: head.committed_length,
                    committed_chain: head.committed_chain,
                    pending: Some(root.clone()),
                    expected_chain: next,
                },
            )?;
            let _offset = journal
                .seek(SeekFrom::End(0))
                .map_err(|error| admitted_io_error("seek attempt journal end", &error))?;
            #[cfg(test)]
            crate::publication_faults::reach_core(
                crate::publication_faults::Cut::AttemptJournalAppendWrite,
                crate::publication_faults::Side::Before,
            )?;
            journal
                .write_all(&entry)
                .map_err(|error| admitted_io_error("append attempt journal", &error))?;
            #[cfg(test)]
            crate::publication_faults::reach_core(
                crate::publication_faults::Cut::AttemptJournalAppendWrite,
                crate::publication_faults::Side::After,
            )?;
            #[cfg(test)]
            crate::publication_faults::reach_core(
                crate::publication_faults::Cut::AttemptJournalAppendFileSync,
                crate::publication_faults::Side::Before,
            )?;
            Ok::<(), CoreError>(())
        })();
        if let Err(primary) = before_sync {
            return Err(retire_failed_journal(journal, primary));
        }
        self.store.sync_and_settle_journal(
            journal,
            quanta_index_core::DiagnosticSyncLabelV1::PoolAdoptionJournalFull,
            "sync attempt journal",
        )?;
        #[cfg(test)]
        crate::publication_faults::reach_core(
            crate::publication_faults::Cut::AttemptJournalAppendFileSync,
            crate::publication_faults::Side::After,
        )?;
        self.store
            .write_journal_head(&self.name, &JournalHead::clean(new_length, next))
    }

    /// Caller must have durably published a sealed root which owns these
    /// object references before this lease can be retired.
    pub(crate) fn finish_published(self, committed_roots: &[ObjectRef]) -> Result<(), CoreError> {
        self.retire(Some(committed_roots))
    }

    /// Caller must first prove the unsealed attempt was fenced and discarded.
    pub(crate) fn discard_unsealed(self) -> Result<(), CoreError> {
        self.retire(None)
    }

    /// The immutable sealed target and writer-free fence independently
    /// protect the committed roots; this attempt may have stopped earlier.
    fn retire_after_sealed_settlement(self) -> Result<(), CoreError> {
        self.retire(None)
    }

    fn retire(self, committed: Option<&[ObjectRef]>) -> Result<(), CoreError> {
        let _gate = self.store.lock_gate()?;
        let record = self
            .store
            .read_attempt(&self.name)?
            .ok_or_else(|| corrupt("attempt record disappeared before retirement"))?;
        if record.0 != self.binding {
            return Err(corrupt("attempt binding changed before retirement"));
        }
        if let Some(committed) = committed {
            let mut pending = BTreeSet::new();
            pending.extend(
                committed
                    .iter()
                    .filter(|root| record.1.binary_search(root).is_err())
                    .cloned(),
            );
            self.store.visit_attempt_log(&self.name, |root| {
                let _removed = pending.remove(root);
                Ok(())
            })?;
            if !pending.is_empty() {
                return Err(invalid("published root was not adopted by attempt"));
            }
        }
        let _epoch = self.store.bump_epoch_locked()?;
        self.store.write_retirement_marker(&self.name, &record)?;
        self.store.recover_retirement_locked(&self.name)
    }
}

impl ObjectStore {
    /// Capture an adoption/publication fence before off-gate closure marking.
    pub(crate) fn gc_epoch(&self) -> Result<u64, CoreError> {
        let _gate = self.lock_gate()?;
        self.read_epoch()
    }

    pub(crate) fn adopted_root_cursor(&self) -> Result<AdoptedRootCursor, CoreError> {
        // This control recovery is capped by MAX_ATTEMPTS. It is a one-time
        // startup cost, separate from the per-step root/journal work bound.
        self.recover_retirements()?;
        let directory = rustix::fs::Dir::read_from(self.attempts.as_ref())
            .map_err(|error| storage("enumerate adopted attempts", error))?;
        Ok(AdoptedRootCursor {
            directory,
            attempts_seen: 0,
            header_roots: Vec::new().into_iter(),
            journal: None,
        })
    }

    pub(crate) fn object_inventory_cursor(&self) -> Result<ObjectInventoryCursor, CoreError> {
        {
            let _gate = self.lock_gate()?;
            self.reconcile_pending_pool_unlinks_locked()?;
        }
        self.cleanup_control_temporaries(self.pool.as_ref(), is_pool_temporary_name)?;
        Ok(ObjectInventoryCursor {
            next_prefix: 0,
            directory: None,
        })
    }

    /// Enumerate physical digest names before sweeping. Crash-orphaned
    /// publication temporaries are removed under the physical write gate.
    /// The caller must not delete digest objects while this cursor is live.
    #[cfg(test)]
    pub(crate) fn visit_object_digests(
        &self,
        visitor: &mut dyn FnMut(&[u8; 32]) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        {
            let _gate = self.lock_gate()?;
            self.reconcile_pending_pool_unlinks_locked()?;
        }
        self.cleanup_control_temporaries(self.pool.as_ref(), is_pool_temporary_name)?;
        for prefix in 0_u16..=u16::from(u8::MAX) {
            self.visit_object_shard(
                u8::try_from(prefix).map_err(|_conversion_error| invalid("object shard width"))?,
                visitor,
            )?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn visit_object_shard(
        &self,
        prefix: u8,
        visitor: &mut dyn FnMut(&[u8; 32]) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        use std::os::unix::ffi::OsStringExt as _;

        let hex = hex_digest(&[prefix; 32]);
        let shard_name = hex.get(..2).ok_or_else(|| invalid("object shard prefix"))?;
        let directory = match openat(
            self.objects.as_ref(),
            shard_name,
            dir_flags(),
            Mode::empty(),
        ) {
            Ok(directory) => File::from(directory),
            Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(error) => return Err(storage("open GC object shard", error)),
        };
        self.check_object_shard_device(&directory)?;
        let mut entries = rustix::fs::Dir::read_from(&directory)
            .map_err(|error| storage("enumerate object shard", error))?;
        while let Some(entry) = entries.read() {
            let entry = entry.map_err(|error| storage("read object shard entry", error))?;
            let name = std::ffi::OsString::from_vec(entry.file_name().to_bytes().to_vec());
            let Some(name) = name.to_str() else {
                return Err(corrupt("non-UTF8 object entry"));
            };
            if name == "." || name == ".." {
                continue;
            }
            if is_object_temporary_name(name, prefix) {
                let _gate = self.lock_gate()?;
                self.cleanup_pool_temporary(&directory, name)?;
                continue;
            }
            if name.len() != 64
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(corrupt("noncanonical object entry"));
            }
            let mut digest = [0_u8; 32];
            for (slot, pair) in digest.iter_mut().zip(name.as_bytes().chunks_exact(2)) {
                let high = decode_hex_digit(
                    *pair
                        .first()
                        .ok_or_else(|| corrupt("object name pair absent"))?,
                )?;
                let low = decode_hex_digit(
                    *pair
                        .get(1)
                        .ok_or_else(|| corrupt("object name pair truncated"))?,
                )?;
                *slot = (high << 4) | low;
            }
            if digest.first() != Some(&prefix) {
                return Err(corrupt("object is in the wrong shard"));
            }
            visitor(&digest)?;
        }
        Ok(())
    }

    /// Caller marks these roots together with every retained sealed generation.
    /// An epoch mismatch at sweep discards this mark pass.
    #[cfg(test)]
    pub(crate) fn visit_adopted_roots(
        &self,
        visitor: &mut dyn FnMut(&ObjectRef) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        use std::os::unix::ffi::OsStringExt as _;
        self.recover_retirements()?;
        let mut directory = rustix::fs::Dir::read_from(self.attempts.as_ref())
            .map_err(|error| storage("enumerate attempts", error))?;
        let mut count = 0_usize;
        while let Some(entry) = directory.read() {
            let entry = entry.map_err(|error| storage("read attempt entry", error))?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            let name = std::ffi::OsString::from_vec(name.to_vec());
            let Some(name) = name.to_str() else {
                return Err(corrupt("non-UTF8 attempt entry"));
            };
            if name.starts_with('.') && name.contains(".tmp-") {
                continue;
            }
            if let Some(header_name) = name.strip_suffix(".cbor.log") {
                let canonical_header = format!("{header_name}.cbor");
                if self.read_attempt_header(&canonical_header)?.is_none() {
                    let _gate = self.lock_gate()?;
                    let _cleared = self.cleanup_unowned_attempt_locked(&canonical_header)?;
                }
                continue;
            }
            if let Some(header_name) = name.strip_suffix(".cbor.head") {
                let canonical_header = format!("{header_name}.cbor");
                if self.read_attempt_header(&canonical_header)?.is_none() {
                    let _gate = self.lock_gate()?;
                    let _cleared = self.cleanup_unowned_attempt_locked(&canonical_header)?;
                }
                continue;
            }
            if let Some(header_name) = name.strip_suffix(".cbor.scan") {
                if !is_lower_digest_name(header_name) {
                    return Err(corrupt("hard-scan checkpoint name differs"));
                }
                let canonical_header = format!("{header_name}.cbor");
                if self.read_attempt_header(&canonical_header)?.is_none() {
                    let _gate = self.lock_gate()?;
                    let _cleared = self.cleanup_unowned_attempt_locked(&canonical_header)?;
                }
                continue;
            }
            if name.strip_suffix(".cbor").is_none() {
                return Err(corrupt("unexpected attempt entry"));
            }
            count = count
                .checked_add(1)
                .ok_or_else(|| invalid("attempt count overflow"))?;
            if count > MAX_ATTEMPTS {
                return Err(invalid("protected attempt count exceeds bound"));
            }
            let record = self
                .read_attempt(name)?
                .ok_or_else(|| corrupt("listed attempt disappeared"))?;
            for root in &record.1 {
                visitor(root)?;
            }
            self.visit_attempt_log(name, &mut *visitor)?;
        }
        Ok(())
    }

    fn recover_retirements(&self) -> Result<(), CoreError> {
        use std::os::unix::ffi::OsStringExt as _;
        self.cleanup_control_temporaries(self.attempts.as_ref(), is_attempt_temporary_name)?;
        let mut retirement_names = Vec::new();
        let mut scan_names = Vec::new();
        let mut retirement_directory = rustix::fs::Dir::read_from(self.attempts.as_ref())
            .map_err(|error| storage("enumerate attempt retirements", error))?;
        let mut inspected = 0_usize;
        while let Some(entry) = retirement_directory.read() {
            let entry = entry.map_err(|error| storage("read retirement entry", error))?;
            inspected = inspected
                .checked_add(1)
                .ok_or_else(|| invalid("attempt control entry count overflow"))?;
            if inspected > MAX_ATTEMPT_CONTROL_ENTRIES {
                return Err(invalid(
                    "attempt control directory exceeds bounded inventory",
                ));
            }
            let name = std::ffi::OsString::from_vec(entry.file_name().to_bytes().to_vec());
            let Some(name) = name.to_str() else {
                return Err(corrupt("non-UTF8 attempt entry"));
            };
            if let Some(header) = name.strip_suffix(".cbor.retired") {
                if retirement_names.len() >= MAX_ATTEMPTS {
                    return Err(invalid("retirement count exceeds bound"));
                }
                retirement_names.push(format!("{header}.cbor"));
            }
            if let Some(header) = name.strip_suffix(".cbor.scan") {
                if !is_lower_digest_name(header) || scan_names.len() >= MAX_ATTEMPTS {
                    return Err(corrupt("hard-scan checkpoint inventory differs"));
                }
                scan_names.push(format!("{header}.cbor"));
            }
        }
        drop(retirement_directory);
        for name in retirement_names {
            let _gate = self.lock_gate()?;
            self.recover_retirement_locked(&name)?;
        }
        for name in scan_names {
            let _gate = self.lock_gate()?;
            if self.read_attempt_header(&name)?.is_none() {
                let _cleared = self.cleanup_unowned_attempt_locked(&name)?;
            }
        }
        Ok(())
    }

    /// Removes at most one small batch per maintenance visit. A concurrent
    /// writer holds the same gate from temporary creation through rename;
    /// once acquired, any still-present canonical temp is an orphan.
    fn cleanup_control_temporaries(
        &self,
        parent: &File,
        is_ours: fn(&str) -> bool,
    ) -> Result<(), CoreError> {
        use std::os::unix::ffi::OsStringExt as _;
        // Reconcile a retired create or prior unlink before a pathname scan:
        // its last path may already be absent from this directory snapshot.
        self.reconcile_control_deletions()?;
        let qualified_updates = if let Some(control) = &self.control_output_admission {
            if control.held_file_allowance()?.is_some() {
                let updates = control.file_updates()?.ok_or_else(|| {
                    invalid("physical Control temporary lacks its file update authority")
                })?;
                let directory = if std::ptr::eq(parent, self.pool.as_ref()) {
                    self.state_root.join(POOL_DIR)
                } else if std::ptr::eq(parent, self.attempts.as_ref()) {
                    self.state_root.join(POOL_DIR).join(ATTEMPTS_DIR)
                } else {
                    return Err(invalid("Control temporary parent differs from pool"));
                };
                Self::require_current_control_parent(parent, &directory)?;
                Some((updates, directory))
            } else {
                None
            }
        } else {
            None
        };
        let mut entries = rustix::fs::Dir::read_from(parent)
            .map_err(|error| storage("enumerate control temporaries", error))?;
        let mut names = Vec::new();
        let mut inspected = 0_usize;
        names
            .try_reserve_exact(MAX_TEMP_CLEANUP_BATCH)
            .map_err(|error| storage("reserve temporary cleanup batch", error))?;
        while let Some(entry) = entries.read() {
            let entry = entry.map_err(|error| storage("read control temporary", error))?;
            inspected = inspected
                .checked_add(1)
                .ok_or_else(|| invalid("control temporary entry count overflow"))?;
            if inspected > MAX_ATTEMPT_CONTROL_ENTRIES {
                return Err(invalid("control directory exceeds bounded inventory"));
            }
            let name = std::ffi::OsString::from_vec(entry.file_name().to_bytes().to_vec());
            let Some(name) = name.to_str() else {
                return Err(corrupt("non-UTF8 control entry"));
            };
            if is_ours(name) {
                names.push(name.to_owned());
                if names.len() >= MAX_TEMP_CLEANUP_BATCH {
                    break;
                }
            }
        }
        drop(entries);
        if names.is_empty() {
            return Ok(());
        }
        let _gate = self.lock_gate()?;
        self.reconcile_control_deletions()?;
        if let Some((_, directory)) = &qualified_updates {
            Self::require_current_control_parent(parent, directory)?;
        }
        let mut removed = false;
        for name in names {
            if let Some((updates, directory)) = &qualified_updates {
                let control = self
                    .control_output_admission
                    .as_ref()
                    .ok_or_else(|| invalid("Control temporary admission missing"))?;
                updates.remove_file_and_sync(&directory.join(name), control.clone())?;
                continue;
            }
            match unlinkat(parent, name.as_str(), AtFlags::empty()) {
                Ok(()) => removed = true,
                Err(rustix::io::Errno::NOENT) => {}
                Err(error) => return Err(storage("remove control temporary", error)),
            }
        }
        if removed {
            parent
                .sync_all()
                .map_err(|error| storage("sync control temporary cleanup", error))?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn adopted_roots(&self) -> Result<Vec<ObjectRef>, CoreError> {
        let mut roots = BTreeSet::new();
        self.visit_adopted_roots(&mut |root| {
            let _inserted = roots.insert(root.clone());
            Ok(())
        })?;
        Ok(roots.into_iter().collect())
    }

    /// Deletes one bounded orphan batch only when no attempt or publication
    /// transition has changed since the caller's complete closure mark.
    /// `false` means stale mark; no candidate was deleted.
    #[cfg(test)]
    pub(crate) fn gc_sweep_batch(
        &self,
        expected_epoch: u64,
        candidate_digests: &[[u8; 32]],
        live_digests: &BTreeSet<[u8; 32]>,
    ) -> Result<bool, CoreError> {
        if self.object_lifetime.is_some() {
            return Err(invalid("qualified GC requires retained progress custody"));
        }
        self.gc_sweep_checked_batch(
            expected_epoch,
            candidate_digests,
            &mut |digest| Ok(live_digests.contains(digest)),
            &mut None,
        )
    }

    pub(crate) fn settle_pending_gc_delete(
        &self,
        pending: &mut Option<GcDeleteCustody>,
    ) -> Result<(), CoreError> {
        let _gate = self.lock_gate()?;
        self.settle_pending_gc_delete_locked(pending)
    }

    fn settle_pending_gc_delete_locked(
        &self,
        pending: &mut Option<GcDeleteCustody>,
    ) -> Result<(), CoreError> {
        let Some(custody) = pending.as_mut() else {
            return Ok(());
        };
        match custody.delete_and_settle(self.pool_write_admission.as_deref()) {
            Ok(()) => {
                drop(pending.take());
                Ok(())
            }
            Err(error) => {
                // Only qualified deletion tickets own a physical path claim.
                // The unqualified pool's existing quota reconciliation may
                // settle the old unlink before another writer recreates the
                // digest; retaining that old slot would later refund new data.
                if !custody.is_qualified() || !custody.must_retain_after_error() {
                    drop(pending.take());
                }
                Err(error)
            }
        }
    }

    pub(crate) fn gc_sweep_checked_batch(
        &self,
        expected_epoch: u64,
        candidate_digests: &[[u8; 32]],
        is_live: &mut dyn FnMut(&[u8; 32]) -> Result<bool, CoreError>,
        pending: &mut Option<GcDeleteCustody>,
    ) -> Result<bool, CoreError> {
        if candidate_digests.len() > MAX_GC_BATCH {
            return Err(invalid("GC batch exceeds bounded gate policy"));
        }
        let _gate = self.lock_gate()?;
        self.settle_pending_gc_delete_locked(pending)?;
        if self.read_epoch()? != expected_epoch {
            return Ok(false);
        }
        self.reconcile_pending_pool_unlinks_locked()?;
        for digest in candidate_digests {
            if is_live(digest)? {
                continue;
            }
            let hex = hex_digest(digest);
            let prefix = hex.get(..2).ok_or_else(|| invalid("GC digest prefix"))?;
            let directory = match openat(self.objects.as_ref(), prefix, dir_flags(), Mode::empty())
            {
                Ok(directory) => File::from(directory),
                Err(rustix::io::Errno::NOENT) => continue,
                Err(error) => return Err(storage("open GC object shard", error)),
            };
            self.check_object_shard_device(&directory)?;
            let (metadata, ticket) = if let Some(lifetime) = &self.object_lifetime {
                // A prior item in this batch may already have settled before
                // a later item failed. Replaying the unadvanced cursor must
                // skip that absent entry before the binder opens a source FD.
                match rustix::fs::statat(&directory, hex.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(_present) => {}
                    Err(rustix::io::Errno::NOENT) => continue,
                    Err(error) => return Err(storage("probe qualified GC candidate", error)),
                }
                let path = self
                    .state_root
                    .join(POOL_DIR)
                    .join(OBJECTS_DIR)
                    .join(prefix)
                    .join(&hex);
                let ticket = match lifetime.begin_delete(&path) {
                    Ok(ticket) => ticket,
                    Err(CoreError::NotFound(_)) => continue,
                    Err(error) => return Err(error),
                };
                let (source, anchored_parent) = ticket.source_metadata()?;
                let held_parent = directory
                    .metadata()
                    .map_err(|error| storage("stat GC shard", error))?;
                if held_parent.dev() != anchored_parent.dev()
                    || held_parent.ino() != anchored_parent.ino()
                {
                    return Err(corrupt("GC deletion ticket differs from the held shard"));
                }
                (source, Some(ticket))
            } else {
                let object = match openat(
                    &directory,
                    hex.as_str(),
                    OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                    Mode::empty(),
                ) {
                    Ok(object) => File::from(object),
                    Err(rustix::io::Errno::NOENT) => continue,
                    Err(error) => return Err(storage("open GC candidate", error)),
                };
                let metadata = object
                    .metadata()
                    .map_err(|error| storage("stat GC candidate", error))?;
                drop(object);
                (metadata, None)
            };
            if !metadata.is_file()
                || metadata.dev() != self.objects_device
                || metadata.len()
                    > u64::try_from(MAX_OBJECT_BYTES)
                        .map_err(|error| storage("GC object bound width", error))?
            {
                return Err(corrupt("GC candidate is not a bounded regular object"));
            }
            *pending = Some(GcDeleteCustody::new(
                *digest,
                metadata.len(),
                directory,
                ticket,
            ));
            self.settle_pending_gc_delete_locked(pending)?;
        }
        Ok(true)
    }
}

impl AdoptedRootCursor {
    /// Advances by one root, journal entry, or directory entry. Reading an
    /// admitted attempt header (at most 8 MiB) is one indivisible unit.
    pub(crate) fn next_step(
        &mut self,
        store: &ObjectStore,
    ) -> Result<AdoptedRootAdvance, CoreError> {
        use std::os::unix::ffi::OsStringExt as _;

        if let Some(root) = self.header_roots.next() {
            return Ok(AdoptedRootAdvance::Root(root));
        }
        if let Some(journal) = self.journal.as_mut() {
            if journal.remaining == 0 {
                if journal.previous != journal.expected_chain {
                    return Err(corrupt("attempt journal final chain differs from head"));
                }
                self.journal
                    .take()
                    .ok_or_else(|| corrupt("attempt journal cursor missing"))?
                    .file
                    .retire()?;
                return Ok(AdoptedRootAdvance::Advanced);
            }
            let mut entry = [0_u8; 73];
            journal
                .file
                .read_exact(&mut entry)
                .map_err(|error| storage("read adopted attempt journal", error))?;
            let reference_bytes = entry
                .get(..41)
                .ok_or_else(|| corrupt("attempt journal reference truncated"))?;
            let mut hasher = Sha256::new();
            hasher.update(b"lexical-attempt-adoption-v1");
            hasher.update(journal.previous);
            hasher.update(reference_bytes);
            let expected: [u8; 32] = hasher.finalize().into();
            if entry.get(41..) != Some(expected.as_slice()) {
                return Err(corrupt("attempt journal chain differs"));
            }
            journal.previous = expected;
            journal.remaining = journal
                .remaining
                .checked_sub(1)
                .ok_or_else(|| corrupt("attempt journal cursor underflow"))?;
            let mut offset = 0;
            return Ok(AdoptedRootAdvance::Root(decode_ref(
                reference_bytes,
                &mut offset,
            )?));
        }
        let Some(entry) = self.directory.read() else {
            return Ok(AdoptedRootAdvance::Done);
        };
        let entry = entry.map_err(|error| storage("read adopted attempt entry", error))?;
        let name = std::ffi::OsString::from_vec(entry.file_name().to_bytes().to_vec());
        let Some(name) = name.to_str() else {
            return Err(corrupt("non-UTF8 attempt entry"));
        };
        if name == "." || name == ".." || (name.starts_with('.') && name.contains(".tmp-")) {
            return Ok(AdoptedRootAdvance::Advanced);
        }
        if let Some(header_name) = name.strip_suffix(".cbor.log") {
            let canonical_header = format!("{header_name}.cbor");
            if store.read_attempt_header(&canonical_header)?.is_none() {
                let _gate = store.lock_gate()?;
                let _cleared = store.cleanup_unowned_attempt_locked(&canonical_header)?;
            }
            return Ok(AdoptedRootAdvance::Advanced);
        }
        if let Some(header_name) = name.strip_suffix(".cbor.head") {
            let canonical_header = format!("{header_name}.cbor");
            if store.read_attempt_header(&canonical_header)?.is_none() {
                let _gate = store.lock_gate()?;
                let _cleared = store.cleanup_unowned_attempt_locked(&canonical_header)?;
            }
            return Ok(AdoptedRootAdvance::Advanced);
        }
        if let Some(header_name) = name.strip_suffix(".cbor.scan") {
            if !is_lower_digest_name(header_name) {
                return Err(corrupt("hard-scan checkpoint name differs"));
            }
            let canonical_header = format!("{header_name}.cbor");
            if store.read_attempt_header(&canonical_header)?.is_none() {
                let _gate = store.lock_gate()?;
                let _cleared = store.cleanup_unowned_attempt_locked(&canonical_header)?;
            }
            return Ok(AdoptedRootAdvance::Advanced);
        }
        if name.strip_suffix(".cbor").is_none() {
            return Err(corrupt("unexpected attempt entry"));
        }
        self.attempts_seen = self
            .attempts_seen
            .checked_add(1)
            .ok_or_else(|| invalid("attempt count overflow"))?;
        if self.attempts_seen > MAX_ATTEMPTS {
            return Err(invalid("protected attempt count exceeds bound"));
        }
        let record = store
            .read_attempt(name)?
            .ok_or_else(|| corrupt("listed attempt disappeared"))?;
        let journal = store.open_attempt_log(name)?;
        let result = (|| {
            let length = journal.metadata()?.len();
            let head = store.read_journal_head(name)?;
            let remaining = length
                .checked_div(JOURNAL_ENTRY_BYTES)
                .ok_or_else(|| corrupt("attempt journal entry width"))?;
            Ok((remaining, head.committed_chain))
        })();
        let (remaining, expected_chain) = match result {
            Ok(value) => value,
            Err(primary) => return finish_control_read(journal, Err(primary)),
        };
        self.header_roots = record.1.into_iter();
        self.journal = Some(AdoptedJournal {
            file: journal,
            remaining,
            previous: journal_seed(name),
            expected_chain,
        });
        Ok(AdoptedRootAdvance::Advanced)
    }
}

impl ObjectInventoryCursor {
    /// Advances by at most one shard open or one readdir entry. The retained
    /// directory stream avoids quadratic rescans within a populated shard.
    pub(crate) fn next_step(
        &mut self,
        store: &ObjectStore,
    ) -> Result<ObjectInventoryAdvance, CoreError> {
        use std::os::unix::ffi::OsStringExt as _;

        if self.directory.is_none() {
            if self.next_prefix > u16::from(u8::MAX) {
                return Ok(ObjectInventoryAdvance::Done);
            }
            let prefix = u8::try_from(self.next_prefix)
                .map_err(|_conversion_error| invalid("object shard cursor width"))?;
            self.next_prefix = self
                .next_prefix
                .checked_add(1)
                .ok_or_else(|| invalid("object shard cursor overflow"))?;
            let hex = hex_digest(&[prefix; 32]);
            let shard_name = hex.get(..2).ok_or_else(|| invalid("object shard prefix"))?;
            let directory = match openat(
                store.objects.as_ref(),
                shard_name,
                dir_flags(),
                Mode::empty(),
            ) {
                Ok(directory) => File::from(directory),
                Err(rustix::io::Errno::NOENT) => return Ok(ObjectInventoryAdvance::Advanced),
                Err(error) => return Err(storage("open GC object shard", error)),
            };
            store.check_object_shard_device(&directory)?;
            let entries = rustix::fs::Dir::read_from(&directory)
                .map_err(|error| storage("enumerate object shard", error))?;
            self.directory = Some((prefix, directory, entries));
            return Ok(ObjectInventoryAdvance::Advanced);
        }
        let Some((prefix, directory, entries)) = self.directory.as_mut() else {
            return Err(invalid("object inventory cursor lost its shard"));
        };
        let Some(entry) = entries.read() else {
            self.directory = None;
            return Ok(ObjectInventoryAdvance::Advanced);
        };
        let entry = entry.map_err(|error| storage("read object shard entry", error))?;
        let name = std::ffi::OsString::from_vec(entry.file_name().to_bytes().to_vec());
        let Some(name) = name.to_str() else {
            return Err(corrupt("non-UTF8 object entry"));
        };
        if name == "." || name == ".." {
            return Ok(ObjectInventoryAdvance::Advanced);
        }
        if is_object_temporary_name(name, *prefix) {
            let _gate = store.lock_gate()?;
            store.cleanup_pool_temporary(directory, name)?;
            return Ok(ObjectInventoryAdvance::Advanced);
        }
        if name.len() != 64
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(corrupt("noncanonical object entry"));
        }
        let mut digest = [0_u8; 32];
        for (slot, pair) in digest.iter_mut().zip(name.as_bytes().chunks_exact(2)) {
            let high = decode_hex_digit(
                *pair
                    .first()
                    .ok_or_else(|| corrupt("object name pair absent"))?,
            )?;
            let low = decode_hex_digit(
                *pair
                    .get(1)
                    .ok_or_else(|| corrupt("object name pair truncated"))?,
            )?;
            *slot = (high << 4) | low;
        }
        if digest.first() != Some(prefix) {
            return Err(corrupt("object is in the wrong shard"));
        }
        Ok(ObjectInventoryAdvance::Digest(digest))
    }
}

fn strictly_sorted(values: &[ObjectRef]) -> bool {
    values
        .windows(2)
        .all(|pair| match (pair.first(), pair.get(1)) {
            (Some(a), Some(b)) => a < b,
            _ => false,
        })
}

fn canonical_deps(dependencies: &[ObjectRef]) -> Result<Vec<ObjectRef>, CoreError> {
    if dependencies.len() > MAX_OBJECT_DEPS {
        return Err(invalid("object dependency count exceeds bound"));
    }
    let mut deps = dependencies.to_vec();
    deps.sort();
    deps.dedup();
    Ok(deps)
}

fn encode_ref(buffer: &mut Vec<u8>, reference: &ObjectRef) {
    buffer.push(reference.kind.code());
    buffer.extend_from_slice(&reference.bytes.to_be_bytes());
    buffer.extend_from_slice(&reference.sha256);
}

fn batch_manifest_payload(
    name: &str,
    head: &JournalHead,
    count: usize,
) -> Result<Vec<u8>, CoreError> {
    let count = u16::try_from(count).map_err(|_conversion_error| invalid("batch count width"))?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(82)
        .map_err(|error| storage("reserve batch manifest payload", error))?;
    payload.extend_from_slice(ADOPTION_BATCH_MAGIC);
    payload.extend_from_slice(&<[u8; 32]>::from(Sha256::digest(name.as_bytes())));
    payload.extend_from_slice(&head.committed_length.to_be_bytes());
    payload.extend_from_slice(&head.committed_chain);
    payload.extend_from_slice(&count.to_be_bytes());
    Ok(payload)
}

fn adoption_entries(
    previous: [u8; 32],
    references: &[ObjectRef],
) -> Result<(Vec<u8>, [u8; 32]), CoreError> {
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(
            references
                .len()
                .checked_mul(73)
                .ok_or_else(|| invalid("batched journal byte count overflow"))?,
        )
        .map_err(|error| storage("reserve batched journal entries", error))?;
    let mut chain = previous;
    for reference in references {
        let start = entries.len();
        encode_ref(&mut entries, reference);
        let mut hasher = Sha256::new();
        hasher.update(b"lexical-attempt-adoption-v1");
        hasher.update(chain);
        hasher.update(
            entries
                .get(start..)
                .ok_or_else(|| invalid("batched journal entry offset"))?,
        );
        chain = hasher.finalize().into();
        entries.extend_from_slice(&chain);
    }
    Ok((entries, chain))
}

fn decode_ref(bytes: &[u8], offset: &mut usize) -> Result<ObjectRef, CoreError> {
    let end = offset
        .checked_add(41)
        .ok_or_else(|| corrupt("reference offset overflow"))?;
    let slice = bytes
        .get(*offset..end)
        .ok_or_else(|| corrupt("truncated object reference"))?;
    *offset = end;
    let kind = ObjectKind::from_code(
        *slice
            .first()
            .ok_or_else(|| corrupt("empty object reference"))?,
    )
    .ok_or_else(|| corrupt("unknown object reference kind"))?;
    let mut width = [0_u8; 8];
    width.copy_from_slice(
        slice
            .get(1..9)
            .ok_or_else(|| corrupt("truncated reference width"))?,
    );
    let mut sha256 = [0_u8; 32];
    sha256.copy_from_slice(
        slice
            .get(9..41)
            .ok_or_else(|| corrupt("truncated reference digest"))?,
    );
    Ok(ObjectRef {
        kind,
        bytes: u64::from_be_bytes(width),
        sha256,
    })
}

fn encode_object(
    kind: ObjectKind,
    dependencies: &[ObjectRef],
    payload: &[u8],
) -> Result<Vec<u8>, CoreError> {
    let dependencies = canonical_deps(dependencies)?;
    if payload.len() > MAX_OBJECT_PAYLOAD {
        return Err(invalid("object payload exceeds physical block bound"));
    }
    let len = 21_usize
        .checked_add(
            dependencies
                .len()
                .checked_mul(41)
                .ok_or_else(|| invalid("object dependency length overflow"))?,
        )
        .and_then(|length| length.checked_add(payload.len()))
        .ok_or_else(|| invalid("encoded object length overflow"))?;
    if len > MAX_OBJECT_BYTES {
        return Err(invalid("encoded object exceeds physical bound"));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|error| storage("reserve object encoding", error))?;
    bytes.extend_from_slice(OBJECT_MAGIC);
    bytes.extend_from_slice(&OBJECT_FORMAT.to_be_bytes());
    bytes.push(kind.code());
    bytes.extend_from_slice(
        &u16::try_from(dependencies.len())
            .map_err(|_conversion_error| invalid("dependency count width"))?
            .to_be_bytes(),
    );
    bytes.extend_from_slice(
        &u64::try_from(payload.len())
            .map_err(|_conversion_error| invalid("payload length width"))?
            .to_be_bytes(),
    );
    for dependency in &dependencies {
        encode_ref(&mut bytes, dependency);
    }
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

fn decode_object(bytes: &[u8]) -> Result<(ObjectKind, Vec<ObjectRef>, Vec<u8>), CoreError> {
    if bytes.len() < 21 || bytes.get(..8) != Some(OBJECT_MAGIC.as_slice()) {
        return Err(corrupt("bad object header"));
    }
    let format = u16::from_be_bytes(
        bytes
            .get(8..10)
            .ok_or_else(|| corrupt("truncated format"))?
            .try_into()
            .map_err(|_conversion_error| corrupt("format width"))?,
    );
    if format != OBJECT_FORMAT {
        return Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
            message: format!(
                "lexical object store: unsupported persisted object format {format}; this build supports {OBJECT_FORMAT}"
            ),
        });
    }
    let kind = ObjectKind::from_code(
        *bytes
            .get(10)
            .ok_or_else(|| corrupt("truncated object kind"))?,
    )
    .ok_or_else(|| corrupt("unknown object kind"))?;
    let count = usize::from(u16::from_be_bytes(
        bytes
            .get(11..13)
            .ok_or_else(|| corrupt("truncated dependency count"))?
            .try_into()
            .map_err(|_conversion_error| corrupt("dependency count width"))?,
    ));
    let mut payload_len_bytes = [0_u8; 8];
    payload_len_bytes.copy_from_slice(
        bytes
            .get(13..21)
            .ok_or_else(|| corrupt("truncated payload length"))?,
    );
    let payload_len = usize::try_from(u64::from_be_bytes(payload_len_bytes))
        .map_err(|_conversion_error| corrupt("payload length width"))?;
    if count > MAX_OBJECT_DEPS || payload_len > MAX_OBJECT_PAYLOAD {
        return Err(corrupt("object bounds exceeded"));
    }
    let expected = 21_usize
        .checked_add(
            count
                .checked_mul(41)
                .ok_or_else(|| corrupt("dependency length overflow"))?,
        )
        .and_then(|value| value.checked_add(payload_len))
        .ok_or_else(|| corrupt("object length overflow"))?;
    if bytes.len() != expected {
        return Err(corrupt("object encoded length differs"));
    }
    let mut cursor = 21;
    let mut deps = Vec::new();
    deps.try_reserve_exact(count)
        .map_err(|error| storage("reserve dependencies", error))?;
    for _ in 0..count {
        deps.push(decode_ref(bytes, &mut cursor)?);
    }
    if !strictly_sorted(&deps) {
        return Err(corrupt("object dependencies are not canonical"));
    }
    Ok((
        kind,
        deps,
        bytes
            .get(cursor..)
            .ok_or_else(|| corrupt("payload cursor beyond object"))?
            .to_vec(),
    ))
}

fn encode_blob_node(children: &[(ObjectRef, u64)]) -> Result<(Vec<u8>, Vec<ObjectRef>), CoreError> {
    if children.is_empty() || children.len() > NODE_FANOUT {
        return Err(invalid("blob node fanout"));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(
            3_usize
                .checked_add(
                    children
                        .len()
                        .checked_mul(49)
                        .ok_or_else(|| invalid("blob node length overflow"))?,
                )
                .ok_or_else(|| invalid("blob node length overflow"))?,
        )
        .map_err(|error| storage("reserve blob node", error))?;
    bytes.push(1);
    bytes.extend_from_slice(
        &u16::try_from(children.len())
            .map_err(|_conversion_error| invalid("blob child count width"))?
            .to_be_bytes(),
    );
    let mut deps = Vec::new();
    deps.try_reserve_exact(children.len())
        .map_err(|error| storage("reserve blob dependencies", error))?;
    for (reference, length) in children {
        encode_ref(&mut bytes, reference);
        bytes.extend_from_slice(&length.to_be_bytes());
        deps.push(reference.clone());
    }
    Ok((bytes, deps))
}

fn decode_blob_node(bytes: &[u8]) -> Result<Vec<(ObjectRef, u64)>, CoreError> {
    let count_bytes = bytes
        .get(..2)
        .ok_or_else(|| corrupt("blob node count is truncated"))?;
    let count = usize::from(u16::from_be_bytes(
        count_bytes
            .try_into()
            .map_err(|_conversion_error| corrupt("blob child count width"))?,
    ));
    if count == 0
        || count > NODE_FANOUT
        || bytes.len()
            != 2_usize
                .checked_add(
                    count
                        .checked_mul(49)
                        .ok_or_else(|| corrupt("blob node length overflow"))?,
                )
                .ok_or_else(|| corrupt("blob node length overflow"))?
    {
        return Err(corrupt("blob node count or length differs"));
    }
    let mut offset = 2;
    let mut children = Vec::new();
    children
        .try_reserve_exact(count)
        .map_err(|error| storage("reserve blob children", error))?;
    for _ in 0..count {
        let reference = decode_ref(bytes, &mut offset)?;
        let end = offset
            .checked_add(8)
            .ok_or_else(|| corrupt("blob child offset overflow"))?;
        let mut width = [0_u8; 8];
        width.copy_from_slice(
            bytes
                .get(offset..end)
                .ok_or_else(|| corrupt("truncated blob child length"))?,
        );
        offset = end;
        let length = u64::from_be_bytes(width);
        if length == 0 {
            return Err(corrupt("zero-length blob child"));
        }
        children.push((reference, length));
    }
    Ok(children)
}

#[cfg(test)]
mod journal_tests {
    use super::*;
    use crate::lexical_store::{MapEdit, MapRoot};

    fn physical_pool_inventory(store: &ObjectStore) -> (u64, u64) {
        let mut objects = 0_u64;
        let mut bytes = 0_u64;
        store
            .visit_object_digests(&mut |digest| {
                let directory = store.object_dir(digest, false)?;
                let name = hex_digest(digest);
                let object = File::from(
                    openat(
                        &directory,
                        name.as_str(),
                        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                        Mode::empty(),
                    )
                    .map_err(|error| storage("open inventoried object", error))?,
                );
                objects = objects
                    .checked_add(1)
                    .ok_or_else(|| invalid("physical inventory count overflow"))?;
                bytes = bytes
                    .checked_add(
                        object
                            .metadata()
                            .map_err(|error| storage("stat inventoried object", error))?
                            .len(),
                    )
                    .ok_or_else(|| invalid("physical inventory byte overflow"))?;
                Ok(())
            })
            .expect("physical pool inventory");
        (objects, bytes)
    }

    #[test]
    fn attempt_write_cost_matches_pool_inventory_across_nonbatch_and_batch_flushes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        assert_eq!(physical_pool_inventory(&store), (0, 0));

        let nonbatch = store
            .put_adopted_object(&lease, ObjectKind::Record, &[], b"nonbatch")
            .expect("new nonbatched object");
        assert_eq!(
            store
                .put_adopted_object(&lease, ObjectKind::Record, &[], b"nonbatch")
                .expect("reused nonbatched object"),
            nonbatch
        );
        let cost = lease.write_cost_snapshot().expect("nonbatch cost");
        assert_eq!(cost.new_objects, 1);
        assert_eq!(cost.new_encoded_bytes, nonbatch.bytes);
        assert_eq!(cost.reused_objects, 1);
        assert_eq!(cost.reused_encoded_bytes, nonbatch.bytes);
        assert_eq!(physical_pool_inventory(&store), (1, nonbatch.bytes));

        let mut batched_reference = None;
        store
            .with_adoption_batch(&lease, |scoped| {
                let first = scoped.put_adopted_object(
                    &lease,
                    ObjectKind::Record,
                    &[],
                    b"explicit batch",
                )?;
                batched_reference = Some(first.clone());
                scoped.flush_adoption_batch()?;
                let repeated = scoped.put_adopted_object(
                    &lease,
                    ObjectKind::Record,
                    &[],
                    b"explicit batch",
                )?;
                assert_eq!(first, repeated);
                scoped.flush_adoption_batch()?;
                for number in 0..129_u16 {
                    let _reference = scoped.put_adopted_object(
                        &lease,
                        ObjectKind::Record,
                        &[],
                        &number.to_be_bytes(),
                    )?;
                }
                Ok(())
            })
            .expect("explicit and automatic batch flushes");
        let cost = lease.write_cost_snapshot().expect("whole attempt cost");
        let physical = physical_pool_inventory(&store);
        assert_eq!((cost.new_objects, cost.new_encoded_bytes), physical);
        assert_eq!(cost.reused_objects, 2);
        assert_eq!(
            cost.reused_encoded_bytes,
            nonbatch.bytes + batched_reference.expect("batched reference").bytes
        );
        // 129 references force one automatic flush; four batch manifests
        // (two explicit, two automatic/final) are included in the inventory.
        assert_eq!(cost.new_objects, 1 + 1 + 129 + 4);
    }

    #[test]
    fn closure_object_counter_overflow_keeps_product_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let body = store
            .put_blob(&lease, ObjectKind::FileBody, &[], b"closure body")
            .expect("body");
        let mut totals = crate::causal_profile::ClosureReadTotalsV1 {
            objects_read: u64::MAX,
            bytes_read: u64::MAX - 1,
            objects_hashed: u64::MAX,
            bytes_hashed: u64::MAX - 1,
            objects_decoded: u64::MAX,
            bytes_decoded: u64::MAX - 1,
            read_elapsed_ns: u64::MAX,
            hash_elapsed_ns: u64::MAX,
            decode_elapsed_ns: u64::MAX,
            ..crate::causal_profile::ClosureReadTotalsV1::default()
        };
        let (dependencies, payload) = store
            .read_object_inner(&body.root, None, false, Some(&mut totals))
            .expect("authenticated object read");
        assert!(dependencies.is_empty());
        assert!(!payload.is_empty());
        assert_eq!(totals.objects_read, u64::MAX);
        assert_eq!(totals.bytes_read, u64::MAX - 1);
        assert_eq!(totals.objects_hashed, u64::MAX);
        assert_eq!(totals.bytes_hashed, u64::MAX - 1);
        assert_eq!(totals.objects_decoded, u64::MAX);
        assert_eq!(totals.bytes_decoded, u64::MAX - 1);
        assert_eq!(totals.read_elapsed_ns, u64::MAX);
        assert_eq!(totals.hash_elapsed_ns, u64::MAX);
        assert_eq!(totals.decode_elapsed_ns, u64::MAX);
        assert_eq!(
            totals.overflowed
                & (crate::causal_profile::ClosureReadTotalsV1::READ_ELAPSED
                    | crate::causal_profile::ClosureReadTotalsV1::HASH_ELAPSED
                    | crate::causal_profile::ClosureReadTotalsV1::DECODE_ELAPSED
                    | crate::causal_profile::ClosureReadTotalsV1::OBJECTS_READ
                    | crate::causal_profile::ClosureReadTotalsV1::BYTES_READ
                    | crate::causal_profile::ClosureReadTotalsV1::OBJECTS_HASHED
                    | crate::causal_profile::ClosureReadTotalsV1::BYTES_HASHED
                    | crate::causal_profile::ClosureReadTotalsV1::OBJECTS_DECODED
                    | crate::causal_profile::ClosureReadTotalsV1::BYTES_DECODED),
            crate::causal_profile::ClosureReadTotalsV1::READ_ELAPSED
                | crate::causal_profile::ClosureReadTotalsV1::HASH_ELAPSED
                | crate::causal_profile::ClosureReadTotalsV1::DECODE_ELAPSED
                | crate::causal_profile::ClosureReadTotalsV1::OBJECTS_READ
                | crate::causal_profile::ClosureReadTotalsV1::BYTES_READ
                | crate::causal_profile::ClosureReadTotalsV1::OBJECTS_HASHED
                | crate::causal_profile::ClosureReadTotalsV1::BYTES_HASHED
                | crate::causal_profile::ClosureReadTotalsV1::OBJECTS_DECODED
                | crate::causal_profile::ClosureReadTotalsV1::BYTES_DECODED
        );
        lease.discard_unsealed().expect("retire attempt");
    }

    #[test]
    fn fixed_object_header_distinguishes_unsupported_format_from_corruption() {
        // Literal persisted header: magic, format 2, Record kind, zero deps,
        // and zero payload bytes. The current writer emits format 1.
        let mut old_format = b"QLSTOR01\x00\x02\x01\x00\x00".to_vec();
        old_format.extend_from_slice(&[0_u8; 8]);
        let unsupported = decode_object(&old_format);
        assert!(matches!(
            &unsupported,
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                message,
            }) if message.contains("format 2") && message.contains("supports 1")
        ));

        let mut unknown_kind = old_format;
        *unknown_kind.get_mut(9).expect("format byte") = 1;
        *unknown_kind.get_mut(10).expect("kind byte") = 255;
        assert!(matches!(
            decode_object(&unknown_kind),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
    }

    #[test]
    fn attempt_record_retains_fixed_two_field_cbor_shape() {
        let record = AttemptRecord(binding(), Vec::new());
        let encoded = crate::channel_payloads::encode_cbor(&record, "attempt golden")
            .expect("encode attempt record");
        let mut golden = vec![0x82, 0x88, 0x90];
        golden.extend([7_u8; 16]);
        for byte in [8_u8, 11, 9] {
            golden.extend([0x98, 0x20]);
            golden.extend([byte; 32]);
        }
        golden.extend([0x01, 0xf6, 0x98, 0x20]);
        golden.extend([10_u8; 32]);
        golden.extend([0x01, 0x80]);
        assert_eq!(encoded, golden);
        let decoded: AttemptRecord =
            crate::channel_payloads::decode_cbor_exact(&golden).expect("decode fixed attempt");
        assert_eq!(decoded.0, record.0);
        assert!(decoded.1.is_empty());
        let mut extra_field = golden.clone();
        *extra_field.first_mut().expect("array header") = 0x83;
        extra_field.push(0x00);
        assert!(crate::channel_payloads::decode_cbor_exact::<AttemptRecord>(&extra_field).is_err());
        let _last = golden.pop().expect("nonempty record");
        assert!(crate::channel_payloads::decode_cbor_exact::<AttemptRecord>(&golden).is_err());
    }

    fn one_row_map(store: &ObjectStore, lease: &AttemptLease, key: &[u8]) -> MapRoot {
        let value = store
            .put_blob(lease, ObjectKind::Record, &[], key)
            .expect("record blob");
        store
            .map_bootstrap(
                lease,
                std::iter::once(Ok(MapEdit {
                    key: key.to_vec(),
                    value: Some(value),
                })),
            )
            .expect("one-row map")
    }

    #[test]
    fn request_map_page_cache_meters_only_physical_reads_and_reauthenticates_next_request() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let root = one_row_map(&store, &lease, b"alpha");
        let page = root.page.as_ref().expect("map page").object.clone();

        let query = store.metered_clone();
        assert!(
            query
                .map_get(&root, b"alpha", None)
                .expect("first lookup")
                .is_some()
        );
        let first = query.read_meter_snapshot();
        assert_eq!(first, (page.bytes, 1));
        assert!(
            query
                .map_get(&root, b"alpha", None)
                .expect("cached lookup")
                .is_some()
        );
        assert_eq!(query.read_meter_snapshot(), first);

        let expired = RequestBudgetV1::until(std::time::Instant::now());
        assert!(query.map_get(&root, b"alpha", Some(&expired)).is_err());
        assert_eq!(query.read_meter_snapshot(), first);

        let shard = hex_digest(&page.sha256);
        let path = dir
            .path()
            .join(POOL_DIR)
            .join(OBJECTS_DIR)
            .join(shard.get(..2).expect("digest prefix"))
            .join(&shard);
        let mut bytes = std::fs::read(&path).expect("page bytes");
        let last = bytes.last_mut().expect("nonempty page");
        *last ^= 1;
        std::fs::write(&path, bytes).expect("same-length mutation");
        let fresh = store.metered_clone();
        assert!(matches!(
            fresh.map_get(&root, b"alpha", None),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
        assert_eq!(fresh.read_meter_snapshot().1, 1);
    }

    #[test]
    fn request_map_page_cache_evicts_oldest_page_at_bounded_capacity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let a = one_row_map(&store, &lease, b"alpha");
        let b = one_row_map(&store, &lease, b"beta");
        let query = store.metered_clone();
        assert!(
            query
                .map_get(&a, b"alpha", None)
                .expect("a lookup")
                .is_some()
        );
        let meter = query.read_meter.as_ref().expect("query meter");
        let one_page_charge = {
            let cache = meter.objects_cache.lock().expect("cache lock");
            assert!(cache.retained_bytes <= REQUEST_OBJECT_CACHE_BYTES);
            assert!(cache.entries.values().all(|entry| entry.decoded.is_some()));
            cache
                .entries
                .values()
                .next()
                .expect("cached page")
                .charged_bytes
        };
        {
            let mut cache = meter.objects_cache.lock().expect("cache lock");
            cache.limit_bytes = one_page_charge;
        }
        assert!(
            query
                .map_get(&b, b"beta", None)
                .expect("b lookup")
                .is_some()
        );
        assert_eq!(query.read_meter_snapshot().1, 2);
        assert!(
            query
                .map_get(&a, b"alpha", None)
                .expect("a reread")
                .is_some()
        );
        assert_eq!(query.read_meter_snapshot().1, 3);
        assert!(
            meter
                .objects_cache
                .lock()
                .expect("cache lock")
                .retained_bytes
                <= one_page_charge
        );
    }

    #[test]
    fn request_metadata_cache_reuses_authenticated_objects_and_refuses_forged_descriptors() {
        for kind in [
            ObjectKind::Record,
            ObjectKind::Segment,
            ObjectKind::FilePostings,
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let store = ObjectStore::open(dir.path()).expect("store");
            let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
            let reference = store
                .put_blob(&lease, kind, &[], b"authenticated metadata")
                .expect("metadata blob");
            let query = store.metered_clone();
            assert_eq!(
                query.read_blob(&reference, 1024, None).expect("first read"),
                b"authenticated metadata"
            );
            let first = query.read_meter_snapshot();
            assert_eq!(first, (reference.root.bytes, 1));
            let _cached = query
                .read_blob(&reference, 1024, None)
                .expect("cached read");
            assert_eq!(query.read_meter_snapshot(), first);
            let expired = RequestBudgetV1::until(std::time::Instant::now());
            assert!(query.read_blob(&reference, 1024, Some(&expired)).is_err());
            assert_eq!(query.read_meter_snapshot(), first);

            let mut forged_object = reference.root.clone();
            forged_object.bytes += 1;
            assert!(matches!(
                query.read_object(&forged_object, None),
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    ..
                })
            ));
            let mut forged_blob = reference.clone();
            forged_blob.sha256[0] ^= 1;
            assert!(matches!(
                query.read_blob(&forged_blob, 1024, None),
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    ..
                })
            ));

            let shard = hex_digest(&reference.root.sha256);
            let path = dir
                .path()
                .join(POOL_DIR)
                .join(OBJECTS_DIR)
                .join(shard.get(..2).expect("digest prefix"))
                .join(&shard);
            let mut bytes = std::fs::read(&path).expect("object bytes");
            *bytes.last_mut().expect("nonempty object") ^= 1;
            std::fs::write(path, bytes).expect("same-length mutation");
            let fresh = store.metered_clone();
            assert!(matches!(
                fresh.read_blob(&reference, 1024, None),
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    ..
                })
            ));
            assert_eq!(fresh.read_meter_snapshot().1, 1);
        }
    }

    #[test]
    fn request_metadata_cache_shares_one_bound_and_meters_evicted_rereads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let a = store
            .put_blob(&lease, ObjectKind::Record, &[], b"alpha")
            .expect("record");
        let b = store
            .put_blob(&lease, ObjectKind::Segment, &[], b"beta")
            .expect("segment");
        let query = store.metered_clone();
        let _first = query.read_blob(&a, 1024, None).expect("first read");
        let meter = query.read_meter.as_ref().expect("query meter");
        let limit = {
            let mut cache = meter.objects_cache.lock().expect("cache lock");
            let charge = cache.retained_bytes;
            assert!(charge <= REQUEST_OBJECT_CACHE_BYTES);
            cache.limit_bytes = charge;
            charge
        };
        let _second = query.read_blob(&b, 1024, None).expect("second read");
        let _reread = query.read_blob(&a, 1024, None).expect("evicted reread");
        assert_eq!(query.read_meter_snapshot().1, 3);
        assert!(
            meter
                .objects_cache
                .lock()
                .expect("cache lock")
                .retained_bytes
                <= limit
        );
    }

    fn inline_record_reference(payload: &[u8]) -> ObjectRef {
        let mut encoded_payload = vec![0_u8];
        encoded_payload.extend_from_slice(payload);
        let encoded =
            encode_object(ObjectKind::Record, &[], &encoded_payload).expect("encode fixed record");
        ObjectRef {
            kind: ObjectKind::Record,
            bytes: u64::try_from(encoded.len()).expect("record width"),
            sha256: Sha256::digest(encoded).into(),
        }
    }

    fn binding() -> AttemptBinding {
        AttemptBinding {
            root_incarnation: [7; 16],
            repository_sha256: [8; 32],
            revision_sha256: [11; 32],
            source_event_sha256: [9; 32],
            target_generation: 1,
            base_root_sha256: None,
            operations_sha256: [10; 32],
            format: 1,
        }
    }

    fn pending_entry(store: &ObjectStore, name: &str, root: &ObjectRef) -> Vec<u8> {
        let head = store.read_journal_head(name).expect("head");
        let mut entry = Vec::new();
        encode_ref(&mut entry, root);
        let mut hasher = Sha256::new();
        hasher.update(b"lexical-attempt-adoption-v1");
        hasher.update(head.committed_chain);
        hasher.update(&entry);
        let next: [u8; 32] = hasher.finalize().into();
        store
            .write_journal_head(
                name,
                &JournalHead {
                    committed_length: head.committed_length,
                    committed_chain: head.committed_chain,
                    pending: Some(root.clone()),
                    expected_chain: next,
                },
            )
            .expect("pending head");
        entry.extend_from_slice(&next);
        entry
    }

    fn pending_batch(store: &ObjectStore, name: &str, roots: &[ObjectRef]) -> Vec<u8> {
        let head = store.read_journal_head(name).expect("head");
        let sorted = canonical_deps(roots).expect("sorted refs");
        let payload = batch_manifest_payload(name, &head, sorted.len()).expect("payload");
        let manifest = store
            .put_object_locked(ObjectKind::AttemptBatch, &sorted, &payload)
            .expect("manifest");
        let (entries, final_chain) =
            adoption_entries(head.committed_chain, &sorted).expect("batch entries");
        store
            .write_journal_head(
                name,
                &JournalHead {
                    committed_length: head.committed_length,
                    committed_chain: head.committed_chain,
                    pending: Some(manifest),
                    expected_chain: final_chain,
                },
            )
            .expect("pending batch head");
        entries
    }

    #[test]
    fn adoption_manifest_reservation_matches_existing_object_codec_bound() {
        let head = JournalHead::clean(0, [0; 32]);
        let references = (0..128_u16)
            .map(|index| ObjectRef {
                kind: ObjectKind::Record,
                bytes: 22,
                sha256: Sha256::digest(index.to_be_bytes()).into(),
            })
            .collect::<Vec<_>>();
        let payload = batch_manifest_payload("fixed-attempt", &head, 128).expect("payload");
        assert_eq!(payload.len(), 82);
        let encoded = encode_object(ObjectKind::AttemptBatch, &references, &payload)
            .expect("128-reference manifest");
        assert_eq!(encoded.len(), 5_351);
        assert_eq!(
            u64::try_from(encoded.len()).expect("width"),
            MAX_ADOPTION_BATCH_MANIFEST_BYTES
        );
        let (kind, decoded, decoded_payload) = decode_object(&encoded).expect("decode manifest");
        assert_eq!(kind, ObjectKind::AttemptBatch);
        assert_eq!(decoded.len(), 128);
        assert_eq!(decoded_payload, payload);
        let mut too_many = references;
        too_many.push(ObjectRef {
            kind: ObjectKind::Record,
            bytes: 22,
            sha256: Sha256::digest(128_u16.to_be_bytes()).into(),
        });
        assert!(encode_object(ObjectKind::AttemptBatch, &too_many, &payload).is_err());
    }

    #[test]
    fn batched_adoption_bounds_epoch_and_keeps_direct_root_membership() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let lease = store.begin_attempt(&active, &[]).expect("attempt");
        let before = store.gc_epoch().expect("before epoch");
        let roots = store
            .with_adoption_batch(&lease, |scoped| {
                let mut roots = Vec::new();
                for index in 0..129_u16 {
                    let root = scoped
                        .put_blob(&lease, ObjectKind::Record, &[], &index.to_be_bytes())?
                        .root;
                    roots.push(root);
                }
                Ok(roots)
            })
            .expect("batched writes");
        assert_eq!(
            store.gc_epoch().expect("after epoch"),
            before.saturating_add(2)
        );
        let head = store
            .read_journal_head(&active.identity_name())
            .expect("head");
        assert!(head.pending.is_none());
        assert_eq!(
            head.committed_length,
            129_u64.saturating_mul(JOURNAL_ENTRY_BYTES)
        );
        let adopted = store.adopted_roots().expect("adopted roots");
        assert_eq!(adopted.len(), 129);
        for root in &roots {
            assert!(adopted.contains(root));
        }
        lease
            .finish_published(&[
                roots.first().expect("first").clone(),
                roots.last().expect("last").clone(),
            ])
            .expect("direct committed roots");
    }

    #[test]
    fn renamed_object_with_unsynced_shard_is_repaired_on_all_retry_paths() {
        use crate::publication_faults::{Cut, Side, with_error_at};

        for retry_path in 0_u8..3 {
            let dir = tempfile::tempdir().expect("tempdir");
            let store = ObjectStore::open(dir.path()).expect("store");
            let active = binding();
            let lease = store.begin_attempt(&active, &[]).expect("attempt");
            let payload = [retry_path, 0x7b];
            let expected = inline_record_reference(&payload);
            let (failed, fired) =
                with_error_at(Cut::PoolObjectDirectorySync, Side::Before, 1, || {
                    store.with_adoption_batch(&lease, |scoped| {
                        scoped.put_blob(&lease, ObjectKind::Record, &[], &payload)
                    })
                });
            assert!(fired, "rename-before-shard-sync cut must execute");
            assert!(failed.is_err());
            assert!(store.adopted_roots().expect("uncommitted roots").is_empty());
            let name = hex_digest(&expected.sha256);
            assert!(
                dir.path()
                    .join(POOL_DIR)
                    .join(OBJECTS_DIR)
                    .join(name.get(..2).expect("shard"))
                    .join(&name)
                    .exists(),
                "the failed physical object must have crossed rename"
            );

            let recovered = match retry_path {
                0 => {
                    store
                        .with_adoption_batch(&lease, |scoped| {
                            scoped.put_blob(&lease, ObjectKind::Record, &[], &payload)
                        })
                        .expect("batched retry")
                        .root
                }
                1 => {
                    store
                        .put_blob(&lease, ObjectKind::Record, &[], &payload)
                        .expect("unbatched retry")
                        .root
                }
                _ => {
                    store
                        .adopt_existing_root(&lease, &expected)
                        .expect("inherited root retry");
                    expected.clone()
                }
            };
            assert_eq!(recovered, expected);
            drop(lease);
            let reopened = ObjectStore::open(dir.path()).expect("cold reopen");
            assert!(
                reopened
                    .adopted_roots()
                    .expect("adoption")
                    .contains(&expected)
            );
            let (dependencies, actual) = reopened.read_object(&expected, None).expect("object");
            assert!(dependencies.is_empty());
            assert_eq!(actual.get(..1), Some([0_u8].as_slice()));
            assert_eq!(actual.get(1..), Some(payload.as_slice()));
        }
    }

    #[test]
    fn failed_full_pool_barrier_never_publishes_an_adoption_head() {
        use crate::publication_faults::{Cut, Side, with_error_at};

        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let lease = store.begin_attempt(&active, &[]).expect("attempt");
        let payload = b"barrier-refusal";
        let expected = inline_record_reference(payload);
        let (failed, fired) = with_error_at(Cut::PoolFullBarrier, Side::Before, 1, || {
            store.with_adoption_batch(&lease, |scoped| {
                scoped.put_blob(&lease, ObjectKind::Record, &[], payload)
            })
        });
        assert!(fired, "the pre-head barrier must execute");
        assert!(failed.is_err());
        let head = store
            .read_journal_head(&active.identity_name())
            .expect("head");
        assert!(head.pending.is_none());
        assert_eq!(head.committed_length, 0);
        assert!(store.adopted_roots().expect("uncommitted roots").is_empty());
        drop(lease);

        let reopened = ObjectStore::open(dir.path()).expect("cold reopen");
        assert!(reopened.adopted_roots().expect("cold roots").is_empty());
        let retry = reopened.begin_attempt(&active, &[]).expect("retry attempt");
        let recovered = reopened
            .with_adoption_batch(&retry, |scoped| {
                scoped.put_blob(&retry, ObjectKind::Record, &[], payload)
            })
            .expect("retry after barrier refusal");
        assert_eq!(recovered.root, expected);
        assert!(
            reopened
                .adopted_roots()
                .expect("retried roots")
                .contains(&expected)
        );
    }

    #[test]
    fn reused_batch_manifest_requires_its_own_post_repair_barrier() {
        use crate::publication_faults::{Cut, Side, with_error_at};

        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let lease = store.begin_attempt(&active, &[]).expect("attempt");
        let payload = b"manifest-reuse";
        let expected = inline_record_reference(payload);
        let (manifest_failed, manifest_cut) =
            with_error_at(Cut::PoolObjectDirectorySync, Side::Before, 2, || {
                store.with_adoption_batch(&lease, |scoped| {
                    scoped.put_blob(&lease, ObjectKind::Record, &[], payload)
                })
            });
        assert!(manifest_cut, "second physical object is the batch manifest");
        assert!(manifest_failed.is_err());
        assert!(store.adopted_roots().expect("uncommitted roots").is_empty());

        let (barrier_failed, second_barrier) =
            with_error_at(Cut::PoolFullBarrier, Side::Before, 2, || {
                store.with_adoption_batch(&lease, |scoped| {
                    scoped.put_blob(&lease, ObjectKind::Record, &[], payload)
                })
            });
        assert!(
            second_barrier,
            "reused manifest needs a post-repair barrier"
        );
        assert!(barrier_failed.is_err());
        let head = store
            .read_journal_head(&active.identity_name())
            .expect("head");
        assert!(head.pending.is_none());
        assert_eq!(head.committed_length, 0);

        let recovered = store
            .with_adoption_batch(&lease, |scoped| {
                scoped.put_blob(&lease, ObjectKind::Record, &[], payload)
            })
            .expect("durable manifest retry");
        assert_eq!(recovered.root, expected);
        assert!(store.adopted_roots().expect("roots").contains(&expected));
        let cost = lease.write_cost_snapshot().expect("retry write cost");
        let (physical_objects, physical_bytes) = physical_pool_inventory(&store);
        assert_eq!(physical_objects, 2);
        // The first manifest survived rename but failed before verification;
        // both later invocations authenticate and reuse both physical objects.
        assert_eq!(cost.new_objects, 1);
        assert_eq!(cost.new_encoded_bytes, expected.bytes);
        assert_eq!(cost.reused_objects, 4);
        assert_eq!(cost.reused_encoded_bytes, physical_bytes * 2);
    }

    #[test]
    fn lease_rejects_an_independently_replaced_object_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let pool = dir.path().join(POOL_DIR);
        std::fs::rename(pool.join(OBJECTS_DIR), pool.join("objects-retired"))
            .expect("replace object directory");
        std::fs::create_dir(pool.join(OBJECTS_DIR)).expect("new object directory");
        let reopened = ObjectStore::open_existing(dir.path()).expect("new pinned store");
        assert!(
            reopened
                .put_blob(&lease, ObjectKind::Record, &[], b"wrong pinned tree")
                .is_err()
        );
    }

    #[test]
    fn gc_object_device_guard_refuses_foreign_shards() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let shard = store.object_dir(&[0; 32], true).expect("local shard");
        store
            .check_object_shard_device(&shard)
            .expect("same-device shard");
        let foreign_device = store.objects_device.wrapping_add(1);
        assert!(store.check_object_device(foreign_device).is_err());
    }

    #[test]
    fn explicit_batch_flush_releases_gate_and_next_write_starts_new_epoch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let before = store.gc_epoch().expect("before epoch");
        let roots = store
            .with_adoption_batch(&lease, |scoped| {
                let first = scoped
                    .put_blob(&lease, ObjectKind::Record, &[], b"one")?
                    .root;
                scoped.flush_adoption_batch()?;
                // An independent handle must acquire the gate after flush.
                let midpoint = store.gc_epoch()?;
                assert_eq!(midpoint, before.saturating_add(1));
                let second = scoped
                    .put_blob(&lease, ObjectKind::Record, &[], b"two")?
                    .root;
                Ok([first, second])
            })
            .expect("two groups");
        assert_eq!(
            store.gc_epoch().expect("after epoch"),
            before.saturating_add(2)
        );
        let adopted = store.adopted_roots().expect("adopted");
        assert!(roots.iter().all(|root| adopted.contains(root)));
    }

    #[test]
    fn encoded_byte_bound_splits_before_reference_bound() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let before = store.gc_epoch().expect("before epoch");
        let references = store
            .with_adoption_batch(&lease, |scoped| {
                let mut references = Vec::new();
                let mut payload = vec![0_u8; MAX_OBJECT_PAYLOAD];
                for index in 0..32_u8 {
                    *payload.first_mut().expect("physical payload") = index;
                    references.push(scoped.put_adopted_object(
                        &lease,
                        ObjectKind::Record,
                        &[],
                        &payload,
                    )?);
                }
                Ok(references)
            })
            .expect("bounded physical groups");
        assert_eq!(references.len(), 32);
        let first_group_width = references
            .iter()
            .take(31)
            .map(|reference| reference.bytes)
            .sum::<u64>();
        assert!(
            first_group_width.saturating_add(MAX_ADOPTION_BATCH_MANIFEST_BYTES)
                <= MAX_ADOPTION_BATCH_BYTES
        );
        assert!(
            references
                .iter()
                .map(|reference| reference.bytes)
                .sum::<u64>()
                > MAX_ADOPTION_BATCH_DATA_BYTES
        );
        // 32 references fit the count policy; a second epoch proves the
        // physical byte limit, including manifest reserve, split the group.
        assert_eq!(
            store.gc_epoch().expect("after epoch"),
            before.saturating_add(2)
        );
        assert_eq!(store.adopted_roots().expect("adopted").len(), 32);
    }

    #[test]
    fn batched_map_bootstrap_and_apply_finish_without_gate_reentry() {
        use crate::lexical_store::MapEdit;
        use std::sync::mpsc;
        use std::time::Duration;

        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = (|| -> Result<(), CoreError> {
                let dir =
                    tempfile::tempdir().map_err(|error| storage("map test tempdir", error))?;
                let store = ObjectStore::open(dir.path())?;
                let lease = store.begin_attempt(&binding(), &[])?;
                store.with_adoption_batch(&lease, |scoped| {
                    let first = scoped.put_blob(&lease, ObjectKind::Record, &[], b"first")?;
                    let second = scoped.put_blob(&lease, ObjectKind::Record, &[], b"second")?;
                    let base = scoped.map_bootstrap(
                        &lease,
                        std::iter::once(Ok(MapEdit {
                            key: b"key".to_vec(),
                            value: Some(first),
                        })),
                    )?;
                    let updated = scoped.map_apply(
                        &lease,
                        &base,
                        std::iter::once(Ok(MapEdit {
                            key: b"key".to_vec(),
                            value: Some(second.clone()),
                        })),
                    )?;
                    if scoped.map_get(&updated, b"key", None)? != Some(second) {
                        return Err(invalid("batched map result differs"));
                    }
                    Ok(())
                })
            })();
            let _sent = sender.send(result);
        });
        receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("batched map operation stalled on its own gate")
            .expect("batched map result");
        worker.join().expect("map worker join");
    }

    #[test]
    fn batched_pending_partial_and_full_tails_recover_exact_refs() {
        for complete in [false, true] {
            let dir = tempfile::tempdir().expect("tempdir");
            let store = ObjectStore::open(dir.path()).expect("store");
            let active = binding();
            let lease = store.begin_attempt(&active, &[]).expect("attempt");
            let first = store
                .put_blob(&lease, ObjectKind::Record, &[], b"first")
                .expect("first")
                .root;
            let second = store
                .put_blob(&lease, ObjectKind::Record, &[], b"second")
                .expect("second")
                .root;
            let name = active.identity_name();
            let before = store
                .read_journal_head(&name)
                .expect("before head")
                .committed_length;
            let expected = pending_batch(&store, &name, &[first.clone(), second.clone()]);
            let mut journal = File::from(
                openat(
                    store.attempts.as_ref(),
                    journal_name(&name).as_str(),
                    OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                    Mode::empty(),
                )
                .expect("journal"),
            );
            let _offset = journal.seek(SeekFrom::End(0)).expect("seek tail");
            let written = if complete {
                expected.as_slice()
            } else {
                expected.get(..37).expect("partial")
            };
            journal.write_all(written).expect("tail write");
            journal.sync_all().expect("tail sync");
            drop(lease);
            let reopened = ObjectStore::open(dir.path()).expect("reopen");
            let replay = reopened.begin_attempt(&active, &[]).expect("recover batch");
            let head = reopened.read_journal_head(&name).expect("recovered head");
            assert!(head.pending.is_none());
            assert_eq!(
                head.committed_length,
                before.saturating_add(2_u64.saturating_mul(JOURNAL_ENTRY_BYTES))
            );
            let roots = reopened.adopted_roots().expect("recovered roots");
            assert!(roots.contains(&first));
            assert!(roots.contains(&second));
            replay.discard_unsealed().expect("discard replay");
        }
    }

    #[test]
    fn full_width_pending_batch_recovers_empty_partial_and_complete_tails() {
        // Fixed 128-reference groups exercise the codec/recovery boundary;
        // expected journal lengths do not use the writer's group constant.
        for written_bytes in [0_usize, 4_709, 9_344] {
            let dir = tempfile::tempdir().expect("tempdir");
            let store = ObjectStore::open(dir.path()).expect("store");
            let active = binding();
            let lease = store.begin_attempt(&active, &[]).expect("attempt");
            let roots = store
                .with_adoption_batch(&lease, |scoped| {
                    (0..128_u16)
                        .map(|index| {
                            scoped
                                .put_blob(&lease, ObjectKind::Record, &[], &index.to_be_bytes())
                                .map(|blob| blob.root)
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .expect("full-width group");
            let name = active.identity_name();
            assert_eq!(
                store
                    .read_journal_head(&name)
                    .expect("head")
                    .committed_length,
                9_344
            );
            let expected = pending_batch(&store, &name, &roots);
            assert_eq!(expected.len(), 9_344);
            let mut journal = File::from(
                openat(
                    store.attempts.as_ref(),
                    journal_name(&name).as_str(),
                    OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                    Mode::empty(),
                )
                .expect("journal"),
            );
            let _offset = journal.seek(SeekFrom::End(0)).expect("seek tail");
            journal
                .write_all(expected.get(..written_bytes).expect("fixed tail length"))
                .expect("write tail");
            journal.sync_all().expect("tail sync");
            drop(journal);
            drop(lease);
            let reopened = ObjectStore::open(dir.path()).expect("reopen");
            let replay = reopened
                .begin_attempt(&active, &[])
                .expect("recover full group");
            let head = reopened.read_journal_head(&name).expect("recovered head");
            assert!(head.pending.is_none());
            assert_eq!(head.committed_length, 18_688);
            let adopted = reopened.adopted_roots().expect("adopted roots");
            assert_eq!(adopted.len(), 128);
            assert!(roots.iter().all(|root| adopted.contains(root)));
            replay.discard_unsealed().expect("discard replay");
        }
    }

    #[test]
    fn batched_manifest_corruption_refuses_retry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let lease = store.begin_attempt(&active, &[]).expect("attempt");
        let root = store
            .put_blob(&lease, ObjectKind::Record, &[], b"v")
            .expect("blob")
            .root;
        let name = active.identity_name();
        let _entries = pending_batch(&store, &name, &[root]);
        let pending = store
            .read_journal_head(&name)
            .expect("pending head")
            .pending
            .expect("manifest");
        let path = dir
            .path()
            .join("lexical-object-pool")
            .join("objects")
            .join(hex_digest(&pending.sha256).get(..2).expect("prefix"))
            .join(hex_digest(&pending.sha256));
        let mut bytes = std::fs::read(&path).expect("manifest bytes");
        *bytes.last_mut().expect("manifest not empty") ^= 1;
        std::fs::write(path, bytes).expect("corrupt manifest");
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).expect("reopen");
        assert!(reopened.begin_attempt(&active, &[]).is_err());
        assert!(reopened.adopted_roots().is_err());
    }

    #[test]
    fn batched_pending_wrong_chain_refuses_retry_without_repair() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let lease = store.begin_attempt(&active, &[]).expect("attempt");
        let root = store
            .put_blob(&lease, ObjectKind::Record, &[], b"v")
            .expect("blob")
            .root;
        let name = active.identity_name();
        let _entries = pending_batch(&store, &name, &[root]);
        let mut head = store.read_journal_head(&name).expect("pending head");
        head.expected_chain = [42; 32];
        store.write_journal_head(&name, &head).expect("wrong chain");
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).expect("reopen");
        assert!(reopened.begin_attempt(&active, &[]).is_err());
        assert!(reopened.adopted_roots().is_err());
    }

    #[test]
    fn escaped_batched_views_refuse_writes_after_error_or_unwind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let mut escaped = None;
        let failure: Result<(), CoreError> = store.with_adoption_batch(&lease, |scoped| {
            escaped = Some(scoped.clone());
            let _root = scoped.put_blob(&lease, ObjectKind::Record, &[], b"error")?;
            Err(invalid("injected callback failure"))
        });
        assert!(failure.is_err());
        let closed = escaped.take().expect("escaped error view");
        assert!(
            closed
                .put_blob(&lease, ObjectKind::Record, &[], b"late")
                .is_err()
        );
        let gate = store.lock_gate().expect("gate released after error");
        drop(gate);
        let mut escaped_panic = None;
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop(store.with_adoption_batch::<()>(&lease, |scoped| {
                escaped_panic = Some(scoped.clone());
                let _root = scoped.put_blob(&lease, ObjectKind::Record, &[], b"panic")?;
                panic!("injected unwind");
            }));
        }));
        assert!(unwind.is_err());
        let closed_panic = escaped_panic.expect("escaped panic view");
        assert!(
            closed_panic
                .put_blob(&lease, ObjectKind::Record, &[], b"late")
                .is_err()
        );
        let _gate = store.lock_gate().expect("gate released after unwind");
    }

    #[test]
    fn retry_cleans_uncommitted_initial_journal_before_header_publication() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let name = active.identity_name();
        store
            .write_atomic_at(
                store.attempts.as_ref(),
                &journal_name(&name),
                &[],
                AtomicPurpose::Other,
            )
            .expect("initial journal");
        store
            .write_journal_head(&name, &JournalHead::clean(0, journal_seed(&name)))
            .expect("initial head");
        let reopened = ObjectStore::open(dir.path()).expect("reopen");
        let lease = reopened
            .begin_attempt(&active, &[])
            .expect("retry initial attempt");
        assert!(
            reopened
                .read_attempt(&name)
                .expect("committed attempt")
                .is_some()
        );
        lease.discard_unsealed().expect("discard");
    }

    #[test]
    fn retry_recovers_after_initial_journal_without_head() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let name = active.identity_name();
        store
            .write_atomic_at(
                store.attempts.as_ref(),
                &journal_name(&name),
                &[],
                AtomicPurpose::Other,
            )
            .expect("initial journal");
        let reopened = ObjectStore::open(dir.path()).expect("reopen");
        let lease = reopened
            .begin_attempt(&active, &[])
            .expect("retry initial attempt");
        assert!(
            reopened
                .read_attempt(&name)
                .expect("committed attempt")
                .is_some()
        );
        lease.discard_unsealed().expect("discard");
    }

    #[test]
    fn gc_recovers_orphan_initial_controls_without_protecting_objects() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let name = binding().identity_name();
        store
            .write_atomic_at(
                store.attempts.as_ref(),
                &journal_name(&name),
                &[],
                AtomicPurpose::Other,
            )
            .expect("initial journal");
        store
            .write_journal_head(&name, &JournalHead::clean(0, journal_seed(&name)))
            .expect("initial head");
        assert!(
            store
                .adopted_roots()
                .expect("recover initial controls")
                .is_empty()
        );
        for component in [journal_name(&name), journal_head_name(&name)] {
            assert_eq!(
                openat(
                    store.attempts.as_ref(),
                    component.as_str(),
                    OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                    Mode::empty(),
                )
                .expect_err("uncommitted control removed"),
                rustix::io::Errno::NOENT
            );
        }
    }

    #[test]
    fn same_binding_retry_restores_only_the_pending_adoption() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let lease = store.begin_attempt(&active, &[]).expect("attempt");
        let root = store
            .put_blob(&lease, ObjectKind::Record, &[], b"v")
            .expect("blob")
            .root;
        let name = active.identity_name();
        let original_length = store
            .read_journal_head(&name)
            .expect("original head")
            .committed_length;
        let entry = pending_entry(&store, &name, &root);
        let mut journal = File::from(
            openat(
                store.attempts.as_ref(),
                journal_name(&name).as_str(),
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .expect("journal"),
        );
        let _offset = journal.seek(SeekFrom::End(0)).expect("seek");
        journal
            .write_all(entry.get(..17).expect("partial entry"))
            .expect("partial write");
        journal.sync_all().expect("partial sync");
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).expect("reopen");
        let replay = reopened
            .begin_attempt(&active, &[])
            .expect("recover same binding");
        assert!(reopened.adopted_roots().expect("roots").contains(&root));
        let head = reopened.read_journal_head(&name).expect("clean head");
        assert!(head.pending.is_none());
        assert_eq!(
            head.committed_length,
            original_length.saturating_add(JOURNAL_ENTRY_BYTES)
        );
        replay.discard_unsealed().expect("discard");
    }

    #[test]
    fn completed_adoption_truncation_refuses_retry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let lease = store.begin_attempt(&active, &[]).expect("attempt");
        let _root = store
            .put_blob(&lease, ObjectKind::Record, &[], b"v")
            .expect("blob");
        let name = active.identity_name();
        let journal = File::from(
            openat(
                store.attempts.as_ref(),
                journal_name(&name).as_str(),
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .expect("journal"),
        );
        journal.set_len(0).expect("truncate complete adoption");
        journal.sync_all().expect("sync truncation");
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).expect("reopen");
        assert!(reopened.begin_attempt(&active, &[]).is_err());
        assert!(reopened.adopted_roots().is_err());
    }

    #[test]
    fn complete_pending_entry_is_synced_and_committed_on_retry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let lease = store.begin_attempt(&active, &[]).expect("attempt");
        let root = store
            .put_blob(&lease, ObjectKind::Record, &[], b"v")
            .expect("blob")
            .root;
        let name = active.identity_name();
        let entry = pending_entry(&store, &name, &root);
        let mut journal = File::from(
            openat(
                store.attempts.as_ref(),
                journal_name(&name).as_str(),
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .expect("journal"),
        );
        let _offset = journal.seek(SeekFrom::End(0)).expect("seek");
        journal.write_all(&entry).expect("full pending entry");
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).expect("reopen");
        let replay = reopened
            .begin_attempt(&active, &[])
            .expect("recover full pending entry");
        assert!(reopened.adopted_roots().expect("roots").contains(&root));
        replay.discard_unsealed().expect("discard");
    }

    #[test]
    fn retirement_marker_finishes_partial_unlink_before_gc_marking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let lease = store.begin_attempt(&active, &[]).expect("attempt");
        let name = active.identity_name();
        let record = store
            .read_attempt(&name)
            .expect("read attempt")
            .expect("record");
        store
            .write_retirement_marker(&name, &record)
            .expect("durable marker");
        unlinkat(store.attempts.as_ref(), name.as_str(), AtFlags::empty())
            .expect("partial retirement");
        store.attempts.sync_all().expect("sync partial retirement");
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).expect("reopen");
        assert!(
            reopened
                .adopted_roots()
                .expect("retirement recovery")
                .is_empty()
        );
        assert!(
            reopened
                .read_retirement_marker(&name)
                .expect("marker probe")
                .is_none()
        );
        assert!(
            reopened
                .read_attempt(&name)
                .expect("attempt probe")
                .is_none()
        );
    }

    #[test]
    fn absent_target_cleanup_finishes_partial_retirement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let target = binding();
        let lease = store.begin_attempt(&target, &[]).expect("attempt");
        let name = target.identity_name();
        let record = store
            .read_attempt(&name)
            .expect("read attempt")
            .expect("record");
        store
            .write_retirement_marker(&name, &record)
            .expect("durable marker");
        unlinkat(store.attempts.as_ref(), name.as_str(), AtFlags::empty())
            .expect("partial retirement");
        store.attempts.sync_all().expect("sync partial retirement");
        drop(lease);
        assert_eq!(
            store
                .discard_target_attempts(
                    target.repository_sha256,
                    target.revision_sha256,
                    target.target_generation,
                )
                .expect("absent target cleanup"),
            0
        );
        assert!(
            store
                .read_retirement_marker(&name)
                .expect("marker probe")
                .is_none()
        );
        assert!(store.read_attempt(&name).expect("attempt probe").is_none());
    }

    #[test]
    fn gc_inventory_removes_only_crash_orphaned_object_temporaries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let digest = [7_u8; 32];
        let shard = store.object_dir(&digest, true).expect("shard");
        let temp_name = format!(".{}.tmp-17-4", hex_digest(&digest));
        let mut temp = File::from(
            openat(
                &shard,
                temp_name.as_str(),
                OFlags::WRONLY | OFlags::CLOEXEC | OFlags::CREATE | OFlags::EXCL,
                Mode::RUSR | Mode::WUSR,
            )
            .expect("staged object"),
        );
        temp.write_all(b"interrupted").expect("body");
        temp.sync_all().expect("durable staged object");
        drop(temp);
        let mut digests = Vec::new();
        store
            .visit_object_digests(&mut |observed| {
                digests.push(*observed);
                Ok(())
            })
            .expect("GC inventory");
        assert!(digests.is_empty());
        assert_eq!(
            openat(
                &shard,
                temp_name.as_str(),
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .expect_err("orphan temp removed"),
            rustix::io::Errno::NOENT
        );
    }

    #[test]
    fn maintenance_removes_crash_orphaned_control_temporaries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let attempt_name = format!(".{}.tmp-17-4", binding().identity_name());
        let epoch_name = format!(".{EPOCH_FILE}.tmp-17-5");
        for (parent, name) in [
            (store.attempts.as_ref(), attempt_name.as_str()),
            (store.pool.as_ref(), epoch_name.as_str()),
        ] {
            let mut file = File::from(
                openat(
                    parent,
                    name,
                    OFlags::WRONLY | OFlags::CLOEXEC | OFlags::CREATE | OFlags::EXCL,
                    Mode::RUSR | Mode::WUSR,
                )
                .expect("staged control"),
            );
            file.write_all(b"interrupted").expect("body");
            file.sync_all().expect("durable staged control");
        }
        assert!(
            store
                .adopted_roots()
                .expect("attempt maintenance")
                .is_empty()
        );
        store
            .visit_object_digests(&mut |_digest| Ok(()))
            .expect("pool maintenance");
        for (parent, name) in [
            (store.attempts.as_ref(), attempt_name.as_str()),
            (store.pool.as_ref(), epoch_name.as_str()),
        ] {
            assert_eq!(
                openat(
                    parent,
                    name,
                    OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                    Mode::empty(),
                )
                .expect_err("orphan control temp removed"),
                rustix::io::Errno::NOENT
            );
        }
    }

    #[test]
    fn absent_target_cleanup_matches_repository_revision_and_generation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let target = binding();
        let other = AttemptBinding {
            revision_sha256: [12; 32],
            ..target
        };
        let target_lease = store.begin_attempt(&target, &[]).expect("target attempt");
        let other_lease = store
            .begin_attempt(&other, &[])
            .expect("other revision attempt");
        let scopes = store
            .pending_target_scopes()
            .expect("pending target scopes");
        assert_eq!(scopes.len(), 2);
        assert!(scopes.contains(&(
            target.repository_sha256,
            target.revision_sha256,
            target.target_generation,
        )));
        assert_eq!(
            store
                .discard_target_attempts(
                    target.repository_sha256,
                    target.revision_sha256,
                    target.target_generation,
                )
                .expect("exact absent target cleanup"),
            1
        );
        assert!(
            store
                .read_attempt(&target.identity_name())
                .expect("target read")
                .is_none()
        );
        assert!(
            store
                .read_attempt(&other.identity_name())
                .expect("other read")
                .is_some()
        );
        drop(target_lease);
        other_lease.discard_unsealed().expect("cleanup other");
    }

    #[test]
    fn sealed_target_settlement_retires_after_authenticating_committed_roots() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let target = binding();
        let lease = store.begin_attempt(&target, &[]).expect("attempt");
        let root = store
            .put_blob(&lease, ObjectKind::Record, &[], b"sealed")
            .expect("adopted object")
            .root;
        drop(lease); // Simulates a crash after the independently sealed identity.
        assert_eq!(
            store
                .finish_target_attempts(
                    target.repository_sha256,
                    target.revision_sha256,
                    target.target_generation,
                    target.source_event_sha256,
                    std::slice::from_ref(&root),
                )
                .expect("sealed target settlement"),
            1
        );
        assert!(
            store
                .adopted_roots()
                .expect("remaining attempts")
                .is_empty()
        );
    }

    #[test]
    fn sealed_target_settlement_retires_older_incomplete_same_event_attempt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let current = binding();
        let older = AttemptBinding {
            root_incarnation: [43; 16],
            ..current
        };
        let old_lease = store.begin_attempt(&older, &[]).expect("older attempt");
        let current_lease = store.begin_attempt(&current, &[]).expect("current attempt");
        let body = store
            .put_blob(&current_lease, ObjectKind::Record, &[], b"sealed")
            .expect("current adopted root");
        drop(old_lease);
        drop(current_lease);
        assert_eq!(
            store
                .finish_target_attempts(
                    current.repository_sha256,
                    current.revision_sha256,
                    current.target_generation,
                    current.source_event_sha256,
                    std::slice::from_ref(&body.root),
                )
                .expect("sealed settlement"),
            2
        );
        assert!(
            store
                .adopted_roots()
                .expect("remaining attempts")
                .is_empty()
        );
        let mut sealed_live = BTreeSet::new();
        let _inserted = sealed_live.insert(body.root.sha256);
        let epoch = store.gc_epoch().expect("settled epoch");
        assert!(
            store
                .gc_sweep_batch(epoch, &[body.root.sha256], &sealed_live)
                .expect("protected GC batch")
        );
        assert_eq!(
            store
                .read_blob(&body, 6, None)
                .expect("sealed closure retained"),
            b"sealed"
        );
    }

    #[test]
    fn sealed_target_settlement_refuses_conflicting_source_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::open(dir.path()).expect("store");
        let target = binding();
        let conflicting = AttemptBinding {
            source_event_sha256: [42; 32],
            ..target
        };
        let first = store.begin_attempt(&target, &[]).expect("first attempt");
        let second = store
            .begin_attempt(&conflicting, &[])
            .expect("conflicting attempt");
        assert!(
            store
                .finish_target_attempts(
                    target.repository_sha256,
                    target.revision_sha256,
                    target.target_generation,
                    target.source_event_sha256,
                    &[],
                )
                .is_err()
        );
        assert!(
            store
                .read_attempt(&target.identity_name())
                .expect("first record")
                .is_some()
        );
        assert!(
            store
                .read_attempt(&conflicting.identity_name())
                .expect("conflicting record")
                .is_some()
        );
        first.discard_unsealed().expect("discard first");
        second.discard_unsealed().expect("discard second");
    }
}
