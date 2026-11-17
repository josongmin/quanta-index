//! One bounded private datagram exchange. Child exit is never a receipt.

use anyhow::{Result, anyhow, ensure};
use quanta_index_contract::native_cutover_evidence_v1 as wire;
use quanta_index_core::{
    RequestBudgetV1,
    index::{MemoryLifetime, WorkBudget, WorkUsage},
};
use std::{
    io::{ErrorKind, Write},
    os::unix::net::UnixDatagram,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use crate::deployment_selection::{
    DeploymentSelectionErrorV1 as Error, DeploymentSelectionPublishedV1,
};

const FRAME_BYTES: usize = wire::MAX_NATIVE_CUTOVER_CONTROL_BYTES_V1;
const POLL: Duration = Duration::from_millis(25);
const MONITOR_STACK: usize = 128 * 1024;

pub(super) fn receive_begin_v1(
    control: &UnixDatagram,
    request: &RequestBudgetV1,
) -> Result<(wire::NativeCutoverControlBeginV1, WorkUsage)> {
    // Config determines Control admission. Until then only FD3 and one bounded
    // frame are available. Charge successful bootstrap work on that ledger.
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(60))
        .ok_or_else(|| anyhow!("recovery Begin deadline overflow"))?;
    let request = request.capped_at(deadline);
    control.set_read_timeout(Some(POLL))?;
    let mut bytes = [0; FRAME_BYTES + 1];
    // Three FD3 operations plus at most 4096 receive attempts.
    for reads in 4..=4099 {
        request.checkpoint("receive recovery Begin")?;
        match control.recv(&mut bytes) {
            Ok(count) => {
                request.checkpoint("decode recovery Begin")?;
                ensure!(
                    count > 0 && count <= FRAME_BYTES,
                    "invalid recovery Begin frame size"
                );
                let begin = wire::decode_native_cutover_control_begin_v1(&bytes[..count])?;
                return Ok((
                    begin,
                    WorkUsage {
                        reads,
                        bytes: u64::try_from(bytes.len())?,
                        ..WorkUsage::default()
                    },
                ));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("recovery Begin exceeded its polling bound")
}

/// Convert the original shared-kernel monotonic expiry with a conservative
/// observation order. Reading Instant first cannot extend the parent's expiry.
#[cfg(target_os = "linux")]
pub(super) fn original_deadline_v1(deadline_monotonic_ns: u64) -> Result<Instant> {
    let local = Instant::now();
    let clock = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    let now = u64::try_from(clock.tv_sec)?
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(u64::try_from(clock.tv_nsec).ok()?))
        .ok_or_else(|| anyhow!("monotonic clock range overflow"))?;
    let remaining = deadline_monotonic_ns
        .checked_sub(now)
        .filter(|value| *value > 0)
        .ok_or_else(|| anyhow!("original recovery deadline expired"))?;
    local
        .checked_add(Duration::from_nanos(remaining))
        .ok_or_else(|| anyhow!("original recovery deadline range overflow"))
}

#[cfg(not(target_os = "linux"))]
pub(super) fn original_deadline_v1(_deadline: u64) -> Result<Instant> {
    anyhow::bail!("recovery requires the original shared Linux monotonic deadline")
}

pub(super) fn with_cancel_monitor_v1(
    control: &UnixDatagram,
    begin: &wire::NativeCutoverControlBeginV1,
    budget: &mut WorkBudget,
    execute: impl FnOnce(
        &mut WorkBudget,
        &AtomicBool,
    ) -> std::result::Result<DeploymentSelectionPublishedV1, Error>,
) -> std::result::Result<DeploymentSelectionPublishedV1, Error> {
    let memory = budget
        .reserve_memory(
            (MONITOR_STACK + 128 * 1024) as u64,
            MemoryLifetime::Temporary,
        )
        .map_err(|error| Error::RefusedBeforeRename(error.into()))?
        .ok_or_else(|| {
            Error::RefusedBeforeRename(anyhow!("control monitor requires admitted memory"))
        })?;
    let receiver = control
        .try_clone()
        .map_err(|error| Error::RefusedBeforeRename(error.into()))?;
    receiver
        .set_read_timeout(Some(POLL))
        .map_err(|error| Error::RefusedBeforeRename(error.into()))?;
    super::custody::charge(budget, 2, 0).map_err(Error::RefusedBeforeRename)?;
    let signal = Arc::new(AtomicBool::new(false));
    struct Signals {
        ids: [Option<signal_hook::SigId>; 2],
    }
    impl Drop for Signals {
        fn drop(&mut self) {
            for id in self.ids.iter_mut().filter_map(Option::take) {
                let _removed = signal_hook::low_level::unregister(id);
            }
        }
    }
    let mut signals = Signals { ids: [None, None] };
    for (slot, number) in signals
        .ids
        .iter_mut()
        .zip([signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM])
    {
        *slot = Some(
            signal_hook::flag::register(number, Arc::clone(&signal))
                .map_err(|error| Error::RefusedBeforeRename(error.into()))?,
        );
    }
    let request = budget.request().clone();
    // Prefund the finite control polling envelope on the SAME WorkBudget.
    // Each retry also sleeps POLL, so even spurious immediate socket timeouts
    // cannot exceed this bound. Include the nonblocking receive after join.
    let polls = request
        .remaining()
        .as_nanos()
        .checked_div(POLL.as_nanos())
        .and_then(|value| value.checked_add(3))
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| Error::RefusedBeforeRename(anyhow!("control polling envelope overflow")))?;
    super::custody::charge(budget, polls, (FRAME_BYTES + 1) as u64)
        .map_err(Error::RefusedBeforeRename)?;
    let cancelled = request.cancel_handle();
    let stop = AtomicBool::new(false);
    let failure = AtomicU8::new(0);
    let result = std::thread::scope(|scope| {
        let join = std::thread::Builder::new()
            .name("native-recovery-control".into())
            .stack_size(MONITOR_STACK)
            .spawn_scoped(scope, || {
                let mut bytes = [0; FRAME_BYTES + 1];
                while !stop.load(Ordering::Acquire) {
                    if signal.load(Ordering::Acquire) {
                        failure.store(5, Ordering::Release);
                        cancelled.cancel();
                        break;
                    }
                    if request.remaining().is_zero() {
                        failure.store(4, Ordering::Release);
                        cancelled.cancel();
                        break;
                    }
                    match receiver.recv(&mut bytes) {
                        Ok(count) => {
                            let valid = count > 0
                                && count <= FRAME_BYTES
                                && wire::decode_native_cutover_control_cancel_v1(&bytes[..count])
                                    .is_ok_and(|cancel| {
                                        cancel.operation_id == begin.operation_id
                                            && cancel.nonce == begin.nonce
                                    });
                            failure.store(if valid { 1 } else { 2 }, Ordering::Release);
                            cancelled.cancel();
                            break;
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                ErrorKind::WouldBlock | ErrorKind::TimedOut
                            ) =>
                        {
                            std::thread::sleep(POLL)
                        }
                        Err(_) => {
                            failure.store(3, Ordering::Release);
                            cancelled.cancel();
                            break;
                        }
                    }
                }
            })
            .map_err(|error| Error::RefusedBeforeRename(error.into()))?;
        let result = execute(budget, &signal);
        stop.store(true, Ordering::Release);
        let joined = join.join();
        let published = result?;
        if joined.is_err() || failure.load(Ordering::Acquire) != 0 || signal.load(Ordering::Acquire)
        {
            return Err(Error::PersistenceUncertain(anyhow!(
                "recovery completed publication while control cancellation/protocol outcome was unresolved: {:?}",
                published.after_sha256_v1()
            )));
        }
        let published = settle_published_v1(control, published)?;
        if signal.load(Ordering::Acquire) {
            return Err(Error::PersistenceUncertain(anyhow!(
                "recovery was interrupted while settling its published selection"
            )));
        }
        budget
            .checkpoint()
            .map(|()| published)
            .map_err(|error| Error::PersistenceUncertain(error.into()))
    });
    drop(signals);
    drop(memory);
    result
}

pub(crate) fn settle_published_v1(
    control: &UnixDatagram,
    published: DeploymentSelectionPublishedV1,
) -> std::result::Result<DeploymentSelectionPublishedV1, Error> {
    // The monitor can exit between polls with a datagram queued. After join
    // this is the sole receiver. Any frame, including an empty datagram,
    // makes success unsafe. The parent checks its Original again on reap.
    let mut bytes = [0; FRAME_BYTES + 1];
    match rustix::net::recv(control, &mut bytes[..], rustix::net::RecvFlags::DONTWAIT) {
        Err(rustix::io::Errno::AGAIN) => Ok(published),
        Err(error) => Err(Error::PersistenceUncertain(anyhow!(
            "read recovery control during publication settlement: {error}"
        ))),
        Ok(_) => Err(Error::PersistenceUncertain(anyhow!(
            "recovery control frame remained queued after publication: {:?}",
            published.after_sha256_v1()
        ))),
    }
}

pub(super) fn send_terminal_v1(
    control: &UnixDatagram,
    terminal: &wire::NativeCutoverControlTerminalV1,
) -> Result<()> {
    struct Bounded<'a> {
        remaining: &'a mut [u8],
        written: usize,
    }
    impl Write for Bounded<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.remaining.len() {
                return Err(std::io::Error::other(
                    "recovery terminal exceeds frame bound",
                ));
            }
            let remaining = std::mem::take(&mut self.remaining);
            let (target, rest) = remaining.split_at_mut(bytes.len());
            target.copy_from_slice(bytes);
            self.remaining = rest;
            self.written += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    terminal.validate_v1()?;
    let mut bytes = [0; FRAME_BYTES];
    let count = {
        let mut writer = Bounded {
            remaining: &mut bytes,
            written: 0,
        };
        ciborium::ser::into_writer(terminal, &mut writer)?;
        writer.written
    };
    // This is bounded terminal notification after the publication result;
    // request cancellation never suppresses its uncertainty report.
    control.set_write_timeout(Some(POLL))?;
    ensure!(
        control.send(&bytes[..count])? == count,
        "short recovery terminal datagram"
    );
    Ok(())
}
