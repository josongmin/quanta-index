//! The sealed generation manifest: what a sealed semantic generation promises
//! a query can open (QI-BB-017).
//!
//! The scope manifest (`semantic-manifest.cbor`) carries the row root and the
//! membership root, and before this every open re-derived both by streaming
//! every row of both tables — once at activation, again at the first query,
//! again after every eviction. That scan proves content, but content in a
//! sealed generation never changes: every file `LanceDB` writes is an
//! immutable versioned object (G0-S). So the seal now commits to the files
//! once, and every door re-measures files instead of rows.
//!
//! The sealed manifest names every file under `dataset/` plus the scope
//! manifest and the build contract, each with its length and SHA-256, and it
//! carries the identity's `manifest_digest` so the two bind each other. It
//! is written after the scope manifest and before the sealed marker, so a
//! sealed marker implies a sealed manifest. Verification is a streamed hash
//! of every committed file with bounded memory, refuses missing, extra,
//! truncated and rewritten files, and never decodes a row.

use std::path::{Path, PathBuf};

use quanta_index_core::{
    CoreError, SealedArtifactCommitmentV1, commit_tree_v1, sha256_of_file,
    verify_tree_commitment_v1,
};

use crate::layout::{self, BUILD_CONTRACT_FILE_NAME, DATASET_DIR_NAME, MANIFEST_FILE_NAME};

pub(crate) const SEALED_MANIFEST_FILE_NAME: &str = "semantic-sealed-manifest.cbor";
const SEALED_MANIFEST_FORMAT_VERSION: u32 = 1;

pub(crate) fn sealed_manifest_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(SEALED_MANIFEST_FILE_NAME)
}

/// What a sealed semantic generation promises a query can open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SemanticSealedManifestV1 {
    format_version: u32,
    manifest_digest: String,
    /// Length and SHA-256 of `semantic-manifest.cbor`.
    scope_manifest: (u64, [u8; 32]),
    /// Length and SHA-256 of `semantic-build-contract.cbor`, which every
    /// current-format generation carries.
    build_contract: (u64, [u8; 32]),
    /// Every file under `dataset/`, in path order.
    artifacts: Vec<SealedArtifactCommitmentV1>,
}

/// Wire shape: a fixed-order CBOR array so the encoding is auditable without
/// a derive.
type SealedManifestRowV1 = (
    u32,
    String,
    (u64, [u8; 32]),
    (u64, [u8; 32]),
    Vec<(String, u64, [u8; 32])>,
);

impl SemanticSealedManifestV1 {
    fn to_row(&self) -> SealedManifestRowV1 {
        (
            self.format_version,
            self.manifest_digest.clone(),
            self.scope_manifest,
            self.build_contract,
            self.artifacts
                .iter()
                .map(|artifact| (artifact.name.clone(), artifact.bytes, artifact.sha256))
                .collect(),
        )
    }

    fn from_row(row: SealedManifestRowV1) -> Self {
        let (format_version, manifest_digest, scope_manifest, build_contract, artifacts) = row;
        Self {
            format_version,
            manifest_digest,
            scope_manifest,
            build_contract,
            artifacts: artifacts
                .into_iter()
                .map(|(name, bytes, sha256)| SealedArtifactCommitmentV1 {
                    name,
                    bytes,
                    sha256,
                })
                .collect(),
        }
    }

    /// Total committed dataset bytes; the resident-size estimate an open
    /// reports without walking the tree a second time.
    pub(crate) fn dataset_bytes(&self) -> u64 {
        self.artifacts.iter().fold(0_u64, |total, artifact| {
            total.saturating_add(artifact.bytes)
        })
    }
}

fn measure(path: &Path, label: &str) -> Result<(u64, [u8; 32]), CoreError> {
    sha256_of_file(path).map_err(|error| {
        CoreError::Storage(format!(
            "semantic: read {label} {} for commitment: {error}",
            path.display()
        ))
    })
}

/// Measure the promoted generation as sealed and return its manifest bytes.
///
/// Runs after the dataset and the build contract are promoted and the scope
/// manifest is written, and before the sealed marker, so a crash in between
/// leaves an unsealed generation (no marker), never a sealed one without a
/// commitment.
pub(crate) fn build_sealed_manifest_bytes(
    generation_dir: &Path,
    manifest_digest: &str,
) -> Result<Vec<u8>, CoreError> {
    let scope_manifest = measure(&layout::manifest_path(generation_dir), "scope manifest")?;
    let build_contract = measure(
        &layout::build_contract_path(generation_dir),
        "build contract",
    )?;
    let artifacts = commit_tree_v1(&layout::dataset_dir(generation_dir), DATASET_DIR_NAME)
        .map_err(|error| {
            CoreError::Storage(format!(
                "semantic: commit dataset tree {} at seal: {error}",
                generation_dir.display()
            ))
        })?;
    if artifacts.is_empty() {
        return Err(CoreError::Storage(format!(
            "semantic: refusing to seal {} with an empty dataset tree",
            generation_dir.display()
        )));
    }
    let manifest = SemanticSealedManifestV1 {
        format_version: SEALED_MANIFEST_FORMAT_VERSION,
        manifest_digest: manifest_digest.to_string(),
        scope_manifest,
        build_contract,
        artifacts,
    };
    let mut bytes = Vec::new();
    ciborium::into_writer(&manifest.to_row(), &mut bytes).map_err(|error| {
        CoreError::Storage(format!(
            "semantic: encode sealed generation manifest: {error}"
        ))
    })?;
    Ok(bytes)
}

fn read_sealed_manifest(generation_dir: &Path) -> Result<SemanticSealedManifestV1, CoreError> {
    let path = sealed_manifest_path(generation_dir);
    let bytes = std::fs::read(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CoreError::Typed {
                code: "GENERATION_MANIFEST_MISSING".to_string(),
                message: format!(
                    "semantic: sealed generation has no content manifest at {}; it predates the sealed-manifest format and requires explicit migration",
                    path.display()
                ),
            }
        } else {
            CoreError::Storage(format!(
                "semantic: read sealed generation manifest {}: {error}",
                path.display()
            ))
        }
    })?;
    let row: SealedManifestRowV1 = ciborium::from_reader(bytes.as_slice()).map_err(|error| {
        CoreError::Storage(format!(
            "semantic: decode sealed generation manifest {}: {error}",
            path.display()
        ))
    })?;
    let manifest = SemanticSealedManifestV1::from_row(row);
    if manifest.format_version == SEALED_MANIFEST_FORMAT_VERSION {
        Ok(manifest)
    } else {
        Err(CoreError::Typed {
            code: "GENERATION_MANIFEST_FORMAT_UNSUPPORTED".to_string(),
            message: format!(
                "semantic: sealed generation manifest {} has format {} (supported {})",
                path.display(),
                manifest.format_version,
                SEALED_MANIFEST_FORMAT_VERSION
            ),
        })
    }
}

fn sidecar_corrupt(generation_dir: &Path, detail: &str) -> CoreError {
    CoreError::Typed {
        code: "GENERATION_SIDECAR_CORRUPT".to_string(),
        message: format!(
            "semantic: sealed generation {} does not match its manifest: {detail}",
            generation_dir.display()
        ),
    }
}

/// Prove that what is on disk is what the seal committed to.
///
/// This is the check the activation validator and the cold open share, so
/// activation can only ack a generation a query can open. It hashes the
/// scope manifest, the build contract and every dataset file — once per
/// residency, thanks to the snapshot registry — and refuses on any missing,
/// extra, truncated or rewritten file and on a manifest whose digest is not
/// the identity's.
pub(crate) fn verify_sealed_manifest(
    generation_dir: &Path,
    manifest_digest: &str,
) -> Result<SemanticSealedManifestV1, CoreError> {
    let manifest = read_sealed_manifest(generation_dir)?;
    if manifest.manifest_digest != manifest_digest {
        return Err(CoreError::Typed {
            code: "GENERATION_IDENTITY_DIGEST_MISMATCH".to_string(),
            message: format!(
                "semantic: sealed manifest under {} was written for digest {} but the identity says {manifest_digest}",
                generation_dir.display(),
                manifest.manifest_digest
            ),
        });
    }
    for (label, path, committed) in [
        (
            MANIFEST_FILE_NAME,
            layout::manifest_path(generation_dir),
            manifest.scope_manifest,
        ),
        (
            BUILD_CONTRACT_FILE_NAME,
            layout::build_contract_path(generation_dir),
            manifest.build_contract,
        ),
    ] {
        if !path.is_file() {
            return Err(sidecar_corrupt(
                generation_dir,
                &format!("{label}: missing"),
            ));
        }
        let (bytes, sha256) = measure(&path, label)?;
        if bytes != committed.0 {
            return Err(sidecar_corrupt(
                generation_dir,
                &format!("{label}: {bytes} bytes on disk, {} committed", committed.0),
            ));
        }
        if sha256 != committed.1 {
            return Err(sidecar_corrupt(
                generation_dir,
                &format!("{label}: content digest differs from the committed digest"),
            ));
        }
    }
    let verified = verify_tree_commitment_v1(
        &layout::dataset_dir(generation_dir),
        DATASET_DIR_NAME,
        &manifest.artifacts,
    )
    .map_err(|error| {
        CoreError::Storage(format!(
            "semantic: re-measure dataset tree {}: {error}",
            generation_dir.display()
        ))
    })?;
    if let Err(mismatch) = verified {
        return Err(sidecar_corrupt(generation_dir, &mismatch.to_string()));
    }
    Ok(manifest)
}
