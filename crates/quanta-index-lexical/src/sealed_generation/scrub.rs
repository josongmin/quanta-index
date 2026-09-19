//! The integrity scrub over sealed lexical generations, and the receipts it
//! leaves beside them (QI-BB-017).
//!
//! A door proves the index segment files by presence and length only; the
//! seal proved their content once, and between seals only the scrub does.
//! The scrub is bounded and resumable: each step re-checks every committed
//! file's presence and length, then hashes committed files from a cursor
//! until a byte budget is spent. Two receipts live beside a generation,
//! written by atomic durable rename and never part of the sealed manifest
//! (so writing one never makes the generation look tampered with):
//!
//! - the scrub receipt, when a pass over every committed file completed
//!   and matched — the manifest digest it proved and when;
//! - the quarantine receipt, when a committed file did not match. While it
//!   exists the inventory quarantines the generation as
//!   `GENERATION_QUARANTINE_CONTENT_CORRUPT` and every door refuses it
//!   typed `GENERATION_QUARANTINED`; the quarantine discard surface is the
//!   one way it leaves the disk.

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ciborium::Value as CborValue;
use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind};
use quanta_index_core::{
    CoreError, GENERATION_QUARANTINED_CODE, GenerationQuarantineReasonV1, IntegrityScrubBudgetV1,
    IntegrityScrubCursorV1, IntegrityScrubOutcomeV1, IntegrityScrubReportV1,
    QuarantinedGenerationV1, SealedArtifactCommitmentV1, TreeCommitmentMismatchV1,
    TreeScrubVerdictV1, hash_committed_step_v1,
};

use crate::sealed_generation::manifest::read_bound_manifest;
use crate::text_authority::leading_format_version;

/// File name of the completed-pass receipt inside the generation directory.
pub(crate) const LEXICAL_SCRUB_RECEIPT_FILE_NAME: &str = "search-corpus-generation-scrub.cbor";
/// Completed-pass receipt: the manifest digest the pass proved, when it
/// completed (Unix seconds), and what it covered.
pub(crate) const LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION: u32 = 2;
/// File name of the quarantine receipt inside the generation directory.
pub(crate) const LEXICAL_QUARANTINE_RECEIPT_FILE_NAME: &str =
    "search-corpus-generation-quarantine.cbor";
/// Quarantine receipt: the reason code, what did not match, and when.
pub(crate) const LEXICAL_QUARANTINE_RECEIPT_FORMAT_VERSION: u32 = 1;
/// Typed refusal for a receipt this build cannot trust: unreadable,
/// another format, or written for a different sealed digest.
pub(crate) const GENERATION_SCRUB_RECEIPT_INVALID_CODE: &str = "GENERATION_SCRUB_RECEIPT_INVALID";

/// Wire shape of the completed-pass receipt: a fixed-order CBOR array,
/// format version first.
type ScrubReceiptRow = (u32, String, u64, u64, u64);
/// Wire shape of the quarantine receipt, format version first.
type QuarantineReceiptRow = (u32, String, String, u64);

fn scrub_receipt_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(LEXICAL_SCRUB_RECEIPT_FILE_NAME)
}

fn quarantine_receipt_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(LEXICAL_QUARANTINE_RECEIPT_FILE_NAME)
}

fn receipt_invalid(path: &Path, reason: &str) -> CoreError {
    CoreError::Typed {
        code: GENERATION_SCRUB_RECEIPT_INVALID_CODE.to_string(),
        message: format!("lexical: receipt {}: {reason}", path.display()),
    }
}

fn now_unix() -> Result<u64, CoreError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs())
        .map_err(|error| {
            CoreError::Storage(format!("lexical: system clock before the epoch: {error}"))
        })
}

/// Read and version-check one receipt beside a generation; `None` when
/// there is none.
fn read_receipt_value(
    path: &Path,
    what: &str,
    format_version: u32,
) -> Result<Option<CborValue>, CoreError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "lexical: read {what} {}: {error}",
                path.display()
            )));
        }
    };
    let value: CborValue = ciborium::from_reader(bytes.as_slice())
        .map_err(|error| receipt_invalid(path, &format!("does not decode: {error}")))?;
    let found = leading_format_version(&value, what, path)?;
    if found != format_version {
        return Err(receipt_invalid(
            path,
            &format!("has format {found}, this build serves {format_version}"),
        ));
    }
    Ok(Some(value))
}

/// When the last completed pass over the generation sealed for `identity`
/// finished (Unix seconds), or `None` when none ever did.
pub(crate) fn last_completed_scrub(
    generation_dir: &Path,
    identity: &GenerationSnapshot,
) -> Result<Option<u64>, CoreError> {
    let path = scrub_receipt_path(generation_dir);
    let Some(value) =
        read_receipt_value(&path, "scrub receipt", LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION)?
    else {
        return Ok(None);
    };
    let (_format, manifest_digest, completed_unix, _files, _bytes): ScrubReceiptRow = value
        .deserialized()
        .map_err(|error| receipt_invalid(&path, &format!("does not decode: {error}")))?;
    if manifest_digest != identity.manifest_digest {
        return Err(receipt_invalid(
            &path,
            &format!(
                "records a pass over digest {manifest_digest} but the identity says {}",
                identity.manifest_digest
            ),
        ));
    }
    Ok(Some(completed_unix))
}

/// The quarantine receipt beside `generation_dir`, as the inventory entry
/// it stands for, or `None` when the scrub never proved it corrupt.
pub(crate) fn quarantined_by_scrub(
    generation_dir: &Path,
) -> Result<Option<QuarantinedGenerationV1>, CoreError> {
    let path = quarantine_receipt_path(generation_dir);
    let Some(value) = read_receipt_value(
        &path,
        "quarantine receipt",
        LEXICAL_QUARANTINE_RECEIPT_FORMAT_VERSION,
    )?
    else {
        return Ok(None);
    };
    let (_format, reason_code, detail, detected_unix): QuarantineReceiptRow = value
        .deserialized()
        .map_err(|error| receipt_invalid(&path, &format!("does not decode: {error}")))?;
    let reason = GenerationQuarantineReasonV1::from_code_str(&reason_code).ok_or_else(|| {
        receipt_invalid(&path, &format!("names an unknown reason `{reason_code}`"))
    })?;
    Ok(Some(QuarantinedGenerationV1 {
        track: SearchPlaneTrackKind::Lexical,
        path: generation_dir.to_path_buf(),
        reason,
        detail: format!("{detail} (detected at unix {detected_unix})"),
    }))
}

/// Refuse a generation the scrub quarantined, typed, naming the receipt.
/// Every door and the scrub itself run this first.
pub(crate) fn refuse_if_quarantined(generation_dir: &Path) -> Result<(), CoreError> {
    let Some(entry) = quarantined_by_scrub(generation_dir)? else {
        return Ok(());
    };
    Err(CoreError::Typed {
        code: GENERATION_QUARANTINED_CODE.to_string(),
        message: format!(
            "lexical: generation {} is quarantined as {}: {}; discard it through the quarantine surface or rebuild it from its producer",
            generation_dir.display(),
            entry.reason.as_code_str(),
            entry.detail
        ),
    })
}

/// A committed name as a path inside `generation_dir`: relative and made
/// of plain components only, so no manifest entry can reach outside.
fn committed_child(generation_dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    let relative = Path::new(name);
    if relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Ok(generation_dir.join(relative))
    } else {
        Err(std::io::Error::other(format!(
            "committed name {name} is not a plain relative path"
        )))
    }
}

/// Every committed file present at its committed length, from directory
/// metadata only: the cheap part of each step, so a missing or resized
/// file is found whatever the cursor.
fn verify_committed_lengths(
    generation_dir: &Path,
    committed: &[SealedArtifactCommitmentV1],
) -> Result<Option<TreeCommitmentMismatchV1>, CoreError> {
    for artifact in committed {
        let path = committed_child(generation_dir, &artifact.name)
            .map_err(|error| CoreError::Storage(format!("lexical: scrub: {error}")))?;
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.len() == artifact.bytes => {}
            Ok(metadata) => {
                return Ok(Some(TreeCommitmentMismatchV1::Length {
                    name: artifact.name.clone(),
                    on_disk: metadata.len(),
                    committed: artifact.bytes,
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Some(TreeCommitmentMismatchV1::Missing {
                    name: artifact.name.clone(),
                }));
            }
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: scrub: stat {}: {error}",
                    path.display()
                )));
            }
        }
    }
    Ok(None)
}

/// Write the quarantine receipt the inventory and every door will read.
fn quarantine(
    generation_dir: &Path,
    mismatch: &TreeCommitmentMismatchV1,
) -> Result<QuarantinedGenerationV1, CoreError> {
    quarantine_content_corrupt(generation_dir, format!("scrub found {mismatch}"))
}

/// Record that `generation_dir`'s content no longer matches its seal.
///
/// The receipt the inventory lists as content-corrupt and every door
/// refuses by. The scrub writes it, and so does the re-proof of a door's
/// finding (QI-BB-017, QI-BB-026).
pub(crate) fn quarantine_content_corrupt(
    generation_dir: &Path,
    detail: String,
) -> Result<QuarantinedGenerationV1, CoreError> {
    let reason = GenerationQuarantineReasonV1::ContentCorrupt;
    let row: QuarantineReceiptRow = (
        LEXICAL_QUARANTINE_RECEIPT_FORMAT_VERSION,
        reason.as_code_str().to_string(),
        detail.clone(),
        now_unix()?,
    );
    let bytes = crate::encode_cbor(&row, "quarantine receipt")?;
    crate::write_atomic_durable(
        &quarantine_receipt_path(generation_dir),
        &bytes,
        "quarantine receipt",
    )?;
    Ok(QuarantinedGenerationV1 {
        track: SearchPlaneTrackKind::Lexical,
        path: generation_dir.to_path_buf(),
        reason,
        detail,
    })
}

/// One bounded scrub step over the generation sealed for `identity`.
///
/// The manifest must be the one the identity binds; a quarantined
/// generation is refused. Every committed file is checked for presence
/// and length, then files are hashed from `cursor` until `budget` is
/// spent. A mismatch writes the quarantine receipt and reports `Corrupt`;
/// a pass that reaches the end writes the completed-pass receipt.
pub(crate) fn scrub_step(
    generation_dir: &Path,
    identity: &GenerationSnapshot,
    cursor: Option<IntegrityScrubCursorV1>,
    budget: IntegrityScrubBudgetV1,
) -> Result<IntegrityScrubReportV1, CoreError> {
    refuse_if_quarantined(generation_dir)?;
    let manifest = read_bound_manifest(generation_dir, &identity.manifest_digest)?;
    let committed: Vec<SealedArtifactCommitmentV1> = manifest.all_commitments().cloned().collect();
    if let Some(mismatch) = verify_committed_lengths(generation_dir, &committed)? {
        return Ok(IntegrityScrubReportV1 {
            generation: identity.clone(),
            files_verified: 0,
            bytes_read: 0,
            outcome: IntegrityScrubOutcomeV1::Corrupt {
                quarantined: quarantine(generation_dir, &mismatch)?,
            },
        });
    }
    let step = hash_committed_step_v1(
        &|name| committed_child(generation_dir, name),
        &committed,
        cursor.map_or(0, |cursor| cursor.next_artifact),
        budget.max_bytes,
    )
    .map_err(|error| {
        CoreError::Storage(format!(
            "lexical: scrub {}: {error}",
            generation_dir.display()
        ))
    })?;
    let outcome = match step.verdict {
        TreeScrubVerdictV1::Completed => {
            let files = crate::count_from_len(committed.len())?;
            let bytes = committed.iter().fold(0_u64, |total, artifact| {
                total.saturating_add(artifact.bytes)
            });
            let row: ScrubReceiptRow = (
                LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION,
                identity.manifest_digest.clone(),
                now_unix()?,
                files,
                bytes,
            );
            crate::write_atomic_durable(
                &scrub_receipt_path(generation_dir),
                &crate::encode_cbor(&row, "scrub receipt")?,
                "scrub receipt",
            )?;
            IntegrityScrubOutcomeV1::Completed
        }
        TreeScrubVerdictV1::Paused { next_artifact } => IntegrityScrubOutcomeV1::Paused {
            cursor: IntegrityScrubCursorV1 { next_artifact },
        },
        TreeScrubVerdictV1::Mismatch(mismatch) => IntegrityScrubOutcomeV1::Corrupt {
            quarantined: quarantine(generation_dir, &mismatch)?,
        },
    };
    Ok(IntegrityScrubReportV1 {
        generation: identity.clone(),
        files_verified: step.files_verified,
        bytes_read: step.bytes_read,
        outcome,
    })
}
