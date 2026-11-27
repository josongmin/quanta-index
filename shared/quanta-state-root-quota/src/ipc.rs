//! One-request, one-FD Unix protocol for a privileged quota reader.

use crate::{XfsProjectQuota, read_xfs_project_quota};
use std::ffi::CString;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

const RESPONSE_LEN: usize = 56;
const TIMEOUT: Duration = Duration::from_secs(5);
const MAGIC: &[u8; 4] = b"QXFS";

unsafe extern "C" {
    fn quanta_socket_peer_uid(socket_fd: libc::c_int, out: *mut u32) -> libc::c_int;
    fn quanta_connect_unix_socket(
        path: *const libc::c_char,
        timeout_ms: u32,
        out: *mut libc::c_int,
    ) -> libc::c_int;
    fn quanta_send_root_fd(socket_fd: libc::c_int, root_fd: libc::c_int) -> libc::c_int;
    fn quanta_receive_root_fd(socket_fd: libc::c_int, out: *mut libc::c_int) -> libc::c_int;
    fn quanta_take_systemd_listener(out: *mut libc::c_int) -> libc::c_int;
    fn quanta_require_privileged_xfs_reader() -> libc::c_int;
}

fn errno_result(error: libc::c_int) -> io::Result<()> {
    if error == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(error))
    }
}

fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut uid = 0;
    // SAFETY: `uid` is a writable u32 and stream owns a live socket FD.
    errno_result(unsafe { quanta_socket_peer_uid(stream.as_raw_fd(), &mut uid) })?;
    Ok(uid)
}

fn receive_fd(stream: &UnixStream) -> io::Result<OwnedFd> {
    let mut fd = -1;
    // SAFETY: `fd` is writable; C accepts exactly one SCM_RIGHTS FD, marks it
    // CLOEXEC, and closes all received descriptors on malformed input.
    errno_result(unsafe { quanta_receive_root_fd(stream.as_raw_fd(), &mut fd) })?;
    if fd < 0 {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    // SAFETY: successful C return transfers this one newly received FD.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn encode_result(value: io::Result<XfsProjectQuota>) -> [u8; RESPONSE_LEN] {
    let mut out = [0_u8; RESPONSE_LEN];
    out[..4].copy_from_slice(MAGIC);
    out[4..6].copy_from_slice(&2_u16.to_le_bytes());
    match value {
        Ok(quota) => {
            out[12..16].copy_from_slice(&quota.project_id.to_le_bytes());
            let flags = u32::from(quota.inherits_project)
                | (u32::from(quota.accounting_enabled) << 1)
                | (u32::from(quota.enforcement_enabled) << 2);
            out[16..20].copy_from_slice(&flags.to_le_bytes());
            out[24..32].copy_from_slice(&quota.block_hard_limit_bytes.to_le_bytes());
            out[32..40].copy_from_slice(&quota.inode_hard_limit.to_le_bytes());
            out[40..48].copy_from_slice(&quota.used_bytes.to_le_bytes());
            out[48..56].copy_from_slice(&quota.used_inodes.to_le_bytes());
        }
        Err(error) => {
            let code = error.raw_os_error().unwrap_or(libc::EIO) as u32;
            out[8..12].copy_from_slice(&code.to_le_bytes());
        }
    }
    out
}

fn decode_result(raw: [u8; RESPONSE_LEN]) -> io::Result<XfsProjectQuota> {
    if &raw[..4] != MAGIC
        || raw[4..6] != 2_u16.to_le_bytes()
        || raw[6..8] != [0; 2]
        || raw[20..24] != [0; 4]
    {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let status = u32::from_le_bytes(raw[8..12].try_into().expect("fixed response"));
    if status != 0 {
        if raw[12..20] != [0; 8] || raw[24..56] != [0; 32] || status > 4095 {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let project_id = u32::from_le_bytes(raw[12..16].try_into().expect("fixed response"));
    let flags = u32::from_le_bytes(raw[16..20].try_into().expect("fixed response"));
    let block_hard_limit_bytes =
        u64::from_le_bytes(raw[24..32].try_into().expect("fixed response"));
    let inode_hard_limit = u64::from_le_bytes(raw[32..40].try_into().expect("fixed response"));
    let used_bytes = u64::from_le_bytes(raw[40..48].try_into().expect("fixed response"));
    let used_inodes = u64::from_le_bytes(raw[48..56].try_into().expect("fixed response"));
    if project_id == 0
        || flags != 7
        || block_hard_limit_bytes == 0
        || block_hard_limit_bytes % 512 != 0
        || inode_hard_limit == 0
        || used_bytes % 512 != 0
    {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    Ok(XfsProjectQuota {
        project_id,
        inherits_project: true,
        accounting_enabled: true,
        enforcement_enabled: true,
        block_hard_limit_bytes,
        inode_hard_limit,
        used_bytes,
        used_inodes,
    })
}

fn one_connection(mut stream: UnixStream, allowed_client_uid: u32) -> io::Result<()> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    if peer_uid(&stream)? != allowed_client_uid {
        return Err(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    let root = receive_fd(&stream)?;
    stream.write_all(&encode_result(read_xfs_project_quota(&root)))
}

/// Serve quota queries on a caller-owned Unix listener. The caller owns
/// socket activation, path permissions, and lifecycle. Invalid connections
/// are dropped; the server continues accepting the next request.
pub fn serve_xfs_project_quota(listener: UnixListener, allowed_client_uid: u32) -> io::Result<()> {
    if allowed_client_uid == 0 {
        return Err(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    // SAFETY: C reads current Linux credentials and effective capabilities.
    errno_result(unsafe { quanta_require_privileged_xfs_reader() })?;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = one_connection(stream, allowed_client_uid);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

/// Consume fd 3 from systemd socket activation after validating the root-owned
/// pathname socket and its root-owned, non-writable parent directory. The
/// socket must be mode 0660 or stricter and group-writable for the client.
pub fn take_systemd_listener() -> io::Result<UnixListener> {
    let mut fd = -1;
    // SAFETY: C validates LISTEN_PID, LISTEN_FDS, fd 3's listener type and
    // filesystem boundary, then transfers the already-owned process FD.
    errno_result(unsafe { quanta_take_systemd_listener(&mut fd) })?;
    if fd != 3 {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    // SAFETY: fd 3 is inherited exclusively for this listener. This is called
    // once at process startup and ownership moves to UnixListener.
    Ok(unsafe { UnixListener::from_raw_fd(fd) })
}

/// Query a root's installed XFS quota through a root-owned verifier socket.
/// The root FD, never its pathname, is transferred to the verifier. A peer
/// whose kernel-reported uid is not 0 is refused before any FD transfer.
pub fn read_xfs_project_quota_via(socket: &Path, root: &impl AsFd) -> io::Result<XfsProjectQuota> {
    read_xfs_project_quota_via_before(socket, root, Instant::now() + TIMEOUT)
}

/// Limit the entire verifier exchange to the caller's deadline and the local
/// five-second cap, including progress across partial reads.
pub fn read_xfs_project_quota_via_before(
    socket: &Path,
    root: &impl AsFd,
    deadline: Instant,
) -> io::Result<XfsProjectQuota> {
    let deadline = deadline.min(Instant::now() + TIMEOUT);
    let path = CString::new(socket.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let connect_budget = remaining(deadline)?;
    let connect_ms = u32::try_from(connect_budget.as_nanos().div_ceil(1_000_000))
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut fd = -1;
    // SAFETY: C reads the NUL-terminated path, returns a newly owned CLOEXEC
    // socket FD, and bounds nonblocking connect to the supplied interval.
    errno_result(unsafe { quanta_connect_unix_socket(path.as_ptr(), connect_ms, &mut fd) })?;
    if fd < 0 {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    // SAFETY: successful C return transfers ownership of one socket FD.
    let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
    if peer_uid(&stream)? != 0 {
        return Err(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    loop {
        stream.set_write_timeout(Some(remaining(deadline)?))?;
        // SAFETY: the root borrowed FD and socket remain open for sendmsg; C
        // transmits exactly one descriptor and never takes ownership of it.
        let status = errno_result(unsafe {
            quanta_send_root_fd(stream.as_raw_fd(), root.as_fd().as_raw_fd())
        });
        match status {
            Ok(()) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(normalize_socket_timeout(error)),
        }
    }
    read_response(&mut stream, deadline)
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    let duration = deadline.saturating_duration_since(Instant::now());
    if duration.is_zero() {
        return Err(io::Error::from(io::ErrorKind::TimedOut));
    }
    Ok(duration)
}

fn normalize_socket_timeout(error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::WouldBlock {
        io::Error::from(io::ErrorKind::TimedOut)
    } else {
        error
    }
}

fn read_response(stream: &mut UnixStream, deadline: Instant) -> io::Result<XfsProjectQuota> {
    let mut response = [0_u8; RESPONSE_LEN];
    let mut pending = response.as_mut_slice();
    while !pending.is_empty() {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        match stream.read(pending) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(bytes) => pending = &mut pending[bytes..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(normalize_socket_timeout(error)),
        }
    }
    let _remaining = remaining(deadline)?;
    decode_result(response)
}

#[cfg(test)]
mod tests {
    use super::{
        decode_result, encode_result, one_connection, read_response, read_xfs_project_quota_via,
        read_xfs_project_quota_via_before,
    };
    use crate::XfsProjectQuota;
    use std::fs::{File, remove_file};
    use std::io::{self, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::thread;
    use std::time::{Duration, Instant};

    const QUOTA_RESPONSE: [u8; 56] = [
        b'Q', b'X', b'F', b'S', 2, 0, 0, 0, // Header.
        0, 0, 0, 0, 42, 0, 0, 0, // Status and project ID.
        7, 0, 0, 0, 0, 0, 0, 0, // Feature flags and reserved bytes.
        0, 0, 8, 0, 0, 0, 0, 0, // 512 KiB hard limit.
        100, 0, 0, 0, 0, 0, 0, 0, // 100 inode hard limit.
        0, 0, 1, 0, 0, 0, 0, 0, // 64 KiB in use.
        4, 0, 0, 0, 0, 0, 0, 0, // Four inodes in use.
    ];

    #[test]
    fn expired_quota_read_refuses_without_consuming_an_available_response() -> io::Result<()> {
        let (mut receiver, mut sender) = UnixStream::pair()?;
        sender.write_all(&QUOTA_RESPONSE)?;
        let error = read_response(&mut receiver, Instant::now()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let mut available = [0; super::RESPONSE_LEN];
        receiver.read_exact(&mut available)?;
        assert_eq!(available, QUOTA_RESPONSE);
        Ok(())
    }

    #[test]
    fn partial_verifier_reply_does_not_extend_the_response_deadline() -> io::Result<()> {
        let (mut receiver, mut sender) = UnixStream::pair()?;
        sender.write_all(&QUOTA_RESPONSE[..1])?;
        let deadline = Instant::now() + Duration::from_millis(40);
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(120));
            sender.write_all(&QUOTA_RESPONSE[1..])
        });
        let result = read_response(&mut receiver, deadline);
        writer.join().expect("writer thread panicked")?;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        Ok(())
    }

    #[test]
    fn verifier_close_after_a_partial_frame_is_an_error() -> io::Result<()> {
        let (mut receiver, mut sender) = UnixStream::pair()?;
        sender.write_all(&QUOTA_RESPONSE[..QUOTA_RESPONSE.len() - 1])?;
        drop(sender);
        let error =
            read_response(&mut receiver, Instant::now() + Duration::from_secs(2)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        Ok(())
    }

    #[test]
    fn expired_caller_deadline_takes_precedence_over_socket_lookup() -> io::Result<()> {
        let root = File::open("/")?;
        let error =
            read_xfs_project_quota_via_before(Path::new("/"), &root, Instant::now()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        Ok(())
    }

    #[test]
    fn verifier_frame_checks_each_fixed_field_and_error_payload() -> io::Result<()> {
        let expected = XfsProjectQuota {
            project_id: 42,
            inherits_project: true,
            accounting_enabled: true,
            enforcement_enabled: true,
            block_hard_limit_bytes: 524_288,
            inode_hard_limit: 100,
            used_bytes: 65_536,
            used_inodes: 4,
        };
        assert_eq!(decode_result(QUOTA_RESPONSE)?, expected);
        for (field, offset, byte) in [
            ("magic", 0, b'X'),
            ("version", 4, 1),
            ("header reserved", 6, 1),
            ("error with success payload", 8, 1),
            ("zero project", 12, 0),
            ("disabled accounting", 16, 5),
            ("unknown flag", 16, 15),
            ("body reserved", 20, 1),
            ("unaligned block limit", 24, 1),
            ("zero block limit", 26, 0),
            ("zero inode limit", 32, 0),
            ("unaligned bytes in use", 40, 1),
        ] {
            let mut frame = QUOTA_RESPONSE;
            frame[offset] = byte;
            assert_eq!(
                decode_result(frame).unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "invalid {field} accepted",
            );
        }
        let mut error_frame = [0; super::RESPONSE_LEN];
        error_frame[..8].copy_from_slice(&QUOTA_RESPONSE[..8]);
        error_frame[8..12].copy_from_slice(&libc::EDQUOT.to_le_bytes());
        assert_eq!(
            decode_result(error_frame).unwrap_err().raw_os_error(),
            Some(libc::EDQUOT),
        );
        error_frame[8..12].copy_from_slice(&4096_u32.to_le_bytes());
        assert_eq!(
            decode_result(error_frame).unwrap_err().kind(),
            io::ErrorKind::InvalidData,
        );
        Ok(())
    }

    #[test]
    fn response_round_trip_and_tamper_refusal() {
        let quota = XfsProjectQuota {
            project_id: 42,
            inherits_project: true,
            accounting_enabled: true,
            enforcement_enabled: true,
            block_hard_limit_bytes: 512 * 1024,
            inode_hard_limit: 100,
            used_bytes: 512 * 128,
            used_inodes: 4,
        };
        let response = encode_result(Ok(quota));
        assert_eq!(decode_result(response).unwrap(), quota);
        let mut changed = response;
        changed[16] = 0;
        assert!(decode_result(changed).is_err());
        assert_eq!(
            decode_result(encode_result(Err(std::io::Error::from_raw_os_error(1))))
                .unwrap_err()
                .raw_os_error(),
            Some(1)
        );
    }

    #[test]
    fn root_peer_protocol_transfers_one_fd_and_returns_kernel_error() {
        // The CI container runs this as root. The production listener rejects
        // uid 0 clients; this tests the wire protocol below that policy gate.
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let effective_uid = status
            .lines()
            .find_map(|line| line.strip_prefix("Uid:"))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|uid| uid.parse::<u32>().ok());
        if effective_uid != Some(0) {
            return;
        }
        let path = std::env::temp_dir().join(format!(
            "quanta-xfs-test-{}-{}.sock",
            std::process::id(),
            thread::current().name().unwrap_or("protocol")
        ));
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            one_connection(stream, 0).unwrap();
        });
        let root = File::open(std::env::temp_dir()).unwrap();
        let error = read_xfs_project_quota_via(&path, &root).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EOPNOTSUPP));
        server.join().unwrap();
        remove_file(path).unwrap();
    }

    #[test]
    fn mismatched_peer_uid_is_refused() {
        let (server, _client) = UnixStream::pair().unwrap();
        let error = one_connection(server, u32::MAX).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn root_writer_and_root_client_uid_are_refused() {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let effective_uid = status
            .lines()
            .find_map(|line| line.strip_prefix("Uid:"))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|uid| uid.parse::<u32>().ok());
        if effective_uid == Some(0) {
            assert_eq!(
                crate::require_unprivileged_xfs_writer()
                    .unwrap_err()
                    .raw_os_error(),
                Some(libc::EPERM)
            );
        }
        let listener = UnixListener::bind(
            std::env::temp_dir().join(format!("quanta-xfs-uid-test-{}.sock", std::process::id())),
        )
        .unwrap();
        assert_eq!(
            super::serve_xfs_project_quota(listener, 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
