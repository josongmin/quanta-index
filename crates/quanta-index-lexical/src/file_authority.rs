//! Generation-bound, immutable source files for file-level code search.
//!
//! Chunk text is not a complete source-file authority: terms can occur in
//! different chunks and a literal can cross a chunk boundary. This sidecar
//! stores the producer's entire source bytes under their verified source
//! digest. Its manifest maps an exact source-repository/path identity to one
//! source revision. Replacing or tombstoning a path updates that map before a
//! generation can seal; a query never consults the live filesystem.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{SearchScopeSurface, SourceFileKey, SourceFileRevision};
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_trigram::{DocId, TrigramIndex, TrigramIndexBuilder, trigrams_of};
use sha2::{Digest as _, Sha256};

use crate::channel_payloads::{decode_replace_scope_payload, decode_tombstone_scope_payload};

pub(crate) const DIR: &str = "file-authority";
pub(crate) const MANIFEST: &str = "manifest.cbor";
// The replace-scope transport is a single bounded frame; leave room for
// coverage, chunks, symbols and encoding overhead in its 16 MiB frame.
pub(crate) const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_TOTAL_SOURCE_BYTES: usize = 128 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
// A cold open can index an admitted 8 MiB source. Check the request between
// bounded slices instead of waiting for an entire file's trigram build.
const TRIGRAM_BUILD_SLICE_BYTES: usize = 64 * 1024;
// Only folded content/path postings are retained. Sensitive matches are
// verified against the original NFC surfaces, so this candidate superset
// shares one index per surface. Forward-only scratch omits reverse postings.
// Bound both total memberships and each dictionary's estimated scratch heap;
// a high-entropy source can have many singleton dictionary entries.
const MAX_FILE_INDEX_POSTING_MEMBERSHIPS: usize = 4_000_000;
const MAX_FILE_INDEX_BUILD_HEAP_BYTES: usize = 128 * 1024 * 1024;
const TRIGRAM_BITMAP_BYTES: usize = 2 * 1024 * 1024;

// This sealed artifact evolves the existing manifest. Counts are per source
// file, not a second index: deltas can sum unchanged rows without rescanning
// their source bytes at every seal. Cold open independently recomputes each
// count from committed bytes before the source can serve a query.
pub(crate) type FileManifestRow = (SourceFileRevision, u32);

#[derive(Clone, Debug)]
pub(crate) struct SourceFile {
    pub(crate) source: SourceFileRevision,
    pub(crate) bytes: Vec<u8>,
    pub(crate) text_admitted: bool,
    pub(crate) language: LanguageCode,
    pub(crate) indexed_text: Option<String>,
    pub(crate) folded_text: Option<String>,
    pub(crate) indexed_path: String,
    pub(crate) folded_path: String,
    pub(crate) expected_postings: u32,
}

impl SourceFile {
    pub(crate) fn admitted_text(&self) -> Result<Option<&str>, CoreError> {
        if self.text_admitted {
            std::str::from_utf8(&self.bytes)
                .map(Some)
                .map_err(|error| invalid(&format!("text-admitted source is not UTF-8: {error}")))
        } else {
            Ok(None)
        }
    }
}

pub(crate) struct FileAuthority {
    pub(crate) files: BTreeMap<SourceFileKey, SourceFile>,
    pub(crate) ordered_keys: Vec<SourceFileKey>,
    pub(crate) content_folded: TrigramIndex,
    pub(crate) path_folded: TrigramIndex,
}

impl FileAuthority {
    pub(crate) fn heap_bytes_estimate(&self) -> u64 {
        let bytes = self.files.iter().fold(0_u64, |total, (key, file)| {
            total
                .saturating_add(128)
                .saturating_add(saturating_usize_to_u64(key.source_repo_id.as_str().len()))
                .saturating_add(saturating_usize_to_u64(
                    key.repo_relative_path.as_str().len(),
                ))
                .saturating_add(saturating_usize_to_u64(file.bytes.len()))
                .saturating_add(
                    file.indexed_text
                        .as_ref()
                        .map_or(0, |text| saturating_usize_to_u64(text.len())),
                )
                .saturating_add(
                    file.folded_text
                        .as_ref()
                        .map_or(0, |text| saturating_usize_to_u64(text.len())),
                )
                .saturating_add(saturating_usize_to_u64(
                    file.indexed_path
                        .len()
                        .saturating_add(file.folded_path.len()),
                ))
        });
        [&self.content_folded, &self.path_folded]
            .iter()
            .fold(bytes, |total, index| {
                index.iter().fold(total, |total, (_tri, postings)| {
                    total
                        .saturating_add(64)
                        .saturating_add(saturating_usize_to_u64(postings.len()).saturating_mul(8))
                })
            })
    }
}

// The resident estimate is intentionally conservative on a target whose usize
// can exceed u64; every contribution and the total saturate at u64::MAX.
fn saturating_usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).map_or(u64::MAX, std::convert::identity)
}

pub(crate) fn manifest_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(DIR).join(MANIFEST)
}

fn file_name(digest: &[u8; 32]) -> String {
    let mut name = String::with_capacity(68);
    for &byte in digest {
        name.push(hex_digit(byte >> 4));
        name.push(hex_digit(byte & 0x0f));
    }
    name.push_str(".bin");
    name
}

fn hex_digit(nibble: u8) -> char {
    char::from(if nibble < 10 {
        b'0'.saturating_add(nibble)
    } else {
        b'a'.saturating_add(nibble.saturating_sub(10))
    })
}

pub(crate) fn artifact_name(source: &SourceFileRevision) -> String {
    format!("{DIR}/{}", file_name(&source.source_sha256))
}

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidContract(format!("lexical file authority: {reason}"))
}

fn corrupt(generation_dir: &Path, name: &str, reason: &str) -> CoreError {
    crate::index_store::sidecar_corrupt(generation_dir, name, reason)
}

pub(crate) fn decode_verified_manifest(
    bytes: &[u8],
    generation_dir: &Path,
) -> Result<Vec<FileManifestRow>, CoreError> {
    let entries: Vec<FileManifestRow> = crate::channel_payloads::decode_cbor_exact(bytes)
        .map_err(|error| corrupt(generation_dir, MANIFEST, &format!("decode: {error}")))?;
    let mut previous: Option<&SourceFileKey> = None;
    let mut postings = 0_usize;
    for (source, count) in &entries {
        source
            .validate()
            .map_err(|error| corrupt(generation_dir, MANIFEST, error))?;
        if previous.is_some_and(|key| key >= &source.file) {
            return Err(corrupt(
                generation_dir,
                MANIFEST,
                "source file keys are not strictly ascending",
            ));
        }
        previous = Some(&source.file);
        postings = postings.saturating_add(usize::try_from(*count).map_err(|error| {
            corrupt(generation_dir, MANIFEST, &format!("posting count: {error}"))
        })?);
        if postings > MAX_FILE_INDEX_POSTING_MEMBERSHIPS {
            return Err(corrupt(
                generation_dir,
                MANIFEST,
                "file trigram posting membership admission exceeded",
            ));
        }
    }
    Ok(entries)
}

pub(crate) fn read_manifest(
    generation_dir: &Path,
) -> Result<Option<Vec<FileManifestRow>>, CoreError> {
    let path = manifest_path(generation_dir);
    let mut file = match crate::sealed_generation::open_regular_nofollow(
        generation_dir,
        Path::new(&format!("{DIR}/{MANIFEST}")),
    ) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "lexical: open file authority {}: {error}",
                path.display()
            )));
        }
    };
    let bytes = crate::sealed_generation::read_opened_bounded(&mut file, MAX_MANIFEST_BYTES)
        .map_err(|error| corrupt(generation_dir, MANIFEST, &format!("read: {error}")))?;
    decode_verified_manifest(&bytes, generation_dir).map(Some)
}

pub(crate) fn ensure_empty_manifest(generation_dir: &Path) -> Result<(), CoreError> {
    if read_manifest(generation_dir)?.is_some() {
        return Ok(());
    }
    let dir = generation_dir.join(DIR);
    std::fs::create_dir_all(&dir).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: create file authority {}: {error}",
            dir.display()
        ))
    })?;
    let empty: Vec<FileManifestRow> = Vec::new();
    let encoded = crate::channel_payloads::encode_cbor(&empty, "empty file authority manifest")?;
    crate::index_store::write_atomic_durable(
        &manifest_path(generation_dir),
        &encoded,
        "file authority manifest",
    )
}

#[derive(Debug)]
pub(crate) struct FileAuthorityDelta {
    sources: Vec<FileManifestRow>,
    writes: BTreeMap<[u8; 32], Vec<u8>>,
    encoded: Vec<u8>,
}

/// Validate the net file authority before the index writer is changed.
/// The returned delta is the only input to persistence after the commit.
pub(crate) fn plan_ops(
    generation_dir: &Path,
    ops: &[LexicalChannelOp],
) -> Result<FileAuthorityDelta, CoreError> {
    let mut files: BTreeMap<SourceFileKey, FileManifestRow> = read_manifest(generation_dir)?
        .unwrap_or_default()
        .into_iter()
        .map(|row| (row.0.file.clone(), row))
        .collect();
    let mut bitmap = None;
    let mut writes: BTreeMap<[u8; 32], Vec<u8>> = BTreeMap::new();
    for op in ops {
        match op {
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                let (_, _, scope) = decode_replace_scope_payload(&payload.payload)?;
                let source = scope.coverage.source;
                let text_admitted = scope.coverage.text_admitted;
                let bytes = scope.source_bytes;
                if bytes.len() > MAX_FILE_BYTES {
                    return Err(invalid(
                        "source file exceeds the 8 MiB file-search admission limit",
                    ));
                }
                let observed: [u8; 32] = Sha256::digest(&bytes).into();
                if observed != source.source_sha256 {
                    return Err(invalid("source bytes digest differs from source revision"));
                }
                let bitmap = bitmap.get_or_insert_with(|| vec![0_u8; TRIGRAM_BITMAP_BYTES]);
                let count = source_posting_memberships(&source, &bytes, text_admitted, bitmap)?;
                let _prior = writes.insert(observed, bytes);
                let _prior = files.insert(source.file.clone(), (source, count));
            }
            LexicalChannelOp::TombstoneLexicalScope(payload) => {
                let (_, _, scope) = decode_tombstone_scope_payload(&payload.payload)?;
                let _prior = files.remove(&scope.file);
            }
            LexicalChannelOp::ClearLexicalSurface(payload)
                if payload.surface == SearchScopeSurface::Chunk =>
            {
                files.clear();
            }
            LexicalChannelOp::ClearLexicalSurface(_)
            | LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::UpsertChunk(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::ReplaceStructuralScope(_) => {}
        }
    }
    let sources: Vec<FileManifestRow> = files.into_values().collect();
    let total_postings = sources.iter().try_fold(0_usize, |total, row| {
        total
            .checked_add(
                usize::try_from(row.1)
                    .map_err(|error| invalid(&format!("posting count conversion: {error}")))?,
            )
            .ok_or_else(|| invalid("file trigram posting count overflow"))
    })?;
    if total_postings > MAX_FILE_INDEX_POSTING_MEMBERSHIPS {
        return Err(invalid(
            "file trigram posting membership admission exceeded",
        ));
    }
    let encoded = crate::channel_payloads::encode_cbor(&sources, "file authority manifest")?;
    if encoded.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("file authority manifest exceeds 16 MiB"));
    }
    let mut total = 0_usize;
    for (source, _) in &sources {
        let bytes = if let Some(bytes) = writes.get(&source.source_sha256) {
            bytes.len()
        } else {
            let name = artifact_name(source);
            let file =
                crate::sealed_generation::open_regular_nofollow(generation_dir, Path::new(&name))
                    .map_err(|error| {
                    CoreError::Storage(format!("lexical: open file authority {name}: {error}"))
                })?;
            usize::try_from(
                file.metadata()
                    .map_err(|error| {
                        CoreError::Storage(format!("lexical: stat file authority {name}: {error}"))
                    })?
                    .len(),
            )
            .map_err(|error| {
                CoreError::Storage(format!("lexical: file authority size {name}: {error}"))
            })?
        };
        total = total
            .checked_add(bytes)
            .ok_or_else(|| invalid("source byte sum overflows"))?;
        if total > MAX_TOTAL_SOURCE_BYTES {
            return Err(invalid(
                "file authority exceeds 128 MiB source byte admission",
            ));
        }
    }
    Ok(FileAuthorityDelta {
        sources,
        writes,
        encoded,
    })
}

/// Persist the prevalidated delta after the Tantivy commit. The writer lock
/// spans both writes; a crash between them leaves an unsealed generation.
pub(crate) fn apply_plan(generation_dir: &Path, plan: FileAuthorityDelta) -> Result<(), CoreError> {
    let FileAuthorityDelta {
        sources,
        writes,
        encoded,
    } = plan;
    let dir = generation_dir.join(DIR);
    std::fs::create_dir_all(&dir).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: create file authority {}: {error}",
            dir.display()
        ))
    })?;
    for (digest, bytes) in &writes {
        let path = dir.join(file_name(digest));
        if !path.is_file() {
            crate::index_store::write_atomic_durable(&path, bytes, "file authority source")?;
        }
    }
    crate::index_store::write_atomic_durable(
        &manifest_path(generation_dir),
        &encoded,
        "file authority manifest",
    )?;
    let keep: BTreeSet<String> = sources
        .iter()
        .map(|(source, _)| file_name(&source.source_sha256))
        .collect();
    for entry in std::fs::read_dir(&dir)
        .map_err(|error| CoreError::Storage(format!("lexical: list file authority: {error}")))?
    {
        let entry = entry.map_err(|error| {
            CoreError::Storage(format!("lexical: file authority entry: {error}"))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != MANIFEST
            && !keep.contains(&name)
            && !crate::index_store::is_durable_write_temporary(&name)
        {
            std::fs::remove_file(entry.path()).map_err(|error| {
                CoreError::Storage(format!("lexical: retire file authority {name}: {error}"))
            })?;
        }
    }
    Ok(())
}

pub(crate) fn expected_names(sources: &[SourceFileRevision]) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = sources.iter().map(artifact_name).collect();
    let _inserted = names.insert(format!("{DIR}/{MANIFEST}"));
    names
}

pub(crate) fn max_manifest_bytes() -> usize {
    MAX_MANIFEST_BYTES
}

fn doc_id_for_index(index: usize) -> Result<DocId, CoreError> {
    let ordinal = index
        .checked_add(1)
        .ok_or_else(|| CoreError::Storage("lexical: file id overflow".to_owned()))?;
    u64::try_from(ordinal)
        .map(DocId)
        .map_err(|error| CoreError::Storage(format!("lexical: file id overflow: {error}")))
}

// Admission bookkeeping, discarded after proof; the only serving index is
// the canonical TrigramIndex. The bitmaps charge dictionary keys once per
// surface and memberships once per file, including repeated source digests.
struct FileIndexAdmission {
    dictionaries: [Vec<u8>; 2],
    local: Vec<u8>,
    dictionary_keys: [usize; 2],
    memberships: [usize; 2],
    total: usize,
}

impl FileIndexAdmission {
    fn new() -> Self {
        Self {
            dictionaries: std::array::from_fn(|_| vec![0; TRIGRAM_BITMAP_BYTES]),
            local: vec![0; TRIGRAM_BITMAP_BYTES],
            dictionary_keys: [0; 2],
            memberships: [0; 2],
            total: 0,
        }
    }

    fn add(
        &mut self,
        path: &str,
        content: Option<&str>,
        expected: u32,
        max_heap_bytes: usize,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        let prior = self.total;
        for (surface, bytes) in [Some(path.as_bytes()), content.map(str::as_bytes)]
            .into_iter()
            .enumerate()
        {
            let Some(bytes) = bytes else {
                continue;
            };
            self.local.fill(0);
            for (offset, [first, second, third]) in trigrams_of(bytes).enumerate() {
                if offset % TRIGRAM_BUILD_SLICE_BYTES == 0 {
                    checkpoint(budget)?;
                }
                let key =
                    (usize::from(first) << 16) | (usize::from(second) << 8) | usize::from(third);
                let mask = 1_u8 << (key & 7);
                let local = self
                    .local
                    .get_mut(key >> 3)
                    .ok_or_else(|| invalid("trigram bitmap index overflow"))?;
                if *local & mask != 0 {
                    continue;
                }
                *local |= mask;
                self.total = self.total.saturating_add(1);
                let memberships = self
                    .memberships
                    .get_mut(surface)
                    .ok_or_else(|| invalid("trigram surface outside admission"))?;
                *memberships = memberships.saturating_add(1);
                let dictionary = self
                    .dictionaries
                    .get_mut(surface)
                    .and_then(|bitmap| bitmap.get_mut(key >> 3))
                    .ok_or_else(|| invalid("dictionary bitmap index overflow"))?;
                let keys = self
                    .dictionary_keys
                    .get_mut(surface)
                    .ok_or_else(|| invalid("dictionary surface outside admission"))?;
                if *dictionary & mask == 0 {
                    *dictionary |= mask;
                    *keys = keys.saturating_add(1);
                }
                if self.total > MAX_FILE_INDEX_POSTING_MEMBERSHIPS {
                    return Err(invalid(
                        "file trigram posting membership admission exceeded",
                    ));
                }
                if TrigramIndexBuilder::forward_heap_bytes_for(*memberships, *keys) > max_heap_bytes
                {
                    return Err(invalid("file trigram scratch heap admission exceeded"));
                }
            }
        }
        if self.total.saturating_sub(prior)
            != usize::try_from(expected)
                .map_err(|error| invalid(&format!("manifest posting count: {error}")))?
        {
            return Err(invalid("file posting count differs from sealed manifest"));
        }
        checkpoint(budget)
    }
}

/// Prove the dictionary/membership bound before writing the sealed manifest.
///
/// Read each complete source once, using explicit staged
/// coverage for text admission; no query index or live worktree is consulted.
/// Return actual source-path reads and bytes, separately from commitment hashing.
pub(crate) fn validate_index_build_budget(
    generation_dir: &Path,
    identity: &quanta_index_contract::GenerationSnapshot,
    rows: &[FileManifestRow],
) -> Result<(u64, u64), CoreError> {
    if rows.is_empty() {
        return Ok((0, 0));
    }
    let coverage =
        crate::sealed_generation::coverage::read_staged_coverage(generation_dir, identity)?
            .ok_or_else(|| invalid("source coverage missing at seal"))?;
    if coverage.coverage.len() != rows.len() {
        return Err(invalid("source coverage universe differs at seal"));
    }
    let mut admission = FileIndexAdmission::new();
    let mut source_bytes = 0_usize;
    for (source, expected) in rows {
        let coverage_row = coverage
            .coverage
            .get(&source.file)
            .ok_or_else(|| invalid("source missing from coverage at seal"))?;
        if coverage_row.source != *source {
            return Err(invalid("source identity differs from coverage at seal"));
        }
        let name = artifact_name(source);
        let mut file =
            crate::sealed_generation::open_regular_nofollow(generation_dir, Path::new(&name))
                .map_err(|error| {
                    CoreError::Storage(format!("lexical: open source at seal {name}: {error}"))
                })?;
        let bytes = crate::sealed_generation::read_opened_bounded(&mut file, MAX_FILE_BYTES)
            .map_err(|error| corrupt(generation_dir, &name, &format!("source read: {error}")))?;
        source_bytes = source_bytes.saturating_add(bytes.len());
        if source_bytes > MAX_TOTAL_SOURCE_BYTES {
            return Err(invalid(
                "file authority exceeds 128 MiB source byte admission",
            ));
        }
        let observed: [u8; 32] = Sha256::digest(&bytes).into();
        if observed != source.source_sha256 {
            return Err(invalid(
                "source bytes digest differs from source revision at seal",
            ));
        }
        let raw =
            if coverage_row.text_admitted {
                Some(std::str::from_utf8(&bytes).map_err(|error| {
                    invalid(&format!("text-admitted source is not UTF-8: {error}"))
                })?)
            } else {
                None
            };
        let (_, folded_path, _, folded_content) =
            normalized_surfaces(source.file.repo_relative_path.as_str(), raw);
        admission.add(
            &folded_path,
            folded_content.as_deref(),
            *expected,
            MAX_FILE_INDEX_BUILD_HEAP_BYTES,
            None,
        )?;
    }
    Ok((
        saturating_usize_to_u64(rows.len()),
        saturating_usize_to_u64(source_bytes),
    ))
}

pub(crate) fn from_verified_files(
    files: Vec<SourceFile>,
    budget: Option<&RequestBudgetV1>,
) -> Result<FileAuthority, CoreError> {
    let mut by_key = BTreeMap::new();
    for mut file in files {
        checkpoint(budget)?;
        normalize_file(&mut file)?;
        if by_key.insert(file.source.file.clone(), file).is_some() {
            return Err(invalid("duplicate verified source file"));
        }
        checkpoint(budget)?;
    }
    let mut content_folded = TrigramIndexBuilder::new_forward_only(1).map_err(|error| {
        CoreError::Storage(format!("lexical: folded file content index: {error}"))
    })?;
    let mut path_folded = TrigramIndexBuilder::new_forward_only(1)
        .map_err(|error| CoreError::Storage(format!("lexical: folded file path index: {error}")))?;
    let mut ordered_keys = Vec::with_capacity(by_key.len());
    let mut posting_memberships = 0_usize;
    for (index, (key, file)) in by_key.iter().enumerate() {
        checkpoint(budget)?;
        let id = doc_id_for_index(index)?;
        let prior_memberships = posting_memberships;
        add_doc_with_checkpoints(
            &mut path_folded,
            id,
            file.folded_path.as_bytes(),
            &mut posting_memberships,
            MAX_FILE_INDEX_POSTING_MEMBERSHIPS,
            || checkpoint(budget),
        )?;
        if let Some(folded) = &file.folded_text {
            add_doc_with_checkpoints(
                &mut content_folded,
                id,
                folded.as_bytes(),
                &mut posting_memberships,
                MAX_FILE_INDEX_POSTING_MEMBERSHIPS,
                || checkpoint(budget),
            )?;
        }
        let actual_postings = posting_memberships.saturating_sub(prior_memberships);
        if actual_postings
            != usize::try_from(file.expected_postings)
                .map_err(|error| invalid(&format!("manifest posting count: {error}")))?
        {
            return Err(invalid("file posting count differs from sealed manifest"));
        }
        ordered_keys.push(key.clone());
    }
    checkpoint(budget)?;
    let content_folded = content_folded.finish();
    checkpoint(budget)?;
    let path_folded = path_folded.finish();
    checkpoint(budget)?;
    Ok(FileAuthority {
        files: by_key,
        ordered_keys,
        content_folded,
        path_folded,
    })
}

fn normalize_file(file: &mut SourceFile) -> Result<(), CoreError> {
    let raw = file.admitted_text()?;
    let (path, folded_path, content, folded_content) =
        normalized_surfaces(file.source.file.repo_relative_path.as_str(), raw);
    file.indexed_path = path;
    file.folded_path = folded_path;
    file.indexed_text = content;
    file.folded_text = folded_content;
    Ok(())
}

fn normalized_surfaces(
    path: &str,
    raw: Option<&str>,
) -> (String, String, Option<String>, Option<String>) {
    let path = crate::normalize::nfc(path).into_owned();
    let folded_path = crate::normalize::fold(&path);
    let content = raw.map(|raw| crate::normalize::nfc(raw).into_owned());
    let folded_content = content.as_deref().map(crate::normalize::fold);
    (path, folded_path, content, folded_content)
}

fn source_posting_memberships(
    source: &SourceFileRevision,
    bytes: &[u8],
    text_admitted: bool,
    bitmap: &mut [u8],
) -> Result<u32, CoreError> {
    let raw = if text_admitted {
        Some(
            std::str::from_utf8(bytes)
                .map_err(|error| invalid(&format!("text-admitted source is not UTF-8: {error}")))?,
        )
    } else {
        None
    };
    let (_, folded_path, _, folded_content) =
        normalized_surfaces(source.file.repo_relative_path.as_str(), raw);
    let mut total = 0_usize;
    for surface in [
        Some(folded_path.as_bytes()),
        folded_content.as_deref().map(str::as_bytes),
    ]
    .into_iter()
    .flatten()
    {
        count_posting_memberships(
            surface,
            bitmap,
            &mut total,
            MAX_FILE_INDEX_POSTING_MEMBERSHIPS,
        )?;
    }
    u32::try_from(total).map_err(|error| invalid(&format!("posting count: {error}")))
}

fn count_posting_memberships(
    source: &[u8],
    bitmap: &mut [u8],
    total: &mut usize,
    max_memberships: usize,
) -> Result<(), CoreError> {
    bitmap.fill(0);
    for [first, second, third] in trigrams_of(source) {
        let key = (usize::from(first) << 16) | (usize::from(second) << 8) | usize::from(third);
        let entry = bitmap
            .get_mut(key >> 3)
            .ok_or_else(|| CoreError::Storage("lexical: trigram bitmap index overflow".into()))?;
        let mask = 1_u8 << (key & 7);
        if *entry & mask == 0 {
            *entry |= mask;
            *total = total.saturating_add(1);
            if *total > max_memberships {
                return Err(invalid(
                    "file trigram posting membership admission exceeded",
                ));
            }
        }
    }
    Ok(())
}

fn add_doc_with_checkpoints<F>(
    builder: &mut TrigramIndexBuilder,
    id: DocId,
    bytes: &[u8],
    total_memberships: &mut usize,
    max_memberships: usize,
    mut checkpoint: F,
) -> Result<(), CoreError>
where
    F: FnMut() -> Result<(), CoreError>,
{
    let prior = builder.posting_memberships();
    if bytes.len() < 3 {
        checkpoint()?;
        return Ok(());
    }
    let mut start: usize = 0;
    loop {
        checkpoint()?;
        let end = start
            .saturating_add(TRIGRAM_BUILD_SLICE_BYTES)
            .min(bytes.len());
        let slice = bytes.get(start..end).ok_or_else(|| {
            CoreError::Storage("lexical: trigram build slice outside source".into())
        })?;
        builder.add_doc(id, slice);
        ensure_trigram_build_heap(builder, MAX_FILE_INDEX_BUILD_HEAP_BYTES)?;
        let current = total_memberships
            .saturating_sub(prior)
            .saturating_add(builder.posting_memberships());
        if current > max_memberships {
            return Err(invalid(
                "file trigram posting membership admission exceeded",
            ));
        }
        if end == bytes.len() {
            break;
        }
        // Preserve the two windows that straddle the slice boundary.
        start = end.saturating_sub(2);
    }
    *total_memberships = total_memberships
        .saturating_sub(prior)
        .saturating_add(builder.posting_memberships());
    checkpoint()
}

fn ensure_trigram_build_heap(
    builder: &TrigramIndexBuilder,
    max_heap_bytes: usize,
) -> Result<(), CoreError> {
    if builder.forward_heap_bytes_estimate() > max_heap_bytes {
        Err(invalid("file trigram scratch heap admission exceeded"))
    } else {
        Ok(())
    }
}

fn checkpoint(budget: Option<&RequestBudgetV1>) -> Result<(), CoreError> {
    budget.map_or(Ok(()), |budget| {
        budget.checkpoint("lexical:cold-open:file-index")
    })
}

#[cfg(test)]
mod tests {
    use super::{
        DIR, MANIFEST, MAX_FILE_BYTES, SourceFile, TRIGRAM_BUILD_SLICE_BYTES,
        add_doc_with_checkpoints, count_posting_memberships, decode_verified_manifest,
        doc_id_for_index, file_name, from_verified_files, plan_ops, saturating_usize_to_u64,
    };
    use quanta_index_contract::channel::{LexicalChannelOp, TombstoneLexicalScope};
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        BatchIngestMode, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
        SearchCorpusTombstoneScope, SourceFileKey, SourceFileRevision,
    };
    use quanta_index_lq_trigram::{DocId, TrigramIndexBuilder};
    use sha2::Digest as _;

    #[test]
    fn shared_source_admission_refuses_high_entropy_dictionary_before_build() {
        let mut diverse = super::FileIndexAdmission::new();
        let error = diverse
            .add("x", Some("abcdefghijk"), 9, 1_000, None)
            .expect_err("nine distinct keys exceed dictionary byte admission");
        assert!(
            matches!(error, quanta_index_core::CoreError::InvalidContract(message)
            if message.contains("scratch heap"))
        );
        let repeated_source = "abc".repeat(1_000);
        let mut repeated = super::FileIndexAdmission::new();
        repeated
            .add("x", Some(&repeated_source), 3, 1_000, None)
            .expect("three distinct trigrams fit regardless of source repetitions");
        repeated
            .add("x", Some(&repeated_source), 3, 1_000, None)
            .expect("dictionary is shared, memberships remain per file");
        assert_eq!(repeated.total, 6);
        assert_eq!(repeated.dictionary_keys, [0, 3]);
    }

    #[test]
    fn shared_source_admission_binds_counts_even_without_constructing_postings() {
        let mut admission = super::FileIndexAdmission::new();
        let error = admission
            .add("abc", Some("def"), 1, usize::MAX, None)
            .expect_err("two surface memberships cannot claim one");
        assert!(
            matches!(error, quanta_index_core::CoreError::InvalidContract(message)
            if message.contains("posting count differs"))
        );
    }

    #[test]
    fn scratch_heap_admission_counts_distinct_postings_not_source_repetitions() {
        let mut dense = TrigramIndexBuilder::new_forward_only(1).expect("builder");
        for id in 1..=1_000 {
            dense.add_doc(DocId(id), b"abc");
        }
        assert!(super::ensure_trigram_build_heap(&dense, 16_384).is_err());
        let mut repeated = TrigramIndexBuilder::new_forward_only(1).expect("builder");
        for _ in 0..1_000 {
            repeated.add_doc(DocId(1), b"abc");
        }
        assert!(super::ensure_trigram_build_heap(&repeated, 16_384).is_ok());
    }

    #[test]
    fn folded_manifest_admits_more_than_old_duplicate_membership_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("repo").expect("repo"),
                repo_relative_path: RepoRelativePath::new("src/a.rs"),
            },
            revision_id: RevisionId::new("revision").expect("revision"),
            source_sha256: [7; 32],
        };
        let mut admitted = Vec::new();
        ciborium::into_writer(&vec![(source, 3_379_823_u32)], &mut admitted).expect("encode");
        assert_eq!(
            decode_verified_manifest(&admitted, dir.path())
                .expect("admitted")
                .len(),
            1
        );
    }

    #[test]
    fn sliced_trigram_build_preserves_boundary_windows() {
        let mut bytes = vec![b'x'; TRIGRAM_BUILD_SLICE_BYTES + 7];
        bytes
            .get_mut(
                TRIGRAM_BUILD_SLICE_BYTES.saturating_sub(2)
                    ..TRIGRAM_BUILD_SLICE_BYTES.saturating_add(2),
            )
            .expect("boundary is inside source")
            .copy_from_slice(b"abcd");
        let mut whole = TrigramIndexBuilder::new(1).expect("whole");
        whole.add_doc(DocId(1), &bytes);
        let mut sliced = TrigramIndexBuilder::new(1).expect("sliced");
        let mut checks = 0;
        let mut memberships = 0;
        add_doc_with_checkpoints(
            &mut sliced,
            DocId(1),
            &bytes,
            &mut memberships,
            usize::MAX,
            || {
                checks += 1;
                Ok(())
            },
        )
        .expect("sliced build");
        assert!(checks >= 3);
        assert_eq!(sliced.finish(), whole.finish());
        assert!(memberships >= 4);
    }

    #[test]
    fn sliced_trigram_build_stops_at_checkpoint() {
        let bytes = vec![b'x'; TRIGRAM_BUILD_SLICE_BYTES * 3];
        let mut builder = TrigramIndexBuilder::new(1).expect("builder");
        let mut checks = 0;
        let mut memberships = 0;
        let result = add_doc_with_checkpoints(
            &mut builder,
            DocId(1),
            &bytes,
            &mut memberships,
            usize::MAX,
            || {
                checks += 1;
                if checks == 2 {
                    Err(quanta_index_core::CoreError::InvalidContract(
                        "cancelled".into(),
                    ))
                } else {
                    Ok(())
                }
            },
        );
        assert!(result.is_err());
        assert_eq!(checks, 2);
    }

    #[test]
    fn file_trigram_membership_limit_refuses_before_unbounded_growth() {
        let mut builder = TrigramIndexBuilder::new(1).expect("builder");
        let mut memberships = 0;
        let result = add_doc_with_checkpoints(
            &mut builder,
            DocId(1),
            b"abcdef",
            &mut memberships,
            3,
            || Ok(()),
        );
        assert!(
            matches!(result, Err(quanta_index_core::CoreError::InvalidContract(ref message)) if message.contains("posting membership"))
        );
        assert_eq!(builder.posting_memberships(), 4);
    }

    #[test]
    fn file_trigram_membership_limit_accumulates_across_files() {
        let mut builder = TrigramIndexBuilder::new(1).expect("builder");
        let mut memberships = 0;
        add_doc_with_checkpoints(&mut builder, DocId(1), b"abcd", &mut memberships, 3, || {
            Ok(())
        })
        .expect("first file fits");
        assert_eq!(memberships, 2);
        let result =
            add_doc_with_checkpoints(&mut builder, DocId(2), b"wxyz", &mut memberships, 3, || {
                Ok(())
            });
        assert!(result.is_err());
    }

    #[test]
    fn file_trigram_membership_limit_accumulates_across_surfaces() {
        let mut content = TrigramIndexBuilder::new(1).expect("content");
        let mut path = TrigramIndexBuilder::new(1).expect("path");
        let mut memberships = 0;
        add_doc_with_checkpoints(&mut content, DocId(1), b"abcd", &mut memberships, 3, || {
            Ok(())
        })
        .expect("content fits");
        let result =
            add_doc_with_checkpoints(&mut path, DocId(1), b"file", &mut memberships, 3, || Ok(()));
        assert!(result.is_err());
    }

    #[test]
    fn seal_count_matches_builder_memberships_and_refuses_over_limit() {
        let mut bitmap = vec![0_u8; super::TRIGRAM_BITMAP_BYTES];
        let mut total = 0;
        let sources: [&[u8]; 2] = [b"ababa", "ÄÄÄ".as_bytes()];
        let mut builder = TrigramIndexBuilder::new(1).expect("builder");
        for (index, source) in sources.into_iter().enumerate() {
            count_posting_memberships(source, &mut bitmap, &mut total, usize::MAX).expect("count");
            builder.add_doc(
                DocId(u64::try_from(index.saturating_add(1)).expect("id")),
                source,
            );
        }
        assert_eq!(total, builder.posting_memberships());
        let mut refused = 0;
        let error = count_posting_memberships(b"abcdef", &mut bitmap, &mut refused, 3)
            .expect_err("four unique trigrams exceed three memberships");
        assert!(
            matches!(error, quanta_index_core::CoreError::InvalidContract(message) if message.contains("posting membership"))
        );
    }

    #[test]
    fn file_manifest_requires_posting_count_and_refuses_excess() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("repo").expect("repo"),
                repo_relative_path: RepoRelativePath::new("src/a.rs"),
            },
            revision_id: RevisionId::new("revision").expect("revision"),
            source_sha256: [7; 32],
        };
        let mut old = Vec::new();
        ciborium::into_writer(&vec![source.clone()], &mut old).expect("old encode");
        assert!(decode_verified_manifest(&old, dir.path()).is_err());
        let mut over = Vec::new();
        ciborium::into_writer(&vec![(source, 4_000_001_u32)], &mut over).expect("over encode");
        assert!(decode_verified_manifest(&over, dir.path()).is_err());
    }

    #[test]
    fn cold_open_recounts_committed_source_postings() {
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("repo").expect("repo"),
                repo_relative_path: RepoRelativePath::new("a.rs"),
            },
            revision_id: RevisionId::new("revision").expect("revision"),
            source_sha256: sha2::Sha256::digest(b"abc").into(),
        };
        let file = SourceFile {
            source,
            bytes: b"abc".to_vec(),
            text_admitted: true,
            language: LanguageCode::new("rust").expect("language"),
            indexed_text: None,
            folded_text: None,
            indexed_path: String::new(),
            folded_path: String::new(),
            // "a.rs" has two distinct trigrams and "abc" has one; each
            // appears once in the shared folded index.
            expected_postings: 3,
        };
        assert!(from_verified_files(vec![file.clone()], None).is_ok());
        let mut forged = file;
        forged.expected_postings = 2;
        assert!(matches!(
            from_verified_files(vec![forged], None),
            Err(quanta_index_core::CoreError::InvalidContract(message))
                if message.contains("posting count differs")
        ));
    }

    #[test]
    fn file_name_encodes_both_nibbles_as_lowercase_hex() {
        assert_eq!(file_name(&[0xab; 32]), format!("{}.bin", "ab".repeat(32)));
        assert_eq!(file_name(&[0x05; 32]), format!("{}.bin", "05".repeat(32)));
    }

    #[test]
    fn planned_net_source_bytes_enforce_the_generation_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let generation_dir = dir.path().join("generation");
        let authority_dir = generation_dir.join(DIR);
        std::fs::create_dir_all(&authority_dir).expect("authority dir");
        let mut sources = Vec::new();
        for index in 1_u8..=17 {
            let digest = [index; 32];
            let source = SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("cap-repo").expect("repo"),
                    repo_relative_path: RepoRelativePath::new(format!("{index:02}.rs")),
                },
                revision_id: RevisionId::new("cap-revision").expect("revision"),
                source_sha256: digest,
            };
            let file = std::fs::File::create(authority_dir.join(file_name(&digest)))
                .expect("source artifact");
            file.set_len(if index == 17 {
                1
            } else {
                u64::try_from(MAX_FILE_BYTES).expect("file cap fits u64")
            })
            .expect("sparse source length");
            sources.push(source);
        }
        let write_manifest = |sources: &[SourceFileRevision]| {
            let mut encoded = Vec::new();
            let rows: Vec<_> = sources
                .iter()
                .cloned()
                .map(|source| (source, 0_u32))
                .collect();
            ciborium::into_writer(&rows, &mut encoded).expect("encode manifest");
            std::fs::write(authority_dir.join(MANIFEST), encoded).expect("write manifest");
        };
        write_manifest(sources.get(..16).expect("first sixteen sources"));
        let at_cap = plan_ops(&generation_dir, &[]);
        assert!(at_cap.is_ok(), "128 MiB is admitted: {at_cap:?}");
        write_manifest(&sources);
        let result = plan_ops(&generation_dir, &[]);
        assert!(
            matches!(result, Err(quanta_index_core::CoreError::InvalidContract(ref message)) if message.contains("128 MiB")),
            "128 MiB + 1 byte must be refused"
        );
        let mut payload = Vec::new();
        ciborium::into_writer(
            &(
                BatchIngestMode::ReplaceGeneration,
                None::<ManifestGeneration>,
                SearchCorpusTombstoneScope {
                    file: sources.first().expect("first source").file.clone(),
                },
            ),
            &mut payload,
        )
        .expect("encode tombstone");
        let tombstone = LexicalChannelOp::TombstoneLexicalScope(TombstoneLexicalScope {
            repo_id: RepoId::new("cap-repo").expect("repo"),
            revision_id: RevisionId::new("cap-revision").expect("revision"),
            generation: ManifestGeneration::new(1),
            payload,
        });
        assert!(
            plan_ops(&generation_dir, &[tombstone]).is_ok(),
            "retiring an 8 MiB file must be judged by the net source set"
        );
    }

    #[test]
    fn file_id_rejects_overflow_instead_of_wrapping() {
        assert_eq!(doc_id_for_index(0).expect("first id"), DocId(1));
        assert!(doc_id_for_index(usize::MAX).is_err());
    }

    #[test]
    fn resident_estimate_conversion_preserves_or_saturates() {
        assert_eq!(saturating_usize_to_u64(123), 123);
        if usize::BITS > u64::BITS {
            assert_eq!(saturating_usize_to_u64(usize::MAX), u64::MAX);
        }
    }
}
