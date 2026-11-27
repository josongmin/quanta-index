//! Product FD custody for reads of a sealed dataset through the memory URI.
//! Staging writes and deletes use the generation authority.

use std::fmt;
use std::fs::Metadata;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt as _, stream, stream::BoxStream};
use lance_io::object_store::providers::local::FileStoreProvider;
use lance_io::object_store::{
    ObjectStore as LanceObjectStore, ObjectStoreParams, ObjectStoreProvider,
};
use object_store::local::LocalFileSystem;
use object_store::path::Path;
use object_store::{
    Attributes, CopyMode, CopyOptions, GetOptions, GetRange, GetResult, GetResultPayload,
    ListResult, MultipartUpload, ObjectMeta, ObjectStore as ObjectStoreTrait, PutMode,
    PutMultipartOptions, PutOptions, PutPayload, PutResult, RenameOptions, RenameTargetMode,
    Result as StoreResult,
};
use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::{
    PublicationObjectLifetimePort, PublicationObjectReader,
};
use url::Url;

use crate::{SemanticGenerationFileIo, SemanticStagingTargetMode};

mod listing;
mod multipart;
mod sealed;
pub use sealed::AdmittedMemorySealedStore;
pub(super) use sealed::SealedDatasetProvider;

const READ_CHUNK_BYTES: u64 = 8 * 1024;

/// Object operations over one admitted staging dataset and its declared base.
#[derive(Clone)]
pub struct AdmittedMemoryWorkStore {
    generation: Arc<dyn SemanticGenerationFileIo>,
}

impl fmt::Debug for AdmittedMemoryWorkStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdmittedMemoryWorkStore")
            .finish_non_exhaustive()
    }
}

impl AdmittedMemoryWorkStore {
    #[must_use]
    pub fn new(generation: Arc<dyn SemanticGenerationFileIo>) -> Self {
        Self { generation }
    }
}

fn require_private_store_params(params: &ObjectStoreParams) -> lance::Result<()> {
    #[allow(deprecated)]
    if params.object_store.is_some() || params.object_store_wrapper.is_some() {
        return Err(lance::Error::invalid_input(
            "qualified file provider refuses custom inner store or wrapper",
        ));
    }
    Ok(())
}

pub(super) fn dataset_path_from_url(url: &Url) -> lance::Result<Path> {
    if url.scheme() != "memory"
        || url.has_host()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.path().starts_with('/')
    {
        return Err(lance::Error::invalid_input(
            "qualified dataset URI must name an absolute path",
        ));
    }
    Path::from_url_path(url.path()).map_err(|error| {
        lance::Error::invalid_input(format!("qualified dataset URI path is invalid: {error}"))
    })
}

#[derive(Debug)]
pub(super) struct WorkDatasetProvider(pub(super) AdmittedMemoryWorkStore);

#[async_trait]
impl ObjectStoreProvider for WorkDatasetProvider {
    async fn new_store(
        &self,
        base_path: Url,
        params: &ObjectStoreParams,
    ) -> lance::Result<LanceObjectStore> {
        require_private_store_params(params)?;
        let mut store = FileStoreProvider.new_store(base_path, params).await?;
        store.inner = Arc::new(self.0.clone());
        Ok(store)
    }

    fn extract_path(&self, url: &Url) -> lance::Result<Path> {
        dataset_path_from_url(url)
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
        FileStoreProvider.calculate_object_store_prefix(url, storage_options)
    }
}

impl fmt::Display for AdmittedMemoryWorkStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("admitted work dataset")
    }
}

pub(super) fn local_path(path: &Path) -> StoreResult<std::path::PathBuf> {
    // Stock LocalFileSystem owns URL encoding and its filename restrictions.
    // The public conversion avoids hand-decoding an encoded path into a
    // different physical inode.
    let physical = LocalFileSystem::new().path_to_filesystem(path)?;
    validate_local_path(path, physical)
}

fn validate_local_path(path: &Path, physical: PathBuf) -> StoreResult<PathBuf> {
    if Path::from_absolute_path(&physical)? != *path {
        return Err(object_store::Error::Generic {
            store: "admitted-file",
            source: Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "candidate delete path changed during filesystem conversion",
            )),
        });
    }
    Ok(physical)
}

fn file_error(path: &std::path::Path, error: io::Error) -> object_store::Error {
    if error.kind() == io::ErrorKind::NotFound {
        object_store::Error::NotFound {
            path: path.to_string_lossy().into_owned(),
            source: Box::new(error),
        }
    } else {
        object_store::Error::Generic {
            store: "admitted-file",
            source: Box::new(error),
        }
    }
}

fn core_file_error(path: &std::path::Path, error: CoreError) -> object_store::Error {
    let kind = match &error {
        CoreError::NotFound(_) => io::ErrorKind::NotFound,
        CoreError::StorageIo { kind, .. } => *kind,
        _ => io::ErrorKind::Other,
    };
    file_error(path, io::Error::new(kind, error))
}

fn staging_namespace_error(
    from: &std::path::Path,
    to: &std::path::Path,
    error: CoreError,
) -> object_store::Error {
    match error {
        error @ CoreError::StorageIo {
            kind: io::ErrorKind::AlreadyExists,
            ..
        } => object_store::Error::AlreadyExists {
            path: to.to_string_lossy().into_owned(),
            source: Box::new(error),
        },
        error => core_file_error(from, error),
    }
}

fn blocking_file_error(error: tokio::task::JoinError) -> object_store::Error {
    object_store::Error::Generic {
        store: "admitted-file",
        source: Box::new(io::Error::other(error)),
    }
}

fn local_put_result(metadata: &Metadata) -> PutResult {
    PutResult {
        e_tag: Some(local_etag(metadata, metadata.modified().ok())),
        version: None,
    }
}

fn local_etag(metadata: &Metadata, modified: Option<SystemTime>) -> String {
    local_file_etag(metadata.ino(), metadata.len(), modified)
}

fn local_file_etag(inode: u64, bytes: u64, modified: Option<SystemTime>) -> String {
    let microseconds = modified
        .and_then(|mtime| mtime.duration_since(SystemTime::UNIX_EPOCH).ok())
        .unwrap_or_default()
        .as_micros();
    format!("{inode:x}-{microseconds:x}-{bytes:x}")
}

fn settle_reader<T>(
    reader: Box<dyn PublicationObjectReader>,
    path: &std::path::Path,
    result: StoreResult<T>,
) -> StoreResult<T> {
    let retired = reader
        .retire()
        .map_err(|error| file_error(path, io::Error::other(error)));
    match (result, retired) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(retirement)) => Err(object_store::Error::Generic {
            store: "admitted-file",
            source: Box::new(io::Error::other(format!(
                "{primary}; dataset reader retirement failed: {retirement}"
            ))),
        }),
    }
}

fn read_stream(
    reader: Box<dyn PublicationObjectReader>,
    path: PathBuf,
    remaining: u64,
) -> BoxStream<'static, StoreResult<Bytes>> {
    stream::try_unfold(Some((reader, path, remaining)), |state| async move {
        let Some((mut reader, path, remaining)) = state else {
            return Ok(None);
        };
        tokio::task::spawn_blocking(move || {
            let width = remaining.min(READ_CHUNK_BYTES);
            let size = usize::try_from(width)
                .map_err(|error| file_error(&path, io::Error::other(error)))?;
            let mut bytes = vec![0; size];
            let read =
                Read::read_exact(&mut reader, &mut bytes).map_err(|error| file_error(&path, error));
            // An exhausted or failed read must witness retirement before it
            // reports a result. Dropping an unfinished stream lets the same
            // binder close the FD and retain any uncertain charge.
            let next = if remaining == width || read.is_err() {
                settle_reader(reader, &path, read)?;
                None
            } else {
                Some((reader, path, remaining - width))
            };
            Ok(Some((Bytes::from(bytes), next)))
        })
        .await
        .map_err(|error| object_store::Error::Generic {
            store: "admitted-file",
            source: Box::new(io::Error::other(error)),
        })?
    })
    .boxed()
}

fn read_dataset(
    location: Path,
    options: GetOptions,
    lifetime: &dyn PublicationObjectLifetimePort,
    committed_size: u64,
) -> StoreResult<GetResult> {
    let path = local_path(&location)?;
    let reader =
        crate::control_file::product::open_dataset_reader_with_control_lifetime(&path, lifetime)
            .map_err(|error| file_error(&path, error))?;
    read_opened_dataset(location, path, options, reader, Some(committed_size))
}

fn resolve_range(request: Option<&GetRange>, size: u64) -> StoreResult<Range<u64>> {
    request.map_or(Ok(0..size), |range| {
        range
            .as_range(size)
            .map_err(|source| object_store::Error::Generic {
                store: "LocalFileSystem",
                source: Box::new(source),
            })
    })
}

fn read_opened_dataset(
    location: Path,
    path: PathBuf,
    options: GetOptions,
    mut reader: Box<dyn PublicationObjectReader>,
    committed_size: Option<u64>,
) -> StoreResult<GetResult> {
    let headers = (|| {
        let metadata = reader
            .metadata()
            .map_err(|error| file_error(&path, io::Error::other(error)))?;
        if !metadata.is_file() {
            return Err(file_error(
                &path,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "candidate dataset object is not a file",
                ),
            ));
        }
        if committed_size.is_some_and(|size| metadata.len() != size) {
            return Err(file_error(
                &path,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "sealed dataset object width changed",
                ),
            ));
        }
        let modified = metadata
            .modified()
            .map_err(|error| file_error(&path, error))?;
        let meta = ObjectMeta {
            location,
            last_modified: modified.into(),
            size: metadata.len(),
            e_tag: Some(local_etag(&metadata, Some(modified))),
            version: None,
        };
        options.check_preconditions(&meta)?;
        let range = resolve_range(options.range.as_ref(), meta.size)?;
        if !options.head && !range.is_empty() {
            let _position = Seek::seek(&mut reader, SeekFrom::Start(range.start))
                .map_err(|error| file_error(&path, error))?;
        }
        Ok((meta, range))
    })();
    let (meta, range) = match headers {
        Ok(headers) => headers,
        Err(error) => return settle_reader(reader, &path, Err(error)),
    };
    let stream = if options.head || range.is_empty() {
        settle_reader(reader, &path, Ok(()))?;
        stream::empty().boxed()
    } else {
        read_stream(reader, path, range.end - range.start)
    };
    Ok(GetResult {
        payload: GetResultPayload::Stream(stream),
        meta,
        range,
        attributes: Attributes::default(),
    })
}

fn read_work_range(
    generation: &dyn SemanticGenerationFileIo,
    path: &std::path::Path,
    range: Range<u64>,
) -> StoreResult<Bytes> {
    let mut reader = generation
        .open_work_dataset_reader(path)
        .map_err(|error| core_file_error(path, error))?;
    let result = (|| {
        let metadata = reader
            .metadata()
            .map_err(|error| core_file_error(path, error))?;
        let range = resolve_range(Some(&GetRange::Bounded(range)), metadata.len())?;
        let size = usize::try_from(range.end - range.start)
            .map_err(|error| file_error(path, io::Error::other(error)))?;
        if size == 0 {
            return Ok(Bytes::new());
        }
        let _position = Seek::seek(&mut reader, SeekFrom::Start(range.start))
            .map_err(|error| file_error(path, error))?;
        let mut data = Vec::new();
        data.try_reserve_exact(size)
            .map_err(|error| file_error(path, io::Error::other(error)))?;
        data.resize(size, 0);
        Read::read_exact(&mut reader, &mut data).map_err(|error| file_error(path, error))?;
        Ok(Bytes::from(data))
    })();
    settle_reader(reader, path, result)
}

#[async_trait]
#[deny(clippy::missing_trait_methods)]
impl ObjectStoreTrait for AdmittedMemoryWorkStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> StoreResult<PutResult> {
        let generation = Arc::clone(&self.generation);
        let mode = match opts.mode {
            PutMode::Create => SemanticStagingTargetMode::Create,
            PutMode::Overwrite => SemanticStagingTargetMode::Overwrite,
            PutMode::Update(_) => {
                return Err(object_store::Error::NotImplemented {
                    operation: "`put_opts` with mode `PutMode::Update`".into(),
                    implementer: LocalFileSystem::new().to_string(),
                });
            }
        };
        if !opts.attributes.is_empty() {
            return Err(object_store::Error::NotImplemented {
                operation: "`put_opts` with `opts.attributes` specified".into(),
                implementer: LocalFileSystem::new().to_string(),
            });
        }

        let path = local_path(location)?;
        tokio::task::spawn_blocking(move || {
            let chunks: Vec<_> = payload.iter().map(Bytes::as_ref).collect();
            let metadata = generation
                .put_staging_file(&path, mode, &chunks)
                .map_err(|error| core_file_error(&path, error))?
                .ok_or_else(|| object_store::Error::AlreadyExists {
                    path: path.to_string_lossy().into_owned(),
                    source: Box::new(io::Error::from(io::ErrorKind::AlreadyExists)),
                })?;
            Ok(local_put_result(&metadata))
        })
        .await
        .map_err(blocking_file_error)?
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> StoreResult<Box<dyn MultipartUpload>> {
        let generation = Arc::clone(&self.generation);
        if !opts.attributes.is_empty() {
            return Err(object_store::Error::NotImplemented {
                operation: "`put_multipart_opts` with `opts.attributes` specified".into(),
                implementer: LocalFileSystem::new().to_string(),
            });
        }
        let path = local_path(location)?;
        let upload = tokio::task::spawn_blocking(move || {
            let upload = generation
                .begin_staging_upload(&path)
                .map_err(|error| core_file_error(&path, error))?;
            Ok::<_, object_store::Error>(multipart::StagingUpload::new(path, upload))
        })
        .await
        .map_err(blocking_file_error)??;
        Ok(Box::new(upload))
    }

    async fn get_opts(&self, location: &Path, opts: GetOptions) -> StoreResult<GetResult> {
        let path = local_path(location)?;
        let location = location.clone();
        let generation = Arc::clone(&self.generation);
        tokio::task::spawn_blocking(move || {
            let reader = generation
                .open_work_dataset_reader(&path)
                .map_err(|error| core_file_error(&path, error))?;
            read_opened_dataset(location, path, opts, reader, None)
        })
        .await
        .map_err(blocking_file_error)?
    }

    async fn get_ranges(&self, location: &Path, ranges: &[Range<u64>]) -> StoreResult<Vec<Bytes>> {
        if ranges.is_empty() {
            return Ok(Vec::new());
        }
        let path = local_path(location)?;
        let ranges = ranges.to_vec();
        let generation = Arc::clone(&self.generation);
        // Keep admitted range order and per-range FD settlement inside
        // one blocking task. Large ranges need no stream-task round trips.
        tokio::task::spawn_blocking(move || {
            ranges
                .into_iter()
                .map(|range| read_work_range(generation.as_ref(), &path, range))
                .collect()
        })
        .await
        .map_err(blocking_file_error)?
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, StoreResult<Path>>,
    ) -> BoxStream<'static, StoreResult<Path>> {
        let generation = Arc::clone(&self.generation);
        locations
            .then(move |item| {
                let generation = Arc::clone(&generation);
                async move {
                    let path = item?;
                    let local = local_path(&path)?;
                    tokio::task::spawn_blocking(move || {
                        generation
                            .remove_staging_file_and_sync(&local)
                            .map_err(|error| core_file_error(&local, error))
                    })
                    .await
                    .map_err(blocking_file_error)??;
                    Ok(path)
                }
            })
            .boxed()
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, StoreResult<ObjectMeta>> {
        listing::list(Arc::clone(&self.generation), prefix, None)
    }

    fn list_with_offset(
        &self,
        prefix: Option<&Path>,
        offset: &Path,
    ) -> BoxStream<'static, StoreResult<ObjectMeta>> {
        listing::list(Arc::clone(&self.generation), prefix, Some(offset))
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> StoreResult<ListResult> {
        listing::list_with_delimiter(Arc::clone(&self.generation), prefix).await
    }

    async fn copy_opts(&self, from: &Path, to: &Path, opts: CopyOptions) -> StoreResult<()> {
        let generation = Arc::clone(&self.generation);
        let mode = match opts.mode {
            CopyMode::Create => SemanticStagingTargetMode::Create,
            CopyMode::Overwrite => SemanticStagingTargetMode::Overwrite,
        };
        let from = local_path(from)?;
        let to = local_path(to)?;
        tokio::task::spawn_blocking(move || {
            generation
                .copy_staging_file(&from, &to, mode)
                .map_err(|error| staging_namespace_error(&from, &to, error))
        })
        .await
        .map_err(blocking_file_error)?
    }

    async fn rename_opts(&self, from: &Path, to: &Path, opts: RenameOptions) -> StoreResult<()> {
        let generation = Arc::clone(&self.generation);
        let mode = match opts.target_mode {
            RenameTargetMode::Create => SemanticStagingTargetMode::Create,
            RenameTargetMode::Overwrite => SemanticStagingTargetMode::Overwrite,
        };
        let from = local_path(from)?;
        let to = local_path(to)?;
        tokio::task::spawn_blocking(move || {
            generation
                .rename_staging_file(&from, &to, mode)
                .map_err(|error| staging_namespace_error(&from, &to, error))
        })
        .await
        .map_err(blocking_file_error)?
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Arc, CoreError, LanceObjectStore, ObjectStoreParams, core_file_error, io,
        require_private_store_params,
    };
    use lance_io::object_store::{ChainedWrappingObjectStore, uri_to_url};

    #[test]
    #[expect(
        deprecated,
        reason = "the provider must reject the pinned dependency's explicit store override"
    )]
    fn private_provider_refuses_store_and_wrapper_overrides() -> lance::Result<()> {
        require_private_store_params(&ObjectStoreParams::default())?;
        let override_store = ObjectStoreParams {
            object_store: Some((LanceObjectStore::memory().inner, uri_to_url("memory:///")?)),
            ..ObjectStoreParams::default()
        };
        let wrapper = ObjectStoreParams {
            object_store_wrapper: Some(Arc::new(ChainedWrappingObjectStore::new(Vec::new()))),
            ..ObjectStoreParams::default()
        };
        for params in [override_store, wrapper] {
            assert!(matches!(
                require_private_store_params(&params),
                Err(lance::Error::InvalidInput { .. })
            ));
        }
        Ok(())
    }

    #[test]
    fn work_store_does_not_use_error_text_to_infer_absence() {
        let path = std::path::Path::new("/sealed/receipt");
        assert!(matches!(
            core_file_error(path, CoreError::NotFound("missing receipt".into())),
            object_store::Error::NotFound { source, .. }
                if source.downcast_ref::<io::Error>().is_some_and(|error| {
                    error.kind() == io::ErrorKind::NotFound
                        && error.get_ref().is_some_and(|cause| {
                            matches!(cause.downcast_ref::<CoreError>(), Some(CoreError::NotFound(_)))
                        })
                })
        ));
        assert!(matches!(
            core_file_error(
                path,
                CoreError::Storage("open anchored file: No such file or directory".into()),
            ),
            object_store::Error::Generic { .. }
        ));
    }
}
