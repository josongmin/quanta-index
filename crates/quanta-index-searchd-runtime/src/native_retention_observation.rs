//! Retention progress the maintenance pump publishes, observable without timing.
//!
//! Retention has two goals a waiter can name: a completed turn of some scope,
//! and a settled release backlog. Both run on the shared progress state
//! machine and share one structural stall rule.

use crate::native_progress_observation::{
    NativeProgressClose, NativeProgressKind, NativeProgressMark, NativeProgressObservation,
    NativeProgressRound, NativeProgressWaitError,
};
use crate::native_retention::NativeRetentionRound;

/// Consecutive retention passes without a completed turn that prove
/// retention is stuck.
///
/// A pass gives every configured scope one turn. A turn that ran and did
/// not complete observed its scope: its floor or page step was refused, for
/// the scope's own state or for the round's budget. One pass with no
/// completed turn can be one episode of budget pressure overlapping each of
/// its turns; the next pass tries every scope again, so a second consecutive
/// pass without a completed turn means no configured scope could complete
/// across two full rotations. Two is the smallest count a single episode
/// cannot reach. A round the maintenance scheduler refuses runs no turn and
/// records nothing: it reflects whoever else holds the maintenance class,
/// not any scope.
pub const NATIVE_RETENTION_STALL_PASSES: u32 = 2;

/// Retention turns: the goal is a completed turn of any scope.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeRetentionTurns;

impl NativeProgressKind for NativeRetentionTurns {
    const STALL_ROUNDS: Option<u32> = Some(NATIVE_RETENTION_STALL_PASSES);
}

/// Retention release demand: the goal is a round after which no committed
/// release range waits for its scope's release turn.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeRetentionReleases;

impl NativeProgressKind for NativeRetentionReleases {
    const STALL_ROUNDS: Option<u32> = Some(NATIVE_RETENTION_STALL_PASSES);
}

/// A point in the pump's retention history; see [`NativeRetentionObservation::mark`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeRetentionMark {
    turns: NativeProgressMark<NativeRetentionTurns>,
    releases: NativeProgressMark<NativeRetentionReleases>,
}

/// Retention rounds as the pump records them.
///
/// The pump records every round that ran a turn and closes the observation
/// when it stops or has no retention driver. Waiters block on these events,
/// never on elapsed time.
#[derive(Debug, Default)]
pub struct NativeRetentionObservation {
    turns: NativeProgressObservation<NativeRetentionTurns>,
    releases: NativeProgressObservation<NativeRetentionReleases>,
}

impl NativeRetentionObservation {
    /// Record one round that ran a scope's turn. A completed turn is
    /// progress for both goals; a pass that ended without one is a stalled
    /// round for both.
    pub(crate) fn record_round(&self, round: &NativeRetentionRound) {
        self.turns.record(NativeProgressRound {
            reached: round.completed,
            progressed: round.completed,
            stalled: round.stalled_pass,
        });
        self.releases.record(NativeProgressRound {
            reached: !round.releases_pending,
            progressed: round.completed,
            stalled: round.stalled_pass,
        });
    }

    /// The current point in the retention history, to wait from.
    #[must_use]
    pub fn mark(&self) -> NativeRetentionMark {
        NativeRetentionMark {
            turns: self.turns.mark(),
            releases: self.releases.mark(),
        }
    }

    /// Whether waiting for completed turns from `mark` has finished.
    #[cfg(test)]
    pub(crate) fn completed_outcome_after(
        &self,
        mark: NativeRetentionMark,
    ) -> Option<Result<(), NativeProgressWaitError>> {
        self.turns.outcome_after(mark.turns)
    }

    /// Whether waiting for settled releases from `mark` has finished.
    #[cfg(test)]
    pub(crate) fn released_outcome_after(
        &self,
        mark: NativeRetentionMark,
    ) -> Option<Result<(), NativeProgressWaitError>> {
        self.releases.outcome_after(mark.releases)
    }

    /// Block until two retention turns complete after `mark`.
    ///
    /// The second began after the first was recorded, so it ran after every
    /// commit made before `mark`. With one configured scope that is a turn of
    /// that scope, whose floor step applies the committed head. With several
    /// scopes the turns may belong to any of them: to wait for a released
    /// view, use [`Self::wait_released_after`].
    ///
    /// # Errors
    ///
    /// `Stalled` once [`NATIVE_RETENTION_STALL_PASSES`] consecutive passes,
    /// the latest recorded after `mark`, completed no turn; `Stopped` when the
    /// pump stops first.
    pub fn wait_completed_after(
        &self,
        mark: NativeRetentionMark,
    ) -> Result<(), NativeProgressWaitError> {
        self.turns.wait_reached_after(mark.turns)
    }

    /// Block until every release committed before `mark` has been served.
    ///
    /// Take `mark` after a read-session release commits. The first round
    /// recorded after it with no release waiting may have run before the pump
    /// took that release's demand. The second began after the first was
    /// recorded, after the pump took the demand, so every released range
    /// has been retired by its scope's release turn, or was cleared because
    /// the released epoch is no longer the scope's current epoch.
    ///
    /// # Errors
    ///
    /// `Stalled` once [`NATIVE_RETENTION_STALL_PASSES`] consecutive passes,
    /// the latest recorded after `mark`, completed no turn; `Stopped` when the
    /// pump stops first.
    pub fn wait_released_after(
        &self,
        mark: NativeRetentionMark,
    ) -> Result<(), NativeProgressWaitError> {
        self.releases.wait_reached_after(mark.releases)
    }
}

impl NativeProgressClose for NativeRetentionObservation {
    fn close(&self) {
        self.turns.close();
        self.releases.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_progress_observation::CloseOnExit;

    const fn round(
        completed: bool,
        stalled_pass: bool,
        releases_pending: bool,
    ) -> NativeRetentionRound {
        NativeRetentionRound {
            completed,
            stalled_pass,
            releases_pending,
        }
    }

    #[test]
    fn two_completed_turns_after_the_mark_finish_the_turn_wait() {
        let observation = NativeRetentionObservation::default();
        observation.record_round(&round(true, false, false));
        let mark = observation.mark();
        observation.record_round(&round(false, false, false));
        observation.record_round(&round(true, false, false));
        // The first completed turn after the mark may have begun before it.
        assert_eq!(observation.completed_outcome_after(mark), None);
        observation.record_round(&round(true, false, false));
        assert_eq!(observation.completed_outcome_after(mark), Some(Ok(())));
        assert_eq!(observation.wait_completed_after(mark), Ok(()));
    }

    /// The release wait needs two rounds after the mark with no release
    /// waiting; rounds that still hold a release do not count.
    #[test]
    fn two_settled_rounds_after_the_mark_finish_the_release_wait() {
        let observation = NativeRetentionObservation::default();
        let mark = observation.mark();
        observation.record_round(&round(true, false, true));
        observation.record_round(&round(false, false, true));
        observation.record_round(&round(true, false, false));
        assert_eq!(observation.released_outcome_after(mark), None);
        observation.record_round(&round(false, false, false));
        assert_eq!(observation.released_outcome_after(mark), Some(Ok(())));
        assert_eq!(observation.wait_released_after(mark), Ok(()));
    }

    /// Failed turns alone never end a wait; two consecutive passes without a
    /// completed turn, the latest after the mark, are a typed stall for both
    /// goals, and a completed turn between them resets the count.
    #[test]
    fn two_passes_without_a_completed_turn_stall_both_waits() {
        let observation = NativeRetentionObservation::default();
        observation.record_round(&round(false, true, true));
        let mark = observation.mark();
        for _ in 0..4 {
            observation.record_round(&round(false, false, true));
        }
        observation.record_round(&round(true, false, true));
        observation.record_round(&round(false, true, true));
        assert_eq!(observation.completed_outcome_after(mark), None);
        assert_eq!(observation.released_outcome_after(mark), None);
        observation.record_round(&round(false, false, true));
        observation.record_round(&round(false, true, true));
        let stalled = Some(Err(NativeProgressWaitError::Stalled { rounds: 3 }));
        assert_eq!(observation.completed_outcome_after(mark), stalled);
        assert_eq!(observation.released_outcome_after(mark), stalled);
    }

    #[test]
    fn a_closed_observation_stops_both_waits() {
        let observation = NativeRetentionObservation::default();
        let mark = observation.mark();
        {
            let _close = CloseOnExit(&observation);
        }
        assert_eq!(
            observation.wait_completed_after(mark),
            Err(NativeProgressWaitError::Stopped)
        );
        assert_eq!(
            observation.wait_released_after(mark),
            Err(NativeProgressWaitError::Stopped)
        );
    }
}
