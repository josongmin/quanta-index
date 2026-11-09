//! Token-only native text member. BM25, regex and DSL query routes require
//! separate query admission and are not capabilities of this builder.

mod cold_tokens;
mod contribution;
mod dense_posting;
mod entry_map;
mod entry_patch;
mod occurrence_pack;
mod posting_witness;
pub(super) use entry_map::EntryMap;
use posting_witness::OccurrenceDigestWitness;

use super::text_memory::{PreparedKey, PreparedRows};
use super::text_occurrence::{OccurrenceCount, OccurrenceReader};
use super::text_spool::{
    InventoryPartition, MAX_INVENTORY_ROOTS, MAX_TERM_KEY_BYTES, PostingCasePages,
    PostingScratchRow, PostingTermKey, TextSpool,
};
use quanta_index_core::DiagnosticStageV1;
use quanta_index_core::index::{
    AttemptContext, BaseMember, DeltaInput, Digest, IndexError, MAX_SOURCE_PATH_BYTES, MapEdit,
    MapRoot, MapValue, MemberBinding, ObjectBatchInput, ObjectRef, OwnerKey, PlanContext,
    PlanDecision, ProducerId, QueryReadContext, SourceDelta, SourceInputCursor, SourceRecord,
    SourceTextAdmission, StableOwnerId, StoredObject, TicketRecord, ViewRef, WorkBudget, WorkUsage,
    member_map_key, owner_map_key,
    ports::{ImmutableObjectPort, IndexBuildPort, IndexFuture, PersistentMapPort},
};
use quanta_index_core::index::{MapPageCursor, MapPageEntry};
pub use quanta_index_lq_text_normalizer::CaseMode;
use quanta_index_lq_text_normalizer::{
    TEXT_NORMALIZER_VERSION, TextQueryError, TokenCursor, case_chars,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_NFC_BYTES: u64 = 16 * 1024 * 1024;
const MAX_NFC_WORK_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const MAX_POSITIONS_PER_TERM: u64 = 8_192;
const PAGE: u32 = 64;
const POSTING_PAGE_CAPACITY: usize = 64;
const DENSE_POSTING_TAG: u8 = 1;
// Every input run emits two fixed case surfaces into the 64-row SQL packet.
const TOKEN_RUN_BATCH: usize = 32;
// Only empty-base cold inventories use this wider root window.
const DEFAULT_COLD_INVENTORY_BATCH_ROOTS: usize = MAX_INVENTORY_ROOTS;

/// Maximum raw UTF-8 input for diagnostic single-term inspection.
pub const MAX_TEXT_INSPECT_TERM_BYTES: usize = 4_096;

/// An explicit producer admission stamp and fixed token-only output recipe.
/// Changing normalization, positions, caps or case surfaces changes its digest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextRecipe {
    pub admission_policy: Digest,
}

impl TextRecipe {
    #[must_use]
    pub const fn new(admission_policy: Digest) -> Self {
        Self { admission_policy }
    }

    #[must_use]
    pub fn output_policy(self) -> Digest {
        let mut hash = Sha256::new();
        hash.update(b"qidx.native.text.token-only.v1\0");
        hash.update(self.admission_policy.0);
        hash.update(TEXT_NORMALIZER_VERSION.major.to_le_bytes());
        hash.update(TEXT_NORMALIZER_VERSION.minor.to_le_bytes());
        hash.update(MAX_SOURCE_BYTES.to_le_bytes());
        hash.update(MAX_NFC_BYTES.to_le_bytes());
        hash.update(MAX_NFC_WORK_BYTES.to_le_bytes());
        hash.update(MAX_POSITIONS_PER_TERM.to_le_bytes());
        hash.update(MAX_TERM_KEY_BYTES.to_le_bytes());
        hash.update(b"sensitive+folded;indexed-runs-only;u32-positions");
        hash.update(entry_map::LAYOUT_POLICY);
        hash.update(contribution::LAYOUT_POLICY);
        hash.update(super::text_occurrence::block::LAYOUT_POLICY);
        Digest(hash.finalize().into())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeTextIndex {
    recipe: TextRecipe,
    inventory_reuse_limit: usize,
    inventory_batch_limit: Option<usize>,
}

/// The already authenticated member is valid only for its original ticket
/// view and the reader's selected instance in this query execution.
pub(super) struct BoundTextMember {
    member: MemberBinding,
    view: ViewRef,
}

impl BoundTextMember {
    pub(super) fn from_context(context: &mut QueryReadContext<'_>) -> Result<Self, IndexError> {
        let member = context.source_member_binding()?;
        Ok(Self {
            member,
            view: context.ticket.view.identity,
        })
    }

    pub(super) fn member(&self) -> &MemberBinding {
        &self.member
    }

    pub(super) fn root(
        &self,
        index: &NativeTextIndex,
        context: &mut TextReadContext<'_>,
    ) -> Result<Root, IndexError> {
        if self.view != context.ticket.view.identity
            || self.member.input.policy != index.recipe().output_policy()
            || self.member.input.root != context.ticket.view.source.owners
        {
            return Err(IndexError::Conflict);
        }
        read_root(&self.member, context.objects, context.budget)
    }
}

impl NativeTextIndex {
    #[must_use]
    pub(super) const fn recipe(&self) -> TextRecipe {
        self.recipe
    }

    /// Validate the selected immutable source before an exact-path no-hit result.
    pub(super) fn validate_selected_source(
        &self,
        record: &SourceRecord,
    ) -> Result<bool, IndexError> {
        match record.text {
            SourceTextAdmission::Excluded => Ok(false),
            SourceTextAdmission::Utf8 { policy }
                if policy == self.recipe.admission_policy
                    && record.byte_len <= MAX_SOURCE_BYTES =>
            {
                Ok(true)
            }
            SourceTextAdmission::Utf8 { .. } => Err(IndexError::CorruptObject(
                "selected native text source differs from its admission envelope".into(),
            )),
        }
    }

    #[must_use]
    pub const fn new(recipe: TextRecipe) -> Self {
        Self {
            recipe,
            inventory_reuse_limit: quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS,
            inventory_batch_limit: None,
        }
    }

    /// Bound the resident verified-body and confirmed-occurrence hot windows.
    /// A positive window may spill same-attempt confirmed occurrence identities
    /// into the already admitted disposable text scratch. Zero disables reuse.
    /// This physical policy does not alter the canonical recipe or output identity.
    pub fn with_inventory_reuse_limit(mut self, entries: usize) -> Result<Self, IndexError> {
        if entries > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            return Err(IndexError::InvalidInput(
                "native text inventory reuse exceeds its bounded window".into(),
            ));
        }
        self.inventory_reuse_limit = entries;
        Ok(self)
    }

    /// Override both cold and warm inventory root windows (1..=64).
    /// One selects scalar grouping. This physical policy preserves artifacts.
    pub fn with_inventory_batch_limit(mut self, roots: usize) -> Result<Self, IndexError> {
        if roots == 0 || roots > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            return Err(IndexError::InvalidInput(
                "invalid inventory batch limit".into(),
            ));
        }
        self.inventory_batch_limit = Some(roots);
        Ok(self)
    }

    fn inventory_batch_roots(self, cold: bool) -> usize {
        self.inventory_batch_limit.unwrap_or(if cold {
            DEFAULT_COLD_INVENTORY_BATCH_ROOTS
        } else {
            quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
        })
    }
}

#[cfg(test)]
mod tests {
    #[path = "count_integration_tests.rs"]
    mod count_integration_tests;
    #[path = "overlay_tests.rs"]
    mod overlay_tests;
    #[path = "universe_tests.rs"]
    mod universe_tests;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::path::Path;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    };
    use std::task::{Context, Poll, Waker};
    use std::time::{Duration, Instant};

    use quanta_index_core::index::OwnerMutationDigest;
    use quanta_index_core::index::{
        AttemptId, AttemptRef, CatalogView, CollectionId, GroupActivation, IndexInstanceId,
        IndexKind, InputBinding, InputDomain, MapBatchRequest, MapBatchWrite, MapPage, MapWrite,
        OperationKey, OwnerMutationSet, ProducerId, PublicationGroupId, ReadSessionRef,
        ReadTicketLease, RegisteredObject, SchemaId, ScratchAllocation, ScratchRequest,
        ScratchSpacePort, SlotId, SourceChange, SourceCut, SourceRecordOwner, StableOwnerId,
        StorageFormatId, StoredObject, StreamId, TenantId, TicketId, ViewRef, WorkLimits,
        WorkspaceKey, WorktreeId,
        ports::{MapEditCursor, ObjectCustodyPort, ObjectWriteSink},
    };
    use quanta_index_core::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
        DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1,
        DiagnosticStageV1, DiagnosticTerminalV1, RequestBudgetV1,
    };

    use super::*;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "test assertion failed at {}:{}: {}", file!(), line!(), stringify!($condition)
                )).into());
            }
        }};
        ($condition:expr, $($message:tt)+) => {{
            if !$condition {
                return Err(IndexError::BackendViolation(format!($($message)+)).into());
            }
        }};
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            // Keep temporary owners alive while comparing their borrowed fields.
            match (&$left, &$right) {
                (left, right) if *left == *right => {}
                (left, right) => {
                    return Err(IndexError::BackendViolation(format!(
                        "test equality failed at {}:{}: {left:?} != {right:?}",
                        file!(),
                        line!()
                    ))
                    .into());
                }
            }
        }};
    }

    mod confirmed_reuse_tests;
    mod count_spill_tests;
    mod diff_trace_tests;
    mod install_envelope;
    mod inventory_trace_tests;
    mod map_read_window_tests;
    mod occurrence_blocks;
    mod occurrence_packets;
    mod occurrence_validation_trace_tests;
    mod shared_entry_maps;
    mod token_spool_batch;

    #[test]
    fn compact_contribution_has_fixed_data_all_edges_and_needs_no_object_read()
    -> Result<(), IndexError> {
        let source = reference(71);
        let a = reference(72);
        let b = reference(73);
        let mut small = entry_map::SmallMap::default();
        for (key, child) in [(b"\0a", a), (b"\0b", b), (b"\x01a", a), (b"\x01b", b)] {
            check!(small.push(key, child)?);
        }
        let logical = Contribution {
            record: source,
            inventory: EntryMap::inline(&small),
            indexed_tokens: 2,
        };
        let stored = contribution::inline(&logical, &budget()?)?.ok_or_else(|| {
            IndexError::BackendViolation("small contribution was not compacted".into())
        })?;
        let MapValue::Inline(data) = stored else {
            return Err(IndexError::BackendViolation("expected compact data".into()));
        };
        let gold = b"\x04\0\0\0\0\0\0\0\x02\x02\0a\x02\0b\x02\x01a\x02\x01b";
        check_eq!(data.data()?, gold);
        check_eq!(data.children()?, &[source, a, b, a, b]);
        // Empty store: a successful decode cannot have fetched a wrapper object.
        let objects = TestObjects::default();
        let decoded = contribution::read(stored, &objects, &mut budget()?)?;
        check_eq!(decoded.record, source);
        check_eq!(decoded.indexed_tokens, 2);
        check_eq!(decoded.inventory, logical.inventory);
        let mut unordered = gold.to_vec();
        unordered
            .get_mut(13..15)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(b"\0a");
        let mut trailing = gold.to_vec();
        trailing.push(0);
        for invalid in [
            MapValue::inline(gold, [source, a, b, a])?,
            MapValue::inline(&unordered, [source, a, b, a, b])?,
            MapValue::inline(&trailing, [source, a, b, a, b])?,
            MapValue::inline(&gold[..20], [source, a, b, a, b])?,
        ] {
            check!(matches!(
                contribution::read(invalid, &objects, &mut budget()?),
                Err(IndexError::CorruptObject(_))
            ));
        }
        let cancelled = budget()?;
        cancelled.request().cancel_handle().cancel();
        check!(matches!(
            contribution::inline(&logical, &cancelled),
            Err(IndexError::Cancelled)
        ));
        check!(small.push(b"\x01c", a)?);
        let larger = Contribution {
            inventory: EntryMap::inline(&small),
            ..logical
        };
        check!(contribution::inline(&larger, &budget()?)?.is_none());
        Ok(())
    }

    #[test]
    fn rooted_contribution_rejects_wrong_tags_width_and_child_count() -> Result<(), IndexError> {
        let objects = TestObjects::default();
        let record = reference(71);
        let root = reference(72);
        for invalid in [
            MapValue::inline(&[0x83; 9], [record, root])?,
            MapValue::inline(&[0x80; 8], [record, root])?,
            MapValue::inline(&[0x81; 10], [record, root])?,
            MapValue::inline(&[0x80; 9], [record])?,
            MapValue::inline(&[0x81; 9], [record, root, reference(73)])?,
            MapValue::inline(&[0x82; 8], [record, root])?,
            MapValue::inline(&[0x82; 9], [record])?,
        ] {
            check!(matches!(
                contribution::read(invalid, &objects, &mut budget()?),
                Err(IndexError::CorruptObject(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn occurrence_codec_preserves_wire_and_bounds_sequences_during_decode()
    -> Result<(), Box<dyn std::error::Error>> {
        let gold = br#"{"positions":[0,2]}"#;
        let decoded: Occurrence = serde_json::from_slice(gold)?;
        check_eq!(&*decoded.positions, &[0, 2]);
        check_eq!(serde_json::to_vec(&decoded)?, gold);
        for positions in [Vec::<u32>::new(), (0..8193).collect()] {
            let invalid = serde_json::json!({ "positions": positions });
            check!(serde_json::from_value::<Occurrence>(invalid.clone()).is_err());
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&invalid, &mut cbor)?;
            check!(ciborium::de::from_reader::<Occurrence, _>(cbor.as_slice()).is_err());
        }
        let boundary = serde_json::json!({ "positions": (0..8192u32).collect::<Vec<_>>() });
        check_eq!(
            serde_json::from_value::<Occurrence>(boundary)?
                .positions
                .len(),
            8192
        );
        for invalid in [
            br#"{"positions":[-1]}"#.as_slice(),
            br#"{"positions":[4294967296]}"#,
            br#"{"positions":[1.5]}"#,
        ] {
            check!(serde_json::from_slice::<Occurrence>(invalid).is_err());
        }
        Ok(())
    }

    fn source(owners: MapRoot) -> SourceCut {
        SourceCut {
            owners,
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        }
    }

    fn context(recipe: TextRecipe, owners: MapRoot) -> PlanContext {
        let policy = recipe.output_policy();
        PlanContext {
            instance: quanta_index_core::index::InstanceSpec {
                instance: IndexInstanceId([1; 16]),
                kind: IndexKind([2; 16]),
                group: PublicationGroupId([3; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: SchemaId([4; 16]),
                policy,
                format: StorageFormatId([5; 16]),
            },
            source: source(owners),
            input: InputBinding {
                root: owners,
                schema: SchemaId([4; 16]),
                policy,
                context: Digest([6; 32]),
            },
        }
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            deadline,
        ))
    }

    // Fixed work envelope for the explicit multi-owner/190-row packet oracles.
    // It funds staging, complete validation, PK promotion and cursor replay;
    // product limits and all denial fixtures keep their original envelopes.
    fn packet_budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 1024,
                writes: 1024,
                bytes: 4_000_000,
                mutations: 1024,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    // Emitted by the called test port from the budget it actually receives.
    // This detects a stage span whose child context was never passed to I/O.
    fn observe_port_budget(budget: &WorkBudget, lane: u16) -> Result<(), IndexError> {
        let diagnostic = budget.request().diagnostic_context().ok_or_else(|| {
            IndexError::BackendViolation("test port did not receive diagnostic budget".into())
        })?;
        let span = diagnostic
            .enter(DiagnosticStageV1::Operation, DiagnosticLaneV1(lane))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let _finished = span.finish(DiagnosticTerminalV1::Ok);
        Ok(())
    }

    #[test]
    fn plan_rejects_forged_source_context_before_prepare() -> Result<(), IndexError> {
        let recipe = TextRecipe::new(Digest([7; 32]));
        let index = NativeTextIndex::new(recipe);
        let old = MapRoot(Some(ObjectRef(Digest([8; 32]))));
        let next = MapRoot(Some(ObjectRef(Digest([9; 32]))));
        let forged = MapRoot(Some(ObjectRef(Digest([10; 32]))));
        let current = context(recipe, next);
        let base = MemberBinding {
            instance: current.instance.instance,
            format: current.instance.format,
            artifact: ObjectRef(Digest([11; 32])),
            input: InputBinding {
                root: old,
                ..current.input
            },
            logical_commitment: Digest([12; 32]),
        };
        let mut delta = SourceDelta {
            prior: source(old),
            next: source(forged),
            changed: OwnerMutationSet {
                root: MapRoot(None),
                count: 0,
                digest: Digest([13; 32]),
            },
        };
        check!(matches!(
            index.plan(
                BaseMember::Present(Box::new(base.clone())),
                DeltaInput::Patch(delta.clone()),
                &current,
                &mut budget()?,
            ),
            Err(IndexError::Conflict)
        ));
        delta.next = current.source;
        let mut forged_input = current.clone();
        forged_input.input.root = forged;
        check!(matches!(
            index.plan(
                BaseMember::Present(Box::new(base)),
                DeltaInput::Patch(delta),
                &forged_input,
                &mut budget()?,
            ),
            Err(IndexError::Conflict)
        ));
        let mut empty = context(recipe, MapRoot(None));
        empty.input.root = forged;
        check!(matches!(
            index.plan(
                BaseMember::Absent,
                DeltaInput::Bootstrap,
                &empty,
                &mut budget()?,
            ),
            Ok(PlanDecision::RebuildRequired)
        ));
        check!(matches!(
            index.plan(
                BaseMember::Absent,
                DeltaInput::Bootstrap,
                &current,
                &mut budget()?,
            ),
            Ok(PlanDecision::Incremental(_))
        ));
        Ok(())
    }

    #[derive(Default)]
    struct TestObjects {
        stored: BTreeMap<ObjectRef, StoredObject>,
        counted_body: Option<ObjectRef>,
        cancel_after_get: Option<ObjectRef>,
        body_reads: AtomicUsize,
        counted_occurrence: Option<ObjectRef>,
        occurrence_reads: AtomicUsize,
        installs: AtomicUsize,
        install_batches: AtomicUsize,
        install_batch_inputs: AtomicUsize,
        install_outputs: Option<Vec<ObjectRef>>,
        token_memory_at_install: Option<(Arc<TokenMemory>, u64)>,
        json_at_install: Option<JsonInstallObservation>,
        trace_install_budget: bool,
    }

    struct JsonInstallObservation {
        memory: Arc<TokenMemory>,
        payload: &'static [u8],
        refuse: bool,
    }

    impl TestObjects {
        fn add(
            &mut self,
            reference: ObjectRef,
            payload: Vec<u8>,
            children: Vec<ObjectRef>,
        ) -> Result<(), IndexError> {
            let bytes =
                u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?;
            let _previous = self.stored.insert(
                reference,
                StoredObject::new(
                    RegisteredObject::unscoped(reference, bytes, children),
                    payload,
                    None,
                ),
            );
            Ok(())
        }
    }

    impl ImmutableObjectPort for TestObjects {
        fn identify(
            &self,
            payload: &[u8],
            children: &[ObjectRef],
            budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            budget.checkpoint()?;
            Ok(RegisteredObject::unscoped(
                ObjectRef(Digest(Sha256::digest(payload).into())),
                u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
                children.to_vec(),
            ))
        }

        fn put(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test object write".into(),
            ))
        }

        fn install(
            &self,
            payload: &[u8],
            children: &[ObjectRef],
            sink: &dyn ObjectWriteSink,
            budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            budget.checkpoint()?;
            if self.trace_install_budget {
                observe_port_budget(budget, 90)?;
            }
            if let Some(observed) = &self.json_at_install {
                check_eq!(payload, observed.payload);
                check!(children.is_empty());
                check_eq!(
                    observed.memory.live.load(Ordering::Relaxed),
                    u64::try_from(observed.payload.len())
                        .map_err(|_error| IndexError::ResourceLimited)?
                );
                if observed.refuse {
                    return Err(IndexError::BackendViolation(
                        "native JSON observer refused installation".into(),
                    ));
                }
            }
            let object = RegisteredObject::unscoped(
                reference(40),
                u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
                children.to_vec(),
            );
            sink.reserve(&object)?;
            sink.installed(&object)?;
            let _previous = self.installs.fetch_add(1, Ordering::Relaxed);
            Ok(object)
        }

        fn install_batch(
            &self,
            inputs: &[ObjectBatchInput],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
            if let Some((memory, expected_live)) = &self.token_memory_at_install {
                check_eq!(memory.live.load(Ordering::Relaxed), *expected_live);
            }
            let _previous = self.install_batches.fetch_add(1, Ordering::Relaxed);
            let _previous = self
                .install_batch_inputs
                .fetch_add(inputs.len(), Ordering::Relaxed);
            if let Some(outputs) = &self.install_outputs {
                return Ok(quanta_index_core::index::RegisteredObjectBatch::unscoped(
                    outputs
                        .iter()
                        .map(|reference| RegisteredObject::unscoped(*reference, 0, Vec::new()))
                        .collect(),
                ));
            }
            Err(IndexError::BackendViolation(
                "changed-body occurrence build reached object batch".into(),
            ))
        }

        fn get(
            &self,
            reference: ObjectRef,
            budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            if self.counted_body == Some(reference) {
                let _previous = self.body_reads.fetch_add(1, Ordering::Relaxed);
            }
            if self.counted_occurrence == Some(reference) {
                let _previous = self.occurrence_reads.fetch_add(1, Ordering::Relaxed);
            }
            if self.cancel_after_get == Some(reference) {
                budget.request().cancel_handle().cancel();
            }
            self.stored
                .get(&reference)
                .cloned()
                .ok_or_else(|| IndexError::CorruptObject("missing test object".into()))
        }
    }

    #[derive(Default)]
    struct TestMaps {
        roots: BTreeMap<ObjectRef, BTreeMap<Vec<u8>, quanta_index_core::index::MapValue>>,
        empty_diff: Option<(MapRoot, MapRoot)>,
        posting_root: MapRoot,
        malformed_page: Option<MapPage>,
        counted_inventory: MapRoot,
        inventory_scans: AtomicUsize,
        counted_terms: MapRoot,
        term_lookups: AtomicUsize,
        batch_lookups: AtomicUsize,
        map_applies: AtomicUsize,
        trace_stream_budget: bool,
    }

    impl TestMaps {
        fn add(
            &mut self,
            root: MapRoot,
            key: Vec<u8>,
            value: impl Into<quanta_index_core::index::MapValue>,
        ) -> Result<(), IndexError> {
            let reference = root
                .0
                .ok_or_else(|| IndexError::BackendViolation("missing test map root".into()))?;
            let _previous = self
                .roots
                .entry(reference)
                .or_default()
                .insert(key, value.into());
            Ok(())
        }
    }

    impl PersistentMapPort for TestMaps {
        fn visit_diff(
            &self,
            before: MapRoot,
            after: MapRoot,
            range: quanta_index_core::index::ports::MapDiffRange<'_>,
            _: &mut dyn quanta_index_core::index::ports::MapDiffVisitor,
            budget: &mut WorkBudget,
        ) -> Result<bool, IndexError> {
            budget.checkpoint()?;
            if self.empty_diff == Some((before, after))
                && range.prefix.is_empty()
                && range.after.is_none()
                && range.limit == 64
            {
                return Ok(false);
            }
            Err(IndexError::BackendViolation(
                "unexpected test structural diff".into(),
            ))
        }

        fn lookup(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            if root == self.counted_terms && root.0.is_some() {
                let _previous = self.term_lookups.fetch_add(1, Ordering::Relaxed);
            }
            Ok(root
                .0
                .and_then(|reference| self.roots.get(&reference))
                .and_then(|entries| entries.get(key))
                .copied())
        }

        fn lookup_batch(
            &self,
            root: MapRoot,
            keys: &[&[u8]],
            values: &mut [Option<MapValue>],
            budget: &mut WorkBudget,
        ) -> Result<(), IndexError> {
            if keys.len() > quanta_index_core::index::ports::MAX_MAP_LOOKUP_BATCH
                || keys.len() != values.len()
            {
                return Err(IndexError::InvalidInput(
                    "test map batch shape differs".into(),
                ));
            }
            budget.charge(WorkUsage {
                bytes: u64::try_from(
                    keys.len()
                        .checked_mul(std::mem::size_of::<Option<MapValue>>())
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let _previous = self.batch_lookups.fetch_add(1, Ordering::Relaxed);
            for (key, value) in keys.iter().zip(values) {
                *value = self.lookup(root, key, budget)?;
            }
            Ok(())
        }

        fn scan(
            &self,
            root: MapRoot,
            after: Option<&[u8]>,
            limit: u32,
            budget: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            if root == self.counted_inventory && root.0.is_some() {
                let _previous = self.inventory_scans.fetch_add(1, Ordering::Relaxed);
            }
            if root == self.posting_root
                && let Some(page) = &self.malformed_page
            {
                return Ok(page.clone());
            }
            let mut page = Vec::new();
            let mut has_more = false;
            if let Some(entries) = root.0.and_then(|reference| self.roots.get(&reference)) {
                for (key, value) in entries {
                    if after.is_some_and(|prior| key.as_slice() <= prior) {
                        continue;
                    }
                    if page.len()
                        == usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?
                    {
                        has_more = true;
                        break;
                    }
                    page.push((key.clone(), *value));
                }
            }
            Ok(MapPage::new(page, has_more, Vec::new(), None))
        }

        fn apply(
            &self,
            _: MapRoot,
            _: &[MapEdit],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<MapWrite, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test map write".into(),
            ))
        }

        fn apply_batch(
            &self,
            _: &[MapBatchRequest],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<MapBatchWrite, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test map write".into(),
            ))
        }

        fn apply_stream(
            &self,
            _: &[MapRoot],
            _: &mut dyn MapEditCursor,
            _: &dyn ObjectWriteSink,
            budget: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
            if self.trace_stream_budget {
                observe_port_budget(budget, 91)?;
            }
            let _previous = self.map_applies.fetch_add(1, Ordering::Relaxed);
            Err(IndexError::BackendViolation(
                "unexpected test map write".into(),
            ))
        }
    }

    #[derive(Default)]
    struct TestScratch {
        allocations: AtomicUsize,
        reserve_observations: Arc<AtomicUsize>,
    }
    struct TestAllocation(tempfile::TempDir, u64, Arc<AtomicUsize>);

    impl ScratchAllocation for TestAllocation {
        fn path(&self) -> &Path {
            self.0.path()
        }

        fn max_bytes(&self) -> u64 {
            self.1
        }

        fn reserved_bytes(&self) -> Result<u64, IndexError> {
            let _previous = self.2.fetch_add(1, Ordering::Relaxed);
            // This fixture admits its whole envelope; storage tests cover growth.
            Ok(self.1)
        }

        fn reserve_at_least(&self, bytes: u64, budget: &mut WorkBudget) -> Result<(), IndexError> {
            budget.checkpoint()?;
            if bytes > self.1 {
                return Err(IndexError::ResourceLimited);
            }
            Ok(())
        }
    }

    impl ScratchSpacePort for TestScratch {
        fn allocate(
            &self,
            request: ScratchRequest,
            budget: &mut WorkBudget,
        ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
            budget.checkpoint()?;
            let _previous = self.allocations.fetch_add(1, Ordering::Relaxed);
            let directory = tempfile::tempdir()
                .map_err(|error| IndexError::Storage(format!("test scratch: {error}")))?;
            Ok(Box::new(TestAllocation(
                directory,
                request.max_bytes,
                Arc::clone(&self.reserve_observations),
            )))
        }
    }

    struct TestCustody;

    impl ObjectCustodyPort for TestCustody {
        fn reserve_objects(&self, _: AttemptRef, _: &[RegisteredObject]) -> Result<(), IndexError> {
            Ok(())
        }

        fn register_objects(
            &self,
            _: AttemptRef,
            _: &[RegisteredObject],
        ) -> Result<(), IndexError> {
            Ok(())
        }
    }

    fn test_attempt(workspace: WorkspaceKey) -> AttemptRef {
        AttemptRef {
            operation: OperationKey {
                workspace,
                epoch: 1,
                producer: ProducerId([22; 16]),
                stream: StreamId([33; 16]),
                stream_epoch: 1,
                sequence: 1,
            },
            attempt: AttemptId([34; 16]),
            fence: 1,
        }
    }

    fn reference(byte: u8) -> ObjectRef {
        ObjectRef(Digest([byte; 32]))
    }

    struct VisitorFixture {
        objects: TestObjects,
        maps: TestMaps,
        ticket: TicketRecord,
        member: MemberBinding,
        member_ref: ObjectRef,
        recipe: TextRecipe,
        owner: OwnerKey,
        source_ref: ObjectRef,
        occurrence_ref: ObjectRef,
        inventory_root: MapRoot,
        term: Vec<u8>,
        posting_ref: ObjectRef,
        posting_root: MapRoot,
    }

    fn visitor_fixture() -> Result<VisitorFixture, IndexError> {
        let recipe = TextRecipe::new(Digest([21; 32]));
        let owner = OwnerKey {
            producer: ProducerId([22; 16]),
            stable_id: StableOwnerId([23; 16]),
        };
        let source_root = MapRoot(Some(reference(1)));
        let members_root = MapRoot(Some(reference(2)));
        let owners_root = MapRoot(Some(reference(3)));
        let terms_root = MapRoot(Some(reference(4)));
        let posting_root = MapRoot(Some(reference(5)));
        let inventory_root = MapRoot(Some(reference(6)));
        let body_ref = reference(7);
        let source_ref = reference(8);
        let occurrence_ref = reference(9);
        let contribution_ref = reference(10);
        let posting_ref = reference(11);
        let artifact = reference(12);
        let member_ref = reference(13);
        let term = stored_term_key("one", CaseMode::Sensitive)?;
        let mut objects = TestObjects::default();
        objects.add(body_ref, b"one".to_vec(), Vec::new())?;
        let record = SourceRecord {
            path: "a".into(),
            bytes: body_ref,
            byte_len: 3,
            text: SourceTextAdmission::Utf8 {
                policy: recipe.admission_policy,
            },
        };
        objects.add(source_ref, encode(&record)?, vec![body_ref])?;
        let occurrence = Occurrence {
            // A dropped over-long run at position 1 does not reduce the
            // position of this second indexed occurrence.
            positions: Positions(vec![0, 2]),
        };
        objects.add(occurrence_ref, encode(&occurrence)?, Vec::new())?;
        let contribution = Contribution {
            record: source_ref,
            inventory: inventory_root.into(),
            indexed_tokens: 2,
        };
        objects.add(
            contribution_ref,
            encode(&contribution)?,
            contribution_children(&contribution)?.as_slice().to_vec(),
        )?;
        let posting = Posting {
            root: posting_root.into(),
            df: 1,
        };
        objects.add(
            posting_ref,
            encode(&posting)?,
            posting_children(&posting)?.as_slice().to_vec(),
        )?;
        let root = Root {
            owners: owners_root,
            terms: terms_root,
            docs: 1,
            indexed_tokens: 2,
        };
        let payload = encode(&root)?;
        objects.add(
            artifact,
            payload.clone(),
            root_children(root).as_slice().to_vec(),
        )?;
        let plan = context(recipe, source_root);
        let member = MemberBinding {
            instance: plan.instance.instance,
            format: plan.instance.format,
            artifact,
            input: plan.input,
            logical_commitment: Digest(Sha256::digest(&payload).into()),
        };
        objects.add(member_ref, encode(&member)?, vec![artifact, reference(1)])?;
        let mut maps = TestMaps {
            posting_root,
            counted_terms: terms_root,
            ..TestMaps::default()
        };
        maps.add(source_root, owner_map_key(owner).to_vec(), source_ref)?;
        maps.add(
            members_root,
            {
                let mut key = b"member\0".to_vec();
                key.extend_from_slice(&member.instance.0);
                key
            },
            member_ref,
        )?;
        maps.add(owners_root, owner_key(owner).to_vec(), contribution_ref)?;
        maps.add(terms_root, term.clone(), posting_ref)?;
        maps.add(
            posting_root,
            owner_key(owner).to_vec(),
            OccurrenceDigestWitness::from_reference(occurrence_ref).into_value()?,
        )?;
        maps.add(inventory_root, term.clone(), occurrence_ref)?;
        let ticket = TicketRecord {
            lease: ReadTicketLease {
                id: TicketId([24; 16]),
                session: ReadSessionRef { id: 1, fence: 1 },
                fence: 1,
                expires_at_ms: 100,
            },
            view: CatalogView {
                identity: ViewRef {
                    workspace: WorkspaceKey {
                        tenant: TenantId([25; 16]),
                        collection: CollectionId([26; 16]),
                        worktree: WorktreeId([27; 16]),
                        slot: SlotId([28; 16]),
                    },
                    epoch: 1,
                    sequence: 1,
                    digest: Digest([29; 32]),
                },
                profile: reference(30),
                source: source(source_root),
                members: members_root,
                coverage: MapRoot(None),
                projections: MapRoot(None),
            },
        };
        Ok(VisitorFixture {
            objects,
            maps,
            ticket,
            member,
            member_ref,
            recipe,
            owner,
            source_ref,
            occurrence_ref,
            inventory_root,
            term,
            posting_ref,
            posting_root,
        })
    }

    #[test]
    fn authenticated_text_member_reuses_single_get_and_refuses_different_view()
    -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        fixture.objects.counted_body = Some(fixture.member_ref);
        let index = NativeTextIndex::new(fixture.recipe);
        let instance = context(fixture.recipe, fixture.ticket.view.source.owners).instance;
        let mut work = budget()?;
        let bound = BoundTextMember::from_context(&mut QueryReadContext {
            ticket: &fixture.ticket,
            instance: &instance,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut work,
        })?;
        let mut different = fixture.ticket.clone();
        different.view.identity.digest = Digest([99; 32]);
        let mut text = TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut work,
        };
        let root = bound.root(&index, &mut text)?;
        check_eq!(fixture.objects.body_reads.load(Ordering::Relaxed), 1);
        let direct = index.pinned_root(&fixture.member, &mut text)?;
        check_eq!(fixture.objects.body_reads.load(Ordering::Relaxed), 2);
        check_eq!(root.owners, direct.owners);
        check_eq!(root.terms, direct.terms);

        text.ticket = &different;
        check!(matches!(
            bound.root(&index, &mut text),
            Err(IndexError::Conflict)
        ));
        check_eq!(fixture.objects.body_reads.load(Ordering::Relaxed), 2);
        Ok(())
    }

    // The positions are fixture facts, independent of the phrase executor.
    // One/two/gap/one/one/three has an adjacent, a reversed, a gapped and a
    // repeated-token witness while preserving the existing pinned member path.
    fn phrase_fixture() -> Result<VisitorFixture, IndexError> {
        let mut fixture = visitor_fixture()?;
        let body = b"one two gap one one three";
        let mut record = fixture_record(&fixture)?;
        record.byte_len =
            u64::try_from(body.len()).map_err(|_error| IndexError::ResourceLimited)?;
        fixture
            .objects
            .add(record.bytes, body.to_vec(), Vec::new())?;
        fixture
            .objects
            .add(fixture.source_ref, encode(&record)?, vec![record.bytes])?;
        fixture.objects.add(
            fixture.occurrence_ref,
            encode(&Occurrence {
                positions: Positions(vec![0, 3, 4]),
            })?,
            Vec::new(),
        )?;
        let terms = [
            ("two", vec![1], 41, 42, 43),
            ("gap", vec![2], 44, 45, 46),
            ("three", vec![5], 47, 48, 49),
        ];
        for (term, positions, occurrence_id, root_id, posting_id) in terms {
            let key = stored_term_key(term, CaseMode::Sensitive)?;
            let occurrence_ref = reference(occurrence_id);
            let posting_root = MapRoot(Some(reference(root_id)));
            let posting_ref = reference(posting_id);
            fixture.objects.add(
                occurrence_ref,
                encode(&Occurrence {
                    positions: Positions(positions),
                })?,
                Vec::new(),
            )?;
            let posting = Posting {
                root: posting_root.into(),
                df: 1,
            };
            fixture.objects.add(
                posting_ref,
                encode(&posting)?,
                posting_children(&posting)?.as_slice().to_vec(),
            )?;
            fixture.maps.add(
                posting_root,
                owner_key(fixture.owner).to_vec(),
                OccurrenceDigestWitness::from_reference(occurrence_ref).into_value()?,
            )?;
            fixture
                .maps
                .add(MapRoot(Some(reference(4))), key.clone(), posting_ref)?;
            fixture
                .maps
                .add(fixture.inventory_root, key, occurrence_ref)?;
        }
        let contribution = Contribution {
            record: fixture.source_ref,
            inventory: fixture.inventory_root.into(),
            indexed_tokens: 6,
        };
        fixture.objects.add(
            reference(10),
            encode(&contribution)?,
            contribution_children(&contribution)?.as_slice().to_vec(),
        )?;
        phrase_fixture_root(&mut fixture, 1, 6)?;
        Ok(fixture)
    }

    fn phrase_fixture_root(
        fixture: &mut VisitorFixture,
        docs: u64,
        indexed_tokens: u64,
    ) -> Result<(), IndexError> {
        let root = Root {
            owners: MapRoot(Some(reference(3))),
            terms: MapRoot(Some(reference(4))),
            docs,
            indexed_tokens,
        };
        let payload = encode(&root)?;
        fixture.objects.add(
            fixture.member.artifact,
            payload.clone(),
            root_children(root).as_slice().to_vec(),
        )?;
        fixture.member.logical_commitment = Digest(Sha256::digest(&payload).into());
        fixture.objects.add(
            fixture.member_ref,
            encode(&fixture.member)?,
            vec![fixture.member.artifact, reference(1)],
        )?;
        Ok(())
    }

    fn two_owner_phrase_fixture() -> Result<(VisitorFixture, OwnerKey), IndexError> {
        let mut fixture = phrase_fixture()?;
        let second = OwnerKey {
            producer: fixture.owner.producer,
            stable_id: StableOwnerId([31; 16]),
        };
        let source = reference(71);
        let inventory = MapRoot(Some(reference(72)));
        let record = SourceRecord {
            path: "b".into(),
            bytes: reference(70),
            byte_len: 7,
            text: SourceTextAdmission::Utf8 {
                policy: fixture.recipe.admission_policy,
            },
        };
        fixture
            .objects
            .add(record.bytes, b"one two".to_vec(), Vec::new())?;
        fixture
            .objects
            .add(source, encode(&record)?, vec![record.bytes])?;
        let contribution = Contribution {
            record: source,
            inventory: inventory.into(),
            indexed_tokens: 2,
        };
        fixture.objects.add(
            reference(73),
            encode(&contribution)?,
            contribution_children(&contribution)?.as_slice().to_vec(),
        )?;
        fixture.maps.add(
            fixture.ticket.view.source.owners,
            owner_map_key(second).to_vec(),
            source,
        )?;
        fixture.maps.add(
            MapRoot(Some(reference(3))),
            owner_key(second).to_vec(),
            reference(73),
        )?;
        for (term, occurrence_id, posting_root, posting_ref) in [
            ("one", 74, fixture.posting_root, fixture.posting_ref),
            ("two", 75, MapRoot(Some(reference(42))), reference(43)),
        ] {
            let occurrence_ref = reference(occurrence_id);
            let key = stored_term_key(term, CaseMode::Sensitive)?;
            let positions = if term == "one" { vec![0] } else { vec![1] };
            fixture.objects.add(
                occurrence_ref,
                encode(&Occurrence {
                    positions: Positions(positions),
                })?,
                Vec::new(),
            )?;
            fixture.maps.add(inventory, key, occurrence_ref)?;
            fixture.maps.add(
                posting_root,
                owner_key(second).to_vec(),
                OccurrenceDigestWitness::from_reference(occurrence_ref).into_value()?,
            )?;
            let posting = Posting {
                root: posting_root.into(),
                df: 2,
            };
            fixture.objects.add(
                posting_ref,
                encode(&posting)?,
                posting_children(&posting)?.as_slice().to_vec(),
            )?;
        }
        phrase_fixture_root(&mut fixture, 2, 8)?;
        Ok((fixture, second))
    }

    fn seed_phrase_source_paths(
        fixture: &mut VisitorFixture,
        second: OwnerKey,
    ) -> Result<(), IndexError> {
        let paths = MapRoot(Some(reference(80)));
        for (owner, source) in [(fixture.owner, fixture.source_ref), (second, reference(71))] {
            let stored = fixture
                .objects
                .stored
                .get(&source)
                .ok_or_else(|| IndexError::CorruptObject("fixture source missing".into()))?;
            let record: SourceRecord = decode(&stored.payload)?;
            let mut key = b"path\0".to_vec();
            key.extend_from_slice(record.path.as_bytes());
            fixture.maps.add(
                paths,
                key,
                quanta_index_core::index::path_owner_map_value(owner)?,
            )?;
        }
        fixture.ticket.view.source.paths = paths;
        Ok(())
    }

    fn fixture_record(fixture: &VisitorFixture) -> Result<SourceRecord, IndexError> {
        let stored = fixture
            .objects
            .stored
            .get(&fixture.source_ref)
            .ok_or_else(|| IndexError::CorruptObject("fixture source missing".into()))?;
        decode(&stored.payload)
    }

    fn drive_change(
        fixture: &VisitorFixture,
        change: &SourceChange,
    ) -> Result<(Root, Option<quanta_index_core::index::OrdinalMapEdit>), IndexError> {
        let stored = fixture
            .objects
            .stored
            .get(&fixture.member.artifact)
            .ok_or_else(|| IndexError::CorruptObject("fixture root missing".into()))?;
        let mut root: Root = decode(&stored.payload)?;
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut work = budget()?;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut context = AttemptContext {
            attempt: test_attempt(fixture.ticket.view.identity.workspace),
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let mut contributions = ContributionWrites::new()?;
        let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
        process_change(
            change,
            &mut root,
            fixture.recipe,
            &mut context,
            &spool,
            &mut contributions,
            &mut inventories,
        )?;
        contributions.flush(&mut context, &spool)?;
        let edit = spool.top_cursor().next_edit(&mut work)?;
        Ok((root, edit))
    }

    fn ready<T>(mut future: IndexFuture<'_, T>) -> Result<T, IndexError> {
        let mut context = Context::from_waker(Waker::noop());
        match Future::poll(future.as_mut(), &mut context) {
            Poll::Ready(result) => result,
            Poll::Pending => Err(IndexError::BackendViolation(
                "native text test future unexpectedly pending".into(),
            )),
        }
    }

    fn empty_patch_plan(
        fixture: &VisitorFixture,
        changed: OwnerMutationSet,
    ) -> Result<TextPlan, IndexError> {
        let index = NativeTextIndex::new(fixture.recipe);
        let current = context(fixture.recipe, fixture.ticket.view.source.owners);
        match index.plan(
            BaseMember::Present(Box::new(fixture.member.clone())),
            DeltaInput::Patch(SourceDelta {
                prior: current.source,
                next: current.source,
                changed,
            }),
            &current,
            &mut budget()?,
        )? {
            PlanDecision::Incremental(plan) => Ok(plan),
            PlanDecision::Reuse(_)
            | PlanDecision::RebuildRequired
            | PlanDecision::MaintenanceRequired => Err(IndexError::BackendViolation(
                "empty patch did not produce an incremental text plan".into(),
            )),
        }
    }

    #[test]
    fn verified_empty_patch_reuses_text_artifact_without_writes() -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        let inconsistent_root = MapRoot(Some(ObjectRef(Digest([0x23; 32]))));
        fixture.maps.add(
            inconsistent_root,
            owner_map_key(fixture.owner).to_vec(),
            fixture.source_ref,
        )?;
        fixture.maps.empty_diff = Some((fixture.ticket.view.source.owners, inconsistent_root));
        let plan = empty_patch_plan(
            &fixture,
            OwnerMutationSet {
                root: MapRoot(None),
                count: 0,
                digest: OwnerMutationDigest::default().finish(),
            },
        )?;
        let prior_context = plan.context.clone();
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut work = budget()?;
        let mut attempt = AttemptContext {
            attempt: test_attempt(fixture.ticket.view.identity.workspace),
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let index = NativeTextIndex::new(fixture.recipe);
        let candidate = ready(index.prepare(plan, &mut attempt))?;
        check_eq!(candidate.binding, fixture.member);
        check_eq!(
            ready(index.verify_and_seal(candidate, &mut attempt))?,
            fixture.member
        );
        check_eq!(scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        check_eq!(fixture.maps.map_applies.load(Ordering::Relaxed), 0);

        let mut next_context = prior_context.clone();
        next_context.source.frontiers = MapRoot(Some(ObjectRef(Digest([0x21; 32]))));
        next_context.input.context = Digest([0x22; 32]);
        let PlanDecision::Incremental(next_plan) = index.plan(
            BaseMember::Present(Box::new(fixture.member.clone())),
            DeltaInput::Patch(SourceDelta {
                prior: prior_context.source,
                next: next_context.source,
                changed: OwnerMutationSet {
                    root: MapRoot(None),
                    count: 0,
                    digest: OwnerMutationDigest::default().finish(),
                },
            }),
            &next_context,
            attempt.budget,
        )?
        else {
            return Err(IndexError::BackendViolation(
                "frontier-only patch was rejected".into(),
            ));
        };
        let rebound = ready(index.prepare(next_plan, &mut attempt))?;
        check_eq!(rebound.binding.artifact, fixture.member.artifact);
        check_eq!(
            rebound.binding.logical_commitment,
            fixture.member.logical_commitment
        );
        check_eq!(rebound.binding.input, next_context.input);
        if rebound.binding == fixture.member {
            return Err(IndexError::BackendViolation(
                "rebound member unexpectedly reused the prior binding".into(),
            ));
        }
        check_eq!(
            ready(index.verify_and_seal(rebound, &mut attempt))?.input,
            next_context.input
        );
        check_eq!(scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
        check_eq!(fixture.maps.map_applies.load(Ordering::Relaxed), 0);

        let mut inconsistent_context = prior_context.clone();
        inconsistent_context.source.owners = inconsistent_root;
        inconsistent_context.input.root = inconsistent_context.source.owners;
        let PlanDecision::Incremental(inconsistent_plan) = index.plan(
            BaseMember::Present(Box::new(fixture.member.clone())),
            DeltaInput::Patch(SourceDelta {
                prior: prior_context.source,
                next: inconsistent_context.source,
                changed: OwnerMutationSet {
                    root: MapRoot(None),
                    count: 0,
                    digest: OwnerMutationDigest::default().finish(),
                },
            }),
            &inconsistent_context,
            attempt.budget,
        )?
        else {
            return Err(IndexError::BackendViolation(
                "inconsistent patch was rejected before prepare".into(),
            ));
        };
        check!(matches!(
            ready(index.prepare(inconsistent_plan, &mut attempt)),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(scratch.allocations.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn forged_empty_patch_digest_is_refused_before_text_scratch() -> Result<(), IndexError> {
        let fixture = visitor_fixture()?;
        let plan = empty_patch_plan(
            &fixture,
            OwnerMutationSet {
                root: MapRoot(None),
                count: 0,
                digest: Digest([0xff; 32]),
            },
        )?;
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut work = budget()?;
        let mut attempt = AttemptContext {
            attempt: test_attempt(fixture.ticket.view.identity.workspace),
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let index = NativeTextIndex::new(fixture.recipe);
        check!(matches!(
            ready(index.prepare(plan, &mut attempt)),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
        check_eq!(fixture.maps.map_applies.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn prepare_trace_preserves_result_and_work_and_closes_error_span() -> Result<(), IndexError> {
        use quanta_index_core::DiagnosticCounterV1;

        #[derive(Debug, Default)]
        struct CountingClock(AtomicU64, AtomicU64);

        impl DiagnosticClockV1 for CountingClock {
            fn now_ns(&self) -> u64 {
                self.0.fetch_add(1, Ordering::SeqCst)
            }

            fn thread_cpu_now_ns(&self) -> Option<u64> {
                Some(self.1.fetch_add(1, Ordering::SeqCst))
            }
        }

        #[derive(Debug, Default)]
        struct RecordingSink(Mutex<Vec<DiagnosticEmitV1>>);

        impl DiagnosticEventSinkV1 for RecordingSink {
            fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
                let Ok(mut events) = self.0.try_lock() else {
                    return false;
                };
                if events.len() == 128 {
                    return false;
                }
                events.push(event);
                true
            }
        }

        let mut fixture = visitor_fixture()?;
        let index = NativeTextIndex::new(fixture.recipe);
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let bootstrap = TextPlan {
            base: None,
            context: context(fixture.recipe, MapRoot(None)),
            input: DeltaInput::Bootstrap,
        };
        // A non-null source root whose existing fake scan returns no rows
        // reaches the empty postings apply path; TestMaps then refuses its
        // stream install. This checks span closure without a new fake map.
        let empty_postings = TextPlan {
            base: None,
            context: context(fixture.recipe, MapRoot(Some(reference(200)))),
            input: DeltaInput::Bootstrap,
        };
        let clock = Arc::new(CountingClock::default());
        let sink = Arc::new(RecordingSink::default());
        let limits = WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 1_000_000,
            mutations: 100,
        };

        let mut off_work = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        let off_candidate = ready(index.prepare(
            bootstrap.clone(),
            &mut AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut off_work,
            },
        ))?;
        let mut off_postings_work = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        check!(matches!(
            ready(index.prepare(
                empty_postings.clone(),
                &mut AttemptContext {
                    attempt: test_attempt(fixture.ticket.view.identity.workspace),
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    custody: &custody,
                    scratch: &scratch,
                    budget: &mut off_postings_work,
                },
            )),
            Err(IndexError::BackendViolation(message)) if message == "unexpected test map write"
        ));
        check_eq!(clock.0.load(Ordering::SeqCst), 0);
        check_eq!(clock.1.load(Ordering::SeqCst), 0);

        fixture.objects.trace_install_budget = true;
        fixture.maps.trace_stream_budget = true;

        let trace =
            DiagnosticContextV1::new(DiagnosticCaptureIdV1([31; 16]), clock.clone(), sink.clone());
        let root = trace
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let diagnostic = trace.child_context(&root);
        let mut recorded_work = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30))
                .with_diagnostic_context(diagnostic.clone()),
        );
        let recorded_candidate = ready(index.prepare(
            bootstrap,
            &mut AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut recorded_work,
            },
        ))?;
        check_eq!(recorded_candidate.binding, off_candidate.binding);
        check_eq!(recorded_work.usage(), off_work.usage());
        check!(clock.0.load(Ordering::SeqCst) > 0);
        let restored_success = recorded_work
            .request()
            .diagnostic_context()
            .ok_or_else(|| IndexError::BackendViolation("successful prepare lost trace".into()))?
            .enter(DiagnosticStageV1::Operation, DiagnosticLaneV1(92))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let _finished = restored_success.finish(DiagnosticTerminalV1::Ok);

        let mut recorded_postings_work = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30))
                .with_diagnostic_context(diagnostic.clone()),
        );
        check!(matches!(
            ready(index.prepare(
                empty_postings,
                &mut AttemptContext {
                    attempt: test_attempt(fixture.ticket.view.identity.workspace),
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    custody: &custody,
                    scratch: &scratch,
                    budget: &mut recorded_postings_work,
                },
            )),
            Err(IndexError::BackendViolation(message)) if message == "unexpected test map write"
        ));
        check_eq!(recorded_postings_work.usage(), off_postings_work.usage());

        let rejected = empty_patch_plan(
            &fixture,
            OwnerMutationSet {
                root: MapRoot(None),
                count: 0,
                digest: Digest([0xff; 32]),
            },
        )?;
        let mut off_error_work = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        check!(matches!(
            ready(index.prepare(
                rejected.clone(),
                &mut AttemptContext {
                    attempt: test_attempt(fixture.ticket.view.identity.workspace),
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    custody: &custody,
                    scratch: &scratch,
                    budget: &mut off_error_work,
                },
            )),
            Err(IndexError::CorruptObject(_))
        ));
        let mut recorded_error_work = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30))
                .with_diagnostic_context(diagnostic),
        );
        check!(matches!(
            ready(index.prepare(
                rejected,
                &mut AttemptContext {
                    attempt: test_attempt(fixture.ticket.view.identity.workspace),
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    custody: &custody,
                    scratch: &scratch,
                    budget: &mut recorded_error_work,
                },
            )),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(recorded_error_work.usage(), off_error_work.usage());
        let restored_error = recorded_error_work
            .request()
            .diagnostic_context()
            .ok_or_else(|| IndexError::BackendViolation("failed prepare lost trace".into()))?
            .enter(DiagnosticStageV1::Operation, DiagnosticLaneV1(93))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let _finished = restored_error.finish(DiagnosticTerminalV1::Ok);
        let root_id = root.span_id();
        root.finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;

        let events = sink
            .0
            .lock()
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let mut stages = BTreeMap::new();
        let mut parents = BTreeMap::new();
        let mut port_spans = Vec::new();
        let mut lexical_outcomes = Vec::new();
        let mut postings_outcomes = Vec::new();
        let mut postings_parent = None;
        let mut posting_steps = Vec::new();
        let mut root_outcomes = Vec::new();
        for event in events.iter() {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id,
                stage,
                lane,
                ..
            } = event.kind
            {
                let _previous = stages.insert(span_id, stage);
                let _previous = parents.insert(span_id, parent_span_id);
                if stage == DiagnosticStageV1::Operation && matches!(lane.0, 90 | 91) {
                    port_spans.push((lane.0, parent_span_id));
                } else if stage == DiagnosticStageV1::NativeTextPostings && lane.0 == 2 {
                    check!(postings_parent.replace(span_id).is_none());
                } else if stage == DiagnosticStageV1::NativeTextPostings
                    && matches!(lane.0, 35..=39)
                {
                    posting_steps.push((lane.0, parent_span_id, span_id));
                }
            } else if let DiagnosticEventKindV1::SpanEnd { span_id, outcome } = event.kind {
                let stage = stages.get(&span_id);
                if stage == Some(&DiagnosticStageV1::LexicalBuild) {
                    lexical_outcomes.push(outcome);
                } else if stage == Some(&DiagnosticStageV1::NativeTextPostings)
                    && postings_parent == Some(span_id)
                {
                    postings_outcomes.push(outcome);
                } else if stage == Some(&DiagnosticStageV1::NativeTextRoot) {
                    root_outcomes.push(outcome);
                }
            }
        }
        check_eq!(
            lexical_outcomes,
            vec![
                DiagnosticTerminalV1::Ok,
                DiagnosticTerminalV1::Error { code: 1 },
                DiagnosticTerminalV1::Error { code: 1 }
            ]
        );
        check_eq!(
            postings_outcomes,
            vec![DiagnosticTerminalV1::Error { code: 1 }]
        );
        check_eq!(
            posting_steps
                .iter()
                .map(|(lane, _, _)| *lane)
                .collect::<Vec<_>>(),
            vec![35, 39]
        );
        for (lane, parent, span_id) in posting_steps {
            check_eq!(parent, postings_parent);
            let terminal = events
                .iter()
                .filter_map(|event| {
                    if let DiagnosticEventKindV1::SpanEnd {
                        span_id: ended,
                        outcome,
                    } = event.kind
                    {
                        (ended == span_id).then_some(outcome)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            check_eq!(
                terminal,
                vec![if lane == 35 {
                    DiagnosticTerminalV1::Ok
                } else {
                    DiagnosticTerminalV1::Error { code: 1 }
                }]
            );
            let cpu = events
                .iter()
                .filter_map(|event| {
                    if let DiagnosticEventKindV1::Counter {
                        span_id: owner,
                        key: DiagnosticCounterV1::ThreadCpuNs,
                        value,
                    } = event.kind
                    {
                        (owner == span_id).then_some(value)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            check_eq!(cpu, vec![1]);
        }
        check_eq!(root_outcomes, vec![DiagnosticTerminalV1::Ok]);
        check_eq!(port_spans.len(), 2);
        let lexical_spans = events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    stage: DiagnosticStageV1::LexicalBuild,
                    ..
                } = event.kind
                {
                    Some(span_id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        check_eq!(lexical_spans.len(), 3);
        let keys = [
            DiagnosticCounterV1::WorkReads,
            DiagnosticCounterV1::WorkWrites,
            DiagnosticCounterV1::WorkBytes,
            DiagnosticCounterV1::WorkMutations,
        ];
        // Compare the actual prepare envelope with its original work budget,
        // including both failure outcomes. Nested stages must not be summed.
        for (span_id, expected) in lexical_spans.into_iter().zip([
            recorded_work.usage(),
            recorded_postings_work.usage(),
            recorded_error_work.usage(),
        ]) {
            let end = events
                .iter()
                .position(|event| {
                    matches!(
                        event.kind,
                        DiagnosticEventKindV1::SpanEnd { span_id: ended, .. } if ended == span_id
                    )
                })
                .ok_or_else(|| IndexError::BackendViolation("prepare terminal absent".into()))?;
            let mut observed = [None; 4];
            for (event_index, event) in events.iter().enumerate() {
                if let DiagnosticEventKindV1::Counter {
                    span_id: owner,
                    key,
                    value,
                } = event.kind
                    && owner == span_id
                    && let Some(slot) = keys.iter().position(|known| *known == key)
                {
                    check!(event_index < end);
                    check!(
                        observed
                            .get_mut(slot)
                            .ok_or(IndexError::ResourceLimited)?
                            .replace(value)
                            .is_none()
                    );
                }
            }
            check_eq!(
                observed,
                [
                    expected.reads,
                    expected.writes,
                    expected.bytes,
                    expected.mutations
                ]
                .map(Some)
            );
        }
        for lane in [92, 93] {
            check!(events.iter().any(|event| matches!(
                event.kind,
                DiagnosticEventKindV1::SpanBegin {
                    parent_span_id: Some(parent),
                    stage: DiagnosticStageV1::Operation,
                    lane: DiagnosticLaneV1(observed),
                    ..
                } if observed == lane && parent == root_id
            )));
        }
        drop(events);
        for (lane, parent) in port_spans {
            let expected = if lane == 90 {
                DiagnosticStageV1::NativeTextRoot
            } else {
                DiagnosticStageV1::NativeTextPostings
            };
            let stage_id = parent.ok_or_else(|| {
                IndexError::BackendViolation("test port lost its stage parent".into())
            })?;
            check_eq!(stages.get(&stage_id), Some(&expected));
            let outer_id = if lane == 91 {
                let outer = parents.get(&stage_id).copied().flatten().ok_or_else(|| {
                    IndexError::BackendViolation("posting step lost its parent".into())
                })?;
                check_eq!(Some(outer), postings_parent);
                outer
            } else {
                stage_id
            };
            let lexical_id = parents.get(&outer_id).copied().flatten().ok_or_else(|| {
                IndexError::BackendViolation("native text stage lost lexical parent".into())
            })?;
            check_eq!(
                stages.get(&lexical_id),
                Some(&DiagnosticStageV1::LexicalBuild)
            );
        }
        Ok(())
    }

    #[test]
    fn immutable_noop_and_rename_do_not_read_body_or_scan_inventory() -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        let prior = fixture_record(&fixture)?;
        fixture.objects.counted_body = Some(prior.bytes);
        fixture.maps.counted_inventory = fixture.inventory_root;
        let no_op = SourceChange {
            owner: fixture.owner,
            prior: Some((
                fixture.source_ref,
                SourceRecordOwner::new_unscoped(prior.clone()),
            )),
            next: Some((
                fixture.source_ref,
                SourceRecordOwner::new_unscoped(prior.clone()),
            )),
        };
        let (root, edit) = drive_change(&fixture, &no_op)?;
        check!(edit.is_none());
        check_eq!(root.docs, 1);
        check_eq!(root.indexed_tokens, 2);
        check_eq!(fixture.objects.body_reads.load(Ordering::Relaxed), 0);
        check_eq!(fixture.maps.inventory_scans.load(Ordering::Relaxed), 0);
        check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
        let mut forged_same_ref = prior.clone();
        forged_same_ref.path = "forged".into();
        check!(matches!(
            drive_change(
                &fixture,
                &SourceChange {
                    owner: fixture.owner,
                    prior: Some((
                        fixture.source_ref,
                        SourceRecordOwner::new_unscoped(prior.clone())
                    )),
                    next: Some((
                        fixture.source_ref,
                        SourceRecordOwner::new_unscoped(forged_same_ref)
                    )),
                }
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(fixture.objects.body_reads.load(Ordering::Relaxed), 0);
        check_eq!(fixture.maps.inventory_scans.load(Ordering::Relaxed), 0);

        let mut renamed = prior.clone();
        renamed.path = "b".into();
        let renamed_ref = reference(41);
        fixture
            .objects
            .add(renamed_ref, encode(&renamed)?, vec![renamed.bytes])?;
        let (root, edit) = drive_change(
            &fixture,
            &SourceChange {
                owner: fixture.owner,
                prior: Some((fixture.source_ref, SourceRecordOwner::new_unscoped(prior))),
                next: Some((renamed_ref, SourceRecordOwner::new_unscoped(renamed))),
            },
        )?;
        check_eq!(root.docs, 1);
        check_eq!(root.indexed_tokens, 2);
        check_eq!(fixture.objects.body_reads.load(Ordering::Relaxed), 0);
        check_eq!(fixture.maps.inventory_scans.load(Ordering::Relaxed), 0);
        check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        let row = edit.ok_or_else(|| IndexError::BackendViolation("rename edit missing".into()))?;
        let edit = &row.edit;
        check_eq!(edit.key.as_slice(), &owner_key(fixture.owner));
        check_eq!(edit.expected, Some(MapValue::Object(reference(10))));
        let next = edit
            .next
            .ok_or_else(|| IndexError::BackendViolation("rename contribution missing".into()))?;
        let renamed = read_contribution(next, &fixture.objects, &mut budget()?)?;
        check_eq!(renamed.record, renamed_ref);
        check_eq!(renamed.inventory, EntryMap::from(fixture.inventory_root));
        check_eq!(renamed.indexed_tokens, 2);
        Ok(())
    }

    #[test]
    fn changed_body_enters_token_build_and_bad_reuse_metadata_refuses() -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        let prior = fixture_record(&fixture)?;
        let changed_body = reference(42);
        fixture
            .objects
            .add(changed_body, b"two".to_vec(), Vec::new())?;
        fixture.objects.counted_body = Some(changed_body);
        let changed = SourceRecord {
            path: "b".into(),
            bytes: changed_body,
            byte_len: 3,
            text: prior.text,
        };
        let result = drive_change(
            &fixture,
            &SourceChange {
                owner: fixture.owner,
                prior: Some((
                    fixture.source_ref,
                    SourceRecordOwner::new_unscoped(prior.clone()),
                )),
                next: Some((reference(43), SourceRecordOwner::new_unscoped(changed))),
            },
        );
        check!(matches!(result, Err(IndexError::BackendViolation(_))));
        check_eq!(fixture.objects.body_reads.load(Ordering::Relaxed), 1);
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 1);

        fixture.objects.counted_body = Some(prior.bytes);
        let bad_length = SourceRecord {
            path: "c".into(),
            byte_len: 4,
            ..prior
        };
        let result = drive_change(
            &fixture,
            &SourceChange {
                owner: fixture.owner,
                prior: Some((fixture.source_ref, SourceRecordOwner::new_unscoped(prior))),
                next: Some((reference(44), SourceRecordOwner::new_unscoped(bad_length))),
            },
        );
        check!(matches!(result, Err(IndexError::CorruptObject(_))));
        check_eq!(fixture.objects.body_reads.load(Ordering::Relaxed), 2);

        let contribution = Contribution {
            record: fixture.source_ref,
            inventory: fixture.inventory_root.into(),
            indexed_tokens: 2,
        };
        check!(matches!(
            validate_reused_inventory(
                &contribution,
                &Contribution {
                    indexed_tokens: 3,
                    ..contribution
                }
            ),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    #[test]
    fn utf8_stamp_with_invalid_bytes_refuses_before_token_writes() -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        let prior = fixture_record(&fixture)?;
        let bad_body = reference(45);
        fixture.objects.add(bad_body, vec![0xff], Vec::new())?;
        fixture.objects.counted_body = Some(bad_body);
        let result = drive_change(
            &fixture,
            &SourceChange {
                owner: fixture.owner,
                prior: Some((
                    fixture.source_ref,
                    SourceRecordOwner::new_unscoped(prior.clone()),
                )),
                next: Some((
                    reference(46),
                    SourceRecordOwner::new_unscoped(SourceRecord {
                        path: "invalid-utf8".into(),
                        bytes: bad_body,
                        byte_len: 1,
                        text: prior.text,
                    }),
                )),
            },
        );
        check!(matches!(result, Err(IndexError::CorruptObject(message))
            if message == "text-admitted source body is not UTF-8"));
        check_eq!(fixture.objects.body_reads.load(Ordering::Relaxed), 1);
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn inventory_diff_reads_shared_occurrence_once_and_still_checks_both_totals()
    -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
        let next_inventory = MapRoot(Some(reference(47)));
        fixture
            .maps
            .add(next_inventory, fixture.term.clone(), fixture.occurrence_ref)?;
        let corrupt_inventory = MapRoot(Some(reference(48)));
        let corrupt_occurrence = reference(49);
        fixture.objects.add(
            corrupt_occurrence,
            b"{\"positions\":[]}".to_vec(),
            Vec::new(),
        )?;
        fixture
            .maps
            .add(corrupt_inventory, fixture.term.clone(), corrupt_occurrence)?;
        let prior = Contribution {
            record: fixture.source_ref,
            inventory: fixture.inventory_root.into(),
            indexed_tokens: 2,
        };
        let next = Contribution {
            inventory: next_inventory.into(),
            ..prior
        };
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut work = budget()?;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut context = AttemptContext {
            attempt: test_attempt(fixture.ticket.view.identity.workspace),
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let mut posting_edits = PostingEdits::new()?;
        let mut confirmed = ConfirmedOccurrences::new(0)?;
        diff_inventory(
            &owner_key(fixture.owner),
            Some(&prior),
            Some(&next),
            &mut context,
            &spool,
            &mut posting_edits,
            &mut confirmed,
        )?;
        check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 1);
        check!(spool.posting_term_page(None, context.budget)?.is_empty());
        for (old, new) in [
            (
                Contribution {
                    indexed_tokens: 3,
                    ..prior
                },
                next,
            ),
            (
                prior,
                Contribution {
                    indexed_tokens: 3,
                    ..next
                },
            ),
        ] {
            check!(matches!(
                diff_inventory(
                    &owner_key(fixture.owner),
                    Some(&old),
                    Some(&new),
                    &mut context,
                    &spool,
                    &mut posting_edits,
                    &mut confirmed,
                ),
                Err(IndexError::CorruptObject(_))
            ));
        }
        // Equal term names do not authorize reuse of a different immutable
        // identity: the new occurrence still needs independent validation.
        check!(matches!(
            diff_inventory(
                &owner_key(fixture.owner),
                Some(&prior),
                Some(&Contribution {
                    inventory: corrupt_inventory.into(),
                    ..next
                }),
                &mut context,
                &spool,
                &mut posting_edits,
                &mut confirmed,
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 4);
        check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn confirmed_new_count_reads_once_across_terms_and_owners_but_prior_still_reads()
    -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
        let other_term = stored_term_key("two", CaseMode::Sensitive)?;
        let other_inventory = MapRoot(Some(reference(51)));
        fixture
            .maps
            .add(other_inventory, other_term, fixture.occurrence_ref)?;
        let first = Contribution {
            record: fixture.source_ref,
            inventory: fixture.inventory_root.into(),
            indexed_tokens: 2,
        };
        let second = Contribution {
            inventory: other_inventory.into(),
            ..first
        };
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut work = budget()?;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut confirmed = ConfirmedOccurrences::new(2)?;
        confirmed.prepare(attempt, &work)?;
        // The production caller remembers only after the complete put_batch
        // ordinal and identity check; this test starts at that confirmed state.
        confirmed.remember(fixture.occurrence_ref, attempt, &work)?;
        let mut posting_edits = PostingEdits::new()?;
        {
            let mut context = AttemptContext {
                attempt,
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            diff_inventory(
                &owner_key(fixture.owner),
                None,
                Some(&first),
                &mut context,
                &spool,
                &mut posting_edits,
                &mut confirmed,
            )?;
            let other_owner = OwnerKey {
                stable_id: StableOwnerId([52; 16]),
                ..fixture.owner
            };
            diff_inventory(
                &owner_key(other_owner),
                None,
                Some(&second),
                &mut context,
                &spool,
                &mut posting_edits,
                &mut confirmed,
            )?;
            check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 1);
            check_eq!(
                confirmed
                    .references
                    .first()
                    .map(|entry| entry.validated_count),
                Some(Some(2))
            );
            let mut disabled = ConfirmedOccurrences::new(0)?;
            for _ in 0..2 {
                check_eq!(
                    disabled.validated_new_count(
                        fixture.term.as_slice(),
                        fixture.occurrence_ref,
                        &spool,
                        &mut context,
                    )?,
                    2
                );
            }
            check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 3);
        }
        fixture.objects.add(
            fixture.occurrence_ref,
            b"{\"positions\":[]}".to_vec(),
            Vec::new(),
        )?;
        let mut context = AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        check!(matches!(
            diff_inventory(
                &owner_key(fixture.owner),
                Some(&first),
                None,
                &mut context,
                &spool,
                &mut posting_edits,
                &mut confirmed,
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 4);
        Ok(())
    }

    #[test]
    fn inventory_cursor_refuses_oversized_page_before_yielding_any_entry() -> Result<(), IndexError>
    {
        let mut fixture = visitor_fixture()?;
        fixture.maps.posting_root = fixture.inventory_root;
        let entries = (0..65)
            .map(|ordinal| {
                (
                    format!("term{ordinal:03}").into_bytes(),
                    fixture.occurrence_ref.into(),
                )
            })
            .collect();
        fixture.maps.malformed_page = Some(MapPage::new(entries, false, Vec::new(), None));
        let mut cursor = MapEntries::new(&fixture.objects, &fixture.maps, fixture.inventory_root);
        check!(matches!(
            cursor.next(&mut budget()?),
            Err(IndexError::BackendViolation(_))
        ));
        check!(cursor.entries.is_empty());
        check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn inventory_cursor_accepts_full_page_and_refuses_repeated_continuation()
    -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        fixture.maps.posting_root = fixture.inventory_root;
        fixture.maps.malformed_page = Some(MapPage::new(
            (0..64)
                .map(|ordinal| {
                    (
                        format!("term{ordinal:03}").into_bytes(),
                        fixture.occurrence_ref.into(),
                    )
                })
                .collect(),
            true,
            Vec::new(),
            None,
        ));
        let mut cursor = MapEntries::new(&fixture.objects, &fixture.maps, fixture.inventory_root);
        let mut work = budget()?;
        for ordinal in 0..64 {
            check_eq!(
                cursor
                    .next(&mut work)?
                    .map(|entry| entry.get().cloned())
                    .transpose()?,
                Some((
                    format!("term{ordinal:03}").into_bytes(),
                    fixture.occurrence_ref.into()
                ))
            );
        }
        check!(matches!(
            cursor.next(&mut work),
            Err(IndexError::BackendViolation(_))
        ));
        Ok(())
    }

    #[test]
    fn packed_diff_cursor_retains_checked_data_only_through_its_last_row() -> Result<(), IndexError>
    {
        let occurrence_ref = reference(61);
        let mut payload = b"QITXOC01".to_vec();
        payload.extend_from_slice(&2_u16.to_be_bytes());
        payload.extend_from_slice(&2_u16.to_be_bytes());
        payload.extend_from_slice(&[0; 4]);
        for (key, vector) in [(b"\0a", 0_u16), (b"\0b", 1)] {
            payload.extend_from_slice(&2_u16.to_be_bytes());
            payload.extend_from_slice(key);
            payload.extend_from_slice(&vector.to_be_bytes());
        }
        payload.extend_from_slice(&1_u16.to_be_bytes());
        payload.extend_from_slice(&0_u32.to_be_bytes());
        payload.extend_from_slice(&2_u16.to_be_bytes());
        payload.extend_from_slice(&0_u32.to_be_bytes());
        payload.extend_from_slice(&2_u32.to_be_bytes());
        let mut objects = TestObjects::default();
        objects.counted_occurrence = Some(occurrence_ref);
        objects.add(occurrence_ref, payload, Vec::new())?;
        let maps = TestMaps::default();
        let mut work = budget()?;
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let attempt = test_attempt(workspace);
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut confirmed = ConfirmedOccurrences::new(1)?;
        confirmed.prepare(attempt, &work)?;
        confirmed.remember(reference(63), attempt, &work)?;
        spool.admit_confirmed_occurrences(&[occurrence_ref], attempt, &mut work)?;
        spool.record_confirmed_occurrences(&[occurrence_ref], attempt, &work)?;
        let mut context = AttemptContext {
            attempt,
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let inventory = EntryMap::OccurrenceBlock(occurrence_ref);
        let mut cursor = MapEntries::new(&objects, &maps, inventory);
        cursor.retain_packed_for_count = true;
        let first = cursor
            .next(context.budget)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(first.key()?, b"\0a");
        let packed = cursor.packed.as_mut().ok_or(IndexError::ResourceLimited)?;
        check_eq!(packed.reference(), occurrence_ref);
        check!(!packed.count_verified());
        let before = context.budget.usage().reads;
        check_eq!(
            confirmed.validated_new_count_observed(
                first.key()?,
                occurrence_ref,
                &spool,
                &mut context,
                Some(packed),
                None,
            )?,
            1
        );
        check_eq!(objects.occurrence_reads.load(Ordering::Relaxed), 2);
        // The first count crosses retained-read admission once. An extra
        // scalar memo lookup would also consume one logical read.
        check_eq!(context.budget.usage().reads.checked_sub(before), Some(1));
        let second = cursor
            .next(context.budget)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(second.key()?, b"\0b");
        check_eq!(
            confirmed.validated_new_count_observed(
                second.key()?,
                occurrence_ref,
                &spool,
                &mut context,
                cursor.packed.as_mut(),
                None,
            )?,
            2
        );
        check_eq!(objects.occurrence_reads.load(Ordering::Relaxed), 2);
        check_eq!(context.budget.usage().reads.checked_sub(before), Some(1));
        let packed = cursor.packed.as_mut().ok_or(IndexError::ResourceLimited)?;
        check_eq!(packed.validated_count(b"\0a", context.budget)?, 1);
        check!(matches!(
            packed.validated_count(b"\0missing", context.budget),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(packed.validated_count(b"\0b", context.budget)?, 2);
        check!(cursor.next(context.budget)?.is_none());
        check!(
            cursor
                .packed
                .as_ref()
                .is_some_and(|packed| packed.count_verified())
        );
        check!(
            cursor
                .packed
                .take()
                .and_then(entry_map::occurrence::PackedInventoryCursor::into_validated_block)
                .is_some()
        );

        let mut disabled = MapEntries::new(&objects, &maps, inventory);
        check!(disabled.next(context.budget)?.is_some());
        check!(disabled.packed.is_none());

        let contribution = Contribution {
            record: reference(62),
            inventory,
            indexed_tokens: 3,
        };
        for owner in [[1_u8; 32], [2_u8; 32]] {
            let prior_reads = objects.occurrence_reads.load(Ordering::Relaxed);
            let mut posting_rows = PostingEdits::new()?;
            diff_inventory(
                &owner,
                None,
                Some(&contribution),
                &mut context,
                &spool,
                &mut posting_rows,
                &mut confirmed,
            )?;
            check!(confirmed.blocks.contains(occurrence_ref));
            check_eq!(
                objects.occurrence_reads.load(Ordering::Relaxed) - prior_reads,
                if owner[0] == 1 { 2 } else { 1 }
            );
        }
        let mut refused = ConfirmedOccurrences::new(2)?;
        refused.prepare(attempt, context.budget)?;
        let invalid = Contribution {
            indexed_tokens: 1,
            ..contribution
        };
        let mut posting_rows = PostingEdits::new()?;
        check!(matches!(
            diff_inventory(
                &[3_u8; 32],
                None,
                Some(&invalid),
                &mut context,
                &spool,
                &mut posting_rows,
                &mut refused,
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check!(!refused.blocks.contains(occurrence_ref));
        Ok(())
    }

    #[test]
    fn term_visit_validates_owner_and_preserves_position_gaps() -> Result<(), IndexError> {
        let fixture = visitor_fixture()?;
        let mut work = budget()?;
        let mut hits = Vec::new();
        let summary = NativeTextIndex::new(fixture.recipe).visit_term(
            &fixture.member,
            CaseMode::Sensitive,
            "one",
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut work,
            },
            |hit| {
                hits.push((
                    hit.owner,
                    hit.source,
                    hit.tf,
                    hit.doc_indexed_tokens,
                    hit.positions.to_vec(),
                ));
                Ok(())
            },
        )?;
        check_eq!(
            summary,
            TextTermScan {
                docs: 1,
                df: 1,
                indexed_tokens: 2
            }
        );
        check_eq!(
            hits,
            vec![(fixture.owner, fixture.source_ref, 2, 2, vec![0, 2])]
        );
        Ok(())
    }

    #[test]
    fn term_visit_refuses_malformed_page_before_callback() -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        fixture.maps.malformed_page = Some(MapPage::new(
            vec![
                (
                    owner_key(fixture.owner).to_vec(),
                    fixture.occurrence_ref.into()
                );
                65
            ],
            false,
            Vec::new(),
            None,
        ));
        let mut work = budget()?;
        let mut called = false;
        let result = NativeTextIndex::new(fixture.recipe).visit_term(
            &fixture.member,
            CaseMode::Sensitive,
            "one",
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut work,
            },
            |_hit| {
                called = true;
                Ok(())
            },
        );
        check!(matches!(result, Err(IndexError::BackendViolation(_))));
        check!(!called);
        Ok(())
    }

    #[test]
    fn term_visit_refuses_stale_source_and_corrupt_inventory() -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        fixture.maps.add(
            fixture.ticket.view.source.owners,
            owner_map_key(fixture.owner).to_vec(),
            reference(31),
        )?;
        let mut work = budget()?;
        let result = NativeTextIndex::new(fixture.recipe).visit_term(
            &fixture.member,
            CaseMode::Sensitive,
            "one",
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut work,
            },
            |_hit| Ok(()),
        );
        check!(matches!(result, Err(IndexError::CorruptObject(_))));
        fixture.maps.add(
            fixture.ticket.view.source.owners,
            owner_map_key(fixture.owner).to_vec(),
            fixture.source_ref,
        )?;
        fixture
            .maps
            .add(fixture.inventory_root, fixture.term.clone(), reference(32))?;
        let mut work = budget()?;
        let result = NativeTextIndex::new(fixture.recipe).visit_term(
            &fixture.member,
            CaseMode::Sensitive,
            "one",
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut work,
            },
            |_hit| Ok(()),
        );
        check!(matches!(result, Err(IndexError::CorruptObject(_))));
        Ok(())
    }

    #[test]
    fn term_visit_never_reports_success_for_incorrect_df() -> Result<(), IndexError> {
        use quanta_index_contract::{LqCase, LqExpr, LqLeaf, LqQuery, LqSpan};
        let mut fixture = visitor_fixture()?;
        let posting = Posting {
            root: fixture.posting_root.into(),
            df: 2,
        };
        fixture.objects.add(
            fixture.posting_ref,
            encode(&posting)?,
            posting_children(&posting)?.as_slice().to_vec(),
        )?;
        let root = Root {
            owners: MapRoot(Some(reference(3))),
            terms: MapRoot(Some(reference(4))),
            docs: 2,
            indexed_tokens: 2,
        };
        let payload = encode(&root)?;
        fixture.objects.add(
            fixture.member.artifact,
            payload.clone(),
            root_children(root).as_slice().to_vec(),
        )?;
        fixture.member.logical_commitment = Digest(Sha256::digest(&payload).into());
        fixture.objects.add(
            fixture.member_ref,
            encode(&fixture.member)?,
            vec![fixture.member.artifact, reference(1)],
        )?;
        let mut work = budget()?;
        let mut provisional = 0u64;
        let result = NativeTextIndex::new(fixture.recipe).visit_term(
            &fixture.member,
            CaseMode::Sensitive,
            "one",
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut work,
            },
            |_hit| {
                provisional = provisional
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                Ok(())
            },
        );
        check!(matches!(result, Err(IndexError::CorruptObject(_))));
        check_eq!(provisional, 1);
        // A missing AND operand cannot hide corruption in another operand.
        // Ranking must also discard provisional top-k hits on late EOF failure.
        for expr in [
            LqExpr::All(vec![
                LqExpr::Leaf(LqLeaf::Keyword("one".into())),
                LqExpr::Leaf(LqLeaf::Keyword("absent".into())),
            ]),
            LqExpr::Any(vec![
                LqExpr::Leaf(LqLeaf::Keyword("absent".into())),
                LqExpr::Leaf(LqLeaf::Keyword("one".into())),
            ]),
        ] {
            let mut query = LqQuery::empty(LqSpan::synthetic(0));
            query.expr = expr;
            query.options.case = Some(LqCase::Sensitive);
            let plan = crate::planner::LexicalPlanner::plan(&query)
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
            let result = NativeTextIndex::new(fixture.recipe).search_content(
                &fixture.member,
                &plan,
                &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                1,
                super::super::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
                TextReadContext {
                    ticket: &fixture.ticket,
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    budget: &mut budget()?,
                },
            );
            check!(matches!(result, Err(IndexError::CorruptObject(_))));
        }
        Ok(())
    }

    #[test]
    fn repeated_query_term_opens_one_posting_and_keeps_rank() -> Result<(), IndexError> {
        use quanta_index_contract::{LqCase, LqExpr, LqLeaf, LqQuery, LqSpan};

        let fixture = visitor_fixture()?;
        let index = NativeTextIndex::new(fixture.recipe);
        let search = |expr| -> Result<super::super::text_query::NativeTextRanking, IndexError> {
            let mut query = LqQuery::empty(LqSpan::synthetic(0));
            query.expr = expr;
            query.options.case = Some(LqCase::Sensitive);
            let plan = crate::planner::LexicalPlanner::plan(&query)
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
            index.search_content(
                &fixture.member,
                &plan,
                &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                1,
                super::super::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
                TextReadContext {
                    ticket: &fixture.ticket,
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    budget: &mut budget()?,
                },
            )
        };
        let leaf = || LqExpr::Leaf(LqLeaf::Keyword("one".into()));
        let single = search(leaf())?;
        check_eq!(fixture.maps.term_lookups.load(Ordering::Relaxed), 1);
        check_eq!(fixture.maps.batch_lookups.load(Ordering::Relaxed), 0);
        let repeated = search(LqExpr::Any((0..64).map(|_| leaf()).collect()))?;
        check_eq!(fixture.maps.term_lookups.load(Ordering::Relaxed), 2);
        check_eq!(fixture.maps.batch_lookups.load(Ordering::Relaxed), 0);
        check_eq!(repeated, single);
        Ok(())
    }

    #[test]
    fn multi_owner_posting_page_batches_both_custody_maps() -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let (fixture, _second) = two_owner_phrase_fixture()?;
        let plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Keyword("one".into())))?;
        let ranking = search_phrase_plan(&fixture, &plan, 2)?;
        check_eq!(ranking.matched, 2);
        check_eq!(fixture.maps.batch_lookups.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn repeated_common_and_rare_term_scores_match_original_formula() -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let (fixture, second) = two_owner_phrase_fixture()?;
        let leaf = |term: &str| LqExpr::Leaf(LqLeaf::Keyword(term.into()));
        let run = |terms: [&str; 2]| {
            let plan = planned_phrase_query(LqExpr::Any(terms.into_iter().map(leaf).collect()))?;
            let mut work = budget()?;
            let ranking = NativeTextIndex::new(fixture.recipe).search_content(
                &fixture.member,
                &plan,
                &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                2,
                super::super::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
                TextReadContext {
                    ticket: &fixture.ticket,
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    budget: &mut work,
                },
            )?;
            Ok::<_, IndexError>((ranking, work.usage()))
        };
        let (ranking, usage) = run(["one", "gap"])?;
        let (permuted, permuted_usage) = run(["gap", "one"])?;
        check_eq!(ranking, permuted);
        check_eq!(usage, permuted_usage);
        check_eq!(ranking.matched, 2);
        check_eq!(ranking.hits.len(), 2);

        // Fixture facts: two documents contain `one`; only the six-token
        // first document contains `gap`. The corpus has eight tokens total.
        let original_formula = |df: f64, tf: f64, dl: f64| {
            let idf = ((2.0 - df + 0.5) / (df + 0.5)).ln_1p();
            let length = 1.0 - 0.75 + 0.75 * dl / 4.0;
            idf * ((1.0 + 1.2_f64.recip()) / (1.2_f64.recip() + length / tf))
        };
        let first_expected = original_formula(1.0, 1.0, 6.0) + original_formula(2.0, 3.0, 6.0);
        let second_expected = original_formula(2.0, 1.0, 2.0);
        let first = ranking
            .hits
            .iter()
            .find(|hit| hit.owner == fixture.owner)
            .ok_or_else(|| IndexError::BackendViolation("first BM25 owner missing".into()))?;
        let second_hit = ranking
            .hits
            .iter()
            .find(|hit| hit.owner == second)
            .ok_or_else(|| IndexError::BackendViolation("second BM25 owner missing".into()))?;
        check_eq!(first.score.to_bits(), first_expected.to_bits());
        check_eq!(second_hit.score.to_bits(), second_expected.to_bits());
        Ok(())
    }

    #[test]
    fn invalid_common_posting_is_rejected_before_boolean_miss() -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let (mut fixture, _second) = two_owner_phrase_fixture()?;
        // Its first owner has six indexed tokens. Seven occurrences must be
        // refused even when the other AND term has no matching posting.
        fixture.objects.add(
            fixture.occurrence_ref,
            encode(&Occurrence {
                positions: Positions((0..7).collect()),
            })?,
            Vec::new(),
        )?;
        let plan = planned_phrase_query(LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::Keyword("one".into())),
            LqExpr::Leaf(LqLeaf::Keyword("absent".into())),
        ]))?;
        let outcome = search_phrase_plan(&fixture, &plan, 2);
        check!(matches!(outcome, Err(IndexError::CorruptObject(_))));
        Ok(())
    }

    fn planned_phrase_query(
        expr: quanta_index_contract::LqExpr,
    ) -> Result<crate::plan::LexicalPlan, IndexError> {
        use quanta_index_contract::{LqCase, LqQuery, LqSpan};

        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = expr;
        query.options.case = Some(LqCase::Sensitive);
        crate::planner::LexicalPlanner::plan(&query)
            .map_err(|error| IndexError::InvalidInput(error.to_string()))
    }

    fn search_phrase_plan(
        fixture: &VisitorFixture,
        plan: &crate::plan::LexicalPlan,
        top_k: u32,
    ) -> Result<super::super::text_query::NativeTextRanking, IndexError> {
        NativeTextIndex::new(fixture.recipe).search_content(
            &fixture.member,
            plan,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            top_k,
            super::super::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut budget()?,
            },
        )
    }

    #[test]
    fn native_phrase_query_trace_preserves_result_usage_and_error_counters()
    -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf};
        use quanta_index_core::DiagnosticCounterV1;

        #[derive(Debug, Default)]
        struct CountingClock(AtomicU64);
        impl DiagnosticClockV1 for CountingClock {
            fn now_ns(&self) -> u64 {
                self.0.fetch_add(1, Ordering::SeqCst)
            }
        }
        #[derive(Debug, Default)]
        struct RecordingSink(Mutex<Vec<DiagnosticEmitV1>>);
        impl DiagnosticEventSinkV1 for RecordingSink {
            fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
                let Ok(mut events) = self.0.try_lock() else {
                    return false;
                };
                if events.len() >= 64 {
                    return false;
                }
                events.push(event);
                true
            }
        }

        let (fixture, _second) = two_owner_phrase_fixture()?;
        let index = NativeTextIndex::new(fixture.recipe);
        let mut plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Phrase("one two".into())))?;
        let limits = WorkLimits {
            reads: 1_000,
            writes: 100,
            bytes: 1_000_000,
            mutations: 100,
        };
        let clock = Arc::new(CountingClock::default());
        let sink = Arc::new(RecordingSink::default());
        let trace =
            DiagnosticContextV1::new(DiagnosticCaptureIdV1([32; 16]), clock.clone(), sink.clone());
        let root = trace
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let child = trace.child_context(&root);
        let search = |plan: &crate::plan::LexicalPlan, budget: &mut WorkBudget| {
            index.search_content(
                &fixture.member,
                plan,
                &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                2,
                super::super::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
                TextReadContext {
                    ticket: &fixture.ticket,
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    budget,
                },
            )
        };
        let mut off = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        let before_off_clock = clock.0.load(Ordering::SeqCst);
        let expected = search(&plan, &mut off)?;
        check_eq!(clock.0.load(Ordering::SeqCst), before_off_clock);
        let mut recorded = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30))
                .with_diagnostic_context(child.clone()),
        );
        check_eq!(search(&plan, &mut recorded)?, expected);
        check_eq!(recorded.usage(), off.usage());

        let crate::plan::PlanNode::Leaf { cap, .. } = &mut plan.root else {
            return Err(IndexError::BackendViolation(
                "phrase planner lost its leaf".into(),
            ));
        };
        *cap = Some(crate::plan::CandidateCap(1));
        let mut off_error = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        check!(matches!(
            search(&plan, &mut off_error),
            Err(IndexError::ResourceLimited)
        ));
        let mut recorded_error = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30)).with_diagnostic_context(child),
        );
        check!(matches!(
            search(&plan, &mut recorded_error),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(recorded_error.usage(), off_error.usage());
        root.finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let events = sink
            .0
            .lock()
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let query_spans = events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    stage: DiagnosticStageV1::QueryLexical,
                    ..
                } = event.kind
                {
                    Some(span_id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        check_eq!(query_spans.len(), 2);
        let outcomes = query_spans
            .iter()
            .map(|query| {
                events.iter().find_map(|event| {
                    if let DiagnosticEventKindV1::SpanEnd { span_id, outcome } = event.kind {
                        (span_id == *query).then_some(outcome)
                    } else {
                        None
                    }
                })
            })
            .collect::<Vec<_>>();
        check_eq!(
            outcomes,
            vec![
                Some(DiagnosticTerminalV1::Ok),
                Some(DiagnosticTerminalV1::Error { code: 1 })
            ]
        );
        for (ordinal, query) in query_spans.into_iter().enumerate() {
            let count = |key| {
                events.iter().find_map(|event| {
                    if let DiagnosticEventKindV1::Counter {
                        span_id,
                        key: found,
                        value,
                    } = event.kind
                    {
                        (span_id == query && found == key).then_some(value)
                    } else {
                        None
                    }
                })
            };
            let calls = count(DiagnosticCounterV1::QueryPostingCalls)
                .ok_or_else(|| IndexError::BackendViolation("missing posting calls".into()))?;
            let hits = count(DiagnosticCounterV1::QueryPostingHits)
                .ok_or_else(|| IndexError::BackendViolation("missing posting hits".into()))?;
            let checks = count(DiagnosticCounterV1::QueryPhraseChecks)
                .ok_or_else(|| IndexError::BackendViolation("missing phrase checks".into()))?;
            let expected = if ordinal == 0 { (6, 4, 2) } else { (4, 4, 1) };
            check_eq!((calls, hits, checks), expected);
            check!(count(DiagnosticCounterV1::QueryPostingElapsedNs).is_some());
            check!(count(DiagnosticCounterV1::QueryPhraseElapsedNs).is_some());
        }
        drop(events);
        Ok(())
    }

    fn search_phrase_with_constraints(
        fixture: &VisitorFixture,
        plan: &crate::plan::LexicalPlan,
        constraints: &quanta_index_contract::QueryConstraintSetV1,
        top_k: u32,
    ) -> Result<super::super::text_query::NativeTextRanking, IndexError> {
        NativeTextIndex::new(fixture.recipe).search_content(
            &fixture.member,
            plan,
            constraints,
            top_k,
            super::super::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut budget()?,
            },
        )
    }

    fn exact_path_constraint(
        path: &str,
    ) -> Result<quanta_index_contract::QueryConstraintSetV1, IndexError> {
        let path = quanta_index_contract::ExactRepoRelativePathV1::new(path)
            .map_err(|message| IndexError::InvalidInput(message.into()))?;
        Ok(quanta_index_contract::QueryConstraintSetV1::from_exact_repo_relative_path(path))
    }

    #[test]
    fn native_phrase_uses_actual_posting_positions_for_order_gap_and_repetition()
    -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let fixture = phrase_fixture()?;
        // Independent body gold: one two gap one one three.
        for (phrase, matches) in [
            ("one two", true),
            ("two gap", true),
            ("one one", true),
            ("two one", false),
            ("two three", false),
            ("one one one", false),
        ] {
            let plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Phrase(phrase.into())))?;
            let ranking = search_phrase_plan(&fixture, &plan, 1)?;
            check_eq!(ranking.matched, u64::from(matches));
            check_eq!(ranking.hits.len(), usize::from(matches));
            if matches {
                let hit = ranking.hits.first().ok_or_else(|| {
                    IndexError::BackendViolation("matching phrase lost its first hit".into())
                })?;
                check_eq!(hit.owner, fixture.owner);
                check_eq!(
                    hit.source,
                    super::super::text_query::NativeTextHitSource::Durable(fixture.source_ref)
                );
            }
        }
        Ok(())
    }

    #[test]
    fn native_phrase_boolean_not_filters_without_duplicating_positive_term_score()
    -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let fixture = phrase_fixture()?;
        let phrase = |text: &str| LqExpr::Leaf(LqLeaf::Phrase(text.into()));
        let keyword = |text: &str| LqExpr::Leaf(LqLeaf::Keyword(text.into()));
        let baseline = search_phrase_plan(&fixture, &planned_phrase_query(phrase("one two"))?, 1)?;
        check_eq!(baseline.hits.len(), 1);
        let mixed = planned_phrase_query(LqExpr::All(vec![
            phrase("one two"),
            keyword("one"),
            LqExpr::Not(Box::new(phrase("two one"))),
        ]))?;
        check_eq!(search_phrase_plan(&fixture, &mixed, 1)?, baseline);
        let denied = planned_phrase_query(LqExpr::All(vec![
            phrase("one two"),
            LqExpr::Not(Box::new(phrase("one one"))),
        ]))?;
        check!(search_phrase_plan(&fixture, &denied, 1)?.hits.is_empty());
        let unanchored = planned_phrase_query(LqExpr::Not(Box::new(phrase("one two"))))?;
        let empty = search_phrase_plan(&fixture, &unanchored, 1)?;
        check_eq!(empty.matched, 0);
        check!(empty.hits.is_empty());
        Ok(())
    }

    #[test]
    fn native_phrase_candidate_cap_refuses_complete_result_after_second_owner()
    -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let (fixture, second) = two_owner_phrase_fixture()?;
        let mut plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Phrase("one two".into())))?;
        let complete = search_phrase_plan(&fixture, &plan, 2)?;
        check_eq!(complete.matched, 2);
        check_eq!(complete.hits.len(), 2);
        check!(complete.hits.iter().any(|hit| hit.owner == fixture.owner));
        check!(complete.hits.iter().any(|hit| hit.owner == second));
        let crate::plan::PlanNode::Leaf { cap, .. } = &mut plan.root else {
            return Err(IndexError::BackendViolation(
                "phrase planner lost its leaf".into(),
            ));
        };
        *cap = Some(crate::plan::CandidateCap(2));
        check_eq!(search_phrase_plan(&fixture, &plan, 2)?, complete);
        let crate::plan::PlanNode::Leaf { cap, .. } = &mut plan.root else {
            return Err(IndexError::BackendViolation(
                "phrase planner lost its leaf".into(),
            ));
        };
        *cap = Some(crate::plan::CandidateCap(1));
        // A successful top-1 prefix would hide the second owner and violate
        // the cap's complete-candidate condition.
        check!(matches!(
            search_phrase_plan(&fixture, &plan, 1),
            Err(IndexError::ResourceLimited)
        ));
        Ok(())
    }

    #[test]
    fn native_phrase_discards_ranked_prefix_when_later_posting_is_corrupt() -> Result<(), IndexError>
    {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let (mut fixture, second) = two_owner_phrase_fixture()?;
        seed_phrase_source_paths(&mut fixture, second)?;
        let first_occurrence = fixture.occurrence_ref;
        fixture.objects.counted_occurrence = Some(first_occurrence);
        let second_occurrence = fixture
            .maps
            .lookup_object(
                MapRoot(Some(reference(72))),
                &stored_term_key("two", CaseMode::Sensitive)?,
                &mut budget()?,
            )?
            .ok_or_else(|| IndexError::BackendViolation("second phrase posting missing".into()))?;
        fixture
            .objects
            .add(second_occurrence, b"{".to_vec(), Vec::new())?;
        let plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Phrase("one two".into())))?;
        check!(matches!(
            search_phrase_plan(&fixture, &plan, 2),
            Err(IndexError::CorruptObject(_))
        ));
        // Exact selection and legitimate absence cannot hide damage in a
        // later nonselected owner's posting. Both queries must drain to EOF.
        for path in ["a", "missing"] {
            check!(matches!(
                search_phrase_with_constraints(&fixture, &plan, &exact_path_constraint(path)?, 1),
                Err(IndexError::CorruptObject(_))
            ));
        }
        check!(fixture.objects.occurrence_reads.load(Ordering::Relaxed) > 0);
        Ok(())
    }

    #[test]
    fn native_phrase_rejects_forged_plan_before_posting_lookup() -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let fixture = phrase_fixture()?;
        let plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Phrase("one two".into())))?;
        let before = fixture.maps.term_lookups.load(Ordering::Relaxed);
        for variant in 0..6 {
            let mut forged = plan.clone();
            let crate::plan::PlanNode::Leaf { leaf, cap } = &mut forged.root else {
                return Err(IndexError::BackendViolation(
                    "phrase planner lost its leaf".into(),
                ));
            };
            let crate::plan::PlanLeaf::Phrase { plan, .. } = leaf else {
                return Err(IndexError::BackendViolation(
                    "phrase planner lost its plan".into(),
                ));
            };
            match variant {
                0 => *cap = Some(crate::plan::CandidateCap(0)),
                1 => plan.slop = 1,
                2 => plan.case_sensitive = false,
                3 => plan.field = crate::phrase::PhraseField::Path,
                4 => plan.tokens.swap(0, 1),
                _ => plan.tokens.push("invented".into()),
            }
            check!(matches!(
                search_phrase_plan(&fixture, &forged, 1),
                Err(IndexError::InvalidInput(_))
            ));
            check_eq!(fixture.maps.term_lookups.load(Ordering::Relaxed), before);
        }
        Ok(())
    }

    #[test]
    fn native_exact_path_selects_lower_ranked_owner_before_top_k_and_matched_count()
    -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf, QueryConstraintSetV1};

        let (mut fixture, second) = two_owner_phrase_fixture()?;
        seed_phrase_source_paths(&mut fixture, second)?;
        let plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Phrase("one two".into())))?;
        // Both owners contain the phrase. With these fixed corpus statistics,
        // the second document's shorter length outweighs the first's extra
        // occurrences of `one`.
        let unfiltered = search_phrase_with_constraints(
            &fixture,
            &plan,
            &QueryConstraintSetV1::unconstrained(),
            1,
        )?;
        check_eq!(unfiltered.matched, 2);
        check_eq!(unfiltered.hits.first().map(|hit| hit.owner), Some(second));

        let selected =
            search_phrase_with_constraints(&fixture, &plan, &exact_path_constraint("a")?, 1)?;
        check_eq!(selected.matched, 1);
        check_eq!(selected.hits.len(), 1);
        check_eq!(
            selected.hits.first().map(|hit| hit.owner),
            Some(fixture.owner)
        );
        Ok(())
    }

    #[test]
    fn native_exact_path_missing_returns_zero_without_ranked_prefix() -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let (mut fixture, second) = two_owner_phrase_fixture()?;
        seed_phrase_source_paths(&mut fixture, second)?;
        let plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Phrase("one two".into())))?;
        let ranking =
            search_phrase_with_constraints(&fixture, &plan, &exact_path_constraint("missing")?, 1)?;
        check_eq!(ranking.matched, 0);
        check!(ranking.hits.is_empty());
        Ok(())
    }

    #[test]
    fn native_exact_path_rejects_inline_owner_or_source_path_corruption() -> Result<(), IndexError>
    {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Phrase("one two".into())))?;
        for corrupt_owner in [true, false] {
            let (mut fixture, second) = two_owner_phrase_fixture()?;
            seed_phrase_source_paths(&mut fixture, second)?;
            if corrupt_owner {
                fixture.maps.add(
                    fixture.ticket.view.source.paths,
                    b"path\0a".to_vec(),
                    quanta_index_core::index::path_owner_map_value(second)?,
                )?;
            } else {
                let mut record = fixture_record(&fixture)?;
                record.path = "other".into();
                fixture
                    .objects
                    .add(fixture.source_ref, encode(&record)?, vec![record.bytes])?;
            }
            check!(matches!(
                search_phrase_with_constraints(&fixture, &plan, &exact_path_constraint("a")?, 1,),
                Err(IndexError::CorruptObject(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn native_language_constraint_refuses_before_any_term_lookup() -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf, QueryConstraintSetV1};

        let (mut fixture, second) = two_owner_phrase_fixture()?;
        seed_phrase_source_paths(&mut fixture, second)?;
        let plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Phrase("one two".into())))?;
        let constraints: QueryConstraintSetV1 =
            serde_json::from_str(r#"{"language_any_of":["rust"]}"#)
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let before = fixture.maps.term_lookups.load(Ordering::Relaxed);
        check!(matches!(
            search_phrase_with_constraints(&fixture, &plan, &constraints, 1),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(fixture.maps.term_lookups.load(Ordering::Relaxed), before);
        Ok(())
    }

    #[test]
    fn native_exact_path_authenticates_admission_even_without_a_matching_posting()
    -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Keyword("absent".into())))?;
        for wrong_policy in [false, true] {
            let (mut fixture, second) = two_owner_phrase_fixture()?;
            seed_phrase_source_paths(&mut fixture, second)?;
            let mut record = fixture_record(&fixture)?;
            if wrong_policy {
                record.text = SourceTextAdmission::Utf8 {
                    policy: Digest([99; 32]),
                };
            } else {
                record.byte_len = MAX_SOURCE_BYTES
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            fixture
                .objects
                .add(fixture.source_ref, encode(&record)?, vec![record.bytes])?;
            check!(matches!(
                search_phrase_with_constraints(&fixture, &plan, &exact_path_constraint("a")?, 1),
                Err(IndexError::CorruptObject(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn native_exact_path_excluded_source_is_a_legitimate_absence() -> Result<(), IndexError> {
        let (mut fixture, second) = two_owner_phrase_fixture()?;
        seed_phrase_source_paths(&mut fixture, second)?;
        let mut record = fixture_record(&fixture)?;
        record.text = SourceTextAdmission::Excluded;
        fixture
            .objects
            .add(fixture.source_ref, encode(&record)?, vec![record.bytes])?;
        let selected = super::super::text_filter::resolve(
            &NativeTextIndex::new(fixture.recipe),
            &exact_path_constraint("a")?,
            &mut TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut budget()?,
            },
        )?;
        check_eq!(selected, super::super::text_filter::OwnerSelection::Absent);
        Ok(())
    }

    #[test]
    fn planned_text_read_funds_retained_exact_path_before_copy_and_refunds_on_drop()
    -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf, QueryConstraintSetV1};
        use quanta_index_core::index::MemoryLifetime;

        let path = "src/검색.rs";
        let constraints = exact_path_constraint(path)?;
        let plan = || planned_phrase_query(LqExpr::Leaf(LqLeaf::Phrase("one two".into())));
        let bytes = u64::try_from(path.len()).map_err(|_error| IndexError::ResourceLimited)?;

        let denied_ceiling = bytes.checked_sub(1).ok_or(IndexError::ResourceLimited)?;
        let (denied_budget, denied_memory) = funded_batch_budget(denied_ceiling)?;
        check!(matches!(
            super::super::PlannedTextRead::new(plan()?, 1, &constraints, |amount, lifetime| {
                check_eq!(amount, bytes);
                check_eq!(lifetime, MemoryLifetime::Retained);
                denied_budget.reserve_memory(amount, lifetime)
            }),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(denied_memory.calls.load(Ordering::Relaxed), 1);
        check_eq!(denied_memory.live.load(Ordering::Relaxed), 0);
        check_eq!(
            constraints
                .repo_relative_path_exact
                .as_ref()
                .map(quanta_index_contract::ExactRepoRelativePathV1::as_str),
            Some(path)
        );

        let (funded_budget, funded_memory) = funded_batch_budget(bytes)?;
        let request =
            super::super::PlannedTextRead::new(plan()?, 1, &constraints, |amount, lifetime| {
                check_eq!(amount, bytes);
                check_eq!(lifetime, MemoryLifetime::Retained);
                funded_budget.reserve_memory(amount, lifetime)
            })?;
        check_eq!(funded_memory.calls.load(Ordering::Relaxed), 1);
        check_eq!(funded_memory.retained_calls.load(Ordering::Relaxed), 1);
        check_eq!(funded_memory.live.load(Ordering::Relaxed), bytes);
        check_eq!(
            constraints
                .repo_relative_path_exact
                .as_ref()
                .map(quanta_index_contract::ExactRepoRelativePathV1::as_str),
            Some(path)
        );
        drop(request);
        check_eq!(funded_memory.live.load(Ordering::Relaxed), 0);
        check!(!funded_memory.invalid_release.load(Ordering::Relaxed));

        let language: QueryConstraintSetV1 =
            serde_json::from_str(r#"{"language_any_of":["rust"]}"#)
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let (language_budget, language_memory) = funded_batch_budget(bytes)?;
        check!(matches!(
            super::super::PlannedTextRead::new(plan()?, 1, &language, |amount, lifetime| {
                language_budget.reserve_memory(amount, lifetime)
            }),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(language_memory.calls.load(Ordering::Relaxed), 0);
        check_eq!(language_memory.live.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn native_exact_path_above_source_limit_is_absent_before_storage() -> Result<(), IndexError> {
        let fixture = visitor_fixture()?;
        for length in 4092..=4096 {
            let path = "x".repeat(length);
            let constraints = exact_path_constraint(&path)?;
            let mut work = budget()?;
            let selected = super::super::text_filter::resolve(
                &NativeTextIndex::new(fixture.recipe),
                &constraints,
                &mut TextReadContext {
                    ticket: &fixture.ticket,
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    budget: &mut work,
                },
            )?;
            check_eq!(selected, super::super::text_filter::OwnerSelection::Absent);
            check_eq!(work.usage().reads, 0);
        }
        Ok(())
    }

    #[test]
    fn native_exact_path_preserves_unicode_bytes_and_ignores_text_case_mode()
    -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let (mut fixture, second) = two_owner_phrase_fixture()?;
        let exact = "src/검색.rs";
        let mut record = fixture_record(&fixture)?;
        record.path = exact.into();
        fixture
            .objects
            .add(fixture.source_ref, encode(&record)?, vec![record.bytes])?;
        seed_phrase_source_paths(&mut fixture, second)?;
        let plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Phrase("one two".into())))?;
        let matched =
            search_phrase_with_constraints(&fixture, &plan, &exact_path_constraint(exact)?, 1)?;
        check_eq!(matched.matched, 1);
        check_eq!(
            matched.hits.first().map(|hit| hit.owner),
            Some(fixture.owner)
        );
        let case_changed = search_phrase_with_constraints(
            &fixture,
            &plan,
            &exact_path_constraint("SRC/검색.rs")?,
            1,
        )?;
        check_eq!(case_changed.matched, 0);
        check!(case_changed.hits.is_empty());
        Ok(())
    }

    #[test]
    fn native_phrase_late_cancellation_discards_the_provisional_ranked_hit()
    -> Result<(), IndexError> {
        use quanta_index_contract::{LqExpr, LqLeaf};

        let (mut fixture, _second) = two_owner_phrase_fixture()?;
        fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
        let second_occurrence = fixture
            .maps
            .lookup_object(
                MapRoot(Some(reference(72))),
                &stored_term_key("two", CaseMode::Sensitive)?,
                &mut budget()?,
            )?
            .ok_or_else(|| IndexError::BackendViolation("second phrase posting missing".into()))?;
        fixture.objects.cancel_after_get = Some(second_occurrence);
        let plan = planned_phrase_query(LqExpr::Leaf(LqLeaf::Phrase("one two".into())))?;
        check!(matches!(
            search_phrase_plan(&fixture, &plan, 1),
            Err(IndexError::Cancelled)
        ));
        check!(fixture.objects.occurrence_reads.load(Ordering::Relaxed) > 0);
        Ok(())
    }

    #[test]
    fn anchored_not_uses_real_postings_and_refuses_unbounded_universe() -> Result<(), IndexError> {
        use quanta_index_contract::{LqCase, LqExpr, LqLeaf, LqQuery, LqSpan};

        let fixture = visitor_fixture()?;
        let index = NativeTextIndex::new(fixture.recipe);
        let leaf = |text: &str| LqExpr::Leaf(LqLeaf::Keyword(text.to_owned()));
        let search = |expr| -> Result<super::super::text_query::NativeTextRanking, IndexError> {
            let mut query = LqQuery::empty(LqSpan::synthetic(0));
            query.expr = expr;
            query.options.case = Some(LqCase::Sensitive);
            let plan = crate::planner::LexicalPlanner::plan(&query)
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
            index.search_content(
                &fixture.member,
                &plan,
                &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                1,
                super::super::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
                TextReadContext {
                    ticket: &fixture.ticket,
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    budget: &mut budget()?,
                },
            )
        };
        // Independent fixture fact: its only owner contains `one`, never `absent`.
        // Adding a negative filter preserves the positive score exactly.
        let baseline = search(leaf("one"))?;
        check_eq!(
            search(LqExpr::All(vec![
                leaf("one"),
                LqExpr::Not(Box::new(leaf("absent"))),
            ]))?,
            baseline
        );
        check!(
            search(LqExpr::All(vec![
                leaf("one"),
                LqExpr::Not(Box::new(leaf("one"))),
            ]))?
            .hits
            .is_empty()
        );
        check_eq!(
            search(LqExpr::Any(vec![
                LqExpr::All(vec![leaf("one"), LqExpr::Not(Box::new(leaf("absent")))]),
                leaf("absent"),
            ]))?,
            baseline
        );
        let negative = search(LqExpr::Not(Box::new(leaf("one"))))?;
        check_eq!(negative.matched, 0);
        check!(negative.hits.is_empty());
        let disjunction = search(LqExpr::Any(vec![
            leaf("one"),
            LqExpr::Not(Box::new(leaf("absent"))),
        ]))?;
        check_eq!(disjunction, baseline);
        Ok(())
    }

    #[test]
    fn changed_body_scratch_reservation_work_scales_with_token_batches() -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        let mut observations = Vec::new();
        for runs in [1usize, 32, 33] {
            let scratch = TestScratch::default();
            let custody = TestCustody;
            let (mut work, memory) = funded_batch_budget(65_536)?;
            let csv = (0..runs)
                .map(|position| position.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let payload_bytes = u64::try_from(format!("{{\"positions\":[{csv}]}}").len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            let expected_arrays = std::mem::size_of::<(InventoryPartition, MapEdit)>()
                .checked_mul(8)
                // Sensitive/folded keys share one fixed position leaf.
                .and_then(|rows| rows.checked_add(std::mem::size_of::<ObjectRef>()))
                .ok_or(IndexError::ResourceLimited)?;
            let expected_arrays =
                u64::try_from(expected_arrays).map_err(|_error| IndexError::ResourceLimited)?;
            let expected_live = OccurrenceWrites::metadata_bytes(8)?
                // One unique payload remains. Staged key grants have dropped
                // after SQL insertion, before the residual physical install.
                .checked_add(payload_bytes)
                .and_then(|sum| sum.checked_add(expected_arrays))
                .and_then(|sum| sum.checked_add(inventory_packet_grant_bytes()))
                .ok_or(IndexError::ResourceLimited)?;
            fixture.objects.token_memory_at_install = Some((Arc::clone(&memory), expected_live));
            let spool = TextSpool::new(&scratch, &mut work)?;
            let baseline = scratch.reserve_observations.load(Ordering::Relaxed);
            let batches_before = fixture.objects.install_batches.load(Ordering::Relaxed);
            let mut inventory_edits = InventoryEdits::new(0)?;
            let mut context = AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            // The fixture's object writer refuses the first occurrence batch.
            // Reaching it proves every token row was admitted and inserted;
            // no map root is published from this disposable attempt.
            let result = {
                let mut writes = OccurrenceWrites::new();
                let mut confirmed = ConfirmedOccurrences::new(0)?;
                prepare_inventory(
                    &"a ".repeat(runs),
                    &MapRoot(None).into(),
                    0,
                    &mut context,
                    &spool,
                    &mut confirmed,
                    &mut inventory_edits,
                    &mut writes,
                )
                .and_then(|_| {
                    writes.flush(&mut context, &spool, &mut confirmed, &mut inventory_edits)
                })
            };
            check!(
                matches!(&result,
                    Err(IndexError::BackendViolation(message))
                        if message == "changed-body occurrence build reached object batch"
                ),
                "unexpected occurrence install result: {result:?}"
            );
            check_eq!(
                fixture.objects.install_batches.load(Ordering::Relaxed),
                batches_before
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            // The payload output and the reusable packet witness are both
            // retained for this disposable spool's lifetime.
            check_eq!(memory.retained_calls.load(Ordering::Relaxed), 2);
            drop(inventory_edits);
            check_eq!(
                memory.live.load(Ordering::Relaxed),
                inventory_packet_grant_bytes()
            );
            check!(!memory.invalid_release.load(Ordering::Relaxed));
            // The install observer excludes token/position grants from the
            // independently expected live payload and metadata allocation.
            // Inspect stored rows after the failed occurrence write with a
            // separate reader that does not renew the original attempt budget.
            let mut inspect = budget()?;
            check_eq!(
                spool
                    .token_term_page(None, &mut inspect)?
                    .iter()
                    .map(|key| key.as_slice().to_vec())
                    .collect::<Vec<_>>(),
                vec![b"\0a".to_vec(), b"\x01a".to_vec()]
            );
            let positions: Vec<_> =
                (0..u32::try_from(runs).map_err(|_error| IndexError::ResourceLimited)?).collect();
            for key in [b"\0a", b"\x01a"] {
                let observed = spool.positions(key, 64, &mut inspect)?;
                check_eq!(observed.as_slice(), positions.as_slice());
            }
            observations.push(
                scratch
                    .reserve_observations
                    .load(Ordering::Relaxed)
                    .checked_sub(baseline)
                    .ok_or_else(|| {
                        IndexError::BackendViolation(
                            "scratch reservation observation counter regressed".into(),
                        )
                    })?,
            );
            drop(spool);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
        }
        // 32 runs create 64 sensitive/folded rows, one token batch. The
        // 33rd run crosses the batch boundary. This counts the injected
        // scratch allocation's actual reservation observations, not time.
        let [first, second, third] = observations.as_slice() else {
            return Err(IndexError::BackendViolation(
                "scratch reservation fixture did not emit three observations".into(),
            ));
        };
        check_eq!(first, second);
        check!(*second <= 8, "{observations:?}");
        check!(*third > *second, "{observations:?}");
        check!(
            *third <= second.checked_add(2).ok_or(IndexError::ResourceLimited)?,
            "{observations:?}"
        );
        Ok(())
    }

    struct RefuseTokenMemory {
        calls: Arc<AtomicUsize>,
        requested: Arc<AtomicU64>,
    }

    impl quanta_index_core::index::MemoryScopePort for RefuseTokenMemory {
        fn reserve(
            &self,
            bytes: u64,
            _: quanta_index_core::index::MemoryLifetime,
        ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
            let _prior = self.calls.fetch_add(1, Ordering::Relaxed);
            self.requested.store(bytes, Ordering::Relaxed);
            Err(IndexError::ResourceLimited)
        }
    }

    #[test]
    fn cold_inventory_window_admits_256_owner_slots_without_widening_warm_or_override()
    -> Result<(), IndexError> {
        let recipe = TextRecipe::new(Digest([0; 32]));
        let default = NativeTextIndex::new(recipe);
        check_eq!(default.inventory_batch_roots(false), 64);
        check_eq!(default.inventory_batch_roots(true), 256);
        let explicit = default.with_inventory_batch_limit(32)?;
        check_eq!(explicit.inventory_batch_roots(false), 32);
        check_eq!(explicit.inventory_batch_roots(true), 32);
        check!(matches!(
            default.with_inventory_batch_limit(256),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            InventoryBatch::new(65),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            InventoryBatch::cold(257),
            Err(IndexError::InvalidInput(_))
        ));

        let (work, memory) = funded_batch_budget(1_000_000)?;
        let mut batch = InventoryBatch::cold(default.inventory_batch_roots(true))?;
        for slot in 0..256u16 {
            let owner = [u8::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?; 32];
            batch.rows.push(
                PendingTextChange {
                    owner,
                    prior_ref: None,
                    prior: None,
                    next: PendingTextNext::Ready,
                },
                &work,
            )?;
            batch.ready.push(ReadyTextNext::default(), &work)?;
            batch.bases.push(EntryMap::default(), &work)?;
            batch.counts.push(0, &work)?;
        }
        check_eq!(batch.rows.len(), 256);
        check_eq!(batch.ready.len(), 256);
        check_eq!(batch.bases.len(), 256);
        check_eq!(batch.counts.len(), 256);
        check!(matches!(
            batch.rows.push(
                PendingTextChange {
                    owner: [0; 32],
                    prior_ref: None,
                    prior: None,
                    next: PendingTextNext::Ready,
                },
                &work,
            ),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(batch.rows.len(), 256);
        drop(batch);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        use quanta_index_core::index::StagedObjectBatch;
        check!(matches!(
            StagedObjectBatch::new(65, &work),
            Err(IndexError::ResourceLimited)
        ));
        check!(matches!(
            StagedObjectBatch::for_install_packet(257, &work),
            Err(IndexError::ResourceLimited)
        ));
        let packet = StagedObjectBatch::for_install_packet(256, &work)?;
        check!(memory.live.load(Ordering::Relaxed) > 0);
        drop(packet);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn inventory_batch_admits_metadata_before_leaf_work_and_keeps_attempt_control()
    -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        let record = fixture_record(&fixture)?;
        fixture.objects.counted_body = Some(record.bytes);
        let setup = budget()?;
        let change = SourceChange {
            owner: fixture.owner,
            prior: None,
            next: Some((
                fixture.source_ref,
                quanta_index_core::index::SourceRecordOwner::decode_json(
                    &encode(&record)?,
                    &setup,
                    quanta_index_core::index::MemoryLifetime::Temporary,
                )?,
            )),
        };
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let spool = TextSpool::new(&scratch, &mut budget()?)?;
        let mut root = Root::default();
        let mut contributions = ContributionWrites::new()?;
        let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
        for limit in [0, 65] {
            check!(matches!(
                InventoryBatch::new(limit),
                Err(IndexError::InvalidInput(_))
            ));
            check!(matches!(
                NativeTextIndex::new(fixture.recipe).with_inventory_batch_limit(limit),
                Err(IndexError::InvalidInput(_))
            ));
        }
        let (mut work, memory) = funded_token_budget(0)?;
        let mut batch = InventoryBatch::new(64)?;
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let mut context = AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        check!(matches!(
            batch.push_change(
                &change,
                &mut root,
                &mut context,
                &spool,
                &mut contributions,
                &mut inventories
            ),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(batch.rows.len(), 0);
        check_eq!(batch.bases.len(), 0);
        check!(batch.attempt.is_none());
        check_eq!(fixture.objects.body_reads.load(Ordering::Relaxed), 0);
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        drop(batch);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);

        let (mut work, memory) = funded_token_budget(1_000_000)?;
        let mut batch = InventoryBatch::new(64)?;
        batch.rows.push(
            PendingTextChange {
                owner: owner_key(fixture.owner),
                prior_ref: None,
                prior: None,
                next: PendingTextNext::Preparing,
            },
            &work,
        )?;
        batch.attempt = Some(attempt);
        let mut context = AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        check!(matches!(
            batch.flush(
                &mut root,
                &mut context,
                &spool,
                &mut contributions,
                &mut inventories
            ),
            Err(IndexError::BackendViolation(_))
        ));
        context.attempt.fence = attempt
            .fence
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        check!(matches!(
            batch.flush(
                &mut root,
                &mut context,
                &spool,
                &mut contributions,
                &mut inventories
            ),
            Err(IndexError::FenceLost)
        ));
        context.attempt = attempt;
        context.budget.request().cancel_handle().cancel();
        check!(matches!(
            batch.flush(
                &mut root,
                &mut context,
                &spool,
                &mut contributions,
                &mut inventories
            ),
            Err(IndexError::Cancelled)
        ));
        check_eq!(batch.rows.len(), 1);
        check_eq!(fixture.maps.map_applies.load(Ordering::Relaxed), 0);
        check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
        drop(batch);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn source_body_keeps_original_cancellation_before_limits_and_after_successful_read()
    -> Result<(), IndexError> {
        for case in 0..4 {
            let mut fixture = visitor_fixture()?;
            let mut record = fixture_record(&fixture)?;
            fixture.objects.counted_body = Some(record.bytes);
            if case == 0 {
                record.byte_len = MAX_SOURCE_BYTES
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            } else {
                if case != 1 {
                    // Valid immutable fixture bytes contradict this input length.
                    record.byte_len = 4;
                }
                if case != 3 {
                    fixture.objects.cancel_after_get = Some(record.bytes);
                }
            }
            let scratch = TestScratch::default();
            let custody = TestCustody;
            let mut work = budget()?;
            if case == 0 {
                work.request().cancel_handle().cancel();
            }
            let mut context = AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            let result = source_body(&record, &mut context);
            if case == 3 {
                check!(matches!(result, Err(IndexError::CorruptObject(_))));
            } else {
                check!(matches!(result, Err(IndexError::Cancelled)));
            }
            check_eq!(
                fixture.objects.body_reads.load(Ordering::Relaxed),
                usize::from(case != 0)
            );
            check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
            check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        }
        Ok(())
    }

    #[test]
    fn native_body_memory_refuses_nfc_or_ascii_batch_before_token_writes() -> Result<(), IndexError>
    {
        let fixture = visitor_fixture()?;
        let batch_bytes = u64::try_from(
            quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS * std::mem::size_of::<TokenRow>(),
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
        for (body, expected_request) in [("e\u{301}", 2), ("a", batch_bytes)] {
            let scratch = TestScratch::default();
            let custody = TestCustody;
            let calls = Arc::new(AtomicUsize::new(0));
            let requested = Arc::new(AtomicU64::new(0));
            let mut work = budget()?;
            let spool = TextSpool::new(&scratch, &mut work)?;
            work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
                RefuseTokenMemory {
                    calls: Arc::clone(&calls),
                    requested: Arc::clone(&requested),
                },
            ))));
            let mut context = AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            let before = fixture.objects.install_batches.load(Ordering::Relaxed);
            let mut inventory_edits = InventoryEdits::new(0)?;
            let mut writes = OccurrenceWrites::new();
            let result = prepare_inventory(
                body,
                &MapRoot(None).into(),
                0,
                &mut context,
                &spool,
                &mut ConfirmedOccurrences::new(0)?,
                &mut inventory_edits,
                &mut writes,
            );
            check!(matches!(result, Err(IndexError::ResourceLimited)));
            check_eq!(calls.load(Ordering::Relaxed), 1);
            check_eq!(requested.load(Ordering::Relaxed), expected_request);
            check!(spool.token_term_page(None, context.budget)?.is_empty());
            check_eq!(
                fixture.objects.install_batches.load(Ordering::Relaxed),
                before
            );
        }
        Ok(())
    }

    #[derive(Default)]
    struct TokenMemory {
        live: AtomicU64,
        peak: AtomicU64,
        calls: AtomicUsize,
        retained_calls: AtomicUsize,
        invalid_release: AtomicBool,
        ceiling: u64,
    }

    impl quanta_index_core::index::MemoryLeaseOwner for TokenMemory {
        fn release(&self, bytes: u64) {
            if self
                .live
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                    live.checked_sub(bytes)
                })
                .is_err()
            {
                self.invalid_release.store(true, Ordering::Relaxed);
            }
        }
    }

    struct TokenMemoryScope(Arc<TokenMemory>);

    fn token_memory_grant(
        memory: &Arc<TokenMemory>,
        bytes: u64,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        let _prior = memory.calls.fetch_add(1, Ordering::Relaxed);
        let before = memory
            .live
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                live.checked_add(bytes)
                    .filter(|next| *next <= memory.ceiling)
            })
            .map_err(|_live| IndexError::ResourceLimited)?;
        let live = before.checked_add(bytes).ok_or_else(|| {
            IndexError::BackendViolation("test memory admission lost checked sum".into())
        })?;
        let _peak = memory.peak.fetch_max(live, Ordering::Relaxed);
        Ok(quanta_index_core::index::MemoryLease::new(
            bytes,
            memory.clone(),
        ))
    }

    struct BatchOutputMemoryScope(Arc<TokenMemory>);

    impl quanta_index_core::index::MemoryScopePort for BatchOutputMemoryScope {
        fn reserve(
            &self,
            bytes: u64,
            lifetime: quanta_index_core::index::MemoryLifetime,
        ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
            if lifetime == quanta_index_core::index::MemoryLifetime::Retained {
                let _previous = self.0.retained_calls.fetch_add(1, Ordering::Relaxed);
            }
            token_memory_grant(&self.0, bytes)
        }
    }

    impl quanta_index_core::index::MemoryScopePort for TokenMemoryScope {
        fn reserve(
            &self,
            bytes: u64,
            lifetime: quanta_index_core::index::MemoryLifetime,
        ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
            check_eq!(
                lifetime,
                quanta_index_core::index::MemoryLifetime::Temporary
            );
            token_memory_grant(&self.0, bytes)
        }
    }

    fn funded_token_budget(ceiling: u64) -> Result<(WorkBudget, Arc<TokenMemory>), IndexError> {
        let memory = Arc::new(TokenMemory {
            ceiling,
            ..TokenMemory::default()
        });
        let mut work = budget()?;
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            TokenMemoryScope(Arc::clone(&memory)),
        ))));
        Ok((work, memory))
    }

    fn funded_batch_budget(ceiling: u64) -> Result<(WorkBudget, Arc<TokenMemory>), IndexError> {
        let (mut work, memory) = funded_token_budget(ceiling)?;
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            BatchOutputMemoryScope(Arc::clone(&memory)),
        ))));
        Ok((work, memory))
    }

    const fn inventory_packet_grant_bytes() -> u64 {
        (std::mem::size_of::<u64>()
            + quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH
                * std::mem::size_of::<ObjectRef>()
            + std::mem::size_of::<usize>()) as u64
    }

    #[test]
    fn pinned_member_decode_requires_original_parser_credit_and_refunds_it()
    -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        let wire = encode(&fixture.member)?;
        let floor = u64::try_from(wire.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(8)
            .and_then(|bytes| bytes.checked_add(512))
            .ok_or(IndexError::ResourceLimited)?;
        let index = NativeTextIndex::new(fixture.recipe);
        for ceiling in [floor - 1, floor] {
            let (mut work, memory) = funded_token_budget(ceiling)?;
            let result = index.pinned_root(
                &fixture.member,
                &mut TextReadContext {
                    ticket: &fixture.ticket,
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    budget: &mut work,
                },
            );
            if ceiling < floor {
                check!(matches!(result, Err(IndexError::ResourceLimited)));
            } else {
                let root = result?;
                check_eq!(root.docs, 1);
                check_eq!(root.indexed_tokens, 2);
                check_eq!(memory.peak.load(Ordering::Relaxed), floor);
            }
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        fixture.objects.add(
            fixture.member_ref,
            vec![b' '; quanta_index_core::index::MAX_BINDING_METADATA_BYTES + 1],
            vec![
                fixture.member.artifact,
                fixture.member.input.root.0.ok_or_else(|| {
                    IndexError::BackendViolation("fixture requires a source root".into())
                })?,
            ],
        )?;
        let (mut work, memory) = funded_token_budget(u64::MAX)?;
        check!(matches!(
            index.pinned_root(
                &fixture.member,
                &mut TextReadContext {
                    ticket: &fixture.ticket,
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    budget: &mut work,
                },
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(memory.calls.load(Ordering::Relaxed), 0);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn fixed_metadata_readers_require_exact_parser_credit_and_refund_it() -> Result<(), IndexError>
    {
        let fixture = visitor_fixture()?;
        for (kind, object, expected) in [
            (0, fixture.member.artifact, 1),
            (1, reference(10), 2),
            (2, fixture.posting_ref, 1),
        ] {
            let stored = fixture.objects.stored.get(&object).ok_or_else(|| {
                IndexError::BackendViolation("fixed metadata fixture is missing".into())
            })?;
            let floor = u64::try_from(stored.payload.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_mul(8)
                .and_then(|bytes| bytes.checked_add(512))
                .ok_or(IndexError::ResourceLimited)?;
            for ceiling in [floor - 1, floor] {
                let (mut work, memory) = funded_token_budget(ceiling)?;
                let result = match kind {
                    0 => read_root(&fixture.member, &fixture.objects, &mut work)
                        .map(|root| root.docs),
                    1 => read_contribution(object, &fixture.objects, &mut work)
                        .map(|contribution| contribution.indexed_tokens),
                    _ => {
                        read_posting(object, &fixture.objects, &mut work).map(|posting| posting.df)
                    }
                };
                if ceiling < floor {
                    check!(matches!(result, Err(IndexError::ResourceLimited)));
                    check_eq!(memory.peak.load(Ordering::Relaxed), 0);
                } else {
                    check_eq!(result?, expected);
                    check_eq!(memory.peak.load(Ordering::Relaxed), floor);
                }
                check_eq!(memory.live.load(Ordering::Relaxed), 0);
                check!(!memory.invalid_release.load(Ordering::Relaxed));
            }
        }
        Ok(())
    }

    #[test]
    fn fixed_metadata_bounds_and_original_cancellation_precede_json_errors()
    -> Result<(), IndexError> {
        let (work, memory) = funded_token_budget(520)?;
        check!(matches!(
            super::super::metadata::decode_fixed::<Root>(b"{", &work),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(memory.peak.load(Ordering::Relaxed), 520);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        let (denied, denied_memory) = funded_token_budget(519)?;
        check!(matches!(
            super::super::metadata::decode_fixed::<Root>(b"{", &denied),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(denied_memory.live.load(Ordering::Relaxed), 0);
        let oversized = vec![b' '; quanta_index_core::index::MAX_BINDING_METADATA_BYTES + 1];
        let (unfunded, unfunded_memory) = funded_token_budget(0)?;
        check!(matches!(
            super::super::metadata::decode_fixed::<Root>(&oversized, &unfunded),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(unfunded_memory.calls.load(Ordering::Relaxed), 0);
        work.request().cancel_handle().cancel();
        check!(matches!(
            super::super::metadata::decode_fixed::<Root>(&oversized, &work),
            Err(IndexError::Cancelled)
        ));
        check_eq!(memory.calls.load(Ordering::Relaxed), 1);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn inventory_reuse_requires_exact_input_policy_and_refunds_its_bound() -> Result<(), IndexError>
    {
        let fixture = visitor_fixture()?;
        let record = fixture_record(&fixture)?;
        check!(matches!(
            NativeTextIndex::new(fixture.recipe).with_inventory_reuse_limit(65),
            Err(IndexError::InvalidInput(_))
        ));
        let (work, memory) = funded_token_budget(1_000_000)?;
        let mut reuse = InventoryReuse::new(fixture.recipe, 1)?;
        reuse.remember(&record, &fixture.inventory_root.into(), 1, &work)?;
        let renamed = SourceRecord {
            path: "renamed.rs".into(),
            ..record.clone()
        };
        check_eq!(
            reuse.lookup(&renamed, &work)?,
            Some((fixture.inventory_root.into(), 1))
        );
        let different_body = SourceRecord {
            bytes: reference(99),
            ..record.clone()
        };
        check!(reuse.lookup(&different_body, &work)?.is_none());
        let wrong_length = SourceRecord {
            byte_len: record
                .byte_len
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?,
            ..record.clone()
        };
        check!(reuse.lookup(&wrong_length, &work)?.is_none());
        let excluded = SourceRecord {
            text: SourceTextAdmission::Excluded,
            ..record.clone()
        };
        check!(reuse.lookup(&excluded, &work)?.is_none());
        let wrong_policy = SourceRecord {
            text: SourceTextAdmission::Utf8 {
                policy: Digest([99; 32]),
            },
            ..record.clone()
        };
        check!(matches!(
            reuse.lookup(&wrong_policy, &work),
            Err(IndexError::InvalidInput(_))
        ));
        let before = memory.live.load(Ordering::Relaxed);
        reuse.remember(&different_body, &MapRoot(None).into(), 0, &work)?;
        check!(reuse.lookup(&different_body, &work)?.is_none());
        check_eq!(memory.live.load(Ordering::Relaxed), before);
        check!(before > 0);
        work.request().cancel_handle().cancel();
        check!(matches!(
            reuse.lookup(&record, &work),
            Err(IndexError::Cancelled)
        ));
        drop(reuse);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        let (work, memory) = funded_token_budget(0)?;
        let mut denied = InventoryReuse::new(fixture.recipe, 1)?;
        check!(matches!(
            denied.remember(&record, &fixture.inventory_root.into(), 1, &work),
            Err(IndexError::ResourceLimited)
        ));
        check!(denied.lookup(&record, &work)?.is_none());
        drop(denied);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        let mut disabled = InventoryReuse::new(fixture.recipe, 0)?;
        disabled.remember(&record, &fixture.inventory_root.into(), 1, &work)?;
        check!(disabled.lookup(&record, &work)?.is_none());
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn confirmed_occurrences_are_bounded_attempt_local_and_keep_original_custody()
    -> Result<(), IndexError> {
        let fixture = visitor_fixture()?;
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let foreign = quanta_index_core::index::AttemptRef {
            fence: attempt
                .fence
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?,
            ..attempt
        };
        let (work, memory) = funded_token_budget(1_000_000)?;
        let mut confirmed = ConfirmedOccurrences::new(2)?;
        check!(!confirmed.contains(reference(90), attempt, &work)?);
        check!(matches!(
            confirmed.remember(reference(90), attempt, &work),
            Err(IndexError::BackendViolation(_))
        ));
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        confirmed.prepare(attempt, &work)?;
        let bytes = u64::try_from(std::mem::size_of::<ConfirmedOccurrence>())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(memory.live.load(Ordering::Relaxed), bytes);
        confirmed.remember(reference(90), attempt, &work)?;
        confirmed.remember(reference(90), attempt, &work)?;
        confirmed.remember(reference(91), attempt, &work)?;
        confirmed.remember(reference(92), attempt, &work)?;
        check_eq!(
            confirmed.references.as_slice(),
            [
                ConfirmedOccurrence {
                    reference: reference(90),
                    validated_count: None,
                },
                ConfirmedOccurrence {
                    reference: reference(91),
                    validated_count: None,
                },
            ]
        );
        check!(!confirmed.contains(reference(92), attempt, &work)?);
        check!(matches!(
            confirmed.contains(reference(90), foreign, &work),
            Err(IndexError::FenceLost)
        ));
        check!(matches!(
            confirmed.prepare(foreign, &work),
            Err(IndexError::FenceLost)
        ));
        check!(matches!(
            confirmed.remember(reference(92), foreign, &work),
            Err(IndexError::FenceLost)
        ));
        check_eq!(memory.live.load(Ordering::Relaxed), bytes);
        work.request().cancel_handle().cancel();
        check!(matches!(
            confirmed.contains(reference(90), attempt, &work),
            Err(IndexError::Cancelled)
        ));
        check!(matches!(
            confirmed.remember(reference(92), attempt, &work),
            Err(IndexError::Cancelled)
        ));
        drop(confirmed);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn confirmed_new_count_does_not_memoize_bad_read_or_bypass_fence_and_cancel()
    -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        let malformed = reference(53);
        fixture
            .objects
            .add(malformed, b"{\"positions\":[]}".to_vec(), Vec::new())?;
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let foreign = quanta_index_core::index::AttemptRef {
            fence: attempt
                .fence
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?,
            ..attempt
        };
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut work = budget()?;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut confirmed = ConfirmedOccurrences::new(1)?;
        confirmed.prepare(attempt, &work)?;
        confirmed.remember(malformed, attempt, &work)?;
        {
            let mut context = AttemptContext {
                attempt,
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            check!(matches!(
                confirmed.validated_new_count(
                    fixture.term.as_slice(),
                    malformed,
                    &spool,
                    &mut context
                ),
                Err(IndexError::CorruptObject(_))
            ));
            check_eq!(
                confirmed
                    .references
                    .first()
                    .map(|entry| entry.validated_count),
                Some(None)
            );
        }
        fixture.objects.add(
            malformed,
            encode(&Occurrence {
                positions: Positions(vec![0, 2]),
            })?,
            Vec::new(),
        )?;
        let mut context = AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        check_eq!(
            confirmed.validated_new_count(
                fixture.term.as_slice(),
                malformed,
                &spool,
                &mut context
            )?,
            2
        );
        check_eq!(
            confirmed
                .references
                .first()
                .map(|entry| entry.validated_count),
            Some(Some(2))
        );
        context.attempt = foreign;
        check!(matches!(
            confirmed.validated_new_count(fixture.term.as_slice(), malformed, &spool, &mut context),
            Err(IndexError::FenceLost)
        ));
        context.attempt = attempt;
        context.budget.request().cancel_handle().cancel();
        check!(matches!(
            confirmed.validated_new_count(fixture.term.as_slice(), malformed, &spool, &mut context),
            Err(IndexError::Cancelled)
        ));
        Ok(())
    }

    #[test]
    fn confirmed_occurrence_backing_refuses_before_install_and_disabled_window_allocates_nothing()
    -> Result<(), IndexError> {
        let fixture = visitor_fixture()?;
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let (work, memory) = funded_token_budget(0)?;
        let mut denied = ConfirmedOccurrences::new(1)?;
        check!(matches!(
            denied.prepare(attempt, &work),
            Err(IndexError::ResourceLimited)
        ));
        check!(denied.references.is_empty());
        check!(denied.attempt.is_none());
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        let mut disabled = ConfirmedOccurrences::new(0)?;
        disabled.prepare(attempt, &work)?;
        disabled.remember(reference(90), attempt, &work)?;
        check!(!disabled.contains(reference(90), attempt, &work)?);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(matches!(
            ConfirmedOccurrences::new(65),
            Err(IndexError::InvalidInput(_))
        ));
        Ok(())
    }

    #[test]
    fn occurrence_reuse_never_remembers_refused_or_malformed_batch_prefix() -> Result<(), IndexError>
    {
        #[derive(Serialize)]
        struct FixedOccurrence {
            positions: [u32; 1],
        }
        for case in 0..3 {
            let mut fixture = visitor_fixture()?;
            if case == 2 {
                fixture.objects.install_outputs = Some(vec![reference(99)]);
            }
            let scratch = TestScratch::default();
            let custody = TestCustody;
            let mut setup = budget()?;
            let spool = TextSpool::new(&scratch, &mut setup)?;
            // Inputs are explicit unscoped fixture data; the used flush itself
            // receives the tested scope before any SQL/cache/output arrays.
            let mut writes = OccurrenceWrites::new();
            let mut inventory_edits = InventoryEdits::new(0)?;
            for key in [b"\0a", b"\x01a"] {
                let payload =
                    super::super::text_memory::encode(&FixedOccurrence { positions: [0] }, &setup)?;
                writes.push(
                    OccurrenceEdit {
                        ordinal: 0,
                        key: PreparedKey::copy(key, &setup)?,
                        prior: None,
                        identified: reference(90),
                    },
                    payload,
                    &spool,
                    &mut inventory_edits,
                    &mut setup,
                )?;
            }
            let (mut work, memory) = funded_batch_budget(if case == 0 { 0 } else { 1_000_000 })?;
            let mut confirmed = ConfirmedOccurrences::new(2)?;
            let mut context = AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            let result = writes.flush(&mut context, &spool, &mut confirmed, &mut inventory_edits);
            match case {
                0 => check!(matches!(result, Err(IndexError::ResourceLimited))),
                1 => check!(matches!(result, Err(IndexError::BackendViolation(message))
                    if message == "changed-body occurrence build reached object batch")),
                _ => check!(matches!(result, Err(IndexError::BackendViolation(message))
                    if message == "native text occurrence identity changed during installation")),
            }
            check_eq!(
                fixture.objects.install_batches.load(Ordering::Relaxed),
                usize::from(case != 0)
            );
            check!(confirmed.references.is_empty());
            check_eq!(writes.pending.len(), 1);
            check_eq!(writes.fanout_rows, 2);
            if case == 0 {
                check!(spool.inventory_cursor(1)?.next_edit(&mut setup)?.is_none());
            } else {
                check!(spool.inventory_cursor(1)?.next_edit(&mut setup).is_err());
            }
            drop(inventory_edits);
            drop(confirmed);
            drop(writes);
            check_eq!(
                memory.live.load(Ordering::Relaxed),
                if case == 0 {
                    0
                } else {
                    inventory_packet_grant_bytes()
                }
            );
            drop(spool);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        Ok(())
    }

    fn wrapper_contribution_inventory(child: ObjectRef) -> Result<EntryMap, IndexError> {
        let mut small = entry_map::SmallMap::default();
        // A valid Small key whose contribution data exceeds the 32-byte inline limit.
        check!(small.push(b"\0abcdefghijklmnopqrstuvw", child)?);
        Ok(EntryMap::inline(&small))
    }

    #[test]
    fn contribution_packet_denial_cancel_and_refusal_preserve_custody() -> Result<(), IndexError> {
        for ceiling in [0, 64] {
            let fixture = visitor_fixture()?;
            let scratch = TestScratch::default();
            let spool = TextSpool::new(&scratch, &mut budget()?)?;
            let custody = TestCustody;
            let (mut work, memory) = funded_batch_budget(ceiling)?;
            let mut context = AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            let mut writes = ContributionWrites::new()?;
            let result = writes.push(
                [1; 32],
                None,
                &Contribution {
                    record: fixture.source_ref,
                    inventory: wrapper_contribution_inventory(reference(9))?,
                    indexed_tokens: 1,
                },
                &mut context,
                &spool,
            );
            check!(matches!(result, Err(IndexError::ResourceLimited)));
            check_eq!(writes.pending.len(), 0);
            check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
            check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
            drop(writes);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        for cancelled in [false, true] {
            let fixture = visitor_fixture()?;
            let scratch = TestScratch::default();
            let spool = TextSpool::new(&scratch, &mut budget()?)?;
            let custody = TestCustody;
            let (mut work, memory) = funded_batch_budget(1_000_000)?;
            let mut context = AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            let mut writes = ContributionWrites::new()?;
            for owner in [[1; 32], [2; 32]] {
                writes.push(
                    owner,
                    None,
                    &Contribution {
                        record: fixture.source_ref,
                        inventory: wrapper_contribution_inventory(reference(9))?,
                        indexed_tokens: 1,
                    },
                    &mut context,
                    &spool,
                )?;
            }
            check_eq!(writes.pending.len(), 2);
            check!(memory.live.load(Ordering::Relaxed) > 0);
            if cancelled {
                context.budget.request().cancel_handle().cancel();
            }
            let result = writes.flush(&mut context, &spool);
            if cancelled {
                check!(matches!(result, Err(IndexError::Cancelled)));
            } else {
                check!(matches!(result, Err(IndexError::BackendViolation(message))
                    if message == "changed-body occurrence build reached object batch"));
            }
            check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
            check_eq!(
                fixture.objects.install_batches.load(Ordering::Relaxed),
                usize::from(!cancelled)
            );
            check_eq!(
                memory.retained_calls.load(Ordering::Relaxed),
                usize::from(!cancelled)
            );
            // A queued or refused installation is never a top-map reference.
            check!(spool.top_cursor().next_edit(&mut budget()?)?.is_none());
            check!(memory.live.load(Ordering::Relaxed) > 0);
            drop(writes);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        Ok(())
    }

    #[test]
    fn inline_contribution_packet_is_funded_and_publishes_no_wrapper() -> Result<(), IndexError> {
        for cancelled in [false, true] {
            let fixture = visitor_fixture()?;
            let scratch = TestScratch::default();
            let spool = TextSpool::new(&scratch, &mut budget()?)?;
            let custody = TestCustody;
            let (mut work, memory) = funded_batch_budget(1_000_000)?;
            let logical = Contribution {
                record: fixture.source_ref,
                inventory: EntryMap::default(),
                indexed_tokens: 0,
            };
            let expected = MapValue::inline(&[0; 9], Some(fixture.source_ref))?;
            let mut writes = ContributionWrites::new()?;
            let mut context = AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            writes.push([1; 32], None, &logical, &mut context, &spool)?;
            check!(
                memory.live.load(Ordering::Relaxed)
                    >= u64::try_from(std::mem::size_of::<PendingContribution>())
                        .map_err(|_error| IndexError::ResourceLimited)?
            );
            if cancelled {
                context.budget.request().cancel_handle().cancel();
            }
            let result = writes.flush(&mut context, &spool);
            if cancelled {
                check!(matches!(result, Err(IndexError::Cancelled)));
                check!(spool.top_cursor().next_edit(&mut budget()?)?.is_none());
            } else {
                result?;
                let row = spool
                    .top_cursor()
                    .next_edit(&mut budget()?)?
                    .ok_or_else(|| {
                        IndexError::BackendViolation("compact contribution edit missing".into())
                    })?;
                check_eq!(row.edit.next, Some(expected));
            }
            check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
            check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
            drop(writes);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        Ok(())
    }

    #[test]
    fn mixed_contribution_packet_preserves_logical_order_and_actual_install_count()
    -> Result<(), IndexError> {
        let fixture = visitor_fixture()?;
        let scratch = TestScratch::default();
        let spool = TextSpool::new(&scratch, &mut budget()?)?;
        let custody = TestCustody;
        let (mut work, memory) = funded_batch_budget(1_000_000)?;
        let compact = Contribution {
            record: fixture.source_ref,
            inventory: EntryMap::default(),
            indexed_tokens: 0,
        };
        let object = Contribution {
            record: fixture.source_ref,
            inventory: wrapper_contribution_inventory(reference(9))?,
            indexed_tokens: 1,
        };
        let inventory_root = fixture
            .inventory_root
            .0
            .ok_or_else(|| IndexError::BackendViolation("test inventory root missing".into()))?;
        let rooted = Contribution {
            record: fixture.source_ref,
            inventory: EntryMap::Tree(inventory_root),
            indexed_tokens: 2,
        };
        let inline_gold = MapValue::inline(&[0; 9], Some(fixture.source_ref))?;
        let rooted_gold = MapValue::inline(
            b"\x81\0\0\0\0\0\0\0\x02",
            [fixture.source_ref, inventory_root],
        )?;
        let mut context = AttemptContext {
            attempt: test_attempt(fixture.ticket.view.identity.workspace),
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let mut writes = ContributionWrites::new()?;
        for (id, logical) in [(1, &compact), (2, &object), (3, &rooted)] {
            writes.push([id; 32], None, logical, &mut context, &spool)?;
        }
        writes.flush(&mut context, &spool)?;
        let mut cursor = spool.top_cursor();
        for (id, expected) in [
            (1, inline_gold),
            (2, MapValue::Object(reference(40))),
            (3, rooted_gold),
        ] {
            let row = cursor.next_edit(&mut budget()?)?.ok_or_else(|| {
                IndexError::BackendViolation("mixed contribution row missing".into())
            })?;
            check_eq!(row.edit.key.as_slice(), &[id; 32]);
            check_eq!(row.edit.expected, None);
            check_eq!(row.edit.next, Some(expected));
        }
        check!(cursor.next_edit(&mut budget()?)?.is_none());
        check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 1);
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        drop(writes);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn cold_contribution_batch_spans_two_sql_pages() -> Result<(), IndexError> {
        let fixture = visitor_fixture()?;
        let scratch = TestScratch::default();
        let spool = TextSpool::new(&scratch, &mut budget()?)?;
        let custody = TestCustody;
        let (mut work, memory) = funded_batch_budget(1_000_000)?;
        let inventory_root = fixture
            .inventory_root
            .0
            .ok_or_else(|| IndexError::BackendViolation("test inventory root missing".into()))?;
        let mut context = AttemptContext {
            attempt: test_attempt(fixture.ticket.view.identity.workspace),
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let mut writes = ContributionWrites::cold()?;
        for id in 0..65 {
            writes.push(
                [id; 32],
                None,
                &Contribution {
                    record: reference(id + 80),
                    inventory: EntryMap::Tree(inventory_root),
                    indexed_tokens: 2,
                },
                &mut context,
                &spool,
            )?;
        }
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        writes.flush(&mut context, &spool)?;
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);

        let mut cursor = spool.top_cursor();
        let mut read = budget()?;
        for id in 0..65 {
            let row = cursor.next_edit(&mut read)?.ok_or_else(|| {
                IndexError::BackendViolation("cold contribution row missing".into())
            })?;
            check_eq!(row.edit.key.as_slice(), &[id; 32]);
            check_eq!(row.edit.expected, None);
            let expected = MapValue::inline(
                b"\x81\0\0\0\0\0\0\0\x02",
                [reference(id + 80), inventory_root],
            )?;
            check_eq!(row.edit.next, Some(expected));
        }
        check!(cursor.next_edit(&mut read)?.is_none());
        drop(writes);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn query_term_keys_keep_canonical_unicode_and_refund_after_denial_or_cancel()
    -> Result<(), IndexError> {
        for (text, case, gold) in [
            ("...one...", CaseMode::Sensitive, b"\0one".as_slice()),
            ("CAFE\u{301}", CaseMode::Folded, "\u{1}café".as_bytes()),
            ("İ", CaseMode::Folded, "\u{1}i\u{307}".as_bytes()),
            ("ΟΣ", CaseMode::Folded, "\u{1}οσ".as_bytes()),
            ("ß", CaseMode::Folded, "\u{1}ß".as_bytes()),
        ] {
            let (mut work, memory) = funded_token_budget(16_384)?;
            let key = admitted_query_term(text, case, &mut work)?;
            check_eq!(key.as_slice(), gold);
            check_eq!(
                memory.live.load(Ordering::Relaxed),
                u64::try_from(gold.len()).map_err(|_error| IndexError::ResourceLimited)?
            );
            drop(key);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        for text in ["!!!".to_owned(), "one two".to_owned(), "x".repeat(257)] {
            let (mut work, memory) = funded_token_budget(16_384)?;
            check!(matches!(
                admitted_query_term(&text, CaseMode::Folded, &mut work),
                Err(IndexError::InvalidInput(_))
            ));
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
        }
        let (mut work, memory) = funded_token_budget(3)?;
        check!(matches!(
            admitted_query_term("one", CaseMode::Sensitive, &mut work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        let calls = memory.calls.load(Ordering::Relaxed);
        work.request().cancel_handle().cancel();
        check!(matches!(
            admitted_query_term("one", CaseMode::Sensitive, &mut work),
            Err(IndexError::Cancelled)
        ));
        check_eq!(memory.calls.load(Ordering::Relaxed), calls);
        Ok(())
    }

    struct QueryMemoryScope {
        memory: Arc<TokenMemory>,
        refuse_retained: bool,
    }

    #[test]
    fn query_source_decode_refuses_owned_path_before_overlong_record_validation()
    -> Result<(), IndexError> {
        use quanta_index_contract::{LqCase, LqExpr, LqLeaf, LqQuery, LqSpan};

        let mut fixture = visitor_fixture()?;
        let mut record = fixture_record(&fixture)?;
        // The raw-object test port deliberately does not charge its input. The
        // newly decoded String must still be refused by the real query owner.
        record.path = "x".repeat(1_048_576);
        fixture
            .objects
            .add(fixture.source_ref, encode(&record)?, vec![record.bytes])?;
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = LqExpr::Leaf(LqLeaf::Keyword("one".into()));
        query.options.case = Some(LqCase::Sensitive);
        let plan = crate::planner::LexicalPlanner::plan(&query)
            .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let (mut work, memory) = funded_token_budget(1_048_576)?;
        let result = NativeTextIndex::new(fixture.recipe).search_content(
            &fixture.member,
            &plan,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            1,
            super::super::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
            TextReadContext {
                ticket: &fixture.ticket,
                objects: &fixture.objects,
                maps: &fixture.maps,
                budget: &mut work,
            },
        );
        check!(matches!(result, Err(IndexError::ResourceLimited)));
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    impl quanta_index_core::index::MemoryScopePort for QueryMemoryScope {
        fn reserve(
            &self,
            bytes: u64,
            lifetime: quanta_index_core::index::MemoryLifetime,
        ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
            if self.refuse_retained
                && lifetime == quanta_index_core::index::MemoryLifetime::Retained
            {
                return Err(IndexError::ResourceLimited);
            }
            // Reuse this fixture's byte ceiling/release oracle for both classes.
            quanta_index_core::index::MemoryScopePort::reserve(
                &TokenMemoryScope(Arc::clone(&self.memory)),
                bytes,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )
        }
    }

    #[test]
    fn query_workspace_refusal_and_ranking_transfer_preserve_allocation_custody()
    -> Result<(), IndexError> {
        use quanta_index_contract::{LqCase, LqExpr, LqLeaf, LqQuery, LqSpan};

        let fixture = visitor_fixture()?;
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = LqExpr::Leaf(LqLeaf::Keyword("one".into()));
        query.options.case = Some(LqCase::Sensitive);
        let plan = crate::planner::LexicalPlanner::plan(&query)
            .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        for (ceiling, refuse_retained) in [(1, false), (1_048_576, true), (1_048_576, false)] {
            let (mut work, memory) = funded_token_budget(ceiling)?;
            work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
                QueryMemoryScope {
                    memory: Arc::clone(&memory),
                    refuse_retained,
                },
            ))));
            let lookups = fixture.maps.term_lookups.load(Ordering::Relaxed);
            let outcome = NativeTextIndex::new(fixture.recipe).search_content(
                &fixture.member,
                &plan,
                &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                1,
                super::super::text_query::Bm25Policy { k1: 1.2, b: 0.75 },
                TextReadContext {
                    ticket: &fixture.ticket,
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    budget: &mut work,
                },
            );
            if ceiling == 1 || refuse_retained {
                check!(matches!(outcome, Err(IndexError::ResourceLimited)));
                if ceiling == 1 {
                    check_eq!(fixture.maps.term_lookups.load(Ordering::Relaxed), lookups);
                }
            } else {
                let ranking = outcome?;
                check_eq!(ranking.matched, 1);
                check_eq!(ranking.hits.len(), 1);
                let hit = ranking.hits.first().ok_or_else(|| {
                    IndexError::BackendViolation("ranking lost its expected hit".into())
                })?;
                check_eq!(hit.owner, fixture.owner);
                let hit_bytes = ranking
                    .hits
                    .capacity()
                    .checked_mul(std::mem::size_of::<
                        super::super::text_query::NativeTextRankedHit,
                    >())
                    .ok_or(IndexError::ResourceLimited)?;
                check_eq!(
                    memory.live.load(Ordering::Relaxed),
                    u64::try_from(hit_bytes).map_err(|_error| IndexError::ResourceLimited)?
                );
                check!(memory.peak.load(Ordering::Relaxed) > memory.live.load(Ordering::Relaxed));
                drop(ranking);
            }
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        Ok(())
    }

    fn one_source_run(text: &str, position: u32) -> [SourceRun; 1] {
        [SourceRun {
            start: 0,
            end: text.len(),
            position,
        }]
    }

    fn token_row_backing() -> Result<u64, IndexError> {
        let bytes = 64usize
            .checked_mul(std::mem::size_of::<TokenRow>())
            .ok_or(IndexError::ResourceLimited)?;
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
    }

    #[test]
    fn ascii_token_packet_matches_canonical_case_surfaces_beside_unicode() -> Result<(), IndexError>
    {
        let (work, memory) = funded_token_budget(4096)?;
        let source = "AbC_09 İ";
        let runs = [
            SourceRun {
                start: 0,
                end: 6,
                position: 3,
            },
            SourceRun {
                start: 7,
                end: source.len(),
                position: 4,
            },
        ];
        let mut batch = TokenBatch::new(&work)?;
        batch.prepare(source, &runs, &work)?;
        check_eq!(batch.rows().len(), 4);
        for (slot, (run, text)) in runs.iter().zip(["AbC_09", "İ"]).enumerate() {
            for (case, surface) in [(CaseMode::Sensitive, 0_u8), (CaseMode::Folded, 1_u8)] {
                let ordinal = slot
                    .checked_mul(2)
                    .and_then(|index| index.checked_add(usize::from(surface)))
                    .ok_or(IndexError::ResourceLimited)?;
                let row = batch
                    .rows()
                    .get(ordinal)
                    .ok_or(IndexError::ResourceLimited)?;
                let canonical: String = case_chars(text, case).collect();
                let mut expected = vec![surface];
                expected.extend_from_slice(canonical.as_bytes());
                check_eq!(batch.term(row)?, expected.as_slice());
                check_eq!(row.position(), run.position);
            }
        }
        drop(batch);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn funded_token_keys_preserve_fixed_unicode_surfaces_and_positions() -> Result<(), IndexError> {
        let (work, memory) = funded_token_budget(4096)?;
        for (input, sensitive, folded) in [
            ("A", b"\0A".as_slice(), b"\x01a".as_slice()),
            ("İ", b"\0\xc4\xb0".as_slice(), b"\x01i\xcc\x87".as_slice()),
            ("ΟΔΟΣ", "\0ΟΔΟΣ".as_bytes(), "\x01οδοσ".as_bytes()),
            ("ß", "\0ß".as_bytes(), "\x01ß".as_bytes()),
            ("CAFÉ", "\0CAFÉ".as_bytes(), "\x01café".as_bytes()),
        ] {
            let mut batch = TokenBatch::new(&work)?;
            batch.prepare(input, &one_source_run(input, 37), &work)?;
            for (row, expected) in batch.rows().iter().zip([sensitive, folded]) {
                check_eq!(batch.term(row)?, expected);
                check_eq!(row.position, 37);
            }
            check_eq!(batch.rows().len(), 2);
            let key_bytes = sensitive
                .len()
                .checked_add(folded.len())
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(
                memory.live.load(Ordering::Relaxed),
                token_row_backing()?
                    .checked_add(
                        u64::try_from(key_bytes).map_err(|_error| IndexError::ResourceLimited)?
                    )
                    .ok_or(IndexError::ResourceLimited)?
            );
            drop(batch);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
        }
        let source = "İ".repeat(128);
        let mut batch = TokenBatch::new(&work)?;
        batch.prepare(&source, &one_source_run(&source, 0), &work)?;
        let [sensitive, folded] = batch.rows() else {
            return Err(IndexError::BackendViolation(
                "case packet lost two surfaces".into(),
            ));
        };
        let sensitive_gold = format!("\0{}", "İ".repeat(128));
        let folded_gold = format!("\x01{}", "i\u{307}".repeat(128));
        check_eq!(batch.term(sensitive)?, sensitive_gold.as_bytes());
        check_eq!(batch.term(folded)?, folded_gold.as_bytes());
        check_eq!(
            memory.live.load(Ordering::Relaxed),
            token_row_backing()?
                .checked_add(642)
                .ok_or(IndexError::ResourceLimited)?
        );
        drop(batch);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn token_key_cap_refuses_before_reservation_and_accepts_exact_boundary()
    -> Result<(), IndexError> {
        let (work, memory) = funded_token_budget(
            token_row_backing()?
                .checked_add(2048)
                .ok_or(IndexError::ResourceLimited)?,
        )?;
        let mut batch = TokenBatch::new(&work)?;
        for input in [String::new(), "a".repeat(1024), "İ".repeat(342)] {
            check!(matches!(
                batch.prepare(&input, &one_source_run(&input, 0), &work),
                Err(IndexError::ResourceLimited)
            ));
        }
        check_eq!(memory.calls.load(Ordering::Relaxed), 1);
        check!(batch.keys.is_empty());
        let input = "A".repeat(1023);
        batch.prepare(&input, &one_source_run(&input, 0), &work)?;
        for row in batch.rows() {
            check_eq!(batch.term(row)?.len(), 1024);
        }
        check_eq!(
            memory.live.load(Ordering::Relaxed),
            token_row_backing()?
                .checked_add(2048)
                .ok_or(IndexError::ResourceLimited)?
        );
        drop(batch);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn token_arena_reuse_holds_two_grants_and_refuses_oversized_packet_before_growth()
    -> Result<(), IndexError> {
        let (work, memory) = funded_token_budget(16_384)?;
        let mut batch = TokenBatch::new(&work)?;
        let backing = memory.live.load(Ordering::Relaxed);
        check_eq!(
            backing,
            u64::try_from(64 * std::mem::size_of::<TokenRow>())
                .map_err(|_error| IndexError::ResourceLimited)?
        );
        let capacity = batch.rows.capacity();
        let full = backing
            .checked_add(128)
            .ok_or(IndexError::ResourceLimited)?;
        let input = "a ".repeat(32);
        let runs = (0..32u32)
            .map(|position| {
                let start = usize::try_from(position)
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .checked_mul(2)
                    .ok_or(IndexError::ResourceLimited)?;
                Ok(SourceRun {
                    start,
                    end: start.checked_add(1).ok_or(IndexError::ResourceLimited)?,
                    position,
                })
            })
            .collect::<Result<Vec<_>, IndexError>>()?;
        batch.prepare(&input, &runs, &work)?;
        check_eq!(memory.live.load(Ordering::Relaxed), full);
        check_eq!(memory.calls.load(Ordering::Relaxed), 2);
        let oversized = [SourceRun::default(); 33];
        check!(matches!(
            batch.prepare(&input, &oversized, &work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(batch.rows.len(), 64);
        check_eq!(batch.rows.capacity(), capacity);
        check_eq!(memory.live.load(Ordering::Relaxed), full);
        check_eq!(memory.calls.load(Ordering::Relaxed), 2);
        batch.clear();
        check_eq!(memory.live.load(Ordering::Relaxed), full);
        batch.prepare("İ", &one_source_run("İ", 65), &work)?;
        check_eq!(batch.rows.capacity(), capacity);
        check_eq!(memory.live.load(Ordering::Relaxed), full);
        check_eq!(memory.calls.load(Ordering::Relaxed), 2);
        drop(batch);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn token_packet_refusal_precedes_key_birth_and_releases_metadata() -> Result<(), IndexError> {
        let backing = u64::try_from(64 * std::mem::size_of::<TokenRow>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let ceiling = backing.checked_add(3).ok_or(IndexError::ResourceLimited)?;
        let (work, memory) = funded_token_budget(ceiling)?;
        let result = (|| -> Result<(), IndexError> {
            let mut batch = TokenBatch::new(&work)?;
            let result = batch.prepare("İ", &one_source_run("İ", 0), &work);
            check!(batch.keys.is_empty());
            check!(batch.rows.is_empty());
            result?;
            Ok(())
        })();
        check!(matches!(result, Err(IndexError::ResourceLimited)));
        check_eq!(memory.peak.load(Ordering::Relaxed), backing);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn token_arena_growth_admits_old_and_new_backing_and_preserves_old_packet_on_refusal()
    -> Result<(), IndexError> {
        let backing = token_row_backing()?;
        for extra in [10u64, 11] {
            let (work, memory) = funded_token_budget(
                backing
                    .checked_add(extra)
                    .ok_or(IndexError::ResourceLimited)?,
            )?;
            let mut batch = TokenBatch::new(&work)?;
            batch.prepare("A", &one_source_run("A", 7), &work)?;
            let result = batch.prepare("İ", &one_source_run("İ", 8), &work);
            let [sensitive, folded] = batch.rows() else {
                return Err(IndexError::BackendViolation(
                    "case packet lost two surfaces".into(),
                ));
            };
            let (live, peak) = if extra == 10 {
                check!(matches!(result, Err(IndexError::ResourceLimited)));
                check_eq!(batch.term(sensitive)?, b"\0A");
                check_eq!(batch.term(folded)?, b"\x01a");
                check_eq!(sensitive.position(), 7);
                (4, 4)
            } else {
                result?;
                check_eq!(batch.term(sensitive)?, "\0İ".as_bytes());
                check_eq!(batch.term(folded)?, "\x01i\u{307}".as_bytes());
                check_eq!(sensitive.position(), 8);
                (7, 11)
            };
            check_eq!(
                memory.live.load(Ordering::Relaxed),
                backing
                    .checked_add(live)
                    .ok_or(IndexError::ResourceLimited)?
            );
            check_eq!(
                memory.peak.load(Ordering::Relaxed),
                backing
                    .checked_add(peak)
                    .ok_or(IndexError::ResourceLimited)?
            );
            drop(batch);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        Ok(())
    }

    struct CancelArenaGrowth {
        memory: Arc<TokenMemory>,
        request: quanta_index_core::RequestBudgetV1,
    }

    impl quanta_index_core::index::MemoryScopePort for CancelArenaGrowth {
        fn reserve(
            &self,
            bytes: u64,
            lifetime: quanta_index_core::index::MemoryLifetime,
        ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
            let lease = quanta_index_core::index::MemoryScopePort::reserve(
                &TokenMemoryScope(Arc::clone(&self.memory)),
                bytes,
                lifetime,
            )?;
            if self.memory.calls.load(Ordering::Relaxed) == 3 {
                self.request.cancel_handle().cancel();
            }
            Ok(lease)
        }
    }

    #[test]
    fn token_arena_growth_cancellation_releases_new_grant_and_preserves_old_packet()
    -> Result<(), IndexError> {
        let (mut work, memory) = funded_token_budget(16_384)?;
        let request = work.request().clone();
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            CancelArenaGrowth {
                memory: Arc::clone(&memory),
                request,
            },
        ))));
        let mut batch = TokenBatch::new(&work)?;
        batch.prepare("A", &one_source_run("A", 7), &work)?;
        check!(matches!(
            batch.prepare("İ", &one_source_run("İ", 8), &work),
            Err(IndexError::Cancelled)
        ));
        let [sensitive, folded] = batch.rows() else {
            return Err(IndexError::BackendViolation(
                "cancelled growth lost old packet".into(),
            ));
        };
        check_eq!(batch.term(sensitive)?, b"\0A");
        check_eq!(batch.term(folded)?, b"\x01a");
        check_eq!(
            memory.live.load(Ordering::Relaxed),
            token_row_backing()?
                .checked_add(4)
                .ok_or(IndexError::ResourceLimited)?
        );
        check_eq!(
            memory.peak.load(Ordering::Relaxed),
            token_row_backing()?
                .checked_add(11)
                .ok_or(IndexError::ResourceLimited)?
        );
        drop(batch);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn token_arena_panic_after_birth_releases_both_grants() {
        let result = (|| -> Result<bool, IndexError> {
            let (work, memory) = funded_token_budget(16_384)?;
            let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut batch = TokenBatch::new(&work)?;
                batch.prepare("A", &one_source_run("A", 0), &work)?;
                check_eq!(memory.calls.load(Ordering::Relaxed), 2);
                assert_eq!(
                    memory.live.load(Ordering::Relaxed),
                    0,
                    "injected panic after admitted arena birth"
                );
                Ok::<(), IndexError>(())
            }));
            Ok(unwind.is_err()
                && memory.live.load(Ordering::Relaxed) == 0
                && !memory.invalid_release.load(Ordering::Relaxed))
        })();
        assert!(
            matches!(result, Ok(true)),
            "arena panic cleanup differs: {result:?}"
        );
    }

    #[test]
    fn prepared_positions_growth_refuses_before_copy_and_releases_backing() -> Result<(), IndexError>
    {
        let (work, memory) = funded_token_budget(11)?;
        let mut positions = PreparedPositions::new(4)?;
        positions.push(0, &work)?;
        check!(matches!(
            positions.push(99, &work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(positions.as_slice(), &[0]);
        check_eq!(memory.live.load(Ordering::Relaxed), 4);
        drop(positions);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        let (work, memory) = funded_token_budget(24)?;
        let mut positions = PreparedPositions::new(4)?;
        for position in [0, 9, 10, u32::MAX] {
            positions.push(position, &work)?;
        }
        check_eq!(positions.as_slice(), &[0, 9, 10, 4_294_967_295]);
        check!(matches!(
            positions.push(42, &work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(memory.live.load(Ordering::Relaxed), 16);
        check_eq!(memory.peak.load(Ordering::Relaxed), 24);
        drop(positions);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn funded_occurrence_json_preserves_wire_and_batch_owns_payload_until_drop()
    -> Result<(), IndexError> {
        let (mut work, memory) = funded_token_budget(65_536)?;
        let scratch = TestScratch::default();
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut edits = InventoryEdits::new(0)?;
        let mut writes = OccurrenceWrites::new();
        let baseline = memory.live.load(Ordering::Relaxed);
        let mut positions = PreparedPositions::new(4)?;
        for position in [0, 9, u32::MAX] {
            positions.push(position, &work)?;
        }
        let payload = super::super::text_memory::encode(&positions.occurrence, &work)?;
        let gold = br#"{"positions":[0,9,4294967295]}"#;
        check_eq!(payload.as_slice(), gold);
        drop(positions);
        let expected = baseline
            .checked_add(u64::try_from(gold.len()).map_err(|_error| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(memory.live.load(Ordering::Relaxed), expected);
        writes.push(
            OccurrenceEdit {
                ordinal: 0,
                key: PreparedKey::copy(b"\0a", &work)?,
                prior: None,
                identified: reference(90),
            },
            payload,
            &spool,
            &mut edits,
            &mut work,
        )?;
        let row_backing = u64::try_from(8 * std::mem::size_of::<(InventoryPartition, MapEdit)>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        check_eq!(
            memory.live.load(Ordering::Relaxed),
            expected
                .checked_add(2)
                .ok_or(IndexError::ResourceLimited)?
                .checked_add(OccurrenceWrites::metadata_bytes(8)?)
                .and_then(|bytes| bytes.checked_add(row_backing))
                .ok_or(IndexError::ResourceLimited)?
        );
        check_eq!(
            writes.inputs.first().map(|input| input.payload.as_slice()),
            Some(gold.as_slice())
        );
        drop(edits);
        drop(writes);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    struct GrowingJson(std::cell::Cell<bool>);
    impl Serialize for GrowingJson {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(if self.0.replace(true) { "longer" } else { "x" })
        }
    }

    #[test]
    fn funded_json_refuses_census_drift_without_unfunded_growth() -> Result<(), IndexError> {
        let (work, memory) = funded_token_budget(32)?;
        check!(matches!(
            super::super::text_memory::encode(&GrowingJson(std::cell::Cell::new(false)), &work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(memory.peak.load(Ordering::Relaxed), 3);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn funded_json_preserves_original_cancellation_before_allocation() -> Result<(), IndexError> {
        let (work, memory) = funded_token_budget(32)?;
        work.request().cancel_handle().cancel();
        check!(matches!(
            super::super::text_memory::encode(&[0u32, 7], &work),
            Err(IndexError::Cancelled)
        ));
        check_eq!(memory.calls.load(Ordering::Relaxed), 0);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn native_small_children_preserve_edge_order_duplicates_and_empty_surfaces()
    -> Result<(), IndexError> {
        for (owners, terms, gold) in [
            (None, None, Vec::new()),
            (Some(reference(9)), None, vec![reference(9)]),
            (None, Some(reference(2)), vec![reference(2)]),
            (
                Some(reference(9)),
                Some(reference(2)),
                vec![reference(9), reference(2)],
            ),
            (
                Some(reference(4)),
                Some(reference(4)),
                vec![reference(4), reference(4)],
            ),
        ] {
            let root = Root {
                owners: MapRoot(owners),
                terms: MapRoot(terms),
                ..Root::default()
            };
            check_eq!(root_children(root).as_slice(), gold.as_slice());
        }
        let contribution = Contribution {
            record: reference(9),
            inventory: MapRoot(Some(reference(2))).into(),
            indexed_tokens: 3,
        };
        check_eq!(
            contribution_children(&contribution)?.as_slice(),
            &[reference(9), reference(2)]
        );
        check_eq!(
            posting_children(&Posting {
                root: MapRoot(None).into(),
                df: 0
            })?
            .as_slice(),
            &[]
        );
        check_eq!(
            posting_children(&Posting {
                root: MapRoot(Some(reference(2))).into(),
                df: 1
            })?
            .as_slice(),
            &[reference(2)]
        );
        Ok(())
    }

    #[test]
    fn native_owner_key_stack_bytes_match_fixed_protocol_gold() -> Result<(), IndexError> {
        let key = OwnerKey {
            producer: ProducerId(*b"0123456789abcdef"),
            stable_id: StableOwnerId(*b"FEDCBA9876543210"),
        };
        check_eq!(
            owner_key(key).as_slice(),
            b"0123456789abcdefFEDCBA9876543210"
        );
        Ok(())
    }

    #[test]
    fn native_root_json_is_funded_through_put_and_refusal_preserves_wire_commitment()
    -> Result<(), IndexError> {
        let gold = br#"{"owners":null,"terms":null,"docs":0,"indexed_tokens":0}"#;
        let length = u64::try_from(gold.len()).map_err(|_error| IndexError::ResourceLimited)?;
        for (ceiling, refuse) in [
            (
                length.checked_sub(1).ok_or(IndexError::ResourceLimited)?,
                false,
            ),
            (length, false),
            (length, true),
        ] {
            let mut fixture = visitor_fixture()?;
            let (mut work, memory) = funded_token_budget(ceiling)?;
            fixture.objects.json_at_install = Some(JsonInstallObservation {
                memory: Arc::clone(&memory),
                payload: gold,
                refuse,
            });
            let scratch = TestScratch::default();
            let custody = TestCustody;
            let mut context = AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            let result = write_root(Root::default(), &mut context);
            if ceiling < length {
                check!(matches!(result, Err(IndexError::ResourceLimited)));
                check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
            } else if refuse {
                check!(
                    matches!(result, Err(IndexError::BackendViolation(message)) if message == "native JSON observer refused installation")
                );
                check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 0);
            } else {
                let (artifact, commitment) = result?;
                check_eq!(artifact, reference(40));
                check_eq!(commitment, Digest(Sha256::digest(gold).into()));
                check_eq!(fixture.objects.installs.load(Ordering::Relaxed), 1);
            }
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        Ok(())
    }

    #[test]
    fn inventory_heads_share_page_key_without_preparation_copy() -> Result<(), IndexError> {
        let mut fixture = visitor_fixture()?;
        fixture.maps.posting_root = fixture.inventory_root;
        let page = MapPage::new(
            vec![(b"\0a".to_vec(), fixture.occurrence_ref.into())],
            false,
            Vec::new(),
            None,
        );
        let pointer = page
            .entries
            .first()
            .ok_or(IndexError::ResourceLimited)?
            .0
            .as_ptr();
        fixture.maps.malformed_page = Some(page);
        let (mut work, memory) = funded_token_budget(1)?;
        let mut cursor = MapEntries::new(&fixture.objects, &fixture.maps, fixture.inventory_root);
        let head = cursor.next(&mut work)?.ok_or(IndexError::ResourceLimited)?;
        check!(head.get()?.0.as_ptr() == pointer);
        check_eq!(head.key()?, b"\0a");
        check_eq!(memory.calls.load(Ordering::Relaxed), 0);
        drop(cursor);
        check!(head.get()?.0.as_ptr() == pointer);
        check_eq!(head.get()?.1, fixture.occurrence_ref.into());
        Ok(())
    }

    #[test]
    fn confirmed_inventory_edits_share_bounded_batches_with_deletions() -> Result<(), IndexError> {
        let mut observations = Vec::new();
        for scalar in [true, false] {
            let (mut work, memory) = funded_batch_budget(1_000_000)?;
            let scratch = TestScratch::default();
            let spool = TextSpool::new(&scratch, &mut work)?;
            let baseline = scratch.reserve_observations.load(Ordering::Relaxed);
            let mut edits = InventoryEdits::new(0)?;
            for id in 0..65u8 {
                let key = PreparedKey::copy(&[0, b'a', id], &work)?;
                let pointer = key.as_slice().as_ptr();
                let next = (id & 1 == 0).then_some(reference(91));
                edits.push_prepared(key, Some(reference(90)), next, &work)?;
                check!(
                    edits
                        .rows
                        .as_slice()
                        .last()
                        .is_some_and(|(_, row)| row.key.as_ptr() == pointer)
                );
                if scalar || edits.rows.len() == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
                    edits.flush(&spool, &mut work)?;
                    check!(edits.key_memory.iter().all(Option::is_none));
                }
            }
            edits.flush(&spool, &mut work)?;
            observations.push(
                scratch
                    .reserve_observations
                    .load(Ordering::Relaxed)
                    .checked_sub(baseline)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            let mut cursor = spool.inventory_cursor(1)?;
            for id in 0..65u8 {
                let row = cursor
                    .next_edit(&mut work)?
                    .ok_or(IndexError::ResourceLimited)?;
                check_eq!(row.root, 0);
                check_eq!(row.edit.key.as_slice(), [0, b'a', id]);
                check_eq!(row.edit.expected, Some(reference(90).into()));
                check_eq!(row.edit.next, (id & 1 == 0).then_some(reference(91).into()));
            }
            check!(cursor.next_edit(&mut work)?.is_none());
            check!(memory.retained_calls.load(Ordering::Relaxed) > 0);
            drop(cursor);
            drop(edits);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        let [scalar, batched] = observations.as_slice() else {
            return Err(IndexError::BackendViolation(
                "inventory batch controls missing".into(),
            ));
        };
        check!(*batched < *scalar, "{observations:?}");
        Ok(())
    }

    #[test]
    fn occurrence_packet_installs_identical_leaves_once_and_expands_original_edits()
    -> Result<(), IndexError> {
        #[derive(Serialize)]
        struct FixedOccurrence {
            positions: [u32; 1],
        }
        // Nonadjacent aliases must preserve three term edits while installing
        // two leaves. A conflicting input or bad later output admits no hit.
        'cases: for fault in 0..3 {
            let mut fixture = visitor_fixture()?;
            fixture.objects.install_outputs = Some(vec![
                reference(90),
                reference(if fault == 2 { 99 } else { 91 }),
            ]);
            let scratch = TestScratch::default();
            let custody = TestCustody;
            let mut work = budget()?;
            let spool = TextSpool::new(&scratch, &mut work)?;
            let mut confirmed = ConfirmedOccurrences::new(2)?;
            let mut edits = InventoryEdits::new(0)?;
            let mut writes = OccurrenceWrites::new();
            for (key, reference, position) in [
                ([0, b'a'], reference(90), 0),
                ([0, b'b'], reference(91), 1),
                ([1, b'a'], reference(90), if fault == 1 { 2 } else { 0 }),
            ] {
                let result = writes.push(
                    OccurrenceEdit {
                        ordinal: 0,
                        key: PreparedKey::copy(&key, &work)?,
                        prior: None,
                        identified: reference,
                    },
                    super::super::text_memory::encode(
                        &FixedOccurrence {
                            positions: [position],
                        },
                        &work,
                    )?,
                    &spool,
                    &mut edits,
                    &mut work,
                );
                if fault == 1 && key == [1, b'a'] {
                    check!(matches!(result, Err(IndexError::BackendViolation(_))));
                    check!(confirmed.references.is_empty());
                    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
                    edits.flush(&spool, &mut work)?;
                    check!(spool.inventory_cursor(1)?.next_edit(&mut work).is_err());
                    continue 'cases;
                }
                result?;
            }
            let mut context = AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            let result = writes.flush(&mut context, &spool, &mut confirmed, &mut edits);
            if fault != 0 {
                check!(matches!(result, Err(IndexError::BackendViolation(_))));
                check!(confirmed.references.is_empty());
                check_eq!(writes.pending.len(), 2);
                check_eq!(
                    fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
                    2
                );
                check!(
                    spool
                        .inventory_cursor(1)?
                        .next_edit(context.budget)
                        .is_err()
                );
                continue;
            }
            result?;
            check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 1);
            check_eq!(
                fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
                2
            );
            check_eq!(confirmed.references.len(), 2);
            edits.flush(&spool, context.budget)?;
            let mut cursor = spool.inventory_cursor(1)?;
            for (key, reference) in [
                ([0, b'a'], reference(90)),
                ([0, b'b'], reference(91)),
                ([1, b'a'], reference(90)),
            ] {
                let row = cursor
                    .next_edit(context.budget)?
                    .ok_or(IndexError::ResourceLimited)?;
                check_eq!(row.root, 0);
                check_eq!(row.edit.key.as_slice(), key);
                check_eq!(row.edit.expected, None);
                check_eq!(row.edit.next, Some(reference.into()));
            }
            check!(cursor.next_edit(context.budget)?.is_none());
        }
        Ok(())
    }

    #[test]
    fn confirmed_occurrences_from_32_owners_share_one_inventory_packet() -> Result<(), IndexError> {
        #[derive(Serialize)]
        struct FixedOccurrence {
            positions: [u32; 1],
        }
        let mut fixture = visitor_fixture()?;
        fixture.objects.install_outputs = Some(vec![reference(90), reference(91)]);
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut work = packet_budget()?;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut confirmed = ConfirmedOccurrences::new(0)?;
        let mut inventory_edits = InventoryEdits::new(0)?;
        let mut context = AttemptContext {
            attempt: test_attempt(fixture.ticket.view.identity.workspace),
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let mut writes = OccurrenceWrites::new();
        for ordinal in 0..32_u32 {
            inventory_edits.ordinal = ordinal;
            for (surface, reference) in [(0_u8, reference(90)), (1_u8, reference(91))] {
                writes.push(
                    OccurrenceEdit {
                        ordinal,
                        key: PreparedKey::copy(&[surface, b'a'], context.budget)?,
                        prior: None,
                        identified: reference,
                    },
                    super::super::text_memory::encode(
                        &FixedOccurrence { positions: [0] },
                        context.budget,
                    )?,
                    &spool,
                    &mut inventory_edits,
                    context.budget,
                )?;
            }
            if ordinal == 30 {
                check_eq!(inventory_edits.rows.len(), 62);
                check!(
                    spool
                        .inventory_cursor(32)?
                        .next_edit(context.budget)?
                        .is_none()
                );
            }
        }
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        writes.flush(&mut context, &spool, &mut confirmed, &mut inventory_edits)?;
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 1);
        check_eq!(
            fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
            2
        );
        check_eq!(inventory_edits.rows.len(), 0);
        let mut cursor = spool.inventory_cursor(32)?;
        for ordinal in 0..32_u32 {
            for (surface, reference) in [(0_u8, reference(90)), (1_u8, reference(91))] {
                let row = cursor
                    .next_edit(context.budget)?
                    .ok_or(IndexError::ResourceLimited)?;
                check_eq!(row.root, ordinal);
                check_eq!(row.edit.key.as_slice(), &[surface, b'a']);
                check_eq!(row.edit.expected, None);
                check_eq!(row.edit.next, Some(reference.into()));
            }
        }
        check!(cursor.next_edit(context.budget)?.is_none());
        Ok(())
    }

    #[test]
    fn confirmed_inventory_key_transfer_refuses_before_sql_and_preserves_queued_rows()
    -> Result<(), IndexError> {
        let (work, memory) = funded_token_budget(3)?;
        let key = PreparedKey::copy(b"\0ab", &work)?;
        let mut edits = InventoryEdits::new(0)?;
        check!(matches!(
            edits.push_prepared(key, None, Some(reference(91)), &work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(edits.rows.len(), 0);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        drop(edits);
        check!(!memory.invalid_release.load(Ordering::Relaxed));

        let (mut work, memory) = funded_token_budget(1_000_000)?;
        let scratch = TestScratch::default();
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut edits = InventoryEdits::new(0)?;
        edits.push_prepared(
            PreparedKey::copy(b"\0a", &work)?,
            None,
            Some(reference(91)),
            &work,
        )?;
        check!(matches!(
            edits.push_prepared(
                PreparedKey::copy(b"\0b", &work)?,
                Some(reference(90)),
                Some(reference(90)),
                &work
            ),
            Err(IndexError::BackendViolation(_))
        ));
        check_eq!(edits.rows.len(), 1);
        let baseline = scratch.reserve_observations.load(Ordering::Relaxed);
        work.request().cancel_handle().cancel();
        check!(matches!(
            edits.flush(&spool, &mut work),
            Err(IndexError::Cancelled)
        ));
        check_eq!(
            scratch.reserve_observations.load(Ordering::Relaxed),
            baseline
        );
        check_eq!(edits.rows.len(), 1);
        // Independent inspection cannot renew the cancelled write authority.
        check!(
            spool
                .inventory_cursor(1)?
                .next_edit(&mut budget()?)?
                .is_none()
        );
        drop(edits);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn inventory_delete_row_growth_keeps_old_and_new_custody_and_prior_data()
    -> Result<(), IndexError> {
        let row = u64::try_from(std::mem::size_of::<(InventoryPartition, MapEdit)>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let small = row.checked_mul(8).ok_or(IndexError::ResourceLimited)?;
        let large = row.checked_mul(16).ok_or(IndexError::ResourceLimited)?;
        let peak = small
            .checked_add(large)
            .and_then(|bytes| bytes.checked_add(18))
            .ok_or(IndexError::ResourceLimited)?;
        for ceiling in [
            peak.checked_sub(1).ok_or(IndexError::ResourceLimited)?,
            peak,
        ] {
            let (work, memory) = funded_token_budget(ceiling)?;
            let mut deletes = InventoryEdits::new(0)?;
            check_eq!(memory.calls.load(Ordering::Relaxed), 0);
            for key in b"abcdefgh" {
                deletes.push(&[0, *key], reference(90), &work)?;
            }
            let result = deletes.push(b"\0i", reference(90), &work);
            let (gold, expected) = if ceiling == peak {
                result?;
                check_eq!(memory.peak.load(Ordering::Relaxed), peak);
                (
                    b"abcdefghi".as_slice(),
                    large.checked_add(18).ok_or(IndexError::ResourceLimited)?,
                )
            } else {
                check!(matches!(result, Err(IndexError::ResourceLimited)));
                (
                    b"abcdefgh".as_slice(),
                    small.checked_add(16).ok_or(IndexError::ResourceLimited)?,
                )
            };
            check_eq!(deletes.rows.len(), gold.len());
            for ((root, edit), key) in deletes.rows.as_slice().iter().zip(gold) {
                check_eq!(*root, InventoryPartition::Ready(0));
                check_eq!(edit.key.as_slice(), &[0, *key]);
                check_eq!(edit.expected, Some(reference(90).into()));
                check_eq!(edit.next, None);
            }
            check_eq!(memory.live.load(Ordering::Relaxed), expected);
            drop(deletes);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        Ok(())
    }

    #[test]
    fn posting_term_inline_boundary_keeps_long_key_custody() -> Result<(), IndexError> {
        let (work, memory) = funded_token_budget(33)?;
        let inline_bytes = [7_u8; 32];
        let long_bytes = [8_u8; 33];
        let inline = PostingTermKey::copy(&inline_bytes, &work)?;
        check_eq!(inline.as_slice(), &inline_bytes);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        let long = PostingTermKey::copy(&long_bytes, &work)?;
        check_eq!(long.as_slice(), &long_bytes);
        check_eq!(memory.live.load(Ordering::Relaxed), 33);
        drop(long);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        work.request().cancel_handle().cancel();
        check!(matches!(
            PostingTermKey::copy(&inline_bytes, &work),
            Err(IndexError::Cancelled)
        ));
        Ok(())
    }

    #[test]
    fn native_delta_packets_flush_drop_keys_retain_row_backing_and_refuse_before_sql()
    -> Result<(), IndexError> {
        let (mut work, memory) = funded_token_budget(65_536)?;
        let scratch = TestScratch::default();
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut deletes = InventoryEdits::new(0)?;
        let mut postings = PostingEdits::new()?;
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        deletes.push(b"\0a", reference(90), &work)?;
        check!(matches!(
            postings.push(b"\0b", &[7; 31], None, Some(reference(91)), &work),
            Err(IndexError::BackendViolation(_))
        ));
        check_eq!(postings.rows.len(), 0);
        postings.push(b"\0b", &[7; 32], None, Some(reference(91)), &work)?;
        let metadata = u64::try_from(
            std::mem::size_of::<(InventoryPartition, MapEdit)>()
                .checked_add(std::mem::size_of::<PostingScratchRow>())
                .and_then(|row| row.checked_mul(8))
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
        check_eq!(
            memory.live.load(Ordering::Relaxed),
            metadata.checked_add(2).ok_or(IndexError::ResourceLimited)?
        );
        deletes.flush(&spool, &mut work)?;
        postings.flush(&spool, &mut work)?;
        check_eq!(memory.live.load(Ordering::Relaxed), metadata);
        let (mut read_work, read_memory) = funded_batch_budget(65_536)?;
        let terms = spool.posting_term_page(None, &mut read_work)?;
        let mut cursor = spool.posting_cursor(terms);
        let stored = cursor
            .next_edit(&mut read_work)?
            .ok_or_else(|| IndexError::BackendViolation("posting scratch row missing".into()))?;
        check_eq!(stored.root, 0);
        check_eq!(stored.edit.key, vec![7; 32]);
        check_eq!(stored.edit.expected, None);
        check_eq!(stored.edit.next, Some(reference(91).into()));
        check!(cursor.next_edit(&mut read_work)?.is_none());
        drop(stored);
        drop(cursor);
        check_eq!(read_memory.live.load(Ordering::Relaxed), 0);
        check!(!read_memory.invalid_release.load(Ordering::Relaxed));
        deletes.push(b"\0c", reference(92), &work)?;
        work.request().cancel_handle().cancel();
        check!(matches!(
            deletes.flush(&spool, &mut work),
            Err(IndexError::Cancelled)
        ));
        check_eq!(deletes.rows.len(), 1);
        check_eq!(
            deletes
                .rows
                .as_slice()
                .first()
                .map(|(_, edit)| edit.key.as_slice()),
            Some(b"\0c".as_slice())
        );
        drop(postings);
        drop(deletes);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        let (work, memory) = funded_token_budget(33)?;
        let mut postings = PostingEdits::new()?;
        check!(matches!(
            postings.push(b"\0a", &[7; 32], None, Some(reference(91)), &work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(postings.rows.len(), 0);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        drop(postings);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn term_page_iterator_keeps_each_moved_key_charged_and_denial_preserves_prior()
    -> Result<(), IndexError> {
        let metadata = u64::try_from(
            64usize
                .checked_mul(std::mem::size_of::<PreparedKey>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
        for extra in [4, 5] {
            let (work, memory) = funded_token_budget(
                metadata
                    .checked_add(extra)
                    .ok_or(IndexError::ResourceLimited)?,
            )?;
            let mut page = super::super::text_spool::TermPage::new();
            page.push(b"abc", &work)?;
            let next = page.push(b"xy", &work);
            if extra == 4 {
                check!(matches!(next, Err(IndexError::ResourceLimited)));
                check_eq!(page.len(), 1);
            } else {
                next?;
                check_eq!(page.len(), 2);
            }
            check_eq!(
                page.first().map(PreparedKey::as_slice),
                Some(b"abc".as_slice())
            );
            let mut iter = page.into_iter();
            let first = iter.next().ok_or_else(|| {
                IndexError::BackendViolation("term page lost admitted key".into())
            })?;
            check_eq!(
                memory.live.load(Ordering::Relaxed),
                metadata
                    .checked_add(if extra == 4 { 3 } else { 5 })
                    .ok_or(IndexError::ResourceLimited)?
            );
            drop(iter);
            check_eq!(first.as_slice(), b"abc");
            check_eq!(memory.live.load(Ordering::Relaxed), 3);
            drop(first);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        Ok(())
    }

    #[test]
    fn term_page_empty_and_first_array_denial_allocate_no_key() -> Result<(), IndexError> {
        let (work, memory) = funded_token_budget(1)?;
        let mut page = super::super::text_spool::TermPage::new();
        check_eq!(memory.calls.load(Ordering::Relaxed), 0);
        check!(matches!(
            page.push(b"abc", &work),
            Err(IndexError::ResourceLimited)
        ));
        check!(page.is_empty());
        check_eq!(memory.calls.load(Ordering::Relaxed), 1);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        drop(page);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn preparation_key_copy_requires_independent_capacity_and_preserves_cancellation()
    -> Result<(), IndexError> {
        let (work, memory) = funded_token_budget(5)?;
        let first = PreparedKey::copy(b"abc", &work)?;
        check!(matches!(
            PreparedKey::copy(first.as_slice(), &work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(first.as_slice(), b"abc");
        check_eq!(memory.live.load(Ordering::Relaxed), 3);
        work.request().cancel_handle().cancel();
        let calls = memory.calls.load(Ordering::Relaxed);
        check!(matches!(
            PreparedKey::copy(b"x", &work),
            Err(IndexError::Cancelled)
        ));
        check_eq!(memory.calls.load(Ordering::Relaxed), calls);
        drop(first);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn empty_occurrence_writes_allocate_nothing_and_first_denial_releases_payload()
    -> Result<(), IndexError> {
        let (mut work, memory) = funded_token_budget(3)?;
        let scratch = TestScratch::default();
        let spool = TextSpool::new(&scratch, &mut budget()?)?;
        let mut edits = InventoryEdits::new(0)?;
        let mut writes = OccurrenceWrites::new();
        check_eq!(memory.calls.load(Ordering::Relaxed), 0);
        check_eq!(writes.inputs.capacity(), 0);
        check_eq!(writes.pending.capacity(), 0);
        check_eq!(writes.payload_memory.capacity(), 0);
        let payload = super::super::text_memory::encode(&0u32, &work)?;
        check!(matches!(
            writes.push(
                OccurrenceEdit {
                    ordinal: 0,
                    key: PreparedKey::copy(b"\0a", &work)?,
                    prior: None,
                    identified: reference(90)
                },
                payload,
                &spool,
                &mut edits,
                &mut work
            ),
            Err(IndexError::ResourceLimited)
        ));
        check!(writes.inputs.is_empty());
        check_eq!(writes.inputs.capacity(), 0);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn occurrence_flush_admits_refs_and_edits_before_install_and_refunds_after_refusal()
    -> Result<(), IndexError> {
        #[derive(Serialize)]
        struct FixedOccurrence {
            positions: [u32; 1],
        }
        for case in 0..3 {
            let fixture = visitor_fixture()?;
            let (mut work, memory) = funded_batch_budget(1_000_000)?;
            let scratch = TestScratch::default();
            let custody = TestCustody;
            let spool = TextSpool::new(&scratch, &mut work)?;
            let mut writes = OccurrenceWrites::new();
            let mut inventory_edits = InventoryEdits::new(0)?;
            for key in [b"\0a", b"\x01a"] {
                let payload =
                    super::super::text_memory::encode(&FixedOccurrence { positions: [0] }, &work)?;
                check_eq!(payload.as_slice(), br#"{"positions":[0]}"#);
                writes.push(
                    OccurrenceEdit {
                        ordinal: 0,
                        key: PreparedKey::copy(key, &work)?,
                        prior: None,
                        identified: reference(90),
                    },
                    payload,
                    &spool,
                    &mut inventory_edits,
                    &mut work,
                )?;
            }
            check_eq!(writes.pending.len(), 1);
            check_eq!(writes.fanout_rows, 2);
            let denied = Arc::new(AtomicUsize::new(0));
            if case == 0 {
                work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
                    RefuseTokenMemory {
                        calls: Arc::clone(&denied),
                        requested: Arc::new(AtomicU64::new(0)),
                    },
                ))));
            } else if case == 1 {
                work.request().cancel_handle().cancel();
            }
            let result = writes.flush(
                &mut AttemptContext {
                    attempt: test_attempt(fixture.ticket.view.identity.workspace),
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    custody: &custody,
                    scratch: &scratch,
                    budget: &mut work,
                },
                &spool,
                &mut ConfirmedOccurrences::new(0)?,
                &mut inventory_edits,
            );
            match case {
                0 => {
                    check!(matches!(result, Err(IndexError::ResourceLimited)));
                    check_eq!(denied.load(Ordering::Relaxed), 1);
                }
                1 => check!(matches!(result, Err(IndexError::Cancelled))),
                _ => check!(matches!(result,
                    Err(IndexError::BackendViolation(message))
                        if message == "changed-body occurrence build reached object batch"
                )),
            }
            check_eq!(
                fixture.objects.install_batches.load(Ordering::Relaxed),
                usize::from(case == 2),
            );
            check_eq!(writes.pending.len(), 1);
            check_eq!(writes.inputs.len(), 1);
            // Staged fanout is never a successful map prefix after refusal.
            if case == 2 {
                check!(
                    spool
                        .inventory_cursor(1)?
                        .next_edit(&mut budget()?)
                        .is_err()
                );
            } else {
                check!(
                    spool
                        .inventory_cursor(1)?
                        .next_edit(&mut budget()?)?
                        .is_none()
                );
            }
            drop(inventory_edits);
            drop(writes);
            check_eq!(
                memory.live.load(Ordering::Relaxed),
                if case == 2 {
                    inventory_packet_grant_bytes()
                } else {
                    0
                }
            );
            drop(spool);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        Ok(())
    }

    #[test]
    fn occurrence_metadata_growth_admits_old_and_new_arrays_preserves_payloads_on_denial()
    -> Result<(), IndexError> {
        let small = OccurrenceWrites::metadata_bytes(8)?;
        let large = OccurrenceWrites::metadata_bytes(16)?;
        let peak = small
            .checked_add(large)
            .and_then(|sum| sum.checked_add(9))
            .ok_or(IndexError::ResourceLimited)?;
        for ceiling in [
            peak.checked_sub(1).ok_or(IndexError::ResourceLimited)?,
            peak,
        ] {
            let (work, memory) = funded_token_budget(ceiling)?;
            let mut writes = OccurrenceWrites::new();
            // Fixed one-byte payloads isolate metadata growth from SQL fanout.
            for value in 0..8u32 {
                let payload = super::super::text_memory::encode(&value, &work)?;
                writes.reserve_next(&work)?;
                let (payload, grant) = payload.into_parts();
                writes.inputs.push(ObjectBatchInput {
                    payload,
                    children: Vec::new(),
                });
                writes.pending.push(PendingOccurrence {
                    ordinal: 0,
                    identified: reference(90),
                });
                writes.payload_memory.push(grant);
            }
            check_eq!(
                memory.live.load(Ordering::Relaxed),
                small.checked_add(8).ok_or(IndexError::ResourceLimited)?
            );
            let payload = super::super::text_memory::encode(&8u32, &work)?;
            let result = writes.reserve_next(&work);
            if ceiling == peak {
                result?;
                check_eq!(writes.inputs.capacity(), 16);
                check_eq!(memory.peak.load(Ordering::Relaxed), peak);
            } else {
                check!(matches!(result, Err(IndexError::ResourceLimited)));
                check_eq!(writes.inputs.capacity(), 8);
            }
            check_eq!(writes.inputs.len(), 8);
            check_eq!(writes.pending.len(), 8);
            check_eq!(writes.payload_memory.len(), 8);
            for (input, byte) in writes.inputs.iter().zip(b"01234567") {
                check_eq!(input.payload.as_slice(), std::slice::from_ref(byte));
            }
            check_eq!(payload.as_slice(), b"8");
            drop(payload);
            drop(writes);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            check!(!memory.invalid_release.load(Ordering::Relaxed));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Root {
    owners: MapRoot,
    terms: MapRoot,
    docs: u64,
    indexed_tokens: u64,
}
impl Root {
    pub(super) fn owner_universe(self) -> (MapRoot, u64, u64) {
        (self.owners, self.docs, self.indexed_tokens)
    }
}

pub(super) fn selected_overlay_prior(
    root: Root,
    recipe: TextRecipe,
    owner: OwnerKey,
    context: &mut TextReadContext<'_>,
) -> Result<Option<(EntryMap, u64)>, IndexError> {
    let key = owner_key(owner);
    let Some(reference) = context.maps.lookup(root.owners, &key, context.budget)? else {
        let selected = context.maps.lookup_object(
            context.ticket.view.source.owners,
            &owner_map_key(owner),
            context.budget,
        )?;
        let Some(source) = selected else {
            return Ok(None);
        };
        let stored = context.objects.get(source, context.budget)?;
        let record = quanta_index_core::index::SourceRecordOwner::decode_json(
            &stored.payload,
            context.budget,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        if stored.registration.reference != source
            || stored.registration.children.as_slice() != [record.bytes]
            || record.path.is_empty()
            || record.path.len() > MAX_SOURCE_PATH_BYTES
            || record.path.starts_with('/')
            || record.path.contains('\0')
            || record
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(IndexError::CorruptObject(
                "selected overlay source record or path is invalid".into(),
            ));
        }
        if !matches!(record.text, SourceTextAdmission::Excluded) {
            return Err(IndexError::SnapshotUnavailable);
        }
        return Ok(None);
    };
    let validated = validate_universe_owner(&key, reference, root, recipe, context)?;
    let contribution = read_contribution(reference, context.objects, context.budget)?;
    if validated.owner != owner || validated.doc_indexed_tokens != contribution.indexed_tokens {
        return Err(IndexError::CorruptObject(
            "selected text overlay contribution differs from owner".into(),
        ));
    }
    Ok(Some((contribution.inventory, contribution.indexed_tokens)))
}

pub(super) fn validate_overlay_term_membership(
    root: Root,
    recipe: TextRecipe,
    owner: OwnerKey,
    key: &[u8],
    inventory_ref: Option<ObjectRef>,
    context: &mut TextReadContext<'_>,
) -> Result<bool, IndexError> {
    check_term_key(key)?;
    let posting = context
        .maps
        .lookup(root.terms, key, context.budget)?
        .map(|reference| read_posting(reference, context.objects, context.budget))
        .transpose()?;
    let owner_key = owner_key(owner);
    let posting_witness = posting
        .map(|posting| {
            posting.root.posting_lookup(
                context.objects,
                context.maps,
                &owner_key,
                posting.df,
                context.budget,
            )
        })
        .transpose()?
        .flatten();
    match (inventory_ref, posting_witness) {
        (None, None) => {}
        (Some(reference), Some(witness)) if witness.matches(reference) => {}
        _ => {
            return Err(IndexError::CorruptObject(
                "overlay owner inventory differs from selected term posting".into(),
            ));
        }
    }
    if let Some(witness) = posting_witness {
        let mut occurrences = OccurrenceReader::new(1)?;
        let hit = validate_posting_hit(
            &owner_key,
            witness,
            key,
            root,
            recipe,
            context,
            &mut occurrences,
        )?;
        if hit.owner != owner {
            return Err(IndexError::CorruptObject(
                "overlay selected posting owner changed".into(),
            ));
        }
    }
    Ok(posting_witness.is_some())
}
quanta_index_contract::native_serde_struct!(Root {
    owners: MapRoot,
    terms: MapRoot,
    docs: u64,
    indexed_tokens: u64,
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Contribution {
    record: ObjectRef,
    inventory: EntryMap,
    indexed_tokens: u64,
}
quanta_index_contract::native_serde_struct!(Contribution {
    record: ObjectRef,
    inventory: EntryMap,
    indexed_tokens: u64,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Occurrence {
    pub(super) positions: Positions,
}
quanta_index_contract::native_serde_struct!(Occurrence {
    positions: Positions,
});

/// Same sequence wire shape; enforce its leaf envelope while decoding, before
/// an attacker-controlled sequence length can reserve a corpus-sized vector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Positions(pub(super) Vec<u32>);

pub(super) struct PreparedPositions {
    occurrence: Occurrence,
    maximum: usize,
    memory: Option<quanta_index_core::index::MemoryLease>,
}

impl PreparedPositions {
    pub(super) fn new(maximum: usize) -> Result<Self, IndexError> {
        if maximum == 0
            || u64::try_from(maximum).map_err(|_error| IndexError::ResourceLimited)?
                > MAX_POSITIONS_PER_TERM
        {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            occurrence: Occurrence {
                positions: Positions(Vec::new()),
            },
            maximum,
            memory: None,
        })
    }

    pub(super) fn as_slice(&self) -> &[u32] {
        &self.occurrence.positions
    }

    pub(super) fn push(&mut self, position: u32, budget: &WorkBudget) -> Result<(), IndexError> {
        let needed = self
            .as_slice()
            .len()
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if needed > self.maximum {
            return Err(IndexError::ResourceLimited);
        }
        if needed > self.occurrence.positions.0.capacity() {
            let capacity = self
                .occurrence
                .positions
                .0
                .capacity()
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?
                .max(needed)
                .min(self.maximum);
            let bytes = capacity
                .checked_mul(std::mem::size_of::<u32>())
                .ok_or(IndexError::ResourceLimited)?;
            let memory = budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )?;
            let mut positions = Vec::new();
            positions
                .try_reserve_exact(capacity)
                .map_err(|_error| IndexError::ResourceLimited)?;
            positions.append(&mut self.occurrence.positions.0);
            self.occurrence.positions.0 = positions;
            self.memory = memory;
        } else {
            budget.checkpoint()?;
        }
        self.occurrence.positions.0.push(position);
        Ok(())
    }
}

impl std::ops::Deref for Positions {
    type Target = [u32];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Serialize for Positions {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let count = u64::try_from(self.len()).map_err(serde::ser::Error::custom)?;
        if count == 0 || count > MAX_POSITIONS_PER_TERM {
            return Err(serde::ser::Error::custom(
                "text positions exceed their leaf envelope",
            ));
        }
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Positions {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Positions;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("1..=8192 text positions")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let limit =
                    usize::try_from(MAX_POSITIONS_PER_TERM).map_err(serde::de::Error::custom)?;
                if sequence.size_hint().is_some_and(|count| count > limit) {
                    return Err(serde::de::Error::custom(
                        "text position sequence exceeds its leaf envelope",
                    ));
                }
                let mut positions = Vec::new();
                while let Some(position) = sequence.next_element::<u32>()? {
                    if positions.len() == limit {
                        return Err(serde::de::Error::custom(
                            "text position sequence exceeds its leaf envelope",
                        ));
                    }
                    positions.push(position);
                }
                if positions.is_empty() {
                    return Err(serde::de::Error::custom("text position sequence is empty"));
                }
                Ok(Positions(positions))
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Posting {
    root: EntryMap,
    df: u64,
}
quanta_index_contract::native_serde_struct!(Posting {
    root: EntryMap,
    df: u64,
});

impl Posting {
    fn checked_df(
        self,
        objects: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        if let EntryMap::PostingPages(reference) = self.root {
            let directory = dense_posting::wire::decode_directory(
                objects.get(reference, budget)?,
                reference,
                budget,
            )?;
            if directory.total() != self.df {
                return Err(IndexError::CorruptObject(
                    "packed posting directory DF differs from metadata".into(),
                ));
            }
        }
        Ok(self.df)
    }
}

#[derive(Clone, Debug)]
pub struct TextPlan {
    base: Option<MemberBinding>,
    context: PlanContext,
    input: DeltaInput<SourceDelta>,
}

#[derive(Clone, Debug)]
pub struct TextCandidate {
    pub binding: MemberBinding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextStatistics {
    /// Number of explicitly admitted owners, including empty text files.
    pub docs: u64,
    /// Indexable NFC runs counted once, regardless of the two case surfaces.
    pub indexed_tokens: u64,
}

pub struct TextReadContext<'a> {
    pub ticket: &'a TicketRecord,
    pub objects: &'a dyn ImmutableObjectPort,
    pub maps: &'a dyn PersistentMapPort,
    pub budget: &'a mut WorkBudget,
}

/// One validated posting. The position slice is valid only during the visit.
#[derive(Clone, Copy, Debug)]
pub struct TextTermHit<'a> {
    pub owner: OwnerKey,
    pub source: ObjectRef,
    pub tf: u64,
    pub doc_indexed_tokens: u64,
    pub positions: &'a [u32],
}

/// Returned only after a complete, validated posting traversal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextTermScan {
    pub docs: u64,
    pub df: u64,
    pub indexed_tokens: u64,
}

#[cfg(test)]
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, IndexError> {
    serde_json::to_vec(value).map_err(|error| IndexError::InvalidInput(error.to_string()))
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, IndexError> {
    serde_json::from_slice(bytes).map_err(|error| IndexError::CorruptObject(error.to_string()))
}

struct TextChildren(Option<std::array::IntoIter<ObjectRef, 9>>);
impl TextChildren {
    fn pair(first: Option<ObjectRef>, second: Option<ObjectRef>) -> Self {
        let (first, tail) = match (first, second) {
            (None, None) => return Self(None),
            (Some(first), second) => (first, second),
            (None, Some(second)) => (second, None),
        };
        let mut values = [first; 9];
        let len = tail.map_or(1, |second| {
            values[1] = second;
            2
        });
        let mut values = values.into_iter();
        for _slot in len..9 {
            let _unused = values.next_back();
        }
        Self(Some(values))
    }
    fn with(
        first: Option<ObjectRef>,
        refs: impl Iterator<Item = ObjectRef>,
    ) -> Result<Self, IndexError> {
        let mut refs = first.into_iter().chain(refs);
        let Some(first) = refs.next() else {
            return Ok(Self(None));
        };
        let mut values = [first; 9];
        let mut len = 1;
        for reference in refs {
            *values.get_mut(len).ok_or(IndexError::ResourceLimited)? = reference;
            len = len.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        let mut values = values.into_iter();
        for _slot in len..9 {
            let _unused = values.next_back();
        }
        Ok(Self(Some(values)))
    }
    fn as_slice(&self) -> &[ObjectRef] {
        self.0.as_ref().map_or(&[], std::array::IntoIter::as_slice)
    }
}
fn root_children(root: Root) -> TextChildren {
    TextChildren::pair(root.owners.0, root.terms.0)
}
fn contribution_children(value: &Contribution) -> Result<TextChildren, IndexError> {
    TextChildren::with(Some(value.record), value.inventory.children()?)
}
fn posting_children(value: &Posting) -> Result<TextChildren, IndexError> {
    TextChildren::with(None, value.root.children()?)
}

fn owner_key(owner: OwnerKey) -> [u8; 32] {
    let mut key = [0; 32];
    for (target, byte) in key
        .iter_mut()
        .zip(owner.producer.0.iter().chain(owner.stable_id.0.iter()))
    {
        *target = *byte;
    }
    key
}

enum ContributionInput {
    Inline(MapValue),
    Object(Option<ObjectBatchInput>),
}
struct PendingContribution {
    input: ContributionInput,
    owner: [u8; 32],
    prior: Option<MapValue>,
    // Owned input backing is destroyed before these original grants.
    _payload_memory: Option<quanta_index_core::index::MemoryLease>,
    _children_memory: Option<quanta_index_core::index::MemoryLease>,
}

/// Top-map values retain only confirmed physical children.
///
/// Values contain confirmed contribution references or bounded inline data.
/// The original attempt owns every installation; no queued reference is authority.
struct ContributionWrites {
    pending: PreparedRows<PendingContribution>,
    limit: usize,
}

#[derive(Clone, Copy)]
struct VerifiedInventory {
    bytes: ObjectRef,
    byte_len: u64,
    inventory: EntryMap,
    indexed_tokens: u64,
}

/// One prepare's confirmed inventory roots; the original attempt owns their GC
/// protection. No body data, foreign profile or persistent cache escapes here.
struct InventoryReuse {
    recipe: TextRecipe,
    entries: Option<PreparedRows<VerifiedInventory>>,
    occurrences: ConfirmedOccurrences,
    limit: usize,
}

/// Confirmed leaf identities owned by this one prepare attempt's custody sink.
/// No pending reference, body, recipe state or cross-attempt cache escapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ConfirmedOccurrence {
    reference: ObjectRef,
    // Filled only after the original object-port read and full count validation.
    validated_count: Option<u64>,
}

struct ConfirmedOccurrences {
    references: Vec<ConfirmedOccurrence>,
    blocks: OccurrenceReader,
    limit: usize,
    attempt: Option<quanta_index_core::index::AttemptRef>,
    // Backing is destroyed before its original allocation grant.
    memory: Option<quanta_index_core::index::MemoryLease>,
}

impl ConfirmedOccurrences {
    fn new(limit: usize) -> Result<Self, IndexError> {
        if limit > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            return Err(IndexError::InvalidInput(
                "occurrence reuse exceeds its bounded window".into(),
            ));
        }
        Ok(Self {
            references: Vec::new(),
            blocks: if limit == 0 {
                OccurrenceReader::uncached()?
            } else {
                OccurrenceReader::new(2)?
            },
            limit,
            attempt: None,
            memory: None,
        })
    }

    fn prepare(
        &mut self,
        attempt: quanta_index_core::index::AttemptRef,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        self.check_attempt(attempt)?;
        if self.limit == 0 || self.references.capacity() != 0 {
            return Ok(());
        }
        let bytes = self
            .limit
            .checked_mul(std::mem::size_of::<ConfirmedOccurrence>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        let mut references = Vec::new();
        references
            .try_reserve_exact(self.limit)
            .map_err(|_error| IndexError::ResourceLimited)?;
        self.references = references;
        self.memory = memory;
        self.attempt = Some(attempt);
        Ok(())
    }

    fn check_attempt(
        &self,
        attempt: quanta_index_core::index::AttemptRef,
    ) -> Result<(), IndexError> {
        if self.attempt.is_some_and(|bound| bound != attempt) {
            return Err(IndexError::FenceLost);
        }
        Ok(())
    }

    fn contains(
        &self,
        reference: ObjectRef,
        attempt: quanta_index_core::index::AttemptRef,
        budget: &WorkBudget,
    ) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        self.check_attempt(attempt)?;
        Ok(self
            .references
            .iter()
            .any(|entry| entry.reference == reference))
    }

    fn contains_or_spill(
        &self,
        reference: ObjectRef,
        attempt: quanta_index_core::index::AttemptRef,
        spool: &TextSpool,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        if self.contains(reference, attempt, budget)? {
            return Ok(true);
        }
        if self.limit == 0 {
            return Ok(false);
        }
        spool.confirmed_occurrence_contains(reference, attempt, budget)
    }

    /// Called only after `put_batch` confirms the identified immutable leaf.
    fn remember(
        &mut self,
        reference: ObjectRef,
        attempt: quanta_index_core::index::AttemptRef,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        self.check_attempt(attempt)?;
        if self.references.len() >= self.limit
            || self
                .references
                .iter()
                .any(|entry| entry.reference == reference)
        {
            return Ok(());
        }
        if self.references.capacity() < self.limit {
            return Err(IndexError::BackendViolation(
                "occurrence reuse was not admitted before installation".into(),
            ));
        }
        self.references.push(ConfirmedOccurrence {
            reference,
            validated_count: None,
        });
        Ok(())
    }

    /// Reuse only an occurrence confirmed by this attempt and fully validated
    /// once. Published/prior leaves retain the original read path.
    #[cfg(test)]
    fn validated_new_count(
        &mut self,
        key: &[u8],
        reference: ObjectRef,
        spool: &TextSpool,
        context: &mut AttemptContext<'_>,
    ) -> Result<u64, IndexError> {
        self.validated_new_count_observed(key, reference, spool, context, None, None)
    }

    fn validated_new_count_observed(
        &mut self,
        key: &[u8],
        reference: ObjectRef,
        spool: &TextSpool,
        context: &mut AttemptContext<'_>,
        packed: Option<&mut entry_map::occurrence::PackedInventoryCursor<'_>>,
        mut totals: Option<&mut OccurrenceValidationTotals>,
    ) -> Result<u64, IndexError> {
        use super::text_spool::ConfirmedOccurrenceCount;

        context.budget.checkpoint()?;
        self.check_attempt(context.attempt)?;
        check_term_key(key)?;
        if let Some(count) = self
            .blocks
            .validated_count(reference, key, context.budget)?
        {
            if let Some(totals) = totals.as_deref_mut() {
                totals.ram_reuse();
            }
            return Ok(count.value);
        }
        // The cursor already authenticated this exact packed object. A
        // scalar leaf count cannot describe its distinct term vectors.
        if self.limit != 0
            && let Some(packed) = packed
            && packed.reference() == reference
        {
            let retained = packed.count_verified();
            let result = if retained {
                packed.validated_count(key, context.budget)
            } else {
                if let Some(totals) = totals.as_deref_mut() {
                    totals.attempt(ValidationSide::Next);
                }
                let diagnostic = context.budget.request().diagnostic_context().cloned();
                observe_text_step(
                    diagnostic.as_ref(),
                    DiagnosticStageV1::NativeTextOccurrenceRead,
                    31,
                    context,
                    |context| packed.validated_count(key, context.budget),
                )
            };
            if let Some(totals) = totals.as_deref_mut() {
                if result.is_ok() && retained {
                    totals.ram_reuse();
                } else if result.is_err() && !retained {
                    totals.validation_failure();
                }
            }
            return result;
        }
        let entry = self
            .references
            .iter()
            .position(|entry| entry.reference == reference);
        if let Some(count) = entry
            .and_then(|slot| self.references.get(slot))
            .and_then(|entry| entry.validated_count)
        {
            if let Some(totals) = totals.as_deref_mut() {
                totals.ram_reuse();
            }
            return Ok(count);
        }
        // Only confirmed spill rows may carry derived DATA. A disabled or
        // unprepared memo and a missing row retain the original read path.
        let spilled = if entry.is_none() && self.limit != 0 && self.attempt == Some(context.attempt)
        {
            spool.confirmed_occurrence_count(reference, context.attempt, context.budget)?
        } else {
            ConfirmedOccurrenceCount::Missing
        };
        if let ConfirmedOccurrenceCount::Validated(count) = spilled {
            if let Some(totals) = totals.as_deref_mut() {
                totals.spill_reuse();
            }
            return Ok(count);
        }
        let count = validated_occurrence_count(
            key,
            reference,
            &mut self.blocks,
            context,
            totals,
            ValidationSide::Next,
        )?;
        if !count.leaf {
            // A retained block window cannot cross preparation attempts.
            // Zero selects the original read on every count and retains no memo.
            if self.limit != 0 {
                self.attempt = Some(context.attempt);
            }
            return Ok(count.value);
        }
        let count = count.value;
        if let Some(slot) = entry {
            self.references
                .get_mut(slot)
                .ok_or_else(|| {
                    IndexError::BackendViolation("confirmed occurrence slot absent".into())
                })?
                .validated_count = Some(count);
        } else if matches!(spilled, ConfirmedOccurrenceCount::Unvalidated) {
            // A storage/admission failure aborts this attempt. Never hide it
            // behind an expensive-read fallback or reuse the failed spool.
            spool.record_validated_occurrence_count(
                reference,
                context.attempt,
                count,
                context.budget,
            )?;
        }
        Ok(count)
    }
}

impl InventoryReuse {
    fn new(recipe: TextRecipe, limit: usize) -> Result<Self, IndexError> {
        if limit > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            return Err(IndexError::InvalidInput(
                "native text inventory reuse exceeds its bounded window".into(),
            ));
        }
        Ok(Self {
            recipe,
            entries: if limit == 0 {
                None
            } else {
                Some(PreparedRows::new(limit)?)
            },
            occurrences: ConfirmedOccurrences::new(limit)?,
            limit,
        })
    }

    fn lookup(
        &self,
        record: &SourceRecord,
        budget: &WorkBudget,
    ) -> Result<Option<(EntryMap, u64)>, IndexError> {
        budget.checkpoint()?;
        if !check_admission(record, self.recipe)? {
            return Ok(None);
        }
        if record.byte_len > MAX_SOURCE_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        if let Some(entries) = &self.entries {
            for entry in entries.as_slice() {
                budget.checkpoint()?;
                if entry.bytes == record.bytes && entry.byte_len == record.byte_len {
                    return Ok(Some((entry.inventory, entry.indexed_tokens)));
                }
            }
        }
        Ok(None)
    }

    #[cfg(test)]
    fn remember(
        &mut self,
        record: &SourceRecord,
        inventory: &EntryMap,
        indexed_tokens: u64,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if !check_admission(record, self.recipe)? || record.byte_len > MAX_SOURCE_BYTES {
            return Err(IndexError::InvalidInput(
                "inventory reuse requires a bounded admitted source".into(),
            ));
        }
        self.remember_verified(
            &VerifiedInventory {
                bytes: record.bytes,
                byte_len: record.byte_len,
                inventory: *inventory,
                indexed_tokens,
            },
            budget,
        )
    }

    fn remember_verified(
        &mut self,
        value: &VerifiedInventory,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if let Some(entries) = &mut self.entries
            && entries.len() < self.limit
        {
            entries.push(*value, budget)?;
        }
        Ok(())
    }
}

impl ContributionWrites {
    fn new() -> Result<Self, IndexError> {
        Self::with_limit(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS)
    }

    fn cold() -> Result<Self, IndexError> {
        Self::with_limit(quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH)
    }

    fn with_limit(limit: usize) -> Result<Self, IndexError> {
        Ok(Self {
            pending: PreparedRows::new(limit)?,
            limit,
        })
    }
    fn push(
        &mut self,
        owner: [u8; 32],
        prior: Option<MapValue>,
        value: &Contribution,
        context: &mut AttemptContext<'_>,
        spool: &TextSpool,
    ) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        let pending = if let Some(value) = contribution::inline(value, context.budget)? {
            PendingContribution {
                input: ContributionInput::Inline(value),
                owner,
                prior,
                _payload_memory: None,
                _children_memory: None,
            }
        } else {
            let children = contribution_children(value)?;
            let bytes = children
                .as_slice()
                .len()
                .checked_mul(std::mem::size_of::<ObjectRef>())
                .ok_or(IndexError::ResourceLimited)?;
            let children_memory = context.budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )?;
            let mut owned_children = Vec::new();
            owned_children
                .try_reserve_exact(children.as_slice().len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            owned_children.extend_from_slice(children.as_slice());
            let payload = super::text_memory::encode(value, context.budget)?;
            let (payload, payload_memory) = payload.into_parts();
            PendingContribution {
                input: ContributionInput::Object(Some(ObjectBatchInput {
                    payload,
                    children: owned_children,
                })),
                owner,
                prior,
                _payload_memory: payload_memory,
                _children_memory: children_memory,
            }
        };
        self.pending.push(pending, context.budget)?;
        if self.pending.len() == self.limit {
            self.flush(context, spool)?;
        }
        Ok(())
    }
    fn flush(
        &mut self,
        context: &mut AttemptContext<'_>,
        spool: &TextSpool,
    ) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        if self.pending.len() == 0 {
            return Ok(());
        }
        if self
            .pending
            .as_slice()
            .iter()
            .any(|row| matches!(row.input, ContributionInput::Object(None)))
        {
            return Err(IndexError::BackendViolation(
                "native text contribution packet already consumed".into(),
            ));
        }
        let count = self
            .pending
            .as_slice()
            .iter()
            .filter(|row| matches!(row.input, ContributionInput::Object(_)))
            .count();
        let bytes = count
            .checked_mul(std::mem::size_of::<ObjectBatchInput>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = if bytes == 0 {
            None
        } else {
            context.budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )?
        };
        let mut inputs = Vec::new();
        inputs
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        for row in self.pending.as_mut_slice() {
            if let ContributionInput::Object(input) = &mut row.input {
                inputs.push(input.take().ok_or_else(|| {
                    IndexError::BackendViolation("contribution input missing".into())
                })?);
            }
        }
        let references = if inputs.is_empty() {
            None
        } else if let [input] = inputs.as_slice() {
            let reference_memory = context.budget.reserve_memory(
                u64::try_from(std::mem::size_of::<ObjectRef>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                quanta_index_core::index::MemoryLifetime::Retained,
            )?;
            let mut references = Vec::new();
            references
                .try_reserve_exact(1)
                .map_err(|_error| IndexError::ResourceLimited)?;
            references.push(context.put(&input.payload, &input.children)?);
            Some(
                quanta_index_core::index::StagedObjectReferences::from_parts(
                    references,
                    reference_memory,
                )?,
            )
        } else {
            Some(context.put_batch(&inputs)?)
        };
        if references.as_ref().map_or(0, |refs| refs.len()) != count {
            return Err(IndexError::BackendViolation(
                "contribution batch lost ordinal output".into(),
            ));
        }
        let mut reference_iter = references.as_ref().map(|refs| refs.iter());
        let sql_limit = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS;
        for page in self.pending.as_slice().chunks(sql_limit) {
            let mut key_memory: [Option<quanta_index_core::index::MemoryLease>;
                quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS] = std::array::from_fn(|_| None);
            let mut edits = PreparedRows::new(sql_limit)?;
            for (row, key_grant) in page.iter().zip(&mut key_memory) {
                let next = match row.input {
                    ContributionInput::Inline(value) => value,
                    ContributionInput::Object(_) => (*reference_iter
                        .as_mut()
                        .and_then(Iterator::next)
                        .ok_or_else(|| {
                            IndexError::BackendViolation("contribution ordinal missing".into())
                        })?)
                    .into(),
                };
                if row.prior != Some(next) {
                    let key = PreparedKey::copy(&row.owner, context.budget)?;
                    let (key, grant) = key.into_parts();
                    *key_grant = grant;
                    edits.push(
                        (
                            1,
                            MapEdit {
                                key,
                                expected: row.prior,
                                next: Some(next),
                            },
                        ),
                        context.budget,
                    )?;
                }
            }
            if edits.len() != 0 {
                spool.top_batch(edits.as_slice(), context.budget)?;
            }
        }
        drop(references);
        drop(inputs);
        self.pending.clear();
        drop(memory);
        Ok(())
    }
}

fn write_root(
    root: Root,
    context: &mut AttemptContext<'_>,
) -> Result<(ObjectRef, Digest), IndexError> {
    let payload = super::text_memory::encode(&root, context.budget)?;
    let artifact = context.put(payload.as_slice(), root_children(root).as_slice())?;
    let commitment = Digest(Sha256::digest(payload.as_slice()).into());
    Ok((artifact, commitment))
}

fn decode_posting_owner(key: &[u8]) -> Result<OwnerKey, IndexError> {
    let raw: &[u8; 32] = key.try_into().map_err(|_error| {
        IndexError::CorruptObject("native text posting owner key width".into())
    })?;
    let producer = raw
        .get(..16)
        .ok_or_else(|| IndexError::CorruptObject("native text producer key".into()))?;
    let stable = raw
        .get(16..)
        .ok_or_else(|| IndexError::CorruptObject("native text stable owner key".into()))?;
    let mut producer_id = [0u8; 16];
    producer_id.copy_from_slice(producer);
    let mut stable_id = [0u8; 16];
    stable_id.copy_from_slice(stable);
    Ok(OwnerKey {
        producer: ProducerId(producer_id),
        stable_id: StableOwnerId(stable_id),
    })
}

pub(super) struct TokenRow {
    start: usize,
    end: usize,
    position: u32,
}

impl TokenRow {
    pub(super) const fn position(&self) -> u32 {
        self.position
    }
}

#[derive(Clone, Copy, Default)]
struct SourceRun {
    start: usize,
    end: usize,
    position: u32,
}

impl SourceRun {
    fn text(self, normalized: &str) -> Result<&str, IndexError> {
        normalized.get(self.start..self.end).ok_or_else(|| {
            IndexError::BackendViolation("native token source range is invalid".into())
        })
    }
}

pub(super) struct TokenBatch {
    keys: Vec<u8>,
    rows: Vec<TokenRow>,
    // Both data allocations drop before their grants, including error/unwind.
    key_memory: Option<quanta_index_core::index::MemoryLease>,
    _row_memory: Option<quanta_index_core::index::MemoryLease>,
}

impl TokenBatch {
    fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let bytes = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
            .checked_mul(std::mem::size_of::<TokenRow>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            keys: Vec::new(),
            rows,
            key_memory: None,
            _row_memory: memory,
        })
    }

    pub(super) fn rows(&self) -> &[TokenRow] {
        &self.rows
    }

    pub(super) fn term(&self, row: &TokenRow) -> Result<&[u8], IndexError> {
        self.keys.get(row.start..row.end).ok_or_else(|| {
            IndexError::BackendViolation("native token arena range is invalid".into())
        })
    }

    fn clear(&mut self) {
        self.rows.clear();
        self.keys.clear();
    }

    fn reserve_keys(&mut self, needed: usize, budget: &WorkBudget) -> Result<(), IndexError> {
        if needed <= self.keys.capacity() {
            return budget.checkpoint();
        }
        // The previous arena and its grant stay live through admission and
        // allocation. Exact census avoids worst-case overreservation.
        let memory = budget.reserve_memory(
            u64::try_from(needed).map_err(|_error| IndexError::ResourceLimited)?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        let mut keys = Vec::new();
        keys.try_reserve_exact(needed)
            .map_err(|_error| IndexError::ResourceLimited)?;
        // No old key bytes are needed after successful replacement admission.
        self.keys = keys;
        self.key_memory = memory;
        Ok(())
    }

    fn prepare(
        &mut self,
        normalized: &str,
        runs: &[SourceRun],
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let row_count = runs
            .len()
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?;
        if row_count == 0 || row_count > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            return Err(IndexError::ResourceLimited);
        }
        let maximum =
            usize::try_from(MAX_TERM_KEY_BYTES).map_err(|_error| IndexError::ResourceLimited)?;
        let mut lengths = [0usize; quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];
        let mut slots = lengths.iter_mut();
        let mut needed = 0usize;
        for run in runs {
            budget.checkpoint()?;
            let text = run.text(normalized)?;
            if text.is_empty() {
                return Err(IndexError::ResourceLimited);
            }
            let ascii = text.is_ascii();
            for case in [CaseMode::Sensitive, CaseMode::Folded] {
                let length = if ascii {
                    text.len()
                        .checked_add(1)
                        .filter(|length| *length <= maximum)
                        .ok_or(IndexError::ResourceLimited)?
                } else {
                    case_chars(text, case).try_fold(1usize, |bytes, ch| {
                        bytes
                            .checked_add(ch.len_utf8())
                            .filter(|next| *next <= maximum)
                            .ok_or(IndexError::ResourceLimited)
                    })?
                };
                *slots.next().ok_or(IndexError::ResourceLimited)? = length;
                needed = needed
                    .checked_add(length)
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        self.reserve_keys(needed, budget)?;
        self.clear();
        let mut lengths = lengths.into_iter();
        for run in runs {
            budget.checkpoint()?;
            let text = run.text(normalized)?;
            let ascii = text.is_ascii();
            for case in [CaseMode::Sensitive, CaseMode::Folded] {
                let start = self.keys.len();
                let end = start
                    .checked_add(lengths.next().ok_or(IndexError::ResourceLimited)?)
                    .ok_or(IndexError::ResourceLimited)?;
                self.keys.push(match case {
                    CaseMode::Sensitive => 0,
                    CaseMode::Folded => 1,
                });
                if ascii {
                    match case {
                        CaseMode::Sensitive => self.keys.extend_from_slice(text.as_bytes()),
                        CaseMode::Folded => self
                            .keys
                            .extend(text.bytes().map(|byte| byte.to_ascii_lowercase())),
                    }
                } else {
                    let mut encoded = [0; 4];
                    for ch in case_chars(text, case) {
                        self.keys
                            .extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
                    }
                }
                if self.keys.len() != end {
                    return Err(IndexError::BackendViolation(
                        "native token key census changed".into(),
                    ));
                }
                self.rows.push(TokenRow {
                    start,
                    end,
                    position: run.position,
                });
            }
        }
        if self.keys.len() != needed || self.rows.len() != row_count {
            return Err(IndexError::BackendViolation(
                "native token packet census changed".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
fn stored_term_key(term: &str, case: CaseMode) -> Result<Vec<u8>, IndexError> {
    let surface = match case {
        CaseMode::Sensitive => 0,
        CaseMode::Folded => 1,
    };
    let length = term
        .len()
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    if term.is_empty()
        || u64::try_from(length).map_err(|_error| IndexError::ResourceLimited)? > MAX_TERM_KEY_BYTES
    {
        return Err(IndexError::ResourceLimited);
    }
    let mut key = Vec::with_capacity(length);
    key.push(surface);
    key.extend_from_slice(term.as_bytes());
    Ok(key)
}

pub(super) fn admitted_query_tokens<'a>(
    term: &'a str,
    budget: &mut WorkBudget,
) -> Result<TokenCursor<'a, Option<quanta_index_core::index::MemoryLease>>, IndexError> {
    budget.checkpoint()?;
    if term.len() > MAX_TEXT_INSPECT_TERM_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    budget.charge(WorkUsage {
        bytes: u64::try_from(term.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(4)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    TokenCursor::try_new_bounded(
        term,
        usize::try_from(MAX_NFC_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
        usize::try_from(MAX_NFC_WORK_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
        |_units| budget.checkpoint(),
        |bytes| {
            budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )
        },
    )
    .map_err(|error| {
        error
            .into_checkpoint()
            .map_or(IndexError::ResourceLimited, std::convert::identity)
    })
}

pub(super) fn admitted_query_term(
    term: &str,
    case: CaseMode,
    budget: &mut WorkBudget,
) -> Result<PreparedKey, IndexError> {
    let mut tokens = admitted_query_tokens(term, budget)?;
    let mut count = 0usize;
    let mut first_span = None;
    while let Some(run) = tokens.next_run() {
        budget.checkpoint()?;
        if !run.is_indexable() {
            return Err(IndexError::InvalidInput(
                TextQueryError::TokenTooLong {
                    bytes: run.end.saturating_sub(run.start),
                    max: quanta_index_lq_text_normalizer::MAX_TOKEN_BYTES,
                }
                .to_string(),
            ));
        }
        count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        if first_span.is_none() {
            first_span = Some(run.start..run.end);
        }
    }
    let Some(span) = first_span else {
        return Err(IndexError::InvalidInput(
            TextQueryError::NoTokens.to_string(),
        ));
    };
    if count != 1 {
        return Err(IndexError::InvalidInput(
            "native text term requires exactly one token".into(),
        ));
    }
    let token = tokens.text().get(span).ok_or_else(|| {
        IndexError::BackendViolation("query token span exceeds normalized text".into())
    })?;
    PreparedKey::from_case_term(token, case, budget)
}

fn check_term_key(key: &[u8]) -> Result<(), IndexError> {
    super::text_spool::check_term_key(key)
}

fn check_admission(record: &SourceRecord, recipe: TextRecipe) -> Result<bool, IndexError> {
    match record.text {
        SourceTextAdmission::Excluded => Ok(false),
        SourceTextAdmission::Utf8 { policy } if policy == recipe.admission_policy => Ok(true),
        SourceTextAdmission::Utf8 { .. } => Err(IndexError::InvalidInput(
            "source text admission policy differs from native text recipe".into(),
        )),
    }
}

fn read_root(
    member: &MemberBinding,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<Root, IndexError> {
    let stored = objects.get(member.artifact, budget)?;
    let root = super::metadata::decode_fixed::<Root>(&stored.payload, budget)?;
    if stored.registration.reference != member.artifact
        || stored.registration.children.as_slice() != root_children(root).as_slice()
        || Digest(Sha256::digest(&stored.payload).into()) != member.logical_commitment
    {
        return Err(IndexError::CorruptObject(
            "native text root commitment or edges mismatch".into(),
        ));
    }
    Ok(root)
}

fn read_contribution(
    value: impl Into<MapValue>,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<Contribution, IndexError> {
    contribution::read(value.into(), objects, budget)
}

fn read_posting(
    stored_value: impl Into<quanta_index_core::index::MapValue>,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<Posting, IndexError> {
    budget.checkpoint()?;
    let value = match stored_value.into() {
        quanta_index_core::index::MapValue::Object(reference) => {
            let stored = objects.get(reference, budget)?;
            let value = super::metadata::decode_fixed::<Posting>(&stored.payload, budget)?;
            if stored.registration.reference != reference
                || stored.registration.children.as_slice() != posting_children(&value)?.as_slice()
            {
                return Err(IndexError::CorruptObject(
                    "posting object identity/edges differ".into(),
                ));
            }
            value
        }
        quanta_index_core::index::MapValue::Inline(inline) => {
            let child = inline
                .single_child()?
                .ok_or_else(|| IndexError::CorruptObject("inline posting lacks child".into()))?;
            let data = inline.data()?;
            if data.len() == 32 {
                let _owner = decode_posting_owner(data)?;
                let mut small = entry_map::SmallMap::default();
                if !small.push(data, child)? {
                    return Err(IndexError::CorruptObject(
                        "singleton posting exceeds bound".into(),
                    ));
                }
                Posting {
                    root: EntryMap::inline(&small),
                    df: 1,
                }
            } else if data.len() == 8 {
                Posting {
                    root: MapRoot(Some(child)).into(),
                    df: u64::from_be_bytes(
                        data.try_into().map_err(|_error| {
                            IndexError::CorruptObject("posting DF width".into())
                        })?,
                    ),
                }
            } else if let Some((&DENSE_POSTING_TAG, encoded_df)) = data.split_first() {
                Posting {
                    root: EntryMap::PostingPages(child),
                    df: u64::from_be_bytes(encoded_df.try_into().map_err(|_error| {
                        IndexError::CorruptObject("packed posting DF width".into())
                    })?),
                }
            } else {
                return Err(IndexError::CorruptObject(
                    "inline posting shape differs".into(),
                ));
            }
        }
    };
    value.root.validate_for(entry_map::EntryMapUse::Posting)?;
    if matches!(value.root, EntryMap::PostingPages(_))
        && !(dense_posting::wire::MIN_ROWS..=dense_posting::wire::MAX_ROWS).contains(&value.df)
    {
        return Err(IndexError::CorruptObject(
            "packed posting DF exceeds representation bounds".into(),
        ));
    }
    if value.df == 0 || value.root.inline_map().is_some_and(|map| map.len() == 0) {
        return Err(IndexError::CorruptObject(
            "native text posting metadata or edges mismatch".into(),
        ));
    }
    if let Some(small) = value.root.inline_map() {
        if u64::try_from(small.len()).map_err(|_error| IndexError::ResourceLimited)? != value.df {
            return Err(IndexError::CorruptObject(
                "inline posting DF differs from membership".into(),
            ));
        }
        for slot in 0..small.len() {
            let (key, _) = small
                .entry(slot)?
                .ok_or_else(|| IndexError::CorruptObject("missing posting entry".into()))?;
            let _owner = decode_posting_owner(key)?;
        }
    }
    Ok(value)
}

pub(super) struct ValidatedPostingHit {
    pub(super) owner: OwnerKey,
    pub(super) source_record: ObjectRef,
    pub(super) doc_indexed_tokens: u64,
    pub(super) occurrence: super::text_occurrence::ScopedOccurrence,
}

pub(super) struct ValidatedUniverseOwner {
    pub(super) owner: OwnerKey,
    pub(super) source_record: ObjectRef,
    pub(super) doc_indexed_tokens: u64,
}

/// Validate one admitted member owner under the same selected source ticket.
///
/// The broader source owner map includes excluded documents and is not a
/// candidate universe. No source text or producer IR is reconstructed here.
pub(super) fn validate_universe_owner(
    key: &[u8],
    contribution_ref: impl Into<MapValue>,
    root: Root,
    recipe: TextRecipe,
    context: &mut TextReadContext<'_>,
) -> Result<ValidatedUniverseOwner, IndexError> {
    context.budget.charge(WorkUsage {
        reads: 1,
        bytes: 32,
        ..WorkUsage::default()
    })?;
    let owner = decode_posting_owner(key)?;
    let contribution = read_contribution(contribution_ref, context.objects, context.budget)?;
    if root.docs == 0 || contribution.indexed_tokens > root.indexed_tokens {
        return Err(IndexError::CorruptObject(
            "text universe contribution exceeds root statistics".into(),
        ));
    }
    let selected = context.maps.lookup_object(
        context.ticket.view.source.owners,
        &owner_map_key(owner),
        context.budget,
    )?;
    if selected != Some(contribution.record) {
        return Err(IndexError::CorruptObject(
            "text universe contribution is stale against pinned source".into(),
        ));
    }
    let stored = context.objects.get(contribution.record, context.budget)?;
    let record = quanta_index_core::index::SourceRecordOwner::decode_json(
        &stored.payload,
        context.budget,
        quanta_index_core::index::MemoryLifetime::Temporary,
    )?;
    if stored.registration.reference != contribution.record
        || stored.registration.children.as_slice() != [record.bytes]
        || record.byte_len > MAX_SOURCE_BYTES
        || record.path.is_empty()
        || record.path.len() > MAX_SOURCE_PATH_BYTES
        || record.path.starts_with('/')
        || record.path.contains('\0')
        || record
            .path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !matches!(record.text, SourceTextAdmission::Utf8 { policy } if policy == recipe.admission_policy)
    {
        return Err(IndexError::CorruptObject(
            "text universe source admission, path or edges mismatch".into(),
        ));
    }
    context.budget.checkpoint()?;
    Ok(ValidatedUniverseOwner {
        owner,
        source_record: contribution.record,
        doc_indexed_tokens: contribution.indexed_tokens,
    })
}

fn validate_posting_hit(
    key: &[u8],
    witness: OccurrenceDigestWitness,
    term_key: &[u8],
    root: Root,
    recipe: TextRecipe,
    context: &mut TextReadContext<'_>,
    occurrences: &mut OccurrenceReader,
) -> Result<ValidatedPostingHit, IndexError> {
    let owner = decode_posting_owner(key)?;
    let contribution_ref = context.maps.lookup(root.owners, key, context.budget)?;
    validate_posting_hit_with_refs(
        witness,
        term_key,
        &PostingHitBinding {
            owner,
            recipe,
            contribution_ref,
            source: PostingSourceBinding::Lookup,
        },
        context,
        occurrences,
    )
}

#[derive(Clone, Copy)]
struct PostingHitBinding {
    owner: OwnerKey,
    recipe: TextRecipe,
    contribution_ref: Option<MapValue>,
    source: PostingSourceBinding,
}

#[derive(Clone, Copy)]
enum PostingSourceBinding {
    Lookup,
    Prefetched(Option<MapValue>),
}

fn validate_posting_hit_with_refs(
    witness: OccurrenceDigestWitness,
    term_key: &[u8],
    binding: &PostingHitBinding,
    context: &mut TextReadContext<'_>,
    occurrences: &mut OccurrenceReader,
) -> Result<ValidatedPostingHit, IndexError> {
    let PostingHitBinding {
        owner,
        recipe,
        contribution_ref,
        source,
    } = *binding;
    let contribution_ref = contribution_ref
        .ok_or_else(|| IndexError::CorruptObject("text posting lacks owner contribution".into()))?;
    let contribution = read_contribution(contribution_ref, context.objects, context.budget)?;
    let pinned_record = match source {
        PostingSourceBinding::Prefetched(value) => value,
        PostingSourceBinding::Lookup => context.maps.lookup(
            context.ticket.view.source.owners,
            &owner_map_key(owner),
            context.budget,
        )?,
    };
    let pinned_record = pinned_record.map(MapValue::object_ref).transpose()?;
    if pinned_record != Some(contribution.record) {
        return Err(IndexError::CorruptObject(
            "text contribution is stale against pinned source".into(),
        ));
    }
    let source_object = context.objects.get(contribution.record, context.budget)?;
    // The object lease covers payload bytes, not the newly owned JSON path or
    // escape scratch. Fund both before decoding even a malformed source record.
    let decode_bytes = u64::try_from(source_object.payload.len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(3)
        .ok_or(IndexError::ResourceLimited)?;
    let _decode_memory = if decode_bytes == 0 {
        None
    } else {
        context.budget.reserve_memory(
            decode_bytes,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?
    };
    let decoded = decode::<SourceRecord>(&source_object.payload);
    context.budget.checkpoint()?;
    let record = decoded?;
    if source_object.registration.reference != contribution.record
        || source_object.registration.children.as_slice() != [record.bytes]
        || record.byte_len > MAX_SOURCE_BYTES
        || record.path.is_empty()
        || record.path.len() > MAX_SOURCE_PATH_BYTES
        || record.path.starts_with('/')
        || record.path.contains('\0')
        || record
            .path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !matches!(record.text, SourceTextAdmission::Utf8 { policy } if policy == recipe.admission_policy)
    {
        return Err(IndexError::CorruptObject(
            "text source record path, edges or admission mismatch".into(),
        ));
    }
    let packed = matches!(contribution.inventory, EntryMap::OccurrenceBlock(_));
    let inventory_ref = match contribution.inventory {
        EntryMap::OccurrenceBlock(reference) => reference,
        inventory => inventory
            .lookup(context.objects, context.maps, term_key, context.budget)?
            .ok_or_else(|| {
                IndexError::CorruptObject("text posting lacks owner term inventory".into())
            })?,
    };
    if !witness.matches(inventory_ref) {
        return Err(IndexError::CorruptObject(
            "text posting differs from owner term inventory".into(),
        ));
    }
    let occurrence = if packed {
        occurrences.load_packed(inventory_ref, term_key, context.objects, context.budget)?
    } else {
        occurrences.load(inventory_ref, term_key, context.objects, context.budget)?
    };
    let tf =
        u64::try_from(occurrence.positions.len()).map_err(|_error| IndexError::ResourceLimited)?;
    if tf > contribution.indexed_tokens {
        return Err(IndexError::CorruptObject(
            "text term frequency exceeds document indexed token count".into(),
        ));
    }
    Ok(ValidatedPostingHit {
        owner,
        source_record: contribution.record,
        doc_indexed_tokens: contribution.indexed_tokens,
        occurrence,
    })
}

/// One posting page's two immutable map projections. The page and these
/// bounded values are provisional until the cursor reaches its checked EOF.
struct PostingPageLookups {
    contributions: PreparedRows<Option<MapValue>>,
    sources: PreparedRows<Option<MapValue>>,
    next: usize,
}

impl PostingPageLookups {
    fn new() -> Result<Self, IndexError> {
        Ok(Self {
            contributions: PreparedRows::new(POSTING_PAGE_CAPACITY)?,
            sources: PreparedRows::new(POSTING_PAGE_CAPACITY)?,
            next: 0,
        })
    }

    fn fill(
        &mut self,
        page: &quanta_index_core::index::MapPage,
        root: Root,
        context: &mut TextReadContext<'_>,
    ) -> Result<(), IndexError> {
        let scratch_bytes = std::mem::size_of::<[&[u8]; POSTING_PAGE_CAPACITY]>()
            .checked_mul(2)
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<[[u8; 38]; POSTING_PAGE_CAPACITY]>())
            })
            .ok_or(IndexError::ResourceLimited)?;
        let _scratch = context.budget.reserve_memory(
            u64::try_from(scratch_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        self.contributions.clear();
        self.sources.clear();
        self.next = 0;
        let count = page.entries.len();
        let mut owner_keys: [&[u8]; POSTING_PAGE_CAPACITY] = [&[]; POSTING_PAGE_CAPACITY];
        let mut source_keys = [[0_u8; 38]; POSTING_PAGE_CAPACITY];
        for (slot, (key, _value)) in page.entries.iter().enumerate() {
            context.budget.checkpoint()?;
            *owner_keys
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)? = key;
            *source_keys
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)? = owner_map_key(decode_posting_owner(key)?);
            self.contributions.push(None, context.budget)?;
            self.sources.push(None, context.budget)?;
        }
        context.maps.lookup_batch(
            root.owners,
            owner_keys.get(..count).ok_or(IndexError::ResourceLimited)?,
            self.contributions.as_mut_slice(),
            context.budget,
        )?;
        let mut source_refs: [&[u8]; POSTING_PAGE_CAPACITY] = [&[]; POSTING_PAGE_CAPACITY];
        for (slot, key) in source_keys.iter().enumerate() {
            *source_refs
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)? = key;
        }
        context.maps.lookup_batch(
            context.ticket.view.source.owners,
            source_refs
                .get(..count)
                .ok_or(IndexError::ResourceLimited)?,
            self.sources.as_mut_slice(),
            context.budget,
        )?;
        Ok(())
    }

    fn next(&mut self) -> Result<(Option<MapValue>, Option<MapValue>), IndexError> {
        let slot = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok((
            *self
                .contributions
                .as_slice()
                .get(slot)
                .ok_or(IndexError::ResourceLimited)?,
            *self
                .sources
                .as_slice()
                .get(slot)
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }
}

/// Query-local, page-bounded traversal. A hit is provisional until EOF;
/// callers must exhaust every opened cursor before publishing a result.
pub(super) struct TextPostingCursor {
    root: Root,
    recipe: TextRecipe,
    key: PreparedKey,
    posting: Option<Posting>,
    after: Option<MapPageEntry>,
    entries: MapPageCursor,
    page_lookups: Option<PostingPageLookups>,
    more: bool,
    visited: u64,
    dense: Option<dense_posting::read::PackedCursor>,
}

impl TextPostingCursor {
    pub(super) fn key(&self) -> &[u8] {
        self.key.as_slice()
    }

    pub(super) fn statistics(&self) -> TextTermScan {
        TextTermScan {
            docs: self.root.docs,
            indexed_tokens: self.root.indexed_tokens,
            df: self.posting.map_or(0, |posting| posting.df),
        }
    }

    pub(super) fn next_hit(
        &mut self,
        context: &mut TextReadContext<'_>,
        occurrences: &mut OccurrenceReader,
    ) -> Result<Option<ValidatedPostingHit>, IndexError> {
        context.budget.checkpoint()?;
        let Some(posting) = self.posting else {
            return Ok(None);
        };
        loop {
            if let Some(entry) = self.entries.next() {
                let (owner_key, value) = entry.get()?;
                let witness = posting.root.posting_witness(*value)?;
                if self
                    .after
                    .as_ref()
                    .map(MapPageEntry::key)
                    .transpose()?
                    .is_some_and(|prior| owner_key.as_slice() <= prior)
                {
                    return Err(IndexError::BackendViolation(
                        "text posting scan is not strictly ordered".into(),
                    ));
                }
                context.budget.charge(WorkUsage {
                    reads: 1,
                    bytes: 32,
                    ..WorkUsage::default()
                })?;
                let hit = if let Some(lookups) = self.page_lookups.as_mut() {
                    let (contribution_ref, pinned_record) = lookups.next()?;
                    validate_posting_hit_with_refs(
                        witness,
                        self.key.as_slice(),
                        &PostingHitBinding {
                            owner: decode_posting_owner(owner_key)?,
                            recipe: self.recipe,
                            contribution_ref,
                            source: PostingSourceBinding::Prefetched(pinned_record),
                        },
                        context,
                        occurrences,
                    )?
                } else {
                    validate_posting_hit(
                        owner_key,
                        witness,
                        self.key.as_slice(),
                        self.root,
                        self.recipe,
                        context,
                        occurrences,
                    )?
                };
                self.visited = self
                    .visited
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                if self.visited > posting.df {
                    return Err(IndexError::CorruptObject(
                        "text posting exceeds declared document frequency".into(),
                    ));
                }
                self.after = Some(entry);
                return Ok(Some(hit));
            }
            if !self.more {
                if self.visited != posting.df {
                    return Err(IndexError::CorruptObject(
                        "text posting document frequency differs from complete scan".into(),
                    ));
                }
                return Ok(None);
            }
            let page = if let EntryMap::PostingPages(reference) = posting.root {
                if self.dense.is_none() {
                    let cursor = dense_posting::read::PackedCursor::open(
                        context.objects,
                        reference,
                        context.budget,
                    )?;
                    if cursor.total() != posting.df {
                        return Err(IndexError::CorruptObject(
                            "packed posting directory DF differs from metadata".into(),
                        ));
                    }
                    self.dense = Some(cursor);
                }
                self.dense
                    .as_mut()
                    .ok_or(IndexError::ResourceLimited)?
                    .next_page(context.objects, PAGE, context.budget)?
            } else {
                posting.root.scan(
                    context.objects,
                    context.maps,
                    self.after.as_ref().map(MapPageEntry::key).transpose()?,
                    PAGE,
                    context.budget,
                )?
            };
            if page.entries.len()
                > usize::try_from(PAGE).map_err(|_error| IndexError::ResourceLimited)?
                || (page.entries.is_empty() && page.has_more)
            {
                return Err(IndexError::BackendViolation(
                    "text posting scan exceeded its page or did not advance".into(),
                ));
            }
            if page.entries.len() > 1 {
                if self.page_lookups.is_none() {
                    self.page_lookups = Some(PostingPageLookups::new()?);
                }
                self.page_lookups
                    .as_mut()
                    .ok_or(IndexError::ResourceLimited)?
                    .fill(&page, self.root, context)?;
            } else {
                self.page_lookups = None;
            }
            self.more = page.has_more;
            self.entries = page.cursor();
            if !self.more {
                self.dense = None;
            }
        }
    }
}

fn adjust(current: u64, old: u64, new: u64) -> Result<u64, IndexError> {
    current
        .checked_sub(old)
        .and_then(|value| value.checked_add(new))
        .ok_or_else(|| IndexError::CorruptObject("native text statistic overflow".into()))
}

struct MapEntries<'a> {
    objects: &'a dyn ImmutableObjectPort,
    maps: &'a dyn PersistentMapPort,
    root: EntryMap,
    after: Option<MapPageEntry>,
    entries: MapPageCursor,
    more: bool,
    block: Option<entry_map::block::BlockCursor<'a>>,
    packed: Option<entry_map::occurrence::PackedInventoryCursor<'a>>,
    retain_packed_for_count: bool,
    dense: Option<dense_posting::read::PackedCursor>,
}

impl<'a> MapEntries<'a> {
    fn new(
        objects: &'a dyn ImmutableObjectPort,
        maps: &'a dyn PersistentMapPort,
        root: impl Into<EntryMap>,
    ) -> Self {
        Self {
            objects,
            maps,
            root: root.into(),
            after: None,
            entries: MapPageCursor::default(),
            more: true,
            block: None,
            packed: None,
            retain_packed_for_count: false,
            dense: None,
        }
    }

    fn next(&mut self, budget: &mut WorkBudget) -> Result<Option<MapPageEntry>, IndexError> {
        // An empty inline inventory has no page backing to retain. Validate
        // before short-circuiting so malformed padding cannot become absence.
        if self.more && self.root.inline_map().is_some_and(|small| small.len() == 0) {
            self.root.validate()?;
            budget.checkpoint()?;
            self.more = false;
        }
        loop {
            if let Some(entry) = self.entries.next() {
                // The canonical immutable page owns both head and continuation.
                // Only an actual SQL delta needs a separately admitted key copy.
                self.after = Some(entry.clone());
                return Ok(Some(entry));
            }
            if !self.more {
                if !self.retain_packed_for_count {
                    self.packed = None;
                }
                return Ok(None);
            }
            let page = if let EntryMap::Block(reference) = self.root {
                if self.block.is_none() {
                    self.block = Some(entry_map::block::BlockCursor::open(
                        self.objects,
                        reference,
                        budget,
                    )?);
                }
                self.block
                    .as_mut()
                    .ok_or(IndexError::ResourceLimited)?
                    .next_page(PAGE, budget)?
            } else if let EntryMap::OccurrenceBlock(reference) = self.root {
                if self.packed.is_none() {
                    self.packed = Some(entry_map::occurrence::PackedInventoryCursor::open(
                        self.objects,
                        reference,
                        budget,
                    )?);
                }
                self.packed
                    .as_mut()
                    .ok_or(IndexError::ResourceLimited)?
                    .next_page(PAGE, budget)?
            } else if let EntryMap::PostingPages(reference) = self.root {
                if self.dense.is_none() {
                    self.dense = Some(dense_posting::read::PackedCursor::open(
                        self.objects,
                        reference,
                        budget,
                    )?);
                }
                self.dense
                    .as_mut()
                    .ok_or(IndexError::ResourceLimited)?
                    .next_page(self.objects, PAGE, budget)?
            } else {
                self.root.scan(
                    self.objects,
                    self.maps,
                    self.after.as_ref().map(MapPageEntry::key).transpose()?,
                    PAGE,
                    budget,
                )?
            };
            if page.entries.len()
                > usize::try_from(PAGE).map_err(|_error| IndexError::ResourceLimited)?
                || (page.entries.is_empty() && page.has_more)
            {
                return Err(IndexError::BackendViolation(
                    "native text inventory scan exceeded its page or did not advance".into(),
                ));
            }
            let mut prior = self.after.as_ref().map(MapPageEntry::key).transpose()?;
            for (key, _) in &page.entries {
                if prior.is_some_and(|value| key.as_slice() <= value) {
                    return Err(IndexError::BackendViolation(
                        "native text inventory scan is not ordered".into(),
                    ));
                }
                prior = Some(key);
            }
            self.more = page.has_more;
            self.entries = page.cursor();
            if !self.more {
                self.block = None;
                if !self.retain_packed_for_count {
                    self.packed = None;
                }
                self.dense = None;
            }
        }
    }
}

fn source_body(
    record: &SourceRecord,
    context: &mut AttemptContext<'_>,
) -> Result<StoredObject, IndexError> {
    context.budget.checkpoint()?;
    if record.byte_len > MAX_SOURCE_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let stored = context.objects.get(record.bytes, context.budget)?;
    context.budget.checkpoint()?;
    if stored.registration.reference != record.bytes
        || !stored.registration.children.is_empty()
        || u64::try_from(stored.payload.len()).map_err(|_error| IndexError::ResourceLimited)?
            != record.byte_len
    {
        return Err(IndexError::CorruptObject(
            "native text source body identity, edges or length mismatch".into(),
        ));
    }
    Ok(stored)
}

type PacketKeyMemory = [Option<quanta_index_core::index::MemoryLease>;
    quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];

struct InventoryEdits {
    ordinal: u32,
    rows: PreparedRows<(InventoryPartition, MapEdit)>,
    key_memory: PacketKeyMemory,
}

impl InventoryEdits {
    fn new(ordinal: u32) -> Result<Self, IndexError> {
        Ok(Self {
            ordinal,
            rows: PreparedRows::new(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS)?,
            key_memory: std::array::from_fn(|_| None),
        })
    }

    fn push(
        &mut self,
        key: &[u8],
        prior: ObjectRef,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        check_term_key(key)?;
        self.push_prepared(PreparedKey::copy(key, budget)?, Some(prior), None, budget)
    }

    /// Move an already funded key; only confirmed immutable references may enter.
    fn push_prepared(
        &mut self,
        key: PreparedKey,
        prior: Option<ObjectRef>,
        next: Option<ObjectRef>,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        self.push_partition(
            InventoryPartition::Ready(self.ordinal),
            key,
            prior,
            next,
            budget,
        )
    }

    fn push_partition(
        &mut self,
        partition: InventoryPartition,
        key: PreparedKey,
        prior: Option<ObjectRef>,
        next: Option<ObjectRef>,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        check_term_key(key.as_slice())?;
        if prior == next {
            return Err(IndexError::BackendViolation(
                "inventory packet contains an unchanged edit".into(),
            ));
        }
        let custody = self
            .key_memory
            .get_mut(self.rows.len())
            .ok_or(IndexError::ResourceLimited)?;
        let (key, grant) = key.into_parts();
        self.rows.push(
            (
                partition,
                MapEdit {
                    key,
                    expected: prior.map(Into::into),
                    next: next.map(Into::into),
                },
            ),
            budget,
        )?;
        *custody = grant;
        Ok(())
    }

    fn flush(&mut self, spool: &TextSpool, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if self.rows.len() == 0 {
            return Ok(());
        }
        spool.inventory_batch(self.rows.as_slice(), budget)?;
        self.rows.clear();
        self.key_memory.iter_mut().for_each(|grant| *grant = None);
        Ok(())
    }
}

struct PostingEdits {
    rows: PreparedRows<PostingScratchRow>,
    limit: usize,
}

impl PostingEdits {
    fn new() -> Result<Self, IndexError> {
        Self::with_limit(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS)
    }

    fn with_limit(limit: usize) -> Result<Self, IndexError> {
        Ok(Self {
            rows: PreparedRows::new(limit)?,
            limit,
        })
    }

    fn is_full(&self) -> bool {
        self.rows.len() == self.limit
    }

    fn push(
        &mut self,
        term: &[u8],
        owner: &[u8],
        expected: Option<ObjectRef>,
        next: Option<ObjectRef>,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        check_term_key(term)?;
        if expected == next {
            return Err(IndexError::BackendViolation(
                "invalid native posting delta".into(),
            ));
        }
        let owner: [u8; 32] = owner.try_into().map_err(|_error| {
            IndexError::BackendViolation("invalid native posting owner width".into())
        })?;
        let term = PostingTermKey::copy(term, budget)?;
        self.rows.push(
            PostingScratchRow {
                term,
                owner,
                expected: expected.map(Into::into),
                next: next.map(Into::into),
            },
            budget,
        )
    }

    fn flush(&mut self, spool: &TextSpool, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if self.rows.len() == 0 {
            return Ok(());
        }
        spool.posting_batch_rows(self.rows.as_slice(), budget)?;
        self.rows.clear();
        Ok(())
    }
}

struct OccurrenceEdit {
    ordinal: u32,
    key: PreparedKey,
    prior: Option<ObjectRef>,
    identified: ObjectRef,
}

struct PendingOccurrence {
    // Largest inventory ordinal among this unique leaf's SQL fanout.
    ordinal: u32,
    identified: ObjectRef,
}

#[derive(Clone, Copy)]
struct PendingOccurrenceSlot {
    index: usize,
    identified: ObjectRef,
}

// A byte boundary complements the canonical object-count envelope. Large
// occurrence leaves flush earlier through the same durable installer.
const MAX_OCCURRENCE_STAGED_BYTES: u64 = 1024 * 1024;

struct OccurrenceWrites {
    pack_owners: bool,
    inputs: Vec<ObjectBatchInput>,
    pending: Vec<PendingOccurrence>,
    fanout_rows: u64,
    retained_bytes: u64,
    // Payload backing drops before its grants; array backing before metadata.
    payload_memory: Vec<Option<quanta_index_core::index::MemoryLease>>,
    metadata_memory: Option<quanta_index_core::index::MemoryLease>,
}

impl OccurrenceWrites {
    fn metadata_bytes(capacity: usize) -> Result<u64, IndexError> {
        let per_row = [
            std::mem::size_of::<ObjectBatchInput>(),
            std::mem::size_of::<PendingOccurrence>(),
            std::mem::size_of::<Option<quanta_index_core::index::MemoryLease>>(),
        ]
        .into_iter()
        .try_fold(0usize, |sum, bytes| {
            sum.checked_add(bytes).ok_or(IndexError::ResourceLimited)
        })?;
        let bytes = per_row
            .checked_mul(capacity)
            .ok_or(IndexError::ResourceLimited)?;
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
    }

    fn new() -> Self {
        Self {
            pack_owners: false,
            inputs: Vec::new(),
            pending: Vec::new(),
            fanout_rows: 0,
            retained_bytes: 0,
            payload_memory: Vec::new(),
            metadata_memory: None,
        }
    }

    fn reserve_next(&mut self, budget: &WorkBudget) -> Result<(), IndexError> {
        let needed = self
            .inputs
            .len()
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if needed > quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH {
            return Err(IndexError::ResourceLimited);
        }
        if needed <= self.inputs.capacity() {
            return budget.checkpoint();
        }
        let capacity = self
            .inputs
            .capacity()
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?
            .max(needed)
            .clamp(8, quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH);
        let memory = budget.reserve_memory(
            Self::metadata_bytes(capacity)?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        let mut inputs = Vec::new();
        let mut pending = Vec::new();
        let mut payload_memory = Vec::new();
        inputs
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        pending
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        payload_memory
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        inputs.append(&mut self.inputs);
        pending.append(&mut self.pending);
        payload_memory.append(&mut self.payload_memory);
        self.inputs = inputs;
        self.pending = pending;
        self.payload_memory = payload_memory;
        self.metadata_memory = memory;
        Ok(())
    }

    fn needs_flush(&self, reference: ObjectRef, payload_bytes: usize) -> Result<bool, IndexError> {
        if self.pending.iter().any(|row| row.identified == reference) {
            return Ok(false);
        }
        let next = self
            .retained_bytes
            .checked_add(
                u64::try_from(payload_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        Ok(!self.pending.is_empty()
            && (self.pending.len() == quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH
                || next > MAX_OCCURRENCE_STAGED_BYTES))
    }

    fn push(
        &mut self,
        edit: OccurrenceEdit,
        payload: super::text_memory::PreparedPayload,
        spool: &TextSpool,
        inventory_edits: &mut InventoryEdits,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.push_with_slot(edit, payload, spool, inventory_edits, budget)
            .map(|_slot| ())
    }

    fn push_with_slot(
        &mut self,
        edit: OccurrenceEdit,
        payload: super::text_memory::PreparedPayload,
        spool: &TextSpool,
        inventory_edits: &mut InventoryEdits,
        budget: &mut WorkBudget,
    ) -> Result<PendingOccurrenceSlot, IndexError> {
        let OccurrenceEdit {
            ordinal,
            key: term,
            prior,
            identified,
        } = edit;
        budget.checkpoint()?;
        if usize::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?
            >= MAX_INVENTORY_ROOTS
        {
            return Err(IndexError::BackendViolation(
                "occurrence ordinal exceeds its inventory window".into(),
            ));
        }
        check_term_key(term.as_slice())?;
        if prior == Some(identified) {
            return Err(IndexError::BackendViolation(
                "inventory packet contains an unchanged edit".into(),
            ));
        }
        let fanout_rows = self
            .fanout_rows
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let duplicate = self
            .pending
            .iter()
            .position(|pending| pending.identified == identified);
        let slot = duplicate.unwrap_or(self.pending.len());
        let retained_bytes = if duplicate.is_some() {
            self.retained_bytes
        } else {
            self.retained_bytes
                .checked_add(
                    u64::try_from(payload.as_slice().len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?
        };
        if retained_bytes > MAX_OCCURRENCE_STAGED_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        if let Some(slot) = duplicate {
            let earlier = self.inputs.get(slot).ok_or(IndexError::ResourceLimited)?;
            if earlier.payload.as_slice() != payload.as_slice() || !earlier.children.is_empty() {
                return Err(IndexError::BackendViolation(
                    "native text repeated occurrence identity has different input".into(),
                ));
            }
        } else {
            self.reserve_next(budget)?;
        }
        // Logical fanout is bounded by the existing scratch/work limits, not
        // retained as aliases in RAM. Planned rows cannot be read as map edits.
        inventory_edits.push_partition(
            InventoryPartition::Planned(ordinal),
            term,
            prior,
            Some(identified),
            budget,
        )?;
        if inventory_edits.rows.len() == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            inventory_edits.flush(spool, budget)?;
        }
        if let Some(slot) = duplicate {
            let pending = self
                .pending
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)?;
            pending.ordinal = pending.ordinal.max(ordinal);
        } else {
            let (payload, memory) = payload.into_parts();
            self.inputs.push(ObjectBatchInput {
                payload,
                children: Vec::new(),
            });
            self.pending.push(PendingOccurrence {
                ordinal,
                identified,
            });
            self.payload_memory.push(memory);
        }
        self.fanout_rows = fanout_rows;
        self.retained_bytes = retained_bytes;
        Ok(PendingOccurrenceSlot {
            index: slot,
            identified,
        })
    }

    fn flush(
        &mut self,
        context: &mut AttemptContext<'_>,
        spool: &TextSpool,
        confirmed: &mut ConfirmedOccurrences,
        inventory_edits: &mut InventoryEdits,
    ) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        if self.inputs.len() != self.pending.len() || self.inputs.len() != self.payload_memory.len()
        {
            return Err(IndexError::BackendViolation(
                "native occurrence batch custody differs".into(),
            ));
        }
        if self.pending.is_empty() {
            if self.fanout_rows != 0 || self.retained_bytes != 0 {
                return Err(IndexError::BackendViolation(
                    "native occurrence fanout has no physical packet".into(),
                ));
            }
            return Ok(());
        }
        if self.pending.len() > quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH {
            return Err(IndexError::ResourceLimited);
        }
        // Every allocation and SQL PK migration is admitted before durable
        // installation. One unique payload is kept for each physical output.
        confirmed.prepare(context.attempt, context.budget)?;
        inventory_edits.flush(spool, context.budget)?;
        let mut references =
            [ObjectRef(Digest([0; 32])); quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH];
        for (slot, pending) in self.pending.iter().enumerate() {
            *references
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)? = pending.identified;
        }
        let references = references
            .get(..self.pending.len())
            .ok_or(IndexError::ResourceLimited)?;
        let confirmation = spool.admit_inventory_confirmation(
            references,
            self.fanout_rows,
            context.attempt,
            confirmed.limit != 0,
            context.budget,
        )?;
        let installed = context.put_batch(&self.inputs)?;
        if installed.len() != references.len() {
            return Err(IndexError::BackendViolation(
                "native text changed occurrence batch lost ordinal output".into(),
            ));
        }
        // A malformed later output must leave every logical row Planned.
        for (expected, actual) in references.iter().zip(installed.iter()) {
            context.budget.checkpoint()?;
            if expected != actual {
                return Err(IndexError::BackendViolation(
                    "native text occurrence identity changed during installation".into(),
                ));
            }
        }
        confirmation.finish(context.budget)?;
        for reference in references {
            confirmed.remember(*reference, context.attempt, context.budget)?;
        }
        drop(installed);
        self.inputs.clear();
        self.pending.clear();
        self.payload_memory.clear();
        self.fanout_rows = 0;
        self.retained_bytes = 0;
        Ok(())
    }
}

pub(super) fn visit_indexed_token_batches(
    body: &str,
    budget: &mut WorkBudget,
    mut consume: impl FnMut(&TokenBatch, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<u64, IndexError> {
    budget.charge(WorkUsage {
        bytes: u64::try_from(body.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(4)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let mut cursor = TokenCursor::try_new_bounded(
        body,
        usize::try_from(MAX_NFC_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
        usize::try_from(MAX_NFC_WORK_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
        |_units| budget.checkpoint(),
        |bytes| {
            budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )
        },
    )
    .map_err(|error| {
        error
            .into_checkpoint()
            .map_or(IndexError::ResourceLimited, std::convert::identity)
    })?;
    let mut indexed_tokens = 0u64;
    let mut token_rows = TokenBatch::new(budget)?;
    let mut runs = [SourceRun::default(); TOKEN_RUN_BATCH];
    let mut run_count = 0usize;
    while let Some(run) = cursor.next_run() {
        budget.checkpoint()?;
        if !run.is_indexable() {
            continue;
        }
        let position = u32::try_from(run.position).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            mutations: 1,
            bytes: u64::try_from(run.text.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_mul(4)
                .and_then(|bytes| bytes.checked_add(128))
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        *runs.get_mut(run_count).ok_or(IndexError::ResourceLimited)? = SourceRun {
            start: run.start,
            end: run.end,
            position,
        };
        run_count = run_count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if run_count == TOKEN_RUN_BATCH {
            token_rows.prepare(
                cursor.text(),
                runs.get(..run_count).ok_or(IndexError::ResourceLimited)?,
                budget,
            )?;
            consume(&token_rows, budget)?;
            token_rows.clear();
            run_count = 0;
        }
        indexed_tokens = indexed_tokens
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
    }
    if run_count > 0 {
        token_rows.prepare(
            cursor.text(),
            runs.get(..run_count).ok_or(IndexError::ResourceLimited)?,
            budget,
        )?;
        consume(&token_rows, budget)?;
    }
    drop(token_rows);
    drop(cursor);
    Ok(indexed_tokens)
}

struct PreparedInventory {
    changed: bool,
    indexed_tokens: u64,
    term_count: u64,
    alias: Option<entry_patch::ColdInventoryAlias>,
}

fn prepare_inventory(
    body: &str,
    prior_inventory: &EntryMap,
    ordinal: u32,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    confirmed: &mut ConfirmedOccurrences,
    inventory_edits: &mut InventoryEdits,
    writes: &mut OccurrenceWrites,
) -> Result<PreparedInventory, IndexError> {
    context.budget.checkpoint()?;
    if inventory_edits.ordinal != ordinal {
        return Err(IndexError::BackendViolation(
            "native inventory ordinal changed during preparation".into(),
        ));
    }
    let cold = writes.pack_owners
        && *prior_inventory == EntryMap::default()
        && body.is_ascii();
    let mut cold_tokens = if cold {
        cold_tokens::ColdTokens::try_new(body.len(), context.budget)?
    } else {
        None
    };
    let diagnostic = context.budget.request().diagnostic_context().cloned();
    let indexed_tokens = observe_attempt_on_lane(
        context,
        diagnostic.as_ref(),
        DiagnosticStageV1::NativeTextInventoryPrepare,
        quanta_index_core::DiagnosticLaneV1(34),
        |_child, context| {
            if let Some(tokens) = cold_tokens.as_mut() {
                let indexed = visit_indexed_token_batches(body, context.budget, |rows, budget| {
                    tokens.push_batch(rows, budget)
                })?;
                tokens.sort_and_validate(context.budget)?;
                Ok(indexed)
            } else {
                spool.reset_tokens(context.budget)?;
                visit_indexed_token_batches(body, context.budget, |rows, budget| {
                    spool.token_batch(rows, budget)
                })
            }
        },
    )?;

    // Merge two sorted file-local inventories. Unchanged occurrence identities
    // stay reachable from the prior COW root; only changed keys become map edits.
    let (changed, term_count, alias) = observe_attempt_on_lane(
        context,
        diagnostic.as_ref(),
        DiagnosticStageV1::NativeTextInventoryPrepare,
        quanta_index_core::DiagnosticLaneV1(35),
        |_child, context| {
            if writes.pack_owners && *prior_inventory == EntryMap::default() {
                let (count, alias) = occurrence_pack::prepare_owner_blocks(
                    ordinal,
                    context,
                    spool,
                    cold_tokens.as_ref(),
                    confirmed,
                    inventory_edits,
                    writes,
                )?;
                return Ok((count != 0, count, alias));
            }
            let mut occurrences = if confirmed.limit == 0 {
                OccurrenceReader::uncached()?
            } else {
                OccurrenceReader::new(2)?
            };
            let mut old_cursor = MapEntries::new(context.objects, context.maps, prior_inventory);
            let mut old = old_cursor.next(context.budget)?;
            let mut changed = false;
            let mut term_count = 0_u64;
            // Reborrow the existing authority with the visitor's original
            // budget. No context or grant escapes the live token cursor.
            let attempt = context.attempt;
            let objects = context.objects;
            let maps = context.maps;
            let custody = context.custody;
            let scratch = context.scratch;
            spool.visit_token_groups(
                usize::try_from(MAX_POSITIONS_PER_TERM)
                    .map_err(|_error| IndexError::ResourceLimited)?,
                context.budget,
                |term, positions, budget| {
                    let mut group_context = AttemptContext {
                        attempt,
                        objects,
                        maps,
                        custody,
                        scratch,
                        budget,
                    };
                    let context = &mut group_context;
                    context.budget.checkpoint()?;
                    term_count = term_count
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                    check_term_key(term.as_slice())?;
                    while old
                        .as_ref()
                        .map(MapPageEntry::key)
                        .transpose()?
                        .is_some_and(|key| key < term.as_slice())
                    {
                        let entry = old.take().ok_or_else(|| {
                            IndexError::BackendViolation(
                                "native text old inventory lost entry".into(),
                            )
                        })?;
                        let (key, reference) = entry.object_parts()?;
                        inventory_edits.push(key, *reference, context.budget)?;
                        changed = true;
                        if inventory_edits.rows.len()
                            == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
                        {
                            inventory_edits.flush(spool, context.budget)?;
                        }
                        old = old_cursor.next(context.budget)?;
                    }
                    let previous = if old
                        .as_ref()
                        .map(MapPageEntry::key)
                        .transpose()?
                        .is_some_and(|key| key == term.as_slice())
                    {
                        let entry = old.take().ok_or_else(|| {
                            IndexError::BackendViolation(
                                "native text old inventory lost match".into(),
                            )
                        })?;
                        let reference = *entry.object_parts()?.1;
                        old = old_cursor.next(context.budget)?;
                        Some(reference)
                    } else {
                        None
                    };
                    if positions.as_slice().is_empty() {
                        return Err(IndexError::BackendViolation(
                            "native text token term has no positions".into(),
                        ));
                    }
                    let payload =
                        super::text_memory::encode(&positions.occurrence, context.budget)?;
                    let identified =
                        context
                            .objects
                            .identify(payload.as_slice(), &[], context.budget)?;
                    if previous == Some(identified.reference) {
                        return Ok(());
                    }
                    // The current policy accepts both leaf and block DATA.
                    // A delta keeps the authenticated block identity when the
                    // selected term's positions have not changed.
                    if let Some(reference) = previous {
                        let prior = occurrences.load(
                            reference,
                            term.as_slice(),
                            context.objects,
                            context.budget,
                        )?;
                        if prior.positions.iter().eq(positions.as_slice()) {
                            return Ok(());
                        }
                    }
                    drop(positions);
                    changed = true;
                    // A pending leaf has not reached confirmation. Keep its
                    // attempt fence without querying the guaranteed SQL miss;
                    // push still verifies the duplicate payload before staging.
                    let pending = writes
                        .pending
                        .iter()
                        .any(|row| row.identified == identified.reference);
                    if pending {
                        confirmed.check_attempt(context.attempt)?;
                    }
                    if !pending
                        && confirmed.contains_or_spill(
                            identified.reference,
                            context.attempt,
                            spool,
                            context.budget,
                        )?
                    {
                        inventory_edits.push_prepared(
                            term,
                            previous,
                            Some(identified.reference),
                            context.budget,
                        )?;
                        if inventory_edits.rows.len()
                            == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
                        {
                            inventory_edits.flush(spool, context.budget)?;
                        }
                        return Ok(());
                    }
                    if writes.needs_flush(identified.reference, payload.as_slice().len())? {
                        writes.flush(context, spool, confirmed, inventory_edits)?;
                    }
                    writes.push(
                        OccurrenceEdit {
                            ordinal,
                            key: term,
                            prior: previous,
                            identified: identified.reference,
                        },
                        payload,
                        spool,
                        inventory_edits,
                        context.budget,
                    )?;
                    if writes.pending.len()
                        == quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH
                    {
                        writes.flush(context, spool, confirmed, inventory_edits)?;
                    }
                    Ok(())
                },
            )?;
            while let Some(entry) = old {
                context.budget.checkpoint()?;
                let (key, reference) = entry.object_parts()?;
                inventory_edits.push(key, *reference, context.budget)?;
                changed = true;
                if inventory_edits.rows.len() == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
                    inventory_edits.flush(spool, context.budget)?;
                }
                old = old_cursor.next(context.budget)?;
            }
            Ok((changed, term_count, None))
        },
    )?;
    Ok(PreparedInventory {
        changed,
        indexed_tokens,
        term_count,
        alias,
    })
}

#[derive(Clone, Copy)]
enum ValidationSide {
    Prior,
    Next,
}

#[derive(Default)]
struct OccurrenceValidationTotals {
    prior_validation_attempts: u64,
    next_validation_attempts: u64,
    ram_reuses: u64,
    spill_reuses: u64,
    equal_reuses: u64,
    validation_failures: u64,
    overflowed: bool,
}

impl OccurrenceValidationTotals {
    fn increment(value: &mut u64, overflowed: &mut bool) {
        if let Some(next) = value.checked_add(1) {
            *value = next;
        } else {
            *overflowed = true;
        }
    }

    fn attempt(&mut self, side: ValidationSide) {
        let value = match side {
            ValidationSide::Prior => &mut self.prior_validation_attempts,
            ValidationSide::Next => &mut self.next_validation_attempts,
        };
        Self::increment(value, &mut self.overflowed);
    }

    fn ram_reuse(&mut self) {
        Self::increment(&mut self.ram_reuses, &mut self.overflowed);
    }

    fn spill_reuse(&mut self) {
        Self::increment(&mut self.spill_reuses, &mut self.overflowed);
    }

    fn equal_reuse(&mut self) {
        Self::increment(&mut self.equal_reuses, &mut self.overflowed);
    }

    fn validation_failure(&mut self) {
        Self::increment(&mut self.validation_failures, &mut self.overflowed);
    }

    fn emit(
        &self,
        diagnostic: &quanta_index_core::DiagnosticContextV1,
        span: &quanta_index_core::DiagnosticSpanGuardV1,
    ) {
        use quanta_index_core::DiagnosticCounterV1;

        if self.overflowed {
            diagnostic.health(quanta_index_core::DiagnosticHealthCodeV1::CounterOverflow);
            return;
        }
        for (key, value) in [
            (
                DiagnosticCounterV1::TextOccurrencePriorValidationAttempts,
                self.prior_validation_attempts,
            ),
            (
                DiagnosticCounterV1::TextOccurrenceNextValidationAttempts,
                self.next_validation_attempts,
            ),
            (
                DiagnosticCounterV1::TextOccurrenceRamReuses,
                self.ram_reuses,
            ),
            (
                DiagnosticCounterV1::TextOccurrenceSpillReuses,
                self.spill_reuses,
            ),
            (
                DiagnosticCounterV1::TextOccurrenceEqualReuses,
                self.equal_reuses,
            ),
            (
                DiagnosticCounterV1::TextOccurrenceValidationFailures,
                self.validation_failures,
            ),
        ] {
            diagnostic.counter(span, key, value);
        }
    }
}

fn validated_occurrence_count(
    key: &[u8],
    reference: ObjectRef,
    occurrences: &mut OccurrenceReader,
    context: &mut AttemptContext<'_>,
    mut totals: Option<&mut OccurrenceValidationTotals>,
    side: ValidationSide,
) -> Result<OccurrenceCount, IndexError> {
    let diagnostic = context.budget.request().diagnostic_context().cloned();
    observe_text_step(
        diagnostic.as_ref(),
        DiagnosticStageV1::NativeTextOccurrenceRead,
        31,
        context,
        |context| {
            check_term_key(key)?;
            let retained = occurrences.contains(reference);
            if !retained && let Some(totals) = totals.as_deref_mut() {
                totals.attempt(side);
            }
            let result = occurrences.count(reference, key, context.objects, context.budget);
            if result.is_ok()
                && retained
                && let Some(totals) = totals.as_deref_mut()
            {
                totals.ram_reuse();
            }
            if result.is_err()
                && !retained
                && let Some(totals) = totals
            {
                totals.validation_failure();
            }
            result
        },
    )
}

fn read_diff_inventory_entry(
    cursor: &mut MapEntries<'_>,
    context: &mut AttemptContext<'_>,
) -> Result<Option<MapPageEntry>, IndexError> {
    // Buffered rows and terminal EOF perform no map-port fetch. Avoid three
    // events and clock reads per row; keep attribution at the actual page fill.
    if !cursor.entries.is_empty()
        || !cursor.more
        || cursor
            .root
            .inline_map()
            .is_some_and(|small| small.len() == 0)
    {
        return cursor.next(context.budget);
    }
    let diagnostic = context.budget.request().diagnostic_context().cloned();
    observe_text_step(
        diagnostic.as_ref(),
        DiagnosticStageV1::NativeTextInventoryRead,
        30,
        context,
        |context| cursor.next(context.budget),
    )
}

fn flush_diff_posting_edits(
    rows: &mut PostingEdits,
    spool: &TextSpool,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let diagnostic = context.budget.request().diagnostic_context().cloned();
    observe_text_step(
        diagnostic.as_ref(),
        DiagnosticStageV1::NativeTextPostingEdit,
        33,
        context,
        |context| rows.flush(spool, context.budget),
    )
}

fn diff_inventory(
    owner: &[u8],
    prior: Option<&Contribution>,
    next: Option<&Contribution>,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    posting_rows: &mut PostingEdits,
    confirmed: &mut ConfirmedOccurrences,
) -> Result<(), IndexError> {
    let diagnostic = context.budget.request().diagnostic_context().cloned();
    let Some(diagnostic) = diagnostic.as_ref() else {
        return diff_inventory_inner(
            owner,
            (prior, next),
            context,
            spool,
            posting_rows,
            confirmed,
            None,
        );
    };
    let Some(span) =
        crate::causal_profile::enter_span(diagnostic, DiagnosticStageV1::NativeTextInventoryDiff)
    else {
        // A missing owner must not attribute its children to an outer span.
        // Reuse the existing explicit-mask path while preserving product work.
        return observe_attempt(
            context,
            None,
            DiagnosticStageV1::NativeTextInventoryDiff,
            |_diagnostic, context| {
                diff_inventory_inner(
                    owner,
                    (prior, next),
                    context,
                    spool,
                    posting_rows,
                    confirmed,
                    None,
                )
            },
        );
    };
    let child = diagnostic.child_context(&span);
    let mut totals = OccurrenceValidationTotals::default();
    let before = context.budget.usage();
    let result = {
        let mut scoped_budget = context.budget.scoped_diagnostic_context(Some(child));
        let mut scoped_context = AttemptContext {
            attempt: context.attempt,
            objects: context.objects,
            maps: context.maps,
            custody: context.custody,
            scratch: context.scratch,
            budget: &mut scoped_budget,
        };
        let result = diff_inventory_inner(
            owner,
            (prior, next),
            &mut scoped_context,
            spool,
            posting_rows,
            confirmed,
            Some(&mut totals),
        );
        scoped_context
            .budget
            .record_diagnostic_usage_delta(before, &span);
        result
    };
    totals.emit(diagnostic, &span);
    let _finished = span.finish(quanta_index_core::DiagnosticTerminalV1::from_result(
        &result,
    ));
    result
}

fn diff_inventory_inner(
    owner: &[u8],
    (prior, next): (Option<&Contribution>, Option<&Contribution>),
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    posting_rows: &mut PostingEdits,
    confirmed: &mut ConfirmedOccurrences,
    mut totals: Option<&mut OccurrenceValidationTotals>,
) -> Result<(), IndexError> {
    let mut before = MapEntries::new(
        context.objects,
        context.maps,
        prior.map_or_else(EntryMap::default, |value| value.inventory),
    );
    let mut after = MapEntries::new(
        context.objects,
        context.maps,
        next.map_or_else(EntryMap::default, |value| value.inventory),
    );
    after.retain_packed_for_count = confirmed.limit != 0;
    let mut old = read_diff_inventory_entry(&mut before, context)?;
    let mut new = read_diff_inventory_entry(&mut after, context)?;
    let mut old_tokens = 0u64;
    let mut new_tokens = 0u64;
    while old.is_some() || new.is_some() {
        context.budget.checkpoint()?;
        match (
            old.as_ref().map(MapPageEntry::object_parts).transpose()?,
            new.as_ref().map(MapPageEntry::object_parts).transpose()?,
        ) {
            (Some((old_key, old_ref)), Some((new_key, new_ref))) if old_key == new_key => {
                let old_count = validated_occurrence_count(
                    old_key,
                    *old_ref,
                    &mut confirmed.blocks,
                    context,
                    totals.as_deref_mut(),
                    ValidationSide::Prior,
                )?
                .value;
                // One immutable identity has one decoded value. Both contribution
                // totals are still checked after the complete inventory merge.
                let new_count = if old_ref == new_ref {
                    if let Some(totals) = totals.as_deref_mut() {
                        totals.equal_reuse();
                    }
                    old_count
                } else {
                    confirmed.validated_new_count_observed(
                        new_key,
                        *new_ref,
                        spool,
                        context,
                        after.packed.as_mut(),
                        totals.as_deref_mut(),
                    )?
                };
                if old_key.first() == Some(&0) {
                    old_tokens = old_tokens
                        .checked_add(old_count)
                        .ok_or(IndexError::ResourceLimited)?;
                    new_tokens = new_tokens
                        .checked_add(new_count)
                        .ok_or(IndexError::ResourceLimited)?;
                }
                if old_ref != new_ref {
                    posting_rows.push(
                        old_key,
                        owner,
                        Some(*old_ref),
                        Some(*new_ref),
                        context.budget,
                    )?;
                }
                old = read_diff_inventory_entry(&mut before, context)?;
                new = read_diff_inventory_entry(&mut after, context)?;
            }
            (Some((old_key, old_ref)), Some((new_key, _))) if old_key < new_key => {
                let count = validated_occurrence_count(
                    old_key,
                    *old_ref,
                    &mut confirmed.blocks,
                    context,
                    totals.as_deref_mut(),
                    ValidationSide::Prior,
                )?
                .value;
                if old_key.first() == Some(&0) {
                    old_tokens = old_tokens
                        .checked_add(count)
                        .ok_or(IndexError::ResourceLimited)?;
                }
                posting_rows.push(old_key, owner, Some(*old_ref), None, context.budget)?;
                old = read_diff_inventory_entry(&mut before, context)?;
            }
            (Some((old_key, old_ref)), None) => {
                let count = validated_occurrence_count(
                    old_key,
                    *old_ref,
                    &mut confirmed.blocks,
                    context,
                    totals.as_deref_mut(),
                    ValidationSide::Prior,
                )?
                .value;
                if old_key.first() == Some(&0) {
                    old_tokens = old_tokens
                        .checked_add(count)
                        .ok_or(IndexError::ResourceLimited)?;
                }
                posting_rows.push(old_key, owner, Some(*old_ref), None, context.budget)?;
                old = read_diff_inventory_entry(&mut before, context)?;
            }
            (Some((old_key, _)), Some((new_key, new_ref))) if old_key > new_key => {
                let count = confirmed.validated_new_count_observed(
                    new_key,
                    *new_ref,
                    spool,
                    context,
                    after.packed.as_mut(),
                    totals.as_deref_mut(),
                )?;
                if new_key.first() == Some(&0) {
                    new_tokens = new_tokens
                        .checked_add(count)
                        .ok_or(IndexError::ResourceLimited)?;
                }
                posting_rows.push(new_key, owner, None, Some(*new_ref), context.budget)?;
                new = read_diff_inventory_entry(&mut after, context)?;
            }
            (None, Some((new_key, new_ref))) => {
                let count = confirmed.validated_new_count_observed(
                    new_key,
                    *new_ref,
                    spool,
                    context,
                    after.packed.as_mut(),
                    totals.as_deref_mut(),
                )?;
                if new_key.first() == Some(&0) {
                    new_tokens = new_tokens
                        .checked_add(count)
                        .ok_or(IndexError::ResourceLimited)?;
                }
                posting_rows.push(new_key, owner, None, Some(*new_ref), context.budget)?;
                new = read_diff_inventory_entry(&mut after, context)?;
            }
            (None, None) => break,
            (Some(_), Some(_)) => {
                return Err(IndexError::BackendViolation(
                    "native text inventory merge did not advance".into(),
                ));
            }
        }
        if posting_rows.is_full() {
            flush_diff_posting_edits(posting_rows, spool, context)?;
        }
    }
    if prior.is_some_and(|value| value.indexed_tokens != old_tokens)
        || next.is_some_and(|value| value.indexed_tokens != new_tokens)
    {
        return Err(IndexError::CorruptObject(
            "native text contribution token count differs from inventory".into(),
        ));
    }
    // Only a complete, count-consistent Diff can promote this cursor's
    // checked DATA into the same attempt's existing bounded reader window.
    if let Some(checked) = after
        .packed
        .take()
        .and_then(entry_map::occurrence::PackedInventoryCursor::into_validated_block)
    {
        context.budget.checkpoint()?;
        confirmed.check_attempt(context.attempt)?;
        let _slot = confirmed.blocks.admit_checked(checked, context.budget)?;
        confirmed.attempt = Some(context.attempt);
    }
    Ok(())
}

fn validate_reused_inventory(prior: &Contribution, next: &Contribution) -> Result<(), IndexError> {
    if prior.inventory != next.inventory || prior.indexed_tokens != next.indexed_tokens {
        return Err(IndexError::CorruptObject(
            "native text reused inventory metadata differs from prior contribution".into(),
        ));
    }
    Ok(())
}

/// Only published or original-attempt confirmed inventories reach this stage.
/// Fixed metadata retains no decoded source, body or uninstalled map reference.
struct ConfirmedTextChange {
    owner: [u8; 32],
    prior_ref: Option<MapValue>,
    prior: Option<Contribution>,
    next: Option<Contribution>,
    reused_inventory: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct TextBodyKey {
    bytes: ObjectRef,
    byte_len: u64,
}

#[derive(Clone, Copy, Default)]
struct ReadyTextNext {
    contribution: Option<Contribution>,
    reused: bool,
    verified: Option<VerifiedInventory>,
}
enum PendingTextNext {
    Preparing,
    Ready,
    Draft {
        record: ObjectRef,
        slot: usize,
        body: TextBodyKey,
        tokens: u64,
        alias: Option<entry_patch::ColdInventoryAlias>,
    },
}

struct PendingTextChange {
    owner: [u8; 32],
    prior_ref: Option<MapValue>,
    prior: Option<Contribution>,
    next: PendingTextNext,
}

/// A bounded private prepare window, never a publication or source owner.
struct InventoryBatch {
    rows: PreparedRows<PendingTextChange>,
    ready: PreparedRows<ReadyTextNext>,
    bases: PreparedRows<EntryMap>,
    // A compact alias has one staged anchor row and its full count in the seal.
    counts: PreparedRows<u64>,
    inventory_edits: InventoryEdits,
    occurrence_writes: OccurrenceWrites,
    posting_edits: PostingEdits,
    limit: usize,
    source_bytes: u64,
    attempt: Option<quanta_index_core::index::AttemptRef>,
}

impl InventoryBatch {
    fn new(limit: usize) -> Result<Self, IndexError> {
        Self::with_limit(limit, quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS)
    }

    fn cold(limit: usize) -> Result<Self, IndexError> {
        Self::with_limit(limit, MAX_INVENTORY_ROOTS)
    }

    fn with_limit(limit: usize, maximum: usize) -> Result<Self, IndexError> {
        if limit == 0 || limit > maximum {
            return Err(IndexError::InvalidInput(
                "invalid inventory batch limit".into(),
            ));
        }
        Ok(Self {
            rows: PreparedRows::new(limit)?,
            ready: PreparedRows::new(limit)?,
            bases: PreparedRows::new(limit)?,
            counts: PreparedRows::new(limit)?,
            inventory_edits: InventoryEdits::new(0)?,
            occurrence_writes: OccurrenceWrites::new(),
            posting_edits: PostingEdits::new()?,
            limit,
            source_bytes: 0,
            attempt: None,
        })
    }

    fn set_ready(
        &mut self,
        slot: usize,
        value: &ReadyTextNext,
    ) -> Result<PendingTextNext, IndexError> {
        *self
            .ready
            .as_mut_slice()
            .get_mut(slot)
            .ok_or_else(|| IndexError::BackendViolation("inventory ready slot missing".into()))? =
            *value;
        Ok(PendingTextNext::Ready)
    }

    fn check_attempt(&self, context: &AttemptContext<'_>) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        if self.attempt.is_some_and(|bound| bound != context.attempt) {
            return Err(IndexError::FenceLost);
        }
        Ok(())
    }

    fn flush(
        &mut self,
        root: &mut Root,
        context: &mut AttemptContext<'_>,
        spool: &TextSpool,
        contributions: &mut ContributionWrites,
        inventories: &mut InventoryReuse,
    ) -> Result<(), IndexError> {
        self.check_attempt(context)?;
        if self.rows.len() != self.ready.len() {
            return Err(IndexError::BackendViolation(
                "inventory ready rows lost cardinality".into(),
            ));
        }
        if self.rows.len() == 0 {
            if !self.occurrence_writes.pending.is_empty() {
                return Err(IndexError::BackendViolation(
                    "occurrence packet has no inventory owners".into(),
                ));
            }
            return Ok(());
        }
        if self
            .rows
            .as_slice()
            .iter()
            .any(|row| matches!(row.next, PendingTextNext::Preparing))
        {
            return Err(IndexError::BackendViolation(
                "inventory batch contains an unfinished owner".into(),
            ));
        }
        if self.occurrence_writes.pending.iter().any(|pending| {
            !usize::try_from(pending.ordinal).is_ok_and(|ordinal| ordinal < self.bases.len())
        }) {
            return Err(IndexError::BackendViolation(
                "occurrence packet lost its inventory ordinal".into(),
            ));
        }
        // File boundaries do not end a physical packet. Confirm the bounded
        // residual before a mixed-root cursor can expose any of its leaves.
        if !self.occurrence_writes.pending.is_empty() {
            let diagnostic = context.budget.request().diagnostic_context().cloned();
            observe_attempt(
                context,
                diagnostic.as_ref(),
                quanta_index_core::DiagnosticStageV1::NativeTextInventoryPrepare,
                |_child, context| {
                    self.occurrence_writes.flush(
                        context,
                        spool,
                        &mut inventories.occurrences,
                        &mut self.inventory_edits,
                    )
                },
            )?;
        }
        // Mixed-root SQL rows must exist before either root cursor reads them.
        self.inventory_edits.flush(spool, context.budget)?;
        let roots = if self.bases.len() == 0 {
            None
        } else {
            let diagnostic = context.budget.request().diagnostic_context().cloned();
            Some(observe_attempt(
                context,
                diagnostic.as_ref(),
                quanta_index_core::DiagnosticStageV1::NativeTextInventoryMapApply,
                |_child, context| {
                    let usage = if self.occurrence_writes.pack_owners {
                        entry_map::EntryMapUse::ColdInventory
                    } else {
                        entry_map::EntryMapUse::Inventory
                    };
                    let mut roots = entry_patch::apply(
                        usage,
                        self.bases.as_slice(),
                        self.counts.as_slice(),
                        &mut spool.inventory_cursor(self.bases.len())?,
                        |slot| spool.inventory_root_cursor(slot),
                        context,
                        None,
                    )?;
                    // The ordinary apply independently seals each staged
                    // anchor. Expand only after reading and proving the full
                    // immutable block, before any owner transition finishes.
                    for row in self.rows.as_slice() {
                        if let PendingTextNext::Draft {
                            slot,
                            alias: Some(alias),
                            ..
                        } = &row.next
                        {
                            if usage != entry_map::EntryMapUse::ColdInventory
                                || self.bases.as_slice().get(*slot) != Some(&EntryMap::default())
                            {
                                return Err(IndexError::BackendViolation(
                                    "inventory alias requires an empty cold base".into(),
                                ));
                            }
                            let root = roots.as_mut_slice().get_mut(*slot).ok_or_else(|| {
                                IndexError::BackendViolation(
                                    "inventory alias lost its ordinal".into(),
                                )
                            })?;
                            *root = alias.verify(root, context)?;
                        }
                    }
                    Ok(roots)
                },
            )?)
        };
        // All roots must be confirmed before any owner transition is completed.
        // Keep the retained root array alive while resolving its exact ordinals.
        for (row, ready) in self.rows.as_slice().iter().zip(self.ready.as_slice()) {
            context.budget.checkpoint()?;
            let (next, reused, verified) = match &row.next {
                PendingTextNext::Preparing => {
                    return Err(IndexError::BackendViolation(
                        "unfinished inventory owner".into(),
                    ));
                }
                PendingTextNext::Ready => (ready.contribution, ready.reused, ready.verified),
                PendingTextNext::Draft {
                    record,
                    slot,
                    body,
                    tokens,
                    ..
                } => {
                    let inventory = roots
                        .as_ref()
                        .and_then(|values| values.as_slice().get(*slot))
                        .copied()
                        .ok_or_else(|| {
                            IndexError::BackendViolation(
                                "inventory batch lost ordinal output".into(),
                            )
                        })?;
                    (
                        Some(Contribution {
                            record: *record,
                            inventory,
                            indexed_tokens: *tokens,
                        }),
                        false,
                        Some(VerifiedInventory {
                            bytes: body.bytes,
                            byte_len: body.byte_len,
                            inventory,
                            indexed_tokens: *tokens,
                        }),
                    )
                }
            };
            if let Some(value) = verified {
                inventories.remember_verified(&value, context.budget)?;
            }
            finish_change(
                &ConfirmedTextChange {
                    owner: row.owner,
                    prior_ref: row.prior_ref,
                    prior: row.prior,
                    next,
                    reused_inventory: reused,
                },
                root,
                context,
                spool,
                contributions,
                &mut self.posting_edits,
                &mut inventories.occurrences,
            )?;
        }
        flush_diff_posting_edits(&mut self.posting_edits, spool, context)?;
        if self.bases.len() != 0 {
            spool.reset_inventories(context.budget)?;
        }
        self.rows.clear();
        self.ready.clear();
        self.bases.clear();
        self.counts.clear();
        self.source_bytes = 0;
        Ok(())
    }

    fn push_change(
        &mut self,
        change: &quanta_index_core::index::SourceChange,
        root: &mut Root,
        context: &mut AttemptContext<'_>,
        spool: &TextSpool,
        contributions: &mut ContributionWrites,
        inventories: &mut InventoryReuse,
    ) -> Result<(), IndexError> {
        self.check_attempt(context)?;
        let recipe = inventories.recipe;
        let owner = owner_key(change.owner);
        let prior_source = change.prior.as_ref().map(|(reference, _)| *reference);
        let prior_admitted = change
            .prior
            .as_ref()
            .map(|(_, record)| check_admission(record, recipe))
            .transpose()?
            .unwrap_or(false);
        let prior_ref = context.maps.lookup(root.owners, &owner, context.budget)?;
        let prior = prior_ref
            .map(|reference| read_contribution(reference, context.objects, context.budget))
            .transpose()?;
        if prior_admitted != prior.is_some()
            || prior.is_some_and(|value| Some(value.record) != prior_source)
        {
            return Err(IndexError::CorruptObject(
                "native text prior contribution differs from source owner".into(),
            ));
        }
        let next = match change.next.as_ref() {
            Some((reference, record)) if check_admission(record, recipe)? => {
                Some((*reference, &**record))
            }
            Some(_) | None => None,
        };
        if let Some((reference, record)) = next
            && prior.is_some_and(|value| value.record == reference)
        {
            if change
                .prior
                .as_ref()
                .is_some_and(|(_, old)| &**old != record)
            {
                return Err(IndexError::CorruptObject(
                    "native text identical source reference has different metadata".into(),
                ));
            }
            return Ok(());
        }
        if next.is_none() && prior.is_none() {
            return Ok(());
        }
        if self.rows.len() == self.limit {
            self.flush(root, context, spool, contributions, inventories)?;
        }
        let reuse = next.is_some_and(|(_, record)| {
            change
                .prior
                .as_ref()
                .zip(prior)
                .is_some_and(|((_, old), _)| {
                    old.bytes == record.bytes
                        && old.byte_len == record.byte_len
                        && old.text == record.text
                })
        });
        let mut cached = if reuse {
            None
        } else {
            next.map(|(_, record)| inventories.lookup(record, context.budget))
                .transpose()?
                .flatten()
        };
        if let Some((_, record)) = next
            && !reuse
            && cached.is_none()
        {
            let body = TextBodyKey {
                bytes: record.bytes,
                byte_len: record.byte_len,
            };
            let repeated =
                inventories
                    .entries
                    .as_ref()
                    .is_some_and(|entries| entries.len() < inventories.limit)
                    && self.rows.as_slice().iter().zip(self.ready.as_slice()).any(
                        |(row, ready)| match &row.next {
                            PendingTextNext::Draft { body: pending, .. } => *pending == body,
                            PendingTextNext::Ready => ready.verified.is_some_and(|value| {
                                value.bytes == body.bytes && value.byte_len == body.byte_len
                            }),
                            PendingTextNext::Preparing => false,
                        },
                    );
            let total = self
                .source_bytes
                .checked_add(record.byte_len)
                .ok_or(IndexError::ResourceLimited)?;
            if repeated || (self.rows.len() != 0 && total > MAX_SOURCE_BYTES) {
                self.flush(root, context, spool, contributions, inventories)?;
                cached = inventories.lookup(record, context.budget)?;
            }
        }
        // Fund the queue slot before token preparation or any leaf installation.
        let slot = self.rows.len();
        self.rows.push(
            PendingTextChange {
                owner,
                prior_ref,
                prior,
                next: PendingTextNext::Preparing,
            },
            context.budget,
        )?;
        self.ready.push(ReadyTextNext::default(), context.budget)?;
        self.attempt = Some(context.attempt);
        let prepared = if let Some((reference, record)) = next {
            if reuse {
                let old = prior.as_ref().ok_or_else(|| {
                    IndexError::BackendViolation("reusable inventory missing".into())
                })?;
                self.set_ready(
                    slot,
                    &ReadyTextNext {
                        contribution: Some(Contribution {
                            record: reference,
                            ..*old
                        }),
                        reused: true,
                        verified: None,
                    },
                )?
            } else if let Some((inventory, indexed_tokens)) = cached {
                self.set_ready(
                    slot,
                    &ReadyTextNext {
                        contribution: Some(Contribution {
                            record: reference,
                            inventory,
                            indexed_tokens,
                        }),
                        reused: false,
                        verified: None,
                    },
                )?
            } else {
                let base = prior.map_or_else(EntryMap::default, |value| value.inventory);
                let ordinal = self.bases.len();
                self.bases.push(base, context.budget)?;
                self.counts.push(0, context.budget)?;
                let diagnostic = context.budget.request().diagnostic_context().cloned();
                let PreparedInventory {
                    changed,
                    indexed_tokens: tokens,
                    term_count,
                    alias,
                } = observe_attempt(
                    context,
                    diagnostic.as_ref(),
                    quanta_index_core::DiagnosticStageV1::NativeTextInventoryPrepare,
                    |_child, context| {
                        let body = source_body(record, context)?;
                        let text = std::str::from_utf8(&body.payload).map_err(|_error| {
                            IndexError::CorruptObject(
                                "text-admitted source body is not UTF-8".into(),
                            )
                        })?;
                        self.inventory_edits.ordinal =
                            u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?;
                        prepare_inventory(
                            text,
                            &base,
                            u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?,
                            context,
                            spool,
                            &mut inventories.occurrences,
                            &mut self.inventory_edits,
                            &mut self.occurrence_writes,
                        )
                    },
                )?;
                self.source_bytes = self
                    .source_bytes
                    .checked_add(record.byte_len)
                    .ok_or(IndexError::ResourceLimited)?;
                if changed {
                    *self
                        .counts
                        .as_mut_slice()
                        .get_mut(ordinal)
                        .ok_or(IndexError::ResourceLimited)? =
                        if alias.is_some() { 1 } else { term_count };
                    PendingTextNext::Draft {
                        record: reference,
                        slot: ordinal,
                        body: TextBodyKey {
                            bytes: record.bytes,
                            byte_len: record.byte_len,
                        },
                        tokens,
                        alias,
                    }
                } else {
                    let _count = self.counts.pop().ok_or_else(|| {
                        IndexError::BackendViolation("inventory draft count missing".into())
                    })?;
                    let _base = self.bases.pop().ok_or_else(|| {
                        IndexError::BackendViolation("inventory draft base missing".into())
                    })?;
                    self.set_ready(
                        slot,
                        &ReadyTextNext {
                            contribution: Some(Contribution {
                                record: reference,
                                inventory: base,
                                indexed_tokens: tokens,
                            }),
                            reused: false,
                            verified: Some(VerifiedInventory {
                                bytes: record.bytes,
                                byte_len: record.byte_len,
                                inventory: base,
                                indexed_tokens: tokens,
                            }),
                        },
                    )?
                }
            }
        } else {
            self.set_ready(
                slot,
                &ReadyTextNext {
                    contribution: None,
                    reused: false,
                    verified: None,
                },
            )?
        };
        self.rows
            .as_mut_slice()
            .get_mut(slot)
            .ok_or_else(|| IndexError::BackendViolation("inventory queue slot missing".into()))?
            .next = prepared;
        if self.rows.len() == self.limit {
            self.flush(root, context, spool, contributions, inventories)?;
        }
        Ok(())
    }
}

#[cfg(test)]
fn process_change(
    change: &quanta_index_core::index::SourceChange,
    root: &mut Root,
    recipe: TextRecipe,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    contributions: &mut ContributionWrites,
    inventories: &mut InventoryReuse,
) -> Result<(), IndexError> {
    if recipe != inventories.recipe {
        return Err(IndexError::Conflict);
    }
    let mut batch = InventoryBatch::new(1)?;
    batch.push_change(change, root, context, spool, contributions, inventories)?;
    batch.flush(root, context, spool, contributions, inventories)
}

/// Apply one confirmed owner transition in original source-cursor order.
/// Scratch edits remain disposable; the caller publishes only a complete root.
fn finish_change(
    change: &ConfirmedTextChange,
    root: &mut Root,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    contributions: &mut ContributionWrites,
    posting_edits: &mut PostingEdits,
    confirmed: &mut ConfirmedOccurrences,
) -> Result<(), IndexError> {
    let ConfirmedTextChange {
        owner,
        prior_ref,
        prior,
        next,
        reused_inventory,
    } = change;
    if let Some(value) = next.as_ref() {
        contributions.push(*owner, *prior_ref, value, context, spool)?;
    }
    if *reused_inventory {
        // This path trusts a previously published NativeText contribution from
        // the same recipe and its immutable inventory root. It does not scrub
        // physical source bytes or revalidate every term on each rename. Catalog
        // writers that can forge native members outside this builder are unsupported.
        let old = prior.as_ref().ok_or_else(|| {
            IndexError::BackendViolation("native text reusable prior missing".into())
        })?;
        let new = next.as_ref().ok_or_else(|| {
            IndexError::BackendViolation("native text reusable next missing".into())
        })?;
        validate_reused_inventory(old, new)?;
    } else {
        diff_inventory(
            owner,
            prior.as_ref(),
            next.as_ref(),
            context,
            spool,
            posting_edits,
            confirmed,
        )?;
    }
    root.docs = adjust(
        root.docs,
        u64::from(prior.is_some()),
        u64::from(next.is_some()),
    )?;
    root.indexed_tokens = adjust(
        root.indexed_tokens,
        prior.map_or(0, |value| value.indexed_tokens),
        next.map_or(0, |value| value.indexed_tokens),
    )?;
    if next.is_none() && prior_ref.is_some() {
        let owner = PreparedKey::copy(owner, context.budget)?;
        let (owner, owner_memory) = owner.into_parts();
        let edits = [(
            1,
            MapEdit {
                key: owner,
                expected: *prior_ref,
                next: None,
            },
        )];
        spool.top_batch(&edits, context.budget)?;
        drop(edits);
        drop(owner_memory);
    }
    Ok(())
}

fn inline_posting(
    value: &Posting,
) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
    value.root.validate_for(entry_map::EntryMapUse::Posting)?;
    if let Some(small) = value.root.inline_map() {
        if small.len() == 1 && value.df == 1 {
            let (key, child) = small.entry(0)?.ok_or_else(|| {
                IndexError::CorruptObject("singleton posting entry absent".into())
            })?;
            return quanta_index_core::index::MapValue::inline(key, Some(child)).map(Some);
        }
        Ok(None)
    } else if let EntryMap::PostingPages(child) = value.root {
        let mut data = [DENSE_POSTING_TAG; 9];
        data.get_mut(1..)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(&value.df.to_be_bytes());
        MapValue::inline(&data, Some(child)).map(Some)
    } else {
        let child = value
            .root
            .tree_base()
            .0
            .ok_or_else(|| IndexError::CorruptObject("posting tree root absent".into()))?;
        quanta_index_core::index::MapValue::inline(&value.df.to_be_bytes(), Some(child)).map(Some)
    }
}

/// Attribute a bounded text step without changing the work grant or outcome.
/// Off executes the original closure directly, without clock or context work.
fn observe_text_step<T>(
    diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    stage: DiagnosticStageV1,
    lane: u16,
    context: &mut AttemptContext<'_>,
    operation: impl FnOnce(&mut AttemptContext<'_>) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let Some(diagnostic) = diagnostic else {
        return operation(context);
    };
    let span = match diagnostic.enter(stage, quanta_index_core::DiagnosticLaneV1(lane)) {
        Ok(span) => span,
        Err(error) => {
            diagnostic.health(match error {
                quanta_index_core::DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                    quanta_index_core::DiagnosticHealthCodeV1::CounterOverflow
                }
                quanta_index_core::DiagnosticTraceErrorV1::ClockRegressed => {
                    quanta_index_core::DiagnosticHealthCodeV1::ClockRegressed
                }
                quanta_index_core::DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                    quanta_index_core::DiagnosticHealthCodeV1::ForeignCaptureSpan
                }
                quanta_index_core::DiagnosticTraceErrorV1::MissingParentSpan => {
                    quanta_index_core::DiagnosticHealthCodeV1::MissingParentSpan
                }
            });
            return operation(context);
        }
    };
    let child = diagnostic.child_context(&span);
    // Per-object read/edit spans remain time-only. Postings has only bounded
    // page steps, where work deltas distinguish map build from wrapper writes.
    let before = (stage == DiagnosticStageV1::NativeTextPostings).then(|| context.budget.usage());
    let result = {
        let mut scoped_budget = context.budget.scoped_diagnostic_context(Some(child));
        let mut scoped_context = AttemptContext {
            attempt: context.attempt,
            objects: context.objects,
            maps: context.maps,
            custody: context.custody,
            scratch: context.scratch,
            budget: &mut scoped_budget,
        };
        let result = operation(&mut scoped_context);
        if let Some(before) = before {
            scoped_context
                .budget
                .record_diagnostic_usage_delta(before, &span);
        }
        result
    };
    let _finished = span.finish(quanta_index_core::DiagnosticTerminalV1::from_result(
        &result,
    ));
    result
}

fn apply_postings(
    root: &mut Root,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
) -> Result<(), IndexError> {
    // With no prior terms, identical edits from the two case surfaces can use
    // the storage provider's existing same-batch root sharing.
    let mut cold_pages = root.terms.0.is_none().then(PostingCasePages::new);
    let mut after = None;
    loop {
        let terms = observe_text_step(
            diagnostic,
            DiagnosticStageV1::NativeTextPostings,
            35,
            context,
            |context| {
                if let Some(pages) = cold_pages.as_mut() {
                    pages.next(spool, context.budget)
                } else {
                    spool.posting_term_page(
                        after.as_ref().map(PreparedKey::as_slice),
                        context.budget,
                    )
                }
            },
        )?;
        if terms.is_empty() {
            break;
        }
        if cold_pages.is_none() {
            after = terms
                .last()
                .map(|key| PreparedKey::copy(key.as_slice(), context.budget))
                .transpose()?;
        }
        let length = terms.len();
        let per_row = [
            std::mem::size_of::<(PreparedKey, Option<quanta_index_core::index::MapValue>, u64)>(),
            std::mem::size_of::<EntryMap>(),
            std::mem::size_of::<u64>(),
            std::mem::size_of::<ObjectBatchInput>(),
            std::mem::size_of::<(PreparedKey, Option<quanta_index_core::index::MapValue>)>(),
            std::mem::size_of::<(u8, MapEdit)>(),
            std::mem::size_of::<[ObjectRef; 8]>(),
        ]
        .into_iter()
        .try_fold(0usize, |sum, bytes| {
            sum.checked_add(bytes).ok_or(IndexError::ResourceLimited)
        })?;
        let bytes = length
            .checked_mul(per_row)
            .ok_or(IndexError::ResourceLimited)?;
        // All bounded page arrays, plus at most eight children per wrapper, are
        // admitted before birth. Keys and JSON backing have separate charges.
        let array_memory = context.budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        let mut key_memory: [Option<quanta_index_core::index::MemoryLease>;
            quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS] = std::array::from_fn(|_| None);
        let mut payload_memory: [Option<quanta_index_core::index::MemoryLease>;
            quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS] = std::array::from_fn(|_| None);
        let mut metadata = Vec::new();
        let mut bases = Vec::new();
        let mut counts = Vec::new();
        let mut wrappers = Vec::new();
        let mut wrapper_keys = Vec::new();
        let mut top_rows = Vec::new();
        metadata
            .try_reserve_exact(length)
            .map_err(|_error| IndexError::ResourceLimited)?;
        bases
            .try_reserve_exact(length)
            .map_err(|_error| IndexError::ResourceLimited)?;
        counts
            .try_reserve_exact(length)
            .map_err(|_error| IndexError::ResourceLimited)?;
        wrappers
            .try_reserve_exact(length)
            .map_err(|_error| IndexError::ResourceLimited)?;
        wrapper_keys
            .try_reserve_exact(length)
            .map_err(|_error| IndexError::ResourceLimited)?;
        top_rows
            .try_reserve_exact(length)
            .map_err(|_error| IndexError::ResourceLimited)?;
        observe_text_step(
            diagnostic,
            DiagnosticStageV1::NativeTextPostings,
            35,
            context,
            |context| {
                let lookup_bytes = length
                    .checked_mul(
                        std::mem::size_of::<Option<MapValue>>() + std::mem::size_of::<&[u8]>(),
                    )
                    .ok_or(IndexError::ResourceLimited)?;
                let _lookup_memory = context.budget.reserve_memory(
                    u64::try_from(lookup_bytes).map_err(|_error| IndexError::ResourceLimited)?,
                    quanta_index_core::index::MemoryLifetime::Temporary,
                )?;
                let mut prior_values = Vec::new();
                prior_values
                    .try_reserve_exact(length)
                    .map_err(|_error| IndexError::ResourceLimited)?;
                prior_values.resize(length, None);
                let mut keys = Vec::new();
                keys.try_reserve_exact(length)
                    .map_err(|_error| IndexError::ResourceLimited)?;
                for term in terms.iter() {
                    check_term_key(term.as_slice())?;
                    keys.push(term.as_slice());
                }
                // Top edits stay in the spool until all pages are complete.
                // Cold pages have no prior term map or keys to traverse.
                if root.terms.0.is_some() {
                    context.maps.lookup_batch(
                        root.terms,
                        &keys,
                        &mut prior_values,
                        context.budget,
                    )?;
                }
                for (term, old_wrapper) in keys.into_iter().zip(prior_values) {
                    context.budget.checkpoint()?;
                    let prior = old_wrapper
                        .map(|reference| read_posting(reference, context.objects, context.budget))
                        .transpose()?;
                    metadata.push((
                        PreparedKey::copy(term, context.budget)?,
                        old_wrapper,
                        prior
                            .map(|posting| posting.checked_df(context.objects, context.budget))
                            .transpose()?
                            .unwrap_or(0),
                    ));
                    bases.push(prior.map_or_else(EntryMap::default, |posting| posting.root));
                }
                Ok(())
            },
        )?;
        // Authenticate every base and key before reading page-wide deltas.
        // The scratch SQL scope ends before the independent edit stream runs.
        let cardinalities = observe_text_step(
            diagnostic,
            DiagnosticStageV1::NativeTextPostings,
            36,
            context,
            |context| spool.posting_cardinalities(&terms, context.budget),
        )?;
        for (slot, (_, _, df)) in metadata.iter_mut().enumerate() {
            *df = cardinalities.df(slot, *df)?;
            counts.push(*df);
        }
        drop(cardinalities);
        let roots = observe_text_step(
            diagnostic,
            DiagnosticStageV1::NativeTextPostings,
            37,
            context,
            |context| {
                // Only an empty base bounds the edit count by the final DF.
                // A dense posting shrinking to Small can still have many deletes.
                let mut sparse = [false; quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];
                let sparse = sparse
                    .get_mut(..bases.len())
                    .ok_or(IndexError::ResourceLimited)?;
                let inline_entries = u64::try_from(entry_map::INLINE_ENTRIES)
                    .map_err(|_error| IndexError::ResourceLimited)?;
                let empty = EntryMap::default();
                for ((selected, base), count) in sparse.iter_mut().zip(&bases).zip(&counts) {
                    *selected = *count <= inline_entries && *base == empty;
                }
                let mut root_cursors =
                    spool.posting_root_cursors(&terms, sparse, context.budget)?;
                let maps = context.maps;
                let objects = context.objects;
                let prior_owners = root.owners;
                let mut resolve_prior_posting =
                    |slot: usize,
                     owner_key: &[u8],
                     witness: OccurrenceDigestWitness,
                     budget: &mut WorkBudget| {
                        let _owner = decode_posting_owner(owner_key)?;
                        let term = metadata
                            .get(slot)
                            .ok_or(IndexError::ResourceLimited)?
                            .0
                            .as_slice();
                        let contribution_ref = maps
                            .lookup(prior_owners, owner_key, budget)?
                            .ok_or_else(|| {
                                IndexError::CorruptObject(
                                    "posting Tree lacks prior owner contribution".into(),
                                )
                            })?;
                        let contribution = read_contribution(contribution_ref, objects, budget)?;
                        let occurrence_ref = contribution
                            .inventory
                            .lookup(objects, maps, term, budget)?
                            .ok_or_else(|| {
                                IndexError::CorruptObject(
                                    "posting Tree lacks prior owner term inventory".into(),
                                )
                            })?;
                        if !witness.matches(occurrence_ref) {
                            return Err(IndexError::CorruptObject(
                                "posting Tree differs from prior owner inventory".into(),
                            ));
                        }
                        Ok(occurrence_ref)
                    };
                entry_patch::apply(
                    entry_map::EntryMapUse::PackedPosting,
                    &bases,
                    &counts,
                    &mut spool.posting_cursor(terms),
                    |slot| {
                        root_cursors.cursor(
                            slot,
                            metadata
                                .get(slot)
                                .ok_or(IndexError::ResourceLimited)?
                                .0
                                .as_slice(),
                        )
                    },
                    context,
                    Some(&mut resolve_prior_posting),
                )
            },
        )?;
        if roots.len() != metadata.len() {
            return Err(IndexError::BackendViolation(
                "native text posting batch lost ordinal output".into(),
            ));
        }
        observe_text_step(
            diagnostic,
            DiagnosticStageV1::NativeTextPostings,
            38,
            context,
            |context| {
                for ((term, old_wrapper, df), new_root) in
                    metadata.into_iter().zip(roots.as_slice().iter().copied())
                {
                    if df != 0 {
                        if let Some(value) = inline_posting(&Posting { root: new_root, df })? {
                            if old_wrapper != Some(value) {
                                let custody = key_memory
                                    .get_mut(top_rows.len())
                                    .ok_or(IndexError::ResourceLimited)?;
                                let (key, grant) = term.into_parts();
                                *custody = grant;
                                top_rows.push((
                                    0,
                                    MapEdit {
                                        key,
                                        expected: old_wrapper,
                                        next: Some(value),
                                    },
                                ));
                            }
                            continue;
                        }
                        let payload = super::text_memory::encode(
                            &Posting { root: new_root, df },
                            context.budget,
                        )?;
                        let children = posting_children(&Posting { root: new_root, df })?;
                        let identified = context.objects.identify(
                            payload.as_slice(),
                            children.as_slice(),
                            context.budget,
                        )?;
                        // Only top rows from earlier successful put_batch calls can hit.
                        // This prepare's original attempt retains custody; the final
                        // parent-map install still authenticates every registered child.
                        if spool.has_confirmed_posting(identified.reference, context.budget)? {
                            if old_wrapper != Some(identified.reference.into()) {
                                let custody =
                                    key_memory.get_mut(top_rows.len()).ok_or_else(|| {
                                        IndexError::BackendViolation(
                                            "posting reused key exceeds page".into(),
                                        )
                                    })?;
                                let (key, grant) = term.into_parts();
                                *custody = grant;
                                top_rows.push((
                                    0,
                                    MapEdit {
                                        key,
                                        expected: old_wrapper,
                                        next: Some((identified.reference).into()),
                                    },
                                ));
                            }
                            continue;
                        }
                        let custody = payload_memory.get_mut(wrappers.len()).ok_or_else(|| {
                            IndexError::BackendViolation(
                                "posting payload custody exceeds page".into(),
                            )
                        })?;
                        let (payload, grant) = payload.into_parts();
                        *custody = grant;
                        wrappers.push(ObjectBatchInput {
                            payload,
                            children: children.as_slice().to_vec(),
                        });
                        wrapper_keys.push((term, old_wrapper));
                    } else if old_wrapper.is_some() {
                        let custody = key_memory.get_mut(top_rows.len()).ok_or_else(|| {
                            IndexError::BackendViolation("posting key custody exceeds page".into())
                        })?;
                        let (key, grant) = term.into_parts();
                        *custody = grant;
                        top_rows.push((
                            0,
                            MapEdit {
                                key,
                                expected: old_wrapper,
                                next: None,
                            },
                        ));
                    }
                }
                Ok(())
            },
        )?;
        observe_text_step(
            diagnostic,
            DiagnosticStageV1::NativeTextPostings,
            39,
            context,
            |context| {
                let new_wrappers = context.put_batch(&wrappers)?;
                if new_wrappers.len() != wrapper_keys.len() {
                    return Err(IndexError::BackendViolation(
                        "native text wrapper batch lost ordinal output".into(),
                    ));
                }
                for ((term, old_wrapper), next) in wrapper_keys.into_iter().zip(new_wrappers) {
                    if old_wrapper != Some(next.into()) {
                        let custody = key_memory.get_mut(top_rows.len()).ok_or_else(|| {
                            IndexError::BackendViolation("posting key custody exceeds page".into())
                        })?;
                        let (key, grant) = term.into_parts();
                        *custody = grant;
                        top_rows.push((
                            0,
                            MapEdit {
                                key,
                                expected: old_wrapper,
                                next: Some((next).into()),
                            },
                        ));
                    }
                }
                if !top_rows.is_empty() {
                    spool.top_batch(&top_rows, context.budget)?;
                }
                Ok(())
            },
        )?;
        drop(top_rows);
        drop(wrappers);
        drop(bases);
        drop(counts);
        drop(roots);
        drop(payload_memory);
        drop(key_memory);
        drop(array_memory);
    }
    let roots = observe_text_step(
        diagnostic,
        DiagnosticStageV1::NativeTextPostings,
        39,
        context,
        |context| context.apply_stream(&[root.terms, root.owners], &mut spool.top_cursor()),
    )?;
    let mut roots = roots.into_iter();
    root.terms = roots
        .next()
        .ok_or_else(|| IndexError::BackendViolation("native text terms root missing".into()))?;
    root.owners = roots
        .next()
        .ok_or_else(|| IndexError::BackendViolation("native text owners root missing".into()))?;
    if roots.next().is_some() {
        return Err(IndexError::BackendViolation(
            "native text root batch returned extra output".into(),
        ));
    }
    Ok(())
}

/// Bind the budget seen by the actual map/object ports to the observed stage.
/// The scoped request restores on return or unwind; no authority or work changes.
fn observe_attempt<T>(
    context: &mut AttemptContext<'_>,
    diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    stage: quanta_index_core::DiagnosticStageV1,
    operation: impl FnOnce(
        Option<&quanta_index_core::DiagnosticContextV1>,
        &mut AttemptContext<'_>,
    ) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    observe_attempt_on_lane(
        context,
        diagnostic,
        stage,
        quanta_index_core::DiagnosticLaneV1(2),
        operation,
    )
}

fn observe_attempt_on_lane<T>(
    context: &mut AttemptContext<'_>,
    diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    stage: quanta_index_core::DiagnosticStageV1,
    lane: quanta_index_core::DiagnosticLaneV1,
    operation: impl FnOnce(
        Option<&quanta_index_core::DiagnosticContextV1>,
        &mut AttemptContext<'_>,
    ) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    // An explicit mask must still hide an enclosing diagnostic context.
    // Fully disabled tracing needs neither a scoped budget nor a request clone.
    if diagnostic.is_none() && context.budget.request().diagnostic_context().is_none() {
        return operation(None, context);
    }
    crate::causal_profile::observe_result_with_span_on_lane(
        diagnostic,
        stage,
        lane,
        |child, span| {
            let mut scoped_budget = context.budget.scoped_diagnostic_override(child.cloned());
            let before = span
                .filter(|_span| lane.0 == 2)
                .map(|_span| scoped_budget.usage());
            let mut scoped_context = AttemptContext {
                attempt: context.attempt,
                objects: context.objects,
                maps: context.maps,
                custody: context.custody,
                scratch: context.scratch,
                budget: &mut scoped_budget,
            };
            let result = operation(child, &mut scoped_context);
            if let (Some(before), Some(span)) = (before, span) {
                scoped_context
                    .budget
                    .record_diagnostic_usage_delta(before, span);
            }
            result
        },
    )
}

impl IndexBuildPort for NativeTextIndex {
    type Delta = SourceDelta;
    type Plan = TextPlan;
    type Candidate = TextCandidate;

    fn resource_demand(&self) -> quanta_index_core::index::effort::ResourceNeed {
        quanta_index_core::index::effort::ResourceNeed::default()
    }

    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        if instance.policy != self.recipe.output_policy()
            || instance.activation != quanta_index_core::index::GroupActivation::CoreRequired
            || instance.input_domain != quanta_index_core::index::InputDomain::SourceLocal
        {
            return Err(IndexError::InvalidInput(
                "native text instance has a different recipe or input domain".into(),
            ));
        }
        Ok(())
    }

    fn plan(
        &self,
        base: BaseMember,
        input: DeltaInput<SourceDelta>,
        context: &PlanContext,
        budget: &mut WorkBudget,
    ) -> Result<PlanDecision<TextPlan>, IndexError> {
        budget.checkpoint()?;
        self.validate_instance(&context.instance)?;
        if context.input.policy != context.instance.policy {
            return Err(IndexError::InvalidInput(
                "native text instance has a different recipe or input domain".into(),
            ));
        }
        let (base, input) = match (base, input) {
            (BaseMember::Absent, DeltaInput::Bootstrap)
                if context.input.root == context.source.owners =>
            {
                (None, DeltaInput::Bootstrap)
            }
            (BaseMember::Present(member), DeltaInput::Patch(delta)) => {
                if member.instance != context.instance.instance
                    || member.format != context.instance.format
                    || member.input.schema != context.input.schema
                    || member.input.policy != context.input.policy
                    || member.input.root != delta.prior.owners
                    || delta.next != context.source
                    || context.input.root != delta.next.owners
                {
                    return Err(IndexError::Conflict);
                }
                (Some(*member), DeltaInput::Patch(delta))
            }
            (
                BaseMember::Absent | BaseMember::Present(_),
                DeltaInput::Bootstrap | DeltaInput::Patch(_),
            ) => return Ok(PlanDecision::RebuildRequired),
        };
        Ok(PlanDecision::Incremental(TextPlan {
            base,
            context: context.clone(),
            input,
        }))
    }

    fn prepare<'a>(
        &'a self,
        plan: TextPlan,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, TextCandidate> {
        Box::pin(async move {
            let diagnostic = context.budget.request().diagnostic_context().cloned();
            observe_attempt(
                context,
                diagnostic.as_ref(),
                quanta_index_core::DiagnosticStageV1::LexicalBuild,
                |diagnostic, context| {
                    let mut root = plan
                        .base
                        .as_ref()
                        .map(|base| read_root(base, context.objects, context.budget))
                        .transpose()?
                        .unwrap_or_default();
                    if !matches!(&plan.input, DeltaInput::Bootstrap)
                        || plan.context.source.owners.0.is_some()
                    {
                        let mut changes = SourceInputCursor::new(
                            plan.context.source,
                            plan.input.clone(),
                            context.maps,
                            context.objects,
                        )?;
                        let mut next_change = changes.next_change(context.budget)?;
                        if next_change.is_none()
                            && let (Some(base), DeltaInput::Patch(delta)) =
                                (&plan.base, &plan.input)
                        {
                            if delta.prior.owners != delta.next.owners {
                                return Err(IndexError::CorruptObject(
                                    "empty native text patch changed owner root".into(),
                                ));
                            }
                            return Ok(TextCandidate {
                                binding: MemberBinding {
                                    instance: plan.context.instance.instance,
                                    format: plan.context.instance.format,
                                    artifact: base.artifact,
                                    input: plan.context.input,
                                    logical_commitment: base.logical_commitment,
                                },
                            });
                        }
                        // Reuse the canonical DATA reader only inside this one
                        // funded prepare. Empty/no-op paths allocate no window.
                        // The original source cursor keeps its original port.
                        let maps = context.maps;
                        let read_window = if next_change.is_some() {
                            maps.scoped_reads(context.budget)?
                        } else {
                            None
                        };
                        let mut build_context = AttemptContext {
                            attempt: context.attempt,
                            objects: context.objects,
                            maps: read_window.as_deref().unwrap_or(maps),
                            custody: context.custody,
                            scratch: context.scratch,
                            budget: &mut *context.budget,
                        };
                        let context = &mut build_context;
                        let spool = TextSpool::new(context.scratch, context.budget)?;
                        // Initial population also reaches this adapter as a
                        // Patch after publishing an empty profile. The prior
                        // source binding, rather than corpus size or tier,
                        // selects the same bounded construction strategy.
                        let cold = matches!(&plan.input, DeltaInput::Bootstrap)
                            || matches!(&plan.input, DeltaInput::Patch(delta)
                                if delta.prior.owners.0.is_none());
                        let mut contributions = if cold {
                            ContributionWrites::cold()?
                        } else {
                            ContributionWrites::new()?
                        };
                        let mut inventories =
                            InventoryReuse::new(self.recipe, self.inventory_reuse_limit)?;
                        let limit = self.inventory_batch_roots(cold);
                        let mut batch =
                            if cold && limit > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
                                InventoryBatch::cold(limit)?
                            } else {
                                InventoryBatch::new(limit)?
                            };
                        batch.occurrence_writes.pack_owners = cold;
                        if cold {
                            batch.posting_edits = PostingEdits::with_limit(
                                spool.posting_write_batch_rows(context.budget)?,
                            )?;
                        }
                        while let Some(change) = next_change {
                            context.budget.checkpoint()?;
                            batch.push_change(
                                &change,
                                &mut root,
                                context,
                                &spool,
                                &mut contributions,
                                &mut inventories,
                            )?;
                            next_change = changes.next_change(context.budget)?;
                        }
                        batch.flush(
                            &mut root,
                            context,
                            &spool,
                            &mut contributions,
                            &mut inventories,
                        )?;
                        contributions.flush(context, &spool)?;
                        observe_attempt(
                            context,
                            diagnostic,
                            quanta_index_core::DiagnosticStageV1::NativeTextPostings,
                            |child, context| apply_postings(&mut root, context, &spool, child),
                        )?;
                    }
                    let (artifact, logical_commitment) = observe_attempt(
                        context,
                        diagnostic,
                        quanta_index_core::DiagnosticStageV1::NativeTextRoot,
                        |_child, context| write_root(root, context),
                    )?;
                    Ok(TextCandidate {
                        binding: MemberBinding {
                            instance: plan.context.instance.instance,
                            format: plan.context.instance.format,
                            artifact,
                            input: plan.context.input,
                            logical_commitment,
                        },
                    })
                },
            )
        })
    }

    fn verify_and_seal<'a>(
        &'a self,
        candidate: TextCandidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        Box::pin(async move {
            let _root = read_root(&candidate.binding, context.objects, context.budget)?;
            Ok(candidate.binding)
        })
    }
}

impl NativeTextIndex {
    /// Check a saved owner against the actual indexed population, including
    /// admission and exact source identity. Excluded owners remain overlays.
    pub(super) fn validate_searchable_owner(
        &self,
        member: &MemberBinding,
        owner: OwnerKey,
        source: Option<ObjectRef>,
        mut context: TextReadContext<'_>,
    ) -> Result<(), IndexError> {
        let root = self.pinned_root(member, &mut context)?;
        let selected = context.maps.lookup_object(
            context.ticket.view.source.owners,
            &owner_map_key(owner),
            context.budget,
        )?;
        if selected != source {
            return Err(IndexError::Conflict);
        }
        let key = owner_key(owner);
        let contribution = context.maps.lookup(root.owners, &key, context.budget)?;
        match (source, contribution) {
            (None, None) => Ok(()),
            (Some(source), Some(reference)) => {
                let indexed =
                    validate_universe_owner(&key, reference, root, self.recipe, &mut context)?;
                if indexed.source_record != source {
                    return Err(IndexError::CorruptObject(
                        "saved text contribution differs from source".into(),
                    ));
                }
                Ok(())
            }
            (Some(_), None) => Err(IndexError::SnapshotUnavailable),
            (None, Some(_)) => Err(IndexError::CorruptObject(
                "deleted source remains in text member".into(),
            )),
        }
    }

    pub(super) fn pinned_root(
        &self,
        member: &MemberBinding,
        context: &mut TextReadContext<'_>,
    ) -> Result<Root, IndexError> {
        if member.input.policy != self.recipe.output_policy()
            || member.input.root != context.ticket.view.source.owners
        {
            return Err(IndexError::Conflict);
        }
        let key = member_map_key(member.instance);
        let member_ref = context
            .maps
            .lookup_object(context.ticket.view.members, &key, context.budget)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let stored = context.objects.get(member_ref, context.budget)?;
        let expected_children = [Some(member.artifact), member.input.root.0];
        if stored.registration.reference != member_ref
            || !stored
                .registration
                .children
                .iter()
                .copied()
                .eq(expected_children.into_iter().flatten())
        {
            return Err(IndexError::CorruptObject(
                "native text member reference or edges mismatch".into(),
            ));
        }
        let published =
            quanta_index_core::index::decode_member_binding(&stored.payload, context.budget)?;
        if &published != member {
            return Err(IndexError::Conflict);
        }
        read_root(member, context.objects, context.budget)
    }

    /// Inspect exact live statistics from a pinned member. This is not a
    /// ranking or search API.
    pub fn inspect_stats(
        &self,
        member: &MemberBinding,
        mut context: TextReadContext<'_>,
    ) -> Result<TextStatistics, IndexError> {
        let root = self.pinned_root(member, &mut context)?;
        Ok(TextStatistics {
            docs: root.docs,
            indexed_tokens: root.indexed_tokens,
        })
    }

    /// Visit one canonical term's postings from an exactly pinned member.
    ///
    /// Callback effects are provisional: a later page, cancellation or
    /// cardinality error invalidates the whole traversal. Only the returned
    /// `TextTermScan` proves that every posting was checked. Callers must
    /// discard any accumulated candidates on `Err`.
    pub fn visit_term<F>(
        &self,
        member: &MemberBinding,
        case: CaseMode,
        term: &str,
        mut context: TextReadContext<'_>,
        mut visitor: F,
    ) -> Result<TextTermScan, IndexError>
    where
        F: for<'hit> FnMut(TextTermHit<'hit>) -> Result<(), IndexError>,
    {
        let mut cursor = self.posting_cursor(member, case, term, &mut context)?;
        let mut occurrences = OccurrenceReader::new(1)?;
        while let Some(hit) = cursor.next_hit(&mut context, &mut occurrences)? {
            visitor(TextTermHit {
                owner: hit.owner,
                source: hit.source_record,
                tf: u64::try_from(hit.occurrence.positions.len())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                doc_indexed_tokens: hit.doc_indexed_tokens,
                positions: &hit.occurrence.positions,
            })?;
        }
        Ok(cursor.statistics())
    }

    pub(super) fn posting_cursor(
        &self,
        member: &MemberBinding,
        case: CaseMode,
        term: &str,
        context: &mut TextReadContext<'_>,
    ) -> Result<TextPostingCursor, IndexError> {
        let key = admitted_query_term(term, case, context.budget)?;
        let root = self.pinned_root(member, context)?;
        self.posting_cursor_from_key(root, key, context)
    }

    pub(super) fn posting_cursor_from_key(
        &self,
        root: Root,
        key: PreparedKey,
        context: &mut TextReadContext<'_>,
    ) -> Result<TextPostingCursor, IndexError> {
        let posting = context
            .maps
            .lookup(root.terms, key.as_slice(), context.budget)?
            .map(|reference| read_posting(reference, context.objects, context.budget))
            .transpose()?;
        if posting.is_some_and(|posting| posting.df > root.docs) {
            return Err(IndexError::CorruptObject(
                "text document frequency exceeds corpus document count".into(),
            ));
        }
        Ok(TextPostingCursor {
            root,
            recipe: self.recipe,
            key,
            posting,
            after: None,
            entries: MapPageCursor::default(),
            page_lookups: None,
            more: posting.is_some(),
            visited: 0,
            dense: None,
        })
    }

    /// Inspect exact owner document frequency of one normalized query term.
    /// This does not execute BM25, phrase search or regex.
    pub fn inspect_term_df(
        &self,
        member: &MemberBinding,
        case: CaseMode,
        term: &str,
        mut context: TextReadContext<'_>,
    ) -> Result<u64, IndexError> {
        let key = admitted_query_term(term, case, context.budget)?;
        let root = self.pinned_root(member, &mut context)?;
        let reference = context
            .maps
            .lookup(root.terms, key.as_slice(), context.budget)?;
        reference
            .map(|reference| {
                read_posting(reference, context.objects, context.budget)?
                    .checked_df(context.objects, context.budget)
            })
            .transpose()
            .map(|df| df.unwrap_or(0))
    }
}
