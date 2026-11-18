//! Selected Native History export. The old root lease remains held through
//! the final immutable frame and the deployment selection recheck.

use super::*;
use quanta_index_core::index::{MemoryLifetime, ObjectRef, StoredObject};

const CHUNK_BYTES: usize = 64 * 1024;

pub(super) fn export_v1(
    selection: &deployment_selection::DeploymentSelectionDocumentV1,
    scope: &NativeHistoryExportScopeV1,
    output: &mut impl io::Write,
    budget: &mut WorkBudget,
) -> Result<()> {
    boot::with_selected_native_history_v1(selection, budget, |source, budget| {
        if matches!(scope, NativeHistoryExportScopeV1::Protected) {
            return export_protected_v1(source, output, budget);
        }
        let repo = match scope {
            NativeHistoryExportScopeV1::Protected => {
                return Err(
                    IndexError::InvalidInput("protected scope was not handled".into()).into(),
                );
            }
            NativeHistoryExportScopeV1::Inventory(repo) => repo,
            NativeHistoryExportScopeV1::Checkpoint(generation) => &generation.repo_id,
        };
        let mut exported = 0_u64;
        let witnessed_history_count =
            source.visit_history_members_v1(Some(repo), budget, |input, reader, budget| {
                if let NativeHistoryExportScopeV1::Checkpoint(generation) = scope {
                    if &input.cut.generation != generation {
                        return Ok(());
                    }
                    ensure!(
                        exported == 0,
                        "Native History generation has multiple active members"
                    );
                    export_closure_v1(
                        input.member,
                        u32::try_from(
                            source
                                .selected_config_v1()
                                .resources
                                .physical
                                .inventory
                                .max_entries,
                        )
                        .map_err(|_| IndexError::ResourceLimited)?,
                        |reference, budget| reader.get(reference, budget),
                        output,
                        budget,
                    )?;
                }
                write_frame(
                    output,
                    &ExportFrame::NativeHistory {
                        source_root_nonce: input.source_root_nonce,
                        source_view: input.source_view,
                        instance: input.instance,
                        member: input.member,
                        cut: &input.cut,
                    },
                    budget,
                )?;
                exported = exported.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                Ok(())
            })?;
        ensure!(
            witnessed_history_count >= exported,
            "Native History export count exceeds selected inventory"
        );
        if matches!(scope, NativeHistoryExportScopeV1::Inventory(_)) {
            ensure!(
                witnessed_history_count == exported,
                "Native History inventory count differs"
            );
        }
        if matches!(scope, NativeHistoryExportScopeV1::Checkpoint(_)) {
            ensure!(
                exported == 1,
                "selected Native History generation is absent"
            );
        }
        write_frame(
            output,
            &ExportFrame::NativeInventoryComplete {
                repo_id: repo,
                count: exported,
            },
            budget,
        )
    })
}

fn export_protected_v1(
    source: &boot::HeldPreviousNativeV1,
    output: &mut impl io::Write,
    budget: &mut WorkBudget,
) -> Result<()> {
    if source
        .observed_v1()
        .snapshot
        .selected
        .as_ref()
        .and_then(|selected| selected.protected_source_routes)
        .is_none()
    {
        return write_frame(
            output,
            &ExportFrame::NativeProtectedComplete {
                source_root_nonce: quanta_index_core::index::Digest(
                    source.observed_v1().snapshot.root_nonce,
                ),
                source_routes: ObjectRef(quanta_index_core::index::Digest([0; 32])),
                snapshot: None,
            },
            budget,
        );
    }
    source.with_protected_routes_v1(budget, |routes, reader, budget| {
        let selected = source
            .observed_v1()
            .snapshot
            .selected
            .as_ref()
            .ok_or(IndexError::SnapshotUnavailable)?;
        quanta_index_catalog::visit_native_protected_snapshot_rows_v1(
            source.selected_config_v1().state_root.as_path(),
            source.catalog_io_v1(),
            selected,
            routes,
            budget,
            |snapshot, budget| {
                let length = snapshot
                    .encoded_payload_len_v1()
                    .map_err(|message| IndexError::InvalidInput(message.into()))?;
                let memory = u64::try_from(length).map_err(|_| IndexError::ResourceLimited)?;
                let _snapshot_memory = budget
                    .reserve_memory(memory, MemoryLifetime::Temporary)?
                    .ok_or(IndexError::ResourceLimited)?;
                let payload = serde_json::to_vec(snapshot)
                    .map_err(|cause| IndexError::Storage(cause.to_string()))?;
                let reference = snapshot
                    .object_ref_v1()
                    .map_err(|message| IndexError::InvalidInput(message.into()))?;
                let max_pending = u32::try_from(
                    source
                        .selected_config_v1()
                        .resources
                        .physical
                        .inventory
                        .max_entries,
                )
                .map_err(|_| IndexError::ResourceLimited)?;
                for root in &snapshot.roots {
                    export_closure_v1(
                        *root,
                        max_pending,
                        |reference, budget| reader.get(reference, budget),
                        output,
                        budget,
                    )
                    .map_err(|cause| {
                        cause
                            .downcast::<IndexError>()
                            .unwrap_or_else(|cause| IndexError::Storage(cause.to_string()))
                    })?;
                }
                write_frame(
                    output,
                    &ExportFrame::NativeObjectBegin {
                        reference,
                        payload_bytes: memory,
                        children: &snapshot.roots,
                    },
                    budget,
                )
                .map_err(|cause| IndexError::Storage(cause.to_string()))?;
                for (index, bytes) in payload.chunks(CHUNK_BYTES).enumerate() {
                    write_frame(
                        output,
                        &ExportFrame::NativeObjectChunk {
                            offset: u64::try_from(
                                index
                                    .checked_mul(CHUNK_BYTES)
                                    .ok_or(IndexError::ResourceLimited)?,
                            )
                            .map_err(|_| IndexError::ResourceLimited)?,
                            bytes,
                        },
                        budget,
                    )
                    .map_err(|cause| IndexError::Storage(cause.to_string()))?;
                }
                write_frame(output, &ExportFrame::NativeObjectEnd { reference }, budget)
                    .map_err(|cause| IndexError::Storage(cause.to_string()))?;
                write_frame(
                    output,
                    &ExportFrame::NativeProtectedComplete {
                        source_root_nonce: snapshot.source_root_nonce,
                        source_routes: snapshot.source_routes,
                        snapshot: Some(reference),
                    },
                    budget,
                )
                .map_err(|cause| IndexError::Storage(cause.to_string()))?;
                Ok(())
            },
        )?;
        Ok(())
    })?;

    Ok(())
}

/// Child-before-parent transfer preserves the canonical ordered child vector.
/// Shared subgraphs may be revisited; the original request work bounds every
/// physical read. No corpus-sized visited set or recursive stack is allocated.
fn export_closure_v1(
    root: ObjectRef,
    max_pending: u32,
    mut get: impl FnMut(ObjectRef, &mut WorkBudget) -> std::result::Result<StoredObject, IndexError>,
    output: &mut impl io::Write,
    budget: &mut WorkBudget,
) -> Result<()> {
    let maximum = usize::try_from(max_pending)?;
    ensure!(maximum > 0, "Native History export frontier is empty");
    let bytes = u64::from(max_pending)
        .checked_mul(u64::try_from(std::mem::size_of::<(ObjectRef, bool)>())?)
        .ok_or(IndexError::ResourceLimited)?;
    let _frontier_memory = budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut pending = Vec::new();
    pending
        .try_reserve_exact(maximum)
        .map_err(|_| IndexError::ResourceLimited)?;
    pending.push((root, false));
    while let Some((reference, expanded)) = pending.pop() {
        let stored = get(reference, budget)?;
        if !expanded {
            let needed = pending
                .len()
                .checked_add(1)
                .and_then(|n| n.checked_add(stored.registration.children.len()))
                .ok_or(IndexError::ResourceLimited)?;
            if needed > maximum {
                return Err(IndexError::ResourceLimited.into());
            }
            pending.push((reference, true));
            pending.extend(
                stored
                    .registration
                    .children
                    .iter()
                    .rev()
                    .map(|child| (*child, false)),
            );
            continue;
        }
        write_frame(
            output,
            &ExportFrame::NativeObjectBegin {
                reference,
                payload_bytes: u64::try_from(stored.payload.len())?,
                children: &stored.registration.children,
            },
            budget,
        )?;
        for (ordinal, chunk) in stored.payload.chunks(CHUNK_BYTES).enumerate() {
            let offset = ordinal
                .checked_mul(CHUNK_BYTES)
                .ok_or(IndexError::ResourceLimited)?;
            write_frame(
                output,
                &ExportFrame::NativeObjectChunk {
                    offset: u64::try_from(offset)?,
                    bytes: chunk,
                },
                budget,
            )?;
        }
        write_frame(output, &ExportFrame::NativeObjectEnd { reference }, budget)?;
    }
    budget.checkpoint()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::HistorySnapshotExportFrameV1 as Frame;
    use quanta_index_core::index::{
        MemoryLease, MemoryLeaseOwner, MemoryScope, MemoryScopePort, RegisteredObject, WorkLimits,
    };
    use std::{collections::BTreeMap, sync::Arc, time::Duration};

    struct Memory;
    impl MemoryLeaseOwner for Memory {
        fn release(&self, _: u64) {}
    }
    impl MemoryScopePort for Memory {
        fn reserve(
            &self,
            bytes: u64,
            _: MemoryLifetime,
        ) -> std::result::Result<MemoryLease, IndexError> {
            Ok(MemoryLease::new(bytes, Arc::new(Self)))
        }
    }
    fn work() -> WorkBudget {
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 10_000_000,
                mutations: 100,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Memory))));
        work
    }
    fn object(payload: Vec<u8>, children: Vec<ObjectRef>) -> Result<StoredObject> {
        let reference =
            quanta_index_contract::index::native_immutable_object_ref_v1(&payload, &children)
                .ok_or_else(|| anyhow!("test object identity refused"))?;
        Ok(StoredObject::new(
            RegisteredObject::unscoped(reference, payload.len() as u64, children),
            payload,
            None,
        ))
    }
    fn frames(mut bytes: &[u8]) -> Result<Vec<Frame>> {
        let mut frames = Vec::new();
        while !bytes.is_empty() {
            let length = u32::from_le_bytes(
                bytes
                    .get(..4)
                    .ok_or_else(|| anyhow!("missing length"))?
                    .try_into()?,
            ) as usize;
            let body = bytes
                .get(4..4 + length)
                .ok_or_else(|| anyhow!("missing frame"))?;
            frames.push(serde_json::from_slice(body)?);
            bytes = &bytes[4 + length..];
        }
        Ok(frames)
    }
    #[test]
    fn native_export_is_postorder_and_preserves_repeated_edges_and_fragment_offsets() -> Result<()>
    {
        let first = object(vec![11], vec![])?;
        let second = object(vec![22], vec![])?;
        let edges = vec![
            second.registration.reference,
            first.registration.reference,
            second.registration.reference,
        ];
        let root = object(vec![7; CHUNK_BYTES + 7], edges.clone())?;
        let root_ref = root.registration.reference;
        let objects: BTreeMap<_, _> = [first.clone(), second.clone(), root]
            .into_iter()
            .map(|object| (object.registration.reference, object))
            .collect();
        let mut output = Vec::new();
        export_closure_v1(
            root_ref,
            8,
            |reference, _| {
                objects
                    .get(&reference)
                    .cloned()
                    .ok_or(IndexError::SnapshotUnavailable)
            },
            &mut output,
            &mut work(),
        )?;
        let frames = frames(&output)?;
        let completed: Vec<_> = frames
            .iter()
            .filter_map(|frame| match frame {
                Frame::NativeObjectEnd { reference } => Some(*reference),
                _ => None,
            })
            .collect();
        ensure!(
            completed
                == vec![
                    second.registration.reference,
                    first.registration.reference,
                    second.registration.reference,
                    root_ref
                ]
        );
        let root_start = frames.iter().position(|frame| matches!(frame,
            Frame::NativeObjectBegin { reference, children, .. } if *reference == root_ref && *children == edges))
            .ok_or_else(|| anyhow!("root ordered children lost"))?;
        ensure!(
            matches!(&frames[root_start + 1], Frame::NativeObjectChunk { offset: 0, bytes } if bytes.len() == CHUNK_BYTES)
        );
        ensure!(
            matches!(&frames[root_start + 2], Frame::NativeObjectChunk { offset, bytes } if *offset == CHUNK_BYTES as u64 && bytes == &[7; 7])
        );
        Ok(())
    }

    #[test]
    fn native_export_refuses_missing_child_and_exhausted_frontier() -> Result<()> {
        let missing = ObjectRef(Digest([9; 32]));
        let root = object(vec![1], vec![missing])?;
        let mut output = Vec::new();
        let failure = export_closure_v1(
            root.registration.reference,
            2,
            |reference, _| {
                if reference == root.registration.reference {
                    Ok(root.clone())
                } else {
                    Err(IndexError::SnapshotUnavailable)
                }
            },
            &mut output,
            &mut work(),
        );
        ensure!(failure.is_err() && output.is_empty());
        let failure = export_closure_v1(
            root.registration.reference,
            1,
            |_, _| Ok(root.clone()),
            &mut output,
            &mut work(),
        );
        ensure!(failure.is_err() && output.is_empty());
        Ok(())
    }
}
