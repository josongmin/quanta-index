//! Generation-bound, immutable source files for file-level code search.
//!
//! Chunk text is not a complete source-file authority: terms can occur in
//! different chunks and a literal can cross a chunk boundary. This sidecar
//! stores the producer's entire source bytes under their verified source
//! digest. Its manifest maps an exact source-repository/path identity to one
//! source revision. Replacing or tombstoning a path updates that map before a
//! generation can seal; a query never consults the live filesystem.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{SearchScopeSurface, SourceFileKey, SourceFileRevision};
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_trigram::trigrams_of;
use sha2::{Digest as _, Sha256};

use crate::channel_payloads::{decode_replace_scope_payload, decode_tombstone_scope_payload};

mod codec;
mod producer;
mod reader;
pub(crate) mod root;
mod verify;

pub(crate) use codec::PostingSurface;
pub(crate) use reader::QueryWork;
pub(crate) struct VerifiedAuthority {
    authority: verify::VerifiedAuthority,
    pinned_objects: BTreeMap<[u8; 32], ObjectIdentity>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObjectIdentity {
    dev: u64,
    ino: u64,
    len: u64,
}

pub(crate) const DIR: &str = root::DIR_NAME;
pub(crate) const MANIFEST: &str = "staging/manifest.cbor";
pub(crate) const ROOT: &str = root::ROOT_FILE_NAME;
pub(crate) const OBJECTS: &str = root::OBJECTS_NAME;
// The replace-scope transport is a single bounded frame; leave room for
// coverage, chunks, symbols and encoding overhead in its 16 MiB frame.
pub(crate) const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_TOTAL_SOURCE_BYTES: u32 = 128 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
// A cold open can index an admitted 8 MiB source. Check the request between
// bounded slices instead of waiting for an entire file's trigram build.
// Only folded content/path postings are retained. Sensitive matches are
// verified against the original NFC surfaces, so this candidate superset
// shares one index per surface. Forward-only scratch omits reverse postings.
// Bound both total memberships and each dictionary's estimated scratch heap;
// a high-entropy source can have many singleton dictionary entries.
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

pub(crate) struct FileAuthority {
    pub(crate) files: BTreeMap<SourceFileKey, SourceFile>,
    pub(crate) ordered_keys: Vec<SourceFileKey>,
    root: root::AuthorityRoot,
    posting_directory: verify::PostingDirectory,
    term_directory_charge: u64,
    object_dir: PathBuf,
    pinned_objects: BTreeMap<[u8; 32], ObjectIdentity>,
    keys_by_id: BTreeMap<u64, SourceFileKey>,
    ids_by_key: BTreeMap<SourceFileKey, u64>,
}

impl FileAuthority {
    pub(crate) fn id_for_key(&self, key: &SourceFileKey) -> Result<u64, CoreError> {
        self.ids_by_key.get(key).copied().ok_or_else(|| {
            CoreError::Storage("lexical: verified file key lacks stable source id".into())
        })
    }

    pub(crate) fn file_for_id(&self, id: u64) -> Result<&SourceFile, CoreError> {
        self.keys_by_id
            .get(&id)
            .and_then(|key| self.files.get(key))
            .ok_or_else(|| {
                CoreError::Storage("lexical: posting id lacks verified source file".into())
            })
    }

    pub(crate) fn posting_lists(
        &self,
        surface: PostingSurface,
        grams: &[[u8; 3]],
        work: &mut QueryWork,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeMap<[u8; 3], Vec<u64>>, CoreError> {
        reader::posting_lists(
            &self.root,
            &self.posting_directory,
            policy(),
            surface,
            grams,
            work,
            budget,
            |digest, total_len, offset, len| {
                let pinned = self.pinned_objects.get(&digest).ok_or_else(|| {
                    CoreError::Storage("lexical: posting object has no pinned identity".into())
                })?;
                read_object_range(
                    &self.object_dir,
                    digest,
                    total_len,
                    offset,
                    len,
                    *pinned,
                    budget,
                )
            },
        )
    }

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
        bytes
            .saturating_add(saturating_usize_to_u64(self.root.sources.len()).saturating_mul(256))
            .saturating_add(self.term_directory_charge)
            .saturating_add(saturating_usize_to_u64(self.keys_by_id.len()).saturating_mul(160))
            .saturating_add(saturating_usize_to_u64(self.ids_by_key.len()).saturating_mul(160))
    }

    fn checked_resident_bytes(&self) -> Result<u64, CoreError> {
        let mut total = self.term_directory_charge;
        for row in &self.root.sources {
            let file = self
                .files
                .get(&row.source.file)
                .ok_or_else(|| invalid("resident source is absent"))?;
            let charge = root::resident_file_charge(
                &file.source,
                &file.language,
                file.bytes.len(),
                file.indexed_path.len(),
                file.folded_path.len(),
                file.indexed_text.as_ref().map_or(0, String::len),
                file.folded_text.as_ref().map_or(0, String::len),
            )
            .map_err(|reason| invalid(&reason))?;
            if charge != row.resident_heap_bytes {
                return Err(invalid("resident source charge differs from root"));
            }
            total = total
                .checked_add(charge)
                .ok_or_else(|| invalid("resident heap charge overflow"))?;
            if total > policy().resident_file_heap_bytes {
                return Err(invalid("resident heap exceeds policy"));
            }
        }
        Ok(total)
    }
}

pub(crate) fn read_object(
    object_dir: &Path,
    digest: [u8; 32],
    expected_len: u64,
) -> Result<Vec<u8>, String> {
    let generation_dir = object_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "object directory has no generation parent".to_owned())?;
    let name = object_name(&digest);
    let mut file =
        crate::sealed_generation::open_regular_nofollow(generation_dir, Path::new(&name))
            .map_err(|error| format!("open {name}: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("stat {name}: {error}"))?;
    if !metadata.is_file() || metadata.len() != expected_len {
        return Err(format!("{name} is not a regular file of committed length"));
    }
    let maximum = usize::try_from(expected_len)
        .map_err(|error| format!("object length exceeds usize: {error}"))?;
    crate::sealed_generation::read_opened_bounded(&mut file, maximum)
        .map_err(|error| format!("read {name}: {error}"))
}

pub(crate) fn read_object_pinned(
    object_dir: &Path,
    digest: [u8; 32],
    expected_len: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<(Vec<u8>, ObjectIdentity), CoreError> {
    let generation_dir = object_dir.parent().and_then(Path::parent).ok_or_else(|| {
        CoreError::Storage("lexical: F15 object directory has no generation parent".into())
    })?;
    let name = object_name(&digest);
    let mut file =
        crate::sealed_generation::open_regular_nofollow(generation_dir, Path::new(&name))
            .map_err(|error| corrupt(generation_dir, &name, &format!("open: {error}")))?;
    let metadata = file
        .metadata()
        .map_err(|error| corrupt(generation_dir, &name, &format!("stat: {error}")))?;
    if !metadata.is_file() || metadata.len() != expected_len {
        return Err(corrupt(
            generation_dir,
            &name,
            "object differs from committed length",
        ));
    }
    let maximum = usize::try_from(expected_len).map_err(|error| {
        CoreError::Storage(format!("lexical: F15 object length exceeds usize: {error}"))
    })?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(maximum).map_err(|error| {
        CoreError::Storage(format!("lexical: F15 object allocation refused: {error}"))
    })?;
    bytes.resize(maximum, 0);
    for chunk in bytes.chunks_mut(64 * 1024) {
        checkpoint(budget)?;
        file.read_exact(chunk)
            .map_err(|error| corrupt(generation_dir, &name, &format!("read: {error}")))?;
    }
    let mut extra = [0_u8; 1];
    if file
        .read(&mut extra)
        .map_err(|error| corrupt(generation_dir, &name, &format!("read tail: {error}")))?
        != 0
    {
        return Err(corrupt(generation_dir, &name, "object grew after stat"));
    }
    checkpoint(budget)?;
    Ok((
        bytes,
        ObjectIdentity {
            dev: metadata.dev(),
            ino: metadata.ino(),
            len: metadata.len(),
        },
    ))
}

fn read_object_range(
    object_dir: &Path,
    digest: [u8; 32],
    total_len: u64,
    offset: u64,
    len: u64,
    pinned: ObjectIdentity,
    budget: &RequestBudgetV1,
) -> Result<Vec<u8>, CoreError> {
    let generation_dir = object_dir.parent().and_then(Path::parent).ok_or_else(|| {
        CoreError::Storage("lexical: F15 object directory has no generation parent".into())
    })?;
    let name = object_name(&digest);
    if offset.checked_add(len).is_none_or(|end| end > total_len)
        || len > policy().query_decoded_bytes
    {
        return Err(corrupt(
            generation_dir,
            &name,
            "posting range exceeds committed object",
        ));
    }
    let mut file =
        crate::sealed_generation::open_regular_nofollow(generation_dir, Path::new(&name)).map_err(
            |error| {
                corrupt(
                    generation_dir,
                    &name,
                    &format!("open posting range: {error}"),
                )
            },
        )?;
    let metadata = file.metadata().map_err(|error| {
        corrupt(
            generation_dir,
            &name,
            &format!("stat posting range: {error}"),
        )
    })?;
    if !metadata.is_file()
        || metadata.len() != total_len
        || pinned.len != total_len
        || metadata.dev() != pinned.dev
        || metadata.ino() != pinned.ino
    {
        return Err(corrupt(
            generation_dir,
            &name,
            "posting object identity differs from cold-open snapshot",
        ));
    }
    let _position = file.seek(SeekFrom::Start(offset)).map_err(|error| {
        CoreError::Storage(format!("lexical: seek posting range {name}: {error}"))
    })?;
    let length = usize::try_from(len).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: posting range length exceeds usize: {error}"
        ))
    })?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: posting range allocation refused: {error}"
        ))
    })?;
    bytes.resize(length, 0);
    for chunk in bytes.chunks_mut(64 * 1024) {
        budget.checkpoint("lexical:query:posting-range-read")?;
        file.read_exact(chunk).map_err(|error| {
            corrupt(
                generation_dir,
                &name,
                &format!("read posting range: {error}"),
            )
        })?;
    }
    budget.checkpoint("lexical:query:posting-range-read")?;
    Ok(bytes)
}

fn policy() -> root::AuthorityPolicy {
    root::AuthorityPolicy {
        root_bytes: 16 * 1024 * 1024,
        source_files: 32_768,
        source_bytes: u64::from(MAX_TOTAL_SOURCE_BYTES),
        pack_bytes: 16 * 1024 * 1024,
        total_pack_bytes: 160 * 1024 * 1024,
        posting_block_bytes: 32 * 1024 * 1024,
        total_posting_bytes: 512 * 1024 * 1024,
        total_memberships: u64::from(crate::FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT),
        partitions: 256,
        source_id: u64::from(u32::MAX),
        bucket_scratch_bytes: 128 * 1024 * 1024,
        term_directory_bytes: 32 * 1024 * 1024,
        resident_file_heap_bytes: 512 * 1024 * 1024,
        query_list_reads: 16_384,
        query_posting_ids: 2_000_000,
        query_decoded_bytes: 512 * 1024 * 1024,
        query_decoded_ids: u64::from(crate::FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT),
    }
}

pub(crate) fn max_root_bytes() -> u64 {
    policy().root_bytes
}

fn codec_limits() -> Result<codec::CodecLimits, String> {
    let policy = policy();
    Ok(codec::CodecLimits {
        source_pack_encoded_bytes: usize::try_from(policy.pack_bytes)
            .map_err(|error| format!("source pack ceiling exceeds usize: {error}"))?,
        posting_block_encoded_bytes: usize::try_from(policy.posting_block_bytes)
            .map_err(|error| format!("posting block ceiling exceeds usize: {error}"))?,
        sources: usize::try_from(policy.source_files)
            .map_err(|error| format!("source count ceiling exceeds usize: {error}"))?,
        terms: 1 << 24,
        memberships: policy.total_memberships,
    })
}

pub(crate) fn object_name(digest: &[u8; 32]) -> String {
    format!("{DIR}/{OBJECTS}/{}", file_name(digest))
}

fn object_inventory(root: &root::AuthorityRoot) -> BTreeMap<[u8; 32], u64> {
    root.packs
        .iter()
        .chain(&root.path_postings)
        .chain(&root.content_postings)
        .map(|row| (row.sha256, row.bytes))
        .collect()
}

pub(crate) fn verify_v15<R>(
    root_bytes: &[u8],
    mut read_blob: R,
) -> Result<VerifiedAuthority, String>
where
    R: FnMut([u8; 32], u64) -> Result<(Vec<u8>, ObjectIdentity), String>,
{
    let mut pinned_objects = BTreeMap::new();
    let authority =
        verify::verify_authority(root_bytes, policy(), &codec_limits()?, |digest, len| {
            let (bytes, identity) = read_blob(digest, len)?;
            if pinned_objects
                .insert(digest, identity)
                .is_some_and(|prior| prior != identity)
            {
                return Err("F15 object identity changed during cold verification".into());
            }
            Ok(bytes)
        })?;
    if pinned_objects.len() != object_inventory(&authority.root).len() {
        return Err("F15 pinned object inventory differs from root".into());
    }
    Ok(VerifiedAuthority {
        authority,
        pinned_objects,
    })
}

pub(crate) fn verified_inventory(authority: &VerifiedAuthority) -> BTreeMap<String, u64> {
    let mut names = BTreeMap::new();
    let _prior_root = names.insert(format!("{DIR}/{ROOT}"), 0);
    for (digest, len) in object_inventory(&authority.authority.root) {
        let _prior_object = names.insert(object_name(&digest), len);
    }
    names
}

pub(crate) fn verified_matches_coverage(
    authority: &VerifiedAuthority,
    coverage: &quanta_index_contract::FileCoverageSnapshot,
) -> bool {
    root_matches_coverage(&authority.authority.root, coverage)
}

fn root_matches_coverage(
    root: &root::AuthorityRoot,
    coverage: &quanta_index_contract::FileCoverageSnapshot,
) -> bool {
    root.sources.len() == coverage.len()
        && root.sources.iter().all(|row| {
            coverage.get(&row.source.file).is_some_and(|covered| {
                covered.source == row.source
                    && covered.text_admitted == row.text_admitted
                    && covered.language == row.language
            })
        })
}

fn write_object(
    generation_dir: &Path,
    digest: [u8; 32],
    bytes: &[u8],
    next_temp: &mut u64,
) -> Result<(), String> {
    let object_dir = generation_dir.join(DIR).join(OBJECTS);
    let path = object_dir.join(file_name(&digest));
    let expected: [u8; 32] = Sha256::digest(bytes).into();
    if expected != digest {
        return Err("producer blob digest differs from supplied name".into());
    }
    let encoded_len = u64::try_from(bytes.len())
        .map_err(|error| format!("producer object length exceeds u64: {error}"))?;
    if path.exists() {
        let existing = read_object(&object_dir, digest, encoded_len)?;
        if existing != bytes {
            return Err("existing object has different bytes".into());
        }
        return Ok(());
    }
    let (temp, mut file) = loop {
        let temp = object_dir.join(format!(".object-{}-{}.tmp", std::process::id(), *next_temp));
        *next_temp = next_temp
            .checked_add(1)
            .ok_or("object temporary sequence overflow")?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("create object temporary: {error}")),
        }
    };
    file.write_all(bytes)
        .map_err(|error| format!("write object temporary: {error}"))?;
    crate::causal_profile::timed_sync("file_authority_object", || file.sync_all())
        .map_err(|error| format!("sync object temporary: {error}"))?;
    drop(file);
    // link(2) cannot replace a published object if another writer won the
    // digest name. Its bytes are checked before accepting that race.
    match std::fs::hard_link(&temp, &path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = read_object(&object_dir, digest, encoded_len)?;
            if existing != bytes {
                return Err("existing object has different bytes".into());
            }
        }
        Err(error) => return Err(format!("publish object: {error}")),
    }
    std::fs::remove_file(&temp).map_err(|error| format!("retire object temporary: {error}"))?;
    Ok(())
}

fn sync_object_dir(generation_dir: &Path) -> Result<(), CoreError> {
    let path = generation_dir.join(DIR).join(OBJECTS);
    std::fs::File::open(&path)
        .and_then(|dir| {
            crate::causal_profile::timed_sync("file_authority_directory", || dir.sync_all())
        })
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: sync F15 object directory {}: {error}",
                path.display()
            ))
        })
}

fn sealed_base_root(base_dir: &Path) -> Result<root::AuthorityRoot, CoreError> {
    let opened =
        crate::sealed_generation::open_generation_dir_nofollow(base_dir).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: open F15 base {}: {error}",
                base_dir.display()
            ))
        })?;
    let identity = crate::index_store::read_lexical_sealed_identity_at(base_dir, &opened)?;
    let manifest = crate::sealed_generation::read_bound_manifest_at(
        base_dir,
        &opened,
        &identity.manifest_digest,
    )?;
    let name = format!("{DIR}/{ROOT}");
    let commitment = manifest
        .file_authority
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| corrupt(base_dir, &name, "base manifest lacks F15 root"))?;
    let mut file = crate::sealed_generation::open_regular_below(&opened, Path::new(&name))
        .map_err(|error| corrupt(base_dir, &name, &format!("open base root: {error}")))?;
    if file
        .metadata()
        .map_err(|error| corrupt(base_dir, &name, &format!("stat: {error}")))?
        .len()
        != commitment.bytes
    {
        return Err(corrupt(
            base_dir,
            &name,
            "base root length differs from commitment",
        ));
    }
    let maximum = usize::try_from(policy().root_bytes)
        .map_err(|error| CoreError::Storage(format!("lexical: root byte ceiling: {error}")))?;
    let bytes = crate::sealed_generation::read_opened_bounded(&mut file, maximum)
        .map_err(|error| corrupt(base_dir, &name, &format!("read: {error}")))?;
    let observed: [u8; 32] = Sha256::digest(&bytes).into();
    if observed != commitment.sha256 {
        return Err(corrupt(
            base_dir,
            &name,
            "base root digest differs from commitment",
        ));
    }
    let authority = root::AuthorityRoot::decode(&bytes, policy())
        .map_err(|reason| corrupt(base_dir, &name, &reason))?;
    let expected = object_inventory(&authority);
    let expected_count = expected
        .len()
        .checked_add(1)
        .ok_or_else(|| corrupt(base_dir, &name, "base object count overflows"))?;
    if manifest.file_authority.len() != expected_count
        || manifest.file_authority.iter().any(|entry| {
            if entry.name == name {
                return entry != commitment;
            }
            expected
                .iter()
                .find(|(digest, _)| object_name(digest) == entry.name)
                .is_none_or(|(digest, len)| entry.sha256 != *digest || entry.bytes != *len)
        })
    {
        return Err(corrupt(
            base_dir,
            &name,
            "base F15 object commitments differ from root",
        ));
    }
    let coverage_commitment = manifest.source_coverage.as_ref().ok_or_else(|| {
        corrupt(
            base_dir,
            &name,
            "base F15 root has no source coverage commitment",
        )
    })?;
    if coverage_commitment.bytes > crate::sealed_generation::coverage::MAX_COVERAGE_ROOT_BYTES_U64 {
        return Err(corrupt(
            base_dir,
            &name,
            "base source coverage exceeds policy",
        ));
    }
    let coverage_bytes = crate::sealed_generation::coverage::read_committed_coverage_root_at(
        &opened,
        base_dir,
        coverage_commitment,
    )?;
    let coverage = crate::sealed_generation::coverage::decode_coverage_at(
        &opened,
        &coverage_bytes,
        base_dir,
        &identity,
    )?;
    if !root_matches_coverage(&authority, &coverage.coverage) {
        return Err(corrupt(
            base_dir,
            &name,
            "base F15 root differs from committed coverage",
        ));
    }
    Ok(authority)
}

fn replay_object_inherited(
    generation_dir: &Path,
    base_dir: &Path,
    digest: [u8; 32],
    len: u64,
) -> bool {
    let name = object_name(&digest);
    let ours = crate::sealed_generation::open_regular_nofollow(generation_dir, Path::new(&name))
        .and_then(|file| file.metadata());
    let theirs = crate::sealed_generation::open_regular_nofollow(base_dir, Path::new(&name))
        .and_then(|file| file.metadata());
    matches!((ours, theirs), (Ok(ours), Ok(theirs))
        if ours.is_file() && theirs.is_file()
            && ours.len() == len && theirs.len() == len
            && ours.dev() == theirs.dev() && ours.ino() == theirs.ino())
}

fn audit_replayed_objects(
    generation_dir: &Path,
    base_dir: Option<&Path>,
    current: &root::AuthorityRoot,
) -> Result<(u64, u64), CoreError> {
    let base = match base_dir {
        Some(dir) if crate::index_store::sealed_identity_entry_present(dir)? => {
            Some((dir, sealed_base_root(dir)?))
        }
        _ => None,
    };
    let base_inventory = base
        .as_ref()
        .map(|(_, root)| object_inventory(root))
        .unwrap_or_default();
    let object_dir = generation_dir.join(DIR).join(OBJECTS);
    let mut reads = (0_u64, 0_u64);
    for (digest, len) in object_inventory(current) {
        if let Some((dir, _)) = &base
            && base_inventory.get(&digest) == Some(&len)
            && replay_object_inherited(generation_dir, dir, digest, len)
        {
            continue;
        }
        let bytes = read_object(&object_dir, digest, len)
            .map_err(|reason| corrupt(generation_dir, DIR, &reason))?;
        if <[u8; 32]>::from(Sha256::digest(&bytes)) != digest {
            return Err(corrupt(
                generation_dir,
                DIR,
                "replayed object digest differs",
            ));
        }
        reads.0 = reads
            .0
            .checked_add(1)
            .ok_or_else(|| invalid("replay object read count overflow"))?;
        reads.1 = reads
            .1
            .checked_add(len)
            .ok_or_else(|| invalid("replay object byte count overflow"))?;
    }
    Ok(reads)
}

/// Produce or replay one complete F15 authority before the sealed manifest.
/// Staging is unservable and may disappear only after the new root is durable.
pub(crate) fn build_for_seal(
    generation_dir: &Path,
    base_dir: Option<&Path>,
    coverage: &quanta_index_contract::FileCoverageSnapshot,
) -> Result<(Vec<String>, u64, u64, u64, u64), CoreError> {
    let mut source_reads = (0_u64, 0_u64);
    let mut replay_reads = (0_u64, 0_u64);
    let object_dir = generation_dir.join(DIR).join(OBJECTS);
    ensure_local_dir(&generation_dir.join(DIR))?;
    ensure_local_dir(&object_dir)?;
    let root = match read_root(generation_dir)? {
        Some(existing) if root_matches_coverage(&existing, coverage) => {
            replay_reads = audit_replayed_objects(generation_dir, base_dir, &existing)?;
            existing
        }
        _ => {
            let base_root = match base_dir {
                Some(dir) if crate::index_store::sealed_identity_entry_present(dir)? => {
                    Some(sealed_base_root(dir)?)
                }
                _ => None,
            };
            let base_by_key: BTreeMap<SourceFileKey, &root::SourceRow> = base_root
                .as_ref()
                .into_iter()
                .flat_map(|root| root.sources.iter())
                .map(|row| (row.source.file.clone(), row))
                .collect();
            let staging = read_manifest(generation_dir)?.unwrap_or_default();
            let staged_by_key: BTreeMap<SourceFileKey, FileManifestRow> = staging
                .into_iter()
                .map(|row| (row.0.file.clone(), row))
                .collect();
            if staged_by_key.len() != coverage.len() {
                return Err(corrupt(
                    generation_dir,
                    MANIFEST,
                    "staging and coverage source counts differ",
                ));
            }
            let mut changed = BTreeMap::new();
            let mut files_read = 0_u64;
            let mut bytes_read = 0_u64;
            for (key, covered) in coverage {
                let staged = staged_by_key.get(key).ok_or_else(|| {
                    corrupt(
                        generation_dir,
                        MANIFEST,
                        "coverage source is absent from staging",
                    )
                })?;
                if staged.0 != covered.source {
                    return Err(corrupt(
                        generation_dir,
                        MANIFEST,
                        "staging source revision differs from coverage",
                    ));
                }
                let inherited = base_by_key.get(key).is_some_and(|row| {
                    row.source == covered.source
                        && row.text_admitted == covered.text_admitted
                        && row.language == covered.language
                });
                if inherited {
                    continue;
                }
                let name = artifact_name(&covered.source);
                let mut file = crate::sealed_generation::open_regular_nofollow(
                    generation_dir,
                    Path::new(&name),
                )
                .map_err(|error| {
                    corrupt(
                        generation_dir,
                        &name,
                        &format!("open staged source: {error}"),
                    )
                })?;
                let metadata = file
                    .metadata()
                    .map_err(|error| corrupt(generation_dir, &name, &format!("stat: {error}")))?;
                let file_ceiling = u64::try_from(MAX_FILE_BYTES).map_err(|error| {
                    CoreError::Storage(format!("lexical: staged source ceiling: {error}"))
                })?;
                if metadata.len() > file_ceiling {
                    return Err(corrupt(
                        generation_dir,
                        &name,
                        "staged source exceeds 8 MiB",
                    ));
                }
                let bytes =
                    crate::sealed_generation::read_opened_bounded(&mut file, MAX_FILE_BYTES)
                        .map_err(|error| {
                            corrupt(generation_dir, &name, &format!("read: {error}"))
                        })?;
                let sha: [u8; 32] = Sha256::digest(&bytes).into();
                if sha != covered.source.source_sha256 {
                    return Err(corrupt(
                        generation_dir,
                        &name,
                        "staged source digest differs from coverage",
                    ));
                }
                files_read = files_read
                    .checked_add(1)
                    .ok_or_else(|| invalid("source read count overflow"))?;
                let source_len = u64::try_from(bytes.len()).map_err(|error| {
                    CoreError::Storage(format!("lexical: staged source length: {error}"))
                })?;
                bytes_read = bytes_read
                    .checked_add(source_len)
                    .ok_or_else(|| invalid("source read byte count overflow"))?;
                if changed.insert(key.clone(), bytes).is_some() {
                    return Err(invalid("staged source key appears twice in coverage"));
                }
            }
            let mut dispositions = Vec::with_capacity(coverage.len());
            for (key, covered) in coverage {
                if let Some(bytes) = changed.get(key) {
                    dispositions.push(producer::SourceDisposition::Updated {
                        source: covered.source.clone(),
                        bytes,
                        text_admitted: covered.text_admitted,
                        language: covered.language.clone(),
                    });
                } else {
                    dispositions.push(producer::SourceDisposition::Inherited {
                        source: covered.source.clone(),
                        text_admitted: covered.text_admitted,
                        language: covered.language.clone(),
                    });
                }
            }
            let base_read = |digest, len| {
                let dir =
                    base_dir.ok_or_else(|| "inherited source without sealed base".to_owned())?;
                read_object(&dir.join(DIR).join(OBJECTS), digest, len)
            };
            let base = base_root.as_ref().map(|root| producer::CommittedBase {
                root,
                read_blob: &base_read,
            });
            let mut next_temp = 0_u64;
            let mut sink =
                |digest, bytes: &[u8]| write_object(generation_dir, digest, bytes, &mut next_temp);
            let produced = producer::produce_authority(
                &dispositions,
                base.as_ref(),
                policy(),
                root::PREFIX_BITS,
                &mut sink,
            )
            .map_err(|error| match error.kind {
                producer::ProducerErrorKind::CorruptBase => {
                    corrupt(generation_dir, DIR, &error.reason)
                }
                producer::ProducerErrorKind::Invalid | producer::ProducerErrorKind::Limit => {
                    invalid(&error.reason)
                }
            })?;
            // New bytes were hashed against their digest by the sink. The
            // inherited descriptors were bound to the sealed base above;
            // the outer seal measurer verifies newly named objects and
            // carries only same-inode committed base objects.
            sync_object_dir(generation_dir)?;
            crate::index_store::write_atomic_durable(
                &root_path(generation_dir),
                &produced.root_bytes,
                "F15 file authority root",
            )?;
            source_reads = (files_read, bytes_read);
            produced.root
        }
    };
    let expected = object_inventory(&root);
    for entry in std::fs::read_dir(&object_dir)
        .map_err(|error| CoreError::Storage(format!("lexical: list F15 objects: {error}")))?
    {
        let entry = entry
            .map_err(|error| CoreError::Storage(format!("lexical: F15 object entry: {error}")))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !expected.keys().any(|digest| file_name(digest) == name) {
            std::fs::remove_file(entry.path()).map_err(|error| {
                CoreError::Storage(format!("lexical: retire F15 object {name}: {error}"))
            })?;
        }
    }
    sync_object_dir(generation_dir)?;
    let staging_dir = generation_dir.join(DIR).join("staging");
    if staging_dir.exists() {
        std::fs::remove_dir_all(&staging_dir)
            .map_err(|error| CoreError::Storage(format!("lexical: retire F15 staging: {error}")))?;
        std::fs::File::open(generation_dir.join(DIR))
            .and_then(|dir| {
                crate::causal_profile::timed_sync("file_authority_directory", || dir.sync_all())
            })
            .map_err(|error| {
                CoreError::Storage(format!("lexical: sync F15 authority directory: {error}"))
            })?;
    }
    let mut names = vec![format!("{DIR}/{ROOT}")];
    names.extend(expected.keys().map(object_name));
    names.sort();
    Ok((
        names,
        source_reads.0,
        source_reads.1,
        replay_reads.0,
        replay_reads.1,
    ))
}

// The resident estimate is intentionally conservative on a target whose usize
// can exceed u64; every contribution and the total saturate at u64::MAX.
fn saturating_usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).map_or(u64::MAX, std::convert::identity)
}

pub(crate) fn manifest_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(DIR).join(MANIFEST)
}

pub(crate) fn root_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(DIR).join(ROOT)
}

pub(crate) fn read_root(generation_dir: &Path) -> Result<Option<root::AuthorityRoot>, CoreError> {
    let name = format!("{DIR}/{ROOT}");
    let mut file =
        match crate::sealed_generation::open_regular_nofollow(generation_dir, Path::new(&name)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(corrupt(generation_dir, &name, &format!("open: {error}"))),
        };
    let maximum = usize::try_from(policy().root_bytes)
        .map_err(|error| CoreError::Storage(format!("lexical: root byte ceiling: {error}")))?;
    let bytes = crate::sealed_generation::read_opened_bounded(&mut file, maximum)
        .map_err(|error| corrupt(generation_dir, &name, &format!("read: {error}")))?;
    root::AuthorityRoot::decode(&bytes, policy())
        .map(Some)
        .map_err(|reason| corrupt(generation_dir, &name, &reason))
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
    format!("{DIR}/staging/{}", file_name(&source.source_sha256))
}

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidContract(format!("lexical file authority: {reason}"))
}

fn corrupt(generation_dir: &Path, name: &str, reason: &str) -> CoreError {
    crate::index_store::sidecar_corrupt(generation_dir, name, reason)
}

fn ensure_local_dir(path: &Path) -> Result<(), CoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(CoreError::Storage(format!(
            "lexical: file authority directory is not a local directory: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(path)
            .map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: create file authority directory {}: {error}",
                    path.display()
                ))
            }),
        Err(error) => Err(CoreError::Storage(format!(
            "lexical: inspect file authority directory {}: {error}",
            path.display()
        ))),
    }
}

pub(crate) fn decode_verified_manifest(
    bytes: &[u8],
    generation_dir: &Path,
) -> Result<Vec<FileManifestRow>, CoreError> {
    let entries: Vec<FileManifestRow> = crate::channel_payloads::decode_cbor_exact(bytes)
        .map_err(|error| corrupt(generation_dir, MANIFEST, &format!("decode: {error}")))?;
    let mut previous: Option<&SourceFileKey> = None;
    let mut postings = 0_u64;
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
        postings = postings
            .checked_add(u64::from(*count))
            .ok_or_else(|| corrupt(generation_dir, MANIFEST, "posting count overflow"))?;
        if postings > u64::from(crate::FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT) {
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
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let legacy = generation_dir.join(DIR).join("manifest.cbor");
            match std::fs::symlink_metadata(&legacy) {
                Ok(_) => return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                    message: format!("lexical: legacy file authority {} requires F15 rebuild", legacy.display()),
                }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(CoreError::Storage(format!("lexical: inspect legacy file authority {}: {error}", legacy.display()))),
            }
            return read_root(generation_dir).map(|root| {
                root.map(|root| {
                    root.sources
                        .into_iter()
                        .map(|row| (row.source, row.posting_memberships))
                        .collect()
                })
            });
        }
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
    let inherited_root = read_root(generation_dir)?;
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
    let total_postings = sources.iter().try_fold(0_u64, |total, row| {
        total
            .checked_add(u64::from(row.1))
            .ok_or_else(|| invalid("file trigram posting count overflow"))
    })?;
    if total_postings > u64::from(crate::FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT) {
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
            match crate::sealed_generation::open_regular_nofollow(generation_dir, Path::new(&name))
            {
                Ok(file) => usize::try_from(
                    file.metadata()
                        .map_err(|error| {
                            CoreError::Storage(format!(
                                "lexical: stat file authority {name}: {error}"
                            ))
                        })?
                        .len(),
                )
                .map_err(|error| {
                    CoreError::Storage(format!("lexical: file authority size {name}: {error}"))
                })?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let row = inherited_root
                        .as_ref()
                        .and_then(|root| root.sources.iter().find(|row| row.source == *source))
                        .ok_or_else(|| {
                            corrupt(
                                generation_dir,
                                &name,
                                "source has neither staged bytes nor inherited root row",
                            )
                        })?;
                    usize::try_from(row.source_bytes).map_err(|error| {
                        CoreError::Storage(format!(
                            "lexical: inherited source size {name}: {error}"
                        ))
                    })?
                }
                Err(error) => {
                    return Err(CoreError::Storage(format!(
                        "lexical: open file authority {name}: {error}"
                    )));
                }
            }
        };
        total = total
            .checked_add(bytes)
            .ok_or_else(|| invalid("source byte sum overflows"))?;
        let total_bytes = u64::try_from(total).map_err(|error| {
            CoreError::Storage(format!("lexical: source byte sum width: {error}"))
        })?;
        if total_bytes > u64::from(MAX_TOTAL_SOURCE_BYTES) {
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
pub(crate) fn apply_plan(
    generation_dir: &Path,
    plan: FileAuthorityDelta,
) -> Result<u64, CoreError> {
    let FileAuthorityDelta {
        sources,
        writes,
        encoded,
    } = plan;
    let dir = generation_dir.join(DIR).join("staging");
    ensure_local_dir(&generation_dir.join(DIR))?;
    ensure_local_dir(&dir)?;
    let source_write_started = Instant::now();
    for (digest, bytes) in &writes {
        let path = dir.join(file_name(digest));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => file.write_all(bytes).map_err(|error| {
                CoreError::Storage(format!("lexical: stage source {}: {error}", path.display()))
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let relative = format!("{DIR}/staging/{}", file_name(digest));
                let mut opened = crate::sealed_generation::open_regular_nofollow(
                    generation_dir,
                    Path::new(&relative),
                )
                .map_err(|error| {
                    corrupt(
                        generation_dir,
                        &relative,
                        &format!("open existing staged source: {error}"),
                    )
                })?;
                let observed =
                    crate::sealed_generation::read_opened_bounded(&mut opened, MAX_FILE_BYTES)
                        .map_err(|error| {
                            corrupt(
                                generation_dir,
                                &relative,
                                &format!("read existing staged source: {error}"),
                            )
                        })?;
                if observed != *bytes {
                    return Err(corrupt(
                        generation_dir,
                        &relative,
                        "staged digest path has different bytes",
                    ));
                }
            }
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: stage source {}: {error}",
                    path.display()
                )));
            }
        }
    }
    let source_write_ns = crate::stage_timing::elapsed_stage_ns(source_write_started)?;
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
        if name != "manifest.cbor"
            && !keep.contains(&name)
            && !crate::index_store::is_durable_write_temporary(&name)
        {
            std::fs::remove_file(entry.path()).map_err(|error| {
                CoreError::Storage(format!("lexical: retire file authority {name}: {error}"))
            })?;
        }
    }
    Ok(source_write_ns)
}

pub(crate) fn from_v15_verified(
    verified: VerifiedAuthority,
    object_dir: PathBuf,
    budget: Option<&RequestBudgetV1>,
) -> Result<FileAuthority, CoreError> {
    let VerifiedAuthority {
        authority,
        pinned_objects,
    } = verified;
    let verify::VerifiedAuthority {
        root,
        files,
        posting_directory,
    } = authority;
    let mut by_key = BTreeMap::new();
    for file in files {
        checkpoint(budget)?;
        if by_key.insert(file.source.file.clone(), file).is_some() {
            return Err(invalid("duplicate verified source file"));
        }
        checkpoint(budget)?;
    }
    let ordered_keys = by_key.keys().cloned().collect();
    let mut keys_by_id = BTreeMap::new();
    let mut ids_by_key = BTreeMap::new();
    for row in &root.sources {
        checkpoint(budget)?;
        let key = row.source.file.clone();
        let file = by_key
            .get(&key)
            .ok_or_else(|| invalid("verified row lacks source"))?;
        if file.source != row.source
            || file.text_admitted != row.text_admitted
            || file.language != row.language
        {
            return Err(invalid("verified source differs from root metadata"));
        }
        if file.expected_postings != row.posting_memberships {
            return Err(invalid("verified source posting count differs from root"));
        }
        if keys_by_id.insert(row.source_id, key.clone()).is_some()
            || ids_by_key.insert(key, row.source_id).is_some()
        {
            return Err(invalid("duplicate stable source id or key"));
        }
    }
    if by_key.len() != root.sources.len() {
        return Err(invalid("verified source count differs from root"));
    }
    let term_directory_charge = root
        .term_directory_charge(policy())
        .map_err(|reason| invalid(&reason))?;
    checkpoint(budget)?;
    let authority = FileAuthority {
        files: by_key,
        ordered_keys,
        root,
        posting_directory,
        term_directory_charge,
        object_dir,
        pinned_objects,
        keys_by_id,
        ids_by_key,
    };
    let _resident_bytes = authority.checked_resident_bytes()?;
    Ok(authority)
}

struct NormalizedSurfacesPlan<'a> {
    path: crate::normalize::NfcFoldPlan<'a>,
    content: Option<crate::normalize::NfcFoldPlan<'a>>,
}

impl<'a> NormalizedSurfacesPlan<'a> {
    fn new_with_budget(
        path: &'a str,
        raw: Option<&'a str>,
        scratch_current: usize,
        scratch_ceiling: usize,
    ) -> Result<Self, crate::normalize::NfcFoldBuildError> {
        Ok(Self {
            path: crate::normalize::NfcFoldPlan::new_with_budget(
                path,
                scratch_current,
                scratch_ceiling,
            )?,
            content: raw
                .map(|raw| {
                    crate::normalize::NfcFoldPlan::new_with_budget(
                        raw,
                        scratch_current,
                        scratch_ceiling,
                    )
                })
                .transpose()?,
        })
    }

    fn lengths(&self) -> (usize, usize, usize, usize) {
        (
            self.path.nfc_bytes(),
            self.path.folded_bytes(),
            self.content
                .as_ref()
                .map_or(0, crate::normalize::NfcFoldPlan::nfc_bytes),
            self.content
                .as_ref()
                .map_or(0, crate::normalize::NfcFoldPlan::folded_bytes),
        )
    }

    fn build_with_budget(
        self,
        scratch_current: usize,
        scratch_ceiling: usize,
    ) -> Result<(String, String, Option<String>, Option<String>), crate::normalize::NfcFoldBuildError>
    {
        let (indexed_path_bytes, folded_path_bytes, indexed_text_bytes, folded_text_bytes) =
            self.lengths();
        let total = [
            indexed_path_bytes,
            folded_path_bytes,
            indexed_text_bytes,
            folded_text_bytes,
        ]
        .into_iter()
        .try_fold(0_usize, usize::checked_add)
        .ok_or(crate::normalize::NfcFoldBuildError::LengthOverflow)?;
        let admitted = scratch_current
            .checked_add(total)
            .ok_or(crate::normalize::NfcFoldBuildError::LengthOverflow)?;
        if admitted > scratch_ceiling {
            return Err(crate::normalize::NfcFoldBuildError::ScratchExceeded);
        }
        let path_output = indexed_path_bytes
            .checked_add(folded_path_bytes)
            .ok_or(crate::normalize::NfcFoldBuildError::LengthOverflow)?;
        let (path, folded_path) = self
            .path
            .build_with_budget(scratch_current, scratch_ceiling)?;
        let (content, folded_content) = match self.content {
            Some(plan) => {
                let content_base = scratch_current
                    .checked_add(path_output)
                    .ok_or(crate::normalize::NfcFoldBuildError::LengthOverflow)?;
                let (content, folded_content) =
                    plan.build_with_budget(content_base, scratch_ceiling)?;
                (Some(content), Some(folded_content))
            }
            None => (None, None),
        };
        Ok((path, folded_path, content, folded_content))
    }
}

fn normalized_surfaces(
    path: &str,
    raw: Option<&str>,
    scratch_current: usize,
    scratch_ceiling: usize,
) -> Result<(String, String, Option<String>, Option<String>), CoreError> {
    NormalizedSurfacesPlan::new_with_budget(path, raw, scratch_current, scratch_ceiling)
        .and_then(|plan| plan.build_with_budget(scratch_current, scratch_ceiling))
        .map_err(|error| invalid(&format!("normalization: {error}")))
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
    let scratch_ceiling = usize::try_from(policy().bucket_scratch_bytes)
        .map_err(|error| invalid(&format!("normalization scratch ceiling width: {error}")))?;
    let (_, folded_path, _, folded_content) = normalized_surfaces(
        source.file.repo_relative_path.as_str(),
        raw,
        TRIGRAM_BITMAP_BYTES,
        scratch_ceiling,
    )?;
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
            usize::try_from(crate::FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT)
                .map_err(|error| invalid(&format!("posting limit exceeds usize: {error}")))?,
        )?;
    }
    u32::try_from(total).map_err(|error| invalid(&format!("posting count: {error}")))
}

fn count_posting_memberships(
    source: &[u8],
    bitmap: &mut [u8],
    total: &mut usize,
    memberships: usize,
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
            if *total > memberships {
                return Err(invalid(
                    "file trigram posting membership admission exceeded",
                ));
            }
        }
    }
    Ok(())
}

fn checkpoint(budget: Option<&RequestBudgetV1>) -> Result<(), CoreError> {
    budget.map_or(Ok(()), |budget| {
        budget.checkpoint("lexical:cold-open:file-index")
    })
}

#[cfg(test)]
pub(crate) fn from_test_files(
    mut files: Vec<SourceFile>,
    generation_dir: &Path,
) -> Result<FileAuthority, CoreError> {
    files.sort_by(|left, right| left.source.file.cmp(&right.source.file));
    let canonical_generation_dir = std::fs::canonicalize(generation_dir).map_err(|error| {
        CoreError::Storage(format!("lexical: canonicalize test generation: {error}"))
    })?;
    let generation_dir = canonical_generation_dir.as_path();
    ensure_local_dir(&generation_dir.join(DIR))?;
    let object_dir = generation_dir.join(DIR).join(OBJECTS);
    ensure_local_dir(&object_dir)?;
    let dispositions: Vec<_> = files
        .iter()
        .map(|file| producer::SourceDisposition::Updated {
            source: file.source.clone(),
            bytes: &file.bytes,
            text_admitted: file.text_admitted,
            language: file.language.clone(),
        })
        .collect();
    let mut next_temp = 0_u64;
    let mut sink =
        |digest, bytes: &[u8]| write_object(generation_dir, digest, bytes, &mut next_temp);
    let produced =
        producer::produce_authority(&dispositions, None, policy(), root::PREFIX_BITS, &mut sink)
            .map_err(|error| invalid(&format!("test F15 producer: {}", error.reason)))?;
    for (expected, row) in files.iter().zip(&produced.root.sources) {
        if expected.source != row.source {
            return Err(invalid("test source differs from canonical F15 root"));
        }
        if expected.expected_postings != row.posting_memberships {
            return Err(invalid(
                "test source expected postings differ from canonical F15 root",
            ));
        }
    }
    sync_object_dir(generation_dir)?;
    crate::index_store::write_atomic_durable(
        &root_path(generation_dir),
        &produced.root_bytes,
        "test F15 root",
    )?;
    let verified = verify_v15(&produced.root_bytes, |digest, len| {
        read_object_pinned(&object_dir, digest, len, None).map_err(|error| error.to_string())
    })
    .map_err(|reason| corrupt(generation_dir, ROOT, &reason))?;
    from_v15_verified(verified, object_dir, None)
}

#[cfg(test)]
mod tests {
    use super::{
        TRIGRAM_BITMAP_BYTES, decode_verified_manifest, file_name, normalized_surfaces,
        source_posting_memberships,
    };
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        RepoId, RepoRelativePath, RevisionId, SourceFileKey, SourceFileRevision,
    };
    use sha2::{Digest as _, Sha256};

    fn source() -> SourceFileRevision {
        SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("repo").expect("repo"),
                repo_relative_path: RepoRelativePath::new("src/a.rs"),
            },
            revision_id: RevisionId::new("revision").expect("revision"),
            source_sha256: [7; 32],
        }
    }

    #[test]
    fn staging_manifest_accepts_xl_memberships_and_refuses_global_overflow() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut encoded = Vec::new();
        ciborium::into_writer(&vec![(source(), 17_715_020_u32)], &mut encoded).expect("encode");
        assert_eq!(
            decode_verified_manifest(&encoded, dir.path())
                .expect("within F15 cap")
                .len(),
            1
        );
        encoded.clear();
        ciborium::into_writer(&vec![(source(), 20_000_001_u32)], &mut encoded).expect("encode");
        assert!(decode_verified_manifest(&encoded, dir.path()).is_err());
    }

    #[test]
    fn staging_counter_keeps_path_and_content_memberships_distinct() {
        let mut bitmap = vec![0_u8; TRIGRAM_BITMAP_BYTES];
        let path_only = source_posting_memberships(&source(), b"\xff", false, &mut bitmap)
            .expect("binary path");
        assert_eq!(
            path_only,
            source_posting_memberships(&source(), b"abc", false, &mut bitmap)
                .expect("text path only")
        );
        let with_content =
            source_posting_memberships(&source(), b"abc", true, &mut bitmap).expect("text content");
        assert!(with_content > path_only);
        assert!(source_posting_memberships(&source(), b"\xff", true, &mut bitmap).is_err());
    }

    #[test]
    fn resident_charge_counts_utf8_fold_expansion() {
        let mut source = source();
        source.file.repo_relative_path = RepoRelativePath::new("src/İ.go");
        source.revision_id = RevisionId::new("rev").expect("revision");
        let language = LanguageCode::new("go").expect("language");
        let (indexed_path, folded_path, indexed_text, folded_text) =
            normalized_surfaces("src/İ.go", Some("İ"), TRIGRAM_BITMAP_BYTES, 128 << 20)
                .expect("normalized surfaces");
        assert_eq!(indexed_path.len(), 9);
        assert_eq!(folded_path.len(), 10);
        assert_eq!(indexed_text.as_ref().map(String::len), Some(2));
        assert_eq!(folded_text.as_ref().map(String::len), Some(3));
        let charge = super::root::resident_file_charge(
            &source,
            &language,
            2,
            indexed_path.len(),
            folded_path.len(),
            indexed_text.as_ref().map_or(0, String::len),
            folded_text.as_ref().map_or(0, String::len),
        )
        .expect("checked charge");
        assert_eq!(charge, 1138);
    }

    #[test]
    fn replay_audit_counts_only_read_objects_and_refuses_corrupt_bytes() {
        let generation = tempfile::tempdir().expect("tempdir");
        let generation_dir = generation
            .path()
            .canonicalize()
            .expect("canonical generation");
        let object_dir = generation_dir.join(super::DIR).join(super::OBJECTS);
        std::fs::create_dir_all(&object_dir).expect("objects");
        let body = b"canonical object";
        let body_len = u64::try_from(body.len()).expect("fixture body length fits u64");
        let digest: [u8; 32] = Sha256::digest(body).into();
        let object = object_dir.join(super::file_name(&digest));
        std::fs::write(&object, body).expect("object");
        let root = super::root::AuthorityRoot {
            policy_sha256: super::policy().digest(),
            next_source_id: 1,
            sources: vec![],
            packs: vec![super::root::Partition {
                prefix_bits: super::root::PREFIX_BITS,
                prefix: [0; 32],
                sha256: digest,
                bytes: body_len,
                entries: 1,
                terms: 0,
            }],
            path_postings: vec![],
            content_postings: vec![],
        };
        assert_eq!(
            super::audit_replayed_objects(&generation_dir, None, &root).expect("audit"),
            (1, body_len)
        );
        std::fs::write(&object, b"corrupt!! object").expect("in-place corruption");
        assert!(super::audit_replayed_objects(&generation_dir, None, &root).is_err());
        std::fs::remove_file(&object).expect("replace");
        std::fs::write(&object, b"replaced! object").expect("replacement");
        assert!(super::audit_replayed_objects(&generation_dir, None, &root).is_err());
    }

    #[test]
    fn replay_inheritance_requires_same_inode_and_length() {
        let family = tempfile::tempdir().expect("family");
        let family_dir = family.path().canonicalize().expect("canonical family");
        let base = family_dir.join("base");
        let target = family_dir.join("target");
        for generation in [&base, &target] {
            std::fs::create_dir_all(generation.join(super::DIR).join(super::OBJECTS))
                .expect("objects");
        }
        let body = b"same inode";
        let body_len = u64::try_from(body.len()).expect("fixture body length fits u64");
        let digest: [u8; 32] = Sha256::digest(body).into();
        let base_object = base.join(super::object_name(&digest));
        let target_object = target.join(super::object_name(&digest));
        std::fs::write(&base_object, body).expect("base object");
        std::fs::hard_link(&base_object, &target_object).expect("inherited object");
        assert!(super::replay_object_inherited(
            &target, &base, digest, body_len
        ));
        std::fs::remove_file(&target_object).expect("unlink inherited");
        std::fs::write(&target_object, body).expect("same bytes, new inode");
        assert!(!super::replay_object_inherited(
            &target, &base, digest, body_len
        ));
    }

    #[test]
    fn object_names_are_lowercase_hex() {
        assert_eq!(file_name(&[0xab; 32]), format!("{}.bin", "ab".repeat(32)));
        assert_eq!(file_name(&[0x05; 32]), format!("{}.bin", "05".repeat(32)));
    }

    #[test]
    fn legacy_unsealed_manifest_requires_rebuild() {
        let generation = tempfile::tempdir().expect("tempdir");
        let generation_dir = generation
            .path()
            .canonicalize()
            .expect("canonical generation");
        std::fs::create_dir(generation_dir.join(super::DIR)).expect("authority dir");
        std::fs::write(
            generation_dir.join(super::DIR).join("manifest.cbor"),
            b"old",
        )
        .expect("legacy file");
        let result = super::read_manifest(&generation_dir);
        assert!(matches!(
            result,
            Err(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                ..
            })
        ), "legacy manifest result: {result:?}");
    }
}
