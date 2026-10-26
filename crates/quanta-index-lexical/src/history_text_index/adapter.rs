//! The [`HistoryTextIndexPort`] implementation rooted at one directory.

use std::path::PathBuf;
use std::sync::Arc;

use quanta_index_contract::{AuxEpochV1, ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::HistoryTextFileAdmissionFactory;
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, DiagnosticContextV1, DiagnosticStageV1,
    HistoryTextBuildV1, HistoryTextDiscardOutcomeV1, HistoryTextEpochReceiptV1,
    HistoryTextEpochStatusV1, HistoryTextIndexPort, HistoryTextSearcher,
    LEXICAL_WRITER_HEAP_BYTES_MIN, RequestBudgetV1,
};

use crate::history_text_index::layout::{
    epoch_dir, fsync_parent_observed, generation_dir, list_epochs, list_epochs_admitted,
    list_generations, list_generations_admitted, tree_bytes,
};
use crate::history_text_index::manifest;
use crate::history_text_index::publish;
use crate::history_text_index::searcher::EpochSearcher;

/// The per-epoch history text index of every generation under one root
/// (`state_root/authorities/history-text/` in the daemon).
pub struct HistoryTextIndexAdapter {
    root: PathBuf,
    writer_heap_bytes: usize,
    physical: Option<Arc<dyn HistoryTextFileAdmissionFactory>>,
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
            physical: None,
        })
    }

    /// Install the canonical physical authority before any history operation.
    pub fn with_file_admission(
        root: PathBuf,
        physical: Arc<dyn HistoryTextFileAdmissionFactory>,
    ) -> Result<Self, CoreError> {
        let read = physical.for_read()?;
        read.validate_root(&root)?;
        let mut adapter = Self::with_root(root)?;
        adapter.physical = Some(physical);
        Ok(adapter)
    }

    fn discard_tree(
        path: &std::path::Path,
        sync: Option<&crate::causal_profile::SyncTotalsV1>,
    ) -> Result<HistoryTextDiscardOutcomeV1, CoreError> {
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
        fsync_parent_observed(path, sync)?;
        Ok(HistoryTextDiscardOutcomeV1::Discarded { bytes })
    }
}

impl HistoryTextIndexPort for HistoryTextIndexAdapter {
    fn epoch_status(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<HistoryTextEpochStatusV1, CoreError> {
        let dir = epoch_dir(&self.root, generation, epoch);
        if let Some(physical) = &self.physical {
            let io = physical.for_read()?;
            io.validate_root(&self.root)?;
            manifest::epoch_status_admitted(&dir, epoch, io.as_ref())
        } else {
            manifest::epoch_status(&dir, epoch)
        }
    }

    fn publish_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
        build: HistoryTextBuildV1,
        budget: &RequestBudgetV1,
    ) -> Result<HistoryTextEpochReceiptV1, CoreError> {
        crate::causal_profile::observe_result_with_sync(
            budget.diagnostic_context(),
            DiagnosticStageV1::Finalize,
            |_diagnostic, sync| {
                if let Some(physical) = &self.physical {
                    let retirement = physical.for_retirement()?;
                    if let Some(receipt) = publish::recover_epoch_admitted(
                        &self.root,
                        generation,
                        epoch,
                        &build,
                        retirement.as_ref(),
                    )? {
                        return Ok(receipt);
                    }
                    budget.checkpoint("history text: before publication reserve")?;
                    let io = physical.for_publication(budget)?;
                    publish::publish_epoch_admitted(
                        &self.root,
                        generation,
                        epoch,
                        build,
                        self.writer_heap_bytes,
                        &io,
                        budget,
                    )
                } else {
                    publish::publish_epoch(
                        &self.root,
                        generation,
                        epoch,
                        build,
                        self.writer_heap_bytes,
                        sync.as_deref(),
                        budget,
                    )
                }
            },
        )
    }

    fn open_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<Box<dyn HistoryTextSearcher>, CoreError> {
        let dir = epoch_dir(&self.root, generation, epoch);
        let admitted = self
            .physical
            .as_ref()
            .map(|physical| physical.for_read())
            .transpose()?;
        let status = if let Some(io) = &admitted {
            io.validate_root(&self.root)?;
            manifest::epoch_status_admitted(&dir, epoch, io.as_ref())?
        } else {
            manifest::epoch_status(&dir, epoch)?
        };
        match status {
            HistoryTextEpochStatusV1::Servable => {}
            HistoryTextEpochStatusV1::Absent => {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexNotReady,
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
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexNormalizerUnsupported,
                    message: format!(
                        "history text index: epoch {epoch} was built under text normalizer {built_with}; it is never served with mismatched text semantics and the next history batch rebuilds it"
                    ),
                });
            }
        }
        Ok(Box::new(if let Some(io) = admitted {
            EpochSearcher::open_admitted(&dir, &io)?
        } else {
            EpochSearcher::open(&dir)?
        }))
    }

    fn durable_epochs(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
    ) -> Result<Vec<AuxEpochV1>, CoreError> {
        if let Some(physical) = &self.physical {
            let io = physical.for_read()?;
            io.validate_root(&self.root)?;
            list_epochs_admitted(&self.root, generation, io.as_ref())
        } else {
            list_epochs(&self.root, generation)
        }
    }

    fn durable_generations(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<ManifestGeneration>, CoreError> {
        if let Some(physical) = &self.physical {
            let io = physical.for_read()?;
            io.validate_root(&self.root)?;
            list_generations_admitted(&self.root, repo_id, revision_id, io.as_ref())
        } else {
            list_generations(&self.root, repo_id, revision_id)
        }
    }

    fn discard_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<HistoryTextDiscardOutcomeV1, CoreError> {
        crate::causal_profile::observe_result_with_sync(
            diagnostic,
            DiagnosticStageV1::Finalize,
            |_diagnostic, sync| {
                if let Some(physical) = &self.physical {
                    let io = physical.for_retirement()?;
                    io.validate_root(&self.root)?;
                    let path = epoch_dir(&self.root, generation, epoch);
                    let removed = io
                        .recover_tree_delete(&path)?
                        .map_or_else(|| io.remove_tree(&path), |bytes| Ok(Some(bytes)))?;
                    removed.map_or(Ok(HistoryTextDiscardOutcomeV1::Absent), |bytes| {
                        Ok(HistoryTextDiscardOutcomeV1::Discarded { bytes })
                    })
                } else {
                    Self::discard_tree(&epoch_dir(&self.root, generation, epoch), sync.as_deref())
                }
            },
        )
    }

    fn discard_generation(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<HistoryTextDiscardOutcomeV1, CoreError> {
        crate::causal_profile::observe_result_with_sync(
            diagnostic,
            DiagnosticStageV1::Finalize,
            |_diagnostic, sync| {
                if let Some(physical) = &self.physical {
                    let io = physical.for_retirement()?;
                    io.validate_root(&self.root)?;
                    let path = generation_dir(&self.root, generation);
                    let removed = io
                        .recover_tree_delete(&path)?
                        .map_or_else(|| io.remove_tree(&path), |bytes| Ok(Some(bytes)))?;
                    removed.map_or(Ok(HistoryTextDiscardOutcomeV1::Absent), |bytes| {
                        Ok(HistoryTextDiscardOutcomeV1::Discarded { bytes })
                    })
                } else {
                    Self::discard_tree(&generation_dir(&self.root, generation), sync.as_deref())
                }
            },
        )
    }
}
