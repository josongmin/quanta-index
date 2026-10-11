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

    pub(crate) fn remove(&mut self, key: &GenKey) -> Option<Arc<Mutex<GenerationWriter>>> {
        self.order.retain(|candidate| candidate != key);
        self.entries.remove(key).map(|cached| cached.handle)
    }

    /// Commit and drop one writer, counting why.
    ///
    /// A seal release also waits for the writer's background merges: a
    /// merge that finished after the seal measured the directory would
    /// rewrite `meta.json` behind the manifest, so the seal's commit is
    /// only final once no merge is in flight. That needs the writer by
    /// value, which is only possible when no in-flight build still holds
    /// it — for a seal, a structural guarantee the batch order gives.
    pub(crate) fn release(
        &mut self,
        key: &GenKey,
        why: WriterRelease,
    ) -> Result<WriterSealTimings, CoreError> {
        let Some(victim) = self.remove(key) else {
            return Err(CoreError::Storage(
                "lexical writer cache: order references missing entry".to_string(),
            ));
        };
        let mut timings = WriterSealTimings::default();
        match why {
            WriterRelease::Seal => {
                let Ok(owned) = Arc::try_unwrap(victim) else {
                    return Err(CoreError::Storage(format!(
                        "lexical: seal of generation {} while a build still holds its writer",
                        key.generation.get()
                    )));
                };
                let GenerationWriter {
                    index: _index,
                    mut writer,
                } = owned.into_inner().map_err(|err| {
                    CoreError::Storage(format!("lexical: release lock poisoned: {err}"))
                })?;
                let commit_started = Instant::now();
                let _opstamp = writer
                    .commit()
                    .map_err(|err| CoreError::Storage(format!("lexical: seal commit: {err}")))?;
                timings.commit_ns = crate::stage_timing::elapsed_stage_ns(commit_started)?;
                let merge_started = Instant::now();
                writer.wait_merging_threads().map_err(|err| {
                    CoreError::Storage(format!("lexical: seal wait for merges: {err}"))
                })?;
                timings.merge_wait_ns = crate::stage_timing::elapsed_stage_ns(merge_started)?;
            }
            WriterRelease::Lru | WriterRelease::Idle => {
                // If a concurrent build still holds the Arc this lock contends;
                // that is acceptable because the cap is small and contention
                // only happens on release, not on the hot path.
                let mut guarded = victim.lock().map_err(|err| {
                    CoreError::Storage(format!("lexical: release lock poisoned: {err}"))
                })?;
                let _opstamp = guarded
                    .writer
                    .commit()
                    .map_err(|err| CoreError::Storage(format!("lexical: release commit: {err}")))?;
                drop(guarded);
                drop(victim);
            }
        }
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
            let Some(victim_key) = self.order.front().cloned() else {
                // entries and order are kept in lock-step; an empty order with
                // non-empty entries would be a structural bug.
                return Err(CoreError::Storage(
                    "lexical writer cache: order/entries desync during release".to_string(),
                ));
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
            .filter(|(_, cached)| now.saturating_duration_since(cached.last_used) >= idle_after)
            .map(|(key, _)| key.clone())
            .collect();
        let released = count_from_usize(idle.len());
        for key in idle {
            let _timings = self.release(&key, WriterRelease::Idle)?;
        }
        Ok(released)
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
}
