//! Boot inventory through the engine-independent persisted DATA surface.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};

use quanta_index_contract::{
    GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
};
use quanta_index_core::{
    CoreError, GenerationQuarantineReasonV1, GenerationStorageKeyV1, SealedGenerationInventoryV1,
    SealedGenerationScanPort,
};
use quanta_index_lexical::LexicalGenerationScanner;

type TestResult = Result<(), Box<dyn Error>>;

const IDENTITY: &str = "search-corpus-generation-identity.cbor";
const QUARANTINE: &str = "search-corpus-generation-quarantine.cbor";
const CONTENT_WITNESS: &str = "text-authority/manifest.cbor";

fn repo() -> Result<RepoId, Box<dyn Error>> {
    Ok(RepoId::new("inventory-data-repo")?)
}

fn revision() -> Result<RevisionId, Box<dyn Error>> {
    Ok(RevisionId::new("inventory-data-revision")?)
}

fn identity(generation: u64) -> Result<GenerationSnapshot, Box<dyn Error>> {
    Ok(GenerationSnapshot {
        repo_id: repo()?,
        revision_id: revision()?,
        track: SearchPlaneTrackKind::Lexical,
        manifest_generation: ManifestGeneration::new(generation),
        manifest_digest: format!("inventory-data-manifest-{generation}"),
    })
}

fn generation_dir(root: &Path, generation: u64) -> Result<PathBuf, Box<dyn Error>> {
    Ok(
        GenerationStorageKeyV1::for_repo_revision(&repo()?, &revision()?)
            .generation_dir(root, ManifestGeneration::new(generation)),
    )
}

fn write_identity(root: &Path, generation: u64) -> Result<PathBuf, Box<dyn Error>> {
    let directory = generation_dir(root, generation)?;
    std::fs::create_dir_all(directory.join("text-authority"))?;
    let mut bytes = Vec::new();
    ciborium::into_writer(&identity(generation)?, &mut bytes)?;
    std::fs::write(directory.join(IDENTITY), bytes)?;
    std::fs::write(
        directory.join(CONTENT_WITNESS),
        b"content is not boot metadata",
    )?;
    Ok(directory)
}

fn require_durable_quarantine(root: &Path, generation: &Path) -> TestResult {
    let scanner = LexicalGenerationScanner::new(root.to_path_buf());
    let inventory = scanner.inventory_sealed_generations()?;
    if !inventory.sealed.is_empty() {
        return Err(format!("quarantined generation remained sealed: {inventory:?}").into());
    }
    let [entry] = inventory.quarantined.as_slice() else {
        return Err(format!("expected one durable quarantine: {inventory:?}").into());
    };
    if entry.path != generation
        || entry.track != SearchPlaneTrackKind::Lexical
        || entry.reason != GenerationQuarantineReasonV1::ContentCorrupt
        || !entry.detail.contains("deadbeef")
        || !entry.detail.contains("detected at unix 17")
    {
        return Err(format!("unexpected durable quarantine: {entry:?}").into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn scan_without_content_io(
    scanner: &dyn SealedGenerationScanPort,
    witness: &Path,
) -> Result<SealedGenerationInventoryV1, Box<dyn Error>> {
    use std::mem::MaybeUninit;

    use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
    use rustix::io::Errno;

    let inotify = inotify::init(CreateFlags::NONBLOCK | CreateFlags::CLOEXEC)?;
    let descriptor = inotify::add_watch(&inotify, witness, WatchFlags::OPEN | WatchFlags::ACCESS)?;
    let mut buffer = [MaybeUninit::uninit(); 512];
    let mut events = inotify::Reader::new(inotify, &mut buffer);

    let _calibration = std::fs::read(witness)?;
    let mut observed = ReadFlags::empty();
    loop {
        match events.next() {
            Ok(event) if event.wd() == descriptor => observed |= event.events(),
            Ok(event) => return Err(format!("unexpected inotify event: {event:?}").into()),
            Err(Errno::AGAIN) => break,
            Err(error) => return Err(error.into()),
        }
    }
    if !observed.contains(ReadFlags::OPEN | ReadFlags::ACCESS) {
        return Err("inotify calibration did not observe an open and read".into());
    }

    let inventory = scanner.inventory_sealed_generations()?;
    match events.next() {
        Err(Errno::AGAIN) => Ok(inventory),
        Ok(event) => Err(format!("boot inventory touched content: {event:?}").into()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(target_os = "linux"))]
fn scan_without_content_io(
    scanner: &dyn SealedGenerationScanPort,
    witness: &Path,
) -> Result<SealedGenerationInventoryV1, Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt as _;

    // APFS commonly leaves atime unchanged after a read, so atime cannot be
    // the non-Linux oracle. Make the content unreadable instead: metadata-only
    // boot inventory must still succeed, while any content open fails at the
    // filesystem boundary.
    let mode = std::fs::metadata(witness)?.permissions().mode();
    std::fs::set_permissions(witness, std::fs::Permissions::from_mode(0o000))?;
    let inaccessible = matches!(
        std::fs::File::open(witness),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied
    );
    let inventory = scanner.inventory_sealed_generations();
    std::fs::set_permissions(witness, std::fs::Permissions::from_mode(mode))?;
    if !inaccessible {
        return Err("test process can read the mode-000 content witness".into());
    }
    inventory.map_err(Into::into)
}

#[test]
fn nofeatures_inventory_quarantines_layout_without_reading_content() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("lexical");
    let sealed = write_identity(&root, 1)?;
    let family = sealed.parent().ok_or("generation has no family")?;
    std::fs::create_dir_all(family.join("g2"))?;
    let noncanonical_generation = family.join("g03");
    std::fs::create_dir_all(&noncanonical_generation)?;
    let noncanonical_family = root.join("repo");
    std::fs::create_dir_all(noncanonical_family.join("revision/g1"))?;
    let unreadable_identity = family.join("g4");
    std::fs::create_dir_all(&unreadable_identity)?;
    std::fs::write(unreadable_identity.join(IDENTITY), b"\xff\x00not-cbor")?;
    let displaced_identity = family.join("g5");
    std::fs::create_dir_all(&displaced_identity)?;
    let _copied = std::fs::copy(sealed.join(IDENTITY), displaced_identity.join(IDENTITY))?;
    let invalid_receipt = write_identity(&root, 6)?;
    std::fs::write(invalid_receipt.join(QUARANTINE), [0xff])?;
    let dangling_identity = family.join("g7");
    std::fs::create_dir_all(&dangling_identity)?;
    std::os::unix::fs::symlink("missing-identity", dangling_identity.join(IDENTITY))?;
    let oversized_identity = family.join("g8");
    std::fs::create_dir_all(&oversized_identity)?;
    let oversized_identity_file = std::fs::File::create(oversized_identity.join(IDENTITY))?;
    oversized_identity_file.set_len(4097)?;
    let trailing_identity = family.join("g9");
    std::fs::create_dir_all(&trailing_identity)?;
    let mut trailing_identity_bytes = std::fs::read(sealed.join(IDENTITY))?;
    trailing_identity_bytes.push(0xff);
    std::fs::write(trailing_identity.join(IDENTITY), trailing_identity_bytes)?;
    let trailing_receipt = write_identity(&root, 10)?;
    let mut trailing_receipt_bytes = Vec::new();
    ciborium::into_writer(
        &(
            1_u32,
            GenerationQuarantineReasonV1::ContentCorrupt
                .as_code_str()
                .to_string(),
            "trailing receipt witness".to_string(),
            10_u64,
        ),
        &mut trailing_receipt_bytes,
    )?;
    trailing_receipt_bytes.push(0xff);
    std::fs::write(trailing_receipt.join(QUARANTINE), trailing_receipt_bytes)?;
    let oversized_receipt = write_identity(&root, 11)?;
    let oversized_receipt_file = std::fs::File::create(oversized_receipt.join(QUARANTINE))?;
    oversized_receipt_file.set_len(64 * 1024 + 1)?;
    let linked_receipt = write_identity(&root, 12)?;
    std::os::unix::fs::symlink("missing-receipt", linked_receipt.join(QUARANTINE))?;
    let fifo_identity = family.join("g13");
    std::fs::create_dir_all(&fifo_identity)?;
    let fifo_status = std::process::Command::new("mkfifo")
        .arg(fifo_identity.join(IDENTITY))
        .status()?;
    if !fifo_status.success() {
        return Err("mkfifo failed for the sealed identity fixture".into());
    }
    let socket_identity = family.join("g14");
    std::fs::create_dir_all(&socket_identity)?;
    // nextest target roots can exceed macOS SUN_LEN. Reach the same directory
    // through a short alias so the fixture creates a real socket inode there.
    let socket_aliases = tempfile::tempdir_in("/tmp")?;
    let identity_alias = socket_aliases.path().join("identity");
    std::os::unix::fs::symlink(&socket_identity, &identity_alias)?;
    let _identity_socket = std::os::unix::net::UnixListener::bind(identity_alias.join(IDENTITY))?;
    let socket_receipt = write_identity(&root, 15)?;
    let receipt_alias = socket_aliases.path().join("receipt");
    std::os::unix::fs::symlink(&socket_receipt, &receipt_alias)?;
    let _receipt_socket = std::os::unix::net::UnixListener::bind(receipt_alias.join(QUARANTINE))?;
    let directory_identity = family.join("g16");
    std::fs::create_dir_all(directory_identity.join(IDENTITY))?;
    let directory_receipt = write_identity(&root, 17)?;
    std::fs::create_dir(directory_receipt.join(QUARANTINE))?;
    let fifo_receipt = write_identity(&root, 18)?;
    let fifo_status = std::process::Command::new("mkfifo")
        .arg(fifo_receipt.join(QUARANTINE))
        .status()?;
    if !fifo_status.success() {
        return Err("mkfifo failed for the quarantine receipt fixture".into());
    }

    let scanner = LexicalGenerationScanner::new(root);
    let inventory = scan_without_content_io(&scanner, &sealed.join(CONTENT_WITNESS))?;
    let [entry] = inventory.sealed.as_slice() else {
        return Err(format!("expected one sealed generation: {inventory:?}").into());
    };
    if entry.identity != identity(1)? || entry.path != sealed {
        return Err(format!("unexpected sealed entry: {entry:?}").into());
    }
    let mut quarantined = inventory
        .quarantined
        .iter()
        .map(|entry| (entry.path.as_path(), entry.reason))
        .collect::<Vec<_>>();
    quarantined.sort();
    let mut expected = vec![
        (
            noncanonical_generation.as_path(),
            GenerationQuarantineReasonV1::NonCanonicalLayout,
        ),
        (
            noncanonical_family.as_path(),
            GenerationQuarantineReasonV1::NonCanonicalLayout,
        ),
        (
            unreadable_identity.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            displaced_identity.as_path(),
            GenerationQuarantineReasonV1::ScopeMismatch,
        ),
        (
            invalid_receipt.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            dangling_identity.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            oversized_identity.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            trailing_identity.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            trailing_receipt.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            oversized_receipt.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            linked_receipt.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            fifo_identity.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            socket_identity.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            socket_receipt.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            directory_identity.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            directory_receipt.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
        (
            fifo_receipt.as_path(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
        ),
    ];
    expected.sort();
    if quarantined != expected {
        return Err(format!("unexpected quarantine set: {quarantined:?}").into());
    }

    let moved_family = temp.path().join("moved-family");
    std::fs::rename(family, &moved_family)?;
    std::os::unix::fs::symlink(&moved_family, family)?;
    let through_family_link = scan_without_content_io(&scanner, &sealed.join(CONTENT_WITNESS))?;
    if !through_family_link.sealed.is_empty()
        || through_family_link
            .quarantined
            .iter()
            .any(|entry| entry.path.starts_with(family))
    {
        return Err(format!(
            "boot inventory followed a canonical family symlink: {through_family_link:?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn quarantine_receipt_survives_scanner_restart_and_restored_content() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("lexical");
    let generation = write_identity(&root, 1)?;
    let identity_path = generation.join(IDENTITY);
    let original_identity = std::fs::read(&identity_path)?;
    let content_path = generation.join(CONTENT_WITNESS);
    let original = std::fs::read(&content_path)?;
    let damaged: &[u8] = b"damaged content";
    assert_ne!(damaged, original.as_slice());

    let row = (
        1_u32,
        GenerationQuarantineReasonV1::ContentCorrupt
            .as_code_str()
            .to_string(),
        "object digest deadbeef".to_string(),
        17_u64,
    );
    let mut bytes = Vec::new();
    ciborium::into_writer(&row, &mut bytes)?;
    std::fs::write(generation.join(QUARANTINE), bytes)?;

    for content in [damaged, original.as_slice()] {
        std::fs::write(&content_path, content)?;
        assert_eq!(std::fs::read(&content_path)?.as_slice(), content);
        require_durable_quarantine(&root, &generation)?;
    }

    std::fs::remove_file(&identity_path)?;
    require_durable_quarantine(&root, &generation)?;

    std::fs::write(&identity_path, b"malformed identity")?;
    require_durable_quarantine(&root, &generation)?;

    std::fs::remove_file(&identity_path)?;
    let outside_identity = temp.path().join("outside-identity.cbor");
    std::fs::write(&outside_identity, original_identity)?;
    std::os::unix::fs::symlink(&outside_identity, &identity_path)?;
    require_durable_quarantine(&root, &generation)?;
    Ok(())
}

#[test]
fn control_file_permission_failures_are_storage_errors_and_recover() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    let temp = tempfile::tempdir()?;
    let root = temp.path().join("lexical");
    let generation = write_identity(&root, 1)?;
    let scanner = LexicalGenerationScanner::new(root);
    let baseline = scanner.inventory_sealed_generations()?;
    if baseline.sealed.len() != 1 || !baseline.quarantined.is_empty() {
        return Err(format!("healthy generation was not inventoried: {baseline:?}").into());
    }

    for name in [IDENTITY, QUARANTINE] {
        let path = generation.join(name);
        if name == QUARANTINE {
            std::fs::write(&path, [0xff])?;
        }
        let mode = std::fs::metadata(&path)?.permissions().mode();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))?;
        let inaccessible = match std::fs::File::open(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => true,
            Err(error) => {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
                return Err(format!("{name}: expected PermissionDenied, got {error}").into());
            }
            Ok(_) => false,
        };
        let observed = scanner.inventory_sealed_generations();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
        if name == QUARANTINE {
            std::fs::remove_file(&path)?;
        }
        if !inaccessible {
            return Err(format!(
                "test process can read mode-000 {name}; PermissionDenied was not injected"
            )
            .into());
        }
        if !matches!(observed, Err(CoreError::Storage(_))) {
            return Err(format!("{name} read failure was hidden: {observed:?}").into());
        }
    }

    let recovered = scanner.inventory_sealed_generations()?;
    let [entry] = recovered.sealed.as_slice() else {
        return Err(format!("restored generation was not inventoried: {recovered:?}").into());
    };
    if entry.identity != identity(1)?
        || entry.path != generation
        || !recovered.quarantined.is_empty()
    {
        return Err(format!("unexpected recovered inventory: {recovered:?}").into());
    }
    Ok(())
}

#[test]
fn missing_root_is_an_empty_inventory_without_legacy_engine() -> TestResult {
    let temp = tempfile::tempdir()?;
    let scanner = LexicalGenerationScanner::new(temp.path().join("absent"));
    let inventory = scanner.inventory_sealed_generations()?;
    if !inventory.sealed.is_empty() || !inventory.quarantined.is_empty() {
        return Err(format!("expected empty inventory: {inventory:?}").into());
    }
    Ok(())
}

#[test]
fn file_ancestor_is_a_storage_error_without_legacy_engine() -> TestResult {
    let temp = tempfile::tempdir()?;
    let ancestor = temp.path().join("not-a-directory");
    std::fs::write(&ancestor, b"not a directory")?;
    let outside = temp.path().join("outside-lexical");
    let _outside_generation = write_identity(&outside, 1)?;
    let linked_root = temp.path().join("linked-lexical");
    std::os::unix::fs::symlink(&outside, &linked_root)?;
    let dangling_root = temp.path().join("dangling-lexical");
    std::os::unix::fs::symlink("missing-lexical", &dangling_root)?;
    let linked_parent_container = temp.path().join("linked-parent-container");
    std::fs::create_dir(&linked_parent_container)?;
    let linked_parent = linked_parent_container.join("indexes");
    std::os::unix::fs::symlink(temp.path(), &linked_parent)?;

    for (root, label) in [
        (ancestor.join("lexical"), "ENOTDIR ancestor"),
        (linked_root, "symlinked root"),
        (dangling_root, "dangling root symlink"),
        (
            linked_parent.join("outside-lexical"),
            "symlinked direct parent",
        ),
    ] {
        let scanner = LexicalGenerationScanner::new(root);
        if !matches!(
            scanner.inventory_sealed_generations(),
            Err(CoreError::Storage(_))
        ) {
            return Err(format!("expected storage error for {label}").into());
        }
    }
    Ok(())
}
