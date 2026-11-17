//! Reconstruct a complete source snapshot from authenticated producer loans.
//! This never reads the target Catalog to manufacture its expected digest.

use super::*;
use quanta_index_contract::BatchPublishReceipt;
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectRef, OperationKey, WorkUsage,
};
use quanta_index_ipc::{
    CanonicalBatchDigestProgressError, CborPayloadLengthProgressErrorV1,
    canonical_batch_digest_with_progress_v1, cbor_payload_len_with_progress_v1,
    matches_batch_digest_token_v1,
};
use sha2::{Digest as _, Sha256};

pub use quanta_index_contract::index::HistoryReplayFenceV1;

/// Single-use fold. An error consumes the provisional state: a failed visit
/// cannot be retried on a partially applied fold. ACK sequence is root-global,
/// whereas the publication ordinal/auxiliary epoch is History-local.
pub struct HistorySourceReplayV1 {
    fence: HistoryReplayFenceV1,
    state: HistoryAuthorityState,
    ordinal: u64,
    sequence: u64,
    replay: Digest,
    // State must be destroyed before the DATA custody that funds its clones.
    custody: ReplayCustody,
}

/// Grants follow the current rows, rather than every historical publication.
/// Superseded grants stay here until their old state rows have been dropped.
#[derive(Default)]
pub(crate) struct ReplayCustody {
    rows: std::collections::BTreeMap<(AuxiliaryRowFamilyV1, Vec<u8>), Option<MemoryLease>>,
    retired: Vec<Option<MemoryLease>>,
    generation_identity: Option<MemoryLease>,
}
impl ReplayCustody {
    pub(crate) fn replace(
        &mut self,
        family: AuxiliaryRowFamilyV1,
        key: Vec<u8>,
        memory: Option<MemoryLease>,
    ) {
        if let Some(old) = self.rows.insert((family, key), memory) {
            self.retired.push(old);
        }
    }
    fn remove(&mut self, family: AuxiliaryRowFamilyV1, key: Vec<u8>) {
        if let Some(old) = self.rows.remove(&(family, key)) {
            self.retired.push(old);
        }
    }
    fn settle(&mut self) {
        self.retired = Vec::new();
    }
}

/// Complete fold only. Consumers borrow the state and cut together, retaining
/// the original allocation custody until their immutable conversion finishes.
pub struct ReplayedHistorySourceV1 {
    pub(crate) state: HistoryAuthorityState,
    pub(crate) cut: HistorySourceCutV1,
    pub(crate) sequence: u64,
    pub(crate) manifest_digest: Option<String>,
    pub(crate) _custody: ReplayCustody,
}

impl ReplayedHistorySourceV1 {
    #[must_use]
    pub const fn cut(&self) -> &HistorySourceCutV1 {
        &self.cut
    }

    /// Continue a complete prior snapshot under a new authenticated source
    /// fence. The new count is cumulative; the first replay is this cut's end.
    pub fn resume(self, fence: HistoryReplayFenceV1) -> Result<HistorySourceReplayV1, CoreError> {
        if fence.generation != self.cut.generation
            || fence.manifest_digest != self.manifest_digest
            || fence.first_replay != self.cut.replay_start
            || fence.final_replay == Digest([0; 32])
            || fence.first_replay == fence.final_replay
            || fence.publication_count <= self.cut.durable_epoch.get()
            || fence.final_durable_sequence <= self.sequence
        {
            return Err(corrupt_history(
                "History DELTA does not continue the complete prior cut",
            ));
        }
        Ok(HistorySourceReplayV1 {
            replay: self.cut.replay_start,
            ordinal: self.cut.durable_epoch.get(),
            sequence: self.sequence,
            fence,
            state: self.state,
            custody: self._custody,
        })
    }

    /// Materialize an original accepted by the active Native operation. Only
    /// publication of that operation makes the resulting cut durable.
    pub(crate) fn append_native(
        mut self,
        operation: OperationKey,
        next_view_sequence: u64,
        input: ObjectRef,
        batch: &HistoryIngestBatch,
        budget: &mut WorkBudget,
    ) -> Result<Self, CoreError> {
        let generation = &self.cut.generation;
        if batch.repo_id != generation.repo_id
            || batch.revision_id != generation.revision_id
            || batch.generation != generation.generation
            || batch.manifest_digest != self.manifest_digest
            || next_view_sequence == 0
        {
            return Err(corrupt_history(
                "Native History original differs from the prior scope",
            ));
        }
        let epoch = self
            .cut
            .durable_epoch
            .checked_next()
            .ok_or_else(|| corrupt_history("Native History epoch overflow"))?;
        let body = read_history_batch(batch, budget)?;
        if body.replaces == 0 && body.deletes == 0 {
            return Err(corrupt_history("Native History original contains no mutations"));
        }
        apply_history_body(
            &mut self.state,
            &mut self._custody,
            epoch.get(),
            batch,
            &body,
            budget,
        )?;
        let snapshot = history_snapshot_digest(generation, epoch, &self.state, budget)?;
        budget
            .charge(WorkUsage {
                bytes: 384,
                ..WorkUsage::default()
            })
            .map_err(|error| history_work_error(budget, error))?;
        self.cut.replay_start = native_replay_position(
            b"qidx.native.history.append.v1\0", &self.cut, operation,
            next_view_sequence, epoch, input, body.digest, snapshot,
        );
        self.cut.snapshot_digest = snapshot;
        self.cut.durable_epoch = epoch;
        self.sequence = next_view_sequence;
        budget
            .checkpoint()
            .map_err(|error| history_work_error(budget, error))?;
        Ok(self)
    }

    /// Carry the exact committed History into a new generation and apply the
    /// original delta there. Prior rows supply Git parent/ref closure. The
    /// generation and its local epoch are bound by the new row-set digest.
    pub(crate) fn advance_native_generation(
        mut self,
        operation: OperationKey,
        next_view_sequence: u64,
        input: ObjectRef,
        batch: &HistoryIngestBatch,
        budget: &mut WorkBudget,
    ) -> Result<Self, CoreError> {
        let manifest = batch.manifest_digest.as_deref().filter(|value| !value.is_empty())
            .ok_or_else(|| corrupt_history("Native History successor manifest is absent"))?;
        if !self.cut.generation.is_next_generation_of_repo_v1(&batch.repo_id, batch.generation)
            || next_view_sequence == 0
        {
            return Err(corrupt_history("Native History generation does not succeed the prior scope"));
        }
        let identity_bytes = batch.repo_id.as_str().len()
            .checked_add(batch.revision_id.as_str().len())
            .and_then(|size| size.checked_add(manifest.len()))
            .and_then(|size| u64::try_from(size).ok())
            .ok_or_else(|| corrupt_history("Native History successor identity overflow"))?;
        let identity_memory = budget.reserve_memory(identity_bytes, MemoryLifetime::Retained)
            .map_err(|error| history_work_error(budget, error))?;
        let generation = generation_key(&batch.repo_id, &batch.revision_id, batch.generation);
        let epoch = AuxEpochV1::new(1);
        let body = read_history_batch(batch, budget)?;
        if body.replaces == 0 && body.deletes == 0 {
            return Err(corrupt_history("Native History original contains no mutations"));
        }
        apply_history_body(&mut self.state, &mut self._custody, epoch.get(), batch, &body, budget)?;
        let snapshot = history_snapshot_digest(&generation, epoch, &self.state, budget)?;
        budget.charge(WorkUsage { bytes: 384, ..WorkUsage::default() })
            .map_err(|error| history_work_error(budget, error))?;
        let replay = native_replay_position(
            b"qidx.native.history.generation.v1\0", &self.cut, operation,
            next_view_sequence, epoch, input, body.digest, snapshot,
        );
        budget.checkpoint().map_err(|error| history_work_error(budget, error))?;
        self.cut = HistorySourceCutV1 { generation, durable_epoch: epoch, snapshot_digest: snapshot, replay_start: replay };
        self.sequence = next_view_sequence;
        self.manifest_digest = Some(manifest.to_owned());
        self._custody.generation_identity = identity_memory;
        Ok(self)
    }

    /// Provisional rows in the Catalog's normative order, with one admitted row
    /// backing at a time. Consumer failure aborts the entire conversion.
    pub fn visit_rows(
        &self,
        budget: &mut WorkBudget,
        visit: &mut dyn FnMut(&AuxiliaryRowV1, &mut WorkBudget) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        visit_history_state_rows(
            &self.cut.generation,
            self.cut.durable_epoch,
            &self.state,
            budget,
            visit,
        )
    }
}

fn native_replay_position(
    domain: &[u8], prior: &HistorySourceCutV1, operation: OperationKey,
    view_sequence: u64, epoch: AuxEpochV1, input: ObjectRef, body: [u8; 32], snapshot: Digest,
) -> Digest {
    let mut replay = Sha256::new();
    replay.update(domain);
    replay.update((prior.replay_start).0);
    replay.update((prior.snapshot_digest).0);
    replay.update(operation.workspace.tenant.0);
    replay.update(operation.workspace.collection.0);
    replay.update(operation.workspace.worktree.0);
    replay.update(operation.workspace.slot.0);
    replay.update(operation.epoch.to_le_bytes());
    replay.update(operation.producer.0);
    replay.update(operation.stream.0);
    replay.update(operation.stream_epoch.to_le_bytes());
    replay.update(operation.sequence.to_le_bytes());
    replay.update(view_sequence.to_le_bytes());
    replay.update(epoch.get().to_le_bytes());
    replay.update((input.0).0);
    replay.update(body);
    replay.update(snapshot.0);
    Digest(replay.finalize().into())
}

impl HistorySourceReplayV1 {
    pub fn new(fence: HistoryReplayFenceV1) -> Result<Self, CoreError> {
        if fence.publication_count == 0
            || fence.final_durable_sequence == 0
            || fence.first_replay == Digest([0; 32])
            || fence.final_replay == Digest([0; 32])
            || fence.first_replay == fence.final_replay
        {
            return Err(corrupt_history(
                "History complete inventory fence is absent or empty",
            ));
        }
        Ok(Self {
            replay: fence.first_replay,
            fence,
            state: HistoryAuthorityState::default(),
            ordinal: 0,
            sequence: 0,
            custody: ReplayCustody::default(),
        })
    }

    /// The enclosing source validator authenticates `(ordinal, before, after)`
    /// against its complete inventory. This function additionally checks the
    /// actual canonical body, the full ACK and every History transition.
    pub fn apply(
        mut self,
        ordinal: u64,
        before: Digest,
        after: Digest,
        batch: &HistoryIngestBatch,
        ack: &BatchPublishReceipt,
        budget: &mut WorkBudget,
    ) -> Result<Self, CoreError> {
        budget
            .checkpoint()
            .map_err(|error| history_work_error(budget, error))?;
        if ordinal
            != self
                .ordinal
                .checked_add(1)
                .ok_or_else(|| corrupt_history("History ordinal overflow"))?
            || ordinal > self.fence.publication_count
            || before != self.replay
            || after == Digest([0; 32])
            || after == before
            || generation_key(&batch.repo_id, &batch.revision_id, batch.generation)
                != self.fence.generation
            || batch.manifest_digest != self.fence.manifest_digest
            || ack.generation != batch.generation
            || ack.manifest_digest != batch.manifest_digest
            || ack.batch_digest != batch.batch_digest
            || ack.durable_sequence <= self.sequence
            || ack.durable_sequence > self.fence.final_durable_sequence
            || ack.sealed
            || ack.semantic_content.is_some()
            || ack.accepted_clear_surfaces != 0
            || ack.accepted_semantic_replace_scopes != 0
            || ack.accepted_semantic_tombstone_scopes != 0
        {
            return Err(corrupt_history(
                "History publication does not continue its authenticated source fence",
            ));
        }
        let body = read_history_batch(batch, budget)?;
        if u32::try_from(body.replaces).ok() != Some(ack.accepted_replace_scopes)
            || u32::try_from(body.deletes).ok() != Some(ack.accepted_tombstone_scopes)
        {
            return Err(corrupt_history(
                "History ACK scope counts differ from its body",
            ));
        }
        apply_history_body(
            &mut self.state,
            &mut self.custody,
            ordinal,
            batch,
            &body,
            budget,
        )?;
        self.ordinal = ordinal;
        self.sequence = ack.durable_sequence;
        self.replay = after;
        Ok(self)
    }

    pub fn finish(self, budget: &mut WorkBudget) -> Result<ReplayedHistorySourceV1, CoreError> {
        if self.ordinal != self.fence.publication_count
            || self.sequence != self.fence.final_durable_sequence
            || self.replay != self.fence.final_replay
        {
            return Err(corrupt_history(
                "History source inventory is incomplete at its fenced replay boundary",
            ));
        }
        let epoch = AuxEpochV1::new(self.ordinal);
        let snapshot = history_snapshot_digest(&self.fence.generation, epoch, &self.state, budget)?;
        Ok(ReplayedHistorySourceV1 {
            state: self.state,
            cut: HistorySourceCutV1 {
                generation: self.fence.generation,
                durable_epoch: epoch,
                snapshot_digest: snapshot,
                replay_start: self.replay,
            },
            sequence: self.sequence,
            manifest_digest: self.fence.manifest_digest,
            _custody: self.custody,
        })
    }
}

struct HistoryBatchFacts {
    digest: [u8; 32],
    encoded_bytes: u64,
    replaces: usize,
    deletes: usize,
}

fn read_history_batch(
    batch: &HistoryIngestBatch,
    budget: &mut WorkBudget,
) -> Result<HistoryBatchFacts, CoreError> {
    let mut encoded_bytes = 0_u64;
    let digest = canonical_batch_digest_with_progress_v1(batch, &mut |bytes| {
        let bytes = u64::try_from(bytes)
            .map_err(|_| quanta_index_core::index::IndexError::ResourceLimited)?;
        encoded_bytes = encoded_bytes
            .checked_add(bytes)
            .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })
    })
    .map_err(|error| match error {
        CanonicalBatchDigestProgressError::Progress(error) => history_work_error(budget, error),
        CanonicalBatchDigestProgressError::Encode(error) => CoreError::Storage(error.to_string()),
    })?;
    if !matches_batch_digest_token_v1(&batch.batch_digest, &digest) {
        return Err(corrupt_history(
            "History body differs from its canonical batch digest",
        ));
    }
    let mut replaces = batch
        .commits
        .len()
        .checked_add(batch.diff_hunks.len())
        .ok_or_else(|| corrupt_history("History scope count overflow"))?;
    let mut deletes = 0_usize;
    for change in batch.refs.iter().chain(&batch.tags) {
        match change {
            HistoryRefMutation::Upsert(_) => {
                replaces = replaces
                    .checked_add(1)
                    .ok_or_else(|| corrupt_history("History scope count overflow"))?
            }
            HistoryRefMutation::Delete(_) => {
                deletes = deletes
                    .checked_add(1)
                    .ok_or_else(|| corrupt_history("History scope count overflow"))?
            }
        }
    }
    Ok(HistoryBatchFacts {
        digest,
        encoded_bytes,
        replaces,
        deletes,
    })
}

fn apply_history_body(
    state: &mut HistoryAuthorityState,
    custody: &mut ReplayCustody,
    ordinal: u64,
    batch: &HistoryIngestBatch,
    body: &HistoryBatchFacts,
    budget: &mut WorkBudget,
) -> Result<(), CoreError> {
    let HistoryBatchFacts {
        encoded_bytes,
        replaces,
        deletes,
        ..
    } = *body;
    // The batch scratch grant covers transition/row-encoding clones and
    // transient COW paths. Escaping state rows receive their own grants;
    // repeated ref moves must not retain every superseded batch's grant.
    let bound = encoded_bytes
        .checked_mul(20)
        .and_then(|n| n.checked_add(65_536))
        .ok_or_else(|| corrupt_history("History replay allocation bound overflow"))?;
    let _scratch_memory = budget
        .reserve_memory(bound, MemoryLifetime::Temporary)
        .map_err(|error| history_work_error(budget, error))?;
    budget
        .charge(WorkUsage {
            reads: u64::try_from(replaces)
                .ok()
                .and_then(|n| n.checked_add(u64::try_from(deletes).ok()?))
                .and_then(|n| n.checked_add(1))
                .ok_or_else(|| corrupt_history("History work count overflow"))?,
            mutations: u64::try_from(replaces)
                .ok()
                .and_then(|n| n.checked_add(u64::try_from(deletes).ok()?))
                .and_then(|n| n.checked_add(1))
                .ok_or_else(|| corrupt_history("History work count overflow"))?,
            ..WorkUsage::default()
        })
        .map_err(|error| history_work_error(budget, error))?;
    let epoch = AuxEpochV1::new(ordinal);
    let delta = history_transition(Some(state), epoch, batch)?;
    for mutation in history_delta_rows(&delta)?.rows {
        match mutation {
            AuxiliaryRowMutationV1::Upsert(row) => {
                let bytes = u64::try_from(row.value.len())
                    .ok()
                    .and_then(|bytes| bytes.checked_mul(16))
                    .and_then(|bytes| bytes.checked_add(u64::try_from(row.key.row_key.len()).ok()?))
                    .and_then(|bytes| bytes.checked_add(4096))
                    .ok_or_else(|| corrupt_history("History state allocation bound overflow"))?;
                let memory = budget
                    .reserve_memory(bytes, MemoryLifetime::Retained)
                    .map_err(|error| history_work_error(budget, error))?;
                custody.replace(row.key.family, row.key.row_key, memory);
            }
            AuxiliaryRowMutationV1::Delete(key) => custody.remove(key.family, key.row_key),
            _ => {
                return Err(corrupt_history(
                    "unexpected whole-family mutation in History replay",
                ));
            }
        }
    }
    state.apply_delta(&delta);
    // Only after the old records are destroyed may their grants retire.
    custody.settle();
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    Ok(())
}

fn history_snapshot_digest(
    generation: &AuxiliaryGenerationKeyV1,
    epoch: AuxEpochV1,
    state: &HistoryAuthorityState,
    budget: &mut WorkBudget,
) -> Result<Digest, CoreError> {
    let header_bytes = u64::try_from(generation.repo_id.as_str().len())
        .ok()
        .and_then(|bytes| {
            bytes.checked_add(u64::try_from(generation.revision_id.as_str().len()).ok()?)
        })
        .and_then(|bytes| bytes.checked_add(128))
        .ok_or_else(|| corrupt_history("History snapshot header work overflow"))?;
    budget
        .charge(WorkUsage {
            bytes: header_bytes,
            ..WorkUsage::default()
        })
        .map_err(|error| history_work_error(budget, error))?;
    let mut hash = HistorySnapshotDigestV1::new(generation);
    visit_history_state_rows(generation, epoch, state, budget, &mut |row, _| {
        hash.push_row(row.key.family, &row.key.row_key, &row.value);
        Ok(())
    })?;
    Ok(hash.finish())
}

pub(crate) fn visit_history_state_rows(
    generation: &AuxiliaryGenerationKeyV1,
    epoch: AuxEpochV1,
    state: &HistoryAuthorityState,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(&AuxiliaryRowV1, &mut WorkBudget) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    fn emit<T: Serialize>(
        generation: &AuxiliaryGenerationKeyV1,
        family: AuxiliaryRowFamilyV1,
        key: &[u8],
        value: &T,
        budget: &mut WorkBudget,
        visit: &mut dyn FnMut(&AuxiliaryRowV1, &mut WorkBudget) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let length = cbor_payload_len_with_progress_v1(value, &mut |bytes| {
            budget.charge(WorkUsage {
                bytes: u64::try_from(bytes)
                    .map_err(|_| quanta_index_core::index::IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })
        })
        .map_err(|error| match error {
            CborPayloadLengthProgressErrorV1::Progress(error) => history_work_error(budget, error),
            CborPayloadLengthProgressErrorV1::Codec(error) => CoreError::Storage(error.to_string()),
        })?;
        let identity_bytes = generation
            .repo_id
            .as_str()
            .len()
            .checked_add(generation.revision_id.as_str().len())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<AuxiliaryRowV1>() + 256))
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or_else(|| corrupt_history("History row identity bound overflow"))?;
        let bound = length
            .checked_mul(2)
            .and_then(|n| n.checked_add(u64::try_from(key.len()).ok()?))
            .and_then(|n| n.checked_add(identity_bytes))
            .ok_or_else(|| corrupt_history("History row allocation bound overflow"))?;
        let _memory = budget
            .reserve_memory(bound, MemoryLifetime::Temporary)
            .map_err(|error| history_work_error(budget, error))?;
        budget
            .charge(WorkUsage {
                reads: 1,
                bytes: length
                    .checked_add(
                        u64::try_from(family.as_code_str().len() + 24)
                            .map_err(|_| corrupt_history("History framing work overflow"))?,
                    )
                    .ok_or_else(|| corrupt_history("History framing work overflow"))?
                    .checked_add(
                        u64::try_from(key.len())
                            .map_err(|_| corrupt_history("History key work overflow"))?,
                    )
                    .ok_or_else(|| corrupt_history("History row work overflow"))?,
                ..WorkUsage::default()
            })
            .map_err(|error| history_work_error(budget, error))?;
        let row = AuxiliaryRowV1 {
            key: row_key(AuxiliaryDomainV1::History, generation, family, key.to_vec()),
            value: encode(family.as_code_str(), value)?,
        };
        visit(&row, budget)
    }
    // Epoch first, then the exact SQL `ORDER BY family,row_key` order. Enum
    // discriminant order is different and must not define this commitment.
    emit(
        generation,
        AuxiliaryRowFamilyV1::Epoch,
        &[],
        &epoch,
        budget,
        visit,
    )?;
    for (sha, record) in state.commits() {
        emit(
            generation,
            AuxiliaryRowFamilyV1::Commit,
            sha.as_bytes(),
            record,
            budget,
            visit,
        )?;
    }
    for (key, record) in state.diff_hunks() {
        let _memory = budget
            .reserve_memory(
                u64::try_from(COMMIT_SHA_LEN + 1 + key.file_path().len())
                    .map_err(|_| corrupt_history("History diff key overflow"))?,
                MemoryLifetime::Temporary,
            )
            .map_err(|error| history_work_error(budget, error))?;
        emit(
            generation,
            AuxiliaryRowFamilyV1::DiffHunk,
            &diff_key_bytes(key),
            record,
            budget,
            visit,
        )?;
    }
    for (name, sha) in state.refs() {
        emit(
            generation,
            AuxiliaryRowFamilyV1::Ref,
            name.as_bytes(),
            sha,
            budget,
            visit,
        )?;
    }
    emit(
        generation,
        AuxiliaryRowFamilyV1::StateMeta,
        &[],
        &state.meta(),
        budget,
        visit,
    )?;
    for (name, sha) in state.tags() {
        emit(
            generation,
            AuxiliaryRowFamilyV1::Tag,
            name.as_bytes(),
            sha,
            budget,
            visit,
        )?;
    }
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    Ok(())
}

pub(crate) fn history_work_error(budget: &WorkBudget, error: IndexError) -> CoreError {
    if let Err(interrupted) = budget.request().checkpoint("History source replay") {
        return interrupted;
    }
    match error {
        IndexError::ResourceLimited | IndexError::WorkLimitExceeded => CoreError::Typed {
            code: SearchPlaneErrorCodeV2::ServerOverloaded,
            message: "History source replay exceeds its admitted work".into(),
        },
        other => CoreError::Storage(format!("History source replay: {other}")),
    }
}

#[cfg(test)]
mod tests;
