//! Preserve the adjacent-tag History exporter wire in the manual reader.

use super::{
    AuxiliaryGenerationKeyV1, BatchPublishReceipt, HistorySnapshotExportFrameV1,
    HistorySourceCutV1, ObjectRef, RepoId,
};
use crate::index::source_restore::FrameJsonValueCheckV1;

use serde::{
    Deserialize, Deserializer,
    de::{
        DeserializeSeed, EnumAccess, Error, IgnoredAny, MapAccess, SeqAccess, VariantAccess,
        Visitor,
    },
};

const FRAME_NAME: &str = "HistorySnapshotExportFrameV1";
const FRAME_FIELDS: &[&str] = &["kind", "body"];

struct FieldKey(&'static [&'static str]);

impl<'de> DeserializeSeed<'de> for FieldKey {
    type Value = Option<&'static str>;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for FieldKey {
    type Value = Option<&'static str>;

    fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("a History export field identifier")
    }

    fn visit_str<E: Error>(self, key: &str) -> Result<Self::Value, E> {
        self.visit_bytes(key.as_bytes())
    }

    fn visit_bytes<E: Error>(self, key: &[u8]) -> Result<Self::Value, E> {
        Ok(self.0.iter().copied().find(|field| field.as_bytes() == key))
    }

    fn visit_u64<E: Error>(self, ordinal: u64) -> Result<Self::Value, E> {
        Ok(usize::try_from(ordinal)
            .ok()
            .and_then(|i| self.0.get(i))
            .copied())
    }
}

macro_rules! history_body {
    ($decoder:expr, $variant:ident { $($field:ident : $ty:ty $(= $default:ident)?),+ $(,)? }) => {{
        const FIELDS: &[&str] = &[$(stringify!($field)),+];
        struct BodyVisitor;

        impl<'de> Visitor<'de> for BodyVisitor {
            type Value = HistorySnapshotExportFrameV1;

            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "struct variant {FRAME_NAME}::{}", stringify!($variant))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                $(let mut $field: Option<$ty> = None;)+
                while let Some(key) = map.next_key_seed(FieldKey(FIELDS))? {
                    match key {
                        $(Some(stringify!($field)) => {
                            if $field.is_some() {
                                return Err(Error::duplicate_field(stringify!($field)));
                            }
                            $field = Some(map.next_value::<$ty>()?);
                        },)+
                        _ => { let _: IgnoredAny = map.next_value()?; }
                    }
                }
                Ok(HistorySnapshotExportFrameV1::$variant {
                    $($field: history_body!(@value $field $(= $default)?),)+
                })
            }
        }

        $decoder.deserialize_any(BodyVisitor)
    }};
    (@value $field:ident) => {
        $field.ok_or_else(|| Error::missing_field(stringify!($field)))?
    };
    (@value $field:ident = default) => {
        $field.unwrap_or_default()
    };
}

#[cfg(test)]
pub(super) use crate::index::frame_body_replay::BODY_REPLAY_ENTRY_COUNT;
use crate::index::frame_body_replay::BodyReplay;

struct HistoryFrameVisitor(Option<HistoryFrameKindV1>);

impl<'de> Visitor<'de> for HistoryFrameVisitor {
    type Value = HistorySnapshotExportFrameV1;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "adjacently tagged enum {FRAME_NAME}")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut kind = None;
        let mut deferred = None;
        let mut frame = None;
        while let Some(key) = map.next_key_seed(FieldKey(FRAME_FIELDS))? {
            match key {
                Some("kind") => {
                    if kind.is_some() {
                        return Err(Error::duplicate_field("kind"));
                    }
                    let read_kind = map.next_value_seed(HistoryKindTag)?;
                    if self.0.is_some_and(|expected| expected != read_kind) {
                        return Err(Error::custom("History frame kind changed between passes"));
                    }
                    if let Some(body) = deferred.take() {
                        frame = Some(
                            HistoryFrameBodySeedV1(read_kind)
                                .deserialize(body)
                                .map_err(A::Error::custom)?,
                        );
                    }
                    kind = Some(read_kind);
                }
                Some("body") => {
                    if frame.is_some() || deferred.is_some() {
                        return Err(Error::duplicate_field("body"));
                    }
                    if let Some(kind) = kind.or(self.0) {
                        frame = Some(map.next_value_seed(HistoryFrameBodySeedV1(kind))?);
                    } else {
                        deferred = Some(map.next_value::<BodyReplay<'de>>()?);
                    }
                }
                _ => {
                    let _: IgnoredAny = map.next_value()?;
                }
            }
        }
        if kind.is_none() {
            return Err(Error::missing_field("kind"));
        }
        frame.ok_or_else(|| Error::missing_field("body"))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let kind = seq
            .next_element()?
            .ok_or_else(|| Error::invalid_length(0, &self))?;
        if self.0.is_some_and(|expected| expected != kind) {
            return Err(Error::custom("History frame kind changed between passes"));
        }
        seq.next_element_seed(HistoryFrameBodySeedV1(kind))?
            .ok_or_else(|| Error::invalid_length(1, &self))
    }
}

impl<'de> Deserialize<'de> for HistorySnapshotExportFrameV1 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_struct(FRAME_NAME, FRAME_FIELDS, HistoryFrameVisitor(None))
    }
}

struct HistoryFrameKindReadV1;

impl HistoryFrameKindReadV1 {
    fn decode(bytes: &[u8]) -> Result<HistoryFrameKindV1, serde_json::Error> {
        struct KindReader;

        impl<'de> DeserializeSeed<'de> for KindReader {
            type Value = HistoryFrameKindV1;

            fn deserialize<D: Deserializer<'de>>(
                self,
                decoder: D,
            ) -> Result<Self::Value, D::Error> {
                struct KindVisitor;

                impl<'de> Visitor<'de> for KindVisitor {
                    type Value = HistoryFrameKindV1;

                    fn expecting(
                        &self,
                        formatter: &mut core::fmt::Formatter<'_>,
                    ) -> core::fmt::Result {
                        write!(formatter, "adjacently tagged enum {FRAME_NAME}")
                    }

                    fn visit_map<A: MapAccess<'de>>(
                        self,
                        mut map: A,
                    ) -> Result<Self::Value, A::Error> {
                        let mut kind = None;
                        let mut body_seen = false;
                        while let Some(key) = map.next_key_seed(FieldKey(FRAME_FIELDS))? {
                            match key {
                                Some("kind") => {
                                    if kind.is_some() {
                                        return Err(Error::duplicate_field("kind"));
                                    }
                                    kind = Some(map.next_value_seed(HistoryKindTag)?);
                                }
                                Some("body") => {
                                    if body_seen {
                                        return Err(Error::duplicate_field("body"));
                                    }
                                    if kind.is_none() {
                                        let _: FrameJsonValueCheckV1 = map.next_value()?;
                                    } else {
                                        let _: IgnoredAny = map.next_value()?;
                                    }
                                    body_seen = true;
                                }
                                _ => {
                                    let _: IgnoredAny = map.next_value()?;
                                }
                            }
                        }
                        let kind = kind.ok_or_else(|| Error::missing_field("kind"))?;
                        if !body_seen {
                            return Err(Error::missing_field("body"));
                        }
                        Ok(kind)
                    }

                    fn visit_seq<A: SeqAccess<'de>>(
                        self,
                        mut seq: A,
                    ) -> Result<Self::Value, A::Error> {
                        let kind = seq
                            .next_element_seed(HistoryKindTag)?
                            .ok_or_else(|| Error::invalid_length(0, &self))?;
                        let _: IgnoredAny = seq
                            .next_element()?
                            .ok_or_else(|| Error::invalid_length(1, &self))?;
                        if seq.next_element::<IgnoredAny>()?.is_some() {
                            return Err(Error::invalid_length(3, &self));
                        }
                        Ok(kind)
                    }
                }

                decoder.deserialize_struct(FRAME_NAME, FRAME_FIELDS, KindVisitor)
            }
        }

        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let kind = KindReader.deserialize(&mut decoder)?;
        decoder.end()?;
        Ok(kind)
    }
}

impl HistorySnapshotExportFrameV1 {
    /// Decode one admitted JSON frame without retaining an untyped body tree.
    /// The caller owns frame-length and memory admission before this call.
    pub fn decode_json_v1(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let kind = HistoryFrameKindReadV1::decode(bytes)?;
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let frame = (&mut decoder).deserialize_struct(
            FRAME_NAME,
            FRAME_FIELDS,
            HistoryFrameVisitor(Some(kind)),
        )?;
        decoder.end()?;
        Ok(frame)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum HistoryFrameKindV1 {
    NativeProtectedComplete,
    NativeObjectBegin,
    NativeObjectChunk,
    NativeObjectEnd,
    NativeHistory,
    NativeInventoryComplete,
    Page,
    Complete,
    InventoryKey,
    InventoryComplete,
}

impl HistoryFrameKindV1 {
    const NAMES: &'static [&'static str] = &[
        "native_protected_complete",
        "native_object_begin",
        "native_object_chunk",
        "native_object_end",
        "native_history",
        "native_inventory_complete",
        "page",
        "complete",
        "inventory_key",
        "inventory_complete",
    ];

    fn parse<E: serde::de::Error>(name: &str) -> Result<Self, E> {
        match name {
            "native_protected_complete" => Ok(Self::NativeProtectedComplete),
            "native_object_begin" => Ok(Self::NativeObjectBegin),
            "native_object_chunk" => Ok(Self::NativeObjectChunk),
            "native_object_end" => Ok(Self::NativeObjectEnd),
            "native_history" => Ok(Self::NativeHistory),
            "native_inventory_complete" => Ok(Self::NativeInventoryComplete),
            "page" => Ok(Self::Page),
            "complete" => Ok(Self::Complete),
            "inventory_key" => Ok(Self::InventoryKey),
            "inventory_complete" => Ok(Self::InventoryComplete),
            _ => Err(E::unknown_variant(name, Self::NAMES)),
        }
    }
}

impl<'de> serde::Deserialize<'de> for HistoryFrameKindV1 {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct KindVisitor;
        impl serde::de::Visitor<'_> for KindVisitor {
            type Value = HistoryFrameKindV1;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a History export frame kind")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                HistoryFrameKindV1::parse(value)
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                HistoryFrameKindV1::parse(&value)
            }
            fn visit_bytes<E: Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                match core::str::from_utf8(value) {
                    Ok(name) => self.visit_str(name),
                    Err(_) => Err(E::unknown_variant(
                        &String::from_utf8_lossy(value),
                        HistoryFrameKindV1::NAMES,
                    )),
                }
            }
            fn visit_u64<E: Error>(self, ordinal: u64) -> Result<Self::Value, E> {
                let kind = usize::try_from(ordinal)
                    .ok()
                    .and_then(|i| HistoryFrameKindV1::NAMES.get(i));
                match kind {
                    Some(kind) => self.visit_str(kind),
                    None => Err(E::invalid_value(
                        serde::de::Unexpected::Unsigned(ordinal),
                        &"variant index 0 <= i < 10",
                    )),
                }
            }
        }
        decoder.deserialize_identifier(KindVisitor)
    }
}

struct HistoryKindTag;

impl<'de> DeserializeSeed<'de> for HistoryKindTag {
    type Value = HistoryFrameKindV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        struct TagVisitor;

        impl<'de> Visitor<'de> for TagVisitor {
            type Value = HistoryFrameKindV1;

            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "variant of enum {FRAME_NAME}")
            }

            fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
                let (kind, variant) = data.variant()?;
                variant.unit_variant()?;
                Ok(kind)
            }
        }

        decoder.deserialize_enum(FRAME_NAME, HistoryFrameKindV1::NAMES, TagVisitor)
    }
}
struct HistoryFrameBodySeedV1(HistoryFrameKindV1);

impl<'de> DeserializeSeed<'de> for HistoryFrameBodySeedV1 {
    type Value = HistorySnapshotExportFrameV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        // The derived struct variants have map bodies. Page is a newtype
        // variant and delegates directly to the page's own map/sequence codec.
        match self.0 {
            HistoryFrameKindV1::NativeProtectedComplete => {
                history_body!(decoder, NativeProtectedComplete {
                    source_root_nonce: crate::index::Digest,
                    source_routes: ObjectRef,
                    snapshot: Option<ObjectRef> = default,
                })
            }
            HistoryFrameKindV1::NativeObjectBegin => history_body!(decoder, NativeObjectBegin {
                reference: ObjectRef,
                payload_bytes: u64,
                children: Vec<ObjectRef>,
            }),
            HistoryFrameKindV1::NativeObjectChunk => history_body!(decoder, NativeObjectChunk {
                offset: u64,
                bytes: Vec<u8>,
            }),
            HistoryFrameKindV1::NativeObjectEnd => history_body!(
                decoder,
                NativeObjectEnd {
                    reference: ObjectRef,
                }
            ),
            HistoryFrameKindV1::NativeHistory => history_body!(
                decoder,
                NativeHistory {
                    source_root_nonce: crate::index::Digest,
                    source_view: crate::index::ViewRef,
                    instance: crate::index::IndexInstanceId,
                    member: ObjectRef,
                    cut: HistorySourceCutV1,
                }
            ),
            HistoryFrameKindV1::NativeInventoryComplete => history_body!(
                decoder,
                NativeInventoryComplete {
                    repo_id: RepoId,
                    count: u64,
                }
            ),
            HistoryFrameKindV1::Page => {
                Deserialize::deserialize(decoder).map(HistorySnapshotExportFrameV1::Page)
            }
            HistoryFrameKindV1::Complete => history_body!(
                decoder,
                Complete {
                    generation: AuxiliaryGenerationKeyV1,
                    legacy_root: String,
                    durable_epoch: crate::AuxEpochV1,
                    snapshot_digest: crate::index::Digest,
                    row_count: u64,
                    terminal_ack: BatchPublishReceipt,
                }
            ),
            HistoryFrameKindV1::InventoryKey => history_body!(
                decoder,
                InventoryKey {
                    generation: AuxiliaryGenerationKeyV1,
                }
            ),
            HistoryFrameKindV1::InventoryComplete => history_body!(
                decoder,
                InventoryComplete {
                    repo_id: RepoId,
                    count: u64,
                }
            ),
        }
    }
}
