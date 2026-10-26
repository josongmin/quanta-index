//! Selection, physical authentication and same-lease service handoff.
//! Catalog is the sole selection issuer; no second durable head is introduced.

#[path = "native_boot/catalog_digest.rs"]
mod catalog_digest;
#[path = "native_boot/receipt.rs"]
mod receipt;

use std::{fs, sync::Arc};

use anyhow::{Result, anyhow, bail};
use quanta_index_catalog::{
    NativeBootManifest, SqliteCatalog, verify_native_boot_candidate, verify_native_boot_catalog,
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{IndexError, ObjectReadIoPort, WorkBudget, effort::ReservationGuard},
};
use quanta_index_kernel::{AdapterRegistry, verify_native_boot_inputs};
use quanta_index_searchd::{
    app::runtime::StateRootLease,
    cli::{NativeBootIdentityV1, NativeCommandModeV1, NativeStateCommandV1},
};
use quanta_index_storage::FileObjectReader;

use super::ValidatedConfig;
use crate::{
    native::{NativeBootAuthority, prepare_native_boot_authority, verify_existing_native_format},
    native_object_read_io::NativeObjectReadIo,
    native_resources::{NativeResourceClass, NativeResourceLedger},
};

fn identity(nonce: [u8; 32], manifest: &NativeBootManifest) -> Result<NativeBootIdentityV1> {
    Ok(NativeBootIdentityV1 {
        root_nonce: nonce,
        manifest_operation: manifest.operation.key,
        manifest_object: manifest.operation.request,
        source_cut_digest: manifest.source_cut_digest()?,
    })
}

fn authenticate(
    authority: &NativeBootAuthority,
    snapshot: &quanta_index_catalog::NativeBootCatalogSnapshot,
    manifest: &NativeBootManifest,
    registry: &AdapterRegistry,
    maximum: u64,
    budget: &mut WorkBudget,
) -> Result<()> {
    let root = authority.lease.state_root_identity_v1();
    let object_root = root.join("native-objects");
    let io = Arc::new(NativeObjectReadIo::new(Arc::clone(&authority.physical)));
    let lock = io.open(&object_root.join(".object-namespace.lease"), budget)?;
    let reader = FileObjectReader::open_existing(&object_root, lock, io, budget)?;
    if snapshot.namespace != Some(reader.namespace_binding()?) {
        return Err(IndexError::DigestConflict.into());
    }
    let mut roots = [manifest.operation.request; 5];
    let mut count = 0usize;
    for root in manifest.protected_roots().into_iter().flatten() {
        *roots.get_mut(count).ok_or(IndexError::ResourceLimited)? = root;
        count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    // Every pending reference comes from one extant immutable edge. The explicit
    // whole-root entry ceiling also bounds this separately admitted frontier.
    let maximum = u32::try_from(maximum).map_err(|_error| IndexError::ResourceLimited)?;
    reader.verify_closure(
        roots.get(..count).ok_or(IndexError::ResourceLimited)?,
        maximum,
        budget,
    )?;
    let request = reader.get(manifest.operation.request, budget)?;
    let profile = reader.get(manifest.publication.profile, budget)?;
    verify_native_boot_inputs(
        &manifest.operation,
        &manifest.publication,
        &request,
        &profile,
        registry,
        budget,
    )?;
    drop(profile);
    drop(request);
    drop(reader);
    authority.physical.sweep_retired()?;
    authority.lease.require_state_root_v1(root)?;
    budget.checkpoint()?;
    Ok(())
}

// Close failures poison the Common owner even when they were observed in a
// destructor. No receipt or listener handoff may precede this witness.
fn require_retired(authority: &NativeBootAuthority) -> Result<()> {
    authority.io.drain_retirements()?;
    authority.physical.sweep_retired()?;
    if !authority.physical.is_quiescent()? {
        return Err(IndexError::BackendViolation(
            "native boot retained unresolved physical IO custody".into(),
        )
        .into());
    }
    Ok(())
}

/// None is offline completion; Some transfers the same root and Common owner.
pub(super) fn execute(
    command: &NativeStateCommandV1,
    config: &ValidatedConfig,
    registry: &AdapterRegistry,
    request: RequestBudgetV1,
) -> Result<Option<NativeBootAuthority>> {
    match command.mode {
        NativeCommandModeV1::Select
            if command.select_operation.is_none() || command.receipt_path.is_none() =>
        {
            bail!("native selection requires an operation and receipt path")
        }
        NativeCommandModeV1::Verify
            if command.identity.is_none()
                || command.config_digest.is_none()
                || command.install_catalog_digest.is_none() =>
        {
            bail!("native verification requires complete identity and install digests")
        }
        NativeCommandModeV1::Serve
            if command.identity.is_none() || command.config_digest.is_none() =>
        {
            bail!("selected Native service requires complete identity and config digest")
        }
        NativeCommandModeV1::Select | NativeCommandModeV1::Verify | NativeCommandModeV1::Serve => {}
    }
    if fs::symlink_metadata(&config.state_root)?
        .file_type()
        .is_symlink()
    {
        bail!("selected native state root must not be a symlink");
    }
    let ledger = NativeResourceLedger::with_maintenance(
        config.resources.execution,
        config.owned_work.is_some(),
        config.maintenance.is_some(),
    )?;
    let scheduler = ledger.scheduler(NativeResourceClass::Control);
    let control =
        ReservationGuard::try_acquire(Arc::clone(&scheduler), config.resources.execution.control)
            .ok_or_else(|| scheduler.denial())?;
    let mut budget = WorkBudget::with_request(config.limits, request);
    budget.bind_memory_scope(control.memory_scope());
    let lease = Arc::new(StateRootLease::acquire_existing(&config.state_root)?);
    let root = lease.state_root_identity_v1().to_path_buf();
    let authority = prepare_native_boot_authority(
        lease,
        &config.socket_path,
        config.resources,
        budget.request(),
    )?;
    verify_existing_native_format(&authority, &config.socket_path, &mut budget)?;
    let maximum = config.resources.physical.inventory.max_entries;
    if command.mode == NativeCommandModeV1::Select {
        let target = receipt::target(
            command
                .receipt_path
                .as_ref()
                .ok_or_else(|| anyhow!("missing native receipt"))?,
            &root,
        )?;
        let operation = command
            .select_operation
            .ok_or_else(|| anyhow!("missing native operation"))?;
        let candidate =
            verify_native_boot_candidate(&root, authority.io.as_ref(), operation, &mut budget)?;
        authenticate(
            &authority,
            candidate.snapshot(),
            candidate.manifest(),
            registry,
            maximum,
            &mut budget,
        )?;
        let expected = identity(candidate.snapshot().root_nonce, candidate.manifest())?;
        drop(candidate);
        {
            let _funding = authority.io.enter_maintenance_scope()?;
            let catalog = SqliteCatalog::open_with_io_admission(
                &root,
                std::time::Duration::ZERO,
                authority.io.clone(),
            )?;
            let selected = catalog.select_native_boot_manifest(operation, &mut budget)?;
            if identity(catalog.native_root_nonce(&mut budget)?, &selected)? != expected {
                return Err(IndexError::DigestConflict.into());
            }
            drop(catalog);
        }
        require_retired(&authority)?;
        let snapshot = verify_native_boot_catalog(&root, authority.io.as_ref(), &mut budget)?;
        let selected = snapshot
            .selected
            .as_ref()
            .ok_or(IndexError::SnapshotUnavailable)?;
        if identity(snapshot.root_nonce, selected)? != expected {
            return Err(IndexError::DigestConflict.into());
        }
        authenticate(
            &authority,
            &snapshot,
            selected,
            registry,
            maximum,
            &mut budget,
        )?;
        let io = NativeObjectReadIo::new(Arc::clone(&authority.physical));
        let digest = catalog_digest::catalog_content_digest(&root, &io, &mut budget)?;
        drop(snapshot);
        require_retired(&authority)?;
        authority.lease.require_state_root_v1(&root)?;
        budget.checkpoint()?;
        // Publication is the final fallible operation. Catalog selection may
        // already be committed on failure; postcommit uncertainty can also
        // leave a durable receipt. Inspect that outcome before retrying with
        // a fresh, absent target; installation must independently verify it.
        target.write(expected, digest, &mut budget)?;
        return Ok(None);
    }
    let snapshot = verify_native_boot_catalog(&root, authority.io.as_ref(), &mut budget)?;
    let selected = snapshot
        .selected
        .as_ref()
        .ok_or(IndexError::SnapshotUnavailable)?;
    if Some(identity(snapshot.root_nonce, selected)?) != command.identity {
        return Err(IndexError::DigestConflict.into());
    }
    authenticate(
        &authority,
        &snapshot,
        selected,
        registry,
        maximum,
        &mut budget,
    )?;
    if let Some(expected) = command.install_catalog_digest {
        let io = NativeObjectReadIo::new(Arc::clone(&authority.physical));
        if catalog_digest::catalog_content_digest(&root, &io, &mut budget)? != expected {
            return Err(IndexError::DigestConflict.into());
        }
    }
    drop(snapshot);
    require_retired(&authority)?;
    authority.lease.require_state_root_v1(&root)?;
    budget.checkpoint()?;
    if command.mode == NativeCommandModeV1::Serve {
        Ok(Some(authority))
    } else {
        Ok(None)
    }
}
