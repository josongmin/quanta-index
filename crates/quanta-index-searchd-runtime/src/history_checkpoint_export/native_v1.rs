//! Selected Native History export. The old root lease remains held through
//! the final immutable frame and the deployment selection recheck.

use super::*;
use quanta_index_contract::index::{
    NativeProtectedNativeSnapshotV1, NativeProtectedSnapshotPlanV1,
};
use quanta_index_core::index::{MemoryLease, MemoryLifetime, ObjectRef, StoredObject};

const CHUNK_BYTES: usize = 64 * 1024;

fn export_index_error(cause: anyhow::Error) -> IndexError {
    cause
        .downcast::<IndexError>()
        .unwrap_or_else(|cause| IndexError::Storage(cause.to_string()))
}

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
                let plan = snapshot
                    .physical_plan_with_work_v1(&mut || budget.checkpoint())
                    .map_err(|cause| {
                        cause.into_error(|message| IndexError::InvalidInput(message.into()))
                    })?;
                let length = plan.pages_v1().map(|(_, bytes)| bytes).sum::<usize>();
                budget.charge(WorkUsage {
                    bytes: u64::try_from(length).map_err(|_| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
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
                    .map_err(export_index_error)?;
                }
                write_protected_snapshot_v1(snapshot, &plan, output, budget)?;
                write_frame(
                    output,
                    &ExportFrame::NativeProtectedComplete {
                        source_root_nonce: snapshot.source_root_nonce,
                        source_routes: snapshot.source_routes,
                        snapshot: Some(plan.reference_v1()),
                    },
                    budget,
                )
                .map_err(export_index_error)?;
                Ok(())
            },
        )?;
        Ok(())
    })?;

    Ok(())
}

fn write_protected_snapshot_v1(
    snapshot: &NativeProtectedNativeSnapshotV1,
    plan: &NativeProtectedSnapshotPlanV1,
    output: &mut impl io::Write,
    budget: &mut WorkBudget,
) -> std::result::Result<(), IndexError> {
    let memory = budget
        .reserve_memory(CHUNK_BYTES as u64, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut chunk = Vec::new();
    chunk
        .try_reserve_exact(CHUNK_BYTES)
        .map_err(|_| IndexError::ResourceLimited)?;
    if chunk.capacity() > CHUNK_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let mut writer = SnapshotPageWriter {
        pages: plan.pages_v1(),
        roots: &snapshot.roots,
        page: None,
        chunk,
        output,
        budget,
        failure: None,
    };
    let encoded = serde_json::to_writer(&mut writer, snapshot);
    if let Some(error) = writer.failure.take() {
        return Err(error);
    }
    encoded.map_err(|error| IndexError::Storage(error.to_string()))?;
    if writer.page.is_some() || writer.pages.next().is_some() {
        return Err(IndexError::DigestConflict);
    }
    drop(writer);
    drop(memory);
    if let Some((reference, content, children)) = plan.manifest_v1() {
        write_native_object_v1(reference, content, children, output, budget)
            .map_err(export_index_error)?;
    }
    Ok(())
}

struct SnapshotPage {
    reference: ObjectRef,
    bytes: usize,
    offset: usize,
}

struct SnapshotPageWriter<'a, W, P> {
    pages: P,
    roots: &'a [ObjectRef],
    page: Option<SnapshotPage>,
    chunk: Vec<u8>,
    output: &'a mut W,
    budget: &'a mut WorkBudget,
    failure: Option<IndexError>,
}

impl<W: io::Write, P: Iterator<Item = (ObjectRef, usize)>> SnapshotPageWriter<'_, W, P> {
    fn push(&mut self, mut input: &[u8]) -> std::result::Result<(), IndexError> {
        while !input.is_empty() {
            if self.page.is_none() {
                let (reference, bytes) = self.pages.next().ok_or(IndexError::DigestConflict)?;
                if bytes == 0 {
                    return Err(IndexError::DigestConflict);
                }
                write_frame(
                    self.output,
                    &ExportFrame::NativeObjectBegin {
                        reference,
                        payload_bytes: u64::try_from(bytes)
                            .map_err(|_| IndexError::ResourceLimited)?,
                        children: std::mem::take(&mut self.roots),
                    },
                    self.budget,
                )
                .map_err(export_index_error)?;
                self.page = Some(SnapshotPage {
                    reference,
                    bytes,
                    offset: 0,
                });
            }
            let page = self.page.as_mut().ok_or(IndexError::DigestConflict)?;
            let width = (page.bytes - page.offset).min(CHUNK_BYTES);
            let (prefix, remaining) = input.split_at((width - self.chunk.len()).min(input.len()));
            self.chunk.extend_from_slice(prefix);
            input = remaining;
            if self.chunk.len() == width {
                write_frame(
                    self.output,
                    &ExportFrame::NativeObjectChunk {
                        offset: u64::try_from(page.offset)
                            .map_err(|_| IndexError::ResourceLimited)?,
                        bytes: &self.chunk,
                    },
                    self.budget,
                )
                .map_err(export_index_error)?;
                self.chunk.clear();
                page.offset += width;
                if page.offset == page.bytes {
                    write_frame(
                        self.output,
                        &ExportFrame::NativeObjectEnd {
                            reference: page.reference,
                        },
                        self.budget,
                    )
                    .map_err(export_index_error)?;
                    self.page = None;
                }
            }
        }
        Ok(())
    }
}

impl<W: io::Write, P: Iterator<Item = (ObjectRef, usize)>> io::Write
    for SnapshotPageWriter<'_, W, P>
{
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if let Err(error) = self.push(input) {
            self.failure = Some(error);
            return Err(io::Error::other("protected Native snapshot export refused"));
        }
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn write_native_object_v1(
    reference: ObjectRef,
    payload: &[u8],
    children: &[ObjectRef],
    output: &mut impl io::Write,
    budget: &mut WorkBudget,
) -> Result<()> {
    write_frame(
        output,
        &ExportFrame::NativeObjectBegin {
            reference,
            payload_bytes: u64::try_from(payload.len())?,
            children,
        },
        budget,
    )?;
    for (ordinal, chunk) in payload.chunks(CHUNK_BYTES).enumerate() {
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
    write_frame(output, &ExportFrame::NativeObjectEnd { reference }, budget)
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
    let mut frontier = ExportFrontier::default();
    frontier.reserve(1, maximum, budget)?;
    frontier.pending.push((root, false));
    while let Some((reference, expanded)) = frontier.pending.pop() {
        let stored = get(reference, budget)?;
        if !expanded {
            let additional = stored
                .registration
                .children
                .len()
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            frontier.reserve(additional, maximum, budget)?;
            frontier.pending.push((reference, true));
            frontier.pending.extend(
                stored
                    .registration
                    .children
                    .iter()
                    .rev()
                    .map(|child| (*child, false)),
            );
            continue;
        }
        write_native_object_v1(
            reference,
            &stored.payload,
            &stored.registration.children,
            output,
            budget,
        )?;
    }
    budget.checkpoint()?;
    Ok(())
}

#[derive(Default)]
struct ExportFrontier {
    pending: Vec<(ObjectRef, bool)>,
    // Drop the pending backing before refunding its loan.
    _memory: Option<MemoryLease>,
}

impl ExportFrontier {
    fn reserve(
        &mut self,
        additional: usize,
        maximum: usize,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        let needed = self
            .pending
            .len()
            .checked_add(additional)
            .filter(|needed| *needed <= maximum)
            .ok_or(IndexError::ResourceLimited)?;
        let current = self.pending.capacity();
        if needed <= current {
            return Ok(());
        }
        let capacity = current.saturating_mul(2).max(needed).min(maximum);
        let bytes = capacity
            .checked_mul(std::mem::size_of::<(ObjectRef, bool)>())
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget
            .reserve_memory(bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(capacity)
            .map_err(|_| IndexError::ResourceLimited)?;
        if pending.capacity() > capacity {
            return Err(IndexError::ResourceLimited);
        }
        pending.append(&mut self.pending);
        self.pending = pending;
        self._memory = Some(memory);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::HistorySnapshotExportFrameV1 as Frame;
    use quanta_index_core::index::{
        MemoryLease, MemoryLeaseOwner, MemoryScope, MemoryScopePort, RegisteredObject, WorkLimits,
    };
    use std::{
        collections::BTreeMap,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    #[derive(Clone, Default)]
    struct Memory(Arc<AtomicU64>, Option<u64>);
    impl MemoryLeaseOwner for Memory {
        fn release(&self, bytes: u64) {
            let previous = self.0.fetch_sub(bytes, Ordering::Relaxed);
            assert!(previous >= bytes);
        }
    }
    impl MemoryScopePort for Memory {
        fn reserve(
            &self,
            bytes: u64,
            _: MemoryLifetime,
        ) -> std::result::Result<MemoryLease, IndexError> {
            let _previous = self
                .0
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                    live.checked_add(bytes)
                        .filter(|next| self.1.is_none_or(|limit| *next <= limit))
                })
                .map_err(|_| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, Arc::new(self.clone())))
        }
    }
    fn work(memory: Memory) -> WorkBudget {
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 10_000_000,
                mutations: 100,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(memory))));
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
    fn protected_snapshot_stream_matches_materialized_frames_under_one_chunk_budget() -> Result<()>
    {
        let route = ObjectRef(Digest([11; 32]));
        let snapshot = NativeProtectedNativeSnapshotV1 {
            wire_version: NativeProtectedNativeSnapshotV1::WIRE_VERSION,
            source_root_nonce: Digest([10; 32]),
            source_routes: route,
            operation_rows: vec![vec![7; CHUNK_BYTES]],
            genesis_views: Vec::new(),
            frontiers: Vec::new(),
            source_authorities: Vec::new(),
            event_claims: Vec::new(),
            roots: vec![route],
        };
        let canonical = serde_json::to_vec(&snapshot)?;
        ensure!(canonical.len() > CHUNK_BYTES);
        let reference = quanta_index_contract::index::native_immutable_object_ref_v1(
            &canonical,
            &snapshot.roots,
        )
        .ok_or_else(|| anyhow!("snapshot object identity refused"))?;
        let mut expected = Vec::new();
        write_native_object_v1(
            reference,
            &canonical,
            &snapshot.roots,
            &mut expected,
            &mut work(Memory::default()),
        )?;

        let plan = snapshot.physical_plan_v1().map_err(anyhow::Error::msg)?;
        ensure!(plan.reference_v1() == reference);
        let memory = Memory(Arc::default(), Some(CHUNK_BYTES as u64));
        let mut output = Vec::new();
        write_protected_snapshot_v1(&snapshot, &plan, &mut output, &mut work(memory.clone()))?;
        ensure!(output == expected);
        ensure!(memory.0.load(Ordering::Relaxed) == 0);
        output.clear();
        let mut denied = work(memory.clone());
        denied.charge(WorkUsage {
            writes: 98,
            ..WorkUsage::default()
        })?;
        let refused = write_protected_snapshot_v1(&snapshot, &plan, &mut output, &mut denied);
        ensure!(refused == Err(IndexError::ResourceLimited));
        ensure!(matches!(
            frames(&output)?.as_slice(),
            [Frame::NativeObjectBegin { reference: observed, .. }] if *observed == reference
        ));
        ensure!(memory.0.load(Ordering::Relaxed) == 0);
        Ok(())
    }

    #[test]
    fn protected_snapshot_writer_splits_fragments_at_chunk_and_page_boundaries() -> Result<()> {
        let child = ObjectRef(Digest([20; 32]));
        let payload = vec![13; CHUNK_BYTES + 17];
        let (first, second) = payload.split_at(CHUNK_BYTES + 3);
        let first_ref =
            quanta_index_contract::index::native_immutable_object_ref_v1(first, &[child])
                .ok_or_else(|| anyhow!("snapshot first page identity refused"))?;
        let second_ref = quanta_index_contract::index::native_immutable_object_ref_v1(second, &[])
            .ok_or_else(|| anyhow!("snapshot second page identity refused"))?;
        let mut expected = Vec::new();
        let mut expected_budget = work(Memory::default());
        write_native_object_v1(
            first_ref,
            first,
            &[child],
            &mut expected,
            &mut expected_budget,
        )?;
        write_native_object_v1(second_ref, second, &[], &mut expected, &mut expected_budget)?;

        let mut output = Vec::new();
        let mut budget = work(Memory::default());
        let mut writer = SnapshotPageWriter {
            pages: [(first_ref, first.len()), (second_ref, second.len())].into_iter(),
            roots: &[child],
            page: None,
            chunk: Vec::with_capacity(CHUNK_BYTES),
            output: &mut output,
            budget: &mut budget,
            failure: None,
        };
        io::Write::write_all(&mut writer, &payload[..7])?;
        io::Write::write_all(&mut writer, &payload[7..])?;
        ensure!(writer.page.is_none() && writer.pages.next().is_none());
        ensure!(writer.chunk.is_empty() && writer.failure.is_none());
        let extra = io::Write::write_all(&mut writer, &[1]);
        ensure!(extra.is_err() && writer.failure == Some(IndexError::DigestConflict));
        drop(writer);
        ensure!(output == expected);
        Ok(())
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
        let memory = Memory(Arc::default(), Some(64 * 1024));
        export_closure_v1(
            root_ref,
            500_000,
            |reference, _| {
                objects
                    .get(&reference)
                    .cloned()
                    .ok_or(IndexError::SnapshotUnavailable)
            },
            &mut output,
            &mut work(memory.clone()),
        )?;
        ensure!(memory.0.load(Ordering::Relaxed) == 0);
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
            &mut work(Memory::default()),
        );
        ensure!(failure.is_err() && output.is_empty());
        let failure = export_closure_v1(
            root.registration.reference,
            1,
            |_, _| Ok(root.clone()),
            &mut output,
            &mut work(Memory::default()),
        );
        ensure!(failure.is_err() && output.is_empty());
        let zero = export_closure_v1(
            root.registration.reference,
            0,
            |_, _| Ok(root.clone()),
            &mut output,
            &mut work(Memory::default()),
        );
        ensure!(
            matches!(
                zero.map_err(export_index_error),
                Err(IndexError::ResourceLimited)
            ) && output.is_empty()
        );
        let slot_bytes = u64::try_from(std::mem::size_of::<(ObjectRef, bool)>())?;
        let memory = Memory(Arc::default(), Some(2 * slot_bytes));
        let growth = export_closure_v1(
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
            &mut work(memory.clone()),
        );
        ensure!(matches!(
            growth.map_err(export_index_error),
            Err(IndexError::ResourceLimited)
        ));
        ensure!(output.is_empty() && memory.0.load(Ordering::Relaxed) == 0);
        Ok(())
    }

    #[test]
    fn native_export_preserves_funded_refusal_after_provisional_frames() -> Result<()> {
        let good = object(vec![3], vec![])?;
        let missing = ObjectRef(Digest([99; 32]));
        let root = object(vec![4], vec![good.registration.reference, missing])?;
        let mut output = Vec::new();
        let memory = Memory::default();
        let mut budget = work(memory.clone());
        let mut live_at_refusal = 0;
        let result = export_closure_v1(
            root.registration.reference,
            4,
            |reference, _| {
                if reference == root.registration.reference {
                    Ok(root.clone())
                } else if reference == good.registration.reference {
                    Ok(good.clone())
                } else if reference == missing {
                    live_at_refusal = memory.0.load(Ordering::Relaxed);
                    Err(IndexError::SnapshotUnavailable)
                } else {
                    Err(IndexError::DigestConflict)
                }
            },
            &mut output,
            &mut budget,
        );
        ensure!(matches!(
            result.map_err(export_index_error),
            Err(IndexError::SnapshotUnavailable)
        ));
        ensure!(live_at_refusal > 0, "frontier loan was not held at refusal");
        ensure!(
            memory.0.load(Ordering::Relaxed) == 0,
            "frontier loan survived the failed export"
        );
        let emitted = frames(&output)?;
        ensure!(
            emitted
                .iter()
                .any(|frame| matches!(frame, Frame::NativeObjectEnd { reference } if *reference == good.registration.reference))
        );
        ensure!(
            !emitted
                .iter()
                .any(|frame| matches!(frame, Frame::NativeObjectEnd { reference } if *reference == root.registration.reference))
        );

        let mut exhausted = work(Memory::default());
        exhausted.charge(WorkUsage {
            bytes: 10_000_000,
            ..WorkUsage::default()
        })?;
        let result = write_frame(
            &mut Vec::new(),
            &ExportFrame::NativeObjectEnd {
                reference: good.registration.reference,
            },
            &mut exhausted,
        );
        ensure!(matches!(
            result.map_err(export_index_error),
            Err(IndexError::ResourceLimited)
        ));
        Ok(())
    }
}
