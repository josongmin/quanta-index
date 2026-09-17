//! Ledger-backed readiness reads shared by every route: lexical materialization
//! snapshots, structural chunk authority, and semantic selection validation.

use std::time::Instant;

use quanta_index_contract::{
    AuxEpochV1, GenerationPin, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
};
use quanta_index_core::{CoreError, StructuralError};

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::selection::SemanticSelection;
use crate::readiness::{AuxRead, StructuralAuthorityState};

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

    /// The structural snapshot of the pinned generation and its epoch,
    /// shared rather than copied (QI-BB-020 W2): the current one when
    /// `epoch` is `None`, else exactly the named one — refused typed when
    /// it is no longer retained or never existed, never substituted.
    ///
    /// A generation with no structural authority at all is refused with
    /// the structural domain's own readiness code, the same one its
    /// producer reports.
    pub(super) fn structural_read(
        &self,
        pin: &GenerationPin,
        epoch: Option<AuxEpochV1>,
    ) -> Result<AuxRead<StructuralAuthorityState>, CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        guard
            .structural_read_at(
                &pin.repo_id,
                &pin.revision_id,
                pin.manifest_generation,
                epoch,
                Instant::now(),
            )?
            .ok_or_else(|| {
                let not_ready = StructuralError::GenerationNotReady;
                CoreError::Typed {
                    code: not_ready.code().to_string(),
                    message: format!(
                        "{not_ready}: generation {} has no structural authority to pin",
                        pin.manifest_generation.get()
                    ),
                }
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
