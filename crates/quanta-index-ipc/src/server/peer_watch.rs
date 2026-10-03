//! Peer hangup watch for an in-flight IPC dispatch.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use quanta_index_core::CancelHandleV1;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;
use rustix::net::{RecvFlags, SendFlags, recv, send};

use crate::counters::IpcServerCounters;

/// How one watch run ended: the two terminal reasons are distinct types,
/// never one boolean the caller has to interpret.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PeerWatchOutcome {
    /// The peer hung up while watched; its budget was cancelled.
    HungUp,
    /// The watch stopped with the peer still able to receive.
    Stopped,
}

/// The watch thread's lifecycle, observed without touching production
/// state (TOPT-02 / R4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WatchEvent {
    /// The watcher completed its first observation and stays watching.
    /// A first observation that already sees a hang-up reports
    /// `PeerDisconnected` instead, and a stop that wins before the first
    /// observation reports `Stopped`: `Armed` means the watch is live.
    Armed,
    /// The watcher confirmed a hang-up and completed budget cancellation.
    PeerDisconnected,
    /// The watcher stopped with the peer still present.
    Stopped,
    /// The owner reaped the watcher thread.
    Joined,
}

/// A bounded, never-blocking sink for [`WatchEvent`].
///
/// Production arms its watches with [`WatchObserver::null`], whose sends
/// go nowhere; tests arm with [`WatchObserver::channel`] and wait on the
/// receiver instead of sleeping out poll intervals. The observer only
/// receives events — it cannot mutate the watch.
#[derive(Clone, Default)]
pub(super) struct WatchObserver {
    events: Option<mpsc::SyncSender<WatchEvent>>,
}

impl WatchObserver {
    fn null() -> Self {
        Self { events: None }
    }

    #[cfg(test)]
    pub(super) fn channel() -> (Self, mpsc::Receiver<WatchEvent>) {
        // Four lifecycle events per watch; the bound only contains a test
        // that stopped draining.
        let (events, received) = mpsc::sync_channel(16);
        (
            Self {
                events: Some(events),
            },
            received,
        )
    }

    fn fire(&self, event: WatchEvent) {
        if let Some(events) = &self.events {
            let _dropped = events.try_send(event);
        }
    }
}

/// Watches a connection for a hang-up while its request is dispatching.
///
/// The dispatch runs synchronously on the connection thread, so a second
/// thread polls the socket. Readable with data means the peer pipelined
/// its next request; that is not a hang-up, but the peer may still leave
/// before its answer, so the watch keeps asking whether it can receive
/// instead of polling (the pipelined bytes keep the socket readable).
/// `POLLHUP`, `POLLERR` or an end-of-file mean the peer at least shut its
/// write side — which is *not* yet a hang-up: a peer that sent its request
/// and half-closed is still waiting for the response. The watch confirms a
/// hang-up by asking whether the peer can still receive (a zero-byte send,
/// which fails with `EPIPE` only once the peer's read side is gone), and
/// after a half-close or a pipelined frame it keeps asking at the probe
/// cadence. The watch ends when the dispatch returns.
///
/// Stopping is event-driven: the watched poll set carries a wake FD beside
/// the peer socket, and disarming signals it before joining, so the join
/// never waits out a poll quantum. The poll timeout and the probe cadence
/// remain only as a defensive fallback — the kernel reports no event when
/// a half-closed peer's read side goes away, so the send probe is still
/// what notices that transition.
pub(super) struct PeerWatch {
    stop: Arc<AtomicBool>,
    hung_up: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The write end of the wake pair; the watcher thread owns the read
    /// end. Both close exactly once with their owners.
    wake: Option<UnixStream>,
    observer: WatchObserver,
}

impl PeerWatch {
    /// Defensive poll/probe bound: `poll` returns at once on peer events
    /// and on the wake FD, so this only paces the send probe after a
    /// half-close and bounds a stuck poll.
    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    pub(super) fn arm(
        stream: &UnixStream,
        cancel: CancelHandleV1,
        counters: Arc<IpcServerCounters>,
    ) -> std::io::Result<Self> {
        Self::arm_with_observer(stream, cancel, counters, WatchObserver::null())
    }

    pub(super) fn arm_with_observer(
        stream: &UnixStream,
        cancel: CancelHandleV1,
        counters: Arc<IpcServerCounters>,
        observer: WatchObserver,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let hung_up = Arc::new(AtomicBool::new(false));
        let watched = stream.try_clone()?;
        let (wake_read, wake_write) = UnixStream::pair()?;
        let thread = {
            let stop = Arc::clone(&stop);
            let hung_up = Arc::clone(&hung_up);
            let observer = observer.clone();
            std::thread::Builder::new()
                .name("uds-peer-watch".to_string())
                .spawn(move || {
                    // Once the socket stays readable (a half-close or a
                    // pipelined frame), polling would spin; the send probe
                    // alone tells a waiting peer from a departed one. The
                    // probe wait is a wake-FD poll, so a stop still lands
                    // at once instead of at the end of a sleep.
                    let mut probe_only = false;
                    let mut armed = false;
                    loop {
                        if stop.load(Ordering::Acquire) {
                            observer.fire(WatchEvent::Stopped);
                            return;
                        }
                        if probe_only {
                            if stop_signalled(&wake_read) && stop.load(Ordering::Acquire) {
                                observer.fire(WatchEvent::Stopped);
                                return;
                            }
                            if peer_can_receive(&watched) {
                                continue;
                            }
                            hung_up.store(true, Ordering::Release);
                            counters.peer_hangup_detected();
                            cancel.cancel();
                            observer.fire(WatchEvent::PeerDisconnected);
                            return;
                        }
                        match peer_state(&watched, &wake_read) {
                            PeerState::Alive => {}
                            PeerState::HalfClosed | PeerState::Pipelined => probe_only = true,
                            PeerState::StopRequested => {
                                if stop.load(Ordering::Acquire) {
                                    observer.fire(WatchEvent::Stopped);
                                    return;
                                }
                                // A wake byte with no stop is impossible —
                                // the only writer sets the flag first — so
                                // drain defensively and keep watching rather
                                // than spin on the readable FD.
                                drain_wake_byte(&wake_read);
                                continue;
                            }
                            PeerState::HungUp => {
                                hung_up.store(true, Ordering::Release);
                                counters.peer_hangup_detected();
                                cancel.cancel();
                                observer.fire(WatchEvent::PeerDisconnected);
                                return;
                            }
                        }
                        if !armed {
                            armed = true;
                            observer.fire(WatchEvent::Armed);
                        }
                    }
                })?
        };
        Ok(Self {
            stop,
            hung_up,
            thread: Some(thread),
            wake: Some(wake_write),
            observer,
        })
    }

    /// Stop the watcher and join its thread — at most once, whether the
    /// owner disarms or unwinds past the watch.
    fn stop_and_join(&mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            if let Some(mut wake) = self.wake.take() {
                // The watcher may already have returned (and closed the
                // read end with its thread): a failed write then means
                // exactly that, and the join below still reaps it.
                let _signalled = wake.write_all(&[1]);
            }
            let joined = thread.join();
            self.observer.fire(WatchEvent::Joined);
            joined.map_err(|panic| format!("peer watch thread panicked: {panic:?}"))?;
        }
        Ok(())
    }

    /// Stop watching; reports how the watch run ended.
    pub(super) fn disarm(mut self) -> Result<PeerWatchOutcome, String> {
        self.stop_and_join()?;
        if self.hung_up.load(Ordering::Acquire) {
            Ok(PeerWatchOutcome::HungUp)
        } else {
            Ok(PeerWatchOutcome::Stopped)
        }
    }
}

impl Drop for PeerWatch {
    /// Reconcile a watch whose owner did not disarm it — a panicking
    /// dispatcher unwinding (S21-09): the watcher thread is stopped and
    /// joined, never left spinning without an owner.
    fn drop(&mut self) {
        // A dispatcher already unwinding cannot return another error;
        // the normal disarm path above propagates watcher failure.
        let _result = self.stop_and_join();
    }
}

enum PeerState {
    Alive,
    /// The peer shut its write side and is waiting for the response.
    HalfClosed,
    Pipelined,
    HungUp,
    /// The wake FD fired: the owner is stopping the watch.
    StopRequested,
}

/// Block up to one poll interval for the owner's stop signal.
fn stop_signalled(wake: &UnixStream) -> bool {
    let fd = std::os::fd::AsFd::as_fd(wake);
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    matches!(poll(&mut fds, Some(&poll_timeout())), Ok(count) if count > 0)
}

/// Best-effort drain of one wake byte; only the defensive path uses it.
fn drain_wake_byte(wake: &UnixStream) {
    let fd = std::os::fd::AsFd::as_fd(wake);
    let mut byte = [0_u8; 1];
    let _drained = recv(fd, &mut byte, RecvFlags::empty());
}

fn poll_timeout() -> Timespec {
    Timespec {
        tv_sec: 0,
        tv_nsec: i64::try_from(PeerWatch::POLL_INTERVAL.as_nanos())
            .map_or(50_000_000, |nanos| nanos),
    }
}

/// Whether the peer can still receive: a zero-byte send succeeds while
/// the peer's read side is open and fails with `EPIPE` once it is gone.
///
/// A half-close (`shutdown(Write)` on the peer) leaves its read side open,
/// so this is what tells a waiting peer from a departed one; `poll` alone
/// cannot, because Darwin reports `POLLHUP` for both. `SIGPIPE` is not a
/// concern: the Rust runtime ignores it, and the socket carries
/// `SO_NOSIGPIPE` where the platform has it.
fn peer_can_receive(stream: &UnixStream) -> bool {
    let fd = std::os::fd::AsFd::as_fd(stream);
    match send(fd, &[], peer_probe_send_flags()) {
        Ok(_sent) => true,
        Err(Errno::AGAIN | Errno::INTR) => true,
        Err(_gone) => false,
    }
}

#[cfg(target_os = "linux")]
fn peer_probe_send_flags() -> SendFlags {
    SendFlags::DONTWAIT | SendFlags::NOSIGNAL
}

#[cfg(not(target_os = "linux"))]
fn peer_probe_send_flags() -> SendFlags {
    SendFlags::DONTWAIT
}

/// One bounded poll of the watched socket, for a peer not yet seen to
/// half-close. The wake FD rides in the same poll set so a stop lands
/// at once instead of at the end of the timeout.
fn peer_state(stream: &UnixStream, wake: &UnixStream) -> PeerState {
    let fd = std::os::fd::AsFd::as_fd(stream);
    let wake_fd = std::os::fd::AsFd::as_fd(wake);
    let mut fds = [
        PollFd::new(&fd, PollFlags::IN | PollFlags::HUP),
        PollFd::new(&wake_fd, PollFlags::IN),
    ];
    let timeout = poll_timeout();
    let closed_or_gone = |stream: &UnixStream| {
        if peer_can_receive(stream) {
            PeerState::HalfClosed
        } else {
            PeerState::HungUp
        }
    };
    match poll(&mut fds, Some(&timeout)) {
        Ok(0) | Err(Errno::INTR) => PeerState::Alive,
        Ok(_) => {
            if fds
                .get(1)
                .map_or(PollFlags::empty(), PollFd::revents)
                .contains(PollFlags::IN)
            {
                return PeerState::StopRequested;
            }
            let revents = fds.first().map_or(PollFlags::empty(), PollFd::revents);
            if revents.contains(PollFlags::IN) {
                // Readable: either pipelined data or the peer's end-of-file.
                // A peek leaves the bytes for the connection thread's next
                // request read.
                let mut probe = [0_u8; 1];
                return match recv(fd, &mut probe, RecvFlags::PEEK) {
                    Ok((0, _)) => closed_or_gone(stream),
                    Ok(_) => PeerState::Pipelined,
                    Err(Errno::AGAIN | Errno::INTR) => PeerState::Alive,
                    Err(_) => closed_or_gone(stream),
                };
            }
            if revents.contains(PollFlags::HUP) || revents.contains(PollFlags::ERR) {
                return closed_or_gone(stream);
            }
            PeerState::Alive
        }
        Err(_) => closed_or_gone(stream),
    }
}
