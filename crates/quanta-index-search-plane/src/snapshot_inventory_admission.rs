//! Reconcile cached query admission with durable and physical seal identities.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind};
use quanta_index_core::{CoreError, SealedGenerationIdentityProbePort};

use crate::{Ledger, SnapshotRegistries, SnapshotRegistry};

/// The inventory check is deliberately outside a query's hot path. When an
/// identity disappears, the resident handle is evicted and an opening flight
/// is fenced; its next acquire must pass the adapter's physical door again.
pub struct SnapshotInventoryAdmission {
    ledger: Arc<RwLock<Ledger>>,
    lexical_probe: Arc<dyn SealedGenerationIdentityProbePort>,
    semantic_probe: Arc<dyn SealedGenerationIdentityProbePort>,
    snapshots: SnapshotRegistries,
}

impl SnapshotInventoryAdmission {
    #[must_use]
    pub fn new(
        ledger: Arc<RwLock<Ledger>>,
        lexical_probe: Arc<dyn SealedGenerationIdentityProbePort>,
        semantic_probe: Arc<dyn SealedGenerationIdentityProbePort>,
        snapshots: SnapshotRegistries,
    ) -> Self {
        Self {
            ledger,
            lexical_probe,
            semantic_probe,
            snapshots,
        }
    }

    /// Revalidate only keys that could otherwise bypass the adapter door.
    /// A probe or ledger failure invalidates affected keys for its track,
    /// then reports the failure. Neither failure is promoted to a clean seal.
    pub fn reconcile(&self) -> Result<u64, CoreError> {
        let lexical = self.reconcile_track(
            SearchPlaneTrackKind::Lexical,
            self.lexical_probe.as_ref(),
            &self.snapshots.lexical,
        );
        let semantic = self.reconcile_track(
            SearchPlaneTrackKind::Semantic,
            self.semantic_probe.as_ref(),
            &self.snapshots.semantic,
        );
        match (lexical, semantic) {
            (Ok(left), Ok(right)) => Ok(left.saturating_add(right)),
            (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
            (Err(left), Err(right)) => Err(CoreError::Storage(format!(
                "snapshot inventory admission failed for both tracks: lexical={left}; semantic={right}"
            ))),
        }
    }

    fn reconcile_track<H: ?Sized + Send + Sync + 'static>(
        &self,
        track: SearchPlaneTrackKind,
        probe: &dyn SealedGenerationIdentityProbePort,
        registry: &SnapshotRegistry<H>,
    ) -> Result<u64, CoreError> {
        let keys = registry.revalidation_keys()?;
        if keys.is_empty() {
            return Ok(0);
        }

        let expected = self.ledger.read().map(|ledger| {
            keys.iter()
                .map(|key| {
                    (
                        key.clone(),
                        ledger.sealed_track_identity_digest(
                            &key.repo_id,
                            &key.revision_id,
                            track,
                            key.generation,
                        ),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        });
        let mut invalidated = 0_u64;
        let mut first_probe_error = None;
        for key in &keys {
            let recorded = expected
                .as_ref()
                .ok()
                .and_then(|expected| expected.get(key))
                .and_then(Option::as_deref);
            let healthy = if let Some(digest) = recorded {
                let candidate = GenerationSnapshot {
                    repo_id: key.repo_id.clone(),
                    revision_id: key.revision_id.clone(),
                    track,
                    manifest_generation: key.generation,
                    manifest_digest: digest.to_string(),
                };
                match probe.inventory_sealed_generation_identity(&candidate) {
                    Ok(matches) => matches,
                    Err(error) => {
                        if first_probe_error.is_none() {
                            first_probe_error = Some(error);
                        }
                        false
                    }
                }
            } else {
                false
            };
            if !healthy && registry.invalidate_for_revalidation(key)? {
                invalidated = invalidated.saturating_add(1);
            }
        }
        if let Some(error) = first_probe_error {
            return Err(CoreError::Storage(format!(
                "snapshot inventory admission {track:?} point probe failed after invalidating {invalidated} keys: {error}"
            )));
        }
        if let Err(error) = expected {
            return Err(CoreError::Storage(format!(
                "snapshot inventory admission {track:?} ledger unavailable after invalidating {invalidated} keys: {error}"
            )));
        }
        Ok(invalidated)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use quanta_index_core::LexicalSearcher;

    use super::*;
    use crate::query_dispatcher::tests::support::lexical::StubLexicalSearcher;
    use crate::{OpenedSnapshot, SnapshotKey, SnapshotRegistryPolicy};

    struct PointProbe {
        healthy: bool,
        calls: AtomicUsize,
    }

    impl SealedGenerationIdentityProbePort for PointProbe {
        fn probe_sealed_generation_identity(
            &self,
            _candidate: &GenerationSnapshot,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        fn inventory_sealed_generation_identity(
            &self,
            candidate: &GenerationSnapshot,
        ) -> Result<bool, CoreError> {
            if candidate.manifest_digest != "manifest-digest-9"
                || candidate.manifest_generation
                    != quanta_index_contract::ManifestGeneration::new(9)
            {
                return Err(CoreError::InvalidContract(
                    "point probe received the wrong ledger identity".into(),
                ));
            }
            let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.healthy)
        }
    }

    #[test]
    fn only_a_cached_key_is_probed_and_a_missing_identity_is_evicted() -> Result<(), CoreError> {
        let snapshots = SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT);
        let repo = quanta_index_contract::RepoId::new("repo")
            .expect("static fixture repo satisfies canonical policy");
        let revision = quanta_index_contract::RevisionId::new("rev")
            .expect("static fixture revision satisfies canonical policy");
        let generation = quanta_index_contract::ManifestGeneration::new(9);
        let key = SnapshotKey::new(&repo, &revision, generation);
        let mut ledger = Ledger::default();
        ledger.record_historically_sealed_search_corpus(
            &repo,
            &revision,
            generation,
            "manifest-digest-9",
        );
        let handle: Arc<dyn LexicalSearcher> = Arc::new(StubLexicalSearcher::default());
        let opened = OpenedSnapshot {
            handle,
            resident_bytes: 1,
        };
        let _retained = snapshots.lexical.begin_promotion(&key)?.promote(&opened)?;
        let lexical = Arc::new(PointProbe {
            healthy: false,
            calls: AtomicUsize::new(0),
        });
        let semantic = Arc::new(PointProbe {
            healthy: true,
            calls: AtomicUsize::new(0),
        });
        let admission = SnapshotInventoryAdmission::new(
            Arc::new(RwLock::new(ledger)),
            Arc::clone(&lexical) as Arc<dyn SealedGenerationIdentityProbePort>,
            Arc::clone(&semantic) as Arc<dyn SealedGenerationIdentityProbePort>,
            snapshots.clone(),
        );
        assert_eq!(admission.reconcile()?, 1);
        assert_eq!(lexical.calls.load(Ordering::SeqCst), 1);
        assert_eq!(semantic.calls.load(Ordering::SeqCst), 0);
        assert_eq!(snapshots.lexical.stats()?.entries, 0);
        assert_eq!(admission.reconcile()?, 0);
        assert_eq!(lexical.calls.load(Ordering::SeqCst), 1);
        Ok(())
    }
}
