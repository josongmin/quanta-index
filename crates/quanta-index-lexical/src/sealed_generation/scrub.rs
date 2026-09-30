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

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ciborium::Value as CborValue;
use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind};
use quanta_index_core::{
    CoreError, GenerationQuarantineReasonV1, IntegrityScrubBudgetV1, IntegrityScrubCursorV1,
    IntegrityScrubOutcomeV1, IntegrityScrubReportV1, QuarantinedGenerationV1,
    SealedArtifactCommitmentV1, TreeCommitmentMismatchV1, TreeScrubVerdictV1,
    hash_committed_step_opened_v1,
};
use sha2::{Digest as _, Sha256};

use crate::sealed_generation::manifest::read_bound_manifest_at;
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
/// Wire shape of the completed-pass receipt: a fixed-order CBOR array,
/// format version first.
type ScrubReceiptRow = (u32, String, u64, u64, u64);
/// Wire shape of the quarantine receipt, format version first.
type QuarantineReceiptRow = (u32, String, String, u64);
const MAX_RECEIPT_BYTES: usize = 64 * 1024;
const MAX_QUARANTINE_DETAIL_BYTES: usize = 4 * 1024;

fn bounded_quarantine_detail(detail: &str) -> String {
    if detail.len() <= MAX_QUARANTINE_DETAIL_BYTES {
        return detail.to_string();
    }
    let fingerprint = format!("{:x}", Sha256::digest(detail.as_bytes()));
    let suffix = format!("... [truncated; original_sha256={fingerprint}]");
    let prefix_limit = MAX_QUARANTINE_DETAIL_BYTES.saturating_sub(suffix.len());
    let prefix: String = detail
        .char_indices()
        .take_while(|(start, ch)| start.saturating_add(ch.len_utf8()) <= prefix_limit)
        .map(|(_, ch)| ch)
        .collect();
    format!("{prefix}{suffix}")
}

fn scrub_receipt_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(LEXICAL_SCRUB_RECEIPT_FILE_NAME)
}

fn quarantine_receipt_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(LEXICAL_QUARANTINE_RECEIPT_FILE_NAME)
}

fn receipt_invalid(path: &Path, reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid,
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
    read_receipt_value_with(path, what, format_version, |directory, name| {
        super::open_regular_nofollow(directory, name)
    })
}

fn read_receipt_value_at(
    root: &File,
    path: &Path,
    what: &str,
    format_version: u32,
) -> Result<Option<CborValue>, CoreError> {
    read_receipt_value_with(path, what, format_version, |_directory, name| {
        super::open_regular_below(root, name)
    })
}

fn read_receipt_value_with(
    path: &Path,
    what: &str,
    format_version: u32,
    open: impl FnOnce(&Path, &Path) -> std::io::Result<File>,
) -> Result<Option<CborValue>, CoreError> {
    let directory = path
        .parent()
        .ok_or_else(|| receipt_invalid(path, "has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| receipt_invalid(path, "has no file name"))?;
    let mut file = match open(directory, Path::new(name)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if super::is_unsafe_artifact_path(&error) => {
            return Err(receipt_invalid(path, "is not a regular file"));
        }
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "lexical: read {what} {}: {error}",
                path.display()
            )));
        }
    };
    let bytes = super::read_opened_bounded(&mut file, MAX_RECEIPT_BYTES).map_err(|error| {
        if matches!(
            error.kind(),
            std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
        ) {
            receipt_invalid(path, &format!("cannot read admitted bytes: {error}"))
        } else {
            CoreError::Storage(format!("lexical: read {what} {}: {error}", path.display()))
        }
    })?;
    let value: CborValue = crate::channel_payloads::decode_cbor_exact(bytes.as_slice())
        .map_err(|error| receipt_invalid(path, &format!("does not decode: {error}")))?;
    let found = leading_format_version(&value).map_err(|detail| receipt_invalid(path, detail))?;
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
    let value = read_receipt_value(
        &path,
        "quarantine receipt",
        LEXICAL_QUARANTINE_RECEIPT_FORMAT_VERSION,
    )?;
    decode_quarantine_receipt(generation_dir, &path, value)
}

pub(crate) fn quarantined_by_scrub_at(
    root: &File,
    generation_dir: &Path,
) -> Result<Option<QuarantinedGenerationV1>, CoreError> {
    let path = quarantine_receipt_path(generation_dir);
    let value = read_receipt_value_at(
        root,
        &path,
        "quarantine receipt",
        LEXICAL_QUARANTINE_RECEIPT_FORMAT_VERSION,
    )?;
    decode_quarantine_receipt(generation_dir, &path, value)
}

fn decode_quarantine_receipt(
    generation_dir: &Path,
    path: &Path,
    value: Option<CborValue>,
) -> Result<Option<QuarantinedGenerationV1>, CoreError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let (_format, reason_code, detail, detected_unix): QuarantineReceiptRow = value
        .deserialized()
        .map_err(|error| receipt_invalid(path, &format!("does not decode: {error}")))?;
    let reason = GenerationQuarantineReasonV1::from_code_str(&reason_code).ok_or_else(|| {
        receipt_invalid(path, &format!("names an unknown reason `{reason_code}`"))
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
pub(crate) fn refuse_if_quarantined_at(
    root: &File,
    generation_dir: &Path,
) -> Result<(), CoreError> {
    refuse_quarantine_entry(
        generation_dir,
        quarantined_by_scrub_at(root, generation_dir)?,
    )
}

fn refuse_quarantine_entry(
    generation_dir: &Path,
    entry: Option<QuarantinedGenerationV1>,
) -> Result<(), CoreError> {
    let Some(entry) = entry else {
        return Ok(());
    };
    Err(CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationQuarantined,
        message: format!(
            "lexical: generation {} is quarantined as {}: {}; discard it through the quarantine surface or rebuild it from its producer",
            generation_dir.display(),
            entry.reason.as_code_str(),
            entry.detail
        ),
    })
}

/// Every committed file present at its committed length, through an anchored
/// descriptor. A symlink is corruption even when its target has valid bytes.
fn verify_committed_lengths(
    root: &File,
    generation_dir: &Path,
    committed: &[SealedArtifactCommitmentV1],
) -> Result<Option<TreeCommitmentMismatchV1>, CoreError> {
    for artifact in committed {
        let path = generation_dir.join(&artifact.name);
        match super::open_regular_below(root, Path::new(&artifact.name))
            .and_then(|file| file.metadata())
        {
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
            Err(error) if super::is_unsafe_artifact_path(&error) => {
                return Ok(Some(TreeCommitmentMismatchV1::Digest {
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
    root: &File,
    generation_dir: &Path,
    mismatch: &TreeCommitmentMismatchV1,
) -> Result<QuarantinedGenerationV1, CoreError> {
    quarantine_content_corrupt_at(root, generation_dir, &format!("scrub found {mismatch}"))
}

/// Record that `generation_dir`'s content no longer matches its seal.
///
/// The receipt the inventory lists as content-corrupt and every door
/// refuses by. The scrub writes it, and so does the re-proof of a door's
/// finding (QI-BB-017, QI-BB-026).
pub(crate) fn quarantine_content_corrupt_at(
    root: &File,
    generation_dir: &Path,
    detail: &str,
) -> Result<QuarantinedGenerationV1, CoreError> {
    quarantine_seal_failure_at(
        root,
        generation_dir,
        GenerationQuarantineReasonV1::ContentCorrupt,
        detail,
    )
}

/// Classify persistent physical defects in a seal.
///
/// Missing and unsupported formats are not content proofs: callers fence
/// their resident handles but retain the typed refusal.
pub(crate) fn seal_failure_quarantine_reason(
    error: &CoreError,
) -> Option<GenerationQuarantineReasonV1> {
    match error {
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
            ..
        } => Some(GenerationQuarantineReasonV1::ContentCorrupt),
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
            ..
        } => Some(GenerationQuarantineReasonV1::IdentityDigestMismatch),
        CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_) => None,
    }
}

pub(crate) fn quarantine_seal_failure_at(
    root: &File,
    generation_dir: &Path,
    reason: GenerationQuarantineReasonV1,
    detail: &str,
) -> Result<QuarantinedGenerationV1, CoreError> {
    let detail = bounded_quarantine_detail(detail);
    let row: QuarantineReceiptRow = (
        LEXICAL_QUARANTINE_RECEIPT_FORMAT_VERSION,
        reason.as_code_str().to_string(),
        detail.clone(),
        now_unix()?,
    );
    let bytes = crate::channel_payloads::encode_cbor(&row, "quarantine receipt")?;
    if bytes.len() > MAX_RECEIPT_BYTES {
        return Err(CoreError::Storage(format!(
            "lexical: quarantine receipt exceeds {MAX_RECEIPT_BYTES} encoded bytes"
        )));
    }
    crate::index_store::write_atomic_durable_at(
        root,
        generation_dir,
        LEXICAL_QUARANTINE_RECEIPT_FILE_NAME,
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
    start_artifact: u64,
    budget: IntegrityScrubBudgetV1,
    before_quarantine: &dyn Fn() -> Result<(), CoreError>,
) -> Result<IntegrityScrubReportV1, CoreError> {
    let root = super::open_generation_dir_nofollow(generation_dir).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: open generation for scrub {}: {error}",
            generation_dir.display()
        ))
    })?;
    let observed = crate::index_store::read_lexical_sealed_identity_at(generation_dir, &root)?;
    crate::index_store::validate_lexical_sealed_identity(&observed, identity)?;
    refuse_if_quarantined_at(&root, generation_dir)?;
    let manifest = match read_bound_manifest_at(generation_dir, &root, &identity.manifest_digest) {
        Ok(manifest) => manifest,
        Err(error) => {
            if let Some(reason) = seal_failure_quarantine_reason(&error) {
                before_quarantine()?;
                return Ok(IntegrityScrubReportV1 {
                    generation: identity.clone(),
                    files_verified: 0,
                    bytes_read: 0,
                    outcome: IntegrityScrubOutcomeV1::Corrupt {
                        quarantined: quarantine_seal_failure_at(
                            &root,
                            generation_dir,
                            reason,
                            &format!("scrub cannot authenticate sealed manifest: {error}"),
                        )?,
                    },
                });
            }
            if matches!(
                &error,
                CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestMissing
                        | quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                    ..
                }
            ) {
                before_quarantine()?;
            }
            return Err(error);
        }
    };
    let mut committed: Vec<SealedArtifactCommitmentV1> =
        manifest.all_commitments().cloned().collect();
    if let Some(coverage_root) = &manifest.source_coverage {
        let pages = crate::sealed_generation::coverage::root_page_commitments_at(
            &root,
            generation_dir,
            coverage_root,
            identity,
        );
        match pages {
            Ok(pages) => committed.extend(pages),
            Err(
                error @ CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt
                        | quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    ..
                },
            ) => {
                before_quarantine()?;
                return Ok(IntegrityScrubReportV1 {
                    generation: identity.clone(),
                    files_verified: 0,
                    bytes_read: 0,
                    outcome: IntegrityScrubOutcomeV1::Corrupt {
                        quarantined: quarantine_content_corrupt_at(
                            &root,
                            generation_dir,
                            &format!("scrub cannot authenticate committed coverage root: {error}"),
                        )?,
                    },
                });
            }
            Err(error) => return Err(error),
        }
    }
    if let Some(mismatch) = verify_committed_lengths(&root, generation_dir, &committed)? {
        before_quarantine()?;
        return Ok(IntegrityScrubReportV1 {
            generation: identity.clone(),
            files_verified: 0,
            bytes_read: 0,
            outcome: IntegrityScrubOutcomeV1::Corrupt {
                quarantined: quarantine(&root, generation_dir, &mismatch)?,
            },
        });
    }
    let step = hash_committed_step_opened_v1(
        &|name| super::open_regular_below(&root, Path::new(name)),
        &committed,
        start_artifact,
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
            let files = crate::channel_payloads::count_from_len(committed.len())?;
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
            let receipt = crate::channel_payloads::encode_cbor(&row, "scrub receipt")?;
            if receipt.len() > MAX_RECEIPT_BYTES {
                return Err(CoreError::Storage(format!(
                    "lexical: scrub receipt exceeds {MAX_RECEIPT_BYTES} encoded bytes"
                )));
            }
            crate::index_store::write_atomic_durable_at(
                &root,
                generation_dir,
                LEXICAL_SCRUB_RECEIPT_FILE_NAME,
                &receipt,
                "scrub receipt",
            )?;
            IntegrityScrubOutcomeV1::Completed
        }
        TreeScrubVerdictV1::Paused { next_artifact } => IntegrityScrubOutcomeV1::Paused {
            cursor: IntegrityScrubCursorV1 { next_artifact },
        },
        TreeScrubVerdictV1::Mismatch(mismatch) => {
            before_quarantine()?;
            IntegrityScrubOutcomeV1::Corrupt {
                quarantined: quarantine(&root, generation_dir, &mismatch)?,
            }
        }
    };
    Ok(IntegrityScrubReportV1 {
        generation: identity.clone(),
        files_verified: step.files_verified,
        bytes_read: step.bytes_read,
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use sha2::Digest as _;

    #[test]
    fn completed_receipt_rejects_trailing_cbor() -> Result<(), Box<dyn std::error::Error>> {
        let generation = tempfile::tempdir()?;
        let path = generation
            .path()
            .join(super::LEXICAL_SCRUB_RECEIPT_FILE_NAME);
        let row: super::ScrubReceiptRow = (
            super::LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION,
            "digest".into(),
            1,
            1,
            1,
        );
        let mut bytes = crate::channel_payloads::encode_cbor(&row, "scrub test")?;
        bytes.push(0xff);
        std::fs::write(&path, bytes)?;
        match super::read_receipt_value(
            &path,
            "scrub receipt",
            super::LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION,
        ) {
            Err(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid,
                ..
            }) => Ok(()),
            other => Err(format!("trailing receipt bytes were admitted: {other:?}").into()),
        }
    }

    #[test]
    fn receipt_write_stays_in_opened_generation_after_path_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let parent = tempfile::tempdir()?;
        let generation = parent.path().join("generation");
        std::fs::create_dir(&generation)?;
        let opened = super::super::open_generation_dir_nofollow(&generation)?;
        let old = parent.path().join("old-generation");
        std::fs::rename(&generation, &old)?;
        std::fs::create_dir(&generation)?;
        crate::index_store::write_atomic_durable_at(
            &opened,
            &generation,
            super::LEXICAL_SCRUB_RECEIPT_FILE_NAME,
            b"pinned",
            "scrub receipt test",
        )?;
        if std::fs::read(old.join(super::LEXICAL_SCRUB_RECEIPT_FILE_NAME))? != b"pinned"
            || generation
                .join(super::LEXICAL_SCRUB_RECEIPT_FILE_NAME)
                .exists()
        {
            return Err("receipt write followed a replacement generation".into());
        }
        Ok(())
    }

    #[test]
    fn long_multibyte_quarantine_detail_stays_bounded_and_identifiable() {
        let original = "가".repeat(2_000);
        let fingerprint = format!("{:x}", sha2::Sha256::digest(original.as_bytes()));
        let bounded = super::bounded_quarantine_detail(&original);
        assert!(bounded.len() <= super::MAX_QUARANTINE_DETAIL_BYTES);
        assert!(bounded.contains(&format!("original_sha256={fingerprint}")));
        assert!(
            bounded
                .split("... [truncated;")
                .next()
                .is_some_and(|prefix| !prefix.is_empty() && original.starts_with(prefix))
        );
    }

    #[test]
    fn oversized_receipt_refuses_before_decode() -> Result<(), Box<dyn std::error::Error>> {
        let generation = tempfile::tempdir()?;
        let path = generation
            .path()
            .join(super::LEXICAL_SCRUB_RECEIPT_FILE_NAME);
        let file = std::fs::File::create(&path)?;
        file.set_len(u64::try_from(super::MAX_RECEIPT_BYTES)? + 1)?;
        let result = super::read_receipt_value(
            &path,
            "scrub receipt",
            super::LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION,
        );
        if !matches!(
            result,
            Err(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid,
                ..
            })
        ) {
            return Err("oversized receipt did not refuse typed".into());
        }
        Ok(())
    }
}
