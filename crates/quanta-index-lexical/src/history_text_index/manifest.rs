//! The per-epoch manifest: what an epoch directory promises.
//!
//! Written last into the staging directory, before the rename that
//! publishes the epoch, so a published directory always has one. It
//! records the text normalizer the index was built under — an index
//! built under any other normalizer is never served, it is rebuilt — and
//! the digest of each kind's index commit file so a directory whose
//! files do not match its manifest is refused rather than searched.

use std::path::{Path, PathBuf};

use quanta_index_contract::AuxEpochV1;
use quanta_index_core::{CoreError, HistoryTextEpochStatusV1, HistoryTextKindV1, sha256_of_file};

use crate::history_text_index::layout::{fsync_parent, kind_dir};
use crate::normalize::{TEXT_NORMALIZER_VERSION, TextNormalizerVersion};

const MANIFEST_FILE_NAME: &str = "history-text-manifest.cbor";
/// Manifest format; there is no readable earlier format.
const MANIFEST_FORMAT_VERSION: u32 = 1;
/// The index engine's commit file, whose digest commits the index content.
const INDEX_COMMIT_FILE_NAME: &str = "meta.json";

/// What one epoch directory holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HistoryTextManifest {
    pub(super) normalizer: TextNormalizerVersion,
    pub(super) epoch: AuxEpochV1,
    pub(super) commits_commit_sha256: [u8; 32],
    pub(super) diffs_commit_sha256: [u8; 32],
}

/// Wire shape: a fixed-order CBOR array so the encoding is auditable
/// without a derive. Element 0 is the format version.
type ManifestRow = (u32, (u16, u16), u64, [u8; 32], [u8; 32]);

fn manifest_path(epoch_dir: &Path) -> PathBuf {
    epoch_dir.join(MANIFEST_FILE_NAME)
}

fn corrupt(message: String) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexCorrupt,
        message,
    }
}

/// The digest of one kind's index commit file inside `epoch_dir`.
pub(super) fn kind_commit_sha256(
    epoch_dir: &Path,
    kind: HistoryTextKindV1,
) -> Result<[u8; 32], CoreError> {
    let path = kind_dir(epoch_dir, kind).join(INDEX_COMMIT_FILE_NAME);
    sha256_of_file(&path)
        .map(|(_len, digest)| digest)
        .map_err(|err| {
            corrupt(format!(
                "history text index: read {} commit file {}: {err}",
                kind.as_str(),
                path.display()
            ))
        })
}

impl HistoryTextManifest {
    /// The manifest of a freshly built epoch directory: the current
    /// normalizer and the digests of what was just committed.
    pub(super) fn describe(epoch_dir: &Path, epoch: AuxEpochV1) -> Result<Self, CoreError> {
        Ok(Self {
            normalizer: TEXT_NORMALIZER_VERSION,
            epoch,
            commits_commit_sha256: kind_commit_sha256(epoch_dir, HistoryTextKindV1::Commit)?,
            diffs_commit_sha256: kind_commit_sha256(epoch_dir, HistoryTextKindV1::Diff)?,
        })
    }

    fn to_row(&self) -> ManifestRow {
        (
            MANIFEST_FORMAT_VERSION,
            (self.normalizer.major, self.normalizer.minor),
            self.epoch.get(),
            self.commits_commit_sha256,
            self.diffs_commit_sha256,
        )
    }

    fn from_row(row: ManifestRow) -> Result<Self, CoreError> {
        let (format_version, (major, minor), epoch, commits_commit_sha256, diffs_commit_sha256) =
            row;
        if format_version != MANIFEST_FORMAT_VERSION {
            return Err(corrupt(format!(
                "history text index: manifest format {format_version} is not the supported {MANIFEST_FORMAT_VERSION}"
            )));
        }
        Ok(Self {
            normalizer: TextNormalizerVersion { major, minor },
            epoch: AuxEpochV1::new(epoch),
            commits_commit_sha256,
            diffs_commit_sha256,
        })
    }

    /// Write the manifest into `epoch_dir` durably: a temporary file,
    /// fsync, atomic rename, directory fsync.
    pub(super) fn write(&self, epoch_dir: &Path) -> Result<(), CoreError> {
        let path = manifest_path(epoch_dir);
        let temporary = epoch_dir.join(format!("{MANIFEST_FILE_NAME}.tmp"));
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&self.to_row(), &mut bytes).map_err(|err| {
            CoreError::Storage(format!("history text index: encode manifest: {err}"))
        })?;
        {
            let mut file = std::fs::File::create(&temporary).map_err(|err| {
                CoreError::Storage(format!(
                    "history text index: create manifest {}: {err}",
                    temporary.display()
                ))
            })?;
            std::io::Write::write_all(&mut file, &bytes).map_err(|err| {
                CoreError::Storage(format!(
                    "history text index: write manifest {}: {err}",
                    temporary.display()
                ))
            })?;
            file.sync_all().map_err(|err| {
                CoreError::Storage(format!(
                    "history text index: fsync manifest {}: {err}",
                    temporary.display()
                ))
            })?;
        }
        std::fs::rename(&temporary, &path).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: publish manifest {}: {err}",
                path.display()
            ))
        })?;
        fsync_parent(&path)
    }

    /// The manifest a published epoch directory carries.
    pub(super) fn read(epoch_dir: &Path) -> Result<Self, CoreError> {
        let path = manifest_path(epoch_dir);
        let bytes = std::fs::read(&path).map_err(|err| {
            corrupt(format!("history text index: read manifest {}: {err}", path.display()))
        })?;
        let row: ManifestRow = ciborium::de::from_reader(bytes.as_slice()).map_err(|err| {
            corrupt(format!("history text index: decode manifest {}: {err}", path.display()))
        })?;
        Self::from_row(row)
    }
}

/// Whether the epoch directory at `epoch_dir` exists and can be served.
///
/// A directory that exists is complete by construction (see the module
/// doc of [`crate::history_text_index::layout`]), so a missing or
/// unreadable manifest, an epoch that is not the directory's, or a commit
/// file whose digest differs from the manifest's is a corruption error,
/// not an absence.
pub(super) fn epoch_status(
    epoch_dir: &Path,
    epoch: AuxEpochV1,
) -> Result<HistoryTextEpochStatusV1, CoreError> {
    if !epoch_dir.is_dir() {
        return Ok(HistoryTextEpochStatusV1::Absent);
    }
    let manifest = HistoryTextManifest::read(epoch_dir)?;
    if manifest.epoch != epoch {
        return Err(corrupt(format!(
            "history text index: {} is stamped epoch {} but sits under epoch {}",
            epoch_dir.display(),
            manifest.epoch,
            epoch
        )));
    }
    if manifest.normalizer != TEXT_NORMALIZER_VERSION {
        return Ok(HistoryTextEpochStatusV1::Unsupported {
            built_with: manifest.normalizer.to_string(),
        });
    }
    for (kind, expected) in [
        (HistoryTextKindV1::Commit, manifest.commits_commit_sha256),
        (HistoryTextKindV1::Diff, manifest.diffs_commit_sha256),
    ] {
        let observed = kind_commit_sha256(epoch_dir, kind)?;
        if observed != expected {
            return Err(corrupt(format!(
                "history text index: {} {} index does not match its manifest",
                epoch_dir.display(),
                kind.as_str()
            )));
        }
    }
    Ok(HistoryTextEpochStatusV1::Servable)
}
