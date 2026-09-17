//! The durable receipt of enforcing one repo/revision search-corpus history
//! window, applied to the ledger before a new seal is exposed.

use std::collections::BTreeSet;

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};

/// Durable result of enforcing one repo/revision history window.
///
/// Consumers must apply this receipt to the in-memory ledger before exposing a
/// newly sealed generation. This keeps same-process rollback authority aligned
/// with the durable retained window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusHistoryRetentionReceiptV1 {
    pub(super) repo_id: RepoId,
    pub(super) revision_id: RevisionId,
    pub(super) retained_generations: BTreeSet<ManifestGeneration>,
    pub(super) reaped_generations: BTreeSet<ManifestGeneration>,
    pub(super) store_reconciled_v1: bool,
}

impl SearchCorpusHistoryRetentionReceiptV1 {
    #[cfg(test)]
    pub(crate) fn retaining_generations_v1(
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generations: impl IntoIterator<Item = ManifestGeneration>,
    ) -> Self {
        Self {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            retained_generations: generations.into_iter().collect(),
            reaped_generations: BTreeSet::new(),
            store_reconciled_v1: true,
        }
    }

    #[must_use]
    pub fn repo_id(&self) -> &RepoId {
        &self.repo_id
    }

    #[must_use]
    pub fn revision_id(&self) -> &RevisionId {
        &self.revision_id
    }

    #[must_use]
    pub fn retains(&self, generation: ManifestGeneration) -> bool {
        self.retained_generations.contains(&generation)
    }

    #[must_use]
    pub fn reaped_generations(&self) -> &BTreeSet<ManifestGeneration> {
        &self.reaped_generations
    }
}
