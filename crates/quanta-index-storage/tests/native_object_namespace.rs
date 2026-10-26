//! RED contract tests for the object namespace lease. No reclaim is exercised:
//! these establish writer ownership before a physical deletion port exists.
#![expect(
    clippy::panic_in_result_fn,
    reason = "integration assertions are independent namespace ownership oracles"
)]

#[path = "common/namespace_authority.rs"]
mod namespace_authority;
use namespace_authority::managed as open_managed;

use quanta_index_core::index::{IndexError, WorkBudget, WorkLimits, ports::ImmutableObjectPort};
use quanta_index_storage::{FileObjectStore, FileScratchSpace};
use std::{
    error::Error,
    fs::{self, OpenOptions},
    io,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

// Keep the lock outside `objects/` so it cannot be mistaken for an immutable
// object during physical inventory. This filename is part of the test contract.
const LEASE: &str = ".object-namespace.lease";

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn budget() -> TestResult<WorkBudget> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or_else(|| io::Error::other("namespace test deadline overflow"))?;
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 1_000_000,
            mutations: 100,
        },
        deadline,
    ))
}

fn scratch(parent: &Path) -> Result<Arc<FileScratchSpace>, Box<dyn Error>> {
    Ok(Arc::new(FileScratchSpace::open_standalone(
        parent.join("scratch"),
        1_000_000,
    )?))
}

#[test]
fn managed_store_refuses_unsunk_install_before_object_or_budget_side_effects() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("store");
    let managed = open_managed(&root, scratch(parent.path())?)?;
    let mut work = budget()?;
    let before = work.usage();
    let reference = managed
        .identify(b"unreserved", &[], &mut budget()?)?
        .reference;
    assert!(matches!(
        managed.put(b"unreserved", &[], &mut work),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(work.usage(), before);
    assert!(managed.get(reference, &mut budget()?).is_err());
    assert_eq!(fs::read_dir(root.join("objects"))?.count(), 0);
    drop(managed);
    let standalone = FileObjectStore::open(&root)?;
    let object = standalone.put(b"unreserved", &[], &mut budget()?)?;
    assert_eq!(object.reference, reference);
    assert_eq!(
        standalone.get(reference, &mut budget()?)?.payload,
        b"unreserved"
    );
    Ok(())
}

#[test]
fn standalone_openers_share_namespace_but_exclude_managed_owner() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("store");
    let scratch = scratch(parent.path())?;
    let first = FileObjectStore::open_with_scratch(&root, scratch.clone())?;
    let second = FileObjectStore::open_with_scratch(&root, scratch.clone())?;
    let object = first.put(b"existing", &[], &mut budget()?)?;
    assert_eq!(
        second.get(object.reference, &mut budget()?)?.payload,
        b"existing"
    );
    assert!(open_managed(&root, scratch.clone()).is_err());
    drop(first);
    assert!(open_managed(&root, scratch.clone()).is_err());
    drop(second);
    let managed = open_managed(&root, scratch)?;
    assert_eq!(
        managed.get(object.reference, &mut budget()?)?.payload,
        b"existing"
    );
    Ok(())
}

#[test]
fn managed_lease_survives_arc_clone_and_releases_on_last_drop() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("store");
    let scratch = scratch(parent.path())?;
    let seed = FileObjectStore::open_with_scratch(&root, scratch.clone())?;
    let object = seed.put(b"kept", &[], &mut budget()?)?;
    drop(seed);
    let managed = Arc::new(open_managed(&root, scratch.clone())?);
    let clone = managed.clone();
    assert!(FileObjectStore::open(&root).is_err());
    assert!(FileObjectStore::open_with_scratch(&root, scratch.clone()).is_err());
    drop(managed);
    assert!(FileObjectStore::open_with_scratch(&root, scratch.clone()).is_err());
    assert_eq!(
        clone.get(object.reference, &mut budget()?)?.payload,
        b"kept"
    );
    drop(clone);
    let standalone = FileObjectStore::open_with_scratch(&root, scratch.clone())?;
    assert_eq!(
        standalone.get(object.reference, &mut budget()?)?.payload,
        b"kept"
    );
    drop(standalone);
    let reopened = open_managed(&root, scratch)?;
    assert_eq!(
        reopened.get(object.reference, &mut budget()?)?.payload,
        b"kept"
    );
    Ok(())
}

#[test]
fn path_aliases_cannot_bypass_exclusive_object_namespace() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("store");
    let scratch = scratch(parent.path())?;
    let managed = open_managed(&root, scratch.clone())?;

    // `..` names the same root directory without depending on symlink policy.
    let parent_alias = root.join("..").join("store");
    assert!(FileObjectStore::open_with_scratch(&parent_alias, scratch.clone()).is_err());
    assert!(open_managed(&parent_alias, scratch.clone()).is_err());

    // Following a symlink may be rejected outright; it must never create an
    // independent lock that permits a second writer of the same physical root.
    let symlink_alias = parent.path().join("store-alias");
    std::os::unix::fs::symlink(&root, &symlink_alias)?;
    assert!(FileObjectStore::open_with_scratch(&symlink_alias, scratch.clone()).is_err());
    assert!(open_managed(&symlink_alias, scratch.clone()).is_err());
    drop(managed);

    let direct = FileObjectStore::open_with_scratch(&root, scratch)?;
    drop(direct);
    Ok(())
}

#[test]
fn rejected_opener_does_not_recreate_objects_before_acquiring_namespace() -> TestResult {
    for managed_competitor in [false, true] {
        let parent = tempfile::tempdir()?;
        let root = parent.path().join("store");
        let scratch = scratch(parent.path())?;
        let seed = FileObjectStore::open_with_scratch(&root, scratch.clone())?;
        let object = seed.put(b"retained", &[], &mut budget()?)?;
        drop(seed);
        let owner = open_managed(&root, scratch.clone())?;
        let objects = root.join("objects");
        let moved = root.join("objects-retained");
        fs::rename(&objects, &moved)?;
        assert!(!objects.exists());

        let contender = if managed_competitor {
            open_managed(&root, scratch.clone())
        } else {
            FileObjectStore::open_with_scratch(&root, scratch.clone())
        };
        assert!(contender.is_err());
        assert!(
            !objects.exists(),
            "a denied opener recreated objects before acquiring the lease"
        );

        fs::rename(&moved, &objects)?;
        assert_eq!(
            owner.get(object.reference, &mut budget()?)?.payload,
            b"retained"
        );
    }
    Ok(())
}

fn private_root(root: &Path) -> Result<(), Box<dyn Error>> {
    fs::DirBuilder::new().mode(0o700).create(root)?;
    Ok(())
}

fn assert_unsafe_lease_refused(root: &Path, scratch: &Arc<FileScratchSpace>) {
    assert!(FileObjectStore::open_with_scratch(root, scratch.clone()).is_err());
    assert!(open_managed(root, scratch.clone()).is_err());
}

#[test]
fn symlink_lease_is_refused_without_touching_its_target() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("store");
    private_root(&root)?;
    let sentinel = parent.path().join("sentinel");
    fs::write(&sentinel, b"untouched")?;
    std::os::unix::fs::symlink(&sentinel, root.join(LEASE))?;
    let scratch = scratch(parent.path())?;
    assert_unsafe_lease_refused(&root, &scratch);
    assert_eq!(fs::read(sentinel)?, b"untouched");
    Ok(())
}

#[test]
fn hardlinked_lease_is_refused_without_unlinking_an_alias() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("store");
    private_root(&root)?;
    let alias = parent.path().join("other-name");
    let _file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join(LEASE))?;
    fs::hard_link(root.join(LEASE), &alias)?;
    let scratch = scratch(parent.path())?;
    assert_unsafe_lease_refused(&root, &scratch);
    assert!(alias.exists());
    Ok(())
}

#[test]
fn unsafe_lease_mode_is_refused_without_replacing_the_file() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("store");
    private_root(&root)?;
    let lease = root.join(LEASE);
    fs::write(&lease, b"marker")?;
    fs::set_permissions(&lease, fs::Permissions::from_mode(0o644))?;
    let scratch = scratch(parent.path())?;
    assert_unsafe_lease_refused(&root, &scratch);
    assert_eq!(fs::read(lease)?, b"marker");
    Ok(())
}
