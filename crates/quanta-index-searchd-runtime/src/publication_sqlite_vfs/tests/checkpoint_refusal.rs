use super::*;

#[test]
fn native_capacity_refusal_during_catalog_checkpoint_is_not_deferred() -> TestResult {
    let fixture = funded_fixture(64 * 1024 * 1024)?;
    let initial_parent = fixture
        .common
        .reserve_publication_allowance(16 * 1024 * 1024, 8)?;
    let initial_funding = fixture
        .vfs
        .enter_funded_scope(Arc::clone(&initial_parent))?;
    let admission: Arc<dyn CatalogIoAdmission> = fixture.vfs.clone();
    let mut catalog =
        SqliteCatalog::open_with_io_admission(&fixture.root, Duration::from_secs(2), admission)?;
    let path = catalog.path().to_path_buf();
    let writer = Connection::open_with_flags_and_vfs(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE,
        fixture.vfs.name(),
    )?;
    writer.execute_batch(
        "PRAGMA wal_autocheckpoint=0;
         CREATE TABLE checkpoint_capacity_gold(id INTEGER PRIMARY KEY, payload BLOB);
         INSERT INTO checkpoint_capacity_gold VALUES(7, zeroblob(65536));",
    )?;
    let accepted: (i64, i64) = writer.query_row(
        "SELECT id, length(payload) FROM checkpoint_capacity_gold",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    require(accepted == (7, 65_536), "fixture row did not commit")?;
    let wal = path.with_file_name(format!("{CATALOG_FILE_NAME}-wal"));
    require(
        fs::metadata(&wal)?.len() > 65_536,
        "fixture did not retain committed WAL pages for checkpoint",
    )?;
    // Reduce only the original parent's unassigned window. Its committed
    // children and exact VFS pending-parent identity remain in custody.
    initial_parent.resize_available(1, 4)?;
    require(
        initial_parent.available_capacity()? == (1, 4),
        "fixture did not narrow the original parent window",
    )?;
    let mut budget = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 536_870_912,
            mutations: 1_000_000,
        },
        RequestBudgetV1::unbounded(),
    );
    let checkpoint = catalog.checkpoint_wal_for_reclaim(1, 64 * 1024 * 1024, &mut budget);
    require(
        matches!(&checkpoint, Err(IndexError::ResourceLimited)),
        &format!("native checkpoint capacity refusal was reclassified: {checkpoint:?}"),
    )?;
    require(
        budget.usage().writes > 0,
        "checkpoint was deferred before its work charge",
    )?;
    let after: (i64, i64) = writer.query_row(
        "SELECT id, length(payload) FROM checkpoint_capacity_gold",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    require(
        after == accepted,
        "failed checkpoint changed the committed Catalog row",
    )?;

    initial_parent
        .resize_available(16 * 1024 * 1024, 8)
        .map_err(|error| {
            std::io::Error::other(format!("restore original checkpoint allowance: {error}"))
        })?;
    // This is an explicit later recovery with original funding restored, not
    // a same-round GC retry or an OS ENOSPC assertion.
    let mut recovery = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 536_870_912,
            mutations: 1_000_000,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    let recovered = catalog
        .checkpoint_wal_for_reclaim(1, 64 * 1024 * 1024, &mut recovery)
        .map_err(|error| {
            std::io::Error::other(format!(
                "recover checkpoint after capacity refusal: {error:?}"
            ))
        })?;
    require(
        recovered == quanta_index_catalog::WalReclaimOutcome::Completed,
        "checkpoint did not recover under its restored original funding",
    )?;
    drop(writer);
    catalog
        .close_checked()
        .map_err(|error| std::io::Error::other(format!("close recovered Catalog: {error}")))?;
    drop(catalog);
    fixture.vfs.drain_retirements()?;
    drop(initial_funding);
    drop(initial_parent);
    let settled = fixture.authority.snapshot()?;
    require(
        settled.active_tickets == 0
            && settled.active_allowances == 0
            && settled.uncertain_pending == 0
            && !settled.poisoned,
        "native checkpoint refusal/recovery retained provisional custody",
    )
}
