//! Epoch-named, retention-bounded snapshots of one auxiliary authority
//! generation (QI-BB-020 W2, plan §5.6 / §6.2).
//!
//! Every durable mutation of a history, runtime-metadata or structural
//! generation produces a new immutable snapshot and names it with the
//! next [`AuxEpochV1`] of that `(repo, revision, generation, domain)`.
//! The registry here keeps the current snapshot and the last
//! [`AUX_EPOCH_RETAIN`] superseded ones, each for at most
//! [`AUX_EPOCH_RETAIN_FOR`] after it was superseded, so a keyset
//! continuation that names the epoch it started in is served from
//! exactly that snapshot: a row can neither appear on two pages nor fall
//! between them. A continuation naming a pruned epoch is refused
//! [`AUX_EPOCH_EXPIRED_CODE`]; one naming an epoch newer than the current
//! is refused [`AUX_EPOCH_UNKNOWN_CODE`]. Neither is ever served from
//! another epoch.
//!
//! The states hold their record maps in persistent (structurally shared)
//! maps, so advancing an epoch clones the state in `O(1)` and a retained
//! snapshot costs only the paths the mutation rewrote — memory is bounded
//! by the retain count times the retained deltas, not by
//! `AUX_EPOCH_RETAIN` copies of the generation.
//!
//! Wall-clock time is passed in by the caller (`now: Instant`) rather than
//! read here, so retention is a pure function of the mutation history and
//! the instants it was asked about.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use quanta_index_contract::AuxEpochV1;
use quanta_index_core::{
    AUX_EPOCH_EXPIRED_CODE, AUX_EPOCH_RETAIN, AUX_EPOCH_RETAIN_FOR, AUX_EPOCH_UNKNOWN_CODE,
    AuxiliaryDomainV1, AuxiliaryGenerationKeyV1, CoreError,
};

/// Why a read at a named epoch was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuxEpochRefusedError {
    /// The epoch was superseded and is no longer retained.
    Expired {
        domain: AuxiliaryDomainV1,
        requested: AuxEpochV1,
        current: AuxEpochV1,
    },
    /// The epoch is newer than anything this authority has produced.
    Unknown {
        domain: AuxiliaryDomainV1,
        requested: AuxEpochV1,
        current: AuxEpochV1,
    },
}

impl AuxEpochRefusedError {
    /// The wire code of the refusal.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Expired { .. } => AUX_EPOCH_EXPIRED_CODE,
            Self::Unknown { .. } => AUX_EPOCH_UNKNOWN_CODE,
        }
    }
}

impl fmt::Display for AuxEpochRefusedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Expired {
                domain,
                requested,
                current,
            } => write!(
                formatter,
                "{domain} authority epoch {requested} is no longer retained (current epoch {current}); restart the page walk"
            ),
            Self::Unknown {
                domain,
                requested,
                current,
            } => write!(
                formatter,
                "{domain} authority epoch {requested} is newer than the current epoch {current}; no snapshot of this state root ever had it"
            ),
        }
    }
}

impl std::error::Error for AuxEpochRefusedError {}

impl From<AuxEpochRefusedError> for CoreError {
    fn from(refused: AuxEpochRefusedError) -> Self {
        Self::Typed {
            code: refused.code().to_string(),
            message: refused.to_string(),
        }
    }
}

/// A snapshot a later epoch superseded, kept for continuations.
#[derive(Debug)]
struct SupersededSnapshot<S> {
    epoch: AuxEpochV1,
    superseded_at: Instant,
    state: Arc<S>,
}

/// What a reader got: the snapshot, which generation and domain it is a
/// snapshot of, the epoch it is, and how many superseded epochs were
/// still retained beside the current one at the time of the read.
///
/// The generation is stamped by the registry that served the read, so a
/// read view can prove every snapshot it holds belongs to its pin rather
/// than trust the order its parts were fetched in.
#[derive(Clone, Debug)]
pub struct AuxRead<S> {
    pub domain: AuxiliaryDomainV1,
    pub generation: AuxiliaryGenerationKeyV1,
    pub epoch: AuxEpochV1,
    pub retained: usize,
    pub state: Arc<S>,
}

/// The current snapshot of one authority generation and the superseded
/// ones still retained, oldest first.
#[derive(Debug)]
pub(crate) struct AuxSnapshots<S> {
    domain: AuxiliaryDomainV1,
    generation: AuxiliaryGenerationKeyV1,
    current_epoch: AuxEpochV1,
    current: Arc<S>,
    superseded: VecDeque<SupersededSnapshot<S>>,
}

impl<S> AuxSnapshots<S>
where
    S: Clone + Default,
{
    /// A registry of `generation`'s `domain` authority at
    /// [`AuxEpochV1::GENESIS`].
    ///
    /// Its current snapshot is the default state: the shape of a
    /// generation restored from rows before its epoch row (if any) is
    /// restored, or of one created in memory.
    pub(crate) fn genesis(domain: AuxiliaryDomainV1, generation: AuxiliaryGenerationKeyV1) -> Self {
        Self {
            domain,
            generation,
            current_epoch: AuxEpochV1::GENESIS,
            current: Arc::new(S::default()),
            superseded: VecDeque::with_capacity(AUX_EPOCH_RETAIN),
        }
    }

    /// The current snapshot's state.
    pub(crate) fn current(&self) -> &S {
        self.current.as_ref()
    }

    /// The epoch the next mutation produces.
    pub(crate) fn next_epoch(&self) -> Result<AuxEpochV1, CoreError> {
        self.current_epoch.checked_next().ok_or_else(|| {
            CoreError::Storage(format!(
                "{} authority epoch sequence is exhausted at {}",
                self.domain, self.current_epoch
            ))
        })
    }

    /// How many superseded snapshots are retained beside the current one;
    /// never more than [`AUX_EPOCH_RETAIN`].
    pub(crate) fn retained(&self) -> usize {
        self.superseded.len()
    }

    /// Every epoch the registry holds a snapshot of: the superseded ones
    /// still retained, oldest first, then the current one.
    ///
    /// A wall-clock-aged snapshot is listed until the next mutation prunes
    /// it; it is unreadable meanwhile, and whatever is bound to it is
    /// reclaimed with it.
    pub(crate) fn retained_epochs(&self) -> Vec<AuxEpochV1> {
        self.superseded
            .iter()
            .map(|snapshot| snapshot.epoch)
            .chain(std::iter::once(self.current_epoch))
            .collect()
    }

    /// The current snapshot as a read.
    pub(crate) fn read_current(&self) -> AuxRead<S> {
        AuxRead {
            domain: self.domain,
            generation: self.generation.clone(),
            epoch: self.current_epoch,
            retained: self.retained(),
            state: Arc::clone(&self.current),
        }
    }

    /// The snapshot at `epoch`: the current one, or a retained one that was
    /// superseded no longer than [`AUX_EPOCH_RETAIN_FOR`] before `now`.
    pub(crate) fn read_at(
        &self,
        epoch: AuxEpochV1,
        now: Instant,
    ) -> Result<AuxRead<S>, AuxEpochRefusedError> {
        if epoch == self.current_epoch {
            return Ok(self.read_current());
        }
        if epoch > self.current_epoch {
            return Err(AuxEpochRefusedError::Unknown {
                domain: self.domain,
                requested: epoch,
                current: self.current_epoch,
            });
        }
        let expired = || AuxEpochRefusedError::Expired {
            domain: self.domain,
            requested: epoch,
            current: self.current_epoch,
        };
        let Some(snapshot) = self
            .superseded
            .iter()
            .find(|snapshot| snapshot.epoch == epoch)
        else {
            return Err(expired());
        };
        if now.saturating_duration_since(snapshot.superseded_at) > AUX_EPOCH_RETAIN_FOR {
            return Err(expired());
        }
        Ok(AuxRead {
            domain: self.domain,
            generation: self.generation.clone(),
            epoch: snapshot.epoch,
            retained: self.retained(),
            state: Arc::clone(&snapshot.state),
        })
    }

    /// Produce the next snapshot.
    ///
    /// `mutate` runs on a clone of the current state and, if it succeeds,
    /// the result becomes the current snapshot at `epoch`, which must be
    /// [`Self::next_epoch`] — the epoch the caller made durable with the
    /// rows. The previous snapshot stays retained, superseded at `now`;
    /// whatever the bounds no longer keep is pruned. If `mutate` fails,
    /// nothing changes.
    pub(crate) fn advance(
        &mut self,
        epoch: AuxEpochV1,
        now: Instant,
        mutate: impl FnOnce(&mut S) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let expected = self.next_epoch()?;
        if epoch != expected {
            return Err(CoreError::Storage(format!(
                "{} authority epoch drift: mutation stamped {epoch} but the next epoch is {expected}",
                self.domain
            )));
        }
        let mut next = self.current().clone();
        mutate(&mut next)?;
        let previous = std::mem::replace(&mut self.current, Arc::new(next));
        self.superseded.push_back(SupersededSnapshot {
            epoch: self.current_epoch,
            superseded_at: now,
            state: previous,
        });
        self.current_epoch = epoch;
        self.prune(now);
        Ok(())
    }

    /// Drop superseded snapshots past the count bound, then the ones
    /// superseded longer than [`AUX_EPOCH_RETAIN_FOR`] before `now`.
    fn prune(&mut self, now: Instant) {
        while self.superseded.len() > AUX_EPOCH_RETAIN {
            let _pruned = self.superseded.pop_front();
        }
        while self.superseded.front().is_some_and(|snapshot| {
            now.saturating_duration_since(snapshot.superseded_at) > AUX_EPOCH_RETAIN_FOR
        }) {
            let _pruned = self.superseded.pop_front();
        }
    }

    /// The current state, to be filled from durable rows at boot.
    ///
    /// Restore rebuilds the snapshot the rows describe rather than
    /// producing a new one, so the epoch does not advance here; the epoch
    /// row restores it through [`Self::restore_epoch`]. Boot runs before
    /// the ledger is shared, so no reader holds the snapshot and the
    /// in-place mutation copies nothing.
    pub(crate) fn restore_state_mut(&mut self) -> &mut S {
        Arc::make_mut(&mut self.current)
    }

    /// Install the epoch the durable rows were stamped with.
    pub(crate) const fn restore_epoch(&mut self, epoch: AuxEpochV1) {
        self.current_epoch = epoch;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use quanta_index_contract::{AuxEpochV1, ManifestGeneration, RepoId, RevisionId};
    use quanta_index_core::{
        AUX_EPOCH_RETAIN, AUX_EPOCH_RETAIN_FOR, AuxiliaryDomainV1, AuxiliaryGenerationKeyV1,
        CoreError,
    };

    use super::{AuxEpochRefusedError, AuxSnapshots};

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    fn generation() -> AuxiliaryGenerationKeyV1 {
        AuxiliaryGenerationKeyV1 {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
        }
    }

    /// A state whose content is the list of mutations applied to it.
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    struct Journal(Vec<u64>);

    fn advance(ring: &mut AuxSnapshots<Journal>, value: u64, now: Instant) -> TestRes {
        let epoch = ring.next_epoch()?;
        ring.advance(epoch, now, |state| {
            state.0.push(value);
            Ok(())
        })?;
        Ok(())
    }

    #[test]
    fn a_retained_epoch_reads_as_the_content_it_had() -> TestRes {
        let now = Instant::now();
        let mut ring = AuxSnapshots::<Journal>::genesis(AuxiliaryDomainV1::History, generation());
        advance(&mut ring, 1, now)?;
        advance(&mut ring, 2, now)?;
        advance(&mut ring, 3, now)?;
        let at_one = ring.read_at(AuxEpochV1::new(1), now)?;
        let at_two = ring.read_at(AuxEpochV1::new(2), now)?;
        let current = ring.read_current();
        if at_one.state.0 != vec![1]
            || at_two.state.0 != vec![1, 2]
            || current.state.0 != vec![1, 2, 3]
        {
            return Err(format!("epochs must read as their own content: {ring:?}").into());
        }
        if at_one.epoch != AuxEpochV1::new(1)
            || current.epoch != AuxEpochV1::new(3)
            || current.retained != 3
        {
            return Err(format!("read identity drifted: {at_one:?} {current:?}").into());
        }
        Ok(())
    }

    #[test]
    fn the_registry_never_retains_more_than_the_bound() -> TestRes {
        let now = Instant::now();
        let mut ring = AuxSnapshots::<Journal>::genesis(AuxiliaryDomainV1::Runtime, generation());
        for value in 1..=u64::try_from(AUX_EPOCH_RETAIN)?.saturating_mul(4) {
            advance(&mut ring, value, now)?;
            if ring.retained() > AUX_EPOCH_RETAIN {
                return Err(format!(
                    "the registry retained {} superseded snapshots after {value} mutations; the bound is {AUX_EPOCH_RETAIN}",
                    ring.retained(),
                )
                .into());
            }
        }
        if ring.retained() != AUX_EPOCH_RETAIN {
            return Err(format!(
                "a busy registry retains exactly the bound, retained {}",
                ring.retained()
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn an_epoch_pruned_by_count_is_expired_and_never_served_from_another() -> TestRes {
        let now = Instant::now();
        let mut ring = AuxSnapshots::<Journal>::genesis(AuxiliaryDomainV1::History, generation());
        advance(&mut ring, 1, now)?;
        let pinned = ring.read_current().epoch;
        for value in 2..=u64::try_from(AUX_EPOCH_RETAIN)?.saturating_add(1) {
            advance(&mut ring, value, now)?;
            let served = ring.read_at(pinned, now)?;
            if served.state.0 != vec![1] {
                return Err(format!("epoch 1 must still read as [1], got {served:?}").into());
            }
        }
        // One more mutation pushes epoch 1 out of the registry.
        advance(&mut ring, 99, now)?;
        match ring.read_at(pinned, now) {
            Err(AuxEpochRefusedError::Expired { requested, .. }) if requested == pinned => Ok(()),
            other => Err(format!("a pruned epoch is refused expired, got {other:?}").into()),
        }
    }

    #[test]
    fn an_epoch_superseded_longer_than_the_wall_clock_bound_is_expired() -> TestRes {
        let start = Instant::now();
        let mut ring =
            AuxSnapshots::<Journal>::genesis(AuxiliaryDomainV1::Structural, generation());
        advance(&mut ring, 1, start)?;
        let pinned = ring.read_current().epoch;
        advance(&mut ring, 2, start)?;
        let within = start
            .checked_add(AUX_EPOCH_RETAIN_FOR)
            .ok_or("instant arithmetic")?;
        if ring.read_at(pinned, within)?.state.0 != vec![1] {
            return Err("within the bound the superseded epoch is served".into());
        }
        let past = within
            .checked_add(Duration::from_millis(1))
            .ok_or("instant arithmetic")?;
        match ring.read_at(pinned, past) {
            Err(AuxEpochRefusedError::Expired { .. }) => {}
            other => {
                return Err(format!("past the bound the epoch is expired, got {other:?}").into());
            }
        }
        // The current epoch has no wall-clock bound: it is the live data.
        if ring.read_at(ring.read_current().epoch, past)?.state.0 != vec![1, 2] {
            return Err("the current epoch is always served".into());
        }
        // A later mutation reclaims the stale snapshot.
        advance(&mut ring, 3, past)?;
        if ring.retained() != 1 {
            return Err(format!(
                "the stale snapshot is pruned on the next mutation, registry retains {}",
                ring.retained()
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn an_epoch_newer_than_the_current_is_unknown() -> TestRes {
        let now = Instant::now();
        let mut ring = AuxSnapshots::<Journal>::genesis(AuxiliaryDomainV1::History, generation());
        advance(&mut ring, 1, now)?;
        match ring.read_at(AuxEpochV1::new(2), now) {
            Err(AuxEpochRefusedError::Unknown { .. }) => Ok(()),
            other => Err(format!("a future epoch is refused unknown, got {other:?}").into()),
        }
    }

    #[test]
    fn a_failed_mutation_changes_nothing() -> TestRes {
        let now = Instant::now();
        let mut ring = AuxSnapshots::<Journal>::genesis(AuxiliaryDomainV1::History, generation());
        advance(&mut ring, 1, now)?;
        let epoch = ring.next_epoch()?;
        let refused = ring.advance(epoch, now, |state| {
            state.0.push(2);
            Err(CoreError::InvalidContract("refused".to_string()))
        });
        if refused.is_ok() {
            return Err("the mutation's error propagates".into());
        }
        if ring.read_current().epoch != AuxEpochV1::new(1)
            || ring.current().0 != vec![1]
            || ring.retained() != 1
        {
            return Err(
                format!("a refused mutation leaves the registry as it was: {ring:?}").into(),
            );
        }
        Ok(())
    }

    #[test]
    fn a_mutation_stamped_with_the_wrong_epoch_is_refused() -> TestRes {
        let now = Instant::now();
        let mut ring = AuxSnapshots::<Journal>::genesis(AuxiliaryDomainV1::History, generation());
        advance(&mut ring, 1, now)?;
        let drift = ring.advance(AuxEpochV1::new(5), now, |_state| Ok(()));
        match drift {
            Err(CoreError::Storage(message)) if message.contains("epoch drift") => Ok(()),
            other => Err(format!("epoch drift is a storage error, got {other:?}").into()),
        }
    }

    #[test]
    fn restore_fills_the_current_snapshot_without_advancing() -> TestRes {
        let now = Instant::now();
        let mut ring = AuxSnapshots::<Journal>::genesis(AuxiliaryDomainV1::Runtime, generation());
        ring.restore_state_mut().0.push(7);
        ring.restore_epoch(AuxEpochV1::new(41));
        if ring.retained() != 0 || ring.read_current().epoch != AuxEpochV1::new(41) {
            return Err(format!("restore names the epoch the rows carry: {ring:?}").into());
        }
        // The sequence continues from the restored epoch.
        advance(&mut ring, 8, now)?;
        if ring.read_current().epoch != AuxEpochV1::new(42) || ring.current().0 != vec![7, 8] {
            return Err(format!("the next mutation continues the sequence: {ring:?}").into());
        }
        Ok(())
    }
}
