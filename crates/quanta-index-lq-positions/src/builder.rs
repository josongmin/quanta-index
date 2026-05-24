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

use crate::errors::PositionsError;
use crate::index::PositionsIndex;
use crate::types::{DocId, NormalizerVersion, Position};
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
    pub fn add_token(&mut self, doc_id: DocId, term: &str, pos: Position) {
        let key: Box<str> = Box::from(term);
        let entry = self.by_term.entry(key).or_default();
        let positions = entry.entry(doc_id).or_default();
        positions.push(pos);
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
    use crate::types::{DocId, NormalizerVersion, Position};

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
        b.add_token(DocId(0), "x", Position(0));
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
        a.add_token(DocId(0), "t", Position(5));
        a.add_token(DocId(0), "t", Position(1));
        a.add_token(DocId(0), "t", Position(3));
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        b.add_token(DocId(0), "t", Position(1));
        b.add_token(DocId(0), "t", Position(3));
        b.add_token(DocId(0), "t", Position(5));
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
}
