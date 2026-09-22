//! QI-BB-014 shared mode: what a bound socket looks like on disk and who
//! its accept loop lets past, over the real control envelopes.
//!
//! The oracles are independent of the server: file modes and group ids
//! are read back with `stat`, admission is read from a stub dispatcher's
//! own count of the requests that reached it, and the server's counters
//! are checked the way a scrape reads them (QI-BB-015).
//!
//! A second uid is not available to a test, so a refused peer is produced
//! by scripting the credentials the accept loop observes through the
//! `PeerCredentialsSource` port. What that leaves unproven — that the
//! kernel refuses a stranger's `connect` on a `0660` socket, and that a
//! real stranger's `SO_PEERCRED`/`getpeereid` report is what the scripted
//! one models — is the kernel's contract, not this crate's.
//!
//! Shared sockets are bound under `/tmp`: the process temp dir is not
//! traversable by other users on macOS, and a shared policy whose path the
//! peers cannot walk is refused at bind (which is also proven here).

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use quanta_index_contract::ipc::{
    CurrentGenerationRequest, GenerationSnapshot, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
    SearchPlaneControlIpcResponseEnvelope, SearchPlaneTrackKind,
};
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_ipc::{
    ClientIoPolicy, GROUP_DIRECTORY_MODE, GROUP_SOCKET_MODE, IpcDispatcher, IpcError,
    IpcServerCounters, PRIVATE_SOCKET_MODE, PeerCredentials, PeerCredentialsSource,
    RequestBudgetV1, ServerAdmissionPolicy, SharedSocketAccess, SocketAccessPolicy, UdsServer,
    WORLD_SOCKET_MODE, send_request,
};

type TestResult = Result<(), Box<dyn Error>>;

const ACCEPT_IDLE: Duration = Duration::from_millis(1);
const CLIENT_BUDGET: Duration = Duration::from_secs(10);

/// Answers every `CurrentGeneration` and counts how many reached it.
struct CountingStub {
    served: Arc<AtomicU64>,
}

impl IpcDispatcher<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse> for CountingStub {
    fn dispatch(
        &self,
        _context: &quanta_index_ipc::DispatchContextV1,
        request: SearchPlaneControlIpcRequest,
        _budget: &RequestBudgetV1,
    ) -> SearchPlaneControlIpcResponse {
        let _served = self.served.fetch_add(1, Ordering::SeqCst);
        match request {
            SearchPlaneControlIpcRequest::CurrentGeneration(request) => {
                SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(GenerationSnapshot {
                    repo_id: request.repo_id,
                    revision_id: request.revision_id,
                    track: request.track,
                    manifest_generation: ManifestGeneration::new(1),
                    manifest_digest: "digest".to_string(),
                })
            }
            other @ (SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(_)
            | SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(_)
            | SearchPlaneControlIpcRequest::RepoMapActivate(_)
            | SearchPlaneControlIpcRequest::GenerationStatus(_)
            | SearchPlaneControlIpcRequest::MetricsSnapshot(_)
            | SearchPlaneControlIpcRequest::QuarantineInventory(_)
            | SearchPlaneControlIpcRequest::QuarantineDiscard(_)
            | SearchPlaneControlIpcRequest::ProcessReadiness(_)) => {
                SearchPlaneControlIpcResponse::Error(quanta_index_contract::SearchPlaneIpcError {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::Internal,
                    message: format!("only CurrentGeneration is stubbed, got {other:?}"),
                    repair: None,
                })
            }
        }
    }
}

/// What the scripted source reports for the next peers.
#[derive(Clone, Copy, Debug)]
enum ScriptedPeer {
    Reports(PeerCredentials),
    Unreadable,
}

/// A peer-credential source that answers from a script instead of the
/// kernel, so a test running as one uid can present any peer.
struct ScriptedSource {
    next: Mutex<ScriptedPeer>,
}

impl ScriptedSource {
    fn set(&self, peer: ScriptedPeer) -> TestResult {
        *self
            .next
            .lock()
            .map_err(|poisoned| format!("script poisoned: {poisoned}"))? = peer;
        Ok(())
    }
}

impl PeerCredentialsSource for ScriptedSource {
    fn peer_credentials(&self, _stream: &UnixStream) -> std::io::Result<PeerCredentials> {
        let guard = self
            .next
            .lock()
            .map_err(|poisoned| std::io::Error::other(poisoned.to_string()))?;
        match *guard {
            ScriptedPeer::Reports(peer) => Ok(peer),
            ScriptedPeer::Unreadable => Err(std::io::Error::other(
                "scripted: the kernel did not report the peer",
            )),
        }
    }
}

struct Served {
    uds: Arc<UdsServer>,
    join: Option<thread::JoinHandle<Result<(), IpcError>>>,
    /// Requests that reached the dispatcher, counted by the dispatcher.
    reached: Arc<AtomicU64>,
    counters: Arc<IpcServerCounters>,
}

impl Served {
    fn start(
        socket: &Path,
        access: &SocketAccessPolicy,
        source: Arc<dyn PeerCredentialsSource>,
    ) -> Result<Self, Box<dyn Error>> {
        let counters = Arc::new(IpcServerCounters::for_plane("test"));
        let uds = Arc::new(UdsServer::bind_with_peer_source(
            socket,
            ServerAdmissionPolicy::DEFAULT,
            access.clone(),
            access,
            Arc::clone(&counters),
            source,
        )?);
        if uds.socket_access_policy() != access {
            return Err(format!(
                "the bound server carries the policy it enforces: {:?} vs {access:?}",
                uds.socket_access_policy()
            )
            .into());
        }
        let reached = Arc::new(AtomicU64::new(0));
        let dispatcher = Arc::new(CountingStub {
            served: Arc::clone(&reached),
        });
        let join = {
            let uds = Arc::clone(&uds);
            thread::spawn(move || {
                uds.run::<SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponseEnvelope, SearchPlaneControlIpcResponse, CountingStub>(
                    &dispatcher,
                    quanta_index_ipc::IpcPlane::Control,
                    ACCEPT_IDLE,
                )
            })
        };
        Ok(Self {
            uds,
            join: Some(join),
            reached,
            counters,
        })
    }

    fn served(&self) -> u64 {
        self.reached.load(Ordering::SeqCst)
    }

    fn stop(&mut self) -> TestResult {
        self.uds.shutdown_handle().trigger();
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|panic| format!("server thread panicked: {panic:?}"))??;
        }
        Ok(())
    }
}

#[expect(
    clippy::expect_used,
    reason = "test fixture IDs provably satisfy the canonical ID policy"
)]
fn request(repo: &str) -> SearchPlaneControlIpcRequestEnvelope {
    SearchPlaneControlIpcRequestEnvelope {
        request_id: 3,
        payload: SearchPlaneControlIpcRequest::CurrentGeneration(CurrentGenerationRequest {
            repo_id: RepoId::new(repo).expect("test fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev")
                .expect("static fixture ID satisfies canonical policy"),
            track: SearchPlaneTrackKind::Lexical,
        }),
    }
}

fn send(socket: &Path, repo: &str) -> Result<SearchPlaneControlIpcResponse, IpcError> {
    let policy = ClientIoPolicy::try_new(CLIENT_BUDGET)?;
    send_request::<SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponseEnvelope>(
        socket,
        &request(repo),
        policy,
    )
    .map(|envelope| envelope.payload)
}

fn expect_snapshot(response: SearchPlaneControlIpcResponse, repo: &str) -> TestResult {
    match response {
        SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(snapshot)
            if snapshot.repo_id.as_str() == repo =>
        {
            Ok(())
        }
        other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
        | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
        | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
        | SearchPlaneControlIpcResponse::Error(_)
        | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
        | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
        | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
        | SearchPlaneControlIpcResponse::QuarantineInventory(_)
        | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
        | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => {
            Err(format!("expected a snapshot for {repo}, got {other:?}").into())
        }
    }
}

fn mode_of(path: &Path) -> Result<u32, Box<dyn Error>> {
    Ok(std::fs::symlink_metadata(path)?.mode() & 0o7777)
}

fn gid_of(path: &Path) -> Result<u32, Box<dyn Error>> {
    Ok(std::fs::symlink_metadata(path)?.gid())
}

fn self_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

fn self_gid() -> u32 {
    rustix::process::getegid().as_raw()
}

/// A gid this process is not a member of, by the same reading of
/// `getgroups` the server uses.
fn a_gid_this_process_is_not_in() -> Result<u32, Box<dyn Error>> {
    let mut members: BTreeSet<u32> = rustix::process::getgroups()?
        .into_iter()
        .map(rustix::process::Gid::as_raw)
        .collect();
    let _inserted = members.insert(self_gid());
    (1_u32..=u32::MAX)
        .find(|gid| !members.contains(gid))
        .ok_or_else(|| "this process is a member of every gid".into())
}

fn shared(group: Option<u32>, uids: &[u32]) -> SocketAccessPolicy {
    SocketAccessPolicy::Shared(SharedSocketAccess::new(
        group,
        uids.iter().copied().collect::<BTreeSet<u32>>(),
    ))
}

/// A directory under `/tmp` (sticky, world-traversable) that the group
/// `gid` can traverse: the placement a shared socket needs.
fn group_reachable_directory(gid: u32) -> Result<tempfile::TempDir, Box<dyn Error>> {
    let dir = tempfile::Builder::new()
        .prefix("qi-ipc-shared-")
        .tempdir_in("/tmp")?;
    std::os::unix::fs::chown(dir.path(), None, Some(gid))?;
    std::fs::set_permissions(
        dir.path(),
        std::fs::Permissions::from_mode(GROUP_DIRECTORY_MODE),
    )?;
    Ok(dir)
}

fn kernel_source() -> Arc<dyn PeerCredentialsSource> {
    Arc::new(quanta_index_ipc::KernelPeerCredentials)
}

/// A socket shared with this process's own primary group is `0660` of that
/// group; this process (the owner, and a member) is admitted by the
/// kernel's own report of its credentials.
#[test]
fn a_group_shared_socket_is_0660_of_that_group_and_serves_its_owner() -> TestResult {
    let dir = group_reachable_directory(self_gid())?;
    let socket = dir.path().join("group.sock");
    let mut server = Served::start(&socket, &shared(Some(self_gid()), &[]), kernel_source())?;
    if mode_of(&socket)? != GROUP_SOCKET_MODE {
        return Err(format!(
            "socket mode must be {GROUP_SOCKET_MODE:04o}, got {:04o}",
            mode_of(&socket)?
        )
        .into());
    }
    if gid_of(&socket)? != self_gid() {
        return Err(format!(
            "socket group must be {}, got {}",
            self_gid(),
            gid_of(&socket)?
        )
        .into());
    }
    expect_snapshot(send(&socket, "owner")?, "owner")?;
    let snapshot = server.counters.snapshot();
    if server.served() != 1
        || snapshot.peers_refused != 0
        || snapshot.peer_credentials_unreadable != 0
        || snapshot.connections_accepted != 1
    {
        return Err(format!(
            "the owner must be admitted once and nothing refused: served={} {snapshot:?}",
            server.served()
        )
        .into());
    }
    server.stop()
}

/// A socket the daemon creates the directory for gets that directory made
/// `0710` of the shared group, and every missing component of the path.
#[test]
fn a_directory_created_for_a_group_shared_socket_is_0710_of_the_group() -> TestResult {
    let dir = group_reachable_directory(self_gid())?;
    let leaf = dir.path().join("created").join("deeper");
    let socket = leaf.join("group.sock");
    let mut server = Served::start(&socket, &shared(Some(self_gid()), &[]), kernel_source())?;
    for created in [dir.path().join("created"), leaf] {
        if mode_of(&created)? != GROUP_DIRECTORY_MODE || gid_of(&created)? != self_gid() {
            return Err(format!(
                "{} must be {GROUP_DIRECTORY_MODE:04o} of gid {}, got {:04o} gid {}",
                created.display(),
                self_gid(),
                mode_of(&created)?,
                gid_of(&created)?
            )
            .into());
        }
    }
    expect_snapshot(send(&socket, "owner")?, "owner")?;
    server.stop()
}

/// A uid allow-list makes the socket world-connectable; the accept loop
/// is the gate.
///
/// A peer the script presents as a stranger is closed before any frame
/// reaches the dispatcher and counted; one the kernel cannot describe
/// likewise; the listed uid is then served on the same listener.
#[test]
fn a_listed_uid_policy_screens_every_peer_before_a_frame_is_read() -> TestResult {
    let listed = self_uid().saturating_add(10_001);
    let stranger = self_uid().saturating_add(10_002);
    let dir = group_reachable_directory(self_gid())?;
    // The listed uid could not traverse a `0710` directory of our group.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o711))?;
    let socket = dir.path().join("listed.sock");
    let script = Arc::new(ScriptedSource {
        next: Mutex::new(ScriptedPeer::Reports(PeerCredentials {
            uid: stranger,
            gid: self_gid(),
            pid: None,
        })),
    });
    let scripted: Arc<ScriptedSource> = Arc::clone(&script);
    let source: Arc<dyn PeerCredentialsSource> = scripted;
    let mut server = Served::start(&socket, &shared(None, &[listed]), source)?;
    if mode_of(&socket)? != WORLD_SOCKET_MODE {
        return Err(format!(
            "socket mode must be {WORLD_SOCKET_MODE:04o}, got {:04o}",
            mode_of(&socket)?
        )
        .into());
    }

    // A stranger — even one of our own gid, since no group is named.
    let refused = send(&socket, "stranger");
    if let Ok(response) = refused {
        return Err(format!("a stranger must not be answered, got {response:?}").into());
    }
    // A peer the kernel cannot describe.
    script.set(ScriptedPeer::Unreadable)?;
    let refused = send(&socket, "unreadable");
    if let Ok(response) = refused {
        return Err(format!("an unreadable peer must not be answered, got {response:?}").into());
    }
    let snapshot = server.counters.snapshot();
    if server.served() != 0
        || snapshot.peers_refused != 1
        || snapshot.peer_credentials_unreadable != 1
        || snapshot.connections_accepted != 0
        || snapshot.request_decode_failures != 0
        || snapshot.requests_dispatched != 0
    {
        return Err(format!(
            "no frame may reach the dispatcher and each refusal is counted once: served={} {snapshot:?}",
            server.served()
        )
        .into());
    }

    // The listener is still alive: the listed uid is served.
    script.set(ScriptedPeer::Reports(PeerCredentials {
        uid: listed,
        gid: self_gid().saturating_add(10_002),
        pid: None,
    }))?;
    expect_snapshot(send(&socket, "listed")?, "listed")?;
    let snapshot = server.counters.snapshot();
    if server.served() != 1 || snapshot.connections_accepted != 1 || snapshot.peers_refused != 1 {
        return Err(format!(
            "the listed uid must be served exactly once after the refusals: served={} {snapshot:?}",
            server.served()
        )
        .into());
    }
    server.stop()
}

/// A private socket keeps `0600` and admits only the owner: a scripted
/// stranger of the owner's own group is refused.
#[test]
fn a_private_socket_is_0600_and_admits_only_its_owner() -> TestResult {
    let dir = tempfile::tempdir()?;
    // The server refuses a socket directory wider than 0700 (QI-BB-014); a
    // tempdir is created under the umask, so it is narrowed here.
    std::fs::set_permissions(
        dir.path(),
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
    )?;
    let socket = dir.path().join("private.sock");
    let script = Arc::new(ScriptedSource {
        next: Mutex::new(ScriptedPeer::Reports(PeerCredentials {
            uid: self_uid(),
            gid: self_gid(),
            pid: None,
        })),
    });
    let scripted: Arc<ScriptedSource> = Arc::clone(&script);
    let source: Arc<dyn PeerCredentialsSource> = scripted;
    let mut server = Served::start(&socket, &SocketAccessPolicy::Private, source)?;
    if mode_of(&socket)? != PRIVATE_SOCKET_MODE {
        return Err(format!(
            "socket mode must be {PRIVATE_SOCKET_MODE:04o}, got {:04o}",
            mode_of(&socket)?
        )
        .into());
    }
    expect_snapshot(send(&socket, "owner")?, "owner")?;
    script.set(ScriptedPeer::Reports(PeerCredentials {
        uid: self_uid().saturating_add(10_003),
        gid: self_gid(),
        pid: None,
    }))?;
    if let Ok(response) = send(&socket, "stranger") {
        return Err(format!("a stranger must not be answered privately, got {response:?}").into());
    }
    let snapshot = server.counters.snapshot();
    if server.served() != 1 || snapshot.peers_refused != 1 || snapshot.connections_accepted != 1 {
        return Err(format!(
            "private: owner served once, stranger refused once: served={} {snapshot:?}",
            server.served()
        )
        .into());
    }
    server.stop()
}

/// A shared policy the host cannot honour is refused typed before
/// anything is bound.
///
/// Two such policies: a group this process is not in, and a path the
/// peers cannot traverse. Nothing is left on disk either way, and the
/// same path still binds privately.
#[test]
fn an_unsatisfiable_shared_policy_is_refused_before_bind() -> TestResult {
    let outsider = a_gid_this_process_is_not_in()?;
    let reachable = group_reachable_directory(self_gid())?;
    let socket = reachable.path().join("outsider.sock");
    match UdsServer::bind_observed(
        &socket,
        ServerAdmissionPolicy::DEFAULT,
        shared(Some(outsider), &[]),
        Arc::new(IpcServerCounters::for_plane("test")),
    ) {
        Ok(_bound) => return Err("a group this process is not in must be refused".into()),
        Err(IpcError::SocketAccessUnsatisfiable { reason, .. })
            if reason.contains(&format!("gid {outsider}")) => {}
        Err(other) => {
            return Err(format!("expected SOCKET_ACCESS_UNSATISFIABLE, got {other}").into());
        }
    }
    if socket.exists() {
        return Err("a refused bind must not leave a socket file".into());
    }

    // An owner-only directory (`0700`) is not traversable by the group.
    let unreachable = tempfile::Builder::new()
        .prefix("qi-ipc-unreachable-")
        .tempdir_in("/tmp")?;
    std::fs::set_permissions(unreachable.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = unreachable.path().join("unreachable.sock");
    match UdsServer::bind_observed(
        &socket,
        ServerAdmissionPolicy::DEFAULT,
        shared(Some(self_gid()), &[]),
        Arc::new(IpcServerCounters::for_plane("test")),
    ) {
        Ok(_bound) => return Err("an untraversable path must be refused".into()),
        Err(IpcError::SocketAccessUnsatisfiable { path, reason }) => {
            let resolved = std::fs::canonicalize(unreachable.path())?;
            if path != resolved || !reason.contains("cannot be traversed") {
                return Err(format!(
                    "the refusal must name the blocking directory {}: {} / {reason}",
                    resolved.display(),
                    path.display()
                )
                .into());
            }
        }
        Err(other) => {
            return Err(format!("expected SOCKET_ACCESS_UNSATISFIABLE, got {other}").into());
        }
    }
    if socket.exists() {
        return Err("a refused bind must not leave a socket file".into());
    }
    let private = UdsServer::bind(&socket)?;
    if mode_of(&socket)? != PRIVATE_SOCKET_MODE {
        return Err("the same path still binds privately".into());
    }
    drop(private);
    Ok(())
}
