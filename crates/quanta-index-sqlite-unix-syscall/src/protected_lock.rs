//! Linux OFD lock for the byte range used by SQLite's main-file locks.

use std::{fs::File, io};

/// Hold an exclusive, nonblocking lock across SQLite's PENDING, RESERVED, and
/// SHARED byte roles on this already-open source file. The caller must retain
/// the same `File` until the protected read has fully retired.
///
/// The file must be independently opened read/write. Linux OFD locks are tied
/// to the open file description and released when its final descriptor closes.
/// Contention returns the kernel's EAGAIN or EACCES; unsupported filesystems
/// and kernels return their original error without falling back to POSIX locks.
#[cfg(target_os = "linux")]
pub(super) fn lock_sqlite_source(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    // SQLite's default lock range: PENDING_BYTE=0x40000000, RESERVED_BYTE=+1,
    // SHARED_FIRST=+2, SHARED_SIZE=510. The combined span is exactly 512 bytes.
    const SQLITE_PENDING_BYTE: libc::off_t = 0x4000_0000;
    const SQLITE_LOCK_SPAN: libc::off_t = 512;

    if !file.metadata()?.is_file() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let mut lock = libc::flock {
        l_type: libc::F_WRLCK as libc::c_short,
        l_whence: libc::SEEK_SET as libc::c_short,
        l_start: SQLITE_PENDING_BYTE,
        l_len: SQLITE_LOCK_SPAN,
        l_pid: 0,
    };
    // SAFETY: `lock` is a live writable flock with the Linux ABI layout;
    // `file` owns a valid FD throughout this nonblocking fcntl call.
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLK, &mut lock) };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn lock_sqlite_source(_file: &File) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "SQLite source OFD lock requires Linux",
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{
        fs::{OpenOptions, remove_file},
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn independent_descriptions_conflict_until_holder_closes() -> io::Result<()> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "quanta-sqlite-source-ofd-{}-{nonce}.db",
            std::process::id()
        ));
        let first = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        let second = OpenOptions::new().read(true).write(true).open(&path)?;
        lock_sqlite_source(&first)?;
        let contested = lock_sqlite_source(&second).unwrap_err();
        assert!(matches!(
            contested.raw_os_error(),
            Some(libc::EAGAIN) | Some(libc::EACCES)
        ));
        drop(first);
        lock_sqlite_source(&second)?;
        drop(second);
        remove_file(path)?;
        Ok(())
    }
}
