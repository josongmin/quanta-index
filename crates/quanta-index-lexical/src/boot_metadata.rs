//! Engine-independent readers for lexical boot metadata.
//!
//! Boot inventory must survive removal of the optional serving engine.  These
//! readers therefore depend only on the persisted DATA contract and anchored
//! filesystem access.

use std::{fs::File, path::Path};

use ciborium::Value as CborValue;
use quanta_index_contract::{GenerationSnapshot, SearchPlaneErrorCodeV2, SearchPlaneTrackKind};
use quanta_index_core::{CoreError, GenerationQuarantineReasonV1, QuarantinedGenerationV1};

use crate::legacy::errors::sidecar_corrupt;
use crate::legacy::names::{
    LEXICAL_QUARANTINE_RECEIPT_FILE_NAME, LEXICAL_SEALED_IDENTITY_FILE_NAME,
    MAX_SEALED_IDENTITY_BYTES,
};

pub(crate) const LEXICAL_QUARANTINE_RECEIPT_FORMAT_VERSION: u32 = 1;
pub(crate) const MAX_RECEIPT_BYTES: usize = 64 * 1024;

type QuarantineReceiptRow = (u32, String, String, u64);

pub(crate) fn sealed_identity_entry_present_at(
    root: &File,
    generation_dir: &Path,
) -> Result<bool, CoreError> {
    let path = generation_dir.join(LEXICAL_SEALED_IDENTITY_FILE_NAME);
    crate::sealed_path_io::optional_entry_file_type_at(
        root,
        Path::new(LEXICAL_SEALED_IDENTITY_FILE_NAME),
    )
    .map(|entry| entry.is_some())
    .map_err(|error| {
        CoreError::Storage(format!(
            "lexical: inspect sealed identity {}: {error}",
            path.display()
        ))
    })
}

pub(crate) fn read_lexical_sealed_identity_at(
    root: &File,
    generation_dir: &Path,
) -> Result<GenerationSnapshot, CoreError> {
    let path = generation_dir.join(LEXICAL_SEALED_IDENTITY_FILE_NAME);
    let file_type = crate::sealed_path_io::optional_entry_file_type_at(
        root,
        Path::new(LEXICAL_SEALED_IDENTITY_FILE_NAME),
    )
    .map_err(|error| {
        CoreError::Storage(format!(
            "lexical: inspect sealed identity {}: {error}",
            path.display()
        ))
    })?;
    let Some(file_type) = file_type else {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::GenerationIdentityIncomplete,
            message: format!(
                "lexical: incomplete generation has no sealed identity at {}",
                path.display()
            ),
        });
    };
    if file_type != rustix::fs::FileType::RegularFile {
        return Err(sidecar_corrupt(
            generation_dir,
            LEXICAL_SEALED_IDENTITY_FILE_NAME,
            "is not a regular file within its generation",
        ));
    }
    let bytes = crate::sealed_path_io::read_regular_bounded_at(
        root,
        Path::new(LEXICAL_SEALED_IDENTITY_FILE_NAME),
        MAX_SEALED_IDENTITY_BYTES,
    )
    .map_err(|error| {
        if crate::sealed_path_io::is_unsafe_artifact_path(&error)
            || matches!(
                error.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
            )
        {
            sidecar_corrupt(
                generation_dir,
                LEXICAL_SEALED_IDENTITY_FILE_NAME,
                &format!("cannot read admitted bytes: {error}"),
            )
        } else {
            CoreError::Storage(format!(
                "lexical: read sealed generation identity {}: {error}",
                path.display()
            ))
        }
    })?;
    crate::channel_payloads::decode_cbor_exact(bytes.as_slice()).map_err(|error| {
        sidecar_corrupt(
            generation_dir,
            LEXICAL_SEALED_IDENTITY_FILE_NAME,
            &format!("cannot decode: {error}"),
        )
    })
}

fn receipt_invalid(path: &Path, reason: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid,
        message: format!("lexical: receipt {}: {reason}", path.display()),
    }
}

pub(crate) fn quarantined_by_scrub_at(
    root: &File,
    generation_dir: &Path,
) -> Result<Option<QuarantinedGenerationV1>, CoreError> {
    let path = generation_dir.join(LEXICAL_QUARANTINE_RECEIPT_FILE_NAME);
    let file_type = crate::sealed_path_io::optional_entry_file_type_at(
        root,
        Path::new(LEXICAL_QUARANTINE_RECEIPT_FILE_NAME),
    )
    .map_err(|error| {
        CoreError::Storage(format!(
            "lexical: inspect quarantine receipt {}: {error}",
            path.display()
        ))
    })?;
    let Some(file_type) = file_type else {
        return Ok(None);
    };
    if file_type != rustix::fs::FileType::RegularFile {
        return Err(receipt_invalid(&path, "is not a regular file"));
    }
    let bytes = crate::sealed_path_io::read_regular_bounded_at(
        root,
        Path::new(LEXICAL_QUARANTINE_RECEIPT_FILE_NAME),
        MAX_RECEIPT_BYTES,
    )
    .map_err(|error| {
        if crate::sealed_path_io::is_unsafe_artifact_path(&error)
            || matches!(
                error.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
            )
        {
            receipt_invalid(&path, &format!("cannot read admitted bytes: {error}"))
        } else {
            CoreError::Storage(format!(
                "lexical: read quarantine receipt {}: {error}",
                path.display()
            ))
        }
    })?;
    let value: CborValue = crate::channel_payloads::decode_cbor_exact(bytes.as_slice())
        .map_err(|error| receipt_invalid(&path, &format!("does not decode: {error}")))?;
    let found = crate::channel_payloads::leading_cbor_array_version(&bytes)
        .map_err(|detail| receipt_invalid(&path, detail))?;
    if found != LEXICAL_QUARANTINE_RECEIPT_FORMAT_VERSION {
        return Err(receipt_invalid(
            &path,
            &format!(
                "has format {found}, this build serves {LEXICAL_QUARANTINE_RECEIPT_FORMAT_VERSION}"
            ),
        ));
    }
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
