//! The file-backed part of the search-plane authority: opening the store,
//! startup reconciliation, and restoring / migrating auxiliary rows into
//! the ledger.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use quanta_index_contract::{AuxEpochV1, SearchPlaneTrackKind};
use quanta_index_core::{
    AuxiliaryAuthorityCatalogPort, AuxiliaryGenerationKeyV1, AuxiliaryMutationBatchV1, CoreError,
};

use crate::auxiliary_authority;
use crate::readiness::durable_fs::{
    FsParentDirectorySyncPort, ParentDirectorySyncPort, ensure_durable_directory_v1,
    is_owned_search_corpus_staging_name_v1, reconcile_legacy_atomic_temporaries_v1,
};
use crate::readiness::history_state::HistoryAuthoritySnapshot;
use crate::readiness::keys::AuthorityKey;
use crate::readiness::ledger::Ledger;
use crate::readiness::pair_digest::search_corpus_pair_digest;
use crate::readiness::runtime_state::RuntimeAuthoritySnapshot;
#[cfg(test)]
use crate::readiness::search_corpus_generation::SearchCorpusGenerationV1;
use crate::readiness::structural_state::StructuralAuthoritySnapshot;
#[cfg(test)]
use crate::search_corpus_lifecycle::SearchCorpusPairMutationGuard;
use crate::search_corpus_lifecycle::{
    ActiveSearchCorpusPinReadPort, SearchCorpusPairMutationCoordinator,
};
use crate::search_corpus_retention::{
    SearchCorpusHistoryRetentionPolicyV1, SearchCorpusIndexBytesPort,
};
#[cfg(test)]
use quanta_index_contract::RepoId;
#[cfg(test)]
use quanta_index_contract::RevisionId;

/// What the one-shot migration of the pre-catalog snapshot files moved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAuxiliaryMigrationReceipt {
    /// Distinct `(repo, revision, generation)` keys the snapshots held.
    pub generations: usize,
    /// Rows the catalog wrote for them.
    pub rows_written: u64,
}

/// The file-backed part of the search-plane authority.
///
/// It holds the durable search-corpus rollback history under
/// `authorities/search-corpus/` (one record per sealed generation,
/// retention-bounded) and performs the one-shot migration of the
/// pre-catalog auxiliary snapshot files.
///
/// The auxiliary authorities themselves — history, runtime metadata,
/// structural — live in the catalog as rows since QI-BB-020; the three
/// paths kept here name only where their legacy snapshots were.
#[derive(Debug)]
pub struct AuxiliaryAuthorityStore {
    pub(super) history: PathBuf,
    pub(super) runtime: PathBuf,
    pub(super) structural: PathBuf,
    pub(super) search_corpus_dir: PathBuf,
    pub(super) search_corpus_staging_dir: PathBuf,
    // State-root count/byte admission must be atomic across pair stripes.
    // Production also holds the process-level state-root lease, while this
    // lock closes same-process races between different repo/revision pairs.
    pub(super) search_corpus_root_lock: Mutex<()>,
    pub(super) lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
    pub(super) active_pins: Arc<dyn ActiveSearchCorpusPinReadPort>,
    pub(super) search_corpus_history_retention: SearchCorpusHistoryRetentionPolicyV1,
    /// The on-disk index bytes retention measures its byte limits over.
    pub(super) index_bytes: Arc<dyn SearchCorpusIndexBytesPort>,
    pub(super) parent_sync: Arc<dyn ParentDirectorySyncPort>,
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct NoActiveSearchCorpusPinsV1;

/// The test measurement: every generation occupies exactly
/// [`TEST_INDEX_BYTES_PER_GENERATION`] bytes, so byte caps in tests are
/// generation counts times a constant.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct ScriptedIndexBytesV1;

#[cfg(test)]
pub(crate) const TEST_INDEX_BYTES_PER_GENERATION: u64 = 100;

#[cfg(test)]
impl SearchCorpusIndexBytesPort for ScriptedIndexBytesV1 {
    fn measure_index_bytes(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        generations: &std::collections::BTreeSet<quanta_index_contract::ManifestGeneration>,
    ) -> Result<u64, CoreError> {
        Ok(TEST_INDEX_BYTES_PER_GENERATION
            .saturating_mul(u64::try_from(generations.len()).map_or(u64::MAX, |len| len)))
    }
}

impl AuxiliaryAuthorityStore {
    /// A store over the scripted measurement (tests only).
    #[cfg(test)]
    pub fn open(
        root: impl AsRef<Path>,
        search_corpus_history_retention: SearchCorpusHistoryRetentionPolicyV1,
    ) -> Result<Self, CoreError> {
        let coordinator = SearchCorpusPairMutationCoordinator::shared();
        Self::open_with_parent_sync(
            root,
            search_corpus_history_retention,
            coordinator,
            Arc::new(NoActiveSearchCorpusPinsV1),
            Arc::new(ScriptedIndexBytesV1),
            Arc::new(FsParentDirectorySyncPort),
        )
    }

    pub(crate) fn open_with_lifecycle_v1(
        root: impl AsRef<Path>,
        search_corpus_history_retention: SearchCorpusHistoryRetentionPolicyV1,
        lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
        active_pins: Arc<dyn ActiveSearchCorpusPinReadPort>,
        index_bytes: Arc<dyn SearchCorpusIndexBytesPort>,
    ) -> Result<Self, CoreError> {
        Self::open_with_parent_sync(
            root,
            search_corpus_history_retention,
            lifecycle_coordinator,
            active_pins,
            index_bytes,
            Arc::new(FsParentDirectorySyncPort),
        )
    }

    pub(super) fn open_with_parent_sync(
        root: impl AsRef<Path>,
        search_corpus_history_retention: SearchCorpusHistoryRetentionPolicyV1,
        lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
        active_pins: Arc<dyn ActiveSearchCorpusPinReadPort>,
        index_bytes: Arc<dyn SearchCorpusIndexBytesPort>,
        parent_sync: Arc<dyn ParentDirectorySyncPort>,
    ) -> Result<Self, CoreError> {
        let root = root.as_ref();
        ensure_durable_directory_v1(root, "search-plane authority store", parent_sync.as_ref())?;
        let history_dir = root.join("history");
        let runtime_dir = root.join("runtime");
        let structural_dir = root.join("structural");
        let search_corpus_dir = root.join("search-corpus");
        let search_corpus_staging_dir = search_corpus_dir.join(".staging");
        for dir in [
            &history_dir,
            &runtime_dir,
            &structural_dir,
            &search_corpus_dir,
            &search_corpus_staging_dir,
        ] {
            ensure_durable_directory_v1(dir, "search-plane authority store", parent_sync.as_ref())?;
        }
        let store = Self {
            history: history_dir.join("state.cbor"),
            runtime: runtime_dir.join("state.cbor"),
            structural: structural_dir.join("state.cbor"),
            search_corpus_dir,
            search_corpus_staging_dir,
            search_corpus_root_lock: Mutex::new(()),
            lifecycle_coordinator,
            active_pins,
            search_corpus_history_retention,
            index_bytes,
            parent_sync,
        };
        store.reconcile_search_corpus_startup_v1()?;
        Ok(store)
    }

    fn reconcile_search_corpus_startup_v1(&self) -> Result<(), CoreError> {
        let _root_guard = self.search_corpus_root_lock.lock().map_err(|error| {
            CoreError::Storage(format!(
                "search-corpus authority: startup reconciliation lock poisoned: {error}"
            ))
        })?;
        self.reconcile_search_corpus_staging_v1()?;
        let active_pair_names = self.active_search_corpus_pair_names_v1()?;
        let empty_pairs =
            self.collect_abandoned_search_corpus_pair_directories_v1(&active_pair_names)?;
        self.remove_abandoned_search_corpus_pair_directories_v1(&empty_pairs)
    }

    fn reconcile_search_corpus_staging_v1(&self) -> Result<(), CoreError> {
        let mut removed_staging = false;
        for entry in fs::read_dir(&self.search_corpus_staging_dir).map_err(|error| {
            CoreError::Storage(format!(
                "search-corpus authority: list staging {}: {error}",
                self.search_corpus_staging_dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: read staging entry in {}: {error}",
                    self.search_corpus_staging_dir.display()
                ))
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: inspect staging entry {}: {error}",
                    path.display()
                ))
            })?;
            let owned = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_owned_search_corpus_staging_name_v1);
            if !file_type.is_file() || !owned {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: foreign staging entry {}",
                    path.display()
                )));
            }
            fs::remove_file(&path).map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: remove abandoned staging file {}: {error}",
                    path.display()
                ))
            })?;
            removed_staging = true;
        }
        if removed_staging {
            self.parent_sync
                .sync_parent(&self.search_corpus_staging_dir)
                .map_err(|error| {
                    CoreError::Storage(format!(
                        "search-corpus authority: fsync staging after reconciliation {}: {error}",
                        self.search_corpus_staging_dir.display()
                    ))
                })?;
        }
        Ok(())
    }

    fn active_search_corpus_pair_names_v1(&self) -> Result<BTreeSet<String>, CoreError> {
        let active_pair_names = self
            .active_pins
            .all_active_search_corpora_for_bootstrap_v1()?
            .into_iter()
            .map(|active| search_corpus_pair_digest(active.repo_id(), active.revision_id()))
            .collect();
        Ok(active_pair_names)
    }

    fn collect_abandoned_search_corpus_pair_directories_v1(
        &self,
        active_pair_names: &BTreeSet<String>,
    ) -> Result<Vec<PathBuf>, CoreError> {
        let mut empty_pairs = Vec::new();
        for entry in fs::read_dir(&self.search_corpus_dir).map_err(|error| {
            CoreError::Storage(format!(
                "search-corpus authority: list root for reconciliation {}: {error}",
                self.search_corpus_dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: read root entry during reconciliation: {error}"
                ))
            })?;
            let path = entry.path();
            if path == self.search_corpus_staging_dir {
                continue;
            }
            let file_type = entry.file_type().map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: inspect root entry {}: {error}",
                    path.display()
                ))
            })?;
            let pair_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    CoreError::Storage(format!(
                        "search-corpus authority: non-UTF8 root entry {}",
                        path.display()
                    ))
                })?;
            if !file_type.is_dir()
                || pair_name.len() != 64
                || !pair_name.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: foreign root entry {}",
                    path.display()
                )));
            }
            reconcile_legacy_atomic_temporaries_v1(
                &path,
                ".cbor",
                "search-corpus authority",
                self.parent_sync.as_ref(),
            )?;
            if fs::read_dir(&path)
                .map_err(|error| {
                    CoreError::Storage(format!(
                        "search-corpus authority: list pair {} during reconciliation: {error}",
                        path.display()
                    ))
                })?
                .next()
                .is_none()
            {
                if active_pair_names.contains(pair_name) {
                    return Err(CoreError::Storage(format!(
                        "search-corpus authority: active pair directory is empty: {}",
                        path.display()
                    )));
                }
                empty_pairs.push(path);
            }
        }
        Ok(empty_pairs)
    }

    fn remove_abandoned_search_corpus_pair_directories_v1(
        &self,
        empty_pairs: &[PathBuf],
    ) -> Result<(), CoreError> {
        for pair in empty_pairs {
            fs::remove_dir(pair).map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: remove abandoned empty pair {}: {error}",
                    pair.display()
                ))
            })?;
        }
        if !empty_pairs.is_empty() {
            self.parent_sync
                .sync_parent(&self.search_corpus_dir)
                .map_err(|error| {
                    CoreError::Storage(format!(
                        "search-corpus authority: fsync root after empty-pair reconciliation {}: {error}",
                        self.search_corpus_dir.display()
                    ))
                })?;
        }
        Ok(())
    }

    /// Move the pre-catalog whole-map snapshot files, if any exist, into
    /// the auxiliary row catalog as one transaction, then remove them
    /// (QI-BB-020, one-shot).
    ///
    /// The migrated generations are stamped [`AuxEpochV1::GENESIS`]: they
    /// predate epoch stamping, and their first stamped mutation starts the
    /// sequence.
    ///
    /// A crash after the transaction and before the removal re-runs the
    /// migration on the next open; every row is an upsert of the same
    /// content, so the second run converges on the same catalog.
    pub fn migrate_legacy_auxiliary_snapshots(
        &self,
        catalog: &dyn AuxiliaryAuthorityCatalogPort,
    ) -> Result<Option<LegacyAuxiliaryMigrationReceipt>, CoreError> {
        let history = self.read_cbor::<HistoryAuthoritySnapshot>(&self.history, "history")?;
        let runtime =
            self.read_cbor::<RuntimeAuthoritySnapshot>(&self.runtime, "runtime metadata")?;
        let structural =
            self.read_cbor::<StructuralAuthoritySnapshot>(&self.structural, "structural")?;
        if history.is_none() && runtime.is_none() && structural.is_none() {
            return Ok(None);
        }
        let mut batch = AuxiliaryMutationBatchV1::default();
        let mut generations: BTreeSet<AuthorityKey> = BTreeSet::new();
        let generation_key = |key: &AuthorityKey| AuxiliaryGenerationKeyV1 {
            repo_id: key.repo_id.clone(),
            revision_id: key.revision_id.clone(),
            generation: key.generation,
        };
        if let Some(history) = history {
            for (key, state) in &history.entries {
                let _new = generations.insert(key.clone());
                batch.rows.extend(auxiliary_authority::history_state_rows(
                    &generation_key(key),
                    AuxEpochV1::GENESIS,
                    state,
                )?);
            }
        }
        if let Some(runtime) = runtime {
            for (key, state) in &runtime.entries {
                let _new = generations.insert(key.clone());
                batch.rows.extend(auxiliary_authority::runtime_state_rows(
                    &generation_key(key),
                    AuxEpochV1::GENESIS,
                    state,
                )?);
            }
        }
        if let Some(structural) = structural {
            for (key, state) in &structural.entries {
                let _new = generations.insert(key.clone());
                batch
                    .rows
                    .extend(auxiliary_authority::structural_state_rows(
                        &generation_key(key),
                        AuxEpochV1::GENESIS,
                        state,
                    )?);
            }
            for (key, state) in &structural.tracks {
                if key.track != SearchPlaneTrackKind::Structural {
                    continue;
                }
                batch.tracks.push(auxiliary_authority::structural_track_row(
                    &key.repo_id,
                    &key.revision_id,
                    state,
                )?);
            }
        }
        let receipt = catalog.apply(&batch)?;
        for path in [&self.history, &self.runtime, &self.structural] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(CoreError::Storage(format!(
                        "search-plane authority store: remove migrated snapshot {}: {err}",
                        path.display()
                    )));
                }
            }
        }
        Ok(Some(LegacyAuxiliaryMigrationReceipt {
            generations: generations.len(),
            rows_written: receipt.rows_written,
        }))
    }

    /// Restore the search-corpus rollback history into the ledger; the
    /// auxiliary authorities are restored from the catalog by
    /// [`restore_auxiliary_rows_into`].
    pub fn restore_into(&self, ledger: &mut Ledger) -> Result<(), CoreError> {
        self.restore_search_corpus_history_into(ledger)?;
        Ok(())
    }
}

/// Rebuild the ledger's auxiliary authorities from the catalog's rows.
pub fn restore_auxiliary_rows_into(
    ledger: &mut Ledger,
    catalog: &dyn AuxiliaryAuthorityCatalogPort,
) -> Result<u64, CoreError> {
    let mut restored = 0_u64;
    catalog.for_each_row(&mut |row| {
        auxiliary_authority::restore_row_into(ledger, &row)?;
        restored = restored.saturating_add(1);
        Ok(())
    })?;
    for track in catalog.track_rows()? {
        auxiliary_authority::restore_track_row_into(ledger, &track)?;
    }
    Ok(restored)
}

#[cfg(test)]
impl ActiveSearchCorpusPinReadPort for NoActiveSearchCorpusPinsV1 {
    fn active_search_corpus_under_guard_v1(
        &self,
        _guard: &SearchCorpusPairMutationGuard<'_>,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
    ) -> Result<Option<SearchCorpusGenerationV1>, CoreError> {
        Ok(None)
    }

    fn all_active_search_corpora_for_bootstrap_v1(
        &self,
    ) -> Result<Vec<SearchCorpusGenerationV1>, CoreError> {
        Ok(Vec::new())
    }
}
