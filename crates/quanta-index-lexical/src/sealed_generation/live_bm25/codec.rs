//! Bounded format-14 live-statistics serialization and validation.

use std::collections::BTreeMap;
use std::io::Write;
use std::marker::PhantomData;
use std::path::Path;

use quanta_index_core::CoreError;
use serde::de::{self, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeSeq, Serializer};
use serde::{Deserialize, Deserializer};
use tantivy::Searcher;
use tantivy::schema::{Field, FieldType};

use super::{
    LiveBm25Statistics, MAX_BYTES, MAX_RESIDENT, SegmentIdentity, SegmentStatistics, corrupt,
    push_segment, segment_identity,
};

const LIVE_BM25_FORMAT_VERSION: u32 = 1;

type FieldRow = (u32, u64);
type CorrectionRow = (u32, Vec<u8>, u64);
#[cfg(test)]
type SegmentWire = (SegmentIdentity, Vec<FieldRow>, Vec<CorrectionRow>);
#[cfg(test)]
pub(super) type Wire = (u32, [u8; 32], u64, Vec<SegmentWire>);
type SegmentWireDecode = (
    SegmentIdentity,
    BoundedRows<FieldRow, 64, MAX_BYTES>,
    BoundedRows<CorrectionRow, MAX_CORRECTION_ROWS, MAX_BYTES>,
);
type WireDecode = (
    u32,
    [u8; 32],
    u64,
    BoundedRows<SegmentWireDecode, MAX_SEGMENT_ROWS, MAX_RESIDENT>,
);

/// The wire row bounds derive from the decoded statistics ceiling.
const MAX_CORRECTION_ROWS: usize = MAX_RESIDENT.div_euclid(96);
const MAX_SEGMENT_ROWS: usize = MAX_RESIDENT.div_euclid(96);

trait HeapCharge {
    fn heap_charge(&self) -> Option<usize>;
}

impl HeapCharge for FieldRow {
    fn heap_charge(&self) -> Option<usize> {
        Some(32)
    }
}

impl HeapCharge for CorrectionRow {
    fn heap_charge(&self) -> Option<usize> {
        self.1.len().checked_add(96)
    }
}

impl HeapCharge for SegmentWireDecode {
    fn heap_charge(&self) -> Option<usize> {
        self.0
            .0
            .len()
            .checked_add(96)?
            .checked_add(self.1.charge)?
            .checked_add(self.2.charge)
    }
}

struct BoundedRows<T, const MAX: usize, const CHARGE: usize> {
    rows: Vec<T>,
    charge: usize,
}

struct BoundedWriter(Vec<u8>);

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self
            .0
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("live BM25 encoded length overflow"))?;
        if next > MAX_BYTES {
            return Err(std::io::Error::other("live BM25 encoded limit exceeded"));
        }
        self.0
            .try_reserve(bytes.len())
            .map_err(std::io::Error::other)?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<T, const MAX: usize, const CHARGE: usize> BoundedRows<T, MAX, CHARGE> {
    fn len(&self) -> usize {
        self.rows.len()
    }
}

impl<T, const MAX: usize, const CHARGE: usize> IntoIterator for BoundedRows<T, MAX, CHARGE> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.rows.into_iter()
    }
}

impl<'de, T: Deserialize<'de> + HeapCharge, const MAX: usize, const CHARGE: usize> Deserialize<'de>
    for BoundedRows<T, MAX, CHARGE>
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowsVisitor<T, const MAX: usize, const CHARGE: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de> + HeapCharge, const MAX: usize, const CHARGE: usize>
            Visitor<'de> for RowsVisitor<T, MAX, CHARGE>
        {
            type Value = BoundedRows<T, MAX, CHARGE>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "bounded live BM25 rows")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                if access.size_hint().is_some_and(|hint| hint > MAX) {
                    return Err(de::Error::custom(
                        "live BM25 row count exceeds resident limit",
                    ));
                }
                let mut rows = Vec::new();
                let mut charge = 0_usize;
                while rows.len() < MAX {
                    let Some(row) = access.next_element::<T>()? else {
                        return Ok(BoundedRows { rows, charge });
                    };
                    charge = charge
                        .checked_add(
                            row.heap_charge().ok_or_else(|| {
                                de::Error::custom("live BM25 row charge overflow")
                            })?,
                        )
                        .ok_or_else(|| de::Error::custom("live BM25 resident charge overflow"))?;
                    if charge > CHARGE {
                        return Err(de::Error::custom("live BM25 resident limit exceeded"));
                    }
                    rows.try_reserve(1).map_err(de::Error::custom)?;
                    rows.push(row);
                }
                if access.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom(
                        "live BM25 row count exceeds resident limit",
                    ));
                }
                Ok(BoundedRows { rows, charge })
            }
        }
        deserializer.deserialize_seq(RowsVisitor::<T, MAX, CHARGE>(PhantomData))
    }
}

struct FieldRows<'a>(&'a BTreeMap<u32, u64>);
struct CorrectionRows<'a>(&'a BTreeMap<(u32, Vec<u8>), u64>);
struct SegmentRows<'a>(&'a [SegmentStatistics]);

impl Serialize for FieldRows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut rows = serializer.serialize_seq(Some(self.0.len()))?;
        for (&field, &tokens) in self.0 {
            rows.serialize_element(&(field, tokens))?;
        }
        rows.end()
    }
}

impl Serialize for CorrectionRows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut rows = serializer.serialize_seq(Some(self.0.len()))?;
        for ((field, term), &count) in self.0 {
            rows.serialize_element(&(*field, term, count))?;
        }
        rows.end()
    }
}

impl Serialize for SegmentRows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut rows = serializer.serialize_seq(Some(self.0.len()))?;
        for segment in self.0 {
            rows.serialize_element(&(
                &segment.identity,
                FieldRows(&segment.field_tokens),
                CorrectionRows(&segment.dead_df),
            ))?;
        }
        rows.end()
    }
}

impl LiveBm25Statistics {
    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        let row = (
            LIVE_BM25_FORMAT_VERSION,
            self.index_meta_digest,
            self.live_docs,
            SegmentRows(&self.segments),
        );
        let mut writer = BoundedWriter(Vec::new());
        ciborium::into_writer(&row, &mut writer).map_err(|error| {
            CoreError::InvalidContract(format!("lexical: encode live BM25 statistics: {error}"))
        })?;
        Ok(writer.0)
    }

    pub(crate) fn decode(
        bytes: &[u8],
        dir: &Path,
        meta_digest: [u8; 32],
        searcher: &Searcher,
    ) -> Result<Self, CoreError> {
        if bytes.len() > MAX_BYTES {
            return Err(corrupt(dir, "exceeds the committed control-file limit"));
        }
        let (format, digest, live_docs, rows): WireDecode =
            crate::channel_payloads::decode_cbor_exact(bytes)
                .map_err(|error| corrupt(dir, &format!("decode: {error}")))?;
        if format != LIVE_BM25_FORMAT_VERSION
            || digest != meta_digest
            || live_docs != searcher.num_docs()
            || rows.len() != searcher.segment_readers().len()
        {
            return Err(corrupt(dir, "index commit or segment count differs"));
        }
        let schema = searcher.schema();
        let indexed: Vec<_> = schema
            .fields()
            .filter_map(|(field, entry)| {
                entry
                    .field_type()
                    .get_index_record_option()
                    .map(|_| field.field_id())
            })
            .collect();
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(rows.len())
            .map_err(|error| corrupt(dir, &format!("segment allocation: {error}")))?;
        let mut admitted = 0_usize;
        for ((identity, fields, corrections), segment) in
            rows.into_iter().zip(searcher.segment_readers())
        {
            if identity != segment_identity(segment) {
                return Err(corrupt(dir, "source segment deletion identity differs"));
            }
            let mut field_tokens = BTreeMap::new();
            let mut previous_field = None;
            for (field, tokens) in fields {
                if previous_field.is_some_and(|previous| previous >= field)
                    || indexed.binary_search(&field).is_err()
                {
                    return Err(corrupt(dir, "field statistics are not canonical"));
                }
                previous_field = Some(field);
                let _old = field_tokens.insert(field, tokens);
            }
            if field_tokens.len() != indexed.len() {
                return Err(corrupt(dir, "indexed field statistics are incomplete"));
            }
            let mut dead_df = BTreeMap::new();
            let mut previous_key: Option<(u32, Vec<u8>)> = None;
            let field_charge = field_tokens
                .len()
                .checked_mul(32)
                .and_then(|bytes| bytes.checked_add(96))
                .ok_or_else(|| corrupt(dir, "field resident charge overflow"))?;
            let mut segment_charge = identity
                .0
                .len()
                .checked_add(field_charge)
                .ok_or_else(|| corrupt(dir, "segment resident charge overflow"))?;
            for (field, term, count) in corrections {
                segment_charge = term
                    .len()
                    .checked_add(96)
                    .and_then(|bytes| segment_charge.checked_add(bytes))
                    .ok_or_else(|| corrupt(dir, "correction resident charge overflow"))?;
                if segment_charge > MAX_BYTES
                    || admitted
                        .checked_add(segment_charge)
                        .is_none_or(|total| total > MAX_RESIDENT)
                {
                    return Err(corrupt(dir, "statistics resident limit exceeded"));
                }
                let key = (field, term);
                if !field_tokens.contains_key(&field) {
                    return Err(corrupt(dir, "correction field is not indexed"));
                }
                let canonical_term = match schema
                    .get_field_entry(Field::from_field_id(field))
                    .field_type()
                {
                    FieldType::Str(_) => {
                        key.1.len() <= tantivy::tokenizer::MAX_TOKEN_LEN
                            && std::str::from_utf8(&key.1).is_ok()
                    }
                    FieldType::U64(_) => key.1.len() == 8,
                    FieldType::I64(_)
                    | FieldType::F64(_)
                    | FieldType::Bool(_)
                    | FieldType::Date(_)
                    | FieldType::Facet(_)
                    | FieldType::Bytes(_)
                    | FieldType::JsonObject(_)
                    | FieldType::IpAddr(_) => false,
                };
                if previous_key
                    .as_ref()
                    .is_some_and(|previous| previous >= &key)
                    || count == 0
                    || !canonical_term
                    || !segment.has_deletes()
                {
                    return Err(corrupt(dir, "frequency corrections are not canonical"));
                }
                previous_key = Some(key.clone());
                // A bounded term-dictionary lookup validates each sparse
                // correction at open. No postings walk occurs on a query.
                let inverted = segment
                    .inverted_index(Field::from_field_id(field))
                    .map_err(|error| corrupt(dir, &format!("correction field: {error}")))?;
                let raw = inverted
                    .terms()
                    .get(&key.1)
                    .map_err(|error| corrupt(dir, &format!("corrected term: {error}")))?
                    .map_or(0_u64, |info| u64::from(info.doc_freq));
                if count > raw {
                    return Err(corrupt(dir, "dead df correction exceeds raw df"));
                }
                let _old = dead_df.insert(key, count);
            }
            push_segment(
                &mut segments,
                &mut admitted,
                SegmentStatistics {
                    identity,
                    field_tokens,
                    dead_df,
                },
            )
            .map_err(|error| corrupt(dir, &error.to_string()))?;
        }
        Self::from_segments(digest, live_docs, segments, &indexed, bytes.len())
    }
}
