//! Process-bootstrap selection of Git and its immutable ambient environment.
//! Bootstrap storage predates request roots. Requests borrow this authority;
//! they never recapture PATH or the ambient environment after admission.

use crate::{platform, ExplicitFdProcessErrorV1 as Error, PinnedExecutableV1};
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

struct SelectedGitExecutableV1 {
    executable_v1: PinnedExecutableV1,
    #[cfg(test)]
    selected_path_v1: PathBuf,
    #[cfg(test)]
    sha256_v1: [u8; 32],
}

pub struct GitProcessBootstrapAuthorityV1 {
    #[cfg(test)]
    captured_at_v1: SystemTime,
    environment_v1: Vec<(OsString, OsString)>,
    selected_v1: Result<SelectedGitExecutableV1, Error>,
}

impl GitProcessBootstrapAuthorityV1 {
    /// Call only while constructing the process-owned provider, before any
    /// request is admitted. A missing Git installation is retained as a typed
    /// bootstrap result; repositories without a Git marker need no executable.
    pub fn capture_current_process_v1() -> Self {
        let captured_at_v1 = SystemTime::now();
        Self::from_process_bootstrap_inputs_v1(
            captured_at_v1,
            std::env::vars_os().collect(),
            std::env::current_dir()
                .map_err(|error_v1| io_error_v1("git_bootstrap_cwd_v1", error_v1)),
        )
    }

    fn from_process_bootstrap_inputs_v1(
        captured_at_v1: SystemTime,
        mut environment_v1: Vec<(OsString, OsString)>,
        cwd_v1: Result<PathBuf, Error>,
    ) -> Self {
        #[cfg(not(test))]
        let _ = captured_at_v1;
        environment_v1.retain(|(key_v1, _)| !key_v1.as_bytes().starts_with(b"GIT_"));
        let selected_v1 = cwd_v1.and_then(|cwd_v1| select_git_v1(&environment_v1, &cwd_v1));
        Self {
            #[cfg(test)]
            captured_at_v1,
            environment_v1,
            selected_v1,
        }
    }

    pub fn executable_v1(&self) -> Result<&PinnedExecutableV1, Error> {
        self.selected_v1
            .as_ref()
            .map(|selected_v1| &selected_v1.executable_v1)
            .map_err(Clone::clone)
    }

    pub fn environment_v1(&self) -> &[(OsString, OsString)] {
        &self.environment_v1
    }

    #[cfg(test)]
    pub(crate) fn selected_path_v1(&self) -> Option<&Path> {
        match self.selected_v1.as_ref() {
            Ok(selected_v1) => Some(selected_v1.selected_path_v1.as_path()),
            Err(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn selected_sha256_v1(&self) -> Option<&[u8; 32]> {
        match self.selected_v1.as_ref() {
            Ok(selected_v1) => Some(&selected_v1.sha256_v1),
            Err(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn captured_at_v1(&self) -> SystemTime {
        self.captured_at_v1
    }
}

fn io_error_v1(operation_v1: &'static str, error_v1: std::io::Error) -> Error {
    Error::DescriptorOperation {
        operation_v1,
        errno_v1: error_v1.raw_os_error().unwrap_or(libc::EIO),
    }
}

fn default_executable_path_v1() -> Result<OsString, Error> {
    // POSIX confstr reads a fixed OS configuration, not mutable environ.
    // Its allocation belongs to process bootstrap, before request admission.
    // SAFETY: a null buffer with size zero queries the required POSIX length.
    let length_v1 = unsafe { libc::confstr(libc::_CS_PATH, std::ptr::null_mut(), 0) };
    if length_v1 == 0 {
        return Err(Error::DescriptorOperation {
            operation_v1: "git_default_path_v1",
            errno_v1: libc::EIO,
        });
    }
    let mut bytes_v1 = vec![0_u8; length_v1];
    // SAFETY: the output slice has the exact size returned by confstr.
    let observed_v1 =
        unsafe { libc::confstr(libc::_CS_PATH, bytes_v1.as_mut_ptr().cast(), bytes_v1.len()) };
    if observed_v1 != length_v1 || bytes_v1.pop() != Some(0) {
        return Err(Error::DescriptorOperation {
            operation_v1: "git_default_path_v1",
            errno_v1: libc::EIO,
        });
    }
    Ok(OsString::from_vec(bytes_v1))
}

fn select_git_v1(
    environment_v1: &[(OsString, OsString)],
    cwd_v1: &Path,
) -> Result<SelectedGitExecutableV1, Error> {
    let default_path_v1;
    let path_v1 = match environment_v1
        .iter()
        .find(|(key_v1, _)| key_v1 == OsStr::new("PATH"))
    {
        Some((_, path_v1)) => path_v1.as_os_str(),
        None => {
            default_path_v1 = default_executable_path_v1()?;
            default_path_v1.as_os_str()
        }
    };
    let mut denied_v1 = false;
    for directory_v1 in std::env::split_paths(path_v1) {
        let directory_v1 = if directory_v1.is_absolute() {
            directory_v1
        } else {
            cwd_v1.join(directory_v1)
        };
        let candidate_v1 = directory_v1.join("git");
        if let Err(error_v1) = rustix::fs::accessat(
            rustix::fs::CWD,
            &candidate_v1,
            rustix::fs::Access::EXEC_OK,
            rustix::fs::AtFlags::EACCESS,
        ) {
            let error_v1 = std::io::Error::from(error_v1);
            match error_v1.raw_os_error() {
                Some(libc::ENOENT | libc::ENOTDIR) => continue,
                Some(libc::EACCES) => {
                    denied_v1 = true;
                    continue;
                }
                _ => return Err(io_error_v1("git_path_execute_access_v1", error_v1)),
            }
        }
        let mut source_v1 = match rustix::fs::open(
            &candidate_v1,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NONBLOCK,
            rustix::fs::Mode::empty(),
        )
        .map(File::from)
        .map_err(std::io::Error::from)
        {
            Ok(source_v1) => source_v1,
            Err(error_v1)
                if matches!(error_v1.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR)) =>
            {
                continue
            }
            Err(error_v1) if error_v1.kind() == std::io::ErrorKind::PermissionDenied => {
                denied_v1 = true;
                continue;
            }
            Err(error_v1) => return Err(io_error_v1("git_path_open_v1", error_v1)),
        };
        let before_v1 = platform::descriptor_identity_v1(source_v1.as_fd())?;
        if !before_v1.regular_file_v1 || before_v1.mode_v1 & 0o111 == 0 {
            denied_v1 = true;
            continue;
        }
        if before_v1.size_v1 == 0 || before_v1.size_v1 > crate::MAX_PINNED_EXECUTABLE_BYTES_V1 {
            return Err(Error::InvalidExecutableDescriptor);
        }
        let mut hash_v1 = Sha256::new();
        let mut bytes_v1 = [0_u8; 64 * 1024];
        let mut total_v1 = 0_u64;
        loop {
            let read_v1 = source_v1
                .read(&mut bytes_v1)
                .map_err(|error_v1| io_error_v1("git_descriptor_hash_v1", error_v1))?;
            if read_v1 == 0 {
                break;
            }
            total_v1 = total_v1
                .checked_add(read_v1 as u64)
                .ok_or(Error::ExecutableIdentityDrift)?;
            if total_v1 > before_v1.size_v1 {
                return Err(Error::ExecutableIdentityDrift);
            }
            hash_v1.update(&bytes_v1[..read_v1]);
        }
        if total_v1 != before_v1.size_v1
            || platform::descriptor_identity_v1(source_v1.as_fd())? != before_v1
        {
            return Err(Error::ExecutableIdentityDrift);
        }
        let sha256_v1: [u8; 32] = hash_v1.finalize().into();
        let mut hex_v1 = [0_u8; 64];
        const HEX_V1: &[u8; 16] = b"0123456789abcdef";
        for (index_v1, byte_v1) in sha256_v1.iter().copied().enumerate() {
            hex_v1[index_v1 * 2] = HEX_V1[usize::from(byte_v1 >> 4)];
            hex_v1[index_v1 * 2 + 1] = HEX_V1[usize::from(byte_v1 & 0x0f)];
        }
        let digest_v1 = std::str::from_utf8(&hex_v1).map_err(|_| Error::InvalidExpectedSha256)?;
        let executable_v1 = PinnedExecutableV1::try_pin_v1(source_v1.as_fd(), digest_v1)?;
        return Ok(SelectedGitExecutableV1 {
            executable_v1,
            #[cfg(test)]
            selected_path_v1: candidate_v1,
            #[cfg(test)]
            sha256_v1,
        });
    }
    Err(Error::DescriptorOperation {
        operation_v1: "git_path_selection_v1",
        errno_v1: if denied_v1 {
            libc::EACCES
        } else {
            libc::ENOENT
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn executable_fixture_v1(directory_v1: &Path, bytes_v1: &[u8], executable_v1: bool) -> PathBuf {
        std::fs::create_dir_all(directory_v1).unwrap();
        let path_v1 = directory_v1.join("git");
        std::fs::write(&path_v1, bytes_v1).unwrap();
        std::fs::set_permissions(
            &path_v1,
            std::fs::Permissions::from_mode(if executable_v1 { 0o755 } else { 0o644 }),
        )
        .unwrap();
        path_v1
    }

    #[test]
    fn bootstrap_freezes_filtered_environment_and_exact_selected_bytes_v1() {
        let root_v1 = tempfile::tempdir().unwrap();
        let denied_v1 = root_v1.path().join("denied");
        let chosen_v1 = root_v1.path().join("chosen");
        let later_v1 = root_v1.path().join("later");
        executable_fixture_v1(&denied_v1, b"denied", false);
        let path_v1 = executable_fixture_v1(&chosen_v1, b"chosen-original", true);
        executable_fixture_v1(&later_v1, b"later", true);
        let path_env_v1 = std::env::join_paths([&denied_v1, &chosen_v1, &later_v1]).unwrap();
        let captured_v1 = SystemTime::now();
        let authority_v1 = GitProcessBootstrapAuthorityV1::from_process_bootstrap_inputs_v1(
            captured_v1,
            vec![
                ("PATH".into(), path_env_v1.clone()),
                ("GIT_DIR".into(), "foreign".into()),
                ("GIT_CONFIG_COUNT".into(), "1".into()),
                ("git_lowercase".into(), "retained".into()),
            ],
            Ok(root_v1.path().to_owned()),
        );
        assert_eq!(authority_v1.captured_at_v1(), captured_v1);
        assert_eq!(authority_v1.selected_path_v1(), Some(path_v1.as_path()));
        let expected_v1: [u8; 32] = Sha256::digest(b"chosen-original").into();
        assert_eq!(authority_v1.selected_sha256_v1(), Some(&expected_v1));
        assert_eq!(
            authority_v1.environment_v1(),
            &[
                (OsString::from("PATH"), path_env_v1),
                (OsString::from("git_lowercase"), OsString::from("retained")),
            ]
        );
        // Subsequent pathname replacement cannot change the selected image.
        std::fs::rename(&path_v1, chosen_v1.join("old-git")).unwrap();
        executable_fixture_v1(&chosen_v1, b"replacement", true);
        authority_v1.executable_v1().unwrap().verify_v1().unwrap();
        assert_eq!(authority_v1.selected_sha256_v1(), Some(&expected_v1));
    }

    #[test]
    fn bootstrap_relative_and_empty_path_components_bind_actual_boot_cwd_v1() {
        let root_v1 = tempfile::tempdir().unwrap();
        let relative_v1 = executable_fixture_v1(&root_v1.path().join("tools"), b"relative", true);
        let cwd_v1 = executable_fixture_v1(root_v1.path(), b"cwd", true);
        for (path_v1, expected_v1) in [("tools:", relative_v1), (":tools", cwd_v1)] {
            let selected_v1 =
                select_git_v1(&[(OsString::from("PATH"), OsString::from(path_v1))], root_v1.path())
                    .unwrap();
            assert_eq!(selected_v1.selected_path_v1, expected_v1);
        }
    }

    #[test]
    fn bootstrap_missing_and_non_executable_git_remain_distinct_typed_failures_v1() {
        let root_v1 = tempfile::tempdir().unwrap();
        let path_v1 = [(OsString::from("PATH"), root_v1.path().as_os_str().to_owned())];
        assert!(matches!(
            select_git_v1(&path_v1, root_v1.path()),
            Err(Error::DescriptorOperation {
                operation_v1: "git_path_selection_v1",
                errno_v1: libc::ENOENT,
            })
        ));
        executable_fixture_v1(root_v1.path(), b"denied", false);
        assert!(matches!(
            select_git_v1(&path_v1, root_v1.path()),
            Err(Error::DescriptorOperation {
                operation_v1: "git_path_selection_v1",
                errno_v1: libc::EACCES,
            })
        ));
    }
}
