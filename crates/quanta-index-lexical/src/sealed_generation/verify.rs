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

use std::collections::BTreeSet;
use std::path::Path;

use quanta_index_contract::GenerationSnapshot;
use quanta_index_core::CoreError;
use quanta_index_core::domains::generation::SealedArtifactCommitmentV1;
use tantivy::Index;

use crate::overlay_codec::OverlayFamily;
use crate::sealed_generation::index_files::referenced_index_files;
use crate::sealed_generation::manifest::{LexicalSealedManifest, read_bound_manifest};
use crate::text_authority::{
    ShardBody, TEXT_AUTHORITY_DIR_NAME, TEXT_AUTHORITY_MANIFEST_FILE_NAME, TextAuthorityManifest,
    load_shard, sha256_of_bytes, text_authority_dir,
};
use crate::{OverlaySnapshot, TANTIVY_INDEX_META_FILE_NAME};

/// What a door does with each decoded file.
pub(crate) trait SealedGenerationVisitor {
    /// One text-authority shard, proved and decoded, in ascending index
    /// order.
    fn text_authority_shard(&mut self, index: u64, body: ShardBody) -> Result<(), CoreError>;

    /// One overlay family's snapshot, proved and decoded, in family order.
    fn overlay(&mut self, snapshot: OverlaySnapshot) -> Result<(), CoreError>;
}

/// The validator's visitor: proves and decodes, keeps nothing.
pub(crate) struct DiscardingVisitor;

impl SealedGenerationVisitor for DiscardingVisitor {
    fn text_authority_shard(&mut self, _index: u64, _body: ShardBody) -> Result<(), CoreError> {
        Ok(())
    }

    fn overlay(&mut self, _snapshot: OverlaySnapshot) -> Result<(), CoreError> {
        Ok(())
    }
}

/// What the walk proved and opened, beyond what the visitor kept.
pub(crate) struct VerifiedGeneration {
    pub(crate) manifest: LexicalSealedManifest,
    /// The index, opened from the sealed commit with the tokenizers
    /// registered.
    pub(crate) index: Index,
}

/// Prove `generation_dir` against the manifest sealed for `identity`.
pub(crate) fn walk_sealed_generation<V: SealedGenerationVisitor>(
    generation_dir: &Path,
    identity: &GenerationSnapshot,
    visitor: &mut V,
) -> Result<VerifiedGeneration, CoreError> {
    // A generation the scrub proved corrupt is refused at every door.
    crate::sealed_generation::refuse_if_quarantined(generation_dir)?;
    let manifest = read_bound_manifest(generation_dir, &identity.manifest_digest)?;
    let _meta_bytes = read_committed(generation_dir, &manifest.index_meta)?;
    let index = crate::index_store::open_sealed_index(generation_dir)?;
    verify_index_segments(generation_dir, &index, &manifest.index_segments)?;
    verify_overlays(generation_dir, &manifest, visitor)?;
    verify_text_authority(generation_dir, manifest.text_authority.as_deref(), visitor)?;
    Ok(VerifiedGeneration { manifest, index })
}

/// Read one committed file whole and prove its length and digest.
fn read_committed(
    generation_dir: &Path,
    artifact: &SealedArtifactCommitmentV1,
) -> Result<Vec<u8>, CoreError> {
    let path = generation_dir.join(&artifact.name);
    let bytes = std::fs::read(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            crate::index_store::sidecar_corrupt(generation_dir, &artifact.name, "missing")
        } else {
            CoreError::Storage(format!(
                "lexical: read committed file {}: {error}",
                path.display()
            ))
        }
    })?;
    let length = crate::channel_payloads::count_from_len(bytes.len())?;
    if length != artifact.bytes {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &artifact.name,
            &format!("{length} bytes on disk, {} committed", artifact.bytes),
        ));
    }
    if sha256_of_bytes(&bytes) != artifact.sha256 {
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
    Ok(bytes)
}

/// The segment files the (already proved) commit references are exactly
/// the listed ones, each present at its committed length.
fn verify_index_segments(
    generation_dir: &Path,
    index: &Index,
    committed: &[SealedArtifactCommitmentV1],
) -> Result<(), CoreError> {
    let referenced = referenced_index_files(index, generation_dir)?;
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
        let metadata = std::fs::metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                crate::index_store::sidecar_corrupt(generation_dir, &artifact.name, "missing")
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
fn verify_overlays<V: SealedGenerationVisitor>(
    generation_dir: &Path,
    manifest: &LexicalSealedManifest,
    visitor: &mut V,
) -> Result<(), CoreError> {
    for family in OverlayFamily::ALL {
        match manifest.overlay(family) {
            Some(artifact) => {
                let bytes = read_committed(generation_dir, artifact)?;
                let snapshot =
                    crate::overlay_codec::decode_overlay(family, &bytes, generation_dir)?;
                visitor.overlay(snapshot)?;
            }
            None => {
                if family.path(generation_dir).exists() {
                    return Err(crate::index_store::sidecar_corrupt(
                        generation_dir,
                        family.file_name(),
                        "present although the seal committed to no such overlay",
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The `text-authority/` tree is exactly what the seal listed, and every
/// shard decodes to what its manifest says.
fn verify_text_authority<V: SealedGenerationVisitor>(
    generation_dir: &Path,
    committed: Option<&[SealedArtifactCommitmentV1]>,
    visitor: &mut V,
) -> Result<(), CoreError> {
    let dir = text_authority_dir(generation_dir);
    let files = match (committed, dir.is_dir()) {
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
    let manifest_bytes = read_committed(generation_dir, manifest_commitment)?;
    let manifest = TextAuthorityManifest::decode(&manifest_bytes, generation_dir)?;
    ensure_text_authority_listing(generation_dir, &manifest, &manifest_name, files)?;
    ensure_text_authority_directory(generation_dir, &dir, files)?;
    for entry in &manifest.shards {
        let body = load_shard(generation_dir, entry)?;
        visitor.text_authority_shard(entry.index, body)?;
    }
    Ok(())
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
    generation_dir: &Path,
    dir: &Path,
    committed: &[SealedArtifactCommitmentV1],
) -> Result<(), CoreError> {
    let prefix = format!("{TEXT_AUTHORITY_DIR_NAME}/");
    let owned: BTreeSet<&str> = committed
        .iter()
        .filter_map(|artifact| artifact.name.strip_prefix(prefix.as_str()))
        .collect();
    for entry in std::fs::read_dir(dir).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: list text authority directory {}: {error}",
            dir.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            CoreError::Storage(format!(
                "lexical: read text authority entry in {}: {error}",
                dir.display()
            ))
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let qualified = format!("{prefix}{name}");
        if !owned.contains(name.as_ref()) {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                &qualified,
                "present although the seal did not commit to it",
            ));
        }
        let file_type = entry.file_type().map_err(|error| {
            CoreError::Storage(format!(
                "lexical: inspect text authority entry {}: {error}",
                entry.path().display()
            ))
        })?;
        if !file_type.is_file() {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                &qualified,
                "is not a regular file",
            ));
        }
    }
    Ok(())
}
