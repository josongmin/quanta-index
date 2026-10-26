//! Target-scoped read-only preparation; physical publication remains foreground-owned.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use quanta_index_core::lexical_maintenance::LexicalMaintenancePolicy;
use quanta_index_core::{
    CancelHandleV1, CoreError, DiagnosticContextV1, DiagnosticStageV1, RequestBudgetV1,
};

use crate::file_authority::paged::{AuthorityRoot, PreparedFileCompaction};
use crate::lexical_store::ObjectStore;
use crate::text_authority::{NativeRoot, PreparedNativeCompaction};

// The permit also covers the packet after join, until publication consumes it.
static PREPARATION_BUSY: AtomicBool = AtomicBool::new(false);
const READ_CACHE_BYTES: u64 = 4 * 1024 * 1024;

struct PreparationPermit(&'static AtomicBool);

impl PreparationPermit {
    fn acquire(gate: &'static AtomicBool) -> Option<Self> {
        match gate.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_previous) => Some(Self(gate)),
            Err(_busy) => None,
        }
    }
}

impl Drop for PreparationPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Default)]
pub(crate) struct PreparedCompactions {
    pub(crate) file: Option<PreparedFileCompaction>,
    pub(crate) native: Option<PreparedNativeCompaction>,
    prepare_budget: Option<RequestBudgetV1>,
    _permit: Option<PreparationPermit>,
}

fn packet_allowances(
    fixed_scratch_bytes: u64,
    policy: Option<&LexicalMaintenancePolicy>,
    has_file: bool,
    has_native: bool,
    retired_bytes: Option<u64>,
) -> Result<(u64, u64, bool), CoreError> {
    let fixed_packets = fixed_scratch_bytes
        .checked_sub(READ_CACHE_BYTES)
        .ok_or_else(|| {
            CoreError::InvalidContract("compaction scratch ceiling is too small".into())
        })?;
    if policy.is_none() {
        // Preserve the original debt-only split when no maintenance policy
        // was configured for this service.
        let each = fixed_packets
            .checked_div(2)
            .ok_or_else(|| CoreError::InvalidContract("compaction packet split divisor".into()))?;
        let native_allowed = has_native && retired_bytes.is_some_and(|bytes| bytes <= each);
        return Ok((
            if has_file { each } else { 0 },
            if native_allowed {
                each.saturating_sub(retired_bytes.unwrap_or(0))
            } else {
                0
            },
            native_allowed,
        ));
    }
    let ceiling = fixed_packets
        .min(policy.map_or(fixed_packets, LexicalMaintenancePolicy::max_staging_bytes));
    let native_allowed = has_native && retired_bytes.is_some_and(|bytes| bytes < ceiling);
    let retirement = if native_allowed {
        retired_bytes.unwrap_or(0)
    } else {
        0
    };
    let available = ceiling.checked_sub(retirement).ok_or_else(|| {
        CoreError::InvalidContract("compaction retirement charge exceeds packet ceiling".into())
    })?;
    let half = available
        .checked_div(2)
        .ok_or_else(|| CoreError::InvalidContract("compaction packet split divisor".into()))?;
    let remainder = available
        .checked_sub(half)
        .ok_or_else(|| CoreError::InvalidContract("compaction packet split underflows".into()))?;
    let (file, native) = match (has_file, native_allowed) {
        (true, true) => (half, remainder),
        (true, false) => (available, 0),
        (false, true) => (0, available),
        (false, false) => (0, 0),
    };
    Ok((file, native, native_allowed))
}

/// Always joins before the publication's adoption lease or base guards drop.
pub(crate) struct CompactionWorker {
    worker: Option<JoinedWorker<Result<PreparedCompactions, CoreError>>>,
    cancel: CancelHandleV1,
    prepare_budget: Option<RequestBudgetV1>,
}

impl PreparedCompactions {
    fn optional_request_alive(&self) -> Result<bool, CoreError> {
        let Some(budget) = self.prepare_budget.as_ref() else {
            return Ok(true);
        };
        match budget.checkpoint("lexical:compaction:adopt-packet") {
            Ok(()) => Ok(true),
            Err(CoreError::Typed { code, .. })
                if code == quanta_index_core::REQUEST_CANCELLED_CODE
                    || code == quanta_index_core::REQUEST_DEADLINE_EXCEEDED_CODE =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn file_packet_valid(
        &self,
        file_base: Option<&AuthorityRoot>,
        policy: Option<&LexicalMaintenancePolicy>,
    ) -> Result<bool, CoreError> {
        self.file
            .as_ref()
            .map_or(Ok(true), |packet| packet.policy_valid(file_base, policy))
    }

    /// Recheck immediately before File adoption; writer lock acquisition and
    /// Native publication can age a packet after the worker was joined.
    pub(crate) fn take_file_for(
        &mut self,
        file_base: Option<&AuthorityRoot>,
        policy: Option<&LexicalMaintenancePolicy>,
    ) -> Result<Option<PreparedFileCompaction>, CoreError> {
        if !self.optional_request_alive()? || !self.file_packet_valid(file_base, policy)? {
            self.file = None;
        }
        Ok(self.file.take())
    }

    /// Check cancellation and packet custody at the Native adoption point.
    pub(crate) fn take_native_for(
        &mut self,
        native_base: Option<&NativeRoot>,
        policy: Option<&LexicalMaintenancePolicy>,
    ) -> Result<Option<PreparedNativeCompaction>, CoreError> {
        if !self.optional_request_alive()? {
            self.native = None;
        }
        if let Some(packet) = self.native.as_ref()
            && !packet.policy_valid(native_base, policy)?
        {
            self.native = None;
        }
        Ok(self.native.take())
    }

    fn validate_for(
        mut self,
        file_base: Option<&AuthorityRoot>,
        native_base: Option<&NativeRoot>,
        policy: Option<&LexicalMaintenancePolicy>,
    ) -> Result<Self, CoreError> {
        if !self.optional_request_alive()? {
            return Ok(Self::default());
        }
        if !self.file_packet_valid(file_base, policy)? {
            self.file = None;
        }
        if let Some(packet) = self.native.as_ref()
            && !packet.policy_valid(native_base, policy)?
        {
            self.native = None;
        }
        Ok(self)
    }
}

impl CompactionWorker {
    pub(crate) fn start(
        store: &ObjectStore,
        file: Option<&AuthorityRoot>,
        native: Option<&NativeRoot>,
        native_retired: &[u64],
        diagnostic: Option<&DiagnosticContextV1>,
        request: Option<&RequestBudgetV1>,
        policy: Option<&LexicalMaintenancePolicy>,
    ) -> Result<Option<Self>, CoreError> {
        let file = file
            .filter(|root| root.compaction_debt.entries != 0)
            .cloned();
        let native = native
            .filter(|root| root.compaction_due.entries != 0)
            .cloned();
        if file.is_none() && native.is_none() {
            return Ok(None);
        }
        let Some(permit) = PreparationPermit::acquire(&PREPARATION_BUSY) else {
            return Ok(None);
        };
        let store = store.metered_clone();
        let diagnostic = diagnostic.cloned();
        let retired_bytes = u64::try_from(native_retired.len())
            .map_err(|error| {
                CoreError::InvalidContract(format!("compaction retirement count: {error}"))
            })?
            .checked_mul(8);
        let (file_packet_bytes, native_packet_bytes, native_allowed) = packet_allowances(
            crate::file_authority::policy().bucket_scratch_bytes,
            policy,
            file.is_some(),
            native.is_some(),
            retired_bytes,
        )?;
        let native = native.filter(|_base| native_allowed);
        let native_retired = if native.is_some() {
            native_retired.to_vec()
        } else {
            Vec::new()
        };
        let file_root = file.clone();
        let native_base = native.clone();
        let policy = policy.copied();
        // Preparation is optional and may follow the external request's
        // cancellation/deadline. It has its own cancellation scope: dropping
        // this worker must never cancel the admitted durable publication.
        let budget = request.map_or_else(RequestBudgetV1::unbounded, RequestBudgetV1::child);
        let cancel = budget.cancel_handle();
        let prepare_budget = budget.clone();
        let handle = thread::Builder::new()
            .name("lexical-compaction-prepare".into())
            .spawn(move || {
                crate::causal_profile::observe_result(
                    diagnostic.as_ref(),
                    DiagnosticStageV1::Operation,
                    |_context| {
                        let file = file
                            .as_ref()
                            .map(|base| {
                                crate::file_authority::paged::prepare_compaction(
                                    &store,
                                    base,
                                    file_packet_bytes,
                                    Some(&budget),
                                )
                            })
                            .transpose()?
                            .flatten();
                        let file = match (file, file_root.as_ref(), policy.as_ref()) {
                            (Some(packet), Some(base), Some(policy)) => {
                                Some(packet.bind_policy(base, policy, file_packet_bytes)?)
                            }
                            (packet, _, _) => packet,
                        };
                        let native = native
                            .as_ref()
                            .map(|base| {
                                crate::text_authority::prepare_compaction(
                                    &store,
                                    base,
                                    native_packet_bytes,
                                    &native_retired,
                                    Some(&budget),
                                )
                            })
                            .transpose()?
                            .flatten();
                        let native = match (native, native_base.as_ref(), policy.as_ref()) {
                            (Some(packet), Some(base), Some(policy)) => {
                                Some(packet.bind_policy(base, policy)?)
                            }
                            (packet, _, _) => packet,
                        };
                        Ok(PreparedCompactions {
                            file,
                            native,
                            prepare_budget: None,
                            _permit: Some(permit),
                        })
                    },
                )
            })
            .map_err(|error| CoreError::Storage(format!("compaction worker spawn: {error}")))?;
        Ok(Some(Self {
            worker: Some(JoinedWorker {
                handle: Some(handle),
            }),
            cancel,
            prepare_budget: Some(prepare_budget),
        }))
    }

    pub(crate) fn finish_for(
        self,
        file_base: Option<&AuthorityRoot>,
        native_base: Option<&NativeRoot>,
        policy: Option<&LexicalMaintenancePolicy>,
    ) -> Result<PreparedCompactions, CoreError> {
        self.finish_raw()?
            .validate_for(file_base, native_base, policy)
    }

    #[cfg(test)]
    pub(crate) fn finish(self) -> Result<PreparedCompactions, CoreError> {
        self.finish_raw()
    }

    fn finish_raw(mut self) -> Result<PreparedCompactions, CoreError> {
        let prepared = self
            .worker
            .take()
            .ok_or_else(|| CoreError::Storage("compaction worker was already joined".into()))?
            .finish()?;
        match prepared {
            Err(CoreError::Typed { code, .. })
                if code == quanta_index_core::REQUEST_CANCELLED_CODE
                    || code == quanta_index_core::REQUEST_DEADLINE_EXCEEDED_CODE =>
            {
                // The dispatcher owns publication after durable admission.
                // Refusing optional read-only preparation must not abandon a
                // partly applied generation or cancel its foreground owner.
                Ok(PreparedCompactions::default())
            }
            Ok(mut packet) => {
                if self.prepare_budget.as_ref().is_some_and(|budget| {
                    matches!(
                        budget.checkpoint("lexical:compaction:adopt-packet"),
                        Err(CoreError::Typed { code, .. })
                            if code == quanta_index_core::REQUEST_CANCELLED_CODE
                                || code == quanta_index_core::REQUEST_DEADLINE_EXCEEDED_CODE
                    )
                }) {
                    Ok(PreparedCompactions::default())
                } else {
                    packet.prepare_budget = self.prepare_budget.take();
                    Ok(packet)
                }
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for CompactionWorker {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.cancel.cancel();
        }
        // The remaining JoinedWorker field joins after cancellation, before
        // PublicationContext releases the lease. An in-flight syscall is
        // cooperative rather than externally interrupted.
    }
}

struct JoinedWorker<T> {
    handle: Option<JoinHandle<T>>,
}

impl<T> JoinedWorker<T> {
    fn finish(mut self) -> Result<T, CoreError> {
        self.handle
            .take()
            .ok_or_else(|| CoreError::Storage("compaction worker was already joined".into()))?
            .join()
            .map_err(|_panic| CoreError::Storage("compaction preparation worker panicked".into()))
    }
}

impl<T> Drop for JoinedWorker<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // Join on the error/unwind path too. Never detach a reader of a
            // base whose lifecycle guards are about to be released.
            let _joined = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CompactionWorker, JoinedWorker, PreparationPermit, packet_allowances};
    use quanta_index_core::lexical_maintenance::LexicalMaintenancePolicy;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn configured_packet_allowance_bounds_both_authorities_and_preserves_legacy_split() {
        let fixed = 128 * 1024 * 1024;
        let policy =
            LexicalMaintenancePolicy::new(2, 8, 1, u64::MAX, 2, 100, 100, 1_000).expect("policy");
        let (file, native, allowed) =
            packet_allowances(fixed, Some(&policy), true, true, Some(10)).expect("split");
        assert!(allowed);
        assert_eq!((file, native), (45, 45));
        assert_eq!(file + native + 10, policy.max_staging_bytes());
        let (file, native, allowed) =
            packet_allowances(fixed, Some(&policy), true, true, Some(100)).expect("no native");
        assert!(!allowed);
        assert_eq!((file, native), (100, 0));
        let (file, native, allowed) =
            packet_allowances(fixed, None, true, true, Some(10)).expect("legacy split");
        assert!(allowed);
        assert_eq!(file, (fixed - super::READ_CACHE_BYTES).div_euclid(2));
        assert_eq!(native, file - 10);
    }

    #[test]
    fn preparation_permit_covers_the_packet_until_drop() {
        static GATE: AtomicBool = AtomicBool::new(false);
        let permit = PreparationPermit::acquire(&GATE).expect("first worker");
        assert!(PreparationPermit::acquire(&GATE).is_none());
        let packet = std::thread::spawn(move || permit).join().expect("join");
        assert!(PreparationPermit::acquire(&GATE).is_none());
        drop(packet);
        assert!(PreparationPermit::acquire(&GATE).is_some());
    }

    #[test]
    fn early_return_joins_before_releasing_base_custody() {
        let completed = std::sync::Arc::new(AtomicBool::new(false));
        let observed = completed.clone();
        let worker = JoinedWorker {
            handle: Some(std::thread::spawn(move || {
                observed.store(true, Ordering::Release);
            })),
        };
        drop(worker);
        assert!(completed.load(Ordering::Acquire));
    }

    #[test]
    fn panic_is_an_error_and_releases_the_preparation_permit() {
        static GATE: AtomicBool = AtomicBool::new(false);
        let permit = PreparationPermit::acquire(&GATE).expect("permit");
        let worker = JoinedWorker {
            handle: Some(std::thread::spawn(move || {
                let _permit = permit;
                panic!("preparation fault");
            })),
        };
        assert!(worker.finish().is_err());
        assert!(PreparationPermit::acquire(&GATE).is_some());
    }

    #[test]
    fn abandoned_preparation_cancels_then_joins() {
        let budget = quanta_index_core::RequestBudgetV1::unbounded();
        let cancel = budget.cancel_handle();
        let completed = std::sync::Arc::new(AtomicBool::new(false));
        let observed = completed.clone();
        let worker = CompactionWorker {
            worker: Some(JoinedWorker {
                handle: Some(std::thread::spawn(move || {
                    while budget.checkpoint("compaction-test").is_ok() {
                        std::thread::yield_now();
                    }
                    observed.store(true, Ordering::Release);
                    Ok(super::PreparedCompactions::default())
                })),
            }),
            cancel,
            prepare_budget: None,
        };
        drop(worker);
        assert!(completed.load(Ordering::Acquire));
    }

    #[test]
    fn abandoned_child_preparation_does_not_cancel_publication_owner() {
        let owner = quanta_index_core::RequestBudgetV1::unbounded();
        let child = owner.child();
        let cancel = child.cancel_handle();
        let worker = CompactionWorker {
            worker: Some(JoinedWorker {
                handle: Some(std::thread::spawn(move || {
                    while child.checkpoint("optional-prepare").is_ok() {
                        std::thread::yield_now();
                    }
                    Ok(super::PreparedCompactions::default())
                })),
            }),
            cancel,
            prepare_budget: None,
        };
        drop(worker);
        assert!(owner.checkpoint("durable-publication").is_ok());
    }

    #[test]
    fn external_cancel_discards_optional_packet_after_join() {
        let owner = quanta_index_core::RequestBudgetV1::unbounded();
        let child = owner.child();
        let cancel = child.cancel_handle();
        let joined = std::sync::Arc::new(AtomicBool::new(false));
        let observed = joined.clone();
        let worker = CompactionWorker {
            worker: Some(JoinedWorker {
                handle: Some(std::thread::spawn(move || {
                    while child.checkpoint("optional-prepare").is_ok() {
                        std::thread::yield_now();
                    }
                    observed.store(true, Ordering::Release);
                    child.checkpoint("optional-prepare")?;
                    Ok(super::PreparedCompactions::default())
                })),
            }),
            cancel,
            prepare_budget: None,
        };
        owner.cancel_handle().cancel();
        let packet = worker.finish().expect("durable owner keeps publication");
        assert!(joined.load(Ordering::Acquire));
        assert!(packet.file.is_none() && packet.native.is_none());
    }

    #[test]
    fn cancel_after_worker_completed_discards_packet_before_adoption() {
        static GATE: AtomicBool = AtomicBool::new(false);
        let owner = quanta_index_core::RequestBudgetV1::unbounded();
        let child = owner.child();
        let cancel = child.cancel_handle();
        let permit = PreparationPermit::acquire(&GATE).expect("permit");
        let worker = CompactionWorker {
            worker: Some(JoinedWorker {
                handle: Some(std::thread::spawn(move || {
                    Ok(super::PreparedCompactions {
                        _permit: Some(permit),
                        ..super::PreparedCompactions::default()
                    })
                })),
            }),
            cancel: cancel.clone(),
            prepare_budget: Some(child),
        };
        cancel.cancel();
        let packet = worker.finish().expect("cancel only optional packet");
        assert!(packet.file.is_none() && packet.native.is_none());
        assert!(PreparationPermit::acquire(&GATE).is_some());
        assert!(owner.checkpoint("durable owner").is_ok());
    }

    #[test]
    fn preparation_storage_failure_is_preserved() {
        let budget = quanta_index_core::RequestBudgetV1::unbounded();
        let worker = CompactionWorker {
            worker: Some(JoinedWorker {
                handle: Some(std::thread::spawn(|| {
                    Err(quanta_index_core::CoreError::Storage(
                        "prepare read failure".into(),
                    ))
                })),
            }),
            cancel: budget.cancel_handle(),
            prepare_budget: None,
        };
        assert!(
            matches!(worker.finish(), Err(quanta_index_core::CoreError::Storage(message))
            if message == "prepare read failure")
        );
    }
}
