//! Regression laws exercise the real native VFS and Catalog verifier together.

use super::*;
use quanta_index_catalog::{catalog_dir, verify_native_boot_catalog};
use quanta_index_core::index::{IndexError, WorkLimits};
use std::{os::unix::fs::MetadataExt as _, time::Instant};

const CHILD_ROOT: &str = "QUANTA_TEST_READ_ONLY_WAL_ROOT";
const CHILD_NAME: &str = "publication_sqlite_vfs::tests::read_only::committed_crash_wal_is_read_without_shm_creation_or_source_mutation";

fn deadline() -> Result<Instant, IndexError> {
    Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or_else(|| {
            IndexError::BackendViolation("test native verification deadline overflow".into())
        })
}

fn budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 10_000,
            writes: 0,
            bytes: 64 * 1024 * 1024,
            mutations: 0,
        },
        deadline()?,
    ))
}

fn privatize(root: &Path) -> TestResult {
    let directory = catalog_dir(root);
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    for entry in fs::read_dir(directory)? {
        fs::set_permissions(entry?.path(), fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct ObservedFile {
    path: PathBuf,
    inode: u64,
    mode: u32,
    modified: (i64, i64),
    bytes: Vec<u8>,
}

fn observe(root: &Path) -> Result<Vec<ObservedFile>, Box<dyn std::error::Error>> {
    let directory = catalog_dir(root);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut observed = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        observed.push(ObservedFile {
            path: path.clone(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            bytes: fs::read(path)?,
        });
    }
    observed.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(observed)
}

#[test]
fn clean_catalog_verification_preserves_every_entry_and_denies_write_budget() -> TestResult {
    let mut nonce = [0; 32];
    let fixture = fixture_with_boot_input(|root| {
        let catalog = SqliteCatalog::open(root, Duration::ZERO)?;
        nonce = catalog.native_root_nonce(&mut budget()?)?;
        drop(catalog);
        privatize(root)?;
        Ok(16 * 1024 * 1024)
    })?;
    let before = observe(&fixture.root)?;
    let mut work = budget()?;
    let read = verify_native_boot_catalog(&fixture.root, fixture.vfs.as_ref(), &mut work)?;
    require(
        read.root_nonce == nonce && read.selected.is_none() && read.namespace.is_none(),
        "read-only Catalog changed or invented a boot binding",
    )?;
    require(
        work.usage().writes == 0 && work.usage().mutations == 0,
        "verification spent a write credit",
    )?;
    require(
        observe(&fixture.root)? == before,
        "clean verification changed source files",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn checkpointed_catalog_with_empty_wal_and_uri_characters_is_preserved() -> TestResult {
    let fixture = fixture_with_named_root(
        |root| {
            drop(SqliteCatalog::open(root, Duration::ZERO)?);
            let empty_wal =
                File::create(catalog_dir(root).join(format!("{CATALOG_FILE_NAME}-wal")))?;
            drop(empty_wal);
            privatize(root)?;
            Ok(16 * 1024 * 1024)
        },
        FundingPolicy::Unqualified,
        "state ?immutable=0#% 한글",
    )?;
    let before = observe(&fixture.root)?;
    let read = verify_native_boot_catalog(&fixture.root, fixture.vfs.as_ref(), &mut budget()?)?;
    require(
        read.root_nonce != [0; 32],
        "checkpointed source lost its nonce",
    )?;
    require(
        observe(&fixture.root)? == before,
        "main-only URI changed its source or sidecars",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[expect(
    clippy::exit,
    reason = "isolated crash fixture must bypass Connection destructors after a committed WAL write"
)]
fn crash_left_wal_writer_child() -> TestResult {
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        return Ok(());
    };
    require(
        quanta_index_contract::index::NATIVE_INDEX_STATE_VERSION == 18,
        "refresh the independent crash-WAL stamp for the current persisted format",
    )?;
    let root = PathBuf::from(root);
    let catalog = SqliteCatalog::open(&root, Duration::ZERO)?;
    drop(catalog); // Materialize a distinct committed base before the later WAL write.
    let connection = Connection::open(catalog_dir(&root).join(CATALOG_FILE_NAME))?;
    connection.execute_batch("PRAGMA wal_autocheckpoint=0;")?;
    // Independent Native18 golden: SHA256(capture-format.v1\0 || compact JSON
    // [1,18,2,bytes(source-capture-v2\0),[7;32]]). Only the WAL changes; never
    // compute this expected checksum with the verifier under test.
    let gold = "37edc79b60722764ec2f00ee70531b76116ffc7405d43f282266d04a7f3993af";
    let digest = gold
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Ok(u8::from_str_radix(std::str::from_utf8(pair)?, 16)?))
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let changed = connection.execute(
        "UPDATE native_capture_format_v1 SET root_nonce=?1,row_digest=?2 WHERE singleton=1",
        rusqlite::params![[7_u8; 32].as_slice(), digest],
    )?;
    require(
        changed == 1,
        "crash fixture did not update the capture singleton",
    )?;
    privatize(&root)?;
    std::process::exit(77);
}

#[test]
fn committed_crash_wal_is_read_without_shm_creation_or_source_mutation() -> TestResult {
    if std::env::var_os(CHILD_ROOT).is_some() {
        return crash_left_wal_writer_child();
    }
    for remove_shm in [false, true] {
        let fixture = fixture_with_boot_input(|root| {
            let child = std::process::Command::new(std::env::current_exe()?)
                .args(["--exact", CHILD_NAME, "--nocapture"])
                .env(CHILD_ROOT, root)
                .output()?;
            require(
                child.status.code() == Some(77),
                &format!(
                    "WAL child failed: {}",
                    String::from_utf8_lossy(&child.stderr)
                ),
            )?;
            let directory = catalog_dir(root);
            require(
                fs::metadata(directory.join(format!("{CATALOG_FILE_NAME}-wal")))?.len() > 32,
                "child left no pending WAL frames",
            )?;
            if remove_shm {
                fs::remove_file(directory.join(format!("{CATALOG_FILE_NAME}-shm")))?;
            }
            // Establish that a main-file-only read really misses the commit.
            let main_only = Connection::open_with_flags(
                format!(
                    "file:{}?immutable=1",
                    directory.join(CATALOG_FILE_NAME).display()
                ),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )?;
            let base: Vec<u8> = main_only.query_row(
                "SELECT root_nonce FROM native_capture_format_v1 WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            require(
                base != [7_u8; 32],
                "fixture's base already contains the WAL-only commit",
            )?;
            drop(main_only);
            Ok(16 * 1024 * 1024)
        })?;
        let before = observe(&fixture.root)?;
        let read = verify_native_boot_catalog(&fixture.root, fixture.vfs.as_ref(), &mut budget()?)?;
        require(
            read.root_nonce == [7; 32],
            "verifier omitted the committed crash WAL",
        )?;
        require(
            observe(&fixture.root)? == before,
            "WAL verification changed DB/WAL/SHM entries or bytes",
        )?;
        fixture.vfs.drain_retirements()?;
    }
    Ok(())
}

#[test]
fn missing_corrupt_or_budget_refused_catalog_is_not_created_or_repaired() -> TestResult {
    let missing = fixture(16 * 1024 * 1024)?;
    require(
        verify_native_boot_catalog(&missing.root, missing.vfs.as_ref(), &mut budget()?).is_err(),
        "missing Catalog was accepted",
    )?;
    require(
        !catalog_dir(&missing.root).exists(),
        "verification created a missing Catalog directory",
    )?;
    let fixture = fixture_with_boot_input(|root| {
        drop(SqliteCatalog::open(root, Duration::ZERO)?);
        let connection = Connection::open(catalog_dir(root).join(CATALOG_FILE_NAME))?;
        let changed = connection.execute(
            "UPDATE native_capture_format_v1 SET row_digest=zeroblob(32)",
            [],
        )?;
        require(changed == 1, "corruption fixture missed the singleton")?;
        drop(connection);
        privatize(root)?;
        Ok(16 * 1024 * 1024)
    })?;
    let before = observe(&fixture.root)?;
    require(
        matches!(
            verify_native_boot_catalog(&fixture.root, fixture.vfs.as_ref(), &mut budget()?),
            Err(IndexError::RebuildRequired)
        ),
        "corrupt capture stamp was not refused",
    )?;
    let mut denied = WorkBudget::new(
        WorkLimits {
            reads: 10_000,
            writes: 0,
            bytes: 0,
            mutations: 0,
        },
        deadline()?,
    );
    require(
        matches!(
            verify_native_boot_catalog(&fixture.root, fixture.vfs.as_ref(), &mut denied),
            Err(IndexError::ResourceLimited)
        ),
        "unfunded read was accepted",
    )?;
    require(
        observe(&fixture.root)? == before,
        "refusal repaired or rewrote Catalog files",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn verification_native_handles_refuse_creation_and_writes_even_without_query_only() -> TestResult {
    let fixture = fixture_with_boot_input(|root| {
        drop(SqliteCatalog::open(root, Duration::ZERO)?);
        privatize(root)?;
        Ok(16 * 1024 * 1024)
    })?;
    let before = observe(&fixture.root)?;
    {
        let _scope = fixture
            .vfs
            .enter_read_only_scope(&RequestBudgetV1::unbounded());
        let missing = fixture.root.join("must-not-exist.sqlite");
        require(
            Connection::open_with_flags_and_vfs(
                &missing,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
                fixture.vfs.name(),
            )
            .is_err(),
            "verification scope honored CREATE",
        )?;
        require(!missing.exists(), "verification created a new DB")?;
        let connection = Connection::open_with_flags_and_vfs(
            catalog_dir(&fixture.root).join(CATALOG_FILE_NAME),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        let _configured = connection.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )?;
        connection.execute_batch("PRAGMA locking_mode=EXCLUSIVE; PRAGMA query_only=OFF;")?;
        require(
            connection
                .execute("DELETE FROM native_capture_format_v1", [])
                .is_err(),
            "SQL write bypassed the native verification guard",
        )?;
        connection.close().map_err(|(_connection, error)| error)?;
    }
    require(
        observe(&fixture.root)? == before,
        "native write refusal mutated source files",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn live_sqlite_owner_refuses_verification_without_sidecar_mutation() -> TestResult {
    let mut foreign = None;
    let fixture = fixture_with_boot_input(|root| {
        drop(SqliteCatalog::open(root, Duration::ZERO)?);
        let connection = Connection::open(catalog_dir(root).join(CATALOG_FILE_NAME))?;
        connection.execute_batch("BEGIN;")?;
        let _nonce: Vec<u8> = connection.query_row(
            "SELECT root_nonce FROM native_capture_format_v1 WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        privatize(root)?;
        foreign = Some(connection);
        Ok(16 * 1024 * 1024)
    })?;
    let before = observe(&fixture.root)?;
    require(
        matches!(
            verify_native_boot_catalog(&fixture.root, fixture.vfs.as_ref(), &mut budget()?),
            Err(IndexError::Conflict)
        ),
        "live SQLite owner did not produce a lock conflict",
    )?;
    require(
        observe(&fixture.root)? == before,
        "live-owner refusal changed source files",
    )?;
    fixture.vfs.drain_retirements()?;
    drop(foreign);
    Ok(())
}

#[test]
fn native_verification_reads_observe_the_original_cancellation_without_a_vm_hook() -> TestResult {
    let fixture = fixture_with_boot_input(|root| {
        drop(SqliteCatalog::open(root, Duration::ZERO)?);
        privatize(root)?;
        Ok(16 * 1024 * 1024)
    })?;
    let before = observe(&fixture.root)?;
    let request = RequestBudgetV1::until(deadline()?);
    {
        let _scope = fixture.vfs.enter_read_only_scope(&request);
        let connection = Connection::open_with_flags_and_vfs(
            catalog_dir(&fixture.root).join(CATALOG_FILE_NAME),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        let _configured = connection.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )?;
        connection.execute_batch("PRAGMA locking_mode=EXCLUSIVE; PRAGMA shrink_memory;")?;
        request.cancel_handle().cancel();
        let read: rusqlite::Result<Vec<u8>> = connection.query_row(
            "SELECT root_nonce FROM native_capture_format_v1 WHERE singleton=1",
            [],
            |row| row.get(0),
        );
        require(
            matches!(read, Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::SystemIoFailure),
            "cancelled native read bypassed the same-request checkpoint",
        )?;
        connection.close().map_err(|(_connection, error)| error)?;
    }
    require(
        observe(&fixture.root)? == before,
        "cancelled verification mutated source files",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

fn observe_objects(root: &Path) -> Result<Vec<ObservedFile>, Box<dyn std::error::Error>> {
    let mut pending = vec![root.join("native-objects")];
    let mut observed = Vec::new();
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)?;
        let bytes = if metadata.is_dir() {
            for entry in fs::read_dir(&path)? {
                pending.push(entry?.path());
            }
            Vec::new()
        } else {
            fs::read(&path)?
        };
        observed.push(ObservedFile {
            path,
            inode: metadata.ino(),
            mode: metadata.mode(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            bytes,
        });
    }
    observed.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(observed)
}

#[test]
fn existing_object_reader_authenticates_nonempty_children_without_namespace_mutation() -> TestResult
{
    use crate::native_object_read_io::NativeObjectReadIo;
    use quanta_index_core::index::ObjectReadIoPort as _;
    use quanta_index_core::index::ports::ImmutableObjectPort as _;
    use quanta_index_storage::{FileObjectReader, FileObjectStore};
    let mut references = Vec::new();
    let fixture = fixture_with_boot_input(|root| {
        let store = FileObjectStore::open(root.join("native-objects"))?;
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 10_000,
                writes: 100,
                bytes: 64 * 1024 * 1024,
                mutations: 100,
            },
            deadline()?,
        );
        let child = store.put(b"source-content", &[], &mut work)?;
        let parent = store.put(b"source-root", &[child.reference], &mut work)?;
        references.extend([parent.reference, child.reference]);
        drop(store);
        Ok(16 * 1024 * 1024)
    })?;
    let root = fixture.root.join("native-objects");
    let before = observe_objects(&fixture.root)?;
    let io = Arc::new(NativeObjectReadIo::new(Arc::clone(&fixture.common)));
    let mut work = budget()?;
    let reader = FileObjectReader::open_existing(
        &root,
        io.open(&root.join(".object-namespace.lease"), &mut work)?,
        io.clone(),
        &mut work,
    )?;
    let binding = reader.namespace_binding()?;
    require(
        binding.root_inode != 0 && binding.objects_inode != 0,
        "reader lost namespace identity",
    )?;
    let parent_ref = *references
        .first()
        .ok_or_else(|| std::io::Error::other("missing parent fixture"))?;
    let child_ref = *references
        .get(1)
        .ok_or_else(|| std::io::Error::other("missing child fixture"))?;
    let parent = reader.get(parent_ref, &mut work)?;
    let child = reader.get(child_ref, &mut work)?;
    require(
        parent.payload == b"source-root"
            && parent.registration.children.as_slice() == [child_ref]
            && child.payload == b"source-content",
        "reader changed canonical object payload or child binding",
    )?;
    let second = FileObjectReader::open_existing(
        &root,
        io.open(&root.join(".object-namespace.lease"), &mut work)?,
        io,
        &mut work,
    );
    require(
        matches!(second, Err(IndexError::ResourceLimited)),
        "a second exclusive namespace reader was admitted",
    )?;
    drop(reader);
    require(
        work.usage().writes == 0 && work.usage().mutations == 0,
        "object reader spent mutation credit",
    )?;
    require(
        observe_objects(&fixture.root)? == before,
        "object read changed namespace entries or bytes",
    )?;
    Ok(())
}

#[test]
fn existing_object_reader_refuses_corrupt_bytes_and_missing_lease_without_repair() -> TestResult {
    use crate::native_object_read_io::NativeObjectReadIo;
    use quanta_index_core::index::ObjectReadIoPort as _;
    use quanta_index_core::index::ports::ImmutableObjectPort as _;
    use quanta_index_storage::{FileObjectReader, FileObjectStore};
    let mut reference = None;
    let fixture = fixture_with_boot_input(|root| {
        let store = FileObjectStore::open(root.join("native-objects"))?;
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 10_000,
                writes: 100,
                bytes: 64 * 1024 * 1024,
                mutations: 100,
            },
            deadline()?,
        );
        reference = Some(store.put(b"source-content", &[], &mut work)?.reference);
        drop(store);
        let object = observe_objects(root)?
            .into_iter()
            .find(|entry| entry.path.file_name().is_some_and(|name| name.len() == 64))
            .ok_or_else(|| std::io::Error::other("missing object fixture"))?;
        let mut bytes = object.bytes;
        let last = bytes
            .last_mut()
            .ok_or_else(|| std::io::Error::other("empty object fixture"))?;
        *last ^= 1;
        fs::write(object.path, bytes)?;
        Ok(16 * 1024 * 1024)
    })?;
    let root = fixture.root.join("native-objects");
    let before = observe_objects(&fixture.root)?;
    let io = Arc::new(NativeObjectReadIo::new(Arc::clone(&fixture.common)));
    let mut work = budget()?;
    let reader = FileObjectReader::open_existing(
        &root,
        io.open(&root.join(".object-namespace.lease"), &mut work)?,
        io.clone(),
        &mut work,
    )?;
    require(
        matches!(
            reader.get(
                reference.ok_or_else(|| std::io::Error::other("missing reference"))?,
                &mut work
            ),
            Err(IndexError::CorruptObject(_))
        ),
        "corrupt physical object was accepted",
    )?;
    drop(reader);
    require(
        observe_objects(&fixture.root)? == before,
        "corrupt-object refusal rewrote source",
    )?;
    fs::remove_file(root.join(".object-namespace.lease"))?;
    let missing = observe_objects(&fixture.root)?;
    require(
        io.open(&root.join(".object-namespace.lease"), &mut work)
            .is_err(),
        "missing lease was recreated",
    )?;
    require(
        observe_objects(&fixture.root)? == missing,
        "missing-lease refusal mutated namespace",
    )?;
    Ok(())
}
