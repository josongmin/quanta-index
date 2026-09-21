//! The regex match cache (QI-BB-024).
//!
//! One entry per `(generation, executed regex source)`: the set of
//! text-authority doc ids the regex matched, as a compressed bitmap shared
//! by `Arc`. A hit hands out the resident set — no copy of any member — and
//! a query that holds a set keeps it alive past its eviction or its
//! generation's invalidation, so neither can break a query in flight.
//!
//! The cache is least-recently-used under two bounds, the entry count and
//! the bytes its entries occupy. A set wider than one entry may be — by
//! cardinality or by bytes — is served but not kept, and counted, so a run
//! of broad regexes cannot turn the cache into copies of the corpus.

use std::collections::BTreeMap;
use std::sync::Arc;

use quanta_index_core::{RegexMatchCachePolicy, RegexMatchCacheStats};
use roaring::RoaringBitmap;

use crate::GenKey;

/// Bytes one bitmap container costs beyond its payload: the container's
/// key, store tag and slot in the container vector, rounded up.
const ROARING_CONTAINER_OVERHEAD: u64 = 32;

/// Heap bytes of one dense container: 2^16 bits.
const ROARING_BITSET_CONTAINER_BYTES: u64 = 8_192;

/// Bytes a set costs before its first container: the bitmap's container
/// vector header, rounded up.
const ROARING_SET_OVERHEAD: u64 = 32;

/// Bytes one cache entry costs beyond its set and its key strings: the map
/// node, the recency slot and the `Arc` header, rounded up.
const REGEX_MATCH_CACHE_ENTRY_OVERHEAD: u64 = 128;

/// Bytes a match set occupies, counted high rather than low.
///
/// Sparse containers are counted by their allocated capacity as the bitmap
/// reports it — four bytes a slot for two-byte values, so twice their
/// heap; dense containers by their fixed size (the bitmap's own statistic
/// counts them in bits); run containers by their runs; every container by
/// its header, and the set by its own.
pub(crate) fn match_set_bytes(members: &RoaringBitmap) -> u64 {
    let stats = members.statistics();
    ROARING_SET_OVERHEAD
        .saturating_add(stats.n_bytes_array_containers)
        .saturating_add(stats.n_bytes_run_containers)
        .saturating_add(
            u64::from(stats.n_bitset_containers).saturating_mul(ROARING_BITSET_CONTAINER_BYTES),
        )
        .saturating_add(u64::from(stats.n_containers).saturating_mul(ROARING_CONTAINER_OVERHEAD))
}

fn string_bytes(value: &str) -> u64 {
    u64::try_from(value.len()).map_or(u64::MAX, |bytes| bytes)
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct RegexMatchCacheKey {
    pub(crate) generation: GenKey,
    pub(crate) normalized_source: String,
}

impl RegexMatchCacheKey {
    /// Heap bytes of the key's strings. The key is held twice — in the
    /// entry map and in the recency index — and counted twice.
    fn heap_bytes(&self) -> u64 {
        string_bytes(&self.normalized_source)
            .saturating_add(string_bytes(self.generation.repo_id.as_str()))
            .saturating_add(string_bytes(self.generation.revision_id.as_str()))
            .saturating_mul(2)
    }
}

/// Why a computed match set was not cached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RegexMatchCacheRefusal {
    Cardinality { matches: u64 },
    Bytes { bytes: u64 },
}

struct CachedMatchSet {
    members: Arc<RoaringBitmap>,
    /// What this entry adds to the resident bytes, fixed at insert so an
    /// eviction gives back exactly what the insert took.
    bytes: u64,
    /// The entry's slot in the recency index.
    stamp: u64,
}

/// Byte-weighted LRU of regex match sets, shared by `Arc`.
pub(crate) struct RegexMatchCache {
    entries: BTreeMap<RegexMatchCacheKey, CachedMatchSet>,
    /// Stamp → key, oldest first.
    recency: BTreeMap<u64, RegexMatchCacheKey>,
    next_stamp: u64,
    policy: RegexMatchCachePolicy,
    resident_bytes: u64,
    stats: RegexMatchCacheStats,
}

impl RegexMatchCache {
    pub(crate) fn new(policy: RegexMatchCachePolicy) -> Self {
        Self {
            entries: BTreeMap::new(),
            recency: BTreeMap::new(),
            next_stamp: 0,
            policy,
            resident_bytes: 0,
            stats: RegexMatchCacheStats::default(),
        }
    }

    fn take_stamp(&mut self) -> u64 {
        let stamp = self.next_stamp;
        self.next_stamp = self.next_stamp.saturating_add(1);
        stamp
    }

    /// The resident set for `key`, moved to most recently used; the same
    /// allocation every other holder of it sees.
    pub(crate) fn get(&mut self, key: &RegexMatchCacheKey) -> Option<Arc<RoaringBitmap>> {
        let stamp = self.take_stamp();
        let Some(entry) = self.entries.get_mut(key) else {
            self.stats.misses = self.stats.misses.saturating_add(1);
            return None;
        };
        self.stats.hits = self.stats.hits.saturating_add(1);
        let previous = std::mem::replace(&mut entry.stamp, stamp);
        let members = Arc::clone(&entry.members);
        if let Some(key) = self.recency.remove(&previous) {
            let _prior: Option<RegexMatchCacheKey> = self.recency.insert(stamp, key);
        }
        Some(members)
    }

    /// Account a set a query computed, cached or not.
    pub(crate) fn record_built(&mut self, members: &RoaringBitmap) {
        self.stats.sets_built = self.stats.sets_built.saturating_add(1);
        self.stats.members_built = self.stats.members_built.saturating_add(members.len());
        self.stats.bytes_built = self
            .stats
            .bytes_built
            .saturating_add(match_set_bytes(members));
    }

    fn remove(&mut self, key: &RegexMatchCacheKey) -> bool {
        let Some(removed) = self.entries.remove(key) else {
            return false;
        };
        let _key: Option<RegexMatchCacheKey> = self.recency.remove(&removed.stamp);
        self.resident_bytes = self.resident_bytes.saturating_sub(removed.bytes);
        true
    }

    /// Keep `members` for `key`, evicting least recently used entries until
    /// both bounds hold, or refuse it when it alone breaks one.
    pub(crate) fn insert(
        &mut self,
        key: RegexMatchCacheKey,
        members: Arc<RoaringBitmap>,
    ) -> Result<(), RegexMatchCacheRefusal> {
        let cardinality = members.len();
        if cardinality
            > u64::try_from(self.policy.max_matches_per_entry()).map_or(u64::MAX, |max| max)
        {
            self.stats.refused_cardinality = self.stats.refused_cardinality.saturating_add(1);
            return Err(RegexMatchCacheRefusal::Cardinality {
                matches: cardinality,
            });
        }
        let bytes = match_set_bytes(&members)
            .saturating_add(key.heap_bytes())
            .saturating_add(REGEX_MATCH_CACHE_ENTRY_OVERHEAD);
        if bytes > self.policy.max_resident_bytes() {
            self.stats.refused_bytes = self.stats.refused_bytes.saturating_add(1);
            return Err(RegexMatchCacheRefusal::Bytes { bytes });
        }
        let _replaced: bool = self.remove(&key);
        while !self.entries.is_empty()
            && (self.entries.len() >= self.policy.max_entries()
                || self.resident_bytes.saturating_add(bytes) > self.policy.max_resident_bytes())
        {
            let Some((_stamp, oldest)) = self.recency.pop_first() else {
                break;
            };
            let _evicted: bool = self.remove(&oldest);
            self.stats.evictions = self.stats.evictions.saturating_add(1);
        }
        let stamp = self.take_stamp();
        let _prior: Option<RegexMatchCacheKey> = self.recency.insert(stamp, key.clone());
        self.resident_bytes = self.resident_bytes.saturating_add(bytes);
        let _prior: Option<CachedMatchSet> = self.entries.insert(
            key,
            CachedMatchSet {
                members,
                bytes,
                stamp,
            },
        );
        Ok(())
    }

    /// Drop every entry of `generation`; queries holding one of its sets
    /// keep them.
    pub(crate) fn invalidate_generation(&mut self, generation: &GenKey) {
        let stale: Vec<RegexMatchCacheKey> = self
            .entries
            .keys()
            .filter(|key| &key.generation == generation)
            .cloned()
            .collect();
        for key in &stale {
            let _removed: bool = self.remove(key);
        }
    }

    pub(crate) fn stats(&self) -> RegexMatchCacheStats {
        RegexMatchCacheStats {
            entries: self.entries.len(),
            resident_bytes: self.resident_bytes,
            ..self.stats
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::arithmetic_side_effects,
    reason = "test oracles add and multiply small bounded byte counts"
)]
mod tests {
    use std::sync::Arc;

    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
    use quanta_index_core::RegexMatchCachePolicy;
    use roaring::RoaringBitmap;

    use super::{
        REGEX_MATCH_CACHE_ENTRY_OVERHEAD, RegexMatchCache, RegexMatchCacheKey,
        RegexMatchCacheRefusal, match_set_bytes,
    };
    use crate::GenKey;

    fn generation(generation: u64) -> GenKey {
        GenKey {
            repo_id: RepoId::new("repo-alpha")
                .expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-alpha")
                .expect("static fixture ID satisfies canonical policy"),
            generation: ManifestGeneration::new(generation),
        }
    }

    fn key(generation_key: &GenKey, source: &str) -> RegexMatchCacheKey {
        RegexMatchCacheKey {
            generation: generation_key.clone(),
            normalized_source: source.to_string(),
        }
    }

    fn members(ids: &[u32]) -> Arc<RoaringBitmap> {
        Arc::new(ids.iter().copied().collect())
    }

    /// What one entry costs: its set, its key strings held twice, and the
    /// entry overhead.
    fn entry_bytes(entry_key: &RegexMatchCacheKey, set: &RoaringBitmap) -> u64 {
        let strings = entry_key.normalized_source.len()
            + entry_key.generation.repo_id.as_str().len()
            + entry_key.generation.revision_id.as_str().len();
        match_set_bytes(set)
            + 2 * u64::try_from(strings).expect("fits")
            + REGEX_MATCH_CACHE_ENTRY_OVERHEAD
    }

    fn range(start: u32, len: u32) -> RoaringBitmap {
        let mut set = RoaringBitmap::new();
        let _inserted: u64 = set.insert_range(start..start + len);
        set
    }

    /// `members` ids two apart from `start`: half-full chunks, so the set is
    /// held in dense containers rather than one run each.
    fn every_other(start: u32, members: u32) -> RoaringBitmap {
        RoaringBitmap::from_sorted_iter((0..members).map(|index| start + 2 * index))
            .expect("ascending")
    }

    /// The accounted bytes of a set never undercount its payload.
    ///
    /// The oracle is the bitmap's serialized form, which carries every
    /// container's values (two bytes each), dense words or runs; and a
    /// dense container is never counted as more than its 8 KiB and header.
    #[test]
    fn a_match_set_accounts_at_least_its_own_size() {
        for set in [
            RoaringBitmap::new(),
            (0..3).collect::<RoaringBitmap>(),
            (0..70_000).step_by(17).collect::<RoaringBitmap>(),
            range(0, 1_000_000),
            every_other(0, 1_000_000),
        ] {
            assert!(
                match_set_bytes(&set) >= u64::try_from(set.serialized_size()).expect("fits"),
                "{} members: accounted {} < serialized {}",
                set.len(),
                match_set_bytes(&set),
                set.serialized_size()
            );
        }
        let dense = every_other(0, 1 << 15);
        assert_eq!(dense.statistics().n_bitset_containers, 1);
        assert_eq!(match_set_bytes(&dense), 32 + 8_192 + 32);
    }

    #[test]
    fn invalidation_drops_only_its_generation_and_gives_back_its_bytes() {
        let first = generation(7);
        let second = generation(8);
        let mut cache = RegexMatchCache::new(RegexMatchCachePolicy::DEFAULT);
        cache
            .insert(key(&first, "foo"), members(&[1]))
            .expect("fits");
        cache
            .insert(key(&first, "bar"), members(&[2]))
            .expect("fits");
        cache
            .insert(key(&second, "foo"), members(&[3]))
            .expect("fits");
        assert_eq!(cache.stats().entries, 3);

        cache.invalidate_generation(&first);

        assert_eq!(cache.stats().entries, 1);
        assert!(cache.get(&key(&first, "foo")).is_none());
        assert!(cache.get(&key(&first, "bar")).is_none());
        assert_eq!(cache.get(&key(&second, "foo")).as_deref(), Some(&*members(&[3])));
        assert_eq!(
            cache.stats().resident_bytes,
            entry_bytes(&key(&second, "foo"), &members(&[3])),
            "invalidation gives back exactly the dropped entries' bytes"
        );
    }

    /// The cache is bounded by bytes and cardinality, not entries alone.
    ///
    /// A set wider than the policy is refused and counted, inserts evict
    /// least-recently-used entries until the byte bound holds, and a hit is
    /// the shared set rather than a copy.
    #[test]
    fn the_cache_is_byte_bounded_least_recently_used_and_shares_hits() {
        let generation_key = generation(1);
        let one_entry = entry_bytes(&key(&generation_key, "a"), &members(&[1]));
        let policy = RegexMatchCachePolicy::new(8, one_entry * 2 + 1, 2).expect("valid policy");
        let mut cache = RegexMatchCache::new(policy);

        assert_eq!(
            cache.insert(key(&generation_key, "broad"), members(&[1, 2, 3])),
            Err(RegexMatchCacheRefusal::Cardinality { matches: 3 })
        );
        assert_eq!(cache.stats().refused_cardinality, 1);
        assert_eq!(cache.stats().resident_bytes, 0);

        cache
            .insert(key(&generation_key, "a"), members(&[1]))
            .expect("fits");
        cache
            .insert(key(&generation_key, "b"), members(&[2]))
            .expect("fits");
        assert_eq!(cache.stats().entries, 2);
        // Touch "a": "b" is now the least recently used.
        assert!(cache.get(&key(&generation_key, "a")).is_some());
        cache
            .insert(key(&generation_key, "c"), members(&[3]))
            .expect("fits after eviction");
        let stats = cache.stats();
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.evictions, 1);
        assert!(stats.resident_bytes <= policy.max_resident_bytes());
        assert!(cache.get(&key(&generation_key, "b")).is_none(), "b was LRU");

        let first = cache.get(&key(&generation_key, "a")).expect("a resident");
        let second = cache.get(&key(&generation_key, "a")).expect("a resident");
        assert!(Arc::ptr_eq(&first, &second), "a hit must share, not clone");

        let mut tiny =
            RegexMatchCache::new(RegexMatchCachePolicy::new(8, one_entry - 1, 8).expect("policy"));
        assert_eq!(
            tiny.insert(key(&generation_key, "a"), members(&[1])),
            Err(RegexMatchCacheRefusal::Bytes { bytes: one_entry })
        );
        assert_eq!(tiny.stats().refused_bytes, 1);
    }

    /// Completion criterion #1 at the cache: the declared byte cap holds.
    ///
    /// 128 regexes, each matching a million documents, repeated, never hold
    /// more than the cap, and what is resident is exactly what the resident
    /// entries cost.
    ///
    /// The matches are every other document, so each set lives in dense
    /// containers (about a quarter mebibyte) and 128 of them are several
    /// times the cap: eviction, not a compact encoding, is what holds it.
    #[test]
    fn a_million_member_sets_stay_under_the_declared_byte_cap() {
        let generation_key = generation(1);
        let cap: u64 = 4 * 1024 * 1024;
        let policy = RegexMatchCachePolicy::new(128, cap, 2_000_000).expect("policy");
        let mut cache = RegexMatchCache::new(policy);
        let matches = Arc::new(every_other(0, 1_000_000));
        assert!(match_set_bytes(&matches) * 128 > 4 * cap);
        for round in 0..2 {
            for index in 0..128 {
                let entry_key = key(&generation_key, &format!("regex-{index}"));
                if cache.get(&entry_key).is_none() {
                    cache.record_built(&matches);
                    cache
                        .insert(entry_key, Arc::clone(&matches))
                        .expect("one set fits the cap");
                }
                let stats = cache.stats();
                assert!(
                    stats.resident_bytes <= cap,
                    "round {round} regex {index}: {} > {cap}",
                    stats.resident_bytes
                );
            }
        }
        let stats = cache.stats();
        assert!(stats.evictions > 0, "{stats:?}");
        let resident: u64 = (0..128)
            .map(|index| key(&generation_key, &format!("regex-{index}")))
            .filter_map(|entry_key| {
                cache
                    .entries
                    .get(&entry_key)
                    .map(|entry| entry_bytes(&entry_key, &entry.members))
            })
            .sum();
        assert_eq!(stats.resident_bytes, resident);
        assert!(stats.entries >= 1 && stats.entries < 128, "{stats:?}");
        assert_eq!(stats.members_built, stats.sets_built * 1_000_000);
    }

    /// A set a query holds outlives its eviction and its generation's
    /// invalidation.
    #[test]
    fn a_held_set_survives_eviction_and_invalidation() {
        let generation_key = generation(1);
        let policy = RegexMatchCachePolicy::new(1, 1 << 20, 8).expect("policy");
        let mut cache = RegexMatchCache::new(policy);
        cache
            .insert(key(&generation_key, "a"), members(&[4, 5]))
            .expect("fits");
        let held = cache.get(&key(&generation_key, "a")).expect("resident");
        cache
            .insert(key(&generation_key, "b"), members(&[6]))
            .expect("evicts a");
        assert!(cache.get(&key(&generation_key, "a")).is_none());
        cache.invalidate_generation(&generation_key);
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(held.iter().collect::<Vec<_>>(), vec![4, 5]);
    }
}
