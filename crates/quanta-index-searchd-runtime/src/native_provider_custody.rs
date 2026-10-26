//! Physical provider-attempt custody under the native service's original grant.
//!
//! The worker owns one strong authority while a request runs. A budget holds
//! only a weak port; each admitted transport thread holds one finite lease.
//! Dropping the worker's authority after cancellation therefore returns its
//! response promptly while the existing ledger keeps the grant until every
//! live attempt actually exits.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{
    CoreError, ProviderAttemptCustodyPortV1, ProviderAttemptLeaseV1,
    index::{IndexError, MemoryLease, MemoryLifetime, MemoryScope, effort::ReservationGuard},
};

use crate::native_resources::NativeClassScheduler;

struct Shared {
    reservation: ReservationGuard<NativeClassScheduler>,
    /// The state root that supplied the provider and attempt pool stays live
    /// until the last physical attempt exits after a cancelled request.
    _root_owner: Option<Arc<dyn Send + Sync>>,
    cap: u64,
    live: Mutex<u64>,
    invalid: AtomicBool,
}

/// Authority for the exact Native execution reservation that admits provider
/// IO. This wrapper neither creates another scheduler nor splits the grant.
pub(super) struct NativeProviderExecutionCustody {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for NativeProviderExecutionCustody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeProviderExecutionCustody")
            .field("provider_slots", &self.shared.cap)
            .finish_non_exhaustive()
    }
}

impl NativeProviderExecutionCustody {
    /// Consume the original admitted grant. A provider-capable execution must
    /// have at least one provider unit; the per-attempt cap is never greater
    /// than its charged `provider_units`.
    #[cfg(test)]
    pub(super) fn new(
        reservation: ReservationGuard<NativeClassScheduler>,
    ) -> Result<Arc<Self>, IndexError> {
        Self::construct(reservation, None)
    }

    /// Bind the original reservation and state-root owner before any attempt
    /// can start. The caller cannot install or swap an owner after spawning.
    pub(super) fn new_with_owner(
        reservation: ReservationGuard<NativeClassScheduler>,
        owner: Arc<dyn Send + Sync>,
    ) -> Result<Arc<Self>, IndexError> {
        Self::construct(reservation, Some(owner))
    }

    fn construct(
        reservation: ReservationGuard<NativeClassScheduler>,
        root_owner: Option<Arc<dyn Send + Sync>>,
    ) -> Result<Arc<Self>, IndexError> {
        let cap = reservation.need().provider_units;
        if cap == 0 {
            return Err(IndexError::InvalidInput(
                "provider execution requires reserved provider units".into(),
            ));
        }
        Ok(Arc::new(Self {
            shared: Arc::new(Shared {
                reservation,
                _root_owner: root_owner,
                cap,
                live: Mutex::new(0),
                invalid: AtomicBool::new(false),
            }),
        }))
    }

    pub(super) fn memory_scope(&self) -> Option<MemoryScope> {
        self.shared.reservation.memory_scope()
    }

    /// The unchanged grant for Kernel's exact `required_resources()` check.
    pub(super) fn reservation(&self) -> &ReservationGuard<NativeClassScheduler> {
        &self.shared.reservation
    }

    pub(super) fn port(self: &Arc<Self>) -> Arc<dyn ProviderAttemptCustodyPortV1> {
        self.clone()
    }

    #[cfg(test)]
    fn live_attempts(&self) -> u64 {
        match self.shared.live.lock() {
            Ok(live) => *live,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }
}

struct AttemptLease {
    shared: Arc<Shared>,
}

impl ProviderAttemptLeaseV1 for AttemptLease {}

impl Drop for AttemptLease {
    fn drop(&mut self) {
        // Poison is not permission to leak provider capacity. The only
        // mutation under this lock is one checked increment/decrement.
        let mut live = match self.shared.live.lock() {
            Ok(live) => live,
            Err(poisoned) => poisoned.into_inner(),
        };
        match (*live).checked_sub(1) {
            Some(next) => *live = next,
            None => self.shared.invalid.store(true, Ordering::Release),
        }
    }
}

impl ProviderAttemptCustodyPortV1 for NativeProviderExecutionCustody {
    fn begin_attempt_v1(&self) -> Result<Box<dyn ProviderAttemptLeaseV1>, CoreError> {
        let mut live = self.shared.live.lock().map_err(|error| {
            CoreError::Storage(format!("provider attempt custody lock poisoned: {error}"))
        })?;
        if self.shared.invalid.load(Ordering::Acquire) {
            return Err(CoreError::Storage(
                "provider attempt custody accounting is invalid".into(),
            ));
        }
        if *live >= self.shared.cap {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::ServerOverloaded,
                message: "reserved physical provider-attempt capacity is exhausted".into(),
            });
        }
        *live = (*live).checked_add(1).ok_or_else(|| {
            CoreError::Storage("provider attempt custody count overflowed".into())
        })?;
        drop(live);
        Ok(Box::new(AttemptLease {
            shared: Arc::clone(&self.shared),
        }))
    }

    fn reserve_memory_v1(
        &self,
        bytes: u64,
        lifetime: MemoryLifetime,
    ) -> Result<MemoryLease, CoreError> {
        self.shared
            .reservation
            .memory_scope()
            .ok_or_else(|| {
                CoreError::InvalidContract("native provider memory scope missing".into())
            })?
            .reserve(bytes, lifetime)
            .map_err(|error| match error {
                IndexError::ResourceLimited => CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::ServerOverloaded,
                    message: "native provider memory reservation exceeds admitted capacity".into(),
                },
                IndexError::WorkLimitExceeded => CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::ServerOverloaded,
                    message: "native provider execution work budget exhausted".into(),
                },
                other @ (IndexError::InvalidInput(_)
                | IndexError::Conflict
                | IndexError::DigestConflict
                | IndexError::FenceLost
                | IndexError::WorkFenceLost
                | IndexError::Cancelled
                | IndexError::SnapshotUnavailable
                | IndexError::CorruptObject(_)
                | IndexError::BackendViolation(_)
                | IndexError::Storage(_)
                | IndexError::RebuildRequired
                | IndexError::FocusSubjectNotFound
                | IndexError::MaintenanceRequired) => {
                    CoreError::Storage(format!("native provider memory admission: {other}"))
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use quanta_index_core::TextEmbeddingProvider;
    use quanta_index_core::{RequestBudgetV1, index::effort::ResourceNeed};
    #[cfg(unix)]
    use quanta_index_embed::{
        EmbeddingTransport, HttpResponse, OpenAiEmbeddingProvider, OpenAiProviderConfig,
        ProviderAttemptPool,
    };
    #[cfg(unix)]
    use quanta_index_searchd::app::runtime::StateRootLease;
    #[cfg(unix)]
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    use crate::native_resources::{NativeResourceClass, NativeResourceLedger, NativeResourcePlan};

    fn admitted_reservation_with_ledger(
        cap: u64,
    ) -> Result<
        (
            Arc<NativeResourceLedger>,
            ReservationGuard<NativeClassScheduler>,
        ),
        IndexError,
    > {
        let unit = ResourceNeed {
            cpu_units: 1,
            io_bytes: 1,
            memory_bytes: 64 * 1024,
            provider_units: cap,
        };
        let plan = NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 5,
                io_bytes: 5,
                memory_bytes: unit
                    .memory_bytes
                    .checked_mul(6)
                    .ok_or(IndexError::ResourceLimited)?,
                provider_units: cap.saturating_mul(5),
            },
            control: unit,
            interactive: unit,
            core: unit,
            enrichment: unit,
        };
        let ledger = NativeResourceLedger::new(plan)?;
        let scheduler = ledger.scheduler(NativeResourceClass::Enrichment);
        let reservation =
            ReservationGuard::try_acquire(scheduler, unit).ok_or(IndexError::ResourceLimited)?;
        Ok((ledger, reservation))
    }

    fn admitted_reservation(
        cap: u64,
    ) -> Result<ReservationGuard<NativeClassScheduler>, IndexError> {
        Ok(admitted_reservation_with_ledger(cap)?.1)
    }

    fn admitted(cap: u64) -> Result<Arc<NativeProviderExecutionCustody>, IndexError> {
        NativeProviderExecutionCustody::new(admitted_reservation(cap)?)
    }

    fn core<T>(result: Result<T, CoreError>) -> Result<T, IndexError> {
        result.map_err(|error| IndexError::BackendViolation(error.to_string()))
    }

    macro_rules! check {
        ($condition:expr $(, $message:expr)?) => {
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        };
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr) => {
            check!($left == $right);
        };
    }

    #[cfg(unix)]
    #[derive(Clone)]
    struct FixedOutputTransport {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        body: &'static str,
    }

    #[cfg(unix)]
    impl EmbeddingTransport for FixedOutputTransport {
        fn post_embeddings(
            &self,
            _url: &str,
            _key: &str,
            _body: &str,
            _timeout: Duration,
        ) -> Result<HttpResponse, CoreError> {
            let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(HttpResponse {
                status: 200,
                body: self.body.into(),
            })
        }

        fn post_embeddings_owned_bounded(
            &self,
            url: &str,
            key: &str,
            body: String,
            timeout: Duration,
            maximum: Option<u64>,
        ) -> Result<HttpResponse, CoreError> {
            let response = self.post_embeddings(url, key, &body, timeout)?;
            if maximum.is_some_and(|cap| {
                u64::try_from(response.body.len()).map_or(true, |size| size > cap)
            }) {
                return Err(CoreError::InvalidContract(
                    "fixed response exceeds bound".into(),
                ));
            }
            Ok(response)
        }
    }

    #[cfg(unix)]
    fn output_provider(
        transport: &FixedOutputTransport,
    ) -> Result<
        (
            quanta_index_core::L2UnitEmbeddingProvider<OpenAiEmbeddingProvider>,
            Arc<ProviderAttemptPool>,
        ),
        IndexError,
    > {
        let config =
            OpenAiProviderConfig::new("test".into(), "test-model".into(), "revision".into(), 2)
                .with_max_batch(1)
                .with_concurrency(2)
                .with_max_response_bytes(4096);
        let pool = Arc::new(core(ProviderAttemptPool::new(2))?);
        let provider = core(OpenAiEmbeddingProvider::new(
            config,
            Box::new((*transport).clone()),
        ))?
        .with_attempt_pool(Arc::clone(&pool))
        .with_required_execution_custody();
        Ok((
            core(quanta_index_core::L2UnitEmbeddingProvider::new(provider))?,
            pool,
        ))
    }

    #[cfg(unix)]
    #[test]
    fn provider_output_survives_normalize_merge_and_execution_without_slots()
    -> Result<(), IndexError> {
        let (ledger, reservation) = admitted_reservation_with_ledger(2)?;
        let custody = NativeProviderExecutionCustody::new(reservation)?;
        let mut budget = RequestBudgetV1::for_duration(Duration::from_secs(10));
        core(budget.bind_provider_attempt_custody_v1(&custody.port()))?;
        let transport = Arc::new(FixedOutputTransport {
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            body: r#"{"data":[{"embedding":[3.0,4.0],"index":0}]}"#,
        });
        let (provider, pool) = output_provider(&transport)?;
        let output = core(provider.embed_batch_within(&["first", "second"], &budget))?;
        check_eq!(output.len(), 2);
        let expected = [0.6_f32, 0.8];
        check_eq!(output.first().map(Vec::as_slice), Some(expected.as_slice()));
        check_eq!(output.get(1).map(Vec::as_slice), Some(expected.as_slice()));
        check_eq!(transport.calls.load(Ordering::SeqCst), 2);
        let drain = pool.drain(Duration::from_secs(5));
        check!(drain.unfinished.is_empty() && drain.panicked.is_empty());
        let live = ledger.snapshot()?.live_memory_bytes;
        check!(live > 0);
        drop(custody);
        let detached = ledger.snapshot()?;
        check_eq!(detached.live_memory_bytes, live);
        check_eq!(detached.reserved.cpu_units, 0);
        check_eq!(detached.reserved.io_bytes, 0);
        check_eq!(detached.reserved.provider_units, 0);
        check_eq!(output.first().map(Vec::as_slice), Some(expected.as_slice()));
        drop(output);
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn provider_output_denial_precedes_http_and_refuses_unscoped_extraction()
    -> Result<(), IndexError> {
        let (ledger, reservation) = admitted_reservation_with_ledger(1)?;
        let custody = NativeProviderExecutionCustody::new(reservation)?;
        let mut budget = RequestBudgetV1::for_duration(Duration::from_secs(10));
        core(budget.bind_provider_attempt_custody_v1(&custody.port()))?;
        let transport = Arc::new(FixedOutputTransport {
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            body: r#"{"data":[{"embedding":[3.0,4.0],"index":0}]}"#,
        });
        let (provider, pool) = output_provider(&transport)?;
        let fill = core(budget.reserve_provider_memory_v1(64 * 1024, MemoryLifetime::Temporary))?;
        check!(provider.embed_batch_within(&["first"], &budget).is_err());
        check_eq!(transport.calls.load(Ordering::SeqCst), 0);
        drop(fill);
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0);
        let output = core(provider.embed_batch_within(&["first"], &budget))?;
        check!(output.into_unscoped().is_err());
        let drain = pool.drain(Duration::from_secs(5));
        check!(drain.unfinished.is_empty() && drain.panicked.is_empty());
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0);
        drop(custody);
        check_eq!(ledger.snapshot()?.reserved.provider_units, 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn provider_output_parse_error_returns_all_data_charge() -> Result<(), IndexError> {
        let (ledger, reservation) = admitted_reservation_with_ledger(1)?;
        let custody = NativeProviderExecutionCustody::new(reservation)?;
        let mut budget = RequestBudgetV1::for_duration(Duration::from_secs(10));
        core(budget.bind_provider_attempt_custody_v1(&custody.port()))?;
        let transport = Arc::new(FixedOutputTransport {
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            body: r#"{"data":[{"embedding":[3.0,4.0,5.0],"index":0}]}"#,
        });
        let (provider, pool) = output_provider(&transport)?;
        check!(provider.embed_batch_within(&["invalid"], &budget).is_err());
        check_eq!(transport.calls.load(Ordering::SeqCst), 1);
        let drain = pool.drain(Duration::from_secs(5));
        check!(drain.unfinished.is_empty() && drain.panicked.is_empty());
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0);
        drop(custody);
        check_eq!(ledger.snapshot()?.reserved.provider_units, 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn native_bridge_output_and_query_keep_data_after_provider_execution() -> Result<(), IndexError>
    {
        use quanta_index_core::index::{
            Digest, ObjectRef, OwnerKey, ProducerId, SourceRecord, SourceTextAdmission,
            StableOwnerId,
        };
        use quanta_index_semantic::native::{
            EmbeddingInput, EmbeddingProvider, TextEmbeddingBridge,
        };

        let (ledger, reservation) = admitted_reservation_with_ledger(1)?;
        let custody = NativeProviderExecutionCustody::new(reservation)?;
        let mut budget = RequestBudgetV1::for_duration(Duration::from_secs(10));
        core(budget.bind_provider_attempt_custody_v1(&custody.port()))?;
        let (provider, pool) = output_provider(&FixedOutputTransport {
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            body: r#"{"data":[{"embedding":[3.0,4.0],"index":0}]}"#,
        })?;
        let bridge = TextEmbeddingBridge::new(provider)?;
        let recipe = bridge.recipe(64)?;
        let source = SourceRecord {
            path: "a.rs".into(),
            bytes: ObjectRef(Digest([1; 32])),
            byte_len: 5,
            text: SourceTextAdmission::Excluded,
        };
        let mut future = bridge.embed(EmbeddingInput {
            owner: OwnerKey {
                producer: ProducerId([0; 16]),
                stable_id: StableOwnerId([2; 16]),
            },
            binding: quanta_index_core::index::OwnerInputBinding {
                source: Some(ObjectRef(Digest([1; 32]))),
                projection: None,
                producer_context: Digest([0; 32]),
                derivation_policy: recipe.fingerprint(),
            },
            source: Some(&source),
            source_bytes: b"first",
            recipe: &recipe,
            request_budget: &budget,
        });
        // This bridge delegates to the synchronous provider and does not yield.
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let output = match std::future::Future::poll(future.as_mut(), &mut context) {
            std::task::Poll::Ready(result) => result?,
            std::task::Poll::Pending => {
                return Err(IndexError::BackendViolation(
                    "synchronous bridge unexpectedly yielded".into(),
                ));
            }
        };
        drop(future);
        let query = bridge.embed_query("query", &budget)?;
        let hit = output
            .first()
            .ok_or_else(|| IndexError::BackendViolation("bridge output missing".into()))?;
        let query_vector = query
            .first()
            .ok_or_else(|| IndexError::BackendViolation("bridge query missing".into()))?;
        let expected = [0.6_f32, 0.8];
        check_eq!(hit.id.as_slice(), b"whole-file");
        check_eq!(hit.values.as_slice(), expected.as_slice());
        check_eq!(query_vector.as_slice(), expected.as_slice());
        let drain = pool.drain(Duration::from_secs(5));
        check!(drain.unfinished.is_empty() && drain.panicked.is_empty());
        let live = ledger.snapshot()?.live_memory_bytes;
        check!(live > 0);
        drop(custody);
        let detached = ledger.snapshot()?;
        check_eq!(detached.live_memory_bytes, live);
        check_eq!(detached.reserved.cpu_units, 0);
        check_eq!(detached.reserved.io_bytes, 0);
        check_eq!(detached.reserved.provider_units, 0);
        drop(output);
        let query_live = ledger.snapshot()?.live_memory_bytes;
        check!(query_live > 0 && query_live < live);
        drop(query);
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0);
        Ok(())
    }

    #[test]
    fn attempt_cap_is_exact_original_grant_and_weak_budget_does_not_hold_it()
    -> Result<(), IndexError> {
        let owner = admitted(1)?;
        let weak = Arc::downgrade(&owner);
        let mut request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        core(request.bind_provider_attempt_custody_v1(&owner.port()))?;
        let escaped = request.clone();
        let physical = core(request.begin_provider_attempt_v1())?
            .ok_or_else(|| IndexError::BackendViolation("native lease missing".into()))?;
        check_eq!(owner.live_attempts(), 1);
        check!(matches!(
            request.begin_provider_attempt_v1(),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::ServerOverloaded,
                ..
            })
        ));
        drop(owner);
        check!(weak.upgrade().is_none(), "budget retains no strong owner");
        check!(matches!(
            escaped.begin_provider_attempt_v1(),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::ServerOverloaded,
                ..
            })
        ));
        drop(physical);
        Ok(())
    }

    #[test]
    fn cancelled_request_returns_while_physical_attempt_keeps_grant_charged()
    -> Result<(), IndexError> {
        let (ledger, reservation) = admitted_reservation_with_ledger(1)?;
        let owner = NativeProviderExecutionCustody::new(reservation)?;
        let mut request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        core(request.bind_provider_attempt_custody_v1(&owner.port()))?;
        let physical = core(request.begin_provider_attempt_v1())?
            .ok_or_else(|| IndexError::BackendViolation("native lease missing".into()))?;
        let scope = owner
            .memory_scope()
            .ok_or_else(|| IndexError::BackendViolation("native memory scope missing".into()))?;
        check_eq!(ledger.snapshot()?.reserved.provider_units, 1);
        request.cancel_handle().cancel();
        check!(matches!(
            request.checkpoint("semantic:embed"),
            Err(CoreError::Typed {
                code: quanta_index_core::REQUEST_CANCELLED_CODE,
                ..
            })
        ));
        drop(owner);
        check_eq!(ledger.snapshot()?.reserved.provider_units, 1);
        // The physical lease still holds the original reservation and scope.
        let scoped_memory =
            scope.reserve(1, quanta_index_core::index::MemoryLifetime::Temporary)?;
        drop(scoped_memory);
        drop(physical);
        check_eq!(ledger.snapshot()?.reserved.provider_units, 0);
        Ok(())
    }

    #[test]
    fn state_root_owner_survives_cancelled_caller_until_physical_attempt_exits()
    -> Result<(), IndexError> {
        let root: Arc<dyn Send + Sync> = Arc::new(());
        let weak = Arc::downgrade(&root);
        let owner = NativeProviderExecutionCustody::new_with_owner(
            admitted_reservation(1)?,
            Arc::clone(&root),
        )?;
        let mut request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        core(request.bind_provider_attempt_custody_v1(&owner.port()))?;
        let physical = core(request.begin_provider_attempt_v1())?
            .ok_or_else(|| IndexError::BackendViolation("native lease missing".into()))?;
        request.cancel_handle().cancel();
        drop(request);
        drop(owner);
        drop(root);
        check!(weak.upgrade().is_some(), "physical attempt retains root");
        drop(physical);
        check!(weak.upgrade().is_none(), "root released with final attempt");
        Ok(())
    }

    #[cfg(unix)]
    struct ParkedEmbeddingTransport {
        entered: mpsc::Sender<Duration>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    #[cfg(unix)]
    impl EmbeddingTransport for ParkedEmbeddingTransport {
        fn post_embeddings(
            &self,
            _url: &str,
            _api_key: &str,
            _body: &str,
            timeout: Duration,
        ) -> Result<HttpResponse, CoreError> {
            self.entered.send(timeout).map_err(|error| {
                CoreError::Storage(format!("embedding entry signal failed: {error}"))
            })?;
            self.release
                .lock()
                .map_err(|error| {
                    CoreError::Storage(format!("embedding barrier poisoned: {error}"))
                })?
                .recv()
                .map_err(|error| {
                    CoreError::Storage(format!("embedding release signal failed: {error}"))
                })?;
            Ok(HttpResponse {
                status: 200,
                body: r#"{"data":[{"embedding":[1.0,0.0],"index":0}]}"#.into(),
            })
        }

        fn post_embeddings_owned_bounded(
            &self,
            url: &str,
            api_key: &str,
            body: String,
            timeout: Duration,
            max_response_bytes: Option<u64>,
        ) -> Result<HttpResponse, CoreError> {
            // This test transport has one fixed response. The production
            // reqwest transport bounds the response while reading it.
            let response = self.post_embeddings(url, api_key, &body, timeout)?;
            let response_len = u64::try_from(response.body.len()).map_err(|_error| {
                CoreError::InvalidContract("test response length exceeds u64".into())
            })?;
            if max_response_bytes.is_some_and(|maximum| response_len > maximum) {
                return Err(CoreError::InvalidContract(
                    "test response exceeds configured byte ceiling".into(),
                ));
            }
            Ok(response)
        }
    }

    #[cfg(unix)]
    #[test]
    fn cancelled_openai_http_keeps_real_native_grant_and_state_root_until_physical_join()
    -> Result<(), IndexError> {
        use std::os::unix::fs::PermissionsExt;

        let state_root = tempfile::tempdir().map_err(|error| {
            IndexError::Storage(format!("temporary state root creation failed: {error}"))
        })?;
        std::fs::set_permissions(state_root.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(|error| {
                IndexError::Storage(format!("secure temporary state root: {error}"))
            })?;
        let root = Arc::new(core(StateRootLease::acquire(state_root.path()))?);
        let (ledger, reservation) = admitted_reservation_with_ledger(1)?;
        let owner: Arc<dyn Send + Sync> = root.clone();
        let custody = NativeProviderExecutionCustody::new_with_owner(reservation, owner)?;
        let pool = Arc::new(core(ProviderAttemptPool::new(1))?);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let transport = ParkedEmbeddingTransport {
            entered: entered_tx,
            release: Mutex::new(release_rx),
        };
        let mut config = OpenAiProviderConfig::new(
            "test-key".into(),
            "text-embedding-3-small".into(),
            "test-revision".into(),
            2,
        );
        config.max_batch = 1;
        config.max_retries = 0;
        config.concurrency = 1;
        config.timeout = Duration::from_secs(10);
        config.max_response_bytes = Some(4096);
        let provider = core(OpenAiEmbeddingProvider::new(config, Box::new(transport)))?
            .with_attempt_pool(Arc::clone(&pool))
            .with_required_execution_custody();
        let mut request = RequestBudgetV1::for_duration(Duration::from_secs(3));
        let original_remaining = request.remaining();
        core(request.bind_provider_attempt_custody_v1(&custody.port()))?;
        let escaped_budget = request.clone();
        let cancel = request.cancel_handle();
        let (outcome_tx, outcome_rx) = mpsc::channel();
        let caller = std::thread::spawn(move || {
            let outcome = provider.embed_batch_within(&["physical-attempt"], &request);
            let _delivered = outcome_tx.send(outcome);
        });

        // Even on an assertion failure, release the parked transport, join
        // the caller, and drain the physical attempt before ending the test.
        let entered = entered_rx.recv_timeout(Duration::from_secs(2));
        cancel.cancel();
        let outcome = outcome_rx.recv_timeout(Duration::from_secs(2));
        drop(escaped_budget);
        drop(custody);
        drop(root);
        let proof = (|| -> Result<(), IndexError> {
            let passed_timeout = entered.map_err(|error| {
                IndexError::BackendViolation(format!("physical HTTP did not start: {error}"))
            })?;
            check!(!passed_timeout.is_zero());
            check!(passed_timeout <= original_remaining);
            let result = outcome.map_err(|error| {
                IndexError::BackendViolation(format!("cancelled caller did not return: {error}"))
            })?;
            check!(matches!(
                result,
                Err(CoreError::Typed {
                    code: quanta_index_core::REQUEST_CANCELLED_CODE,
                    ..
                })
            ));
            check_eq!(ledger.snapshot()?.reserved.provider_units, 1);
            check!(matches!(
                StateRootLease::acquire(state_root.path()),
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::StateRootInUse,
                    ..
                })
            ));
            Ok(())
        })();

        let _released = release_tx.send(());
        let caller_joined = caller.join();
        let drained = pool.drain(Duration::from_secs(5));
        caller_joined.map_err(|_panicked| {
            IndexError::BackendViolation("embedding caller thread panicked".into())
        })?;
        proof?;
        check!(drained.unfinished.is_empty());
        check!(drained.panicked.is_empty());
        check_eq!(drained.joined, 1);
        check_eq!(ledger.snapshot()?.reserved.provider_units, 0);
        let reacquired = core(StateRootLease::acquire(state_root.path()))?;
        drop(reacquired);
        Ok(())
    }
}
