//! Deterministic per-generation trigram-index builder.
//!
//! [`TrigramIndexBuilder`] supports three build patterns:
//!
//! 1. **Scratch / append**: construct via `TrigramIndexBuilder::new`, call
//!    `TrigramIndexBuilder::add_doc` for each document, then
//!    `TrigramIndexBuilder::finish`. This
//!    pattern is intentionally NOT replay-safe: re-issuing `add_doc` for
//!    the same `doc_id` with different content is a no-op at the
//!    `(trigram, doc_id)` membership level but does NOT replace prior
//!    content. Callers driving a fresh full-build from an authoritative
//!    snapshot use this pattern.
//!
//! 2. **Upsert-driven** (replay-safe): construct via
//!    `TrigramIndexBuilder::new`, call `TrigramIndexBuilder::upsert_doc`
//!    for each `(doc_id, content)` pair. Re-issuing
//!    `upsert_doc(doc_id, content_b)` after `upsert_doc(doc_id, content_a)`
//!    REPLACES the doc's prior trigram footprint. A delta that may be
//!    re-applied after a crash MUST use this pattern.
//!    `TrigramIndexBuilder::remove_doc` is the matching retirement
//!    handler for a document a scope tombstone or replace removes.
//!
//! 3. **Cross-generation incremental**: construct via
//!    `TrigramIndexBuilder::from_prior`
//!    with the previous generation's [`TrigramIndex`] and a bumped
//!    generation id. Apply any `upsert_doc` / `remove_doc` deltas, then
//!    `TrigramIndexBuilder::finish`. Carries the prior generation's posting
//!    map forward
//!    so that gen N+1 inherits from gen N + new deltas, rather than
//!    rebuilding from scratch.
//!
//! ## Determinism
//!
//! - The on-wire posting map is keyed by a `BTreeMap`, so trigram
//!   iteration order is the lexical sort order of the 3-byte key.
//! - Per-trigram posting lists are sets internally, so they are
//!   automatically sorted and de-duplicated at
//!   `TrigramIndexBuilder::finish` time.
//!   Duplicate inserts of the same `(trigram, doc_id)` pair contribute
//!   exactly one posting entry.
//! - The same upsert/remove sequence applied to two builders of the same
//!   generation produces a byte-identical CBOR encoding of the index.
//!
//! D18 — no proc-macro derives.
//!
//! Producer/search-plane ingress ownership is defined in
//! `docs/adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md`.

use std::collections::{BTreeMap, BTreeSet};

use crate::errors::{TrigramError, TrigramErrorCode};
use crate::index::TrigramIndex;
use crate::types::{DocId, TRIGRAM_LEN, Trigram, trigrams_of};

/// Per-generation trigram builder.
///
/// See the module-level documentation for the three supported build
/// patterns (scratch / upsert-driven / from-prior incremental).
pub struct TrigramIndexBuilder {
    generation: u64,
    by_trigram: BTreeMap<Trigram, BTreeSet<DocId>>,
    by_doc: BTreeMap<DocId, BTreeSet<Trigram>>,
    posting_memberships: usize,
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
            by_doc: BTreeMap::new(),
            posting_memberships: 0,
        })
    }

    /// Construct a builder seeded from a prior generation's index.
    ///
    /// Carries forward every `(trigram, doc_id)` membership from
    /// `prior`. Use this for cross-generation incremental builds: gen N+1
    /// inherits gen N's full posting state and then applies new deltas
    /// via [`Self::upsert_doc`] / [`Self::remove_doc`].
    ///
    /// `new_generation` is the generation id of the builder being
    /// constructed; it does not need to be `prior.generation() + 1`, but
    /// it must be non-zero.
    ///
    /// Cost: O(P + R log D) where P is the total number of postings in
    /// `prior`, D is the number of distinct doc-ids, and R is the
    /// reverse-index size. The reverse map is reconstructed on the fly
    /// from `prior`'s posting lists; persisting the reverse map on disk
    /// is a future optimization.
    pub fn from_prior(prior: &TrigramIndex, new_generation: u64) -> Result<Self, TrigramError> {
        if new_generation == 0 {
            return Err(TrigramError::new(
                TrigramErrorCode::InvalidGeneration,
                "generation must be non-zero",
            ));
        }
        let mut by_trigram: BTreeMap<Trigram, BTreeSet<DocId>> = BTreeMap::new();
        let mut by_doc: BTreeMap<DocId, BTreeSet<Trigram>> = BTreeMap::new();
        let mut posting_memberships = 0_usize;
        for (tri, postings) in prior.iter() {
            let mut set: BTreeSet<DocId> = BTreeSet::new();
            for d in postings {
                let _newly_inserted: bool = set.insert(*d);
                if by_doc.entry(*d).or_default().insert(tri) {
                    posting_memberships = posting_memberships.saturating_add(1);
                }
            }
            if !set.is_empty() {
                let prior_entry = by_trigram.insert(tri, set);
                debug_assert!(prior_entry.is_none(), "iter() emits each trigram once");
            }
        }
        Ok(Self {
            generation: new_generation,
            by_trigram,
            by_doc,
            posting_memberships,
        })
    }

    /// Generation of this builder.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of distinct `(trigram, doc_id)` memberships currently held.
    /// A caller can enforce a build-time memory admission before adding the
    /// next bounded source slice.
    #[must_use]
    pub const fn posting_memberships(&self) -> usize {
        self.posting_memberships
    }

    /// Append a document's byte stream to the builder.
    ///
    /// Inputs shorter than [`TRIGRAM_LEN`] insert zero trigrams; the
    /// document is silently absent from the posting map. The planner's
    /// short-input fast path covers the matching read-side behavior.
    ///
    /// **Not replay-safe.** Calling `add_doc(doc_id, content_a)` then
    /// `add_doc(doc_id, content_b)` UNIONS the two contents' trigram
    /// footprints; the second call does NOT replace the first. Callers
    /// driving a subscriber loop with possible event replay MUST use
    /// [`Self::upsert_doc`] instead.
    pub fn add_doc(&mut self, doc_id: DocId, content: &[u8]) {
        if content.len() < TRIGRAM_LEN {
            return;
        }
        let entry = self.by_doc.entry(doc_id).or_default();
        for tri in trigrams_of(content) {
            let _newly_inserted: bool = self.by_trigram.entry(tri).or_default().insert(doc_id);
            if entry.insert(tri) {
                self.posting_memberships = self.posting_memberships.saturating_add(1);
            }
        }
    }

    /// Idempotent upsert of a document's content.
    ///
    /// If `doc_id` is already present in the builder, REMOVE all of its
    /// prior trigram entries first, then add the new content. Replay-safe:
    /// `upsert_doc(doc_id, content)` is byte-identical at [`Self::finish`]
    /// time regardless of how many times it is invoked, or what prior
    /// content was associated with `doc_id`.
    ///
    /// Inputs shorter than [`TRIGRAM_LEN`] still REMOVE any prior content
    /// for `doc_id` (so an upsert with a 2-byte payload effectively
    /// deletes the doc from the trigram footprint).
    ///
    /// Returns a [`Result`] so future failure modes (e.g. builder caps)
    /// can surface typed errors without an API break.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "API stability: leave Result room for future builder caps per spec"
    )]
    pub fn upsert_doc(&mut self, doc_id: DocId, content: &[u8]) -> Result<(), TrigramError> {
        let _existed: bool = self.remove_doc_internal(doc_id);
        if content.len() < TRIGRAM_LEN {
            return Ok(());
        }
        let entry = self.by_doc.entry(doc_id).or_default();
        for tri in trigrams_of(content) {
            let _newly_inserted: bool = self.by_trigram.entry(tri).or_default().insert(doc_id);
            if entry.insert(tri) {
                self.posting_memberships = self.posting_memberships.saturating_add(1);
            }
        }
        Ok(())
    }

    /// Remove a document from the builder.
    ///
    /// Returns `Ok(true)` if `doc_id` was present and removed; `Ok(false)`
    /// if `doc_id` was not present (no-op). The `false` case is NOT an
    /// error: channel-replay tolerance per the producer/search-plane
    /// contract treats redundant deletes as idempotent.
    ///
    /// Returns a [`Result`] so future failure modes (e.g. builder caps)
    /// can surface typed errors without an API break.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "API stability: leave Result room for future builder caps per spec"
    )]
    pub fn remove_doc(&mut self, doc_id: DocId) -> Result<bool, TrigramError> {
        Ok(self.remove_doc_internal(doc_id))
    }

    /// Internal helper shared by [`Self::upsert_doc`] and
    /// [`Self::remove_doc`]. Returns `true` if the doc existed.
    fn remove_doc_internal(&mut self, doc_id: DocId) -> bool {
        let Some(tris) = self.by_doc.remove(&doc_id) else {
            return false;
        };
        self.posting_memberships = self.posting_memberships.saturating_sub(tris.len());
        for tri in &tris {
            let empty_now = self.by_trigram.get_mut(tri).is_some_and(|postings| {
                let _was_present: bool = postings.remove(&doc_id);
                postings.is_empty()
            });
            if empty_now {
                let _removed: Option<BTreeSet<DocId>> = self.by_trigram.remove(tri);
            }
        }
        true
    }

    /// Finalise the builder into a [`TrigramIndex`].
    ///
    /// Each posting list is materialised as a sorted, de-duplicated
    /// `Vec<DocId>`. The trigram dictionary is the union of every
    /// distinct 3-byte window observed across [`Self::add_doc`] /
    /// [`Self::upsert_doc`] calls that has not subsequently been removed.
    #[must_use]
    pub fn finish(self) -> TrigramIndex {
        let mut out: BTreeMap<Trigram, Vec<DocId>> = BTreeMap::new();
        for (tri, postings) in self.by_trigram {
            if postings.is_empty() {
                continue;
            }
            let vec: Vec<DocId> = postings.into_iter().collect();
            let prior = out.insert(tri, vec);
            debug_assert!(prior.is_none(), "by_trigram is a BTreeMap; key uniqueness");
        }
        TrigramIndex::from_parts(self.generation, out)
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

    // ---- Step 1: upsert_doc idempotency tests --------------------------------

    #[test]
    fn upsert_then_same_upsert_is_byte_identical() {
        // Build A: upsert once.
        let mut a = match TrigramIndexBuilder::new(3) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        if let Err(e) = a.upsert_doc(DocId(42), b"hello world") {
            fatal(&format!("{e}"));
        }
        let idx_a = a.finish();

        // Build B: upsert twice with the same content.
        let mut b = match TrigramIndexBuilder::new(3) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        if let Err(e) = b.upsert_doc(DocId(42), b"hello world") {
            fatal(&format!("{e}"));
        }
        if let Err(e) = b.upsert_doc(DocId(42), b"hello world") {
            fatal(&format!("{e}"));
        }
        let idx_b = b.finish();

        let mut buf_a: Vec<u8> = Vec::new();
        let mut buf_b: Vec<u8> = Vec::new();
        if let Err(e) = idx_a.serialize_cbor(&mut buf_a) {
            fatal(&format!("{e}"));
        }
        if let Err(e) = idx_b.serialize_cbor(&mut buf_b) {
            fatal(&format!("{e}"));
        }
        assert_eq!(buf_a, buf_b, "double-upsert must be byte-identical");
    }

    #[test]
    fn upsert_replaces_content() {
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        if let Err(e) = b.upsert_doc(DocId(42), b"foo_payload") {
            fatal(&format!("{e}"));
        }
        if let Err(e) = b.upsert_doc(DocId(42), b"bar_payload") {
            fatal(&format!("{e}"));
        }
        let idx = b.finish();
        // "foo" no longer references doc 42 because it was overwritten.
        let empty: Vec<DocId> = Vec::new();
        assert_eq!(idx.lookup(*b"foo").to_vec(), empty);
        // "bar" should reference doc 42.
        assert_eq!(idx.lookup(*b"bar"), &[DocId(42)]);
    }

    #[test]
    fn upsert_short_content_clears_doc() {
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        if let Err(e) = b.upsert_doc(DocId(7), b"hello") {
            fatal(&format!("{e}"));
        }
        // Upsert with a 2-byte payload clears doc 7 from the index.
        if let Err(e) = b.upsert_doc(DocId(7), b"hi") {
            fatal(&format!("{e}"));
        }
        let idx = b.finish();
        let empty: Vec<DocId> = Vec::new();
        assert_eq!(idx.lookup(*b"hel").to_vec(), empty);
        assert_eq!(idx.lookup(*b"ell").to_vec(), empty);
        assert_eq!(idx.lookup(*b"llo").to_vec(), empty);
    }

    // ---- Step 2: remove_doc tests -------------------------------------------

    #[test]
    fn remove_existing_doc_removes_all_trigrams() {
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        if let Err(e) = b.upsert_doc(DocId(1), b"abcdef") {
            fatal(&format!("{e}"));
        }
        if let Err(e) = b.upsert_doc(DocId(2), b"xyzdef") {
            fatal(&format!("{e}"));
        }
        let removed = match b.remove_doc(DocId(1)) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(removed);
        let idx = b.finish();
        // doc 1's trigrams "abc", "bcd", "cde" should no longer reference doc 1.
        let empty: Vec<DocId> = Vec::new();
        assert_eq!(idx.lookup(*b"abc").to_vec(), empty);
        assert_eq!(idx.lookup(*b"bcd").to_vec(), empty);
        assert_eq!(idx.lookup(*b"cde").to_vec(), empty);
        // doc 2's trigrams remain.
        assert_eq!(idx.lookup(*b"xyz"), &[DocId(2)]);
        assert_eq!(idx.lookup(*b"yzd"), &[DocId(2)]);
        // "def" is shared between doc 1 and doc 2 originally; after removing
        // doc 1 it must only reference doc 2.
        assert_eq!(idx.lookup(*b"def"), &[DocId(2)]);
    }

    #[test]
    fn remove_nonexistent_doc_returns_false_no_error() {
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        let r = match b.remove_doc(DocId(999)) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(!r);
        // Removing again is still a typed false.
        let r2 = match b.remove_doc(DocId(999)) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(!r2);
    }

    #[test]
    fn remove_then_upsert_same_id_starts_fresh() {
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        if let Err(e) = b.upsert_doc(DocId(7), b"alpha") {
            fatal(&format!("{e}"));
        }
        let r = match b.remove_doc(DocId(7)) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(r);
        if let Err(e) = b.upsert_doc(DocId(7), b"omega") {
            fatal(&format!("{e}"));
        }
        let idx = b.finish();
        // "alp", "lph", "pha" gone; "ome", "meg", "ega" present.
        let empty: Vec<DocId> = Vec::new();
        assert_eq!(idx.lookup(*b"alp").to_vec(), empty);
        assert_eq!(idx.lookup(*b"lph").to_vec(), empty);
        assert_eq!(idx.lookup(*b"pha").to_vec(), empty);
        assert_eq!(idx.lookup(*b"ome"), &[DocId(7)]);
        assert_eq!(idx.lookup(*b"meg"), &[DocId(7)]);
        assert_eq!(idx.lookup(*b"ega"), &[DocId(7)]);
    }

    // ---- Step 3: from_prior tests -------------------------------------------

    #[test]
    fn from_prior_preserves_all_docs() {
        let mut a = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        a.add_doc(DocId(1), b"abcdef");
        a.add_doc(DocId(2), b"xyzabc");
        a.add_doc(DocId(3), b"abcabc");
        let prior = a.finish();

        let next = match TrigramIndexBuilder::from_prior(&prior, 2) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        let next_idx = next.finish();
        assert_eq!(next_idx.generation(), 2);
        // Posting lists should be identical to prior.
        for (tri, postings) in prior.iter() {
            assert_eq!(next_idx.lookup(tri), postings);
        }
        assert_eq!(prior.distinct_trigrams(), next_idx.distinct_trigrams());
    }

    #[test]
    fn from_prior_with_deletes_drops_docs() {
        let mut a = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        a.add_doc(DocId(1), b"abcdef");
        a.add_doc(DocId(2), b"xyzabc");
        let prior = a.finish();
        let mut next = match TrigramIndexBuilder::from_prior(&prior, 2) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        let removed = match next.remove_doc(DocId(1)) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(removed);
        let idx = next.finish();
        // doc 1's exclusive trigrams gone.
        let empty: Vec<DocId> = Vec::new();
        assert_eq!(idx.lookup(*b"bcd").to_vec(), empty);
        assert_eq!(idx.lookup(*b"cde").to_vec(), empty);
        assert_eq!(idx.lookup(*b"def").to_vec(), empty);
        // shared "abc" still references doc 2.
        assert_eq!(idx.lookup(*b"abc"), &[DocId(2)]);
    }

    #[test]
    fn from_prior_with_upserts_replaces() {
        let mut a = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        a.add_doc(DocId(1), b"old_content");
        let prior = a.finish();
        let mut next = match TrigramIndexBuilder::from_prior(&prior, 2) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        if let Err(e) = next.upsert_doc(DocId(1), b"new_content") {
            fatal(&format!("{e}"));
        }
        let idx = next.finish();
        // "old" trigrams no longer reference doc 1.
        let empty: Vec<DocId> = Vec::new();
        assert_eq!(idx.lookup(*b"old").to_vec(), empty);
        // "new" trigrams do.
        assert_eq!(idx.lookup(*b"new"), &[DocId(1)]);
        assert_eq!(idx.lookup(*b"ew_"), &[DocId(1)]);
    }

    #[test]
    fn from_prior_rejects_zero_generation() {
        let a = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        let prior = a.finish();
        match TrigramIndexBuilder::from_prior(&prior, 0) {
            Ok(_) => assert!(false, "must reject generation=0"),
            Err(e) => assert_eq!(e.code, TrigramErrorCode::InvalidGeneration),
        }
    }

    // ---- Sanity: add_doc behaves like before for duplicate insertion --------

    #[test]
    fn add_doc_duplicate_pairs_collapse() {
        // Same content twice via add_doc: posting lists must still be
        // deduplicated. This is the documented short-circuit on the
        // (trigram, doc_id) membership level; add_doc does NOT replace.
        let mut a = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        a.add_doc(DocId(1), b"abc");
        a.add_doc(DocId(1), b"abc");
        assert_eq!(a.posting_memberships(), 1);
        let idx = a.finish();
        assert_eq!(idx.lookup(*b"abc"), &[DocId(1)]);
    }

    #[test]
    fn posting_memberships_follow_upsert_and_delete() {
        let mut builder = TrigramIndexBuilder::new(1).expect("builder");
        builder.add_doc(DocId(1), b"abcd");
        assert_eq!(builder.posting_memberships(), 2);
        builder.add_doc(DocId(2), b"abc");
        assert_eq!(builder.posting_memberships(), 3);
        builder.upsert_doc(DocId(1), b"xyz").expect("upsert");
        assert_eq!(builder.posting_memberships(), 2);
        assert!(builder.remove_doc(DocId(2)).expect("remove"));
        assert_eq!(builder.posting_memberships(), 1);
        let prior = builder.finish();
        let next = TrigramIndexBuilder::from_prior(&prior, 2).expect("next");
        assert_eq!(next.posting_memberships(), 1);
    }
}
