//! Real temporary files exercise the complete Semantic wrapper. These finite
//! test ports are not the installed Legacy root/Source/close authority.
use super::*;
use quanta_index_core::index::{ObjectReadFile, WorkUsage};
use quanta_index_core::{ExistingTreeEntryKindV1, SealedArtifactCommitmentV1};
use std::{
    ffi::OsStr,
    fs::{self, File},
    os::unix::fs::MetadataExt as _,
    path::PathBuf,
};

#[derive(Clone, Copy, Debug, Default)]
enum Fault {
    #[default]
    None,
    SidecarChange,
    Quarantine,
    ReadRefusal,
}

#[derive(Debug)]
struct Files {
    root: PathBuf,
    fault: Fault,
    opens: AtomicU64,
    visits: AtomicU64,
    marker_opens: AtomicU64,
    root_visits: AtomicU64,
    live: Arc<AtomicU64>,
}
struct Opened {
    file: Option<File>,
    live: Arc<AtomicU64>,
    _memory: MemoryLease,
}
impl ObjectReadFile for Opened {
    fn file(&self) -> Result<&File, IndexError> {
        self.file.as_ref().ok_or(IndexError::Conflict)
    }
}
impl Drop for Opened {
    fn drop(&mut self) {
        drop(self.file.take());
        let _prior = self.live.fetch_sub(1, Ordering::SeqCst);
    }
}
impl Files {
    fn contains(&self, path: &Path) -> Result<(), IndexError> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| IndexError::DigestConflict)?;
        if relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    }
}
impl ObjectReadIoPort for Files {
    fn open(
        &self,
        path: &Path,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
        self.contains(path)?;
        budget.checkpoint()?;
        let memory = budget
            .reserve_memory(
                u64::try_from(std::mem::size_of::<Opened>())
                    .map_err(|_| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
            .ok_or(IndexError::ResourceLimited)?;
        if matches!(self.fault, Fault::ReadRefusal) && path.file_name() == Some(OsStr::new("a")) {
            return Err(IndexError::WorkLimitExceeded);
        }
        if path.file_name() == Some(OsStr::new(layout::MARKER_SEALED_FILE_NAME)) {
            let prior = self.marker_opens.fetch_add(1, Ordering::SeqCst);
            if prior == 1 && matches!(self.fault, Fault::SidecarChange) {
                fs::write(path, b"x").map_err(|error| IndexError::Storage(error.to_string()))?;
            }
        }
        let metadata =
            fs::symlink_metadata(path).map_err(|error| IndexError::Storage(error.to_string()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(IndexError::DigestConflict);
        }
        let file = File::open(path).map_err(|error| IndexError::Storage(error.to_string()))?;
        let _prior = self.opens.fetch_add(1, Ordering::SeqCst);
        let _live = self.live.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Opened {
            file: Some(file),
            live: self.live.clone(),
            _memory: memory,
        }))
    }
}
impl ExistingTreeMetadataIoPortV1 for Files {
    fn visit_directory_v1(
        &self,
        path: &Path,
        budget: &mut WorkBudget,
        visit: &mut dyn FnMut(
            &OsStr,
            ExistingTreeEntryKindV1,
            u64,
            &mut WorkBudget,
        ) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        self.contains(path)?;
        budget.charge(WorkUsage {
            reads: 1,
            ..Default::default()
        })?;
        let _directory = memory(budget, 4096)?;
        let _prior = self.visits.fetch_add(1, Ordering::SeqCst);
        if path == self.root
            && self.root_visits.fetch_add(1, Ordering::SeqCst) == 1
            && matches!(self.fault, Fault::Quarantine)
        {
            fs::write(
                path.join(layout::QUARANTINE_RECEIPT_FILE_NAME),
                b"quarantined",
            )?;
        }
        for entry in fs::read_dir(path)? {
            budget.charge(WorkUsage {
                reads: 1,
                ..Default::default()
            })?;
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            let kind = if metadata.file_type().is_symlink() {
                ExistingTreeEntryKindV1::Symlink
            } else if metadata.is_dir() {
                ExistingTreeEntryKindV1::Directory
            } else if metadata.is_file() {
                ExistingTreeEntryKindV1::File
            } else {
                ExistingTreeEntryKindV1::Other
            };
            visit(&entry.file_name(), kind, metadata.len(), budget)?;
        }
        Ok(())
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    expected: GenerationSnapshot,
    owner: u32,
    files: Files,
}
impl Fixture {
    fn new(fault: Fault) -> TestResult<Self> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let owner = fs::metadata(root)?.uid();
        fs::create_dir(root.join(layout::DATASET_DIR_NAME))?;
        fs::write(root.join("dataset/a"), b"abc")?;
        let mut data = valid_codec_data()?;
        let mut seal = crate::legacy::sealed::decode_sealed_manifest_bytes(root, &data[1])?;
        seal.artifacts.push(SealedArtifactCommitmentV1 {
            name: "dataset/a".into(),
            bytes: 3,
            // Published SHA-256("abc"), independent of the verifier/encoder.
            sha256: [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad,
            ],
        });
        data[1] = crate::codec::encode(&seal.to_row(), "physical wrapper fixture")?;
        for (name, bytes) in [
            layout::MARKER_SEALED_FILE_NAME,
            layout::SEALED_MANIFEST_FILE_NAME,
            layout::MANIFEST_FILE_NAME,
            layout::BUILD_CONTRACT_FILE_NAME,
        ]
        .iter()
        .zip(data)
        {
            fs::write(root.join(name), bytes)?;
        }
        let files = Files {
            root: root.to_path_buf(),
            fault,
            opens: AtomicU64::new(0),
            visits: AtomicU64::new(0),
            marker_opens: AtomicU64::new(0),
            root_visits: AtomicU64::new(0),
            live: Arc::new(AtomicU64::new(0)),
        };
        Ok(Self {
            directory,
            expected: expected()?,
            owner,
            files,
        })
    }
    fn edit_scope(&self, edit: impl FnOnce(&mut SemanticManifest)) -> TestResult {
        // Fixture preparation only: retain a valid seal commitment so the
        // requested invariant is tested after integrity-before-interpretation.
        let root = self.directory.path();
        let scope_path = root.join(layout::MANIFEST_FILE_NAME);
        let mut scope = SemanticManifest::decode(&fs::read(&scope_path)?)?;
        edit(&mut scope);
        let bytes = scope.encode()?;
        let seal_path = root.join(layout::SEALED_MANIFEST_FILE_NAME);
        let mut seal =
            crate::legacy::sealed::decode_sealed_manifest_bytes(root, &fs::read(&seal_path)?)?;
        seal.scope_manifest = (u64::try_from(bytes.len())?, Sha256::digest(&bytes).into());
        fs::write(scope_path, bytes)?;
        fs::write(
            seal_path,
            crate::codec::encode(&seal.to_row(), "edited scope fixture")?,
        )?;
        Ok(())
    }
    fn verify<T>(
        &self,
        budget: &mut WorkBudget,
        consumer: impl FnOnce(&SemanticContentRootsV1, &mut WorkBudget) -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        verify_existing_semantic_generation_read_only(
            self.directory.path(),
            &self.expected,
            self.owner,
            &self.files,
            &self.files,
            budget,
            consumer,
        )
    }
    fn retired(&self, memory: &Memory) -> TestResult {
        if self.files.live.load(Ordering::SeqCst) != 0 {
            return Err("physical file owner leaked".into());
        }
        released(memory)
    }
}
fn original() -> (WorkBudget, Arc<Memory>) {
    work(
        32 * ONE_MIB,
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    )
}
fn unreachable_consumer(_: &SemanticContentRootsV1, _: &mut WorkBudget) -> Result<(), Failure> {
    Err(CoreError::InvalidContract("refused physical input reached consumer".into()).into())
}
fn corrupt_result(result: Result<(), Failure>) -> TestResult {
    if !matches!(
        result,
        Err(Failure::Canonical(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
            ..
        }))
    ) {
        return Err(format!("expected canonical physical corruption, got {result:?}").into());
    }
    Ok(())
}

#[test]
fn semantic_existing_real_files_lend_roots_with_original_grants_and_retired_reads() -> TestResult {
    let fixture = Fixture::new(Fault::None)?;
    let (mut budget, memory) = original();
    let supplied = std::ptr::from_ref(&budget);
    fixture.verify(&mut budget, |roots, current| {
        if !std::ptr::eq(supplied, std::ptr::from_ref(current))
            || memory.live.load(Ordering::SeqCst) < ONE_MIB
            || fixture.files.live.load(Ordering::SeqCst) != 0
            || roots.row_root_digest != ROOT
            || roots.membership_root_digest != ROOT
        {
            return Err(CoreError::InvalidContract(
                "physical consumer lost roots, original grants or read retirement".into(),
            )
            .into());
        }
        Ok(())
    })?;
    if fixture.files.marker_opens.load(Ordering::SeqCst) != 2 {
        return Err("sidecars were not reread".into());
    }
    fixture.retired(&memory)
}

#[test]
fn semantic_existing_same_length_payload_tamper_refuses_before_consumer() -> TestResult {
    let fixture = Fixture::new(Fault::None)?;
    fs::write(fixture.directory.path().join("dataset/a"), b"abd")?;
    let (mut budget, memory) = original();
    corrupt_result(fixture.verify(&mut budget, unreachable_consumer))?;
    fixture.retired(&memory)
}

#[test]
fn semantic_existing_missing_or_extra_files_refuse_before_consumer() -> TestResult {
    for missing in [true, false] {
        let fixture = Fixture::new(Fault::None)?;
        if missing {
            fs::remove_file(fixture.directory.path().join("dataset/a"))?;
        } else {
            fs::write(fixture.directory.path().join("dataset/extra"), b"x")?;
        }
        let (mut budget, memory) = original();
        corrupt_result(fixture.verify(&mut budget, unreachable_consumer))?;
        fixture.retired(&memory)?;
    }
    Ok(())
}

#[test]
fn semantic_existing_sidecar_drift_refuses_before_consumer() -> TestResult {
    let fixture = Fixture::new(Fault::SidecarChange)?;
    let (mut budget, memory) = original();
    corrupt_result(fixture.verify(&mut budget, unreachable_consumer))?;
    fixture.retired(&memory)
}

#[test]
fn semantic_existing_late_quarantine_refuses_before_consumer() -> TestResult {
    let fixture = Fixture::new(Fault::Quarantine)?;
    let (mut budget, memory) = original();
    let result = fixture.verify(&mut budget, unreachable_consumer);
    if !matches!(
        result,
        Err(Failure::Canonical(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::GenerationQuarantined,
            ..
        }))
    ) {
        return Err(format!("quarantine cause was lost: {result:?}").into());
    }
    fixture.retired(&memory)
}

#[test]
fn semantic_existing_payload_port_refusal_preserves_original_cause() -> TestResult {
    let fixture = Fixture::new(Fault::ReadRefusal)?;
    let (mut budget, memory) = original();
    let result = fixture.verify(&mut budget, unreachable_consumer);
    if !matches!(
        result,
        Err(Failure::AdmissionOrIo(IndexError::WorkLimitExceeded))
    ) {
        return Err(format!("physical port refusal was rewritten: {result:?}").into());
    }
    fixture.retired(&memory)
}

#[test]
fn semantic_existing_original_refusals_have_no_filesystem_effects() -> TestResult {
    for cancelled in [true, false] {
        let fixture = Fixture::new(Fault::None)?;
        let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        if cancelled {
            request.cancel_handle().cancel();
        }
        let (mut budget, memory) = work(0, request);
        let result = fixture.verify(&mut budget, unreachable_consumer);
        if !matches!(
            (&result, cancelled),
            (Err(Failure::AdmissionOrIo(IndexError::Cancelled)), true)
                | (
                    Err(Failure::AdmissionOrIo(IndexError::ResourceLimited)),
                    false
                )
        ) {
            return Err(format!("original refusal was lost: {result:?}").into());
        }
        if fixture.files.opens.load(Ordering::SeqCst) != 0
            || fixture.files.visits.load(Ordering::SeqCst) != 0
        {
            return Err("refused original performed filesystem work".into());
        }
        fixture.retired(&memory)?;
    }
    Ok(())
}

#[test]
fn semantic_existing_consumer_failure_releases_all_original_grants() -> TestResult {
    let fixture = Fixture::new(Fault::None)?;
    let (mut budget, memory) = original();
    let result = fixture.verify(&mut budget, |_, _| {
        Err::<(), _>(IndexError::Conflict.into())
    });
    if !matches!(result, Err(Failure::AdmissionOrIo(IndexError::Conflict))) {
        return Err(format!("consumer error was rewritten: {result:?}").into());
    }
    fixture.retired(&memory)
}

#[test]
fn semantic_existing_consumer_cancellation_discards_success_and_releases_grants() -> TestResult {
    let fixture = Fixture::new(Fault::None)?;
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let cancel = request.cancel_handle();
    let (mut budget, memory) = work(32 * ONE_MIB, request);
    let result = fixture.verify(&mut budget, |_, _| {
        cancel.cancel();
        Ok(())
    });
    if !matches!(result, Err(Failure::AdmissionOrIo(IndexError::Cancelled))) {
        return Err(format!("cancelled consumer success escaped: {result:?}").into());
    }
    fixture.retired(&memory)
}

#[test]
fn semantic_existing_scope_validation_preserves_normalization_corpus_and_contract_refusals()
-> TestResult {
    for case in 0..3 {
        let fixture = Fixture::new(Fault::None)?;
        fixture.edit_scope(|scope| match case {
            0 => scope.normalization = "unsupported".into(),
            1 => scope.present_corpora = vec!["duplicate".into(), "duplicate".into()],
            _ => scope.model_id = "foreign-model".into(),
        })?;
        let expected = match case {
            0 => "normalization",
            1 => "present_corpora contains duplicates",
            _ => "model_id",
        };
        let (mut budget, memory) = original();
        match fixture.verify(&mut budget, unreachable_consumer) {
            Err(Failure::Canonical(CoreError::Storage(message))) if message.contains(expected) => {}
            other => return Err(format!("canonical {expected} refusal was lost: {other:?}").into()),
        }
        fixture.retired(&memory)?;
    }
    Ok(())
}
