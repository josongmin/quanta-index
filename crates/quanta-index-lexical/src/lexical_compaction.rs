//! Target-scoped read-only preparation; physical publication remains foreground-owned.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

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
    _permit: Option<PreparationPermit>,
}

/// Always joins before the publication's adoption lease or base guards drop.
pub(crate) struct CompactionWorker {
    worker: Option<JoinedWorker<Result<PreparedCompactions, CoreError>>>,
    cancel: CancelHandleV1,
}

impl CompactionWorker {
    pub(crate) fn start(
        store: &ObjectStore,
        file: Option<&AuthorityRoot>,
        native: Option<&NativeRoot>,
        native_retired: &[u64],
        diagnostic: Option<&DiagnosticContextV1>,
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
        // Both admitted packets plus one bounded read cache fit the existing
        // scratch ceiling. This is optional preparation, not a corpus/RSS cap.
        let packet_bytes = crate::file_authority::policy()
            .bucket_scratch_bytes
            .checked_sub(READ_CACHE_BYTES)
            .and_then(|remaining| remaining.checked_div(2))
            .ok_or_else(|| {
                CoreError::InvalidContract("compaction scratch ceiling is too small".into())
            })?;
        let store = store.metered_clone();
        let diagnostic = diagnostic.cloned();
        let retired_bytes = u64::try_from(native_retired.len())
            .map_err(|error| {
                CoreError::InvalidContract(format!("compaction retirement count: {error}"))
            })?
            .checked_mul(8);
        let native =
            native.filter(|_base| retired_bytes.is_some_and(|bytes| bytes <= packet_bytes));
        let native_packet_bytes =
            retired_bytes.map_or(0, |bytes| packet_bytes.saturating_sub(bytes));
        let native_retired = if native.is_some() {
            native_retired.to_vec()
        } else {
            Vec::new()
        };
        let budget = RequestBudgetV1::unbounded();
        let cancel = budget.cancel_handle();
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
                                    packet_bytes,
                                    Some(&budget),
                                )
                            })
                            .transpose()?
                            .flatten();
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
                        Ok(PreparedCompactions {
                            file,
                            native,
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
        }))
    }

    pub(crate) fn finish(mut self) -> Result<PreparedCompactions, CoreError> {
        self.worker
            .take()
            .ok_or_else(|| CoreError::Storage("compaction worker was already joined".into()))?
            .finish()?
    }
}

impl Drop for CompactionWorker {
    fn drop(&mut self) {
        self.cancel.cancel();
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
    use super::{CompactionWorker, JoinedWorker, PreparationPermit};
    use std::sync::atomic::{AtomicBool, Ordering};

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
        };
        drop(worker);
        assert!(completed.load(Ordering::Acquire));
    }
}
