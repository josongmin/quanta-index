//! One immutable generation's read-only object-store provider.

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::ops::Range;
use std::path::Path as FsPath;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt as _, stream, stream::BoxStream};
use lance_io::object_store::providers::local::FileStoreProvider;
use lance_io::object_store::{ObjectStore, ObjectStoreParams, ObjectStoreProvider};
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta,
    ObjectStore as ObjectStoreTrait, ObjectStoreExt as _, PutMultipartOptions, PutOptions,
    PutPayload, PutResult, RenameOptions, Result as StoreResult,
};
use quanta_index_core::publication_admission::PublicationObjectLifetimePort;
use quanta_index_core::{CoreError, SealedArtifactCommitmentV1};
use url::Url;

use super::{dataset_path_from_url, file_error, read_dataset, require_private_store_params};

struct SealedNamespace {
    root: Path,
    /// Store relative names so every entry does not repeat the StateRoot.
    objects: Vec<(Path, u64)>,
}

impl SealedNamespace {
    fn location(&self, relative: &Path) -> Path {
        self.root.parts().chain(relative.parts()).collect()
    }
}

#[derive(Clone)]
pub struct AdmittedMemorySealedStore {
    namespace: Arc<SealedNamespace>,
    lifetime: Arc<dyn PublicationObjectLifetimePort>,
}

impl AdmittedMemorySealedStore {
    /// Use the authenticated file commitments of one sealed dataset.
    pub fn new(
        dataset_dir: &FsPath,
        artifacts: Vec<SealedArtifactCommitmentV1>,
        lifetime: Arc<dyn PublicationObjectLifetimePort>,
    ) -> Result<Self, CoreError> {
        let object_path = |path: &FsPath| {
            Path::from_absolute_path(path).map_err(|error| {
                CoreError::InvalidContract(format!("sealed dataset path: {error}"))
            })
        };
        let root = object_path(dataset_dir)?;
        let mut objects = Vec::with_capacity(artifacts.len());
        for artifact in artifacts {
            let relative = FsPath::new(&artifact.name)
                .strip_prefix(crate::legacy::layout::DATASET_DIR_NAME)
                .map_err(|error| {
                    CoreError::InvalidContract(format!("sealed artifact path: {error}"))
                })?;
            let location = object_path(&dataset_dir.join(relative))?;
            let parts = location
                .prefix_match(&root)
                .filter(|_| location != root)
                .ok_or_else(|| {
                    CoreError::InvalidContract("sealed artifact is outside its dataset".into())
                })?;
            objects.push((parts.collect::<Path>(), artifact.bytes));
        }
        objects.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        if objects.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(CoreError::InvalidContract(
                "sealed artifact paths map to the same object".into(),
            ));
        }
        Ok(Self {
            namespace: Arc::new(SealedNamespace { root, objects }),
            lifetime,
        })
    }

    fn list_prefix(&self, prefix: Option<&Path>) -> StoreResult<Path> {
        prefix
            .and_then(|prefix| prefix.prefix_match(&self.namespace.root))
            .map(|parts| parts.collect::<Path>())
            .ok_or_else(|| {
                file_error(
                    FsPath::new(""),
                    io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "list prefix is outside the sealed dataset",
                    ),
                )
            })
    }

    fn list_objects(
        &self,
        prefix: Option<&Path>,
        offset: Option<&Path>,
    ) -> BoxStream<'static, StoreResult<ObjectMeta>> {
        let prefix = match self.list_prefix(prefix) {
            Ok(prefix) => prefix,
            Err(error) => return stream::once(async move { Err(error) }).boxed(),
        };
        let offset = offset.cloned();
        let namespace = Arc::clone(&self.namespace);
        let store = self.clone();
        stream::iter(0..namespace.objects.len())
            .filter(move |index| {
                let relative = &namespace.objects[*index].0;
                futures::future::ready(
                    relative != &prefix
                        && relative.prefix_matches(&prefix)
                        && offset
                            .as_ref()
                            .is_none_or(|offset| namespace.location(relative) > *offset),
                )
            })
            .then(move |index| {
                let store = store.clone();
                async move {
                    store
                        .head(&store.namespace.location(&store.namespace.objects[index].0))
                        .await
                }
            })
            .boxed()
    }
}

impl fmt::Debug for AdmittedMemorySealedStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("AdmittedMemorySealedStore")
            .field(&self.namespace.root)
            .finish()
    }
}

impl fmt::Display for AdmittedMemorySealedStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "sealed dataset {}", self.namespace.root)
    }
}

#[derive(Debug)]
pub(in crate::qualified_fs) struct SealedDatasetProvider(
    pub(in crate::qualified_fs) AdmittedMemorySealedStore,
);

#[async_trait]
impl ObjectStoreProvider for SealedDatasetProvider {
    async fn new_store(&self, base: Url, params: &ObjectStoreParams) -> lance::Result<ObjectStore> {
        require_private_store_params(params)?;
        let _path = self.extract_path(&base)?;
        let mut store = FileStoreProvider.new_store(base, params).await?;
        store.inner = Arc::new(self.0.clone());
        Ok(store)
    }

    fn extract_path(&self, url: &Url) -> lance::Result<Path> {
        let path = dataset_path_from_url(url)?;
        if !path.prefix_matches(&self.0.namespace.root) {
            return Err(lance::Error::invalid_input(
                "store URL escapes the sealed dataset",
            ));
        }
        Ok(path)
    }

    #[expect(
        clippy::disallowed_types,
        reason = "Lance provider ABI requires HashMap"
    )]
    fn calculate_object_store_prefix(
        &self,
        url: &Url,
        options: Option<&std::collections::HashMap<String, String>>,
    ) -> lance::Result<String> {
        let _path = self.extract_path(url)?;
        FileStoreProvider.calculate_object_store_prefix(url, options)
    }
}

fn read_only_error() -> object_store::Error {
    file_error(
        FsPath::new(""),
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "sealed dataset is read-only",
        ),
    )
}

#[async_trait]
#[deny(clippy::missing_trait_methods)]
impl ObjectStoreTrait for AdmittedMemorySealedStore {
    async fn get_opts(&self, location: &Path, options: GetOptions) -> StoreResult<GetResult> {
        let relative = location
            .prefix_match(&self.namespace.root)
            .map(|parts| parts.collect::<Path>());
        let position = relative
            .and_then(|relative| {
                self.namespace
                    .objects
                    .binary_search_by(|(name, _)| name.cmp(&relative))
                    .ok()
            })
            .ok_or_else(|| object_store::Error::NotFound {
                path: location.to_string(),
                source: Box::new(io::Error::new(
                    io::ErrorKind::NotFound,
                    "object is absent from the sealed dataset",
                )),
            })?;
        let width = self.namespace.objects[position].1;
        let location = location.clone();
        let lifetime = Arc::clone(&self.lifetime);
        tokio::task::spawn_blocking(move || {
            read_dataset(location, options, lifetime.as_ref(), width)
        })
        .await
        .map_err(|error| file_error(FsPath::new(""), io::Error::other(error)))?
    }

    async fn get_ranges(&self, location: &Path, ranges: &[Range<u64>]) -> StoreResult<Vec<Bytes>> {
        let mut result = Vec::with_capacity(ranges.len());
        for range in ranges {
            result.push(self.get_range(location, range.clone()).await?);
        }
        Ok(result)
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, StoreResult<ObjectMeta>> {
        self.list_objects(prefix, None)
    }

    fn list_with_offset(
        &self,
        prefix: Option<&Path>,
        offset: &Path,
    ) -> BoxStream<'static, StoreResult<ObjectMeta>> {
        self.list_objects(prefix, Some(offset))
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> StoreResult<ListResult> {
        let prefix = self.list_prefix(prefix)?;
        let mut common_prefixes = BTreeSet::new();
        let mut objects = Vec::new();
        for (relative, _) in &self.namespace.objects {
            let Some(mut tail) = relative.prefix_match(&prefix) else {
                continue;
            };
            let Some(child) = tail.next() else {
                continue;
            };
            if tail.next().is_some() {
                let _inserted =
                    common_prefixes.insert(self.namespace.location(&prefix).join(child));
            } else {
                objects.push(self.head(&self.namespace.location(relative)).await?);
            }
        }
        Ok(ListResult {
            common_prefixes: common_prefixes.into_iter().collect(),
            objects,
        })
    }

    async fn put_opts(&self, _: &Path, _: PutPayload, _: PutOptions) -> StoreResult<PutResult> {
        Err(read_only_error())
    }

    async fn put_multipart_opts(
        &self,
        _: &Path,
        _: PutMultipartOptions,
    ) -> StoreResult<Box<dyn MultipartUpload>> {
        Err(read_only_error())
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, StoreResult<Path>>,
    ) -> BoxStream<'static, StoreResult<Path>> {
        locations
            .map(|path| path.and_then(|_| Err(read_only_error())))
            .boxed()
    }

    async fn copy_opts(&self, _: &Path, _: &Path, _: CopyOptions) -> StoreResult<()> {
        Err(read_only_error())
    }

    async fn rename_opts(&self, _: &Path, _: &Path, _: RenameOptions) -> StoreResult<()> {
        Err(read_only_error())
    }
}
