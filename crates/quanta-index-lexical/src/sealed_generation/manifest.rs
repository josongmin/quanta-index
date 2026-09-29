//! The sealed manifest: what a sealed lexical generation promises a query
//! can open, file by file.
//!
//! Written after every file it lists is durable and before the sealed
//! identity, so the identity's presence implies the manifest's. It carries
//! the identity's `manifest_digest` so the two files bind each other, the
//! text normalizer the generation was built under, and six sections:
//!
//! - **index meta** — the Tantivy commit (`meta.json`), hashed at every
//!   door: it is the index's identity and names every segment file;
//! - **index segments** — every segment component file that commit
//!   references, by length and SHA-256. Content is proved when the seal
//!   measures it (or inherits the proof from the base generation whose
//!   hard-linked inode it is) and by every scrub; a door proves presence
//!   and length only, because a query maps these files instead of decoding
//!   them and hashing a corpus-sized index at every cold open is the cost
//!   QI-BB-017 removes. The section stamps that policy explicitly;
//! - **ranked keys** — one immutable, digest-proved key table per segment;
//!   a query ranks by these tables without decoding `SSTable` strings;
//! - **text authority** — `None` for a generation built without one, or the
//!   `text-authority/` manifest and every shard it lists. A door reads each
//!   file once, hashing it as it decodes it;
//! - **overlays** — every repo-metadata overlay family the generation
//!   carries, hashed as a door decodes it. A family not listed is one the
//!   generation does not carry; a file that appears anyway is refused;
//! - **source coverage** — an optional commitment to the admitted file
//!   universe and producer event, both decoded from the same proved bytes.
//!   Absence means unavailable capability, not complete coverage.
//!
//! Format 9 binds bounded immutable coverage pages through a generation root.
//! Format 8 added ranked-key tables; format 7 added flat source-file coverage.
//! Formats 8 and earlier require an explicit rebuild for the current layout.
//! The index's text documents carry their
//! text-authority doc id indexed and as a fast column, so a derived match
//! set restricts a query as one bitmap (QI-BB-024), and whose documents
//! carry the ranked page order's columns — candidate id, path, start and
//! end line — as fast columns, so pages are ranked, cut and grouped
//! without reading stored documents (QI-BB-005). Formats 1–5 described
//! earlier layouts (no normalizer stamp; the whole-corpus text authority;
//! the sharded text authority without index-segment or overlay
//! commitments; the doc id stored only; the order columns stored only) and
//! are refused typed: the migration is a rebuild, never a
//! reinterpretation.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use ciborium::Value as CborValue;
use quanta_index_core::CoreError;
use quanta_index_core::domains::generation::SealedArtifactCommitmentV1;

use crate::normalize::{TEXT_NORMALIZER_VERSION, TextNormalizerVersion};
use crate::overlay_codec::OverlayFamily;
use crate::sealed_generation::coverage::SOURCE_FILE_COVERAGE_FILE_NAME;
use crate::text_authority::{TEXT_AUTHORITY_DIR_NAME, leading_format_version};

/// File name of the sealed manifest.
pub(crate) const LEXICAL_SEALED_MANIFEST_FILE_NAME: &str = "search-corpus-generation-manifest.cbor";
/// The manifest format this build writes and serves; see the module
/// documentation for what each earlier format lacked.
pub(crate) const LEXICAL_SEALED_MANIFEST_FORMAT_VERSION: u32 = 9;
/// The format-2 layout: whole-corpus text-authority sidecars beside the
/// index, no doc ids in the index. Refused by that name so the operator
/// learns why a rebuild is needed.
pub(crate) const LEXICAL_SEALED_MANIFEST_WHOLE_CORPUS_TEXT_AUTHORITY_VERSION: u32 = 2;
/// How a door proves the index segment files, stamped into the manifest so
/// the policy the seal committed to is readable from the bytes alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IndexSegmentVerificationV1 {
    /// Content is hashed by the seal and by every scrub; a door checks
    /// presence and length.
    LengthAtOpenContentAtSealAndScrub,
}

impl IndexSegmentVerificationV1 {
    const fn code(self) -> u8 {
        match self {
            Self::LengthAtOpenContentAtSealAndScrub => 1,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::LengthAtOpenContentAtSealAndScrub),
            _ => None,
        }
    }
}

/// What a sealed lexical generation promises a query can open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LexicalSealedManifest {
    /// The identity's digest; the identity and the manifest bind each other.
    pub(crate) manifest_digest: String,
    /// The text normalizer the index and the shards were built under.
    pub(crate) normalizer: TextNormalizerVersion,
    /// The Tantivy commit, `meta.json`.
    pub(crate) index_meta: SealedArtifactCommitmentV1,
    /// How a door proves `index_segments`.
    pub(crate) index_segment_verification: IndexSegmentVerificationV1,
    /// Every segment component file the commit references, ascending by
    /// name.
    pub(crate) index_segments: Vec<SealedArtifactCommitmentV1>,
    /// One immutable ranked-key table per committed index segment.
    pub(crate) ranked_keys: Vec<SealedArtifactCommitmentV1>,
    /// The `text-authority/` tree by `/`-joined path, ascending by name, or
    /// `None` for a generation built without a text authority.
    pub(crate) text_authority: Option<Vec<SealedArtifactCommitmentV1>>,
    /// Every overlay family present, in [`OverlayFamily::ALL`] order.
    pub(crate) overlays: Vec<SealedArtifactCommitmentV1>,
    /// None is unavailable coverage, never a complete empty universe.
    pub(crate) source_coverage: Option<SealedArtifactCommitmentV1>,
}

/// One commitment on the wire.
type CommitmentRow = (String, u64, [u8; 32]);

/// Wire shape of the manifest: a fixed-order CBOR array so the encoding is
/// auditable without a derive. Element 0 is the format version, which is
/// read on its own before the rest of the row is decoded.
type SealedManifestRow = (
    u32,
    String,
    (u16, u16),
    CommitmentRow,
    u8,
    Vec<CommitmentRow>,
    Vec<CommitmentRow>,
    Option<Vec<CommitmentRow>>,
    Vec<CommitmentRow>,
    Option<CommitmentRow>,
);

fn to_commitment_row(artifact: &SealedArtifactCommitmentV1) -> CommitmentRow {
    (artifact.name.clone(), artifact.bytes, artifact.sha256)
}

fn from_commitment_row((name, bytes, sha256): CommitmentRow) -> SealedArtifactCommitmentV1 {
    SealedArtifactCommitmentV1 {
        name,
        bytes,
        sha256,
    }
}

pub(crate) fn manifest_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(LEXICAL_SEALED_MANIFEST_FILE_NAME)
}

fn manifest_corrupt(path: &Path, reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!(
            "lexical: sealed generation manifest {} is structurally invalid: {reason}",
            path.display()
        ),
    }
}

/// Every name ascending and unique, and each acceptable to `accept`.
fn ensure_names(
    path: &Path,
    section: &str,
    artifacts: &[SealedArtifactCommitmentV1],
    accept: impl Fn(&str) -> bool,
) -> Result<(), CoreError> {
    for (position, artifact) in artifacts.iter().enumerate() {
        if !accept(&artifact.name) {
            return Err(manifest_corrupt(
                path,
                &format!(
                    "{section} lists {}, which is not a {section} file",
                    artifact.name
                ),
            ));
        }
        if let Some(previous) = position
            .checked_sub(1)
            .and_then(|index| artifacts.get(index))
            && previous.name >= artifact.name
        {
            return Err(manifest_corrupt(
                path,
                &format!(
                    "{section} lists {} after {}; entries must be strictly ascending",
                    artifact.name, previous.name
                ),
            ));
        }
    }
    Ok(())
}

impl LexicalSealedManifest {
    /// Encode as the fixed-order row this build's decoder accepts.
    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        let row: SealedManifestRow = (
            LEXICAL_SEALED_MANIFEST_FORMAT_VERSION,
            self.manifest_digest.clone(),
            (self.normalizer.major, self.normalizer.minor),
            to_commitment_row(&self.index_meta),
            self.index_segment_verification.code(),
            self.index_segments.iter().map(to_commitment_row).collect(),
            self.ranked_keys.iter().map(to_commitment_row).collect(),
            self.text_authority
                .as_ref()
                .map(|files| files.iter().map(to_commitment_row).collect()),
            self.overlays.iter().map(to_commitment_row).collect(),
            self.source_coverage.as_ref().map(to_commitment_row),
        );
        crate::channel_payloads::encode_cbor(&row, "sealed generation manifest")
    }

    /// Decode and validate a manifest read from `path`, refusing typed any
    /// format or normalizer this build does not serve and any structural
    /// violation of the sections.
    pub(crate) fn decode(bytes: &[u8], path: &Path) -> Result<Self, CoreError> {
        let value: CborValue = ciborium::from_reader(bytes).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: decode sealed generation manifest {}: {error}",
                path.display()
            ))
        })?;
        let format_version = leading_format_version(&value, "sealed generation manifest", path)?;
        if format_version == LEXICAL_SEALED_MANIFEST_WHOLE_CORPUS_TEXT_AUTHORITY_VERSION {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationTextAuthorityFormatUnsupported,
                message: format!(
                    "lexical: sealed generation manifest {} has format {format_version}, the whole-corpus text-authority layout; this build serves format {LEXICAL_SEALED_MANIFEST_FORMAT_VERSION} with the sharded text authority, and the generation must be rebuilt",
                    path.display()
                ),
            });
        }
        if format_version != LEXICAL_SEALED_MANIFEST_FORMAT_VERSION {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                message: format!(
                    "lexical: sealed generation manifest {} has format {format_version} (this build serves {LEXICAL_SEALED_MANIFEST_FORMAT_VERSION}: ranked keys and bounded committed coverage pages); the generation must be rebuilt",
                    path.display()
                ),
            });
        }
        let (
            _format,
            manifest_digest,
            (major, minor),
            index_meta,
            segment_verification,
            index_segments,
            ranked_keys,
            text_authority,
            overlays,
            source_coverage,
        ): SealedManifestRow = value.deserialized().map_err(|error| {
            CoreError::Storage(format!(
                "lexical: decode sealed generation manifest {}: {error}",
                path.display()
            ))
        })?;
        let normalizer = TextNormalizerVersion { major, minor };
        if normalizer != TEXT_NORMALIZER_VERSION {
            return Err(crate::index_store::normalizer_unsupported(path, normalizer));
        }
        let Some(index_segment_verification) =
            IndexSegmentVerificationV1::from_code(segment_verification)
        else {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                message: format!(
                    "lexical: sealed generation manifest {} stamps index-segment verification policy {segment_verification}, which this build does not know",
                    path.display()
                ),
            });
        };
        let manifest = Self {
            manifest_digest,
            normalizer,
            index_meta: from_commitment_row(index_meta),
            index_segment_verification,
            index_segments: index_segments
                .into_iter()
                .map(from_commitment_row)
                .collect(),
            ranked_keys: ranked_keys.into_iter().map(from_commitment_row).collect(),
            text_authority: text_authority
                .map(|files| files.into_iter().map(from_commitment_row).collect()),
            overlays: overlays.into_iter().map(from_commitment_row).collect(),
            source_coverage: source_coverage.map(from_commitment_row),
        };
        manifest.validate_shape(path)?;
        Ok(manifest)
    }

    fn validate_shape(&self, path: &Path) -> Result<(), CoreError> {
        if self.index_meta.name != crate::TANTIVY_INDEX_META_FILE_NAME {
            return Err(manifest_corrupt(
                path,
                &format!(
                    "index meta names {}, not {}",
                    self.index_meta.name,
                    crate::TANTIVY_INDEX_META_FILE_NAME
                ),
            ));
        }
        let top_level = |name: &str| !name.is_empty() && !name.contains('/');
        ensure_names(path, "index segments", &self.index_segments, |name| {
            top_level(name)
                && !crate::ranked_keys::is_file_name(name)
                && name != crate::TANTIVY_INDEX_META_FILE_NAME
                && name != SOURCE_FILE_COVERAGE_FILE_NAME
                && OverlayFamily::from_file_name(name).is_none()
        })?;
        ensure_names(
            path,
            "ranked keys",
            &self.ranked_keys,
            crate::ranked_keys::is_file_name,
        )?;
        if let Some(files) = &self.text_authority {
            let prefix = format!("{TEXT_AUTHORITY_DIR_NAME}/");
            ensure_names(path, "text authority", files, |name| {
                name.strip_prefix(prefix.as_str()).is_some_and(&top_level)
            })?;
            let manifest_name = format!(
                "{TEXT_AUTHORITY_DIR_NAME}/{}",
                crate::text_authority::TEXT_AUTHORITY_MANIFEST_FILE_NAME
            );
            if !files.iter().any(|file| file.name == manifest_name) {
                return Err(manifest_corrupt(
                    path,
                    "text authority is committed without its manifest",
                ));
            }
        }
        if let Some(coverage) = &self.source_coverage
            && coverage.name != SOURCE_FILE_COVERAGE_FILE_NAME
        {
            return Err(manifest_corrupt(
                path,
                "source coverage names another artifact",
            ));
        }
        let mut last_family: Option<OverlayFamily> = None;
        for overlay in &self.overlays {
            let Some(family) = OverlayFamily::from_file_name(&overlay.name) else {
                return Err(manifest_corrupt(
                    path,
                    &format!(
                        "overlays list {}, which is not an overlay family",
                        overlay.name
                    ),
                ));
            };
            if last_family.is_some_and(|previous| previous >= family) {
                return Err(manifest_corrupt(
                    path,
                    &format!(
                        "overlays list {} out of family order or twice",
                        overlay.name
                    ),
                ));
            }
            last_family = Some(family);
        }
        Ok(())
    }

    /// The overlay families this generation carries, with their
    /// commitments, in family order.
    pub(crate) fn overlay_commitments(
        &self,
    ) -> impl Iterator<Item = (OverlayFamily, &SealedArtifactCommitmentV1)> + '_ {
        self.overlays.iter().filter_map(|artifact| {
            OverlayFamily::from_file_name(&artifact.name).map(|family| (family, artifact))
        })
    }

    /// The listed commitment for overlay `family`, if the generation
    /// carries it.
    pub(crate) fn overlay(&self, family: OverlayFamily) -> Option<&SealedArtifactCommitmentV1> {
        self.overlays
            .iter()
            .find(|artifact| artifact.name == family.file_name())
    }

    /// Every commitment the manifest holds, in section order.
    pub(crate) fn all_commitments(&self) -> impl Iterator<Item = &SealedArtifactCommitmentV1> {
        std::iter::once(&self.index_meta)
            .chain(self.index_segments.iter())
            .chain(self.ranked_keys.iter())
            .chain(self.text_authority.iter().flatten())
            .chain(self.overlays.iter())
            .chain(self.source_coverage.iter())
    }
}

/// Read a sealed manifest, refusing typed a missing one and any format or
/// normalizer this build does not serve.
pub(crate) fn read_manifest(generation_dir: &Path) -> Result<LexicalSealedManifest, CoreError> {
    let path = manifest_path(generation_dir);
    let mut file = super::open_regular_nofollow(
        generation_dir,
        Path::new(LEXICAL_SEALED_MANIFEST_FILE_NAME),
    )
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestMissing,
                message: format!(
                    "lexical: sealed generation has no content manifest at {}; it predates the sealed-manifest format and requires explicit migration",
                    path.display()
                ),
            }
        } else if super::is_unsafe_artifact_path(&error) {
            manifest_corrupt(&path, "manifest is not a regular file")
        } else {
            CoreError::Storage(format!(
                "lexical: read sealed generation manifest {}: {error}",
                path.display()
            ))
        }
    })?;
    let mut bytes = Vec::new();
    let _read = file.read_to_end(&mut bytes).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: read sealed generation manifest {}: {error}",
            path.display()
        ))
    })?;
    LexicalSealedManifest::decode(&bytes, &path)
}

/// Read a sealed manifest and prove it was sealed for `manifest_digest`.
pub(crate) fn read_bound_manifest(
    generation_dir: &Path,
    manifest_digest: &str,
) -> Result<LexicalSealedManifest, CoreError> {
    let manifest = read_manifest(generation_dir)?;
    if manifest.manifest_digest != manifest_digest {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
            message: format!(
                "lexical: sealed manifest under {} was written for digest {} but the identity says {manifest_digest}",
                generation_dir.display(),
                manifest.manifest_digest
            ),
        });
    }
    Ok(manifest)
}

/// Write the manifest durably; the seal calls this after every listed file
/// is durable and before it writes the identity.
pub(crate) fn write_manifest(
    generation_dir: &Path,
    manifest: &LexicalSealedManifest,
) -> Result<(), CoreError> {
    let bytes = manifest.encode()?;
    crate::index_store::write_atomic_durable(
        &manifest_path(generation_dir),
        &bytes,
        "sealed generation manifest",
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use quanta_index_core::CoreError;
    use quanta_index_core::domains::generation::SealedArtifactCommitmentV1;

    use super::{
        IndexSegmentVerificationV1, LEXICAL_SEALED_MANIFEST_FORMAT_VERSION, LexicalSealedManifest,
        SealedManifestRow,
    };
    use crate::normalize::TEXT_NORMALIZER_VERSION;

    fn artifact(name: &str) -> SealedArtifactCommitmentV1 {
        SealedArtifactCommitmentV1 {
            name: name.to_string(),
            bytes: 3,
            sha256: [7; 32],
        }
    }

    fn manifest() -> LexicalSealedManifest {
        LexicalSealedManifest {
            manifest_digest: "digest".to_string(),
            normalizer: TEXT_NORMALIZER_VERSION,
            index_meta: artifact("meta.json"),
            index_segment_verification:
                IndexSegmentVerificationV1::LengthAtOpenContentAtSealAndScrub,
            index_segments: vec![artifact("aa.idx"), artifact("aa.term")],
            ranked_keys: vec![artifact("ranked-keys-00000000000000000000000000000000.bin")],
            text_authority: Some(vec![
                artifact("text-authority/manifest.cbor"),
                artifact("text-authority/shard-00000000-0000000000000000.cbor"),
            ]),
            overlays: vec![artifact("repo-metadata.cbor"), artifact("repo-meta.cbor")],
            source_coverage: None,
        }
    }

    fn typed_code(
        result: &Result<LexicalSealedManifest, CoreError>,
    ) -> Option<quanta_index_contract::SearchPlaneErrorCodeV2> {
        match result {
            Err(CoreError::Typed { code, .. }) => Some(*code),
            _ => None,
        }
    }

    #[test]
    fn a_manifest_round_trips() {
        let manifest = manifest();
        let bytes = manifest.encode().expect("encode");
        let decoded = LexicalSealedManifest::decode(&bytes, Path::new("/g1/m")).expect("decode");
        assert_eq!(decoded, manifest);
        assert_eq!(decoded.all_commitments().count(), 8);
    }

    #[test]
    fn sealed_manifest_does_not_follow_a_redirect_to_valid_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let generation = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let bytes = manifest().encode()?;
        let outside_file = outside.path().join("valid-manifest.cbor");
        std::fs::write(&outside_file, bytes)?;
        std::os::unix::fs::symlink(
            outside_file,
            generation
                .path()
                .join(super::LEXICAL_SEALED_MANIFEST_FILE_NAME),
        )?;
        let result = super::read_manifest(generation.path());
        if typed_code(&result)
            != Some(quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt)
        {
            return Err(
                format!("redirected manifest open did not refuse typed: {result:?}").into(),
            );
        }
        Ok(())
    }

    #[test]
    fn structural_violations_are_typed_corruption() {
        let cases: Vec<(&str, LexicalSealedManifest)> = vec![
            (
                "index meta under another name",
                LexicalSealedManifest {
                    index_meta: artifact("meta.json.bak"),
                    ..manifest()
                },
            ),
            (
                "segments out of order",
                LexicalSealedManifest {
                    index_segments: vec![artifact("aa.term"), artifact("aa.idx")],
                    ..manifest()
                },
            ),
            (
                "an overlay listed as a segment",
                LexicalSealedManifest {
                    index_segments: vec![artifact("repo-meta.cbor")],
                    ..manifest()
                },
            ),
            (
                "a ranked-key file without a full segment identity",
                LexicalSealedManifest {
                    ranked_keys: vec![artifact("ranked-keys-short.bin")],
                    ..manifest()
                },
            ),
            (
                "a text-authority file outside its tree",
                LexicalSealedManifest {
                    text_authority: Some(vec![
                        artifact("shard.cbor"),
                        artifact("text-authority/manifest.cbor"),
                    ]),
                    ..manifest()
                },
            ),
            (
                "a text authority without its manifest",
                LexicalSealedManifest {
                    text_authority: Some(vec![artifact(
                        "text-authority/shard-00000000-0000000000000000.cbor",
                    )]),
                    ..manifest()
                },
            ),
            (
                "an unknown overlay family",
                LexicalSealedManifest {
                    overlays: vec![artifact("repo-extra.cbor")],
                    ..manifest()
                },
            ),
            (
                "overlays out of family order",
                LexicalSealedManifest {
                    overlays: vec![artifact("repo-meta.cbor"), artifact("repo-metadata.cbor")],
                    ..manifest()
                },
            ),
        ];
        for (label, manifest) in cases {
            let bytes = manifest.encode().expect("encode");
            assert_eq!(
                typed_code(&LexicalSealedManifest::decode(&bytes, Path::new("/g1/m"))),
                Some(quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE),
                "{label}"
            );
        }
    }

    /// Every format but the served one is refused by name.
    ///
    /// The earlier layouts (format 4: the doc id stored but not indexed;
    /// format 5: the page order stored but not a fast column) and a later
    /// one alike are never read under this build's layout.
    #[test]
    fn another_format_or_policy_is_refused_by_name() {
        for format in [1, 3, 4, 5, 6, 7, LEXICAL_SEALED_MANIFEST_FORMAT_VERSION + 1] {
            let other_format: SealedManifestRow = (
                format,
                "digest".to_string(),
                (TEXT_NORMALIZER_VERSION.major, TEXT_NORMALIZER_VERSION.minor),
                ("meta.json".to_string(), 1, [0; 32]),
                1,
                Vec::new(),
                Vec::new(),
                None,
                Vec::new(),
                None,
            );
            let bytes =
                crate::channel_payloads::encode_cbor(&other_format, "test").expect("encode");
            assert_eq!(
                typed_code(&LexicalSealedManifest::decode(&bytes, Path::new("/g1/m"))),
                Some(quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported),
                "format {format}"
            );
        }
        let other_policy: SealedManifestRow = (
            LEXICAL_SEALED_MANIFEST_FORMAT_VERSION,
            "digest".to_string(),
            (TEXT_NORMALIZER_VERSION.major, TEXT_NORMALIZER_VERSION.minor),
            ("meta.json".to_string(), 1, [0; 32]),
            9,
            Vec::new(),
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let bytes = crate::channel_payloads::encode_cbor(&other_policy, "test").expect("encode");
        assert_eq!(
            typed_code(&LexicalSealedManifest::decode(&bytes, Path::new("/g1/m"))),
            Some(
                quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported
            )
        );
    }
}
