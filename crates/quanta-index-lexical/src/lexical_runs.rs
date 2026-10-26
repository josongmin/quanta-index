//! Persistent run ownership for paged lexical segments.
//!
//! A run is an independently created cohort, not a synonym for a segment.
//! These counters describe the logical encoded index bundle of active
//! segments. They do not reserve filesystem blocks or count shared physical
//! objects across generations.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use quanta_index_core::lexical_maintenance::{MAX_HARD_SCAN_RUNS, TargetCounters};
use quanta_index_core::{CoreError, RequestBudgetV1};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::lexical_store::{
    AttemptLease, BlobRef, MapEdit, MapRoot, ObjectKind, ObjectRef, ObjectStore,
};

const RUN_WIRE_VERSION: u8 = 1;
type RunMemberVisitor<'a> = dyn FnMut(&[u8], &BlobRef, u64) -> Result<(), CoreError> + 'a;

const REGISTRY_WIRE_VERSION: u8 = 2;
const MAX_RECORD_BLOB_BYTES: u64 = 64 * 1024;

fn invalid(message: &str) -> CoreError {
    CoreError::InvalidContract(format!("lexical run registry: {message}"))
}

fn corrupt(message: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!("lexical run registry corrupt: {message}"),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunRegistry {
    pub(crate) runs: MapRoot,
    pub(crate) segment_owners: MapRoot,
    pub(crate) next_run_id: u64,
    pub(crate) active_index_encoded_bytes: u64,
    selection_progress: RunSelectionProgress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RunSelectionProgress {
    pub(crate) next_scan_run_id: u64,
    pub(crate) scan_high_water_run_id: u64,
    pub(crate) pending_run: Option<u64>,
}

impl Default for RunSelectionProgress {
    fn default() -> Self {
        Self {
            next_scan_run_id: 1,
            scan_high_water_run_id: 0,
            pending_run: None,
        }
    }
}

pub(crate) struct RunScanWindow {
    pub(crate) candidates: Vec<RunCandidate>,
    pub(crate) next_scan_run_id: u64,
    pub(crate) scan_high_water_run_id: u64,
    pub(crate) round_complete: bool,
}

/// Work budget for discovering cohorts, separate from merge fan-in. Fixed
/// ceiling prevents an untrusted policy value from creating a corpus scan.
pub(crate) fn bounded_candidate_scan_limit(max_merge_fan_in: u32) -> u32 {
    max_merge_fan_in.saturating_mul(4).clamp(4, 32)
}

impl Default for RunRegistry {
    fn default() -> Self {
        Self {
            runs: MapRoot::default(),
            segment_owners: MapRoot::default(),
            next_run_id: 1,
            active_index_encoded_bytes: 0,
            selection_progress: RunSelectionProgress::default(),
        }
    }
}

impl Serialize for RunRegistry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            REGISTRY_WIRE_VERSION,
            &self.runs,
            &self.segment_owners,
            self.next_run_id,
            self.active_index_encoded_bytes,
            self.selection_progress.next_scan_run_id,
            self.selection_progress.scan_high_water_run_id,
            self.selection_progress.pending_run,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RunRegistry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (
            version,
            runs,
            segment_owners,
            next_run_id,
            active_index_encoded_bytes,
            next_scan_run_id,
            scan_high_water_run_id,
            pending_run,
        ) = <(u8, MapRoot, MapRoot, u64, u64, u64, u64, Option<u64>)>::deserialize(deserializer)?;
        if version != REGISTRY_WIRE_VERSION {
            return Err(serde::de::Error::custom(
                "unknown lexical run registry version",
            ));
        }
        let registry = Self {
            runs,
            segment_owners,
            next_run_id,
            active_index_encoded_bytes,
            selection_progress: RunSelectionProgress {
                next_scan_run_id,
                scan_high_water_run_id,
                pending_run,
            },
        };
        registry.validate().map_err(serde::de::Error::custom)?;
        Ok(registry)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SegmentInput {
    pub(crate) key: Vec<u8>,
    pub(crate) reference: BlobRef,
    pub(crate) index_encoded_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SegmentRemoval {
    pub(crate) key: Vec<u8>,
    pub(crate) reference: BlobRef,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunCandidate {
    pub(crate) run_id: u64,
    pub(crate) members: MapRoot,
    pub(crate) index_encoded_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunHeader {
    version: u8,
    run_id: u64,
    members: MapRoot,
    active_index_encoded_bytes: u64,
}

impl Serialize for RunHeader {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            self.version,
            self.run_id,
            &self.members,
            self.active_index_encoded_bytes,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RunHeader {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (version, run_id, members, active_index_encoded_bytes) =
            <(u8, u64, MapRoot, u64)>::deserialize(deserializer)?;
        Ok(Self {
            version,
            run_id,
            members,
            active_index_encoded_bytes,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunMember {
    version: u8,
    run_id: u64,
    reference: BlobRef,
    index_encoded_bytes: u64,
}

impl Serialize for RunMember {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            self.version,
            self.run_id,
            &self.reference,
            self.index_encoded_bytes,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RunMember {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (version, run_id, reference, index_encoded_bytes) =
            <(u8, u64, BlobRef, u64)>::deserialize(deserializer)?;
        Ok(Self {
            version,
            run_id,
            reference,
            index_encoded_bytes,
        })
    }
}

struct ExistingRunChange {
    header: RunHeader,
    member_edits: Vec<MapEdit>,
    removed_index_bytes: u64,
    added_index_bytes: u64,
}

/// One authenticated member from a complete selected cohort. The retained
/// segment reference and logical byte charge are transferred unchanged.
struct CohortTransferMember {
    old_owner: BlobRef,
    segment: BlobRef,
    index_encoded_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
enum ScrubPhase {
    Runs,
    Members,
    Owners,
    Done,
}

#[derive(Clone, Debug)]
struct CurrentScrubRun {
    key: Vec<u8>,
    header: RunHeader,
    last_member_key: Option<Vec<u8>>,
    observed_members: u64,
    observed_bytes: u64,
}

/// Bounded full scrub continuation. Each `step` processes at most one row;
/// a fresh logarithmic map seek avoids borrowing the store across calls.
#[derive(Clone, Debug)]
pub(crate) struct RunScrubCursor {
    registry: RunRegistry,
    phase: ScrubPhase,
    last_run_key: Option<Vec<u8>>,
    current_run: Option<CurrentScrubRun>,
    last_owner_key: Option<Vec<u8>>,
    observed_runs: u64,
    observed_segments: u64,
    observed_bytes: u64,
    observed_owners: u64,
}

impl RunScrubCursor {
    pub(crate) fn new(registry: &RunRegistry) -> Result<Self, CoreError> {
        registry.validate()?;
        Ok(Self {
            registry: registry.clone(),
            phase: ScrubPhase::Runs,
            last_run_key: None,
            current_run: None,
            last_owner_key: None,
            observed_runs: 0,
            observed_segments: 0,
            observed_bytes: 0,
            observed_owners: 0,
        })
    }

    /// Returns true only after both run/member and owner directions agree.
    pub(crate) fn step(
        &mut self,
        store: &ObjectStore,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<bool, CoreError> {
        if matches!(self.phase, ScrubPhase::Done) {
            return Ok(true);
        }
        if let Some(budget) = budget {
            budget.checkpoint("lexical:run-registry:scrub-step")?;
        }
        match self.phase {
            ScrubPhase::Runs => {
                let row = next_map_row(
                    store,
                    &self.registry.runs,
                    self.last_run_key.as_deref(),
                    budget,
                )?;
                let Some((key, value)) = row else {
                    self.phase = ScrubPhase::Owners;
                    return Ok(false);
                };
                let id = parse_run_key(&key)?;
                if id == 0 || id >= self.registry.next_run_id {
                    return Err(corrupt("run key is outside issued ID range"));
                }
                let header = decoded_header(store, &value, id, budget)?;
                self.current_run = Some(CurrentScrubRun {
                    key,
                    header,
                    last_member_key: None,
                    observed_members: 0,
                    observed_bytes: 0,
                });
                self.phase = ScrubPhase::Members;
                Ok(false)
            }
            ScrubPhase::Members => {
                let current = self
                    .current_run
                    .as_mut()
                    .ok_or_else(|| corrupt("member scrub has no current run"))?;
                let row = next_map_row(
                    store,
                    &current.header.members,
                    current.last_member_key.as_deref(),
                    budget,
                )?;
                if let Some((key, member_ref)) = row {
                    let member = decoded_member(store, &member_ref, budget)?;
                    if member.run_id != current.header.run_id
                        || store.map_get(&self.registry.segment_owners, &key, budget)?
                            != Some(member_ref)
                    {
                        return Err(corrupt("run member and owner row differ"));
                    }
                    current.observed_members = current
                        .observed_members
                        .checked_add(1)
                        .ok_or_else(|| corrupt("run segment count overflow"))?;
                    current.observed_bytes = current
                        .observed_bytes
                        .checked_add(member.index_encoded_bytes)
                        .ok_or_else(|| corrupt("run index bytes overflow"))?;
                    current.last_member_key = Some(key);
                    return Ok(false);
                }
                if current.observed_members != current.header.members.entries
                    || current.observed_bytes != current.header.active_index_encoded_bytes
                {
                    return Err(corrupt("run header totals differ from members"));
                }
                self.observed_runs = self
                    .observed_runs
                    .checked_add(1)
                    .ok_or_else(|| corrupt("run count overflow"))?;
                self.observed_segments = self
                    .observed_segments
                    .checked_add(current.observed_members)
                    .ok_or_else(|| corrupt("segment count overflow"))?;
                self.observed_bytes = self
                    .observed_bytes
                    .checked_add(current.observed_bytes)
                    .ok_or_else(|| corrupt("index byte count overflow"))?;
                self.last_run_key = Some(current.key.clone());
                self.current_run = None;
                self.phase = ScrubPhase::Runs;
                Ok(false)
            }
            ScrubPhase::Owners => {
                let row = next_map_row(
                    store,
                    &self.registry.segment_owners,
                    self.last_owner_key.as_deref(),
                    budget,
                )?;
                if let Some((key, owner_ref)) = row {
                    let _member = self
                        .registry
                        .authenticated_owner(store, &key, &owner_ref, budget)?;
                    self.observed_owners = self
                        .observed_owners
                        .checked_add(1)
                        .ok_or_else(|| corrupt("owner count overflow"))?;
                    self.last_owner_key = Some(key);
                    return Ok(false);
                }
                if self.observed_runs != self.registry.runs.entries
                    || self.observed_segments != self.registry.segment_owners.entries
                    || self.observed_bytes != self.registry.active_index_encoded_bytes
                    || self.observed_owners != self.registry.segment_owners.entries
                {
                    return Err(corrupt("registry totals differ from streamed rows"));
                }
                self.phase = ScrubPhase::Done;
                Ok(true)
            }
            ScrubPhase::Done => Ok(true),
        }
    }
}

fn next_map_row(
    store: &ObjectStore,
    root: &MapRoot,
    after: Option<&[u8]>,
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<(Vec<u8>, BlobRef)>, CoreError> {
    let mut cursor = store.map_cursor(root, after.unwrap_or(&[]), None, budget)?;
    let first = cursor.next()?;
    if first
        .as_ref()
        .is_some_and(|(key, _)| Some(key.as_slice()) == after)
    {
        cursor.next()
    } else {
        Ok(first)
    }
}

fn run_key(run_id: u64) -> Vec<u8> {
    run_id.to_be_bytes().to_vec()
}

fn parse_run_key(key: &[u8]) -> Result<u64, CoreError> {
    let bytes: [u8; 8] = key
        .try_into()
        .map_err(|_error| corrupt("run key width differs"))?;
    Ok(u64::from_be_bytes(bytes))
}

fn decoded_member(
    store: &ObjectStore,
    blob: &BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<RunMember, CoreError> {
    if blob.root.kind != ObjectKind::Record {
        return Err(corrupt("owner value is not a Record"));
    }
    let bytes = store.read_blob(blob, MAX_RECORD_BLOB_BYTES, budget)?;
    let member: RunMember = crate::channel_payloads::decode_cbor_exact(&bytes)
        .map_err(|error| corrupt(&format!("member CBOR: {error}")))?;
    if member.version != RUN_WIRE_VERSION || member.index_encoded_bytes == 0 {
        return Err(corrupt("member version or byte count differs"));
    }
    if store.object_dependencies(&blob.root, budget)? != vec![member.reference.root.clone()] {
        return Err(corrupt("member dependency differs from segment root"));
    }
    Ok(member)
}

fn decoded_header(
    store: &ObjectStore,
    blob: &BlobRef,
    expected_run_id: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<RunHeader, CoreError> {
    if blob.root.kind != ObjectKind::Record {
        return Err(corrupt("run header is not a Record"));
    }
    let bytes = store.read_blob(blob, MAX_RECORD_BLOB_BYTES, budget)?;
    let header: RunHeader = crate::channel_payloads::decode_cbor_exact(&bytes)
        .map_err(|error| corrupt(&format!("run header CBOR: {error}")))?;
    if header.version != RUN_WIRE_VERSION
        || header.run_id != expected_run_id
        || header.members.entries == 0
        || header.active_index_encoded_bytes == 0
    {
        return Err(corrupt("run header identity or counters differ"));
    }
    let dependencies = header
        .members
        .page
        .as_ref()
        .map(|page| page.object.clone())
        .into_iter()
        .collect::<Vec<_>>();
    if store.object_dependencies(&blob.root, budget)? != dependencies {
        return Err(corrupt("run header dependency differs from members root"));
    }
    Ok(header)
}

fn persist_member(
    store: &ObjectStore,
    lease: &AttemptLease,
    member: &RunMember,
) -> Result<BlobRef, CoreError> {
    let bytes = crate::channel_payloads::encode_cbor(member, "run member")?;
    store.put_blob(
        lease,
        ObjectKind::Record,
        std::slice::from_ref(&member.reference.root),
        &bytes,
    )
}

fn persist_header(
    store: &ObjectStore,
    lease: &AttemptLease,
    header: &RunHeader,
) -> Result<BlobRef, CoreError> {
    let bytes = crate::channel_payloads::encode_cbor(header, "run header")?;
    let dependencies = header
        .members
        .page
        .as_ref()
        .map(|page| page.object.clone())
        .into_iter()
        .collect::<Vec<_>>();
    store.put_blob(lease, ObjectKind::Record, &dependencies, &bytes)
}

fn sorted_unique_removed(removed: &[SegmentRemoval]) -> Result<Vec<SegmentRemoval>, CoreError> {
    let mut sorted = removed.to_vec();
    sorted.sort_by(|a, b| a.key.cmp(&b.key));
    if sorted
        .windows(2)
        .any(|pair| matches!(pair, [left, right] if left.key == right.key))
    {
        return Err(invalid("duplicate removed segment key"));
    }
    Ok(sorted)
}

fn sorted_unique_added(added: &[SegmentInput]) -> Result<Vec<SegmentInput>, CoreError> {
    let mut sorted = added.to_vec();
    sorted.sort_by(|a, b| a.key.cmp(&b.key));
    if sorted
        .windows(2)
        .any(|pair| matches!(pair, [left, right] if left.key == right.key))
        || sorted
            .iter()
            .any(|segment| segment.index_encoded_bytes == 0)
    {
        return Err(invalid("duplicate added segment key or zero index bytes"));
    }
    Ok(sorted)
}

impl RunRegistry {
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        if self.next_run_id == 0 || self.runs.entries >= self.next_run_id {
            return Err(invalid("next run ID or run count differs"));
        }
        let progress = self.selection_progress;
        if progress.next_scan_run_id == 0
            || progress.next_scan_run_id > self.next_run_id
            || progress.scan_high_water_run_id >= self.next_run_id
            || progress
                .pending_run
                .is_some_and(|id| id == 0 || id > progress.scan_high_water_run_id)
        {
            return Err(invalid("selection progress is outside issued run IDs"));
        }
        let _counters = self.snapshot()?;
        Ok(())
    }

    pub(crate) fn selection_progress(&self) -> RunSelectionProgress {
        self.selection_progress
    }

    /// Compare durable ownership and attributed cost while deliberately
    /// excluding only the authenticated bounded-scan cursor. Callers must
    /// separately prove every enclosing authority-root field is unchanged.
    pub(crate) fn same_authority_as(&self, prior: &Self) -> bool {
        self.runs == prior.runs
            && self.segment_owners == prior.segment_owners
            && self.next_run_id == prior.next_run_id
            && self.active_index_encoded_bytes == prior.active_index_encoded_bytes
    }

    pub(crate) fn with_selection_progress(
        mut self,
        progress: RunSelectionProgress,
    ) -> Result<Self, CoreError> {
        self.selection_progress = progress;
        self.validate()?;
        Ok(self)
    }

    /// Reauthenticate one old anchor without materializing its members.
    pub(crate) fn candidate_by_id(
        &self,
        store: &ObjectStore,
        id: u64,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Option<RunCandidate>, CoreError> {
        if id == 0 || id >= self.next_run_id {
            return Err(invalid("candidate ID is outside issued range"));
        }
        let Some(reference) = store.map_get(&self.runs, &run_key(id), budget)? else {
            return Ok(None);
        };
        let header = decoded_header(store, &reference, id, budget)?;
        Ok(Some(RunCandidate {
            run_id: id,
            members: header.members,
            index_encoded_bytes: header.active_index_encoded_bytes,
        }))
    }

    /// Scan at most `max_scanned` live headers. The persisted high-water
    /// freezes a round against unbounded appends; a pending anchor is tried
    /// against one complete circular pass before the caller replaces it.
    pub(crate) fn scan_window(
        &self,
        store: &ObjectStore,
        max_scanned: u32,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<RunScanWindow, CoreError> {
        self.scan_window_inner(store, max_scanned, budget, false)
    }

    /// Scan one bounded window of the frozen old frontier. Unlike the soft
    /// rotating cursor, this stops at high-water and never wraps into a
    /// second pass within the same checkpoint window.
    pub(crate) fn scan_hard_window(
        &self,
        store: &ObjectStore,
        max_scanned: u32,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<RunScanWindow, CoreError> {
        if self.runs.entries > MAX_HARD_SCAN_RUNS {
            return Err(CoreError::Typed {
                code: quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                message: "lexical hard recovery exceeds supported run scan bound".into(),
            });
        }
        self.scan_window_inner(store, max_scanned, budget, true)
    }

    fn scan_window_inner(
        &self,
        store: &ObjectStore,
        max_scanned: u32,
        budget: Option<&RequestBudgetV1>,
        stop_at_round_end: bool,
    ) -> Result<RunScanWindow, CoreError> {
        self.validate()?;
        if max_scanned == 0 {
            return Err(invalid("zero candidate scan limit"));
        }
        let issued_high_water = self
            .next_run_id
            .checked_sub(1)
            .ok_or_else(|| corrupt("issued run high-water underflows"))?;
        let mut high = self.selection_progress.scan_high_water_run_id;
        let pending = self.selection_progress.pending_run;
        let mut next = self.selection_progress.next_scan_run_id;
        if high == 0 {
            high = issued_high_water;
        }
        let mut candidates = Vec::new();
        let mut seen = BTreeSet::new();
        let mut round_complete = false;
        while candidates.len()
            < usize::try_from(max_scanned).map_err(|_error| invalid("scan limit width"))?
        {
            if high == 0 || self.runs.entries == 0 {
                break;
            }
            if next > high {
                if pending.is_some() {
                    next = 1;
                } else {
                    // A complete non-anchor pass refreshes the frozen
                    // frontier once, then continues under the same cap.
                    high = issued_high_water;
                    next = 1;
                    round_complete = true;
                    if stop_at_round_end {
                        break;
                    }
                }
            }
            let upper = if let Some(anchor) = pending.filter(|anchor| next <= *anchor) {
                anchor
            } else {
                high.checked_add(1)
                    .ok_or_else(|| corrupt("scan high-water overflows"))?
            };
            if next >= upper {
                round_complete = true;
                break;
            }
            let mut cursor =
                store.map_cursor(&self.runs, &run_key(next), Some(&run_key(upper)), budget)?;
            let Some((key, reference)) = cursor.next()? else {
                if pending.is_some_and(|anchor| next <= anchor) {
                    round_complete = true;
                    break;
                }
                next = high
                    .checked_add(1)
                    .ok_or_else(|| corrupt("scan cursor overflows"))?;
                continue;
            };
            let id = parse_run_key(&key)?;
            if id == 0 || id >= self.next_run_id || !seen.insert(id) {
                round_complete = true;
                break;
            }
            let header = decoded_header(store, &reference, id, budget)?;
            candidates.push(RunCandidate {
                run_id: id,
                members: header.members,
                index_encoded_bytes: header.active_index_encoded_bytes,
            });
            next = id
                .checked_add(1)
                .ok_or_else(|| corrupt("scan cursor overflows"))?;
        }
        Ok(RunScanWindow {
            candidates,
            next_scan_run_id: next,
            scan_high_water_run_id: high,
            round_complete,
        })
    }

    pub(crate) fn object_roots(&self) -> Vec<ObjectRef> {
        self.runs
            .page
            .iter()
            .chain(self.segment_owners.page.iter())
            .map(|page| page.object.clone())
            .collect()
    }

    /// Oldest live cohorts in issued-ID order, bounded by the caller's count.
    #[cfg(test)]
    pub(crate) fn oldest_runs(
        &self,
        store: &ObjectStore,
        max_runs: u32,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Vec<RunCandidate>, CoreError> {
        self.validate()?;
        let limit = usize::try_from(max_runs)
            .map_err(|_error| invalid("run selection count exceeds usize"))?;
        let reserve = usize::try_from(self.runs.entries.min(u64::from(max_runs)))
            .map_err(|_error| invalid("run selection reserve exceeds usize"))?;
        let mut selected = Vec::new();
        selected
            .try_reserve_exact(reserve)
            .map_err(|error| CoreError::Storage(format!("reserve run selection: {error}")))?;
        let mut cursor = store.map_cursor(&self.runs, &[], None, budget)?;
        while selected.len() < limit {
            let Some((key, reference)) = cursor.next()? else {
                break;
            };
            let run_id = parse_run_key(&key)?;
            if run_id == 0 || run_id >= self.next_run_id {
                return Err(corrupt("selected run ID is outside issued range"));
            }
            let header = decoded_header(store, &reference, run_id, budget)?;
            selected.push(RunCandidate {
                run_id,
                members: header.members,
                index_encoded_bytes: header.active_index_encoded_bytes,
            });
        }
        Ok(selected)
    }

    /// Visit one selected cohort without retaining its member list. Every row
    /// must agree with the owner index, and the final count/bytes with header.
    pub(crate) fn visit_run_members(
        &self,
        store: &ObjectStore,
        candidate: &RunCandidate,
        budget: Option<&RequestBudgetV1>,
        visitor: &mut RunMemberVisitor<'_>,
    ) -> Result<(), CoreError> {
        self.validate()?;
        if candidate.run_id == 0 || candidate.run_id >= self.next_run_id {
            return Err(corrupt("selected run ID is outside issued range"));
        }
        let header_ref = store
            .map_get(&self.runs, &run_key(candidate.run_id), budget)?
            .ok_or_else(|| corrupt("selected run is absent"))?;
        let header = decoded_header(store, &header_ref, candidate.run_id, budget)?;
        let expected_index_bytes = candidate.index_encoded_bytes;
        if header.members != candidate.members
            || header.active_index_encoded_bytes != expected_index_bytes
        {
            return Err(corrupt("selected run changed since selection"));
        }
        let mut cursor = store.map_cursor(&candidate.members, &[], None, budget)?;
        let mut count = 0_u64;
        let mut bytes = 0_u64;
        while let Some((key, member_ref)) = cursor.next()? {
            let member = decoded_member(store, &member_ref, budget)?;
            if member.run_id != candidate.run_id
                || store.map_get(&self.segment_owners, &key, budget)? != Some(member_ref)
            {
                return Err(corrupt("selected run member differs from owner"));
            }
            count = count
                .checked_add(1)
                .ok_or_else(|| corrupt("selected run count overflow"))?;
            bytes = bytes
                .checked_add(member.index_encoded_bytes)
                .ok_or_else(|| corrupt("selected run bytes overflow"))?;
            visitor(&key, &member.reference, member.index_encoded_bytes)?;
        }
        if count != candidate.members.entries || bytes != candidate.index_encoded_bytes {
            return Err(corrupt("selected run totals differ from members"));
        }
        Ok(())
    }

    pub(crate) fn snapshot(&self) -> Result<TargetCounters, CoreError> {
        TargetCounters::new(
            self.runs.entries,
            self.segment_owners.entries,
            self.active_index_encoded_bytes,
        )
    }

    fn authenticated_owner(
        &self,
        store: &ObjectStore,
        key: &[u8],
        owner_ref: &BlobRef,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<RunMember, CoreError> {
        let member = decoded_member(store, owner_ref, budget)?;
        if member.run_id == 0 || member.run_id >= self.next_run_id {
            return Err(corrupt("member names an invalid run ID"));
        }
        let run_ref = store
            .map_get(&self.runs, &run_key(member.run_id), budget)?
            .ok_or_else(|| corrupt("owner names an absent run"))?;
        let header = decoded_header(store, &run_ref, member.run_id, budget)?;
        if store.map_get(&header.members, key, budget)? != Some(owner_ref.clone()) {
            return Err(corrupt("owner and member map value differ"));
        }
        Ok(member)
    }

    /// Full offline scrub: streams every run/member and owner row without a
    /// corpus-sized materialized set. Normal publication uses changed keys.
    pub(crate) fn verify_all(
        &self,
        store: &ObjectStore,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        let mut cursor = RunScrubCursor::new(self)?;
        while !cursor.step(store, budget)? {
            // The cursor owns only the current row and checked totals.
        }
        Ok(())
    }

    /// A bounded witness for the segment currently being independently
    /// authenticated by a File or Native verifier.
    pub(crate) fn verify_segment(
        &self,
        store: &ObjectStore,
        key: &[u8],
        reference: &BlobRef,
        index_encoded_bytes: u64,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        let observed = self.verify_segment_owner(store, key, reference, budget)?;
        if observed != index_encoded_bytes {
            return Err(corrupt("segment index byte witness differs"));
        }
        Ok(())
    }

    /// Authenticates one segment's owner/member/header without recounting its
    /// index DAG.
    ///
    /// Streaming scrub callers may compare the returned stored
    /// logical cost against their independently obtained segment witness.
    pub(crate) fn verify_segment_owner(
        &self,
        store: &ObjectStore,
        key: &[u8],
        reference: &BlobRef,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<u64, CoreError> {
        self.validate()?;
        let owner_ref = store
            .map_get(&self.segment_owners, key, budget)?
            .ok_or_else(|| corrupt("segment has no run owner"))?;
        let member = self.authenticated_owner(store, key, &owner_ref, budget)?;
        if &member.reference != reference {
            return Err(corrupt("segment reference differs from run owner"));
        }
        Ok(member.index_encoded_bytes)
    }

    /// Resolve one File segment's canonical run through both owner and run
    /// membership witnesses. An absent or contradictory owner is corruption.
    pub(crate) fn owner_run_id(
        &self,
        store: &ObjectStore,
        segment_digest: [u8; 32],
        budget: Option<&RequestBudgetV1>,
    ) -> Result<u64, CoreError> {
        self.validate()?;
        let owner_ref = store
            .map_get(&self.segment_owners, &segment_digest, budget)?
            .ok_or_else(|| corrupt("File segment has no run owner"))?;
        let member = self.authenticated_owner(store, &segment_digest, &owner_ref, budget)?;
        if member.reference.root.kind != ObjectKind::Segment
            || member.reference.root.sha256 != segment_digest
        {
            return Err(corrupt("File segment digest differs from run member"));
        }
        Ok(member.run_id)
    }

    /// Independently checks only affected owner keys and run membership.
    /// An added run is wholly new, so its complete output membership is read.
    #[cfg(test)]
    pub(crate) fn verify_transition(
        &self,
        target: &Self,
        store: &ObjectStore,
        removed: &[SegmentRemoval],
        added: &[SegmentInput],
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        self.verify_transition_with_progress(
            target,
            store,
            removed,
            added,
            budget,
            self.selection_progress,
        )
    }

    /// Native policy proof supplies its independently derived progress;
    /// ordinary callers require an unchanged selection cursor.
    pub(crate) fn verify_transition_with_progress(
        &self,
        target: &Self,
        store: &ObjectStore,
        removed: &[SegmentRemoval],
        added: &[SegmentInput],
        budget: Option<&RequestBudgetV1>,
        expected_progress: RunSelectionProgress,
    ) -> Result<(), CoreError> {
        self.validate()?;
        target.validate()?;
        if target.selection_progress != expected_progress {
            return Err(corrupt("transition selection progress differs"));
        }
        let removed = sorted_unique_removed(removed)?;
        let added = sorted_unique_added(added)?;
        let new_added = added
            .iter()
            .filter(|segment| {
                removed
                    .binary_search_by(|probe| probe.key.cmp(&segment.key))
                    .is_err()
            })
            .collect::<Vec<_>>();
        let expected_next = self
            .next_run_id
            .checked_add(u64::from(!new_added.is_empty()))
            .ok_or_else(|| invalid("transition next run ID overflow"))?;
        if target.next_run_id != expected_next {
            return Err(corrupt("transition next run ID differs"));
        }

        let mut owner_changes = BTreeMap::<Vec<u8>, (Option<BlobRef>, Option<BlobRef>)>::new();
        let mut old_run_changes =
            BTreeMap::<u64, BTreeMap<Vec<u8>, (BlobRef, Option<BlobRef>, u64, u64)>>::new();
        let mut removed_bytes = 0_u64;
        for segment in &removed {
            let owner_ref = store
                .map_get(&self.segment_owners, &segment.key, budget)?
                .ok_or_else(|| corrupt("removed owner is absent"))?;
            let member = self.authenticated_owner(store, &segment.key, &owner_ref, budget)?;
            if member.reference != segment.reference {
                return Err(corrupt("removed reference differs from owner"));
            }
            let _previous = old_run_changes.entry(member.run_id).or_default().insert(
                segment.key.clone(),
                (owner_ref.clone(), None, member.index_encoded_bytes, 0),
            );
            let _previous = owner_changes.insert(segment.key.clone(), (Some(owner_ref), None));
            removed_bytes = removed_bytes
                .checked_add(member.index_encoded_bytes)
                .ok_or_else(|| corrupt("removed index byte total overflow"))?;
        }

        let mut new_run_members = BTreeMap::<Vec<u8>, (BlobRef, u64)>::new();
        let mut added_bytes = 0_u64;
        let mut new_run_bytes = 0_u64;
        for segment in &added {
            let owner_ref = store
                .map_get(&target.segment_owners, &segment.key, budget)?
                .ok_or_else(|| corrupt("added owner is absent"))?;
            let member = target.authenticated_owner(store, &segment.key, &owner_ref, budget)?;
            let old_owner = owner_changes
                .get(&segment.key)
                .and_then(|change| change.0.clone());
            let expected_run_id = if let Some(old_owner) = old_owner.as_ref() {
                let old_member =
                    self.authenticated_owner(store, &segment.key, old_owner, budget)?;
                if old_member.reference == segment.reference
                    && old_member.index_encoded_bytes == segment.index_encoded_bytes
                {
                    return Err(corrupt("declared segment rewrite has no changed member"));
                }
                let changed = old_run_changes
                    .get_mut(&old_member.run_id)
                    .and_then(|members| members.get_mut(&segment.key))
                    .ok_or_else(|| corrupt("rewritten key has no prior run change"))?;
                changed.1 = Some(owner_ref.clone());
                changed.3 = segment.index_encoded_bytes;
                old_member.run_id
            } else {
                self.next_run_id
            };
            if member.run_id != expected_run_id
                || member.reference != segment.reference
                || member.index_encoded_bytes != segment.index_encoded_bytes
            {
                return Err(corrupt("added member differs from declared output"));
            }
            if store
                .map_get(&self.segment_owners, &segment.key, budget)?
                .is_some()
                && !owner_changes.contains_key(&segment.key)
            {
                return Err(corrupt("added key retained an undeclared prior owner"));
            }
            let entry = owner_changes
                .entry(segment.key.clone())
                .or_insert((None, None));
            entry.1 = Some(owner_ref.clone());
            if old_owner.is_none() {
                let _previous = new_run_members.insert(
                    segment.key.clone(),
                    (owner_ref, segment.index_encoded_bytes),
                );
                new_run_bytes = new_run_bytes
                    .checked_add(segment.index_encoded_bytes)
                    .ok_or_else(|| corrupt("new run byte total overflow"))?;
            }
            added_bytes = added_bytes
                .checked_add(segment.index_encoded_bytes)
                .ok_or_else(|| corrupt("added index byte total overflow"))?;
        }
        let expected_bytes = self
            .active_index_encoded_bytes
            .checked_sub(removed_bytes)
            .and_then(|bytes| bytes.checked_add(added_bytes))
            .ok_or_else(|| corrupt("transition index bytes underflow or overflow"))?;
        if target.active_index_encoded_bytes != expected_bytes {
            return Err(corrupt("transition index byte total differs"));
        }
        let removed_count = u64::try_from(removed.len())
            .map_err(|_error| corrupt("removed segment count exceeds u64"))?;
        let added_count = u64::try_from(added.len())
            .map_err(|_error| corrupt("added segment count exceeds u64"))?;
        if target.segment_owners.entries
            != self
                .segment_owners
                .entries
                .checked_sub(removed_count)
                .and_then(|count| count.checked_add(added_count))
                .ok_or_else(|| corrupt("transition segment count underflows"))?
        {
            return Err(corrupt("transition owner count differs"));
        }
        store.map_changed_keys(
            &self.segment_owners,
            &target.segment_owners,
            budget,
            &mut |key, before, after| {
                let expected = owner_changes
                    .remove(key)
                    .ok_or_else(|| corrupt("unexpected owner key change"))?;
                if before != expected.0.as_ref() || after != expected.1.as_ref() {
                    return Err(corrupt("changed owner value differs"));
                }
                Ok(())
            },
        )?;
        if !owner_changes.is_empty() {
            return Err(corrupt("declared owner change is absent"));
        }

        let mut expected_run_ids = old_run_changes.keys().copied().collect::<BTreeSet<_>>();
        if !new_added.is_empty() {
            let _inserted = expected_run_ids.insert(self.next_run_id);
        }
        let mut fully_removed_runs = 0_u64;
        store.map_changed_keys(
            &self.runs,
            &target.runs,
            budget,
            &mut |key, before, after| {
                let id = parse_run_key(key)?;
                if !expected_run_ids.remove(&id) {
                    return Err(corrupt("unexpected run header change"));
                }
                if id == self.next_run_id {
                    if before.is_some() || new_added.is_empty() {
                        return Err(corrupt("new run existed before transition"));
                    }
                    let value = after.ok_or_else(|| corrupt("new run header is absent"))?;
                    let header = decoded_header(store, value, id, budget)?;
                    if header.members.entries
                        != u64::try_from(new_added.len())
                            .map_err(|_error| corrupt("new run member count overflow"))?
                        || header.active_index_encoded_bytes != new_run_bytes
                    {
                        return Err(corrupt("new run totals differ from output"));
                    }
                    let mut expected_members = new_run_members.clone();
                    store.map_changed_keys(
                        &MapRoot::default(),
                        &header.members,
                        budget,
                        &mut |member_key, old, new| {
                            let (member_ref, _bytes) = expected_members
                                .remove(member_key)
                                .ok_or_else(|| corrupt("unexpected new run member"))?;
                            if old.is_some() || new != Some(&member_ref) {
                                return Err(corrupt("new run member value differs"));
                            }
                            Ok(())
                        },
                    )?;
                    if !expected_members.is_empty() {
                        return Err(corrupt("new run member is absent"));
                    }
                    return Ok(());
                }
                let old_value = before.ok_or_else(|| corrupt("removed run has no old header"))?;
                let old_header = decoded_header(store, old_value, id, budget)?;
                let changes = old_run_changes
                    .get(&id)
                    .ok_or_else(|| corrupt("changed run has no declared members"))?;
                let removed_run_bytes = changes.values().try_fold(
                    0_u64,
                    |total, (_old, _new, bytes, _new_bytes)| {
                        total
                            .checked_add(*bytes)
                            .ok_or_else(|| corrupt("removed run byte total overflow"))
                    },
                )?;
                let rewritten_run_bytes = changes.values().try_fold(
                    0_u64,
                    |total, (_old, _new, _old_bytes, new_bytes)| {
                        total
                            .checked_add(*new_bytes)
                            .ok_or_else(|| corrupt("rewritten run byte total overflow"))
                    },
                )?;
                let permanently_removed =
                    changes.values().filter(|change| change.1.is_none()).count();
                let expected_count = old_header
                    .members
                    .entries
                    .checked_sub(
                        u64::try_from(permanently_removed)
                            .map_err(|_error| corrupt("removed member count overflow"))?,
                    )
                    .ok_or_else(|| corrupt("removed run member count underflows"))?;
                let expected_run_bytes = old_header
                    .active_index_encoded_bytes
                    .checked_sub(removed_run_bytes)
                    .and_then(|bytes| bytes.checked_add(rewritten_run_bytes))
                    .ok_or_else(|| corrupt("removed run bytes underflow"))?;
                let new_header = after
                    .map(|value| decoded_header(store, value, id, budget))
                    .transpose()?;
                if expected_count == 0 {
                    if new_header.is_some() || expected_run_bytes != 0 {
                        return Err(corrupt("empty run was retained"));
                    }
                    fully_removed_runs = fully_removed_runs
                        .checked_add(1)
                        .ok_or_else(|| corrupt("removed run count overflow"))?;
                } else if new_header.as_ref().is_none_or(|header| {
                    header.members.entries != expected_count
                        || header.active_index_encoded_bytes != expected_run_bytes
                }) {
                    return Err(corrupt("updated run totals differ"));
                }
                let new_members = new_header.map_or_else(MapRoot::default, |header| header.members);
                let mut expected_members = changes.clone();
                store.map_changed_keys(
                    &old_header.members,
                    &new_members,
                    budget,
                    &mut |member_key, old, new| {
                        let (old_ref, new_ref, _old_bytes, _new_bytes) = expected_members
                            .remove(member_key)
                            .ok_or_else(|| corrupt("unexpected old run member change"))?;
                        if old != Some(&old_ref) || new != new_ref.as_ref() {
                            return Err(corrupt("changed run member value differs"));
                        }
                        Ok(())
                    },
                )?;
                if !expected_members.is_empty() {
                    return Err(corrupt("changed run member is absent"));
                }
                Ok(())
            },
        )?;
        if !expected_run_ids.is_empty() {
            return Err(corrupt("declared run header change is absent"));
        }
        let expected_runs = self
            .runs
            .entries
            .checked_sub(fully_removed_runs)
            .and_then(|count| count.checked_add(u64::from(!new_added.is_empty())))
            .ok_or_else(|| corrupt("transition run count underflow or overflow"))?;
        if target.runs.entries != expected_runs {
            return Err(corrupt("transition run count differs"));
        }
        Ok(())
    }

    /// Transfer whole immutable segment cohorts into one newly issued run.
    ///
    /// No segment is rebuilt: the new owner records change only `run_id`.
    /// The caller must independently authorize the complete selected IDs and
    /// bound their input bytes before invoking this optional maintenance.
    pub(crate) fn merge_cohorts(
        &self,
        store: &ObjectStore,
        lease: &AttemptLease,
        selected_run_ids: &[u64],
        progress: RunSelectionProgress,
    ) -> Result<Self, CoreError> {
        let (members, total_bytes) =
            self.authenticated_cohort_union(store, selected_run_ids, None)?;
        let new_run_id = self.next_run_id;
        let next_run_id = new_run_id
            .checked_add(1)
            .ok_or_else(|| invalid("cohort merge run ID overflow"))?;
        if store
            .map_get(&self.runs, &run_key(new_run_id), None)?
            .is_some()
        {
            return Err(corrupt("new cohort run ID already exists"));
        }
        let mut new_members = Vec::new();
        let mut owner_edits = Vec::new();
        new_members
            .try_reserve_exact(members.len())
            .map_err(|error| CoreError::Storage(format!("reserve merged run members: {error}")))?;
        owner_edits
            .try_reserve_exact(members.len())
            .map_err(|error| CoreError::Storage(format!("reserve merged owner edits: {error}")))?;
        for (key, member) in members {
            let new_owner = persist_member(
                store,
                lease,
                &RunMember {
                    version: RUN_WIRE_VERSION,
                    run_id: new_run_id,
                    reference: member.segment,
                    index_encoded_bytes: member.index_encoded_bytes,
                },
            )?;
            new_members.push(MapEdit {
                key: key.clone(),
                value: Some(new_owner.clone()),
            });
            owner_edits.push(MapEdit {
                key,
                value: Some(new_owner),
            });
        }
        let members =
            store.map_apply(lease, &MapRoot::default(), new_members.into_iter().map(Ok))?;
        let new_header = persist_header(
            store,
            lease,
            &RunHeader {
                version: RUN_WIRE_VERSION,
                run_id: new_run_id,
                members,
                active_index_encoded_bytes: total_bytes,
            },
        )?;
        let mut run_edits = selected_run_ids
            .iter()
            .map(|id| MapEdit {
                key: run_key(*id),
                value: None,
            })
            .collect::<Vec<_>>();
        run_edits.push(MapEdit {
            key: run_key(new_run_id),
            value: Some(new_header),
        });
        let runs = store.map_apply(lease, &self.runs, run_edits.into_iter().map(Ok))?;
        let segment_owners =
            store.map_apply(lease, &self.segment_owners, owner_edits.into_iter().map(Ok))?;
        let target = Self {
            runs,
            segment_owners,
            next_run_id,
            active_index_encoded_bytes: self.active_index_encoded_bytes,
            selection_progress: progress,
        };
        target.validate()?;
        Ok(target)
    }

    /// Independent proof for an unchanged-physical-reference cohort transfer.
    ///
    /// All selected old headers and members, the exact new union, every owner
    /// rewrite, counters, and selection progress must agree. It accepts no
    /// ordinary segment additions/removals or undeclared run changes.
    pub(crate) fn verify_cohort_merge_transition(
        &self,
        target: &Self,
        store: &ObjectStore,
        selected_run_ids: &[u64],
        expected_progress: RunSelectionProgress,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        let (mut members, total_bytes) =
            self.authenticated_cohort_union(store, selected_run_ids, budget)?;
        target.validate()?;
        if target.selection_progress != expected_progress
            || target.next_run_id
                != self
                    .next_run_id
                    .checked_add(1)
                    .ok_or_else(|| corrupt("merged run ID overflows"))?
            || target.active_index_encoded_bytes != self.active_index_encoded_bytes
            || target.segment_owners.entries != self.segment_owners.entries
            || target.runs.entries
                != self
                    .runs
                    .entries
                    .checked_sub(
                        u64::try_from(selected_run_ids.len())
                            .map_err(|_error| corrupt("selected run count overflows"))?,
                    )
                    .and_then(|count| count.checked_add(1))
                    .ok_or_else(|| corrupt("merged run count underflows"))?
        {
            return Err(corrupt("cohort merge counters or progress differ"));
        }
        let mut changed_owners = 0_u64;
        store.map_changed_keys(
            &self.segment_owners,
            &target.segment_owners,
            budget,
            &mut |key, before, after| {
                let member = members
                    .remove(key)
                    .ok_or_else(|| corrupt("unrelated owner changed in cohort merge"))?;
                if before != Some(&member.old_owner) {
                    return Err(corrupt("cohort merge prior owner differs"));
                }
                let after = after.ok_or_else(|| corrupt("cohort merge dropped an owner"))?;
                let moved = decoded_member(store, after, budget)?;
                if moved.run_id != self.next_run_id
                    || moved.reference != member.segment
                    || moved.index_encoded_bytes != member.index_encoded_bytes
                {
                    return Err(corrupt("cohort merge changed segment reference or bytes"));
                }
                changed_owners = changed_owners
                    .checked_add(1)
                    .ok_or_else(|| corrupt("cohort merge owner count overflows"))?;
                Ok(())
            },
        )?;
        if !members.is_empty() || changed_owners == 0 {
            return Err(corrupt("cohort merge did not transfer every member"));
        }
        let mut changed_old = selected_run_ids.iter().copied().collect::<BTreeSet<_>>();
        let mut saw_new = false;
        store.map_changed_keys(
            &self.runs,
            &target.runs,
            budget,
            &mut |key, before, after| {
                let id = parse_run_key(key)?;
                if id == self.next_run_id {
                    if saw_new || before.is_some() {
                        return Err(corrupt("cohort merge new run is not fresh"));
                    }
                    let after = after.ok_or_else(|| corrupt("cohort merge new run is absent"))?;
                    let header = decoded_header(store, after, id, budget)?;
                    if header.members.entries != changed_owners
                        || header.active_index_encoded_bytes != total_bytes
                    {
                        return Err(corrupt("cohort merge new header totals differ"));
                    }
                    saw_new = true;
                } else if changed_old.remove(&id) {
                    if before.is_none() || after.is_some() {
                        return Err(corrupt("cohort merge retained an old run"));
                    }
                } else {
                    return Err(corrupt("unrelated run changed in cohort merge"));
                }
                Ok(())
            },
        )?;
        if !saw_new || !changed_old.is_empty() {
            return Err(corrupt("cohort merge run change is incomplete"));
        }
        let new_candidate = target
            .candidate_by_id(store, self.next_run_id, budget)?
            .ok_or_else(|| corrupt("cohort merge new run is absent"))?;
        let mut observed = BTreeSet::new();
        target.visit_run_members(
            store,
            &new_candidate,
            budget,
            &mut |key, reference, _bytes| {
                if !observed.insert(key.to_vec()) {
                    return Err(corrupt("cohort merge repeats a segment"));
                }
                let old_owner = store
                    .map_get(&self.segment_owners, key, budget)?
                    .ok_or_else(|| corrupt("cohort merge member lacked prior owner"))?;
                let old_member = self.authenticated_owner(store, key, &old_owner, budget)?;
                if old_member.reference != *reference
                    || selected_run_ids.binary_search(&old_member.run_id).is_err()
                {
                    return Err(corrupt("cohort merge has an unrelated or changed member"));
                }
                Ok(())
            },
        )?;
        if u64::try_from(observed.len())
            .map_err(|_error| corrupt("cohort merge member count exceeds u64"))?
            != changed_owners
        {
            return Err(corrupt("cohort merge new member union differs"));
        }
        Ok(())
    }

    fn authenticated_cohort_union(
        &self,
        store: &ObjectStore,
        selected_run_ids: &[u64],
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(BTreeMap<Vec<u8>, CohortTransferMember>, u64), CoreError> {
        self.validate()?;
        if selected_run_ids.len() < 2
            || selected_run_ids
                .windows(2)
                .any(|pair| matches!(pair, [left, right] if left >= right))
        {
            return Err(invalid("cohort merge needs sorted distinct run IDs"));
        }
        let mut members = BTreeMap::new();
        let mut total_bytes = 0_u64;
        for id in selected_run_ids {
            let candidate = self
                .candidate_by_id(store, *id, budget)?
                .ok_or_else(|| corrupt("selected cohort run is absent"))?;
            self.visit_run_members(store, &candidate, budget, &mut |key, segment, bytes| {
                let old_owner = store
                    .map_get(&self.segment_owners, key, budget)?
                    .ok_or_else(|| corrupt("selected cohort owner is absent"))?;
                if members
                    .insert(
                        key.to_vec(),
                        CohortTransferMember {
                            old_owner,
                            segment: segment.clone(),
                            index_encoded_bytes: bytes,
                        },
                    )
                    .is_some()
                {
                    return Err(corrupt("selected cohorts repeat a segment"));
                }
                total_bytes = total_bytes
                    .checked_add(bytes)
                    .ok_or_else(|| corrupt("selected cohort bytes overflow"))?;
                Ok(())
            })?;
        }
        Ok((members, total_bytes))
    }

    pub(crate) fn apply(
        &self,
        store: &ObjectStore,
        lease: &AttemptLease,
        removed: &[SegmentRemoval],
        added: &[SegmentInput],
    ) -> Result<Self, CoreError> {
        self.validate()?;
        let removed = sorted_unique_removed(removed)?;
        let added = sorted_unique_added(added)?;
        if removed.is_empty() && added.is_empty() {
            return Ok(self.clone());
        }
        let mut owner_edits = BTreeMap::<Vec<u8>, Option<BlobRef>>::new();
        let mut existing = BTreeMap::<u64, ExistingRunChange>::new();
        let mut removed_bytes = 0_u64;
        for segment in &removed {
            let owner_ref = store
                .map_get(&self.segment_owners, &segment.key, None)?
                .ok_or_else(|| invalid("removed segment has no owner"))?;
            let member = decoded_member(store, &owner_ref, None)?;
            if member.reference != segment.reference
                || member.run_id == 0
                || member.run_id >= self.next_run_id
            {
                return Err(invalid("removed segment reference or owner differs"));
            }
            let replacement = match added.binary_search_by(|probe| probe.key.cmp(&segment.key)) {
                Ok(position) => Some(
                    added
                        .get(position)
                        .ok_or_else(|| corrupt("matched segment index disappeared"))?,
                ),
                Err(_) => None,
            };
            if replacement.is_some_and(|new| {
                new.reference == member.reference
                    && new.index_encoded_bytes == member.index_encoded_bytes
            }) {
                return Err(invalid("declared segment rewrite has no changed member"));
            }
            let change = match existing.entry(member.run_id) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let run_ref = store
                        .map_get(&self.runs, &run_key(member.run_id), None)?
                        .ok_or_else(|| corrupt("owner names an absent run"))?;
                    let header = decoded_header(store, &run_ref, member.run_id, None)?;
                    entry.insert(ExistingRunChange {
                        header,
                        member_edits: Vec::new(),
                        removed_index_bytes: 0,
                        added_index_bytes: 0,
                    })
                }
            };
            if store.map_get(&change.header.members, &segment.key, None)? != Some(owner_ref) {
                return Err(corrupt("owner and run member value differ"));
            }
            let new_member_ref = replacement
                .map(|new| {
                    persist_member(
                        store,
                        lease,
                        &RunMember {
                            version: RUN_WIRE_VERSION,
                            run_id: member.run_id,
                            reference: new.reference.clone(),
                            index_encoded_bytes: new.index_encoded_bytes,
                        },
                    )
                })
                .transpose()?;
            change.member_edits.push(MapEdit {
                key: segment.key.clone(),
                value: new_member_ref.clone(),
            });
            change.removed_index_bytes = change
                .removed_index_bytes
                .checked_add(member.index_encoded_bytes)
                .ok_or_else(|| invalid("removed run byte count overflow"))?;
            if let Some(new) = replacement {
                change.added_index_bytes = change
                    .added_index_bytes
                    .checked_add(new.index_encoded_bytes)
                    .ok_or_else(|| invalid("rewritten run byte count overflow"))?;
            }
            removed_bytes = removed_bytes
                .checked_add(member.index_encoded_bytes)
                .ok_or_else(|| invalid("removed target byte count overflow"))?;
            let _previous = owner_edits.insert(segment.key.clone(), new_member_ref);
        }
        let new_segments = added
            .iter()
            .filter(|segment| {
                removed
                    .binary_search_by(|probe| probe.key.cmp(&segment.key))
                    .is_err()
            })
            .collect::<Vec<_>>();
        for segment in &added {
            if store
                .map_get(&self.segment_owners, &segment.key, None)?
                .is_some()
                && !owner_edits.contains_key(&segment.key)
            {
                return Err(invalid("added segment key already has an owner"));
            }
        }
        let next_run_id = if new_segments.is_empty() {
            self.next_run_id
        } else {
            if self.next_run_id == 0
                || store
                    .map_get(&self.runs, &run_key(self.next_run_id), None)?
                    .is_some()
            {
                return Err(corrupt("next run ID is absent or already used"));
            }
            self.next_run_id
                .checked_add(1)
                .ok_or_else(|| invalid("next run ID overflow"))?
        };
        let added_bytes = added.iter().try_fold(0_u64, |total, segment| {
            total
                .checked_add(segment.index_encoded_bytes)
                .ok_or_else(|| invalid("added target byte count overflow"))
        })?;
        let active_index_encoded_bytes = self
            .active_index_encoded_bytes
            .checked_sub(removed_bytes)
            .and_then(|bytes| bytes.checked_add(added_bytes))
            .ok_or_else(|| invalid("target index byte count underflows or overflows"))?;

        let mut run_edits = BTreeMap::<Vec<u8>, Option<BlobRef>>::new();
        for (run_id, change) in existing {
            let members = store.map_apply(
                lease,
                &change.header.members,
                change.member_edits.into_iter().map(Ok),
            )?;
            let bytes = change
                .header
                .active_index_encoded_bytes
                .checked_sub(change.removed_index_bytes)
                .and_then(|bytes| bytes.checked_add(change.added_index_bytes))
                .ok_or_else(|| corrupt("run byte count underflows"))?;
            let value = if members.entries == 0 {
                if bytes != 0 {
                    return Err(corrupt("empty run retains index bytes"));
                }
                None
            } else {
                if bytes == 0 {
                    return Err(corrupt("nonempty run has zero index bytes"));
                }
                Some(persist_header(
                    store,
                    lease,
                    &RunHeader {
                        members,
                        active_index_encoded_bytes: bytes,
                        ..change.header
                    },
                )?)
            };
            let _previous = run_edits.insert(run_key(run_id), value);
        }
        if !new_segments.is_empty() {
            let mut member_edits = Vec::new();
            member_edits
                .try_reserve_exact(new_segments.len())
                .map_err(|error| CoreError::Storage(format!("reserve new run members: {error}")))?;
            for segment in &new_segments {
                let member_ref = persist_member(
                    store,
                    lease,
                    &RunMember {
                        version: RUN_WIRE_VERSION,
                        run_id: self.next_run_id,
                        reference: segment.reference.clone(),
                        index_encoded_bytes: segment.index_encoded_bytes,
                    },
                )?;
                member_edits.push(MapEdit {
                    key: segment.key.clone(),
                    value: Some(member_ref.clone()),
                });
                let _previous = owner_edits.insert(segment.key.clone(), Some(member_ref));
            }
            let members =
                store.map_apply(lease, &MapRoot::default(), member_edits.into_iter().map(Ok))?;
            let header = RunHeader {
                version: RUN_WIRE_VERSION,
                run_id: self.next_run_id,
                members,
                active_index_encoded_bytes: new_segments.iter().try_fold(
                    0_u64,
                    |total, segment| {
                        total
                            .checked_add(segment.index_encoded_bytes)
                            .ok_or_else(|| invalid("new run byte count overflow"))
                    },
                )?,
            };
            let _previous = run_edits.insert(
                run_key(self.next_run_id),
                Some(persist_header(store, lease, &header)?),
            );
        }
        let runs = store.map_apply(
            lease,
            &self.runs,
            run_edits
                .into_iter()
                .map(|(key, value)| Ok(MapEdit { key, value })),
        )?;
        let segment_owners = store.map_apply(
            lease,
            &self.segment_owners,
            owner_edits
                .into_iter()
                .map(|(key, value)| Ok(MapEdit { key, value })),
        )?;
        let target = Self {
            runs,
            segment_owners,
            next_run_id,
            active_index_encoded_bytes,
            selection_progress: self.selection_progress,
        };
        target.validate()?;
        Ok(target)
    }
}

/// Authenticated encoded width of one changed segment's explicit index DAG.
///
/// The caller supplies descriptor/route roots and every posting-page root;
/// native posting inventories do not necessarily encode page dependencies.
/// Source/current/global-map roots are outside this bundle. Body objects are
/// deliberately not traversed even if an index descriptor references one.
/// `max_objects` is the caller's existing traversal safety bound, not a disk
/// or memory reservation. The result is a logical per-segment index cost and
/// may count a shared object again in another segment's bundle.
#[derive(Clone, Debug)]
pub(crate) struct IndexFootprintCursor {
    pending: Vec<ObjectRef>,
    seen: BTreeMap<[u8; 32], ObjectRef>,
    enqueued: u64,
    max_objects: u64,
    encoded_bytes: u64,
    complete: bool,
}

impl IndexFootprintCursor {
    pub(crate) fn new(index_roots: &[ObjectRef], max_objects: u64) -> Result<Self, CoreError> {
        if max_objects == 0 {
            return Err(invalid("index DAG traversal bound is zero"));
        }
        let enqueued = u64::try_from(index_roots.len())
            .map_err(|_error| invalid("index DAG root count exceeds u64"))?;
        if enqueued > max_objects {
            return Err(invalid("index DAG roots exceed caller work bound"));
        }
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(index_roots.len())
            .map_err(|error| CoreError::Storage(format!("reserve index DAG roots: {error}")))?;
        pending.extend_from_slice(index_roots);
        Ok(Self {
            pending,
            seen: BTreeMap::new(),
            enqueued,
            max_objects,
            encoded_bytes: 0,
            complete: false,
        })
    }

    /// Processes at most one queued object. A failing read or cancellation
    /// leaves the queued reference in place, so the step can be retried.
    pub(crate) fn step(
        &mut self,
        store: &ObjectStore,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Option<u64>, CoreError> {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:index-bundle-footprint")?;
        }
        if self.complete || self.pending.is_empty() {
            self.complete = true;
            return Ok(Some(self.encoded_bytes));
        }
        let reference = self
            .pending
            .last()
            .ok_or_else(|| corrupt("index DAG queue unexpectedly empty"))?;
        if let Some(previous) = self.seen.get(&reference.sha256) {
            if previous != reference {
                return Err(corrupt("one digest has conflicting object descriptors"));
            }
            let _already_seen = self.pending.pop();
        } else {
            if u64::try_from(self.seen.len())
                .map_err(|_error| invalid("index DAG seen count exceeds u64"))?
                >= self.max_objects
            {
                return Err(invalid("index DAG exceeds caller work bound"));
            }
            if matches!(
                reference.kind,
                ObjectKind::FileBody | ObjectKind::NativeBody
            ) {
                let reference = self
                    .pending
                    .pop()
                    .ok_or_else(|| corrupt("index DAG queue unexpectedly empty"))?;
                let _previous = self.seen.insert(reference.sha256, reference);
            } else {
                if matches!(
                    reference.kind,
                    ObjectKind::AttemptBatch | ObjectKind::HardScanAnchor
                ) {
                    return Err(corrupt(
                        "attempt control record is not a segment index object",
                    ));
                }
                let dependencies = store.object_dependencies(reference, budget)?;
                let next_bytes = self
                    .encoded_bytes
                    .checked_add(reference.bytes)
                    .ok_or_else(|| invalid("segment index encoded byte count overflow"))?;
                let next_enqueued = self
                    .enqueued
                    .checked_add(
                        u64::try_from(dependencies.len())
                            .map_err(|_error| invalid("index DAG dependency count exceeds u64"))?,
                    )
                    .ok_or_else(|| invalid("index DAG queued count overflow"))?;
                if next_enqueued > self.max_objects {
                    return Err(invalid("index DAG traversal exceeds caller work bound"));
                }
                self.pending
                    .try_reserve(dependencies.len())
                    .map_err(|error| {
                        CoreError::Storage(format!("reserve index DAG work: {error}"))
                    })?;
                let reference = self
                    .pending
                    .pop()
                    .ok_or_else(|| corrupt("index DAG queue unexpectedly empty"))?;
                let _previous = self.seen.insert(reference.sha256, reference);
                self.encoded_bytes = next_bytes;
                self.enqueued = next_enqueued;
                self.pending.extend(dependencies);
            }
        }
        if self.pending.is_empty() {
            self.complete = true;
            Ok(Some(self.encoded_bytes))
        } else {
            Ok(None)
        }
    }
}

pub(crate) fn segment_index_encoded_bytes(
    store: &ObjectStore,
    index_roots: &[ObjectRef],
    budget: Option<&RequestBudgetV1>,
    max_objects: u64,
) -> Result<u64, CoreError> {
    let mut cursor = IndexFootprintCursor::new(index_roots, max_objects)?;
    loop {
        if let Some(bytes) = cursor.step(store, budget)? {
            return Ok(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexical_store::AttemptBinding;
    use std::time::{Duration, Instant};

    fn binding() -> AttemptBinding {
        AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: [2; 32],
            revision_sha256: [3; 32],
            source_event_sha256: [4; 32],
            target_generation: 1,
            base_root_sha256: None,
            operations_sha256: [5; 32],
            format: 1,
        }
    }

    #[test]
    fn bounded_scan_reaches_separated_younger_runs_and_rotates_incompatible_anchor() {
        let dir = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(dir.path()).expect("open store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let mut registry = RunRegistry::empty();
        for id in 1_u64..=20 {
            let reference = store
                .put_blob(
                    &lease,
                    ObjectKind::Segment,
                    &[],
                    format!("segment-{id}").as_bytes(),
                )
                .expect("segment");
            let bytes = match id {
                1 => 13,
                10 | 19 => 8,
                _ => 1_000,
            };
            registry = registry
                .apply(
                    &store,
                    &lease,
                    &[],
                    &[SegmentInput {
                        key: id.to_be_bytes().to_vec(),
                        reference,
                        index_encoded_bytes: bytes,
                    }],
                )
                .expect("append one run");
        }
        let scan = bounded_candidate_scan_limit(2);
        assert_eq!(scan, 8);
        let first = registry
            .scan_window(&store, scan, None)
            .expect("first window");
        assert_eq!(first.candidates.first().map(|row| row.run_id), Some(1));
        assert_eq!(first.candidates.last().map(|row| row.run_id), Some(8));
        let mut progress = RunSelectionProgress {
            next_scan_run_id: 2,
            scan_high_water_run_id: 20,
            pending_run: Some(1),
        };
        registry = registry
            .with_selection_progress(progress)
            .expect("anchor oldest");
        let mut observed_younger = Vec::new();
        let mut completed = false;
        for _ in 0..4 {
            let window = registry
                .scan_window(&store, scan, None)
                .expect("bounded anchor scan");
            assert!(window.candidates.len() <= 8);
            observed_younger.extend(
                window
                    .candidates
                    .iter()
                    .filter(|row| matches!(row.run_id, 10 | 19))
                    .map(|row| row.run_id),
            );
            progress.next_scan_run_id = window.next_scan_run_id;
            completed = window.round_complete;
            registry = registry
                .with_selection_progress(progress)
                .expect("advance bounded scan");
            if completed {
                break;
            }
        }
        assert!(completed, "frozen high-water must complete an anchor round");
        assert_eq!(observed_younger, vec![10, 19]);
        assert!(
            13 + 8 > 20,
            "old anchor cannot pair with either younger run"
        );
        progress.pending_run = None;
        progress.next_scan_run_id = 2;
        registry = registry
            .with_selection_progress(progress)
            .expect("rotate anchor");
        let mut found_ten = false;
        for _ in 0..3 {
            let window = registry
                .scan_window(&store, scan, None)
                .expect("find next anchor");
            progress.next_scan_run_id = window.next_scan_run_id;
            progress.scan_high_water_run_id = window.scan_high_water_run_id;
            if window.candidates.iter().any(|row| row.run_id == 10) {
                found_ten = true;
                break;
            }
            registry = registry
                .with_selection_progress(progress)
                .expect("advance to next anchor");
        }
        assert!(found_ten);
        progress.pending_run = Some(10);
        progress.next_scan_run_id = 11;
        registry = registry
            .with_selection_progress(progress)
            .expect("anchor younger run");
        let mut found_nineteen = false;
        for _ in 0..3 {
            let window = registry
                .scan_window(&store, scan, None)
                .expect("find compatible partner");
            assert!(window.candidates.len() <= 8);
            if window.candidates.iter().any(|row| row.run_id == 19) {
                found_nineteen = true;
                break;
            }
            progress.next_scan_run_id = window.next_scan_run_id;
            registry = registry
                .with_selection_progress(progress)
                .expect("advance partner scan");
        }
        assert!(found_nineteen);
        assert!(
            8 + 8 <= 20,
            "separated younger pair fits the same input cap"
        );
    }

    #[test]
    fn cohort_transfer_preserves_exact_segments_and_rejects_partial_or_false_proof() {
        let dir = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(dir.path()).expect("open store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let mut old = RunRegistry::empty();
        let mut originals = Vec::new();
        for id in 1_u64..=3 {
            let reference = store
                .put_blob(
                    &lease,
                    ObjectKind::Segment,
                    &[],
                    &[u8::try_from(id).expect("byte")],
                )
                .expect("segment");
            old = old
                .apply(
                    &store,
                    &lease,
                    &[],
                    &[SegmentInput {
                        key: id.to_be_bytes().to_vec(),
                        reference: reference.clone(),
                        index_encoded_bytes: id * 11,
                    }],
                )
                .expect("append cohort");
            originals.push(reference);
        }
        let progress = RunSelectionProgress {
            next_scan_run_id: 2,
            scan_high_water_run_id: 3,
            pending_run: None,
        };
        let merged = old
            .merge_cohorts(&store, &lease, &[1, 3], progress)
            .expect("transfer complete cohorts");
        assert_eq!(merged.runs.entries, 2);
        assert_eq!(merged.segment_owners.entries, 3);
        assert_eq!(
            merged.active_index_encoded_bytes,
            old.active_index_encoded_bytes
        );
        assert_eq!(merged.next_run_id, 5);
        for id in 1_u64..=3 {
            merged
                .verify_segment(
                    &store,
                    &id.to_be_bytes(),
                    originals
                        .get(usize::try_from(id - 1).expect("index"))
                        .expect("fixed segment oracle"),
                    id * 11,
                    None,
                )
                .expect("exact segment reference and charge");
        }
        old.verify_cohort_merge_transition(&merged, &store, &[1, 3], progress, None)
            .expect("independent whole-cohort proof");
        assert!(
            old.merge_cohorts(&store, &lease, &[1, 1], progress)
                .is_err()
        );
        assert!(old.merge_cohorts(&store, &lease, &[1], progress).is_err());
        assert!(
            old.verify_cohort_merge_transition(&merged, &store, &[1, 2], progress, None)
                .is_err()
        );
        assert!(
            old.verify_cohort_merge_transition(
                &merged,
                &store,
                &[1, 3],
                RunSelectionProgress::default(),
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn cohorts_preserve_ownership_through_replacement_and_retirement() {
        let dir = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(dir.path()).expect("open store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let first = store
            .put_blob(&lease, ObjectKind::Segment, &[], b"first")
            .expect("first segment");
        let second = store
            .put_blob(&lease, ObjectKind::Segment, &[], b"second")
            .expect("second segment");
        let replacement = store
            .put_blob(&lease, ObjectKind::Segment, &[], b"replacement")
            .expect("replacement segment");
        let rewritten = store
            .put_blob(&lease, ObjectKind::Segment, &[], b"rewritten")
            .expect("rewritten segment");
        let third = store
            .put_blob(&lease, ObjectKind::Segment, &[], b"third")
            .expect("third segment");
        let empty = RunRegistry::empty();
        let initial = empty
            .apply(
                &store,
                &lease,
                &[],
                &[
                    SegmentInput {
                        key: b"a".to_vec(),
                        reference: first.clone(),
                        index_encoded_bytes: 11,
                    },
                    SegmentInput {
                        key: b"b".to_vec(),
                        reference: second.clone(),
                        index_encoded_bytes: 17,
                    },
                ],
            )
            .expect("first cohort");
        assert_eq!(initial.runs.entries, 1);
        assert_eq!(initial.segment_owners.entries, 2);
        assert_eq!(initial.active_index_encoded_bytes, 28);
        initial.verify_all(&store, None).expect("initial scrub");
        assert_eq!(
            initial
                .verify_segment_owner(&store, b"a", &first, None)
                .expect("owner witness"),
            11
        );

        let removed = [SegmentRemoval {
            key: b"a".to_vec(),
            reference: first.clone(),
        }];
        let added = [SegmentInput {
            key: b"a".to_vec(),
            reference: replacement.clone(),
            index_encoded_bytes: 23,
        }];
        let replaced = initial
            .apply(&store, &lease, &removed, &added)
            .expect("replace one member");
        initial
            .verify_transition(&replaced, &store, &removed, &added, None)
            .expect("independent transition witness");
        assert_eq!(replaced.runs.entries, 1);
        assert_eq!(replaced.segment_owners.entries, 2);
        assert_eq!(replaced.active_index_encoded_bytes, 40);
        assert_eq!(replaced.next_run_id, 2);
        assert!(
            replaced
                .verify_segment_owner(&store, b"a", &first, None)
                .is_err()
        );
        assert!(
            replaced.apply(&store, &lease, &removed, &[]).is_err(),
            "stale reference must not retire the replacement"
        );
        let mut false_total = replaced.clone();
        false_total.active_index_encoded_bytes += 1;
        assert!(matches!(
            false_total.verify_all(&store, None),
            Err(CoreError::Typed {
                code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
                ..
            })
        ));
        replaced
            .verify_all(&store, None)
            .expect("replacement scrub");

        let forged_base = initial
            .apply(&store, &lease, &removed, &[])
            .expect("remove old member for forged ownership");
        let forged = forged_base
            .apply(&store, &lease, &[], &added)
            .expect("re-add as wrong cohort");
        assert!(
            initial
                .verify_transition(&forged, &store, &removed, &added, None)
                .is_err()
        );

        let mixed_removed = [SegmentRemoval {
            key: b"a".to_vec(),
            reference: replacement,
        }];
        let mixed_added = [
            SegmentInput {
                key: b"a".to_vec(),
                reference: rewritten.clone(),
                index_encoded_bytes: 29,
            },
            SegmentInput {
                key: b"c".to_vec(),
                reference: third,
                index_encoded_bytes: 31,
            },
        ];
        let mixed = replaced
            .apply(&store, &lease, &mixed_removed, &mixed_added)
            .expect("rewrite and create new cohort");
        replaced
            .verify_transition(&mixed, &store, &mixed_removed, &mixed_added, None)
            .expect("mixed transition witness");
        assert_eq!(mixed.runs.entries, 2);
        assert_eq!(mixed.segment_owners.entries, 3);
        assert_eq!(mixed.active_index_encoded_bytes, 77);
        assert_eq!(mixed.next_run_id, 3);
        let selected = mixed.oldest_runs(&store, 2, None).expect("select cohorts");
        assert_eq!(selected.len(), 2);
        assert_eq!(
            (
                selected.first().expect("first fixed cohort").run_id,
                selected.get(1).expect("second fixed cohort").run_id
            ),
            (1, 2)
        );
        let mut first_run_keys = Vec::new();
        mixed
            .visit_run_members(
                &store,
                selected.first().expect("first fixed cohort"),
                None,
                &mut |key, _reference, _bytes| {
                    first_run_keys.push(key.to_vec());
                    Ok(())
                },
            )
            .expect("stream first cohort");
        assert_eq!(first_run_keys, vec![b"a".to_vec(), b"b".to_vec()]);

        let retirement = [
            SegmentRemoval {
                key: b"a".to_vec(),
                reference: rewritten,
            },
            SegmentRemoval {
                key: b"b".to_vec(),
                reference: second,
            },
        ];
        let retired = mixed
            .apply(&store, &lease, &retirement, &[])
            .expect("retire old run");
        mixed
            .verify_transition(&retired, &store, &retirement, &[], None)
            .expect("full-run retirement witness");
        assert_eq!(retired.runs.entries, 1);
        assert_eq!(retired.segment_owners.entries, 1);
        assert_eq!(retired.active_index_encoded_bytes, 31);
        assert_eq!(retired.next_run_id, 3);
        retired.verify_all(&store, None).expect("retirement scrub");
        let mut cursor = RunScrubCursor::new(&retired).expect("scrub cursor");
        let mut steps = 0;
        while !cursor.step(&store, None).expect("bounded scrub step") {
            steps += 1;
            assert!(steps < 10);
        }
        assert!(steps >= 3, "run, member, and owner require separate steps");
    }

    #[test]
    fn footprint_excludes_body_and_rejects_conflicting_descriptor() {
        let dir = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(dir.path()).expect("open store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let body = store
            .put_blob(&lease, ObjectKind::FileBody, &[], b"source body")
            .expect("body");
        let posting = store
            .put_blob(
                &lease,
                ObjectKind::FilePostings,
                std::slice::from_ref(&body.root),
                b"posting data",
            )
            .expect("posting");
        // QLSTOR01: 21-byte header, 41-byte dependency, 1-byte inline
        // marker, and the 12-byte fixed posting payload.
        assert_eq!(posting.root.bytes, 75);
        let mut cursor = IndexFootprintCursor::new(std::slice::from_ref(&posting.root), 2)
            .expect("bounded footprint cursor");
        assert_eq!(cursor.step(&store, None).expect("descriptor step"), None);
        assert_eq!(
            cursor.step(&store, None).expect("excluded body step"),
            Some(75)
        );
        assert_eq!(
            cursor.step(&store, None).expect("idempotent completion"),
            Some(75)
        );
        let mut cancelled = IndexFootprintCursor::new(std::slice::from_ref(&posting.root), 2)
            .expect("cancellable cursor");
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        budget.cancel_handle().cancel();
        assert!(matches!(
            cancelled.step(&store, Some(&budget)),
            Err(CoreError::Typed {
                code: quanta_index_core::REQUEST_CANCELLED_CODE,
                ..
            })
        ));
        assert_eq!(
            cancelled.step(&store, None).expect("resume after refusal"),
            None
        );
        assert_eq!(
            cancelled.step(&store, None).expect("finish after refusal"),
            Some(75)
        );
        let deadline = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("past instant");
        let expired = RequestBudgetV1::until(deadline);
        let mut timed = IndexFootprintCursor::new(std::slice::from_ref(&posting.root), 2)
            .expect("deadline-bound cursor");
        assert!(matches!(
            timed.step(&store, Some(&expired)),
            Err(CoreError::Typed {
                code: quanta_index_core::REQUEST_DEADLINE_EXCEEDED_CODE,
                ..
            })
        ));
        assert_eq!(
            timed.step(&store, None).expect("resume after deadline"),
            None
        );
        let mut empty = IndexFootprintCursor::new(&[], 1).expect("empty cursor");
        assert_eq!(empty.step(&store, None).expect("empty completion"), Some(0));
        assert!(IndexFootprintCursor::new(&[], 0).is_err());
        assert!(IndexFootprintCursor::new(&[posting.root.clone(), body.root], 1).is_err());
        assert_eq!(
            segment_index_encoded_bytes(
                &store,
                &[posting.root.clone(), posting.root.clone()],
                None,
                3,
            )
            .expect("deduplicated index footprint"),
            posting.root.bytes
        );
        assert!(
            segment_index_encoded_bytes(&store, std::slice::from_ref(&posting.root), None, 1)
                .is_err()
        );
        let mut conflicting = posting.root.clone();
        conflicting.bytes += 1;
        assert!(
            segment_index_encoded_bytes(&store, &[conflicting, posting.root], None, 3,).is_err()
        );
    }
}
