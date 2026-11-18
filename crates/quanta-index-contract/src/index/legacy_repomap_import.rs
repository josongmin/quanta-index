//! Provisional input for importing one physically verified Legacy RepoMap
//! candidate into a Native projection. This shape grants no producer authority.

use super::{Digest, ObjectRef};
use crate::{ManifestGeneration, RepoId, RepoMapExpectedActiveV2, RevisionId};
use serde::{Deserialize, Serialize};

pub const MAX_LEGACY_REPOMAP_IMPORT_PAGE_BYTES_V1: u32 = 8 * 1024 * 1024;
pub const MAX_LEGACY_REPOMAP_IMPORT_PAYLOAD_BYTES_V1: u64 = 64 * 1024 * 1024;
pub const MAX_LEGACY_REPOMAP_IMPORT_PAGES_V1: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRepoMapImportPageV1 {
    pub object: ObjectRef,
    pub bytes: u32,
}

/// The complete old canonical compiler payload is partitioned into immutable,
/// childless pages. The old active candidate remains independently verified
/// under a held Legacy root at the final switch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRepoMapImportV1 {
    pub wire_version: u32,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub old_active: RepoMapExpectedActiveV2,
    pub payload_bytes: u64,
    pub payload_sha256: Digest,
    pub pages: Vec<LegacyRepoMapImportPageV1>,
}

impl LegacyRepoMapImportV1 {
    pub const WIRE_VERSION: u32 = 1;

    pub fn validate_shape_v1(&self) -> Result<(), &'static str> {
        if self.wire_version != Self::WIRE_VERSION
            || self.generation.get() == 0
            || self.payload_bytes == 0
            || self.payload_bytes > MAX_LEGACY_REPOMAP_IMPORT_PAYLOAD_BYTES_V1
            || self.payload_sha256 == Digest([0; 32])
            || self.pages.is_empty()
            || self.pages.len() > MAX_LEGACY_REPOMAP_IMPORT_PAGES_V1
        {
            return Err("Legacy RepoMap import scope or size is invalid");
        }
        let mut total = 0_u64;
        for (position, page) in self.pages.iter().enumerate() {
            if page.object.0 == Digest([0; 32])
                || page.bytes == 0
                || page.bytes > MAX_LEGACY_REPOMAP_IMPORT_PAGE_BYTES_V1
                || (position + 1 < self.pages.len()
                    && page.bytes != MAX_LEGACY_REPOMAP_IMPORT_PAGE_BYTES_V1)
            {
                return Err("Legacy RepoMap import page is not canonical");
            }
            total = total
                .checked_add(u64::from(page.bytes))
                .ok_or("Legacy RepoMap import byte count overflow")?;
        }
        if total != self.payload_bytes {
            return Err("Legacy RepoMap import page lengths differ from payload");
        }
        Ok(())
    }

    pub fn verify_children_v1(&self, children: &[ObjectRef]) -> Result<(), &'static str> {
        self.validate_shape_v1()?;
        let mut expected: Vec<ObjectRef> = self.pages.iter().map(|page| page.object).collect();
        expected.sort_unstable();
        expected.dedup();
        if children != expected {
            return Err("Legacy RepoMap import pages differ from immutable children");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_page_partition_and_child_set_are_required() {
        let mut input = LegacyRepoMapImportV1 {
            wire_version: 1,
            repo_id: RepoId::new("repo").expect("repo"),
            revision_id: RevisionId::new("revision").expect("revision"),
            generation: ManifestGeneration::new(1),
            old_active: RepoMapExpectedActiveV2::new(
                std::num::NonZeroU64::new(1).expect("epoch"),
                crate::CandidateCommitmentV1::from_bytes([4; 32]),
            ),
            payload_bytes: u64::from(MAX_LEGACY_REPOMAP_IMPORT_PAGE_BYTES_V1) + 1,
            payload_sha256: Digest([7; 32]),
            pages: vec![
                LegacyRepoMapImportPageV1 {
                    object: ObjectRef(Digest([2; 32])),
                    bytes: MAX_LEGACY_REPOMAP_IMPORT_PAGE_BYTES_V1,
                },
                LegacyRepoMapImportPageV1 {
                    object: ObjectRef(Digest([3; 32])),
                    bytes: 1,
                },
            ],
        };
        let children = [input.pages[0].object, input.pages[1].object];
        input
            .verify_children_v1(&children)
            .expect("canonical pages");
        input.pages[0].bytes -= 1;
        assert!(input.validate_shape_v1().is_err());
    }

    #[test]
    fn repeated_content_addressed_page_keeps_both_ordered_chunks() {
        let page = ObjectRef(Digest([2; 32]));
        let input = LegacyRepoMapImportV1 {
            wire_version: 1,
            repo_id: RepoId::new("repo").expect("repo"),
            revision_id: RevisionId::new("revision").expect("revision"),
            generation: ManifestGeneration::new(1),
            old_active: RepoMapExpectedActiveV2::new(
                std::num::NonZeroU64::new(1).expect("epoch"),
                crate::CandidateCommitmentV1::from_bytes([4; 32]),
            ),
            payload_bytes: 2 * u64::from(MAX_LEGACY_REPOMAP_IMPORT_PAGE_BYTES_V1),
            payload_sha256: Digest([7; 32]),
            pages: vec![
                LegacyRepoMapImportPageV1 {
                    object: page,
                    bytes: MAX_LEGACY_REPOMAP_IMPORT_PAGE_BYTES_V1,
                },
                LegacyRepoMapImportPageV1 {
                    object: page,
                    bytes: MAX_LEGACY_REPOMAP_IMPORT_PAGE_BYTES_V1,
                },
            ],
        };
        input
            .verify_children_v1(&[page])
            .expect("one physical child");
        assert!(input.verify_children_v1(&[page, page]).is_err());
    }
}
