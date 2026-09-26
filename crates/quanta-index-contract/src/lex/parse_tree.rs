//! `ParseTreeRecord` + `ParseNode` (recursive).
//!
//! This file defines the recursive wire shape. A type definition alone does
//! not establish producer adoption or public ingress support; those claims
//! require the boundary in
//! `docs/adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md`.
//!
//! `ParseNode` is recursive (children: `Vec<ParseNode>`). Serialization /
//! deserialization is therefore reentrant; we rely on serde's seed-driven
//! tree walk (no explicit stack). The historical STR-01 16-depth limit
//! applies to the **pattern**, not this parsed tree. Transport must bound
//! record size separately; this type does not impose either limit.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};
use sha2::{Digest, Sha256};

use super::LanguageCode;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ParseRoleTag {
    pub role: Box<str>,
    pub byte_start: u32,
    pub byte_end: u32,
}

const PARSE_ROLE_TAG_FIELDS: &[&str] = &["role", "byte_start", "byte_end"];

impl Serialize for ParseRoleTag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ParseRoleTag", 3)?;
        state.serialize_field("role", self.role.as_ref())?;
        state.serialize_field("byte_start", &self.byte_start)?;
        state.serialize_field("byte_end", &self.byte_end)?;
        state.end()
    }
}

struct ParseRoleTagVisitor;

impl<'de> Visitor<'de> for ParseRoleTagVisitor {
    type Value = ParseRoleTag;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ParseRoleTag map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut role: Option<String> = None;
        let mut byte_start: Option<u32> = None;
        let mut byte_end: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "role" => {
                    if role.is_some() {
                        return Err(de::Error::duplicate_field("role"));
                    }
                    role = Some(map.next_value()?);
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
                other => return Err(de::Error::unknown_field(other, PARSE_ROLE_TAG_FIELDS)),
            }
        }
        Ok(ParseRoleTag {
            role: role
                .ok_or_else(|| de::Error::missing_field("role"))?
                .into_boxed_str(),
            byte_start: byte_start.ok_or_else(|| de::Error::missing_field("byte_start"))?,
            byte_end: byte_end.ok_or_else(|| de::Error::missing_field("byte_end"))?,
        })
    }
}

impl<'de> Deserialize<'de> for ParseRoleTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("ParseRoleTag", PARSE_ROLE_TAG_FIELDS, ParseRoleTagVisitor)
    }
}

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
/// site (per producer-handoff §3.3.1). The canonical in-repo contract rule is
/// SHA-256 over the chunk's post-normalize `text` bytes; consumers
/// enforce that integrity check.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ParseTreeRecord {
    pub wire_version: u32,
    pub lang: LanguageCode,
    pub root: ParseNode,
    pub source_hash: [u8; 32],
    pub role_tag_schema_version: u32,
    pub role_tags: Vec<ParseRoleTag>,
}

const PARSE_TREE_RECORD_FIELDS: &[&str] = &[
    "wire_version",
    "lang",
    "root",
    "source_hash",
    "role_tag_schema_version",
    "role_tags",
];

impl Serialize for ParseTreeRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ParseTreeRecord", 6)?;
        state.serialize_field("wire_version", &self.wire_version)?;
        state.serialize_field("lang", &self.lang)?;
        state.serialize_field("root", &self.root)?;
        state.serialize_field("source_hash", &self.source_hash)?;
        state.serialize_field("role_tag_schema_version", &self.role_tag_schema_version)?;
        state.serialize_field("role_tags", &self.role_tags)?;
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
        let mut lang: Option<LanguageCode> = None;
        let mut root: Option<ParseNode> = None;
        let mut source_hash: Option<[u8; 32]> = None;
        let mut role_tag_schema_version: Option<u32> = None;
        let mut role_tags: Option<Vec<ParseRoleTag>> = None;
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
                "role_tag_schema_version" => {
                    if role_tag_schema_version.is_some() {
                        return Err(de::Error::duplicate_field("role_tag_schema_version"));
                    }
                    role_tag_schema_version = Some(map.next_value()?);
                }
                "role_tags" => {
                    if role_tags.is_some() {
                        return Err(de::Error::duplicate_field("role_tags"));
                    }
                    role_tags = Some(map.next_value()?);
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
            role_tag_schema_version: role_tag_schema_version
                .ok_or_else(|| de::Error::missing_field("role_tag_schema_version"))?,
            role_tags: role_tags.ok_or_else(|| de::Error::missing_field("role_tags"))?,
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

/// Canonical producer/search-plane integrity hash for one structural chunk.
///
/// Structural parse-tree byte offsets are defined against the chunk's
/// post-normalize text, so the hash authority is the exact `text` byte
/// sequence carried by the sibling [`crate::ChunkRecord`]. This helper is
/// infallible by construction: SHA-256 over an in-memory byte slice has no
/// data-dependent failure mode.
#[must_use]
pub fn compute_parse_tree_source_hash(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::compute_parse_tree_source_hash;

    #[test]
    fn parse_tree_source_hash_is_sha256_of_text_bytes() {
        let got = compute_parse_tree_source_hash("fn main() {}\n");
        let expected = [
            0x53, 0x6e, 0x50, 0x6b, 0xb9, 0x09, 0x14, 0xc2, 0x43, 0xa1, 0x2b, 0x39, 0x7b, 0x9a,
            0x99, 0x8f, 0x85, 0xae, 0x2c, 0xbd, 0x9b, 0xa0, 0x2d, 0xfd, 0x03, 0xa9, 0xe1, 0x55,
            0xca, 0x5c, 0xa0, 0xf4,
        ];
        assert_eq!(got, expected);
    }

    #[test]
    fn parse_tree_source_hash_changes_when_text_changes() {
        let a = compute_parse_tree_source_hash("fn main() {}");
        let b = compute_parse_tree_source_hash("fn main(){ }");
        assert_ne!(a, b);
    }
}
