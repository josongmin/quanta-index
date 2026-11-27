//! Maintenance progress the pump publishes, observable without timing.
//!
//! Reclaim and retention are both rounds the pump drives on its own cadence.
//! A waiter marks a point in one kind's history and blocks until rounds
//! recorded after it reach that kind's goal, prove the kind stuck, or the
//! pump stops. Each kind decides only what one round means: whether it
//! reached the goal, whether it made progress, and whether it was stalled.
//! The counting, the stall rule and the wait are this one state machine.

use std::marker::PhantomData;
use std::sync::{Condvar, Mutex, MutexGuard};

/// One kind of maintenance round and the stall bound that proves it stuck.
pub trait NativeProgressKind {
    /// Consecutive stalled rounds, the latest after the mark, that end a
    /// wait with [`NativeProgressWaitError::Stalled`]. Each kind documents
    /// why its bound is the smallest a transient episode cannot reach. None
    /// for a kind whose rounds observe nothing that could prove it stuck.
    const STALL_ROUNDS: Option<u32>;
}

/// What one completed round of a kind showed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NativeProgressRound {
    /// The round observed the state a waiter waits for.
    pub(crate) reached: bool,
    /// The round did work, or had none left; it ends a stall run.
    pub(crate) progressed: bool,
    /// The round counts toward the kind's stall bound.
    pub(crate) stalled: bool,
}

/// Why waiting for maintenance rounds ended without reaching the goal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeProgressWaitError {
    /// The kind's stall bound of consecutive stalled rounds was reached, the
    /// latest after the mark. `rounds` counts the rounds since the last one
    /// that made progress.
    Stalled { rounds: u64 },
    /// The pump stopped, or the service has no driver of this kind.
    Stopped,
}

impl std::fmt::Display for NativeProgressWaitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stalled { rounds } => write!(
                formatter,
                "native maintenance stalled: {rounds} rounds made no progress"
            ),
            Self::Stopped => formatter.write_str("native maintenance pump stopped"),
        }
    }
}

impl std::error::Error for NativeProgressWaitError {}

/// A point in one kind's history; see [`NativeProgressObservation::mark`].
pub struct NativeProgressMark<K> {
    reached: u64,
    rounds: u64,
    kind: PhantomData<fn() -> K>,
}

impl<K> Clone for NativeProgressMark<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for NativeProgressMark<K> {}

impl<K> std::fmt::Debug for NativeProgressMark<K> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeProgressMark")
            .field("reached", &self.reached)
            .field("rounds", &self.rounds)
            .finish()
    }
}

impl<K> PartialEq for NativeProgressMark<K> {
    fn eq(&self, other: &Self) -> bool {
        (self.reached, self.rounds) == (other.reached, other.rounds)
    }
}

impl<K> Eq for NativeProgressMark<K> {}

/// Rounds of one kind as the pump records them.
///
/// The pump records every round of the kind and closes the observation when
/// it stops or has no driver of the kind. Waiters block on these events,
/// never on elapsed time.
pub struct NativeProgressObservation<K> {
    state: Mutex<Progress>,
    changed: Condvar,
    kind: PhantomData<fn() -> K>,
}

impl<K> Default for NativeProgressObservation<K> {
    fn default() -> Self {
        Self {
            state: Mutex::new(Progress::default()),
            changed: Condvar::new(),
            kind: PhantomData,
        }
    }
}

impl<K> std::fmt::Debug for NativeProgressObservation<K> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeProgressObservation")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct Progress {
    rounds: u64,
    reached: u64,
    /// Rounds since the last that made progress.
    unprogressed_rounds: u64,
    /// Consecutive stalled rounds since then.
    stalled_rounds: u32,
    /// The round that recorded the latest stalled round.
    last_stall_round: u64,
    closed: bool,
}

impl Progress {
    fn outcome<K: NativeProgressKind>(
        &self,
        mark: NativeProgressMark<K>,
    ) -> Option<Result<(), NativeProgressWaitError>> {
        if self.reached >= mark.reached.saturating_add(2) {
            Some(Ok(()))
        } else if K::STALL_ROUNDS.is_some_and(|bound| self.stalled_rounds >= bound)
            && self.last_stall_round > mark.rounds
        {
            Some(Err(NativeProgressWaitError::Stalled {
                rounds: self.unprogressed_rounds,
            }))
        } else if self.closed {
            Some(Err(NativeProgressWaitError::Stopped))
        } else {
            None
        }
    }
}

impl<K: NativeProgressKind> NativeProgressObservation<K> {
    fn progress(&self) -> MutexGuard<'_, Progress> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub(crate) fn record(&self, round: NativeProgressRound) {
        {
            let mut progress = self.progress();
            progress.rounds = progress.rounds.saturating_add(1);
            if round.reached {
                progress.reached = progress.reached.saturating_add(1);
            }
            if round.progressed {
                progress.unprogressed_rounds = 0;
                progress.stalled_rounds = 0;
            } else {
                progress.unprogressed_rounds = progress.unprogressed_rounds.saturating_add(1);
            }
            if round.stalled {
                progress.stalled_rounds = progress.stalled_rounds.saturating_add(1);
                progress.last_stall_round = progress.rounds;
            }
        }
        self.changed.notify_all();
    }

    /// The current point in this kind's history, to wait from.
    #[must_use]
    pub fn mark(&self) -> NativeProgressMark<K> {
        let progress = self.progress();
        NativeProgressMark {
            reached: progress.reached,
            rounds: progress.rounds,
            kind: PhantomData,
        }
    }

    /// Whether waiting from `mark` has finished, without blocking.
    #[cfg(test)]
    pub(crate) fn outcome_after(
        &self,
        mark: NativeProgressMark<K>,
    ) -> Option<Result<(), NativeProgressWaitError>> {
        self.progress().outcome(mark)
    }

    /// Block until two rounds recorded after `mark` reach the goal.
    ///
    /// The first such round may have begun before `mark`; the second began
    /// after the first was recorded, so it observed every commit made before
    /// `mark`. Each kind's waiting method states what that guarantees.
    pub(crate) fn wait_reached_after(
        &self,
        mark: NativeProgressMark<K>,
    ) -> Result<(), NativeProgressWaitError> {
        let mut progress = self.progress();
        let outcome = loop {
            if let Some(outcome) = progress.outcome(mark) {
                break outcome;
            }
            progress = match self.changed.wait(progress) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
        };
        drop(progress);
        outcome
    }
}

/// An observation the pump closes when it stops: every pending and later
/// wait on it then ends with [`NativeProgressWaitError::Stopped`].
pub(crate) trait NativeProgressClose {
    fn close(&self);
}

impl<K: NativeProgressKind> NativeProgressClose for NativeProgressObservation<K> {
    fn close(&self) {
        let mut progress = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        progress.closed = true;
        drop(progress);
        self.changed.notify_all();
    }
}

/// Closes an observation when the pump loop exits by any path.
pub(crate) struct CloseOnExit<'observation>(pub(crate) &'observation dyn NativeProgressClose);

impl Drop for CloseOnExit<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TwoRounds;

    impl NativeProgressKind for TwoRounds {
        const STALL_ROUNDS: Option<u32> = Some(2);
    }

    const REACHED: NativeProgressRound = NativeProgressRound {
        reached: true,
        progressed: true,
        stalled: false,
    };
    const WORKED: NativeProgressRound = NativeProgressRound {
        reached: false,
        progressed: true,
        stalled: false,
    };
    const IDLE: NativeProgressRound = NativeProgressRound {
        reached: false,
        progressed: false,
        stalled: false,
    };
    const STALLED: NativeProgressRound = NativeProgressRound {
        reached: false,
        progressed: false,
        stalled: true,
    };

    /// Every history of one round before the mark and five after it, over
    /// the four round shapes, against the rule stated on the whole history.
    #[test]
    fn the_state_machine_matches_the_rule_over_every_short_history() {
        let shapes = [REACHED, WORKED, IDLE, STALLED];
        let mut histories = vec![Vec::new()];
        for _ in 0..6 {
            histories = histories
                .into_iter()
                .flat_map(|history: Vec<NativeProgressRound>| {
                    shapes.iter().map(move |shape| {
                        let mut next = history.clone();
                        next.push(*shape);
                        next
                    })
                })
                .collect();
        }
        for history in histories {
            let observation = NativeProgressObservation::<TwoRounds>::default();
            let (before, after) = history.split_at(1);
            for round in before {
                observation.record(*round);
            }
            let mark = observation.mark();
            for round in after {
                observation.record(*round);
            }
            assert_eq!(
                observation.outcome_after(mark),
                rule(&history, before.len()),
                "{history:?}"
            );
        }
    }

    /// The stated rule for one recorded history.
    ///
    /// Two goal rounds after the mark finish the wait. Otherwise the run of rounds since the last progress is a stall when it
    /// holds at least two stalled rounds and the last of them is after the
    /// mark; the stall reports the run's length.
    fn rule(
        history: &[NativeProgressRound],
        marked_at: usize,
    ) -> Option<Result<(), NativeProgressWaitError>> {
        let reached_after = history
            .iter()
            .enumerate()
            .filter(|(index, round)| *index >= marked_at && round.reached)
            .count();
        if reached_after >= 2 {
            return Some(Ok(()));
        }
        let run_start = history
            .iter()
            .enumerate()
            .rev()
            .find(|(_, round)| round.progressed)
            .map(|(index, _)| index.saturating_add(1))
            .map_or(0, std::convert::identity);
        let stalls = history
            .iter()
            .enumerate()
            .filter(|(index, round)| *index >= run_start && round.stalled)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let last_after_mark = stalls.last().is_some_and(|index| *index >= marked_at);
        let run = u64::try_from(history.len().saturating_sub(run_start))
            .map_or(u64::MAX, std::convert::identity);
        (stalls.len() >= 2 && last_after_mark)
            .then_some(Err(NativeProgressWaitError::Stalled { rounds: run }))
    }

    #[test]
    fn a_closed_observation_stops_the_wait() {
        let observation = NativeProgressObservation::<TwoRounds>::default();
        let mark = observation.mark();
        {
            let _close = CloseOnExit(&observation);
        }
        assert_eq!(
            observation.wait_reached_after(mark),
            Err(NativeProgressWaitError::Stopped)
        );
    }
}
