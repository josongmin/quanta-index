//! The supervised runtime lifecycle (SEP-21 P08 / S21-09).
//!
//! One `SearchdSupervisor` owns every required child thread of the daemon
//! — the query, control and ingest accept loops, the maintenance timer,
//! and any provider executor registered later — plus the runtime guards
//! (maintenance, corpus lifecycle, state-root lease). No child holds a
//! detached `JoinHandle`: every spawned child is registered with a stop
//! closure and a join handle, reports its terminal result through one
//! channel. An escalated child is transferred with the runtime guards to
//! a custody reaper, so the state-root lease cannot be released while a
//! child is still running.
//!
//! Lifecycle:
//!
//! ```text
//! Starting -> Ready -> Draining -> Stopped
//!     |         |         |
//!     +-------> Failed <--+
//! ```
//!
//! Shutdown is two-deadline bounded. The cooperative deadline is how long
//! well-behaved children get to observe cancellation and exit on their
//! own; the hard deadline (`HARD_DRAIN_DEADLINE`, frozen) is the outer
//! bound of the join itself. A child still alive at the hard deadline is
//! escalated and named in the receipt, never marked graceful. The supervisor
//! returns by the deadline; a separate custody reaper owns and joins the
//! unfinished child before it drops the guards. A second-signal abort uses
//! the same custody transfer without waiting in the supervisor.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The frozen outer bound of a supervised drain (S21-09). A child still
/// alive after this deadline is escalated, never waited for again.
pub const HARD_DRAIN_DEADLINE: Duration = Duration::from_secs(125);

/// How long well-behaved children get to exit on their own after the
/// cancellation root fires, before the supervisor calls them overdue.
///
/// Distinct from the hard deadline: passing it is a receipt fact, not a
/// failure by itself.
pub const DEFAULT_COOPERATIVE_DRAIN_DEADLINE: Duration = Duration::from_secs(30);

/// The supervisor's observation cadence while serving and draining.
const SUPERVISOR_POLL: Duration = Duration::from_millis(10);

/// The phases of one supervised runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SupervisorPhase {
    /// Children are being spawned; all-or-rollback.
    Starting,
    /// Every required child is spawned and none has exited.
    Ready,
    /// Cancellation has been fired; children are exiting.
    Draining,
    /// Every child joined cleanly.
    Stopped,
    /// A required child was lost, startup rolled back, or the hard
    /// deadline escalated remaining children.
    Failed,
}

/// A process-wide observation of the supervisor and required serving loops.
///
/// The supervisor owns phase transitions; child adapters own their liveness
/// bits. A surviving control socket therefore cannot report the process as
/// ready after another required accept loop has exited.
#[derive(Debug, Default)]
pub(crate) struct SupervisorStatus {
    phase: AtomicU8,
    query: AtomicBool,
    control: AtomicBool,
    ingest: AtomicBool,
    provider: AtomicBool,
}

impl SupervisorStatus {
    fn set_phase(&self, phase: SupervisorPhase) {
        let code = match phase {
            SupervisorPhase::Starting => 0,
            SupervisorPhase::Ready => 1,
            SupervisorPhase::Draining => 2,
            SupervisorPhase::Stopped => 3,
            SupervisorPhase::Failed => 4,
        };
        self.phase.store(code, Ordering::Release);
    }

    /// Read the supervisor's last published lifecycle phase.
    #[must_use]
    pub(crate) fn phase(&self) -> SupervisorPhase {
        match self.phase.load(Ordering::Acquire) {
            1 => SupervisorPhase::Ready,
            2 => SupervisorPhase::Draining,
            3 => SupervisorPhase::Stopped,
            4 => SupervisorPhase::Failed,
            _ => SupervisorPhase::Starting,
        }
    }

    pub(crate) fn set_child_running(&self, name: &'static str, running: bool) {
        let flag = match name {
            "query-accept" => &self.query,
            "control-accept" => &self.control,
            "ingest-accept" => &self.ingest,
            "provider-attempts" => &self.provider,
            _ => return,
        };
        flag.store(running, Ordering::Release);
    }

    #[must_use]
    pub(crate) fn required_planes(&self) -> (bool, bool, bool) {
        (
            self.query.load(Ordering::Acquire),
            self.control.load(Ordering::Acquire),
            self.ingest.load(Ordering::Acquire),
        )
    }

    #[must_use]
    pub(crate) fn provider_running(&self) -> bool {
        self.provider.load(Ordering::Acquire)
    }
}

/// How one registered child ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildExitKind {
    /// The child returned `Ok(())`.
    Completed,
    /// The child returned a typed error.
    Failed,
    /// The child thread panicked; the payload is not a string, the
    /// registry name is the identification.
    Panicked,
}

/// The terminal event one child sends as the last statement of its
/// adapted thread body, immediately before returning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChildExit {
    /// The registered child's name.
    pub name: &'static str,
    /// How it ended.
    pub kind: ChildExitKind,
}

/// A spawn the OS or thread table refused; typed, never a raw string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChildSpawnFailure {
    /// The registered child that could not be spawned.
    pub name: &'static str,
}

impl core::fmt::Display for ChildSpawnFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "supervised child could not be spawned: {}", self.name)
    }
}

impl std::error::Error for ChildSpawnFailure {}

/// The one cancellation tree root shared by the process: the operator
/// shutdown request, the signal latch, and the flag children poll.
///
/// The first request (operator or signal) latches cooperative shutdown;
/// a second signal latches abort, which the supervisor answers with an
/// immediate `128 + signum` exit without waiting for any child.
#[derive(Clone, Debug)]
pub struct CancelRoot {
    shutdown: Arc<AtomicBool>,
    abort_signum: Arc<AtomicI32>,
}

impl Default for CancelRoot {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelRoot {
    /// A fresh root, nothing requested.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shutdown: Arc::new(AtomicBool::new(false)),
            abort_signum: Arc::new(AtomicI32::new(0)),
        }
    }

    /// Request cooperative shutdown (operator request or first signal).
    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    /// Latch an immediate abort (second signal). Also implies shutdown.
    pub fn request_abort(&self, signum: i32) {
        self.shutdown.store(true, Ordering::Release);
        self.abort_signum.store(signum, Ordering::Release);
    }

    /// Whether cooperative shutdown has been requested.
    #[must_use]
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    /// The latched abort signal number, `0` when none.
    #[must_use]
    pub fn abort_signum(&self) -> i32 {
        self.abort_signum.load(Ordering::Acquire)
    }

    /// The cooperative cancellation flag children poll.
    #[must_use]
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown)
    }
}

/// What one child thread receives at spawn time: its name, the shared
/// cooperative cancellation flag, and the terminal-event channel.
#[derive(Clone)]
pub struct ChildContext {
    name: &'static str,
    shutdown: Arc<AtomicBool>,
    exits: Sender<ChildExit>,
}

impl ChildContext {
    /// Report this child's terminal result. Must be the last statement
    /// of the adapted child body, immediately before returning, so a
    /// supervisor that has seen the event joins only bounded thread
    /// teardown.
    pub fn report_exit(&self, kind: ChildExitKind) {
        let _send_result = self.exits.send(ChildExit {
            name: self.name,
            kind,
        });
    }

    /// The cooperative cancellation flag this child must poll.
    #[must_use]
    pub fn shutdown(&self) -> &Arc<AtomicBool> {
        &self.shutdown
    }
}

/// One registered child in the supervisor's registry.
struct RegisteredChild {
    name: &'static str,
    join: Option<JoinHandle<()>>,
    stop: Option<Box<dyn FnOnce() + Send>>,
    kind: Option<ChildExitKind>,
    reports_exit: bool,
}

/// How a supervised runtime ended.
///
/// The exit code is fixed policy: clean operator shutdown `0`;
/// required-child death, startup rollback, child failure during drain,
/// or hard-deadline escalation `70`; a latched abort `128 + signum`.
#[derive(Debug, Eq, PartialEq)]
pub enum SupervisionOutcome {
    /// Operator shutdown; every child exited and joined.
    StoppedClean {
        /// Children that exited and joined, in registration order.
        drained: Vec<&'static str>,
        /// Children that were still running when the cooperative
        /// deadline passed but joined before the hard deadline. The
        /// drain stayed graceful, the receipt still names them.
        cooperative_overdue: Vec<&'static str>,
    },
    /// A child could not be spawned; everything already started was
    /// stopped in reverse order. Unfinished children retain the guards
    /// through the custody reaper.
    StartupRollback {
        /// The child whose spawn failed.
        failed: &'static str,
        /// Children torn down, in reverse start order.
        torn_down: Vec<&'static str>,
        /// Children still alive at the hard deadline of the rollback;
        /// their joins and guards transfer to the custody reaper.
        escalated: Vec<&'static str>,
    },
    /// A required child exited while the runtime was serving; readiness
    /// went down, cancellation fired, the peers drained.
    RequiredChildLost {
        /// The child that was lost.
        name: &'static str,
        /// How it ended.
        kind: ChildExitKind,
        /// The other children that were drained afterwards.
        drained: Vec<&'static str>,
        /// Children escalated at the hard deadline.
        escalated: Vec<&'static str>,
    },
    /// Operator shutdown completed but at least one child ended
    /// uncleanly; this is not a graceful success.
    DrainFailed {
        /// The children that failed or panicked during the drain.
        failed: Vec<(&'static str, ChildExitKind)>,
    },
    /// The hard drain deadline passed with children still alive; their
    /// joins transfer to the custody reaper and are named here.
    HardDeadlineEscalated {
        /// The children still alive at the hard deadline.
        unfinished: Vec<&'static str>,
    },
    /// A second signal latched an immediate abort; nothing was waited
    /// for.
    SignalAbort {
        /// The signal whose delivery latched the abort.
        signum: i32,
    },
}

impl SupervisionOutcome {
    /// The fixed process exit code for this outcome.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::StoppedClean { .. } => 0,
            Self::StartupRollback { .. }
            | Self::RequiredChildLost { .. }
            | Self::DrainFailed { .. }
            | Self::HardDeadlineEscalated { .. } => 70,
            Self::SignalAbort { signum } => (*signum).saturating_add(128),
        }
    }

    /// Whether the runtime stopped without any lost, failed, or
    /// escalated child.
    #[must_use]
    pub fn is_stopped_clean(&self) -> bool {
        matches!(self, Self::StoppedClean { .. })
    }
}

/// The typed supervision error surfaced by compatibility wrappers that
/// must return `Result` instead of a [`SupervisionOutcome`].
#[derive(Debug, Eq, PartialEq)]
pub struct SupervisionError {
    /// The outcome the supervisor derived.
    pub outcome: SupervisionOutcome,
}

impl core::fmt::Display for SupervisionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "supervised runtime did not stop cleanly: exit code {}; outcome: {:?}",
            self.outcome.exit_code(),
            self.outcome
        )
    }
}

impl std::error::Error for SupervisionError {}

/// The later of two instants, for joining a cooperative checkpoint to
/// the hard deadline that follows it.
fn hard_at_or(cooperative_at: Instant, hard_at: Instant) -> Instant {
    if cooperative_at >= hard_at {
        cooperative_at
    } else {
        hard_at
    }
}

/// The terminal states a drain can leave a child in.
#[derive(Default)]
struct DrainTally {
    drained: Vec<&'static str>,
    failed: Vec<(&'static str, ChildExitKind)>,
    unfinished: Vec<&'static str>,
}

/// One `SearchdSupervisor`: the single owner of the daemon's child
/// threads and runtime guards.
///
/// `G` is the guard payload (the daemon's `RuntimeGuards`; a drop-order
/// recorder in tests). It is stored in the supervisor until every child
/// joins, or transferred with unfinished children to the custody reaper.
pub struct SearchdSupervisor<G: Send + 'static> {
    phase: SupervisorPhase,
    status: Option<Arc<SupervisorStatus>>,
    cancel: CancelRoot,
    children: Vec<RegisteredChild>,
    exits: Receiver<ChildExit>,
    exits_tx: Sender<ChildExit>,
    pending: Vec<ChildExit>,
    cooperative_deadline: Duration,
    hard_deadline: Duration,
    cooperative_overdue: Vec<&'static str>,
    guards: Option<G>,
}

impl<G: Send + 'static> Drop for SearchdSupervisor<G> {
    fn drop(&mut self) {
        if self.guards.is_some() {
            self.set_phase(SupervisorPhase::Failed);
            self.cancel.request_shutdown();
            self.release_guards_after_children();
        }
    }
}

impl<G: Send + 'static> SearchdSupervisor<G> {
    /// A supervisor with no children yet, holding `guards` until the
    /// end of its lifecycle. `cancel` is the one cancellation tree root
    /// the whole process shares: children poll its flag, `run` observes
    /// its shutdown and abort latches.
    #[must_use]
    pub fn new(
        cooperative_deadline: Duration,
        hard_deadline: Duration,
        guards: G,
        cancel: CancelRoot,
    ) -> Self {
        debug_assert!(
            cooperative_deadline <= hard_deadline,
            "the cooperative drain deadline must not exceed the hard deadline"
        );
        let (exits_tx, exits) = channel();
        Self {
            phase: SupervisorPhase::Starting,
            status: None,
            cancel,
            children: Vec::new(),
            exits,
            exits_tx,
            pending: Vec::new(),
            cooperative_deadline,
            hard_deadline,
            cooperative_overdue: Vec::new(),
            guards: Some(guards),
        }
    }

    /// Publish lifecycle transitions to the process-readiness authority.
    #[must_use]
    pub(crate) fn with_status(mut self, status: Arc<SupervisorStatus>) -> Self {
        status.set_phase(self.phase);
        self.status = Some(status);
        self
    }

    fn set_phase(&mut self, phase: SupervisorPhase) {
        self.phase = phase;
        if let Some(status) = &self.status {
            status.set_phase(phase);
        }
    }

    /// The supervisor's current phase; readiness is `Ready`.
    #[must_use]
    pub fn phase(&self) -> SupervisorPhase {
        self.phase
    }

    /// The cancellation tree root this supervisor serves under. The
    /// same root is shared with every child context.
    #[must_use]
    pub fn cancel_root(&self) -> &CancelRoot {
        &self.cancel
    }

    /// Spawn and register one child. `spawn` receives the child's
    /// context (name, cancellation flag, terminal channel) and returns
    /// the adapted join handle; an `Err` is a typed spawn failure the
    /// caller answers with [`Self::rollback`].
    pub fn spawn_child<F>(
        &mut self,
        name: &'static str,
        stop: Box<dyn FnOnce() + Send>,
        spawn: F,
    ) -> Result<(), ChildSpawnFailure>
    where
        F: FnOnce(ChildContext) -> anyhow::Result<JoinHandle<()>>,
    {
        let context = ChildContext {
            name,
            shutdown: self.cancel.shutdown_flag(),
            exits: Sender::clone(&self.exits_tx),
        };
        match spawn(context) {
            Ok(join) => {
                self.children.push(RegisteredChild {
                    name,
                    join: Some(join),
                    stop: Some(stop),
                    kind: None,
                    reports_exit: true,
                });
                Ok(())
            }
            Err(_refused) => Err(ChildSpawnFailure { name }),
        }
    }

    /// Register an already-running child whose join handle is transferred
    /// directly to the supervisor. The maintenance timer uses this path:
    /// no second adapter spawn may strand its original timer thread.
    pub fn adopt_child(
        &mut self,
        name: &'static str,
        stop: Box<dyn FnOnce() + Send>,
        join: JoinHandle<()>,
    ) {
        self.children.push(RegisteredChild {
            name,
            join: Some(join),
            stop: Some(stop),
            kind: None,
            reports_exit: false,
        });
    }

    /// Stop and join everything already started, in reverse start
    /// order, bounded by the hard deadline; then release the guards. The
    /// answer to a spawn failure during startup; `failed` is the child
    /// that could not be spawned.
    pub fn rollback(mut self, failed: &'static str) -> SupervisionOutcome {
        let mut torn_down: Vec<&'static str> = Vec::new();
        let mut escalated: Vec<&'static str> = Vec::new();
        // Global cancellation: what was started must now stop.
        self.cancel.request_shutdown();
        let cancel = CancelRoot::clone(&self.cancel);
        let deadline = self.hard_deadline_at_now();
        let Self {
            children,
            pending,
            exits,
            ..
        } = &mut self;
        for child in children.iter_mut().rev() {
            if let Some(stop) = child.stop.take() {
                stop();
            }
            if Self::await_child(child, pending, exits, deadline, &cancel) {
                torn_down.push(child.name);
            } else {
                escalated.push(child.name);
            }
        }
        self.set_phase(SupervisorPhase::Failed);
        if let Some(signum) = self.latched_abort() {
            return self.abort(signum);
        }
        self.release_guards_after_children();
        SupervisionOutcome::StartupRollback {
            failed,
            torn_down,
            escalated,
        }
    }

    /// Serve until the cancellation root fires or a child exits, then
    /// drain under the two-deadline policy and derive the outcome.
    /// Never joins unboundedly; unfinished children retain the guards
    /// through the custody reaper.
    pub fn run(mut self, external: &CancelRoot) -> SupervisionOutcome {
        // Startup: every registered child is spawned; the supervisor
        // becomes ready only with all of them running.
        if external.abort_signum() != 0 {
            return self.abort(external.abort_signum());
        }
        if let Some(lost) = self.observe_finished_direct_children() {
            return self.required_child_lost(lost);
        }
        if let Some(lost) = self.take_pending_exits() {
            return self.required_child_lost(lost);
        }
        self.set_phase(SupervisorPhase::Ready);

        // Serving: watch the cancellation root and the children.
        loop {
            if external.abort_signum() != 0 {
                return self.abort(external.abort_signum());
            }
            let observed: Option<ChildExit> = match self.exits.recv_timeout(SUPERVISOR_POLL) {
                Ok(exit) => {
                    self.pending.push(exit);
                    // A child that exits after cancellation was requested
                    // is part of the drain, not a loss.
                    if external.shutdown_requested() || self.cancel.shutdown_requested() {
                        break;
                    }
                    self.take_pending_exits()
                }
                // The supervisor holds a sender clone, so a disconnect
                // cannot happen; both quiet errors are a quiet poll.
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
            };
            if let Some(lost) = observed {
                return self.required_child_lost(lost);
            }
            if let Some(lost) = self.observe_finished_direct_children() {
                if external.shutdown_requested() || self.cancel.shutdown_requested() {
                    break;
                }
                return self.required_child_lost(lost);
            }
            if external.shutdown_requested() || self.cancel.shutdown_requested() {
                break;
            }
        }
        self.drain()
    }

    /// Operator-requested drain: fire every stop, give children the
    /// cooperative deadline to exit on their own (recording the ones
    /// that miss it), then bound the whole join by the hard deadline.
    /// Children whose terminal event arrived and whose thread actually
    /// finished are joined. A reported child still in teardown at the
    /// hard deadline is escalated with its join handle and guards.
    fn drain(mut self) -> SupervisionOutcome {
        self.set_phase(SupervisorPhase::Draining);
        self.stop_all();
        // Fold any terminal events that queued during the serving loop.
        let _queued = self.take_pending_exits();
        let started = Instant::now();
        let cooperative_at = started
            .checked_add(self.cooperative_deadline)
            .unwrap_or_else(Instant::now);
        self.collect_exits_until(cooperative_at);
        // A latched abort outranks the drain: nothing is waited for.
        if let Some(signum) = self.latched_abort() {
            return self.abort(signum);
        }
        // Who was still running when the cooperative checkpoint passed.
        self.cooperative_overdue = self
            .children
            .iter()
            .filter(|child| {
                child.kind.is_none() || child.join.as_ref().is_some_and(|join| !join.is_finished())
            })
            .map(|child| child.name)
            .collect();
        let deadline = self.hard_deadline_at_now();
        self.collect_exits_until(hard_at_or(cooperative_at, deadline));
        // The abort answer outranks the receipt: nothing is waited for.
        if let Some(signum) = self.latched_abort() {
            return self.abort(signum);
        }
        let tally = self.join_children_with_known_exits();
        self.finish_drain(tally)
    }

    /// The abort signum when one is latched on the shared root.
    fn latched_abort(&self) -> Option<i32> {
        let signum = self.cancel.abort_signum();
        if signum == 0 { None } else { Some(signum) }
    }

    /// A child ended while serving: readiness is down, fire every stop,
    /// then drain under the same hard deadline.
    fn required_child_lost(mut self, lost: ChildExit) -> SupervisionOutcome {
        self.set_phase(SupervisorPhase::Failed);
        // Global cancellation: every well-behaved peer stops now.
        self.cancel.request_shutdown();
        let cancel = CancelRoot::clone(&self.cancel);
        self.stop_all();
        let deadline = self.hard_deadline_at_now();
        let Self {
            children,
            pending,
            exits,
            ..
        } = &mut self;
        for child in children.iter_mut() {
            if child.kind.is_none() {
                let _awaited = Self::await_child(child, pending, exits, deadline, &cancel);
            }
        }
        if let Some(signum) = self.latched_abort() {
            return self.abort(signum);
        }
        let tally = self.join_children_with_known_exits();
        self.release_guards_after_children();
        if !tally.unfinished.is_empty() {
            return SupervisionOutcome::HardDeadlineEscalated {
                unfinished: tally.unfinished,
            };
        }
        let escalated = tally.escalated();
        // The lost child itself is not a drained peer.
        let drained = tally
            .drained
            .into_iter()
            .filter(|name| *name != lost.name)
            .collect();
        SupervisionOutcome::RequiredChildLost {
            name: lost.name,
            kind: lost.kind,
            drained,
            escalated,
        }
    }

    /// The abort answer to a second signal: nothing is waited for.
    fn abort(mut self, signum: i32) -> SupervisionOutcome {
        self.set_phase(SupervisorPhase::Failed);
        self.release_guards_after_children();
        SupervisionOutcome::SignalAbort { signum }
    }

    /// Classify the drain tally into the final outcome. Consumes the
    /// guards after every join or escalation.
    fn finish_drain(mut self, tally: DrainTally) -> SupervisionOutcome {
        self.set_phase(if tally.unfinished.is_empty() && tally.failed.is_empty() {
            SupervisorPhase::Stopped
        } else {
            SupervisorPhase::Failed
        });
        self.release_guards_after_children();
        if !tally.unfinished.is_empty() {
            return SupervisionOutcome::HardDeadlineEscalated {
                unfinished: tally.unfinished,
            };
        }
        if !tally.failed.is_empty() {
            return SupervisionOutcome::DrainFailed {
                failed: tally.failed,
            };
        }
        SupervisionOutcome::StoppedClean {
            drained: tally.drained,
            cooperative_overdue: std::mem::take(&mut self.cooperative_overdue),
        }
    }

    /// Collect terminal events until `deadline` or until every child
    /// reported, folding them onto their children. Polls in small
    /// slices so a latched abort or an early completion is never missed
    /// behind one long blocking wait; returns with the children folded,
    /// never with a queued event unobserved.
    fn collect_exits_until(&mut self, deadline: Instant) {
        loop {
            if self.cancel.abort_signum() != 0 {
                return;
            }
            let _finished = self.observe_finished_direct_children();
            if self.children.iter().all(|child| {
                child.kind.is_some() && child.join.as_ref().is_none_or(JoinHandle::is_finished)
            }) {
                return;
            }
            let now = Instant::now();
            if now >= deadline {
                return;
            }
            let slice = deadline
                .checked_duration_since(now)
                .map_or(SUPERVISOR_POLL, |remaining| {
                    if remaining > SUPERVISOR_POLL {
                        SUPERVISOR_POLL
                    } else {
                        remaining
                    }
                });
            match self.exits.recv_timeout(slice) {
                Ok(exit) => {
                    self.pending.push(exit);
                    let _observed = self.take_pending_exits();
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }
        }
    }

    /// A directly adopted child has no terminal-event adapter. Its actual
    /// finished join handle is the authority for a serving-time loss and
    /// for drain classification; never infer completion from a timer tick.
    fn observe_finished_direct_children(&mut self) -> Option<ChildExit> {
        let mut first = None;
        for child in &mut self.children {
            if child.reports_exit || child.kind.is_some() {
                continue;
            }
            if !child.join.as_ref().is_some_and(JoinHandle::is_finished) {
                continue;
            }
            let kind = match child.join.take().map(JoinHandle::join) {
                Some(Err(_panic)) => ChildExitKind::Panicked,
                Some(Ok(())) | None => ChildExitKind::Completed,
            };
            child.kind = Some(kind);
            if first.is_none() {
                first = Some(ChildExit {
                    name: child.name,
                    kind,
                });
            }
        }
        first
    }

    fn stop_all(&mut self) {
        for child in &mut self.children {
            if let Some(stop) = child.stop.take() {
                stop();
            }
        }
    }

    /// Return without waiting beyond the hard deadline, but never release
    /// the state-root lease while an escalated child may still use it.
    /// The reaper owns each remaining stop closure and join handle as well
    /// as the guards. If the OS refuses to spawn that reaper, deliberately
    /// retain the payload until process exit: dropping the lease early is
    /// less safe than retaining it after an already-failed shutdown.
    fn release_guards_after_children(&mut self) {
        let Some(guards) = self.guards.take() else {
            return;
        };
        if self.children.iter().all(|child| child.join.is_none()) {
            drop(guards);
            return;
        }
        let payload = Arc::new(Mutex::new(Some((
            std::mem::take(&mut self.children),
            guards,
        ))));
        let worker = Arc::clone(&payload);
        let spawned = std::thread::Builder::new()
            .name("searchd-custody-reaper".to_string())
            .spawn(move || {
                let retained = match worker.lock() {
                    Ok(mut slot) => slot.take(),
                    Err(poisoned) => poisoned.into_inner().take(),
                };
                if let Some((mut children, guards)) = retained {
                    for child in &mut children {
                        if let Some(stop) = child.stop.take() {
                            // A faulty stop callback must not unwind the
                            // reaper and release the lease before joins.
                            let _stopped =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(stop));
                        }
                    }
                    for child in &mut children {
                        if let Some(join) = child.join.take() {
                            let _joined = join.join();
                        }
                    }
                    drop(guards);
                }
            });
        if spawned.is_err() {
            // The failure outcome is already non-green. Preserve custody
            // even under thread exhaustion; the process exit releases it.
            std::mem::forget(payload);
        }
    }

    /// Join only children that both reported and actually finished.
    /// A child still running after its event is escalated to the custody
    /// reaper, so a misleading event cannot bypass the hard deadline.
    /// A join that caught a panic reclassifies the child as panicked.
    fn join_children_with_known_exits(&mut self) -> DrainTally {
        let mut tally = DrainTally::default();
        for child in &mut self.children {
            match (
                child.kind,
                child.join.as_ref().is_none_or(JoinHandle::is_finished),
            ) {
                (Some(kind), true) => {
                    let joined = child.join.take().map_or(Ok(()), JoinHandle::join);
                    let kind = match (kind, joined) {
                        (_, Err(_panic_at_join)) => ChildExitKind::Panicked,
                        (kind, Ok(())) => kind,
                    };
                    if matches!(kind, ChildExitKind::Completed) {
                        tally.drained.push(child.name);
                    } else {
                        tally.failed.push((child.name, kind));
                    }
                }
                (None, _) | (Some(_), false) => tally.unfinished.push(child.name),
            }
        }
        tally
    }

    /// Move queued terminal events onto their children. Returns the
    /// first observed exit — any exit during serving is a loss.
    fn take_pending_exits(&mut self) -> Option<ChildExit> {
        let mut first: Option<ChildExit> = None;
        while let Some(exit) = self.pending.pop() {
            if first.is_none() {
                first = Some(exit);
            }
            let (name, kind) = (exit.name, exit.kind);
            for child in &mut self.children {
                if child.name == name {
                    child.kind = Some(kind);
                }
            }
        }
        first
    }

    /// Wait for one child's terminal event and actual thread completion
    /// until `deadline`, buffering other children's events for their own
    /// turn, then take and join its finished handle. `false` means the
    /// deadline passed with the child alive: its join transfers to the
    /// custody reaper, never this bounded wait.
    fn await_child(
        child: &mut RegisteredChild,
        pending: &mut Vec<ChildExit>,
        exits: &Receiver<ChildExit>,
        deadline: Instant,
        cancel: &CancelRoot,
    ) -> bool {
        loop {
            if cancel.abort_signum() != 0 {
                return false;
            }
            if !child.reports_exit && child.join.as_ref().is_some_and(JoinHandle::is_finished) {
                child.kind = Some(match child.join.take().map(JoinHandle::join) {
                    Some(Err(_panic)) => ChildExitKind::Panicked,
                    Some(Ok(())) | None => ChildExitKind::Completed,
                });
                return true;
            }
            if let Some(position) = pending.iter().position(|exit| exit.name == child.name) {
                let exit = pending.remove(position);
                child.kind = Some(exit.kind);
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let remaining = deadline
                .checked_duration_since(now)
                .unwrap_or(Duration::ZERO);
            match exits.recv_timeout(remaining.min(SUPERVISOR_POLL)) {
                Ok(exit) => {
                    pending.push(exit);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return false,
            }
        }
        while child.join.as_ref().is_some_and(|join| !join.is_finished()) {
            if cancel.abort_signum() != 0 {
                return false;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            std::thread::sleep(remaining.min(SUPERVISOR_POLL));
        }
        if let Some(join) = child.join.take()
            && join.join().is_err()
        {
            child.kind = Some(ChildExitKind::Panicked);
        }
        true
    }

    fn hard_deadline_at_now(&self) -> Instant {
        Instant::now()
            .checked_add(self.hard_deadline)
            .unwrap_or_else(Instant::now)
    }
}

impl DrainTally {
    /// Children that never finished are escalated; the empty list when
    /// the drain reached everyone.
    fn escalated(&self) -> Vec<&'static str> {
        self.unfinished.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{SupervisorPhase, SupervisorStatus};

    #[test]
    fn status_publishes_phase_and_required_child_loss() {
        let status = SupervisorStatus::default();
        assert_eq!(status.phase(), SupervisorPhase::Starting);
        status.set_child_running("query-accept", true);
        status.set_child_running("control-accept", true);
        status.set_child_running("ingest-accept", true);
        status.set_phase(SupervisorPhase::Ready);
        assert_eq!(status.required_planes(), (true, true, true));
        status.set_child_running("query-accept", false);
        assert_eq!(status.required_planes(), (false, true, true));
        status.set_phase(SupervisorPhase::Failed);
        assert_eq!(status.phase(), SupervisorPhase::Failed);
    }
}
