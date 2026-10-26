use super::IndexError;
use std::{
    fmt,
    sync::{
        OnceLock,
        atomic::{AtomicU8, Ordering},
    },
    time::Instant,
};

const DENIAL_NONE: u8 = 0;
const DENIAL_CANCELLED: u8 = 1;
const DENIAL_DEADLINE: u8 = 2;
const DENIAL_READS: u8 = 3;
const DENIAL_WRITES: u8 = 4;
const DENIAL_BYTES: u8 = 5;
const DENIAL_MUTATIONS: u8 = 6;
const DENIAL_MEMORY: u8 = 7;
const MAX_EXTERNAL_REASON_BYTES: usize = 256;

/// The first refusal produced by this local work budget. This is not the
/// terminal cause of an enclosing operation or permission to retry its work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkBudgetDenial {
    Cancelled,
    Deadline,
    Reads,
    Writes,
    Bytes,
    Mutations,
    Memory,
    Unknown,
}

fn denial_name(denial: u8) -> &'static str {
    match denial {
        DENIAL_NONE => "none",
        DENIAL_CANCELLED => "cancelled",
        DENIAL_DEADLINE => "deadline",
        DENIAL_READS => "reads",
        DENIAL_WRITES => "writes",
        DENIAL_BYTES => "bytes",
        DENIAL_MUTATIONS => "mutations",
        DENIAL_MEMORY => "memory",
        _ => "unknown",
    }
}

struct DeniedCharge {
    axis: &'static str,
    used: u64,
    attempted: u64,
    limit: u64,
    overflow: bool,
    caller: &'static std::panic::Location<'static>,
}

impl fmt::Debug for DeniedCharge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeniedCharge")
            .field("axis", &self.axis)
            .field("used", &self.used)
            .field("attempted", &self.attempted)
            .field("limit", &self.limit)
            .field("overflow", &self.overflow)
            .field("caller", &self.caller)
            .finish()
    }
}

struct DeniedMemory {
    attempted: u64,
    lifetime: super::MemoryLifetime,
    caller: &'static std::panic::Location<'static>,
}

impl fmt::Debug for DeniedMemory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeniedMemory")
            .field("attempted", &self.attempted)
            .field("lifetime", &self.lifetime)
            .field("caller", &self.caller)
            .finish()
    }
}

struct ExternalResourceRefusal {
    boundary: &'static str,
    reason: [u8; MAX_EXTERNAL_REASON_BYTES],
    length: usize,
    truncated: bool,
    caller: &'static std::panic::Location<'static>,
}

impl ExternalResourceRefusal {
    fn new(
        boundary: &'static str,
        reason: &str,
        caller: &'static std::panic::Location<'static>,
    ) -> Self {
        let mut length = reason.len().min(MAX_EXTERNAL_REASON_BYTES);
        while !reason.is_char_boundary(length) {
            length = length.saturating_sub(1);
        }
        let mut bounded = [0; MAX_EXTERNAL_REASON_BYTES];
        for (destination, byte) in bounded.iter_mut().zip(reason.as_bytes()).take(length) {
            *destination = *byte;
        }
        Self {
            boundary,
            reason: bounded,
            length,
            truncated: length < reason.len(),
            caller,
        }
    }

    fn reason(&self) -> Result<&str, IndexError> {
        let bytes = self.reason.get(..self.length).ok_or_else(|| {
            IndexError::BackendViolation("external refusal length invariant".into())
        })?;
        std::str::from_utf8(bytes).map_err(|_error| {
            IndexError::BackendViolation("external refusal UTF-8 invariant".into())
        })
    }
}

impl fmt::Debug for ExternalResourceRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExternalResourceRefusal")
            .field("boundary", &self.boundary)
            .field("reason", &self.reason().map_err(|_error| fmt::Error)?)
            .field("length", &self.length)
            .field("truncated", &self.truncated)
            .field("caller", &self.caller)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkLimits {
    pub reads: u64,
    pub writes: u64,
    pub bytes: u64,
    pub mutations: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkUsage {
    pub reads: u64,
    pub writes: u64,
    pub bytes: u64,
    pub mutations: u64,
}

pub struct WorkBudget {
    limits: WorkLimits,
    usage: WorkUsage,
    request: crate::RequestBudgetV1,
    memory: Option<super::MemoryScope>,
    // This is a bounded diagnostic for the first local refusal, not a claim
    // about the terminal cause of the enclosing operation.
    first_denial: AtomicU8,
    denied_checkpoint: OnceLock<&'static std::panic::Location<'static>>,
    denied_charge: OnceLock<DeniedCharge>,
    denied_memory: OnceLock<DeniedMemory>,
    external_resource_refusal: OnceLock<ExternalResourceRefusal>,
    failed_stage: OnceLock<crate::DiagnosticStageV1>,
}

impl fmt::Debug for WorkBudget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkBudget")
            .field("limits", &self.limits)
            .field("usage", &self.usage)
            .field(
                "first_denial",
                &denial_name(self.first_denial.load(Ordering::Acquire)),
            )
            .field("denied_checkpoint", &self.denied_checkpoint.get())
            .field("denied_charge", &self.denied_charge.get())
            .field("denied_memory", &self.denied_memory.get())
            .field(
                "external_resource_refusal",
                &self.external_resource_refusal.get(),
            )
            .field("failed_stage", &self.failed_stage.get())
            .finish_non_exhaustive()
    }
}

impl WorkBudget {
    #[must_use]
    pub fn new(limits: WorkLimits, deadline: Instant) -> Self {
        Self::with_request(limits, crate::RequestBudgetV1::until(deadline))
    }

    #[must_use]
    pub fn with_request(limits: WorkLimits, request: crate::RequestBudgetV1) -> Self {
        Self {
            limits,
            usage: WorkUsage::default(),
            request,
            memory: None,
            first_denial: AtomicU8::new(DENIAL_NONE),
            denied_checkpoint: OnceLock::new(),
            denied_charge: OnceLock::new(),
            denied_memory: OnceLock::new(),
            external_resource_refusal: OnceLock::new(),
            failed_stage: OnceLock::new(),
        }
    }

    /// Record the first enclosing stage to observe an unsuccessful result.
    /// This diagnostic neither changes admission nor identifies the dominant
    /// elapsed-time contributor. It allocates no backing and reads no clock.
    pub fn record_failed_stage(&self, stage: crate::DiagnosticStageV1) {
        let _recorded = self.failed_stage.set(stage);
    }

    /// Record the first refusal from an external resource authority. This is
    /// diagnostic data only; it never changes this budget's local denial or
    /// authorizes additional work. The message is copied into fixed storage.
    #[track_caller]
    pub fn record_external_resource_refusal(&self, boundary: &'static str, reason: &str) {
        let caller = std::panic::Location::caller();
        let _recorded = self
            .external_resource_refusal
            .get_or_init(|| ExternalResourceRefusal::new(boundary, reason, caller));
    }

    /// Inspect the original local refusal immediately after a failing budgeted
    /// call. A later clock observation cannot identify that call's cause.
    #[must_use]
    pub fn first_denial(&self) -> Option<WorkBudgetDenial> {
        match self.first_denial.load(Ordering::Acquire) {
            DENIAL_NONE => None,
            DENIAL_CANCELLED => Some(WorkBudgetDenial::Cancelled),
            DENIAL_DEADLINE => Some(WorkBudgetDenial::Deadline),
            DENIAL_READS => Some(WorkBudgetDenial::Reads),
            DENIAL_WRITES => Some(WorkBudgetDenial::Writes),
            DENIAL_BYTES => Some(WorkBudgetDenial::Bytes),
            DENIAL_MUTATIONS => Some(WorkBudgetDenial::Mutations),
            DENIAL_MEMORY => Some(WorkBudgetDenial::Memory),
            _ => Some(WorkBudgetDenial::Unknown),
        }
    }

    fn record_denial(&self, denial: u8) -> bool {
        self.first_denial
            .compare_exchange(DENIAL_NONE, denial, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    #[track_caller]
    fn charge_axis(
        &self,
        denial: u8,
        axis: &'static str,
        used: u64,
        attempted: u64,
        limit: u64,
    ) -> Result<u64, IndexError> {
        let next = used.checked_add(attempted);
        if let Some(next) = next.filter(|next| *next <= limit) {
            return Ok(next);
        }
        if self.record_denial(denial) {
            let _recorded = self.denied_charge.set(DeniedCharge {
                axis,
                used,
                attempted,
                limit,
                overflow: next.is_none(),
                caller: std::panic::Location::caller(),
            });
        }
        Err(IndexError::ResourceLimited)
    }

    #[track_caller]
    pub fn checkpoint(&self) -> Result<(), IndexError> {
        if self.request.is_cancelled() {
            if self.record_denial(DENIAL_CANCELLED) {
                let _recorded = self.denied_checkpoint.set(std::panic::Location::caller());
            }
            return Err(IndexError::Cancelled);
        }
        if Instant::now() >= self.request.deadline() {
            if self.record_denial(DENIAL_DEADLINE) {
                let _recorded = self.denied_checkpoint.set(std::panic::Location::caller());
            }
            Err(IndexError::ResourceLimited)
        } else {
            Ok(())
        }
    }

    #[track_caller]
    pub fn charge(&mut self, delta: WorkUsage) -> Result<(), IndexError> {
        self.checkpoint()?;
        let next = WorkUsage {
            reads: self.charge_axis(
                DENIAL_READS,
                "reads",
                self.usage.reads,
                delta.reads,
                self.limits.reads,
            )?,
            writes: self.charge_axis(
                DENIAL_WRITES,
                "writes",
                self.usage.writes,
                delta.writes,
                self.limits.writes,
            )?,
            bytes: self.charge_axis(
                DENIAL_BYTES,
                "bytes",
                self.usage.bytes,
                delta.bytes,
                self.limits.bytes,
            )?,
            mutations: self.charge_axis(
                DENIAL_MUTATIONS,
                "mutations",
                self.usage.mutations,
                delta.mutations,
                self.limits.mutations,
            )?,
        };
        self.usage = next;
        Ok(())
    }

    /// Bind the actual current stage without replacing the original work
    /// usage/deadline. None is the explicit standalone allocation contract.
    pub fn bind_memory_scope(&mut self, memory: Option<super::MemoryScope>) {
        self.memory = memory;
    }

    /// Reserve before allocation. A retained lease owns data until its last
    /// shared owner; a temporary lease must end before its execution guard.
    #[track_caller]
    pub fn reserve_memory(
        &self,
        bytes: u64,
        lifetime: super::MemoryLifetime,
    ) -> Result<Option<super::MemoryLease>, IndexError> {
        self.checkpoint()?;
        let result = self
            .memory
            .as_ref()
            .map(|scope| scope.reserve(bytes, lifetime))
            .transpose();
        let lease = match result {
            Ok(lease) => lease,
            Err(error) => {
                if error == IndexError::ResourceLimited && self.record_denial(DENIAL_MEMORY) {
                    let _recorded = self.denied_memory.set(DeniedMemory {
                        attempted: bytes,
                        lifetime,
                        caller: std::panic::Location::caller(),
                    });
                }
                return Err(error);
            }
        };
        self.checkpoint()?;
        Ok(lease)
    }

    #[must_use]
    pub const fn usage(&self) -> WorkUsage {
        self.usage
    }

    /// The provider receives the same deadline and cancellation authority as
    /// admission. It must not create a fresh timeout when execution resumes.
    #[must_use]
    pub const fn request(&self) -> &crate::RequestBudgetV1 {
        &self.request
    }

    /// Temporarily parent diagnostic work to an already opened phase span.
    ///
    /// Only the request's diagnostic context changes. The original request,
    /// including its deadline, cancellation, ledgers and provider grant, is
    /// restored when the lexical borrow ends, including on unwind. Off avoids
    /// cloning the request or observing a diagnostic clock.
    pub fn scoped_diagnostic_context(
        &mut self,
        context: Option<crate::DiagnosticContextV1>,
    ) -> impl std::ops::DerefMut<Target = Self> + '_ {
        struct Restore<'a> {
            budget: &'a mut WorkBudget,
            original: Option<crate::RequestBudgetV1>,
        }

        impl std::ops::Deref for Restore<'_> {
            type Target = WorkBudget;

            fn deref(&self) -> &Self::Target {
                self.budget
            }
        }

        impl std::ops::DerefMut for Restore<'_> {
            fn deref_mut(&mut self) -> &mut Self::Target {
                self.budget
            }
        }

        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                if let Some(original) = self.original.take() {
                    self.budget.request = original;
                }
            }
        }

        let original = context.map(|context| {
            let scoped = self.request.clone().with_diagnostic_context(context);
            std::mem::replace(&mut self.request, scoped)
        });
        Restore {
            budget: self,
            original,
        }
    }

    /// Bind a newly admitted execution stage before asynchronous work starts.
    ///
    /// The request retains its original deadline, cancellation and usage.
    pub fn bind_provider_attempt_custody(
        &mut self,
        custody: &std::sync::Arc<dyn crate::ProviderAttemptCustodyPortV1>,
    ) -> Result<(), IndexError> {
        self.request
            .bind_provider_attempt_custody_v1(custody)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))
    }

    /// A synchronous read stage gets a distinct weak provider authority.
    ///
    /// Old request clones cannot spend a later stage's grant. The original
    /// request is restored on success, error and unwind without resetting usage.
    pub fn with_provider_attempt_custody<T>(
        &mut self,
        custody: &std::sync::Arc<dyn crate::ProviderAttemptCustodyPortV1>,
        execute: impl FnOnce(&mut Self) -> Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        struct Restore<'a> {
            budget: &'a mut WorkBudget,
            original: Option<crate::RequestBudgetV1>,
        }
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                if let Some(original) = self.original.take() {
                    self.budget.request = original;
                }
            }
        }
        let mut stage_request = self.request.clone();
        stage_request
            .bind_provider_attempt_custody_v1(custody)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let original = std::mem::replace(&mut self.request, stage_request);
        let restore = Restore {
            budget: self,
            original: Some(original),
        };
        execute(&mut *restore.budget)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    use super::*;

    // Preserve single evaluation and borrowed operands while reporting test failures as errors.
    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        }};
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) => {
                    if left != right {
                        return Err(IndexError::BackendViolation(format!(
                            "assertion failed: {} == {}; left: {left:?}, right: {right:?}",
                            stringify!($left),
                            stringify!($right)
                        )));
                    }
                }
            }
        }};
    }

    fn limits() -> WorkLimits {
        WorkLimits {
            reads: 3,
            writes: 2,
            bytes: 10,
            mutations: 4,
        }
    }

    #[test]
    fn external_resource_refusal_records_first_boundary_without_changing_local_budget()
    -> Result<(), IndexError> {
        let mut budget = WorkBudget::new(
            limits(),
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        check!(budget.external_resource_refusal.get().is_none());
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let accepted = budget.usage();
        let before_call = line!();
        budget.record_external_resource_refusal("file_capacity", "unique-file limit");
        let after_call = line!();
        budget.record_external_resource_refusal("other", "later refusal");
        let first = budget.external_resource_refusal.get().ok_or_else(|| {
            IndexError::BackendViolation("external refusal diagnostic absent".into())
        })?;
        check_eq!(first.boundary, "file_capacity");
        check_eq!(first.reason()?, "unique-file limit");
        check!(!first.truncated);
        check_eq!(first.caller.file(), file!());
        check!(first.caller.line() > before_call && first.caller.line() < after_call);
        check_eq!(budget.usage(), accepted);
        check_eq!(budget.first_denial(), None);
        check!(format!("{budget:?}").contains("external_resource_refusal"));

        check!(matches!(
            budget.charge(WorkUsage {
                reads: 3,
                ..WorkUsage::default()
            }),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(budget.first_denial(), Some(WorkBudgetDenial::Reads));
        check_eq!(budget.usage(), accepted);
        check_eq!(
            budget
                .external_resource_refusal
                .get()
                .map(ExternalResourceRefusal::reason)
                .transpose()?,
            Some("unique-file limit")
        );
        Ok(())
    }

    #[test]
    fn external_resource_reason_truncates_at_utf8_boundary() -> Result<(), IndexError> {
        let budget = WorkBudget::new(
            limits(),
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let prefix = "a".repeat(255);
        let reason = format!("{prefix}🦀suffix");
        budget.record_external_resource_refusal("file_capacity", &reason);
        let first = budget.external_resource_refusal.get().ok_or_else(|| {
            IndexError::BackendViolation("external refusal diagnostic absent".into())
        })?;
        check_eq!(first.reason()?, prefix.as_str());
        check_eq!(first.length, 255);
        check!(first.truncated);
        check_eq!(budget.first_denial(), None);
        check_eq!(budget.usage(), WorkUsage::default());
        Ok(())
    }

    #[test]
    fn first_denial_records_exact_axis_and_preserves_composite_usage() -> Result<(), IndexError> {
        let accepted = WorkUsage {
            reads: 1,
            writes: 1,
            bytes: 4,
            mutations: 1,
        };
        let cases = [
            (
                WorkUsage {
                    reads: 3,
                    ..WorkUsage::default()
                },
                DENIAL_READS,
                "reads",
                1,
                3,
                3,
            ),
            (
                WorkUsage {
                    reads: 1,
                    writes: 2,
                    ..WorkUsage::default()
                },
                DENIAL_WRITES,
                "writes",
                1,
                2,
                2,
            ),
            (
                WorkUsage {
                    reads: 1,
                    writes: 1,
                    bytes: 7,
                    ..WorkUsage::default()
                },
                DENIAL_BYTES,
                "bytes",
                4,
                7,
                10,
            ),
            (
                WorkUsage {
                    reads: 1,
                    writes: 1,
                    bytes: 1,
                    mutations: 4,
                },
                DENIAL_MUTATIONS,
                "mutations",
                1,
                4,
                4,
            ),
        ];
        for (delta, code, axis, used, attempted, limit) in cases {
            let mut budget = WorkBudget::new(
                limits(),
                Instant::now()
                    .checked_add(Duration::from_secs(30))
                    .ok_or(IndexError::ResourceLimited)?,
            );
            budget.charge(accepted)?;
            let denied_line = line!() + 1;
            let refusal = budget.charge(delta);
            check_eq!(refusal, Err(IndexError::ResourceLimited));
            check_eq!(budget.usage(), accepted);
            check_eq!(budget.first_denial.load(Ordering::Acquire), code);
            let denied = budget.denied_charge.get().ok_or_else(|| {
                IndexError::BackendViolation("missing first denied charge".into())
            })?;
            check_eq!(denied.axis, axis);
            check_eq!(denied.used, used);
            check_eq!(denied.attempted, attempted);
            check_eq!(denied.limit, limit);
            check!(!denied.overflow);
            check_eq!(denied.caller.file(), file!());
            check_eq!(denied.caller.line(), denied_line);
            let debug = format!("{budget:?}");
            check!(debug.contains(&format!("first_denial: \"{axis}\"")));
            check!(!debug.contains(", request:"));
            check!(!debug.contains(", memory:"));
            check!(debug.contains("denied_memory: None"));
            check!(budget.denied_memory.get().is_none());
        }
        Ok(())
    }

    #[test]
    fn first_denial_records_arithmetic_overflow_without_changing_usage() -> Result<(), IndexError> {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: u64::MAX,
                ..limits()
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        budget.charge(WorkUsage {
            reads: u64::MAX,
            ..WorkUsage::default()
        })?;
        check_eq!(
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            }),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(budget.usage().reads, u64::MAX);
        check_eq!(budget.first_denial.load(Ordering::Acquire), DENIAL_READS);
        let denied = budget
            .denied_charge
            .get()
            .ok_or_else(|| IndexError::BackendViolation("missing overflow diagnostic".into()))?;
        check_eq!(denied.used, u64::MAX);
        check_eq!(denied.attempted, 1);
        check_eq!(denied.limit, u64::MAX);
        check!(denied.overflow);
        Ok(())
    }

    #[test]
    fn first_denial_is_monotonic_across_later_refusals() -> Result<(), IndexError> {
        let request = crate::RequestBudgetV1::for_duration(Duration::from_secs(30));
        let mut budget = WorkBudget::with_request(limits(), request.clone());
        check_eq!(budget.first_denial(), None);
        check_eq!(
            budget.charge(WorkUsage {
                bytes: 11,
                ..WorkUsage::default()
            }),
            Err(IndexError::ResourceLimited)
        );
        let original_debug = format!("{budget:?}");
        check_eq!(
            budget.charge(WorkUsage {
                reads: 4,
                ..WorkUsage::default()
            }),
            Err(IndexError::ResourceLimited)
        );
        request.cancel_handle().cancel();
        check_eq!(budget.checkpoint(), Err(IndexError::Cancelled));
        check_eq!(budget.first_denial.load(Ordering::Acquire), DENIAL_BYTES);
        check_eq!(budget.first_denial(), Some(WorkBudgetDenial::Bytes));
        check!(budget.denied_checkpoint.get().is_none());
        check_eq!(format!("{budget:?}"), original_debug);
        check_eq!(budget.usage(), WorkUsage::default());
        Ok(())
    }

    #[test]
    fn checkpoint_refusals_record_cancel_and_deadline_separately() -> Result<(), IndexError> {
        let request = crate::RequestBudgetV1::for_duration(Duration::from_secs(30));
        let cancelled = WorkBudget::with_request(limits(), request.clone());
        request.cancel_handle().cancel();
        let cancelled_line = line!() + 1;
        check_eq!(cancelled.checkpoint(), Err(IndexError::Cancelled));
        check_eq!(
            cancelled.first_denial.load(Ordering::Acquire),
            DENIAL_CANCELLED
        );
        let cancelled_caller = cancelled.denied_checkpoint.get().ok_or_else(|| {
            IndexError::BackendViolation("missing cancelled checkpoint caller".into())
        })?;
        check_eq!(cancelled_caller.file(), file!());
        check_eq!(cancelled_caller.line(), cancelled_line);
        check!(cancelled.denied_charge.get().is_none());

        let past = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .ok_or(IndexError::ResourceLimited)?;
        let expired = WorkBudget::new(limits(), past);
        let expired_line = line!() + 1;
        check_eq!(expired.checkpoint(), Err(IndexError::ResourceLimited));
        check_eq!(
            expired.first_denial.load(Ordering::Acquire),
            DENIAL_DEADLINE
        );
        let expired_caller = expired.denied_checkpoint.get().ok_or_else(|| {
            IndexError::BackendViolation("missing expired checkpoint caller".into())
        })?;
        check_eq!(expired_caller.file(), file!());
        check_eq!(expired_caller.line(), expired_line);
        check!(expired.denied_charge.get().is_none());
        check_eq!(cancelled.usage(), WorkUsage::default());
        check_eq!(expired.usage(), WorkUsage::default());
        check_eq!(cancelled.first_denial(), Some(WorkBudgetDenial::Cancelled));
        check_eq!(expired.first_denial(), Some(WorkBudgetDenial::Deadline));
        Ok(())
    }

    #[test]
    fn charge_forwards_first_checkpoint_caller_without_overwriting_it() -> Result<(), IndexError> {
        let past = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .ok_or(IndexError::ResourceLimited)?;
        let mut expired = WorkBudget::new(limits(), past);
        let first_line = line!() + 1;
        let first = expired.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        });
        check_eq!(first, Err(IndexError::ResourceLimited));
        check_eq!(
            expired.first_denial.load(Ordering::Acquire),
            DENIAL_DEADLINE
        );
        let caller = expired.denied_checkpoint.get().ok_or_else(|| {
            IndexError::BackendViolation("missing forwarded checkpoint caller".into())
        })?;
        check_eq!(caller.file(), file!());
        check_eq!(caller.line(), first_line);
        check!(format!("{expired:?}").contains("denied_checkpoint: Some("));
        let second = expired.charge(WorkUsage {
            reads: 2,
            ..WorkUsage::default()
        });
        check_eq!(second, Err(IndexError::ResourceLimited));
        check_eq!(
            expired.denied_checkpoint.get().map(|site| site.line()),
            Some(first_line)
        );
        check!(expired.denied_charge.get().is_none());
        check_eq!(expired.usage(), WorkUsage::default());
        Ok(())
    }

    #[test]
    fn rejected_composite_charge_preserves_all_usage_fields() -> Result<(), IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(limits(), deadline);
        let accepted = WorkUsage {
            reads: 1,
            writes: 1,
            bytes: 4,
            mutations: 1,
        };
        budget.charge(accepted)?;
        check_eq!(
            budget.charge(WorkUsage {
                reads: 1,
                writes: 1,
                bytes: 7,
                mutations: 1,
            }),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(budget.usage(), accepted);
        budget.charge(WorkUsage {
            reads: 2,
            writes: 1,
            bytes: 6,
            mutations: 3,
        })?;
        check_eq!(
            budget.usage(),
            WorkUsage {
                reads: 3,
                writes: 2,
                bytes: 10,
                mutations: 4,
            }
        );
        Ok(())
    }

    #[test]
    fn expired_budget_rejects_before_accounting() {
        let mut budget = WorkBudget::new(limits(), Instant::now());
        assert_eq!(
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            }),
            Err(IndexError::ResourceLimited)
        );
        assert_eq!(budget.usage(), WorkUsage::default());
    }

    #[test]
    fn provider_request_keeps_original_cancellation_authority() -> Result<(), IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let request = crate::RequestBudgetV1::until(deadline);
        let mut budget = WorkBudget::with_request(limits(), request.clone());
        check_eq!(budget.request().deadline(), deadline);
        request.cancel_handle().cancel();
        check!(budget.request().is_cancelled());
        check_eq!(
            budget.charge(WorkUsage {
                writes: 1,
                ..WorkUsage::default()
            }),
            Err(IndexError::Cancelled)
        );
        check_eq!(budget.usage(), WorkUsage::default());
        Ok(())
    }

    #[derive(Debug)]
    struct ProviderPortProbe;

    struct ProviderLeaseProbe;

    impl crate::ProviderAttemptLeaseV1 for ProviderLeaseProbe {}

    impl crate::ProviderAttemptCustodyPortV1 for ProviderPortProbe {
        fn begin_attempt_v1(
            &self,
        ) -> Result<Box<dyn crate::ProviderAttemptLeaseV1>, crate::CoreError> {
            Ok(Box::new(ProviderLeaseProbe))
        }
    }

    fn provider_port() -> Arc<dyn crate::ProviderAttemptCustodyPortV1> {
        Arc::new(ProviderPortProbe)
    }

    #[derive(Debug, Default)]
    struct DiagnosticClockProbe(AtomicU64);

    impl crate::DiagnosticClockV1 for DiagnosticClockProbe {
        fn now_ns(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }

    #[derive(Debug)]
    struct DiscardDiagnosticEvents;

    impl crate::DiagnosticEventSinkV1 for DiscardDiagnosticEvents {
        fn try_emit(&self, _event: crate::DiagnosticEmitV1) -> bool {
            true
        }
    }

    #[test]
    fn scoped_diagnostic_context_preserves_request_grant_usage_and_nested_restore()
    -> Result<(), IndexError> {
        let clock = Arc::new(DiagnosticClockProbe::default());
        let trace = crate::DiagnosticContextV1::new(
            crate::DiagnosticCaptureIdV1([71; 16]),
            clock,
            Arc::new(DiscardDiagnosticEvents),
        );
        let authority = provider_port();
        let mut budget = WorkBudget::with_request(
            limits(),
            crate::RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        budget.bind_provider_attempt_custody(&authority)?;
        let original = budget.request().clone();
        let deadline = original.deadline();
        let first = WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        };
        budget.charge(first)?;
        {
            let mut outer = budget.scoped_diagnostic_context(Some(trace.clone()));
            check!(outer.request().diagnostic_context().is_some());
            check!(outer.request().same_request_as(&original));
            check_eq!(outer.request().deadline(), deadline);
            check!(matches!(
                outer.request().begin_provider_attempt_v1(),
                Ok(Some(_))
            ));
            outer.charge(WorkUsage {
                writes: 1,
                ..WorkUsage::default()
            })?;
            {
                let inner = outer.scoped_diagnostic_context(None);
                check!(inner.request().diagnostic_context().is_some());
                check!(inner.request().same_request_as(&original));
            }
            {
                let inner = outer.scoped_diagnostic_context(Some(trace));
                check!(inner.request().diagnostic_context().is_some());
                check!(inner.request().same_request_as(&original));
                check!(matches!(
                    inner.request().begin_provider_attempt_v1(),
                    Ok(Some(_))
                ));
            }
            check!(outer.request().diagnostic_context().is_some());
        }
        check!(budget.request().diagnostic_context().is_none());
        check!(budget.request().same_request_as(&original));
        check_eq!(budget.request().deadline(), deadline);
        check_eq!(budget.usage().reads, 1);
        check_eq!(budget.usage().writes, 1);
        check!(matches!(
            budget.request().begin_provider_attempt_v1(),
            Ok(Some(_))
        ));
        Ok(())
    }

    #[test]
    fn scoped_diagnostic_context_restores_on_error_unwind_and_off_without_clock()
    -> Result<(), IndexError> {
        let clock = Arc::new(DiagnosticClockProbe::default());
        let trace = crate::DiagnosticContextV1::new(
            crate::DiagnosticCaptureIdV1([72; 16]),
            clock.clone(),
            Arc::new(DiscardDiagnosticEvents),
        );
        let original = crate::RequestBudgetV1::for_duration(Duration::from_secs(30));
        let mut budget = WorkBudget::with_request(limits(), original.clone());
        let before = clock.0.load(Ordering::Relaxed);
        {
            let mut off = budget.scoped_diagnostic_context(None);
            check!(off.request().diagnostic_context().is_none());
            check_eq!(off.checkpoint(), Ok(()));
            off.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
        }
        check_eq!(clock.0.load(Ordering::Relaxed), before);
        let error = {
            let mut scoped = budget.scoped_diagnostic_context(Some(trace.clone()));
            scoped.charge(WorkUsage {
                reads: u64::MAX,
                ..WorkUsage::default()
            })
        };
        check_eq!(error, Err(IndexError::ResourceLimited));
        check_eq!(budget.first_denial(), Some(WorkBudgetDenial::Reads));
        check_eq!(budget.usage().reads, 1);
        check!(budget.request().diagnostic_context().is_none());
        check!(budget.request().same_request_as(&original));
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut scoped = budget.scoped_diagnostic_context(Some(trace));
            let _charged = scoped.charge(WorkUsage {
                writes: 1,
                ..WorkUsage::default()
            });
            std::panic::resume_unwind(Box::new("diagnostic phase unwind"));
        }));
        check!(panicked.is_err());
        check!(budget.request().diagnostic_context().is_none());
        check!(budget.request().same_request_as(&original));
        check_eq!(budget.usage().writes, 1);
        check_eq!(budget.first_denial(), Some(WorkBudgetDenial::Reads));
        original.cancel_handle().cancel();
        check_eq!(budget.checkpoint(), Err(IndexError::Cancelled));
        Ok(())
    }

    #[test]
    fn scoped_provider_stage_restores_request_and_keeps_usage_after_error() -> Result<(), IndexError>
    {
        let original = crate::RequestBudgetV1::for_duration(Duration::from_secs(30));
        let deadline = original.deadline();
        let mut budget = WorkBudget::with_request(limits(), original.clone());
        let initial = WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        };
        budget.charge(initial)?;
        let first = provider_port();
        let mut escaped = None;
        let result = budget.with_provider_attempt_custody(&first, |stage| {
            check_eq!(stage.request().deadline(), deadline);
            check!(stage.request().same_request_as(&original));
            check!(matches!(
                stage.request().begin_provider_attempt_v1(),
                Ok(Some(_))
            ));
            escaped = Some(stage.request().clone());
            stage.charge(WorkUsage {
                writes: 1,
                ..WorkUsage::default()
            })?;
            Err::<(), IndexError>(IndexError::ResourceLimited)
        });
        check_eq!(result, Err(IndexError::ResourceLimited));
        check_eq!(
            budget.usage(),
            WorkUsage {
                reads: 1,
                writes: 1,
                ..WorkUsage::default()
            }
        );
        check_eq!(budget.request().deadline(), deadline);
        check!(budget.request().same_request_as(&original));
        check!(matches!(
            budget.request().begin_provider_attempt_v1(),
            Ok(None)
        ));
        drop(first);

        let second = provider_port();
        budget.with_provider_attempt_custody(&second, |stage| {
            check!(matches!(
                stage.request().begin_provider_attempt_v1(),
                Ok(Some(_))
            ));
            Ok(())
        })?;
        let escaped = escaped.ok_or_else(|| {
            IndexError::BackendViolation("first provider stage did not retain a clone".into())
        })?;
        check!(matches!(
            escaped.begin_provider_attempt_v1(),
            Err(crate::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
                ..
            })
        ));
        check!(matches!(
            budget.request().begin_provider_attempt_v1(),
            Ok(None)
        ));
        Ok(())
    }

    #[test]
    fn scoped_provider_stage_restores_request_on_unwind_and_preserves_cancellation() {
        let original = crate::RequestBudgetV1::for_duration(Duration::from_secs(30));
        let deadline = original.deadline();
        let mut budget = WorkBudget::with_request(limits(), original.clone());
        let authority = provider_port();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _outcome: Result<(), IndexError> =
                budget.with_provider_attempt_custody(&authority, |stage| {
                    assert!(matches!(
                        stage.request().begin_provider_attempt_v1(),
                        Ok(Some(_))
                    ));
                    assert!(
                        stage
                            .charge(WorkUsage {
                                reads: 1,
                                ..WorkUsage::default()
                            })
                            .is_ok()
                    );
                    original.cancel_handle().cancel();
                    std::panic::resume_unwind(Box::new("stage reader panic"));
                });
        }));
        assert!(panicked.is_err());
        assert_eq!(budget.request().deadline(), deadline);
        assert!(budget.request().same_request_as(&original));
        assert!(matches!(
            budget.request().begin_provider_attempt_v1(),
            Err(crate::CoreError::Typed {
                code: crate::REQUEST_CANCELLED_CODE,
                ..
            })
        ));
        assert_eq!(budget.usage().reads, 1);
        assert_eq!(budget.checkpoint(), Err(IndexError::Cancelled));
    }

    struct CancellingMemoryScope {
        request: crate::RequestBudgetV1,
        calls: Arc<AtomicU64>,
        released: Arc<AtomicU64>,
    }

    struct ReleasedBytes(Arc<AtomicU64>);

    impl super::super::MemoryLeaseOwner for ReleasedBytes {
        fn release(&self, bytes: u64) {
            let _previous = self.0.fetch_add(bytes, Ordering::SeqCst);
        }
    }

    impl super::super::MemoryScopePort for CancellingMemoryScope {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: super::super::MemoryLifetime,
        ) -> Result<super::super::MemoryLease, IndexError> {
            let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
            self.request.cancel_handle().cancel();
            Ok(super::super::MemoryLease::new(
                bytes,
                Arc::new(ReleasedBytes(Arc::clone(&self.released))),
            ))
        }
    }

    struct RefusingMemoryScope;

    impl super::super::MemoryScopePort for RefusingMemoryScope {
        fn reserve(
            &self,
            _bytes: u64,
            _lifetime: super::super::MemoryLifetime,
        ) -> Result<super::super::MemoryLease, IndexError> {
            Err(IndexError::ResourceLimited)
        }
    }

    #[test]
    fn memory_refusal_records_first_denial_without_charging_work() -> Result<(), IndexError> {
        let mut budget = WorkBudget::new(
            limits(),
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        budget.bind_memory_scope(Some(super::super::MemoryScope::new(Arc::new(
            RefusingMemoryScope,
        ))));
        let before_call = line!();
        let result = budget.reserve_memory(7, super::super::MemoryLifetime::Temporary);
        let after_call = line!();
        check!(matches!(result, Err(IndexError::ResourceLimited)));
        check_eq!(budget.first_denial.load(Ordering::Acquire), DENIAL_MEMORY);
        check!(budget.denied_charge.get().is_none());
        check!(matches!(
            budget.reserve_memory(11, super::super::MemoryLifetime::Retained),
            Err(IndexError::ResourceLimited)
        ));
        let denied = budget.denied_memory.get().ok_or_else(|| {
            IndexError::BackendViolation("memory refusal lost its first allocation".into())
        })?;
        check_eq!(denied.attempted, 7);
        check_eq!(denied.lifetime, super::super::MemoryLifetime::Temporary);
        check_eq!(denied.caller.file(), file!());
        check!(denied.caller.line() > before_call && denied.caller.line() < after_call);
        check_eq!(budget.usage(), WorkUsage::default());
        Ok(())
    }

    #[test]
    fn memory_reservation_rechecks_original_cancellation_and_releases_refused_lease()
    -> Result<(), IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let request = crate::RequestBudgetV1::until(deadline);
        let calls = Arc::new(AtomicU64::new(0));
        let released = Arc::new(AtomicU64::new(0));
        let mut budget = WorkBudget::with_request(limits(), request.clone());
        let usage = WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        };
        budget.charge(usage)?;
        budget.bind_memory_scope(Some(super::super::MemoryScope::new(Arc::new(
            CancellingMemoryScope {
                request,
                calls: Arc::clone(&calls),
                released: Arc::clone(&released),
            },
        ))));
        check!(matches!(
            budget.reserve_memory(7, super::super::MemoryLifetime::Retained),
            Err(IndexError::Cancelled)
        ));
        check_eq!(calls.load(Ordering::SeqCst), 1);
        check_eq!(released.load(Ordering::SeqCst), 7);
        check_eq!(budget.request().deadline(), deadline);
        check_eq!(budget.usage(), usage);
        check!(matches!(
            budget.reserve_memory(7, super::super::MemoryLifetime::Retained),
            Err(IndexError::Cancelled)
        ));
        check_eq!(calls.load(Ordering::SeqCst), 1);
        budget.bind_memory_scope(None);
        check_eq!(budget.checkpoint(), Err(IndexError::Cancelled));
        Ok(())
    }
}

quanta_index_contract::native_serde_struct!(WorkLimits {
    reads: u64,
    writes: u64,
    bytes: u64,
    mutations: u64,
});
