#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Semantic adapter — persisted, generation-scoped vector index.
//!
//! Implements the batch-native semantic build/open ports from
//! `quanta-index-core::domains::semantic` against a durable on-disk store under
//! `{state_root}/indexes/semantic/{repo}/{revision}/g{generation}/`. A sealed
//! generation is built once (columnar rows + a persisted HNSW graph) and opened
//! directly from durable state — there is no boot-time replay and no rebuild of
//! the graph at open. Vendor / file-layout knowledge stays inside this crate;
//! the public surface is the two ports plus [`scan_persisted_generations`] for
//! readiness seeding at the composition root.
//!
//! See `docs/plans/may-28-lancedb-adoption/` for the backend decision (LDB-00):
//! "Lance" is the planning label for this durable shape; the bytes are an
//! in-house CBOR columnar shard, not the `lance` crate.

mod build;
mod codec;
mod dataset;
mod graph;
mod hnsw;
mod layout;
mod manifest;
mod search;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{
    CoreError, SemanticBatchBuildPort, SemanticIndexOpenPort, domains::semantic::SemanticSearcher,
};

use crate::manifest::SemanticManifest;
use crate::search::{LoadedGeneration, PersistedSemanticSearcher, open_generation};

/// Bounded cache of opened sealed generations. Sealed generations are immutable,
/// so a content-keyed cache can never go stale; a fresh process reloads from
/// durable state and gets identical results.
const OPEN_CACHE_CAPACITY: usize = 8;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct GenKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
}

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

/// Persisted semantic adapter rooted at a semantic state directory (typically
/// `{state_root}/indexes/semantic` at the composition root).
pub struct SemanticAdapter {
    state_root: PathBuf,
    cache: RwLock<OpenCache>,
}

impl SemanticAdapter {
    #[must_use]
    pub fn with_state_root(state_root: PathBuf) -> Self {
        Self {
            state_root,
            cache: RwLock::new(OpenCache::default()),
        }
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
        build::build_batch(&self.state_root, batch)
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
            return Ok(Box::new(PersistedSemanticSearcher::new(loaded)));
        }
        let loaded = Arc::new(open_generation(
            &self.state_root,
            repo,
            revision,
            generation,
        )?);
        self.cache_put(key, Arc::clone(&loaded))?;
        Ok(Box::new(PersistedSemanticSearcher::new(loaded)))
    }
}

/// A sealed semantic generation discovered on disk, for readiness seeding.
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
