//! The text-authority manifest: a bounded index over a generation's shards.
//!
//! A generation's text authority lives under `text-authority/` inside the
//! generation directory: one `manifest.cbor` plus one immutable file per
//! shard. A shard is a fixed doc-id range of [`SHARD_DOCS`] documents, so
//! the shard a document belongs to is a pure function of its doc id and a
//! delta only has to rewrite the shards whose ranges it touched; every
//! other shard file is inherited from the base generation by hard link.
//!
//! The manifest is bounded by the number of shards, never by the number of
//! documents: it records the layout parameters and, per shard, the doc-id
//! extremes, the row count, the byte length and the SHA-256 of the shard
//! file. Shard files are named by index and content digest, so rewriting a
//! shard publishes a new file and the manifest rename is the atomic switch
//! between the old and the new shard set; a crash in between leaves the old
//! manifest consistent and an unowned file the next publish removes.
//!
//! Format version: version 1 is the sharded layout described here, with
//! the doc-table row shape, the `lq-trigram` byte-trigram posting map and
//! the `lq-positions` delta-varint posting encoding pinned as the embedded
//! shard body. Any change to those wire shapes is a format bump. The text
//! semantics (NFC, case folding, token boundaries, positions) are the
//! shared normalizer's, stamped as its version. A generation built under
//! another format, shard size or normalizer is refused typed; the
//! migration is an explicit rebuild, never a reinterpretation.

use std::path::{Path, PathBuf};

use ciborium::Value as CborValue;
use quanta_index_core::CoreError;

use crate::normalize::{TEXT_NORMALIZER_VERSION, TextNormalizerVersion};

/// Directory inside the generation directory that holds the text authority.
pub(crate) const TEXT_AUTHORITY_DIR_NAME: &str = "text-authority";
/// The manifest file inside [`TEXT_AUTHORITY_DIR_NAME`].
pub(crate) const TEXT_AUTHORITY_MANIFEST_FILE_NAME: &str = "manifest.cbor";
/// The sharded layout this build writes and serves.
pub(crate) const TEXT_AUTHORITY_FORMAT_VERSION: u32 = 1;
/// Documents per shard.
///
/// Sized from the measured sidecar cost of about 1.1 KiB per document
/// (QI-BB-006, 1,502 scopes at 1.68 MB): a shard is then about 2 MiB, so a
/// one-scope delta rewrites at most two shards plus the manifest — a few
/// MiB regardless of corpus size — while a 100k-document corpus is 49
/// files, not one small file per chunk. The per-shard candidate set can
/// never exceed the pre-verify cap (100k), so every candidate cap is
/// enforced on the union, exactly as over one index.
pub(crate) const SHARD_DOCS: u64 = 2048;
/// Highest doc id the position engine can encode as a posting gap
/// (`u32::MAX`; the first doc of a posting list is written as an absolute
/// `u32` gap).
pub(crate) const MAX_DOC_ID: u64 = 0xFFFF_FFFF;
/// Shard file names carry the first eight digest bytes as hex.
const SHARD_NAME_DIGEST_BYTES: usize = 8;

/// One shard as the manifest commits to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ShardEntry {
    /// Shard index; the shard covers doc ids
    /// `[index * SHARD_DOCS, (index + 1) * SHARD_DOCS)`.
    pub(crate) index: u64,
    /// Documents in the shard; never zero, an empty shard is not listed.
    pub(crate) rows: u64,
    /// Lowest doc id in the shard.
    pub(crate) min_doc_id: u64,
    /// Highest doc id in the shard.
    pub(crate) max_doc_id: u64,
    /// Length of the shard file.
    pub(crate) bytes: u64,
    /// SHA-256 of the shard file.
    pub(crate) sha256: [u8; 32],
}

impl ShardEntry {
    /// The shard's file name: index and content digest, so a rewrite is a
    /// new file and two shards with different content never share a name.
    pub(crate) fn file_name(&self) -> String {
        shard_file_name(self.index, &self.sha256)
    }

    /// The shard's path inside `generation_dir`.
    pub(crate) fn path(&self, generation_dir: &Path) -> PathBuf {
        text_authority_dir(generation_dir).join(self.file_name())
    }
}

/// The manifest's variable content; the layout parameters and the
/// normalizer stamp are this build's constants, checked at decode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextAuthorityManifest {
    /// Highest doc id ever assigned in this generation's chain (0 when none
    /// was). Doc ids are never reused: a new document is `max_doc_id + 1`,
    /// so it lands in the last shard or opens a new one.
    pub(crate) max_doc_id: u64,
    /// Listed shards, strictly ascending by index; a shard with no rows is
    /// not listed and has no file.
    pub(crate) shards: Vec<ShardEntry>,
}

/// Wire shape: a fixed-order CBOR array, auditable without a derive.
/// Element 0 is the format version, read on its own before the rest.
type ManifestRow = (
    u32,
    u64,
    (u16, u16),
    u64,
    Vec<(u64, u64, u64, u64, u64, [u8; 32])>,
);

pub(crate) fn text_authority_dir(generation_dir: &Path) -> PathBuf {
    generation_dir.join(TEXT_AUTHORITY_DIR_NAME)
}

pub(crate) fn manifest_path(generation_dir: &Path) -> PathBuf {
    text_authority_dir(generation_dir).join(TEXT_AUTHORITY_MANIFEST_FILE_NAME)
}

/// The shard a document belongs to.
pub(crate) fn shard_index_of(doc_id: u64) -> u64 {
    doc_id.div_euclid(SHARD_DOCS)
}

/// The inclusive doc-id range of shard `index`.
pub(crate) fn shard_doc_range(index: u64) -> Result<(u64, u64), CoreError> {
    let first = index.checked_mul(SHARD_DOCS).ok_or_else(|| {
        CoreError::InvalidContract(format!(
            "lexical: text authority shard index {index} overflows the doc-id space"
        ))
    })?;
    let last = first
        .checked_add(SHARD_DOCS.saturating_sub(1))
        .ok_or_else(|| {
            CoreError::InvalidContract(format!(
                "lexical: text authority shard index {index} overflows the doc-id space"
            ))
        })?;
    Ok((first, last))
}

/// `shard-<index>-<digest prefix>.cbor`.
pub(crate) fn shard_file_name(index: u64, sha256: &[u8; 32]) -> String {
    // The first eight digest bytes, big-endian, as one 16-hex-digit word.
    let prefix = sha256
        .iter()
        .take(SHARD_NAME_DIGEST_BYTES)
        .fold(0_u64, |word, byte| word.wrapping_shl(8) | u64::from(*byte));
    format!("shard-{index:08}-{prefix:016x}.cbor")
}

/// The format version at the head of a fixed-order manifest row.
///
/// Read before the row's shape is assumed, so an older format is refused by
/// name rather than as a decode failure.
pub(crate) fn leading_format_version(
    value: &CborValue,
    what: &str,
    path: &Path,
) -> Result<u32, CoreError> {
    let unreadable = |detail: &str| {
        CoreError::Storage(format!(
            "lexical: decode {what} {}: {detail}",
            path.display()
        ))
    };
    let CborValue::Array(items) = value else {
        return Err(unreadable("manifest is not an array"));
    };
    let Some(CborValue::Integer(format_version)) = items.first() else {
        return Err(unreadable("manifest has no leading format version"));
    };
    u32::try_from(*format_version)
        .map_err(|_overflow| unreadable("manifest format version is not a u32"))
}

fn format_unsupported(path: &Path, detail: &str) -> CoreError {
    CoreError::Typed {
        code:
            quanta_index_contract::SearchPlaneErrorCodeV2::GenerationTextAuthorityFormatUnsupported,
        message: format!(
            "lexical: text authority {} {detail}; this build serves text-authority format {TEXT_AUTHORITY_FORMAT_VERSION} with {SHARD_DOCS} documents per shard, and the generation must be rebuilt, never reinterpreted",
            path.display()
        ),
    }
}

/// A manifest that is structurally impossible: the sidecar is corrupt.
fn manifest_corrupt(generation_dir: &Path, reason: &str) -> CoreError {
    crate::index_store::sidecar_corrupt(
        generation_dir,
        &format!("{TEXT_AUTHORITY_DIR_NAME}/{TEXT_AUTHORITY_MANIFEST_FILE_NAME}"),
        reason,
    )
}

impl TextAuthorityManifest {
    /// Encode as the fixed-order row this build's decoder accepts.
    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        let row: ManifestRow = (
            TEXT_AUTHORITY_FORMAT_VERSION,
            SHARD_DOCS,
            (TEXT_NORMALIZER_VERSION.major, TEXT_NORMALIZER_VERSION.minor),
            self.max_doc_id,
            self.shards
                .iter()
                .map(|shard| {
                    (
                        shard.index,
                        shard.rows,
                        shard.min_doc_id,
                        shard.max_doc_id,
                        shard.bytes,
                        shard.sha256,
                    )
                })
                .collect(),
        );
        crate::channel_payloads::encode_cbor(&row, "text authority manifest")
    }

    /// Decode and validate a manifest read from `generation_dir`.
    ///
    /// Refuses typed: another format version or shard size
    /// (`GENERATION_TEXT_AUTHORITY_FORMAT_UNSUPPORTED`), another normalizer
    /// (`GENERATION_NORMALIZER_UNSUPPORTED`), and any structural violation —
    /// duplicate or unordered shards, an empty shard, doc-id extremes
    /// outside the shard's range or above the watermark
    /// (`GENERATION_SIDECAR_CORRUPT`).
    pub(crate) fn decode(bytes: &[u8], generation_dir: &Path) -> Result<Self, CoreError> {
        let path = manifest_path(generation_dir);
        let value: CborValue = ciborium::from_reader(bytes).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: decode text authority manifest {}: {error}",
                path.display()
            ))
        })?;
        let format_version = leading_format_version(&value, "text authority manifest", &path)?;
        if format_version != TEXT_AUTHORITY_FORMAT_VERSION {
            return Err(format_unsupported(
                &path,
                &format!("was written under text-authority format {format_version}"),
            ));
        }
        let (_format, shard_docs, (major, minor), max_doc_id, shards): ManifestRow =
            value.deserialized().map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: decode text authority manifest {}: {error}",
                    path.display()
                ))
            })?;
        if shard_docs != SHARD_DOCS {
            return Err(format_unsupported(
                &path,
                &format!("was written with {shard_docs} documents per shard"),
            ));
        }
        let normalizer = TextNormalizerVersion { major, minor };
        if normalizer != TEXT_NORMALIZER_VERSION {
            return Err(crate::index_store::normalizer_unsupported(
                &path, normalizer,
            ));
        }
        if max_doc_id > MAX_DOC_ID {
            return Err(manifest_corrupt(
                generation_dir,
                &format!("watermark {max_doc_id} exceeds the encodable doc-id range"),
            ));
        }
        let mut entries: Vec<ShardEntry> = Vec::with_capacity(shards.len());
        for (index, rows, min_doc_id, shard_max_doc_id, bytes, sha256) in shards {
            let entry = ShardEntry {
                index,
                rows,
                min_doc_id,
                max_doc_id: shard_max_doc_id,
                bytes,
                sha256,
            };
            validate_entry(&entry, max_doc_id, generation_dir)?;
            if let Some(previous) = entries.last()
                && previous.index >= entry.index
            {
                return Err(manifest_corrupt(
                    generation_dir,
                    &format!(
                        "shard {} listed after shard {}; shards must be strictly ascending",
                        entry.index, previous.index
                    ),
                ));
            }
            entries.push(entry);
        }
        Ok(Self {
            max_doc_id,
            shards: entries,
        })
    }

    /// The shard entry for `index`, if listed.
    pub(crate) fn shard(&self, index: u64) -> Option<&ShardEntry> {
        self.shards
            .binary_search_by_key(&index, |shard| shard.index)
            .map_or(None, |position| self.shards.get(position))
    }
}

fn validate_entry(
    entry: &ShardEntry,
    watermark: u64,
    generation_dir: &Path,
) -> Result<(), CoreError> {
    let (first, last) = shard_doc_range(entry.index)?;
    if entry.rows == 0 {
        return Err(manifest_corrupt(
            generation_dir,
            &format!("shard {} is listed with no rows", entry.index),
        ));
    }
    if entry.rows > SHARD_DOCS {
        return Err(manifest_corrupt(
            generation_dir,
            &format!(
                "shard {} lists {} rows, more than its {SHARD_DOCS}-document range",
                entry.index, entry.rows
            ),
        ));
    }
    if entry.min_doc_id > entry.max_doc_id || entry.min_doc_id < first || entry.max_doc_id > last {
        return Err(manifest_corrupt(
            generation_dir,
            &format!(
                "shard {} lists doc ids {}..={} outside its range {first}..={last}",
                entry.index, entry.min_doc_id, entry.max_doc_id
            ),
        ));
    }
    if entry.max_doc_id > watermark {
        return Err(manifest_corrupt(
            generation_dir,
            &format!(
                "shard {} lists doc id {} above the watermark {watermark}",
                entry.index, entry.max_doc_id
            ),
        ));
    }
    Ok(())
}

/// The manifest under `generation_dir`, or `None` when the generation has
/// published no text authority yet.
///
/// A missing file is the only `None`: an unreadable or invalid manifest is
/// an error, never "no text authority".
pub(crate) fn read_manifest(
    generation_dir: &Path,
) -> Result<Option<TextAuthorityManifest>, CoreError> {
    let path = manifest_path(generation_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "lexical: read text authority manifest {}: {error}",
                path.display()
            )));
        }
    };
    TextAuthorityManifest::decode(&bytes, generation_dir).map(Some)
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_DOC_ID, ManifestRow, SHARD_DOCS, ShardEntry, TEXT_AUTHORITY_FORMAT_VERSION,
        TextAuthorityManifest, shard_doc_range, shard_file_name, shard_index_of,
    };
    use quanta_index_core::CoreError;
    use std::path::Path;

    fn entry(index: u64, min: u64, max: u64) -> ShardEntry {
        ShardEntry {
            index,
            rows: max.saturating_sub(min).saturating_add(1),
            min_doc_id: min,
            max_doc_id: max,
            bytes: 10,
            sha256: [index.to_le_bytes()[0]; 32],
        }
    }

    fn typed_code(
        result: &Result<TextAuthorityManifest, CoreError>,
    ) -> Option<quanta_index_contract::SearchPlaneErrorCodeV2> {
        match result {
            Err(CoreError::Typed { code, .. }) => Some(*code),
            _ => None,
        }
    }

    #[test]
    fn a_document_maps_to_the_shard_holding_its_range() {
        assert_eq!(shard_index_of(1), 0);
        assert_eq!(shard_index_of(SHARD_DOCS - 1), 0);
        assert_eq!(shard_index_of(SHARD_DOCS), 1);
        assert_eq!(shard_index_of(2 * SHARD_DOCS), 2);
        assert_eq!(
            shard_doc_range(1).expect("range"),
            (SHARD_DOCS, 2 * SHARD_DOCS - 1)
        );
        assert_eq!(
            shard_file_name(3, &[0xab; 32]),
            "shard-00000003-abababababababab.cbor"
        );
    }

    #[test]
    fn a_manifest_round_trips_and_answers_shard_lookups() {
        let manifest = TextAuthorityManifest {
            max_doc_id: 2 * SHARD_DOCS + 5,
            shards: vec![
                entry(0, 1, SHARD_DOCS - 1),
                entry(2, 2 * SHARD_DOCS, 2 * SHARD_DOCS + 5),
            ],
        };
        let bytes = manifest.encode().expect("encode");
        let decoded = TextAuthorityManifest::decode(&bytes, Path::new("/g1")).expect("decode");
        assert_eq!(decoded, manifest);
        assert_eq!(decoded.shard(2).map(|shard| shard.rows), Some(6));
        assert!(decoded.shard(1).is_none());
    }

    #[test]
    fn structural_violations_are_typed_corruption() {
        let cases: Vec<(&str, TextAuthorityManifest)> = vec![
            (
                "duplicate shard",
                TextAuthorityManifest {
                    max_doc_id: 10,
                    shards: vec![entry(0, 1, 3), entry(0, 4, 5)],
                },
            ),
            (
                "unordered shards",
                TextAuthorityManifest {
                    max_doc_id: 3 * SHARD_DOCS,
                    shards: vec![entry(1, SHARD_DOCS, SHARD_DOCS), entry(0, 1, 3)],
                },
            ),
            (
                "doc id outside range",
                TextAuthorityManifest {
                    max_doc_id: 3 * SHARD_DOCS,
                    shards: vec![entry(0, 1, SHARD_DOCS)],
                },
            ),
            (
                "doc id above watermark",
                TextAuthorityManifest {
                    max_doc_id: 2,
                    shards: vec![entry(0, 1, 3)],
                },
            ),
            (
                "empty shard",
                TextAuthorityManifest {
                    max_doc_id: 5,
                    shards: vec![ShardEntry {
                        rows: 0,
                        ..entry(0, 1, 3)
                    }],
                },
            ),
            (
                "watermark beyond the encodable range",
                TextAuthorityManifest {
                    max_doc_id: MAX_DOC_ID + 1,
                    shards: Vec::new(),
                },
            ),
        ];
        for (label, manifest) in cases {
            let bytes = manifest.encode().expect("encode");
            let code = typed_code(&TextAuthorityManifest::decode(&bytes, Path::new("/g1")));
            assert_eq!(
                code,
                Some(quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt),
                "{label}"
            );
        }
    }

    /// The refusal code for a manifest row this build should not serve.
    fn refusal_for(row: &ManifestRow) -> Option<quanta_index_contract::SearchPlaneErrorCodeV2> {
        let bytes = crate::channel_payloads::encode_cbor(row, "test").expect("encode");
        typed_code(&TextAuthorityManifest::decode(&bytes, Path::new("/g1")))
    }

    #[test]
    fn another_format_or_shard_size_is_refused_by_name() {
        let other_format: ManifestRow = (99, SHARD_DOCS, (0, 0), 0, Vec::new());
        assert_eq!(
            refusal_for(&other_format),
            Some(quanta_index_contract::SearchPlaneErrorCodeV2::GenerationTextAuthorityFormatUnsupported)
        );
        let other_shard_size: ManifestRow = (
            TEXT_AUTHORITY_FORMAT_VERSION,
            SHARD_DOCS.div_euclid(2),
            (0, 0),
            0,
            Vec::new(),
        );
        assert_eq!(
            refusal_for(&other_shard_size),
            Some(quanta_index_contract::SearchPlaneErrorCodeV2::GenerationTextAuthorityFormatUnsupported)
        );
    }

    #[test]
    fn another_normalizer_is_refused_by_name() {
        let stale: ManifestRow = (
            TEXT_AUTHORITY_FORMAT_VERSION,
            SHARD_DOCS,
            (u16::MAX, u16::MAX),
            0,
            Vec::new(),
        );
        assert_eq!(
            refusal_for(&stale),
            Some(quanta_index_contract::SearchPlaneErrorCodeV2::GenerationNormalizerUnsupported)
        );
    }
}
