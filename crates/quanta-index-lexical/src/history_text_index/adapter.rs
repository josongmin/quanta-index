//! The [`HistoryTextIndexPort`] implementation rooted at one directory.

use std::path::PathBuf;

use quanta_index_contract::{AuxEpochV1, ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, HISTORY_TEXT_INDEX_NORMALIZER_UNSUPPORTED_CODE,
    HISTORY_TEXT_INDEX_NOT_READY_CODE, HistoryTextBuildV1, HistoryTextDiscardOutcomeV1,
    HistoryTextEpochReceiptV1, HistoryTextEpochStatusV1, HistoryTextIndexPort, HistoryTextSearcher,
    LEXICAL_WRITER_HEAP_BYTES_MIN,
};

use crate::history_text_index::layout::{
    epoch_dir, fsync_parent, generation_dir, list_epochs, list_generations, tree_bytes,
};
use crate::history_text_index::manifest;
use crate::history_text_index::publish;
use crate::history_text_index::searcher::EpochSearcher;

/// The per-epoch history text index of every generation under one root
/// (`state_root/authorities/history-text/` in the daemon).
pub struct HistoryTextIndexAdapter {
    root: PathBuf,
    writer_heap_bytes: usize,
}

impl HistoryTextIndexAdapter {
    /// An adapter rooted at `root`, created lazily as epochs are
    /// published.
    ///
    /// One epoch is built by one writer thread under the engine's minimum
    /// heap: a history batch is small next to a corpus batch and the
    /// build is serialized by the auxiliary mutation coordinator anyway.
    pub fn with_root(root: PathBuf) -> Result<Self, CoreError> {
        let writer_heap_bytes = usize::try_from(LEXICAL_WRITER_HEAP_BYTES_MIN).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: writer heap {LEXICAL_WRITER_HEAP_BYTES_MIN} does not fit usize: {err}"
            ))
        })?;
        Ok(Self {
            root,
            writer_heap_bytes,
        })
    }

    fn discard_tree(path: &std::path::Path) -> Result<HistoryTextDiscardOutcomeV1, CoreError> {
        if !path.exists() {
            return Ok(HistoryTextDiscardOutcomeV1::Absent);
        }
        let bytes = tree_bytes(path)?;
        std::fs::remove_dir_all(path).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: remove {}: {err}",
                path.display()
            ))
        })?;
        fsync_parent(path)?;
        Ok(HistoryTextDiscardOutcomeV1::Discarded { bytes })
    }
}

impl HistoryTextIndexPort for HistoryTextIndexAdapter {
    fn epoch_status(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<HistoryTextEpochStatusV1, CoreError> {
        manifest::epoch_status(&epoch_dir(&self.root, generation, epoch), epoch)
    }

    fn publish_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
        build: HistoryTextBuildV1,
    ) -> Result<HistoryTextEpochReceiptV1, CoreError> {
        publish::publish_epoch(&self.root, generation, epoch, build, self.writer_heap_bytes)
    }

    fn open_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<Box<dyn HistoryTextSearcher>, CoreError> {
        let dir = epoch_dir(&self.root, generation, epoch);
        match manifest::epoch_status(&dir, epoch)? {
            HistoryTextEpochStatusV1::Servable => {}
            HistoryTextEpochStatusV1::Absent => {
                return Err(CoreError::Typed {
                    code: HISTORY_TEXT_INDEX_NOT_READY_CODE.to_string(),
                    message: format!(
                        "history text index: generation {} of {}@{} has no text index at epoch {epoch}; the next history batch builds it",
                        generation.generation.get(),
                        generation.repo_id.as_str(),
                        generation.revision_id.as_str()
                    ),
                });
            }
            HistoryTextEpochStatusV1::Unsupported { built_with } => {
                return Err(CoreError::Typed {
                    code: HISTORY_TEXT_INDEX_NORMALIZER_UNSUPPORTED_CODE.to_string(),
                    message: format!(
                        "history text index: epoch {epoch} was built under text normalizer {built_with}; it is never served with mismatched text semantics and the next history batch rebuilds it"
                    ),
                });
            }
        }
        Ok(Box::new(EpochSearcher::open(&dir)?))
    }

    fn durable_epochs(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
    ) -> Result<Vec<AuxEpochV1>, CoreError> {
        list_epochs(&self.root, generation)
    }

    fn durable_generations(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<ManifestGeneration>, CoreError> {
        list_generations(&self.root, repo_id, revision_id)
    }

    fn discard_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<HistoryTextDiscardOutcomeV1, CoreError> {
        Self::discard_tree(&epoch_dir(&self.root, generation, epoch))
    }

    fn discard_generation(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
    ) -> Result<HistoryTextDiscardOutcomeV1, CoreError> {
        Self::discard_tree(&generation_dir(&self.root, generation))
    }
}
