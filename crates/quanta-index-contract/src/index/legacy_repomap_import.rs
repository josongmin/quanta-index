//! Provisional input for importing one physically verified Legacy RepoMap
//! candidate into a Native projection. This shape grants no producer authority.

use super::{Digest, ObjectRef};
use crate::{ManifestGeneration, RepoId, RepoMapExpectedActiveV2, RevisionId};

pub const MAX_LEGACY_REPOMAP_IMPORT_PAGE_BYTES_V1: u32 = 8 * 1024 * 1024;
pub const MAX_LEGACY_REPOMAP_IMPORT_PAYLOAD_BYTES_V1: u64 = 64 * 1024 * 1024;
pub const MAX_LEGACY_REPOMAP_IMPORT_PAGES_V1: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyRepoMapImportPageV1 {
    pub object: ObjectRef,
    pub bytes: u32,
}
crate::native_serde_struct!([serde_identifier] LegacyRepoMapImportPageV1 {
    object: ObjectRef,
    bytes: u32,
});

/// The complete old canonical compiler payload is partitioned into immutable,
/// childless pages. The old active candidate remains independently verified
/// under a held Legacy root at the final switch.
#[derive(Clone, Debug, Eq, PartialEq)]
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
crate::native_serde_struct!([serde_identifier] LegacyRepoMapImportV1 {
    wire_version: u32,
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    old_active: RepoMapExpectedActiveV2,
    payload_bytes: u64,
    payload_sha256: Digest,
    pages: Vec<LegacyRepoMapImportPageV1>,
});

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
    use sha2::Digest as _;

    #[test]
    fn legacy_import_wire_preserves_ordered_fields_and_byte_key_decode() {
        let input = LegacyRepoMapImportV1 {
            wire_version: 1,
            repo_id: RepoId::new("repo").expect("repo"),
            revision_id: RevisionId::new("revision").expect("revision"),
            generation: ManifestGeneration::new(1),
            old_active: RepoMapExpectedActiveV2::new(
                std::num::NonZeroU64::new(1).expect("epoch"),
                crate::CandidateCommitmentV1::from_bytes([4; 32]),
            ),
            payload_bytes: 1,
            payload_sha256: Digest([7; 32]),
            pages: vec![LegacyRepoMapImportPageV1 {
                object: ObjectRef(Digest([2; 32])),
                bytes: 1,
            }],
        };
        let json = serde_json::to_vec(&input).expect("JSON encode");
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&input, &mut cbor).expect("CBOR encode");
        assert_eq!(
            format!("{:x}", sha2::Sha256::digest(&json)),
            "674895d0c8a40253e9192db7c693d5ba21030cef0be31f80406b5ad4a8b1d70c"
        );
        assert_eq!(
            format!("{:x}", sha2::Sha256::digest(&cbor)),
            "bcc376efc108986bfad4c79779c4d75d88de70116c6b770cb4caade11bcb9e02"
        );

        let mut value = ciborium::value::Value::serialized(&input).expect("CBOR value");
        let ciborium::value::Value::Map(fields) = &mut value else {
            panic!("header must be a map");
        };
        for (key, field_value) in fields.iter_mut() {
            if let ciborium::value::Value::Text(name) = key {
                *key = ciborium::value::Value::Bytes(name.as_bytes().to_vec());
            }
            if matches!(key, ciborium::value::Value::Bytes(name) if name == b"pages") {
                let ciborium::value::Value::Array(pages) = field_value else {
                    panic!("pages must be an array");
                };
                let ciborium::value::Value::Map(page_fields) =
                    pages.first_mut().expect("single page")
                else {
                    panic!("page must be a map");
                };
                for (page_key, _) in page_fields {
                    if let ciborium::value::Value::Text(name) = page_key {
                        *page_key = ciborium::value::Value::Bytes(name.as_bytes().to_vec());
                    }
                }
            }
        }
        let mut byte_keys = Vec::new();
        ciborium::ser::into_writer(&value, &mut byte_keys).expect("byte-key CBOR encode");
        let decoded: LegacyRepoMapImportV1 =
            ciborium::de::from_reader(byte_keys.as_slice()).expect("byte-key CBOR decode");
        assert_eq!(decoded, input);
        decoded.validate_shape_v1().expect("valid decoded import");
    }

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
