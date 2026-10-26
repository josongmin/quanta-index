//! Bounded, attempt-local Lance staging. Persistent bytes are installed through
//! the managed object graph by the caller after this store is frozen.

use std::{
    collections::BTreeSet,
    fmt,
    ops::Range,
    sync::{Arc, Mutex, OnceLock},
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt as _, TryStreamExt as _, stream::BoxStream};
use object_store::{
    CopyOptions, GetOptions, GetResult, GetResultPayload, ListResult, MultipartUpload, ObjectMeta,
    ObjectStore, ObjectStoreExt as _, PutMultipartOptions, PutOptions, PutPayload, PutResult,
    UploadPart, memory::InMemory, path::Path,
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage},
};
use tokio::sync::RwLock;

const MIB: u64 = 1024 * 1024;

/// All limits are cumulative for one region build, including failed attempts.
#[derive(Clone, Copy, Debug)]
#[expect(
    clippy::struct_field_names,
    reason = "the max prefix distinguishes policy caps from live usage counters"
)]
pub(super) struct AnnStoreLimits {
    pub max_write_bytes: u64,
    pub max_read_bytes: u64,
    pub max_ops: u64,
    pub max_files: u32,
    pub max_path_bytes: usize,
    pub max_part_bytes: u64,
}

impl Default for AnnStoreLimits {
    fn default() -> Self {
        Self {
            max_write_bytes: 32 * MIB,
            max_read_bytes: 64 * MIB,
            max_ops: 4096,
            max_files: 256,
            max_path_bytes: 256,
            max_part_bytes: 32 * MIB,
        }
    }
}

impl AnnStoreLimits {
    fn validate(self) -> Result<(), IndexError> {
        if self.max_write_bytes == 0
            || self.max_read_bytes == 0
            || self.max_ops == 0
            || self.max_files == 0
            || self.max_path_bytes == 0
            || self.max_path_bytes > 256
            || self.max_part_bytes == 0
            || self.max_part_bytes > 32 * MIB
            || self.max_write_bytes > 32 * MIB
            || self.max_read_bytes > 64 * MIB
            || self.max_ops > 4096
            || self.max_files > 256
        {
            return Err(IndexError::InvalidInput(
                "ANN staging exceeds bounded region policy".into(),
            ));
        }
        Ok(())
    }

    fn admitted_memory(self) -> Result<u64, IndexError> {
        // InMemory may hold multipart parts and a completed contiguous copy at
        // once. Include sorted path/operation metadata; returned Bytes shares
        // this one custody owner and cannot outlive its charge.
        let path_capacity = u64::try_from(self.max_path_bytes)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(256)
            .ok_or(IndexError::ResourceLimited)?;
        let path_metadata = u64::from(self.max_files)
            .checked_mul(path_capacity)
            .ok_or(IndexError::ResourceLimited)?;
        self.max_write_bytes
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(path_metadata))
            .and_then(|bytes| bytes.checked_add(self.max_ops.checked_mul(256)?))
            .ok_or(IndexError::ResourceLimited)
    }
}

#[derive(Debug)]
struct Counters {
    write_bytes: u64,
    read_bytes: u64,
    ops: u64,
    paths: BTreeSet<Path>,
    active_uploads: u32,
    frozen: bool,
}

impl Counters {
    fn charge(
        &mut self,
        limits: AnnStoreLimits,
        path: Option<&Path>,
        write_bytes: u64,
        read_bytes: u64,
        mutation: bool,
    ) -> object_store::Result<()> {
        if mutation && self.frozen {
            return Err(denied("ANN staging is frozen"));
        }
        let next_ops = self
            .ops
            .checked_add(1)
            .ok_or_else(|| denied("ANN op overflow"))?;
        let next_write = self
            .write_bytes
            .checked_add(write_bytes)
            .ok_or_else(|| denied("ANN write overflow"))?;
        let next_read = self
            .read_bytes
            .checked_add(read_bytes)
            .ok_or_else(|| denied("ANN read overflow"))?;
        if next_ops > limits.max_ops
            || next_write > limits.max_write_bytes
            || next_read > limits.max_read_bytes
        {
            return Err(denied("ANN cumulative work limit exceeded"));
        }
        if let Some(path) = path {
            validate_path(path, limits.max_path_bytes)?;
            let file_limit = usize::try_from(limits.max_files)
                .map_err(|_error| denied("ANN file count conversion overflow"))?;
            if mutation && !self.paths.contains(path) && self.paths.len() >= file_limit {
                return Err(denied("ANN file count exceeded"));
            }
        }
        self.ops = next_ops;
        self.write_bytes = next_write;
        self.read_bytes = next_read;
        if mutation && let Some(path) = path {
            let _inserted = self.paths.insert(path.clone());
        }
        Ok(())
    }
}

#[derive(Debug)]
struct AnnFailure {
    error: IndexError,
    context: &'static str,
}

impl fmt::Display for AnnFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.context, self.error)
    }
}

impl std::error::Error for AnnFailure {}

fn object_error(error: IndexError, context: &'static str) -> object_store::Error {
    object_store::Error::Generic {
        store: "AnnMemoryStore",
        source: Box::new(AnnFailure { error, context }),
    }
}

fn denied(message: &'static str) -> object_store::Error {
    object_error(IndexError::ResourceLimited, message)
}

pub(super) fn index_error(error: object_store::Error) -> IndexError {
    if let object_store::Error::Generic { store, source } = error {
        if let Some(failure) = source.downcast_ref::<AnnFailure>() {
            return failure.error.clone();
        }
        IndexError::Storage(format!("ANN store {store}: {source}"))
    } else {
        IndexError::Storage(format!("ANN store: {error}"))
    }
}

fn validate_path(path: &Path, max_bytes: usize) -> object_store::Result<()> {
    let value: &str = path.as_ref();
    if value.is_empty()
        || value.len() > max_bytes
        || value.starts_with('/')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(object_error(
            IndexError::InvalidInput("ANN path is not bounded and normalized".into()),
            "ANN path",
        ));
    }
    Ok(())
}

fn checkpoint(request: &RequestBudgetV1) -> object_store::Result<()> {
    if request.is_cancelled() {
        Err(object_error(IndexError::Cancelled, "ANN request cancelled"))
    } else if request.remaining().is_zero() {
        Err(object_error(
            IndexError::ResourceLimited,
            "ANN request deadline expired",
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone)]
struct Shared {
    counters: Arc<Mutex<Counters>>,
    mutation_gate: Arc<RwLock<()>>,
    request: RequestBudgetV1,
    limits: AnnStoreLimits,
    backing: Option<MemoryLease>,
    first_denial: Arc<OnceLock<IndexError>>,
}

impl Shared {
    fn charge(
        &self,
        path: Option<&Path>,
        write_bytes: u64,
        read_bytes: u64,
        mutation: bool,
    ) -> object_store::Result<()> {
        checkpoint(&self.request)?;
        let result = self
            .counters
            .lock()
            .map_err(|_error| denied("ANN counters poisoned"))?
            .charge(self.limits, path, write_bytes, read_bytes, mutation);
        if let Err(object_store::Error::Generic { source, .. }) = &result
            && let Some(failure) = source.downcast_ref::<AnnFailure>()
        {
            let _first = self.first_denial.set(failure.error.clone());
        }
        result
    }
}

#[derive(Clone)]
pub(super) struct AnnMemoryStore {
    inner: InMemory,
    shared: Shared,
}

impl fmt::Debug for AnnMemoryStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnnMemoryStore")
            .finish_non_exhaustive()
    }
}

impl fmt::Display for AnnMemoryStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AnnMemoryStore")
    }
}

struct OwnedBytes {
    data: Bytes,
    _backing: Option<MemoryLease>,
}

impl AsRef<[u8]> for OwnedBytes {
    fn as_ref(&self) -> &[u8] {
        self.data.as_ref()
    }
}

fn owned_bytes(data: Bytes, backing: Option<MemoryLease>) -> Bytes {
    Bytes::from_owner(OwnedBytes {
        data,
        _backing: backing,
    })
}

pub(super) struct AnnFile {
    pub path: String,
    pub bytes: Bytes,
}

impl AnnMemoryStore {
    /// Prepay one original work envelope and retain backing through all Bytes.
    pub(super) fn new(
        limits: AnnStoreLimits,
        budget: &mut WorkBudget,
    ) -> Result<Arc<Self>, IndexError> {
        limits.validate()?;
        let bytes = limits
            .max_write_bytes
            .checked_add(limits.max_read_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            reads: limits.max_ops,
            writes: limits.max_ops,
            bytes,
            mutations: limits.max_ops,
        })?;
        let backing = budget.reserve_memory(limits.admitted_memory()?, MemoryLifetime::Retained)?;
        Ok(Arc::new(Self {
            inner: InMemory::new(),
            shared: Shared {
                counters: Arc::new(Mutex::new(Counters {
                    write_bytes: 0,
                    read_bytes: 0,
                    ops: 0,
                    paths: BTreeSet::new(),
                    active_uploads: 0,
                    frozen: false,
                })),
                mutation_gate: Arc::new(RwLock::new(())),
                request: budget.request().clone(),
                limits,
                backing,
                first_denial: Arc::new(OnceLock::new()),
            },
        }))
    }

    /// Preserve the facade's first typed refusal across vendor string errors.
    pub(super) fn first_denial(&self) -> Option<IndexError> {
        self.shared.first_denial.get().cloned()
    }

    /// Wait for in-flight mutations before sealing the file inventory.
    pub(super) async fn freeze(&self) -> Result<(), IndexError> {
        let _exclusive = self.shared.mutation_gate.write().await;
        if self.shared.request.is_cancelled() {
            return Err(IndexError::Cancelled);
        }
        if self.shared.request.remaining().is_zero() {
            return Err(IndexError::ResourceLimited);
        }
        let mut counters = self
            .shared
            .counters
            .lock()
            .map_err(|_error| IndexError::BackendViolation("ANN counters poisoned".into()))?;
        if counters.active_uploads != 0 {
            return Err(IndexError::BackendViolation(
                "ANN multipart upload has not settled".into(),
            ));
        }
        counters.frozen = true;
        drop(counters);
        Ok(())
    }

    /// Frozen, lexicographically sorted files; each payload retains the charge.
    pub(super) async fn snapshot(&self) -> Result<Vec<AnnFile>, IndexError> {
        {
            let counters =
                self.shared.counters.lock().map_err(|_error| {
                    IndexError::BackendViolation("ANN counters poisoned".into())
                })?;
            if !counters.frozen {
                return Err(IndexError::BackendViolation(
                    "ANN snapshot requires a frozen store".into(),
                ));
            }
        }
        let mut metadata: Vec<ObjectMeta> =
            self.list(None).try_collect().await.map_err(index_error)?;
        let file_limit = usize::try_from(self.shared.limits.max_files)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if metadata.len() > file_limit {
            return Err(IndexError::BackendViolation(
                "ANN inventory exceeded cap".into(),
            ));
        }
        metadata.sort_by(|a, b| a.location.cmp(&b.location));
        let mut files = Vec::new();
        files
            .try_reserve_exact(metadata.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        for meta in metadata {
            checkpoint(&self.shared.request).map_err(index_error)?;
            let path = meta.location.to_string();
            let bytes = self
                .get(&meta.location)
                .await
                .map_err(index_error)?
                .bytes()
                .await
                .map_err(index_error)?;
            if u64::try_from(bytes.len()).map_err(|_error| IndexError::ResourceLimited)?
                != meta.size
            {
                return Err(IndexError::CorruptObject(
                    "ANN inventory length changed after freeze".into(),
                ));
            }
            files.push(AnnFile { path, bytes });
        }
        Ok(files)
    }
}

#[async_trait]
impl ObjectStore for AnnMemoryStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        let _mutation = self.shared.mutation_gate.read().await;
        let bytes = u64::try_from(payload.content_length()).map_err(|_error| denied("ANN size"))?;
        self.shared.charge(Some(location), bytes, 0, true)?;
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        let _mutation = self.shared.mutation_gate.read().await;
        self.shared.charge(Some(location), 0, 0, true)?;
        let inner = self.inner.put_multipart_opts(location, opts).await?;
        let mut counters = self
            .shared
            .counters
            .lock()
            .map_err(|_error| denied("ANN counters poisoned"))?;
        counters.active_uploads = counters
            .active_uploads
            .checked_add(1)
            .ok_or_else(|| denied("ANN upload count overflow"))?;
        drop(counters);
        Ok(Box::new(AnnUpload {
            inner,
            path: location.clone(),
            shared: self.shared.clone(),
            settled: false,
        }))
    }

    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        checkpoint(&self.shared.request)?;
        validate_path(location, self.shared.limits.max_path_bytes)?;
        let meta = self.inner.head(location).await?;
        let head_only = options.head;
        let read_bytes = if head_only {
            0
        } else if let Some(range) = &options.range {
            let range =
                range
                    .as_range(meta.size)
                    .map_err(|error| object_store::Error::Generic {
                        store: "AnnMemoryStore",
                        source: Box::new(error),
                    })?;
            range
                .end
                .checked_sub(range.start)
                .ok_or_else(|| denied("ANN read range invalid"))?
        } else {
            meta.size
        };
        self.shared.charge(Some(location), 0, read_bytes, false)?;
        let mut result = self.inner.get_opts(location, options).await?;
        if head_only {
            result.range = 0..0;
            result.payload = GetResultPayload::Stream(futures::stream::empty().boxed());
            return Ok(result);
        }
        let GetResultPayload::Stream(stream) = result.payload else {
            return Err(object_error(
                IndexError::BackendViolation("ANN memory backend returned a file handle".into()),
                "ANN read",
            ));
        };
        let backing = self.shared.backing.clone();
        result.payload = GetResultPayload::Stream(
            stream
                .map(move |item| item.map(|data| owned_bytes(data, backing.clone())))
                .boxed(),
        );
        Ok(result)
    }

    async fn get_ranges(
        &self,
        location: &Path,
        ranges: &[Range<u64>],
    ) -> object_store::Result<Vec<Bytes>> {
        if u64::try_from(ranges.len()).map_err(|_error| denied("ANN range count"))?
            > self.shared.limits.max_ops
        {
            return Err(denied("ANN range count exceeded"));
        }
        let bytes = ranges.iter().try_fold(0u64, |total, range| {
            total
                .checked_add(
                    range
                        .end
                        .checked_sub(range.start)
                        .ok_or_else(|| denied("ANN range invalid"))?,
                )
                .ok_or_else(|| denied("ANN range bytes overflow"))
        })?;
        self.shared.charge(Some(location), 0, bytes, false)?;
        let backing = self.shared.backing.clone();
        Ok(self
            .inner
            .get_ranges(location, ranges)
            .await?
            .into_iter()
            .map(|data| owned_bytes(data, backing.clone()))
            .collect())
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        if let Err(error) = self.shared.charge(None, 0, 0, true) {
            return futures::stream::once(async { Err(error) }).boxed();
        }
        let store = self.clone();
        locations
            .then(move |location| {
                let store = store.clone();
                async move {
                    let path = location?;
                    let _mutation = store.shared.mutation_gate.read().await;
                    store.shared.charge(Some(&path), 0, 0, true)?;
                    let one = futures::stream::once(async move { Ok(path) }).boxed();
                    let deleted = store.inner.delete_stream(one).try_next().await?;
                    deleted.ok_or_else(|| denied("ANN delete did not settle"))
                }
            })
            .boxed()
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        if let Err(error) = self.shared.charge(prefix, 0, 0, false) {
            return futures::stream::once(async { Err(error) }).boxed();
        }
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> object_store::Result<ListResult> {
        self.shared.charge(prefix, 0, 0, false)?;
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        let _mutation = self.shared.mutation_gate.read().await;
        validate_path(from, self.shared.limits.max_path_bytes)?;
        let source = self.inner.head(from).await?;
        self.shared.charge(Some(to), source.size, 0, true)?;
        self.inner.copy_opts(from, to, options).await
    }
}

struct AnnUpload {
    inner: Box<dyn MultipartUpload>,
    path: Path,
    shared: Shared,
    settled: bool,
}

impl fmt::Debug for AnnUpload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("AnnUpload").finish_non_exhaustive()
    }
}

impl Drop for AnnUpload {
    fn drop(&mut self) {
        if !self.settled
            && let Ok(mut counters) = self.shared.counters.lock()
        {
            counters.active_uploads = counters.active_uploads.saturating_sub(1);
        }
    }
}

#[async_trait]
impl MultipartUpload for AnnUpload {
    fn put_part(&mut self, data: PutPayload) -> UploadPart {
        if self.settled {
            return Box::pin(async { Err(denied("ANN multipart upload already settled")) });
        }
        let bytes = match u64::try_from(data.content_length()) {
            Ok(bytes) if bytes <= self.shared.limits.max_part_bytes => bytes,
            _ => return Box::pin(async { Err(denied("ANN multipart part exceeds limit")) }),
        };
        let part = self.inner.put_part(data);
        let shared = self.shared.clone();
        let path = self.path.clone();
        Box::pin(async move {
            let _mutation = shared.mutation_gate.read().await;
            shared.charge(Some(&path), bytes, 0, true)?;
            part.await
        })
    }

    async fn complete(&mut self) -> object_store::Result<PutResult> {
        if self.settled {
            return Err(denied("ANN multipart upload already settled"));
        }
        let _mutation = self.shared.mutation_gate.read().await;
        self.shared.charge(Some(&self.path), 0, 0, true)?;
        let result = self.inner.complete().await?;
        self.settled = true;
        let mut counters = self
            .shared
            .counters
            .lock()
            .map_err(|_error| denied("ANN counters poisoned"))?;
        counters.active_uploads = counters.active_uploads.saturating_sub(1);
        drop(counters);
        Ok(result)
    }

    async fn abort(&mut self) -> object_store::Result<()> {
        if self.settled {
            return Err(denied("ANN multipart upload already settled"));
        }
        let _mutation = self.shared.mutation_gate.read().await;
        self.shared.charge(Some(&self.path), 0, 0, true)?;
        self.inner.abort().await?;
        self.settled = true;
        let mut counters = self
            .shared
            .counters
            .lock()
            .map_err(|_error| denied("ANN counters poisoned"))?;
        counters.active_uploads = counters.active_uploads.saturating_sub(1);
        drop(counters);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits};

    struct MemoryCount(AtomicU64);

    impl MemoryLeaseOwner for MemoryCount {
        fn release(&self, bytes: u64) {
            let _previous = self.0.fetch_sub(bytes, Ordering::AcqRel);
        }
    }

    struct MemoryAdmission(Arc<MemoryCount>);

    impl MemoryScopePort for MemoryAdmission {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let _previous = self.0.0.fetch_add(bytes, Ordering::AcqRel);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    fn test_budget() -> Result<(WorkBudget, Arc<MemoryCount>), IndexError> {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1024,
                writes: 1024,
                bytes: 128 * MIB,
                mutations: 1024,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let counter = Arc::new(MemoryCount(AtomicU64::new(0)));
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(MemoryAdmission(
            counter.clone(),
        )))));
        Ok((budget, counter))
    }

    fn tiny_limits() -> AnnStoreLimits {
        AnnStoreLimits {
            max_write_bytes: 7,
            max_read_bytes: 16,
            max_ops: 20,
            max_files: 2,
            max_path_bytes: 16,
            max_part_bytes: 4,
        }
    }

    fn test_runtime() -> Result<tokio::runtime::Runtime, IndexError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| IndexError::Storage(format!("ANN test runtime: {error}")))
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "fixed quota and grant-lifetime assertions are the test oracle"
    )]
    fn quota_freeze_and_exported_bytes_keep_original_grant() -> Result<(), IndexError> {
        let runtime = test_runtime()?;
        crate::run_blocking(&runtime, async {
            let (mut budget, counter) = test_budget()?;
            let store = AnnMemoryStore::new(tiny_limits(), &mut budget)?;
            let _put = store
                .put(&Path::from("a"), PutPayload::from_static(b"abcd"))
                .await
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            assert!(
                store
                    .put(&Path::from("b"), PutPayload::from_static(b"efgh"))
                    .await
                    .is_err()
            );
            let _put = store
                .put(&Path::from("b"), PutPayload::from_static(b"ef"))
                .await
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            assert!(
                store
                    .put(&Path::from("c"), PutPayload::from_static(b"g"))
                    .await
                    .is_err()
            );
            assert!(
                store
                    .put(
                        &Path::from("abcdefghijklmnopq"),
                        PutPayload::from_static(b"g"),
                    )
                    .await
                    .is_err()
            );
            store.freeze().await?;
            assert!(
                store
                    .put(&Path::from("a"), PutPayload::from_static(b"x"))
                    .await
                    .is_err()
            );
            let files = store.snapshot().await?;
            let [first, second] = files.as_slice() else {
                return Err(IndexError::BackendViolation(
                    "expected two frozen files".into(),
                ));
            };
            assert_eq!(first.path, "a");
            assert_eq!(first.bytes.as_ref(), b"abcd");
            assert_eq!(second.path, "b");
            assert_eq!(second.bytes.as_ref(), b"ef");
            let retained = first.bytes.clone();
            drop(store);
            assert!(counter.0.load(Ordering::Acquire) > 0);
            drop(files);
            assert!(counter.0.load(Ordering::Acquire) > 0);
            drop(retained);
            assert_eq!(counter.0.load(Ordering::Acquire), 0);
            Ok(())
        })
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "fixed empty-store and cumulative-read assertions are the test oracle"
    )]
    fn empty_snapshot_and_cumulative_read_limit() -> Result<(), IndexError> {
        let runtime = test_runtime()?;
        crate::run_blocking(&runtime, async {
            let (mut budget, _counter) = test_budget()?;
            let store = AnnMemoryStore::new(tiny_limits(), &mut budget)?;
            store.freeze().await?;
            assert!(store.snapshot().await?.is_empty());

            let (mut budget, _counter) = test_budget()?;
            let store = AnnMemoryStore::new(
                AnnStoreLimits {
                    max_read_bytes: 5,
                    ..tiny_limits()
                },
                &mut budget,
            )?;
            let _put = store
                .put(&Path::from("a"), PutPayload::from_static(b"abcd"))
                .await
                .map_err(index_error)?;
            assert_eq!(
                store
                    .get(&Path::from("a"))
                    .await
                    .map_err(index_error)?
                    .bytes()
                    .await
                    .map_err(index_error)?
                    .as_ref(),
                b"abcd"
            );
            let head = GetOptions {
                head: true,
                ..GetOptions::default()
            };
            assert!(
                store
                    .get_opts(&Path::from("a"), head)
                    .await
                    .map_err(index_error)?
                    .bytes()
                    .await
                    .map_err(index_error)?
                    .is_empty()
            );
            assert!(store.get(&Path::from("a")).await.is_err());
            Ok(())
        })
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "fixed cancellation and multipart-settlement assertions are the test oracle"
    )]
    fn cancellation_and_unsettled_multipart_refuse_freeze() -> Result<(), IndexError> {
        let runtime = test_runtime()?;
        crate::run_blocking(&runtime, async {
            let (mut budget, _counter) = test_budget()?;
            let cancel = budget.request().cancel_handle();
            let store = AnnMemoryStore::new(tiny_limits(), &mut budget)?;
            let mut upload = store
                .put_multipart(&Path::from("a"))
                .await
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            assert!(store.freeze().await.is_err());
            let delayed_part = upload.put_part(PutPayload::from_static(b"x"));
            drop(upload);
            store.freeze().await?;
            assert!(delayed_part.await.is_err());
            cancel.cancel();
            assert!(store.get(&Path::from("a")).await.is_err());
            Ok(())
        })
    }
}
