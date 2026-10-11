//! Paged file authority and transient selected-source views.

use quanta_index_contract::SourceFileRevision;
#[cfg(test)]
use quanta_index_contract::lex::LanguageCode;
use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};
use std::path::Path;
use std::sync::Arc;

pub(crate) mod paged;
mod plan;
mod query;

pub(crate) use plan::{FileAuthorityPlanCache, plan_ops};
pub(crate) use query::{PostingSurface, QueryWork};

pub(crate) const DIR: &str = "file-authority";
pub(crate) const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_TOTAL_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
// A CodeSearch request can perform language, short-literal size and exact
// scans, then language and shortlist scans on an empty-result typo fallback.
const MAX_QUERY_SOURCE_WALKS: u64 = 5;

#[derive(Clone, Copy)]
pub(crate) struct FilePolicy {
    pub(crate) source_files: u64,
    pub(crate) source_bytes: u64,
    pub(crate) total_memberships: u64,
    pub(crate) partitions: u64,
    pub(crate) bucket_scratch_bytes: u64,
    pub(crate) query_list_reads: u64,
    pub(crate) query_posting_ids: u64,
    pub(crate) query_decoded_bytes: u64,
    pub(crate) query_decoded_ids: u64,
}

pub(crate) fn policy() -> FilePolicy {
    FilePolicy {
        source_files: 32_768,
        source_bytes: MAX_TOTAL_SOURCE_BYTES,
        total_memberships: u64::from(crate::FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT),
        partitions: 256,
        bucket_scratch_bytes: 128 * 1024 * 1024,
        query_list_reads: 16_384,
        query_posting_ids: 2_000_000,
        query_decoded_bytes: 512 * 1024 * 1024,
        query_decoded_ids: u64::from(crate::FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT),
    }
}

pub(crate) fn query_physical_read_limit(policy: FilePolicy) -> Result<u64, CoreError> {
    // One authenticated source-head read follows at most height + 1 map
    // pages for each row in a full source-map walk.
    let reads_per_source = u64::from(crate::lexical_store::MAX_MAP_HEIGHT)
        .checked_add(2)
        .ok_or_else(|| CoreError::InvalidContract("file authority map depth overflow".into()))?;
    policy
        .source_files
        .checked_mul(reads_per_source)
        .and_then(|reads| reads.checked_mul(MAX_QUERY_SOURCE_WALKS))
        .and_then(|reads| reads.checked_add(policy.query_list_reads))
        .ok_or_else(|| {
            CoreError::InvalidContract("file authority query read limit overflow".into())
        })
}

fn policy_digest_for_paged() -> [u8; 32] {
    let policy = policy();
    let mut digest = Sha256::new();
    digest.update(b"quanta-file-authority-v17-paged-policy-fields\0");
    for field in [
        policy.source_files,
        policy.source_bytes,
        policy.total_memberships,
        policy.partitions,
        policy.bucket_scratch_bytes,
        policy.query_list_reads,
        policy.query_posting_ids,
        policy.query_decoded_bytes,
        policy.query_decoded_ids,
        MAX_QUERY_SOURCE_WALKS,
        u64::from(crate::lexical_store::MAX_MAP_HEIGHT),
    ] {
        digest.update(field.to_le_bytes());
    }
    digest.finalize().into()
}

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidContract(format!("file authority: {reason}"))
}

fn read_root_bytes(directory: &Path) -> Result<Option<Vec<u8>>, CoreError> {
    use std::io::ErrorKind;
    let corrupt_root = |error: &std::io::Error| {
        crate::index_store::sidecar_corrupt(
            directory,
            crate::lexical_root_names::FILE_ROOT_NAME,
            &error.to_string(),
        )
    };
    let generation = match crate::sealed_generation::open_generation_dir_nofollow(directory) {
        Ok(generation) => generation,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) if crate::sealed_generation::is_unsafe_artifact_path(&error) => {
            return Err(corrupt_root(&error));
        }
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "pin file authority generation: {error}"
            )));
        }
    };
    // A missing family is a valid pre-publication state. Inspect it relative
    // to the pinned generation so a symlink or non-directory is never mistaken
    // for absence (including platforms that report ENOTDIR on the nested open).
    let family_present = crate::sealed_generation::optional_entry_at(&generation, Path::new(DIR))
        .map_err(|error| {
        CoreError::Storage(format!("inspect file authority family: {error}"))
    })?;
    if !family_present {
        return Ok(None);
    }
    let relative = Path::new(crate::lexical_root_names::FILE_ROOT_NAME);
    let mut opened = match crate::sealed_generation::open_regular_below(&generation, relative) {
        Ok(opened) => opened,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) if crate::sealed_generation::is_unsafe_artifact_path(&error) => {
            return Err(corrupt_root(&error));
        }
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "open file authority root: {error}"
            )));
        }
    };
    crate::sealed_generation::read_opened_bounded(&mut opened, paged::MAX_ROOT_BYTES)
        .map(Some)
        .map_err(|error| {
            if matches!(
                error.kind(),
                ErrorKind::InvalidData | ErrorKind::UnexpectedEof
            ) {
                corrupt_root(&error)
            } else {
                CoreError::Storage(format!("read file authority root: {error}"))
            }
        })
}

/// Raw bytes share their allocation with normalized text only when the proved
/// byte sequences are identical. Binary sources never acquire a text view.
#[derive(Clone, Debug)]
pub(crate) enum SourceBytes {
    Text(Arc<str>),
    Binary(Arc<[u8]>),
}

impl std::ops::Deref for SourceBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::Binary(bytes) => bytes,
        }
    }
}

#[cfg(test)]
impl From<Vec<u8>> for SourceBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Binary(bytes.into())
    }
}

fn share_verified_surfaces(
    source: &[u8],
    raw: Option<&str>,
    indexed: Option<String>,
    folded: Option<String>,
) -> (SourceBytes, Option<Arc<str>>, Option<Arc<str>>) {
    let raw = raw.map(Arc::<str>::from);
    let indexed = indexed.map(|text| {
        raw.as_ref()
            .filter(|raw| raw.as_bytes() == text.as_bytes())
            .map_or_else(|| Arc::<str>::from(text), Arc::clone)
    });
    let folded = folded.map(|text| {
        raw.as_ref()
            .filter(|raw| raw.as_bytes() == text.as_bytes())
            .or_else(|| {
                indexed
                    .as_ref()
                    .filter(|indexed| indexed.as_bytes() == text.as_bytes())
            })
            .map_or_else(|| Arc::<str>::from(text), Arc::clone)
    });
    let bytes = raw.map_or_else(|| SourceBytes::Binary(Arc::from(source)), SourceBytes::Text);
    (bytes, indexed, folded)
}

#[derive(Clone, Debug)]
pub(crate) struct SourceFile {
    pub(crate) source: SourceFileRevision,
    pub(crate) bytes: SourceBytes,
    #[cfg(test)]
    pub(crate) text_admitted: bool,
    #[cfg(test)]
    pub(crate) language: LanguageCode,
    pub(crate) indexed_text: Option<Arc<str>>,
    pub(crate) folded_text: Option<Arc<str>>,
    pub(crate) indexed_path: String,
    pub(crate) folded_path: String,
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

#[cfg(test)]
mod root_read_tests {
    use super::*;

    #[test]
    fn optional_root_distinguishes_absence_from_oversize_and_nofollow_faults() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let generation = temporary.path().join("family").join("generation");
        std::fs::create_dir_all(&generation).expect("generation");
        assert!(
            read_root_bytes(&generation)
                .expect("absent family")
                .is_none()
        );

        let family = generation.join(DIR);
        std::fs::create_dir(&family).expect("authority family");
        assert!(read_root_bytes(&generation).expect("absent root").is_none());

        let root = family.join("root.cbor");
        std::fs::write(&root, vec![0_u8; paged::MAX_ROOT_BYTES + 1]).expect("oversized root");
        assert!(matches!(
            read_root_bytes(&generation),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));

        std::fs::remove_file(&root).expect("remove oversized root");
        std::os::unix::fs::symlink(temporary.path().join("external"), &root)
            .expect("dangling root symlink");
        assert!(matches!(
            read_root_bytes(&generation),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
    }
}
