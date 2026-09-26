//! `ActivationCatalog`: the query-time serve-head authority persisted under
//! `state_root/activations/`, with CAS activation and rollback.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use quanta_index_contract::{
    ACTIVATION_ROOT_INCARNATION_BYTES_V1, GenerationPin, GenerationSnapshot, ManifestGeneration,
    RepoId, RevisionId, SearchCorpusActivationTokenV1, SearchCorpusActiveHeadV1,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneSearchCorpusRollbackCasAck,
    SearchPlaneTrackKind, SourcePublicationEvent,
};
use quanta_index_core::CoreError;

mod repository_envelope;
mod source_events;
#[cfg(test)]
mod tests;

use repository_envelope::EventHistory;

use crate::readiness::durable_fs::{
    AtomicFileWriteOutcomeV1, FsParentDirectorySyncPort, ParentDirectorySyncPort,
    atomic_replace_file_from_staging_v1, ensure_durable_directory_v1,
    read_regular_file_nofollow_v1, reconcile_legacy_atomic_temporaries_v1,
    reconcile_owned_staging_directory_v1,
};
use crate::readiness::search_corpus_generation::{
    PreparedSearchCorpusGenerationV1, SearchCorpusGenerationActivationV1, SearchCorpusGenerationV1,
    validate_prepared_search_corpus_expectation,
};
use crate::search_corpus_lifecycle::{
    ActiveSearchCorpusPinReadPort, SearchCorpusPairMutationCoordinator,
    SearchCorpusPairMutationGuard,
};

/// One query-visible `(repo, revision)` root inside its containing repository
/// envelope. Multiple revision roots share a durable source-event transaction.
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
type ActiveRoots = BTreeMap<ActivationKey, ActiveSearchCorpusHeadV1>;

#[derive(Debug, Default)]
struct CatalogState {
    roots: ActiveRoots,
    histories: BTreeMap<RepoId, EventHistory>,
}

impl CatalogState {
    // Clone only the bounded repository about to be changed. The catalog lock
    // remains held across its durable replace and in-memory publication.
    fn repository(&self, repo: &RepoId) -> Self {
        Self {
            roots: self
                .roots
                .iter()
                .filter(|(key, _)| &key.repo_id == repo)
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            histories: self
                .histories
                .get(repo)
                .map(|history| BTreeMap::from([(repo.clone(), history.clone())]))
                .unwrap_or_default(),
        }
    }
}

const ROOT_INCARNATION_FILE_V1: &str = ".activation-root-incarnation-v1";
const ROOT_INCARNATION_BYTES_V1: usize = ACTIVATION_ROOT_INCARNATION_BYTES_V1;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ActiveSearchCorpusHeadV1 {
    generation: SearchCorpusGenerationV1,
    activation_sequence: NonZeroU64,
}

impl ActiveSearchCorpusHeadV1 {
    fn activation_token_v1(
        &self,
        root_incarnation: [u8; ROOT_INCARNATION_BYTES_V1],
    ) -> Result<SearchCorpusActivationTokenV1, CoreError> {
        SearchCorpusActivationTokenV1::new(root_incarnation, self.activation_sequence).map_err(
            |error| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: invalid active token: {error}"
                ))
            },
        )
    }

    fn to_contract_v1(
        &self,
        root_incarnation: [u8; ROOT_INCARNATION_BYTES_V1],
    ) -> Result<SearchCorpusActiveHeadV1, CoreError> {
        Ok(SearchCorpusActiveHeadV1 {
            generation: self.generation.to_contract_v1(),
            activation_token: self.activation_token_v1(root_incarnation)?,
        })
    }

    fn next_sequence(&self) -> Result<NonZeroU64, CoreError> {
        let next = self
            .activation_sequence
            .get()
            .checked_add(1)
            .ok_or_else(|| {
                CoreError::Storage("search-corpus activation sequence exhausted".to_string())
            })?;
        NonZeroU64::new(next).ok_or_else(|| {
            CoreError::Storage("search-corpus activation sequence became zero".to_string())
        })
    }
}

#[derive(Debug)]
pub struct ActivationCatalog {
    activations_dir: PathBuf,
    staging_dir: PathBuf,
    root_incarnation: [u8; ROOT_INCARNATION_BYTES_V1],
    entries: RwLock<CatalogState>,
    // Serialize repository writers without holding the query snapshot during
    // durable I/O. Readers retain the last acknowledged pair until promotion.
    mutation: Mutex<()>,
    lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
    // A rename may succeed while the parent-directory fsync fails.  At that
    // point the durable head is ambiguous until a fresh process reopens the
    // canonical root, so this process must not serve its old in-memory head.
    durability_uncertain_v1: AtomicBool,
    parent_sync: Arc<dyn ParentDirectorySyncPort>,
}

impl ActivationCatalog {
    /// Controlled offline restore rotates the catalog incarnation in the
    /// staging root before its manifest is published. Uncontrolled directory
    /// copies are outside local fencing and require an operator boundary.
    pub fn rotate_root_incarnation_for_restore_v1(
        activations_root: &Path,
    ) -> Result<(), CoreError> {
        let parent_sync = FsParentDirectorySyncPort;
        ensure_durable_directory_v1(
            activations_root,
            "search-plane activation catalog restore",
            &parent_sync,
        )?;
        let path = activations_root.join(ROOT_INCARNATION_FILE_V1);
        if matches!(fs::symlink_metadata(&path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        {
            let has_roots = fs::read_dir(activations_root)
                .map_err(|error| {
                    CoreError::Storage(format!(
                        "search-plane activation catalog: list restore roots {}: {error}",
                        activations_root.display()
                    ))
                })?
                .try_fold(false, |found, entry| {
                    let entry = entry.map_err(|error| {
                        CoreError::Storage(format!(
                            "search-plane activation catalog: inspect restore root: {error}"
                        ))
                    })?;
                    Ok::<_, CoreError>(found || is_search_corpus_root_path(&entry.path()))
                })?;
            let _created =
                load_or_create_root_incarnation_v1(activations_root, !has_roots, &parent_sync)?;
            return Ok(());
        }
        let previous = read_root_incarnation_v1(&path)?;
        let next = fresh_root_incarnation_v1()?;
        if next == previous {
            return Err(CoreError::Storage(
                "search-plane activation catalog: restore incarnation did not change".to_string(),
            ));
        }
        let staging_dir = activations_root.join(".staging");
        ensure_durable_directory_v1(
            &staging_dir,
            "search-plane activation catalog restore staging",
            &parent_sync,
        )?;
        match atomic_replace_file_from_staging_v1(
            &path,
            &next,
            &staging_dir,
            "search-plane activation catalog restore incarnation",
            &parent_sync,
        )? {
            AtomicFileWriteOutcomeV1::Durable => Ok(()),
            AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(error) => Err(error),
        }
    }

    /// A single catalog snapshot for process readiness. Comparing the
    /// complete pair identities and activation tokens before and after a
    /// physical probe rejects replacement or A -> B -> A reactivation even
    /// when the pair count and generation identities are unchanged.
    pub fn active_inventory_v1(
        &self,
    ) -> Result<
        (
            Vec<(SearchCorpusGenerationV1, SearchCorpusActivationTokenV1)>,
            u64,
        ),
        CoreError,
    > {
        self.ensure_durability_certain_v1()?;
        let entries = self.entries.read().map_err(|error| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {error}"))
        })?;
        self.ensure_durability_certain_v1()?;
        let repositories = entries
            .roots
            .keys()
            .map(|key| &key.repo_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let repositories = u64::try_from(repositories).map_err(|error| {
            CoreError::Storage(format!("active repository count overflow: {error}"))
        })?;
        let inventory = entries
            .roots
            .values()
            .map(|head| {
                Ok((
                    head.generation.clone(),
                    head.activation_token_v1(self.root_incarnation)?,
                ))
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        drop(entries);
        Ok((inventory, repositories))
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
        let mut entries = CatalogState::default();
        let mut loaded_repositories = std::collections::BTreeSet::new();
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
            let bytes = repository_envelope::read_bounded(&path)?;
            let (repo, loaded) = repository_envelope::decode(&path, &bytes)?;
            if !loaded_repositories.insert(repo) {
                return Err(repository_envelope::corrupt(
                    "duplicate repository envelope",
                ));
            }
            entries.roots.extend(loaded.roots);
            entries.histories.extend(loaded.histories);
        }
        let root_incarnation = load_or_create_root_incarnation_v1(
            root,
            loaded_repositories.is_empty(),
            parent_sync.as_ref(),
        )?;
        Ok(Self {
            activations_dir: root.to_path_buf(),
            staging_dir,
            root_incarnation,
            entries: RwLock::new(entries),
            mutation: Mutex::new(()),
            lifecycle_coordinator,
            durability_uncertain_v1: AtomicBool::new(false),
            parent_sync,
        })
    }

    pub(crate) fn lifecycle_coordinator(&self) -> Arc<SearchCorpusPairMutationCoordinator> {
        Arc::clone(&self.lifecycle_coordinator)
    }

    /// This catalog-owned incarnation changes on a controlled restore. It is
    /// not an independent cache or a promise to fence uncontrolled copies.
    #[must_use]
    pub const fn root_incarnation_v1(&self) -> [u8; ROOT_INCARNATION_BYTES_V1] {
        self.root_incarnation
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
        sealed_event: Option<&SourcePublicationEvent>,
    ) -> Result<SearchCorpusGenerationActivationV1, CoreError> {
        let candidate = prepared.candidate();
        repository_envelope::bound_manifest_token(&candidate.lexical().manifest_digest)?;
        repository_envelope::bound_manifest_token(&candidate.semantic().manifest_digest)?;
        guard.require_pair_v1(
            self.lifecycle_coordinator.as_ref(),
            candidate.repo_id(),
            candidate.revision_id(),
        )?;
        let mutation = self.lock_mutation()?;
        let entries = self
            .entries
            .read()
            .map_err(|error| repository_envelope::corrupt(error.to_string()))?;
        self.ensure_durability_certain_v1()?;
        let current_head = active_search_corpus_head_v1(
            &entries.roots,
            candidate.repo_id(),
            candidate.revision_id(),
        );
        let current = current_head
            .as_ref()
            .map(|head| head.to_contract_v1(self.root_incarnation))
            .transpose()?;
        validate_prepared_search_corpus_expectation(prepared, current.as_ref())?;

        if let Some(current) = &current
            && candidate.manifest_generation().get()
                <= current.generation.lexical.manifest_generation.get()
        {
            return Err(CoreError::InvalidContract(format!(
                "search-corpus activation: candidate generation must advance active generation for repo={} revision={}: candidate={} active={}",
                candidate.repo_id().as_str(),
                candidate.revision_id().as_str(),
                candidate.manifest_generation().get(),
                current.generation.lexical.manifest_generation.get(),
            )));
        }

        let activation_sequence = current_head.as_ref().map_or_else(
            || {
                NonZeroU64::new(1).ok_or_else(|| {
                    CoreError::Storage("invalid initial activation sequence".to_string())
                })
            },
            ActiveSearchCorpusHeadV1::next_sequence,
        )?;
        if current_head.is_none()
            && entries
                .roots
                .keys()
                .filter(|key| &key.repo_id == candidate.repo_id())
                .count()
                >= repository_envelope::MAX_ROOTS
        {
            return Err(repository_envelope::capacity("revision root limit"));
        }
        let mut next = entries.repository(candidate.repo_id());
        source_events::activate_event(&mut next, candidate.lexical(), sealed_event)?;
        insert_search_corpus_generation_records(
            &mut next.roots,
            ActiveSearchCorpusHeadV1 {
                generation: candidate.clone(),
                activation_sequence,
            },
        );
        drop(entries);
        self.persist_repository(&mutation, candidate.repo_id(), next)?;

        Ok(SearchCorpusGenerationActivationV1 {
            active: ActiveSearchCorpusHeadV1 {
                generation: candidate.clone(),
                activation_sequence,
            }
            .to_contract_v1(self.root_incarnation)?,
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
        self.activate_prepared_under_guard_v1(&guard, prepared, None)
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
            SearchCorpusGenerationV1::from_contract_v1(&request.expected_active.generation)?;
        let target = SearchCorpusGenerationV1::from_contract_v1(&request.target)?;
        repository_envelope::bound_manifest_token(&target.lexical().manifest_digest)?;
        repository_envelope::bound_manifest_token(&target.semantic().manifest_digest)?;

        guard.require_pair_v1(
            self.lifecycle_coordinator.as_ref(),
            expected_active.repo_id(),
            expected_active.revision_id(),
        )?;
        let mutation = self.lock_mutation()?;
        let entries = self
            .entries
            .read()
            .map_err(|error| repository_envelope::corrupt(error.to_string()))?;
        self.ensure_durability_certain_v1()?;
        let current_head = active_search_corpus_head_v1(
            &entries.roots,
            expected_active.repo_id(),
            expected_active.revision_id(),
        )
        .ok_or_else(|| {
            CoreError::NotReady("search-corpus rollback: no active composite generation".into())
        })?;
        let current = current_head.to_contract_v1(self.root_incarnation)?;
        if current != request.expected_active {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::RollbackCasConflict,
                message: format!(
                    "search-corpus rollback: active composite root changed for repo={} revision={}: expected generation={} digest={}, observed generation={} digest={}",
                    expected_active.repo_id().as_str(),
                    expected_active.revision_id().as_str(),
                    expected_active.manifest_generation().get(),
                    expected_active.manifest_digest(),
                    current.generation.lexical.manifest_generation.get(),
                    current.generation.lexical.manifest_digest,
                ),
            });
        }

        let activation_sequence = current_head.next_sequence()?;
        let mut next = entries.repository(target.repo_id());
        insert_search_corpus_generation_records(
            &mut next.roots,
            ActiveSearchCorpusHeadV1 {
                generation: target.clone(),
                activation_sequence,
            },
        );
        // Administrative rollback does not erase accepted source lineage.
        drop(entries);
        self.persist_repository(&mutation, target.repo_id(), next)?;

        Ok(SearchPlaneSearchCorpusRollbackCasAck {
            active: ActiveSearchCorpusHeadV1 {
                generation: target,
                activation_sequence,
            }
            .to_contract_v1(self.root_incarnation)?,
            previous_sealed_active: current,
        })
    }

    #[cfg(test)]
    pub(crate) fn rollback(
        &self,
        request: &SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    ) -> Result<SearchPlaneSearchCorpusRollbackCasAck, CoreError> {
        let expected_active =
            SearchCorpusGenerationV1::from_contract_v1(&request.expected_active.generation)?;
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
                .roots
                .get(&ActivationKey::for_pair(repo_id, revision_id))
                .ok_or_else(not_ready)?;
            match track {
                SearchPlaneTrackKind::Lexical => active_record_of(generation.generation.lexical()),
                SearchPlaneTrackKind::Semantic => {
                    active_record_of(generation.generation.semantic())
                }
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
            .roots
            .get(&ActivationKey::for_pair(repo_id, revision_id))
            .map(|generation| {
                vec![
                    active_record_of(generation.generation.lexical()),
                    active_record_of(generation.generation.semantic()),
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
            &entries.roots,
            repo_id,
            revision_id,
        ))
    }

    /// Read generation and activation token from one catalog snapshot.
    /// Query resolution and CAS must use this pair rather than reconstructing
    /// a token from a manifest generation or a separate read view.
    pub fn active_search_corpus_with_token_v1(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Option<(SearchCorpusGenerationV1, SearchCorpusActivationTokenV1)>, CoreError> {
        let entries = self.entries.read().map_err(|err| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
        })?;
        self.ensure_durability_certain_v1()?;
        active_search_corpus_head_v1(&entries.roots, repo_id, revision_id)
            .map(|head| {
                let token = head.activation_token_v1(self.root_incarnation)?;
                Ok((head.generation, token))
            })
            .transpose()
    }

    fn lock_mutation(&self) -> Result<MutexGuard<'_, ()>, CoreError> {
        self.mutation
            .lock()
            .map_err(|error| repository_envelope::corrupt(error.to_string()))
    }

    fn persist_repository(
        &self,
        _mutation: &MutexGuard<'_, ()>,
        repo: &RepoId,
        next: CatalogState,
    ) -> Result<(), CoreError> {
        self.ensure_durability_certain_v1()?;
        let bytes = repository_envelope::encode(repo, &next)?;
        let path = self
            .activations_dir
            .join(repository_envelope::file_name(repo));
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
        let mut current = self
            .entries
            .write()
            .map_err(|error| repository_envelope::corrupt(error.to_string()))?;
        self.ensure_durability_certain_v1()?;
        current.roots.retain(|key, _| &key.repo_id != repo);
        current.roots.extend(next.roots);
        let _prior = current.histories.remove(repo);
        current.histories.extend(next.histories);
        Ok(())
    }

    fn ensure_durability_certain_v1(&self) -> Result<(), CoreError> {
        if self.durability_uncertain_v1.load(Ordering::Acquire) || self.mutation.is_poisoned() {
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
            &entries.roots,
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
        let active = entries
            .roots
            .values()
            .map(|head| head.generation.clone())
            .collect();
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
    active_search_corpus_head_v1(entries, repo_id, revision_id).map(|head| head.generation)
}

fn active_search_corpus_head_v1(
    entries: &ActiveRoots,
    repo_id: &RepoId,
    revision_id: &RevisionId,
) -> Option<ActiveSearchCorpusHeadV1> {
    entries
        .get(&ActivationKey::for_pair(repo_id, revision_id))
        .cloned()
}

fn insert_search_corpus_generation_records(
    entries: &mut ActiveRoots,
    head: ActiveSearchCorpusHeadV1,
) {
    let _prior = entries.insert(
        ActivationKey::for_pair(head.generation.repo_id(), head.generation.revision_id()),
        head,
    );
}

#[cfg(test)]
pub(super) fn search_corpus_root_file_name(repo_id: &RepoId, _revision_id: &RevisionId) -> String {
    repository_envelope::file_name(repo_id)
}

fn is_search_corpus_root_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("--corpus.json"))
}

fn load_or_create_root_incarnation_v1(
    root: &Path,
    allow_create: bool,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<[u8; ROOT_INCARNATION_BYTES_V1], CoreError> {
    let path = root.join(ROOT_INCARNATION_FILE_V1);
    match fs::symlink_metadata(&path) {
        Ok(_) => return read_root_incarnation_v1(&path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "search-plane activation catalog: inspect root incarnation {}: {error}",
                path.display()
            )));
        }
    }
    if !allow_create {
        return Err(CoreError::Storage(format!(
            "search-plane activation catalog: active roots lack root incarnation at {}",
            path.display()
        )));
    }
    let value = fresh_root_incarnation_v1()?;
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return read_root_incarnation_v1(&path);
        }
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "search-plane activation catalog: create root incarnation {}: {error}",
                path.display()
            )));
        }
    };
    file.write_all(&value)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            CoreError::Storage(format!(
                "search-plane activation catalog: persist root incarnation {}: {error}",
                path.display()
            ))
        })?;
    parent_sync.sync_parent(root).map_err(|error| {
        CoreError::Storage(format!(
            "search-plane activation catalog: sync root incarnation parent {}: {error}",
            root.display()
        ))
    })?;
    read_root_incarnation_v1(&path)
}

fn read_root_incarnation_v1(path: &Path) -> Result<[u8; ROOT_INCARNATION_BYTES_V1], CoreError> {
    let bytes = read_regular_file_nofollow_v1(path).map_err(|error| {
        CoreError::Storage(format!(
            "search-plane activation catalog: read root incarnation {}: {error}",
            path.display()
        ))
    })?;
    let value: [u8; ROOT_INCARNATION_BYTES_V1] = bytes.try_into().map_err(|bytes: Vec<u8>| {
        CoreError::Storage(format!(
            "search-plane activation catalog: root incarnation has {} bytes, expected {}",
            bytes.len(),
            ROOT_INCARNATION_BYTES_V1
        ))
    })?;
    if value.iter().all(|byte| *byte == 0) {
        return Err(CoreError::Storage(
            "search-plane activation catalog: root incarnation is zero".to_string(),
        ));
    }
    Ok(value)
}

fn fresh_root_incarnation_v1() -> Result<[u8; ROOT_INCARNATION_BYTES_V1], CoreError> {
    let mut entropy = fs::File::open("/dev/urandom").map_err(|error| {
        CoreError::Storage(format!(
            "search-plane activation catalog: open entropy source: {error}"
        ))
    })?;
    let mut value = [0u8; ROOT_INCARNATION_BYTES_V1];
    entropy.read_exact(&mut value).map_err(|error| {
        CoreError::Storage(format!(
            "search-plane activation catalog: read root incarnation entropy: {error}"
        ))
    })?;
    if value.iter().all(|byte| *byte == 0) {
        return Err(CoreError::Storage(
            "search-plane activation catalog: entropy returned zero root incarnation".to_string(),
        ));
    }
    Ok(value)
}
