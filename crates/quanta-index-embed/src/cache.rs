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
//! namespace holds at most so many entries and so many bytes, none older
//! than the age cap, the least recently used entries are evicted to stay
//! within the first two and expired entries are removed for the third,
//! the cache root holds at most so many namespaces, and every namespace
//! together stays under one total-bytes ceiling. The bound is enforced by
//! an in-process ledger that is rebuilt at open from the namespace's
//! persisted manifest and its directory listing (see [`manifest`]), so a
//! restart neither loses the accounting nor trusts a stale one, and an
//! eviction only ever removes whole entries — a concurrent reader sees the
//! entry or a miss, never a torn file.

mod manifest;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use quanta_index_contract::EmbeddingNormalization;
use quanta_index_core::{
    CoreError, MetricPointV1, MetricSourcePort, RequestBudgetV1, SemanticPolicy,
    TextEmbeddingProvider,
};
use sha2::{Digest, Sha256};

use crate::cache::manifest::{
    MANIFEST_FILE, MANIFEST_FLUSH_EVERY_PUTS, ManifestRead, ManifestRecord, encode_manifest,
    read_manifest,
};
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

    /// The key's digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// A key from its digest bytes, as the manifest stores them.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
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

/// How much one cache namespace may hold, how old an entry may be, how
/// many namespaces a cache root may keep, and how much every namespace
/// under it may hold together.
///
/// Every bound is a strict maximum; zero is refused at construction because
/// a zero ceiling is a configuration defect, not a disabled cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmbeddingCacheRetentionPolicy {
    entries: u64,
    resident_bytes: u64,
    namespaces: usize,
    max_entry_age: Duration,
    max_total_bytes: u64,
}

impl EmbeddingCacheRetentionPolicy {
    /// Production defaults: 500,000 entries and 2 GiB per namespace, no
    /// entry older than thirty days, four namespaces per cache root (the
    /// current identity and the three most recently opened others, so an
    /// A/B rotation keeps both sides warm), and 4 GiB across them all.
    pub const DEFAULT: Self = Self {
        entries: 500_000,
        resident_bytes: 2 * 1024 * 1024 * 1024,
        namespaces: 4,
        max_entry_age: Duration::from_secs(30 * 24 * 60 * 60),
        max_total_bytes: 4 * 1024 * 1024 * 1024,
    };

    /// A policy with explicit ceilings; each must be at least one, and
    /// the total must hold at least one namespace's bytes.
    pub fn new(
        max_entries: u64,
        max_resident_bytes: u64,
        max_namespaces: usize,
        max_entry_age: Duration,
        max_total_bytes: u64,
    ) -> Result<Self, CoreError> {
        if max_entries == 0
            || max_resident_bytes == 0
            || max_namespaces == 0
            || max_entry_age.is_zero()
            || max_total_bytes == 0
        {
            return Err(CoreError::InvalidContract(
                "embedding cache retention policy: every ceiling must be at least one".to_string(),
            ));
        }
        if max_total_bytes < max_resident_bytes {
            return Err(CoreError::InvalidContract(format!(
                "embedding cache retention policy: the total ceiling {max_total_bytes} cannot hold one namespace of {max_resident_bytes} bytes"
            )));
        }
        Ok(Self {
            entries: max_entries,
            resident_bytes: max_resident_bytes,
            namespaces: max_namespaces,
            max_entry_age,
            max_total_bytes,
        })
    }

    /// Oldest an entry may be, measured from its write.
    #[must_use]
    pub const fn max_entry_age(&self) -> Duration {
        self.max_entry_age
    }

    /// Most bytes every namespace under one cache root holds together.
    #[must_use]
    pub const fn max_total_bytes(&self) -> u64 {
        self.max_total_bytes
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
    /// Entries removed to stay within the entry and byte ceilings.
    pub evictions: u64,
    /// Entries removed because they were written longer ago than the
    /// policy's age cap.
    pub expirations: u64,
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
    /// Entries expired at open because they were older than the age cap.
    pub expired_at_open: u64,
    /// Temporary files of interrupted writes removed.
    pub stale_staging_removed: u64,
    /// Other identities' namespaces removed to stay within the namespace
    /// ceiling or the total-bytes ceiling.
    pub retired_namespaces: u64,
    /// Entries removed from the other, retained namespaces to bring each
    /// within the per-namespace policy.
    pub trimmed_foreign_entries: u64,
    /// Bytes the other, retained namespaces hold after reconciliation.
    pub retained_foreign_bytes: u64,
    /// Pre-namespace (format v1) shard directories removed.
    pub reclaimed_legacy_directories: u64,
    /// Whether this namespace's manifest was present and decoded.
    pub manifest_present: bool,
    /// Whether a manifest was present but did not decode (ignored).
    pub manifest_malformed: bool,
    /// Entry files whose metadata had to be read at open because no
    /// manifest covered them, over this namespace and the retained others.
    pub stat_calls_at_open: u64,
    /// Manifest records of this namespace whose entry was gone from disk.
    pub manifest_stale_records: u64,
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
}

/// The wrapped store's residency and traffic as scrape points,
/// `embedding_cache_…` (QI-BB-015).
impl MetricSourcePort for CachingEmbeddingProvider {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let stats = self.cache_stats();
        Ok(vec![
            MetricPointV1::gauge_count("embedding_cache_entries", stats.entries),
            MetricPointV1::gauge_count("embedding_cache_resident_bytes", stats.resident_bytes),
            MetricPointV1::counter("embedding_cache_hits_total", stats.hits),
            MetricPointV1::counter("embedding_cache_misses_total", stats.misses),
            MetricPointV1::counter("embedding_cache_corrupt_misses_total", stats.corrupt_misses),
            MetricPointV1::counter("embedding_cache_puts_total", stats.puts),
            MetricPointV1::counter("embedding_cache_evictions_total", stats.evictions),
            MetricPointV1::counter("embedding_cache_expirations_total", stats.expirations),
            MetricPointV1::counter(
                "embedding_cache_refused_oversize_total",
                stats.refused_oversize,
            ),
        ])
    }
}

/// What opening the file store found, as scrape points,
/// `embedding_cache_open_…` (QI-BB-009): the bounded rebuild's cost and
/// what reconciliation removed, fixed at open.
impl MetricSourcePort for FileEmbeddingCache {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let report = self.open_report();
        Ok(vec![
            MetricPointV1::gauge_count(
                "embedding_cache_open_stat_calls",
                report.stat_calls_at_open,
            ),
            MetricPointV1::gauge_count(
                "embedding_cache_open_scanned_entries",
                report.scanned_entries,
            ),
            MetricPointV1::gauge_count("embedding_cache_open_expired", report.expired_at_open),
            MetricPointV1::gauge_count("embedding_cache_open_evicted", report.evicted_at_open),
            MetricPointV1::gauge_count(
                "embedding_cache_open_retired_namespaces",
                report.retired_namespaces,
            ),
            MetricPointV1::gauge_count(
                "embedding_cache_open_trimmed_foreign_entries",
                report.trimmed_foreign_entries,
            ),
            MetricPointV1::gauge_count(
                "embedding_cache_retained_foreign_bytes",
                report.retained_foreign_bytes,
            ),
            MetricPointV1::counter(
                "embedding_cache_manifest_flushes_total",
                self.manifest_flushes(),
            ),
        ])
    }
}

impl CachingEmbeddingProvider {
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
        self.embed_batch_within(texts, &RequestBudgetV1::unbounded())
    }

    /// Hits are served from the store; only the misses reach the inner
    /// provider, under the caller's budget (QI-BB-002).
    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<Vec<Vec<f32>>, CoreError> {
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
            let fresh = self.inner.embed_batch_within(&distinct_texts, budget)?;
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
    /// When the entry was written, in nanoseconds since the Unix epoch;
    /// the age cap is measured from here, not from the last read.
    written_nanos: u64,
    value: V,
}

/// Least-recently-used accounting shared by every store: which keys are
/// resident, how many bytes they occupy, when each was written, and which
/// to evict first.
///
/// Recency is a monotonic tick; a lookup or write moves the key to the
/// newest tick, and eviction pops the oldest. Age is the write time; an
/// entry past the policy's age cap is expired at the next open, write, or
/// lookup of it. Every map is `O(log n)` per operation so a namespace of
/// hundreds of thousands of entries costs the same per hit as one of ten.
struct RetentionLedger<V> {
    policy: EmbeddingCacheRetentionPolicy,
    by_key: BTreeMap<EmbeddingCacheKey, LedgerSlot<V>>,
    by_tick: BTreeMap<u64, EmbeddingCacheKey>,
    /// Oldest write first, for the age cap.
    by_written: BTreeSet<(u64, EmbeddingCacheKey)>,
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
            by_written: BTreeSet::new(),
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

    /// The oldest write time still admitted at `now_nanos`.
    fn oldest_admitted_nanos(&self, now_nanos: u64) -> u64 {
        let age_nanos =
            u64::try_from(self.policy.max_entry_age.as_nanos()).map_or(u64::MAX, |nanos| nanos);
        now_nanos.saturating_sub(age_nanos)
    }

    /// The value under `key`, made most recent; `None` counts a miss. An
    /// entry past the age cap is expired here — counted as an expiration
    /// and a miss, and returned so a file store can remove it.
    fn touch(&mut self, key: &EmbeddingCacheKey, now_nanos: u64) -> Touch<'_, V> {
        let tick = self.take_tick();
        let oldest_admitted = self.oldest_admitted_nanos(now_nanos);
        let Some(written_nanos) = self.by_key.get(key).map(|slot| slot.written_nanos) else {
            self.stats.misses = self.stats.misses.saturating_add(1);
            return Touch::Miss;
        };
        if written_nanos < oldest_admitted {
            let value = self.expire(key);
            self.stats.misses = self.stats.misses.saturating_add(1);
            return Touch::Expired(value);
        }
        let Some(slot) = self.by_key.get_mut(key) else {
            self.stats.misses = self.stats.misses.saturating_add(1);
            return Touch::Miss;
        };
        let _previous: Option<EmbeddingCacheKey> = self.by_tick.remove(&slot.tick);
        slot.tick = tick;
        let _displaced: Option<EmbeddingCacheKey> = self.by_tick.insert(tick, *key);
        self.stats.hits = self.stats.hits.saturating_add(1);
        Touch::Hit(&slot.value)
    }

    /// Whether one entry of `bytes` can ever be resident under the policy.
    const fn admits(&self, bytes: u64) -> bool {
        bytes <= self.policy.resident_bytes
    }

    /// Record `value` under `key` at `bytes`, written at `written_nanos`,
    /// replacing any previous slot, then expire and evict until the policy
    /// holds.
    ///
    /// Returns what was evicted, oldest first, so a file store can remove
    /// the files. An entry the policy does not admit is refused outright
    /// and counted as such; the previous slot under that key, if any, is
    /// dropped too because the store no longer holds it.
    fn insert(
        &mut self,
        key: EmbeddingCacheKey,
        bytes: u64,
        written_nanos: u64,
        value: V,
    ) -> LedgerInsert<V> {
        let displaced = self.remove(&key);
        if !self.admits(bytes) {
            self.stats.refused_oversize = self.stats.refused_oversize.saturating_add(1);
            return LedgerInsert {
                displaced,
                evicted: Vec::new(),
            };
        }
        self.stats.puts = self.stats.puts.saturating_add(1);
        let evicted = self.seed(key, bytes, written_nanos, value);
        LedgerInsert { displaced, evicted }
    }

    /// Record `value` under `key` at `bytes` as the newest entry without
    /// counting a write (a rebuild from disk is not traffic), then evict
    /// least recently used entries until the policy holds.
    fn seed(
        &mut self,
        key: EmbeddingCacheKey,
        bytes: u64,
        written_nanos: u64,
        value: V,
    ) -> Vec<(EmbeddingCacheKey, V)> {
        let tick = self.take_tick();
        let _absent: Option<LedgerSlot<V>> = self.by_key.insert(
            key,
            LedgerSlot {
                tick,
                bytes,
                written_nanos,
                value,
            },
        );
        let _displaced: Option<EmbeddingCacheKey> = self.by_tick.insert(tick, key);
        let _new: bool = self.by_written.insert((written_nanos, key));
        self.resident_bytes = self.resident_bytes.saturating_add(bytes);
        self.evict_to_policy()
    }

    /// Drop `key` without counting an eviction; returns its value.
    fn remove(&mut self, key: &EmbeddingCacheKey) -> Option<V> {
        let slot = self.by_key.remove(key)?;
        let _removed: Option<EmbeddingCacheKey> = self.by_tick.remove(&slot.tick);
        let _removed: bool = self.by_written.remove(&(slot.written_nanos, *key));
        self.resident_bytes = self.resident_bytes.saturating_sub(slot.bytes);
        Some(slot.value)
    }

    /// Drop `key` as expired, counting the expiration.
    fn expire(&mut self, key: &EmbeddingCacheKey) -> Option<V> {
        let value = self.remove(key)?;
        self.stats.expirations = self.stats.expirations.saturating_add(1);
        Some(value)
    }

    /// Expire every entry written before the age cap admits at `now_nanos`,
    /// oldest first.
    fn expire_to_policy(&mut self, now_nanos: u64) -> Vec<(EmbeddingCacheKey, V)> {
        let oldest_admitted = self.oldest_admitted_nanos(now_nanos);
        let mut expired = Vec::new();
        while let Some(&(written_nanos, key)) = self.by_written.first() {
            if written_nanos >= oldest_admitted {
                break;
            }
            match self.expire(&key) {
                Some(value) => expired.push((key, value)),
                None => {
                    // The maps are kept in lock-step; an orphan index entry
                    // is dropped rather than looped on.
                    let _orphan: bool = self.by_written.remove(&(written_nanos, key));
                }
            }
        }
        expired
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
            let _removed: bool = self.by_written.remove(&(slot.written_nanos, key));
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

    /// Every resident entry's accounting, for the manifest.
    fn manifest_records(&self) -> BTreeMap<EmbeddingCacheKey, ManifestRecord> {
        self.by_key
            .iter()
            .map(|(key, slot)| {
                (
                    *key,
                    ManifestRecord {
                        bytes: slot.bytes,
                        written_nanos: slot.written_nanos,
                    },
                )
            })
            .collect()
    }
}

/// The outcome of one ledger lookup.
enum Touch<'a, V> {
    Hit(&'a V),
    Miss,
    /// The entry was resident but past the age cap; it is gone from the
    /// ledger and its value is handed back so the store can remove it.
    Expired(Option<V>),
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
    /// A store that holds at most what `policy` allows; the namespace and
    /// total-bytes ceilings do not apply to a store with one namespace.
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
            Ok(mut guard) => match guard.touch(key, unix_nanos_now()) {
                Touch::Hit(vector) => Some(vector.clone()),
                Touch::Miss | Touch::Expired(_) => None,
            },
            Err(_poisoned) => None,
        }
    }

    fn put(&self, key: &EmbeddingCacheKey, vector: &[f32]) {
        if let Ok(mut guard) = self.ledger.lock() {
            let now = unix_nanos_now();
            let _expired: Vec<(EmbeddingCacheKey, Vec<f32>)> = guard.expire_to_policy(now);
            let _outcome: LedgerInsert<Vec<f32>> =
                guard.insert(*key, vector_bytes(vector), now, vector.to_vec());
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
/// the previous one whole. Opening a namespace rebuilds its ledger from its
/// manifest and the directory listing (`stat`-ing only what the manifest
/// does not cover), expires entries past the age cap, evicts down to a
/// tightened policy, removes the temporary files of interrupted writes,
/// reclaims pre-namespace shard directories, and reconciles the other
/// namespaces under the root: the least recently opened beyond the
/// namespace ceiling are removed, each retained one is trimmed to the
/// per-namespace policy, and whole namespaces are removed, least recently
/// opened first, until they leave room for this namespace's full ceiling
/// under the total-bytes ceiling.
pub struct FileEmbeddingCache {
    root: PathBuf,
    ledger: Mutex<RetentionLedger<()>>,
    open_report: EmbeddingCacheOpenReport,
    /// Writes since the manifest was last flushed.
    puts_since_flush: AtomicU64,
    /// Manifest flushes so far, for the scrape.
    manifest_flushes: AtomicU64,
}

/// One entry of a namespace as its ledger seeds it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ScannedEntry {
    key: EmbeddingCacheKey,
    bytes: u64,
    written_nanos: u64,
}

/// What loading one namespace from its manifest and directory found.
struct LoadedNamespace {
    entries: Vec<ScannedEntry>,
    stale_staging_removed: u64,
    manifest: ManifestOutcome,
    /// Entries the manifest did not cover, each of which cost one `stat`.
    stat_calls: u64,
    /// Manifest records whose entry was gone from disk.
    manifest_stale_records: u64,
}

/// How a namespace's manifest was found at open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ManifestOutcome {
    Present,
    Absent,
    Malformed,
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
        let now_nanos = unix_nanos_now();
        let reclaimed_legacy_directories = reclaim_legacy_shards(root)?;
        let others = reconcile_other_namespaces(root, &namespace, policy, now_nanos)?;
        let loaded = load_namespace(&namespace_dir, now_nanos)?;
        let scanned_entries = count_u64(loaded.entries.len());
        write_atomic(
            &namespace_dir.join(OPENED_MARKER),
            now_nanos.div_euclid(1_000_000_000).to_string().as_bytes(),
        )
        .map_err(|err| storage_error("write open marker in", &namespace_dir, &err))?;
        let mut ledger = RetentionLedger::new(policy);
        let mut entries = loaded.entries;
        // Oldest write first, so the ledger's recency is the directory's.
        entries.sort_by_key(|entry| (entry.written_nanos, entry.key));
        let mut removed: Vec<EmbeddingCacheKey> = Vec::new();
        for entry in entries {
            removed.extend(
                ledger
                    .seed(entry.key, entry.bytes, entry.written_nanos, ())
                    .into_iter()
                    .map(|(key, ())| key),
            );
        }
        // A tightened policy evicts on open: the directory held more than
        // the policy now allows.
        let evicted_at_open = count_u64(removed.len());
        let expired: Vec<EmbeddingCacheKey> = ledger
            .expire_to_policy(now_nanos)
            .into_iter()
            .map(|(key, ())| key)
            .collect();
        let expired_at_open = count_u64(expired.len());
        removed.extend(expired);
        for key in removed {
            remove_entry_file(&entry_path(&namespace_dir, &key));
        }
        let store = Self {
            root: namespace_dir,
            ledger: Mutex::new(ledger),
            open_report: EmbeddingCacheOpenReport {
                scanned_entries,
                evicted_at_open,
                expired_at_open,
                stale_staging_removed: loaded.stale_staging_removed,
                retired_namespaces: others.retired_namespaces,
                trimmed_foreign_entries: others.trimmed_entries,
                retained_foreign_bytes: others.retained_bytes,
                reclaimed_legacy_directories,
                manifest_present: loaded.manifest == ManifestOutcome::Present,
                manifest_malformed: loaded.manifest == ManifestOutcome::Malformed,
                stat_calls_at_open: loaded.stat_calls.saturating_add(others.stat_calls),
                manifest_stale_records: loaded.manifest_stale_records,
            },
            puts_since_flush: AtomicU64::new(0),
            manifest_flushes: AtomicU64::new(0),
        };
        // The open's own view is the first manifest, so a crash before the
        // first flush costs the next open no more than the writes since.
        store.flush_manifest();
        Ok(store)
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

    /// Manifest flushes since open.
    #[must_use]
    pub fn manifest_flushes(&self) -> u64 {
        self.manifest_flushes.load(Ordering::Acquire)
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

    /// Write the ledger's accounting as the namespace manifest.
    ///
    /// Best-effort like every write here: a failed flush costs the next
    /// open a `stat` per entry written since the last good one, never a
    /// wrong vector.
    fn flush_manifest(&self) {
        let records = match self.lock_ledger() {
            Some(ledger) => ledger.manifest_records(),
            None => return,
        };
        if write_atomic(&self.root.join(MANIFEST_FILE), &encode_manifest(&records)).is_ok() {
            self.puts_since_flush.store(0, Ordering::Release);
            let _prior = self.manifest_flushes.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Count one write toward the flush cadence and flush when it is due.
    fn note_put(&self) {
        let since = self
            .puts_since_flush
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        if since >= MANIFEST_FLUSH_EVERY_PUTS {
            self.flush_manifest();
        }
    }
}

impl Drop for FileEmbeddingCache {
    /// The last flush: a clean shutdown leaves the next open nothing to
    /// `stat`.
    fn drop(&mut self) {
        self.flush_manifest();
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

/// Now, in nanoseconds since the Unix epoch: the resolution the ledger
/// orders writes by, so two writes in one second still evict oldest
/// first. A `u64` of nanoseconds reaches the year 2554.
fn unix_nanos_now() -> u64 {
    unix_nanos_of(SystemTime::now())
}

fn unix_nanos_of(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).map_or(u64::MAX, |nanos| nanos)
        })
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

/// What reconciling the other namespaces under a root did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct OtherNamespaces {
    retired_namespaces: u64,
    trimmed_entries: u64,
    retained_bytes: u64,
    stat_calls: u64,
}

/// Reconcile every namespace under `root` but `current` with `policy`
/// (QI-BB-009).
///
/// Keep the `max_namespaces - 1` most recently opened and remove the
/// rest; trim each kept one to the per-namespace policy — expired
/// entries, then the oldest writes past the entry and byte ceilings — and
/// rewrite its manifest; then, least recently opened first, remove whole
/// namespaces until what they hold together plus the current namespace's
/// full byte ceiling fits under the total ceiling.
fn reconcile_other_namespaces(
    root: &Path,
    current: &str,
    policy: EmbeddingCacheRetentionPolicy,
    now_nanos: u64,
) -> Result<OtherNamespaces, CoreError> {
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
    let mut report = OtherNamespaces::default();
    let keep = policy.namespaces.saturating_sub(1);
    let mut retained: Vec<(PathBuf, u64)> = Vec::new();
    for (index, (_opened, path)) in others.into_iter().enumerate() {
        if index >= keep {
            std::fs::remove_dir_all(&path).map_err(|err| storage_error("remove", &path, &err))?;
            report.retired_namespaces = report.retired_namespaces.saturating_add(1);
            continue;
        }
        let trimmed = trim_namespace_to_policy(&path, policy, now_nanos)?;
        report.trimmed_entries = report.trimmed_entries.saturating_add(trimmed.removed);
        report.stat_calls = report.stat_calls.saturating_add(trimmed.stat_calls);
        retained.push((path, trimmed.retained_bytes));
    }
    // The global ceiling: the current namespace may fill its whole
    // per-namespace ceiling, so the others must leave that much room.
    let room_for_others = policy.max_total_bytes.saturating_sub(policy.resident_bytes);
    let mut retained_bytes: u64 = retained.iter().map(|(_path, bytes)| *bytes).sum();
    while retained_bytes > room_for_others {
        let Some((path, bytes)) = retained.pop() else {
            break;
        };
        std::fs::remove_dir_all(&path).map_err(|err| storage_error("remove", &path, &err))?;
        report.retired_namespaces = report.retired_namespaces.saturating_add(1);
        retained_bytes = retained_bytes.saturating_sub(bytes);
    }
    report.retained_bytes = retained_bytes;
    Ok(report)
}

/// What trimming one retained namespace did.
struct NamespaceTrim {
    removed: u64,
    retained_bytes: u64,
    stat_calls: u64,
}

/// Bring a namespace this process is not writing to within the policy.
///
/// Expired entries go first and then, oldest write first, whatever
/// exceeds the entry or byte ceiling; the manifest is rewritten so the
/// next open of the namespace `stat`s nothing.
fn trim_namespace_to_policy(
    namespace_dir: &Path,
    policy: EmbeddingCacheRetentionPolicy,
    now_nanos: u64,
) -> Result<NamespaceTrim, CoreError> {
    let loaded = load_namespace(namespace_dir, now_nanos)?;
    let mut ledger: RetentionLedger<()> = RetentionLedger::new(policy);
    let mut entries = loaded.entries;
    entries.sort_by_key(|entry| (entry.written_nanos, entry.key));
    let mut removed: Vec<EmbeddingCacheKey> = Vec::new();
    for entry in entries {
        removed.extend(
            ledger
                .seed(entry.key, entry.bytes, entry.written_nanos, ())
                .into_iter()
                .map(|(key, ())| key),
        );
    }
    removed.extend(
        ledger
            .expire_to_policy(now_nanos)
            .into_iter()
            .map(|(key, ())| key),
    );
    for key in &removed {
        remove_entry_file(&entry_path(namespace_dir, key));
    }
    write_atomic(
        &namespace_dir.join(MANIFEST_FILE),
        &encode_manifest(&ledger.manifest_records()),
    )
    .map_err(|err| storage_error("write manifest in", namespace_dir, &err))?;
    Ok(NamespaceTrim {
        removed: count_u64(removed.len()),
        retained_bytes: ledger.stats().resident_bytes,
        stat_calls: loaded.stat_calls,
    })
}

/// Load a namespace's entries from its manifest and directory listing,
/// removing the temporary files of interrupted writes.
///
/// The listing names every entry without touching its metadata; an entry
/// the manifest covers takes its bytes and write time from there, and only
/// an uncovered entry is `stat`ed, each one counted. A manifest record
/// with no file behind it is dropped and counted.
fn load_namespace(namespace_dir: &Path, now_nanos: u64) -> Result<LoadedNamespace, CoreError> {
    let (manifest, mut records) = match read_manifest(namespace_dir) {
        ManifestRead::Present(records) => (ManifestOutcome::Present, records),
        ManifestRead::Absent => (ManifestOutcome::Absent, BTreeMap::new()),
        ManifestRead::Malformed => (ManifestOutcome::Malformed, BTreeMap::new()),
    };
    let mut entries = Vec::new();
    let mut stale_staging_removed = 0_u64;
    let mut stat_calls = 0_u64;
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
            if let Some(record) = records.remove(&key) {
                entries.push(ScannedEntry {
                    key,
                    bytes: record.bytes,
                    written_nanos: record.written_nanos,
                });
                continue;
            }
            stat_calls = stat_calls.saturating_add(1);
            let metadata =
                std::fs::metadata(&path).map_err(|err| storage_error("stat", &path, &err))?;
            let written_nanos = metadata
                .modified()
                .map(unix_nanos_of)
                .map_err(|err| storage_error("stat", &path, &err))?
                .min(now_nanos);
            entries.push(ScannedEntry {
                key,
                bytes: metadata.len(),
                written_nanos,
            });
        }
    }
    Ok(LoadedNamespace {
        entries,
        stale_staging_removed,
        manifest,
        stat_calls,
        manifest_stale_records: count_u64(records.len()),
    })
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
        let path = self.path_for(key);
        {
            let mut ledger = self.lock_ledger()?;
            // The ledger is authoritative for what this process holds: an
            // absent key is a miss without a disk probe, and an entry past
            // the age cap is removed here, under the lock, before anyone
            // could read it again.
            match ledger.touch(key, unix_nanos_now()) {
                Touch::Hit(()) => {}
                Touch::Miss => return None,
                Touch::Expired(_) => {
                    remove_entry_file(&path);
                    return None;
                }
            }
        }
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
                let refused = ledger.insert(*key, entry_bytes, unix_nanos_now(), ());
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
        // agree on its bytes whichever renamed last, and expire and evict
        // under the same lock so the ceilings hold at every point a reader
        // can see.
        let Ok(on_disk) = std::fs::metadata(&path) else {
            return;
        };
        let now = unix_nanos_now();
        let removed: Vec<EmbeddingCacheKey> = {
            let Some(mut ledger) = self.lock_ledger() else {
                return;
            };
            let mut removed: Vec<EmbeddingCacheKey> = ledger
                .expire_to_policy(now)
                .into_iter()
                .map(|(key, ())| key)
                .collect();
            let outcome = ledger.insert(*key, on_disk.len(), now, ());
            removed.extend(outcome.evicted.into_iter().map(|(key, ())| key));
            removed
        };
        for gone in removed {
            remove_entry_file(&self.path_for(&gone));
        }
        self.note_put();
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

    use quanta_index_core::L2UnitEmbeddingProvider;

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

    /// An age no test entry reaches.
    const LONG_AGE: Duration = Duration::from_secs(365 * 24 * 60 * 60);

    /// A small policy: `entries` entries, `bytes` bytes, one namespace,
    /// no age or total ceiling in reach.
    fn policy(entries: u64, bytes: u64) -> EmbeddingCacheRetentionPolicy {
        EmbeddingCacheRetentionPolicy::new(entries, bytes, 1, LONG_AGE, u64::MAX).expect("policy")
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

    /// A raw provider: no output is on the unit sphere (each is the text's
    /// byte sum and length, scaled by 2.5), as a remote model's may not be.
    struct RawProvider {
        embedded: Arc<AtomicUsize>,
    }

    impl TextEmbeddingProvider for RawProvider {
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
            let _prior = self.embedded.fetch_add(texts.len(), Ordering::SeqCst);
            Ok(texts
                .iter()
                .map(|text| {
                    let sum: u32 = text.bytes().map(u32::from).sum();
                    let sum = u16::try_from(sum % 1000).map_or(0.0, f32::from);
                    let length = u16::try_from(text.len()).map_or(0.0, f32::from);
                    vec![2.5 * sum, 2.5 * length, 2.5]
                })
                .collect())
        }
        fn model_id(&self) -> &'static str {
            "raw-mixed"
        }
        fn model_revision(&self) -> &'static str {
            "r1"
        }
        fn dimension(&self) -> usize {
            3
        }
        fn normalization(&self) -> EmbeddingNormalization {
            EmbeddingNormalization::None
        }
    }

    /// Where a mixed batch's hits come from.
    #[derive(Clone, Copy, Debug)]
    enum Store {
        Memory,
        File,
    }

    /// A batch that mixes cache hits and fresh misses is the batch an
    /// uncached provider returns, bit for bit (QI-BB-031 완료 기준 #3).
    ///
    /// The production shape — a cache over the `L2Unit` wrapper over a raw
    /// provider — is warmed with two texts, then asked for a batch holding
    /// both, two new texts and a repeat: every vector equals by bits what the
    /// wrapper alone returns for the same batch, only the two new texts
    /// reach the provider, and every vector holds the unit contract. Once
    /// through memory and once through the file cache's disk round trip.
    #[test]
    fn a_mixed_hit_and_miss_batch_is_the_uncached_batch_bit_for_bit() {
        let batch = ["alpha", "beta", "gamma", "delta", "alpha"];
        let bits = |vectors: &[Vec<f32>]| -> Vec<Vec<u32>> {
            vectors
                .iter()
                .map(|vector| vector.iter().map(|component| component.to_bits()).collect())
                .collect()
        };
        let oracle = L2UnitEmbeddingProvider::new(RawProvider {
            embedded: Arc::new(AtomicUsize::new(0)),
        })
        .expect("wrap the raw provider")
        .embed_batch(&batch)
        .expect("uncached batch");
        let dir = tempfile::tempdir().expect("tempdir");
        for store in [Store::Memory, Store::File] {
            let embedded = Arc::new(AtomicUsize::new(0));
            let inner = L2UnitEmbeddingProvider::new(RawProvider {
                embedded: Arc::clone(&embedded),
            })
            .expect("wrap the raw provider");
            let cache: Box<dyn EmbeddingCache> = match store {
                Store::Memory => Box::new(InMemoryEmbeddingCache::default()),
                Store::File => Box::new(
                    FileEmbeddingCache::new(
                        &dir.path().join("embed-cache"),
                        &EmbeddingCacheIdentityV1::of(&inner),
                        EmbeddingCacheRetentionPolicy::DEFAULT,
                    )
                    .expect("file cache"),
                ),
            };
            let provider = CachingEmbeddingProvider::new(Box::new(inner), cache);
            let _warm = provider.embed_batch(&["alpha", "gamma"]).expect("warm");
            let warmed = embedded.load(Ordering::SeqCst);
            let mixed = provider.embed_batch(&batch).expect("mixed batch");
            assert_eq!(
                embedded.load(Ordering::SeqCst).saturating_sub(warmed),
                2,
                "{store:?}: only the two new texts reach the provider"
            );
            assert_eq!(bits(&mixed), bits(&oracle), "{store:?}");
            for vector in &mixed {
                SemanticPolicy::validate_embedding_vector_v1(
                    vector,
                    3,
                    EmbeddingNormalization::L2Unit,
                )
                .expect("every vector holds the unit contract");
            }
        }
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
        // Three other identities opened in order under a generous policy
        // (a finite per-namespace ceiling, so the total ceiling leaves the
        // others room).
        let identities: Vec<EmbeddingCacheIdentityV1> =
            (0..3).map(|i| identity("m", &format!("old-{i}"))).collect();
        for (index, identity) in identities.iter().enumerate() {
            let cache = FileEmbeddingCache::new(
                &root,
                identity,
                EmbeddingCacheRetentionPolicy::new(64, 1 << 30, 8, LONG_AGE, u64::MAX)
                    .expect("policy"),
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
            EmbeddingCacheRetentionPolicy::new(64, 1 << 30, 3, LONG_AGE, u64::MAX).expect("policy"),
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
        let age = Duration::from_secs(1);
        assert!(EmbeddingCacheRetentionPolicy::new(0, 1, 1, age, 1).is_err());
        assert!(EmbeddingCacheRetentionPolicy::new(1, 0, 1, age, 1).is_err());
        assert!(EmbeddingCacheRetentionPolicy::new(1, 1, 0, age, 1).is_err());
        assert!(EmbeddingCacheRetentionPolicy::new(1, 1, 1, Duration::ZERO, 1).is_err());
        assert!(EmbeddingCacheRetentionPolicy::new(1, 1, 1, age, 0).is_err());
        assert!(
            EmbeddingCacheRetentionPolicy::new(1, 2, 1, age, 1).is_err(),
            "the total ceiling must hold one namespace"
        );
        assert!(EmbeddingCacheRetentionPolicy::new(1, 1, 1, age, 1).is_ok());
        assert!(
            EmbeddingCacheRetentionPolicy::DEFAULT.max_total_bytes()
                >= EmbeddingCacheRetentionPolicy::DEFAULT.max_resident_bytes()
        );
    }

    fn nanos(secs: u64) -> u64 {
        secs.saturating_mul(1_000_000_000)
    }

    /// The age cap, on the ledger with an injected clock: an entry past
    /// it is expired at the next lookup (a counted miss) and at the next
    /// write, never served.
    #[test]
    fn the_ledger_expires_entries_past_the_age_cap_on_lookup_and_on_write() {
        let policy =
            EmbeddingCacheRetentionPolicy::new(64, u64::MAX, 1, Duration::from_secs(100), u64::MAX)
                .expect("policy");
        let mut ledger: RetentionLedger<u32> = RetentionLedger::new(policy);
        let old = key("old");
        let fresh = key("fresh");
        let _seeded: Vec<(EmbeddingCacheKey, u32)> = ledger.seed(old, 8, nanos(1_000), 1);
        let _seeded: Vec<(EmbeddingCacheKey, u32)> = ledger.seed(fresh, 8, nanos(1_050), 2);
        // Within the cap, both are hits.
        assert!(matches!(ledger.touch(&old, nanos(1_099)), Touch::Hit(1)));
        assert!(matches!(ledger.touch(&fresh, nanos(1_099)), Touch::Hit(2)));
        // Past it for `old` only: expired on lookup, counted once, gone.
        assert!(matches!(
            ledger.touch(&old, nanos(1_101)),
            Touch::Expired(Some(1))
        ));
        assert!(matches!(ledger.touch(&old, nanos(1_101)), Touch::Miss));
        let stats = ledger.stats();
        assert_eq!(
            (stats.expirations, stats.misses, stats.hits, stats.entries),
            (1, 2, 2, 1)
        );
        // A write at a time past `fresh`'s age expires it before the insert.
        let expired = ledger.expire_to_policy(nanos(1_151));
        assert_eq!(expired, vec![(fresh, 2)]);
        let _inserted = ledger.insert(key("newest"), 8, nanos(1_151), 3);
        let stats = ledger.stats();
        assert_eq!(
            (stats.expirations, stats.entries, stats.resident_bytes),
            (2, 1, 8)
        );
        assert!(matches!(
            ledger.touch(&key("newest"), nanos(1_200)),
            Touch::Hit(3)
        ));
    }

    /// The file store expires on open what the manifest says is too old,
    /// removes the files, and the ledger equals the disk afterwards.
    #[test]
    fn opening_expires_entries_older_than_the_age_cap_from_the_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("embed-cache");
        let keys: Vec<EmbeddingCacheKey> = (0..4).map(|i| key(&format!("aged-{i}"))).collect();
        {
            let cache = file_cache(&root, policy(64, u64::MAX));
            for key in &keys {
                cache.put(key, &[0.0, 1.0]);
            }
        }
        // Rewrite the manifest so two entries were written ten days ago.
        let namespace_dir = root.join(identity("m", "r").namespace());
        let mut records = match read_manifest(&namespace_dir) {
            ManifestRead::Present(records) => records,
            ManifestRead::Absent | ManifestRead::Malformed => {
                panic!("the drop flushed a manifest")
            }
        };
        let ten_days_ago = unix_nanos_now() - nanos(10 * 24 * 60 * 60);
        for key in keys.iter().take(2) {
            records.get_mut(key).expect("manifested").written_nanos = ten_days_ago;
        }
        write_atomic(
            &namespace_dir.join(MANIFEST_FILE),
            &encode_manifest(&records),
        )
        .expect("manifest");

        let reopened = FileEmbeddingCache::new(
            &root,
            &identity("m", "r"),
            EmbeddingCacheRetentionPolicy::new(
                64,
                u64::MAX,
                1,
                Duration::from_secs(24 * 60 * 60),
                u64::MAX,
            )
            .expect("policy"),
        )
        .expect("reopen");
        let report = reopened.open_report();
        assert_eq!((report.expired_at_open, report.scanned_entries), (2, 4));
        assert!(report.manifest_present && report.stat_calls_at_open == 0);
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(reopened.path_for(key).exists(), index >= 2, "entry {index}");
        }
        assert_eq!(reopened.stats().entries, 2);
        assert_ledger_matches_disk(&reopened);
    }

    /// Opening trusts the manifest for the entries it covers and reads
    /// metadata only for the ones it does not.
    ///
    /// The covered entries keep the manifest's write times (distinct from
    /// their file times, so a re-read would show), the uncovered ones are
    /// counted, and a record without a file behind it is dropped.
    #[test]
    fn opening_trusts_the_manifest_and_stats_only_the_entries_it_does_not_cover() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("embed-cache");
        let keys: Vec<EmbeddingCacheKey> = (0..6).map(|i| key(&format!("m-{i}"))).collect();
        {
            let cache = file_cache(&root, policy(64, u64::MAX));
            for key in keys.iter().take(4) {
                cache.put(key, &[0.0, 1.0]);
            }
        }
        let namespace_dir = root.join(identity("m", "r").namespace());
        // Two entries written after the last flush (a crash before the
        // next one): the store is forgotten, so its drop never flushes.
        {
            let cache = file_cache(&root, policy(64, u64::MAX));
            for key in keys.iter().skip(4) {
                cache.put(key, &[0.0, 1.0]);
            }
            std::mem::forget(cache);
        }
        // The manifest says entry 0 is the newest write and entry 3 the
        // oldest — the reverse of the file times.
        let mut records = match read_manifest(&namespace_dir) {
            ManifestRead::Present(records) => records,
            ManifestRead::Absent | ManifestRead::Malformed => {
                panic!("the drop flushed a manifest")
            }
        };
        let base = unix_nanos_now() - nanos(1_000);
        for (index, key) in keys.iter().take(4).enumerate() {
            records.get_mut(key).expect("manifested").written_nanos =
                base - nanos(u64::try_from(index).expect("fits"));
        }
        // A record whose file is gone.
        let _stale = records.insert(
            key("vanished"),
            ManifestRecord {
                bytes: entry_len(),
                written_nanos: base,
            },
        );
        write_atomic(
            &namespace_dir.join(MANIFEST_FILE),
            &encode_manifest(&records),
        )
        .expect("manifest");
        // The uncovered entries' file times are the newest of all.
        let reopened = file_cache(&root, policy(64, u64::MAX));
        let report = reopened.open_report();
        assert!(report.manifest_present);
        assert_eq!(
            report.stat_calls_at_open, 2,
            "only the uncovered entries were read"
        );
        assert_eq!(report.manifest_stale_records, 1);
        assert_eq!(report.scanned_entries, 6);
        assert_ledger_matches_disk(&reopened);
        drop(reopened);
        // Tightened to three entries: the manifest's order rules, so the
        // survivors are the two uncovered (newest by file time) entries
        // and entry 0, which the manifest calls the newest covered write —
        // not entries 2 and 3, which the file times would have kept.
        let tightened = file_cache(&root, policy(3, u64::MAX));
        assert_eq!(tightened.open_report().evicted_at_open, 3);
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(
                tightened.path_for(key).exists(),
                index == 0 || index >= 4,
                "entry {index}"
            );
        }
        assert_ledger_matches_disk(&tightened);
    }

    /// A malformed manifest is ignored and counted: every entry is read
    /// from disk and the store still opens whole.
    #[test]
    fn a_malformed_manifest_is_ignored_and_every_entry_is_read_from_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("embed-cache");
        {
            let cache = file_cache(&root, policy(64, u64::MAX));
            cache.put(&key("a"), &[0.0, 1.0]);
            cache.put(&key("b"), &[0.0, 1.0]);
        }
        let namespace_dir = root.join(identity("m", "r").namespace());
        std::fs::write(namespace_dir.join(MANIFEST_FILE), b"not a manifest").expect("clobber");
        let reopened = file_cache(&root, policy(64, u64::MAX));
        let report = reopened.open_report();
        assert!(report.manifest_malformed && !report.manifest_present);
        assert_eq!((report.stat_calls_at_open, report.scanned_entries), (2, 2));
        assert_eq!(reopened.stats().entries, 2);
        assert!(reopened.get(&key("a")).is_some());
        assert_ledger_matches_disk(&reopened);
        assert_eq!(
            reopened.manifest_flushes(),
            1,
            "the open rewrote the manifest"
        );
    }

    /// Every `.vec` byte under a cache root, over every namespace.
    fn root_bytes(root: &Path) -> u64 {
        let mut total = 0_u64;
        for namespace in std::fs::read_dir(root).expect("root") {
            let namespace = namespace.expect("namespace");
            if !namespace.path().is_dir()
                || !is_hex_name(&namespace.file_name().to_string_lossy(), NAMESPACE_LEN)
            {
                continue;
            }
            total = total.saturating_add(on_disk_entries(&namespace.path()).values().sum::<u64>());
        }
        total
    }

    /// A narrower policy is applied to the retained other namespaces on
    /// open, not just counted.
    ///
    /// Each is trimmed to the per-namespace policy (oldest writes first)
    /// and rewrites its manifest, and whole namespaces are dropped, least
    /// recently opened first, until the total ceiling leaves room for the
    /// current namespace's full ceiling.
    #[test]
    fn opening_trims_retained_namespaces_to_the_policy_and_holds_the_total_ceiling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("embed-cache");
        let others: Vec<EmbeddingCacheIdentityV1> = (0..2)
            .map(|i| identity("m", &format!("other-{i}")))
            .collect();
        for (index, other) in others.iter().enumerate() {
            let cache = FileEmbeddingCache::new(
                &root,
                other,
                EmbeddingCacheRetentionPolicy::new(64, 1 << 30, 8, LONG_AGE, u64::MAX)
                    .expect("policy"),
            )
            .expect("cache");
            for entry in 0..4 {
                cache.put(&other.key(&format!("text-{entry}")), &[0.0, 1.0]);
            }
            drop(cache);
            let stamp = SystemTime::UNIX_EPOCH
                + Duration::from_secs(1_700_000_000 + u64::try_from(index).expect("fits"));
            std::fs::File::open(root.join(other.namespace()).join(OPENED_MARKER))
                .expect("marker")
                .set_modified(stamp)
                .expect("set modified");
        }
        assert_eq!(root_bytes(&root), entry_len() * 8);

        // Two entries per namespace: both others are trimmed to two, the
        // oldest writes going first, and the total ceiling (room for the
        // current namespace's 3 entries plus 4 more) keeps both.
        let current = identity("m", "current");
        let per_namespace = entry_len() * 3;
        let cache = FileEmbeddingCache::new(
            &root,
            &current,
            EmbeddingCacheRetentionPolicy::new(
                2,
                per_namespace,
                3,
                LONG_AGE,
                per_namespace + entry_len() * 4,
            )
            .expect("policy"),
        )
        .expect("cache");
        let report = cache.open_report();
        assert_eq!(
            (report.retired_namespaces, report.trimmed_foreign_entries),
            (0, 4)
        );
        assert_eq!(report.retained_foreign_bytes, entry_len() * 4);
        for other in &others {
            let disk = on_disk_entries(&root.join(other.namespace()));
            assert_eq!(disk.len(), 2, "{} trimmed to the policy", other.namespace());
            for entry in 0..4 {
                assert_eq!(
                    disk.contains_key(&other.key(&format!("text-{entry}")).hex()),
                    entry >= 2,
                    "the oldest writes went first"
                );
            }
            assert!(matches!(
                read_manifest(&root.join(other.namespace())),
                ManifestRead::Present(records) if records.len() == 2
            ));
        }
        assert_eq!(root_bytes(&root), entry_len() * 4);
        drop(cache);

        // A total ceiling with room for only two foreign entries beside
        // the current namespace's full ceiling: the least recently opened
        // other namespace is dropped whole.
        let cache = FileEmbeddingCache::new(
            &root,
            &current,
            EmbeddingCacheRetentionPolicy::new(
                2,
                per_namespace,
                3,
                LONG_AGE,
                per_namespace + entry_len() * 2,
            )
            .expect("policy"),
        )
        .expect("cache");
        let report = cache.open_report();
        assert_eq!(
            (report.retired_namespaces, report.trimmed_foreign_entries),
            (1, 0)
        );
        assert_eq!(report.retained_foreign_bytes, entry_len() * 2);
        assert!(
            !root.join(others[0].namespace()).is_dir(),
            "the older namespace went"
        );
        assert!(root.join(others[1].namespace()).is_dir());
        // Filling the current namespace to its own ceiling never takes the
        // root past the total ceiling.
        for entry in 0..10 {
            cache.put(&current.key(&format!("fill-{entry}")), &[0.0, 1.0]);
            assert!(root_bytes(&root) <= per_namespace + entry_len() * 2);
        }
        assert_ledger_matches_disk(&cache);
    }
}
