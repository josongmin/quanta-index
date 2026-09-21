//! Who may connect to a socket this server binds (QI-BB-014, shared mode).
//!
//! Private mode was the daemon's only mode: socket `0600` in an owner-only
//! (or sticky) directory. Shared mode opens one socket to a group and/or to
//! listed users. Two gates apply to every socket, whatever its mode:
//!
//! 1. the **file mode** the kernel enforces at `connect` — `0600` (private),
//!    `0660` with the named group, or `0666` when users are listed by uid,
//!    since a uid outside the group cannot otherwise reach the socket;
//! 2. the **peer check** this server enforces at `accept`, before a frame is
//!    read: [`admit_peer`] over the credentials the kernel reports for the
//!    connected peer ([`PeerCredentials`]).
//!
//! The peer check is on the peer's *effective* uid and gid, which is what
//! `SO_PEERCRED` and `getpeereid(3)` report; supplementary groups are not
//! reported and are not guessed. A user whose membership in the shared group
//! is only supplementary is not admitted by gid and must be listed by uid.
//!
//! Prefer a group: with `0660` the kernel refuses strangers at `connect`,
//! costing the daemon nothing. A uid allow-list makes the socket
//! world-connectable, so a stranger's refusal happens in the accept loop —
//! still before any frame is read, but at the daemon's expense.
//!
//! A shared socket does not imply a shared state root: the state root keeps
//! its owner-only write policy, and only the socket's own file and the
//! traversal of the directories above it are opened to the peers.
//!
//! # The socket directory and symlinked parents
//!
//! A directory the server creates for a socket gets the policy's mode
//! (`0700` / `0710` / `0711`) and is re-read to prove it, so the umask
//! cannot widen it. A pre-existing directory is judged **by what the path
//! resolves to**, not refused for being reached through a symlink: `/tmp`
//! is a symlink on Darwin and the default socket directory of many
//! deployments, and refusing every symlinked parent would refuse the
//! platform default while proving nothing about the resolved directory. The
//! resolved directory must be a real directory that either belongs to the
//! binding user with no group/other write bit, or carries the sticky bit
//! (a shared temporary directory, where nobody may unlink or rename this
//! user's socket). What a symlink could otherwise buy an attacker — a
//! socket bound into a directory they control — is closed by that check on
//! the target and by the accept-time peer check on every connection, which
//! holds whatever directory the socket sits in.

use std::collections::BTreeSet;

/// Mode of a socket file only its owner may connect to.
pub const PRIVATE_SOCKET_MODE: u32 = 0o600;
/// Mode of a socket file the named group may connect to.
pub const GROUP_SOCKET_MODE: u32 = 0o660;
/// Mode of a socket file any local user may connect to; the peer check is
/// the gate.
pub const WORLD_SOCKET_MODE: u32 = 0o666;
/// Mode of a socket directory this server creates for a private socket.
pub const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
/// Mode of a socket directory this server creates for a group-shared
/// socket: the group may traverse it, nobody else may list or write it.
pub const GROUP_DIRECTORY_MODE: u32 = 0o710;
/// Mode of a socket directory this server creates for a uid-shared socket:
/// anyone may traverse it, nobody else may list or write it.
pub const WORLD_DIRECTORY_MODE: u32 = 0o711;

/// The peers a bound socket admits besides its owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SharedSocketAccess {
    /// The group whose effective gid is admitted, and which the socket file
    /// is assigned to; the binding process must be a member.
    group: Option<u32>,
    /// Users admitted by effective uid, whatever their group.
    allowed_uids: BTreeSet<u32>,
}

impl SharedSocketAccess {
    /// Access for `group` and/or `allowed_uids`.
    ///
    /// Neither is required: an access that names nothing admits the owner
    /// only, which [`SocketAccessPolicy::Private`] says more directly. The
    /// daemon's config refuses that spelling; the type keeps it
    /// representable so the admission table can be proven over it.
    #[must_use]
    pub const fn new(group: Option<u32>, allowed_uids: BTreeSet<u32>) -> Self {
        Self {
            group,
            allowed_uids,
        }
    }

    #[must_use]
    pub const fn group(&self) -> Option<u32> {
        self.group
    }

    #[must_use]
    pub const fn allowed_uids(&self) -> &BTreeSet<u32> {
        &self.allowed_uids
    }

    /// Whether this access admits anyone besides the owner.
    #[must_use]
    pub fn admits_others(&self) -> bool {
        self.group.is_some() || !self.allowed_uids.is_empty()
    }
}

/// How one bound socket is exposed and who it admits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SocketAccessPolicy {
    /// Socket `0600` in an owner-only or sticky directory; only this
    /// process's effective user is admitted.
    Private,
    /// The socket is opened to the named group and/or the listed users;
    /// they and the owner are admitted.
    Shared(SharedSocketAccess),
}

impl SocketAccessPolicy {
    /// The file mode the bound socket is set to.
    #[must_use]
    pub fn socket_mode(&self) -> u32 {
        match self {
            Self::Private => PRIVATE_SOCKET_MODE,
            Self::Shared(access) => {
                if !access.allowed_uids.is_empty() {
                    WORLD_SOCKET_MODE
                } else if access.group.is_some() {
                    GROUP_SOCKET_MODE
                } else {
                    PRIVATE_SOCKET_MODE
                }
            }
        }
    }

    /// The mode a socket directory created for this policy gets.
    #[must_use]
    pub fn directory_mode(&self) -> u32 {
        match self {
            Self::Private => PRIVATE_DIRECTORY_MODE,
            Self::Shared(access) => {
                if !access.allowed_uids.is_empty() {
                    WORLD_DIRECTORY_MODE
                } else if access.group.is_some() {
                    GROUP_DIRECTORY_MODE
                } else {
                    PRIVATE_DIRECTORY_MODE
                }
            }
        }
    }

    /// The group the socket file (and a directory this server creates for
    /// it) is assigned to, when the policy names one.
    #[must_use]
    pub const fn group(&self) -> Option<u32> {
        match self {
            Self::Private => None,
            Self::Shared(access) => access.group,
        }
    }

    /// Whether peers other than the owner are admitted at all.
    #[must_use]
    pub fn admits_others(&self) -> bool {
        match self {
            Self::Private => false,
            Self::Shared(access) => access.admits_others(),
        }
    }

    /// The policy a directory holding sockets of every policy in
    /// `policies` is created and verified under.
    ///
    /// It is the one whose directory mode is widest (uid-shared `0711`
    /// over group-shared `0710` over private `0700`), so every socket's
    /// admitted peers can traverse it and nobody else can list or write
    /// it. Private when `policies` is empty.
    #[must_use]
    pub fn widest<'a>(policies: impl IntoIterator<Item = &'a Self>) -> Self {
        policies
            .into_iter()
            .fold(Self::Private, |widest, candidate| {
                if candidate.directory_mode() > widest.directory_mode() {
                    candidate.clone()
                } else {
                    widest
                }
            })
    }
}

/// The effective identity of the process at the other end of a connected
/// `AF_UNIX` stream, as the kernel reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerCredentials {
    /// The peer's effective uid.
    pub uid: u32,
    /// The peer's effective gid. Supplementary groups are not reported.
    pub gid: u32,
    /// The peer's process id where the platform reports one (`SO_PEERCRED`
    /// does, `getpeereid` does not). Admission never reads it; a refusal
    /// carries it so the refused peer is named precisely.
    pub pid: Option<u32>,
}

/// Why a connected peer was closed before a frame was read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerRefusal {
    /// The peer is not the owner, not a listed uid, and not of the shared
    /// group.
    NotAdmitted { peer: PeerCredentials },
}

impl core::fmt::Display for PeerRefusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotAdmitted { peer } => write!(
                f,
                "peer uid {} gid {} is not admitted by the socket's access policy",
                peer.uid, peer.gid
            ),
        }
    }
}

impl std::error::Error for PeerRefusal {}

/// Decide whether `peer` may use a socket bound under `policy` by a process
/// whose effective uid is `self_euid`.
///
/// The owner is always admitted. A shared policy also admits a peer whose
/// effective uid is listed, or whose effective gid is the shared group.
/// Nothing else is: root is not special-cased, and a peer's supplementary
/// groups are neither reported nor inferred.
pub fn admit_peer(
    policy: &SocketAccessPolicy,
    peer: PeerCredentials,
    self_euid: u32,
) -> Result<(), PeerRefusal> {
    if peer.uid == self_euid {
        return Ok(());
    }
    match policy {
        SocketAccessPolicy::Private => Err(PeerRefusal::NotAdmitted { peer }),
        SocketAccessPolicy::Shared(access) => {
            if let Some(group) = access.group
                && group == peer.gid
            {
                return Ok(());
            }
            if access.allowed_uids.contains(&peer.uid) {
                return Ok(());
            }
            Err(PeerRefusal::NotAdmitted { peer })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GROUP_DIRECTORY_MODE, GROUP_SOCKET_MODE, PRIVATE_DIRECTORY_MODE, PRIVATE_SOCKET_MODE,
        PeerCredentials, PeerRefusal, SharedSocketAccess, SocketAccessPolicy, WORLD_DIRECTORY_MODE,
        WORLD_SOCKET_MODE, admit_peer,
    };
    use std::collections::BTreeSet;

    const SELF_UID: u32 = 1000;
    const OTHER_UID: u32 = 1001;
    const LISTED_UID: u32 = 1002;
    const SHARED_GID: u32 = 2000;
    const OTHER_GID: u32 = 2001;

    const fn peer(uid: u32, gid: u32) -> PeerCredentials {
        PeerCredentials {
            uid,
            gid,
            pid: None,
        }
    }

    fn shared(group: Option<u32>, uids: &[u32]) -> SocketAccessPolicy {
        SocketAccessPolicy::Shared(SharedSocketAccess::new(
            group,
            uids.iter().copied().collect::<BTreeSet<u32>>(),
        ))
    }

    /// The admission table, one row per (policy, peer) pair. The oracle is
    /// the rule as written: owner always; shared adds listed uid and shared
    /// effective gid; nothing else, root included.
    #[test]
    fn the_admission_table_holds() {
        let private = SocketAccessPolicy::Private;
        let group_only = shared(Some(SHARED_GID), &[]);
        let uids_only = shared(None, &[LISTED_UID]);
        let both = shared(Some(SHARED_GID), &[LISTED_UID]);
        let nobody = shared(None, &[]);
        let rows: [(&str, &SocketAccessPolicy, PeerCredentials, bool); 16] = [
            ("private/self", &private, peer(SELF_UID, OTHER_GID), true),
            ("private/other", &private, peer(OTHER_UID, OTHER_GID), false),
            ("private/root", &private, peer(0, 0), false),
            ("private/shared gid", &private, peer(OTHER_UID, SHARED_GID), false),
            ("group/self", &group_only, peer(SELF_UID, OTHER_GID), true),
            ("group/member", &group_only, peer(OTHER_UID, SHARED_GID), true),
            ("group/stranger", &group_only, peer(OTHER_UID, OTHER_GID), false),
            ("group/listed elsewhere", &group_only, peer(LISTED_UID, OTHER_GID), false),
            ("uids/self", &uids_only, peer(SELF_UID, OTHER_GID), true),
            ("uids/listed", &uids_only, peer(LISTED_UID, OTHER_GID), true),
            ("uids/stranger", &uids_only, peer(OTHER_UID, SHARED_GID), false),
            ("both/member", &both, peer(OTHER_UID, SHARED_GID), true),
            ("both/listed", &both, peer(LISTED_UID, OTHER_GID), true),
            ("both/stranger", &both, peer(OTHER_UID, OTHER_GID), false),
            ("nobody/self", &nobody, peer(SELF_UID, SHARED_GID), true),
            ("nobody/other", &nobody, peer(OTHER_UID, SHARED_GID), false),
        ];
        for (name, policy, credentials, admitted) in rows {
            let outcome = admit_peer(policy, credentials, SELF_UID);
            match (outcome, admitted) {
                (Ok(()), true) => {}
                (Err(PeerRefusal::NotAdmitted { peer }), false) => {
                    assert_eq!(peer, credentials, "{name}: the refusal names the peer");
                }
                (outcome, expected) => {
                    panic!("{name}: expected admitted={expected}, got {outcome:?}");
                }
            }
        }
    }

    /// Root is admitted only when the daemon itself runs as root.
    #[test]
    fn root_is_the_owner_only_when_the_daemon_is_root() {
        assert!(admit_peer(&SocketAccessPolicy::Private, peer(0, 0), 0).is_ok());
        assert!(admit_peer(&SocketAccessPolicy::Private, peer(0, 0), SELF_UID).is_err());
    }

    /// The file modes follow the widest peer the policy admits.
    #[test]
    fn modes_follow_the_widest_admitted_peer() {
        let cases: [(SocketAccessPolicy, u32, u32); 5] = [
            (SocketAccessPolicy::Private, PRIVATE_SOCKET_MODE, PRIVATE_DIRECTORY_MODE),
            (shared(Some(SHARED_GID), &[]), GROUP_SOCKET_MODE, GROUP_DIRECTORY_MODE),
            (shared(None, &[LISTED_UID]), WORLD_SOCKET_MODE, WORLD_DIRECTORY_MODE),
            (shared(Some(SHARED_GID), &[LISTED_UID]), WORLD_SOCKET_MODE, WORLD_DIRECTORY_MODE),
            (shared(None, &[]), PRIVATE_SOCKET_MODE, PRIVATE_DIRECTORY_MODE),
        ];
        for (policy, socket_mode, directory_mode) in cases {
            assert_eq!(policy.socket_mode(), socket_mode, "{policy:?}");
            assert_eq!(policy.directory_mode(), directory_mode, "{policy:?}");
            assert_eq!(policy.admits_others(), socket_mode != PRIVATE_SOCKET_MODE, "{policy:?}");
        }
        assert_eq!(shared(Some(SHARED_GID), &[]).group(), Some(SHARED_GID));
        assert_eq!(SocketAccessPolicy::Private.group(), None);
    }

    /// The directory policy for a set of sockets: the widest of them.
    ///
    /// A uid-shared socket beside private ones makes the directory
    /// world-traversable, a group-shared one beside private ones makes it
    /// the group's, and private sockets alone keep it private.
    #[test]
    fn the_widest_policy_decides_the_shared_directory() {
        let private = SocketAccessPolicy::Private;
        let group = shared(Some(SHARED_GID), &[]);
        let listed = shared(None, &[LISTED_UID]);
        assert_eq!(SocketAccessPolicy::widest([]), private);
        assert_eq!(SocketAccessPolicy::widest([&private, &private]), private);
        assert_eq!(SocketAccessPolicy::widest([&group, &private, &private]), group);
        assert_eq!(SocketAccessPolicy::widest([&private, &group, &listed]), listed);
        assert_eq!(
            SocketAccessPolicy::widest([&private, &group, &listed]).directory_mode(),
            WORLD_DIRECTORY_MODE
        );
    }
}
