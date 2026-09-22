//! Phrase query: ordered contiguous-token match.
//!
//! Implements the `"…"` exact phrase leaf for the post-normalize position
//! shard. Algorithm:
//!
//! 1. Empty input slice → empty [`PhraseMatches`] (typed, not error).
//! 2. Single term → one 1-token [`PhraseMatch`] per `(doc, position)` pair.
//! 3. Multi-term — for the first term iterate its postings; for each doc
//!    that shares ids with the rest of the terms, walk positions and
//!    check that `t_{i+1}` occurs at `t_i + 1` in the same doc. Output a
//!    match per anchor position whose contiguous run is intact.
//!
//! Cross-chunk semantics are inherited from the index shape (chunk id =
//! doc id); a phrase that legitimately spans a chunk boundary yields an
//! empty result, never a typed error — see `dsl.md §3.2`.
//!
//! D18 — every public type is hand-rolled serde.

use core::fmt;
use std::collections::BTreeMap;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Deserializer, Serializer};

use crate::errors::{LimitDimension, PositionsError};
use crate::index::TermPostingsEntry;
use crate::source::TermPostingSource;
use crate::types::{DocId, MAX_PHRASE_LEN, Position};

/// One contiguous phrase hit inside a single doc.
///
/// `start_position` is the position of the first token in the phrase;
/// `end_position` is the position of the last token. For an N-token phrase
/// the invariant is `end_position == start_position + (N - 1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PhraseMatch {
    pub doc_id: DocId,
    pub start_position: Position,
    pub end_position: Position,
}

/// Result set of [`query_phrase`].
///
/// Empty `matches` is a valid, typed outcome — there is no separate
/// "not-found" error code. Cross-chunk and zero-input cases both produce
/// `PhraseMatches { matches: vec![] }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhraseMatches {
    pub matches: Vec<PhraseMatch>,
}

impl PhraseMatches {
    /// Empty constructor — the canonical no-hit outcome.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            matches: Vec::new(),
        }
    }
}

impl serde::Serialize for PhraseMatch {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("PhraseMatch", 3)?;
        st.serialize_field("doc_id", &self.doc_id)?;
        st.serialize_field("start_position", &self.start_position)?;
        st.serialize_field("end_position", &self.end_position)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for PhraseMatch {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'d> Visitor<'d> for V {
            type Value = PhraseMatch;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("PhraseMatch { doc_id, start_position, end_position }")
            }
            fn visit_map<M: MapAccess<'d>>(self, mut m: M) -> Result<PhraseMatch, M::Error> {
                let mut doc_id: Option<DocId> = None;
                let mut start: Option<Position> = None;
                let mut end: Option<Position> = None;
                while let Some(k) = m.next_key::<String>()? {
                    match k.as_str() {
                        "doc_id" => doc_id = Some(m.next_value()?),
                        "start_position" => start = Some(m.next_value()?),
                        "end_position" => end = Some(m.next_value()?),
                        other => {
                            return Err(de::Error::unknown_field(
                                other,
                                &["doc_id", "start_position", "end_position"],
                            ));
                        }
                    }
                }
                Ok(PhraseMatch {
                    doc_id: doc_id.ok_or_else(|| de::Error::missing_field("doc_id"))?,
                    start_position: start
                        .ok_or_else(|| de::Error::missing_field("start_position"))?,
                    end_position: end.ok_or_else(|| de::Error::missing_field("end_position"))?,
                })
            }
        }
        d.deserialize_map(V)
    }
}

impl serde::Serialize for PhraseMatches {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.matches.len()))?;
        for m in &self.matches {
            seq.serialize_element(m)?;
        }
        seq.end()
    }
}

impl<'de> serde::Deserialize<'de> for PhraseMatches {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'d> Visitor<'d> for V {
            type Value = PhraseMatches;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("PhraseMatches sequence")
            }
            fn visit_seq<A: SeqAccess<'d>>(self, mut seq: A) -> Result<PhraseMatches, A::Error> {
                let mut out: Vec<PhraseMatch> = Vec::new();
                while let Some(m) = seq.next_element::<PhraseMatch>()? {
                    out.push(m);
                }
                Ok(PhraseMatches { matches: out })
            }
        }
        d.deserialize_seq(V)
    }
}

/// Decode every posting for `term` into a `(DocId -> Vec<Position>)` map.
///
/// Used by [`query_phrase`] and [`crate::adjacency_query::query_adjacency`]
/// to materialize a doc-keyed view for join logic. Surfaces any decode
/// failure as a typed [`PositionsError`].
pub(crate) fn collect_term_postings<S: TermPostingSource + ?Sized>(
    idx: &S,
    term: &str,
) -> Result<BTreeMap<DocId, Vec<Position>>, PositionsError> {
    let mut out: BTreeMap<DocId, Vec<Position>> = BTreeMap::new();
    let Some(iter) = idx.term_postings(term) else {
        return Ok(out);
    };
    for r in iter {
        let entry: TermPostingsEntry = r?;
        drop(out.insert(entry.doc_id, entry.positions));
    }
    Ok(out)
}

/// Run a phrase query against `idx` for the ordered token sequence `terms`.
///
/// `idx` is any [`TermPostingSource`]: one [`crate::PositionsIndex`] or a
/// [`crate::ShardedPositionsIndex`] over a doc-id partition; the algorithm
/// is the same and so is the answer, in the same doc-then-anchor order.
///
/// Empty `terms` is a valid input that yields an empty result set. A single
/// term yields one match per posting. Multi-term phrases require a strict
/// `position + 1` chain in the same doc across all terms.
pub fn query_phrase<S: TermPostingSource + ?Sized>(
    idx: &S,
    terms: &[&str],
) -> Result<PhraseMatches, PositionsError> {
    // LEX-03 §4.2: phrase length cap. `terms.len()` is `usize`; when it
    // exceeds `u32`, it trivially exceeds `MAX_PHRASE_LEN` — surface that
    // as the same `PlanLimitExceeded { PhraseLen }` outcome rather than a
    // generic `IndexCorrupted`, because the failure mode is identical from
    // the caller's perspective (input shape too large to plan). No silent
    // default — both arms emit a typed error or fall through to the cap
    // comparison.
    match u32::try_from(terms.len()) {
        Ok(observed) => {
            if observed > MAX_PHRASE_LEN {
                return Err(PositionsError::plan_limit_exceeded(
                    LimitDimension::PhraseLen,
                    format!("phrase length {observed} exceeds cap={MAX_PHRASE_LEN}"),
                ));
            }
        }
        Err(_) => {
            return Err(PositionsError::plan_limit_exceeded(
                LimitDimension::PhraseLen,
                format!(
                    "phrase length {} exceeds u32 (cap={MAX_PHRASE_LEN})",
                    terms.len()
                ),
            ));
        }
    }

    if terms.is_empty() {
        return Ok(PhraseMatches::empty());
    }

    // Single-term path: one 1-token match per `(doc, position)`.
    let first_term = match terms.first() {
        Some(t) => *t,
        None => return Ok(PhraseMatches::empty()),
    };
    if terms.len() == 1 {
        let mut out: Vec<PhraseMatch> = Vec::new();
        let Some(iter) = idx.term_postings(first_term) else {
            return Ok(PhraseMatches::empty());
        };
        for r in iter {
            let entry: TermPostingsEntry = r?;
            for p in entry.positions {
                out.push(PhraseMatch {
                    doc_id: entry.doc_id,
                    start_position: p,
                    end_position: p,
                });
            }
        }
        return Ok(PhraseMatches { matches: out });
    }

    // Multi-term: collect all term postings into doc-keyed maps once,
    // then for each doc shared by every term, scan anchor positions of
    // the first term and check the contiguous chain in the rest.
    let first_map = collect_term_postings(idx, first_term)?;
    if first_map.is_empty() {
        return Ok(PhraseMatches::empty());
    }
    let mut rest_maps: Vec<BTreeMap<DocId, Vec<Position>>> =
        Vec::with_capacity(terms.len().saturating_sub(1));
    for t in terms.iter().skip(1) {
        let m = collect_term_postings(idx, t)?;
        if m.is_empty() {
            return Ok(PhraseMatches::empty());
        }
        rest_maps.push(m);
    }

    let Ok(phrase_len_u32) = u32::try_from(terms.len()) else {
        return Ok(PhraseMatches::empty());
    };
    let last_offset: u32 = phrase_len_u32.saturating_sub(1);

    let mut out: Vec<PhraseMatch> = Vec::new();
    for (doc_id, anchors) in &first_map {
        // Build per-doc position views for each follow-on term; if any term
        // is absent from this doc, the doc cannot contribute a match.
        let mut follow: Vec<&[Position]> = Vec::with_capacity(rest_maps.len());
        let mut all_present = true;
        for m in &rest_maps {
            if let Some(v) = m.get(doc_id) {
                follow.push(v.as_slice());
            } else {
                all_present = false;
                break;
            }
        }
        if !all_present {
            continue;
        }

        for anchor in anchors {
            let mut chain_ok = true;
            for (offset_minus_one, positions) in follow.iter().enumerate() {
                let Ok(off_raw) = u32::try_from(offset_minus_one) else {
                    chain_ok = false;
                    break;
                };
                let offset_u32 = off_raw.saturating_add(1);
                let Some(expected_raw) = anchor.0.checked_add(offset_u32) else {
                    chain_ok = false;
                    break;
                };
                let expected = Position(expected_raw);
                if positions.binary_search(&expected).is_err() {
                    chain_ok = false;
                    break;
                }
            }
            if chain_ok {
                let Some(end_raw) = anchor.0.checked_add(last_offset) else {
                    continue;
                };
                out.push(PhraseMatch {
                    doc_id: *doc_id,
                    start_position: *anchor,
                    end_position: Position(end_raw),
                });
            }
        }
    }

    Ok(PhraseMatches { matches: out })
}

#[cfg(test)]
#[expect(
    clippy::manual_let_else,
    clippy::unreachable,
    reason = "test fixtures preserve readable match expressions for failure clarity"
)]
mod tests {
    use super::{PhraseMatch, PhraseMatches, query_phrase};
    use crate::builder::PositionsBuilder;
    use crate::errors::{LimitDimension, PositionsErrorCode};
    use crate::index::PositionsIndex;
    use crate::types::{DocId, MAX_PHRASE_LEN, NormalizerVersion, Position};

    fn fixture() -> PositionsIndex {
        // Doc 0: "the quick brown fox"
        // Doc 1: "the lazy dog"
        // Doc 2: "quick brown the fox"   (phrase order broken)
        // Doc 3: "the quick brown fox jumps over the quick brown fox"
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));

        let docs: &[(DocId, &[&str])] = &[
            (DocId(0), &["the", "quick", "brown", "fox"]),
            (DocId(1), &["the", "lazy", "dog"]),
            (DocId(2), &["quick", "brown", "the", "fox"]),
            (
                DocId(3),
                &[
                    "the", "quick", "brown", "fox", "jumps", "over", "the", "quick", "brown", "fox",
                ],
            ),
        ];
        for (doc_id, tokens) in docs {
            for (pos, t) in tokens.iter().enumerate() {
                let p = match u32::try_from(pos) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Err(e) = b.add_token(*doc_id, t, Position(p)) {
                    assert!(false, "{e}");
                    unreachable!()
                }
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
    fn empty_terms_returns_empty_matches() {
        let idx = fixture();
        let r = match query_phrase(&idx, &[]) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert!(r.matches.is_empty());
    }

    #[test]
    fn single_term_returns_one_match_per_position() {
        let idx = fixture();
        let r = match query_phrase(&idx, &["the"]) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // doc 0: 1 hit at pos 0
        // doc 1: 1 hit at pos 0
        // doc 2: 1 hit at pos 2
        // doc 3: 2 hits at pos 0, 6
        assert_eq!(r.matches.len(), 5);
        for m in &r.matches {
            assert_eq!(m.start_position, m.end_position, "1-token match");
        }
    }

    #[test]
    fn two_term_phrase_matches_contiguous_pair() {
        let idx = fixture();
        let r = match query_phrase(&idx, &["the", "quick"]) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // doc 0: "the quick brown fox"   -> hit at 0
        // doc 1: "the lazy dog"          -> no hit
        // doc 2: "quick brown the fox"   -> no hit (the at 2, quick at 0)
        // doc 3: 2 hits at anchors 0 and 6
        let mut hits: Vec<(DocId, Position)> = r
            .matches
            .iter()
            .map(|m| (m.doc_id, m.start_position))
            .collect();
        hits.sort_unstable();
        assert_eq!(
            hits,
            vec![
                (DocId(0), Position(0)),
                (DocId(3), Position(0)),
                (DocId(3), Position(6)),
            ]
        );
        for m in &r.matches {
            assert_eq!(m.end_position.0, m.start_position.0.saturating_add(1));
        }
    }

    #[test]
    fn two_term_phrase_no_match_when_terms_not_contiguous() {
        let idx = fixture();
        let r = match query_phrase(&idx, &["the", "dog"]) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // doc 1: "the lazy dog" -> "the"@0, "dog"@2 — not contiguous; no
        // other doc contains "dog". Result must be empty.
        assert!(r.matches.is_empty());
    }

    #[test]
    fn three_term_phrase_match() {
        let idx = fixture();
        let r = match query_phrase(&idx, &["the", "quick", "brown"]) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // doc 0: anchor 0; doc 3: anchors 0, 6.
        let mut hits: Vec<(DocId, Position, Position)> = r
            .matches
            .iter()
            .map(|m| (m.doc_id, m.start_position, m.end_position))
            .collect();
        hits.sort_unstable();
        assert_eq!(
            hits,
            vec![
                (DocId(0), Position(0), Position(2)),
                (DocId(3), Position(0), Position(2)),
                (DocId(3), Position(6), Position(8)),
            ]
        );
    }

    #[test]
    fn four_term_phrase_match() {
        let idx = fixture();
        let r = match query_phrase(&idx, &["the", "quick", "brown", "fox"]) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut hits: Vec<(DocId, Position)> = r
            .matches
            .iter()
            .map(|m| (m.doc_id, m.start_position))
            .collect();
        hits.sort_unstable();
        assert_eq!(
            hits,
            vec![
                (DocId(0), Position(0)),
                (DocId(3), Position(0)),
                (DocId(3), Position(6)),
            ]
        );
        for m in &r.matches {
            assert_eq!(m.end_position.0, m.start_position.0.saturating_add(3));
        }
    }

    #[test]
    fn missing_term_yields_empty_matches() {
        let idx = fixture();
        let r = match query_phrase(&idx, &["the", "absent_word"]) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert!(r.matches.is_empty());
    }

    #[test]
    fn phrase_match_serde_roundtrip_via_ciborium() {
        let pm = PhraseMatch {
            doc_id: DocId(11),
            start_position: Position(2),
            end_position: Position(4),
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&pm, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<PhraseMatch, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, pm),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn phrase_length_cap_fails_closed_at_one_over_cap() {
        // LEX-03 §4.2: phrase length > MAX_PHRASE_LEN → PlanLimitExceeded
        // with dimension=PhraseLen. The fixture index is irrelevant — the
        // cap check runs before any postings are consulted.
        let idx = fixture();
        // Build a vector of (cap+1) cheap, valid terms.
        let Ok(n_usize) = usize::try_from(MAX_PHRASE_LEN.saturating_add(1)) else {
            assert!(false, "cap+1 fits in usize");
            return;
        };
        let terms: Vec<&str> = vec!["the"; n_usize];
        match query_phrase(&idx, &terms) {
            Ok(_) => assert!(false, "expected PlanLimitExceeded"),
            Err(e) => {
                assert_eq!(e.code, PositionsErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::PhraseLen));
            }
        }
    }

    #[test]
    fn phrase_length_at_cap_is_accepted() {
        // Boundary: `terms.len() == MAX_PHRASE_LEN` is the largest accepted
        // input. Result set is irrelevant; we only assert no cap error.
        let idx = fixture();
        let Ok(n_usize) = usize::try_from(MAX_PHRASE_LEN) else {
            assert!(false, "cap fits in usize");
            return;
        };
        let terms: Vec<&str> = vec!["absent_token"; n_usize];
        match query_phrase(&idx, &terms) {
            Ok(r) => assert!(r.matches.is_empty()),
            Err(e) => {
                assert_ne!(
                    e.code,
                    PositionsErrorCode::PlanLimitExceeded,
                    "must not trigger cap at exactly MAX_PHRASE_LEN"
                );
                assert!(false, "{e}");
            }
        }
    }

    #[test]
    fn phrase_matches_serde_roundtrip_via_ciborium() {
        let pms = PhraseMatches {
            matches: vec![
                PhraseMatch {
                    doc_id: DocId(0),
                    start_position: Position(0),
                    end_position: Position(0),
                },
                PhraseMatch {
                    doc_id: DocId(1),
                    start_position: Position(5),
                    end_position: Position(7),
                },
            ],
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&pms, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<PhraseMatches, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, pms),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
