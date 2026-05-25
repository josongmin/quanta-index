//! Typed errors for the LEX-07 history engine.
//!
//! Every failure across the commit graph, parent walk, revisions enumerator,
//! tag resolver, write-packet trace, and manifest ledger maps to exactly one
//! [`HistoryErrorCode`] variant. No silent fallback; no silent default.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of history-engine failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HistoryErrorCode {
    /// A named ref (branch, tag, sha) did not resolve in the commit graph.
    HistoryRefNotFound,
    /// A revision range exceeded the configured capacity.
    HistoryRangeOverrun,
    /// A walk over the commit DAG observed a cycle.
    HistoryMergeCycle,
    /// `since.time:` was requested against a generation whose trace lacks
    /// an entry for the requested time anchor.
    HistoryTraceIncomplete,
    /// `type:commit` / `type:diff` was requested against a `(repo, rev)`
    /// whose history shards are absent.
    HistoryUnindexed,
    /// Generic plan-time cap exceeded. Always paired with a
    /// [`LimitDimension`] tag.
    PlanLimitExceeded,
    /// A manifest activation attempted to regress the current generation.
    StateGenerationRegression,
    /// A constructor was called with a generation id of `0`.
    InvalidGeneration,
    /// A commit insert referenced a parent SHA that was not present in
    /// the commit graph. Surfaces in strict (non-buffered) upsert mode.
    HistoryCommitParentUnknown,
    /// An in-graph `remove_commit` was attempted on a commit that still
    /// has children referencing it as parent. The conservative policy
    /// refuses the removal rather than orphan-rooting the children.
    HistoryCommitHasChildren,
}

impl HistoryErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::HistoryRefNotFound => "HISTORY_REF_NOT_FOUND",
            Self::HistoryRangeOverrun => "HISTORY_RANGE_OVERRUN",
            Self::HistoryMergeCycle => "HISTORY_MERGE_CYCLE",
            Self::HistoryTraceIncomplete => "HISTORY_TRACE_INCOMPLETE",
            Self::HistoryUnindexed => "HISTORY_UNINDEXED",
            Self::PlanLimitExceeded => "PLAN_LIMIT_EXCEEDED",
            Self::StateGenerationRegression => "STATE_GENERATION_REGRESSION",
            Self::InvalidGeneration => "INVALID_GENERATION",
            Self::HistoryCommitParentUnknown => "HISTORY_COMMIT_PARENT_UNKNOWN",
            Self::HistoryCommitHasChildren => "HISTORY_COMMIT_HAS_CHILDREN",
        }
    }

    /// Inverse of [`HistoryErrorCode::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "HISTORY_REF_NOT_FOUND" => Self::HistoryRefNotFound,
            "HISTORY_RANGE_OVERRUN" => Self::HistoryRangeOverrun,
            "HISTORY_MERGE_CYCLE" => Self::HistoryMergeCycle,
            "HISTORY_TRACE_INCOMPLETE" => Self::HistoryTraceIncomplete,
            "HISTORY_UNINDEXED" => Self::HistoryUnindexed,
            "PLAN_LIMIT_EXCEEDED" => Self::PlanLimitExceeded,
            "STATE_GENERATION_REGRESSION" => Self::StateGenerationRegression,
            "INVALID_GENERATION" => Self::InvalidGeneration,
            "HISTORY_COMMIT_PARENT_UNKNOWN" => Self::HistoryCommitParentUnknown,
            "HISTORY_COMMIT_HAS_CHILDREN" => Self::HistoryCommitHasChildren,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for HistoryErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for HistoryErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for HistoryErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = HistoryErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("HistoryErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<HistoryErrorCode, E> {
                HistoryErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<HistoryErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Closed taxonomy of cap-dimension tags carried by
/// [`HistoryErrorCode::PlanLimitExceeded`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LimitDimension {
    /// `parent:` walk depth exceeded the cap.
    ParentDepth,
    /// `revisions:<range>` enumeration exceeded the cap.
    RevisionsCount,
    /// Tag pattern complexity exceeded the regex NFA cap.
    TagPattern,
}

impl LimitDimension {
    /// `kebab-case` wire representation matching the DSL `dimension=` tag.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::ParentDepth => "parent-depth",
            Self::RevisionsCount => "history-revisions",
            Self::TagPattern => "tag-pattern",
        }
    }

    /// Inverse of [`LimitDimension::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "parent-depth" => Self::ParentDepth,
            "history-revisions" => Self::RevisionsCount,
            "tag-pattern" => Self::TagPattern,
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
                f.write_str("LimitDimension kebab-case string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LimitDimension, E> {
                LimitDimension::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<LimitDimension>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete history-engine failure.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HistoryError {
    pub code: HistoryErrorCode,
    pub dimension: Option<LimitDimension>,
    pub detail: Box<str>,
}

impl HistoryError {
    /// Build a non-cap typed failure.
    #[must_use]
    pub fn new(code: HistoryErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            dimension: None,
            detail: detail.into(),
        }
    }

    /// Build a `PLAN_LIMIT_EXCEEDED` failure tagged with a [`LimitDimension`].
    #[must_use]
    pub fn plan_limit(dim: LimitDimension, detail: impl Into<Box<str>>) -> Self {
        Self {
            code: HistoryErrorCode::PlanLimitExceeded,
            dimension: Some(dim),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.dimension {
            Some(d) => write!(f, "{}[dimension={}]: {}", self.code, d, self.detail),
            None => write!(f, "{}: {}", self.code, self.detail),
        }
    }
}

impl core::error::Error for HistoryError {}

#[cfg(test)]
mod tests {
    use super::{HistoryError, HistoryErrorCode, LimitDimension};

    const ALL_CODES: &[HistoryErrorCode] = &[
        HistoryErrorCode::HistoryRefNotFound,
        HistoryErrorCode::HistoryRangeOverrun,
        HistoryErrorCode::HistoryMergeCycle,
        HistoryErrorCode::HistoryTraceIncomplete,
        HistoryErrorCode::HistoryUnindexed,
        HistoryErrorCode::PlanLimitExceeded,
        HistoryErrorCode::StateGenerationRegression,
        HistoryErrorCode::InvalidGeneration,
        HistoryErrorCode::HistoryCommitParentUnknown,
        HistoryErrorCode::HistoryCommitHasChildren,
    ];

    const ALL_DIMS: &[LimitDimension] = &[
        LimitDimension::ParentDepth,
        LimitDimension::RevisionsCount,
        LimitDimension::TagPattern,
    ];

    #[test]
    fn code_strs_are_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for c in ALL_CODES {
            let s = c.as_code_str();
            assert!(!seen.contains(&s), "duplicate code: {s}");
            seen.push(s);
        }
    }

    #[test]
    fn code_strs_roundtrip() {
        for c in ALL_CODES {
            assert_eq!(HistoryErrorCode::from_code_str(c.as_code_str()), Some(*c));
        }
    }

    #[test]
    fn code_unknown_returns_none() {
        assert!(HistoryErrorCode::from_code_str("NOT_A_CODE").is_none());
        assert!(HistoryErrorCode::from_code_str("").is_none());
    }

    #[test]
    fn dim_strs_roundtrip() {
        for d in ALL_DIMS {
            assert_eq!(LimitDimension::from_code_str(d.as_code_str()), Some(*d));
        }
    }

    #[test]
    fn display_carries_dimension_when_set() {
        let e = HistoryError::plan_limit(LimitDimension::ParentDepth, "too deep");
        let s = format!("{e}");
        assert!(s.contains("PLAN_LIMIT_EXCEEDED"));
        assert!(s.contains("parent-depth"));
        assert!(s.contains("too deep"));
    }

    #[test]
    fn display_omits_dimension_when_absent() {
        let e = HistoryError::new(HistoryErrorCode::HistoryRefNotFound, "no ref");
        let s = format!("{e}");
        assert!(s.contains("HISTORY_REF_NOT_FOUND"));
        assert!(!s.contains("dimension="));
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in ALL_CODES {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(c, &mut buf);
            assert!(w.is_ok(), "serialize failed for {c:?}");
            let read: Result<HistoryErrorCode, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *c),
                Err(e) => assert!(false, "deserialize failed for {c:?}: {e}"),
            }
        }
    }

    #[test]
    fn dim_serde_roundtrip_via_ciborium() {
        for d in ALL_DIMS {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(d, &mut buf);
            assert!(w.is_ok());
            let read: Result<LimitDimension, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *d),
                Err(e) => assert!(false, "deserialize failed for {d:?}: {e}"),
            }
        }
    }
}
