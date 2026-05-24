//! Deterministic per-generation trigram-index builder.
//!
//! [`TrigramIndexBuilder`] consumes `(DocId, &[u8])` pairs, slides a
//! 3-byte window over each input, and finalises into a [`TrigramIndex`].
//! Output is deterministic:
//!
//! 1. The on-wire posting map is keyed by a `BTreeMap`, so trigram
//!    iteration order is the lexical sort order of the 3-byte key.
//! 2. Per-trigram posting lists are sorted and de-duplicated at
//!    [`TrigramIndexBuilder::finish`] time, so duplicate inserts of the
//!    same `(trigram, doc_id)` pair contribute exactly one posting entry.
//!
//! D18 — no proc-macro derives.

use std::collections::BTreeMap;

use crate::errors::{TrigramError, TrigramErrorCode};
use crate::index::TrigramIndex;
use crate::types::{DocId, TRIGRAM_LEN, Trigram, trigrams_of};

/// Per-generation trigram builder.
pub struct TrigramIndexBuilder {
    generation: u64,
    by_trigram: BTreeMap<Trigram, Vec<DocId>>,
}

impl TrigramIndexBuilder {
    /// Construct a fresh builder for `generation`.
    ///
    /// Returns [`TrigramErrorCode::InvalidGeneration`] if `generation`
    /// is `0`.
    pub fn new(generation: u64) -> Result<Self, TrigramError> {
        if generation == 0 {
            return Err(TrigramError::new(
                TrigramErrorCode::InvalidGeneration,
                "generation must be non-zero",
            ));
        }
        Ok(Self {
            generation,
            by_trigram: BTreeMap::new(),
        })
    }

    /// Generation of this builder.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Add a document's byte stream to the builder.
    ///
    /// Inputs shorter than [`TRIGRAM_LEN`] insert zero trigrams; the
    /// document is silently absent from the posting map. The planner's
    /// short-input fast path covers the matching read-side behavior.
    pub fn add_doc(&mut self, doc_id: DocId, content: &[u8]) {
        if content.len() < TRIGRAM_LEN {
            return;
        }
        for tri in trigrams_of(content) {
            self.by_trigram.entry(tri).or_default().push(doc_id);
        }
    }

    /// Finalise the builder into a [`TrigramIndex`].
    ///
    /// Each posting list is sorted by `DocId` ascending and de-duplicated.
    /// The trigram dictionary is the union of every distinct 3-byte
    /// window observed across [`Self::add_doc`] calls.
    #[must_use]
    pub fn finish(mut self) -> TrigramIndex {
        for postings in self.by_trigram.values_mut() {
            postings.sort_unstable();
            postings.dedup();
        }
        TrigramIndex::from_parts(self.generation, self.by_trigram)
    }
}

#[cfg(test)]
mod tests {
    use super::TrigramIndexBuilder;
    use crate::errors::TrigramErrorCode;
    use crate::types::DocId;

    fn fatal(msg: &str) -> ! {
        assert!(false, "{msg}");
        std::process::abort();
    }

    #[test]
    fn rejects_zero_generation() {
        match TrigramIndexBuilder::new(0) {
            Ok(_) => assert!(false, "must reject generation=0"),
            Err(e) => assert_eq!(e.code, TrigramErrorCode::InvalidGeneration),
        }
    }

    #[test]
    fn empty_builder_finishes_to_empty_index() {
        let b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        let idx = b.finish();
        assert_eq!(idx.generation(), 1);
        let empty: Vec<DocId> = Vec::new();
        assert_eq!(idx.lookup(*b"abc").to_vec(), empty);
    }

    #[test]
    fn short_doc_inserts_no_trigrams() {
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        b.add_doc(DocId(1), b"ab");
        let idx = b.finish();
        assert_eq!(idx.distinct_trigrams(), 0);
    }

    #[test]
    fn exact_three_bytes_inserts_one_trigram() {
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        b.add_doc(DocId(1), b"abc");
        let idx = b.finish();
        assert_eq!(idx.distinct_trigrams(), 1);
        assert_eq!(idx.lookup(*b"abc"), &[DocId(1)]);
    }

    #[test]
    fn posting_lists_are_sorted_and_deduped() {
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        b.add_doc(DocId(3), b"abcabc");
        b.add_doc(DocId(1), b"xyzabc");
        b.add_doc(DocId(2), b"abc");
        let idx = b.finish();
        // "abc" appears in all three docs; in doc 3 twice — must dedup.
        let got = idx.lookup(*b"abc").to_vec();
        assert_eq!(got, vec![DocId(1), DocId(2), DocId(3)]);
    }

    #[test]
    fn multi_byte_doc_emits_all_windows() {
        let mut b = match TrigramIndexBuilder::new(7) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        b.add_doc(DocId(1), b"abcd");
        let idx = b.finish();
        assert_eq!(idx.distinct_trigrams(), 2);
        assert_eq!(idx.lookup(*b"abc"), &[DocId(1)]);
        assert_eq!(idx.lookup(*b"bcd"), &[DocId(1)]);
        assert_eq!(idx.generation(), 7);
    }
}
