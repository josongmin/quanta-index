//! The quarantine a daemon reports and discards over its control socket
//! (QI-BB-026 follow-up).
//!
//! Boot sets aside what it cannot trust — a generation directory whose
//! identity is unreadable or names another scope, a `RepoMap` file that
//! does not decode or match its digest — and serves without it. This is
//! the operator's typed view of that set, read live from the adapters, and
//! the one way to remove an entry: by naming exactly what the inventory
//! reported, so a repaired or sealed directory is never removed by a stale
//! listing.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::SearchPlaneTrackKind;

/// Ask the daemon what is quarantined right now.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QuarantineInventoryRequest;

const QUARANTINE_INVENTORY_REQUEST_FIELDS: &[&str] = &[];

impl Serialize for QuarantineInventoryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer
            .serialize_struct("QuarantineInventoryRequest", 0)?
            .end()
    }
}

struct QuarantineInventoryRequestVisitor;

impl<'de> Visitor<'de> for QuarantineInventoryRequestVisitor {
    type Value = QuarantineInventoryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an empty QuarantineInventoryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        if let Some(key) = map.next_key::<String>()? {
            return Err(de::Error::unknown_field(&key, QUARANTINE_INVENTORY_REQUEST_FIELDS));
        }
        Ok(QuarantineInventoryRequest)
    }
}

impl<'de> Deserialize<'de> for QuarantineInventoryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "QuarantineInventoryRequest",
            QUARANTINE_INVENTORY_REQUEST_FIELDS,
            QuarantineInventoryRequestVisitor,
        )
    }
}

/// One generation directory the inventory set aside.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantinedGenerationEntryV1 {
    pub track: SearchPlaneTrackKind,
    /// The directory, as the adapter reports it.
    pub path: String,
    /// The quarantine reason's code (`GENERATION_QUARANTINE_…`).
    pub reason: String,
    pub detail: String,
}

/// One `RepoMap` file the store moved aside.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantinedRepoMapFileEntryV1 {
    pub file_name: String,
    pub reason: String,
}

/// Everything quarantined right now, per authority.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QuarantineInventoryV1 {
    pub lexical: Vec<QuarantinedGenerationEntryV1>,
    pub semantic: Vec<QuarantinedGenerationEntryV1>,
    pub repo_map: Vec<QuarantinedRepoMapFileEntryV1>,
}

/// What to discard: exactly one entry as the inventory reported it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuarantineTargetV1 {
    Generation(QuarantinedGenerationEntryV1),
    RepoMapFile(QuarantinedRepoMapFileEntryV1),
}

/// Discard one quarantined entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantineDiscardRequest {
    pub target: QuarantineTargetV1,
}

/// What a discard did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuarantineDiscardOutcomeDtoV1 {
    /// The entry's bytes are gone; `bytes` is what was on disk.
    Discarded { bytes: u64 },
    /// Nothing was at the path any more.
    Absent,
}

/// The answer to a discard: the target as it was named, and the outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantineDiscardAck {
    pub target: QuarantineTargetV1,
    pub outcome: QuarantineDiscardOutcomeDtoV1,
}

fn reject_empty<E: de::Error>(field: &str, value: &str) -> Result<(), E> {
    if value.is_empty() {
        return Err(E::custom(format!("`{field}` must not be empty")));
    }
    Ok(())
}

/// Serialize one struct with the given fields, then decode it back field by
/// field, refusing duplicates, unknown and missing fields.
///
/// The default form runs the type's `validate_wire` after decoding; the
/// `unvalidated` form is for a type whose fields validate themselves.
macro_rules! quarantine_struct_serde {
    ($ty:ident, $visitor:ident, $fields:ident, [$($field:ident : $field_ty:ty),+ $(,)?]) => {
        quarantine_struct_serde!(@codec $ty, $visitor, $fields, [$($field : $field_ty),+]);

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let decoded = deserializer.deserialize_struct(stringify!($ty), $fields, $visitor)?;
                decoded.validate_wire::<D::Error>()?;
                Ok(decoded)
            }
        }
    };
    ($ty:ident, $visitor:ident, $fields:ident, [$($field:ident : $field_ty:ty),+ $(,)?], unvalidated) => {
        quarantine_struct_serde!(@codec $ty, $visitor, $fields, [$($field : $field_ty),+]);

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_struct(stringify!($ty), $fields, $visitor)
            }
        }
    };
    (@codec $ty:ident, $visitor:ident, $fields:ident, [$($field:ident : $field_ty:ty),+ $(,)?]) => {
        const $fields: &[&str] = &[$(stringify!($field)),+];

        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut state = serializer.serialize_struct(stringify!($ty), $fields.len())?;
                $(state.serialize_field(stringify!($field), &self.$field)?;)+
                state.end()
            }
        }

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
            type Value = $ty;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!("a ", stringify!($ty), " map"))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                $(let mut $field: Option<$field_ty> = None;)+
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        $(
                            stringify!($field) => {
                                if $field.is_some() {
                                    return Err(de::Error::duplicate_field(stringify!($field)));
                                }
                                $field = Some(map.next_value()?);
                            }
                        )+
                        other => return Err(de::Error::unknown_field(other, $fields)),
                    }
                }
                Ok($ty {
                    $($field: $field.ok_or_else(|| de::Error::missing_field(stringify!($field)))?,)+
                })
            }
        }
    };
}

quarantine_struct_serde!(
    QuarantinedGenerationEntryV1,
    QuarantinedGenerationEntryVisitor,
    QUARANTINED_GENERATION_ENTRY_FIELDS,
    [track: SearchPlaneTrackKind, path: String, reason: String, detail: String]
);
quarantine_struct_serde!(
    QuarantinedRepoMapFileEntryV1,
    QuarantinedRepoMapFileEntryVisitor,
    QUARANTINED_REPO_MAP_FILE_ENTRY_FIELDS,
    [file_name: String, reason: String]
);
quarantine_struct_serde!(
    QuarantineInventoryV1,
    QuarantineInventoryVisitor,
    QUARANTINE_INVENTORY_FIELDS,
    [
        lexical: Vec<QuarantinedGenerationEntryV1>,
        semantic: Vec<QuarantinedGenerationEntryV1>,
        repo_map: Vec<QuarantinedRepoMapFileEntryV1>
    ]
);
quarantine_struct_serde!(
    QuarantineDiscardRequest,
    QuarantineDiscardRequestVisitor,
    QUARANTINE_DISCARD_REQUEST_FIELDS,
    [target: QuarantineTargetV1],
    unvalidated
);
quarantine_struct_serde!(
    QuarantineDiscardAck,
    QuarantineDiscardAckVisitor,
    QUARANTINE_DISCARD_ACK_FIELDS,
    [target: QuarantineTargetV1, outcome: QuarantineDiscardOutcomeDtoV1],
    unvalidated
);

impl QuarantinedGenerationEntryV1 {
    fn validate_wire<E: de::Error>(&self) -> Result<(), E> {
        reject_empty("path", &self.path)?;
        reject_empty("reason", &self.reason)
    }
}

impl QuarantinedRepoMapFileEntryV1 {
    fn validate_wire<E: de::Error>(&self) -> Result<(), E> {
        reject_empty("file_name", &self.file_name)?;
        // A file name is one path segment; anything that could walk out of
        // the quarantine directory is refused before it reaches a port.
        if self.file_name == "." || self.file_name == ".." || self.file_name.contains('/') {
            return Err(E::custom(format!(
                "`file_name` `{}` is not a single path segment",
                self.file_name
            )));
        }
        Ok(())
    }
}

impl QuarantineInventoryV1 {
    fn validate_wire<E: de::Error>(&self) -> Result<(), E> {
        for entry in &self.lexical {
            if entry.track != SearchPlaneTrackKind::Lexical {
                return Err(E::custom(format!(
                    "lexical quarantine entry `{}` carries track {:?}",
                    entry.path, entry.track
                )));
            }
        }
        for entry in &self.semantic {
            if entry.track != SearchPlaneTrackKind::Semantic {
                return Err(E::custom(format!(
                    "semantic quarantine entry `{}` carries track {:?}",
                    entry.path, entry.track
                )));
            }
        }
        Ok(())
    }
}

const QUARANTINE_TARGET_FIELDS: &[&str] = &["kind", "payload"];
const QUARANTINE_TARGET_VARIANTS: &[&str] = &["Generation", "RepoMapFile"];

impl Serialize for QuarantineTargetV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("QuarantineTargetV1", 2)?;
        match self {
            Self::Generation(entry) => {
                state.serialize_field("kind", "Generation")?;
                state.serialize_field("payload", entry)?;
            }
            Self::RepoMapFile(entry) => {
                state.serialize_field("kind", "RepoMapFile")?;
                state.serialize_field("payload", entry)?;
            }
        }
        state.end()
    }
}

struct QuarantineTargetVisitor;

impl<'de> Visitor<'de> for QuarantineTargetVisitor {
    type Value = QuarantineTargetV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a QuarantineTargetV1 adjacent-tagged map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut payload: Option<QuarantineTargetV1> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "payload" => {
                    if payload.is_some() {
                        return Err(de::Error::duplicate_field("payload"));
                    }
                    let kind = kind
                        .as_deref()
                        .ok_or_else(|| de::Error::custom("`payload` must follow `kind`"))?;
                    payload = Some(match kind {
                        "Generation" => QuarantineTargetV1::Generation(map.next_value()?),
                        "RepoMapFile" => QuarantineTargetV1::RepoMapFile(map.next_value()?),
                        other => {
                            return Err(de::Error::unknown_variant(
                                other,
                                QUARANTINE_TARGET_VARIANTS,
                            ));
                        }
                    });
                }
                other => return Err(de::Error::unknown_field(other, QUARANTINE_TARGET_FIELDS)),
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        payload.ok_or_else(|| {
            if QUARANTINE_TARGET_VARIANTS.contains(&kind.as_str()) {
                de::Error::missing_field("payload")
            } else {
                de::Error::unknown_variant(kind.as_str(), QUARANTINE_TARGET_VARIANTS)
            }
        })
    }
}

impl<'de> Deserialize<'de> for QuarantineTargetV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "QuarantineTargetV1",
            QUARANTINE_TARGET_FIELDS,
            QuarantineTargetVisitor,
        )
    }
}

const QUARANTINE_DISCARD_OUTCOME_FIELDS: &[&str] = &["kind", "bytes"];
const QUARANTINE_DISCARD_OUTCOME_VARIANTS: &[&str] = &["Discarded", "Absent"];

impl Serialize for QuarantineDiscardOutcomeDtoV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            Self::Discarded { bytes } => {
                let mut state = serializer.serialize_struct("QuarantineDiscardOutcomeDtoV1", 2)?;
                state.serialize_field("kind", "Discarded")?;
                state.serialize_field("bytes", &bytes)?;
                state.end()
            }
            Self::Absent => {
                let mut state = serializer.serialize_struct("QuarantineDiscardOutcomeDtoV1", 1)?;
                state.serialize_field("kind", "Absent")?;
                state.end()
            }
        }
    }
}

struct QuarantineDiscardOutcomeVisitor;

impl<'de> Visitor<'de> for QuarantineDiscardOutcomeVisitor {
    type Value = QuarantineDiscardOutcomeDtoV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a QuarantineDiscardOutcomeDtoV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut bytes: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "bytes" => {
                    if bytes.is_some() {
                        return Err(de::Error::duplicate_field("bytes"));
                    }
                    bytes = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, QUARANTINE_DISCARD_OUTCOME_FIELDS));
                }
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        match (kind.as_str(), bytes) {
            ("Discarded", Some(bytes)) => Ok(QuarantineDiscardOutcomeDtoV1::Discarded { bytes }),
            ("Discarded", None) => Err(de::Error::missing_field("bytes")),
            ("Absent", None) => Ok(QuarantineDiscardOutcomeDtoV1::Absent),
            ("Absent", Some(_)) => Err(de::Error::custom("`Absent` carries no `bytes`")),
            (other, _) => {
                Err(de::Error::unknown_variant(other, QUARANTINE_DISCARD_OUTCOME_VARIANTS))
            }
        }
    }
}

impl<'de> Deserialize<'de> for QuarantineDiscardOutcomeDtoV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "QuarantineDiscardOutcomeDtoV1",
            QUARANTINE_DISCARD_OUTCOME_FIELDS,
            QuarantineDiscardOutcomeVisitor,
        )
    }
}
