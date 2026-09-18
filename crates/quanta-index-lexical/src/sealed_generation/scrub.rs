//! The scrub: re-measure every byte a sealed generation commits to, and
//! record when that was last done.
//!
//! A door proves the index segment files by presence and length only; the
//! seal proved their content once, and between seals only a scrub does. A
//! scrub runs the doors' walk (so it refuses everything a door refuses)
//! and then hashes every segment file against its commitment. A pass is
//! recorded beside the generation in a receipt written by atomic durable
//! rename; the receipt is not query-required, so it is not part of the
//! manifest, and a generation without one is "deep-verified at seal, never
//! scrubbed".

use std::path::{Path, PathBuf};

use ciborium::Value as CborValue;
use quanta_index_contract::GenerationSnapshot;
use quanta_index_core::{
    CoreError, IntegrityScrubReportV1, IntegrityScrubStampV1, IntegrityScrubStatusV1,
    sha256_of_file,
};

use crate::sealed_generation::verify::{DiscardingVisitor, walk_sealed_generation};
use crate::text_authority::leading_format_version;

/// File name of the scrub receipt inside the generation directory.
pub(crate) const LEXICAL_SCRUB_RECEIPT_FILE_NAME: &str = "search-corpus-generation-scrub.cbor";
/// Receipt format: the manifest digest the pass proved, the caller's
/// stamp, and what was measured.
pub(crate) const LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION: u32 = 1;
/// Typed refusal for a receipt this build cannot trust: unreadable,
/// another format, or written for a different sealed digest.
pub(crate) const GENERATION_SCRUB_RECEIPT_INVALID_CODE: &str = "GENERATION_SCRUB_RECEIPT_INVALID";

/// Wire shape of the receipt: a fixed-order CBOR array, format version
/// first.
type ScrubReceiptRow = (u32, String, u64, u64, u64);

pub(crate) fn receipt_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(LEXICAL_SCRUB_RECEIPT_FILE_NAME)
}

/// Re-measure `generation_dir` against the manifest sealed for `identity`
/// and record the pass at `stamp`.
pub(crate) fn scrub_sealed_generation(
    generation_dir: &Path,
    identity: &GenerationSnapshot,
    stamp: IntegrityScrubStampV1,
) -> Result<IntegrityScrubReportV1, CoreError> {
    let verified = walk_sealed_generation(generation_dir, identity, &mut DiscardingVisitor)?;
    let mut files_verified = 0_u64;
    let mut bytes_verified = 0_u64;
    for artifact in verified.manifest.all_commitments() {
        // The walk read and hashed everything but the segment files; those
        // are what the scrub exists for. Their commitments were proved by
        // length there, so hashing them is the only step left.
        if verified
            .manifest
            .index_segments
            .iter()
            .any(|segment| segment.name == artifact.name)
        {
            let path = generation_dir.join(&artifact.name);
            let (bytes, sha256) = sha256_of_file(&path).map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: read {} for scrub: {error}",
                    path.display()
                ))
            })?;
            if bytes != artifact.bytes {
                return Err(crate::sidecar_corrupt(
                    generation_dir,
                    &artifact.name,
                    &format!("{bytes} bytes on disk, {} committed", artifact.bytes),
                ));
            }
            if sha256 != artifact.sha256 {
                return Err(crate::sidecar_corrupt(
                    generation_dir,
                    &artifact.name,
                    "content digest differs from the committed digest",
                ));
            }
        }
        files_verified = files_verified.saturating_add(1);
        bytes_verified = bytes_verified.saturating_add(artifact.bytes);
    }
    let report = IntegrityScrubReportV1 {
        files_verified,
        bytes_verified,
        stamp,
    };
    let row: ScrubReceiptRow = (
        LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION,
        identity.manifest_digest.clone(),
        stamp.unix_ms,
        files_verified,
        bytes_verified,
    );
    let bytes = crate::encode_cbor(&row, "scrub receipt")?;
    crate::write_atomic_durable(&receipt_path(generation_dir), &bytes, "scrub receipt")?;
    Ok(report)
}

fn receipt_invalid(path: &Path, reason: &str) -> CoreError {
    CoreError::Typed {
        code: GENERATION_SCRUB_RECEIPT_INVALID_CODE.to_string(),
        message: format!("lexical: scrub receipt {}: {reason}", path.display()),
    }
}

/// The most recent scrub of the generation sealed for `identity`, or that
/// none has run since the seal.
pub(crate) fn scrub_status(
    generation_dir: &Path,
    identity: &GenerationSnapshot,
) -> Result<IntegrityScrubStatusV1, CoreError> {
    let path = receipt_path(generation_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(IntegrityScrubStatusV1::DeepVerifiedAtSeal);
        }
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "lexical: read scrub receipt {}: {error}",
                path.display()
            )));
        }
    };
    let value: CborValue = ciborium::from_reader(bytes.as_slice())
        .map_err(|error| receipt_invalid(&path, &format!("does not decode: {error}")))?;
    let format_version = leading_format_version(&value, "scrub receipt", &path)?;
    if format_version != LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION {
        return Err(receipt_invalid(
            &path,
            &format!(
                "has format {format_version}, this build serves {LEXICAL_SCRUB_RECEIPT_FORMAT_VERSION}"
            ),
        ));
    }
    let (_format, manifest_digest, unix_ms, files_verified, bytes_verified): ScrubReceiptRow =
        value
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
    Ok(IntegrityScrubStatusV1::ScrubVerifiedSince(
        IntegrityScrubReportV1 {
            files_verified,
            bytes_verified,
            stamp: IntegrityScrubStampV1 { unix_ms },
        },
    ))
}
