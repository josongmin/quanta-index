//! Core newtype + config types for the LEX-03 phrase position index.
//!
//! Mirror the shape that the LEX-02 trigram crate publishes for `DocId` so a
//! later lexical-adapter integration ticket can unify the two without a wire
//! break. There is no inter-crate dep this wave — the two crates ship the
//! same `(pub u64)` shape independently.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Default adjacency window expressed in normalized tokens.
///
/// Authoritative reference: `dsl.md §5.3` ("Default window: 8 tokens").
pub const DEFAULT_WINDOW_TOKENS: u32 = 8;

/// Hard cap on adjacency window width.
///
/// Beyond this, the adjacency query returns
/// [`crate::errors::PositionsErrorCode::WindowOutOfRange`].
pub const MAX_WINDOW_TOKENS: u32 = 1024;

/// Maximum phrase length (in normalized tokens) per LEX-03 §4.2.
///
/// `query_phrase` with `terms.len() > MAX_PHRASE_LEN` fails closed with
/// [`crate::errors::PositionsErrorCode::PlanLimitExceeded`] carrying
/// [`crate::errors::LimitDimension::PhraseLen`].
pub const MAX_PHRASE_LEN: u32 = 64;

/// Maximum candidate-pair scan budget for `query_adjacency` per LEX-03
/// §4.2 (default `100_000`; the spec table also documents the floor
/// `1_000` and the ceiling `1_000_000`).
///
/// Exceeding this scan depth — measured as the running count of
/// position-pair comparisons across all docs that hold both terms — fails
/// closed with [`crate::errors::PositionsErrorCode::PlanLimitExceeded`]
/// carrying [`crate::errors::LimitDimension::AdjacencyScanDepth`].
pub const MAX_ADJACENCY_SCAN_DEPTH: u32 = 100_000;

/// Maximum positions retained for a single `(term, doc)` cell per LEX-03
/// §4.2.
///
/// `PositionsBuilder::add_token` increments a per-`(term, doc)` counter
/// and returns [`crate::errors::PositionsErrorCode::PlanLimitExceeded`]
/// with [`crate::errors::LimitDimension::PositionsPerCell`] when the
/// counter would exceed this value. No silent truncation.
pub const MAX_POSITIONS_PER_CELL: u32 = 4_096;

/// Maximum distinct docs in a single term's posting list per LEX-03 §4.2:
/// `2^25 = 33_554_432`.
///
/// `PositionsBuilder::finish` walks per-term doc lists; a list whose
/// length exceeds this cap fails closed with
/// [`crate::errors::PositionsErrorCode::PlanLimitExceeded`] and
/// [`crate::errors::LimitDimension::DocsPerTerm`].
pub const MAX_DOCS_PER_TERM: u32 = 1 << 25;

/// Per-generation document identifier.
///
/// Newtype around `u64`. Shape (`pub u64`) is deliberately identical to the
/// `DocId` that LEX-02 (`quanta-index-lq-trigram`) ships in the same wave;
/// the lexical-adapter integration ticket will unify the two into a single
/// canonical newtype without a wire break.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocId(pub u64);

impl From<u64> for DocId {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl fmt::Display for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DocId({})", self.0)
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
                f.write_str("u64 DocId")
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<DocId, E> {
                Ok(DocId(v))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<DocId, E> {
                if v < 0 {
                    return Err(E::custom("DocId must be non-negative"));
                }
                let u = u64::try_from(v).map_err(E::custom)?;
                Ok(DocId(u))
            }
        }
        de.deserialize_u64(V)
    }
}

/// Normalized-token offset within a single document.
///
/// Position 0 is the first token in the post-normalize stream for the doc.
/// Positions are dense (no gaps for filtered-out content) because the
/// stopword filter is locked OFF — see crate-level docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Position(pub u32);

impl From<u32> for Position {
    fn from(v: u32) -> Self {
        Self(v)
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Position({})", self.0)
    }
}

impl serde::Serialize for Position {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_u32(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for Position {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = Position;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("u32 Position")
            }
            fn visit_u32<E: serde::de::Error>(self, v: u32) -> Result<Position, E> {
                Ok(Position(v))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Position, E> {
                let v = u32::try_from(v).map_err(E::custom)?;
                Ok(Position(v))
            }
        }
        de.deserialize_u32(V)
    }
}

/// Adjacency query configuration.
///
/// `window_tokens` is the maximum allowed token gap between the two
/// adjacency terms; default = [`DEFAULT_WINDOW_TOKENS`], hard ceiling =
/// [`MAX_WINDOW_TOKENS`]. The match is symmetric (term `b` may appear
/// before or after term `a`); see [`crate::adjacency_query::query_adjacency`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AdjacencyConfig {
    pub window_tokens: u32,
}

impl AdjacencyConfig {
    /// Construct with the default window (`DEFAULT_WINDOW_TOKENS`).
    #[must_use]
    pub const fn default_window() -> Self {
        Self {
            window_tokens: DEFAULT_WINDOW_TOKENS,
        }
    }

    /// Construct with an explicit window in tokens.
    ///
    /// Returns `None` if the window exceeds [`MAX_WINDOW_TOKENS`] or is `0`.
    /// `0`-width adjacency is rejected because that degenerates to exact
    /// `==` position equality, which is meaningless for two distinct terms.
    #[must_use]
    pub const fn new(window_tokens: u32) -> Option<Self> {
        if window_tokens == 0 || window_tokens > MAX_WINDOW_TOKENS {
            return None;
        }
        Some(Self { window_tokens })
    }
}

impl Default for AdjacencyConfig {
    fn default() -> Self {
        Self::default_window()
    }
}

/// LEX-00 normalizer-version stamp.
///
/// Embedded in the shard manifest so a reader can fail closed with
/// [`crate::errors::PositionsErrorCode::NormalizerVersionMismatch`] when the
/// shard was built under a different normalizer. Major-version bumps are
/// breaking; minor bumps are forward-compatible reads only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NormalizerVersion {
    pub major: u16,
    pub minor: u16,
}

impl NormalizerVersion {
    #[must_use]
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for NormalizerVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl serde::Serialize for NormalizerVersion {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("major", &self.major)?;
        m.serialize_entry("minor", &self.minor)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for NormalizerVersion {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = NormalizerVersion;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("NormalizerVersion map with fields major, minor")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<NormalizerVersion, M::Error> {
                let mut major: Option<u16> = None;
                let mut minor: Option<u16> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "major" => {
                            if major.is_some() {
                                return Err(serde::de::Error::duplicate_field("major"));
                            }
                            major = Some(map.next_value()?);
                        }
                        "minor" => {
                            if minor.is_some() {
                                return Err(serde::de::Error::duplicate_field("minor"));
                            }
                            minor = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["major", "minor"],
                            ));
                        }
                    }
                }
                let major = major.ok_or_else(|| serde::de::Error::missing_field("major"))?;
                let minor = minor.ok_or_else(|| serde::de::Error::missing_field("minor"))?;
                Ok(NormalizerVersion { major, minor })
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdjacencyConfig, DEFAULT_WINDOW_TOKENS, DocId, MAX_WINDOW_TOKENS, NormalizerVersion,
        Position,
    };

    #[test]
    fn doc_id_from_u64_roundtrip() {
        let d: DocId = 7u64.into();
        assert_eq!(d, DocId(7));
    }

    #[test]
    fn doc_id_display_includes_value() {
        let s = format!("{}", DocId(42));
        assert!(s.contains("42"));
    }

    #[test]
    fn doc_id_serde_roundtrip_via_ciborium() {
        for v in [0u64, 1, 1_000, 0x7fff_ffff_ffff_ffffu64, u64::MAX] {
            let d = DocId(v);
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(&d, &mut buf);
            assert!(w.is_ok());
            let read: Result<DocId, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, d),
                Err(e) => assert!(false, "{e}"),
            }
        }
    }

    #[test]
    fn position_serde_roundtrip_via_ciborium() {
        for v in [0u32, 1, 1024, u32::MAX] {
            let p = Position(v);
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(&p, &mut buf);
            assert!(w.is_ok());
            let read: Result<Position, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, p),
                Err(e) => assert!(false, "{e}"),
            }
        }
    }

    #[test]
    fn adjacency_config_default_uses_8() {
        assert_eq!(
            AdjacencyConfig::default().window_tokens,
            DEFAULT_WINDOW_TOKENS
        );
        assert_eq!(DEFAULT_WINDOW_TOKENS, 8);
    }

    #[test]
    fn adjacency_config_new_rejects_zero_and_overflow() {
        assert!(AdjacencyConfig::new(0).is_none());
        assert!(AdjacencyConfig::new(1).is_some());
        assert!(AdjacencyConfig::new(MAX_WINDOW_TOKENS).is_some());
        assert!(AdjacencyConfig::new(MAX_WINDOW_TOKENS.saturating_add(1)).is_none());
    }

    #[test]
    fn plan_limit_constants_match_spec() {
        // LEX-03 §4.2 default-knob row: pinned so a casual edit can't drift
        // the wire-visible caps. Floors/ceilings are not enforced by this
        // crate (planner concern) so only the defaults are pinned here.
        assert_eq!(super::MAX_PHRASE_LEN, 64);
        assert_eq!(super::MAX_ADJACENCY_SCAN_DEPTH, 100_000);
        assert_eq!(super::MAX_POSITIONS_PER_CELL, 4_096);
        assert_eq!(super::MAX_DOCS_PER_TERM, 33_554_432);
        assert_eq!(super::MAX_DOCS_PER_TERM, 1u32 << 25);
    }

    #[test]
    fn normalizer_version_display_is_dotted() {
        let s = format!("{}", NormalizerVersion::new(1, 2));
        assert_eq!(s, "1.2");
    }

    #[test]
    fn normalizer_version_serde_roundtrip_via_ciborium() {
        let v = NormalizerVersion::new(3, 14);
        let mut buf: Vec<u8> = Vec::new();
        let w = ciborium::ser::into_writer(&v, &mut buf);
        assert!(w.is_ok());
        let r: Result<NormalizerVersion, _> = ciborium::de::from_reader(buf.as_slice());
        match r {
            Ok(got) => assert_eq!(got, v),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
