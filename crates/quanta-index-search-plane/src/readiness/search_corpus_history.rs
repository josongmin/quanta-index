//! Durable sealed search-corpus history under `authorities/search-corpus/`:
//! one retention-bounded record per sealed generation, its admission,
//! loading, projection validation, and retention receipts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::{fmt, fs};

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::CoreError;
use quanta_index_ipc::{decode_cbor_payload, encode_cbor_payload};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::readiness::auxiliary_store::AuxiliaryAuthorityStore;
use crate::readiness::durable_fs::{
    AtomicFileWriteOutcomeV1, atomic_replace_file_from_staging_v1, ensure_durable_directory_v1,
    read_regular_file_nofollow_v1, sync_existing_file_parent_v1,
};
use crate::readiness::errors::ERR_SEARCH_CORPUS_AUTHORITY_CONFLICT;
use crate::readiness::ledger::Ledger;
use crate::readiness::pair_digest::search_corpus_pair_digest;
use crate::readiness::retention_receipt::SearchCorpusHistoryRetentionReceiptV1;
use crate::readiness::search_corpus_generation::SearchCorpusGenerationV1;
use crate::readiness::serde_support::impl_struct_serde;
use crate::search_corpus_retention::{
    ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED, SearchCorpusHistoryRetentionItemV1,
};

impl AuxiliaryAuthorityStore {
    /// Persist one complete sealed corpus as one immutable generation record.
    ///
    /// Admission performs one authoritative state-root scan, O(P + G), where
    /// `P` is the number of repo/revision pairs and `G` is the number of
    /// retained generation records. The resulting snapshot is reused for
    /// pair-local GC; no second pair scan occurs. Cross-pair deletion is
    /// intentionally forbidden because this owner has no product-active pin
    /// authority for choosing a safe victim.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the pair guard must remain held through active-pin validation, durable record admission, and retention GC"
    )]
    pub fn record_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError> {
        let pair_guard = self.lifecycle_coordinator.lock_pair(repo_id, revision_id)?;
        let active = self.active_pins.active_search_corpus_under_guard_v1(
            &pair_guard,
            repo_id,
            revision_id,
        )?;
        let _root_guard = self.search_corpus_root_lock.lock().map_err(|err| {
            CoreError::Storage(format!(
                "search-corpus authority: state-root retention lock poisoned: {err}"
            ))
        })?;
        let pair_dir = self.search_corpus_pair_dir(repo_id, revision_id);
        let mut root_snapshot = self.load_search_corpus_root_snapshot_v1()?;
        let _pair_directory_was_present = root_snapshot.pair_directories.remove(&pair_dir);
        let pair_records = root_snapshot.pairs.remove(&pair_dir).unwrap_or_default();
        for authority in &pair_records {
            if authority.record.repo_id != *repo_id || authority.record.revision_id != *revision_id
            {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: pair directory contains foreign identity in {}",
                    authority.path.display()
                )));
            }
        }
        let path = self.search_corpus_authority_path(repo_id, revision_id, generation);
        let existing_digest = pair_records
            .iter()
            .find(|authority| authority.record.generation == generation)
            .map(|authority| authority.record.manifest_digest.clone());
        if let Some(existing_digest) = existing_digest {
            if existing_digest == manifest_digest {
                return self.reconcile_existing_search_corpus_record_v1(
                    repo_id,
                    revision_id,
                    generation,
                    &path,
                    &pair_dir,
                    active.as_ref(),
                    &root_snapshot,
                    pair_records,
                );
            }
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_AUTHORITY_CONFLICT.to_string(),
                message: format!(
                    "search-corpus authority: conflicting digest for repo={} revision={} generation={}: expected={}, observed={}",
                    repo_id.as_str(),
                    revision_id.as_str(),
                    generation.get(),
                    manifest_digest,
                    existing_digest,
                ),
            });
        }
        self.persist_new_search_corpus_record_v1(
            repo_id,
            revision_id,
            generation,
            manifest_digest,
            &path,
            &pair_dir,
            active.as_ref(),
            &root_snapshot,
            pair_records,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "exact retry reconciliation consumes one complete immutable pair/root snapshot and its active pin"
    )]
    fn reconcile_existing_search_corpus_record_v1(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        path: &Path,
        pair_dir: &Path,
        active: Option<&SearchCorpusGenerationV1>,
        root_without_pair: &SearchCorpusAuthorityRootSnapshotV1,
        pair_records: Vec<SearchCorpusAuthorityFileV1>,
    ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError> {
        ensure_durable_directory_v1(
            pair_dir,
            "search-corpus authority",
            self.parent_sync.as_ref(),
        )?;
        sync_existing_file_parent_v1(path, "search-corpus authority", self.parent_sync.as_ref())?;
        self.sync_search_corpus_staging_after_exact_retry_v1()?;
        let plan =
            self.plan_search_corpus_pair_records_v1(&pair_records, Some(generation), active)?;
        self.validate_search_corpus_state_root_projection_v1(
            root_without_pair,
            &pair_records,
            &plan,
        )?;
        let enforced = self.enforce_search_corpus_history_retention_snapshot_v1(
            repo_id,
            revision_id,
            pair_dir,
            pair_records,
            &plan,
        )?;
        if enforced
            .retained
            .iter()
            .any(|authority| authority.record.generation == generation)
        {
            return Ok(enforced.receipt);
        }
        Err(CoreError::Typed {
            code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
            message: format!(
                "search-corpus history retention: generation {} has been reaped for repo={} revision={}",
                generation.get(),
                repo_id.as_str(),
                revision_id.as_str(),
            ),
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the immutable record admission joins one exact pair identity, path pair, active pin, root snapshot, and pair snapshot"
    )]
    fn persist_new_search_corpus_record_v1(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
        path: &Path,
        pair_dir: &Path,
        active: Option<&SearchCorpusGenerationV1>,
        root_without_pair: &SearchCorpusAuthorityRootSnapshotV1,
        mut pair_records: Vec<SearchCorpusAuthorityFileV1>,
    ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError> {
        let record =
            SearchCorpusAuthorityRecordV1::new(repo_id, revision_id, generation, manifest_digest);
        let bytes = encode_cbor_payload(&record).map_err(|err| {
            CoreError::Storage(format!(
                "search-corpus authority: encode {}: {err}",
                path.display()
            ))
        })?;
        let encoded_len = u64::try_from(bytes.len()).map_err(|_error| {
            CoreError::Storage(
                "search-corpus history retention: candidate record length exceeds u64".to_string(),
            )
        })?;
        if encoded_len > self.search_corpus_history_retention.max_bytes() {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
                message: format!(
                    "search-corpus history retention: generation {} record exceeds max_bytes={} for repo={} revision={}",
                    generation.get(),
                    self.search_corpus_history_retention.max_bytes(),
                    repo_id.as_str(),
                    revision_id.as_str(),
                ),
            });
        }
        pair_records.push(SearchCorpusAuthorityFileV1 {
            path: path.to_path_buf(),
            record,
            encoded_len,
        });
        pair_records.sort_by(|left, right| {
            right
                .record
                .generation
                .get()
                .cmp(&left.record.generation.get())
        });
        let plan =
            self.plan_search_corpus_pair_records_v1(&pair_records, Some(generation), active)?;
        self.validate_search_corpus_state_root_projection_v1(
            root_without_pair,
            &pair_records,
            &plan,
        )?;
        ensure_durable_directory_v1(
            pair_dir,
            "search-corpus authority",
            self.parent_sync.as_ref(),
        )?;
        match atomic_replace_file_from_staging_v1(
            path,
            &bytes,
            &self.search_corpus_staging_dir,
            "search-corpus authority",
            self.parent_sync.as_ref(),
        )? {
            AtomicFileWriteOutcomeV1::Durable => {
                let enforced = self.enforce_search_corpus_history_retention_snapshot_v1(
                    repo_id,
                    revision_id,
                    pair_dir,
                    pair_records,
                    &plan,
                )?;
                Ok(enforced.receipt)
            }
            AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(error) => Err(error),
        }
    }

    fn sync_search_corpus_staging_after_exact_retry_v1(&self) -> Result<(), CoreError> {
        self.parent_sync
            .sync_parent(&self.search_corpus_staging_dir)
            .map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: exact retry failed to fsync staging parent {}: {error}",
                    self.search_corpus_staging_dir.display(),
                ))
            })
    }

    pub fn inspect_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError> {
        let path = self.search_corpus_authority_path(repo_id, revision_id, generation);
        let Some(record) =
            self.read_cbor::<SearchCorpusAuthorityRecordV1>(&path, "search corpus")?
        else {
            return Ok(SealedSearchCorpusAuthorityStateV1::Absent);
        };
        record.validate_identity(repo_id, revision_id, generation, &path)?;
        if record.manifest_digest != manifest_digest {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_AUTHORITY_CONFLICT.to_string(),
                message: format!(
                    "search-corpus authority: conflicting digest for repo={} revision={} generation={}",
                    repo_id.as_str(),
                    revision_id.as_str(),
                    generation.get(),
                ),
            });
        }
        Ok(SealedSearchCorpusAuthorityStateV1::Exact)
    }

    pub(super) fn load_search_corpus_root_snapshot_v1(
        &self,
    ) -> Result<SearchCorpusAuthorityRootSnapshotV1, CoreError> {
        let mut snapshot = SearchCorpusAuthorityRootSnapshotV1::default();
        for pair_entry in fs::read_dir(&self.search_corpus_dir).map_err(|err| {
            CoreError::Storage(format!(
                "search-corpus authority: list {}: {err}",
                self.search_corpus_dir.display()
            ))
        })? {
            let pair_entry = pair_entry.map_err(|err| {
                CoreError::Storage(format!(
                    "search-corpus authority: read directory entry in {}: {err}",
                    self.search_corpus_dir.display()
                ))
            })?;
            let pair_path = pair_entry.path();
            if pair_path == self.search_corpus_staging_dir {
                continue;
            }
            if !pair_entry
                .file_type()
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "search-corpus authority: inspect {}: {err}",
                        pair_path.display()
                    ))
                })?
                .is_dir()
            {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: foreign root entry {}",
                    pair_path.display()
                )));
            }
            if !pair_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: invalid pair directory name {}",
                    pair_path.display()
                )));
            }
            let _inserted = snapshot.pair_directories.insert(pair_path.clone());
            let records = self.load_search_corpus_pair_records_v1(&pair_path)?;
            if records.is_empty() {
                continue;
            }
            for record in &records {
                snapshot.total_bytes = snapshot
                    .total_bytes
                    .checked_add(record.encoded_len)
                    .ok_or_else(|| {
                        CoreError::Storage(
                            "search-corpus history retention: state-root byte total overflow"
                                .to_string(),
                        )
                    })?;
            }
            let _previous = snapshot.pairs.insert(pair_path, records);
        }
        Ok(snapshot)
    }

    fn load_search_corpus_pair_records_v1(
        &self,
        pair_dir: &Path,
    ) -> Result<Vec<SearchCorpusAuthorityFileV1>, CoreError> {
        let mut records = Vec::new();
        for entry in fs::read_dir(pair_dir).map_err(|err| {
            CoreError::Storage(format!(
                "search-corpus authority: list {}: {err}",
                pair_dir.display()
            ))
        })? {
            let entry = entry.map_err(|err| {
                CoreError::Storage(format!(
                    "search-corpus authority: read directory entry in {}: {err}",
                    pair_dir.display()
                ))
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|err| {
                CoreError::Storage(format!(
                    "search-corpus authority: inspect {}: {err}",
                    path.display()
                ))
            })?;
            if !file_type.is_file() || path.extension().is_none_or(|extension| extension != "cbor")
            {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: foreign history entry {}",
                    path.display()
                )));
            }
            let bytes = read_regular_file_nofollow_v1(&path).map_err(|err| {
                CoreError::Storage(format!(
                    "search-corpus authority: read {}: {err}",
                    path.display()
                ))
            })?;
            let record = decode_cbor_payload::<SearchCorpusAuthorityRecordV1>(bytes.as_slice())
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "search-corpus authority: decode {}: {err}",
                        path.display()
                    ))
                })?;
            record.validate_identity(
                &record.repo_id,
                &record.revision_id,
                record.generation,
                &path,
            )?;
            let expected_path = self.search_corpus_authority_path(
                &record.repo_id,
                &record.revision_id,
                record.generation,
            );
            if expected_path != path {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: filename/payload identity mismatch in {}",
                    path.display()
                )));
            }
            let encoded_len = u64::try_from(bytes.len()).map_err(|_error| {
                CoreError::Storage(format!(
                    "search-corpus authority: record length exceeds u64 in {}",
                    path.display()
                ))
            })?;
            records.push(SearchCorpusAuthorityFileV1 {
                path,
                record,
                encoded_len,
            });
        }
        records.sort_by(|left, right| {
            right
                .record
                .generation
                .get()
                .cmp(&left.record.generation.get())
        });
        Ok(records)
    }

    pub(super) fn plan_search_corpus_pair_records_v1(
        &self,
        records: &[SearchCorpusAuthorityFileV1],
        required_generation: Option<ManifestGeneration>,
        active: Option<&SearchCorpusGenerationV1>,
    ) -> Result<crate::search_corpus_retention::SearchCorpusHistoryRetentionPlanV1, CoreError> {
        if let Some(active) = active {
            let exact = records.iter().any(|record| {
                record.record.generation == active.manifest_generation()
                    && record.record.manifest_digest.as_str() == active.manifest_digest()
            });
            if !exact {
                return Err(CoreError::Storage(format!(
                    "search-corpus history retention: active generation is absent from durable history for repo={} revision={} generation={} digest={}",
                    active.repo_id().as_str(),
                    active.revision_id().as_str(),
                    active.manifest_generation().get(),
                    active.manifest_digest(),
                )));
            }
        }
        self.search_corpus_history_retention.plan(
            records
                .iter()
                .map(|record| SearchCorpusHistoryRetentionItemV1 {
                    generation: record.record.generation.get(),
                    encoded_len: record.encoded_len,
                    candidate: required_generation == Some(record.record.generation),
                    active: active.is_some_and(|active| {
                        active.manifest_generation() == record.record.generation
                            && active.manifest_digest() == record.record.manifest_digest.as_str()
                    }),
                })
                .collect(),
        )
    }

    fn validate_search_corpus_state_root_projection_v1(
        &self,
        root_without_pair: &SearchCorpusAuthorityRootSnapshotV1,
        pair_records: &[SearchCorpusAuthorityFileV1],
        plan: &crate::search_corpus_retention::SearchCorpusHistoryRetentionPlanV1,
    ) -> Result<(), CoreError> {
        let existing_pair_bytes = pair_records
            .iter()
            .filter(|record| record.path.exists())
            .try_fold(0_u64, |total, record| {
                total.checked_add(record.encoded_len).ok_or_else(|| {
                    CoreError::Storage(
                        "search-corpus history retention: pair byte total overflow".to_string(),
                    )
                })
            })?;
        let retained_pair_bytes = pair_records
            .iter()
            .filter(|record| plan.retains(record.record.generation.get()))
            .try_fold(0_u64, |total, record| {
                total.checked_add(record.encoded_len).ok_or_else(|| {
                    CoreError::Storage(
                        "search-corpus history retention: retained byte total overflow".to_string(),
                    )
                })
            })?;
        let projected_pairs = root_without_pair
            .pair_directories
            .len()
            .checked_add(1)
            .ok_or_else(|| {
                CoreError::Storage(
                    "search-corpus history retention: revision-pair count overflow".to_string(),
                )
            })?;
        let projected_total_bytes = root_without_pair
            .total_bytes
            .checked_sub(existing_pair_bytes)
            .and_then(|remaining| remaining.checked_add(retained_pair_bytes))
            .ok_or_else(|| {
                CoreError::Storage(
                    "search-corpus history retention: projected state-root byte total overflow"
                        .to_string(),
                )
            })?;
        if projected_pairs > self.search_corpus_history_retention.max_revision_pairs()
            || projected_total_bytes > self.search_corpus_history_retention.max_total_bytes()
        {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
                message: format!(
                    "search-corpus history retention: state-root admission requires revision_pairs={projected_pairs} total_bytes={projected_total_bytes}, limits are max_revision_pairs={} max_total_bytes={}; cross-pair deletion is unavailable without product-active pin authority",
                    self.search_corpus_history_retention.max_revision_pairs(),
                    self.search_corpus_history_retention.max_total_bytes(),
                ),
            });
        }
        Ok(())
    }

    pub(super) fn enforce_search_corpus_history_retention_snapshot_v1(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        pair_dir: &Path,
        records: Vec<SearchCorpusAuthorityFileV1>,
        plan: &crate::search_corpus_retention::SearchCorpusHistoryRetentionPlanV1,
    ) -> Result<EnforcedSearchCorpusHistoryRetentionV1, CoreError> {
        for record in &records {
            if &record.record.repo_id != repo_id || &record.record.revision_id != revision_id {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: pair directory contains foreign identity in {}",
                    record.path.display()
                )));
            }
        }

        let (retained, reaped): (Vec<_>, Vec<_>) = records
            .into_iter()
            .partition(|record| plan.retains(record.record.generation.get()));
        let retained_generations = retained
            .iter()
            .map(|record| record.record.generation)
            .collect();
        let reaped_generations = reaped
            .iter()
            .map(|record| record.record.generation)
            .collect();
        for record in &reaped {
            fs::remove_file(&record.path).map_err(|err| {
                CoreError::Storage(format!(
                    "search-corpus history retention: remove reaped authority {}: {err}",
                    record.path.display()
                ))
            })?;
        }
        if !reaped.is_empty() {
            self.parent_sync.sync_parent(pair_dir).map_err(|err| {
                CoreError::Storage(format!(
                    "search-corpus history retention: fsync pair directory {} after GC: {err}",
                    pair_dir.display()
                ))
            })?;
        }
        Ok(EnforcedSearchCorpusHistoryRetentionV1 {
            retained,
            receipt: SearchCorpusHistoryRetentionReceiptV1 {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                retained_generations,
                reaped_generations,
                store_reconciled_v1: true,
            },
        })
    }

    pub(super) fn restore_search_corpus_history_into(
        &self,
        ledger: &mut Ledger,
    ) -> Result<(), CoreError> {
        let _root_guard = self.search_corpus_root_lock.lock().map_err(|err| {
            CoreError::Storage(format!(
                "search-corpus authority: restore state-root retention lock poisoned: {err}"
            ))
        })?;
        let snapshot = self.load_search_corpus_root_snapshot_v1()?;
        let active_corpora = self
            .active_pins
            .all_active_search_corpora_for_bootstrap_v1()?;
        for active in &active_corpora {
            let exact = snapshot.pairs.values().flatten().any(|record| {
                &record.record.repo_id == active.repo_id()
                    && &record.record.revision_id == active.revision_id()
                    && record.record.generation == active.manifest_generation()
                    && record.record.manifest_digest.as_str() == active.manifest_digest()
            });
            if !exact {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: active composite root has no exact durable history record for repo={} revision={} generation={}",
                    active.repo_id().as_str(),
                    active.revision_id().as_str(),
                    active.manifest_generation().get(),
                )));
            }
        }
        if snapshot.pair_directories.len()
            > self.search_corpus_history_retention.max_revision_pairs()
        {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
                message: format!(
                    "search-corpus history retention: restore observed {} revision pairs, exceeding max_revision_pairs={}; cross-pair deletion is unavailable without product-active pin authority",
                    snapshot.pair_directories.len(),
                    self.search_corpus_history_retention.max_revision_pairs(),
                ),
            });
        }
        let mut planned = Vec::with_capacity(snapshot.pairs.len());
        let mut projected_total_bytes = 0_u64;
        for (pair_path, observed) in snapshot.pairs {
            let first = observed.first().ok_or_else(|| {
                CoreError::Storage(format!(
                    "search-corpus history retention: non-empty root snapshot lost pair records for {}",
                    pair_path.display()
                ))
            })?;
            let repo_id = first.record.repo_id.clone();
            let revision_id = first.record.revision_id.clone();
            let active = active_corpora.iter().find(|active| {
                active.repo_id() == &repo_id && active.revision_id() == &revision_id
            });
            let plan = self.plan_search_corpus_pair_records_v1(&observed, None, active)?;
            for record in observed
                .iter()
                .filter(|record| plan.retains(record.record.generation.get()))
            {
                projected_total_bytes = projected_total_bytes
                    .checked_add(record.encoded_len)
                    .ok_or_else(|| {
                        CoreError::Storage(
                            "search-corpus history retention: restore byte total overflow"
                                .to_string(),
                        )
                    })?;
            }
            planned.push((pair_path, observed, repo_id, revision_id, plan));
        }
        if projected_total_bytes > self.search_corpus_history_retention.max_total_bytes() {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
                message: format!(
                    "search-corpus history retention: restore requires total_bytes={projected_total_bytes}, exceeding max_total_bytes={}; cross-pair deletion is unavailable without product-active pin authority",
                    self.search_corpus_history_retention.max_total_bytes(),
                ),
            });
        }
        for (pair_path, observed, repo_id, revision_id, plan) in planned {
            let enforced = self.enforce_search_corpus_history_retention_snapshot_v1(
                &repo_id,
                &revision_id,
                &pair_path,
                observed,
                &plan,
            )?;
            for authority in enforced.retained {
                let record = authority.record;
                ledger.record_historically_sealed_search_corpus(
                    &record.repo_id,
                    &record.revision_id,
                    record.generation,
                    &record.manifest_digest,
                );
            }
        }
        Ok(())
    }

    pub(super) fn search_corpus_pair_dir(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> PathBuf {
        self.search_corpus_dir
            .join(search_corpus_pair_digest(repo_id, revision_id))
    }

    pub(super) fn search_corpus_authority_path(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> PathBuf {
        self.search_corpus_pair_dir(repo_id, revision_id)
            .join(format!("g{}.cbor", generation.get()))
    }

    pub(super) fn read_cbor<T: for<'de> Deserialize<'de>>(
        &self,
        path: &Path,
        label: &str,
    ) -> Result<Option<T>, CoreError> {
        let bytes = match read_regular_file_nofollow_v1(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(CoreError::Storage(format!(
                    "search-plane authority store: read {label} {}: {err}",
                    path.display()
                )));
            }
        };
        decode_cbor_payload(bytes.as_slice())
            .map(Some)
            .map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane authority store: decode {label} {}: {err}",
                    path.display()
                ))
            })
    }
}

pub(super) const SEARCH_CORPUS_AUTHORITY_SCHEMA_V1: u32 = 1;

#[derive(Clone, Debug)]
pub(super) struct SearchCorpusAuthorityRecordV1 {
    schema_version: u32,
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    manifest_digest: String,
}

#[derive(Debug)]
pub(super) struct SearchCorpusAuthorityFileV1 {
    path: PathBuf,
    record: SearchCorpusAuthorityRecordV1,
    encoded_len: u64,
}

#[derive(Debug, Default)]
pub(super) struct SearchCorpusAuthorityRootSnapshotV1 {
    pub(super) pair_directories: BTreeSet<PathBuf>,
    pub(super) pairs: BTreeMap<PathBuf, Vec<SearchCorpusAuthorityFileV1>>,
    total_bytes: u64,
}

#[derive(Debug)]
pub(super) struct EnforcedSearchCorpusHistoryRetentionV1 {
    pub(super) retained: Vec<SearchCorpusAuthorityFileV1>,
    receipt: SearchCorpusHistoryRetentionReceiptV1,
}

impl SearchCorpusAuthorityRecordV1 {
    pub(super) fn new(
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Self {
        Self {
            schema_version: SEARCH_CORPUS_AUTHORITY_SCHEMA_V1,
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            generation,
            manifest_digest: manifest_digest.to_string(),
        }
    }

    fn validate_identity(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        path: &Path,
    ) -> Result<(), CoreError> {
        if self.schema_version != SEARCH_CORPUS_AUTHORITY_SCHEMA_V1 {
            return Err(CoreError::Storage(format!(
                "search-corpus authority: unsupported schema version {} in {}",
                self.schema_version,
                path.display()
            )));
        }
        if &self.repo_id != repo_id
            || &self.revision_id != revision_id
            || self.generation != generation
        {
            return Err(CoreError::Storage(format!(
                "search-corpus authority: filename/payload identity mismatch in {}",
                path.display()
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SealedSearchCorpusAuthorityStateV1 {
    Absent,
    Exact,
}

impl_struct_serde!(SearchCorpusAuthorityRecordV1 {
    schema_version: u32,
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    manifest_digest: String,
});
