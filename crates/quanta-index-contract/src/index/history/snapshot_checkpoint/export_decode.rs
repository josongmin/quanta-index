//! Preserve the adjacent-tag History exporter wire in the manual reader.

use super::{
    AuxiliaryGenerationKeyV1, BatchPublishReceipt, HistorySnapshotExportFrameV1,
    HistorySourceCutV1, ObjectRef, RepoId,
};
use crate::index::source_restore::FrameJsonValueCheckV1;

use serde::{
    Deserialize, Deserializer,
    de::{
        DeserializeSeed, EnumAccess, Error, IgnoredAny, IntoDeserializer, MapAccess, SeqAccess,
        VariantAccess, Visitor,
        value::{CowStrDeserializer, MapAccessDeserializer, MapDeserializer, SeqDeserializer},
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

// The normal exporter writes kind first and decodes the typed body directly.
// When body comes first, retain its scalar types and map entries until the
// tag is available. In particular, a CBOR bignum must not become a small u64.
enum BodyReplay<'de> {
    Unit,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Text(std::borrow::Cow<'de, str>),
    Bytes(std::borrow::Cow<'de, [u8]>),
    None,
    Some(Box<Self>),
    Newtype(Box<Self>),
    Sequence(Vec<Self>),
    Map(Vec<(Self, Self)>),
}

#[cfg(test)]
std::thread_local! {
    pub(super) static BODY_REPLAY_ENTRY_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl<'de> Deserialize<'de> for BodyReplay<'de> {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[cfg(test)]
        BODY_REPLAY_ENTRY_COUNT.with(|count| count.set(count.get() + 1));
        struct Capture;

        macro_rules! capture_scalar {
            ($($visit:ident($ty:ty) => $variant:ident),+ $(,)?) => {
                $(fn $visit<E: Error>(self, value: $ty) -> Result<Self::Value, E> {
                    Ok(BodyReplay::$variant(value))
                })+
            };
        }

        impl<'de> Visitor<'de> for Capture {
            type Value = BodyReplay<'de>;

            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("any value")
            }

            capture_scalar!(
                visit_bool(bool) => Bool,
                visit_i64(i64) => I64,
                visit_u64(u64) => U64,
                visit_f64(f64) => F64,
            );

            fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
                Ok(BodyReplay::Unit)
            }

            fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
                Ok(BodyReplay::None)
            }

            fn visit_some<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
                BodyReplay::deserialize(decoder).map(|value| BodyReplay::Some(Box::new(value)))
            }

            fn visit_newtype_struct<D: Deserializer<'de>>(
                self,
                decoder: D,
            ) -> Result<Self::Value, D::Error> {
                BodyReplay::deserialize(decoder).map(|value| BodyReplay::Newtype(Box::new(value)))
            }

            fn visit_borrowed_str<E: Error>(self, value: &'de str) -> Result<Self::Value, E> {
                Ok(BodyReplay::Text(std::borrow::Cow::Borrowed(value)))
            }

            fn visit_str<E: Error>(self, value: &str) -> Result<Self::Value, E> {
                self.visit_string(value.to_owned())
            }

            fn visit_string<E: Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(BodyReplay::Text(std::borrow::Cow::Owned(value)))
            }

            fn visit_borrowed_bytes<E: Error>(self, value: &'de [u8]) -> Result<Self::Value, E> {
                Ok(BodyReplay::Bytes(std::borrow::Cow::Borrowed(value)))
            }

            fn visit_bytes<E: Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                self.visit_byte_buf(value.to_vec())
            }

            fn visit_byte_buf<E: Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
                Ok(BodyReplay::Bytes(std::borrow::Cow::Owned(value)))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(BodyReplay::Sequence(values))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut pairs = Vec::new();
                while let Some(pair) = map.next_entry()? {
                    pairs.push(pair);
                }
                Ok(BodyReplay::Map(pairs))
            }

            fn visit_enum<A: EnumAccess<'de>>(self, _: A) -> Result<Self::Value, A::Error> {
                Err(Error::custom(
                    "untagged and internally tagged enums do not support enum input",
                ))
            }
        }

        decoder.deserialize_any(Capture)
    }
}

impl<'de> IntoDeserializer<'de, serde::de::value::Error> for BodyReplay<'de> {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}

impl<'de> Deserializer<'de> for BodyReplay<'de> {
    type Error = serde::de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        use std::borrow::Cow;
        match self {
            Self::Unit => visitor.visit_unit(),
            Self::Bool(value) => visitor.visit_bool(value),
            Self::I64(value) => visitor.visit_i64(value),
            Self::U64(value) => visitor.visit_u64(value),
            Self::F64(value) => visitor.visit_f64(value),
            Self::Text(Cow::Borrowed(value)) => visitor.visit_borrowed_str(value),
            Self::Text(Cow::Owned(value)) => visitor.visit_string(value),
            Self::Bytes(Cow::Borrowed(value)) => visitor.visit_borrowed_bytes(value),
            Self::Bytes(Cow::Owned(value)) => visitor.visit_byte_buf(value),
            Self::None => visitor.visit_none(),
            Self::Some(value) => visitor.visit_some(*value),
            Self::Newtype(value) => visitor.visit_newtype_struct(*value),
            Self::Sequence(items) => {
                SeqDeserializer::new(items.into_iter()).deserialize_any(visitor)
            }
            Self::Map(entries) => {
                MapDeserializer::new(entries.into_iter()).deserialize_any(visitor)
            }
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Unit => visitor.visit_unit(),
            Self::None => visitor.visit_none(),
            Self::Some(value) => visitor.visit_some(*value),
            value => visitor.visit_some(value),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            Self::Newtype(value) => visitor.visit_newtype_struct(*value),
            value => visitor.visit_newtype_struct(value),
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            Self::Text(tag) => visitor.visit_enum(CowStrDeserializer::new(tag)),
            Self::Map(entries) if entries.len() == 1 => {
                MapAccessDeserializer::new(MapDeserializer::new(entries.into_iter()))
                    .deserialize_enum(name, variants, visitor)
            }
            _ => Err(Error::custom("expected a tag or singleton map")),
        }
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        unit unit_struct seq tuple tuple_struct map struct identifier
    }
}

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
