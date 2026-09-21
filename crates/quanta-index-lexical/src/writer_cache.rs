//! The bounded cache of open generation writers and the heap envelope they share (QI-BB-016).

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::index_store::open_or_create_index;
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
    pub(crate) fn release(&mut self, key: &GenKey, why: WriterRelease) -> Result<(), CoreError> {
        let Some(victim) = self.remove(key) else {
            return Err(CoreError::Storage(
                "lexical writer cache: order references missing entry".to_string(),
            ));
        };
        match why {
            WriterRelease::Seal => {
                let Ok(owned) = Arc::try_unwrap(victim) else {
                    return Err(CoreError::Storage(format!(
                        "lexical: seal of generation {} while a build still holds its writer",
                        key.generation.get()
                    )));
                };
                let GenerationWriter { index, mut writer } = owned.into_inner().map_err(|err| {
                    CoreError::Storage(format!("lexical: release lock poisoned: {err}"))
                })?;
                let _opstamp = writer
                    .commit()
                    .map_err(|err| CoreError::Storage(format!("lexical: seal commit: {err}")))?;
                writer.wait_merging_threads().map_err(|err| {
                    CoreError::Storage(format!("lexical: seal wait for merges: {err}"))
                })?;
                drop(index);
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
        Ok(())
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
            self.release(&victim_key, WriterRelease::Lru)?;
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
            self.release(&key, WriterRelease::Idle)?;
        }
        Ok(released)
    }

    /// Returns the cached handle for `key`, opening and inserting a new one if
    /// absent. The returned handle is the entry's `Arc<Mutex<_>>`; the cache
    /// retains its own clone so subsequent calls hit the same writer.
    pub(crate) fn get_or_open(
        &mut self,
        key: &GenKey,
        fields: &SchemaFields,
        path: &Path,
        now: Instant,
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
        let index = open_or_create_index(fields, path)?;
        let heap_bytes = usize::try_from(self.policy.writer_heap_bytes()).map_err(|err| {
            CoreError::Storage(format!("lexical: writer heap does not fit this platform: {err}"))
        })?;
        let writer: IndexWriter = index
            .writer_with_num_threads(writer_threads_for_heap(heap_bytes), heap_bytes)
            .map_err(|err| CoreError::Storage(format!("lexical: writer: {err}")))?;
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
        .map_or(1, |minimum| heap_bytes.checked_div(minimum).map_or(1, |threads| threads))
        .max(1);
    let by_machine = std::thread::available_parallelism().map_or(1, usize::from);
    by_heap.min(by_machine).min(WRITER_THREADS_MAX)
}
