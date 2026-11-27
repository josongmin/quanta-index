//! Reclaim progress the maintenance pump publishes, observable without timing.

use crate::native_progress_observation::{
    NativeProgressKind, NativeProgressMark, NativeProgressObservation, NativeProgressRound,
    NativeProgressWaitError,
};
use crate::native_reclaim_driver::NativeReclaimRound;

/// Consecutive stalled passes that prove the reclaim queue is stuck.
///
/// A stalled pass sweeps the queue from its head to its end and every key it
/// examines is contended, so nothing is reclaimed. Custody contention is
/// transient: one contention episode on a key fails one attempt, and the
/// sweep moves past that key instead of retrying it. One stalled pass can
/// therefore be a single episode on each key it met, even when more contended
/// keys than one round's steps keep that pass going for several rounds. The
/// next pass attempts every key again, so a second consecutive stalled pass
/// means some key stayed contended across two complete sweeps while nothing
/// else could be reclaimed. Two is the smallest count a single contention
/// episode cannot reach.
pub const NATIVE_RECLAIM_STALL_PASSES: u32 = 2;

/// Automatic reclaim rounds: the goal is a round that finds the queue empty.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeReclaimProgress;

impl NativeProgressKind for NativeReclaimProgress {
    const STALL_ROUNDS: Option<u32> = Some(NATIVE_RECLAIM_STALL_PASSES);
}

/// Reclaim rounds as the pump completes them.
///
/// # Refused rounds
///
/// A round the maintenance scheduler refuses for resources runs no step and
/// records nothing, so a wait stays pending through refusals. No count of
/// refusals can prove the queue stuck: a refusal reflects whoever else holds
/// the maintenance class or the process budget at that moment (a retention
/// round, read-session recovery, owned work or foreground pressure), not the
/// queue, and each holder releases on its own schedule. Only stalled passes,
/// which do observe the queue, end a wait early.
pub type NativeReclaimObservation = NativeProgressObservation<NativeReclaimProgress>;
/// A point in the pump's reclaim history.
pub type NativeReclaimMark = NativeProgressMark<NativeReclaimProgress>;

impl NativeProgressObservation<NativeReclaimProgress> {
    /// Record one completed reclaim round. A round that applied work or
    /// found the queue empty made progress; a completed stalled pass is a
    /// stalled round.
    pub(crate) fn record_round(&self, round: &NativeReclaimRound) {
        self.record(NativeProgressRound {
            reached: round.queue_empty,
            progressed: round.queue_empty || round.applied > 0,
            stalled: round.stalled_pass,
        });
    }

    /// Block until the pump observes the reclaim queue empty after `mark`.
    ///
    /// Take `mark` after the commit whose queued work must be reclaimed. The
    /// first drained round recorded after it may have read the queue before
    /// that commit. The second began after the first was recorded, so its
    /// empty queue includes the commit.
    ///
    /// # Errors
    ///
    /// `Stalled` once [`NATIVE_RECLAIM_STALL_PASSES`] consecutive passes, the
    /// latest recorded after `mark`, reclaimed nothing; `Stopped` when the
    /// pump stops first.
    pub fn wait_drained_after(
        &self,
        mark: NativeReclaimMark,
    ) -> Result<(), NativeProgressWaitError> {
        self.wait_reached_after(mark)
    }
}
