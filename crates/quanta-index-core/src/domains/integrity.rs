//! Deep integrity of sealed generations, off the serving path (QI-BB-017).
//!
//! A door — activation, restart, a cold open — proves a sealed generation
//! by its layout: every committed file exists at its committed length, and
//! the two decoded sidecars hash to their commitments. Byte integrity of the
//! dataset itself is proven here instead, by a scrub the composition root
//! runs as quota'd maintenance: bounded bytes per step, a cursor to resume
//! from, and a typed outcome. A generation the scrub finds corrupt is
//! quarantined through the QI-BB-026 surface, durably, so the boot
//! inventory keeps it out of readiness and every door refuses it typed. A
//! defect a door proves while activation or rollback picks a generation is
//! recorded the same way, by the owning adapter re-proving it.

use quanta_index_contract::GenerationSnapshot;

use crate::CoreError;

use super::generation::QuarantinedGenerationV1;

/// Where a paused scrub resumes: the index of the next committed artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntegrityScrubCursorV1 {
    pub next_artifact: u64,
}

/// The most bytes one scrub step may read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntegrityScrubBudgetV1 {
    pub max_bytes: u64,
}

/// How one scrub step ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntegrityScrubOutcomeV1 {
    /// Every committed file was hashed and matched the seal; the adapter
    /// recorded the completion beside the generation.
    Completed,
    /// The byte budget ran out; the next step resumes from `cursor`.
    Paused { cursor: IntegrityScrubCursorV1 },
    /// A committed file does not match the seal; the adapter quarantined
    /// the generation and this is the entry the inventory now lists.
    Corrupt {
        quarantined: QuarantinedGenerationV1,
    },
}

/// What one scrub step did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrityScrubReportV1 {
    pub generation: GenerationSnapshot,
    /// Committed files hashed and matched in this step.
    pub files_verified: u64,
    /// Bytes read and hashed in this step.
    pub bytes_read: u64,
    pub outcome: IntegrityScrubOutcomeV1,
}

/// One sealed generation the scrub may take on, with when it was last
/// scrubbed to completion, if ever.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrityScrubCandidateV1 {
    pub identity: GenerationSnapshot,
    /// Unix seconds of the last completed scrub the adapter recorded.
    pub last_completed_unix: Option<u64>,
}

/// Wire code for a door that meets a generation the scrub quarantined.
pub const GENERATION_QUARANTINED_CODE: &str = "GENERATION_QUARANTINED";

/// Wire code for a door that proved a sealed generation is not what its
/// seal committed to.
///
/// A committed file missing, resized or rewritten, a sidecar that does not
/// hash to its commitment, a file the seal never listed: a verdict about
/// content, never an I/O failure, answered by every adapter's door under
/// this one code.
pub const GENERATION_SIDECAR_CORRUPT_CODE: &str = "GENERATION_SIDECAR_CORRUPT";

/// How the composition root paces the scrub as quota'd maintenance: one
/// step every `interval_millis`, each reading at most `max_bytes_per_step`
/// (plus the one file that crosses the budget).
///
/// The default reads 64 MiB every five seconds: about 13 MB/s of
/// background I/O, which walks a ten-million-row generation of 1536-wide
/// vectors (roughly 61 GB) in about eighty minutes without contending
/// with serving.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntegrityScrubPolicyV1 {
    pub interval_millis: u64,
    pub max_bytes_per_step: u64,
}

impl IntegrityScrubPolicyV1 {
    pub const DEFAULT: Self = Self {
        interval_millis: 5_000,
        max_bytes_per_step: 64 * 1024 * 1024,
    };

    /// A policy with both knobs positive; zero would either spin or never
    /// read a byte, neither of which is a scrub.
    pub fn new(interval_millis: u64, max_bytes_per_step: u64) -> Result<Self, CoreError> {
        if interval_millis == 0 || max_bytes_per_step == 0 {
            return Err(CoreError::InvalidContract(format!(
                "integrity scrub policy: interval_millis ({interval_millis}) and max_bytes_per_step ({max_bytes_per_step}) must both be positive"
            )));
        }
        Ok(Self {
            interval_millis,
            max_bytes_per_step,
        })
    }

    #[must_use]
    pub const fn budget(self) -> IntegrityScrubBudgetV1 {
        IntegrityScrubBudgetV1 {
            max_bytes: self.max_bytes_per_step,
        }
    }
}

/// What re-proving a door's finding did (QI-BB-026).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DoorFindingOutcome {
    /// The generation carries a content-corrupt quarantine receipt — the
    /// re-proof wrote it, or the scrub had — and this is the entry the
    /// inventory lists.
    Quarantined {
        quarantined: QuarantinedGenerationV1,
    },
    /// The re-proof admitted the generation: the finding did not reproduce
    /// and nothing was recorded.
    NotReproduced,
}

/// Records a content defect a door proved on a sealed generation
/// (QI-BB-026).
///
/// A door answers [`GENERATION_SIDECAR_CORRUPT_CODE`] and leaves the disk
/// as it found it: a read never writes. The gates that pick a generation
/// that is not the serve head — activation and rollback — hand the finding
/// here, and the owning adapter proves it again under its directory
/// lifecycle lock, so a scrub or a discard cannot interleave. The caller is
/// never trusted: the receipt is written only when the adapter's own door
/// proof fails with [`GENERATION_SIDECAR_CORRUPT_CODE`]; any other failure
/// (the directory gone, an identity that no longer names it, an I/O error)
/// is returned and records nothing. The receipt is the scrub's, so the
/// inventory lists the generation as
/// [`GenerationQuarantineReasonV1::ContentCorrupt`](super::generation::GenerationQuarantineReasonV1::ContentCorrupt),
/// every door refuses it [`GENERATION_QUARANTINED_CODE`], and no later
/// activation or rollback picks it until it is discarded or rebuilt.
pub trait DoorFindingQuarantinePort: Send + Sync {
    fn quarantine_door_finding(
        &self,
        generation: &GenerationSnapshot,
    ) -> Result<DoorFindingOutcome, CoreError>;
}

/// Deep, bounded, resumable integrity verification of one adapter's sealed
/// generations (QI-BB-017).
///
/// Implementations hash committed files against the seal, never decode a
/// row, read at most `budget.max_bytes` per step (plus the one file that
/// crosses it), and on a mismatch write a durable quarantine receipt that
/// their own inventory reports as
/// [`GenerationQuarantineReasonV1::ContentCorrupt`](super::generation::GenerationQuarantineReasonV1::ContentCorrupt)
/// and their own doors refuse under [`GENERATION_QUARANTINED_CODE`]. A
/// completed scrub is recorded beside the generation so the next
/// [`Self::scrub_candidates`] can order by staleness.
pub trait IntegrityScrubPort: Send + Sync {
    /// Every sealed generation this adapter can scrub right now, cheaply:
    /// identities and receipts only, no content.
    fn scrub_candidates(&self) -> Result<Vec<IntegrityScrubCandidateV1>, CoreError>;

    /// Run one bounded step over `generation`, from `cursor` or from the
    /// start.
    fn scrub(
        &self,
        generation: &GenerationSnapshot,
        cursor: Option<IntegrityScrubCursorV1>,
        budget: IntegrityScrubBudgetV1,
    ) -> Result<IntegrityScrubReportV1, CoreError>;
}
