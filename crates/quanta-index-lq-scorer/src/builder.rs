//! Deterministic per-generation IDF table builder.
//!
//! `IdfBuilder` aggregates document-frequency statistics across a corpus
//! and finalises into an [`crate::idf::IdfTable`]. The result is
//! deterministic in two senses:
//!
//! 1. The output `IdfTable` is independent of the order in which documents
//!    are added — `term_doc_counts` is a `BTreeMap` keyed on owned term
//!    bytes.
//! 2. Within a single document, repeated terms contribute exactly one to
//!    the term's document-frequency.
//!
//! D18 — no proc-macro derives.

use std::collections::{BTreeMap, BTreeSet};

use crate::errors::{ScorerError, ScorerErrorCode};
use crate::idf::IdfTable;
use crate::scorer::IdfTokenSource;

/// Per-generation IDF builder.
pub struct IdfBuilder {
    generation: u64,
    term_doc_counts: BTreeMap<Box<str>, u64>,
    total_doc_len: u128,
    total_docs: u64,
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
            total_doc_len: 0,
            total_docs: 0,
        })
    }

    /// Add a document's term stream to the builder.
    ///
    /// `doc_len` is the document's analyzer-emitted token count. Duplicate
    /// terms within a single document contribute exactly one to the term's
    /// document-frequency.
    pub fn add_doc(&mut self, terms: &mut dyn IdfTokenSource, doc_len: u32) {
        let mut seen: BTreeSet<Box<str>> = BTreeSet::new();
        while let Some(term) = terms.next_term() {
            let owned: Box<str> = term.into();
            if seen.insert(owned.clone()) {
                let entry = self.term_doc_counts.entry(owned).or_insert(0);
                *entry = entry.saturating_add(1);
            }
        }
        self.total_doc_len = self.total_doc_len.saturating_add(u128::from(doc_len));
        self.total_docs = self.total_docs.saturating_add(1);
    }

    /// Finalise the builder into an `IdfTable`.
    pub fn finish(self) -> Result<IdfTable, ScorerError> {
        if self.total_docs == 0 {
            return Err(ScorerError::new(
                ScorerErrorCode::EmptyCorpus,
                "IdfBuilder requires at least one document before finish()",
            ));
        }
        let avg = u128_to_f64(self.total_doc_len) / u64_to_f64(self.total_docs);
        IdfTable::new(self.generation, self.total_docs, self.term_doc_counts, avg)
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn total_docs(&self) -> u64 {
        self.total_docs
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
    use super::IdfBuilder;
    use crate::errors::ScorerErrorCode;
    use crate::scorer::SliceTokenSource;

    fn fatal(msg: &str) -> ! {
        assert!(false, "{msg}");
        std::process::abort();
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
        let terms = ["foo", "foo", "foo"];
        let mut src = SliceTokenSource::new(&terms);
        b.add_doc(&mut src, 3);
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
        let doc_a = ["foo", "bar"];
        let doc_b = ["foo", "baz"];
        let doc_c = ["bar", "baz"];
        let mut sa = SliceTokenSource::new(&doc_a);
        b.add_doc(&mut sa, 2);
        let mut sb = SliceTokenSource::new(&doc_b);
        b.add_doc(&mut sb, 2);
        let mut sc = SliceTokenSource::new(&doc_c);
        b.add_doc(&mut sc, 2);
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
        let mk = |order: &[usize]| -> crate::idf::IdfTable {
            let mut b = match IdfBuilder::new(1) {
                Ok(b) => b,
                Err(e) => fatal(&format!("{e}")),
            };
            let docs: [&[&str]; 3] = [&["foo", "bar"], &["foo", "baz"], &["bar", "baz"]];
            for &i in order {
                let Some(&d) = docs.get(i) else { continue };
                let mut s = SliceTokenSource::new(d);
                b.add_doc(&mut s, 2);
            }
            match b.finish() {
                Ok(t) => t,
                Err(e) => fatal(&format!("{e}")),
            }
        };
        let t1 = mk(&[0, 1, 2]);
        let t2 = mk(&[2, 0, 1]);
        let t3 = mk(&[1, 2, 0]);
        assert_eq!(t1, t2);
        assert_eq!(t2, t3);
    }
}
