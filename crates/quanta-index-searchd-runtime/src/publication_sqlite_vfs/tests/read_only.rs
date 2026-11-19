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
        quanta_index_contract::index::NATIVE_INDEX_STATE_VERSION == 20,
        "refresh the independent crash-WAL stamp for the current persisted format",
    )?;
    let root = PathBuf::from(root);
    let catalog = SqliteCatalog::open(&root, Duration::ZERO)?;
    drop(catalog); // Materialize a distinct committed base before the later WAL write.
    let connection = Connection::open(catalog_dir(&root).join(CATALOG_FILE_NAME))?;
    connection.execute_batch("PRAGMA wal_autocheckpoint=0;")?;
    // Independent Native20 golden: SHA256(capture-format.v1\0 || compact JSON
    // [1,20,2,bytes(source-capture-v2\0),1,[7;32]]). Only the WAL changes; never
    // compute this expected checksum with the verifier under test.
    let gold = "c9fb42237d0c58893c7a0c22bf0499d49e182aeeda0dac777f29dc4277662c1c";
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

fn legacy_terminal_input(
    root: &Path,
    retire: bool,
) -> Result<
    (
        quanta_index_core::IdempotencyKeyV1,
        quanta_index_contract::BatchPublishReceipt,
    ),
    Box<dyn std::error::Error>,
> {
    legacy_terminal_input_with(root, retire, |_| Ok(()))
}

fn legacy_terminal_input_with(
    root: &Path,
    retire: bool,
    extra: impl FnOnce(&SqliteCatalog) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<
    (
        quanta_index_core::IdempotencyKeyV1,
        quanta_index_contract::BatchPublishReceipt,
    ),
    Box<dyn std::error::Error>,
> {
    use quanta_index_contract::{
        BatchPublishReceipt, IngestOperationKindV1, ManifestGeneration, RepoId, RevisionId,
    };
    use quanta_index_core::{ClaimOutcomeV1, IdempotencyCatalogPort};
    let catalog = SqliteCatalog::open(root, Duration::ZERO)?;
    let key = quanta_index_core::IdempotencyKeyV1 {
        kind: IngestOperationKindV1::SearchCorpus,
        repo_id: RepoId::new("legacy-repo")?,
        revision_id: RevisionId::new("legacy-revision")?,
        generation: ManifestGeneration::new(17),
        batch_digest: "legacy-batch".into(),
    };
    let body = [17_u8; 32];
    let claim = match catalog.claim_prepared(
        &key,
        &body,
        "legacy-owner",
        i64::MAX.unsigned_abs(),
        &body,
    )? {
        ClaimOutcomeV1::Claimed(claim) => claim,
        _ => return Err("legacy fixture did not obtain a fresh claim".into()),
    };
    catalog.mark_applying(&claim)?;
    let mut receipt = BatchPublishReceipt::empty_for(
        key.generation,
        Some("legacy-manifest".into()),
        key.batch_digest.clone(),
    );
    receipt.accept_replace_scope();
    let sequence = catalog.commit(&claim, &receipt)?;
    extra(&catalog)?;
    if retire {
        require(
            catalog.forget_generation(&key.repo_id, &key.revision_id, key.generation)? == 1,
            "legacy fixture did not retire the exact committed generation",
        )?;
    }
    drop(catalog);

    // Leave the real committed Legacy journal/event relation, then remove
    // Native-only schema so any accidental Native stamp dependency fails.
    let connection = Connection::open(catalog_dir(root).join(CATALOG_FILE_NAME))?;
    connection.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let native_tables: Vec<String> = {
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name GLOB 'native_*'",
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    require(
        !native_tables.is_empty(),
        "legacy fixture never removed Native tables",
    )?;
    for name in native_tables {
        // Names came from the current Catalog schema, not external input.
        connection.execute_batch(&format!("DROP TABLE \"{name}\";"))?;
    }
    let remaining: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name GLOB 'native_*'",
        [],
        |row| row.get(0),
    )?;
    require(remaining == 0, "legacy fixture retained a Native table")?;
    drop(connection);
    privatize(root)?;
    Ok((key, receipt.recorded_at(sequence)))
}

fn legacy_original_budget() -> Result<WorkBudget, IndexError> {
    legacy_original_budget_with_work_bytes(64 * 1024 * 1024)
}

fn legacy_original_budget_with_work_bytes(bytes: u64) -> Result<WorkBudget, IndexError> {
    use quanta_index_core::index::{
        MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    struct Meter(AtomicU64);
    impl MemoryLeaseOwner for Meter {
        fn release(&self, bytes: u64) {
            let _prior = self.0.fetch_sub(bytes, Ordering::SeqCst);
        }
    }
    struct Original(Arc<Meter>);
    impl MemoryScopePort for Original {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let _prior = self
                .0
                .0
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                    used.checked_add(bytes)
                        .filter(|next| *next <= 64 * 1024 * 1024)
                })
                .map_err(|_| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }
    let mut budget = WorkBudget::new(
        WorkLimits {
            reads: 10_000,
            writes: 0,
            bytes,
            mutations: 0,
        },
        deadline()?,
    );
    budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Original(Arc::new(Meter(
        AtomicU64::new(0),
    )))))));
    Ok(budget)
}

#[test]
fn legacy_only_catalog_terminal_ack_replay_and_quiescence_keep_source_bytes() -> TestResult {
    use quanta_index_catalog::{
        TerminalBatchReadError, verify_legacy_catalog_quiescence_v1,
        verify_legacy_terminal_batch_receipt,
    };
    use quanta_index_contract::RepoId;
    let mut input = None;
    let fixture = fixture_with_boot_input(|root| {
        input = Some(legacy_terminal_input(root, false)?);
        Ok(16 * 1024 * 1024)
    })?;
    let (key, expected) = input.ok_or("missing genuine Legacy ACK")?;
    let before = observe(&fixture.root)?;
    let actual = verify_legacy_terminal_batch_receipt(
        &fixture.root,
        fixture.vfs.as_ref(),
        &key,
        &expected,
        &mut legacy_original_budget()?,
    )?;
    require(
        actual.receipt() == &expected,
        "committed Legacy ACK changed",
    )?;
    drop(actual);
    let replay = expected.clone().replayed();
    let actual = verify_legacy_terminal_batch_receipt(
        &fixture.root,
        fixture.vfs.as_ref(),
        &key,
        &replay,
        &mut legacy_original_budget()?,
    )?;
    require(
        actual.receipt() == &expected,
        "replay lost original applied sequence",
    )?;
    drop(actual);
    let mut wrong_manifest = expected.clone();
    wrong_manifest.manifest_digest = Some("other".into());
    require(
        matches!(
            verify_legacy_terminal_batch_receipt(
                &fixture.root,
                fixture.vfs.as_ref(),
                &key,
                &wrong_manifest,
                &mut legacy_original_budget()?,
            ),
            Err(TerminalBatchReadError::ExpectedMismatch("manifest_digest")),
        ),
        "wrong Legacy manifest was accepted",
    )?;
    let mut wrong_sequence = expected.clone();
    wrong_sequence.durable_sequence += 1;
    require(
        matches!(
            verify_legacy_terminal_batch_receipt(
                &fixture.root,
                fixture.vfs.as_ref(),
                &key,
                &wrong_sequence,
                &mut legacy_original_budget()?,
            ),
            Err(TerminalBatchReadError::ExpectedMismatch("durable_sequence")),
        ),
        "wrong Legacy sequence was accepted",
    )?;
    let mut cancelled = legacy_original_budget()?;
    cancelled.request().cancel_handle().cancel();
    require(
        matches!(
            verify_legacy_terminal_batch_receipt(
                &fixture.root,
                fixture.vfs.as_ref(),
                &key,
                &expected,
                &mut cancelled,
            ),
            Err(TerminalBatchReadError::Admission(IndexError::Cancelled)),
        ),
        "cancelled Legacy read was accepted",
    )?;
    let quiescence = verify_legacy_catalog_quiescence_v1(
        &fixture.root,
        fixture.vfs.as_ref(),
        &key.repo_id,
        &key.revision_id,
        &mut legacy_original_budget()?,
    )?;
    require(
        quiescence.terminal_rows == 1
            && quiescence.latest_retained_search_corpus_sequence == Some(expected.durable_sequence),
        "Legacy quiescence changed the committed SearchCorpus sequence",
    )?;
    require(
        !quiescence.has_history_rows_v1(),
        "SearchCorpus receipt alone does not mean that the Catalog holds History rows",
    )?;
    drop(quiescence);
    let foreign_repo = RepoId::new("foreign-legacy-repo")?;
    require(
        matches!(
            verify_legacy_catalog_quiescence_v1(
                &fixture.root,
                fixture.vfs.as_ref(),
                &foreign_repo,
                &key.revision_id,
                &mut legacy_original_budget()?,
            ),
            Err(IndexError::Conflict)
        ),
        "Legacy quiescence accepted a foreign repository",
    )?;
    require(
        observe(&fixture.root)? == before,
        "Legacy read changed Catalog source files or sidecars",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn legacy_history_snapshot_visits_only_the_requested_generation() -> TestResult {
    use quanta_index_catalog::{
        verify_legacy_catalog_quiescence_v1, visit_legacy_history_snapshot_rows_v1,
    };
    use quanta_index_contract::lex::CommitSha;
    use quanta_index_contract::{AuxEpochV1, ManifestGeneration, RepoId, RevisionId};
    use quanta_index_core::{
        AuxiliaryAuthorityCatalogPort, AuxiliaryDomainV1, AuxiliaryGenerationKeyV1,
        AuxiliaryMutationBatchV1, AuxiliaryRowFamilyV1, AuxiliaryRowKeyV1, AuxiliaryRowMutationV1,
        AuxiliaryRowV1, CoreError,
    };

    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("legacy-repo")?,
        revision_id: RevisionId::new("legacy-revision")?,
        generation: ManifestGeneration::new(18),
    };
    let epoch = quanta_index_ipc::encode_cbor_payload(&AuxEpochV1::new(7))?;
    let reference = quanta_index_ipc::encode_cbor_payload(&CommitSha::from_bytes([19; 20]))?;
    let fixture = fixture_with_boot_input(|root| {
        let _corpus = legacy_terminal_input_with(root, false, |catalog| {
            let mut other = generation.clone();
            other.generation = ManifestGeneration::new(19);
            let row = |generation, family, row_key, value| {
                AuxiliaryRowMutationV1::Upsert(AuxiliaryRowV1 {
                    key: AuxiliaryRowKeyV1 {
                        domain: AuxiliaryDomainV1::History,
                        generation,
                        family,
                        row_key,
                    },
                    value,
                })
            };
            let _written = catalog.apply(&AuxiliaryMutationBatchV1 {
                rows: vec![
                    row(
                        generation.clone(),
                        AuxiliaryRowFamilyV1::Ref,
                        b"refs/heads/main".to_vec(),
                        reference.clone(),
                    ),
                    row(
                        other,
                        AuxiliaryRowFamilyV1::Epoch,
                        Vec::new(),
                        quanta_index_ipc::encode_cbor_payload(&AuxEpochV1::new(9))?,
                    ),
                    row(
                        generation.clone(),
                        AuxiliaryRowFamilyV1::Epoch,
                        Vec::new(),
                        epoch.clone(),
                    ),
                ],
                tracks: Vec::new(),
            })?;
            Ok(())
        })?;
        Ok(16 * 1024 * 1024)
    })?;
    let before = observe(&fixture.root)?;
    let quiescent = verify_legacy_catalog_quiescence_v1(
        &fixture.root,
        fixture.vfs.as_ref(),
        &generation.repo_id,
        &generation.revision_id,
        &mut legacy_original_budget()?,
    )?;
    require(
        quiescent.has_history_rows_v1(),
        "durable History rows disappeared from the quiescent Legacy census",
    )?;
    drop(quiescent);

    let mut visits = 0;
    visit_legacy_history_snapshot_rows_v1(
        &fixture.root,
        fixture.vfs.as_ref(),
        &generation,
        &mut legacy_original_budget()?,
        &mut |row, _budget| {
            let matches_row = row.key.generation == generation
                && match visits {
                    0 => {
                        row.key.family == AuxiliaryRowFamilyV1::Epoch
                            && row.key.row_key.is_empty()
                            && row.value == epoch
                    }
                    1 => {
                        row.key.family == AuxiliaryRowFamilyV1::Ref
                            && row.key.row_key == b"refs/heads/main"
                            && row.value == reference
                    }
                    _ => false,
                };
            if !matches_row {
                return Err(CoreError::InvalidContract(
                    "unexpected row in the held Legacy History snapshot".into(),
                ));
            }
            visits += 1;
            Ok(())
        },
    )?;
    require(visits == 2, "Legacy History snapshot omitted expected rows")?;

    let mut absent = generation;
    absent.generation = ManifestGeneration::new(20);
    let mut absent_was_visited = false;
    let read = visit_legacy_history_snapshot_rows_v1(
        &fixture.root,
        fixture.vfs.as_ref(),
        &absent,
        &mut legacy_original_budget()?,
        &mut |_row, _budget| {
            absent_was_visited = true;
            Ok(())
        },
    );
    require(
        read.is_err() && !absent_was_visited,
        "missing Legacy History must refuse before visiting rows",
    )?;
    require(
        observe(&fixture.root)? == before,
        "Legacy History snapshot changed Catalog source bytes",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn legacy_history_source_requires_exact_original_ack_and_complete_inventory() -> TestResult {
    use quanta_index_catalog::{
        LegacyHistoryProducerScopeV1, LegacyHistorySourceProofV1,
        observe_legacy_history_snapshot_v1, verify_legacy_history_source_inventory_v1,
        verify_legacy_history_sources_v1, visit_legacy_history_checkpoint_pages_v1,
        visit_legacy_history_generation_keys_v1,
    };
    use quanta_index_contract::index::{
        Digest, HistoryReplayFenceV1, HistoryReplayPublicationV1, HistorySnapshotCheckpointV1,
        HistorySnapshotRowV1, HistorySourceCutV1, HistorySourceInputV1, ObjectRef,
    };
    use quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceLegacyHistoryV1;
    use quanta_index_contract::{
        AuxEpochV1, BatchPublishReceipt, HistoryIngestBatch, IngestOperationKindV1,
        ManifestGeneration, RepoId, RevisionId,
    };
    use quanta_index_core::{
        AuxiliaryAuthorityCatalogPort, AuxiliaryDomainV1, AuxiliaryGenerationKeyV1,
        AuxiliaryMutationBatchV1, AuxiliaryRowFamilyV1, AuxiliaryRowKeyV1, AuxiliaryRowMutationV1,
        AuxiliaryRowV1, ClaimOutcomeV1, CoreError, IdempotencyCatalogPort, IdempotencyKeyV1,
        domains::auxiliary::HistorySnapshotDigestV1,
    };

    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("legacy-repo")?,
        revision_id: RevisionId::new("legacy-revision")?,
        generation: ManifestGeneration::new(18),
    };
    let mut batch = HistoryIngestBatch {
        repo_id: generation.repo_id.clone(),
        revision_id: generation.revision_id.clone(),
        generation: generation.generation,
        manifest_digest: None,
        batch_digest: String::new(),
        commits: Vec::new(),
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    };
    quanta_index_ipc::stamp_batch_digest_v1(&mut batch)?;
    let epoch = AuxEpochV1::new(1);
    let epoch_value = quanta_index_ipc::encode_cbor_payload(&epoch)?;
    let meta_value = quanta_index_ipc::encode_cbor_payload(&serde_json::json!({
        "commits_materialized": false,
        "refs_materialized": false,
        "tags_materialized": false,
        "diff_hunks_materialized": false,
    }))?;
    let mut ack = None;
    let fixture = fixture_with_boot_input(|root| {
        let _search_corpus = legacy_terminal_input_with(root, false, |catalog| {
            let key = IdempotencyKeyV1 {
                kind: IngestOperationKindV1::History,
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                batch_digest: batch.batch_digest.clone(),
            };
            let digest = quanta_index_ipc::canonical_batch_digest_v1(&batch)?;
            let ClaimOutcomeV1::Claimed(claim) = catalog.claim_prepared(
                &key,
                &digest,
                "legacy-history-original",
                i64::MAX.unsigned_abs(),
                &digest,
            )?
            else {
                return Err("expected fresh History claim".into());
            };
            catalog.mark_applying(&claim)?;
            let _written = catalog.apply(&AuxiliaryMutationBatchV1 {
                rows: [
                    (AuxiliaryRowFamilyV1::Epoch, &epoch_value),
                    (AuxiliaryRowFamilyV1::StateMeta, &meta_value),
                ]
                .into_iter()
                .map(|(family, value)| {
                    AuxiliaryRowMutationV1::Upsert(AuxiliaryRowV1 {
                        key: AuxiliaryRowKeyV1 {
                            domain: AuxiliaryDomainV1::History,
                            generation: generation.clone(),
                            family,
                            row_key: Vec::new(),
                        },
                        value: value.clone(),
                    })
                })
                .collect(),
                tracks: Vec::new(),
            })?;
            let receipt =
                BatchPublishReceipt::empty_for(batch.generation, None, batch.batch_digest.clone());
            let sequence = catalog.commit(&claim, &receipt)?;
            ack = Some(receipt.recorded_at(sequence));
            Ok(())
        })?;
        Ok(16 * 1024 * 1024)
    })?;
    let ack = ack.ok_or("History receipt missing")?;
    let producer = LegacyHistoryProducerScopeV1 {
        writer_state_root: "/fixture/producer-state",
        workspace_root: "/fixture/workspace",
        contract_key: "history-fixture",
    };
    let genesis = |batch: &HistoryIngestBatch, ack: &BatchPublishReceipt| {
        quanta_index_ipc::history_replay_genesis_with_progress_v1(
            [
                producer.writer_state_root,
                producer.workspace_root,
                producer.contract_key,
                generation.repo_id.as_str(),
                generation.revision_id.as_str(),
            ],
            batch,
            ack,
            &mut |_| Ok::<_, std::convert::Infallible>(()),
        )
    };
    let first = genesis(&batch, &ack)?;
    let last = quanta_index_ipc::history_replay_advance_with_progress_v1(
        first,
        1,
        &batch,
        &ack,
        &mut |_| Ok::<_, std::convert::Infallible>(()),
    )?;
    let mut hash = HistorySnapshotDigestV1::new(&generation);
    hash.push_row(AuxiliaryRowFamilyV1::Epoch, &[], &epoch_value);
    hash.push_row(AuxiliaryRowFamilyV1::StateMeta, &[], &meta_value);
    let cut = HistorySourceCutV1 {
        generation: generation.clone(),
        durable_epoch: epoch,
        snapshot_digest: hash.finish(),
        replay_start: last,
    };
    let mut input = HistorySourceInputV1 {
        wire_version: 1,
        fence: HistoryReplayFenceV1 {
            generation: generation.clone(),
            manifest_digest: None,
            publication_count: 1,
            first_replay: first,
            final_replay: last,
            final_durable_sequence: ack.durable_sequence,
        },
        publications: vec![HistoryReplayPublicationV1 {
            ordinal: 1,
            before: first,
            after: last,
            batch,
            ack,
        }],
    };
    let before = observe(&fixture.root)?;
    let snapshot = observe_legacy_history_snapshot_v1(
        &fixture.root,
        fixture.vfs.as_ref(),
        &generation,
        &mut legacy_original_budget()?,
    )?;
    require(
        snapshot.durable_epoch == cut.durable_epoch
            && snapshot.snapshot_digest == cut.snapshot_digest
            && snapshot.row_count == 2
            && snapshot.terminal_receipt == input.publications[0].ack,
        "independent History snapshot disagrees with the persisted epoch, bytes or ACK",
    )?;
    let mut page_count = 0_u32;
    let mut restored_rows = Vec::new();
    let paged = visit_legacy_history_checkpoint_pages_v1(
        &fixture.root,
        fixture.vfs.as_ref(),
        &generation,
        &mut legacy_original_budget()?,
        &mut |page, _budget| {
            page_count += 1;
            if page.ordinal != page_count
                || page.generation != generation
                || page.validate_shape().is_err()
                || serde_json::to_vec(page)
                    .map_err(|error| CoreError::Storage(error.to_string()))?
                    .len()
                    > quanta_index_contract::index::MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1
            {
                return Err(CoreError::Storage(
                    "Legacy History export page differs".into(),
                ));
            }
            restored_rows.extend(page.rows.iter().cloned());
            Ok(())
        },
    )?;
    require(
        page_count == 1
            && restored_rows
                == vec![
                    HistorySnapshotRowV1 {
                        family: "epoch".into(),
                        row_key: Vec::new(),
                        value: epoch_value.clone(),
                    },
                    HistorySnapshotRowV1 {
                        family: "state-meta".into(),
                        row_key: Vec::new(),
                        value: meta_value.clone(),
                    },
                ]
            && paged.durable_epoch == epoch
            && paged.snapshot_digest == cut.snapshot_digest
            && paged.row_count == 2
            && paged.terminal_receipt == input.publications[0].ack,
        "paged History export differs from independently persisted rows or ACK",
    )?;
    let checkpoint_replay = quanta_index_ipc::history_snapshot_checkpoint_replay_with_progress_v1(
        [
            producer.writer_state_root,
            producer.workspace_root,
            producer.contract_key,
            fixture.root.to_str().ok_or("fixture root is not UTF-8")?,
        ],
        &generation,
        snapshot.durable_epoch,
        snapshot.snapshot_digest,
        &snapshot.terminal_receipt,
        &mut |_| Ok::<_, std::convert::Infallible>(()),
    )?;
    let mut checkpoint = HistorySnapshotCheckpointV1 {
        wire_version: 2,
        cut: HistorySourceCutV1 {
            replay_start: checkpoint_replay,
            ..cut.clone()
        },
        terminal_ack: snapshot.terminal_receipt.clone(),
        row_count: snapshot.row_count,
        // The Catalog independently verifies the Legacy facts. The candidate
        // adapter and object reader authenticate the separate immutable pages.
        pages: vec![ObjectRef(Digest([81; 32]))],
    };
    let mut required = NativeCutoverEvidenceLegacyHistoryV1 {
        generation: generation.clone(),
        batch_digest: snapshot.terminal_receipt.batch_digest.clone(),
        durable_sequence: snapshot.terminal_receipt.durable_sequence,
    };
    drop(snapshot);
    let verify_checkpoint =
        |producer,
         checkpoint: &HistorySnapshotCheckpointV1,
         required: &NativeCutoverEvidenceLegacyHistoryV1| {
            verify_legacy_history_source_inventory_v1(
                &fixture.root,
                fixture.vfs.as_ref(),
                &generation.repo_id,
                producer,
                &mut legacy_original_budget()?,
                |budget, verify| {
                    verify(
                        &checkpoint.cut,
                        LegacyHistorySourceProofV1::Snapshot {
                            checkpoint,
                            required: Some(required),
                        },
                        budget,
                    )
                },
            )
        };
    verify_checkpoint(producer, &checkpoint, &required)?;
    required.durable_sequence += 1;
    require(
        matches!(
            verify_checkpoint(producer, &checkpoint, &required),
            Err(IndexError::DigestConflict)
        ),
        "snapshot accepted a different required Legacy ACK",
    )?;
    required.durable_sequence -= 1;
    checkpoint.row_count += 1;
    require(
        matches!(
            verify_checkpoint(producer, &checkpoint, &required),
            Err(IndexError::DigestConflict)
        ),
        "snapshot accepted an unobserved Legacy row",
    )?;
    checkpoint.row_count -= 1;
    require(
        matches!(
            verify_checkpoint(
                LegacyHistoryProducerScopeV1 {
                    contract_key: "foreign",
                    ..producer
                },
                &checkpoint,
                &required
            ),
            Err(IndexError::DigestConflict)
        ),
        "snapshot replay accepted a different producer contract",
    )?;
    let verify = |producer, sources: &[(&HistorySourceCutV1, &HistorySourceInputV1)]| {
        verify_legacy_history_sources_v1(
            &fixture.root,
            fixture.vfs.as_ref(),
            &generation.repo_id,
            &generation.revision_id,
            producer,
            sources,
            &mut legacy_original_budget()?,
        )
    };
    verify(producer, &[(&cut, &input)])?;
    require(
        matches!(verify(producer, &[]), Err(IndexError::DigestConflict)),
        "retained History accepted no source",
    )?;
    for foreign in [
        LegacyHistoryProducerScopeV1 {
            writer_state_root: "/fixture/another-state",
            ..producer
        },
        LegacyHistoryProducerScopeV1 {
            workspace_root: "/fixture/another-workspace",
            ..producer
        },
        LegacyHistoryProducerScopeV1 {
            contract_key: "other-contract",
            ..producer
        },
    ] {
        require(
            matches!(
                verify(foreign, &[(&cut, &input)]),
                Err(IndexError::DigestConflict)
            ),
            "History replay accepted a different producer namespace",
        )?;
    }
    let publication = &mut input.publications[0];
    publication.ack.accept_replace_scope();
    publication.before = genesis(&publication.batch, &publication.ack)?;
    publication.after = quanta_index_ipc::history_replay_advance_with_progress_v1(
        publication.before,
        1,
        &publication.batch,
        &publication.ack,
        &mut |_| Ok::<_, std::convert::Infallible>(()),
    )?;
    input.fence.first_replay = publication.before;
    input.fence.final_replay = publication.after;
    let mut foreign_ack_cut = cut.clone();
    foreign_ack_cut.replay_start = publication.after;
    require(
        matches!(
            verify(producer, &[(&foreign_ack_cut, &input)]),
            Err(IndexError::DigestConflict)
        ),
        "foreign ACK counts accepted",
    )?;
    require(
        observe(&fixture.root)? == before,
        "History proof changed Legacy Catalog bytes",
    )?;
    fixture.vfs.drain_retirements()?;

    // A retained prior revision and the current revision must be accounted
    // for by one physical census. A proof for only the current revision may
    // not silently discard the older committed History generation.
    let generations = [
        AuxiliaryGenerationKeyV1 {
            repo_id: generation.repo_id.clone(),
            revision_id: RevisionId::new("legacy-prior-revision")?,
            generation: ManifestGeneration::new(18),
        },
        AuxiliaryGenerationKeyV1 {
            repo_id: generation.repo_id.clone(),
            revision_id: generation.revision_id.clone(),
            generation: ManifestGeneration::new(18),
        },
    ];
    let mut committed = Vec::new();
    let multi = fixture_with_boot_input(|root| {
        let _search_corpus = legacy_terminal_input_with(root, false, |catalog| {
            for next in &generations {
                let mut body = HistoryIngestBatch {
                    repo_id: next.repo_id.clone(),
                    revision_id: next.revision_id.clone(),
                    generation: next.generation,
                    manifest_digest: None,
                    batch_digest: String::new(),
                    commits: Vec::new(),
                    refs: Vec::new(),
                    tags: Vec::new(),
                    diff_hunks: Vec::new(),
                };
                quanta_index_ipc::stamp_batch_digest_v1(&mut body)?;
                let key = IdempotencyKeyV1 {
                    kind: IngestOperationKindV1::History,
                    repo_id: next.repo_id.clone(),
                    revision_id: next.revision_id.clone(),
                    generation: next.generation,
                    batch_digest: body.batch_digest.clone(),
                };
                let digest = quanta_index_ipc::canonical_batch_digest_v1(&body)?;
                let ClaimOutcomeV1::Claimed(claim) = catalog.claim_prepared(
                    &key,
                    &digest,
                    "legacy-history-multi-original",
                    i64::MAX.unsigned_abs(),
                    &digest,
                )?
                else {
                    return Err("expected fresh multi-revision History claim".into());
                };
                catalog.mark_applying(&claim)?;
                let _written = catalog.apply(&AuxiliaryMutationBatchV1 {
                    rows: [
                        (AuxiliaryRowFamilyV1::Epoch, &epoch_value),
                        (AuxiliaryRowFamilyV1::StateMeta, &meta_value),
                    ]
                    .into_iter()
                    .map(|(family, value)| {
                        AuxiliaryRowMutationV1::Upsert(AuxiliaryRowV1 {
                            key: AuxiliaryRowKeyV1 {
                                domain: AuxiliaryDomainV1::History,
                                generation: next.clone(),
                                family,
                                row_key: Vec::new(),
                            },
                            value: value.clone(),
                        })
                    })
                    .collect(),
                    tracks: Vec::new(),
                })?;
                let receipt = BatchPublishReceipt::empty_for(
                    next.generation,
                    None,
                    body.batch_digest.clone(),
                );
                let sequence = catalog.commit(&claim, &receipt)?;
                committed.push((next.clone(), body, receipt.recorded_at(sequence)));
            }
            Ok(())
        })?;
        Ok(16 * 1024 * 1024)
    })?;
    let multi_before = observe(&multi.root)?;
    let mut observed_keys = 0_usize;
    let key_count = visit_legacy_history_generation_keys_v1(
        &multi.root,
        multi.vfs.as_ref(),
        &generation.repo_id,
        &mut legacy_original_budget()?,
        &mut |key, _| {
            if generations.get(observed_keys) != Some(key) {
                return Err(IndexError::DigestConflict);
            }
            observed_keys += 1;
            Ok(())
        },
    )?;
    require(
        key_count == 2 && observed_keys == 2,
        "held Legacy History key inventory omitted a retained revision",
    )?;
    let mut proofs = Vec::new();
    for (next, body, receipt) in committed {
        let first = quanta_index_ipc::history_replay_genesis_with_progress_v1(
            [
                producer.writer_state_root,
                producer.workspace_root,
                producer.contract_key,
                next.repo_id.as_str(),
                next.revision_id.as_str(),
            ],
            &body,
            &receipt,
            &mut |_| Ok::<_, std::convert::Infallible>(()),
        )?;
        let last = quanta_index_ipc::history_replay_advance_with_progress_v1(
            first,
            1,
            &body,
            &receipt,
            &mut |_| Ok::<_, std::convert::Infallible>(()),
        )?;
        let mut digest = HistorySnapshotDigestV1::new(&next);
        digest.push_row(AuxiliaryRowFamilyV1::Epoch, &[], &epoch_value);
        digest.push_row(AuxiliaryRowFamilyV1::StateMeta, &[], &meta_value);
        let cut = HistorySourceCutV1 {
            generation: next.clone(),
            durable_epoch: epoch,
            snapshot_digest: digest.finish(),
            replay_start: last,
        };
        let input = HistorySourceInputV1 {
            wire_version: 1,
            fence: HistoryReplayFenceV1 {
                generation: next.clone(),
                manifest_digest: None,
                publication_count: 1,
                first_replay: first,
                final_replay: last,
                final_durable_sequence: receipt.durable_sequence,
            },
            publications: vec![HistoryReplayPublicationV1 {
                ordinal: 1,
                before: first,
                after: last,
                batch: body,
                ack: receipt.clone(),
            }],
        };
        let observed = observe_legacy_history_snapshot_v1(
            &multi.root,
            multi.vfs.as_ref(),
            &next,
            &mut legacy_original_budget()?,
        )?;
        let mut exported_rows = Vec::new();
        let exported = visit_legacy_history_checkpoint_pages_v1(
            &multi.root,
            multi.vfs.as_ref(),
            &next,
            &mut legacy_original_budget()?,
            &mut |page, _| {
                if page.generation != next || page.validate_shape().is_err() {
                    return Err(CoreError::Storage(
                        "wrong multi-revision History page".into(),
                    ));
                }
                exported_rows.extend(page.rows.iter().cloned());
                Ok(())
            },
        )?;
        require(
            observed.durable_epoch == epoch
                && observed.snapshot_digest == cut.snapshot_digest
                && observed.row_count == 2
                && observed.terminal_receipt == receipt
                && exported.durable_epoch == observed.durable_epoch
                && exported.snapshot_digest == observed.snapshot_digest
                && exported.row_count == observed.row_count
                && exported.terminal_receipt == observed.terminal_receipt
                && exported_rows
                    == vec![
                        HistorySnapshotRowV1 {
                            family: "epoch".into(),
                            row_key: Vec::new(),
                            value: epoch_value.clone(),
                        },
                        HistorySnapshotRowV1 {
                            family: "state-meta".into(),
                            row_key: Vec::new(),
                            value: meta_value.clone(),
                        },
                    ],
            "multi-revision History export lost rows or an exact terminal ACK",
        )?;
        proofs.push((cut, input));
    }
    let [prior, current] = proofs.as_slice() else {
        return Err("multi-revision History fixture lost a committed generation".into());
    };
    let verify_multi = |repo: &RepoId, sources: &[(&HistorySourceCutV1, &HistorySourceInputV1)]| {
        verify_legacy_history_source_inventory_v1(
            &multi.root,
            multi.vfs.as_ref(),
            repo,
            producer,
            &mut legacy_original_budget()?,
            |budget, verify| {
                for &(cut, input) in sources {
                    verify(cut, LegacyHistorySourceProofV1::Replay(input), budget)?;
                }
                Ok(())
            },
        )
    };
    verify_multi(
        &generation.repo_id,
        &[(&prior.0, &prior.1), (&current.0, &current.1)],
    )?;
    require(
        matches!(
            verify_multi(
                &generation.repo_id,
                &[(&current.0, &current.1), (&prior.0, &prior.1)],
            ),
            Err(IndexError::DigestConflict)
        ),
        "History census accepted reordered revisions at one generation",
    )?;
    require(
        matches!(
            verify_multi(&generation.repo_id, &[(&current.0, &current.1)]),
            Err(IndexError::DigestConflict)
        ) && matches!(
            verify_multi(&generation.repo_id, &[(&prior.0, &prior.1)]),
            Err(IndexError::DigestConflict)
        ),
        "History census accepted a missing revision",
    )?;
    require(
        matches!(
            verify_multi(
                &RepoId::new("foreign-repo")?,
                &[(&prior.0, &prior.1), (&current.0, &current.1)],
            ),
            Err(IndexError::Conflict)
        ),
        "History census accepted a foreign repository scope",
    )?;
    require(
        observe(&multi.root)? == multi_before,
        "multi-revision History proof changed Legacy Catalog bytes",
    )?;
    multi.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn legacy_quiescence_refuses_live_other_scope_but_allows_inactive_history() -> TestResult {
    use quanta_index_catalog::verify_legacy_catalog_quiescence_v1;
    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind};
    use quanta_index_core::{
        AuxiliaryAuthorityCatalogPort, AuxiliaryDomainV1, AuxiliaryGenerationKeyV1,
        AuxiliaryMutationBatchV1, AuxiliaryRowFamilyV1, AuxiliaryRowKeyV1, AuxiliaryRowMutationV1,
        AuxiliaryRowV1, AuxiliaryTrackRowV1,
    };

    for kind in 0..9 {
        let mut input = None;
        let fixture = fixture_with_boot_input(|root| {
            input = Some(legacy_terminal_input_with(root, false, |catalog| {
                match kind {
                    0 => {
                        let _sealed = catalog.seal_repomap_candidate(
                            "legacy-repo",
                            "legacy-revision",
                            18,
                            &[1; 32],
                            &[2; 32],
                            &[3; 32],
                            4,
                            "{}",
                        )?;
                    }
                    1 | 4 | 5 => {
                        let revision = if kind == 5 {
                            "legacy-revision"
                        } else {
                            "other-revision"
                        };
                        let _sealed = catalog.seal_repomap_candidate(
                            "legacy-repo",
                            revision,
                            18,
                            &[1; 32],
                            &[2; 32],
                            &[3; 32],
                            4,
                            "{}",
                        )?;
                        let _active = catalog.activate_repomap_candidate(
                            "legacy-repo",
                            revision,
                            18,
                            &[1; 32],
                            None,
                        )?;
                        if kind == 4 {
                            let _invalidated = catalog.invalidate_repomap_activation(
                                "legacy-repo",
                                "other-revision",
                                18,
                                "retired",
                            )?;
                        }
                    }
                    2 | 3 | 6 | 7 | 8 => {
                        let repo_id = RepoId::new(if kind == 6 { "foreign-repo" } else { "legacy-repo" })?;
                        let revision_id = RevisionId::new("other-revision")?;
                        let generation = AuxiliaryGenerationKeyV1 {
                            repo_id: repo_id.clone(),
                            revision_id: revision_id.clone(),
                            generation: ManifestGeneration::new(18),
                        };
                        let rows = if kind != 3 {
                            vec![AuxiliaryRowMutationV1::Upsert(AuxiliaryRowV1 {
                                key: AuxiliaryRowKeyV1 {
                                    domain: match kind {
                                        7 => AuxiliaryDomainV1::Runtime,
                                        8 => AuxiliaryDomainV1::Structural,
                                        _ => AuxiliaryDomainV1::History,
                                    },
                                    generation,
                                    family: if matches!(kind, 7 | 8) {
                                        AuxiliaryRowFamilyV1::StateMeta
                                    } else {
                                        AuxiliaryRowFamilyV1::Commit
                                    },
                                    row_key: b"other".to_vec(),
                                },
                                value: b"durable".to_vec(),
                            })]
                        } else {
                            Vec::new()
                        };
                        let tracks = if kind == 3 {
                            vec![AuxiliaryTrackRowV1 {
                                repo_id,
                                revision_id,
                                track: SearchPlaneTrackKind::Lexical,
                                value: b"current".to_vec(),
                            }]
                        } else {
                            Vec::new()
                        };
                        let _written = catalog.apply(&AuxiliaryMutationBatchV1 { rows, tracks })?;
                    }
                    _ => return Err("unknown Legacy quiescence fixture".into()),
                }
                Ok(())
            })?);
            Ok(16 * 1024 * 1024)
        })?;
        let (key, _) = input.ok_or("missing genuine Legacy ACK")?;
        let before = observe(&fixture.root)?;
        let observed = verify_legacy_catalog_quiescence_v1(
            &fixture.root,
            fixture.vfs.as_ref(),
            &key.repo_id,
            &key.revision_id,
            &mut legacy_original_budget()?,
        );
        if kind == 2 || kind == 4 || kind == 5 {
            let scope = observed?;
            require(
                scope.active_repomap_candidate_v1().is_some() == (kind == 5),
                "only the selected active RepoMap candidate is returned for physical verification",
            )?;
        } else {
            require(
                matches!(observed, Err(IndexError::Conflict)),
                "foreign or unresolved Legacy authority outside current scope was accepted",
            )?;
        }
        require(
            observe(&fixture.root)? == before,
            "Legacy refusal changed source bytes",
        )?;
        fixture.vfs.drain_retirements()?;
    }
    Ok(())
}

#[test]
fn legacy_quiescence_funds_quarantine_payloads_before_ledger_decode() -> TestResult {
    use quanta_index_catalog::verify_legacy_catalog_quiescence_v1;
    use quanta_index_contract::{
        QuarantineIncidentV1, QuarantineObservationEvidenceV1, QuarantineReasonCodeV1, RepoId,
        RevisionId, StateRootUuidCommitmentV1,
    };
    use quanta_index_core::CoreError;

    const PAYLOAD_BYTES: usize = 512 * 1024;
    let fixture = fixture_with_boot_input(|root| {
        let catalog = SqliteCatalog::open(root, Duration::ZERO)?;
        let evidence = QuarantineObservationEvidenceV1::new(
            vec![vec![b'/'; PAYLOAD_BYTES]],
            None,
            None,
            None,
            QuarantineReasonCodeV1::NonCanonicalSourceAddress,
            StateRootUuidCommitmentV1::from_bytes([1; 32]),
        )?;
        let identity = evidence.digest()?;
        let _incident = catalog.record_repomap_quarantine_incident(
            identity.as_bytes(),
            &[0; 32],
            0,
            "NonCanonicalSourceAddress",
            "source",
            &|sequence| {
                let storage = |error: &dyn std::fmt::Display| CoreError::Storage(error.to_string());
                let sequence = u64::try_from(sequence).map_err(|error| storage(&error))?;
                let incident = QuarantineIncidentV1::new(sequence, 0, evidence.clone())
                    .map_err(|error| storage(&error))?;
                let digest = incident.digest().map_err(|error| storage(&error))?;
                let bytes = incident
                    .encode_canonical()
                    .map_err(|error| storage(&error))?;
                Ok((*digest.as_bytes(), bytes))
            },
        )?;
        let _discarded = catalog.discard_repomap_quarantine_payload(identity.as_bytes())?;
        drop(catalog);
        privatize(root)?;
        Ok(16 * 1024 * 1024)
    })?;
    let before = observe(&fixture.root)?;
    let path = catalog_dir(&fixture.root).join(CATALOG_FILE_NAME);
    let wal = path.with_file_name(format!("{CATALOG_FILE_NAME}-wal"));
    let input_bytes = before
        .iter()
        .filter(|file| file.path == path || file.path == wal)
        .try_fold(0_u64, |bytes, file| {
            bytes.checked_add(file.bytes.len().try_into().ok()?)
        })
        .ok_or("fixture Catalog input overflow")?;
    let repo = RepoId::new("legacy-repo")?;
    let revision = RevisionId::new("legacy-revision")?;
    let verify = |budget: &mut WorkBudget| {
        verify_legacy_catalog_quiescence_v1(
            &fixture.root,
            fixture.vfs.as_ref(),
            &repo,
            &revision,
            budget,
        )
    };
    // The existing outer I/O scan plus fixed metadata fits. The logical
    // copy and hashing of the quarantine body needs a separate allowance.
    let mut limited = legacy_original_budget_with_work_bytes(
        input_bytes
            .checked_add(64 * 1024)
            .ok_or("fixture work overflow")?,
    )?;
    require(
        matches!(verify(&mut limited), Err(IndexError::ResourceLimited))
            && limited.first_denial() == Some(quanta_index_core::index::WorkBudgetDenial::Bytes),
        "quarantine payload was decoded on the fixed ledger allowance",
    )?;
    let mut record_only = legacy_original_budget_with_work_bytes(
        input_bytes
            .checked_add(64 * 1024 + 2 * u64::try_from(PAYLOAD_BYTES)?)
            .ok_or("fixture work overflow")?,
    )?;
    require(
        matches!(verify(&mut record_only), Err(IndexError::ResourceLimited))
            && record_only.first_denial()
                == Some(quanta_index_core::index::WorkBudgetDenial::Bytes)
            && record_only.usage().bytes
                >= input_bytes.saturating_add(u64::try_from(PAYLOAD_BYTES)?),
        "discard event reused the record event's payload allowance",
    )?;
    drop(verify(&mut legacy_original_budget()?)?);
    require(observe(&fixture.root)? == before, "Catalog changed")?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn legacy_active_repomap_verifies_real_objects_without_reconciliation_writes() -> TestResult {
    use quanta_index_catalog::{
        verify_legacy_active_repomap_receipts_with_siblings_v1, verify_legacy_catalog_quiescence_v1,
    };
    use quanta_index_contract::{
        CandidateCommitmentV1, CandidateObjectDigestV1, FileId, ManifestGeneration, RepoId,
        RepoMapActivateGenerationRequestV2, RepoMapExactnessSummary, RepoMapExpectedActiveV2,
        RepoMapFileNode, RepoMapGraphCoverage, RepoMapGraphCoverageClass,
        RepoMapItemIndexAvailability, RepoMapModuleId, RepoMapModuleNode, RepoMapNode,
        RepoMapPublishBundleRequestV2, RepoMapRedactionState, RepoMapSourceBundle,
        RepoRelativePath, RevisionId, SearchPlaneErrorCodeV2,
    };
    use quanta_index_core::index::existing_file::ExistingFileReadFailureV1;
    use quanta_index_repomap::{CandidateObjectAddressV1, RepoMapGenerationStore};

    const MAXIMUM_BYTES: u64 = 3 * 1024 * 1024;

    let repo = RepoId::new("legacy-repo")?;
    let revision = RevisionId::new("legacy-revision")?;
    let mut observed_receipts = None;
    let fixture = fixture_with_boot_input(|root| {
        let catalog = Arc::new(SqliteCatalog::open(root, Duration::ZERO)?);
        let store = RepoMapGenerationStore::open(root.join("repo-map"), catalog)?.store;
        let mut bundle = RepoMapSourceBundle::new(
            repo.clone(),
            revision.clone(),
            ManifestGeneration::new(18),
            "a".repeat(64),
            "snapshot-17",
            1,
            "b".repeat(64),
            RepoMapGraphCoverage {
                item_index_availability: RepoMapItemIndexAvailability::Available,
                graph_coverage_class: RepoMapGraphCoverageClass::Complete,
            },
            RepoMapExactnessSummary::Exact,
            RepoMapRedactionState::Unredacted,
        );
        let mut prior = bundle.clone().with_node(RepoMapNode::File(RepoMapFileNode {
            file_id: FileId::new("file://src/previous.rs"),
            repo_relative_path: RepoRelativePath::new("src/previous.rs"),
            line_count: 10,
        }));
        prior.manifest_generation = ManifestGeneration::new(17);
        let _prior_publish =
            store.ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(prior.clone())?)?;
        let prior_activation = store
            .activate_generation_v2(&RepoMapActivateGenerationRequestV2::for_bundle(&prior)?)?;
        let prior = RepoMapExpectedActiveV2::new(
            std::num::NonZeroU64::new(prior_activation.mutation.activation_epoch)
                .ok_or("missing prior activation epoch")?,
            CandidateCommitmentV1::from_wire_str(
                &prior_activation.mutation.new_candidate_commitment,
            )?,
        );
        let directory = "source_module/".repeat(30);
        for index in 0..1_100 {
            let path = format!("src/{directory}file_{index:04}.rs");
            bundle = bundle.with_node(RepoMapNode::File(RepoMapFileNode {
                file_id: FileId::new(format!("file://{path}")),
                repo_relative_path: RepoRelativePath::new(path),
                line_count: 10,
            }));
        }
        for index in 0..1_000 {
            let path = format!("src/{directory}module_{index:04}.rs");
            bundle = bundle.with_node(RepoMapNode::Module(RepoMapModuleNode {
                module_id: RepoMapModuleId::new(format!("module://{path}")),
                repo_relative_path: RepoRelativePath::new(path),
                qualified_name: format!("module_{index:04}"),
            }));
        }
        let published =
            store.ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(bundle.clone())?)?;
        let activated = store.activate_generation_v2(
            &RepoMapActivateGenerationRequestV2::for_bundle(&bundle)?.with_expected_active(prior),
        )?;
        observed_receipts = Some((published, activated));
        drop(store);
        privatize(root)?;
        Ok(16 * 1024 * 1024)
    })?;
    let (published, activated) = observed_receipts.ok_or("missing actual RepoMap receipts")?;
    let owner = rustix::process::geteuid().as_raw();
    let io = crate::native_object_read_io::NativeObjectReadIo::new(Arc::clone(&fixture.common));
    let mut work = legacy_original_budget()?;
    let before = observe(&fixture.root)?;
    let require_inapplicable = |revision: &RevisionId| -> TestResult {
        quanta_index_catalog::require_legacy_repomap_inapplicable_v1(
            &fixture.root,
            fixture.vfs.as_ref(),
            &repo,
            revision,
            &mut legacy_original_budget()?,
        )?;
        Ok(())
    };
    require(
        matches!(require_inapplicable(&revision), Err(error)
            if error.downcast_ref::<IndexError>() == Some(&IndexError::DigestConflict)),
        "NotApplicable accepted a RepoMap for the exact Legacy scope",
    )?;
    require_inapplicable(&RevisionId::new("sibling-revision")?)?;
    let quiescence = verify_legacy_catalog_quiescence_v1(
        &fixture.root,
        fixture.vfs.as_ref(),
        &repo,
        &revision,
        &mut work,
    )?;
    verify_legacy_active_repomap_receipts_with_siblings_v1(
        &fixture.root,
        fixture.vfs.as_ref(),
        &published,
        &activated,
        &mut work,
    )?;
    let active = quiescence
        .active_repomap_candidate_v1()
        .ok_or("no active RepoMap candidate")?;
    require(
        active.manifest_generation == 18,
        "wrong generation returned by the active RepoMap read",
    )?;
    let path = fixture.root.join("repo-map").join(
        CandidateObjectAddressV1::new(CandidateObjectDigestV1::from_bytes(active.object_address))
            .relative_path(),
    );
    let length = fs::metadata(&path)?.len();
    require(
        (2 * 1024 * 1024..=MAXIMUM_BYTES).contains(&length),
        "fixture did not create a multi-mebibyte RepoMap object",
    )?;
    let verify = |maximum_bytes, work: &mut WorkBudget| {
        RepoMapGenerationStore::verify_existing_active_publication_v1(
            &fixture.root,
            &quiescence,
            (&published, &activated),
            owner,
            maximum_bytes,
            &io,
            work,
        )
    };
    require(
        matches!(
            verify(
                length.checked_sub(1).ok_or("empty object")?,
                &mut legacy_original_budget()?
            ),
            Err(ExistingFileReadFailureV1::AdmissionOrIo(
                IndexError::DigestConflict
            ))
        ),
        "oversized active RepoMap object was accepted",
    )?;
    let verified = verify(MAXIMUM_BYTES, &mut work)?;
    verified.require_current_v1(&fixture.root, &io, &mut work)?;

    let mut replayed_publish = published.clone();
    let mut replayed_activate = activated.clone();
    replayed_publish.mutation.replayed = true;
    replayed_activate.mutation.replayed = true;
    require(
        quiescence.require_active_repomap_ack_v1(
            &replayed_publish,
            &replayed_activate,
            &mut work,
        )? == active,
        "replayed receipts did not verify the same candidate",
    )?;
    let mut wrong_seal = published.clone();
    wrong_seal.mutation.terminal_sequence += 1;
    require(
        matches!(
            verify_legacy_active_repomap_receipts_with_siblings_v1(
                &fixture.root,
                fixture.vfs.as_ref(),
                &wrong_seal,
                &activated,
                &mut legacy_original_budget()?,
            ),
            Err(IndexError::DigestConflict)
        ),
        "foreign RepoMap publish receipt matched the sibling-capable Catalog read",
    )?;
    let mut wrong_epoch = activated.clone();
    wrong_epoch.mutation.activation_epoch += 1;
    let mut wrong_prior = activated.clone();
    wrong_prior.mutation.prior_candidate_commitment = None;
    let mut wrong_commitment = activated.clone();
    wrong_commitment.mutation.new_candidate_commitment =
        CandidateCommitmentV1::from_bytes([42; 32]).to_wire_string();
    for (seal, activation) in [
        (&wrong_seal, &activated),
        (&published, &wrong_epoch),
        (&published, &wrong_prior),
        (&published, &wrong_commitment),
        (&activated, &published),
    ] {
        require(
            matches!(
                quiescence.require_active_repomap_ack_v1(seal, activation, &mut work),
                Err(IndexError::DigestConflict)
            ),
            "foreign RepoMap receipt matched the active Catalog",
        )?;
    }
    let mut wrong_metadata = activated.clone();
    wrong_metadata.authority_digest.push('0');
    require(
        matches!(
            RepoMapGenerationStore::verify_existing_active_publication_v1(
                &fixture.root,
                &quiescence,
                (&published, &wrong_metadata),
                owner,
                MAXIMUM_BYTES,
                &io,
                &mut legacy_original_budget()?,
            ),
            Err(ExistingFileReadFailureV1::AdmissionOrIo(
                IndexError::DigestConflict
            ))
        ),
        "foreign projection metadata matched the sealed RepoMap object",
    )?;

    let object = fs::read(&path)?;
    let mut malformed = object.clone();
    malformed[0] ^= 1;
    // Same inode and length still need the sealed content-address check.
    fs::write(&path, &malformed)?;
    require(
        matches!(
            verified.require_current_v1(&fixture.root, &io, &mut work),
            Err(ExistingFileReadFailureV1::AdmissionOrIo(
                IndexError::DigestConflict
            ))
        ),
        "the verified observation survived a change to the same object inode",
    )?;
    require(
        matches!(
            verify(MAXIMUM_BYTES, &mut legacy_original_budget()?),
            Err(ExistingFileReadFailureV1::Canonical(
                quanta_index_core::CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    ..
                }
            ))
        ),
        "corrupt active RepoMap object did not return CatalogRowCorrupt",
    )?;
    require(
        fs::read(&path)? == malformed,
        "verification altered the corrupt object",
    )?;
    fs::write(&path, &object)?;
    let restored = verify(MAXIMUM_BYTES, &mut legacy_original_budget()?)?;
    restored.require_current_v1(&fixture.root, &io, &mut work)?;
    let uuid_path = fixture.root.join("repo-map/root-uuid.bin");
    let mut uuid = fs::read(&uuid_path)?;
    uuid[0] ^= 1;
    fs::write(&uuid_path, &uuid)?;
    require(
        matches!(
            restored.require_current_v1(&fixture.root, &io, &mut work),
            Err(ExistingFileReadFailureV1::AdmissionOrIo(
                IndexError::DigestConflict
            ))
        ),
        "the verified observation survived a change to the root UUID",
    )?;
    fs::remove_file(&path)?;
    require(
        verify(MAXIMUM_BYTES, &mut legacy_original_budget()?).is_err(),
        "missing active RepoMap object was accepted",
    )?;
    require(
        !path.exists(),
        "verification rematerialized the missing active object",
    )?;
    require(
        observe(&fixture.root)? == before,
        "verification wrote the Legacy Catalog",
    )?;
    fixture.vfs.drain_retirements()?;
    require(
        fixture.common.is_quiescent()?,
        "RepoMap verifier retained a live file",
    )?;
    Ok(())
}

#[test]
fn legacy_multiscope_census_refuses_sealed_repomap_candidate() -> TestResult {
    use quanta_index_catalog::visit_legacy_active_catalog_heads_v1;
    use quanta_index_contract::{
        FileId, ManifestGeneration, RepoId, RepoMapExactnessSummary, RepoMapFileNode,
        RepoMapGraphCoverage, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapNode,
        RepoMapPublishBundleRequestV2, RepoMapRedactionState, RepoMapSourceBundle,
        RepoRelativePath, RevisionId,
    };
    use quanta_index_repomap::RepoMapGenerationStore;

    let fixture = fixture_with_boot_input(|root| {
        let catalog = Arc::new(SqliteCatalog::open(root, Duration::ZERO)?);
        let store = RepoMapGenerationStore::open(root.join("repo-map"), catalog)?.store;
        let bundle = RepoMapSourceBundle::new(
            RepoId::new("pending-repo")?,
            RevisionId::new("pending-revision")?,
            ManifestGeneration::new(1),
            "a".repeat(64),
            "pending-snapshot",
            1,
            "b".repeat(64),
            RepoMapGraphCoverage {
                item_index_availability: RepoMapItemIndexAvailability::Available,
                graph_coverage_class: RepoMapGraphCoverageClass::Complete,
            },
            RepoMapExactnessSummary::Exact,
            RepoMapRedactionState::Unredacted,
        )
        .with_node(RepoMapNode::File(RepoMapFileNode {
            file_id: FileId::new("file://src/pending.rs"),
            repo_relative_path: RepoRelativePath::new("src/pending.rs"),
            line_count: 1,
        }));
        let _sealed = store.ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(bundle)?)?;
        drop(store);
        privatize(root)?;
        Ok(16 * 1024 * 1024)
    })?;
    let before = observe(&fixture.root)?;
    let mut work = legacy_original_budget()?;
    require(
        matches!(
            visit_legacy_active_catalog_heads_v1(
                &fixture.root,
                fixture.vfs.as_ref(),
                &mut work,
                &mut |_, _, _, _| Ok(()),
                &mut |_, _, _| Ok(()),
            ),
            Err(IndexError::Conflict)
        ),
        "multi-head recovery accepted a RepoMap candidate awaiting activation",
    )?;
    require(
        matches!(
            quanta_index_catalog::require_legacy_repomap_inapplicable_v1(
                &fixture.root,
                fixture.vfs.as_ref(),
                &RepoId::new("absent-repo")?,
                &RevisionId::new("absent-revision")?,
                &mut legacy_original_budget()?,
            ),
            Err(IndexError::Conflict)
        ),
        "NotApplicable returned absence from a Catalog with unresolved work",
    )?;
    require(observe(&fixture.root)? == before, "Catalog changed")?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn legacy_only_catalog_retired_terminal_returns_typed_replay_floor() -> TestResult {
    use quanta_index_catalog::{TerminalBatchReadError, verify_legacy_terminal_batch_receipt};
    let mut input = None;
    let fixture = fixture_with_boot_input(|root| {
        input = Some(legacy_terminal_input(root, true)?);
        Ok(16 * 1024 * 1024)
    })?;
    let (key, expected) = input.ok_or("missing retired Legacy ACK")?;
    let before = observe(&fixture.root)?;
    require(
        matches!(
            verify_legacy_terminal_batch_receipt(
                &fixture.root,
                fixture.vfs.as_ref(),
                &key,
                &expected,
                &mut legacy_original_budget()?,
            ),
            Err(TerminalBatchReadError::Journal(
                quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::OperationReplayFloor,
                    ..
                }
            )),
        ),
        "retired Legacy key lost its typed replay floor",
    )?;
    require(
        observe(&fixture.root)? == before,
        "retired Legacy read changed Catalog source files or sidecars",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}
