use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use quanta_index_core::{CoreError, TextEmbeddingProvider};
use sha2::{Digest, Sha256};

const FIELD_SEPARATOR: &[u8] = b"\x1f";
const FLOAT_BYTES: usize = 4;

/// Persistent (or in-memory) store of `content -> embedding vector`, keyed so a
/// model or dimension change can never reuse a stale vector. A miss returns
/// `None`; `put` is best-effort (a write failure only forces a future recompute,
/// never a wrong result).
pub trait EmbeddingCache: Send + Sync {
    fn get(&self, key: &str) -> Option<Vec<f32>>;
    fn put(&self, key: &str, vector: &[f32]);
}

/// Wraps any [`TextEmbeddingProvider`], serving cached vectors and only calling
/// the inner provider for cache misses. Keys bind the inner provider's model id
/// and dimension, so a model swap yields fresh keys (no stale reuse).
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
        let mut miss_texts: Vec<&str> = Vec::new();
        let mut miss_keys: Vec<String> = Vec::new();
        let mut miss_positions: Vec<usize> = Vec::new();
        for (position, &text) in texts.iter().enumerate() {
            let key = cache_key(model_id, dimension, text);
            match self.cache.get(&key) {
                Some(vector) => slots.push(Some(vector)),
                None => {
                    slots.push(None);
                    miss_texts.push(text);
                    miss_keys.push(key);
                    miss_positions.push(position);
                }
            }
        }
        if !miss_texts.is_empty() {
            let fresh = self.inner.embed_batch(&miss_texts)?;
            if fresh.len() != miss_texts.len() {
                return Err(CoreError::Storage(format!(
                    "embedding cache: inner returned {} vectors for {} misses",
                    fresh.len(),
                    miss_texts.len()
                )));
            }
            for ((position, key), vector) in
                miss_positions.iter().zip(miss_keys.iter()).zip(fresh)
            {
                self.cache.put(key, &vector);
                if let Some(slot) = slots.get_mut(*position) {
                    *slot = Some(vector);
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
        hex.push_str(&format!("{byte:02x}"));
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
        self.entries
            .lock()
            .ok()
            .and_then(|guard| guard.get(key).cloned())
    }

    fn put(&self, key: &str, vector: &[f32]) {
        if let Ok(mut guard) = self.entries.lock() {
            drop(guard.insert(key.to_string(), vector.to_vec()));
        }
    }
}

/// Durable file cache: one `<key>.vec` file per entry under `root`, holding the
/// vector as little-endian `f32` bytes.
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

    fn path_for(&self, key: &str) -> PathBuf {
        self.root.join(format!("{key}.vec"))
    }
}

impl EmbeddingCache for FileEmbeddingCache {
    fn get(&self, key: &str) -> Option<Vec<f32>> {
        let bytes = std::fs::read(self.path_for(key)).ok()?;
        decode_vector(&bytes)
    }

    fn put(&self, key: &str, vector: &[f32]) {
        // Best-effort: a write failure only forces a recompute next time. The
        // returned vector is already valid, so caching must not gate embedding.
        let bytes = encode_vector(vector);
        drop(std::fs::write(self.path_for(key), &bytes));
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
        let array = <[u8; FLOAT_BYTES]>::try_from(chunk).ok()?;
        out.push(f32::from_le_bytes(array));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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
        let provider = CachingEmbeddingProvider::new(
            Box::new(counting("m-1", Arc::clone(&embedded))),
            cache,
        );
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
        assert_eq!(embedded_b.load(Ordering::SeqCst), 1, "model change must recompute, not reuse m-1's vector");
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
        assert_eq!(embedded2.load(Ordering::SeqCst), 0, "persisted cache must avoid all re-embedding");
    }
}
