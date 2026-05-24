//! Immutable per-generation position index.
//!
//! Holds the `(term -> delta-encoded-varint-bytes)` map plus the
//! `(generation, normalizer_version)` stamp. Exposes term iteration via
//! [`TermPostings`] / [`TermPostingsEntry`], which decode the on-wire
//! delta-varint posting list back into `(DocId, Vec<Position>)` rows for
//! consumption by [`crate::phrase_query`] and [`crate::adjacency_query`].
//!
//! D18 — manual `impl serde::Serialize`/`Deserialize`.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use ciborium::{de::from_reader, ser::into_writer};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::errors::{PositionsError, PositionsErrorCode};
use crate::types::{DocId, NormalizerVersion, Position};
use crate::varint::decode_u32;

/// Immutable per-generation `(term -> delta-encoded-varint-bytes)` index.
///
/// Stamped with the originating `generation` plus the
/// [`NormalizerVersion`] that produced the input token stream. Open-time
/// version checks live on the lexical-adapter integration boundary; this
/// type only carries the stamp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionsIndex {
    generation: u64,
    normalizer_version: NormalizerVersion,
    by_term: BTreeMap<Box<str>, Vec<u8>>,
}

impl PositionsIndex {
    /// Construct from builder-produced raw posting-list bytes.
    #[must_use]
    pub fn from_raw(
        generation: u64,
        normalizer_version: NormalizerVersion,
        by_term: BTreeMap<Box<str>, Vec<u8>>,
    ) -> Self {
        Self {
            generation,
            normalizer_version,
            by_term,
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

    /// CBOR-canonical serialize.
    pub fn serialize_cbor<W: Write>(&self, w: &mut W) -> Result<(), PositionsError> {
        into_writer(self, w).map_err(|e| {
            PositionsError::new(
                PositionsErrorCode::IndexCorrupted,
                format!("cbor encode: {e}"),
            )
        })
    }

    /// CBOR-canonical deserialize.
    pub fn deserialize_cbor<R: Read>(r: R) -> Result<Self, PositionsError> {
        from_reader(r).map_err(|e| {
            PositionsError::new(
                PositionsErrorCode::IndexDeserialize,
                format!("cbor decode: {e}"),
            )
        })
    }

    /// Borrow the raw delta-varint posting list bytes for `term`, if present.
    ///
    /// Returns `None` for terms not in the index (no entries were added under
    /// that key). The returned slice is the canonical wire shape produced by
    /// [`crate::builder::PositionsBuilder::finish`]; consumers normally walk
    /// it via [`PositionsIndex::term_postings`] instead of decoding inline.
    #[must_use]
    pub fn raw_postings(&self, term: &str) -> Option<&[u8]> {
        self.by_term.get(term).map(Vec::as_slice)
    }

    /// Open a decoding iterator over the posting list of `term`.
    ///
    /// Returns `None` if `term` is not present in the index. The iterator
    /// yields one [`TermPostingsEntry`] per doc that contains the term;
    /// each entry carries the absolute `DocId` and ascending absolute
    /// [`Position`] values (the delta encoding is unwound for the caller).
    #[must_use]
    pub fn term_postings(&self, term: &str) -> Option<TermPostings<'_>> {
        let bytes = self.by_term.get(term)?.as_slice();
        Some(TermPostings::new(bytes))
    }
}

/// One decoded posting entry: a `(doc_id, positions)` pair.
///
/// Positions are absolute (delta encoding already unwound), ascending, and
/// dense (no gaps for filtered-out content — stopword filter is locked OFF).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermPostingsEntry {
    pub doc_id: DocId,
    pub positions: Vec<Position>,
}

/// Streaming iterator over a single term's posting list.
///
/// Constructed via [`PositionsIndex::term_postings`]. The iterator is
/// fail-closed: a malformed posting list surfaces
/// [`PositionsErrorCode::IndexCorrupted`] via the `Result` `Item` type. The
/// iterator stops yielding after the first error and does not advance past
/// the failure point.
pub struct TermPostings<'a> {
    bytes: &'a [u8],
    cursor: usize,
    docs_remaining: u32,
    prev_doc: u64,
    started: bool,
    errored: bool,
}

impl<'a> TermPostings<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            cursor: 0,
            docs_remaining: 0,
            prev_doc: 0,
            started: false,
            errored: false,
        }
    }

    fn read_varint(&mut self) -> Result<u32, PositionsError> {
        let rest = self.bytes.get(self.cursor..).ok_or_else(|| {
            PositionsError::new(
                PositionsErrorCode::IndexCorrupted,
                "posting cursor past end",
            )
        })?;
        let (v, n) = decode_u32(rest)?;
        self.cursor = self.cursor.saturating_add(n);
        Ok(v)
    }

    fn read_header(&mut self) -> Result<(), PositionsError> {
        let count = self.read_varint()?;
        self.docs_remaining = count;
        self.started = true;
        Ok(())
    }

    fn read_entry(&mut self) -> Result<TermPostingsEntry, PositionsError> {
        // Doc gap is absolute on the first doc, relative on every subsequent
        // doc — mirrors the builder's encoding contract.
        let gap = self.read_varint()?;
        let cur_doc = self.prev_doc.checked_add(u64::from(gap)).ok_or_else(|| {
            PositionsError::new(
                PositionsErrorCode::IndexCorrupted,
                "doc id overflow during posting decode",
            )
        })?;
        self.prev_doc = cur_doc;

        let pos_count = self.read_varint()?;
        let pos_count_usize = usize::try_from(pos_count).map_err(|_err| {
            PositionsError::new(
                PositionsErrorCode::IndexCorrupted,
                "position_count exceeds usize",
            )
        })?;
        let mut positions: Vec<Position> = Vec::with_capacity(pos_count_usize);
        let mut prev_pos: u32 = 0;
        let mut first_pos = true;
        let mut i: u32 = 0;
        while i < pos_count {
            let gap = self.read_varint()?;
            let cur_pos = if first_pos {
                first_pos = false;
                gap
            } else {
                prev_pos.checked_add(gap).ok_or_else(|| {
                    PositionsError::new(
                        PositionsErrorCode::IndexCorrupted,
                        "position overflow during posting decode",
                    )
                })?
            };
            positions.push(Position(cur_pos));
            prev_pos = cur_pos;
            i = i.saturating_add(1);
        }
        Ok(TermPostingsEntry {
            doc_id: DocId(cur_doc),
            positions,
        })
    }
}

impl Iterator for TermPostings<'_> {
    type Item = Result<TermPostingsEntry, PositionsError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.errored {
            return None;
        }
        if !self.started
            && let Err(e) = self.read_header()
        {
            self.errored = true;
            return Some(Err(e));
        }
        if self.docs_remaining == 0 {
            return None;
        }
        match self.read_entry() {
            Ok(entry) => {
                self.docs_remaining = self.docs_remaining.saturating_sub(1);
                Some(Ok(entry))
            }
            Err(e) => {
                self.errored = true;
                Some(Err(e))
            }
        }
    }
}

impl Serialize for PositionsIndex {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("PositionsIndex", 3)?;
        st.serialize_field("generation", &self.generation)?;
        st.serialize_field("normalizer_version", &self.normalizer_version)?;
        let kv: Vec<(&str, &Vec<u8>)> = self.by_term.iter().map(|(k, v)| (k.as_ref(), v)).collect();
        st.serialize_field("by_term", &kv)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for PositionsIndex {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = PositionsIndex;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("PositionsIndex { generation, normalizer_version, by_term }")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut generation_opt: Option<u64> = None;
                let mut nv: Option<NormalizerVersion> = None;
                let mut kv: Option<Vec<(String, Vec<u8>)>> = None;
                while let Some(k) = m.next_key::<String>()? {
                    match k.as_str() {
                        "generation" => generation_opt = Some(m.next_value()?),
                        "normalizer_version" => nv = Some(m.next_value()?),
                        "by_term" => kv = Some(m.next_value()?),
                        unknown => {
                            return Err(<A::Error as serde::de::Error>::unknown_field(
                                unknown,
                                &["generation", "normalizer_version", "by_term"],
                            ));
                        }
                    }
                }
                let generation = generation_opt
                    .ok_or_else(|| <A::Error as serde::de::Error>::missing_field("generation"))?;
                let normalizer_version = nv.ok_or_else(|| {
                    <A::Error as serde::de::Error>::missing_field("normalizer_version")
                })?;
                let kv =
                    kv.ok_or_else(|| <A::Error as serde::de::Error>::missing_field("by_term"))?;
                let mut by_term: BTreeMap<Box<str>, Vec<u8>> = BTreeMap::new();
                for (k, v) in kv {
                    drop(by_term.insert(Box::from(k), v));
                }
                Ok(PositionsIndex {
                    generation,
                    normalizer_version,
                    by_term,
                })
            }
        }
        d.deserialize_map(V)
    }
}

#[cfg(test)]
#[expect(
    clippy::unreachable,
    clippy::indexing_slicing,
    reason = "test fixtures use direct indexing for failure clarity"
)]
mod tests {
    use super::{PositionsIndex, TermPostingsEntry};
    use crate::builder::PositionsBuilder;
    use crate::types::{DocId, NormalizerVersion, Position};

    fn collect_postings(idx: &PositionsIndex, term: &str) -> Vec<TermPostingsEntry> {
        let Some(iter) = idx.term_postings(term) else {
            return Vec::new();
        };
        let mut out: Vec<TermPostingsEntry> = Vec::new();
        for r in iter {
            match r {
                Ok(e) => out.push(e),
                Err(e) => {
                    assert!(false, "decode failed: {e}");
                    return out;
                }
            }
        }
        out
    }

    fn build_simple() -> PositionsIndex {
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        let inserts: &[(DocId, &str, Position)] = &[
            (DocId(0), "alpha", Position(0)),
            (DocId(0), "alpha", Position(3)),
            (DocId(2), "alpha", Position(7)),
            (DocId(0), "beta", Position(1)),
            (DocId(2), "beta", Position(8)),
            (DocId(3), "gamma", Position(0)),
        ];
        for (d, t, p) in inserts {
            if let Err(e) = b.add_token(*d, t, *p) {
                assert!(false, "{e}");
                unreachable!()
            }
        }
        match b.finish() {
            Ok(idx) => idx,
            Err(e) => {
                assert!(false, "{e}");
                unreachable!()
            }
        }
    }

    #[test]
    fn term_postings_missing_term_returns_none() {
        let idx = build_simple();
        assert!(idx.term_postings("nope").is_none());
    }

    #[test]
    fn term_postings_decodes_single_doc_positions() {
        let idx = build_simple();
        let v = collect_postings(&idx, "alpha");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].doc_id, DocId(0));
        assert_eq!(v[0].positions, vec![Position(0), Position(3)]);
        assert_eq!(v[1].doc_id, DocId(2));
        assert_eq!(v[1].positions, vec![Position(7)]);
    }

    #[test]
    fn term_postings_decodes_multi_doc_ascending_order() {
        let idx = build_simple();
        let v = collect_postings(&idx, "beta");
        assert_eq!(v.len(), 2);
        assert!(v[0].doc_id < v[1].doc_id);
        assert_eq!(v[0].doc_id, DocId(0));
        assert_eq!(v[1].doc_id, DocId(2));
    }

    #[test]
    fn term_postings_single_position_term() {
        let idx = build_simple();
        let v = collect_postings(&idx, "gamma");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].doc_id, DocId(3));
        assert_eq!(v[0].positions, vec![Position(0)]);
    }

    #[test]
    fn term_postings_round_trips_via_builder() {
        // Insert non-sequential positions and high doc ids; the iterator
        // must reproduce the ascending absolute values.
        let mut b = PositionsBuilder::new(7, NormalizerVersion::new(2, 1));
        let inserts: &[(DocId, &str, Position)] = &[
            (DocId(100), "t", Position(50)),
            (DocId(100), "t", Position(10)),
            (DocId(100), "t", Position(30)),
            (DocId(1_000_000), "t", Position(0)),
        ];
        for (d, t, p) in inserts {
            if let Err(e) = b.add_token(*d, t, *p) {
                assert!(false, "{e}");
                return;
            }
        }
        let idx = match b.finish() {
            Ok(idx) => idx,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let v = collect_postings(&idx, "t");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].doc_id, DocId(100));
        assert_eq!(
            v[0].positions,
            vec![Position(10), Position(30), Position(50)]
        );
        assert_eq!(v[1].doc_id, DocId(1_000_000));
        assert_eq!(v[1].positions, vec![Position(0)]);
    }
}
