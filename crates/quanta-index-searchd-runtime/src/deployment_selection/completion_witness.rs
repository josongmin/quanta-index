//! Bounded evidence retained beside a recovery selection.
//!
//! A staged sidecar is data. The selected inode's attribute makes it
//! discoverable only after the canonical selection publisher renames that
//! inode. A reader must still verify the recovery domain before settling it.

use super::*;
use quanta_index_contract::native_cutover_evidence_v1::{
    MAX_NATIVE_CUTOVER_CONTROL_BYTES_V1, MAX_NATIVE_CUTOVER_EVIDENCE_TEXT_BYTES_V1,
};
use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

// The Legacy identity and RepoMap path plus two receipts contribute up to
// nineteen bounded text fields. Account for the History terminal and framing.
pub(super) const MAX_WITNESS_BYTES: usize = MAX_NATIVE_CUTOVER_CONTROL_BYTES_V1
    + 19 * MAX_NATIVE_CUTOVER_EVIDENCE_TEXT_BYTES_V1
    + 1024;
const ATTRIBUTE_BYTES: usize = 33;
const WITNESS_DOMAIN: &[u8] = b"quanta.native-cutover.selection-witness.v1\0";

#[cfg(target_os = "linux")]
const WITNESS_ATTRIBUTE: &str = "user.quanta-index.recovery-witness";
#[cfg(target_os = "macos")]
const WITNESS_ATTRIBUTE: &str = "com.quanta-index.recovery-witness";

#[derive(Clone, Copy)]
struct WitnessAttribute {
    slot: u8,
    digest: Digest,
}

fn sidecar_name(selection_name: &OsStr, slot: u8) -> Result<String> {
    ensure!(slot < 2, "invalid recovery witness slot");
    let name = selection_name.as_bytes();
    ensure!(
        !name.is_empty()
            && name.len() <= 255
            && !name.contains(&b'/')
            && name != b"."
            && name != b"..",
        "invalid recovery selection basename"
    );
    let identity = Sha256::digest(name);
    Ok(format!(".quanta-recovery-witness-{identity:x}-{slot}"))
}

fn temporary_name(selection_name: &OsStr, slot: u8) -> Result<String> {
    Ok(format!("{}.tmp", sidecar_name(selection_name, slot)?))
}

fn witness_digest(
    selection_name: &OsStr,
    selection: &File,
    slot: u8,
    bytes: &[u8],
    budget: &mut WorkBudget,
) -> Result<Digest> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_WITNESS_BYTES,
        "invalid recovery witness length"
    );
    charge(budget, 1, 0, u64::try_from(bytes.len())?, 0)?;
    let metadata = selection.metadata()?;
    control_metadata(&metadata)?;
    let mut digest = Sha256::new();
    digest.update(WITNESS_DOMAIN);
    digest.update(u64::try_from(selection_name.as_bytes().len())?.to_be_bytes());
    digest.update(selection_name.as_bytes());
    digest.update([slot]);
    digest.update(metadata.dev().to_be_bytes());
    digest.update(metadata.ino().to_be_bytes());
    digest.update(u64::try_from(bytes.len())?.to_be_bytes());
    digest.update(bytes);
    Ok(Digest(digest.finalize().into()))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_attribute(selection: &File, budget: &mut WorkBudget) -> Result<Option<WitnessAttribute>> {
    charge(budget, 1, 0, ATTRIBUTE_BYTES as u64, 0)?;
    let mut bytes = [0_u8; ATTRIBUTE_BYTES + 1];
    let count = match rustix::fs::fgetxattr(selection, WITNESS_ATTRIBUTE, &mut bytes) {
        Ok(count) => count,
        #[cfg(target_os = "linux")]
        Err(rustix::io::Errno::NODATA) => return Ok(None),
        #[cfg(target_os = "macos")]
        Err(rustix::io::Errno::NOATTR) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        count == ATTRIBUTE_BYTES && bytes[0] < 2,
        "invalid recovery witness attribute"
    );
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(&bytes[1..ATTRIBUTE_BYTES]);
    Ok(Some(WitnessAttribute {
        slot: bytes[0],
        digest: Digest(digest),
    }))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_attribute(_: &File, _: &mut WorkBudget) -> Result<Option<WitnessAttribute>> {
    Err(anyhow!(
        "recovery witness requires file extended attributes"
    ))
}

fn with_checked_sidecar<T>(
    directory: &File,
    selection_name: &OsStr,
    selection: &File,
    attribute: WitnessAttribute,
    budget: &mut WorkBudget,
    read: impl FnOnce(&[u8], &mut WorkBudget) -> Result<T>,
) -> Result<T> {
    let name = sidecar_name(selection_name, attribute.slot)?;
    charge(budget, 5, 0, 0, 0)?;
    let mut file = File::from(
        rustix::fs::openat(
            directory,
            name.as_str(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            if error == rustix::io::Errno::NOENT {
                anyhow::Error::from(error)
                    .context(format!("selected recovery witness sidecar is absent: {error}"))
            } else {
                error.into()
            }
        })?,
    );
    let before = file.metadata()?;
    control_metadata(&before)?;
    ensure!(
        (1..=MAX_WITNESS_BYTES as u64).contains(&before.len()),
        "invalid recovery witness length"
    );
    let read_limit = before.len() + 1;
    let _memory = budget
        .reserve_memory(read_limit, MemoryLifetime::Temporary)?
        .ok_or_else(|| anyhow!("recovery witness requires admitted memory"))?;
    charge(budget, 0, 0, read_limit, 0)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(usize::try_from(read_limit)?)?;
    let _read = (&mut file).take(read_limit).read_to_end(&mut bytes)?;
    ensure!(
        u64::try_from(bytes.len())? == before.len(),
        "invalid recovery witness length"
    );
    require_named_control_file(directory, Path::new(&name), &file)?;
    ensure!(
        witness_digest(selection_name, selection, attribute.slot, &bytes, budget)?
            == attribute.digest,
        "recovery witness differs from selected inode"
    );
    let outcome = read(&bytes, budget)?;
    let mut check = [0_u8; 4096];
    charge(budget, 5 + u64::try_from(bytes.len().div_ceil(check.len()))?, 0, read_limit, 0)?;
    let _start = file.seek(SeekFrom::Start(0))?;
    for chunk in bytes.chunks(check.len()) {
        let buffer = &mut check[..chunk.len()];
        file.read_exact(buffer)?;
        ensure!(&*buffer == chunk, "recovery witness changed while read");
    }
    ensure!(
        file.read(&mut check[..1])? == 0,
        "recovery witness grew while read"
    );
    require_named_control_file(directory, Path::new(&name), &file)?;
    let after = file.metadata()?;
    ensure!(
        before.len() == after.len()
            && before.mtime() == after.mtime()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec(),
        "recovery witness changed during verification"
    );
    ensure!(
        read_attribute(selection, budget)?.is_some_and(|current| {
            current.slot == attribute.slot && current.digest == attribute.digest
        }),
        "selected recovery witness changed"
    );
    Ok(outcome)
}

/// Read only an inode-bound sidecar. Absence is distinct from a malformed or
/// missing sidecar. The caller must hold the deployment lock throughout.
pub(super) fn with_read<T>(
    directory: &File,
    selection_name: &OsStr,
    selection: &File,
    budget: &mut WorkBudget,
    read: impl FnOnce(&[u8], Digest, &mut WorkBudget) -> Result<T>,
) -> Result<Option<T>> {
    charge(budget, 3, 0, 0, 0)?;
    require_named_control_file(directory, Path::new(selection_name), selection)?;
    let Some(attribute) = read_attribute(selection, budget)? else {
        charge(budget, 2, 0, 0, 0)?;
        require_named_control_file(directory, Path::new(selection_name), selection)?;
        return Ok(None);
    };
    let outcome = with_checked_sidecar(
        directory,
        selection_name,
        selection,
        attribute,
        budget,
        |bytes, budget| read(bytes, attribute.digest, budget),
    )?;
    charge(budget, 2, 0, 0, 0)?;
    require_named_control_file(directory, Path::new(selection_name), selection)?;
    Ok(Some(outcome))
}

/// Stage the next immutable witness in the inactive slot, then bind it to the
/// new selection inode. The canonical publisher must fsync that inode before
/// rename, and must keep its deployment lock for the whole transaction.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn stage(
    directory: &File,
    selection_name: &OsStr,
    active: &File,
    prepared: &File,
    bytes: &[u8],
    budget: &mut WorkBudget,
) -> Result<()> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_WITNESS_BYTES,
        "invalid recovery witness length"
    );
    charge(budget, 2, 0, 0, 0)?;
    require_named_control_file(directory, Path::new(selection_name), active)?;
    let active_slot = read_attribute(active, budget)?.map(|attribute| attribute.slot);
    let slot = active_slot.map_or(0, |value| value ^ 1);
    let name = sidecar_name(selection_name, slot)?;
    let temp = temporary_name(selection_name, slot)?;

    // The deterministic scratch name bounds crashed attempts to one leftover
    // per slot. An old scratch file is removed only while held and verified.
    charge(budget, 2, 0, 0, 0)?;
    match rustix::fs::openat(
        directory,
        temp.as_str(),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => {
            let stale = File::from(fd);
            require_named_control_file(directory, Path::new(&temp), &stale)?;
            charge(budget, 0, 1, 0, 1)?;
            rustix::fs::unlinkat(directory, temp.as_str(), AtFlags::empty())?;
        }
        Err(rustix::io::Errno::NOENT) => {}
        Err(error) => return Err(error.into()),
    }
    charge(budget, 2, 3, u64::try_from(bytes.len())?, 1)?;
    let mut file = File::from(rustix::fs::openat(
        directory,
        temp.as_str(),
        OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )?);
    file.write_all(bytes)?;
    file.sync_all()?;
    require_named_control_file(directory, Path::new(&temp), &file)?;
    let digest = witness_digest(selection_name, prepared, slot, bytes, budget)?;
    charge(budget, 2, 2, 0, 1)?;
    rustix::fs::renameat(directory, temp.as_str(), directory, name.as_str())?;
    require_named_control_file(directory, Path::new(&name), &file)?;
    directory.sync_all()?;
    let mut attribute = [0_u8; ATTRIBUTE_BYTES];
    attribute[0] = slot;
    attribute[1..].copy_from_slice(&digest.0);
    charge(budget, 0, 1, ATTRIBUTE_BYTES as u64, 1)?;
    rustix::fs::fsetxattr(
        prepared,
        WITNESS_ATTRIBUTE,
        &attribute,
        rustix::fs::XattrFlags::CREATE,
    )?;
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn stage(
    _: &File,
    _: &OsStr,
    _: &File,
    _: &File,
    _: &[u8],
    _: &mut WorkBudget,
) -> Result<()> {
    Err(anyhow!(
        "recovery witness requires file extended attributes"
    ))
}

pub(super) fn require(
    directory: &File,
    selection_name: &OsStr,
    file_name: &OsStr,
    selection: &File,
    expected: &[u8],
    budget: &mut WorkBudget,
) -> Result<()> {
    ensure!(
        !expected.is_empty() && expected.len() <= MAX_WITNESS_BYTES,
        "invalid expected witness length"
    );
    // The digest is bound to the final selection name, while the same inode
    // is still named by its temporary entry before the canonical rename.
    charge(budget, 2, 0, 0, 0)?;
    require_named_control_file(directory, Path::new(file_name), selection)?;
    let attribute = read_attribute(selection, budget)?
        .ok_or_else(|| anyhow!("selected recovery witness is absent"))?;
    with_checked_sidecar(
        directory,
        selection_name,
        selection,
        attribute,
        budget,
        |bytes, _| {
            ensure!(bytes == expected, "selected recovery witness differs");
            Ok(())
        },
    )?;
    charge(budget, 2, 0, 0, 0)?;
    require_named_control_file(directory, Path::new(file_name), selection)
}
