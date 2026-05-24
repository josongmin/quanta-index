//! `GenerationPinPort` impl backed by the control-plane `SQLite` snapshot.
//!
//! Per design decision D11, the pin is **per-query**: at the moment a query
//! starts, `ControlPlane::pin_generation(repo, rev)` snapshots the currently
//! active generation for that (repo, rev) tuple and returns a
//! [`BoundGenerationPin`] that owns the snapshot. The pin holds no live
//! reference to the database, so a concurrent `activate_generation` /
//! `mark_active_generation` from another writer cannot retroactively switch
//! the in-flight query's view of the active generation (U-SP4).

use quanta_index_contract::{
    GenerationId, ManifestGeneration, PublishedGenerationSet, RepoId, RevisionId,
};
use quanta_index_core::{CoreError, GenerationPinPort};
use rusqlite::{OptionalExtension, params};

use super::ControlPlane;

/// Owned snapshot of the active generation for a (repo, rev) at pin time.
///
/// Implements [`GenerationPinPort`]. The snapshot is immutable once captured;
/// callers re-pin to observe activations that landed after construction.
#[derive(Clone, Debug)]
pub struct BoundGenerationPin {
    snapshot: Option<PublishedGenerationSet>,
}

impl BoundGenerationPin {
    /// Construct a pin holding the supplied snapshot. Primarily for tests;
    /// production code uses [`ControlPlane::pin_generation`].
    #[must_use]
    pub fn from_snapshot(snapshot: Option<PublishedGenerationSet>) -> Self {
        Self { snapshot }
    }
}

impl GenerationPinPort for BoundGenerationPin {
    fn pinned_generation(&self) -> Result<Option<PublishedGenerationSet>, CoreError> {
        Ok(self.snapshot.clone())
    }
}

impl ControlPlane {
    /// Snapshot the active generation for `(repo, rev)` into an owned
    /// [`BoundGenerationPin`].
    ///
    /// Returns a pin whose snapshot is `None` when no row exists in
    /// `generation_activation_state` for the requested tuple. Per D11 the
    /// snapshot is owned — subsequent activations against this DB do not
    /// mutate the returned pin.
    pub fn pin_generation(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<BoundGenerationPin, CoreError> {
        let snapshot = self
            .conn
            .query_row(
                "
                SELECT
                    g.repo_id,
                    g.revision_id,
                    g.manifest_generation,
                    g.lexical_generation,
                    g.symbol_generation,
                    g.structural_generation,
                    g.history_generation,
                    g.semantic_generation,
                    g.metadata_generation
                FROM generation_catalog g
                JOIN generation_activation_state a
                  ON a.repo_id = g.repo_id
                 AND a.revision_id = g.revision_id
                 AND a.active_manifest_generation = g.manifest_generation
                WHERE g.repo_id = ?1 AND g.revision_id = ?2
                ",
                params![repo_id.as_str(), revision_id.as_str()],
                row_to_generation_set,
            )
            .optional()
            .map_err(|error| CoreError::Storage(format!("pin_generation read failed: {error}")))?;
        Ok(BoundGenerationPin { snapshot })
    }
}

fn row_to_generation_set(row: &rusqlite::Row<'_>) -> rusqlite::Result<PublishedGenerationSet> {
    Ok(PublishedGenerationSet {
        repo_id: RepoId::new(row.get::<_, String>(0)?),
        revision_id: RevisionId::new(row.get::<_, String>(1)?),
        manifest_generation: ManifestGeneration::new(row.get::<_, u64>(2)?),
        lexical_generation: GenerationId::new(row.get::<_, u64>(3)?),
        symbol_generation: GenerationId::new(row.get::<_, u64>(4)?),
        structural_generation: row.get::<_, Option<u64>>(5)?.map(GenerationId::new),
        history_generation: row.get::<_, Option<u64>>(6)?.map(GenerationId::new),
        semantic_generation: row.get::<_, Option<u64>>(7)?.map(GenerationId::new),
        metadata_generation: row.get::<_, Option<u64>>(8)?.map(GenerationId::new),
    })
}
