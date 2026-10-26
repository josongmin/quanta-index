//! Admitted decode policy for the existing commit wire visitor.

use core::fmt;

use quanta_index_contract_base::{
    AdmittedBoxedTextSeedV1, AdmittedOptionalSeedV1, AdmittedSequenceSeedV1,
    DecodeBackingAdmissionV1,
};
use serde::Deserializer;
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};

use super::{COMMIT_RECORD_FIELDS, CommitRecord, CommitRecordVisitor, CommitSha};

#[derive(Clone, Copy)]
pub(super) struct Ordinary;

#[derive(Clone, Copy)]
struct Admitted<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
    max_items: usize,
}

struct FieldSeed;

impl<'de> DeserializeSeed<'de> for FieldSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for FieldSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed commit field name")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        COMMIT_RECORD_FIELDS
            .iter()
            .copied()
            .find(|field| *field == value)
            .ok_or_else(|| E::unknown_field(value, COMMIT_RECORD_FIELDS))
    }

    fn visit_string<E: de::Error>(self, _value: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted commit field key must be borrowed"))
    }
}

struct ShaSeed<'a, A: ?Sized>(&'a A);

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de> for ShaSeed<'_, A> {
    type Value = CommitSha;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.0;
        let result = decoder.deserialize_str(self);
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

impl<A: ?Sized> Visitor<'_> for ShaSeed<'_, A> {
    type Value = CommitSha;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed 40-char hex CommitSha string")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        CommitSha::from_hex(value).map_err(E::custom)
    }

    fn visit_string<E: de::Error>(self, _value: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted CommitSha must be borrowed"))
    }
}

impl CommitSha {
    /// Parse the fixed-size wire spelling without a heap-backed temporary.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        ShaSeed(admission)
    }
}

pub(super) trait CommitDecodePolicy<'de> {
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error>;
    fn next_sha<M: MapAccess<'de>>(&self, map: &mut M) -> Result<CommitSha, M::Error>;
    fn next_parents<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<CommitSha>, M::Error>;
    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error>;
    fn next_optional_text<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<Box<str>>, M::Error>;
    fn next_tags<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<Box<str>>, M::Error>;
}

impl<'de> CommitDecodePolicy<'de> for Ordinary {
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error> {
        map.next_key::<String>()?
            .map(|key| {
                COMMIT_RECORD_FIELDS
                    .iter()
                    .copied()
                    .find(|field| *field == key)
                    .ok_or_else(|| de::Error::unknown_field(&key, COMMIT_RECORD_FIELDS))
            })
            .transpose()
    }

    fn next_sha<M: MapAccess<'de>>(&self, map: &mut M) -> Result<CommitSha, M::Error> {
        map.next_value()
    }

    fn next_parents<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<CommitSha>, M::Error> {
        map.next_value()
    }

    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        Ok(map.next_value::<String>()?.into_boxed_str())
    }

    fn next_optional_text<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<Box<str>>, M::Error> {
        Ok(map
            .next_value::<Option<String>>()?
            .map(String::into_boxed_str))
    }

    fn next_tags<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<Box<str>>, M::Error> {
        Ok(map
            .next_value::<Vec<String>>()?
            .into_iter()
            .map(String::into_boxed_str)
            .collect())
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> CommitDecodePolicy<'de> for Admitted<'_, A> {
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error> {
        map.next_key_seed(FieldSeed)
    }

    fn next_sha<M: MapAccess<'de>>(&self, map: &mut M) -> Result<CommitSha, M::Error> {
        map.next_value_seed(CommitSha::deserialize_seed_with_admission_v1(
            self.admission,
        ))
    }

    fn next_parents<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<CommitSha>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.max_items,
            || CommitSha::deserialize_seed_with_admission_v1(self.admission),
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

    fn next_tags<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<Box<str>>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.max_items,
            || AdmittedBoxedTextSeedV1::new(self.admission, self.max_text_bytes),
        ))
    }
}

impl CommitRecord {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
        max_items: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        Admitted {
            admission,
            max_text_bytes,
            max_items,
        }
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de> for Admitted<'_, A> {
    type Value = CommitRecord;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_struct(
            "CommitRecord",
            COMMIT_RECORD_FIELDS,
            CommitRecordVisitor(self),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}
