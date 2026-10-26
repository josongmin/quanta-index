//! Ordinary and admitted decoding share each record's closed-map visitor.

use core::fmt;

use quanta_index_contract_base::{
    AdmittedOptionalSeedV1, AdmittedTextSeedV1, DecodeBackingAdmissionV1, SourceFileRevision,
    SourceIdentityDecodeAdmissionV1,
};
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::{SourceFileCoverage, SourcePublicationEvent, SymbolCoverage, SymbolNameSourcePolicyV1};
use crate::lex::LanguageCode;

const SYMBOL_FIELDS: &[&str] = &["state", "symbol_count"];
const SYMBOL_STATES: &[&str] = &[
    "complete",
    "not_requested",
    "unsupported",
    "parse_failed",
    "producer_failed",
];
const COVERAGE_FIELDS: &[&str] = &[
    "source",
    "language",
    "producer_policy_sha256",
    "symbol_name_source_policy",
    "unit_set_sha256",
    "text_admitted",
    "symbols",
];
const EVENT_FIELDS: &[&str] = &[
    "stream_id",
    "event_id",
    "expected_base_event_id",
    "payload_sha256",
];

fn known_field<E: de::Error>(
    key: &str,
    fields: &'static [&'static str],
) -> Result<&'static str, E> {
    fields
        .iter()
        .copied()
        .find(|field| *field == key)
        .ok_or_else(|| E::unknown_field(key, fields))
}

struct FieldSeed(&'static [&'static str]);

impl<'de> DeserializeSeed<'de> for FieldSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for FieldSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a known source coverage field name")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        known_field(key, self.0)
    }

    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom(
            "admitted source coverage field key must be borrowed",
        ))
    }
}

trait FieldPolicy<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error>;
}

struct Ordinary;

impl<'de> FieldPolicy<'de> for Ordinary {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error> {
        map.next_key::<String>()?
            .map(|key| known_field(key.as_str(), fields))
            .transpose()
    }
}

struct Admitted<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> FieldPolicy<'de> for Admitted<'_, A> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error> {
        map.next_key_seed(FieldSeed(fields))
    }
}

fn known_symbol_state<E: de::Error>(state: &str) -> Result<&'static str, E> {
    SYMBOL_STATES
        .iter()
        .copied()
        .find(|candidate| *candidate == state)
        .ok_or_else(|| E::unknown_variant(state, SYMBOL_STATES))
}

struct SymbolStateSeed;

impl<'de> DeserializeSeed<'de> for SymbolStateSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_str(self)
    }
}

impl Visitor<'_> for SymbolStateSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a known symbol coverage state")
    }

    fn visit_str<E: de::Error>(self, state: &str) -> Result<Self::Value, E> {
        known_symbol_state(state)
    }

    fn visit_string<E: de::Error>(self, _state: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted symbol coverage state must be borrowed"))
    }
}

trait SymbolPolicy<'de>: FieldPolicy<'de> {
    fn next_state<M: MapAccess<'de>>(&self, map: &mut M) -> Result<&'static str, M::Error>;
}

impl<'de> SymbolPolicy<'de> for Ordinary {
    fn next_state<M: MapAccess<'de>>(&self, map: &mut M) -> Result<&'static str, M::Error> {
        let state: String = map.next_value()?;
        known_symbol_state(&state)
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> SymbolPolicy<'de> for Admitted<'_, A> {
    fn next_state<M: MapAccess<'de>>(&self, map: &mut M) -> Result<&'static str, M::Error> {
        map.next_value_seed(SymbolStateSeed)
    }
}

struct SymbolVisitor<P>(P);

impl<'de, P: SymbolPolicy<'de>> Visitor<'de> for SymbolVisitor<P> {
    type Value = SymbolCoverage;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an explicit symbol coverage state")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut state = None;
        let mut count = None;
        while let Some(field) = self.0.next_field(&mut map, SYMBOL_FIELDS)? {
            match field {
                "state" => {
                    if state.is_some() {
                        return Err(de::Error::duplicate_field("state"));
                    }
                    state = Some(self.0.next_state(&mut map)?);
                }
                "symbol_count" => {
                    if count.is_some() {
                        return Err(de::Error::duplicate_field("symbol_count"));
                    }
                    count = Some(map.next_value::<u64>()?);
                }
                _ => return Err(de::Error::custom("invalid symbol coverage field")),
            }
        }
        let state = state.ok_or_else(|| de::Error::missing_field("state"))?;
        match state {
            "complete" => Ok(SymbolCoverage::Complete {
                symbol_count: count.ok_or_else(|| de::Error::missing_field("symbol_count"))?,
            }),
            "not_requested" | "unsupported" | "parse_failed" | "producer_failed"
                if count.is_some() =>
            {
                Err(de::Error::custom(
                    "only complete coverage has a symbol_count",
                ))
            }
            "not_requested" => Ok(SymbolCoverage::NotRequested),
            "unsupported" => Ok(SymbolCoverage::Unsupported),
            "parse_failed" => Ok(SymbolCoverage::ParseFailed),
            "producer_failed" => Ok(SymbolCoverage::ProducerFailed),
            _ => Err(de::Error::custom("invalid symbol coverage state")),
        }
    }
}

impl<'de> Deserialize<'de> for SymbolCoverage {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_map(SymbolVisitor(Ordinary))
    }
}

impl SymbolCoverage {
    fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        Admitted {
            admission,
            max_text_bytes: 0,
        }
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de> for Admitted<'_, A> {
    type Value = SymbolCoverage;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_map(SymbolVisitor(self));
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

trait CoveragePolicy<'de>: FieldPolicy<'de> {
    fn next_source<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SourceFileRevision, M::Error>;
    fn next_language<M: MapAccess<'de>>(&self, map: &mut M) -> Result<LanguageCode, M::Error>;
    fn next_symbols<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SymbolCoverage, M::Error>;
}

impl<'de> CoveragePolicy<'de> for Ordinary {
    fn next_source<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SourceFileRevision, M::Error> {
        map.next_value()
    }

    fn next_language<M: MapAccess<'de>>(&self, map: &mut M) -> Result<LanguageCode, M::Error> {
        map.next_value()
    }

    fn next_symbols<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SymbolCoverage, M::Error> {
        map.next_value()
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> CoveragePolicy<'de> for Admitted<'_, A> {
    fn next_source<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SourceFileRevision, M::Error> {
        map.next_value_seed(SourceFileRevision::deserialize_seed_with_admission_v1(
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

    fn next_symbols<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SymbolCoverage, M::Error> {
        map.next_value_seed(SymbolCoverage::deserialize_seed_with_admission_v1(
            self.admission,
        ))
    }
}

struct CoverageVisitor<P>(P);

impl<'de, P: CoveragePolicy<'de>> Visitor<'de> for CoverageVisitor<P> {
    type Value = SourceFileCoverage;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a complete SourceFileCoverage map")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut source = None;
        let mut language = None;
        let mut producer_policy_sha256 = None;
        let mut symbol_name_source_policy = None;
        let mut unit_set_sha256 = None;
        let mut text_admitted = None;
        let mut symbols = None;
        while let Some(field) = self.0.next_field(&mut map, COVERAGE_FIELDS)? {
            match field {
                "source" => {
                    if source.is_some() {
                        return Err(de::Error::duplicate_field("source"));
                    }
                    source = Some(self.0.next_source(&mut map)?);
                }
                "language" => {
                    if language.is_some() {
                        return Err(de::Error::duplicate_field("language"));
                    }
                    language = Some(self.0.next_language(&mut map)?);
                }
                "producer_policy_sha256" => {
                    if producer_policy_sha256.is_some() {
                        return Err(de::Error::duplicate_field("producer_policy_sha256"));
                    }
                    producer_policy_sha256 = Some(map.next_value::<[u8; 32]>()?);
                }
                "symbol_name_source_policy" => {
                    if symbol_name_source_policy.is_some() {
                        return Err(de::Error::duplicate_field("symbol_name_source_policy"));
                    }
                    symbol_name_source_policy = Some(map.next_value::<SymbolNameSourcePolicyV1>()?);
                }
                "unit_set_sha256" => {
                    if unit_set_sha256.is_some() {
                        return Err(de::Error::duplicate_field("unit_set_sha256"));
                    }
                    unit_set_sha256 = Some(map.next_value::<[u8; 32]>()?);
                }
                "text_admitted" => {
                    if text_admitted.is_some() {
                        return Err(de::Error::duplicate_field("text_admitted"));
                    }
                    text_admitted = Some(map.next_value::<bool>()?);
                }
                "symbols" => {
                    if symbols.is_some() {
                        return Err(de::Error::duplicate_field("symbols"));
                    }
                    symbols = Some(self.0.next_symbols(&mut map)?);
                }
                _ => return Err(de::Error::custom("invalid source coverage field")),
            }
        }
        let value = SourceFileCoverage {
            source: source.ok_or_else(|| de::Error::missing_field("source"))?,
            language: language.ok_or_else(|| de::Error::missing_field("language"))?,
            producer_policy_sha256: producer_policy_sha256
                .ok_or_else(|| de::Error::missing_field("producer_policy_sha256"))?,
            symbol_name_source_policy: symbol_name_source_policy
                .ok_or_else(|| de::Error::missing_field("symbol_name_source_policy"))?,
            unit_set_sha256: unit_set_sha256
                .ok_or_else(|| de::Error::missing_field("unit_set_sha256"))?,
            text_admitted: text_admitted
                .ok_or_else(|| de::Error::missing_field("text_admitted"))?,
            symbols: symbols.ok_or_else(|| de::Error::missing_field("symbols"))?,
        };
        value.validate().map_err(de::Error::custom)?;
        Ok(value)
    }
}

impl<'de> Deserialize<'de> for SourceFileCoverage {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_map(CoverageVisitor(Ordinary))
    }
}

struct AdmittedCoverageSeed<'a, A: ?Sized>(Admitted<'a, A>);

impl SourceFileCoverage {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    {
        AdmittedCoverageSeed(Admitted {
            admission,
            max_text_bytes,
        })
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedCoverageSeed<'_, A>
{
    type Value = SourceFileCoverage;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.0.admission;
        let result = decoder.deserialize_map(CoverageVisitor(self.0));
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

pub(super) fn valid_event_token_v1(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn validate_event_token_v1(value: &str) -> Result<(), &'static str> {
    if valid_event_token_v1(value) {
        Ok(())
    } else {
        Err("invalid source event token")
    }
}

trait EventPolicy<'de>: FieldPolicy<'de> {
    fn next_token<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error>;
    fn next_optional_token<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<String>, M::Error>;
}

impl<'de> EventPolicy<'de> for Ordinary {
    fn next_token<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value()
    }
    fn next_optional_token<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<String>, M::Error> {
        map.next_value()
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> EventPolicy<'de> for Admitted<'_, A> {
    fn next_token<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value_seed(AdmittedTextSeedV1::new_validated(
            self.admission,
            self.max_text_bytes.min(512),
            validate_event_token_v1,
        ))
    }

    fn next_optional_token<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<String>, M::Error> {
        map.next_value_seed(AdmittedOptionalSeedV1::new(
            self.admission,
            AdmittedTextSeedV1::new_validated(
                self.admission,
                self.max_text_bytes.min(512),
                validate_event_token_v1,
            ),
        ))
    }
}

struct EventVisitor<P>(P);

impl<'de, P: EventPolicy<'de>> Visitor<'de> for EventVisitor<P> {
    type Value = SourcePublicationEvent;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a complete SourcePublicationEvent map")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut stream_id = None;
        let mut event_id = None;
        let mut expected_base_event_id: Option<Option<String>> = None;
        let mut payload_sha256 = None;
        while let Some(field) = self.0.next_field(&mut map, EVENT_FIELDS)? {
            match field {
                "stream_id" => {
                    if stream_id.is_some() {
                        return Err(de::Error::duplicate_field("stream_id"));
                    }
                    stream_id = Some(self.0.next_token(&mut map)?);
                }
                "event_id" => {
                    if event_id.is_some() {
                        return Err(de::Error::duplicate_field("event_id"));
                    }
                    event_id = Some(self.0.next_token(&mut map)?);
                }
                "expected_base_event_id" => {
                    if expected_base_event_id.is_some() {
                        return Err(de::Error::duplicate_field("expected_base_event_id"));
                    }
                    expected_base_event_id = Some(self.0.next_optional_token(&mut map)?);
                }
                "payload_sha256" => {
                    if payload_sha256.is_some() {
                        return Err(de::Error::duplicate_field("payload_sha256"));
                    }
                    payload_sha256 = Some(map.next_value::<[u8; 32]>()?);
                }
                _ => return Err(de::Error::custom("invalid source event field")),
            }
        }
        let value = SourcePublicationEvent {
            stream_id: stream_id.ok_or_else(|| de::Error::missing_field("stream_id"))?,
            event_id: event_id.ok_or_else(|| de::Error::missing_field("event_id"))?,
            expected_base_event_id: expected_base_event_id
                .ok_or_else(|| de::Error::missing_field("expected_base_event_id"))?,
            payload_sha256: payload_sha256
                .ok_or_else(|| de::Error::missing_field("payload_sha256"))?,
        };
        value.validate().map_err(de::Error::custom)?;
        Ok(value)
    }
}

impl<'de> Deserialize<'de> for SourcePublicationEvent {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_map(EventVisitor(Ordinary))
    }
}

struct AdmittedEventSeed<'a, A: ?Sized>(Admitted<'a, A>);

impl SourcePublicationEvent {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        AdmittedEventSeed(Admitted {
            admission,
            max_text_bytes,
        })
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de> for AdmittedEventSeed<'_, A> {
    type Value = SourcePublicationEvent;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.0.admission;
        let result = decoder.deserialize_map(EventVisitor(self.0));
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

#[cfg(test)]
mod tests;
