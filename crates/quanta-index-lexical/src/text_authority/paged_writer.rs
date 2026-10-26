//! Publication of changed Native source bundles into immutable segments.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use quanta_index_contract::{ManifestGeneration, SourceFileKey};
use quanta_index_core::ingest_resource::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE;
use quanta_index_core::lexical_maintenance::{
    LexicalMaintenancePolicy, StagedPacketBounds, StagedPacketIdentity,
};
use quanta_index_core::{CoreError, RequestBudgetV1};
use sha2::{Digest as _, Sha256};

use crate::lexical_runs::{
    RunCandidate, RunSelectionProgress, SegmentInput, SegmentRemoval, bounded_candidate_scan_limit,
    segment_index_encoded_bytes,
};
use crate::lexical_store::{
    AttemptLease, Authority, BlobRef, CandidateWitness, EligibilityWitness, IneligibleReason,
    MAX_CLOSURE_REFS, MapEdit, MapRoot, ObjectKind, ObjectRef, ObjectStore, ScanLane,
    WindowWitness,
};
use crate::text_authority::paged_bootstrap::{BootstrapMap, NativeBootstrapMaps};
use crate::text_authority::paged_manifest::NativeRoot;
use crate::text_authority::paged_posting_page::{HEADER_BYTES, PostingPage, TARGET_BYTES};
use crate::text_authority::paged_segment::{
    BuiltPosting, BuiltSegment, GRAM_FOLDED, GRAM_RAW, TERM_FOLDED, TERM_RAW,
    compaction_scratch_limit, decode_body, decode_body_for_preparation,
};
use crate::text_authority::paged_wire::{
    CurrentRecord, MAX_BODY_BYTES, MAX_POSTING_BYTES, NativeSourceEdit, SEGMENT_DOCS,
    SegmentDescriptor, current_key, decode_source, encode_source, gram_prefix, membership_key,
    prefix_upper, route_key, segment_key, source_digest, term_prefix,
};
use crate::text_authority::types::{AddedTextDoc, TextAuthorityWriteReceipt};

const SEGMENT_TARGET_TEXT_BYTES: usize = 2 * 1024 * 1024;
const MAX_CURRENT_RECORD_BYTES: u64 = 32 * 1024;
const MAX_SOURCE_RECORD_BYTES: u64 = 64 * 1024;

impl NativeRoot {
    /// A maintenance-only no-op may persist bounded scan progress without
    /// changing any source, posting, body, membership, or run authority. The
    /// independent seal re-derives the cursor from the prior authenticated
    /// run graph; this projection only gates the hard-limit exception.
    pub(crate) fn is_cursor_only_successor_of(&self, prior: &Self) -> Result<bool, CoreError> {
        self.validate()?;
        prior.validate()?;
        if self.run_registry.selection_progress() == prior.run_registry.selection_progress()
            || !self.run_registry.same_authority_as(&prior.run_registry)
        {
            return Ok(false);
        }
        let mut projected = self.clone();
        projected.run_registry = prior.run_registry.clone();
        Ok(projected == *prior)
    }
}

/// Request-local preparation. No object-store reference is published until a
/// target lease binds the derived relations to its own segment ID.
pub(crate) struct PreparedNativeCompaction {
    selected: Vec<(u64, BlobRef, SegmentDescriptor)>,
    current_witnesses: Vec<(u64, BlobRef, BlobRef)>,
    docs: Vec<AddedTextDoc>,
    staging_charged_bytes: u64,
    policy_binding: Option<NativePacketBinding>,
}

struct NativePacketBinding {
    identity: StagedPacketIdentity,
    bounds: StagedPacketBounds,
    created_at: Instant,
}

impl PreparedNativeCompaction {
    /// Bind a completed optional packet to its exact old root and service
    /// policy. The policy check is repeated immediately before adoption.
    pub(crate) fn bind_policy(
        mut self,
        base: &NativeRoot,
        policy: &LexicalMaintenancePolicy,
    ) -> Result<Self, CoreError> {
        let identity =
            StagedPacketIdentity::new(native_root_sha256(base)?, policy.policy_sha256())?;
        self.policy_binding = Some(NativePacketBinding {
            identity,
            bounds: StagedPacketBounds {
                encoded_bytes: self.staging_charged_bytes,
                created_at_monotonic_millis: 0,
                input_runs: u32::try_from(self.selected.len()).map_err(|_error| {
                    CoreError::InvalidContract("lexical: Native staged fan-in exceeds u32".into())
                })?,
            },
            created_at: Instant::now(),
        });
        Ok(self)
    }

    pub(crate) fn policy_valid(
        &self,
        base: Option<&NativeRoot>,
        policy: Option<&LexicalMaintenancePolicy>,
    ) -> Result<bool, CoreError> {
        match (base, policy, &self.policy_binding) {
            (Some(base), Some(policy), Some(binding)) => {
                if binding.bounds.encoded_bytes != self.staging_charged_bytes
                    || binding.bounds.input_runs
                        != u32::try_from(self.selected.len()).map_err(|_error| {
                            CoreError::InvalidContract(
                                "lexical: Native staged fan-in exceeds u32".into(),
                            )
                        })?
                {
                    return Ok(false);
                }
                let age =
                    u64::try_from(binding.created_at.elapsed().as_millis()).map_err(|_error| {
                        CoreError::InvalidContract("lexical: Native staged age exceeds u64".into())
                    })?;
                Ok(policy
                    .check_staged_packet(
                        binding.identity,
                        native_root_sha256(base)?,
                        binding.bounds,
                        age,
                    )
                    .is_ok())
            }
            (_, None, None) => Ok(true),
            _ => Ok(false),
        }
    }
}

fn native_root_sha256(root: &NativeRoot) -> Result<[u8; 32], CoreError> {
    root.validate()?;
    let bytes = crate::channel_payloads::encode_cbor(root, "Native staged base root")?;
    Ok(Sha256::digest(&bytes).into())
}

struct ScratchAdmission {
    limit: u64,
    charged: u64,
}

struct PreparedCleanMerge {
    selected: Vec<(BlobRef, SegmentDescriptor)>,
    docs: Vec<AddedTextDoc>,
    text_bytes: u64,
    charged: u64,
}

#[cfg(test)]
thread_local! {
    static CLEAN_MERGE_PREPARE_CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn clean_merge_scratch_refusal() -> CoreError {
    CoreError::Typed {
        code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
        message: "lexical: Native clean-run merge body scratch exceeds configured input allowance"
            .into(),
    }
}

/// Re-derive the exact logical staging charge from authenticated descriptor,
/// current, and body references.
///
/// This never reads or allocates a body document;
/// selected cohorts undergo full body preparation before mutation.
fn probe_clean_cohort_metadata(
    store: &ObjectStore,
    base: &NativeRoot,
    candidate: &RunCandidate,
    touched: &BTreeSet<u64>,
    policy: &LexicalMaintenancePolicy,
    budget: Option<&RequestBudgetV1>,
) -> Result<EligibilityWitness, CoreError> {
    if candidate.index_encoded_bytes > policy.max_merge_input_encoded_bytes() {
        return Ok(EligibilityWitness::Ineligible(IneligibleReason::InputCap));
    }
    if candidate.members.entries == 0 {
        return Ok(EligibilityWitness::Ineligible(
            IneligibleReason::EmptyCohort,
        ));
    }
    let mut charged = 0_u64;
    let mut refusal = None;
    let mut staging_cap = false;
    base.run_registry.visit_run_members(
        store,
        candidate,
        budget,
        &mut |key, reference, _bytes| {
            if refusal.is_some() {
                return Ok(());
            }
            if let Some(budget) = budget {
                budget.checkpoint("lexical:native:probe-clean-member")?;
            }
            let id = u64::from_be_bytes(key.try_into().map_err(|error| {
                CoreError::InvalidContract(format!(
                    "lexical: clean probe member key width: {error}"
                ))
            })?);
            if touched.contains(&id) || store.map_get(&base.compaction_due, key, budget)?.is_some()
            {
                refusal = Some(IneligibleReason::TouchedOrDebt);
                return Ok(());
            }
            if store.map_get(&base.segments, key, budget)?.as_ref() != Some(reference) {
                refusal = Some(IneligibleReason::RetiredDueOrStale);
                return Ok(());
            }
            let descriptor = read_segment_budget(store, base, id, budget)?;
            if descriptor.live_locals.len() != descriptor.logical_ids.len() {
                refusal = Some(IneligibleReason::PartialSegment);
                return Ok(());
            }
            let doc_slots = u64::try_from(descriptor.live_locals.len())
                .map_err(|error| {
                    CoreError::InvalidContract(format!("lexical: clean probe slot count: {error}"))
                })?
                .checked_mul(u64::try_from(std::mem::size_of::<AddedTextDoc>()).map_err(
                    |error| {
                        CoreError::InvalidContract(format!(
                            "lexical: clean probe slot width: {error}"
                        ))
                    },
                )?)
                .ok_or_else(|| {
                    CoreError::InvalidContract("lexical: clean probe slot charge overflows".into())
                })?;
            let descriptor_charge = reference.bytes.checked_mul(2).ok_or_else(|| {
                CoreError::InvalidContract(
                    "lexical: clean probe descriptor charge overflows".into(),
                )
            })?;
            charged = charged
                .checked_add(descriptor_charge)
                .and_then(|value| value.checked_add(doc_slots))
                .and_then(|value| value.checked_add(doc_slots))
                .ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical: clean probe staging charge overflows".into(),
                    )
                })?;
            if charged > policy.max_staging_bytes() {
                staging_cap = true;
            }
            for local in &descriptor.live_locals {
                if let Some(budget) = budget {
                    budget.checkpoint("lexical:native:probe-clean-current")?;
                }
                let logical_id = *descriptor
                    .logical_ids
                    .get(usize::try_from(*local).map_err(|error| {
                        CoreError::InvalidContract(format!(
                            "lexical: clean probe local width: {error}"
                        ))
                    })?)
                    .ok_or_else(|| {
                        CoreError::InvalidContract("lexical: clean probe local absent".into())
                    })?;
                let (current_ref, current) =
                    read_current_with_ref_budget(store, base, logical_id, budget)?.ok_or_else(
                        || CoreError::InvalidContract("lexical: clean probe current absent".into()),
                    )?;
                if current.segment_key != descriptor.id
                    || current.local_id != *local
                    || current.body.root.kind != ObjectKind::NativeBody
                    || current.body.bytes > MAX_BODY_BYTES
                {
                    return Err(CoreError::InvalidContract(
                        "lexical: clean probe current locator differs".into(),
                    ));
                }
                let member = store.map_get(
                    &base.membership,
                    &membership_key(&source_digest(&current.source)?, logical_id),
                    budget,
                )?;
                if member.as_ref() != Some(&current_ref) {
                    return Err(CoreError::InvalidContract(
                        "lexical: clean probe membership differs".into(),
                    ));
                }
                let body_charge = current.body.bytes.checked_mul(2).ok_or_else(|| {
                    CoreError::InvalidContract("lexical: clean probe body charge overflows".into())
                })?;
                charged = charged
                    .checked_add(current_ref.bytes)
                    .and_then(|value| value.checked_add(body_charge))
                    .ok_or_else(|| {
                        CoreError::InvalidContract(
                            "lexical: clean probe staging charge overflows".into(),
                        )
                    })?;
                if charged > policy.max_staging_bytes() {
                    staging_cap = true;
                }
            }
            Ok(())
        },
    )?;
    if let Some(reason) = refusal {
        return Ok(EligibilityWitness::Ineligible(reason));
    }
    if staging_cap {
        return Ok(EligibilityWitness::Ineligible(IneligibleReason::StagingCap));
    }
    Ok(EligibilityWitness::NativeEligible {
        effective_input_encoded_bytes: candidate.index_encoded_bytes,
        staging_bytes: charged,
    })
}

fn native_candidate_header_sha256(
    store: &ObjectStore,
    base: &NativeRoot,
    candidate: &RunCandidate,
    budget: Option<&RequestBudgetV1>,
) -> Result<[u8; 32], CoreError> {
    let reference = store
        .map_get(
            &base.run_registry.runs,
            &candidate.run_id.to_be_bytes(),
            budget,
        )?
        .ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native hard scan header absent".into())
        })?;
    let membership =
        crate::channel_payloads::encode_cbor(&candidate.members, "Native hard scan membership")?;
    let mut hasher = Sha256::new();
    hasher.update(b"native-hard-scan-header-v1");
    hasher.update(reference.root.sha256);
    hasher.update(candidate.run_id.to_be_bytes());
    hasher.update(candidate.index_encoded_bytes.to_be_bytes());
    hasher.update(&membership);
    Ok(hasher.finalize().into())
}

/// Observe one immutable, at-most-32-header Native window before publication.
/// The checkpoint owner supplies ordinal and chain fields after this read.
pub(crate) fn prepare_hard_scan_window(
    store: &ObjectStore,
    base: &NativeRoot,
    retired: &[u64],
    policy: &LexicalMaintenancePolicy,
    progress: RunSelectionProgress,
    budget: Option<&RequestBudgetV1>,
) -> Result<WindowWitness, CoreError> {
    base.validate()?;
    if retired
        .windows(2)
        .any(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a, b)| a >= b))
    {
        return Err(CoreError::InvalidContract(
            "lexical: Native hard scan retirements are not canonical".into(),
        ));
    }
    let high = base
        .run_registry
        .next_run_id
        .checked_sub(1)
        .filter(|high| *high != 0)
        .ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native hard scan high-water underflows".into())
        })?;
    if !policy.hard_pressure(base.run_registry.snapshot()?)
        || progress.pending_run.is_some()
        || progress.scan_high_water_run_id != 0 && progress.scan_high_water_run_id != high
    {
        return Err(CoreError::InvalidContract(
            "lexical: Native hard scan policy or cursor differs".into(),
        ));
    }
    let mut touched = BTreeSet::new();
    for id in retired {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:native:hard-scan-retired")?;
        }
        let row = read_current_with_ref_budget(store, base, *id, budget)?
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: Native hard scan retirement absent".into())
            })?
            .1;
        let _inserted = touched.insert(row.segment_key);
    }
    let virtual_registry = base
        .run_registry
        .clone()
        .with_selection_progress(progress)?;
    let window = virtual_registry.scan_hard_window(store, 32, budget)?;
    if window.scan_high_water_run_id != high {
        return Err(CoreError::InvalidContract(
            "lexical: Native hard scan frozen frontier differs".into(),
        ));
    }
    let mut candidates = Vec::new();
    candidates
        .try_reserve(window.candidates.len())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: reserve Native hard scan witnesses: {error}"
            ))
        })?;
    for candidate in &window.candidates {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:native:hard-scan-candidate")?;
        }
        candidates.push(CandidateWitness {
            run_id: candidate.run_id,
            index_encoded_bytes: candidate.index_encoded_bytes,
            eligibility: probe_clean_cohort_metadata(
                store, base, candidate, &touched, policy, budget,
            )?,
            header_sha256: native_candidate_header_sha256(store, base, candidate, budget)?,
        });
    }
    Ok(WindowWitness {
        ordinal: 0,
        prior_chain_sha256: [0; 32],
        start_run_id: progress.next_scan_run_id,
        next_run_id: window.next_scan_run_id,
        frozen_high_water_run_id: high,
        round_complete: window.round_complete,
        candidates,
    })
}

type HardRunSelection = (Vec<(u64, u64, u64)>, RunSelectionProgress);

fn native_hard_selection(
    base: &NativeRoot,
    policy: &LexicalMaintenancePolicy,
    hard_scan: Option<&ScanLane>,
) -> Result<HardRunSelection, CoreError> {
    let lane = hard_scan.ok_or_else(|| {
        CoreError::InvalidContract("lexical: Native hard scan checkpoint is missing".into())
    })?;
    let high = base
        .run_registry
        .next_run_id
        .checked_sub(1)
        .ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native hard scan high-water underflows".into())
        })?;
    if lane.authority != Authority::Native
        || lane.base_authority_root_sha256 != native_root_sha256(base)?
        || lane.frozen_high_water_run_id != high
        || !lane
            .windows
            .last()
            .is_some_and(|window| window.round_complete)
    {
        return Err(CoreError::InvalidContract(
            "lexical: Native hard scan checkpoint is stale or incomplete".into(),
        ));
    }
    let mut selected = Vec::new();
    let mut earlier = Vec::<(u64, u64, u64)>::new();
    let mut bytes = 0_u64;
    let mut charge = 0_u64;
    let mut next_start = 1_u64;
    let max_fan_in = usize::try_from(policy.max_merge_fan_in()).map_err(|error| {
        CoreError::InvalidContract(format!("lexical: Native fan-in width: {error}"))
    })?;
    if lane.windows.len() > 129 {
        return Err(CoreError::InvalidContract(
            "lexical: Native hard scan window bound differs".into(),
        ));
    }
    let mut examined = 0_usize;
    for (ordinal, window) in lane.windows.iter().enumerate() {
        let last = ordinal.checked_add(1) == Some(lane.windows.len());
        let end_exclusive = if last {
            high.checked_add(1).ok_or_else(|| {
                CoreError::InvalidContract("lexical: Native hard scan frontier overflows".into())
            })?
        } else {
            window.next_run_id
        };
        if window.start_run_id != next_start
            || window.frozen_high_water_run_id != high
            || usize::from(window.ordinal) != ordinal
            || window.round_complete != last
            || window.candidates.len() > 32
            || (!last && window.candidates.is_empty())
            || (!last
                && (window.next_run_id <= next_start
                    || window.next_run_id > high.saturating_add(1)))
            || (last && window.next_run_id != 1)
        {
            return Err(CoreError::InvalidContract(
                "lexical: Native hard scan window cursor differs".into(),
            ));
        }
        examined = examined
            .checked_add(window.candidates.len())
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: Native hard scan count overflows".into())
            })?;
        if examined > 4096 {
            return Err(CoreError::InvalidContract(
                "lexical: Native hard scan count bound differs".into(),
            ));
        }
        let mut prior_id = window.start_run_id.saturating_sub(1);
        for candidate in &window.candidates {
            if candidate.run_id <= prior_id || candidate.run_id >= end_exclusive {
                return Err(CoreError::InvalidContract(
                    "lexical: Native hard scan candidate order differs".into(),
                ));
            }
            prior_id = candidate.run_id;
        }
        next_start = window.next_run_id;
        for candidate in &window.candidates {
            if selected.len() >= max_fan_in {
                continue;
            }
            let EligibilityWitness::NativeEligible {
                effective_input_encoded_bytes,
                staging_bytes,
            } = candidate.eligibility
            else {
                if matches!(
                    candidate.eligibility,
                    EligibilityWitness::FileEligible { .. }
                ) {
                    return Err(CoreError::InvalidContract(
                        "lexical: File witness in Native lane".into(),
                    ));
                }
                continue;
            };
            if effective_input_encoded_bytes != candidate.index_encoded_bytes
                || effective_input_encoded_bytes > policy.max_merge_input_encoded_bytes()
                || staging_bytes > policy.max_staging_bytes()
            {
                return Err(CoreError::InvalidContract(
                    "lexical: Native hard scan cost differs".into(),
                ));
            }
            if !selected.is_empty() {
                if let (Some(next_bytes), Some(next_charge)) = (
                    bytes.checked_add(effective_input_encoded_bytes),
                    charge.checked_add(staging_bytes),
                ) && next_bytes <= policy.max_merge_input_encoded_bytes()
                    && next_charge <= policy.max_staging_bytes()
                {
                    bytes = next_bytes;
                    charge = next_charge;
                    selected.push((
                        candidate.run_id,
                        effective_input_encoded_bytes,
                        staging_bytes,
                    ));
                }
                continue;
            }
            for &(first_id, first_bytes, first_charge) in &earlier {
                if let (Some(next_bytes), Some(next_charge)) = (
                    first_bytes.checked_add(effective_input_encoded_bytes),
                    first_charge.checked_add(staging_bytes),
                ) && next_bytes <= policy.max_merge_input_encoded_bytes()
                    && next_charge <= policy.max_staging_bytes()
                {
                    bytes = next_bytes;
                    charge = next_charge;
                    selected.push((first_id, first_bytes, first_charge));
                    selected.push((
                        candidate.run_id,
                        effective_input_encoded_bytes,
                        staging_bytes,
                    ));
                    break;
                }
            }
            if selected.is_empty() {
                earlier.push((
                    candidate.run_id,
                    effective_input_encoded_bytes,
                    staging_bytes,
                ));
            }
        }
    }
    Ok((
        selected,
        RunSelectionProgress {
            next_scan_run_id: lane.windows.last().map_or(1, |window| window.next_run_id),
            scan_high_water_run_id: high,
            pending_run: None,
        },
    ))
}

/// Prepare complete bodies before the first source/segment write.
///
/// The staging limit charges descriptor/current/body carriers and document
/// slots; it is separate from attributed input-index bytes and is not RSS.
fn prepare_clean_merge(
    store: &ObjectStore,
    base: &NativeRoot,
    selected: &[(u64, BlobRef)],
    limit: u64,
    charged: u64,
) -> Result<Option<PreparedCleanMerge>, CoreError> {
    if selected.is_empty() {
        return Ok(None);
    }
    #[cfg(test)]
    CLEAN_MERGE_PREPARE_CALLS.with(|count| count.set(count.get().saturating_add(1)));
    let mut scratch = ScratchAdmission { limit, charged };
    let mut descriptors = Vec::new();
    let mut docs = Vec::new();
    let mut text_bytes = 0_u64;
    for (id, reference) in selected {
        scratch
            .charge(reference.bytes.checked_mul(2).ok_or_else(|| {
                CoreError::InvalidContract("lexical: clean descriptor charge overflows".into())
            })?)
            .map_err(|_error| clean_merge_scratch_refusal())?;
        let descriptor = read_segment(store, base, *id)?;
        let mut segment_docs =
            live_segment_docs_admitted(store, base, &descriptor, Some(&mut scratch), None)
                .map_err(|error| {
                    if matches!(&error, CoreError::InvalidContract(message) if message == "lexical: Native compaction preparation scratch budget exceeded") {
                        clean_merge_scratch_refusal()
                    } else {
                        error
                    }
                })?;
        scratch
            .charge(
                u64::try_from(segment_docs.len())
                    .map_err(|_error| {
                        CoreError::InvalidContract(
                            "lexical: clean document slot count overflows".into(),
                        )
                    })?
                    .checked_mul(u64::try_from(std::mem::size_of::<AddedTextDoc>()).map_err(
                        |_error| {
                            CoreError::InvalidContract(
                                "lexical: clean document slot width overflows".into(),
                            )
                        },
                    )?)
                    .ok_or_else(|| {
                        CoreError::InvalidContract(
                            "lexical: clean document slot charge overflows".into(),
                        )
                    })?,
            )
            .map_err(|_error| clean_merge_scratch_refusal())?;
        docs.try_reserve_exact(segment_docs.len())
            .map_err(|error| {
                CoreError::Storage(format!("lexical: reserve clean merge documents: {error}"))
            })?;
        for doc in &segment_docs {
            text_bytes = text_bytes
                .checked_add(u64::try_from(doc.text.len()).map_err(|_error| {
                    CoreError::InvalidContract("lexical: clean text width overflows".into())
                })?)
                .ok_or_else(|| {
                    CoreError::InvalidContract("lexical: clean text byte count overflows".into())
                })?;
        }
        docs.append(&mut segment_docs);
        descriptors.push((reference.clone(), descriptor));
    }
    docs.sort_unstable_by_key(|doc| doc.doc_id);
    if docs.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(left, right)| left.doc_id == right.doc_id)
    }) {
        return Err(CoreError::InvalidContract(
            "lexical: clean merge inputs repeat logical ID".into(),
        ));
    }
    Ok(Some(PreparedCleanMerge {
        selected: descriptors,
        docs,
        text_bytes,
        charged: scratch.charged,
    }))
}

/// Immutable inputs shared by hard and soft clean-cohort selection.
struct CleanMergeContext<'a> {
    store: &'a ObjectStore,
    base: &'a NativeRoot,
    touched: &'a BTreeSet<u64>,
    policy: &'a LexicalMaintenancePolicy,
}

/// Try one complete cohort without changing the accumulated candidate set.
fn try_select_clean_cohort(
    context: &CleanMergeContext<'_>,
    candidate: &RunCandidate,
    prepared: &mut PreparedCleanMerge,
    selected_bytes: &mut u64,
    selected_runs: &mut u32,
    strict_selected: bool,
) -> Result<bool, CoreError> {
    let Some(next_bytes) = selected_bytes.checked_add(candidate.index_encoded_bytes) else {
        return Ok(false);
    };
    if next_bytes > context.policy.max_merge_input_encoded_bytes() {
        return Ok(false);
    }
    let mut cohort = Vec::new();
    let mut complete = true;
    context.base.run_registry.visit_run_members(
        context.store,
        candidate,
        None,
        &mut |key, reference, _bytes| {
            if !complete {
                return Ok(());
            }
            let id = u64::from_be_bytes(key.try_into().map_err(|_error| {
                CoreError::InvalidContract("lexical: clean merge segment key width".into())
            })?);
            let current = context.store.map_get(&context.base.segments, key, None)?;
            let due = context
                .store
                .map_get(&context.base.compaction_due, key, None)?;
            if context.touched.contains(&id) || current.as_ref() != Some(reference) || due.is_some()
            {
                complete = false;
                return Ok(());
            }
            let descriptor = read_segment(context.store, context.base, id)?;
            if descriptor.live_locals.len() != descriptor.logical_ids.len() {
                complete = false;
                return Ok(());
            }
            cohort.push((id, reference.clone()));
            Ok(())
        },
    )?;
    if !complete {
        return Ok(false);
    }
    let tentative = match prepare_clean_merge(
        context.store,
        context.base,
        &cohort,
        context.policy.max_staging_bytes(),
        prepared.charged,
    ) {
        Ok(Some(tentative)) => tentative,
        Ok(None) => return Ok(false),
        Err(CoreError::Typed { code, .. })
            if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE && !strict_selected =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    *selected_bytes = next_bytes;
    *selected_runs = selected_runs.checked_add(1).ok_or_else(|| {
        CoreError::InvalidContract("lexical: clean merge selected run count overflow".into())
    })?;
    prepared.charged = tentative.charged;
    prepared.text_bytes = prepared
        .text_bytes
        .checked_add(tentative.text_bytes)
        .ok_or_else(|| {
            CoreError::InvalidContract("lexical: clean text byte count overflows".into())
        })?;
    prepared.selected.extend(tentative.selected);
    prepared.docs.extend(tentative.docs);
    Ok(true)
}

/// Select at most the configured fan-in from a bounded rotating candidate
/// window and one authenticated pending anchor. Selection never consults an
/// optional preparation packet.
fn select_clean_merge_inputs(
    store: &ObjectStore,
    base: &NativeRoot,
    retired: &[u64],
    policy: &LexicalMaintenancePolicy,
    hard_scan: Option<&ScanLane>,
) -> Result<(Option<PreparedCleanMerge>, RunSelectionProgress), CoreError> {
    let prior_progress = base.run_registry.selection_progress();
    let before = base.run_registry.snapshot()?;
    if !policy.merge_due(before) {
        return Ok((None, prior_progress));
    }
    let mut touched = BTreeSet::new();
    for id in retired {
        let row = read_current(store, base, *id)?.ok_or_else(|| {
            CoreError::InvalidContract("lexical: clean merge retirement absent in base".into())
        })?;
        let _inserted = touched.insert(row.segment_key);
    }
    let selection_context = CleanMergeContext {
        store,
        base,
        touched: &touched,
        policy,
    };
    let mut prepared = PreparedCleanMerge {
        selected: Vec::new(),
        docs: Vec::new(),
        text_bytes: 0,
        charged: 0,
    };
    let mut selected_runs = 0_u32;
    let mut selected_bytes = 0_u64;
    let mut first_selected_run = None;
    let hard = policy.hard_pressure(before);
    let (soft_window, hard_progress) = if hard {
        let (hard_selected, progress) = native_hard_selection(base, policy, hard_scan)?;
        for (id, expected_bytes, expected_charge) in &hard_selected {
            let candidate = base
                .run_registry
                .candidate_by_id(store, *id, None)?
                .ok_or_else(|| {
                    CoreError::InvalidContract("lexical: hard merge run disappeared".into())
                })?;
            let prior_charge = prepared.charged;
            if candidate.index_encoded_bytes != *expected_bytes
                || !try_select_clean_cohort(
                    &selection_context,
                    &candidate,
                    &mut prepared,
                    &mut selected_bytes,
                    &mut selected_runs,
                    true,
                )?
                || prepared.charged.checked_sub(prior_charge) != Some(*expected_charge)
            {
                return Err(CoreError::InvalidContract(
                    "lexical: hard merge selected cohort differs from checkpoint".into(),
                ));
            }
        }
        if selected_runs < 2 {
            return Ok((None, progress));
        }
        (None, Some(progress))
    } else {
        if hard_scan.is_some() {
            return Err(CoreError::InvalidContract(
                "lexical: Native hard checkpoint supplied outside hard pressure".into(),
            ));
        }
        let mut consider = |candidate: &RunCandidate| -> Result<bool, CoreError> {
            if selected_runs >= policy.max_merge_fan_in() {
                return Ok(true);
            }
            if try_select_clean_cohort(
                &selection_context,
                candidate,
                &mut prepared,
                &mut selected_bytes,
                &mut selected_runs,
                false,
            )? {
                let _first = first_selected_run.get_or_insert(candidate.run_id);
            }
            Ok(selected_runs >= policy.max_merge_fan_in())
        };
        if let Some(id) = prior_progress.pending_run
            && let Some(candidate) = base.run_registry.candidate_by_id(store, id, None)?
        {
            let _full = consider(&candidate)?;
        }
        let window = base.run_registry.scan_window(
            store,
            bounded_candidate_scan_limit(policy.max_merge_fan_in()),
            None,
        )?;
        for candidate in &window.candidates {
            if Some(candidate.run_id) != prior_progress.pending_run {
                let _full = consider(candidate)?;
            }
        }
        (Some(window), None)
    };
    let mut progress = if let Some(progress) = hard_progress {
        progress
    } else {
        let window = soft_window.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native soft scan window absent".into())
        })?;
        RunSelectionProgress {
            next_scan_run_id: window.next_scan_run_id,
            scan_high_water_run_id: window.scan_high_water_run_id,
            pending_run: None,
        }
    };
    if selected_runs < 2 {
        if hard {
            return Ok((None, progress));
        }
        let window = soft_window.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native soft scan window absent".into())
        })?;
        if prior_progress.pending_run.is_some() && window.round_complete {
            // Exhausted a frozen round without a compatible partner. Rotate
            // the anchor so an individually eligible old run cannot pin the
            // frontier forever.
            progress.next_scan_run_id = prior_progress
                .pending_run
                .and_then(|id| id.checked_add(1))
                .ok_or_else(|| {
                    CoreError::InvalidContract("lexical: pending scan cursor overflows".into())
                })?;
            progress.scan_high_water_run_id = base
                .run_registry
                .next_run_id
                .checked_sub(1)
                .ok_or_else(|| {
                    CoreError::InvalidContract("lexical: scan high-water underflow".into())
                })?;
        } else if let Some(anchor) = first_selected_run {
            progress.pending_run = Some(anchor);
            if prior_progress.pending_run != Some(anchor) {
                progress.next_scan_run_id = anchor.checked_add(1).ok_or_else(|| {
                    CoreError::InvalidContract("lexical: pending scan cursor overflows".into())
                })?;
                progress.scan_high_water_run_id = base
                    .run_registry
                    .next_run_id
                    .checked_sub(1)
                    .ok_or_else(|| {
                        CoreError::InvalidContract("lexical: scan high-water underflow".into())
                    })?;
            }
        }
        return Ok((None, progress));
    }
    prepared.docs.sort_unstable_by_key(|doc| doc.doc_id);
    if prepared.docs.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(left, right)| left.doc_id == right.doc_id)
    }) {
        return Err(CoreError::InvalidContract(
            "lexical: clean merge inputs repeat logical ID".into(),
        ));
    }
    Ok((Some(prepared), progress))
}

impl ScratchAdmission {
    fn charge(&mut self, bytes: u64) -> Result<(), CoreError> {
        self.charged = self
            .charged
            .checked_add(bytes)
            .ok_or_else(compaction_scratch_limit)?;
        if self.charged > self.limit {
            return Err(compaction_scratch_limit());
        }
        Ok(())
    }
}

/// Read at most the two oldest debts and retain their authenticated bodies and
/// exact map witnesses.
///
/// The caller holds the base's retention custody while
/// this read is active. A rejected scratch budget leaves no partial objects.
pub(crate) fn prepare_compaction(
    store: &ObjectStore,
    base: &NativeRoot,
    max_scratch_bytes: u64,
    excluded_ids: &[u64],
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<PreparedNativeCompaction>, CoreError> {
    match prepare_compaction_inner(store, base, max_scratch_bytes, excluded_ids, budget) {
        Err(CoreError::InvalidContract(message))
            if message == "lexical: Native compaction preparation scratch budget exceeded" =>
        {
            Ok(None)
        }
        result => result,
    }
}

fn prepare_compaction_inner(
    store: &ObjectStore,
    base: &NativeRoot,
    max_scratch_bytes: u64,
    excluded_ids: &[u64],
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<PreparedNativeCompaction>, CoreError> {
    if let Some(budget) = budget {
        budget.checkpoint("lexical:native:prepare:begin")?;
    }
    if excluded_ids
        .windows(2)
        .any(|pair| pair.first() >= pair.get(1))
    {
        return Err(CoreError::InvalidContract(
            "lexical: prepared retirement IDs are not sorted and unique".into(),
        ));
    }
    base.validate()?;
    let due = store.map_scan_limit(&base.compaction_due, &[0], None, 2, budget)?;
    if due.is_empty() {
        return Ok(None);
    }
    let mut admission = ScratchAdmission {
        limit: max_scratch_bytes,
        charged: 0,
    };
    // Fixed transient record/descriptor/source-key decode space. Retained
    // source strings are charged by their authenticated record length below.
    admission.charge(8 * MAX_CURRENT_RECORD_BYTES + 2 * MAX_SOURCE_RECORD_BYTES)?;
    let mut selected = Vec::new();
    selected.try_reserve_exact(2).map_err(|error| {
        CoreError::Storage(format!("lexical: reserve prepared descriptors: {error}"))
    })?;
    let mut docs = Vec::new();
    admission.charge(
        u64::try_from(
            SEGMENT_DOCS
                .checked_mul(std::mem::size_of::<AddedTextDoc>())
                .ok_or_else(compaction_scratch_limit)?,
        )
        .map_err(|_error| compaction_scratch_limit())?,
    )?;
    docs.try_reserve_exact(SEGMENT_DOCS).map_err(|error| {
        CoreError::Storage(format!("lexical: reserve prepared documents: {error}"))
    })?;
    for (key, reference) in due {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:native:prepare:segment")?;
        }
        admission.charge(
            reference
                .bytes
                .checked_add(
                    u64::try_from(std::mem::size_of::<(u64, BlobRef, SegmentDescriptor)>())
                        .map_err(|_error| compaction_scratch_limit())?,
                )
                .ok_or_else(compaction_scratch_limit)?,
        )?;
        let id = u64::from_be_bytes(key.as_slice().try_into().map_err(|_error| {
            CoreError::InvalidContract("lexical: Native compaction key width differs".into())
        })?);
        if store.map_get(&base.segments, &key, budget)? != Some(reference.clone()) {
            return Err(CoreError::InvalidContract(
                "lexical: due segment version differs".into(),
            ));
        }
        let descriptor = read_segment_budget(store, base, id, budget)?;
        for local in &descriptor.live_locals {
            let index = usize::try_from(*local).map_err(|error| {
                CoreError::InvalidContract(format!("lexical: prepared local ID width: {error}"))
            })?;
            let id = descriptor.logical_ids.get(index).ok_or_else(|| {
                CoreError::InvalidContract("lexical: prepared local ID absent".into())
            })?;
            if excluded_ids.binary_search(id).is_ok() {
                // Never read a body which this publication will retire.
                return Ok(None);
            }
        }
        let segment_docs =
            live_segment_docs_admitted(store, base, &descriptor, Some(&mut admission), budget)?;
        let added_bytes = segment_docs
            .iter()
            .try_fold(0_usize, |sum, doc| sum.checked_add(doc.text.len()))
            .ok_or_else(compaction_scratch_limit)?;
        let prior_bytes = docs
            .iter()
            .try_fold(0_usize, |sum: usize, doc: &AddedTextDoc| {
                sum.checked_add(doc.text.len())
            })
            .ok_or_else(compaction_scratch_limit)?;
        if !selected.is_empty()
            && (docs.len().saturating_add(segment_docs.len()) > SEGMENT_DOCS
                || prior_bytes.saturating_add(added_bytes) > SEGMENT_TARGET_TEXT_BYTES)
        {
            break;
        }
        docs.extend(segment_docs);
        selected.push((id, reference, descriptor));
    }
    if selected.is_empty() || docs.is_empty() {
        return Err(CoreError::InvalidContract(
            "lexical: Native compaction selected no live segment".into(),
        ));
    }
    docs.sort_unstable_by_key(|doc| doc.doc_id);
    if docs.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(a, b)| a.doc_id == b.doc_id)
    }) {
        return Err(CoreError::InvalidContract(
            "lexical: compaction source segments share a live logical ID".into(),
        ));
    }
    admission.charge(
        u64::try_from(docs.len())
            .map_err(|_error| compaction_scratch_limit())?
            .checked_mul(
                u64::try_from(std::mem::size_of::<(u64, BlobRef, BlobRef)>())
                    .map_err(|_error| compaction_scratch_limit())?,
            )
            .ok_or_else(compaction_scratch_limit)?,
    )?;
    let mut current_witnesses = Vec::new();
    current_witnesses
        .try_reserve_exact(docs.len())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: reserve prepared Native witnesses: {error}"
            ))
        })?;
    for doc in &docs {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:native:prepare:witness")?;
        }
        let (current_ref, current) = read_current_with_ref_budget(store, base, doc.doc_id, budget)?
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: prepared current is absent".into())
            })?;
        let digest = source_digest(&current.source)?;
        let membership_ref = store
            .map_get(
                &base.membership,
                &membership_key(&digest, doc.doc_id),
                budget,
            )?
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: prepared membership is absent".into())
            })?;
        let same_source = current.source == doc.source_file;
        if current_ref != membership_ref || !same_source || current.candidate_id != doc.candidate_id
        {
            return Err(CoreError::InvalidContract(
                "lexical: prepared current/membership differs".into(),
            ));
        }
        current_witnesses.push((doc.doc_id, current_ref, membership_ref));
    }
    Ok(Some(PreparedNativeCompaction {
        selected,
        current_witnesses,
        docs,
        staging_charged_bytes: admission.charged,
        policy_binding: None,
    }))
}

#[derive(Clone, Copy, Default)]
struct PostingPageStats {
    pages: u64,
    oversized_pages: u64,
    oversized_bytes: u64,
}

impl PostingPageStats {
    fn add(&mut self, other: Self) -> Result<(), CoreError> {
        self.pages = self
            .pages
            .checked_add(other.pages)
            .ok_or_else(page_count_overflow)?;
        self.oversized_pages = self
            .oversized_pages
            .checked_add(other.oversized_pages)
            .ok_or_else(page_count_overflow)?;
        self.oversized_bytes = self
            .oversized_bytes
            .checked_add(other.oversized_bytes)
            .ok_or_else(page_count_overflow)?;
        Ok(())
    }
}

fn page_count_overflow() -> CoreError {
    CoreError::InvalidContract("lexical: Native posting page accounting overflow".into())
}

pub(crate) fn read_current(
    store: &ObjectStore,
    root: &NativeRoot,
    id: u64,
) -> Result<Option<CurrentRecord>, CoreError> {
    Ok(read_current_with_ref(store, root, id)?.map(|(_, record)| record))
}

/// Return the authenticated current map value alongside its decoded record.
/// Seal proofs use the same lookup to compare current and membership references.
pub(crate) fn read_current_with_ref(
    store: &ObjectStore,
    root: &NativeRoot,
    id: u64,
) -> Result<Option<(BlobRef, CurrentRecord)>, CoreError> {
    read_current_with_ref_budget(store, root, id, None)
}

pub(crate) fn read_current_with_ref_budget(
    store: &ObjectStore,
    root: &NativeRoot,
    id: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<(BlobRef, CurrentRecord)>, CoreError> {
    let Some(reference) = store.map_get(&root.current, &current_key(id), budget)? else {
        return Ok(None);
    };
    if reference.root.kind != ObjectKind::Record {
        return Err(CoreError::InvalidContract(
            "lexical: Native current value kind differs".into(),
        ));
    }
    let record =
        CurrentRecord::decode(&store.read_blob(&reference, MAX_CURRENT_RECORD_BYTES, budget)?)?;
    if record.id != id {
        return Err(CoreError::InvalidContract(
            "lexical: Native current key/value ID differs".into(),
        ));
    }
    Ok(Some((reference, record)))
}

fn apply_edits(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: &MapRoot,
    edits: BTreeMap<Vec<u8>, Option<BlobRef>>,
) -> Result<MapRoot, CoreError> {
    store.map_apply(
        lease,
        base,
        edits
            .into_iter()
            .map(|(key, value)| Ok(MapEdit { key, value })),
    )
}

fn checked_insert(
    edits: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    key: Vec<u8>,
    value: Option<BlobRef>,
) -> Result<(), CoreError> {
    if edits.insert(key, value).is_some() {
        return Err(CoreError::InvalidContract(
            "lexical: duplicate Native map edit key".into(),
        ));
    }
    Ok(())
}

/// Publish one complete effective change set. The caller owns the durable
/// allocation reservation and supplies the exact retired IDs before mutating
/// Tantivy.
///
/// Logical IDs are never selected from the live maximum here.
#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "one publish call binds the complete declared delta and durable allocation scope"
)]
pub(crate) fn publish_delta(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&NativeRoot>,
    source_edits: &[NativeSourceEdit],
    retired: &[u64],
    added: &[AddedTextDoc],
    clear_all: bool,
    target_generation: ManifestGeneration,
    allocation_scope_sha256: [u8; 32],
) -> Result<(NativeRoot, TextAuthorityWriteReceipt), CoreError> {
    publish_delta_with_option(
        store,
        lease,
        base,
        source_edits,
        retired,
        added,
        clear_all,
        target_generation,
        allocation_scope_sha256,
        None,
        None,
        None,
    )
}

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "one publish call binds the complete declared delta and optional pure preparation"
)]
pub(crate) fn publish_delta_prepared(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&NativeRoot>,
    source_edits: &[NativeSourceEdit],
    retired: &[u64],
    added: &[AddedTextDoc],
    clear_all: bool,
    target_generation: ManifestGeneration,
    allocation_scope_sha256: [u8; 32],
    prepared: Option<PreparedNativeCompaction>,
) -> Result<(NativeRoot, TextAuthorityWriteReceipt), CoreError> {
    publish_delta_prepared_with_policy(
        store,
        lease,
        base,
        source_edits,
        retired,
        added,
        clear_all,
        target_generation,
        allocation_scope_sha256,
        prepared,
        None,
        None,
    )
}

/// Policy-aware publication. The legacy entrypoints pass no policy and keep
/// their existing debt-only behavior.
#[expect(
    clippy::too_many_arguments,
    reason = "one publish call binds one delta, optional preparation, and the service policy"
)]
pub(crate) fn publish_delta_prepared_with_policy(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&NativeRoot>,
    source_edits: &[NativeSourceEdit],
    retired: &[u64],
    added: &[AddedTextDoc],
    clear_all: bool,
    target_generation: ManifestGeneration,
    allocation_scope_sha256: [u8; 32],
    prepared: Option<PreparedNativeCompaction>,
    policy: Option<&LexicalMaintenancePolicy>,
    hard_scan: Option<&ScanLane>,
) -> Result<(NativeRoot, TextAuthorityWriteReceipt), CoreError> {
    publish_delta_with_option(
        store,
        lease,
        base,
        source_edits,
        retired,
        added,
        clear_all,
        target_generation,
        allocation_scope_sha256,
        prepared,
        policy,
        hard_scan,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "one publication binds one complete delta and its optional prepared input"
)]
fn publish_delta_with_option(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&NativeRoot>,
    source_edits: &[NativeSourceEdit],
    retired: &[u64],
    added: &[AddedTextDoc],
    clear_all: bool,
    target_generation: ManifestGeneration,
    allocation_scope_sha256: [u8; 32],
    prepared: Option<PreparedNativeCompaction>,
    policy: Option<&LexicalMaintenancePolicy>,
    hard_scan: Option<&ScanLane>,
) -> Result<(NativeRoot, TextAuthorityWriteReceipt), CoreError> {
    store.with_adoption_batch(lease, |batched_store| {
        // Preserve an explicit caller's physical-read meter. Ordinary callers
        // receive a bounded producer cache; seal proofs create fresh scopes.
        let producer_store = if batched_store.has_read_meter() {
            batched_store.clone()
        } else {
            batched_store.metered_clone()
        };
        publish_delta_inner(
            &producer_store,
            lease,
            base,
            source_edits,
            retired,
            added,
            clear_all,
            target_generation,
            allocation_scope_sha256,
            prepared,
            policy,
            hard_scan,
        )
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "one publish call binds the complete declared delta and durable allocation scope"
)]
fn publish_delta_inner(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&NativeRoot>,
    source_edits: &[NativeSourceEdit],
    retired: &[u64],
    added: &[AddedTextDoc],
    clear_all: bool,
    target_generation: ManifestGeneration,
    allocation_scope_sha256: [u8; 32],
    prepared: Option<PreparedNativeCompaction>,
    policy: Option<&LexicalMaintenancePolicy>,
    hard_scan: Option<&ScanLane>,
) -> Result<(NativeRoot, TextAuthorityWriteReceipt), CoreError> {
    let inherited = base
        .cloned()
        .unwrap_or_else(|| NativeRoot::empty(allocation_scope_sha256));
    inherited.validate()?;
    if inherited.allocation_scope_sha256 != allocation_scope_sha256 {
        return Err(CoreError::InvalidContract(
            "lexical: Native allocation scope changed".into(),
        ));
    }
    let mut next = if clear_all {
        let mut empty = NativeRoot::empty(allocation_scope_sha256);
        empty.next_segment_id = inherited.next_segment_id;
        empty
    } else {
        inherited.clone()
    };
    if added.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(a, b)| a.doc_id >= b.doc_id)
    }) || retired
        .windows(2)
        .any(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a, b)| a >= b))
        || added.iter().any(|doc| doc.doc_id == 0)
    {
        return Err(CoreError::InvalidContract(
            "lexical: Native delta IDs are not canonical".into(),
        ));
    }
    if hard_scan.is_some() && (base.is_none() || clear_all || policy.is_none()) {
        return Err(CoreError::InvalidContract(
            "lexical: Native hard checkpoint without old policy authority".into(),
        ));
    }
    let (clean_merge, selection_progress) = match (base, policy) {
        (Some(prior), Some(policy)) if !clear_all => {
            select_clean_merge_inputs(store, prior, retired, policy, hard_scan)?
        }
        _ if clear_all => (None, RunSelectionProgress::default()),
        _ => (None, inherited.run_registry.selection_progress()),
    };
    let mut source_map = BTreeMap::new();
    let mut current_map = BTreeMap::new();
    let mut membership_map = BTreeMap::new();
    let mut segment_map = BTreeMap::new();
    let mut gram_map = BTreeMap::new();
    let mut term_map = BTreeMap::new();
    let mut due_map = BTreeMap::new();
    let mut retired_segments = BTreeMap::<u64, SegmentDescriptor>::new();
    let mut changed_sources = BTreeSet::<[u8; 32]>::new();
    let mut upserted_sources = BTreeSet::<[u8; 32]>::new();
    for edit in source_edits {
        let digest = source_digest(edit.key())?;
        if !changed_sources.insert(digest) {
            return Err(CoreError::InvalidContract(
                "lexical: duplicate Native source edit".into(),
            ));
        }
        if !clear_all && let Some(prior) = store.map_get(&inherited.source, &digest, None)? {
            let prior = decode_source(&store.read_blob(&prior, MAX_SOURCE_RECORD_BYTES, None)?)?;
            if prior.source.file != *edit.key() {
                return Err(CoreError::InvalidContract(
                    "lexical: Native source digest collision".into(),
                ));
            }
        }
        let value = match edit {
            NativeSourceEdit::Upsert(coverage) => {
                let _new = upserted_sources.insert(digest);
                let bytes = encode_source(coverage)?;
                Some(store.put_blob(lease, ObjectKind::Record, &[], &bytes)?)
            }
            NativeSourceEdit::Delete(_) => None,
        };
        checked_insert(&mut source_map, digest.to_vec(), value)?;
    }
    if clear_all && !retired.is_empty() {
        return Err(CoreError::InvalidContract(
            "lexical: Native clear-all cannot also name individual retirements".into(),
        ));
    }
    if !clear_all {
        let mut expected_retired = Vec::new();
        for edit in source_edits {
            expected_retired.extend(source_ids(store, &inherited, edit.key())?);
        }
        expected_retired.sort_unstable();
        if expected_retired != retired {
            return Err(CoreError::InvalidContract(
                "lexical: Native source bundle retirement is incomplete or foreign".into(),
            ));
        }
        for id in retired {
            let record = read_current(store, &inherited, *id)?.ok_or_else(|| {
                CoreError::InvalidContract(format!("lexical: retire missing Native doc {id}"))
            })?;
            let digest = source_digest(&record.source)?;
            let member_key = membership_key(&digest, *id);
            let member = store
                .map_get(&inherited.membership, &member_key, None)?
                .ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical: Native current doc lacks source membership".into(),
                    )
                })?;
            let member_record = CurrentRecord::decode(&store.read_blob(
                &member,
                MAX_CURRENT_RECORD_BYTES,
                None,
            )?)?;
            if member_record.id != *id || member_record.source != record.source {
                return Err(CoreError::InvalidContract(
                    "lexical: Native source membership differs from current".into(),
                ));
            }
            if let std::collections::btree_map::Entry::Vacant(entry) =
                retired_segments.entry(record.segment_key)
            {
                let descriptor = read_segment(store, &inherited, record.segment_key)?;
                let _prior = entry.insert(descriptor);
            }
            let descriptor = retired_segments
                .get_mut(&record.segment_key)
                .ok_or_else(|| {
                    CoreError::InvalidContract("lexical: Native retired segment vanished".into())
                })?;
            if descriptor
                .logical_ids
                .get(usize::try_from(record.local_id).map_err(|error| {
                    CoreError::InvalidContract(format!("lexical: Native local ID width: {error}"))
                })?)
                != Some(id)
            {
                return Err(CoreError::InvalidContract(
                    "lexical: Native retired current locator differs from segment".into(),
                ));
            }
            let position = descriptor
                .live_locals
                .binary_search(&record.local_id)
                .map_err(|_error| {
                    CoreError::InvalidContract(
                        "lexical: Native retired doc is not live in segment".into(),
                    )
                })?;
            let _removed = descriptor.live_locals.remove(position);
            checked_insert(&mut current_map, current_key(*id).to_vec(), None)?;
            checked_insert(&mut membership_map, member_key, None)?;
        }
    }
    for (id, descriptor) in retired_segments {
        if descriptor.live_locals.is_empty() {
            for route in read_route_inventory(store, &descriptor)? {
                let target = match route.first().copied() {
                    Some(GRAM_RAW | GRAM_FOLDED) => &mut gram_map,
                    Some(TERM_RAW | TERM_FOLDED) => &mut term_map,
                    _ => {
                        return Err(CoreError::InvalidContract(
                            "lexical: Native route inventory has unknown surface".into(),
                        ));
                    }
                };
                checked_insert(target, route, None)?;
            }
            checked_insert(&mut segment_map, segment_key(id).to_vec(), None)?;
            if store
                .map_get(&inherited.compaction_due, &segment_key(id), None)?
                .is_some()
            {
                checked_insert(&mut due_map, segment_key(id).to_vec(), None)?;
            }
            continue;
        }
        let bytes = descriptor.encode()?;
        let inventory = descriptor.route_inventory.as_ref().ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: retired Native segment lacks route inventory".into(),
            )
        })?;
        let reference = store.put_blob(
            lease,
            ObjectKind::Segment,
            std::slice::from_ref(&inventory.root),
            &bytes,
        )?;
        checked_insert(
            &mut segment_map,
            segment_key(id).to_vec(),
            Some(reference.clone()),
        )?;
        checked_insert(&mut due_map, segment_key(id).to_vec(), Some(reference))?;
    }
    for doc in added {
        let digest = source_digest(&doc.source_file)?;
        if !upserted_sources.contains(&digest) {
            return Err(CoreError::InvalidContract(
                "lexical: Native added document has no upserted source binding".into(),
            ));
        }
    }
    // Retirements are one bounded delta. Publish them into the private root
    // before writing additions so route/current edit maps never accumulate
    // across a full bootstrap corpus.
    next.source = apply_edits(store, lease, &next.source, source_map)?;
    next.current = apply_edits(store, lease, &next.current, current_map)?;
    next.membership = apply_edits(store, lease, &next.membership, membership_map)?;
    next.segments = apply_edits(store, lease, &next.segments, segment_map)?;
    next.gram_routes = apply_edits(store, lease, &next.gram_routes, gram_map)?;
    next.term_routes = apply_edits(store, lease, &next.term_routes, term_map)?;
    next.compaction_due = apply_edits(store, lease, &next.compaction_due, due_map)?;
    let (
        consumed,
        mut posting_stats,
        mut compaction_rederived_docs,
        mut compaction_rederived_text_bytes,
    ) = compact_due_segments(
        store,
        lease,
        &mut next,
        target_generation,
        added,
        prepared,
        base,
        policy,
    )?;
    let mut bootstrap = if consumed == 0
        && !added.is_empty()
        && next.current.entries == 0
        && next.membership.entries == 0
        && next.segments.entries == 0
        && next.gram_routes.entries == 0
        && next.term_routes.entries == 0
        && next.compaction_due.entries == 0
    {
        store.flush_adoption_batch()?;
        Some(NativeBootstrapMaps::new()?)
    } else {
        None
    };
    for group in segment_groups(added.get(consumed..).ok_or_else(|| {
        CoreError::InvalidContract("lexical: Native consumed beyond added documents".into())
    })?) {
        let group = group?;
        // Tokenization is bounded by one segment cohort; release the write
        // gate while building its postings, then reacquire on the next write.
        store.flush_adoption_batch()?;
        let segment = BuiltSegment::build(next.next_segment_id, target_generation, group)?;
        let mut current_map = BTreeMap::new();
        let mut membership_map = BTreeMap::new();
        let mut segment_map = BTreeMap::new();
        let mut gram_map = BTreeMap::new();
        let mut term_map = BTreeMap::new();
        posting_stats.add(write_segment(
            store,
            lease,
            &next,
            &segment,
            group,
            &mut current_map,
            &mut membership_map,
            &mut segment_map,
            &mut gram_map,
            &mut term_map,
            None,
        )?)?;
        if let Some(scratch) = &mut bootstrap {
            // Keep private SQLite work outside the durable adoption gate.
            store.flush_adoption_batch()?;
            scratch.stage(BootstrapMap::Current, current_map)?;
            scratch.stage(BootstrapMap::Membership, membership_map)?;
            scratch.stage(BootstrapMap::Segments, segment_map)?;
            scratch.stage(BootstrapMap::GramRoutes, gram_map)?;
            scratch.stage(BootstrapMap::TermRoutes, term_map)?;
        } else {
            next.current = apply_edits(store, lease, &next.current, current_map)?;
            next.membership = apply_edits(store, lease, &next.membership, membership_map)?;
            next.segments = apply_edits(store, lease, &next.segments, segment_map)?;
            next.gram_routes = apply_edits(store, lease, &next.gram_routes, gram_map)?;
            next.term_routes = apply_edits(store, lease, &next.term_routes, term_map)?;
        }
        next.next_segment_id = next.next_segment_id.checked_add(1).ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native segment ID overflow".into())
        })?;
    }
    if let Some(scratch) = bootstrap {
        store.flush_adoption_batch()?;
        let roots = scratch.finish(store, lease)?;
        next.current = roots.current;
        next.membership = roots.membership;
        next.segments = roots.segments;
        next.gram_routes = roots.gram_routes;
        next.term_routes = roots.term_routes;
    }
    if let Some(clean_merge) = clean_merge {
        let (clean_stats, clean_docs, clean_text_bytes) =
            compact_clean_runs(store, lease, &mut next, target_generation, clean_merge)?;
        posting_stats.add(clean_stats)?;
        compaction_rederived_docs = compaction_rederived_docs
            .checked_add(clean_docs)
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: compaction docs overflow".into())
            })?;
        compaction_rederived_text_bytes = compaction_rederived_text_bytes
            .checked_add(clean_text_bytes)
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: compaction bytes overflow".into())
            })?;
    }
    // Run ownership follows the final physical segment diff, including any
    // compaction consumed by this publication. The changed-key walk skips
    // identical authenticated map subtrees; bulk clear/bootstrap and a
    // genuinely corpus-sized delta can still visit the full segment set.
    // Release the adoption write gate before reading changed descriptors and
    // posting closures. Run-map writes reacquire it after this read phase.
    store.flush_adoption_batch()?;
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    store.map_changed_keys(
        &inherited.segments,
        &next.segments,
        None,
        &mut |key, before, after| {
            if let Some(reference) = before {
                removed.push(SegmentRemoval {
                    key: key.to_vec(),
                    reference: reference.clone(),
                });
            }
            if let Some(reference) = after {
                changed.push((key.to_vec(), reference.clone()));
            }
            Ok(())
        },
    )?;
    let old_by_key = removed
        .iter()
        .map(|row| (row.key.as_slice(), &row.reference))
        .collect::<BTreeMap<_, _>>();
    let mut created = Vec::new();
    for (key, reference) in changed {
        let inherited_bytes = old_by_key
            .get(key.as_slice())
            .map(|old_ref| {
                inherit_unchanged_posting_index_bytes(
                    store, &inherited, &next, &key, old_ref, &reference,
                )
            })
            .transpose()?
            .flatten();
        let index_encoded_bytes = if let Some(bytes) = inherited_bytes {
            bytes
        } else {
            let index_roots = native_segment_index_roots(store, &next, &key, &reference)?;
            segment_index_encoded_bytes(
                store,
                &index_roots,
                None,
                u64::try_from(MAX_CLOSURE_REFS).map_err(|error| {
                    CoreError::InvalidContract(format!("lexical: closure bound width: {error}"))
                })?,
            )?
        };
        created.push(SegmentInput {
            key,
            reference,
            index_encoded_bytes,
        });
    }
    next.run_registry = inherited
        .run_registry
        .apply(store, lease, &removed, &created)?
        .with_selection_progress(selection_progress)?;
    next.live_docs = next.current.entries;
    next.validate()?;
    let shards_written = next
        .next_segment_id
        .saturating_sub(inherited.next_segment_id);
    let receipt = TextAuthorityWriteReceipt {
        docs_derived: u64::try_from(added.len()).map_err(|error| {
            CoreError::InvalidContract(format!("lexical: Native added count width: {error}"))
        })?,
        docs_retired: if clear_all {
            inherited.live_docs
        } else {
            u64::try_from(retired.len()).map_err(|error| {
                CoreError::InvalidContract(format!("lexical: Native retired count width: {error}"))
            })?
        },
        compaction_rederived_docs,
        compaction_rederived_text_bytes,
        shards_written,
        shards_inherited: next.segments.entries.saturating_sub(shards_written),
        posting_pages_written: posting_stats.pages,
        oversized_posting_pages: posting_stats.oversized_pages,
        oversized_posting_bytes: posting_stats.oversized_bytes,
    };
    Ok((next, receipt))
}

/// An inline descriptor rewrite may reuse the old authenticated posting subtotal.
///
/// The only charged index object that changed must be the segment
/// descriptor itself. Multi-block descriptors fall back because their blob
/// children may share an object with the immutable inventory closure.
fn inherit_unchanged_posting_index_bytes(
    store: &ObjectStore,
    old: &NativeRoot,
    next: &NativeRoot,
    key: &[u8],
    old_ref: &BlobRef,
    next_ref: &BlobRef,
) -> Result<Option<u64>, CoreError> {
    let id = u64::from_be_bytes(key.try_into().map_err(|_error| {
        CoreError::InvalidContract("lexical: rewritten segment key width differs".into())
    })?);
    let old_descriptor = read_segment(store, old, id)?;
    let next_descriptor = read_segment(store, next, id)?;
    if old_descriptor.logical_ids != next_descriptor.logical_ids
        || old_descriptor.route_inventory != next_descriptor.route_inventory
    {
        return Ok(None);
    }
    let Some(inventory) = next_descriptor.route_inventory.as_ref() else {
        return Ok(None);
    };
    if old_ref.root.kind != ObjectKind::Segment
        || next_ref.root.kind != ObjectKind::Segment
        || store.object_dependencies(&old_ref.root, None)? != vec![inventory.root.clone()]
        || store.object_dependencies(&next_ref.root, None)? != vec![inventory.root.clone()]
    {
        return Ok(None);
    }
    for route in read_route_inventory(store, &next_descriptor)? {
        let (old_map, next_map, kind) = match route.first().copied() {
            Some(GRAM_RAW | GRAM_FOLDED) => (
                &old.gram_routes,
                &next.gram_routes,
                ObjectKind::NativeTrigrams,
            ),
            Some(TERM_RAW | TERM_FOLDED) => (
                &old.term_routes,
                &next.term_routes,
                ObjectKind::NativePositions,
            ),
            _ => {
                return Err(CoreError::InvalidContract(
                    "lexical: rewritten segment route surface differs".into(),
                ));
            }
        };
        let before = store.map_get(old_map, &route, None)?;
        let after = store.map_get(next_map, &route, None)?;
        if before.is_none()
            || before != after
            || before.as_ref().is_some_and(|row| row.root.kind != kind)
        {
            return Ok(None);
        }
    }
    let old_charge = old
        .run_registry
        .verify_segment_owner(store, key, old_ref, None)?;
    let charged = old_charge
        .checked_sub(old_ref.root.bytes)
        .and_then(|subtotal| subtotal.checked_add(next_ref.root.bytes))
        .ok_or_else(|| {
            CoreError::InvalidContract("lexical: unchanged posting charge overflows".into())
        })?;
    Ok(Some(charged))
}

/// Logical encoded bytes owned by one Native segment.
///
/// Current/source/body
/// objects and map pages are outside this posting-run counter and must be
/// accounted by the service disk authority. Route inventory is authenticated and every page
/// named by it must still be reachable through the target route map.
fn native_segment_index_roots(
    store: &ObjectStore,
    root: &NativeRoot,
    key: &[u8],
    reference: &BlobRef,
) -> Result<Vec<ObjectRef>, CoreError> {
    let id = u64::from_be_bytes(key.try_into().map_err(|_error| {
        CoreError::InvalidContract("lexical: Native run segment key width differs".into())
    })?);
    if reference.root.kind != ObjectKind::Segment {
        return Err(CoreError::InvalidContract(
            "lexical: Native run descriptor kind differs".into(),
        ));
    }
    let descriptor = read_segment(store, root, id)?;
    let inventory = descriptor.route_inventory.as_ref().ok_or_else(|| {
        CoreError::InvalidContract("lexical: Native run route inventory absent".into())
    })?;
    let mut roots = BTreeSet::from([reference.root.clone(), inventory.root.clone()]);
    for route in read_route_inventory(store, &descriptor)? {
        let (map, kind) = match route.first().copied() {
            Some(GRAM_RAW | GRAM_FOLDED) => (&root.gram_routes, ObjectKind::NativeTrigrams),
            Some(TERM_RAW | TERM_FOLDED) => (&root.term_routes, ObjectKind::NativePositions),
            _ => {
                return Err(CoreError::InvalidContract(
                    "lexical: Native run route surface differs".into(),
                ));
            }
        };
        let posting = store.map_get(map, &route, None)?.ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native run posting route absent".into())
        })?;
        if posting.root.kind != kind {
            return Err(CoreError::InvalidContract(
                "lexical: Native run posting page kind differs".into(),
            ));
        }
        let _inserted = roots.insert(posting.root);
    }
    Ok(roots.into_iter().collect())
}

pub(crate) fn read_route_inventory(
    store: &ObjectStore,
    descriptor: &SegmentDescriptor,
) -> Result<Vec<Vec<u8>>, CoreError> {
    read_route_inventory_budget(store, descriptor, None)
}

pub(crate) fn read_route_inventory_budget(
    store: &ObjectStore,
    descriptor: &SegmentDescriptor,
    budget: Option<&RequestBudgetV1>,
) -> Result<Vec<Vec<u8>>, CoreError> {
    let reference = descriptor.route_inventory.as_ref().ok_or_else(|| {
        CoreError::InvalidContract("lexical: Native segment lacks route inventory".into())
    })?;
    if reference.root.kind != ObjectKind::Record {
        return Err(CoreError::InvalidContract(
            "lexical: Native route inventory kind differs".into(),
        ));
    }
    let bytes = store.read_blob(reference, 128 * 1024 * 1024, budget)?;
    let (version, routes): (u16, Vec<Vec<u8>>) = crate::channel_payloads::decode_cbor_exact(&bytes)
        .map_err(|error| {
            CoreError::Storage(format!("lexical: decode Native route inventory: {error}"))
        })?;
    if version != 1
        || routes
            .windows(2)
            .any(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a, b)| a >= b))
        || routes
            .iter()
            .any(|route| route.len() < 9 || !route.ends_with(&descriptor.id.to_be_bytes()))
    {
        return Err(CoreError::InvalidContract(
            "lexical: Native route inventory is not canonical".into(),
        ));
    }
    Ok(routes)
}

pub(crate) fn read_segment(
    store: &ObjectStore,
    root: &NativeRoot,
    id: u64,
) -> Result<SegmentDescriptor, CoreError> {
    read_segment_budget(store, root, id, None)
}

fn read_segment_budget(
    store: &ObjectStore,
    root: &NativeRoot,
    id: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<SegmentDescriptor, CoreError> {
    let reference = store
        .map_get(&root.segments, &segment_key(id), budget)?
        .ok_or_else(|| CoreError::InvalidContract("lexical: Native segment is missing".into()))?;
    if reference.root.kind != ObjectKind::Segment {
        return Err(CoreError::InvalidContract(
            "lexical: Native segment kind differs".into(),
        ));
    }
    let bytes = store.read_blob(&reference, 64 * 1024, budget)?;
    let descriptor = SegmentDescriptor::decode(&bytes)?;
    if descriptor.id != id {
        return Err(CoreError::InvalidContract(
            "lexical: Native segment key/value differs".into(),
        ));
    }
    Ok(descriptor)
}

pub(crate) fn source_ids(
    store: &ObjectStore,
    root: &NativeRoot,
    source: &SourceFileKey,
) -> Result<Vec<u64>, CoreError> {
    let digest = source_digest(source)?;
    let mut ids = Vec::new();
    store.map_scan(
        &root.membership,
        &digest,
        prefix_upper(&digest).as_deref(),
        None,
        &mut |key, reference| {
            if key.len() != 40 || !key.starts_with(&digest) {
                return Err(CoreError::InvalidContract(
                    "lexical: Native membership key differs".into(),
                ));
            }
            let id = u64::from_be_bytes(
                key.get(32..40)
                    .ok_or_else(|| {
                        CoreError::InvalidContract(
                            "lexical: Native membership ID suffix missing".into(),
                        )
                    })?
                    .try_into()
                    .map_err(|error| {
                        CoreError::InvalidContract(format!(
                            "lexical: Native membership ID width: {error}"
                        ))
                    })?,
            );
            let row = CurrentRecord::decode(&store.read_blob(
                reference,
                MAX_CURRENT_RECORD_BYTES,
                None,
            )?)?;
            if row.id != id || row.source != *source || ids.last().is_some_and(|prior| *prior >= id)
            {
                return Err(CoreError::InvalidContract(
                    "lexical: Native membership record differs from source".into(),
                ));
            }
            ids.push(id);
            Ok(())
        },
    )?;
    Ok(ids)
}

fn prepared_matches_target(
    store: &ObjectStore,
    root: &NativeRoot,
    packet: &PreparedNativeCompaction,
) -> Result<bool, CoreError> {
    let due = store.map_scan_limit(
        &root.compaction_due,
        &[0],
        None,
        packet.selected.len(),
        None,
    )?;
    if due.len() != packet.selected.len() || packet.current_witnesses.len() != packet.docs.len() {
        return Ok(false);
    }
    for ((key, reference), (id, expected, descriptor)) in due.iter().zip(&packet.selected) {
        if key.as_slice() != segment_key(*id).as_slice()
            || reference != expected
            || store.map_get(&root.segments, key, None)?.as_ref() != Some(expected)
            || read_segment(store, root, *id)?.encode()? != descriptor.encode()?
        {
            return Ok(false);
        }
    }
    for ((id, current_ref, membership_ref), doc) in
        packet.current_witnesses.iter().zip(&packet.docs)
    {
        if *id != doc.doc_id
            || store
                .map_get(&root.current, &current_key(*id), None)?
                .as_ref()
                != Some(current_ref)
            || store
                .map_get(
                    &root.membership,
                    &membership_key(&source_digest(&doc.source_file)?, *id),
                    None,
                )?
                .as_ref()
                != Some(membership_ref)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Pay at most two queued segment debts on one publication.
///
/// Every selected live document is re-derived from its committed body and current locator;
/// the replacement is published in the same immutable-root transaction as
/// removal of the old route inventory.
fn compact_due_segments(
    store: &ObjectStore,
    lease: &AttemptLease,
    root: &mut NativeRoot,
    generation: ManifestGeneration,
    added: &[AddedTextDoc],
    prepared: Option<PreparedNativeCompaction>,
    base: Option<&NativeRoot>,
    policy: Option<&LexicalMaintenancePolicy>,
) -> Result<(usize, PostingPageStats, u64, u64), CoreError> {
    // The prior map edits are durable before reading and re-deriving old
    // bodies; this bounded compaction preparation does not hold the gate.
    store.flush_adoption_batch()?;
    let due = store.map_scan_limit(&root.compaction_due, &[0], None, 2, None)?;
    if due.is_empty() {
        return Ok((0, PostingPageStats::default(), 0, 0));
    }
    let prepared = match prepared {
        Some(packet)
            if packet.policy_valid(base, policy)?
                && prepared_matches_target(store, root, &packet)? =>
        {
            Some(packet)
        }
        _ => None,
    };
    let (selected, mut docs) = if let Some(packet) = prepared {
        let selected = packet
            .selected
            .into_iter()
            .map(|(_, _, descriptor)| descriptor)
            .collect::<Vec<_>>();
        (selected, packet.docs)
    } else {
        let mut selected = Vec::new();
        let mut docs = Vec::new();
        for (key, reference) in due {
            let id = u64::from_be_bytes(key.as_slice().try_into().map_err(|_error| {
                CoreError::InvalidContract("lexical: Native compaction key width differs".into())
            })?);
            let segment_ref = store.map_get(&root.segments, &key, None)?.ok_or_else(|| {
                CoreError::InvalidContract("lexical: due segment vanished".into())
            })?;
            if segment_ref != reference {
                return Err(CoreError::InvalidContract(
                    "lexical: due segment version differs from committed descriptor".into(),
                ));
            }
            let descriptor = read_segment(store, root, id)?;
            let segment_docs = live_segment_docs(store, root, &descriptor)?;
            let added_bytes = segment_docs
                .iter()
                .try_fold(0_usize, |sum, doc| sum.checked_add(doc.text.len()))
                .ok_or_else(|| {
                    CoreError::InvalidContract("lexical: compacted byte count overflow".into())
                })?;
            let prior_bytes = docs
                .iter()
                .try_fold(0_usize, |sum: usize, doc: &AddedTextDoc| {
                    sum.checked_add(doc.text.len())
                })
                .ok_or_else(|| {
                    CoreError::InvalidContract("lexical: compacted byte count overflow".into())
                })?;
            if !selected.is_empty()
                && (docs.len().saturating_add(segment_docs.len()) > SEGMENT_DOCS
                    || prior_bytes.saturating_add(added_bytes) > SEGMENT_TARGET_TEXT_BYTES)
            {
                break;
            }
            docs.extend(segment_docs);
            selected.push(descriptor);
        }
        (selected, docs)
    };
    if selected.is_empty() || docs.is_empty() {
        return Err(CoreError::InvalidContract(
            "lexical: Native compaction selected no live segment".into(),
        ));
    }
    let compaction_rederived_docs = u64::try_from(docs.len()).map_err(|error| {
        CoreError::InvalidContract(format!("lexical: compaction document count width: {error}"))
    })?;
    let compaction_rederived_text_bytes = docs.iter().try_fold(0_u64, |sum, doc| {
        let bytes = u64::try_from(doc.text.len()).map_err(|error| {
            CoreError::InvalidContract(format!("lexical: compaction text width: {error}"))
        })?;
        sum.checked_add(bytes).ok_or_else(|| {
            CoreError::InvalidContract("lexical: compaction text byte count overflow".into())
        })
    })?;
    let compacted_segments = u64::try_from(selected.len()).map_err(|error| {
        CoreError::InvalidContract(format!("lexical: compaction segment count width: {error}"))
    })?;
    store.record_compaction(
        compacted_segments,
        compaction_rederived_docs,
        compaction_rederived_text_bytes,
    );
    let existing_ids = docs.iter().map(|doc| doc.doc_id).collect::<BTreeSet<_>>();
    let mut consumed = 0_usize;
    let mut text_bytes = docs.iter().try_fold(0_usize, |sum, doc| {
        sum.checked_add(doc.text.len()).ok_or_else(|| {
            CoreError::InvalidContract("lexical: compacted byte count overflow".into())
        })
    })?;
    while let Some(doc) = added.get(consumed) {
        let next_bytes = text_bytes.checked_add(doc.text.len()).ok_or_else(|| {
            CoreError::InvalidContract("lexical: compacted byte count overflow".into())
        })?;
        if docs.len() >= SEGMENT_DOCS || next_bytes > SEGMENT_TARGET_TEXT_BYTES {
            break;
        }
        docs.push(doc.clone());
        text_bytes = next_bytes;
        consumed = consumed.saturating_add(1);
    }
    docs.sort_unstable_by_key(|doc| doc.doc_id);
    if docs.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(a, b)| a.doc_id == b.doc_id)
    }) {
        return Err(CoreError::InvalidContract(
            "lexical: compaction source segments share a live logical ID".into(),
        ));
    }
    store.flush_adoption_batch()?;
    let replacement = BuiltSegment::build(root.next_segment_id, generation, &docs)?;
    let mut current_map = BTreeMap::new();
    let mut membership_map = BTreeMap::new();
    let mut segment_map = BTreeMap::new();
    let mut gram_map = BTreeMap::new();
    let mut term_map = BTreeMap::new();
    let mut due_map = BTreeMap::new();
    let posting_stats = write_segment(
        store,
        lease,
        root,
        &replacement,
        &docs,
        &mut current_map,
        &mut membership_map,
        &mut segment_map,
        &mut gram_map,
        &mut term_map,
        Some(&existing_ids),
    )?;
    for descriptor in selected {
        for route in read_route_inventory(store, &descriptor)? {
            let target = match route.first().copied() {
                Some(GRAM_RAW | GRAM_FOLDED) => &mut gram_map,
                Some(TERM_RAW | TERM_FOLDED) => &mut term_map,
                _ => {
                    return Err(CoreError::InvalidContract(
                        "lexical: compacted route has unknown surface".into(),
                    ));
                }
            };
            checked_insert(target, route, None)?;
        }
        checked_insert(&mut segment_map, segment_key(descriptor.id).to_vec(), None)?;
        checked_insert(&mut due_map, segment_key(descriptor.id).to_vec(), None)?;
    }
    root.current = apply_edits(store, lease, &root.current, current_map)?;
    root.membership = apply_edits(store, lease, &root.membership, membership_map)?;
    root.segments = apply_edits(store, lease, &root.segments, segment_map)?;
    root.gram_routes = apply_edits(store, lease, &root.gram_routes, gram_map)?;
    root.term_routes = apply_edits(store, lease, &root.term_routes, term_map)?;
    root.compaction_due = apply_edits(store, lease, &root.compaction_due, due_map)?;
    root.next_segment_id = root.next_segment_id.checked_add(1).ok_or_else(|| {
        CoreError::InvalidContract("lexical: Native compaction segment ID overflow".into())
    })?;
    root.live_docs = root.current.entries;
    root.validate_before_run_registry()?;
    Ok((
        consumed,
        posting_stats,
        compaction_rederived_docs,
        compaction_rederived_text_bytes,
    ))
}

/// Rebuild selected healthy runs into fresh physical segment IDs.
///
/// The input body carrier was bounded and authenticated before the first
/// publication write. Output groups reuse the canonical segment builder and
/// writer, and each map edit is bounded by one output or retired segment.
fn compact_clean_runs(
    store: &ObjectStore,
    lease: &AttemptLease,
    root: &mut NativeRoot,
    generation: ManifestGeneration,
    prepared: PreparedCleanMerge,
) -> Result<(PostingPageStats, u64, u64), CoreError> {
    store.flush_adoption_batch()?;
    for (reference, descriptor) in &prepared.selected {
        let key = segment_key(descriptor.id);
        if store.map_get(&root.segments, &key, None)?.as_ref() != Some(reference)
            || store.map_get(&root.compaction_due, &key, None)?.is_some()
            || descriptor.live_locals.len() != descriptor.logical_ids.len()
        {
            return Err(CoreError::InvalidContract(
                "lexical: selected clean merge cohort drifted before publication".into(),
            ));
        }
    }
    let rederived_docs = u64::try_from(prepared.docs.len()).map_err(|error| {
        CoreError::InvalidContract(format!("lexical: clean merge docs width: {error}"))
    })?;
    let input_segments = u64::try_from(prepared.selected.len()).map_err(|error| {
        CoreError::InvalidContract(format!("lexical: clean merge segment count width: {error}"))
    })?;
    store.record_compaction(input_segments, rederived_docs, prepared.text_bytes);
    let mut posting_stats = PostingPageStats::default();
    for group in segment_groups(&prepared.docs) {
        let group = group?;
        store.flush_adoption_batch()?;
        let replacement = BuiltSegment::build(root.next_segment_id, generation, group)?;
        let existing_ids = group.iter().map(|doc| doc.doc_id).collect::<BTreeSet<_>>();
        let mut current_map = BTreeMap::new();
        let mut membership_map = BTreeMap::new();
        let mut segment_map = BTreeMap::new();
        let mut gram_map = BTreeMap::new();
        let mut term_map = BTreeMap::new();
        posting_stats.add(write_segment(
            store,
            lease,
            root,
            &replacement,
            group,
            &mut current_map,
            &mut membership_map,
            &mut segment_map,
            &mut gram_map,
            &mut term_map,
            Some(&existing_ids),
        )?)?;
        root.current = apply_edits(store, lease, &root.current, current_map)?;
        root.membership = apply_edits(store, lease, &root.membership, membership_map)?;
        root.segments = apply_edits(store, lease, &root.segments, segment_map)?;
        root.gram_routes = apply_edits(store, lease, &root.gram_routes, gram_map)?;
        root.term_routes = apply_edits(store, lease, &root.term_routes, term_map)?;
        root.next_segment_id = root.next_segment_id.checked_add(1).ok_or_else(|| {
            CoreError::InvalidContract("lexical: clean merge segment ID overflow".into())
        })?;
    }
    for (reference, descriptor) in prepared.selected {
        let key = segment_key(descriptor.id);
        if store.map_get(&root.segments, &key, None)?.as_ref() != Some(&reference) {
            return Err(CoreError::InvalidContract(
                "lexical: clean merge input segment changed during output".into(),
            ));
        }
        let mut gram_map = BTreeMap::new();
        let mut term_map = BTreeMap::new();
        for route in read_route_inventory(store, &descriptor)? {
            match route.first().copied() {
                Some(GRAM_RAW | GRAM_FOLDED) => checked_insert(&mut gram_map, route, None)?,
                Some(TERM_RAW | TERM_FOLDED) => checked_insert(&mut term_map, route, None)?,
                _ => {
                    return Err(CoreError::InvalidContract(
                        "lexical: clean merge route surface differs".into(),
                    ));
                }
            }
        }
        let mut segment_map = BTreeMap::new();
        checked_insert(&mut segment_map, key.to_vec(), None)?;
        root.segments = apply_edits(store, lease, &root.segments, segment_map)?;
        root.gram_routes = apply_edits(store, lease, &root.gram_routes, gram_map)?;
        root.term_routes = apply_edits(store, lease, &root.term_routes, term_map)?;
    }
    root.live_docs = root.current.entries;
    root.validate_before_run_registry()?;
    Ok((posting_stats, rederived_docs, prepared.text_bytes))
}

fn live_segment_docs(
    store: &ObjectStore,
    root: &NativeRoot,
    descriptor: &SegmentDescriptor,
) -> Result<Vec<AddedTextDoc>, CoreError> {
    live_segment_docs_admitted(store, root, descriptor, None, None)
}

fn live_segment_docs_admitted(
    store: &ObjectStore,
    root: &NativeRoot,
    descriptor: &SegmentDescriptor,
    mut admission: Option<&mut ScratchAdmission>,
    budget: Option<&RequestBudgetV1>,
) -> Result<Vec<AddedTextDoc>, CoreError> {
    if let Some(admission) = admission.as_deref_mut() {
        admission.charge(
            u64::try_from(descriptor.live_locals.len())
                .map_err(|_error| compaction_scratch_limit())?
                .checked_mul(
                    u64::try_from(std::mem::size_of::<AddedTextDoc>())
                        .map_err(|_error| compaction_scratch_limit())?,
                )
                .ok_or_else(compaction_scratch_limit)?,
        )?;
    }
    let mut docs = Vec::new();
    docs.try_reserve_exact(descriptor.live_locals.len())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: reserve Native compaction documents: {error}"
            ))
        })?;
    for local in &descriptor.live_locals {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:native:prepare:body")?;
        }
        let id = *descriptor
            .logical_ids
            .get(usize::try_from(*local).map_err(|_error| {
                CoreError::InvalidContract("lexical: compaction local ID width differs".into())
            })?)
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: compaction local ID absent".into())
            })?;
        let (current_ref, current) = read_current_with_ref_budget(store, root, id, budget)?
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: compaction live current ID absent".into())
            })?;
        if let Some(admission) = admission.as_deref_mut() {
            admission.charge(current_ref.bytes)?;
        }
        if current.segment_key != descriptor.id || current.local_id != *local {
            return Err(CoreError::InvalidContract(
                "lexical: compaction current locator differs".into(),
            ));
        }
        if current.body.root.kind != ObjectKind::NativeBody {
            return Err(CoreError::InvalidContract(
                "lexical: compaction body kind differs".into(),
            ));
        }
        if let Some(admission) = admission.as_deref_mut() {
            // The raw logical blob and the decoded candidate/indexed strings
            // each require at most the committed logical byte count. Charge
            // both before either body buffer is allocated.
            admission.charge(
                current
                    .body
                    .bytes
                    .checked_mul(2)
                    .ok_or_else(compaction_scratch_limit)?,
            )?;
        }
        let bytes = store.read_blob(&current.body, MAX_BODY_BYTES, budget)?;
        let (candidate_id, indexed, _folded) = match admission.as_deref() {
            Some(admission) => decode_body_for_preparation(
                &bytes,
                usize::try_from(admission.charged).map_err(|_error| compaction_scratch_limit())?,
                usize::try_from(admission.limit).map_err(|_error| compaction_scratch_limit())?,
            )?,
            None => decode_body(&bytes)?,
        };
        let indexed_sha256: [u8; 32] = Sha256::digest(indexed.as_bytes()).into();
        if candidate_id != current.candidate_id || indexed_sha256 != current.indexed_sha256 {
            return Err(CoreError::InvalidContract(
                "lexical: compaction body differs from current identity".into(),
            ));
        }
        let digest = source_digest(&current.source)?;
        let member = store
            .map_get(&root.membership, &membership_key(&digest, id), budget)?
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: compacted membership absent".into())
            })?;
        let member_record =
            CurrentRecord::decode(&store.read_blob(&member, MAX_CURRENT_RECORD_BYTES, budget)?)?;
        if member_record.id != id
            || member_record.segment_key != descriptor.id
            || member_record.local_id != *local
            || member_record.source != current.source
            || member_record.candidate_id != current.candidate_id
            || member_record.indexed_sha256 != current.indexed_sha256
        {
            return Err(CoreError::InvalidContract(
                "lexical: compacted membership differs from current".into(),
            ));
        }
        docs.push(AddedTextDoc {
            doc_id: id,
            source_file: current.source,
            candidate_id,
            text: indexed,
        });
    }
    Ok(docs)
}

fn segment_groups(
    docs: &[AddedTextDoc],
) -> impl Iterator<Item = Result<&[AddedTextDoc], CoreError>> {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start >= docs.len() {
            return None;
        }
        let mut end = start;
        let mut bytes = 0_usize;
        while end < docs.len() && end.saturating_sub(start) < SEGMENT_DOCS {
            let next = bytes.saturating_add(docs.get(end)?.text.len());
            if end > start && next > SEGMENT_TARGET_TEXT_BYTES {
                break;
            }
            bytes = next;
            end = end.saturating_add(1);
        }
        let group_start = start;
        start = end;
        Some(docs.get(group_start..end).ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: Native segment group outside added documents".into(),
            )
        }))
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "one segment write updates its coupled current, membership, segment and posting maps atomically"
)]
fn write_segment(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: &NativeRoot,
    segment: &BuiltSegment,
    docs: &[AddedTextDoc],
    current_map: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    membership_map: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    segment_map: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    gram_map: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    term_map: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    replace_existing: Option<&BTreeSet<u64>>,
) -> Result<PostingPageStats, CoreError> {
    let mut inventory = Vec::new();
    inventory
        .try_reserve(segment.grams.len().saturating_add(segment.terms.len()))
        .map_err(|error| {
            CoreError::Storage(format!("lexical: reserve Native route inventory: {error}"))
        })?;
    for posting in &segment.grams {
        let gram: [u8; 3] = posting.key.as_slice().try_into().map_err(|_error| {
            CoreError::InvalidContract("lexical: Native gram key width differs".into())
        })?;
        inventory.push(route_key(
            &gram_prefix(posting.surface, gram),
            segment.descriptor.id,
        ));
    }
    for posting in &segment.terms {
        let term = std::str::from_utf8(&posting.key).map_err(|error| {
            CoreError::InvalidContract(format!("lexical: Native term UTF-8: {error}"))
        })?;
        inventory.push(route_key(
            &term_prefix(posting.surface, term)?,
            segment.descriptor.id,
        ));
    }
    let inventory_count = inventory.len();
    inventory.sort_unstable();
    inventory.dedup();
    if inventory.len() != inventory_count {
        return Err(CoreError::InvalidContract(
            "lexical: duplicate Native segment route".into(),
        ));
    }
    let inventory_bytes =
        crate::channel_payloads::encode_cbor(&(1_u16, &inventory), "Native route inventory")?;
    let inventory_ref = store.put_blob(lease, ObjectKind::Record, &[], &inventory_bytes)?;
    let mut descriptor = segment.descriptor.clone();
    descriptor.route_inventory = Some(inventory_ref.clone());
    let descriptor_bytes = descriptor.encode()?;
    let descriptor_ref = store.put_blob(
        lease,
        ObjectKind::Segment,
        &[inventory_ref.root],
        &descriptor_bytes,
    )?;
    checked_insert(
        segment_map,
        segment_key(segment.descriptor.id).to_vec(),
        Some(descriptor_ref),
    )?;
    for body in &segment.bodies {
        let input = docs
            .get(usize::try_from(body.local_id).map_err(|error| {
                CoreError::InvalidContract(format!("lexical: Native body local width: {error}"))
            })?)
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: Native body local missing".into())
            })?;
        let expected_logical_id = input.doc_id;
        let expected_candidate_id = &input.candidate_id;
        let logical_id_matches = expected_logical_id == body.logical_id;
        let candidate_id_matches = expected_candidate_id == &body.candidate_id;
        if !logical_id_matches || !candidate_id_matches {
            return Err(CoreError::InvalidContract(
                "lexical: Native body and input differ".into(),
            ));
        }
        let existing = read_current(store, base, body.logical_id)?;
        match (replace_existing, existing) {
            (None, None) => {}
            (None, Some(_)) => {
                return Err(CoreError::InvalidContract(
                    "lexical: Native allocation reuses an existing logical ID".into(),
                ));
            }
            (Some(expected), Some(prior)) if expected.contains(&body.logical_id) => {
                let source_matches = prior.source == input.source_file;
                let candidate_matches = prior.candidate_id == body.candidate_id;
                if !source_matches
                    || !candidate_matches
                    || prior.indexed_sha256 != body.indexed_sha256
                {
                    return Err(CoreError::InvalidContract(
                        "lexical: Native compaction input differs from current ID".into(),
                    ));
                }
            }
            (Some(expected), None) if !expected.contains(&body.logical_id) => {}
            (Some(_), _) => {
                return Err(CoreError::InvalidContract(
                    "lexical: Native compaction current membership differs".into(),
                ));
            }
        }
        let source = source_digest(&input.source_file)?;
        let body_ref = store.put_blob(lease, ObjectKind::NativeBody, &[], &body.bytes)?;
        let record = CurrentRecord {
            id: body.logical_id,
            source: input.source_file.clone(),
            candidate_id: body.candidate_id.clone(),
            indexed_sha256: body.indexed_sha256,
            body: body_ref.clone(),
            segment_key: segment.descriptor.id,
            local_id: body.local_id,
        };
        let record_ref = store.put_blob(
            lease,
            ObjectKind::Record,
            &[body_ref.root],
            &record.encode()?,
        )?;
        checked_insert(
            current_map,
            current_key(body.logical_id).to_vec(),
            Some(record_ref.clone()),
        )?;
        checked_insert(
            membership_map,
            membership_key(&source, body.logical_id),
            Some(record_ref),
        )?;
    }
    let mut posting_stats = write_posting_pages(
        store,
        lease,
        segment.descriptor.id,
        &segment.grams,
        ObjectKind::NativeTrigrams,
        gram_map,
    )?;
    posting_stats.add(write_posting_pages(
        store,
        lease,
        segment.descriptor.id,
        &segment.terms,
        ObjectKind::NativePositions,
        term_map,
    )?)?;
    Ok(posting_stats)
}

fn write_posting_pages(
    store: &ObjectStore,
    lease: &AttemptLease,
    segment_key: u64,
    postings: &[BuiltPosting],
    kind: ObjectKind,
    edits: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
) -> Result<PostingPageStats, CoreError> {
    let mut page = Vec::<&BuiltPosting>::new();
    let mut page_bytes = HEADER_BYTES;
    let mut stats = PostingPageStats::default();
    for posting in postings {
        let row_bytes = PostingPage::encoded_len_for(posting)?;
        let next_bytes = page_bytes.checked_add(row_bytes).ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native posting page length overflow".into())
        })?;
        if u64::try_from(HEADER_BYTES.saturating_add(row_bytes))
            .map_or(true, |len| len > MAX_POSTING_BYTES)
        {
            return Err(CoreError::InvalidContract(
                "lexical: Native posting exceeds admitted bound".into(),
            ));
        }
        if !page.is_empty() && next_bytes > TARGET_BYTES {
            stats.add(flush_posting_page(
                store,
                lease,
                segment_key,
                kind,
                &page,
                edits,
            )?)?;
            page.clear();
            page_bytes = HEADER_BYTES;
        }
        page_bytes = page_bytes.checked_add(row_bytes).ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native posting page length overflow".into())
        })?;
        page.try_reserve(1).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: reserve Native posting page rows: {error}"
            ))
        })?;
        page.push(posting);
    }
    if !page.is_empty() {
        stats.add(flush_posting_page(
            store,
            lease,
            segment_key,
            kind,
            &page,
            edits,
        )?)?;
    }
    Ok(stats)
}

fn flush_posting_page(
    store: &ObjectStore,
    lease: &AttemptLease,
    segment_key: u64,
    kind: ObjectKind,
    page: &[&BuiltPosting],
    edits: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
) -> Result<PostingPageStats, CoreError> {
    let bytes = PostingPage::encode(segment_key, page)?;
    if u64::try_from(bytes.len()).map_or(true, |len| len > MAX_POSTING_BYTES) {
        return Err(CoreError::InvalidContract(
            "lexical: Native posting page exceeds admitted bound".into(),
        ));
    }
    // An oversized singleton remains one bounded logical blob; the object
    // store splits its physical representation into fixed-size blocks.
    let reference = store.put_blob(lease, kind, &[], &bytes)?;
    for posting in page {
        let prefix = match kind {
            ObjectKind::NativeTrigrams => {
                if !matches!(posting.surface, GRAM_RAW | GRAM_FOLDED) {
                    return Err(CoreError::InvalidContract(
                        "lexical: Native gram surface differs".into(),
                    ));
                }
                let gram: [u8; 3] = posting.key.as_slice().try_into().map_err(|_error| {
                    CoreError::InvalidContract("lexical: Native gram key width differs".into())
                })?;
                gram_prefix(posting.surface, gram).to_vec()
            }
            ObjectKind::NativePositions => {
                if !matches!(posting.surface, TERM_RAW | TERM_FOLDED) {
                    return Err(CoreError::InvalidContract(
                        "lexical: Native term surface differs".into(),
                    ));
                }
                let term = std::str::from_utf8(&posting.key).map_err(|error| {
                    CoreError::InvalidContract(format!("lexical: Native term UTF-8: {error}"))
                })?;
                term_prefix(posting.surface, term)?
            }
            ObjectKind::FileBody
            | ObjectKind::FilePostings
            | ObjectKind::NativeBody
            | ObjectKind::Segment
            | ObjectKind::Record
            | ObjectKind::MapPage
            | ObjectKind::BlobBlock
            | ObjectKind::BlobNode
            | ObjectKind::AttemptBatch
            | ObjectKind::HardScanAnchor => {
                return Err(CoreError::InvalidContract(
                    "lexical: Native posting object kind differs".into(),
                ));
            }
        };
        checked_insert(
            edits,
            route_key(&prefix, segment_key),
            Some(reference.clone()),
        )?;
    }
    let oversized = bytes.len() > TARGET_BYTES;
    Ok(PostingPageStats {
        pages: 1,
        oversized_pages: u64::from(oversized),
        oversized_bytes: if oversized {
            u64::try_from(bytes.len()).map_err(|error| {
                CoreError::InvalidContract(format!(
                    "lexical: Native oversized page byte width: {error}"
                ))
            })?
        } else {
            0
        },
    })
}

#[cfg(test)]
mod fairness_tests {
    use super::{
        CLEAN_MERGE_PREPARE_CALLS, PreparedNativeCompaction, inherit_unchanged_posting_index_bytes,
        native_root_sha256, native_segment_index_roots, prepare_hard_scan_window, publish_delta,
        publish_delta_prepared_with_policy, read_current, read_route_inventory, read_segment,
        select_clean_merge_inputs,
    };
    use crate::file_authority::paged::{FileAuthorityMutation, build_for_seal};
    use crate::lexical_allocation::DocVersionReservation;
    use crate::lexical_publication::admit_maintenance_roots;
    use crate::lexical_runs::{
        RunSelectionProgress, SegmentInput, SegmentRemoval, segment_index_encoded_bytes,
    };
    use crate::lexical_store::{
        AttemptBinding, Authority, EligibilityWitness, MapEdit, ObjectStore, ScanLane,
    };
    use crate::text_authority::paged_manifest::NativeRoot;
    use crate::text_authority::paged_transition::verify_transition_for_seal_with_policy;
    use crate::text_authority::paged_verify::{
        verify_all_for_scrub, verify_changed_segments_for_seal,
    };
    use crate::text_authority::{AddedTextDoc, NativeAuthority, NativeSourceEdit};
    use quanta_index_contract::{
        ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SourceFileCoverage,
        SourceFileKey, SourceFileRevision, SymbolCoverage, SymbolNameSourcePolicyV1,
    };
    use quanta_index_core::lexical_maintenance::LexicalMaintenancePolicy;
    use quanta_index_core::{CoreError, RequestBudgetV1};
    use sha2::{Digest as _, Sha256};

    #[test]
    fn empty_native_packet_refused_under_configured_policy() {
        let base = NativeRoot::empty([9; 32]);
        let policy =
            LexicalMaintenancePolicy::new(2, 8, 1, u64::MAX, 2, 100, 100, 1_000).expect("policy");
        let packet = PreparedNativeCompaction {
            selected: Vec::new(),
            current_witnesses: Vec::new(),
            docs: Vec::new(),
            staging_charged_bytes: 100,
            policy_binding: None,
        }
        .bind_policy(&base, &policy)
        .expect("bind packet");
        assert!(
            !packet
                .policy_valid(Some(&base), Some(&policy))
                .expect("validate")
        );
        assert!(
            !packet
                .policy_valid(None, Some(&policy))
                .expect("missing base")
        );
    }

    #[test]
    fn descriptor_only_retirement_inherits_exact_posting_charge()
    -> Result<(), Box<dyn std::error::Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }
        macro_rules! check_ne {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left == *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left == *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let dir = tempfile::tempdir()?;
        let store = ObjectStore::open(dir.path())?;
        let source = |id: u64| -> Result<SourceFileKey, Box<dyn std::error::Error>> {
            Ok(SourceFileKey {
                source_repo_id: RepoId::new("source-repo")?,
                repo_relative_path: RepoRelativePath::new(format!("src/member-{id}.rs")),
            })
        };
        let coverage = |file: &SourceFileKey,
                        id: u64|
         -> Result<SourceFileCoverage, Box<dyn std::error::Error>> {
            Ok(SourceFileCoverage {
                source: SourceFileRevision {
                    file: file.clone(),
                    revision_id: RevisionId::new(format!("source-r{id}"))?,
                    source_sha256: Sha256::digest(format!("source {id}").as_bytes()).into(),
                },
                language: quanta_index_contract::lex::LanguageCode::new("rust")?,
                producer_policy_sha256: [1; 32],
                symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
                unit_set_sha256: [2; 32],
                text_admitted: true,
                symbols: SymbolCoverage::Complete { symbol_count: 0 },
            })
        };
        let source_one = source(1)?;
        let source_two = source(2)?;
        let added = [
            AddedTextDoc {
                doc_id: 1,
                source_file: source_one.clone(),
                candidate_id: "one".into(),
                text: "alpha spectral needle".into(),
            },
            AddedTextDoc {
                doc_id: 2,
                source_file: source_two.clone(),
                candidate_id: "two".into(),
                text: "beta spectral needle".into(),
            },
        ];
        let first_lease = store.begin_attempt(&binding(1)?, &[])?;
        let (old, _) = publish_delta(
            &store,
            &first_lease,
            None,
            &[
                NativeSourceEdit::Upsert(coverage(&source_one, 1)?),
                NativeSourceEdit::Upsert(coverage(&source_two, 2)?),
            ],
            &[],
            &added,
            false,
            ManifestGeneration::new(1),
            [9; 32],
        )?;
        check_eq!(
            old.segments.entries,
            1,
            "both documents must share one segment"
        );
        first_lease.finish_published(&old.root_objects())?;

        let second_lease = store.begin_attempt(&binding(2)?, &old.root_objects())?;
        let (next, _) = publish_delta(
            &store,
            &second_lease,
            Some(&old),
            &[NativeSourceEdit::Delete(source_one)],
            &[1],
            &[],
            false,
            ManifestGeneration::new(2),
            [9; 32],
        )?;
        check!(
            !next.is_cursor_only_successor_of(&old)?,
            "a source retirement cannot use cursor-only admission"
        );
        let key = 1_u64.to_be_bytes();
        let old_ref = store
            .map_get(&old.segments, &key, None)?
            .ok_or("old segment missing")?;
        let next_ref = store
            .map_get(&next.segments, &key, None)?
            .ok_or("rewritten segment missing")?;
        check_ne!(old_ref, next_ref);
        let inherited =
            inherit_unchanged_posting_index_bytes(&store, &old, &next, &key, &old_ref, &next_ref)?
                .ok_or("unchanged postings were not inherited")?;
        let full = segment_index_encoded_bytes(
            &store,
            &native_segment_index_roots(&store, &next, &key, &next_ref)?,
            None,
            u64::try_from(crate::lexical_store::MAX_CLOSURE_REFS)?,
        )?;
        check_eq!(
            inherited,
            full,
            "inherited charge must equal physical closure oracle"
        );
        check_eq!(
            next.run_registry
                .verify_segment_owner(&store, &key, &next_ref, None)?,
            full
        );
        let reservation = DocVersionReservation {
            first: 0,
            count: 0,
            scope_sha256: [9; 32],
        };
        let _seal_proof = verify_transition_for_seal_with_policy(
            &store,
            &next,
            Some(&old),
            &reservation,
            &[NativeSourceEdit::Delete(source(1)?)],
            &[1],
            &[],
            false,
            old.next_segment_id,
            None,
            None,
            None,
        )?;
        let mut dishonest_charge = next.clone();
        dishonest_charge.run_registry = old.run_registry.apply(
            &store,
            &second_lease,
            &[SegmentRemoval {
                key: key.to_vec(),
                reference: old_ref.clone(),
            }],
            &[SegmentInput {
                key: key.to_vec(),
                reference: next_ref.clone(),
                index_encoded_bytes: full.checked_add(1).ok_or("test charge overflow")?,
            }],
        )?;
        check!(
            verify_transition_for_seal_with_policy(
                &store,
                &dishonest_charge,
                Some(&old),
                &reservation,
                &[NativeSourceEdit::Delete(source(1)?)],
                &[1],
                &[],
                false,
                old.next_segment_id,
                None,
                None,
                None,
            )
            .is_err(),
            "independent transition must reject a forged target charge"
        );

        let descriptor = read_segment(&store, &next, 1)?;
        let changed_route = read_route_inventory(&store, &descriptor)?
            .into_iter()
            .next()
            .ok_or("segment route inventory empty")?;
        let mut changed_posting = next.clone();
        let target = match changed_route.first().copied() {
            Some(super::GRAM_RAW | super::GRAM_FOLDED) => &mut changed_posting.gram_routes,
            Some(super::TERM_RAW | super::TERM_FOLDED) => &mut changed_posting.term_routes,
            _ => return Err("unknown route surface".into()),
        };
        *target = store.map_apply(
            &second_lease,
            target,
            std::iter::once(Ok(MapEdit {
                key: changed_route,
                value: None,
            })),
        )?;
        check_eq!(
            inherit_unchanged_posting_index_bytes(
                &store,
                &old,
                &changed_posting,
                &key,
                &old_ref,
                &next_ref,
            )?,
            None,
            "changed posting route must force an independent recount"
        );

        second_lease.finish_published(&next.root_objects())?;
        let third_lease = store.begin_attempt(&binding(3)?, &next.root_objects())?;
        let (no_op, no_op_receipt) = publish_delta(
            &store,
            &third_lease,
            Some(&next),
            &[],
            &[],
            &[],
            false,
            ManifestGeneration::new(3),
            [9; 32],
        )?;
        check_eq!(no_op.segments, next.segments);
        check_eq!(no_op.run_registry, next.run_registry);
        check_eq!(no_op_receipt.posting_pages_written, 0);
        third_lease.finish_published(&no_op.root_objects())?;
        let authority = NativeAuthority::open(store, no_op)?;
        let budget = RequestBudgetV1::unbounded();
        check_eq!(
            authority.intersect_terms(&["spectral"], true, &budget)?,
            vec![2]
        );
        check!(authority.read_document(1, &budget)?.is_none());
        check_eq!(
            authority
                .read_document(2, &budget)?
                .map(|doc| doc.indexed_text),
            Some("beta spectral needle".into()),
        );
        Ok(())
    }

    fn binding(generation: u64) -> Result<AttemptBinding, Box<dyn std::error::Error>> {
        let byte = u8::try_from(generation)?;
        Ok(AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: [2; 32],
            revision_sha256: [byte; 32],
            source_event_sha256: [byte; 32],
            target_generation: generation,
            base_root_sha256: (generation > 1).then_some([byte.wrapping_sub(1); 32]),
            operations_sha256: [byte; 32],
            format: 1,
        })
    }

    fn corpus_text(id: u64) -> String {
        match id {
            1 => format!("needle{id} {}", "anchor ".repeat(128)),
            10 | 19 => format!("needle{id} tiny"),
            _ => format!("needle{id} {}", "filler ".repeat(512)),
        }
    }

    fn complete_hard_lane(
        store: &ObjectStore,
        base: &NativeRoot,
        retired: &[u64],
        policy: &LexicalMaintenancePolicy,
    ) -> Result<ScanLane, Box<dyn std::error::Error>> {
        let high = base
            .run_registry
            .next_run_id
            .checked_sub(1)
            .ok_or("high-water underflow")?;
        let mut progress = RunSelectionProgress::default();
        let mut windows = Vec::new();
        loop {
            let mut window =
                prepare_hard_scan_window(store, base, retired, policy, progress, None)?;
            window.ordinal = u16::try_from(windows.len())?;
            let complete = window.round_complete;
            progress = RunSelectionProgress {
                next_scan_run_id: window.next_run_id,
                scan_high_water_run_id: high,
                pending_run: None,
            };
            windows.push(window);
            if complete {
                break;
            }
            if windows.len() > 129 {
                return Err("hard scan did not finish within bound".into());
            }
        }
        Ok(ScanLane {
            authority: Authority::Native,
            base_authority_root_sha256: native_root_sha256(base)?,
            frozen_high_water_run_id: high,
            windows,
        })
    }

    #[test]
    fn hard_pressure_ordinary_append_finds_pair_beyond_soft_window()
    -> Result<(), Box<dyn std::error::Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let dir = tempfile::tempdir()?;
        let store = ObjectStore::open(dir.path())?;
        let mut base: Option<NativeRoot> = None;
        for id in 1_u64..=34 {
            let text = if id == 1 {
                format!("needle{id} {}", "filler ".repeat(64))
            } else if id <= 32 {
                format!("needle{id} {}", "filler ".repeat(512))
            } else {
                format!("needle{id} tiny")
            };
            let source = SourceFileKey {
                source_repo_id: RepoId::new("source-repo")?,
                repo_relative_path: RepoRelativePath::new(format!("src/doc-{id}.rs")),
            };
            let coverage = SourceFileCoverage {
                source: SourceFileRevision {
                    file: source.clone(),
                    revision_id: RevisionId::new(format!("source-r{id}"))?,
                    source_sha256: Sha256::digest(text.as_bytes()).into(),
                },
                language: quanta_index_contract::lex::LanguageCode::new("rust")?,
                producer_policy_sha256: [1; 32],
                symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
                unit_set_sha256: [2; 32],
                text_admitted: true,
                symbols: SymbolCoverage::Complete { symbol_count: 0 },
            };
            let inherited = base
                .as_ref()
                .map_or_else(Vec::new, NativeRoot::root_objects);
            let lease = store.begin_attempt(&binding(id)?, &inherited)?;
            let (next, _) = publish_delta(
                &store,
                &lease,
                base.as_ref(),
                &[NativeSourceEdit::Upsert(coverage)],
                &[],
                &[AddedTextDoc {
                    doc_id: id,
                    source_file: source,
                    candidate_id: format!("doc-{id}"),
                    text,
                }],
                false,
                ManifestGeneration::new(id),
                [9; 32],
            )?;
            lease.finish_published(&next.root_objects())?;
            base = Some(next);
        }
        let base = base.ok_or("missing 34 run base")?;
        check_eq!(base.run_registry.runs.entries, 34);
        let cost = |id| -> Result<u64, Box<dyn std::error::Error>> {
            Ok(base
                .run_registry
                .candidate_by_id(&store, id, None)?
                .ok_or("missing run")?
                .index_encoded_bytes)
        };
        let tail_pair = cost(33)?
            .checked_add(cost(34)?)
            .ok_or("pair cost overflow")?;
        let pair_cap = cost(1)?.max(tail_pair);
        check!(
            cost(1)? <= pair_cap,
            "first anchor is individually eligible"
        );
        check!(cost(1)? + cost(33)? > pair_cap, "first anchor blocks tail");
        check!(tail_pair <= pair_cap, "tail pair fits");
        for id in 2_u64..=32 {
            check!(cost(id)? > pair_cap, "prefix run {id} must be ineligible");
        }
        let policy = LexicalMaintenancePolicy::new(
            2,
            34,
            1,
            u64::MAX,
            8, // 32-header soft window
            pair_cap,
            64 * 1024 * 1024,
            60_000,
        )?;
        let source = SourceFileKey {
            source_repo_id: RepoId::new("source-repo")?,
            repo_relative_path: RepoRelativePath::new("src/doc-35.rs"),
        };
        let text = "needle35 tiny".to_owned();
        let edit = NativeSourceEdit::Upsert(SourceFileCoverage {
            source: SourceFileRevision {
                file: source.clone(),
                revision_id: RevisionId::new("source-r35")?,
                source_sha256: Sha256::digest(text.as_bytes()).into(),
            },
            language: quanta_index_contract::lex::LanguageCode::new("rust")?,
            producer_policy_sha256: [1; 32],
            symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
            unit_set_sha256: [2; 32],
            text_admitted: true,
            symbols: SymbolCoverage::Complete { symbol_count: 0 },
        });
        let added = AddedTextDoc {
            doc_id: 35,
            source_file: source,
            candidate_id: "doc-35".into(),
            text,
        };
        check!(
            select_clean_merge_inputs(&store, &base, &[], &policy, None).is_err(),
            "hard publication must refuse a missing checkpoint before mutation"
        );
        CLEAN_MERGE_PREPARE_CALLS.with(|count| count.set(0));
        let lane = complete_hard_lane(&store, &base, &[], &policy)?;
        check_eq!(
            CLEAN_MERGE_PREPARE_CALLS.with(std::cell::Cell::get),
            0,
            "unselected hard-scan candidates must not materialize body documents"
        );
        let lease = store.begin_attempt(&binding(35)?, &base.root_objects())?;
        let (target, _) = publish_delta_prepared_with_policy(
            &store,
            &lease,
            Some(&base),
            std::slice::from_ref(&edit),
            &[],
            std::slice::from_ref(&added),
            false,
            ManifestGeneration::new(35),
            [9; 32],
            None,
            Some(&policy),
            Some(&lane),
        )?;
        check_eq!(
            CLEAN_MERGE_PREPARE_CALLS.with(std::cell::Cell::get),
            2,
            "only the two selected cohorts may materialize bodies"
        );
        let _seal_proof = verify_transition_for_seal_with_policy(
            &store,
            &target,
            Some(&base),
            &DocVersionReservation {
                first: 35,
                count: 1,
                scope_sha256: [9; 32],
            },
            std::slice::from_ref(&edit),
            &[],
            std::slice::from_ref(&added),
            false,
            base.next_segment_id,
            None,
            Some(&policy),
            Some(&lane),
        )?;
        let mut forged_lane = lane.clone();
        let tail = forged_lane
            .windows
            .iter_mut()
            .flat_map(|window| &mut window.candidates)
            .find(|candidate| candidate.run_id == 33)
            .ok_or("missing tail witness")?;
        if let EligibilityWitness::NativeEligible {
            ref mut staging_bytes,
            ..
        } = tail.eligibility
        {
            *staging_bytes = staging_bytes
                .checked_add(1)
                .ok_or("forged staging overflow")?;
        } else {
            return Err("tail witness unexpectedly ineligible".into());
        }
        check!(
            verify_transition_for_seal_with_policy(
                &store,
                &target,
                Some(&base),
                &DocVersionReservation {
                    first: 35,
                    count: 1,
                    scope_sha256: [9; 32]
                },
                std::slice::from_ref(&edit),
                &[],
                std::slice::from_ref(&added),
                false,
                base.next_segment_id,
                None,
                Some(&policy),
                Some(&forged_lane),
            )
            .is_err(),
            "independent seal must refuse forged Native staging charge"
        );
        let mut forged_header = lane.clone();
        let first_header = forged_header
            .windows
            .first_mut()
            .and_then(|window| window.candidates.first_mut())
            .ok_or("missing Native header witness")?;
        let first_byte = first_header
            .header_sha256
            .first_mut()
            .ok_or("empty header digest")?;
        *first_byte ^= 1;
        check!(
            verify_transition_for_seal_with_policy(
                &store,
                &target,
                Some(&base),
                &DocVersionReservation {
                    first: 35,
                    count: 1,
                    scope_sha256: [9; 32]
                },
                std::slice::from_ref(&edit),
                &[],
                std::slice::from_ref(&added),
                false,
                base.next_segment_id,
                None,
                Some(&policy),
                Some(&forged_header),
            )
            .is_err(),
            "independent seal must refuse a forged authenticated header"
        );
        let mut skipped_window = lane;
        let first_window = skipped_window
            .windows
            .first_mut()
            .ok_or("missing Native scan window")?;
        first_window.next_run_id = first_window
            .next_run_id
            .checked_add(1)
            .ok_or("forged cursor overflow")?;
        check!(
            verify_transition_for_seal_with_policy(
                &store,
                &target,
                Some(&base),
                &DocVersionReservation {
                    first: 35,
                    count: 1,
                    scope_sha256: [9; 32]
                },
                std::slice::from_ref(&edit),
                &[],
                std::slice::from_ref(&added),
                false,
                base.next_segment_id,
                None,
                Some(&policy),
                Some(&skipped_window),
            )
            .is_err(),
            "independent seal must refuse a skipped window cursor"
        );
        policy.admit_target(
            base.run_registry.snapshot()?,
            target.run_registry.snapshot()?,
        )?;
        check_eq!(target.run_registry.runs.entries, 34);
        check!(
            target
                .run_registry
                .candidate_by_id(&store, 33, None)?
                .is_none()
        );
        check!(
            target
                .run_registry
                .candidate_by_id(&store, 34, None)?
                .is_none()
        );
        verify_all_for_scrub(&store, &target, None)?;
        let authority = NativeAuthority::open(store.clone(), target)?;
        let budget = RequestBudgetV1::unbounded();
        for id in 1_u64..=35 {
            check_eq!(
                authority.intersect_terms(&[&format!("needle{id}")], true, &budget)?,
                vec![id]
            );
            let expected = if id == 1 {
                format!("needle{id} {}", "filler ".repeat(64))
            } else if id <= 32 {
                format!("needle{id} {}", "filler ".repeat(512))
            } else {
                format!("needle{id} tiny")
            };
            check_eq!(
                authority
                    .read_document(id, &budget)?
                    .map(|doc| doc.indexed_text),
                Some(expected)
            );
        }
        Ok(())
    }

    fn run_separated_pair_case(
        pending_old_anchor: bool,
        over_hard: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let dir = tempfile::tempdir()?;
        let store = ObjectStore::open(dir.path())?;
        let mut base: Option<NativeRoot> = None;
        let mut source_gold = Vec::new();
        let mut text_gold = Vec::new();
        for id in 1_u64..=20 {
            let text = corpus_text(id);
            let source = SourceFileKey {
                source_repo_id: RepoId::new("source-repo")?,
                repo_relative_path: RepoRelativePath::new(format!("src/doc-{id}.rs")),
            };
            let coverage = SourceFileCoverage {
                source: SourceFileRevision {
                    file: source.clone(),
                    revision_id: RevisionId::new(format!("source-r{id}"))?,
                    source_sha256: Sha256::digest(text.as_bytes()).into(),
                },
                language: quanta_index_contract::lex::LanguageCode::new("rust")?,
                producer_policy_sha256: [1; 32],
                symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
                unit_set_sha256: [2; 32],
                text_admitted: true,
                symbols: SymbolCoverage::Complete { symbol_count: 0 },
            };
            let inherited = base
                .as_ref()
                .map_or_else(Vec::new, NativeRoot::root_objects);
            let lease = store.begin_attempt(&binding(id)?, &inherited)?;
            let (next, _) = publish_delta(
                &store,
                &lease,
                base.as_ref(),
                &[NativeSourceEdit::Upsert(coverage)],
                &[],
                &[AddedTextDoc {
                    doc_id: id,
                    source_file: source.clone(),
                    candidate_id: format!("doc-{id}"),
                    text: text.clone(),
                }],
                false,
                ManifestGeneration::new(id),
                [9; 32],
            )?;
            lease.finish_published(&next.root_objects())?;
            base = Some(next);
            source_gold.push(source);
            text_gold.push(text);
        }
        let original = base.as_ref().ok_or("missing append-only base")?;
        check_eq!(original.run_registry.runs.entries, 20);
        let empty_file_lease = store.begin_attempt(&binding(41)?, &original.root_objects())?;
        let empty_file = build_for_seal(
            &store,
            &empty_file_lease,
            None,
            &DocVersionReservation {
                first: 0,
                count: 0,
                scope_sha256: [9; 32],
            },
            &FileAuthorityMutation {
                edits: Vec::new(),
                clear_all: false,
                operations_sha256: [41; 32],
            },
        )?
        .root;
        empty_file_lease.finish_published(&empty_file.object_roots())?;
        let indexed = |id| -> Result<u64, Box<dyn std::error::Error>> {
            let candidate = original
                .run_registry
                .candidate_by_id(&store, id, None)?
                .ok_or("missing run candidate")?;
            Ok(candidate.index_encoded_bytes)
        };
        let old_bytes = indexed(1)?;
        let younger_a = indexed(10)?;
        let younger_b = indexed(19)?;
        let pair_bytes = younger_a
            .checked_add(younger_b)
            .ok_or("pair byte overflow")?;
        check!(
            old_bytes > pair_bytes,
            "fixture anchor must exceed the young pair"
        );
        let cap = if pending_old_anchor {
            old_bytes
                .checked_add(younger_a.min(younger_b))
                .and_then(|bytes| bytes.checked_sub(1))
                .ok_or("anchor cap overflow")?
        } else {
            pair_bytes
        };
        check!(pair_bytes <= cap);
        check_eq!(old_bytes <= cap, pending_old_anchor);
        check!(
            old_bytes
                .checked_add(younger_a.min(younger_b))
                .ok_or("pair cap overflow")?
                > cap
        );
        for id in 2_u64..=20 {
            if id != 10 && id != 19 {
                check!(
                    indexed(id)? > cap,
                    "fixture filler run {id} must be oversized"
                );
            }
        }
        let policy = LexicalMaintenancePolicy::new(
            2,
            if over_hard { 10 } else { 40 },
            1,
            u64::MAX,
            2,
            cap,
            64 * 1024 * 1024,
            60_000,
        )?;
        let reservation = DocVersionReservation {
            first: 0,
            count: 0,
            scope_sha256: [9; 32],
        };
        let mut merged = false;
        let mut cursor_only_admissions = 0_u32;
        for generation in 21_u64..=40 {
            let previous = base.as_ref().ok_or("missing prior root")?;
            let lane = if policy.hard_pressure(previous.run_registry.snapshot()?) {
                Some(complete_hard_lane(&store, previous, &[], &policy)?)
            } else {
                None
            };
            let lease = store.begin_attempt(&binding(generation)?, &previous.root_objects())?;
            let (next, _) = publish_delta_prepared_with_policy(
                &store,
                &lease,
                Some(previous),
                &[],
                &[],
                &[],
                false,
                ManifestGeneration::new(generation),
                [9; 32],
                None,
                Some(&policy),
                lane.as_ref(),
            )?;
            let _seal_proof = verify_transition_for_seal_with_policy(
                &store,
                &next,
                Some(previous),
                &reservation,
                &[],
                &[],
                &[],
                false,
                previous.next_segment_id,
                None,
                Some(&policy),
                lane.as_ref(),
            )?;
            let before = previous.run_registry.snapshot()?;
            let after = next.run_registry.snapshot()?;
            if policy.admit_target(before, after).is_err() {
                check!(
                    next.is_cursor_only_successor_of(previous)?,
                    "only an authenticated cursor-only root may bypass an existing hard breach"
                );
                policy.admit_cursor_only_target(before, after)?;
                admit_maintenance_roots(
                    &store,
                    &policy,
                    (Some(&empty_file), &empty_file),
                    (Some(previous), &next),
                    true,
                )?;
                check!(
                    admit_maintenance_roots(
                        &store,
                        &policy,
                        (Some(&empty_file), &empty_file),
                        (Some(previous), &next),
                        false,
                    )
                    .is_err(),
                    "a non-no-op source event must not receive cursor-only admission"
                );
                cursor_only_admissions = cursor_only_admissions
                    .checked_add(1)
                    .ok_or("cursor-only admission count overflow")?;
                let mut stale_cursor = next.clone();
                stale_cursor.run_registry = stale_cursor
                    .run_registry
                    .with_selection_progress(previous.run_registry.selection_progress())?;
                check!(
                    verify_transition_for_seal_with_policy(
                        &store,
                        &stale_cursor,
                        Some(previous),
                        &reservation,
                        &[],
                        &[],
                        &[],
                        false,
                        previous.next_segment_id,
                        None,
                        Some(&policy),
                        lane.as_ref(),
                    )
                    .is_err(),
                    "independent seal must reject a stale bounded-scan cursor"
                );
            } else {
                admit_maintenance_roots(
                    &store,
                    &policy,
                    (Some(&empty_file), &empty_file),
                    (Some(previous), &next),
                    true,
                )?;
            }
            if next.run_registry.runs.entries < previous.run_registry.runs.entries {
                check_eq!(next.run_registry.runs.entries, 19);
                verify_changed_segments_for_seal(&store, &next, previous.next_segment_id)?;
                merged = true;
            }
            lease.finish_published(&next.root_objects())?;
            base = Some(next);
            if merged {
                break;
            }
        }
        check!(
            merged,
            "bounded publications must reach the separated younger pair"
        );
        if over_hard {
            check_eq!(
                cursor_only_admissions,
                0,
                "complete hard recovery must find the pair without a cursor-only generation"
            );
        }
        let final_root = base.ok_or("missing final root")?;
        for id in [10_u64, 19] {
            check!(
                store
                    .map_get(&final_root.segments, &id.to_be_bytes(), None)?
                    .is_none()
            );
        }
        check!(
            store
                .map_get(&final_root.segments, &1_u64.to_be_bytes(), None)?
                .is_some()
        );
        verify_all_for_scrub(&store, &final_root, None)?;
        let authority = NativeAuthority::open(store.clone(), final_root.clone())?;
        let budget = RequestBudgetV1::unbounded();
        for id in 1_u64..=20 {
            let slot = usize::try_from(id.checked_sub(1).ok_or("zero document id")?)?;
            let expected_term = format!("needle{id}");
            check_eq!(
                authority.intersect_terms(&[&expected_term], true, &budget)?,
                vec![id]
            );
            check_eq!(
                authority
                    .read_document(id, &budget)?
                    .map(|doc| doc.indexed_text),
                Some(text_gold.get(slot).ok_or("missing text oracle")?.clone()),
            );
            let row = read_current(&store, &final_root, id)?.ok_or("missing current")?;
            check_eq!(
                row.source,
                source_gold
                    .get(slot)
                    .ok_or("missing source oracle")?
                    .clone()
            );
            check_eq!(row.candidate_id, format!("doc-{id}"));
        }
        Ok(())
    }

    #[test]
    fn bounded_fairness_skips_oversized_old_runs_and_merges_separated_young_pair() {
        run_separated_pair_case(false, false).expect("oversized-old fair Native merge");
    }

    #[test]
    fn bounded_fairness_rotates_individually_eligible_unpairable_old_anchor() {
        run_separated_pair_case(true, false).expect("unpairable-anchor fair Native merge");
    }

    #[test]
    fn bounded_fairness_persists_cursor_when_base_exceeds_new_hard_limit() {
        run_separated_pair_case(false, true).expect("over-hard cursor-only Native recovery");
    }
}
