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

use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind};
use quanta_index_core::{
    CoreError, DoorFindingOutcome, DoorFindingQuarantinePort, GENERATION_QUARANTINED_CODE,
    GENERATION_SIDECAR_CORRUPT_CODE, GenerationQuarantineReasonV1, IntegrityScrubBudgetV1,
    IntegrityScrubCandidateV1, IntegrityScrubCursorV1, IntegrityScrubOutcomeV1, IntegrityScrubPort,
    IntegrityScrubReportV1, MetricPointV1, QuarantinedGenerationV1,
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
    let bytes = match std::fs::read(path) {
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
    read_receipt(
        &layout::scrub_receipt_path(generation_dir),
        "scrub receipt",
        SCRUB_RECEIPT_FORMAT_VERSION,
    )
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
        code: GENERATION_QUARANTINED_CODE.to_string(),
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
fn quarantine(generation_dir: &Path, detail: String) -> Result<QuarantinedGenerationV1, CoreError> {
    let receipt = QuarantineReceiptV1 {
        format_version: QUARANTINE_RECEIPT_FORMAT_VERSION,
        reason: GenerationQuarantineReasonV1::ContentCorrupt
            .as_code_str()
            .to_string(),
        detail: detail.clone(),
        detected_unix: now_unix()?,
    };
    write_atomic(
        &layout::quarantine_receipt_path(generation_dir),
        &codec::encode(&receipt, "quarantine receipt")?,
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

impl IntegrityScrubPort for SemanticAdapter {
    fn scrub_candidates(&self) -> Result<Vec<IntegrityScrubCandidateV1>, CoreError> {
        let inventory = crate::inventory_persisted_generations(self.state_root())?;
        inventory
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
                let last_completed_unix =
                    read_scrub_receipt(&generation_dir)?.map(|receipt| receipt.completed_unix);
                Ok(IntegrityScrubCandidateV1 {
                    identity,
                    last_completed_unix,
                })
            })
            .collect()
    }

    fn scrub(
        &self,
        generation: &GenerationSnapshot,
        cursor: Option<IntegrityScrubCursorV1>,
        budget: IntegrityScrubBudgetV1,
    ) -> Result<IntegrityScrubReportV1, CoreError> {
        let _lifecycle = self.directory_lifecycle_guard()?;
        let (generation_dir, sealed_digest) = self.sealed_generation_dir(generation, "scrub")?;
        refuse_if_quarantined(&generation_dir)?;
        let start = cursor.map_or(0, |cursor| cursor.next_artifact);
        let SealedManifestScrubV1 {
            files_verified,
            bytes_read,
            verdict,
        } = scrub_sealed_manifest(&generation_dir, &sealed_digest, start, budget.max_bytes)?;
        let outcome = match verdict {
            SealedManifestScrubVerdictV1::Completed(committed) => {
                let receipt = ScrubReceiptV1 {
                    format_version: SCRUB_RECEIPT_FORMAT_VERSION,
                    completed_unix: now_unix()?,
                    files_verified: committed.dataset_files(),
                    bytes_verified: committed.dataset_bytes(),
                };
                write_atomic(
                    &layout::scrub_receipt_path(&generation_dir),
                    &codec::encode(&receipt, "scrub receipt")?,
                    "write scrub receipt",
                )?;
                IntegrityScrubOutcomeV1::Completed
            }
            SealedManifestScrubVerdictV1::Paused { next_artifact } => {
                IntegrityScrubOutcomeV1::Paused {
                    cursor: IntegrityScrubCursorV1 { next_artifact },
                }
            }
            SealedManifestScrubVerdictV1::Corrupt { detail } => IntegrityScrubOutcomeV1::Corrupt {
                quarantined: quarantine(&generation_dir, detail)?,
            },
        };
        Ok(IntegrityScrubReportV1 {
            generation: generation.clone(),
            files_verified,
            bytes_read,
            outcome,
        })
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
                let quarantined = quarantine(&generation_dir, format!("a door found {message}"))?;
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
        let sealed_digest = std::fs::read_to_string(&marker_path).map_err(|error| {
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
                code: "GENERATION_IDENTITY_DIGEST_MISMATCH".to_string(),
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
