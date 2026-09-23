//! `ActivationCatalog`: the query-time serve-head authority persisted under
//! `state_root/activations/`, with CAS activation and rollback.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    GenerationPin, GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneSearchCorpusRollbackCasAck,
    SearchPlaneTrackKind,
};
use quanta_index_core::CoreError;

use crate::readiness::durable_fs::{
    AtomicFileWriteOutcomeV1, FsParentDirectorySyncPort, ParentDirectorySyncPort,
    atomic_replace_file_from_staging_v1, ensure_durable_directory_v1,
    read_regular_file_nofollow_v1, reconcile_legacy_atomic_temporaries_v1,
    reconcile_owned_staging_directory_v1,
};
use crate::readiness::pair_digest::search_corpus_pair_digest;
use crate::readiness::search_corpus_generation::{
    PersistedSearchCorpusGenerationRootV1, PreparedSearchCorpusGenerationV1,
    SearchCorpusGenerationActivationV1, SearchCorpusGenerationV1,
    search_corpus_generation_from_validated_rollback_identity,
    search_corpus_generation_into_contract, validate_prepared_search_corpus_expectation,
};
use crate::search_corpus_lifecycle::{
    ActiveSearchCorpusPinReadPort, SearchCorpusPairMutationCoordinator,
    SearchCorpusPairMutationGuard,
};

/// One `(repo, revision)` pair: the unit an activation root is persisted
/// and served for.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct ActivationKey {
    repo_id: RepoId,
    revision_id: RevisionId,
}

impl ActivationKey {
    fn for_pair(repo_id: &RepoId, revision_id: &RevisionId) -> Self {
        Self {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveGenerationRecord {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
    pub track: SearchPlaneTrackKind,
}

/// The active composite roots, one per pair. Each is the complete
/// lexical + semantic identity with the semantic content roots
/// (QI-BB-028); per-track views are derived on read.
type ActiveRoots = BTreeMap<ActivationKey, SearchCorpusGenerationV1>;

#[derive(Debug)]
pub struct ActivationCatalog {
    activations_dir: PathBuf,
    staging_dir: PathBuf,
    entries: RwLock<ActiveRoots>,
    lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
    // A rename may succeed while the parent-directory fsync fails.  At that
    // point the durable head is ambiguous until a fresh process reopens the
    // canonical root, so this process must not serve its old in-memory head.
    durability_uncertain_v1: AtomicBool,
    parent_sync: Arc<dyn ParentDirectorySyncPort>,
}

impl ActivationCatalog {
    /// A single catalog snapshot for process readiness. Comparing the
    /// complete pair identities before and after a physical probe rejects
    /// a concurrent replacement even when the pair count is unchanged.
    pub fn active_inventory_v1(&self) -> Result<(Vec<SearchCorpusGenerationV1>, u64), CoreError> {
        self.ensure_durability_certain_v1()?;
        let entries = self.entries.read().map_err(|error| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {error}"))
        })?;
        self.ensure_durability_certain_v1()?;
        let repositories = entries
            .keys()
            .map(|key| &key.repo_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let repositories = u64::try_from(repositories).map_err(|error| {
            CoreError::Storage(format!("active repository count overflow: {error}"))
        })?;
        Ok((entries.values().cloned().collect(), repositories))
    }

    #[cfg(test)]
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        Self::open_with_parent_sync(
            root,
            SearchCorpusPairMutationCoordinator::shared(),
            Arc::new(FsParentDirectorySyncPort),
        )
    }

    pub(crate) fn open_with_lifecycle_v1(
        root: impl AsRef<Path>,
        lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
    ) -> Result<Self, CoreError> {
        Self::open_with_parent_sync(
            root,
            lifecycle_coordinator,
            Arc::new(FsParentDirectorySyncPort),
        )
    }

    pub(super) fn open_with_parent_sync(
        root: impl AsRef<Path>,
        lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
        parent_sync: Arc<dyn ParentDirectorySyncPort>,
    ) -> Result<Self, CoreError> {
        let root = root.as_ref();
        ensure_durable_directory_v1(
            root,
            "search-plane activation catalog",
            parent_sync.as_ref(),
        )?;
        let staging_dir = root.join(".staging");
        ensure_durable_directory_v1(
            &staging_dir,
            "search-plane activation catalog staging",
            parent_sync.as_ref(),
        )?;
        reconcile_owned_staging_directory_v1(
            &staging_dir,
            "search-plane activation catalog",
            parent_sync.as_ref(),
        )?;
        reconcile_legacy_atomic_temporaries_v1(
            root,
            "--corpus.json",
            "search-plane activation catalog",
            parent_sync.as_ref(),
        )?;
        let mut entries = BTreeMap::new();
        let mut composite_roots = Vec::new();
        let dir_entries = fs::read_dir(root).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane activation catalog: list root {}: {err}",
                root.display()
            ))
        })?;
        for entry in dir_entries {
            let entry = entry.map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: read dir entry in {}: {err}",
                    root.display()
                ))
            })?;
            let file_type = entry.file_type().map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: inspect dir entry in {}: {err}",
                    root.display()
                ))
            })?;
            let path = entry.path();
            if path == staging_dir {
                continue;
            }
            if !file_type.is_file() {
                if is_search_corpus_root_path(&path) {
                    return Err(CoreError::Storage(format!(
                        "search-plane activation catalog: activation root is not a regular non-symlink file: {}",
                        path.display()
                    )));
                }
                continue;
            }
            if path.extension().is_none_or(|value| value != "json") {
                continue;
            }
            if !is_search_corpus_root_path(&path) {
                return Err(CoreError::Storage(format!(
                    "search-plane activation catalog: legacy per-track root is unsupported: {}",
                    path.display()
                )));
            }
            let bytes = read_regular_file_nofollow_v1(&path).map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: read activation {}: {err}",
                    path.display()
                ))
            })?;
            let persisted = serde_json::from_slice::<PersistedSearchCorpusGenerationRootV1>(&bytes)
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "search-plane activation catalog: decode activation {}: {err}",
                        path.display()
                    ))
                })?;
            let composite = persisted.into_generation()?;
            validate_loaded_search_corpus_root_v1(&path, &composite)?;
            composite_roots.push(composite);
        }
        // Every persisted root is one canonical lexical+semantic authority.
        for composite in composite_roots {
            insert_search_corpus_generation_records(&mut entries, &composite);
        }
        Ok(Self {
            activations_dir: root.to_path_buf(),
            staging_dir,
            entries: RwLock::new(entries),
            lifecycle_coordinator,
            durability_uncertain_v1: AtomicBool::new(false),
            parent_sync,
        })
    }

    pub(crate) fn lifecycle_coordinator(&self) -> Arc<SearchCorpusPairMutationCoordinator> {
        Arc::clone(&self.lifecycle_coordinator)
    }

    /// Durably promote one fully prepared lexical plus semantic corpus root.
    ///
    /// The persistent root is replaced and its parent directory is synced
    /// before either in-memory track entry changes. A failed durable write
    /// therefore leaves the query-visible head unchanged.
    pub(crate) fn activate_prepared_under_guard_v1(
        &self,
        guard: &SearchCorpusPairMutationGuard<'_>,
        prepared: &PreparedSearchCorpusGenerationV1,
    ) -> Result<SearchCorpusGenerationActivationV1, CoreError> {
        let candidate = prepared.candidate();
        guard.require_pair_v1(
            self.lifecycle_coordinator.as_ref(),
            candidate.repo_id(),
            candidate.revision_id(),
        )?;
        self.ensure_durability_certain_v1()?;
        let current = {
            let entries = self.entries.read().map_err(|err| {
                CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
            })?;
            active_search_corpus_generation_v1(
                &entries,
                candidate.repo_id(),
                candidate.revision_id(),
            )
        };
        validate_prepared_search_corpus_expectation(prepared, current.as_ref())?;

        if let Some(current) = &current
            && candidate.manifest_generation().get() <= current.manifest_generation().get()
        {
            return Err(CoreError::InvalidContract(format!(
                "search-corpus activation: candidate generation must advance active generation for repo={} revision={}: candidate={} active={}",
                candidate.repo_id().as_str(),
                candidate.revision_id().as_str(),
                candidate.manifest_generation().get(),
                current.manifest_generation().get(),
            )));
        }

        let persisted = PersistedSearchCorpusGenerationRootV1::from_generation(candidate);
        let path = self.activations_dir.join(search_corpus_root_file_name(
            candidate.repo_id(),
            candidate.revision_id(),
        ));
        let bytes = serde_json::to_vec_pretty(&persisted).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane activation catalog: encode composite activation {}: {err}",
                path.display()
            ))
        })?;
        match atomic_replace_file_from_staging_v1(
            &path,
            &bytes,
            &self.staging_dir,
            "search-plane activation catalog",
            self.parent_sync.as_ref(),
        )? {
            AtomicFileWriteOutcomeV1::Durable => {}
            AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(error) => {
                self.mark_durability_uncertain_v1();
                return Err(error);
            }
        }
        let memory_commit = (|| {
            self.ensure_durability_certain_v1()?;
            let mut entries = self.entries.write().map_err(|err| {
                CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
            })?;
            let observed = active_search_corpus_generation_v1(
                &entries,
                candidate.repo_id(),
                candidate.revision_id(),
            );
            if observed != current {
                return Err(CoreError::Storage(
                    "search-corpus activation: mutation lock failed to preserve the checked head"
                        .to_string(),
                ));
            }
            insert_search_corpus_generation_records(&mut entries, candidate);
            drop(entries);
            Ok(())
        })();
        if let Err(error) = memory_commit {
            self.mark_durability_uncertain_v1();
            return Err(error);
        }

        Ok(SearchCorpusGenerationActivationV1 {
            active: candidate.clone(),
            previous_active: current,
        })
    }

    #[cfg(test)]
    pub fn activate_prepared_search_corpus_generation_v1(
        &self,
        prepared: &PreparedSearchCorpusGenerationV1,
    ) -> Result<SearchCorpusGenerationActivationV1, CoreError> {
        let candidate = prepared.candidate();
        let guard = self
            .lifecycle_coordinator
            .lock_pair(candidate.repo_id(), candidate.revision_id())?;
        self.activate_prepared_under_guard_v1(&guard, prepared)
    }

    /// Roll back the complete query-visible lexical plus semantic corpus.
    ///
    /// The request carries both complete identities, so no single-track
    /// selector can create or persist semantic-only authority.
    pub(crate) fn rollback_under_guard_v1(
        &self,
        guard: &SearchCorpusPairMutationGuard<'_>,
        request: &SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    ) -> Result<SearchPlaneSearchCorpusRollbackCasAck, CoreError> {
        request.validate_v1().map_err(|error| {
            CoreError::InvalidContract(format!(
                "search-corpus rollback: invalid request: {}",
                error.code_v1()
            ))
        })?;
        let expected_active =
            search_corpus_generation_from_validated_rollback_identity(&request.expected_active)?;
        let target = search_corpus_generation_from_validated_rollback_identity(&request.target)?;

        guard.require_pair_v1(
            self.lifecycle_coordinator.as_ref(),
            expected_active.repo_id(),
            expected_active.revision_id(),
        )?;
        self.ensure_durability_certain_v1()?;
        let current = {
            let entries = self.entries.read().map_err(|err| {
                CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
            })?;
            active_search_corpus_generation_v1(
                &entries,
                expected_active.repo_id(),
                expected_active.revision_id(),
            )
            .ok_or_else(|| {
                CoreError::NotReady(format!(
                    "search-corpus rollback: no active composite generation for repo={} revision={}",
                    expected_active.repo_id().as_str(),
                    expected_active.revision_id().as_str()
                ))
            })?
        };
        if current != expected_active {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::RollbackCasConflict,
                message: format!(
                    "search-corpus rollback: active composite root changed for repo={} revision={}: expected generation={} digest={}, observed generation={} digest={}",
                    expected_active.repo_id().as_str(),
                    expected_active.revision_id().as_str(),
                    expected_active.manifest_generation().get(),
                    expected_active.manifest_digest(),
                    current.manifest_generation().get(),
                    current.manifest_digest(),
                ),
            });
        }

        let persisted = PersistedSearchCorpusGenerationRootV1::from_generation(&target);
        let path = self.activations_dir.join(search_corpus_root_file_name(
            target.repo_id(),
            target.revision_id(),
        ));
        let bytes = serde_json::to_vec_pretty(&persisted).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane activation catalog: encode composite rollback {}: {err}",
                path.display()
            ))
        })?;
        match atomic_replace_file_from_staging_v1(
            &path,
            &bytes,
            &self.staging_dir,
            "search-plane activation catalog",
            self.parent_sync.as_ref(),
        )? {
            AtomicFileWriteOutcomeV1::Durable => {}
            AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(error) => {
                self.mark_durability_uncertain_v1();
                return Err(error);
            }
        }
        let memory_commit = (|| {
            self.ensure_durability_certain_v1()?;
            let mut entries = self.entries.write().map_err(|err| {
                CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
            })?;
            let observed = active_search_corpus_generation_v1(
                &entries,
                expected_active.repo_id(),
                expected_active.revision_id(),
            );
            if observed.as_ref() != Some(&current) {
                return Err(CoreError::Storage(
                    "search-corpus rollback: mutation lock failed to preserve the checked head"
                        .to_string(),
                ));
            }
            insert_search_corpus_generation_records(&mut entries, &target);
            drop(entries);
            Ok(())
        })();
        if let Err(error) = memory_commit {
            self.mark_durability_uncertain_v1();
            return Err(error);
        }

        Ok(SearchPlaneSearchCorpusRollbackCasAck {
            active: search_corpus_generation_into_contract(&target),
            previous_sealed_active: search_corpus_generation_into_contract(&current),
        })
    }

    #[cfg(test)]
    pub(crate) fn rollback(
        &self,
        request: &SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    ) -> Result<SearchPlaneSearchCorpusRollbackCasAck, CoreError> {
        let expected_active =
            search_corpus_generation_from_validated_rollback_identity(&request.expected_active)?;
        let guard = self
            .lifecycle_coordinator
            .lock_pair(expected_active.repo_id(), expected_active.revision_id())?;
        self.rollback_under_guard_v1(&guard, request)
    }

    pub fn resolve(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
    ) -> Result<GenerationPin, CoreError> {
        let record = self.resolve_record(repo_id, revision_id, track)?;
        Ok(GenerationPin::new(
            record.repo_id,
            record.revision_id,
            record.manifest_generation,
        ))
    }

    /// QI-ACT-01: return the full [`ActiveGenerationRecord`] for one
    /// `(repo, revision, track)` triple — same lookup as [`Self::resolve`]
    /// but preserves `manifest_digest` (load-bearing for the admin
    /// surface). Fail-closed on missing entry per CLAUDE.md safety rule.
    pub fn resolve_record(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
    ) -> Result<ActiveGenerationRecord, CoreError> {
        let entries = self.entries.read().map_err(|err| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
        })?;
        self.ensure_durability_certain_v1()?;
        let not_ready = || {
            CoreError::NotReady(format!(
                "activate-generation: no active {track:?} generation for repo={} revision={}",
                repo_id.as_str(),
                revision_id.as_str()
            ))
        };
        let record = {
            let generation = entries
                .get(&ActivationKey::for_pair(repo_id, revision_id))
                .ok_or_else(not_ready)?;
            match track {
                SearchPlaneTrackKind::Lexical => active_record_of(generation.lexical()),
                SearchPlaneTrackKind::Semantic => active_record_of(generation.semantic()),
                SearchPlaneTrackKind::Structural => return Err(not_ready()),
            }
        };
        drop(entries);
        Ok(record)
    }

    /// QI-ACT-01: return every active generation record for one
    /// `(repo, revision)` pair, ordered by track declaration order (lexical
    /// before semantic). Empty Vec when nothing is activated yet — empty
    /// is a legitimate state, distinct from `NotReady` for a specific track.
    pub fn entries_for(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<ActiveGenerationRecord>, CoreError> {
        let entries = self.entries.read().map_err(|err| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
        })?;
        self.ensure_durability_certain_v1()?;
        // Lexical before semantic: track declaration order.
        Ok(entries
            .get(&ActivationKey::for_pair(repo_id, revision_id))
            .map(|generation| {
                vec![
                    active_record_of(generation.lexical()),
                    active_record_of(generation.semantic()),
                ]
            })
            .unwrap_or_default())
    }

    /// The active composite root for one pair, with the semantic content
    /// roots it was activated under (QI-BB-028); `None` when the pair has
    /// no activation.
    pub fn active_search_corpus_v1(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Option<SearchCorpusGenerationV1>, CoreError> {
        let entries = self.entries.read().map_err(|err| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
        })?;
        self.ensure_durability_certain_v1()?;
        Ok(active_search_corpus_generation_v1(
            &entries,
            repo_id,
            revision_id,
        ))
    }

    fn ensure_durability_certain_v1(&self) -> Result<(), CoreError> {
        if self.durability_uncertain_v1.load(Ordering::Acquire) {
            return Err(CoreError::NotReady(
                "search-plane activation catalog: activation durability is uncertain; reopen the catalog before serving or mutating generations".to_string(),
            ));
        }
        Ok(())
    }

    pub(super) fn mark_durability_uncertain_v1(&self) {
        self.durability_uncertain_v1.store(true, Ordering::Release);
    }
}

impl ActiveSearchCorpusPinReadPort for ActivationCatalog {
    fn active_search_corpus_under_guard_v1(
        &self,
        guard: &SearchCorpusPairMutationGuard<'_>,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Option<SearchCorpusGenerationV1>, CoreError> {
        guard.require_pair_v1(self.lifecycle_coordinator.as_ref(), repo_id, revision_id)?;
        self.ensure_durability_certain_v1()?;
        let entries = self.entries.read().map_err(|error| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {error}"))
        })?;
        Ok(active_search_corpus_generation_v1(
            &entries,
            repo_id,
            revision_id,
        ))
    }

    fn all_active_search_corpora_for_bootstrap_v1(
        &self,
    ) -> Result<Vec<SearchCorpusGenerationV1>, CoreError> {
        self.ensure_durability_certain_v1()?;
        let entries = self.entries.read().map_err(|error| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {error}"))
        })?;
        let active = entries.values().cloned().collect();
        drop(entries);
        Ok(active)
    }
}

/// The per-track view of one snapshot of an active composite root.
fn active_record_of(snapshot: &GenerationSnapshot) -> ActiveGenerationRecord {
    ActiveGenerationRecord {
        repo_id: snapshot.repo_id.clone(),
        revision_id: snapshot.revision_id.clone(),
        manifest_generation: snapshot.manifest_generation,
        manifest_digest: snapshot.manifest_digest.clone(),
        track: snapshot.track,
    }
}

fn active_search_corpus_generation_v1(
    entries: &ActiveRoots,
    repo_id: &RepoId,
    revision_id: &RevisionId,
) -> Option<SearchCorpusGenerationV1> {
    entries
        .get(&ActivationKey::for_pair(repo_id, revision_id))
        .cloned()
}

fn insert_search_corpus_generation_records(
    entries: &mut ActiveRoots,
    generation: &SearchCorpusGenerationV1,
) {
    let _prior = entries.insert(
        ActivationKey::for_pair(generation.repo_id(), generation.revision_id()),
        generation.clone(),
    );
}

pub(super) fn search_corpus_root_file_name(repo_id: &RepoId, revision_id: &RevisionId) -> String {
    format!(
        "{}--corpus.json",
        search_corpus_pair_digest(repo_id, revision_id)
    )
}

fn is_search_corpus_root_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("--corpus.json"))
}

fn validate_loaded_search_corpus_root_v1(
    path: &Path,
    generation: &SearchCorpusGenerationV1,
) -> Result<(), CoreError> {
    let expected_name =
        search_corpus_root_file_name(generation.repo_id(), generation.revision_id());
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err(CoreError::Storage(format!(
            "search-plane activation catalog: filename/payload identity mismatch: expected {expected_name}, observed {}",
            path.display()
        )));
    }
    Ok(())
}
