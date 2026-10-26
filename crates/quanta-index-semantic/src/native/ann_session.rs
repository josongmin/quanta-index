//! Per-region Lance session backed by an admitted, bounded in-memory store.
//!
//! A build session is writable until `freeze_and_snapshot`. A selected read
//! session rehydrates verified managed chunks, freezes, and then connects.
//! The registry is private to this session; no process-global memory store is
//! used to resolve a persisted ANN descriptor.

use std::{fmt, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;
use lance::session::Session;
use lance_io::object_store::{
    ObjectStore as LanceObjectStore, ObjectStoreParams, ObjectStoreProvider, ObjectStoreRegistry,
    providers::memory::MemoryStoreProvider,
};
use object_store::{ObjectStore as _, PutMultipartOptions, PutOptions, PutPayload, path::Path};
use quanta_index_core::{
    RequestBudgetV1,
    index::{IndexError, StoredObject, WorkBudget},
};
use url::Url;

use super::{
    ann_files::LoadedAnnFileSet,
    ann_store::{AnnFile, AnnMemoryStore, AnnStoreLimits},
};

/// The authority and path are fixed across build and selected-ticket reopen.
/// Each connection receives a distinct `Session` and provider instance.
const ANN_URI: &str = "memory://native-ann/region";

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

    fn requires_provider_path(&self) -> bool {
        true
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

struct StoredChunkBytes(StoredObject);

impl AsRef<[u8]> for StoredChunkBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0.payload
    }
}

fn store_error(error: object_store::Error) -> IndexError {
    super::ann_store::index_error(error)
}

impl AnnLanceSession {
    /// Admit all store work and backing against the original request budget.
    pub(super) fn new(limits: AnnStoreLimits, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let request = budget.request().clone();
        let store = AnnMemoryStore::new(limits, budget)?;
        // Lance's default cache capacities are separate memory allocations;
        // disable them for this region instead of escaping the work grant.
        let session = Arc::new(Session::new(0, 0, Arc::new(ObjectStoreRegistry::default())));
        session.store_registry().insert(
            "memory",
            Arc::new(AdmittedMemoryProvider {
                inner: Arc::clone(&store),
                delegate: MemoryStoreProvider,
            }),
        );
        budget.checkpoint()?;
        Ok(Self {
            store,
            session,
            request,
        })
    }

    /// Reconstruct the selected descriptor into a private, read-only store.
    /// Each source chunk keeps its managed-object memory grant through upload.
    /// The only contiguous copy is the bounded per-file `InMemory` completion.
    pub(super) async fn hydrate(
        fileset: &LoadedAnnFileSet,
        limits: AnnStoreLimits,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let instance = Self::new(limits, budget)?;
        for file in &fileset.files {
            budget.checkpoint()?;
            let location = Path::parse(&file.path)
                .map_err(|error| IndexError::CorruptObject(format!("ANN file path: {error}")))?;
            if location.as_ref() != file.path.as_str() {
                return Err(IndexError::CorruptObject(
                    "ANN file path changed during hydration".into(),
                ));
            }
            if file.chunks.is_empty() {
                let _put = instance
                    .store
                    .put_opts(&location, PutPayload::new(), PutOptions::default())
                    .await
                    .map_err(store_error)?;
                continue;
            }
            let mut upload = instance
                .store
                .put_multipart_opts(&location, PutMultipartOptions::default())
                .await
                .map_err(store_error)?;
            let mut total = 0u64;
            for chunk in &file.chunks {
                budget.checkpoint()?;
                total = total
                    .checked_add(u64::from(chunk.length))
                    .ok_or(IndexError::ResourceLimited)?;
                let bytes = Bytes::from_owner(StoredChunkBytes(chunk.object.clone()));
                upload
                    .put_part(PutPayload::from_bytes(bytes))
                    .await
                    .map_err(store_error)?;
            }
            if total != file.length {
                return Err(IndexError::CorruptObject(
                    "ANN file length changed during hydration".into(),
                ));
            }
            budget.checkpoint()?;
            let _put = upload.complete().await.map_err(store_error)?;
        }
        budget.checkpoint()?;
        instance.store.freeze().await?;
        Ok(instance)
    }

    /// A connection uses this instance's registry, including on reopen.
    pub(super) async fn connect(&self) -> Result<lancedb::Connection, IndexError> {
        self.checkpoint()?;
        let connection = lancedb::connect(ANN_URI)
            .session(Arc::clone(&self.session))
            .execute()
            .await
            .map_err(|error| IndexError::Storage(format!("ANN Lance connect: {error}")));
        let connection = self.resolve(connection)?;
        self.checkpoint()?;
        Ok(connection)
    }

    /// Vendor helpers can render backend errors into strings. Keep the original
    /// facade refusal class while preserving unrelated failures.
    pub(super) fn resolve<T>(&self, result: Result<T, IndexError>) -> Result<T, IndexError> {
        result.map_err(|error| self.store.first_denial().unwrap_or(error))
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
