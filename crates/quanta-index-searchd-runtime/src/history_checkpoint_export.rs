//! Private offline History checkpoint exporter from the selected Legacy root.
//! Pages are provisional data. Installed BOOT independently authenticates
//! the source, terminal ACK and complete row set before selecting Native.

use std::{io, os::fd::AsFd as _};

use anyhow::{Context as _, Result, anyhow, ensure};
use quanta_index_contract::{
    RepoId,
    index::{
        AuxiliaryGenerationKeyV1, Digest, HistorySnapshotPageV1, MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1,
    },
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{IndexError, WorkBudget, WorkUsage},
};
use quanta_index_searchd::cli::{NativeHistoryExportCommandV1, NativeHistoryExportScopeV1};
use serde::Serialize;

use super::{boot, with_native_recovery_control_v1};
use crate::deployment_selection;

const MAX_FRAME_BYTES: usize = MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1 + 4096;

#[derive(Serialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
enum ExportFrame<'a> {
    Page(&'a HistorySnapshotPageV1),
    InventoryKey {
        generation: &'a AuxiliaryGenerationKeyV1,
    },
    InventoryComplete {
        repo_id: &'a RepoId,
        count: u64,
    },
    Complete {
        generation: &'a AuxiliaryGenerationKeyV1,
        legacy_root: &'a str,
        durable_epoch: quanta_index_contract::AuxEpochV1,
        snapshot_digest: Digest,
        row_count: u64,
        terminal_ack: &'a quanta_index_contract::BatchPublishReceipt,
    },
}

struct FrameCensus<'a> {
    bytes: usize,
    budget: &'a mut WorkBudget,
    denial: Option<IndexError>,
}

impl io::Write for FrameCensus<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|length| *length <= MAX_FRAME_BYTES)
            .ok_or_else(|| io::Error::other("History export frame exceeds bound"))?;
        if let Err(error) = self.budget.charge(WorkUsage {
            bytes: u64::try_from(bytes.len()).map_err(io::Error::other)?,
            ..WorkUsage::default()
        }) {
            self.denial = Some(error);
            return Err(io::Error::other("History export frame census refused"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn write_frame(
    output: &mut impl io::Write,
    frame: &ExportFrame<'_>,
    budget: &mut WorkBudget,
) -> Result<()> {
    let mut census = FrameCensus {
        bytes: 0,
        budget,
        denial: None,
    };
    let counted = serde_json::to_writer(&mut census, frame);
    if let Some(error) = census.denial {
        return Err(error.into());
    }
    counted?;
    let length = u32::try_from(census.bytes)?;
    budget.charge(WorkUsage {
        writes: 2,
        bytes: u64::from(length) + 4,
        ..WorkUsage::default()
    })?;
    output.write_all(&length.to_le_bytes())?;
    serde_json::to_writer(&mut *output, frame)?;
    output.flush()?;
    Ok(())
}

pub fn run(command: &NativeHistoryExportCommandV1, request: RequestBudgetV1) -> Result<()> {
    command.validate()?;
    ensure!(
        rustix::fs::FileType::from_raw_mode(rustix::fs::fstat(io::stdout().as_fd())?.st_mode)
            == rustix::fs::FileType::Fifo,
        "History exporter requires a private inherited stdout pipe"
    );
    let _inherited_umask = quanta_index_searchd::app::harden_umask();
    with_native_recovery_control_v1(
        &command.config_path,
        command.config_digest,
        request,
        |_context, budget| {
            let output = io::stdout();
            let mut output = output.lock();
            deployment_selection::with_locked_legacy_export_selection_v1(
                &command.selection_path,
                command.expected_current_sha256,
                budget,
                |selection, budget| {
                    boot::with_legacy_checkpoint_catalog_v1(
                        selection,
                        budget,
                        |legacy_root, catalog, budget| {
                            match &command.scope {
                                NativeHistoryExportScopeV1::Checkpoint(generation) => {
                                    let root = legacy_root
                                        .to_str()
                                        .ok_or_else(|| anyhow!("Legacy root is not UTF-8"))?;
                                    ensure!(root.len() <= 4096, "Legacy root exceeds export bound");
                                    let mut page_count = 0_u32;
                                    let observation =
                                        quanta_index_catalog::visit_legacy_history_checkpoint_pages_v1(
                                            legacy_root,
                                            catalog,
                                            generation,
                                            budget,
                                            &mut |page: &HistorySnapshotPageV1, budget| {
                                                page_count = page_count.checked_add(1).ok_or_else(|| {
                                                    quanta_index_core::CoreError::Storage(
                                                        "History page count overflow".into(),
                                                    )
                                                })?;
                                                write_frame(&mut output, &ExportFrame::Page(page), budget)
                                                    .map_err(|error| {
                                                        quanta_index_core::CoreError::Storage(format!(
                                                            "History page stream: {error}"
                                                        ))
                                                    })
                                            },
                                        )?;
                                    ensure!(page_count > 0, "History export contains no pages");
                                    write_frame(
                                        &mut output,
                                        &ExportFrame::Complete {
                                            generation,
                                            legacy_root: root,
                                            durable_epoch: observation.durable_epoch,
                                            snapshot_digest: observation.snapshot_digest,
                                            row_count: observation.row_count,
                                            terminal_ack: &observation.terminal_receipt,
                                        },
                                        budget,
                                    )
                                    .context("History completion frame")?;
                                }
                                NativeHistoryExportScopeV1::Inventory(repo_id) => {
                                    let count = quanta_index_catalog::visit_legacy_history_generation_keys_v1(
                                        legacy_root,
                                        catalog,
                                        repo_id,
                                        budget,
                                        &mut |generation, budget| {
                                            write_frame(
                                                &mut output,
                                                &ExportFrame::InventoryKey { generation },
                                                budget,
                                            )
                                            .map_err(|error| {
                                                error.downcast::<IndexError>().unwrap_or_else(|error| {
                                                    IndexError::Storage(format!("History inventory frame: {error}"))
                                                })
                                            })
                                        },
                                    )?;
                                    write_frame(
                                        &mut output,
                                        &ExportFrame::InventoryComplete { repo_id, count },
                                        budget,
                                    )
                                    .context("History inventory completion frame")?;
                                }
                            }
                            Ok(())
                        },
                    )
                },
            )
        },
    )
}
