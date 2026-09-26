//! Admitted source files, including files without indexed units, bound to the
//! existing lexical generation. This artifact is not a parser completeness
//! proof: source hashes and extraction policy remain producer attestations.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::path::Path;

use quanta_index_contract::{
    FileCoverageSnapshot, GenerationSnapshot, RepoId, RepoRelativePath, RevisionId,
    SearchScopeSurface, SourceFileCoverage, SourceFileKey, SourcePublicationEvent,
};
use quanta_index_core::CoreError;

pub(crate) const SOURCE_FILE_COVERAGE_FILE_NAME: &str = "source-file-coverage.cbor";
const COVERAGE_FORMAT: u32 = 1;

pub(crate) type CoverageSnapshot = FileCoverageSnapshot;

/// Both values are proved by one manifest commitment and one generation open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CoverageArtifact {
    pub(crate) coverage: CoverageSnapshot,
    pub(crate) publication: SourcePublicationEvent,
}

// Rows rather than a serialized map preserve duplicate keys for validation.
// A map deserializer could silently retain only the last duplicate.
type CoverageRow = (
    u32,
    GenerationSnapshot,
    SourcePublicationEvent,
    Vec<SourceFileCoverage>,
);

fn corrupt(generation_dir: &Path, reason: &str) -> CoreError {
    crate::index_store::sidecar_corrupt(generation_dir, SOURCE_FILE_COVERAGE_FILE_NAME, reason)
}

pub(crate) fn decode_coverage(
    bytes: &[u8],
    generation_dir: &Path,
    expected: &GenerationSnapshot,
) -> Result<CoverageArtifact, CoreError> {
    let mut reader = Cursor::new(bytes);
    let (format, identity, publication, rows): CoverageRow = ciborium::from_reader(&mut reader)
        .map_err(|error| corrupt(generation_dir, &format!("decode coverage: {error}")))?;
    if reader.position()
        != u64::try_from(bytes.len()).map_err(|error| {
            corrupt(
                generation_dir,
                &format!("coverage byte length overflow: {error}"),
            )
        })?
    {
        return Err(corrupt(
            generation_dir,
            "trailing bytes after coverage artifact",
        ));
    }
    if format != COVERAGE_FORMAT {
        return Err(corrupt(generation_dir, "unsupported coverage format"));
    }
    if identity != *expected {
        return Err(corrupt(
            generation_dir,
            "coverage belongs to another generation identity",
        ));
    }
    let mut snapshot = BTreeMap::new();
    for mut entry in rows {
        // The private ID wrappers do not expose String capacity. Compact them
        // at the decode boundary so retained string bytes can be accounted
        // from their lengths rather than guessing hidden spare capacity.
        let key = &mut entry.source.file;
        key.source_repo_id = RepoId::new(
            key.source_repo_id
                .as_str()
                .to_owned()
                .into_boxed_str()
                .into_string(),
        )
        .map_err(|error| corrupt(generation_dir, &error.to_string()))?;
        key.repo_relative_path = RepoRelativePath::new(
            key.repo_relative_path
                .as_str()
                .to_owned()
                .into_boxed_str()
                .into_string(),
        );
        entry.source.revision_id = RevisionId::new(
            entry
                .source
                .revision_id
                .as_str()
                .to_owned()
                .into_boxed_str()
                .into_string(),
        )
        .map_err(|error| corrupt(generation_dir, &error.to_string()))?;
        let key = &entry.source.file;
        if snapshot
            .last_key_value()
            .is_some_and(|(previous, _)| previous >= key)
        {
            return Err(corrupt(
                generation_dir,
                "coverage files are duplicate or out of order",
            ));
        }
        let _previous = snapshot.insert(key.clone(), entry);
    }
    Ok(CoverageArtifact {
        coverage: snapshot,
        publication,
    })
}

/// Conservative retained-heap admission estimate, not measured allocator use
/// or RSS. Charge a full 16-slot B-tree node per file (including links/header),
/// both the map key and the duplicate source key, all retained strings, and
/// the event's actual String capacities. This intentionally overestimates
/// partially occupied nodes. Decode compacts opaque ID string allocations.
pub(crate) fn coverage_heap_bytes_estimate(
    coverage: Option<&CoverageSnapshot>,
    publication: Option<&SourcePublicationEvent>,
) -> Result<u64, CoreError> {
    fn add(total: &mut u64, bytes: usize) -> Result<(), CoreError> {
        let bytes = u64::try_from(bytes).map_err(|error| {
            CoreError::InvalidContract(format!(
                "lexical: coverage heap estimate length overflow: {error}",
            ))
        })?;
        *total = total.checked_add(bytes).ok_or_else(|| {
            CoreError::InvalidContract("lexical: coverage heap estimate overflow".into())
        })?;
        Ok(())
    }
    let mut bytes = 0;
    if let Some(snapshot) = coverage {
        add(&mut bytes, std::mem::size_of::<CoverageSnapshot>())?;
        for (key, entry) in snapshot {
            for _slot in 0..16 {
                add(
                    &mut bytes,
                    std::mem::size_of::<(SourceFileKey, SourceFileCoverage)>(),
                )?;
                add(&mut bytes, std::mem::size_of::<usize>())?;
            }
            add(&mut bytes, 128)?;
            for string in [
                key.source_repo_id.as_str(),
                key.repo_relative_path.as_str(),
                entry.source.file.source_repo_id.as_str(),
                entry.source.file.repo_relative_path.as_str(),
                entry.source.revision_id.as_str(),
                entry.language.as_str(),
            ] {
                add(&mut bytes, string.len())?;
                add(&mut bytes, 16)?;
            }
        }
    }
    if let Some(event) = publication {
        add(&mut bytes, std::mem::size_of::<SourcePublicationEvent>())?;
        add(&mut bytes, event.stream_id.capacity())?;
        add(&mut bytes, event.event_id.capacity())?;
        if let Some(base) = &event.expected_base_event_id {
            add(&mut bytes, base.capacity())?;
        }
        add(&mut bytes, 48)?;
    }
    Ok(bytes)
}

/// Read an unsealed generation's explicitly published coverage, without
/// fabricating an empty universe when the artifact is absent.
pub(crate) fn read_staged_coverage(
    generation_dir: &Path,
    expected: &GenerationSnapshot,
) -> Result<Option<CoverageArtifact>, CoreError> {
    let path = generation_dir.join(SOURCE_FILE_COVERAGE_FILE_NAME);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "lexical: read source coverage {}: {error}",
                path.display()
            )));
        }
    };
    decode_coverage(&bytes, generation_dir, expected).map(Some)
}

/// Atomic rename preserves immutable old-reader and hard-link ownership.
/// Called only after the request and inherited snapshot have been validated.
pub(crate) fn write_staged_coverage(
    generation_dir: &Path,
    identity: &GenerationSnapshot,
    publication: &SourcePublicationEvent,
    snapshot: &CoverageSnapshot,
) -> Result<(), CoreError> {
    for (key, entry) in snapshot {
        if key != &entry.source.file {
            return Err(corrupt(
                generation_dir,
                "coverage map key differs from file owner",
            ));
        }
    }
    let rows: Vec<_> = snapshot.values().collect();
    let mut bytes = Vec::new();
    ciborium::into_writer(&(COVERAGE_FORMAT, identity, publication, rows), &mut bytes)
        .map_err(|error| corrupt(generation_dir, &format!("encode coverage: {error}")))?;
    std::fs::create_dir_all(generation_dir).map_err(|error| {
        corrupt(
            generation_dir,
            &format!("create coverage staging directory: {error}"),
        )
    })?;
    crate::index_store::write_atomic_durable(
        &generation_dir.join(SOURCE_FILE_COVERAGE_FILE_NAME),
        &bytes,
        "source file coverage",
    )
}

/// Compute an immutable candidate before any index or sidecar mutation. Empty
/// replacements remain members of the admitted file universe. A delta retains
/// every unchanged entry, including failed or unrequested symbol extraction.
pub(crate) fn apply_file_coverage(
    base: &CoverageSnapshot,
    replacements: &[SourceFileCoverage],
    tombstones: &[SourceFileKey],
    clears: &[SearchScopeSurface],
) -> Result<CoverageSnapshot, CoreError> {
    if clears.iter().any(|surface| {
        matches!(
            surface,
            SearchScopeSurface::Chunk | SearchScopeSurface::Symbol
        )
    }) {
        return Err(CoreError::InvalidContract(
            "lexical: coverage-bound generations forbid independent Chunk/Symbol clear".into(),
        ));
    }
    let mut owners = BTreeSet::new();
    for entry in replacements {
        entry
            .validate()
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        if !owners.insert(&entry.source.file) {
            return Err(CoreError::InvalidContract(
                "lexical: duplicate coverage replacement owner".into(),
            ));
        }
    }
    for key in tombstones {
        key.validate()
            .map_err(|error| CoreError::InvalidContract(error.into()))?;
        if !owners.insert(key) {
            return Err(CoreError::InvalidContract(
                "lexical: duplicate or conflicting coverage tombstone".into(),
            ));
        }
    }
    let mut result = base.clone();
    for entry in replacements {
        let _previous = result.insert(entry.source.file.clone(), entry.clone());
    }
    for key in tombstones {
        let _previous = result.remove(key);
    }
    Ok(result)
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "assertions report regression failures"
)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::path::Path;

    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
        SearchPlaneTrackKind, SearchScopeSurface, SourceFileCoverage, SourceFileKey,
        SourceFileRevision, SourcePublicationEvent, SymbolCoverage,
    };
    use quanta_index_core::domains::generation::SealedArtifactCommitmentV1;
    use sha2::{Digest as _, Sha256};

    use super::{
        COVERAGE_FORMAT, CoverageRow, CoverageSnapshot, SOURCE_FILE_COVERAGE_FILE_NAME,
        apply_file_coverage, decode_coverage, read_staged_coverage, write_staged_coverage,
    };
    use crate::sealed_generation::verify::verify_source_coverage;

    type TestResult = Result<(), Box<dyn Error>>;

    fn publication() -> SourcePublicationEvent {
        SourcePublicationEvent {
            stream_id: "coverage-test".into(),
            event_id: "event-1".into(),
            expected_base_event_id: None,
            payload_sha256: [4; 32],
        }
    }

    fn identity(generation: u64) -> Result<GenerationSnapshot, Box<dyn Error>> {
        Ok(GenerationSnapshot {
            repo_id: RepoId::new("containing-repo")?,
            revision_id: RevisionId::new("containing-revision")?,
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: format!("manifest-{generation}"),
        })
    }

    fn file(path: &str, symbols: SymbolCoverage) -> Result<SourceFileCoverage, Box<dyn Error>> {
        Ok(SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("source-repo")?,
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: RevisionId::new("source-revision")?,
                source_sha256: [2; 32],
            },
            language: LanguageCode::new("rust")?,
            producer_policy_sha256: [1; 32],
            unit_set_sha256: quanta_index_contract::source_file_unit_set_sha256(&[], &[])?,
            text_admitted: true,
            symbols,
        })
    }

    fn encode(rows: Vec<SourceFileCoverage>) -> Result<Vec<u8>, Box<dyn Error>> {
        let row: CoverageRow = (COVERAGE_FORMAT, identity(1)?, publication(), rows);
        let mut bytes = Vec::new();
        ciborium::into_writer(&row, &mut bytes)?;
        Ok(bytes)
    }

    #[test]
    fn empty_file_and_failed_parser_survive_delta_inheritance() -> TestResult {
        let empty = file("empty.rs", SymbolCoverage::Complete { symbol_count: 0 })?;
        let failed = file("failed.rs", SymbolCoverage::ParseFailed)?;
        let base =
            apply_file_coverage(&BTreeMap::new(), &[empty.clone(), failed.clone()], &[], &[])?;
        let added = file("new.rs", SymbolCoverage::NotRequested)?;
        let delta = apply_file_coverage(&base, std::slice::from_ref(&added), &[], &[])?;
        assert_eq!(delta.len(), 3);
        assert_eq!(delta.get(&empty.source.file), Some(&empty));
        assert_eq!(delta.get(&failed.source.file), Some(&failed));
        let removed =
            apply_file_coverage(&delta, &[], std::slice::from_ref(&failed.source.file), &[])?;
        assert_eq!(removed.len(), 2);
        assert!(!removed.contains_key(&failed.source.file));
        assert_eq!(base.len(), 2, "immutable base must not change");
        Ok(())
    }

    #[test]
    fn rejected_clears_and_conflicts_leave_the_base_unchanged() -> TestResult {
        let entry = file("a.rs", SymbolCoverage::Complete { symbol_count: 0 })?;
        let base = apply_file_coverage(&BTreeMap::new(), std::slice::from_ref(&entry), &[], &[])?;
        let before = base.clone();
        for surface in [SearchScopeSurface::Chunk, SearchScopeSurface::Symbol] {
            assert!(apply_file_coverage(&base, &[], &[], &[surface]).is_err());
        }
        assert!(apply_file_coverage(&base, &[entry.clone(), entry.clone()], &[], &[]).is_err());
        assert!(
            apply_file_coverage(
                &base,
                std::slice::from_ref(&entry),
                std::slice::from_ref(&entry.source.file),
                &[]
            )
            .is_err()
        );
        assert!(
            apply_file_coverage(
                &base,
                &[],
                &[entry.source.file.clone(), entry.source.file.clone()],
                &[]
            )
            .is_err()
        );
        assert_eq!(
            apply_file_coverage(
                &base,
                &[],
                &[],
                &[SearchScopeSurface::File, SearchScopeSurface::Module]
            )?,
            before
        );
        assert_eq!(base, before);
        Ok(())
    }

    #[test]
    fn decoder_rejects_duplicate_reordered_wrong_generation_and_trailing_bytes() -> TestResult {
        let a = file("a.rs", SymbolCoverage::Complete { symbol_count: 0 })?;
        let b = file("b.rs", SymbolCoverage::Unsupported)?;
        let dir = Path::new("/coverage-fixture");
        for rows in [vec![a.clone(), a.clone()], vec![b.clone(), a.clone()]] {
            assert!(decode_coverage(&encode(rows)?, dir, &identity(1)?).is_err());
        }
        let bytes = encode(vec![a, b])?;
        assert!(decode_coverage(&bytes, dir, &identity(2)?).is_err());
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode_coverage(&trailing, dir, &identity(1)?).is_err());
        let decoded = decode_coverage(&bytes, dir, &identity(1)?)?;
        assert_eq!(decoded.coverage.len(), 2);
        assert_eq!(decoded.publication, publication());
        Ok(())
    }

    #[test]
    fn missing_unknown_and_tampered_artifacts_cannot_claim_empty_complete() -> TestResult {
        let dir = tempfile::tempdir()?;
        let identity = identity(1)?;
        assert!(read_staged_coverage(dir.path(), &identity)?.is_none());
        write_staged_coverage(
            dir.path(),
            &identity,
            &publication(),
            &CoverageSnapshot::new(),
        )?;
        assert_eq!(
            read_staged_coverage(dir.path(), &identity)?.map(|artifact| artifact.coverage),
            Some(CoverageSnapshot::new())
        );
        let path = dir.path().join(SOURCE_FILE_COVERAGE_FILE_NAME);
        let bytes = std::fs::read(&path)?;
        let committed = SealedArtifactCommitmentV1 {
            name: SOURCE_FILE_COVERAGE_FILE_NAME.into(),
            bytes: u64::try_from(bytes.len())?,
            sha256: Sha256::digest(&bytes).into(),
        };
        assert!(verify_source_coverage(dir.path(), &identity, None).is_err());
        assert_eq!(
            verify_source_coverage(dir.path(), &identity, Some(&committed))?
                .map(|artifact| artifact.coverage),
            Some(CoverageSnapshot::new())
        );
        let mut tampered = bytes.clone();
        let byte = tampered.last_mut().ok_or("empty artifact fixture")?;
        *byte ^= 1;
        std::fs::write(&path, &tampered)?;
        assert!(verify_source_coverage(dir.path(), &identity, Some(&committed)).is_err());
        std::fs::remove_file(&path)?;
        assert!(verify_source_coverage(dir.path(), &identity, Some(&committed)).is_err());
        assert!(verify_source_coverage(dir.path(), &identity, None)?.is_none());
        Ok(())
    }
}
