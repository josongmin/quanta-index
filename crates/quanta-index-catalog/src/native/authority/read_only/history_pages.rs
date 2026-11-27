//! Bounded provisional pages from one quiescent Legacy History transaction.
//! Page delivery cannot issue a replay cut or attest the eventual Native root.

use std::{io, mem};

use quanta_index_contract::{
    index::{
        CollectionId, HistorySnapshotPageV1, HistorySnapshotRowV1, IngressRef,
        MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1, MAX_HISTORY_SNAPSHOT_ROWS_PER_PAGE_V1, OperationKey,
        ProducerId, SlotId, StreamId, TenantId, WorkspaceKey, WorktreeId,
    },
    native_protocol::{
        NativeIndexRequest, NativeIndexRequestEnvelope, NativeProjectionOutputChildren,
    },
};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, AuxiliaryRowV1,
    index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage},
};
use quanta_index_ipc::{
    CborPayloadLengthProgressErrorV1, MAX_FRAME_BODY_BYTES, cbor_payload_len_with_progress_v1,
};
use serde::Serialize;

use super::{
    CatalogReadOnlyIoAdmission, LegacyHistorySnapshotObservationV1, Path,
    observe_history_in_snapshot, with_quiescent_legacy_history_all_v2,
    with_quiescent_legacy_history_v1,
};

/// One provisional page from a quiescent Legacy History read. An oversized
/// physical row keeps the same page ordinal while the caller encodes it as a
/// bounded immutable object closure.
#[derive(Clone, Copy)]
pub enum LegacyHistoryCheckpointPageV1<'a> {
    V1 {
        page: &'a HistorySnapshotPageV1,
        encoded_bytes: usize,
    },
    LargeRow {
        generation: &'a AuxiliaryGenerationKeyV1,
        ordinal: u32,
        row: &'a HistorySnapshotRowV1,
    },
}

struct CountWriter<'a> {
    bytes: usize,
    budget: &'a mut WorkBudget,
    refusal: Option<IndexError>,
}
impl io::Write for CountWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("History page JSON length overflow"))?;
        if let Err(error) = self.budget.charge(WorkUsage {
            bytes: u64::try_from(bytes.len())
                .map_err(|_| io::Error::other("History page chunk exceeds u64"))?,
            ..WorkUsage::default()
        }) {
            self.refusal = Some(error);
            return Err(io::Error::other("History page JSON sizing work refused"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encoded_len(value: &impl Serialize, budget: &mut WorkBudget) -> Result<usize, IndexError> {
    let mut count = CountWriter {
        bytes: 0,
        budget,
        refusal: None,
    };
    let result = serde_json::to_writer(&mut count, value);
    if let Some(error) = count.refusal {
        return Err(error);
    }
    result.map_err(|error| IndexError::Storage(format!("History page size: {error}")))?;
    Ok(count.bytes)
}

fn stage_page_request(bytes: Vec<u8>) -> Result<NativeIndexRequestEnvelope, IndexError> {
    // Size the canonical envelope at the largest encoding of every variable ID.
    Ok(NativeIndexRequestEnvelope {
        request_id: u64::MAX,
        payload: NativeIndexRequest::StageProjectionOutput {
            ingress: IngressRef {
                operation: OperationKey {
                    workspace: WorkspaceKey {
                        tenant: TenantId([u8::MAX; 16]),
                        collection: CollectionId([u8::MAX; 16]),
                        worktree: WorktreeId([u8::MAX; 16]),
                        slot: SlotId([u8::MAX; 16]),
                    },
                    epoch: u64::MAX,
                    producer: ProducerId([u8::MAX; 16]),
                    stream: StreamId([u8::MAX; 16]),
                    stream_epoch: u64::MAX,
                    sequence: u64::MAX,
                },
                fence: u64::MAX,
            },
            bytes,
            children: NativeProjectionOutputChildren::new(Vec::new())
                .map_err(|error| IndexError::Storage(error.into()))?,
        },
    })
}

fn packed_page_bytes(budget: &mut WorkBudget) -> Result<usize, IndexError> {
    let request = stage_page_request(Vec::new())?;
    let envelope_bytes = cbor_payload_len_with_progress_v1(&request, &mut |bytes| {
        budget.charge(WorkUsage {
            bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })
    })
    .map_err(|error| match error {
        CborPayloadLengthProgressErrorV1::Progress(error) => error,
        CborPayloadLengthProgressErrorV1::Codec(error) => IndexError::Storage(error.to_string()),
    })?;
    // Preserve the established grouping and immutable input IDs on retry.
    // StageProjectionOutput encodes Vec<u8> as an array of integers, each up to
    // two bytes. The frame-sized array needs at most four additional length bytes.
    let available = usize::try_from(envelope_bytes)
        .ok()
        .and_then(|bytes| bytes.checked_add(mem::size_of::<u32>()))
        .and_then(|bytes| MAX_FRAME_BODY_BYTES.checked_sub(bytes))
        .filter(|bytes| *bytes >= 2)
        .ok_or(IndexError::ResourceLimited)?;
    Ok((available / 2).min(MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1))
}

struct PageBuffer {
    page: HistorySnapshotPageV1,
    row_grants: Vec<Option<MemoryLease>>,
    _container_grant: Option<MemoryLease>,
    encoded_len: usize,
    packing_bytes: usize,
}

impl PageBuffer {
    fn new(
        generation: &AuxiliaryGenerationKeyV1,
        ordinal: u32,
        packing_bytes: usize,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        if usize::try_from(ordinal).ok().is_none_or(|ordinal| {
            ordinal > quanta_index_contract::native_protocol::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN
        }) {
            return Err(IndexError::Storage(
                "History checkpoint page count exceeds Native child bound".into(),
            ));
        }
        let slots = MAX_HISTORY_SNAPSHOT_ROWS_PER_PAGE_V1;
        let bytes = slots
            .checked_mul(
                mem::size_of::<HistorySnapshotRowV1>() + mem::size_of::<Option<MemoryLease>>(),
            )
            .and_then(|bytes| bytes.checked_mul(2))
            .and_then(|bytes| bytes.checked_add(mem::size_of::<HistorySnapshotPageV1>() + 2048))
            .and_then(|bytes| bytes.checked_add(generation.repo_id.as_str().len()))
            .and_then(|bytes| bytes.checked_add(generation.revision_id.as_str().len()))
            .ok_or_else(|| IndexError::Storage("History page backing overflow".into()))?;
        let _container_grant = budget.reserve_memory(
            u64::try_from(bytes)
                .map_err(|_| IndexError::Storage("History page backing exceeds u64".into()))?,
            MemoryLifetime::Temporary,
        )?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(slots)
            .map_err(|_| IndexError::ResourceLimited)?;
        let mut row_grants = Vec::new();
        row_grants
            .try_reserve_exact(slots)
            .map_err(|_| IndexError::ResourceLimited)?;
        let page = HistorySnapshotPageV1 {
            wire_version: 1,
            generation: generation.clone(),
            ordinal,
            rows,
        };
        let encoded_len = encoded_len(&page, budget)?;
        Ok(Self {
            page,
            row_grants,
            _container_grant,
            encoded_len,
            packing_bytes,
        })
    }

    fn can_accept(&self, row_bytes: usize) -> bool {
        // Keep the established grouping for ordinary rows. A representable
        // larger row retains a V1 page with the same canonical bytes.
        let byte_limit = if self.page.rows.is_empty() {
            MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1
        } else {
            self.packing_bytes
        };
        self.page.rows.len() < MAX_HISTORY_SNAPSHOT_ROWS_PER_PAGE_V1
            && self
                .encoded_len
                .checked_add(row_bytes)
                .and_then(|bytes| bytes.checked_add(usize::from(!self.page.rows.is_empty())))
                .is_some_and(|bytes| bytes <= byte_limit)
    }

    fn push(
        &mut self,
        row: HistorySnapshotRowV1,
        row_bytes: usize,
        grant: Option<MemoryLease>,
    ) -> Result<(), IndexError> {
        if !self.can_accept(row_bytes) {
            return Err(IndexError::Storage(
                "History snapshot row exceeds page bound".into(),
            ));
        }
        self.encoded_len += row_bytes + usize::from(!self.page.rows.is_empty());
        self.row_grants.push(grant);
        self.page.rows.push(row);
        Ok(())
    }
}

struct Pages<'a> {
    generation: &'a AuxiliaryGenerationKeyV1,
    packing_bytes: usize,
    next_ordinal: u32,
    row_count: u64,
    active: Option<PageBuffer>,
    sink: &'a mut dyn FnMut(
        LegacyHistoryCheckpointPageV1<'_>,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
}

impl Pages<'_> {
    fn flush(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if let Some(page) = self.active.take() {
            page.page
                .validate_shape()
                .map_err(|error| IndexError::Storage(error.into()))?;
            (self.sink)(
                LegacyHistoryCheckpointPageV1::V1 {
                    page: &page.page,
                    encoded_bytes: page.encoded_len,
                },
                budget,
            )?;
            self.next_ordinal = self
                .next_ordinal
                .checked_add(1)
                .ok_or_else(|| IndexError::Storage("History page ordinal overflow".into()))?;
        }
        Ok(())
    }

    fn accept(&mut self, row: AuxiliaryRowV1, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if row.key.generation != *self.generation {
            return Err(IndexError::Storage("foreign History checkpoint row".into()));
        }
        let family = row.key.family.as_code_str();
        let backing = row
            .key
            .row_key
            .capacity()
            .checked_add(row.value.capacity())
            .and_then(|bytes| bytes.checked_add(family.len() + 128))
            .ok_or_else(|| IndexError::Storage("History checkpoint row backing overflow".into()))?;
        let grant = budget.reserve_memory(
            u64::try_from(backing).map_err(|_| {
                IndexError::Storage("History checkpoint row backing exceeds u64".into())
            })?,
            MemoryLifetime::Temporary,
        )?;
        let payload = HistorySnapshotRowV1 {
            family: family.to_owned(),
            row_key: row.key.row_key,
            value: row.value,
        };
        let row_bytes = encoded_len(&payload, budget)?;
        if self.active.is_none() {
            self.active = Some(PageBuffer::new(
                self.generation,
                self.next_ordinal,
                self.packing_bytes,
                budget,
            )?);
        }
        if self
            .active
            .as_ref()
            .is_some_and(|page| !page.page.rows.is_empty() && !page.can_accept(row_bytes))
        {
            self.flush(budget)?;
            self.active = Some(PageBuffer::new(
                self.generation,
                self.next_ordinal,
                self.packing_bytes,
                budget,
            )?);
        }
        if !self
            .active
            .as_ref()
            .is_some_and(|page| page.can_accept(row_bytes))
        {
            drop(self.active.take());
            let delivered = (self.sink)(
                LegacyHistoryCheckpointPageV1::LargeRow {
                    generation: self.generation,
                    ordinal: self.next_ordinal,
                    row: &payload,
                },
                budget,
            );
            drop(payload);
            drop(grant);
            delivered?;
            self.next_ordinal = self
                .next_ordinal
                .checked_add(1)
                .ok_or_else(|| IndexError::Storage("History page ordinal overflow".into()))?;
            self.row_count = self
                .row_count
                .checked_add(1)
                .ok_or_else(|| IndexError::Storage("History page row count overflow".into()))?;
            return Ok(());
        }
        self.active
            .as_mut()
            .ok_or_else(|| IndexError::Storage("History page absent".into()))?
            .push(payload, row_bytes, grant)?;
        self.row_count = self
            .row_count
            .checked_add(1)
            .ok_or_else(|| IndexError::Storage("History page row count overflow".into()))?;
        Ok(())
    }
}

/// Lend V1 pages and oversized physical rows while the saved Legacy root and
/// one quiescent SQL read transaction remain live. Each item is provisional
/// until the observation and producer scope are authenticated by the candidate.
pub fn visit_legacy_history_checkpoint_pages_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    generation: &AuxiliaryGenerationKeyV1,
    budget: &mut WorkBudget,
    sink: &mut dyn FnMut(
        LegacyHistoryCheckpointPageV1<'_>,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
) -> Result<LegacyHistorySnapshotObservationV1, IndexError> {
    with_quiescent_legacy_history_v1(
        state_root,
        io,
        &generation.repo_id,
        budget,
        |connection, path, budget| {
            visit_checkpoint_pages_in_snapshot_v1(connection, path, generation, budget, sink)
        },
    )
}

/// Admit one checkpoint among all physical Legacy History repositories. The
/// page and ACK encoding is identical to the original repository-local path.
pub fn visit_legacy_history_checkpoint_pages_all_v2(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    generation: &AuxiliaryGenerationKeyV1,
    budget: &mut WorkBudget,
    sink: &mut dyn FnMut(LegacyHistoryCheckpointPageV1<'_>, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<LegacyHistorySnapshotObservationV1, IndexError> {
    with_quiescent_legacy_history_all_v2(state_root, io, budget, |connection, path, budget| {
        visit_checkpoint_pages_in_snapshot_v1(connection, path, generation, budget, sink)
    })
}

fn visit_checkpoint_pages_in_snapshot_v1(
    connection: &rusqlite::Connection,
    path: &Path,
    generation: &AuxiliaryGenerationKeyV1,
    budget: &mut WorkBudget,
    sink: &mut dyn FnMut(LegacyHistoryCheckpointPageV1<'_>, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<LegacyHistorySnapshotObservationV1, IndexError> {
    let mut pages = Pages {
        generation,
        packing_bytes: packed_page_bytes(budget)?,
        next_ordinal: 1,
        row_count: 0,
        active: None,
        sink,
    };
    let observation = observe_history_in_snapshot(
        connection,
        path,
        generation,
        budget,
        &mut |row, budget| pages.accept(row, budget).map_err(Into::into),
    )?;
    pages.flush(budget)?;
    if pages.row_count != observation.row_count {
        return Err(IndexError::DigestConflict);
    }
    Ok(observation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{
        DiffHunkSide, ManifestGeneration, RepoId, RevisionId, lex::DiffHunkRecord,
    };
    use quanta_index_core::{
        AuxiliaryDomainV1, AuxiliaryRowFamilyV1, AuxiliaryRowKeyV1, index::WorkLimits,
    };

    fn work() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 1_000_000_000,
                mutations: 1_000_000,
            },
            std::time::Instant::now() + std::time::Duration::from_secs(90),
        )
    }

    #[test]
    fn ordinary_history_page_packing_fits_one_canonical_cbor_frame() {
        let bound = packed_page_bytes(&mut work())
            .expect("determine the admitted History page payload bound");
        assert!(bound > 0);
        let within_request =
            stage_page_request(vec![b'0'; bound]).expect("build the maximal History page request");
        let within = quanta_index_ipc::cbor_payload_len(&within_request)
            .expect("encode the maximal History page request");
        let outside_request = stage_page_request(vec![b'0'; bound + 1])
            .expect("build the first oversized History page request");
        let outside = quanta_index_ipc::cbor_payload_len(&outside_request)
            .expect("encode the first oversized History page request");
        let frame = u64::try_from(quanta_index_ipc::MAX_FRAME_BODY_BYTES)
            .expect("IPC frame bound fits u64");
        assert!(within <= frame, "the admitted page must fit one CBOR frame");
        assert!(outside > frame, "the page bound must be maximal");
    }

    #[test]
    fn legacy_history_isolates_a_large_row_and_preserves_ordinary_packing()
    -> Result<(), Box<dyn std::error::Error>> {
        let generation = AuxiliaryGenerationKeyV1 {
            repo_id: RepoId::new("legacy-history")?,
            revision_id: RevisionId::new("main")?,
            generation: ManifestGeneration::new(1),
        };
        let mut budget = work();
        let mut observed = Vec::new();
        {
            let mut sink = |page: LegacyHistoryCheckpointPageV1<'_>, _budget: &mut WorkBudget| {
                let LegacyHistoryCheckpointPageV1::V1 { page, .. } = page else {
                    return Err(IndexError::Storage(
                        "representable History row required a large-row page".into(),
                    ));
                };
                let keys = page
                    .rows
                    .iter()
                    .map(|row| row.row_key[0])
                    .collect::<Vec<_>>();
                let payload = serde_json::to_vec(page)
                    .map_err(|error| IndexError::Storage(error.to_string()))?;
                let payload_len = payload.len();
                let staged = quanta_index_ipc::cbor_payload_len(&stage_page_request(payload)?)
                    .map_err(|error| IndexError::Storage(error.to_string()))?;
                observed.push((page.ordinal, keys, payload_len, staged));
                Ok(())
            };
            let mut pages = Pages {
                generation: &generation,
                packing_bytes: packed_page_bytes(&mut budget)?,
                next_ordinal: 1,
                row_count: 0,
                active: None,
                sink: &mut sink,
            };
            for key in 1_u8..=5 {
                let width = if key == 3 { 3 * 1024 * 1024 } else { 1 };
                let record = DiffHunkRecord {
                    wire_version: 1,
                    hunk_header: "@@".into(),
                    side: DiffHunkSide::After,
                    added_text: "x".repeat(width).into_boxed_str(),
                    removed_text: "".into(),
                    touched_text: "".into(),
                    byte_start: 0,
                    byte_end: u32::try_from(width)?,
                };
                let mut row_key = vec![key; 20];
                row_key.extend_from_slice(b"\x1fsrc/file.rs");
                pages.accept(
                    AuxiliaryRowV1 {
                        key: AuxiliaryRowKeyV1 {
                            domain: AuxiliaryDomainV1::History,
                            generation: generation.clone(),
                            family: AuxiliaryRowFamilyV1::DiffHunk,
                            row_key,
                        },
                        value: quanta_index_ipc::encode_cbor_payload(&record)?,
                    },
                    &mut budget,
                )?;
            }
            pages.flush(&mut budget)?;
            assert_eq!(pages.row_count, 5);
        }

        let [
            (first, before, _, _),
            (second, large, page_bytes, stage_bytes),
            (third, after, _, _),
        ] = observed.as_slice()
        else {
            return Err("expected ordinary, isolated, and ordinary History pages".into());
        };
        assert_eq!((*first, before.as_slice()), (1, &[1, 2][..]));
        assert_eq!((*second, large.as_slice()), (2, &[3][..]));
        assert_eq!((*third, after.as_slice()), (3, &[4, 5][..]));
        assert!(
            (MAX_FRAME_BODY_BYTES / 2..=MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1).contains(page_bytes)
        );
        assert!(*stage_bytes > u64::try_from(MAX_FRAME_BODY_BYTES)?);
        assert!(
            *stage_bytes <= u64::try_from(quanta_index_ipc::MAX_MULTIFRAME_REQUEST_BODY_BYTES)?
        );
        Ok(())
    }

    #[test]
    fn legacy_history_exports_an_unrepresentable_v1_row_at_its_original_ordinal()
    -> Result<(), Box<dyn std::error::Error>> {
        let generation = AuxiliaryGenerationKeyV1 {
            repo_id: RepoId::new("legacy-history")?,
            revision_id: RevisionId::new("main")?,
            generation: ManifestGeneration::new(1),
        };
        let mut budget = work();
        let mut observed = Vec::new();
        {
            let mut sink = |page: LegacyHistoryCheckpointPageV1<'_>, _budget: &mut WorkBudget| {
                match page {
                    LegacyHistoryCheckpointPageV1::V1 {
                        page,
                        encoded_bytes,
                    } => {
                        page.validate_shape()
                            .map_err(|cause| IndexError::Storage(cause.into()))?;
                        if page.rows.len() != 1
                            || encoded_bytes > MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1
                        {
                            return Err(IndexError::DigestConflict);
                        }
                        observed.push((page.ordinal, page.rows[0].row_key[0], false));
                    }
                    LegacyHistoryCheckpointPageV1::LargeRow {
                        generation: selected,
                        ordinal,
                        row,
                    } => {
                        if selected != &generation
                            || row.family != AuxiliaryRowFamilyV1::DiffHunk.as_code_str()
                            || row.value.len() <= 5 * 1024 * 1024
                        {
                            return Err(IndexError::DigestConflict);
                        }
                        observed.push((ordinal, row.row_key[0], true));
                    }
                }
                Ok(())
            };
            let mut pages = Pages {
                generation: &generation,
                packing_bytes: packed_page_bytes(&mut budget)?,
                next_ordinal: 1,
                row_count: 0,
                active: None,
                sink: &mut sink,
            };
            for (key, width) in [
                (1_u8, 1),
                (2, 5 * 1024 * 1024),
                (3, 5 * 1024 * 1024),
                (4, 1),
            ] {
                let record = DiffHunkRecord {
                    wire_version: 1,
                    hunk_header: "@@".into(),
                    side: DiffHunkSide::After,
                    added_text: "x".repeat(width).into_boxed_str(),
                    removed_text: "".into(),
                    touched_text: "".into(),
                    byte_start: 0,
                    byte_end: u32::try_from(width)?,
                };
                let mut row_key = vec![key; 20];
                row_key.extend_from_slice(b"\x1fsrc/file.rs");
                pages.accept(
                    AuxiliaryRowV1 {
                        key: AuxiliaryRowKeyV1 {
                            domain: AuxiliaryDomainV1::History,
                            generation: generation.clone(),
                            family: AuxiliaryRowFamilyV1::DiffHunk,
                            row_key,
                        },
                        value: quanta_index_ipc::encode_cbor_payload(&record)?,
                    },
                    &mut budget,
                )?;
            }
            pages.flush(&mut budget)?;
            assert_eq!(pages.row_count, 4);
        }
        assert_eq!(
            observed,
            vec![(1, 1, false), (2, 2, true), (3, 3, true), (4, 4, false),]
        );
        Ok(())
    }
}
