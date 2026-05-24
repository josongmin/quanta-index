//! Typed errors for the RT-01 runtime metadata + `dirty:` channel.
//!
//! Every failure across the per-tenant per-repo dirty buffer maps to exactly
//! one [`RuntimeErrorCode`] variant. No silent fallback; no silent default.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of runtime-engine failures emitted by the dirty buffer
/// and snapshot resolution rails.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuntimeErrorCode {
    /// `apply_changes` packet was submitted against a generation older than
    /// the currently-active generation pinned to the buffer.
    DirtyStaleGen,
    /// The per-(tenant, repo) dirty buffer capacity cap would be exceeded
    /// by this `apply_changes` packet. No partial accept; reject and retry.
    DirtyBufferFull,
    /// Every entry visible to a `dirty:` read has TTL-expired; the buffer is
    /// effectively empty until the producer re-publishes.
    DirtyTtlExpired,
    /// `DirtyEntry.tenant_id` / `repo_id` does not align with the buffer
    /// the packet was routed to.
    DirtyBadIdentity,
    /// Authority is absent (snapshot not yet known, snapshot reaped,
    /// metadata missing, semantic derivative not yet built). Carries a
    /// [`StateNotReadyReason`] tag for the precise sub-cause.
    StateNotReady,
    /// `BufferConfig` construction received an invalid parameter (e.g. zero
    /// capacity). Constructor-side error; the dirty buffer is never built
    /// with a degenerate config that would silently disable capacity caps.
    InvalidBufferConfig,
}

impl RuntimeErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::DirtyStaleGen => "DIRTY_STALE_GEN",
            Self::DirtyBufferFull => "DIRTY_BUFFER_FULL",
            Self::DirtyTtlExpired => "DIRTY_TTL_EXPIRED",
            Self::DirtyBadIdentity => "DIRTY_BAD_IDENTITY",
            Self::StateNotReady => "STATE_NOT_READY",
            Self::InvalidBufferConfig => "INVALID_BUFFER_CONFIG",
        }
    }

    /// Inverse of [`RuntimeErrorCode::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "DIRTY_STALE_GEN" => Self::DirtyStaleGen,
            "DIRTY_BUFFER_FULL" => Self::DirtyBufferFull,
            "DIRTY_TTL_EXPIRED" => Self::DirtyTtlExpired,
            "DIRTY_BAD_IDENTITY" => Self::DirtyBadIdentity,
            "STATE_NOT_READY" => Self::StateNotReady,
            "INVALID_BUFFER_CONFIG" => Self::InvalidBufferConfig,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for RuntimeErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for RuntimeErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for RuntimeErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = RuntimeErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("RuntimeErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<RuntimeErrorCode, E> {
                RuntimeErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<RuntimeErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Closed taxonomy of `STATE_NOT_READY` sub-causes per RT-01 § 8.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StateNotReadyReason {
    /// `snapshot:<name>` not in catalog.
    SnapshotUnknown,
    /// Named historical snapshot reaped per retention policy.
    SnapshotReaped,
    /// Ownership registry row absent for a requested `meta.*` key.
    MetadataMissing,
    /// `affected:` / `invalidated_by:` invoked before SEM-02 lands.
    SemanticDerivativeUnbuilt,
}

impl StateNotReadyReason {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::SnapshotUnknown => "SNAPSHOT_UNKNOWN",
            Self::SnapshotReaped => "SNAPSHOT_REAPED",
            Self::MetadataMissing => "METADATA_MISSING",
            Self::SemanticDerivativeUnbuilt => "SEMANTIC_DERIVATIVE_UNBUILT",
        }
    }

    /// Inverse of [`StateNotReadyReason::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "SNAPSHOT_UNKNOWN" => Self::SnapshotUnknown,
            "SNAPSHOT_REAPED" => Self::SnapshotReaped,
            "METADATA_MISSING" => Self::MetadataMissing,
            "SEMANTIC_DERIVATIVE_UNBUILT" => Self::SemanticDerivativeUnbuilt,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for StateNotReadyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for StateNotReadyReason {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for StateNotReadyReason {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = StateNotReadyReason;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("StateNotReadyReason SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<StateNotReadyReason, E> {
                StateNotReadyReason::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<StateNotReadyReason>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete runtime-engine failure.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RuntimeError {
    pub code: RuntimeErrorCode,
    pub state_reason: Option<StateNotReadyReason>,
    pub detail: Box<str>,
}

impl RuntimeError {
    /// Build a typed runtime failure without a `STATE_NOT_READY` sub-cause.
    #[must_use]
    pub fn new(code: RuntimeErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            state_reason: None,
            detail: detail.into(),
        }
    }

    /// Build a `STATE_NOT_READY` failure carrying a [`StateNotReadyReason`].
    #[must_use]
    pub fn state_not_ready(reason: StateNotReadyReason, detail: impl Into<Box<str>>) -> Self {
        Self {
            code: RuntimeErrorCode::StateNotReady,
            state_reason: Some(reason),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.state_reason {
            Some(r) => write!(f, "{}[reason={}]: {}", self.code, r, self.detail),
            None => write!(f, "{}: {}", self.code, self.detail),
        }
    }
}

impl core::error::Error for RuntimeError {}

#[cfg(test)]
mod tests {
    use super::{RuntimeError, RuntimeErrorCode, StateNotReadyReason};

    const ALL_CODES: &[RuntimeErrorCode] = &[
        RuntimeErrorCode::DirtyStaleGen,
        RuntimeErrorCode::DirtyBufferFull,
        RuntimeErrorCode::DirtyTtlExpired,
        RuntimeErrorCode::DirtyBadIdentity,
        RuntimeErrorCode::StateNotReady,
        RuntimeErrorCode::InvalidBufferConfig,
    ];

    const ALL_REASONS: &[StateNotReadyReason] = &[
        StateNotReadyReason::SnapshotUnknown,
        StateNotReadyReason::SnapshotReaped,
        StateNotReadyReason::MetadataMissing,
        StateNotReadyReason::SemanticDerivativeUnbuilt,
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
            assert_eq!(RuntimeErrorCode::from_code_str(c.as_code_str()), Some(*c));
        }
    }

    #[test]
    fn code_unknown_returns_none() {
        assert!(RuntimeErrorCode::from_code_str("NOT_A_CODE").is_none());
        assert!(RuntimeErrorCode::from_code_str("").is_none());
    }

    #[test]
    fn reason_strs_roundtrip() {
        for r in ALL_REASONS {
            assert_eq!(
                StateNotReadyReason::from_code_str(r.as_code_str()),
                Some(*r),
            );
        }
    }

    #[test]
    fn display_carries_reason_when_set() {
        let e = RuntimeError::state_not_ready(StateNotReadyReason::SnapshotUnknown, "no snapshot");
        let s = format!("{e}");
        assert!(s.contains("STATE_NOT_READY"));
        assert!(s.contains("SNAPSHOT_UNKNOWN"));
        assert!(s.contains("no snapshot"));
    }

    #[test]
    fn display_omits_reason_when_absent() {
        let e = RuntimeError::new(RuntimeErrorCode::DirtyStaleGen, "old gen");
        let s = format!("{e}");
        assert!(s.contains("DIRTY_STALE_GEN"));
        assert!(!s.contains("reason="));
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in ALL_CODES {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(c, &mut buf);
            assert!(w.is_ok(), "serialize failed for {c:?}");
            let read: Result<RuntimeErrorCode, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *c),
                Err(e) => assert!(false, "deserialize failed for {c:?}: {e}"),
            }
        }
    }

    #[test]
    fn reason_serde_roundtrip_via_ciborium() {
        for r in ALL_REASONS {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(r, &mut buf);
            assert!(w.is_ok());
            let read: Result<StateNotReadyReason, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *r),
                Err(e) => assert!(false, "deserialize failed for {r:?}: {e}"),
            }
        }
    }
}
