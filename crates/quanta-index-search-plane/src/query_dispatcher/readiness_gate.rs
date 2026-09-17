//! Ledger-backed readiness reads shared by every route: lexical materialization
//! snapshots, structural chunk authority, and semantic selection validation.

use std::sync::Arc;

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
};
use quanta_index_core::CoreError;

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::selection::SemanticSelection;
use crate::readiness::StructuralAuthorityState;

impl SearchPlaneDispatcher {
    pub(super) fn snapshot_lex_materialized(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Option<ManifestGeneration>, CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        Ok(guard.track_materialized(repo_id, revision_id, SearchPlaneTrackKind::Lexical))
    }

    /// The structural snapshot of the pinned generation, shared rather
    /// than copied (QI-BB-020).
    pub(super) fn snapshot_structural_state(
        &self,
        pin: &GenerationPin,
    ) -> Result<Arc<StructuralAuthorityState>, CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        guard
            .structural_snapshot(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
            .ok_or_else(|| {
                CoreError::NotReady(format!(
                    "structural: generation {} chunk authority is not materialized",
                    pin.manifest_generation.get()
                ))
            })
    }

    pub(super) fn validate_semantic_selection(
        &self,
        selection: &SemanticSelection,
        plane: &str,
    ) -> Result<(), CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        guard.validate_semantic_generation(
            &selection.pin.repo_id,
            &selection.pin.revision_id,
            selection.pin.manifest_generation,
            selection.expected_manifest_digest.as_deref(),
            true,
            plane,
        )
    }

    pub(super) fn validated_semantic_manifest_digest(
        &self,
        selection: &SemanticSelection,
        plane: &str,
    ) -> Result<String, CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        guard.validate_semantic_generation(
            &selection.pin.repo_id,
            &selection.pin.revision_id,
            selection.pin.manifest_generation,
            selection.expected_manifest_digest.as_deref(),
            true,
            plane,
        )?;
        guard
            .semantic_generation_state(
                &selection.pin.repo_id,
                &selection.pin.revision_id,
                selection.pin.manifest_generation,
            )
            .map(|state| state.manifest_digest().to_string())
            .ok_or_else(|| {
                CoreError::NotReady(format!(
                    "{plane}: semantic generation {} manifest authority disappeared after validation",
                    selection.pin.manifest_generation.get()
                ))
            })
    }
}
