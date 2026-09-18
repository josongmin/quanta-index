//! The persisted ledger manifest of one embedding cache namespace
//! (QI-BB-009).
//!
//! Opening a namespace used to `stat` every entry file to learn its bytes
//! and write time, so boot cost grew with the cache. The manifest is the
//! ledger's own record of those two facts, written whole (temporary file,
//! `fsync`, rename) every [`MANIFEST_FLUSH_EVERY_PUTS`] writes and when
//! the store is dropped. Opening reads it once, lists the entry files
//! without touching their metadata, and `stat`s only the entries the
//! manifest does not cover — those written since the last flush, or every
//! entry after a crash before the first flush — counting each one so the
//! bounded rebuild is visible in the open report and the scrape.
//!
//! The manifest is advisory accounting, never authority for a vector: a
//! listed entry that is missing on disk is dropped, an unlisted entry on
//! disk is admitted after a `stat`, and a manifest that does not decode is
//! ignored as if absent. Every vector a reader is served still decodes and
//! verifies its own digest.

use std::collections::BTreeMap;
use std::path::Path;

use super::EmbeddingCacheKey;

/// The manifest's file name inside a namespace directory.
pub(super) const MANIFEST_FILE: &str = "ledger.manifest";
/// Magic prefix of a manifest.
const MANIFEST_MAGIC: &[u8; 4] = b"QIEM";
/// Manifest format this crate writes and reads.
pub(super) const MANIFEST_FORMAT_VERSION: u16 = 1;
/// Writes between two manifest flushes; a crash loses at most this many
/// entries' accounting to a `stat` at the next open.
pub(super) const MANIFEST_FLUSH_EVERY_PUTS: u64 = 4_096;
/// Bytes of one key in the manifest.
const KEY_LEN: usize = 32;
/// Bytes of one manifest record: key, entry bytes, write time.
const RECORD_LEN: usize = KEY_LEN + 8 + 8;
const HEADER_LEN: usize = MANIFEST_MAGIC.len() + 2 + 8;

/// What the manifest records about one entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ManifestRecord {
    pub(super) bytes: u64,
    pub(super) written_nanos: u64,
}

/// Encode `records` as one manifest.
pub(super) fn encode_manifest(records: &BTreeMap<EmbeddingCacheKey, ManifestRecord>) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(HEADER_LEN.saturating_add(records.len().saturating_mul(RECORD_LEN)));
    out.extend_from_slice(MANIFEST_MAGIC);
    out.extend_from_slice(&MANIFEST_FORMAT_VERSION.to_le_bytes());
    let count = u64::try_from(records.len()).map_or(u64::MAX, |count| count);
    out.extend_from_slice(&count.to_le_bytes());
    for (key, record) in records {
        out.extend_from_slice(key.as_bytes());
        out.extend_from_slice(&record.bytes.to_le_bytes());
        out.extend_from_slice(&record.written_nanos.to_le_bytes());
    }
    out
}

/// Why a manifest did not decode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ManifestDecodeError {
    /// Fewer bytes than a header, a record, or the declared record count.
    Truncated,
    /// Not this crate's magic.
    ForeignMagic,
    /// A format version this crate does not read.
    UnknownFormat(u16),
    /// More record bytes than the header declared.
    TrailingBytes,
    /// One key listed twice.
    DuplicateKey,
}

impl core::fmt::Display for ManifestDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Truncated => f.write_str("manifest is truncated"),
            Self::ForeignMagic => f.write_str("manifest does not carry this crate's magic"),
            Self::UnknownFormat(format) => write!(f, "manifest format {format} is not read"),
            Self::TrailingBytes => f.write_str("manifest has bytes past its declared records"),
            Self::DuplicateKey => f.write_str("manifest lists one key twice"),
        }
    }
}

impl std::error::Error for ManifestDecodeError {}

/// Exactly `N` bytes from the front of `bytes`, and the rest.
fn take_array<const N: usize>(bytes: &[u8]) -> Result<([u8; N], &[u8]), ManifestDecodeError> {
    let (head, rest) = bytes
        .split_at_checked(N)
        .ok_or(ManifestDecodeError::Truncated)?;
    let mut array = [0_u8; N];
    array.copy_from_slice(head);
    Ok((array, rest))
}

/// Decode one manifest, refusing anything that is not exactly a manifest
/// of this format with every record present.
pub(super) fn decode_manifest(
    bytes: &[u8],
) -> Result<BTreeMap<EmbeddingCacheKey, ManifestRecord>, ManifestDecodeError> {
    let (magic, rest) = take_array::<{ MANIFEST_MAGIC.len() }>(bytes)?;
    if &magic != MANIFEST_MAGIC {
        return Err(ManifestDecodeError::ForeignMagic);
    }
    let (format, rest) = take_array::<2>(rest)?;
    let format = u16::from_le_bytes(format);
    if format != MANIFEST_FORMAT_VERSION {
        return Err(ManifestDecodeError::UnknownFormat(format));
    }
    let (count, mut rest) = take_array::<8>(rest)?;
    let count = usize::try_from(u64::from_le_bytes(count))
        .map_err(|_wider_than_this_platform| ManifestDecodeError::Truncated)?;
    let declared = count
        .checked_mul(RECORD_LEN)
        .ok_or(ManifestDecodeError::Truncated)?;
    if rest.len() < declared {
        return Err(ManifestDecodeError::Truncated);
    }
    if rest.len() > declared {
        return Err(ManifestDecodeError::TrailingBytes);
    }
    let mut records = BTreeMap::new();
    for _ in 0..count {
        let (key, tail) = take_array::<KEY_LEN>(rest)?;
        let (entry_bytes, tail) = take_array::<8>(tail)?;
        let (written, tail) = take_array::<8>(tail)?;
        rest = tail;
        let record = ManifestRecord {
            bytes: u64::from_le_bytes(entry_bytes),
            written_nanos: u64::from_le_bytes(written),
        };
        if records
            .insert(EmbeddingCacheKey::from_bytes(key), record)
            .is_some()
        {
            return Err(ManifestDecodeError::DuplicateKey);
        }
    }
    Ok(records)
}

/// The manifest of `namespace_dir`: present and decoded, absent, or
/// malformed (unreadable, or not a manifest of this format).
pub(super) fn read_manifest(namespace_dir: &Path) -> ManifestRead {
    let path = namespace_dir.join(MANIFEST_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => match decode_manifest(&bytes) {
            Ok(records) => ManifestRead::Present(records),
            Err(_malformed) => ManifestRead::Malformed,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ManifestRead::Absent,
        Err(_unreadable) => ManifestRead::Malformed,
    }
}

/// What reading a namespace's manifest found.
pub(super) enum ManifestRead {
    Present(BTreeMap<EmbeddingCacheKey, ManifestRecord>),
    Absent,
    /// Present but not a manifest of this format, or unreadable; treated
    /// as absent and counted.
    Malformed,
}

#[cfg(test)]
mod tests {
    use super::{ManifestDecodeError, ManifestRecord, decode_manifest, encode_manifest};
    use crate::cache::EmbeddingCacheKey;
    use std::collections::BTreeMap;

    #[test]
    fn a_manifest_round_trips_and_refuses_a_truncated_or_foreign_one() {
        let mut records = BTreeMap::new();
        for index in 0..5_u8 {
            let _fresh = records.insert(
                EmbeddingCacheKey::from_bytes([index; 32]),
                ManifestRecord {
                    bytes: u64::from(index) * 100,
                    written_nanos: 1_700_000_000 + u64::from(index),
                },
            );
        }
        let encoded = encode_manifest(&records);
        assert_eq!(decode_manifest(&encoded).as_ref(), Ok(&records));
        let truncated = encoded.get(..encoded.len() - 1).expect("shorter");
        assert_eq!(
            decode_manifest(truncated),
            Err(ManifestDecodeError::Truncated)
        );
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert_eq!(
            decode_manifest(&trailing),
            Err(ManifestDecodeError::TrailingBytes)
        );
        let mut foreign = encoded.clone();
        if let Some(first) = foreign.first_mut() {
            *first = b'X';
        }
        assert_eq!(
            decode_manifest(&foreign),
            Err(ManifestDecodeError::ForeignMagic)
        );
        let mut future = encoded.clone();
        if let Some(format) = future.get_mut(4) {
            *format = 9;
        }
        assert_eq!(
            decode_manifest(&future),
            Err(ManifestDecodeError::UnknownFormat(9))
        );
        assert_eq!(decode_manifest(&[]), Err(ManifestDecodeError::Truncated));
        assert_eq!(
            decode_manifest(&encode_manifest(&BTreeMap::new())),
            Ok(BTreeMap::new())
        );
    }
}
