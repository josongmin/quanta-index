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
    history_replay_advance_with_progress_v1, matches_batch_digest_token_v1,
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
    pub(crate) fn for_generation(
        repo: &RepoId,
        revision: &RevisionId,
        manifest: Option<&str>,
        budget: &WorkBudget,
    ) -> Result<Self, CoreError> {
        let bytes = repo
            .as_str()
            .len()
            .checked_add(revision.as_str().len())
            .and_then(|bytes| bytes.checked_add(manifest.map_or(0, str::len)))
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or_else(|| corrupt_history("History generation identity allocation overflow"))?;
        let generation_identity = budget
            .reserve_memory(bytes, MemoryLifetime::Retained)
            .map_err(|error| history_work_error(budget, error))?;
        Ok(Self {
            generation_identity,
            ..Self::default()
        })
    }

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
            return Err(corrupt_history(
                "Native History original contains no mutations",
            ));
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
            b"qidx.native.history.append.v1\0",
            &self.cut,
            operation,
            next_view_sequence,
            epoch,
            input,
            body.digest,
            snapshot,
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
        let manifest = batch
            .manifest_digest
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| corrupt_history("Native History successor manifest is absent"))?;
        if !self
            .cut
            .generation
            .is_next_generation_of_repo_v1(&batch.repo_id, batch.generation)
            || next_view_sequence == 0
        {
            return Err(corrupt_history(
                "Native History generation does not succeed the prior scope",
            ));
        }
        let identity = ReplayCustody::for_generation(
            &batch.repo_id,
            &batch.revision_id,
            Some(manifest),
            budget,
        )?;
        let generation = generation_key(&batch.repo_id, &batch.revision_id, batch.generation);
        let epoch = AuxEpochV1::new(1);
        let body = read_history_batch(batch, budget)?;
        if body.replaces == 0 && body.deletes == 0 {
            return Err(corrupt_history(
                "Native History original contains no mutations",
            ));
        }
        apply_history_body(
            &mut self.state,
            &mut self._custody,
            epoch.get(),
            batch,
            &body,
            budget,
        )?;
        let snapshot = history_snapshot_digest(&generation, epoch, &self.state, budget)?;
        budget
            .charge(WorkUsage {
                bytes: 384,
                ..WorkUsage::default()
            })
            .map_err(|error| history_work_error(budget, error))?;
        let replay = native_replay_position(
            b"qidx.native.history.generation.v1\0",
            &self.cut,
            operation,
            next_view_sequence,
            epoch,
            input,
            body.digest,
            snapshot,
        );
        budget
            .checkpoint()
            .map_err(|error| history_work_error(budget, error))?;
        self.cut = HistorySourceCutV1 {
            generation,
            durable_epoch: epoch,
            snapshot_digest: snapshot,
            replay_start: replay,
        };
        self.sequence = next_view_sequence;
        self.manifest_digest = Some(manifest.to_owned());
        self._custody.generation_identity = identity.generation_identity;
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
    domain: &[u8],
    prior: &HistorySourceCutV1,
    operation: OperationKey,
    view_sequence: u64,
    epoch: AuxEpochV1,
    input: ObjectRef,
    body: [u8; 32],
    snapshot: Digest,
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

    /// The enclosing source validator authenticates the inventory and its
    /// starting position. The fold binds every advance to the original batch
    /// and ACK before applying the History transition.
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
            || batch.repo_id != self.fence.generation.repo_id
            || batch.revision_id != self.fence.generation.revision_id
            || batch.generation != self.fence.generation.generation
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
        let expected =
            history_replay_advance_with_progress_v1(before, ordinal, batch, ack, &mut |bytes| {
                budget.charge(WorkUsage {
                    bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })
            })
            .map_err(|error| match error {
                CanonicalBatchDigestProgressError::Progress(error) => {
                    history_work_error(budget, error)
                }
                CanonicalBatchDigestProgressError::Encode(error) => {
                    CoreError::Storage(error.to_string())
                }
            })?;
        if expected != after {
            return Err(corrupt_history(
                "History replay advance differs from the original batch and ACK",
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

    pub fn finish(mut self, budget: &mut WorkBudget) -> Result<ReplayedHistorySourceV1, CoreError> {
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
        // A resumed fold already owns the exact same generation identity.
        // A fresh fold must fund its identity before it leaves the input loan.
        if self.custody.generation_identity.is_none() {
            let identity = ReplayCustody::for_generation(
                &self.fence.generation.repo_id,
                &self.fence.generation.revision_id,
                self.fence.manifest_digest.as_deref(),
                budget,
            )?;
            self.custody.generation_identity = identity.generation_identity;
        }
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
    replaces: usize,
    deletes: usize,
}

fn read_history_batch(
    batch: &HistoryIngestBatch,
    budget: &mut WorkBudget,
) -> Result<HistoryBatchFacts, CoreError> {
    let digest = canonical_batch_digest_with_progress_v1(batch, &mut |bytes| {
        let bytes = u64::try_from(bytes)
            .map_err(|_| quanta_index_core::index::IndexError::ResourceLimited)?;
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
    let count = body
        .replaces
        .checked_add(body.deletes)
        .and_then(|count| count.checked_add(1))
        .and_then(|count| u64::try_from(count).ok())
        .ok_or_else(|| corrupt_history("History work count overflow"))?;
    budget
        .charge(WorkUsage {
            reads: count,
            mutations: count,
            ..WorkUsage::default()
        })
        .map_err(|error| history_work_error(budget, error))?;
    let epoch = AuxEpochV1::new(ordinal);
    let generation_bytes = batch
        .repo_id
        .as_str()
        .len()
        .checked_add(batch.revision_id.as_str().len())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(|| corrupt_history("History validation allocation bound overflow"))?;
    let _generation = budget
        .reserve_memory(generation_bytes, MemoryLifetime::Temporary)
        .map_err(|error| history_work_error(budget, error))?;
    let delta = {
        let longest_ref = batch
            .refs
            .iter()
            .chain(&batch.tags)
            .filter_map(|mutation| match mutation {
                HistoryRefMutation::Upsert(update) => Some(update.name.len()),
                HistoryRefMutation::Delete(_) => None,
            })
            .max()
            .unwrap_or(0);
        // The validation set and a rejection diagnostic are the only
        // scratch allocations. The validated delta borrows the records.
        let scratch = batch
            .commits
            .len()
            .checked_mul(128)
            .and_then(|bytes| bytes.checked_add(longest_ref.checked_mul(2)?))
            .and_then(|bytes| bytes.checked_add(4096))
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or_else(|| corrupt_history("History validation allocation bound overflow"))?;
        let _validation = budget
            .reserve_memory(scratch, MemoryLifetime::Temporary)
            .map_err(|error| history_work_error(budget, error))?;
        history_transition(Some(state), epoch, batch)?
    };
    admit_history_delta(&delta, custody, budget)?;
    state.apply_delta(&delta);
    // Only after the old records are destroyed may their grants retire.
    custody.settle();
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    Ok(())
}

fn admit_history_delta(
    delta: &HistoryDelta<'_>,
    custody: &mut ReplayCustody,
    budget: &WorkBudget,
) -> Result<(), CoreError> {
    for record in delta.commits {
        let mut bytes = StateRowBacking::new(COMMIT_SHA_LEN)?;
        for text in [
            Some(record.author.as_ref()),
            record.author_name.as_deref(),
            record.author_email.as_deref(),
            Some(record.committer.as_ref()),
            record.committer_name.as_deref(),
            record.committer_email.as_deref(),
            Some(record.message.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            bytes.add(text.len())?;
        }
        bytes.add_items::<CommitSha>(record.parents.len())?;
        bytes.add_items::<Box<str>>(record.tags.len())?;
        for tag in &record.tags {
            bytes.add(tag.len())?;
        }
        let memory = bytes.reserve(budget)?;
        custody.replace(
            AuxiliaryRowFamilyV1::Commit,
            record.sha.as_bytes().to_vec(),
            memory,
        );
    }
    for (family, changes) in [
        (AuxiliaryRowFamilyV1::Ref, delta.refs),
        (AuxiliaryRowFamilyV1::Tag, delta.tags),
    ] {
        for change in changes {
            match change {
                HistoryRefMutation::Upsert(update) => {
                    let memory = StateRowBacking::new(update.name.len())?.reserve(budget)?;
                    custody.replace(family, update.name.as_bytes().to_vec(), memory);
                }
                HistoryRefMutation::Delete(update) => {
                    let bytes = u64::try_from(update.name.len())
                        .map_err(|_| corrupt_history("History state allocation bound overflow"))?;
                    let _key = if bytes == 0 {
                        None
                    } else {
                        budget
                            .reserve_memory(bytes, MemoryLifetime::Temporary)
                            .map_err(|error| history_work_error(budget, error))?
                    };
                    custody.remove(family, update.name.as_bytes().to_vec());
                }
            }
        }
    }
    for hunk in delta.diff_hunks {
        let key_bytes = hunk
            .file_path
            .len()
            .checked_add(COMMIT_SHA_LEN + 1)
            .ok_or_else(|| corrupt_history("History state allocation bound overflow"))?;
        let mut bytes = StateRowBacking::new(key_bytes)?;
        for text in [
            &hunk.record.hunk_header,
            &hunk.record.added_text,
            &hunk.record.removed_text,
            &hunk.record.touched_text,
        ] {
            bytes.add(text.len())?;
        }
        let memory = bytes.reserve(budget)?;
        custody.replace(
            AuxiliaryRowFamilyV1::DiffHunk,
            history_diff_row_key(hunk.commit_sha, &hunk.file_path),
            memory,
        );
    }
    Ok(())
}

struct StateRowBacking(usize);

impl StateRowBacking {
    fn new(key_bytes: usize) -> Result<Self, CoreError> {
        // Typed key, custody key and a tree separator, plus inline values and
        // container nodes. Record payloads are copied exactly once below.
        key_bytes
            .checked_mul(3)
            .and_then(|bytes| bytes.checked_add(4096))
            .map(Self)
            .ok_or_else(|| corrupt_history("History state allocation bound overflow"))
    }

    fn add(&mut self, bytes: usize) -> Result<(), CoreError> {
        self.0 = self
            .0
            .checked_add(bytes)
            .ok_or_else(|| corrupt_history("History state allocation bound overflow"))?;
        Ok(())
    }

    fn add_items<T>(&mut self, count: usize) -> Result<(), CoreError> {
        self.add(
            count
                .checked_mul(std::mem::size_of::<T>())
                .ok_or_else(|| corrupt_history("History state allocation bound overflow"))?,
        )
    }

    fn reserve(self, budget: &WorkBudget) -> Result<Option<MemoryLease>, CoreError> {
        let bytes = u64::try_from(self.0)
            .map_err(|_| corrupt_history("History state allocation bound overflow"))?;
        budget
            .reserve_memory(bytes, MemoryLifetime::Retained)
            .map_err(|error| history_work_error(budget, error))
    }
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
