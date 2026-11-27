//! Service-owned bounded retirement; Catalog remains the lifetime authority.
//!
//! # Rotation
//!
//! The configured scopes form one rotation: every round runs the turn of the
//! scope at its front and moves that scope to the back, whether the turn
//! completed or failed, so with `n` configured scopes every scope takes a
//! turn every `n` rounds and a scope that keeps failing never takes another
//! scope's turn. A committed release queues its view range on its scope; the
//! scope's next turn retires that range unless its previous turn already
//! served a release, so regular Work, Views and Custody turns continue under
//! sustained releases and a queued range is reached within `2n` rounds.
//!
//! A scope's queued ranges are cleared when its floor step refuses with the
//! typed `SnapshotUnavailable`: the scope's epoch is not its workspace's
//! current epoch. A release implies the workspace was current at that epoch,
//! and the Catalog never changes a workspace's epoch or removes its row, so
//! such a range can never be served.
//!
//! # Passes
//!
//! A pass ends once every configured scope has taken a turn in it. A pass in
//! which no turn completed is a stalled pass; see
//! [`crate::native_retention_observation::NATIVE_RETENTION_STALL_PASSES`].

use std::{
    collections::{BTreeSet, VecDeque},
    time::Duration,
};

use quanta_index_catalog::{MaintenanceDemand, ReleasedViewRange};
use quanta_index_core::index::{
    IndexError, WorkBudget, WorkLimits, WorkspaceKey,
    ports::{
        CatalogCustodyMaintenancePort, CatalogExactHistoryPort, CatalogTerminalWorkRetentionPort,
        WorkRetirementCursor,
    },
};

use crate::native_work_diagnostic::RetentionProgress;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NativeRetentionScope {
    pub workspace: WorkspaceKey,
    pub epoch: u64,
}

/// Explicit operator sequence-window policy, independent of wall-clock age.
/// A wider window cannot undo a durable floor already advanced by an owner.
#[derive(Clone, Debug)]
pub struct NativeRetentionConfig {
    pub scopes: Vec<NativeRetentionScope>,
    pub sequence_window: u64,
    pub page_limit: u32,
    /// Per-operation custody child page. Defaults to `page_limit` in product config.
    pub custody_page_limit: u32,
    pub interval: Duration,
    pub round_timeout: Duration,
    pub work_limits: WorkLimits,
    pub head_probe_every: u32,
}

impl NativeRetentionConfig {
    /// Initial bounded-round admission floor.
    ///
    /// View retirement has at most seven roots,
    /// work retirement at most four children, and intent metadata is bounded
    /// to 1 MiB. This is not a proved maximum for every receipt payload.
    pub fn minimum_work_limits(page_limit: u32) -> Result<WorkLimits, IndexError> {
        if !(1..=32).contains(&page_limit) {
            return Err(IndexError::InvalidInput(
                "invalid retention page size".into(),
            ));
        }
        let rows = u64::from(page_limit)
            .checked_add(4)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(WorkLimits {
            reads: rows.checked_mul(128).ok_or(IndexError::ResourceLimited)?,
            writes: rows.checked_mul(32).ok_or(IndexError::ResourceLimited)?,
            bytes: rows
                .checked_mul(2 * 1024 * 1024)
                .ok_or(IndexError::ResourceLimited)?,
            mutations: rows.checked_mul(32).ok_or(IndexError::ResourceLimited)?,
        })
    }

    pub fn validate(&self) -> Result<(), IndexError> {
        if self.scopes.is_empty()
            || self.scopes.len() > 1024
            || self.sequence_window == 0
            || self.sequence_window > i64::MAX.unsigned_abs()
            || !(1..=32).contains(&self.page_limit)
            || !(1..=32).contains(&self.custody_page_limit)
            || !(2..=1024).contains(&self.head_probe_every)
            || self.interval < Duration::from_millis(10)
            || self.interval > Duration::from_secs(60)
            || self.round_timeout < Duration::from_millis(100)
            || self.round_timeout > Duration::from_secs(120)
            || [
                self.work_limits.reads,
                self.work_limits.writes,
                self.work_limits.bytes,
                self.work_limits.mutations,
            ]
            .contains(&0)
        {
            return Err(IndexError::InvalidInput(
                "invalid native retention envelope".into(),
            ));
        }
        let minimum = Self::minimum_work_limits(self.page_limit.max(self.custody_page_limit))?;
        if self.work_limits.reads < minimum.reads
            || self.work_limits.writes < minimum.writes
            || self.work_limits.bytes < minimum.bytes
            || self.work_limits.mutations < minimum.mutations
        {
            return Err(IndexError::InvalidInput(
                "native retention WorkLimits are below the bounded round allowance".into(),
            ));
        }
        let mut unique = BTreeSet::new();
        if self
            .scopes
            .iter()
            .any(|scope| scope.epoch > i64::MAX.unsigned_abs() || !unique.insert(*scope))
        {
            return Err(IndexError::InvalidInput(
                "invalid or duplicate native retention scope".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct SweepRequest<K> {
    after: Option<K>,
    through: Option<K>,
    head: bool,
}

/// Forward progress and a finite head prefix have independent cursors. New
/// high keys cannot extend an in-flight head sweep or hide lower arrivals.
struct Sweep<K> {
    forward: Option<K>,
    head: Option<(Option<K>, K)>,
    turns: u32,
}

impl<K: Copy> Sweep<K> {
    const fn new() -> Self {
        Self {
            forward: None,
            head: None,
            turns: 0,
        }
    }

    fn request(&mut self, period: u32) -> Result<SweepRequest<K>, IndexError> {
        if self.head.is_none() && self.turns >= period {
            if let Some(frontier) = self.forward {
                self.head = Some((None, frontier));
            }
            self.turns = 0;
        }
        if let Some((after, frontier)) = self.head {
            Ok(SweepRequest {
                after,
                through: Some(frontier),
                head: true,
            })
        } else {
            self.turns = self
                .turns
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            Ok(SweepRequest {
                after: self.forward,
                through: None,
                head: false,
            })
        }
    }

    fn accept(
        &mut self,
        request: SweepRequest<K>,
        after: Option<K>,
        has_more: bool,
        reached: bool,
    ) {
        if request.head {
            self.head = if !has_more || reached {
                None
            } else {
                request.through.map(|upper| (after, upper))
            };
        } else {
            self.forward = if has_more { after } else { None };
        }
    }
}

fn validate_page<K: Copy + Ord>(
    request: SweepRequest<K>,
    after: Option<K>,
    counts: [u32; 3],
    has_more: bool,
    limit: u32,
) -> Result<(), IndexError> {
    let [examined, retired, blocked] = counts;
    let progresses = after.is_some_and(|next| request.after.is_none_or(|prior| next > prior));
    if examined > limit
        || retired.checked_add(blocked) != Some(examined)
        || (examined > 0 && !progresses)
        || (examined == 0 && (has_more || after != request.after))
    {
        return Err(IndexError::BackendViolation(
            "retirement page violated its bounded progress contract".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Phase {
    Work,
    Views,
    Custody,
}

// Release hints accelerate the durable Views sweep. Keep a bounded backlog
// even when readers release pins faster than retention can run.
const MAX_PENDING_RELEASE_RANGES: usize = 256;

struct ReleasedViews {
    after: Option<u64>,
    last_sequence: u64,
}

impl ReleasedViews {
    fn new(released: &ReleasedViewRange) -> Self {
        Self {
            after: released.first_sequence.checked_sub(1),
            last_sequence: released.last_sequence,
        }
    }

    fn covers_remaining(&self, released: &ReleasedViewRange) -> bool {
        self.after
            .is_none_or(|after| after < released.first_sequence)
            && self.last_sequence >= released.last_sequence
    }

    fn request(&self) -> SweepRequest<u64> {
        SweepRequest {
            after: self.after,
            through: Some(self.last_sequence),
            head: false,
        }
    }
}

struct ScopeState {
    scope: NativeRetentionScope,
    phase: Phase,
    work: Sweep<WorkRetirementCursor>,
    views: Sweep<u64>,
    custody: Sweep<u64>,
    // A successful partial release proves that this scope has another page.
    // An empty scan clears the hint; Catalog remains the debt authority.
    custody_unfinished: bool,
    // Independent, gap-free ranges in arrival order. The front retains its
    // cursor when a new release arrives behind it.
    released_views: VecDeque<ReleasedViews>,
    // A release page gets the first turn, then yields one regular phase.
    served_release_last: bool,
}

/// What one round of the driver showed, for the retention observation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct NativeRetentionRound {
    /// The round's turn completed.
    pub(crate) completed: bool,
    /// The round ended a pass in which no turn completed.
    pub(crate) stalled_pass: bool,
    /// A committed release range still waits for its scope's release turn.
    pub(crate) releases_pending: bool,
}

pub(crate) struct NativeRetentionDriver {
    config: NativeRetentionConfig,
    scopes: VecDeque<ScopeState>,
    progress: Option<RetentionProgress>,
    // Scopes that have not taken a turn in the current pass.
    pass_untried: BTreeSet<NativeRetentionScope>,
    // Whether a turn of the current pass completed.
    pass_completed: bool,
    last_round: NativeRetentionRound,
}

impl NativeRetentionDriver {
    pub(crate) fn new(config: NativeRetentionConfig) -> Result<Self, IndexError> {
        config.validate()?;
        let scopes = config
            .scopes
            .iter()
            .copied()
            .map(|scope| ScopeState {
                scope,
                phase: Phase::Work,
                work: Sweep::new(),
                views: Sweep::new(),
                custody: Sweep::new(),
                custody_unfinished: false,
                released_views: VecDeque::new(),
                served_release_last: false,
            })
            .collect();
        let pass_untried = config.scopes.iter().copied().collect();
        Ok(Self {
            config,
            scopes,
            progress: RetentionProgress::new_if_enabled(),
            pass_untried,
            pass_completed: false,
            last_round: NativeRetentionRound::default(),
        })
    }

    pub(crate) fn config(&self) -> &NativeRetentionConfig {
        &self.config
    }

    /// Steer the next rounds by committed Catalog demand.
    ///
    /// Released ranges run in arrival order, independently of ordinary Views.
    /// The durable Views sweep still covers hints that cannot be queued.
    pub(crate) fn schedule(&mut self, demand: &MaintenanceDemand) {
        for released in &demand.released_views {
            let Some(state) = self.scopes.iter_mut().find(|state| {
                state.scope.workspace == released.workspace && state.scope.epoch == released.epoch
            }) else {
                continue;
            };
            let pending = &mut state.released_views;
            if pending.len() < MAX_PENDING_RELEASE_RANGES
                && !pending.iter().any(|range| range.covers_remaining(released))
                && pending.try_reserve(1).is_ok()
            {
                pending.push_back(ReleasedViews::new(released));
            }
        }
    }

    /// What the latest round showed.
    pub(crate) const fn last_round(&self) -> NativeRetentionRound {
        self.last_round
    }

    pub(crate) fn record_admission_refusal(&mut self) {
        if let Some(progress) = &mut self.progress {
            progress.admission_refused();
        }
    }

    /// One scope, one scheduled retirement phase and one original budget per
    /// round. An empty Work or Views page, or a blocked page with a known
    /// partial custody release, may use spare allowance for one custody page
    /// without changing the next scheduled phase.
    pub(crate) fn run_round<C>(
        &mut self,
        catalog: &C,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>
    where
        C: CatalogExactHistoryPort
            + CatalogTerminalWorkRetentionPort
            + CatalogCustodyMaintenancePort,
    {
        let mut state = self
            .scopes
            .pop_front()
            .ok_or_else(|| IndexError::BackendViolation("retention lost its scopes".into()))?;
        let floor = self.retain_floor(catalog, state.scope, budget);
        let result = match floor {
            Ok(()) => self.run_scope(catalog, &mut state, budget),
            Err(IndexError::SnapshotUnavailable) => {
                // The scope's epoch is not current: its ranges can never be
                // served, so they are cleared rather than retried.
                state.released_views.clear();
                state.served_release_last = false;
                Err(IndexError::SnapshotUnavailable)
            }
            Err(error) => Err(error),
        };
        let _untried = self.pass_untried.remove(&state.scope);
        self.scopes.push_back(state);
        self.pass_completed |= result.is_ok();
        let stalled_pass = self.pass_untried.is_empty() && !self.pass_completed;
        if self.pass_untried.is_empty() {
            self.pass_untried = self.config.scopes.iter().copied().collect();
            self.pass_completed = false;
        }
        self.last_round = NativeRetentionRound {
            completed: result.is_ok(),
            stalled_pass,
            releases_pending: self
                .scopes
                .iter()
                .any(|state| !state.released_views.is_empty()),
        };
        if let Some(progress) = &mut self.progress {
            progress.emit_due();
        }
        result
    }

    /// Every turn first retains its scope's exact-history tail.
    fn retain_floor<C: CatalogExactHistoryPort>(
        &mut self,
        catalog: &C,
        scope: NativeRetentionScope,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let floor = catalog.retain_exact_history_tail(
            scope.workspace,
            scope.epoch,
            self.config.sequence_window,
            budget,
        );
        if let Some(progress) = &mut self.progress {
            progress.floor_result(floor.is_err());
        }
        floor.map(|_floor| ())
    }

    /// The scheduled release or regular phase of a turn whose floor step
    /// succeeded.
    fn run_scope<C>(
        &mut self,
        catalog: &C,
        state: &mut ScopeState,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>
    where
        C: CatalogExactHistoryPort
            + CatalogTerminalWorkRetentionPort
            + CatalogCustodyMaintenancePort,
    {
        let NativeRetentionScope { workspace, epoch } = state.scope;
        let release_turn = !state.released_views.is_empty() && !state.served_release_last;
        state.served_release_last = release_turn;
        let phase = if release_turn {
            Phase::Views
        } else {
            let phase = state.phase;
            state.phase = match phase {
                Phase::Work => Phase::Views,
                Phase::Views => Phase::Custody,
                Phase::Custody => Phase::Work,
            };
            phase
        };
        match phase {
            Phase::Work => {
                let request = state.work.request(self.config.head_probe_every)?;
                let page = catalog.retire_terminal_work_page(
                    workspace,
                    epoch,
                    request.after,
                    self.config.page_limit,
                    budget,
                )?;
                validate_page(
                    request,
                    page.after,
                    [page.examined, page.retired, page.blocked],
                    page.has_more,
                    self.config.page_limit,
                )?;
                let reached = page
                    .after
                    .zip(request.through)
                    .is_some_and(|(after, upper)| after >= upper);
                state
                    .work
                    .accept(request, page.after, page.has_more, reached);
                if (page.examined == 0 || (page.retired == 0 && state.custody_unfinished))
                    && self.can_use_spare_custody(budget)?
                {
                    self.run_custody_page(catalog, state, budget)?;
                }
            }
            Phase::Views => {
                let released = state.released_views.front_mut().filter(|_| release_turn);
                let request = match &released {
                    Some(released) => released.request(),
                    None => state.views.request(self.config.head_probe_every)?,
                };
                let page = match request.through {
                    Some(through) => catalog.retire_exact_history_page_through(
                        workspace,
                        epoch,
                        request.after,
                        through,
                        self.config.page_limit,
                        budget,
                    ),
                    None => catalog.retire_exact_history_page(
                        workspace,
                        epoch,
                        request.after,
                        self.config.page_limit,
                        budget,
                    ),
                }?;
                validate_page(
                    request,
                    page.after,
                    [page.examined, page.retired, page.blocked],
                    page.has_more,
                    self.config.page_limit,
                )?;
                let reached = page
                    .after
                    .zip(request.through)
                    .is_some_and(|(after, upper)| after >= upper);
                if let Some(released) = released {
                    if reached || !page.has_more {
                        let _finished = state.released_views.pop_front();
                    } else {
                        released.after = page.after;
                    }
                } else {
                    state
                        .views
                        .accept(request, page.after, page.has_more, reached);
                }
                if (page.examined == 0 || (page.retired == 0 && state.custody_unfinished))
                    && self.can_use_spare_custody(budget)?
                {
                    self.run_custody_page(catalog, state, budget)?;
                }
            }
            Phase::Custody => self.run_custody_page(catalog, state, budget)?,
        }
        Ok(())
    }

    fn can_use_spare_custody(&self, budget: &WorkBudget) -> Result<bool, IndexError> {
        let scheduled = NativeRetentionConfig::minimum_work_limits(self.config.page_limit)?;
        let custody = NativeRetentionConfig::minimum_work_limits(self.config.custody_page_limit)?;
        let configured = self.config.work_limits;
        let used = budget.usage();
        // The configured envelope must fund both original phase minima, and
        // the same original budget must still have the custody phase minimum.
        // These are conservative admission floors, not a claim that every
        // Catalog receipt fits; Catalog retains its exact charge/refusal.
        // The pump constructs that budget from this config. A narrower caller
        // budget can still refuse through its own original charge checks.
        let spare = [
            (configured.reads, used.reads, scheduled.reads, custody.reads),
            (
                configured.writes,
                used.writes,
                scheduled.writes,
                custody.writes,
            ),
            (configured.bytes, used.bytes, scheduled.bytes, custody.bytes),
            (
                configured.mutations,
                used.mutations,
                scheduled.mutations,
                custody.mutations,
            ),
        ]
        .into_iter()
        .all(|(limit, used, scheduled, custody)| {
            scheduled
                .checked_add(custody)
                .is_some_and(|sum| limit >= sum)
                && limit.checked_sub(used).is_some_and(|left| left >= custody)
        });
        Ok(spare
            && !budget.request().is_cancelled()
            && budget.request().remaining() > Duration::ZERO)
    }

    fn run_custody_page<C>(
        &mut self,
        catalog: &C,
        state: &mut ScopeState,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>
    where
        C: CatalogCustodyMaintenancePort,
    {
        let NativeRetentionScope { workspace, epoch } = state.scope;
        let request = state.custody.request(self.config.head_probe_every)?;
        let candidate = catalog
            .next_terminal_custody(workspace, epoch, request.after, request.through, budget)
            .inspect_err(|_| {
                if let Some(progress) = &mut self.progress {
                    progress.custody_probe_refused();
                }
            })?;
        let (after, has_more) = if let Some(candidate) = candidate {
            let sequence = candidate.sequence;
            if sequence == 0
                || request.after.is_some_and(|prior| sequence <= prior)
                || request.through.is_some_and(|upper| sequence > upper)
                || candidate.operation.workspace != workspace
                || candidate.operation.epoch != epoch
            {
                return Err(IndexError::BackendViolation(
                    "custody cursor did not progress".into(),
                ));
            }
            let released = catalog.release_terminal_custody(
                candidate.operation,
                self.config.custody_page_limit,
                budget,
            );
            if let Some(progress) = &mut self.progress {
                if request.after.is_none() {
                    progress.oldest_candidate(sequence, candidate.operation.sequence);
                }
                progress.custody_release(
                    sequence,
                    match &released {
                        Ok(page) => Some(page),
                        Err(_) => None,
                    },
                );
            }
            let released = released?;
            if released.released > self.config.custody_page_limit
                || (released.has_more && released.released == 0)
            {
                return Err(IndexError::BackendViolation(
                    "custody release did not make bounded progress".into(),
                ));
            }
            state.custody_unfinished = released.has_more;
            // Partial custody advances this traversal too; the next finite
            // head/sweep revisit resumes the same durable row.
            let more = catalog
                .next_terminal_custody(workspace, epoch, Some(sequence), request.through, budget)
                .inspect_err(|_| {
                    if let Some(progress) = &mut self.progress {
                        progress.custody_probe_refused();
                    }
                })?
                .is_some();
            (Some(sequence), more)
        } else {
            state.custody_unfinished = false;
            (request.after, false)
        };
        state.custody.accept(request, after, has_more, false);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{
        OperationKey,
        ports::{
            TerminalCustodyCandidate, TerminalCustodyPage, TerminalWorkRetirementPage,
            ViewRetirementPage,
        },
    };
    use std::{
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        },
        time::Instant,
    };

    /// Faults occur at real driver boundaries, not at the sweep helper.
    #[derive(Default)]
    struct RefusingCatalog {
        calls: Mutex<Vec<(u64, &'static str)>>,
        limits: Mutex<Vec<(&'static str, u32)>>,
        custody_candidate: Option<TerminalCustodyCandidate>,
        work_page: Option<TerminalWorkRetirementPage>,
        view_page: Option<ViewRetirementPage>,
        refuse_views: bool,
        refuse_custody: bool,
        custody_has_more: bool,
        custody_absent: AtomicBool,
    }

    fn bounded_view_page(
        after: Option<u64>,
        through: u64,
        mut page: ViewRetirementPage,
    ) -> ViewRetirementPage {
        if page.after.is_some_and(|sequence| sequence > through) {
            return ViewRetirementPage {
                after,
                examined: 0,
                retired: 0,
                blocked: 0,
                has_more: false,
            };
        }
        page.has_more &= page.after != Some(through);
        page
    }

    impl RefusingCatalog {
        fn record(&self, epoch: u64, phase: &'static str) -> Result<(), IndexError> {
            self.calls
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?
                .push((epoch, phase));
            Ok(())
        }

        fn record_limit(&self, phase: &'static str, limit: u32) -> Result<(), IndexError> {
            self.limits
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?
                .push((phase, limit));
            Ok(())
        }
    }

    impl CatalogExactHistoryPort for RefusingCatalog {
        fn exact_history_floor(
            &self,
            _workspace: WorkspaceKey,
            _epoch: u64,
        ) -> Result<u64, IndexError> {
            Err(IndexError::BackendViolation("unexpected floor read".into()))
        }

        fn advance_exact_history_floor(
            &self,
            _workspace: WorkspaceKey,
            _epoch: u64,
            _floor: u64,
            _budget: &mut WorkBudget,
        ) -> Result<u64, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected explicit floor advance".into(),
            ))
        }

        fn retain_exact_history_tail(
            &self,
            _workspace: WorkspaceKey,
            epoch: u64,
            _window: u64,
            _budget: &mut WorkBudget,
        ) -> Result<u64, IndexError> {
            self.record(epoch, "floor")?;
            if epoch == 1 {
                Err(IndexError::SnapshotUnavailable)
            } else {
                Ok(10)
            }
        }

        fn retire_exact_history_page(
            &self,
            _workspace: WorkspaceKey,
            epoch: u64,
            after: Option<u64>,
            limit: u32,
            _budget: &mut WorkBudget,
        ) -> Result<ViewRetirementPage, IndexError> {
            self.record_limit("views", limit)?;
            self.record(epoch, "views")?;
            if self.refuse_views {
                return Err(IndexError::ResourceLimited);
            }
            if let Some(mut page) = self.view_page {
                if page.examined == 0 {
                    page.after = after;
                }
                return Ok(page);
            }
            Ok(ViewRetirementPage {
                after,
                examined: 0,
                retired: 0,
                blocked: 0,
                has_more: false,
            })
        }

        fn retire_exact_history_page_through(
            &self,
            workspace: WorkspaceKey,
            epoch: u64,
            after: Option<u64>,
            through: u64,
            limit: u32,
            budget: &mut WorkBudget,
        ) -> Result<ViewRetirementPage, IndexError> {
            let page = self.retire_exact_history_page(workspace, epoch, after, limit, budget)?;
            Ok(bounded_view_page(after, through, page))
        }
    }

    impl CatalogTerminalWorkRetentionPort for RefusingCatalog {
        fn retire_terminal_work_page(
            &self,
            _workspace: WorkspaceKey,
            epoch: u64,
            after: Option<WorkRetirementCursor>,
            limit: u32,
            _budget: &mut WorkBudget,
        ) -> Result<TerminalWorkRetirementPage, IndexError> {
            self.record_limit("work", limit)?;
            self.record(epoch, "work")?;
            self.work_page
                .map(|mut page| {
                    if page.examined == 0 {
                        page.after = after;
                    }
                    page
                })
                .ok_or(IndexError::ResourceLimited)
        }
    }

    impl CatalogCustodyMaintenancePort for RefusingCatalog {
        fn next_terminal_custody(
            &self,
            _workspace: WorkspaceKey,
            epoch: u64,
            after: Option<u64>,
            _through: Option<u64>,
            _budget: &mut WorkBudget,
        ) -> Result<Option<TerminalCustodyCandidate>, IndexError> {
            self.record(epoch, "custody")?;
            if self.refuse_custody {
                return Err(IndexError::ResourceLimited);
            }
            Ok(
                (after.is_none() && !self.custody_absent.load(Ordering::Acquire))
                    .then_some(self.custody_candidate)
                    .flatten(),
            )
        }

        fn release_terminal_custody(
            &self,
            key: OperationKey,
            limit: u32,
            _budget: &mut WorkBudget,
        ) -> Result<TerminalCustodyPage, IndexError> {
            self.record(key.epoch, "release")?;
            self.record_limit("custody", limit)?;
            Ok(TerminalCustodyPage {
                released: 1,
                has_more: self.custody_has_more,
            })
        }
    }

    fn fallback_scope(epoch: u64) -> NativeRetentionScope {
        use quanta_index_core::index::{CollectionId, SlotId, TenantId, WorktreeId};
        NativeRetentionScope {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch,
        }
    }

    fn fallback_config(
        scopes: Vec<NativeRetentionScope>,
    ) -> Result<NativeRetentionConfig, IndexError> {
        let minimum = NativeRetentionConfig::minimum_work_limits(1)?;
        Ok(NativeRetentionConfig {
            scopes,
            sequence_window: 1,
            page_limit: 1,
            custody_page_limit: 1,
            interval: Duration::from_millis(20),
            round_timeout: Duration::from_secs(2),
            work_limits: WorkLimits {
                reads: minimum.reads * 3,
                writes: minimum.writes * 3,
                bytes: minimum.bytes * 3,
                mutations: minimum.mutations * 3,
            },
            head_probe_every: 2,
        })
    }

    fn run_fallback_round(
        driver: &mut NativeRetentionDriver,
        catalog: &RefusingCatalog,
    ) -> Result<(), IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(2))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(driver.config.work_limits, deadline);
        driver.run_round(catalog, &mut budget)
    }

    #[test]
    fn empty_work_and_views_use_one_custody_page_without_skipping_normal_turn()
    -> Result<(), IndexError> {
        use quanta_index_core::index::{ProducerId, StreamId};
        let scope = fallback_scope(2);
        let mut driver = NativeRetentionDriver::new(fallback_config(vec![scope])?)?;
        let catalog = RefusingCatalog {
            work_page: Some(TerminalWorkRetirementPage {
                after: None,
                examined: 0,
                retired: 0,
                blocked: 0,
                has_more: false,
            }),
            custody_candidate: Some(TerminalCustodyCandidate {
                sequence: 1,
                operation: OperationKey {
                    workspace: scope.workspace,
                    epoch: scope.epoch,
                    producer: ProducerId([5; 16]),
                    stream: StreamId([6; 16]),
                    stream_epoch: 1,
                    sequence: 1,
                },
            }),
            ..RefusingCatalog::default()
        };
        for _ in 0..3 {
            run_fallback_round(&mut driver, &catalog)?;
        }
        let calls = catalog
            .calls
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        if calls.as_slice()
            != [
                (2, "floor"),
                (2, "work"),
                (2, "custody"),
                (2, "release"),
                (2, "custody"),
                (2, "floor"),
                (2, "views"),
                (2, "custody"),
                (2, "release"),
                (2, "custody"),
                (2, "floor"),
                (2, "custody"),
                (2, "release"),
                (2, "custody"),
            ]
        {
            return Err(IndexError::BackendViolation(
                "empty phase did not replay one custody page".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn blocked_work_and_views_or_refusal_do_not_replay_custody() -> Result<(), IndexError> {
        use quanta_index_core::index::Digest;
        let scope = fallback_scope(2);
        let mut driver = NativeRetentionDriver::new(fallback_config(vec![scope])?)?;
        let catalog = RefusingCatalog {
            work_page: Some(TerminalWorkRetirementPage {
                after: Some(WorkRetirementCursor {
                    sequence: 1,
                    work: Digest([7; 32]),
                }),
                examined: 1,
                retired: 0,
                blocked: 1,
                has_more: false,
            }),
            view_page: Some(ViewRetirementPage {
                after: Some(1),
                examined: 1,
                retired: 0,
                blocked: 1,
                has_more: false,
            }),
            ..RefusingCatalog::default()
        };
        run_fallback_round(&mut driver, &catalog)?;
        run_fallback_round(&mut driver, &catalog)?;
        let calls = catalog
            .calls
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        if calls.as_slice() != [(2, "floor"), (2, "work"), (2, "floor"), (2, "views")] {
            return Err(IndexError::BackendViolation(
                "blocked page replayed custody".into(),
            ));
        }
        drop(calls);
        let mut refused = NativeRetentionDriver::new(fallback_config(vec![scope])?)?;
        let catalog = RefusingCatalog {
            refuse_views: true,
            ..RefusingCatalog::default()
        };
        for _ in 0..2 {
            if !matches!(
                run_fallback_round(&mut refused, &catalog),
                Err(IndexError::ResourceLimited)
            ) {
                return Err(IndexError::BackendViolation(
                    "retention refusal changed".into(),
                ));
            }
        }
        let calls = catalog
            .calls
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        if calls.as_slice() != [(2, "floor"), (2, "work"), (2, "floor"), (2, "views")] {
            return Err(IndexError::BackendViolation(
                "refused page replayed custody".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn blocked_primary_borrows_one_page_only_after_same_scope_partial_custody()
    -> Result<(), IndexError> {
        use quanta_index_core::index::{Digest, ProducerId, StreamId};

        let scope = fallback_scope(2);
        let mut driver = NativeRetentionDriver::new(fallback_config(vec![scope])?)?;
        let catalog = RefusingCatalog {
            work_page: Some(TerminalWorkRetirementPage {
                after: Some(WorkRetirementCursor {
                    sequence: 1,
                    work: Digest([7; 32]),
                }),
                examined: 1,
                retired: 0,
                blocked: 1,
                has_more: false,
            }),
            view_page: Some(ViewRetirementPage {
                after: Some(1),
                examined: 1,
                retired: 0,
                blocked: 1,
                has_more: false,
            }),
            custody_candidate: Some(TerminalCustodyCandidate {
                sequence: 1,
                operation: OperationKey {
                    workspace: scope.workspace,
                    epoch: scope.epoch,
                    producer: ProducerId([5; 16]),
                    stream: StreamId([6; 16]),
                    stream_epoch: 1,
                    sequence: 1,
                },
            }),
            custody_has_more: true,
            ..RefusingCatalog::default()
        };
        // Work and Views cannot borrow from a custody backlog that has not
        // been observed by this scope's own scheduled custody page.
        for _ in 0..2 {
            run_fallback_round(&mut driver, &catalog)?;
        }
        {
            let calls = catalog
                .calls
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            if calls.iter().any(|(_, phase)| *phase == "custody") {
                return Err(IndexError::BackendViolation(
                    "unknown custody debt borrowed a page".into(),
                ));
            }
        }
        run_fallback_round(&mut driver, &catalog)?;
        if !driver
            .scopes
            .front()
            .is_some_and(|state| state.custody_unfinished)
        {
            return Err(IndexError::BackendViolation(
                "partial custody did not retain its continuation".into(),
            ));
        }
        for _ in 0..2 {
            run_fallback_round(&mut driver, &catalog)?;
        }
        {
            let calls = catalog
                .calls
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            let phases: Vec<_> = calls
                .iter()
                .filter_map(|(_, phase)| {
                    (*phase == "work" || *phase == "views" || *phase == "release").then_some(*phase)
                })
                .collect();
            if phases
                != [
                    "work", "views", "release", "work", "release", "views", "release",
                ]
            {
                return Err(IndexError::BackendViolation(
                    "blocked primary changed order or borrowed more than one page".into(),
                ));
            }
        }
        // An actual empty Catalog scan revokes the hint. The next blocked
        // primary must not issue another speculative custody page.
        catalog.custody_absent.store(true, Ordering::Release);
        run_fallback_round(&mut driver, &catalog)?;
        if driver
            .scopes
            .front()
            .is_some_and(|state| state.custody_unfinished)
        {
            return Err(IndexError::BackendViolation(
                "empty custody scan retained a stale continuation".into(),
            ));
        }
        let before = catalog
            .calls
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?
            .len();
        run_fallback_round(&mut driver, &catalog)?;
        let calls = catalog
            .calls
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        if calls
            .get(before..)
            .is_none_or(|tail| tail != [(2, "floor"), (2, "work")])
        {
            return Err(IndexError::BackendViolation(
                "stale custody continuation borrowed a blocked primary".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn retired_primary_and_unfunded_blocked_primary_keep_original_page_count()
    -> Result<(), IndexError> {
        use quanta_index_core::index::{Digest, WorkUsage};

        let scope = fallback_scope(2);
        let mut retired = NativeRetentionDriver::new(fallback_config(vec![scope])?)?;
        retired
            .scopes
            .front_mut()
            .ok_or_else(|| IndexError::BackendViolation("retention scope absent".into()))?
            .custody_unfinished = true;
        let retired_catalog = RefusingCatalog {
            work_page: Some(TerminalWorkRetirementPage {
                after: Some(WorkRetirementCursor {
                    sequence: 1,
                    work: Digest([7; 32]),
                }),
                examined: 1,
                retired: 1,
                blocked: 0,
                has_more: false,
            }),
            ..RefusingCatalog::default()
        };
        run_fallback_round(&mut retired, &retired_catalog)?;
        if retired_catalog
            .calls
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?
            .as_slice()
            != [(2, "floor"), (2, "work")]
        {
            return Err(IndexError::BackendViolation(
                "retiring primary borrowed custody".into(),
            ));
        }

        let tight_config = fallback_config(vec![scope])?;
        let limits = tight_config.work_limits;
        let mut tight = NativeRetentionDriver::new(tight_config)?;
        tight
            .scopes
            .front_mut()
            .ok_or_else(|| IndexError::BackendViolation("retention scope absent".into()))?
            .custody_unfinished = true;
        let tight_catalog = RefusingCatalog {
            work_page: Some(TerminalWorkRetirementPage {
                after: Some(WorkRetirementCursor {
                    sequence: 1,
                    work: Digest([7; 32]),
                }),
                examined: 1,
                retired: 0,
                blocked: 1,
                has_more: false,
            }),
            ..RefusingCatalog::default()
        };
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(2))
            .ok_or(IndexError::ResourceLimited)?;
        let mut work = WorkBudget::new(limits, deadline);
        let custody_reads = NativeRetentionConfig::minimum_work_limits(1)?.reads;
        work.charge(WorkUsage {
            reads: limits
                .reads
                .checked_sub(custody_reads)
                .and_then(|remaining| remaining.checked_add(1))
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        tight.run_round(&tight_catalog, &mut work)?;
        if tight_catalog
            .calls
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?
            .as_slice()
            != [(2, "floor"), (2, "work")]
        {
            return Err(IndexError::BackendViolation(
                "unfunded blocked primary borrowed custody".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn idle_replay_keeps_scope_and_scheduled_phase_fairness() -> Result<(), IndexError> {
        let scopes = vec![fallback_scope(2), fallback_scope(3)];
        let mut driver = NativeRetentionDriver::new(fallback_config(scopes)?)?;
        let catalog = RefusingCatalog {
            work_page: Some(TerminalWorkRetirementPage {
                after: None,
                examined: 0,
                retired: 0,
                blocked: 0,
                has_more: false,
            }),
            ..RefusingCatalog::default()
        };
        for _ in 0..6 {
            run_fallback_round(&mut driver, &catalog)?;
        }
        let calls = catalog
            .calls
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        if calls.as_slice()
            != [
                (2, "floor"),
                (2, "work"),
                (2, "custody"),
                (3, "floor"),
                (3, "work"),
                (3, "custody"),
                (2, "floor"),
                (2, "views"),
                (2, "custody"),
                (3, "floor"),
                (3, "views"),
                (3, "custody"),
                (2, "floor"),
                (2, "custody"),
                (3, "floor"),
                (3, "custody"),
            ]
        {
            return Err(IndexError::BackendViolation(
                "idle replay changed scope fairness".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn absent_epoch_and_limited_work_do_not_starve_other_scopes_or_phases() -> Result<(), IndexError>
    {
        use quanta_index_core::index::{CollectionId, SlotId, TenantId, WorktreeId};
        let workspace = WorkspaceKey {
            tenant: TenantId([0; 16]),
            collection: CollectionId([0; 16]),
            worktree: WorktreeId([0; 16]),
            slot: SlotId([0; 16]),
        };
        let limits = NativeRetentionConfig::minimum_work_limits(1)?;
        let config = NativeRetentionConfig {
            scopes: vec![
                NativeRetentionScope {
                    workspace,
                    epoch: 1,
                },
                NativeRetentionScope {
                    workspace,
                    epoch: 2,
                },
            ],
            sequence_window: 2,
            page_limit: 1,
            custody_page_limit: 1,
            interval: Duration::from_millis(20),
            round_timeout: Duration::from_secs(2),
            work_limits: limits,
            head_probe_every: 2,
        };
        let mut driver = NativeRetentionDriver::new(config)?;
        let catalog = RefusingCatalog::default();
        // The independent expected schedule includes failures. A bad scope
        // survives for retry, while the healthy scope cycles past limited work.
        for expected in [0_u8, 1, 0, 2, 0, 2, 0, 1] {
            let deadline = Instant::now()
                .checked_add(Duration::from_secs(2))
                .ok_or(IndexError::ResourceLimited)?;
            let mut budget = WorkBudget::new(limits, deadline);
            let outcome = driver.run_round(&catalog, &mut budget);
            let matches = match expected {
                0 => matches!(outcome, Err(IndexError::SnapshotUnavailable)),
                1 => matches!(outcome, Err(IndexError::ResourceLimited)),
                _ => outcome.is_ok(),
            };
            if !matches {
                return Err(IndexError::BackendViolation(
                    "retention refusal schedule changed".into(),
                ));
            }
        }
        if catalog
            .calls
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?
            .as_slice()
            != [
                (1, "floor"),
                (2, "floor"),
                (2, "work"),
                (1, "floor"),
                (2, "floor"),
                (2, "views"),
                (1, "floor"),
                (2, "floor"),
                (2, "custody"),
                (1, "floor"),
                (2, "floor"),
                (2, "work"),
            ]
        {
            return Err(IndexError::BackendViolation(
                "retention scope or phase was starved".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn custody_page_limit_is_independent_and_funded_by_the_larger_page() -> Result<(), IndexError> {
        use quanta_index_core::index::{
            CollectionId, ProducerId, SlotId, StreamId, TenantId, WorktreeId,
        };

        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let config = NativeRetentionConfig {
            scopes: vec![NativeRetentionScope {
                workspace,
                epoch: 2,
            }],
            sequence_window: 1,
            page_limit: 1,
            custody_page_limit: 8,
            interval: Duration::from_millis(20),
            round_timeout: Duration::from_secs(2),
            work_limits: NativeRetentionConfig::minimum_work_limits(8)?,
            head_probe_every: 2,
        };
        for invalid in [0, 33] {
            if !matches!(
                NativeRetentionDriver::new(NativeRetentionConfig {
                    custody_page_limit: invalid,
                    ..config.clone()
                }),
                Err(IndexError::InvalidInput(_))
            ) {
                return Err(IndexError::BackendViolation(
                    "invalid custody page limit was admitted".into(),
                ));
            }
        }
        if !matches!(
            NativeRetentionDriver::new(NativeRetentionConfig {
                work_limits: NativeRetentionConfig::minimum_work_limits(1)?,
                ..config.clone()
            }),
            Err(IndexError::InvalidInput(_))
        ) {
            return Err(IndexError::BackendViolation(
                "custody page exceeded its funded work envelope".into(),
            ));
        }
        let limits = config.work_limits;
        let mut driver = NativeRetentionDriver::new(config)?;
        let catalog = RefusingCatalog {
            custody_candidate: Some(TerminalCustodyCandidate {
                sequence: 1,
                operation: OperationKey {
                    workspace,
                    epoch: 2,
                    producer: ProducerId([5; 16]),
                    stream: StreamId([6; 16]),
                    stream_epoch: 1,
                    sequence: 1,
                },
            }),
            ..RefusingCatalog::default()
        };
        for expected_failure in [true, false, false] {
            let deadline = Instant::now()
                .checked_add(Duration::from_secs(2))
                .ok_or(IndexError::ResourceLimited)?;
            let outcome = driver.run_round(&catalog, &mut WorkBudget::new(limits, deadline));
            if expected_failure != matches!(outcome, Err(IndexError::ResourceLimited)) {
                return Err(IndexError::BackendViolation(
                    "retention phase result changed under split page limits".into(),
                ));
            }
            outcome.or_else(|error| {
                if expected_failure && error == IndexError::ResourceLimited {
                    Ok(())
                } else {
                    Err(error)
                }
            })?;
        }
        if catalog
            .limits
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?
            .as_slice()
            != [("work", 1), ("views", 1), ("custody", 8)]
        {
            return Err(IndexError::BackendViolation(
                "retention phases shared the wrong page limit".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn frozen_prefix_revisits_lower_arrival_despite_protected_head_and_busy_forward()
    -> Result<(), IndexError> {
        let mut keys = BTreeSet::from([1_u64, 10, 11, 12, 13, 14, 15, 16]);
        let mut sweep = Sweep::new();
        let mut removed_lower = false;
        for round in 0..24_u64 {
            let _inserted = keys.insert(
                100_u64
                    .checked_add(round)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            if round == 3 {
                let _inserted = keys.insert(2);
            }
            let request = sweep.request(2)?;
            let candidate = keys.iter().copied().find(|key| {
                request.after.is_none_or(|after| *key > after)
                    && request.through.is_none_or(|upper| *key <= upper)
            });
            let has_more = candidate.is_some_and(|key| {
                keys.iter()
                    .any(|next| *next > key && request.through.is_none_or(|upper| *next <= upper))
            });
            if let Some(key) = candidate {
                if key != 1 {
                    let _removed = keys.remove(&key);
                }
                removed_lower |= key == 2;
            }
            sweep.accept(request, candidate.or(request.after), has_more, false);
        }
        if !removed_lower || !keys.contains(&1) || keys.contains(&2) {
            return Err(IndexError::BackendViolation(
                "protected head hid lower arrival under busy forward".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn failed_head_page_preserves_original_cursor_and_frozen_frontier() -> Result<(), IndexError> {
        let mut sweep = Sweep::new();
        for key in [10_u64, 20] {
            let request = sweep.request(2)?;
            sweep.accept(request, Some(key), true, false);
        }
        let failed = sweep.request(2)?;
        let retry = sweep.request(2)?;
        if !failed.head
            || failed.after != retry.after
            || failed.through != Some(20)
            || retry.through != Some(20)
        {
            return Err(IndexError::BackendViolation(
                "failed page reset the frozen head frontier".into(),
            ));
        }
        sweep.accept(retry, Some(1), true, false);
        let continued = sweep.request(2)?;
        if continued.after != Some(1) || continued.through != Some(20) {
            return Err(IndexError::BackendViolation(
                "head prefix did not retain its progress".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn hostile_retirement_page_refuses_backwards_missing_and_inconsistent_counts()
    -> Result<(), IndexError> {
        let request = SweepRequest {
            after: Some(10_u64),
            through: None,
            head: false,
        };
        for (after, counts, more) in [
            (Some(9), [1, 1, 0], true),
            (None, [1, 1, 0], false),
            (Some(11), [1, 0, 0], true),
            (Some(10), [0, 0, 0], true),
            (Some(11), [2, 2, 0], false),
        ] {
            if !matches!(
                validate_page(request, after, counts, more, 1),
                Err(IndexError::BackendViolation(_))
            ) {
                return Err(IndexError::BackendViolation(
                    "hostile retirement page was accepted".into(),
                ));
            }
        }
        validate_page(request, Some(11), [1, 0, 1], true, 1)
    }

    /// An endless, always-blocked view history: every page examines the next
    /// sequence and reports more. Records each view request's cursor.
    #[derive(Default)]
    struct EndlessViews {
        view_afters: Mutex<Vec<(u64, Option<u64>)>>,
        /// Epochs whose next Views page fails once.
        fail_views_once: Mutex<BTreeSet<u64>>,
        /// The epoch of every turn, in order: each turn starts at the floor.
        turns: Mutex<Vec<u64>>,
        /// Epochs whose every floor step fails, with the failure.
        fail_floor: Mutex<std::collections::BTreeMap<u64, IndexError>>,
    }

    impl EndlessViews {
        fn turns(&self) -> Result<Vec<u64>, IndexError> {
            Ok(self
                .turns
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?
                .clone())
        }

        fn fail_floor(&self, epoch: u64, failure: Option<IndexError>) -> Result<(), IndexError> {
            let mut failing = self
                .fail_floor
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            match failure {
                Some(failure) => {
                    let _previous = failing.insert(epoch, failure);
                }
                None => {
                    let _previous = failing.remove(&epoch);
                }
            }
            drop(failing);
            Ok(())
        }

        fn pending_ranges(driver: &NativeRetentionDriver, epoch: u64) -> usize {
            driver
                .scopes
                .iter()
                .filter(|state| state.scope.epoch == epoch)
                .map(|state| state.released_views.len())
                .sum()
        }
    }

    impl CatalogExactHistoryPort for EndlessViews {
        fn exact_history_floor(&self, _: WorkspaceKey, _: u64) -> Result<u64, IndexError> {
            Err(IndexError::BackendViolation("unexpected floor read".into()))
        }

        fn advance_exact_history_floor(
            &self,
            _: WorkspaceKey,
            _: u64,
            _: u64,
            _: &mut WorkBudget,
        ) -> Result<u64, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected floor advance".into(),
            ))
        }

        fn retain_exact_history_tail(
            &self,
            _: WorkspaceKey,
            epoch: u64,
            _: u64,
            _: &mut WorkBudget,
        ) -> Result<u64, IndexError> {
            self.turns
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?
                .push(epoch);
            if let Some(failure) = self
                .fail_floor
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?
                .get(&epoch)
            {
                return Err(failure.clone());
            }
            Ok(1_000)
        }

        fn retire_exact_history_page(
            &self,
            _workspace: WorkspaceKey,
            epoch: u64,
            after: Option<u64>,
            _limit: u32,
            _budget: &mut WorkBudget,
        ) -> Result<ViewRetirementPage, IndexError> {
            self.view_afters
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?
                .push((epoch, after));
            if self
                .fail_views_once
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?
                .remove(&epoch)
            {
                return Err(IndexError::ResourceLimited);
            }
            Ok(ViewRetirementPage {
                after: Some(after.map_or(0, |sequence| sequence.saturating_add(1))),
                examined: 1,
                retired: 0,
                blocked: 1,
                has_more: true,
            })
        }

        fn retire_exact_history_page_through(
            &self,
            workspace: WorkspaceKey,
            epoch: u64,
            after: Option<u64>,
            through: u64,
            limit: u32,
            budget: &mut WorkBudget,
        ) -> Result<ViewRetirementPage, IndexError> {
            let page = self.retire_exact_history_page(workspace, epoch, after, limit, budget)?;
            Ok(bounded_view_page(after, through, page))
        }
    }

    impl CatalogTerminalWorkRetentionPort for EndlessViews {
        fn retire_terminal_work_page(
            &self,
            _: WorkspaceKey,
            _: u64,
            after: Option<WorkRetirementCursor>,
            _: u32,
            _: &mut WorkBudget,
        ) -> Result<TerminalWorkRetirementPage, IndexError> {
            Ok(TerminalWorkRetirementPage {
                after,
                examined: 0,
                retired: 0,
                blocked: 0,
                has_more: false,
            })
        }
    }

    impl CatalogCustodyMaintenancePort for EndlessViews {
        fn next_terminal_custody(
            &self,
            _: WorkspaceKey,
            _: u64,
            _: Option<u64>,
            _: Option<u64>,
            _: &mut WorkBudget,
        ) -> Result<Option<TerminalCustodyCandidate>, IndexError> {
            Ok(None)
        }

        fn release_terminal_custody(
            &self,
            _: OperationKey,
            _: u32,
            _: &mut WorkBudget,
        ) -> Result<TerminalCustodyPage, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected custody release".into(),
            ))
        }
    }

    /// A release starts its own range on the next Views turn and preserves
    /// the ordinary forward cursor.
    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test assertions report the observed view cursors on mismatch"
    )]
    fn released_view_prioritizes_its_sequence_and_preserves_ordinary_cursor()
    -> Result<(), IndexError> {
        let first = fallback_scope(2);
        let second = fallback_scope(3);
        let mut config = fallback_config(vec![first, second])?;
        config.head_probe_every = 64;
        let mut driver = NativeRetentionDriver::new(config)?;
        let catalog = EndlessViews::default();
        let run = |driver: &mut NativeRetentionDriver| -> Result<(), IndexError> {
            let deadline = Instant::now()
                .checked_add(Duration::from_secs(2))
                .ok_or(IndexError::ResourceLimited)?;
            let mut budget = WorkBudget::new(driver.config.work_limits, deadline);
            driver.run_round(&catalog, &mut budget)
        };
        let afters = |catalog: &EndlessViews| -> Result<Vec<(u64, Option<u64>)>, IndexError> {
            Ok(catalog
                .view_afters
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?
                .clone())
        };
        // Two scopes alternate; phases rotate Work, Views, Custody. Twelve
        // rounds give each scope two forward Views turns.
        for _ in 0..12 {
            run(&mut driver)?;
        }
        assert_eq!(
            afters(&catalog)?,
            [(2, None), (3, None), (2, Some(0)), (3, Some(0))]
        );
        // Scope 2 keeps its scheduled turn. Scope 3 takes a release page
        // while preserving its previous Views cursor.
        driver.schedule(&MaintenanceDemand {
            released_views: vec![release_range(second.epoch, 8, 8), release_range(9, 8, 8)],
            ..MaintenanceDemand::default()
        });
        run(&mut driver)?;
        assert_eq!(afters(&catalog)?.len(), 4);
        run(&mut driver)?;
        assert_eq!(afters(&catalog)?.last(), Some(&(3, Some(7))));
        assert_eq!(afters(&catalog)?.len(), 5);
        let ordinary = driver
            .scopes
            .iter()
            .find(|state| state.scope == second)
            .and_then(|state| state.views.forward);
        assert_eq!(ordinary, Some(1));
        Ok(())
    }

    fn release_range(epoch: u64, first_sequence: u64, last_sequence: u64) -> ReleasedViewRange {
        ReleasedViewRange {
            workspace: fallback_scope(epoch).workspace,
            epoch,
            first_sequence,
            last_sequence,
        }
    }

    fn release(epochs: &[u64]) -> MaintenanceDemand {
        MaintenanceDemand {
            released_views: epochs
                .iter()
                .map(|epoch| release_range(*epoch, 0, 0))
                .collect(),
            ..MaintenanceDemand::default()
        }
    }

    fn view_round(
        driver: &mut NativeRetentionDriver,
        catalog: &EndlessViews,
    ) -> Result<(), IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(2))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(driver.config.work_limits, deadline);
        driver.run_round(catalog, &mut budget)
    }

    fn view_afters(catalog: &EndlessViews) -> Result<Vec<(u64, Option<u64>)>, IndexError> {
        Ok(catalog
            .view_afters
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?
            .clone())
    }

    /// Release arrival order leaves the configured scope rotation intact.
    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test assertions report the observed view cursors on mismatch"
    )]
    fn released_scopes_run_in_configured_rotation() -> Result<(), IndexError> {
        let mut config = fallback_config(vec![
            fallback_scope(2),
            fallback_scope(3),
            fallback_scope(4),
        ])?;
        config.head_probe_every = 64;
        let mut driver = NativeRetentionDriver::new(config)?;
        let catalog = EndlessViews::default();
        // Three scopes, three phases: nine rounds give each scope one forward
        // Views turn, so every later unscheduled Views turn starts at Some(0).
        for _ in 0..9 {
            view_round(&mut driver, &catalog)?;
        }
        assert_eq!(view_afters(&catalog)?, [(2, None), (3, None), (4, None)]);
        driver.schedule(&release(&[4]));
        driver.schedule(&release(&[2]));
        driver.schedule(&release(&[2]));
        for _ in 0..3 {
            view_round(&mut driver, &catalog)?;
        }
        assert_eq!(
            view_afters(&catalog)?.get(3..),
            Some([(2, None), (4, None)].as_slice())
        );
        Ok(())
    }

    /// A failed release keeps its cursor and retries after a regular turn.
    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test assertions report the observed view cursors on mismatch"
    )]
    fn failed_release_round_retries_after_a_regular_turn() -> Result<(), IndexError> {
        let mut config = fallback_config(vec![fallback_scope(2), fallback_scope(3)])?;
        config.head_probe_every = 64;
        let mut driver = NativeRetentionDriver::new(config)?;
        let catalog = EndlessViews::default();
        for _ in 0..6 {
            view_round(&mut driver, &catalog)?;
        }
        assert_eq!(view_afters(&catalog)?, [(2, None), (3, None)]);
        let _armed = catalog
            .fail_views_once
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?
            .insert(3);
        driver.schedule(&release(&[3]));
        view_round(&mut driver, &catalog)?;
        assert_eq!(
            view_round(&mut driver, &catalog),
            Err(IndexError::ResourceLimited)
        );
        for _ in 0..3 {
            view_round(&mut driver, &catalog)?;
        }
        assert_eq!(
            view_afters(&catalog)?.get(2..),
            Some([(3, None), (2, Some(0))].as_slice())
        );
        view_round(&mut driver, &catalog)?;
        assert_eq!(
            view_afters(&catalog)?.get(2..),
            Some([(3, None), (2, Some(0)), (3, None)].as_slice())
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "report the observed phase order on mismatch"
    )]
    fn sustained_releases_preserve_every_scope_and_regular_phase() -> Result<(), IndexError> {
        use quanta_index_core::index::Digest;
        let scopes = vec![fallback_scope(2), fallback_scope(3)];
        let mut driver = NativeRetentionDriver::new(fallback_config(scopes)?)?;
        let catalog = RefusingCatalog {
            work_page: Some(TerminalWorkRetirementPage {
                after: Some(WorkRetirementCursor {
                    sequence: 1,
                    work: Digest([7; 32]),
                }),
                examined: 1,
                retired: 1,
                blocked: 0,
                has_more: false,
            }),
            view_page: Some(ViewRetirementPage {
                after: Some(0),
                examined: 1,
                retired: 1,
                blocked: 0,
                has_more: false,
            }),
            ..RefusingCatalog::default()
        };
        for _ in 0..12 {
            driver.schedule(&release(&[2, 3]));
            run_fallback_round(&mut driver, &catalog)?;
        }
        let phases: Vec<_> = catalog
            .calls
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?
            .iter()
            .copied()
            .filter(|(_, phase)| *phase != "floor")
            .collect();
        assert_eq!(
            phases,
            ["views", "work", "views", "views", "views", "custody"]
                .into_iter()
                .flat_map(|phase| [(2, phase), (3, phase)])
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "report the release cursors on mismatch"
    )]
    fn sparse_releases_reach_later_views_without_rewinding() -> Result<(), IndexError> {
        let mut driver = NativeRetentionDriver::new(fallback_config(vec![fallback_scope(2)])?)?;
        let catalog = EndlessViews::default();
        driver.schedule(&MaintenanceDemand {
            released_views: vec![release_range(2, 5, 7), release_range(2, 1_000, 1_000)],
            ..MaintenanceDemand::default()
        });
        view_round(&mut driver, &catalog)?;
        let earlier = MaintenanceDemand {
            released_views: vec![release_range(2, 3, 3)],
            ..MaintenanceDemand::default()
        };
        for _ in 0..7 {
            driver.schedule(&earlier);
            view_round(&mut driver, &catalog)?;
        }
        for _ in 0..3 {
            view_round(&mut driver, &catalog)?;
        }
        assert_eq!(
            view_afters(&catalog)?,
            [Some(4), Some(5), None, Some(6), Some(999), Some(2), Some(0),].map(|after| (2, after))
        );
        assert!(
            driver
                .scopes
                .iter()
                .all(|state| state.released_views.is_empty())
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "report the retention phase trace on mismatch"
    )]
    fn refused_spare_custody_does_not_repeat_an_accepted_release_page() -> Result<(), IndexError> {
        let mut driver = NativeRetentionDriver::new(fallback_config(vec![fallback_scope(2)])?)?;
        driver.schedule(&release(&[2]));
        let refusing = RefusingCatalog {
            refuse_custody: true,
            ..RefusingCatalog::default()
        };
        assert_eq!(
            run_fallback_round(&mut driver, &refusing),
            Err(IndexError::ResourceLimited)
        );
        assert_eq!(
            *refusing
                .calls
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?,
            [(2, "floor"), (2, "views"), (2, "custody")]
        );

        let recovered = RefusingCatalog {
            work_page: Some(TerminalWorkRetirementPage {
                after: None,
                examined: 0,
                retired: 0,
                blocked: 0,
                has_more: false,
            }),
            ..RefusingCatalog::default()
        };
        run_fallback_round(&mut driver, &recovered)?;
        assert_eq!(
            *recovered
                .calls
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?,
            [(2, "floor"), (2, "work"), (2, "custody")]
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "report the retention phase trace on mismatch"
    )]
    fn unavailable_released_scope_does_not_starve_other_scopes() -> Result<(), IndexError> {
        let scopes = vec![fallback_scope(1), fallback_scope(2)];
        let mut driver = NativeRetentionDriver::new(fallback_config(scopes)?)?;
        let catalog = RefusingCatalog {
            work_page: Some(TerminalWorkRetirementPage {
                after: None,
                examined: 0,
                retired: 0,
                blocked: 0,
                has_more: false,
            }),
            ..RefusingCatalog::default()
        };

        driver.schedule(&release(&[1]));
        for _ in 0..2 {
            assert_eq!(
                run_fallback_round(&mut driver, &catalog),
                Err(IndexError::SnapshotUnavailable)
            );
            run_fallback_round(&mut driver, &catalog)?;
        }
        let calls = catalog
            .calls
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let floor_epochs: Vec<_> = calls
            .iter()
            .filter(|(_, phase)| *phase == "floor")
            .map(|(epoch, _)| *epoch)
            .collect();
        assert_eq!(floor_epochs, [1, 2, 1, 2]);
        assert!(calls.contains(&(2, "views")));
        Ok(())
    }

    /// A released scope whose every turn fails keeps its range and its place
    /// in the rotation: the other scopes keep their Work, Views and Custody
    /// turns.
    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test assertions report the observed turns on mismatch"
    )]
    fn permanently_failing_released_scope_does_not_starve_other_scopes() -> Result<(), IndexError> {
        let mut config = fallback_config(vec![
            fallback_scope(2),
            fallback_scope(3),
            fallback_scope(4),
        ])?;
        config.head_probe_every = 64;
        let mut driver = NativeRetentionDriver::new(config)?;
        let catalog = EndlessViews::default();
        catalog.fail_floor(3, Some(IndexError::ResourceLimited))?;
        driver.schedule(&release(&[3]));
        for _ in 0..12 {
            let _turn = view_round(&mut driver, &catalog);
        }
        assert_eq!(catalog.turns()?, [2, 3, 4, 2, 3, 4, 2, 3, 4, 2, 3, 4]);
        // Scopes 2 and 4 ran Work, Views, Custody and Work: one Views turn each.
        assert_eq!(view_afters(&catalog)?, [(2, None), (4, None)]);
        assert_eq!(EndlessViews::pending_ranges(&driver, 3), 1);
        Ok(())
    }

    /// An obsolete release range is cleared, not retried.
    ///
    /// A range whose scope epoch is no longer the workspace's current epoch
    /// can never be served: the floor step refuses with `SnapshotUnavailable`
    /// on every turn. The range is cleared on that typed refusal.
    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test assertions report the observed turns on mismatch"
    )]
    fn obsolete_release_range_is_cleared_not_retried() -> Result<(), IndexError> {
        let mut config = fallback_config(vec![fallback_scope(2), fallback_scope(3)])?;
        config.head_probe_every = 64;
        let mut driver = NativeRetentionDriver::new(config)?;
        let catalog = EndlessViews::default();
        catalog.fail_floor(3, Some(IndexError::SnapshotUnavailable))?;
        driver.schedule(&release(&[3]));
        view_round(&mut driver, &catalog)?;
        assert!(driver.last_round().releases_pending);
        assert_eq!(
            view_round(&mut driver, &catalog),
            Err(IndexError::SnapshotUnavailable)
        );
        assert_eq!(EndlessViews::pending_ranges(&driver, 3), 0);
        assert!(!driver.last_round().releases_pending);
        assert_eq!(catalog.turns()?, [2, 3]);
        Ok(())
    }

    /// A pass is stalled exactly when none of its turns completed.
    ///
    /// A pass ends when every configured scope has taken a turn in it. One
    /// scope that always fails cannot stall a pass in which another scope
    /// completes.
    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test assertions report the observed rounds on mismatch"
    )]
    fn a_pass_is_stalled_only_when_no_turn_in_it_completed() -> Result<(), IndexError> {
        let mut config = fallback_config(vec![fallback_scope(2), fallback_scope(3)])?;
        config.head_probe_every = 64;
        let mut driver = NativeRetentionDriver::new(config)?;
        let catalog = EndlessViews::default();
        catalog.fail_floor(3, Some(IndexError::ResourceLimited))?;
        let mut stalled = Vec::new();
        for _ in 0..4 {
            let _turn = view_round(&mut driver, &catalog);
            stalled.push(driver.last_round().stalled_pass);
        }
        assert_eq!(stalled, [false, false, false, false]);
        catalog.fail_floor(2, Some(IndexError::ResourceLimited))?;
        for _ in 0..4 {
            let _turn = view_round(&mut driver, &catalog);
            let round = driver.last_round();
            assert!(!round.completed);
            stalled.push(round.stalled_pass);
        }
        assert_eq!(
            stalled.get(4..),
            Some([false, true, false, true].as_slice())
        );
        assert_eq!(catalog.turns()?, [2, 3, 2, 3, 2, 3, 2, 3]);
        Ok(())
    }
}
