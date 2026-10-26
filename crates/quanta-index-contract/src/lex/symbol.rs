//! `SymbolKindCode` / `SymbolKindFamily` / `SymbolRelationship` /
//! `SymbolSpan` / `SymbolRecord`.
//!
//! Wire shape: producer-authored symbol rows for lexical publish.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeSeed, MapAccess, Visitor},
    ser::SerializeStruct,
};

use quanta_index_contract_base::{
    AdmittedBoxedTextSeedV1, AdmittedOptionalSeedV1, DecodeBackingAdmissionV1,
};

use super::LanguageCode;
use crate::query::LqVisibility;
use crate::{RepoRelativePath, SymbolId};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SymbolKindCode(Box<str>);

impl SymbolKindCode {
    pub const ALL_V1: &'static [&'static str] = &[
        "function",
        "method",
        "class",
        "struct",
        "enum",
        "trait",
        "interface",
        "variable",
        "constant",
        "module",
        "macro",
        "type_alias",
    ];

    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        Self::new(value).into_iter().next()
    }

    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        validate_symbol_kind_code(value.as_str())?;
        Ok(Self(value.into_boxed_str()))
    }

    /// Retain caller-admitted text without copying or reallocating its backing.
    /// Validation uses the same closed predicate as the ordinary constructor.
    pub fn try_from_boxed_v1(value: Box<str>) -> Result<Self, &'static str> {
        validate_symbol_kind_code(&value)?;
        Ok(Self(value))
    }

    /// Validate exterior backing before moving it. A failed attempt preserves
    /// both slots, so the enclosing Source can retain the complete rejected input.
    pub fn try_from_boxed_into_v3(
        source: &mut Option<Box<str>>,
        destination: &mut Option<Self>,
    ) -> Result<(), &'static str> {
        if destination.is_some() {
            return Err("symbol kind destination is occupied");
        }
        validate_symbol_kind_code(source.as_deref().ok_or("symbol kind source is missing")?)?;
        let value = source.take().ok_or("symbol kind source is missing")?;
        *destination = Some(Self(value));
        Ok(())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    #[must_use]
    pub fn into_inner(self) -> Box<str> {
        self.0
    }

    /// Decode a bounded kind under the caller's retained backing admission.
    /// The closed wire predicate is checked before the boxed text is born.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        SymbolKindCodeAdmittedSeedV1 {
            admission,
            max_text_bytes,
        }
    }
}

struct SymbolKindCodeAdmittedSeedV1<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for SymbolKindCodeAdmittedSeedV1<'_, A>
{
    type Value = SymbolKindCode;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let boxed = AdmittedBoxedTextSeedV1::new_validated(
            self.admission,
            self.max_text_bytes,
            validate_symbol_kind_code,
        )
        .deserialize(decoder)?;
        Ok(SymbolKindCode(boxed))
    }
}

impl fmt::Display for SymbolKindCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for SymbolKindCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct SymbolKindCodeVisitor;

impl Visitor<'_> for SymbolKindCodeVisitor {
    type Value = SymbolKindCode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a canonical lowercase snake_case SymbolKindCode string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        SymbolKindCode::new(value).map_err(de::Error::custom)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        SymbolKindCode::new(value).map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for SymbolKindCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_string(SymbolKindCodeVisitor)
    }
}

fn validate_symbol_kind_code(value: &str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("symbol kind code must not be empty");
    }
    let Some(first) = value.as_bytes().first().copied() else {
        return Err("symbol kind code must not be empty");
    };
    if !first.is_ascii_lowercase() {
        return Err("symbol kind code must start with a lowercase ASCII letter");
    }
    for byte in value.bytes() {
        let ok = byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_';
        if !ok {
            return Err("symbol kind code must be lowercase snake_case ASCII");
        }
    }
    if !SymbolKindCode::ALL_V1.contains(&value) {
        return Err("symbol kind code must be one of the closed v1 wire kinds");
    }
    Ok(())
}

#[cfg(test)]
mod admitted_symbol_kind_tests {
    #[test]
    fn boxed_move_retains_backing_and_rejected_slots_v3() -> Result<(), &'static str> {
        let mut source = Some(String::from("function").into_boxed_str());
        let pointer = source.as_deref().ok_or("fixture source")?.as_ptr();
        let mut destination = None;
        SymbolKindCode::try_from_boxed_into_v3(&mut source, &mut destination)?;
        assert!(source.is_none());
        assert_eq!(
            destination
                .as_ref()
                .ok_or("accepted value")?
                .as_str()
                .as_ptr(),
            pointer
        );

        let mut rejected = Some(String::from("Function").into_boxed_str());
        let rejected_pointer = rejected.as_deref().ok_or("rejected source")?.as_ptr();
        let mut empty = None;
        assert!(SymbolKindCode::try_from_boxed_into_v3(&mut rejected, &mut empty).is_err());
        assert_eq!(rejected.as_deref().ok_or("preserved rejection")?.as_ptr(), rejected_pointer);
        assert!(empty.is_none());

        let mut occupied_source = Some(String::from("function").into_boxed_str());
        let occupied_pointer = occupied_source
            .as_deref()
            .ok_or("occupied source")?
            .as_ptr();
        assert!(
            SymbolKindCode::try_from_boxed_into_v3(&mut occupied_source, &mut destination).is_err()
        );
        assert_eq!(
            occupied_source
                .as_deref()
                .ok_or("occupied rejection")?
                .as_ptr(),
            occupied_pointer
        );
        assert_eq!(
            destination
                .as_ref()
                .ok_or("preserved destination")?
                .as_str()
                .as_ptr(),
            pointer
        );
        Ok(())
    }

    use std::cell::Cell;

    use quanta_index_contract_base::DecodeBackingRefusalV1;
    use serde::de::{DeserializeSeed, value::StrDeserializer};

    use super::DecodeBackingAdmissionV1;
    use super::SymbolKindCode;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn retains_boxed_kind_backing_v1() {
        let boxed = String::from("function").into_boxed_str();
        let pointer = boxed.as_ptr();
        let kind = SymbolKindCode::try_from_boxed_v1(boxed).expect("closed valid kind");
        assert_eq!(kind.as_str().as_ptr(), pointer);
        assert_eq!(kind.as_str(), "function");
        assert!(
            SymbolKindCode::try_from_boxed_v1(String::from("Function").into_boxed_str()).is_err()
        );
    }

    // Semantic seed fixture only; it does not prove a physical memory grant.
    struct TestAdmission {
        copies: Cell<usize>,
        refuse: bool,
        attempt: std::cell::RefCell<quanta_index_contract_base::DecodeAttemptStateV1>,
    }

    impl DecodeBackingAdmissionV1 for TestAdmission {
        fn abort_decode(&self) {
            self.attempt.borrow_mut().abort_v1();
        }

        fn enter_decode_field_v1(
            &self,
            kind: quanta_index_contract_base::DecodeFieldKindV1,
            maximum: usize,
        ) -> Result<quanta_index_contract_base::DecodeFieldTokenV1, DecodeBackingRefusalV1>
        {
            let mut attempt = self.attempt.borrow_mut();
            if !attempt.is_reading_v1() {
                attempt.begin_decode_v1()?;
            }
            attempt.enter_field_v1(kind, maximum)
        }

        fn leave_decode_field_v1(
            &self,
            token: quanta_index_contract_base::DecodeFieldTokenV1,
        ) -> Result<(), DecodeBackingRefusalV1> {
            self.attempt.borrow_mut().leave_field_v1(token)
        }

        fn reserve_text(
            &self,
            _destination: &mut String,
            _additional: usize,
        ) -> Result<(), DecodeBackingRefusalV1> {
            Err(DecodeBackingRefusalV1)
        }

        fn reserve_bytes(
            &self,
            _destination: &mut Vec<u8>,
            _additional: usize,
        ) -> Result<(), DecodeBackingRefusalV1> {
            Err(DecodeBackingRefusalV1)
        }

        fn reserve_elements<T>(
            &self,
            _destination: &mut Vec<T>,
            _additional: usize,
        ) -> Result<(), DecodeBackingRefusalV1> {
            Err(DecodeBackingRefusalV1)
        }

        fn copy_boxed_text(&self, source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
            self.copies.set(self.copies.get().saturating_add(1));
            if self.refuse {
                return Err(DecodeBackingRefusalV1);
            }
            Ok(Box::from(source))
        }

        fn adopt_parser_text<E>(
            &self,
            _source: &mut Option<String>,
            _validate: impl FnOnce(&str) -> Result<(), E>,
        ) -> Result<String, crate::ParserTextInputErrorV1<E>> {
            Err(DecodeBackingRefusalV1.into())
        }

        fn copy_boxed_parser_text<E>(
            &self,
            _source: &mut Option<String>,
            _validate: impl FnOnce(&str) -> Result<(), E>,
        ) -> Result<Box<str>, crate::ParserTextInputErrorV1<E>> {
            Err(DecodeBackingRefusalV1.into())
        }

        fn verify_parser_owned_bytes(
            &self,
            _source: &[u8],
            _capacity: usize,
        ) -> Result<(), DecodeBackingRefusalV1> {
            Err(DecodeBackingRefusalV1)
        }
    }

    #[test]
    fn admitted_kind_matches_fixed_ordinary_wire_examples() -> TestResult {
        let owner = TestAdmission {
            copies: Cell::new(0),
            refuse: false,
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
        };
        for (wire, expected) in [
            (r#""function""#, "function"),
            (r#""type_alias""#, "type_alias"),
            (r#""macro""#, "macro"),
        ] {
            let decoder = StrDeserializer::<serde::de::value::Error>::new(expected);
            let admitted = SymbolKindCode::deserialize_seed_with_admission_v1(&owner, 32)
                .deserialize(decoder)?;
            let ordinary: SymbolKindCode = serde_json::from_str(wire)?;
            if admitted != ordinary || admitted.as_str() != expected {
                return Err(format!("admitted kind differs from fixed wire {wire}").into());
            }
        }
        if owner.copies.get() != 3 {
            return Err("valid kind copies were not requested exactly once each".into());
        }
        Ok(())
    }

    #[test]
    fn invalid_or_oversized_kind_refuses_before_box_birth() -> TestResult {
        let owner = TestAdmission {
            copies: Cell::new(0),
            refuse: false,
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
        };
        for (value, max_bytes) in [
            ("Function", 32),
            ("service", 32),
            ("", 32),
            ("type_alias", 4),
        ] {
            let decoder = StrDeserializer::<serde::de::value::Error>::new(value);
            if SymbolKindCode::deserialize_seed_with_admission_v1(&owner, max_bytes)
                .deserialize(decoder)
                .is_ok()
            {
                return Err(format!("invalid kind {value} was admitted").into());
            }
        }
        if owner.copies.get() != 0 {
            return Err("invalid kind reached boxed backing admission".into());
        }
        Ok(())
    }

    #[test]
    fn backing_owner_refusal_is_not_a_decoded_kind() -> TestResult {
        let owner = TestAdmission {
            copies: Cell::new(0),
            refuse: true,
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
        };
        let decoder = StrDeserializer::<serde::de::value::Error>::new("function");
        let result =
            SymbolKindCode::deserialize_seed_with_admission_v1(&owner, 32).deserialize(decoder);
        if result.is_ok() || owner.copies.get() != 1 {
            return Err("box backing refusal did not abort the typed decode".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SymbolKindFamily {
    Callable,
    Type,
    Module,
    Value,
    Macro,
}

impl SymbolKindFamily {
    pub const ALL: &'static [Self] = &[
        Self::Callable,
        Self::Type,
        Self::Module,
        Self::Value,
        Self::Macro,
    ];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Callable => "Callable",
            Self::Type => "Type",
            Self::Module => "Module",
            Self::Value => "Value",
            Self::Macro => "Macro",
        }
    }

    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "Callable" => Some(Self::Callable),
            "Type" => Some(Self::Type),
            "Module" => Some(Self::Module),
            "Value" => Some(Self::Value),
            "Macro" => Some(Self::Macro),
            _ => None,
        }
    }
}

impl Serialize for SymbolKindFamily {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct SymbolKindFamilyVisitor;

impl Visitor<'_> for SymbolKindFamilyVisitor {
    type Value = SymbolKindFamily;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SymbolKindFamily code string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        SymbolKindFamily::from_code_str(value).ok_or_else(|| {
            de::Error::unknown_variant(value, &["Callable", "Type", "Module", "Value", "Macro"])
        })
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

impl<'de> Deserialize<'de> for SymbolKindFamily {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SymbolKindFamilyVisitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SymbolRelationship {
    Def,
    Ref,
}

impl SymbolRelationship {
    pub const ALL: &'static [Self] = &[Self::Def, Self::Ref];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Def => "Def",
            Self::Ref => "Ref",
        }
    }

    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "Def" => Some(Self::Def),
            "Ref" => Some(Self::Ref),
            _ => None,
        }
    }
}

impl fmt::Display for SymbolRelationship {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

impl Serialize for SymbolRelationship {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct SymbolRelationshipVisitor;

impl Visitor<'_> for SymbolRelationshipVisitor {
    type Value = SymbolRelationship;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SymbolRelationship code string (\"Def\" | \"Ref\")")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        SymbolRelationship::from_code_str(value)
            .ok_or_else(|| de::Error::unknown_variant(value, &["Def", "Ref"]))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

impl<'de> Deserialize<'de> for SymbolRelationship {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SymbolRelationshipVisitor)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SymbolSpan {
    pub path: Box<str>,
    pub byte_start: u32,
    pub byte_end: u32,
    pub line_start: u32,
    pub line_end: u32,
}

const SYMBOL_SPAN_FIELDS: &[&str] = &["path", "byte_start", "byte_end", "line_start", "line_end"];

#[derive(Clone, Copy)]
enum SymbolSpanFieldV1 {
    Path,
    ByteStart,
    ByteEnd,
    LineStart,
    LineEnd,
}

fn symbol_span_field_v1<E: de::Error>(key: &str) -> Result<SymbolSpanFieldV1, E> {
    match key {
        "path" => Ok(SymbolSpanFieldV1::Path),
        "byte_start" => Ok(SymbolSpanFieldV1::ByteStart),
        "byte_end" => Ok(SymbolSpanFieldV1::ByteEnd),
        "line_start" => Ok(SymbolSpanFieldV1::LineStart),
        "line_end" => Ok(SymbolSpanFieldV1::LineEnd),
        other => Err(E::unknown_field(other, SYMBOL_SPAN_FIELDS)),
    }
}

struct SymbolSpanFieldSeedV1;

impl<'de> DeserializeSeed<'de> for SymbolSpanFieldSeedV1 {
    type Value = SymbolSpanFieldV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for SymbolSpanFieldSeedV1 {
    type Value = SymbolSpanFieldV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed SymbolSpan field name")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        symbol_span_field_v1(key)
    }

    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted symbol span field key must be borrowed"))
    }
}

trait SymbolSpanDecodePolicyV1<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SymbolSpanFieldV1>, M::Error>;
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error>;
}

struct OrdinarySymbolSpanPolicyV1;

impl<'de> SymbolSpanDecodePolicyV1<'de> for OrdinarySymbolSpanPolicyV1 {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SymbolSpanFieldV1>, M::Error> {
        map.next_key::<String>()?
            .map(|key| symbol_span_field_v1(key.as_str()))
            .transpose()
    }

    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        let path: String = map.next_value()?;
        Ok(path.into_boxed_str())
    }
}

struct AdmittedSymbolSpanPolicyV1<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> SymbolSpanDecodePolicyV1<'de>
    for AdmittedSymbolSpanPolicyV1<'_, A>
{
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SymbolSpanFieldV1>, M::Error> {
        map.next_key_seed(SymbolSpanFieldSeedV1)
    }

    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        map.next_value_seed(AdmittedBoxedTextSeedV1::new(
            self.admission,
            self.max_text_bytes,
        ))
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedSymbolSpanPolicyV1<'_, A>
{
    type Value = SymbolSpan;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result =
            decoder.deserialize_struct("SymbolSpan", SYMBOL_SPAN_FIELDS, SymbolSpanVisitor(self));
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

impl SymbolSpan {
    /// Decode the existing span wire while retaining its path backing under admission.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        AdmittedSymbolSpanPolicyV1 {
            admission,
            max_text_bytes,
        }
    }
}

impl Serialize for SymbolSpan {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SymbolSpan", 5)?;
        state.serialize_field("path", self.path.as_ref())?;
        state.serialize_field("byte_start", &self.byte_start)?;
        state.serialize_field("byte_end", &self.byte_end)?;
        state.serialize_field("line_start", &self.line_start)?;
        state.serialize_field("line_end", &self.line_end)?;
        state.end()
    }
}

struct SymbolSpanVisitor<P>(P);

impl<'de, P: SymbolSpanDecodePolicyV1<'de>> Visitor<'de> for SymbolSpanVisitor<P> {
    type Value = SymbolSpan;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SymbolSpan map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut path: Option<Box<str>> = None;
        let mut byte_start: Option<u32> = None;
        let mut byte_end: Option<u32> = None;
        let mut line_start: Option<u32> = None;
        let mut line_end: Option<u32> = None;
        while let Some(key) = self.0.next_field(&mut map)? {
            match key {
                SymbolSpanFieldV1::Path => {
                    if path.is_some() {
                        return Err(de::Error::duplicate_field("path"));
                    }
                    path = Some(self.0.next_path(&mut map)?);
                }
                SymbolSpanFieldV1::ByteStart => {
                    if byte_start.is_some() {
                        return Err(de::Error::duplicate_field("byte_start"));
                    }
                    byte_start = Some(map.next_value()?);
                }
                SymbolSpanFieldV1::ByteEnd => {
                    if byte_end.is_some() {
                        return Err(de::Error::duplicate_field("byte_end"));
                    }
                    byte_end = Some(map.next_value()?);
                }
                SymbolSpanFieldV1::LineStart => {
                    if line_start.is_some() {
                        return Err(de::Error::duplicate_field("line_start"));
                    }
                    line_start = Some(map.next_value()?);
                }
                SymbolSpanFieldV1::LineEnd => {
                    if line_end.is_some() {
                        return Err(de::Error::duplicate_field("line_end"));
                    }
                    line_end = Some(map.next_value()?);
                }
            }
        }
        Ok(SymbolSpan {
            path: path.ok_or_else(|| de::Error::missing_field("path"))?,
            byte_start: byte_start.ok_or_else(|| de::Error::missing_field("byte_start"))?,
            byte_end: byte_end.ok_or_else(|| de::Error::missing_field("byte_end"))?,
            line_start: line_start.ok_or_else(|| de::Error::missing_field("line_start"))?,
            line_end: line_end.ok_or_else(|| de::Error::missing_field("line_end"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SymbolSpan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SymbolSpan",
            SYMBOL_SPAN_FIELDS,
            SymbolSpanVisitor(OrdinarySymbolSpanPolicyV1),
        )
    }
}

impl fmt::Display for SymbolSpan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}..{}:{}",
            self.path, self.line_start, self.byte_start, self.line_end, self.byte_end,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolRecord {
    pub symbol_id: SymbolId,
    pub repo_relative_path: RepoRelativePath,
    pub language: LanguageCode,
    pub symbol_kind: SymbolKindCode,
    pub symbol_kind_family: Option<SymbolKindFamily>,
    pub local_name: Box<str>,
    pub qualified_name: Box<str>,
    pub signature: Option<Box<str>>,
    pub visibility: Option<LqVisibility>,
    pub definition_span: SymbolSpan,
    pub container_qualified_name: Option<Box<str>>,
    pub relationship: SymbolRelationship,
}

impl SymbolRecord {
    /// Decode the existing wire with owned text retained by `admission`.
    /// The caller must retain the admission owner alongside the returned record.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        AdmittedSymbolRecordPolicyV1 {
            admission,
            max_text_bytes,
        }
    }
}

const SYMBOL_RECORD_FIELDS: &[&str] = &[
    "symbol_id",
    "repo_relative_path",
    "language",
    "symbol_kind",
    "symbol_kind_family",
    "local_name",
    "qualified_name",
    "signature",
    "visibility",
    "definition_span",
    "container_qualified_name",
    "relationship",
];

#[derive(Clone, Copy)]
enum SymbolRecordFieldV1 {
    SymbolId,
    RepoRelativePath,
    Language,
    SymbolKind,
    SymbolKindFamily,
    LocalName,
    QualifiedName,
    Signature,
    Visibility,
    DefinitionSpan,
    ContainerQualifiedName,
    Relationship,
}

fn symbol_record_field_v1<E: de::Error>(key: &str) -> Result<SymbolRecordFieldV1, E> {
    match key {
        "symbol_id" => Ok(SymbolRecordFieldV1::SymbolId),
        "repo_relative_path" => Ok(SymbolRecordFieldV1::RepoRelativePath),
        "language" => Ok(SymbolRecordFieldV1::Language),
        "symbol_kind" => Ok(SymbolRecordFieldV1::SymbolKind),
        "symbol_kind_family" => Ok(SymbolRecordFieldV1::SymbolKindFamily),
        "local_name" => Ok(SymbolRecordFieldV1::LocalName),
        "qualified_name" => Ok(SymbolRecordFieldV1::QualifiedName),
        "signature" => Ok(SymbolRecordFieldV1::Signature),
        "visibility" => Ok(SymbolRecordFieldV1::Visibility),
        "definition_span" => Ok(SymbolRecordFieldV1::DefinitionSpan),
        "container_qualified_name" => Ok(SymbolRecordFieldV1::ContainerQualifiedName),
        "relationship" => Ok(SymbolRecordFieldV1::Relationship),
        other => Err(E::unknown_field(other, SYMBOL_RECORD_FIELDS)),
    }
}

struct SymbolRecordFieldSeedV1;

impl<'de> DeserializeSeed<'de> for SymbolRecordFieldSeedV1 {
    type Value = SymbolRecordFieldV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for SymbolRecordFieldSeedV1 {
    type Value = SymbolRecordFieldV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed SymbolRecord field name")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        symbol_record_field_v1(key)
    }

    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom(
            "admitted symbol record field key must be borrowed",
        ))
    }
}

trait SymbolRecordDecodePolicyV1<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SymbolRecordFieldV1>, M::Error>;
    fn next_symbol_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SymbolId, M::Error>;
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error>;
    fn next_language<M: MapAccess<'de>>(&self, map: &mut M) -> Result<LanguageCode, M::Error>;
    fn next_kind<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SymbolKindCode, M::Error>;
    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error>;
    fn next_optional_text<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<Box<str>>, M::Error>;
    fn next_span<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SymbolSpan, M::Error>;
}

struct OrdinarySymbolRecordPolicyV1;

impl<'de> SymbolRecordDecodePolicyV1<'de> for OrdinarySymbolRecordPolicyV1 {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SymbolRecordFieldV1>, M::Error> {
        map.next_key::<String>()?
            .map(|key| symbol_record_field_v1(key.as_str()))
            .transpose()
    }

    fn next_symbol_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SymbolId, M::Error> {
        map.next_value()
    }

    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error> {
        map.next_value()
    }

    fn next_language<M: MapAccess<'de>>(&self, map: &mut M) -> Result<LanguageCode, M::Error> {
        map.next_value()
    }

    fn next_kind<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SymbolKindCode, M::Error> {
        map.next_value()
    }

    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        let text: String = map.next_value()?;
        Ok(text.into_boxed_str())
    }

    fn next_optional_text<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<Box<str>>, M::Error> {
        let text: Option<String> = map.next_value()?;
        Ok(text.map(String::into_boxed_str))
    }

    fn next_span<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SymbolSpan, M::Error> {
        map.next_value()
    }
}

struct AdmittedSymbolRecordPolicyV1<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> SymbolRecordDecodePolicyV1<'de>
    for AdmittedSymbolRecordPolicyV1<'_, A>
{
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SymbolRecordFieldV1>, M::Error> {
        map.next_key_seed(SymbolRecordFieldSeedV1)
    }

    fn next_symbol_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SymbolId, M::Error> {
        map.next_value_seed(SymbolId::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_text_bytes,
        ))
    }

    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error> {
        map.next_value_seed(RepoRelativePath::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_text_bytes,
        ))
    }

    fn next_language<M: MapAccess<'de>>(&self, map: &mut M) -> Result<LanguageCode, M::Error> {
        map.next_value_seed(LanguageCode::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_text_bytes,
        ))
    }

    fn next_kind<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SymbolKindCode, M::Error> {
        map.next_value_seed(SymbolKindCode::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_text_bytes,
        ))
    }

    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        map.next_value_seed(AdmittedBoxedTextSeedV1::new(
            self.admission,
            self.max_text_bytes,
        ))
    }

    fn next_optional_text<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<Box<str>>, M::Error> {
        map.next_value_seed(AdmittedOptionalSeedV1::new(
            self.admission,
            AdmittedBoxedTextSeedV1::new(self.admission, self.max_text_bytes),
        ))
    }

    fn next_span<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SymbolSpan, M::Error> {
        map.next_value_seed(SymbolSpan::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_text_bytes,
        ))
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedSymbolRecordPolicyV1<'_, A>
{
    type Value = SymbolRecord;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_struct(
            "SymbolRecord",
            SYMBOL_RECORD_FIELDS,
            SymbolRecordVisitor(self),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

impl Serialize for SymbolRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SymbolRecord", 12)?;
        state.serialize_field("symbol_id", &self.symbol_id)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("language", &self.language)?;
        state.serialize_field("symbol_kind", &self.symbol_kind)?;
        state.serialize_field("symbol_kind_family", &self.symbol_kind_family)?;
        state.serialize_field("local_name", self.local_name.as_ref())?;
        state.serialize_field("qualified_name", self.qualified_name.as_ref())?;
        state.serialize_field("signature", &self.signature.as_deref())?;
        state.serialize_field("visibility", &self.visibility)?;
        state.serialize_field("definition_span", &self.definition_span)?;
        state.serialize_field(
            "container_qualified_name",
            &self.container_qualified_name.as_deref(),
        )?;
        state.serialize_field("relationship", &self.relationship)?;
        state.end()
    }
}

struct SymbolRecordVisitor<P>(P);

impl<'de, P: SymbolRecordDecodePolicyV1<'de>> Visitor<'de> for SymbolRecordVisitor<P> {
    type Value = SymbolRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SymbolRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut symbol_id: Option<SymbolId> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut language: Option<LanguageCode> = None;
        let mut symbol_kind: Option<SymbolKindCode> = None;
        let mut symbol_kind_family: Option<Option<SymbolKindFamily>> = None;
        let mut local_name: Option<Box<str>> = None;
        let mut qualified_name: Option<Box<str>> = None;
        let mut signature: Option<Option<Box<str>>> = None;
        let mut visibility: Option<Option<LqVisibility>> = None;
        let mut definition_span: Option<SymbolSpan> = None;
        let mut container_qualified_name: Option<Option<Box<str>>> = None;
        let mut relationship: Option<SymbolRelationship> = None;
        while let Some(key) = self.0.next_field(&mut map)? {
            match key {
                SymbolRecordFieldV1::SymbolId => {
                    if symbol_id.is_some() {
                        return Err(de::Error::duplicate_field("symbol_id"));
                    }
                    symbol_id = Some(self.0.next_symbol_id(&mut map)?);
                }
                SymbolRecordFieldV1::RepoRelativePath => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(self.0.next_path(&mut map)?);
                }
                SymbolRecordFieldV1::Language => {
                    if language.is_some() {
                        return Err(de::Error::duplicate_field("language"));
                    }
                    language = Some(self.0.next_language(&mut map)?);
                }
                SymbolRecordFieldV1::SymbolKind => {
                    if symbol_kind.is_some() {
                        return Err(de::Error::duplicate_field("symbol_kind"));
                    }
                    symbol_kind = Some(self.0.next_kind(&mut map)?);
                }
                SymbolRecordFieldV1::SymbolKindFamily => {
                    if symbol_kind_family.is_some() {
                        return Err(de::Error::duplicate_field("symbol_kind_family"));
                    }
                    symbol_kind_family = Some(map.next_value()?);
                }
                SymbolRecordFieldV1::LocalName => {
                    if local_name.is_some() {
                        return Err(de::Error::duplicate_field("local_name"));
                    }
                    local_name = Some(self.0.next_text(&mut map)?);
                }
                SymbolRecordFieldV1::QualifiedName => {
                    if qualified_name.is_some() {
                        return Err(de::Error::duplicate_field("qualified_name"));
                    }
                    qualified_name = Some(self.0.next_text(&mut map)?);
                }
                SymbolRecordFieldV1::Signature => {
                    if signature.is_some() {
                        return Err(de::Error::duplicate_field("signature"));
                    }
                    signature = Some(self.0.next_optional_text(&mut map)?);
                }
                SymbolRecordFieldV1::Visibility => {
                    if visibility.is_some() {
                        return Err(de::Error::duplicate_field("visibility"));
                    }
                    visibility = Some(map.next_value()?);
                }
                SymbolRecordFieldV1::DefinitionSpan => {
                    if definition_span.is_some() {
                        return Err(de::Error::duplicate_field("definition_span"));
                    }
                    definition_span = Some(self.0.next_span(&mut map)?);
                }
                SymbolRecordFieldV1::ContainerQualifiedName => {
                    if container_qualified_name.is_some() {
                        return Err(de::Error::duplicate_field("container_qualified_name"));
                    }
                    container_qualified_name = Some(self.0.next_optional_text(&mut map)?);
                }
                SymbolRecordFieldV1::Relationship => {
                    if relationship.is_some() {
                        return Err(de::Error::duplicate_field("relationship"));
                    }
                    relationship = Some(map.next_value()?);
                }
            }
        }
        Ok(SymbolRecord {
            symbol_id: symbol_id.ok_or_else(|| de::Error::missing_field("symbol_id"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            language: language.ok_or_else(|| de::Error::missing_field("language"))?,
            symbol_kind: symbol_kind.ok_or_else(|| de::Error::missing_field("symbol_kind"))?,
            symbol_kind_family: symbol_kind_family
                .ok_or_else(|| de::Error::missing_field("symbol_kind_family"))?,
            local_name: local_name.ok_or_else(|| de::Error::missing_field("local_name"))?,
            qualified_name: qualified_name
                .ok_or_else(|| de::Error::missing_field("qualified_name"))?,
            signature: signature.ok_or_else(|| de::Error::missing_field("signature"))?,
            visibility: visibility.ok_or_else(|| de::Error::missing_field("visibility"))?,
            definition_span: definition_span
                .ok_or_else(|| de::Error::missing_field("definition_span"))?,
            container_qualified_name: container_qualified_name
                .ok_or_else(|| de::Error::missing_field("container_qualified_name"))?,
            relationship: relationship.ok_or_else(|| de::Error::missing_field("relationship"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SymbolRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SymbolRecord",
            SYMBOL_RECORD_FIELDS,
            SymbolRecordVisitor(OrdinarySymbolRecordPolicyV1),
        )
    }
}

impl fmt::Display for SymbolRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} @ {} ({}/{})",
            self.symbol_kind,
            self.qualified_name,
            self.definition_span,
            self.language,
            self.relationship,
        )
    }
}

#[cfg(test)]
mod admitted_symbol_record_tests;
