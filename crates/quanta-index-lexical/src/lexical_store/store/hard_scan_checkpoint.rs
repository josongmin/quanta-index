//! Bounded durable hard-recovery scan progress owned by an exact attempt.

use std::fs::File;
use std::io::Read;
use std::os::unix::fs::MetadataExt as _;

use quanta_index_core::{CoreError, RequestBudgetV1};
use rustix::fs::{Mode, OFlags, openat};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{
    AtomicPurpose, AttemptBinding, AttemptLease, ObjectKind, ObjectRef, ObjectStore, corrupt,
    invalid, storage,
};

const MAGIC: [u8; 8] = *b"QLHSCAN1";
const FORMAT: u16 = 1;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_RUNS: usize = 4096;
const MAX_TOTAL_RUNS: usize = 8192;
const WINDOW_RUNS: usize = 32;
// 4096 live headers / 32 per window, plus one possible empty wrap marker.
const MAX_WINDOWS: usize = 129;
const MAX_ANCHORS: usize = MAX_WINDOWS * 2;
const ANCHOR_MAGIC: [u8; 8] = *b"QIHANCH1";
const ANCHOR_FORMAT: u16 = 1;
const MAX_ANCHOR_PAYLOAD_BYTES: usize = 384;
const MAX_ANCHOR_OBJECT_BYTES: u64 = 21 + 384;

type LaneIdentity = (Authority, [u8; 32], u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(crate) enum Authority {
    File,
    Native,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct CandidateWitness {
    pub(crate) run_id: u64,
    pub(crate) index_encoded_bytes: u64,
    pub(crate) eligibility: EligibilityWitness,
    /// Digest over the independently decoded run header and its authenticated
    /// membership/cost fields, with an authority-specific domain separator.
    pub(crate) header_sha256: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum IneligibleReason {
    InputCap,
    TouchedOrDebt,
    RetiredDueOrStale,
    PartialSegment,
    StagingCap,
    EmptyCohort,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum EligibilityWitness {
    FileEligible {
        effective_input_encoded_bytes: u64,
    },
    NativeEligible {
        effective_input_encoded_bytes: u64,
        staging_bytes: u64,
    },
    Ineligible(IneligibleReason),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct WindowWitness {
    pub(crate) ordinal: u16,
    pub(crate) prior_chain_sha256: [u8; 32],
    pub(crate) start_run_id: u64,
    pub(crate) next_run_id: u64,
    pub(crate) frozen_high_water_run_id: u64,
    pub(crate) round_complete: bool,
    pub(crate) candidates: Vec<CandidateWitness>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ScanLane {
    pub(crate) authority: Authority,
    pub(crate) base_authority_root_sha256: [u8; 32],
    pub(crate) frozen_high_water_run_id: u64,
    pub(crate) windows: Vec<WindowWitness>,
}

impl ScanLane {
    pub(crate) fn next_progress(&self) -> crate::lexical_runs::RunSelectionProgress {
        crate::lexical_runs::RunSelectionProgress {
            next_scan_run_id: self.windows.last().map_or(1, |window| window.next_run_id),
            scan_high_water_run_id: self.frozen_high_water_run_id,
            pending_run: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct CheckpointPayload {
    pub(crate) magic: [u8; 8],
    pub(crate) format: u16,
    /// Includes incarnation, repo, revision, source event, generation, base
    /// root, operations, and attempt format. Identity filename alone is weaker.
    pub(crate) attempt: AttemptBinding,
    pub(crate) policy_sha256: [u8; 32],
    pub(crate) lanes: Vec<ScanLane>,
}

pub(crate) trait IndependentWindowOracle {
    /// Recompute exactly one window from the pinned old authority root. It must
    /// decode headers, membership, and cost using verifier code, not copy the
    /// persisted transcript or invoke producer selection.
    fn replay(
        &mut self,
        lane: Authority,
        base_authority_root_sha256: [u8; 32],
        start_run_id: u64,
        frozen_high_water_run_id: u64,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<WindowWitness, CoreError>;
}

fn checkpoint_budget(budget: Option<&RequestBudgetV1>) -> Result<(), CoreError> {
    if let Some(budget) = budget {
        budget.checkpoint("lexical hard scan checkpoint")?;
    }
    Ok(())
}

fn same_window_content(actual: &WindowWitness, claimed: &WindowWitness) -> bool {
    actual.start_run_id == claimed.start_run_id
        && actual.next_run_id == claimed.next_run_id
        && actual.frozen_high_water_run_id == claimed.frozen_high_water_run_id
        && actual.round_complete == claimed.round_complete
        && actual.candidates == claimed.candidates
}

impl CheckpointPayload {
    pub(crate) fn empty(attempt: AttemptBinding, policy_sha256: [u8; 32]) -> Self {
        Self {
            magic: MAGIC,
            format: FORMAT,
            attempt,
            policy_sha256,
            lanes: Vec::new(),
        }
    }

    pub(crate) fn digest(&self) -> Result<[u8; 32], CoreError> {
        digest(self)
    }

    pub(crate) fn decorate_next_window(
        &self,
        lane_identity: (Authority, [u8; 32], u64),
        mut independently_observed: WindowWitness,
    ) -> Result<WindowWitness, CoreError> {
        self.validate_shape()?;
        let seed_lane = ScanLane {
            authority: lane_identity.0,
            base_authority_root_sha256: lane_identity.1,
            frozen_high_water_run_id: lane_identity.2,
            windows: Vec::new(),
        };
        let mut chain = chain_seed(self, &seed_lane)?;
        let ordinal = if let Some(lane) = self
            .lanes
            .iter()
            .find(|lane| lane.authority == lane_identity.0)
        {
            if (
                lane.base_authority_root_sha256,
                lane.frozen_high_water_run_id,
            ) != (lane_identity.1, lane_identity.2)
            {
                return Err(invalid("hard-scan checkpoint authority binding differs"));
            }
            for old in &lane.windows {
                chain = advance_chain(chain, old)?;
            }
            lane.windows.len()
        } else {
            0
        };
        independently_observed.ordinal = u16::try_from(ordinal)
            .map_err(|_width| invalid("hard-scan checkpoint ordinal width"))?;
        independently_observed.prior_chain_sha256 = chain;
        Ok(independently_observed)
    }

    fn validate_shape(&self) -> Result<(), CoreError> {
        if self.magic != MAGIC || self.format != FORMAT || self.policy_sha256 == [0; 32] {
            return Err(corrupt("hard-scan checkpoint header or policy digest"));
        }
        if self.lanes.len() > 2
            || self
                .lanes
                .iter()
                .zip(self.lanes.iter().skip(1))
                .any(|(left, right)| left.authority >= right.authority)
        {
            return Err(corrupt("hard-scan checkpoint lane order"));
        }
        for lane in &self.lanes {
            if lane.base_authority_root_sha256 == [0; 32]
                || lane.frozen_high_water_run_id == 0
                || lane.windows.len() > MAX_WINDOWS
            {
                return Err(corrupt("hard-scan checkpoint lane bound"));
            }
            let mut next = 1_u64;
            let mut seen = 0_usize;
            let mut complete = false;
            let mut chain = chain_seed(self, lane)?;
            let upper = lane
                .frozen_high_water_run_id
                .checked_add(1)
                .ok_or_else(|| corrupt("hard-scan frozen frontier overflows"))?;
            for (ordinal, window) in lane.windows.iter().enumerate() {
                let end_exclusive = if window.round_complete {
                    upper
                } else {
                    window.next_run_id
                };
                if complete
                    || usize::from(window.ordinal) != ordinal
                    || window.prior_chain_sha256 != chain
                    || window.start_run_id != next
                    || window.frozen_high_water_run_id != lane.frozen_high_water_run_id
                    || window.candidates.len() > WINDOW_RUNS
                    || (window.round_complete && (window.next_run_id != 1 || next > upper))
                    || (!window.round_complete
                        && (window.next_run_id <= next || window.next_run_id > upper))
                {
                    return Err(corrupt("hard-scan checkpoint cursor or window bound"));
                }
                let mut last_id = next.saturating_sub(1);
                for candidate in &window.candidates {
                    if candidate.run_id <= last_id
                        || candidate.run_id >= end_exclusive
                        || candidate.header_sha256 == [0; 32]
                        || matches!(
                            (lane.authority, candidate.eligibility),
                            (Authority::File, EligibilityWitness::NativeEligible { .. })
                                | (Authority::Native, EligibilityWitness::FileEligible { .. })
                        )
                        || matches!(candidate.eligibility,
                            EligibilityWitness::FileEligible { effective_input_encoded_bytes }
                            if effective_input_encoded_bytes < candidate.index_encoded_bytes)
                        || matches!(candidate.eligibility,
                            EligibilityWitness::NativeEligible { effective_input_encoded_bytes, .. }
                            if effective_input_encoded_bytes != candidate.index_encoded_bytes)
                    {
                        return Err(corrupt("hard-scan checkpoint candidate order"));
                    }
                    last_id = candidate.run_id;
                }
                seen = seen
                    .checked_add(window.candidates.len())
                    .ok_or_else(|| corrupt("hard-scan checkpoint count overflow"))?;
                if seen > MAX_RUNS {
                    return Err(corrupt("hard-scan checkpoint run bound"));
                }
                next = window.next_run_id;
                complete = window.round_complete;
                chain = advance_chain(chain, window)?;
            }
        }
        Ok(())
    }

    fn validate_bound(
        &self,
        expected_attempt: &AttemptBinding,
        expected_policy_sha256: [u8; 32],
        expected_roots_and_highwaters: &[(Authority, [u8; 32], u64)],
    ) -> Result<(), CoreError> {
        self.validate_shape()?;
        if expected_roots_and_highwaters.is_empty()
            || expected_roots_and_highwaters.len() > 2
            || expected_roots_and_highwaters
                .iter()
                .enumerate()
                .any(|(index, expected)| {
                    expected.1 == [0; 32]
                        || expected.2 == 0
                        || expected_roots_and_highwaters
                            .iter()
                            .take(index)
                            .any(|prior| prior.0 == expected.0)
                })
        {
            return Err(invalid("hard-scan expected authority set differs"));
        }
        if &self.attempt != expected_attempt || self.policy_sha256 != expected_policy_sha256 {
            return Err(invalid(
                "hard-scan checkpoint attempt or policy binding differs",
            ));
        }
        for lane in &self.lanes {
            if !expected_roots_and_highwaters.iter().any(|expected| {
                *expected
                    == (
                        lane.authority,
                        lane.base_authority_root_sha256,
                        lane.frozen_high_water_run_id,
                    )
            }) {
                return Err(invalid(
                    "hard-scan checkpoint base authority root or frontier differs",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn replay_verified(
        &self,
        expected_attempt: &AttemptBinding,
        expected_policy_sha256: [u8; 32],
        expected_roots_and_highwaters: &[(Authority, [u8; 32], u64)],
        oracle: &mut dyn IndependentWindowOracle,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        checkpoint_budget(budget)?;
        self.validate_bound(
            expected_attempt,
            expected_policy_sha256,
            expected_roots_and_highwaters,
        )?;
        for lane in &self.lanes {
            for claimed in &lane.windows {
                checkpoint_budget(budget)?;
                let actual = oracle.replay(
                    lane.authority,
                    lane.base_authority_root_sha256,
                    claimed.start_run_id,
                    lane.frozen_high_water_run_id,
                    budget,
                )?;
                if !same_window_content(&actual, claimed) {
                    return Err(corrupt(
                        "hard-scan checkpoint window differs from pinned base",
                    ));
                }
                checkpoint_budget(budget)?;
            }
        }
        Ok(())
    }

    /// Call before a hard-limit admission decision or seal. A partial scan
    /// may be resumed, but it cannot silently qualify a complete scan.
    pub(crate) fn require_complete(
        &self,
        expected_roots_and_highwaters: &[(Authority, [u8; 32], u64)],
    ) -> Result<(), CoreError> {
        self.validate_shape()?;
        if expected_roots_and_highwaters.is_empty()
            || self.lanes.len() != expected_roots_and_highwaters.len()
            || expected_roots_and_highwaters
                .iter()
                .enumerate()
                .any(|(index, expected)| {
                    expected_roots_and_highwaters
                        .iter()
                        .take(index)
                        .any(|prior| prior.0 == expected.0)
                })
        {
            return Err(corrupt("hard-scan checkpoint authority set differs"));
        }
        for expected in expected_roots_and_highwaters {
            let lane = self
                .lanes
                .iter()
                .find(|lane| {
                    (
                        lane.authority,
                        lane.base_authority_root_sha256,
                        lane.frozen_high_water_run_id,
                    ) == *expected
                })
                .ok_or_else(|| corrupt("hard-scan checkpoint missing authority lane"))?;
            if !lane
                .windows
                .last()
                .is_some_and(|window| window.round_complete)
            {
                return Err(corrupt("hard-scan checkpoint authority scan incomplete"));
            }
        }
        Ok(())
    }
}

fn digest(payload: &CheckpointPayload) -> Result<[u8; 32], CoreError> {
    let bytes = encode_body(payload)?;
    let mut hash = Sha256::new();
    hash.update(b"lexical-hard-scan-fixed-wire-v1");
    hash.update(&bytes);
    Ok(hash.finalize().into())
}

fn chain_seed(payload: &CheckpointPayload, lane: &ScanLane) -> Result<[u8; 32], CoreError> {
    let bytes = crate::channel_payloads::encode_cbor(
        &(
            &payload.attempt,
            payload.policy_sha256,
            lane.authority,
            lane.base_authority_root_sha256,
            lane.frozen_high_water_run_id,
        ),
        "hard-scan lane seed",
    )?;
    let mut hash = Sha256::new();
    hash.update(b"lexical-hard-scan-window-seed-v1");
    hash.update(&bytes);
    Ok(hash.finalize().into())
}

fn advance_chain(prior: [u8; 32], window: &WindowWitness) -> Result<[u8; 32], CoreError> {
    let bytes = crate::channel_payloads::encode_cbor(window, "hard-scan window")?;
    let mut hash = Sha256::new();
    hash.update(b"lexical-hard-scan-window-link-v1");
    hash.update(prior);
    hash.update(&bytes);
    Ok(hash.finalize().into())
}

fn encode_body(payload: &CheckpointPayload) -> Result<Vec<u8>, CoreError> {
    payload.validate_shape()?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&FORMAT.to_be_bytes());
    let attempt = &payload.attempt;
    bytes.extend_from_slice(&attempt.root_incarnation);
    bytes.extend_from_slice(&attempt.repository_sha256);
    bytes.extend_from_slice(&attempt.revision_sha256);
    bytes.extend_from_slice(&attempt.source_event_sha256);
    bytes.extend_from_slice(&attempt.target_generation.to_be_bytes());
    match attempt.base_root_sha256 {
        Some(base) => {
            bytes.push(1);
            bytes.extend_from_slice(&base);
        }
        None => bytes.push(0),
    }
    bytes.extend_from_slice(&attempt.operations_sha256);
    bytes.extend_from_slice(&attempt.format.to_be_bytes());
    bytes.extend_from_slice(&payload.policy_sha256);
    bytes.push(
        u8::try_from(payload.lanes.len())
            .map_err(|_width| invalid("hard-scan checkpoint lane count width"))?,
    );
    for lane in &payload.lanes {
        bytes.push(match lane.authority {
            Authority::File => 1,
            Authority::Native => 2,
        });
        bytes.extend_from_slice(&lane.base_authority_root_sha256);
        bytes.extend_from_slice(&lane.frozen_high_water_run_id.to_be_bytes());
        bytes.extend_from_slice(
            &u16::try_from(lane.windows.len())
                .map_err(|_width| invalid("hard-scan checkpoint window count width"))?
                .to_be_bytes(),
        );
        for window in &lane.windows {
            bytes.extend_from_slice(&window.ordinal.to_be_bytes());
            bytes.extend_from_slice(&window.prior_chain_sha256);
            bytes.extend_from_slice(&window.start_run_id.to_be_bytes());
            bytes.extend_from_slice(&window.next_run_id.to_be_bytes());
            bytes.extend_from_slice(&window.frozen_high_water_run_id.to_be_bytes());
            bytes.push(u8::from(window.round_complete));
            bytes.push(
                u8::try_from(window.candidates.len())
                    .map_err(|_width| invalid("hard-scan candidate count width"))?,
            );
            for candidate in &window.candidates {
                bytes.extend_from_slice(&candidate.run_id.to_be_bytes());
                bytes.extend_from_slice(&candidate.index_encoded_bytes.to_be_bytes());
                bytes.extend_from_slice(&candidate.header_sha256);
                match candidate.eligibility {
                    EligibilityWitness::FileEligible {
                        effective_input_encoded_bytes,
                    } => {
                        bytes.push(1);
                        bytes.extend_from_slice(&effective_input_encoded_bytes.to_be_bytes());
                    }
                    EligibilityWitness::NativeEligible {
                        effective_input_encoded_bytes,
                        staging_bytes,
                    } => {
                        bytes.push(2);
                        bytes.extend_from_slice(&effective_input_encoded_bytes.to_be_bytes());
                        bytes.extend_from_slice(&staging_bytes.to_be_bytes());
                    }
                    EligibilityWitness::Ineligible(reason) => {
                        bytes.push(3);
                        bytes.push(match reason {
                            IneligibleReason::InputCap => 1,
                            IneligibleReason::TouchedOrDebt => 2,
                            IneligibleReason::RetiredDueOrStale => 3,
                            IneligibleReason::PartialSegment => 4,
                            IneligibleReason::StagingCap => 5,
                            IneligibleReason::EmptyCohort => 6,
                        });
                    }
                }
            }
        }
    }
    Ok(bytes)
}

pub(crate) fn encode(payload: &CheckpointPayload) -> Result<Vec<u8>, CoreError> {
    let mut bytes = encode_body(payload)?;
    bytes.extend_from_slice(&digest(payload)?);
    if bytes.len() > MAX_BYTES {
        return Err(invalid("hard-scan checkpoint encoded bound"));
    }
    Ok(bytes)
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

/// Immutable custody of one independently checked window. The attempt
/// journal authenticates this object's digest and order; the object binds
/// the complete sidecar prefix, not just its last cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
struct AnchorRecord {
    attempt: AttemptBinding,
    policy_sha256: [u8; 32],
    authority: Authority,
    base_authority_root_sha256: [u8; 32],
    frozen_high_water_run_id: u64,
    ordinal: u16,
    prior_anchor_sha256: [u8; 32],
    prior_payload_sha256: [u8; 32],
    payload_sha256: [u8; 32],
}

fn encode_anchor(anchor: &AnchorRecord) -> Result<Vec<u8>, CoreError> {
    anchor.attempt.validate()?;
    if anchor.policy_sha256 == [0; 32]
        || anchor.base_authority_root_sha256 == [0; 32]
        || anchor.frozen_high_water_run_id == 0
        || usize::from(anchor.ordinal) >= MAX_WINDOWS
        || anchor.prior_payload_sha256 == [0; 32]
        || anchor.payload_sha256 == [0; 32]
    {
        return Err(corrupt("hard-scan anchor field bound"));
    }
    let mut bytes = Vec::with_capacity(MAX_ANCHOR_PAYLOAD_BYTES);
    bytes.extend_from_slice(&ANCHOR_MAGIC);
    bytes.extend_from_slice(&ANCHOR_FORMAT.to_be_bytes());
    let attempt = &anchor.attempt;
    bytes.extend_from_slice(&attempt.root_incarnation);
    bytes.extend_from_slice(&attempt.repository_sha256);
    bytes.extend_from_slice(&attempt.revision_sha256);
    bytes.extend_from_slice(&attempt.source_event_sha256);
    bytes.extend_from_slice(&attempt.target_generation.to_be_bytes());
    if let Some(base) = attempt.base_root_sha256 {
        bytes.push(1);
        bytes.extend_from_slice(&base);
    } else {
        bytes.push(0);
        bytes.extend_from_slice(&[0; 32]);
    }
    bytes.extend_from_slice(&attempt.operations_sha256);
    bytes.extend_from_slice(&attempt.format.to_be_bytes());
    bytes.extend_from_slice(&anchor.policy_sha256);
    bytes.push(match anchor.authority {
        Authority::File => 1,
        Authority::Native => 2,
    });
    bytes.extend_from_slice(&anchor.base_authority_root_sha256);
    bytes.extend_from_slice(&anchor.frozen_high_water_run_id.to_be_bytes());
    bytes.extend_from_slice(&anchor.ordinal.to_be_bytes());
    bytes.extend_from_slice(&anchor.prior_anchor_sha256);
    bytes.extend_from_slice(&anchor.prior_payload_sha256);
    bytes.extend_from_slice(&anchor.payload_sha256);
    if bytes.len() > MAX_ANCHOR_PAYLOAD_BYTES {
        return Err(corrupt("hard-scan anchor encoded bound"));
    }
    Ok(bytes)
}

fn decode_anchor(bytes: &[u8]) -> Result<AnchorRecord, CoreError> {
    if bytes.len() > MAX_ANCHOR_PAYLOAD_BYTES {
        return Err(corrupt("hard-scan anchor encoded bound"));
    }
    let mut reader = Reader { bytes, pos: 0 };
    if reader.fixed::<8>()? != ANCHOR_MAGIC || reader.u16()? != ANCHOR_FORMAT {
        return Err(corrupt("hard-scan anchor header"));
    }
    let root_incarnation = reader.fixed()?;
    let repository_sha256 = reader.fixed()?;
    let revision_sha256 = reader.fixed()?;
    let source_event_sha256 = reader.fixed()?;
    let target_generation = reader.u64()?;
    let base_tag = reader.byte()?;
    let base_bytes = reader.fixed()?;
    let base_root_sha256 = match base_tag {
        0 if base_bytes == [0; 32] => None,
        1 => Some(base_bytes),
        _ => return Err(corrupt("hard-scan anchor base option")),
    };
    let operations_sha256 = reader.fixed()?;
    let attempt_format = reader.u16()?;
    let policy_sha256 = reader.fixed()?;
    let authority = match reader.byte()? {
        1 => Authority::File,
        2 => Authority::Native,
        _ => return Err(corrupt("hard-scan anchor authority")),
    };
    let anchor = AnchorRecord {
        attempt: AttemptBinding {
            root_incarnation,
            repository_sha256,
            revision_sha256,
            source_event_sha256,
            target_generation,
            base_root_sha256,
            operations_sha256,
            format: attempt_format,
        },
        policy_sha256,
        authority,
        base_authority_root_sha256: reader.fixed()?,
        frozen_high_water_run_id: reader.u64()?,
        ordinal: reader.u16()?,
        prior_anchor_sha256: reader.fixed()?,
        prior_payload_sha256: reader.fixed()?,
        payload_sha256: reader.fixed()?,
    };
    if reader.pos != bytes.len() || encode_anchor(&anchor)? != bytes {
        return Err(corrupt("hard-scan anchor noncanonical wire"));
    }
    Ok(anchor)
}

impl Reader<'_> {
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], CoreError> {
        let end = self
            .pos
            .checked_add(N)
            .ok_or_else(|| corrupt("hard-scan checkpoint offset overflow"))?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| corrupt("hard-scan checkpoint truncated"))?;
        let mut result = [0; N];
        result.copy_from_slice(slice);
        self.pos = end;
        Ok(result)
    }
    fn byte(&mut self) -> Result<u8, CoreError> {
        let [byte] = self.fixed::<1>()?;
        Ok(byte)
    }
    fn u16(&mut self) -> Result<u16, CoreError> {
        Ok(u16::from_be_bytes(self.fixed()?))
    }
    fn u64(&mut self) -> Result<u64, CoreError> {
        Ok(u64::from_be_bytes(self.fixed()?))
    }
}

pub(crate) fn decode(bytes: &[u8]) -> Result<CheckpointPayload, CoreError> {
    if bytes.len() > MAX_BYTES || bytes.len() < 32 {
        return Err(corrupt("hard-scan checkpoint encoded bound"));
    }
    let body_len = bytes
        .len()
        .checked_sub(32)
        .ok_or_else(|| corrupt("hard-scan checkpoint digest width"))?;
    let (body, trailer) = bytes.split_at(body_len);
    let mut hash = Sha256::new();
    hash.update(b"lexical-hard-scan-fixed-wire-v1");
    hash.update(body);
    let observed_digest: [u8; 32] = hash.finalize().into();
    if observed_digest.as_slice() != trailer {
        return Err(corrupt("hard-scan checkpoint checksum"));
    }
    // Counts are read from fixed-width fields and checked against hard limits
    // before any collection allocation; a small file cannot demand a huge Vec.
    let mut reader = Reader {
        bytes: body,
        pos: 0,
    };
    let magic = reader.fixed()?;
    let format = reader.u16()?;
    let root_incarnation = reader.fixed()?;
    let repository_sha256 = reader.fixed()?;
    let revision_sha256 = reader.fixed()?;
    let source_event_sha256 = reader.fixed()?;
    let target_generation = reader.u64()?;
    let base_root_sha256 = match reader.byte()? {
        0 => None,
        1 => Some(reader.fixed()?),
        _ => return Err(corrupt("hard-scan checkpoint base option")),
    };
    let operations_sha256 = reader.fixed()?;
    let attempt_format = reader.u16()?;
    let policy_sha256 = reader.fixed()?;
    let lane_count = usize::from(reader.byte()?);
    if lane_count > 2 {
        return Err(corrupt("hard-scan checkpoint lane count"));
    }
    let mut lanes = Vec::new();
    let mut total_candidates = 0_usize;
    for _ in 0..lane_count {
        let authority = match reader.byte()? {
            1 => Authority::File,
            2 => Authority::Native,
            _ => return Err(corrupt("hard-scan checkpoint authority tag")),
        };
        let base_authority_root_sha256 = reader.fixed()?;
        let frozen_high_water_run_id = reader.u64()?;
        let window_count = usize::from(reader.u16()?);
        if window_count > MAX_WINDOWS {
            return Err(corrupt("hard-scan checkpoint window count"));
        }
        let mut windows = Vec::new();
        let mut lane_candidates = 0_usize;
        for _ in 0..window_count {
            let ordinal = reader.u16()?;
            let prior_chain_sha256 = reader.fixed()?;
            let start_run_id = reader.u64()?;
            let next_run_id = reader.u64()?;
            let frozen_high_water_run_id = reader.u64()?;
            let round_complete = match reader.byte()? {
                0 => false,
                1 => true,
                _ => return Err(corrupt("hard-scan checkpoint completion tag")),
            };
            let candidate_count = usize::from(reader.byte()?);
            if candidate_count > WINDOW_RUNS {
                return Err(corrupt("hard-scan checkpoint candidate count"));
            }
            lane_candidates = lane_candidates
                .checked_add(candidate_count)
                .ok_or_else(|| corrupt("hard-scan checkpoint lane candidate overflow"))?;
            total_candidates = total_candidates
                .checked_add(candidate_count)
                .ok_or_else(|| corrupt("hard-scan checkpoint total candidate overflow"))?;
            if lane_candidates > MAX_RUNS || total_candidates > MAX_TOTAL_RUNS {
                return Err(corrupt("hard-scan checkpoint candidate admission bound"));
            }
            let mut candidates = Vec::new();
            for _ in 0..candidate_count {
                let run_id = reader.u64()?;
                let index_encoded_bytes = reader.u64()?;
                let header_sha256 = reader.fixed()?;
                let eligibility = match reader.byte()? {
                    1 => EligibilityWitness::FileEligible {
                        effective_input_encoded_bytes: reader.u64()?,
                    },
                    2 => EligibilityWitness::NativeEligible {
                        effective_input_encoded_bytes: reader.u64()?,
                        staging_bytes: reader.u64()?,
                    },
                    3 => EligibilityWitness::Ineligible(match reader.byte()? {
                        1 => IneligibleReason::InputCap,
                        2 => IneligibleReason::TouchedOrDebt,
                        3 => IneligibleReason::RetiredDueOrStale,
                        4 => IneligibleReason::PartialSegment,
                        5 => IneligibleReason::StagingCap,
                        6 => IneligibleReason::EmptyCohort,
                        _ => return Err(corrupt("hard-scan ineligibility reason")),
                    }),
                    _ => return Err(corrupt("hard-scan eligibility tag")),
                };
                candidates.push(CandidateWitness {
                    run_id,
                    index_encoded_bytes,
                    eligibility,
                    header_sha256,
                });
            }
            windows.push(WindowWitness {
                ordinal,
                prior_chain_sha256,
                start_run_id,
                next_run_id,
                frozen_high_water_run_id,
                round_complete,
                candidates,
            });
        }
        lanes.push(ScanLane {
            authority,
            base_authority_root_sha256,
            frozen_high_water_run_id,
            windows,
        });
    }
    if reader.pos != body.len() {
        return Err(corrupt("hard-scan checkpoint trailing bytes"));
    }
    let payload = CheckpointPayload {
        magic,
        format,
        attempt: AttemptBinding {
            root_incarnation,
            repository_sha256,
            revision_sha256,
            source_event_sha256,
            target_generation,
            base_root_sha256,
            operations_sha256,
            format: attempt_format,
        },
        policy_sha256,
        lanes,
    };
    payload.validate_shape()?;
    if encode_body(&payload)? != body {
        return Err(corrupt("hard-scan checkpoint noncanonical fixed wire"));
    }
    Ok(payload)
}

pub(super) fn sidecar_name(header_name: &str) -> String {
    format!("{header_name}.scan")
}

fn window_count(payload: &CheckpointPayload) -> Result<usize, CoreError> {
    payload.lanes.iter().try_fold(0_usize, |total, lane| {
        total
            .checked_add(lane.windows.len())
            .filter(|count| *count <= MAX_ANCHORS)
            .ok_or_else(|| corrupt("hard-scan anchor window count exceeds bound"))
    })
}

fn strip_last_window(
    payload: &CheckpointPayload,
) -> Result<(CheckpointPayload, LaneIdentity, WindowWitness), CoreError> {
    let mut prefix = payload.clone();
    let lane = prefix
        .lanes
        .last_mut()
        .ok_or_else(|| corrupt("hard-scan anchor sidecar has no lane"))?;
    let identity = (
        lane.authority,
        lane.base_authority_root_sha256,
        lane.frozen_high_water_run_id,
    );
    let tail = lane
        .windows
        .pop()
        .ok_or_else(|| corrupt("hard-scan anchor sidecar has empty last lane"))?;
    if lane.windows.is_empty() {
        let _removed = prefix.lanes.pop();
    }
    Ok((prefix, identity, tail))
}

fn verify_anchor_metadata(
    payload: &CheckpointPayload,
    anchors: &[(ObjectRef, AnchorRecord)],
) -> Result<(), CoreError> {
    if payload.lanes.iter().any(|lane| lane.windows.is_empty()) {
        return Err(corrupt("hard-scan anchor sidecar has empty lane"));
    }
    let mut windows = payload.lanes.iter().flat_map(|lane| {
        lane.windows.iter().map(move |window| {
            (
                lane.authority,
                lane.base_authority_root_sha256,
                lane.frozen_high_water_run_id,
                window.ordinal,
            )
        })
    });
    for (_reference, anchor) in anchors {
        let expected = windows
            .next()
            .ok_or_else(|| corrupt("hard-scan anchor exceeds sidecar windows"))?;
        if (
            anchor.authority,
            anchor.base_authority_root_sha256,
            anchor.frozen_high_water_run_id,
            anchor.ordinal,
        ) != expected
        {
            return Err(corrupt("hard-scan anchor window identity differs"));
        }
    }
    Ok(())
}

impl AttemptLease {
    /// Caller holds the physical attempt gate. Journal entries are read in
    /// order, and each anchor object is checked against its immutable digest.
    fn read_anchor_chain_locked(
        &self,
        policy_sha256: [u8; 32],
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Vec<(ObjectRef, AnchorRecord)>, CoreError> {
        let empty_digest =
            CheckpointPayload::empty(self.binding.clone(), policy_sha256).digest()?;
        let mut anchors = Vec::new();
        self.store.visit_attempt_log(&self.name, |reference| {
            checkpoint_budget(budget)?;
            if reference.kind != ObjectKind::HardScanAnchor {
                return Ok(());
            }
            if anchors.len() >= MAX_ANCHORS {
                return Err(corrupt("hard-scan anchor count exceeds bound"));
            }
            if reference.bytes > MAX_ANCHOR_OBJECT_BYTES {
                return Err(corrupt("hard-scan anchor object exceeds fixed bound"));
            }
            let (dependencies, bytes) = self.store.read_object(reference, budget)?;
            if !dependencies.is_empty() {
                return Err(corrupt("hard-scan anchor has dependencies"));
            }
            let anchor = decode_anchor(&bytes)?;
            let prior = anchors.last().map_or(
                empty_digest,
                |(_ref, previous): &(ObjectRef, AnchorRecord)| previous.payload_sha256,
            );
            let prior_anchor = anchors
                .last()
                .map_or([0; 32], |(previous, _)| previous.sha256);
            if anchor.attempt != self.binding
                || anchor.policy_sha256 != policy_sha256
                || anchor.prior_anchor_sha256 != prior_anchor
                || anchor.prior_payload_sha256 != prior
                || anchor.payload_sha256 == prior
            {
                return Err(corrupt("hard-scan anchor binding or chain differs"));
            }
            anchors.push((reference.clone(), anchor));
            Ok(())
        })?;
        Ok(anchors)
    }

    fn adopt_anchor_locked(&self, anchor: &AnchorRecord) -> Result<(), CoreError> {
        let bytes = encode_anchor(anchor)?;
        let (reference, newly_written) =
            self.store
                .put_object_locked_with_status(ObjectKind::HardScanAnchor, &[], &bytes)?;
        self.record_write_cost(&reference, newly_written)?;
        self.adopt_locked(&reference)
    }
}

impl ObjectStore {
    /// A visible rename is not durable when the append's parent fsync failed.
    /// Repair must cross that barrier before its journal anchor can be durable.
    fn sync_hard_scan_sidecar_parent_locked(&self) -> Result<(), CoreError> {
        #[cfg(test)]
        super::reach_atomic_cut(
            AtomicPurpose::HardScanCheckpoint,
            3,
            crate::publication_faults::Side::Before,
        )?;
        crate::causal_profile::observed_sync_legacy(
            self.sync_totals.as_deref(),
            quanta_index_core::DiagnosticSyncLabelV1::PoolControlParentFull,
            || self.attempts.sync_all(),
        )
        .map_err(|error| storage("sync repaired hard-scan checkpoint directory", error))?;
        #[cfg(test)]
        super::reach_atomic_cut(
            AtomicPurpose::HardScanCheckpoint,
            3,
            crate::publication_faults::Side::After,
        )?;
        Ok(())
    }

    /// Caller holds the lifecycle gate. Never follow a sidecar symlink or
    /// accept a changing/nonregular/overbound file.
    fn read_hard_scan_locked(&self, name: &str) -> Result<Option<CheckpointPayload>, CoreError> {
        let file = match openat(
            self.attempts.as_ref(),
            sidecar_name(name).as_str(),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(file) => File::from(file),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => return Err(storage("open hard-scan checkpoint", error)),
        };
        let metadata = file
            .metadata()
            .map_err(|error| storage("stat hard-scan checkpoint", error))?;
        let max_bytes = u64::try_from(MAX_BYTES)
            .map_err(|_width| corrupt("hard-scan checkpoint byte bound width"))?;
        if !metadata.is_file()
            || metadata.dev()
                != self
                    .attempts
                    .metadata()
                    .map_err(|error| storage("stat attempts directory", error))?
                    .dev()
            || metadata.len() > max_bytes
        {
            return Err(corrupt(
                "hard-scan checkpoint is not a bounded regular file",
            ));
        }
        let mut bytes = Vec::new();
        let _read = file
            .take(
                max_bytes
                    .checked_add(1)
                    .ok_or_else(|| corrupt("hard-scan checkpoint read bound overflow"))?,
            )
            .read_to_end(&mut bytes)
            .map_err(|error| storage("read hard-scan checkpoint", error))?;
        if u64::try_from(bytes.len())
            .map_err(|_width| corrupt("hard-scan checkpoint read width"))?
            != metadata.len()
            || bytes.len() > MAX_BYTES
        {
            return Err(corrupt("hard-scan checkpoint changed while reading"));
        }
        Ok(Some(decode(&bytes)?))
    }
}

impl AttemptLease {
    /// Resume a journal-anchored prefix without rescanning its old authority
    /// windows. A crash may leave exactly the final sidecar window ahead of
    /// its anchor; replay only that tail, then durably adopt its anchor.
    pub(crate) fn resume_hard_scan(
        &self,
        policy_sha256: [u8; 32],
        expected_roots_and_highwaters: &[(Authority, [u8; 32], u64)],
        oracle: &mut dyn IndependentWindowOracle,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Option<CheckpointPayload>, CoreError> {
        checkpoint_budget(budget)?;
        let gate = self.store.lock_gate()?;
        let record = self
            .store
            .read_attempt_header(&self.name)?
            .ok_or_else(|| corrupt("hard-scan anchor attempt disappeared"))?;
        if record.0 != self.binding {
            return Err(corrupt("hard-scan anchor attempt changed"));
        }
        self.store.recover_attempt_log(&self.name)?;
        let payload = self.store.read_hard_scan_locked(&self.name)?;
        let anchors = self.read_anchor_chain_locked(policy_sha256, budget)?;
        drop(gate);
        checkpoint_budget(budget)?;
        let Some(payload) = payload else {
            if anchors.is_empty() {
                return Ok(None);
            }
            return Err(corrupt("hard-scan anchored sidecar is missing"));
        };
        payload.validate_bound(&self.binding, policy_sha256, expected_roots_and_highwaters)?;
        let count = window_count(&payload)?;
        if count == 0 {
            return Err(corrupt("hard-scan unanchored empty sidecar"));
        }
        verify_anchor_metadata(&payload, &anchors)?;
        let anchored = anchors.len();
        if count == anchored {
            if anchors
                .last()
                .map(|(_reference, record)| record.payload_sha256)
                != Some(payload.digest()?)
            {
                return Err(corrupt("hard-scan sidecar differs from journal anchor"));
            }
            return Ok(Some(payload));
        }
        if count != anchored.saturating_add(1) {
            return Err(corrupt("hard-scan sidecar is more than one window ahead"));
        }
        let (prefix, identity, tail) = strip_last_window(&payload)?;
        let prior_digest = prefix.digest()?;
        let expected_prior = anchors.last().map_or(
            CheckpointPayload::empty(self.binding.clone(), policy_sha256).digest()?,
            |(_reference, record)| record.payload_sha256,
        );
        if prior_digest != expected_prior {
            return Err(corrupt("hard-scan unanchored tail prefix differs"));
        }
        let actual = oracle.replay(
            identity.0,
            identity.1,
            tail.start_run_id,
            identity.2,
            budget,
        )?;
        if !same_window_content(&actual, &tail) {
            return Err(corrupt(
                "hard-scan unanchored tail differs from pinned base",
            ));
        }
        checkpoint_budget(budget)?;
        let current_digest = payload.digest()?;
        let anchor = AnchorRecord {
            attempt: self.binding.clone(),
            policy_sha256,
            authority: identity.0,
            base_authority_root_sha256: identity.1,
            frozen_high_water_run_id: identity.2,
            ordinal: tail.ordinal,
            prior_anchor_sha256: anchors
                .last()
                .map_or([0; 32], |(reference, _)| reference.sha256),
            prior_payload_sha256: prior_digest,
            payload_sha256: current_digest,
        };
        let _gate = self.store.lock_gate()?;
        let record = self
            .store
            .read_attempt_header(&self.name)?
            .ok_or_else(|| corrupt("hard-scan anchor attempt disappeared during repair"))?;
        if record.0 != self.binding {
            return Err(corrupt("hard-scan anchor attempt changed during repair"));
        }
        self.store.recover_attempt_log(&self.name)?;
        let current = self
            .store
            .read_hard_scan_locked(&self.name)?
            .ok_or_else(|| corrupt("hard-scan repair sidecar disappeared"))?;
        if current.digest()? != current_digest
            || self.read_anchor_chain_locked(policy_sha256, budget)? != anchors
        {
            return Err(invalid("hard-scan repair predecessor changed concurrently"));
        }
        checkpoint_budget(budget)?;
        self.store.sync_hard_scan_sidecar_parent_locked()?;
        self.adopt_anchor_locked(&anchor)?;
        checkpoint_budget(budget)?;
        Ok(Some(payload))
    }

    /// Final seal verification replays every claimed window against the pinned
    /// old root. Preparation/resume uses journal anchors instead. The caller
    /// retains the publication lifecycle fence and old-root read pin while
    /// this full replay runs outside the short physical attempt-directory gate.
    pub(crate) fn load_hard_scan(
        &self,
        policy_sha256: [u8; 32],
        expected_roots_and_highwaters: &[(Authority, [u8; 32], u64)],
        oracle: &mut dyn IndependentWindowOracle,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Option<CheckpointPayload>, CoreError> {
        checkpoint_budget(budget)?;
        let gate = self.store.lock_gate()?;
        let record = self
            .store
            .read_attempt_header(&self.name)?
            .ok_or_else(|| corrupt("hard-scan checkpoint attempt disappeared"))?;
        if record.0 != self.binding {
            return Err(corrupt("hard-scan checkpoint attempt changed"));
        }
        checkpoint_budget(budget)?;
        let payload = self.store.read_hard_scan_locked(&self.name)?;
        drop(gate);
        checkpoint_budget(budget)?;
        if let Some(payload) = &payload {
            payload.replay_verified(
                &self.binding,
                policy_sha256,
                expected_roots_and_highwaters,
                oracle,
                budget,
            )?;
        }
        checkpoint_budget(budget)?;
        Ok(payload)
    }

    /// Append one already independently verified window. The expected prior
    /// digest fences a concurrent/stale writer; old transcript replay is done
    /// once at load, rather than on every append. The caller must retain the
    /// same old-root pin and mutation-free publication phase through this call.
    pub(crate) fn append_hard_scan_window(
        &self,
        mut expected_prior: CheckpointPayload,
        expected_prior_digest: [u8; 32],
        lane_identity: (Authority, [u8; 32], u64),
        witness: WindowWitness,
        oracle: &mut dyn IndependentWindowOracle,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(CheckpointPayload, [u8; 32]), CoreError> {
        checkpoint_budget(budget)?;
        if digest(&expected_prior)? != expected_prior_digest
            || expected_prior.attempt != self.binding
        {
            return Err(invalid("hard-scan checkpoint stale in-memory predecessor"));
        }
        let actual = oracle.replay(
            lane_identity.0,
            lane_identity.1,
            witness.start_run_id,
            lane_identity.2,
            budget,
        )?;
        if !same_window_content(&actual, &witness) {
            return Err(corrupt(
                "hard-scan checkpoint append differs from pinned base",
            ));
        }
        checkpoint_budget(budget)?;
        let _gate = self.store.lock_gate()?;
        let existing = self.store.read_hard_scan_locked(&self.name)?;
        checkpoint_budget(budget)?;
        match existing {
            Some(ref found) if digest(found)? != expected_prior_digest => {
                return Err(invalid(
                    "hard-scan checkpoint concurrent predecessor differs",
                ));
            }
            None if !expected_prior.lanes.is_empty() => {
                return Err(corrupt("hard-scan checkpoint predecessor disappeared"));
            }
            _ => {}
        }
        let record = self
            .store
            .read_attempt_header(&self.name)?
            .ok_or_else(|| corrupt("hard-scan checkpoint attempt disappeared"))?;
        if record.0 != self.binding {
            return Err(corrupt("hard-scan checkpoint attempt changed"));
        }
        self.store.recover_attempt_log(&self.name)?;
        let anchors = self.read_anchor_chain_locked(expected_prior.policy_sha256, budget)?;
        verify_anchor_metadata(&expected_prior, &anchors)?;
        if anchors.len() != window_count(&expected_prior)?
            || anchors
                .last()
                .map_or(expected_prior_digest, |(_reference, record)| {
                    record.payload_sha256
                })
                != expected_prior_digest
        {
            return Err(corrupt(
                "hard-scan checkpoint predecessor is not journal anchored",
            ));
        }
        let prior_anchor_sha256 = anchors
            .last()
            .map_or([0; 32], |(reference, _)| reference.sha256);
        let window_ordinal = witness.ordinal;
        if let Some(lane) = expected_prior
            .lanes
            .iter_mut()
            .find(|lane| lane.authority == lane_identity.0)
        {
            if (
                lane.base_authority_root_sha256,
                lane.frozen_high_water_run_id,
            ) != (lane_identity.1, lane_identity.2)
            {
                return Err(invalid("hard-scan checkpoint authority binding differs"));
            }
            lane.windows.push(witness);
        } else {
            expected_prior.lanes.push(ScanLane {
                authority: lane_identity.0,
                base_authority_root_sha256: lane_identity.1,
                frozen_high_water_run_id: lane_identity.2,
                windows: vec![witness],
            });
            expected_prior.lanes.sort_by_key(|lane| lane.authority);
        }
        let encoded = encode(&expected_prior)?;
        checkpoint_budget(budget)?;
        self.store.write_atomic_at(
            self.store.attempts.as_ref(),
            sidecar_name(&self.name).as_str(),
            &encoded,
            AtomicPurpose::HardScanCheckpoint,
        )?;
        let new_digest = digest(&expected_prior)?;
        self.adopt_anchor_locked(&AnchorRecord {
            attempt: self.binding.clone(),
            policy_sha256: expected_prior.policy_sha256,
            authority: lane_identity.0,
            base_authority_root_sha256: lane_identity.1,
            frozen_high_water_run_id: lane_identity.2,
            ordinal: window_ordinal,
            prior_anchor_sha256,
            prior_payload_sha256: expected_prior_digest,
            payload_sha256: new_digest,
        })?;
        checkpoint_budget(budget)?;
        Ok((expected_prior, new_digest))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ATTEMPTS_DIR, POOL_DIR};
    use super::*;

    const ROOT: [u8; 32] = [7; 32];
    const POLICY: [u8; 32] = [9; 32];
    const LANE_COUNT_OFFSET: usize = 229;
    const WINDOW_COUNT_OFFSET: usize = 271;

    fn only_window_mut(payload: &mut CheckpointPayload) -> &mut WindowWitness {
        payload
            .lanes
            .first_mut()
            .and_then(|lane| lane.windows.first_mut())
            .expect("fixed checkpoint window")
    }

    fn binding() -> AttemptBinding {
        AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: [2; 32],
            revision_sha256: [3; 32],
            source_event_sha256: [4; 32],
            target_generation: 19,
            base_root_sha256: Some([5; 32]),
            operations_sha256: [6; 32],
            format: 1,
        }
    }

    fn fixed_window() -> WindowWitness {
        WindowWitness {
            ordinal: 0,
            prior_chain_sha256: [0; 32],
            start_run_id: 1,
            next_run_id: 1,
            frozen_high_water_run_id: 34,
            round_complete: true,
            candidates: vec![CandidateWitness {
                run_id: 34,
                index_encoded_bytes: 41,
                eligibility: EligibilityWitness::FileEligible {
                    effective_input_encoded_bytes: 53,
                },
                header_sha256: [8; 32],
            }],
        }
    }

    fn payload() -> CheckpointPayload {
        let mut payload = CheckpointPayload {
            magic: MAGIC,
            format: FORMAT,
            attempt: binding(),
            policy_sha256: POLICY,
            lanes: vec![ScanLane {
                authority: Authority::File,
                base_authority_root_sha256: ROOT,
                frozen_high_water_run_id: 34,
                windows: vec![fixed_window()],
            }],
        };
        let seed = chain_seed(&payload, payload.lanes.first().expect("fixed lane")).unwrap();
        only_window_mut(&mut payload).prior_chain_sha256 = seed;
        payload
    }

    struct FixedOracle;
    impl IndependentWindowOracle for FixedOracle {
        fn replay(
            &mut self,
            lane: Authority,
            root: [u8; 32],
            start: u64,
            high: u64,
            _budget: Option<&RequestBudgetV1>,
        ) -> Result<WindowWitness, CoreError> {
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

            check_eq!((lane, root, start, high), (Authority::File, ROOT, 1, 34));
            // Fixed old-root oracle, independent of the saved payload bytes.
            Ok(WindowWitness {
                ordinal: 0,
                prior_chain_sha256: [0; 32],
                start_run_id: 1,
                next_run_id: 1,
                frozen_high_water_run_id: 34,
                round_complete: true,
                candidates: vec![CandidateWitness {
                    run_id: 34,
                    index_encoded_bytes: 41,
                    eligibility: EligibilityWitness::FileEligible {
                        effective_input_encoded_bytes: 53,
                    },
                    header_sha256: [8; 32],
                }],
            })
        }
    }

    #[derive(Default)]
    struct CountingOracle {
        calls: usize,
    }

    impl IndependentWindowOracle for CountingOracle {
        fn replay(
            &mut self,
            lane: Authority,
            root: [u8; 32],
            start: u64,
            high: u64,
            budget: Option<&RequestBudgetV1>,
        ) -> Result<WindowWitness, CoreError> {
            self.calls = self.calls.saturating_add(1);
            FixedOracle.replay(lane, root, start, high, budget)
        }
    }

    #[test]
    fn codec_rejects_truncation_corruption_and_overbound_input() {
        let bytes = encode(&payload()).unwrap();
        assert_eq!(decode(&bytes).unwrap(), payload());
        assert!(
            decode(
                bytes
                    .get(..bytes.len().saturating_sub(1))
                    .expect("truncated fixed wire")
            )
            .is_err()
        );
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        let mut corrupt_bytes = bytes.clone();
        *corrupt_bytes.first_mut().expect("fixed wire magic") ^= 1;
        assert!(decode(&corrupt_bytes).is_err());
        let mut wrong_checksum = bytes;
        *wrong_checksum.last_mut().unwrap() ^= 1;
        assert!(decode(&wrong_checksum).is_err());
        assert!(decode(&vec![0; MAX_BYTES.saturating_add(1)]).is_err());
    }

    #[test]
    fn rehashed_malicious_collection_lengths_are_rejected_before_allocation() {
        let mut body = encode_body(&payload()).unwrap();
        // Fixed wire layout, including the Some(base-root) option byte.
        assert_eq!(body.get(LANE_COUNT_OFFSET), Some(&1));
        *body.get_mut(LANE_COUNT_OFFSET).expect("lane count byte") = u8::MAX;
        let mut bytes = body.clone();
        let mut hash = Sha256::new();
        hash.update(b"lexical-hard-scan-fixed-wire-v1");
        hash.update(&body);
        bytes.extend_from_slice(&<[u8; 32]>::from(hash.finalize()));
        assert!(decode(&bytes).is_err());

        let mut body = encode_body(&payload()).unwrap();
        body.get_mut(WINDOW_COUNT_OFFSET..WINDOW_COUNT_OFFSET.saturating_add(2))
            .expect("window count bytes")
            .copy_from_slice(&u16::MAX.to_be_bytes());
        let mut bytes = body.clone();
        let mut hash = Sha256::new();
        hash.update(b"lexical-hard-scan-fixed-wire-v1");
        hash.update(&body);
        bytes.extend_from_slice(&<[u8; 32]>::from(hash.finalize()));
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn completed_scanner_wrap_can_follow_exactly_full_window() {
        let mut payload = CheckpointPayload::empty(binding(), POLICY);
        let mut lane = ScanLane {
            authority: Authority::File,
            base_authority_root_sha256: ROOT,
            frozen_high_water_run_id: 32,
            windows: Vec::new(),
        };
        let seed = chain_seed(&payload, &lane).unwrap();
        let first = WindowWitness {
            ordinal: 0,
            prior_chain_sha256: seed,
            start_run_id: 1,
            next_run_id: 33,
            frozen_high_water_run_id: 32,
            round_complete: false,
            candidates: (1_u64..=32)
                .map(|run_id| CandidateWitness {
                    run_id,
                    index_encoded_bytes: 41,
                    eligibility: EligibilityWitness::FileEligible {
                        effective_input_encoded_bytes: 53,
                    },
                    header_sha256: [8; 32],
                })
                .collect(),
        };
        let next_chain = advance_chain(seed, &first).unwrap();
        lane.windows.push(first);
        lane.windows.push(WindowWitness {
            ordinal: 1,
            prior_chain_sha256: next_chain,
            start_run_id: 33,
            next_run_id: 1,
            frozen_high_water_run_id: 32,
            round_complete: true,
            candidates: Vec::new(),
        });
        payload.lanes.push(lane);
        assert_eq!(decode(&encode(&payload).unwrap()).unwrap(), payload);
        payload
            .require_complete(&[(Authority::File, ROOT, 32)])
            .unwrap();
    }

    #[test]
    fn two_full_authority_frontiers_fit_the_same_one_mebibyte_sidecar() {
        let mut payload = CheckpointPayload::empty(binding(), POLICY);
        for authority in [Authority::File, Authority::Native] {
            let mut lane = ScanLane {
                authority,
                base_authority_root_sha256: ROOT,
                frozen_high_water_run_id: 4096,
                windows: Vec::new(),
            };
            let mut chain = chain_seed(&payload, &lane).unwrap();
            for ordinal in 0_u16..128 {
                let start = u64::from(ordinal)
                    .checked_mul(32)
                    .and_then(|id| id.checked_add(1))
                    .unwrap();
                let next = start.checked_add(32).unwrap();
                let candidates = (start..next)
                    .map(|run_id| CandidateWitness {
                        run_id,
                        index_encoded_bytes: 41,
                        eligibility: match authority {
                            Authority::File => EligibilityWitness::FileEligible {
                                effective_input_encoded_bytes: 53,
                            },
                            Authority::Native => EligibilityWitness::NativeEligible {
                                effective_input_encoded_bytes: 41,
                                staging_bytes: 97,
                            },
                        },
                        header_sha256: [8; 32],
                    })
                    .collect();
                let window = WindowWitness {
                    ordinal,
                    prior_chain_sha256: chain,
                    start_run_id: start,
                    next_run_id: next,
                    frozen_high_water_run_id: 4096,
                    round_complete: false,
                    candidates,
                };
                chain = advance_chain(chain, &window).unwrap();
                lane.windows.push(window);
            }
            lane.windows.push(WindowWitness {
                ordinal: 128,
                prior_chain_sha256: chain,
                start_run_id: 4097,
                next_run_id: 1,
                frozen_high_water_run_id: 4096,
                round_complete: true,
                candidates: Vec::new(),
            });
            payload.lanes.push(lane);
        }
        let encoded = encode(&payload).unwrap();
        assert!(encoded.len() < MAX_BYTES);
        assert_eq!(decode(&encoded).unwrap(), payload);
    }

    #[test]
    fn exact_attempt_policy_base_and_highwater_are_bound() {
        let saved = payload();
        let expected = [(Authority::File, ROOT, 34)];
        saved
            .replay_verified(&binding(), POLICY, &expected, &mut FixedOracle, None)
            .unwrap();
        let mut changed = binding();
        changed.operations_sha256 = [13; 32];
        assert!(
            saved
                .replay_verified(&changed, POLICY, &expected, &mut FixedOracle, None)
                .is_err()
        );
        changed = binding();
        changed.source_event_sha256 = [14; 32];
        assert!(
            saved
                .replay_verified(&changed, POLICY, &expected, &mut FixedOracle, None)
                .is_err()
        );
        assert!(
            saved
                .replay_verified(&binding(), [10; 32], &expected, &mut FixedOracle, None)
                .is_err()
        );
        assert!(
            saved
                .replay_verified(
                    &binding(),
                    POLICY,
                    &[(Authority::File, [11; 32], 34)],
                    &mut FixedOracle,
                    None,
                )
                .is_err()
        );
        assert!(
            saved
                .replay_verified(
                    &binding(),
                    POLICY,
                    &[(Authority::File, ROOT, 35)],
                    &mut FixedOracle,
                    None,
                )
                .is_err()
        );
    }

    #[test]
    fn forged_skip_with_recomputed_checksum_fails_independent_replay() {
        let mut forged = payload();
        // A writer can recompute the unkeyed checksum. It still cannot make
        // the pinned old root's first window start at run 2.
        only_window_mut(&mut forged).start_run_id = 2;
        assert!(forged.validate_shape().is_err());
        let mut forged = payload();
        only_window_mut(&mut forged).candidates.clear();
        let forged_bytes = encode(&forged).unwrap();
        let decoded = decode(&forged_bytes).unwrap();
        assert!(
            decoded
                .replay_verified(
                    &binding(),
                    POLICY,
                    &[(Authority::File, ROOT, 34)],
                    &mut FixedOracle,
                    None,
                )
                .is_err()
        );
        let mut partial = payload();
        only_window_mut(&mut partial).round_complete = false;
        assert!(
            partial
                .require_complete(&[(Authority::File, ROOT, 34)])
                .is_err()
        );
    }

    #[test]
    fn sidecar_reopens_with_same_attempt_and_retires_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let lease = store.begin_attempt(&binding(), &[]).unwrap();
        let prior = CheckpointPayload::empty(binding(), POLICY);
        let witness = prior
            .decorate_next_window((Authority::File, ROOT, 34), fixed_window())
            .unwrap();
        let (written, _) = lease
            .append_hard_scan_window(
                prior.clone(),
                prior.digest().unwrap(),
                (Authority::File, ROOT, 34),
                witness,
                &mut FixedOracle,
                None,
            )
            .unwrap();
        let saved_window = written
            .lanes
            .first()
            .and_then(|lane| lane.windows.first())
            .cloned()
            .expect("saved first window");
        assert!(
            lease
                .append_hard_scan_window(
                    prior.clone(),
                    prior.digest().unwrap(),
                    (Authority::File, ROOT, 34),
                    saved_window,
                    &mut FixedOracle,
                    None,
                )
                .is_err()
        );
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).unwrap();
        let lease = reopened.begin_attempt(&binding(), &[]).unwrap();
        let loaded = lease
            .load_hard_scan(
                POLICY,
                &[(Authority::File, ROOT, 34)],
                &mut FixedOracle,
                None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(loaded, written);
        assert!(
            lease
                .load_hard_scan(
                    [10; 32],
                    &[(Authority::File, ROOT, 34)],
                    &mut FixedOracle,
                    None
                )
                .is_err()
        );
        let path = dir
            .path()
            .join(POOL_DIR)
            .join(ATTEMPTS_DIR)
            .join(sidecar_name(&binding().identity_name()));
        assert!(path.is_file());
        lease.discard_unsealed().unwrap();
        assert!(!path.exists());
        let after = ObjectStore::open(dir.path()).unwrap();
        assert!(after.adopted_roots().unwrap().is_empty());
    }

    #[test]
    fn journal_anchor_resumes_without_replaying_committed_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let lease = store.begin_attempt(&binding(), &[]).unwrap();
        let prior = CheckpointPayload::empty(binding(), POLICY);
        let witness = prior
            .decorate_next_window((Authority::File, ROOT, 34), fixed_window())
            .unwrap();
        let (written, _digest) = lease
            .append_hard_scan_window(
                prior.clone(),
                prior.digest().unwrap(),
                (Authority::File, ROOT, 34),
                witness,
                &mut FixedOracle,
                None,
            )
            .unwrap();
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).unwrap();
        let lease = reopened.begin_attempt(&binding(), &[]).unwrap();
        let mut oracle = CountingOracle::default();
        assert_eq!(
            lease
                .resume_hard_scan(POLICY, &[(Authority::File, ROOT, 34)], &mut oracle, None)
                .unwrap(),
            Some(written)
        );
        assert_eq!(oracle.calls, 0);
        assert!(
            lease
                .resume_hard_scan([10; 32], &[(Authority::File, ROOT, 34)], &mut oracle, None)
                .is_err()
        );
        assert!(
            lease
                .resume_hard_scan(
                    POLICY,
                    &[(Authority::File, [11; 32], 34)],
                    &mut oracle,
                    None
                )
                .is_err()
        );
        lease.discard_unsealed().unwrap();
    }

    #[test]
    fn recomputed_sidecar_checksum_cannot_override_journal_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let lease = store.begin_attempt(&binding(), &[]).unwrap();
        let prior = CheckpointPayload::empty(binding(), POLICY);
        let witness = prior
            .decorate_next_window((Authority::File, ROOT, 34), fixed_window())
            .unwrap();
        let (mut written, _digest) = lease
            .append_hard_scan_window(
                prior.clone(),
                prior.digest().unwrap(),
                (Authority::File, ROOT, 34),
                witness,
                &mut FixedOracle,
                None,
            )
            .unwrap();
        only_window_mut(&mut written).candidates.clear();
        let forged = encode(&written).unwrap();
        let path = dir
            .path()
            .join(POOL_DIR)
            .join(ATTEMPTS_DIR)
            .join(sidecar_name(&binding().identity_name()));
        std::fs::write(&path, forged).unwrap();
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).unwrap();
        let lease = reopened.begin_attempt(&binding(), &[]).unwrap();
        let mut oracle = CountingOracle::default();
        assert!(
            lease
                .resume_hard_scan(POLICY, &[(Authority::File, ROOT, 34)], &mut oracle, None)
                .is_err()
        );
        assert_eq!(oracle.calls, 0);
        lease.discard_unsealed().unwrap();
    }

    #[test]
    fn unanchored_crash_tail_replays_once_then_becomes_zero_replay_prefix() {
        use crate::publication_faults::{Cut, Side, with_error_at};
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let lease = store.begin_attempt(&binding(), &[]).unwrap();
        let prior = CheckpointPayload::empty(binding(), POLICY);
        let witness = prior
            .decorate_next_window((Authority::File, ROOT, 34), fixed_window())
            .unwrap();
        let (result, fired) =
            with_error_at(Cut::HardScanCheckpointDirectorySync, Side::After, 1, || {
                lease.append_hard_scan_window(
                    prior.clone(),
                    prior.digest().unwrap(),
                    (Authority::File, ROOT, 34),
                    witness,
                    &mut FixedOracle,
                    None,
                )
            });
        assert!(fired);
        assert!(result.is_err());
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).unwrap();
        let lease = reopened.begin_attempt(&binding(), &[]).unwrap();
        let mut oracle = CountingOracle::default();
        let loaded = lease
            .resume_hard_scan(POLICY, &[(Authority::File, ROOT, 34)], &mut oracle, None)
            .unwrap()
            .expect("durable tail");
        assert_eq!(oracle.calls, 1);
        loaded
            .require_complete(&[(Authority::File, ROOT, 34)])
            .unwrap();
        drop(lease);
        let third = ObjectStore::open(dir.path()).unwrap();
        let lease = third.begin_attempt(&binding(), &[]).unwrap();
        let mut oracle = CountingOracle::default();
        assert_eq!(
            lease
                .resume_hard_scan(POLICY, &[(Authority::File, ROOT, 34)], &mut oracle, None)
                .unwrap(),
            Some(loaded)
        );
        assert_eq!(oracle.calls, 0);
        lease.discard_unsealed().unwrap();
    }

    #[test]
    fn repair_syncs_visible_unconfirmed_sidecar_before_adopting_anchor() {
        use crate::publication_faults::{Cut, Side, with_error_at};

        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let lease = store.begin_attempt(&binding(), &[]).unwrap();
        let prior = CheckpointPayload::empty(binding(), POLICY);
        let witness = prior
            .decorate_next_window((Authority::File, ROOT, 34), fixed_window())
            .unwrap();
        let (append, fired) = with_error_at(
            Cut::HardScanCheckpointDirectorySync,
            Side::Before,
            1,
            || {
                lease.append_hard_scan_window(
                    prior.clone(),
                    prior.digest().unwrap(),
                    (Authority::File, ROOT, 34),
                    witness,
                    &mut FixedOracle,
                    None,
                )
            },
        );
        assert!(fired);
        assert!(append.is_err());
        assert!(
            store
                .adopted_roots()
                .unwrap()
                .iter()
                .all(|reference| reference.kind != ObjectKind::HardScanAnchor)
        );

        drop(lease);
        let reopened = ObjectStore::open(dir.path()).unwrap();
        let lease = reopened.begin_attempt(&binding(), &[]).unwrap();
        let mut oracle = CountingOracle::default();
        let (repair, fired) = with_error_at(
            Cut::HardScanCheckpointDirectorySync,
            Side::Before,
            1,
            || lease.resume_hard_scan(POLICY, &[(Authority::File, ROOT, 34)], &mut oracle, None),
        );
        assert!(fired, "repair must retry the checkpoint parent fsync");
        assert!(repair.is_err());
        assert_eq!(oracle.calls, 1);
        assert!(
            reopened
                .adopted_roots()
                .unwrap()
                .iter()
                .all(|reference| reference.kind != ObjectKind::HardScanAnchor)
        );

        let repaired = lease
            .resume_hard_scan(POLICY, &[(Authority::File, ROOT, 34)], &mut oracle, None)
            .unwrap()
            .expect("visible unanchored tail");
        assert_eq!(oracle.calls, 2);
        assert_eq!(
            reopened
                .adopted_roots()
                .unwrap()
                .iter()
                .filter(|reference| reference.kind == ObjectKind::HardScanAnchor)
                .count(),
            1
        );
        drop(lease);
        let third = ObjectStore::open(dir.path()).unwrap();
        let lease = third.begin_attempt(&binding(), &[]).unwrap();
        let mut oracle = CountingOracle::default();
        assert_eq!(
            lease
                .resume_hard_scan(POLICY, &[(Authority::File, ROOT, 34)], &mut oracle, None)
                .unwrap(),
            Some(repaired)
        );
        assert_eq!(oracle.calls, 0);
        lease.discard_unsealed().unwrap();
    }

    #[test]
    fn pending_anchor_journal_fault_recovers_without_old_window_io() {
        use crate::publication_faults::{Cut, Side, with_error_at};
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let lease = store.begin_attempt(&binding(), &[]).unwrap();
        let prior = CheckpointPayload::empty(binding(), POLICY);
        let witness = prior
            .decorate_next_window((Authority::File, ROOT, 34), fixed_window())
            .unwrap();
        let (result, fired) =
            with_error_at(Cut::AttemptJournalAppendWrite, Side::Before, 1, || {
                lease.append_hard_scan_window(
                    prior.clone(),
                    prior.digest().unwrap(),
                    (Authority::File, ROOT, 34),
                    witness,
                    &mut FixedOracle,
                    None,
                )
            });
        assert!(fired);
        assert!(result.is_err());
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).unwrap();
        let lease = reopened.begin_attempt(&binding(), &[]).unwrap();
        let mut oracle = CountingOracle::default();
        let loaded = lease
            .resume_hard_scan(POLICY, &[(Authority::File, ROOT, 34)], &mut oracle, None)
            .unwrap()
            .expect("journal-recovered anchor");
        assert_eq!(oracle.calls, 0);
        loaded
            .require_complete(&[(Authority::File, ROOT, 34)])
            .unwrap();
        lease.discard_unsealed().unwrap();
    }

    #[test]
    fn sidecar_nofollow_refuses_symlink_without_touching_target() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let lease = store.begin_attempt(&binding(), &[]).unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"unchanged").unwrap();
        let path = dir
            .path()
            .join(POOL_DIR)
            .join(ATTEMPTS_DIR)
            .join(sidecar_name(&binding().identity_name()));
        std::os::unix::fs::symlink(&victim, &path).unwrap();
        assert!(
            lease
                .load_hard_scan(
                    POLICY,
                    &[(Authority::File, ROOT, 34)],
                    &mut FixedOracle,
                    None
                )
                .is_err()
        );
        assert_eq!(std::fs::read(&victim).unwrap(), b"unchanged".to_vec());
        lease.discard_unsealed().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn atomic_checkpoint_faults_preserve_old_or_new_valid_prefix() {
        use crate::publication_faults::{Cut, Side, with_error_at};
        for (cut, side, expected_written) in [
            (Cut::HardScanCheckpointRename, Side::Before, false),
            (Cut::HardScanCheckpointDirectorySync, Side::After, true),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = ObjectStore::open(dir.path()).unwrap();
            let lease = store.begin_attempt(&binding(), &[]).unwrap();
            let prior = CheckpointPayload::empty(binding(), POLICY);
            let witness = prior
                .decorate_next_window((Authority::File, ROOT, 34), fixed_window())
                .unwrap();
            let (result, fired) = with_error_at(cut, side, 1, || {
                lease.append_hard_scan_window(
                    prior.clone(),
                    prior.digest().unwrap(),
                    (Authority::File, ROOT, 34),
                    witness,
                    &mut FixedOracle,
                    None,
                )
            });
            assert!(fired);
            assert!(result.is_err());
            drop(lease);
            let reopened = ObjectStore::open(dir.path()).unwrap();
            let lease = reopened.begin_attempt(&binding(), &[]).unwrap();
            let observed = lease
                .load_hard_scan(
                    POLICY,
                    &[(Authority::File, ROOT, 34)],
                    &mut FixedOracle,
                    None,
                )
                .unwrap();
            assert_eq!(observed.is_some(), expected_written);
            lease.discard_unsealed().unwrap();
            assert!(reopened.adopted_roots().unwrap().is_empty());
        }
    }

    #[test]
    fn orphan_sidecar_is_removed_by_bounded_startup_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let lease = store.begin_attempt(&binding(), &[]).unwrap();
        let prior = CheckpointPayload::empty(binding(), POLICY);
        let witness = prior
            .decorate_next_window((Authority::File, ROOT, 34), fixed_window())
            .unwrap();
        let _saved = lease
            .append_hard_scan_window(
                prior.clone(),
                prior.digest().unwrap(),
                (Authority::File, ROOT, 34),
                witness,
                &mut FixedOracle,
                None,
            )
            .unwrap();
        let attempts = dir.path().join(POOL_DIR).join(ATTEMPTS_DIR);
        let sidecar = attempts.join(sidecar_name(&binding().identity_name()));
        drop(lease);
        std::fs::remove_file(attempts.join(binding().identity_name())).unwrap();
        let reopened = ObjectStore::open(dir.path()).unwrap();
        assert!(reopened.adopted_roots().unwrap().is_empty());
        assert!(!sidecar.exists());
    }
}
