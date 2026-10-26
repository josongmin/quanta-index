//! Service-owned bounded retirement; Catalog remains the lifetime authority.

use std::{
    collections::{BTreeSet, VecDeque},
    time::Duration,
};

use quanta_index_core::index::{
    IndexError, WorkBudget, WorkLimits, WorkspaceKey,
    ports::{
        CatalogCustodyMaintenancePort, CatalogExactHistoryPort, CatalogTerminalWorkRetentionPort,
        WorkRetirementCursor,
    },
};

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
        let minimum = Self::minimum_work_limits(self.page_limit)?;
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

struct ScopeState {
    scope: NativeRetentionScope,
    phase: Phase,
    work: Sweep<WorkRetirementCursor>,
    views: Sweep<u64>,
    custody: Sweep<u64>,
}

pub(crate) struct NativeRetentionDriver {
    config: NativeRetentionConfig,
    scopes: VecDeque<ScopeState>,
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
            })
            .collect();
        Ok(Self { config, scopes })
    }

    pub(crate) fn config(&self) -> &NativeRetentionConfig {
        &self.config
    }

    /// One scope, one retirement phase and one original budget per round.
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
        let result = self.run_scope(catalog, &mut state, budget);
        // Reinsert even on refusal: a single absent epoch/limited page must
        // not erase configuration or keep other scopes behind itself.
        self.scopes.push_back(state);
        result
    }

    fn run_scope<C>(
        &self,
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
        let _floor = catalog.retain_exact_history_tail(
            workspace,
            epoch,
            self.config.sequence_window,
            budget,
        )?;
        let phase = state.phase;
        state.phase = match phase {
            Phase::Work => Phase::Views,
            Phase::Views => Phase::Custody,
            Phase::Custody => Phase::Work,
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
            }
            Phase::Views => {
                let request = state.views.request(self.config.head_probe_every)?;
                let page = catalog.retire_exact_history_page(
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
                    .views
                    .accept(request, page.after, page.has_more, reached);
            }
            Phase::Custody => {
                let request = state.custody.request(self.config.head_probe_every)?;
                let candidate = catalog.next_terminal_custody(
                    workspace,
                    epoch,
                    request.after,
                    request.through,
                    budget,
                )?;
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
                        self.config.page_limit,
                        budget,
                    )?;
                    if released.released > self.config.page_limit
                        || (released.has_more && released.released == 0)
                    {
                        return Err(IndexError::BackendViolation(
                            "custody release did not make bounded progress".into(),
                        ));
                    }
                    // Partial custody advances this traversal too; the next
                    // finite head/sweep revisit resumes the same durable row.
                    let more = catalog
                        .next_terminal_custody(
                            workspace,
                            epoch,
                            Some(sequence),
                            request.through,
                            budget,
                        )?
                        .is_some();
                    (Some(sequence), more)
                } else {
                    (request.after, false)
                };
                state.custody.accept(request, after, has_more, false);
            }
        }
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
    use std::{sync::Mutex, time::Instant};

    /// Faults occur at real driver boundaries, not at the sweep helper.
    #[derive(Default)]
    struct RefusingCatalog {
        calls: Mutex<Vec<(u64, &'static str)>>,
    }

    impl RefusingCatalog {
        fn record(&self, epoch: u64, phase: &'static str) -> Result<(), IndexError> {
            self.calls
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?
                .push((epoch, phase));
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
            _limit: u32,
            _budget: &mut WorkBudget,
        ) -> Result<ViewRetirementPage, IndexError> {
            self.record(epoch, "views")?;
            Ok(ViewRetirementPage {
                after,
                examined: 0,
                retired: 0,
                blocked: 0,
                has_more: false,
            })
        }
    }

    impl CatalogTerminalWorkRetentionPort for RefusingCatalog {
        fn retire_terminal_work_page(
            &self,
            _workspace: WorkspaceKey,
            epoch: u64,
            _after: Option<WorkRetirementCursor>,
            _limit: u32,
            _budget: &mut WorkBudget,
        ) -> Result<TerminalWorkRetirementPage, IndexError> {
            self.record(epoch, "work")?;
            Err(IndexError::ResourceLimited)
        }
    }

    impl CatalogCustodyMaintenancePort for RefusingCatalog {
        fn next_terminal_custody(
            &self,
            _workspace: WorkspaceKey,
            epoch: u64,
            _after: Option<u64>,
            _through: Option<u64>,
            _budget: &mut WorkBudget,
        ) -> Result<Option<TerminalCustodyCandidate>, IndexError> {
            self.record(epoch, "custody")?;
            Ok(None)
        }

        fn release_terminal_custody(
            &self,
            _key: OperationKey,
            _limit: u32,
            _budget: &mut WorkBudget,
        ) -> Result<TerminalCustodyPage, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected custody release".into(),
            ))
        }
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
}
