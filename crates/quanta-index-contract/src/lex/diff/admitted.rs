//! Admitted text policy for the existing diff hunk visitor.

use core::fmt;

use quanta_index_contract_base::{AdmittedBoxedTextSeedV1, DecodeBackingAdmissionV1};
use serde::Deserializer;
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};

use super::{DIFF_HUNK_RECORD_FIELDS, DiffHunkRecord, DiffHunkRecordVisitor};

pub(super) struct Ordinary;

struct Admitted<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
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
        formatter.write_str("a borrowed diff hunk field name")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        DIFF_HUNK_RECORD_FIELDS
            .iter()
            .copied()
            .find(|field| *field == value)
            .ok_or_else(|| E::unknown_field(value, DIFF_HUNK_RECORD_FIELDS))
    }

    fn visit_string<E: de::Error>(self, _value: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted diff hunk field key must be borrowed"))
    }
}

pub(super) trait DiffDecodePolicy<'de> {
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error>;
    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error>;
}

impl<'de> DiffDecodePolicy<'de> for Ordinary {
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error> {
        map.next_key::<String>()?
            .map(|key| {
                DIFF_HUNK_RECORD_FIELDS
                    .iter()
                    .copied()
                    .find(|field| *field == key)
                    .ok_or_else(|| de::Error::unknown_field(&key, DIFF_HUNK_RECORD_FIELDS))
            })
            .transpose()
    }

    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        Ok(map.next_value::<String>()?.into_boxed_str())
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DiffDecodePolicy<'de> for Admitted<'_, A> {
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error> {
        map.next_key_seed(FieldSeed)
    }

    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        map.next_value_seed(AdmittedBoxedTextSeedV1::new(
            self.admission,
            self.max_text_bytes,
        ))
    }
}

impl DiffHunkRecord {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        Admitted {
            admission,
            max_text_bytes,
        }
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de> for Admitted<'_, A> {
    type Value = DiffHunkRecord;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_struct(
            "DiffHunkRecord",
            DIFF_HUNK_RECORD_FIELDS,
            DiffHunkRecordVisitor(self),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}
