//! Request-local inode membership with custody over semantic generation writes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{MutexGuard, RwLockReadGuard, TryLockError};

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneErrorCodeV2};
use quanta_index_core::domains::generation::{
    SealedGenerationByteSession, SealedGenerationBytesV1,
};
use quanta_index_core::{CoreError, DiagnosticContextV1, DiagnosticStageV1, RequestBudgetV1};
use quanta_index_scratch_sqlite::retention::RetainedInodeMembership;
use sha2::{Digest as _, Sha256};

use crate::{SemanticAdapter, control_file, layout, manifest::SemanticManifest};

pub(crate) struct SemanticRetentionSession<'a> {
    adapter: &'a SemanticAdapter,
    repo_id: RepoId,
    revision_id: RevisionId,
    diagnostic: Option<DiagnosticContextV1>,
    budget: RequestBudgetV1,
    membership: RetainedInodeMembership,
    mutation_guards: BTreeMap<usize, MutexGuard<'a, ()>>,
    _lifecycle: RwLockReadGuard<'a, ()>,
}

impl<'a> SemanticRetentionSession<'a> {
    pub(crate) fn open(
        adapter: &'a SemanticAdapter,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        budget: RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<Self, CoreError> {
        budget.checkpoint("semantic-retention:begin")?;
        let lifecycle = adapter.directory_lifecycle_read_guard()?;
        let membership = RetainedInodeMembership::open(
            adapter.retention_measurement_policy.max_main_file_bytes(),
        )?;
        Ok(Self {
            adapter,
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            diagnostic: diagnostic.cloned(),
            budget,
            membership,
            mutation_guards: BTreeMap::new(),
            _lifecycle: lifecycle,
        })
    }

    fn validate_identity(&mut self, generation: ManifestGeneration) -> Result<bool, CoreError> {
        self.budget.checkpoint("semantic-retention:identity")?;
        let directory = layout::generation_dir(
            &self.adapter.state_root,
            &self.repo_id,
            &self.revision_id,
            generation,
        );
        let marker = layout::sealed_marker_path(&directory);
        if !directory_present(&self.adapter.state_root)? {
            return Ok(false);
        }
        if !directory_present(&directory)? {
            return Ok(false);
        }
        self.pin_mutations(generation)?;
        if !control_file::regular_file_present(&marker).map_err(|error| {
            CoreError::Storage(format!(
                "semantic retention inspect seal {}: {error}",
                marker.display()
            ))
        })? {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::GenerationIdentityIncomplete,
                message: format!(
                    "semantic retention generation {} has no sealed identity",
                    generation.get()
                ),
            });
        }
        let digest = control_file::read_string_bounded_with_admission(
            &marker,
            control_file::MAX_SEALED_MARKER_BYTES,
            self.adapter.lance_read_admission.as_deref(),
        )
        .map_err(|error| crate::errors::fs_err("retention read seal", &marker, &error))?;
        if !self.membership.contains_generation(generation)? {
            let manifest_path = layout::manifest_path(&directory);
            let manifest = SemanticManifest::decode(
                &control_file::read_bounded_with_admission(
                    &manifest_path,
                    control_file::MAX_SCOPE_MANIFEST_BYTES,
                    self.adapter.lance_read_admission.as_deref(),
                )
                .map_err(|error| {
                    crate::errors::fs_err("retention read manifest", &manifest_path, &error)
                })?,
            )?;
            manifest.validate_scope(&self.repo_id, &self.revision_id, generation)?;
            if manifest.manifest_digest != digest {
                return Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
                    message: format!(
                        "semantic retention seal and manifest disagree for generation {}",
                        generation.get()
                    ),
                });
            }
        }
        self.membership
            .bind_generation(generation, Sha256::digest(digest.as_bytes()).into())?;
        Ok(true)
    }

    fn pin_mutations(&mut self, generation: ManifestGeneration) -> Result<(), CoreError> {
        let stripe = self.adapter.generation_mutation_stripe(
            &self.repo_id,
            &self.revision_id,
            generation,
        )?;
        if self.mutation_guards.contains_key(&stripe) {
            return Ok(());
        }
        let mutex = self
            .adapter
            .generation_mutations
            .get(stripe)
            .ok_or_else(|| {
                CoreError::InvalidContract(format!(
                    "semantic retention mutation stripe {stripe} is out of range"
                ))
            })?;
        // Writers acquire mutation custody before lifecycle custody. Waiting
        // here while holding a lifecycle reader could deadlock a reclaimer.
        let _prior_guard = self.mutation_guards.insert(
            stripe,
            mutex.try_lock().map_err(|error| match error {
                TryLockError::WouldBlock => CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
                    message: format!(
                        "semantic retention generation {} is being mutated; retry measurement",
                        generation.get()
                    ),
                },
                TryLockError::Poisoned(error) => CoreError::Storage(format!(
                    "semantic retention mutation lock poisoned: {error}"
                )),
            })?,
        );
        Ok(())
    }
}

fn directory_present(path: &Path) -> Result<bool, CoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(CoreError::Storage(format!(
            "semantic retention {} is not a directory",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(CoreError::Storage(format!(
            "semantic retention inspect directory {}: {error}",
            path.display()
        ))),
    }
}

impl SealedGenerationByteSession for SemanticRetentionSession<'_> {
    fn measure(
        &mut self,
        generations: &BTreeSet<ManifestGeneration>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        let diagnostic = self.diagnostic.clone();
        crate::observe_result(
            diagnostic.as_ref(),
            DiagnosticStageV1::RetentionMeasure,
            |_| {
                self.budget.checkpoint("semantic-retention:entry")?;
                let mut present = BTreeSet::new();
                let mut missing = BTreeSet::new();
                for &generation in generations {
                    self.budget.checkpoint("semantic-retention:generation")?;
                    if self.validate_identity(generation)? {
                        let _new_present = present.insert(generation);
                    } else {
                        let _new_missing = missing.insert(generation);
                    }
                }
                let mut measurement = self.membership.measure_with_budget(
                    &self.adapter.state_root,
                    &present,
                    &|generation| {
                        layout::generation_dir(
                            &self.adapter.state_root,
                            &self.repo_id,
                            &self.revision_id,
                            generation,
                        )
                    },
                    &|_name| false,
                    &self.budget,
                    diagnostic.as_ref(),
                )?;
                measurement.absent.extend(missing);
                self.budget.checkpoint("semantic-retention:complete")?;
                Ok(measurement)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::domains::generation::SealedGenerationReclaimPort as _;
    use std::os::unix::fs::MetadataExt as _;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(format!("fixture assertion failed: {}", stringify!($condition)).into());
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if left != right => {
                    return Err(format!(
                        "fixture equality failed: {} = {left:?}, {} = {right:?}",
                        stringify!($left),
                        stringify!($right)
                    )
                    .into());
                }
                _ => {}
            }
        }};
    }

    fn build(adapter: &SemanticAdapter, generation: u64) -> Result<(), Box<dyn std::error::Error>> {
        let record = crate::legacy_chunk_embedding_record_v1(
            "retention",
            "src/one.rs",
            vec![1.0, 0.0, 0.0],
        )?;
        crate::build_resident_batch_v1(
            adapter,
            &crate::sealed_replace_batch_v1(
                RepoId::new("retention")?,
                RevisionId::new("revision")?,
                ManifestGeneration::new(generation),
                "src/one.rs",
                vec![record],
                3,
            ),
        )?;
        Ok(())
    }

    fn physical_bytes(root: &Path) -> Result<u64, Box<dyn std::error::Error>> {
        let mut seen = BTreeSet::new();
        let mut pending = vec![root.to_path_buf()];
        let mut bytes = 0_u64;
        while let Some(path) = pending.pop() {
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                for entry in std::fs::read_dir(&path)? {
                    pending.push(entry?.path());
                }
            } else if metadata.is_file() && seen.insert((metadata.dev(), metadata.ino())) {
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or("fixture byte overflow")?;
            }
        }
        Ok(bytes)
    }

    #[test]
    fn production_session_counts_shared_inodes_once_for_arbitrary_subsets()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let adapter = SemanticAdapter::with_state_root(directory.path().to_path_buf())?;
        let repo = RepoId::new("retention")?;
        let revision = RevisionId::new("revision")?;
        build(&adapter, 1)?;
        build(&adapter, 2)?;
        let first = layout::generation_dir(
            directory.path(),
            &repo,
            &revision,
            ManifestGeneration::new(1),
        );
        let second = layout::generation_dir(
            directory.path(),
            &repo,
            &revision,
            ManifestGeneration::new(2),
        );
        let shared = b"shared-retention-bytes";
        std::fs::write(first.join("shared-fixture"), shared)?;
        std::fs::hard_link(first.join("shared-fixture"), second.join("shared-fixture"))?;
        let first_bytes = physical_bytes(&first)?;
        let second_bytes = physical_bytes(&second)?;
        let shared_bytes = u64::try_from(shared.len())?;
        let both_bytes = first_bytes
            .checked_add(second_bytes)
            .and_then(|bytes| bytes.checked_sub(shared_bytes))
            .ok_or("fixture union overflow")?;
        let mut session =
            adapter.begin_pair_measurement(&repo, &revision, RequestBudgetV1::unbounded(), None)?;
        for (requested, gold) in [
            (vec![1], first_bytes),
            (vec![1, 2], both_bytes),
            (vec![2], second_bytes),
            (vec![1, 2], both_bytes),
            (vec![], 0),
        ] {
            let requested = requested.into_iter().map(ManifestGeneration::new).collect();
            let measured = session.measure(&requested)?;
            check_eq!(measured.bytes, gold);
            check!(measured.absent.is_empty());
        }
        Ok(())
    }

    #[test]
    fn production_session_reprobes_absence_and_refuses_changed_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let adapter = SemanticAdapter::with_state_root(directory.path().to_path_buf())?;
        let repo = RepoId::new("retention")?;
        let revision = RevisionId::new("revision")?;
        let generation = ManifestGeneration::new(3);
        let requested = BTreeSet::from([generation]);
        let mut session =
            adapter.begin_pair_measurement(&repo, &revision, RequestBudgetV1::unbounded(), None)?;
        check_eq!(session.measure(&requested)?.absent, requested);
        let stripe = adapter.generation_mutation_stripe(&repo, &revision, generation)?;
        drop(
            adapter
                .generation_mutations
                .get(stripe)
                .ok_or("missing fixture mutation stripe")?
                .try_lock()
                .map_err(|error| format!("fixture generation mutation retry: {error}"))?,
        );
        build(&adapter, 3)?;
        let measured = session.measure(&requested)?;
        check!(measured.absent.is_empty());
        check!(measured.bytes > 0);
        let root = layout::generation_dir(directory.path(), &repo, &revision, generation);
        std::fs::write(layout::sealed_marker_path(&root), "changed-seal")?;
        check!(session.measure(&requested).is_err());
        std::fs::write(layout::sealed_marker_path(&root), "manifest:3")?;
        check_eq!(session.measure(&requested)?.bytes, measured.bytes);
        Ok(())
    }

    #[test]
    fn production_session_refuses_busy_mutation_and_holds_custody_until_drop()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let adapter = SemanticAdapter::with_state_root(directory.path().to_path_buf())?;
        let repo = RepoId::new("retention")?;
        let revision = RevisionId::new("revision")?;
        let generation = ManifestGeneration::new(1);
        build(&adapter, 1)?;
        let stripe = adapter.generation_mutation_stripe(&repo, &revision, generation)?;
        let mutex = adapter
            .generation_mutations
            .get(stripe)
            .ok_or("missing fixture mutation stripe")?;
        let busy = mutex
            .lock()
            .map_err(|error| format!("fixture generation mutation lock: {error}"))?;
        let mut session =
            adapter.begin_pair_measurement(&repo, &revision, RequestBudgetV1::unbounded(), None)?;
        let requested = BTreeSet::from([generation]);
        check!(matches!(
            session.measure(&requested),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
                ..
            })
        ));
        drop(busy);
        check!(session.measure(&requested)?.bytes > 0);
        check!(matches!(mutex.try_lock(), Err(TryLockError::WouldBlock)));
        drop(session);
        drop(
            mutex
                .try_lock()
                .map_err(|error| format!("fixture generation mutation retry: {error}"))?,
        );
        Ok(())
    }
}
