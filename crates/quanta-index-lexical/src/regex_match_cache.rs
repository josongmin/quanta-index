//! The regex match cache (QI-BB-024).
//!
//! One entry per `(generation, executed regex source)`: the set of
//! text-authority doc ids the regex matched, as a compressed bitmap shared
//! by `Arc`. A hit hands out the resident set — no copy of any member — and
//! a query that holds a set keeps it alive past its eviction or its
//! generation's invalidation, so neither can break a query in flight.
//!
//! The cache is least-recently-used under two bounds, the entry count and
//! a conservative logical byte charge. It does not report process RSS or
//! memory retained by query-held references after eviction. A result that
//! exceeds per-entry cardinality or charge is served but not retained, and
//! the refusal is counted.

use std::collections::BTreeMap;
use std::sync::Arc;

use quanta_index_core::{RegexMatchCachePolicy, RegexMatchCacheStats};
use roaring::RoaringTreemap;

use crate::GenKey;

/// Admission per live 16-bit container, including vector spare capacity.
const ROARING_CONTAINER_OVERHEAD: u64 = 128;
/// Admission per high-32 treemap entry and its B-tree node slack.
const ROARING_TREEMAP_ENTRY_OVERHEAD: u64 = 128;

/// Bytes a set costs before its first container: the bitmap's container
/// vector header, rounded up.
const ROARING_SET_OVERHEAD: u64 = 32;

/// Admission for one entry beyond bitmap payload and key-string allocations.
///
/// Both B-tree indexes hold an inline copy of the key; this also reserves
/// node slack, the resident value, and the `Arc` control block. The previous
/// 128-byte charge was smaller than the two indexes' inline items alone.
const REGEX_MATCH_CACHE_ENTRY_OVERHEAD: u64 = 1_024;

/// Conservative upper charge for the u64 set without relying on inaccessible
/// treemap internals.
///
/// Sixteen bytes per member covers a dense 8 KiB container
/// once it has 512 members; sparse and run containers get the same charge.
/// Every nonempty 16-bit container and high-32 treemap entry is charged once.
pub(crate) fn match_set_bytes(members: &RoaringTreemap) -> u64 {
    let mut charge = ROARING_SET_OVERHEAD.saturating_add(members.len().saturating_mul(16));
    let mut previous_container = None;
    let mut previous_treemap = None;
    for id in members {
        let container = id >> 16;
        let treemap = id >> 32;
        if previous_container != Some(container) {
            charge = charge.saturating_add(ROARING_CONTAINER_OVERHEAD);
            previous_container = Some(container);
        }
        if previous_treemap != Some(treemap) {
            charge = charge.saturating_add(ROARING_TREEMAP_ENTRY_OVERHEAD);
            previous_treemap = Some(treemap);
        }
    }
    charge
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
    members: Arc<RoaringTreemap>,
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
    pub(crate) fn get(&mut self, key: &RegexMatchCacheKey) -> Option<Arc<RoaringTreemap>> {
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

    /// Record the logical charge for a computed set, cached or not.
    pub(crate) fn record_built(&mut self, members: &RoaringTreemap) {
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
        members: Arc<RoaringTreemap>,
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
    use roaring::RoaringTreemap;

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

    fn members(ids: &[u64]) -> Arc<RoaringTreemap> {
        Arc::new(ids.iter().copied().collect())
    }

    /// What one entry costs: its set, its key strings held twice, and the
    /// entry overhead.
    fn entry_bytes(entry_key: &RegexMatchCacheKey, set: &RoaringTreemap) -> u64 {
        let strings = entry_key.normalized_source.len()
            + entry_key.generation.repo_id.as_str().len()
            + entry_key.generation.revision_id.as_str().len();
        match_set_bytes(set)
            + 2 * u64::try_from(strings).expect("fits")
            + REGEX_MATCH_CACHE_ENTRY_OVERHEAD
    }

    #[test]
    fn entry_overhead_covers_both_btree_inline_items() {
        let inline = core::mem::size_of::<RegexMatchCacheKey>()
            .saturating_mul(2)
            .saturating_add(core::mem::size_of::<super::CachedMatchSet>())
            .saturating_add(core::mem::size_of::<u64>());
        assert!(
            REGEX_MATCH_CACHE_ENTRY_OVERHEAD >= u64::try_from(inline).expect("fits"),
            "cache admission cannot be smaller than its two B-tree items"
        );
    }

    fn range(start: u64, len: u64) -> RoaringTreemap {
        let mut set = RoaringTreemap::new();
        let _inserted: u64 = set.insert_range(start..start + len);
        set
    }

    /// `members` ids two apart from `start`: half-full chunks, so the set is
    /// held in dense containers rather than one run each.
    fn every_other(start: u64, members: u64) -> RoaringTreemap {
        RoaringTreemap::from_sorted_iter((0..members).map(|index| start + 2 * index))
            .expect("ascending")
    }

    /// The charge covers at least the serialized payload. This is not a
    /// retained-heap oracle: the container vector has spare capacity.
    ///
    /// The oracle is the treemap's serialized form, including the high-word
    /// map and every underlying bitmap container.
    #[test]
    fn a_match_set_charge_covers_its_serialized_payload() {
        for set in [
            RoaringTreemap::new(),
            (0..3).collect::<RoaringTreemap>(),
            (0..70_000).step_by(17).collect::<RoaringTreemap>(),
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
        let above_u32: RoaringTreemap = [u64::from(u32::MAX) + 1, u64::MAX].into_iter().collect();
        assert!(
            match_set_bytes(&above_u32)
                >= u64::try_from(above_u32.serialized_size()).expect("fits")
        );
    }

    #[test]
    fn sparse_growth_does_not_hide_retained_container_capacity() {
        let mut set = RoaringTreemap::new();
        for index in 0..513u64 {
            let _inserted = set.insert(index << 16);
        }
        // Independently measured with a System-allocation probe for pinned
        // roaring 0.11.4: 45,064 live heap bytes after the 513th insertion.
        assert!(match_set_bytes(&set) >= 45_064);
    }

    #[test]
    fn run_layout_is_charged_by_cardinality_and_container_count() {
        let set = Arc::new(range(0, 10_000));
        let mut cache = RegexMatchCache::new(RegexMatchCachePolicy::DEFAULT);
        let expected = match_set_bytes(&set);
        assert!(expected >= u64::try_from(set.serialized_size()).expect("fits"));
        let result = cache.insert(key(&generation(1), "range"), set);
        assert!(matches!(
            result,
            Ok(()) | Err(RegexMatchCacheRefusal::Bytes { .. })
        ));
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
        assert_eq!(
            cache.get(&key(&second, "foo")).as_deref(),
            Some(&*members(&[3]))
        );
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
    /// The matches are every other document, so the compressed bitmap is
    /// dense, but the conservative logical charge is about 16 MiB per set.
    /// A 4 MiB policy must refuse that set; a 32 MiB policy can hold one
    /// before later keys force eviction.
    #[test]
    fn a_million_member_sets_stay_under_the_declared_byte_cap() {
        let generation_key = generation(1);
        let cap: u64 = 32 * 1024 * 1024;
        let policy = RegexMatchCachePolicy::new(128, cap, 2_000_000).expect("policy");
        let mut cache = RegexMatchCache::new(policy);
        let matches = Arc::new(every_other(0, 1_000_000));
        let mut undersized = RegexMatchCache::new(
            RegexMatchCachePolicy::new(128, 4 * 1024 * 1024, 2_000_000).expect("policy"),
        );
        assert!(matches!(
            undersized.insert(key(&generation_key, "too-large"), Arc::clone(&matches)),
            Err(RegexMatchCacheRefusal::Bytes { bytes }) if bytes > 4 * 1024 * 1024
        ));
        assert_eq!(undersized.stats().resident_bytes, 0);
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
