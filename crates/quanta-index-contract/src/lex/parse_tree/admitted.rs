//! Decode policies for the existing recursive parse-tree wire visitors.

use core::fmt;

use quanta_index_contract_base::{
    AdmittedBoxedTextSeedV1, AdmittedSequenceSeedV1, DecodeBackingAdmissionV1,
};
use serde::{
    Deserializer,
    de::{self, DeserializeSeed, MapAccess, Visitor},
};

use super::{
    LanguageCode, PARSE_NODE_FIELDS, PARSE_ROLE_TAG_FIELDS, PARSE_TREE_RECORD_FIELDS, ParseNode,
    ParseNodeVisitor, ParseRoleTag, ParseRoleTagVisitor, ParseTreeRecord, ParseTreeRecordVisitor,
};

pub(super) trait ParseTreeDecodePolicy<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error>;
    fn next_boxed<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error>;
    fn next_lang<M: MapAccess<'de>>(&self, map: &mut M) -> Result<LanguageCode, M::Error>;
    fn next_root<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ParseNode, M::Error>;
    fn next_children<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<ParseNode>, M::Error>;
    fn next_role_tags<M: MapAccess<'de>>(&self, map: &mut M)
    -> Result<Vec<ParseRoleTag>, M::Error>;
}

pub(super) struct OrdinaryParseTree;

impl<'de> ParseTreeDecodePolicy<'de> for OrdinaryParseTree {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error> {
        map.next_key::<String>()?
            .map(|key| {
                fields
                    .iter()
                    .copied()
                    .find(|field| *field == key)
                    .ok_or_else(|| de::Error::unknown_field(key.as_str(), fields))
            })
            .transpose()
    }

    fn next_boxed<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        let text: String = map.next_value()?;
        Ok(text.into_boxed_str())
    }

    fn next_lang<M: MapAccess<'de>>(&self, map: &mut M) -> Result<LanguageCode, M::Error> {
        map.next_value()
    }
    fn next_root<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ParseNode, M::Error> {
        map.next_value()
    }
    fn next_children<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<ParseNode>, M::Error> {
        map.next_value()
    }
    fn next_role_tags<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<ParseRoleTag>, M::Error> {
        map.next_value()
    }
}

struct ParseTreeFieldSeed {
    fields: &'static [&'static str],
}

impl<'de> DeserializeSeed<'de> for ParseTreeFieldSeed {
    type Value = &'static str;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for ParseTreeFieldSeed {
    type Value = &'static str;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed parse-tree field")
    }
    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        self.fields
            .iter()
            .copied()
            .find(|field| *field == key)
            .ok_or_else(|| E::unknown_field(key, self.fields))
    }
    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted parse-tree field must be borrowed"))
    }
}

struct AdmittedParseTree<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
    max_collection_items: usize,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> ParseTreeDecodePolicy<'de>
    for AdmittedParseTree<'_, A>
{
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error> {
        map.next_key_seed(ParseTreeFieldSeed { fields })
    }

    fn next_boxed<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        map.next_value_seed(AdmittedBoxedTextSeedV1::new(
            self.admission,
            self.max_text_bytes,
        ))
    }

    fn next_lang<M: MapAccess<'de>>(&self, map: &mut M) -> Result<LanguageCode, M::Error> {
        map.next_value_seed(LanguageCode::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_text_bytes,
        ))
    }

    fn next_root<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ParseNode, M::Error> {
        map.next_value_seed(ParseNode::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_text_bytes,
            self.max_collection_items,
        ))
    }

    fn next_children<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<ParseNode>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.max_collection_items,
            || {
                ParseNode::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.max_text_bytes,
                    self.max_collection_items,
                )
            },
        ))
    }

    fn next_role_tags<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<ParseRoleTag>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.max_collection_items,
            || {
                ParseRoleTag::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.max_text_bytes,
                )
            },
        ))
    }
}

impl ParseRoleTag {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        ParseRoleTagSeed(AdmittedParseTree {
            admission,
            max_text_bytes,
            max_collection_items: 0,
        })
    }
}

impl ParseNode {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
        max_collection_items: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        ParseNodeSeed(AdmittedParseTree {
            admission,
            max_text_bytes,
            max_collection_items,
        })
    }
}

impl ParseTreeRecord {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
        max_collection_items: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        ParseTreeRecordSeed(AdmittedParseTree {
            admission,
            max_text_bytes,
            max_collection_items,
        })
    }
}

struct ParseRoleTagSeed<P>(P);
struct ParseNodeSeed<P>(P);
struct ParseTreeRecordSeed<P>(P);

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for ParseRoleTagSeed<AdmittedParseTree<'_, A>>
{
    type Value = ParseRoleTag;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.0.admission;
        let result = decoder.deserialize_struct(
            "ParseRoleTag",
            PARSE_ROLE_TAG_FIELDS,
            ParseRoleTagVisitor(self.0),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for ParseNodeSeed<AdmittedParseTree<'_, A>>
{
    type Value = ParseNode;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.0.admission;
        let result =
            decoder.deserialize_struct("ParseNode", PARSE_NODE_FIELDS, ParseNodeVisitor(self.0));
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for ParseTreeRecordSeed<AdmittedParseTree<'_, A>>
{
    type Value = ParseTreeRecord;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.0.admission;
        let result = decoder.deserialize_struct(
            "ParseTreeRecord",
            PARSE_TREE_RECORD_FIELDS,
            ParseTreeRecordVisitor(self.0),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}
