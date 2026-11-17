//! Bounded provisional pages from one quiescent Legacy History transaction.
//! Page delivery cannot issue a replay cut or attest the eventual Native root.

use std::{io, mem};

use quanta_index_contract::{
    SearchPlaneErrorCodeV2,
    index::{
        CollectionId, HistorySnapshotPageV1, HistorySnapshotRowV1, IngressRef,
        MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1, MAX_HISTORY_SNAPSHOT_ROWS_PER_PAGE_V1, OperationKey,
        ProducerId, SlotId, StreamId, TenantId, WorkspaceKey, WorktreeId,
    },
    native_protocol::{NativeIndexRequest, NativeIndexRequestEnvelope, NativeProjectionOutputChildren},
};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, AuxiliaryRowV1, CoreError,
    index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage},
};
use quanta_index_ipc::{
    CborPayloadLengthProgressErrorV1, MAX_FRAME_BODY_BYTES, cbor_payload_len_with_progress_v1,
};
use serde::Serialize;

use super::{
    CatalogReadOnlyIoAdmission, LegacyHistorySnapshotObservationV1, Path,
    observe_history_in_snapshot, with_quiescent_legacy_history_v1,
};

fn work_error(budget: &WorkBudget, error: IndexError) -> CoreError {
    if let Err(interrupted) = budget.request().checkpoint("Catalog History page export") {
        return interrupted;
    }
    match error {
        IndexError::ResourceLimited | IndexError::WorkLimitExceeded => CoreError::Typed {
            code: SearchPlaneErrorCodeV2::ServerOverloaded,
            message: "Legacy History checkpoint page exceeds admitted work".into(),
        },
        other => CoreError::Storage(format!("Legacy History checkpoint page: {other}")),
    }
}

struct CountWriter<'a> {
    bytes: usize,
    budget: &'a mut WorkBudget,
    refusal: Option<CoreError>,
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
            self.refusal = Some(work_error(self.budget, error));
            return Err(io::Error::other("History page JSON sizing work refused"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encoded_len(value: &impl Serialize, budget: &mut WorkBudget) -> Result<usize, CoreError> {
    let mut count = CountWriter {
        bytes: 0,
        budget,
        refusal: None,
    };
    let result = serde_json::to_writer(&mut count, value);
    if let Some(error) = count.refusal {
        return Err(error);
    }
    result.map_err(|error| CoreError::Storage(format!("History page size: {error}")))?;
    Ok(count.bytes)
}

fn stage_page_request(bytes: Vec<u8>) -> Result<NativeIndexRequestEnvelope, CoreError> {
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
                .map_err(|error| CoreError::Storage(error.into()))?,
        },
    })
}

fn maximum_staged_page_bytes(budget: &mut WorkBudget) -> Result<usize, CoreError> {
    let request = stage_page_request(Vec::new())?;
    let envelope_bytes = cbor_payload_len_with_progress_v1(&request, &mut |bytes| {
        budget.charge(WorkUsage {
            bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })
    })
    .map_err(|error| match error {
        CborPayloadLengthProgressErrorV1::Progress(error) => work_error(budget, error),
        CborPayloadLengthProgressErrorV1::Codec(error) => CoreError::Storage(error.to_string()),
    })?;
    // The admitted producer sends one CBOR frame. StageProjectionOutput encodes
    // Vec<u8> as an array of integers, each up to two bytes. A frame-sized
    // array needs at most four length bytes beyond the empty array header.
    let available = usize::try_from(envelope_bytes)
        .ok()
        .and_then(|bytes| bytes.checked_add(mem::size_of::<u32>()))
        .and_then(|bytes| MAX_FRAME_BODY_BYTES.checked_sub(bytes))
        .filter(|bytes| *bytes >= 2)
        .ok_or_else(|| work_error(budget, IndexError::ResourceLimited))?;
    Ok((available / 2).min(MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1))
}

struct PageBuffer {
    page: HistorySnapshotPageV1,
    row_grants: Vec<Option<MemoryLease>>,
    _container_grant: Option<MemoryLease>,
    encoded_len: usize,
    maximum_bytes: usize,
}

impl PageBuffer {
    fn new(
        generation: &AuxiliaryGenerationKeyV1,
        ordinal: u32,
        maximum_bytes: usize,
        budget: &mut WorkBudget,
    ) -> Result<Self, CoreError> {
        if usize::try_from(ordinal).ok().is_none_or(|ordinal| {
            ordinal > quanta_index_contract::native_protocol::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN
        }) {
            return Err(CoreError::Storage(
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
            .ok_or_else(|| CoreError::Storage("History page backing overflow".into()))?;
        let _container_grant = budget
            .reserve_memory(
                u64::try_from(bytes)
                    .map_err(|_| CoreError::Storage("History page backing exceeds u64".into()))?,
                MemoryLifetime::Temporary,
            )
            .map_err(|error| work_error(budget, error))?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(slots).map_err(|error| {
            CoreError::Storage(format!("History page rows allocation: {error}"))
        })?;
        let mut row_grants = Vec::new();
        row_grants.try_reserve_exact(slots).map_err(|error| {
            CoreError::Storage(format!("History page grants allocation: {error}"))
        })?;
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
            maximum_bytes,
        })
    }

    fn can_accept(&self, row_bytes: usize) -> bool {
        self.page.rows.len() < MAX_HISTORY_SNAPSHOT_ROWS_PER_PAGE_V1
            && self
                .encoded_len
                .checked_add(row_bytes)
                .and_then(|bytes| bytes.checked_add(usize::from(!self.page.rows.is_empty())))
                .is_some_and(|bytes| bytes <= self.maximum_bytes)
    }

    fn push(
        &mut self,
        row: HistorySnapshotRowV1,
        row_bytes: usize,
        grant: Option<MemoryLease>,
    ) -> Result<(), CoreError> {
        if !self.can_accept(row_bytes) {
            return Err(CoreError::Storage(
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
    maximum_bytes: usize,
    next_ordinal: u32,
    row_count: u64,
    active: Option<PageBuffer>,
    sink: &'a mut dyn FnMut(&HistorySnapshotPageV1, &mut WorkBudget) -> Result<(), CoreError>,
}

impl Pages<'_> {
    fn flush(&mut self, budget: &mut WorkBudget) -> Result<(), CoreError> {
        if let Some(page) = self.active.take() {
            page.page
                .validate_shape()
                .map_err(|error| CoreError::Storage(error.into()))?;
            (self.sink)(&page.page, budget)?;
            self.next_ordinal = self
                .next_ordinal
                .checked_add(1)
                .ok_or_else(|| CoreError::Storage("History page ordinal overflow".into()))?;
        }
        Ok(())
    }

    fn accept(&mut self, row: AuxiliaryRowV1, budget: &mut WorkBudget) -> Result<(), CoreError> {
        if row.key.generation != *self.generation {
            return Err(CoreError::Storage("foreign History checkpoint row".into()));
        }
        let family = row.key.family.as_code_str();
        let backing = row
            .key
            .row_key
            .capacity()
            .checked_add(row.value.capacity())
            .and_then(|bytes| bytes.checked_add(family.len() + 128))
            .ok_or_else(|| CoreError::Storage("History checkpoint row backing overflow".into()))?;
        let grant = budget
            .reserve_memory(
                u64::try_from(backing).map_err(|_| {
                    CoreError::Storage("History checkpoint row backing exceeds u64".into())
                })?,
                MemoryLifetime::Temporary,
            )
            .map_err(|error| work_error(budget, error))?;
        let payload = HistorySnapshotRowV1 {
            family: family.to_owned(),
            row_key: row.key.row_key,
            value: row.value,
        };
        let row_bytes = encoded_len(&payload, budget)?;
        if self.active.is_none() {
            self.active = Some(PageBuffer::new(
                self.generation, self.next_ordinal, self.maximum_bytes, budget,
            )?);
        }
        if !self
            .active
            .as_ref()
            .is_some_and(|page| page.can_accept(row_bytes))
        {
            self.flush(budget)?;
            self.active = Some(PageBuffer::new(
                self.generation, self.next_ordinal, self.maximum_bytes, budget,
            )?);
        }
        self.active
            .as_mut()
            .ok_or_else(|| CoreError::Storage("History page absent".into()))?
            .push(payload, row_bytes, grant)?;
        self.row_count = self
            .row_count
            .checked_add(1)
            .ok_or_else(|| CoreError::Storage("History page row count overflow".into()))?;
        Ok(())
    }
}

/// Lend childless pages while the saved Legacy root and one quiescent SQL read
/// transaction remain live. Every page is provisional until the returned
/// observation and producer scope are authenticated by the candidate owner.
pub fn visit_legacy_history_checkpoint_pages_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    generation: &AuxiliaryGenerationKeyV1,
    budget: &mut WorkBudget,
    sink: &mut dyn FnMut(&HistorySnapshotPageV1, &mut WorkBudget) -> Result<(), CoreError>,
) -> Result<LegacyHistorySnapshotObservationV1, IndexError> {
    with_quiescent_legacy_history_v1(
        state_root,
        io,
        &generation.repo_id,
        budget,
        |connection, path, budget| {
            let mut pages = Pages {
                generation,
                maximum_bytes: maximum_staged_page_bytes(budget).map_err(super::core)?,
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
                &mut |row, budget| pages.accept(row, budget),
            )?;
            pages.flush(budget).map_err(super::core)?;
            if pages.row_count != observation.row_count {
                return Err(IndexError::DigestConflict);
            }
            Ok(observation)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::WorkLimits;

    #[test]
    fn staged_history_page_bound_fits_one_canonical_cbor_frame() {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 1_000_000_000,
                mutations: 1_000_000,
            },
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        );
        let bound = maximum_staged_page_bytes(&mut budget)
            .expect("determine the admitted History page payload bound");
        assert!(bound > 0);
        let within_request = stage_page_request(vec![b'0'; bound])
            .expect("build the maximal History page request");
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
}
