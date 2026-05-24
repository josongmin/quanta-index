//! Per-generation IDF table — canonical serialized artifact.
//!
//! `IdfTable` captures the inverse-document-frequency state pinned at write
//! time for a generation. It is the authoritative source consulted at score
//! time; LEX-01 forbids recomputing IDF from live segment statistics.
//!
//! The Robertson-Spärck Jones formula used here is:
//!
//! ```text
//! idf(t) = ln((N - n_t + 0.5) / (n_t + 0.5) + 1.0)
//! ```
//!
//! where `N` is the total document count for the generation and `n_t` is
//! the number of documents containing term `t`. The `+1.0` inside the
//! logarithm guarantees `idf >= 0` even when `n_t` exceeds `N / 2`, which
//! matches Lucene / Tantivy semantics.
//!
//! Unknown terms (i.e. terms absent from `term_doc_counts`) score as if
//! `n_t == 0`; this yields the maximum IDF for the corpus and conservatively
//! over-weights novel terms.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use crate::errors::{ScorerError, ScorerErrorCode};

/// Authoritative per-generation IDF state.
///
/// `term_doc_counts` uses `BTreeMap<Box<str>, u64>` so iteration order is
/// the lexical sort order of the term bytes — this is what gives the CBOR
/// encoding its byte-identical-across-runs property.
#[derive(Clone, Debug, PartialEq)]
pub struct IdfTable {
    generation: u64,
    total_docs: u64,
    term_doc_counts: BTreeMap<Box<str>, u64>,
    avg_doc_len: f64,
}

impl IdfTable {
    /// Construct an IDF table from materialised statistics.
    pub fn new(
        generation: u64,
        total_docs: u64,
        term_doc_counts: BTreeMap<Box<str>, u64>,
        avg_doc_len: f64,
    ) -> Result<Self, ScorerError> {
        if generation == 0 {
            return Err(ScorerError::new(
                ScorerErrorCode::InvalidGeneration,
                "generation must be non-zero",
            ));
        }
        if total_docs == 0 {
            return Err(ScorerError::new(
                ScorerErrorCode::EmptyCorpus,
                "IdfTable requires at least one document",
            ));
        }
        if !avg_doc_len.is_finite() || avg_doc_len < 0.0 {
            return Err(ScorerError::new(
                ScorerErrorCode::EmptyCorpus,
                "avg_doc_len must be finite and non-negative",
            ));
        }
        Ok(Self {
            generation,
            total_docs,
            term_doc_counts,
            avg_doc_len,
        })
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn total_docs(&self) -> u64 {
        self.total_docs
    }

    #[must_use]
    pub const fn avg_doc_len(&self) -> f64 {
        self.avg_doc_len
    }

    /// Number of documents containing `term`, or `0` if term is unseen.
    #[must_use]
    pub fn term_doc_count(&self, term: &str) -> u64 {
        self.term_doc_counts.get(term).copied().unwrap_or(0)
    }

    /// Borrowed view of the underlying term-to-document-count map.
    #[must_use]
    pub const fn term_doc_counts(&self) -> &BTreeMap<Box<str>, u64> {
        &self.term_doc_counts
    }

    /// Serialize as canonical CBOR.
    pub fn serialize_cbor<W: Write>(&self, writer: W) -> Result<(), ScorerError> {
        ciborium::ser::into_writer(self, writer).map_err(|e| {
            ScorerError::new(
                ScorerErrorCode::IdfTableDeserialize,
                format!("CBOR encode failed: {e}"),
            )
        })
    }

    /// Inverse of [`Self::serialize_cbor`].
    pub fn deserialize_cbor<R: Read>(reader: R) -> Result<Self, ScorerError> {
        ciborium::de::from_reader(reader).map_err(|e| {
            ScorerError::new(
                ScorerErrorCode::IdfTableDeserialize,
                format!("CBOR decode failed: {e}"),
            )
        })
    }
}

/// Robertson-Spärck Jones IDF for a term against `table`.
///
/// Formula: `ln((N - n + 0.5) / (n + 0.5) + 1.0)`, bounded at `0.0` by the
/// `+1.0` inside the logarithm.
#[must_use]
pub fn idf_for_term(table: &IdfTable, term: &str) -> f32 {
    if table.total_docs == 0 {
        return 0.0;
    }
    let n_t = table.term_doc_count(term);
    let big_n = u64_to_f64(table.total_docs);
    let n = u64_to_f64(n_t.min(table.total_docs));
    let numerator = big_n - n + 0.5;
    let denom = n + 0.5;
    // denom >= 0.5 → division is finite.
    let inner = numerator / denom + 1.0;
    // `inner >= 1.0` by construction, so `ln(inner) >= 0`.
    let raw = inner.ln();
    f64_to_f32_saturating(raw)
}

fn u64_to_f64(v: u64) -> f64 {
    #[expect(
        clippy::as_conversions,
        reason = "f64::from(u64) does not exist for u64 above 2^32; explicit `as` is the only stable conversion"
    )]
    #[expect(
        clippy::cast_precision_loss,
        reason = "IDF formula tolerates sub-ULP rounding at corpus sizes above 2^53; precision loss is intentional"
    )]
    let f = v as f64;
    f
}

fn f64_to_f32_saturating(v: f64) -> f32 {
    #[expect(
        clippy::as_conversions,
        reason = "the f64 to f32 narrowing cast is the documented downcast; no safe wrapper exists for saturating conversion in stable std"
    )]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "IDF downcast f64 -> f32 is intentional; envelope cap in normalization layer absorbs overflow"
    )]
    let f = v as f32;
    f
}

impl serde::Serialize for IdfTable {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(4))?;
        m.serialize_entry("generation", &self.generation)?;
        m.serialize_entry("total_docs", &self.total_docs)?;
        m.serialize_entry("avg_doc_len", &self.avg_doc_len)?;
        m.serialize_entry("term_doc_counts", &SortedMapRef(&self.term_doc_counts))?;
        m.end()
    }
}

struct SortedMapRef<'a>(&'a BTreeMap<Box<str>, u64>);

impl serde::Serialize for SortedMapRef<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0 {
            m.serialize_entry(k.as_ref(), v)?;
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for IdfTable {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = IdfTable;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str(
                    "IdfTable map with fields generation, total_docs, avg_doc_len, term_doc_counts",
                )
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<IdfTable, M::Error> {
                let mut generation: Option<u64> = None;
                let mut total_docs: Option<u64> = None;
                let mut avg_doc_len: Option<f64> = None;
                let mut term_doc_counts: Option<BTreeMap<Box<str>, u64>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "generation" => {
                            if generation.is_some() {
                                return Err(serde::de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                        "total_docs" => {
                            if total_docs.is_some() {
                                return Err(serde::de::Error::duplicate_field("total_docs"));
                            }
                            total_docs = Some(map.next_value()?);
                        }
                        "avg_doc_len" => {
                            if avg_doc_len.is_some() {
                                return Err(serde::de::Error::duplicate_field("avg_doc_len"));
                            }
                            avg_doc_len = Some(map.next_value()?);
                        }
                        "term_doc_counts" => {
                            if term_doc_counts.is_some() {
                                return Err(serde::de::Error::duplicate_field("term_doc_counts"));
                            }
                            let raw: BTreeMap<String, u64> = map.next_value()?;
                            let mut out: BTreeMap<Box<str>, u64> = BTreeMap::new();
                            for (k, v) in raw {
                                let prior = out.insert(k.into_boxed_str(), v);
                                if prior.is_some() {
                                    return Err(serde::de::Error::custom(
                                        "duplicate term in term_doc_counts",
                                    ));
                                }
                            }
                            term_doc_counts = Some(out);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &["generation", "total_docs", "avg_doc_len", "term_doc_counts"],
                            ));
                        }
                    }
                }
                let g = generation.ok_or_else(|| serde::de::Error::missing_field("generation"))?;
                let n = total_docs.ok_or_else(|| serde::de::Error::missing_field("total_docs"))?;
                let a =
                    avg_doc_len.ok_or_else(|| serde::de::Error::missing_field("avg_doc_len"))?;
                let t = term_doc_counts
                    .ok_or_else(|| serde::de::Error::missing_field("term_doc_counts"))?;
                IdfTable::new(g, n, t, a).map_err(serde::de::Error::custom)
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{IdfTable, idf_for_term};
    use crate::errors::ScorerErrorCode;
    use std::collections::BTreeMap;

    fn fatal(msg: &str) -> ! {
        assert!(false, "{msg}");
        std::process::abort();
    }

    fn sample_table() -> IdfTable {
        let mut m: BTreeMap<Box<str>, u64> = BTreeMap::new();
        for (k, v) in [("foo", 1u64), ("bar", 2), ("baz", 3)] {
            let prior = m.insert(k.into(), v);
            assert!(prior.is_none());
        }
        match IdfTable::new(7, 3, m, 12.0) {
            Ok(t) => t,
            Err(e) => fatal(&format!("{e}")),
        }
    }

    #[test]
    fn new_rejects_zero_generation() {
        match IdfTable::new(0, 1, BTreeMap::new(), 1.0) {
            Ok(_) => assert!(false, "must reject generation=0"),
            Err(e) => assert_eq!(e.code, ScorerErrorCode::InvalidGeneration),
        }
    }

    #[test]
    fn new_rejects_zero_docs() {
        match IdfTable::new(1, 0, BTreeMap::new(), 1.0) {
            Ok(_) => assert!(false, "must reject total_docs=0"),
            Err(e) => assert_eq!(e.code, ScorerErrorCode::EmptyCorpus),
        }
    }

    #[test]
    fn new_rejects_nan_avg_doc_len() {
        match IdfTable::new(1, 1, BTreeMap::new(), f64::NAN) {
            Ok(_) => assert!(false, "must reject NaN avg_doc_len"),
            Err(e) => assert_eq!(e.code, ScorerErrorCode::EmptyCorpus),
        }
    }

    #[test]
    fn idf_for_known_term_is_positive() {
        let t = sample_table();
        let v = idf_for_term(&t, "foo");
        assert!(v > 0.0, "idf(foo) must be > 0, got {v}");
        assert!(v.is_finite());
    }

    #[test]
    fn idf_for_unknown_term_uses_n_eq_zero() {
        let t = sample_table();
        let v_unknown = idf_for_term(&t, "unseen");
        let v_known = idf_for_term(&t, "baz");
        assert!(v_unknown > v_known, "{v_unknown} !> {v_known}");
    }

    #[test]
    fn idf_is_never_negative() {
        let t = sample_table();
        for term in ["foo", "bar", "baz", "unseen", ""] {
            let v = idf_for_term(&t, term);
            assert!(v >= 0.0, "idf({term}) = {v} < 0");
            assert!(v.is_finite());
        }
    }

    #[test]
    fn cbor_roundtrip_preserves_value() {
        let t = sample_table();
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = t.serialize_cbor(&mut buf) {
            fatal(&format!("{e}"));
        }
        match IdfTable::deserialize_cbor(buf.as_slice()) {
            Ok(got) => assert_eq!(got, t),
            Err(e) => fatal(&format!("{e}")),
        }
    }

    #[test]
    fn cbor_encoding_is_byte_identical_across_runs() {
        let t1 = sample_table();
        let t2 = sample_table();
        let mut b1: Vec<u8> = Vec::new();
        let mut b2: Vec<u8> = Vec::new();
        if let Err(e) = t1.serialize_cbor(&mut b1) {
            fatal(&format!("{e}"));
        }
        if let Err(e) = t2.serialize_cbor(&mut b2) {
            fatal(&format!("{e}"));
        }
        assert_eq!(b1, b2);
    }

    #[test]
    fn deserialize_rejects_truncated_bytes() {
        let t = sample_table();
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = t.serialize_cbor(&mut buf) {
            fatal(&format!("{e}"));
        }
        let last_idx = buf.len().saturating_sub(3);
        let Some(truncated) = buf.get(..last_idx) else {
            fatal("truncation slice");
        };
        match IdfTable::deserialize_cbor(truncated) {
            Ok(_) => assert!(false, "truncated CBOR must fail"),
            Err(e) => assert_eq!(e.code, ScorerErrorCode::IdfTableDeserialize),
        }
    }
}
