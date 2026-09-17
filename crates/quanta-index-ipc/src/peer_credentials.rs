//! Reading a connected peer's credentials from the kernel (QI-BB-014).
//!
//! Linux and Android answer `getsockopt(SOL_SOCKET, SO_PEERCRED)` with the
//! peer's pid, effective uid and effective gid as of its `connect`; `rustix`
//! wraps the call. The BSDs, macOS included, answer `getpeereid(3)` with the
//! effective uid and gid only — no pid, and on neither family the peer's
//! supplementary groups. `rustix` has no `getpeereid` wrapper; `nix` (MIT)
//! supplies the safe one, so this crate stays free of `unsafe`. A platform
//! that offers neither call does not build: admission over guessed
//! credentials would be a silent hole, not a fallback.
//!
//! The server reads credentials through the [`PeerCredentialsSource`] port
//! so a test can script the peer it observes; [`KernelPeerCredentials`] is
//! the only production source.

use std::os::unix::net::UnixStream;

use crate::socket_access::PeerCredentials;

/// Where the server learns who is at the other end of an accepted stream.
pub trait PeerCredentialsSource: Send + Sync {
    /// The credentials of the process connected through `stream`.
    ///
    /// An error means the kernel did not report them; the server treats
    /// that as a refusal, never as an admission.
    fn peer_credentials(&self, stream: &UnixStream) -> std::io::Result<PeerCredentials>;
}

/// The kernel's own report, per platform.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KernelPeerCredentials;

impl PeerCredentialsSource for KernelPeerCredentials {
    fn peer_credentials(&self, stream: &UnixStream) -> std::io::Result<PeerCredentials> {
        read_peer_credentials(stream)
    }
}

/// `SO_PEERCRED`: pid, effective uid and effective gid at `connect`.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn read_peer_credentials(stream: &UnixStream) -> std::io::Result<PeerCredentials> {
    let credentials = rustix::net::sockopt::socket_peercred(stream)?;
    let pid = u32::try_from(credentials.pid.as_raw_nonzero().get()).map_err(|error| {
        std::io::Error::other(format!(
            "peer credentials reported a pid outside the unsigned range: {error}"
        ))
    })?;
    Ok(PeerCredentials {
        uid: credentials.uid.as_raw(),
        gid: credentials.gid.as_raw(),
        pid: Some(pid),
    })
}

/// `getpeereid(3)`: effective uid and effective gid at `connect`; no pid.
#[cfg(any(
    target_vendor = "apple",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
))]
fn read_peer_credentials(stream: &UnixStream) -> std::io::Result<PeerCredentials> {
    let (uid, gid) = nix::unistd::getpeereid(stream)?;
    Ok(PeerCredentials {
        uid: uid.as_raw(),
        gid: gid.as_raw(),
        pid: None,
    })
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
)))]
compile_error!(
    "quanta-index-ipc: this platform reports no AF_UNIX peer credentials (neither SO_PEERCRED nor getpeereid); socket admission cannot be enforced here, so the crate does not build"
);

#[cfg(test)]
mod tests {
    use super::{KernelPeerCredentials, PeerCredentialsSource};
    use std::os::unix::net::UnixStream;

    /// The kernel names this very process as its own peer over a socket
    /// pair: the oracle is `geteuid`/`getegid`, read independently of the
    /// socket.
    #[test]
    fn the_kernel_reports_this_process_as_its_own_peer() -> Result<(), String> {
        let (ours, theirs) = UnixStream::pair().map_err(|error| error.to_string())?;
        let observed = KernelPeerCredentials
            .peer_credentials(&ours)
            .map_err(|error| error.to_string())?;
        drop(theirs);
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let process = Some(std::process::id());
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let process = None;
        let expected = super::PeerCredentials {
            uid: rustix::process::geteuid().as_raw(),
            gid: rustix::process::getegid().as_raw(),
            pid: process,
        };
        if observed != expected {
            return Err(format!("expected {expected:?}, observed {observed:?}"));
        }
        Ok(())
    }
}
