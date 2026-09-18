//! Deep integrity verification of sealed generations (QI-BB-017 보완 #3,
//! QI-BB-030).
//!
//! A door that admits a sealed generation — the activation validator, the
//! cold open — proves every byte a query decodes as it reads it, and proves
//! only the presence and length of what a query maps without decoding (the
//! index segment files). Their content is deep-verified when the seal
//! measures them and again whenever a scrub runs; between seals the scrub
//! is the only authority for "the bytes on disk are still the sealed
//! bytes". The scrub records its pass beside the generation, so a reader
//! can tell "deep-verified at seal, never scrubbed" from "scrub-verified
//! since `stamp`".

use quanta_index_contract::GenerationSnapshot;

use crate::CoreError;

/// When a scrub ran, as the caller's clock says it (Unix milliseconds).
///
/// The port takes the stamp from its caller instead of reading a clock so
/// the record is attributable and a test can pin it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntegrityScrubStampV1 {
    pub unix_ms: u64,
}

/// What one scrub re-measured.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntegrityScrubReportV1 {
    /// Committed files whose length and digest were re-measured.
    pub files_verified: u64,
    /// Bytes read to do so.
    pub bytes_verified: u64,
    /// When the pass ran.
    pub stamp: IntegrityScrubStampV1,
}

/// How deeply a sealed generation's content has been proved since its seal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrityScrubStatusV1 {
    /// The seal measured every committed byte; no scrub has run since.
    DeepVerifiedAtSeal,
    /// A scrub re-measured every committed byte, most recently as reported.
    ScrubVerifiedSince(IntegrityScrubReportV1),
}

/// Re-measure every byte a sealed generation commits to, and record it.
///
/// A scrub is strictly stronger than a door: it proves everything a door
/// proves and hashes what a door only measures by length. Any mismatch is
/// the same typed refusal a door would give, never a partial pass; a
/// generation that is not sealed, or whose identity is not `candidate`, is
/// refused typed as well.
pub trait IntegrityScrubPort: Send + Sync {
    fn scrub_sealed_generation(
        &self,
        candidate: &GenerationSnapshot,
        stamp: IntegrityScrubStampV1,
    ) -> Result<IntegrityScrubReportV1, CoreError>;

    /// The most recent scrub's record for `candidate`, or that none ran.
    fn integrity_scrub_status(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<IntegrityScrubStatusV1, CoreError>;
}
