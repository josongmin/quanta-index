use super::*;

fn regular_file_inventory(root: &Path) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let mut pending = vec![root.to_path_buf()];
    let mut bytes = 0_u64;
    let mut files = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                require(
                    metadata.is_file(),
                    "unexpected service Catalog inventory entry",
                )?;
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or("inventory byte overflow")?;
                files = files.checked_add(1).ok_or("inventory file overflow")?;
            }
        }
    }
    Ok((bytes, files))
}

#[test]
fn service_parent_funds_catalog_open_statement_and_physical_close() -> TestResult {
    let fixture = funded_fixture(64 * 1024 * 1024)?;
    let service_parent = fixture
        .common
        .reserve_publication_allowance(16 * 1024 * 1024, 8)?;
    let admission: Arc<dyn CatalogIoAdmission> = Arc::new(ServiceFundedCatalogIo::new(
        Arc::clone(&fixture.vfs),
        Arc::clone(&service_parent),
    )?);
    let catalog = SqliteCatalog::open_with_io_admission(
        &fixture.root,
        Duration::from_secs(1),
        Arc::clone(&admission),
    )?;
    let (_next, _exhausted) = catalog.sequence_allocator()?;
    let path = catalog.path().to_path_buf();
    let scope = admission.enter_scope(None);
    let nested_scope = admission.enter_scope(None);
    drop(nested_scope);
    let writer = Connection::open_with_flags_and_vfs(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE,
        fixture.vfs.name(),
    )?;
    writer.execute_batch(
        "CREATE TABLE service_funding_probe(v BLOB); \
         INSERT INTO service_funding_probe VALUES (zeroblob(4096));",
    )?;
    let rows: i64 = writer.query_row("SELECT count(*) FROM service_funding_probe", [], |row| {
        row.get(0)
    })?;
    require(
        rows == 1,
        "service-funded Catalog statement was not durable",
    )?;
    drop(writer);
    drop(scope);
    drop(catalog);
    fixture.vfs.drain_retirements()?;
    drop(admission);
    drop(service_parent);
    let (disk_bytes, disk_files) = regular_file_inventory(&fixture.root)?;
    let settled = fixture.authority.snapshot()?;
    require(
        settled.used_bytes == disk_bytes
            && settled.used_files == disk_files
            && settled.active_tickets == 0
            && settled.active_allowances == 0,
        "service-funded Catalog close diverged from real file inventory",
    )?;
    require(
        fixture.common.is_quiescent()?,
        "service-funded Catalog retained actual native handles",
    )?;
    Ok(())
}

#[test]
fn checked_service_retirement_refuses_live_reader_then_releases_after_close() -> TestResult {
    let fixture = funded_fixture(64 * 1024 * 1024)?;
    let service_parent = fixture
        .common
        .reserve_publication_allowance(16 * 1024 * 1024, 8)?;
    let owned = Arc::new(ServiceFundedCatalogIo::new(
        Arc::clone(&fixture.vfs),
        Arc::clone(&service_parent),
    )?);
    let admission: Arc<dyn CatalogIoAdmission> = owned.clone();
    let catalog =
        SqliteCatalog::open_with_io_admission(&fixture.root, Duration::from_secs(1), admission)?;
    let path = catalog.path().to_path_buf();
    drop(catalog);
    fixture.vfs.drain_retirements()?;
    let service = Arc::try_unwrap(owned)
        .map_err(|_owned| std::io::Error::other("Catalog still owns the service admission"))?;
    let scope = service.enter_scope(None);
    let reader = Connection::open_with_flags_and_vfs(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
        fixture.vfs.name(),
    )?;
    let before = fixture.authority.snapshot()?;
    let parent_before = service_parent.available_capacity()?;
    let error = match service.retire_checked() {
        Ok(()) => return Err("checked retirement accepted a live SQLite reader".into()),
        Err(refused) => refused,
    };
    require(
        matches!(error, CoreError::NotReady(_))
            && fixture.authority.snapshot()? == before
            && service_parent.available_capacity()? == parent_before,
        "live-reader refusal changed the Catalog authority or held parent",
    )?;
    drop(reader);
    drop(scope);
    service.retire_checked()?;
    fixture.vfs.retire_checked()?;
    fixture.vfs.retire_checked()?;
    drop(service);
    drop(service_parent);
    let (disk_bytes, disk_files) = regular_file_inventory(&fixture.root)?;
    let settled = fixture.authority.snapshot()?;
    require(
        settled.used_bytes == disk_bytes
            && settled.used_files == disk_files
            && settled.active_tickets == 0
            && settled.active_allowances == 0
            && fixture.common.is_quiescent()?,
        "checked Catalog retirement did not reach disk/ledger parity",
    )?;
    Ok(())
}

#[test]
fn service_parent_constructor_refuses_foreign_authority_and_unfunded_mode() -> TestResult {
    let funded = funded_fixture(64 * 1024 * 1024)?;
    let foreign = funded_fixture(64 * 1024 * 1024)?;
    let foreign_parent = foreign.common.reserve_publication_allowance(4096, 2)?;
    let before = funded.authority.snapshot()?;
    require(
        matches!(
            ServiceFundedCatalogIo::new(Arc::clone(&funded.vfs), foreign_parent),
            Err(CoreError::InvalidContract(_))
        ),
        "service Catalog accepted another authority's parent",
    )?;
    require(
        funded.authority.snapshot()? == before,
        "foreign parent changed service Catalog capacity",
    )?;

    let unqualified = fixture(64 * 1024 * 1024)?;
    let own_parent = unqualified.common.reserve_publication_allowance(4096, 2)?;
    require(
        matches!(
            ServiceFundedCatalogIo::new(Arc::clone(&unqualified.vfs), own_parent),
            Err(CoreError::InvalidContract(_))
        ),
        "service Catalog accepted an unqualified VFS",
    )?;
    Ok(())
}

#[test]
fn existing_event_parent_does_not_fall_back_to_service_capacity() -> TestResult {
    let fixture = funded_fixture(64 * 1024 * 1024)?;
    let service_parent = fixture
        .common
        .reserve_publication_allowance(16 * 1024 * 1024, 8)?;
    let admission: Arc<dyn CatalogIoAdmission> = Arc::new(ServiceFundedCatalogIo::new(
        Arc::clone(&fixture.vfs),
        Arc::clone(&service_parent),
    )?);
    let catalog = SqliteCatalog::open_with_io_admission(
        &fixture.root,
        Duration::from_secs(1),
        Arc::clone(&admission),
    )?;
    let path = catalog.path().to_path_buf();
    let event_parent = fixture.common.reserve_publication_allowance(1, 4)?;
    let service_before = service_parent.available_capacity()?;
    let event_funding = fixture.vfs.enter_funded_scope(event_parent)?;
    let scope = admission.enter_scope(None);
    let nested_scope = admission.enter_scope(None);
    drop(nested_scope);
    let writer = Connection::open_with_flags_and_vfs(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE,
        fixture.vfs.name(),
    )?;
    let denied = match writer.execute_batch(
        "CREATE TABLE event_must_pay(v BLOB); INSERT INTO event_must_pay VALUES (zeroblob(4096));",
    ) {
        Ok(()) => return Err("one-byte event parent admitted a Catalog page write".into()),
        Err(error) => error,
    };
    require(
        scope.is_capacity_refusal(&denied),
        "exhausted event parent did not return the scoped capacity refusal",
    )?;
    require(
        service_parent.available_capacity()? == service_before,
        "exhausted event parent silently spent service capacity",
    )?;
    drop(writer);
    drop(scope);
    drop(event_funding);
    drop(catalog);
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn foreign_vfs_scope_never_borrows_service_parent() -> TestResult {
    let fixture = funded_fixture(64 * 1024 * 1024)?;
    let foreign = funded_fixture(64 * 1024 * 1024)?;
    let service_parent = fixture.common.reserve_publication_allowance(4096, 2)?;
    let foreign_parent = foreign.common.reserve_publication_allowance(4096, 2)?;
    let admission =
        ServiceFundedCatalogIo::new(Arc::clone(&fixture.vfs), Arc::clone(&service_parent))?;
    let service_before = service_parent.available_capacity()?;
    let foreign_funding = foreign.vfs.enter_funded_scope(foreign_parent)?;
    let scope = admission.enter_scope(None);
    let path = fixture.root.join("foreign-scope-must-refuse.sqlite");
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    require(
        Connection::open_with_flags_and_vfs(&path, flags, fixture.vfs.name()).is_err(),
        "foreign VFS scope borrowed the service Catalog parent",
    )?;
    require(!path.exists(), "foreign VFS scope created a Catalog file")?;
    require(
        service_parent.available_capacity()? == service_before,
        "foreign VFS scope consumed service Catalog capacity",
    )?;
    drop(scope);
    drop(foreign_funding);
    Ok(())
}
