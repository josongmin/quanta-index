//! Bounded receipt decoding used by the search door and integrity maintenance.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::path::Path;

use quanta_index_core::publication_admission::PublicationObjectLifetimePort;
use quanta_index_core::{CoreError, GenerationQuarantineReasonV1};

use crate::codec::{cbor_serde, decode_current_format};
use crate::layout;

pub(crate) const SCRUB_RECEIPT_FORMAT_VERSION: u32 = 1;
pub(crate) const QUARANTINE_RECEIPT_FORMAT_VERSION: u32 = 1;

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

/// Read a receipt beside a generation, `None` when there is none.
fn read_receipt<T: serde::de::DeserializeOwned>(
    path: &Path,
    what: &str,
    format_version: u32,
    lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> Result<Option<T>, CoreError> {
    let bytes = match read_receipt_bytes(path, lifetime) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid,
                message: format!("semantic: {what} {} is invalid: {error}", path.display()),
            });
        }
        Err(error) => {
            return Err(crate::errors::fs_err(&format!("read {what}"), path, &error));
        }
    };
    decode_current_format(&bytes, what, format_version)
        .map(Some)
        .map_err(|error| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid,
            message: format!("semantic: {what} {} is invalid: {error}", path.display()),
        })
}

fn read_receipt_bytes(
    path: &Path,
    lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> std::io::Result<Vec<u8>> {
    crate::control_file::read_bounded_for_generation(
        path,
        crate::control_file::MAX_RECEIPT_BYTES,
        lifetime,
    )
}

pub(crate) fn read_scrub_receipt(
    generation_dir: &Path,
    lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> Result<Option<ScrubReceiptV1>, CoreError> {
    let path = layout::scrub_receipt_path(generation_dir);
    let bytes = match read_receipt_bytes(&path, lifetime) {
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
            return Err(crate::errors::fs_err("read scrub receipt", &path, &error));
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

pub(crate) fn read_quarantine_receipt(
    generation_dir: &Path,
    lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> Result<Option<QuarantineReceiptV1>, CoreError> {
    let receipt: Option<QuarantineReceiptV1> = read_receipt(
        &layout::quarantine_receipt_path(generation_dir),
        "quarantine receipt",
        QUARANTINE_RECEIPT_FORMAT_VERSION,
        lifetime,
    )?;
    if let Some(receipt) = receipt.as_ref()
        && GenerationQuarantineReasonV1::from_code_str(&receipt.reason).is_none()
    {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid,
            message: format!(
                "semantic: quarantine receipt under {} names an unknown reason `{}`",
                generation_dir.display(),
                receipt.reason
            ),
        });
    }
    Ok(receipt)
}

pub(crate) fn refuse_if_quarantined(
    generation_dir: &Path,
    lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> Result<(), CoreError> {
    let Some(receipt) = read_quarantine_receipt(generation_dir, lifetime)? else {
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
