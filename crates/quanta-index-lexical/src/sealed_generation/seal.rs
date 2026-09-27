//! The seal: measure a finalized generation and write its manifest.
//!
//! Runs after the index writer has committed, waited for its merges and
//! been released, after the text authority has been brought to its
//! manifest's file set, and before the sealed identity is written — so a
//! crash in between leaves an unsealed generation, never a sealed one
//! without a manifest.
//!
//! The seal reads bytes proportional to what the generation changed, not
//! to its size (QI-BB-006 보완 #4): a file whose inode is the base
//! generation's — an index segment, a text-authority shard or an overlay
//! the delta inherited by hard link — carries the base manifest's
//! commitment without being read, because the base seal proved that very
//! inode; everything else — a new segment, a rewritten shard, the
//! rewritten `meta.json`, a rewritten overlay — is read once through a
//! counting hasher. A fresh generation is therefore proved whole at its
//! seal, and every later delta proves only what it wrote.
//!
//! The measurement is reported per seal so a test can hold the seal to
//! that shape with an inode oracle instead of a clock.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use quanta_index_contract::GenerationSnapshot;
use quanta_index_core::CoreError;
use quanta_index_core::domains::generation::SealedArtifactCommitmentV1;
use sha2::{Digest, Sha256};
use tantivy::collector::Count;
use tantivy::query::TermQuery;
use tantivy::schema::IndexRecordOption;
use tantivy::{Index, IndexReader, ReloadPolicy, Term};

use crate::normalize::TEXT_NORMALIZER_VERSION;
use crate::overlay_codec::OverlayFamily;
use crate::ranked_keys::{self, MAX_RANKED_KEYS_BYTES};
use crate::sealed_generation::coverage::SOURCE_FILE_COVERAGE_FILE_NAME;
use crate::sealed_generation::index_files::referenced_index_files;
use crate::sealed_generation::manifest::{
    IndexSegmentVerificationV1, LexicalSealedManifest, manifest_path, read_manifest, write_manifest,
};
use crate::sealed_generation::verify::verify_source_coverage;
use crate::text_authority::{
    TEXT_AUTHORITY_DIR_NAME, TEXT_AUTHORITY_MANIFEST_FILE_NAME, TextAuthorityManifest,
    finalize_for_seal,
};
use crate::{SchemaFields, TANTIVY_INDEX_META_FILE_NAME, TEXT_DOC_KIND};

/// What the seals of one adapter read to commit their generations
/// (QI-BB-006 보완 #4).
///
/// Every committed file is counted under exactly one of two sources.
/// `bytes_hashed` is what the seals actually read through their hasher;
/// `bytes_inherited` is what they did not have to read.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LexicalSealCommitmentStats {
    /// Generations sealed.
    pub seals: u64,
    /// Files read and hashed by a seal.
    pub files_hashed: u64,
    /// Bytes read through the seal's hasher.
    pub bytes_hashed: u64,
    /// Files whose commitment was carried from the base generation's
    /// manifest because the file is the base's inode.
    pub files_inherited: u64,
    /// Their length, not read.
    pub bytes_inherited: u64,
}

impl LexicalSealCommitmentStats {
    /// Fold one seal's measurement into the running totals.
    pub(crate) fn absorb(&mut self, seal: Self) {
        self.seals = self.seals.saturating_add(seal.seals);
        self.files_hashed = self.files_hashed.saturating_add(seal.files_hashed);
        self.bytes_hashed = self.bytes_hashed.saturating_add(seal.bytes_hashed);
        self.files_inherited = self.files_inherited.saturating_add(seal.files_inherited);
        self.bytes_inherited = self.bytes_inherited.saturating_add(seal.bytes_inherited);
    }

    fn hashed(&mut self, bytes: u64) {
        self.files_hashed = self.files_hashed.saturating_add(1);
        self.bytes_hashed = self.bytes_hashed.saturating_add(bytes);
    }

    fn inherited(&mut self, bytes: u64) {
        self.files_inherited = self.files_inherited.saturating_add(1);
        self.bytes_inherited = self.bytes_inherited.saturating_add(bytes);
    }
}

/// The base generation's commitments, for inheritance by inode.
struct BaseCommitments {
    dir: PathBuf,
    by_name: BTreeMap<String, SealedArtifactCommitmentV1>,
}

impl BaseCommitments {
    /// The base's manifest, if the base is sealed.
    ///
    /// An unsealed base (a delta on a generation whose own seal never came)
    /// or a base already reclaimed has nothing to inherit from; a base
    /// whose manifest cannot be read is an error, not a reason to hash
    /// quietly.
    fn read(base_dir: Option<&Path>) -> Result<Option<Self>, CoreError> {
        let Some(base_dir) = base_dir else {
            return Ok(None);
        };
        if !manifest_path(base_dir).is_file() {
            return Ok(None);
        }
        let manifest = read_manifest(base_dir)?;
        let by_name = manifest
            .all_commitments()
            .map(|artifact| (artifact.name.clone(), artifact.clone()))
            .collect();
        Ok(Some(Self {
            dir: base_dir.to_path_buf(),
            by_name,
        }))
    }

    /// The base's commitment for `name`, if `path` is the very inode the
    /// base committed to.
    fn inherited(
        &self,
        name: &str,
        path: &Path,
    ) -> Result<Option<SealedArtifactCommitmentV1>, CoreError> {
        let Some(listed) = self.by_name.get(name) else {
            return Ok(None);
        };
        let ours = std::fs::metadata(path).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: inspect {} for commitment: {error}",
                path.display()
            ))
        })?;
        let theirs = match std::fs::metadata(self.dir.join(name)) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: inspect base generation entry {} for commitment: {error}",
                    self.dir.join(name).display()
                )));
            }
        };
        let same_inode = ours.dev() == theirs.dev() && ours.ino() == theirs.ino();
        if same_inode && ours.len() == listed.bytes {
            return Ok(Some(listed.clone()));
        }
        Ok(None)
    }
}

/// One seal in progress: its measurement and where it may inherit from.
struct Measurer {
    generation_dir: PathBuf,
    base: Option<BaseCommitments>,
    stats: LexicalSealCommitmentStats,
}

impl Measurer {
    /// Length and SHA-256 of `name`, read once through a fixed buffer and
    /// counted.
    fn hash(&mut self, name: &str) -> Result<SealedArtifactCommitmentV1, CoreError> {
        let path = self.generation_dir.join(name);
        let mut file = File::open(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                crate::index_store::sidecar_corrupt(&self.generation_dir, name, "missing")
            } else {
                CoreError::Storage(format!(
                    "lexical: open {} for commitment: {error}",
                    path.display()
                ))
            }
        })?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 1 << 16];
        let mut length = 0_u64;
        loop {
            let read = file.read(&mut buffer).map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: read {} for commitment: {error}",
                    path.display()
                ))
            })?;
            if read == 0 {
                break;
            }
            let chunk = buffer.get(..read).ok_or_else(|| {
                CoreError::Storage(format!(
                    "lexical: read {} for commitment: read returned more bytes than the buffer holds",
                    path.display()
                ))
            })?;
            hasher.update(chunk);
            length = length
                .checked_add(crate::channel_payloads::count_from_len(read)?)
                .ok_or_else(|| {
                    CoreError::Storage(format!("lexical: {} length overflows u64", path.display()))
                })?;
        }
        self.stats.hashed(length);
        Ok(SealedArtifactCommitmentV1 {
            name: name.to_string(),
            bytes: length,
            sha256: hasher.finalize().into(),
        })
    }

    /// The commitment for `name`: the base's if the file is the base's
    /// inode, otherwise measured.
    fn commit(&mut self, name: &str) -> Result<SealedArtifactCommitmentV1, CoreError> {
        let path = self.generation_dir.join(name);
        if let Some(base) = &self.base
            && let Some(inherited) = base.inherited(name, &path)?
        {
            self.stats.inherited(inherited.bytes);
            return Ok(inherited);
        }
        self.hash(name)
    }
}

/// Measure `generation_dir` as sealed and write its manifest.
///
/// `base_dir` is the delta base the generation was carried forward from,
/// if any. Returns what this seal read and did not have to read.
pub(crate) fn seal_generation(
    generation_dir: &Path,
    fields: &SchemaFields,
    identity: &GenerationSnapshot,
    base_dir: Option<&Path>,
) -> Result<LexicalSealCommitmentStats, CoreError> {
    remove_publish_leftovers(generation_dir)?;
    let mut measurer = Measurer {
        generation_dir: generation_dir.to_path_buf(),
        base: BaseCommitments::read(base_dir)?,
        stats: LexicalSealCommitmentStats {
            seals: 1,
            ..LexicalSealCommitmentStats::default()
        },
    };
    let index = crate::index_store::open_sealed_index(generation_dir)?;
    let index_meta = measurer.hash(TANTIVY_INDEX_META_FILE_NAME)?;
    let mut index_segments = Vec::new();
    for name in referenced_index_files(&index, generation_dir)? {
        index_segments.push(measurer.commit(&name)?);
    }
    let ranked_keys = commit_ranked_keys(&index, &mut measurer)?;
    let text_authority_manifest = finalize_for_seal(generation_dir)?;
    ensure_text_authority_covers_index(
        &index,
        generation_dir,
        fields,
        text_authority_manifest.as_ref(),
    )?;
    let text_authority = text_authority_manifest
        .as_ref()
        .map(|manifest| commit_text_authority(&mut measurer, manifest))
        .transpose()?;
    let mut overlays = Vec::new();
    for family in OverlayFamily::ALL {
        if family.path(generation_dir).is_file() {
            overlays.push(measurer.commit(family.file_name())?);
        }
    }
    let source_coverage =
        match std::fs::symlink_metadata(generation_dir.join(SOURCE_FILE_COVERAGE_FILE_NAME)) {
            Ok(metadata) if metadata.is_file() => {
                Some(measurer.hash(SOURCE_FILE_COVERAGE_FILE_NAME)?)
            }
            Ok(_) => {
                return Err(crate::index_store::sidecar_corrupt(
                    generation_dir,
                    SOURCE_FILE_COVERAGE_FILE_NAME,
                    "coverage is not a regular file",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: inspect coverage before sealing: {error}",
                )));
            }
        };
    let _coverage = verify_source_coverage(generation_dir, identity, source_coverage.as_ref())?;
    let manifest = LexicalSealedManifest {
        manifest_digest: identity.manifest_digest.clone(),
        normalizer: TEXT_NORMALIZER_VERSION,
        index_meta,
        index_segment_verification: IndexSegmentVerificationV1::LengthAtOpenContentAtSealAndScrub,
        index_segments,
        ranked_keys,
        text_authority,
        overlays,
        source_coverage,
    };
    write_manifest(generation_dir, &manifest)?;
    Ok(measurer.stats)
}

/// Build only new segment tables; a delta inherits the exact inode and
/// commitment of an unchanged segment's table. Remove tables for merged or
/// deleted segments so an unlisted sidecar cannot masquerade as authority.
fn commit_ranked_keys(
    index: &Index,
    measurer: &mut Measurer,
) -> Result<Vec<SealedArtifactCommitmentV1>, CoreError> {
    let reader: IndexReader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|error| CoreError::Storage(format!("lexical: ranked-key seal reader: {error}")))?;
    reader
        .reload()
        .map_err(|error| CoreError::Storage(format!("lexical: ranked-key seal reload: {error}")))?;
    let searcher = reader.searcher();
    let mut segments: Vec<(String, &tantivy::SegmentReader)> = searcher
        .segment_readers()
        .iter()
        .map(|segment| (ranked_keys::file_name(segment), segment))
        .collect();
    segments.sort_by(|left, right| left.0.cmp(&right.0));
    if segments.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(CoreError::Storage(
            "lexical: duplicate ranked-key segment".into(),
        ));
    }
    let expected: std::collections::BTreeSet<&str> =
        segments.iter().map(|(name, _)| name.as_str()).collect();
    for entry in std::fs::read_dir(&measurer.generation_dir)
        .map_err(|error| CoreError::Storage(format!("lexical: list ranked-key files: {error}")))?
    {
        let entry = entry.map_err(|error| {
            CoreError::Storage(format!("lexical: read ranked-key entry: {error}"))
        })?;
        let entry_name = entry.file_name();
        let Some(entry_name) = entry_name.to_str() else {
            continue;
        };
        if ranked_keys::is_ranked_key_entry(entry_name) && !expected.contains(entry_name) {
            std::fs::remove_file(entry.path()).map_err(|error| {
                CoreError::Storage(format!("lexical: remove stale ranked-key table: {error}"))
            })?;
        }
    }
    let mut commitments = Vec::new();
    let mut total = 0_u64;
    for (name, segment) in segments {
        let path = measurer.generation_dir.join(&name);
        let existing = match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => true,
            Ok(_) => {
                return Err(crate::index_store::sidecar_corrupt(
                    &measurer.generation_dir,
                    &name,
                    "ranked-key table is not a regular file",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: inspect ranked-key table {}: {error}",
                    path.display()
                )));
            }
        };
        let inherited = if existing {
            measurer
                .base
                .as_ref()
                .map(|base| base.inherited(&name, &path))
                .transpose()?
                .flatten()
        } else {
            None
        };
        if inherited.is_none() {
            let bytes = ranked_keys::encode(segment)?;
            crate::index_store::write_atomic_durable(&path, &bytes, "ranked keys")?;
        }
        let commitment = measurer.commit(&name)?;
        total = total
            .checked_add(commitment.bytes)
            .ok_or_else(|| CoreError::Storage("lexical: ranked-key table size overflow".into()))?;
        if total > MAX_RANKED_KEYS_BYTES as u64 {
            return Err(CoreError::Storage(
                "lexical: ranked-key tables exceed resident limit".into(),
            ));
        }
        commitments.push(commitment);
    }
    Ok(commitments)
}

/// The `text-authority/` tree as the seal commits it: the manifest file
/// hashed, each listed shard measured (or inherited by inode) and held to
/// the digest its manifest lists, ascending by name.
///
/// A shard whose bytes are not what the text-authority manifest says
/// refuses the seal: the two manifests can never disagree about what a
/// query will decode.
fn commit_text_authority(
    measurer: &mut Measurer,
    manifest: &TextAuthorityManifest,
) -> Result<Vec<SealedArtifactCommitmentV1>, CoreError> {
    let mut files = Vec::with_capacity(manifest.shards.len().saturating_add(1));
    files.push(measurer.hash(&format!(
        "{TEXT_AUTHORITY_DIR_NAME}/{TEXT_AUTHORITY_MANIFEST_FILE_NAME}"
    ))?);
    for shard in &manifest.shards {
        let name = format!("{TEXT_AUTHORITY_DIR_NAME}/{}", shard.file_name());
        let committed = measurer.commit(&name)?;
        if committed.bytes != shard.bytes || committed.sha256 != shard.sha256 {
            return Err(crate::index_store::sidecar_corrupt(
                &measurer.generation_dir,
                &name,
                "content digest differs from the digest the text-authority manifest lists",
            ));
        }
        files.push(committed);
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(files)
}

/// Remove the temporary files an interrupted durable write left behind.
///
/// `write_atomic_durable` names its temporaries `.<file>.tmp-<pid>-<n>`
/// and renames them into place only once fsynced, so any such file at seal
/// time is a crash's leftover with no owner; the seal commits to the file
/// set it measures and must not seal a directory it does not own outright.
fn remove_publish_leftovers(generation_dir: &Path) -> Result<(), CoreError> {
    let mut removed_any = false;
    for entry in std::fs::read_dir(generation_dir).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: list {} before sealing: {error}",
            generation_dir.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            CoreError::Storage(format!(
                "lexical: read entry of {} before sealing: {error}",
                generation_dir.display()
            ))
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !crate::index_store::is_durable_write_temporary(name) {
            continue;
        }
        std::fs::remove_file(entry.path()).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: remove interrupted publish leftover {}: {error}",
                entry.path().display()
            ))
        })?;
        removed_any = true;
    }
    if removed_any {
        File::open(generation_dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: fsync {} after removing publish leftovers: {error}",
                    generation_dir.display()
                ))
            })?;
    }
    Ok(())
}

/// How many live text documents the committed index holds.
fn live_text_doc_count(index: &Index, fields: &SchemaFields) -> Result<u64, CoreError> {
    let reader: IndexReader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|err| CoreError::Storage(format!("lexical: text doc count reader: {err}")))?;
    reader.reload().map_err(|err| {
        CoreError::Storage(format!("lexical: text doc count reader reload: {err}"))
    })?;
    let query = TermQuery::new(
        Term::from_field_text(fields.doc_kind, TEXT_DOC_KIND),
        IndexRecordOption::Basic,
    );
    let count = reader
        .searcher()
        .search(&query, &Count)
        .map_err(|err| CoreError::Storage(format!("lexical: text doc count: {err}")))?;
    crate::channel_payloads::count_from_len(count)
}

/// The seal's coverage proof: the text authority lists exactly as many
/// documents as the index holds live text documents.
///
/// The two are published separately (Tantivy commit, then the shards), so
/// a publish that crashed in between leaves the index ahead of the
/// authority; sealing that would serve keyword hits a regex or phrase can
/// never see. The count is one term query, never a scan.
fn ensure_text_authority_covers_index(
    index: &Index,
    generation_dir: &Path,
    fields: &SchemaFields,
    manifest: Option<&TextAuthorityManifest>,
) -> Result<(), CoreError> {
    let live = live_text_doc_count(index, fields)?;
    let listed = manifest.map_or(0, |manifest| {
        manifest
            .shards
            .iter()
            .fold(0_u64, |total, shard| total.saturating_add(shard.rows))
    });
    if live != listed {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            TEXT_AUTHORITY_DIR_NAME,
            &format!("lists {listed} documents but the index holds {live} live text documents"),
        ));
    }
    Ok(())
}
