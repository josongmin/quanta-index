//! The sealed generation manifest: what a sealed semantic generation promises
//! a query can open (QI-BB-017).
//!
//! The scope manifest (`semantic-manifest.cbor`) carries the row root and the
//! membership root. Content in a sealed generation never changes: every file
//! `LanceDB` writes is an immutable versioned object (G0-S). So the seal
//! commits to the files once — every file under `dataset/` plus the scope
//! manifest and the build contract, each with its length and SHA-256, bound
//! to the identity's `manifest_digest` — and the doors split the proof:
//!
//! - **Open** (activation, restart, a cold query open) is cheap: it hashes
//!   the two sidecars it decodes anyway and checks every dataset file for
//!   existence and length from directory metadata, never reading a dataset
//!   byte. Its cost is the number of files, not their bytes.
//! - **Scrub** ([`scrub_sealed_manifest`]) is the deep proof: it hashes
//!   every committed dataset file, bounded per step and resumable, off the
//!   serving path. A same-length rewrite is the scrub's to find.
//!
//! A delta seal inherits its base's immutable files by hard link and carries
//! the base's digest for every file that is still the base's inode, so the
//! bytes a seal hashes are proportional to what the generation added.
//!
//! The sealed manifest is written after the scope manifest and before the
//! sealed marker, so a sealed marker implies a sealed manifest.

use std::path::{Path, PathBuf};

use quanta_index_core::{
    CoreError, SealedArtifactCommitmentV1, TreeScrubVerdictV1, commit_tree_inheriting_v1,
    scrub_tree_commitment_v1, sha256_of_file, verify_tree_layout_v1,
};

use crate::layout::{self, BUILD_CONTRACT_FILE_NAME, DATASET_DIR_NAME, MANIFEST_FILE_NAME};
use crate::manifest::format_unsupported;

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
    /// Length and SHA-256 of `semantic-build-contract.cbor`.
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

    /// Committed dataset files.
    pub(crate) fn dataset_files(&self) -> u64 {
        quanta_index_core::count_from_usize(self.artifacts.len())
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

/// How much a seal read to commit its dataset tree (QI-BB-006 #4).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SealMeasurementV1 {
    /// Dataset bytes this seal hashed itself.
    pub(crate) hashed_bytes: u64,
    /// Dataset bytes whose digest was inherited from the base's seal.
    pub(crate) inherited_bytes: u64,
    /// Dataset files whose digest was inherited from the base's seal.
    pub(crate) inherited_files: u64,
}

/// The base generation a delta seal may inherit digests from: its dataset
/// directory and the commitment its own seal recorded.
struct InheritableBaseV1 {
    dataset_dir: PathBuf,
    artifacts: Vec<SealedArtifactCommitmentV1>,
}

/// The base's sealed commitment: the base must be sealed under a sealed
/// manifest its own marker vouches for, or the delta cannot be sealed on
/// it at all.
fn inheritable_base_v1(base_generation_dir: &Path) -> Result<InheritableBaseV1, CoreError> {
    let marker_path = layout::sealed_marker_path(base_generation_dir);
    let base_digest = std::fs::read_to_string(&marker_path).map_err(|error| {
        CoreError::Storage(format!(
            "semantic: read delta base sealed marker {}: {error}",
            marker_path.display()
        ))
    })?;
    let manifest = read_bound_sealed_manifest(base_generation_dir, &base_digest)?;
    Ok(InheritableBaseV1 {
        dataset_dir: layout::dataset_dir(base_generation_dir),
        artifacts: manifest.artifacts,
    })
}

/// Measure the promoted generation as sealed and return its manifest bytes
/// with what the measurement cost.
///
/// Runs after the dataset and the build contract are promoted and the scope
/// manifest is written, and before the sealed marker, so a crash in between
/// leaves an unsealed generation (no marker), never a sealed one without a
/// commitment. With `base_generation_dir`, every dataset file that is still
/// the base's inode carries the base's digest instead of being read.
pub(crate) fn build_sealed_manifest_bytes(
    generation_dir: &Path,
    manifest_digest: &str,
    base_generation_dir: Option<&Path>,
) -> Result<(Vec<u8>, SealMeasurementV1), CoreError> {
    let scope_manifest = measure(&layout::manifest_path(generation_dir), "scope manifest")?;
    let build_contract = measure(
        &layout::build_contract_path(generation_dir),
        "build contract",
    )?;
    let base = base_generation_dir.map(inheritable_base_v1).transpose()?;
    let commitment = commit_tree_inheriting_v1(
        &layout::dataset_dir(generation_dir),
        DATASET_DIR_NAME,
        base.as_ref()
            .map(|base| (base.dataset_dir.as_path(), base.artifacts.as_slice())),
    )
    .map_err(|error| {
        CoreError::Storage(format!(
            "semantic: commit dataset tree {} at seal: {error}",
            generation_dir.display()
        ))
    })?;
    if commitment.artifacts.is_empty() {
        return Err(CoreError::Storage(format!(
            "semantic: refusing to seal {} with an empty dataset tree",
            generation_dir.display()
        )));
    }
    let measurement = SealMeasurementV1 {
        hashed_bytes: commitment.hashed_bytes,
        inherited_bytes: commitment.inherited_bytes,
        inherited_files: commitment.inherited_files,
    };
    let manifest = SemanticSealedManifestV1 {
        format_version: SEALED_MANIFEST_FORMAT_VERSION,
        manifest_digest: manifest_digest.to_string(),
        scope_manifest,
        build_contract,
        artifacts: commitment.artifacts,
    };
    let mut bytes = Vec::new();
    ciborium::into_writer(&manifest.to_row(), &mut bytes).map_err(|error| {
        CoreError::Storage(format!(
            "semantic: encode sealed generation manifest: {error}"
        ))
    })?;
    Ok((bytes, measurement))
}

fn read_sealed_manifest(generation_dir: &Path) -> Result<SemanticSealedManifestV1, CoreError> {
    let path = sealed_manifest_path(generation_dir);
    let bytes = std::fs::read(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CoreError::Typed {
                code: "GENERATION_MANIFEST_MISSING".to_string(),
                message: format!(
                    "semantic: sealed generation has no content manifest at {}; it predates the sealed-manifest format and must be rebuilt from its producer",
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
        Err(format_unsupported(
            "sealed generation manifest",
            manifest.format_version,
            SEALED_MANIFEST_FORMAT_VERSION,
        ))
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

/// Read the sealed manifest and refuse one written for another identity.
fn read_bound_sealed_manifest(
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
    Ok(manifest)
}

/// Hash the two decoded sidecars against their commitments.
fn verify_sidecars(
    generation_dir: &Path,
    manifest: &SemanticSealedManifestV1,
) -> Result<(), CoreError> {
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
    Ok(())
}

/// The cheap door proof: what is on disk has the shape the seal committed
/// to, and the sidecars a door decodes are the sealed ones.
///
/// This is the check the activation validator and the cold open share, so
/// activation can only ack a generation a query can open. It hashes the
/// scope manifest and the build contract (both decoded right after) and
/// checks every dataset file for existence and length from directory
/// metadata; it reads no dataset byte. It refuses a missing, extra or
/// resized file and a manifest whose digest is not the identity's. Byte
/// integrity of the dataset is the scrub's ([`scrub_sealed_manifest`]).
pub(crate) fn verify_sealed_manifest(
    generation_dir: &Path,
    manifest_digest: &str,
) -> Result<SemanticSealedManifestV1, CoreError> {
    let manifest = read_bound_sealed_manifest(generation_dir, manifest_digest)?;
    verify_sidecars(generation_dir, &manifest)?;
    let verified = verify_tree_layout_v1(
        &layout::dataset_dir(generation_dir),
        DATASET_DIR_NAME,
        &manifest.artifacts,
    )
    .map_err(|error| {
        CoreError::Storage(format!(
            "semantic: measure dataset tree {}: {error}",
            generation_dir.display()
        ))
    })?;
    if let Err(mismatch) = verified {
        return Err(sidecar_corrupt(generation_dir, &mismatch.to_string()));
    }
    Ok(manifest)
}

/// What one bounded scrub step over a sealed generation found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SealedManifestScrubV1 {
    /// Committed files hashed and matched in this step.
    pub(crate) files_verified: u64,
    /// Bytes read in this step, a mismatching file's included.
    pub(crate) bytes_read: u64,
    pub(crate) verdict: SealedManifestScrubVerdictV1,
}

/// How one scrub step over a sealed generation ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SealedManifestScrubVerdictV1 {
    /// Every committed file has been hashed and matched; `manifest` is
    /// what was proven.
    Completed(SemanticSealedManifestV1),
    /// The byte budget ran out; resume at this committed index.
    Paused { next_artifact: u64 },
    /// A sidecar or dataset file does not match the seal.
    Corrupt { detail: String },
}

/// The deep proof: hash committed dataset files against the seal, from
/// `start_artifact`, reading at most `max_bytes` (plus the one file that
/// crosses the budget) — see [`scrub_tree_commitment_v1`].
///
/// The sidecars are re-hashed on every step (they are small and the step
/// would otherwise trust a rewritten manifest), and the layout is checked
/// before any byte is read so a missing or resized file is found first.
pub(crate) fn scrub_sealed_manifest(
    generation_dir: &Path,
    manifest_digest: &str,
    start_artifact: u64,
    max_bytes: u64,
) -> Result<SealedManifestScrubV1, CoreError> {
    let manifest = read_bound_sealed_manifest(generation_dir, manifest_digest)?;
    match verify_sidecars(generation_dir, &manifest) {
        Ok(()) => {}
        Err(CoreError::Typed { code, message }) if code == "GENERATION_SIDECAR_CORRUPT" => {
            return Ok(SealedManifestScrubV1 {
                files_verified: 0,
                bytes_read: 0,
                verdict: SealedManifestScrubVerdictV1::Corrupt { detail: message },
            });
        }
        Err(other) => return Err(other),
    }
    let step = scrub_tree_commitment_v1(
        &layout::dataset_dir(generation_dir),
        DATASET_DIR_NAME,
        &manifest.artifacts,
        start_artifact,
        max_bytes,
    )
    .map_err(|error| {
        CoreError::Storage(format!(
            "semantic: scrub dataset tree {}: {error}",
            generation_dir.display()
        ))
    })?;
    let verdict = match step.verdict {
        TreeScrubVerdictV1::Completed => SealedManifestScrubVerdictV1::Completed(manifest),
        TreeScrubVerdictV1::Paused { next_artifact } => {
            SealedManifestScrubVerdictV1::Paused { next_artifact }
        }
        TreeScrubVerdictV1::Mismatch(mismatch) => SealedManifestScrubVerdictV1::Corrupt {
            detail: sidecar_corrupt(generation_dir, &mismatch.to_string()).to_string(),
        },
    };
    Ok(SealedManifestScrubV1 {
        files_verified: step.files_verified,
        bytes_read: step.bytes_read,
        verdict,
    })
}
