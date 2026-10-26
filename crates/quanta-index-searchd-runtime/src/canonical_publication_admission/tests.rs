use std::error::Error;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Arc;

use lance_io::object_store::file_admission::FileAdmission as _;
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchCorpusIngestBatch};
use quanta_index_core::publication_admission::{
    PublicationAdmissionRequest, PublicationBuildWork, PublicationOutputClass,
    PublicationOutputReservation, PublicationResourceAdmissionPort as _,
    validate_control_object_lifetime, validate_object_lifetime,
};
use quanta_index_core::publication_capacity::PublicationCapacity;
use quanta_index_core::{
    CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, IngestBatchFootprint,
    PublicationValidationOwner, REQUEST_CANCELLED_CODE, RequestBudgetV1,
};
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd_harness::SourceCorpusFixture;

use super::{CanonicalPublicationAdmission, PublicationAllowanceLimit};
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
use crate::publication_lance_io::PublicationLanceAdmission;

type TestResult = Result<(), Box<dyn Error>>;

macro_rules! check {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err(
                io::Error::other(concat!("assertion failed: ", stringify!($condition))).into(),
            );
        }
    };
}
macro_rules! check_eq {
    ($left:expr, $right:expr $(,)?) => {
        check!($left == $right);
    };
}

type Fixture = (
    tempfile::TempDir,
    PathBuf,
    PublicationCapacity,
    Arc<PublicationFileCapacity>,
    CanonicalPublicationAdmission,
);

fn fixture() -> Result<Fixture, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let requested = directory.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let authority = seed_publication_capacity(
        &lease,
        &root,
        PublicationBootInventoryLimits {
            max_bytes: 1024,
            max_files: 32,
            max_pending: 32,
            max_entries: 64,
            max_depth: 8,
            max_pending_frames: 16,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let binder = PublicationFileCapacity::new(lease, authority.clone(), 16, 64, 256)?;
    let admission = CanonicalPublicationAdmission::new(
        Arc::new(PublicationFileDeletions::new(Arc::clone(&binder))),
        PublicationAllowanceLimit::new(2, 1)?,
        PublicationAllowanceLimit::new(4, 2)?,
    );
    Ok((directory, root, authority, binder, admission))
}

#[test]
fn same_ledger_different_root_lifetime_fails_before_object_work() -> TestResult {
    let (_directory, _root, authority, _binder, admission) = fixture()?;
    let other_directory = tempfile::tempdir()?;
    let requested = other_directory.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    let other_lease = StateRootLease::acquire(&requested)?;
    let other_root = other_lease.state_root_identity_v1().to_path_buf();
    let other_binder = PublicationFileCapacity::new(other_lease, authority.clone(), 16, 64, 256)?;
    let other_admission = CanonicalPublicationAdmission::new(
        Arc::new(PublicationFileDeletions::new(other_binder)),
        PublicationAllowanceLimit::new(2, 1)?,
        PublicationAllowanceLimit::new(4, 2)?,
    );
    let baseline = authority.snapshot()?;
    let batch = batch()?;
    let budget = RequestBudgetV1::unbounded();
    let mut owner = PublicationValidationOwner::default();
    owner.admit_publication_control(&batch, &budget, &admission)?;
    owner.admit_publication_resources(&request(&batch), &budget, &admission)?;
    let control = owner
        .output_admission()
        .owned_control_admission()?
        .ok_or("held Control authority missing")?;
    let work = owner
        .output_admission()
        .owned_work_admission()?
        .ok_or("held work authority missing")?;
    let owned_lifetime = admission.object_lifetime();
    let foreign_lifetime = other_admission.object_lifetime();
    let control_parent = control
        .held_file_allowance()?
        .ok_or("held Control file allowance missing")?;
    let parent = work
        .held_file_allowance()?
        .ok_or("held file allowance missing")?;
    // Both binders accept this ledger's allowance. The owned capability must
    // still reject the wrong registry/root before any object path is touched.
    foreign_lifetime.validate_parent(&parent)?;
    foreign_lifetime.validate_parent(&control_parent)?;
    check!(Arc::ptr_eq(&owned_lifetime, &admission.object_lifetime()));
    check!(matches!(
        validate_control_object_lifetime(Some(&control), Some(&foreign_lifetime)),
        Err(CoreError::InvalidContract(_))
    ));
    validate_control_object_lifetime(Some(&control), Some(&owned_lifetime))?;
    check!(matches!(
        validate_object_lifetime(Some(&work), Some(&foreign_lifetime)),
        Err(CoreError::InvalidContract(_))
    ));
    validate_object_lifetime(Some(&work), Some(&owned_lifetime))?;
    check!(
        !other_root
            .join("indexes/lexical/lexical-object-pool")
            .exists()
    );
    drop(parent);
    drop(control_parent);
    drop(control);
    drop(work);
    drop(owner);
    check_eq!(authority.snapshot()?, baseline);
    Ok(())
}

#[test]
fn control_only_owned_child_retains_parent_without_work_authority() -> TestResult {
    let (_directory, root, authority, binder, admission) = fixture()?;
    let baseline = authority.snapshot()?;
    let batch = batch()?;
    let mut owner = PublicationValidationOwner::default();
    owner.admit_publication_control(&batch, &RequestBudgetV1::unbounded(), &admission)?;
    check!(matches!(
        owner.output_admission().owned_work_admission(),
        Err(CoreError::InvalidContract(_))
    ));

    let control = owner
        .output_admission()
        .owned_control_admission()?
        .ok_or("held Control authority missing")?;
    let control_clone = control.clone();
    let lifetime = admission.object_lifetime();
    validate_control_object_lifetime(Some(&control), Some(&lifetime))?;
    let work_request = PublicationOutputReservation::new(PublicationOutputClass::Semantic, 1, 1)?;
    check!(matches!(
        control.reserve(work_request),
        Err(CoreError::InvalidContract(_))
    ));

    let control_request = PublicationOutputReservation::new(PublicationOutputClass::Control, 1, 1)?;
    let mut child = control.reserve(control_request)?;
    check!(control_clone.reserve(control_request).is_err());
    drop(owner);
    drop(control);
    drop(control_clone);
    let held = authority.snapshot()?;
    check_eq!(
        held.used_bytes,
        baseline
            .used_bytes
            .checked_add(2)
            .ok_or("test oracle addition overflow")?
    );
    check_eq!(
        held.used_files,
        baseline
            .used_files
            .checked_add(1)
            .ok_or("test oracle addition overflow")?
    );
    let staging = root.join("control-child.tmp");
    let target = root.join("control-child");
    child.bind_output(&staging, &target)?;
    check!(!staging.exists() && !target.exists());
    check_eq!(authority.snapshot()?, held);
    drop(child);
    check_eq!(authority.snapshot()?, baseline);
    check!(binder.is_quiescent()?);
    Ok(())
}

fn contains_budget_cause(error: &(dyn Error + 'static)) -> bool {
    let mut source = Some(error);
    while let Some(current) = source {
        if matches!(
            current.downcast_ref::<CoreError>(),
            Some(CoreError::Typed { code, .. }) if *code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ) {
            return true;
        }
        source = current
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .map(|inner| -> &(dyn Error + 'static) { inner })
            .or_else(|| current.source());
    }
    false
}

#[test]
fn control_child_retry_drains_retired_temporary_from_original_charge() -> TestResult {
    use std::io::Write as _;
    let (_directory, root, authority, binder, admission) = fixture()?;
    let baseline = authority.snapshot()?;
    let batch = batch()?;
    let mut owner = PublicationValidationOwner::default();
    owner.admit_publication_control(&batch, &RequestBudgetV1::unbounded(), &admission)?;
    let control = owner
        .output_admission()
        .owned_control_admission()?
        .ok_or("Control absent")?;
    let request = PublicationOutputReservation::new(PublicationOutputClass::Control, 1, 1)?;
    let staging = root.join("control-retry.tmp");
    let target = root.join("control-retry");
    let mut child = control.reserve(request)?;
    child.bind_output(&staging, &target)?;
    child.creation_started()?;
    let mut writer = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    child.created(&writer, &staging)?;
    writer.write_all(b"x")?;
    child.retire_writer_preserving_pending(writer)?;
    drop(child);
    check_eq!(
        authority.snapshot()?.uncertain_pending,
        baseline
            .uncertain_pending
            .checked_add(1)
            .ok_or("pending overflow")?
    );
    // The same held parent is full until its original pending child settles.
    let retry = control.reserve(request)?;
    check!(!staging.exists() && !target.exists());
    check_eq!(
        authority.snapshot()?.uncertain_pending,
        baseline.uncertain_pending
    );
    drop(retry);
    drop(control);
    drop(owner);
    check_eq!(authority.snapshot()?, baseline);
    check!(binder.is_quiescent()?);
    Ok(())
}

#[test]
fn new_control_parent_recovers_published_inode_without_domain_success() -> TestResult {
    use std::io::Write as _;
    let (_directory, root, authority, binder, admission) = fixture()?;
    let baseline = authority.snapshot()?;
    let batch = batch()?;
    let mut owner = PublicationValidationOwner::default();
    owner.admit_publication_control(&batch, &RequestBudgetV1::unbounded(), &admission)?;
    let control = owner
        .output_admission()
        .owned_control_admission()?
        .ok_or("Control absent")?;
    let request = PublicationOutputReservation::new(PublicationOutputClass::Control, 1, 1)?;
    let staging = root.join("control-published.tmp");
    let target = root.join("control-published");
    let mut child = control.reserve(request)?;
    child.bind_output(&staging, &target)?;
    child.creation_started()?;
    let mut writer = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    child.created(&writer, &staging)?;
    writer.write_all(b"x")?;
    child.before_replace()?;
    fs::rename(&staging, &target)?;
    child.retire_writer_preserving_pending(writer)?;
    drop(child);
    drop(control);
    drop(owner);
    let mut retry = PublicationValidationOwner::default();
    retry.admit_publication_control(&batch, &RequestBudgetV1::unbounded(), &admission)?;
    drop(retry);
    check_eq!(fs::read(&target)?, b"x");
    let retained = authority.snapshot()?;
    check_eq!(retained.uncertain_pending, baseline.uncertain_pending);
    check_eq!(
        retained.used_bytes,
        baseline.used_bytes.checked_add(1).ok_or("byte overflow")?
    );
    check_eq!(
        retained.used_files,
        baseline.used_files.checked_add(1).ok_or("file overflow")?
    );
    check!(binder.is_quiescent()?);
    Ok(())
}

fn batch() -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut producer = SourceCorpusFixture::new(
        RepoId::new("admission-fixture")?,
        RevisionId::new("revision-1")?,
        ManifestGeneration::new(1),
        "admission-event".into(),
    );
    Ok(producer.frozen_batch()?)
}

fn request(batch: &SearchCorpusIngestBatch) -> PublicationAdmissionRequest<'_> {
    PublicationAdmissionRequest {
        batch,
        footprint: IngestBatchFootprint {
            carried_records: 0,
            embedded_records: 0,
            source_bytes: 0,
            text_bytes: 0,
            vector_bytes: 0,
        },
        work: PublicationBuildWork::FinalizeOnly,
    }
}

#[test]
fn control_and_work_have_disjoint_charge_and_refund_provisional_drop() -> TestResult {
    let (_directory, root, authority, _binder, admission) = fixture()?;
    let batch = batch()?;
    let baseline = authority.snapshot()?;
    let budget = RequestBudgetV1::unbounded();
    let control = admission.try_acquire_control(&batch, &budget)?;
    let work = admission.try_acquire(&request(&batch), &budget)?;
    let charged = authority.snapshot()?;
    check_eq!(
        charged.used_bytes,
        baseline
            .used_bytes
            .checked_add(6)
            .ok_or("test oracle addition overflow")?
    );
    check_eq!(
        charged.used_files,
        baseline
            .used_files
            .checked_add(3)
            .ok_or("test oracle addition overflow")?
    );
    check_eq!(
        charged.active_allowances,
        baseline
            .active_allowances
            .checked_add(2)
            .ok_or("test oracle addition overflow")?
    );
    check!(control.held_file_allowance()?.is_some());
    check!(work.held_file_allowance()?.is_some());

    let leaf_request = PublicationOutputReservation::new(PublicationOutputClass::Semantic, 3, 1)?;
    let mut leaf = work.reserve_output(leaf_request)?;
    let target = root.join("provisional");
    leaf.bind_output(&root.join("provisional.tmp"), &target)?;
    check_eq!(
        authority.snapshot()?.active_tickets,
        baseline
            .active_tickets
            .checked_add(1)
            .ok_or("test oracle addition overflow")?
    );
    drop(leaf);
    check!(!target.exists());
    check_eq!(authority.snapshot()?, charged);

    drop(work);
    let control_only = authority.snapshot()?;
    check_eq!(
        control_only.used_bytes,
        baseline
            .used_bytes
            .checked_add(2)
            .ok_or("test oracle addition overflow")?
    );
    check_eq!(
        control_only.used_files,
        baseline
            .used_files
            .checked_add(1)
            .ok_or("test oracle addition overflow")?
    );
    drop(control);
    check_eq!(authority.snapshot()?, baseline);
    Ok(())
}

#[test]
fn output_class_and_growth_stay_within_the_held_parent() -> TestResult {
    let (_directory, root, authority, binder, admission) = fixture()?;
    let batch = batch()?;
    let baseline = authority.snapshot()?;
    let budget = RequestBudgetV1::unbounded();
    let control = admission.try_acquire_control(&batch, &budget)?;
    let work = admission.try_acquire(&request(&batch), &budget)?;
    let control_output = PublicationOutputReservation::new(PublicationOutputClass::Control, 1, 1)?;
    let semantic_output =
        PublicationOutputReservation::new(PublicationOutputClass::Semantic, 1, 1)?;
    check!(control.reserve_output(semantic_output).is_err());
    check!(work.reserve_output(control_output).is_err());
    drop(control.reserve_output(control_output)?);
    drop(work.reserve_output(semantic_output)?);

    let parent = work
        .held_file_allowance()?
        .ok_or("work allowance missing")?;
    let deletions = Arc::new(PublicationFileDeletions::new(binder));
    let lance = PublicationLanceAdmission::for_work(deletions, parent)?;
    let target = root.join("bounded-lance");
    let mut writer = lance.begin_create(&target)?;
    check!(matches!(writer.reserve_end(5), Err(error) if contains_budget_cause(&error)));
    check!(!target.exists());
    drop(writer);
    drop(lance);
    drop(work);
    drop(control);
    check_eq!(authority.snapshot()?, baseline);
    Ok(())
}

#[test]
fn cancelled_budget_refuses_before_any_parent_reservation() -> TestResult {
    let (_directory, _root, authority, _binder, admission) = fixture()?;
    let batch = batch()?;
    let baseline = authority.snapshot()?;
    let budget = RequestBudgetV1::unbounded();
    budget.cancel_handle().cancel();
    check!(matches!(
        admission.try_acquire_control(&batch, &budget),
        Err(CoreError::Typed { code, .. }) if code == REQUEST_CANCELLED_CODE
    ));
    check!(matches!(
        admission.try_acquire(&request(&batch), &budget),
        Err(CoreError::Typed { code, .. }) if code == REQUEST_CANCELLED_CODE
    ));
    check_eq!(authority.snapshot()?, baseline);
    Ok(())
}

#[test]
fn limits_require_positive_bytes_and_files() {
    assert!(PublicationAllowanceLimit::new(0, 1).is_err());
    assert!(PublicationAllowanceLimit::new(1, 0).is_err());
    assert!(PublicationAllowanceLimit::new(1, 1).is_ok());
}
