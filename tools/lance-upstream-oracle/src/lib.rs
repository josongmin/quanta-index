//! Official Lance provider ABI probe for the bounded Native ANN store.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use lance::session::Session;
    use lance_io::object_store::{
        ObjectStore as LanceObjectStore, ObjectStoreParams, ObjectStoreProvider,
        ObjectStoreRegistry,
        providers::{local::FileStoreProvider, memory::MemoryStoreProvider},
    };
    use lancedb::arrow::arrow_schema::{DataType, Field, Schema};
    use object_store::{ObjectStoreExt as _, PutPayload, memory::InMemory, path::Path};
    use url::Url;

    #[tokio::test]
    async fn lancedb_refuses_to_mutate_a_checked_out_older_version() {
        use futures_util::TryStreamExt as _;
        use lance::deps::arrow_array::{Array as _, RecordBatch, StringArray};
        use lancedb::query::ExecutableQuery as _;

        let root = tempfile::tempdir().unwrap();
        let uri = root.path().to_string_lossy().into_owned();
        let connection = lancedb::connect(&uri).execute().await.unwrap();
        let schema = Arc::new(Schema::new(vec![Field::new(
            "row_id",
            DataType::Utf8,
            false,
        )]));
        let batch = |ids: Vec<&str>| {
            RecordBatch::try_new(schema.clone(), vec![Arc::new(StringArray::from(ids))]).unwrap()
        };
        let table = connection
            .create_table("probe", batch(vec!["base", "victim"]))
            .execute()
            .await
            .unwrap();
        let base_version = table.version().await.unwrap();
        table.delete("row_id = 'victim'").await.unwrap();
        let latest_version = table.version().await.unwrap();
        table.checkout(base_version).await.unwrap();
        let batches: Vec<RecordBatch> = table
            .query()
            .execute()
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        let mut checked_out_ids = Vec::new();
        for batch in &batches {
            let ids = batch
                .column_by_name("row_id")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            for row in 0..ids.len() {
                checked_out_ids.push(ids.value(row).to_string());
            }
        }
        checked_out_ids.sort();
        assert_eq!(checked_out_ids, vec!["base", "victim"]);
        let branch_attempt = table.add(batch(vec!["branch"])).execute().await;
        println!(
            "G0S-EVIDENCE old_version_branch base_version={base_version} latest_version={latest_version} checked_out_row_count={} branch_write_rejected={}",
            checked_out_ids.len(),
            branch_attempt.is_err()
        );
        assert!(
            branch_attempt.is_err(),
            "LanceDB Table wrote from an old checkout"
        );
    }

    #[derive(Debug)]
    struct InjectedMemoryProvider {
        inner: Arc<InMemory>,
        delegate: MemoryStoreProvider,
    }

    #[derive(Debug)]
    struct InjectedLocalObjectProvider {
        inner: Arc<InMemory>,
    }

    #[derive(Debug)]
    struct ForcedLocalReaderProvider;

    #[async_trait]
    impl ObjectStoreProvider for ForcedLocalReaderProvider {
        async fn new_store(
            &self,
            base_path: Url,
            params: &ObjectStoreParams,
        ) -> lance::Result<LanceObjectStore> {
            let mut params = params.clone();
            // Keep the footer in the last block, while the fixture's index
            // files stay large enough to use LocalReader instead of SmallReader.
            params.block_size = Some(512);
            FileStoreProvider.new_store(base_path, &params).await
        }

        fn extract_path(&self, url: &Url) -> lance::Result<Path> {
            FileStoreProvider.extract_path(url)
        }

        fn calculate_object_store_prefix(
            &self,
            url: &Url,
            storage_options: Option<&std::collections::HashMap<String, String>>,
        ) -> lance::Result<String> {
            FileStoreProvider.calculate_object_store_prefix(url, storage_options)
        }
    }

    #[async_trait]
    impl ObjectStoreProvider for InjectedLocalObjectProvider {
        async fn new_store(
            &self,
            base_path: Url,
            params: &ObjectStoreParams,
        ) -> lance::Result<LanceObjectStore> {
            let mut store = FileStoreProvider.new_store(base_path, params).await?;
            store.inner = self.inner.clone();
            Ok(store)
        }

        fn extract_path(&self, url: &Url) -> lance::Result<Path> {
            FileStoreProvider.extract_path(url)
        }

        fn calculate_object_store_prefix(
            &self,
            url: &Url,
            storage_options: Option<&std::collections::HashMap<String, String>>,
        ) -> lance::Result<String> {
            FileStoreProvider.calculate_object_store_prefix(url, storage_options)
        }
    }

    #[async_trait]
    impl ObjectStoreProvider for InjectedMemoryProvider {
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

        fn calculate_object_store_prefix(
            &self,
            url: &Url,
            storage_options: Option<&std::collections::HashMap<String, String>>,
        ) -> lance::Result<String> {
            self.delegate
                .calculate_object_store_prefix(url, storage_options)
        }
    }

    fn memory_session(inner: Arc<InMemory>) -> Arc<Session> {
        let registry = Arc::new(ObjectStoreRegistry::default());
        registry.insert(
            "memory",
            Arc::new(InjectedMemoryProvider {
                inner,
                delegate: MemoryStoreProvider,
            }),
        );
        Arc::new(Session::new(0, 0, registry))
    }

    #[tokio::test]
    async fn official_memory_provider_creates_reopens_and_drops_dataset() {
        let uri = "memory://persisted-ann/region";
        let inner = Arc::new(InMemory::new());
        let writer = lancedb::connect(uri)
            .session(memory_session(inner.clone()))
            .execute()
            .await
            .unwrap();
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let table = writer
            .create_empty_table("selected", schema.clone())
            .execute()
            .await
            .unwrap();
        assert_eq!(table.schema().await.unwrap(), schema);
        drop(table);
        drop(writer);

        let unrelated = lancedb::connect(uri)
            .session(memory_session(Arc::new(InMemory::new())))
            .execute()
            .await
            .unwrap();
        assert!(unrelated.open_table("selected").execute().await.is_err());
        drop(unrelated);

        let reader = lancedb::connect(uri)
            .session(memory_session(inner.clone()))
            .execute()
            .await
            .unwrap();
        let reopened = reader.open_table("selected").execute().await.unwrap();
        assert_eq!(reopened.schema().await.unwrap(), schema);
        assert_eq!(reopened.count_rows(None).await.unwrap(), 0);
        drop(reopened);
        reader.drop_table("selected", &[]).await.unwrap();
        assert!(reader.open_table("selected").execute().await.is_err());
    }

    #[tokio::test]
    async fn official_session_uses_injected_store_without_cross_session_leak() {
        let uri = Url::parse("memory://native-ann/region").unwrap();
        let selected = Path::from("native-ann/region/selected");
        let first_inner = Arc::new(InMemory::new());
        let second_inner = Arc::new(InMemory::new());
        let sessions = [first_inner.clone(), second_inner.clone()].map(|inner| {
            let registry = Arc::new(ObjectStoreRegistry::default());
            registry.insert(
                "memory",
                Arc::new(InjectedMemoryProvider {
                    inner,
                    delegate: MemoryStoreProvider,
                }),
            );
            Arc::new(Session::new(0, 0, registry))
        });
        let _first_connection = lancedb::connect("memory://native-ann/region")
            .session(Arc::clone(&sessions[0]))
            .execute()
            .await
            .unwrap();
        let first = sessions[0]
            .store_registry()
            .get_store(uri.clone(), &ObjectStoreParams::default())
            .await
            .unwrap();
        let second = sessions[1]
            .store_registry()
            .get_store(uri, &ObjectStoreParams::default())
            .await
            .unwrap();

        first
            .inner
            .put(&selected, PutPayload::from_static(b"sealed"))
            .await
            .unwrap();
        assert_eq!(
            first_inner
                .get(&selected)
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
                .as_ref(),
            b"sealed".as_slice()
        );
        assert!(second.inner.get(&selected).await.is_err());
        assert!(second_inner.get(&selected).await.is_err());
    }

    #[tokio::test]
    #[allow(deprecated)]
    async fn official_explicit_inner_store_bypasses_registered_provider() {
        let registry = Arc::new(ObjectStoreRegistry::default());
        let admitted = Arc::new(InMemory::new());
        let explicit = Arc::new(InMemory::new());
        registry.insert(
            "memory",
            Arc::new(InjectedMemoryProvider {
                inner: admitted.clone(),
                delegate: MemoryStoreProvider,
            }),
        );
        let uri = "memory://native-ann/region";
        let params = ObjectStoreParams {
            object_store: Some((explicit.clone(), Url::parse(uri).unwrap())),
            ..Default::default()
        };
        let (store, path) = LanceObjectStore::from_uri_and_params(registry, uri, &params)
            .await
            .unwrap();
        store
            .inner
            .put(&path, PutPayload::from_static(b"bypass"))
            .await
            .unwrap();
        assert!(admitted.get(&path).await.is_err());
        assert_eq!(
            explicit
                .get(&path)
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
                .as_ref(),
            b"bypass".as_slice()
        );
    }

    #[tokio::test]
    async fn official_file_object_store_writer_uses_injected_inner() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("selected");
        let uri = format!("file-object-store://{}", target.display());
        let registry = Arc::new(ObjectStoreRegistry::default());
        let inner = Arc::new(InMemory::new());
        registry.insert(
            "file-object-store",
            Arc::new(InjectedLocalObjectProvider {
                inner: inner.clone(),
            }),
        );
        let (store, path) =
            LanceObjectStore::from_uri_and_params(registry, &uri, &ObjectStoreParams::default())
                .await
                .unwrap();
        store.put(&path, b"sealed").await.unwrap();
        assert_eq!(
            inner
                .get(&path)
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
                .as_ref(),
            b"sealed".as_slice()
        );
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn official_file_object_store_directory_removal_bypasses_inner() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("victim");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("sentinel"), b"outside-inner").unwrap();
        let uri = format!("file-object-store://{}", target.display());
        let registry = Arc::new(ObjectStoreRegistry::default());
        let inner = Arc::new(InMemory::new());
        registry.insert(
            "file-object-store",
            Arc::new(InjectedLocalObjectProvider { inner }),
        );
        let (store, path) =
            LanceObjectStore::from_uri_and_params(registry, &uri, &ObjectStoreParams::default())
                .await
                .unwrap();

        store.remove_dir_all(path).await.unwrap();
        assert!(!target.exists());
    }

    /// Count only descriptors for this fixture's index files, excluding
    /// unrelated Tokio/test-runner descriptors from the oracle.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn index_fds(dataset: &std::path::Path) -> usize {
        #[cfg(target_os = "macos")]
        use std::path::PathBuf;

        let index_dir = dataset.canonicalize().unwrap().join("_indices");
        #[cfg(target_os = "macos")]
        let descriptor_dir = "/dev/fd";
        #[cfg(target_os = "linux")]
        let descriptor_dir = "/proc/self/fd";
        std::fs::read_dir(descriptor_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                #[cfg(target_os = "linux")]
                {
                    std::fs::read_link(entry.path()).ok()
                }
                #[cfg(target_os = "macos")]
                {
                    use std::ffi::{CStr, OsStr};
                    use std::os::unix::ffi::OsStrExt as _;

                    let descriptor = entry.file_name().to_string_lossy().parse::<i32>().ok()?;
                    let mut path = [0_i8; libc::PATH_MAX as usize];
                    // F_GETPATH reads this process's existing FD target only.
                    let status =
                        unsafe { libc::fcntl(descriptor, libc::F_GETPATH, path.as_mut_ptr()) };
                    if status < 0 {
                        return None;
                    }
                    let bytes = unsafe { CStr::from_ptr(path.as_ptr()) }.to_bytes();
                    Some(PathBuf::from(OsStr::from_bytes(bytes)))
                }
            })
            .filter(|path| path.starts_with(&index_dir))
            .count()
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn official_ivf_local_index_fds_return_after_session_drop() {
        use futures_util::StreamExt as _;
        use lance::dataset::DEFAULT_INDEX_CACHE_SIZE;
        use lance::deps::arrow_array::{
            ArrayRef, FixedSizeListArray, Float32Array, Int64Array, RecordBatch,
        };
        use lancedb::index::{Index, vector::IvfHnswSqIndexBuilder};
        use lancedb::query::{ExecutableQuery as _, QueryBase as _};

        let directory = tempfile::tempdir().unwrap();
        let uri = directory.path().to_str().unwrap();
        let dataset = directory.path().join("selected.lance");
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), 8),
                false,
            ),
        ]));
        let values = (0..256)
            .flat_map(|row| {
                let x = row as f32 / 256.0;
                [x, x * x, x.sin(), x.cos(), 1.0 - x, x * 0.5, x * 0.25, 0.5]
            })
            .collect::<Vec<_>>();
        let vectors = FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, true)),
            8,
            Arc::new(Float32Array::from(values)),
            None,
        )
        .unwrap();
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from_iter_values(0..256)) as ArrayRef,
                Arc::new(vectors) as ArrayRef,
            ],
        )
        .unwrap();

        let writer_session = Arc::new(Session::new(
            DEFAULT_INDEX_CACHE_SIZE,
            0,
            Arc::new(ObjectStoreRegistry::default()),
        ));
        let writer = lancedb::connect(uri)
            .session(Arc::clone(&writer_session))
            .execute()
            .await
            .unwrap();
        let table = writer
            .create_table("selected", batch)
            .execute()
            .await
            .unwrap();
        table
            .create_index(
                &["vector"],
                Index::IvfHnswSq(
                    IvfHnswSqIndexBuilder::default()
                        .num_partitions(1)
                        .sample_rate(256)
                        .max_iterations(8)
                        .num_edges(8)
                        .ef_construction(32),
                ),
            )
            .name("ann".to_owned())
            .execute()
            .await
            .unwrap();
        drop(table);
        drop(writer);
        drop(writer_session);
        let baseline = index_fds(&dataset);
        assert_eq!(
            baseline, 0,
            "writer retained index FDs with metadata disabled"
        );

        let query = |session| async move {
            let reader = lancedb::connect(uri)
                .session(session)
                .execute()
                .await
                .unwrap();
            let table = reader.open_table("selected").execute().await.unwrap();
            let mut stream = table
                .query()
                .nearest_to(vec![0.1_f32, 0.01, 0.1, 1.0, 0.9, 0.05, 0.025, 0.5])
                .unwrap()
                .limit(5)
                .execute()
                .await
                .unwrap();
            let mut hits = Vec::new();
            while let Some(batch) = stream.next().await {
                let batch = batch.unwrap();
                let ids = batch
                    .column_by_name("id")
                    .unwrap()
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap();
                let distances = batch
                    .column_by_name("_distance")
                    .unwrap()
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .unwrap();
                for row in 0..batch.num_rows() {
                    hits.push((ids.value(row), distances.value(row).to_bits()));
                }
            }
            assert_eq!(hits.len(), 5);
            hits
        };

        // Disable the separate index cache: only the metadata cache can hold
        // the CachedIndexReaders under test.
        let registry = Arc::new(ObjectStoreRegistry::default());
        registry.insert("file", Arc::new(ForcedLocalReaderProvider));
        let reader_session = Arc::new(Session::new(0, 64 * 1024 * 1024, registry));
        let expected = query(Arc::clone(&reader_session)).await;

        let held = index_fds(&dataset);
        assert!(
            held > baseline,
            "stock IVF query did not retain index FDs: baseline={baseline} held={held}"
        );
        assert_eq!(reader_session.index_cache_stats().await.num_entries, 0);
        assert!(reader_session.metadata_cache_stats().await.num_entries > 0);
        reader_session.file_metadata_cache().clear().await;
        let after_clear = index_fds(&dataset);
        drop(reader_session);
        let after_drop = index_fds(&dataset);
        println!(
            "IVF_INDEX_FDS baseline={baseline} cached={held} after_clear={after_clear} after_drop={after_drop}"
        );
        assert_eq!(
            after_drop, baseline,
            "explicit metadata clear and session drop retained an index FD"
        );

        let registry = Arc::new(ObjectStoreRegistry::default());
        registry.insert("file", Arc::new(ForcedLocalReaderProvider));
        let scoped_session = Arc::new(Session::new(DEFAULT_INDEX_CACHE_SIZE, 0, registry));
        assert_eq!(query(Arc::clone(&scoped_session)).await, expected);
        assert_eq!(scoped_session.metadata_cache_stats().await.num_entries, 0);
        drop(scoped_session);
        assert_eq!(
            index_fds(&dataset),
            baseline,
            "stock IVF retained local FDs after dropping a metadata-disabled session"
        );
    }
}
