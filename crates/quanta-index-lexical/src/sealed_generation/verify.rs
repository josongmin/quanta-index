//! The walk both doors share: prove a sealed generation is what its
//! manifest says, reading every decodable file exactly once.
//!
//! The activation validator and the cold open call the same function over
//! the same manifest, so activation can only admit a generation a query
//! can open (QI-BB-030). Each section is proved the way the manifest stamps
//! it:
//!
//! - `meta.json` is read and hashed; the index is opened from it and the
//!   segment files it references must be exactly the listed ones, each
//!   present at its committed length (content is the seal's and the
//!   scrub's to prove, see [`crate::sealed_generation::scrub`]);
//! - every listed overlay is read once, hashed, decoded and handed to the
//!   visitor; an overlay file the seal did not list is refused;
//! - the text-authority manifest is read once, hashed and decoded, must
//!   describe exactly the listed shards, and the directory must hold
//!   nothing else; every shard is then read once, hashed and decoded as it
//!   is loaded and handed to the visitor.
//!
//! The visitor decides what to keep: the open keeps everything and becomes
//! a searcher, the validator keeps nothing. Both decode, so a door that
//! admits a generation has run every step a query's open runs.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;
use std::sync::Arc;

use ciborium::Value as CborValue;
use quanta_index_contract::{GenerationSnapshot, SourcePublicationEvent};
use quanta_index_core::domains::generation::SealedArtifactCommitmentV1;
use quanta_index_core::{CoreError, RequestBudgetV1};
use sha2::{Digest as _, Sha256};
use tantivy::{Index, IndexReader, ReloadPolicy};

use crate::file_authority;
use crate::overlay_codec::OverlayFamily;
use crate::ranked_keys::{self, MAX_RANKED_KEYS_BYTES, RankedKeyTables, SegmentKeys};
use crate::sealed_generation::coverage::{
    CoverageArtifact, CoverageDecodeCache, CoverageSnapshot, LexicalCoverageReadStats,
    SOURCE_FILE_COVERAGE_FILE_NAME, decode_coverage_at,
};
use crate::sealed_generation::index_directory::MAX_INDEX_CONTROL_BYTES;
use crate::sealed_generation::index_files::referenced_index_files_at;
use crate::sealed_generation::live_bm25::LiveBm25Statistics;
use crate::sealed_generation::manifest::{LexicalSealedManifest, read_bound_manifest_at};
use crate::text_authority::{
    MAX_MANIFEST_BYTES, ShardBody, TEXT_AUTHORITY_DIR_NAME, TEXT_AUTHORITY_MANIFEST_FILE_NAME,
    TextAuthorityManifest, sha256_of_bytes, text_authority_dir,
};
use crate::{OverlaySnapshot, TANTIVY_INDEX_META_FILE_NAME};

/// What a door does with each decoded file.
pub(crate) trait SealedGenerationVisitor {
    /// One text-authority shard, proved and decoded, in ascending index
    /// order.
    fn text_authority_shard(
        &mut self,
        shard: crate::text_authority::ProvedTextShard,
        body: ShardBody,
    ) -> Result<(), CoreError>;

    fn text_authority_complete(
        &mut self,
        _root: &File,
        _directory: &Path,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    /// Verified full-file sources. Only a query-open visitor builds the
    /// in-memory search index; validation visitors discard these bytes.
    fn file_authority(
        &mut self,
        authority: file_authority::VerifiedAuthority,
        object_dir: std::path::PathBuf,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError>;

    /// One overlay family's snapshot, proved and decoded, in family order.
    fn overlay(&mut self, snapshot: OverlaySnapshot) -> Result<(), CoreError>;
}

/// The validator's visitor: proves and decodes, keeps nothing.
pub(crate) struct DiscardingVisitor;

impl SealedGenerationVisitor for DiscardingVisitor {
    fn text_authority_shard(
        &mut self,
        _shard: crate::text_authority::ProvedTextShard,
        _body: ShardBody,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    fn file_authority(
        &mut self,
        _authority: file_authority::VerifiedAuthority,
        _object_dir: std::path::PathBuf,
        _budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    fn overlay(&mut self, _snapshot: OverlaySnapshot) -> Result<(), CoreError> {
        Ok(())
    }
}

/// What the walk proved and opened, beyond what the visitor kept.
pub(crate) struct VerifiedGeneration {
    pub(crate) manifest: LexicalSealedManifest,
    /// The reader whose segment set the ranked-key verifier bound. Reusing it
    /// for serving avoids reopening every segment after the door's proof.
    pub(crate) reader: IndexReader,
    pub(crate) ranked_keys: Arc<RankedKeyTables>,
    pub(crate) live_bm25: Arc<LiveBm25Statistics>,
    /// Decoded from this generation's committed artifact; None is unavailable.
    pub(crate) coverage: Option<CoverageSnapshot>,
    pub(crate) source_publication: Option<SourcePublicationEvent>,
    pub(crate) coverage_read_stats: LexicalCoverageReadStats,
}

/// Prove `generation_dir` against the manifest sealed for `identity`.
pub(crate) fn walk_sealed_generation<V: SealedGenerationVisitor>(
    generation_dir: &Path,
    identity: &GenerationSnapshot,
    visitor: &mut V,
    budget: Option<&RequestBudgetV1>,
) -> Result<VerifiedGeneration, CoreError> {
    walk_sealed_generation_reusing_coverage(generation_dir, identity, visitor, None, budget)
}

pub(crate) fn walk_sealed_generation_reusing_coverage<V: SealedGenerationVisitor>(
    generation_dir: &Path,
    identity: &GenerationSnapshot,
    visitor: &mut V,
    cache: Option<&mut CoverageDecodeCache>,
    budget: Option<&RequestBudgetV1>,
) -> Result<VerifiedGeneration, CoreError> {
    checkpoint(budget, "lexical:cold-open:walk")?;
    let root = super::open_generation_dir_nofollow(generation_dir).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: open sealed generation {}: {error}",
            generation_dir.display()
        ))
    })?;
    walk_sealed_generation_at(&root, generation_dir, identity, visitor, cache, budget)
}

pub(crate) fn walk_sealed_generation_at<V: SealedGenerationVisitor>(
    root: &File,
    generation_dir: &Path,
    identity: &GenerationSnapshot,
    visitor: &mut V,
    cache: Option<&mut CoverageDecodeCache>,
    budget: Option<&RequestBudgetV1>,
) -> Result<VerifiedGeneration, CoreError> {
    checkpoint(budget, "lexical:cold-open:identity")?;
    let observed = crate::index_store::read_lexical_sealed_identity_at(generation_dir, root)?;
    checkpoint(budget, "lexical:cold-open:identity")?;
    crate::index_store::validate_lexical_sealed_identity(&observed, identity)?;
    // A generation the scrub proved corrupt is refused at every door.
    crate::sealed_generation::refuse_if_quarantined_at(root, generation_dir)?;
    let manifest = read_bound_manifest_at(generation_dir, root, &identity.manifest_digest)?;
    if !matches!(usize::try_from(manifest.index_meta.bytes), Ok(bytes) if bytes <= MAX_INDEX_CONTROL_BYTES)
    {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &manifest.index_meta.name,
            "index commit exceeds the control-file byte limit",
        ));
    }
    checkpoint(budget, "lexical:cold-open:manifest")?;
    let meta_bytes = read_committed(root, generation_dir, &manifest.index_meta, budget)?;
    let index = crate::index_store::open_sealed_index_at(generation_dir, root, meta_bytes)?;
    checkpoint(budget, "lexical:cold-open:index")?;
    verify_index_segments(root, generation_dir, &index, &manifest.index_segments)?;
    let (ranked_keys, reader) =
        verify_ranked_keys(root, generation_dir, &index, &manifest.ranked_keys, budget)?;
    checkpoint(budget, "lexical:cold-open:live-bm25")?;
    let live_bytes = read_committed(root, generation_dir, &manifest.live_bm25, budget)?;
    let live_bm25 = Arc::new(LiveBm25Statistics::decode(
        &live_bytes,
        generation_dir,
        manifest.index_meta.sha256,
        &reader.searcher(),
    )?);
    verify_overlays(root, generation_dir, &manifest, visitor, budget)?;
    verify_text_authority(
        root,
        generation_dir,
        manifest.text_authority.as_deref(),
        visitor,
        budget,
    )?;
    checkpoint(budget, "lexical:cold-open:coverage")?;
    let coverage = match cache {
        Some(cache) => verify_source_coverage_reusing(
            root,
            generation_dir,
            identity,
            manifest.source_coverage.as_ref(),
            Some(cache),
        ),
        None => verify_source_coverage_reusing(
            root,
            generation_dir,
            identity,
            manifest.source_coverage.as_ref(),
            None,
        ),
    }?;
    checkpoint(budget, "lexical:cold-open:coverage")?;
    let (coverage, source_publication, coverage_read_stats) = coverage.map_or(
        (None, None, LexicalCoverageReadStats::default()),
        |artifact| {
            (
                Some(artifact.coverage),
                Some(artifact.publication),
                artifact.read_stats,
            )
        },
    );
    verify_file_authority(
        root,
        generation_dir,
        &manifest.file_authority,
        coverage.as_ref(),
        visitor,
        budget,
    )?;
    checkpoint(budget, "lexical:cold-open:walk")?;
    Ok(VerifiedGeneration {
        manifest,
        reader,
        ranked_keys,
        live_bm25,
        coverage,
        source_publication,
        coverage_read_stats,
    })
}

fn verify_file_authority<V: SealedGenerationVisitor>(
    root: &File,
    generation_dir: &Path,
    committed: &[SealedArtifactCommitmentV1],
    coverage: Option<&quanta_index_contract::FileCoverageSnapshot>,
    visitor: &mut V,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    checkpoint(budget, "lexical:cold-open:file-authority")?;
    let root_name = format!("{}/{}", file_authority::DIR, file_authority::ROOT);
    let by_name: BTreeMap<&str, &SealedArtifactCommitmentV1> = committed
        .iter()
        .map(|artifact| (artifact.name.as_str(), artifact))
        .collect();
    if by_name.len() != committed.len() {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            file_authority::DIR,
            "duplicate file authority commitment",
        ));
    }
    let root_commitment = by_name.get(root_name.as_str()).copied().ok_or_else(|| {
        crate::index_store::sidecar_corrupt(generation_dir, &root_name, "F15 root is not committed")
    })?;
    if root_commitment.bytes > file_authority::max_root_bytes() {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &root_name,
            "F15 root exceeds byte policy",
        ));
    }
    let root_bytes = read_committed(root, generation_dir, root_commitment, budget)?;
    let mut read_failure = None;
    let verified_result = file_authority::verify_v15(&root_bytes, |digest, expected_len| {
        let name = file_authority::object_name(&digest);
        let commitment = by_name
            .get(name.as_str())
            .copied()
            .ok_or_else(|| format!("object {name} is absent from outer manifest"))?;
        if commitment.bytes != expected_len || commitment.sha256 != digest {
            return Err(format!("object {name} differs from root descriptor"));
        }
        file_authority::read_object_pinned(
            &generation_dir
                .join(file_authority::DIR)
                .join(file_authority::OBJECTS),
            digest,
            expected_len,
            budget,
        )
        .map_err(|error| {
            read_failure = Some(error);
            format!("object {name} read failed")
        })
    });
    if let Some(error) = read_failure {
        return Err(error);
    }
    let verified = verified_result.map_err(|reason| {
        crate::index_store::sidecar_corrupt(generation_dir, &root_name, &reason)
    })?;
    checkpoint(budget, "lexical:cold-open:file-census")?;
    let expected = file_authority::verified_inventory(&verified);
    if expected.len() != committed.len()
        || !expected
            .keys()
            .map(String::as_str)
            .eq(by_name.keys().copied())
        || expected.iter().any(|(name, len)| {
            name != &root_name
                && by_name
                    .get(name.as_str())
                    .is_none_or(|row| row.bytes != *len)
        })
    {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            file_authority::DIR,
            "outer manifest object inventory differs from F15 root",
        ));
    }
    let coverage = coverage.ok_or_else(|| {
        crate::index_store::sidecar_corrupt(
            generation_dir,
            file_authority::DIR,
            "source coverage is missing",
        )
    })?;
    if !file_authority::verified_matches_coverage(&verified, coverage) {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            file_authority::DIR,
            "F15 source metadata differs from coverage",
        ));
    }
    // List through pinned nofollow directory descriptors, including nested objects.
    let authority_dir = open_child_directory(root, file_authority::DIR).map_err(|error| {
        crate::index_store::sidecar_corrupt(
            generation_dir,
            file_authority::DIR,
            &format!("open directory: {error}"),
        )
    })?;
    let top: BTreeSet<String> = super::entry_names_at(&authority_dir, None)
        .map_err(|error| CoreError::Storage(format!("lexical: list F15 authority: {error}")))?
        .into_iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    if top
        != BTreeSet::from([
            file_authority::ROOT.to_owned(),
            file_authority::OBJECTS.to_owned(),
        ])
    {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            file_authority::DIR,
            "F15 authority directory has uncommitted entries",
        ));
    }
    let object_dir =
        open_child_directory(&authority_dir, file_authority::OBJECTS).map_err(|error| {
            crate::index_store::sidecar_corrupt(
                generation_dir,
                file_authority::OBJECTS,
                &format!("open object directory: {error}"),
            )
        })?;
    let actual_objects: BTreeSet<String> = super::entry_names_at(&object_dir, None)
        .map_err(|error| CoreError::Storage(format!("lexical: list F15 objects: {error}")))?
        .into_iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    let object_prefix = format!("{}/{}/", file_authority::DIR, file_authority::OBJECTS);
    let expected_objects: BTreeSet<String> = expected
        .keys()
        .filter_map(|name| name.strip_prefix(&object_prefix).map(str::to_owned))
        .collect();
    if actual_objects != expected_objects {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            file_authority::DIR,
            "F15 object directory differs from root",
        ));
    }
    let object_path = generation_dir
        .join(file_authority::DIR)
        .join(file_authority::OBJECTS);
    visitor.file_authority(verified, object_path, budget)?;
    checkpoint(budget, "lexical:cold-open:file-authority")
}

fn open_child_directory(parent: &File, name: &str) -> std::io::Result<File> {
    use rustix::fs::{Mode, OFlags, openat};
    let descriptor = openat(
        parent,
        Path::new(name),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
    Ok(File::from(descriptor))
}

fn checkpoint(budget: Option<&RequestBudgetV1>, stage: &'static str) -> Result<(), CoreError> {
    budget.map_or(Ok(()), |budget| budget.checkpoint(stage))
}

fn verify_ranked_keys(
    root: &File,
    generation_dir: &Path,
    index: &Index,
    commitments: &[SealedArtifactCommitmentV1],
    budget: Option<&RequestBudgetV1>,
) -> Result<(Arc<RankedKeyTables>, IndexReader), CoreError> {
    checkpoint(budget, "lexical:cold-open:ranked-keys")?;
    let reader: IndexReader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|error| {
            CoreError::Storage(format!("lexical: ranked-key verifier reader: {error}"))
        })?;
    let searcher = reader.searcher();
    if commitments.len() != searcher.segment_readers().len() {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            "ranked keys",
            "segment count mismatch",
        ));
    }
    let by_name: std::collections::BTreeMap<&str, &SealedArtifactCommitmentV1> = commitments
        .iter()
        .map(|artifact| (artifact.name.as_str(), artifact))
        .collect();
    let mut total = 0_u64;
    let resident_limit = u64::try_from(MAX_RANKED_KEYS_BYTES).map_err(|error| {
        CoreError::Storage(format!("lexical: ranked-key limit overflow: {error}"))
    })?;
    let mut tables = Vec::new();
    for segment in searcher.segment_readers() {
        checkpoint(budget, "lexical:cold-open:ranked-keys")?;
        let name = ranked_keys::file_name(segment);
        let commitment = by_name.get(name.as_str()).copied().ok_or_else(|| {
            crate::index_store::sidecar_corrupt(generation_dir, &name, "missing commitment")
        })?;
        total = total.checked_add(commitment.bytes).ok_or_else(|| {
            CoreError::Storage("lexical: ranked-key resident bytes overflow".into())
        })?;
        if total > resident_limit {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                &name,
                "resident table exceeds limit",
            ));
        }
        let bytes = read_committed(root, generation_dir, commitment, budget)?;
        tables.push(Arc::new(SegmentKeys::decode(bytes, segment)?));
        checkpoint(budget, "lexical:cold-open:ranked-keys-decode")?;
    }
    let tables = RankedKeyTables::bind(tables, searcher.segment_readers())?;
    // A stale or uncommitted table is never silently ignored.
    for name in super::entry_names_at(root, None).map_err(|error| {
        CoreError::Storage(format!("lexical: list ranked-key directory: {error}"))
    })? {
        let name = name.to_string_lossy();
        if ranked_keys::is_ranked_key_entry(&name) && !by_name.contains_key(name.as_ref()) {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                &name,
                "uncommitted ranked-key file",
            ));
        }
    }
    Ok((Arc::new(tables), reader))
}

/// Verify and decode the same bytes. A missing committed artifact or an
/// uncommitted extra artifact is corruption, not an empty file universe.
#[cfg(test)]
pub(crate) fn verify_source_coverage(
    generation_dir: &Path,
    identity: &GenerationSnapshot,
    committed: Option<&SealedArtifactCommitmentV1>,
) -> Result<Option<CoverageArtifact>, CoreError> {
    let root = super::open_generation_dir_nofollow(generation_dir).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: open coverage generation {}: {error}",
            generation_dir.display()
        ))
    })?;
    verify_source_coverage_reusing(&root, generation_dir, identity, committed, None)
}

fn verify_source_coverage_reusing(
    root: &File,
    generation_dir: &Path,
    identity: &GenerationSnapshot,
    committed: Option<&SealedArtifactCommitmentV1>,
    cache: Option<&mut CoverageDecodeCache>,
) -> Result<Option<CoverageArtifact>, CoreError> {
    if let Some(artifact) = committed {
        if artifact.bytes > super::coverage::MAX_COVERAGE_ROOT_BYTES_U64 {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                SOURCE_FILE_COVERAGE_FILE_NAME,
                "coverage root exceeds its byte ceiling",
            ));
        }
        let bytes =
            super::coverage::read_committed_coverage_root_at(root, generation_dir, artifact)?;
        return cache
            .map_or_else(
                || decode_coverage_at(root, &bytes, generation_dir, identity),
                |cache| cache.decode_at(root, &bytes, generation_dir, identity),
            )
            .map(Some);
    }
    match super::optional_entry_at(root, Path::new(SOURCE_FILE_COVERAGE_FILE_NAME)) {
        Ok(true) => Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            SOURCE_FILE_COVERAGE_FILE_NAME,
            "coverage exists without a manifest commitment",
        )),
        Ok(false) => {
            for name in super::entry_names_at(root, None)
                .map_err(|error| CoreError::Storage(error.to_string()))?
            {
                if super::coverage::is_coverage_page(&name.to_string_lossy()) {
                    return Err(crate::index_store::sidecar_corrupt(
                        generation_dir,
                        SOURCE_FILE_COVERAGE_FILE_NAME,
                        "coverage page exists without a root commitment",
                    ));
                }
            }
            Ok(None)
        }
        Err(error) => Err(CoreError::Storage(format!(
            "lexical: inspect source coverage under {}: {error}",
            generation_dir.display()
        ))),
    }
}

/// Read one committed file whole and prove its length and digest.
pub(crate) fn read_committed(
    root: &File,
    generation_dir: &Path,
    artifact: &SealedArtifactCommitmentV1,
    budget: Option<&RequestBudgetV1>,
) -> Result<Vec<u8>, CoreError> {
    checkpoint(budget, "lexical:cold-open:file-read")?;
    let path = generation_dir.join(&artifact.name);
    let mut opened =
        super::open_regular_below(root, Path::new(&artifact.name)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound
                || super::is_unsafe_artifact_path(&error)
            {
                crate::index_store::sidecar_corrupt(
                    generation_dir,
                    &artifact.name,
                    "changed before open",
                )
            } else {
                CoreError::Storage(format!(
                    "lexical: read committed file {}: {error}",
                    path.display()
                ))
            }
        })?;
    let opened_metadata = opened.metadata().map_err(|error| {
        CoreError::Storage(format!(
            "lexical: inspect opened committed file {}: {error}",
            path.display()
        ))
    })?;
    if !opened_metadata.is_file() || opened_metadata.len() != artifact.bytes {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &artifact.name,
            "opened file differs from the committed length",
        ));
    }
    let admitted_len = usize::try_from(artifact.bytes).map_err(|error| {
        CoreError::Storage(format!("lexical: committed read length overflow: {error}"))
    })?;
    let bytes = super::coverage::read_admitted_bytes(&mut opened, admitted_len, &path)?;
    checkpoint(budget, "lexical:cold-open:file-read")?;
    let length = crate::channel_payloads::count_from_len(bytes.len())?;
    if length != artifact.bytes {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &artifact.name,
            &format!("{length} bytes on disk, {} committed", artifact.bytes),
        ));
    }
    let digest = if budget.is_some() {
        let mut hasher = Sha256::new();
        for chunk in bytes.chunks(64 * 1024) {
            checkpoint(budget, "lexical:cold-open:file-hash")?;
            hasher.update(chunk);
        }
        <[u8; 32]>::from(hasher.finalize())
    } else {
        sha256_of_bytes(&bytes)
    };
    if digest != artifact.sha256 {
        let reason = if artifact.name == TANTIVY_INDEX_META_FILE_NAME {
            "index commit differs from the sealed commit"
        } else {
            "content digest differs from the committed digest"
        };
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &artifact.name,
            reason,
        ));
    }
    checkpoint(budget, "lexical:cold-open:file-hash")?;
    Ok(bytes)
}

/// The segment files the (already proved) commit references are exactly
/// the listed ones, each present at its committed length.
pub(crate) fn verify_index_segments(
    root: &File,
    generation_dir: &Path,
    index: &Index,
    committed: &[SealedArtifactCommitmentV1],
) -> Result<(), CoreError> {
    let referenced = referenced_index_files_at(index, generation_dir, root)?;
    let listed: BTreeSet<&str> = committed
        .iter()
        .map(|artifact| artifact.name.as_str())
        .collect();
    for name in &referenced {
        if !listed.contains(name.as_str()) {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                name,
                "referenced by the sealed commit although the seal did not commit to it",
            ));
        }
    }
    let referenced: BTreeSet<&str> = referenced.iter().map(String::as_str).collect();
    for artifact in committed {
        if !referenced.contains(artifact.name.as_str()) {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                &artifact.name,
                "committed although the sealed commit does not reference it",
            ));
        }
        let path = generation_dir.join(&artifact.name);
        let metadata = super::open_regular_below(root, Path::new(&artifact.name))
            .and_then(|file| file.metadata())
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound
                    || super::is_unsafe_artifact_path(&error)
                {
                    crate::index_store::sidecar_corrupt(
                        generation_dir,
                        &artifact.name,
                        "missing or unsafe",
                    )
                } else {
                    CoreError::Storage(format!(
                        "lexical: inspect committed segment file {}: {error}",
                        path.display()
                    ))
                }
            })?;
        if !metadata.is_file() {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                &artifact.name,
                "is not a regular file",
            ));
        }
        if metadata.len() != artifact.bytes {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                &artifact.name,
                &format!(
                    "{} bytes on disk, {} committed",
                    metadata.len(),
                    artifact.bytes
                ),
            ));
        }
    }
    Ok(())
}

/// Every listed overlay is read once, proved and decoded; a family the
/// seal did not list must not be on disk.
struct DigestingReader<R> {
    inner: R,
    hasher: Sha256,
    bytes: u64,
    expected: u64,
}

impl<R: Read> Read for DigestingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        let next = self
            .bytes
            .checked_add(u64::try_from(count).map_err(std::io::Error::other)?)
            .ok_or_else(|| std::io::Error::other("overlay read length overflow"))?;
        if next > self.expected {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "overlay grew beyond committed length",
            ));
        }
        let consumed = buffer
            .get(..count)
            .ok_or_else(|| std::io::Error::other("overlay reader exceeded its buffer"))?;
        self.hasher.update(consumed);
        self.bytes = next;
        Ok(count)
    }
}

/// Authenticate the opened inode with bounded scratch before parsing. The
/// decoder hashes again, so an in-place mutation between passes cannot publish
/// bytes other than the ones committed by the manifest.
fn read_committed_overlay_value(
    root: &File,
    generation_dir: &Path,
    artifact: &SealedArtifactCommitmentV1,
    budget: Option<&RequestBudgetV1>,
) -> Result<CborValue, CoreError> {
    checkpoint(budget, "lexical:cold-open:overlay")?;
    let path = generation_dir.join(&artifact.name);
    let mut file = super::open_regular_below(root, Path::new(&artifact.name)).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound || super::is_unsafe_artifact_path(&error) {
            crate::index_store::sidecar_corrupt(generation_dir, &artifact.name, "missing or unsafe")
        } else {
            CoreError::Storage(format!("lexical: open overlay {}: {error}", path.display()))
        }
    })?;
    let length = file
        .metadata()
        .map_err(|error| {
            CoreError::Storage(format!("lexical: stat overlay {}: {error}", path.display()))
        })?
        .len();
    if length != artifact.bytes {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &artifact.name,
            "opened overlay differs from committed length",
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut admitted = (&mut file).take(artifact.bytes);
    loop {
        checkpoint(budget, "lexical:cold-open:overlay-hash")?;
        let count = admitted.read(&mut buffer).map_err(|error| {
            CoreError::Storage(format!("lexical: hash overlay {}: {error}", path.display()))
        })?;
        if count == 0 {
            break;
        }
        let consumed = buffer
            .get(..count)
            .ok_or_else(|| CoreError::Storage("lexical: overlay hash buffer overflow".into()))?;
        hasher.update(consumed);
    }
    if admitted.limit() != 0 {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &artifact.name,
            "overlay shrank while hashing",
        ));
    }
    let mut sentinel = [0_u8; 1];
    if file.read(&mut sentinel).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: finish overlay hash {}: {error}",
            path.display()
        ))
    })? != 0
    {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &artifact.name,
            "overlay grew while hashing",
        ));
    }
    if <[u8; 32]>::from(hasher.finalize()) != artifact.sha256 {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &artifact.name,
            "content digest differs from the committed digest",
        ));
    }
    file.rewind().map_err(|error| {
        CoreError::Storage(format!(
            "lexical: rewind overlay {}: {error}",
            path.display()
        ))
    })?;
    let mut reader = DigestingReader {
        inner: BufReader::with_capacity(64 * 1024, file),
        hasher: Sha256::new(),
        bytes: 0,
        expected: artifact.bytes,
    };
    checkpoint(budget, "lexical:cold-open:overlay-decode")?;
    let value =
        ciborium::from_reader::<CborValue, _>(&mut reader).map_err(|error| match error {
            ciborium::de::Error::Io(io_error)
                if !matches!(
                    io_error.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::InvalidData
                ) =>
            {
                CoreError::Storage(format!(
                    "lexical: read overlay {}: {io_error}",
                    path.display()
                ))
            }
            other @ (ciborium::de::Error::Io(_)
            | ciborium::de::Error::Syntax(_)
            | ciborium::de::Error::Semantic(_, _)
            | ciborium::de::Error::RecursionLimitExceeded) => crate::index_store::sidecar_corrupt(
                generation_dir,
                &artifact.name,
                &format!("decode committed CBOR: {other}"),
            ),
        })?;
    let mut sentinel = [0_u8; 1];
    match reader.read(&mut sentinel) {
        Ok(0) if reader.bytes == artifact.bytes => {}
        Ok(0) => {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                &artifact.name,
                "overlay has missing bytes",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                &artifact.name,
                "overlay grew while decoding",
            ));
        }
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "lexical: finish overlay read {}: {error}",
                path.display()
            )));
        }
        Ok(_) => {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                &artifact.name,
                "overlay has trailing CBOR bytes",
            ));
        }
    }
    if <[u8; 32]>::from(reader.hasher.finalize()) != artifact.sha256 {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &artifact.name,
            "content digest differs from the committed digest",
        ));
    }
    checkpoint(budget, "lexical:cold-open:overlay-decode")?;
    Ok(value)
}

fn verify_overlays<V: SealedGenerationVisitor>(
    root: &File,
    generation_dir: &Path,
    manifest: &LexicalSealedManifest,
    visitor: &mut V,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    for family in OverlayFamily::ALL {
        checkpoint(budget, "lexical:cold-open:overlay")?;
        if let Some(artifact) = manifest.overlay(family) {
            let value = read_committed_overlay_value(root, generation_dir, artifact, budget)?;
            let snapshot =
                crate::overlay_codec::decode_overlay_value(family, value, generation_dir)?;
            checkpoint(budget, "lexical:cold-open:overlay-decode")?;
            visitor.overlay(snapshot)?;
        } else {
            let path = family.path(generation_dir);
            match super::optional_entry_at(root, Path::new(family.file_name())) {
                Ok(true) => {
                    return Err(crate::index_store::sidecar_corrupt(
                        generation_dir,
                        family.file_name(),
                        "present although the seal committed to no such overlay",
                    ));
                }
                Ok(false) => {}
                Err(error) => {
                    return Err(CoreError::Storage(format!(
                        "lexical: inspect absent overlay {}: {error}",
                        path.display()
                    )));
                }
            }
        }
    }
    Ok(())
}

/// The `text-authority/` tree is exactly what the seal listed, and every
/// shard decodes to what its manifest says.
fn verify_text_authority<V: SealedGenerationVisitor>(
    root: &File,
    generation_dir: &Path,
    committed: Option<&[SealedArtifactCommitmentV1]>,
    visitor: &mut V,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    checkpoint(budget, "lexical:cold-open:text-authority")?;
    let dir = text_authority_dir(generation_dir);
    let observed =
        super::optional_entry_at(root, Path::new(TEXT_AUTHORITY_DIR_NAME)).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: inspect text authority {}: {error}",
                dir.display()
            ))
        })?;
    let files = match (committed, observed) {
        (None, false) => return Ok(()),
        (None, true) => {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                TEXT_AUTHORITY_DIR_NAME,
                "present although the seal committed to no text authority",
            ));
        }
        (Some(_), false) => {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                TEXT_AUTHORITY_DIR_NAME,
                "missing",
            ));
        }
        (Some(files), true) => files,
    };
    let manifest_name = format!("{TEXT_AUTHORITY_DIR_NAME}/{TEXT_AUTHORITY_MANIFEST_FILE_NAME}");
    let manifest_commitment = files
        .iter()
        .find(|artifact| artifact.name == manifest_name)
        .ok_or_else(|| {
            crate::index_store::sidecar_corrupt(generation_dir, &manifest_name, "not committed")
        })?;
    if !matches!(usize::try_from(manifest_commitment.bytes), Ok(bytes) if bytes <= MAX_MANIFEST_BYTES)
    {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &manifest_name,
            "text-authority manifest exceeds its format byte limit",
        ));
    }
    let manifest_bytes = read_committed(root, generation_dir, manifest_commitment, budget)?;
    let manifest = TextAuthorityManifest::decode(&manifest_bytes, generation_dir)?;
    checkpoint(budget, "lexical:cold-open:text-manifest-decode")?;
    ensure_text_authority_listing(generation_dir, &manifest, &manifest_name, files)?;
    ensure_text_authority_directory(root, generation_dir, &dir, files)?;
    for entry in &manifest.shards {
        checkpoint(budget, "lexical:cold-open:text-shard")?;
        let (body, file) =
            crate::text_authority::load_shard_file_at(root, generation_dir, entry, budget)?;
        checkpoint(budget, "lexical:cold-open:text-shard")?;
        visitor.text_authority_shard(
            crate::text_authority::ProvedTextShard {
                entry: entry.clone(),
                file,
            },
            body,
        )?;
    }
    visitor.text_authority_complete(root, generation_dir)
}

/// The sealed manifest's text-authority section and the text-authority
/// manifest describe the same shard files, byte for byte.
fn ensure_text_authority_listing(
    generation_dir: &Path,
    manifest: &TextAuthorityManifest,
    manifest_name: &str,
    committed: &[SealedArtifactCommitmentV1],
) -> Result<(), CoreError> {
    let mut expected: Vec<(String, u64, [u8; 32])> = manifest
        .shards
        .iter()
        .map(|shard| {
            (
                format!("{TEXT_AUTHORITY_DIR_NAME}/{}", shard.file_name()),
                shard.bytes,
                shard.sha256,
            )
        })
        .collect();
    expected.sort();
    let mut listed: Vec<(String, u64, [u8; 32])> = committed
        .iter()
        .filter(|artifact| artifact.name != manifest_name)
        .map(|artifact| (artifact.name.clone(), artifact.bytes, artifact.sha256))
        .collect();
    listed.sort();
    if expected != listed {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            manifest_name,
            "lists shards other than the ones the seal committed to",
        ));
    }
    Ok(())
}

/// The `text-authority/` directory holds exactly the committed files.
fn ensure_text_authority_directory(
    root: &File,
    generation_dir: &Path,
    dir: &Path,
    committed: &[SealedArtifactCommitmentV1],
) -> Result<(), CoreError> {
    let prefix = format!("{TEXT_AUTHORITY_DIR_NAME}/");
    let owned: BTreeSet<&str> = committed
        .iter()
        .filter_map(|artifact| artifact.name.strip_prefix(prefix.as_str()))
        .collect();
    for name in
        super::entry_names_at(root, Some(OsStr::new(TEXT_AUTHORITY_DIR_NAME))).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound
                || super::is_unsafe_artifact_path(&error)
            {
                crate::index_store::sidecar_corrupt(
                    generation_dir,
                    TEXT_AUTHORITY_DIR_NAME,
                    "missing or not a directory",
                )
            } else {
                CoreError::Storage(format!(
                    "lexical: list text authority directory {}: {error}",
                    dir.display()
                ))
            }
        })?
    {
        let name = name.to_string_lossy();
        let qualified = format!("{prefix}{name}");
        if !owned.contains(name.as_ref()) {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                &qualified,
                "present although the seal did not commit to it",
            ));
        }
        let _file = super::open_regular_below(root, Path::new(&qualified)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound
                || super::is_unsafe_artifact_path(&error)
            {
                crate::index_store::sidecar_corrupt(
                    generation_dir,
                    &qualified,
                    "is not a regular file",
                )
            } else {
                CoreError::Storage(format!(
                    "lexical: inspect text authority entry {}: {error}",
                    dir.join(name.as_ref()).display()
                ))
            }
        })?;
    }
    Ok(())
}
