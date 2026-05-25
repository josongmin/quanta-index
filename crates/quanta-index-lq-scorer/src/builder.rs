//! Deterministic per-generation IDF table builder.
//!
//! [`IdfBuilder`] aggregates document-frequency statistics across a corpus
//! and finalises into an [`crate::idf::IdfTable`]. The result is
//! deterministic in two senses:
//!
//! 1. The output `IdfTable` is independent of the order in which documents
//!    are added — `term_doc_counts` is a `BTreeMap` keyed on owned term
//!    bytes.
//! 2. Within a single document, repeated terms contribute exactly one to
//!    the term's document-frequency.
//!
//! ## Idempotent upsert / delete / `from_prior`
//!
//! Each document is identified by a stable [`DocId`]. The builder keeps a
//! per-doc state map (`DocStats`) so [`IdfBuilder::upsert_doc`] can subtract
//! a document's prior contribution before adding the new one — that makes
//! replays no-ops and stale-doc removals exact. [`IdfBuilder::remove_doc`]
//! drops a doc's contribution without re-adding it.
//!
//! **`from_prior` source**: a finished [`IdfTable`] is aggregate-only — we
//! cannot subtract an individual doc from it. To seed a new generation we
//! ship [`IdfStateBundle`], a serializable per-doc snapshot that the
//! builder produces via [`IdfBuilder::state_bundle`] and that
//! [`IdfBuilder::from_prior`] consumes. Callers that need to seed from a
//! prior in-process builder can use [`IdfBuilder::from_prior_builder`].
//!
//! D18 — no proc-macro derives.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use crate::errors::{ScorerError, ScorerErrorCode};
use crate::idf::IdfTable;
use crate::scorer::IdfTokenSource;

/// Stable per-document identifier for builder state.
///
/// Newtype around `u64`; equality and ordering match the wrapped value.
/// Sibling LQ crates (symbol, trigram, positions) all use the same shape;
/// this crate defines its own copy rather than taking a workspace dep so
/// the scorer stays a standalone foundation crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(pub u64);

impl DocId {
    #[must_use]
    pub const fn new(v: u64) -> Self {
        Self(v)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for DocId {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<DocId> for u64 {
    fn from(d: DocId) -> Self {
        d.0
    }
}

impl core::fmt::Display for DocId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for DocId {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_u64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for DocId {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = DocId;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("DocId u64")
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<DocId, E> {
                Ok(DocId(v))
            }
            fn visit_u32<E: serde::de::Error>(self, v: u32) -> Result<DocId, E> {
                Ok(DocId(u64::from(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<DocId, E> {
                if v < 0 {
                    return Err(E::custom("DocId must be non-negative"));
                }
                let u = u64::try_from(v)
                    .map_err(|err| E::custom(format!("DocId out of u64 range: {err}")))?;
                Ok(DocId(u))
            }
        }
        de.deserialize_u64(V)
    }
}

/// Per-doc state held by the builder while staging mutations.
///
/// `term_set` is the set of distinct terms the doc contributes to
/// `term_doc_counts`; the IDF formula only cares about doc-frequency, so
/// repeats inside a doc don't matter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocStats {
    pub doc_len: u32,
    pub term_set: std::collections::BTreeSet<Box<str>>,
}

impl DocStats {
    fn new(doc_len: u32, term_set: std::collections::BTreeSet<Box<str>>) -> Self {
        Self { doc_len, term_set }
    }
}

/// Per-generation IDF builder.
///
/// Keyed on [`DocId`] for replay safety: [`Self::upsert_doc`] subtracts a
/// doc's prior contribution before re-adding, and [`Self::remove_doc`]
/// drops the contribution.
pub struct IdfBuilder {
    generation: u64,
    /// Doc-frequency table — number of distinct docs containing each term.
    term_doc_counts: BTreeMap<Box<str>, u64>,
    /// Per-doc state used for subtract-on-upsert / subtract-on-remove.
    docs: BTreeMap<DocId, DocStats>,
    /// Running sum of all `doc_len` values; kept in `u128` to avoid
    /// overflow at corpus sizes that would saturate `u64`.
    total_doc_len: u128,
}

impl IdfBuilder {
    /// Construct a fresh builder for `generation`.
    pub fn new(generation: u64) -> Result<Self, ScorerError> {
        if generation == 0 {
            return Err(ScorerError::new(
                ScorerErrorCode::InvalidGeneration,
                "generation must be non-zero",
            ));
        }
        Ok(Self {
            generation,
            term_doc_counts: BTreeMap::new(),
            docs: BTreeMap::new(),
            total_doc_len: 0,
        })
    }

    /// Seed a fresh builder for `new_generation` from a per-doc state
    /// bundle.
    ///
    /// The bundle is the unit of cross-generation handoff; produce it via
    /// [`Self::state_bundle`] before finalising the prior generation, ship
    /// it across the generation boundary, then call `from_prior` to recover
    /// a builder whose state is byte-equivalent (modulo `generation`).
    pub fn from_prior(prior: &IdfStateBundle, new_generation: u64) -> Result<Self, ScorerError> {
        if new_generation == 0 {
            return Err(ScorerError::new(
                ScorerErrorCode::InvalidGeneration,
                "new_generation must be non-zero",
            ));
        }
        // Rebuild aggregates from the bundle's per-doc state. This is
        // O(Σ |term_set|) but only runs once per generation handoff.
        let mut term_doc_counts: BTreeMap<Box<str>, u64> = BTreeMap::new();
        let mut total_doc_len: u128 = 0;
        for stats in prior.docs.values() {
            total_doc_len = total_doc_len.saturating_add(u128::from(stats.doc_len));
            for term in &stats.term_set {
                let entry = term_doc_counts.entry(term.clone()).or_insert(0);
                *entry = entry.saturating_add(1);
            }
        }
        Ok(Self {
            generation: new_generation,
            term_doc_counts,
            docs: prior.docs.clone(),
            total_doc_len,
        })
    }

    /// Seed from another live builder. Cheaper than the bundle path when
    /// the prior builder is still in-process; the resulting builder owns
    /// an independent clone of the per-doc map.
    pub fn from_prior_builder(prior: &Self, new_generation: u64) -> Result<Self, ScorerError> {
        if new_generation == 0 {
            return Err(ScorerError::new(
                ScorerErrorCode::InvalidGeneration,
                "new_generation must be non-zero",
            ));
        }
        Ok(Self {
            generation: new_generation,
            term_doc_counts: prior.term_doc_counts.clone(),
            docs: prior.docs.clone(),
            total_doc_len: prior.total_doc_len,
        })
    }

    /// Snapshot the builder's per-doc state for cross-generation handoff.
    #[must_use]
    pub fn state_bundle(&self) -> IdfStateBundle {
        IdfStateBundle {
            generation: self.generation,
            docs: self.docs.clone(),
        }
    }

    /// Add (or replace) a document's term stream.
    ///
    /// **BREAKING vs. pre-audit `add_doc`**: this signature takes a
    /// [`DocId`] so the builder can track per-doc state. If `doc_id` is
    /// already present, this is a replace — the prior contribution is
    /// subtracted first. `doc_len` is the document's analyzer-emitted token
    /// count. Duplicate terms within a single document still contribute
    /// exactly one to the term's document-frequency.
    pub fn add_doc(
        &mut self,
        doc_id: DocId,
        terms: &mut dyn IdfTokenSource,
        doc_len: u32,
    ) -> Result<(), ScorerError> {
        self.upsert_doc(doc_id, terms, doc_len)
    }

    /// Upsert a document. If `doc_id` is present, subtracts its prior
    /// contribution to `term_doc_counts` and `total_doc_len` before adding
    /// the new contribution. Idempotent: re-applying the same `(doc_id,
    /// terms, doc_len)` leaves builder state unchanged.
    pub fn upsert_doc(
        &mut self,
        doc_id: DocId,
        terms: &mut dyn IdfTokenSource,
        doc_len: u32,
    ) -> Result<(), ScorerError> {
        // Snapshot the new terms first; we need to know the new term-set
        // before deciding what to subtract.
        let mut new_terms: std::collections::BTreeSet<Box<str>> = std::collections::BTreeSet::new();
        while let Some(t) = terms.next_term() {
            let _inserted = new_terms.insert(t.into());
        }

        // Subtract the prior contribution if present.
        if let Some(prior) = self.docs.remove(&doc_id) {
            self.total_doc_len = self
                .total_doc_len
                .checked_sub(u128::from(prior.doc_len))
                .ok_or_else(|| {
                    ScorerError::new(
                        ScorerErrorCode::EmptyCorpus,
                        "total_doc_len underflow on upsert subtract",
                    )
                })?;
            for term in &prior.term_set {
                self.decrement_term(term.as_ref())?;
            }
        }

        // Add the new contribution.
        self.total_doc_len = self.total_doc_len.saturating_add(u128::from(doc_len));
        for term in &new_terms {
            let entry = self.term_doc_counts.entry(term.clone()).or_insert(0);
            *entry = entry.saturating_add(1);
        }
        let _prior = self.docs.insert(doc_id, DocStats::new(doc_len, new_terms));
        Ok(())
    }

    /// Remove a document's contribution. Returns `true` if the doc was
    /// present, `false` if not. Idempotent: repeated calls return `false`
    /// after the first successful removal.
    pub fn remove_doc(&mut self, doc_id: DocId) -> Result<bool, ScorerError> {
        let Some(prior) = self.docs.remove(&doc_id) else {
            return Ok(false);
        };
        self.total_doc_len = self
            .total_doc_len
            .checked_sub(u128::from(prior.doc_len))
            .ok_or_else(|| {
                ScorerError::new(
                    ScorerErrorCode::EmptyCorpus,
                    "total_doc_len underflow on remove",
                )
            })?;
        for term in &prior.term_set {
            self.decrement_term(term.as_ref())?;
        }
        Ok(true)
    }

    fn decrement_term(&mut self, term: &str) -> Result<(), ScorerError> {
        let Some(entry) = self.term_doc_counts.get_mut(term) else {
            return Err(ScorerError::new(
                ScorerErrorCode::EmptyCorpus,
                format!("decrement_term: term {term:?} missing from term_doc_counts"),
            ));
        };
        match entry.checked_sub(1) {
            Some(0) => {
                let _prior = self.term_doc_counts.remove(term);
            }
            Some(v) => *entry = v,
            None => {
                return Err(ScorerError::new(
                    ScorerErrorCode::EmptyCorpus,
                    format!("decrement_term: underflow on term {term:?}"),
                ));
            }
        }
        Ok(())
    }

    /// Finalise the builder into an [`IdfTable`].
    pub fn finish(self) -> Result<IdfTable, ScorerError> {
        let total_docs = self.total_docs_checked()?;
        if total_docs == 0 {
            return Err(ScorerError::new(
                ScorerErrorCode::EmptyCorpus,
                "IdfBuilder requires at least one document before finish()",
            ));
        }
        let avg = u128_to_f64(self.total_doc_len) / u64_to_f64(total_docs);
        IdfTable::new(self.generation, total_docs, self.term_doc_counts, avg)
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of docs currently staged in the builder. Returns
    /// [`ScorerErrorCode::EmptyCorpus`] if the staged count overflows
    /// `u64` (only reachable on 128-bit `usize` platforms, which the
    /// scorer does not currently target — surfacing the failure typed is
    /// the no-silent-failure choice).
    pub fn total_docs(&self) -> Result<u64, ScorerError> {
        self.total_docs_checked()
    }

    fn total_docs_checked(&self) -> Result<u64, ScorerError> {
        u64::try_from(self.docs.len()).map_err(|e| {
            ScorerError::new(
                ScorerErrorCode::EmptyCorpus,
                format!("staged doc count overflows u64: {e}"),
            )
        })
    }

    /// `true` if no docs are currently staged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }
}

/// Cross-generation per-doc state snapshot.
///
/// Wire shape is `{ generation, docs: { doc_id -> {doc_len, term_set} } }`;
/// CBOR encoding is byte-identical across runs with the same input thanks
/// to the deterministic `BTreeMap` / `BTreeSet` ordering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdfStateBundle {
    generation: u64,
    docs: BTreeMap<DocId, DocStats>,
}

impl IdfStateBundle {
    /// Construct a bundle from raw parts. Returns
    /// [`ScorerErrorCode::InvalidGeneration`] if `generation == 0`.
    pub fn new(generation: u64, docs: BTreeMap<DocId, DocStats>) -> Result<Self, ScorerError> {
        if generation == 0 {
            return Err(ScorerError::new(
                ScorerErrorCode::InvalidGeneration,
                "generation must be non-zero",
            ));
        }
        Ok(Self { generation, docs })
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of docs in the snapshot.
    #[must_use]
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// `true` if the snapshot has zero docs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Borrowed view of the underlying per-doc state map.
    #[must_use]
    pub const fn docs(&self) -> &BTreeMap<DocId, DocStats> {
        &self.docs
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

impl serde::Serialize for DocStats {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("doc_len", &self.doc_len)?;
        m.serialize_entry("term_set", &TermSetRef(&self.term_set))?;
        m.end()
    }
}

struct TermSetRef<'a>(&'a std::collections::BTreeSet<Box<str>>);

impl serde::Serialize for TermSetRef<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq as _;
        let mut s = ser.serialize_seq(Some(self.0.len()))?;
        for t in self.0 {
            s.serialize_element(t.as_ref())?;
        }
        s.end()
    }
}

impl<'de> serde::Deserialize<'de> for DocStats {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = DocStats;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("DocStats map { doc_len: u32, term_set: [str] }")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<DocStats, M::Error> {
                let mut doc_len: Option<u32> = None;
                let mut term_set: Option<std::collections::BTreeSet<Box<str>>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "doc_len" => {
                            if doc_len.is_some() {
                                return Err(serde::de::Error::duplicate_field("doc_len"));
                            }
                            doc_len = Some(map.next_value()?);
                        }
                        "term_set" => {
                            if term_set.is_some() {
                                return Err(serde::de::Error::duplicate_field("term_set"));
                            }
                            let raw: Vec<String> = map.next_value()?;
                            let mut set: std::collections::BTreeSet<Box<str>> =
                                std::collections::BTreeSet::new();
                            for t in raw {
                                let _inserted = set.insert(t.into_boxed_str());
                            }
                            term_set = Some(set);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["doc_len", "term_set"],
                            ));
                        }
                    }
                }
                let doc_len = doc_len.ok_or_else(|| serde::de::Error::missing_field("doc_len"))?;
                let term_set =
                    term_set.ok_or_else(|| serde::de::Error::missing_field("term_set"))?;
                Ok(DocStats::new(doc_len, term_set))
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for IdfStateBundle {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("generation", &self.generation)?;
        m.serialize_entry("docs", &DocsMapRef(&self.docs))?;
        m.end()
    }
}

struct DocsMapRef<'a>(&'a BTreeMap<DocId, DocStats>);

impl serde::Serialize for DocsMapRef<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0 {
            m.serialize_entry(&k.0, v)?;
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for IdfStateBundle {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = IdfStateBundle;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("IdfStateBundle map { generation: u64, docs: { u64 -> DocStats } }")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<IdfStateBundle, M::Error> {
                let mut generation: Option<u64> = None;
                let mut docs: Option<BTreeMap<DocId, DocStats>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "generation" => {
                            if generation.is_some() {
                                return Err(serde::de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                        "docs" => {
                            if docs.is_some() {
                                return Err(serde::de::Error::duplicate_field("docs"));
                            }
                            let raw: BTreeMap<u64, DocStats> = map.next_value()?;
                            let mut out: BTreeMap<DocId, DocStats> = BTreeMap::new();
                            for (k, v) in raw {
                                let prior = out.insert(DocId(k), v);
                                if prior.is_some() {
                                    return Err(serde::de::Error::custom(
                                        "duplicate doc_id in IdfStateBundle.docs",
                                    ));
                                }
                            }
                            docs = Some(out);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["generation", "docs"],
                            ));
                        }
                    }
                }
                let g = generation.ok_or_else(|| serde::de::Error::missing_field("generation"))?;
                if g == 0 {
                    return Err(serde::de::Error::custom("generation must be non-zero"));
                }
                let d = docs.ok_or_else(|| serde::de::Error::missing_field("docs"))?;
                Ok(IdfStateBundle {
                    generation: g,
                    docs: d,
                })
            }
        }
        de.deserialize_map(V)
    }
}

fn u128_to_f64(v: u128) -> f64 {
    #[expect(
        clippy::as_conversions,
        reason = "f64::from(u128) is not available; explicit `as` is the only stable conversion"
    )]
    #[expect(
        clippy::cast_precision_loss,
        reason = "total_doc_len precision loss above 2^53 is sub-ULP for the resulting avg"
    )]
    let f = v as f64;
    f
}

fn u64_to_f64(v: u64) -> f64 {
    #[expect(
        clippy::as_conversions,
        reason = "f64::from(u64) is not available for u64 above 2^32; explicit `as` is the only stable conversion"
    )]
    #[expect(
        clippy::cast_precision_loss,
        reason = "total_docs precision loss above 2^53 is sub-ULP for the resulting avg"
    )]
    let f = v as f64;
    f
}

#[cfg(test)]
mod tests {
    use super::{DocId, IdfBuilder, IdfStateBundle};
    use crate::errors::ScorerErrorCode;
    use crate::scorer::SliceTokenSource;

    fn fatal(msg: &str) -> ! {
        assert!(false, "{msg}");
        std::process::abort();
    }

    fn add(b: &mut IdfBuilder, doc_id: u64, terms: &[&str], doc_len: u32) {
        let mut src = SliceTokenSource::new(terms);
        if let Err(e) = b.add_doc(DocId(doc_id), &mut src, doc_len) {
            fatal(&format!("{e}"));
        }
    }

    #[test]
    fn new_rejects_zero_generation() {
        match IdfBuilder::new(0) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, ScorerErrorCode::InvalidGeneration),
        }
    }

    #[test]
    fn finish_rejects_empty_corpus() {
        let b = match IdfBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        match b.finish() {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, ScorerErrorCode::EmptyCorpus),
        }
    }

    #[test]
    fn duplicate_terms_in_doc_count_once() {
        let mut b = match IdfBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        add(&mut b, 1, &["foo", "foo", "foo"], 3);
        let t = match b.finish() {
            Ok(t) => t,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(t.term_doc_count("foo"), 1);
        assert_eq!(t.total_docs(), 1);
        assert!((t.avg_doc_len() - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cross_doc_counts_accumulate() {
        let mut b = match IdfBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        add(&mut b, 1, &["foo", "bar"], 2);
        add(&mut b, 2, &["foo", "baz"], 2);
        add(&mut b, 3, &["bar", "baz"], 2);
        let t = match b.finish() {
            Ok(t) => t,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(t.total_docs(), 3);
        assert_eq!(t.term_doc_count("foo"), 2);
        assert_eq!(t.term_doc_count("bar"), 2);
        assert_eq!(t.term_doc_count("baz"), 2);
    }

    #[test]
    fn order_independent_output() {
        let mk = |order: &[u64]| -> crate::idf::IdfTable {
            let mut b = match IdfBuilder::new(1) {
                Ok(b) => b,
                Err(e) => fatal(&format!("{e}")),
            };
            let docs: [(u64, &[&str]); 3] = [
                (1, &["foo", "bar"]),
                (2, &["foo", "baz"]),
                (3, &["bar", "baz"]),
            ];
            for &id in order {
                let Some((_, d)) = docs.iter().find(|(k, _)| *k == id) else {
                    continue;
                };
                add(&mut b, id, d, 2);
            }
            match b.finish() {
                Ok(t) => t,
                Err(e) => fatal(&format!("{e}")),
            }
        };
        let t1 = mk(&[1, 2, 3]);
        let t2 = mk(&[3, 1, 2]);
        let t3 = mk(&[2, 3, 1]);
        assert_eq!(t1, t2);
        assert_eq!(t2, t3);
    }

    // ── delta-handling: upsert_doc / remove_doc / from_prior ──────────

    #[test]
    fn upsert_doc_replaces_prior_contribution() {
        let mut b = match IdfBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        add(&mut b, 1, &["foo", "bar"], 2);
        add(&mut b, 2, &["foo", "baz"], 2);
        // Upsert doc 1 with a new term-set — "bar" drops out, "qux" enters.
        let mut src = SliceTokenSource::new(&["foo", "qux"]);
        if let Err(e) = b.upsert_doc(DocId(1), &mut src, 2) {
            fatal(&format!("{e}"));
        }
        let t = match b.finish() {
            Ok(t) => t,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(t.total_docs(), 2);
        // foo: still in both docs.
        assert_eq!(t.term_doc_count("foo"), 2);
        // bar: dropped from doc 1, never in doc 2 → 0.
        assert_eq!(t.term_doc_count("bar"), 0);
        // baz: still only in doc 2.
        assert_eq!(t.term_doc_count("baz"), 1);
        // qux: new in doc 1.
        assert_eq!(t.term_doc_count("qux"), 1);
    }

    #[test]
    fn upsert_doc_is_idempotent_on_replay() {
        let mk = || -> IdfBuilder {
            let mut b = match IdfBuilder::new(1) {
                Ok(b) => b,
                Err(e) => fatal(&format!("{e}")),
            };
            add(&mut b, 1, &["foo", "bar"], 2);
            add(&mut b, 2, &["foo", "baz"], 2);
            b
        };
        let b_once = mk();
        let mut b_replay = mk();
        // Replay the same upserts; state must be unchanged.
        let mut src1 = SliceTokenSource::new(&["foo", "bar"]);
        if let Err(e) = b_replay.upsert_doc(DocId(1), &mut src1, 2) {
            fatal(&format!("{e}"));
        }
        let mut src2 = SliceTokenSource::new(&["foo", "baz"]);
        if let Err(e) = b_replay.upsert_doc(DocId(2), &mut src2, 2) {
            fatal(&format!("{e}"));
        }
        let t_once = match b_once.finish() {
            Ok(t) => t,
            Err(e) => fatal(&format!("{e}")),
        };
        let t_replay = match b_replay.finish() {
            Ok(t) => t,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(t_once, t_replay);
    }

    #[test]
    fn remove_doc_subtracts_correctly() {
        let mut b = match IdfBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        add(&mut b, 1, &["foo", "bar"], 2);
        add(&mut b, 2, &["foo", "baz"], 2);
        add(&mut b, 3, &["foo", "qux"], 2);
        // Drop doc 2.
        match b.remove_doc(DocId(2)) {
            Ok(true) => {}
            Ok(false) => {
                fatal("first remove must return true");
            }
            Err(e) => fatal(&format!("{e}")),
        }
        // Idempotency: second drop returns false.
        match b.remove_doc(DocId(2)) {
            Ok(false) => {}
            Ok(true) => fatal("second remove must return false"),
            Err(e) => fatal(&format!("{e}")),
        }
        let t = match b.finish() {
            Ok(t) => t,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(t.total_docs(), 2);
        // foo: was in 3 docs, now in 2.
        assert_eq!(t.term_doc_count("foo"), 2);
        // bar: still in doc 1.
        assert_eq!(t.term_doc_count("bar"), 1);
        // baz: was only in doc 2 → fully removed.
        assert_eq!(t.term_doc_count("baz"), 0);
        // qux: still in doc 3.
        assert_eq!(t.term_doc_count("qux"), 1);
    }

    #[test]
    fn remove_doc_unknown_is_idempotent() {
        let mut b = match IdfBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        add(&mut b, 1, &["foo"], 1);
        match b.remove_doc(DocId(99)) {
            Ok(false) => {}
            Ok(true) => fatal("remove of unknown doc must return false"),
            Err(e) => fatal(&format!("{e}")),
        }
        match b.total_docs() {
            Ok(n) => assert_eq!(n, 1),
            Err(e) => fatal(&format!("{e}")),
        }
    }

    #[test]
    fn from_prior_or_from_builder_equivalence() {
        let mut prior = match IdfBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        add(&mut prior, 1, &["foo", "bar"], 2);
        add(&mut prior, 2, &["foo", "baz"], 2);

        let bundle = prior.state_bundle();
        assert_eq!(bundle.generation(), 1);
        assert_eq!(bundle.len(), 2);

        // Two seedings, same new_generation. Must converge.
        let via_bundle = match IdfBuilder::from_prior(&bundle, 2) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        let via_builder = match IdfBuilder::from_prior_builder(&prior, 2) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(via_bundle.generation(), 2);
        assert_eq!(via_builder.generation(), 2);
        let n_a = match via_bundle.total_docs() {
            Ok(n) => n,
            Err(e) => fatal(&format!("{e}")),
        };
        let n_b = match via_builder.total_docs() {
            Ok(n) => n,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(n_a, n_b);

        let t_b = match via_bundle.finish() {
            Ok(t) => t,
            Err(e) => fatal(&format!("{e}")),
        };
        let t_v = match via_builder.finish() {
            Ok(t) => t,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(t_b, t_v);
    }

    #[test]
    fn state_bundle_cbor_roundtrip() {
        let mut prior = match IdfBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        add(&mut prior, 1, &["foo", "bar"], 2);
        add(&mut prior, 2, &["foo", "baz"], 2);
        let bundle = prior.state_bundle();
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = bundle.serialize_cbor(&mut buf) {
            fatal(&format!("{e}"));
        }
        let got = match IdfStateBundle::deserialize_cbor(buf.as_slice()) {
            Ok(g) => g,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(got, bundle);
    }

    #[test]
    fn state_bundle_cbor_byte_identical_across_runs() {
        let mut a = match IdfBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        let mut b = match IdfBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        add(&mut a, 1, &["foo", "bar"], 2);
        add(&mut a, 2, &["foo", "baz"], 2);
        add(&mut b, 2, &["foo", "baz"], 2);
        add(&mut b, 1, &["foo", "bar"], 2);
        let mut buf_a: Vec<u8> = Vec::new();
        let mut buf_b: Vec<u8> = Vec::new();
        if let Err(e) = a.state_bundle().serialize_cbor(&mut buf_a) {
            fatal(&format!("{e}"));
        }
        if let Err(e) = b.state_bundle().serialize_cbor(&mut buf_b) {
            fatal(&format!("{e}"));
        }
        assert_eq!(buf_a, buf_b);
    }

    #[test]
    fn from_prior_rejects_zero_new_generation() {
        let mut prior = match IdfBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        add(&mut prior, 1, &["foo"], 1);
        let bundle = prior.state_bundle();
        match IdfBuilder::from_prior(&bundle, 0) {
            Ok(_) => assert!(false, "must reject 0"),
            Err(e) => assert_eq!(e.code, ScorerErrorCode::InvalidGeneration),
        }
        match IdfBuilder::from_prior_builder(&prior, 0) {
            Ok(_) => assert!(false, "must reject 0"),
            Err(e) => assert_eq!(e.code, ScorerErrorCode::InvalidGeneration),
        }
    }

    #[test]
    fn upsert_with_same_terms_is_no_op() {
        let mk = || -> IdfBuilder {
            let mut b = match IdfBuilder::new(1) {
                Ok(b) => b,
                Err(e) => fatal(&format!("{e}")),
            };
            add(&mut b, 1, &["foo", "bar"], 2);
            b
        };
        let plain = mk();
        let mut replayed = mk();
        let mut src = SliceTokenSource::new(&["foo", "bar"]);
        if let Err(e) = replayed.upsert_doc(DocId(1), &mut src, 2) {
            fatal(&format!("{e}"));
        }
        let t1 = match plain.finish() {
            Ok(t) => t,
            Err(e) => fatal(&format!("{e}")),
        };
        let t2 = match replayed.finish() {
            Ok(t) => t,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(t1, t2);
    }

    #[test]
    fn docid_roundtrip_via_u64() {
        let d = DocId::from(7u64);
        assert_eq!(d.get(), 7);
        let v: u64 = d.into();
        assert_eq!(v, 7);
    }
}
