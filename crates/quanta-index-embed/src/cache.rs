use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;

use quanta_index_core::{CoreError, TextEmbeddingProvider};
use sha2::{Digest, Sha256};

use crate::telemetry;

const FIELD_SEPARATOR: &[u8] = b"\x1f";
const FLOAT_BYTES: usize = 4;

/// Store of `content -> embedding vector`, keyed by model + dimension.
///
/// Keying by model/dimension means a model or dimension change can never reuse a
/// stale vector. A miss returns `None`; `put` is best-effort (a write failure
/// only forces a future recompute, never a wrong result).
pub trait EmbeddingCache: Send + Sync {
    fn get(&self, key: &str) -> Option<Vec<f32>>;
    fn put(&self, key: &str, vector: &[f32]);
}

/// Wraps any [`TextEmbeddingProvider`], serving cached vectors for hits.
///
/// Only cache misses call the inner provider. Keys bind the inner provider's
/// model id and dimension, so a model swap yields fresh keys (no stale reuse).
pub struct CachingEmbeddingProvider {
    inner: Box<dyn TextEmbeddingProvider>,
    cache: Box<dyn EmbeddingCache>,
}

impl CachingEmbeddingProvider {
    #[must_use]
    pub fn new(inner: Box<dyn TextEmbeddingProvider>, cache: Box<dyn EmbeddingCache>) -> Self {
        Self { inner, cache }
    }
}

impl TextEmbeddingProvider for CachingEmbeddingProvider {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        let model_id = self.inner.model_id();
        let dimension = self.inner.dimension();
        let mut slots: Vec<Option<Vec<f32>>> = Vec::with_capacity(texts.len());
        let mut cache_hit_count = 0_usize;
        // Misses deduped BY CONTENT: each distinct uncached text is embedded once
        // and its vector is fanned out to every position that requested it, so a
        // batch with N copies of one text costs one inner embedding, not N.
        let mut distinct_texts: Vec<&str> = Vec::new();
        let mut distinct_keys: Vec<String> = Vec::new();
        let mut waiters: Vec<Vec<usize>> = Vec::new();
        let mut distinct_index_by_key: BTreeMap<String, usize> = BTreeMap::new();
        for (position, &text) in texts.iter().enumerate() {
            let key = cache_key(model_id, dimension, text);
            if let Some(vector) = self.cache.get(&key) {
                cache_hit_count = cache_hit_count.saturating_add(1);
                slots.push(Some(vector));
                continue;
            }
            slots.push(None);
            if let Some(&existing) = distinct_index_by_key.get(&key) {
                if let Some(positions) = waiters.get_mut(existing) {
                    positions.push(position);
                }
            } else {
                let next_index = distinct_texts.len();
                let _ = distinct_index_by_key.insert(key.clone(), next_index);
                distinct_texts.push(text);
                distinct_keys.push(key);
                waiters.push(vec![position]);
            }
        }
        telemetry::record_cache_observation(texts.len(), cache_hit_count, distinct_texts.len());
        if !distinct_texts.is_empty() {
            let fresh = self.inner.embed_batch(&distinct_texts)?;
            if fresh.len() != distinct_texts.len() {
                return Err(CoreError::Storage(format!(
                    "embedding cache: inner returned {} vectors for {} distinct misses",
                    fresh.len(),
                    distinct_texts.len()
                )));
            }
            for ((key, positions), vector) in distinct_keys.iter().zip(waiters.iter()).zip(fresh) {
                self.cache.put(key, &vector);
                // Fan out to every waiter: clone for all but the last, then MOVE the
                // vector into the last slot. The common case (one waiter, no
                // duplicate text) does zero clones — saving one dim-sized copy per
                // distinct text over a large corpus.
                if let Some((&last, rest)) = positions.split_last() {
                    for &position in rest {
                        if let Some(slot) = slots.get_mut(position) {
                            *slot = Some(vector.clone());
                        }
                    }
                    if let Some(slot) = slots.get_mut(last) {
                        *slot = Some(vector);
                    }
                }
            }
        }
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(slots.len());
        for slot in slots {
            out.push(slot.ok_or_else(|| {
                CoreError::Storage("embedding cache: unfilled vector slot".to_string())
            })?);
        }
        Ok(out)
    }

    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn model_version(&self) -> Option<&str> {
        self.inner.model_version()
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }
}

/// Hex SHA-256 over `(model_id, dimension, text)`; model+dimension are part of the
/// key so a model/dimension change cannot collide with a prior model's vector.
fn cache_key(model_id: &str, dimension: usize, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(model_id.as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(dimension.to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len().saturating_mul(2));
    for byte in digest {
        // Infallible write into a String; `write!` avoids the per-byte temporary
        // `format!` allocation that `format_push_string`/`format_collect` flag.
        let _written: Result<(), std::fmt::Error> = write!(hex, "{byte:02x}");
    }
    hex
}

/// In-memory cache (process-lifetime; used in tests and as a non-persistent
/// fallback).
#[derive(Default)]
pub struct InMemoryEmbeddingCache {
    entries: Mutex<BTreeMap<String, Vec<f32>>>,
}

impl EmbeddingCache for InMemoryEmbeddingCache {
    fn get(&self, key: &str) -> Option<Vec<f32>> {
        // A poisoned lock degrades to a miss (forces recompute), never a wrong hit.
        match self.entries.lock() {
            Ok(guard) => guard.get(key).cloned(),
            Err(_poisoned) => None,
        }
    }

    fn put(&self, key: &str, vector: &[f32]) {
        if let Ok(mut guard) = self.entries.lock() {
            drop(guard.insert(key.to_string(), vector.to_vec()));
        }
    }
}

/// Number of leading hex characters of the (SHA-256) key used as a shard
/// subdirectory. Two hex chars = 256 buckets, keeping any single directory's
/// fan-out ~1/256th of the corpus so directory operations stay fast.
const SHARD_PREFIX_LEN: usize = 2;

/// Durable file cache: one `<key>.vec` file per entry, sharded as
/// `root/<key[..2]>/<key>.vec` and holding the vector as little-endian `f32`
/// bytes. Sharding avoids piling tens of thousands of files into one directory
/// (a filesystem scaling cliff); a legacy flat `root/<key>.vec` is still read so
/// upgrading an existing cache never triggers a paid re-embed storm.
pub struct FileEmbeddingCache {
    root: PathBuf,
}

impl FileEmbeddingCache {
    /// Create a cache rooted at `root`, creating the directory if needed. A
    /// directory-creation failure is returned (the caller decides whether to fall
    /// back to in-memory).
    pub fn new(root: PathBuf) -> Result<Self, CoreError> {
        std::fs::create_dir_all(&root).map_err(|err| {
            CoreError::Storage(format!(
                "embedding cache: create dir {} failed: {err}",
                root.display()
            ))
        })?;
        Ok(Self { root })
    }

    /// Sharded path `root/<key[..2]>/<key>.vec`. SHA-256 hex keys distribute the
    /// prefix uniformly across buckets. Keys shorter than the prefix (only test
    /// keys) fall back to a single `_` bucket.
    fn path_for(&self, key: &str) -> PathBuf {
        let shard = key.get(0..SHARD_PREFIX_LEN).unwrap_or("_");
        self.root.join(shard).join(format!("{key}.vec"))
    }

    /// Pre-sharding flat path `root/<key>.vec`, read-only for migration so an
    /// existing flat cache is still served after the layout change.
    fn legacy_flat_path(&self, key: &str) -> PathBuf {
        self.root.join(format!("{key}.vec"))
    }
}

impl EmbeddingCache for FileEmbeddingCache {
    fn get(&self, key: &str) -> Option<Vec<f32>> {
        // A read failure (absent / unreadable) degrades to a miss, never a wrong
        // hit. Try the sharded path, then the legacy flat path so a pre-sharding
        // cache keeps serving (the extra stat only happens on a sharded miss, when
        // a network embedding is imminent anyway, so its relative cost is ~0).
        let bytes = match std::fs::read(self.path_for(key)) {
            Ok(bytes) => bytes,
            Err(_absent) => match std::fs::read(self.legacy_flat_path(key)) {
                Ok(bytes) => bytes,
                Err(_absent) => return None,
            },
        };
        decode_vector(&bytes)
    }

    fn put(&self, key: &str, vector: &[f32]) {
        // Best-effort: any failure only forces a recompute next time. The returned
        // vector is already valid, so caching must not gate embedding. Ensure the
        // shard subdir exists before writing (cheap no-op once warm).
        let bytes = encode_vector(vector);
        let path = self.path_for(key);
        if let Some(parent) = path.parent() {
            drop(std::fs::create_dir_all(parent));
        }
        drop(std::fs::write(path, &bytes));
    }
}

fn encode_vector(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len().saturating_mul(FLOAT_BYTES));
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode_vector(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len().checked_rem(FLOAT_BYTES) != Some(0) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len().checked_div(FLOAT_BYTES).unwrap_or(0));
    for chunk in bytes.chunks(FLOAT_BYTES) {
        let array = match <[u8; FLOAT_BYTES]>::try_from(chunk) {
            Ok(array) => array,
            Err(_malformed) => return None,
        };
        out.push(f32::from_le_bytes(array));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts how many texts it was asked to embed, so a cache hit can be proven
    /// to avoid the inner provider.
    struct CountingProvider {
        model_id: String,
        dimension: usize,
        embedded: Arc<AtomicUsize>,
    }

    impl TextEmbeddingProvider for CountingProvider {
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
            let _prior = self.embedded.fetch_add(texts.len(), Ordering::SeqCst);
            // Deterministic fake vector: [len(text) as bytes...] padded to dim.
            Ok(texts
                .iter()
                .map(|text| {
                    let mut v = vec![0.0_f32; self.dimension];
                    if let Some(first) = v.first_mut() {
                        *first = u16::try_from(text.len()).map_or(0.0, f32::from);
                    }
                    v
                })
                .collect())
        }
        fn model_id(&self) -> &str {
            &self.model_id
        }
        fn model_version(&self) -> Option<&str> {
            None
        }
        fn dimension(&self) -> usize {
            self.dimension
        }
    }

    fn counting(model_id: &str, embedded: Arc<AtomicUsize>) -> CountingProvider {
        CountingProvider {
            model_id: model_id.to_string(),
            dimension: 2,
            embedded,
        }
    }

    // P0-1: duplicate texts within one batch are embedded once and fanned out.
    #[test]
    fn duplicate_texts_embed_once_and_fan_out() {
        let embedded = Arc::new(AtomicUsize::new(0));
        let provider = CachingEmbeddingProvider::new(
            Box::new(counting("m-dedup", Arc::clone(&embedded))),
            Box::new(InMemoryEmbeddingCache::default()),
        );
        let out = provider.embed_batch(&["a", "a", "bb"]).expect("dedup ok");
        assert_eq!(out.len(), 3);
        // The two "a" positions share the same vector...
        assert_eq!(
            out.first(),
            out.get(1),
            "duplicate positions must share a vector"
        );
        // ...and "bb" differs.
        assert_ne!(out.first(), out.get(2));
        // Distinct misses = {"a","bb"} = 2 inner embeds, NOT 3.
        assert_eq!(
            embedded.load(Ordering::SeqCst),
            2,
            "duplicate text must not be embedded twice"
        );
    }

    #[test]
    fn second_identical_batch_is_served_from_cache() {
        let embedded = Arc::new(AtomicUsize::new(0));
        let provider = CachingEmbeddingProvider::new(
            Box::new(counting("m-1", Arc::clone(&embedded))),
            Box::new(InMemoryEmbeddingCache::default()),
        );
        let first = provider.embed_batch(&["a", "bb"]).expect("first ok");
        let second = provider.embed_batch(&["a", "bb"]).expect("second ok");
        assert_eq!(first, second);
        // Inner embedded only the first batch (2 texts); the second was all hits.
        assert_eq!(embedded.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn partial_hit_only_embeds_the_misses() {
        let embedded = Arc::new(AtomicUsize::new(0));
        let cache = Box::new(InMemoryEmbeddingCache::default());
        let provider =
            CachingEmbeddingProvider::new(Box::new(counting("m-1", Arc::clone(&embedded))), cache);
        let _warm = provider.embed_batch(&["a"]).expect("warm ok");
        // "a" is cached; only "bb" is a miss on the second call.
        let out = provider.embed_batch(&["a", "bb"]).expect("mixed ok");
        assert_eq!(out.len(), 2);
        assert_eq!(embedded.load(Ordering::SeqCst), 2); // 1 (warm) + 1 (miss)
    }

    #[test]
    fn different_model_id_does_not_reuse_stale_vectors() {
        let cache = Arc::new(InMemoryEmbeddingCache::default());
        // Provider A caches "a" under model m-1.
        let embedded_a = Arc::new(AtomicUsize::new(0));
        {
            let inner = counting("m-1", Arc::clone(&embedded_a));
            // Wrap a shared cache via a thin forwarder.
            let provider = CachingEmbeddingProvider::new(
                Box::new(inner),
                Box::new(SharedCache(Arc::clone(&cache))),
            );
            let _a = provider.embed_batch(&["a"]).expect("a ok");
        }
        // Provider B (different model) must MISS on "a" (model-scoped key).
        let embedded_b = Arc::new(AtomicUsize::new(0));
        let provider_b = CachingEmbeddingProvider::new(
            Box::new(counting("m-2", Arc::clone(&embedded_b))),
            Box::new(SharedCache(Arc::clone(&cache))),
        );
        let _b = provider_b.embed_batch(&["a"]).expect("b ok");
        assert_eq!(
            embedded_b.load(Ordering::SeqCst),
            1,
            "model change must recompute, not reuse m-1's vector"
        );
    }

    struct SharedCache(Arc<InMemoryEmbeddingCache>);
    impl EmbeddingCache for SharedCache {
        fn get(&self, key: &str) -> Option<Vec<f32>> {
            self.0.get(key)
        }
        fn put(&self, key: &str, vector: &[f32]) {
            self.0.put(key, vector);
        }
    }

    #[test]
    fn file_cache_round_trips_vectors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = FileEmbeddingCache::new(dir.path().join("embed-cache")).expect("file cache");
        assert!(cache.get("missing").is_none());
        cache.put("k1", &[1.0, -0.5, 2.25]);
        assert_eq!(cache.get("k1"), Some(vec![1.0, -0.5, 2.25]));
    }

    #[test]
    fn file_cache_shards_entries_into_prefix_subdirectories() {
        // A put must land under root/<key[..2]>/<key>.vec, not flat under root, so
        // a large corpus never piles all entries into one directory.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("embed-cache");
        let cache = FileEmbeddingCache::new(root.clone()).expect("file cache");
        let key = "abcd1234ef"; // stands in for a SHA-256 hex key
        cache.put(key, &[1.0, 2.0]);
        let sharded = root.join("ab").join("abcd1234ef.vec");
        let flat = root.join("abcd1234ef.vec");
        assert!(
            sharded.exists(),
            "entry must be written under its shard subdir {}",
            sharded.display()
        );
        assert!(!flat.exists(), "entry must NOT be written flat under root");
        // ...and it round-trips back through the sharded read path.
        assert_eq!(cache.get(key), Some(vec![1.0, 2.0]));
    }

    #[test]
    fn file_cache_reads_legacy_flat_layout_after_sharding_upgrade() {
        // Migration safety: a pre-sharding flat entry must still be served so
        // upgrading the layout never forces a paid re-embed of an existing cache.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("embed-cache");
        let cache = FileEmbeddingCache::new(root.clone()).expect("file cache");
        let key = "deadbeef99";
        // Write the entry in the OLD flat layout directly (no shard subdir).
        let flat = root.join("deadbeef99.vec");
        std::fs::write(&flat, encode_vector(&[7.0, -1.5])).expect("seed legacy flat entry");
        assert!(
            !root.join("de").join("deadbeef99.vec").exists(),
            "precondition: no sharded copy yet"
        );
        assert_eq!(
            cache.get(key),
            Some(vec![7.0, -1.5]),
            "legacy flat entry must still be readable after the sharding upgrade"
        );
    }

    #[test]
    fn file_cache_serves_a_cached_batch_without_reembedding() {
        let dir = tempfile::tempdir().expect("tempdir");
        let embedded = Arc::new(AtomicUsize::new(0));
        let make = |embedded: Arc<AtomicUsize>| {
            CachingEmbeddingProvider::new(
                Box::new(counting("m-file", embedded)),
                Box::new(
                    FileEmbeddingCache::new(dir.path().join("embed-cache")).expect("file cache"),
                ),
            )
        };
        let first = make(Arc::clone(&embedded));
        let a = first.embed_batch(&["a", "bb"]).expect("first ok");
        // A fresh provider + fresh file cache over the SAME dir reuses the vectors.
        let embedded2 = Arc::new(AtomicUsize::new(0));
        let second = make(Arc::clone(&embedded2));
        let b = second.embed_batch(&["a", "bb"]).expect("second ok");
        assert_eq!(a, b);
        assert_eq!(
            embedded2.load(Ordering::SeqCst),
            0,
            "persisted cache must avoid all re-embedding"
        );
    }
}
