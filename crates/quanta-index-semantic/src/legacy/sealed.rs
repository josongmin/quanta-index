//! One canonical sealed-manifest data type/tuple/decoder; no IO or engine.
use crate::codec::format_unsupported;
use quanta_index_core::{CoreError, SealedArtifactCommitmentV1};
use std::path::Path;
pub(crate) const SEALED_MANIFEST_FORMAT_VERSION: u32 = 1;

/// What a sealed semantic generation promises a query can open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SemanticSealedManifestV1 {
    pub(crate) format_version: u32,
    pub(crate) manifest_digest: String,
    /// Length and SHA-256 of `semantic-manifest.cbor`.
    pub(crate) scope_manifest: (u64, [u8; 32]),
    /// Length and SHA-256 of `semantic-build-contract.cbor`.
    pub(crate) build_contract: (u64, [u8; 32]),
    /// Every file under `dataset/`, in path order.
    pub(crate) artifacts: Vec<SealedArtifactCommitmentV1>,
}

/// Wire shape: a fixed-order CBOR array so the encoding is auditable without
/// a derive.
pub(crate) type SealedManifestRowV1 = (
    u32,
    String,
    (u64, [u8; 32]),
    (u64, [u8; 32]),
    Vec<(String, u64, [u8; 32])>,
);

impl SemanticSealedManifestV1 {
    #[cfg(any(test, feature = "legacy-semantic-adapter"))]
    pub(crate) fn to_row(&self) -> SealedManifestRowV1 {
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
    #[cfg(any(test, feature = "legacy-semantic-adapter"))]
    pub(crate) fn dataset_bytes(&self) -> u64 {
        self.artifacts.iter().fold(0_u64, |total, artifact| {
            total.saturating_add(artifact.bytes)
        })
    }

    /// Committed dataset files.
    #[cfg(any(test, feature = "legacy-semantic-adapter"))]
    pub(crate) fn dataset_files(&self) -> u64 {
        quanta_index_core::count_from_usize(self.artifacts.len())
    }
}

pub(crate) fn decode_sealed_manifest_bytes(
    generation_dir: &Path,
    bytes: &[u8],
) -> Result<SemanticSealedManifestV1, CoreError> {
    super::limits::ensure_bounded(
        bytes,
        super::limits::MAX_SEALED_MANIFEST_BYTES,
        "sealed manifest",
    )?;
    let mut input = std::io::Cursor::new(bytes);
    let row: SealedManifestRowV1 = ciborium::from_reader(&mut input).map_err(|error| {
        sidecar_corrupt(
            generation_dir,
            &format!("sealed manifest cannot be decoded: {error}"),
        )
    })?;
    if usize::try_from(input.position()) != Ok(bytes.len()) {
        return Err(sidecar_corrupt(
            generation_dir,
            "sealed manifest contains trailing bytes",
        ));
    }
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

pub(crate) fn sidecar_corrupt(generation_dir: &Path, detail: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        message: format!(
            "semantic: sealed generation {} does not match its manifest: {detail}",
            generation_dir.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::SearchPlaneErrorCodeV2;

    // Fixed current owner row: [1,"d",[0,[0;32]],[0,[0;32]],[]]. This
    // literal does not use the candidate's encoder and proves no real dataset.
    fn fixed_row() -> Vec<u8> {
        let mut bytes = vec![0x85, 0x01, 0x61, b'd', 0x82, 0x00, 0x98, 0x20];
        bytes.extend([0_u8; 32]);
        bytes.extend([0x82, 0x00, 0x98, 0x20]);
        bytes.extend([0_u8; 32]);
        bytes.push(0x80);
        bytes
    }

    // Captured with the pre-retirement Ciborium fork from Git tree
    // 74cd6a99c3eccf1372da9ff77e8a69b2d940411b. This is a codec fixture,
    // not a recovered physical generation.
    const LEGACY_VENDOR_POPULATED_SEALED_ROW: [u8; 200] = [
        0x85, 0x01, 0x78, 0x40, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61,
        0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61,
        0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61,
        0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61,
        0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x82, 0x18, 0x27, 0x98, 0x20, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x82, 0x18, 0x33, 0x98, 0x20, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02,
        0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02,
        0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x81, 0x83, 0x72, 0x64, 0x61, 0x74, 0x61, 0x73,
        0x65, 0x74, 0x2f, 0x30, 0x30, 0x30, 0x31, 0x2e, 0x6c, 0x61, 0x6e, 0x63, 0x65, 0x19, 0x10,
        0x00, 0x98, 0x20, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03,
        0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03,
        0x03, 0x03, 0x03, 0x03, 0x03,
    ];

    #[test]
    fn legacy_vendor_populated_sealed_row_reads_and_preserves_wire() -> Result<(), CoreError> {
        let old_bytes = LEGACY_VENDOR_POPULATED_SEALED_ROW;
        let value = decode_sealed_manifest_bytes(Path::new("fixture"), &old_bytes)?;
        assert_eq!(value.format_version, 1);
        assert_eq!(value.manifest_digest, "a".repeat(64));
        assert_eq!(value.scope_manifest, (39, [1; 32]));
        assert_eq!(value.build_contract, (51, [2; 32]));
        assert_eq!(value.artifacts.len(), 1);
        assert_eq!(value.artifacts[0].name, "dataset/0001.lance");
        assert_eq!(value.artifacts[0].bytes, 4096);
        assert_eq!(value.artifacts[0].sha256, [3; 32]);

        let mut current_bytes = Vec::new();
        ciborium::into_writer(&value.to_row(), &mut current_bytes)
            .expect("current writer must encode the old sealed row");
        assert_eq!(current_bytes, old_bytes);

        let mut foreign = old_bytes;
        foreign[1] = 2;
        assert!(matches!(
            decode_sealed_manifest_bytes(Path::new("fixture"), &foreign),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                ..
            })
        ));
        let mut trailing = old_bytes.to_vec();
        trailing.push(0);
        assert!(matches!(
            decode_sealed_manifest_bytes(Path::new("fixture"), &trailing),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn legacy_vendor_sealed_row_opens_via_product_control_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let generation = directory.path().join("generation-v1-fixture/g1");
        std::fs::create_dir_all(&generation)?;
        let seal_path = crate::sealed_manifest::sealed_manifest_path(&generation);
        std::fs::write(&seal_path, LEGACY_VENDOR_POPULATED_SEALED_ROW)?;

        crate::sealed_manifest::inspect_sealed_manifest_identity(
            &generation,
            &"a".repeat(64),
            None,
        )?;

        std::fs::write(&seal_path, [0xff])?;
        assert!(matches!(
            crate::sealed_manifest::inspect_sealed_manifest_identity(
                &generation,
                &"a".repeat(64),
                None,
            ),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn fixed_sealed_row_preserves_owner_shape_without_io() -> Result<(), CoreError> {
        let value = decode_sealed_manifest_bytes(Path::new("fixture"), &fixed_row())?;
        assert_eq!(value.format_version, 1);
        assert_eq!(value.manifest_digest, "d");
        assert_eq!(value.scope_manifest, (0, [0; 32]));
        assert_eq!(value.build_contract, (0, [0; 32]));
        assert!(value.artifacts.is_empty());
        Ok(())
    }

    #[test]
    fn fixed_sealed_row_refuses_foreign_format_and_trailing_bytes() -> Result<(), CoreError> {
        let mut foreign = fixed_row();
        *foreign
            .get_mut(1)
            .ok_or_else(|| CoreError::InvalidContract("fixed sealed row lacks version".into()))? =
            2;
        assert!(matches!(
            decode_sealed_manifest_bytes(Path::new("fixture"), &foreign),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                ..
            })
        ));
        let mut trailing = fixed_row();
        trailing.push(0);
        assert!(matches!(
            decode_sealed_manifest_bytes(Path::new("fixture"), &trailing),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
        Ok(())
    }
}
