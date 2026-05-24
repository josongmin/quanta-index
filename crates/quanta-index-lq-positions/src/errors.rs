//! Typed errors for the LEX-03 phrase position index.
//!
//! Every failure path through [`crate::builder`], [`crate::index`],
//! [`crate::phrase_query`], [`crate::adjacency_query`], and
//! [`crate::varint`] surfaces a [`PositionsError`] carrying a closed
//! [`PositionsErrorCode`].
//!
//! Production paths never panic, never silently default, and never
//! synthesize empty success in place of a real failure. Cross-chunk phrase
//! semantics (an empty match set, not an error) are encoded by the public
//! API contract of [`crate::phrase_query::PhraseMatches`], not by this
//! error type.
//!
//! Resource-cap failures (LEX-03 §4.2) surface as
//! [`PositionsErrorCode::PlanLimitExceeded`] carrying a closed
//! [`LimitDimension`] tag on [`PositionsError::dimension`]. No silent
//! truncation, no heuristic skip — every exceeded cap is fail-closed.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of phrase / adjacency / persistence failures.
///
/// Adding a variant is a wire-format change; bump callers when the closed
/// set grows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PositionsErrorCode {
    /// Generation id on the manifest moved backward relative to the in-memory
    /// authority. Per RFC § Monotonicity rules.
    StateGenerationRegression,
    /// `meta.normalizer_version` on the loaded shard does not match the
    /// expected authority. The shard must be rebuilt under the new
    /// normalizer; see RFC § Atomicity contract.
    NormalizerVersionMismatch,
    /// CBOR decode failure when loading a `PositionsIndex`.
    IndexDeserialize,
    /// On-disk posting list bytes are malformed (truncated, gap underflow,
    /// length mismatch).
    IndexCorrupted,
    /// Caller supplied an empty term, a phrase token with no chars, or a
    /// term that violates the post-normalize contract.
    InvalidTerm,
    /// Adjacency window value is outside the configured floor / ceiling.
    WindowOutOfRange,
    /// A per-deployment resource cap from LEX-03 §4.2 was crossed. The
    /// specific cap is named by [`PositionsError::dimension`]; absence of a
    /// dimension on a `PlanLimitExceeded` error is itself a typed bug.
    PlanLimitExceeded,
}

impl PositionsErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::StateGenerationRegression => "STATE_GENERATION_REGRESSION",
            Self::NormalizerVersionMismatch => "NORMALIZER_VERSION_MISMATCH",
            Self::IndexDeserialize => "INDEX_DESERIALIZE",
            Self::IndexCorrupted => "INDEX_CORRUPTED",
            Self::InvalidTerm => "INVALID_TERM",
            Self::WindowOutOfRange => "WINDOW_OUT_OF_RANGE",
            Self::PlanLimitExceeded => "PLAN_LIMIT_EXCEEDED",
        }
    }

    /// Inverse of [`PositionsErrorCode::as_code_str`]. Returns `None` for
    /// any string not in the closed set.
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "STATE_GENERATION_REGRESSION" => Self::StateGenerationRegression,
            "NORMALIZER_VERSION_MISMATCH" => Self::NormalizerVersionMismatch,
            "INDEX_DESERIALIZE" => Self::IndexDeserialize,
            "INDEX_CORRUPTED" => Self::IndexCorrupted,
            "INVALID_TERM" => Self::InvalidTerm,
            "WINDOW_OUT_OF_RANGE" => Self::WindowOutOfRange,
            "PLAN_LIMIT_EXCEEDED" => Self::PlanLimitExceeded,
            _ => return None,
        };
        Some(v)
    }
}

/// Closed taxonomy of per-deployment cap dimensions from LEX-03 §4.2.
///
/// Carried alongside [`PositionsErrorCode::PlanLimitExceeded`] on
/// [`PositionsError::dimension`]. Adding a variant is a wire-format change;
/// bump callers when the closed set grows.
///
/// The `SCREAMING_SNAKE_CASE` wire form is the crate-internal contract.
/// The planner layer maps these to the spec-facing kebab-case strings
/// (`"phrase-length"`, `"adjacency-scan-docs"`, etc.) at the boundary —
/// see [LEX-03 §4.2](../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/LEX-03.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LimitDimension {
    /// `terms.len() > MAX_PHRASE_LEN` on `query_phrase`.
    PhraseLen,
    /// Adjacency candidate position-pair scan crossed
    /// `MAX_ADJACENCY_SCAN_DEPTH`.
    AdjacencyScanDepth,
    /// Builder observed more than `MAX_POSITIONS_PER_CELL` positions for a
    /// single `(term, doc)` cell.
    PositionsPerCell,
    /// Builder finalisation found more than `MAX_DOCS_PER_TERM` docs in a
    /// single term's posting list.
    DocsPerTerm,
}

impl LimitDimension {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::PhraseLen => "PHRASE_LEN",
            Self::AdjacencyScanDepth => "ADJACENCY_SCAN_DEPTH",
            Self::PositionsPerCell => "POSITIONS_PER_CELL",
            Self::DocsPerTerm => "DOCS_PER_TERM",
        }
    }

    /// Inverse of [`LimitDimension::as_code_str`]. Returns `None` for any
    /// string not in the closed set.
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "PHRASE_LEN" => Self::PhraseLen,
            "ADJACENCY_SCAN_DEPTH" => Self::AdjacencyScanDepth,
            "POSITIONS_PER_CELL" => Self::PositionsPerCell,
            "DOCS_PER_TERM" => Self::DocsPerTerm,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for LimitDimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for LimitDimension {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for LimitDimension {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LimitDimension;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LimitDimension SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LimitDimension, E> {
                LimitDimension::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<LimitDimension>"]))
            }
        }
        de.deserialize_str(V)
    }
}

impl fmt::Display for PositionsErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for PositionsErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for PositionsErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = PositionsErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("PositionsErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<PositionsErrorCode, E> {
                PositionsErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<PositionsErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete phrase / adjacency / persistence failure with engineering-facing
/// detail.
///
/// `detail` is a short, free-form engineering anchor; it is not parsed by
/// callers. Callers branch on `code` (and, for
/// [`PositionsErrorCode::PlanLimitExceeded`], on `dimension`).
///
/// Carrier shape choice: `dimension: Option<LimitDimension>` lives on the
/// existing struct rather than as a separate `PlanLimitExceededError` type.
/// Rationale:
/// - Callers already branch on `code`; adding a second carrier would force
///   every consumer to learn a new type for a 4-variant tag.
/// - `Option` makes it structurally explicit that non-`PlanLimitExceeded`
///   codes never carry a dimension; the constructors enforce the pairing.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PositionsError {
    pub code: PositionsErrorCode,
    pub detail: Box<str>,
    /// `Some(_)` iff `code == PlanLimitExceeded`. Other codes always set
    /// this to `None` via the [`PositionsError::new`] constructor.
    pub dimension: Option<LimitDimension>,
}

impl PositionsError {
    /// Constructor for non-`PlanLimitExceeded` codes. Sets `dimension` to
    /// `None`. Use [`PositionsError::plan_limit_exceeded`] for cap errors.
    #[must_use]
    pub fn new(code: PositionsErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            detail: detail.into(),
            dimension: None,
        }
    }

    /// Constructor for [`PositionsErrorCode::PlanLimitExceeded`] errors —
    /// pairs the code with its required [`LimitDimension`] tag.
    #[must_use]
    pub fn plan_limit_exceeded(dimension: LimitDimension, detail: impl Into<Box<str>>) -> Self {
        Self {
            code: PositionsErrorCode::PlanLimitExceeded,
            detail: detail.into(),
            dimension: Some(dimension),
        }
    }
}

impl fmt::Display for PositionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.dimension {
            Some(d) => write!(f, "{}[{d}]: {}", self.code, self.detail),
            None => write!(f, "{}: {}", self.code, self.detail),
        }
    }
}

impl core::error::Error for PositionsError {}

#[cfg(test)]
mod tests {
    use super::{LimitDimension, PositionsError, PositionsErrorCode};

    const ALL_CODES: &[PositionsErrorCode] = &[
        PositionsErrorCode::StateGenerationRegression,
        PositionsErrorCode::NormalizerVersionMismatch,
        PositionsErrorCode::IndexDeserialize,
        PositionsErrorCode::IndexCorrupted,
        PositionsErrorCode::InvalidTerm,
        PositionsErrorCode::WindowOutOfRange,
        PositionsErrorCode::PlanLimitExceeded,
    ];

    const ALL_DIMENSIONS: &[LimitDimension] = &[
        LimitDimension::PhraseLen,
        LimitDimension::AdjacencyScanDepth,
        LimitDimension::PositionsPerCell,
        LimitDimension::DocsPerTerm,
    ];

    #[test]
    fn code_strs_are_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for c in ALL_CODES {
            let s = c.as_code_str();
            assert!(!seen.contains(&s), "duplicate code str: {s}");
            seen.push(s);
        }
        assert_eq!(seen.len(), ALL_CODES.len());
    }

    #[test]
    fn code_strs_roundtrip_via_from_code_str() {
        for c in ALL_CODES {
            let s = c.as_code_str();
            let parsed = PositionsErrorCode::from_code_str(s);
            assert_eq!(parsed, Some(*c));
        }
    }

    #[test]
    fn from_code_str_rejects_unknown() {
        assert_eq!(PositionsErrorCode::from_code_str("NOT_A_CODE"), None);
        assert_eq!(PositionsErrorCode::from_code_str(""), None);
    }

    #[test]
    fn plan_limit_exceeded_wire_form() {
        assert_eq!(
            PositionsErrorCode::PlanLimitExceeded.as_code_str(),
            "PLAN_LIMIT_EXCEEDED"
        );
        assert_eq!(
            PositionsErrorCode::from_code_str("PLAN_LIMIT_EXCEEDED"),
            Some(PositionsErrorCode::PlanLimitExceeded)
        );
    }

    #[test]
    fn limit_dimension_strs_are_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for d in ALL_DIMENSIONS {
            let s = d.as_code_str();
            assert!(!seen.contains(&s), "duplicate dimension str: {s}");
            seen.push(s);
        }
        assert_eq!(seen.len(), ALL_DIMENSIONS.len());
    }

    #[test]
    fn limit_dimension_roundtrip_via_from_code_str() {
        for d in ALL_DIMENSIONS {
            let s = d.as_code_str();
            assert_eq!(LimitDimension::from_code_str(s), Some(*d));
        }
    }

    #[test]
    fn limit_dimension_rejects_unknown() {
        assert_eq!(LimitDimension::from_code_str("NOT_A_DIM"), None);
        assert_eq!(LimitDimension::from_code_str(""), None);
    }

    #[test]
    fn limit_dimension_serde_roundtrip_via_ciborium() {
        for d in ALL_DIMENSIONS {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(d, &mut buf);
            assert!(w.is_ok(), "serialize failed for {d:?}");
            let read: Result<LimitDimension, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *d),
                Err(e) => assert!(false, "deserialize failed for {d:?}: {e}"),
            }
        }
    }

    #[test]
    fn display_includes_code_and_detail() {
        let e = PositionsError::new(PositionsErrorCode::InvalidTerm, "empty term");
        let s = format!("{e}");
        assert!(s.contains("INVALID_TERM"));
        assert!(s.contains("empty term"));
    }

    #[test]
    fn plan_limit_exceeded_constructor_pairs_code_and_dimension() {
        let e = PositionsError::plan_limit_exceeded(
            LimitDimension::PhraseLen,
            "terms.len()=65 > MAX_PHRASE_LEN=64",
        );
        assert_eq!(e.code, PositionsErrorCode::PlanLimitExceeded);
        assert_eq!(e.dimension, Some(LimitDimension::PhraseLen));
        let s = format!("{e}");
        assert!(s.contains("PLAN_LIMIT_EXCEEDED"));
        assert!(s.contains("PHRASE_LEN"));
    }

    #[test]
    fn new_constructor_never_sets_dimension() {
        for c in ALL_CODES {
            // The `new` constructor is only legitimate for non-cap codes,
            // but it must always set `dimension = None` to keep the
            // invariant "Some(_) iff PlanLimitExceeded" enforced by
            // construction.
            let e = PositionsError::new(*c, "any");
            assert_eq!(e.dimension, None, "dimension must be None for {c:?}");
        }
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in ALL_CODES {
            let mut buf: Vec<u8> = Vec::new();
            let write = ciborium::ser::into_writer(c, &mut buf);
            assert!(write.is_ok(), "serialize failed for {c:?}");
            let read: Result<PositionsErrorCode, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *c),
                Err(e) => assert!(false, "deserialize failed for {c:?}: {e}"),
            }
        }
    }
}
