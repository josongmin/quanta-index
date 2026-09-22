//! Exact `RepoMap` V2 publish/activate request and terminal receipt contracts.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};
use sha2::{Digest as _, Sha256};

use super::{RepoMapActivateGenerationRequest, RepoMapMutationAck, RepoMapSourceBundle};

const SOURCE_BUNDLE_DIGEST_DOMAIN_V2: &[u8] = b"quanta-index/repomap-source-bundle/v2\0";

/// The canonical-CBOR format version of a [`RepoMapTerminalReceiptV2`]
/// persisted as an operation-journal terminal payload (SEP-21 P02B).
///
/// Same rule as the batch receipt tag: a journal-persisted payload is
/// this version tag followed by the receipt's canonical CBOR, and any
/// other version is a typed refusal before any mutation — no dual
/// decoder, no live migration. Receipts of another version are
/// offline-migration input only.
pub const REPOMAP_TERMINAL_RECEIPT_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapSourceBundleDigestErrorV2 {
    detail: String,
}

impl fmt::Display for RepoMapSourceBundleDigestErrorV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "REPOMAP_SOURCE_BUNDLE_DIGEST_FAILED:{}",
            self.detail
        )
    }
}

impl std::error::Error for RepoMapSourceBundleDigestErrorV2 {}

/// SHA-256 over the exact manual-serde CBOR bundle, with a versioned domain.
///
/// The manual serde implementations emit every struct-map key and enum
/// variant in fixed order and the DTO has no floating-point values, so this
/// byte stream is deterministic across producer and daemon.
pub fn canonical_repo_map_source_bundle_digest_v2(
    bundle: &RepoMapSourceBundle,
) -> Result<String, RepoMapSourceBundleDigestErrorV2> {
    let mut encoded = Vec::new();
    ciborium::ser::into_writer(bundle, &mut encoded).map_err(|error| {
        RepoMapSourceBundleDigestErrorV2 {
            detail: error.to_string(),
        }
    })?;
    let mut hasher = Sha256::new();
    hasher.update(SOURCE_BUNDLE_DIGEST_DOMAIN_V2);
    hasher.update(encoded);
    let digest: [u8; 32] = hasher.finalize().into();
    Ok(format_sha256_wire_v2(&digest))
}

#[expect(
    clippy::indexing_slicing,
    reason = "both indices are masked to four bits and therefore within HEX"
)]
fn format_sha256_wire_v2(digest: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut wire = String::with_capacity(71);
    wire.push_str("sha256:");
    for byte in digest {
        wire.push(char::from(HEX[usize::from(byte >> 4)]));
        wire.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    wire
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepoMapMutationPhaseV2 {
    Publish,
    Activate,
}

impl RepoMapMutationPhaseV2 {
    const VARIANTS: &'static [&'static str] = &["publish", "activate"];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Publish => "publish",
            Self::Activate => "activate",
        }
    }
}

impl Serialize for RepoMapMutationPhaseV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

impl<'de> Deserialize<'de> for RepoMapMutationPhaseV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PhaseVisitor;
        impl Visitor<'_> for PhaseVisitor {
            type Value = RepoMapMutationPhaseV2;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a RepoMapMutationPhaseV2 string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "publish" => Ok(RepoMapMutationPhaseV2::Publish),
                    "activate" => Ok(RepoMapMutationPhaseV2::Activate),
                    other => Err(de::Error::unknown_variant(
                        other,
                        RepoMapMutationPhaseV2::VARIANTS,
                    )),
                }
            }
        }
        deserializer.deserialize_str(PhaseVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapPublishBundleRequestV2 {
    pub source_bundle_digest: String,
    pub bundle: RepoMapSourceBundle,
}

impl RepoMapPublishBundleRequestV2 {
    pub fn new(bundle: RepoMapSourceBundle) -> Result<Self, RepoMapSourceBundleDigestErrorV2> {
        let source_bundle_digest = canonical_repo_map_source_bundle_digest_v2(&bundle)?;
        Ok(Self {
            source_bundle_digest,
            bundle,
        })
    }
}

const PUBLISH_REQUEST_V2_FIELDS: &[&str] = &["source_bundle_digest", "bundle"];

impl Serialize for RepoMapPublishBundleRequestV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapPublishBundleRequestV2", 2)?;
        state.serialize_field("source_bundle_digest", &self.source_bundle_digest)?;
        state.serialize_field("bundle", &self.bundle)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for RepoMapPublishBundleRequestV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RequestVisitor;
        impl<'de> Visitor<'de> for RequestVisitor {
            type Value = RepoMapPublishBundleRequestV2;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a RepoMapPublishBundleRequestV2 map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut digest = None;
                let mut bundle = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "source_bundle_digest" if digest.is_none() => {
                            digest = Some(map.next_value()?);
                        }
                        "source_bundle_digest" => {
                            return Err(de::Error::duplicate_field("source_bundle_digest"));
                        }
                        "bundle" if bundle.is_none() => bundle = Some(map.next_value()?),
                        "bundle" => return Err(de::Error::duplicate_field("bundle")),
                        other => {
                            return Err(de::Error::unknown_field(other, PUBLISH_REQUEST_V2_FIELDS));
                        }
                    }
                }
                Ok(RepoMapPublishBundleRequestV2 {
                    source_bundle_digest: digest
                        .ok_or_else(|| de::Error::missing_field("source_bundle_digest"))?,
                    bundle: bundle.ok_or_else(|| de::Error::missing_field("bundle"))?,
                })
            }
        }
        deserializer.deserialize_struct(
            "RepoMapPublishBundleRequestV2",
            PUBLISH_REQUEST_V2_FIELDS,
            RequestVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapActivateGenerationRequestV2 {
    pub request_v1: RepoMapActivateGenerationRequest,
    pub snapshot_id: String,
    pub projection_version: u32,
    pub authority_digest: String,
    pub source_bundle_digest: String,
}

impl RepoMapActivateGenerationRequestV2 {
    pub fn for_bundle(
        bundle: &RepoMapSourceBundle,
    ) -> Result<Self, RepoMapSourceBundleDigestErrorV2> {
        Ok(Self {
            request_v1: RepoMapActivateGenerationRequest {
                repo_id: bundle.repo_id.clone(),
                revision_id: bundle.revision_id.clone(),
                manifest_generation: bundle.manifest_generation,
                manifest_digest: bundle.manifest_digest.clone(),
            },
            snapshot_id: bundle.snapshot_id.clone(),
            projection_version: bundle.projection_version,
            authority_digest: bundle.authority_digest.clone(),
            source_bundle_digest: canonical_repo_map_source_bundle_digest_v2(bundle)?,
        })
    }
}

const ACTIVATE_REQUEST_V2_FIELDS: &[&str] = &[
    "request_v1",
    "snapshot_id",
    "projection_version",
    "authority_digest",
    "source_bundle_digest",
];

impl Serialize for RepoMapActivateGenerationRequestV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapActivateGenerationRequestV2", 5)?;
        state.serialize_field("request_v1", &self.request_v1)?;
        state.serialize_field("snapshot_id", &self.snapshot_id)?;
        state.serialize_field("projection_version", &self.projection_version)?;
        state.serialize_field("authority_digest", &self.authority_digest)?;
        state.serialize_field("source_bundle_digest", &self.source_bundle_digest)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for RepoMapActivateGenerationRequestV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RequestVisitor;
        impl<'de> Visitor<'de> for RequestVisitor {
            type Value = RepoMapActivateGenerationRequestV2;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a RepoMapActivateGenerationRequestV2 map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let (
                    mut request_v1,
                    mut snapshot_id,
                    mut projection_version,
                    mut authority_digest,
                    mut source_bundle_digest,
                ) = (None, None, None, None, None);
                while let Some(key) = map.next_key::<String>()? {
                    macro_rules! read_once {
                        ($slot:ident, $field:literal) => {{
                            if $slot.is_some() {
                                return Err(de::Error::duplicate_field($field));
                            }
                            $slot = Some(map.next_value()?);
                        }};
                    }
                    match key.as_str() {
                        "request_v1" => read_once!(request_v1, "request_v1"),
                        "snapshot_id" => read_once!(snapshot_id, "snapshot_id"),
                        "projection_version" => {
                            read_once!(projection_version, "projection_version");
                        }
                        "authority_digest" => read_once!(authority_digest, "authority_digest"),
                        "source_bundle_digest" => {
                            read_once!(source_bundle_digest, "source_bundle_digest");
                        }
                        other => {
                            return Err(de::Error::unknown_field(
                                other,
                                ACTIVATE_REQUEST_V2_FIELDS,
                            ));
                        }
                    }
                }
                Ok(RepoMapActivateGenerationRequestV2 {
                    request_v1: request_v1.ok_or_else(|| de::Error::missing_field("request_v1"))?,
                    snapshot_id: snapshot_id
                        .ok_or_else(|| de::Error::missing_field("snapshot_id"))?,
                    projection_version: projection_version
                        .ok_or_else(|| de::Error::missing_field("projection_version"))?,
                    authority_digest: authority_digest
                        .ok_or_else(|| de::Error::missing_field("authority_digest"))?,
                    source_bundle_digest: source_bundle_digest
                        .ok_or_else(|| de::Error::missing_field("source_bundle_digest"))?,
                })
            }
        }
        deserializer.deserialize_struct(
            "RepoMapActivateGenerationRequestV2",
            ACTIVATE_REQUEST_V2_FIELDS,
            RequestVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapTerminalReceiptV2 {
    pub phase: RepoMapMutationPhaseV2,
    pub mutation: RepoMapMutationAck,
    pub manifest_digest: String,
    pub snapshot_id: String,
    pub projection_version: u32,
    pub authority_digest: String,
    pub source_bundle_digest: String,
}

const TERMINAL_RECEIPT_V2_FIELDS: &[&str] = &[
    "phase",
    "mutation",
    "manifest_digest",
    "snapshot_id",
    "projection_version",
    "authority_digest",
    "source_bundle_digest",
];

impl Serialize for RepoMapTerminalReceiptV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapTerminalReceiptV2", 7)?;
        state.serialize_field("phase", &self.phase)?;
        state.serialize_field("mutation", &self.mutation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("snapshot_id", &self.snapshot_id)?;
        state.serialize_field("projection_version", &self.projection_version)?;
        state.serialize_field("authority_digest", &self.authority_digest)?;
        state.serialize_field("source_bundle_digest", &self.source_bundle_digest)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for RepoMapTerminalReceiptV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ReceiptVisitor;
        impl<'de> Visitor<'de> for ReceiptVisitor {
            type Value = RepoMapTerminalReceiptV2;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a RepoMapTerminalReceiptV2 map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let (
                    mut phase,
                    mut mutation,
                    mut manifest_digest,
                    mut snapshot_id,
                    mut projection_version,
                    mut authority_digest,
                    mut source_bundle_digest,
                ) = (None, None, None, None, None, None, None);
                while let Some(key) = map.next_key::<String>()? {
                    macro_rules! read_once {
                        ($slot:ident, $field:literal) => {{
                            if $slot.is_some() {
                                return Err(de::Error::duplicate_field($field));
                            }
                            $slot = Some(map.next_value()?);
                        }};
                    }
                    match key.as_str() {
                        "phase" => read_once!(phase, "phase"),
                        "mutation" => read_once!(mutation, "mutation"),
                        "manifest_digest" => read_once!(manifest_digest, "manifest_digest"),
                        "snapshot_id" => read_once!(snapshot_id, "snapshot_id"),
                        "projection_version" => {
                            read_once!(projection_version, "projection_version");
                        }
                        "authority_digest" => read_once!(authority_digest, "authority_digest"),
                        "source_bundle_digest" => {
                            read_once!(source_bundle_digest, "source_bundle_digest");
                        }
                        other => {
                            return Err(de::Error::unknown_field(
                                other,
                                TERMINAL_RECEIPT_V2_FIELDS,
                            ));
                        }
                    }
                }
                Ok(RepoMapTerminalReceiptV2 {
                    phase: phase.ok_or_else(|| de::Error::missing_field("phase"))?,
                    mutation: mutation.ok_or_else(|| de::Error::missing_field("mutation"))?,
                    manifest_digest: manifest_digest
                        .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
                    snapshot_id: snapshot_id
                        .ok_or_else(|| de::Error::missing_field("snapshot_id"))?,
                    projection_version: projection_version
                        .ok_or_else(|| de::Error::missing_field("projection_version"))?,
                    authority_digest: authority_digest
                        .ok_or_else(|| de::Error::missing_field("authority_digest"))?,
                    source_bundle_digest: source_bundle_digest
                        .ok_or_else(|| de::Error::missing_field("source_bundle_digest"))?,
                })
            }
        }
        deserializer.deserialize_struct(
            "RepoMapTerminalReceiptV2",
            TERMINAL_RECEIPT_V2_FIELDS,
            ReceiptVisitor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FileId, ManifestGeneration, RepoId, RepoMapExactnessSummary, RepoMapFileNode,
        RepoMapGraphCoverage, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapNode,
        RepoMapRedactionState, RepoRelativePath, RevisionId,
    };

    fn bundle_fixture_v2() -> RepoMapSourceBundle {
        RepoMapSourceBundle::new(
            RepoId::new("repo-terminal-v2").expect("canonical repo id"),
            RevisionId::new("revision-terminal-v2").expect("canonical revision id"),
            ManifestGeneration::new(7),
            "a".repeat(64),
            "snapshot-terminal-v2",
            3,
            "b".repeat(64),
            RepoMapGraphCoverage {
                item_index_availability: RepoMapItemIndexAvailability::Available,
                graph_coverage_class: RepoMapGraphCoverageClass::Complete,
            },
            RepoMapExactnessSummary::Exact,
            RepoMapRedactionState::Unredacted,
        )
        .with_node(RepoMapNode::File(RepoMapFileNode {
            file_id: FileId::new("file://src/lib.rs"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            line_count: 17,
        }))
    }

    #[test]
    fn source_bundle_digest_binds_every_activation_axis_v2() {
        let source_v2 = bundle_fixture_v2();
        let expected_v2 = canonical_repo_map_source_bundle_digest_v2(&source_v2)
            .expect("canonical source bundle digest");
        assert_eq!(
            RepoMapPublishBundleRequestV2::new(source_v2.clone())
                .expect("publish request")
                .source_bundle_digest,
            expected_v2,
        );
        assert_eq!(
            RepoMapActivateGenerationRequestV2::for_bundle(&source_v2)
                .expect("activation request")
                .source_bundle_digest,
            expected_v2,
        );

        let mut mutations_v2 = Vec::new();
        let mut repo_v2 = source_v2.clone();
        repo_v2.repo_id = RepoId::new("repo-terminal-v2-foreign").expect("canonical repo id");
        mutations_v2.push(repo_v2);
        let mut revision_v2 = source_v2.clone();
        revision_v2.revision_id =
            RevisionId::new("revision-terminal-v2-foreign").expect("canonical revision id");
        mutations_v2.push(revision_v2);
        let mut generation_v2 = source_v2.clone();
        generation_v2.manifest_generation = ManifestGeneration::new(8);
        mutations_v2.push(generation_v2);
        let mut manifest_v2 = source_v2.clone();
        manifest_v2.manifest_digest = "c".repeat(64);
        mutations_v2.push(manifest_v2);
        let mut snapshot_v2 = source_v2.clone();
        snapshot_v2.snapshot_id.push_str("-foreign");
        mutations_v2.push(snapshot_v2);
        let mut projection_v2 = source_v2.clone();
        projection_v2.projection_version += 1;
        mutations_v2.push(projection_v2);
        let mut authority_v2 = source_v2.clone();
        authority_v2.authority_digest = "d".repeat(64);
        mutations_v2.push(authority_v2);
        let mut payload_v2 = source_v2;
        payload_v2.nodes.push(RepoMapNode::File(RepoMapFileNode {
            file_id: FileId::new("file://src/foreign.rs"),
            repo_relative_path: RepoRelativePath::new("src/foreign.rs"),
            line_count: 23,
        }));
        mutations_v2.push(payload_v2);

        for mutated_v2 in mutations_v2 {
            assert_ne!(
                canonical_repo_map_source_bundle_digest_v2(&mutated_v2)
                    .expect("mutated source bundle digest"),
                expected_v2,
            );
        }
    }

    #[test]
    fn publish_request_refuses_duplicate_or_unknown_fields_v2() {
        let duplicate_v2 = serde_json::from_str::<RepoMapPublishBundleRequestV2>(
            r#"{"source_bundle_digest":"a","source_bundle_digest":"b"}"#,
        )
        .expect_err("duplicate digest field must fail");
        assert!(duplicate_v2.to_string().contains("duplicate field"));

        let unknown_v2 =
            serde_json::from_str::<RepoMapPublishBundleRequestV2>(r#"{"foreign":true}"#)
                .expect_err("unknown field must fail");
        assert!(unknown_v2.to_string().contains("unknown field"));
    }
}
