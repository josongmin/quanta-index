//! One canonical incremental IR shared by native SDKs and the application core.
//! Live leases and reclaim authority are intentionally absent from this DTO surface.
pub use quanta_index_contract_base::index_refs::*;
mod history;
mod immutable_object_wire;
mod legacy_repomap_import;
mod protected_source_routes;
mod retired_projection_policy_v1;
mod protected_native_snapshot;
pub use protected_native_snapshot::*;
mod source_restore;
mod native_source_restore;
pub use native_source_restore::{NativeImportedNativeSourceCompactV1, NativeImportedNativeSourceHeadV1, NativeSourceOriginCompactV1, NativeSourceOriginV1, NativeSourceSnapshotExportFrameV1, imported_native_source_manifest_digest_v1, imported_native_source_completed_digest_v1, native_source_target_manifest_digest_v1};
pub use source_restore::{
    LegacySourceManifestDigestV1, LegacySourceOriginV1, LexicalSourceHeadsCommitmentV1,
    NativeImportedSourceHeadV1, MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1,
    SourceSnapshotExportFrameV1, imported_legacy_source_completed_digest_v1,
    imported_legacy_source_manifest_digest_v1,
};
pub use legacy_repomap_import::{
    LegacyRepoMapImportPageV1, LegacyRepoMapImportV1,
    MAX_LEGACY_REPOMAP_IMPORT_PAGE_BYTES_V1, MAX_LEGACY_REPOMAP_IMPORT_PAGES_V1,
    MAX_LEGACY_REPOMAP_IMPORT_PAYLOAD_BYTES_V1,
};
pub use protected_source_routes::{
    MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1, MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1, MAX_NATIVE_PROTECTED_SOURCE_ROUTE_BYTES_V1,
    MAX_NATIVE_PROTECTED_VIEW_MARKER_BYTES_V1, NativeProtectedRepoMapHeadV1, NativeProtectedSourceHeadV1,
    NativeProtectedSourceRoutesV1, NativeProtectedNativeOriginV1, native_protected_view_marker_bytes_v1,
    native_protected_view_marker_ref_v1, native_protected_source_route_seal_key_v1,
};
pub use retired_projection_policy_v1::{
    MAX_RETIRED_PROJECTION_IDENTITIES_V1, MAX_RETIRED_PROJECTION_POLICY_BYTES_V1,
    RetiredProjectionIdentityV1, RetiredProjectionPolicyErrorV1, RetiredProjectionPolicyHeaderV1,
    RetiredProjectionPolicyRefV1, RetiredProjectionPolicyViewV1,
    decode_retired_projection_policy_v1, encode_retired_projection_policy_v1,
    preflight_retired_projection_policy_v1,
    retired_repomap_route_identity_v1,
};
pub(crate) use immutable_object_wire::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN;
pub use immutable_object_wire::{
    NATIVE_IMMUTABLE_OBJECT_HASH_DOMAIN_V1, NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1,
    NATIVE_IMMUTABLE_OBJECT_MAGIC_V1, NativeProtectedEncodingErrorV1,
    native_immutable_object_header_v1, native_immutable_object_ref_v1,
    native_immutable_object_ref_with_work_v1,
};
pub use history::{
    AuxiliaryGenerationKeyV1, HistoryAppendV1, HistoryGenerationAdvanceV1, HistoryGenerationInputV1, HistoryReplayFenceV1, HistoryReplayPublicationV1,
    NativeHistoryRestoreInputV1,
    HistorySnapshotCheckpointV1, HistorySnapshotExportFrameV1, HistorySnapshotPageV1, HistorySnapshotRowV1,
    HistorySourceCutV1, HistorySourceInputV1, HistoryTransition, SourceHistoryTransitionV1,
    MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1, MAX_HISTORY_SNAPSHOT_ROWS_PER_PAGE_V1,
    MAX_HISTORY_SOURCE_INPUT_BYTES_V1,
};

/// Source path byte limit, leaving five bytes for the canonical path map key
/// namespace within the native 4096-byte map key format.
pub const MAX_SOURCE_PATH_BYTES: usize = 4091;
/// Complete immutable wire bound, including header and ordered child references.
pub const MAX_NATIVE_IMMUTABLE_OBJECT_WIRE_BYTES_V1: u64 = 64 * 1024 * 1024;
/// Maximum number of source items in one Native `StageSources` request.
pub const MAX_NATIVE_STAGE_SOURCES_INPUTS: usize = 128;
/// Maximum sum of path, source, repository ID and language bytes (when present),
/// plus 256 bytes per staged item.
/// The IPC frame and encoded object batch apply independent byte limits.
pub const MAX_NATIVE_STAGE_SOURCES_WEIGHTED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MapRoot(pub Option<ObjectRef>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceCut {
    pub owners: MapRoot,
    pub paths: MapRoot,
    pub frontiers: MapRoot,
}

impl SourceCut {
    /// Canonical source-local input context, preserving the Kernel's framing.
    /// This fingerprints all three roots; it grants no source or writer authority.
    /// The fixed-field wire is counted and hashed without allocating a buffer.
    pub fn input_context_digest(&self) -> Result<Digest, serde_json::Error> {
        use sha2::{Digest as _, Sha256};

        struct ContextWriter<'a> {
            bytes: u64,
            hash: Option<&'a mut Sha256>,
        }

        impl std::io::Write for ContextWriter<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                let len = u64::try_from(bytes.len())
                    .map_err(|_| std::io::Error::other("source context length exceeds u64"))?;
                self.bytes = self
                    .bytes
                    .checked_add(len)
                    .ok_or_else(|| std::io::Error::other("source context length overflow"))?;
                if let Some(hash) = self.hash.as_mut() {
                    hash.update(bytes);
                }
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        const DOMAIN: &[u8; 14] = b"source-context";
        let mut count = ContextWriter {
            bytes: 0,
            hash: None,
        };
        serde_json::to_writer(&mut count, self)?;
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-vnext\0");
        hash.update(14_u64.to_be_bytes());
        hash.update(DOMAIN);
        hash.update(count.bytes.to_be_bytes());
        serde_json::to_writer(
            ContextWriter {
                bytes: 0,
                hash: Some(&mut hash),
            },
            self,
        )?;
        Ok(Digest(hash.finalize().into()))
    }
}

#[cfg(test)]
#[path = "index/source_context_tests.rs"]
mod source_context_tests;

/// Explicit producer admission for lexical text. Raw-byte and projection
/// consumers retain their separate input policies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceTextAdmission {
    Excluded,
    Utf8 { policy: Digest },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRecord {
    pub path: String,
    pub bytes: ObjectRef,
    pub byte_len: u64,
    pub text: SourceTextAdmission,
    /// Absent on records from the original path-only publication protocol.
    pub source_repo_id: Option<crate::RepoId>,
    /// Absent on records staged before authenticated language publication.
    pub language: Option<crate::lex::LanguageCode>,
}

impl serde::Serialize for SourceRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let fields = 4_usize
            .saturating_add(usize::from(self.source_repo_id.is_some()))
            .saturating_add(usize::from(self.language.is_some()));
        let mut state = serializer.serialize_struct("SourceRecord", fields)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "path", &self.path)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "bytes", &self.bytes)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "byte_len", &self.byte_len)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "text", &self.text)?;
        if self.source_repo_id.is_some() {
            serde::ser::SerializeStruct::serialize_field(
                &mut state,
                "source_repo_id",
                &self.source_repo_id,
            )?;
        }
        if self.language.is_some() {
            serde::ser::SerializeStruct::serialize_field(&mut state, "language", &self.language)?;
        }
        serde::ser::SerializeStruct::end(state)
    }
}

impl<'de> serde::Deserialize<'de> for SourceRecord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &[
            "path",
            "bytes",
            "byte_len",
            "text",
            "source_repo_id",
            "language",
        ];
        struct SourceRecordVisitor;
        impl<'de> serde::de::Visitor<'de> for SourceRecordVisitor {
            type Value = SourceRecord;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a closed source record with optional source identity")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let (mut path, mut bytes, mut byte_len, mut text) = (None, None, None, None);
                let (mut source_repo_id, mut language) = (None, None);
                while let Some(key) = map.next_key_seed(
                    quanta_index_contract_base::ClosedFieldKeySeedV1(FIELDS).serde_identifier(),
                )? {
                    match key {
                        "path" => {
                            if path.is_some() {
                                return Err(serde::de::Error::duplicate_field("path"));
                            }
                            path = Some(map.next_value::<String>()?);
                        }
                        "bytes" => {
                            if bytes.is_some() {
                                return Err(serde::de::Error::duplicate_field("bytes"));
                            }
                            bytes = Some(map.next_value::<ObjectRef>()?);
                        }
                        "byte_len" => {
                            if byte_len.is_some() {
                                return Err(serde::de::Error::duplicate_field("byte_len"));
                            }
                            byte_len = Some(map.next_value::<u64>()?);
                        }
                        "text" => {
                            if text.is_some() {
                                return Err(serde::de::Error::duplicate_field("text"));
                            }
                            text = Some(map.next_value::<SourceTextAdmission>()?);
                        }
                        "source_repo_id" => {
                            if source_repo_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("source_repo_id"));
                            }
                            source_repo_id = Some(map.next_value::<Option<crate::RepoId>>()?);
                        }
                        "language" => {
                            if language.is_some() {
                                return Err(serde::de::Error::duplicate_field("language"));
                            }
                            language = Some(map.next_value::<Option<crate::lex::LanguageCode>>()?);
                        }
                        _ => unreachable!("closed field seed only returns schema keys"),
                    }
                }
                Ok(SourceRecord {
                    path: path.ok_or_else(|| serde::de::Error::missing_field("path"))?,
                    bytes: bytes.ok_or_else(|| serde::de::Error::missing_field("bytes"))?,
                    byte_len: byte_len
                        .ok_or_else(|| serde::de::Error::missing_field("byte_len"))?,
                    text: text.ok_or_else(|| serde::de::Error::missing_field("text"))?,
                    source_repo_id: source_repo_id.flatten(),
                    language: language.flatten(),
                })
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let path = seq
                    .next_element::<String>()?
                    .ok_or_else(|| serde::de::Error::missing_field("path"))?;
                let bytes = seq
                    .next_element::<ObjectRef>()?
                    .ok_or_else(|| serde::de::Error::missing_field("bytes"))?;
                let byte_len = seq
                    .next_element::<u64>()?
                    .ok_or_else(|| serde::de::Error::missing_field("byte_len"))?;
                let text = seq
                    .next_element::<SourceTextAdmission>()?
                    .ok_or_else(|| serde::de::Error::missing_field("text"))?;
                let source_repo_id = seq.next_element::<Option<crate::RepoId>>()?.flatten();
                let language = seq
                    .next_element::<Option<crate::lex::LanguageCode>>()?
                    .flatten();
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(7, &self));
                }
                Ok(SourceRecord {
                    path,
                    bytes,
                    byte_len,
                    text,
                    source_repo_id,
                    language,
                })
            }
        }
        deserializer.deserialize_struct("SourceRecord", FIELDS, SourceRecordVisitor)
    }
}

#[cfg(test)]
mod source_record_codec_tests {
    use super::{Digest, ObjectRef, SourceRecord, SourceTextAdmission};

    fn legacy_record() -> SourceRecord {
        SourceRecord {
            path: "a.rs".into(),
            bytes: ObjectRef(Digest([8; 32])),
            byte_len: 6,
            text: SourceTextAdmission::Excluded,
            source_repo_id: None,
            language: None,
        }
    }

    #[test]
    fn legacy_optional_source_fields_keep_named_and_positional_wire() {
        let record = legacy_record();
        let json = serde_json::to_string(&record).expect("source record JSON");
        assert!(!json.contains("source_repo_id"));
        assert!(!json.contains("language"));
        assert_eq!(
            serde_json::from_str::<SourceRecord>(&json).expect("map"),
            record
        );

        let positional = serde_json::json!(["a.rs", record.bytes, 6, record.text]);
        assert_eq!(
            serde_json::from_value::<SourceRecord>(positional.clone()).expect("short sequence"),
            record
        );
        let mut with_null = positional.as_array().expect("sequence").clone();
        with_null.push(serde_json::Value::Null);
        assert_eq!(
            serde_json::from_value::<SourceRecord>(serde_json::Value::Array(with_null.clone()))
                .expect("explicit nullable source identity"),
            record
        );
        with_null.push(serde_json::Value::Null);
        assert_eq!(
            serde_json::from_value::<SourceRecord>(serde_json::Value::Array(with_null.clone()))
                .expect("explicit nullable language"),
            record
        );
        with_null.push(serde_json::Value::Null);
        assert!(
            serde_json::from_value::<SourceRecord>(serde_json::Value::Array(with_null)).is_err()
        );

        let mut scoped = record.clone();
        scoped.source_repo_id = Some(crate::RepoId::new("corp-a").expect("repo"));
        scoped.language = Some(crate::lex::LanguageCode::new("rust").expect("language"));
        let scoped_json = serde_json::to_string(&scoped).expect("scoped JSON");
        assert!(
            scoped_json.find("source_repo_id").expect("source identity")
                < scoped_json.find("language").expect("language")
        );
        assert_eq!(
            serde_json::from_str::<SourceRecord>(&scoped_json).expect("scoped map"),
            scoped
        );

        let unknown = json.replacen("\"byte_len\":", "\"unknown\":0,\"byte_len\":", 1);
        let duplicate = json.replacen("\"byte_len\":", "\"byte_len\":6,\"byte_len\":", 1);
        assert!(serde_json::from_str::<SourceRecord>(&unknown).is_err());
        assert!(serde_json::from_str::<SourceRecord>(&duplicate).is_err());
    }

    #[test]
    fn source_record_cbor_retains_order_and_known_byte_key_compatibility() {
        let record = legacy_record();
        let mut value = ciborium::value::Value::serialized(&record).expect("source record CBOR");
        let ciborium::value::Value::Map(fields) = &mut value else {
            panic!("source record must be a CBOR map");
        };
        let names = fields
            .iter()
            .map(|(key, _)| match key {
                ciborium::value::Value::Text(name) => name.as_str(),
                _ => panic!("source record field key must be text"),
            })
            .collect::<Vec<_>>();
        assert_eq!(names, ["path", "bytes", "byte_len", "text"]);
        let (key, _) = fields
            .iter_mut()
            .find(|(key, _)| matches!(key, ciborium::value::Value::Text(name) if name == "path"))
            .expect("path key");
        *key = ciborium::value::Value::Bytes(b"path".to_vec());
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&value, &mut cbor).expect("byte-key CBOR");
        assert_eq!(
            ciborium::de::from_reader::<SourceRecord, _>(cbor.as_slice()).expect("known byte key"),
            record
        );
    }
}

/// One bounded native source intake item. A batch preserves this order in its
/// returned source-record references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceInput {
    pub path: String,
    pub bytes: Vec<u8>,
    pub text: SourceTextAdmission,
    pub language: Option<crate::lex::LanguageCode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerMutation {
    pub owner: OwnerKey,
    pub expected: Option<ObjectRef>,
    pub next: Option<ObjectRef>,
}

impl OwnerMutation {
    /// An expected stamp is historical CAS metadata, never a physical edge.
    #[must_use]
    pub fn object_children(&self) -> Vec<ObjectRef> {
        self.next.into_iter().collect()
    }
}

/// A complete, sorted owner-key mutation map. `digest` commits to the
/// canonical item stream, including each owner's expected and next stamp,
/// independently of map page layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerMutationSet {
    pub root: MapRoot,
    pub count: u64,
    pub digest: Digest,
}

impl OwnerMutationSet {
    #[must_use]
    pub fn object_children(self) -> Vec<ObjectRef> {
        self.root.0.into_iter().collect()
    }
}

/// Durable pre-admission input custody. The catalog owns this fence; callers
/// cannot turn an ingress into an operation attempt or publisher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IngressRef {
    pub operation: OperationKey,
    pub fence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceEvent {
    pub producer: ProducerId,
    pub stream: StreamId,
    pub stream_epoch: u64,
    pub predecessor: u64,
    pub sequence: u64,
    pub payload: Digest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTransition {
    pub expected_source: SourceCut,
    pub owners: OwnerMutationSet,
    pub events: Vec<SourceEvent>,
}

// Keep the persisted catalog epoch and filesystem marker in one declaration.
macro_rules! native_index_state_format {
    ($version:literal) => {
        /// Persisted native index state format, shared by the catalog and runtime.
        pub const NATIVE_INDEX_STATE_VERSION: u32 = $version;
        pub const NATIVE_INDEX_STATE_MARKER_NAME: &str =
            concat!(".native-index-v", stringify!($version));
        pub const NATIVE_INDEX_STATE_MARKER_CONTENT: &[u8] =
            concat!("quanta-index-native-state-v", stringify!($version), "\n").as_bytes();
    };
}
native_index_state_format!(20);
/// Persisted native source-event capture payload format.
pub const NATIVE_SOURCE_CAPTURE_VERSION: u32 = 2;
/// Hash domain for logical owner mutation capture, independent of execution base.
pub const NATIVE_SOURCE_CAPTURE_HASH_DOMAIN: &[u8] = b"quanta-index-native-source-capture-v2\0";

impl SourceTransition {
    /// Logical capture identity for immutable source-event claims. The count
    /// and digest commit to the canonical mutation stream, including each
    /// owner's expected and next stamp. The execution `SourceCut`, physical map
    /// root and event positions are excluded so a refused operation can retry
    /// the same capture against a new exact execution base.
    pub fn owner_digest(&self) -> Result<Digest, serde_json::Error> {
        use sha2::{Digest as _, Sha256};
        struct CaptureWriter(Sha256);
        impl std::io::Write for CaptureWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = CaptureWriter(Sha256::new());
        writer.0.update(NATIVE_SOURCE_CAPTURE_HASH_DOMAIN);
        // Preserve the canonical encoder and byte sequence without creating
        // an intermediate owned JSON buffer in producer/replay validation.
        serde_json::to_writer(&mut writer, &(self.owners.count, self.owners.digest))?;
        Ok(Digest(writer.0.finalize().into()))
    }
}

#[cfg(test)]
mod source_capture_tests {
    use super::{
        Digest, MapRoot, NATIVE_INDEX_STATE_VERSION, NATIVE_SOURCE_CAPTURE_HASH_DOMAIN,
        NATIVE_SOURCE_CAPTURE_VERSION, ObjectRef, OwnerMutationSet, SourceCut, SourceTransition,
    };

    #[test]
    fn source_capture_v2_has_independent_golden_and_exact_logical_inputs() {
        // Independently computed as SHA-256(domain || JSON([2, [165; 32]])).
        const GOLDEN: [u8; 32] = [
            0x6e, 0x7d, 0x2e, 0xa9, 0xaa, 0xec, 0xd9, 0x46, 0x23, 0xb7, 0xbb, 0x2e, 0x4b, 0x46,
            0x40, 0x8b, 0x26, 0xa6, 0xa1, 0xca, 0xa5, 0xab, 0x69, 0xcf, 0xb6, 0xd0, 0x1d, 0xc1,
            0xbe, 0x88, 0x40, 0xe2,
        ];
        assert_eq!(NATIVE_INDEX_STATE_VERSION, 20);
        assert_eq!(super::NATIVE_INDEX_STATE_MARKER_NAME, ".native-index-v20");
        assert_eq!(
            super::NATIVE_INDEX_STATE_MARKER_CONTENT,
            b"quanta-index-native-state-v20\n"
        );
        assert_eq!(NATIVE_SOURCE_CAPTURE_VERSION, 2);
        assert_eq!(
            NATIVE_SOURCE_CAPTURE_HASH_DOMAIN,
            b"quanta-index-native-source-capture-v2\0"
        );
        let mut transition = SourceTransition {
            expected_source: SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            owners: OwnerMutationSet {
                root: MapRoot(None),
                count: 2,
                digest: Digest([165; 32]),
            },
            events: Vec::new(),
        };
        let captured = transition.owner_digest().map_err(|error| error.to_string());
        assert_eq!(captured, Ok(Digest(GOLDEN)));

        transition.expected_source.owners = MapRoot(Some(ObjectRef(Digest([1; 32]))));
        transition.expected_source.paths = MapRoot(Some(ObjectRef(Digest([2; 32]))));
        transition.expected_source.frontiers = MapRoot(Some(ObjectRef(Digest([3; 32]))));
        transition.owners.root = MapRoot(Some(ObjectRef(Digest([4; 32]))));
        assert_eq!(
            transition.owner_digest().map_err(|error| error.to_string()),
            captured
        );

        transition.owners.count = 3;
        let changed_count = transition.owner_digest().map_err(|error| error.to_string());
        assert!(changed_count.is_ok());
        assert_ne!(changed_count, captured);
        transition.owners.count = 2;
        transition.owners.digest = Digest([166; 32]);
        let changed_digest = transition.owner_digest().map_err(|error| error.to_string());
        assert!(changed_digest.is_ok());
        assert_ne!(changed_digest, captured);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputBinding {
    pub root: MapRoot,
    pub schema: SchemaId,
    pub policy: Digest,
    pub context: Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverageStatus {
    Pending { previous_missing: MapRoot },
    Partial { missing: MapRoot },
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoverageBinding {
    pub instance: IndexInstanceId,
    pub desired: InputBinding,
    pub covered: Option<InputBinding>,
    pub searchable: Option<ObjectRef>,
    pub status: CoverageStatus,
    /// Owner-local desired/covered state. Missing owners are rooted separately
    /// so one completion never scans every owner to establish completeness.
    pub owners: MapRoot,
}

/// Per-owner materialization identity. Admission limits are excluded from this
/// binding; output-affecting recipe/model/producer context are included.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerInputBinding {
    pub source: Option<ObjectRef>,
    pub projection: Option<ObjectRef>,
    pub producer_context: Digest,
    pub derivation_policy: Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerCoverage {
    pub owner: OwnerKey,
    pub desired: OwnerInputBinding,
    pub covered: Option<OwnerInputBinding>,
    pub searchable: Option<ObjectRef>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectionBinding {
    pub projection: ProjectionId,
    pub producer: ProducerId,
    pub owners: MapRoot,
    pub source: SourceCut,
    pub schema: SchemaId,
    pub producer_policy: Digest,
    pub dependency_context: Digest,
    pub certificate: ObjectRef,
}

impl ProjectionBinding {
    /// Global projection context; owner reuse still uses each contribution stamp.
    #[must_use]
    pub fn materialization_context(&self) -> Digest {
        use sha2::{Digest as _, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-projection-context-v1\0");
        hash.update(self.projection.0);
        hash.update(self.producer.0);
        hash.update(self.schema.0);
        hash.update(self.producer_policy.0);
        hash.update(self.dependency_context.0);
        Digest(hash.finalize().into())
    }

    /// Only current dependencies are physical children. Historical stamps are
    /// checked against a pinned base and must not retain an ancestor chain.
    #[must_use]
    pub fn object_children(self) -> Vec<ObjectRef> {
        let mut children = vec![self.certificate];
        children.extend(self.owners.0);
        children.extend(source_object_children(self.source));
        canonical_object_children(children)
    }
}

/// One complete producer-owned contribution. A present envelope with no output
/// is a certified empty result; failed or skipped owners have no envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectionOwnerContribution {
    pub projection: ProjectionId,
    pub producer: ProducerId,
    pub owner: OwnerKey,
    /// Canonical source owner when the contribution is tied to a source file.
    pub source_owner: Option<OwnerKey>,
    pub source: Option<ObjectRef>,
    pub dependency_stamp: Digest,
    pub output: Option<ObjectRef>,
}

impl ProjectionOwnerContribution {
    /// Stable owner context shared by publication and adapter verification.
    #[must_use]
    pub fn materialization_context(&self) -> Digest {
        use sha2::{Digest as _, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-projection-owner-context-v1\0");
        hash.update(self.projection.0);
        hash.update(self.producer.0);
        hash.update(self.dependency_stamp.0);
        Digest(hash.finalize().into())
    }

    #[must_use]
    pub fn object_children(self) -> Vec<ObjectRef> {
        canonical_object_children([self.source, self.output].into_iter().flatten().collect())
    }
}

/// Producer attestation for the entire affected owner set, including owners
/// whose source bytes did not change. Index validates binding, not truthfulness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionCertificate {
    pub projection: ProjectionId,
    pub producer: ProducerId,
    pub prior: Option<ProjectionBinding>,
    pub target_source: SourceCut,
    pub schema: SchemaId,
    pub producer_policy: Digest,
    pub dependency_context: Digest,
    pub affected: OwnerMutationSet,
}

impl ProjectionCertificate {
    /// `prior` is a comparison stamp, not live custody of historical roots.
    #[must_use]
    pub fn object_children(&self) -> Vec<ObjectRef> {
        let mut children = source_object_children(self.target_source);
        children.extend(self.affected.object_children());
        canonical_object_children(children)
    }
}

fn source_object_children(source: SourceCut) -> Vec<ObjectRef> {
    canonical_object_children(
        [source.owners.0, source.paths.0, source.frontiers.0]
            .into_iter()
            .flatten()
            .collect(),
    )
}

fn canonical_object_children(mut children: Vec<ObjectRef>) -> Vec<ObjectRef> {
    children.sort_unstable();
    children.dedup();
    children
}

/// The producer owns affected-set completeness. A missing attestation never
/// becomes an empty successful replacement or evidence of unaffected owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProducerCompleteness {
    CompleteAffectedDomain { certificate: ObjectRef },
    Partial { missing: MapRoot },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionTransition {
    pub projection: ProjectionId,
    pub producer: ProducerId,
    pub expected: Option<ProjectionBinding>,
    pub target_source: SourceCut,
    pub schema: SchemaId,
    pub producer_policy: Digest,
    pub dependency_context: Digest,
    pub completeness: ProducerCompleteness,
    /// Complete replacement per affected owner; omission inherits other owners.
    pub owners: OwnerMutationSet,
    /// Separate producer stream frontiers; every payload binds this transition.
    pub events: Vec<SourceEvent>,
}

impl ProjectionTransition {
    pub fn payload_digest(&self) -> Result<Digest, serde_json::Error> {
        match self.payload_digest_with_work_v3(&mut |_| Ok::<(), core::convert::Infallible>(())) {
            Ok(digest) => Ok(digest),
            Err(ProjectionPayloadDigestErrorV3::Encode(cause)) => Err(cause),
            Err(ProjectionPayloadDigestErrorV3::Arithmetic) => {
                Err(<serde_json::Error as serde::ser::Error>::custom(
                    "projection payload JSON length overflow",
                ))
            }
            Err(ProjectionPayloadDigestErrorV3::Work(never)) => match never {},
        }
    }

    /// Hash the canonical event payload without allocating an encoded copy.
    /// The callback pays before every JSON chunk and the fixed hash domain.
    pub fn payload_digest_with_work_v3<E>(
        &self,
        work: &mut impl FnMut(usize) -> Result<(), E>,
    ) -> Result<Digest, ProjectionPayloadDigestErrorV3<E>> {
        use sha2::{Digest as _, Sha256};
        const DOMAIN: &[u8] = b"quanta-index-projection-transition-v1\0";
        let payload = (
            self.projection,
            self.producer,
            &self.expected,
            self.target_source,
            self.schema,
            self.producer_policy,
            self.dependency_context,
            self.completeness,
            &self.owners,
        );
        let mut hash = Sha256::new();
        work(DOMAIN.len()).map_err(ProjectionPayloadDigestErrorV3::Work)?;
        hash.update(DOMAIN);
        emit_paid_identity_json_v3(&payload, ViewDigestSinkV3::Hash(&mut hash), work).map_err(
            |cause| match cause {
                CatalogViewDigestErrorV3::Work(cause) => {
                    ProjectionPayloadDigestErrorV3::Work(cause)
                }
                CatalogViewDigestErrorV3::Arithmetic => ProjectionPayloadDigestErrorV3::Arithmetic,
                CatalogViewDigestErrorV3::Encode(cause) => {
                    ProjectionPayloadDigestErrorV3::Encode(cause)
                }
            },
        )?;
        Ok(Digest(hash.finalize().into()))
    }
}

#[derive(Debug)]
pub enum ProjectionPayloadDigestErrorV3<E> {
    Work(E),
    Arithmetic,
    Encode(serde_json::Error),
}

impl<E: core::fmt::Display> core::fmt::Display for ProjectionPayloadDigestErrorV3<E> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Work(cause) => write!(formatter, "work refused: {cause}"),
            Self::Arithmetic => formatter.write_str("digest length overflow"),
            Self::Encode(cause) => write!(formatter, "digest encoding failed: {cause}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ProjectionPayloadDigestErrorV3<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Work(cause) => Some(cause),
            Self::Arithmetic => None,
            Self::Encode(cause) => Some(cause),
        }
    }
}

#[cfg(test)]
#[path = "projection_payload_digest_tests_v3.rs"]
mod projection_payload_digest_tests_v3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkScope {
    Owner {
        owner: OwnerKey,
        expected: OwnerInputBinding,
    },
    ProjectionDomain {
        projection: ProjectionId,
        source: SourceCut,
        producer_policy: Digest,
        dependency_context: Digest,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkIntent {
    pub workspace: WorkspaceKey,
    pub epoch: u64,
    pub instance: IndexInstanceId,
    pub scope: WorkScope,
}

impl WorkIntent {
    pub fn identity_digest(&self) -> Result<Digest, serde_json::Error> {
        use sha2::{Digest as _, Sha256};
        struct HashWriter<'a>(&'a mut Sha256);
        impl std::io::Write for HashWriter<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-owner-work-v1\0");
        serde_json::to_writer(HashWriter(&mut hash), self)?;
        Ok(Digest(hash.finalize().into()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkClaim {
    pub intent: WorkIntent,
    pub fence: u64,
    /// Server catalog wall-clock lease; never inferred from a view sequence.
    pub lease_until_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkStatus {
    Unknown,
    Pending,
    Claimed(WorkClaim),
    Completed(ViewRef),
    Superseded,
    Blocked(Refusal),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerMemberBinding {
    pub instance: IndexInstanceId,
    pub owner: OwnerKey,
    pub input: OwnerInputBinding,
    pub format: StorageFormatId,
    pub artifact: ObjectRef,
    pub logical_commitment: Digest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberBinding {
    pub instance: IndexInstanceId,
    pub format: StorageFormatId,
    pub artifact: ObjectRef,
    pub input: InputBinding,
    pub logical_commitment: Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupActivation {
    CoreRequired,
    AsyncCoverage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceSpec {
    pub instance: IndexInstanceId,
    pub kind: IndexKind,
    pub group: PublicationGroupId,
    pub activation: GroupActivation,
    pub input_domain: InputDomain,
    pub schema: SchemaId,
    pub policy: Digest,
    pub format: StorageFormatId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputDomain {
    SourceLocal,
    History,
    Projection {
        projection: ProjectionId,
        producer: ProducerId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexProfile {
    /// Bounded configuration, not corpus-sized membership.
    pub instances: Vec<InstanceSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogView {
    pub identity: ViewRef,
    pub profile: ObjectRef,
    pub source: SourceCut,
    pub projections: MapRoot,
    pub members: MapRoot,
    /// Desired/covered/searchable state for async instances is independently rooted.
    pub coverage: MapRoot,
}

impl CatalogView {
    /// Fixed canonical field order, excluding this digest's own value.
    pub fn identity_digest(&self) -> Result<Digest, serde_json::Error> {
        match self.identity_digest_with_work_v3(&mut |_| Ok::<(), core::convert::Infallible>(())) {
            Ok(digest) => Ok(digest),
            Err(CatalogViewDigestErrorV3::Encode(cause)) => Err(cause),
            Err(CatalogViewDigestErrorV3::Arithmetic) => {
                Err(<serde_json::Error as serde::ser::Error>::custom(
                    "catalog view JSON length overflow",
                ))
            }
            Err(CatalogViewDigestErrorV3::Work(never)) => match never {},
        }
    }

    /// Stream the same canonical JSON identity without cloning the view or
    /// building an unadmitted Vec. The callback pays for both serialization
    /// passes and the fixed domain/length commitment bytes.
    pub fn identity_digest_with_work_v3<E>(
        &self,
        work: &mut impl FnMut(usize) -> Result<(), E>,
    ) -> Result<Digest, CatalogViewDigestErrorV3<E>> {
        use sha2::{Digest as _, Sha256};
        const DOMAIN: &[u8] = b"quanta-index-vnext\0";
        const LABEL: &[u8] = b"catalog-view";

        let view = CatalogViewIdentityRefV3(self);
        let mut encoded_len = 0_u64;
        emit_paid_identity_json_v3(&view, ViewDigestSinkV3::Count(&mut encoded_len), work)?;
        let mut hash = Sha256::new();
        let commitment_len = DOMAIN
            .len()
            .checked_add(8)
            .and_then(|len| len.checked_add(LABEL.len()))
            .and_then(|len| len.checked_add(8))
            .ok_or(CatalogViewDigestErrorV3::Arithmetic)?;
        work(commitment_len).map_err(CatalogViewDigestErrorV3::Work)?;
        hash.update(DOMAIN);
        hash.update(12_u64.to_be_bytes());
        hash.update(LABEL);
        hash.update(encoded_len.to_be_bytes());
        emit_paid_identity_json_v3(&view, ViewDigestSinkV3::Hash(&mut hash), work)?;
        Ok(Digest(hash.finalize().into()))
    }
}

#[derive(Debug)]
pub enum CatalogViewDigestErrorV3<E> {
    Work(E),
    Arithmetic,
    Encode(serde_json::Error),
}

impl<E: core::fmt::Display> core::fmt::Display for CatalogViewDigestErrorV3<E> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Work(cause) => write!(formatter, "work refused: {cause}"),
            Self::Arithmetic => formatter.write_str("digest length overflow"),
            Self::Encode(cause) => write!(formatter, "digest encoding failed: {cause}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for CatalogViewDigestErrorV3<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Work(cause) => Some(cause),
            Self::Arithmetic => None,
            Self::Encode(cause) => Some(cause),
        }
    }
}

struct CatalogViewIdentityRefV3<'a>(&'a CatalogView);

impl serde::Serialize for CatalogViewIdentityRefV3<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut identity = self.0.identity;
        identity.digest = Digest([0; 32]);
        let mut state = serializer.serialize_struct("CatalogView", 6)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "identity", &identity)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "profile", &self.0.profile)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "source", &self.0.source)?;
        serde::ser::SerializeStruct::serialize_field(
            &mut state,
            "projections",
            &self.0.projections,
        )?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "members", &self.0.members)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "coverage", &self.0.coverage)?;
        serde::ser::SerializeStruct::end(state)
    }
}

enum ViewDigestSinkV3<'a> {
    Count(&'a mut u64),
    Hash(&'a mut sha2::Sha256),
}

struct ViewDigestWriterV3<'a, E, F> {
    sink: ViewDigestSinkV3<'a>,
    work: &'a mut F,
    first_work_refusal: &'a mut Option<E>,
    overflow: &'a mut bool,
}

impl<E, F: FnMut(usize) -> Result<(), E>> std::io::Write for ViewDigestWriterV3<'_, E, F> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Err(cause) = (self.work)(bytes.len()) {
            *self.first_work_refusal = Some(cause);
            return Err(std::io::ErrorKind::Other.into());
        }
        match &mut self.sink {
            ViewDigestSinkV3::Count(count) => {
                let len = match u64::try_from(bytes.len()) {
                    Ok(len) => len,
                    Err(_overflow) => {
                        *self.overflow = true;
                        return Err(std::io::ErrorKind::InvalidData.into());
                    }
                };
                let Some(next) = count.checked_add(len) else {
                    *self.overflow = true;
                    return Err(std::io::ErrorKind::InvalidData.into());
                };
                **count = next;
            }
            ViewDigestSinkV3::Hash(hash) => {
                use sha2::Digest as _;
                hash.update(bytes);
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn emit_paid_identity_json_v3<E, T: serde::Serialize>(
    view: &T,
    sink: ViewDigestSinkV3<'_>,
    work: &mut impl FnMut(usize) -> Result<(), E>,
) -> Result<(), CatalogViewDigestErrorV3<E>> {
    let mut first_work_refusal = None;
    let mut overflow = false;
    let result = serde_json::to_writer(
        ViewDigestWriterV3 {
            sink,
            work,
            first_work_refusal: &mut first_work_refusal,
            overflow: &mut overflow,
        },
        view,
    );
    if let Some(cause) = first_work_refusal {
        return Err(CatalogViewDigestErrorV3::Work(cause));
    }
    if overflow {
        return Err(CatalogViewDigestErrorV3::Arithmetic);
    }
    result.map_err(CatalogViewDigestErrorV3::Encode)
}

#[cfg(test)]
mod catalog_view_digest_tests_v3 {
    use super::*;
    use sha2::{Digest as _, Sha256};

    fn sample_view() -> CatalogView {
        CatalogView {
            identity: ViewRef {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                epoch: 7,
                sequence: 11,
                digest: Digest([9; 32]),
            },
            profile: ObjectRef(Digest([5; 32])),
            source: SourceCut {
                owners: MapRoot(Some(ObjectRef(Digest([6; 32])))),
                paths: MapRoot(None),
                frontiers: MapRoot(Some(ObjectRef(Digest([7; 32])))),
            },
            projections: MapRoot(None),
            members: MapRoot(Some(ObjectRef(Digest([8; 32])))),
            coverage: MapRoot(None),
        }
    }

    #[test]
    fn borrowed_catalog_digest_matches_original_owned_json_commitment() {
        let view = sample_view();
        let mut old = view.clone();
        old.identity.digest = Digest([0; 32]);
        let bytes = serde_json::to_vec(&old).expect("original catalog JSON");
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-vnext\0");
        hash.update(12_u64.to_be_bytes());
        hash.update(b"catalog-view");
        hash.update(
            u64::try_from(bytes.len())
                .expect("bounded test JSON")
                .to_be_bytes(),
        );
        hash.update(&bytes);
        let expected = Digest(hash.finalize().into());

        let mut charged = 0_usize;
        let actual = view
            .identity_digest_with_work_v3(&mut |bytes| {
                charged += bytes;
                Ok::<_, &'static str>(())
            })
            .expect("borrowed digest");
        assert_eq!(actual, expected);
        assert_eq!(view.identity_digest().expect("ordinary facade"), expected);
        let commitment_bytes = b"quanta-index-vnext\0"
            .len()
            .checked_add(16)
            .and_then(|bytes| bytes.checked_add(b"catalog-view".len()))
            .expect("bounded commitment");
        let expected_work = bytes
            .len()
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(commitment_bytes))
            .expect("bounded fixture work");
        assert_eq!(charged, expected_work);
    }

    #[test]
    fn borrowed_catalog_digest_retains_typed_work_refusal() {
        let cause = sample_view()
            .identity_digest_with_work_v3(&mut |_| Err::<(), _>("work cap"))
            .expect_err("first byte must refuse");
        assert!(matches!(cause, CatalogViewDigestErrorV3::Work("work cap")));
    }

    #[test]
    fn closed_nested_catalog_keys_preserve_json_and_cbor_shape() {
        let view = sample_view();
        let json = serde_json::to_vec(&view).expect("catalog JSON");
        assert_eq!(
            serde_json::from_slice::<CatalogView>(&json).expect("decode"),
            view
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&view, &mut cbor).expect("catalog CBOR");
        assert_eq!(
            ciborium::de::from_reader::<CatalogView, _>(cbor.as_slice()).expect("decode"),
            view
        );
        let invalid = String::from_utf8(json)
            .expect("JSON is UTF-8")
            .replace("\"frontiers\":", "\"unexpected\":");
        assert!(serde_json::from_str::<CatalogView>(&invalid).is_err());
    }

    #[test]
    fn operation_reply_keys_preserve_duplicate_missing_and_unknown_refusal() {
        let operation = OperationKey {
            workspace: sample_view().identity.workspace,
            epoch: 7,
            producer: ProducerId([8; 16]),
            stream: StreamId([9; 16]),
            stream_epoch: 10,
            sequence: 11,
        };
        let reply = OperationReply {
            operation,
            payload: 17_u64,
        };
        let json = serde_json::to_string(&reply).expect("reply JSON");
        assert_eq!(
            serde_json::from_str::<OperationReply<u64>>(&json).expect("decode"),
            reply
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&reply, &mut cbor).expect("reply CBOR");
        assert_eq!(
            ciborium::de::from_reader::<OperationReply<u64>, _>(cbor.as_slice()).expect("decode"),
            reply
        );
        for invalid in [
            json.replace("\"payload\":17", "\"payload\":17,\"payload\":18"),
            json.replace(",\"payload\":17", ""),
            json.replace("\"payload\":17", "\"other\":17"),
        ] {
            assert!(serde_json::from_str::<OperationReply<u64>>(&invalid).is_err());
        }
    }
}

/// Immutable source-authority intent, persisted with the original operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceOperationMode {
    Unchanged,
    Advance,
    Resync { prior_epoch: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateKind {
    SourceAdvance(SourceTransition),
    SourceHistoryAdvance(Box<SourceHistoryTransitionV1>),
    /// Complete replacement of the known producer owner prefix. The producer
    /// epoch switches only when the new baseline and original receipt publish.
    SourceResync {
        prior_epoch: u64,
        transition: SourceTransition,
    },
    /// Replace the producer's Source prefix and advance its History generation
    /// in the same Catalog publication.
    SourceHistoryResync {
        prior_epoch: u64,
        transition: Box<SourceHistoryTransitionV1>,
    },
    ProjectionAdvance(Box<ProjectionTransition>),
    HistoryAdvance(HistoryTransition),
    HistoryAppend(HistoryAppendV1),
    Reconfigure {
        profile: ObjectRef,
    },
    /// Repack one owner range of an exact published SourceLocal member.
    CompactSourceLocal {
        instance: IndexInstanceId,
        expected_artifact: ObjectRef,
        owner: OwnerKey,
    },
    /// Completion can rebase over unrelated source/view advances. The owner's
    /// exact desired input and live claim fence remain mandatory.
    Materialize {
        claim: WorkClaim,
    },
}

impl UpdateKind {
    #[must_use]
    pub fn source_transition_v1(&self) -> Option<&SourceTransition> {
        match self {
            Self::SourceAdvance(transition) | Self::SourceResync { transition, .. } => Some(transition),
            Self::SourceHistoryAdvance(transition)
            | Self::SourceHistoryResync { transition, .. } => Some(&transition.source),
            _ => None,
        }
    }

    #[must_use]
    pub fn history_generation_advance_v1(&self) -> Option<&HistoryGenerationAdvanceV1> {
        match self {
            Self::SourceHistoryAdvance(transition)
            | Self::SourceHistoryResync { transition, .. } => Some(&transition.history),
            _ => None,
        }
    }

    #[must_use]
    pub const fn source_mode(&self) -> SourceOperationMode {
        match self {
            Self::SourceAdvance(_) | Self::SourceHistoryAdvance(_) => SourceOperationMode::Advance,
            Self::SourceResync { prior_epoch, .. } | Self::SourceHistoryResync { prior_epoch, .. } => SourceOperationMode::Resync {
                prior_epoch: *prior_epoch,
            },
            Self::ProjectionAdvance(_)
            | Self::HistoryAdvance(_)
            | Self::HistoryAppend(_)
            | Self::Reconfigure { .. }
            | Self::CompactSourceLocal { .. }
            | Self::Materialize { .. } => SourceOperationMode::Unchanged,
        }
    }
}

#[cfg(test)]
mod source_mode_tests {
    use super::SourceOperationMode;

    #[test]
    fn source_mode_uses_a_closed_wire_contract() {
        let golden = r#"{"Resync":{"prior_epoch":7}}"#;
        let expected = SourceOperationMode::Resync { prior_epoch: 7 };
        assert_eq!(
            serde_json::from_str::<SourceOperationMode>(golden).map_err(|error| error.to_string()),
            Ok(expected)
        );
        assert_eq!(
            serde_json::to_string(&expected).map_err(|error| error.to_string()),
            Ok(golden.to_owned())
        );
        for invalid in [
            r#"{"Resync":{}}"#,
            r#"{"Resync":{"prior_epoch":7,"epoch":8}}"#,
            r#"{"Resync":{"prior_epoch":7,"prior_epoch":8}}"#,
            r#""Reset""#,
        ] {
            assert!(serde_json::from_str::<SourceOperationMode>(invalid).is_err());
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateRequest {
    pub operation: OperationKey,
    pub expected_view: ViewRef,
    pub kind: UpdateKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImmutableOperation {
    pub key: OperationKey,
    /// Digest of the complete canonical request; no self-referential digest field.
    pub request: ObjectRef,
    pub source_mode: SourceOperationMode,
}

/// The small immutable projection descriptor retained with an operation after
/// the physical request and its staging objects have been released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectionReplayStamp {
    pub expected_view: ViewRef,
    pub projection: ProjectionId,
    pub source: SourceCut,
    pub context: Digest,
}

quanta_index_contract_base::native_serde_struct!(ProjectionReplayStamp {
    expected_view: ViewRef,
    projection: ProjectionId,
    source: SourceCut,
    context: Digest,
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    Conflict,
    FenceLost,
    InvalidInput,
    ResourceLimited,
    BackendViolation,
    RebuildRequired,
    MaintenanceRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationStatus {
    Unknown,
    Pending(AttemptRef),
    Published(ViewRef),
    Refused(Refusal),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewSelection {
    Latest { workspace: WorkspaceKey, epoch: u64 },
    Exact(ViewRef),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubmitOutcome {
    Accepted(AttemptRef),
    Terminal(OperationStatus),
    Unknown(OperationKey),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationReply<T> {
    pub operation: OperationKey,
    pub payload: T,
}

// Closed manual codecs preserve the named JSON/CBOR representation.
quanta_index_contract_base::native_serde_newtype!(MapRoot, Option<ObjectRef>);
quanta_index_contract_base::native_serde_struct!(SourceCut {
    owners: MapRoot,
    paths: MapRoot,
    frontiers: MapRoot
});
impl serde::Serialize for SourceInput {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer
            .serialize_struct("SourceInput", if self.language.is_some() { 4 } else { 3 })?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "path", &self.path)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "bytes", &self.bytes)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "text", &self.text)?;
        if let Some(language) = &self.language {
            serde::ser::SerializeStruct::serialize_field(&mut state, "language", language)?;
        }
        serde::ser::SerializeStruct::end(state)
    }
}

impl<'de> serde::Deserialize<'de> for SourceInput {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["path", "bytes", "text", "language"];
        struct SourceInputVisitor;
        impl<'de> serde::de::Visitor<'de> for SourceInputVisitor {
            type Value = SourceInput;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a closed source input with optional language")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let (mut path, mut bytes, mut text, mut language) = (None, None, None, None);
                while let Some(key) =
                    map.next_key_seed(quanta_index_contract_base::ClosedFieldKeySeedV1(FIELDS))?
                {
                    match key {
                        "path" => {
                            if path.is_some() {
                                return Err(serde::de::Error::duplicate_field("path"));
                            }
                            path = Some(map.next_value::<String>()?);
                        }
                        "bytes" => {
                            if bytes.is_some() {
                                return Err(serde::de::Error::duplicate_field("bytes"));
                            }
                            bytes = Some(map.next_value::<Vec<u8>>()?);
                        }
                        "text" => {
                            if text.is_some() {
                                return Err(serde::de::Error::duplicate_field("text"));
                            }
                            text = Some(map.next_value::<SourceTextAdmission>()?);
                        }
                        "language" => {
                            if language.is_some() {
                                return Err(serde::de::Error::duplicate_field("language"));
                            }
                            language = Some(map.next_value::<Option<crate::lex::LanguageCode>>()?);
                        }
                        _ => unreachable!("closed field seed only returns schema keys"),
                    }
                }
                Ok(SourceInput {
                    path: path.ok_or_else(|| serde::de::Error::missing_field("path"))?,
                    bytes: bytes.ok_or_else(|| serde::de::Error::missing_field("bytes"))?,
                    text: text.ok_or_else(|| serde::de::Error::missing_field("text"))?,
                    language: language.flatten(),
                })
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let path = seq
                    .next_element::<String>()?
                    .ok_or_else(|| serde::de::Error::missing_field("path"))?;
                let bytes = seq
                    .next_element::<Vec<u8>>()?
                    .ok_or_else(|| serde::de::Error::missing_field("bytes"))?;
                let text = seq
                    .next_element::<SourceTextAdmission>()?
                    .ok_or_else(|| serde::de::Error::missing_field("text"))?;
                let language = seq
                    .next_element::<Option<crate::lex::LanguageCode>>()?
                    .flatten();
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(5, &self));
                }
                Ok(SourceInput {
                    path,
                    bytes,
                    text,
                    language,
                })
            }
        }
        deserializer.deserialize_struct("SourceInput", FIELDS, SourceInputVisitor)
    }
}
quanta_index_contract_base::native_serde_enum!(SourceTextAdmission {
    units { Excluded = 0 }
    newtypes { }
    records { Utf8 = 1 { policy: Digest } }
});
quanta_index_contract_base::native_serde_struct!(OwnerMutation { owner: OwnerKey, expected: Option<ObjectRef>, next: Option<ObjectRef> });
quanta_index_contract_base::native_serde_struct!(OwnerMutationSet {
    root: MapRoot,
    count: u64,
    digest: Digest
});
quanta_index_contract_base::native_serde_struct!(IngressRef {
    operation: OperationKey,
    fence: u64
});
quanta_index_contract_base::native_serde_struct!(SourceEvent {
    producer: ProducerId,
    stream: StreamId,
    stream_epoch: u64,
    predecessor: u64,
    sequence: u64,
    payload: Digest
});
quanta_index_contract_base::native_serde_struct!(SourceTransition { expected_source: SourceCut, owners: OwnerMutationSet, events: Vec<SourceEvent> });
quanta_index_contract_base::native_serde_struct!(InputBinding {
    root: MapRoot,
    schema: SchemaId,
    policy: Digest,
    context: Digest
});
quanta_index_contract_base::native_serde_enum!(CoverageStatus { units { Complete = 2 } newtypes {  } records { Pending = 0 { previous_missing: MapRoot }, Partial = 1 { missing: MapRoot } } });
quanta_index_contract_base::native_serde_struct!(CoverageBinding { instance: IndexInstanceId, desired: InputBinding, covered: Option<InputBinding>, searchable: Option<ObjectRef>, status: CoverageStatus, owners: MapRoot });
quanta_index_contract_base::native_serde_struct!(OwnerInputBinding { source: Option<ObjectRef>, projection: Option<ObjectRef>, producer_context: Digest, derivation_policy: Digest });
quanta_index_contract_base::native_serde_struct!(OwnerCoverage { owner: OwnerKey, desired: OwnerInputBinding, covered: Option<OwnerInputBinding>, searchable: Option<ObjectRef> });
quanta_index_contract_base::native_serde_struct!(ProjectionBinding {
    projection: ProjectionId,
    producer: ProducerId,
    owners: MapRoot,
    source: SourceCut,
    schema: SchemaId,
    producer_policy: Digest,
    dependency_context: Digest,
    certificate: ObjectRef
});
quanta_index_contract_base::native_serde_struct!(ProjectionOwnerContribution { projection: ProjectionId, producer: ProducerId, owner: OwnerKey, source_owner: Option<OwnerKey>, source: Option<ObjectRef>, dependency_stamp: Digest, output: Option<ObjectRef> });
quanta_index_contract_base::native_serde_struct!(ProjectionCertificate { projection: ProjectionId, producer: ProducerId, prior: Option<ProjectionBinding>, target_source: SourceCut, schema: SchemaId, producer_policy: Digest, dependency_context: Digest, affected: OwnerMutationSet });
quanta_index_contract_base::native_serde_enum!(ProducerCompleteness { units {  } newtypes {  } records { CompleteAffectedDomain = 0 { certificate: ObjectRef }, Partial = 1 { missing: MapRoot } } });
quanta_index_contract_base::native_serde_struct!(ProjectionTransition { projection: ProjectionId, producer: ProducerId, expected: Option<ProjectionBinding>, target_source: SourceCut, schema: SchemaId, producer_policy: Digest, dependency_context: Digest, completeness: ProducerCompleteness, owners: OwnerMutationSet, events: Vec<SourceEvent> });
quanta_index_contract_base::native_serde_enum!(WorkScope { units {  } newtypes {  } records { Owner = 0 { owner: OwnerKey, expected: OwnerInputBinding }, ProjectionDomain = 1 { projection: ProjectionId, source: SourceCut, producer_policy: Digest, dependency_context: Digest } } });
quanta_index_contract_base::native_serde_struct!(WorkIntent {
    workspace: WorkspaceKey,
    epoch: u64,
    instance: IndexInstanceId,
    scope: WorkScope
});
quanta_index_contract_base::native_serde_struct!(WorkClaim {
    intent: WorkIntent,
    fence: u64,
    lease_until_ms: u64
});
quanta_index_contract_base::native_serde_enum!(WorkStatus { units { Unknown = 0, Pending = 1, Superseded = 4 } newtypes { Claimed = 2 => WorkClaim, Completed = 3 => ViewRef, Blocked = 5 => Refusal } records {  } });
quanta_index_contract_base::native_serde_struct!(OwnerMemberBinding {
    instance: IndexInstanceId,
    owner: OwnerKey,
    input: OwnerInputBinding,
    format: StorageFormatId,
    artifact: ObjectRef,
    logical_commitment: Digest
});
quanta_index_contract_base::native_serde_struct!(MemberBinding {
    instance: IndexInstanceId,
    format: StorageFormatId,
    artifact: ObjectRef,
    input: InputBinding,
    logical_commitment: Digest
});
quanta_index_contract_base::native_serde_enum!(GroupActivation { units { CoreRequired = 0, AsyncCoverage = 1 } newtypes {  } records {  } });
quanta_index_contract_base::native_serde_struct!(InstanceSpec {
    instance: IndexInstanceId,
    kind: IndexKind,
    group: PublicationGroupId,
    activation: GroupActivation,
    input_domain: InputDomain,
    schema: SchemaId,
    policy: Digest,
    format: StorageFormatId
});
quanta_index_contract_base::native_serde_enum!(InputDomain { units { SourceLocal = 0, History = 2 } newtypes {  } records { Projection = 1 { projection: ProjectionId, producer: ProducerId } } });
quanta_index_contract_base::native_serde_struct!(IndexProfile { instances: Vec<InstanceSpec> });
quanta_index_contract_base::native_serde_struct!(CatalogView {
    identity: ViewRef,
    profile: ObjectRef,
    source: SourceCut,
    projections: MapRoot,
    members: MapRoot,
    coverage: MapRoot
});
quanta_index_contract_base::native_serde_enum!(SourceOperationMode { units { Unchanged = 0, Advance = 1 } newtypes { } records { Resync = 2 { prior_epoch: u64 } } });
quanta_index_contract_base::native_serde_enum!(UpdateKind { units {  } newtypes { SourceAdvance = 0 => SourceTransition, ProjectionAdvance = 1 => Box<ProjectionTransition>, HistoryAdvance = 5 => HistoryTransition, HistoryAppend = 6 => HistoryAppendV1, SourceHistoryAdvance = 7 => Box<SourceHistoryTransitionV1> } records { Reconfigure = 2 { profile: ObjectRef }, Materialize = 3 { claim: WorkClaim }, SourceResync = 4 { prior_epoch: u64, transition: SourceTransition }, CompactSourceLocal = 8 { instance: IndexInstanceId, expected_artifact: ObjectRef, owner: OwnerKey }, SourceHistoryResync = 9 { prior_epoch: u64, transition: Box<SourceHistoryTransitionV1> } } });
quanta_index_contract_base::native_serde_struct!(UpdateRequest {
    operation: OperationKey,
    expected_view: ViewRef,
    kind: UpdateKind
});
quanta_index_contract_base::native_serde_struct!(ImmutableOperation {
    key: OperationKey,
    request: ObjectRef,
    source_mode: SourceOperationMode
});
quanta_index_contract_base::native_serde_enum!(Refusal { units { Conflict = 0, FenceLost = 1, InvalidInput = 2, ResourceLimited = 3, BackendViolation = 4, RebuildRequired = 5, MaintenanceRequired = 6 } newtypes {  } records {  } });
quanta_index_contract_base::native_serde_enum!(OperationStatus { units { Unknown = 0 } newtypes { Pending = 1 => AttemptRef, Published = 2 => ViewRef, Refused = 3 => Refusal } records {  } });
quanta_index_contract_base::native_serde_enum!(ViewSelection { units {  } newtypes { Exact = 1 => ViewRef } records { Latest = 0 { workspace: WorkspaceKey, epoch: u64 } } });
quanta_index_contract_base::native_serde_enum!(SubmitOutcome { units {  } newtypes { Accepted = 0 => AttemptRef, Terminal = 1 => OperationStatus, Unknown = 2 => OperationKey } records {  } });

impl<T: serde::Serialize> serde::Serialize for OperationReply<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("OperationReply", 2)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "operation", &self.operation)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "payload", &self.payload)?;
        serde::ser::SerializeStruct::end(state)
    }
}

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for OperationReply<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["operation", "payload"];
        struct ReplyVisitor<T>(std::marker::PhantomData<fn() -> T>);
        impl<'de, T: serde::Deserialize<'de>> serde::de::Visitor<'de> for ReplyVisitor<T> {
            type Value = OperationReply<T>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a complete original-operation reply")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut operation = None;
                let mut payload = None;
                while let Some(key) =
                    map.next_key_seed(quanta_index_contract_base::ClosedFieldKeySeedV1(FIELDS))?
                {
                    match key {
                        "operation" => {
                            if operation.is_some() {
                                return Err(serde::de::Error::duplicate_field("operation"));
                            }
                            operation = Some(map.next_value::<OperationKey>()?);
                        }
                        "payload" => {
                            if payload.is_some() {
                                return Err(serde::de::Error::duplicate_field("payload"));
                            }
                            payload = Some(map.next_value::<T>()?);
                        }
                        unknown => return Err(serde::de::Error::unknown_field(unknown, FIELDS)),
                    }
                }
                Ok(OperationReply {
                    operation: operation
                        .ok_or_else(|| serde::de::Error::missing_field("operation"))?,
                    payload: payload.ok_or_else(|| serde::de::Error::missing_field("payload"))?,
                })
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let operation = seq
                    .next_element::<OperationKey>()?
                    .ok_or_else(|| serde::de::Error::missing_field("operation"))?;
                let payload = seq
                    .next_element::<T>()?
                    .ok_or_else(|| serde::de::Error::missing_field("payload"))?;
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(3, &self));
                }
                Ok(OperationReply { operation, payload })
            }
        }
        deserializer.deserialize_struct(
            "OperationReply",
            FIELDS,
            ReplyVisitor(std::marker::PhantomData),
        )
    }
}
