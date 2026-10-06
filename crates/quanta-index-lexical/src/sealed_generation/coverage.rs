//! Admitted source files, including files without indexed units, bound to the
//! existing lexical generation.
//!
//! This artifact is not a parser completeness
//! proof: source hashes and extraction policy remain producer attestations.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::Path;

use quanta_index_contract::{
    FileCoverageSnapshot, GenerationSnapshot, SearchScopeSurface, SourceFileCoverage,
    SourceFileKey, SourcePublicationEvent,
};
use quanta_index_core::CoreError;
use quanta_index_core::domains::generation::SealedArtifactCommitmentV1;

pub(crate) const SOURCE_FILE_COVERAGE_FILE_NAME: &str = "source-file-coverage.cbor";
#[path = "coverage_pages.rs"]
mod pages;
pub(crate) use pages::{
    CoveragePlan, CoverageWriteBase, MAX_COVERAGE_ROOT_BYTES_U64, is_coverage_page,
    read_admitted_bytes, read_committed_coverage_root_at, root_page_commitments,
    root_page_commitments_at,
};

pub(crate) type CoverageSnapshot = FileCoverageSnapshot;

/// Successful coverage authentication and decode work.
///
/// A decoded-root cache
/// hit still adds every byte/page read and hashed, but no decoded rows. These
/// are logical bytes, not filesystem block I/O or process RSS.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LexicalCoverageReadStats {
    pub decodes: u64,
    pub root_bytes: u64,
    pub pages: u64,
    pub page_bytes: u64,
    pub rows: u64,
    /// Maximum conservative decode-heap admission charge, not measured RSS.
    pub max_decode_heap_admission_bytes: u64,
}

impl LexicalCoverageReadStats {
    pub(crate) fn absorb(&mut self, other: Self) {
        self.decodes = self.decodes.saturating_add(other.decodes);
        self.root_bytes = self.root_bytes.saturating_add(other.root_bytes);
        self.pages = self.pages.saturating_add(other.pages);
        self.page_bytes = self.page_bytes.saturating_add(other.page_bytes);
        self.rows = self.rows.saturating_add(other.rows);
        self.max_decode_heap_admission_bytes = self
            .max_decode_heap_admission_bytes
            .max(other.max_decode_heap_admission_bytes);
    }
}

/// Authenticated coverage work attributed to the caller's publication phase.
/// These are logical reads; seal hashing and non-coverage I/O are separate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LexicalCoverageReadByPhaseStats {
    pub total: LexicalCoverageReadStats,
    pub before_intent: LexicalCoverageReadStats,
    pub under_operation_lock: LexicalCoverageReadStats,
    pub build: LexicalCoverageReadStats,
    pub open: LexicalCoverageReadStats,
}

#[derive(Clone, Copy)]
pub(crate) enum CoverageReadPhase {
    BeforeIntent,
    UnderOperationLock,
    Build,
    Open,
}

impl LexicalCoverageReadByPhaseStats {
    pub(crate) fn absorb(&mut self, phase: CoverageReadPhase, read: LexicalCoverageReadStats) {
        self.total.absorb(read);
        match phase {
            CoverageReadPhase::BeforeIntent => &mut self.before_intent,
            CoverageReadPhase::UnderOperationLock => &mut self.under_operation_lock,
            CoverageReadPhase::Build => &mut self.build,
            CoverageReadPhase::Open => &mut self.open,
        }
        .absorb(read);
    }
}

/// Both values are proved by one manifest commitment and one generation open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CoverageArtifact {
    pub(crate) coverage: CoverageSnapshot,
    pub(crate) publication: SourcePublicationEvent,
    pub(crate) read_stats: LexicalCoverageReadStats,
}

/// One decoded sealed root. Every reuse still reads and hashes every committed
/// page and checks the current directory inventory before returning its rows.
#[derive(Default)]
pub(crate) struct CoverageDecodeCache {
    entry: Option<([u8; 32], CoverageArtifact)>,
}

impl CoverageDecodeCache {
    #[cfg(test)]
    pub(crate) fn decode(
        &mut self,
        bytes: &[u8],
        directory: &Path,
        expected: &GenerationSnapshot,
    ) -> Result<CoverageArtifact, CoreError> {
        self.decode_with(bytes, directory, expected, None)
    }

    pub(crate) fn decode_at(
        &mut self,
        root: &File,
        bytes: &[u8],
        directory: &Path,
        expected: &GenerationSnapshot,
    ) -> Result<CoverageArtifact, CoreError> {
        self.decode_with(bytes, directory, expected, Some(root))
    }

    fn decode_with(
        &mut self,
        bytes: &[u8],
        directory: &Path,
        expected: &GenerationSnapshot,
        root: Option<&File>,
    ) -> Result<CoverageArtifact, CoreError> {
        use sha2::{Digest as _, Sha256};
        let digest = <[u8; 32]>::from(Sha256::digest(bytes));
        let previous = self.entry.take();
        let cached = previous
            .as_ref()
            .filter(|(key, _)| *key == digest)
            .map(|(_, artifact)| artifact);
        let artifact = match root {
            Some(root) => {
                pages::decode_coverage_pages_reusing_at(root, bytes, directory, expected, cached)?
            }
            None => pages::decode_coverage_pages_reusing(bytes, directory, expected, cached)?,
        };
        // Bound adapter-retained decoded state independently from the larger
        // one-call decode admission. Large roots remain on the uncached rail.
        if artifact.read_stats.max_decode_heap_admission_bytes <= 8 * 1024 * 1024 {
            self.entry = Some((digest, artifact.clone()));
        }
        Ok(artifact)
    }
}

fn corrupt(generation_dir: &Path, reason: &str) -> CoreError {
    crate::index_store::sidecar_corrupt(generation_dir, SOURCE_FILE_COVERAGE_FILE_NAME, reason)
}

#[cfg(test)]
pub(crate) fn decode_coverage(
    bytes: &[u8],
    generation_dir: &Path,
    expected: &GenerationSnapshot,
) -> Result<CoverageArtifact, CoreError> {
    pages::decode_coverage_pages(bytes, generation_dir, expected)
}

pub(crate) fn decode_coverage_at(
    root: &File,
    bytes: &[u8],
    generation_dir: &Path,
    expected: &GenerationSnapshot,
) -> Result<CoverageArtifact, CoreError> {
    pages::decode_coverage_pages_reusing_at(root, bytes, generation_dir, expected, None)
}

/// Conservative retained-heap admission estimate, not measured allocator use
/// or RSS.
///
/// Use the snapshot owner's structural bound for both trees and shared keys/rows,
/// then charge all retained strings and
/// the event's actual String capacities. This intentionally overestimates
/// partially occupied nodes. Strings include conservative decoder capacity;
/// this is not a measured allocator ceiling.
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
        bytes = CoverageSnapshot::structural_heap_bytes_bound(
            u64::try_from(snapshot.len()).map_err(|error| {
                CoreError::InvalidContract(format!("lexical: coverage row count width: {error}"))
            })?,
        )
        .ok_or_else(|| {
            CoreError::InvalidContract("lexical: coverage structural heap estimate overflow".into())
        })?;
        for (key, entry) in snapshot {
            for string in [
                key.source_repo_id.as_str(),
                key.repo_relative_path.as_str(),
                entry.source.file.source_repo_id.as_str(),
                entry.source.file.repo_relative_path.as_str(),
                entry.source.revision_id.as_str(),
                entry.language.as_str(),
            ] {
                add(&mut bytes, string.len())?;
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
    pages::read_coverage_root(generation_dir)?
        .map(|bytes| pages::decode_staged_coverage_pages(&bytes, generation_dir, expected))
        .transpose()
}

/// Atomic rename preserves immutable old-reader and hard-link ownership.
/// Called only after the request and inherited snapshot have been validated.
pub(crate) fn write_staged_coverage(
    generation_dir: &Path,
    identity: &GenerationSnapshot,
    publication: &SourcePublicationEvent,
    plan: &CoveragePlan,
) -> Result<SealedArtifactCommitmentV1, CoreError> {
    pages::write_coverage_pages(
        generation_dir,
        identity,
        publication,
        &plan.coverage,
        plan.base.as_ref(),
        &plan.touched,
    )
}

pub(crate) fn plan_file_coverage<'a>(
    base: &CoverageSnapshot,
    write_base: Option<CoverageWriteBase>,
    replacements: impl Clone + IntoIterator<Item = &'a SourceFileCoverage>,
    tombstones: impl Clone + IntoIterator<Item = &'a SourceFileKey>,
    clears: &[SearchScopeSurface],
) -> Result<CoveragePlan, CoreError> {
    let coverage = apply_file_coverage(base, replacements.clone(), tombstones.clone(), clears)?;
    let touched = replacements
        .into_iter()
        .map(|row| &row.source.file)
        .chain(tombstones)
        .map(CoverageSnapshot::partition_for)
        .collect();
    Ok(CoveragePlan {
        coverage,
        base: write_base,
        touched,
    })
}

/// Compute an immutable candidate before any index or sidecar mutation.
///
/// Empty
/// replacements remain members of the admitted file universe. A delta retains
/// every unchanged entry, including failed or unrequested symbol extraction.
pub(crate) fn apply_file_coverage<'a>(
    base: &CoverageSnapshot,
    replacements: impl Clone + IntoIterator<Item = &'a SourceFileCoverage>,
    tombstones: impl Clone + IntoIterator<Item = &'a SourceFileKey>,
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
    for entry in replacements.clone() {
        entry
            .validate()
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        if !owners.insert(&entry.source.file) {
            return Err(CoreError::InvalidContract(
                "lexical: duplicate coverage replacement owner".into(),
            ));
        }
    }
    for key in tombstones.clone() {
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
        result.insert_without_previous(entry.source.file.clone(), entry.clone());
    }
    for key in tombstones {
        result.remove_without_previous(key);
    }
    Ok(result)
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "assertions report regression failures"
)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error;
    use std::fmt::Write as _;
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
        CoverageSnapshot, SOURCE_FILE_COVERAGE_FILE_NAME, apply_file_coverage, decode_coverage,
        read_staged_coverage,
    };
    use crate::sealed_generation::verify::verify_source_coverage;

    use super::pages::write_coverage_pages as write_staged_coverage;

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn decoded_root_reuse_reauthenticates_pages_and_invalidates_after_refusal() -> TestResult {
        let dir = crate::test_support::generation_fixture()?;
        let generation = identity(1)?;
        let row = file("a.rs", SymbolCoverage::ParseFailed)?;
        let snapshot = CoverageSnapshot::from([(row.source.file.clone(), row)]);
        let root = write_staged_coverage(
            dir.path(),
            &generation,
            &publication(),
            &snapshot,
            None,
            &BTreeSet::new(),
        )?;
        let bytes = std::fs::read(dir.path().join(SOURCE_FILE_COVERAGE_FILE_NAME))?;
        let mut cache = super::CoverageDecodeCache::default();
        let first = cache.decode(&bytes, dir.path(), &generation)?;
        assert_eq!(first.read_stats.decodes, 1);
        let repeated = cache.decode(&bytes, dir.path(), &generation)?;
        assert_eq!(repeated.coverage, snapshot);
        assert_eq!(repeated.read_stats.decodes, 0);
        assert_eq!(repeated.read_stats.rows, 0);
        assert_eq!(repeated.read_stats.page_bytes, first.read_stats.page_bytes);
        assert_eq!(repeated.read_stats.pages, first.read_stats.pages);

        let pages = super::root_page_commitments(dir.path(), &root, &generation)?;
        let path = dir
            .path()
            .join(&pages.first().ok_or("missing fixture page")?.name);
        let original = std::fs::read(&path)?;
        let mut corrupted = original.clone();
        *corrupted.last_mut().ok_or("empty page")? ^= 1;
        std::fs::write(&path, corrupted)?;
        assert!(cache.decode(&bytes, dir.path(), &generation).is_err());
        std::fs::write(&path, original)?;
        let retry = cache.decode(&bytes, dir.path(), &generation)?;
        assert_eq!(
            retry.read_stats.decodes, 1,
            "a refusal must discard cached rows"
        );
        assert_eq!(retry.coverage, snapshot);

        let orphan = dir.path().join("source-file-coverage-page-orphan.cbor");
        std::fs::write(&orphan, b"orphan")?;
        assert!(cache.decode(&bytes, dir.path(), &generation).is_err());
        std::fs::remove_file(orphan)?;
        assert!(cache.decode(&bytes, dir.path(), &identity(2)?).is_err());
        assert_eq!(
            cache
                .decode(&bytes, dir.path(), &generation)?
                .read_stats
                .decodes,
            1
        );

        let other = file("b.rs", SymbolCoverage::NotRequested)?;
        let next = CoverageSnapshot::from([(other.source.file.clone(), other)]);
        let _root = write_staged_coverage(
            dir.path(),
            &generation,
            &publication(),
            &next,
            None,
            &BTreeSet::new(),
        )?;
        let changed = std::fs::read(dir.path().join(SOURCE_FILE_COVERAGE_FILE_NAME))?;
        let refreshed = cache.decode(&changed, dir.path(), &generation)?;
        assert_eq!(refreshed.read_stats.decodes, 1);
        assert_eq!(refreshed.coverage, next);
        Ok(())
    }

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
            symbol_name_source_policy: quanta_index_contract::SymbolNameSourcePolicyV1::Unspecified,
            unit_set_sha256: quanta_index_contract::source_file_unit_set_sha256(&[], &[])?,
            text_admitted: true,
            symbols,
        })
    }

    fn encode(rows: Vec<SourceFileCoverage>) -> Result<Vec<u8>, Box<dyn Error>> {
        // Legacy flat-row artifacts are explicitly rejected by format 2.
        let row = (1_u32, identity(1)?, publication(), rows);
        let mut bytes = Vec::new();
        ciborium::into_writer(&row, &mut bytes)?;
        Ok(bytes)
    }

    #[test]
    fn empty_file_and_failed_parser_survive_delta_inheritance() -> TestResult {
        let empty = file("empty.rs", SymbolCoverage::Complete { symbol_count: 0 })?;
        let failed = file("failed.rs", SymbolCoverage::ParseFailed)?;
        let base = apply_file_coverage(
            &CoverageSnapshot::new(),
            &[empty.clone(), failed.clone()],
            &[],
            &[],
        )?;
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
    fn borrowed_delta_inputs_replace_and_delete_without_mutating_the_base() -> TestResult {
        let original = file("replace.rs", SymbolCoverage::NotRequested)?;
        let deleted = file("delete.rs", SymbolCoverage::ParseFailed)?;
        let base = CoverageSnapshot::from([
            (original.source.file.clone(), original.clone()),
            (deleted.source.file.clone(), deleted.clone()),
        ]);
        let mut replacement = original.clone();
        replacement.symbols = SymbolCoverage::Complete { symbol_count: 0 };
        let replacements = [replacement];
        let tombstones = [deleted.source.file.clone()];
        let plan =
            super::plan_file_coverage(&base, None, replacements.iter(), tombstones.iter(), &[])?;
        assert_eq!(plan.snapshot().len(), 1);
        assert_eq!(
            plan.snapshot().get(&original.source.file),
            Some(&replacements[0])
        );
        assert!(!plan.snapshot().contains_key(&deleted.source.file));
        let replacement_partition = CoverageSnapshot::partition_for(&original.source.file);
        assert_eq!(
            plan.snapshot()
                .partition(replacement_partition)
                .find(|(key, _)| *key == &original.source.file)
                .map(|(_, row)| row),
            Some(&replacements[0])
        );
        let deleted_partition = CoverageSnapshot::partition_for(&deleted.source.file);
        assert!(
            plan.snapshot()
                .partition(deleted_partition)
                .all(|(key, _)| key != &deleted.source.file)
        );
        assert_eq!(base.get(&original.source.file), Some(&original));
        assert_eq!(base.get(&deleted.source.file), Some(&deleted));
        assert_eq!(
            plan.touched,
            BTreeSet::from([
                CoverageSnapshot::partition_for(&original.source.file),
                CoverageSnapshot::partition_for(&deleted.source.file),
            ])
        );
        Ok(())
    }

    #[test]
    fn rejected_clears_and_conflicts_leave_the_base_unchanged() -> TestResult {
        let entry = file("a.rs", SymbolCoverage::Complete { symbol_count: 0 })?;
        let base = apply_file_coverage(
            &CoverageSnapshot::new(),
            std::slice::from_ref(&entry),
            &[],
            &[],
        )?;
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
                &[entry.source.file.clone(), entry.source.file],
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
        assert!(decode_coverage(&bytes, dir, &identity(1)?).is_err());
        Ok(())
    }

    #[test]
    fn missing_unknown_and_tampered_artifacts_cannot_claim_empty_complete() -> TestResult {
        let dir = crate::test_support::generation_fixture()?;
        let identity = identity(1)?;
        assert!(read_staged_coverage(dir.path(), &identity)?.is_none());
        assert!(read_staged_coverage(&dir.path().join("uncreated"), &identity)?.is_none());
        let _root = write_staged_coverage(
            dir.path(),
            &identity,
            &publication(),
            &CoverageSnapshot::new(),
            None,
            &BTreeSet::new(),
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
        let mut tampered = bytes;
        let byte = tampered.last_mut().ok_or("empty artifact fixture")?;
        *byte ^= 1;
        std::fs::write(&path, &tampered)?;
        assert!(verify_source_coverage(dir.path(), &identity, Some(&committed)).is_err());
        std::fs::remove_file(&path)?;
        assert!(verify_source_coverage(dir.path(), &identity, Some(&committed)).is_err());
        assert!(verify_source_coverage(dir.path(), &identity, None)?.is_none());
        Ok(())
    }

    #[test]
    fn invalid_map_owner_is_refused_before_creating_the_target() -> TestResult {
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("uncreated");
        let row = file("a.rs", SymbolCoverage::Complete { symbol_count: 0 })?;
        let mut key = row.source.file.clone();
        key.repo_relative_path = RepoRelativePath::new("b.rs");
        let invalid = CoverageSnapshot::from([(key, row)]);
        assert!(
            write_staged_coverage(
                &target,
                &identity(1)?,
                &publication(),
                &invalid,
                None,
                &BTreeSet::new()
            )
            .is_err()
        );
        assert!(
            !target.exists(),
            "refused ownership cannot leave partial staging"
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::print_stdout,
        reason = "owner-local physical page cost evidence"
    )]
    fn one_file_delta_shares_rows_and_inherits_unmodified_pages() -> TestResult {
        use std::os::unix::fs::MetadataExt as _;
        for files in [1024, 2048, 4096, 32_768] {
            let directory = tempfile::tempdir()?;
            let base_dir = directory.path().join("base");
            let candidate_dir = directory.path().join("delta");
            let mut original = Vec::new();
            for n in 0..files {
                original.push(file(
                    &format!("src/{n:05}.rs"),
                    SymbolCoverage::Complete { symbol_count: 0 },
                )?);
            }
            let base = apply_file_coverage(&CoverageSnapshot::new(), &original, &[], &[])?;
            let _root = write_staged_coverage(
                &base_dir,
                &identity(1)?,
                &publication(),
                &base,
                None,
                &BTreeSet::new(),
            )?;
            let root_bytes = std::fs::read(base_dir.join(SOURCE_FILE_COVERAGE_FILE_NAME))?;
            let root = SealedArtifactCommitmentV1 {
                name: SOURCE_FILE_COVERAGE_FILE_NAME.into(),
                bytes: u64::try_from(root_bytes.len())?,
                sha256: Sha256::digest(&root_bytes).into(),
            };
            let mut changed = original.first().ok_or("missing first fixture")?.clone();
            changed.source.source_sha256 = [9; 32];
            changed.symbols = SymbolCoverage::ParseFailed;
            let candidate = apply_file_coverage(&base, std::slice::from_ref(&changed), &[], &[])?;
            assert!(
                std::ptr::eq(
                    base.get(&original.get(1).ok_or("missing second fixture")?.source.file)
                        .ok_or("missing base")?,
                    candidate
                        .get(&original.get(1).ok_or("missing second fixture")?.source.file)
                        .ok_or("missing candidate")?
                ),
                "an unchanged row must be shared, not deep-cloned"
            );
            let touched = BTreeSet::from([CoverageSnapshot::partition_for(&changed.source.file)]);
            let inherited = super::CoverageWriteBase {
                directory: base_dir.clone(),
                root,
            };
            let event = SourcePublicationEvent {
                event_id: "event-2".into(),
                expected_base_event_id: Some("event-1".into()),
                ..publication()
            };
            let _root = write_staged_coverage(
                &candidate_dir,
                &identity(2)?,
                &event,
                &candidate,
                Some(&inherited),
                &touched,
            )?;
            crate::generation_dir::clone_generation_directory_preserving_existing(
                &base_dir,
                &candidate_dir,
            )?;
            let reopened =
                read_staged_coverage(&candidate_dir, &identity(2)?)?.ok_or("coverage missing")?;
            assert_eq!(reopened.publication, event);
            for row in &original {
                let expected = if row.source.file == changed.source.file {
                    &changed
                } else {
                    row
                };
                assert_eq!(reopened.coverage.get(&row.source.file), Some(expected));
            }
            assert_eq!(reopened.coverage.len(), original.len());
            let retained_charge = super::coverage_heap_bytes_estimate(
                Some(&reopened.coverage),
                Some(&reopened.publication),
            )?;
            if retained_charge > reopened.read_stats.max_decode_heap_admission_bytes {
                return Err(format!(
                    "opened coverage charge escaped decode admission: {retained_charge}/{}",
                    reopened.read_stats.max_decode_heap_admission_bytes,
                )
                .into());
            }
            assert_eq!(base.get(&changed.source.file), original.first());
            let mut fresh = 0_u64;
            let mut base_bytes = 0_u64;
            let mut inherited_pages = 0;
            for entry in std::fs::read_dir(&base_dir)? {
                base_bytes += entry?.metadata()?.len();
            }
            for entry in std::fs::read_dir(&candidate_dir)? {
                let entry = entry?;
                let metadata = entry.metadata()?;
                let source = base_dir.join(entry.file_name());
                if source.exists() && source.metadata()?.ino() == metadata.ino() {
                    inherited_pages += 1;
                } else {
                    fresh += metadata.len();
                }
            }
            assert!(
                inherited_pages > 200,
                "untouched partitions must carry their original inodes"
            );
            assert!(
                fresh.saturating_mul(4) < base_bytes,
                "one-file update must not rewrite all coverage bytes: {fresh}/{base_bytes}"
            );
            println!(
                "COVERAGE-PAGE-EVIDENCE files={files} base_bytes={base_bytes} fresh_bytes={fresh} inherited_pages={inherited_pages}"
            );
        }
        Ok(())
    }
    fn cbor(value: &impl serde::Serialize) -> Result<Vec<u8>, Box<dyn Error>> {
        let mut bytes = Vec::new();
        ciborium::into_writer(value, &mut bytes)?;
        Ok(bytes)
    }

    #[test]
    fn committed_pages_refuse_semantic_forgery_even_with_recomputed_hashes() -> TestResult {
        let dir = crate::test_support::generation_fixture()?;
        let a = file("a.rs", SymbolCoverage::NotRequested)?;
        let slot = CoverageSnapshot::partition_for(&a.source.file);
        let b = (0..10000)
            .map(|n| file(&format!("z{n:05}.rs"), SymbolCoverage::ParseFailed))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .find(|row| CoverageSnapshot::partition_for(&row.source.file) == slot)
            .ok_or("no deterministic collision fixture")?;
        let expected = vec![a.clone(), b];
        for mutant in 0..12 {
            let mut rows = expected.clone();
            match mutant {
                1 => *rows.get_mut(1).ok_or("missing duplicate fixture")? = a.clone(),
                2 => rows.reverse(),
                3 => {
                    rows.first_mut()
                        .ok_or("missing misrouted fixture")?
                        .source
                        .file
                        .repo_relative_path = RepoRelativePath::new("misrouted.rs");
                }
                _ => {}
            }
            let mut raw = cbor(&(if mutant == 4 { 1_u32 } else { 2 }, slot, rows))?;
            if mutant == 5 {
                raw.push(0);
            }
            let digest: [u8; 32] = Sha256::digest(&raw).into();
            let mut hex = String::with_capacity(64);
            for byte in digest {
                write!(&mut hex, "{byte:02x}")?;
            }
            let name = format!("source-file-coverage-page-{slot:02x}-{hex}.cbor");
            std::fs::write(dir.path().join(&name), &raw)?;
            let mut pages = vec![(
                slot,
                u64::try_from(raw.len())?,
                digest,
                if mutant == 6 { 1_u32 } else { 2 },
            )];
            if mutant == 7 {
                pages.push(*pages.first().ok_or("missing page fixture")?);
            }
            if mutant == 8 {
                pages.first_mut().ok_or("missing page fixture")?.1 += 1;
            }
            if mutant == 9 {
                *pages
                    .first_mut()
                    .ok_or("missing page fixture")?
                    .2
                    .first_mut()
                    .ok_or("missing page digest")? ^= 1;
            }
            let generation = identity(if mutant == 10 { 2 } else { 1 })?;
            let mut root = cbor(&(2_u32, generation, publication(), pages))?;
            if mutant == 11 {
                root.push(0);
            }
            let decoded = decode_coverage(&root, dir.path(), &identity(1)?);
            if mutant == 0 {
                let decoded = decoded?;
                assert_eq!(decoded.coverage.len(), 2);
                for row in &expected {
                    assert_eq!(decoded.coverage.get(&row.source.file), Some(row));
                }
            } else {
                assert!(decoded.is_err(), "semantic mutant {mutant} must be refused");
            }
            std::fs::remove_file(dir.path().join(name))?;
        }
        Ok(())
    }

    #[test]
    fn missing_tampered_symlink_and_orphan_pages_are_typed_corruption() -> TestResult {
        use quanta_index_contract::SearchPlaneErrorCodeV2;
        let dir = crate::test_support::generation_fixture()?;
        let row = file("a.rs", SymbolCoverage::NotRequested)?;
        let snapshot = CoverageSnapshot::from([(row.source.file.clone(), row)]);
        let _root = write_staged_coverage(
            dir.path(),
            &identity(1)?,
            &publication(),
            &snapshot,
            None,
            &BTreeSet::new(),
        )?;
        let root_path = dir.path().join(SOURCE_FILE_COVERAGE_FILE_NAME);
        let root = std::fs::read(&root_path)?;
        let page = std::fs::read_dir(dir.path())?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .find(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(super::is_coverage_page)
            })
            .ok_or("no page")?
            .path();
        let raw = std::fs::read(&page)?;
        for mutant in 0..3 {
            match mutant {
                0 => {
                    let mut changed = raw.clone();
                    *changed.first_mut().ok_or("missing page bytes")? ^= 1;
                    std::fs::write(&page, changed)?;
                }
                1 => {
                    std::fs::remove_file(&page)?;
                }
                _ => {
                    std::fs::remove_file(&page)?;
                    std::os::unix::fs::symlink(&root_path, &page)?;
                }
            }
            assert!(matches!(
                decode_coverage(&root, dir.path(), &identity(1)?),
                Err(quanta_index_core::CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    ..
                })
            ));
            if page.symlink_metadata().is_ok() {
                std::fs::remove_file(&page)?;
            }
            std::fs::write(&page, &raw)?;
        }
        let orphan = dir
            .path()
            .join("source-file-coverage-page-uncommitted.cbor");
        std::fs::write(&orphan, b"uncommitted")?;
        assert!(decode_coverage(&root, dir.path(), &identity(1)?).is_err());
        std::fs::remove_file(orphan)?;
        std::fs::remove_file(root_path)?;
        assert!(read_staged_coverage(dir.path(), &identity(1)?)?.is_none());
        assert!(verify_source_coverage(dir.path(), &identity(1)?, None).is_err());
        // Candidate pages without a root can be reclaimed by an identical
        // retry. The sealed door above must still reject that same state.
        let row = file("a.rs", SymbolCoverage::NotRequested)?;
        let snapshot = CoverageSnapshot::from([(row.source.file.clone(), row)]);
        let _root = write_staged_coverage(
            dir.path(),
            &identity(1)?,
            &publication(),
            &snapshot,
            None,
            &BTreeSet::new(),
        )?;
        assert_eq!(
            read_staged_coverage(dir.path(), &identity(1)?)?
                .ok_or("missing recovered root")?
                .coverage,
            snapshot
        );
        Ok(())
    }

    #[test]
    fn unsealed_retry_reclaims_orphans_without_weakening_sealed_verification() -> TestResult {
        let dir = crate::test_support::generation_fixture()?;
        let row = file("a.rs", SymbolCoverage::NotRequested)?;
        let snapshot = CoverageSnapshot::from([(row.source.file.clone(), row)]);
        let generation = identity(1)?;
        let event = publication();
        let root_commitment = write_staged_coverage(
            dir.path(),
            &generation,
            &event,
            &snapshot,
            None,
            &BTreeSet::new(),
        )?;
        let root = std::fs::read(dir.path().join(SOURCE_FILE_COVERAGE_FILE_NAME))?;
        let orphan = dir
            .path()
            .join("source-file-coverage-page-uncommitted.cbor");
        std::fs::write(&orphan, b"interrupted page write")?;

        assert!(decode_coverage(&root, dir.path(), &generation).is_err());
        assert!(
            super::root_page_commitments(dir.path(), &root_commitment, &generation).is_err(),
            "the seal path must reject an orphan even when the staged retry can remove it"
        );
        assert_eq!(
            read_staged_coverage(dir.path(), &generation)?
                .ok_or("missing staged root")?
                .coverage,
            snapshot
        );
        let _root = write_staged_coverage(
            dir.path(),
            &generation,
            &event,
            &snapshot,
            None,
            &BTreeSet::new(),
        )?;
        assert!(!orphan.exists());
        let root = std::fs::read(dir.path().join(SOURCE_FILE_COVERAGE_FILE_NAME))?;
        assert_eq!(
            decode_coverage(&root, dir.path(), &generation)?.coverage,
            snapshot
        );
        Ok(())
    }

    #[test]
    fn deleting_the_last_file_replaces_the_root_without_retaining_pages() -> TestResult {
        let dir = tempfile::tempdir()?;
        let row = file("last.rs", SymbolCoverage::ParseFailed)?;
        let base = CoverageSnapshot::from([(row.source.file.clone(), row.clone())]);
        let base_dir = dir.path().join("base");
        let target = dir.path().join("delta");
        let _root = write_staged_coverage(
            &base_dir,
            &identity(1)?,
            &publication(),
            &base,
            None,
            &BTreeSet::new(),
        )?;
        let raw = std::fs::read(base_dir.join(SOURCE_FILE_COVERAGE_FILE_NAME))?;
        let plan = super::plan_file_coverage(
            &base,
            Some(super::CoverageWriteBase {
                directory: base_dir.clone(),
                root: SealedArtifactCommitmentV1 {
                    name: SOURCE_FILE_COVERAGE_FILE_NAME.into(),
                    bytes: u64::try_from(raw.len())?,
                    sha256: Sha256::digest(&raw).into(),
                },
            }),
            &[],
            std::slice::from_ref(&row.source.file),
            &[],
        )?;
        let event = SourcePublicationEvent {
            event_id: "event-2".into(),
            expected_base_event_id: Some("event-1".into()),
            ..publication()
        };
        let _root = super::write_staged_coverage(&target, &identity(2)?, &event, &plan)?;
        crate::generation_dir::clone_generation_directory_preserving_existing(&base_dir, &target)?;
        let reopened = read_staged_coverage(&target, &identity(2)?)?.ok_or("missing empty root")?;
        assert!(reopened.coverage.is_empty());
        assert_eq!(reopened.publication, event);
        assert_eq!(std::fs::read_dir(&target)?.count(), 1);
        assert_eq!(
            read_staged_coverage(&base_dir, &identity(1)?)?
                .ok_or("missing old reader")?
                .coverage,
            base
        );
        Ok(())
    }

    #[test]
    fn oversized_page_refuses_before_target_mutation() -> TestResult {
        use quanta_index_contract::SearchPlaneErrorCodeV2;
        // Precomputed collision suffixes keep the boundary test cheap even in a
        // debug build; every row is still routed and validated at runtime.
        const SUFFIXES: [usize; 300] = [
            0, 158, 399, 454, 1131, 1235, 1315, 1935, 2123, 2161, 2404, 2629, 2913, 2970, 3024,
            3046, 3177, 3504, 3700, 3744, 4246, 4287, 4479, 4500, 4686, 4765, 4818, 4964, 5364,
            5562, 6188, 6255, 6464, 6567, 6748, 6954, 7000, 7361, 7443, 7597, 7633, 8280, 9125,
            9332, 9413, 9460, 9647, 9992, 10623, 10910, 11402, 11554, 11715, 11745, 12161, 13473,
            14550, 15390, 15504, 15640, 16680, 16876, 16987, 17384, 17446, 17756, 18309, 18314,
            18519, 18763, 18865, 19278, 19515, 19620, 19750, 20695, 20948, 21640, 21675, 22376,
            22735, 22978, 23092, 23810, 23816, 23865, 23953, 23959, 24006, 24224, 24397, 24805,
            25656, 25931, 26057, 26130, 26263, 26470, 26632, 26858, 27023, 27056, 27330, 27458,
            27846, 27889, 28003, 28365, 28831, 29061, 29121, 29315, 29948, 30199, 30372, 31252,
            31560, 32024, 32283, 32353, 32374, 32694, 32807, 33468, 33470, 33642, 33661, 33728,
            33766, 33908, 34091, 34753, 35340, 35647, 36553, 36633, 36946, 36953, 37082, 37309,
            37559, 37724, 38184, 38224, 38358, 38477, 38593, 38625, 38751, 38753, 39091, 39291,
            39403, 39474, 40109, 40144, 40155, 40263, 40291, 40417, 40731, 41091, 41252, 41311,
            41688, 41800, 42364, 42508, 42735, 42926, 43353, 43541, 43562, 43937, 44845, 45269,
            46030, 47032, 47372, 49200, 49235, 49635, 49796, 50261, 50339, 50581, 51295, 51392,
            51634, 51763, 51824, 52485, 52628, 52687, 52758, 52866, 53364, 54657, 54697, 54803,
            55152, 55182, 55457, 55673, 55790, 55832, 55856, 56324, 56830, 57291, 57403, 57467,
            57508, 57843, 58057, 58298, 58498, 58820, 58822, 58947, 58977, 59320, 59619, 60060,
            60176, 60253, 60433, 60579, 60630, 60762, 61008, 61206, 61398, 61502, 61662, 61883,
            62067, 62164, 62197, 62650, 63154, 63440, 63563, 63621, 63731, 63833, 63969, 64040,
            64306, 65063, 66043, 66175, 66800, 66930, 66965, 67069, 67137, 67529, 67815, 68591,
            68754, 69039, 69061, 69161, 69551, 69568, 70419, 70658, 70977, 71273, 71570, 71590,
            71725, 71886, 72805, 72944, 73519, 73738, 73854, 74218, 74246, 74470, 74698, 74742,
            74954, 74967, 75488, 75554, 75559, 75906, 75949, 76301, 76420, 76709, 76772, 76779,
            77073, 77259, 77796, 78023,
        ];
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("uncreated");
        let mut rows = Vec::with_capacity(SUFFIXES.len());
        for n in SUFFIXES {
            let row = file(
                &format!("src/{}-{n:05}.rs", "x".repeat(3900)),
                SymbolCoverage::NotRequested,
            )?;
            assert_eq!(CoverageSnapshot::partition_for(&row.source.file), 223);
            rows.push((row.source.file.clone(), row));
        }
        let snapshot: CoverageSnapshot = rows.into_iter().collect();
        assert!(matches!(
            write_staged_coverage(
                &target,
                &identity(1)?,
                &publication(),
                &snapshot,
                None,
                &BTreeSet::new()
            ),
            Err(quanta_index_core::CoreError::Typed {
                code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                ..
            })
        ));
        assert!(!target.exists());
        Ok(())
    }
}
