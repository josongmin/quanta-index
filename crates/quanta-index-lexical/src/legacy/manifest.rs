//! The ONE canonical sealed lexical manifest decoder and DATA shape.
use std::path::Path;

use quanta_index_core::CoreError;
use quanta_index_core::domains::generation::SealedArtifactCommitmentV1;

use super::names::SOURCE_FILE_COVERAGE_FILE_NAME;
use super::names::live_bm25;
use super::overlay_names::OverlayFamily;
use crate::channel_payloads::{
    BoundedCborRows, ExactManifestRow, ValueCompatible, leading_cbor_array_version,
    preflight_manifest_shape,
};
use crate::normalize::{TEXT_NORMALIZER_VERSION, TextNormalizerVersion};

/// File name of the sealed manifest.
pub(crate) const LEXICAL_SEALED_MANIFEST_FILE_NAME: &str = "search-corpus-generation-manifest.cbor";
/// Format-9 admission for the encoded artifact inventory. Publishers and
/// readers enforce the same limit, so a sealed generation stays readable.
pub(crate) const MAX_SEALED_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
// A commitment row needs at least a three-item array header, a text header,
// one integer, and a 32-byte digest (encoded in at least 34 bytes). Dividing
// by 37 cannot exclude any row that fits the existing encoded-byte ceiling.
pub(crate) const MAX_COMMITMENT_ROWS: usize = MAX_SEALED_MANIFEST_BYTES.div_euclid(37);
/// The manifest format this build writes and serves; see the module
/// documentation for what each earlier format lacked.
pub(crate) use crate::lexical_root_names::LEXICAL_SEALED_MANIFEST_FORMAT_VERSION;
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
    /// Exact live BM25 statistics bound to this index commit.
    pub(crate) live_bm25: SealedArtifactCommitmentV1,
    /// The `text-authority/` tree by `/`-joined path, ascending by name, or
    /// `None` for a generation built without a text authority.
    pub(crate) text_authority: Option<Vec<SealedArtifactCommitmentV1>>,
    /// The compact File authority root; it commits pooled source/posting pages.
    pub(crate) file_authority: Vec<SealedArtifactCommitmentV1>,
    /// Every overlay family present, in [`OverlayFamily::ALL`] order.
    pub(crate) overlays: Vec<SealedArtifactCommitmentV1>,
    /// None is unavailable coverage, never a complete empty universe.
    pub(crate) source_coverage: Option<SealedArtifactCommitmentV1>,
}

/// One commitment on the wire.
pub(crate) type CommitmentRow = (String, u64, [u8; 32]);

/// Wire shape of the manifest: a fixed-order CBOR array so the encoding is
/// auditable without a derive. Element 0 is the format version, which is
/// read on its own before the rest of the row is decoded.

// Only the reader uses bounded collections. The writer keeps the exact
// format-16 tuple above; both tuple layouts serialize identically.
type DecodedSealedManifestRow = (
    u32,
    ValueCompatible<String>,
    ValueCompatible<(u16, u16)>,
    ValueCompatible<CommitmentRow>,
    ValueCompatible<u8>,
    BoundedCborRows<ValueCompatible<CommitmentRow>, MAX_COMMITMENT_ROWS>,
    BoundedCborRows<ValueCompatible<CommitmentRow>, MAX_COMMITMENT_ROWS>,
    ValueCompatible<CommitmentRow>,
    Option<BoundedCborRows<ValueCompatible<CommitmentRow>, MAX_COMMITMENT_ROWS>>,
    BoundedCborRows<ValueCompatible<CommitmentRow>, MAX_COMMITMENT_ROWS>,
    BoundedCborRows<ValueCompatible<CommitmentRow>, MAX_COMMITMENT_ROWS>,
    Option<ValueCompatible<CommitmentRow>>,
);

fn from_commitment_row((name, bytes, sha256): CommitmentRow) -> SealedArtifactCommitmentV1 {
    SealedArtifactCommitmentV1 {
        name,
        bytes,
        sha256,
    }
}

fn from_compatible_commitment(row: ValueCompatible<CommitmentRow>) -> SealedArtifactCommitmentV1 {
    from_commitment_row(row.0)
}

pub(crate) fn manifest_corrupt(path: &Path, reason: &str) -> CoreError {
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
    /// Decode and validate a manifest read from `path`, refusing typed any
    /// format or normalizer this build does not serve and any structural
    /// violation of the sections.
    pub(crate) fn decode(bytes: &[u8], path: &Path) -> Result<Self, CoreError> {
        let format_version =
            leading_cbor_array_version(bytes).map_err(|detail| manifest_corrupt(path, detail))?;
        if format_version == LEXICAL_SEALED_MANIFEST_WHOLE_CORPUS_TEXT_AUTHORITY_VERSION {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationTextAuthorityFormatUnsupported,
                message: format!(
                    "lexical: sealed generation manifest {} has format {format_version}, the whole-corpus text-authority layout; this build serves format {LEXICAL_SEALED_MANIFEST_FORMAT_VERSION} with shared paged authorities, and the generation must be rebuilt",
                    path.display()
                ),
            });
        }
        if !matches!(
            format_version,
            LEXICAL_SEALED_MANIFEST_FORMAT_VERSION
                | crate::lexical_root_names::LEXICAL_UNPACKED_BODY_MANIFEST_FORMAT_VERSION
        ) {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                message: format!(
                    "lexical: sealed generation manifest {} has format {format_version} (this build serves {LEXICAL_SEALED_MANIFEST_FORMAT_VERSION}: shared paged lexical authorities); the generation must be rebuilt",
                    path.display()
                ),
            });
        }
        preflight_manifest_shape(bytes, 12, &[5, 6, 8, 9, 10], MAX_COMMITMENT_ROWS, true)
            .map_err(|detail| manifest_corrupt(path, detail))?;
        let (
            _format,
            manifest_digest,
            normalizer_version,
            index_meta,
            segment_verification,
            index_segments,
            ranked_keys,
            live_bm25,
            text_authority,
            file_authority,
            overlays,
            source_coverage,
        ) = crate::channel_payloads::decode_cbor_exact::<
            ExactManifestRow<DecodedSealedManifestRow>,
        >(bytes)
        .map_err(|error| manifest_corrupt(path, &format!("decode row: {error}")))?
        .0;
        let (major, minor) = normalizer_version.0;
        let normalizer = TextNormalizerVersion { major, minor };
        if normalizer != TEXT_NORMALIZER_VERSION {
            return Err(super::errors::normalizer_unsupported(path, normalizer));
        }
        let Some(index_segment_verification) =
            IndexSegmentVerificationV1::from_code(segment_verification.0)
        else {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                message: format!(
                    "lexical: sealed generation manifest {} stamps index-segment verification policy {}, which this build does not know",
                    path.display(),
                    segment_verification.0
                ),
            });
        };
        let manifest = Self {
            manifest_digest: manifest_digest.0,
            normalizer,
            index_meta: from_commitment_row(index_meta.0),
            index_segment_verification,
            index_segments: index_segments
                .0
                .into_iter()
                .map(from_compatible_commitment)
                .collect(),
            ranked_keys: ranked_keys
                .0
                .into_iter()
                .map(from_compatible_commitment)
                .collect(),
            live_bm25: from_commitment_row(live_bm25.0),
            text_authority: text_authority.map(|files| {
                files
                    .0
                    .into_iter()
                    .map(from_compatible_commitment)
                    .collect()
            }),
            file_authority: file_authority
                .0
                .into_iter()
                .map(from_compatible_commitment)
                .collect(),
            overlays: overlays
                .0
                .into_iter()
                .map(from_compatible_commitment)
                .collect(),
            source_coverage: source_coverage.map(from_compatible_commitment),
        };
        manifest.validate_shape(path)?;
        Ok(manifest)
    }

    fn validate_shape(&self, path: &Path) -> Result<(), CoreError> {
        if self.index_meta.name != super::names::TANTIVY_INDEX_META_FILE_NAME {
            return Err(manifest_corrupt(
                path,
                &format!(
                    "index meta names {}, not {}",
                    self.index_meta.name,
                    super::names::TANTIVY_INDEX_META_FILE_NAME
                ),
            ));
        }
        let top_level = |name: &str| !name.is_empty() && !name.contains('/');
        ensure_names(path, "index segments", &self.index_segments, |name| {
            top_level(name)
                && !super::names::is_ranked_key_file_name(name)
                && name != super::names::TANTIVY_INDEX_META_FILE_NAME
                && name != SOURCE_FILE_COVERAGE_FILE_NAME
                && OverlayFamily::from_file_name(name).is_none()
        })?;
        ensure_names(
            path,
            "ranked keys",
            &self.ranked_keys,
            super::names::is_ranked_key_file_name,
        )?;
        let max_live_bm25_bytes = u64::try_from(live_bm25::MAX_BYTES).map_err(|error| {
            manifest_corrupt(
                path,
                &format!("live BM25 control bound exceeds u64: {error}"),
            )
        })?;
        if self.live_bm25.name != live_bm25::FILE_NAME || self.live_bm25.bytes > max_live_bm25_bytes
        {
            return Err(manifest_corrupt(path, "invalid live BM25 commitment"));
        }
        let max_root_bytes =
            u64::try_from(crate::lexical_root_names::MAX_ROOT_BYTES).map_err(|error| {
                manifest_corrupt(path, &format!("authority root bound exceeds u64: {error}"))
            })?;
        let native_name = crate::lexical_root_names::NATIVE_ROOT_NAME;
        if !self.text_authority.as_deref().is_some_and(
            |files| matches!(files, [root] if root.name == native_name && root.bytes <= max_root_bytes),
        ) {
            return Err(manifest_corrupt(
                path,
                "Native authority requires exactly one bounded root commitment",
            ));
        }
        if !matches!(self.file_authority.as_slice(), [root]
            if root.name == crate::lexical_root_names::FILE_ROOT_NAME && root.bytes <= max_root_bytes)
        {
            return Err(manifest_corrupt(
                path,
                "File authority requires exactly one bounded root commitment",
            ));
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
            .chain(std::iter::once(&self.live_bm25))
            .chain(self.text_authority.iter().flatten())
            .chain(self.file_authority.iter())
            .chain(self.overlays.iter())
            .chain(self.source_coverage.iter())
    }
}
