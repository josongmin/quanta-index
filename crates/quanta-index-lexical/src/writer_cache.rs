//! The bounded cache of open generation writers and the heap envelope they share (QI-BB-016).

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

#[cfg(test)]
use crate::index_store::open_or_create_index;
use crate::index_store::open_or_create_index_observed;
use crate::{
    CachedWriter, GenKey, GenerationWriter, SchemaFields, WRITER_THREADS_MAX, WriterCache,
    WriterRelease,
};
use quanta_index_core::{
    CoreError, LEXICAL_WRITER_HEAP_BYTES_MIN, LexicalWriterCacheStats, LexicalWriterPolicy,
    UnboundedWriterAdmission, count_from_usize,
};
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tantivy::IndexWriter;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WriterSealTimings {
    pub(crate) commit_ns: u64,
    pub(crate) merge_wait_ns: u64,
}

impl WriterCache {
    pub(crate) fn new(policy: LexicalWriterPolicy) -> Self {
        Self {
            entries: BTreeMap::new(),
            order: VecDeque::new(),
            policy,
            admission: Arc::new(UnboundedWriterAdmission),
            lru_releases: 0,
            idle_releases: 0,
            seal_releases: 0,
            quiescence_failed: false,
        }
    }

    /// Move `key` to the most-recently-used end of `order`. Caller must hold
    /// the cache mutex. Silently no-ops when `key` is absent.
    pub(crate) fn touch(&mut self, key: &GenKey) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            let removed = self.order.remove(pos);
            if let Some(k) = removed {
                self.order.push_back(k);
            }
        }
    }

    fn remove(&mut self, key: &GenKey) -> Option<Arc<Mutex<GenerationWriter>>> {
        self.order.retain(|candidate| candidate != key);
        self.entries.remove(key).map(|cached| cached.handle)
    }

    /// Discard an abandoned generation without publishing uncommitted rows.
    /// All indexing and merge workers must stop before its namespace is deleted.
    pub(crate) fn discard(&mut self, key: &GenKey) -> Result<bool, CoreError> {
        let Some(cached) = self.entries.get(key) else {
            return Ok(false);
        };
        if Arc::strong_count(&cached.handle) != 1 {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                message: format!(
                    "lexical: generation {} writer is still in use; retry discard",
                    key.generation.get()
                ),
            });
        }
        let victim = self.remove(key).ok_or_else(|| {
            CoreError::Storage("lexical writer cache: entry disappeared during discard".into())
        })?;
        let Ok(owned) = Arc::try_unwrap(victim) else {
            self.quiescence_failed = true;
            return Err(CoreError::Storage(
                "lexical: writer ownership changed during discard".into(),
            ));
        };
        let (owned, poisoned) = match owned.into_inner() {
            Ok(writer) => (writer, false),
            Err(error) => (error.into_inner(), true),
        };
        let GenerationWriter {
            index: _index,
            writer,
        } = owned;
        let joined = writer.wait_merging_threads();
        if let Err(error) = joined {
            self.quiescence_failed = true;
            return Err(CoreError::Storage(format!(
                "lexical: discard generation {} wait for merges: {error}",
                key.generation.get()
            )));
        }
        if poisoned {
            return Err(CoreError::Storage(format!(
                "lexical: discard generation {} writer mutex was poisoned",
                key.generation.get()
            )));
        }
        Ok(true)
    }

    /// Commit and join all background merges before the cache loses a writer.
    pub(crate) fn release(
        &mut self,
        key: &GenKey,
        why: WriterRelease,
    ) -> Result<WriterSealTimings, CoreError> {
        let Some(cached) = self.entries.get(key) else {
            return Err(CoreError::Storage(
                "lexical writer cache: order references missing entry".to_string(),
            ));
        };
        if Arc::strong_count(&cached.handle) != 1 {
            return match why {
                WriterRelease::Seal => Err(CoreError::Storage(format!(
                    "lexical: seal of generation {} while a build still holds its writer",
                    key.generation.get()
                ))),
                WriterRelease::Lru | WriterRelease::Idle => Err(CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    message: format!(
                        "lexical: generation {} writer is still in use; retry writer release",
                        key.generation.get()
                    ),
                }),
            };
        }
        let victim = self.remove(key).ok_or_else(|| {
            CoreError::Storage("lexical writer cache: entry disappeared during release".into())
        })?;
        let mut timings = WriterSealTimings::default();
        let Ok(owned) = Arc::try_unwrap(victim) else {
            self.quiescence_failed = true;
            return Err(CoreError::Storage(
                "lexical: writer ownership changed during release".into(),
            ));
        };
        let (owned, poisoned) = match owned.into_inner() {
            Ok(writer) => (writer, false),
            Err(error) => (error.into_inner(), true),
        };
        let GenerationWriter {
            index: _index,
            mut writer,
        } = owned;
        let commit_started = Instant::now();
        let committed = writer.commit();
        let commit_elapsed = crate::stage_timing::elapsed_stage_ns(commit_started);
        let merge_started = Instant::now();
        let merged = writer.wait_merging_threads();
        let merge_elapsed = crate::stage_timing::elapsed_stage_ns(merge_started);
        if merged.is_err() {
            self.quiescence_failed = true;
        }
        let label = match why {
            WriterRelease::Seal => "seal",
            WriterRelease::Lru => "LRU",
            WriterRelease::Idle => "idle",
        };
        match (committed, merged) {
            (Err(commit), Err(merge)) => {
                return Err(CoreError::Storage(format!(
                    "lexical: {label} commit failed ({commit}); merge join failed ({merge})"
                )));
            }
            (Err(commit), Ok(())) => {
                return Err(CoreError::Storage(format!(
                    "lexical: {label} commit: {commit}"
                )));
            }
            (Ok(_opstamp), Err(merge)) => {
                return Err(CoreError::Storage(format!(
                    "lexical: {label} wait for merges: {merge}"
                )));
            }
            (Ok(_opstamp), Ok(())) => {}
        }
        if poisoned {
            return Err(CoreError::Storage(format!(
                "lexical: {label} writer mutex was poisoned"
            )));
        }
        timings.commit_ns = commit_elapsed?;
        timings.merge_wait_ns = merge_elapsed?;
        match why {
            WriterRelease::Lru => self.lru_releases = self.lru_releases.saturating_add(1),
            WriterRelease::Idle => self.idle_releases = self.idle_releases.saturating_add(1),
            WriterRelease::Seal => self.seal_releases = self.seal_releases.saturating_add(1),
        }
        Ok(timings)
    }

    /// Release the least-recently-used writers until there is room for one
    /// more under the envelope.
    pub(crate) fn release_until_room(&mut self) -> Result<(), CoreError> {
        let max_writers = self.policy.max_writers();
        while self.entries.len() >= max_writers {
            let Some(victim_key) = self
                .order
                .iter()
                .find(|key| {
                    self.entries
                        .get(*key)
                        .is_some_and(|cached| Arc::strong_count(&cached.handle) == 1)
                })
                .cloned()
            else {
                // entries and order are kept in lock-step; an empty order with
                // non-empty entries would be a structural bug. All-held entries
                // instead require the caller to retry after their builds finish.
                if self.order.is_empty() {
                    return Err(CoreError::Storage(
                        "lexical writer cache: order/entries desync during release".into(),
                    ));
                }
                return Err(CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    message: "lexical: all cached writers are held by active builds".into(),
                });
            };
            let _timings = self.release(&victim_key, WriterRelease::Lru)?;
        }
        Ok(())
    }

    /// Release every writer nothing has touched for the idle interval;
    /// returns how many were released.
    pub(crate) fn release_idle(&mut self, now: Instant) -> Result<u64, CoreError> {
        let idle_after = self.policy.idle_after();
        let idle: Vec<GenKey> = self
            .entries
            .iter()
            .filter(|(_, cached)| {
                now.saturating_duration_since(cached.last_used) >= idle_after
                    && Arc::strong_count(&cached.handle) == 1
            })
            .map(|(key, _)| key.clone())
            .collect();
        let released = count_from_usize(idle.len());
        for key in idle {
            let _timings = self.release(&key, WriterRelease::Idle)?;
        }
        Ok(released)
    }

    /// A physical track census may proceed only after every cached writer has
    /// been joined and no failed join has left its background work uncertain.
    pub(crate) fn assert_physical_quiescent(&self) -> Result<(), CoreError> {
        if self.quiescence_failed {
            return Err(CoreError::Storage(
                "lexical: a writer merge join failed; physical track is not proved quiescent"
                    .into(),
            ));
        }
        if !self.entries.is_empty() {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
                message: "lexical: cached unsealed writers prevent a physical track census".into(),
            });
        }
        Ok(())
    }

    /// Returns the cached handle for `key`, opening and inserting a new one if
    /// absent. The returned handle is the entry's `Arc<Mutex<_>>`; the cache
    /// retains its own clone so subsequent calls hit the same writer.
    #[cfg(test)]
    pub(crate) fn get_or_open(
        &mut self,
        key: &GenKey,
        fields: &SchemaFields,
        path: &Path,
        now: Instant,
    ) -> Result<Arc<Mutex<GenerationWriter>>, CoreError> {
        self.get_or_open_observed(key, fields, path, now, None)
    }

    pub(crate) fn get_or_open_observed(
        &mut self,
        key: &GenKey,
        fields: &SchemaFields,
        path: &Path,
        now: Instant,
        sync: Option<&crate::causal_profile::SyncTotalsV1>,
    ) -> Result<Arc<Mutex<GenerationWriter>>, CoreError> {
        if let Some(existing) = self.entries.get_mut(key) {
            existing.last_used = now;
            let cloned = Arc::clone(&existing.handle);
            self.touch(key);
            return Ok(cloned);
        }
        let _released = self.release_idle(now)?;
        self.release_until_room()?;
        // The process-level gate (QI-BB-016): a writer is never opened while
        // the process is above its resident-memory ceiling.
        self.admission.admit_writer_open()?;
        let index = open_or_create_index_observed(fields, path, sync)?;
        let heap_bytes = usize::try_from(self.policy.writer_heap_bytes()).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: writer heap does not fit this platform: {err}"
            ))
        })?;
        let writer: IndexWriter = index
            .writer_with_num_threads(writer_threads_for_heap(heap_bytes), heap_bytes)
            .map_err(|err| CoreError::Storage(format!("lexical: writer: {err}")))?;
        if crate::generation_dir::read_lexical_delta_base(path)?.is_some() {
            // A delta keeps its inherited segment files: auto merge would
            // rewrite unchanged base bytes before seal. The seal still waits
            // for any native merge already in flight before committing.
            writer.set_merge_policy(Box::new(tantivy::indexer::NoMergePolicy));
        }
        let handle = Arc::new(Mutex::new(GenerationWriter { index, writer }));
        let _prior = self.entries.insert(
            key.clone(),
            CachedWriter {
                handle: Arc::clone(&handle),
                last_used: now,
            },
        );
        self.order.push_back(key.clone());
        Ok(handle)
    }

    pub(crate) fn stats(&self) -> LexicalWriterCacheStats {
        let open_writers = self.entries.len();
        LexicalWriterCacheStats {
            open_writers,
            max_writers: self.policy.max_writers(),
            allocated_heap_bytes: u64::try_from(open_writers).map_or(u64::MAX, |writers| {
                writers.saturating_mul(self.policy.writer_heap_bytes())
            }),
            lru_releases: self.lru_releases,
            idle_releases: self.idle_releases,
            seal_releases: self.seal_releases,
        }
    }
}

/// Indexing threads for one writer of `heap_bytes`.
///
/// As many as the heap gives the minimum arena to, capped by the machine
/// and by the writer's own ceiling — the derivation the library applies,
/// made explicit so the envelope's per-writer term is the whole story.
pub(crate) fn writer_threads_for_heap(heap_bytes: usize) -> usize {
    let by_heap = usize::try_from(LEXICAL_WRITER_HEAP_BYTES_MIN)
        .map_or(1, |minimum| {
            heap_bytes.checked_div(minimum).map_or(1, |threads| threads)
        })
        .max(1);
    let by_machine = std::thread::available_parallelism().map_or(1, usize::from);
    by_heap.min(by_machine).min(WRITER_THREADS_MAX)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
    use tantivy::doc;

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    fn key(generation: u64) -> Result<GenKey, Box<dyn Error>> {
        Ok(GenKey {
            repo_id: RepoId::new("envelope-repo")?,
            revision_id: RevisionId::new("envelope-rev")?,
            generation: ManifestGeneration::new(generation),
        })
    }

    fn path(root: &Path, generation: u64) -> std::path::PathBuf {
        root.join(format!("generation-{generation}"))
    }

    fn add_row(
        cache: &mut WriterCache,
        fields: &SchemaFields,
        root: &Path,
        generation: u64,
        now: Instant,
    ) -> TestResult {
        let handle = cache.get_or_open(&key(generation)?, fields, &path(root, generation), now)?;
        {
            let writer = handle
                .lock()
                .map_err(|error| format!("test writer lock poisoned: {error}"))?;
            let _opstamp = writer.writer.add_document(tantivy::doc!(
                fields.candidate_id => format!("candidate-{generation}"),
                fields.chunk_text => "envelope marker",
            ))?;
            drop(writer);
        }
        Ok(())
    }

    fn committed_rows(
        fields: &SchemaFields,
        root: &Path,
        generation: u64,
    ) -> Result<u64, Box<dyn Error>> {
        let index = open_or_create_index(fields, &path(root, generation))?;
        let reader = index.reader()?;
        Ok(reader.searcher().num_docs())
    }

    #[test]
    fn envelope_evicts_lru_only_after_committing_its_rows() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let fields = SchemaFields::build();
        let policy = LexicalWriterPolicy::new(
            3 * LEXICAL_WRITER_HEAP_BYTES_MIN,
            LEXICAL_WRITER_HEAP_BYTES_MIN,
            Duration::from_secs(3600),
        )?;
        let mut cache = WriterCache::new(policy);
        let start = Instant::now();
        if policy.max_writers() != 3 {
            return Err("test policy does not hold three writers".into());
        }
        for generation in 0..5 {
            add_row(&mut cache, &fields, &root, generation, start)?;
            let stats = cache.stats();
            if stats.open_writers > stats.max_writers
                || stats.allocated_heap_bytes > policy.envelope_bytes()
                || stats.max_writers != 3
            {
                return Err(format!(
                    "writer envelope exceeded at generation {generation}: {stats:?}"
                )
                .into());
            }
        }
        let stats = cache.stats();
        if (stats.open_writers, stats.lru_releases, stats.idle_releases) != (3, 2, 0) {
            return Err(
                format!("five opens through three slots did not evict two: {stats:?}").into(),
            );
        }
        for generation in 0..2 {
            if committed_rows(&fields, &root, generation)? != 1 {
                return Err(
                    format!("evicted generation {generation} lost its committed row").into(),
                );
            }
        }
        add_row(&mut cache, &fields, &root, 0, start)?;
        let stats = cache.stats();
        if (stats.open_writers, stats.lru_releases) != (3, 3)
            || committed_rows(&fields, &root, 2)? != 1
        {
            return Err(format!("reopen did not evict and commit generation 2: {stats:?}").into());
        }
        Ok(())
    }

    #[test]
    fn idle_sweep_commits_rows_and_releases_writer_heap() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let fields = SchemaFields::build();
        let idle = Duration::from_millis(400);
        let policy = LexicalWriterPolicy::new(
            4 * LEXICAL_WRITER_HEAP_BYTES_MIN,
            LEXICAL_WRITER_HEAP_BYTES_MIN,
            idle,
        )?;
        let mut cache = WriterCache::new(policy);
        let start = Instant::now();
        add_row(&mut cache, &fields, &root, 1, start)?;
        add_row(&mut cache, &fields, &root, 2, start)?;
        if cache.release_idle(start + idle + idle)? != 2 {
            return Err("idle sweep did not release both writers".into());
        }
        let stats = cache.stats();
        if (
            stats.open_writers,
            stats.idle_releases,
            stats.allocated_heap_bytes,
        ) != (0, 2, 0)
        {
            return Err(format!("idle sweep retained writer heap: {stats:?}").into());
        }
        for generation in [1, 2] {
            if committed_rows(&fields, &root, generation)? != 1 {
                return Err(format!("idle generation {generation} lost its committed row").into());
            }
        }
        add_row(&mut cache, &fields, &root, 3, start + idle + idle)?;
        add_row(
            &mut cache,
            &fields,
            &root,
            4,
            start + idle + idle + idle + idle,
        )?;
        let stats = cache.stats();
        if (stats.open_writers, stats.idle_releases) != (1, 3)
            || committed_rows(&fields, &root, 3)? != 1
        {
            return Err(
                format!("open-time idle sweep did not commit generation 3: {stats:?}").into(),
            );
        }
        Ok(())
    }

    #[test]
    fn held_writer_is_neither_idled_nor_evicted_and_blocks_physical_census() -> TestResult {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let fields = SchemaFields::build();
        let idle = Duration::from_millis(1);
        let policy = LexicalWriterPolicy::new(
            2 * LEXICAL_WRITER_HEAP_BYTES_MIN,
            LEXICAL_WRITER_HEAP_BYTES_MIN,
            idle,
        )?;
        let mut cache = WriterCache::new(policy);
        let start = Instant::now();
        let held = cache.get_or_open(&key(1)?, &fields, &path(&root, 1), start)?;
        add_row(&mut cache, &fields, &root, 2, start)?;
        check!(matches!(
            cache.assert_physical_quiescent(),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
                ..
            })
        ));
        check_eq!(cache.release_idle(start + idle + idle)?, 1);
        check_eq!(cache.stats().open_writers, 1);
        check!(cache.entries.contains_key(&key(1)?));
        check_eq!(committed_rows(&fields, &root, 2)?, 1);
        drop(held);
        check_eq!(cache.release_idle(start + idle + idle)?, 1);
        cache.assert_physical_quiescent()?;
        Ok(())
    }

    #[test]
    fn lru_skips_held_writer_and_refuses_when_all_slots_are_held() -> TestResult {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let fields = SchemaFields::build();
        let policy = LexicalWriterPolicy::new(
            2 * LEXICAL_WRITER_HEAP_BYTES_MIN,
            LEXICAL_WRITER_HEAP_BYTES_MIN,
            Duration::from_secs(3600),
        )?;
        let mut cache = WriterCache::new(policy);
        let start = Instant::now();
        let first = cache.get_or_open(&key(1)?, &fields, &path(&root, 1), start)?;
        let _opstamp = first
            .lock()
            .map_err(|error| format!("test writer lock poisoned: {error}"))?
            .writer
            .add_document(tantivy::doc!(fields.candidate_id => "held-first"))?;
        add_row(&mut cache, &fields, &root, 2, start)?;
        add_row(&mut cache, &fields, &root, 3, start)?;
        check!(cache.entries.contains_key(&key(1)?));
        check!(!cache.entries.contains_key(&key(2)?));
        check_eq!(committed_rows(&fields, &root, 2)?, 1);
        let third = cache.get_or_open(&key(3)?, &fields, &path(&root, 3), start)?;
        let _opstamp = third
            .lock()
            .map_err(|error| format!("test writer lock poisoned: {error}"))?
            .writer
            .add_document(tantivy::doc!(fields.candidate_id => "held-third"))?;
        let refused = cache.get_or_open(&key(4)?, &fields, &path(&root, 4), start);
        check!(matches!(
            refused,
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                ..
            })
        ));
        check_eq!(cache.stats().open_writers, 2);
        check!(!path(&root, 4).exists());
        drop(first);
        drop(third);
        let _timings = cache.release(&key(1)?, WriterRelease::Seal)?;
        let _timings = cache.release(&key(3)?, WriterRelease::Seal)?;
        cache.assert_physical_quiescent()?;
        check_eq!(committed_rows(&fields, &root, 1)?, 1);
        check_eq!(committed_rows(&fields, &root, 3)?, 1);
        Ok(())
    }

    #[test]
    fn discard_waits_for_writer_workers_without_committing_abandoned_rows() -> TestResult {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let fields = SchemaFields::build();
        let policy = LexicalWriterPolicy::new(
            LEXICAL_WRITER_HEAP_BYTES_MIN,
            LEXICAL_WRITER_HEAP_BYTES_MIN,
            Duration::from_secs(3600),
        )?;
        let mut cache = WriterCache::new(policy);
        let generation = key(1)?;
        let held = cache.get_or_open(&generation, &fields, &path(&root, 1), Instant::now())?;
        {
            let mut writer = held
                .lock()
                .map_err(|error| format!("test writer lock poisoned: {error}"))?;
            let _opstamp = writer
                .writer
                .add_document(tantivy::doc!(fields.candidate_id => "committed"))?;
            let _committed = writer.writer.commit()?;
            let _opstamp = writer
                .writer
                .add_document(tantivy::doc!(fields.candidate_id => "abandoned"))?;
            drop(writer);
        }
        check!(matches!(
            cache.discard(&generation),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                ..
            })
        ));
        check_eq!(cache.stats().open_writers, 1);
        drop(held);
        check!(cache.discard(&generation)?);
        check!(!cache.discard(&generation)?);
        cache.assert_physical_quiescent()?;
        check_eq!(committed_rows(&fields, &root, 1)?, 1);
        Ok(())
    }
}
