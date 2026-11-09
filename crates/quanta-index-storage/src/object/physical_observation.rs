//! Bounded per-packet observation of disjoint physical object-install work.

use quanta_index_core::{
    DiagnosticContextV1, DiagnosticHealthCodeV1, DiagnosticObjectInstallPhaseV1,
    DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1, index::WorkBudget,
};

use super::FileObjectStore;

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
