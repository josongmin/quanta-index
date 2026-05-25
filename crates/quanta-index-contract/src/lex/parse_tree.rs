//! `ParseTreeRecord` + `ParseNode` (recursive).
//!
//! PROPOSED: gated on STR-01 Option A integration decision
//! ([`docs/plans/may-24-lexical-indexing-sorucegraph/tickets/STR-01.md`](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/STR-01.md)
//! §1.1; [`docs/ssot/producer-handoff.md`](../../../../docs/ssot/producer-handoff.md)
//! §3.3 + AMB-PROD-11). If Option B is selected at wave-5 entry, this op
//! never appears on the wire and these types remain a scaffold-only shape so
//! downstream `lq_structural` can compile against one canonical surface.
//!
//! Wire shape: producer-handoff §3.3.1.
//!
//! `ParseNode` is recursive (children: Vec<ParseNode>). Serialization /
//! deserialization is therefore reentrant; we rely on serde's seed-driven
//! tree walk (no explicit stack) which is fine for the producer-side depth
//! caps (STR-01 §3 — 16-depth limit applies to the **pattern**, not the
//! parsed tree, but producer-side records still bound at the 16 MiB frame cap
//! per channel-architecture.md §4.2). The pattern-side depth limit is enforced
//! elsewhere; this contract scaffold does not impose one.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use super::lang::LangId;

/// Producer-authored parse tree node.
///
/// `kind` is a producer-authoritative grammar node name (e.g. `"function_item"`,
/// `"identifier"`). `byte_start` / `byte_end` are chunk-local post-normalize
/// offsets per producer-handoff §3.3.1.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ParseNode {
    pub kind: Box<str>,
    pub byte_start: u32,
    pub byte_end: u32,
    pub children: Vec<ParseNode>,
}

const PARSE_NODE_FIELDS: &[&str] = &["kind", "byte_start", "byte_end", "children"];

impl Serialize for ParseNode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ParseNode", 4)?;
        state.serialize_field("kind", self.kind.as_ref())?;
        state.serialize_field("byte_start", &self.byte_start)?;
        state.serialize_field("byte_end", &self.byte_end)?;
        state.serialize_field("children", &self.children)?;
        state.end()
    }
}

struct ParseNodeVisitor;

impl<'de> Visitor<'de> for ParseNodeVisitor {
    type Value = ParseNode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ParseNode map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut byte_start: Option<u32> = None;
        let mut byte_end: Option<u32> = None;
        let mut children: Option<Vec<ParseNode>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
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
                "children" => {
                    if children.is_some() {
                        return Err(de::Error::duplicate_field("children"));
                    }
                    children = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, PARSE_NODE_FIELDS)),
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        let byte_start = byte_start.ok_or_else(|| de::Error::missing_field("byte_start"))?;
        let byte_end = byte_end.ok_or_else(|| de::Error::missing_field("byte_end"))?;
        let children = children.ok_or_else(|| de::Error::missing_field("children"))?;
        Ok(ParseNode {
            kind: kind.into_boxed_str(),
            byte_start,
            byte_end,
            children,
        })
    }
}

impl<'de> Deserialize<'de> for ParseNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("ParseNode", PARSE_NODE_FIELDS, ParseNodeVisitor)
    }
}

/// Producer-authored parse-tree payload per producer-handoff §3.3.1.
///
/// `source_hash` is the producer's content hash of the chunk source; mismatch
/// between the tree and the chunk text surfaces as
/// `STR_PARSE_TREE_DECODE_FAIL{reason=source_hash_mismatch}` at the decode
/// site (per producer-handoff §3.3.1). This scaffold carries the field
/// verbatim; consumers enforce the integrity check.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ParseTreeRecord {
    pub wire_version: u32,
    pub lang: LangId,
    pub root: ParseNode,
    pub source_hash: [u8; 32],
}

const PARSE_TREE_RECORD_FIELDS: &[&str] = &["wire_version", "lang", "root", "source_hash"];

impl Serialize for ParseTreeRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ParseTreeRecord", 4)?;
        state.serialize_field("wire_version", &self.wire_version)?;
        state.serialize_field("lang", &self.lang)?;
        state.serialize_field("root", &self.root)?;
        state.serialize_field("source_hash", &self.source_hash)?;
        state.end()
    }
}

struct ParseTreeRecordVisitor;

impl<'de> Visitor<'de> for ParseTreeRecordVisitor {
    type Value = ParseTreeRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ParseTreeRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut wire_version: Option<u32> = None;
        let mut lang: Option<LangId> = None;
        let mut root: Option<ParseNode> = None;
        let mut source_hash: Option<[u8; 32]> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "wire_version" => {
                    if wire_version.is_some() {
                        return Err(de::Error::duplicate_field("wire_version"));
                    }
                    wire_version = Some(map.next_value()?);
                }
                "lang" => {
                    if lang.is_some() {
                        return Err(de::Error::duplicate_field("lang"));
                    }
                    lang = Some(map.next_value()?);
                }
                "root" => {
                    if root.is_some() {
                        return Err(de::Error::duplicate_field("root"));
                    }
                    root = Some(map.next_value()?);
                }
                "source_hash" => {
                    if source_hash.is_some() {
                        return Err(de::Error::duplicate_field("source_hash"));
                    }
                    source_hash = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, PARSE_TREE_RECORD_FIELDS)),
            }
        }
        let wire_version = wire_version.ok_or_else(|| de::Error::missing_field("wire_version"))?;
        let lang = lang.ok_or_else(|| de::Error::missing_field("lang"))?;
        let root = root.ok_or_else(|| de::Error::missing_field("root"))?;
        let source_hash = source_hash.ok_or_else(|| de::Error::missing_field("source_hash"))?;
        Ok(ParseTreeRecord {
            wire_version,
            lang,
            root,
            source_hash,
        })
    }
}

impl<'de> Deserialize<'de> for ParseTreeRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ParseTreeRecord",
            PARSE_TREE_RECORD_FIELDS,
            ParseTreeRecordVisitor,
        )
    }
}
