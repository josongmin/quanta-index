//! Canonical compact coverage root and committed page naming, without I/O.
use super::names::SOURCE_FILE_COVERAGE_FILE_NAME;
use quanta_index_contract::{
    FileCoverageSnapshot as CoverageSnapshot, GenerationSnapshot, SourceFileCoverage,
    SourcePublicationEvent,
};
use quanta_index_core::CoreError;
use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, de};
use std::{io::Cursor, marker::PhantomData, path::Path};
pub(crate) const COVERAGE_FORMAT: u32 = 2;
pub(crate) const MAX_COVERAGE_ROOT_BYTES: usize = 32 * 1024;
#[cfg(feature = "legacy-tantivy")]
pub(crate) const MAX_COVERAGE_PAGE_BYTES: usize = 1024 * 1024;
#[cfg(feature = "legacy-tantivy")]
pub(crate) const MAX_COVERAGE_PAGE_ROWS: usize = 4096;
pub(crate) const MAX_COVERAGE_ROOT_BYTES_U64: u64 = 32 * 1024;
pub(crate) const MAX_COVERAGE_PAGE_BYTES_U64: u64 = 1024 * 1024;
pub(crate) const MAX_COVERAGE_PAGE_ROWS_U32: u32 = 4096;
pub(crate) const MAX_COVERAGE_ENCODED_BYTES: u64 = crate::FILE_COVERAGE_ENCODED_BYTES_LIMIT;
pub(crate) const MAX_COVERAGE_DECODE_HEAP: u64 = crate::FILE_COVERAGE_DECODE_HEAP_BYTES_LIMIT;
pub(crate) const PAGE_PREFIX: &str = "source-file-coverage-page-";

pub(crate) type PageRow = (u8, u64, [u8; 32], u32);
pub(crate) type CoverageRow = (
    u32,
    GenerationSnapshot,
    SourcePublicationEvent,
    BoundedRows<PageRow, 256>,
);
pub(crate) struct BoundedRows<T, const MAX: usize>(pub(crate) Vec<T>);

impl<T, const MAX: usize> std::ops::Deref for BoundedRows<T, MAX> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T, const MAX: usize> IntoIterator for BoundedRows<T, MAX> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'de, T: Deserialize<'de>, const MAX: usize> Deserialize<'de> for BoundedRows<T, MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowsVisitor<T, const MAX: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const MAX: usize> Visitor<'de> for RowsVisitor<T, MAX> {
            type Value = BoundedRows<T, MAX>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "at most {MAX} coverage rows")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut rows = Vec::new();
                if let Some(length) = access.size_hint() {
                    if length > MAX {
                        return Err(de::Error::custom("coverage row count exceeds its ceiling"));
                    }
                    rows.try_reserve_exact(length).map_err(de::Error::custom)?;
                }
                while rows.len() < MAX {
                    let Some(row) = access.next_element()? else {
                        return Ok(BoundedRows(rows));
                    };
                    // Indefinite CBOR arrays have no length hint. Grow
                    // geometrically within MAX instead of reallocating for
                    // every decoded row.
                    rows.try_reserve(1).map_err(de::Error::custom)?;
                    rows.push(row);
                }
                if access.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("coverage row count exceeds its ceiling"));
                }
                Ok(BoundedRows(rows))
            }
        }
        deserializer.deserialize_seq(RowsVisitor::<T, MAX>(PhantomData))
    }
}

pub(crate) fn corrupt(generation_dir: &Path, reason: &str) -> CoreError {
    super::errors::sidecar_corrupt(generation_dir, SOURCE_FILE_COVERAGE_FILE_NAME, reason)
}
pub(crate) fn resource(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message: format!("lexical: coverage resource envelope: {reason}"),
    }
}
pub(crate) fn page_name(slot: u8, digest: &[u8; 32]) -> String {
    fn hex_digit(nibble: u8) -> char {
        char::from(if nibble < 10 {
            b'0'.saturating_add(nibble)
        } else {
            b'a'.saturating_add(nibble.saturating_sub(10))
        })
    }
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push(hex_digit(byte >> 4));
        hex.push(hex_digit(byte & 0x0f));
    }
    format!("{PAGE_PREFIX}{slot:02x}-{hex}.cbor")
}
pub(crate) fn has_length(bytes: &[u8], expected: u64) -> bool {
    u64::try_from(bytes.len()) == Ok(expected)
}
pub(crate) fn is_coverage_page(name: &str) -> bool {
    name.starts_with(PAGE_PREFIX)
}
pub(crate) fn decode_root(bytes: &[u8], directory: &Path) -> Result<(CoverageRow, u64), CoreError> {
    if bytes.len() > MAX_COVERAGE_ROOT_BYTES {
        return Err(corrupt(directory, "coverage root exceeds its byte ceiling"));
    }
    let mut reader = Cursor::new(bytes);
    let root: CoverageRow = ciborium::from_reader(&mut reader)
        .map_err(|error| corrupt(directory, &format!("decode coverage root: {error}")))?;
    if !has_length(bytes, reader.position()) || root.0 != COVERAGE_FORMAT {
        return Err(corrupt(
            directory,
            "unsupported coverage format or trailing bytes",
        ));
    }
    root.2
        .validate()
        .map_err(|error| corrupt(directory, &error.to_string()))?;
    if root.3.len() > 256 {
        return Err(corrupt(directory, "too many coverage partitions"));
    }
    let mut previous = None;
    let mut total_bytes = 0_u64;
    let mut total_rows = 0_u64;
    for (slot, bytes, _, rows) in root.3.iter() {
        if previous.is_some_and(|previous| previous >= *slot)
            || *bytes == 0
            || *bytes > MAX_COVERAGE_PAGE_BYTES_U64
            || *rows == 0
            || *rows > MAX_COVERAGE_PAGE_ROWS_U32
        {
            return Err(corrupt(
                directory,
                "invalid, duplicate or reordered coverage partition",
            ));
        }
        previous = Some(*slot);
        total_bytes = total_bytes
            .checked_add(*bytes)
            .ok_or_else(|| resource("coverage encoded length overflow"))?;
        total_rows = total_rows
            .checked_add(u64::from(*rows))
            .ok_or_else(|| resource("coverage row count overflow"))?;
    }
    let heap_charge = decode_heap_charge(total_rows, total_bytes)?;
    if total_bytes > MAX_COVERAGE_ENCODED_BYTES || heap_charge > MAX_COVERAGE_DECODE_HEAP {
        return Err(resource(&format!(
            "effective coverage exceeds supported decode residency: rows={total_rows}, encoded_bytes={total_bytes}/{MAX_COVERAGE_ENCODED_BYTES}, decode_heap_bytes={heap_charge}/{MAX_COVERAGE_DECODE_HEAP}"
        )));
    }
    Ok((root, heap_charge))
}
pub(crate) fn decode_heap_charge(total_rows: u64, total_bytes: u64) -> Result<u64, CoreError> {
    let structural = CoverageSnapshot::structural_heap_bytes_bound(total_rows)
        .ok_or_else(|| resource("coverage structural heap charge overflow"))?;
    // Keys and row strings have two owning copies; trees share Arc keys/rows.
    // Keep the existing 8x encoded-byte charge for string buffers, raw pages and
    // decoder transients. The bounded page Vec additionally holds inline rows
    // before they move into shared allocations; reserve two page capacities.
    // A malformed page can declare MAX rows even when the authenticated root
    // commits one row. Admission must cover that reservation before the decoder
    // checks the root count, including indefinite arrays growing to the ceiling.
    let page_rows = if total_rows == 0 {
        0
    } else {
        u64::from(MAX_COVERAGE_PAGE_ROWS_U32)
    };
    let inline_row_bytes = u64::try_from(std::mem::size_of::<SourceFileCoverage>())
        .map_err(|error| resource(&format!("coverage row width: {error}")))?;
    let page_rows = page_rows
        .checked_mul(inline_row_bytes)
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or_else(|| resource("coverage page row charge overflow"))?;
    structural
        .checked_add(page_rows)
        // The bounded root retains its publication strings and page inventory
        // while one page is decoded. Its fixed maximum is separate from pages.
        .and_then(|charge| {
            MAX_COVERAGE_ROOT_BYTES_U64
                .checked_mul(8)
                .and_then(|bytes| charge.checked_add(bytes))
        })
        .and_then(|charge| {
            total_bytes
                .checked_mul(8)
                .and_then(|bytes| charge.checked_add(bytes))
        })
        .ok_or_else(|| resource("coverage decode charge overflow"))
}

pub(crate) fn page_commitments(
    bytes: &[u8],
    directory: &Path,
    expected: &GenerationSnapshot,
) -> Result<Vec<quanta_index_core::SealedArtifactCommitmentV1>, CoreError> {
    let ((_, identity, _, pages), _) = decode_root(bytes, directory)?;
    if identity != *expected {
        return Err(corrupt(
            directory,
            "coverage root belongs to another generation",
        ));
    }
    let commitments: Vec<_> = pages
        .into_iter()
        .map(
            |(slot, bytes, sha256, _)| quanta_index_core::SealedArtifactCommitmentV1 {
                name: page_name(slot, &sha256),
                bytes,
                sha256,
            },
        )
        .collect();
    Ok(commitments)
}
