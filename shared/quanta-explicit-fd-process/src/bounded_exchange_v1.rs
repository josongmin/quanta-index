//! Shared bounded parent pipe exchange. This is the original parser polling
//! algorithm over borrowed storage; wire framing and checkpoint authority stay
//! with each caller. The child value is owned until success or kill/reap on Drop.

use crate::ExplicitFdChildV1;
use rustix::event::{PollFd, PollFlags, Timespec};
use std::fs::File;
use std::io::{Read, Write};
use std::process::ExitStatus;
use std::time::{Duration, Instant};

/// These are the actual fixed stores used by the SAME polling loop. Descriptor
/// borrows end before mutable pipe I/O, while byte scratch spans the exchange.
struct ExchangeByteScratchV1 {
    continuation_v1: [u8; 64],
}

struct ExchangeStderrScratchV1([u8; 1024]);
struct ExchangeOverflowScratchV1([u8; 1]);
struct ExchangeReadinessV1([PollFlags; 3]);

struct ExchangePollScratchV1<'pipe> {
    fds_v1: [PollFd<'pipe>; 3],
    slots_v1: [Option<usize>; 3],
    pipes_v1: [Option<&'pipe File>; 3],
    timeout_v1: Timespec,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundedExchangePollErrorV1;

/// Implementors own the child and must kill/reap it on every early Drop. The
/// production implementation is the explicit-FD process owner below.
pub trait BoundedExchangeChildV1 {
    fn take_stdin_v1(&mut self) -> Option<File>;
    fn take_stdout_v1(&mut self) -> Option<File>;
    fn take_stderr_v1(&mut self) -> Option<File>;
    fn try_wait_v1(&mut self) -> Result<Option<ExitStatus>, BoundedExchangePollErrorV1>;
}

impl BoundedExchangeChildV1 for ExplicitFdChildV1 {
    fn take_stdin_v1(&mut self) -> Option<File> {
        ExplicitFdChildV1::take_stdin_v1(self)
    }
    fn take_stdout_v1(&mut self) -> Option<File> {
        ExplicitFdChildV1::take_stdout_v1(self)
    }
    fn take_stderr_v1(&mut self) -> Option<File> {
        ExplicitFdChildV1::take_stderr_v1(self)
    }
    fn try_wait_v1(&mut self) -> Result<Option<ExitStatus>, BoundedExchangePollErrorV1> {
        ExplicitFdChildV1::try_wait_v1(self).map_err(|_| BoundedExchangePollErrorV1)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum BoundedExchangeErrorV1<E> {
    Checkpoint(E),
    OutputAdmission(E),
    WallLimit,
    ChildFailed,
    Io,
    OutputLimit,
    StderrLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundedExchangeExitPolicyV1 {
    RequireSuccess,
    CollectStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundedExchangeOutcomeV1 {
    pub stdout_bytes_v1: usize,
    pub exit_status_v1: ExitStatus,
}

/// The caller owns output backing and its original-root grants. Admission and
/// native growth precede the append copy. This default uses a fixed stack read
/// chunk; it captures no output-sized buffer or independent allocation owner.
pub trait BoundedExchangeOutputV1<E> {
    fn append_chunk_v1(&mut self, chunk_v1: &[u8]) -> Result<(), E>;

    /// Copy a pending fixed protocol continuation only after its owner has
    /// admitted it. Zero means no continuation. This hook grants no authority;
    /// the native-work frame owner invokes its borrowed original control.
    fn write_continuation_v1(&mut self, _continuation_v1: &mut [u8]) -> Result<usize, E> {
        Ok(0)
    }

    /// Streaming frame owners may release consumed fixed credit metadata and
    /// report their actual retained output. Ordinary collectors retain the
    /// upstream cumulative-byte bound by returning None.
    fn retained_output_bytes_v1(&self) -> Option<usize> {
        None
    }

    /// Framed callers return true only after the canonical frame producer has
    /// consumed its complete response. One-shot callers keep the default.
    fn frame_complete_v1(&self) -> Result<bool, E> {
        Ok(false)
    }

    /// A frame owner can restrict a read to its remaining header/body span.
    /// This prevents transport from consuming the next frame's bytes.
    fn read_limit_v1(&self, remaining_v1: usize) -> Result<usize, E> {
        Ok(remaining_v1)
    }

    fn read_stdout_v1(
        &mut self,
        stdout_v1: &mut File,
        remaining_v1: usize,
    ) -> Result<std::io::Result<usize>, E> {
        let mut chunk_v1 = [0_u8; 8192];
        let limit_v1 = remaining_v1.min(chunk_v1.len());
        let read_v1 = stdout_v1.read(&mut chunk_v1[..limit_v1]);
        if let Ok(bytes_v1) = read_v1 {
            if bytes_v1 != 0 {
                self.append_chunk_v1(&chunk_v1[..bytes_v1])?;
            }
        }
        Ok(read_v1)
    }
}

struct FixedOutputV1<'out> {
    output_v1: &'out mut [u8],
    received_v1: usize,
}
impl<E> BoundedExchangeOutputV1<E> for FixedOutputV1<'_> {
    fn append_chunk_v1(&mut self, chunk_v1: &[u8]) -> Result<(), E> {
        let end_v1 = self.received_v1 + chunk_v1.len();
        self.output_v1[self.received_v1..end_v1].copy_from_slice(chunk_v1);
        self.received_v1 = end_v1;
        Ok(())
    }

    fn read_stdout_v1(
        &mut self,
        stdout_v1: &mut File,
        remaining_v1: usize,
    ) -> Result<std::io::Result<usize>, E> {
        // Preserve the original parser's direct borrowed-slice read shape.
        let read_v1 =
            stdout_v1.read(&mut self.output_v1[self.received_v1..self.received_v1 + remaining_v1]);
        if let Ok(bytes_v1) = read_v1 {
            self.received_v1 += bytes_v1;
        }
        Ok(read_v1)
    }
}

fn nonblocking_parent_pipe_v1<E>(file_v1: &std::fs::File) -> Result<(), BoundedExchangeErrorV1<E>> {
    let flags_v1 = rustix::fs::fcntl_getfl(file_v1).map_err(|_| BoundedExchangeErrorV1::Io)?;
    rustix::fs::fcntl_setfl(file_v1, flags_v1 | rustix::fs::OFlags::NONBLOCK)
        .map_err(|_| BoundedExchangeErrorV1::Io)
}

fn discard_bounded_stderr_v1<E>(
    stderr_v1: &mut std::fs::File,
    received_v1: &mut usize,
    stderr_limit_v1: usize,
) -> Result<bool, BoundedExchangeErrorV1<E>> {
    let mut scratch_v1 = ExchangeStderrScratchV1([0_u8; 1024]);
    let remaining_v1 = stderr_limit_v1 - *received_v1;
    let attempt_v1 = scratch_v1.0.len().min(remaining_v1.saturating_add(1));
    match stderr_v1.read(&mut scratch_v1.0[..attempt_v1]) {
        Ok(0) => Ok(true),
        Ok(bytes_v1) if bytes_v1 > remaining_v1 => Err(BoundedExchangeErrorV1::StderrLimit),
        Ok(bytes_v1) => {
            *received_v1 += bytes_v1;
            Ok(false)
        }
        Err(error_v1) if error_v1.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
        Err(error_v1) if error_v1.kind() == std::io::ErrorKind::Interrupted => Ok(false),
        Err(_) => Err(BoundedExchangeErrorV1::Io),
    }
}

/// Three fixed pipe descriptors and borrowed input slices. No transport heap
/// allocation or additional thread is born after the child permit is issued.
pub fn poll_bounded_exchange_v1<C: BoundedExchangeChildV1, E>(
    child_v1: C,
    parts_v1: &[&[u8]],
    output_v1: &mut [u8],
    checkpoint_v1: impl FnMut() -> Result<(), E>,
    wall_limit_v1: Duration,
    stderr_limit_v1: usize,
) -> Result<usize, BoundedExchangeErrorV1<E>> {
    poll_bounded_exchange_outcome_v1(
        child_v1,
        parts_v1,
        output_v1,
        checkpoint_v1,
        wall_limit_v1,
        stderr_limit_v1,
        BoundedExchangeExitPolicyV1::RequireSuccess,
    )
    .map(|outcome_v1| outcome_v1.stdout_bytes_v1)
}

/// The same exchange algorithm with an explicit exit contract. CollectStatus
/// drains bounded pipes before returning the actual status for caller-owned
/// command semantics (for example Git's unborn-HEAD exit). It does not convert
/// that status into success or suppress transport/checkpoint refusals.
pub(crate) fn poll_bounded_exchange_outcome_v1<C: BoundedExchangeChildV1, E>(
    child_v1: C,
    parts_v1: &[&[u8]],
    output_v1: &mut [u8],
    checkpoint_v1: impl FnMut() -> Result<(), E>,
    wall_limit_v1: Duration,
    stderr_limit_v1: usize,
    exit_policy_v1: BoundedExchangeExitPolicyV1,
) -> Result<BoundedExchangeOutcomeV1, BoundedExchangeErrorV1<E>> {
    let output_limit_v1 = output_v1.len();
    let mut output_v1 = FixedOutputV1 {
        output_v1,
        received_v1: 0,
    };
    poll_bounded_exchange_output_v1(
        child_v1,
        parts_v1,
        &mut output_v1,
        output_limit_v1,
        checkpoint_v1,
        Some(wall_limit_v1),
        stderr_limit_v1,
        exit_policy_v1,
    )
}

/// One polling algorithm for fixed parser output and admitted caller-owned
/// collectors. The installed cap is enforced before each append and by an
/// independent one-byte stack overflow probe after the exact cap is reached.
/// None adds no relative wall cap; the caller's checkpoint retains authority
/// over the original absolute deadline across successive subprocesses.
#[allow(
    clippy::too_many_arguments,
    reason = "explicit borrowed storage, lifecycle and exit contracts"
)]
pub fn poll_bounded_exchange_output_v1<
    C: BoundedExchangeChildV1,
    O: BoundedExchangeOutputV1<E>,
    E,
>(
    child_v1: C,
    parts_v1: &[&[u8]],
    output_v1: &mut O,
    output_limit_v1: usize,
    checkpoint_v1: impl FnMut() -> Result<(), E>,
    wall_limit_v1: Option<Duration>,
    stderr_limit_v1: usize,
    exit_policy_v1: BoundedExchangeExitPolicyV1,
) -> Result<BoundedExchangeOutcomeV1, BoundedExchangeErrorV1<E>> {
    let mut session_v1 = BoundedExchangeSessionV1::try_new_v1(child_v1, stderr_limit_v1)?;
    match session_v1.poll_loop_v1(
        parts_v1,
        output_v1,
        output_limit_v1,
        checkpoint_v1,
        wall_limit_v1,
        ExchangeCompletionV1::Exit(exit_policy_v1),
    )? {
        ExchangeCompletionOutcomeV1::Exit(outcome_v1) => Ok(outcome_v1),
        ExchangeCompletionOutcomeV1::Frame(_) => Err(BoundedExchangeErrorV1::Io),
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ExchangeCompletionV1 {
    Exit(BoundedExchangeExitPolicyV1),
    Frame,
}
enum ExchangeCompletionOutcomeV1 {
    Exit(BoundedExchangeOutcomeV1),
    Frame(usize),
}

/// One child and its three pipes retained across sequential frame exchanges.
/// No heap backing, independent deadline, or additional polling algorithm is
/// introduced. The caller retains its original-root child permit through Drop.
/// Every failed exchange poisons the session and synchronously drops/reaps the
/// child before returning the first transport/admission/checkpoint error.
pub struct BoundedExchangeSessionV1<C: BoundedExchangeChildV1> {
    child_v1: Option<C>,
    stdin_v1: Option<File>,
    stdout_v1: File,
    stderr_v1: File,
    stderr_received_v1: usize,
    stderr_limit_v1: usize,
    eof_v1: bool,
    stderr_eof_v1: bool,
    status_v1: Option<ExitStatus>,
}
impl<C: BoundedExchangeChildV1> BoundedExchangeSessionV1<C> {
    /// Fixed parent backing only; caller output storage has its own owner.
    /// Layout overflow stays an admission failure at the caller boundary.
    pub fn fixed_poll_storage_layout_v1() -> Result<std::alloc::Layout, std::alloc::LayoutError> {
        let mut layout_v1 = std::alloc::Layout::new::<ExchangeByteScratchV1>();
        for backing_v1 in [
            std::alloc::Layout::new::<ExchangePollScratchV1<'static>>(),
            std::alloc::Layout::new::<ExchangeStderrScratchV1>(),
            std::alloc::Layout::new::<ExchangeOverflowScratchV1>(),
            std::alloc::Layout::new::<ExchangeReadinessV1>(),
        ] {
            layout_v1 = layout_v1.extend(backing_v1)?.0;
        }
        Ok(layout_v1.pad_to_align())
    }

    pub fn try_new_v1<E>(
        mut child_v1: C,
        stderr_limit_v1: usize,
    ) -> Result<Self, BoundedExchangeErrorV1<E>> {
        let stdin_v1 = child_v1.take_stdin_v1().ok_or(BoundedExchangeErrorV1::Io)?;
        let stdout_v1 = child_v1
            .take_stdout_v1()
            .ok_or(BoundedExchangeErrorV1::Io)?;
        let stderr_v1 = child_v1
            .take_stderr_v1()
            .ok_or(BoundedExchangeErrorV1::Io)?;
        nonblocking_parent_pipe_v1(&stdin_v1)?;
        nonblocking_parent_pipe_v1(&stdout_v1)?;
        nonblocking_parent_pipe_v1(&stderr_v1)?;
        Ok(Self {
            child_v1: Some(child_v1),
            stdin_v1: Some(stdin_v1),
            stdout_v1,
            stderr_v1,
            stderr_received_v1: 0,
            stderr_limit_v1,
            eof_v1: false,
            stderr_eof_v1: false,
            status_v1: None,
        })
    }

    pub fn exchange_frame_v1<O: BoundedExchangeOutputV1<E>, E>(
        &mut self,
        parts_v1: &[&[u8]],
        output_v1: &mut O,
        output_limit_v1: usize,
        checkpoint_v1: impl FnMut() -> Result<(), E>,
        wall_limit_v1: Option<Duration>,
    ) -> Result<usize, BoundedExchangeErrorV1<E>> {
        let result_v1 = self.poll_loop_v1(
            parts_v1,
            output_v1,
            output_limit_v1,
            checkpoint_v1,
            wall_limit_v1,
            ExchangeCompletionV1::Frame,
        );
        match result_v1 {
            Ok(ExchangeCompletionOutcomeV1::Frame(bytes_v1)) => Ok(bytes_v1),
            Ok(ExchangeCompletionOutcomeV1::Exit(_)) => {
                self.stdin_v1 = None;
                self.child_v1 = None;
                Err(BoundedExchangeErrorV1::ChildFailed)
            }
            Err(error_v1) => {
                self.stdin_v1 = None;
                self.child_v1 = None;
                Err(error_v1)
            }
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "borrowed storage and original lifecycle"
    )]
    fn poll_loop_v1<O: BoundedExchangeOutputV1<E>, E>(
        &mut self,
        parts_v1: &[&[u8]],
        output_v1: &mut O,
        output_limit_v1: usize,
        mut checkpoint_v1: impl FnMut() -> Result<(), E>,
        wall_limit_v1: Option<Duration>,
        completion_v1: ExchangeCompletionV1,
    ) -> Result<ExchangeCompletionOutcomeV1, BoundedExchangeErrorV1<E>> {
        let mut part_v1 = 0_usize;
        let mut written_v1 = 0_usize;
        let mut received_v1 = 0_usize;
        let mut input_complete_v1 =
            completion_v1 == ExchangeCompletionV1::Frame && parts_v1.is_empty();
        let mut byte_scratch_v1 = ExchangeByteScratchV1 {
            continuation_v1: [0_u8; 64],
        };
        let mut continuation_length_v1 = 0;
        let mut continuation_written_v1 = 0;
        let started_v1 = Instant::now();
        loop {
            checkpoint_v1().map_err(BoundedExchangeErrorV1::Checkpoint)?;
            if wall_limit_v1.is_some_and(|limit_v1| started_v1.elapsed() >= limit_v1) {
                return Err(BoundedExchangeErrorV1::WallLimit);
            }
            if self.status_v1.is_none() {
                self.status_v1 = self
                    .child_v1
                    .as_mut()
                    .ok_or(BoundedExchangeErrorV1::ChildFailed)?
                    .try_wait_v1()
                    .map_err(|_| BoundedExchangeErrorV1::Io)?;
            }
            if completion_v1
                == ExchangeCompletionV1::Exit(BoundedExchangeExitPolicyV1::RequireSuccess)
                && self
                    .status_v1
                    .as_ref()
                    .is_some_and(|status_v1| !status_v1.success())
            {
                return Err(BoundedExchangeErrorV1::ChildFailed);
            }
            if completion_v1 == ExchangeCompletionV1::Frame && input_complete_v1 {
                if self.eof_v1 || self.status_v1.is_some() {
                    return Err(BoundedExchangeErrorV1::ChildFailed);
                }
                if continuation_written_v1 == continuation_length_v1 {
                    continuation_length_v1 = output_v1
                        .write_continuation_v1(&mut byte_scratch_v1.continuation_v1)
                        .map_err(BoundedExchangeErrorV1::OutputAdmission)?;
                    if continuation_length_v1 > byte_scratch_v1.continuation_v1.len() {
                        return Err(BoundedExchangeErrorV1::Io);
                    }
                    continuation_written_v1 = 0;
                }
                if continuation_written_v1 == continuation_length_v1
                    && output_v1
                        .frame_complete_v1()
                        .map_err(BoundedExchangeErrorV1::OutputAdmission)?
                {
                    return Ok(ExchangeCompletionOutcomeV1::Frame(received_v1));
                }
            }
            if self.eof_v1 && self.stderr_eof_v1 && input_complete_v1 {
                if let Some(exit_status_v1) = self.status_v1 {
                    return Ok(ExchangeCompletionOutcomeV1::Exit(
                        BoundedExchangeOutcomeV1 {
                            stdout_bytes_v1: received_v1,
                            exit_status_v1,
                        },
                    ));
                }
            }
            // Poll only live pipe ends in a fixed stack prefix. Return copied
            // readiness flags so borrowed descriptors end before mutable I/O.
            let ready_v1 = ExchangeReadinessV1({
                let mut scratch_v1 = ExchangePollScratchV1 {
                    fds_v1: std::array::from_fn(|_| {
                        PollFd::new(&self.stdout_v1, PollFlags::empty())
                    }),
                    slots_v1: [None; 3],
                    pipes_v1: [
                        self.stdin_v1.as_ref().filter(|_| {
                            !input_complete_v1 || continuation_written_v1 < continuation_length_v1
                        }),
                        (!self.eof_v1).then_some(&self.stdout_v1),
                        (!self.stderr_eof_v1).then_some(&self.stderr_v1),
                    ],
                    timeout_v1: Timespec {
                        tv_sec: 0,
                        tv_nsec: 5_000_000,
                    },
                };
                let mut active_v1 = 0;
                for (pipe_v1, file_v1) in scratch_v1.pipes_v1.into_iter().enumerate() {
                    if let Some(file_v1) = file_v1 {
                        let events_v1 = if pipe_v1 == 0 {
                            PollFlags::OUT
                        } else {
                            PollFlags::IN
                        };
                        scratch_v1.fds_v1[active_v1] = PollFd::new(file_v1, events_v1);
                        scratch_v1.slots_v1[pipe_v1] = Some(active_v1);
                        active_v1 += 1;
                    }
                }
                match rustix::event::poll(
                    &mut scratch_v1.fds_v1[..active_v1],
                    Some(&scratch_v1.timeout_v1),
                ) {
                    Ok(_) => {}
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(_) => return Err(BoundedExchangeErrorV1::Io),
                }
                scratch_v1.slots_v1.map(|slot_v1| {
                    slot_v1.map_or(PollFlags::empty(), |index_v1| {
                        scratch_v1.fds_v1[index_v1].revents()
                    })
                })
            });
            let ready_v1 = &ready_v1.0;
            if ready_v1
                .iter()
                .any(|flags_v1| flags_v1.contains(PollFlags::NVAL))
            {
                return Err(BoundedExchangeErrorV1::Io);
            }
            if ready_v1[0].intersects(PollFlags::OUT | PollFlags::ERR | PollFlags::HUP) {
                while part_v1 < parts_v1.len() && written_v1 == parts_v1[part_v1].len() {
                    part_v1 += 1;
                    written_v1 = 0;
                }
                if part_v1 == parts_v1.len() {
                    input_complete_v1 = true;
                    if matches!(completion_v1, ExchangeCompletionV1::Exit(_)) {
                        self.stdin_v1 = None;
                    }
                }
                let continuing_v1 =
                    input_complete_v1 && continuation_written_v1 < continuation_length_v1;
                let write_span_v1 = if continuing_v1 {
                    Some(
                        &byte_scratch_v1.continuation_v1
                            [continuation_written_v1..continuation_length_v1],
                    )
                } else if !input_complete_v1 {
                    Some(&parts_v1[part_v1][written_v1..])
                } else {
                    None
                };
                if let (Some(stdin_v1), Some(write_span_v1)) =
                    (self.stdin_v1.as_mut(), write_span_v1)
                {
                    match stdin_v1.write(write_span_v1) {
                        Ok(0) => return Err(BoundedExchangeErrorV1::Io),
                        Ok(bytes_v1) if continuing_v1 => continuation_written_v1 += bytes_v1,
                        Ok(bytes_v1) => written_v1 += bytes_v1,
                        Err(error_v1) if error_v1.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(error_v1) if error_v1.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => return Err(BoundedExchangeErrorV1::Io),
                    }
                }
            }
            if !self.eof_v1
                && ready_v1[1].intersects(PollFlags::IN | PollFlags::ERR | PollFlags::HUP)
            {
                let mut overflow_v1 = ExchangeOverflowScratchV1([0_u8; 1]);
                let retained_v1 = output_v1.retained_output_bytes_v1().unwrap_or(received_v1);
                if retained_v1 > output_limit_v1 {
                    return Err(BoundedExchangeErrorV1::OutputLimit);
                }
                let overflow_mode_v1 = retained_v1 == output_limit_v1;
                let read_v1 = if overflow_mode_v1 {
                    self.stdout_v1.read(&mut overflow_v1.0)
                } else {
                    let remaining_v1 = output_limit_v1 - retained_v1;
                    let read_limit_v1 = output_v1
                        .read_limit_v1(remaining_v1)
                        .map_err(BoundedExchangeErrorV1::OutputAdmission)?;
                    if read_limit_v1 == 0 || read_limit_v1 > remaining_v1 {
                        return Err(BoundedExchangeErrorV1::OutputLimit);
                    }
                    output_v1
                        .read_stdout_v1(&mut self.stdout_v1, read_limit_v1)
                        .map_err(BoundedExchangeErrorV1::OutputAdmission)?
                };
                match read_v1 {
                    Ok(0) => self.eof_v1 = true,
                    Ok(_) if overflow_mode_v1 => return Err(BoundedExchangeErrorV1::OutputLimit),
                    Ok(bytes_v1) if bytes_v1 > output_limit_v1 - retained_v1 => {
                        return Err(BoundedExchangeErrorV1::OutputLimit);
                    }
                    Ok(bytes_v1) => {
                        received_v1 = received_v1
                            .checked_add(bytes_v1)
                            .ok_or(BoundedExchangeErrorV1::OutputLimit)?;
                    }
                    Err(error_v1) if error_v1.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error_v1) if error_v1.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return Err(BoundedExchangeErrorV1::Io),
                }
            }
            if !self.stderr_eof_v1
                && ready_v1[2].intersects(PollFlags::IN | PollFlags::ERR | PollFlags::HUP)
            {
                self.stderr_eof_v1 = discard_bounded_stderr_v1(
                    &mut self.stderr_v1,
                    &mut self.stderr_received_v1,
                    self.stderr_limit_v1,
                )?;
            }
        }
    }
}

#[cfg(test)]
mod tests;
