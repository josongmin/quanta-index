//! Source carrier -> replay -> derived History member under the original attempt.
//! Profile bootstrap, view CAS and publication remain Kernel responsibilities.
use super::*;
use crate::HistorySourceReplayV1;
use quanta_index_contract::index::{
    HistoryAppendV1, HistoryGenerationAdvanceV1, HistorySnapshotCheckpointV1, HistorySnapshotPageV1, HistorySourceInputV1,
    HistoryTransition, InstanceSpec, MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1,
    MAX_HISTORY_SOURCE_INPUT_BYTES_V1,
};
use quanta_index_core::index::{StoredObject, effort::ResourceNeed, ports::IndexHistoryBuildPort};
use serde::Deserialize;

pub struct NativeHistoryBuildV1 {
    demand: ResourceNeed,
}
impl NativeHistoryBuildV1 {
    pub fn new(demand: ResourceNeed) -> Result<Self, IndexError> {
        if demand.provider_units != 0 || demand.cpu_units == 0 || demand.memory_bytes == 0 {
            return Err(IndexError::InvalidInput(
                "History build needs admitted CPU/memory and no provider".into(),
            ));
        }
        Ok(Self { demand })
    }
}
impl IndexHistoryBuildPort for NativeHistoryBuildV1 {
    fn resource_demand(&self) -> ResourceNeed {
        self.demand
    }
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        super::validate_instance(instance)
            .map_err(|error| IndexError::InvalidInput(error.to_string()))
    }
    fn verify_history(
        &self,
        instance: &InstanceSpec,
        prior: Option<&MemberBinding>,
        transition: &HistoryTransition,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError> {
        verify(instance, prior, transition, context)
            .map_err(|error| index_error(error, context.budget))
    }
    fn append_history(
        &self,
        instance: &InstanceSpec,
        prior: &MemberBinding,
        append: &HistoryAppendV1,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError> {
        append_native(instance, prior, append, next_view_sequence, context)
            .map_err(|error| index_error(error, context.budget))
    }

    fn advance_history_generation(
        &self,
        instance: &InstanceSpec,
        prior: &MemberBinding,
        advance: &HistoryGenerationAdvanceV1,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError> {
        advance_generation(instance, prior, advance, next_view_sequence, context)
            .map_err(|error| index_error(error, context.budget))
    }
}

pub(super) struct SourceInputData<T> {
    pub(super) value: T,
    _memory: Option<MemoryLease>,
}

/// A borrowed boot source. Snapshot DATA remains provisional until the
/// independent Legacy Catalog and producer scope verify it before selection.
pub enum NativeHistoryBootSourceV1<'a> {
    Replay(&'a HistorySourceInputV1),
    Snapshot(&'a HistorySnapshotCheckpointV1),
}

enum LoadedBootSourceV1 {
    Replay(SourceInputData<HistorySourceInputV1>),
    Snapshot(SourceInputData<HistorySnapshotCheckpointV1>),
}

#[derive(Deserialize)]
struct BootSourceVersionV1 {
    wire_version: u32,
}

fn decode_boot_source_v1(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<LoadedBootSourceV1, CoreError> {
    if stored.registration.reference != reference
        || stored.payload.len() > MAX_HISTORY_SOURCE_INPUT_BYTES_V1
    {
        return Err(invalid(
            "History boot source identity or encoded length is invalid",
        ));
    }
    budget
        .charge(WorkUsage {
            bytes: u64::try_from(stored.payload.len())
                .map_err(|_| invalid("History boot source length exceeds u64"))?,
            ..WorkUsage::default()
        })
        .map_err(|error| history_work_error(budget, error))?;
    let version: BootSourceVersionV1 =
        serde_json::from_slice(&stored.payload).map_err(|error| invalid(&error.to_string()))?;
    match version.wire_version {
        1 => {
            let input = decode_history_object::<HistorySourceInputV1>(reference, stored, budget)?;
            input.value.validate_shape().map_err(invalid)?;
            Ok(LoadedBootSourceV1::Replay(input))
        }
        2 => {
            let input: SourceInputData<HistorySnapshotCheckpointV1> =
                decode_history_object_allow_children(reference, stored, budget)?;
            input.value.validate_shape().map_err(invalid)?;
            validate_checkpoint_children_v1(&input.value, stored, budget)?;
            Ok(LoadedBootSourceV1::Snapshot(input))
        }
        _ => Err(invalid("History boot source version is invalid")),
    }
}

fn validate_checkpoint_children_v1(
    checkpoint: &HistorySnapshotCheckpointV1,
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<(), CoreError> {
    let bytes = u64::try_from(checkpoint.pages.len())
        .ok()
        .and_then(|count| count.checked_mul(std::mem::size_of::<ObjectRef>() as u64))
        .ok_or_else(|| invalid("History page reference allocation overflow"))?;
    budget
        .charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })
        .map_err(|error| history_work_error(budget, error))?;
    let _allocation = budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)
        .map_err(|error| history_work_error(budget, error))?;
    let mut sorted = checkpoint.pages.clone();
    sorted.sort_unstable();
    if sorted.windows(2).any(|pair| pair[0] == pair[1])
        || stored.registration.children.as_slice() != sorted.as_slice()
    {
        return Err(invalid(
            "History checkpoint page refs differ from exact immutable children",
        ));
    }
    Ok(())
}

/// Lend the sealed page stream without retaining earlier pages or cloning row values.
pub(super) fn visit_snapshot_checkpoint_rows_v1(
    checkpoint: &HistorySnapshotCheckpointV1,
    mut get: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<StoredObject, IndexError>,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(AuxiliaryRowV1, &mut WorkBudget) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let mut count = 0_u64;
    for (index, reference) in checkpoint.pages.iter().copied().enumerate() {
        budget.checkpoint().map_err(|error| history_work_error(budget, error))?;
        let stored = get(reference, budget).map_err(|error| history_work_error(budget, error))?;
        if stored.payload.len() > MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1 {
            return Err(invalid("History snapshot page encoded length exceeds bound"));
        }
        let page: SourceInputData<HistorySnapshotPageV1> =
            decode_history_object(reference, &stored, budget)?;
        page.value.validate_shape().map_err(invalid)?;
        if page.value.generation != checkpoint.cut.generation
            || usize::try_from(page.value.ordinal).ok() != Some(index + 1)
        {
            return Err(invalid("History snapshot page identity or ordinal differs"));
        }
        let SourceInputData { value: page, _memory: _page_memory } = page;
        drop(stored);
        for row in page.rows {
            budget.checkpoint().map_err(|error| history_work_error(budget, error))?;
            let family = AuxiliaryRowFamilyV1::from_code_str(&row.family)
                .ok_or_else(|| invalid("History snapshot row family invalid"))?;
            visit(
                AuxiliaryRowV1 {
                    key: quanta_index_core::AuxiliaryRowKeyV1 {
                        domain: quanta_index_core::AuxiliaryDomainV1::History,
                        generation: page.generation.clone(),
                        family,
                        row_key: row.row_key,
                    },
                    value: row.value,
                },
                budget,
            )?;
            count = count.checked_add(1).ok_or_else(|| invalid("History snapshot row count overflow"))?;
        }
    }
    if count != checkpoint.row_count {
        return Err(invalid("History snapshot page rows differ from declared count"));
    }
    Ok(())
}

/// Lend the exact sealed source object to BOOT without mistaking a provisional
/// snapshot for retained-original replay. Both branches keep decode custody.
pub fn with_native_history_boot_source_v1<T>(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
    verify: impl FnOnce(NativeHistoryBootSourceV1<'_>, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let source = decode_boot_source_v1(reference, stored, budget)
        .map_err(|error| index_error(error, budget))?;
    match &source {
        LoadedBootSourceV1::Replay(input) => {
            verify(NativeHistoryBootSourceV1::Replay(&input.value), budget)
        }
        LoadedBootSourceV1::Snapshot(input) => {
            verify(NativeHistoryBootSourceV1::Snapshot(&input.value), budget)
        }
    }
}
fn load_source(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<SourceInputData<HistorySourceInputV1>, CoreError> {
    let result = load_history_object::<HistorySourceInputV1>(reference, context)?;
    result.value.validate_shape().map_err(invalid)?;
    Ok(result)
}

/// Lend the same canonical History source carrier to offline read-only
/// verification, with its decode allocation retained for the entire loan.
pub fn with_native_history_source_input_v1<T>(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
    verify: impl FnOnce(&HistorySourceInputV1, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let input = decode_history_object::<HistorySourceInputV1>(reference, stored, budget)
        .map_err(|error| index_error(error, budget))?;
    input
        .value
        .validate_shape()
        .map_err(|error| index_error(invalid(error), budget))?;
    verify(&input.value, budget)
}

pub(super) fn load_history_object<T: serde::de::DeserializeOwned>(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<SourceInputData<T>, CoreError> {
    let stored = context
        .objects
        .get(reference, context.budget)
        .map_err(|error| history_work_error(context.budget, error))?;
    decode_history_object(reference, &stored, context.budget)
}

fn decode_history_object<T: serde::de::DeserializeOwned>(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<SourceInputData<T>, CoreError> {
    decode_history_object_inner(reference, stored, budget, false)
}

pub(super) fn decode_history_object_allow_children<T: serde::de::DeserializeOwned>(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<SourceInputData<T>, CoreError> {
    decode_history_object_inner(reference, stored, budget, true)
}

fn decode_history_object_inner<T: serde::de::DeserializeOwned>(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
    allow_children: bool,
) -> Result<SourceInputData<T>, CoreError> {
    if stored.registration.reference != reference
        || (!allow_children && !stored.registration.children.is_empty())
        || stored.payload.len() > MAX_HISTORY_SOURCE_INPUT_BYTES_V1
    {
        return Err(invalid(
            "History source payload identity, child set or encoded length is invalid",
        ));
    }
    let bytes = u64::try_from(stored.payload.len())
        .ok()
        .and_then(|bytes| bytes.checked_mul(16))
        .and_then(|bytes| bytes.checked_add(4096))
        .ok_or_else(|| invalid("History source decode bound overflow"))?;
    let memory = budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)
        .map_err(|error| history_work_error(budget, error))?;
    // Keep decode work on the original attempt and observe refusal between
    // bounded reads. The backing buffer is covered by the scratch grant above.
    struct SourceReader<'a> {
        bytes: &'a [u8],
        budget: &'a mut WorkBudget,
        refusal: Option<IndexError>,
    }
    impl std::io::Read for SourceReader<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let count = output.len().min(self.bytes.len());
            if let Err(error) = self.budget.charge(WorkUsage {
                bytes: count as u64,
                ..WorkUsage::default()
            }) {
                self.refusal = Some(error);
                return Err(std::io::Error::other("History source decode work refused"));
            }
            output[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            Ok(count)
        }
    }
    let mut reader = std::io::BufReader::with_capacity(
        4096,
        SourceReader {
            bytes: &stored.payload,
            budget,
            refusal: None,
        },
    );
    let decoded = serde_json::from_reader(&mut reader);
    let refusal = reader.get_mut().refusal.take();
    drop(reader);
    if let Some(error) = refusal {
        return Err(history_work_error(budget, error));
    }
    let value = decoded.map_err(|error| invalid(&error.to_string()))?;
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    Ok(SourceInputData {
        value,
        _memory: memory,
    })
}
fn verify(
    instance: &InstanceSpec,
    prior: Option<&MemberBinding>,
    transition: &HistoryTransition,
    context: &mut AttemptContext<'_>,
) -> Result<MemberBinding, CoreError> {
    super::validate_instance(instance)?;
    context
        .budget
        .checkpoint()
        .map_err(|error| history_work_error(context.budget, error))?;
    if transition.instance != instance.instance {
        return Err(invalid(
            "History transition instance differs from selected profile",
        ));
    }
    if prior.is_some() != transition.expected.is_some() {
        return Err(invalid(
            "History FULL/DELTA prior member and expected cut disagree",
        ));
    }
    let prepared = match (prior, transition.expected.as_ref()) {
        (None, None) => {
            let stored = context
                .objects
                .get(transition.input, context.budget)
                .map_err(|error| history_work_error(context.budget, error))?;
            match decode_boot_source_v1(transition.input, &stored, context.budget)? {
                LoadedBootSourceV1::Replay(input) => {
                    if input.value.fence.generation != transition.next.generation
                        || input.value.fence.publication_count
                            != transition.next.durable_epoch.get()
                        || input.value.fence.final_replay != transition.next.replay_start
                    {
                        return Err(invalid(
                            "History source fence differs from requested next cut",
                        ));
                    }
                    let mut replay = HistorySourceReplayV1::new(input.value.fence.clone())?;
                    for publication in &input.value.publications {
                        replay = replay.apply(
                            publication.ordinal,
                            publication.before,
                            publication.after,
                            &publication.batch,
                            &publication.ack,
                            context.budget,
                        )?;
                    }
                    let source = replay.finish(context.budget)?;
                    if source.cut() != &transition.next {
                        return Err(invalid(
                            "History FULL source rows differ from requested complete cut",
                        ));
                    }
                    prepare_native_history_full_v1(&source, transition.input, context)?
                }
                LoadedBootSourceV1::Snapshot(checkpoint) => {
                    if checkpoint.value.cut != transition.next {
                        return Err(invalid("History snapshot cut differs from requested FULL"));
                    }
                    let objects = context.objects;
                    let manifest = checkpoint.value.terminal_ack.manifest_digest.clone();
                    prepare_native_history_snapshot_full_v1(
                        &checkpoint.value.cut,
                        checkpoint.value.terminal_ack.durable_sequence,
                        manifest,
                        transition.input,
                        |budget, visit| {
                            visit_snapshot_checkpoint_rows_v1(
                                &checkpoint.value,
                                |reference, budget| objects.get(reference, budget),
                                budget,
                                visit,
                            )
                        },
                        context,
                    )?
                }
            }
        }
        (Some(prior), Some(expected)) => {
            let input = load_source(transition.input, context)?;
            if input.value.fence.generation != transition.next.generation
                || input.value.fence.publication_count != transition.next.durable_epoch.get()
                || input.value.fence.final_replay != transition.next.replay_start
            {
                return Err(invalid(
                    "History source fence differs from requested next cut",
                ));
            }
            let old = open_manifest(prior.artifact, context.objects, context.budget)?;
            validate_member(prior, instance, &old.manifest)?;
            if !matches!(
                old.manifest.format,
                REPLAY_HISTORY_MANIFEST | SNAPSHOT_HISTORY_MANIFEST
            ) || old.manifest.cut() != *expected
                || old.manifest.epoch.checked_next() != Some(transition.next.durable_epoch)
                || input.value.fence.first_replay != expected.replay_start
                || input.value.publications.len() != 1
            {
                return Err(invalid(
                    "History DELTA does not continue the actual committed prior cut",
                ));
            }
            let publication = input
                .value
                .publications
                .first()
                .ok_or_else(|| invalid("History DELTA publication absent"))?;
            if publication.ordinal != input.value.fence.publication_count
                || publication.before != input.value.fence.first_replay
                || publication.after != input.value.fence.final_replay
            {
                return Err(invalid(
                    "History DELTA original publication ordinal/replay positions differ",
                ));
            }
            let source = load_rows(&old.manifest, context.objects, context.maps, context.budget)?;
            let source = replay_delta_source(
                source,
                input.value.fence.clone(),
                &publication.batch,
                &publication.ack,
                context.budget,
            )?;
            if source.cut() != &transition.next {
                return Err(invalid(
                    "History DELTA source body/ACK does not produce the requested next rows",
                ));
            }
            prepare(
                &source,
                Some((&old.manifest, &publication.batch)),
                transition.input,
                REPLAY_HISTORY_MANIFEST,
                context,
            )?
        }
        _ => return Err(invalid("History FULL/DELTA mode is invalid")),
    };
    if prepared.manifest.predecessor.as_ref() != transition.expected.as_ref()
        || prepared.manifest.source_input != transition.input
    {
        return Err(invalid(
            "History derived artifact lost its original source/prior binding",
        ));
    }
    // Original source DATA and replayed state do not escape. Retire them before
    // checking the derived storage, avoiding two simultaneous whole snapshots.
    verify_native_history_input_v1(prepared.artifact(), &transition.next, instance, context)
}

fn append_native(
    instance: &InstanceSpec,
    prior: &MemberBinding,
    append: &HistoryAppendV1,
    next_view_sequence: u64,
    context: &mut AttemptContext<'_>,
) -> Result<MemberBinding, CoreError> {
    super::validate_instance(instance)?;
    let old = open_manifest(prior.artifact, context.objects, context.budget)?;
    validate_member(prior, instance, &old.manifest)?;
    if append.instance != instance.instance
        || old.manifest.cut() != append.expected
        || (matches!(old.manifest.format, APPEND_HISTORY_MANIFEST | SUCCESSOR_HISTORY_MANIFEST)
            && old.manifest.sequence >= next_view_sequence)
    {
        return Err(invalid(
            "Native History original does not continue the exact committed member",
        ));
    }
    let input = load_history_object::<HistoryIngestBatch>(append.input, context)?;
    let source = load_rows(&old.manifest, context.objects, context.maps, context.budget)?;
    let updated = source.append_native(
        context.attempt.operation,
        next_view_sequence,
        append.input,
        &input.value,
        context.budget,
    )?;
    let prepared = prepare(
        &updated,
        Some((&old.manifest, &input.value)),
        append.input,
        APPEND_HISTORY_MANIFEST,
        context,
    )?;
    drop(updated);
    drop(input);
    let _readback = load_rows(
        &prepared.manifest,
        context.objects,
        context.maps,
        context.budget,
    )?;
    prepared.into_member(instance)
}

fn advance_generation(
    instance: &InstanceSpec,
    prior: &MemberBinding,
    advance: &HistoryGenerationAdvanceV1,
    next_view_sequence: u64,
    context: &mut AttemptContext<'_>,
) -> Result<MemberBinding, CoreError> {
    super::validate_instance(instance)?;
    let old = open_manifest(prior.artifact, context.objects, context.budget)?;
    let prior_spec = InstanceSpec { instance: advance.prior_instance, ..instance.clone() };
    validate_member(prior, &prior_spec, &old.manifest)?;
    if advance.instance != instance.instance
        || old.manifest.cut() != advance.expected
        || (matches!(old.manifest.format, APPEND_HISTORY_MANIFEST | SUCCESSOR_HISTORY_MANIFEST)
            && old.manifest.sequence >= next_view_sequence)
    {
        return Err(invalid("Native History generation does not succeed the exact committed member"));
    }
    let input = super::generation_input::load_verified(advance.input, context)?;
    let base = load_rows(&old.manifest, context.objects, context.maps, context.budget)?;
    let source = base.advance_native_generation(
        context.attempt.operation,
        next_view_sequence,
        advance.input,
        &input.value,
        context.budget,
    )?;
    let prepared = prepare(
        &source,
        Some((&old.manifest, &input.value)),
        advance.input,
        SUCCESSOR_HISTORY_MANIFEST,
        context,
    )?;
    drop(source);
    drop(input);
    let _readback = load_rows(&prepared.manifest, context.objects, context.maps, context.budget)?;
    prepared.into_member(instance)
}

#[cfg(test)]
mod snapshot_checkpoint_tests {
    use super::*;
    use quanta_index_contract::{
        batch_digest_token_v1,
        index::{native_immutable_object_ref_v1, Digest, HistorySnapshotRowV1, IndexInstanceId},
    };
    use quanta_index_core::index::{MapValue, RegisteredObject, WorkLimits};
    use std::time::{Duration, Instant};

    fn budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                mutations: 100,
                bytes: 10_000_000,
            },
            Instant::now() + Duration::from_secs(60),
        )
    }

    fn checkpoint() -> HistorySnapshotCheckpointV1 {
        let generation = ManifestGeneration::new(2);
        HistorySnapshotCheckpointV1 {
            wire_version: 2,
            cut: HistorySourceCutV1 {
                generation: quanta_index_core::AuxiliaryGenerationKeyV1 {
                    repo_id: RepoId::new("repo-history-snapshot").expect("fixture repo"),
                    revision_id: RevisionId::new("rev-history-snapshot").expect("fixture revision"),
                    generation,
                },
                durable_epoch: AuxEpochV1::new(2),
                snapshot_digest: Digest([3; 32]),
                replay_start: Digest([4; 32]),
            },
            terminal_ack: BatchPublishReceipt::empty_for(
                generation,
                Some("manifest".into()),
                batch_digest_token_v1(&[5; 32]),
            )
            .recorded_at(7),
            row_count: 2,
            // Page order is physical SQL order; registration children are sorted.
            pages: vec![ObjectRef(Digest([9; 32])), ObjectRef(Digest([8; 32]))],
        }
    }

    fn stored(checkpoint: &HistorySnapshotCheckpointV1, children: Vec<ObjectRef>) -> StoredObject {
        let payload = serde_json::to_vec(checkpoint).expect("fixture serialization");
        StoredObject::new(
            RegisteredObject::unscoped(ObjectRef(Digest([7; 32])), payload.len() as u64, children),
            payload,
            None,
        )
    }

    #[test]
    fn native_history_snapshot_checkpoint_requires_exact_page_closure_and_terminal_ack() {
        let checkpoint = checkpoint();
        let root = ObjectRef(Digest([7; 32]));
        let children = vec![checkpoint.pages[1], checkpoint.pages[0]];
        let valid = stored(&checkpoint, children.clone());
        assert!(matches!(
            decode_boot_source_v1(root, &valid, &mut budget()),
            Ok(LoadedBootSourceV1::Snapshot(_))
        ));

        let mut extra = children.clone();
        extra.push(ObjectRef(Digest([10; 32])));
        assert!(decode_boot_source_v1(root, &stored(&checkpoint, extra), &mut budget()).is_err());

        let mut duplicated = checkpoint.clone();
        duplicated.pages[1] = duplicated.pages[0];
        assert!(
            decode_boot_source_v1(
                root,
                &stored(&duplicated, vec![duplicated.pages[0]]),
                &mut budget()
            )
            .is_err()
        );

        let mut unrecorded = checkpoint;
        unrecorded.terminal_ack.durable_sequence = 0;
        assert!(
            decode_boot_source_v1(root, &stored(&unrecorded, children), &mut budget()).is_err()
        );
    }

    #[test]
    fn native_history_boot_rejects_content_addressed_rows_with_wrong_snapshot_digest() {
        let generation = quanta_index_core::AuxiliaryGenerationKeyV1 {
            repo_id: RepoId::new("repo-history-boot-rows").expect("fixture repo"),
            revision_id: RevisionId::new("rev-history-boot-rows").expect("fixture revision"),
            generation: ManifestGeneration::new(2),
        };
        let epoch = AuxEpochV1::new(2);
        let mut work = budget();
        let mut rows = Vec::new();
        crate::auxiliary_authority::history_source::visit_history_state_rows(
            &generation,
            epoch,
            &HistoryAuthorityState::default(),
            &mut work,
            &mut |row, _| {
                rows.push(row.clone());
                Ok(())
            },
        )
        .expect("canonical empty History snapshot rows");
        assert_eq!(rows.len(), 2);
        let mut digest = HistorySnapshotDigestV1::new(&generation);
        let mut maximum_row_bytes = 0;
        let mut entries = Vec::new();
        let mut objects = Vec::new();
        for row in &rows {
            digest.push_row(row.key.family, &row.key.row_key, &row.value);
            maximum_row_bytes = maximum_row_bytes.max(row.key.row_key.len() + row.value.len());
            let data = if row.key.family == AuxiliaryRowFamilyV1::Epoch {
                NativeRowData::Epoch(epoch)
            } else {
                NativeRowData::Row(decode_history_row(row).expect("canonical History metadata"))
            };
            let payload = serde_json::to_vec(&NativeRowRef {
                key: &row.key.row_key,
                data: &data,
            })
            .expect("typed Native History row");
            let reference = native_immutable_object_ref_v1(&payload, &[])
                .expect("content-addressed Native History row");
            entries.push((native_key(row.key.family, &row.key.row_key), MapValue::Object(reference)));
            objects.push(StoredObject::new(
                RegisteredObject::unscoped(reference, payload.len() as u64, Vec::new()),
                payload,
                None,
            ));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let snapshot = digest.finish();
        let row_root = MapRoot(Some(
            native_immutable_object_ref_v1(b"fixture History row map", &[])
                .expect("content-addressed fixture row root"),
        ));
        let mut source_page = HistorySnapshotPageV1 {
            wire_version: 1,
            generation: generation.clone(),
            ordinal: 1,
            rows: rows
                .iter()
                .map(|row| HistorySnapshotRowV1 {
                    family: row.key.family.as_code_str().into(),
                    row_key: row.key.row_key.clone(),
                    value: row.value.clone(),
                })
                .collect(),
        };
        let mut manifest = Manifest {
            format: SNAPSHOT_HISTORY_MANIFEST,
            repo: generation.repo_id.clone(),
            revision: generation.revision_id.clone(),
            generation: generation.generation,
            epoch,
            snapshot,
            replay: Digest([4; 32]),
            sequence: 7,
            source_manifest: None,
            predecessor: None,
            source_input: ObjectRef(Digest([0; 32])),
            rows: row_root,
            row_count: rows.len() as u64,
            maximum_row_bytes: maximum_row_bytes as u64,
        };
        let cut = manifest.cut();
        let verify = |manifest: &mut Manifest,
                      page: &HistorySnapshotPageV1,
                      entries: &[(Vec<u8>, MapValue)],
                      objects: &[StoredObject]| {
            let mut work = budget();
            let page_payload = serde_json::to_vec(page).expect("History fixture page JSON");
            let page_ref = native_immutable_object_ref_v1(&page_payload, &[])
                .expect("content-addressed fixture snapshot page");
            let page = StoredObject::new(
                RegisteredObject::unscoped(page_ref, page_payload.len() as u64, Vec::new()),
                page_payload,
                None,
            );
            let mut original = checkpoint();
            original.cut = cut.clone();
            original.terminal_ack.manifest_digest = None;
            original.pages = vec![page_ref];
            original.validate_shape().expect("complete fixture snapshot checkpoint");
            let source_payload = serde_json::to_vec(&original).expect("History checkpoint JSON");
            let source_input = native_immutable_object_ref_v1(&source_payload, &[page_ref])
                .expect("content-addressed fixture source input");
            let source = RegisteredObject::unscoped(
                source_input, source_payload.len() as u64, vec![page_ref],
            );
            manifest.source_input = source_input;
            let payload = serde_json::to_vec(manifest).expect("History manifest JSON");
            let children = vec![row_root.0.expect("row root"), source_input];
            let artifact = native_immutable_object_ref_v1(&payload, &children)
                .expect("content-addressed History manifest");
            let stored_manifest = StoredObject::new(
                RegisteredObject::unscoped(artifact, payload.len() as u64, children),
                payload,
                None,
            );
            let member = MemberBinding {
                instance: IndexInstanceId([7; 16]),
                format: NATIVE_HISTORY_FORMAT_V1,
                artifact,
                input: InputBinding {
                    root: row_root,
                    schema: NATIVE_HISTORY_SCHEMA_V1,
                    policy: native_history_policy_v1(),
                    context: cut.input_context_v1().expect("History cut context"),
                },
                logical_commitment: snapshot,
            };
            let prepared = verify_native_history_boot_member_v1(
                &member,
                &stored_manifest,
                &cut,
                &source,
                &NativeHistoryBootSourceV1::Snapshot(&original),
                |reference, _| {
                    assert_eq!(reference, page_ref);
                    Ok(page.clone())
                },
                &mut work,
            )?;
            prepared.verify_boot_rows_v1(
                |reference, _| {
                    objects
                        .iter()
                        .find(|stored| stored.registration.reference == reference)
                        .cloned()
                        .ok_or(IndexError::SnapshotUnavailable)
                },
                |root, after, limit, _| {
                    assert_eq!(root, row_root);
                    assert!(after.is_none());
                    assert_eq!(limit, 64);
                    Ok(MapPage::new(entries.to_vec(), false, Vec::new(), None))
                },
                &mut work,
            )
        };
        let _artifact = verify(&mut manifest, &source_page, &entries, &objects)
            .expect("canonical Native History rows match the cut");

        manifest.format = REPLAY_HISTORY_MANIFEST;
        assert!(matches!(
            verify(&mut manifest, &source_page, &entries, &objects),
            Err(IndexError::InvalidInput(message)) if message.contains("History source input child graph differs")
        ));
        manifest.format = SNAPSHOT_HISTORY_MANIFEST;
        manifest.sequence += 1;
        assert!(matches!(
            verify(&mut manifest, &source_page, &entries, &objects),
            Err(IndexError::DigestConflict)
        ));
        manifest.sequence -= 1;
        manifest.source_manifest = Some("different-source-manifest".into());
        assert!(matches!(
            verify(&mut manifest, &source_page, &entries, &objects),
            Err(IndexError::DigestConflict)
        ));
        manifest.source_manifest = None;

        let changed_epoch = quanta_index_ipc::encode_cbor_payload(&AuxEpochV1::new(3))
            .expect("different well-formed History epoch row");
        let original_epoch = std::mem::replace(&mut source_page.rows[0].value, changed_epoch);
        assert!(matches!(
            verify(&mut manifest, &source_page, &entries, &objects),
            Err(IndexError::InvalidInput(message)) if message.contains("History source rows differ from independently issued cut")
        ));
        source_page.rows[0].value = original_epoch;

        let meta_key = native_key(AuxiliaryRowFamilyV1::StateMeta, &[]);
        let changed_payload = serde_json::to_vec(&NativeRow {
            key: Vec::new(),
            data: NativeRowData::Row(HistoryReadbackRowV1::Materialization {
                commits: true,
                refs: false,
                tags: false,
                diff_hunks: false,
            }),
        })
        .expect("different valid Native History metadata");
        let changed_reference = native_immutable_object_ref_v1(&changed_payload, &[])
            .expect("content-addressed changed row");
        let entry = entries.iter_mut().find(|(key, _)| *key == meta_key)
            .expect("History metadata map entry");
        let original_reference = match entry.1 {
            MapValue::Object(reference) => reference,
            MapValue::Inline(_) => panic!("History row must be a child object"),
        };
        entry.1 = MapValue::Object(changed_reference);
        let object = objects.iter_mut()
            .find(|stored| stored.registration.reference == original_reference)
            .expect("original History metadata object");
        *object = StoredObject::new(
            RegisteredObject::unscoped(changed_reference, changed_payload.len() as u64, Vec::new()),
            changed_payload,
            None,
        );
        assert!(matches!(
            verify(&mut manifest, &source_page, &entries, &objects),
            Err(IndexError::InvalidInput(message)) if message.contains("History full row digest differs")
        ));
    }
}
