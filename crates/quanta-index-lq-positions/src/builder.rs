//! Position-index builder.
//!
//! Accumulates `(term, doc_id, position)` triples in a deterministic
//! `BTreeMap` and finalises into a [`PositionsIndex`] whose per-term
//! posting list is delta-encoded varint bytes.
//!
//! Determinism: `BTreeMap` iteration order is byte-lex on the term key and
//! ascending on `DocId`, so the same insertion multiset emits a
//! byte-identical posting-list payload regardless of insertion order.
//!
//! # Two ingestion patterns
//!
//! 1. **Fresh-build (token-stream)**: drive the builder with
//!    [`PositionsBuilder::add_token`] for each `(doc_id, term, position)`
//!    observation in the normalizer's emit order. Suitable for first-time
//!    indexing of a generation with no prior state. The caller owns
//!    idempotency: re-emitting the same `(doc_id, term, position)` produces
//!    a duplicate position.
//!
//! 2. **Idempotent-replay (per-doc upsert / delete)**: drive the builder
//!    with [`PositionsBuilder::upsert_doc`] (replaces every `(term,
//!    doc_id, *)` cell atomically) and [`PositionsBuilder::remove_doc`]
//!    (drops every `(term, doc_id, *)` cell for `doc_id`). Suitable for
//!    delta re-application: the same scope replace applied twice
//!    yields the same builder state. Combine with
//!    [`PositionsBuilder::from_prior`] to import the previous generation's
//!    posting lists into a new generation before applying the delta.

use std::collections::BTreeMap;

use crate::errors::{LimitDimension, PositionsError, PositionsErrorCode};
use crate::index::PositionsIndex;
use crate::types::{DocId, MAX_DOCS_PER_TERM, MAX_POSITIONS_PER_CELL, NormalizerVersion, Position};
use crate::varint::encode_u32;

/// Mutable, in-memory accumulator for a per-generation position index.
pub struct PositionsBuilder {
    generation: u64,
    normalizer_version: NormalizerVersion,
    by_term: BTreeMap<Box<str>, BTreeMap<DocId, Vec<Position>>>,
}

impl PositionsBuilder {
    /// Construct an empty builder against `(generation, normalizer_version)`.
    #[must_use]
    pub fn new(generation: u64, normalizer_version: NormalizerVersion) -> Self {
        Self {
            generation,
            normalizer_version,
            by_term: BTreeMap::new(),
        }
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn normalizer_version(&self) -> NormalizerVersion {
        self.normalizer_version
    }

    /// Record one normalized token observation.
    ///
    /// Caller owns idempotency: emitting the same `(doc_id, term, pos)`
    /// twice yields a duplicate position. The builder does not dedupe —
    /// duplicate positions on the on-disk posting list signal a caller bug.
    ///
    /// Cap (LEX-03 §4.2): a single `(term, doc)` cell is bounded by
    /// [`MAX_POSITIONS_PER_CELL`]. The (cap + 1)-th `add_token` for that
    /// cell fails closed with [`PositionsErrorCode::PlanLimitExceeded`] and
    /// [`LimitDimension::PositionsPerCell`]; the offending position is
    /// **not** appended — the builder rejects rather than truncates.
    ///
    /// Breaking-change note: prior to LEX-03 §4.2 this method returned `()`.
    /// Per CLAUDE.md "breaking-first" posture there is no compat shim.
    pub fn add_token(
        &mut self,
        doc_id: DocId,
        term: &str,
        pos: Position,
    ) -> Result<(), PositionsError> {
        let key: Box<str> = Box::from(term);
        let entry = self.by_term.entry(key).or_default();
        let positions = entry.entry(doc_id).or_default();
        // `positions.len()` is `usize`; convert via `u32::try_from` so the
        // cap check is exact regardless of platform width. The cap is
        // `4_096`, so `len < cap` is well below `u32::MAX`.
        let cur_len = u32::try_from(positions.len()).map_err(|_err| {
            PositionsError::new(
                PositionsErrorCode::IndexCorrupted,
                "positions.len() exceeds u32",
            )
        })?;
        if cur_len >= MAX_POSITIONS_PER_CELL {
            return Err(PositionsError::plan_limit_exceeded(
                LimitDimension::PositionsPerCell,
                format!("positions in (term, doc) cell would exceed cap={MAX_POSITIONS_PER_CELL}"),
            ));
        }
        positions.push(pos);
        Ok(())
    }

    /// Idempotent per-doc replacement.
    ///
    /// Drops every `(term, doc_id, *)` cell for `doc_id` across all terms,
    /// then re-applies the supplied `(term, position)` pairs. The producer
    /// emits the doc's COMPLETE final tokenization for the current
    /// generation; this method does not merge with prior tokens for the
    /// same doc — it replaces them.
    ///
    /// Idempotency: calling `upsert_doc(d, pairs)` twice yields the same
    /// builder state as calling it once; the resulting [`finish`] output
    /// is byte-identical between the two runs.
    ///
    /// Cap (LEX-03 §4.2): per-`(term, doc_id)` position count is bounded
    /// by [`MAX_POSITIONS_PER_CELL`]. The (cap + 1)-th pair targeting the
    /// same `(term, doc_id)` cell fails closed with
    /// [`PositionsErrorCode::PlanLimitExceeded`] /
    /// [`LimitDimension::PositionsPerCell`].
    ///
    /// Failure semantics: when the cap is tripped, the prior state for
    /// `doc_id` has already been cleared. The builder is left with the
    /// pairs accepted up to (but not including) the offending one, which
    /// reflects the fail-closed posture — callers MUST treat a returned
    /// `Err` as "this doc must be re-upserted from a clean slate or
    /// removed" rather than as a partial-success signal.
    ///
    /// [`finish`]: PositionsBuilder::finish
    pub fn upsert_doc<'a, I>(
        &mut self,
        doc_id: DocId,
        term_position_pairs: I,
    ) -> Result<(), PositionsError>
    where
        I: IntoIterator<Item = (&'a str, Position)>,
    {
        // Step 1: drop every (term, doc_id, *) cell for doc_id.
        // Visit each term's per-doc map and remove `doc_id`. If removing it
        // leaves the term map empty, drop the term entirely so the
        // serialized layout never contains zero-doc terms.
        let mut emptied_terms: Vec<Box<str>> = Vec::new();
        for (term, docs) in &mut self.by_term {
            if docs.remove(&doc_id).is_some() && docs.is_empty() {
                emptied_terms.push(term.clone());
            }
        }
        for term in emptied_terms {
            drop(self.by_term.remove(&term));
        }

        // Step 2: re-apply pairs. Cap enforcement mirrors `add_token`.
        for (term, pos) in term_position_pairs {
            let key: Box<str> = Box::from(term);
            let entry = self.by_term.entry(key).or_default();
            let positions = entry.entry(doc_id).or_default();
            let cur_len = u32::try_from(positions.len()).map_err(|_err| {
                PositionsError::new(
                    PositionsErrorCode::IndexCorrupted,
                    "positions.len() exceeds u32",
                )
            })?;
            if cur_len >= MAX_POSITIONS_PER_CELL {
                return Err(PositionsError::plan_limit_exceeded(
                    LimitDimension::PositionsPerCell,
                    format!(
                        "positions in (term, doc) cell would exceed cap={MAX_POSITIONS_PER_CELL}"
                    ),
                ));
            }
            positions.push(pos);
        }
        Ok(())
    }

    /// Idempotent per-doc removal.
    ///
    /// Drops every `(term, doc_id, *)` cell for `doc_id` across all terms.
    /// Empty terms (terms whose only doc was `doc_id`) are removed from
    /// the builder so the serialized layout never contains zero-doc terms.
    ///
    /// Returns `Ok(true)` if any cell was removed; `Ok(false)` if `doc_id`
    /// was already absent. Calling `remove_doc` twice for the same id is
    /// safe — the second call returns `Ok(false)` with no error.
    ///
    /// This API currently cannot fail; the `Result` return is preserved
    /// for forward compatibility with cap dimensions that may apply to
    /// removal in future revisions (e.g. a per-tick removal budget).
    #[expect(
        clippy::unnecessary_wraps,
        reason = "Result return is part of the published API contract — forward-compat hook for future per-tick removal-budget caps; collapsing to bool now would force a wire break later"
    )]
    pub fn remove_doc(&mut self, doc_id: DocId) -> Result<bool, PositionsError> {
        let mut removed_any = false;
        let mut emptied_terms: Vec<Box<str>> = Vec::new();
        for (term, docs) in &mut self.by_term {
            if docs.remove(&doc_id).is_some() {
                removed_any = true;
                if docs.is_empty() {
                    emptied_terms.push(term.clone());
                }
            }
        }
        for term in emptied_terms {
            drop(self.by_term.remove(&term));
        }
        Ok(removed_any)
    }

    /// Import a prior generation's posting lists into a fresh builder.
    ///
    /// Decodes `prior`'s per-term posting lists (delta-varint) into the
    /// in-memory `BTreeMap` so the new builder starts populated with the
    /// prior generation's `(term, doc_id, positions)` state. The returned
    /// builder is stamped with `new_generation` and `new_normalizer_version`
    /// — *not* the prior's stamps. The caller then applies the new
    /// generation's delta via [`upsert_doc`] / [`remove_doc`] before
    /// calling [`finish`].
    ///
    /// Normalizer compatibility (fail-closed): if `new_normalizer_version`
    /// differs from `prior.normalizer_version()`, returns
    /// [`PositionsErrorCode::NormalizerVersionMismatch`]. Carrying a
    /// position posting list across a normalizer change would couple
    /// post-normalize token positions from incompatible token streams.
    ///
    /// Cap enforcement: per-term doc counts are validated against
    /// [`MAX_DOCS_PER_TERM`] during import. A prior whose posting list
    /// already exceeds the cap fails closed with
    /// [`PositionsErrorCode::PlanLimitExceeded`] /
    /// [`LimitDimension::DocsPerTerm`].
    ///
    /// [`upsert_doc`]: PositionsBuilder::upsert_doc
    /// [`remove_doc`]: PositionsBuilder::remove_doc
    /// [`finish`]: PositionsBuilder::finish
    pub fn from_prior(
        prior: &PositionsIndex,
        new_generation: u64,
        new_normalizer_version: NormalizerVersion,
    ) -> Result<Self, PositionsError> {
        if prior.normalizer_version() != new_normalizer_version {
            return Err(PositionsError::new(
                PositionsErrorCode::NormalizerVersionMismatch,
                format!(
                    "from_prior: prior normalizer_version={} != requested={}",
                    prior.normalizer_version(),
                    new_normalizer_version
                ),
            ));
        }
        let mut by_term: BTreeMap<Box<str>, BTreeMap<DocId, Vec<Position>>> = BTreeMap::new();
        for term in prior.terms() {
            let Some(iter) = prior.term_postings(term) else {
                // Listed in `terms()` but absent from the underlying map
                // would indicate a corrupted index; report it explicitly.
                return Err(PositionsError::new(
                    PositionsErrorCode::IndexCorrupted,
                    format!("from_prior: term '{term}' listed but missing"),
                ));
            };
            let mut per_doc: BTreeMap<DocId, Vec<Position>> = BTreeMap::new();
            let mut doc_count: u32 = 0;
            for entry_res in iter {
                let entry = entry_res?;
                doc_count = doc_count.checked_add(1).ok_or_else(|| {
                    PositionsError::new(
                        PositionsErrorCode::IndexCorrupted,
                        "from_prior: doc_count overflow",
                    )
                })?;
                if doc_count > MAX_DOCS_PER_TERM {
                    return Err(PositionsError::plan_limit_exceeded(
                        LimitDimension::DocsPerTerm,
                        format!("term '{term}' has >{MAX_DOCS_PER_TERM} docs in prior index",),
                    ));
                }
                drop(per_doc.insert(entry.doc_id, entry.positions));
            }
            drop(by_term.insert(Box::from(term), per_doc));
        }
        Ok(Self {
            generation: new_generation,
            normalizer_version: new_normalizer_version,
            by_term,
        })
    }

    /// Finalise into an immutable [`PositionsIndex`].
    ///
    /// Positions within a `(term, doc)` cell are sorted ascending before
    /// delta-encoding so the wire shape is canonical even if the caller
    /// emitted positions out of order.
    pub fn finish(self) -> Result<PositionsIndex, PositionsError> {
        let mut out_by_term: BTreeMap<Box<str>, Vec<u8>> = BTreeMap::new();
        for (term, docs) in self.by_term {
            let mut buf: Vec<u8> = Vec::new();
            // varint(doc_count)
            let doc_count = u32::try_from(docs.len()).map_err(|_err| {
                PositionsError::new(
                    crate::errors::PositionsErrorCode::IndexCorrupted,
                    "doc_count exceeds u32",
                )
            })?;
            // LEX-03 §4.2 per-term cap: reject before emitting any bytes
            // for this term so the encoder never half-commits a posting
            // list. Fail-closed; no truncation.
            if doc_count > MAX_DOCS_PER_TERM {
                return Err(PositionsError::plan_limit_exceeded(
                    LimitDimension::DocsPerTerm,
                    format!("term '{term}' has {doc_count} docs, exceeds cap={MAX_DOCS_PER_TERM}"),
                ));
            }
            encode_u32(doc_count, &mut buf);
            let mut prev_doc: u64 = 0;
            let mut first_doc = true;
            for (doc_id, mut positions) in docs {
                let cur = doc_id.0;
                let gap = if first_doc {
                    first_doc = false;
                    cur
                } else {
                    cur.checked_sub(prev_doc).ok_or_else(|| {
                        PositionsError::new(
                            crate::errors::PositionsErrorCode::IndexCorrupted,
                            "doc gap underflow",
                        )
                    })?
                };
                let gap_u32 = u32::try_from(gap).map_err(|_err| {
                    PositionsError::new(
                        crate::errors::PositionsErrorCode::IndexCorrupted,
                        "doc gap exceeds u32",
                    )
                })?;
                encode_u32(gap_u32, &mut buf);
                prev_doc = cur;

                positions.sort_unstable();
                let pos_count = u32::try_from(positions.len()).map_err(|_err| {
                    PositionsError::new(
                        crate::errors::PositionsErrorCode::IndexCorrupted,
                        "position_count exceeds u32",
                    )
                })?;
                encode_u32(pos_count, &mut buf);
                let mut prev_pos: u32 = 0;
                let mut first_pos = true;
                for p in positions {
                    let cur_pos = p.0;
                    let pos_gap = if first_pos {
                        first_pos = false;
                        cur_pos
                    } else {
                        cur_pos.checked_sub(prev_pos).ok_or_else(|| {
                            PositionsError::new(
                                crate::errors::PositionsErrorCode::IndexCorrupted,
                                "position gap underflow",
                            )
                        })?
                    };
                    encode_u32(pos_gap, &mut buf);
                    prev_pos = cur_pos;
                }
            }
            // BTreeMap iteration yields unique keys, so re-insert into a
            // fresh BTreeMap below cannot collide. The `let _ = ...` form
            // would trigger `clippy::let_underscore_must_use`; drop the
            // return explicitly instead.
            drop(out_by_term.insert(term, buf));
        }
        Ok(PositionsIndex::from_raw(
            self.generation,
            self.normalizer_version,
            out_by_term,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::PositionsBuilder;
    use crate::errors::{LimitDimension, PositionsErrorCode};
    use crate::types::{DocId, MAX_POSITIONS_PER_CELL, NormalizerVersion, Position};

    #[test]
    fn builder_finishes_empty() {
        let b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        match b.finish() {
            Ok(idx) => assert_eq!(idx.generation(), 1),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn builder_preserves_generation_and_normalizer() {
        let nv = NormalizerVersion::new(2, 5);
        let mut b = PositionsBuilder::new(99, nv);
        match b.add_token(DocId(0), "x", Position(0)) {
            Ok(()) => {}
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        }
        match b.finish() {
            Ok(idx) => {
                assert_eq!(idx.generation(), 99);
                assert_eq!(idx.normalizer_version(), nv);
            }
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn builder_orders_positions_canonically() {
        // Insert out-of-order positions; expect identical encoding to sorted
        // insertion via the deterministic `finish` step.
        let mut a = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        for p in [5u32, 1, 3] {
            if let Err(e) = a.add_token(DocId(0), "t", Position(p)) {
                assert!(false, "{e}");
                return;
            }
        }
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        for p in [1u32, 3, 5] {
            if let Err(e) = b.add_token(DocId(0), "t", Position(p)) {
                assert!(false, "{e}");
                return;
            }
        }
        let ia = match a.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let ib = match b.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut ba: Vec<u8> = Vec::new();
        let mut bb: Vec<u8> = Vec::new();
        if let Err(e) = ia.serialize_cbor(&mut ba) {
            assert!(false, "{e}");
        }
        if let Err(e) = ib.serialize_cbor(&mut bb) {
            assert!(false, "{e}");
        }
        assert_eq!(ba, bb, "insertion order must not affect encoding");
    }

    #[test]
    fn add_token_accepts_exactly_cap_positions_for_cell() {
        // Inserting MAX_POSITIONS_PER_CELL distinct positions must all
        // succeed; the (cap+1)-th must fail closed.
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        for i in 0..MAX_POSITIONS_PER_CELL {
            match b.add_token(DocId(0), "t", Position(i)) {
                Ok(()) => {}
                Err(e) => {
                    assert!(false, "unexpected failure at i={i}: {e}");
                    return;
                }
            }
        }
    }

    #[test]
    fn add_token_rejects_beyond_cell_cap_with_plan_limit_exceeded() {
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        for i in 0..MAX_POSITIONS_PER_CELL {
            if let Err(e) = b.add_token(DocId(0), "t", Position(i)) {
                assert!(false, "{e}");
                return;
            }
        }
        // The (cap+1)-th insertion must be rejected.
        match b.add_token(DocId(0), "t", Position(MAX_POSITIONS_PER_CELL)) {
            Ok(()) => assert!(false, "expected PlanLimitExceeded"),
            Err(e) => {
                assert_eq!(e.code, PositionsErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::PositionsPerCell));
            }
        }
    }

    #[test]
    fn add_token_cell_cap_does_not_truncate_existing_positions() {
        // After the cap is hit, the rejected position is NOT appended.
        // The builder must contain exactly MAX_POSITIONS_PER_CELL entries
        // for that (term, doc) cell — fail-closed, not fail-truncate.
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        for i in 0..MAX_POSITIONS_PER_CELL {
            if let Err(e) = b.add_token(DocId(0), "t", Position(i)) {
                assert!(false, "{e}");
                return;
            }
        }
        // Intentionally drop the rejection — the assertion below verifies
        // the rejected position was not appended.
        drop(b.add_token(DocId(0), "t", Position(99_999)));
        // Finish must still succeed with exactly cap positions for the cell.
        let idx = match b.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut count: u32 = 0;
        let Some(iter) = idx.term_postings("t") else {
            assert!(false, "term 't' missing");
            return;
        };
        for r in iter {
            let entry = match r {
                Ok(v) => v,
                Err(e) => {
                    assert!(false, "{e}");
                    return;
                }
            };
            let Ok(len) = u32::try_from(entry.positions.len()) else {
                assert!(false, "positions length overflow");
                return;
            };
            count = count.saturating_add(len);
        }
        assert_eq!(count, MAX_POSITIONS_PER_CELL);
    }

    #[test]
    fn add_token_cell_cap_is_per_cell_not_global() {
        // The cap is per `(term, doc)` cell — different (term, doc) pairs
        // each get a fresh budget.
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        // Fill cell (term="a", doc=0) to the cap.
        for i in 0..MAX_POSITIONS_PER_CELL {
            if let Err(e) = b.add_token(DocId(0), "a", Position(i)) {
                assert!(false, "{e}");
                return;
            }
        }
        // A different term in the same doc must still accept new positions.
        match b.add_token(DocId(0), "b", Position(0)) {
            Ok(()) => {}
            Err(e) => assert!(false, "different term must not share cap: {e}"),
        }
        // A different doc for the same term must still accept new positions.
        match b.add_token(DocId(1), "a", Position(0)) {
            Ok(()) => {}
            Err(e) => assert!(false, "different doc must not share cap: {e}"),
        }
    }

    fn serialize_bytes(b: PositionsBuilder) -> Vec<u8> {
        let idx = match b.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return Vec::new();
            }
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = idx.serialize_cbor(&mut buf) {
            assert!(false, "{e}");
        }
        buf
    }

    #[test]
    fn upsert_doc_replaces_prior_positions() {
        // First upsert seeds the doc with one tokenization; second upsert
        // with a different tokenization must result in only the second
        // set of positions being present (no merge).
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        let first: &[(&str, Position)] = &[
            ("foo", Position(0)),
            ("bar", Position(1)),
            ("foo", Position(2)),
        ];
        if let Err(e) = b.upsert_doc(DocId(42), first.iter().copied()) {
            assert!(false, "{e}");
            return;
        }
        let second: &[(&str, Position)] = &[("baz", Position(0)), ("foo", Position(5))];
        if let Err(e) = b.upsert_doc(DocId(42), second.iter().copied()) {
            assert!(false, "{e}");
            return;
        }
        let idx = match b.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // `bar` was only in the first upsert — must be gone entirely.
        assert!(
            idx.term_postings("bar").is_none(),
            "term 'bar' must be removed by upsert replacement"
        );
        // `baz` is new from the second upsert.
        let Some(iter) = idx.term_postings("baz") else {
            assert!(false, "term 'baz' missing");
            return;
        };
        let mut found_baz = false;
        for r in iter {
            match r {
                Ok(e) => {
                    assert_eq!(e.doc_id, DocId(42));
                    assert_eq!(e.positions, vec![Position(0)]);
                    found_baz = true;
                }
                Err(e) => {
                    assert!(false, "{e}");
                    return;
                }
            }
        }
        assert!(found_baz, "doc 42 must appear in 'baz' postings");
        // `foo` was in both upserts; only second-upsert position (5) survives.
        let Some(iter) = idx.term_postings("foo") else {
            assert!(false, "term 'foo' missing");
            return;
        };
        for r in iter {
            match r {
                Ok(e) => {
                    assert_eq!(e.doc_id, DocId(42));
                    assert_eq!(
                        e.positions,
                        vec![Position(5)],
                        "first-upsert positions must not persist"
                    );
                }
                Err(e) => {
                    assert!(false, "{e}");
                    return;
                }
            }
        }
    }

    #[test]
    fn upsert_doc_idempotent_byte_identical_finish() {
        // upsert_doc called once vs twice with the same pairs must yield
        // the same finish() bytes.
        let pairs: &[(&str, Position)] = &[
            ("alpha", Position(0)),
            ("beta", Position(1)),
            ("alpha", Position(3)),
            ("gamma", Position(2)),
        ];
        let mut once = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        if let Err(e) = once.upsert_doc(DocId(7), pairs.iter().copied()) {
            assert!(false, "{e}");
            return;
        }
        let mut twice = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        if let Err(e) = twice.upsert_doc(DocId(7), pairs.iter().copied()) {
            assert!(false, "{e}");
            return;
        }
        if let Err(e) = twice.upsert_doc(DocId(7), pairs.iter().copied()) {
            assert!(false, "{e}");
            return;
        }
        let b1 = serialize_bytes(once);
        let b2 = serialize_bytes(twice);
        assert_eq!(b1, b2, "upsert_doc must be idempotent");
    }

    #[test]
    fn upsert_doc_preserves_other_docs() {
        // Upserting doc A must not affect doc B's posting entries.
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        // Doc B via add_token.
        if let Err(e) = b.add_token(DocId(2), "shared", Position(0)) {
            assert!(false, "{e}");
            return;
        }
        if let Err(e) = b.add_token(DocId(2), "shared", Position(4)) {
            assert!(false, "{e}");
            return;
        }
        // Doc A via upsert.
        let pairs: &[(&str, Position)] = &[("shared", Position(1)), ("only_a", Position(2))];
        if let Err(e) = b.upsert_doc(DocId(1), pairs.iter().copied()) {
            assert!(false, "{e}");
            return;
        }
        // Re-upsert doc A with a different shape; doc B must stay intact.
        let pairs2: &[(&str, Position)] = &[("shared", Position(9))];
        if let Err(e) = b.upsert_doc(DocId(1), pairs2.iter().copied()) {
            assert!(false, "{e}");
            return;
        }
        let idx = match b.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // `only_a` removed by the second upsert.
        assert!(idx.term_postings("only_a").is_none());
        let Some(iter) = idx.term_postings("shared") else {
            assert!(false, "term 'shared' missing");
            return;
        };
        let mut by_doc: std::collections::BTreeMap<DocId, Vec<Position>> =
            std::collections::BTreeMap::new();
        for r in iter {
            match r {
                Ok(e) => {
                    drop(by_doc.insert(e.doc_id, e.positions));
                }
                Err(e) => {
                    assert!(false, "{e}");
                    return;
                }
            }
        }
        assert_eq!(by_doc.get(&DocId(1)), Some(&vec![Position(9)]));
        assert_eq!(
            by_doc.get(&DocId(2)),
            Some(&vec![Position(0), Position(4)]),
            "doc 2 must be untouched by upserts on doc 1"
        );
    }

    #[test]
    fn upsert_doc_enforces_positions_per_cell_cap() {
        // Building one upsert with > MAX_POSITIONS_PER_CELL entries for
        // the same (term, doc) must fail closed.
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        let mut pairs: Vec<(&str, Position)> = Vec::new();
        for i in 0..=MAX_POSITIONS_PER_CELL {
            pairs.push(("t", Position(i)));
        }
        match b.upsert_doc(DocId(0), pairs.iter().copied()) {
            Ok(()) => assert!(false, "expected PlanLimitExceeded"),
            Err(e) => {
                assert_eq!(e.code, PositionsErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::PositionsPerCell));
            }
        }
    }

    #[test]
    fn remove_doc_returns_false_on_noop() {
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        match b.remove_doc(DocId(42)) {
            Ok(v) => assert!(!v, "remove_doc on empty builder must return false"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn remove_doc_drops_target_doc_only() {
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        if let Err(e) = b.add_token(DocId(1), "a", Position(0)) {
            assert!(false, "{e}");
            return;
        }
        if let Err(e) = b.add_token(DocId(1), "b", Position(1)) {
            assert!(false, "{e}");
            return;
        }
        if let Err(e) = b.add_token(DocId(2), "a", Position(0)) {
            assert!(false, "{e}");
            return;
        }
        match b.remove_doc(DocId(1)) {
            Ok(true) => {}
            Ok(false) => assert!(false, "remove_doc must report removal"),
            Err(e) => assert!(false, "{e}"),
        }
        let idx = match b.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // Doc 1's terms gone; 'b' had only doc 1 so the term is gone too.
        assert!(idx.term_postings("b").is_none());
        let Some(iter) = idx.term_postings("a") else {
            assert!(false, "term 'a' missing");
            return;
        };
        for r in iter {
            match r {
                Ok(e) => assert_eq!(e.doc_id, DocId(2)),
                Err(e) => {
                    assert!(false, "{e}");
                    return;
                }
            }
        }
    }

    #[test]
    fn remove_doc_idempotent_second_call_returns_false() {
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        if let Err(e) = b.add_token(DocId(1), "a", Position(0)) {
            assert!(false, "{e}");
            return;
        }
        match b.remove_doc(DocId(1)) {
            Ok(true) => {}
            Ok(false) => assert!(false, "first remove_doc must report removal"),
            Err(e) => assert!(false, "{e}"),
        }
        match b.remove_doc(DocId(1)) {
            Ok(false) => {}
            Ok(true) => assert!(false, "second remove_doc must be no-op"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn from_prior_preserves_all_term_doc_positions() {
        // Build a prior, then import via from_prior and compare bytes.
        let mut prior_b = PositionsBuilder::new(1, NormalizerVersion::new(2, 3));
        let inserts: &[(DocId, &str, Position)] = &[
            (DocId(0), "alpha", Position(0)),
            (DocId(0), "alpha", Position(3)),
            (DocId(0), "beta", Position(1)),
            (DocId(2), "alpha", Position(7)),
            (DocId(5), "gamma", Position(0)),
        ];
        for (d, t, p) in inserts {
            if let Err(e) = prior_b.add_token(*d, t, *p) {
                assert!(false, "{e}");
                return;
            }
        }
        let prior = match prior_b.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // Import via from_prior with a new generation but same nv.
        let imported_b =
            match PositionsBuilder::from_prior(&prior, 99, NormalizerVersion::new(2, 3)) {
                Ok(v) => v,
                Err(e) => {
                    assert!(false, "{e}");
                    return;
                }
            };
        let imported = match imported_b.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(imported.generation(), 99);
        assert_eq!(imported.normalizer_version(), prior.normalizer_version());
        // Posting list bytes must match for every term.
        let prior_terms: Vec<String> = prior.terms().map(ToOwned::to_owned).collect();
        let imported_terms: Vec<String> = imported.terms().map(ToOwned::to_owned).collect();
        assert_eq!(prior_terms, imported_terms);
        for t in &prior_terms {
            assert_eq!(prior.raw_postings(t), imported.raw_postings(t));
        }
    }

    #[test]
    fn from_prior_rejects_normalizer_mismatch() {
        let prior_b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        let prior = match prior_b.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        match PositionsBuilder::from_prior(&prior, 2, NormalizerVersion::new(2, 0)) {
            Ok(_) => assert!(false, "expected NormalizerVersionMismatch"),
            Err(e) => assert_eq!(e.code, PositionsErrorCode::NormalizerVersionMismatch),
        }
    }

    #[test]
    fn from_prior_then_remove_drops_only_target() {
        // Stage a prior with two docs; from_prior; remove one; finish.
        let mut prior_b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        if let Err(e) = prior_b.add_token(DocId(10), "x", Position(0)) {
            assert!(false, "{e}");
            return;
        }
        if let Err(e) = prior_b.add_token(DocId(20), "x", Position(0)) {
            assert!(false, "{e}");
            return;
        }
        if let Err(e) = prior_b.add_token(DocId(20), "y", Position(1)) {
            assert!(false, "{e}");
            return;
        }
        let prior = match prior_b.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut new_b = match PositionsBuilder::from_prior(&prior, 2, NormalizerVersion::new(1, 0))
        {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        match new_b.remove_doc(DocId(10)) {
            Ok(true) => {}
            Ok(false) => assert!(false, "remove_doc must report removal"),
            Err(e) => assert!(false, "{e}"),
        }
        let idx = match new_b.finish() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // term `x`: only doc 20 left.
        let Some(iter) = idx.term_postings("x") else {
            assert!(false, "term 'x' missing");
            return;
        };
        let mut x_docs: Vec<DocId> = Vec::new();
        for r in iter {
            match r {
                Ok(e) => x_docs.push(e.doc_id),
                Err(e) => {
                    assert!(false, "{e}");
                    return;
                }
            }
        }
        assert_eq!(x_docs, vec![DocId(20)]);
        // term `y`: doc 20 still there.
        let Some(iter) = idx.term_postings("y") else {
            assert!(false, "term 'y' missing");
            return;
        };
        let mut y_docs: Vec<DocId> = Vec::new();
        for r in iter {
            match r {
                Ok(e) => y_docs.push(e.doc_id),
                Err(e) => {
                    assert!(false, "{e}");
                    return;
                }
            }
        }
        assert_eq!(y_docs, vec![DocId(20)]);
    }
}
