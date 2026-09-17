//! Content-addressed embedding cache with a verified entry format and a
//! bounded footprint (QI-BB-028, QI-BB-009).
//!
//! An entry is keyed by the embedding *identity* — model id, model revision,
//! dimension and normalization policy — and the text, so a provider revision
//! that produces different vectors under the same model name can never serve
//! the previous revision's vectors. Each identity is its own namespace
//! directory, so rotating a revision starts an empty namespace and leaves
//! the previous one whole until retention retires it.
//!
//! Every persisted entry is self-describing: a magic, a format version, the
//! dimension, the little-endian `f32` payload and a truncated SHA-256 over
//! header and payload. A hit is served only after the entry decodes, the
//! digest matches and the vector passes the same validator fresh provider
//! output passes; anything else is a miss, and the malformed file is
//! removed so it cannot be re-read. Writes go through a temporary file,
//! `fsync`, rename and parent `fsync`, so a crash leaves either the previous
//! entry or the new one, never a torn file.
//!
//! Residency is bounded by an [`EmbeddingCacheRetentionPolicy`]: a
//! namespace holds at most so many entries and so many bytes, the least
//! recently used entries are evicted to stay within both, and the cache
//! root holds at most so many namespaces. The bound is enforced by an
//! in-process ledger that is rebuilt from the directory at open, so a
//! restart neither loses the accounting nor trusts a stale one, and an
//! eviction only ever removes whole entries — a concurrent reader sees the
//! entry or a miss, never a torn file.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

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
/// Bytes of a cache key: one SHA-256 digest.
const KEY_LEN: usize = 32;

/// Store of `content -> embedding vector` within one identity namespace.
///
/// A miss returns `None`; `put` is best-effort (a write failure only forces
/// a future recompute, never a wrong result); `evict` removes an entry the
/// caller found unusable so it is not read again; `stats` reports the
/// store's residency and traffic.
pub trait EmbeddingCache: Send + Sync {
    fn get(&self, key: &EmbeddingCacheKey) -> Option<Vec<f32>>;
    fn put(&self, key: &EmbeddingCacheKey, vector: &[f32]);
    fn evict(&self, key: &EmbeddingCacheKey);
    fn stats(&self) -> EmbeddingCacheStats;
}

/// The key of one entry: the SHA-256 of the identity and the text.
///
/// Only [`EmbeddingCacheIdentityV1::key`] produces one, so every key a
/// store sees is a full digest and the sharded on-disk layout is total.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EmbeddingCacheKey([u8; KEY_LEN]);

impl EmbeddingCacheKey {
    /// The key's digest, lowercase hex.
    #[must_use]
    pub fn hex(&self) -> String {
        hex_encode(&self.0)
    }

    /// A key from its hex form; `None` unless it is exactly one digest.
    #[must_use]
    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != KEY_LEN.saturating_mul(2) {
            return None;
        }
        let mut bytes = [0_u8; KEY_LEN];
        for (slot, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks(2)) {
            let Ok(pair) = std::str::from_utf8(pair) else {
                return None;
            };
            let Ok(byte) = u8::from_str_radix(pair, 16) else {
                return None;
            };
            *slot = byte;
        }
        // Only lowercase hex round-trips; an uppercase spelling is not a key
        // this module wrote.
        if hex_encode(&bytes) != hex {
            return None;
        }
        Some(Self(bytes))
    }
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
    pub fn key(&self, text: &str) -> EmbeddingCacheKey {
        let mut hasher = Sha256::new();
        hasher.update(KEY_DOMAIN);
        self.hash_fields(&mut hasher);
        hasher.update(FIELD_SEPARATOR);
        hasher.update(text.as_bytes());
        EmbeddingCacheKey(hasher.finalize().into())
    }
}

const fn normalization_tag(normalization: EmbeddingNormalization) -> &'static str {
    match normalization {
        EmbeddingNormalization::None => "none",
        EmbeddingNormalization::L2Unit => "l2unit",
    }
}

/// A count as `u64`, saturating on a platform whose `usize` is wider.
fn count_u64(value: usize) -> u64 {
    let Ok(value) = u64::try_from(value) else {
        return u64::MAX;
    };
    value
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        // Infallible write into a String.
        let _written: Result<(), std::fmt::Error> = write!(hex, "{byte:02x}");
    }
    hex
}

/// How much one cache namespace may hold, and how many namespaces a cache
/// root may keep.
///
/// Every bound is a strict maximum; zero is refused at construction because
/// a zero ceiling is a configuration defect, not a disabled cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmbeddingCacheRetentionPolicy {
    entries: u64,
    resident_bytes: u64,
    namespaces: usize,
}

impl EmbeddingCacheRetentionPolicy {
    /// Production defaults: 500,000 entries and 2 GiB per namespace, four
    /// namespaces per cache root (the current identity and the three most
    /// recently opened others, so an A/B rotation keeps both sides warm).
    pub const DEFAULT: Self = Self {
        entries: 500_000,
        resident_bytes: 2 * 1024 * 1024 * 1024,
        namespaces: 4,
    };

    /// A policy with explicit ceilings; each must be at least one.
    pub fn new(
        max_entries: u64,
        max_resident_bytes: u64,
        max_namespaces: usize,
    ) -> Result<Self, CoreError> {
        if max_entries == 0 || max_resident_bytes == 0 || max_namespaces == 0 {
            return Err(CoreError::InvalidContract(
                "embedding cache retention policy: every ceiling must be at least one".to_string(),
            ));
        }
        Ok(Self {
            entries: max_entries,
            resident_bytes: max_resident_bytes,
            namespaces: max_namespaces,
        })
    }

    /// Most entries one namespace holds.
    #[must_use]
    pub const fn max_entries(&self) -> u64 {
        self.entries
    }

    /// Most bytes of entries one namespace holds.
    #[must_use]
    pub const fn max_resident_bytes(&self) -> u64 {
        self.resident_bytes
    }

    /// Most namespaces one cache root keeps, the current one included.
    #[must_use]
    pub const fn max_namespaces(&self) -> usize {
        self.namespaces
    }
}

/// Residency and traffic of one cache store.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EmbeddingCacheStats {
    /// Entries resident now.
    pub entries: u64,
    /// Bytes resident now, as the store accounts them.
    pub resident_bytes: u64,
    /// Lookups answered from the store.
    pub hits: u64,
    /// Lookups the store could not answer.
    pub misses: u64,
    /// Misses caused by an entry that existed but did not decode; each one
    /// was removed.
    pub corrupt_misses: u64,
    /// Entries written.
    pub puts: u64,
    /// Entries removed to stay within the policy.
    pub evictions: u64,
    /// Writes refused because one entry alone exceeds the byte ceiling.
    pub refused_oversize: u64,
}

/// What opening a file cache found and removed before serving.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EmbeddingCacheOpenReport {
    /// Entries the namespace held when it was opened, before any eviction
    /// a tightened policy required.
    pub scanned_entries: u64,
    /// Entries evicted at open because the namespace exceeded the policy.
    pub evicted_at_open: u64,
    /// Temporary files of interrupted writes removed.
    pub stale_staging_removed: u64,
    /// Other identities' namespaces removed to stay within the namespace
    /// ceiling.
    pub retired_namespaces: u64,
    /// Pre-namespace (format v1) shard directories removed.
    pub reclaimed_legacy_directories: u64,
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

    /// The wrapped store's residency and traffic.
    #[must_use]
    pub fn cache_stats(&self) -> EmbeddingCacheStats {
        self.cache.stats()
    }

    /// A cached vector that is usable, or `None` after evicting one that is
    /// not.
    fn usable_hit(&self, key: &EmbeddingCacheKey) -> Option<Vec<f32>> {
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
        let mut distinct_keys: Vec<EmbeddingCacheKey> = Vec::new();
        let mut waiters: Vec<Vec<usize>> = Vec::new();
        let mut distinct_index_by_key: BTreeMap<EmbeddingCacheKey, usize> = BTreeMap::new();
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
                let previous = distinct_index_by_key.insert(key, next_index);
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

/// One resident entry as the ledger accounts it.
struct LedgerSlot<V> {
    tick: u64,
    bytes: u64,
    value: V,
}

/// Least-recently-used accounting shared by every store: which keys are
/// resident, how many bytes they occupy, and which to evict first.
///
/// Recency is a monotonic tick; a lookup or write moves the key to the
/// newest tick, and eviction pops the oldest. Both maps are `O(log n)` per
/// operation so a namespace of hundreds of thousands of entries costs the
/// same per hit as one of ten.
struct RetentionLedger<V> {
    policy: EmbeddingCacheRetentionPolicy,
    by_key: BTreeMap<EmbeddingCacheKey, LedgerSlot<V>>,
    by_tick: BTreeMap<u64, EmbeddingCacheKey>,
    next_tick: u64,
    resident_bytes: u64,
    stats: EmbeddingCacheStats,
}

impl<V> RetentionLedger<V> {
    fn new(policy: EmbeddingCacheRetentionPolicy) -> Self {
        Self {
            policy,
            by_key: BTreeMap::new(),
            by_tick: BTreeMap::new(),
            next_tick: 0,
            resident_bytes: 0,
            stats: EmbeddingCacheStats::default(),
        }
    }

    fn take_tick(&mut self) -> u64 {
        let tick = self.next_tick;
        self.next_tick = self.next_tick.saturating_add(1);
        tick
    }

    /// The value under `key`, made most recent; `None` counts a miss.
    fn touch(&mut self, key: &EmbeddingCacheKey) -> Option<&V> {
        let tick = self.take_tick();
        let Some(slot) = self.by_key.get_mut(key) else {
            self.stats.misses = self.stats.misses.saturating_add(1);
            return None;
        };
        let _previous: Option<EmbeddingCacheKey> = self.by_tick.remove(&slot.tick);
        slot.tick = tick;
        let _displaced: Option<EmbeddingCacheKey> = self.by_tick.insert(tick, *key);
        self.stats.hits = self.stats.hits.saturating_add(1);
        Some(&slot.value)
    }

    /// Whether one entry of `bytes` can ever be resident under the policy.
    const fn admits(&self, bytes: u64) -> bool {
        bytes <= self.policy.resident_bytes
    }

    /// Record `value` under `key` at `bytes`, replacing any previous slot,
    /// then evict least recently used entries until the policy holds.
    ///
    /// Returns what was evicted, oldest first, so a file store can remove
    /// the files. An entry the policy does not admit is refused outright
    /// and counted as such; the previous slot under that key, if any, is
    /// dropped too because the store no longer holds it.
    fn insert(&mut self, key: EmbeddingCacheKey, bytes: u64, value: V) -> LedgerInsert<V> {
        let displaced = self.remove(&key);
        if !self.admits(bytes) {
            self.stats.refused_oversize = self.stats.refused_oversize.saturating_add(1);
            return LedgerInsert {
                displaced,
                evicted: Vec::new(),
            };
        }
        self.stats.puts = self.stats.puts.saturating_add(1);
        let evicted = self.seed(key, bytes, value);
        LedgerInsert { displaced, evicted }
    }

    /// Record `value` under `key` at `bytes` as the newest entry without
    /// counting a write (a rebuild from disk is not traffic), then evict
    /// least recently used entries until the policy holds.
    fn seed(
        &mut self,
        key: EmbeddingCacheKey,
        bytes: u64,
        value: V,
    ) -> Vec<(EmbeddingCacheKey, V)> {
        let tick = self.take_tick();
        let _absent: Option<LedgerSlot<V>> =
            self.by_key.insert(key, LedgerSlot { tick, bytes, value });
        let _displaced: Option<EmbeddingCacheKey> = self.by_tick.insert(tick, key);
        self.resident_bytes = self.resident_bytes.saturating_add(bytes);
        self.evict_to_policy()
    }

    /// Drop `key` without counting an eviction; returns its value.
    fn remove(&mut self, key: &EmbeddingCacheKey) -> Option<V> {
        let slot = self.by_key.remove(key)?;
        let _removed: Option<EmbeddingCacheKey> = self.by_tick.remove(&slot.tick);
        self.resident_bytes = self.resident_bytes.saturating_sub(slot.bytes);
        Some(slot.value)
    }

    /// Evict least recently used entries until both ceilings hold.
    fn evict_to_policy(&mut self) -> Vec<(EmbeddingCacheKey, V)> {
        let mut evicted = Vec::new();
        while self.over_policy() {
            let Some((_tick, key)) = self.by_tick.pop_first() else {
                break;
            };
            let Some(slot) = self.by_key.remove(&key) else {
                continue;
            };
            self.resident_bytes = self.resident_bytes.saturating_sub(slot.bytes);
            self.stats.evictions = self.stats.evictions.saturating_add(1);
            evicted.push((key, slot.value));
        }
        evicted
    }

    fn over_policy(&self) -> bool {
        let entries = count_u64(self.by_key.len());
        entries > self.policy.entries || self.resident_bytes > self.policy.resident_bytes
    }

    fn stats(&self) -> EmbeddingCacheStats {
        EmbeddingCacheStats {
            entries: count_u64(self.by_key.len()),
            resident_bytes: self.resident_bytes,
            ..self.stats
        }
    }
}

/// The outcome of one ledger insert; a refused insert evicts nothing and
/// is counted on the ledger.
struct LedgerInsert<V> {
    /// The value previously under the same key, if any.
    displaced: Option<V>,
    /// Entries evicted to stay within the policy, oldest first.
    evicted: Vec<(EmbeddingCacheKey, V)>,
}

/// In-memory store bounded by a retention policy (process-lifetime; used
/// in tests and as a non-persistent fallback).
pub struct InMemoryEmbeddingCache {
    ledger: Mutex<RetentionLedger<Vec<f32>>>,
}

impl Default for InMemoryEmbeddingCache {
    fn default() -> Self {
        Self::bounded(EmbeddingCacheRetentionPolicy::DEFAULT)
    }
}

impl InMemoryEmbeddingCache {
    /// A store that holds at most what `policy` allows; the namespace
    /// ceiling does not apply to a store with one namespace.
    #[must_use]
    pub fn bounded(policy: EmbeddingCacheRetentionPolicy) -> Self {
        Self {
            ledger: Mutex::new(RetentionLedger::new(policy)),
        }
    }
}

fn vector_bytes(vector: &[f32]) -> u64 {
    count_u64(vector.len().saturating_mul(FLOAT_BYTES))
}

impl EmbeddingCache for InMemoryEmbeddingCache {
    fn get(&self, key: &EmbeddingCacheKey) -> Option<Vec<f32>> {
        // A poisoned lock degrades to a miss (forces recompute), never a wrong hit.
        match self.ledger.lock() {
            Ok(mut guard) => guard.touch(key).cloned(),
            Err(_poisoned) => None,
        }
    }

    fn put(&self, key: &EmbeddingCacheKey, vector: &[f32]) {
        if let Ok(mut guard) = self.ledger.lock() {
            let _outcome: LedgerInsert<Vec<f32>> =
                guard.insert(*key, vector_bytes(vector), vector.to_vec());
        }
    }

    fn evict(&self, key: &EmbeddingCacheKey) {
        if let Ok(mut guard) = self.ledger.lock() {
            drop(guard.remove(key));
        }
    }

    fn stats(&self) -> EmbeddingCacheStats {
        match self.ledger.lock() {
            Ok(guard) => guard.stats(),
            Err(_poisoned) => EmbeddingCacheStats::default(),
        }
    }
}

/// Number of leading hex characters of the key used as a shard
/// subdirectory.
///
/// Two hex chars = 256 buckets, keeping any single directory's fan-out
/// ~1/256th of the corpus so directory operations stay fast.
const SHARD_PREFIX_LEN: usize = 2;
/// Hex characters of a pre-namespace (format v1) shard directory directly
/// under the cache root, which no reader of this format ever opens.
const LEGACY_SHARD_LEN: usize = 2;
/// Suffix of one entry's file name.
const ENTRY_SUFFIX: &str = ".vec";
/// Marker written at open so namespaces can be retired by last use.
const OPENED_MARKER: &str = ".opened";
/// Infix every temporary write carries; a survivor is an interrupted write.
const STAGING_INFIX: &str = ".tmp-";

/// Magic prefix of a persisted entry.
const ENTRY_MAGIC: &[u8; 4] = b"QIEC";
/// Entry format this crate writes and reads.
const ENTRY_FORMAT_VERSION: u16 = 2;
/// Bytes of the truncated SHA-256 that close every entry.
const ENTRY_DIGEST_LEN: usize = 16;
const ENTRY_HEADER_LEN: usize = ENTRY_MAGIC.len() + 2 + 4;

static ATOMIC_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Durable file cache: one verified `<key>.vec` per entry under
/// `root/<namespace>/<key[..2]>/<key>.vec`, bounded by a retention policy.
///
/// The namespace directory is the identity's ([`EmbeddingCacheIdentityV1::
/// namespace`]), so a revision rotation starts an empty namespace and leaves
/// the previous one whole. Opening a namespace rebuilds its ledger from the
/// directory (recency is the entries' write order), removes the temporary
/// files of interrupted writes, reclaims pre-namespace shard directories,
/// and retires the least recently opened other namespaces beyond the
/// policy's ceiling.
pub struct FileEmbeddingCache {
    root: PathBuf,
    ledger: Mutex<RetentionLedger<()>>,
    open_report: EmbeddingCacheOpenReport,
}

/// One entry found while rebuilding a namespace ledger.
struct ScannedEntry {
    modified: SystemTime,
    key: EmbeddingCacheKey,
    bytes: u64,
}

impl FileEmbeddingCache {
    /// Open the cache for `identity` at `root/<namespace>` under `policy`,
    /// creating the directory if needed.
    pub fn new(
        root: &Path,
        identity: &EmbeddingCacheIdentityV1,
        policy: EmbeddingCacheRetentionPolicy,
    ) -> Result<Self, CoreError> {
        let namespace = identity.namespace();
        let namespace_dir = root.join(&namespace);
        std::fs::create_dir_all(&namespace_dir).map_err(|err| {
            CoreError::Storage(format!(
                "embedding cache: create dir {} failed: {err}",
                namespace_dir.display()
            ))
        })?;
        let reclaimed_legacy_directories = reclaim_legacy_shards(root)?;
        let retired_namespaces = retire_other_namespaces(root, &namespace, policy.namespaces)?;
        let (mut entries, stale_staging_removed) = scan_namespace(&namespace_dir)?;
        let scanned_entries = count_u64(entries.len());
        write_atomic(
            &namespace_dir.join(OPENED_MARKER),
            unix_seconds_now().to_string().as_bytes(),
        )
        .map_err(|err| storage_error("write open marker in", &namespace_dir, &err))?;
        let mut ledger = RetentionLedger::new(policy);
        // Oldest write first, so the ledger's recency is the directory's.
        entries.sort_by_key(|entry| entry.modified);
        let mut evicted = Vec::new();
        for entry in entries {
            evicted.extend(
                ledger
                    .seed(entry.key, entry.bytes, ())
                    .into_iter()
                    .map(|(key, ())| key),
            );
        }
        // A tightened policy evicts on open: the directory held more than
        // the policy now allows.
        let evicted_at_open = count_u64(evicted.len());
        for key in evicted {
            remove_entry_file(&entry_path(&namespace_dir, &key));
        }
        Ok(Self {
            root: namespace_dir,
            ledger: Mutex::new(ledger),
            open_report: EmbeddingCacheOpenReport {
                scanned_entries,
                evicted_at_open,
                stale_staging_removed,
                retired_namespaces,
                reclaimed_legacy_directories,
            },
        })
    }

    /// What opening this namespace found and removed.
    #[must_use]
    pub const fn open_report(&self) -> &EmbeddingCacheOpenReport {
        &self.open_report
    }

    /// The namespace directory entries are written under.
    #[must_use]
    pub fn namespace_dir(&self) -> &Path {
        &self.root
    }

    /// Sharded path `root/<key[..2]>/<key>.vec`.
    fn path_for(&self, key: &EmbeddingCacheKey) -> PathBuf {
        entry_path(&self.root, key)
    }

    fn lock_ledger(&self) -> Option<std::sync::MutexGuard<'_, RetentionLedger<()>>> {
        // A poisoned ledger cannot be trusted for eviction accounting; the
        // store degrades to misses (forced recompute), never to wrong hits.
        let Ok(guard) = self.ledger.lock() else {
            return None;
        };
        Some(guard)
    }
}

fn entry_path(namespace_dir: &Path, key: &EmbeddingCacheKey) -> PathBuf {
    let hex = key.hex();
    // A key's hex is 64 characters by construction; the prefix always exists.
    let (shard, _rest) = hex.split_at(SHARD_PREFIX_LEN);
    namespace_dir
        .join(shard)
        .join(format!("{hex}{ENTRY_SUFFIX}"))
}

fn remove_entry_file(path: &Path) {
    // Best-effort: an entry that is already gone is the state wanted.
    drop(std::fs::remove_file(path));
}

fn unix_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Whether `name` is exactly `len` lowercase hex digits, as every
/// directory this module creates is named.
fn is_hex_name(name: &str, len: usize) -> bool {
    name.len() == len
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn storage_error(action: &str, path: &Path, err: &std::io::Error) -> CoreError {
    CoreError::Storage(format!(
        "embedding cache: {action} {} failed: {err}",
        path.display()
    ))
}

/// Remove pre-namespace (format v1) shard directories directly under the
/// cache root. No reader of this format opens them; they are dead bytes.
fn reclaim_legacy_shards(root: &Path) -> Result<u64, CoreError> {
    let mut reclaimed = 0_u64;
    for entry in std::fs::read_dir(root).map_err(|err| storage_error("list", root, &err))? {
        let entry = entry.map_err(|err| storage_error("list", root, &err))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_hex_name(name, LEGACY_SHARD_LEN) || !entry.path().is_dir() {
            continue;
        }
        let path = entry.path();
        std::fs::remove_dir_all(&path).map_err(|err| storage_error("remove", &path, &err))?;
        reclaimed = reclaimed.saturating_add(1);
    }
    Ok(reclaimed)
}

/// Keep the current namespace and the `max_namespaces - 1` most recently
/// opened others; remove the rest.
fn retire_other_namespaces(
    root: &Path,
    current: &str,
    max_namespaces: usize,
) -> Result<u64, CoreError> {
    let mut others: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|err| storage_error("list", root, &err))? {
        let entry = entry.map_err(|err| storage_error("list", root, &err))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == current || !is_hex_name(name, NAMESPACE_LEN) || !entry.path().is_dir() {
            continue;
        }
        let path = entry.path();
        let marker = path.join(OPENED_MARKER);
        // A namespace never opened under this format has no claim to
        // recency: it sorts before every namespace that was.
        let last_opened = if marker.is_file() {
            std::fs::metadata(&marker)
                .and_then(|metadata| metadata.modified())
                .map_err(|err| storage_error("stat", &marker, &err))?
        } else {
            SystemTime::UNIX_EPOCH
        };
        others.push((last_opened, path));
    }
    // Most recently opened first; the current namespace takes one slot.
    others.sort_by(|left, right| right.0.cmp(&left.0));
    let mut retired = 0_u64;
    for (_opened, path) in others.into_iter().skip(max_namespaces.saturating_sub(1)) {
        std::fs::remove_dir_all(&path).map_err(|err| storage_error("remove", &path, &err))?;
        retired = retired.saturating_add(1);
    }
    Ok(retired)
}

/// Read every entry of a namespace into the report and remove the
/// temporary files of interrupted writes.
fn scan_namespace(namespace_dir: &Path) -> Result<(Vec<ScannedEntry>, u64), CoreError> {
    let mut entries = Vec::new();
    let mut stale_staging_removed = 0_u64;
    let children = std::fs::read_dir(namespace_dir)
        .map_err(|err| storage_error("list", namespace_dir, &err))?;
    for child in children {
        let child = child.map_err(|err| storage_error("list", namespace_dir, &err))?;
        let child_name = child.file_name();
        let Some(child_name) = child_name.to_str() else {
            continue;
        };
        let child_path = child.path();
        if child_path.is_file() && child_name.contains(STAGING_INFIX) {
            std::fs::remove_file(&child_path)
                .map_err(|err| storage_error("remove", &child_path, &err))?;
            stale_staging_removed = stale_staging_removed.saturating_add(1);
            continue;
        }
        if !is_hex_name(child_name, SHARD_PREFIX_LEN) || !child_path.is_dir() {
            continue;
        }
        let files = std::fs::read_dir(&child_path)
            .map_err(|err| storage_error("list", &child_path, &err))?;
        for file in files {
            let file = file.map_err(|err| storage_error("list", &child_path, &err))?;
            let file_name = file.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let path = file.path();
            if file_name.contains(STAGING_INFIX) {
                std::fs::remove_file(&path).map_err(|err| storage_error("remove", &path, &err))?;
                stale_staging_removed = stale_staging_removed.saturating_add(1);
                continue;
            }
            let Some(key) = file_name
                .strip_suffix(ENTRY_SUFFIX)
                .and_then(EmbeddingCacheKey::from_hex)
            else {
                continue;
            };
            let metadata =
                std::fs::metadata(&path).map_err(|err| storage_error("stat", &path, &err))?;
            let modified = metadata
                .modified()
                .map_err(|err| storage_error("stat", &path, &err))?;
            entries.push(ScannedEntry {
                modified,
                key,
                bytes: metadata.len(),
            });
        }
    }
    Ok((entries, stale_staging_removed))
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
        ".{}{STAGING_INFIX}{}-{sequence}",
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

impl EmbeddingCache for FileEmbeddingCache {
    fn get(&self, key: &EmbeddingCacheKey) -> Option<Vec<f32>> {
        {
            let mut ledger = self.lock_ledger()?;
            // The ledger is authoritative for what this process holds: an
            // absent key is a miss without a disk probe.
            let _resident: &() = ledger.touch(key)?;
        }
        let path = self.path_for(key);
        // Read outside the lock: a concurrent eviction between the touch and
        // the read leaves an absent file, which is a miss.
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_absent_or_unreadable) => return None,
        };
        let decoded = decode_entry(&bytes);
        if decoded.is_none() {
            // A file that exists but does not decode as an entry of this
            // format is not a miss that will heal itself: stop accounting
            // it and remove it, under the lock so a concurrent write of
            // the same key cannot slip a fresh entry between the two.
            if let Some(mut ledger) = self.lock_ledger() {
                let _slot: Option<()> = ledger.remove(key);
                ledger.stats.corrupt_misses = ledger.stats.corrupt_misses.saturating_add(1);
                remove_entry_file(&path);
            }
        }
        decoded
    }

    fn put(&self, key: &EmbeddingCacheKey, vector: &[f32]) {
        // Best-effort: any failure only forces a recompute next time. The
        // returned vector is already valid, so caching must not gate embedding.
        let Some(bytes) = encode_entry(vector) else {
            return;
        };
        let entry_bytes = count_u64(bytes.len());
        let path = self.path_for(key);
        {
            // An entry the policy can never hold is not written at all; a
            // previous entry under the key is dropped with it.
            let Some(mut ledger) = self.lock_ledger() else {
                return;
            };
            if !ledger.admits(entry_bytes) {
                let refused = ledger.insert(*key, entry_bytes, ());
                drop(ledger);
                if refused.displaced.is_some() {
                    remove_entry_file(&path);
                }
                return;
            }
        }
        if write_atomic(&path, &bytes).is_err() {
            return;
        }
        // Account the entry as it is on disk now, so two writers of one key
        // agree on its bytes whichever renamed last, and evict under the
        // same lock so the ceiling holds at every point a reader can see.
        let Ok(on_disk) = std::fs::metadata(&path) else {
            return;
        };
        let Some(mut ledger) = self.lock_ledger() else {
            return;
        };
        let outcome = ledger.insert(*key, on_disk.len(), ());
        for (evicted, ()) in outcome.evicted {
            remove_entry_file(&self.path_for(&evicted));
        }
    }

    fn evict(&self, key: &EmbeddingCacheKey) {
        // Slot first, file second, both under the lock: a concurrent write
        // of the same key then either precedes this and is removed with
        // it, or follows it and is accounted whole.
        if let Some(mut ledger) = self.lock_ledger() {
            let _slot: Option<()> = ledger.remove(key);
            remove_entry_file(&self.path_for(key));
        }
    }

    fn stats(&self) -> EmbeddingCacheStats {
        self.lock_ledger()
            .map_or_else(EmbeddingCacheStats::default, |ledger| ledger.stats())
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
    use std::time::Duration;

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

    fn key(text: &str) -> EmbeddingCacheKey {
        identity("m", "r").key(text)
    }

    /// A small policy: `entries` entries, `bytes` bytes, one namespace.
    fn policy(entries: u64, bytes: u64) -> EmbeddingCacheRetentionPolicy {
        EmbeddingCacheRetentionPolicy::new(entries, bytes, 1).expect("policy")
    }

    fn file_cache(root: &Path, policy: EmbeddingCacheRetentionPolicy) -> FileEmbeddingCache {
        FileEmbeddingCache::new(root, &identity("m", "r"), policy).expect("file cache")
    }

    /// Every `.vec` entry under a namespace, with its bytes, read from disk.
    fn on_disk_entries(namespace_dir: &Path) -> BTreeMap<String, u64> {
        let mut found = BTreeMap::new();
        for shard in std::fs::read_dir(namespace_dir).expect("list namespace") {
            let shard = shard.expect("shard");
            if !shard.path().is_dir() {
                continue;
            }
            for file in std::fs::read_dir(shard.path()).expect("list shard") {
                let file = file.expect("file");
                let name = file.file_name().to_string_lossy().to_string();
                if let Some(stem) = name.strip_suffix(ENTRY_SUFFIX) {
                    let len = file.metadata().expect("metadata").len();
                    assert!(found.insert(stem.to_string(), len).is_none());
                }
            }
        }
        found
    }

    fn assert_ledger_matches_disk(cache: &FileEmbeddingCache) {
        let disk = on_disk_entries(cache.namespace_dir());
        let stats = cache.stats();
        assert_eq!(
            stats.entries,
            u64::try_from(disk.len()).expect("fits"),
            "ledger entries must equal the entries on disk"
        );
        assert_eq!(
            stats.resident_bytes,
            disk.values().sum::<u64>(),
            "ledger bytes must equal the bytes on disk"
        );
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
        let stats = provider.cache_stats();
        assert_eq!((stats.hits, stats.misses, stats.puts), (2, 2, 2));
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
        fn get(&self, key: &EmbeddingCacheKey) -> Option<Vec<f32>> {
            self.0.get(key)
        }
        fn put(&self, key: &EmbeddingCacheKey, vector: &[f32]) {
            self.0.put(key, vector);
        }
        fn evict(&self, key: &EmbeddingCacheKey) {
            self.0.evict(key);
        }
        fn stats(&self) -> EmbeddingCacheStats {
            self.0.stats()
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
    fn a_key_round_trips_through_its_hex_and_only_lowercase_parses() {
        let key = key("round trip");
        let hex = key.hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(EmbeddingCacheKey::from_hex(&hex), Some(key));
        assert_eq!(EmbeddingCacheKey::from_hex(&hex.to_uppercase()), None);
        let (short, _last) = hex.split_at(63);
        assert_eq!(EmbeddingCacheKey::from_hex(short), None);
        assert_eq!(EmbeddingCacheKey::from_hex(&format!("{hex}0")), None);
        assert_eq!(EmbeddingCacheKey::from_hex(&hex.replace('a', "g")), None);
    }

    #[test]
    fn file_cache_round_trips_vectors_under_the_identity_namespace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("embed-cache");
        let identity = identity("m", "r");
        let cache = file_cache(&root, EmbeddingCacheRetentionPolicy::DEFAULT);
        assert_eq!(cache.namespace_dir(), root.join(identity.namespace()));
        assert!(cache.get(&key("missing")).is_none());
        let key = key("present");
        cache.put(&key, &[1.0, -0.5]);
        let hex = key.hex();
        let (shard, _rest) = hex.split_at(2);
        let sharded = root
            .join(identity.namespace())
            .join(shard)
            .join(format!("{hex}.vec"));
        assert!(
            sharded.exists(),
            "entry must be written under {}",
            sharded.display()
        );
        assert_eq!(cache.get(&key), Some(vec![1.0, -0.5]));
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
        assert_ledger_matches_disk(&cache);
    }

    type Damage = Box<dyn Fn(&[u8]) -> Vec<u8>>;

    /// Every way a persisted entry can be damaged reads as a miss, and the
    /// damaged file is removed so it is not read again.
    #[test]
    fn file_cache_refuses_and_removes_damaged_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = file_cache(
            &dir.path().join("embed-cache"),
            EmbeddingCacheRetentionPolicy::DEFAULT,
        );
        let key = key("damaged");
        let path = cache.path_for(&key);
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
        let case_count = u64::try_from(cases.len()).expect("fits");
        for (label, damage) in cases {
            cache.put(&key, &[0.6, 0.8]);
            let good = std::fs::read(&path).expect("entry written");
            std::fs::write(&path, damage(&good)).expect("damage");
            assert!(
                cache.get(&key).is_none(),
                "{label}: damaged entry must miss"
            );
            assert!(!path.exists(), "{label}: damaged entry must be removed");
            assert_ledger_matches_disk(&cache);
        }
        assert_eq!(cache.stats().corrupt_misses, case_count);
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
        let cache = Arc::new(file_cache(
            &dir.path().join("embed-cache"),
            EmbeddingCacheRetentionPolicy::DEFAULT,
        ));
        let key = key("contended");
        let writers = (0..8_u8)
            .map(|index| {
                let cache = Arc::clone(&cache);
                std::thread::spawn(move || {
                    let value = 0.1_f32 * (f32::from(index) + 1.0);
                    for _ in 0..50 {
                        cache.put(&key, &[value, 1.0 - value]);
                        let read = cache.get(&key).expect("an entry is always readable");
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
        assert!(cache.get(&key).is_some());
        assert_ledger_matches_disk(&cache);
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
                EmbeddingCacheRetentionPolicy::DEFAULT,
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

    /// Bytes of one two-component entry on disk.
    fn entry_len() -> u64 {
        u64::try_from(encode_entry(&[0.0, 1.0]).expect("encode").len()).expect("fits")
    }

    /// Under a policy of three entries' bytes, writing eight leaves the
    /// three newest resident, on disk and in the ledger, and the ledger
    /// never reports more than the policy at any point.
    #[test]
    fn resident_bytes_and_entries_never_exceed_the_policy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bound = policy(64, entry_len() * 3);
        let cache = file_cache(&dir.path().join("embed-cache"), bound);
        let keys: Vec<EmbeddingCacheKey> = (0..8).map(|i| key(&format!("entry-{i}"))).collect();
        for key in &keys {
            cache.put(key, &[0.0, 1.0]);
            let stats = cache.stats();
            assert!(
                stats.resident_bytes <= bound.max_resident_bytes(),
                "resident bytes {} exceed the policy {}",
                stats.resident_bytes,
                bound.max_resident_bytes()
            );
            assert_ledger_matches_disk(&cache);
        }
        let stats = cache.stats();
        assert_eq!((stats.entries, stats.evictions, stats.puts), (3, 5, 8));
        for (index, key) in keys.iter().enumerate() {
            let expected_resident = index >= 5;
            assert_eq!(
                cache.get(key).is_some(),
                expected_resident,
                "entry {index} residency drifted"
            );
        }
        // The entry ceiling binds on its own too.
        let cache = file_cache(&dir.path().join("by-count"), policy(2, u64::MAX));
        for key in &keys {
            cache.put(key, &[0.0, 1.0]);
        }
        assert_eq!(cache.stats().entries, 2);
        assert_ledger_matches_disk(&cache);
    }

    /// A hit refreshes recency: the entry read most recently survives an
    /// eviction that removes an entry written after it.
    #[test]
    fn a_hit_makes_an_entry_the_newest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = file_cache(&dir.path().join("embed-cache"), policy(2, u64::MAX));
        let (first, second, third) = (key("first"), key("second"), key("third"));
        cache.put(&first, &[0.0, 1.0]);
        cache.put(&second, &[0.0, 1.0]);
        assert!(cache.get(&first).is_some(), "first is resident");
        cache.put(&third, &[0.0, 1.0]);
        assert!(
            cache.get(&first).is_some(),
            "the recently read entry survives"
        );
        assert!(
            cache.get(&second).is_none(),
            "the least recently used entry was evicted"
        );
        assert!(cache.get(&third).is_some());
        assert_ledger_matches_disk(&cache);
    }

    /// An entry that alone exceeds the byte ceiling is refused, not written,
    /// and a previous entry under its key is dropped with it.
    #[test]
    fn an_oversize_entry_is_refused_and_never_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = file_cache(&dir.path().join("embed-cache"), policy(64, entry_len()));
        let key = key("oversize");
        cache.put(&key, &[0.0, 1.0]);
        assert!(cache.get(&key).is_some());
        cache.put(&key, &[0.0, 1.0, 0.0]);
        assert!(cache.get(&key).is_none(), "an oversize write drops the key");
        assert!(!cache.path_for(&key).exists());
        let stats = cache.stats();
        assert_eq!((stats.refused_oversize, stats.entries), (1, 0));
        assert_ledger_matches_disk(&cache);
    }

    /// The accounting survives a restart: reopening rebuilds the ledger from
    /// the directory, and reopening under a tighter policy evicts the oldest
    /// writes first.
    #[test]
    fn reopening_rebuilds_the_ledger_and_a_tighter_policy_evicts_the_oldest_writes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("embed-cache");
        let keys: Vec<EmbeddingCacheKey> = (0..6).map(|i| key(&format!("durable-{i}"))).collect();
        {
            let cache = file_cache(&root, policy(64, u64::MAX));
            for (index, key) in keys.iter().enumerate() {
                cache.put(key, &[0.0, 1.0]);
                // Pin the write order into the file times, independent of
                // the filesystem's timestamp granularity.
                let stamp = SystemTime::UNIX_EPOCH
                    + Duration::from_secs(1_700_000_000 + u64::try_from(index).expect("fits"));
                std::fs::File::open(cache.path_for(key))
                    .expect("entry")
                    .set_modified(stamp)
                    .expect("set modified");
            }
        }
        let reopened = file_cache(&root, policy(64, u64::MAX));
        let stats = reopened.stats();
        assert_eq!((stats.entries, stats.resident_bytes), (6, entry_len() * 6));
        assert_eq!(reopened.open_report().scanned_entries, 6);
        assert_eq!(reopened.open_report().evicted_at_open, 0);
        assert_ledger_matches_disk(&reopened);
        drop(reopened);

        let tightened = file_cache(&root, policy(2, u64::MAX));
        assert_eq!(tightened.open_report().evicted_at_open, 4);
        assert_eq!(tightened.stats().entries, 2);
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(
                tightened.path_for(key).exists(),
                index >= 4,
                "entry {index}: only the two newest writes survive a tighter policy"
            );
        }
        assert_ledger_matches_disk(&tightened);
    }

    /// Opening removes the temporary files of interrupted writes, reclaims
    /// pre-namespace shard directories, and retires the least recently
    /// opened other namespaces beyond the policy.
    #[test]
    fn opening_removes_stale_staging_legacy_shards_and_surplus_namespaces() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("embed-cache");
        // Three other identities opened in order under a generous policy.
        let identities: Vec<EmbeddingCacheIdentityV1> =
            (0..3).map(|i| identity("m", &format!("old-{i}"))).collect();
        for (index, identity) in identities.iter().enumerate() {
            let cache = FileEmbeddingCache::new(
                &root,
                identity,
                EmbeddingCacheRetentionPolicy::new(64, u64::MAX, 8).expect("policy"),
            )
            .expect("cache");
            cache.put(&identity.key("warm"), &[0.0, 1.0]);
            let stamp = SystemTime::UNIX_EPOCH
                + Duration::from_secs(1_700_000_000 + u64::try_from(index).expect("fits"));
            std::fs::File::open(cache.namespace_dir().join(OPENED_MARKER))
                .expect("marker")
                .set_modified(stamp)
                .expect("set modified");
        }
        // A pre-namespace shard directory with an entry nobody reads, left
        // behind by the format before namespaces.
        std::fs::create_dir_all(root.join("ab")).expect("legacy shard");
        std::fs::write(root.join("ab").join("abcd.vec"), b"legacy").expect("legacy entry");
        // A non-hex directory is not ours and is left alone.
        std::fs::create_dir_all(root.join("keep-me")).expect("foreign dir");
        // An interrupted write in the current namespace.
        let current = identity("m", "current");
        let shard = root.join(current.namespace()).join("00");
        std::fs::create_dir_all(&shard).expect("shard");
        std::fs::write(shard.join(".x.vec.tmp-1-1"), b"partial").expect("staging");
        std::fs::write(
            root.join(current.namespace()).join("..opened.tmp-1-2"),
            b"partial",
        )
        .expect("staging marker");

        // Room for the current namespace and two others.
        let cache = FileEmbeddingCache::new(
            &root,
            &current,
            EmbeddingCacheRetentionPolicy::new(64, u64::MAX, 3).expect("policy"),
        )
        .expect("cache");
        let report = cache.open_report();
        assert_eq!(report.reclaimed_legacy_directories, 1);
        assert_eq!(report.retired_namespaces, 1);
        assert_eq!(report.stale_staging_removed, 2);
        assert!(!root.join("ab").exists(), "legacy shard reclaimed");
        assert!(root.join("keep-me").is_dir(), "foreign directory untouched");
        assert!(
            !root.join(identities[0].namespace()).exists(),
            "the least recently opened namespace is retired"
        );
        assert!(root.join(identities[1].namespace()).is_dir());
        assert!(root.join(identities[2].namespace()).is_dir());
        assert!(root.join(current.namespace()).join(OPENED_MARKER).is_file());
        assert!(!shard.join(".x.vec.tmp-1-1").exists());
        assert_ledger_matches_disk(&cache);
    }

    /// Eviction racing reads never serves a wrong or torn vector.
    ///
    /// Under a tiny ceiling with writers and readers racing, a reader is
    /// never served a vector that is not the one written under its key,
    /// and once the writers stop, exactly the policy's worth of entries is
    /// resident, served correctly, and equal to the directory.
    #[test]
    fn concurrent_eviction_never_serves_a_wrong_or_torn_vector() {
        use std::sync::atomic::AtomicBool;

        let dir = tempfile::tempdir().expect("tempdir");
        let cache = Arc::new(file_cache(
            &dir.path().join("embed-cache"),
            policy(4, u64::MAX),
        ));
        let expected = |index: u8| -> Vec<f32> {
            let x = f32::from(index) / 32.0;
            vec![x, (1.0 - x * x).sqrt()]
        };
        let keys: Arc<Vec<EmbeddingCacheKey>> =
            Arc::new((0..16_u8).map(|i| key(&format!("race-{i}"))).collect());
        let writers_done = Arc::new(AtomicBool::new(false));
        let readers = (0..4_u8)
            .map(|worker| {
                let cache = Arc::clone(&cache);
                let keys = Arc::clone(&keys);
                let writers_done = Arc::clone(&writers_done);
                std::thread::spawn(move || {
                    let mut served = 0_u32;
                    let mut round = 0_u8;
                    while !writers_done.load(Ordering::Acquire) {
                        let index = (worker.wrapping_mul(7).wrapping_add(round)) % 16;
                        if let Some(vector) = cache.get(&keys[usize::from(index)]) {
                            assert_eq!(vector, expected(index), "wrong vector for {index}");
                            served += 1;
                        }
                        round = round.wrapping_add(1);
                    }
                    served
                })
            })
            .collect::<Vec<_>>();
        let writers = (0..4_u8)
            .map(|worker| {
                let cache = Arc::clone(&cache);
                let keys = Arc::clone(&keys);
                std::thread::spawn(move || {
                    for round in 0..40_u8 {
                        let index = (worker.wrapping_mul(4).wrapping_add(round)) % 16;
                        cache.put(&keys[usize::from(index)], &expected(index));
                    }
                })
            })
            .collect::<Vec<_>>();
        for writer in writers {
            writer.join().expect("writer");
        }
        writers_done.store(true, Ordering::Release);
        // A reader's count is not asserted — how many hits the race yields
        // is scheduling — only that every hit it did see was right.
        for reader in readers {
            let _served_while_racing: u32 = reader.join().expect("reader");
        }
        let stats = cache.stats();
        assert_eq!(stats.entries, 4, "exactly the policy's worth is resident");
        assert!(stats.evictions > 0, "the race must have evicted");
        let served_after: usize = (0..16_u8)
            .filter(|&index| {
                cache
                    .get(&keys[usize::from(index)])
                    .inspect(|vector| assert_eq!(*vector, expected(index)))
                    .is_some()
            })
            .count();
        assert_eq!(served_after, 4, "every resident entry is served");
        assert_ledger_matches_disk(&cache);
    }

    /// The in-memory store honors the same bound.
    #[test]
    fn in_memory_cache_is_bounded_by_entries_and_bytes() {
        let by_entries = InMemoryEmbeddingCache::bounded(policy(2, u64::MAX));
        for i in 0..5 {
            by_entries.put(&key(&format!("m-{i}")), &[0.0, 1.0]);
        }
        let stats = by_entries.stats();
        assert_eq!((stats.entries, stats.evictions), (2, 3));
        assert!(by_entries.get(&key("m-4")).is_some());
        assert!(by_entries.get(&key("m-0")).is_none());

        let by_bytes = InMemoryEmbeddingCache::bounded(policy(64, 8));
        by_bytes.put(&key("b-0"), &[0.0, 1.0]);
        by_bytes.put(&key("b-1"), &[0.0, 1.0]);
        by_bytes.put(&key("b-2"), &[0.0, 1.0, 0.0]);
        let stats = by_bytes.stats();
        assert_eq!(stats.refused_oversize, 1);
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.resident_bytes, 8);
        assert!(by_bytes.get(&key("b-1")).is_some());
    }

    #[test]
    fn zero_ceilings_are_refused_at_construction() {
        assert!(EmbeddingCacheRetentionPolicy::new(0, 1, 1).is_err());
        assert!(EmbeddingCacheRetentionPolicy::new(1, 0, 1).is_err());
        assert!(EmbeddingCacheRetentionPolicy::new(1, 1, 0).is_err());
        assert!(EmbeddingCacheRetentionPolicy::new(1, 1, 1).is_ok());
    }
}
