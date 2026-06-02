#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Semantic adapter — persisted, generation-scoped vector index backed by the
//! `lancedb` crate (LDB-00 §3.2 revised decision).
//!
//! Implements the batch-native semantic build/open ports from
//! `quanta-index-core::domains::semantic` against a real lancedb dataset under
//! `{state_root}/indexes/semantic/{repo}/{revision}/g{generation}/`. The
//! adapter owns a `tokio::runtime::Runtime` and bridges async lancedb calls to
//! the sync port surface via `Runtime::block_on` — this adapter *is* the
//! deliberate async↔sync seam (the `disallowed_methods` rule against
//! `block_on` is honored by a single, narrowly-scoped `#[expect]` on the
//! crate-private [`run_blocking`] helper — the only `block_on` call site in
//! the crate; both the build and query paths funnel through it).
//!
//! A sealed generation is built once (lancedb table + ANN index + scope
//! manifest + READY/SEALED markers) and opened directly from durable state at
//! query time. There is no boot-time replay. Vendor / file-layout knowledge
//! stays inside this crate; the public surface is the two ports plus
//! [`scan_persisted_generations`] for readiness seeding at the composition
//! root, and [`semantic_state_root`] so the composition root does not hardcode
//! the layout root.
//!
//! See `docs/plans/may-28-lancedb-adoption/` for the full backend decision and
//! migration packet.

mod build;
mod codec;
mod generation_contract;
mod layout;
mod manifest;
mod search;
mod sql;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{
    CoreError, SemanticBatchBuildPort, SemanticIndexOpenPort, domains::semantic::SemanticSearcher,
};

use crate::manifest::SemanticManifest;
use crate::search::{LoadedGeneration, PersistedSemanticSearcher, open_generation};

#[cfg(debug_assertions)]
pub mod test_support {
    /// Debug-only test hook for injected append failure rails.
    pub fn set_append_fail_path(path: Option<&str>) {
        crate::build::set_append_fail_path_for_debug(path);
    }
}

/// Capacity of the opened-generation cache.
const OPEN_CACHE_CAPACITY: usize = 8;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct GenKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
}

/// Bounded cache of opened sealed generations, keyed by `(repo, revision,
/// generation)`.
///
/// Sealed generations are immutable, so a content-keyed entry can never go
/// stale — a fresh process reopens the lancedb dataset from disk and gets
/// identical results, and eviction only costs a reopen. Eviction is FIFO (not
/// LRU) on purpose: a cache *hit* takes only a read lock (so concurrent opens
/// of a hot generation proceed in parallel), whereas LRU recency tracking
/// would force a write lock on every hit.
#[derive(Default)]
struct OpenCache {
    entries: BTreeMap<GenKey, Arc<LoadedGeneration>>,
    order: Vec<GenKey>,
}

impl OpenCache {
    fn get(&self, key: &GenKey) -> Option<Arc<LoadedGeneration>> {
        self.entries.get(key).map(Arc::clone)
    }

    fn insert(&mut self, key: GenKey, value: Arc<LoadedGeneration>) {
        if let std::collections::btree_map::Entry::Occupied(mut slot) =
            self.entries.entry(key.clone())
        {
            let _prior = slot.insert(value);
            return;
        }
        if self.order.len() >= OPEN_CACHE_CAPACITY && !self.order.is_empty() {
            let evicted = self.order.remove(0);
            let _removed = self.entries.remove(&evicted);
        }
        self.order.push(key.clone());
        let _prior = self.entries.insert(key, value);
    }
}

/// Persisted semantic adapter rooted at a semantic state directory.
///
/// The state directory is typically `{state_root}/indexes/semantic` at the
/// composition root. Owns a tokio runtime used to drive lancedb's async API
/// behind the sync port surface.
pub struct SemanticAdapter {
    state_root: PathBuf,
    runtime: Arc<tokio::runtime::Runtime>,
    cache: RwLock<OpenCache>,
}

/// Single crate-wide async↔sync seam funnel.
///
/// Every entry from a sync port (`SemanticBatchBuildPort` /
/// `SemanticIndexOpenPort` / `SemanticSearcher`) into the async lancedb crate
/// routes through this one helper. Both the build path (held by
/// `SemanticAdapter`) and the query path (held by `PersistedSemanticSearcher`)
/// call through it, so the workspace `disallowed_methods` exception is
/// localized to exactly one `#[expect]` site rather than mirrored across
/// adapter + searcher.
#[expect(
    clippy::disallowed_methods,
    reason = "the semantic adapter is the deliberate async↔sync seam between the sync port surface and the async lancedb crate; the entire crate funnels through this single helper"
)]
pub(crate) fn run_blocking<F: core::future::Future>(
    runtime: &tokio::runtime::Runtime,
    future: F,
) -> F::Output {
    runtime.block_on(future)
}

impl SemanticAdapter {
    /// Returns `Err` if the tokio runtime cannot be constructed (a rare system-
    /// resource failure). The construction would otherwise have to panic, which
    /// the workspace lint regime forbids.
    pub fn with_state_root(state_root: PathBuf) -> Result<Self, CoreError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("quanta-index-semantic-lancedb")
            .build()
            .map_err(|err| {
                CoreError::Storage(format!(
                    "semantic: tokio runtime init for lancedb adapter: {err}"
                ))
            })?;
        Ok(Self {
            state_root,
            runtime: Arc::new(runtime),
            cache: RwLock::new(OpenCache::default()),
        })
    }

    fn cache_get(&self, key: &GenKey) -> Result<Option<Arc<LoadedGeneration>>, CoreError> {
        Ok(self
            .cache
            .read()
            .map_err(|err| CoreError::Storage(format!("semantic open cache poisoned: {err}")))?
            .get(key))
    }

    fn cache_put(&self, key: GenKey, value: Arc<LoadedGeneration>) -> Result<(), CoreError> {
        self.cache
            .write()
            .map_err(|err| CoreError::Storage(format!("semantic open cache poisoned: {err}")))?
            .insert(key, value);
        Ok(())
    }
}

impl SemanticBatchBuildPort for SemanticAdapter {
    fn build_batch(
        &self,
        batch: &quanta_index_contract::SemanticIngestBatch,
    ) -> Result<(), CoreError> {
        run_blocking(&self.runtime, build::build_batch(&self.state_root, batch))
    }
}

impl SemanticIndexOpenPort for SemanticAdapter {
    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
        let key = GenKey {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
        };
        if let Some(loaded) = self.cache_get(&key)? {
            return Ok(Box::new(PersistedSemanticSearcher::new(
                loaded,
                Arc::clone(&self.runtime),
            )));
        }
        let loaded = Arc::new(run_blocking(
            &self.runtime,
            open_generation(&self.state_root, repo, revision, generation),
        )?);
        self.cache_put(key, Arc::clone(&loaded))?;
        Ok(Box::new(PersistedSemanticSearcher::new(
            loaded,
            Arc::clone(&self.runtime),
        )))
    }
}

/// The semantic sub-root within an overall daemon state root.
///
/// This crate owns the semantic on-disk layout, including where its generation
/// tree is rooted, so the composition root derives the path here rather than
/// hardcoding `indexes/semantic` itself.
#[must_use]
pub fn semantic_state_root(state_root: &Path) -> PathBuf {
    state_root.join("indexes").join("semantic")
}

/// A sealed semantic generation discovered on disk, for readiness seeding.
///
/// `#[non_exhaustive]` reserves the right to add fields without a breaking
/// change at this public surface: this struct is **produced** by
/// [`scan_persisted_generations`] and **consumed** by the composition root's
/// readiness seeder, both of which already match on named fields. Adding e.g.
/// a `materialized_at` or `index_state` later then does not break callers.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PersistedSemanticGeneration {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub manifest_digest: String,
}

/// Scan a semantic state root for sealed generations.
///
/// Returns one record per `(repo, revision, generation)` directory that carries
/// a SEALED marker and a scope-consistent manifest. In-progress (materialized
/// but unsealed) generations are intentionally skipped so a crashed build never
/// seeds false readiness. The composition root feeds these into the readiness
/// ledger; all file-layout knowledge stays inside this crate.
pub fn scan_persisted_generations(
    semantic_root: &Path,
) -> Result<Vec<PersistedSemanticGeneration>, CoreError> {
    let mut out: Vec<PersistedSemanticGeneration> = Vec::new();
    if !semantic_root.exists() {
        return Ok(out);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_name("quanta-index-semantic-scan")
        .build()
        .map_err(|err| {
            CoreError::Storage(format!(
                "semantic: tokio runtime init for persisted-generation scan: {err}"
            ))
        })?;
    for repo_entry in read_dir(semantic_root)? {
        let repo_entry = dir_entry(repo_entry)?;
        if !is_dir(&repo_entry)? {
            continue;
        }
        let repo_id = RepoId::new(repo_entry.file_name().to_string_lossy().into_owned());
        for revision_entry in read_dir(&repo_entry.path())? {
            let revision_entry = dir_entry(revision_entry)?;
            if !is_dir(&revision_entry)? {
                continue;
            }
            let revision_id =
                RevisionId::new(revision_entry.file_name().to_string_lossy().into_owned());
            for generation_entry in read_dir(&revision_entry.path())? {
                let generation_entry = dir_entry(generation_entry)?;
                if !is_dir(&generation_entry)? {
                    continue;
                }
                let name = generation_entry.file_name().to_string_lossy().into_owned();
                let Some(suffix) = name.strip_prefix('g') else {
                    continue;
                };
                let Ok(raw_generation) = suffix.parse::<u64>() else {
                    continue;
                };
                let generation = ManifestGeneration::new(raw_generation);
                let generation_dir = generation_entry.path();
                if !layout::sealed_marker_path(&generation_dir).exists() {
                    continue;
                }
                let manifest_path = layout::manifest_path(&generation_dir);
                let manifest_bytes = std::fs::read(&manifest_path).map_err(|err| {
                    CoreError::Storage(format!(
                        "semantic: read manifest {}: {err}",
                        manifest_path.display()
                    ))
                })?;
                let manifest = SemanticManifest::decode(&manifest_bytes)?;
                manifest.validate_scope(&repo_id, &revision_id, generation)?;
                let _loaded = run_blocking(
                    &runtime,
                    open_generation(semantic_root, &repo_id, &revision_id, generation),
                )?;
                out.push(PersistedSemanticGeneration {
                    repo_id: repo_id.clone(),
                    revision_id: revision_id.clone(),
                    generation,
                    manifest_digest: manifest.manifest_digest,
                });
            }
        }
    }
    Ok(out)
}

fn read_dir(path: &Path) -> Result<std::fs::ReadDir, CoreError> {
    std::fs::read_dir(path)
        .map_err(|err| CoreError::Storage(format!("semantic: list {}: {err}", path.display())))
}

fn dir_entry(entry: std::io::Result<std::fs::DirEntry>) -> Result<std::fs::DirEntry, CoreError> {
    entry.map_err(|err| CoreError::Storage(format!("semantic: read directory entry: {err}")))
}

fn is_dir(entry: &std::fs::DirEntry) -> Result<bool, CoreError> {
    let file_type = entry.file_type().map_err(|err| {
        CoreError::Storage(format!(
            "semantic: file type {}: {err}",
            entry.path().display()
        ))
    })?;
    Ok(file_type.is_dir())
}
