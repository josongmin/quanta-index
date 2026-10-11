//! Durable scrub and quarantine receipts for sealed lexical generations.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ciborium::Value as CborValue;
use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind};
use quanta_index_core::{CoreError, GenerationQuarantineReasonV1, QuarantinedGenerationV1};
use sha2::{Digest as _, Sha256};

/// File name of the completed-pass receipt inside the generation directory.
pub(crate) const LEXICAL_SCRUB_RECEIPT_FILE_NAME: &str = "search-corpus-generation-scrub.cbor";
/// Completed-pass receipt: manifest digest, completion time, and covered work.
pub(crate) const LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION: u32 = 2;
/// File name of the quarantine receipt inside the generation directory.
pub(crate) const LEXICAL_QUARANTINE_RECEIPT_FILE_NAME: &str =
    "search-corpus-generation-quarantine.cbor";
/// Quarantine receipt: reason code, mismatch detail, and time.
pub(crate) const LEXICAL_QUARANTINE_RECEIPT_FORMAT_VERSION: u32 = 1;
/// Fixed-order CBOR array, format version first.
pub(super) type ScrubReceiptRow = (u32, String, u64, u64, u64);
type QuarantineReceiptRow = (u32, String, String, u64);
pub(super) const MAX_RECEIPT_BYTES: usize = 64 * 1024;
pub(super) const MAX_QUARANTINE_DETAIL_BYTES: usize = 4 * 1024;

pub(super) fn bounded_quarantine_detail(detail: &str) -> String {
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

pub(super) fn now_unix() -> Result<u64, CoreError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs())
        .map_err(|error| {
            CoreError::Storage(format!("lexical: system clock before the epoch: {error}"))
        })
}

/// Read and version-check one receipt beside a generation; `None` when
/// there is none.
pub(super) fn read_receipt_value(
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
    let found = crate::channel_payloads::leading_cbor_array_version(&bytes)
        .map_err(|detail| receipt_invalid(path, detail))?;
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
