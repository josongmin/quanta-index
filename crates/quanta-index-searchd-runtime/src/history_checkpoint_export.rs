//! Private offline History checkpoint exporter from the selected Legacy root.
//! Pages are provisional data. Installed BOOT independently authenticates
//! the source, terminal ACK and complete row set before selecting Native.

use std::{io, os::fd::AsFd as _};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use quanta_index_catalog::LegacyHistoryCheckpointPageV1;
use quanta_index_contract::{
    RepoId,
    index::{
        AuxiliaryGenerationKeyV1, Digest, HistorySnapshotPageV1,
        MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1, ObjectRef, native_immutable_object_ref_with_work_v1,
    },
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        IndexError, MemoryLifetime, WorkBudget, WorkUsage,
        history_checkpoint::{LegacyHistoryCheckpointPageRefV2, encode_large_row_v2},
    },
};
use quanta_index_searchd::cli::{NativeHistoryExportCommandV1, NativeHistoryExportScopeV1};
use serde::Serialize;
use serde::ser::SerializeStruct as _;

use super::{boot, with_native_recovery_control_v1};
use crate::deployment_selection;

#[path = "history_checkpoint_export/native_v1.rs"]
mod native_v1;
#[path = "history_checkpoint_export/source_v1.rs"]
mod source_v1;
#[path = "history_checkpoint_export/native_source_v1.rs"]
mod native_source_v1;

pub use source_v1::run_source_v1;

const MAX_FRAME_BYTES: usize = MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1 + 4096;

enum ExportFrame<'a> {
    NativeProtectedComplete {
        source_root_nonce: quanta_index_core::index::Digest,
        source_routes: quanta_index_core::index::ObjectRef,
        snapshot: Option<quanta_index_core::index::ObjectRef>,
    },
    NativeObjectBegin {
        reference: quanta_index_core::index::ObjectRef,
        payload_bytes: u64,
        children: &'a [quanta_index_core::index::ObjectRef],
    },
    NativeObjectChunk {
        offset: u64,
        bytes: &'a [u8],
    },
    NativeObjectEnd {
        reference: quanta_index_core::index::ObjectRef,
    },
    NativeHistory {
        source_root_nonce: Digest,
        source_view: quanta_index_core::index::ViewRef,
        instance: quanta_index_core::index::IndexInstanceId,
        member: quanta_index_core::index::ObjectRef,
        cut: &'a quanta_index_contract::index::HistorySourceCutV1,
    },
    NativeInventoryComplete {
        repo_id: &'a RepoId,
        count: u64,
    },
    CheckpointPage(LegacyHistoryCheckpointPageRefV2),
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

// Keep the adjacent-tag frame shape and field order stable: write_frame counts
// the JSON once before writing those exact bytes to the authenticated stream.
struct ExportBody<'frame, 'value>(&'frame ExportFrame<'value>);

macro_rules! serialize_body {
    ($serializer:expr, $name:literal, { $($field:ident),+ $(,)? }) => {{
        let mut body = $serializer.serialize_struct($name, [$(stringify!($field)),+].len())?;
        $(body.serialize_field(stringify!($field), $field)?;)+
        body.end()
    }};
}

impl Serialize for ExportBody<'_, '_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            ExportFrame::NativeProtectedComplete {
                source_root_nonce,
                source_routes,
                snapshot,
            } => serialize_body!(serializer, "NativeProtectedComplete", {
                source_root_nonce,
                source_routes,
                snapshot
            }),
            ExportFrame::NativeObjectBegin {
                reference,
                payload_bytes,
                children,
            } => serialize_body!(serializer, "NativeObjectBegin", {
                reference,
                payload_bytes,
                children
            }),
            ExportFrame::NativeObjectChunk { offset, bytes } => {
                serialize_body!(serializer, "NativeObjectChunk", { offset, bytes })
            }
            ExportFrame::NativeObjectEnd { reference } => {
                serialize_body!(serializer, "NativeObjectEnd", { reference })
            }
            ExportFrame::NativeHistory {
                source_root_nonce,
                source_view,
                instance,
                member,
                cut,
            } => serialize_body!(serializer, "NativeHistory", {
                source_root_nonce,
                source_view,
                instance,
                member,
                cut
            }),
            ExportFrame::NativeInventoryComplete { repo_id, count } => {
                serialize_body!(serializer, "NativeInventoryComplete", { repo_id, count })
            }
            ExportFrame::CheckpointPage(page) => page.serialize(serializer),
            ExportFrame::InventoryKey { generation } => {
                serialize_body!(serializer, "InventoryKey", { generation })
            }
            ExportFrame::InventoryComplete { repo_id, count } => {
                serialize_body!(serializer, "InventoryComplete", { repo_id, count })
            }
            ExportFrame::Complete {
                generation,
                legacy_root,
                durable_epoch,
                snapshot_digest,
                row_count,
                terminal_ack,
            } => serialize_body!(serializer, "Complete", {
                generation,
                legacy_root,
                durable_epoch,
                snapshot_digest,
                row_count,
                terminal_ack
            }),
        }
    }
}

impl Serialize for ExportFrame<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let kind = match self {
            ExportFrame::NativeProtectedComplete { .. } => "native_protected_complete",
            ExportFrame::NativeObjectBegin { .. } => "native_object_begin",
            ExportFrame::NativeObjectChunk { .. } => "native_object_chunk",
            ExportFrame::NativeObjectEnd { .. } => "native_object_end",
            ExportFrame::NativeHistory { .. } => "native_history",
            ExportFrame::NativeInventoryComplete { .. } => "native_inventory_complete",
            ExportFrame::CheckpointPage(..) => "checkpoint_page",
            ExportFrame::InventoryKey { .. } => "inventory_key",
            ExportFrame::InventoryComplete { .. } => "inventory_complete",
            ExportFrame::Complete { .. } => "complete",
        };
        let mut frame = serializer.serialize_struct("ExportFrame", 2)?;
        frame.serialize_field("kind", kind)?;
        frame.serialize_field("body", &ExportBody(self))?;
        frame.end()
    }
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

fn write_frame<T: Serialize>(
    output: &mut impl io::Write,
    frame: &T,
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

fn emit_v1_checkpoint_page(
    page: &HistorySnapshotPageV1,
    encoded_bytes: usize,
    output: &mut impl io::Write,
    budget: &mut WorkBudget,
) -> std::result::Result<ObjectRef, IndexError> {
    if encoded_bytes == 0 || encoded_bytes > MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1 {
        return Err(IndexError::ResourceLimited);
    }
    page.validate_shape()
        .map_err(|cause| IndexError::Storage(cause.into()))?;
    let _payload_memory = budget.reserve_memory(
        u64::try_from(encoded_bytes).map_err(|_| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(encoded_bytes)
        .map_err(|_| IndexError::ResourceLimited)?;
    if payload.capacity() > encoded_bytes {
        return Err(IndexError::ResourceLimited);
    }
    payload.resize(encoded_bytes, 0);
    let mut remaining = payload.as_mut_slice();
    serde_json::to_writer(&mut remaining, page)
        .map_err(|cause| IndexError::Storage(format!("History page encode: {cause}")))?;
    if !remaining.is_empty() {
        return Err(IndexError::DigestConflict);
    }
    budget.charge(WorkUsage {
        bytes: u64::try_from(payload.len()).map_err(|_| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let reference =
        native_immutable_object_ref_with_work_v1(&payload, &[], &mut || budget.checkpoint())?
            .ok_or(IndexError::DigestConflict)?;
    native_v1::write_native_object_v1(reference, &payload, &[], output, budget).map_err(
        |cause| {
            cause.downcast::<IndexError>().unwrap_or_else(|cause| {
                IndexError::Storage(format!("History page object stream: {cause}"))
            })
        },
    )?;
    Ok(reference)
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
            deployment_selection::with_locked_export_selection_v1(
                &command.selection_path,
                command.expected_current_sha256,
                budget,
                |selection, budget| {
                    if selection.native_command_v1().is_some() {
                        return native_v1::export_v1(
                            selection,
                            &command.scope,
                            &mut output,
                            budget,
                        );
                    }
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
                                            &mut |page: LegacyHistoryCheckpointPageV1<'_>, budget| {
                                                let (reference, ordinal, row_count) = match page {
                                                    LegacyHistoryCheckpointPageV1::V1 { page, encoded_bytes } => {
                                                        let reference = emit_v1_checkpoint_page(
                                                            page, encoded_bytes, &mut output, budget,
                                                        )?;
                                                        (reference, page.ordinal, page.rows.len() as u64)
                                                    }
                                                    LegacyHistoryCheckpointPageV1::LargeRow {
                                                        generation, ordinal, row,
                                                    } => {
                                                        let reference = encode_large_row_v2(
                                                            generation, ordinal, row, budget,
                                                            |reference, payload, children, budget| {
                                                                native_v1::write_native_object_v1(
                                                                    reference, payload, children,
                                                                    &mut output, budget,
                                                                ).map_err(|cause| {
                                                                    cause.downcast::<IndexError>()
                                                                        .unwrap_or_else(|cause| {
                                                                            IndexError::Storage(format!(
                                                                                "History large row object stream: {cause}"
                                                                            ))
                                                                        })
                                                                })
                                                            },
                                                        )?;
                                                        (reference, ordinal, 1)
                                                    }
                                                };
                                                page_count = page_count.checked_add(1).ok_or_else(|| {
                                                    IndexError::Storage("History page count overflow".into())
                                                })?;
                                                write_frame(
                                                    &mut output,
                                                    &ExportFrame::CheckpointPage(
                                                        LegacyHistoryCheckpointPageRefV2 {
                                                            reference, ordinal, row_count,
                                                        },
                                                    ),
                                                    budget,
                                                ).map_err(|cause| {
                                                    cause.downcast::<IndexError>().unwrap_or_else(|cause| {
                                                        IndexError::Storage(format!("History page marker stream: {cause}"))
                                                    })
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
                                NativeHistoryExportScopeV1::Protected => {
                                    bail!("protected Native export requires a Native selection");
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

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::{HistorySnapshotExportFrameV1, HistorySnapshotRowV1};
    use quanta_index_contract::{ManifestGeneration, RevisionId};
    use quanta_index_core::index::WorkLimits;

    #[test]
    fn v1_checkpoint_object_stream_keeps_canonical_bytes_and_reference()
    -> Result<(), Box<dyn std::error::Error>> {
        let page = HistorySnapshotPageV1 {
            wire_version: 1,
            generation: AuxiliaryGenerationKeyV1 {
                repo_id: RepoId::new("legacy")?,
                revision_id: RevisionId::new("main")?,
                generation: ManifestGeneration::new(1),
            },
            ordinal: 1,
            rows: vec![HistorySnapshotRowV1 {
                family: "epoch".into(),
                row_key: Vec::new(),
                value: vec![1, 2, 3],
            }],
        };
        let canonical = serde_json::to_vec(&page)?;
        let expected =
            quanta_index_contract::index::native_immutable_object_ref_v1(&canonical, &[])
                .ok_or("V1 page reference is invalid")?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 1_000_000_000,
                mutations: 1_000_000,
            },
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        );
        let mut stream = Vec::new();
        let actual = emit_v1_checkpoint_page(&page, canonical.len(), &mut stream, &mut budget)?;
        assert_eq!(actual, expected);
        let mut decoded = Vec::new();
        let mut frames = stream.as_slice();
        while !frames.is_empty() {
            let width = usize::try_from(u32::from_le_bytes(frames[..4].try_into()?))?;
            let frame: HistorySnapshotExportFrameV1 =
                serde_json::from_slice(&frames[4..4 + width])?;
            frames = &frames[4 + width..];
            match frame {
                HistorySnapshotExportFrameV1::NativeObjectBegin {
                    reference,
                    payload_bytes,
                    children,
                } => {
                    assert_eq!(reference, expected);
                    assert_eq!(payload_bytes, canonical.len() as u64);
                    assert!(children.is_empty());
                }
                HistorySnapshotExportFrameV1::NativeObjectChunk { offset, bytes } => {
                    assert_eq!(offset, decoded.len() as u64);
                    decoded.extend_from_slice(&bytes);
                }
                HistorySnapshotExportFrameV1::NativeObjectEnd { reference } => {
                    assert_eq!(reference, expected);
                }
                _ => return Err("V1 page stream emitted an unexpected frame".into()),
            }
        }
        assert_eq!(decoded, canonical);
        Ok(())
    }
}
