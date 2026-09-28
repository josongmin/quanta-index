//! One generation root commits bounded, immutable hash-routed coverage pages.
//! Unchanged pages are linked from the authenticated base, never reserialized.

use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, de};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Cursor, Read as _, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use quanta_index_contract::{GenerationSnapshot, SourceFileCoverage, SourcePublicationEvent};
use quanta_index_core::{CoreError, SealedArtifactCommitmentV1};
use sha2::{Digest as _, Sha256};

use super::{CoverageArtifact, CoverageSnapshot, SOURCE_FILE_COVERAGE_FILE_NAME, corrupt};

pub(crate) const COVERAGE_FORMAT: u32 = 2;
pub(crate) const MAX_COVERAGE_ROOT_BYTES: usize = 32 * 1024;
pub(crate) const MAX_COVERAGE_PAGE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_COVERAGE_PAGE_ROWS: usize = 4096;
pub(crate) const MAX_COVERAGE_ROOT_BYTES_U64: u64 = 32 * 1024;
const MAX_COVERAGE_PAGE_BYTES_U64: u64 = 1024 * 1024;
const MAX_COVERAGE_PAGE_ROWS_U32: u32 = 4096;
const MAX_COVERAGE_ENCODED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_COVERAGE_DECODE_HEAP: u64 = 256 * 1024 * 1024;
const PAGE_PREFIX: &str = "source-file-coverage-page-";

// Slot, encoded bytes, content hash, row count. Root ordering is canonical.
type PageRow = (u8, u64, [u8; 32], u32);
type CoverageRow = (
    u32,
    GenerationSnapshot,
    SourcePublicationEvent,
    BoundedRows<PageRow, 256>,
);
type PageBody<'a> = (u32, u8, Vec<&'a SourceFileCoverage>);
type DecodedPage = (
    u32,
    u8,
    BoundedRows<SourceFileCoverage, MAX_COVERAGE_PAGE_ROWS>,
);

// Refuse cardinality before trusting serialized length hints or growing buffers.
struct BoundedRows<T, const MAX: usize>(Vec<T>);

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
                if access.size_hint().is_some_and(|length| length > MAX) {
                    return Err(de::Error::custom("coverage row count exceeds its ceiling"));
                }
                let mut rows = Vec::new();
                while rows.len() < MAX {
                    let Some(row) = access.next_element()? else {
                        return Ok(BoundedRows(rows));
                    };
                    rows.try_reserve_exact(1).map_err(de::Error::custom)?;
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

pub(crate) struct CoverageWriteBase {
    pub(crate) directory: PathBuf,
    pub(crate) root: SealedArtifactCommitmentV1,
}

pub(crate) struct CoveragePlan {
    pub(super) coverage: CoverageSnapshot,
    pub(super) base: Option<CoverageWriteBase>,
    pub(super) touched: BTreeSet<u8>,
}

impl CoveragePlan {
    pub(crate) fn snapshot(&self) -> &CoverageSnapshot {
        &self.coverage
    }
}

fn resource(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message: format!("lexical: coverage resource envelope: {reason}"),
    }
}

struct BoundedBytes {
    bytes: Vec<u8>,
    ceiling: usize,
}

impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|len| len > self.ceiling)
        {
            return Err(std::io::Error::other(
                "coverage encoded byte ceiling exceeded",
            ));
        }
        self.bytes
            .try_reserve_exact(bytes.len())
            .map_err(std::io::Error::other)?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encode(value: &impl serde::Serialize, ceiling: usize) -> Result<Vec<u8>, CoreError> {
    let mut output = BoundedBytes {
        bytes: Vec::new(),
        ceiling,
    };
    ciborium::into_writer(value, &mut output)
        .map_err(|error| resource(&format!("bounded encoding failed: {error}")))?;
    Ok(output.bytes)
}

fn read_bounded(path: &Path, ceiling: usize) -> Result<Vec<u8>, CoreError> {
    let directory = path.parent().unwrap_or(path);
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            corrupt(directory, "missing committed coverage artifact")
        } else {
            CoreError::Storage(format!("inspect {}: {error}", path.display()))
        }
    })?;
    if !metadata.is_file() {
        return Err(corrupt(
            directory,
            "coverage artifact is not a regular file",
        ));
    }
    let ceiling_u64 = u64::try_from(ceiling).map_err(|error| {
        resource(&format!(
            "coverage byte ceiling is not representable: {error}"
        ))
    })?;
    if metadata.len() > ceiling_u64 {
        return Err(resource("encoded artifact exceeds its byte ceiling"));
    }
    let mut file = File::open(path)
        .map_err(|error| CoreError::Storage(format!("open {}: {error}", path.display())))?;
    let opened = file
        .metadata()
        .map_err(|error| CoreError::Storage(error.to_string()))?;
    if !opened.is_file() {
        return Err(corrupt(
            directory,
            "opened coverage artifact is not a regular file",
        ));
    }
    if opened.len() > ceiling_u64 {
        return Err(resource("encoded artifact exceeds its byte ceiling"));
    }
    let expected_len = usize::try_from(opened.len())
        .map_err(|error| resource(&format!("coverage read length overflow: {error}")))?;
    // Reserve the opened file's actual length instead of the page ceiling for
    // every small page. read_exact plus one byte detects a concurrent change
    // without allowing Vec's geometric growth to exceed the admitted length.
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(expected_len)
        .map_err(|error| resource(&format!("coverage read allocation refused: {error}")))?;
    bytes.resize(expected_len, 0);
    file.read_exact(&mut bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            corrupt(directory, "coverage artifact changed during read")
        } else {
            CoreError::Storage(format!("read {}: {error}", path.display()))
        }
    })?;
    let mut extra = [0_u8; 1];
    if file
        .read(&mut extra)
        .map_err(|error| CoreError::Storage(format!("read {}: {error}", path.display())))?
        != 0
    {
        return Err(corrupt(directory, "coverage artifact changed during read"));
    }
    Ok(bytes)
}

fn page_name(slot: u8, digest: &[u8; 32]) -> String {
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

fn has_length(bytes: &[u8], expected: u64) -> bool {
    u64::try_from(bytes.len()) == Ok(expected)
}

pub(crate) fn is_coverage_page(name: &str) -> bool {
    name.starts_with(PAGE_PREFIX)
}

fn decode_root(bytes: &[u8], directory: &Path) -> Result<CoverageRow, CoreError> {
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
    // Two derived tree indexes, shared row allocations, decode temporaries and
    // compacted strings. This is conservative admission, not measured RSS.
    let row_charge = std::mem::size_of::<quanta_index_contract::SourceFileKey>()
        .checked_add(std::mem::size_of::<SourceFileCoverage>())
        .and_then(|size| size.checked_add(32))
        .and_then(|size| size.checked_mul(32))
        .ok_or_else(|| resource("coverage row charge overflow"))?;
    let row_charge = u64::try_from(row_charge).map_err(|error| {
        resource(&format!(
            "coverage row charge is not representable: {error}"
        ))
    })?;
    let heap_charge = total_rows
        .checked_mul(row_charge)
        .and_then(|charge| {
            total_bytes
                .checked_mul(8)
                .and_then(|bytes| charge.checked_add(bytes))
        })
        .ok_or_else(|| resource("coverage decode charge overflow"))?;
    if total_bytes > MAX_COVERAGE_ENCODED_BYTES || heap_charge > MAX_COVERAGE_DECODE_HEAP {
        return Err(resource(
            "effective coverage exceeds supported decode residency",
        ));
    }
    Ok(root)
}

pub(crate) fn root_page_commitments(
    directory: &Path,
    root: &SealedArtifactCommitmentV1,
    expected: &GenerationSnapshot,
) -> Result<Vec<SealedArtifactCommitmentV1>, CoreError> {
    if root.name != SOURCE_FILE_COVERAGE_FILE_NAME || root.bytes > MAX_COVERAGE_ROOT_BYTES_U64 {
        return Err(corrupt(directory, "invalid coverage root commitment"));
    }
    let bytes = read_bounded(&directory.join(&root.name), MAX_COVERAGE_ROOT_BYTES)?;
    if !has_length(&bytes, root.bytes) || <[u8; 32]>::from(Sha256::digest(&bytes)) != root.sha256 {
        return Err(corrupt(
            directory,
            "coverage root differs from its commitment",
        ));
    }
    let (_, identity, _, pages) = decode_root(&bytes, directory)?;
    if identity != *expected {
        return Err(corrupt(
            directory,
            "coverage root belongs to another generation",
        ));
    }
    let commitments: Vec<_> = pages
        .into_iter()
        .map(|(slot, bytes, sha256, _)| SealedArtifactCommitmentV1 {
            name: page_name(slot, &sha256),
            bytes,
            sha256,
        })
        .collect();
    let names: BTreeSet<_> = commitments.iter().map(|page| page.name.as_str()).collect();
    for entry in
        std::fs::read_dir(directory).map_err(|error| CoreError::Storage(error.to_string()))?
    {
        let entry = entry.map_err(|error| CoreError::Storage(error.to_string()))?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if is_coverage_page(&name) && !names.contains(name.as_ref()) {
            return Err(corrupt(directory, "uncommitted coverage page"));
        }
    }
    Ok(commitments)
}

pub(crate) fn decode_coverage_pages(
    bytes: &[u8],
    directory: &Path,
    expected: &GenerationSnapshot,
) -> Result<CoverageArtifact, CoreError> {
    decode_coverage_pages_impl(bytes, directory, expected, false)
}

pub(crate) fn decode_staged_coverage_pages(
    bytes: &[u8],
    directory: &Path,
    expected: &GenerationSnapshot,
) -> Result<CoverageArtifact, CoreError> {
    // A crash on either side of the root rename can leave pages from the
    // other version. Only the unsealed retry path may ignore those pages;
    // the caller still compares the decoded event and complete snapshot.
    decode_coverage_pages_impl(bytes, directory, expected, true)
}

fn decode_coverage_pages_impl(
    bytes: &[u8],
    directory: &Path,
    expected: &GenerationSnapshot,
    allow_orphans: bool,
) -> Result<CoverageArtifact, CoreError> {
    let (_, identity, publication, pages) = decode_root(bytes, directory)?;
    if identity != *expected {
        return Err(corrupt(
            directory,
            "coverage belongs to another generation identity",
        ));
    }
    let mut coverage = CoverageSnapshot::new();
    let mut names = BTreeSet::new();
    for (slot, length, digest, count) in pages {
        let name = page_name(slot, &digest);
        let _inserted = names.insert(name.clone());
        let raw = read_bounded(&directory.join(&name), MAX_COVERAGE_PAGE_BYTES)?;
        if !has_length(&raw, length) || <[u8; 32]>::from(Sha256::digest(&raw)) != digest {
            return Err(corrupt(
                directory,
                "coverage page differs from its commitment",
            ));
        }
        let mut reader = Cursor::new(&raw);
        let (format, found_slot, rows): DecodedPage = ciborium::from_reader(&mut reader)
            .map_err(|error| corrupt(directory, &format!("decode coverage page: {error}")))?;
        if format != COVERAGE_FORMAT
            || found_slot != slot
            || u32::try_from(rows.len()) != Ok(count)
            || !has_length(&raw, reader.position())
        {
            return Err(corrupt(
                directory,
                "coverage page format/slot/count/trailing bytes differ",
            ));
        }
        let mut previous = None;
        for row in rows {
            row.validate()
                .map_err(|error| corrupt(directory, &error.to_string()))?;
            let key = row.source.file.clone();
            if CoverageSnapshot::partition_for(&key) != slot
                || previous.as_ref().is_some_and(|previous| previous >= &key)
            {
                return Err(corrupt(
                    directory,
                    "duplicate, reordered or misrouted coverage row",
                ));
            }
            previous = Some(key.clone());
            let _previous = coverage.insert(key, row);
        }
    }
    if !allow_orphans {
        // A sealed generation cannot silently retain a second coverage universe.
        for entry in
            std::fs::read_dir(directory).map_err(|error| CoreError::Storage(error.to_string()))?
        {
            let entry = entry.map_err(|error| CoreError::Storage(error.to_string()))?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if is_coverage_page(&name) && !names.contains(name.as_ref()) {
                return Err(corrupt(directory, "uncommitted coverage page"));
            }
        }
    }
    Ok(CoverageArtifact {
        coverage,
        publication,
    })
}

pub(crate) fn read_coverage_root(directory: &Path) -> Result<Option<Vec<u8>>, CoreError> {
    let path = directory.join(SOURCE_FILE_COVERAGE_FILE_NAME);
    match std::fs::symlink_metadata(&path) {
        // A crash before root publication can leave candidate pages. They have
        // no authority in an unsealed directory; a retry will re-plan and the
        // page writer removes every orphan after publishing the new root.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CoreError::Storage(format!(
            "inspect {}: {error}",
            path.display()
        ))),
        Ok(_) => read_bounded(&path, MAX_COVERAGE_ROOT_BYTES).map(Some),
    }
}

pub(crate) fn write_coverage_pages(
    directory: &Path,
    identity: &GenerationSnapshot,
    publication: &SourcePublicationEvent,
    snapshot: &CoverageSnapshot,
    base: Option<&CoverageWriteBase>,
    touched: &BTreeSet<u8>,
) -> Result<SealedArtifactCommitmentV1, CoreError> {
    publication
        .validate()
        .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
    let mut inherited = BTreeMap::new();
    if let Some(base) = base {
        let raw = read_bounded(
            &base.directory.join(&base.root.name),
            MAX_COVERAGE_ROOT_BYTES,
        )?;
        if !has_length(&raw, base.root.bytes)
            || <[u8; 32]>::from(Sha256::digest(&raw)) != base.root.sha256
        {
            return Err(corrupt(
                &base.directory,
                "coverage base root changed after admission",
            ));
        }
        let root = decode_root(&raw, &base.directory)?;
        inherited.extend(root.3.into_iter().map(|row| (row.0, row)));
    }
    let mut pages = Vec::new();
    let mut encoded = BTreeMap::new();
    let mut encoded_bytes = 0_u64;
    for slot in 0_u8..=255 {
        if base.is_some() && !touched.contains(&slot) {
            if let Some(row) = inherited.get(&slot) {
                pages.push(*row);
            }
            continue;
        }
        let entries: Vec<_> = snapshot
            .partition(slot)
            .take(MAX_COVERAGE_PAGE_ROWS + 1)
            .collect();
        for (key, row) in &entries {
            if *key != &row.source.file {
                return Err(corrupt(
                    directory,
                    "coverage map key differs from file owner",
                ));
            }
        }
        let rows: Vec<_> = entries.iter().map(|(_, row)| *row).collect();
        if rows.len() > MAX_COVERAGE_PAGE_ROWS {
            return Err(resource("coverage partition row ceiling exceeded"));
        }
        if rows.is_empty() {
            continue;
        }
        for row in &rows {
            row.validate()
                .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        }
        let count = u32::try_from(rows.len()).map_err(|error| {
            resource(&format!("coverage partition row count overflow: {error}"))
        })?;
        let body: PageBody<'_> = (COVERAGE_FORMAT, slot, rows);
        let raw = encode(&body, MAX_COVERAGE_PAGE_BYTES)?;
        let raw_len = u64::try_from(raw.len())
            .map_err(|error| resource(&format!("coverage page length overflow: {error}")))?;
        encoded_bytes = encoded_bytes
            .checked_add(raw_len)
            .ok_or_else(|| resource("encoded coverage length overflow"))?;
        if encoded_bytes > MAX_COVERAGE_ENCODED_BYTES {
            return Err(resource(
                "new coverage pages exceed the encoded byte ceiling",
            ));
        }
        let digest = <[u8; 32]>::from(Sha256::digest(&raw));
        pages.push((slot, raw_len, digest, count));
        let _previous = encoded.insert(page_name(slot, &digest), raw);
    }
    let root = encode(
        &(COVERAGE_FORMAT, identity, publication, pages),
        MAX_COVERAGE_ROOT_BYTES,
    )?;
    let root_commitment = SealedArtifactCommitmentV1 {
        name: SOURCE_FILE_COVERAGE_FILE_NAME.into(),
        bytes: u64::try_from(root.len())
            .map_err(|error| resource(&format!("coverage root length overflow: {error}")))?,
        sha256: Sha256::digest(&root).into(),
    };
    let (_, _, _, pages) = decode_root(&root, directory)?;
    let planned_rows: u64 = pages.iter().map(|(_, _, _, rows)| u64::from(*rows)).sum();
    if u64::try_from(snapshot.len()) != Ok(planned_rows) {
        return Err(corrupt(
            directory,
            "coverage plan changed an unadmitted partition",
        ));
    }
    // All capacity/identity admission above precedes the first target mutation.
    std::fs::create_dir_all(directory).map_err(|error| CoreError::Storage(error.to_string()))?;
    let mut names = BTreeSet::new();
    for (slot, length, digest, _) in pages {
        let name = page_name(slot, &digest);
        let _inserted = names.insert(name.clone());
        let target = directory.join(&name);
        if target.exists() {
            let raw = read_bounded(&target, MAX_COVERAGE_PAGE_BYTES)?;
            if !has_length(&raw, length) || <[u8; 32]>::from(Sha256::digest(&raw)) != digest {
                return Err(corrupt(directory, "pre-existing coverage page differs"));
            }
        } else if let Some(raw) = encoded.get(&name) {
            crate::index_store::write_atomic_durable(&target, raw, "coverage page")?;
        } else if let Some(base) = base {
            std::fs::hard_link(base.directory.join(&name), &target).map_err(|error| {
                CoreError::Storage(format!("inherit coverage page {name}: {error}"))
            })?;
        } else {
            return Err(corrupt(directory, "coverage page has no admitted source"));
        }
    }
    // Publish the new root while every page named by the old root still
    // exists. A crash before the rename leaves the old root readable; a
    // crash after it leaves the new root readable. The retry cleans orphans.
    crate::index_store::write_atomic_durable(
        &directory.join(SOURCE_FILE_COVERAGE_FILE_NAME),
        &root,
        "coverage root",
    )?;
    let mut removed_orphan = false;
    for entry in
        std::fs::read_dir(directory).map_err(|error| CoreError::Storage(error.to_string()))?
    {
        let entry = entry.map_err(|error| CoreError::Storage(error.to_string()))?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if is_coverage_page(&name) && !names.contains(name.as_ref()) {
            std::fs::remove_file(entry.path()).map_err(|error| {
                CoreError::Storage(format!("remove orphan coverage page {name}: {error}"))
            })?;
            removed_orphan = true;
        }
    }
    if removed_orphan {
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                CoreError::Storage(format!(
                    "fsync coverage orphan cleanup {}: {error}",
                    directory.display()
                ))
            })?;
    }
    Ok(root_commitment)
}

#[cfg(test)]
mod tests {
    use super::{BoundedRows, MAX_COVERAGE_PAGE_BYTES, read_bounded};

    #[test]
    fn small_coverage_page_does_not_allocate_the_page_ceiling()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("page");
        std::fs::write(&path, [7_u8; 64])?;
        let bytes = read_bounded(&path, MAX_COVERAGE_PAGE_BYTES)?;
        if bytes.as_slice() != [7_u8; 64]
            || bytes.capacity().saturating_mul(2) >= MAX_COVERAGE_PAGE_BYTES
        {
            return Err("small coverage page used the maximum page reserve".into());
        }
        Ok(())
    }

    #[test]
    fn serialized_cardinality_hints_cannot_allocate_unbounded_vectors() {
        // CBOR array declaring 2^64-1 entries, with no body. Refuse the hint
        // before attempting element deserialization or allocating its capacity.
        let declared = [0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        let hint = ciborium::from_reader::<BoundedRows<u8, 256>, _>(&declared[..]);
        assert!(hint.is_err());
        // Indefinite arrays have no trustworthy hint; enforce the same cap.
        let declared = [0x9f, 0x01, 0x02, 0x03, 0xff];
        assert!(ciborium::from_reader::<BoundedRows<u8, 2>, _>(&declared[..]).is_err());
        let allowed = [0x82, 0x01, 0x02];
        let rows = ciborium::from_reader::<BoundedRows<u8, 2>, _>(&allowed[..]);
        assert!(matches!(rows, Ok(rows) if rows.0 == vec![1, 2]));
    }
}
