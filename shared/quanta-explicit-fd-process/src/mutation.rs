//! Sticky path mutation observation for descriptor-custodied files.

#[cfg(target_os = "macos")]
use std::{fs::File, os::unix::fs::MetadataExt};
use std::{
    io,
    os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    sync::Mutex,
};

/// Records whether a watched inode or its watched custody directory mutated.
pub struct PathMutationGuardV1 {
    backend_v1: Mutex<PathMutationBackendV1>,
}

impl PathMutationGuardV1 {
    pub fn try_new_v1<'a>(
        descriptors_v1: impl IntoIterator<Item = BorrowedFd<'a>>,
    ) -> io::Result<Self> {
        Ok(Self {
            backend_v1: Mutex::new(PathMutationBackendV1::try_new_v1(descriptors_v1)?),
        })
    }

    /// Add one descriptor to the sticky mutation set.
    ///
    /// The backend duplicates and owns the descriptor before registering it,
    /// so the watch cannot disappear when the caller closes or reuses its FD.
    pub fn try_watch_v1(&self, descriptor_v1: BorrowedFd<'_>) -> io::Result<()> {
        self.backend_v1
            .lock()
            .map_err(|_| io::Error::other("path mutation observer lock poisoned"))?
            .try_watch_v1(descriptor_v1)
    }

    pub fn mutation_observed_v1(&self) -> io::Result<bool> {
        self.backend_v1
            .lock()
            .map_err(|_| io::Error::other("path mutation observer lock poisoned"))?
            .mutation_observed_v1()
    }
}

#[cfg(target_os = "macos")]
struct PathMutationBackendV1 {
    queue_v1: OwnedFd,
    watched_v1: Vec<WatchedVnodeV1>,
    mutation_seen_v1: bool,
}

#[cfg(target_os = "macos")]
struct WatchedVnodeV1 {
    file_v1: File,
    stable_metadata_v1: StableVnodeMetadataV1,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, PartialEq, Eq)]
struct StableVnodeMetadataV1 {
    device_v1: u64,
    inode_v1: u64,
    mode_v1: u32,
    links_v1: u64,
    owner_v1: u32,
    group_v1: u32,
    size_v1: u64,
    modified_v1: (i64, i64),
    changed_v1: (i64, i64),
}

#[cfg(target_os = "macos")]
impl StableVnodeMetadataV1 {
    fn from_metadata_v1(metadata_v1: &std::fs::Metadata) -> Self {
        Self {
            device_v1: metadata_v1.dev(),
            inode_v1: metadata_v1.ino(),
            mode_v1: metadata_v1.mode(),
            links_v1: metadata_v1.nlink(),
            owner_v1: metadata_v1.uid(),
            group_v1: metadata_v1.gid(),
            size_v1: metadata_v1.size(),
            modified_v1: (metadata_v1.mtime(), metadata_v1.mtime_nsec()),
            changed_v1: (metadata_v1.ctime(), metadata_v1.ctime_nsec()),
        }
    }
}

#[cfg(target_os = "macos")]
impl PathMutationBackendV1 {
    fn try_new_v1<'a>(
        descriptors_v1: impl IntoIterator<Item = BorrowedFd<'a>>,
    ) -> io::Result<Self> {
        // SAFETY: kqueue returns a fresh descriptor on success.
        let raw_queue_v1 = unsafe { libc::kqueue() };
        if raw_queue_v1 < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful kqueue returned a fresh owned descriptor.
        let queue_v1 = unsafe { OwnedFd::from_raw_fd(raw_queue_v1) };
        let mut backend_v1 = Self {
            queue_v1,
            watched_v1: Vec::new(),
            mutation_seen_v1: false,
        };
        for descriptor_v1 in descriptors_v1 {
            backend_v1.try_watch_v1(descriptor_v1)?;
        }
        Ok(backend_v1)
    }

    fn try_watch_v1(&mut self, descriptor_v1: BorrowedFd<'_>) -> io::Result<()> {
        let file_v1 = File::from(duplicate_descriptor_v1(descriptor_v1)?);
        let stable_metadata_v1 = StableVnodeMetadataV1::from_metadata_v1(&file_v1.metadata()?);
        let change_v1 = libc::kevent {
            ident: usize::try_from(file_v1.as_raw_fd()).expect("file descriptor is negative"),
            filter: libc::EVFILT_VNODE,
            flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_CLEAR,
            fflags: libc::NOTE_DELETE
                | libc::NOTE_WRITE
                | libc::NOTE_EXTEND
                | libc::NOTE_ATTRIB
                | libc::NOTE_LINK
                | libc::NOTE_RENAME
                | libc::NOTE_REVOKE,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: pointers reference one initialized registration and no output buffer.
        let registered_v1 = unsafe {
            libc::kevent(
                self.queue_v1.as_raw_fd(),
                &change_v1,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if registered_v1 < 0 {
            return Err(io::Error::last_os_error());
        }
        self.watched_v1.push(WatchedVnodeV1 {
            file_v1,
            stable_metadata_v1,
        });
        Ok(())
    }

    fn mutation_observed_v1(&mut self) -> io::Result<bool> {
        if self.mutation_seen_v1 {
            return Ok(true);
        }
        let timeout_v1 = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // Reading a watched rlib/rmeta may change only atime. Darwin reports
        // that as NOTE_ATTRIB; it is not a source-content mutation. Drain all
        // events, retaining a sticky result for any write, rename, or metadata
        // change other than atime (including a chmod-then-restore via ctime).
        for _ in 0..self.watched_v1.len().saturating_mul(4).max(16) {
            let mut event_v1 = std::mem::MaybeUninit::<libc::kevent>::uninit();
            // SAFETY: event points to one writable event and timeout is initialized.
            let observed_v1 = unsafe {
                libc::kevent(
                    self.queue_v1.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    event_v1.as_mut_ptr(),
                    1,
                    &timeout_v1,
                )
            };
            if observed_v1 < 0 {
                return Err(io::Error::last_os_error());
            }
            if observed_v1 == 0 {
                // OK-EMPTY-JUSTIFIED: the nonblocking queue is drained; no mutation event remains.
                return Ok(false);
            }
            // SAFETY: a positive kevent result initialized the output slot.
            let event_v1 = unsafe { event_v1.assume_init() };
            if event_v1.flags & libc::EV_ERROR != 0
                || event_v1.fflags == 0
                || event_v1.fflags & !libc::NOTE_ATTRIB != 0
            {
                self.mutation_seen_v1 = true;
                return Ok(true);
            }
            let Some(watched_v1) = self
                .watched_v1
                .iter()
                .find(|watched_v1| watched_v1.file_v1.as_raw_fd() as usize == event_v1.ident)
            else {
                self.mutation_seen_v1 = true;
                return Ok(true);
            };
            let stable_now_v1 = watched_v1
                .file_v1
                .metadata()
                .map(|metadata_v1| StableVnodeMetadataV1::from_metadata_v1(&metadata_v1));
            if !matches!(stable_now_v1, Ok(ref metadata_v1) if metadata_v1 == &watched_v1.stable_metadata_v1)
            {
                self.mutation_seen_v1 = true;
                return Ok(true);
            }
        }
        self.mutation_seen_v1 = true;
        Ok(true)
    }
}

#[cfg(target_os = "linux")]
struct PathMutationBackendV1 {
    queue_v1: OwnedFd,
    watched_v1: Vec<OwnedFd>,
}

#[cfg(target_os = "linux")]
impl PathMutationBackendV1 {
    fn try_new_v1<'a>(
        descriptors_v1: impl IntoIterator<Item = BorrowedFd<'a>>,
    ) -> io::Result<Self> {
        // SAFETY: inotify_init1 returns a fresh descriptor on success.
        let raw_queue_v1 = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if raw_queue_v1 < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful inotify_init1 returned a fresh owned descriptor.
        let queue_v1 = unsafe { OwnedFd::from_raw_fd(raw_queue_v1) };
        let mut backend_v1 = Self {
            queue_v1,
            watched_v1: Vec::new(),
        };
        for descriptor_v1 in descriptors_v1 {
            backend_v1.try_watch_v1(descriptor_v1)?;
        }
        Ok(backend_v1)
    }

    fn try_watch_v1(&mut self, descriptor_v1: BorrowedFd<'_>) -> io::Result<()> {
        let descriptor_v1 = duplicate_descriptor_v1(descriptor_v1)?;
        let mask_v1 = libc::IN_ATTRIB
            | libc::IN_CLOSE_WRITE
            | libc::IN_CREATE
            | libc::IN_DELETE
            | libc::IN_DELETE_SELF
            | libc::IN_MODIFY
            | libc::IN_MOVE_SELF
            | libc::IN_MOVED_FROM
            | libc::IN_MOVED_TO
            | libc::IN_UNMOUNT;
        let descriptor_path_v1 =
            std::ffi::CString::new(format!("/proc/self/fd/{}", descriptor_v1.as_raw_fd()))
                .expect("descriptor path cannot contain NUL");
        // SAFETY: queue is live and descriptor path is NUL-terminated.
        let watch_v1 = unsafe {
            libc::inotify_add_watch(self.queue_v1.as_raw_fd(), descriptor_path_v1.as_ptr(), mask_v1)
        };
        if watch_v1 < 0 {
            return Err(io::Error::last_os_error());
        }
        self.watched_v1.push(descriptor_v1);
        Ok(())
    }

    fn mutation_observed_v1(&mut self) -> io::Result<bool> {
        let mut buffer_v1 = [0u8; 4096];
        // SAFETY: read writes at most buffer length bytes to initialized storage.
        let read_v1 = unsafe {
            libc::read(self.queue_v1.as_raw_fd(), buffer_v1.as_mut_ptr().cast(), buffer_v1.len())
        };
        if read_v1 > 0 {
            return Ok(true);
        }
        if read_v1 == 0 {
            return Ok(false);
        }
        let error_v1 = io::Error::last_os_error();
        if error_v1.kind() == io::ErrorKind::WouldBlock {
            Ok(false)
        } else {
            Err(error_v1)
        }
    }
}

fn duplicate_descriptor_v1(descriptor_v1: BorrowedFd<'_>) -> io::Result<OwnedFd> {
    // SAFETY: F_DUPFD_CLOEXEC creates a new owned descriptor on success.
    let duplicated_v1 = unsafe { libc::fcntl(descriptor_v1.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if duplicated_v1 < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fcntl returned a fresh descriptor owned by this function.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicated_v1) })
}

#[cfg(all(test, target_os = "macos"))]
mod darwin_tests_v1 {
    use super::PathMutationGuardV1;
    use std::{fs, os::fd::AsFd};

    #[test]
    fn compiler_style_read_does_not_mutate_a_custodied_file_v1() {
        let root_v1 = tempfile::tempdir().expect("temporary root");
        let path_v1 = root_v1.path().join("dependency.rmeta");
        fs::write(&path_v1, b"immutable dependency").expect("write dependency");
        let retained_v1 = fs::File::open(&path_v1).expect("retain dependency");
        let guard_v1 =
            PathMutationGuardV1::try_new_v1([retained_v1.as_fd()]).expect("watch dependency");
        assert_eq!(fs::read(&path_v1).expect("read dependency"), b"immutable dependency");
        assert!(!guard_v1
            .mutation_observed_v1()
            .expect("read mutation status"));
    }

    #[test]
    fn swap_restore_remains_sticky_even_when_visible_bytes_match_v1() {
        let root_v1 = tempfile::tempdir().expect("temporary root");
        let path_v1 = root_v1.path().join("dependency.rmeta");
        let backup_v1 = root_v1.path().join("backup.rmeta");
        fs::write(&path_v1, b"immutable dependency").expect("write dependency");
        let retained_v1 = fs::File::open(&path_v1).expect("retain dependency");
        let guard_v1 =
            PathMutationGuardV1::try_new_v1([retained_v1.as_fd()]).expect("watch dependency");
        fs::rename(&path_v1, &backup_v1).expect("move original");
        fs::write(&path_v1, b"replacement").expect("replace dependency");
        fs::remove_file(&path_v1).expect("remove replacement");
        fs::rename(&backup_v1, &path_v1).expect("restore original");
        assert!(guard_v1
            .mutation_observed_v1()
            .expect("read mutation status"));
        assert!(guard_v1.mutation_observed_v1().expect("mutation is sticky"));
    }
}
