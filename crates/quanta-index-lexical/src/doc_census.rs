//! Per-document indexed-term census captured before native indexing.
//!
//! The terms come from the exact `TantivyDocument` values and the registered
//! schema analyzer that Tantivy will consume. A later delta can subtract a
//! newly retired document without reading unrelated postings or guessing
//! from partially stored fields.

#![expect(
    clippy::redundant_pub_crate,
    reason = "this module is private to the lexical crate"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::marker::PhantomData;
use std::time::Instant;

use quanta_index_core::CoreError;
use serde::de::{self, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeSeq, Serializer};
use serde::{Deserialize, Deserializer};
use tantivy::schema::{Field, FieldType, TantivyDocument, Value as _};
use tantivy::tokenizer::TokenStream as _;
use tantivy::{Index, Term};

use crate::SchemaFields;

const DOC_CENSUS_FORMAT_VERSION: u32 = 1;
/// One document's census is part of the bounded native store payload.
/// Crossing this limit fails at the producer before writer admission.
const MAX_BYTES: usize = crate::sealed_generation::MAX_INDEX_CONTROL_BYTES;
type FieldWireDecode = (u32, u64, BoundedRows<Vec<u8>, MAX_TERMS>);
type WireDecode = (u32, BoundedRows<FieldWireDecode, 64>);
const MAX_HEAP: usize = MAX_BYTES * 4;
const MAX_TERMS: usize = MAX_BYTES.div_euclid(32);

trait HeapCharge {
    fn heap_charge(&self) -> Option<usize>;
}

impl HeapCharge for Vec<u8> {
    fn heap_charge(&self) -> Option<usize> {
        self.len().checked_add(96)
    }
}

impl HeapCharge for FieldWireDecode {
    fn heap_charge(&self) -> Option<usize> {
        self.2.charge.checked_add(48)
    }
}

struct BoundedRows<T, const MAX: usize> {
    rows: Vec<T>,
    charge: usize,
}

impl<T, const MAX: usize> BoundedRows<T, MAX> {
    fn len(&self) -> usize {
        self.rows.len()
    }
}

impl<T, const MAX: usize> IntoIterator for BoundedRows<T, MAX> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.rows.into_iter()
    }
}

impl<'de, T: Deserialize<'de> + HeapCharge, const MAX: usize> Deserialize<'de>
    for BoundedRows<T, MAX>
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowsVisitor<T, const MAX: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de> + HeapCharge, const MAX: usize> Visitor<'de> for RowsVisitor<T, MAX> {
            type Value = BoundedRows<T, MAX>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "bounded document census rows")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                if access.size_hint().is_some_and(|hint| hint > MAX) {
                    return Err(de::Error::custom("document census row count exceeds limit"));
                }
                let mut rows = Vec::new();
                let mut charge = 0_usize;
                while rows.len() < MAX {
                    let Some(row) = access.next_element::<T>()? else {
                        return Ok(BoundedRows { rows, charge });
                    };
                    charge = charge
                        .checked_add(row.heap_charge().ok_or_else(|| {
                            de::Error::custom("document census row charge overflow")
                        })?)
                        .ok_or_else(|| de::Error::custom("document census heap overflow"))?;
                    if charge > MAX_HEAP {
                        return Err(de::Error::custom("document census heap limit exceeded"));
                    }
                    rows.try_reserve(1).map_err(de::Error::custom)?;
                    rows.push(row);
                }
                if access.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("document census row count exceeds limit"));
                }
                Ok(BoundedRows { rows, charge })
            }
        }
        deserializer.deserialize_seq(RowsVisitor::<T, MAX>(PhantomData))
    }
}

struct BoundedWriter(Vec<u8>);
struct FieldRows<'a>(&'a BTreeMap<u32, (u64, BTreeSet<Vec<u8>>)>);
struct Terms<'a>(&'a BTreeSet<Vec<u8>>);

impl Serialize for Terms<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut rows = serializer.serialize_seq(Some(self.0.len()))?;
        for term in self.0 {
            rows.serialize_element(term)?;
        }
        rows.end()
    }
}

impl Serialize for FieldRows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut rows = serializer.serialize_seq(Some(self.0.len()))?;
        for (&field, (count, terms)) in self.0 {
            rows.serialize_element(&(field, *count, Terms(terms)))?;
        }
        rows.end()
    }
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self
            .0
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("document census length overflow"))?;
        if next > MAX_BYTES {
            return Err(std::io::Error::other(
                "document census encoded limit exceeded",
            ));
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

#[derive(Clone, Debug)]
pub(crate) struct DocCensus {
    pub(crate) fields: BTreeMap<u32, (u64, BTreeSet<Vec<u8>>)>,
}

/// Per writer mutation, opt-in observation of only census construction/encoding.
/// This is not native writer time, memory peak, or physical I/O.
pub(crate) struct CensusBuildObservation {
    enabled: bool,
    docs: u64,
    encoded_bytes: u64,
    elapsed_ns: u64,
    invalid: bool,
}

impl CensusBuildObservation {
    pub(crate) fn new() -> Self {
        Self {
            enabled: crate::causal_profile::enabled(),
            docs: 0,
            encoded_bytes: 0,
            elapsed_ns: 0,
            invalid: false,
        }
    }

    fn record(&mut self, encoded_bytes: usize, started: Option<Instant>) {
        let Some(started) = started else {
            return;
        };
        let (Ok(bytes), Ok(elapsed_ns)) = (
            u64::try_from(encoded_bytes),
            u64::try_from(started.elapsed().as_nanos()),
        ) else {
            self.invalid = true;
            return;
        };
        let next = self
            .docs
            .checked_add(1)
            .zip(self.encoded_bytes.checked_add(bytes))
            .zip(self.elapsed_ns.checked_add(elapsed_ns));
        if let Some(((docs, encoded_bytes), elapsed_ns)) = next {
            self.docs = docs;
            self.encoded_bytes = encoded_bytes;
            self.elapsed_ns = elapsed_ns;
        } else {
            self.invalid = true;
        }
    }

    #[expect(
        clippy::print_stderr,
        reason = "bounded opt-in causal marker is replayed against the scale artifact"
    )]
    pub(crate) fn emit(&self) {
        if !self.enabled {
            return;
        }
        if self.invalid {
            eprintln!("QI_CAUSAL_V1 kind=bm25_census_build ok=0 reason=counter_overflow");
            return;
        }
        eprintln!(
            "QI_CAUSAL_V1 kind=bm25_census_build ok=1 elapsed_ns={} docs={} encoded_bytes={}",
            self.elapsed_ns, self.docs, self.encoded_bytes,
        );
    }
}

fn invalid(reason: &str) -> CoreError {
    CoreError::Storage(format!(
        "lexical: invalid live BM25 document census: {reason}"
    ))
}

#[expect(
    clippy::set_contains_or_insert,
    reason = "Distinct-term capacity must be admitted before the set retains a new key."
)]
fn admit_term(
    terms: &mut BTreeSet<Vec<u8>>,
    admitted: &mut usize,
    raw: Vec<u8>,
) -> Result<(), CoreError> {
    if !terms.contains(&raw) {
        let next = (*admitted)
            .checked_add(raw.len())
            .and_then(|bytes| bytes.checked_add(32))
            .ok_or_else(|| invalid("census byte count overflows"))?;
        if next > MAX_BYTES {
            return Err(CoreError::InvalidContract(format!(
                "lexical: document BM25 census exceeds {MAX_BYTES} bytes"
            )));
        }
        let _new = terms.insert(raw);
        *admitted = next;
    }
    Ok(())
}

impl DocCensus {
    fn from_document(index: &Index, doc: &TantivyDocument) -> Result<Self, CoreError> {
        let schema = index.schema();
        let mut fields = BTreeMap::new();
        let mut admitted = 0_usize;
        for (field, entry) in schema.fields() {
            if !entry.field_type().is_indexed() {
                continue;
            }
            let mut total = 0_u64;
            let mut terms = BTreeSet::new();
            match entry.field_type() {
                FieldType::Str(options) => {
                    let indexing = options
                        .get_indexing_options()
                        .ok_or_else(|| invalid("indexed text has no indexing options"))?;
                    let mut analyzer = index
                        .tokenizers()
                        .get(indexing.tokenizer())
                        .ok_or_else(|| invalid("indexed text tokenizer is unregistered"))?;
                    for value in doc.get_all(field) {
                        let text = value
                            .as_str()
                            .ok_or_else(|| invalid("indexed text value is not a string"))?;
                        let mut stream = analyzer.token_stream(text);
                        while stream.advance() {
                            // The native postings writer drops overlong tokens
                            // after the analyzer has produced them. Census and
                            // postings must apply the same final byte limit.
                            if stream.token().text.len() > tantivy::tokenizer::MAX_TOKEN_LEN {
                                continue;
                            }
                            total = total
                                .checked_add(1)
                                .ok_or_else(|| invalid("token count overflows"))?;
                            let term = Term::from_field_text(field, &stream.token().text);
                            let raw = term.serialized_value_bytes().to_vec();
                            admit_term(&mut terms, &mut admitted, raw)?;
                        }
                    }
                }
                FieldType::U64(_) => {
                    for value in doc.get_all(field) {
                        let number = value
                            .as_u64()
                            .ok_or_else(|| invalid("indexed u64 value is malformed"))?;
                        total = total
                            .checked_add(1)
                            .ok_or_else(|| invalid("numeric token count overflows"))?;
                        let raw = Term::from_field_u64(field, number)
                            .serialized_value_bytes()
                            .to_vec();
                        admit_term(&mut terms, &mut admitted, raw)?;
                    }
                }
                FieldType::I64(_)
                | FieldType::F64(_)
                | FieldType::Bool(_)
                | FieldType::Date(_)
                | FieldType::Facet(_)
                | FieldType::Bytes(_)
                | FieldType::JsonObject(_)
                | FieldType::IpAddr(_) => {
                    return Err(invalid("indexed field type lacks a census encoder"));
                }
            }
            let _old = fields.insert(field.field_id(), (total, terms));
        }
        Ok(Self { fields })
    }

    fn encode(&self) -> Result<Vec<u8>, CoreError> {
        let mut writer = BoundedWriter(Vec::new());
        ciborium::into_writer(
            &(DOC_CENSUS_FORMAT_VERSION, FieldRows(&self.fields)),
            &mut writer,
        )
        .map_err(|error| {
            CoreError::InvalidContract(format!("lexical: encode document BM25 census: {error}"))
        })?;
        Ok(writer.0)
    }

    pub(crate) fn from_stored(
        index: &Index,
        fields: &SchemaFields,
        doc: &TantivyDocument,
    ) -> Result<Self, CoreError> {
        let mut values = doc.get_all(fields.live_bm25_doc_census);
        let value = values
            .next()
            .ok_or_else(|| invalid("mandatory census is missing"))?;
        if values.next().is_some() {
            return Err(invalid("duplicate census"));
        }
        let bytes = value
            .as_bytes()
            .ok_or_else(|| invalid("census is not bytes"))?;
        if bytes.len() > MAX_BYTES {
            return Err(invalid("census exceeds encoded byte limit"));
        }
        let (format, rows): WireDecode = crate::channel_payloads::decode_cbor_exact(bytes)
            .map_err(|error| invalid(&format!("CBOR: {error}")))?;
        if format != DOC_CENSUS_FORMAT_VERSION {
            return Err(invalid("format differs"));
        }
        let schema = index.schema();
        let expected: Vec<u32> = schema
            .fields()
            .filter(|(_, entry)| entry.field_type().is_indexed())
            .map(|(field, _)| field.field_id())
            .collect();
        if rows.len() != expected.len() {
            return Err(invalid("indexed field census incomplete"));
        }
        let mut fields = BTreeMap::new();
        for ((field, count, terms), expected_field) in rows.into_iter().zip(expected) {
            let unique_count = u64::try_from(terms.len())
                .map_err(|error| invalid(&format!("term count width: {error}")))?;
            if field != expected_field || unique_count > count {
                return Err(invalid("field or token count differs"));
            }
            let field_type = schema
                .get_field_entry(Field::from_field_id(field))
                .field_type();
            let mut unique = BTreeSet::new();
            for term in terms {
                let canonical = match field_type {
                    FieldType::Str(_) => {
                        term.len() <= tantivy::tokenizer::MAX_TOKEN_LEN
                            && std::str::from_utf8(&term).is_ok()
                    }
                    FieldType::U64(_) => term.len() == 8,
                    FieldType::I64(_)
                    | FieldType::F64(_)
                    | FieldType::Bool(_)
                    | FieldType::Date(_)
                    | FieldType::Facet(_)
                    | FieldType::Bytes(_)
                    | FieldType::JsonObject(_)
                    | FieldType::IpAddr(_) => false,
                };
                if !canonical || unique.last().is_some_and(|prior| prior >= &term) {
                    return Err(invalid("terms are not strictly ordered"));
                }
                let _new = unique.insert(term);
            }
            let _old = fields.insert(field, (count, unique));
        }
        Ok(Self { fields })
    }
}

/// Attach once, immediately before the native writer consumes `doc`.
#[cfg(test)]
pub(crate) fn attach(
    index: &Index,
    fields: &SchemaFields,
    doc: &mut TantivyDocument,
) -> Result<(), CoreError> {
    attach_observed(index, fields, doc, None)
}

pub(crate) fn attach_observed(
    index: &Index,
    fields: &SchemaFields,
    doc: &mut TantivyDocument,
    observation: Option<&mut CensusBuildObservation>,
) -> Result<(), CoreError> {
    let started = observation
        .as_ref()
        .and_then(|probe| probe.enabled.then(Instant::now));
    if doc.get_first(fields.live_bm25_doc_census).is_some() {
        return Err(invalid("producer attempted duplicate census"));
    }
    let census = DocCensus::from_document(index, doc)?;
    let encoded = census.encode()?;
    if let Some(observation) = observation {
        observation.record(encoded.len(), started);
    }
    doc.add_bytes(fields.live_bm25_doc_census, encoded);
    Ok(())
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::panic_in_result_fn,
        reason = "Independent test assertions report mismatches; Result carries native setup errors."
    )]

    use super::*;

    #[test]
    fn native_final_token_length_matches_census_byte_boundaries()
    -> Result<(), Box<dyn std::error::Error>> {
        let fields = SchemaFields::build();
        let index = Index::create_in_ram(fields.schema.clone());
        crate::analyzer::register_analyzers(&index);
        let ascii_at = "a".repeat(tantivy::tokenizer::MAX_TOKEN_LEN);
        let ascii_over = "b".repeat(tantivy::tokenizer::MAX_TOKEN_LEN + 1);
        let utf8_at = "é".repeat(tantivy::tokenizer::MAX_TOKEN_LEN.div_euclid(2));
        let utf8_over = format!("{utf8_at}é");
        let mut doc = TantivyDocument::new();
        for token in [&ascii_at, &ascii_over, &utf8_at, &utf8_over, "short", ""] {
            doc.add_text(fields.symbol_local_name, token);
        }
        attach(&index, &fields, &mut doc)?;
        let census = DocCensus::from_stored(&index, &fields, &doc)?;
        let (count, terms) = census
            .fields
            .get(&fields.symbol_local_name.field_id())
            .ok_or("missing raw symbol census")?;
        assert_eq!(*count, 4);
        for term in [&ascii_at, &utf8_at, "short", ""] {
            assert!(terms.contains(
                Term::from_field_text(fields.symbol_local_name, term).serialized_value_bytes()
            ));
        }
        for term in [&ascii_over, &utf8_over] {
            assert!(!terms.contains(
                Term::from_field_text(fields.symbol_local_name, term).serialized_value_bytes()
            ));
        }
        let mut writer = index.writer(50_000_000)?;
        let _opstamp = writer.add_document(doc)?;
        let _commit = writer.commit()?;
        writer.wait_merging_threads()?;
        let reader = index.reader()?;
        let searcher = reader.searcher();
        let segment = searcher
            .segment_readers()
            .first()
            .ok_or("missing segment")?;
        let inverted = segment.inverted_index(fields.symbol_local_name)?;
        assert_eq!(inverted.total_num_tokens(), 4);
        let norms = segment.get_fieldnorms_reader(fields.symbol_local_name)?;
        assert_eq!(
            norms.fieldnorm_id(0),
            tantivy::fieldnorm::FieldNormReader::fieldnorm_to_id(4)
        );
        for term in [&ascii_at, &utf8_at, "short", ""] {
            assert_eq!(
                searcher.doc_freq(&Term::from_field_text(fields.symbol_local_name, term))?,
                1
            );
        }
        for term in [&ascii_over, &utf8_over] {
            assert_eq!(
                searcher.doc_freq(&Term::from_field_text(fields.symbol_local_name, term))?,
                0
            );
        }
        Ok(())
    }
}
