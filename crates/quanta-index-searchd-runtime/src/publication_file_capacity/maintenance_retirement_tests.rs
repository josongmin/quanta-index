use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;

use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
use quanta_index_core::RequestBudgetV1;

type TestResult = Result<(), Box<dyn std::error::Error>>;

type Fixture = (
    tempfile::TempDir,
    StateRootLease,
    PublicationCapacity,
    Arc<PublicationFileCapacity>,
);

fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let requested_root = directory.path().join("state");
    fs::create_dir(&requested_root)?;
    fs::set_permissions(&requested_root, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested_root)?;
    let authority = seed_publication_capacity(
        &lease,
        lease.state_root_identity_v1(),
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
    let binder = PublicationFileCapacity::new_with_maintenance(
        lease.clone(),
        authority.clone(),
        16,
        64,
        256,
        16,
        2,
    )?;
    Ok((directory, lease, authority, binder))
}

#[test]
fn maintenance_retirement_releases_actual_last_parent_and_freezes_admission() -> TestResult {
    let (_directory, lease, authority, binder) = fixture()?;
    let before = authority.snapshot()?;
    if before.active_allowances != 1 || !binder.is_quiescent()? {
        return Err(format!("unexpected held maintenance baseline: {before:?}").into());
    }
    if binder.maintenance_available()? != (16, 2) {
        return Err("maintenance reserve has wrong fixed width".into());
    }
    let external = binder
        .maintenance_parent()?
        .ok_or("configured maintenance parent is absent")?;
    if !matches!(
        binder.retire_maintenance_reserve_checked(),
        Err(CoreError::NotReady(_))
    ) || authority.snapshot()?.active_allowances != 1
    {
        return Err("external maintenance parent was retired".into());
    }
    let weak = Arc::downgrade(&external);
    drop(external);
    if !matches!(
        binder.retire_maintenance_reserve_checked(),
        Err(CoreError::NotReady(_))
    ) || authority.snapshot()?.active_allowances != 1
    {
        return Err("retirement raced an upgradeable maintenance owner".into());
    }
    drop(weak);
    binder.retire_maintenance_reserve_checked()?;
    binder.retire_maintenance_reserve_checked()?;
    let after = authority.snapshot()?;
    if after.active_allowances != 0
        || after.active_tickets != 0
        || after.uncertain_pending != 0
        || after.poisoned
        || !binder.is_quiescent()?
    {
        return Err(format!("maintenance did not physically retire: {after:?}").into());
    }
    if !matches!(
        binder.maintenance_parent(),
        Err(CoreError::InvalidContract(_))
    ) || !matches!(
        binder.maintenance_available(),
        Err(CoreError::InvalidContract(_))
    ) || !matches!(
        binder.refill_maintenance_reserve(),
        Err(CoreError::InvalidContract(_))
    ) || !matches!(
        binder.refresh_maintenance_reserve(),
        Err(CoreError::InvalidContract(_))
    ) || !matches!(
        binder.reserve_publication_allowance(1, 1),
        Err(CoreError::InvalidContract(_))
    ) {
        return Err("retired binder admitted a new maintenance or writer operation".into());
    }
    lease.require_state_root_v1(lease.state_root_identity_v1())?;
    Ok(())
}

#[test]
fn maintenance_retirement_refuses_other_core_allowances_without_freezing_recovery() -> TestResult {
    let (_directory, lease, authority, binder) = fixture()?;
    let other = binder.reserve_publication_allowance(4, 1)?;
    if !matches!(
        binder.retire_maintenance_reserve_checked(),
        Err(CoreError::NotReady(_))
    ) || authority.snapshot()?.active_allowances != 2
    {
        return Err("retirement discarded a non-maintenance parent".into());
    }
    let next = binder.reserve_publication_allowance(1, 1)?;
    drop(next);
    drop(other);
    binder.retire_maintenance_reserve_checked()?;
    if authority.snapshot()?.active_allowances != 0 {
        return Err("maintenance parent remained charged after retry".into());
    }
    lease.require_state_root_v1(lease.state_root_identity_v1())?;
    Ok(())
}
