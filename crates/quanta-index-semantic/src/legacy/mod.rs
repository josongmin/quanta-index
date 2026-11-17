//! Canonical persisted Legacy data borrowed by existing-only verification.
//!
//! Decoding creates no runtime, provider, IO, activation, Source or query
//! authority. Physical verification separately requires genuine root-bound
//! ports, Original admission and caller proof. Caller IO/allocation admission
//! must precede this pure operation.

#[cfg(unix)]
mod existing;
use crate::{generation_contract, manifest};
#[cfg(unix)]
pub use existing::verify_existing_semantic_generation_read_only;

pub mod layout;
pub mod limits;
pub(crate) mod sealed;

use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind, SemanticContentRootsV1};
use quanta_index_core::{CoreError, SealedArtifactCommitmentV1};
use sha2::{Digest as _, Sha256};
use std::path::Path;

/// Decode and bind existing marker/seal/sidecar DATA, then borrow its committed
/// file list and roots to the caller. This performs no physical verification:
/// a no-op callback cannot become a Source or activation permit. Original
/// budget, memory, safe paths, exact file set and admitted reads remain the
/// caller's required contract. No returned value is an opaque proof witness.
pub fn with_decoded_generation_data<T, E>(
    generation_dir: &Path,
    expected: &GenerationSnapshot,
    marker: &str,
    seal_bytes: &[u8],
    scope_bytes: &[u8],
    contract_bytes: &[u8],
    use_data: impl FnOnce(&[SealedArtifactCommitmentV1], SemanticContentRootsV1) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<CoreError>,
{
    if expected.track != SearchPlaneTrackKind::Semantic {
        return Err(
            CoreError::InvalidContract("semantic data received a foreign track".into()).into(),
        );
    }
    limits::ensure_bounded(
        marker.as_bytes(),
        limits::MAX_SEALED_MARKER_BYTES,
        "sealed marker",
    )?;
    limits::ensure_bounded(
        scope_bytes,
        limits::MAX_SCOPE_MANIFEST_BYTES,
        "scope manifest",
    )?;
    limits::ensure_bounded(
        contract_bytes,
        limits::MAX_GENERATION_CONTRACT_BYTES,
        "generation contract",
    )?;
    let seal = sealed::decode_sealed_manifest_bytes(generation_dir, seal_bytes)?;
    if marker != expected.manifest_digest || seal.manifest_digest != expected.manifest_digest {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
            message: "semantic marker/seal differs from the supplied identity".into(),
        }
        .into());
    }
    // Preserve the owner's integrity-before-interpretation order: scope and
    // build-contract bytes must match their seal before either is decoded.
    for (bytes, committed) in [
        (scope_bytes, seal.scope_manifest),
        (contract_bytes, seal.build_contract),
    ] {
        if u64::try_from(bytes.len()).ok() != Some(committed.0)
            || <[u8; 32]>::from(Sha256::digest(bytes)) != committed.1
        {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                message: "semantic sidecar bytes differ from the seal commitment".into(),
            }
            .into());
        }
    }
    let scope = manifest::SemanticManifest::decode(scope_bytes)?;
    let contract = generation_contract::GenerationContract::decode(contract_bytes)?;
    scope.validate_scope(
        &expected.repo_id,
        &expected.revision_id,
        expected.manifest_generation,
    )?;
    if scope.manifest_digest != expected.manifest_digest {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
            message: "semantic scope differs from the supplied identity".into(),
        }
        .into());
    }
    scope.validate_against(&contract)?;
    // validate_scope already checks normalization and corpus coverage. Keep
    // that single canonical pass, including its original refusal ordering.
    use_data(
        &seal.artifacts,
        SemanticContentRootsV1 {
            row_root_digest: scope.semantic_row_root_digest,
            membership_root_digest: scope.cluster_membership_root_digest,
        },
    )
}
