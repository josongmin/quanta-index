//! Per-region Lance session backed by an admitted, bounded in-memory store.
//!
//! A build session is writable until `freeze_and_snapshot`. A selected read
//! session rehydrates verified managed chunks, freezes, and then connects.
//! The registry is private to this session; no process-global memory store is
//! used to resolve a persisted ANN descriptor.

use std::{fmt, sync::Arc};

use async_trait::async_trait;
use lance::{Dataset, dataset::builder::DatasetBuilder, session::Session};
use lance_io::object_store::{
    ObjectStore as LanceObjectStore, ObjectStoreParams, ObjectStoreProvider, ObjectStoreRegistry,
    providers::memory::MemoryStoreProvider,
};
use object_store::path::Path;
use quanta_index_core::{
    RequestBudgetV1,
    index::{IndexError, WorkBudget},
};
use url::Url;

use super::{
    ann_files::LoadedAnnFileSet,
    ann_lance::{self, ANN_DATASET_URI, RegionRow, RegionSeal},
    ann_store::{AnnFile, AnnMemoryStore, AnnStoreLimits},
};

#[derive(Debug)]
struct AdmittedMemoryProvider {
    inner: Arc<AnnMemoryStore>,
    delegate: MemoryStoreProvider,
}

#[async_trait]
impl ObjectStoreProvider for AdmittedMemoryProvider {
    async fn new_store(
        &self,
        base_path: Url,
        params: &ObjectStoreParams,
    ) -> lance::Result<LanceObjectStore> {
        let mut store = self.delegate.new_store(base_path, params).await?;
        store.inner = self.inner.clone();
        Ok(store)
    }

    fn extract_path(&self, url: &Url) -> lance::Result<Path> {
        self.delegate.extract_path(url)
    }

    #[expect(
        clippy::disallowed_types,
        reason = "pinned Lance provider ABI requires HashMap"
    )]
    fn calculate_object_store_prefix(
        &self,
        url: &Url,
        storage_options: Option<&std::collections::HashMap<String, String>>,
    ) -> lance::Result<String> {
        self.delegate
            .calculate_object_store_prefix(url, storage_options)
    }
}

/// One bounded native ANN build or selected-ticket read lifetime.
pub(super) struct AnnLanceSession {
    store: Arc<AnnMemoryStore>,
    session: Arc<Session>,
    request: RequestBudgetV1,
}

impl fmt::Debug for AnnLanceSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnnLanceSession")
            .finish_non_exhaustive()
    }
}

impl AnnLanceSession {
    #[cfg(test)]
    pub(in crate::native) fn arm_test_read_gate(&self, gate: Arc<super::ann_store::TestReadGate>) {
        self.store.arm_test_read_gate(gate);
    }

    /// Admit all store work and backing against the original request budget.
    pub(super) fn new(limits: AnnStoreLimits, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let store = AnnMemoryStore::new(limits, budget)?;
        Self::with_store(store, budget)
    }

    fn with_store(store: Arc<AnnMemoryStore>, budget: &WorkBudget) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let request = budget.request().clone();
        let registry = ObjectStoreRegistry::empty();
        registry.insert(
            "memory",
            Arc::new(AdmittedMemoryProvider {
                inner: Arc::clone(&store),
                delegate: MemoryStoreProvider,
            }),
        );
        // Use only the request-owned store and disable Lance's separate caches.
        let session = Arc::new(Session::new(0, 0, Arc::new(registry)));
        budget.checkpoint()?;
        Ok(Self {
            store,
            session,
            request,
        })
    }

    /// Reconstruct the selected descriptor into a private, read-only store.
    pub(super) async fn hydrate(
        fileset: &LoadedAnnFileSet,
        limits: AnnStoreLimits,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let store = AnnMemoryStore::from_verified_files(&fileset.files, limits, budget).await?;
        Self::with_store(store, budget)
    }

    pub(super) fn read_store(&self) -> Arc<AnnMemoryStore> {
        Arc::clone(&self.store)
    }

    /// Create only the fixed dataset in this bounded session.
    pub(super) async fn build_region(
        &self,
        rows: &[RegionRow<'_>],
        recipe: quanta_index_core::index::Digest,
        generation: u64,
        dimension: usize,
        budget: &mut WorkBudget,
    ) -> Result<(Arc<Dataset>, RegionSeal), IndexError> {
        self.checkpoint()?;
        ann_lance::build_region_direct(
            Arc::clone(&self.session),
            rows,
            recipe,
            generation,
            dimension,
            budget,
        )
        .await
    }

    pub(super) async fn open_dataset(&self) -> Result<Arc<Dataset>, IndexError> {
        self.checkpoint()?;
        let loaded = DatasetBuilder::from_uri(ANN_DATASET_URI)
            .with_session(Arc::clone(&self.session))
            .load()
            .await
            .map(Arc::new)
            .map_err(|error| IndexError::Storage(format!("ANN selected table: {error}")));
        let dataset = self.resolve(loaded)?;
        self.checkpoint()?;
        Ok(dataset)
    }

    /// Vendor helpers can render store errors into strings. Consume the refusal
    /// at this operation boundary so it cannot classify a later operation.
    pub(super) fn resolve<T>(&self, result: Result<T, IndexError>) -> Result<T, IndexError> {
        let denial = self.store.take_denial();
        match result {
            Ok(value) => Ok(value),
            Err(error @ IndexError::Storage(_)) => Err(denial.unwrap_or(error)),
            Err(error) => Err(error),
        }
    }

    /// Seal write access before returning an installable sorted inventory.
    pub(super) async fn freeze_and_snapshot(&self) -> Result<Vec<AnnFile>, IndexError> {
        self.checkpoint()?;
        self.store.freeze().await?;
        let files = self.store.snapshot().await?;
        self.checkpoint()?;
        Ok(files)
    }

    fn checkpoint(&self) -> Result<(), IndexError> {
        if self.request.is_cancelled() {
            Err(IndexError::Cancelled)
        } else if self.request.remaining().is_zero() {
            Err(IndexError::ResourceLimited)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use object_store::{ObjectStoreExt as _, PutPayload};
    use quanta_index_core::index::WorkLimits;

    fn test_session() -> Result<(AnnLanceSession, WorkBudget), IndexError> {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 32,
                writes: 32,
                bytes: 64,
                mutations: 32,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let session = AnnLanceSession::new(
            AnnStoreLimits {
                max_write_bytes: 1,
                max_read_bytes: 1,
                max_ops: 8,
                max_files: 1,
                max_path_bytes: 16,
                max_part_bytes: 1,
            },
            &mut budget,
        )?;
        Ok((session, budget))
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "the registry must return the exact bounded store owned by the ANN session"
    )]
    fn registry_uses_bounded_store_and_freeze_refuses_later_writes() -> Result<(), IndexError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| IndexError::Storage(format!("ANN test runtime: {error}")))?;
        crate::run_blocking(&runtime, async {
            let (session, _budget) = test_session()?;
            let (other_session, _other_budget) = test_session()?;
            let registry = session.session.store_registry();
            assert!(registry.get_provider("file").is_none());
            assert!(registry.get_provider("shared-memory").is_none());
            let uri = Url::parse(ANN_DATASET_URI)
                .map_err(|error| IndexError::Storage(format!("ANN test URI: {error}")))?;
            let store = registry
                .get_store(uri.clone(), &ObjectStoreParams::default())
                .await
                .map_err(|error| IndexError::Storage(format!("ANN registry store: {error}")))?;
            let other_store = other_session
                .session
                .store_registry()
                .get_store(uri, &ObjectStoreParams::default())
                .await
                .map_err(|error| {
                    IndexError::Storage(format!("ANN other registry store: {error}"))
                })?;
            let path = Path::from("a");
            let _written = store
                .inner
                .put(&path, PutPayload::from_static(b"x"))
                .await
                .map_err(super::super::ann_store::index_error)?;
            // The registry wraps the injected store for tracing and I/O
            // accounting, so compare its behavior and backing rather than
            // the outer Arc pointer.
            assert!(
                store
                    .inner
                    .put(&Path::from("b"), PutPayload::from_static(b"x"))
                    .await
                    .is_err()
            );
            assert!(matches!(
                session.store.take_denial(),
                Some(IndexError::ResourceLimited)
            ));
            assert!(matches!(
                other_store.inner.get(&path).await,
                Err(object_store::Error::NotFound { .. })
            ));
            let files = session.freeze_and_snapshot().await?;
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].path, "a");
            assert_eq!(files[0].bytes.as_ref(), b"x");
            assert!(
                store
                    .inner
                    .put(&path, PutPayload::from_static(b"y"))
                    .await
                    .is_err()
            );
            Ok(())
        })
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "the error classes are the test oracle"
    )]
    fn resolve_consumes_refusal_and_preserves_unrelated_or_typed_errors() -> Result<(), IndexError>
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| IndexError::Storage(format!("ANN test runtime: {error}")))?;
        crate::run_blocking(&runtime, async {
            let (session, _budget) = test_session()?;
            let path = Path::from("a");
            let _written = session
                .store
                .put(&path, PutPayload::from_static(b"x"))
                .await
                .map_err(super::super::ann_store::index_error)?;
            let _read = session
                .store
                .get(&path)
                .await
                .map_err(super::super::ann_store::index_error)?;

            let refused = session.store.get(&path).await;
            assert!(matches!(
                refused.map_err(super::super::ann_store::index_error),
                Err(IndexError::ResourceLimited)
            ));
            assert_eq!(session.resolve(Ok::<_, IndexError>(7))?, 7);
            let unrelated = IndexError::Storage("unrelated vendor failure".into());
            assert_eq!(
                session.resolve::<()>(Err(unrelated.clone())),
                Err(unrelated)
            );

            let _refused = session.store.get(&path).await;
            let corrupt = IndexError::CorruptObject("authenticated seal mismatch".into());
            assert_eq!(session.resolve::<()>(Err(corrupt.clone())), Err(corrupt));

            let _refused = session.store.get(&path).await;
            assert_eq!(
                session.resolve::<()>(Err(IndexError::Cancelled)),
                Err(IndexError::Cancelled)
            );

            let refused = session.store.get(&path).await.expect_err("quota refusal");
            let vendor = IndexError::Storage(format!("ANN vendor: {refused}"));
            assert_eq!(
                session.resolve::<()>(Err(vendor)),
                Err(IndexError::ResourceLimited)
            );
            Ok(())
        })
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "the recovered cancellation is the test oracle"
    )]
    fn resolve_recovers_store_cancellation_after_vendor_stringification() -> Result<(), IndexError>
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| IndexError::Storage(format!("ANN test runtime: {error}")))?;
        crate::run_blocking(&runtime, async {
            let (session, budget) = test_session()?;
            budget.request().cancel_handle().cancel();
            let store_error = session
                .store
                .get(&Path::from("a"))
                .await
                .expect_err("cancelled store read");
            let vendor = IndexError::Storage(format!("ANN vendor: {store_error}"));
            assert_eq!(
                super::super::ann_store::index_error(store_error),
                IndexError::Cancelled
            );
            assert_eq!(
                session.resolve::<()>(Err(vendor)),
                Err(IndexError::Cancelled)
            );
            Ok(())
        })
    }
}
