//! Canonical per-generation trigram index — serialized artifact.
//!
//! [`TrigramIndex`] captures the trigram-to-posting-list state pinned at
//! build time for a generation. It is the authoritative source consulted
//! at query time; LEX-02 forbids recomputing posting lists on the fly to
//! mask a missing authoritative artifact.
//!
//! Iteration order over the posting map is the lexical sort order of
//! 3-byte trigram keys (a `BTreeMap`), giving the CBOR encoding its
//! byte-identical-across-runs property.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use crate::errors::{LimitDimension, TrigramError, TrigramErrorCode};
use crate::types::{DocId, MAX_CANDIDATE_PRE_VERIFY, TRIGRAM_LEN, Trigram};

/// Authoritative per-generation trigram-index state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrigramIndex {
    generation: u64,
    by_trigram: BTreeMap<Trigram, Vec<DocId>>,
}

impl TrigramIndex {
    /// Build directly from materialised parts (used by
    /// [`crate::builder::TrigramIndexBuilder::finish`]).
    #[must_use]
    pub(crate) fn from_parts(generation: u64, by_trigram: BTreeMap<Trigram, Vec<DocId>>) -> Self {
        Self {
            generation,
            by_trigram,
        }
    }

    /// Generation id pinned at build time.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of distinct trigram keys in the dictionary.
    #[must_use]
    pub fn distinct_trigrams(&self) -> usize {
        self.by_trigram.len()
    }

    /// Borrow the posting list for `trigram`, or an empty slice if the
    /// trigram is absent.
    #[must_use]
    pub fn lookup(&self, trigram: Trigram) -> &[DocId] {
        match self.by_trigram.get(&trigram) {
            Some(v) => v.as_slice(),
            None => &[],
        }
    }

    /// AND-intersect the posting lists for `query_trigrams`.
    ///
    /// Returns the sorted, de-duplicated set of `DocId`s that appear in
    /// EVERY posting list. An empty `query_trigrams` slice yields an
    /// empty result (typed, not error). Caps:
    ///
    /// - `query_trigrams.len() > MAX_TRIGRAMS_PER_QUERY` →
    ///   [`TrigramErrorCode::PlanLimitExceeded`] with
    ///   [`LimitDimension::Trigrams`].
    /// - intermediate candidate set > [`MAX_CANDIDATE_PRE_VERIFY`] →
    ///   [`TrigramErrorCode::PlanLimitExceeded`] with
    ///   [`LimitDimension::CandidateSet`].
    pub fn intersect_trigrams(
        &self,
        query_trigrams: &[Trigram],
    ) -> Result<Vec<DocId>, TrigramError> {
        ensure_query_trigram_count(query_trigrams)?;
        if query_trigrams.is_empty() {
            return Ok(Vec::new());
        }

        // Seed with the shortest posting list so subsequent intersect
        // passes shrink monotonically. We collect a unique trigram set
        // first to keep duplicates from causing an empty-list seed.
        let mut uniq: Vec<Trigram> = query_trigrams.to_vec();
        uniq.sort_unstable();
        uniq.dedup();

        // Materialise each unique trigram's posting list.
        let mut lists: Vec<&[DocId]> = Vec::with_capacity(uniq.len());
        for t in &uniq {
            let lst = self.lookup(*t);
            if lst.is_empty() {
                // Any missing trigram → empty intersect, no candidate set.
                return Ok(Vec::new());
            }
            lists.push(lst);
        }
        lists.sort_by_key(|l| l.len());

        let Some((seed, rest)) = lists.split_first() else {
            return Ok(Vec::new());
        };
        let mut acc: Vec<DocId> = seed.to_vec();
        if acc.len() > MAX_CANDIDATE_PRE_VERIFY {
            return Err(TrigramError::plan_limit(
                LimitDimension::CandidateSet,
                format!(
                    "seed candidate set {} exceeds cap {}",
                    acc.len(),
                    MAX_CANDIDATE_PRE_VERIFY
                ),
            ));
        }

        for list in rest {
            acc = intersect_sorted(&acc, list);
            if acc.is_empty() {
                return Ok(acc);
            }
            if acc.len() > MAX_CANDIDATE_PRE_VERIFY {
                return Err(TrigramError::plan_limit(
                    LimitDimension::CandidateSet,
                    format!("candidate set {} exceeds cap {}", acc.len(), MAX_CANDIDATE_PRE_VERIFY),
                ));
            }
        }
        Ok(acc)
    }

    /// Iterate over the trigram dictionary in canonical (sorted) order.
    pub fn iter(&self) -> impl Iterator<Item = (Trigram, &[DocId])> {
        self.by_trigram.iter().map(|(k, v)| (*k, v.as_slice()))
    }

    /// Return every trigram that references `doc_id` in the index, in
    /// sorted order.
    ///
    /// Used by [`crate::builder::TrigramIndexBuilder::from_prior`] to
    /// reconstruct the reverse `(doc_id → trigrams)` map for cross-
    /// generation incremental builds. The current implementation scans
    /// every posting list (O(P log) where P is total postings); a future
    /// optimization may persist the reverse map directly on disk.
    #[must_use]
    pub fn iter_by_doc(&self, doc_id: DocId) -> Vec<Trigram> {
        let mut out: Vec<Trigram> = Vec::new();
        for (tri, postings) in &self.by_trigram {
            if postings.binary_search(&doc_id).is_ok() {
                out.push(*tri);
            }
        }
        out
    }

    /// Serialize as canonical CBOR.
    pub fn serialize_cbor<W: Write>(&self, writer: W) -> Result<(), TrigramError> {
        ciborium::ser::into_writer(self, writer).map_err(|e| {
            TrigramError::new(
                TrigramErrorCode::IndexDeserialize,
                format!("CBOR encode failed: {e}"),
            )
        })
    }

    /// Inverse of [`Self::serialize_cbor`].
    pub fn deserialize_cbor<R: Read>(reader: R) -> Result<Self, TrigramError> {
        ciborium::de::from_reader(reader).map_err(|e| {
            TrigramError::new(
                TrigramErrorCode::IndexDeserialize,
                format!("CBOR decode failed: {e}"),
            )
        })
    }
}

/// The per-query trigram cap, shared by every posting source so a sharded
/// union refuses exactly the queries one index refuses.
pub(crate) fn ensure_query_trigram_count(query_trigrams: &[Trigram]) -> Result<(), TrigramError> {
    if query_trigrams.len() > crate::types::MAX_TRIGRAMS_PER_QUERY {
        return Err(TrigramError::plan_limit(
            LimitDimension::Trigrams,
            format!(
                "query trigram count {} exceeds cap {}",
                query_trigrams.len(),
                crate::types::MAX_TRIGRAMS_PER_QUERY
            ),
        ));
    }
    Ok(())
}

/// Linear two-pointer intersect on already-sorted, dedup'd posting lists.
fn intersect_sorted(a: &[DocId], b: &[DocId]) -> Vec<DocId> {
    use core::cmp::Ordering;
    let mut out: Vec<DocId> = Vec::new();
    let mut i: usize = 0;
    let mut j: usize = 0;
    while let (Some(x), Some(y)) = (a.get(i), b.get(j)) {
        match x.cmp(y) {
            Ordering::Equal => {
                out.push(*x);
                i = i.saturating_add(1);
                j = j.saturating_add(1);
            }
            Ordering::Less => {
                i = i.saturating_add(1);
            }
            Ordering::Greater => {
                j = j.saturating_add(1);
            }
        }
    }
    out
}

impl serde::Serialize for TrigramIndex {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("generation", &self.generation)?;
        m.serialize_entry("by_trigram", &SortedTrigramMap(&self.by_trigram))?;
        m.end()
    }
}

struct SortedTrigramMap<'a>(&'a BTreeMap<Trigram, Vec<DocId>>);

impl serde::Serialize for SortedTrigramMap<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq as _;
        // Emit as a sequence of [trigram_bytes, [doc_ids]] pairs so the
        // CBOR encoding is canonical and trigram key bytes don't need to
        // be re-interpreted as map-key strings.
        let mut s = ser.serialize_seq(Some(self.0.len()))?;
        for (k, v) in self.0 {
            s.serialize_element(&TrigramEntry(*k, v))?;
        }
        s.end()
    }
}

struct TrigramEntry<'a>(Trigram, &'a Vec<DocId>);

impl serde::Serialize for TrigramEntry<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeTuple as _;
        let mut t = ser.serialize_tuple(2)?;
        // Serialize the trigram bytes as a 3-byte array.
        t.serialize_element(&TrigramKey(self.0))?;
        t.serialize_element(&PostingList(self.1))?;
        t.end()
    }
}

struct TrigramKey(Trigram);

impl serde::Serialize for TrigramKey {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_bytes(&self.0)
    }
}

struct PostingList<'a>(&'a Vec<DocId>);

impl serde::Serialize for PostingList<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq as _;
        let mut s = ser.serialize_seq(Some(self.0.len()))?;
        for d in self.0 {
            s.serialize_element(d)?;
        }
        s.end()
    }
}

impl<'de> serde::Deserialize<'de> for TrigramIndex {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = TrigramIndex;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("TrigramIndex map with fields generation, by_trigram")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<TrigramIndex, M::Error> {
                let mut generation: Option<u64> = None;
                let mut by_trigram: Option<BTreeMap<Trigram, Vec<DocId>>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "generation" => {
                            if generation.is_some() {
                                return Err(serde::de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                        "by_trigram" => {
                            if by_trigram.is_some() {
                                return Err(serde::de::Error::duplicate_field("by_trigram"));
                            }
                            let entries: Vec<TrigramEntryOwned> = map.next_value()?;
                            let mut out: BTreeMap<Trigram, Vec<DocId>> = BTreeMap::new();
                            for entry in entries {
                                let TrigramEntryOwned(k, v) = entry;
                                let prior = out.insert(k, v);
                                if prior.is_some() {
                                    return Err(serde::de::Error::custom(
                                        "duplicate trigram key in by_trigram",
                                    ));
                                }
                            }
                            by_trigram = Some(out);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &["generation", "by_trigram"],
                            ));
                        }
                    }
                }
                let g = generation.ok_or_else(|| serde::de::Error::missing_field("generation"))?;
                if g == 0 {
                    return Err(serde::de::Error::custom("generation must be non-zero"));
                }
                let m = by_trigram.ok_or_else(|| serde::de::Error::missing_field("by_trigram"))?;
                for postings in m.values() {
                    // Posting lists must be sorted and de-duplicated.
                    let mut prev: Option<DocId> = None;
                    for d in postings {
                        if let Some(p) = prev
                            && *d <= p
                        {
                            return Err(serde::de::Error::custom(
                                "posting list not strictly sorted/dedup'd",
                            ));
                        }
                        prev = Some(*d);
                    }
                }
                Ok(TrigramIndex {
                    generation: g,
                    by_trigram: m,
                })
            }
        }
        de.deserialize_map(V)
    }
}

struct TrigramEntryOwned(Trigram, Vec<DocId>);

impl<'de> serde::Deserialize<'de> for TrigramEntryOwned {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = TrigramEntryOwned;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("2-tuple of (trigram bytes, posting list)")
            }
            fn visit_seq<A: serde::de::SeqAccess<'d>>(
                self,
                mut seq: A,
            ) -> Result<TrigramEntryOwned, A::Error> {
                let bytes: serde_bytes_compat::Bytes = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                if bytes.0.len() != TRIGRAM_LEN {
                    return Err(serde::de::Error::custom("trigram key must be exactly 3 bytes"));
                }
                let mut tri: Trigram = [0u8; TRIGRAM_LEN];
                for (i, b) in bytes.0.iter().enumerate() {
                    let slot = tri
                        .get_mut(i)
                        .ok_or_else(|| serde::de::Error::custom("trigram index out of range"))?;
                    *slot = *b;
                }
                let postings: Vec<DocId> = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                Ok(TrigramEntryOwned(tri, postings))
            }
        }
        de.deserialize_tuple(2, V)
    }
}

/// Small inline byte-buffer deserialize helper. We avoid the
/// `serde_bytes` crate dependency by hand-rolling exactly the visit
/// methods we need.
mod serde_bytes_compat {
    pub(super) struct Bytes(pub Vec<u8>);

    impl<'de> serde::Deserialize<'de> for Bytes {
        fn deserialize<D>(de: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            struct V;
            impl<'d> serde::de::Visitor<'d> for V {
                type Value = Bytes;
                fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                    f.write_str("byte buffer")
                }
                fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Bytes, E> {
                    Ok(Bytes(v.to_vec()))
                }
                fn visit_borrowed_bytes<E: serde::de::Error>(
                    self,
                    v: &'d [u8],
                ) -> Result<Bytes, E> {
                    Ok(Bytes(v.to_vec()))
                }
                fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Bytes, E> {
                    Ok(Bytes(v))
                }
                fn visit_seq<A: serde::de::SeqAccess<'d>>(
                    self,
                    mut seq: A,
                ) -> Result<Bytes, A::Error> {
                    let mut out: Vec<u8> = Vec::new();
                    while let Some(b) = seq.next_element::<u8>()? {
                        out.push(b);
                    }
                    Ok(Bytes(out))
                }
            }
            de.deserialize_bytes(V)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TrigramIndex;
    use crate::builder::TrigramIndexBuilder;
    use crate::errors::{LimitDimension, TrigramErrorCode};
    use crate::types::{DocId, MAX_TRIGRAMS_PER_QUERY, Trigram};

    fn fatal(msg: &str) -> ! {
        assert!(false, "{msg}");
        std::process::abort();
    }

    fn fixture() -> TrigramIndex {
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        b.add_doc(DocId(1), b"abcdef");
        b.add_doc(DocId(2), b"xyzabc");
        b.add_doc(DocId(3), b"abcabc");
        b.finish()
    }

    #[test]
    fn lookup_returns_empty_for_missing_trigram() {
        let idx = fixture();
        let empty: Vec<DocId> = Vec::new();
        assert_eq!(idx.lookup(*b"QQQ").to_vec(), empty);
    }

    #[test]
    fn iter_by_doc_returns_only_that_docs_trigrams() {
        let idx = fixture();
        // doc 2's content is "xyzabc" → trigrams xyz, yza, zab, abc.
        let mut tris = idx.iter_by_doc(DocId(2));
        tris.sort_unstable();
        assert_eq!(tris, vec![*b"abc", *b"xyz", *b"yza", *b"zab"]);
    }

    #[test]
    fn iter_by_doc_returns_empty_for_unknown_doc() {
        let idx = fixture();
        let tris = idx.iter_by_doc(DocId(99));
        assert!(tris.is_empty());
    }

    #[test]
    fn lookup_returns_sorted_dedup_postings() {
        let idx = fixture();
        assert_eq!(idx.lookup(*b"abc"), &[DocId(1), DocId(2), DocId(3)]);
    }

    #[test]
    fn intersect_with_empty_input_is_empty() {
        let idx = fixture();
        let out = match idx.intersect_trigrams(&[]) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(out.is_empty());
    }

    #[test]
    fn intersect_one_trigram_returns_posting() {
        let idx = fixture();
        let out = match idx.intersect_trigrams(&[*b"abc"]) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(out, vec![DocId(1), DocId(2), DocId(3)]);
    }

    #[test]
    fn intersect_two_trigrams_narrows() {
        let idx = fixture();
        // bcd appears only in doc 1; abc appears in 1, 2, 3.
        let out = match idx.intersect_trigrams(&[*b"abc", *b"bcd"]) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(out, vec![DocId(1)]);
    }

    #[test]
    fn intersect_with_missing_trigram_is_empty() {
        let idx = fixture();
        let out = match idx.intersect_trigrams(&[*b"abc", *b"QQQ"]) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(out.is_empty());
    }

    #[test]
    fn intersect_rejects_too_many_trigrams() {
        let idx = fixture();
        let big: Vec<Trigram> = (0..MAX_TRIGRAMS_PER_QUERY.saturating_add(1))
            .map(|_| *b"abc")
            .collect();
        match idx.intersect_trigrams(&big) {
            Ok(_) => assert!(false, "expected PLAN_LIMIT_EXCEEDED"),
            Err(e) => {
                assert_eq!(e.code, TrigramErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::Trigrams));
            }
        }
    }

    #[test]
    fn cbor_roundtrip_preserves_value() {
        let idx = fixture();
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = idx.serialize_cbor(&mut buf) {
            fatal(&format!("{e}"));
        }
        match TrigramIndex::deserialize_cbor(buf.as_slice()) {
            Ok(got) => assert_eq!(got, idx),
            Err(e) => fatal(&format!("{e}")),
        }
    }

    #[test]
    fn cbor_encoding_is_byte_identical_across_runs() {
        let idx1 = fixture();
        let idx2 = fixture();
        let mut b1: Vec<u8> = Vec::new();
        let mut b2: Vec<u8> = Vec::new();
        if let Err(e) = idx1.serialize_cbor(&mut b1) {
            fatal(&format!("{e}"));
        }
        if let Err(e) = idx2.serialize_cbor(&mut b2) {
            fatal(&format!("{e}"));
        }
        assert_eq!(b1, b2);
    }

    #[test]
    fn deserialize_rejects_truncated_bytes() {
        let idx = fixture();
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = idx.serialize_cbor(&mut buf) {
            fatal(&format!("{e}"));
        }
        let last = buf.len().saturating_sub(3);
        let Some(t) = buf.get(..last) else {
            fatal("slice");
        };
        match TrigramIndex::deserialize_cbor(t) {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, TrigramErrorCode::IndexDeserialize),
        }
    }
}
