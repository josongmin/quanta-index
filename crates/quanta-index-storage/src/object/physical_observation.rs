//! Bounded per-packet observation of disjoint physical object-install work.

use quanta_index_core::{
    DiagnosticContextV1, DiagnosticCounterV1, DiagnosticHealthCodeV1,
    DiagnosticObjectInstallPhaseV1, DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1,
    index::{MemoryLease, MemoryLifetime, ObjectRef, RegisteredObject, WorkBudget},
};

use super::{BatchReferences, FileObjectStore, InstallGateGuards, PreparedInstallGroup};

// Bound the extra diagnostic work and temporary pointer backing independently
// of the largest legal physical install group.
const MAX_OBSERVED_CHILD_REFERENCES: usize = 16_384;

fn external_child_shape(
    budget: &WorkBudget,
    objects: &[RegisteredObject],
    local_refs: &BatchReferences,
    held_gates: &InstallGateGuards<'_>,
) -> Option<[u64; 3]> {
    let count = objects.iter().try_fold(0usize, |count, object| {
        count.checked_add(object.children.len())
    })?;
    if count == 0 || count > MAX_OBSERVED_CHILD_REFERENCES {
        return None;
    }
    let backing = u64::try_from(count.checked_mul(std::mem::size_of::<&ObjectRef>())?).ok()?;
    // The optional allocation drops before physical validation. The census
    // never charges Work or supplies authentication evidence to the installer.
    let Ok(Some(_memory)) =
        budget.try_reserve_memory_for_optimization(backing, MemoryLifetime::Temporary)
    else {
        return None;
    };
    let mut external = Vec::new();
    external.try_reserve_exact(count).ok()?;
    if external.capacity() > count {
        return None;
    }
    for object in objects {
        for child in &object.children {
            if !local_refs.contains(*child).ok()? {
                external.push(child);
            }
        }
    }
    external.sort_unstable();
    let mut repeated = 0;
    let mut guarded_repeated = 0;
    for (first, second) in external.iter().zip(external.iter().skip(1)) {
        if first == second {
            repeated += 1;
            guarded_repeated +=
                u64::from(held_gates[FileObjectStore::gate_index(**first)].is_some());
        }
    }
    let external_count = u64::try_from(external.len()).ok()?;
    Some([external_count, external_count - repeated, guarded_repeated])
}

#[derive(Default)]
pub(super) struct ChildCheckCounts {
    pub(super) reused: u64,
    pub(super) cross_group: Option<CrossGroupChildCounts>,
}

#[derive(Default)]
pub(super) struct CrossGroupChildCounts {
    planned_overlap: u64,
    pub(super) repeated_path_checks: u64,
    pub(super) path_checks: u64,
}

/// Diagnostic join of the current group's guarded external children with
/// those of earlier, completed physical groups under the same gate union.
#[derive(Default)]
pub(super) struct CrossGroupChildOverlap<'a> {
    references: Vec<&'a ObjectRef>,
    _memory: Option<MemoryLease>,
}

impl<'a> CrossGroupChildOverlap<'a> {
    pub(super) fn measure(
        budget: &WorkBudget,
        previous: &'a [Option<PreparedInstallGroup>],
        current: &'a PreparedInstallGroup,
        gates: &InstallGateGuards<'_>,
    ) -> Option<Self> {
        if previous.is_empty() {
            return Some(Self::default());
        }
        let mut groups = previous.iter().map(Option::as_ref).chain([Some(current)]);
        let capacity = groups.try_fold(0usize, |capacity, group| {
            let count = group?
                .metadata
                .as_slice()
                .iter()
                .try_fold(0usize, |count, object| {
                    count.checked_add(object.children.len())
                })?;
            if count > MAX_OBSERVED_CHILD_REFERENCES {
                return None;
            }
            capacity.checked_add(count)
        })?;
        if capacity == 0 {
            return Some(Self::default());
        }
        let bytes = u64::try_from(capacity.checked_mul(std::mem::size_of::<&ObjectRef>())?).ok()?;
        let memory = budget
            .try_reserve_memory_for_optimization(bytes, MemoryLifetime::Temporary)
            .ok()??;
        let mut references = Vec::new();
        references.try_reserve_exact(capacity).ok()?;
        if references.capacity() > capacity {
            return None;
        }
        for group in previous {
            Self::visit_guarded_external(group.as_ref()?, gates, |reference| {
                references.push(reference);
            })?;
        }
        references.sort_unstable();
        references.dedup();
        let earlier = references.len();
        Self::visit_guarded_external(current, gates, |reference| {
            if references[..earlier].binary_search(&reference).is_ok() {
                references.push(reference);
            }
        })?;
        references.copy_within(earlier.., 0);
        references.truncate(references.len() - earlier);
        references.sort_unstable();
        references.dedup();
        Some(Self {
            references,
            _memory: Some(memory),
        })
    }

    fn visit_guarded_external(
        group: &'a PreparedInstallGroup,
        gates: &InstallGateGuards<'_>,
        mut visit: impl FnMut(&'a ObjectRef),
    ) -> Option<()> {
        for object in group.metadata.as_slice() {
            for reference in &object.children {
                if gates[FileObjectStore::gate_index(*reference)].is_some()
                    && !group.references.contains(*reference).ok()?
                {
                    visit(reference);
                }
            }
        }
        Some(())
    }

    pub(super) fn counts(&self) -> CrossGroupChildCounts {
        CrossGroupChildCounts {
            planned_overlap: self.references.len() as u64,
            ..CrossGroupChildCounts::default()
        }
    }

    pub(super) fn contains(&self, reference: &ObjectRef) -> bool {
        self.references.binary_search(&reference).is_ok()
    }
}

#[derive(Clone, Copy)]
struct PhaseCell {
    calls: u64,
    elapsed_ns: Option<u64>,
    thread_cpu_ns: Option<u64>,
    failures: u64,
}

struct Sample {
    started_ns: u64,
    ended_ns: u64,
    started_cpu_ns: Option<u64>,
    ended_cpu_ns: Option<u64>,
    same_thread: bool,
    failed: bool,
}

const fn phase_index(phase: DiagnosticObjectInstallPhaseV1) -> usize {
    match phase {
        DiagnosticObjectInstallPhaseV1::ChildValidation => 0,
        DiagnosticObjectInstallPhaseV1::NamespaceAndCreate => 1,
        DiagnosticObjectInstallPhaseV1::Write => 2,
        DiagnosticObjectInstallPhaseV1::FileSync => 3,
        DiagnosticObjectInstallPhaseV1::PublishAndRetire => 4,
        DiagnosticObjectInstallPhaseV1::DirectorySync => 5,
        DiagnosticObjectInstallPhaseV1::Barrier => 6,
        DiagnosticObjectInstallPhaseV1::Settlement => 7,
        DiagnosticObjectInstallPhaseV1::Existing => 8,
        DiagnosticObjectInstallPhaseV1::NamespaceValidation => 9,
        DiagnosticObjectInstallPhaseV1::WriterCreate => 10,
        DiagnosticObjectInstallPhaseV1::WriterTicketPrepare => 11,
        DiagnosticObjectInstallPhaseV1::WriterOpen => 12,
        DiagnosticObjectInstallPhaseV1::WriterCreatedWitness => 13,
    }
}

impl Default for PhaseCell {
    fn default() -> Self {
        Self {
            calls: 0,
            elapsed_ns: Some(0),
            thread_cpu_ns: Some(0),
            failures: 0,
        }
    }
}

pub(super) struct PhysicalObservation {
    context: DiagnosticContextV1,
    span: Option<DiagnosticSpanGuardV1>,
    cells: [PhaseCell; DiagnosticObjectInstallPhaseV1::ALL.len()],
    flushed: bool,
}

impl PhysicalObservation {
    pub(super) fn new(budget: &WorkBudget, managed: bool, inputs: usize) -> Option<Self> {
        if !managed {
            return None;
        }
        let context = budget.request().diagnostic_context()?.clone();
        let span = FileObjectStore::begin_managed_custody_span(
            budget,
            DiagnosticStageV1::ManagedObjectInstall,
            inputs,
        )?;
        Some(Self {
            context,
            span: Some(span),
            cells: [PhaseCell::default(); DiagnosticObjectInstallPhaseV1::ALL.len()],
            flushed: false,
        })
    }

    pub(super) fn child_context(&self) -> Option<DiagnosticContextV1> {
        self.span
            .as_ref()
            .map(|span| self.context.child_context(span))
    }

    pub(super) fn observe_external_children(
        &self,
        budget: &WorkBudget,
        objects: &[RegisteredObject],
        local_refs: &BatchReferences,
        held_gates: &InstallGateGuards<'_>,
    ) {
        let Some(span) = self.span.as_ref() else {
            return;
        };
        let Some(values) = external_child_shape(budget, objects, local_refs, held_gates) else {
            return;
        };
        let keys = [
            DiagnosticCounterV1::ManagedObjectExternalChildReferences,
            DiagnosticCounterV1::ManagedObjectUniqueExternalChildren,
            DiagnosticCounterV1::ManagedObjectGuardedRepeatedExternalChildren,
        ];
        for (key, value) in keys.into_iter().zip(values) {
            self.context.counter(span, key, value);
        }
    }

    pub(super) fn observe_child_checks(&self, counts: &ChildCheckCounts) {
        if let Some(span) = self.span.as_ref() {
            self.context.counter(
                span,
                DiagnosticCounterV1::ManagedObjectReusedExternalChildValidations,
                counts.reused,
            );
            if let Some(cross_group) = &counts.cross_group {
                for (key, count) in [
                    (
                        DiagnosticCounterV1::ManagedObjectCrossGroupExternalChildOverlap,
                        cross_group.planned_overlap,
                    ),
                    (
                        DiagnosticCounterV1::ManagedObjectCrossGroupRepeatedChildPathChecks,
                        cross_group.repeated_path_checks,
                    ),
                    (
                        DiagnosticCounterV1::ManagedObjectChildPathChecks,
                        cross_group.path_checks,
                    ),
                ] {
                    self.context.counter(span, key, count);
                }
            }
        }
    }

    fn record(&mut self, phase: DiagnosticObjectInstallPhaseV1, sample: &Sample) {
        let elapsed = sample.ended_ns.checked_sub(sample.started_ns);
        let cpu = if sample.same_thread {
            sample
                .started_cpu_ns
                .zip(sample.ended_cpu_ns)
                .and_then(|(start, end)| end.checked_sub(start))
        } else {
            None
        };
        if elapsed.is_none() {
            self.context.health(DiagnosticHealthCodeV1::ClockRegressed);
        }
        if sample.same_thread
            && sample.started_cpu_ns.zip(sample.ended_cpu_ns).is_some()
            && cpu.is_none()
        {
            self.context
                .health(DiagnosticHealthCodeV1::CounterRegressed);
        }
        if !sample.same_thread {
            self.context
                .health(DiagnosticHealthCodeV1::CounterUnavailable);
        }
        let mut overflow = false;
        let Some(cell) = self.cells.get_mut(phase_index(phase)) else {
            self.context.health(DiagnosticHealthCodeV1::CounterOverflow);
            return;
        };
        if let Some(value) = cell.calls.checked_add(1) {
            cell.calls = value;
        } else {
            overflow = true;
        }
        if sample.failed {
            if let Some(value) = cell.failures.checked_add(1) {
                cell.failures = value;
            } else {
                overflow = true;
            }
        }
        if let (Some(sum), Some(item)) = (cell.elapsed_ns, elapsed) {
            cell.elapsed_ns = sum.checked_add(item);
            overflow |= cell.elapsed_ns.is_none();
        } else {
            cell.elapsed_ns = None;
        }
        if let (Some(sum), Some(item)) = (cell.thread_cpu_ns, cpu) {
            cell.thread_cpu_ns = sum.checked_add(item);
            overflow |= cell.thread_cpu_ns.is_none();
        } else {
            cell.thread_cpu_ns = None;
        }
        if overflow {
            self.context.health(DiagnosticHealthCodeV1::CounterOverflow);
        }
    }

    pub(super) fn measure<T, E>(
        &mut self,
        phase: DiagnosticObjectInstallPhaseV1,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let thread = std::thread::current().id();
        let started_cpu_ns = self.context.clock_thread_cpu_now_ns();
        let started_ns = self.context.clock_now_ns();
        let result = operation();
        let ended_ns = self.context.clock_now_ns();
        let ended_cpu_ns = self.context.clock_thread_cpu_now_ns();
        self.record(
            phase,
            &Sample {
                started_ns,
                ended_ns,
                started_cpu_ns,
                ended_cpu_ns,
                same_thread: thread == std::thread::current().id(),
                failed: result.is_err(),
            },
        );
        result
    }

    fn flush(&mut self) {
        if self.flushed {
            return;
        }
        self.flushed = true;
        let Some(span) = self.span.as_ref() else {
            return;
        };
        for (phase, cell) in DiagnosticObjectInstallPhaseV1::ALL
            .into_iter()
            .zip(self.cells)
        {
            if cell.calls > 0
                && let Some(elapsed_ns) = cell.elapsed_ns
            {
                self.context.object_install_aggregate(
                    span,
                    phase,
                    cell.calls,
                    elapsed_ns,
                    cell.thread_cpu_ns,
                    cell.failures,
                );
            }
        }
    }

    pub(super) fn finish<T, E>(&mut self, result: &Result<T, E>) {
        self.flush();
        if let Some(span) = self.span.take() {
            let _finished = span.finish(DiagnosticTerminalV1::from_result(result));
        }
    }
}

impl Drop for PhysicalObservation {
    fn drop(&mut self) {
        self.flush();
        // An unfinished parent guard emits Aborted from its own Drop.
    }
}

pub(super) fn measure_physical<T, E>(
    observation: &mut Option<PhysicalObservation>,
    phase: DiagnosticObjectInstallPhaseV1,
    operation: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    match observation {
        Some(observation) => observation.measure(phase, operation),
        None => operation(),
    }
}

#[cfg(test)]
#[path = "physical_observation/tests.rs"]
mod tests;
