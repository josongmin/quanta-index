//! Immutable History members. The Kernel owns view CAS and ticket lifetime;
//! this adapter owns complete History rows, source-cut binding and execution.
//! Historical paths are never represented as current SourceLocal file owners.

mod build;
mod generation_input;
mod query;
mod reader;
pub use build::{
    NativeHistoryBootSourceV1, NativeHistoryBuildV1, with_native_history_boot_source_v1,
    with_native_history_source_input_v1,
};
pub use reader::{NativeHistoryReaderV1, PlannedNativeHistoryReadV1, plan_native_history_read_v1};

use crate::readiness::serde_support::impl_struct_serde;
use quanta_index_contract::{
    AuxEpochV1, BatchPublishReceipt, HistoryIngestBatch, ManifestGeneration, RepoId, RevisionId,
    index::{
        Digest, GroupActivation, IndexKind, InputBinding, InputDomain, MapRoot, MemberBinding,
        ObjectRef, SchemaId, StorageFormatId, ViewRef,
    },
};
use quanta_index_core::{
    AuxiliaryRowFamilyV1, AuxiliaryRowV1, CoreError,
    domains::auxiliary::{HistoryReadbackRowV1, HistorySnapshotDigestV1, HistorySourceCutV1},
    index::{
        AttemptContext, IndexError, MapEdit, MapPage, MapPageEntry, MemoryLease, MemoryLifetime,
        QueryReadContext, RegisteredObject, StoredObject, WorkBudget, WorkUsage,
        decode_member_binding, member_map_key,
    },
};
use quanta_index_ipc::{
    CborPayloadLengthProgressErrorV1, cbor_payload_len_with_progress_v1, decode_cbor_payload,
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, EnumAccess, MapAccess, VariantAccess, Visitor},
    ser::SerializeStruct,
};
use sha2::{Digest as _, Sha256};
use std::fmt;

use crate::auxiliary_authority::{
    decode_history_row,
    history_source::{ReplayCustody, history_work_error},
};
use crate::readiness::{HistoryAuthorityState, HistoryDiffKey, HistoryStateMeta};
use crate::{HistoryReplayFenceV1, ReplayedHistorySourceV1};
pub use query::{NativeHistoryPageV1, NativeHistoryQueryResultV1};

pub const NATIVE_HISTORY_KIND_V1: IndexKind = IndexKind(*b"qidx.history.v1!");
pub const NATIVE_HISTORY_FORMAT_V1: StorageFormatId = StorageFormatId(*b"qidx.hist.rows1!");
pub const NATIVE_HISTORY_SCHEMA_V1: SchemaId = SchemaId(*b"qidx.hist.data1!");
const REPLAY_HISTORY_MANIFEST: u32 = 1;
const APPEND_HISTORY_MANIFEST: u32 = 2;
const SNAPSHOT_HISTORY_MANIFEST: u32 = 3;
const SUCCESSOR_HISTORY_MANIFEST: u32 = 4;

#[must_use]
pub fn native_history_policy_v1() -> Digest {
    let mut hash = Sha256::new();
    hash.update(b"qidx.native.history.rows.v1\0;hashed-row-keys;typed-json-rows;bound-source-input;canonical-history-values;commit+diff;exact-length-bm25:k1=1.2,b=.75;sha-path-total-order");
    hash.update(
        quanta_index_lq_text_normalizer::TEXT_NORMALIZER_VERSION
            .major
            .to_le_bytes(),
    );
    hash.update(
        quanta_index_lq_text_normalizer::TEXT_NORMALIZER_VERSION
            .minor
            .to_le_bytes(),
    );
    Digest(hash.finalize().into())
}

pub(crate) struct Manifest {
    format: u32,
    repo: RepoId,
    revision: RevisionId,
    generation: ManifestGeneration,
    epoch: AuxEpochV1,
    snapshot: Digest,
    replay: Digest,
    sequence: u64,
    source_manifest: Option<String>,
    predecessor: Option<HistorySourceCutV1>,
    source_input: ObjectRef,
    rows: MapRoot,
    row_count: u64,
    pub(crate) maximum_row_bytes: u64,
}

impl_struct_serde!(Manifest {
    format: u32, repo: RepoId, revision: RevisionId, generation: ManifestGeneration,
    epoch: AuxEpochV1, snapshot: Digest, replay: Digest, sequence: u64,
    source_manifest: Option<String>, predecessor: Option<HistorySourceCutV1>, source_input: ObjectRef,
    rows: MapRoot, row_count: u64, maximum_row_bytes: u64,
});

impl Manifest {
    fn cut(&self) -> HistorySourceCutV1 {
        HistorySourceCutV1 {
            generation: quanta_index_core::AuxiliaryGenerationKeyV1 {
                repo_id: self.repo.clone(),
                revision_id: self.revision.clone(),
                generation: self.generation,
            },
            durable_epoch: self.epoch,
            snapshot_digest: self.snapshot,
            replay_start: self.replay,
        }
    }
    fn context(&self) -> Result<Digest, CoreError> {
        self.cut().input_context_v1().map_err(invalid)
    }
    fn validate(&self) -> Result<(), CoreError> {
        if !matches!(self.format, REPLAY_HISTORY_MANIFEST | APPEND_HISTORY_MANIFEST | SNAPSHOT_HISTORY_MANIFEST | SUCCESSOR_HISTORY_MANIFEST)
            || self.epoch == AuxEpochV1::GENESIS
            || self.sequence == 0
            || self.snapshot == Digest([0; 32])
            || self.replay == Digest([0; 32])
            || self.rows.0.is_none()
            || self.row_count < 2
            || self.maximum_row_bytes == 0
            || self.source_input.0 == Digest([0; 32])
        {
            return Err(invalid("incomplete Native History manifest"));
        }
        if self.format == APPEND_HISTORY_MANIFEST
            && !self.predecessor.as_ref().is_some_and(|prior| {
                prior.generation.repo_id == self.repo
                    && prior.generation.revision_id == self.revision
                    && prior.generation.generation == self.generation
                    && prior.durable_epoch.checked_next() == Some(self.epoch)
                    && prior.replay_start != self.replay
            })
        {
            return Err(invalid("Native History append lacks its exact predecessor"));
        }
        if self.format == SNAPSHOT_HISTORY_MANIFEST && self.predecessor.is_some() {
            return Err(invalid("Native History snapshot must be its initial FULL"));
        }
        if self.format == SUCCESSOR_HISTORY_MANIFEST
            && (self.epoch != AuxEpochV1::new(1)
                || !self.source_manifest.as_ref().is_some_and(|value| !value.is_empty())
                || !self.predecessor.as_ref().is_some_and(|prior| {
                    prior.generation.is_next_generation_of_repo_v1(&self.repo, self.generation)
                        && prior.replay_start != self.replay
                }))
        {
            return Err(invalid("Native History generation lacks its exact predecessor"));
        }
        Ok(())
    }
}

/// Prepared DATA only. Installation uses the original AttemptContext custody;
/// only the Kernel may adopt its member through the original CatalogView CAS.
pub struct PreparedNativeHistoryV1 {
    artifact: ObjectRef,
    manifest: Manifest,
    _manifest_memory: Option<MemoryLease>,
}

impl PreparedNativeHistoryV1 {
    #[must_use]
    pub const fn artifact(&self) -> ObjectRef {
        self.artifact
    }

    /// Recompute the authenticated boot member's cut from the installed rows.
    /// The caller can release the decoded original input before this readback.
    pub fn verify_boot_rows_v1(
        self,
        get: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<StoredObject, IndexError>,
        scan: impl FnMut(MapRoot, Option<&[u8]>, u32, &mut WorkBudget) -> Result<MapPage, IndexError>,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        drop(
            load_rows_with_v1(&self.manifest, get, scan, budget)
                .map_err(|error| index_error(error, budget))?,
        );
        budget.checkpoint()?;
        Ok(self.artifact)
    }
    pub fn into_member(
        self,
        instance: &quanta_index_contract::index::InstanceSpec,
    ) -> Result<MemberBinding, CoreError> {
        validate_instance(instance)?;
        Ok(MemberBinding {
            instance: instance.instance,
            format: instance.format,
            artifact: self.artifact,
            input: InputBinding {
                root: self.manifest.rows,
                schema: instance.schema,
                policy: instance.policy,
                context: self.manifest.context()?,
            },
            logical_commitment: self.manifest.snapshot,
        })
    }
}

/// A same-ticket member and its complete immutable rows. It owns every decoded
/// DATA grant; query consumes it into a result that retains those grants through
/// response encoding/writing. It owns no ticket or execution authority.
pub struct NativeHistoryReadV1 {
    pub(crate) source: ReplayedHistorySourceV1,
    pub(crate) manifest: Manifest,
    pub(crate) view: ViewRef,
    pub(crate) instance: quanta_index_contract::index::IndexInstanceId,
    pub(crate) artifact: ObjectRef,
    _manifest_memory: Option<MemoryLease>,
}

pub fn prepare_native_history_full_v1(
    source: &ReplayedHistorySourceV1,
    source_input: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<PreparedNativeHistoryV1, CoreError> {
    prepare(source, None, source_input, REPLAY_HISTORY_MANIFEST, context)
}

/// Prepare an initial member from a source owner's already fenced SQL row scan.
/// The caller must hold the Legacy writer/producer fence, issue `expected`
/// independently of this target, and pass its actual sealed source anchor.
/// This helper grants no source authority and is not a generic input stager.
pub fn prepare_native_history_snapshot_full_v1(
    expected: &HistorySourceCutV1,
    durable_sequence: u64,
    source_manifest: Option<String>,
    source_input: ObjectRef,
    read_rows: impl FnOnce(
        &mut WorkBudget,
        &mut dyn FnMut(AuxiliaryRowV1, &mut WorkBudget) -> Result<(), CoreError>,
    ) -> Result<(), CoreError>,
    context: &mut AttemptContext<'_>,
) -> Result<PreparedNativeHistoryV1, CoreError> {
    let source = restore_source_snapshot_rows(
        expected,
        durable_sequence,
        source_manifest,
        read_rows,
        context.budget,
    )?;
    prepare(&source, None, source_input, SNAPSHOT_HISTORY_MANIFEST, context)
}

impl NativeHistoryReadV1 {
    pub fn source_cut(&self) -> &HistorySourceCutV1 {
        self.source.cut()
    }
    #[must_use]
    pub const fn view(&self) -> ViewRef {
        self.view
    }

    /// Validated ACK/body + exact predecessor. Unrelated durable sequences may
    /// intervene; the cumulative History epoch must advance exactly once.
    pub fn prepare_delta(
        self,
        fence: HistoryReplayFenceV1,
        batch: &HistoryIngestBatch,
        ack: &BatchPublishReceipt,
        source_input: ObjectRef,
        context: &mut AttemptContext<'_>,
    ) -> Result<PreparedNativeHistoryV1, CoreError> {
        let (source, prior) = self.advance_source(fence, batch, ack, context.budget)?;
        prepare(
            &source,
            Some((&prior.manifest, batch)),
            source_input,
            REPLAY_HISTORY_MANIFEST,
            context,
        )
    }

    fn advance_source(
        self,
        fence: HistoryReplayFenceV1,
        batch: &HistoryIngestBatch,
        ack: &BatchPublishReceipt,
        budget: &mut WorkBudget,
    ) -> Result<(ReplayedHistorySourceV1, PreparedNativeHistoryV1), CoreError> {
        if self.manifest.format != REPLAY_HISTORY_MANIFEST {
            return Err(invalid("Legacy History replay cannot advance a Native publication"));
        }
        let source = replay_delta_source(self.source, fence, batch, ack, budget)?;
        let prior = PreparedNativeHistoryV1 {
            artifact: self.artifact,
            manifest: self.manifest,
            _manifest_memory: self._manifest_memory,
        };
        Ok((source, prior))
    }

    pub fn open(context: &mut QueryReadContext<'_>) -> Result<Self, CoreError> {
        let prepared = open_ticket_manifest(context)?;
        verify_manifest_source(&prepared.manifest, context.objects, context.budget)?;
        let source = load_rows(&prepared.manifest, context.objects, context.maps, context.budget)?;
        Ok(Self {
            source,
            manifest: prepared.manifest,
            view: context.ticket.view.identity,
            instance: context.instance.instance,
            artifact: prepared.artifact,
            _manifest_memory: prepared._manifest_memory,
        })
    }
}

fn open_ticket_manifest(context: &mut QueryReadContext<'_>) -> Result<PreparedNativeHistoryV1, CoreError> {
    validate_instance(context.instance)?;
    let key = member_map_key(context.instance.instance);
    let reference = context.maps.lookup_object(context.ticket.view.members, &key, context.budget)
        .map_err(|error| history_work_error(context.budget, error))?
        .ok_or_else(|| invalid("History member absent from selected view"))?;
    let stored = context.objects.get(reference, context.budget)
        .map_err(|error| history_work_error(context.budget, error))?;
    let member = decode_member_binding(&stored.payload, context.budget)
        .map_err(|error| history_work_error(context.budget, error))?;
    if stored.registration.reference != reference
        || !stored.registration.children.iter().copied().eq([Some(member.artifact), member.input.root.0].into_iter().flatten())
    {
        return Err(invalid("History member differs from selected ticket"));
    }
    let prepared = load_manifest(member.artifact, context.objects, context.budget)?;
    validate_member(&member, context.instance, &prepared.manifest)?;
    Ok(prepared)
}

/// Read just the cut of the exact pinned member. The retained grant travels
/// with the returned repository/revision identifiers through response encoding.
pub fn read_native_history_cut_v1(
    context: &mut QueryReadContext<'_>,
) -> Result<(ObjectRef, HistorySourceCutV1, Option<MemoryLease>), IndexError> {
    let prepared = open_ticket_manifest(context)
        .map_err(|error| index_error(error, context.budget))?;
    let Manifest { repo, revision, generation, epoch, snapshot, replay, .. } = prepared.manifest;
    let cut = HistorySourceCutV1 {
        generation: quanta_index_core::AuxiliaryGenerationKeyV1 {
            repo_id: repo, revision_id: revision, generation,
        },
        durable_epoch: epoch,
        snapshot_digest: snapshot,
        replay_start: replay,
    };
    Ok((prepared.artifact, cut, prepared._manifest_memory))
}

/// Read the source closure published with the selected generation's member.
pub fn read_native_history_generation_v1(
    context: &mut QueryReadContext<'_>,
) -> Result<(ObjectRef, ObjectRef, quanta_index_contract::index::HistoryGenerationInputV1, Digest), IndexError> {
    (|| {
        let prepared = open_ticket_manifest(context)?;
        if prepared.manifest.format != SUCCESSOR_HISTORY_MANIFEST {
            return Err(CoreError::NotReady("History generation outputs were not published by a Source advance".into()));
        }
        let reference = prepared.manifest.source_input;
        let stored = context.objects.get(reference, context.budget)
            .map_err(|error| history_work_error(context.budget, error))?;
        let inputs = generation_input::decode_closure(reference, &stored, context.budget)?;
        let predecessor = prepared.manifest.predecessor.as_ref()
            .ok_or_else(|| invalid("History successor predecessor is absent"))?;
        let bytes = predecessor.generation.repo_id.as_str().len()
            + predecessor.generation.revision_id.as_str().len() + 128;
        context.budget.charge(WorkUsage { bytes: bytes as u64, ..WorkUsage::default() })
            .map_err(|error| history_work_error(context.budget, error))?;
        let predecessor_context = predecessor.input_context_v1().map_err(invalid)?;
        Ok((prepared.artifact, reference, inputs, predecessor_context))
    })().map_err(|error| index_error(error, context.budget))
}

/// Check a generated immutable History artifact under the original attempt.
/// The typed build port derives it from the distinct source input carrier;
/// this structural check alone grants no source completeness/publication.
pub fn verify_native_history_input_v1(
    input: ObjectRef,
    expected: &HistorySourceCutV1,
    instance: &quanta_index_contract::index::InstanceSpec,
    context: &mut AttemptContext<'_>,
) -> Result<MemberBinding, CoreError> {
    validate_instance(instance)?;
    let prepared = open_manifest(input, context.objects, context.budget)?;
    if &prepared.manifest.cut() != expected {
        return Err(invalid(
            "sealed History input differs from requested source cut",
        ));
    }
    let _complete = load_rows(
        &prepared.manifest,
        context.objects,
        context.maps,
        context.budget,
    )?;
    prepared.into_member(instance)
}

fn open_manifest(
    input: ObjectRef,
    objects: &dyn quanta_index_core::index::ports::ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<PreparedNativeHistoryV1, CoreError> {
    let prepared = load_manifest(input, objects, budget)?;
    verify_manifest_source(&prepared.manifest, objects, budget)?;
    Ok(prepared)
}

fn load_manifest(
    input: ObjectRef,
    objects: &dyn quanta_index_core::index::ports::ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<PreparedNativeHistoryV1, CoreError> {
    let stored = objects
        .get(input, budget)
        .map_err(|error| history_work_error(budget, error))?;
    decode_manifest(input, &stored, budget)
}

fn decode_manifest(
    input: ObjectRef,
    stored: &quanta_index_core::index::StoredObject,
    budget: &mut WorkBudget,
) -> Result<PreparedNativeHistoryV1, CoreError> {
    let bound = u64::try_from(stored.payload.len())
        .ok()
        .and_then(|bytes| bytes.checked_mul(32))
        .and_then(|bytes| bytes.checked_add(512))
        .ok_or_else(|| invalid("History manifest allocation bound overflow"))?;
    let memory = budget
        .reserve_memory(bound, MemoryLifetime::Retained)
        .map_err(|error| history_work_error(budget, error))?;
    let manifest: Manifest =
        serde_json::from_slice(&stored.payload).map_err(|error| invalid(&error.to_string()))?;
    manifest.validate()?;
    if stored.registration.reference != input
        || !stored.registration.children.iter().copied().eq([
            manifest.rows.0,
            Some(manifest.source_input),
        ]
        .into_iter()
        .flatten())
    {
        return Err(invalid(
            "History manifest child graph differs from its row map",
        ));
    }
    Ok(PreparedNativeHistoryV1 {
        artifact: input,
        manifest,
        _manifest_memory: memory,
    })
}

/// Bind the installed FULL manifest to the independently checked source input.
/// The caller verifies its persisted row-set with `verify_boot_rows_v1`.
pub fn verify_native_history_boot_member_v1(
    member: &MemberBinding,
    stored: &StoredObject,
    expected: &HistorySourceCutV1,
    source: &RegisteredObject,
    input: &NativeHistoryBootSourceV1<'_>,
    get: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<StoredObject, IndexError>,
    budget: &mut WorkBudget,
) -> Result<PreparedNativeHistoryV1, IndexError> {
    let prepared = decode_manifest(member.artifact, stored, budget)
        .map_err(|error| index_error(error, budget))?;
    let manifest = &prepared.manifest;
    if !matches!(manifest.format, REPLAY_HISTORY_MANIFEST | SNAPSHOT_HISTORY_MANIFEST)
        || manifest.predecessor.is_some()
        || &manifest.cut() != expected
        || member.format != NATIVE_HISTORY_FORMAT_V1
        || member.input.schema != NATIVE_HISTORY_SCHEMA_V1
        || member.input.policy != native_history_policy_v1()
        || member.input.root != manifest.rows
        || member.input.context != manifest.context().map_err(|error| index_error(error, budget))?
        || member.logical_commitment != manifest.snapshot
    {
        return Err(IndexError::DigestConflict);
    }
    validate_manifest_source_registration(manifest, source)
        .map_err(|error| index_error(error, budget))?;
    let (sequence, source_manifest) = match input {
        NativeHistoryBootSourceV1::Replay(original) => (
            original.fence.final_durable_sequence,
            original.fence.manifest_digest.as_deref(),
        ),
        NativeHistoryBootSourceV1::Snapshot(checkpoint) => (
            checkpoint.terminal_ack.durable_sequence,
            checkpoint.terminal_ack.manifest_digest.as_deref(),
        ),
    };
    if manifest.sequence != sequence || manifest.source_manifest.as_deref() != source_manifest {
        return Err(IndexError::DigestConflict);
    }
    if let NativeHistoryBootSourceV1::Snapshot(checkpoint) = input {
        verify_source_snapshot_rows(
            expected,
            |budget, visit| build::visit_snapshot_checkpoint_rows_v1(checkpoint, get, budget, visit),
            budget,
            |_, _| Ok(()),
        )
        .map_err(|error| index_error(error, budget))?;
    }
    budget.checkpoint()?;
    Ok(prepared)
}

fn verify_manifest_source(
    manifest: &Manifest,
    objects: &dyn quanta_index_core::index::ports::ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<(), CoreError> {
    let origin = objects.get(manifest.source_input, budget)
        .map_err(|error| history_work_error(budget, error))?;
    validate_manifest_source_registration(manifest, &origin.registration)?;
    if manifest.format == SUCCESSOR_HISTORY_MANIFEST {
        let _verified_closure =
            generation_input::decode_closure(manifest.source_input, &origin, budget)?;
    }
    Ok(())
}

fn validate_manifest_source_registration(
    manifest: &Manifest,
    source: &RegisteredObject,
) -> Result<(), CoreError> {
    if manifest.source_input != source.reference
        || (manifest.format != SUCCESSOR_HISTORY_MANIFEST
            && (manifest.format == SNAPSHOT_HISTORY_MANIFEST) == source.children.is_empty())
    {
        return Err(invalid("History source input child graph differs"));
    }
    Ok(())
}

fn replay_delta_source(
    source: ReplayedHistorySourceV1,
    fence: HistoryReplayFenceV1,
    batch: &HistoryIngestBatch,
    ack: &BatchPublishReceipt,
    budget: &mut WorkBudget,
) -> Result<ReplayedHistorySourceV1, CoreError> {
    let ordinal = source
        .cut()
        .durable_epoch
        .get()
        .checked_add(1)
        .ok_or_else(|| invalid("History epoch overflow"))?;
    if fence.publication_count != ordinal {
        return Err(invalid(
            "History DELTA must contain exactly the next publication",
        ));
    }
    let before = fence.first_replay;
    let after = fence.final_replay;
    source
        .resume(fence)?
        .apply(ordinal, before, after, batch, ack, budget)?
        .finish(budget)
}

fn validate_member(
    member: &MemberBinding,
    instance: &quanta_index_contract::index::InstanceSpec,
    manifest: &Manifest,
) -> Result<(), CoreError> {
    if member.instance != instance.instance
        || member.format != instance.format
        || member.input.schema != instance.schema
        || member.input.policy != instance.policy
        || member.input.root != manifest.rows
        || member.input.context != manifest.context()?
        || member.logical_commitment != manifest.snapshot
    {
        return Err(invalid(
            "History member differs from its selected instance and committed cut",
        ));
    }
    Ok(())
}

fn validate_instance(
    instance: &quanta_index_contract::index::InstanceSpec,
) -> Result<(), CoreError> {
    if instance.input_domain != InputDomain::History
        || instance.kind != NATIVE_HISTORY_KIND_V1
        || instance.format != NATIVE_HISTORY_FORMAT_V1
        || instance.schema != NATIVE_HISTORY_SCHEMA_V1
        || instance.policy != native_history_policy_v1()
        || instance.activation != GroupActivation::CoreRequired
    {
        return Err(invalid("incompatible Native History instance"));
    }
    Ok(())
}

fn prepare(
    source: &ReplayedHistorySourceV1,
    prior: Option<(&Manifest, &HistoryIngestBatch)>,
    source_input: ObjectRef,
    format: u32,
    context: &mut AttemptContext<'_>,
) -> Result<PreparedNativeHistoryV1, CoreError> {
    let cut = source.cut();
    let mut root = prior.map_or(MapRoot(None), |(prior, _)| prior.rows);
    let mut row_count = 0_u64;
    let mut maximum_row_bytes = 0_u64;
    let attempt = context.attempt;
    let objects = context.objects;
    let maps = context.maps;
    let custody = context.custody;
    let scratch = context.scratch;
    source.visit_rows(context.budget, &mut |row, budget| {
        row_count = row_count
            .checked_add(1)
            .ok_or_else(|| invalid("History row count overflow"))?;
        maximum_row_bytes = maximum_row_bytes.max(
            u64::try_from(
                row.value
                    .len()
                    .checked_add(row.key.row_key.len())
                    .ok_or_else(|| invalid("History row overflow"))?,
            )
            .map_err(|_| invalid("History row overflow"))?,
        );
        let _key_memory = budget
            .reserve_memory(
                u64::try_from(row.key.row_key.len() + 64)
                    .map_err(|_| invalid("History key overflow"))?,
                MemoryLifetime::Temporary,
            )
            .map_err(|error| history_work_error(budget, error))?;
        let key = native_key(row.key.family, &row.key.row_key);
        let old = maps
            .lookup_object(root, &key, budget)
            .map_err(|error| history_work_error(budget, error))?;
        let _projection_memory = budget
            .reserve_memory(
                u64::try_from(row.value.len())
                    .ok()
                    .and_then(|bytes| bytes.checked_mul(32))
                    .and_then(|bytes| bytes.checked_add(4096))
                    .ok_or_else(|| invalid("History row projection bound overflow"))?,
                MemoryLifetime::Temporary,
            )
            .map_err(|error| history_work_error(budget, error))?;
        // Source rows were encoded from this process's complete typed fold.
        // Persist typed JSON so untrusted CBOR length hints cannot allocate a
        // sequence before the Native object's original byte grant admits it.
        let data = if row.key.family == AuxiliaryRowFamilyV1::Epoch {
            NativeRowData::Epoch(
                decode_cbor_payload(&row.value).map_err(|error| invalid(&error.to_string()))?,
            )
        } else {
            NativeRowData::Row(decode_history_row(row)?)
        };
        let encoded = encode_json(
            &NativeRowRef {
                key: &row.key.row_key,
                data: &data,
            },
            budget,
        )?;
        let mut local = AttemptContext {
            attempt,
            objects,
            maps,
            custody,
            scratch,
            budget: &mut *budget,
        };
        let next = local
            .put(&encoded.bytes, &[])
            .map_err(|error| history_work_error(local.budget, error))?;
        if old != Some(next) {
            root = local
                .apply(root, &[MapEdit::objects(key, old, Some(next))])
                .map_err(|error| history_work_error(local.budget, error))?;
        }
        Ok(())
    })?;
    // DELTA deletions are explicit COW removes. The old root remains immutable
    // for retained views; do not clear/rebuild a family or edit the old root.
    if let Some((_, batch)) = prior {
        for (family, mutations) in [
            (AuxiliaryRowFamilyV1::Ref, &batch.refs),
            (AuxiliaryRowFamilyV1::Tag, &batch.tags),
        ] {
            for mutation in mutations {
                if let quanta_index_contract::HistoryRefMutation::Delete(deletion) = mutation {
                    let retained = if family == AuxiliaryRowFamilyV1::Ref {
                        source.state.refs().contains_key(deletion.name.as_ref())
                    } else {
                        source.state.tags().contains_key(deletion.name.as_ref())
                    };
                    // The canonical transition permits ordered same-key changes;
                    // an upsert after a delete must remain in the final snapshot.
                    if retained {
                        continue;
                    }
                    let _memory = context
                        .budget
                        .reserve_memory(128, MemoryLifetime::Temporary)
                        .map_err(|error| history_work_error(context.budget, error))?;
                    let key = native_key(family, deletion.name.as_bytes());
                    let old = context
                        .maps
                        .lookup_object(root, &key, context.budget)
                        .map_err(|error| history_work_error(context.budget, error))?;
                    if old.is_some() {
                        root = context
                            .apply(root, &[MapEdit::objects(key, old, None)])
                            .map_err(|error| history_work_error(context.budget, error))?;
                    }
                }
            }
        }
    }
    let manifest_bound = cut
        .generation
        .repo_id
        .as_str()
        .len()
        .checked_add(cut.generation.revision_id.as_str().len())
        .and_then(|bytes| bytes.checked_add(source.manifest_digest.as_ref().map_or(0, String::len)))
        .and_then(|bytes| {
            bytes.checked_add(prior.map_or(0, |(prior, _)| {
                prior.repo.as_str().len() + prior.revision.as_str().len()
            }))
        })
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Manifest>() + 512))
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(|| invalid("History manifest allocation bound overflow"))?;
    let manifest_memory = context
        .budget
        .reserve_memory(manifest_bound, MemoryLifetime::Retained)
        .map_err(|error| history_work_error(context.budget, error))?;
    let manifest = Manifest {
        format,
        repo: cut.generation.repo_id.clone(),
        revision: cut.generation.revision_id.clone(),
        generation: cut.generation.generation,
        epoch: cut.durable_epoch,
        snapshot: cut.snapshot_digest,
        replay: cut.replay_start,
        sequence: source.sequence,
        source_manifest: source.manifest_digest.clone(),
        predecessor: prior.map(|(prior, _)| prior.cut()),
        source_input,
        rows: root,
        row_count,
        maximum_row_bytes,
    };
    manifest.validate()?;
    let payload = encode_json(&manifest, context.budget)?;
    let row_child = root.0.ok_or_else(|| invalid("History row root absent"))?;
    let artifact = context
        .put(&payload.bytes, &[row_child, source_input])
        .map_err(|error| history_work_error(context.budget, error))?;
    Ok(PreparedNativeHistoryV1 {
        artifact,
        manifest,
        _manifest_memory: manifest_memory,
    })
}

// Fixed map keys avoid truncating a valid historical path at the map-key
// boundary. The original key is carried by the authenticated immutable row.
struct NativeRowRef<'a> {
    key: &'a [u8],
    data: &'a NativeRowData,
}
struct NativeRow {
    key: Vec<u8>,
    data: NativeRowData,
}
enum NativeRowData {
    Epoch(AuxEpochV1),
    Row(HistoryReadbackRowV1),
}

impl Serialize for NativeRowRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut row = serializer.serialize_struct("NativeRow", 2)?;
        row.serialize_field("key", self.key)?;
        row.serialize_field("data", self.data)?;
        row.end()
    }
}
impl_struct_serde!(NativeRow { key: Vec<u8>, data: NativeRowData });
impl Serialize for NativeRowData {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Epoch(epoch) => {
                serializer.serialize_newtype_variant("NativeRowData", 0, "Epoch", epoch)
            }
            Self::Row(row) => serializer.serialize_newtype_variant("NativeRowData", 1, "Row", row),
        }
    }
}
impl<'de> Deserialize<'de> for NativeRowData {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DataVisitor;
        impl<'de> Visitor<'de> for DataVisitor {
            type Value = NativeRowData;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("Native History row data")
            }
            fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
                let (name, variant) = data.variant::<String>()?;
                match name.as_str() {
                    "Epoch" => variant.newtype_variant().map(NativeRowData::Epoch),
                    "Row" => variant.newtype_variant().map(NativeRowData::Row),
                    _ => Err(de::Error::unknown_variant(&name, &["Epoch", "Row"])),
                }
            }
        }
        deserializer.deserialize_enum("NativeRowData", &["Epoch", "Row"], DataVisitor)
    }
}

fn native_key(family: AuxiliaryRowFamilyV1, key: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(family.as_code_str().len() + 1 + 32);
    bytes.extend_from_slice(family.as_code_str().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&Sha256::digest(key));
    bytes
}
fn split_key(key: &[u8]) -> Result<AuxiliaryRowFamilyV1, CoreError> {
    let offset = key
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| invalid("History key family missing"))?;
    if key.len() != offset + 33 {
        return Err(invalid("History map key digest width invalid"));
    }
    std::str::from_utf8(&key[..offset])
        .ok()
        .and_then(AuxiliaryRowFamilyV1::from_code_str)
        .ok_or_else(|| invalid("History key family invalid"))
}

// Both immutable Native readback and the fenced SQL snapshot bootstrap restore
// the same typed state and run the same final reference/digest checks.
struct HistoryRowRestorerV1 {
    output: ReplayedHistorySourceV1,
    count: u64,
    saw_epoch: bool,
    saw_meta: bool,
}

impl HistoryRowRestorerV1 {
    fn new(
        cut: HistorySourceCutV1,
        sequence: u64,
        source_manifest: Option<String>,
    ) -> Result<Self, CoreError> {
        if cut.durable_epoch == AuxEpochV1::GENESIS
            || cut.snapshot_digest == Digest([0; 32])
            || cut.replay_start == Digest([0; 32])
            || sequence == 0
        {
            return Err(invalid("History source cut or durable sequence is incomplete"));
        }
        Ok(Self {
            output: ReplayedHistorySourceV1 {
                state: HistoryAuthorityState::default(),
                cut,
                sequence,
                manifest_digest: source_manifest,
                _custody: ReplayCustody::default(),
            },
            count: 0,
            saw_epoch: false,
            saw_meta: false,
        })
    }

    fn accept(
        &mut self,
        family: AuxiliaryRowFamilyV1,
        key: Vec<u8>,
        data: NativeRowData,
        memory: Option<MemoryLease>,
    ) -> Result<(), CoreError> {
        self.count = self.count.checked_add(1).ok_or_else(|| invalid("History row count overflow"))?;
        match data {
            NativeRowData::Epoch(epoch) => {
                if family != AuxiliaryRowFamilyV1::Epoch
                    || self.saw_epoch
                    || !key.is_empty()
                    || epoch != self.output.cut.durable_epoch
                {
                    return Err(invalid("History epoch singleton differs from committed cut"));
                }
                self.saw_epoch = true;
            }
            NativeRowData::Row(decoded) => {
                validate_row_key(family, &key, &decoded)?;
                match decoded {
                    HistoryReadbackRowV1::Commit(record) => self.output.state.restore_commit(record.sha, record),
                    HistoryReadbackRowV1::Ref(update) => self.output.state.restore_ref(&update.name, update.sha),
                    HistoryReadbackRowV1::Tag(update) => self.output.state.restore_tag(&update.name, update.sha),
                    HistoryReadbackRowV1::DiffHunk(update) => self.output.state.restore_diff_hunk(
                        HistoryDiffKey::new(update.commit_sha, &update.file_path), update.record,
                    ),
                    HistoryReadbackRowV1::Materialization { commits, refs, tags, diff_hunks } => {
                        if self.saw_meta {
                            return Err(invalid("duplicate History metadata"));
                        }
                        self.output.state.restore_meta(HistoryStateMeta {
                            commits_materialized: commits,
                            refs_materialized: refs,
                            tags_materialized: tags,
                            diff_hunks_materialized: diff_hunks,
                        });
                        self.saw_meta = true;
                    }
                }
                self.output._custody.replace(family, key, memory);
            }
        }
        Ok(())
    }

    fn finish(
        self,
        expected_count: Option<u64>,
        expected_maximum_row_bytes: Option<u64>,
        budget: &mut WorkBudget,
    ) -> Result<ReplayedHistorySourceV1, CoreError> {
        if expected_count.is_some_and(|count| count != self.count)
            || self.count < 2
            || !self.saw_epoch
            || !self.saw_meta
        {
            return Err(invalid("History complete inventory differs from committed cut"));
        }
        let output = self.output;
        for record in output.state.commits().values() {
            budget.checkpoint().map_err(|error| history_work_error(budget, error))?;
            if record.parents.iter().any(|parent| !output.state.commits().contains_key(parent)) {
                return Err(invalid("History parent absent from snapshot"));
            }
        }
        if output.state.refs().values().chain(output.state.tags().values())
            .any(|sha| !output.state.commits().contains_key(sha))
            || output.state.diff_hunks().keys()
                .any(|key| !output.state.commits().contains_key(&key.commit_sha()))
        {
            return Err(invalid("History target absent from snapshot"));
        }
        // Native map keys are hash-ordered. Rehash the one canonical History
        // row encoder so readback and source export use the same logical cut.
        let mut hash = HistorySnapshotDigestV1::new(&output.cut.generation);
        let mut maximum_row_bytes = 0_u64;
        output.visit_rows(budget, &mut |row, _| {
            maximum_row_bytes = maximum_row_bytes.max(
                u64::try_from(row.value.len().checked_add(row.key.row_key.len())
                    .ok_or_else(|| invalid("History row weight overflow"))?)
                    .map_err(|_| invalid("History row weight overflow"))?,
            );
            hash.push_row(row.key.family, &row.key.row_key, &row.value);
            Ok(())
        })?;
        if hash.finish() != output.cut.snapshot_digest
            || expected_maximum_row_bytes.is_some_and(|bytes| bytes != maximum_row_bytes)
        {
            return Err(invalid("History full row digest differs from committed cut"));
        }
        Ok(output)
    }
}

fn restore_source_snapshot_rows(
    expected: &HistorySourceCutV1,
    durable_sequence: u64,
    source_manifest: Option<String>,
    read_rows: impl FnOnce(
        &mut WorkBudget,
        &mut dyn FnMut(AuxiliaryRowV1, &mut WorkBudget) -> Result<(), CoreError>,
    ) -> Result<(), CoreError>,
    budget: &mut WorkBudget,
) -> Result<ReplayedHistorySourceV1, CoreError> {
    let mut restore = HistoryRowRestorerV1::new(
        expected.clone(),
        durable_sequence,
        source_manifest,
    )?;
    verify_source_snapshot_rows(expected, read_rows, budget, |row, budget| {
        let family = row.key.family;
        let row_bytes = u64::try_from(row.value.len())
            .ok()
            .and_then(|bytes| bytes.checked_add(u64::try_from(row.key.row_key.len()).ok()?))
            .and_then(|bytes| bytes.checked_mul(16))
            .and_then(|bytes| bytes.checked_add(4096))
            .ok_or_else(|| invalid("History source row allocation bound overflow"))?;
        let memory = budget
            .reserve_memory(row_bytes, MemoryLifetime::Retained)
            .map_err(|error| history_work_error(budget, error))?;
        let data = if family == AuxiliaryRowFamilyV1::Epoch {
            NativeRowData::Epoch(
                decode_cbor_payload(&row.value).map_err(|error| invalid(&error.to_string()))?,
            )
        } else {
            NativeRowData::Row(decode_history_row(&row)?)
        };
        restore.accept(family, row.key.row_key, data, memory)
    })?;
    restore.finish(None, None, budget)
}

fn verify_source_snapshot_rows(
    expected: &HistorySourceCutV1,
    read_rows: impl FnOnce(
        &mut WorkBudget,
        &mut dyn FnMut(AuxiliaryRowV1, &mut WorkBudget) -> Result<(), CoreError>,
    ) -> Result<(), CoreError>,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(AuxiliaryRowV1, &mut WorkBudget) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let mut source_hash = HistorySnapshotDigestV1::new(&expected.generation);
    let mut saw_epoch = false;
    let mut previous: Option<(AuxiliaryRowFamilyV1, Vec<u8>, Option<MemoryLease>)> = None;
    read_rows(budget, &mut |row, budget| {
        budget.checkpoint().map_err(|error| history_work_error(budget, error))?;
        if row.key.domain != quanta_index_core::AuxiliaryDomainV1::History
            || row.key.generation != expected.generation
        {
            return Err(invalid("History source scan returned a foreign row"));
        }
        let family = row.key.family;
        let key = &row.key.row_key;
        if !saw_epoch {
            if family != AuxiliaryRowFamilyV1::Epoch || !key.is_empty() {
                return Err(invalid("History source scan must start with its epoch"));
            }
            saw_epoch = true;
        } else if family == AuxiliaryRowFamilyV1::Epoch
            || previous.as_ref().is_some_and(|(prior_family, prior_key, _)| {
                (family.as_code_str(), key.as_slice())
                    <= (prior_family.as_code_str(), prior_key.as_slice())
            })
        {
            return Err(invalid("History source rows are duplicated or out of order"));
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(row.value.len())
                .ok()
                .and_then(|bytes| bytes.checked_add(u64::try_from(key.len()).ok()?))
                .ok_or_else(|| invalid("History source row work overflow"))?,
            ..WorkUsage::default()
        }).map_err(|error| history_work_error(budget, error))?;
        source_hash.push_row(family, key, &row.value);
        // The epoch is a prefix, while only subsequent rows are key-ordered.
        if family != AuxiliaryRowFamilyV1::Epoch {
            let key_bytes = u64::try_from(key.len())
                .ok()
                .and_then(|bytes| bytes.checked_add(64))
                .ok_or_else(|| invalid("History source order key allocation overflow"))?;
            let order_memory = budget.reserve_memory(key_bytes, MemoryLifetime::Temporary)
                .map_err(|error| history_work_error(budget, error))?;
            previous = Some((family, key.clone(), order_memory));
        }
        visit(row, budget)
    })?;
    if !saw_epoch || source_hash.finish() != expected.snapshot_digest {
        return Err(invalid("History source rows differ from independently issued cut"));
    }
    Ok(())
}

fn load_rows(
    manifest: &Manifest,
    objects: &dyn quanta_index_core::index::ports::ImmutableObjectPort,
    maps: &dyn quanta_index_core::index::ports::PersistentMapPort,
    budget: &mut WorkBudget,
) -> Result<ReplayedHistorySourceV1, CoreError> {
    load_rows_with_v1(
        manifest,
        |reference, budget| objects.get(reference, budget),
        |root, after, limit, budget| maps.scan(root, after, limit, budget),
        budget,
    )
}

fn load_rows_with_v1(
    manifest: &Manifest,
    mut get: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<StoredObject, IndexError>,
    mut scan: impl FnMut(MapRoot, Option<&[u8]>, u32, &mut WorkBudget) -> Result<MapPage, IndexError>,
    budget: &mut WorkBudget,
) -> Result<ReplayedHistorySourceV1, CoreError> {
    // The struct owns state before its grants even when a late row fails.
    let mut restore = HistoryRowRestorerV1::new(
        manifest.cut(),
        manifest.sequence,
        manifest.source_manifest.clone(),
    )?;
    let mut after: Option<MapPageEntry> = None;
    loop {
        let page = scan(
            manifest.rows,
            after
                .as_ref()
                .map(MapPageEntry::key)
                .transpose()
                .map_err(|error| history_work_error(budget, error))?,
            64,
            budget,
        )
        .map_err(|error| history_work_error(budget, error))?;
        let more = page.has_more;
        let mut advanced = false;
        for entry in page.cursor() {
            let (key, reference) = entry
                .object_parts()
                .map_err(|error| history_work_error(budget, error))?;
            if restore.count >= manifest.row_count {
                return Err(invalid("History row inventory exceeds manifest"));
            }
            let family = split_key(key)?;
            let stored = get(*reference, budget)
                .map_err(|error| history_work_error(budget, error))?;
            if stored.registration.reference != *reference
                || !stored.registration.children.is_empty()
            {
                return Err(invalid("History row child graph differs"));
            }
            let bound = u64::try_from(stored.payload.len())
                .ok()
                .and_then(|n| n.checked_mul(16))
                .and_then(|n| n.checked_add(4096))
                .ok_or_else(|| invalid("History row allocation overflow"))?;
            let memory = budget
                .reserve_memory(bound, MemoryLifetime::Retained)
                .map_err(|error| history_work_error(budget, error))?;
            let native: NativeRow = serde_json::from_slice(&stored.payload)
                .map_err(|error| invalid(&error.to_string()))?;
            if native_key(family, &native.key) != *key {
                return Err(invalid("History original key differs from immutable map"));
            }
            restore.accept(family, native.key, native.data, memory)?;
            after = Some(entry);
            advanced = true;
        }
        if !more {
            break;
        }
        if !advanced {
            return Err(invalid("History scan made no progress"));
        }
    }
    restore.finish(
        Some(manifest.row_count),
        Some(manifest.maximum_row_bytes),
        budget,
    )
}
fn validate_row_key(
    family: AuxiliaryRowFamilyV1,
    key: &[u8],
    row: &HistoryReadbackRowV1,
) -> Result<(), CoreError> {
    let valid = match row {
        HistoryReadbackRowV1::Commit(record) => {
            family == AuxiliaryRowFamilyV1::Commit && key == record.sha.as_bytes()
        }
        HistoryReadbackRowV1::Ref(update) => {
            family == AuxiliaryRowFamilyV1::Ref && key == update.name.as_bytes()
        }
        HistoryReadbackRowV1::Tag(update) => {
            family == AuxiliaryRowFamilyV1::Tag && key == update.name.as_bytes()
        }
        HistoryReadbackRowV1::DiffHunk(update) => {
            let (sha, path) = crate::auxiliary_authority::decode_diff_key(key)?;
            family == AuxiliaryRowFamilyV1::DiffHunk
                && sha == update.commit_sha
                && path == update.file_path.as_ref()
        }
        HistoryReadbackRowV1::Materialization { .. } => {
            family == AuxiliaryRowFamilyV1::StateMeta && key.is_empty()
        }
    };
    if !valid {
        return Err(invalid(
            "History typed row differs from its original key/family",
        ));
    }
    Ok(())
}

struct FundedJson {
    bytes: Vec<u8>,
    _memory: Option<MemoryLease>,
}
fn encode_json<T: Serialize>(value: &T, budget: &mut WorkBudget) -> Result<FundedJson, CoreError> {
    struct Measure<'a> {
        length: u64,
        budget: &'a mut WorkBudget,
        refusal: Option<IndexError>,
    }
    impl std::io::Write for Measure<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let length = u64::try_from(bytes.len())
                .map_err(|_| std::io::Error::other("History encoding length overflow"))?;
            if let Err(error) = self.budget.charge(WorkUsage {
                bytes: length,
                ..WorkUsage::default()
            }) {
                self.refusal = Some(error);
                return Err(std::io::Error::other("History encoding work refused"));
            }
            self.length = self
                .length
                .checked_add(length)
                .ok_or_else(|| std::io::Error::other("History encoding length overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    let mut measure = Measure {
        length: 0,
        budget,
        refusal: None,
    };
    let encoded = serde_json::to_writer(&mut measure, value);
    if let Some(error) = measure.refusal.take() {
        return Err(history_work_error(measure.budget, error));
    }
    encoded.map_err(|error| invalid(&error.to_string()))?;
    let length = measure.length;
    let memory = budget
        .reserve_memory(
            length
                .checked_mul(2)
                .ok_or_else(|| invalid("History encoding allocation bound overflow"))?,
            MemoryLifetime::Temporary,
        )
        .map_err(|error| history_work_error(budget, error))?;
    budget
        .charge(WorkUsage {
            bytes: length,
            ..WorkUsage::default()
        })
        .map_err(|error| history_work_error(budget, error))?;
    let bytes = serde_json::to_vec(value).map_err(|error| invalid(&error.to_string()))?;
    if u64::try_from(bytes.len()).ok() != Some(length) {
        return Err(invalid("History encoding changed between admitted passes"));
    }
    Ok(FundedJson {
        bytes,
        _memory: memory,
    })
}

fn index_error(error: CoreError, budget: &WorkBudget) -> IndexError {
    // Keep the original work/cancellation refusal instead of reclassifying it
    // as malformed input. The closed Native boundary maps domain rejections.
    match budget.first_denial() {
        Some(quanta_index_core::index::WorkBudgetDenial::Cancelled) => {
            return IndexError::Cancelled;
        }
        Some(_) => return IndexError::WorkLimitExceeded,
        None => {}
    }
    match error {
        CoreError::Storage(message) | CoreError::StorageIo { message, .. } => {
            IndexError::Storage(message)
        }
        CoreError::NotReady(_) | CoreError::NotFound(_) => IndexError::SnapshotUnavailable,
        CoreError::InvalidContract(message) | CoreError::NotImplemented(message) => {
            IndexError::InvalidInput(message)
        }
        CoreError::Typed { code, message } => {
            IndexError::InvalidInput(format!("{code}: {message}"))
        }
    }
}

fn invalid(message: &str) -> CoreError {
    CoreError::InvalidContract(message.into())
}
