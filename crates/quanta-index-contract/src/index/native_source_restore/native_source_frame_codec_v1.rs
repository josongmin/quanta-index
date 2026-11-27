//! Closed, two-pass JSON codec for one already bounded Native Source frame.
//! The first pass discovers `kind`; the second decodes its typed `body` from
//! the same input slice. No untyped body tree is retained for body-first maps.

use super::*;
use quanta_index_contract_base::ClosedFieldKeySeedV1;
use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::Deserialize;

type Frame = NativeSourceSnapshotExportFrameV1;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Kind {
    HeadBegin,
    FileBegin,
    FileChunk,
    FileEnd,
    HeadComplete,
    RetiredProjectionIdentity,
    InventoryComplete,
}

impl Kind {
    const NAMES: &'static [&'static str] = &[
        "head_begin",
        "file_begin",
        "file_chunk",
        "file_end",
        "head_complete",
        "retired_projection_identity",
        "inventory_complete",
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::HeadBegin => "head_begin",
            Self::FileBegin => "file_begin",
            Self::FileChunk => "file_chunk",
            Self::FileEnd => "file_end",
            Self::HeadComplete => "head_complete",
            Self::RetiredProjectionIdentity => "retired_projection_identity",
            Self::InventoryComplete => "inventory_complete",
        }
    }

    fn parse<E: serde::de::Error>(name: &str) -> Result<Self, E> {
        match name {
            "head_begin" => Ok(Self::HeadBegin),
            "file_begin" => Ok(Self::FileBegin),
            "file_chunk" => Ok(Self::FileChunk),
            "file_end" => Ok(Self::FileEnd),
            "head_complete" => Ok(Self::HeadComplete),
            "retired_projection_identity" => Ok(Self::RetiredProjectionIdentity),
            "inventory_complete" => Ok(Self::InventoryComplete),
            _ => Err(E::unknown_variant(name, Self::NAMES)),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Kind {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Read;
        impl Visitor<'_> for Read {
            type Value = Kind;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a Native Source frame kind")
            }
            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<Kind, E> {
                Kind::parse(name)
            }
            fn visit_string<E: serde::de::Error>(self, name: String) -> Result<Kind, E> {
                Kind::parse(&name)
            }
        }
        decoder.deserialize_identifier(Read)
    }
}

impl serde::Serialize for Kind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

// `IgnoredAny` may skip malformed JSON scalar contents. Validate every scalar
// in the first pass while discarding the body so no DOM allocation is needed.
struct JsonValueCheck;
impl serde::Serialize for JsonValueCheck {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_unit()
    }
}
impl<'de> serde::Deserialize<'de> for JsonValueCheck {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Read;
        impl<'de> Visitor<'de> for Read {
            type Value = JsonValueCheck;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a valid JSON value")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(JsonValueCheck)
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<JsonValueCheck>()?.is_some() {}
                self.visit_unit()
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map
                    .next_entry::<JsonValueCheck, JsonValueCheck>()?
                    .is_some()
                {}
                self.visit_unit()
            }
        }
        decoder.deserialize_any(Read)
    }
}

struct KindProbe {
    kind: Kind,
    body: JsonValueCheck,
}
quanta_index_contract_base::native_serde_struct!(KindProbe {
    kind: Kind,
    body: JsonValueCheck
});

macro_rules! body_record {
    ($name:ident => $variant:ident { $($field:ident : $ty:ty $(= $default:ident)?),+ $(,)? }) => {
        struct $name { $( $field: $ty, )+ }
        quanta_index_contract_base::native_serde_struct!($name { $($field: $ty $(= $default)?),+ });
        impl From<$name> for Frame {
            fn from(body: $name) -> Self {
                Self::$variant { $( $field: body.$field, )+ }
            }
        }
    };
}

body_record!(HeadBeginBody => HeadBegin {
    source_root_nonce: Digest, source_routes: ObjectRef, repo_id: RepoId,
    revision_id: RevisionId, source_head: NativeProtectedSourceHeadV1,
    file_count: u64
});
body_record!(FileBeginBody => FileBegin {
    source: SourceFileRevision, text_admitted: bool, source_bytes: u64,
    language: Option<String> = default
});
body_record!(FileChunkBody => FileChunk { offset: u64, bytes: Vec<u8> });
body_record!(FileEndBody => FileEnd { source: SourceFileRevision });
body_record!(HeadCompleteBody => HeadComplete {
    repo_id: RepoId, revision_id: RevisionId, file_count: u64,
    file_census_sha256: Digest
});
body_record!(RetiredProjectionBody => RetiredProjectionIdentity {
    projection: super::super::ProjectionId, instance: super::super::IndexInstanceId
});
body_record!(InventoryCompleteBody => InventoryComplete {
    source_root_nonce: Digest, source_routes: ObjectRef, head_count: u64,
    route_head_count: u64, retired_identity_count: u64,
    file_census_sha256: Digest, transcript_sha256: Digest
});

struct BodySeed(Kind);
impl<'de> DeserializeSeed<'de> for BodySeed {
    type Value = Frame;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Frame, D::Error> {
        Ok(match self.0 {
            Kind::HeadBegin => HeadBeginBody::deserialize(decoder)?.into(),
            Kind::FileBegin => FileBeginBody::deserialize(decoder)?.into(),
            Kind::FileChunk => FileChunkBody::deserialize(decoder)?.into(),
            Kind::FileEnd => FileEndBody::deserialize(decoder)?.into(),
            Kind::HeadComplete => HeadCompleteBody::deserialize(decoder)?.into(),
            Kind::RetiredProjectionIdentity => RetiredProjectionBody::deserialize(decoder)?.into(),
            Kind::InventoryComplete => InventoryCompleteBody::deserialize(decoder)?.into(),
        })
    }
}

struct FrameSeed(Kind);
impl<'de> DeserializeSeed<'de> for FrameSeed {
    type Value = Frame;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Frame, D::Error> {
        struct Read(Kind);
        impl<'de> Visitor<'de> for Read {
            type Value = Frame;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a closed Native Source frame")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Frame, A::Error> {
                let mut kind_seen = false;
                let mut body = None;
                while let Some(key) = map.next_key_seed(ClosedFieldKeySeedV1(&["kind", "body"]))? {
                    match key {
                        "kind" => {
                            if kind_seen {
                                return Err(A::Error::duplicate_field("kind"));
                            }
                            if map.next_value::<Kind>()? != self.0 {
                                return Err(A::Error::custom(
                                    "Native Source kind changed between passes",
                                ));
                            }
                            kind_seen = true;
                        }
                        "body" => {
                            if body.is_some() {
                                return Err(A::Error::duplicate_field("body"));
                            }
                            body = Some(map.next_value_seed(BodySeed(self.0))?);
                        }
                        _ => unreachable!("closed field seed returns only schema keys"),
                    }
                }
                if !kind_seen {
                    return Err(A::Error::missing_field("kind"));
                }
                body.ok_or_else(|| A::Error::missing_field("body"))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Frame, A::Error> {
                let actual = seq
                    .next_element::<Kind>()?
                    .ok_or_else(|| A::Error::missing_field("kind"))?;
                if actual != self.0 {
                    return Err(A::Error::custom(
                        "Native Source kind changed between passes",
                    ));
                }
                let body = seq
                    .next_element_seed(BodySeed(self.0))?
                    .ok_or_else(|| A::Error::missing_field("body"))?;
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(A::Error::invalid_length(3, &self));
                }
                Ok(body)
            }
        }
        decoder.deserialize_struct(
            "NativeSourceSnapshotExportFrameV1",
            &["kind", "body"],
            Read(self.0),
        )
    }
}

macro_rules! encode_body {
    ($serializer:ident, $name:literal, { $($field:ident),+ $(,)? }) => {{
        let mut body = $serializer.serialize_struct($name, [$(stringify!($field)),+].len())?;
        $(body.serialize_field(stringify!($field), $field)?;)+
        body.end()
    }};
}

struct BodyRef<'a>(&'a Frame);
impl serde::Serialize for BodyRef<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Frame::HeadBegin {
                source_root_nonce,
                source_routes,
                repo_id,
                revision_id,
                source_head,
                file_count,
            } => {
                encode_body!(serializer, "HeadBegin", { source_root_nonce, source_routes, repo_id, revision_id, source_head, file_count })
            }
            Frame::FileBegin {
                source,
                text_admitted,
                source_bytes,
                language,
            } => {
                encode_body!(serializer, "FileBegin", { source, text_admitted, source_bytes, language })
            }
            Frame::FileChunk { offset, bytes } => {
                encode_body!(serializer, "FileChunk", { offset, bytes })
            }
            Frame::FileEnd { source } => encode_body!(serializer, "FileEnd", { source }),
            Frame::HeadComplete {
                repo_id,
                revision_id,
                file_count,
                file_census_sha256,
            } => {
                encode_body!(serializer, "HeadComplete", { repo_id, revision_id, file_count, file_census_sha256 })
            }
            Frame::RetiredProjectionIdentity {
                projection,
                instance,
            } => encode_body!(serializer, "RetiredProjectionIdentity", { projection, instance }),
            Frame::InventoryComplete {
                source_root_nonce,
                source_routes,
                head_count,
                route_head_count,
                retired_identity_count,
                file_census_sha256,
                transcript_sha256,
            } => encode_body!(serializer, "InventoryComplete", { source_root_nonce, source_routes,
                    head_count, route_head_count, retired_identity_count, file_census_sha256, transcript_sha256 }),
        }
    }
}

impl serde::Serialize for Frame {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let kind = match self {
            Self::HeadBegin { .. } => Kind::HeadBegin,
            Self::FileBegin { .. } => Kind::FileBegin,
            Self::FileChunk { .. } => Kind::FileChunk,
            Self::FileEnd { .. } => Kind::FileEnd,
            Self::HeadComplete { .. } => Kind::HeadComplete,
            Self::RetiredProjectionIdentity { .. } => Kind::RetiredProjectionIdentity,
            Self::InventoryComplete { .. } => Kind::InventoryComplete,
        };
        let mut frame = serializer.serialize_struct("NativeSourceSnapshotExportFrameV1", 2)?;
        frame.serialize_field("kind", kind.name())?;
        frame.serialize_field("body", &BodyRef(self))?;
        frame.end()
    }
}

impl Frame {
    /// The caller must enforce the frame length and reserve decode memory
    /// before handing this exact slice to both passes.
    pub fn decode_json_v1(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let probe: KindProbe = serde_json::from_slice(bytes)?;
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let frame = FrameSeed(probe.kind).deserialize(&mut decoder)?;
        decoder.end()?;
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::index::{
        CollectionId, MapRoot, OperationKey, ProducerId, SlotId, SourceCut, SourceOperationMode,
        StreamId, TenantId, ViewRef, WorkspaceKey, WorktreeId,
    };
    use crate::{RepoRelativePath, SourceFileKey};

    fn sample_head(repo: &RepoId, revision: &RevisionId) -> NativeProtectedSourceHeadV1 {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let operation = ImmutableOperation {
            key: OperationKey {
                workspace,
                epoch: 1,
                producer: ProducerId([5; 16]),
                stream: StreamId([6; 16]),
                stream_epoch: 1,
                sequence: 1,
            },
            request: ObjectRef(Digest([7; 32])),
            source_mode: SourceOperationMode::Advance,
        };
        let mut publication = CatalogView {
            identity: ViewRef {
                workspace,
                epoch: 1,
                sequence: 1,
                digest: Digest([0; 32]),
            },
            profile: ObjectRef(Digest([8; 32])),
            source: SourceCut {
                owners: MapRoot(Some(ObjectRef(Digest([9; 32])))),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            projections: MapRoot(None),
            members: MapRoot(None),
            coverage: MapRoot(None),
        };
        publication.identity.digest = publication.identity_digest().expect("view digest");
        NativeProtectedSourceHeadV1 {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            source_operation: operation.clone(),
            source_publication: publication.clone(),
            operation,
            publication,
        }
    }

    #[test]
    fn native_file_chunk_json_wire_and_body_first_match() {
        let frame = Frame::FileChunk {
            offset: 7,
            bytes: vec![0, 1, 255],
        };
        let encoded = serde_json::to_string(&frame).expect("encode bounded frame");
        assert_eq!(
            encoded,
            r#"{"kind":"file_chunk","body":{"offset":7,"bytes":[0,1,255]}}"#
        );
        assert_eq!(
            Frame::decode_json_v1(encoded.as_bytes()).expect("canonical order"),
            frame
        );
        assert_eq!(
            Frame::decode_json_v1(
                br#"{"body":{"offset":7,"bytes":[0,1,255]},"kind":"file_chunk"}"#
            )
            .expect("body before kind"),
            frame,
        );
    }

    #[test]
    fn native_frame_closed_fields_and_scalar_validation() {
        for rejected in [
            r#"{"kind":"file_chunk","body":{"offset":7,"bytes":[0]},"kind":"file_chunk"}"#,
            r#"{"kind":"file_chunk","body":{"offset":7,"bytes":[0]},"extra":0}"#,
            r#"{"kind":"file_chunk","body":{"offset":7,"bytes":[0],"extra":0}}"#,
            r#"{"kind":"file_chunk","body":{"offset":7,"offset":8,"bytes":[0]}}"#,
            r#"{"kind":"file_chunk","body":{"offset":7,"bytes":[0]},"body":{}}"#,
            r#"{"kind":"future","body":{}}"#,
            r#"{"kind":"head_begin","body":{}}"#,
            r#"{"kind":"file_begin","body":{}}"#,
            r#"{"kind":"head_complete","body":{}}"#,
            r#"{"kind":"inventory_complete","body":{}}"#,
            r#"{"kind":"retired_projection_identity","body":{}}"#,
            r#"{"body":{"offset":7,"bytes":[0],"extra":1e9999},"kind":"file_chunk"}"#,
            r#"{"body":{"offset":7,"bytes":[0],"extra":"\ud800"},"kind":"file_chunk"}"#,
        ] {
            assert!(
                Frame::decode_json_v1(rejected.as_bytes()).is_err(),
                "{rejected}"
            );
        }
    }

    #[test]
    fn native_source_frame_fixed_field_order_and_optional_language() {
        let repo = RepoId::new("repo").expect("repo");
        let revision = RevisionId::new("rev").expect("revision");
        let head = sample_head(&repo, &revision);
        let head_json = serde_json::to_string(&head).expect("head wire");
        let head_begin = Frame::HeadBegin {
            source_root_nonce: Digest([9; 32]),
            source_routes: ObjectRef(Digest([8; 32])),
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            source_head: head,
            file_count: 1,
        };
        let nonce_json = serde_json::to_string(&Digest([9; 32])).expect("nonce wire");
        let routes_json = serde_json::to_string(&ObjectRef(Digest([8; 32]))).expect("routes wire");
        let head_wire = format!(
            "{{\"kind\":\"head_begin\",\"body\":{{\"source_root_nonce\":{nonce_json},\"source_routes\":{routes_json},\"repo_id\":\"repo\",\"revision_id\":\"rev\",\"source_head\":{head_json},\"file_count\":1}}}}"
        );
        assert_eq!(
            serde_json::to_string(&head_begin).expect("head begin wire"),
            head_wire
        );
        assert_eq!(
            Frame::decode_json_v1(head_wire.as_bytes()).expect("head begin decode"),
            head_begin
        );
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: repo.clone(),
                repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            },
            revision_id: revision.clone(),
            source_sha256: [2; 32],
        };
        let source_json = serde_json::to_string(&source).expect("source wire");
        let file = Frame::FileBegin {
            source: source.clone(),
            text_admitted: true,
            source_bytes: 4,
            language: None,
        };
        let file_wire = format!(
            "{{\"kind\":\"file_begin\",\"body\":{{\"source\":{source_json},\"text_admitted\":true,\"source_bytes\":4,\"language\":null}}}}"
        );
        assert_eq!(serde_json::to_string(&file).expect("file wire"), file_wire);
        assert_eq!(
            Frame::decode_json_v1(file_wire.as_bytes()).expect("file decode"),
            file
        );
        let absent_language = file_wire.replace(",\"language\":null", "");
        assert_eq!(
            Frame::decode_json_v1(absent_language.as_bytes()).expect("optional language"),
            file
        );

        let census = Digest([7; 32]);
        let census_json = serde_json::to_string(&census).expect("digest wire");
        let complete = Frame::HeadComplete {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            file_count: 1,
            file_census_sha256: census,
        };
        let complete_wire = format!(
            "{{\"kind\":\"head_complete\",\"body\":{{\"repo_id\":\"repo\",\"revision_id\":\"rev\",\"file_count\":1,\"file_census_sha256\":{census_json}}}}}"
        );
        assert_eq!(
            serde_json::to_string(&complete).expect("complete wire"),
            complete_wire
        );
        assert_eq!(
            Frame::decode_json_v1(complete_wire.as_bytes()).expect("complete decode"),
            complete
        );

        let root = Digest([9; 32]);
        let routes = ObjectRef(Digest([8; 32]));
        let root_json = serde_json::to_string(&root).expect("root wire");
        let routes_json = serde_json::to_string(&routes).expect("routes wire");
        let inventory = Frame::InventoryComplete {
            source_root_nonce: root,
            source_routes: routes,
            head_count: 1,
            route_head_count: 1,
            retired_identity_count: 0,
            file_census_sha256: census,
            transcript_sha256: Digest([6; 32]),
        };
        let transcript_json = serde_json::to_string(&Digest([6; 32])).expect("transcript wire");
        let inventory_wire = format!(
            "{{\"kind\":\"inventory_complete\",\"body\":{{\"source_root_nonce\":{root_json},\"source_routes\":{routes_json},\"head_count\":1,\"route_head_count\":1,\"retired_identity_count\":0,\"file_census_sha256\":{census_json},\"transcript_sha256\":{transcript_json}}}}}"
        );
        assert_eq!(
            serde_json::to_string(&inventory).expect("inventory wire"),
            inventory_wire
        );
        assert_eq!(
            Frame::decode_json_v1(inventory_wire.as_bytes()).expect("inventory decode"),
            inventory
        );

        let projection = super::super::super::ProjectionId([10; 16]);
        let instance = super::super::super::IndexInstanceId([11; 16]);
        let identity = Frame::RetiredProjectionIdentity {
            projection,
            instance,
        };
        let identity_wire = format!(
            "{{\"kind\":\"retired_projection_identity\",\"body\":{{\"projection\":{},\"instance\":{}}}}}",
            serde_json::to_string(&projection).expect("projection wire"),
            serde_json::to_string(&instance).expect("instance wire"),
        );
        assert_eq!(
            serde_json::to_string(&identity).expect("identity wire"),
            identity_wire
        );
        assert_eq!(
            Frame::decode_json_v1(identity_wire.as_bytes()).expect("identity decode"),
            identity
        );
    }
}
