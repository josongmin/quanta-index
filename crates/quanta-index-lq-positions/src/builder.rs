//! Position-index builder.
//!
//! Accumulates `(term, doc_id, position)` triples in a deterministic
//! `BTreeMap` and finalises into a [`PositionsIndex`] whose per-term
//! posting list is delta-encoded varint bytes.
//!
//! Determinism: `BTreeMap` iteration order is byte-lex on the term key and
//! ascending on `DocId`, so the same insertion multiset emits a
//! byte-identical posting-list payload regardless of insertion order.

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
}
