//! Stable types shared between [`crate::builder`], [`crate::index`],
//! [`crate::query`], and [`crate::regex_prefilter`].
//!
//! The [`DocId`] newtype is intentionally a thin wrapper around `u64`
//! so sibling LEX-03 work (positions index) can re-use the same id
//! without re-implementing a parallel type.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Width of a byte trigram. Locked at 3 for LEX-02.
pub const TRIGRAM_LEN: usize = 3;

/// Per-query trigram set cap before the index returns
/// [`crate::TrigramErrorCode::PlanLimitExceeded`] with dimension
/// [`crate::LimitDimension::Trigrams`].
pub const MAX_TRIGRAMS_PER_QUERY: usize = 4_096;

/// Pre-verify candidate set cap before the index returns
/// [`crate::TrigramErrorCode::PlanLimitExceeded`] with dimension
/// [`crate::LimitDimension::CandidateSet`].
pub const MAX_CANDIDATE_PRE_VERIFY: usize = 100_000;

/// A 3-byte trigram. Byte trigrams (not code-point trigrams) per
/// [`crate`] module doc.
pub type Trigram = [u8; TRIGRAM_LEN];

/// Stable document identifier shared with sibling LEX-03 indices.
///
/// Newtype around `u64`; equality and ordering match the wrapped value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(pub u64);

impl DocId {
    /// Construct a new [`DocId`] from a raw `u64`.
    #[must_use]
    pub const fn new(v: u64) -> Self {
        Self(v)
    }

    /// Borrow the wrapped `u64`.
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

impl fmt::Display for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("DocId unsigned 64-bit integer")
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

/// Iterator that emits every byte trigram in `input` in order.
///
/// Inputs shorter than [`TRIGRAM_LEN`] yield no trigrams; this is the
/// short-input fast-path the planner uses to route through the verify-only
/// surface.
#[must_use]
pub fn trigrams_of(input: &[u8]) -> TrigramIter<'_> {
    TrigramIter { input, pos: 0 }
}

/// Concrete iterator returned by [`trigrams_of`].
pub struct TrigramIter<'a> {
    input: &'a [u8],
    pos: usize,
}

impl Iterator for TrigramIter<'_> {
    type Item = Trigram;

    fn next(&mut self) -> Option<Self::Item> {
        let end = self.pos.saturating_add(TRIGRAM_LEN);
        if end > self.input.len() {
            return None;
        }
        let window = self.input.get(self.pos..end)?;
        let mut out: Trigram = [0u8; TRIGRAM_LEN];
        for (i, b) in window.iter().enumerate() {
            let slot = out.get_mut(i)?;
            *slot = *b;
        }
        self.pos = self.pos.saturating_add(1);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::{DocId, TRIGRAM_LEN, Trigram, trigrams_of};

    #[test]
    fn docid_roundtrip_u64() {
        let d = DocId::from(42u64);
        assert_eq!(d.get(), 42);
        let v: u64 = d.into();
        assert_eq!(v, 42);
    }

    #[test]
    fn docid_display_matches_inner() {
        assert_eq!(format!("{}", DocId(7)), "7");
    }

    #[test]
    fn docid_serde_roundtrip() {
        let d = DocId(99);
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&d, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<DocId, _> = ciborium::de::from_reader(buf.as_slice());
        if let Ok(v) = got {
            assert_eq!(v, d);
        } else if let Err(e) = got {
            assert!(false, "{e}");
        }
    }

    #[test]
    fn trigrams_of_empty_input() {
        assert!(trigrams_of(b"").next().is_none());
    }

    #[test]
    fn trigrams_of_below_n() {
        assert!(trigrams_of(b"ab").next().is_none());
    }

    #[test]
    fn trigrams_of_exact_n() {
        let v: Vec<Trigram> = trigrams_of(b"abc").collect();
        assert_eq!(v, vec![*b"abc"]);
    }

    #[test]
    fn trigrams_of_sliding_window() {
        let v: Vec<Trigram> = trigrams_of(b"abcd").collect();
        assert_eq!(v, vec![*b"abc", *b"bcd"]);
    }

    #[test]
    fn trigrams_of_six_bytes() {
        let v: Vec<Trigram> = trigrams_of(b"abcdef").collect();
        assert_eq!(v.len(), 4);
        assert_eq!(v.first().copied(), Some(*b"abc"));
        assert_eq!(v.get(1).copied(), Some(*b"bcd"));
        assert_eq!(v.get(2).copied(), Some(*b"cde"));
        assert_eq!(v.get(3).copied(), Some(*b"def"));
    }

    #[test]
    fn trigram_len_locked_at_3() {
        assert_eq!(TRIGRAM_LEN, 3);
    }
}
