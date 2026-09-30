//! The integrity scrub over sealed semantic generations, and the receipts
//! it leaves behind (QI-BB-017).
//!
//! The door proof is cheap by design (see [`crate::sealed_manifest`]); the
//! bytes of a sealed dataset are proven here, off the serving path, by
//! bounded, resumable steps the composition root schedules as quota'd
//! maintenance. Two durable receipts live beside a generation:
//!
//! - `semantic-scrub-receipt.cbor` — written when a pass over every
//!   committed file completed and matched; the inventory reports its epoch
//!   so the scheduler can order generations by staleness.
//! - `semantic-quarantine.cbor` — written when a committed file did not
//!   match the seal. While it exists the boot inventory quarantines the
//!   generation under `GENERATION_QUARANTINE_CONTENT_CORRUPT` and every
//!   door refuses it typed as `GENERATION_QUARANTINED`; the QI-BB-026
//!   discard surface is the one way it leaves the disk.
//!
//! Neither receipt is part of the sealed commitment (which names the
//! dataset tree and the two decoded sidecars only), so writing one never
//! makes a sealed generation look tampered with.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest as _, Sha256};

use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind};
use quanta_index_core::{
    CoreError, DoorFindingOutcome, DoorFindingQuarantinePort, GENERATION_SIDECAR_CORRUPT_CODE,
    GenerationQuarantineReasonV1, IntegrityScrubBudgetV1, IntegrityScrubCandidateV1,
    IntegrityScrubCursorV1, IntegrityScrubOutcomeV1, IntegrityScrubPort, IntegrityScrubReportV1,
    MetricPointV1, QuarantinedGenerationV1,
};

use crate::SemanticAdapter;
use crate::codec::decode_current_format;
use crate::codec::{self, cbor_serde};
use crate::durable_write::write_atomic;
use crate::layout;
use crate::sealed_manifest::{
    SealedManifestScrubV1, SealedManifestScrubVerdictV1, scrub_sealed_manifest,
    verify_sealed_manifest,
};

const SCRUB_RECEIPT_FORMAT_VERSION: u32 = 1;
const QUARANTINE_RECEIPT_FORMAT_VERSION: u32 = 1;
const MAX_QUARANTINE_DETAIL_BYTES: usize = 4 * 1024;

fn bounded_quarantine_detail(detail: &str) -> String {
    if detail.len() <= MAX_QUARANTINE_DETAIL_BYTES {
        return detail.to_string();
    }
    let suffix = format!(
        "... [truncated; original_sha256={:x}]",
        Sha256::digest(detail.as_bytes())
    );
    let prefix_limit = MAX_QUARANTINE_DETAIL_BYTES.saturating_sub(suffix.len());
    let prefix: String = detail
        .char_indices()
        .take_while(|(start, ch)| start.saturating_add(ch.len_utf8()) <= prefix_limit)
        .map(|(_, ch)| ch)
        .collect();
    format!("{prefix}{suffix}")
}

/// The receipt of a completed scrub pass: when it completed and what it
/// covered, as the seal committed it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ScrubReceiptV1 {
    pub(crate) format_version: u32,
    pub(crate) completed_unix: u64,
    pub(crate) files_verified: u64,
    pub(crate) bytes_verified: u64,
}

cbor_serde!(ScrubReceiptV1 {
    format_version: u32,
    completed_unix: u64,
    files_verified: u64,
    bytes_verified: u64,
});

/// The receipt of a corruption the scrub proved: the quarantine reason's
/// code, what did not match, and when.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuarantineReceiptV1 {
    pub(crate) format_version: u32,
    pub(crate) reason: String,
    pub(crate) detail: String,
    pub(crate) detected_unix: u64,
}

cbor_serde!(QuarantineReceiptV1 {
    format_version: u32,
    reason: String,
    detail: String,
    detected_unix: u64,
});

fn now_unix() -> Result<u64, CoreError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|err| {
            CoreError::Storage(format!("semantic: system clock before unix epoch: {err}"))
        })
}

/// Read a receipt beside a generation, `None` when there is none.
fn read_receipt<T: serde::de::DeserializeOwned>(
    path: &Path,
    what: &str,
    format_version: u32,
) -> Result<Option<T>, CoreError> {
    let bytes =
        match crate::control_file::read_bounded(path, crate::control_file::MAX_RECEIPT_BYTES) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "semantic: read {what} {}: {error}",
                    path.display()
                )));
            }
        };
    decode_current_format(&bytes, what, format_version).map(Some)
}

/// The scrub receipt beside `generation_dir`, if a pass ever completed.
pub(crate) fn read_scrub_receipt(
    generation_dir: &Path,
) -> Result<Option<ScrubReceiptV1>, CoreError> {
    let path = layout::scrub_receipt_path(generation_dir);
    let bytes = match crate::control_file::read_bounded(
        &path,
        crate::control_file::MAX_RECEIPT_BYTES,
    ) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid,
                message: format!(
                    "semantic: scrub receipt {} is invalid: {error}",
                    path.display()
                ),
            });
        }
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "semantic: read scrub receipt {}: {error}",
                path.display()
            )));
        }
    };
    decode_current_format(&bytes, "scrub receipt", SCRUB_RECEIPT_FORMAT_VERSION)
        .map(Some)
        .map_err(|error| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid,
            message: format!(
                "semantic: scrub receipt {} is invalid: {error}",
                path.display()
            ),
        })
}

/// The quarantine receipt beside `generation_dir`, if the scrub ever
/// proved it corrupt.
pub(crate) fn read_quarantine_receipt(
    generation_dir: &Path,
) -> Result<Option<QuarantineReceiptV1>, CoreError> {
    let receipt: Option<QuarantineReceiptV1> = read_receipt(
        &layout::quarantine_receipt_path(generation_dir),
        "quarantine receipt",
        QUARANTINE_RECEIPT_FORMAT_VERSION,
    )?;
    if let Some(receipt) = receipt.as_ref()
        && GenerationQuarantineReasonV1::from_code_str(&receipt.reason).is_none()
    {
        return Err(CoreError::Storage(format!(
            "semantic: quarantine receipt under {} names an unknown reason `{}`",
            generation_dir.display(),
            receipt.reason
        )));
    }
    Ok(receipt)
}

/// Refuse a generation the scrub quarantined, typed, naming the receipt.
pub(crate) fn refuse_if_quarantined(generation_dir: &Path) -> Result<(), CoreError> {
    let Some(receipt) = read_quarantine_receipt(generation_dir)? else {
        return Ok(());
    };
    Err(CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationQuarantined,
        message: format!(
            "semantic: generation {} is quarantined as {} since unix {}: {}; discard it through the quarantine surface or rebuild it from its producer",
            generation_dir.display(),
            receipt.reason,
            receipt.detected_unix,
            receipt.detail
        ),
    })
}

/// Write the quarantine receipt the inventory and every door will read.
///
/// The scrub writes it, and so does the re-proof of a door's finding
/// (QI-BB-017, QI-BB-026).
fn quarantine(generation_dir: &Path, detail: &str) -> Result<QuarantinedGenerationV1, CoreError> {
    let detail = bounded_quarantine_detail(detail);
    let receipt = QuarantineReceiptV1 {
        format_version: QUARANTINE_RECEIPT_FORMAT_VERSION,
        reason: GenerationQuarantineReasonV1::ContentCorrupt
            .as_code_str()
            .to_string(),
        detail: detail.clone(),
        detected_unix: now_unix()?,
    };
    let bytes = codec::encode(&receipt, "quarantine receipt")?;
    crate::control_file::ensure_bounded(
        &bytes,
        crate::control_file::MAX_RECEIPT_BYTES,
        "quarantine receipt",
    )?;
    write_atomic(
        &layout::quarantine_receipt_path(generation_dir),
        &bytes,
        "write quarantine receipt",
    )?;
    Ok(QuarantinedGenerationV1 {
        track: SearchPlaneTrackKind::Semantic,
        path: generation_dir.to_path_buf(),
        reason: GenerationQuarantineReasonV1::ContentCorrupt,
        detail,
    })
}

/// What the adapter's seals measured, for the metrics scrape
/// (QI-BB-006 #4): the bytes a seal hashed itself against the bytes and
/// files whose digest it inherited from its base.
#[derive(Debug, Default)]
pub(crate) struct SealTalliesV1 {
    hashed_bytes: AtomicU64,
    inherited_bytes: AtomicU64,
    inherited_files: AtomicU64,
}

impl SealTalliesV1 {
    pub(crate) fn record(&self, measurement: crate::sealed_manifest::SealMeasurementV1) {
        let _previous = self
            .hashed_bytes
            .fetch_add(measurement.hashed_bytes, Ordering::Relaxed);
        let _previous = self
            .inherited_bytes
            .fetch_add(measurement.inherited_bytes, Ordering::Relaxed);
        let _previous = self
            .inherited_files
            .fetch_add(measurement.inherited_files, Ordering::Relaxed);
    }

    /// `semantic_seal_hashed_bytes_total`, `semantic_seal_inherited_bytes_total`
    /// and `semantic_seal_inherited_files_total`.
    pub(crate) fn scrape(&self) -> Vec<MetricPointV1> {
        vec![
            MetricPointV1::counter(
                "semantic_seal_hashed_bytes_total",
                self.hashed_bytes.load(Ordering::Relaxed),
            ),
            MetricPointV1::counter(
                "semantic_seal_inherited_bytes_total",
                self.inherited_bytes.load(Ordering::Relaxed),
            ),
            MetricPointV1::counter(
                "semantic_seal_inherited_files_total",
                self.inherited_files.load(Ordering::Relaxed),
            ),
        ]
    }
}

impl SemanticAdapter {
    fn scrub_with_fence(
        &self,
        generation: &GenerationSnapshot,
        cursor: Option<IntegrityScrubCursorV1>,
        budget: IntegrityScrubBudgetV1,
        before_quarantine: &dyn Fn() -> Result<(), CoreError>,
    ) -> Result<IntegrityScrubReportV1, CoreError> {
        let _mutation = self.generation_mutation_guard(generation)?;
        let _lifecycle = self.directory_lifecycle_read_guard()?;
        let (generation_dir, sealed_digest) = self.sealed_generation_dir(generation, "scrub")?;
        refuse_if_quarantined(&generation_dir)?;
        let mut progress = self.scrub_progress.lock().map_err(|error| {
            CoreError::Storage(format!("semantic scrub progress poisoned: {error}"))
        })?;
        let start = progress.start_or_resume(generation, cursor)?;
        let scrubbed =
            scrub_sealed_manifest(&generation_dir, &sealed_digest, start, budget.max_bytes);
        // An absent or unsupported seal is not proof of corrupt content. The
        // scheduler must nevertheless retire any resident handle before it
        // reports the typed refusal; a future compatible reader can retry.
        if matches!(
            &scrubbed,
            Err(CoreError::Typed {
                code:
                    quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestMissing
                    | quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                ..
            })
        ) {
            before_quarantine()?;
        }
        let SealedManifestScrubV1 {
            files_verified,
            bytes_read,
            verdict,
        } = scrubbed?;
        let outcome = match verdict {
            SealedManifestScrubVerdictV1::Completed(committed) => {
                let receipt = ScrubReceiptV1 {
                    format_version: SCRUB_RECEIPT_FORMAT_VERSION,
                    completed_unix: now_unix()?,
                    files_verified: committed.dataset_files(),
                    bytes_verified: committed.dataset_bytes(),
                };
                let bytes = codec::encode(&receipt, "scrub receipt")?;
                crate::control_file::ensure_bounded(
                    &bytes,
                    crate::control_file::MAX_RECEIPT_BYTES,
                    "scrub receipt",
                )?;
                write_atomic(
                    &layout::scrub_receipt_path(&generation_dir),
                    &bytes,
                    "write scrub receipt",
                )?;
                IntegrityScrubOutcomeV1::Completed
            }
            SealedManifestScrubVerdictV1::Paused { next_artifact } => {
                IntegrityScrubOutcomeV1::Paused {
                    cursor: IntegrityScrubCursorV1 { next_artifact },
                }
            }
            SealedManifestScrubVerdictV1::Corrupt { detail } => {
                before_quarantine()?;
                IntegrityScrubOutcomeV1::Corrupt {
                    quarantined: quarantine(&generation_dir, &detail)?,
                }
            }
        };
        progress.record(generation, &outcome)?;
        drop(progress);
        Ok(IntegrityScrubReportV1 {
            generation: generation.clone(),
            files_verified,
            bytes_read,
            outcome,
        })
    }
}

impl IntegrityScrubPort for SemanticAdapter {
    fn scrub_candidates(&self) -> Result<Vec<IntegrityScrubCandidateV1>, CoreError> {
        let inventory = crate::inventory_persisted_generations(self.state_root())?;
        let mut candidates: Vec<_> = inventory
            .sealed
            .iter()
            .map(|record| {
                let identity = record.identity();
                let generation_dir = layout::generation_dir(
                    self.state_root(),
                    &identity.repo_id,
                    &identity.revision_id,
                    identity.manifest_generation,
                );
                // An invalid completion receipt cannot prove a previous pass.
                // Rescrub this generation without starving other candidates.
                let last_completed_unix = match read_scrub_receipt(&generation_dir) {
                    Ok(receipt) => receipt.map(|receipt| receipt.completed_unix),
                    Err(CoreError::Typed { code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid, .. }) => None,
                    Err(error) => return Err(error),
                };
                Ok(IntegrityScrubCandidateV1 {
                    identity,
                    last_completed_unix,
                })
            })
            .collect::<Result<_, _>>()?;
        // A damaged sealed manifest is not admitted as a sealed generation by
        // inventory, but its independently checked scope manifest and marker
        // still identify the resident key that the scrub must fence. A
        // persistent quarantine receipt is excluded by the identity reader.
        candidates.extend(inventory.quarantined.iter().filter_map(|entry| {
            crate::scrub_candidate_from_quarantined(self.state_root(), entry).map(|record| {
                IntegrityScrubCandidateV1 {
                    identity: record.identity(),
                    last_completed_unix: None,
                }
            })
        }));
        Ok(candidates)
    }

    fn scrub(
        &self,
        generation: &GenerationSnapshot,
        cursor: Option<IntegrityScrubCursorV1>,
        budget: IntegrityScrubBudgetV1,
    ) -> Result<IntegrityScrubReportV1, CoreError> {
        self.scrub_with_fence(generation, cursor, budget, &|| Ok(()))
    }

    fn scrub_with_quarantine_fence(
        &self,
        generation: &GenerationSnapshot,
        cursor: Option<IntegrityScrubCursorV1>,
        budget: IntegrityScrubBudgetV1,
        before_quarantine: &dyn Fn() -> Result<(), CoreError>,
    ) -> Result<IntegrityScrubReportV1, CoreError> {
        self.scrub_with_fence(generation, cursor, budget, before_quarantine)
    }
}

impl DoorFindingQuarantinePort for SemanticAdapter {
    /// Prove the sealed manifest's layout again — the part of every door
    /// that can find a content defect — under the directory lifecycle lock;
    /// quarantine the generation only if that proof fails with one.
    fn quarantine_door_finding(
        &self,
        generation: &GenerationSnapshot,
    ) -> Result<DoorFindingOutcome, CoreError> {
        let _lifecycle = self.directory_lifecycle_guard()?;
        let (generation_dir, sealed_digest) =
            self.sealed_generation_dir(generation, "door-finding quarantine")?;
        if let Some(receipt) = read_quarantine_receipt(&generation_dir)? {
            return Ok(DoorFindingOutcome::Quarantined {
                quarantined: QuarantinedGenerationV1 {
                    track: SearchPlaneTrackKind::Semantic,
                    path: generation_dir,
                    reason: GenerationQuarantineReasonV1::ContentCorrupt,
                    detail: receipt.detail,
                },
            });
        }
        match verify_sealed_manifest(&generation_dir, &sealed_digest) {
            Ok(_manifest) => Ok(DoorFindingOutcome::NotReproduced),
            Err(CoreError::Typed { code, message }) if code == GENERATION_SIDECAR_CORRUPT_CODE => {
                let quarantined = quarantine(&generation_dir, &format!("a door found {message}"))?;
                Ok(DoorFindingOutcome::Quarantined { quarantined })
            }
            Err(other) => Err(other),
        }
    }
}

impl SemanticAdapter {
    /// The directory of the sealed generation `generation` names and the
    /// digest its sealed marker carries, which must be the one named.
    fn sealed_generation_dir(
        &self,
        generation: &GenerationSnapshot,
        what: &str,
    ) -> Result<(std::path::PathBuf, String), CoreError> {
        if generation.track != SearchPlaneTrackKind::Semantic {
            return Err(CoreError::InvalidContract(format!(
                "semantic {what} received {:?} track",
                generation.track
            )));
        }
        let generation_dir = layout::generation_dir(
            self.state_root(),
            &generation.repo_id,
            &generation.revision_id,
            generation.manifest_generation,
        );
        let marker_path = layout::sealed_marker_path(&generation_dir);
        let sealed_digest = crate::control_file::read_string_bounded(
            &marker_path,
            crate::control_file::MAX_SEALED_MARKER_BYTES,
        )
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CoreError::NotReady(format!(
                    "semantic {what}: generation {} is not sealed",
                    generation.manifest_generation.get()
                ))
            } else {
                CoreError::Storage(format!(
                    "semantic: read sealed marker {}: {error}",
                    marker_path.display()
                ))
            }
        })?;
        if sealed_digest != generation.manifest_digest {
            return Err(CoreError::Typed {
                code:
                    quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
                message: format!(
                    "semantic {what}: sealed marker says {sealed_digest} but the target names {} for generation {}",
                    generation.manifest_digest,
                    generation.manifest_generation.get()
                ),
            });
        }
        Ok((generation_dir, sealed_digest))
    }
}
