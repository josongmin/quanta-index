use std::{ffi::CString, sync::Arc, time::Duration};

use quanta_index_catalog::{CatalogIoAdmission, CatalogWriterCachePolicy, SqliteCatalog};
use quanta_index_core::{RequestBudgetV1, index::effort::ResourceNeed};
use rusqlite::{Connection, OpenFlags};

use super::*;
use crate::{
    native_resources::{NativeResourceLedger, NativeResourcePlan},
    publication_file_capacity::PublicationFileCapacity,
    publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn require(condition: bool, detail: impl Into<String>) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(detail.into()).into())
    }
}

fn require_eq<A: std::fmt::Debug + PartialEq<B>, B: std::fmt::Debug>(
    actual: &A,
    expected: B,
) -> TestResult {
    require(
        actual == &expected,
        format!("Catalog retirement gold differs: actual={actual:?}, expected={expected:?}"),
    )
}

#[test]
fn checked_retirement_waits_for_another_real_sqlite_handle_then_retries() -> TestResult {
    let directory = tempfile::tempdir()?;
    let lease = Arc::new(StateRootLease::acquire(&directory.path().join("state"))?);
    let root = lease.state_root_identity_v1().to_path_buf();
    let authority = seed_publication_capacity(
        &lease,
        &root,
        PublicationBootInventoryLimits {
            max_bytes: 32 * 1024 * 1024,
            max_files: 128,
            max_pending: 256,
            max_entries: 512,
            max_depth: 8,
            max_pending_frames: 32,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let common = PublicationFileCapacity::new_with_maintenance(
        lease.as_ref().clone(),
        authority.clone(),
        32,
        512,
        256,
        16 * 1024 * 1024,
        64,
    )?;
    let io = NativeScratchIo::register_with_maintenance(
        Arc::clone(&common),
        CString::new("quanta-publication-capacity")?,
        512,
    )?;
    let unit = ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024,
        memory_bytes: 1024 * 1024,
        provider_units: 1,
    };
    let ledger = NativeResourceLedger::new(NativeResourcePlan {
        capacity: ResourceNeed {
            cpu_units: 4,
            io_bytes: 4096,
            memory_bytes: 8 * 1024 * 1024,
            provider_units: 4,
        },
        control: unit,
        interactive: unit,
        core: unit,
        enrichment: unit,
    })?;
    let policy = CatalogWriterCachePolicy { kib: 64 };
    let cache_bytes = policy.reservation_bytes()?;
    let cache_memory = ledger.reserve_catalog_cache(cache_bytes)?;
    let admission: Arc<dyn CatalogIoAdmission> = io.clone();
    let catalog = {
        let _funding = io.enter_maintenance_scope()?;
        Arc::new(SqliteCatalog::open_with_io_admission_and_writer_cache(
            &root,
            Duration::from_secs(2),
            admission,
            policy,
            cache_memory,
        )?)
    };
    require_eq(&ledger.snapshot()?.retained_memory_bytes, cache_bytes)?;

    // This is a separate native VFS handle, not a Catalog Arc alias. Its FD
    // remains live after the initial opening scope exits.
    let other = {
        let _funding = io.enter_maintenance_scope()?;
        let _scope = CatalogIoAdmission::enter_scope(io.as_ref(), None);
        Connection::open_with_flags_and_vfs(
            catalog.path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
            io.vfs_name(),
        )?
    };
    let retirement = NativeCatalogRetirement::new(catalog, Arc::clone(&io), Arc::clone(&lease));
    let first = retirement.retire();
    require(
        matches!(first, Err(CoreError::NotReady(_))),
        format!("{first:?}"),
    )?;
    // The Catalog connection really closed, so only its funded cache is
    // refunded. The separate VFS handle still retains root custody.
    require_eq(&ledger.snapshot()?.retained_memory_bytes, 0)?;
    {
        let state = retirement.state.lock().map_err(|error| {
            std::io::Error::other(format!("retirement state poisoned: {error}"))
        })?;
        let (completed, catalog_present, io_present, root_present) = (
            state.completed,
            state.catalog.is_some(),
            state.io.is_some(),
            state.root.is_some(),
        );
        drop(state);
        require(
            !completed,
            "retirement completed with another live SQLite handle",
        )?;
        require(
            catalog_present && io_present && root_present,
            "retirement released custody while another SQLite handle remained",
        )?;
    }
    // Local io/common/lease clones also hold the root; only the final
    // reacquire after dropping all of them is an independent root oracle.
    {
        let _scope = CatalogIoAdmission::enter_close_scope(io.as_ref());
        other.close().map_err(|(_connection, error)| error)?;
    }
    retirement.retire()?;
    retirement.retire()?;
    {
        let state = retirement.state.lock().map_err(|error| {
            std::io::Error::other(format!("retirement state poisoned: {error}"))
        })?;
        let (completed, catalog_present, io_present, root_present) = (
            state.completed,
            state.catalog.is_some(),
            state.io.is_some(),
            state.root.is_some(),
        );
        drop(state);
        require(completed, "retirement did not complete after handle close")?;
        require(
            !catalog_present && !io_present && !root_present,
            "retirement retained custody after successful completion",
        )?;
    }
    require_eq(&ledger.snapshot()?.retained_memory_bytes, 0)?;
    drop(retirement);
    drop(io);
    drop(common);
    drop(lease);
    drop(authority);
    let _reacquired = StateRootLease::acquire_existing(&root)?;
    Ok(())
}
