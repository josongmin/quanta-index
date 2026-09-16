//! Content-addressed embedding cache with a verified entry format
//! (QI-BB-028).
//!
//! An entry is keyed by the embedding *identity* — model id, model revision,
//! dimension and normalization policy — and the text, so a provider revision
//! that produces different vectors under the same model name can never serve
//! the previous revision's vectors. Each identity is its own namespace
//! directory, so rotating a revision leaves the old namespace behind as a
//! unit an operator can remove.
//!
//! Every persisted entry is self-describing: a magic, a format version, the
//! dimension, the little-endian `f32` payload and a truncated SHA-256 over
//! header and payload. A hit is served only after the entry decodes, the
//! digest matches and the vector passes the same validator fresh provider
//! output passes; anything else is a miss, and the malformed file is
//! removed so it cannot be re-read. Writes go through a temporary file,
//! `fsync`, rename and parent `fsync`, so a crash leaves either the previous
//! entry or the new one, never a torn file.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use quanta_index_contract::EmbeddingNormalization;
use quanta_index_core::{CoreError, SemanticPolicy, TextEmbeddingProvider};
use sha2::{Digest, Sha256};

use crate::telemetry;

const FIELD_SEPARATOR: &[u8] = b"\x1f";
const FLOAT_BYTES: usize = 4;
const KEY_DOMAIN: &[u8] = b"quanta-index:embedding-cache-key:v2\0";
const NAMESPACE_DOMAIN: &[u8] = b"quanta-index:embedding-cache-namespace:v2\0";
/// Hex characters of the namespace digest used as the directory name.
const NAMESPACE_LEN: usize = 16;

/// Store of `content -> embedding vector` within one identity namespace.
///
/// A miss returns `None`; `put` is best-effort (a write failure only forces
/// a future recompute, never a wrong result); `evict` removes an entry the
/// caller found unusable so it is not read again.
pub trait EmbeddingCache: Send + Sync {
    fn get(&self, key: &str) -> Option<Vec<f32>>;
    fn put(&self, key: &str, vector: &[f32]);
    fn evict(&self, key: &str);
}

/// The identity every cache key and namespace is derived from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddingCacheIdentityV1 {
    model_id: String,
    model_revision: String,
    dimension: usize,
    normalization: EmbeddingNormalization,
}

impl EmbeddingCacheIdentityV1 {
    /// The identity a provider's vectors are cached under.
    #[must_use]
    pub fn of(provider: &dyn TextEmbeddingProvider) -> Self {
        Self {
            model_id: provider.model_id().to_string(),
            model_revision: provider.model_revision().to_string(),
            dimension: provider.dimension(),
            normalization: provider.normalization(),
        }
    }

    fn hash_fields(&self, hasher: &mut Sha256) {
        hasher.update(self.model_id.as_bytes());
        hasher.update(FIELD_SEPARATOR);
        hasher.update(self.model_revision.as_bytes());
        hasher.update(FIELD_SEPARATOR);
        hasher.update(self.dimension.to_le_bytes());
        hasher.update(FIELD_SEPARATOR);
        hasher.update(normalization_tag(self.normalization).as_bytes());
    }

    /// The namespace this identity's entries live under: a stable hex prefix
    /// of the identity digest, distinct for every model, revision,
    /// dimension or policy.
    #[must_use]
    pub fn namespace(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(NAMESPACE_DOMAIN);
        self.hash_fields(&mut hasher);
        let mut hex = hex_encode(hasher.finalize().as_slice());
        hex.truncate(NAMESPACE_LEN);
        hex
    }

    /// The cache key for `text` under this identity.
    #[must_use]
    pub fn key(&self, text: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(KEY_DOMAIN);
        self.hash_fields(&mut hasher);
        hasher.update(FIELD_SEPARATOR);
        hasher.update(text.as_bytes());
        hex_encode(hasher.finalize().as_slice())
    }
}

const fn normalization_tag(normalization: EmbeddingNormalization) -> &'static str {
    match normalization {
        EmbeddingNormalization::None => "none",
        EmbeddingNormalization::L2Unit => "l2unit",
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        // Infallible write into a String.
        let _written: Result<(), std::fmt::Error> = write!(hex, "{byte:02x}");
    }
    hex
}

/// Wraps any [`TextEmbeddingProvider`], serving cached vectors for hits.
///
/// Only cache misses call the inner provider. Keys bind the inner
/// provider's full identity, so a model, revision, dimension or policy
/// change yields fresh keys. A hit is validated exactly as fresh output is;
/// a hit that fails validation is evicted and recomputed.
pub struct CachingEmbeddingProvider {
    inner: Box<dyn TextEmbeddingProvider>,
    cache: Box<dyn EmbeddingCache>,
    identity: EmbeddingCacheIdentityV1,
}

impl CachingEmbeddingProvider {
    #[must_use]
    pub fn new(inner: Box<dyn TextEmbeddingProvider>, cache: Box<dyn EmbeddingCache>) -> Self {
        let identity = EmbeddingCacheIdentityV1::of(inner.as_ref());
        Self {
            inner,
            cache,
            identity,
        }
    }

    #[must_use]
    pub const fn identity(&self) -> &EmbeddingCacheIdentityV1 {
        &self.identity
    }

    /// A cached vector that is usable, or `None` after evicting one that is
    /// not.
    fn usable_hit(&self, key: &str) -> Option<Vec<f32>> {
        let vector = self.cache.get(key)?;
        match SemanticPolicy::validate_embedding_vector_v1(
            &vector,
            self.identity.dimension,
            self.identity.normalization,
        ) {
            Ok(()) => Some(vector),
            Err(_defect) => {
                // The entry decoded but is not a vector this identity could
                // have produced; it must not be served again.
                self.cache.evict(key);
                None
            }
        }
    }
}

impl TextEmbeddingProvider for CachingEmbeddingProvider {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
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
            let key = self.identity.key(text);
            if let Some(vector) = self.usable_hit(&key) {
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
                let previous = distinct_index_by_key.insert(key.clone(), next_index);
                debug_assert!(
                    previous.is_none(),
                    "distinct embedding miss key must not already be indexed"
                );
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
                // Fresh output is held to the same contract a hit is; a
                // provider that breaks it fails the batch instead of
                // poisoning the cache.
                SemanticPolicy::validate_embedding_vector_v1(
                    &vector,
                    self.identity.dimension,
                    self.identity.normalization,
                )?;
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

    fn model_revision(&self) -> &str {
        self.inner.model_revision()
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    fn normalization(&self) -> EmbeddingNormalization {
        self.inner.normalization()
    }
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

    fn evict(&self, key: &str) {
        if let Ok(mut guard) = self.entries.lock() {
            drop(guard.remove(key));
        }
    }
}

/// Number of leading hex characters of the (SHA-256) key used as a shard
/// subdirectory.
///
/// Two hex chars = 256 buckets, keeping any single directory's fan-out
/// ~1/256th of the corpus so directory operations stay fast.
const SHARD_PREFIX_LEN: usize = 2;

/// Magic prefix of a persisted entry.
const ENTRY_MAGIC: &[u8; 4] = b"QIEC";
/// Entry format this crate writes and reads.
const ENTRY_FORMAT_VERSION: u16 = 2;
/// Bytes of the truncated SHA-256 that close every entry.
const ENTRY_DIGEST_LEN: usize = 16;
const ENTRY_HEADER_LEN: usize = ENTRY_MAGIC.len() + 2 + 4;

static ATOMIC_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Durable file cache: one verified `<key>.vec` per entry under
/// `root/<namespace>/<key[..2]>/<key>.vec`.
///
/// The namespace directory is the identity's ([`EmbeddingCacheIdentityV1::
/// namespace`]), so a revision rotation starts an empty namespace and leaves
/// the previous one whole. Sharding keeps directory fan-out bounded.
pub struct FileEmbeddingCache {
    root: PathBuf,
}

impl FileEmbeddingCache {
    /// Create a cache for `identity` rooted at `root/<namespace>`, creating the
    /// directory if needed.
    pub fn new(root: &Path, identity: &EmbeddingCacheIdentityV1) -> Result<Self, CoreError> {
        let root = root.join(identity.namespace());
        std::fs::create_dir_all(&root).map_err(|err| {
            CoreError::Storage(format!(
                "embedding cache: create dir {} failed: {err}",
                root.display()
            ))
        })?;
        Ok(Self { root })
    }

    /// The namespace directory entries are written under.
    #[must_use]
    pub fn namespace_dir(&self) -> &Path {
        &self.root
    }

    /// Sharded path `root/<key[..2]>/<key>.vec`. SHA-256 hex keys distribute the
    /// prefix uniformly across buckets. Keys shorter than the prefix (only test
    /// keys) fall back to a single `_` bucket.
    fn path_for(&self, key: &str) -> PathBuf {
        let shard = key.get(0..SHARD_PREFIX_LEN).unwrap_or("_");
        self.root.join(shard).join(format!("{key}.vec"))
    }

    /// Replace `path` with `bytes` so a reader sees either the previous
    /// entry or the whole new one: temporary file, `fsync`, rename, parent
    /// `fsync`. Any failure removes the temporary file.
    fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("cache entry has no parent"))?;
        std::fs::create_dir_all(parent)?;
        let sequence = ATOMIC_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let staging = parent.join(format!(
            ".{}.tmp-{}-{sequence}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("entry"),
            std::process::id()
        ));
        let outcome = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staging)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&staging, path)?;
            std::fs::File::open(parent)?.sync_all()
        })();
        if outcome.is_err() {
            drop(std::fs::remove_file(&staging));
        }
        outcome
    }
}

impl EmbeddingCache for FileEmbeddingCache {
    fn get(&self, key: &str) -> Option<Vec<f32>> {
        let path = self.path_for(key);
        // An absent or unreadable file is a miss; the error carries nothing a
        // cache can act on beyond recomputing.
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_absent_or_unreadable) => return None,
        };
        let decoded = decode_entry(&bytes);
        if decoded.is_none() {
            // A file that exists but does not decode as an entry of this
            // format is not a miss that will heal itself; remove it.
            drop(std::fs::remove_file(&path));
        }
        decoded
    }

    fn put(&self, key: &str, vector: &[f32]) {
        // Best-effort: any failure only forces a recompute next time. The
        // returned vector is already valid, so caching must not gate embedding.
        let Some(bytes) = encode_entry(vector) else {
            return;
        };
        drop(Self::write_atomic(&self.path_for(key), &bytes));
    }

    fn evict(&self, key: &str) {
        drop(std::fs::remove_file(self.path_for(key)));
    }
}

fn entry_digest(header_and_payload: &[u8]) -> [u8; ENTRY_DIGEST_LEN] {
    let digest = Sha256::digest(header_and_payload);
    let mut truncated = [0_u8; ENTRY_DIGEST_LEN];
    for (slot, byte) in truncated.iter_mut().zip(digest.iter()) {
        *slot = *byte;
    }
    truncated
}

/// Encode one entry; `None` when the vector's length does not fit the
/// header's `u32` dimension.
fn encode_entry(vector: &[f32]) -> Option<Vec<u8>> {
    let Ok(dimension) = u32::try_from(vector.len()) else {
        return None;
    };
    let mut bytes = Vec::with_capacity(
        ENTRY_HEADER_LEN
            .saturating_add(vector.len().saturating_mul(FLOAT_BYTES))
            .saturating_add(ENTRY_DIGEST_LEN),
    );
    bytes.extend_from_slice(ENTRY_MAGIC);
    bytes.extend_from_slice(&ENTRY_FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&dimension.to_le_bytes());
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let digest = entry_digest(&bytes);
    bytes.extend_from_slice(&digest);
    Some(bytes)
}

/// Decode one entry, refusing anything that is not exactly an entry of this
/// format with a matching digest and the declared number of components.
fn decode_entry(bytes: &[u8]) -> Option<Vec<f32>> {
    let (header, rest) = bytes.split_at_checked(ENTRY_HEADER_LEN)?;
    let (magic, header_rest) = header.split_at_checked(ENTRY_MAGIC.len())?;
    if magic != ENTRY_MAGIC {
        return None;
    }
    let (format, dimension_bytes) = header_rest.split_at_checked(2)?;
    let Ok(format) = <[u8; 2]>::try_from(format) else {
        return None;
    };
    if u16::from_le_bytes(format) != ENTRY_FORMAT_VERSION {
        return None;
    }
    let Ok(dimension_bytes) = <[u8; 4]>::try_from(dimension_bytes) else {
        return None;
    };
    let Ok(dimension) = usize::try_from(u32::from_le_bytes(dimension_bytes)) else {
        return None;
    };
    let payload_len = dimension.checked_mul(FLOAT_BYTES)?;
    let (payload, digest) = rest.split_at_checked(payload_len)?;
    if digest.len() != ENTRY_DIGEST_LEN {
        return None;
    }
    let committed = bytes.get(..ENTRY_HEADER_LEN.checked_add(payload_len)?)?;
    if entry_digest(committed) != digest {
        return None;
    }
    let mut out = Vec::with_capacity(dimension);
    for chunk in payload.chunks(FLOAT_BYTES) {
        let Ok(array) = <[u8; FLOAT_BYTES]>::try_from(chunk) else {
            return None;
        };
        out.push(f32::from_le_bytes(array));
    }
    Some(out)
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "fault-injection fixtures index entries they just wrote at known offsets; an out-of-range index is a test authoring bug that should fail loudly"
)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts how many texts it was asked to embed, so a cache hit can be proven
    /// to avoid the inner provider. Vectors are unit-normalized, as the
    /// composition root's wrapper guarantees for a real provider.
    struct CountingProvider {
        model_id: String,
        model_revision: String,
        dimension: usize,
        embedded: Arc<AtomicUsize>,
    }

    impl TextEmbeddingProvider for CountingProvider {
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
            let _prior = self.embedded.fetch_add(texts.len(), Ordering::SeqCst);
            // Deterministic fake vector: text length in the first slot, the
            // revision length in the second, unit-normalized.
            Ok(texts
                .iter()
                .map(|text| {
                    let mut v = vec![0.0_f32; self.dimension];
                    if let Some(first) = v.first_mut() {
                        *first = u16::try_from(text.len()).map_or(0.0, f32::from);
                    }
                    if let Some(second) = v.get_mut(1) {
                        *second = u16::try_from(self.model_revision.len()).map_or(1.0, f32::from);
                    }
                    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                    for x in &mut v {
                        *x /= norm;
                    }
                    v
                })
                .collect())
        }
        fn model_id(&self) -> &str {
            &self.model_id
        }
        fn model_revision(&self) -> &str {
            &self.model_revision
        }
        fn dimension(&self) -> usize {
            self.dimension
        }
        fn normalization(&self) -> EmbeddingNormalization {
            EmbeddingNormalization::L2Unit
        }
    }

    fn counting(model_id: &str, embedded: Arc<AtomicUsize>) -> CountingProvider {
        counting_at(model_id, "r1", embedded)
    }

    fn counting_at(model_id: &str, revision: &str, embedded: Arc<AtomicUsize>) -> CountingProvider {
        CountingProvider {
            model_id: model_id.to_string(),
            model_revision: revision.to_string(),
            dimension: 2,
            embedded,
        }
    }

    fn identity(model_id: &str, revision: &str) -> EmbeddingCacheIdentityV1 {
        EmbeddingCacheIdentityV1 {
            model_id: model_id.to_string(),
            model_revision: revision.to_string(),
            dimension: 2,
            normalization: EmbeddingNormalization::L2Unit,
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
        assert_eq!(
            out.first(),
            out.get(1),
            "duplicate positions must share a vector"
        );
        assert_ne!(out.first(), out.get(2));
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
        assert_eq!(embedded.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn partial_hit_only_embeds_the_misses() {
        let embedded = Arc::new(AtomicUsize::new(0));
        let cache = Box::new(InMemoryEmbeddingCache::default());
        let provider =
            CachingEmbeddingProvider::new(Box::new(counting("m-1", Arc::clone(&embedded))), cache);
        let _warm = provider.embed_batch(&["a"]).expect("warm ok");
        let out = provider.embed_batch(&["a", "bb"]).expect("mixed ok");
        assert_eq!(out.len(), 2);
        assert_eq!(embedded.load(Ordering::SeqCst), 2); // 1 (warm) + 1 (miss)
    }

    struct SharedCache(Arc<InMemoryEmbeddingCache>);
    impl EmbeddingCache for SharedCache {
        fn get(&self, key: &str) -> Option<Vec<f32>> {
            self.0.get(key)
        }
        fn put(&self, key: &str, vector: &[f32]) {
            self.0.put(key, vector);
        }
        fn evict(&self, key: &str) {
            self.0.evict(key);
        }
    }

    /// Same model id and dimension, different revision: the second provider
    /// must miss (QI-BB-028), and so must a different model id.
    #[test]
    fn a_revision_or_model_change_never_reuses_a_cached_vector() {
        let cache = Arc::new(InMemoryEmbeddingCache::default());
        let embedded_a = Arc::new(AtomicUsize::new(0));
        {
            let provider = CachingEmbeddingProvider::new(
                Box::new(counting_at("m-1", "2024-01", Arc::clone(&embedded_a))),
                Box::new(SharedCache(Arc::clone(&cache))),
            );
            let _a = provider.embed_batch(&["a"]).expect("a ok");
        }
        let embedded_revision = Arc::new(AtomicUsize::new(0));
        let provider_revision = CachingEmbeddingProvider::new(
            Box::new(counting_at(
                "m-1",
                "2024-02",
                Arc::clone(&embedded_revision),
            )),
            Box::new(SharedCache(Arc::clone(&cache))),
        );
        let _r = provider_revision.embed_batch(&["a"]).expect("revision ok");
        assert_eq!(
            embedded_revision.load(Ordering::SeqCst),
            1,
            "a revision change must recompute, not reuse the previous revision's vector"
        );
        let embedded_model = Arc::new(AtomicUsize::new(0));
        let provider_model = CachingEmbeddingProvider::new(
            Box::new(counting_at("m-2", "2024-01", Arc::clone(&embedded_model))),
            Box::new(SharedCache(Arc::clone(&cache))),
        );
        let _m = provider_model.embed_batch(&["a"]).expect("model ok");
        assert_eq!(embedded_model.load(Ordering::SeqCst), 1);
        assert_ne!(
            identity("m-1", "2024-01").namespace(),
            identity("m-1", "2024-02").namespace(),
            "revisions live in distinct namespaces"
        );
    }

    /// A cached vector that decodes but is not one this identity could have
    /// produced (wrong dimension, non-finite, not unit) is evicted and
    /// recomputed rather than served.
    #[test]
    fn an_unusable_hit_is_evicted_and_recomputed() {
        let cache = Arc::new(InMemoryEmbeddingCache::default());
        let embedded = Arc::new(AtomicUsize::new(0));
        let provider = CachingEmbeddingProvider::new(
            Box::new(counting("m-1", Arc::clone(&embedded))),
            Box::new(SharedCache(Arc::clone(&cache))),
        );
        let key = provider.identity().key("a");
        for poisoned in [
            vec![0.5, 0.0],
            vec![f32::NAN, 0.0],
            vec![1.0, 0.0, 0.0],
            vec![0.0, 0.0],
        ] {
            cache.put(&key, &poisoned);
            let out = provider.embed_batch(&["a"]).expect("recompute ok");
            assert_ne!(out.first(), Some(&poisoned), "poisoned entry was served");
            assert!(
                cache.get(&key).is_some_and(|stored| stored != poisoned),
                "the poisoned entry must be replaced by the recomputed vector"
            );
        }
        assert_eq!(embedded.load(Ordering::SeqCst), 4);
    }

    /// Fresh provider output is held to the same contract; a provider that
    /// returns a non-unit vector under an `L2Unit` identity fails the batch and
    /// caches nothing.
    #[test]
    fn a_provider_that_breaks_its_own_contract_poisons_nothing() {
        struct Liar;
        impl TextEmbeddingProvider for Liar {
            fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
                Ok(texts.iter().map(|_| vec![2.0, 0.0]).collect())
            }
            fn model_id(&self) -> &'static str {
                "liar"
            }
            fn model_revision(&self) -> &'static str {
                "r1"
            }
            fn dimension(&self) -> usize {
                2
            }
            fn normalization(&self) -> EmbeddingNormalization {
                EmbeddingNormalization::L2Unit
            }
        }
        let cache = Arc::new(InMemoryEmbeddingCache::default());
        let provider = CachingEmbeddingProvider::new(
            Box::new(Liar),
            Box::new(SharedCache(Arc::clone(&cache))),
        );
        let key = provider.identity().key("a");
        assert!(provider.embed_batch(&["a"]).is_err());
        assert!(
            cache.get(&key).is_none(),
            "a refused vector must not be cached"
        );
    }

    #[test]
    fn file_cache_round_trips_vectors_under_the_identity_namespace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("embed-cache");
        let identity = identity("m-file", "r1");
        let cache = FileEmbeddingCache::new(&root, &identity).expect("file cache");
        assert_eq!(cache.namespace_dir(), root.join(identity.namespace()));
        assert!(cache.get("missing").is_none());
        let key = "abcd1234ef";
        cache.put(key, &[1.0, -0.5]);
        let sharded = root
            .join(identity.namespace())
            .join("ab")
            .join("abcd1234ef.vec");
        assert!(
            sharded.exists(),
            "entry must be written under {}",
            sharded.display()
        );
        assert_eq!(cache.get(key), Some(vec![1.0, -0.5]));
        assert!(
            std::fs::read_dir(sharded.parent().expect("shard"))
                .expect("list shard")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp-")),
            "no temporary file survives a successful write"
        );
    }

    type Damage = Box<dyn Fn(&[u8]) -> Vec<u8>>;

    /// Every way a persisted entry can be damaged reads as a miss, and the
    /// damaged file is removed so it is not read again.
    #[test]
    fn file_cache_refuses_and_removes_damaged_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = FileEmbeddingCache::new(&dir.path().join("embed-cache"), &identity("m", "r"))
            .expect("cache");
        let key = "feedface00";
        let path = cache.path_for(key);
        let cases: Vec<(&str, Damage)> = vec![
            (
                "truncated",
                Box::new(|bytes| bytes[..bytes.len() - 1].to_vec()),
            ),
            (
                "payload bit flip",
                Box::new(|bytes| {
                    let mut out = bytes.to_vec();
                    out[ENTRY_HEADER_LEN] ^= 0x01;
                    out
                }),
            ),
            (
                "digest bit flip",
                Box::new(|bytes| {
                    let mut out = bytes.to_vec();
                    let last = out.len() - 1;
                    out[last] ^= 0x80;
                    out
                }),
            ),
            (
                "wrong magic",
                Box::new(|bytes| {
                    let mut out = bytes.to_vec();
                    out[0] = b'X';
                    out
                }),
            ),
            (
                "legacy raw floats",
                Box::new(|_| 1.0_f32.to_le_bytes().to_vec()),
            ),
            ("empty", Box::new(|_| Vec::new())),
        ];
        for (label, damage) in cases {
            cache.put(key, &[0.6, 0.8]);
            let good = std::fs::read(&path).expect("entry written");
            std::fs::write(&path, damage(&good)).expect("damage");
            assert!(cache.get(key).is_none(), "{label}: damaged entry must miss");
            assert!(!path.exists(), "{label}: damaged entry must be removed");
        }
    }

    /// An entry that was written with different components but the same
    /// length is a different vector, not a hit for the old one: the digest
    /// covers the payload.
    #[test]
    fn same_length_rewrite_is_a_different_entry() {
        let a = encode_entry(&[0.6, 0.8]).expect("encode");
        let b = encode_entry(&[0.8, 0.6]).expect("encode");
        assert_ne!(a, b);
        assert_eq!(decode_entry(&a), Some(vec![0.6, 0.8]));
        assert_eq!(decode_entry(&b), Some(vec![0.8, 0.6]));
    }

    /// Concurrent writers of the same key never leave a torn entry: every
    /// interleaving ends with a decodable entry equal to one of the writes.
    #[test]
    fn concurrent_writers_leave_a_whole_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = Arc::new(
            FileEmbeddingCache::new(&dir.path().join("embed-cache"), &identity("m", "r"))
                .expect("cache"),
        );
        let key = "c0ffee0000";
        let writers = (0..8_u8)
            .map(|index| {
                let cache = Arc::clone(&cache);
                std::thread::spawn(move || {
                    let value = 0.1_f32 * (f32::from(index) + 1.0);
                    for _ in 0..50 {
                        cache.put(key, &[value, 1.0 - value]);
                        let read = cache.get(key).expect("an entry is always readable");
                        assert_eq!(read.len(), 2);
                        assert!(
                            (read[0] + read[1] - 1.0).abs() < 1e-6,
                            "torn entry: {read:?}"
                        );
                    }
                })
            })
            .collect::<Vec<_>>();
        for writer in writers {
            writer.join().expect("writer thread");
        }
        assert!(cache.get(key).is_some());
    }

    #[test]
    fn file_cache_serves_a_cached_batch_without_reembedding() {
        let dir = tempfile::tempdir().expect("tempdir");
        let embedded = Arc::new(AtomicUsize::new(0));
        let make = |embedded: Arc<AtomicUsize>| {
            let inner = counting("m-file", embedded);
            let cache = FileEmbeddingCache::new(
                &dir.path().join("embed-cache"),
                &EmbeddingCacheIdentityV1::of(&inner),
            )
            .expect("file cache");
            CachingEmbeddingProvider::new(Box::new(inner), Box::new(cache))
        };
        let first = make(Arc::clone(&embedded));
        let a = first.embed_batch(&["a", "bb"]).expect("first ok");
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
