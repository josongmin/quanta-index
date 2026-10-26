//! Bind repeated daemon launches to one observed path and file content.
//!
//! This checks ordinary path replacement and file modification. It does not
//! attest which inode the kernel executed or how the binary was built.

use std::{
    fs::{self, Metadata, OpenOptions},
    io::Read as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use nix::fcntl::OFlag;
use sha2::{Digest as _, Sha256};

const HASH_BUFFER_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    owner: u32,
    group: u32,
    size: u64,
    modified_seconds: i64,
    modified_nanos: i64,
    changed_seconds: i64,
    changed_nanos: i64,
}

impl FileIdentity {
    fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            links: metadata.nlink(),
            owner: metadata.uid(),
            group: metadata.gid(),
            size: metadata.size(),
            modified_seconds: metadata.mtime(),
            modified_nanos: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanos: metadata.ctime_nsec(),
        }
    }
}

struct Snapshot {
    identity: FileIdentity,
    sha256: String,
}

fn snapshot(path: &Path) -> Result<Snapshot> {
    // Reject a nonregular path early. O_NONBLOCK also prevents a FIFO swapped
    // into this path before open from blocking the caller.
    let path_before = fs::metadata(path).context("stat native daemon executable")?;
    ensure!(
        path_before.is_file(),
        "native daemon executable is not a regular file"
    );
    let identity = FileIdentity::of(&path_before);

    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(OFlag::O_NONBLOCK.bits())
        .open(path)
        .context("open native daemon executable")?;
    let opened_before = file
        .metadata()
        .context("stat opened native daemon executable")?;
    ensure!(
        opened_before.is_file() && FileIdentity::of(&opened_before) == identity,
        "native daemon executable changed before hashing"
    );

    let mut digest = Sha256::new();
    let mut buffer = [0_u8; HASH_BUFFER_BYTES];
    let maximum_read = identity
        .size
        .checked_add(1)
        .context("native daemon executable length overflows hash bound")?;
    let mut observed = 0_u64;
    loop {
        let remaining = maximum_read
            .checked_sub(observed)
            .context("native daemon executable hash bound exhausted")?;
        let requested = usize::try_from(
            remaining.min(u64::try_from(buffer.len()).context("hash buffer length overflows")?),
        )
        .context("native daemon executable read size overflows")?;
        let count = file
            .read(
                buffer
                    .get_mut(..requested)
                    .context("native daemon executable read slice absent")?,
            )
            .context("read native daemon executable")?;
        if count == 0 {
            break;
        }
        observed = observed
            .checked_add(u64::try_from(count).context("native daemon executable read overflow")?)
            .context("native daemon executable length overflow")?;
        ensure!(
            observed <= identity.size,
            "native daemon executable grew during hashing"
        );
        digest.update(
            buffer
                .get(..count)
                .context("native daemon executable read exceeded hash buffer")?,
        );
    }
    ensure!(
        observed == identity.size,
        "native daemon executable length changed during hashing"
    );

    let opened_after = file
        .metadata()
        .context("restat opened native daemon executable")?;
    let path_after = fs::metadata(path).context("restat native daemon executable path")?;
    ensure!(
        FileIdentity::of(&opened_after) == identity
            && path_after.is_file()
            && FileIdentity::of(&path_after) == identity,
        "native daemon executable changed during hashing"
    );
    let digest = digest.finalize();
    Ok(Snapshot {
        identity,
        sha256: format!("{digest:x}"),
    })
}

/// Observed path/content consistency across the first and reopened daemon.
pub(super) struct NativeExecutable {
    path: PathBuf,
    initial: Snapshot,
}

impl NativeExecutable {
    pub(super) fn capture(path: &Path) -> Result<Self> {
        ensure!(
            path.is_absolute(),
            "native daemon executable path must be absolute"
        );
        Ok(Self {
            path: path.to_path_buf(),
            initial: snapshot(path)?,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn sha256(&self) -> &str {
        &self.initial.sha256
    }

    pub(super) fn verify(&self) -> Result<()> {
        let current = snapshot(&self.path)?;
        ensure!(
            current.identity == self.initial.identity,
            "native daemon executable path identity changed"
        );
        ensure!(
            current.sha256 == self.initial.sha256,
            "native daemon executable content changed"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use anyhow::{Result, ensure};
    use nix::{sys::stat::Mode, unistd::mkfifo};

    use super::NativeExecutable;

    fn executable(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
        fs::write(path, bytes)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    #[test]
    fn unchanged_executable_verifies_and_same_length_rewrite_is_rejected() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("daemon");
        executable(&path, b"first-image")?;
        let captured = NativeExecutable::capture(&path)?;
        ensure!(captured.path() == path.as_path());
        ensure!(
            captured.sha256() == "c4fa18ea50d5eec4c0beb98a66bc09e0e807b55e47826cf3c696f7346547df8a",
            "native executable SHA differs from the independent fixed golden"
        );
        captured.verify()?;

        executable(&path, b"other-image")?;
        ensure!(
            captured.verify().is_err(),
            "same-length in-place rewrite kept the first binary binding"
        );
        Ok(())
    }

    #[test]
    fn atomic_executable_path_replacement_is_rejected() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("daemon");
        let replacement = directory.path().join("replacement");
        executable(&path, b"first-image")?;
        let captured = NativeExecutable::capture(&path)?;
        executable(&replacement, b"first-image")?;
        fs::rename(replacement, &path)?;
        ensure!(
            captured.verify().is_err(),
            "new inode with identical bytes replaced the first binary"
        );
        Ok(())
    }

    #[test]
    fn nonregular_and_missing_executable_paths_are_rejected() -> Result<()> {
        let directory = tempfile::tempdir()?;
        ensure!(NativeExecutable::capture(directory.path()).is_err());
        let socket = directory.path().join("socket");
        let _listener = std::os::unix::net::UnixListener::bind(&socket)?;
        ensure!(NativeExecutable::capture(&socket).is_err());
        let fifo = directory.path().join("fifo");
        mkfifo(&fifo, Mode::S_IRUSR | Mode::S_IWUSR)?;
        ensure!(NativeExecutable::capture(&fifo).is_err());
        let missing = directory.path().join("missing");
        ensure!(NativeExecutable::capture(&missing).is_err());

        let path = directory.path().join("daemon");
        executable(&path, b"first-image")?;
        let captured = NativeExecutable::capture(&path)?;
        fs::remove_file(path)?;
        ensure!(
            captured.verify().is_err(),
            "missing path did not preserve its metadata/read error"
        );
        Ok(())
    }
}
