//! `DiffHunkRecord`.
//!
//! Producer-authored diff hunk payload used by the history authority ingest
//! path. Search-side consumers treat this as authoritative and never shell out
//! to `git diff`.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::DiffHunkSide;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DiffHunkRecord {
    pub wire_version: u32,
    pub hunk_header: Box<str>,
    pub side: DiffHunkSide,
    pub added_text: Box<str>,
    pub removed_text: Box<str>,
    pub touched_text: Box<str>,
    pub byte_start: u32,
    pub byte_end: u32,
}

const DIFF_HUNK_RECORD_FIELDS: &[&str] = &[
    "wire_version",
    "hunk_header",
    "side",
    "added_text",
    "removed_text",
    "touched_text",
    "byte_start",
    "byte_end",
];

impl Serialize for DiffHunkRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DiffHunkRecord", 8)?;
        state.serialize_field("wire_version", &self.wire_version)?;
        state.serialize_field("hunk_header", self.hunk_header.as_ref())?;
        state.serialize_field("side", &self.side)?;
        state.serialize_field("added_text", self.added_text.as_ref())?;
        state.serialize_field("removed_text", self.removed_text.as_ref())?;
        state.serialize_field("touched_text", self.touched_text.as_ref())?;
        state.serialize_field("byte_start", &self.byte_start)?;
        state.serialize_field("byte_end", &self.byte_end)?;
        state.end()
    }
}

struct DiffHunkRecordVisitor;

impl<'de> Visitor<'de> for DiffHunkRecordVisitor {
    type Value = DiffHunkRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DiffHunkRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut wire_version: Option<u32> = None;
        let mut hunk_header: Option<String> = None;
        let mut side: Option<DiffHunkSide> = None;
        let mut added_text: Option<String> = None;
        let mut removed_text: Option<String> = None;
        let mut touched_text: Option<String> = None;
        let mut byte_start: Option<u32> = None;
        let mut byte_end: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "wire_version" => {
                    if wire_version.is_some() {
                        return Err(de::Error::duplicate_field("wire_version"));
                    }
                    wire_version = Some(map.next_value()?);
                }
                "hunk_header" => {
                    if hunk_header.is_some() {
                        return Err(de::Error::duplicate_field("hunk_header"));
                    }
                    hunk_header = Some(map.next_value()?);
                }
                "side" => {
                    if side.is_some() {
                        return Err(de::Error::duplicate_field("side"));
                    }
                    side = Some(map.next_value()?);
                }
                "added_text" => {
                    if added_text.is_some() {
                        return Err(de::Error::duplicate_field("added_text"));
                    }
                    added_text = Some(map.next_value()?);
                }
                "removed_text" => {
                    if removed_text.is_some() {
                        return Err(de::Error::duplicate_field("removed_text"));
                    }
                    removed_text = Some(map.next_value()?);
                }
                "touched_text" => {
                    if touched_text.is_some() {
                        return Err(de::Error::duplicate_field("touched_text"));
                    }
                    touched_text = Some(map.next_value()?);
                }
                "byte_start" => {
                    if byte_start.is_some() {
                        return Err(de::Error::duplicate_field("byte_start"));
                    }
                    byte_start = Some(map.next_value()?);
                }
                "byte_end" => {
                    if byte_end.is_some() {
                        return Err(de::Error::duplicate_field("byte_end"));
                    }
                    byte_end = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, DIFF_HUNK_RECORD_FIELDS)),
            }
        }
        Ok(DiffHunkRecord {
            wire_version: wire_version.ok_or_else(|| de::Error::missing_field("wire_version"))?,
            hunk_header: hunk_header
                .ok_or_else(|| de::Error::missing_field("hunk_header"))?
                .into_boxed_str(),
            side: side.ok_or_else(|| de::Error::missing_field("side"))?,
            added_text: added_text
                .ok_or_else(|| de::Error::missing_field("added_text"))?
                .into_boxed_str(),
            removed_text: removed_text
                .ok_or_else(|| de::Error::missing_field("removed_text"))?
                .into_boxed_str(),
            touched_text: touched_text
                .ok_or_else(|| de::Error::missing_field("touched_text"))?
                .into_boxed_str(),
            byte_start: byte_start.ok_or_else(|| de::Error::missing_field("byte_start"))?,
            byte_end: byte_end.ok_or_else(|| de::Error::missing_field("byte_end"))?,
        })
    }
}

impl<'de> Deserialize<'de> for DiffHunkRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "DiffHunkRecord",
            DIFF_HUNK_RECORD_FIELDS,
            DiffHunkRecordVisitor,
        )
    }
}
