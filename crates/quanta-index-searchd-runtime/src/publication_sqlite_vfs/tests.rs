use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::time::Duration;

use quanta_index_catalog::{CATALOG_FILE_NAME, SqliteCatalog};
use quanta_index_core::RequestBudgetV1;
use quanta_index_searchd::app::runtime::StateRootLease;
use rusqlite::{Connection, OpenFlags};

use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

mod boot_candidate;
mod native_retirement;
mod read_only;
mod root_identity;
mod service;
mod sync_diagnostic;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn require(condition: bool, message: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

struct Fixture {
    // Rust drops fields in declaration order. Keep the namespace available
    // through VFS retirement and its final native FD callbacks.
    vfs: Arc<SqliteCapacityVfs>,
    common: Arc<PublicationFileCapacity>,
    authority: quanta_index_core::publication_capacity::PublicationCapacity,
    root: PathBuf,
    directory: tempfile::TempDir,
}

fn fixture(max_bytes: u64) -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with_boot_input(|_root| Ok(max_bytes))
}

fn fixture_with_boot_input(
    prepare: impl FnOnce(&Path) -> Result<u64, Box<dyn std::error::Error>>,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with_boot_input_and_mode(prepare, FundingPolicy::Unqualified)
}

fn funded_fixture(max_bytes: u64) -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with_boot_input_and_mode(|_root| Ok(max_bytes), FundingPolicy::HeldParent)
}

fn fixture_with_boot_input_and_mode(
    prepare: impl FnOnce(&Path) -> Result<u64, Box<dyn std::error::Error>>,
    funding_policy: FundingPolicy,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with_named_root(prepare, funding_policy, "state")
}

fn fixture_with_named_root(
    prepare: impl FnOnce(&Path) -> Result<u64, Box<dyn std::error::Error>>,
    funding_policy: FundingPolicy,
    root_name: &str,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join(root_name);
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let max_bytes = prepare(&root)?;
    let lease = StateRootLease::acquire(&root)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let authority = seed_publication_capacity(
        &lease,
        &root,
        PublicationBootInventoryLimits {
            max_bytes,
            max_files: 64,
            max_pending: 128,
            max_entries: 256,
            max_depth: 8,
            max_pending_frames: 16,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let common = PublicationFileCapacity::new(lease, authority.clone(), 32, 256, 256)?;
    let vfs_name = CString::new("quanta-publication-capacity")?;
    let vfs = Arc::new(match funding_policy {
        FundingPolicy::Unqualified => {
            SqliteCapacityVfs::register(Arc::clone(&common), vfs_name, 512)?
        }
        FundingPolicy::HeldParent => {
            SqliteCapacityVfs::register_funded(Arc::clone(&common), vfs_name, 512)?
        }
        FundingPolicy::LocalOrHeldParent => {
            SqliteCapacityVfs::register_with_optional_funding(Arc::clone(&common), vfs_name, 512)?
        }
    });
    Ok(Fixture {
        vfs,
        common,
        authority,
        root,
        directory,
    })
}

#[test]
fn native_event_abi_matches_the_compiled_shim() -> TestResult {
    quanta_capacity_vfs::verify_event_abi()
        .map_err(|error| std::io::Error::other(format!("VFS event ABI: {error:?}")))?;
    Ok(())
}

#[test]
fn catalog_initialization_and_close_use_real_admission_scope() -> TestResult {
    let fixture = fixture(16 * 1024 * 1024)?;
    let admission: Arc<dyn CatalogIoAdmission> = fixture.vfs.clone();
    let catalog =
        SqliteCatalog::open_with_io_admission(&fixture.root, Duration::from_secs(1), admission)?;
    require(
        fixture.authority.snapshot()?.used_bytes > 0,
        "Catalog initialized without accounted bytes",
    )?;
    require(
        !fixture.common.is_quiescent()?,
        "live Catalog lost FD ownership",
    )?;
    drop(catalog);
    fixture.vfs.drain_retirements()?;
    require(
        fixture.common.is_quiescent()?,
        "closed Catalog retained unresolved native handles",
    )?;
    let mut pending = vec![fixture.root.clone()];
    let mut disk_bytes = 0_u64;
    let mut disk_files = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                require(metadata.is_file(), "unexpected Catalog fixture entry")?;
                disk_bytes = disk_bytes
                    .checked_add(metadata.len())
                    .ok_or("Catalog disk byte count overflow")?;
                disk_files = disk_files
                    .checked_add(1)
                    .ok_or("Catalog disk file count overflow")?;
            }
        }
    }
    let settled = fixture.authority.snapshot()?;
    require(
        settled.used_bytes == disk_bytes
            && settled.used_files == disk_files
            && settled.active_tickets == 0
            && settled.active_allowances == 0,
        "closed Catalog retained a deleted WAL/SHM charge or provisional owner",
    )?;

    let Fixture {
        directory,
        root,
        authority,
        common,
        vfs,
    } = fixture;
    drop(vfs);
    drop(common);
    drop(authority);
    // The process-global VFS router must relinquish the root after real
    // native FD retirement; logical connection drop alone is insufficient.
    let reopened = StateRootLease::acquire(&root)?;
    drop(reopened);
    drop(directory);
    Ok(())
}

#[test]
fn wal_truncate_checkpoint_retires_original_funding_before_next_writer() -> TestResult {
    let fixture = funded_fixture(64 * 1024 * 1024)?;
    let first = fixture
        .common
        .reserve_publication_allowance(16 * 1024 * 1024, 8)?;
    let next = fixture
        .common
        .reserve_publication_allowance(16 * 1024 * 1024, 8)?;
    let funding = fixture.vfs.enter_funded_scope(Arc::clone(&first))?;
    let admission: Arc<dyn CatalogIoAdmission> = fixture.vfs.clone();
    let catalog =
        SqliteCatalog::open_with_io_admission(&fixture.root, Duration::from_secs(1), admission)?;
    let writer = Connection::open_with_flags_and_vfs(
        catalog.path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE,
        fixture.vfs.name(),
    )?;
    writer.execute_batch("PRAGMA wal_autocheckpoint=0; CREATE TABLE checkpoint_gold(v INTEGER); INSERT INTO checkpoint_gold VALUES(7);")?;
    let result: (i64, i64, i64) =
        writer.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    require(
        result == (0, 0, 0),
        "checkpoint did not truncate its real WAL",
    )?;
    let wal = catalog.path().with_file_name("catalog-v1.sqlite-wal");
    require(
        fs::metadata(wal)?.len() == 0,
        "checkpoint retained WAL bytes",
    )?;
    drop(funding);
    let funding = fixture.vfs.enter_funded_scope(Arc::clone(&next))?;
    let inserted = writer.execute("INSERT INTO checkpoint_gold VALUES(11)", [])?;
    require(
        inserted == 1,
        "new funding could not append after WAL truncation",
    )?;
    let gold: i64 = writer.query_row("SELECT sum(v) FROM checkpoint_gold", [], |row| row.get(0))?;
    require(
        gold == 18,
        "checkpoint and new funding changed committed values",
    )?;
    drop(writer);
    drop(catalog);
    fixture.vfs.drain_retirements()?;
    drop(funding);
    drop(first);
    drop(next);
    let settled = fixture.authority.snapshot()?;
    require(
        settled.active_tickets == 0
            && settled.active_allowances == 0
            && settled.uncertain_pending == 0
            && !settled.poisoned,
        "WAL checkpoint retained provisional custody",
    )
}

#[test]
fn funded_catalog_wal_and_shm_subdivide_only_the_held_parent() -> TestResult {
    let fixture = funded_fixture(64 * 1024 * 1024)?;
    let parent = fixture
        .common
        .reserve_publication_allowance(16 * 1024 * 1024, 8)?;
    let charged = fixture.authority.snapshot()?;
    let funding = fixture.vfs.enter_funded_scope(Arc::clone(&parent))?;
    let admission: Arc<dyn CatalogIoAdmission> = fixture.vfs.clone();
    let catalog =
        SqliteCatalog::open_with_io_admission(&fixture.root, Duration::from_secs(1), admission)?;
    let path = catalog.path().to_path_buf();
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE;
    let writer = Connection::open_with_flags_and_vfs(&path, flags, fixture.vfs.name())?;
    writer.pragma_update(None, "wal_autocheckpoint", 0_i64)?;
    writer.execute_batch(
        "CREATE TABLE funded_probe(v BLOB); INSERT INTO funded_probe VALUES (zeroblob(4096));",
    )?;
    let reader = Connection::open_with_flags_and_vfs(&path, flags, fixture.vfs.name())?;
    let rows: i64 = reader.query_row("SELECT count(*) FROM funded_probe", [], |row| row.get(0))?;
    require(rows == 1, "funded Catalog row was not readable")?;
    let wal = path.with_file_name(format!("{CATALOG_FILE_NAME}-wal"));
    let shm = shm_path(&path);
    require(
        fs::metadata(&wal)?.len() > 0,
        "funded Catalog did not write WAL",
    )?;
    require(
        fs::metadata(&shm)?.len() >= 3,
        "funded Catalog did not attach SHM",
    )?;
    let during = fixture.authority.snapshot()?;
    require(
        during.used_bytes == charged.used_bytes && during.used_files == charged.used_files,
        "native Catalog file growth charged outside its held parent",
    )?;
    drop(reader);
    drop(writer);
    drop(catalog);
    fixture.vfs.drain_retirements()?;
    drop(funding);
    drop(parent);
    let settled = fixture.authority.snapshot()?;
    require(
        settled.active_tickets == 0 && settled.active_allowances == 0,
        "funded Catalog close retained provisional capacity",
    )?;
    Ok(())
}

#[test]
fn funded_sqlite_requires_same_authority_and_refuses_exhausted_parent_without_topup() -> TestResult
{
    let fixture = funded_fixture(64 * 1024 * 1024)?;
    let other = funded_fixture(64 * 1024 * 1024)?;
    let small = fixture.common.reserve_publication_allowance(1, 4)?;
    let large = fixture
        .common
        .reserve_publication_allowance(1024 * 1024, 4)?;
    let charged = fixture.authority.snapshot()?;
    require(
        other.vfs.enter_funded_scope(Arc::clone(&small)).is_err(),
        "foreign Catalog VFS accepted another root's held parent",
    )?;
    let raw_scope = fixture.vfs.scope();
    let path = fixture.root.join("funded-denial.sqlite3");
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    let missing = match Connection::open_with_flags_and_vfs(&path, flags, fixture.vfs.name()) {
        Ok(connection) => {
            drop(connection);
            return Err("funded VFS opened a new file without a parent".into());
        }
        Err(error) => error,
    };
    require(
        fixture.vfs.enter_scope(None).is_capacity_refusal(&missing),
        "missing held parent was not a typed capacity refusal",
    )?;
    drop(raw_scope);
    require(!path.exists(), "missing parent created a SQLite file")?;

    let outer = fixture.vfs.enter_funded_scope(Arc::clone(&small))?;
    let inner = fixture.vfs.enter_funded_scope(Arc::clone(&large))?;
    drop(inner);
    let connection = Connection::open_with_flags_and_vfs(&path, flags, fixture.vfs.name())?;
    let denied = connection
        .execute_batch("CREATE TABLE t(v BLOB); INSERT INTO t VALUES (zeroblob(4096));")
        .expect_err("nested scope restoration bypassed the one-byte parent");
    require(
        fixture.vfs.enter_scope(None).is_capacity_refusal(&denied),
        "exhausted parent was not a typed SQLite capacity refusal",
    )?;
    let after = fixture.authority.snapshot()?;
    require(
        after.used_bytes == charged.used_bytes && after.used_files == charged.used_files,
        "exhausted parent caused a root-global top-up",
    )?;
    drop(connection);
    fixture.vfs.drain_retirements()?;
    drop(outer);
    drop(small);
    drop(large);
    Ok(())
}

#[test]
fn real_vfs_capacity_refusal_keeps_sqlite_code_and_callback_detail() -> TestResult {
    type CapacityCaseResult = Result<(bool, Vec<Vec<u8>>), Box<dyn std::error::Error>>;

    fn run_case(
        vfs: &SqliteCapacityVfs,
        parent: &Arc<PublicationAllowance>,
        path: &Path,
    ) -> CapacityCaseResult {
        let (capacity, lines) = refusal_diagnostic::capture_test_lines(
            || -> Result<bool, Box<dyn std::error::Error>> {
                let _funding = vfs.enter_funded_scope(Arc::clone(parent))?;
                let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
                let connection = Connection::open_with_flags_and_vfs(path, flags, vfs.name())?;
                let denied = match connection
                    .execute_batch("CREATE TABLE t(v BLOB); INSERT INTO t VALUES (zeroblob(4096));")
                {
                    Ok(()) => {
                        return Err(std::io::Error::other(
                            "one-byte parent accepted SQLite growth",
                        )
                        .into());
                    }
                    Err(error) => error,
                };
                let capacity = vfs.enter_scope(None).is_capacity_refusal(&denied);
                drop(connection);
                Ok(capacity)
            },
        );
        Ok((capacity?, lines))
    }

    let fixture = funded_fixture(64 * 1024 * 1024)?;
    let observed = SqliteCapacityVfs::register_with_policy_and_diagnostics(
        Arc::clone(&fixture.common),
        CString::new("quanta-publication-capacity")?,
        512,
        FundingPolicy::HeldParent,
        true,
    )?;
    let parent = fixture.common.reserve_publication_allowance(1, 4)?;
    let (capacity, lines) = run_case(
        &observed,
        &parent,
        &fixture.root.join("observed-capacity.sqlite3"),
    )?;
    require(
        capacity,
        "observed callback changed SQLite capacity classification",
    )?;
    let rows: Vec<serde_json::Value> = lines
        .iter()
        .map(|line| serde_json::from_slice(line))
        .collect::<Result<_, _>>()?;
    // The first empty DB reserves the parent's only byte. The next native
    // journal creation is refused before open, before any xWrite can run.
    let typed = rows.iter().any(|row| {
        row["origin"] == "core_error_observed"
            && row["scope_id"] == observed.scope.get()
            && row["handle_id"].as_u64().is_some_and(|handle| handle != 0)
            && row["callback_kind"] == "BeforeOpen"
            && row["core_variant"] == "Typed"
            && row["mapped_refusal"] == "Capacity"
            && row["callback_returned"] == "Capacity"
            && row["type_code"] == SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded.to_string()
            && row["detail"]
                == "publication physical capacity: output exceeds held publication allowance"
    });
    require(
        typed,
        &format!("real VFS capacity lacked exact callback CoreError observation: {rows:?}"),
    )?;
    drop(parent);
    let off = SqliteCapacityVfs::register_with_policy_and_diagnostics(
        Arc::clone(&fixture.common),
        CString::new("quanta-publication-capacity")?,
        512,
        FundingPolicy::HeldParent,
        false,
    )?;
    let off_parent = fixture.common.reserve_publication_allowance(1, 4)?;
    let (off_capacity, off_lines) = run_case(
        &off,
        &off_parent,
        &fixture.root.join("unobserved-capacity.sqlite3"),
    )?;
    require(
        off_capacity,
        "Off callback changed SQLite capacity classification",
    )?;
    require(off_lines.is_empty(), "Off callback emitted a diagnostic")?;
    drop(off_parent);
    Ok(())
}

#[test]
fn pending_sqlite_writer_rejects_missing_or_different_parent_before_growth() -> TestResult {
    let fixture = funded_fixture(64 * 1024 * 1024)?;
    let parent_a = fixture
        .common
        .reserve_publication_allowance(8 * 1024 * 1024, 8)?;
    let parent_b = fixture
        .common
        .reserve_publication_allowance(1024 * 1024, 4)?;
    let path = fixture.root.join("pending-parent.sqlite3");
    let funding_a = fixture.vfs.enter_funded_scope(Arc::clone(&parent_a))?;
    let connection = Connection::open_with_flags_and_vfs(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        fixture.vfs.name(),
    )?;
    let adapter = {
        let scopes = fixture
            .vfs
            .router
            .scopes
            .lock()
            .map_err(|error| std::io::Error::other(format!("VFS scope lock: {error}")))?;
        Arc::clone(
            &scopes
                .get(&fixture.vfs.scope)
                .ok_or("missing VFS scope")?
                .adapter,
        )
    };
    let handle_id = {
        let state = adapter
            .state
            .lock()
            .map_err(|error| std::io::Error::other(format!("VFS handle lock: {error}")))?;
        state
            .handles
            .iter()
            .find(|(_id, handle)| handle.path == path && handle.mutation.is_some())
            .map(|(id, _handle)| *id)
            .ok_or("new SQLite file has no pending writer")?
    };
    let before_write = Event {
        kind: EventKind::BeforeWrite,
        scope_id: fixture.vfs.scope,
        handle_id,
        path: None,
        open_flags: 0,
        sqlite_result: rusqlite::ffi::SQLITE_OK,
        operation: 0,
        offset: 0,
        amount: 2,
        target_size: 2,
        observed_size: None,
        witness: None,
        fd_retired: false,
        first_shm_fd: false,
        native_sync: quanta_capacity_vfs::NativeSyncObservation::default(),
    };
    drop(funding_a);
    let charged = fixture.authority.snapshot()?;
    let raw_scope = fixture.vfs.scope();
    require(
        matches!(
            adapter.before_mutation(&before_write),
            Err(Refusal::Capacity)
        ),
        "pending writer accepted growth without funding",
    )?;
    drop(raw_scope);
    let funding_b = fixture.vfs.enter_funded_scope(Arc::clone(&parent_b))?;
    require(
        matches!(
            adapter.before_mutation(&before_write),
            Err(Refusal::Capacity)
        ),
        "pending writer used a different publication parent",
    )?;
    drop(funding_b);
    require(
        fixture.authority.snapshot()? == charged,
        "refused pending writer changed the capacity ledger",
    )?;
    let funding_a = fixture.vfs.enter_funded_scope(Arc::clone(&parent_a))?;
    connection.execute_batch("CREATE TABLE t(v BLOB); INSERT INTO t VALUES (zeroblob(2));")?;
    drop(connection);
    fixture.vfs.drain_retirements()?;
    drop(funding_a);
    drop(parent_a);
    drop(parent_b);
    Ok(())
}

#[test]
fn held_wal_reader_refuses_append_before_exceeding_shared_capacity() -> TestResult {
    const MAX_BYTES: u64 = 512 * 1024;
    let fixture = fixture(MAX_BYTES)?;
    let scope = fixture.vfs.scope();
    let path = fixture.root.join("bounded.sqlite3");
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    let writer = Connection::open_with_flags_and_vfs(&path, flags, fixture.vfs.name())?;
    writer.pragma_update(None, "journal_mode", "WAL")?;
    writer.pragma_update(None, "wal_autocheckpoint", 0_i64)?;
    writer.execute_batch("CREATE TABLE t(v BLOB);")?;
    let reader = Connection::open_with_flags_and_vfs(&path, flags, fixture.vfs.name())?;
    reader.execute_batch("BEGIN;")?;
    let initial: i64 = reader.query_row("SELECT count(*) FROM t", [], |row| row.get(0))?;
    require(initial == 0, "reader initial snapshot was not empty")?;

    let mut accepted = 0_usize;
    let mut refusal = None;
    for _ in 0..128 {
        match writer.execute("INSERT INTO t VALUES (zeroblob(8192))", []) {
            Ok(_) => accepted = accepted.checked_add(1).ok_or("insert counter overflow")?,
            Err(error) => {
                refusal = Some(error);
                break;
            }
        }
    }
    require(accepted > 0, "no insertion reached native WAL")?;
    let error = refusal.ok_or("bounded WAL workload did not reach capacity")?;
    let capacity_scope = fixture.vfs.enter_scope(None);
    require(
        capacity_scope.is_capacity_refusal(&error),
        &format!("unexpected refusal: {error:?}"),
    )?;
    let snapshot = fixture.authority.snapshot()?;
    require(
        snapshot.used_bytes <= MAX_BYTES,
        "shared capacity exceeded its configured ceiling",
    )?;
    let mut physical_lengths = 0_u64;
    for entry in fs::read_dir(&fixture.root)? {
        let metadata = fs::symlink_metadata(entry?.path())?;
        if metadata.is_file() {
            physical_lengths = physical_lengths
                .checked_add(metadata.len())
                .ok_or("physical length sum overflow")?;
        }
    }
    require(
        physical_lengths <= snapshot.used_bytes,
        "native file lengths exceed charged capacity",
    )?;
    drop(capacity_scope);
    drop(reader);
    drop(writer);
    fixture.vfs.drain_retirements()?;
    drop(scope);
    Ok(())
}

#[test]
fn poisoned_adapter_refuses_new_sync_and_open_admission() -> TestResult {
    let fixture = fixture(16 * 1024 * 1024)?;
    let adapter = Adapter {
        common: Arc::clone(&fixture.common),
        scope: fixture.vfs.scope,
        funding_policy: FundingPolicy::Unqualified,
        temp_nonce: [0; 16],
        refusal_diagnostics: false,
        state: Mutex::new(AdapterState {
            handles: BTreeMap::new(),
            deletes: BTreeMap::new(),
            poisoned: true,
        }),
    };
    for kind in [EventKind::BeforeSync, EventKind::BeforeOpen] {
        let result = adapter.on_event(Event {
            kind,
            scope_id: fixture.vfs.scope,
            handle_id: 1,
            path: None,
            open_flags: 0,
            sqlite_result: rusqlite::ffi::SQLITE_OK,
            operation: 0,
            offset: 0,
            amount: 0,
            target_size: 0,
            observed_size: None,
            witness: None,
            fd_retired: false,
            first_shm_fd: false,
            native_sync: quanta_capacity_vfs::NativeSyncObservation::default(),
        });
        require(
            matches!(result, Err(Refusal::Io)),
            "poisoned adapter accepted new native I/O",
        )?;
    }
    Ok(())
}

fn shm_probe_fixture(
    headroom: u64,
    existing_shm_bytes: Option<usize>,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with_boot_input(|root| {
        let path = root.join("probe.sqlite3");
        let connection = Connection::open(&path)?;
        connection.execute_batch("CREATE TABLE t(v INTEGER);")?;
        connection.close().map_err(|(_connection, error)| error)?;
        let mut seeded_bytes = fs::metadata(&path)?.len();
        if let Some(bytes) = existing_shm_bytes {
            fs::write(shm_path(&path), vec![0_u8; bytes])?;
            seeded_bytes = seeded_bytes
                .checked_add(u64::try_from(bytes)?)
                .ok_or("seed length overflow")?;
        }
        Ok(seeded_bytes
            .checked_add(headroom)
            .ok_or("seed cap overflow")?)
    })
}

fn first_shm_probe(
    fixture: &Fixture,
) -> Result<quanta_capacity_vfs::ShmReadProbe, quanta_capacity_vfs::ShmReadProbeError> {
    let path = fixture.root.join("probe.sqlite3");
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_error| {
        quanta_capacity_vfs::ShmReadProbeError::Sqlite(rusqlite::ffi::SQLITE_CANTOPEN)
    })?;
    fixture
        .vfs
        .vfs
        .test_first_shm_read_map(fixture.vfs.scope, &path)
}

#[test]
fn first_nonextending_shm_map_reserves_and_settles_three_bytes() -> TestResult {
    let fixture = shm_probe_fixture(128 * 1024, None)?;
    let before = fixture.authority.snapshot()?.used_bytes;
    let probe = first_shm_probe(&fixture)
        .map_err(|error| std::io::Error::other(format!("first SHM probe: {error:?}")))?;
    require(
        !probe.mapping_present,
        "nonextending first map unexpectedly grew a region",
    )?;
    fixture.vfs.drain_retirements()?;
    let shm_bytes = fs::metadata(shm_path(&fixture.root.join("probe.sqlite3")))?.len();
    require(
        shm_bytes == 3,
        "first SHM bootstrap did not use the SQLite three-byte width",
    )?;
    require(
        fixture.authority.snapshot()?.used_bytes
            == before.checked_add(3).ok_or("width overflow")?,
        "bootstrap width was not charged exactly once",
    )?;
    require(
        fixture.common.is_quiescent()?,
        "probe retained closed native handles",
    )
}

#[test]
fn first_nonextending_shm_map_reconciles_existing_bootstrap_width() -> TestResult {
    let fixture = shm_probe_fixture(128 * 1024, Some(32768))?;
    let before = fixture.authority.snapshot()?.used_bytes;
    let _probe = first_shm_probe(&fixture)
        .map_err(|error| std::io::Error::other(format!("existing SHM probe: {error:?}")))?;
    fixture.vfs.drain_retirements()?;
    require(
        fs::metadata(shm_path(&fixture.root.join("probe.sqlite3")))?.len() == 3,
        "unused existing SHM was not reset to bootstrap width",
    )?;
    let expected = before
        .checked_sub(32768)
        .and_then(|bytes| bytes.checked_add(3))
        .ok_or("width overflow")?;
    require(
        fixture.authority.snapshot()?.used_bytes == expected,
        "SHM shrink was not reconciled",
    )?;
    require(
        fixture.common.is_quiescent()?,
        "existing SHM probe retained closed handles",
    )
}

#[test]
fn first_shm_capacity_refusal_preserves_bytes_and_releases_unopened_handle() -> TestResult {
    let fixture = shm_probe_fixture(1, None)?;
    let before = fixture.authority.snapshot()?;
    let result = first_shm_probe(&fixture);
    require(
        matches!(
            result,
            Err(quanta_capacity_vfs::ShmReadProbeError::Sqlite(
                quanta_capacity_vfs::CAPACITY_EXTENDED_CODE
            ))
        ),
        &format!("unexpected first SHM refusal: {result:?}"),
    )?;
    fixture.vfs.drain_retirements()?;
    require(
        !shm_path(&fixture.root.join("probe.sqlite3")).exists(),
        "refused first attach created a SHM file",
    )?;
    let after = fixture.authority.snapshot()?;
    require(
        after.used_bytes == before.used_bytes && after.used_files == before.used_files,
        "refused attach leaked provisional capacity",
    )?;
    require(
        fixture.common.is_quiescent()?,
        "refused BEFORE callback retained an unopened SHM handle",
    )?;
    let Fixture {
        directory,
        root,
        authority,
        common,
        vfs,
    } = fixture;
    drop(vfs);
    drop(common);
    drop(authority);
    let lease = StateRootLease::acquire(&root)?;
    drop(lease);
    drop(directory);
    Ok(())
}

#[test]
fn retiring_sqlite_scope_preserves_other_common_authority_owners() -> TestResult {
    let Fixture {
        directory,
        root,
        authority,
        common,
        vfs,
    } = fixture(16 * 1024 * 1024)?;
    drop(vfs);
    let mutation = common.begin_create(&root.join("after-catalog-close"))?;
    drop(mutation);
    require(
        common.is_quiescent()?,
        "retired native scope retained common operation state",
    )?;
    require(
        StateRootLease::acquire(&root).is_err(),
        "live common owner lost its root lease",
    )?;
    drop(common);
    drop(authority);
    let reopened = StateRootLease::acquire(&root)?;
    drop(reopened);
    drop(directory);
    Ok(())
}
